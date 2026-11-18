//! Library custody for cold-slot seeding (`oxide#137` §15 slice 2).
//!
//! The front door keeps Cargo semantics; this module keeps custody. It exposes
//! the two calls of §6.3 and one sidecar writer:
//!
//! - [`seed_candidates`] is call 1: it takes the pool lock **alone**, validates
//!   that the recipient is still held by the caller's lease, and returns the
//!   bounded catalogs of at most eight donor slots by rank. It reads control
//!   state only: no donor payload is opened and nothing is mutated.
//! - [`seed`] is call 2: it takes the donor's record lock — derived from the
//!   pool's control directory and the plan's record id, so it is exactly that
//!   record's lock and not a caller-supplied path — non-blocking, and then the
//!   pool lock, both held to the end. Under them it re-reads and re-checks the
//!   selected catalog (record id, generation, lease id and bytes), checks the
//!   §7 control record, revalidates every planned donor file against the
//!   catalog's recorded digest and its `.d` join against a fresh parse of the
//!   donated rules, clones with `clonefile` only, hashes each clone against the
//!   same digest, re-reads both ledger generations, and then walks the
//!   recipient as an `observed-while-held` fact before publishing anything.
//! - [`publish_seed_catalog`] writes one generation-bound catalog sidecar
//!   under `<pool>/oxide-managed/seeds/` after bounded orphan and family-size
//!   checks. It is called by the producing lease, not by [`seed`].
//!
//! Failure follows §6.5 exactly:
//!
//! - **Before the first `clonefile` that creates an entry** a missing donor, no
//!   overlap, a cap, a changed catalog or file, a directory-creation fault, a
//!   first clone that fails to be created or the cooperative deadline is a
//!   **skip**: no clone exists, the attempt is abandoned and the caller builds
//!   cold in the same slot. (`SeedSkip`.) A donor path that is not an owned
//!   no-follow entry on the pool's device is not a skip: unknown ownership,
//!   a device mismatch or a link where a real entry belongs **refuses**.
//! - **After it, a failed recipient walk** abandons seeding only: nothing is
//!   published, the clone is retained untrusted, and the caller releases the
//!   slot and cold-builds under ordinary admission. (`SeedAbandon`.)
//! - **After it, every other fault refuses and retains**: no deletion, no
//!   retry, no second lease. (`Err`.)
//!
//! The first **recipient write** — creating the `build/` directory, creating a
//! destination directory, or `clonefile` itself — is the point of no return:
//! before any write a benign fault abandons and the same slot builds cold; from
//! the first write onward every fault refuses and retains the untrusted
//! generation. A **safety refusal** — unknown ownership, a wrong device, a
//! mismatched backing, unknown custody, or an entry that appeared where the
//! seed would create one — refuses on either side of that boundary, because a
//! safety fault is never disguised as a cold fallback.
//!
//! **Both pool backends.** Slice 4 writes a `directory-budget-v1` pool's
//! payload and a registered `apfs-volume-v1` volume's payload. For the
//! removable backend call 2 opens the payload through the same strict mount
//! and volume-UID binding a mutating pool open uses, so a removed, remounted or
//! substituted volume refuses before any donor payload is read (§7). Call 1
//! and the sidecar writers open no payload at all. The caller's monitor owns
//! the removable removal stop and the internal reserve stop (§8.3); this
//! module makes no volume or reserve decision of its own.
//!
//! The donor predicate of §7 has two halves and both are checked here. The
//! ledger half is state `Idle`, no lease, the catalog's lease as
//! `last_lease_id`, the same payload root and device, and the donor slot's
//! recorded project equal to the pool's `oxide-managed/affinity/<record id>`
//! for the catalog's own record id — that affinity is what binds a slot to its
//! front-door record, so a catalog can never select another idle record's
//! lock. The control-record half is read from the front door's own
//! `oxide-managed/state.json`: the record the catalog names must exist, be
//! `idle`, carry no holder, lease id, pending recovery, retirement or lost
//! marker, and its checkout must have no recorded `oxide clean`. The file is
//! read bounded and tolerantly; a missing, unreadable or unknown-version
//! document is unknown custody, which is a skip and never a repair. The donor
//! record lock itself is derived from that same record id, never supplied by
//! the caller.
//!
//! **No reconciliation.** Calls 1 and 2 and the catalog write open the pool
//! through the non-reconciling seam: a recorded `Evicting` slot stays exactly
//! as the ledger records it, and no deletion or unrelated ledger/payload
//! mutation can happen here. That keeps this slice library custody under the
//! Standard tier: nothing in this module decides crash recovery, recycles
//! storage or writes removable media.
//!
//! Nothing here reads the registry adapter, a source tree, an archive or a
//! Cargo home: slice 1's verification evidence reaches this layer only as the
//! catalog's recorded source and file digests, and donation re-hashes the
//! donor's own bytes against them. Source re-verification is the front door's
//! (§6.3 step 4).

pub mod catalog;
mod join;
mod walk;

pub use catalog::{
    digest as catalog_digest, SeedAttemptState, SeedCatalog, SeedFile, SeedFileKind, SeedJoin,
    SeedPackage, SeedProducer, SeedRejectReason, SeedRejection, SeedRole, SeedSourceKind,
    SeedSourceRoot, SeedUnit, SEED_CATALOG_MAX_BYTES, SEED_CATALOG_SCHEMA, SEED_MAX_FILES,
    SEED_MAX_PACKAGES, SEED_MAX_PLAN_INPUTS, SEED_MAX_REJECTED_UNITS, SEED_MAX_REJECTIONS,
    SEED_MAX_UNITS_PER_PACKAGE, SEED_MAX_UNIT_INPUTS,
};
pub use join::{
    output_rule_proven as seed_output_rule_proven, parse as parse_seed_join, JoinRefusal,
    ParsedJoin,
};
pub use walk::{WalkGap as SeedWalkGap, WalkLimits as SeedWalkLimits};

use super::pool::{open_seed, open_seed_payload, SeedPoolView, SeedSlotView};
use super::{
    current_uid, ensure_private_dir, io_error, random_hex, LeaseError, MANAGED_NAME,
    MAX_LEASE_ID_BYTES, MAX_PATH_BYTES,
};
use crate::fs_util::open_directory_at;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Donor slots examined per command: chosen by most recent rank, then slot id.
pub const SEED_MAX_CANDIDATES: usize = 8;
/// Compile units per plan.
pub const SEED_MAX_PLAN_UNITS: usize = 256;
/// Files per plan.
pub const SEED_MAX_PLAN_FILES: usize = 4096;
/// Logical bytes per plan.
pub const SEED_MAX_PLAN_BYTES: u64 = 384 * 1024 * 1024;
/// The seed's cooperative work deadline (clone plus digest verify): 5 s, a
/// ceiling a caller may lower but never raise (§8.1).
pub const SEED_WORK_BUDGET_MS: u64 = 5_000;
/// Legacy front-door version this module can classify; version 7 is accepted
/// only with its exact eviction fence and no one-use fields.
const CONTROL_STATE_VERSION: u32 = 5;
/// The control state's whole-file bound, matching the front door's own
/// (32 MiB since oxide#177's 512-record, 256-slot ceiling).
const CONTROL_STATE_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// The post-clone recipient walk's bounds: the capacity contract's slot walk.
pub const SEED_WALK_MAX_ENTRIES: u64 = 2_000_000;
pub const SEED_WALK_MAX_DEPTH: usize = 64;
pub const SEED_WALK_MAX_ELAPSED_MS: u64 = 60_000;
/// All seed sidecars together (catalog, attempt, last, sources).
pub const SEED_SIDECAR_FAMILY_MAX_BYTES: u64 = 24 * 1024 * 1024;
/// Sidecars one write may remove because their slot left the ledger.
pub const SEED_ORPHAN_CLEANUP_MAX: usize = 8;
/// Catalogs one write may drop to stay inside the family byte ceiling.
pub const SEED_FAMILY_DROP_MAX: usize = 8;

const SEEDS_DIR: &str = "seeds";
const CATALOG_SUFFIX: &str = ".catalog.json";

/// One pool slot generation a caller holds. All three fields are required:
/// a lease id alone cannot name a generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedRecipient {
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
}

/// The donor the caller selected from call 1's candidates, named by the exact
/// catalog bytes it planned from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedDonor {
    pub slot_id: String,
    pub incarnation: String,
    /// The producing lease, which is the ledger's `last_lease_id` while the
    /// catalog is eligible.
    pub lease_id: String,
    /// The producing record id (from the catalog call 1 returned). It is also
    /// the front door record whose lock this call takes and whose control
    /// record it verifies, so the lock cannot be an unrelated one.
    pub record_id: u64,
    /// Lowercase SHA-256 of the catalog bytes call 1 returned.
    pub catalog_digest: String,
}

/// One unit the caller selected from the catalog's recorded data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedUnitRef {
    /// The unit's `fingerprint_dir`, relative to the slot's `build/`.
    pub fingerprint_dir: String,
    pub role: SeedRole,
}

/// One candidate donor from call 1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedCandidate {
    pub slot_id: String,
    pub incarnation: String,
    pub rank: u64,
    pub catalog_digest: String,
    pub catalog: SeedCatalog,
}

/// Call 1's bounded answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedCandidates {
    pub recipient: SeedRecipient,
    pub candidates: Vec<SeedCandidate>,
}

/// Call 2's input. The plan names catalog units, not caller paths: the library
/// derives every donor and recipient path from the re-read catalog itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedRequest {
    pub recipient: SeedRecipient,
    pub donor: SeedDonor,
    pub units: Vec<SeedUnitRef>,
    /// The cooperative work deadline; `None` uses [`SEED_WORK_BUDGET_MS`] and
    /// any value is clamped to that ceiling.
    pub work_budget_ms: Option<u64>,
}

/// Call 2's outcome: published, skipped cold, or abandoned after the walk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeedOutcome {
    Seeded(SeedReceipt),
    Skipped(SeedSkip),
    Abandoned(SeedAbandon),
}

/// The published seed, returned while the pool lock and the donor shared hold were
/// still held through the final donor check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedReceipt {
    pub donor_slot: String,
    pub donor_incarnation: String,
    pub units: u64,
    pub files: u64,
    pub logical_bytes: u64,
    pub catalog_digest: String,
    /// The `observed-while-held` fact the caller records with the attempt.
    pub observation: SeedObservation,
    pub elapsed_ms: u64,
}

/// A cold skip: nothing was written to the recipient.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedSkip {
    pub reason: SeedSkipReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedSkipReason {
    /// No donor slot had an eligible catalog.
    NoCandidate,
    DonorBusy,
    DonorLockMissing,
    /// The donor's control record is missing, unreadable, of an unknown state
    /// version, or carries a pending recovery, retirement or lost marker:
    /// its custody is unknown.
    DonorCustodyUnknown,
    /// The donor's ledger generation no longer matches the planned catalog.
    DonorGenerationChanged,
    /// The catalog bytes call 2 re-read differ from the plan's.
    CatalogChanged,
    /// The donor's `build/` tree is gone.
    DonorPayloadMissing,
    /// A planned unit is not in the re-read catalog.
    UnitMissing,
    /// A plan cap is exceeded.
    PlanCap,
    /// The seed's own cooperative deadline expired before the first write.
    WorkDeadline,
    /// A planned donor file is missing, unreadable or no longer matches its
    /// recorded digest.
    FileChanged,
    /// The donated `.d` parses to different rule targets or inputs than the
    /// catalog recorded: the recorded join is internally inconsistent.
    JoinInconsistent,
    /// The first `clonefile` (or the directory creation before it) failed:
    /// no clone was created, so the attempt is abandoned and the same slot
    /// still builds cold (§6.5's point of no return is the first clone).
    CloneFailed,
    /// This platform has no `clonefile`.
    Unsupported,
}

impl SeedSkipReason {
    /// The bounded reason token a caller prints after `seeding skipped: `.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoCandidate => "no donor",
            Self::DonorBusy => "donor busy",
            Self::DonorLockMissing => "donor lock missing",
            Self::DonorCustodyUnknown => "donor custody unknown",
            Self::DonorGenerationChanged => "donor generation changed",
            Self::CatalogChanged => "catalog changed",
            Self::DonorPayloadMissing => "donor payload missing",
            Self::UnitMissing => "unit missing",
            Self::PlanCap => "cap",
            Self::WorkDeadline => "deadline",
            Self::FileChanged => "file changed",
            Self::JoinInconsistent => "join inconsistent",
            Self::CloneFailed => "clone failed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// A seeding attempt abandoned because the post-clone recipient walk was not
/// complete and ownership-clean. Nothing was published; the caller releases
/// the untrusted recipient and cold-builds under ordinary admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedAbandon {
    pub observation: SeedObservation,
}

/// The generation-bound `observed-while-held` walk fact. Never a closed
/// record, never an admission figure. Its byte figures apply the capacity
/// contract's within-slot inode union: a hard-linked pair inside this one slot
/// is counted once.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedObservation {
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
    pub entries: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    /// `None` when the walk was complete and ownership-clean.
    pub gap: Option<SeedWalkGap>,
    pub limits: SeedWalkLimits,
    pub elapsed_ms: u64,
}

impl SeedObservation {
    pub fn complete(&self) -> bool {
        self.gap.is_none()
    }
}

/// Durable, generation-bound intent and terminal outcome. `None` is not a
/// writable state: absence is the legacy unseeded fact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedAttempt {
    pub schema: String,
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
    pub state: SeedAttemptState,
    pub donor_slot: Option<String>,
    pub units: Option<u64>,
    pub files: Option<u64>,
    pub logical_bytes: Option<u64>,
    pub catalog_digest: Option<String>,
    pub observation: Option<SeedObservation>,
}

pub const ATTEMPT_SCHEMA: &str = "oxide.cold-seed.attempt/1";
const ATTEMPT_MAX_BYTES: usize = 4096;

impl SeedAttempt {
    fn valid(&self) -> bool {
        fn text(value: &str, cap: usize) -> bool {
            !value.is_empty() && value.len() <= cap && value.is_ascii()
        }
        if self.schema != ATTEMPT_SCHEMA
            || !text(&self.store_id, 64)
            || !text(&self.slot_id, 20)
            || !self.slot_id.bytes().all(|b| b.is_ascii_digit())
            || !text(&self.incarnation, 64)
            || !text(&self.lease_id, 160)
            || self.state == SeedAttemptState::None
        {
            return false;
        }
        if let Some(observation) = &self.observation {
            if observation.store_id != self.store_id
                || observation.slot_id != self.slot_id
                || observation.incarnation != self.incarnation
                || observation.lease_id != self.lease_id
                || observation.limits.max_entries > SEED_WALK_MAX_ENTRIES
                || observation.limits.max_depth > SEED_WALK_MAX_DEPTH
                || observation.limits.max_elapsed_ms > SEED_WALK_MAX_ELAPSED_MS
            {
                return false;
            }
        }
        if self.state == SeedAttemptState::Seeded {
            self.donor_slot
                .as_deref()
                .is_some_and(|v| text(v, 20) && v.bytes().all(|b| b.is_ascii_digit()))
                && self
                    .units
                    .is_some_and(|v| v > 0 && v <= SEED_MAX_PLAN_UNITS as u64)
                && self
                    .files
                    .is_some_and(|v| v > 0 && v <= SEED_MAX_PLAN_FILES as u64)
                && self.logical_bytes.is_some_and(|v| v <= SEED_MAX_PLAN_BYTES)
                && self.catalog_digest.as_deref().is_some_and(|v| {
                    v.len() == 64
                        && v.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
                && self
                    .observation
                    .as_ref()
                    .is_some_and(SeedObservation::complete)
        } else {
            self.donor_slot.is_none()
                && self.units.is_none()
                && self.files.is_none()
                && self.logical_bytes.is_none()
                && self.catalog_digest.is_none()
                && self.observation.is_none()
        }
    }
}

#[derive(Debug)]
enum AttemptRead {
    Absent,
    Invalid,
    Present(SeedAttempt),
}

fn read_attempt_at(pool: &Path, slot: &str) -> AttemptRead {
    let path = pool
        .join(MANAGED_NAME)
        .join(SEEDS_DIR)
        .join(format!("{slot}{ATTEMPT_SUFFIX}"));
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return AttemptRead::Absent,
        Err(_) => return AttemptRead::Invalid,
    };
    let Ok(metadata) = file.metadata() else {
        return AttemptRead::Invalid;
    };
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > ATTEMPT_MAX_BYTES as u64
    {
        return AttemptRead::Invalid;
    }
    let mut bytes = Vec::new();
    if Read::by_ref(&mut file)
        .take((ATTEMPT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > ATTEMPT_MAX_BYTES
    {
        return AttemptRead::Invalid;
    }
    match serde_json::from_slice::<SeedAttempt>(&bytes) {
        Ok(record) if record.valid() => AttemptRead::Present(record),
        _ => AttemptRead::Invalid,
    }
}

/// The current held generation's durable attempt state, with corrupt or
/// inaccessible sidecars treated as unknown rather than an unseeded fact.
pub fn held_seed_attempt(
    pool: &Path,
    recipient: &SeedRecipient,
) -> Result<SeedAttemptState, LeaseError> {
    validate_recipient(recipient)?;
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let slot = view
        .slot(&recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if !slot.held
        || slot.incarnation.to_string() != recipient.incarnation
        || slot.lease_id.as_deref() != Some(recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    match read_attempt_at(&view.pool_root, &recipient.slot_id) {
        AttemptRead::Absent => Ok(SeedAttemptState::None),
        AttemptRead::Invalid => Err(LeaseError::UnknownAccounting),
        AttemptRead::Present(record)
            if record.store_id == view.store_id
                && record.incarnation == recipient.incarnation
                && record.lease_id == recipient.lease_id =>
        {
            Ok(record.state)
        }
        AttemptRead::Present(_) => Ok(SeedAttemptState::None),
    }
}

fn catalog_unseeded(pool: &Path, catalog: &SeedCatalog) -> bool {
    match read_attempt_at(pool, &catalog.slot_id) {
        AttemptRead::Absent => catalog.attempt == SeedAttemptState::None,
        AttemptRead::Invalid => false,
        AttemptRead::Present(record)
            if record.store_id == catalog.store_id
                && record.incarnation == catalog.incarnation
                && record.lease_id == catalog.lease_id =>
        {
            record.state == catalog.attempt
                && matches!(
                    record.state,
                    SeedAttemptState::Skipped | SeedAttemptState::Reset
                )
        }
        AttemptRead::Present(_) => catalog.attempt == SeedAttemptState::None,
    }
}

/// Write intent or its terminal state while this exact generation is held.
/// The caller retains its admission lifetime around this call.
pub fn write_seed_attempt(pool: &Path, attempt: &SeedAttempt) -> Result<(), LeaseError> {
    if !attempt.valid() {
        return Err(LeaseError::InvalidInput("invalid seed attempt"));
    }
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let slot = view
        .slot(&attempt.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if view.store_id != attempt.store_id
        || !slot.held
        || slot.incarnation.to_string() != attempt.incarnation
        || slot.lease_id.as_deref() != Some(attempt.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    match read_attempt_at(&view.pool_root, &attempt.slot_id) {
        AttemptRead::Invalid => return Err(LeaseError::UnknownAccounting),
        AttemptRead::Absent if attempt.state != SeedAttemptState::Intent => {
            return Err(LeaseError::UnknownAccounting)
        }
        AttemptRead::Present(old)
            if old.store_id == attempt.store_id
                && old.incarnation == attempt.incarnation
                && old.lease_id == attempt.lease_id =>
        {
            if old.state != SeedAttemptState::Intent || attempt.state == SeedAttemptState::Intent {
                return Err(LeaseError::UnknownAccounting);
            }
        }
        AttemptRead::Present(_) if attempt.state != SeedAttemptState::Intent => {
            return Err(LeaseError::UnknownAccounting)
        }
        _ => {}
    }
    let bytes = serde_json::to_vec(attempt)
        .map_err(|_| LeaseError::InvalidInput("invalid seed attempt"))?;
    if bytes.len() > ATTEMPT_MAX_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let dir = view.pool_root.join(MANAGED_NAME).join(SEEDS_DIR);
    ensure_private_dir(&dir)?;
    remove_orphan_sidecars(&dir, view)?;
    enforce_family_bytes(&dir, bytes.len() as u64, &attempt.slot_id)?;
    write_sidecar(
        &dir.join(format!("{}{ATTEMPT_SUFFIX}", attempt.slot_id)),
        &bytes,
    )
}

/// Publish a verified-source cache for the caller's held generation. The
/// caller holds front-door admission, and this call holds the pool lock while
/// checking the shared sidecar-family cap and publishing the cache.
pub fn write_seed_sources(
    pool: &Path,
    recipient: &SeedRecipient,
    cache: &crate::seed_source::SourceCache,
) -> Result<(), LeaseError> {
    validate_recipient(recipient)?;
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let slot = view
        .slot(&recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if cache.store_id() != view.store_id
        || !slot.held
        || slot.incarnation.to_string() != recipient.incarnation
        || slot.lease_id.as_deref() != Some(recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let bytes = cache
        .encode()
        .map_err(|_| LeaseError::InvalidInput("invalid source cache"))?;
    let dir = view.pool_root.join(MANAGED_NAME).join(SEEDS_DIR);
    ensure_private_dir(&dir)?;
    let path = dir.join("sources.json");
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && metadata.uid() == current_uid()
                && metadata.mode() & 0o077 == 0
                && crate::seed_source::SourceCache::try_read(&path, &view.store_id).is_some() => {}
        Ok(_) => return Err(LeaseError::UnknownAccounting),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    remove_orphan_sidecars(&dir, view)?;
    enforce_family_bytes(&dir, bytes.len() as u64, &recipient.slot_id)?;
    cache
        .write(&path)
        .map_err(|_| LeaseError::Io("cannot publish source cache".to_owned()))
}

/// The one bounded, recorded status summary; its facts are descriptive and
/// are never used for admission or trust.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedLast {
    pub schema: String,
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
    pub donor_slot: String,
    pub units: u64,
    pub files: u64,
    pub logical_bytes: u64,
    pub elapsed_ms: u64,
    pub when_ms: u64,
    pub buckets: Vec<SeedBucket>,
    pub unknown: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedBucket {
    pub reason: SeedRejectReason,
    pub units: u32,
}

impl SeedLast {
    fn valid(&self) -> bool {
        let text = |v: &str, max: usize| !v.is_empty() && v.len() <= max && v.is_ascii();
        let numeric = |v: &str| text(v, 20) && v.bytes().all(|b| b.is_ascii_digit());
        let mut reasons = std::collections::BTreeSet::new();
        self.schema == "oxide.cold-seed/1"
            && text(&self.store_id, 64)
            && numeric(&self.slot_id)
            && numeric(&self.donor_slot)
            && text(&self.incarnation, 64)
            && text(&self.lease_id, 160)
            && self.units > 0
            && self.units <= SEED_MAX_PLAN_UNITS as u64
            && self.files > 0
            && self.files <= SEED_MAX_PLAN_FILES as u64
            && self.logical_bytes <= SEED_MAX_PLAN_BYTES
            && self.buckets.len() <= 8
            && self
                .buckets
                .iter()
                .all(|bucket| bucket.units > 0 && reasons.insert(bucket.reason))
    }
}

/// Passive bounded read: missing, malformed, symlinked and foreign sidecars
/// are ignored. The status caller still binds this record to its ledger view.
pub fn read_seed_last(pool: &Path) -> Option<SeedLast> {
    let path = pool.join(MANAGED_NAME).join(SEEDS_DIR).join("last.json");
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.uid() != current_uid()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 2048
    {
        return None;
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(2049)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 2048 {
        return None;
    }
    let record: SeedLast = serde_json::from_slice(&bytes).ok()?;
    (record.valid()
        && super::pool::seed_record_matches(
            pool,
            &record.store_id,
            &record.slot_id,
            &record.incarnation,
            &record.lease_id,
        ))
    .then_some(record)
}

/// Publish the last summary only for the caller's still-held generation.
pub fn write_seed_last(pool: &Path, record: &SeedLast) -> Result<(), LeaseError> {
    if !record.valid() {
        return Err(LeaseError::InvalidInput("invalid last seed"));
    }
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let slot = view
        .slot(&record.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if view.store_id != record.store_id
        || !slot.held
        || slot.incarnation.to_string() != record.incarnation
        || slot.lease_id.as_deref() != Some(record.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let bytes =
        serde_json::to_vec(record).map_err(|_| LeaseError::InvalidInput("invalid last seed"))?;
    if bytes.len() > 2048 {
        return Err(LeaseError::LedgerExhausted);
    }
    let dir = view.pool_root.join(MANAGED_NAME).join(SEEDS_DIR);
    ensure_private_dir(&dir)?;
    remove_orphan_sidecars(&dir, view)?;
    enforce_family_bytes(&dir, bytes.len() as u64, &record.slot_id)?;
    write_sidecar(&dir.join("last.json"), &bytes)
}

/// Call 1: read-only candidate selection under the pool lock alone.
pub fn seed_candidates(
    pool: &Path,
    recipient: &SeedRecipient,
    max_candidates: usize,
) -> Result<SeedCandidates, LeaseError> {
    validate_recipient(recipient)?;
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let held = view
        .slot(&recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if !held.held
        || held.incarnation.to_string() != recipient.incarnation
        || held.lease_id.as_deref() != Some(recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let mut donors: Vec<&SeedSlotView> = view
        .slots
        .iter()
        .filter(|slot| slot.idle && slot.lease_id.is_none())
        .collect();
    donors.sort_by(|left, right| right.rank.cmp(&left.rank).then(left.id.cmp(&right.id)));
    donors.truncate(max_candidates.clamp(1, SEED_MAX_CANDIDATES));
    let mut candidates = Vec::new();
    for slot in donors {
        let Some((catalog, bytes, catalog_digest)) = read_catalog(&view.pool_root, slot.id) else {
            continue;
        };
        // A catalog is eligible only while the ledger still names its
        // producing generation (§4.6), and a generation that seeded anything
        // never publishes one (§4.1).
        if catalog.store_id != view.store_id
            || catalog.slot_id != slot.id.to_string()
            || catalog.incarnation != slot.incarnation.to_string()
            || catalog.lease_id != slot.last_lease_id
            || !catalog_unseeded(&view.pool_root, &catalog)
            || bytes.is_empty()
        {
            continue;
        }
        candidates.push(SeedCandidate {
            slot_id: slot.id.to_string(),
            incarnation: slot.incarnation.to_string(),
            rank: slot.rank,
            catalog_digest,
            catalog,
        });
    }
    Ok(SeedCandidates {
        recipient: recipient.clone(),
        candidates,
    })
}

/// Call 2: donor record lock, then pool lock, clone and revalidate.
pub fn seed(pool: &Path, request: &SeedRequest) -> Result<SeedOutcome, LeaseError> {
    seed_monitored(pool, request, &mut || false)
}

/// Seed with the caller's reserve/admission monitor. A true result stops the
/// transaction with a refusal, including during hashing and the held walk.
pub fn seed_monitored(
    pool: &Path,
    request: &SeedRequest,
    monitor: &mut dyn FnMut() -> bool,
) -> Result<SeedOutcome, LeaseError> {
    if monitor() {
        return Err(monitor_stop());
    }
    let started = Instant::now();
    let budget = work_budget(request.work_budget_ms);
    validate_request(request)?;
    if !cfg!(target_os = "macos") {
        return Ok(SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::Unsupported,
        }));
    }
    // The donor's record lock is the front door's lock for exactly the record
    // the plan names: `<pool>/oxide-managed/locks/<record_id>.lock`. A caller
    // supplies no path, so a free unrelated lock can never stand in for the
    // donor's closure, and the catalog's own `record_id` is rechecked below.
    let pool_root = super::canonical_existing_dir(pool, true)?;
    let donor_lock_path = pool_root
        .join(MANAGED_NAME)
        .join("locks")
        .join(format!("{}.lock", request.donor.record_id));
    // Lock order (§7): the donor's record lock first, non-blocking and
    // `create: false`, then the pool lock, both held to the end.
    let _donor_lock = match probe_donor_lock(&donor_lock_path)? {
        DonorLock::Taken(file) => file,
        DonorLock::Missing => {
            return Ok(SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::DonorLockMissing,
            }))
        }
        DonorLock::Held => {
            return Ok(SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::DonorBusy,
            }))
        }
    };
    // The non-reconciling open: a recorded `Evicting` slot is never finished,
    // deleted or mutated by a seed. This is the payload seam: for a removable
    // pool it verifies the recorded mount and volume UUID under the pool lock
    // before any donor payload is opened.
    let opened = open_seed_payload(pool)?;
    let view = &opened.view;
    let payload_root = view
        .payload_root
        .as_deref()
        .ok_or(LeaseError::BackingMismatch)?;
    let recipient = view
        .slot(&request.recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if !recipient.held
        || recipient.incarnation.to_string() != request.recipient.incarnation
        || recipient.lease_id.as_deref() != Some(request.recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let Some(donor) = view.slot(&request.donor.slot_id) else {
        return Ok(skip(SeedSkipReason::DonorGenerationChanged));
    };
    if !donor.idle || donor.lease_id.is_some() {
        return Ok(skip(SeedSkipReason::DonorBusy));
    }
    if donor.incarnation.to_string() != request.donor.incarnation
        || donor.last_lease_id != request.donor.lease_id
    {
        return Ok(skip(SeedSkipReason::DonorGenerationChanged));
    }
    let Some((catalog, _bytes, actual_catalog_digest)) = read_catalog(&view.pool_root, donor.id)
    else {
        return Ok(skip(SeedSkipReason::CatalogChanged));
    };
    if actual_catalog_digest != request.donor.catalog_digest
        || catalog.store_id != view.store_id
        || catalog.slot_id != request.donor.slot_id
        || catalog.incarnation != request.donor.incarnation
        || catalog.lease_id != request.donor.lease_id
        || catalog.record_id != request.donor.record_id
        || !catalog_unseeded(&view.pool_root, &catalog)
    {
        return Ok(skip(SeedSkipReason::CatalogChanged));
    }
    // The catalog's record must be the donor slot's own record: a front-door
    // slot records its record's affinity directory as its project, so a
    // catalog naming another idle record can never select that record's lock.
    if donor.project != affinity_dir(&view.pool_root, catalog.record_id) {
        return Ok(skip(SeedSkipReason::DonorCustodyUnknown));
    }
    // The §7 control-record half: the donor's own front-door record and any
    // recorded clean of its checkout.
    match donor_control(&view.pool_root, catalog.record_id) {
        DonorControl::Eligible => {}
        DonorControl::Busy => return Ok(skip(SeedSkipReason::DonorBusy)),
        DonorControl::Unknown => return Ok(skip(SeedSkipReason::DonorCustodyUnknown)),
    }
    let payload_device = fs::symlink_metadata(payload_root).map_err(io_error)?.dev();
    // Every slot directory is opened one no-follow step at a time beneath the
    // verified payload root, so a project folder (or the slots root) replaced
    // by a symlink is refused rather than followed to another directory.
    // The returned path is for messages; the descriptor is the custody.
    let Some((donor_fd, donor_slot_path)) =
        open_donor_slot_dirs(payload_root, payload_device, donor)?
    else {
        // A donor that was evicted or whose leaf vanished before this call is
        // simply unavailable. No recipient write has happened yet, so this is
        // a cold skip, never an unsafe-path refusal for a path that no longer
        // exists. An existing foreign, linked or wrong-device entry still
        // refuses in the helper below.
        return Ok(skip(SeedSkipReason::DonorPayloadMissing));
    };
    let (recipient_fd, recipient_slot) = open_slot_dirs(payload_root, payload_device, recipient)?;
    // The pool/record locks protect workflow facts; Barge's shared root hold
    // protects the donor bytes against storage eviction for the entire seed.
    // The caller keeps the recipient's exclusive lease hold throughout.
    let Some(_donor_hold) = opened.hold_donor(donor, recipient)? else {
        return Ok(skip(SeedSkipReason::DonorBusy));
    };
    // The recipient is this command's fresh slot: its `build/` is absent or
    // an empty owned directory. Anything else is refused, never cleaned.
    let recipient_build = open_recipient_build(recipient_fd.as_fd(), payload_device)?;
    let Some(donor_build) = open_optional_directory(donor_fd.as_fd(), c"build", payload_device)?
    else {
        return Ok(skip(SeedSkipReason::DonorPayloadMissing));
    };
    // Plan from the re-read catalog's own recorded file lists.
    let mut plan = match plan_units(&catalog, &request.units) {
        Ok(plan) => plan,
        Err(reason) => return Ok(skip(reason)),
    };
    // The five-second cooperative work bound starts with donor revalidation,
    // and every read and clone step below checks it.
    let deadline = now() + budget;
    if proof_breach("verify") {
        return Err(monitor_stop());
    }
    // Debug-only oracle controls (absent from release builds; armed only under
    // the shared proof supervisor's own layout): `drop-dep` removes the
    // fingerprint `dep-…` file from the plan, and `admit-run` adds the
    // build-script run payload and run fingerprint the adapter deliberately
    // never offers. Both let the native oracle prove it distinguishes a
    // reused unit from a rerun.
    proof_adjust_plan(
        &mut plan,
        &catalog,
        &request.units,
        &donor_slot_path,
        deadline,
        monitor,
    )?;
    test_point(TestPoint::BeforeVerify { pool });
    // Every donor payload read happens here, under the shared hold: each planned file
    // is opened without following a link, checked against the catalog's
    // recorded size and revalidated against its recorded digest before
    // anything is copied. The `.d` is read bounded, hashed and re-parsed: its
    // recorded targets and inputs must really be this file's rules, so an
    // internally inconsistent catalog is refused rather than trusted.
    let mut sources = Vec::with_capacity(plan.files.len());
    for planned in &plan.files {
        if monitor() {
            return Err(monitor_stop());
        }
        if now() > deadline {
            return Ok(skip(SeedSkipReason::WorkDeadline));
        }
        let file = &planned.file;
        let opened_file = match open_relative_file(donor_build.as_fd(), &file.path, payload_device)
        {
            Ok(opened_file) => opened_file,
            // Unknown ownership, a device mismatch or a link where a real
            // entry belongs is a safety fault: refuse, never a cold fallback
            // (§13). A missing or unreadable entry stays a payload change.
            Err(error @ LeaseError::UnsafePath(_)) => return Err(error),
            Err(_) => return Ok(skip(SeedSkipReason::FileChanged)),
        };
        if opened_file.metadata.st_size.max(0) as u64 != file.size {
            return Ok(skip(SeedSkipReason::FileChanged));
        }
        match &planned.join {
            Some(unit_join) => {
                let bytes = match read_bounded(
                    opened_file.fd.as_fd(),
                    join::MAX_JOIN_BYTES,
                    deadline,
                    monitor,
                ) {
                    Ok(bytes) => bytes,
                    Err(StreamError::Deadline) => return Ok(skip(SeedSkipReason::WorkDeadline)),
                    Err(StreamError::Monitor) => return Err(monitor_stop()),
                    Err(_) => return Ok(skip(SeedSkipReason::FileChanged)),
                };
                if catalog_digest(&bytes) != file.digest || file.digest != unit_join.join.digest {
                    return Ok(skip(SeedSkipReason::FileChanged));
                }
                let parsed = match join::parse(&bytes) {
                    Ok(parsed) => parsed,
                    Err(_) => return Ok(skip(SeedSkipReason::JoinInconsistent)),
                };
                // The flat rule sets must be exactly the recorded ones, the
                // `.d` must be a rule target, and every offered artifact must
                // have its own single-target rule with nonempty source
                // prerequisites. A flattened rule, a missing rule, an empty
                // rule or an artifact the `.d` never names is not a proven
                // unit.
                let proven = parsed.sets_match(&unit_join.join.targets, &unit_join.join.inputs)
                    && join::output_rule_proven(
                        &parsed,
                        &donor_slot_path.join("build"),
                        &unit_join.join.path,
                        &unit_join.join.inputs,
                    )
                    && unit_join.artifacts.iter().all(|artifact| {
                        join::output_rule_proven(
                            &parsed,
                            &donor_slot_path.join("build"),
                            artifact,
                            &unit_join.join.inputs,
                        )
                    });
                if !proven {
                    return Ok(skip(SeedSkipReason::JoinInconsistent));
                }
            }
            None => match hash_fd(opened_file.fd.as_fd(), deadline, monitor) {
                Ok(digest) if digest == file.digest => {}
                Ok(_) => return Ok(skip(SeedSkipReason::FileChanged)),
                Err(StreamError::Deadline) => return Ok(skip(SeedSkipReason::WorkDeadline)),
                Err(StreamError::Monitor) => return Err(monitor_stop()),
                Err(_) => return Ok(skip(SeedSkipReason::FileChanged)),
            },
        }
        sources.push(opened_file.fd);
    }
    if now() > deadline {
        return Ok(skip(SeedSkipReason::WorkDeadline));
    }
    // §6.5's point of no return is the first **recipient write**: creating the
    // `build/` directory, creating a destination directory, or `clonefile`
    // itself. Before any write a fault is a skip and the same slot still builds
    // cold; from the first write onward every fault — an unsafe path, `EEXIST`
    // at a destination, a clone or digest error, the work deadline — refuses
    // and retains. The one exception is the post-clone recipient walk, which
    // abandons seeding only (the caller releases the untrusted recipient and
    // cold-builds in a newly admitted slot).
    let mut wrote_any = false;
    let recipient_build = match recipient_build {
        Some(build) => build,
        None => {
            if let Err(error) = mkdir_owned(recipient_fd.as_fd(), c"build") {
                return pre_write(wrote_any, error);
            }
            // The directory exists now: record the write before any reopen,
            // so a reopen or stat failure refuses instead of cold-skipping.
            wrote_any = true;
            match open_optional_directory(recipient_fd.as_fd(), c"build", payload_device) {
                Ok(Some(build)) => build,
                Ok(None) => return pre_write(wrote_any, missing_directory()),
                Err(error) => return pre_write(wrote_any, error),
            }
        }
    };
    for (index, planned) in plan.files.iter().enumerate() {
        if monitor() {
            return Err(monitor_stop());
        }
        let file = &planned.file;
        if now() > deadline {
            return if wrote_any {
                Err(work_deadline_after_clone())
            } else {
                Ok(skip(SeedSkipReason::WorkDeadline))
            };
        }
        let (parents, name) = split_relative(&file.path);
        let mut deepest: Option<OwnedFd> = None;
        let mut destination = recipient_slot.join("build");
        for component in parents {
            let component = cstring(component)?;
            let parent_fd = match deepest.as_ref() {
                Some(fd) => fd.as_fd(),
                None => recipient_build.as_fd(),
            };
            match open_destination_directory(parent_fd, &component, payload_device) {
                Ok(Some(next)) => deepest = Some(next),
                Ok(None) => {
                    destination.push(component.to_string_lossy().as_ref());
                    test_point(TestPoint::BeforeDestinationMkdir { path: &destination });
                    if let Err(error) = mkdir_owned(parent_fd, &component) {
                        return pre_write(wrote_any, error);
                    }
                    // Mark the write the moment the directory exists, before
                    // the reopen: a fault between mkdir and reopen refuses and
                    // retains rather than cold-skipping in the same slot.
                    wrote_any = true;
                    test_point(TestPoint::AfterDestinationMkdir { path: &destination });
                    match open_optional_directory(parent_fd, &component, payload_device) {
                        Ok(Some(next)) => deepest = Some(next),
                        Ok(None) => return pre_write(wrote_any, missing_directory()),
                        Err(error) => return pre_write(wrote_any, error),
                    }
                }
                Err(error) => return pre_write(wrote_any, error),
            }
        }
        let directory = match deepest.as_ref() {
            Some(fd) => fd.as_fd(),
            None => recipient_build.as_fd(),
        };
        let name = cstring(name)?;
        let recipient_path = recipient_slot.join("build").join(&file.path);
        test_point(TestPoint::BeforeClone {
            index,
            recipient_slot: &recipient_slot,
            relative: &file.path,
        });
        if let Err(error) = clone_file(index, sources[index].as_fd(), directory, &name) {
            return pre_write(wrote_any, error);
        }
        // The clone exists from here: the recipient is untrusted and every
        // fault below refuses and retains.
        wrote_any = true;
        test_point(TestPoint::AfterClone {
            index,
            path: &recipient_path,
        });
        proof_crash_after_first_clone();
        if proof_breach("after-clone") {
            return Err(monitor_stop());
        }
        // The clone is hashed against the same recorded digest; a clone that
        // does not reproduce it is never published.
        let cloned = match walk::open_regular(&directory, &name) {
            Ok(cloned) => cloned,
            Err(error) => return pre_write(wrote_any, io_error(error)),
        };
        let metadata = match fstat(cloned.as_fd()) {
            Ok(metadata) => metadata,
            Err(error) => return pre_write(wrote_any, io_error(error)),
        };
        let digest = match hash_fd(cloned.as_fd(), deadline, monitor) {
            Ok(digest) => digest,
            Err(StreamError::Deadline) => return pre_write(wrote_any, work_deadline_after_clone()),
            Err(StreamError::Monitor) => return Err(monitor_stop()),
            Err(StreamError::Io(error)) => return pre_write(wrote_any, error),
            Err(StreamError::TooLarge) => {
                return pre_write(
                    wrote_any,
                    LeaseError::Io("a seeded clone exceeds its recorded size".to_owned()),
                )
            }
        };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
            || metadata.st_uid != current_uid()
            || metadata.st_dev as u64 != payload_device
            || metadata.st_size.max(0) as u64 != file.size
            || digest != file.digest
        {
            return pre_write(
                wrote_any,
                LeaseError::UnsafePath("a seeded clone does not match its recorded digest"),
            );
        }
        test_point(TestPoint::DuringClone {
            index,
            path: &recipient_path,
        });
        if now() > deadline {
            return Err(work_deadline_after_clone());
        }
    }
    // Final donor recheck: the ledger still names the planned generation for
    // both the donor and the recipient.
    test_point(TestPoint::BeforeFinalDonorCheck { pool });
    if !opened.donor_unchanged(donor)? {
        return Err(LeaseError::UnknownAccounting);
    }
    if !opened.recipient_held(recipient)? {
        return Err(LeaseError::UnknownAccounting);
    }
    // The post-clone recipient walk, before any caller proceeds (§6.3 step 7).
    test_point(TestPoint::BeforeWalk {
        slot: &recipient_slot,
    });
    let limits = walk_limits();
    // The walk's own cooperative deadline starts here; the seed's 5 s work
    // budget above is a separate bound and is not charged to the walk.
    let walked =
        walk::walk_slot_in_monitored(recipient_fd.as_fd(), payload_device, limits, now(), monitor);
    if walked.gap == Some(SeedWalkGap::MonitorStop) || monitor() {
        return Err(monitor_stop());
    }
    // Debug-only walk-gap control (absent from release builds; armed only under
    // the shared proof supervisor's own layout): the native proof injects each
    // failure class it cannot create on disk to prove publication is withheld.
    let walked = proof_inject_walk_gap(walked);
    let observation = SeedObservation {
        store_id: view.store_id.clone(),
        slot_id: recipient.id.to_string(),
        incarnation: recipient.incarnation.to_string(),
        lease_id: request.recipient.lease_id.clone(),
        entries: walked.entries,
        logical_bytes: walked.logical_bytes,
        allocated_bytes: walked.allocated_bytes,
        gap: walked.gap,
        limits,
        elapsed_ms: walked.elapsed_ms,
    };
    if walked.gap.is_some() {
        return Ok(SeedOutcome::Abandoned(SeedAbandon { observation }));
    }
    Ok(SeedOutcome::Seeded(SeedReceipt {
        donor_slot: request.donor.slot_id.clone(),
        donor_incarnation: request.donor.incarnation.clone(),
        units: plan.units as u64,
        files: plan.files.len() as u64,
        logical_bytes: plan.logical_bytes,
        catalog_digest: actual_catalog_digest,
        observation,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }))
}

/// Publish one generation-bound donor catalog sidecar.
///
/// The producing lease must still hold the slot: a catalog is written before
/// release, never after. The sidecar lives in the pool's internal
/// `oxide-managed/seeds/`, so this writes no removable media even for a
/// removable pool (whose *payload* [`seed`] does not write in this slice).
/// Nothing in the payload, ledger or control state is touched, and an over-cap
/// catalog is not written.
pub fn publish_seed_catalog(pool: &Path, catalog: &SeedCatalog) -> Result<(), LeaseError> {
    let bytes = catalog.to_bytes().map_err(LeaseError::InvalidInput)?;
    let opened = open_seed(pool)?;
    let view = &opened.view;
    let slot = view
        .slot(&catalog.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if catalog.store_id != view.store_id
        || !slot.held
        || slot.incarnation.to_string() != catalog.incarnation
        || slot.lease_id.as_deref() != Some(catalog.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    if !catalog_unseeded(&view.pool_root, catalog) {
        return Err(LeaseError::InvalidInput(
            "a generation without durable unseeded evidence publishes no catalog",
        ));
    }
    let managed = view.pool_root.join(MANAGED_NAME);
    ensure_private_dir(&managed)?;
    let dir = managed.join(SEEDS_DIR);
    ensure_private_dir(&dir)?;
    remove_orphan_sidecars(&dir, view)?;
    enforce_family_bytes(&dir, bytes.len() as u64, &catalog.slot_id)?;
    write_sidecar(
        &dir.join(format!("{}{CATALOG_SUFFIX}", catalog.slot_id)),
        &bytes,
    )
}

fn skip(reason: SeedSkipReason) -> SeedOutcome {
    SeedOutcome::Skipped(SeedSkip { reason })
}

/// A debug-only proof seam's value, when the shared supervisor armed this
/// process and the variable is set. Absent from release builds, and never read
/// in an ordinary run.
#[cfg(debug_assertions)]
fn proof_seam(name: &str) -> Option<String> {
    crate::proof::evidence()?;
    std::env::var(name).ok()
}

#[cfg(not(debug_assertions))]
fn proof_seam(_name: &str) -> Option<String> {
    None
}

/// The debug-only plan controls. `drop-dep` drops only the fingerprint
/// `dep-…` files; `admit-run` appends the run payload and run fingerprint of
/// each selected build-script package, with sizes and digests computed here.
/// Both are absent from release builds and armed only under the supervisor.
fn proof_adjust_plan(
    plan: &mut Plan,
    catalog: &SeedCatalog,
    units: &[SeedUnitRef],
    donor_slot: &Path,
    deadline: Instant,
    monitor: &mut dyn FnMut() -> bool,
) -> Result<(), LeaseError> {
    let Some(mode) = proof_seam("OXIDE_TEST_SEED_PLAN") else {
        return Ok(());
    };
    if mode == "drop-dep" {
        plan.files.retain(|planned| {
            !planned
                .file
                .path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.starts_with("dep-"))
        });
        plan.logical_bytes = plan.files.iter().fold(0u64, |total, planned| {
            total.saturating_add(planned.file.size)
        });
        return Ok(());
    }
    if mode != "admit-run" {
        return Ok(());
    }
    let selected: Vec<&str> = units
        .iter()
        .filter(|unit| unit.role == SeedRole::BuildScriptCompile)
        .filter_map(|unit| {
            catalog
                .packages
                .iter()
                .find(|package| {
                    package
                        .units
                        .iter()
                        .any(|candidate| candidate.fingerprint_dir == unit.fingerprint_dir)
                })
                .map(|package| package.name.as_str())
        })
        .collect();
    if selected.is_empty() {
        return Ok(());
    }
    let profile = match catalog.producer.recipe.as_str() {
        "debug" => "debug",
        "release" => "release",
        _ => "debug",
    };
    let root = donor_slot.join("build");
    let mut extras: Vec<(String, PathBuf)> = Vec::new();
    let entries = match fs::read_dir(root.join(profile).join(".fingerprint")) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some((literal, hash)) = name.rsplit_once('-') else {
            continue;
        };
        if !selected.contains(&literal) || hash.len() != 16 {
            continue;
        }
        for file in [
            "run-build-script-build-script-build",
            "run-build-script-build-script-build.json",
        ] {
            let path = format!("{profile}/.fingerprint/{name}/{file}");
            extras.push((path.clone(), root.join(&path)));
        }
        for file in ["output", "root-output", "stderr", "invoked.timestamp"] {
            let path = format!("{profile}/build/{name}/{file}");
            extras.push((path.clone(), root.join(&path)));
        }
        let out = format!("{profile}/build/{name}/out");
        let mut pending = vec![(out.clone(), root.join(&out))];
        while let Some((relative, directory)) = pending.pop() {
            if extras.len() > SEED_MAX_PLAN_FILES || now() > deadline || monitor() {
                return Ok(());
            }
            let Ok(items) = fs::read_dir(&directory) else {
                continue;
            };
            for item in items.flatten() {
                let Some(child) = item.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let child_relative = format!("{relative}/{child}");
                let child_path = root.join(&child_relative);
                let Ok(metadata) = fs::symlink_metadata(&child_path) else {
                    continue;
                };
                if metadata.is_dir() {
                    pending.push((child_relative, child_path));
                } else if metadata.is_file() && metadata.uid() == current_uid() {
                    extras.push((child_relative, child_path));
                }
            }
        }
    }
    for (relative, path) in extras {
        if plan
            .files
            .iter()
            .any(|planned| planned.file.path == relative)
        {
            continue;
        }
        let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        else {
            continue;
        };
        let metadata = match fstat(file.as_fd()) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG || metadata.st_uid != current_uid() {
            continue;
        }
        let size = metadata.st_size.max(0) as u64;
        let digest = match hash_fd(file.as_fd(), deadline, monitor) {
            Ok(digest) => digest,
            Err(_) => continue,
        };
        plan.files.push(PlannedFile {
            join: None,
            file: SeedFile {
                path: relative,
                size,
                mtime_ms: 0,
                digest,
                kind: SeedFileKind::Fingerprint,
            },
        });
        plan.logical_bytes = plan.logical_bytes.saturating_add(size);
    }
    Ok(())
}

/// The debug-only injected-stop seam: report the caller's monitor stop at a
/// named point, so the native proof can exercise a mid-seed fault that retains
/// the untrusted generation and refuses. Armed only under the supervisor.
fn proof_breach(point: &str) -> bool {
    proof_seam("OXIDE_TEST_SEED_BREACH").as_deref() == Some(point)
}

/// The debug-only crash seam: after the first clone exists and before any
/// digest check, abort this command. The untrusted origin and the partial
/// recipient survive for the next command's journaled whole-slot eviction.
fn proof_crash_after_first_clone() {
    if proof_seam("OXIDE_TEST_SEED_CRASH").as_deref() == Some("after-first-clone") {
        std::process::abort();
    }
}

/// Replace a completed walk's clean result with an injected gap, so the native
/// proof can exercise each failure class it cannot create on disk. A real gap
/// is never overwritten (the injected value applies only to a clean walk).
#[cfg(debug_assertions)]
fn proof_inject_walk_gap(walked: walk::WalkOutcome) -> walk::WalkOutcome {
    if walked.gap.is_some() {
        return walked;
    }
    let Some(gap) = proof_seam("OXIDE_TEST_SEED_WALK") else {
        return walked;
    };
    let gap = match gap.as_str() {
        "foreign-owner" | "ownership" => SeedWalkGap::ForeignOwner,
        "unreadable" | "failed" => SeedWalkGap::Unreadable,
        "device-boundary" => SeedWalkGap::DeviceBoundary,
        "entry-limit" => SeedWalkGap::EntryLimit,
        "depth-limit" => SeedWalkGap::DepthLimit,
        "unexpected-type" => SeedWalkGap::UnexpectedType,
        _ => return walked,
    };
    walk::WalkOutcome {
        gap: Some(gap),
        ..walked
    }
}

#[cfg(not(debug_assertions))]
fn proof_inject_walk_gap(walked: walk::WalkOutcome) -> walk::WalkOutcome {
    walked
}

fn walk_limits() -> SeedWalkLimits {
    #[cfg(test)]
    if let Some(limits) = tests::walk_limits_override() {
        return limits;
    }
    SeedWalkLimits {
        max_entries: SEED_WALK_MAX_ENTRIES,
        max_depth: SEED_WALK_MAX_DEPTH,
        max_elapsed_ms: SEED_WALK_MAX_ELAPSED_MS,
    }
}

/// The recorded join of one selected unit: its `.d` and the artifacts the
/// producing epoch classified as its rustc outputs.
struct UnitJoin {
    join: SeedJoin,
    artifacts: Vec<String>,
}

/// One planned file, with the unit join it must satisfy when it is that
/// unit's `.d`.
struct PlannedFile {
    file: SeedFile,
    join: Option<UnitJoin>,
}

/// The plan the library derived from the catalog: the exact file list and the
/// bytes it must still find. Nothing here came from the caller except the
/// unit identities.
struct Plan {
    units: usize,
    files: Vec<PlannedFile>,
    logical_bytes: u64,
}

fn plan_units(catalog: &SeedCatalog, units: &[SeedUnitRef]) -> Result<Plan, SeedSkipReason> {
    if units.len() > SEED_MAX_PLAN_UNITS {
        return Err(SeedSkipReason::PlanCap);
    }
    let mut files: Vec<PlannedFile> = Vec::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut seen_units = std::collections::BTreeSet::new();
    let mut join_inputs = 0usize;
    for wanted in units {
        if !seen_units.insert((wanted.fingerprint_dir.as_str(), wanted.role)) {
            return Err(SeedSkipReason::PlanCap);
        }
        let unit = catalog
            .packages
            .iter()
            .flat_map(|package| package.units.iter())
            .find(|unit| unit.fingerprint_dir == wanted.fingerprint_dir && unit.role == wanted.role)
            .ok_or(SeedSkipReason::UnitMissing)?;
        join_inputs = join_inputs.saturating_add(unit.join.inputs.len());
        if join_inputs > SEED_MAX_PLAN_INPUTS {
            return Err(SeedSkipReason::PlanCap);
        }
        let artifacts: Vec<String> = unit
            .files
            .iter()
            .filter(|file| file.kind == SeedFileKind::Artifact)
            .map(|file| file.path.clone())
            .collect();
        for file in &unit.files {
            if !seen_paths.insert(file.path.as_str()) {
                return Err(SeedSkipReason::PlanCap);
            }
            files.push(PlannedFile {
                join: (file.path == unit.join.path).then(|| UnitJoin {
                    join: unit.join.clone(),
                    artifacts: artifacts.clone(),
                }),
                file: file.clone(),
            });
            if files.len() > SEED_MAX_PLAN_FILES {
                return Err(SeedSkipReason::PlanCap);
            }
        }
    }
    let logical_bytes = files
        .iter()
        .try_fold(0u64, |total, planned| total.checked_add(planned.file.size))
        .ok_or(SeedSkipReason::PlanCap)?;
    if logical_bytes > SEED_MAX_PLAN_BYTES {
        return Err(SeedSkipReason::PlanCap);
    }
    // The running check above is §8.1's plan-wide join input bound: 32,768
    // rule inputs across every selected unit's `.d`, on top of each unit's own
    // 4,096.
    Ok(Plan {
        units: units.len(),
        files,
        logical_bytes,
    })
}

fn validate_recipient(recipient: &SeedRecipient) -> Result<(), LeaseError> {
    validate_slot_id(&recipient.slot_id)?;
    validate_generation(&recipient.incarnation, "incarnation")?;
    validate_generation(&recipient.lease_id, "lease id")?;
    Ok(())
}

fn validate_request(request: &SeedRequest) -> Result<(), LeaseError> {
    validate_recipient(&request.recipient)?;
    validate_slot_id(&request.donor.slot_id)?;
    validate_generation(&request.donor.incarnation, "incarnation")?;
    validate_generation(&request.donor.lease_id, "lease id")?;
    if request.donor.catalog_digest.len() != 64
        || !request
            .donor
            .catalog_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LeaseError::InvalidInput(
            "seed catalog digest is not a lowercase SHA-256",
        ));
    }
    for unit in &request.units {
        let valid = !unit.fingerprint_dir.is_empty()
            && unit.fingerprint_dir.len() <= MAX_PATH_BYTES
            && !unit.fingerprint_dir.starts_with('/')
            && unit
                .fingerprint_dir
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..");
        if !valid {
            return Err(LeaseError::InvalidInput(
                "seed plan names an unusable unit directory",
            ));
        }
    }
    if request.donor.record_id == 0 {
        return Err(LeaseError::InvalidInput(
            "the donor record id must be a positive integer",
        ));
    }
    Ok(())
}

/// This slice's custody calls write only an internal directory-budget pool's
/// payload. A removable `apfs-volume-v1` payload is the Elevated path and is
/// not written here at all.
/// The pool's front-door affinity directory for one record. The front door
/// passes it as the lease's `project`, so the donor slot's recorded project is
/// the binding between a slot and the record a catalog names.
fn affinity_dir(pool_root: &Path, record_id: u64) -> PathBuf {
    pool_root
        .join(MANAGED_NAME)
        .join("affinity")
        .join(record_id.to_string())
}

/// The seed's cooperative clock reading. Production reads the host's monotonic
/// clock; a debug-only seam can freeze this thread's reading so a correctness
/// fixture is never decided by host scheduling (oxide#224). Release builds have
/// no seam and no behaviour change.
fn now() -> Instant {
    #[cfg(debug_assertions)]
    {
        if let Some(frozen) = test_clock::frozen() {
            return frozen;
        }
    }
    Instant::now()
}

/// Debug builds only, absent from release: a thread-scoped clock reading for
/// fixtures that prove catalog pinning and completion rather than a work
/// deadline (oxide#224). It is thread-local, so a fixture that freezes its own
/// thread cannot change another test's clock, and every regression that
/// exercises the five-second bound keeps the real clock.
#[cfg(debug_assertions)]
pub mod test_clock {
    use std::cell::RefCell;
    use std::time::Instant;

    thread_local! {
        static FROZEN: RefCell<Option<Instant>> = const { RefCell::new(None) };
    }

    pub(super) fn frozen() -> Option<Instant> {
        FROZEN.with(|cell| *cell.borrow())
    }

    /// Freeze this thread's seed clock at its current reading until the guard
    /// drops. The guard restores the previous reading, so a nested freeze is
    /// scoped rather than leaked.
    pub fn freeze() -> Freeze {
        let previous = FROZEN.with(|cell| cell.replace(Some(Instant::now())));
        Freeze { previous }
    }

    /// Restores the reading that was in force before [`freeze`].
    pub struct Freeze {
        previous: Option<Instant>,
    }

    impl Drop for Freeze {
        fn drop(&mut self) {
            let previous = self.previous.take();
            FROZEN.with(|cell| *cell.borrow_mut() = previous);
        }
    }
}

/// The cooperative work bound: 5 s at most, however a caller asks for more.
fn work_budget(ms: Option<u64>) -> Duration {
    Duration::from_millis(
        ms.unwrap_or(SEED_WORK_BUDGET_MS)
            .clamp(1, SEED_WORK_BUDGET_MS),
    )
}

fn work_deadline_after_clone() -> LeaseError {
    LeaseError::Io("cold-seed work deadline expired after the first clone".to_owned())
}

/// Before the first recipient write, a fault abandons the attempt and the same
/// slot builds cold; after any write, the fault refuses and the untrusted
/// generation is retained (§6.5).
fn pre_write(wrote_any: bool, error: LeaseError) -> Result<SeedOutcome, LeaseError> {
    if wrote_any {
        return Err(error);
    }
    // A safety refusal is never a cold fallback, with or without a write:
    // unknown ownership, a foreign or wrong-device path, a mismatched backing
    // or unknown custody all refuse. Only a benign first-write fault (a
    // directory or clone that could not be created) may skip.
    match error {
        LeaseError::UnsafePath(_) | LeaseError::BackingMismatch | LeaseError::UnknownAccounting => {
            Err(error)
        }
        _ => Ok(skip(SeedSkipReason::CloneFailed)),
    }
}

/// What the donor's front-door control record says about its custody.
enum DonorControl {
    Eligible,
    /// A holder, a lease or a recorded `oxide clean` of the donor's checkout.
    Busy,
    /// Missing, unreadable, of an unknown version, absent of this record, or
    /// carrying a pending recovery, retirement or lost marker.
    Unknown,
}

/// The bounded part of the front door's control state this predicate needs.
/// It is deliberately tolerant: this is a read-only custody check, not a
/// parser for the state format, and anything it cannot classify is unknown.
#[derive(Deserialize)]
struct ControlState {
    version: u32,
    #[serde(default)]
    eviction_accounting: Option<String>,
    #[serde(default)]
    slot_ceiling: Option<String>,
    #[serde(default)]
    holding: Option<String>,
    #[serde(default)]
    one_use_workspaces: Option<String>,
    #[serde(default)]
    records: Vec<ControlRecord>,
    #[serde(default)]
    cleans: Vec<ControlClean>,
}

#[derive(Default)]
struct Present(bool);

impl<'de> Deserialize<'de> for Present {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        serde::de::IgnoredAny::deserialize(deserializer).map(|_| Self(true))
    }
}

#[derive(Deserialize)]
struct ControlRecord {
    id: u64,
    #[serde(default)]
    class: Present,
    #[serde(default)]
    class_reason: Present,
    #[serde(default)]
    phase: String,
    #[serde(default)]
    checkout: Option<PathBuf>,
    #[serde(default)]
    holder: Option<serde_json::Value>,
    #[serde(default)]
    lease_id: Option<String>,
    #[serde(default)]
    pending_recovery: Option<serde_json::Value>,
    #[serde(default)]
    retired: Option<String>,
    #[serde(default)]
    lost: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ControlClean {
    #[serde(default)]
    checkout: Option<PathBuf>,
}

/// Read the donor's own control record: exactly idle, no holder, no lease, no
/// pending recovery or retirement, and no recorded clean of its checkout.
fn donor_control(pool_root: &Path, record_id: u64) -> DonorControl {
    let path = pool_root.join(MANAGED_NAME).join("state.json");
    let Some(bytes) = read_private_bounded(&path, CONTROL_STATE_MAX_BYTES) else {
        return DonorControl::Unknown;
    };
    let Ok(state) = serde_json::from_slice::<ControlState>(&bytes) else {
        return DonorControl::Unknown;
    };
    let supported = state.version == CONTROL_STATE_VERSION
        || (state.version == 7
            && state.eviction_accounting.as_deref() == Some("oxide.eviction-accounting/1")
            && state.one_use_workspaces.is_none()
            && state
                .records
                .iter()
                .all(|record| !record.class.0 && !record.class_reason.0))
        || (state.version == 8
            && state.eviction_accounting.as_deref() == Some("oxide.eviction-accounting/1")
            && state.slot_ceiling.as_deref() == Some("oxide.slots/256")
            && state.one_use_workspaces.is_none()
            && state
                .records
                .iter()
                .all(|record| !record.class.0 && !record.class_reason.0))
        // Version 9 adds the whole-tree holding fence (oxide#190) and changes
        // nothing this donor check reads: a record's phase, holder and lease are
        // still the donor's own durable facts.
        || (state.version == 9
            && state.eviction_accounting.as_deref() == Some("oxide.eviction-accounting/1")
            && state.slot_ceiling.as_deref() == Some("oxide.slots/256")
            && state.holding.as_deref() == Some("oxide.holding/1")
            && state.one_use_workspaces.is_none()
            && state
                .records
                .iter()
                .all(|record| !record.class.0 && !record.class_reason.0));
    if !supported {
        return DonorControl::Unknown;
    }
    let Some(record) = state.records.iter().find(|record| record.id == record_id) else {
        return DonorControl::Unknown;
    };
    if record.pending_recovery.is_some() || record.retired.is_some() || record.lost.is_some() {
        return DonorControl::Unknown;
    }
    if record.phase != "idle" || record.holder.is_some() || record.lease_id.is_some() {
        return DonorControl::Busy;
    }
    if let Some(checkout) = &record.checkout {
        if state
            .cleans
            .iter()
            .any(|clean| clean.checkout.as_deref() == Some(checkout.as_path()))
        {
            return DonorControl::Busy;
        }
    }
    DonorControl::Eligible
}

/// Read one owner-only sidecar under its byte ceiling, or `None` when it is
/// missing, symlinked, foreign-owned, not private or oversized.
fn read_private_bounded(path: &Path, max: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > max
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(max + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= max).then_some(bytes)
}

fn validate_slot_id(text: &str) -> Result<(), LeaseError> {
    match text.parse::<u64>() {
        Ok(id) if id != 0 && text.bytes().all(|byte| byte.is_ascii_digit()) => Ok(()),
        _ => Err(LeaseError::InvalidInput(
            "slot ID must be a positive integer",
        )),
    }
}

fn validate_generation(text: &str, what: &'static str) -> Result<(), LeaseError> {
    if text.is_empty() || text.len() > MAX_LEASE_ID_BYTES || !text.is_ascii() {
        return Err(LeaseError::InvalidInput(match what {
            "lease id" => "lease id is empty, over-long or not ASCII",
            _ => "incarnation is empty, over-long or not ASCII",
        }));
    }
    Ok(())
}

enum DonorLock {
    Taken(File),
    Missing,
    Held,
}

/// The donor's front-door record lock, taken non-blocking with
/// `create: false`. A missing or held lock is a skip, never a wait.
fn probe_donor_lock(path: &Path) -> Result<DonorLock, LeaseError> {
    if path.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput(
            "the donor record lock path is too long",
        ));
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DonorLock::Missing)
        }
        Err(error) => return Err(io_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
    {
        return Err(LeaseError::UnsafePath(
            "the donor record lock is not an owned regular file",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    // SAFETY: the descriptor is live for the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(DonorLock::Taken(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(DonorLock::Held);
    }
    Err(io_error(error))
}

fn catalog_path(pool_root: &Path, slot_id: u64) -> PathBuf {
    pool_root
        .join(MANAGED_NAME)
        .join(SEEDS_DIR)
        .join(format!("{slot_id}{CATALOG_SUFFIX}"))
}

/// Read one catalog sidecar. A missing, oversized, malformed, symlinked,
/// foreign-owned, unknown-version or generation-mismatched file is ignored:
/// the caller treats the donor as offering nothing.
fn read_catalog(pool_root: &Path, slot_id: u64) -> Option<(SeedCatalog, Vec<u8>, String)> {
    let path = catalog_path(pool_root, slot_id);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > SEED_CATALOG_MAX_BYTES as u64
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .ok()?
        .take(SEED_CATALOG_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > SEED_CATALOG_MAX_BYTES {
        return None;
    }
    let catalog = SeedCatalog::parse(&bytes).ok()?;
    let digest = catalog_digest(&bytes);
    Some((catalog, bytes, digest))
}

/// Remove at most eight sidecars whose slot is no longer in the ledger.
fn remove_orphan_sidecars(dir: &Path, view: &SeedPoolView) -> Result<(), LeaseError> {
    let mut removed = 0usize;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Err(io_error(error)),
    };
    for entry in entries {
        if removed >= SEED_ORPHAN_CLEANUP_MAX {
            break;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        let Some(id) = sidecar_slot_id(text) else {
            continue;
        };
        if view.slot(&id.to_string()).is_some() {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_file()
            && metadata.uid() == current_uid()
            && fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    Ok(())
}

const ATTEMPT_SUFFIX: &str = ".attempt.json";
const OBSERVED_SUFFIX: &str = ".observed.json";

fn sidecar_slot_id(name: &str) -> Option<u64> {
    for suffix in [CATALOG_SUFFIX, ATTEMPT_SUFFIX, OBSERVED_SUFFIX] {
        if let Some(id) = name.strip_suffix(suffix) {
            if id.bytes().all(|byte| byte.is_ascii_digit()) {
                return id.parse().ok().filter(|id| *id != 0);
            }
        }
    }
    None
}

/// Keep the whole sidecar family inside its byte ceiling: drop up to eight of
/// the least recently produced catalogs (never this slot's) and refuse the
/// write if it still would not fit.
fn enforce_family_bytes(dir: &Path, adding: u64, keep_slot: &str) -> Result<(), LeaseError> {
    let cap = family_cap();
    let total = sidecar_bytes(dir)?;
    if total.saturating_add(adding) <= cap {
        return Ok(());
    }
    let mut catalogs: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        let Some(id) = sidecar_slot_id(text) else {
            continue;
        };
        if id.to_string() == keep_slot || !text.ends_with(CATALOG_SUFFIX) {
            continue;
        }
        // A sidecar whose size cannot be read makes the family figure unknown,
        // so the write refuses instead of assuming it fits.
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if !metadata.file_type().is_file() || metadata.uid() != current_uid() {
            continue;
        }
        let produced = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        catalogs.push((produced, metadata.len(), entry.path()));
    }
    catalogs.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(&right.2)));
    // Plan the drops first: a write that cannot fit even after dropping the
    // oldest catalogs must not destroy any of them.
    let mut planned: Vec<(u64, PathBuf)> = Vec::new();
    let mut projected = total;
    for (_, len, path) in catalogs {
        if projected.saturating_add(adding) <= cap || planned.len() >= SEED_FAMILY_DROP_MAX {
            break;
        }
        projected = projected.saturating_sub(len);
        planned.push((len, path));
    }
    if projected.saturating_add(adding) > cap {
        return Err(LeaseError::LedgerExhausted);
    }
    // Only a successful removal frees bytes; a failed one refuses the write
    // rather than crediting space that is still occupied.
    let mut freed = 0u64;
    for (len, path) in planned {
        if fs::remove_file(&path).is_ok() {
            freed = freed.saturating_add(len);
        }
    }
    if total.saturating_sub(freed).saturating_add(adding) > cap {
        return Err(LeaseError::LedgerExhausted);
    }
    Ok(())
}

fn family_cap() -> u64 {
    #[cfg(test)]
    if let Some(cap) = tests::family_cap_override() {
        return cap;
    }
    SEED_SIDECAR_FAMILY_MAX_BYTES
}

fn sidecar_bytes(dir: &Path) -> Result<u64, LeaseError> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

/// Atomic owner-only sidecar write: temp, fsync, rename, fsync the directory.
fn write_sidecar(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    if bytes.len() > SEED_CATALOG_MAX_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("sidecar has no parent"))?;
    let temp = parent.join(format!("catalog.tmp-{}", random_hex(8)?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp).map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    drop(file);
    // Test-only: a fault here models an interruption with the whole temp written
    // and the rename not yet done (the temp stays behind, inert).
    #[cfg(test)]
    if let Some(error) = tests::catalog_fault(false) {
        return Err(error);
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    // Test-only: a fault here models an interruption after the atomic rename,
    // before the directory fsync (the catalog is whole at its real name).
    #[cfg(test)]
    if let Some(error) = tests::catalog_fault(true) {
        return Err(error);
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

/// A custody failure on a slot path: an ownership, device or symlink fact
/// that is not establishable is a refusal, never a cold fallback.
fn slot_custody_error() -> LeaseError {
    LeaseError::UnsafePath("a slot directory is not an owned directory on the pool's device")
}

fn missing_directory() -> LeaseError {
    LeaseError::Io("a created seed directory is missing".to_owned())
}

/// Open the donor's slot path, distinguishing a vanished path from an unsafe
/// one. A missing payload root, `slots`, project folder or leaf means the donor
/// is unavailable before any recipient write, so the caller skips cold. An
/// existing path that is foreign-owned, linked, the wrong type or on another
/// device still refuses through the same custody checks as the recipient.
fn open_donor_slot_dirs(
    payload_root: &Path,
    device: u64,
    slot: &SeedSlotView,
) -> Result<Option<(OwnedFd, PathBuf)>, LeaseError> {
    let payload = match open_path_directory(payload_root) {
        Ok(fd) => fd,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(slot_custody_error()),
    };
    let payload = require_owned_directory(payload, device)?;
    let Some(slots) = open_slot_component_optional(payload.as_fd(), c"slots", device)? else {
        return Ok(None);
    };
    let root = payload_root.join("slots");
    match slot.dir.as_deref() {
        None => {
            let name = cstring(&slot.id.to_string())?;
            let Some(fd) = open_slot_component_optional(slots.as_fd(), &name, device)? else {
                return Ok(None);
            };
            Ok(Some((fd, root.join(slot.id.to_string()))))
        }
        Some(dir) => {
            let Some((folder, leaf)) = dir.split_once('/') else {
                let Some(fd) = open_slot_component_optional(slots.as_fd(), &cstring(dir)?, device)?
                else {
                    return Ok(None);
                };
                return Ok(Some((fd, root.join(dir))));
            };
            let Some(folder_fd) =
                open_slot_component_optional(slots.as_fd(), &cstring(folder)?, device)?
            else {
                return Ok(None);
            };
            let Some(leaf_fd) =
                open_slot_component_optional(folder_fd.as_fd(), &cstring(leaf)?, device)?
            else {
                return Ok(None);
            };
            Ok(Some((leaf_fd, root.join(dir))))
        }
    }
}

fn open_slot_component_optional(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<Option<OwnedFd>, LeaseError> {
    match open_directory_at(parent, name) {
        Ok(fd) => Ok(Some(require_owned_directory(fd, device)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(slot_custody_error()),
    }
}

/// Open one recorded slot beneath the payload's own descriptors, one
/// no-follow step at a time: payload root, `slots`, an optional project folder,
/// and the slot leaf. A project folder or `slots` entry replaced by a symlink
/// fails here instead of being followed, so a recorded slot path can never
/// resolve outside the payload. The returned path is for messages only.
fn open_slot_dirs(
    payload_root: &Path,
    device: u64,
    slot: &SeedSlotView,
) -> Result<(OwnedFd, PathBuf), LeaseError> {
    let payload = open_path_directory(payload_root).map_err(|_| slot_custody_error())?;
    let payload = require_owned_directory(payload, device)?;
    let slots = open_slot_component(payload.as_fd(), c"slots", device)?;
    let root = payload_root.join("slots");
    match slot.dir.as_deref() {
        None => {
            let name = cstring(&slot.id.to_string())?;
            let fd = open_slot_component(slots.as_fd(), &name, device)?;
            Ok((fd, root.join(slot.id.to_string())))
        }
        // A Barge root is one level (`<label>-<id>`); a legacy Oxide slot may
        // sit under its project folder.
        Some(dir) => match dir.split_once('/') {
            Some((folder, leaf)) => {
                let folder_fd = open_slot_component(slots.as_fd(), &cstring(folder)?, device)?;
                let leaf_fd = open_slot_component(folder_fd.as_fd(), &cstring(leaf)?, device)?;
                Ok((leaf_fd, root.join(dir)))
            }
            None => {
                let fd = open_slot_component(slots.as_fd(), &cstring(dir)?, device)?;
                Ok((fd, root.join(dir)))
            }
        },
    }
}

fn open_slot_component(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<OwnedFd, LeaseError> {
    let fd = open_directory_at(parent, name).map_err(|_| slot_custody_error())?;
    require_owned_directory(fd, device)
}

fn open_path_directory(path: &Path) -> std::io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("slot path holds a NUL"))?;
    // SAFETY: the path is a live NUL-terminated string.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_optional_directory(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<Option<OwnedFd>, LeaseError> {
    match open_directory_at(parent, name) {
        Ok(fd) => Ok(Some(require_owned_directory(fd, device)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn require_owned_directory(fd: OwnedFd, device: u64) -> Result<OwnedFd, LeaseError> {
    let metadata = fstat(fd.as_fd()).map_err(io_error)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_uid != current_uid()
        || metadata.st_dev as u64 != device
    {
        return Err(LeaseError::UnsafePath(
            "a slot directory is not an owned directory on the pool's device",
        ));
    }
    Ok(fd)
}

/// The recipient's `build/`: absent, or a real empty owned directory. A
/// non-empty or foreign one refuses; it is never cleaned or reused.
fn open_recipient_build(slot: BorrowedFd<'_>, device: u64) -> Result<Option<OwnedFd>, LeaseError> {
    let Some(build) = open_optional_directory(slot, c"build", device)? else {
        return Ok(None);
    };
    if !walk::directory_is_empty(build.as_fd()).map_err(io_error)? {
        return Err(LeaseError::UnsafePath(
            "the new slot's build directory is not empty",
        ));
    }
    Ok(Some(build))
}

/// Open a destination component, distinguishing a missing entry (create it)
/// from a link-shaped or foreign one (an unknown writer's entry: refuse).
fn open_destination_directory(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<Option<OwnedFd>, LeaseError> {
    match open_directory_at(parent, name) {
        Ok(fd) => Ok(Some(require_owned_directory(fd, device)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(destination_error(error)),
    }
}

fn mkdir_owned(parent: BorrowedFd<'_>, name: &std::ffi::CStr) -> Result<(), LeaseError> {
    // SAFETY: the descriptor and the name are live for the call.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0 {
        return Ok(());
    }
    Err(destination_error(std::io::Error::last_os_error()))
}

/// A destination fault: an entry that appeared where a new directory belongs
/// (`EEXIST`) or a link-shaped path is an unknown writer's entry and refuses;
/// any other filesystem error is a benign write failure.
fn destination_error(error: std::io::Error) -> LeaseError {
    match error.raw_os_error() {
        Some(libc::EEXIST) | Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
            LeaseError::UnsafePath("a destination entry appeared where the seed would create one")
        }
        _ => io_error(error),
    }
}

struct OpenedFile {
    fd: OwnedFd,
    metadata: libc::stat,
}

/// Open `relative` below `root` without following any link, requiring every
/// intermediate component to be a directory and the leaf a regular file on
/// `device`.
fn open_relative_file(
    root: BorrowedFd<'_>,
    relative: &str,
    device: u64,
) -> Result<OpenedFile, LeaseError> {
    let (parents, name) = split_relative(relative);
    let mut deepest: Option<OwnedFd> = None;
    for component in parents {
        let parent_fd = match deepest.as_ref() {
            Some(fd) => fd.as_fd(),
            None => root,
        };
        let next = open_directory_at(parent_fd, &cstring(component)?).map_err(donor_path_error)?;
        deepest = Some(require_owned_directory(next, device)?);
    }
    let directory = match deepest.as_ref() {
        Some(fd) => fd.as_fd(),
        None => root,
    };
    let name = cstring(name)?;
    let fd = walk::open_regular(&directory, &name).map_err(donor_path_error)?;
    let metadata = fstat(fd.as_fd()).map_err(io_error)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_uid != current_uid()
        || metadata.st_dev as u64 != device
    {
        return Err(LeaseError::UnsafePath(
            "a donor file is not an owned regular file on the pool's device",
        ));
    }
    Ok(OpenedFile { fd, metadata })
}

/// A donor path that cannot be established as an owned, no-follow entry is a
/// refusal (unknown ownership is not a cold fallback); a missing or unreadable
/// entry stays a payload change the caller may skip before the first write.
fn donor_path_error(error: std::io::Error) -> LeaseError {
    match error.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::ENOTDIR) => LeaseError::UnsafePath(
            "a donor path is not an owned no-follow entry on the pool's device",
        ),
        _ => io_error(error),
    }
}

fn split_relative(relative: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = relative.split('/').collect();
    let name = parts.pop().unwrap_or(relative);
    (parts, name)
}

fn cstring(text: &str) -> Result<CString, LeaseError> {
    CString::new(text.as_bytes())
        .map_err(|_| LeaseError::InvalidInput("a catalog path holds a NUL"))
}

fn fstat(fd: BorrowedFd<'_>) -> std::io::Result<libc::stat> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is live and `stat` is valid for writes.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stat)
}

/// A streamed read's failure. `Deadline` is the cooperative work bound, `Io`
/// a real read error and `TooLarge` a bounded read past its ceiling.
fn monitor_stop() -> LeaseError {
    LeaseError::Io("cold-seed reserve or admission monitor stopped".to_owned())
}

enum StreamError {
    Monitor,
    Io(LeaseError),
    Deadline,
    TooLarge,
}

/// Hash one file, checking the cooperative deadline between read calls.
fn hash_fd(
    fd: BorrowedFd<'_>,
    deadline: Instant,
    monitor: &mut dyn FnMut() -> bool,
) -> Result<String, StreamError> {
    let mut file = File::from(
        fd.try_clone_to_owned()
            .map_err(io_error)
            .map_err(StreamError::Io)?,
    );
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if monitor() {
            return Err(StreamError::Monitor);
        }
        if now() > deadline {
            return Err(StreamError::Deadline);
        }
        let read = file
            .read(&mut buffer)
            .map_err(io_error)
            .map_err(StreamError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(catalog::hex(&hasher.finalize()))
}

/// Read a bounded file, checking the cooperative deadline between read calls.
fn read_bounded(
    fd: BorrowedFd<'_>,
    max: usize,
    deadline: Instant,
    monitor: &mut dyn FnMut() -> bool,
) -> Result<Vec<u8>, StreamError> {
    let mut file = File::from(
        fd.try_clone_to_owned()
            .map_err(io_error)
            .map_err(StreamError::Io)?,
    );
    let mut bytes = Vec::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if monitor() {
            return Err(StreamError::Monitor);
        }
        if now() > deadline {
            return Err(StreamError::Deadline);
        }
        let read = file
            .read(&mut buffer)
            .map_err(io_error)
            .map_err(StreamError::Io)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > max {
            return Err(StreamError::TooLarge);
        }
    }
    Ok(bytes)
}

/// `clonefile` only: there is no byte-copy, hard-link or shell fallback. A
/// platform without it never seeds (the caller skips before any effect).
///
/// `index` is the plan position the call writes, so a regression can interrupt
/// the clone operation itself — before the syscall (no entry) or after it (the
/// entry exists, unverified) — and then inspect the durable state.
fn clone_file(
    index: usize,
    source: BorrowedFd<'_>,
    destination: BorrowedFd<'_>,
    name: &std::ffi::CStr,
) -> Result<(), LeaseError> {
    #[cfg(test)]
    if let Some(error) = tests::clone_fault(index, false) {
        return Err(error);
    }
    #[cfg(not(test))]
    let _ = index;
    #[cfg(target_os = "macos")]
    {
        // SAFETY: both descriptors and the name are live for the call, and
        // `fclonefileat` creates a new entry rather than following one.
        let result = unsafe {
            libc::fclonefileat(
                source.as_raw_fd(),
                destination.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        };
        if result == 0 {
            #[cfg(test)]
            if let Some(error) = tests::clone_fault(index, true) {
                return Err(error);
            }
            Ok(())
        } else {
            Err(destination_error(std::io::Error::last_os_error()))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, destination, name);
        Err(LeaseError::Unsupported(
            "clonefile is unavailable on this platform",
        ))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Test-only fault-injection points. A production build calls this with the
/// same shape and does nothing: no environment variable, flag or hook reaches
/// a real pool.
fn test_point(_point: TestPoint<'_>) {
    #[cfg(test)]
    tests::inject(_point);
}

#[allow(dead_code)]
enum TestPoint<'a> {
    /// Before any donor file is verified, after the plan exists.
    BeforeVerify { pool: &'a Path },
    /// Before cloning one planned file, by plan index.
    BeforeClone {
        index: usize,
        recipient_slot: &'a Path,
        relative: &'a str,
    },
    /// After one clone was created, before it is verified.
    AfterClone { index: usize, path: &'a Path },
    /// After one clone was verified, before the next step.
    DuringClone { index: usize, path: &'a Path },
    /// Immediately after a destination directory was created, before its
    /// reopen: the first-write boundary.
    AfterDestinationMkdir { path: &'a Path },
    /// Immediately before a destination directory is created (the `EEXIST`
    /// race window).
    BeforeDestinationMkdir { path: &'a Path },
    /// After every clone, before the recipient walk.
    BeforeWalk { slot: &'a Path },
    /// After the walk, before the final ledger recheck.
    BeforeFinalDonorCheck { pool: &'a Path },
}
