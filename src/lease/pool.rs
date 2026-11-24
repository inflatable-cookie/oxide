//! Exclusive pool: short-lived flock, durable JSON ledger, idle-only eviction.

/// Entry points for the `oxide` front door. They open a budget-only
/// directory or removable-volume pool; a pool recording the retired APFS
/// sparse-image backend refuses with [`LeaseError::BackingMismatch`].
pub mod managed;

mod allocate;
mod held_eviction;
mod layout;
mod ledger;
mod lock;
mod open;
mod pending;
mod recover;
mod seed_view;

#[cfg(test)]
mod barge_label_tests;
#[cfg(test)]
mod injected;
#[cfg(test)]
mod lock_wait_tests;
#[cfg(test)]
mod reserve_tests;

use super::admission_cost::{self, Phase, PoolLock};
use super::backing::BackingPaths;
use super::{
    canonical_existing_dir, current_uid, ensure_private_dir, io_error, is_finder_metadata,
    is_tolerated_metadata_file, persist_private_file, random_hex, read_private_file,
    unexpected_entry, AccountingCoverage, AccountingScan, LeaseEnvironment, LeaseError, LeaseGrant,
    Leased, LedgerSlot, LedgerView, MeasuredPool, PassiveSnapshot, PoolStatus, Profile,
    RecordedFact, RecordedVolume, ReleaseReceipt, SlotOccupancy, SlotStatus, UnmeasuredSlot,
    ACCOUNTING_MAX_ELAPSED_MS, CAPACITY_POLICY_VERSION, CONFIG_NAME, LEASE_VERSION, LEDGER_NAME,
    LEDGER_VERSION, LEGACY_CAPACITY_POLICY_VERSION, LEGACY_MAX_SLOTS, LOCK_NAME, MANAGED_NAME,
    MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_PATH_BYTES, MAX_SCRATCH_PATH_BYTES, MAX_SLOTS,
    PREVIOUS_CAPACITY_POLICY_VERSION, RAISED_CAPACITY_POLICY_VERSION, SLOTS_NAME,
};
use super::{
    check_control_writer, fence_managed_control, require_absolute, require_bounded_text,
    IMAGE_NAME, PAYLOAD_NAME, RECEIPT_NAME,
};
use super::{BUDGET_BACKEND_DIRECTORY_V1, VOLUME_BACKEND_APFS_V1};
use crate::fs_util::open_directory_at;
use crate::volume::Binding;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use allocate::*;
#[cfg(test)]
use injected::*;
use layout::*;
use ledger::*;
use lock::*;
use open::*;
use recover::*;

// Items the lease module and its tests reach through `pool::`.
#[cfg(test)]
pub(super) use allocate::check_ample_reserve;
#[cfg(test)]
pub(super) use injected::{
    create_injected, fail_next_adoption, lease_injected, prune_injected, release_injected,
    rewrite_removable_backend_injected, tamper_idle_generations_injected, with_passive_open_delay,
};
#[cfg(test)]
pub(super) use lock::{fail_next_trash_sweep, last_settle_held_no_pool_lock};
pub(super) use open::seed_record_matches;
pub(super) use seed_view::{open_seed, open_seed_writer, SeedPoolView, SeedSlotView};

/// Where a refused entry sits, for [`LeaseError::UnexpectedEntry`].
const POOL_PLACE: &str = "pool directory";
const PAYLOAD_PLACE: &str = "payload directory";
const LEGACY_ADOPTION_BATCH: usize = 8;

#[derive(Clone, Debug)]
pub struct LeaseRequest {
    pub pool: PathBuf,
    pub project: PathBuf,
    pub workspace: PathBuf,
    pub profile: Profile,
    pub holder: String,
    /// The soft budget for this admission. `None` uses the budget recorded
    /// at creation. It drives admission and whole-idle-slot eviction only;
    /// nothing enforces it while a build writes.
    pub budget_bytes: Option<u64>,
    /// The registered removable volume's free-space reserve, separate from
    /// the pool budget. Below reserve, admission tries idle slots until free
    /// space recovers or no eligible root remains. The floor decides refusal.
    pub reserve_bytes: Option<u64>,
    /// Presentation label facts for a new slot's Barge root.
    pub layout: Option<SlotLayout>,
    /// A front-door **count** replacement, never byte pressure: retire this
    /// idle whole slot as part of this admission, before the new slot is
    /// allocated. Count pressure is authorised separately from the recorded
    /// byte figure — a complete pool figure, a removable pool's bound-only
    /// unknown, or an internal pool's conservative charge for a slot whose
    /// walk stopped short (oxide#186), all allow it — while a recorded
    /// byte-pressure admission takes
    /// the ordinary LRU path instead. The front door chooses the victim by
    /// recorded rank; the library rechecks its identity, ownership, occupancy
    /// and accounting under the pool lock.
    pub replace_slot: Option<u64>,
}

/// Presentation label facts of a new slot's Barge root. The root's recorded
/// folder and id come from Barge; the slot key, identity and ledger semantics
/// do not depend on them.
#[derive(Clone, Debug, Default)]
pub struct SlotLayout {
    /// Project folder names, preferred first. The first one labels the new
    /// root; when none is usable the profile alone does.
    pub folders: Vec<String>,
    /// The Oxide profile label.
    pub profile: String,
}

/// Longest project folder or leaf name.
const MAX_SLOT_NAME_BYTES: usize = 80;

/// The retired holding area's name, still tolerated in an older pool's payload
/// entry set so a pre-adoption pool keeps validating. Nothing is written there.
const HOLDING_NAME: &str = ".oxide-holding";

/// One plain directory name: lowercase ASCII letters, digits, `-`, `_` and
/// `.`, not starting with `.` (so never `.`, `..`, hidden or Finder
/// metadata).
fn valid_slot_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SLOT_NAME_BYTES
        && !name.starts_with('.')
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

/// A stable Barge root id for an existing Oxide slot folder: a hex digest of
/// the recorded folder, so the same folder always adopts to the same identity
/// and a repeat adoption is idempotent.
fn legacy_root_id(folder: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(folder.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Longest Barge root label that keeps `<slots>/<label>-<16 hex>/scratch`
/// within [`MAX_SCRATCH_PATH_BYTES`] (oxide#376): the slots directory, one
/// `/`, Barge's `-` plus 16-hex id, and `/scratch`. Capped at Barge's own
/// 32-byte label limit. A zero budget still yields 1 so a new slot gets a
/// valid label and the child-temporary decision keeps refusing the genuinely
/// overlong pool path instead of this label.
pub(super) fn max_barge_label_bytes(slots_dir: &Path) -> usize {
    let overhead = slots_dir
        .as_os_str()
        .as_bytes()
        .len()
        .saturating_add(1)
        .saturating_add(17)
        .saturating_add(8);
    MAX_SCRATCH_PATH_BYTES
        .saturating_sub(overhead)
        .min(32)
        .max(1)
}

/// The Barge label for a new slot root: the project folder and profile
/// squeezed into one label (lowercase letters, digits and `-`, at most
/// `max_bytes`, no trailing `-`). The root's own random id follows it, so
/// labels repeat freely. `max_bytes` comes from [`max_barge_label_bytes`];
/// values above 32 still cap at Barge's own limit, and a zero budget falls
/// back to one byte so the label stays valid.
fn barge_label(layout: Option<&SlotLayout>, max_bytes: usize) -> String {
    let max_bytes = max_bytes.max(1).min(32);
    let profile = layout
        .map(|layout| layout.profile.as_str())
        .unwrap_or("slot");
    let folder = layout
        .and_then(|layout| layout.folders.first())
        .map(String::as_str);
    let mut label = match folder {
        Some(folder) if !folder.is_empty() => format!("{folder}-{profile}"),
        _ => profile.to_owned(),
    };
    label = label
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    // The sanitised label is ASCII only, so a byte truncation cannot split
    // a character.
    label.truncate(max_bytes);
    while label.ends_with('-') || label.is_empty() {
        if label.is_empty() {
            label.push_str("slot");
            label.truncate(max_bytes);
            break;
        }
        label.pop();
    }
    label
}

/// A recorded slot directory: exactly `<folder>/<profile>-<id>` with plain
/// names. Anything else refuses the ledger.
fn split_slot_dir(dir: &str, id: u64) -> Option<(&str, &str)> {
    match dir.split_once('/') {
        Some((folder, leaf)) => {
            let profile = leaf.strip_suffix(&format!("-{id}"))?;
            (valid_slot_name(folder) && valid_slot_name(leaf) && !profile.is_empty())
                .then_some((folder, leaf))
        }
        // A Barge root folder is one level and carries the library's own id,
        // not the numeric slot id: `<label>-<16 hex>`.
        None => (valid_slot_name(dir) && !dir.starts_with('.')).then_some(("", dir)),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PoolConfig {
    version: u32,
    backend: String,
    store_id: String,
    cap_bytes: u64,
    image_bytes: u64,
    /// The removable volume of an `apfs-volume-v1` pool. Absent for every
    /// other backend, so their config bytes are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume: Option<VolumeConfig>,
}

/// Where an `apfs-volume-v1` pool keeps its payload: `payload_dir` directly
/// under the mount root of the volume with this UUID. Config, ledger and
/// lock stay in the internal pool directory.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VolumeConfig {
    mount_point: PathBuf,
    volume_uuid: String,
    payload_dir: String,
}

const LEGACY_LEDGER_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    store_id: String,
    cap_bytes: u64,
    next_slot: u64,
    next_lease: u64,
    /// Permanent revised-capacity fence (oxide#118). Absent means the
    /// legacy representation; once written it is never removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity_policy: Option<u32>,
    /// The pool's monotonic idle-rank sequence. Legacy ledgers keep no
    /// sequence and every record's rank is 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_rank: Option<u64>,
    /// Read-tolerant legacy key from the retired holding journal (oxide#190).
    /// This version never writes or acts on it, and a pool that never evicted
    /// keeps the exact bytes an older binary wrote; an older writer's rows are
    /// kept as inert values so the ledger still parses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    holding: Vec<serde_json::Value>,
    slots: Vec<SlotRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SlotRecord {
    id: u64,
    incarnation: u64,
    state: SlotPersistState,
    project: PathBuf,
    workspace: PathBuf,
    profile: Profile,
    holder: String,
    lease_id: Option<String>,
    last_lease_id: String,
    /// Slot directory below `slots`, fixed at creation; absent for the
    /// legacy `<id>` path, so a legacy ledger reads and writes unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dir: Option<String>,
    /// The Barge root id this slot lives in (oxide#314). Absent for a legacy
    /// Oxide slot directory, which Barge does not own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<String>,
    /// This row has completed the Barge transition. On a held row it means
    /// the grant took a Barge hold, so recovery can test that hold.
    #[serde(default, skip_serializing_if = "is_false")]
    owned: bool,
    /// Idle rank from this pool's monotonic sequence (oxide#118). Legacy
    /// records keep 0 and sort oldest.
    #[serde(default, skip_serializing_if = "is_zero_rank")]
    rank: u64,
    /// A permanently refused legacy path is retained outside Barge custody.
    #[serde(default, skip_serializing_if = "is_false")]
    adoption_refused: bool,
    /// A logical-byte observation bound to the lease generation. The former
    /// allocated-byte field can still be present in an older ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded: Option<serde_json::Value>,
    // Read-tolerant retired deletion journal fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hold_intent: Option<serde_json::Value>,
}

impl SlotRecord {
    fn area_folder(&self) -> Option<&str> {
        self.dir
            .as_deref()?
            .split_once('/')
            .map(|(folder, _)| folder)
    }

    fn is_idle(&self) -> bool {
        self.state == SlotPersistState::Idle && self.lease_id.is_none()
    }

    fn recorded_size(&self) -> Option<RecordedFact> {
        let record = self.recorded.as_ref()?;
        if self.state == SlotPersistState::Evicting
            || record.get("for_lease")?.as_str()? != self.last_lease_id
        {
            return None;
        }
        Some(RecordedFact {
            logical_bytes: record.get("logical_bytes")?.as_u64()?,
            entries: record.get("entries")?.as_u64()?,
            at_ms: record.get("at_ms")?.as_u64()?,
            current: true,
        })
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero_rank(rank: &u64) -> bool {
    *rank == 0
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SlotPersistState {
    Held,
    Idle,
    Evicting,
}

/// Where slots live. Neither payload is an OS-enforced bound.
enum Payload {
    Budget(PathBuf),
    /// A directory on a verified removable volume. Barge creates every slot
    /// root through the retained binding, never by path.
    Volume {
        binding: Binding,
        root: PathBuf,
    },
}

impl Payload {
    fn payload_root(&self) -> &Path {
        match self {
            Self::Budget(root) | Self::Volume { root, .. } => root,
        }
    }
}

/// What one idle-eviction attempt decided.
enum Evicted {
    /// No idle candidate was eligible.
    None,
    /// One row is retired and its root is gone, or its deletion is handed to a
    /// settle pass that runs with the pool lock released.
    Retired,
    /// One row is retired and Barge's committed rename left the trash copy in
    /// place. This is progress, not a refusal: free space is re-read and the
    /// kept tree is reported.
    KeptTrash(String),
}

pub(super) struct Pool {
    /// The exclusive pool lock, held for the life of a mutating open. A
    /// passive measurement holds none.
    _lock: Option<PoolLock>,
    root: PathBuf,
    config: PoolConfig,
    backing: Payload,
    /// The pool's Barge area. `None` for a passive measurement, which creates
    /// and writes nothing.
    storage: Option<crate::storage::PoolStorage>,
    /// The hold this admission took, handed to the caller with the grant so it
    /// lives as long as the command.
    pending_hold: Option<barge::Hold>,
    /// A passive measurement's absolute deadline; a mutating open has none.
    deadline: Option<std::time::Instant>,
    ledger: Ledger,
    /// Admission budget: the recorded one unless the request overrides it.
    budget: u64,
    /// A bounded note the caller prints when admission had to report a policy
    /// decision (for example a fallback or a warning).
    admission_note: Option<String>,
    /// Trash a committed eviction could not delete. It is progress, not a
    /// refusal, so it is reported rather than failing the command.
    eviction_notes: Vec<String>,
    /// This open could not resolve a recorded deletion or a legacy adoption,
    /// so it kept that object out of service instead of refusing every
    /// command. [`LedgerView::reconciliation_blocked`] reports it.
    reconciliation_blocked: bool,
}
