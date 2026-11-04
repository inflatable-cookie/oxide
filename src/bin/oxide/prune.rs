//! The owned-output registry, its forward initialization and the explicit
//! prune/resolve-intent commands (oxide#129).
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Pruning owned
//! output roots). The registry is the authority for which output roots a
//! checkout still selects. It is created once, behind the shared
//! `oxide.outputs/2` forward fence, under an `initializing`/`ready`
//! transaction recorded in both pools' durable state; a registry that is
//! missing, unreadable, incomplete or unexplained is never rebuilt and never
//! grants deletion authority.
//!
//! A prune is explicit: `--dry-run` renders a bounded preview and its SHA-256
//! without mutating anything, and a real run accepts exactly that preview
//! bytes and revalidates every predicate. Each eligible root moves by an
//! exclusive, descriptor-relative rename inside the `oxide-outputs` tree into
//! a quarantine leaf named by a fresh intent ID, is deleted only through that
//! verified descriptor, and its marker is unlinked last. Every phase is a
//! durable journal entry before the next effect.

use crate::outputs::{self, Marker, Receipt, RootKind, Selected};
use crate::process;
use crate::removable;
use crate::store::{self, CheckoutCustody, Store};
use oxide_storage::inventory::{self, InventoryLimits};
use oxide_storage::owned_delete::{
    capture_entry, clear_errno, descriptor_identity, enumeration_error, observe_entry,
    only_finder_metadata, open_directory_at, put_back, rmdir_draining_finder_metadata,
    unlink_captured, verify_captured, Capture, CaptureContext, DrainStop, EntryFacts, Rmdir,
    FINDER_RACE_ATTEMPTS, FINDER_RACE_PAUSE,
};
use oxide_storage::volume::Binding;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const REGISTRY_VERSION: &str = "oxide.outputs.registry/2";
/// Schema 1 named only a whole `target` placement; schema 2 names one
/// reference per profile member and is read from a version-1 document without
/// changing its meaning.
const LEGACY_REGISTRY_VERSION: &str = "oxide.outputs.registry/1";
pub const OBSERVATIONS_VERSION: &str = "oxide.outputs.prune-observations/1";
pub const PREVIEW_VERSION: &str = "oxide.outputs.prune-preview/1";
pub const INTENT_VERSION: &str = "oxide.outputs.prune-intent/1";
pub const RESOLUTION_VERSION: &str = "oxide.outputs.prune-resolution/1";

const REGISTRY_FILE: &str = "registry.json";
/// The registry's frozen numeric format.
const REGISTRY_SCHEMA: u32 = 2;
/// A version-1 registry predates profile member references.
const LEGACY_REGISTRY_SCHEMA: u32 = 1;
const OBSERVATIONS_FILE: &str = "prune-observations.json";
/// The diagnostic tombstone ring (oxide#129): settled prune outcomes only,
/// oldest first, compactable. Never a safety journal or a live cap.
const TOMBSTONES_FILE: &str = "tombstones.json";
const TOMBSTONES_VERSION: &str = "oxide.outputs.tombstones/1";
const MAX_TOMBSTONES: usize = 16_384;
const MAX_TOMBSTONE_BYTES: usize = 4 * 1024 * 1024;
const QUARANTINE_PREFIX: &str = ".prune-";
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROWS: usize = 4096;
const MAX_INTENTS: usize = 256;
const MAX_OBSERVATIONS_BYTES: usize = 256 * 1024;
const MAX_OBSERVATION_ROOTS: usize = 512;
const MAX_PLAN_BYTES: usize = 256 * 1024;
const MAX_CANDIDATES: usize = 512;
/// Kept diagnostic for an eligible root this bounded preview did not select.
const DEFERRED_KEEP_REASON: &str = "eligible remainder deferred to a later prune batch";
const MAX_OPERATOR_LIST_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const DELETE_BATCH: u64 = 512;
const OBSERVATION_ENTRIES: u64 = 50_000;
const OBSERVATION_MS: u64 = 5_000;
/// Debug-only fault injection for the native proof: name a prune phase to stop
/// before its effect.
#[cfg(debug_assertions)]
const CRASH_ENVIRONMENT: &str = "OXIDE_TEST_PRUNE_CRASH";

const USAGE: &str = "\
usage: oxide outputs prune --dry-run [--operator-list FILE]
       oxide outputs prune --plan FILE --plan-sha256 HEX [--operator-list FILE]
       oxide outputs resolve-intent <intent-id>

`--dry-run` prints a bounded preview of output roots whose checkout is
positively absent, with the SHA-256 of exactly those bytes on stderr. It
mutates nothing. A real run needs that exact preview file and SHA, rechecks
every predicate and the root identity set, and deletes only roots this
registry still selects with no active writer and a validated owner marker.";

/// The registry's one writer-facing document.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: String,
    /// The frozen numeric registry format, 1 (shared with `version`).
    #[serde(default)]
    schema: u32,
    /// Advanced before every changed receipt, selection or root effect.
    revision: u64,
    complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    incomplete: Option<Incomplete>,
    /// The initialization transaction that created this registry.
    created_by: String,
    #[serde(default)]
    rows: Vec<Row>,
    /// Unfinished prune journals. Safety entries: never compacted, never
    /// reconstructed, and they count toward the live caps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    intents: Vec<Intent>,
    /// Overflow reservations whose cancellation began but whose receipt has
    /// not yet committed without them. A safety journal: binding refuses any
    /// listed reservation; never compacted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cancelled_overflow: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Incomplete {
    reason: String,
    since_ms: u64,
    transaction: String,
}

/// One live reference to an output root. Rows key by checkout, placement
/// member and root; history rows keep `selected`/`pending` false and never
/// grant deletion authority.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    checkout_id: String,
    checkout: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout_volume_uuid: Option<String>,
    receipt: PathBuf,
    member: String,
    root: PathBuf,
    root_kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    recipe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    /// The root's own immutable creation generation, read from its owner
    /// marker when the row is written (oxide#218). Rows written before
    /// oxide#152 recorded the receipt's committed selection generation
    /// instead; `judge_recorded_root` reconciles exactly that superseded shape
    /// from the recorded marker digest, and never a changed marker.
    generation: u64,
    /// The root's device and inode when the selection was committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_dev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_ino: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marker_digest: Option<String>,
    selected: bool,
    pending: bool,
    /// Set while a prune intent references this root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pruning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pruned_ms: Option<u64>,
    /// The receipt reservation transaction that created this pending row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reservation: Option<String>,
    /// Set durably before an unused reservation's receipt drops it; binding
    /// refuses the reservation from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cancelling: Option<String>,
}

impl Row {
    fn live(&self) -> bool {
        self.selected
            || self.pending
            || self.pruning.is_some()
            || self.reservation.is_some()
            || self.cancelling.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum IntentPhase {
    /// The journal entry exists; no filesystem effect has happened.
    Intent,
    /// The root leaf was exclusively renamed to its quarantine leaf.
    Renamed,
    /// Deletion of the quarantined tree has begun.
    Deleting,
    /// Everything except the marker is gone.
    MarkerLast,
    /// The quarantine leaf is gone: positive completion proof.
    Deleted,
    /// Settled by an explicit operator resolution without deletion.
    Resolved,
}

/// Root identity recorded in the journal. Version-2 roots carry a nonce;
/// legacy roots (written before oxide#129) never need one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootIdentity {
    checkout_id: String,
    checkout: PathBuf,
    receipt: PathBuf,
    member: String,
    root: PathBuf,
    root_kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    generation: u64,
    dev: u64,
    ino: u64,
    marker_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: String,
    id: String,
    identity: RootIdentity,
    phase: IntentPhase,
    quarantine: PathBuf,
    preview_sha256: String,
    created_ms: u64,
    updated_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resolved_ms: Option<u64>,
    /// The checkout receipt's terminal state committed after the registry
    /// terminal state. A `deleted` or `resolved` intent without it is still
    /// unfinished: its receipt step is replayed, never assumed.
    #[serde(default)]
    receipt_settled: bool,
    /// Whether the representative registry row was a live selection when the
    /// intent was journaled. Only a group member's own flag is restored from
    /// [`Intent::group`]; this field is diagnostic for the representative.
    #[serde(default = "selected_by_default")]
    was_selected: bool,
    /// The checkout receipt's exact bytes at intent publication. A replay or
    /// rollback acts only while the live receipt still matches this preimage.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    prior_receipt_sha: String,
    /// Every verified reference of this (checkout, root) group, sorted by
    /// member. The group — never a single row — is the transaction scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    group: Vec<GroupRow>,
    /// Persisted before the terminal receipt publication: the exact terminal
    /// image digest and its FIXED timestamp. A replay accepts only the
    /// recorded preimage or this exact image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_terminal_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_ms: Option<u64>,
}

/// One reference of an intent's (checkout, root) group: its member, immutable
/// root-creation generation, nonce and recorded identity, and the selection
/// flag the rollback restores.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRow {
    member: String,
    generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_dev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_ino: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marker_digest: Option<String>,
    selected: bool,
}

fn selected_by_default() -> bool {
    true
}

/// The bounded preview document. Its canonical bytes are the plan a real run
/// must present unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    version: String,
    generated_ms: u64,
    registry_revision: u64,
    registry_complete: bool,
    coverage: Coverage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operator_list_sha256: Option<String>,
    candidates: Vec<Candidate>,
    kept: Vec<Kept>,
    totals: Totals,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    roots_scanned: u64,
    registry_roots: u64,
    candidate_roots: u64,
    kept_roots: u64,
    complete: bool,
    source: String,
    observed_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    checkout_id: String,
    checkout: PathBuf,
    receipt: PathBuf,
    member: String,
    root: PathBuf,
    root_kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout_volume_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    generation: u64,
    dev: u64,
    ino: u64,
    marker_digest: String,
    logical_bytes: u64,
    allocated_bytes: u64,
    accounting_complete: bool,
    /// An operator-listed legacy root with no registry row and no receipt:
    /// it is protected by an exclusive `pruning` receipt before its intent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    receiptless: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
    reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Totals {
    candidates: u64,
    kept: u64,
    logical_bytes: u64,
    allocated_bytes: u64,
    accounting_complete: bool,
}

/// The recorded facts default `oxide status` renders. Reads only; no root
/// walk, no registry replay and no reconciliation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observations {
    version: String,
    generated_ms: u64,
    registry_revision: u64,
    registry_complete: bool,
    source: String,
    coverage: Coverage,
    roots: Vec<ObservedRoot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedRoot {
    checkout_id: String,
    root: PathBuf,
    outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent: Option<String>,
    logical_bytes: u64,
    allocated_bytes: u64,
    accounting_complete: bool,
}

/// `oxide outputs prune ...` / `oxide outputs resolve-intent ...`.
///
/// The public contract has no generic failure exit: usage and any refusal
/// before an effect are exit 2, and an operation whose own transaction is
/// incomplete reports exit 3. Errors are therefore mapped here instead of
/// reaching the front door's generic exit 1.
pub fn run(args: &[std::ffi::OsString]) -> Result<u8, String> {
    let args: Vec<&str> = match args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()
    {
        Ok(args) => args,
        Err(reason) => {
            eprintln!("oxide: outputs: {reason}");
            return Ok(2);
        }
    };
    let result = match args.as_slice() {
        ["prune", "--help" | "-h"] | ["resolve-intent", "--help" | "-h"] => {
            println!("{USAGE}");
            return Ok(0);
        }
        ["prune", "--dry-run"] => dry_run(None),
        ["prune", "--dry-run", "--operator-list", list] => dry_run(Some(Path::new(list))),
        ["prune", "--plan", plan, "--plan-sha256", sha] => real_run(plan, sha, None),
        ["prune", "--plan", plan, "--plan-sha256", sha, "--operator-list", list] => {
            real_run(plan, sha, Some(Path::new(list)))
        }
        ["resolve-intent", id] => resolve_intent(id),
        _ => {
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };
    match result {
        Ok(code) => Ok(code),
        Err(reason) => {
            eprintln!(
                "oxide: outputs refused: {reason}; no output root was changed and any pending intent stays pending"
            );
            Ok(2)
        }
    }
}

/// A fresh random 128-bit root or transaction nonce, lowercase hex. Entropy
/// failure refuses; no fallback value is ever derived from a path or clock.
pub fn fresh_nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    // SAFETY: the pointer is valid for the buffer's length and getentropy has
    // no other precondition.
    let status = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    if status != 0 {
        return Err(format!(
            "cannot read entropy for an output nonce: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

fn registry_path(control: &Path) -> PathBuf {
    control.join("outputs").join(REGISTRY_FILE)
}

fn observations_path(control: &Path) -> PathBuf {
    control.join("outputs").join(OBSERVATIONS_FILE)
}

fn registry_dir(control: &Path) -> PathBuf {
    control.join("outputs")
}

/// The internal pool for an output operation, refusing when there is none.
fn internal_store() -> Result<Store, String> {
    let settings = store::settings()?;
    Store::open_existing_with(&settings)?
        .ok_or_else(|| "no internal pool exists; run a building oxide command first".to_owned())
}

/// The internal store paired with a registered removable pool's control.
fn paired_store() -> Result<Store, String> {
    let internal = internal_store()?;
    removable::coordinate(Some(internal))?.ok_or_else(|| "the internal pool disappeared".to_owned())
}

/// Ensure the shared forward format and the registry exist exactly once.
/// Called under the caller's checkout barrier for `checkout` and before its
/// admission lock; initialization refuses unless every writer in both pools
/// is positively closed (that barrier excepted).
pub fn ensure_ready(internal: &Store, checkout: &Path) -> Result<(), String> {
    let control = internal.owner_control().to_path_buf();
    let transaction = match internal.outputs_initialization()? {
        Some(store::OutputsInit::Ready { transaction, .. }) => transaction,
        _ => fresh_nonce()?,
    };
    internal.initialize_outputs(&transaction, Some(checkout), &|transaction, may_create| {
        create_or_validate_registry(&control, transaction, may_create)
    })?;
    let registry = ready_registry(&control)?;
    registry.validate()?;
    Ok(())
}

/// Seed the new registry once from the bounded committed receipts already in
/// the control directory. Any unreadable, malformed or oversized receipt makes
/// the sticky-incomplete flag instead of guessing.
fn seed_registry(control: &Path, registry: &mut Registry) -> Result<(), String> {
    #[derive(Deserialize)]
    struct Stub {
        checkout: PathBuf,
        checkout_id: String,
    }
    let directory = registry_dir(control);
    let deadline = Instant::now() + SNAPSHOT_TIME;
    let mut budget = SNAPSHOT_MAX_BYTES;
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    };
    for entry in entries {
        if Instant::now() > deadline || budget == 0 {
            return Err("the receipt seed exceeded its bounds".to_owned());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.starts_with('.')
            || name_text == REGISTRY_FILE
            || name_text == OBSERVATIONS_FILE
            || !name_text.ends_with(".json")
        {
            continue;
        }
        let metadata = entry.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.len() > budget {
            return Err("the receipt seed exceeded its byte budget".to_owned());
        }
        budget = budget.saturating_sub(metadata.len());
        let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
        let stub: Stub = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{} is invalid: {error}", entry.path().display()))?;
        let receipt =
            match outputs::read_receipt_for_prune(control, &stub.checkout, &stub.checkout_id) {
                Ok(Some(receipt)) => receipt,
                Ok(None) => continue,
                Err(error) => return Err(error),
            };
        let checkout_volume_uuid = volume_uuid_of(&stub.checkout).ok().flatten();
        for placement in receipt.links() {
            let member = placement.member();
            for (selected, is_selected) in placement
                .selected_ref()
                .map(|selected| (selected, true))
                .into_iter()
                .chain(
                    placement
                        .prepared_ref()
                        .map(|prepared| (&prepared.to, false)),
                )
            {
                if registry.rows.iter().any(|row| {
                    row.checkout_id == stub.checkout_id
                        && row.member == member
                        && row.root == selected.root()
                }) {
                    continue;
                }
                if registry.rows.len() >= MAX_ROWS {
                    return Err("the receipt seed reached the registry row limit".to_owned());
                }
                let identity = root_identity(selected)?;
                registry.rows.push(Row {
                    checkout_id: stub.checkout_id.clone(),
                    checkout: stub.checkout.clone(),
                    checkout_volume_uuid: checkout_volume_uuid.clone(),
                    receipt: outputs::receipt_path_public(control, &stub.checkout_id),
                    member: member.to_owned(),
                    root: selected.root().to_path_buf(),
                    root_kind: selected.kind(),
                    volume_uuid: selected.volume_uuid().map(str::to_owned),
                    recipe: outputs::RECIPE_PUBLIC.to_owned(),
                    nonce: selected.nonce().map(str::to_owned),
                    generation: identity.generation(selected),
                    root_dev: identity.dev,
                    root_ino: identity.ino,
                    marker_digest: identity.digest,
                    selected: is_selected,
                    pending: !is_selected,
                    pruning: None,
                    pruned_ms: None,
                    reservation: None,
                    cancelling: None,
                });
            }
        }
    }
    Ok(())
}

fn create_or_validate_registry(
    control: &Path,
    transaction: &str,
    may_create: bool,
) -> Result<(), String> {
    ensure_private_dir(&registry_dir(control))?;
    let path = registry_path(control);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "{} is not a real registry file; refusing to initialize over it",
                    path.display()
                ));
            }
            let registry = read_registry(control)?
                .ok_or_else(|| format!("{} disappeared while initializing", path.display()))?;
            if registry.created_by != transaction {
                return Err(format!(
                    "{} was created by transaction {}, not the recorded {transaction}; refusing to resume an unknown initialization",
                    path.display(),
                    registry.created_by
                ));
            }
            registry.validate()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !may_create {
                // A fenced, ready pool whose registry disappeared is never
                // rebuilt: mutation refuses and prune keeps every root.
                return Err(format!(
                    "{} is missing but the control state records a ready registry; refusing to rebuild it",
                    path.display()
                ));
            }
            let mut registry = Registry {
                version: REGISTRY_VERSION.to_owned(),
                schema: REGISTRY_SCHEMA,
                revision: 1,
                complete: true,
                incomplete: None,
                created_by: transaction.to_owned(),
                rows: Vec::new(),
                intents: Vec::new(),
                cancelled_overflow: Vec::new(),
            };
            // Seed live references from the bounded committed receipts this
            // control directory already holds. Never from markers, and never
            // replacing an existing registry.
            if let Err(reason) = seed_registry(control, &mut registry) {
                registry.complete = false;
                registry.incomplete = Some(Incomplete {
                    reason: format!("the receipt seed was incomplete ({reason})"),
                    since_ms: process::now_ms(),
                    transaction: transaction.to_owned(),
                });
            }
            registry.validate()?;
            let bytes = serde_json::to_vec(&registry).map_err(|error| error.to_string())?;
            if bytes.len() as u64 > MAX_REGISTRY_BYTES {
                return Err("the output registry exceeds its byte limit".to_owned());
            }
            // Exclusive, no-follow creation: an existing registry is never
            // replaced or rebuilt.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)
                .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            sync_directory(&registry_dir(control))
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

/// Read the registry, refusing a missing, oversized, malformed or
/// foreign-version document. `Ok(None)` only when the file is positively
/// absent.
fn read_registry(control: &Path) -> Result<Option<Registry>, String> {
    let path = registry_path(control);
    let Some(file) = store::private_file(&path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    #[cfg(not(test))]
    authority_read(file.take(MAX_REGISTRY_BYTES + 1), &mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    #[cfg(test)]
    authority_read_observed(
        &file,
        &path,
        "prune::read_registry",
        MAX_REGISTRY_BYTES,
        &mut bytes,
    )
    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(format!(
            "{} exceeds its {MAX_REGISTRY_BYTES}-byte limit",
            path.display()
        ));
    }
    let registry: Registry = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    registry.validate()?;
    Ok(Some(registry))
}

/// Require the forward format and registry to be ready without changing
/// anything: a dry run writes nothing, and a real run must not initialize
/// between preview and effect.
fn require_ready(store: &Store, control: &Path) -> Result<Registry, String> {
    match store.outputs_initialization()? {
        Some(store::OutputsInit::Ready { .. }) => {}
        _ => {
            #[cfg(test)]
            crate::tape::classify_row(
                "outputs-initialization",
                &control.display().to_string(),
                "not-ready",
            );
            return Err(
                "the output registry is not initialized; run `oxide outputs adopt` once to establish the shared forward format, then prune"
                    .to_owned(),
            );
        }
    }
    #[cfg(test)]
    crate::tape::classify_row(
        "outputs-initialization",
        &control.display().to_string(),
        "ready",
    );
    ready_registry(control)
}

/// Refuse an output transition or replay before its first effect unless the
/// registry is ready (oxide#129). A pool adopted before the shared output
/// format has none; a command holding a slot cannot initialize it (its own
/// record is an open writer), so the refusal names the explicit upgrade.
pub fn require_registry(control: &Path) -> Result<(), String> {
    match read_registry(control)? {
        Some(registry) => registry.validate(),
        None => Err(format!(
            "outputs: the output registry {} is missing, so no owned-output receipt is written and the link, receipt and roots are unchanged. A pool from before the shared output format is upgraded by `oxide outputs adopt` (or `oxide outputs detach`) in this checkout once no other Oxide command runs in either pool",
            registry_path(control).display()
        )),
    }
}

/// The ready registry's current revision.
pub fn registry_revision(control: &Path) -> Result<u64, String> {
    Ok(ready_registry(control)?.revision)
}

/// The registry that must already be ready. Missing is never "empty".
fn ready_registry(control: &Path) -> Result<Registry, String> {
    read_registry(control)?.ok_or_else(|| {
        format!(
            "the output registry {} is missing; output mutation refuses and prune keeps every root",
            registry_path(control).display()
        )
    })
}

impl Registry {
    fn validate(&self) -> Result<(), String> {
        let schema = match self.version.as_str() {
            REGISTRY_VERSION if self.schema == REGISTRY_SCHEMA => REGISTRY_SCHEMA,
            LEGACY_REGISTRY_VERSION if self.schema == LEGACY_REGISTRY_SCHEMA => {
                LEGACY_REGISTRY_SCHEMA
            }
            _ => {
                return Err(format!(
                    "the output registry has an unsupported version {:?}",
                    self.version
                ))
            }
        };
        let _ = schema;
        if self.revision == 0 {
            return Err("the output registry has no revision".to_owned());
        }
        if !text(&self.created_by) {
            return Err("the output registry has an unusable transaction identity".to_owned());
        }
        if self.rows.len() > MAX_ROWS || self.intents.len() > MAX_INTENTS {
            return Err("the output registry exceeds a collection limit".to_owned());
        }
        if let Some(incomplete) = &self.incomplete {
            if self.complete || !text(&incomplete.reason) {
                return Err("the output registry has inconsistent incompleteness".to_owned());
            }
        }
        if !self.complete && self.incomplete.is_none() {
            return Err("the output registry is incomplete without a reason".to_owned());
        }
        for row in &self.rows {
            row.validate()?;
        }
        for intent in &self.intents {
            intent.validate()?;
        }
        Ok(())
    }

    fn bump(&mut self) -> Result<(), String> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("the output registry revision is exhausted")?;
        Ok(())
    }

    fn live_rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|row| row.live())
    }

    fn intent(&self, id: &str) -> Option<&Intent> {
        self.intents.iter().find(|intent| intent.id == id)
    }

    fn unfinished_intents(&self) -> impl Iterator<Item = &Intent> {
        self.intents.iter().filter(|intent| !intent.settled())
    }
}

impl Row {
    fn validate(&self) -> Result<(), String> {
        if !hex64(&self.checkout_id)
            || !text(&self.member)
            || !text(&self.recipe)
            || !self.root.is_absolute()
            || !self.checkout.is_absolute()
            || !self.receipt.is_absolute()
        {
            return Err("the output registry has an unusable row".to_owned());
        }
        // A prepared transition may name its current root as both the
        // selected and the proposed reference; that is one row with both
        // flags, never a contradiction. A pruning row is never selected or
        // pending, because its blocking intent owns it.
        if self.pruning.is_some() && (self.selected || self.pending) {
            return Err("the output registry row is pruning and still referenced".to_owned());
        }
        Ok(())
    }
}

impl Intent {
    /// Settled only once both the registry terminal state and the checkout
    /// receipt's terminal state committed.
    fn settled(&self) -> bool {
        matches!(self.phase, IntentPhase::Deleted | IntentPhase::Resolved) && self.receipt_settled
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != INTENT_VERSION
            || !hex32(&self.id)
            || !hex64(&self.identity.checkout_id)
            || !self.identity.root.is_absolute()
            || !self.identity.receipt.is_absolute()
            || !self.quarantine.is_absolute()
            || !hex64(&self.preview_sha256)
        {
            return Err("the output registry has an unusable prune intent".to_owned());
        }
        if !self.prior_receipt_sha.is_empty() && !hex64(&self.prior_receipt_sha) {
            return Err("the output registry has an unusable intent preimage".to_owned());
        }
        if let Some(sha) = &self.expected_terminal_sha {
            if !hex64(sha) || self.terminal_ms.is_none() {
                return Err("the output registry has an unusable terminal image".to_owned());
            }
        }
        let mut names: Vec<&str> = self.group.iter().map(|row| row.member.as_str()).collect();
        let sorted = names.clone();
        names.sort_unstable();
        names.dedup();
        if names != sorted {
            return Err("the prune intent group is not sorted and unique".to_owned());
        }
        for row in &self.group {
            if !text(&row.member) {
                return Err("the prune intent group has an unusable member".to_owned());
            }
        }
        Ok(())
    }
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| {
                format!("cannot set permissions on {}: {error}", path.display())
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    use std::os::unix::fs::MetadataExt;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(format!(
            "{} must be an owner-only directory",
            path.display()
        ));
    }
    Ok(())
}

fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.contains('\0')
}

fn hex32(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(crate) fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The unsettled prune intent for one checkout, if any. Binding, adoption and
/// replay refuse while it exists.
pub fn unsettled_for(control: &Path, checkout_id: &str) -> Result<Option<String>, String> {
    let Some(registry) = read_registry(control)? else {
        return Ok(None);
    };
    let found = registry
        .unfinished_intents()
        .find(|intent| intent.identity.checkout_id == checkout_id)
        .map(|intent| intent.id.clone());
    Ok(found)
}

/// Publish one receipt's live references into the registry. Called from
/// `outputs::write_receipt` while the internal publication lock is held, and
/// strictly before the changed receipt becomes visible.
///
/// One row is kept per (checkout, member, root) reference. A legacy receipt's
/// one `target` placement keeps the schema-1 meaning exactly; a profile
/// receipt publishes one reference per positively identified member and
/// upgrades the document to schema 2, which only happens after the caller's
/// forward fence.
pub fn note_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    require_registry(control)?;
    let mut registry = ready_registry(control)?;
    registry.validate()?;
    if receipt.layout() == outputs::LayoutKind::Profile {
        registry.version = REGISTRY_VERSION.to_owned();
        registry.schema = REGISTRY_SCHEMA;
    }
    let checkout_id = receipt.checkout_id().to_owned();
    let receipt_path = outputs::receipt_path_public(control, &checkout_id);
    let checkout_volume_uuid = volume_uuid_of(receipt.checkout()).ok().flatten();
    registry.bump()?;
    for placement in receipt.links() {
        note_placement(
            &mut registry,
            receipt,
            placement,
            &receipt_path,
            checkout_volume_uuid.clone(),
        )?;
    }
    // Members this receipt no longer places keep their identity as history but
    // are neither selected nor pending; a blocking prune intent still owns its
    // row.
    let members: Vec<String> = receipt
        .links()
        .iter()
        .map(|placement| placement.member().to_owned())
        .collect();
    for row in registry
        .rows
        .iter_mut()
        .filter(|row| row.checkout_id == checkout_id && !members.contains(&row.member))
    {
        row.selected = false;
        row.pending = false;
    }
    note_reservation(&mut registry, receipt, &receipt_path, checkout_volume_uuid)?;
    write_registry(control, &registry)
}

/// Publish one placement's selected and proposed references.
fn note_placement(
    registry: &mut Registry,
    receipt: &Receipt,
    placement: &outputs::LinkPlacement,
    receipt_path: &Path,
    checkout_volume_uuid: Option<String>,
) -> Result<(), String> {
    let checkout_id = receipt.checkout_id();
    let checkout = receipt.checkout();
    let member = placement.member();
    let selected = placement.selected_ref();
    let proposed = placement.prepared_ref().map(|prepared| &prepared.to);
    // A prepared transition still selects its committed root until the commit
    // is durable; a missing reference is never treated as non-selection.
    // Detach and pruned states retain identity but select nothing.
    let keep_selected = selected.is_some() && receipt.holds_selection();
    let mut matched_selected = false;
    let mut matched_pending = false;
    for row in registry
        .rows
        .iter_mut()
        .filter(|row| row.checkout_id == checkout_id && row.member == member)
    {
        if let Some(selected) = selected {
            if row.root == selected.root() {
                row.selected = keep_selected;
                matched_selected = keep_selected;
                row.pruning = None;
                row.pending = false;
                if keep_selected {
                    // The committed root now exists: refresh the identity the
                    // prune proof matches (a pending row had none yet). The row
                    // records the root's OWN immutable creation generation, not
                    // the selection's commit generation (oxide#218), and a
                    // placement that no longer describes the live marker leaves
                    // the recorded history exactly as it is.
                    let identity = root_identity(selected)?;
                    if identity.describes(selected) {
                        row.generation = identity.generation(selected);
                        row.root_dev = identity.dev;
                        row.root_ino = identity.ino;
                        row.marker_digest = identity.digest;
                        row.nonce = selected.nonce().map(str::to_owned);
                        row.volume_uuid = selected.volume_uuid().map(str::to_owned);
                    }
                }
            } else if row.selected || row.pending {
                row.selected = false;
                row.pending = false;
            }
        } else {
            row.selected = false;
            row.pending = false;
        }
        if let Some(proposed) = proposed {
            if row.root == proposed.root() {
                row.pending = true;
                matched_pending = true;
            }
        }
    }
    if proposed.is_none() {
        // The receipt is authoritative for this placement: a placement with
        // no prepared transition cannot still have a pending reference. This
        // is what clears a rolled-back preparation.
        for row in registry
            .rows
            .iter_mut()
            .filter(|row| row.checkout_id == checkout_id && row.member == member)
        {
            row.pending = false;
        }
    }
    if let Some(selected) = selected {
        if keep_selected && !matched_selected {
            let identity = root_identity(selected)?;
            if registry.rows.len() >= MAX_ROWS {
                mark_incomplete(registry, "the registry row limit was reached")?;
            } else {
                let row = Row {
                    checkout_id: checkout_id.to_owned(),
                    checkout: checkout.to_path_buf(),
                    checkout_volume_uuid: checkout_volume_uuid.clone(),
                    receipt: receipt_path.to_path_buf(),
                    member: member.to_owned(),
                    root: selected.root().to_path_buf(),
                    root_kind: selected.kind(),
                    volume_uuid: selected.volume_uuid().map(str::to_owned),
                    recipe: outputs::RECIPE_PUBLIC.to_owned(),
                    nonce: selected.nonce().map(str::to_owned),
                    generation: identity.generation(selected),
                    root_dev: identity.dev,
                    root_ino: identity.ino,
                    marker_digest: identity.digest,
                    selected: keep_selected,
                    pending: false,
                    pruning: None,
                    pruned_ms: None,
                    reservation: None,
                    cancelling: None,
                };
                registry.rows.push(row);
            }
        }
    }
    if let Some(proposed) = proposed {
        if !matched_pending {
            if registry.rows.len() >= MAX_ROWS {
                mark_incomplete(registry, "the registry row limit was reached")?;
            } else {
                let identity = root_identity(proposed)?;
                registry.rows.push(Row {
                    checkout_id: checkout_id.to_owned(),
                    checkout: checkout.to_path_buf(),
                    checkout_volume_uuid: checkout_volume_uuid.clone(),
                    receipt: receipt_path.to_path_buf(),
                    member: member.to_owned(),
                    root: proposed.root().to_path_buf(),
                    root_kind: proposed.kind(),
                    volume_uuid: proposed.volume_uuid().map(str::to_owned),
                    recipe: outputs::RECIPE_PUBLIC.to_owned(),
                    nonce: proposed.nonce().map(str::to_owned),
                    generation: identity.generation(proposed),
                    root_dev: identity.dev,
                    root_ino: identity.ino,
                    marker_digest: identity.digest,
                    selected: false,
                    pending: true,
                    pruning: None,
                    pruned_ms: None,
                    reservation: None,
                    cancelling: None,
                });
            }
        }
    }
    Ok(())
}

/// The marker a reservation write returns after it durably published sticky
/// incompleteness: the caller retries once with an overflow tag.
pub const OVERFLOW_RETRY: &str =
    "the output registry reached its row limit and is now sticky-incomplete";

/// Most overflow cancellations in flight at once.
const MAX_CANCELLED_OVERFLOW: usize = 64;

/// Whether `value` is a 128-bit lowercase-hex nonce.
pub fn is_nonce(value: &str) -> bool {
    hex32(value)
}

fn overflow_key(checkout_id: &str, id: &str) -> String {
    format!("{checkout_id}/{id}")
}

/// Publish a receipt's reservation reference. An indexed reservation is a
/// pending row carrying its transaction and nonce; an overflow reservation
/// adds no row and is valid only against the durably sticky-incomplete
/// registry it names. A reservation whose cancellation began is never
/// republished, and a dropped reservation without its cancellation journal
/// is refused rather than read as non-selection.
fn note_reservation(
    registry: &mut Registry,
    receipt: &Receipt,
    receipt_path: &Path,
    checkout_volume_uuid: Option<String>,
) -> Result<(), String> {
    let checkout_id = receipt.checkout_id();
    // A reservation is root-level: every member of this checkout shares the
    // pending reference. New rows name the first sorted member (the legacy
    // `target` for a whole-target receipt).
    let member = receipt
        .links()
        .first()
        .map(|placement| placement.member().to_owned())
        .unwrap_or_else(|| outputs::MEMBER.to_owned());
    let member = member.as_str();
    let reserved = receipt.reserved();
    let current_id = reserved.map(|reserved| reserved.id.as_str());
    for row in registry
        .rows
        .iter_mut()
        .filter(|row| row.checkout_id == checkout_id)
    {
        let Some(id) = row.reservation.clone() else {
            continue;
        };
        if Some(id.as_str()) == current_id || row.cancelling.is_some() {
            continue;
        }
        // Consumed: the commit (or the prepared transition) now references
        // the reserved root itself.
        if receipt.links().iter().any(|placement| {
            placement
                .selected_ref()
                .is_some_and(|selected| selected.root() == row.root)
                || placement
                    .prepared_ref()
                    .is_some_and(|prepared| prepared.to.root() == row.root)
        }) {
            row.reservation = None;
            continue;
        }
        return Err(format!(
            "the receipt drops reservation {id} without its cancellation journal; refusing"
        ));
    }
    let Some(reserved) = reserved else {
        return Ok(());
    };
    if let Some(tag) = &reserved.overflow {
        if registry.complete || tag.registry != registry.created_by {
            return Err(
                "an overflow reservation needs the durably sticky-incomplete registry it names; refusing"
                    .to_owned(),
            );
        }
        if registry
            .cancelled_overflow
            .contains(&overflow_key(checkout_id, &reserved.id))
        {
            return Err(format!(
                "reservation {} is being cancelled; refusing to republish it",
                reserved.id
            ));
        }
        return Ok(());
    }
    if let Some(row) = registry
        .rows
        .iter_mut()
        .find(|row| row.checkout_id == checkout_id && row.root == reserved.root)
    {
        if row.cancelling.is_some() {
            return Err(format!(
                "reservation {} is being cancelled; refusing to republish it",
                reserved.id
            ));
        }
        row.reservation = Some(reserved.id.clone());
        if !row.selected {
            row.pending = true;
            row.nonce = Some(reserved.nonce.clone());
            row.generation = reserved.generation;
        }
        return Ok(());
    }
    if !registry.complete || registry.rows.len() >= MAX_ROWS {
        mark_incomplete(registry, "the registry row limit was reached")?;
        return Err(format!(
            "{OVERFLOW_RETRY}; reserve again with an overflow tag"
        ));
    }
    registry.rows.push(Row {
        checkout_id: checkout_id.to_owned(),
        checkout: receipt.checkout().to_path_buf(),
        checkout_volume_uuid,
        receipt: receipt_path.to_path_buf(),
        member: member.to_owned(),
        root: reserved.root.clone(),
        root_kind: reserved.kind,
        volume_uuid: reserved.volume_uuid.clone(),
        recipe: outputs::RECIPE_PUBLIC.to_owned(),
        nonce: Some(reserved.nonce.clone()),
        generation: reserved.generation,
        root_dev: None,
        root_ino: None,
        marker_digest: None,
        selected: false,
        pending: true,
        pruning: None,
        pruned_ms: None,
        reservation: Some(reserved.id.clone()),
        cancelling: None,
    });
    Ok(())
}

/// The overflow tag for a new reservation: `Some` only once the registry is
/// durably sticky-incomplete. Its revision is provenance only.
pub fn overflow_tag(
    control: &Path,
    id: &str,
    nonce: &str,
) -> Result<Option<outputs::OverflowTag>, String> {
    let registry = ready_registry(control)?;
    registry.validate()?;
    if registry.complete {
        return Ok(None);
    }
    Ok(Some(outputs::OverflowTag {
        registry: registry.created_by.clone(),
        transaction: id.to_owned(),
        nonce: nonce.to_owned(),
        revision: registry.revision,
    }))
}

/// Binding validation of an exact surviving reservation. Indexed: its row
/// carries the same transaction, nonce and root and no cancellation. Overflow:
/// the same registry identity is still sticky-incomplete and no cancellation
/// of this transaction began. An unrelated revision advance never refuses.
pub fn validate_reservation(control: &Path, reserved: &outputs::Reservation) -> Result<(), String> {
    let registry = ready_registry(control)?;
    registry.validate()?;
    if let Some(tag) = &reserved.overflow {
        if registry.complete || tag.registry != registry.created_by {
            return Err("its overflow tag names another registry state".to_owned());
        }
        if registry
            .cancelled_overflow
            .iter()
            .any(|key| key.ends_with(&format!("/{}", reserved.id)))
        {
            return Err("its cancellation began".to_owned());
        }
        return Ok(());
    }
    match registry
        .rows
        .iter()
        .find(|row| row.reservation.as_deref() == Some(reserved.id.as_str()))
    {
        Some(row) if row.cancelling.is_some() => Err("its cancellation began".to_owned()),
        Some(row)
            if row.nonce.as_deref() == Some(reserved.nonce.as_str())
                && row.root == reserved.root =>
        {
            Ok(())
        }
        Some(_) => Err("its registry row names another nonce or root".to_owned()),
        None => Err("no registry row backs it".to_owned()),
    }
}

/// First cancellation step, durable before the receipt drops the
/// reservation: the authority is invalidated and the revision advances.
/// Idempotent for the exact same transaction.
pub fn begin_cancel(
    control: &Path,
    checkout_id: &str,
    reserved: &outputs::Reservation,
) -> Result<(), String> {
    let _guard = store::publication_guard(control)?;
    let mut registry = ready_registry(control)?;
    registry.validate()?;
    if reserved.overflow.is_some() {
        let key = overflow_key(checkout_id, &reserved.id);
        if !registry.cancelled_overflow.contains(&key) {
            if registry.cancelled_overflow.len() >= MAX_CANCELLED_OVERFLOW {
                return Err(
                    "too many overflow reservation cancellations are unfinished; refusing"
                        .to_owned(),
                );
            }
            registry.cancelled_overflow.push(key);
        }
    } else {
        let Some(row) = registry
            .rows
            .iter_mut()
            .find(|row| row.reservation.as_deref() == Some(reserved.id.as_str()))
        else {
            return Err(format!(
                "reservation {} has no registry row; it is kept",
                reserved.id
            ));
        };
        row.cancelling = Some(reserved.id.clone());
        row.pending = false;
    }
    registry.bump()?;
    write_registry(control, &registry)
}

/// Last cancellation step, after the receipt committed without the
/// reservation: remove the abandoned row (or overflow journal entry).
pub fn finish_cancel(
    control: &Path,
    checkout_id: &str,
    reserved: &outputs::Reservation,
) -> Result<(), String> {
    let _guard = store::publication_guard(control)?;
    let mut registry = ready_registry(control)?;
    registry.validate()?;
    let key = overflow_key(checkout_id, &reserved.id);
    registry.cancelled_overflow.retain(|entry| entry != &key);
    registry
        .rows
        .retain(|row| row.cancelling.as_deref() != Some(reserved.id.as_str()));
    registry.bump()?;
    write_registry(control, &registry)
}

/// Replay of interrupted cancellations for one checkout whose receipt no
/// longer holds the reservation: finish exactly the recorded transactions.
pub fn finish_dropped_cancels(
    control: &Path,
    checkout_id: &str,
    current: Option<&str>,
) -> Result<(), String> {
    let _guard = store::publication_guard(control)?;
    let mut registry = ready_registry(control)?;
    let before = (registry.rows.len(), registry.cancelled_overflow.len());
    registry.rows.retain(|row| {
        row.checkout_id != checkout_id
            || row.cancelling.is_none()
            || row.cancelling.as_deref() == current
    });
    registry.cancelled_overflow.retain(|key| {
        !key.starts_with(&format!("{checkout_id}/"))
            || current.is_some_and(|id| key.ends_with(&format!("/{id}")))
    });
    if (registry.rows.len(), registry.cancelled_overflow.len()) == before {
        return Ok(());
    }
    registry.bump()?;
    write_registry(control, &registry)
}

/// Set the sticky incompleteness marker, once, durably.
fn mark_incomplete(registry: &mut Registry, reason: &str) -> Result<(), String> {
    if registry.complete {
        registry.complete = false;
        registry.incomplete = Some(Incomplete {
            reason: reason.to_owned(),
            since_ms: process::now_ms(),
            transaction: registry.created_by.clone(),
        });
        registry.bump()?;
    }
    Ok(())
}

/// One selected root's identity as a registry row records it.
struct RecordedRoot {
    dev: Option<u64>,
    ino: Option<u64>,
    digest: Option<String>,
    /// The root's own marker facts — its nonce and immutable creation
    /// generation — when the marker is readable right now.
    marker: Option<(Option<String>, u64)>,
}

impl RecordedRoot {
    /// The generation a row records: the root's OWN immutable creation
    /// generation (oxide#218), never the selection's commit generation, which
    /// advances with every transition while the marker keeps the generation it
    /// was created at. A root with no readable marker (a future or an absent
    /// one) falls back to the selection's generation exactly as before.
    fn generation(&self, selected: &Selected) -> u64 {
        self.marker
            .as_ref()
            .map_or_else(|| selected.generation(), |(_, generation)| *generation)
    }

    /// Whether the root's own marker records exactly this placement's nonce and
    /// creation generation, so these facts describe the live root. A readable
    /// marker that records another nonce or generation is a SUPERSEDED
    /// placement — a version-1 receipt whose root/2 marker was upgraded under
    /// it during a migration — and that placement never overwrites the row's
    /// recorded history with a mixed identity. An unreadable or absent marker
    /// has no fact to contradict the placement and is recorded as before; the
    /// prune observation refuses an invalid marker on its own.
    fn describes(&self, selected: &Selected) -> bool {
        match &self.marker {
            Some((nonce, generation)) => {
                nonce.as_deref() == selected.nonce() && *generation == selected.generation()
            }
            None => true,
        }
    }
}

fn root_identity(selected: &Selected) -> Result<RecordedRoot, String> {
    let Ok(metadata) = fs::symlink_metadata(selected.root()) else {
        return Ok(RecordedRoot {
            dev: None,
            ino: None,
            digest: None,
            marker: None,
        });
    };
    use std::os::unix::fs::MetadataExt;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok(RecordedRoot {
            dev: None,
            ino: None,
            digest: None,
            marker: None,
        });
    }
    let digest = outputs::marker_digest(selected.root()).ok();
    let marker = outputs::read_marker_for_prune(selected.root())
        .ok()
        .map(|marker| (marker.nonce().map(str::to_owned), marker.generation()));
    Ok(RecordedRoot {
        dev: Some(metadata.dev()),
        ino: Some(metadata.ino()),
        digest,
        marker,
    })
}

/// The checkout's volume UUID for a new marker (shared root/2 field).
pub fn checkout_volume_uuid(checkout: &Path) -> Option<String> {
    volume_uuid_of(checkout).ok().flatten()
}

fn volume_uuid_of(path: &Path) -> Result<Option<String>, String> {
    match oxide_storage::volume::observe_path(path) {
        Ok(facts) => Ok(facts.uuid),
        Err(_) => Ok(None),
    }
}

fn write_registry(control: &Path, registry: &Registry) -> Result<(), String> {
    registry.validate()?;
    let directory = registry_dir(control);
    ensure_private_dir(&directory)?;
    let bytes = serde_json::to_vec(registry).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("the output registry reached its byte limit".to_owned());
    }
    let path = registry_path(control);
    let temp = directory.join(format!(".registry.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    drop(file);
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output registry: {error}")
    })?;
    sync_directory(&directory)
}

/// The default recorded status lines for the pool section. Reads only a
/// bounded observations file from the internal control directory; a missing,
/// corrupt, oversized or foreign document means "not recorded", never a
/// fallback scan.
pub fn status_lines(control: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let path = observations_path(control);
    let observed = store::private_file(&path, false).ok().flatten();
    let Some(file) = observed else {
        lines.push("prune: no recorded prune observation".to_owned());
        return lines;
    };
    let mut bytes = Vec::new();
    let Ok(read) = file
        .take(MAX_OBSERVATIONS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    else {
        lines.push("prune: recorded observation is unreadable".to_owned());
        return lines;
    };
    if read as usize > MAX_OBSERVATIONS_BYTES {
        lines.push("prune: recorded observation exceeds its byte limit".to_owned());
        return lines;
    }
    let Ok(observation) = serde_json::from_slice::<Observations>(&bytes) else {
        lines.push("prune: recorded observation is invalid".to_owned());
        return lines;
    };
    if observation.version != OBSERVATIONS_VERSION
        || observation.roots.len() > MAX_OBSERVATION_ROOTS
    {
        lines.push("prune: recorded observation has an unsupported schema".to_owned());
        return lines;
    }
    lines.push(format!(
        "prune: {} ({}; registry revision {}; coverage {}/{} roots{})",
        age_text(observation.generated_ms),
        observation.source,
        observation.registry_revision,
        observation.coverage.candidate_roots,
        observation.coverage.registry_roots,
        if observation.coverage.complete {
            ""
        } else {
            ", partial"
        }
    ));
    for root in observation.roots.iter().take(8) {
        let accounting = if root.accounting_complete {
            format!(
                "{} logical / {} allocated bytes",
                root.logical_bytes, root.allocated_bytes
            )
        } else {
            "unknown/incomplete accounting".to_owned()
        };
        lines.push(format!(
            "  {} {} {} ({}, {})",
            root.outcome,
            root.checkout_id,
            root.root.display(),
            root.intent.as_deref().unwrap_or("no intent"),
            accounting,
        ));
    }
    if observation.roots.len() > 8 {
        lines.push(format!(
            "  ... {} more recorded root(s), not rendered",
            observation.roots.len() - 8
        ));
    }
    lines
}

fn age_text(ms: u64) -> String {
    let now = process::now_ms();
    let seconds = now.saturating_sub(ms) / 1000;
    match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn write_observations(control: &Path, observation: &Observations) -> Result<(), String> {
    let directory = registry_dir(control);
    ensure_private_dir(&directory)?;
    let bytes = serde_json::to_vec(observation).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_OBSERVATIONS_BYTES {
        return Err("the prune observation exceeds its byte limit".to_owned());
    }
    let temp = directory.join(format!(".observations.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", temp.display()))?;
    drop(file);
    fs::rename(&temp, &observations_path(control))
        .map_err(|error| format!("cannot publish the prune observation: {error}"))?;
    sync_directory(&directory)
}

/// Snapshot limits for the one bounded whole-registry read.
const SNAPSHOT_MAX_RECEIPTS: usize = 8192;
const SNAPSHOT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SNAPSHOT_TIME: Duration = Duration::from_secs(10);

// A snapshot's shared input budget is scoped to this synchronous command
// thread. The latched failure survives callers that translate an I/O error
// into an unknown custody result.
thread_local! {
    static AUTHORITY: std::cell::RefCell<Option<AuthorityBudget>> = const { std::cell::RefCell::new(None) };
}

struct AuthorityScope {
    /// The window's fresh-custody witness (oxide#235), created on its first
    /// custody decision and dropped with the window, so no parsed state,
    /// descriptor or verdict crosses an authority window.
    custody: Option<store::CustodyWitness>,
}

struct AuthorityBudget {
    deadline: Instant,
    remaining: u64,
    failed: bool,
}

impl AuthorityScope {
    fn begin() -> Result<Self, String> {
        AUTHORITY.with(|cell| {
            let mut active = cell.borrow_mut();
            if active.is_some() {
                return Err("nested authority snapshot".to_owned());
            }
            #[cfg(not(test))]
            {
                *active = Some(AuthorityBudget {
                    deadline: Instant::now() + SNAPSHOT_TIME,
                    remaining: SNAPSHOT_MAX_BYTES,
                    failed: false,
                });
            }
            #[cfg(test)]
            {
                let started = Instant::now();
                crate::tape::monotonic_observed("authority:begin:window-offset-ns", started);
                *active = Some(AuthorityBudget {
                    deadline: started + SNAPSHOT_TIME,
                    remaining: SNAPSHOT_MAX_BYTES,
                    failed: false,
                });
            }
            #[cfg(test)]
            crate::tape::budget_begin(SNAPSHOT_MAX_BYTES);
            Ok(Self { custody: None })
        })
    }

    fn complete(&self) -> bool {
        AUTHORITY.with(|cell| {
            cell.borrow().as_ref().is_some_and(|budget| {
                #[cfg(not(test))]
                {
                    !budget.failed && budget.remaining > 0 && Instant::now() <= budget.deadline
                }
                #[cfg(test)]
                {
                    if budget.failed || budget.remaining == 0 {
                        false
                    } else {
                        let now = Instant::now();
                        crate::tape::monotonic_observed("authority:complete:window-offset-ns", now);
                        now <= budget.deadline
                    }
                }
            })
        })
    }

    /// One row's custody through the fresh-witness route inside this window
    /// (oxide#235), creating the window's witness on first use. `Err` refuses
    /// the whole authority batch.
    fn fresh_custody(
        &mut self,
        control: &Path,
        checkout: &Path,
        cleans: bool,
    ) -> Result<store::RowCustody, String> {
        if self.custody.is_none() {
            self.custody = Some(store::CustodyWitness::open(control)?);
        }
        let deadline = AUTHORITY.with(|cell| cell.borrow().as_ref().map(|budget| budget.deadline));
        let result = self
            .custody
            .as_mut()
            .expect("the witness was just created")
            .decide(deadline, checkout, cleans);
        #[cfg(debug_assertions)]
        if result.is_ok()
            && prune_fixture_fault(
                control,
                "OXIDE_TEST_PRUNE_WITNESS_PAUSE",
                checkout,
                "witness-paused",
            )
        {
            let evidence = proof_evidence().ok_or("fixture evidence unavailable")?;
            while !evidence.join("witness-release").exists() {
                if deadline.is_none_or(|end| Instant::now() >= end) {
                    return Err("fixture pause reached the authority deadline".to_owned());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        result
    }
}

impl Drop for AuthorityScope {
    fn drop(&mut self) {
        #[cfg(test)]
        AUTHORITY.with(|cell| {
            if let Some(budget) = cell.borrow().as_ref() {
                crate::tape::budget_end(budget.failed, budget.remaining);
            }
        });
        AUTHORITY.with(|cell| *cell.borrow_mut() = None);
    }
}

/// Charge bytes actually returned by each bounded authority input read.
/// Outside a snapshot this is exactly the ordinary read operation.
pub(crate) fn authority_read<R: Read>(reader: R, bytes: &mut Vec<u8>) -> std::io::Result<usize> {
    let remaining = AUTHORITY.with(|cell| cell.borrow().as_ref().map(|budget| budget.remaining));
    let before = bytes.len();
    let result = match remaining {
        Some(allowance) => reader.take(allowance).read_to_end(bytes),
        None => reader.take(u64::MAX).read_to_end(bytes),
    };
    let used = (bytes.len() - before) as u64;
    let exhausted = AUTHORITY.with(|cell| {
        let mut active = cell.borrow_mut();
        let Some(budget) = active.as_mut() else {
            return false;
        };
        budget.remaining = budget.remaining.saturating_sub(used);
        #[cfg(not(test))]
        if result.is_err() || budget.remaining == 0 || Instant::now() > budget.deadline {
            budget.failed = true;
        }
        #[cfg(test)]
        {
            let expired = if result.is_err() || budget.remaining == 0 {
                false
            } else {
                let now = Instant::now();
                crate::tape::monotonic_observed("authority:read:window-offset-ns", now);
                now > budget.deadline
            };
            if result.is_err() || budget.remaining == 0 || expired {
                budget.failed = true;
            }
        }
        budget.failed
    });
    #[cfg(test)]
    crate::tape::authority_core(
        bytes,
        SNAPSHOT_MAX_BYTES,
        exhausted,
        result.is_err() || exhausted,
    );
    if exhausted && result.is_ok() {
        return Err(std::io::Error::other(
            "the snapshot exceeded its input or time budget",
        ));
    }
    result
}

// Preserve the evaluator thread's errno around test-only observation.
#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
fn tape_errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: libc returns this thread's errno location.
    Some(unsafe { libc::__errno_location() })
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
fn tape_errno_slot() -> Option<*mut libc::c_int> {
    // SAFETY: libc returns this thread's errno location.
    Some(unsafe { libc::__error() })
}

#[cfg(all(
    test,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios"
    ))
))]
fn tape_errno_slot() -> Option<*mut libc::c_int> {
    None
}

#[cfg(test)]
pub(crate) fn tape_errno() -> Option<libc::c_int> {
    // SAFETY: the pointer, when available, is this thread's errno slot.
    tape_errno_slot().map(|slot| unsafe { *slot })
}

#[cfg(test)]
pub(crate) fn tape_restore_errno(value: Option<libc::c_int>) {
    if let (Some(slot), Some(value)) = (tape_errno_slot(), value) {
        // SAFETY: the pointer is this thread's errno slot.
        unsafe { *slot = value };
    }
}

/// Test-only observation of the exact descriptor passed to authority_read.
/// The production call below remains unchanged; the descriptor stays open
/// until the real read result has been recorded.
#[cfg(test)]
pub(crate) fn authority_read_observed(
    file: &File,
    path: &Path,
    site: &str,
    limit: u64,
    bytes: &mut Vec<u8>,
) -> std::io::Result<usize> {
    use std::os::unix::fs::MetadataExt;
    // The single test-only fstat observes this held descriptor. It never
    // changes the evaluator result, including when the observation fails.
    let before_observation = tape_errno();
    let metadata = if site == "prune::read_bounded" {
        Some(file.metadata())
    } else {
        None
    };
    if site == "prune::read_bounded" {
        if let Some(Ok(metadata)) = &metadata {
            crate::tape::read_row_without_parent(
                "read-bounded:open",
                path,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                limit,
            );
        }
    }
    tape_restore_errno(before_observation);
    let result = authority_read(file.take(limit + 1), bytes);
    let after_read = tape_errno();
    let outcome = match &result {
        Ok(_) => crate::tape::TapeOutcome::Digest {
            digest: crate::tape::sha256_hex(bytes),
        },
        Err(error) => crate::tape::TapeOutcome::Refusal {
            reason: error.to_string(),
        },
    };
    match metadata {
        Some(Ok(metadata)) => crate::tape::authority_bind_descriptor(
            site,
            path,
            metadata.dev(),
            metadata.ino(),
            limit,
            outcome,
        ),
        Some(Err(error)) => {
            crate::tape::authority_unbound(site, &error.to_string());
            crate::tape::read_row(&format!("authority-read@{site}"), path, outcome, limit);
        }
        None => {
            crate::tape::authority_unbound(site, "no authorized descriptor observation");
            crate::tape::read_row(&format!("authority-read@{site}"), path, outcome, limit);
        }
    }
    tape_restore_errno(after_read);
    result
}

/// One evaluated root: either a candidate or an explicit keep reason.
struct Evaluated {
    row: Row,
    candidate: Option<Candidate>,
    kept: Option<String>,
    legacy: bool,
    /// The real run's own ephemeral absence evidence for this root. It is not
    /// part of the preview or any other serialized document.
    evidence: Option<SnapshotEvidence>,
}

/// The authority result is distinct from per-root eligibility. A partial
/// authority view never supplies candidates or diagnostic observations.
struct Evaluation {
    entries: Vec<Evaluated>,
    complete: bool,
}

impl std::ops::Deref for Evaluation {
    type Target = [Evaluated];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl<'a> IntoIterator for &'a Evaluation {
    type Item = &'a Evaluated;
    type IntoIter = std::slice::Iter<'a, Evaluated>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

/// `oxide outputs prune --dry-run`
fn dry_run(operator_list: Option<&Path>) -> Result<u8, String> {
    let mut authority = AuthorityScope::begin()?;
    let store = paired_store()?;
    let control = store.owner_control().to_path_buf();
    let list = match operator_list {
        Some(path) => Some(read_operator_list(path)?),
        None => None,
    };
    // A dry run mutates nothing: it never initializes, replays or reconciles.
    let registry = match require_ready(&store, &control) {
        Ok(registry) => registry,
        Err(reason) => return Ok(refused(&reason)),
    };
    if let Some(intent) = registry.unfinished_intents().next() {
        return Ok(unsettled(
            &control,
            &format!(
                "prune intent {} is unfinished ({}); settle it with `oxide outputs resolve-intent {}` or restore the recorded root before pruning",
                intent.id,
                phase_label(intent.phase),
                intent.id
            ),
        ));
    }
    let evaluated = evaluate(&store, &control, &registry, list.as_ref(), &mut authority)?;
    drop(authority);
    let preview = build_preview(&registry, &evaluated, list.as_ref(), evaluated.complete)?;
    preview.validate()?;
    let bytes = serde_json::to_vec(&preview).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(format!(
            "the prune preview exceeds its {MAX_PLAN_BYTES}-byte limit; narrow the operator list"
        ));
    }
    let sha = sha256_hex(&bytes);
    // Exactly these bytes are the plan a real run must present; a newline is
    // stderr's, never part of the hashed document.
    std::io::stdout()
        .write_all(&bytes)
        .and_then(|()| std::io::stdout().flush())
        .map_err(|error| format!("cannot write the prune preview: {error}"))?;
    eprintln!();
    let deferred = preview
        .coverage
        .candidate_roots
        .saturating_sub(preview.totals.candidates);
    if deferred > 0 {
        eprintln!(
            "oxide: prune preview SHA-256 {sha} ({} candidate root(s) of {} eligible, {deferred} deferred to a later batch, {} kept); nothing was changed. Write these exact bytes to a file and re-run with --plan FILE --plan-sha256 {sha}",
            preview.totals.candidates, preview.coverage.candidate_roots, preview.totals.kept
        );
    } else {
        eprintln!(
            "oxide: prune preview SHA-256 {sha} ({} candidate root(s), {} kept); nothing was changed. Write these exact bytes to a file and re-run with --plan FILE --plan-sha256 {sha}",
            preview.totals.candidates, preview.totals.kept
        );
    }
    Ok(0)
}

impl Preview {
    fn validate(&self) -> Result<(), String> {
        if self.version != PREVIEW_VERSION || self.candidates.len() > MAX_CANDIDATES {
            return Err("the prune preview has an unsupported schema or too many roots".to_owned());
        }
        if let Some(sha) = &self.operator_list_sha256 {
            if !hex64(sha) {
                return Err("the prune preview has an unusable operator-list digest".to_owned());
            }
        }
        Ok(())
    }
}

/// `oxide outputs prune --plan FILE --plan-sha256 HEX`
fn real_run(plan: &str, sha: &str, operator_list: Option<&Path>) -> Result<u8, String> {
    if !hex64(sha) {
        return Ok(refused(
            "the plan digest must be 64 lowercase hexadecimal characters; nothing was changed",
        ));
    }
    let bytes = read_bounded(Path::new(plan), MAX_PLAN_BYTES, "prune plan")?;
    if sha256_hex(&bytes) != sha {
        return Ok(refused(
            "the plan file's bytes do not match --plan-sha256; nothing was changed",
        ));
    }
    let preview: Preview = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the prune plan is invalid: {error}; nothing was changed"))?;
    preview.validate()?;
    let list = match operator_list {
        Some(path) => {
            let list = read_operator_list(path)?;
            let expected = list.sha256.clone();
            if preview.operator_list_sha256.as_deref() != Some(expected.as_str()) {
                return Ok(refused(
                    "the operator list bytes differ from the list the preview recorded; nothing was changed",
                ));
            }
            Some(list)
        }
        None => {
            if preview.operator_list_sha256.is_some() {
                return Ok(refused(
                    "the plan was made with --operator-list, so the same list is required; nothing was changed",
                ));
            }
            None
        }
    };
    let store = paired_store()?;
    let control = store.owner_control().to_path_buf();
    let mut registry = match require_ready(&store, &control) {
        Ok(registry) => registry,
        Err(reason) => return Ok(refused(&reason)),
    };
    if !registry.complete {
        return Ok(refused(
            "the output registry is sticky-incomplete, so prune keeps every root; nothing was changed",
        ));
    }
    // An unfinished intent is resumed only for its exact recorded
    // transaction; anything it cannot prove stays pending (exit 3). An
    // intent-phase journal with no filesystem effect is rolled back to its
    // pre-intent selection, which is not a completion: its root stays in
    // the plan and is re-evaluated below like any other candidate.
    let pending: Vec<Intent> = registry.unfinished_intents().cloned().collect();
    let mut completed_roots = Vec::new();
    let mut rolled_back = 0usize;
    for intent in &pending {
        let planned = preview.candidates.iter().any(|candidate| {
            candidate.checkout_id == intent.identity.checkout_id
                && candidate.member == intent.identity.member
                && candidate.root == intent.identity.root
        }) || matches!(intent.phase, IntentPhase::Deleted | IntentPhase::Resolved);
        if !planned {
            return Ok(unsettled(
                &control,
                &format!(
                    "prune intent {} for {} is unfinished but is not in this plan; settle it before another prune",
                    intent.id,
                    intent.identity.root.display()
                ),
            ));
        }
        // Every replay effect (rollback, deletion, terminal receipt) runs
        // under the checkout's clean barrier and binding lock.
        match under_checkout_barrier(&store, &intent.identity.checkout, || {
            resume_intent(&store, &control, intent)
        }) {
            Ok(Resumed::Completed) => completed_roots.push(intent.identity.root.clone()),
            Ok(Resumed::RolledBack) => rolled_back += 1,
            Ok(Resumed::Settled) => {}
            Err(reason) => {
                return Ok(unsettled(
                    &control,
                    &format!("prune intent {} cannot resume: {reason}", intent.id),
                ))
            }
        }
    }
    if rolled_back > 0 {
        eprintln!(
            "oxide: prune rolled back {rolled_back} journaled intent(s) that had no filesystem effect; their roots are re-evaluated"
        );
    }
    // Start one fresh authority window after any exact intent replay. The
    // registry and optional list are re-read inside that window.
    let mut authority = AuthorityScope::begin()?;
    let fresh_store = paired_store()?;
    if fresh_store.owner_control() != control {
        return Ok(refused("the internal pool changed during revalidation"));
    }
    let store = fresh_store;
    registry = match ready_registry(&control) {
        Ok(registry) => registry,
        Err(reason) if !completed_roots.is_empty() => {
            return Ok(partial(
                &completed_roots,
                &format!("the fresh registry failed ({reason})"),
            ));
        }
        Err(reason) if rolled_back > 0 => {
            return Ok(unsettled(&control, &format!("a replayed intent settled but the registry became unreadable afterwards ({reason})")));
        }
        Err(reason) => return Ok(refused(&reason)),
    };
    let fresh_list = match operator_list {
        Some(path) => {
            let fresh = match read_operator_list(path) {
                Ok(fresh) => fresh,
                Err(reason) if !completed_roots.is_empty() => {
                    return Ok(partial(&completed_roots, &reason))
                }
                Err(reason) => return Ok(refused(&reason)),
            };
            if list.as_ref().map(|old| &old.sha256) != Some(&fresh.sha256) {
                let reason = "the operator list changed during revalidation";
                return Ok(if completed_roots.is_empty() {
                    refused(reason)
                } else {
                    partial(&completed_roots, reason)
                });
            }
            Some(fresh)
        }
        None => None,
    };
    let evaluated = match evaluate(
        &store,
        &control,
        &registry,
        fresh_list.as_ref(),
        &mut authority,
    ) {
        Ok(evaluated) => evaluated,
        Err(reason) if !completed_roots.is_empty() => {
            return Ok(partial(
                &completed_roots,
                &format!("the fresh snapshot failed ({reason}), so every other root is kept"),
            ))
        }
        Err(reason) => return Err(reason),
    };
    drop(authority);
    // A failed fresh preview after exact intent replay is a partial
    // completion: those roots already changed. Mapping the error through
    // `?` would print "no output root was changed" and exit 2.
    let fresh = match build_preview(
        &registry,
        &evaluated,
        fresh_list.as_ref(),
        evaluated.complete,
    ) {
        Ok(fresh) => fresh,
        Err(reason) if !completed_roots.is_empty() => {
            return Ok(partial(
                &completed_roots,
                &format!("the fresh preview failed ({reason}), so every other root is kept"),
            ));
        }
        Err(reason) => return Err(reason),
    };
    if !fresh.coverage.complete {
        let reason = "the fresh authority snapshot is incomplete, so every remaining root is kept";
        return Ok(if completed_roots.is_empty() {
            refused(reason)
        } else {
            partial(&completed_roots, reason)
        });
    }
    let eligible: Vec<Candidate> = evaluated
        .iter()
        .filter_map(|entry| entry.candidate.clone())
        .collect();
    let Some(authorized) = authorize_plan(
        &preview.candidates,
        &fresh.candidates,
        &eligible,
        &completed_roots,
    ) else {
        let kept = kept_note(&fresh);
        if !completed_roots.is_empty() {
            return Ok(partial(
                &completed_roots,
                &format!(
                    "the eligible root set no longer matches the plan, so every other root is kept ({kept}); run a new dry run before another prune"
                ),
            ));
        }
        return Ok(refused(&format!(
            "the eligible root set changed since the preview ({kept}); no output root was changed"
        )));
    };
    if authorized.is_empty() {
        if completed_roots.is_empty() {
            eprintln!("oxide: no eligible roots; nothing was deleted");
        } else {
            println!(
                "outputs prune: completed {} previously journaled root(s); no fresh candidate remains",
                completed_roots.len()
            );
        }
        let _ = write_observations(
            &control,
            &observation_from(&fresh, &evaluated, Vec::new(), None),
        );
        return Ok(0);
    }
    let mut outcomes: Vec<(Candidate, Option<String>)> = Vec::new();
    // A per-root refusal before its intent keeps that root unchanged and
    // the batch continues; only a journaled intent stops it.
    let mut refusals: Vec<(PathBuf, String)> = Vec::new();
    for candidate in authorized {
        // The real run's own ancestry witness is the baseline every later
        // proof is compared with. A candidate without one is never pruned.
        let Some(evidence) = evidence_for(&evaluated, candidate) else {
            refusals.push((
                candidate.root.clone(),
                "the real run holds no ancestry witness for this root".to_owned(),
            ));
            continue;
        };
        match prune_one(&store, &control, candidate, sha, list.as_ref(), evidence) {
            Ok(()) => outcomes.push((candidate.clone(), None)),
            Err(Stop::Pending(reason)) => {
                let _ = write_observations(
                    &control,
                    &observation_from(&fresh, &evaluated, outcomes, Some(&candidate.root)),
                );
                return Ok(unsettled(&control, &reason));
            }
            Err(Stop::Refuse(reason)) => refusals.push((candidate.root.clone(), reason)),
        }
    }
    let deleted: Vec<PathBuf> = outcomes
        .iter()
        .map(|(candidate, _)| candidate.root.clone())
        .collect();
    let _ = write_observations(
        &control,
        &observation_from(&fresh, &evaluated, outcomes, None),
    );
    if deleted.is_empty() && completed_roots.is_empty() {
        for (root, reason) in &refusals {
            eprintln!("oxide: prune kept {}: {reason}", root.display());
        }
        return Ok(refused(
            "every candidate root was kept by a fresh check before its intent; no output root was changed",
        ));
    }
    let revision = ready_registry(&control)
        .map(|registry| registry.revision.to_string())
        .unwrap_or_else(|reason| format!("unreadable ({reason})"));
    for root in completed_roots.iter().chain(&deleted) {
        println!("outputs prune: deleted {}", root.display());
    }
    println!(
        "outputs prune: deleted {} root(s) whose checkout was positively absent ({} by exact intent replay); registry revision {revision}",
        completed_roots.len() + deleted.len(),
        completed_roots.len()
    );
    for (root, reason) in &refusals {
        println!(
            "outputs prune: kept {} (refused before its intent: {reason})",
            root.display()
        );
    }
    for kept in &fresh.kept {
        println!(
            "outputs prune: kept {} ({})",
            kept.root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "root".to_owned()),
            kept.reason
        );
    }
    Ok(0)
}

/// Exit 0 for a batch that completed some roots and kept the rest with no
/// unfinished intent: the exact partial completion is listed.
fn partial(completed: &[PathBuf], kept: &str) -> u8 {
    for root in completed {
        println!(
            "outputs prune: deleted {} (exact intent replay)",
            root.display()
        );
    }
    println!(
        "outputs prune: partial completion: {} root(s) deleted; {kept}",
        completed.len()
    );
    0
}

/// The real run's own ephemeral absence evidence for one fresh candidate. It
/// is looked up in the snapshot that produced the candidate, so the plan's
/// bytes never have to carry an ancestry witness.
fn evidence_for<'a>(
    evaluated: &'a Evaluation,
    candidate: &Candidate,
) -> Option<&'a SnapshotEvidence> {
    evaluated
        .iter()
        .find(|entry| {
            entry.candidate.as_ref().is_some_and(|current| {
                current.checkout_id == candidate.checkout_id
                    && current.member == candidate.member
                    && current.root == candidate.root
            })
        })
        .and_then(|entry| entry.evidence.as_ref())
}

/// Whether two candidates are the same root identity.
///
/// The checkout, member, root, nonce, inode and marker digest are the root's
/// immutable identity. The device is not: macOS renumbers a mounted volume's
/// `st_dev` after a reboot or reattach, so a plan made before one still binds the
/// same unchanged roots. The fresh snapshot's own device is the one every later
/// step of this run binds (`RootSession::open`), and a renumbered inode, a
/// changed marker or a different volume still refuses the plan.
fn same_identity(left: &Candidate, right: &Candidate) -> bool {
    left.checkout_id == right.checkout_id
        && left.member == right.member
        && left.root == right.root
        && left.nonce == right.nonce
        && left.ino == right.ino
        && left.marker_digest == right.marker_digest
}

/// Whether the plan's selected batch is exactly the fresh snapshot's selected
/// batch. Length and identity must match one-to-one; order does not.
fn same_candidates(plan: &[&Candidate], fresh: &[&Candidate]) -> bool {
    plan.len() == fresh.len()
        && plan
            .iter()
            .all(|planned| fresh.iter().any(|current| same_identity(planned, current)))
}

/// The identities this real run may prune: the plan's remaining selected
/// batch, rebound to the fresh snapshot's own candidate objects.
///
/// A first attempt requires the fresh bounded selection to equal the plan.
/// After exact intent replay, remaining plan identities must still be eligible
/// and unchanged; deferred or newly eligible roots are not backfilled into the
/// already-authorized set.
fn authorize_plan<'a>(
    plan: &'a [Candidate],
    fresh_selected: &'a [Candidate],
    fresh_eligible: &'a [Candidate],
    completed: &[PathBuf],
) -> Option<Vec<&'a Candidate>> {
    let remaining_plan: Vec<&Candidate> = plan
        .iter()
        .filter(|candidate| !completed.contains(&candidate.root))
        .collect();
    if completed.is_empty() {
        let fresh: Vec<&Candidate> = fresh_selected.iter().collect();
        same_candidates(&remaining_plan, &fresh).then_some(fresh)
    } else {
        let mut matching = Vec::with_capacity(remaining_plan.len());
        for planned in remaining_plan {
            matching.push(
                fresh_eligible
                    .iter()
                    .find(|current| same_identity(planned, current))?,
            );
        }
        Some(matching)
    }
}

fn kept_note(preview: &Preview) -> String {
    preview
        .kept
        .iter()
        .map(|kept| {
            format!(
                "{}: {}",
                kept.root
                    .as_ref()
                    .map(|root| root.display().to_string())
                    .unwrap_or_else(|| kept
                        .checkout_id
                        .clone()
                        .unwrap_or_else(|| "root".to_owned())),
                kept.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn phase_label(phase: IntentPhase) -> &'static str {
    match phase {
        IntentPhase::Intent => "journaled",
        IntentPhase::Renamed => "renamed into quarantine",
        IntentPhase::Deleting => "deleting",
        IntentPhase::MarkerLast => "deleting, marker last",
        IntentPhase::Deleted => "deleted",
        IntentPhase::Resolved => "resolved",
    }
}

fn refused(reason: &str) -> u8 {
    eprintln!("oxide: prune refused: {reason}");
    2
}

fn unsettled(control: &Path, reason: &str) -> u8 {
    eprintln!("oxide: prune unsettled: {reason}");
    if let Ok(registry) = ready_registry(control) {
        for intent in registry.unfinished_intents() {
            eprintln!(
                "oxide: pending intent {} at {} ({})",
                intent.id,
                intent.identity.root.display(),
                phase_label(intent.phase)
            );
        }
    }
    3
}

/// Read and validate one operator list: a JSON array of absolute root paths,
/// no `.` or `..` components, no duplicates, bounded bytes.
struct OperatorList {
    roots: Vec<PathBuf>,
    sha256: String,
}

fn read_operator_list(path: &Path) -> Result<OperatorList, String> {
    let bytes = read_bounded(path, MAX_OPERATOR_LIST_BYTES, "operator list")?;
    let roots: Vec<PathBuf> = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "{} is not a JSON array of root paths: {error}",
            path.display()
        )
    })?;
    if roots.len() > MAX_CANDIDATES {
        return Err(format!(
            "{} names more than {MAX_CANDIDATES} roots",
            path.display()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for root in &roots {
        if !root.is_absolute()
            || root.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
        {
            return Err(format!(
                "{} names a non-absolute or relative root {}",
                path.display(),
                root.display()
            ));
        }
        if !seen.insert(root.clone()) {
            return Err(format!(
                "{} names {} twice; duplicates refuse",
                path.display(),
                root.display()
            ));
        }
    }
    Ok(OperatorList {
        roots,
        sha256: sha256_hex(&bytes),
    })
}

fn read_bounded(path: &Path, limit: usize, what: &str) -> Result<Vec<u8>, String> {
    #[cfg(not(test))]
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?;
    #[cfg(test)]
    let metadata = {
        use std::os::unix::fs::MetadataExt;
        let result = fs::symlink_metadata(path);
        let saved_errno = crate::tape::observer_errno();
        match &result {
            Ok(metadata) => crate::tape::read_row_without_parent(
                "read-bounded:lstat",
                path,
                metadata.dev(),
                metadata.ino(),
                crate::tape::TapeOutcome::Opened {
                    dev: metadata.dev(),
                    ino: metadata.ino(),
                },
                limit as u64,
            ),
            Err(error) => crate::tape::read_row(
                "read-bounded:lstat",
                path,
                if error.kind() == std::io::ErrorKind::NotFound {
                    crate::tape::TapeOutcome::Absent
                } else {
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    }
                },
                limit as u64,
            ),
        }
        crate::tape::restore_observer_errno(saved_errno);
        result.map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("the {what} {} is not a real file", path.display()));
    }
    if metadata.len() > limit as u64 {
        return Err(format!(
            "the {what} {} exceeds its {limit}-byte limit",
            path.display()
        ));
    }
    #[cfg(not(test))]
    let file = File::open(path)
        .map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?;
    #[cfg(test)]
    let file = {
        let result = File::open(path);
        let saved_errno = crate::tape::observer_errno();
        let outcome = match &result {
            Ok(_) => crate::tape::TapeOutcome::Facts {
                facts: "opened; descriptor identity pending".to_owned(),
            },
            Err(error) => crate::tape::TapeOutcome::Refusal {
                reason: error.to_string(),
            },
        };
        crate::tape::read_row("read-bounded:open", path, outcome, limit as u64);
        crate::tape::restore_observer_errno(saved_errno);
        result.map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?
    };
    let mut bytes = Vec::new();
    #[cfg(not(test))]
    authority_read(file.take(limit as u64 + 1), &mut bytes)
        .map_err(|error| format!("cannot read the {what} {}: {error}", path.display()))?;
    #[cfg(test)]
    authority_read_observed(&file, path, "prune::read_bounded", limit as u64, &mut bytes)
        .map_err(|error| format!("cannot read the {what} {}: {error}", path.display()))?;
    if bytes.len() > limit {
        return Err(format!(
            "the {what} {} exceeds its {limit}-byte limit",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Evaluate every live registry row against the whole snapshot. Any
/// malformed, unreadable, oversized or incomplete input keeps the whole
/// batch: no candidate is produced from a partial view.
fn evaluate(
    store: &Store,
    control: &Path,
    registry: &Registry,
    list: Option<&OperatorList>,
    authority: &mut AuthorityScope,
) -> Result<Evaluation, String> {
    let mut budget = SNAPSHOT_MAX_BYTES;
    let mut receipts: std::collections::HashMap<String, Result<Receipt, String>> =
        std::collections::HashMap::new();
    let mut evaluated = Vec::new();
    // Eligibility and candidate construction are per (checkout, root): every
    // member of one root under the same checkout is ONE owner, so one
    // representative is evaluated and the full group is verified again at
    // intent publication. A group carrying a pending or otherwise unsettled
    // reference is kept, never offered as a candidate.
    let rows = representatives(
        registry
            .rows
            .iter()
            .filter(|row| row.live() || row.pruned_ms.is_none())
            .cloned()
            .collect(),
    );
    if rows.len() > SNAPSHOT_MAX_RECEIPTS {
        // A cap is not permission to consider part of the registry.
        return Ok(Evaluation {
            entries: rows
                .into_iter()
                .map(|row| Evaluated {
                    row,
                    candidate: None,
                    evidence: None,
                    kept: Some(
                        "the registry exceeds the snapshot receipt cap; the whole batch is kept"
                            .to_owned(),
                    ),
                    legacy: false,
                })
                .collect(),
            complete: false,
        });
    }
    let mut snapshot_error: Option<String> = if registry.complete {
        None
    } else {
        Some("the registry is sticky-incomplete".to_owned())
    };
    for row in rows {
        let entry = receipts.entry(row.checkout_id.clone()).or_insert_with(|| {
            match outputs::read_receipt_for_prune(control, &row.checkout, &row.checkout_id) {
                Ok(Some(receipt)) => Ok(receipt),
                Ok(None) => Err(format!(
                    "the receipt {} is missing",
                    outputs::receipt_path_public(control, &row.checkout_id).display()
                )),
                Err(reason) => Err(reason),
            }
        });
        let receipt = match entry.as_ref() {
            Ok(receipt)
                if receipt.checkout() != row.checkout
                    || receipt.checkout_id() != row.checkout_id
                    || row.receipt != outputs::receipt_path_public(control, &row.checkout_id) =>
            {
                snapshot_error
                    .get_or_insert_with(|| "the registry and receipt identity disagree".to_owned());
                None
            }
            Ok(receipt) => Some(receipt),
            Err(reason) => {
                snapshot_error.get_or_insert_with(|| reason.clone());
                None
            }
        };
        if !authority.complete() {
            snapshot_error.get_or_insert_with(|| "the snapshot exceeded its time limit".to_owned());
        }
        let mut out = Evaluated {
            row: row.clone(),
            candidate: None,
            kept: None,
            legacy: row.nonce.is_none(),
            evidence: None,
        };
        let mut evidence = None;
        let reason = if snapshot_error.is_some() {
            Ok(Some("the registry snapshot is incomplete".to_owned()))
        } else {
            eligibility(
                store,
                control,
                registry,
                &row,
                receipt,
                list,
                authority,
                &mut budget,
                false,
                &mut evidence,
            )
        };
        match reason {
            Ok(None) => {
                let facts = match observe_row_root(&row) {
                    Ok(RootState::Present(facts)) => facts,
                    Ok(RootState::Absent) => {
                        out.kept = Some(
                            "the recorded root is already absent; nothing to delete".to_owned(),
                        );
                        evaluated.push(out);
                        continue;
                    }
                    Ok(RootState::Unknown(reason)) | Err(reason) => {
                        snapshot_error.get_or_insert_with(|| reason.clone());
                        out.kept = Some(reason);
                        evaluated.push(out);
                        continue;
                    }
                };
                if facts.legacy {
                    out.legacy = true;
                }
                out.evidence = evidence;
                out.candidate = Some(Candidate {
                    checkout_id: row.checkout_id.clone(),
                    checkout: row.checkout.clone(),
                    receipt: row.receipt.clone(),
                    member: row.member.clone(),
                    root: row.root.clone(),
                    root_kind: row.root_kind,
                    volume_uuid: row.volume_uuid.clone(),
                    checkout_volume_uuid: row.checkout_volume_uuid.clone(),
                    nonce: row.nonce.clone(),
                    generation: row.generation,
                    dev: facts.dev,
                    ino: facts.ino,
                    marker_digest: facts.marker_digest,
                    logical_bytes: 0,
                    allocated_bytes: 0,
                    accounting_complete: false,
                    receiptless: false,
                });
            }
            Ok(Some(reason)) => out.kept = Some(reason),
            Err(reason) => {
                snapshot_error.get_or_insert_with(|| reason.clone());
                out.kept = Some(reason);
            }
        }
        evaluated.push(out);
    }
    // Operator-listed roots that no registry row names: only a receiptless
    // legacy root with every other predicate positive can be a candidate.
    if let Some(list) = list {
        let mut extra = 0usize;
        for root in &list.roots {
            if registry.rows.iter().any(|row| &row.root == root) {
                continue;
            }
            let mut entry = evaluate_receiptless(store, control, registry, root, false, authority);
            if entry.candidate.is_some() {
                extra += 1;
                if registry.rows.len() + extra > MAX_ROWS {
                    entry.candidate = None;
                    entry.kept = Some(
                        "a temporary pruning row would exceed the registry row limit".to_owned(),
                    );
                }
            }
            if entry.candidate.is_none() {
                let reason = entry
                    .kept
                    .as_deref()
                    .unwrap_or("receiptless authority is unknown");
                // Ordinary path ineligibility is a per-root keep, never an
                // incomplete-authority verdict for the whole batch.
                if reason != "the checkout is present, so its root is kept"
                    && reason != "an active writer still holds this checkout"
                    && !reason.starts_with("a recorded writer still holds this checkout (")
                    && !reason.starts_with("checkout absence is unknown (")
                {
                    snapshot_error.get_or_insert_with(|| reason.to_owned());
                }
            }
            evaluated.push(entry);
        }
    }
    if !authority.complete() {
        snapshot_error.get_or_insert_with(|| "the snapshot exceeded its time limit".to_owned());
    }
    // Every current receipt reference must be represented exactly once in
    // the registry; checking only groups discovered there misses omissions.
    for (checkout_id, result) in &receipts {
        let Ok(receipt) = result else { continue };
        if receipt.prepared_ref().is_some() {
            continue;
        }
        for placement in receipt.links() {
            let Some(selected) = placement.selected_ref() else {
                continue;
            };
            let matches: Vec<&Row> = registry
                .rows
                .iter()
                .filter(|row| {
                    row.checkout_id == *checkout_id
                        && row.selected
                        && row.member == placement.member()
                        && row.root == selected.root()
                })
                .collect();
            if matches.len() != 1
                || matches[0].generation != selected.generation()
                || matches[0].nonce.as_deref() != selected.nonce()
            {
                snapshot_error.get_or_insert_with(|| {
                    "a receipt reference is missing or contradictory in the registry".to_owned()
                });
            }
        }
    }
    // Verify selected receipt references for every complete group, including
    // ordinary kept roots. A representative is not full transaction authority.
    for entry in &evaluated {
        if entry.row.checkout_id.is_empty()
            || entry.candidate.as_ref().is_some_and(|c| c.receiptless)
        {
            continue;
        }
        let members = group_rows(registry, &entry.row.checkout_id, &entry.row.root);
        let mut names = std::collections::HashSet::new();
        if members.iter().any(|row| {
            row.checkout != entry.row.checkout
                || row.receipt != entry.row.receipt
                || !names.insert(&row.member)
        }) {
            snapshot_error.get_or_insert_with(|| {
                "a root group has duplicate or contradictory references".to_owned()
            });
        }
        let selected: Vec<&Row> = members.iter().copied().filter(|row| row.selected).collect();
        if let Some(first) = selected.first() {
            // The recorded device is one observation and may be stale on either
            // side of a reboot, so it is not part of the recorded identity
            // these references must agree on; the immutable inode, generation,
            // nonce, marker digest, kind and volume are.
            if selected.iter().any(|row| {
                row.nonce != first.nonce
                    || row.generation != first.generation
                    || row.root_ino != first.root_ino
                    || row.marker_digest != first.marker_digest
                    || row.root_kind != first.root_kind
                    || row.volume_uuid != first.volume_uuid
            }) {
                snapshot_error.get_or_insert_with(|| {
                    "selected group references disagree about root identity".to_owned()
                });
            }
        }
        let group: Vec<GroupRow> = members
            .iter()
            .map(|row| GroupRow {
                member: row.member.clone(),
                generation: row.generation,
                nonce: row.nonce.clone(),
                root_dev: row.root_dev,
                root_ino: row.root_ino,
                marker_digest: row.marker_digest.clone(),
                selected: row.selected,
            })
            .collect();
        if let Some(Ok(receipt)) = receipts.get(&entry.row.checkout_id) {
            if receipt.prepared_ref().is_some() {
                continue;
            }
            if let Err(reason) = verify_group_against_receipt(receipt, &entry.row.root, &group) {
                snapshot_error.get_or_insert(reason);
            }
        }
    }
    // Candidate identity also requires all of the group's recorded facts.
    for entry in &evaluated {
        if let Some(candidate) = &entry.candidate {
            if candidate.receiptless {
                continue;
            }
            let verified = verified_group(registry, candidate).and_then(|group| {
                let receipt = receipts
                    .get(&candidate.checkout_id)
                    .and_then(|result| result.as_ref().ok())
                    .ok_or_else(|| "the checkout receipt is unavailable".to_owned())?;
                verify_group_against_receipt(receipt, &candidate.root, &group)
            });
            if let Err(reason) = verified {
                snapshot_error.get_or_insert(reason);
            }
        }
    }
    if !authority.complete() {
        snapshot_error.get_or_insert_with(|| "the snapshot exceeded its time limit".to_owned());
    }
    let mut roots: std::collections::HashSet<&Path> = std::collections::HashSet::new();
    for candidate in evaluated
        .iter()
        .filter_map(|entry| entry.candidate.as_ref())
    {
        if !roots.insert(&candidate.root) {
            snapshot_error.get_or_insert_with(|| {
                "more than one live registry reference names this root".to_owned()
            });
        }
    }
    if !authority.complete() {
        snapshot_error
            .get_or_insert_with(|| "the snapshot exceeded its input or time budget".to_owned());
    }
    #[cfg(debug_assertions)]
    if prune_fixture_fault(
        control,
        "OXIDE_TEST_PRUNE_BYTES",
        control,
        "prune-bytes-observed",
    ) {
        AUTHORITY.with(|cell| {
            if let Some(budget) = cell.borrow().as_ref() {
                eprintln!(
                    "oxide proof: authority bytes {}",
                    SNAPSHOT_MAX_BYTES - budget.remaining
                );
            }
        });
    }
    if let Some(reason) = snapshot_error {
        for entry in &mut evaluated {
            entry.candidate = None;
            entry.kept = Some(format!(
                "the registry snapshot is incomplete ({reason}); the whole batch is kept"
            ));
        }
        return Ok(Evaluation {
            entries: evaluated,
            complete: false,
        });
    }
    // Diagnostics do not consume authority time or input budget.
    AUTHORITY.with(|cell| *cell.borrow_mut() = None);
    account_candidates(&mut evaluated, control);
    Ok(Evaluation {
        entries: evaluated,
        complete: true,
    })
}

#[cfg(test)]
fn evaluate_test(
    store: &Store,
    control: &Path,
    registry: &Registry,
    list: Option<&OperatorList>,
) -> Result<Evaluation, String> {
    let mut authority = AuthorityScope::begin()?;
    evaluate(store, control, registry, list, &mut authority)
}

/// The eligibility predicates for one row. `Ok(None)` means every predicate
/// holds; `Ok(Some(reason))` is an explicit keep; `Err` keeps the root and
/// refuses the batch where the snapshot itself is untrustworthy.
#[allow(clippy::too_many_arguments)]
fn eligibility(
    _store: &Store,
    control: &Path,
    registry: &Registry,
    row: &Row,
    receipt: Option<&Receipt>,
    list: Option<&OperatorList>,
    authority: &mut AuthorityScope,
    _budget: &mut u64,
    under_barrier: bool,
    evidence: &mut Option<SnapshotEvidence>,
) -> Result<Option<String>, String> {
    if !registry.complete {
        return Ok(Some(
            "the registry is sticky-incomplete, so prune keeps every root".to_owned(),
        ));
    }
    if row.pruning.is_some() {
        return Ok(Some(
            "this root already carries an unsettled prune intent".to_owned(),
        ));
    }
    if row.pending {
        return Ok(Some(
            "a prepared transition still references this root".to_owned(),
        ));
    }
    if row.reservation.is_some() || row.cancelling.is_some() {
        return Ok(Some(
            "a root reservation still references this root".to_owned(),
        ));
    }
    // One root under one checkout is ONE owner: its own sibling members are
    // the same reference group, not another live reference. Only a reference
    // from a DIFFERENT checkout keeps the root.
    if registry
        .live_rows()
        .any(|other| other.root == row.root && other.checkout_id != row.checkout_id)
    {
        return Ok(Some(
            "another checkout's live reference also names this root".to_owned(),
        ));
    }
    let Some(receipt) = receipt else {
        return Ok(Some(
            "the checkout's receipt is missing or unreadable".to_owned(),
        ));
    };
    if outputs::receipt_is_pruning(receipt) {
        return Ok(Some("the checkout's receipt is pruning".to_owned()));
    }
    // The checkout's receipt either still selects this exact root, or no
    // longer references it as current (a released root: relinked away or
    // detached, kept in history). A registry selection the receipt does not
    // make is a contradiction and keeps the root.
    match receipt.current() {
        Some(selected) if selected.root() == row.root => {
            if !receipt.selects() {
                return Ok(Some(
                    "the checkout's receipt is not committed for this root".to_owned(),
                ));
            }
            if selected.nonce() != row.nonce.as_deref()
                || selected.generation() != row.generation
                || selected.kind() != row.root_kind
            {
                return Err(
                    "the registry row does not match the checkout's committed receipt".to_owned(),
                );
            }
        }
        _ if row.selected => {
            return Err(
                "the registry selects this root but the checkout's receipt does not".to_owned(),
            )
        }
        _ => {}
    }
    if receipt.prepared_ref().is_some() {
        return Ok(Some("the checkout has a prepared transition".to_owned()));
    }
    // A row created before the forward fence carries no root identity; it is
    // never guessed at and only an explicit operator list may still consider
    // it, after every other check.
    let legacy = row.nonce.is_none()
        || row.root_dev.is_none()
        || row.root_ino.is_none()
        || row.marker_digest.is_none();
    let listed = list.is_some_and(|list| list.roots.iter().any(|root| root == &row.root));
    if legacy && !listed {
        return Ok(Some(
            "the root predates the version-2 registry identity; only an explicit operator list may prune it"
                .to_owned(),
        ));
    }
    if !row.root_kind_ok() {
        return Ok(Some(
            "the root is not an external owned output root".to_owned(),
        ));
    }
    // Checkout absence: a no-follow descriptor walk. The operator list can
    // only ever admit the genuine legacy exception (a listed row that
    // predates the version-2 identity); every other row proves its own
    // recorded checkout volume, and no row's volume requirement is waived.
    let legacy_exception = listed && legacy;
    match checkout_absence(row, legacy_exception) {
        Ok(Absence::Absent(witness)) => {
            *evidence = Some(SnapshotEvidence {
                witness: *witness,
                legacy_exception,
            });
        }
        Ok(Absence::Present) => {
            return Ok(Some(
                "the checkout is present, so its root is kept".to_owned(),
            ))
        }
        Ok(Absence::Unknown(reason)) | Err(reason) => {
            // Ordinary path ineligibility keeps THIS root and nothing else:
            // an unprovable path is not an untrustworthy snapshot. Incomplete
            // required authority (a missing, malformed or contradictory
            // receipt or registry, or unknown writer custody) still refuses
            // the whole batch where it is found.
            return Ok(Some(format!(
                "checkout absence is unknown ({reason}), so its root is kept"
            )));
        }
    }
    // Writer custody across both pools, recomputed from freshly witnessed
    // durable states under both admission locks (oxide#235). A caller holding
    // the checkout's barrier excludes only its own barrier.
    if under_barrier {
        if let Err(reason) = custody_clear_under_barrier(authority, control, &row.checkout) {
            return Ok(Some(reason));
        }
        return Ok(None);
    }
    if prune_fixture_fault(
        control,
        "OXIDE_TEST_PRUNE_UNKNOWN_CUSTODY",
        &row.checkout,
        "prune-unknown-custody-fired",
    ) {
        return Err("writer custody is unknown (injected fixture)".to_owned());
    }
    let custody = authority
        .fresh_custody(control, &row.checkout, true)
        .map_err(|reason| format!("writer custody could not be witnessed ({reason})"))?;
    match custody.verdict {
        CheckoutCustody::Clear => {}
        CheckoutCustody::Writers => {
            return Ok(Some(
                "an active writer still holds this checkout".to_owned(),
            ))
        }
        CheckoutCustody::Unknown(reason) => {
            return Err(format!("writer custody is unknown ({reason})"))
        }
    }
    for holder in custody.internal_holders {
        return Ok(Some(format!(
            "a recorded writer still holds this checkout ({holder})"
        )));
    }
    if let Some(holders) = custody.removable_holders {
        if let Some(holder) = holders.into_iter().next() {
            return Ok(Some(format!(
                "a recorded writer in the removable pool still holds this checkout ({holder})"
            )));
        }
    }
    Ok(None)
}

impl Row {
    fn root_kind_ok(&self) -> bool {
        match self.root_kind {
            RootKind::External => {
                let Some(name) = self.root.file_name() else {
                    return false;
                };
                let _ = name;
                self.root
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|parent| parent == outputs::EXTERNAL_DIR)
                    && self.volume_uuid.is_some()
            }
            // A checkout-local fallback is gone with its absent checkout; it is
            // recorded only and never deleted through a recreated checkout.
            RootKind::Local => false,
        }
    }
}

/// Positive absence of a checkout, and the ancestry the run observed to prove
/// it. A witness is ephemeral: it is never serialized into a registry,
/// receipt, plan, preview or observation document.
#[derive(Debug)]
enum Absence {
    Absent(Box<AncestryWitness>),
    Present,
    Unknown(String),
}

/// One directory entry observed without following it: the name, and the
/// identity shared by the named entry and the descriptor opened for it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ObservedEntry {
    name: std::ffi::OsString,
    dev: u64,
    ino: u64,
}

impl ObservedEntry {
    /// Whether one `fstat`/`fstatat` reading is still this observed directory
    /// entry. Only the device, inode and directory type are compared: an
    /// ancestor replaced by a different object of the same name is a change.
    fn matches(&self, stat: &libc::stat) -> bool {
        u64::try_from(stat.st_dev).unwrap_or(u64::MAX) == self.dev
            && stat.st_ino == self.ino
            && stat.st_mode & libc::S_IFMT == libc::S_IFDIR
    }
}

/// What one no-follow descriptor walk saw, and what proves it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AncestryWitness {
    /// The exact recorded checkout path this walk was made for.
    checkout: PathBuf,
    /// The walk's start descriptor: `/`, or a verified mount root.
    start_dev: u64,
    start_ino: u64,
    /// Every component below the start that the walk observed, in walk order.
    observed: Vec<ObservedEntry>,
    /// The index in `observed` of the first component that did not exist; the
    /// last observed entry is then the nearest existing ancestor.
    missing: Option<usize>,
    /// The device and volume facts read through the last observed descriptor
    /// (or the start descriptor). Every proof binds to this held identity.
    ancestor: HeldFacts,
    /// The separately held boot Data facts, when the proof used them.
    boot_data: Option<HeldFacts>,
}

/// A held descriptor's device and volume identity.
#[derive(Clone, Debug, Eq, PartialEq)]
struct HeldFacts {
    dev: u64,
    fstype: String,
    uuid: Option<String>,
}

impl HeldFacts {
    fn of(fd: BorrowedFd<'_>) -> Result<Self, String> {
        let facts = oxide_storage::volume::observe_held(fd)?;
        #[cfg(test)]
        crate::tape::classify_row(
            "held-facts",
            &format!("dev={}", facts.dev),
            &format!("fstype={}|uuid={:?}", facts.fstype, facts.uuid.as_deref()),
        );
        Ok(Self {
            dev: facts.dev,
            fstype: facts.fstype,
            uuid: facts.uuid,
        })
    }
}

/// The real run's own ephemeral evidence for one candidate: the ancestry it
/// positively proved absent and the volume rule it proved under. It is
/// compared again under the checkout's clean barrier and binding and, last,
/// under publication custody immediately before the journal. It is never
/// serialized, so a plan's bytes carry no ancestry witness and a preview's
/// absence proof is never read as authority for an effect.
#[derive(Clone, Debug, Eq, PartialEq)]
struct SnapshotEvidence {
    witness: AncestryWitness,
    /// The genuine legacy exception (boot Data ancestry) instead of the row's
    /// recorded checkout volume. Classified once, from the snapshot's row.
    legacy_exception: bool,
}

/// Where a checkout walk starts. Both `/Volumes/<mount>/...` and its boot Data
/// alias `/System/Volumes/Data/Volumes/<mount>/...` are mount spellings,
/// recognized by their exact components. An absent mount name is therefore a
/// mount refusal, never a plain missing chain.
#[derive(Clone, Debug, Eq, PartialEq)]
enum WalkStart {
    /// `/`, for every checkout outside a volume mount spelling.
    Root,
    /// A registered `/Volumes/<name>` mount root.
    Mount(PathBuf),
}

/// What one no-follow descriptor walk found.
enum Walk {
    /// The leaf itself exists.
    Present,
    /// A component did not exist; the witness names which one and holds the
    /// identity of the nearest existing ancestor.
    Missing(Box<AncestryWitness>),
    /// Anything else is uncertainty.
    Unknown(String),
}

/// The refusal for a checkout path that is not one exact absolute spelling:
/// a `.` or `..` component, a repeated or trailing separator or any other
/// non-normal form is refused, never canonicalized.
fn non_normal_spelling(checkout: &Path) -> String {
    format!(
        "{} is not one exact absolute spelling (a dot or parent component, a repeated separator or a trailing separator)",
        checkout.display()
    )
}

/// The exact components of one absolute checkout path, and where its walk
/// starts. Nothing is canonicalized: a relative path, NUL, `.`/`..`, a
/// repeated or trailing separator and every other non-normal spelling is
/// refused rather than repaired through a symlink.
fn checkout_path(checkout: &Path) -> Result<(WalkStart, Vec<std::ffi::OsString>), String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = checkout.as_os_str().as_bytes();
    if bytes.first() != Some(&b'/') {
        return Err(format!(
            "{} is not an absolute checkout path",
            checkout.display()
        ));
    }
    if bytes.contains(&0) {
        return Err("the checkout path contains a NUL".to_owned());
    }
    let mut names: Vec<std::ffi::OsString> = Vec::new();
    for component in checkout.components() {
        match component {
            std::path::Component::RootDir if names.is_empty() => {}
            std::path::Component::Normal(name) => names.push(name.to_os_string()),
            _ => return Err(non_normal_spelling(checkout)),
        }
    }
    if names.is_empty() {
        return Err("the checkout path has no components below its start".to_owned());
    }
    let mut rebuilt = PathBuf::from("/");
    for name in &names {
        rebuilt.push(name);
    }
    // The bytes, not the normalized components, decide: a repeated or trailing
    // separator or a dropped dot component is a different spelling and is
    // refused rather than quietly repaired.
    if rebuilt.as_os_str().as_bytes() != bytes {
        return Err(format!(
            "{} is not one exact absolute spelling (a repeated or trailing separator or a dot component)",
            checkout.display()
        ));
    }
    let named = |index: usize, name: &str| {
        names
            .get(index)
            .is_some_and(|component| component.as_os_str() == std::ffi::OsStr::new(name))
    };
    let (mount_index, below) = if named(0, "Volumes") {
        (1, 2)
    } else if named(0, "System") && named(1, "Volumes") && named(2, "Data") && named(3, "Volumes") {
        (4, 5)
    } else {
        return Ok((WalkStart::Root, names));
    };
    if names.len() <= below {
        return Err(format!(
            "{} names no checkout below a volume mount",
            checkout.display()
        ));
    }
    let mount = Path::new("/Volumes").join(&names[mount_index]);
    Ok((WalkStart::Mount(mount), names[below..].to_vec()))
}

/// Revalidate the WHOLE named ancestry of one walk against the descriptors the
/// walk opened: every observed descriptor must still be the recorded object and
/// every observed named entry must still name it. No parent path is ever
/// re-resolved, so no ancestor can be swapped between the walk and this check.
fn revalidate_chain(descriptors: &[OwnedFd], observed: &[ObservedEntry]) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    if descriptors.len() != observed.len() + 1 {
        return Err("the checkout ancestry chain is not one descriptor per component".to_owned());
    }
    for (index, entry) in observed.iter().enumerate() {
        #[cfg(not(test))]
        let opened = fstat_fd(&descriptors[index + 1])?;
        #[cfg(test)]
        let opened = match fstat_fd(&descriptors[index + 1]) {
            Ok(opened) => opened,
            Err(error) => {
                crate::tape::component_without_parent(
                    "absence-walk:revalidate-descriptor",
                    &entry.name,
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.clone(),
                    },
                );
                return Err(error);
            }
        };
        #[cfg(test)]
        crate::tape::component_without_parent(
            "absence-walk:revalidate-descriptor",
            &entry.name,
            crate::tape::TapeOutcome::Opened {
                dev: u64::try_from(opened.st_dev).unwrap_or(u64::MAX),
                ino: opened.st_ino,
            },
        );
        if !entry.matches(&opened) {
            #[cfg(test)]
            crate::tape::classify_row(
                "absence-walk:revalidate",
                &entry.name.to_string_lossy(),
                "descriptor-changed",
            );
            return Err("an ancestor of the checkout changed while it was read".to_owned());
        }
        let name = std::ffi::CString::new(entry.name.as_bytes())
            .map_err(|_| "the checkout path contains a NUL".to_owned())?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the held parent descriptor and C string are live for the call.
        let status = unsafe {
            libc::fstatat(
                descriptors[index].as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            #[cfg(not(test))]
            return Err(std::io::Error::last_os_error().to_string());
            #[cfg(test)]
            {
                let error = std::io::Error::last_os_error();
                crate::tape::component_without_parent(
                    "absence-walk:revalidate-name",
                    &entry.name,
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    },
                );
                return Err(error.to_string());
            }
        }
        #[cfg(test)]
        crate::tape::component_without_parent(
            "absence-walk:revalidate-name",
            &entry.name,
            crate::tape::TapeOutcome::Opened {
                dev: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
                ino: stat.st_ino,
            },
        );
        if !entry.matches(&stat) {
            #[cfg(test)]
            crate::tape::classify_row(
                "absence-walk:revalidate",
                &entry.name.to_string_lossy(),
                "entry-changed",
            );
            return Err("an ancestor of the checkout changed while it was read".to_owned());
        }
    }
    Ok(())
}

/// Walk `names` from the held `cursor` without following any component. Every
/// observed entry is opened as a real directory, and the walk keeps EVERY
/// descriptor of the chain it opened, so the whole named ancestry is
/// revalidated — named entry, then opened descriptor — at the first exact
/// `ENOENT` that decides the nearest existing ancestor. Any other error, a
/// symlink, a non-directory component or a changed entry is uncertainty.
fn walk_checkout(
    checkout: &Path,
    start: &WalkStart,
    names: &[std::ffi::OsString],
    cursor: OwnedFd,
) -> Result<Walk, String> {
    use std::os::unix::ffi::OsStrExt;
    let start_stat = fstat_fd(&cursor)?;
    let start_dev = u64::try_from(start_stat.st_dev).unwrap_or(u64::MAX);
    let start_ino = start_stat.st_ino;
    let mut walked = match start {
        WalkStart::Root => PathBuf::from("/"),
        WalkStart::Mount(mount) => mount.clone(),
    };
    let leaf = names.len() - 1;
    let mut observed: Vec<ObservedEntry> = Vec::new();
    // The pinned chain: `descriptors[0]` is the start and `descriptors[i + 1]`
    // is the descriptor `observed[i]` was opened as. Every one stays open for
    // the whole walk.
    let mut descriptors: Vec<OwnedFd> = vec![cursor];
    #[cfg(test)]
    crate::tape::walk_start_row("absence-walk:start", &walked, start_dev, start_ino);
    for (index, name) in names.iter().enumerate() {
        let entry = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "the checkout path contains a NUL".to_owned())?;
        let parent = descriptors[index].as_raw_fd();
        // The debug-only fixture: the walk "cannot read" this directory.
        let injected = injected_eacces(&walked);
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the held parent descriptor and C string are live for the call.
        let status = if injected {
            -1
        } else {
            unsafe { libc::fstatat(parent, entry.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) }
        };
        if status != 0 {
            let error = if injected {
                std::io::Error::from_raw_os_error(libc::EACCES)
            } else {
                std::io::Error::last_os_error()
            };
            #[cfg(test)]
            {
                if let Some((pdev, pino)) = Some(if index == 0 {
                    (start_dev, start_ino)
                } else {
                    (observed[index - 1].dev, observed[index - 1].ino)
                }) {
                    crate::tape::component_row(
                        "absence-walk:fstatat",
                        pdev,
                        pino,
                        name,
                        if error.kind() == std::io::ErrorKind::NotFound {
                            crate::tape::TapeOutcome::Absent
                        } else {
                            crate::tape::TapeOutcome::Refusal {
                                reason: error.to_string(),
                            }
                        },
                    );
                }
            }
            if error.kind() != std::io::ErrorKind::NotFound {
                return Ok(Walk::Unknown(error.to_string()));
            }
            // ENOENT is decided from the HELD nearest-ancestor descriptor, and
            // only after the whole named ancestry has been revalidated against
            // the descriptors this walk still holds.
            revalidate_chain(&descriptors, &observed)?;
            let ancestor = HeldFacts::of(descriptors[index].as_fd())?;
            return Ok(Walk::Missing(Box::new(AncestryWitness {
                checkout: checkout.to_path_buf(),
                start_dev,
                start_ino,
                observed,
                missing: Some(index),
                ancestor,
                boot_data: None,
            })));
        }
        if index == leaf {
            #[cfg(test)]
            {
                if let Some((pdev, pino)) = Some(if index == 0 {
                    (start_dev, start_ino)
                } else {
                    (observed[index - 1].dev, observed[index - 1].ino)
                }) {
                    crate::tape::component_row(
                        "absence-walk:fstatat",
                        pdev,
                        pino,
                        name,
                        crate::tape::TapeOutcome::Opened {
                            dev: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
                            ino: stat.st_ino,
                        },
                    );
                }
            }
            return Ok(Walk::Present);
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Ok(Walk::Unknown(
                "an ancestor of the checkout is not a directory".to_owned(),
            ));
        }
        // SAFETY: the held parent descriptor and C string are live for the call.
        let next = unsafe {
            libc::openat(
                parent,
                entry.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next < 0 {
            #[cfg(not(test))]
            return Ok(Walk::Unknown(std::io::Error::last_os_error().to_string()));
            #[cfg(test)]
            {
                let error = std::io::Error::last_os_error();
                let (pdev, pino) = if index == 0 {
                    (start_dev, start_ino)
                } else {
                    (observed[index - 1].dev, observed[index - 1].ino)
                };
                crate::tape::component_row(
                    "absence-walk:openat",
                    pdev,
                    pino,
                    name.as_os_str(),
                    crate::tape::TapeOutcome::Refusal {
                        reason: error.to_string(),
                    },
                );
                return Ok(Walk::Unknown(error.to_string()));
            }
        }
        // SAFETY: `next` is a newly owned descriptor.
        let opened = unsafe { OwnedFd::from_raw_fd(next) };
        let opened_stat = fstat_fd(&opened)?;
        #[cfg(test)]
        {
            if let Some((pdev, pino)) = Some(if index == 0 {
                (start_dev, start_ino)
            } else {
                (observed[index - 1].dev, observed[index - 1].ino)
            }) {
                crate::tape::component_row(
                    "absence-walk:openat",
                    pdev,
                    pino,
                    name.as_os_str(),
                    crate::tape::TapeOutcome::Opened {
                        dev: u64::try_from(opened_stat.st_dev).unwrap_or(u64::MAX),
                        ino: opened_stat.st_ino,
                    },
                );
            }
        }
        if opened_stat.st_dev != stat.st_dev || opened_stat.st_ino != stat.st_ino {
            return Ok(Walk::Unknown(
                "the directory the walk opened is not the entry it named".to_owned(),
            ));
        }
        // The named entry must still be the descriptor that was opened.
        let mut again: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the held parent descriptor and C string are live for the call.
        let revalidated = unsafe {
            libc::fstatat(
                parent,
                entry.as_ptr(),
                &mut again,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if revalidated != 0 {
            return Ok(Walk::Unknown(std::io::Error::last_os_error().to_string()));
        }
        let observed_entry = ObservedEntry {
            name: name.clone(),
            dev: u64::try_from(opened_stat.st_dev).unwrap_or(u64::MAX),
            ino: opened_stat.st_ino,
        };
        if !observed_entry.matches(&again) {
            return Ok(Walk::Unknown(
                "an ancestor of the checkout changed while it was read".to_owned(),
            ));
        }
        observed.push(observed_entry);
        descriptors.push(opened);
        walked.push(name);
    }
    Ok(Walk::Unknown("the checkout walk ended early".to_owned()))
}

/// The recorded-volume rule for a registered checkout: the held nearest
/// existing ancestor is on a local APFS or HFS volume with the checkout's
/// recorded UUID. A conflicting or unknown recorded volume is never waived
/// because today's ancestor happens to sit on the boot Data volume.
fn judge_recorded_volume(ancestor: &HeldFacts, uuid: &str) -> Result<(), String> {
    if !matches!(ancestor.fstype.as_str(), "apfs" | "hfs") {
        #[cfg(test)]
        crate::tape::classify_row("absence-judge:recorded-volume", uuid, "refused");
        return Err(format!(
            "the checkout's nearest existing ancestor is on a {} file system, not a verified local volume",
            ancestor.fstype
        ));
    }
    if ancestor.uuid.as_deref() != Some(uuid) {
        #[cfg(test)]
        crate::tape::classify_row("absence-judge:recorded-volume", uuid, "refused");
        return Err(
            "the checkout's nearest existing ancestor is not on the checkout's recorded volume"
                .to_owned(),
        );
    }
    #[cfg(test)]
    crate::tape::classify_row("absence-judge:recorded-volume", uuid, "accepted");
    Ok(())
}

/// The boot Data rule, judged only from held descriptors: the nearest existing
/// ancestor is a readable directory on the same device and APFS volume UUID as
/// `/System/Volumes/Data`.
fn judge_boot_data(ancestor: &HeldFacts, data: &HeldFacts) -> Result<(), String> {
    if data.fstype != "apfs" || data.uuid.is_none() {
        #[cfg(test)]
        crate::tape::classify_row("absence-judge:boot-data", "boot-data", "refused");
        return Err("the boot Data volume is not a verified APFS volume".to_owned());
    }
    if ancestor.fstype != "apfs" || ancestor.dev != data.dev || ancestor.uuid != data.uuid {
        #[cfg(test)]
        crate::tape::classify_row("absence-judge:boot-data", "boot-data", "refused");
        return Err(
            "the checkout's nearest existing ancestor is not on the boot Data volume (device and APFS volume UUID differ)"
                .to_owned(),
        );
    }
    #[cfg(test)]
    crate::tape::classify_row("absence-judge:boot-data", "boot-data", "accepted");
    Ok(())
}

/// The held boot Data identity the missing-chain and legacy rules bind to.
fn boot_data_facts() -> Result<HeldFacts, String> {
    let text = std::ffi::CString::new("/System/Volumes/Data")
        .map_err(|_| "the boot Data path contains a NUL".to_owned())?;
    // SAFETY: a NUL-terminated absolute path; the last component is not
    // followed and the result is a newly owned descriptor.
    let raw = unsafe {
        libc::openat(
            libc::AT_FDCWD,
            text.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        #[cfg(test)]
        crate::tape::read_row(
            "absence-walk:boot-data",
            Path::new("/System/Volumes/Data"),
            crate::tape::TapeOutcome::Refusal {
                reason: std::io::Error::last_os_error().to_string(),
            },
            0,
        );
        return Err(format!(
            "the boot Data volume is unverifiable ({})",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `raw` is a newly owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    #[cfg(test)]
    {
        if let Some((dev, ino)) = crate::tape::fd_dev_ino(fd.as_fd()) {
            crate::tape::read_row_dev(
                "absence-walk:boot-data",
                Path::new("/System/Volumes/Data"),
                dev,
                ino,
                crate::tape::TapeOutcome::Opened { dev, ino },
                0,
            );
        }
    }
    HeldFacts::of(fd.as_fd())
}

/// Positive absence of a checkout.
///
/// Every observed component opens as a real directory without following a
/// link and is revalidated against the descriptor the walk holds; the walk's
/// first exact `ENOENT` is the only positive answer. The actual leaf may be
/// absent under a fully present ancestry, and a missing ancestor chain is
/// provable only when the held nearest existing ancestor is a readable
/// directory on the boot Data volume. A mount spelling never takes the
/// missing-chain rule, every other answer is uncertainty, and every refusal
/// keeps the root.
///
/// A RECORDED checkout volume is never waived: whatever the row's age, a row
/// (or receiptless marker) that records one must find its held ancestor on that
/// volume. Only a row with no recorded volume at all may use the genuine legacy
/// boot Data rule, and a missing chain needs the boot Data proof in every case.
fn checkout_absence(row: &Row, legacy_exception: bool) -> Result<Absence, String> {
    let checkout = &row.checkout;
    let (start, names) = match checkout_path(checkout) {
        Ok(found) => found,
        Err(reason) => return Ok(Absence::Unknown(reason)),
    };
    let (cursor, binding) = match &start {
        WalkStart::Root => match open_root() {
            Ok(fd) => (fd, None),
            Err(reason) => return Ok(Absence::Unknown(reason)),
        },
        WalkStart::Mount(mount) => {
            // A checkout below a volume mount keeps the existing leaf-only
            // rule: the recorded volume must be mounted and match, and the
            // walk continues from its verified mount root.
            let Some(uuid) = row.checkout_volume_uuid.as_deref() else {
                return Ok(Absence::Unknown(
                    "a /Volumes checkout has no recorded volume UUID".to_owned(),
                ));
            };
            let binding = match Binding::open(mount, Some(uuid)) {
                Ok(binding) => {
                    #[cfg(test)]
                    crate::tape::volume_row(
                        "absence-walk:binding-open",
                        binding.mount_point(),
                        Some(binding.uuid()),
                        Some(binding.device()),
                        crate::tape::TapeOutcome::Facts {
                            facts: format!(
                                "mount={}|uuid={}",
                                binding.mount_point().display(),
                                binding.uuid()
                            ),
                        },
                    );
                    binding
                }
                Err(error) => {
                    #[cfg(test)]
                    crate::tape::volume_row(
                        "absence-walk:binding-open",
                        mount,
                        Some(uuid),
                        None,
                        crate::tape::TapeOutcome::Refusal {
                            reason: error.chars().take(240).collect::<String>(),
                        },
                    );
                    return Ok(Absence::Unknown(format!(
                        "the recorded volume is not mounted and matching ({error})"
                    )));
                }
            };
            // SAFETY: `dup` returns a new owned descriptor on success.
            let opened = unsafe { libc::dup(binding.fd().as_raw_fd()) };
            if opened < 0 {
                return Ok(Absence::Unknown(
                    "cannot duplicate the mount descriptor".to_owned(),
                ));
            }
            // SAFETY: `opened` is a newly owned descriptor.
            (unsafe { OwnedFd::from_raw_fd(opened) }, Some(binding))
        }
    };
    let mut witness = match walk_checkout(checkout, &start, &names, cursor) {
        Ok(Walk::Present) => return Ok(Absence::Present),
        Ok(Walk::Unknown(reason)) => return Ok(Absence::Unknown(reason)),
        Ok(Walk::Missing(witness)) => witness,
        Err(reason) => return Ok(Absence::Unknown(reason)),
    };
    // The verified mount must still be the same mounted volume after the walk.
    if let Some(binding) = &binding {
        if let Err(reason) = binding.verify() {
            #[cfg(test)]
            crate::tape::volume_row(
                "absence-walk:binding-verify",
                binding.mount_point(),
                Some(binding.uuid()),
                Some(binding.device()),
                crate::tape::TapeOutcome::Refusal {
                    reason: reason.chars().take(240).collect::<String>(),
                },
            );
            return Ok(Absence::Unknown(format!(
                "the recorded volume changed during the walk ({reason})"
            )));
        }
        #[cfg(test)]
        crate::tape::volume_row(
            "absence-walk:binding-verify",
            binding.mount_point(),
            Some(binding.uuid()),
            Some(binding.device()),
            crate::tape::TapeOutcome::Facts {
                facts: "unchanged".to_owned(),
            },
        );
    }
    let Some(missing) = witness.missing else {
        return Ok(Absence::Unknown("the checkout walk ended early".to_owned()));
    };
    let leaf = names.len() - 1;
    let mount_start = matches!(start, WalkStart::Mount(_));
    if mount_start && missing < leaf {
        return Ok(Absence::Unknown(
            "an ancestor of the checkout below its mount is missing, so only the actual leaf may be absent"
                .to_owned(),
        ));
    }
    if !mount_start {
        let recorded = row.checkout_volume_uuid.as_deref();
        if recorded.is_none() && !legacy_exception {
            return Ok(Absence::Unknown(
                "the checkout has no recorded volume UUID".to_owned(),
            ));
        }
        // A recorded volume is required whatever the row's age. The operator
        // list is a distinct permission and never replaces it, so a conflicting
        // recorded volume keeps the root even when a surviving parent sits on
        // the boot Data volume.
        if let Some(uuid) = recorded {
            if let Err(reason) = judge_recorded_volume(&witness.ancestor, uuid) {
                return Ok(Absence::Unknown(reason));
            }
        }
        if legacy_exception {
            // The genuine legacy exception keeps its already-authorized boot
            // Data ancestry rule. It is an additional requirement, never a
            // replacement for a recorded volume above.
            let data = match boot_data_facts() {
                Ok(facts) => facts,
                Err(reason) => return Ok(Absence::Unknown(reason)),
            };
            if let Err(reason) = judge_boot_data(&witness.ancestor, &data) {
                return Ok(Absence::Unknown(reason));
            }
            witness.boot_data = Some(data);
        }
    }
    if missing < leaf {
        // A missing parent chain never waives a mount boundary or a non-Data
        // volume: the held nearest existing ancestor must itself be on the
        // boot Data volume, proved from held descriptors only.
        let data = match boot_data_facts() {
            Ok(facts) => facts,
            Err(reason) => return Ok(Absence::Unknown(reason)),
        };
        if let Err(reason) = judge_boot_data(&witness.ancestor, &data) {
            return Ok(Absence::Unknown(reason));
        }
        witness.boot_data = Some(data);
    }
    Ok(Absence::Absent(witness))
}

/// Re-prove one candidate's absence and compare it with the real run's own
/// ancestry witness. It runs under the checkout's clean barrier and binding,
/// and again under publication custody immediately before the journal. Only
/// the call made under publication custody DECIDES: it is a fresh descriptor
/// walk, so a checkout recreated, an ancestor replaced, a changed start
/// descriptor or a different first-missing component is refused before any
/// intent, receipt or quarantine effect even when the change lands while this
/// root waits for that custody.
fn confirm_absence(candidate: &Candidate, evidence: &SnapshotEvidence) -> Result<(), String> {
    let row = candidate_row(candidate);
    match checkout_absence(&row, evidence.legacy_exception)? {
        Absence::Absent(current) if *current == evidence.witness => Ok(()),
        Absence::Absent(_) => Err(
            "the checkout's ancestry changed after the real run's snapshot, so its root is kept"
                .to_owned(),
        ),
        Absence::Present => Err("the checkout reappeared before the prune".to_owned()),
        Absence::Unknown(reason) => Err(reason),
    }
}
/// Debug builds only: inject EACCES when the checkout walk reads inside the
/// directory named by `OXIDE_TEST_PRUNE_EACCES`, so a proof drives an
/// unreadable ancestor without a real unreadable directory. Bound like the
/// walk-mutation fixture: only under the shared proof supervisor (a
/// deadline, canonical scratch below /private/tmp, evidence nested in or a
/// sibling `evidence/` of it) and only for a canonical path inside that
/// scratch. Each firing appends the path to `prune-eacces-fired` in the
/// evidence directory, so a proof can show the injection fired.
#[cfg(debug_assertions)]
fn injected_eacces(directory: &Path) -> bool {
    let Ok(target) = std::env::var("OXIDE_TEST_PRUNE_EACCES") else {
        return false;
    };
    let armed = (|| -> Option<PathBuf> {
        let evidence = proof_evidence()?;
        let scratch = fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")?).ok()?;
        let target = PathBuf::from(&target);
        let canonical = fs::canonicalize(&target).ok()?;
        if canonical != target || canonical == scratch || !canonical.starts_with(&scratch) {
            return None;
        }
        Some(evidence)
    })();
    let Some(evidence) = armed else {
        return false;
    };
    if directory != Path::new(&target) {
        return false;
    }
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(evidence.join("prune-eacces-fired"))
        .and_then(|mut file| writeln!(file, "{target}"));
    true
}

#[cfg(not(debug_assertions))]
fn injected_eacces(_directory: &Path) -> bool {
    false
}

/// The evidence directory of an armed debug proof fixture, under the shared
/// proof supervisor (a deadline, canonical owned scratch, evidence nested in
/// it or its `evidence/` sibling). The caller decides placement.
#[cfg(debug_assertions)]
fn supervisor_evidence() -> Option<PathBuf> {
    oxide_storage::proof::evidence()
}

/// The one placement rule of the shared debug fixture gate, `proof_evidence`.
/// Every `OXIDE_TEST_PRUNE_*` hook routed through it arms on the internal volume
/// (scratch below /private/tmp), or under a declared scratch pool that is a
/// canonical strict child of the supervisor scratch. `given` is the declaration
/// as the proof spelled it and `canonical` its resolved form, so a symlinked or
/// non-canonical declaration refuses.
#[cfg(debug_assertions)]
fn declaration_allowed(scratch: &Path, given: Option<&Path>, canonical: Option<&Path>) -> bool {
    if scratch.starts_with("/private/tmp") {
        return true;
    }
    match (given, canonical) {
        (Some(given), Some(canonical)) => {
            given == canonical && canonical != scratch && canonical.starts_with(scratch)
        }
        _ => false,
    }
}

/// The evidence directory of an armed debug proof fixture, through the one
/// shared gate: supervised evidence plus the declaration rule above. Hooks that
/// need a control directory apply their own control checks on top
/// (`hook_control_allowed`). Used by the EACCES, low-reserve and control hooks.
#[cfg(debug_assertions)]
fn proof_evidence() -> Option<PathBuf> {
    let evidence = supervisor_evidence()?;
    let scratch = fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")?).ok()?;
    let given = std::env::var_os("OXIDE_SCRATCH_ROOT").map(PathBuf::from);
    let canonical = given
        .as_deref()
        .and_then(|path| fs::canonicalize(path).ok());
    declaration_allowed(&scratch, given.as_deref(), canonical.as_deref()).then_some(evidence)
}

/// Where an armed prune fixture may place its control directory (oxide#211, S4,
/// decision 7aedd5af). Internal: the supervisor scratch is below /private/tmp.
/// Selected volume: the control directory must sit under the proof's DECLARED
/// scratch pool root (`OXIDE_SCRATCH_ROOT`), which is a strict child of the
/// supervisor scratch. The declaration itself was verified by the product at
/// admission; this only names where a test hook may fire.
#[cfg(debug_assertions)]
fn hook_placement_allowed(scratch: &Path, declared: Option<&Path>, control: &Path) -> bool {
    if scratch.starts_with("/private/tmp") {
        return true;
    }
    match declared {
        Some(declared) => {
            declared != scratch
                && declared.starts_with(scratch)
                && control != declared
                && control.starts_with(declared)
        }
        None => false,
    }
}

/// The control-directory conditions of [`prune_fixture_fault`] as pure path
/// facts: `given` is the path the proof named, `canonical` its resolved form.
/// A symlinked or non-canonical control, the supervisor scratch root itself and
/// any control outside the armed placement all refuse.
#[cfg(debug_assertions)]
fn hook_control_allowed(
    given: &Path,
    canonical: &Path,
    scratch: &Path,
    declared: Option<&Path>,
) -> bool {
    canonical == given
        && canonical != scratch
        && canonical.starts_with(scratch)
        && hook_placement_allowed(scratch, declared, canonical)
}

/// Debug builds only, under [`proof_evidence`]: a low-reserve reading for
/// one prune worker check, in exactly one of two modes (both set disarms
/// both; malformed or zero values disarm):
/// - `OXIDE_TEST_PRUNE_LOW_RESERVE_AT=<n>`: the n-th check of this process
///   (1 = the check before the rename);
/// - `OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_ENTRIES=<n>`: the first check made
///   once at least n quarantined entries were actually deleted.
/// It fires once, records itself in the evidence directory and changes no
/// production reserve policy.
/// Decide whether one check reads a low reserve. One mode at a time; an absent,
/// malformed or combined set disarms every mode. The modes are the n-th check,
/// the first check after n deletions of any kind, and the first check after n
/// **recovered Finder metadata** entries — the last keys on the bounded seam's
/// own work, never on the pass's total.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
fn low_reserve_decision(
    check: u64,
    deleted: u64,
    recovered: u64,
    at: Option<u64>,
    after_entries: Option<u64>,
    after_recovered: Option<u64>,
) -> Option<&'static str> {
    match (at, after_entries, after_recovered) {
        (Some(at), None, None) if check == at => Some("check"),
        (None, Some(after), None) if deleted >= after => Some("entries"),
        (None, None, Some(after)) if recovered >= after => Some("recovered"),
        _ => None,
    }
}

#[cfg(debug_assertions)]
fn injected_low_reserve(deleted: u64, recovered: u64) -> bool {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static CHECKS: AtomicU64 = AtomicU64::new(0);
    static FIRED: AtomicBool = AtomicBool::new(false);
    let positive = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|value| value.parse::<u64>().ok().filter(|n| *n > 0))
    };
    let count = CHECKS.fetch_add(1, Ordering::Relaxed) + 1;
    let Some(mode) = low_reserve_decision(
        count,
        deleted,
        recovered,
        positive("OXIDE_TEST_PRUNE_LOW_RESERVE_AT").flatten(),
        positive("OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_ENTRIES").flatten(),
        positive("OXIDE_TEST_PRUNE_LOW_RESERVE_AFTER_RECOVERED").flatten(),
    ) else {
        return false;
    };
    if FIRED.load(Ordering::Relaxed) {
        return false;
    }
    let Some(evidence) = proof_evidence() else {
        return false;
    };
    FIRED.store(true, Ordering::Relaxed);
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(evidence.join("prune-low-reserve-fired"))
        .and_then(|mut file| {
            writeln!(
                file,
                "{mode}: check {count}, {deleted} entries deleted, {recovered} recovered"
            )
        });
    true
}

#[cfg(not(debug_assertions))]
fn injected_low_reserve(_deleted: u64, _recovered: u64) -> bool {
    false
}

fn open_root() -> Result<OwnedFd, String> {
    let fd = unsafe {
        libc::open(
            b"/\0".as_ptr().cast(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "cannot open /: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

struct RootFacts {
    dev: u64,
    ino: u64,
    marker_digest: String,
    legacy: bool,
}

enum RootState {
    Present(RootFacts),
    Absent,
    Unknown(String),
}

/// Verify the registered volume, the outputs directory, the root entry and
/// the marker entirely through descriptors.
fn observe_row_root(row: &Row) -> Result<RootState, String> {
    if row.root_kind == RootKind::Local {
        return Ok(RootState::Unknown(
            "a checkout-local fallback root is gone with its absent checkout and is recorded only"
                .to_owned(),
        ));
    }
    let Some(uuid) = row.volume_uuid.as_deref() else {
        return Ok(RootState::Unknown(
            "the root has no recorded volume UUID".to_owned(),
        ));
    };
    let Some(outputs_dir) = row.root.parent() else {
        return Ok(RootState::Unknown("the root has no parent".to_owned()));
    };
    let Some(mount) = outputs_dir.parent() else {
        return Ok(RootState::Unknown(
            "the outputs directory has no mount".to_owned(),
        ));
    };
    if outputs_dir.file_name() != Some(std::ffi::OsStr::new(outputs::EXTERNAL_DIR)) {
        return Ok(RootState::Unknown(
            "the root is not inside an oxide-outputs directory".to_owned(),
        ));
    }
    let binding = match Binding::open(mount, Some(uuid)) {
        Ok(binding) => {
            #[cfg(test)]
            crate::tape::volume_row(
                "root-observe:binding-open",
                binding.mount_point(),
                Some(binding.uuid()),
                Some(binding.device()),
                crate::tape::TapeOutcome::Facts {
                    facts: format!(
                        "mount={}|uuid={}",
                        binding.mount_point().display(),
                        binding.uuid()
                    ),
                },
            );
            binding
        }
        Err(error) => {
            #[cfg(test)]
            crate::tape::volume_row(
                "root-observe:binding-open",
                mount,
                Some(uuid),
                None,
                crate::tape::TapeOutcome::Refusal {
                    reason: error.chars().take(240).collect::<String>(),
                },
            );
            return Ok(RootState::Unknown(format!(
                "the recorded volume is not usable ({error})"
            )));
        }
    };
    observe_root_in(&binding, row)
}

/// The descriptor half of [`observe_row_root`], over an already verified mount
/// binding.
///
/// The recorded device number is an observation, not an identity: macOS gives a
/// mounted volume a new `st_dev` after a reboot or a reattach, so a registry row
/// written before one carries a stale number for an unchanged root. The root's
/// identity is the verified volume UUID (checked by the binding and again on the
/// owner marker here), the inode, the generation and the nonce, and the recorded
/// marker digest; those are compared exactly. A stale device is therefore
/// replaced by the number the live descriptor reports rather than making the row
/// unknown, and every other difference — a renumbered inode, another volume, a
/// changed marker or digest — still keeps the root.
fn observe_root_in(binding: &Binding, row: &Row) -> Result<RootState, String> {
    let outer = binding
        .dir_at(binding.fd(), outputs::EXTERNAL_DIR, false)
        .map_err(|error| format!("cannot open the outputs directory: {error}"))?;
    #[cfg(test)]
    {
        let outputs_path = binding.mount_point().join(outputs::EXTERNAL_DIR);
        if let Some((dev, ino)) = crate::tape::fd_dev_ino(outer.as_fd()) {
            crate::tape::read_row_dev(
                "root-observe:outputs-dir",
                &outputs_path,
                dev,
                ino,
                crate::tape::TapeOutcome::Opened { dev, ino },
                0,
            );
        }
    }
    let Some(name) = row.root.file_name() else {
        return Ok(RootState::Unknown("the root has no leaf name".to_owned()));
    };
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| "the root name contains a NUL".to_owned())?;
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor and C string are live for the call.
    let status = unsafe {
        libc::fstatat(
            outer.as_raw_fd(),
            name_c.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        #[cfg(test)]
        crate::tape::component_without_parent(
            "root-observe:entry",
            name,
            if error.kind() == std::io::ErrorKind::NotFound {
                crate::tape::TapeOutcome::Absent
            } else {
                crate::tape::TapeOutcome::Refusal {
                    reason: error.to_string(),
                }
            },
        );
        return Ok(if error.kind() == std::io::ErrorKind::NotFound {
            RootState::Absent
        } else {
            RootState::Unknown(error.to_string())
        });
    }
    #[cfg(test)]
    crate::tape::component_without_parent(
        "root-observe:entry",
        name,
        crate::tape::TapeOutcome::Opened {
            dev: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
            ino: stat.st_ino,
        },
    );
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Ok(RootState::Unknown(
            "the recorded root is not a real directory".to_owned(),
        ));
    }
    let dev = u64::try_from(stat.st_dev).unwrap_or(u64::MAX);
    let ino = stat.st_ino;
    if row.root_ino != Some(ino) {
        return Ok(RootState::Unknown(
            "the recorded root's inode changed".to_owned(),
        ));
    }
    // A recorded device that differs is only a renumbered volume: every fact
    // that identifies the root is still compared below.
    let root = binding
        .dir_at(outer.as_fd(), &name.to_string_lossy(), false)
        .map_err(|error| format!("cannot open the recorded root: {error}"))?;
    #[cfg(test)]
    {
        if let Some((dev, ino)) = crate::tape::fd_dev_ino(root.as_fd()) {
            crate::tape::read_row_dev(
                "root-observe:root-dir",
                &row.root,
                dev,
                ino,
                crate::tape::TapeOutcome::Opened { dev, ino },
                0,
            );
        }
    }
    let marker_name = std::ffi::CString::new(outputs::MARKER).map_err(|error| error.to_string())?;
    // SAFETY: the root descriptor and C string are live for the call.
    let marker_fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            marker_name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if marker_fd < 0 {
        #[cfg(test)]
        crate::tape::read_row(
            "root-observe:marker-open",
            &row.root.join(outputs::MARKER),
            crate::tape::TapeOutcome::Refusal {
                reason: std::io::Error::last_os_error().to_string(),
            },
            0,
        );
        return Ok(RootState::Unknown(format!(
            "the root has no readable owner marker ({})",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `marker_fd` is a newly owned descriptor.
    let marker_file = unsafe { File::from_raw_fd(marker_fd) };
    let mut bytes = Vec::new();
    #[cfg(not(test))]
    authority_read(marker_file.take(64 * 1024 + 1), &mut bytes)
        .map_err(|error| format!("cannot read the owner marker: {error}"))?;
    #[cfg(test)]
    authority_read_observed(
        &marker_file,
        &row.root.join(outputs::MARKER),
        "prune::observe_root_in:marker",
        64 * 1024,
        &mut bytes,
    )
    .map_err(|error| format!("cannot read the owner marker: {error}"))?;
    if bytes.len() > 64 * 1024 {
        return Ok(RootState::Unknown(
            "the owner marker exceeds its byte limit".to_owned(),
        ));
    }
    let marker: Marker = match serde_json::from_slice(&bytes) {
        Ok(marker) => marker,
        Err(error) => {
            return Ok(RootState::Unknown(format!(
                "the owner marker is invalid ({error})"
            )))
        }
    };
    let observed = ObservedIdentity {
        dev,
        ino,
        legacy: marker.is_legacy(),
        marker,
        marker_digest: sha256_hex(&bytes),
    };
    if !observed.marker.is_owned_by(&row.checkout, &row.checkout_id) {
        return Ok(RootState::Unknown(
            "the owner marker names another checkout".to_owned(),
        ));
    }
    match judge_recorded_root(row, &observed) {
        Ok(dev) => {
            #[cfg(test)]
            crate::tape::classify_row(
                "root-judge:recorded-root",
                &row.root.display().to_string(),
                &format!("ok dev={dev} ino={ino}"),
            );
            Ok(RootState::Present(RootFacts {
                dev,
                ino,
                marker_digest: observed.marker_digest,
                legacy: observed.legacy,
            }))
        }
        Err(reason) => {
            #[cfg(test)]
            crate::tape::classify_row(
                "root-judge:recorded-root",
                &row.root.display().to_string(),
                &format!("kept: {reason}"),
            );
            Ok(RootState::Unknown(reason))
        }
    }
}

/// One live observation of a recorded root's descriptor and owner marker.
struct ObservedIdentity {
    dev: u64,
    ino: u64,
    marker: Marker,
    marker_digest: String,
    legacy: bool,
}

/// Whether a recorded generation that is NEWER than the owner marker's own
/// creation generation is the documented pre-oxide#152 superseded selection
/// generation (oxide#218). The attribution is never the number itself: it needs
/// `digest_pinned`, the positive fact that the recorded marker digest is the
/// LIVE marker's exact bytes. Then the document — including its own nonce,
/// volume and creation generation — is the recorded one, so the row's
/// generation can only be the receipt's commit generation that the old writer
/// copied, never a claim about a re-created root. A recorded generation older
/// than the marker's is never reconciled: nothing writes one.
///
/// Shared by the observation and the pre-effect marker verification so those
/// two rules can never drift.
fn superseded_selection_generation(
    recorded: u64,
    marker_generation: u64,
    digest_pinned: bool,
) -> bool {
    digest_pinned && recorded > marker_generation
}

/// Compare one live observation with the recorded row, returning the device this
/// run should bind.
///
/// The recorded device number is one observation, not an identity: macOS gives a
/// mounted volume a fresh `st_dev` after a reboot or a reattach, so a row written
/// before one carries a stale number for a root that never changed. Everything
/// that identifies the root is still compared exactly — the inode, the kind, the
/// generation, the nonce, the marker's own volume UUID and the recorded marker
/// digest — so a stale number is replaced by the live one while a reused inode on
/// another volume, a re-created root or an edited marker is never reconciled.
///
/// The recorded generation has one documented superseded shape (oxide#218).
/// Before oxide#152 a transition recorded the receipt's COMMITTED selection
/// generation in the row, which advances with every transition, while the owner
/// marker keeps the generation the root was created at (it is rewritten only by
/// the version-1 to version-2 upgrade, which preserves it). Such a row is
/// reconciled only when the recorded marker digest pins the EXACT live marker
/// bytes: the document — including its own nonce, volume and creation
/// generation — is then the recorded one, so the field is a superseded
/// selection observation, never a claim about a re-created root. A recorded
/// generation OLDER than the marker's is never reconciled (nothing writes one),
/// and a row without a recorded digest has no pin at all, so it must agree
/// exactly.
fn judge_recorded_root(row: &Row, observed: &ObservedIdentity) -> Result<u64, String> {
    if row.root_ino != Some(observed.ino) {
        return Err("the recorded root's inode changed".to_owned());
    }
    // A row with no recorded device predates the version-2 registry identity and
    // is only ever considered through an explicit operator list; this rule
    // reconciles a recorded number, it never invents one.
    if row.root_dev.is_none() {
        return Err("the recorded root has no device".to_owned());
    }
    let superseded = superseded_selection_generation(
        row.generation,
        observed.marker.generation(),
        row.marker_digest
            .as_deref()
            .is_some_and(|recorded| recorded == observed.marker_digest),
    );
    if observed.marker.kind() != row.root_kind
        || observed.marker.volume_uuid() != row.volume_uuid.as_deref()
        || observed.marker.nonce() != row.nonce.as_deref()
        || (observed.marker.generation() != row.generation && !superseded)
    {
        return Err("the owner marker does not match the recorded root identity".to_owned());
    }
    if row
        .marker_digest
        .as_deref()
        .is_some_and(|recorded| recorded != observed.marker_digest)
    {
        return Err("the owner marker changed since it was recorded".to_owned());
    }
    Ok(observed.dev)
}

fn account_candidates(evaluated: &mut [Evaluated], control: &Path) {
    #[cfg(not(test))]
    let deadline = Instant::now() + Duration::from_millis(OBSERVATION_MS);
    #[cfg(test)]
    let deadline = {
        let started = Instant::now();
        crate::tape::monotonic_observed("accounting:begin:window-offset-ns", started);
        started + Duration::from_millis(OBSERVATION_MS)
    };
    let mut remaining = OBSERVATION_ENTRIES;
    for candidate in evaluated
        .iter_mut()
        .filter_map(|entry| entry.candidate.as_mut())
    {
        let now = Instant::now();
        #[cfg(test)]
        crate::tape::monotonic_observed("accounting:before-delay:window-offset-ns", now);
        if remaining == 0 || now >= deadline {
            break;
        }
        diagnostic_delay(control, &candidate.root);
        let now = Instant::now();
        #[cfg(test)]
        crate::tape::monotonic_observed("accounting:after-delay:window-offset-ns", now);
        if now >= deadline {
            break;
        }
        let limits = InventoryLimits {
            max_entries: remaining,
            max_depth: 64,
            max_output_bytes: 4096,
            max_elapsed_ms: deadline
                .duration_since(now)
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        };
        #[cfg(not(test))]
        let Ok(report) = inventory::inventory_directory(&candidate.root, limits) else {
            #[cfg(test)]
            crate::tape::read_row(
                "accounting-walk",
                &candidate.root,
                crate::tape::TapeOutcome::Refusal {
                    reason: "the inventory walk refused".to_owned(),
                },
                remaining,
            );
            // Consumption is unknown on error, so no further walk is safe.
            break;
        };
        #[cfg(test)]
        let report = {
            let result = inventory::inventory_directory(&candidate.root, limits);
            let saved_errno = crate::tape::observer_errno();
            match result {
                Ok(report) => {
                    crate::tape::read_row(
                        "accounting-walk",
                        &candidate.root,
                        crate::tape::TapeOutcome::Facts {
                            facts: format!(
                                "entries={} complete={}",
                                report.entries_visited, report.coverage.complete
                            ),
                        },
                        remaining,
                    );
                    crate::tape::restore_observer_errno(saved_errno);
                    report
                }
                Err(error) => {
                    crate::tape::read_row(
                        "accounting-walk",
                        &candidate.root,
                        crate::tape::TapeOutcome::Refusal {
                            reason: error.to_string(),
                        },
                        remaining,
                    );
                    crate::tape::restore_observer_errno(saved_errno);
                    break;
                }
            }
        };
        remaining = remaining.saturating_sub(report.entries_visited);
        candidate.logical_bytes = report.total.logical_bytes_by_name;
        candidate.allocated_bytes = report.total.allocated_bytes;
        #[cfg(not(test))]
        {
            candidate.accounting_complete = report.coverage.complete && Instant::now() <= deadline;
        }
        #[cfg(test)]
        {
            candidate.accounting_complete = if report.coverage.complete {
                let now = Instant::now();
                crate::tape::monotonic_observed("accounting:complete:window-offset-ns", now);
                now <= deadline
            } else {
                false
            };
        }
    }
}

#[cfg(debug_assertions)]
fn prune_fixture_fault(control: &Path, variable: &str, target: &Path, marker: &str) -> bool {
    let Some(evidence) = proof_evidence() else {
        return false;
    };
    let Some(scratch) =
        std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").and_then(|path| fs::canonicalize(path).ok())
    else {
        return false;
    };
    let declared =
        std::env::var_os("OXIDE_SCRATCH_ROOT").and_then(|path| fs::canonicalize(path).ok());
    let Ok(canonical_control) = fs::canonicalize(control) else {
        return false;
    };
    if !hook_control_allowed(control, &canonical_control, &scratch, declared.as_deref())
        || !matches!(store::open_control_directory(control), Ok(Some(_)))
        || std::env::var_os(variable).as_deref() != Some(target.as_os_str())
    {
        return false;
    }
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(evidence.join(marker))
        .and_then(|mut file| writeln!(file, "{}", target.display()))
        .is_ok()
}

#[cfg(not(debug_assertions))]
fn prune_fixture_fault(_control: &Path, _variable: &str, _target: &Path, _marker: &str) -> bool {
    false
}

/// This test-only delay is armed only for a private control directory inside
/// the supervisor's scratch. It never relaxes inventory's mutation-hook rules.
#[cfg(debug_assertions)]
fn diagnostic_delay(control: &Path, root: &Path) {
    if prune_fixture_fault(
        control,
        "OXIDE_TEST_PRUNE_DIAGNOSTIC_DELAY",
        root,
        "prune-diagnostic-delay-fired",
    ) {
        std::thread::sleep(Duration::from_secs(11));
    }
}

#[cfg(not(debug_assertions))]
fn diagnostic_delay(_control: &Path, _root: &Path) {}

fn build_preview(
    registry: &Registry,
    evaluated: &[Evaluated],
    list: Option<&OperatorList>,
    authority_complete: bool,
) -> Result<Preview, String> {
    let eligible: Vec<Candidate> = evaluated
        .iter()
        .filter_map(|entry| entry.candidate.clone())
        .collect();
    let ineligible: Vec<Kept> = evaluated
        .iter()
        .filter_map(|entry| {
            entry.kept.as_ref().map(|reason| Kept {
                checkout_id: Some(entry.row.checkout_id.clone()),
                root: Some(entry.row.root.clone()),
                reason: reason.clone(),
            })
        })
        .collect();
    let generated_ms = process::now_ms();
    for candidate in &eligible {
        let trial = preview_document(
            registry,
            list,
            authority_complete,
            generated_ms,
            evaluated.len() as u64,
            eligible.len() as u64,
            ineligible.len() as u64,
            std::slice::from_ref(candidate),
            &[],
        );
        if preview_size(&trial)? > MAX_PLAN_BYTES {
            return Err(format!(
                "prune candidate {} cannot be represented in a {MAX_PLAN_BYTES}-byte preview",
                candidate.root.display()
            ));
        }
    }
    let mut low = 0usize;
    let mut high = eligible.len().min(MAX_CANDIDATES);
    while low < high {
        let mid = (low + high + 1) / 2;
        let trial = preview_document(
            registry,
            list,
            authority_complete,
            generated_ms,
            evaluated.len() as u64,
            eligible.len() as u64,
            ineligible.len() as u64,
            &eligible[..mid],
            &[],
        );
        if preview_size(&trial)? <= MAX_PLAN_BYTES {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let selected = &eligible[..low];
    let deferred = &eligible[low..];
    let mut kept = Vec::new();
    if !deferred.is_empty() {
        kept.push(Kept {
            checkout_id: None,
            root: None,
            reason: format!(
                "{} eligible root(s) deferred to a later prune batch",
                deferred.len()
            ),
        });
    }
    kept.extend(ineligible.iter().cloned());
    for candidate in deferred {
        if kept.len() >= MAX_CANDIDATES {
            break;
        }
        kept.push(Kept {
            checkout_id: Some(candidate.checkout_id.clone()),
            root: Some(candidate.root.clone()),
            reason: DEFERRED_KEEP_REASON.to_owned(),
        });
    }
    kept.truncate(MAX_CANDIDATES);
    while !kept.is_empty() {
        let trial = preview_document(
            registry,
            list,
            authority_complete,
            generated_ms,
            evaluated.len() as u64,
            eligible.len() as u64,
            ineligible.len() as u64,
            selected,
            &kept,
        );
        if preview_size(&trial)? <= MAX_PLAN_BYTES {
            break;
        }
        kept.pop();
    }
    Ok(preview_document(
        registry,
        list,
        authority_complete,
        generated_ms,
        evaluated.len() as u64,
        eligible.len() as u64,
        ineligible.len() as u64,
        selected,
        &kept,
    ))
}

fn preview_size(preview: &Preview) -> Result<usize, String> {
    serde_json::to_vec(preview)
        .map(|bytes| bytes.len())
        .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn preview_document(
    registry: &Registry,
    list: Option<&OperatorList>,
    authority_complete: bool,
    generated_ms: u64,
    roots_scanned: u64,
    eligible_count: u64,
    ineligible_count: u64,
    selected: &[Candidate],
    kept: &[Kept],
) -> Preview {
    Preview {
        version: PREVIEW_VERSION.to_owned(),
        generated_ms,
        registry_revision: registry.revision,
        registry_complete: registry.complete,
        coverage: Coverage {
            roots_scanned,
            registry_roots: registry.rows.len() as u64,
            candidate_roots: eligible_count,
            kept_roots: ineligible_count,
            complete: authority_complete,
            source: "internal output registry".to_owned(),
            observed_ms: generated_ms,
        },
        operator_list_sha256: list.map(|list| list.sha256.clone()),
        totals: Totals {
            candidates: selected.len() as u64,
            kept: ineligible_count,
            logical_bytes: selected.iter().map(|root| root.logical_bytes).sum(),
            allocated_bytes: selected.iter().map(|root| root.allocated_bytes).sum(),
            accounting_complete: selected.iter().all(|root| root.accounting_complete),
        },
        candidates: selected.to_vec(),
        kept: kept.to_vec(),
    }
}

fn observation_from(
    preview: &Preview,
    evaluated: &[Evaluated],
    completed: Vec<(Candidate, Option<String>)>,
    pending: Option<&Path>,
) -> Observations {
    let mut roots: Vec<ObservedRoot> = completed
        .iter()
        .map(|(candidate, intent)| ObservedRoot {
            checkout_id: candidate.checkout_id.clone(),
            root: candidate.root.clone(),
            outcome: "deleted".to_owned(),
            intent: intent.clone(),
            logical_bytes: candidate.logical_bytes,
            allocated_bytes: candidate.allocated_bytes,
            accounting_complete: candidate.accounting_complete,
        })
        .collect();
    for entry in evaluated {
        if roots.iter().any(|root| root.root == entry.row.root) {
            continue;
        }
        let (outcome, intent) = if pending == Some(entry.row.root.as_path()) {
            ("pending".to_owned(), None)
        } else {
            ("kept".to_owned(), None)
        };
        roots.push(ObservedRoot {
            checkout_id: entry.row.checkout_id.clone(),
            root: entry.row.root.clone(),
            outcome,
            intent,
            logical_bytes: entry
                .candidate
                .as_ref()
                .map_or(0, |candidate| candidate.logical_bytes),
            allocated_bytes: entry
                .candidate
                .as_ref()
                .map_or(0, |candidate| candidate.allocated_bytes),
            accounting_complete: entry
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.accounting_complete),
        });
    }
    roots.truncate(MAX_OBSERVATION_ROOTS);
    Observations {
        version: OBSERVATIONS_VERSION.to_owned(),
        generated_ms: process::now_ms(),
        registry_revision: preview.registry_revision,
        registry_complete: preview.registry_complete,
        source: "explicit outputs prune".to_owned(),
        coverage: preview.coverage.clone(),
        roots,
    }
}

enum Stop {
    /// A journaled intent is unfinished or its outcome is contradictory:
    /// leave it pending, stop the batch and exit 3.
    Pending(String),
    /// Refused before this root's intent was journaled: nothing about the
    /// root changed, so it is kept with the reason and the batch continues.
    Refuse(String),
}

/// Any stop before a root's intent is journaled keeps the root unchanged.
fn before_journal(stop: Stop) -> Stop {
    Stop::Refuse(stop_text(stop))
}

/// Any stop after a root's intent is journaled leaves that intent pending.
fn after_journal(stop: Stop) -> Stop {
    Stop::Pending(stop_text(stop))
}

/// One verified external root, opened through the registered volume.
struct RootSession {
    binding: Binding,
    registration: removable::Registration,
    outputs: OwnedFd,
    root: OwnedFd,
    leaf: String,
    dev: u64,
}

impl RootSession {
    fn open(candidate: &Candidate) -> Result<Self, String> {
        // The candidate's device came from this run's own observation, so the
        // reopened descriptor must still report it.
        Self::open_path(
            &candidate.root,
            candidate.volume_uuid.as_deref(),
            candidate.ino,
            Some(candidate.dev),
        )
    }

    /// Reopen the exact root a journal recorded, in a later run or after a
    /// restart. The journal's device number was observed before the crash or
    /// reboot and macOS may have renumbered the mounted volume since, so identity
    /// rests on the recorded inode and the verified volume; the caller compares
    /// the owner marker's recorded nonce, generation and digest before any
    /// effect.
    fn open_recorded(identity: &RootIdentity) -> Result<Self, String> {
        Self::open_path(
            &identity.root,
            identity.volume_uuid.as_deref(),
            identity.ino,
            None,
        )
    }

    /// Open the recorded root leaf under its verified volume and judge the
    /// descriptor against the recorded identity. `recorded_dev` is the number
    /// this run is bound to, or `None` for a journal from an earlier run whose
    /// number a reboot may have made stale.
    fn open_path(
        root_path: &Path,
        uuid: Option<&str>,
        ino: u64,
        recorded_dev: Option<u64>,
    ) -> Result<Self, String> {
        let Some(uuid) = uuid else {
            return Err("the root has no recorded volume UUID".to_owned());
        };
        let outputs_dir = root_path
            .parent()
            .ok_or("the root has no parent directory")?;
        let mount = outputs_dir
            .parent()
            .ok_or("the outputs directory has no mount")?;
        if outputs_dir.file_name() != Some(std::ffi::OsStr::new(outputs::EXTERNAL_DIR)) {
            return Err("the root is not inside an oxide-outputs directory".to_owned());
        }
        let binding = Binding::open(mount, Some(uuid))?;
        let registration = registration_for(uuid)?;
        if registration.mount_point != mount {
            return Err(format!(
                "the registered volume is mounted at {}, not {}",
                registration.mount_point.display(),
                mount.display()
            ));
        }
        let outputs = binding.dir_at(binding.fd(), outputs::EXTERNAL_DIR, false)?;
        let leaf = root_path
            .file_name()
            .ok_or("the root has no leaf name")?
            .to_string_lossy()
            .into_owned();
        let root = binding.dir_at(outputs.as_fd(), &leaf, false)?;
        let facts = fstat_fd(&root)?;
        let dev = judge_root_identity(&facts, binding.verify()?.dev, recorded_dev, ino)?;
        Ok(Self {
            binding,
            registration,
            outputs,
            root,
            leaf,
            dev,
        })
    }

    fn verify_marker(&self, candidate: &Candidate) -> Result<(), String> {
        verify_root_marker(&self.root, candidate)
    }

    /// Recheck the volume and its reserve between bounded work batches.
    /// The prune worker's own monitoring: the registered volume's identity
    /// and reserve, and the host reserve of the internal control volume
    /// (where every journal write lands). Unknown readings stop.
    fn check(&self, control: &Path, deleted: u64, recovered: u64) -> Result<(), Stop> {
        let host = oxide_storage::volume::observe_path(control).map_err(|error| {
            Stop::Pending(format!(
                "the internal control volume cannot be read ({error}); no further writes"
            ))
        })?;
        match host.available_bytes {
            Some(free) if free >= store::HOST_RESERVE_BYTES => {}
            Some(free) => {
                return Err(Stop::Pending(format!(
                "host free space {free} bytes is below the {} byte host reserve; no further writes",
                store::HOST_RESERVE_BYTES
            )))
            }
            None => {
                return Err(Stop::Pending(
                    "host free space is unknown, so the host reserve cannot be proved".to_owned(),
                ))
            }
        }
        let facts = self.binding.verify().map_err(|error| {
            Stop::Pending(format!(
                "the registered volume is no longer verified ({error})"
            ))
        })?;
        // Unknown free space is not above the reserve: stop.
        let available = if injected_low_reserve(deleted, recovered) {
            Some(0)
        } else {
            facts.available_bytes
        };
        let Some(available) = available else {
            return Err(Stop::Pending(
                "the registered volume's free space is unknown, so its reserve cannot be proved"
                    .to_owned(),
            ));
        };
        if available <= self.registration.reserve_bytes {
            return Err(Stop::Pending(format!(
                "the registered volume's free space {available} bytes is at or below the recorded {} byte reserve",
                self.registration.reserve_bytes
            )));
        }
        Ok(())
    }

    /// Exclusively rename the verified root leaf to its intent leaf inside the
    /// same outputs directory.
    fn quarantine(&self, intent: &str) -> Result<String, String> {
        // Identity is rechecked against the entry name immediately before the
        // exclusive rename; a replaced entry is never moved.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let leaf =
            std::ffi::CString::new(self.leaf.as_bytes()).map_err(|error| error.to_string())?;
        // SAFETY: the descriptor and C string are live for the call.
        let status = unsafe {
            libc::fstatat(
                self.outputs.as_raw_fd(),
                leaf.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        let held = fstat_fd(&self.root)?;
        if status != 0
            || u64::try_from(stat.st_dev).unwrap_or(u64::MAX) != self.dev
            || stat.st_ino != held.st_ino
        {
            return Err("the root entry was replaced before the quarantine rename".to_owned());
        }
        let target = format!("{QUARANTINE_PREFIX}{intent}");
        let from =
            std::ffi::CString::new(self.leaf.as_bytes()).map_err(|error| error.to_string())?;
        let to = std::ffi::CString::new(target.as_bytes()).map_err(|error| error.to_string())?;
        // SAFETY: both C strings and the directory descriptor are live; the
        // flags request an exclusive rename inside one directory.
        #[cfg(target_os = "macos")]
        let status = unsafe {
            libc::renameatx_np(
                self.outputs.as_raw_fd(),
                from.as_ptr(),
                self.outputs.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let status = {
            let _ = (&from, &to);
            -1
        };
        if status != 0 {
            return Err(format!(
                "cannot quarantine the root ({})",
                std::io::Error::last_os_error()
            ));
        }
        sync_fd(&self.outputs)?;
        Ok(target)
    }

    /// Remove the quarantine leaf itself. Finder metadata recreated in it after
    /// the delete pass is drained under the shared fixed bound (oxide#155);
    /// anything else and an exhausted bound keep the intent pending. A leaf
    /// that is simply missing is NOT positive deletion proof for a journaled
    /// intent, so it stays pending here.
    fn remove_quarantine(
        &self,
        quarantine: &str,
        check: &mut dyn FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        let name =
            std::ffi::CString::new(quarantine.as_bytes()).map_err(|error| error.to_string())?;
        for attempt in 1..=FINDER_RACE_ATTEMPTS {
            if attempt > 1 {
                std::thread::sleep(FINDER_RACE_PAUSE);
            }
            // Immediately before the leaf's own removal: the volume, reserve
            // and host control volume are checked again.
            check()?;
            // The leaf drain recovers metadata too, so it runs the same caller
            // check after every unlink it performs, not a no-op.
            let mut between = || check().map_err(DrainStop::Stopped);
            let mut context = CaptureContext {
                chain: Vec::new(),
                journal: None,
            };
            let outcome = rmdir_draining_finder_metadata(
                self.outputs.as_fd(),
                &name,
                self.root.as_fd(),
                std::ffi::OsStr::new(quarantine),
                &mut context,
                &mut between,
            );
            match &outcome {
                Rmdir::Removed => return sync_fd(&self.outputs),
                Rmdir::Drained { .. } => continue,
                Rmdir::Vanished => {
                    return Err(
                        "the quarantine leaf is missing without positive deletion proof; the intent stays pending"
                            .to_owned(),
                    )
                }
                Rmdir::Replaced { .. }
                | Rmdir::Refused { .. }
                | Rmdir::RefusedMany { .. }
                | Rmdir::Exhausted { .. }
                | Rmdir::Stopped { .. }
                | Rmdir::Mismatch { .. }
                | Rmdir::CaptureFailed { .. }
                | Rmdir::Unproven { .. }
                | Rmdir::Failed { .. } => {
                    return Err(race_pending("the quarantine leaf", &outcome))
                }
            }
        }
        Err(format!(
            "the quarantine leaf is still not empty after {FINDER_RACE_ATTEMPTS} Finder-metadata attempts; it is kept"
        ))
    }
}

impl RootSession {
    /// Open the verified quarantine leaf for an exact recorded intent.
    fn open_quarantine(identity: &RootIdentity, leaf: &str) -> Result<Self, String> {
        let Some(uuid) = identity.volume_uuid.as_deref() else {
            return Err("the intent records no volume UUID".to_owned());
        };
        let outputs_dir = identity
            .root
            .parent()
            .ok_or("the intent root has no parent")?;
        let mount = outputs_dir
            .parent()
            .ok_or("the outputs directory has no mount")?;
        let binding = Binding::open(mount, Some(uuid))?;
        let registration = registration_for(uuid)?;
        let outputs = binding.dir_at(binding.fd(), outputs::EXTERNAL_DIR, false)?;
        let root = binding.dir_at(outputs.as_fd(), leaf, false)?;
        let facts = fstat_fd(&root)?;
        let dev = judge_root_identity(&facts, binding.verify()?.dev, None, identity.ino).map_err(
            |reason| format!("the quarantine is not the recorded root ({reason}); it is kept"),
        )?;
        Ok(Self {
            binding,
            registration,
            outputs,
            root,
            leaf: leaf.to_owned(),
            dev,
        })
    }
}

fn fstat_fd(fd: &OwnedFd) -> Result<libc::stat, String> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is live for the call.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(format!(
            "cannot stat a directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(stat)
}

/// Judge one opened root descriptor against its recorded identity.
///
/// The inode is the durable identity a rename keeps, and the descriptor must be
/// on the volume the binding verified. `recorded_dev` is the device number this
/// run is bound to: `Some` only when this run observed it itself, so a difference
/// is a changed mount or root; `None` for a journal written by an earlier run,
/// whose number macOS may have renumbered on a reboot or reattach, so only the
/// inode and the verified volume decide and the caller compares the owner marker
/// before any effect. A device number is never accepted as identity on its own.
fn judge_root_identity(
    facts: &libc::stat,
    volume_dev: u64,
    recorded_dev: Option<u64>,
    ino: u64,
) -> Result<u64, String> {
    let dev = u64::try_from(facts.st_dev).unwrap_or(u64::MAX);
    #[cfg(test)]
    crate::tape::classify_row(
        "root-judge:identity",
        &format!("dev={dev} ino={ino} recorded={recorded_dev:?}"),
        &format!("volume-dev={volume_dev}"),
    );
    if facts.st_ino != ino {
        return Err("the root's inode changed".to_owned());
    }
    if dev != volume_dev {
        return Err("the root is not on the verified volume".to_owned());
    }
    if recorded_dev.is_some_and(|recorded| recorded != dev) {
        return Err("the root's device changed".to_owned());
    }
    Ok(dev)
}

fn registration_for(uuid: &str) -> Result<removable::Registration, String> {
    let internal = internal_store()?;
    let registration = removable::registration(&internal)?
        .ok_or_else(|| "no removable drive is registered".to_owned())?;
    if registration.volume_uuid != uuid {
        return Err(format!(
            "the registered volume is {}, not the recorded {uuid}",
            registration.volume_uuid
        ));
    }
    Ok(registration)
}

fn sync_fd(fd: &OwnedFd) -> Result<(), String> {
    // SAFETY: the descriptor is live for the call.
    if unsafe { libc::fsync(fd.as_raw_fd()) } != 0 {
        return Err(format!(
            "cannot sync a directory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// Where one bounded delete pass reads its volume identity and reserves.
///
/// The real caller answers through the registered removable volume; the
/// test-only branch lets a unit test drive this same traversal — observe,
/// capture, verify, descend, remove — against a plain fixture directory, so
/// the production path is what the replacement tests exercise.
enum DeleteControl<'a> {
    /// The registered removable volume and the internal control volume.
    Session(&'a RootSession),
    /// A plain test fixture directory on this host, with no registered drive.
    #[cfg(test)]
    Fixture { device: u64 },
}

/// One bounded delete pass over a verified quarantine directory.
struct Deleter<'a> {
    volume: DeleteControl<'a>,
    control: &'a Path,
    deadline: Instant,
    entries: u64,
    /// Finder metadata entries this pass recovered through the bounded seam,
    /// counted separately from `entries` so a proof trigger can key on the
    /// recovered work itself rather than on every deletion the pass made.
    recovered: u64,
    /// The last volume and reserve check: rechecked every `DELETE_BATCH`
    /// entries and at least at the existing watchdog cadence.
    checked: Instant,
}

impl Deleter<'_> {
    /// The device every entry this pass removes must live on: the registered
    /// volume's own device.
    fn device(&self) -> u64 {
        match self.volume {
            DeleteControl::Session(session) => session.dev,
            #[cfg(test)]
            DeleteControl::Fixture { device } => device,
        }
    }

    fn check(&mut self) -> Result<(), Stop> {
        let (deleted, recovered) = (self.entries, self.recovered);
        let _ = (deleted, recovered);
        // One bounded check: the delete pass's own deadline first, then the
        // registered volume and both reserves. Every caller of `check` — the
        // per-unlink drain check, the quarantine leaf's recheck and the
        // terminal recheck — therefore tests the deadline too.
        if Instant::now() > self.deadline {
            return Err(Stop::Pending(
                "the delete batch exceeded its time window; the quarantine is kept".to_owned(),
            ));
        }
        match self.volume {
            DeleteControl::Session(session) => session.check(self.control, deleted, recovered)?,
            // A fixture has no registered volume or reserve to prove; its
            // deadline was still tested above.
            #[cfg(test)]
            DeleteControl::Fixture { .. } => {}
        }
        self.checked = Instant::now();
        Ok(())
    }

    /// Recheck at least every watchdog interval during any bounded work.
    fn check_due(&mut self) -> Result<(), Stop> {
        if self.checked.elapsed() >= crate::watchdog::INTERVAL {
            self.check()?;
        }
        Ok(())
    }

    /// A pass over a plain fixture directory: no registered volume to prove,
    /// but the same traversal, same-device rule and deadline. This is how the
    /// replacement tests drive the production path without a removable drive.
    #[cfg(test)]
    fn fixture<'a>(root: &'a Path) -> Deleter<'a> {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(root).expect("fixture root must be readable");
        Self::fixture_on(root, metadata.dev())
    }

    /// A fixture pass whose volume reading is the given device, so a test can
    /// drive the same-device refusal without a second mounted filesystem.
    #[cfg(test)]
    fn fixture_on(root: &Path, device: u64) -> Deleter<'_> {
        Deleter {
            volume: DeleteControl::Fixture { device },
            control: root,
            deadline: Instant::now() + Duration::from_secs(30),
            entries: 0,
            recovered: 0,
            checked: Instant::now(),
        }
    }
}

/// Remove one quarantined directory through the held descriptors. A
/// list-then-rmdir race with Finder metadata is drained by the shared seam
/// under its fixed bound (oxide#155); every other new entry, and an exhausted
/// bound, keeps the intent pending.
fn remove_quarantined_directory(
    deleter: &mut Deleter,
    parent: BorrowedFd,
    name_c: &std::ffi::CString,
    name: &str,
    child: BorrowedFd,
) -> Result<(), Stop> {
    let mut drained = 0_u64;
    for attempt in 1..=FINDER_RACE_ATTEMPTS {
        if attempt > 1 {
            std::thread::sleep(FINDER_RACE_PAUSE);
        }
        // The caller's bounded check runs after *every* recovered metadata
        // unlink, not once per batch: a reserve, volume or deadline failure
        // stops the drain at that entry and keeps the intent pending.
        let mut between = || {
            // This entry was unlinked *before* the check that follows it, so both
            // counts advance one recovered unlink at a time: `entries` for every
            // deletion this pass made and `recovered` for the Finder metadata the
            // bounded seam itself removed.
            deleter.entries += 1;
            deleter.recovered += 1;
            deleter
                .check()
                .map_err(|stop| DrainStop::Stopped(stop_text(stop)))
        };
        let mut context = CaptureContext {
            chain: Vec::new(),
            journal: None,
        };
        let outcome = rmdir_draining_finder_metadata(
            parent,
            name_c,
            child,
            std::ffi::OsStr::new(name),
            &mut context,
            &mut between,
        );
        match &outcome {
            Rmdir::Removed => return Ok(()),
            Rmdir::Drained { entries } => {
                // Each recovered entry was already counted and checked by the
                // between-unlinks callback; only this pass's tally remains.
                drained += entries;
            }
            // A mid-drain stop already ran the caller's check and carries its
            // reason: the intent stays pending, nothing is cleared.
            Rmdir::Stopped { reason } => return Err(Stop::Pending(reason.clone())),
            // A nested directory that vanished under us is finished work
            // inside this bounded pass; unlike the quarantine leaf, its
            // absence never proves the intent's deletion.
            Rmdir::Vanished => return Ok(()),
            Rmdir::Replaced { .. }
            | Rmdir::Refused { .. }
            | Rmdir::RefusedMany { .. }
            | Rmdir::Exhausted { .. }
            | Rmdir::Mismatch { .. }
            | Rmdir::CaptureFailed { .. }
            | Rmdir::Unproven { .. }
            | Rmdir::Failed { .. } => {
                return Err(Stop::Pending(race_pending(
                    "a quarantined directory",
                    &outcome,
                )))
            }
        }
    }
    Err(Stop::Pending(format!(
        "a quarantined directory is still not empty after {FINDER_RACE_ATTEMPTS} Finder-metadata attempts ({drained} Finder-managed entries removed); it is kept"
    )))
}

/// The pending reason for a Finder-metadata race outcome that did not remove
/// the directory. Every terminal race outcome keeps the intent pending: an
/// unfinished deletion never becomes a success, an intent phase is never
/// rewritten or cleared to recover it, and a name that is no longer the
/// verified directory is never treated as that directory's removal.
fn race_pending(what: &str, outcome: &Rmdir) -> String {
    match outcome {
        // The name is already escaped by the seam.
        Rmdir::Refused { entry } => format!(
            "{what} holds an entry that is not tolerated Finder metadata ({entry}); deletion stops and the intent stays pending"
        ),
        Rmdir::Replaced { entry } => format!(
            "{what} is no longer the verified directory ({entry}); nothing was removed and the intent stays pending"
        ),
        Rmdir::RefusedMany { .. } | Rmdir::Exhausted { .. } => format!(
            "{what} gained more content than {FINDER_RACE_ATTEMPTS} Finder-metadata attempts may recover; it is kept"
        ),
        // The caller's own bounded check stopped the drain and already gave the
        // reason; that reason is the pending text.
        Rmdir::Stopped { reason } => reason.clone(),
        // A captured object that is not the observed one is never removed; both
        // names are reported so the pending intent names exactly what is left.
        Rmdir::Mismatch {
            original,
            private,
            restored,
            kind,
        } => format!(
            "{what} captured {private} for {original}, but {kind}; {}; nothing was deleted",
            if *restored {
                "it was put back under its original name"
            } else {
                "the original name was taken, so both names are kept"
            }
        ),
        Rmdir::CaptureFailed { error } => {
            format!("{what} could not capture an entry ({error}); it is kept")
        }
        Rmdir::Unproven { reason } => format!("{what} could not durably prove a capture's removal ({reason}); it is kept"),
        Rmdir::Failed { error } => format!("cannot remove {what} ({error}); it is kept"),
        // Completed or retryable outcomes are handled by every caller before
        // this function; reaching them here is still a pending intent, never
        // a success.
        Rmdir::Removed | Rmdir::Drained { .. } | Rmdir::Vanished => format!(
            "{what} was not positively removed by this step; the intent stays pending"
        ),
    }
}

/// Deterministic replacement injection at the traversal's own effect
/// boundaries (oxide#155). Absent from every non-test build: a unit test arms
/// one action that swaps the entry the traversal just observed, so the
/// production capture-and-verify path is driven at exactly that boundary
/// instead of racing on timing.
#[cfg(test)]
mod test_replace_arm {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(super) enum Phase {
        /// After the entry was observed and before it is captured.
        Observed,
        /// After a captured directory verified and before this pass descends
        /// into it.
        Descending,
        /// After a captured directory's own content was deleted and before the
        /// directory itself is removed: the Finder-metadata window.
        BeforeRmdir,
    }

    struct Armed {
        phase: Phase,
        label: String,
        action: Box<dyn FnOnce()>,
    }

    thread_local! {
        static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
    }

    /// One armed replacement for one entry label at one phase.
    #[must_use = "dropping the guard ends the armed replacement"]
    pub(super) struct Guard {
        _private: (),
    }

    pub(super) fn arm(phase: Phase, label: &str, action: impl FnOnce() + 'static) -> Guard {
        ARMED.with(|cell| {
            let mut cell = cell.borrow_mut();
            assert!(cell.is_none(), "overlapping replacement injections");
            *cell = Some(Armed {
                phase,
                label: label.to_owned(),
                action: Box::new(action),
            });
        });
        Guard { _private: () }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            ARMED.with(|cell| {
                let _ = cell.borrow_mut().take();
            });
        }
    }

    /// Run the armed action, if it is for this phase and label.
    pub(super) fn fire(phase: Phase, label: &str) {
        let action = ARMED.with(|cell| {
            let matched = {
                let armed = cell.borrow();
                armed
                    .as_ref()
                    .is_some_and(|armed| armed.phase == phase && armed.label == label)
            };
            if !matched {
                return None;
            }
            cell.borrow_mut().take().map(|armed| armed.action)
        });
        if let Some(action) = action {
            action();
        }
    }
}

/// Deterministic directory-enumeration failure for the deletion pass
/// (oxide#155). Absent from every non-test build: APFS cannot fail a listing on
/// demand, so a unit test injects the failure once the pass has already
/// collected a bounded number of names and asserts that no partial list is ever
/// acted on.
#[cfg(test)]
mod test_enumeration_fault {
    use std::cell::RefCell;

    struct Armed {
        after: u64,
        error: std::io::Error,
    }

    thread_local! {
        static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
    }

    /// One armed enumeration failure, ending on drop.
    #[must_use = "dropping the guard ends the armed enumeration failure"]
    pub(super) struct Guard {
        _private: (),
    }

    pub(super) fn arm(after: u64, code: i32) -> Guard {
        ARMED.with(|cell| {
            let mut cell = cell.borrow_mut();
            assert!(cell.is_none(), "overlapping enumeration faults");
            *cell = Some(Armed {
                after,
                error: std::io::Error::from_raw_os_error(code),
            });
        });
        Guard { _private: () }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            ARMED.with(|cell| {
                let _ = cell.borrow_mut().take();
            });
        }
    }

    /// The armed failure, once `listed` names have been collected.
    pub(super) fn fire(listed: u64) -> Option<std::io::Error> {
        ARMED.with(|cell| {
            let armed = cell.borrow_mut().take()?;
            if listed < armed.after {
                *cell.borrow_mut() = Some(armed);
                return None;
            }
            Some(armed.error)
        })
    }
}

fn delete_contents(deleter: &mut Deleter, dir: BorrowedFd, top: bool) -> Result<(), Stop> {
    delete_contents_at(deleter, dir, top, 0)
}

/// Remove one observed entry of a quarantined directory through the shared
/// capture-and-verify mechanism (oxide#155).
///
/// The traversal never acts on a listed name. It observes the entry with a
/// no-follow `fstatat`, captures it with one exclusive same-directory rename to
/// an unguessable private name, and verifies the **captured** object against
/// that observation before any effect. A directory is descended and removed
/// only through the verified capture and its verified descriptor; anything else
/// is unlinked by the private name. A capture that does not verify is put back
/// under its original name (the private name is kept when a racer took the
/// original name, and nothing is renamed when the private name no longer holds
/// the capture) and stops the caller pending, so a replacement is never
/// removed, recursed into or followed.
///
/// Returns whether this pass removed the entry: one that had already vanished
/// before its observation or its own capture is not this pass's success.
fn remove_observed_entry(
    deleter: &mut Deleter,
    dir: BorrowedFd,
    name: &str,
    depth: usize,
) -> Result<bool, Stop> {
    let name_c = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| Stop::Refuse("a quarantined name contains a NUL".to_owned()))?;
    // 1. Observe the entry, never following a link.
    let observed = match observe_entry(dir, &name_c) {
        Ok(facts) => facts,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(Stop::Refuse(format!(
                "cannot inspect a quarantined entry: {error}"
            )))
        }
    };
    if observed.device != deleter.device() {
        return Err(Stop::Pending(
            "a quarantined entry is on another device; no further delete".to_owned(),
        ));
    }
    // A symbolic link inside the quarantine is never followed, captured or
    // removed: the delete stops and the intent stays pending (exit 3).
    if observed.is_symlink() {
        return Err(Stop::Pending(format!(
            "the quarantine holds a symbolic link ({name}); deletion stops and the intent stays pending"
        )));
    }
    #[cfg(test)]
    test_replace_arm::fire(test_replace_arm::Phase::Observed, name);
    // 2. Capture it. From here on only a private name this process just
    //    created can name the object, so nothing a racer does to the original
    //    name can redirect the effect below.
    let capture = match capture_entry(dir, &name_c) {
        Ok(capture) => capture,
        // The entry vanished between the listing and its own capture: nothing
        // was renamed and nothing is this pass's work.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(Stop::Pending(format!(
                "cannot capture a quarantined entry ({error}); it is kept"
            )))
        }
    };
    // 3. Verify the *captured* object against the observation, never the name
    //    a racer could still be holding.
    let verified = match verify_captured(dir, &capture, &observed) {
        Ok(facts) => facts,
        Err(reason) => {
            // The capture is a different object from the one observed: the
            // observed entry was replaced. Nothing was verified, so nothing is
            // removed; the capture goes back if its private name still holds
            // it, and the intent stops pending either way.
            let restore = restore_capture(dir, &capture, None);
            return Err(Stop::Pending(format!(
                "the quarantined entry captured under {} is not the observed {name} ({reason}); {}; nothing was deleted",
                lossy_name(&capture.private),
                restore.note()
            )));
        }
    };
    // 4. Remove only the verified capture.
    if verified.is_directory() {
        // Descend through a descriptor opened at the private name, never
        // through the original name, and require it to be the verified object.
        let child = match open_directory_at(dir, &capture.private) {
            Ok(child) => child,
            Err(error) => {
                return Err(capture_kept(
                    dir,
                    &capture,
                    &verified,
                    name,
                    &format!("cannot open its captured directory ({error})"),
                ))
            }
        };
        match descriptor_identity(child.as_fd()) {
            Ok(identity) if identity == (verified.device, verified.inode) => {}
            Ok(_) | Err(_) => {
                return Err(capture_kept(
                    dir,
                    &capture,
                    &verified,
                    name,
                    "its captured directory is not the verified object",
                ))
            }
        }
        #[cfg(test)]
        test_replace_arm::fire(test_replace_arm::Phase::Descending, name);
        if let Err(stop) = delete_contents_at(deleter, child.as_fd(), false, depth + 1) {
            return Err(capture_kept(
                dir,
                &capture,
                &verified,
                name,
                &format!(
                    "its content was not removed ({}); it is kept",
                    stop_text(stop)
                ),
            ));
        }
        #[cfg(test)]
        test_replace_arm::fire(test_replace_arm::Phase::BeforeRmdir, name);
        // Metadata recreated inside while this pass descended is drained under
        // the shared fixed bound; the private name is the one removed.
        if let Err(stop) =
            remove_quarantined_directory(deleter, dir, &capture.private, name, child.as_fd())
        {
            return Err(capture_kept(
                dir,
                &capture,
                &verified,
                name,
                &format!("it was not removed ({})", stop_text(stop)),
            ));
        }
    } else if let Err(error) = unlink_captured(dir, &capture) {
        // Only the private name this process created is removed, and a name
        // that vanished before its removal is never this pass's success.
        return Err(capture_kept(
            dir,
            &capture,
            &verified,
            name,
            &format!("cannot remove its captured entry ({error})"),
        ));
    }
    Ok(true)
}

/// The display form of a private capture name, for a pending reason.
fn lossy_name(name: &std::ffi::CStr) -> String {
    String::from_utf8_lossy(name.to_bytes()).into_owned()
}

/// A pending reason for a capture that stopped before its removal: the object
/// is put back while the private name still holds exactly it, and a racer that
/// took the original name leaves both names in place.
fn capture_kept(
    dir: BorrowedFd,
    capture: &Capture,
    verified: &EntryFacts,
    name: &str,
    reason: &str,
) -> Stop {
    let restore = restore_capture(dir, capture, Some(verified));
    Stop::Pending(format!(
        "the quarantined entry {name}: {reason}; {}; the intent stays pending",
        restore.note()
    ))
}

/// What became of a capture this pass did not remove.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Restore {
    /// Back under its original name.
    Restored,
    /// The original name is taken again, so both names are kept.
    NameTaken,
    /// The private name no longer holds the captured object, so nothing was
    /// renamed and nothing was deleted.
    CaptureGone,
}

impl Restore {
    fn note(self) -> &'static str {
        match self {
            Restore::Restored => "it was put back under its original name",
            Restore::NameTaken => "the original name was taken again, so both names are kept",
            Restore::CaptureGone => {
                "the private capture name no longer held the captured object, so nothing was renamed"
            }
        }
    }
}

/// Put a capture back under its original name, but only while the private name
/// still holds the object this pass means to keep.
///
/// `verified` is the object the capture verified as; `None` is used for a
/// capture that failed verification, where only the private name still holding
/// *some* object is required. Every other case leaves both names alone rather
/// than renaming a replacement into the original name.
fn restore_capture(dir: BorrowedFd, capture: &Capture, verified: Option<&EntryFacts>) -> Restore {
    match verified {
        Some(verified) if verify_captured(dir, capture, verified).is_err() => {
            return Restore::CaptureGone
        }
        Some(_) => {}
        None if observe_entry(dir, &capture.private).is_err() => return Restore::CaptureGone,
        None => {}
    }
    match put_back(dir, capture) {
        Ok(()) => Restore::Restored,
        Err(_) => Restore::NameTaken,
    }
}

fn delete_contents_at(
    deleter: &mut Deleter,
    dir: BorrowedFd,
    top: bool,
    depth: usize,
) -> Result<(), Stop> {
    if depth > 64 {
        return Err(Stop::Pending(
            "the quarantine is deeper than the bounded traversal allows; it is kept".to_owned(),
        ));
    }
    // Enumeration is bounded work too: polled while names accumulate, capped,
    // and checked again after sorting, before this directory's first write.
    deleter.check_due()?;
    let mut names = list_directory_checked(dir, deleter)?;
    names.sort();
    deleter.check()?;
    for name in names {
        if top && name == outputs::MARKER {
            continue;
        }
        if Instant::now() > deleter.deadline {
            return Err(Stop::Pending(
                "the delete batch exceeded its time window; the quarantine is kept".to_owned(),
            ));
        }
        // A private capture name is never ordinary content. This caller keeps
        // no capture journal of its own, so a leftover private name here is
        // unrecorded: it is kept and the intent stays pending, never unlinked
        // by name.
        if oxide_storage::owned_delete::is_capture_name(std::ffi::OsStr::new(&name)) {
            return Err(Stop::Pending(
                "the quarantine holds a private capture name no record names; it is kept"
                    .to_owned(),
            ));
        }
        // Only an entry this pass positively removed is counted as deletion
        // work: one that had already vanished is not its success.
        if remove_observed_entry(deleter, dir, &name, depth)? {
            deleter.entries += 1;
        }
        if deleter.entries % DELETE_BATCH == 0 {
            deleter.check()?;
        } else {
            deleter.check_due()?;
        }
    }
    Ok(())
}

/// Most names one quarantined directory may hold in one bounded pass.
const MAX_DIRECTORY_NAMES: usize = 200_000;

/// One listed name, with the end of the directory separated from a failure.
///
/// `readdir` reports both the end of a directory and a failure with the same
/// null return, so `errno` is cleared before the call and read after a null
/// one: a listing that stopped early stops the pass pending instead of letting
/// it act on the partial list it had collected. A directory whose enumeration
/// cannot be proven is never read as an empty one.
fn next_listed_name(stream: *mut libc::DIR, listed: u64) -> Result<Option<String>, Stop> {
    // Only the test seam consults the ordinal; the parameter stays so the
    // caller has one shape.
    #[cfg(not(test))]
    let _ = listed;
    #[cfg(test)]
    if let Some(error) = test_enumeration_fault::fire(listed) {
        return Err(Stop::Pending(format!(
            "cannot finish reading a quarantined directory ({error}); it is kept"
        )));
    }
    clear_errno();
    // SAFETY: `stream` is a live directory stream.
    let entry = unsafe { libc::readdir(stream) };
    if entry.is_null() {
        return match enumeration_error() {
            None => Ok(None),
            Some(error) => Err(Stop::Pending(format!(
                "cannot finish reading a quarantined directory ({error}); it is kept"
            ))),
        };
    }
    // SAFETY: `d_name` is a live NUL-terminated array for this entry.
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
    Ok(Some(name.to_string_lossy().into_owned()))
}

/// `list_directory` for deletion: the deleter's cadence check runs every
/// `DELETE_BATCH` names, and more than `MAX_DIRECTORY_NAMES` stops with the
/// intent pending. One `readdir` call itself is not interrupted.
fn list_directory_checked(dir: BorrowedFd, deleter: &mut Deleter) -> Result<Vec<String>, Stop> {
    // SAFETY: `dup` returns a new descriptor.
    let dup = unsafe { libc::dup(dir.as_raw_fd()) };
    if dup < 0 {
        return Err(Stop::Pending(format!(
            "cannot read a quarantined directory ({})",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `dup` is a live descriptor that `fdopendir` takes ownership of.
    let stream = unsafe { libc::fdopendir(dup) };
    if stream.is_null() {
        // SAFETY: the duplicate is still owned here.
        unsafe { libc::close(dup) };
        return Err(Stop::Pending(format!(
            "cannot read a quarantined directory ({})",
            std::io::Error::last_os_error()
        )));
    }
    let mut names = Vec::new();
    let outcome = loop {
        let name = match next_listed_name(stream, names.len() as u64) {
            Ok(Some(name)) => name,
            Ok(None) => break Ok(()),
            Err(stop) => break Err(stop),
        };
        if name == "." || name == ".." {
            continue;
        }
        names.push(name);
        if names.len() > MAX_DIRECTORY_NAMES {
            break Err(Stop::Pending(
                "one quarantined directory holds more entries than one bounded pass; it is kept"
                    .to_owned(),
            ));
        }
        if names.len() % DELETE_BATCH as usize == 0 {
            if let Err(stop) = deleter.check_due() {
                break Err(stop);
            }
        }
    };
    // SAFETY: `stream` is live and closed once.
    unsafe { libc::closedir(stream) };
    outcome.map(|()| names)
}

fn unlink_marker(root: &OwnedFd) -> Result<(), String> {
    let name = std::ffi::CString::new(outputs::MARKER).map_err(|error| error.to_string())?;
    // SAFETY: the descriptor and C string are live for the call.
    let status = unsafe { libc::unlinkat(root.as_raw_fd(), name.as_ptr(), 0) };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("cannot unlink the owner marker ({error})"));
        }
    }
    sync_fd(root)
}

fn update_intent(control: &Path, id: &str, phase: IntentPhase) -> Result<(), String> {
    // Every registry writer holds the internal publication lock.
    let _guard = store::publication_guard(control)?;
    let mut registry = ready_registry(control)?;
    let Some(intent) = registry.intents.iter_mut().find(|intent| intent.id == id) else {
        return Err(format!("prune intent {id} disappeared"));
    };
    if intent.phase == phase {
        return Ok(());
    }
    intent.phase = phase;
    intent.updated_ms = process::now_ms();
    registry.bump()?;
    write_registry(control, &registry)
}

fn crash_point(point: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if std::env::var(CRASH_ENVIRONMENT).is_ok_and(|value| value == point) {
        return Err(format!(
            "prune: injected stop at {point} ({CRASH_ENVIRONMENT}={point}); the intent stays pending"
        ));
    }
    let _ = point;
    Ok(())
}

/// Prune one fresh candidate. Every journal entry is durable before its
/// effect; an incomplete phase returns [`Stop::Pending`] and keeps the
/// quarantine.
#[allow(clippy::too_many_arguments)]
fn prune_one(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    plan_sha: &str,
    list: Option<&OperatorList>,
    evidence: &SnapshotEvidence,
) -> Result<(), Stop> {
    // The checkout's bounded clean barrier excludes a concurrent command in
    // both pools; the binding lock then holds through the prune's effect.
    let mut waits = store::WaitAccounting::default();
    let guard = match store
        .begin_clean(&candidate.checkout, &mut waits)
        .map_err(Stop::Refuse)?
    {
        store::Cleaning::Guard(guard) => guard,
        store::Cleaning::Interrupted(signal) => {
            return Err(Stop::Refuse(format!(
                "interrupted by signal {signal} while taking the checkout's clean barrier; nothing was changed"
            )))
        }
        // `Recovering` mode never returns `Busy`; only a guarded verb's
        // `Barrier` mode does.
        store::Cleaning::Busy(reason) => return Err(Stop::Refuse(reason)),
        // `Recovering` mode never returns `Retry`; only a guarded verb's
        // `Barrier` mode does.
        store::Cleaning::Retry(reason) => return Err(Stop::Refuse(reason)),
    };
    let outcome = prune_one_barrier(store, control, candidate, plan_sha, list, evidence);
    store
        .abandon_clean(guard)
        .map_err(|error| Stop::Pending(format!("cannot release the clean barrier: {error}")))?;
    outcome
}

fn prune_one_barrier(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    plan_sha: &str,
    list: Option<&OperatorList>,
    evidence: &SnapshotEvidence,
) -> Result<(), Stop> {
    let _binding = store
        .lock_binding(&candidate.checkout)
        .map_err(Stop::Refuse)?;
    // Everything up to the journal is a per-root refusal: the root, its
    // registry row and every receipt are unchanged. This first ancestry proof
    // runs under the clean barrier and binding and is an EARLY refusal only;
    // the deciding comparison is the one taken under publication custody below.
    let (session, id, quarantine) = (|| {
        let session = RootSession::open(candidate).map_err(Stop::Refuse)?;
        session.verify_marker(candidate).map_err(Stop::Refuse)?;
        session.check(control, 0, 0)?;
        confirm_absence(candidate, evidence).map_err(Stop::Refuse)?;
        let id = fresh_nonce().map_err(Stop::Refuse)?;
        let quarantine = candidate
            .root
            .parent()
            .ok_or_else(|| Stop::Refuse("the root has no parent".to_owned()))?
            .join(format!("{QUARANTINE_PREFIX}{id}"));
        Ok((session, id, quarantine))
    })()
    .map_err(before_journal)?;
    // The locked full reread of this root's authority, then a revision
    // compare-and-set at the journal write. A revision change in between
    // gets one bounded reread; a second change keeps the root.
    let mut attempts = 0;
    loop {
        let revision = if candidate.receiptless {
            recheck_receiptless(store, control, candidate, evidence)
        } else {
            recheck_candidate(store, control, candidate, list, evidence)
        }
        .map_err(Stop::Refuse)?;
        #[cfg(debug_assertions)]
        if prune_fixture_fault(
            control,
            "OXIDE_TEST_PRUNE_CAS_PAUSE",
            &candidate.root,
            &format!("prune-cas-paused-{attempts}"),
        ) {
            let evidence = proof_evidence()
                .ok_or_else(|| Stop::Refuse("fixture evidence unavailable".to_owned()))?;
            let stop = Instant::now() + Duration::from_secs(5);
            while !evidence
                .join(format!("prune-cas-release-{attempts}"))
                .exists()
            {
                if Instant::now() >= stop {
                    return Err(Stop::Refuse("fixture CAS pause expired".to_owned()));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // Publication custody is taken for EVERY root, not only a receiptless
        // one, and it is taken BEFORE the deciding ancestry comparison below:
        // the reread above is an early refusal, and nothing it proved may be
        // published while this root waited for the lock. A receiptless root is
        // additionally protected by its exclusive `pruning` receipt before the
        // intent exists; if the intent is not published the receipt is
        // discarded again, with no output effect. The receipt publication and
        // the journal run under this one internal publication lock: the
        // receipt is compare-and-set on the recheck's revision and the
        // journal's expected revision is exactly the one that own publication
        // produced, so no unrelated change is adopted.
        let _publication = store::publication_guard(control).map_err(Stop::Refuse)?;
        // The DECIDING ancestry confirmation: a fresh descriptor walk compared
        // with the real run's ORIGINAL witness, run under publication custody
        // and immediately before the receipt/journal publication. A revision
        // retry keeps that original baseline rather than adopting a new one.
        confirm_absence(candidate, evidence).map_err(Stop::Refuse)?;
        let revision = if candidate.receiptless {
            match outputs::create_pruning_receipt(
                control,
                &candidate.checkout,
                &candidate.checkout_id,
                &id,
                &|intent| journaled(control, intent),
                revision,
            )
            .map_err(Stop::Refuse)?
            {
                Some(own) => own,
                None => {
                    attempts += 1;
                    if attempts >= 2 {
                        return Err(Stop::Refuse(
                            "the output registry changed twice while this root was rechecked; it is kept"
                                .to_owned(),
                        ));
                    }
                    continue;
                }
            }
        } else {
            revision
        };
        let published = journal_intent(
            store,
            control,
            candidate,
            plan_sha,
            &id,
            &quarantine,
            revision,
        );
        if candidate.receiptless && matches!(published, Ok(false) | Err(Stop::Refuse(_))) {
            outputs::discard_pruning_receipt(control, &candidate.checkout, &candidate.checkout_id, &id)
                .map_err(|error| {
                    Stop::Pending(format!(
                        "the pruning receipt for {} could not be discarded after its intent was not published ({error})",
                        candidate.checkout.display()
                    ))
                })?;
        }
        if published? {
            break;
        }
        attempts += 1;
        if attempts >= 2 {
            return Err(Stop::Refuse(
                "the output registry changed twice while this root was rechecked; it is kept"
                    .to_owned(),
            ));
        }
    }
    prune_journaled(store, control, &session, &id).map_err(after_journal)
}

/// Under the checkout's barrier and binding lock: reread the ready registry
/// and this root's receipt and re-run every eligibility predicate for its
/// row, with the root's recorded identity unchanged. Returns the registry
/// revision this proof read.
fn recheck_candidate(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    list: Option<&OperatorList>,
    evidence: &SnapshotEvidence,
) -> Result<u64, String> {
    if prune_fixture_fault(
        control,
        "OXIDE_TEST_PRUNE_LOCKED_REFUSE",
        &candidate.root,
        "prune-locked-refuse-fired",
    ) {
        return Err("the locked fixture recheck refused".to_owned());
    }
    let mut authority = AuthorityScope::begin()?;
    let registry = ready_registry(control)?;
    registry.validate()?;
    if !registry.complete {
        return Err("the registry became sticky-incomplete".to_owned());
    }
    let row = registry
        .rows
        .iter()
        .find(|row| {
            row.checkout_id == candidate.checkout_id
                && row.member == candidate.member
                && row.root == candidate.root
        })
        .cloned()
        .ok_or("the root's registry row disappeared")?;
    if row.nonce != candidate.nonce || row.generation != candidate.generation {
        return Err("the root's recorded nonce or generation changed".to_owned());
    }
    let receipt = outputs::read_receipt_for_prune(control, &row.checkout, &row.checkout_id)?
        .ok_or("the root's receipt is missing")?;
    let mut budget = SNAPSHOT_MAX_BYTES;
    // The snapshot evidence this recheck produces is compared with the real
    // run's witness as an EARLY refusal, under the checkout's clean barrier and
    // binding. The deciding comparison is made again under publication custody
    // in `prune_one_barrier`, immediately before the journal.
    let mut fresh = None;
    if let Some(reason) = eligibility(
        store,
        control,
        &registry,
        &row,
        Some(&receipt),
        list,
        &mut authority,
        &mut budget,
        true,
        &mut fresh,
    )? {
        return Err(reason);
    }
    match &fresh {
        Some(current) if current == evidence => {}
        Some(_) => return Err(
            "the checkout's ancestry changed after the real run's snapshot, so its root is kept"
                .to_owned(),
        ),
        None => return Err("the checkout's absence is no longer positively proved".to_owned()),
    }
    match observe_row_root(&row)? {
        RootState::Present(facts)
            if facts.dev == candidate.dev
                && facts.ino == candidate.ino
                && facts.marker_digest == candidate.marker_digest => {}
        RootState::Present(_) => return Err("the root's identity changed".to_owned()),
        RootState::Absent => return Err("the root is already absent".to_owned()),
        RootState::Unknown(reason) => return Err(reason),
    }
    if !authority.complete() {
        return Err("the locked authority recheck exceeded its input or time budget".to_owned());
    }
    Ok(registry.revision)
}

/// The effects after a durable intent; every stop leaves the intent pending.
fn prune_journaled(
    store: &Store,
    control: &Path,
    session: &RootSession,
    id: &str,
) -> Result<(), Stop> {
    let id = id.to_owned();
    crash_point("intent").map_err(Stop::Pending)?;
    session.quarantine(&id).map_err(Stop::Pending)?;
    if let Err(error) = update_intent(control, &id, IntentPhase::Renamed) {
        return Err(Stop::Pending(format!(
            "the quarantine rename succeeded but its journal phase did not: {error}"
        )));
    }
    crash_point("renamed").map_err(Stop::Pending)?;
    update_intent(control, &id, IntentPhase::Deleting).map_err(|error| {
        Stop::Pending(format!(
            "deletion cannot begin without its journal phase: {error}"
        ))
    })?;
    finish_quarantine(store, control, session, &id)
}

/// Every registry row that references one (checkout, root) group, sorted by
/// member. The group, never one row, is the transaction scope.
fn group_rows<'a>(registry: &'a Registry, checkout_id: &str, root: &Path) -> Vec<&'a Row> {
    let mut rows: Vec<&Row> = registry
        .rows
        .iter()
        .filter(|row| row.checkout_id == checkout_id && row.root == root)
        .collect();
    rows.sort_by(|left, right| left.member.cmp(&right.member));
    rows
}

/// One evaluated representative per (checkout, root) group, in a stable order.
///
/// The representative is the group's first LIVE reference in member order: a
/// row that is selected, pending, reserved or already owned by an intent. An
/// inactive history row is never the representative while the group still has
/// one, because a history reference keeps its own recorded facts (an old legacy
/// `target` row's superseded nonce or generation, oxide#218) and is not the
/// checkout's current authority — comparing it with the live marker would keep
/// the whole batch for a contradiction the group does not have. A group with no
/// live reference keeps the first member, exactly as before.
fn representatives(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|left, right| {
        (
            left.checkout_id.as_str(),
            left.root.as_path(),
            !left.live(),
            left.member.as_str(),
        )
            .cmp(&(
                right.checkout_id.as_str(),
                right.root.as_path(),
                !right.live(),
                right.member.as_str(),
            ))
    });
    let mut grouped: Vec<Row> = Vec::new();
    let mut group_keys: Vec<(String, PathBuf)> = Vec::new();
    for row in rows {
        let key = (row.checkout_id.clone(), row.root.clone());
        if group_keys.iter().any(|known| known == &key) {
            continue;
        }
        group_keys.push(key);
        grouped.push(row);
    }
    grouped
}

/// The verified group of one candidate root: all references share the root's
/// immutable identity, none is pending or already owned by another
/// transaction. Anything else refuses.
fn verified_group(registry: &Registry, candidate: &Candidate) -> Result<Vec<GroupRow>, String> {
    let rows = group_rows(registry, &candidate.checkout_id, &candidate.root);
    if rows.is_empty() {
        return Err("no registry row references the candidate root".to_owned());
    }
    for row in &rows {
        if row.pending
            || row.pruning.is_some()
            || row.reservation.is_some()
            || row.cancelling.is_some()
        {
            return Err(format!(
                "the registry reference {} is not an eligible part of this root's group",
                row.member
            ));
        }
        // A historical row keeps its OWN recorded facts. Only a row that is
        // currently selected is part of the checkout's current authority and
        // therefore has to agree with the root's live identity; requiring
        // every historical row to match the representative would reject a
        // legitimate inactive legacy `target` row (no nonce, old digest).
        if row.nonce.as_deref().is_some_and(|nonce| !is_nonce(nonce))
            || row
                .marker_digest
                .as_deref()
                .is_some_and(|digest| !hex64(digest))
        {
            return Err(format!(
                "the registry reference {} carries an unusable recorded identity",
                row.member
            ));
        }
    }
    // The CURRENT authority is the selected subset. A released or detached
    // root legitimately has none: its complete group is valid history, and
    // physical identity is still proven by the observation behind `candidate`.
    let current: Vec<&Row> = rows.iter().copied().filter(|row| row.selected).collect();
    if let Some(first) = current.first() {
        for row in &current {
            // A recorded device may be stale on either side of a reboot, which is
            // not a disagreement about the root's identity; the immutable facts
            // below are.
            if row.nonce != first.nonce
                || row.generation != first.generation
                || row.root_ino != first.root_ino
                || row.marker_digest != first.marker_digest
                || row.root_kind != first.root_kind
                || row.volume_uuid != first.volume_uuid
            {
                return Err(
                    "the root's currently selected registry references disagree about its identity"
                        .to_owned(),
                );
            }
        }
        // The candidate's device was observed by this run, so a differing
        // recorded number is the stale one: the inode and the marker digest (with
        // the marker's own volume, nonce and generation, verified by the
        // observation) are what prove this is the recorded root.
        if first.nonce != candidate.nonce
            || first.root_ino != Some(candidate.ino)
            || first.marker_digest.as_deref() != Some(candidate.marker_digest.as_str())
        {
            return Err("the root's registry identity changed since the plan".to_owned());
        }
    }
    Ok(rows
        .iter()
        .map(|row| GroupRow {
            member: row.member.clone(),
            generation: row.generation,
            nonce: row.nonce.clone(),
            root_dev: row.root_dev,
            root_ino: row.root_ino,
            marker_digest: row.marker_digest.clone(),
            selected: row.selected,
        })
        .collect())
}

/// The checkout receipt's exact bytes, for the intent's preimage pin.
fn receipt_preimage(control: &Path, checkout_id: &str) -> Result<String, String> {
    let path = outputs::receipt_path_public(control, checkout_id);
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

/// Verify, before any selection flag is recomputed, that the checkout receipt
/// is the recorded preimage and that its owned references to this root are
/// exactly the recorded group (same members, generations and nonce), with no
/// prepared transition.
fn verify_group_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    root: &Path,
    group: &[GroupRow],
) -> Result<(), String> {
    let receipt = outputs::read_receipt_for_prune(control, checkout, checkout_id)?
        .ok_or_else(|| "the checkout's receipt disappeared".to_owned())?;
    verify_group_against_receipt(&receipt, root, group)
}

fn verify_group_against_receipt(
    receipt: &Receipt,
    root: &Path,
    group: &[GroupRow],
) -> Result<(), String> {
    if receipt.prepared_ref().is_some() {
        return Err("the checkout receipt has a pending transition".to_owned());
    }
    // Normalize the receipt's current placements and hand them to the pure
    // comparison. Only the CURRENTLY SELECTED part of the group is the
    // checkout's current authority; registry-only inactive history is never
    // invented into the receipt and is not required to appear there.
    let current: Vec<(String, PathBuf, u64, Option<String>)> = receipt
        .links()
        .iter()
        .filter_map(|placement| {
            placement.selected_ref().map(|selected| {
                (
                    placement.member().to_owned(),
                    selected.root().to_path_buf(),
                    selected.generation(),
                    selected.nonce().map(str::to_owned),
                )
            })
        })
        .collect();
    selected_group_mismatch(&current, root, group)
}

/// The exact rule between a receipt's current references and the recorded
/// group: the receipt's current members for THIS root must be exactly the
/// group's SELECTED members, each with the recorded generation and nonce.
///
/// Historical rows (an inactive legacy `target`, or any released member) keep
/// their own recorded facts and are intentionally absent from this comparison,
/// and a group with no selected row requires the receipt to select nothing for
/// this root. Kept separate from the receipt type so the rule is unit-tested
/// without a receipt fixture.
fn selected_group_mismatch(
    current: &[(String, PathBuf, u64, Option<String>)],
    root: &Path,
    group: &[GroupRow],
) -> Result<(), String> {
    let mut receipt_members: Vec<&str> = current
        .iter()
        .filter(|(_, receipt_root, _, _)| receipt_root == root)
        .map(|(member, _, _, _)| member.as_str())
        .collect();
    receipt_members.sort_unstable();
    let selected: Vec<&GroupRow> = group.iter().filter(|row| row.selected).collect();
    let mut group_members: Vec<&str> = selected.iter().map(|row| row.member.as_str()).collect();
    group_members.sort_unstable();
    if receipt_members != group_members {
        return Err(
            "the checkout receipt's current references do not match the selected part of the recorded group"
                .to_owned(),
        );
    }
    for row in selected {
        let Some((_, _, generation, nonce)) = current
            .iter()
            .find(|(member, receipt_root, _, _)| member == &row.member && receipt_root == root)
        else {
            return Err("the receipt lost a selected group member".to_owned());
        };
        if *generation != row.generation || nonce.as_deref() != row.nonce.as_deref() {
            return Err(format!(
                "the receipt's reference for {} disagrees with the recorded group",
                row.member
            ));
        }
    }
    Ok(())
}

/// Publish the durable intent under internal admission. `Ok(false)` means the
/// registry revision is no longer the one the locked recheck read: nothing
/// was written.
fn journal_intent(
    _store: &Store,
    control: &Path,
    candidate: &Candidate,
    plan_sha: &str,
    id: &str,
    quarantine: &Path,
    expected_revision: u64,
) -> Result<bool, Stop> {
    let _guard = store::publication_guard(control).map_err(Stop::Refuse)?;
    let mut registry = ready_registry(control).map_err(Stop::Refuse)?;
    registry.validate().map_err(Stop::Refuse)?;
    if registry.revision != expected_revision {
        return Ok(false);
    }
    if !registry.complete {
        return Err(Stop::Refuse(
            "the registry became incomplete before the intent".to_owned(),
        ));
    }
    let mut was_selected = false;
    let mut group = Vec::new();
    let mut prior_receipt_sha = String::new();
    if candidate.receiptless {
        // A temporary pruning row, within the live caps; never a selection.
        if registry.rows.iter().any(|row| row.root == candidate.root) {
            return Err(Stop::Refuse(
                "a registry row now names the listed root".to_owned(),
            ));
        }
        if registry.rows.len() >= MAX_ROWS {
            return Err(Stop::Refuse(
                "the registry row limit leaves no room for a temporary pruning row".to_owned(),
            ));
        }
        registry.rows.push(Row {
            checkout_id: candidate.checkout_id.clone(),
            checkout: candidate.checkout.clone(),
            checkout_volume_uuid: None,
            receipt: candidate.receipt.clone(),
            member: candidate.member.clone(),
            root: candidate.root.clone(),
            root_kind: candidate.root_kind,
            volume_uuid: candidate.volume_uuid.clone(),
            recipe: outputs::RECIPE_PUBLIC.to_owned(),
            nonce: candidate.nonce.clone(),
            generation: candidate.generation,
            root_dev: Some(candidate.dev),
            root_ino: Some(candidate.ino),
            marker_digest: Some(candidate.marker_digest.clone()),
            selected: false,
            pending: false,
            pruning: Some(id.to_owned()),
            pruned_ms: None,
            reservation: None,
            cancelling: None,
        });
    } else {
        let verified = verified_group(&registry, candidate).map_err(Stop::Refuse)?;
        prior_receipt_sha =
            receipt_preimage(control, &candidate.checkout_id).map_err(Stop::Refuse)?;
        verify_group_receipt(
            control,
            &candidate.checkout,
            &candidate.checkout_id,
            &candidate.root,
            &verified,
        )
        .map_err(Stop::Refuse)?;
        was_selected = verified.iter().any(|row| row.selected);
        // Atomically mark ALL of the group's rows: one root, one owner. The
        // recorded device is refreshed to this run's own observation in the same
        // publication: macOS renumbers a mounted volume's `st_dev` on a reboot or
        // reattach, and the locked recheck has just proved by the row's inode,
        // the verified volume and the owner marker that this is that root. The
        // intent's group records the same refreshed number, so a rollback
        // compares the journal with rows written under it rather than with a
        // number that predates the reboot.
        for row in registry
            .rows
            .iter_mut()
            .filter(|row| row.checkout_id == candidate.checkout_id && row.root == candidate.root)
        {
            row.selected = false;
            row.pending = false;
            row.pruning = Some(id.to_owned());
            row.root_dev = Some(candidate.dev);
        }
        group = verified
            .into_iter()
            .map(|row| GroupRow {
                root_dev: Some(candidate.dev),
                ..row
            })
            .collect();
    }
    while registry.intents.len() >= MAX_INTENTS {
        let completed = registry
            .intents
            .iter()
            .position(Intent::settled)
            .ok_or_else(|| {
                Stop::Refuse(
                    "the unfinished prune intent limit is reached; settle an intent first"
                        .to_owned(),
                )
            })?;
        registry.intents.remove(completed);
    }
    registry.intents.push(Intent {
        version: INTENT_VERSION.to_owned(),
        id: id.to_owned(),
        identity: RootIdentity {
            checkout_id: candidate.checkout_id.clone(),
            checkout: candidate.checkout.clone(),
            receipt: candidate.receipt.clone(),
            member: candidate.member.clone(),
            root: candidate.root.clone(),
            root_kind: candidate.root_kind,
            volume_uuid: candidate.volume_uuid.clone(),
            nonce: candidate.nonce.clone(),
            generation: candidate.generation,
            dev: candidate.dev,
            ino: candidate.ino,
            marker_digest: candidate.marker_digest.clone(),
        },
        phase: IntentPhase::Intent,
        quarantine: quarantine.to_path_buf(),
        preview_sha256: plan_sha.to_owned(),
        created_ms: process::now_ms(),
        updated_ms: process::now_ms(),
        resolved_ms: None,
        receipt_settled: false,
        was_selected,
        prior_receipt_sha,
        group,
        expected_terminal_sha: None,
        terminal_ms: None,
    });
    registry.bump().map_err(Stop::Refuse)?;
    // A failed publication may still have landed: the journal is then an
    // unfinished intent with no filesystem effect, which the next run rolls
    // back exactly. It is never reported as a pre-effect refusal.
    write_registry(control, &registry).map_err(|error| {
        Stop::Pending(format!(
            "the prune intent's publication did not complete ({error}); a journaled intent may be pending"
        ))
    })?;
    Ok(true)
}

/// Delete the verified quarantine, publish deletion proof, then the registry
/// terminal state and finally the pruned checkout receipt.
fn finish_quarantine(
    store: &Store,
    control: &Path,
    session: &RootSession,
    id: &str,
) -> Result<(), Stop> {
    let mut deleter = Deleter {
        volume: DeleteControl::Session(session),
        control,
        deadline: Instant::now() + Duration::from_secs(120),
        entries: 0,
        recovered: 0,
        checked: Instant::now(),
    };
    crash_point("deleting").map_err(Stop::Pending)?;
    // Every deletion, fresh or resumed, starts with a check before its
    // first write, and the marker-last write is checked too.
    deleter.check()?;
    delete_contents(&mut deleter, session.root.as_fd(), true)?;
    deleter.check()?;
    update_intent(control, id, IntentPhase::MarkerLast)
        .map_err(|error| Stop::Pending(format!("cannot record marker-last: {error}")))?;
    unlink_marker(&session.root).map_err(Stop::Refuse)?;
    // Content gained after the marker was removed keeps the intent pending,
    // except the Finder metadata the bounded removal recovers (oxide#155).
    if !only_finder_metadata(session.root.as_fd())
        .map_err(|error| Stop::Refuse(format!("cannot read the quarantine ({error})")))?
    {
        return Err(Stop::Pending(
            "the quarantine gained content after its marker was removed".to_owned(),
        ));
    }
    deleter.check()?;
    let mut check = || deleter.check().map_err(stop_text);
    session
        .remove_quarantine(&format!("{QUARANTINE_PREFIX}{id}"), &mut check)
        .map_err(Stop::Pending)?;
    // Immediately before the terminal journal writes: the same bounded check.
    deleter.check()?;
    complete_intent(store, control, id, "deleted")
}

fn complete_intent(_store: &Store, control: &Path, id: &str, outcome: &str) -> Result<(), Stop> {
    {
        let _guard = store::publication_guard(control).map_err(Stop::Refuse)?;
        let mut registry = ready_registry(control).map_err(Stop::Refuse)?;
        let identity = registry
            .intent(id)
            .map(|intent| intent.identity.clone())
            .ok_or_else(|| Stop::Refuse("the prune intent disappeared".to_owned()))?;
        let terminal = registry
            .intent(id)
            .is_some_and(|intent| intent.phase == IntentPhase::Deleted);
        if !terminal {
            if let Some(intent) = registry.intents.iter_mut().find(|intent| intent.id == id) {
                intent.phase = IntentPhase::Deleted;
                intent.updated_ms = process::now_ms();
            }
            let now = process::now_ms();
            let group: Vec<String> = registry
                .intent(id)
                .map(|intent| intent.group.iter().map(|row| row.member.clone()).collect())
                .unwrap_or_default();
            for row in registry.rows.iter_mut().filter(|row| {
                row.checkout_id == identity.checkout_id
                    && row.root == identity.root
                    && (group.is_empty() || group.contains(&row.member))
            }) {
                row.selected = false;
                row.pending = false;
                row.pruning = None;
                row.pruned_ms = Some(now);
            }
            registry.bump().map_err(Stop::Refuse)?;
            write_registry(control, &registry).map_err(Stop::Refuse)?;
        }
    }
    crash_point("terminal").map_err(Stop::Pending)?;
    settle_receipt(control, id, outcome).map_err(Stop::Pending)
}

/// Publish the checkout receipt's terminal image with its FIXED recorded
/// timestamp. The expected image digest is durable before the publication;
/// a replay accepts only the recorded preimage or that exact image.
fn publish_terminal_receipt(
    control: &Path,
    intent: &Intent,
    id: &str,
    outcome: &str,
) -> Result<(), String> {
    let path = outputs::receipt_path_public(control, &intent.identity.checkout_id);
    let read_digest = |path: &Path| -> Result<String, String> {
        let bytes = fs::read(path).map_err(|error| {
            format!(
                "cannot read the checkout receipt {}: {error}",
                path.display()
            )
        })?;
        Ok(sha256_hex(&bytes))
    };
    match (&intent.expected_terminal_sha, intent.terminal_ms) {
        (Some(expected), Some(at_ms)) => {
            let live = read_digest(&path)?;
            if &live == expected {
                return Ok(());
            }
            if !intent.prior_receipt_sha.is_empty() && live == intent.prior_receipt_sha {
                outputs::mark_pruned_at(
                    control,
                    &intent.identity.checkout,
                    &intent.identity.checkout_id,
                    id,
                    outcome,
                    at_ms,
                )?;
                let after = read_digest(&path)?;
                if &after != expected {
                    return Err(format!(
                        "the terminal receipt image for intent {id} is not the recorded one; it is kept pending"
                    ));
                }
                return Ok(());
            }
            Err(format!(
                "the checkout receipt is neither the recorded preimage nor the recorded terminal image for intent {id}; it is kept pending"
            ))
        }
        _ => {
            let live = read_digest(&path)?;
            if !intent.prior_receipt_sha.is_empty() && live != intent.prior_receipt_sha {
                return Err(format!(
                    "the checkout receipt changed since prune intent {id}; nothing was published"
                ));
            }
            let at_ms = process::now_ms();
            let image = outputs::terminal_receipt_image(
                control,
                &intent.identity.checkout,
                &intent.identity.checkout_id,
                id,
                outcome,
                at_ms,
            )?;
            let expected = sha256_hex(&image);
            // Durable before publication: the fixed timestamp and the exact
            // expected image. Another writer's record wins; we then publish
            // exactly that image.
            let (expected, at_ms) = {
                let _guard = store::publication_guard(control)?;
                let mut registry = ready_registry(control)?;
                let Some(record) = registry.intents.iter_mut().find(|record| record.id == id)
                else {
                    return Err(format!("prune intent {id} disappeared"));
                };
                match (&record.expected_terminal_sha, record.terminal_ms) {
                    (Some(sha), Some(ms)) => (sha.clone(), ms),
                    _ => {
                        record.expected_terminal_sha = Some(expected.clone());
                        record.terminal_ms = Some(at_ms);
                        record.updated_ms = process::now_ms();
                        registry.bump()?;
                        write_registry(control, &registry)?;
                        (expected, at_ms)
                    }
                }
            };
            outputs::mark_pruned_at(
                control,
                &intent.identity.checkout,
                &intent.identity.checkout_id,
                id,
                outcome,
                at_ms,
            )?;
            let after = read_digest(&path)?;
            if after != expected {
                return Err(format!(
                    "the terminal receipt image for intent {id} is not the recorded one; it is kept pending"
                ));
            }
            Ok(())
        }
    }
}

/// After the registry terminal state is durable: commit the checkout
/// receipt's terminal state (idempotent for the same intent), then record
/// that it committed. Until that record, the intent stays unfinished and
/// this step is replayed.
fn settle_receipt(control: &Path, id: &str, outcome: &str) -> Result<(), String> {
    let intent = ready_registry(control)?
        .intent(id)
        .cloned()
        .ok_or_else(|| format!("prune intent {id} disappeared"))?;
    if intent.receipt_settled {
        return Ok(());
    }
    // A released root is history in its receipt: the receipt's own current
    // selection (if any) is never rewritten for it.
    let receipt = outputs::read_receipt_for_prune(
        control,
        &intent.identity.checkout,
        &intent.identity.checkout_id,
    )?;
    let owns_terminal = receipt.as_ref().is_some_and(|receipt| {
        outputs::receipt_is_pruning(receipt)
            || receipt
                .current()
                .is_some_and(|selected| selected.root() == intent.identity.root)
    });
    if owns_terminal {
        publish_terminal_receipt(control, &intent, id, outcome)?;
        // Debug-only proof stop AFTER the expected terminal image digest is
        // durable and that exact image is published, but BEFORE the intent is
        // marked settled. The existing `terminal` hook keeps its earlier
        // pre-settlement meaning.
        crash_point("terminal-image")?;
    }
    {
        let _guard = store::publication_guard(control)?;
        let mut registry = ready_registry(control)?;
        let Some(record) = registry
            .intents
            .iter()
            .find(|record| record.id == id)
            .cloned()
        else {
            return Err(format!("prune intent {id} disappeared"));
        };
        // Both terminal states are durable: the completed row and the settled
        // intent leave the live registry in this one revision. Unsettled
        // intents are never removed here.
        registry.intents.retain(|record| record.id != id);
        registry.rows.retain(|row| {
            !(row.checkout_id == record.identity.checkout_id
                && row.root == record.identity.root
                && row.pruned_ms.is_some()
                && !row.live())
        });
        registry.bump()?;
        write_registry(control, &registry)?;
    }
    // Diagnostics only, best effort after the positive completion.
    if let Err(reason) = append_tombstone(
        control,
        Tombstone {
            intent: id.to_owned(),
            checkout_id: intent.identity.checkout_id.clone(),
            root: intent.identity.root.clone(),
            nonce: intent.identity.nonce.clone(),
            outcome: outcome.to_owned(),
            at_ms: process::now_ms(),
        },
    ) {
        eprintln!("oxide: the prune tombstone was not recorded ({reason}); diagnostics only");
    }
    Ok(())
}

/// One settled prune outcome kept for diagnostics.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tombstone {
    intent: String,
    checkout_id: String,
    root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    outcome: String,
    at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tombstones {
    version: String,
    entries: Vec<Tombstone>,
}

/// Append to the bounded ring, compacting oldest first to 16,384 entries and
/// 4 MiB. An unreadable or foreign ring is replaced: it is diagnostics, and
/// no safety decision reads it.
fn append_tombstone(control: &Path, entry: Tombstone) -> Result<(), String> {
    let _guard = store::publication_guard(control)?;
    let path = registry_dir(control).join(TOMBSTONES_FILE);
    let mut ring = read_bounded(&path, MAX_TOMBSTONE_BYTES, "tombstone ring")
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Tombstones>(&bytes).ok())
        .filter(|ring| ring.version == TOMBSTONES_VERSION)
        .unwrap_or(Tombstones {
            version: TOMBSTONES_VERSION.to_owned(),
            entries: Vec::new(),
        });
    ring.entries.push(entry);
    compact_tombstones(&mut ring)?;
    let bytes = serde_json::to_vec(&ring).map_err(|error| error.to_string())?;
    let temp = registry_dir(control).join(format!(".tombstones.tmp-{}", std::process::id()));
    write_private(&temp, &bytes)?;
    fs::rename(&temp, &path)
        .map_err(|error| format!("cannot publish the tombstone ring: {error}"))?;
    sync_directory(&registry_dir(control))
}

fn compact_tombstones(ring: &mut Tombstones) -> Result<(), String> {
    let excess = ring.entries.len().saturating_sub(MAX_TOMBSTONES);
    ring.entries.drain(..excess);
    while !ring.entries.is_empty()
        && serde_json::to_vec(ring)
            .map_err(|error| error.to_string())?
            .len()
            > MAX_TOMBSTONE_BYTES
    {
        let drop = (ring.entries.len() / 16).max(1);
        ring.entries.drain(..drop);
    }
    Ok(())
}

/// The row a candidate's own absence proof needs: the recorded checkout path,
/// its recorded volume and the root's recorded identity. It is built here so
/// the locked proof asks exactly the question the snapshot asked.
fn candidate_row(candidate: &Candidate) -> Row {
    Row {
        checkout_id: candidate.checkout_id.clone(),
        checkout: candidate.checkout.clone(),
        checkout_volume_uuid: candidate.checkout_volume_uuid.clone(),
        receipt: candidate.receipt.clone(),
        member: candidate.member.clone(),
        root: candidate.root.clone(),
        root_kind: candidate.root_kind,
        volume_uuid: candidate.volume_uuid.clone(),
        recipe: String::new(),
        nonce: candidate.nonce.clone(),
        generation: candidate.generation,
        root_dev: Some(candidate.dev),
        root_ino: Some(candidate.ino),
        marker_digest: Some(candidate.marker_digest.clone()),
        selected: true,
        pending: false,
        pruning: None,
        pruned_ms: None,
        reservation: None,
        cancelling: None,
    }
}

/// What resuming one unfinished intent settled.
enum Resumed {
    /// The root is deleted and its terminal state committed.
    Completed,
    /// An intent-phase journal with no filesystem effect was removed and the
    /// row's selection restored; nothing was deleted.
    RolledBack,
    /// An operator resolution's terminal receipt committed; no root effect.
    Settled,
}

/// Run `work` under `checkout`'s bounded clean barrier and binding lock, the
/// same custody a fresh prune takes for each root.
fn under_checkout_barrier<T>(
    store: &Store,
    checkout: &Path,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let mut waits = store::WaitAccounting::default();
    let guard = match store.begin_clean(checkout, &mut waits)? {
        store::Cleaning::Guard(guard) => guard,
        store::Cleaning::Interrupted(signal) => {
            return Err(format!(
                "interrupted by signal {signal} while taking the checkout's clean barrier"
            ))
        }
        // `Recovering` mode never returns `Busy`; only a guarded verb's
        // `Barrier` mode does.
        store::Cleaning::Busy(reason) => return Err(reason),
        // `Recovering` mode never returns `Retry`; only a guarded verb's
        // `Barrier` mode does.
        store::Cleaning::Retry(reason) => return Err(reason),
    };
    let outcome = (|| {
        let _binding = store.lock_binding(checkout)?;
        work()
    })();
    match store.abandon_clean(guard) {
        Ok(()) => outcome,
        Err(error) => Err(match outcome {
            Ok(_) => format!("cannot release the clean barrier: {error}"),
            Err(reason) => format!("{reason}; cannot release the clean barrier: {error}"),
        }),
    }
}

/// Resume an unfinished delete transaction for exactly the recorded intent.
fn resume_intent(store: &Store, control: &Path, intent: &Intent) -> Result<Resumed, String> {
    match intent.phase {
        IntentPhase::Deleted => {
            complete_intent(store, control, &intent.id, "deleted").map_err(stop_text)?;
            Ok(Resumed::Completed)
        }
        IntentPhase::Renamed | IntentPhase::Deleting | IntentPhase::MarkerLast => {
            let session =
                RootSession::open_quarantine(&intent.identity, &quarantine_leaf(intent)?)?;
            resume_quarantine(store, control, intent, &session)
        }
        IntentPhase::Resolved => {
            settle_receipt(control, &intent.id, RESOLVED_OUTCOME)?;
            Ok(Resumed::Settled)
        }
        IntentPhase::Intent => {
            // No filesystem effect is recorded. If the original root is still
            // present and unchanged, the journal can be rolled back exactly;
            // any other state is uncertain and stays pending. The journal's
            // device number may predate a reboot, so the reopen proves identity by
            // the inode and the verified volume and the marker comparison in
            // `replay_journaled_root` binds the reserved nonce, generation and
            // recorded digest.
            let candidate = journal_candidate(intent);
            let reopened = match RootSession::open_recorded(&intent.identity) {
                Ok(session) => Reopened::Original(OpenedRoot::Session(session)),
                Err(original) => {
                    // A crash between the exclusive rename and its journal
                    // phase leaves the root under the intent's own random
                    // quarantine leaf. Only that exact inode, on the recorded
                    // volume, with the recorded marker, resumes deletion.
                    match RootSession::open_quarantine(
                        &intent.identity,
                        &quarantine_leaf(intent)?,
                    ) {
                        Ok(session) => Reopened::Quarantine(OpenedRoot::Session(session)),
                        Err(quarantine) => Reopened::Missing(format!(
                            "the root is not at its path ({original}) or in its quarantine ({quarantine})"
                        )),
                    }
                }
            };
            replay_journaled_root(store, control, intent, &candidate, reopened)
        }
    }
}

/// The plan identity a journal recorded, as the replay compares it.
fn journal_candidate(intent: &Intent) -> Candidate {
    Candidate {
        checkout_id: intent.identity.checkout_id.clone(),
        checkout: intent.identity.checkout.clone(),
        receipt: intent.identity.receipt.clone(),
        member: intent.identity.member.clone(),
        root: intent.identity.root.clone(),
        root_kind: intent.identity.root_kind,
        volume_uuid: intent.identity.volume_uuid.clone(),
        checkout_volume_uuid: None,
        nonce: intent.identity.nonce.clone(),
        generation: intent.identity.generation,
        dev: intent.identity.dev,
        ino: intent.identity.ino,
        marker_digest: intent.identity.marker_digest.clone(),
        logical_bytes: 0,
        allocated_bytes: 0,
        accounting_complete: false,
        receiptless: false,
    }
}

/// Where reopening a journaled root found it, and through what its owner marker
/// is compared.
///
/// The opener is the only part of a replay that needs the verified mount, which
/// a unit test cannot create (only `/Volumes/<name>` binds). Everything after
/// that decision is one function, [`replay_journaled_root`], so the decision and
/// its effects are exercised together.
enum Reopened {
    /// The root is at its recorded path: verify the marker, then roll back.
    Original(OpenedRoot),
    /// The exclusive rename happened but its journal phase did not: the root is
    /// under the intent's own quarantine leaf, so verify the marker, record the
    /// rename and resume the deletion.
    Quarantine(OpenedRoot),
    /// Neither location positively holds the recorded root; the intent stays
    /// pending.
    Missing(String),
}

/// A reopened root whose owner marker a replay can compare.
enum OpenedRoot {
    /// The verified session `resume_intent` opens in production.
    Session(RootSession),
    /// A caller-held descriptor for the same recorded root, so a unit test drives
    /// this same decision and its effects without a mount. It runs the identical
    /// [`verify_root_marker`], and it never deletes: only a verified session may
    /// resume a deletion.
    #[cfg(test)]
    Fixture(OwnedFd),
}

impl OpenedRoot {
    fn verify_marker(&self, candidate: &Candidate) -> Result<(), String> {
        match self {
            Self::Session(session) => session.verify_marker(candidate),
            #[cfg(test)]
            Self::Fixture(root) => verify_root_marker(root, candidate),
        }
    }
}

/// Decide and perform one replay of a journal that recorded no filesystem effect.
fn replay_journaled_root(
    store: &Store,
    control: &Path,
    intent: &Intent,
    candidate: &Candidate,
    reopened: Reopened,
) -> Result<Resumed, String> {
    match reopened {
        Reopened::Original(root) => {
            root.verify_marker(candidate)?;
            rollback_intent(store, control, intent)?;
            Ok(Resumed::RolledBack)
        }
        Reopened::Quarantine(root) => {
            root.verify_marker(candidate)?;
            match &root {
                OpenedRoot::Session(session) => {
                    update_intent(control, &intent.id, IntentPhase::Renamed).map_err(|error| {
                        format!("cannot record the observed quarantine rename: {error}")
                    })?;
                    resume_quarantine(store, control, intent, session)
                }
                #[cfg(test)]
                OpenedRoot::Fixture(_) => Err(
                    "a replay without a verified session never resumes a deletion; the intent stays pending"
                        .to_owned(),
                ),
            }
        }
        Reopened::Missing(reason) => Err(reason),
    }
}

/// Compare the owner marker under an opened root descriptor with the journal's
/// recorded identity: its exact bytes, its checkout and its reserved nonce and
/// generation. The device number is deliberately absent — identity was settled by
/// the reopen (recorded inode on the verified volume), and a reboot may have
/// renumbered the volume since the journal was written.
fn verify_root_marker(root: &OwnedFd, candidate: &Candidate) -> Result<(), String> {
    let name = std::ffi::CString::new(outputs::MARKER).map_err(|error| error.to_string())?;
    // SAFETY: the root descriptor and C string are live for the call.
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "the owner marker is not readable ({})",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `fd` is a newly owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read the owner marker: {error}"))?;
    if bytes.len() > 64 * 1024 || sha256_hex(&bytes) != candidate.marker_digest {
        return Err("the owner marker changed before the quarantine rename".to_owned());
    }
    let marker: Marker = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the owner marker is invalid ({error})"))?;
    // The digest comparison above already proved these are the recorded marker
    // bytes, so the same superseded-generation attribution the observation made
    // applies here: a pre-oxide#152 row's commit generation never blocks the
    // effect its own preview named.
    let superseded =
        superseded_selection_generation(candidate.generation, marker.generation(), true);
    if !marker.is_owned_by(&candidate.checkout, &candidate.checkout_id)
        || marker.nonce() != candidate.nonce.as_deref()
        || (marker.generation() != candidate.generation && !superseded)
    {
        return Err("the owner marker does not match the plan".to_owned());
    }
    Ok(())
}

fn quarantine_leaf(intent: &Intent) -> Result<String, String> {
    Ok(intent
        .quarantine
        .file_name()
        .ok_or("the intent quarantine has no leaf")?
        .to_string_lossy()
        .into_owned())
}

fn resume_quarantine(
    store: &Store,
    control: &Path,
    intent: &Intent,
    session: &RootSession,
) -> Result<Resumed, String> {
    update_intent(control, &intent.id, IntentPhase::Deleting)
        .map_err(|error| format!("cannot record the resumed delete phase: {error}"))?;
    finish_quarantine(store, control, session, &intent.id).map_err(stop_text)?;
    Ok(Resumed::Completed)
}

/// Remove an intent that provably had no filesystem effect and restore the
/// row's selection, all in one durable revision.
fn rollback_intent(_store: &Store, control: &Path, intent: &Intent) -> Result<(), String> {
    let identity = intent.identity.clone();
    // A receiptless legacy root's intent is protected by this writer's own
    // `pruning` receipt and a temporary row; both go, registry first, so the
    // checkout stays refused until the journal is gone.
    let receiptless =
        outputs::read_receipt_for_prune(control, &identity.checkout, &identity.checkout_id)?
            .is_some_and(|receipt| outputs::receipt_is_pruning(&receipt));
    {
        let _guard = store::publication_guard(control)?;
        let mut registry = ready_registry(control)?;
        registry.intents.retain(|record| record.id != intent.id);
        let matches = |row: &Row| {
            row.checkout_id == identity.checkout_id
                && row.member == identity.member
                && row.root == identity.root
        };
        if receiptless {
            registry.rows.retain(|row| {
                !(matches(row) && row.pruning.as_deref() == Some(intent.id.as_str()))
            });
        } else {
            // The group and the receipt must still be exactly what the intent
            // recorded; anything else keeps the root (pending3/KEEP).
            if !intent.prior_receipt_sha.is_empty()
                && receipt_preimage(control, &identity.checkout_id)? != intent.prior_receipt_sha
            {
                return Err(
                    "the checkout receipt changed since the prune intent; the root is kept"
                        .to_owned(),
                );
            }
            // The group, not the representative member, is the transaction
            // scope: every row of (checkout, root) is compared and restored.
            let live: Vec<&Row> = registry
                .rows
                .iter()
                .filter(|row| row.checkout_id == identity.checkout_id && row.root == identity.root)
                .collect();
            if live.len() != intent.group.len() {
                return Err(
                    "the root's registry references changed since the prune intent; the root is kept"
                        .to_owned(),
                );
            }
            for recorded in &intent.group {
                let Some(row) = live.iter().find(|row| row.member == recorded.member) else {
                    return Err("a recorded group member disappeared; the root is kept".to_owned());
                };
                if row.nonce != recorded.nonce
                    || row.generation != recorded.generation
                    || row.root_dev != recorded.root_dev
                    || row.root_ino != recorded.root_ino
                    || row.marker_digest != recorded.marker_digest
                    || row.selected
                    || row.pending
                    || row.pruning.as_deref() != Some(intent.id.as_str())
                {
                    return Err(format!(
                        "the registry reference for {} changed since the prune intent; the root is kept",
                        recorded.member
                    ));
                }
            }
            for recorded in &intent.group {
                if let Some(row) = registry.rows.iter_mut().find(|row| {
                    row.checkout_id == identity.checkout_id
                        && row.root == identity.root
                        && row.member == recorded.member
                }) {
                    row.selected = recorded.selected;
                    row.pending = false;
                    row.pruning = None;
                }
            }
        }
        registry.bump()?;
        write_registry(control, &registry)?;
    }
    if receiptless {
        outputs::discard_pruning_receipt(
            control,
            &identity.checkout,
            &identity.checkout_id,
            &intent.id,
        )?;
    }
    Ok(())
}

/// Whether the registry journals `intent`.
fn journaled(control: &Path, intent: &str) -> bool {
    read_registry(control)
        .ok()
        .flatten()
        .is_some_and(|registry| registry.intent(intent).is_some())
}

/// Writer closure for one checkout, outside or (with `under_barrier`) inside
/// its clean barrier, through the fresh per-row custody witness (oxide#235).
/// `Some(reason)` keeps the root.
fn custody_reason(
    authority: &mut AuthorityScope,
    control: &Path,
    checkout: &Path,
    under_barrier: bool,
) -> Result<Option<String>, String> {
    if under_barrier {
        return Ok(custody_clear_under_barrier(authority, control, checkout).err());
    }
    let custody = match authority.fresh_custody(control, checkout, true) {
        Ok(custody) => custody,
        Err(reason) => {
            // A witness or lock failure is unknown custody, not closure: the
            // snapshot classifies it as incomplete authority.
            return Ok(Some(format!(
                "writer custody could not be witnessed ({reason})"
            )));
        }
    };
    match custody.verdict {
        CheckoutCustody::Clear => {}
        CheckoutCustody::Writers => {
            return Ok(Some(
                "an active writer still holds this checkout".to_owned(),
            ))
        }
        CheckoutCustody::Unknown(reason) => {
            return Ok(Some(format!("writer custody is unknown ({reason})")))
        }
    }
    if let Some(holder) = custody.internal_holders.into_iter().next() {
        return Ok(Some(format!(
            "a recorded writer still holds this checkout ({holder})"
        )));
    }
    if let Some(holders) = custody.removable_holders {
        if let Some(holder) = holders.into_iter().next() {
            return Ok(Some(format!(
                "a recorded writer in the removable pool still holds this checkout ({holder})"
            )));
        }
    }
    Ok(None)
}

/// Evaluate one operator-listed root that no registry row names. Only a
/// receiptless legacy root qualifies: a real directory directly inside the
/// registered volume's `oxide-outputs`, a well-formed owner marker for that
/// volume, no receipt for its checkout (or only this writer's own
/// interrupted `pruning` creation), no journal or live reference for the
/// checkout, the checkout's actual leaf absent, every writer closed and the
/// root verified through the registered volume. Anything else keeps it.
fn evaluate_receiptless(
    store: &Store,
    control: &Path,
    registry: &Registry,
    root: &Path,
    under_barrier: bool,
    authority: &mut AuthorityScope,
) -> Evaluated {
    let mut row = Row {
        checkout_id: String::new(),
        checkout: PathBuf::new(),
        checkout_volume_uuid: None,
        receipt: PathBuf::new(),
        member: outputs::MEMBER.to_owned(),
        root: root.to_path_buf(),
        root_kind: RootKind::External,
        volume_uuid: None,
        recipe: outputs::RECIPE_PUBLIC.to_owned(),
        nonce: None,
        generation: 0,
        root_dev: None,
        root_ino: None,
        marker_digest: None,
        selected: false,
        pending: false,
        pruning: None,
        pruned_ms: None,
        reservation: None,
        cancelling: None,
    };
    let mut evidence = None;
    let outcome = receiptless_candidate(
        store,
        control,
        registry,
        root,
        under_barrier,
        authority,
        &mut row,
        &mut evidence,
    );
    match outcome {
        Ok(candidate) => Evaluated {
            row,
            candidate: Some(candidate),
            kept: None,
            legacy: true,
            evidence,
        },
        Err(reason) => Evaluated {
            row,
            candidate: None,
            kept: Some(reason),
            legacy: true,
            evidence: None,
        },
    }
}

fn receiptless_candidate(
    _store: &Store,
    control: &Path,
    registry: &Registry,
    root: &Path,
    under_barrier: bool,
    authority: &mut AuthorityScope,
    row: &mut Row,
    evidence: &mut Option<SnapshotEvidence>,
) -> Result<Candidate, String> {
    if !registry.complete {
        return Err("the registry is sticky-incomplete, so prune keeps every root".to_owned());
    }
    let registration =
        removable::read_registration(control)?.ok_or("no removable drive is registered")?;
    if root.parent()
        != Some(
            registration
                .mount_point
                .join(outputs::EXTERNAL_DIR)
                .as_path(),
        )
    {
        return Err(
            "the listed root is not directly inside the registered volume's oxide-outputs"
                .to_owned(),
        );
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("the listed root is not inspectable ({error})"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("the listed root is not a real directory".to_owned());
    }
    let marker = outputs::read_marker_for_prune(root)?;
    if marker.kind() != RootKind::External
        || marker.volume_uuid() != Some(registration.volume_uuid.as_str())
    {
        return Err("the listed root's marker is not for the registered volume".to_owned());
    }
    use std::os::unix::fs::MetadataExt;
    row.checkout_id = marker.checkout_id().to_owned();
    row.checkout = marker.checkout().to_path_buf();
    if row.checkout_id != crate::identity::digest(&row.checkout.to_string_lossy()) {
        return Err(
            "the listed root's marker checkout identity is not the digest of its checkout path"
                .to_owned(),
        );
    }
    row.receipt = outputs::receipt_path_public(control, &row.checkout_id);
    row.volume_uuid = Some(registration.volume_uuid.clone());
    // A marker that records the checkout's own volume keeps that requirement:
    // only a marker without one (the version-1 shape) may use the genuine
    // legacy boot Data rule, and a conflicting recorded volume keeps the root.
    row.checkout_volume_uuid = marker.checkout_volume_uuid().map(str::to_owned);
    row.nonce = marker.nonce().map(str::to_owned);
    row.generation = marker.generation();
    row.root_dev = Some(metadata.dev());
    row.root_ino = Some(metadata.ino());
    row.marker_digest = Some(outputs::marker_digest(root)?);
    if registry
        .unfinished_intents()
        .any(|intent| intent.identity.checkout_id == row.checkout_id)
    {
        return Err("the checkout has an unsettled prune intent".to_owned());
    }
    if registry
        .live_rows()
        .any(|other| other.checkout_id == row.checkout_id)
    {
        return Err("the checkout has a live registry reference".to_owned());
    }
    match outputs::read_receipt_for_prune(control, &row.checkout, &row.checkout_id)? {
        None => {}
        Some(receipt) if outputs::receipt_is_pruning(&receipt) => {}
        Some(_) => {
            return Err(
                "the checkout has an owned-output receipt; only receiptless listed roots are pruned this way"
                    .to_owned(),
            )
        }
    }
    match checkout_absence(row, true)? {
        Absence::Absent(witness) => {
            *evidence = Some(SnapshotEvidence {
                witness: *witness,
                legacy_exception: true,
            });
        }
        Absence::Present => return Err("the checkout is present, so its root is kept".to_owned()),
        Absence::Unknown(reason) => return Err(format!("checkout absence is unknown ({reason})")),
    }
    if let Some(reason) = custody_reason(authority, control, &row.checkout, under_barrier)? {
        return Err(reason);
    }
    let facts = match observe_row_root(row)? {
        RootState::Present(facts) => facts,
        RootState::Absent => return Err("the listed root is already absent".to_owned()),
        RootState::Unknown(reason) => return Err(reason),
    };
    Ok(Candidate {
        checkout_id: row.checkout_id.clone(),
        checkout: row.checkout.clone(),
        receipt: row.receipt.clone(),
        member: row.member.clone(),
        root: row.root.clone(),
        root_kind: RootKind::External,
        volume_uuid: row.volume_uuid.clone(),
        checkout_volume_uuid: row.checkout_volume_uuid.clone(),
        nonce: row.nonce.clone(),
        generation: row.generation,
        dev: facts.dev,
        ino: facts.ino,
        marker_digest: facts.marker_digest,
        logical_bytes: 0,
        allocated_bytes: 0,
        accounting_complete: false,
        receiptless: true,
    })
}

/// The locked recheck for a receiptless candidate: the same evaluation under
/// the checkout's barrier, with the root's identity unchanged.
fn recheck_receiptless(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    evidence: &SnapshotEvidence,
) -> Result<u64, String> {
    let mut authority = AuthorityScope::begin()?;
    let registry = ready_registry(control)?;
    registry.validate()?;
    let entry = evaluate_receiptless(
        store,
        control,
        &registry,
        &candidate.root,
        true,
        &mut authority,
    );
    let Some(current) = entry.candidate else {
        return Err(entry
            .kept
            .unwrap_or_else(|| "the listed root is no longer eligible".to_owned()));
    };
    if !same_candidates(&[candidate], &[&current]) {
        return Err("the listed root's identity changed".to_owned());
    }
    match &entry.evidence {
        Some(fresh) if fresh == evidence => {}
        Some(_) => {
            return Err(
                "the listed root's checkout ancestry changed after the real run's snapshot"
                    .to_owned(),
            )
        }
        None => return Err("the listed root's absence is no longer positively proved".to_owned()),
    }
    if !authority.complete() {
        return Err("the locked authority recheck exceeded its input or time budget".to_owned());
    }
    Ok(registry.revision)
}

fn stop_text(stop: Stop) -> String {
    match stop {
        Stop::Pending(reason) | Stop::Refuse(reason) => reason,
    }
}

/// `oxide outputs resolve-intent <id>`: settle one pending intent only when
/// both the original and quarantine leaves are positively absent under the
/// verified volume, the registry is complete, no other selection exists and no
/// writer holds the checkout. Every check runs before the first effect; the
/// checkout's clean barrier and binding lock are held from the recheck through
/// the committed terminal transaction.
const RESOLVED_OUTCOME: &str = "resolved-by-operator";

fn resolve_intent(id: &str) -> Result<u8, String> {
    if !hex32(id) {
        return Ok(refused("the intent ID must be 32 lowercase hex characters"));
    }
    let store = paired_store()?;
    let control = store.owner_control().to_path_buf();
    // Resolution never initializes or migrates anything: a pool without a
    // ready registry has no intent this verb may act on.
    let registry = match require_ready(&store, &control) {
        Ok(registry) => registry,
        Err(reason) => return Ok(refused(&reason)),
    };
    let Some(intent) = registry.intent(id).cloned() else {
        return Ok(refused("no pending intent has that ID"));
    };
    if matches!(intent.phase, IntentPhase::Deleted | IntentPhase::Resolved) {
        let deleted = intent.phase == IntentPhase::Deleted;
        if intent.receipt_settled {
            eprintln!(
                "oxide: intent {id} is already {}; nothing to resolve",
                if deleted {
                    "positively deleted"
                } else {
                    "resolved by an operator"
                }
            );
            return Ok(0);
        }
        // The registry terminal state committed but the checkout receipt's
        // did not: finish exactly that step under the checkout's custody. No
        // root effect occurs.
        let finished = under_checkout_barrier(&store, &intent.identity.checkout, || {
            settle_receipt(
                &control,
                id,
                if deleted { "deleted" } else { RESOLVED_OUTCOME },
            )
        });
        if let Err(reason) = finished {
            eprintln!(
                "oxide: resolve-intent unsettled: intent {id}'s registry terminal state committed but its checkout receipt did not ({reason}); it stays pending"
            );
            return Ok(3);
        }
        eprintln!("oxide: intent {id}'s terminal receipt committed; nothing else to resolve");
        return Ok(0);
    }
    // All refusals happen before any effect, including before the barrier.
    if let Err(reason) = resolve_precheck(&store, &control, &intent, &registry) {
        return Ok(refused(&reason));
    }
    let mut waits = store::WaitAccounting::default();
    let guard = match store.begin_clean(&intent.identity.checkout, &mut waits) {
        Ok(store::Cleaning::Guard(guard)) => guard,
        Ok(store::Cleaning::Interrupted(signal)) => {
            return Ok(refused(&format!(
                "interrupted by signal {signal} while waiting for this checkout's commands"
            )))
        }
        // `Recovering` mode never returns `Busy`; only a guarded verb's
        // `Barrier` mode does.
        Ok(store::Cleaning::Busy(reason)) => return Ok(refused(&reason)),
        // `Recovering` mode never returns `Retry`; only a guarded verb's
        // `Barrier` mode does.
        Ok(store::Cleaning::Retry(reason)) => return Ok(refused(&reason)),
        Err(reason) => return Ok(refused(&reason)),
    };
    let outcome = (|| -> Result<u8, String> {
        let _binding = store.lock_binding(&intent.identity.checkout)?;
        let current = require_ready(&store, &control)?;
        if !current.complete {
            return Ok(refused(
                "the registry became sticky-incomplete; the intent stays pending",
            ));
        }
        let Some(record) = current.intent(id) else {
            return Ok(refused("the intent disappeared from the registry"));
        };
        if record.phase != intent.phase || record.identity != intent.identity {
            return Ok(refused(
                "the intent changed while its custody was being acquired",
            ));
        }
        if let Err(reason) = resolve_precheck(&store, &control, &intent, &current) {
            return Ok(refused(&reason));
        }
        // First effect: the durable resolution proof.
        let resolution = serde_json::json!({
            "version": RESOLUTION_VERSION,
            "intent": intent.id,
            "root": intent.identity.root,
            "checkout": intent.identity.checkout,
            "member": intent.identity.member,
            "nonce": intent.identity.nonce,
            "volume_uuid": intent.identity.volume_uuid,
            "dev": intent.identity.dev,
            "ino": intent.identity.ino,
            "marker_digest": intent.identity.marker_digest,
            "resolved_ms": process::now_ms(),
            "binary": std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_default(),
            "package_version": env!("CARGO_PKG_VERSION"),
            "registry_revision": current.revision,
            "coverage": {
                "registry_rows": current.rows.len(),
                "complete": current.complete,
                "source": "internal output registry snapshot",
            },
            "checks": {
                "original_absent": "fstatat(AT_SYMLINK_NOFOLLOW) = ENOENT",
                "quarantine_absent": "fstatat(AT_SYMLINK_NOFOLLOW) = ENOENT",
                "no_other_selection": true,
                "writer_custody": "clear",
                "volume": "verified UUID, device and mount",
            }
        });
        let directory = registry_dir(&control).join("prune-resolutions");
        if let Err(reason) = ensure_private_dir(&directory) {
            return Ok(refused(&format!(
                "cannot prepare the resolution directory ({reason})"
            )));
        }
        let path = directory.join(format!("{}.json", intent.id));
        let bytes = match serde_json::to_vec_pretty(&resolution) {
            Ok(bytes) => bytes,
            Err(error) => return Ok(refused(&format!("cannot encode the resolution ({error})"))),
        };
        let temp = directory.join(format!(".{}.tmp-{}", intent.id, std::process::id()));
        if let Err(reason) = write_private(&temp, &bytes) {
            return Ok(refused(&format!(
                "cannot prepare the resolution proof ({reason})"
            )));
        }
        if let Err(error) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Ok(refused(&format!(
                "cannot publish the resolution proof ({error})"
            )));
        }
        if let Err(error) = sync_directory(&directory) {
            eprintln!(
                "oxide: resolve-intent unsettled: the resolution proof is published but its directory did not sync ({error}); the intent stays pending"
            );
            return Ok(3);
        }
        if let Err(reason) = complete_resolution(&store, &control, &intent, RESOLVED_OUTCOME) {
            eprintln!(
                "oxide: resolve-intent unsettled: {reason}; the resolution proof is published but the terminal transaction did not commit, so the intent stays pending"
            );
            return Ok(3);
        }
        println!(
            "outputs resolve-intent: {} resolved by operator (original and quarantine leaves absent); resolution receipt {}",
            intent.id,
            path.display()
        );
        Ok(0)
    })();
    match store.abandon_clean(guard) {
        Ok(()) => outcome,
        Err(error) => {
            eprintln!(
                "oxide: resolve-intent unsettled: cannot release the checkout barrier ({error}); inspect the clean mark and the registry"
            );
            Ok(3)
        }
    }
}

/// Every positive gate resolution needs, in one place so the pre-effect check
/// and the under-custody recheck are identical.
fn resolve_precheck(
    _store: &Store,
    control: &Path,
    intent: &Intent,
    registry: &Registry,
) -> Result<(), String> {
    if !registry.complete {
        return Err(
            "the output registry is sticky-incomplete, so no intent can be resolved".to_owned(),
        );
    }
    if registry.intent(&intent.id).is_none() {
        return Err("the registry no longer records this intent".to_owned());
    }
    if registry
        .intents
        .iter()
        .any(|other| other.id != intent.id && other.identity.root == intent.identity.root)
    {
        return Err("another intent references the same root; settle it first".to_owned());
    }
    let Some(row) = registry.rows.iter().find(|row| {
        row.checkout_id == intent.identity.checkout_id
            && row.member == intent.identity.member
            && row.root == intent.identity.root
    }) else {
        return Err("the registry no longer holds this intent's blocking row".to_owned());
    };
    if row.pruning.as_deref() != Some(intent.id.as_str()) {
        return Err("the intent's blocking registry reference changed".to_owned());
    }
    if registry.rows.iter().any(|other| {
        other.root == intent.identity.root
            && other.checkout_id != intent.identity.checkout_id
            && other.live()
    }) {
        return Err("another live registry reference still names this root".to_owned());
    }
    if registry.rows.iter().any(|other| {
        other.checkout_id == intent.identity.checkout_id
            && other.member == intent.identity.member
            && other.root != intent.identity.root
            && other.selected
    }) {
        return Err("the checkout selects another root; the intent stays pending".to_owned());
    }
    let Some(uuid) = intent.identity.volume_uuid.as_deref() else {
        return Err("the intent records no volume UUID, so absence cannot be proven".to_owned());
    };
    let Some(outputs_dir) = intent.identity.root.parent() else {
        return Err("the intent root has no parent".to_owned());
    };
    let Some(mount) = outputs_dir.parent() else {
        return Err("the intent root has no mount".to_owned());
    };
    let binding = Binding::open(mount, Some(uuid)).map_err(|error| {
        format!(
            "the recorded volume is not mounted and matching ({error}); the intent stays pending"
        )
    })?;
    let outputs = binding
        .dir_at(binding.fd(), outputs::EXTERNAL_DIR, false)
        .map_err(|error| format!("the outputs directory is not readable ({error})"))?;
    let original = intent
        .identity
        .root
        .file_name()
        .ok_or("the intent root has no leaf")?;
    let quarantine = intent
        .quarantine
        .file_name()
        .ok_or("the intent quarantine has no leaf")?;
    for leaf in [original, quarantine] {
        let name = std::ffi::CString::new(leaf.as_bytes())
            .map_err(|_| "an intent leaf contains a NUL".to_owned())?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor and C string are live for the call.
        let status = unsafe {
            libc::fstatat(
                outputs.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status == 0 {
            return Err(format!(
                "the leaf {} is present; both leaves must be positively absent",
                leaf.to_string_lossy()
            ));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!(
                "cannot prove {} absent ({error}); the intent stays pending",
                leaf.to_string_lossy()
            ));
        }
    }
    // No authority window covers this settlement: one witnessed custody
    // decision, then the witness drops with the barrier still held.
    let mut witness = store::CustodyWitness::open(control)
        .map_err(|reason| format!("writer custody could not be witnessed ({reason})"))?;
    let custody = witness
        .decide(None, &intent.identity.checkout, false)
        .map_err(|reason| format!("writer custody could not be witnessed ({reason})"))?;
    custody_under_barrier_refusal(custody)
}

/// Writer closure for a checkout whose clean barrier and binding lock the
/// caller holds: cooperative custody in both pools is clear and neither
/// pool's witnessed state records an active command for it (oxide#235). The
/// caller's own barrier is the only one `begin_clean` admits, so barriers are
/// not counted. Any witness, lock or state failure is not closure.
fn custody_clear_under_barrier(
    authority: &mut AuthorityScope,
    control: &Path,
    checkout: &Path,
) -> Result<(), String> {
    let custody = authority
        .fresh_custody(control, checkout, false)
        .map_err(|reason| format!("writer custody could not be witnessed ({reason})"))?;
    custody_under_barrier_refusal(custody)
}

/// Map one fresh custody decision onto the under-barrier refusal contract:
/// the caller holds the checkout's clean barrier, so recorded barriers are
/// not counted, and any holder or unknown writer refuses.
fn custody_under_barrier_refusal(custody: store::RowCustody) -> Result<(), String> {
    match custody.verdict {
        CheckoutCustody::Clear => {}
        CheckoutCustody::Writers => {
            return Err("an active writer still holds the checkout".to_owned())
        }
        CheckoutCustody::Unknown(reason) => {
            return Err(format!("writer custody is unknown ({reason})"))
        }
    }
    if let Some(holder) = custody.internal_holders.into_iter().next() {
        return Err(format!(
            "a recorded writer still holds this checkout ({holder})"
        ));
    }
    if let Some(holders) = custody.removable_holders {
        if let Some(holder) = holders.into_iter().next() {
            return Err(format!(
                "a recorded writer in the removable pool still holds this checkout ({holder})"
            ));
        }
    }
    Ok(())
}

fn complete_resolution(
    _store: &Store,
    control: &Path,
    intent: &Intent,
    outcome: &str,
) -> Result<(), String> {
    {
        let _guard = store::publication_guard(control)?;
        let mut registry = ready_registry(control)?;
        if let Some(record) = registry
            .intents
            .iter_mut()
            .find(|record| record.id == intent.id)
        {
            record.phase = IntentPhase::Resolved;
            record.resolved_ms = Some(process::now_ms());
            record.updated_ms = process::now_ms();
        }
        let blocking: Vec<usize> = registry
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.checkout_id == intent.identity.checkout_id
                    && row.root == intent.identity.root
                    && row.pruning.as_deref() == Some(intent.id.as_str())
            })
            .map(|(index, _)| index)
            .collect();
        let expected = if intent.group.is_empty() {
            1
        } else {
            intent.group.len()
        };
        if blocking.len() != expected {
            return Err("the intent's blocking registry references changed".to_owned());
        }
        let now = process::now_ms();
        for index in blocking {
            let row = &mut registry.rows[index];
            row.selected = false;
            row.pending = false;
            row.pruning = None;
            row.pruned_ms = Some(now);
        }
        registry.bump()?;
        write_registry(control, &registry)?;
    }
    settle_receipt(control, &intent.id, outcome)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Test-only seeding for the outputs fixtures: a ready, empty registry with a
/// fresh transaction identity. Product code never creates one this way.
#[cfg(test)]
pub(crate) fn seed_test_registry(control: &Path) -> Result<(), String> {
    if read_registry(control)?.is_some() {
        return Ok(());
    }
    ensure_private_dir(&registry_dir(control))?;
    let transaction = fresh_nonce()?;
    match create_or_validate_registry(control, &transaction, true) {
        Ok(()) => Ok(()),
        // Another test thread won the exclusive creation.
        Err(_) if read_registry(control)?.is_some() => Ok(()),
        Err(reason) => Err(reason),
    }
}

/// Test-only: publish one prune intent for `checkout_id` into a ready
/// registry, so the guarded branch's unsettled-intent refusal (and the shipped
/// definition of a *settled* intent) is exercisable without running a real
/// prune. Returns the intent ID.
#[cfg(test)]
pub(crate) fn seed_test_intent(control: &Path, checkout_id: &str, settled: bool) -> String {
    let mut registry = ready_registry(control).unwrap();
    let id = "b".repeat(32);
    registry.intents.push(Intent {
        version: INTENT_VERSION.to_owned(),
        id: id.clone(),
        identity: RootIdentity {
            checkout_id: checkout_id.to_owned(),
            checkout: PathBuf::from("/checkout"),
            receipt: control.join("outputs").join(format!("{checkout_id}.json")),
            member: "debug".to_owned(),
            root: PathBuf::from("/root"),
            root_kind: RootKind::External,
            volume_uuid: None,
            nonce: Some("a".repeat(32)),
            generation: 1,
            dev: 1,
            ino: 2,
            marker_digest: "c".repeat(64),
        },
        phase: if settled {
            IntentPhase::Resolved
        } else {
            IntentPhase::Intent
        },
        quarantine: PathBuf::from("/quarantine"),
        preview_sha256: "f".repeat(64),
        created_ms: 1,
        updated_ms: 1,
        resolved_ms: None,
        receipt_settled: settled,
        was_selected: true,
        prior_receipt_sha: String::new(),
        group: Vec::new(),
        expected_terminal_sha: None,
        terminal_ms: None,
    });
    registry.bump().unwrap();
    write_registry(control, &registry).unwrap();
    id
}

/// Test-only view of registry rows: root, selected, pending, pruning.
#[cfg(test)]
pub(crate) fn test_rows(
    control: &Path,
) -> Result<Vec<(PathBuf, bool, bool, Option<String>)>, String> {
    let registry = ready_registry(control)?;
    Ok(registry
        .rows
        .iter()
        .map(|row| {
            (
                row.root.clone(),
                row.selected,
                row.pending,
                row.pruning.clone(),
            )
        })
        .collect())
}

/// The prune capture harness's entry into the actual decision chain (X-S1,
/// barge-adoption.md). Compiled only under `cfg(test)`. It follows the
/// `dry_run` decision prefix exactly — `AuthorityScope::begin`, then
/// `paired_store`, then the optional `read_operator_list`, then
/// `require_ready`, then the unfinished-intent refusal through `unsettled`,
/// then `evaluate` — and exports every actual `Evaluation` entry and the
/// completeness flag, or the actual pre-evaluate refusal. It never calls
/// `dry_run` or `build_preview`, renders no preview, and never redirects
/// stdout; the real evaluator, its accounting walk and every read stay
/// unchanged.
#[cfg(test)]
pub(crate) mod tape_entry {
    use super::{
        evaluate, paired_store, phase_label, read_operator_list, require_ready, unsettled,
        AuthorityScope, Evaluation,
    };
    use std::path::Path;

    /// A pre-evaluate refusal, in the engine's own vocabulary and exit code.
    pub(crate) struct ChainRefusal {
        pub(crate) reason: String,
        pub(crate) exit_code: u8,
        pub(crate) unsettled: bool,
    }

    /// One completed chain run: the full bounded actual evaluation export, or
    /// the actual pre-evaluate refusal. An `Err` from the chain itself is the
    /// engine's infrastructure refusal (exit 1, as `prune::run` maps it).
    pub(crate) enum ChainOutcome {
        Evaluated { result: serde_json::Value },
        Refused(ChainRefusal),
    }

    fn export_entry(entry: &super::Evaluated) -> serde_json::Value {
        let row = &entry.row;
        let candidate = entry.candidate.as_ref().map(|candidate| {
            serde_json::json!({
                "root": candidate.root.display().to_string(),
                "dev": candidate.dev,
                "ino": candidate.ino,
                "marker_digest": candidate.marker_digest,
                "logical_bytes": candidate.logical_bytes,
                "allocated_bytes": candidate.allocated_bytes,
                "accounting_complete": candidate.accounting_complete,
                "receiptless": candidate.receiptless,
                "volume_uuid": candidate.volume_uuid,
                "checkout_volume_uuid": candidate.checkout_volume_uuid,
                "nonce": candidate.nonce,
                "generation": candidate.generation,
                "root_kind": candidate.root_kind,
            })
        });
        serde_json::json!({
            "checkout_id": row.checkout_id,
            "checkout": row.checkout.display().to_string(),
            "checkout_volume_uuid": row.checkout_volume_uuid,
            "receipt": row.receipt.display().to_string(),
            "member": row.member,
            "root": row.root.display().to_string(),
            "root_kind": row.root_kind,
            "volume_uuid": row.volume_uuid,
            "generation": row.generation,
            "nonce": row.nonce,
            "selected": row.selected,
            "pending": row.pending,
            "pruning": row.pruning,
            "reservation": row.reservation,
            "cancelling": row.cancelling,
            "candidate": candidate,
            "kept": entry.kept,
            "legacy": entry.legacy,
        })
    }

    fn export_evaluation(evaluated: &Evaluation) -> serde_json::Value {
        serde_json::json!({
            "complete": evaluated.complete,
            "entries": evaluated
                .entries
                .iter()
                .map(export_entry)
                .collect::<Vec<_>>(),
        })
    }

    /// The actual decision chain, in `dry_run`'s exact order, exporting its
    /// real result. Nothing here feeds anything back into the engine.
    pub(crate) fn run_chain(operator_list: Option<&Path>) -> Result<ChainOutcome, String> {
        let mut authority = AuthorityScope::begin()?;
        let store = paired_store()?;
        let control = store.owner_control().to_path_buf();
        let list = match operator_list {
            Some(path) => Some(read_operator_list(path)?),
            None => None,
        };
        // A dry run mutates nothing: it never initializes, replays or
        // reconciles. The refusal branches below are `dry_run`'s own.
        let registry = match require_ready(&store, &control) {
            Ok(registry) => registry,
            Err(reason) => {
                return Ok(ChainOutcome::Refused(ChainRefusal {
                    reason,
                    exit_code: 2,
                    unsettled: false,
                }))
            }
        };
        if let Some(intent) = registry.unfinished_intents().next() {
            let reason = format!(
                "prune intent {} is unfinished ({}); settle it with `oxide outputs resolve-intent {}` or restore the recorded root before pruning",
                intent.id,
                phase_label(intent.phase),
                intent.id
            );
            let exit_code = unsettled(&control, &reason);
            return Ok(ChainOutcome::Refused(ChainRefusal {
                reason,
                exit_code,
                unsettled: true,
            }));
        }
        let evaluated = evaluate(&store, &control, &registry, list.as_ref(), &mut authority)?;
        drop(authority);
        Ok(ChainOutcome::Evaluated {
            result: export_evaluation(&evaluated),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pinned Barge reference reader must preserve every live registry flag.
    /// This compares only the in-memory reference predicates, not release plans,
    /// filesystem capture, writer custody, or parity of the two evaluators.
    #[test]
    fn shadow_reference_flags_and_incomplete_registry() {
        use barge::reference::{OxideReceipts, OxideRegistry, OxideRow, OxideSnapshot};

        let base = Path::new("/fixture");
        for mask in 0..32 {
            let mut oxide = row_for(base);
            oxide.selected = mask & 1 != 0;
            oxide.pending = mask & 2 != 0;
            oxide.pruning = (mask & 4 != 0).then(|| "intent".to_owned());
            oxide.reservation = (mask & 8 != 0).then(|| "reservation".to_owned());
            oxide.cancelling = (mask & 16 != 0).then(|| "cancelling".to_owned());
            let barge = OxideRow {
                checkout_id: oxide.checkout_id.clone(),
                checkout: oxide.checkout.clone(),
                checkout_volume: None,
                root: oxide.root.clone(),
                nonce: oxide.nonce.clone(),
                selected: oxide.selected,
                pending: oxide.pending,
                pruning: oxide.pruning.is_some(),
                reservation: oxide.reservation.is_some(),
                cancelling: oxide.cancelling.is_some(),
            };
            assert_eq!(oxide.live(), barge.live(), "flag mask {mask}");
            let snapshot = OxideSnapshot {
                control: None,
                registry: OxideRegistry::Read {
                    rows: vec![barge],
                    intents: Vec::new(),
                    complete: true,
                },
                receipts: OxideReceipts::Absent,
                detail: None,
                custody: Default::default(),
            };
            let references = snapshot.references_for(&oxide.root, "fixture/root", None, None);
            assert!(references.complete, "flag mask {mask}");
            assert_eq!(references.rows.len(), 1, "flag mask {mask}");
            assert_eq!(references.rows[0].current, oxide.live(), "flag mask {mask}");
            assert_eq!(references.absence.len(), usize::from(!oxide.live()));
        }

        let snapshot = OxideSnapshot {
            control: None,
            registry: OxideRegistry::Read {
                rows: Vec::new(),
                intents: Vec::new(),
                complete: false,
            },
            receipts: OxideReceipts::Absent,
            detail: None,
            custody: Default::default(),
        };
        assert!(!snapshot.complete());
        assert!(
            !snapshot
                .references_for(base, "fixture", None, None)
                .complete
        );
    }

    /// oxide#211 (decision 7aedd5af): a prune test hook arms on the internal
    /// volume, or under a DECLARED scratch pool inside the supervisor scratch.
    /// Every other placement stays inert, including a declaration equal to the
    /// scratch root, a control outside the declared pool and a prefix lookalike.
    #[cfg(debug_assertions)]
    #[test]
    fn prune_hook_placement_is_internal_or_a_declared_scratch_pool() {
        let scratch = Path::new("/Volumes/Builds/scratch/oxide/run/scratch");
        let declared = scratch.join("scratch-pools");
        let control = declared.join("pool/oxide-managed");
        assert!(hook_placement_allowed(scratch, Some(&declared), &control));
        assert!(hook_placement_allowed(
            Path::new("/private/tmp/run/scratch"),
            None,
            Path::new("/private/tmp/run/scratch/pool/oxide-managed"),
        ));
        assert!(!hook_placement_allowed(scratch, None, &control));
        assert!(!hook_placement_allowed(scratch, Some(scratch), &control));
        assert!(!hook_placement_allowed(
            scratch,
            Some(Path::new("/Volumes/Builds/other/scratch-pools")),
            &control,
        ));
        assert!(!hook_placement_allowed(
            scratch,
            Some(&declared),
            &scratch.join("checkouts/pool/oxide-managed"),
        ));
        assert!(!hook_placement_allowed(scratch, Some(&declared), &declared));
        assert!(!hook_placement_allowed(
            scratch,
            Some(&declared),
            &scratch.join("scratch-pools-evil/pool/oxide-managed"),
        ));

        // The control-directory conditions: the canonical strict child passes;
        // a symlinked or non-canonical spelling, the scratch root itself, a
        // task-root sibling and an outside path all refuse.
        assert!(hook_control_allowed(
            &control,
            &control,
            scratch,
            Some(&declared)
        ));
        let real = declared.join("pool/real");
        let link = declared.join("pool/link");
        assert!(
            !hook_control_allowed(&link, &real, scratch, Some(&declared)),
            "symlinked control"
        );
        assert!(
            !hook_control_allowed(
                &declared.join("pool/../pool/oxide-managed"),
                &control,
                scratch,
                Some(&declared),
            ),
            "non-canonical control"
        );
        assert!(
            !hook_control_allowed(scratch, scratch, scratch, Some(&declared)),
            "scratch root itself"
        );
        let sibling_declared = Path::new("/Volumes/Builds/scratch/oxide/run/sibling/scratch-pools");
        let sibling_control = sibling_declared.join("pool/oxide-managed");
        assert!(
            !hook_control_allowed(
                &sibling_control,
                &sibling_control,
                scratch,
                Some(sibling_declared),
            ),
            "task-root sibling declaration"
        );
        let outside = Path::new("/Volumes/Builds/other/pool/oxide-managed");
        assert!(
            !hook_control_allowed(outside, outside, scratch, None),
            "outside path"
        );

        // The shared gate (`proof_evidence`), routed by the EACCES, low-reserve and
        // control hooks alike: the declaration must be a canonical strict child of
        // the scratch. `injected_low_reserve` has no control path, so its accepted
        // and refused cases are decided here by the declaration alone.
        let lookalike = Path::new("/Volumes/Builds/scratch/oxide/run/scratch-evil/scratch-pools");
        assert!(
            declaration_allowed(scratch, Some(&declared), Some(&declared)),
            "low-reserve: declared strict child"
        );
        assert!(
            declaration_allowed(Path::new("/private/tmp/run/scratch"), None, None),
            "internal, no declaration"
        );
        assert!(
            !declaration_allowed(scratch, None, None),
            "low-reserve: no declaration on the selected volume"
        );
        assert!(
            !declaration_allowed(scratch, Some(scratch), Some(scratch)),
            "low-reserve: declaration is the scratch root"
        );
        assert!(
            !declaration_allowed(scratch, Some(sibling_declared), Some(sibling_declared)),
            "low-reserve: task-root sibling"
        );
        assert!(
            !declaration_allowed(scratch, Some(&link), Some(&real)),
            "low-reserve: symlinked declaration"
        );
        assert!(
            !declaration_allowed(
                scratch,
                Some(&scratch.join("scratch-pools/../scratch-pools")),
                Some(&declared),
            ),
            "low-reserve: non-canonical declaration"
        );
        assert!(
            !declaration_allowed(scratch, Some(outside), Some(outside)),
            "low-reserve: outside declaration"
        );
        assert!(
            !declaration_allowed(scratch, Some(&lookalike), Some(&lookalike)),
            "low-reserve: prefix lookalike"
        );
    }

    /// Release builds carry no low-reserve fixture: the stub never reads a reserve.
    #[cfg(not(debug_assertions))]
    #[test]
    fn prune_low_reserve_hook_is_absent_in_release() {
        assert!(!injected_low_reserve(0, 0));
        assert!(!injected_low_reserve(u64::MAX, u64::MAX));
    }

    /// Release builds carry none of the prune fixture hooks: the stub never fires.
    #[cfg(not(debug_assertions))]
    #[test]
    fn prune_fixture_hooks_are_absent_in_release() {
        let path =
            Path::new("/Volumes/Builds/scratch/oxide/run/scratch/scratch-pools/pool/oxide-managed");
        assert!(!prune_fixture_fault(
            path,
            "OXIDE_TEST_PRUNE_DIAGNOSTIC_DELAY",
            path,
            "prune-diagnostic-delay-fired",
        ));
    }

    /// oxide#155: a Finder-metadata race that did not remove the quarantine is
    /// a pending intent for every caller, never a completed deletion. The
    /// journal phases are not rewritten to recover it, and a name that is
    /// missing is not positive deletion proof.
    /// oxide#155: the low-reserve fault modes are decided purely, and the
    /// recovered-metadata mode keys on the seam's own work rather than on the
    /// pass's total. The counterfactual is the old total-count trigger: with
    /// nine ordinary deletions and no recovered entry it stays disarmed, so a
    /// recovered-mode proof cannot be satisfied by ordinary delete work.
    #[test]
    fn the_low_reserve_modes_key_on_their_own_counter() {
        // Recovered mode: disarmed by ordinary deletions, armed by one recovery.
        assert_eq!(
            low_reserve_decision(5, 9, 0, None, None, Some(1)),
            None,
            "ordinary deletions must not satisfy the recovered mode"
        );
        assert_eq!(
            low_reserve_decision(5, 9, 1, None, None, Some(1)),
            Some("recovered")
        );
        // The check mode and the older total-count mode keep their meanings.
        assert_eq!(
            low_reserve_decision(3, 0, 0, Some(3), None, None),
            Some("check")
        );
        assert_eq!(
            low_reserve_decision(1, 2, 0, None, Some(2), None),
            Some("entries")
        );
        // One mode at a time; a combined or absent set disarms.
        assert_eq!(low_reserve_decision(1, 1, 1, Some(1), Some(1), None), None);
        assert_eq!(low_reserve_decision(1, 1, 1, None, None, None), None);
    }

    #[test]
    fn every_terminal_finder_race_outcome_keeps_the_intent_pending() {
        let pending = |outcome: Rmdir| race_pending("the quarantine leaf", &outcome);
        let refused = pending(Rmdir::Refused {
            entry: "stray\\x07".to_owned(),
        });
        assert!(refused.contains("stray\\x07"), "{refused}");
        assert!(pending(Rmdir::Replaced {
            entry: "leaf".to_owned()
        })
        .contains("no longer the verified directory"));
        assert!(!pending(Rmdir::RefusedMany { entries: 65 }).is_empty());
        assert!(!pending(Rmdir::Exhausted { entries: 6 }).is_empty());
        assert!(!pending(Rmdir::Failed {
            error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        })
        .is_empty());
        // Absence is uncertainty, never completion: a missing quarantine leaf
        // and a removal this seam did not positively make both stay pending.
        assert!(!pending(Rmdir::Vanished).is_empty());
        // A mid-drain caller stop carries its own reason and stays pending.
        assert!(pending(Rmdir::Stopped {
            reason: "the registered volume's reserve was reached".to_owned()
        })
        .contains("reserve"));
        assert!(!pending(Rmdir::Removed).is_empty());
        assert!(!pending(Rmdir::Drained { entries: 1 }).is_empty());
    }
    use std::sync::atomic::{AtomicU64, Ordering};

    /// oxide#155: the prune traversal deletes only what it captured and
    /// verified. These cases drive the production pass (`delete_contents`) over
    /// a plain fixture directory and inject one replacement at the traversal's
    /// own effect boundary, so a name the traversal merely listed is never the
    /// thing it acts on.
    ///
    /// The pass is the step `finish_quarantine` runs after it records the
    /// intent as `deleting` and before it writes `marker_last`: a pending stop
    /// here is returned to the caller, so the intent phase keeps its recorded
    /// value and the root stays pending rather than completing.
    fn delete_fixture(root: &Path) -> (Result<(), Stop>, PassCounts) {
        let dir = fs::File::open(root).expect("fixture directory must open");
        let mut deleter = Deleter::fixture(root);
        let outcome = delete_contents(&mut deleter, dir.as_fd(), false);
        let counts = PassCounts {
            entries: deleter.entries,
            recovered: deleter.recovered,
        };
        (outcome, counts)
    }

    /// The counts one pass reported: ordinary deletions and recovered Finder
    /// metadata, kept apart exactly as the production deleter keeps them.
    struct PassCounts {
        entries: u64,
        recovered: u64,
    }

    fn pass_reason(outcome: &Result<(), Stop>) -> String {
        match outcome {
            Ok(()) => String::new(),
            Err(stop) => stop_text(match stop {
                Stop::Pending(reason) | Stop::Refuse(reason) => Stop::Pending(reason.clone()),
            }),
        }
    }

    fn fixture_dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        path
    }

    fn names_in(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The ordinary pass is unchanged: every entry of the quarantine goes,
    /// including a tolerated-metadata name at this level (it is verified like
    /// any other owned entry here) and a nested tree.
    #[test]
    fn an_ordinary_quarantined_tree_is_deleted_through_verified_captures() {
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("owned.txt"), b"owned").unwrap();
        fs::write(root.join(".DS_Store"), b"metadata").unwrap();
        let nested = fixture_dir(&root, "nested");
        fs::write(nested.join("deep.txt"), b"deep").unwrap();
        let (outcome, deleter) = delete_fixture(&root);
        assert!(outcome.is_ok(), "{}", pass_reason(&outcome));
        assert!(names_in(&root).is_empty());
        assert_eq!(deleter.entries, 4);
    }

    /// A regular entry replaced after it was observed is not removed: the
    /// capture is not the observed object, so the pass puts it back and stops
    /// pending, and the racer's object is exactly where it left it. The old
    /// list-then-unlink parent would have unlinked the replacement, which is
    /// what the surviving inode falsifies.
    #[test]
    fn a_replacement_after_the_observation_is_not_removed() {
        use std::cell::Cell;
        use std::os::unix::fs::MetadataExt;
        use std::rc::Rc;
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let target = root.join("owned.txt");
        fs::write(&target, b"owned").unwrap();
        let fired = Rc::new(Cell::new(false));
        let replaced = Rc::new(Cell::new(0));
        let (fired_in_arm, replaced_in_arm, target_in_arm) =
            (Rc::clone(&fired), Rc::clone(&replaced), target.clone());
        let _armed =
            test_replace_arm::arm(test_replace_arm::Phase::Observed, "owned.txt", move || {
                fs::remove_file(&target_in_arm).unwrap();
                fs::write(&target_in_arm, b"the racer's object").unwrap();
                replaced_in_arm.set(fs::metadata(&target_in_arm).unwrap().ino());
                fired_in_arm.set(true);
            });
        let (outcome, deleter) = delete_fixture(&root);
        assert!(fired.get(), "the replacement injection never fired");
        let reason = pass_reason(&outcome);
        assert!(reason.contains("is not the observed"), "{reason}");
        assert!(reason.contains("nothing was deleted"), "{reason}");
        assert_eq!(fs::read(&target).unwrap(), b"the racer's object");
        // Exactly the racer's object survives under its name: a traversal that
        // unlinked the listed name would have lost this inode.
        assert_ne!(replaced.get(), 0, "the injection did not record its object");
        assert_eq!(fs::metadata(&target).unwrap().ino(), replaced.get());
        assert_eq!(names_in(&root), vec!["owned.txt".to_owned()]);
        assert_eq!(deleter.entries, 0);
    }

    /// A directory replaced by a symbolic link after the observation is not
    /// followed, captured as a directory, recursed into or moved into: the
    /// capture fails verification on its type, goes back, and the link and its
    /// referent are untouched.
    #[test]
    fn a_directory_replaced_by_a_link_is_not_followed() {
        use std::cell::Cell;
        use std::rc::Rc;
        let fixture = Fixture::new();
        let (root, outside) = (
            fixture.root.join("quarantine"),
            fixture.root.join("outside"),
        );
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        let sub = fixture_dir(&root, "sub");
        fs::write(sub.join("owned.txt"), b"owned").unwrap();
        let fired = Rc::new(Cell::new(false));
        let (fired_in_arm, sub_in_arm, outside_in_arm) =
            (Rc::clone(&fired), sub.clone(), outside.clone());
        let _armed = test_replace_arm::arm(test_replace_arm::Phase::Observed, "sub", move || {
            fs::remove_dir_all(&sub_in_arm).unwrap();
            std::os::unix::fs::symlink(&outside_in_arm, &sub_in_arm).unwrap();
            fired_in_arm.set(true);
        });
        let (outcome, _deleter) = delete_fixture(&root);
        assert!(fired.get(), "the replacement injection never fired");
        let reason = pass_reason(&outcome);
        assert!(reason.contains("is not the observed"), "{reason}");
        assert!(fs::symlink_metadata(&sub).unwrap().is_symlink());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }

    /// A directory replaced at the descend boundary is not descended into: the
    /// pass removes the object it captured and verified, and the racer's new
    /// directory keeps its own content. The old traversal opened the listed
    /// name and would have deleted that content.
    #[test]
    fn a_directory_replaced_before_the_descent_is_not_descended_into() {
        use std::cell::Cell;
        use std::rc::Rc;
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let sub = fixture_dir(&root, "sub");
        fs::write(sub.join("owned.txt"), b"owned").unwrap();
        let fired = Rc::new(Cell::new(false));
        let (fired_in_arm, root_in_arm) = (Rc::clone(&fired), root.clone());
        let _armed = test_replace_arm::arm(test_replace_arm::Phase::Descending, "sub", move || {
            let replacement = root_in_arm.join("sub");
            fs::create_dir(&replacement).unwrap();
            fs::write(replacement.join("racer.txt"), b"racer").unwrap();
            fired_in_arm.set(true);
        });
        let (outcome, _deleter) = delete_fixture(&root);
        assert!(fired.get(), "the replacement injection never fired");
        assert!(outcome.is_ok(), "{}", pass_reason(&outcome));
        // The verified capture was removed; the racer's replacement is the one
        // name left, with its own content intact.
        assert_eq!(names_in(&root), vec!["sub".to_owned()]);
        assert_eq!(fs::read(sub.join("racer.txt")).unwrap(), b"racer");
        assert_eq!(fs::read_dir(&sub).unwrap().count(), 1);
    }

    /// Finder metadata recreated inside a directory after its content was
    /// deleted and before the directory's own removal is drained under the
    /// shared fixed bound, so the directory completes.
    #[test]
    fn finder_metadata_recreated_before_the_rmdir_is_drained() {
        use std::cell::Cell;
        use std::rc::Rc;
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let sub = fixture_dir(&root, "sub");
        fs::write(sub.join("owned.txt"), b"owned").unwrap();
        let fired = Rc::new(Cell::new(false));
        let (fired_in_arm, root_in_arm) = (Rc::clone(&fired), root.clone());
        let _armed =
            test_replace_arm::arm(test_replace_arm::Phase::BeforeRmdir, "sub", move || {
                // The captured directory is the only private capture name in
                // the quarantine at this boundary.
                let captured = fs::read_dir(&root_in_arm)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name().is_some_and(|name| {
                            name.to_string_lossy().starts_with(".oxide-capture-")
                        })
                    })
                    .expect("the captured directory must be visible under its private name");
                fs::write(captured.join(".DS_Store"), b"metadata").unwrap();
                fs::write(captured.join("._owned.txt"), b"metadata").unwrap();
                fired_in_arm.set(true);
            });
        let (outcome, deleter) = delete_fixture(&root);
        assert!(fired.get(), "the metadata injection never fired");
        assert!(outcome.is_ok(), "{}", pass_reason(&outcome));
        assert!(names_in(&root).is_empty());
        // The two recovered entries are counted as recovered work, not as
        // ordinary deletions of this pass.
        assert_eq!(deleter.recovered, 2);
    }

    /// A foreign entry recreated at the same boundary is refused by the shared
    /// seam: the directory goes back under its original name with the foreign
    /// file untouched, and the pass stops pending.
    #[test]
    fn a_foreign_entry_before_the_rmdir_is_refused_and_untouched() {
        use std::cell::Cell;
        use std::rc::Rc;
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let sub = fixture_dir(&root, "sub");
        fs::write(sub.join("owned.txt"), b"owned").unwrap();
        let fired = Rc::new(Cell::new(false));
        let (fired_in_arm, root_in_arm) = (Rc::clone(&fired), root.clone());
        let _armed =
            test_replace_arm::arm(test_replace_arm::Phase::BeforeRmdir, "sub", move || {
                let captured = fs::read_dir(&root_in_arm)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name().is_some_and(|name| {
                            name.to_string_lossy().starts_with(".oxide-capture-")
                        })
                    })
                    .expect("the captured directory must be visible under its private name");
                fs::write(captured.join("foreign.txt"), b"foreign").unwrap();
                fired_in_arm.set(true);
            });
        let (outcome, _deleter) = delete_fixture(&root);
        assert!(fired.get(), "the foreign injection never fired");
        let reason = pass_reason(&outcome);
        assert!(reason.contains("not tolerated Finder metadata"), "{reason}");
        assert!(reason.contains("intent stays pending"), "{reason}");
        // The directory is back under its own name, with the foreign entry and
        // nothing else, so a replay names exactly what is left.
        assert_eq!(names_in(&root), vec!["sub".to_owned()]);
        assert_eq!(names_in(&sub), vec!["foreign.txt".to_owned()]);
        assert_eq!(fs::read(sub.join("foreign.txt")).unwrap(), b"foreign");
    }

    /// A symbolic link in the quarantine is never followed, captured or
    /// removed: the pass stops pending and the referent is untouched.
    #[test]
    fn an_observed_link_stops_the_pass_without_following_it() {
        let fixture = Fixture::new();
        let (root, outside) = (
            fixture.root.join("quarantine"),
            fixture.root.join("outside"),
        );
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".DS_Store")).unwrap();
        let (outcome, _deleter) = delete_fixture(&root);
        let reason = pass_reason(&outcome);
        assert!(reason.contains("symbolic link"), "{reason}");
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
        assert!(fs::symlink_metadata(root.join(".DS_Store"))
            .unwrap()
            .is_symlink());
    }

    /// A private capture name this pass did not create is never ordinary
    /// content: it is kept and the intent stays pending.
    #[test]
    fn an_unrecorded_private_capture_name_is_kept() {
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let leftover = root.join(format!(
            "{}{}",
            oxide_storage::owned_delete::CAPTURE_PREFIX,
            "a".repeat(32)
        ));
        fs::write(&leftover, b"leftover").unwrap();
        let (outcome, _deleter) = delete_fixture(&root);
        let reason = pass_reason(&outcome);
        assert!(reason.contains("private capture name"), "{reason}");
        assert_eq!(fs::read(&leftover).unwrap(), b"leftover");
    }

    /// An entry whose reading is another device stops the pass before any
    /// effect, and the entry is untouched.
    #[test]
    fn an_entry_on_another_device_stops_the_pass() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        let file = root.join("owned.txt");
        fs::write(&file, b"owned").unwrap();
        let device = fs::metadata(&root).unwrap().dev();
        let dir = fs::File::open(&root).unwrap();
        // One injected volume reading: the pass proves every entry against the
        // volume it was opened for.
        let mut deleter = Deleter::fixture_on(&root, device.wrapping_add(1));
        let outcome = delete_contents(&mut deleter, dir.as_fd(), false);
        let reason = pass_reason(&outcome);
        assert!(reason.contains("another device"), "{reason}");
        assert_eq!(fs::read(&file).unwrap(), b"owned");
        assert_eq!(names_in(&root), vec!["owned.txt".to_owned()]);
    }

    /// An enumeration that stops with an error is never read as an ended
    /// directory: the pass stops pending before it removes anything, even though
    /// the partial list it had already collected held a real owned entry.
    #[test]
    fn an_incomplete_listing_stops_the_pass_before_any_effect() {
        let fixture = Fixture::new();
        let root = fixture.root.join("quarantine");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("owned.txt"), b"owned").unwrap();
        fs::write(root.join("second.txt"), b"second").unwrap();
        let _fault = test_enumeration_fault::arm(1, libc::EIO);
        let (outcome, counts) = delete_fixture(&root);
        let reason = pass_reason(&outcome);
        assert!(reason.contains("cannot finish reading"), "{reason}");
        assert_eq!(
            names_in(&root),
            vec!["owned.txt".to_owned(), "second.txt".to_owned()]
        );
        assert_eq!(counts.entries, 0);
    }

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            Self::at(&std::env::temp_dir())
        }

        /// A fixture inside the explicit internal root named by
        /// `OXIDE_TEST_INTERNAL_ROOT` (AGENTS.md, Rust test recipe), for a test
        /// that must run on the boot Data volume. The leased child's `TMPDIR`
        /// is on the selected pool's volume instead, because the product routes
        /// it there, so this checks the caller's root and creates one unique
        /// owner-only fixture inside it rather than guessing from `TMPDIR`.
        fn at_internal_root() -> Self {
            let named = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT").unwrap_or_else(|| {
                panic!(
                    "set OXIDE_TEST_INTERNAL_ROOT to a fresh private directory on the internal \
                     volume before running this test (AGENTS.md, Rust test recipe), for example \
                     `mktemp -d /private/tmp/oxide-internal.XXXXXX`"
                )
            });
            let base = fs::canonicalize(&named).unwrap_or_else(|error| {
                panic!(
                    "OXIDE_TEST_INTERNAL_ROOT {} is not a usable directory: {error}",
                    Path::new(&named).display()
                )
            });
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // SAFETY: geteuid has no preconditions.
            let euid = unsafe { libc::geteuid() };
            let metadata = fs::metadata(&base).unwrap();
            assert!(metadata.is_dir(), "{} is not a directory", base.display());
            assert_eq!(
                metadata.uid(),
                euid,
                "{} is not owned by this user",
                base.display()
            );
            assert_eq!(
                metadata.permissions().mode() & 0o077,
                0,
                "{} is not private to this user",
                base.display()
            );
            Self::at(&base)
        }

        fn at(base: &Path) -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = base.join(format!("oxide-prune-{}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            Self { root }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn row_for(root: &Path) -> Row {
        Row {
            checkout_id: "a".repeat(64),
            checkout: root.join("checkout"),
            checkout_volume_uuid: None,
            receipt: root.join("receipt.json"),
            member: outputs::MEMBER.to_owned(),
            root: root.join("root"),
            root_kind: RootKind::External,
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            recipe: outputs::RECIPE_PUBLIC.to_owned(),
            nonce: Some("b".repeat(32)),
            generation: 1,
            root_dev: Some(1),
            root_ino: Some(2),
            marker_digest: Some("c".repeat(64)),
            selected: true,
            pending: false,
            pruning: None,
            pruned_ms: None,
            reservation: None,
            cancelling: None,
        }
    }

    /// Two references of one root under the same checkout form one group; the
    /// registry durably keeps the sorted group, its preimage pin and the
    /// expected terminal image, and refuses a group that is not sorted.
    #[test]
    fn a_root_group_is_sorted_durable_and_validated() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.selected = false;
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 1,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![release.clone(), debug.clone()],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let rows = group_rows(&registry, &debug.checkout_id, &debug.root);
        assert_eq!(
            rows.iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            ["debug", "release"]
        );
        registry.validate().unwrap();
        let root = debug.root.clone();
        let group: Vec<GroupRow> = rows
            .iter()
            .map(|row| GroupRow {
                member: row.member.clone(),
                generation: row.generation,
                nonce: row.nonce.clone(),
                root_dev: row.root_dev,
                root_ino: row.root_ino,
                marker_digest: row.marker_digest.clone(),
                selected: row.selected,
            })
            .collect();
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: "d".repeat(32),
            identity: RootIdentity {
                checkout_id: debug.checkout_id.clone(),
                checkout: debug.checkout.clone(),
                receipt: debug.receipt.clone(),
                member: "debug".to_owned(),
                root: root.clone(),
                root_kind: debug.root_kind,
                volume_uuid: debug.volume_uuid.clone(),
                nonce: debug.nonce.clone(),
                generation: debug.generation,
                dev: debug.root_dev.unwrap(),
                ino: debug.root_ino.unwrap(),
                marker_digest: debug.marker_digest.clone().unwrap(),
            },
            phase: IntentPhase::Intent,
            quarantine: root.join(".prune-intent"),
            preview_sha256: "e".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: true,
            prior_receipt_sha: "f".repeat(64),
            group: group.clone(),
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        intent.validate().unwrap();
        let mut registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 2,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![debug, release],
            intents: vec![intent.clone()],
            cancelled_overflow: Vec::new(),
        };
        write_registry(&fixture.root, &registry).unwrap();
        let loaded = read_registry(&fixture.root).unwrap().unwrap();
        assert_eq!(loaded.intents[0].group, group);
        assert_eq!(loaded.intents[0].prior_receipt_sha, "f".repeat(64));
        // An unsorted or duplicated group is refused.
        let mut unsorted = registry.intents[0].clone();
        unsorted.group.reverse();
        assert!(unsorted.validate().is_err());
        let mut terminal = registry.intents[0].clone();
        terminal.expected_terminal_sha = Some("1".repeat(64));
        assert!(
            terminal.validate().is_err(),
            "a terminal image needs its fixed timestamp"
        );
        terminal.terminal_ms = Some(7);
        terminal.validate().unwrap();
        // The fixed terminal image only accepts a recorded preimage or itself.
        terminal.prior_receipt_sha = String::new();
        registry.intents = vec![terminal];
        write_registry(&fixture.root, &registry).unwrap();
        let loaded = read_registry(&fixture.root).unwrap().unwrap();
        assert_eq!(loaded.intents[0].terminal_ms, Some(7));
    }

    #[test]
    fn nonces_are_128_bit_lower_hex_and_never_repeat() {
        let first = fresh_nonce().unwrap();
        let second = fresh_nonce().unwrap();
        assert!(hex32(&first), "{first}");
        assert!(hex32(&second), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn a_preview_is_canonical_and_its_digest_binds_the_exact_bytes() {
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 7,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: Vec::new(),
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let preview = build_preview(&registry, &[], None, registry.complete).unwrap();
        let first = serde_json::to_vec(&preview).unwrap();
        let second = serde_json::to_vec(&preview).unwrap();
        assert_eq!(first, second);
        let mut changed = preview.clone();
        changed.registry_revision = 8;
        assert_ne!(
            sha256_hex(&first),
            sha256_hex(&serde_json::to_vec(&changed).unwrap())
        );
    }

    fn empty_registry() -> Registry {
        Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 7,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: Vec::new(),
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        }
    }

    fn synthetic_entry(index: usize, pad: usize, kept: Option<&str>) -> Evaluated {
        let mut row = row_for(Path::new("/"));
        row.checkout_id = format!("{index:064x}");
        row.checkout = PathBuf::from(format!("/c/{index}"));
        row.receipt = PathBuf::from(format!("/e/{index}"));
        row.member = "t".to_owned();
        row.root = PathBuf::from(format!("/r/{index}{}", "x".repeat(pad)));
        row.root_ino = Some(index as u64);
        if let Some(reason) = kept {
            Evaluated {
                row,
                candidate: None,
                kept: Some(reason.to_owned()),
                legacy: false,
                evidence: None,
            }
        } else {
            let mut candidate = candidate_of(&row);
            candidate.ino = index as u64;
            Evaluated {
                row,
                candidate: Some(candidate),
                kept: None,
                legacy: false,
                evidence: None,
            }
        }
    }

    fn eligible_entries(count: usize, pad: usize) -> Vec<Evaluated> {
        (0..count)
            .map(|index| synthetic_entry(index, pad, None))
            .collect()
    }

    fn preview_of(evaluated: &[Evaluated]) -> Preview {
        build_preview(&empty_registry(), evaluated, None, true).unwrap()
    }

    fn eligible_candidates(evaluated: &[Evaluated]) -> Vec<Candidate> {
        evaluated
            .iter()
            .filter_map(|entry| entry.candidate.clone())
            .collect()
    }

    #[test]
    fn a_preview_under_the_cap_keeps_every_eligible_root() {
        let mut evaluated = eligible_entries(3, 0);
        evaluated.push(synthetic_entry(
            3,
            0,
            Some("the checkout is present, so its root is kept"),
        ));
        let preview = preview_of(&evaluated);
        preview.validate().unwrap();
        assert_eq!(preview.candidates.len(), 3);
        assert_eq!(preview.coverage.candidate_roots, 3);
        assert_eq!(preview.coverage.kept_roots, 1);
        assert_eq!(preview.totals.candidates, 3);
        assert_eq!(preview.totals.kept, 1);
        assert!(preview.coverage.complete);
        assert_eq!(
            preview.candidates[0].root,
            evaluated[0].candidate.as_ref().unwrap().root
        );
        assert!(preview
            .kept
            .iter()
            .any(|kept| kept.reason.contains("checkout is present")));
        let bytes = serde_json::to_vec(&preview).unwrap();
        assert!(bytes.len() <= MAX_PLAN_BYTES);
    }

    #[test]
    fn a_preview_selects_a_bounded_prefix_when_eligible_roots_exceed_512() {
        let mut evaluated = eligible_entries(MAX_CANDIDATES + 1, 0);
        evaluated.push(synthetic_entry(900, 0, Some("a foreign file is present")));
        evaluated.push(synthetic_entry(
            901,
            0,
            Some("an active writer still holds this checkout"),
        ));
        evaluated.push(synthetic_entry(
            902,
            0,
            Some("checkout absence is unknown (no volume facts)"),
        ));
        let preview = preview_of(&evaluated);
        preview.validate().unwrap();
        let bytes = serde_json::to_vec(&preview).unwrap();
        assert!(bytes.len() <= MAX_PLAN_BYTES, "{}", bytes.len());
        assert!(preview.candidates.len() <= MAX_CANDIDATES);
        assert!(!preview.candidates.is_empty());
        assert_eq!(
            preview.coverage.candidate_roots,
            (MAX_CANDIDATES + 1) as u64
        );
        assert_eq!(preview.coverage.kept_roots, 3);
        assert_eq!(preview.totals.candidates, preview.candidates.len() as u64);
        assert_eq!(preview.totals.kept, 3);
        let deferred = preview.coverage.candidate_roots - preview.totals.candidates;
        assert!(deferred > 0);
        assert!(preview.coverage.complete);
        let eligible = eligible_candidates(&evaluated);
        assert_eq!(
            preview.candidates,
            eligible[..preview.candidates.len()].to_vec()
        );
        assert!(preview.kept.iter().any(|kept| kept
            .reason
            .contains("eligible root(s) deferred to a later prune batch")));
        assert!(preview
            .kept
            .iter()
            .any(|kept| kept.reason.contains("foreign file")));
        assert!(preview
            .kept
            .iter()
            .any(|kept| kept.reason.contains("active writer")));
        assert!(preview
            .kept
            .iter()
            .any(|kept| kept.reason.contains("absence is unknown")));
        assert!(preview.candidates.iter().all(|candidate| candidate
            .root
            .as_os_str()
            .to_string_lossy()
            .starts_with("/r/")));
    }

    #[test]
    fn a_preview_fits_byte_pressure_below_the_root_cap() {
        let evaluated = eligible_entries(250, 1000);
        assert!(evaluated.len() < MAX_CANDIDATES);
        let preview = preview_of(&evaluated);
        preview.validate().unwrap();
        let bytes = serde_json::to_vec(&preview).unwrap();
        assert!(bytes.len() <= MAX_PLAN_BYTES, "{}", bytes.len());
        assert!(preview.candidates.len() < evaluated.len());
        assert!(!preview.candidates.is_empty());
        assert_eq!(preview.coverage.candidate_roots, 250);
        assert_eq!(preview.totals.candidates, preview.candidates.len() as u64);
        assert!(preview.coverage.candidate_roots > preview.totals.candidates);
        let eligible = eligible_candidates(&evaluated);
        assert_eq!(
            preview.candidates,
            eligible[..preview.candidates.len()].to_vec()
        );
        assert!(
            evaluated[0]
                .candidate
                .as_ref()
                .unwrap()
                .root
                .as_os_str()
                .len()
                > 900
        );
    }

    #[test]
    fn a_preview_refuses_an_unrepresentable_candidate() {
        let evaluated = eligible_entries(1, MAX_PLAN_BYTES);
        let error = build_preview(&empty_registry(), &evaluated, None, true).unwrap_err();
        assert!(error.contains("cannot be represented in a"), "{error}");
        assert!(
            error.contains(
                &evaluated[0]
                    .candidate
                    .as_ref()
                    .unwrap()
                    .root
                    .display()
                    .to_string()
            ),
            "{error}"
        );
    }

    #[test]
    fn a_preview_batch_is_stable_and_progresses_across_batches() {
        let evaluated = eligible_entries(MAX_CANDIDATES + 1, 0);
        let first = preview_of(&evaluated);
        let again = preview_of(&evaluated);
        assert_eq!(first.candidates, again.candidates);
        assert_eq!(
            first.coverage.candidate_roots,
            again.coverage.candidate_roots
        );
        let eligible = eligible_candidates(&evaluated);
        let authorized = authorize_plan(&first.candidates, &again.candidates, &eligible, &[]);
        assert_eq!(
            authorized.unwrap().into_iter().cloned().collect::<Vec<_>>(),
            first.candidates
        );
        let remainder: Vec<Evaluated> = eligible_entries(MAX_CANDIDATES + 1, 0)
            .into_iter()
            .filter(|entry| {
                entry.candidate.as_ref().is_some_and(|candidate| {
                    !first
                        .candidates
                        .iter()
                        .any(|selected| same_identity(selected, candidate))
                })
            })
            .collect();
        assert!(!remainder.is_empty());
        let second = preview_of(&remainder);
        second.validate().unwrap();
        assert!(!second.candidates.is_empty());
        assert!(second.candidates.iter().all(|candidate| {
            !first
                .candidates
                .iter()
                .any(|selected| same_identity(selected, candidate))
        }));
        assert_eq!(
            first.candidates.len() + second.candidates.len(),
            eligible.len()
        );
    }

    #[test]
    fn a_preview_plan_rejects_changed_or_extra_identities() {
        let evaluated = eligible_entries(MAX_CANDIDATES + 1, 0);
        let plan = preview_of(&evaluated);
        let eligible = eligible_candidates(&evaluated);
        let mut changed = eligible.clone();
        changed[0].nonce = Some("d".repeat(32));
        let mut changed_entries = eligible_entries(MAX_CANDIDATES + 1, 0);
        changed_entries[0].candidate = Some(changed[0].clone());
        let fresh = preview_of(&changed_entries);
        assert!(authorize_plan(&plan.candidates, &fresh.candidates, &changed, &[]).is_none());
        let mut extra = eligible_entries(MAX_CANDIDATES + 1, 0);
        extra.insert(0, synthetic_entry(2000, 0, None));
        let extra_preview = preview_of(&extra);
        let extra_eligible = eligible_candidates(&extra);
        assert!(authorize_plan(
            &plan.candidates,
            &extra_preview.candidates,
            &extra_eligible,
            &[]
        )
        .is_none());
    }

    #[test]
    fn a_preview_plan_does_not_backfill_after_replay() {
        let evaluated = eligible_entries(MAX_CANDIDATES + 1, 0);
        let plan = preview_of(&evaluated);
        let completed = vec![plan.candidates[0].root.clone()];
        let remainder: Vec<Evaluated> = eligible_entries(MAX_CANDIDATES + 1, 0)
            .into_iter()
            .filter(|entry| {
                entry
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| candidate.root != completed[0])
            })
            .collect();
        let fresh = preview_of(&remainder);
        let eligible = eligible_candidates(&remainder);
        let authorized =
            authorize_plan(&plan.candidates, &fresh.candidates, &eligible, &completed).unwrap();
        assert_eq!(authorized.len(), plan.candidates.len() - 1);
        assert!(authorized.iter().all(|candidate| plan
            .candidates
            .iter()
            .any(|planned| same_identity(planned, candidate))));
        let deferred = &eligible_candidates(&evaluated)[plan.candidates.len()];
        assert!(authorized
            .iter()
            .all(|candidate| !same_identity(candidate, deferred)));
    }

    #[test]
    fn the_registry_refuses_unknown_schemas_and_contradictory_incompleteness() {
        let mut registry = Registry {
            version: "oxide.outputs.registry/9".to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 1,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: Vec::new(),
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        assert!(registry.validate().is_err());
        registry.version = REGISTRY_VERSION.to_owned();
        registry.complete = false;
        assert!(registry.validate().is_err());
        registry.complete = true;
        registry.incomplete = Some(Incomplete {
            reason: "x".to_owned(),
            since_ms: 1,
            transaction: "a".repeat(32),
        });
        assert!(registry.validate().is_err());
        registry.revision = 0;
        registry.incomplete = None;
        assert!(registry.validate().is_err());
    }

    #[test]
    fn a_row_can_be_selected_and_pending_but_never_pruning() {
        let root = Fixture::new();
        let mut row = row_for(&root.root);
        assert!(row.validate().is_ok());
        // A prepared relink of the same root names it in both references.
        row.pending = true;
        assert!(row.validate().is_ok());
        row.pruning = Some("b".repeat(32));
        assert!(row.validate().is_err());
        row.selected = false;
        assert!(row.validate().is_err());
        row.pending = false;
        assert!(row.validate().is_ok());
    }

    #[test]
    fn an_external_root_must_sit_inside_an_oxide_outputs_directory() {
        let root = Fixture::new();
        let mut row = row_for(&root.root);
        assert!(!row.root_kind_ok());
        row.root = root.root.join(outputs::EXTERNAL_DIR).join("checkout-0000");
        assert!(row.root_kind_ok());
        row.volume_uuid = None;
        assert!(!row.root_kind_ok());
        row.volume_uuid = Some("uuid".to_owned());
        row.root_kind = RootKind::Local;
        assert!(!row.root_kind_ok());
    }

    #[test]
    fn operator_lists_reject_relative_duplicate_and_oversized_entries() {
        let fixture = Fixture::new();
        let write = |name: &str, body: &str| {
            let path = fixture.root.join(name);
            fs::write(&path, body).unwrap();
            path
        };
        assert!(read_operator_list(&write("relative.json", "[\"a/b\"]")).is_err());
        assert!(read_operator_list(&write("dotdot.json", "[\"/a/../b\"]")).is_err());
        assert!(read_operator_list(&write("duplicate.json", "[\"/a\",\"/a\"]")).is_err());
        assert!(read_operator_list(&write(
            "many.json",
            &format!(
                "[{}]",
                (0..MAX_CANDIDATES + 1)
                    .map(|index| format!("\"/root/{index}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        ))
        .is_err());
        let list = read_operator_list(&write("ok.json", "[\"/root/a\",\"/root/b\"]")).unwrap();
        assert_eq!(list.roots.len(), 2);
        assert_eq!(list.sha256, sha256_hex(b"[\"/root/a\",\"/root/b\"]"));
    }

    #[test]
    fn a_volume_checkout_walk_begins_below_the_verified_mount_root() {
        // Regression for the review finding: the mount root descriptor is
        // opened already, so the volume name must not be visited again. A
        // live checkout previously read as absent and its root was prunable.
        let (start, rest) = checkout_path(Path::new("/Volumes/Builds/oxide-outputs/app")).unwrap();
        assert!(matches!(&start, WalkStart::Mount(mount) if mount == Path::new("/Volumes/Builds")));
        assert_eq!(
            rest,
            vec![
                std::ffi::OsString::from("oxide-outputs"),
                std::ffi::OsString::from("app")
            ]
        );
        // The boot Data alias of /Volumes is the same mount spelling, so an
        // absent mount name can never fall through to the plain-path rule.
        let (start, rest) = checkout_path(Path::new(
            "/System/Volumes/Data/Volumes/Builds/oxide-outputs/app",
        ))
        .unwrap();
        assert!(matches!(&start, WalkStart::Mount(mount) if mount == Path::new("/Volumes/Builds")));
        assert_eq!(
            rest,
            vec![
                std::ffi::OsString::from("oxide-outputs"),
                std::ffi::OsString::from("app")
            ]
        );
        let (start, rest) = checkout_path(Path::new("/Users/tom/src/app")).unwrap();
        assert!(matches!(&start, WalkStart::Root));
        assert_eq!(
            rest,
            vec![
                std::ffi::OsString::from("Users"),
                std::ffi::OsString::from("tom"),
                std::ffi::OsString::from("src"),
                std::ffi::OsString::from("app")
            ]
        );
        assert!(checkout_path(Path::new("/Volumes/Builds")).is_err());
        assert!(checkout_path(Path::new("/System/Volumes/Data/Volumes")).is_err());
        assert!(checkout_path(Path::new("/")).is_err());
    }

    /// The path is validated before it is walked: a spelling that is not one
    /// exact absolute form is refused, never repaired or canonicalized.
    #[test]
    fn a_non_exact_checkout_spelling_is_never_resolved() {
        for spelling in [
            "relative/checkout",
            "",
            "/Users//tom/app",
            "/Users/tom/app/",
            "/Users/./tom/app",
            "/Users/tom/../tom/app",
            "/Users/tom/app\u{0}",
        ] {
            assert!(
                checkout_path(Path::new(spelling)).is_err(),
                "{spelling:?} was accepted"
            );
        }
        assert!(checkout_path(Path::new("/Users/tom/app")).is_ok());
        let mut row = row_for(Path::new("/private/tmp"));
        row.checkout = PathBuf::from("/Users//tom/app");
        assert!(matches!(
            checkout_absence(&row, true).unwrap(),
            Absence::Unknown(_)
        ));
    }

    /// The volume rules are pure decisions over held facts, so a device or
    /// UUID mismatch is isolated from the lexical mount exclusion.
    #[test]
    fn the_volume_rules_judge_held_facts_only() {
        let data = HeldFacts {
            dev: 7,
            fstype: "apfs".to_owned(),
            uuid: Some("data-uuid".to_owned()),
        };
        let ancestor = data.clone();
        assert!(judge_boot_data(&ancestor, &data).is_ok());
        assert!(judge_boot_data(
            &HeldFacts {
                dev: 8,
                ..data.clone()
            },
            &data
        )
        .is_err());
        assert!(judge_boot_data(
            &HeldFacts {
                uuid: Some("pool-uuid".to_owned()),
                ..data.clone()
            },
            &data
        )
        .is_err());
        assert!(judge_boot_data(
            &HeldFacts {
                fstype: "hfs".to_owned(),
                ..data.clone()
            },
            &data
        )
        .is_err());
        assert!(judge_boot_data(
            &HeldFacts {
                uuid: None,
                ..data.clone()
            },
            &data
        )
        .is_err());
        assert!(judge_boot_data(
            &ancestor,
            &HeldFacts {
                uuid: None,
                ..data.clone()
            }
        )
        .is_err());
        assert!(judge_boot_data(
            &ancestor,
            &HeldFacts {
                fstype: "nfs".to_owned(),
                ..data.clone()
            }
        )
        .is_err());
        assert!(judge_recorded_volume(&ancestor, "data-uuid").is_ok());
        assert!(judge_recorded_volume(&ancestor, "pool-uuid").is_err());
        // The recorded-volume rule keeps its existing APFS-or-HFS shape; the
        // boot Data rule is APFS only, because /System/Volumes/Data is APFS.
        assert!(judge_recorded_volume(
            &HeldFacts {
                fstype: "hfs".to_owned(),
                ..data.clone()
            },
            "data-uuid"
        )
        .is_ok());
        assert!(judge_recorded_volume(
            &HeldFacts {
                uuid: None,
                ..data.clone()
            },
            "data-uuid"
        )
        .is_err());
        assert!(judge_recorded_volume(
            &HeldFacts {
                fstype: "exfat".to_owned(),
                ..data.clone()
            },
            "data-uuid"
        )
        .is_err());
    }

    #[test]
    fn a_ready_registry_is_never_recreated_when_it_is_missing() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let transaction = "a".repeat(32);
        let refused = create_or_validate_registry(&control, &transaction, false).unwrap_err();
        assert!(refused.contains("refusing to rebuild"), "{refused}");
        assert!(!registry_path(&control).exists());
        create_or_validate_registry(&control, &transaction, true).unwrap();
        assert!(registry_path(&control).is_file());
        // Validation without creation succeeds for the exact transaction.
        create_or_validate_registry(&control, &transaction, false).unwrap();
        assert_eq!(ready_registry(&control).unwrap().created_by, "a".repeat(32));
    }

    /// A pool adopted before the shared format has no registry: a command's
    /// transition or replay refuses before any effect and names the explicit
    /// upgrade, and a receipt write never creates the registry.
    #[test]
    fn a_pre_fence_pool_refuses_receipt_writes_with_the_upgrade_named() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        let refused = require_registry(&control).unwrap_err();
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert!(refused.contains("unchanged"), "{refused}");
        assert!(!registry_path(&control).exists());
    }

    /// Per-root stops before the journal keep the root (exit 0 or 2 for the
    /// batch); stops after it leave the intent pending (exit 3).
    #[test]
    fn stops_before_the_journal_are_refusals_and_after_it_pending() {
        assert!(matches!(
            before_journal(Stop::Pending("volume".to_owned())),
            Stop::Refuse(reason) if reason == "volume"
        ));
        assert!(matches!(
            after_journal(Stop::Refuse("marker".to_owned())),
            Stop::Pending(reason) if reason == "marker"
        ));
        assert_eq!(partial(&[PathBuf::from("/r")], "kept"), 0);
    }

    #[test]
    fn usage_and_refusals_use_the_contract_exit_codes() {
        assert_eq!(
            run(&[std::ffi::OsString::from("prune"), "bogus".into()]).unwrap(),
            2
        );
    }

    #[test]
    fn absence_is_only_the_actual_leaf_enoent_without_following_links() {
        let fixture = Fixture::new();
        let missing = fixture.root.join("missing-checkout");
        let mut row = row_for(&fixture.root);
        row.checkout = missing.clone();
        // Without a recorded checkout volume, absence is never proved.
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Unknown(_)
        ));
        row.checkout_volume_uuid = checkout_volume_uuid(&fixture.root);
        let Absence::Absent(witness) = checkout_absence(&row, false).unwrap() else {
            panic!("the actual leaf's ENOENT did not prove absence");
        };
        assert_eq!(witness.missing, Some(witness.observed.len()));
        assert!(
            witness.boot_data.is_none(),
            "a fully present ancestry needs no boot Data proof"
        );
        // A different recorded volume keeps it.
        let mut other = row.clone();
        other.checkout_volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        assert!(matches!(
            checkout_absence(&other, false).unwrap(),
            Absence::Unknown(_)
        ));
        fs::create_dir_all(&missing).unwrap();
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Present
        ));
        // A missing ancestor chain is never absence on a volume that is not
        // the boot Data volume. This chain's nearest existing ancestor is `/`,
        // the read-only system volume, so even the legacy exception keeps it.
        let mut deep = row_for(&fixture.root);
        deep.checkout = PathBuf::from(format!(
            "/oxide-absent-{}-chain/checkout",
            std::process::id()
        ));
        assert!(matches!(
            checkout_absence(&deep, false).unwrap(),
            Absence::Unknown(_)
        ));
        assert!(matches!(
            checkout_absence(&deep, true).unwrap(),
            Absence::Unknown(_)
        ));
        // A missing chain below an absent mount name is a MOUNT refusal in
        // both spellings, never a plain missing chain.
        for spelling in [
            "/Volumes/OxideAbsentNoSuchMount/deep/checkout".to_owned(),
            "/System/Volumes/Data/Volumes/OxideAbsentNoSuchMount/deep/checkout".to_owned(),
        ] {
            let mut unmounted = row_for(&fixture.root);
            unmounted.checkout = PathBuf::from(&spelling);
            unmounted.checkout_volume_uuid = checkout_volume_uuid(&fixture.root);
            assert!(
                matches!(
                    checkout_absence(&unmounted, false).unwrap(),
                    Absence::Unknown(_)
                ),
                "{spelling} was not kept as a mount refusal"
            );
            assert!(
                matches!(
                    checkout_absence(&unmounted, true).unwrap(),
                    Absence::Unknown(_)
                ),
                "{spelling} was not kept as a mount refusal"
            );
        }
        // A symlinked ancestor is never followed.
        let link = fixture.root.join("linked");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        let mut linked = row_for(&fixture.root);
        linked.checkout = link.join("child");
        assert!(matches!(
            checkout_absence(&linked, false).unwrap(),
            Absence::Unknown(_)
        ));
        // A non-directory component is never walked through.
        let file = fixture.root.join("plain-file");
        fs::write(&file, b"file").unwrap();
        let mut through_file = row_for(&fixture.root);
        through_file.checkout = file.join("child");
        assert!(matches!(
            checkout_absence(&through_file, false).unwrap(),
            Absence::Unknown(_)
        ));
    }

    #[test]
    fn root_identity_does_not_depend_on_a_nonexistent_legacy_nonce() {
        let fixture = Fixture::new();
        let root = fixture.root.join("root");
        fs::create_dir(&root).unwrap();
        let mut selected = Selected {
            kind: RootKind::Local,
            root: root.clone(),
            volume_uuid: None,
            nonce: None,
            generation: 1,
        };
        let identity = root_identity(&selected).unwrap();
        assert!(identity.dev.is_some() && identity.ino.is_some() && identity.digest.is_none());
        assert!(
            identity.marker.is_none(),
            "a markerless root records no marker"
        );
        selected = Selected {
            kind: RootKind::Local,
            root: root.clone(),
            volume_uuid: None,
            nonce: Some("a".repeat(32)),
            generation: 1,
        };
        let identity = root_identity(&selected).unwrap();
        assert!(identity.digest.is_none());
        // A root without a readable marker has no fact that contradicts the
        // placement, so the row records the selection's generation as before.
        assert!(identity.describes(&selected));
        assert_eq!(identity.generation(&selected), 1);
    }

    /// oxide#218: the row records the root marker's OWN immutable creation
    /// generation, never the placement's commit generation, and a placement that
    /// no longer describes the live marker (the version-1 receipt a migration
    /// upgraded the marker under) does not rewrite the row.
    #[test]
    fn a_recorded_row_takes_its_generation_and_agreement_from_the_marker() {
        let fixture = Fixture::new();
        let root = fixture.root.join("root");
        fs::create_dir_all(&root).unwrap();
        let mut row = row_for(&fixture.root);
        row.nonce = Some("d".repeat(32));
        write_root_marker(&root, &root_marker(&row));
        let selected = Selected {
            kind: RootKind::External,
            root: root.clone(),
            volume_uuid: row.volume_uuid.clone(),
            nonce: Some("d".repeat(32)),
            generation: 1,
        };
        let recorded = root_identity(&selected).unwrap();
        assert_eq!(
            recorded.marker.as_ref(),
            Some(&(Some("d".repeat(32)), 1u64)),
            "the row's identity must read the marker's own nonce and generation"
        );
        // The marker was created at generation 1 while a placement's committed
        // generation is 2: the row records 1, and that placement does not
        // describe the live marker (the migration's superseded whole-target
        // reference, which must never rewrite the row).
        let newer = Selected {
            generation: 2,
            ..selected.clone()
        };
        assert_eq!(recorded.generation(&newer), 1);
        assert!(!recorded.describes(&newer));
        // The same root's matching placement describes it.
        assert!(recorded.describes(&selected));
        assert_eq!(recorded.generation(&selected), 1);
        // A different nonce is a different root creation, never a match.
        let mut other = selected.clone();
        other.nonce = Some("e".repeat(32));
        assert!(!recorded.describes(&other));
    }

    #[test]
    fn candidate_identity_sets_match_one_to_one() {
        let candidate = |root: &str, nonce: &str, dev: u64| Candidate {
            checkout_id: "a".repeat(64),
            checkout: PathBuf::from("/gone/checkout"),
            receipt: PathBuf::from("/control/outputs/a.json"),
            member: outputs::MEMBER.to_owned(),
            root: PathBuf::from(root),
            root_kind: RootKind::External,
            volume_uuid: Some("uuid".to_owned()),
            checkout_volume_uuid: None,
            nonce: Some(nonce.to_owned()),
            generation: 1,
            dev,
            ino: 2,
            marker_digest: "c".repeat(64),
            logical_bytes: 0,
            allocated_bytes: 0,
            accounting_complete: true,
            receiptless: false,
        };
        let first = candidate("/vol/oxide-outputs/a", &"b".repeat(32), 1);
        let second = candidate("/vol/oxide-outputs/a", &"d".repeat(32), 1);
        assert!(same_candidates(&[&first], &[&first]));
        assert!(!same_candidates(&[&first], &[&second]));
        assert!(!same_candidates(&[&first], &[]));
    }

    #[test]
    fn authority_budget_charges_actual_reads_and_latches_exhaustion() {
        let scope = AuthorityScope::begin().unwrap();
        AUTHORITY.with(|cell| cell.borrow_mut().as_mut().unwrap().remaining = 5);
        let mut first = Vec::new();
        assert_eq!(authority_read(&b"abc"[..], &mut first).unwrap(), 3);
        assert!(scope.complete());
        let mut second = Vec::new();
        assert!(authority_read(&b"xyz"[..], &mut second).is_err());
        assert_eq!(second, b"xy");
        assert!(!scope.complete());
    }

    #[test]
    fn a_malformed_receipt_keeps_the_whole_snapshot_batch() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let outputs = registry_dir(&control);
        ensure_private_dir(&outputs).unwrap();
        let mut row = row_for(&fixture.root);
        row.checkout_id = "d".repeat(64);
        let receipt_path = outputs::receipt_path_public(&control, &row.checkout_id);
        fs::write(&receipt_path, b"{not a receipt").unwrap();
        row.receipt = receipt_path;
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 1,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![row],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        write_registry(&control, &registry).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let evaluated = evaluate_test(&store, &control, &registry, None).unwrap();
        assert!(!evaluated.complete);
        assert!(
            !build_preview(&registry, &evaluated, None, evaluated.complete)
                .unwrap()
                .coverage
                .complete
        );
        assert!(evaluated.iter().all(|entry| entry.candidate.is_none()));
        assert!(evaluated.iter().all(|entry| entry
            .kept
            .as_deref()
            .is_some_and(|reason| reason.contains("whole batch"))));
    }

    /// Ordinary path ineligibility keeps ONE root and leaves the snapshot
    /// COMPLETE, so an unprovable sibling never prevents another root from being
    /// considered. Only incomplete required authority refuses the whole batch.
    #[test]
    fn an_unprovable_checkout_keeps_only_its_own_root() {
        // The recipe's internal root: a selected-volume fixture path is a
        // /Volumes mount spelling, where the recorded-volume rule applies first.
        let fixture = Fixture::at_internal_root();
        let present = legacy_snapshot_row(&fixture, &"a".repeat(64));
        fs::create_dir_all(&present.checkout).unwrap();
        fs::create_dir(fixture.root.join("real")).unwrap();
        std::os::unix::fs::symlink(
            fixture.root.join("real"),
            fixture.root.join("linked-parent"),
        )
        .unwrap();
        let mut linked = legacy_snapshot_row(&fixture, &"b".repeat(64));
        linked.checkout = fixture.root.join("linked-parent").join("child");
        let control = ready_control(&fixture, vec![present.clone(), linked.clone()], Vec::new());
        for row in [&present, &linked] {
            publish_snapshot_receipt(
                &control,
                row,
                &snapshot_receipt(
                    row,
                    "oxide.outputs.transition/1",
                    "committed",
                    Some(&row.root),
                ),
            );
        }
        let list = OperatorList {
            roots: vec![present.root.clone(), linked.root.clone()],
            sha256: "d".repeat(64),
        };
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let registry = ready_registry(&control).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert!(
            evaluated.complete,
            "an unprovable sibling made the whole snapshot incomplete"
        );
        assert!(evaluated.iter().all(|entry| entry.candidate.is_none()));
        let reason = |id: &str| {
            evaluated
                .iter()
                .find(|entry| entry.row.checkout_id == id)
                .and_then(|entry| entry.kept.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            reason(&present.checkout_id),
            "the checkout is present, so its root is kept"
        );
        let absent = reason(&linked.checkout_id);
        assert!(
            absent.starts_with("checkout absence is unknown ("),
            "{absent}"
        );
        assert!(!absent.contains("whole batch"), "{absent}");
        // Incomplete required authority is DIFFERENT: a missing receipt still
        // refuses the whole batch, naming its exact file.
        let missing = outputs::receipt_path_public(&control, &linked.checkout_id);
        fs::remove_file(&missing).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert!(
            !evaluated.complete,
            "a missing receipt did not refuse the batch"
        );
        for entry in &evaluated {
            let reason = entry.kept.clone().unwrap_or_default();
            assert!(reason.contains("whole batch"), "{reason}");
            assert!(reason.contains("is missing"), "{reason}");
        }
    }

    /// One synthetic checkout whose recorded root sits inside a `/Volumes`
    /// mount: eligibility stops at the recorded-volume/absence predicate, so a
    /// test needs no real drive. Its recorded identity is legacy (no nonce)
    /// unless the caller sets one.
    fn legacy_snapshot_row(fixture: &Fixture, checkout_id: &str) -> Row {
        let mut row = row_for(&fixture.root);
        row.checkout_id = checkout_id.to_owned();
        row.checkout = fixture.root.join("gone").join(checkout_id);
        row.receipt = outputs::receipt_path_public(&fixture.root.join("control"), checkout_id);
        row.root = PathBuf::from(format!("/Volumes/OxV1/oxide-outputs/{checkout_id}"));
        row.root_kind = RootKind::External;
        row.volume_uuid = Some("11111111-2222-3333-4444-555555555555".to_owned());
        row.checkout_volume_uuid = None;
        row.nonce = None;
        row.generation = 1;
        row.root_dev = Some(1);
        row.root_ino = Some(2);
        row.marker_digest = Some("c".repeat(64));
        row.selected = true;
        row.pending = false;
        row
    }

    /// One receipt document in the published shape of `version`. `root` is the
    /// receipt's current selection (absent for a detached receipt).
    fn snapshot_receipt(
        row: &Row,
        version: &str,
        state: &str,
        root: Option<&Path>,
    ) -> serde_json::Value {
        let selection = root.map(|root| {
            serde_json::json!({
                "kind": "external",
                "root": root.to_string_lossy(),
                "volume_uuid": row.volume_uuid,
                "generation": row.generation,
            })
        });
        let member = if version == "oxide.outputs.transition/3" {
            "debug"
        } else {
            "target"
        };
        let mut document = serde_json::json!({
            "version": version,
            "checkout": row.checkout.to_string_lossy(),
            "checkout_id": row.checkout_id,
            "generation": row.generation,
            "state": state,
            "updated_ms": 1u64,
        });
        if version == "oxide.outputs.transition/1" {
            if let Some(selection) = selection.clone() {
                document["roots"] = serde_json::json!([selection.clone()]);
                document["selected"] = selection;
            }
            return document;
        }
        if version == "oxide.outputs.transition/3" {
            document["layout"] = serde_json::json!("profile");
        }
        if let Some(selection) = selection.clone() {
            document["roots"] = serde_json::json!([selection.clone()]);
        }
        document["links"] = serde_json::json!([{ "member": member, "selected": selection }]);
        document
    }

    /// Publish one receipt document for `row` into a control directory.
    fn publish_snapshot_receipt(control: &Path, row: &Row, document: &serde_json::Value) {
        let path = outputs::receipt_path_public(control, &row.checkout_id);
        fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// oxide#152: a mixed snapshot of version-1, version-2 and version-3
    /// receipts is complete. Every valid legacy receipt is read; each root then
    /// keeps or proceeds on its OWN recorded predicates, never a whole-batch
    /// error caused by its age.
    #[test]
    fn a_mixed_version_one_two_three_snapshot_is_complete() {
        let fixture = Fixture::new();
        // Make the checkout's ancestor deliberately unprovable on both the
        // internal test volume and a /Volumes-backed validation directory.
        fs::create_dir(fixture.root.join("real")).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("real"), fixture.root.join("gone")).unwrap();
        let one = legacy_snapshot_row(&fixture, &"a".repeat(64));
        let two = legacy_snapshot_row(&fixture, &"b".repeat(64));
        let mut three = legacy_snapshot_row(&fixture, &"c".repeat(64));
        three.member = "debug".to_owned();
        let control = ready_control(
            &fixture,
            vec![one.clone(), two.clone(), three.clone()],
            Vec::new(),
        );
        publish_snapshot_receipt(
            &control,
            &one,
            &snapshot_receipt(
                &one,
                "oxide.outputs.transition/1",
                "committed",
                Some(&one.root),
            ),
        );
        publish_snapshot_receipt(
            &control,
            &two,
            &snapshot_receipt(
                &two,
                "oxide.outputs.transition/2",
                "committed",
                Some(&two.root),
            ),
        );
        publish_snapshot_receipt(
            &control,
            &three,
            &snapshot_receipt(
                &three,
                "oxide.outputs.transition/3",
                "committed",
                Some(&three.root),
            ),
        );
        // Every document is exactly what its own reader accepts.
        for row in [&one, &two, &three] {
            assert!(
                outputs::read_receipt_for_prune(&control, &row.checkout, &row.checkout_id)
                    .unwrap()
                    .is_some()
            );
        }
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let registry = ready_registry(&control).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, None).unwrap();
        assert_eq!(evaluated.len(), 3);
        for entry in &evaluated {
            let reason = entry.kept.clone().unwrap_or_default();
            assert!(
                reason.contains("predates the version-2 registry identity"),
                "{reason}"
            );
            assert!(!reason.contains("whole batch"), "{reason}");
        }
        let list = OperatorList {
            roots: vec![one.root.clone(), two.root.clone(), three.root.clone()],
            sha256: "d".repeat(64),
        };
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert_eq!(evaluated.len(), 3);
        // An unprovable checkout path is ordinary per-root ineligibility: it
        // keeps each of these roots and leaves the snapshot COMPLETE.
        assert!(evaluated.complete);
        for entry in &evaluated {
            let reason = entry.kept.clone().unwrap_or_default();
            assert!(reason.contains("checkout absence is unknown"), "{reason}");
            assert!(!reason.contains("whole batch"), "{reason}");
            assert!(entry.candidate.is_none());
        }
    }

    /// The version-1 predicate ladder: the operator list is required, every
    /// original receipt, identity, pending and absence predicate still runs,
    /// and a contradictory or missing document still keeps the whole batch with
    /// its exact receipt file named.
    #[test]
    fn a_version_one_receipt_keeps_every_existing_predicate() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.root.join("real")).unwrap();
        std::os::unix::fs::symlink(fixture.root.join("real"), fixture.root.join("gone")).unwrap();
        let row = legacy_snapshot_row(&fixture, &"a".repeat(64));
        let control = ready_control(&fixture, vec![row.clone()], Vec::new());
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let registry = ready_registry(&control).unwrap();
        let legacy = snapshot_receipt(
            &row,
            "oxide.outputs.transition/1",
            "committed",
            Some(&row.root),
        );
        publish_snapshot_receipt(&control, &row, &legacy);
        let evaluated = evaluate_test(&store, &control, &registry, None).unwrap();
        assert!(evaluated[0]
            .kept
            .as_deref()
            .is_some_and(|reason| reason.contains("explicit operator list")));
        let list = OperatorList {
            roots: vec![row.root.clone()],
            sha256: "d".repeat(64),
        };
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert!(
            evaluated.complete,
            "an unprovable path is not incomplete authority"
        );
        assert!(evaluated[0]
            .kept
            .as_deref()
            .is_some_and(|reason| reason.contains("checkout absence is unknown")));

        // A receipt that selects another root no longer matches its row.
        let elsewhere = PathBuf::from("/Volumes/OxV1/oxide-outputs/elsewhere");
        publish_snapshot_receipt(
            &control,
            &row,
            &snapshot_receipt(
                &row,
                "oxide.outputs.transition/1",
                "committed",
                Some(&elsewhere),
            ),
        );
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert!(evaluated[0]
            .kept
            .as_deref()
            .is_some_and(|reason| reason.contains("the checkout's receipt does not")));

        // A registry row with a recorded nonce is never satisfied by a
        // version-1 receipt: no nonce is invented for it.
        publish_snapshot_receipt(&control, &row, &legacy);
        let mut recorded = row.clone();
        recorded.nonce = Some("b".repeat(32));
        write_snapshot_registry(&control, vec![recorded.clone()], Vec::new());
        let registry = ready_registry(&control).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        assert!(evaluated[0].kept.as_deref().is_some_and(
            |reason| reason.contains("does not match the checkout's committed receipt")
        ));

        // A valid prepared version-1 receipt keeps its live root and its
        // pending root, without poisoning the batch.
        write_snapshot_registry(&control, vec![row.clone()], Vec::new());
        let mut prepared = snapshot_receipt(
            &row,
            "oxide.outputs.transition/1",
            "prepared",
            Some(&row.root),
        );
        prepared["prepared"] = serde_json::json!({
            "generation": 2,
            "to": {"kind": "external", "root": row.root.to_string_lossy(), "volume_uuid": row.volume_uuid, "generation": 2},
            "link": row.checkout.join("target").to_string_lossy(),
        });
        publish_snapshot_receipt(&control, &row, &prepared);
        let registry = ready_registry(&control).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        let reason = evaluated[0].kept.clone().unwrap_or_default();
        assert!(reason.contains("not committed for this root"), "{reason}");

        // A contradictory version-1 document is still invalid: the whole
        // batch is kept and the reason names the exact receipt file.
        let mut contradictory = legacy.clone();
        contradictory["state"] = serde_json::json!("pruned");
        publish_snapshot_receipt(&control, &row, &contradictory);
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        let reason = evaluated[0].kept.clone().unwrap_or_default();
        assert!(reason.contains("whole batch"), "{reason}");
        assert!(
            reason.contains(
                &outputs::receipt_path_public(&control, &row.checkout_id)
                    .display()
                    .to_string()
            ),
            "{reason}"
        );

        // A missing receipt names its own file in the same bounded summary.
        fs::remove_file(outputs::receipt_path_public(&control, &row.checkout_id)).unwrap();
        let evaluated = evaluate_test(&store, &control, &registry, Some(&list)).unwrap();
        let reason = evaluated[0].kept.clone().unwrap_or_default();
        assert!(reason.contains("whole batch"), "{reason}");
        assert!(reason.contains("is missing"), "{reason}");
        assert!(
            reason.contains(
                &outputs::receipt_path_public(&control, &row.checkout_id)
                    .display()
                    .to_string()
            ),
            "{reason}"
        );
    }

    /// The real run's receipt step for an actually pruned version-1 root: the
    /// existing intent/terminal transaction publishes the current receipt shape,
    /// pinned by its digest, and reading it beforehand changes no byte.
    #[test]
    fn a_version_one_root_settles_through_the_existing_terminal_transaction() {
        let fixture = Fixture::new();
        let row = legacy_snapshot_row(&fixture, &"a".repeat(64));
        let id = "e".repeat(32);
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: id.clone(),
            identity: RootIdentity {
                checkout_id: row.checkout_id.clone(),
                checkout: row.checkout.clone(),
                receipt: outputs::receipt_path_public(
                    &fixture.root.join("control"),
                    &row.checkout_id,
                ),
                member: row.member.clone(),
                root: row.root.clone(),
                root_kind: RootKind::External,
                volume_uuid: row.volume_uuid.clone(),
                nonce: None,
                generation: row.generation,
                dev: 1,
                ino: 2,
                marker_digest: "c".repeat(64),
            },
            phase: IntentPhase::Deleted,
            quarantine: PathBuf::from("/Volumes/OxV1/oxide-outputs/.prune-e"),
            preview_sha256: "f".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: true,
            prior_receipt_sha: String::new(),
            group: Vec::new(),
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        let control = ready_control(&fixture, vec![row.clone()], vec![intent.clone()]);
        let path = outputs::receipt_path_public(&control, &row.checkout_id);
        let legacy = snapshot_receipt(
            &row,
            "oxide.outputs.transition/1",
            "committed",
            Some(&row.root),
        );
        publish_snapshot_receipt(&control, &row, &legacy);
        let before = fs::read(&path).unwrap();
        // Reading the terminal image writes nothing.
        assert!(!outputs::terminal_receipt_image(
            &control,
            &row.checkout,
            &row.checkout_id,
            &id,
            "deleted",
            42
        )
        .unwrap()
        .is_empty());
        assert_eq!(fs::read(&path).unwrap(), before);
        // The intent's preimage pin is the version-1 bytes exactly.
        let mut journaled = ready_registry(&control).unwrap();
        journaled.intents[0].prior_receipt_sha = sha256_hex(&before);
        write_registry(&control, &journaled).unwrap();
        settle_receipt(&control, &id, "deleted").unwrap();
        let published = fs::read(&path).unwrap();
        assert_ne!(
            published, before,
            "the pruned root's receipt was not published"
        );
        let document: serde_json::Value = serde_json::from_slice(&published).unwrap();
        assert_eq!(document["version"], "oxide.outputs.transition/2");
        assert_eq!(document["state"], "pruned");
        assert_eq!(document["pruned"]["intent"], id);
        assert_eq!(document["pruned"]["outcome"], "deleted");
        assert_eq!(
            document["roots"][0]["root"],
            row.root.to_string_lossy().as_ref()
        );
        // The published bytes are exactly the image for the recorded fixed
        // timestamp, and the ordinary reader reads the current shape.
        let at_ms = document["pruned"]["at_ms"].as_u64().unwrap();
        assert_eq!(
            outputs::terminal_receipt_image(
                &control,
                &row.checkout,
                &row.checkout_id,
                &id,
                "deleted",
                at_ms
            )
            .unwrap(),
            published
        );
        let read = outputs::read_receipt_for_prune(&control, &row.checkout, &row.checkout_id)
            .unwrap()
            .expect("the terminal receipt was not readable");
        assert!(read.current().is_none());
        let registry = ready_registry(&control).unwrap();
        assert!(registry.intents.is_empty(), "the settled intent remains");
        assert!(registry.rows.iter().all(|row| !row.selected));
    }

    #[test]
    fn resolution_refuses_before_the_volume_without_a_complete_row_bound_snapshot() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: "b".repeat(32),
            identity: RootIdentity {
                checkout_id: "d".repeat(64),
                checkout: fixture.root.join("gone/checkout"),
                receipt: fixture.root.join("control/outputs/d.json"),
                member: outputs::MEMBER.to_owned(),
                root: fixture.root.join("vol/oxide-outputs/app"),
                root_kind: RootKind::External,
                volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
                nonce: Some("c".repeat(32)),
                generation: 1,
                dev: 1,
                ino: 2,
                marker_digest: "e".repeat(64),
            },
            phase: IntentPhase::Renamed,
            quarantine: fixture.root.join("vol/oxide-outputs/.prune-b"),
            preview_sha256: "f".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: true,
            prior_receipt_sha: String::new(),
            group: Vec::new(),
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        let mut registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 1,
            complete: false,
            incomplete: Some(Incomplete {
                reason: "test".to_owned(),
                since_ms: 1,
                transaction: "a".repeat(32),
            }),
            created_by: "a".repeat(32),
            rows: Vec::new(),
            intents: vec![intent.clone()],
            cancelled_overflow: Vec::new(),
        };
        let refused = resolve_precheck(&store, &control, &intent, &registry).unwrap_err();
        assert!(refused.contains("sticky-incomplete"), "{refused}");
        registry.complete = true;
        registry.incomplete = None;
        let refused = resolve_precheck(&store, &control, &intent, &registry).unwrap_err();
        assert!(refused.contains("blocking row"), "{refused}");
    }

    fn ready_control(fixture: &Fixture, rows: Vec<Row>, intents: Vec<Intent>) -> PathBuf {
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        ensure_private_dir(&registry_dir(&control)).unwrap();
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 5,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows,
            intents,
            cancelled_overflow: Vec::new(),
        };
        write_registry(&control, &registry).unwrap();
        control
    }

    /// Replace a control's registry contents in one durable revision, so one
    /// test can walk several recorded variants of the same fixture.
    fn write_snapshot_registry(control: &Path, rows: Vec<Row>, intents: Vec<Intent>) {
        let mut registry = ready_registry(control).unwrap();
        registry.rows = rows;
        registry.intents = intents;
        registry
            .bump()
            .expect("the fixture registry revision is exhausted");
        write_registry(control, &registry).unwrap();
    }

    fn candidate_of(row: &Row) -> Candidate {
        Candidate {
            checkout_id: row.checkout_id.clone(),
            checkout: row.checkout.clone(),
            receipt: row.receipt.clone(),
            member: row.member.clone(),
            root: row.root.clone(),
            root_kind: row.root_kind,
            volume_uuid: row.volume_uuid.clone(),
            checkout_volume_uuid: None,
            nonce: row.nonce.clone(),
            generation: row.generation,
            dev: 1,
            ino: 2,
            marker_digest: "c".repeat(64),
            logical_bytes: 0,
            allocated_bytes: 0,
            accounting_complete: true,
            receiptless: false,
        }
    }

    /// The journal is a compare-and-set on the revision the locked recheck
    /// read, and every intent phase write waits for the internal publication
    /// lock like every other registry writer.
    #[test]
    fn the_journal_is_revision_checked_and_phase_writes_hold_the_publication_lock() {
        let fixture = Fixture::new();
        let row = row_for(&fixture.root);
        let candidate = candidate_of(&row);
        let control = ready_control(&fixture, vec![row.clone()], Vec::new());
        // The intent pins the checkout receipt's exact bytes and verifies its
        // references against the group, so a committed receipt must exist.
        let receipt_path = outputs::receipt_path_public(&control, &row.checkout_id);
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "oxide.outputs.transition/2",
                "checkout": row.checkout.to_string_lossy(),
                "checkout_id": row.checkout_id,
                "generation": 1,
                "state": "committed",
                "links": [{
                    "member": "target",
                    "selected": {
                        "kind": "external",
                        "root": row.root.to_string_lossy(),
                        "volume_uuid": row.volume_uuid,
                        "nonce": row.nonce,
                        "generation": 1
                    }
                }],
                "updated_ms": 1
            }))
            .unwrap(),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let id = "e".repeat(32);
        let quarantine = fixture.root.join(format!("{QUARANTINE_PREFIX}{id}"));
        let plan = "f".repeat(64);
        assert!(matches!(
            journal_intent(&store, &control, &candidate, &plan, &id, &quarantine, 4),
            Ok(false)
        ));
        let unchanged = ready_registry(&control).unwrap();
        assert_eq!(unchanged.revision, 5);
        assert!(unchanged.intents.is_empty() && unchanged.rows[0].selected);
        match journal_intent(&store, &control, &candidate, &plan, &id, &quarantine, 5) {
            Ok(true) => {}
            Ok(false) => panic!("the journal did not publish (revision changed)"),
            Err(stop) => panic!("the journal refused: {}", stop_text(stop)),
        }
        let journaled = ready_registry(&control).unwrap();
        assert_eq!(journaled.revision, 6);
        assert_eq!(journaled.intent(&id).unwrap().phase, IntentPhase::Intent);
        assert_eq!(journaled.rows[0].pruning.as_deref(), Some(id.as_str()));
        let guard = store::publication_guard(&control).unwrap();
        let writer = {
            let (control, id) = (control.clone(), id.clone());
            std::thread::spawn(move || update_intent(&control, &id, IntentPhase::Renamed))
        };
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            ready_registry(&control).unwrap().intent(&id).unwrap().phase,
            IntentPhase::Intent,
            "a phase write ran while the publication lock was held"
        );
        drop(guard);
        writer.join().unwrap().unwrap();
        assert_eq!(
            ready_registry(&control).unwrap().intent(&id).unwrap().phase,
            IntentPhase::Renamed
        );
    }

    /// Eligibility follows the (checkout, root) owner group: sibling members
    /// of the gone checkout do not keep each other, while a reference from a
    /// DIFFERENT checkout still keeps the root.
    #[test]
    fn eligibility_keeps_a_root_for_another_checkout_but_not_for_sibling_members() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        debug.root_kind = RootKind::Local;
        debug.volume_uuid = None;
        debug.checkout = fixture.root.join("gone");
        debug.receipt = fixture.root.join("receipt.json");
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.selected = false;
        let rows = vec![debug.clone(), release.clone()];
        let control = ready_control(&fixture, rows.clone(), Vec::new());
        let registry = ready_registry(&control).unwrap();
        let receipt_path = outputs::receipt_path_public(&control, &debug.checkout_id);
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "oxide.outputs.transition/2",
                "checkout": debug.checkout.to_string_lossy(),
                "checkout_id": debug.checkout_id,
                "generation": 1,
                "state": "committed",
                "links": [{
                    "member": "target",
                    "selected": {
                        "kind": "local",
                        "root": debug.root.to_string_lossy(),
                        "nonce": debug.nonce,
                        "generation": 1
                    }
                }],
                "updated_ms": 1
            }))
            .unwrap(),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let receipt =
            outputs::read_receipt_for_prune(&control, &debug.checkout, &debug.checkout_id).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let mut budget = SNAPSHOT_MAX_BYTES;
        let mut authority = AuthorityScope::begin().unwrap();
        let reason = eligibility(
            &store,
            &control,
            &registry,
            &debug,
            receipt.as_ref(),
            None,
            &mut authority,
            &mut budget,
            false,
            &mut None,
        )
        .unwrap();
        assert!(
            reason.as_deref() != Some("another checkout's live reference also names this root"),
            "a sibling member kept its own root: {reason:?}"
        );
        // A reference from another checkout keeps the shared root.
        let mut other = debug.clone();
        other.checkout_id = "b".repeat(64);
        other.checkout = fixture.root.join("other-gone");
        let mut with_other = ready_registry(&control).unwrap();
        with_other.rows.push(other);
        let reason = eligibility(
            &store,
            &control,
            &with_other,
            &debug,
            receipt.as_ref(),
            None,
            &mut authority,
            &mut budget,
            false,
            &mut None,
        )
        .unwrap();
        assert_eq!(
            reason.as_deref(),
            Some("another checkout's live reference also names this root"),
            "another checkout's reference did not keep the root"
        );
        let _ = registry;
    }

    /// Two members of one root under the same checkout are ONE owner: the
    /// intent marks and journals the complete group, and rollback restores
    /// every row only while the pinned receipt preimage is unchanged.
    #[test]
    fn a_root_group_is_journaled_and_rolled_back_as_one_owner() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.selected = false;
        let rows = vec![debug.clone(), release.clone()];
        let control = ready_control(&fixture, rows, Vec::new());
        let receipt_path = outputs::receipt_path_public(&control, &debug.checkout_id);
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        let selected = |member: &str| {
            serde_json::json!({
                "member": member,
                "selected": {
                    "kind": "external",
                    "root": debug.root.to_string_lossy(),
                    "volume_uuid": debug.volume_uuid,
                    "nonce": debug.nonce,
                    "generation": 1
                }
            })
        };
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "oxide.outputs.transition/3",
                "checkout": debug.checkout.to_string_lossy(),
                "checkout_id": debug.checkout_id,
                "generation": 1,
                "state": "committed",
                "layout": "profile",
                // `release` is INACTIVE history for this root in the registry
                // (selected = false), so the receipt's current references are
                // exactly `debug`. P2 requires that exact agreement; the group
                // still holds both rows and the rollback restores each role.
                "links": [selected("debug")],
                "updated_ms": 1
            }))
            .unwrap(),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let candidate = candidate_of(&debug);
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let id = "a".repeat(32);
        let quarantine = fixture.root.join(format!("{QUARANTINE_PREFIX}{id}"));
        let plan = "f".repeat(64);
        let revision = ready_registry(&control).unwrap().revision;
        match journal_intent(
            &store,
            &control,
            &candidate,
            &plan,
            &id,
            &quarantine,
            revision,
        ) {
            Ok(true) => {}
            Ok(false) => panic!("the group journal did not publish"),
            Err(stop) => panic!("the group journal refused: {}", stop_text(stop)),
        }
        let journaled = ready_registry(&control).unwrap();
        let intent = journaled.intent(&id).unwrap();
        assert_eq!(
            intent.group.len(),
            2,
            "the group did not record both members"
        );
        assert_eq!(
            intent
                .group
                .iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            ["debug", "release"]
        );
        for row in journaled.rows.iter().filter(|row| row.root == debug.root) {
            assert_eq!(
                row.pruning.as_deref(),
                Some(id.as_str()),
                "{} was not marked",
                row.member
            );
            assert!(!row.selected);
        }
        // Restoring the recorded preimage rolls back BOTH rows.
        rollback_intent(&store, &control, intent).unwrap();
        let rolled = ready_registry(&control).unwrap();
        for row in rolled.rows.iter().filter(|row| row.root == debug.root) {
            assert!(row.pruning.is_none(), "{} kept a pruning mark", row.member);
            if row.member == "debug" {
                assert!(row.selected);
            }
        }
        // A changed receipt keeps the root: the second journal must roll back
        // to an error, leaving both rows marked.
        let revision = ready_registry(&control).unwrap().revision;
        match journal_intent(
            &store,
            &control,
            &candidate,
            &plan,
            &"b".repeat(32),
            &quarantine,
            revision,
        ) {
            Ok(true) => {}
            Ok(false) => panic!("the second group journal did not publish"),
            Err(stop) => panic!("the second group journal refused: {}", stop_text(stop)),
        }
        let mut changed = fs::read(&receipt_path).unwrap();
        changed.push(b'\n');
        fs::write(&receipt_path, &changed).unwrap();
        let intent = ready_registry(&control)
            .unwrap()
            .intent(&"b".repeat(32))
            .cloned()
            .unwrap();
        assert!(
            rollback_intent(&store, &control, &intent).is_err(),
            "rollback restored rows for a changed receipt"
        );
        let kept = ready_registry(&control).unwrap();
        for row in kept.rows.iter().filter(|row| row.root == debug.root) {
            assert_eq!(
                row.pruning.as_deref(),
                Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
                "{} was restored",
                row.member
            );
        }
    }

    /// Write one real owner marker for `row` and observe its fixture root the
    /// way the product observes a recorded root: the descriptor's metadata, the
    /// parsed marker and the marker's own digest.
    fn write_root_marker(root: &Path, document: &serde_json::Value) {
        fs::write(
            root.join(outputs::MARKER),
            serde_json::to_vec(document).unwrap(),
        )
        .unwrap();
    }

    fn root_marker(row: &Row) -> serde_json::Value {
        serde_json::json!({
            "version": "oxide.outputs.root/2",
            "checkout": row.checkout.to_string_lossy(),
            "checkout_id": row.checkout_id,
            "label": "app",
            "recipe": outputs::RECIPE_PUBLIC,
            "kind": "external",
            "volume_uuid": row.volume_uuid,
            "nonce": row.nonce,
            "generation": row.generation,
            "created_ms": 1u64,
        })
    }

    fn observe_fixture_root(row: &Row) -> ObservedIdentity {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(&row.root).unwrap();
        let marker = outputs::read_marker_for_prune(&row.root).unwrap();
        ObservedIdentity {
            dev: metadata.dev(),
            ino: metadata.ino(),
            legacy: marker.is_legacy(),
            marker,
            marker_digest: outputs::marker_digest(&row.root).unwrap(),
        }
    }

    fn fixture_stat(path: &Path) -> libc::stat {
        let fd: std::os::fd::OwnedFd = fs::File::open(path).unwrap().into();
        fstat_fd(&fd).unwrap()
    }

    /// The reason a replay refused. A replay that returned a verdict where the
    /// test requires a refusal is itself a failure.
    fn replay_refusal(result: Result<Resumed, String>, what: &str) -> String {
        match result {
            Err(reason) => reason,
            Ok(_) => panic!("{what}"),
        }
    }

    /// A reboot or a reattach renumbers a mounted volume's `st_dev`. The
    /// unchanged root is still the recorded one — same inode, same marker, same
    /// verified volume — so its stale device is reconciled instead of making the
    /// row unknown, while a changed inode never is.
    #[test]
    fn a_renumbered_device_reconciles_an_unchanged_root() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        fs::create_dir_all(&row.root).unwrap();
        write_root_marker(&row.root, &root_marker(&row));
        let observed = observe_fixture_root(&row);
        assert_eq!(row.root_dev, Some(1));
        assert_ne!(
            observed.dev, 1,
            "the fixture root reported the stale recorded device"
        );
        assert_eq!(
            row.root_ino,
            Some(2),
            "the fixture row must start from a stale inode"
        );
        // The stale inode still keeps the root; the fixed inode reconciles the
        // stale device and reports the device this run binds.
        assert_eq!(
            judge_recorded_root(&row, &observed).unwrap_err(),
            "the recorded root's inode changed"
        );
        row.root_ino = Some(observed.ino);
        row.marker_digest = Some(observed.marker_digest.clone());
        assert_eq!(judge_recorded_root(&row, &observed), Ok(observed.dev));
        // A renumbered device is never accepted on its own: an inode that
        // changed stays a different root.
        let mut changed = row.clone();
        changed.root_ino = Some(observed.ino + 1);
        assert_eq!(
            judge_recorded_root(&changed, &observed).unwrap_err(),
            "the recorded root's inode changed"
        );
        // And the same root observed a second time (a later run in the same boot
        // session, where the number no longer differs) binds the same device.
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)),
            Ok(observed.dev)
        );
    }

    /// Every identity fact except the device still has to match: another volume's
    /// marker, a different nonce or generation, and a changed marker digest each
    /// keep the root unknown.
    #[test]
    fn a_changed_volume_nonce_generation_or_marker_keeps_the_root_unknown() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        fs::create_dir_all(&row.root).unwrap();
        write_root_marker(&row.root, &root_marker(&row));
        let observed = observe_fixture_root(&row);
        row.root_ino = Some(observed.ino);
        row.marker_digest = Some(observed.marker_digest.clone());
        assert_eq!(judge_recorded_root(&row, &observed), Ok(observed.dev));

        // Another volume: the marker names a different UUID than the row was
        // recorded with. A copied root is not the recorded one even when its
        // device number happens to be unchanged.
        let mut other_volume = root_marker(&row);
        other_volume["volume_uuid"] = serde_json::json!("99999999-8888-7777-6666-555555555555");
        write_root_marker(&row.root, &other_volume);
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );

        // A different nonce is a different root creation.
        let mut other_nonce = root_marker(&row);
        other_nonce["nonce"] = serde_json::json!("e".repeat(32));
        write_root_marker(&row.root, &other_nonce);
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );

        // A different generation is a different root creation too.
        let mut other_generation = root_marker(&row);
        other_generation["generation"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &other_generation);
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );

        // The recorded digest is the preimage pin: the same identity facts with an
        // edited marker document is still a changed marker.
        write_root_marker(&row.root, &root_marker(&row));
        let mut edited = root_marker(&row);
        edited["created_ms"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &edited);
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker changed since it was recorded"
        );
        // A marker that matches again reconciles, so the digest pin is not a
        // one-way refusal.
        write_root_marker(&row.root, &root_marker(&row));
        assert_eq!(
            judge_recorded_root(&row, &observe_fixture_root(&row)),
            Ok(observed.dev)
        );

        // A row that never recorded a device predates this identity entirely;
        // only the explicit operator list may consider it and no number is
        // invented here.
        let mut unrecorded = row.clone();
        unrecorded.root_dev = None;
        assert_eq!(
            judge_recorded_root(&unrecorded, &observe_fixture_root(&row)).unwrap_err(),
            "the recorded root has no device"
        );
    }

    /// oxide#218: before oxide#152 a transition recorded the receipt's
    /// COMMITTED selection generation in the row, which advances with every
    /// transition, while the owner marker keeps the generation the root was
    /// created at. That superseded shape is reconciled only from the EXACT
    /// recorded marker digest: a row without one, a marker that changed, a
    /// nonce that disagrees, and a recorded generation older than the marker's
    /// all stay unknown.
    #[test]
    fn a_pre_152_selection_generation_reconciles_only_from_the_recorded_marker() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        fs::create_dir_all(&row.root).unwrap();
        write_root_marker(&row.root, &root_marker(&row));
        let observed = observe_fixture_root(&row);
        row.root_ino = Some(observed.ino);
        row.marker_digest = Some(observed.marker_digest.clone());
        assert_eq!(judge_recorded_root(&row, &observed), Ok(observed.dev));

        // The marker was created at generation 1 and the row records a LATER
        // committed generation (the live praxis-5f7d5240 and effigy-47de65c1
        // shapes: 2 and 3).
        let mut superseded = row.clone();
        superseded.generation = 2;
        assert_eq!(
            judge_recorded_root(&superseded, &observed),
            Ok(observed.dev)
        );
        let mut later = row.clone();
        later.generation = 3;
        assert_eq!(judge_recorded_root(&later, &observed), Ok(observed.dev));

        // The recorded digest is the pin. Without one the row carries no
        // evidence that the marker document is the recorded one.
        let mut unpinned = superseded.clone();
        unpinned.marker_digest = None;
        assert_eq!(
            judge_recorded_root(&unpinned, &observed).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );
        // Another nonce is another root creation, whatever the generation says.
        let mut other_nonce = root_marker(&row);
        other_nonce["nonce"] = serde_json::json!("e".repeat(32));
        write_root_marker(&row.root, &other_nonce);
        assert_eq!(
            judge_recorded_root(&superseded, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );
        // An edited marker document is a changed marker: its digest no longer
        // pins the recorded one, so nothing is reconciled.
        write_root_marker(&row.root, &root_marker(&row));
        let recorded = observe_fixture_root(&row);
        let mut edited = root_marker(&row);
        edited["created_ms"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &edited);
        assert_eq!(
            judge_recorded_root(&superseded, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );
        // A recorded generation OLDER than the marker's is never a superseded
        // selection: nothing writes one, so the marker still keeps the root.
        let mut ahead = row.clone();
        ahead.generation = 1;
        let mut newer_marker = root_marker(&row);
        newer_marker["generation"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &newer_marker);
        assert_eq!(
            judge_recorded_root(&ahead, &observe_fixture_root(&row)).unwrap_err(),
            "the owner marker does not match the recorded root identity"
        );
        // A superseded generation is never a way past a failed inode check.
        write_root_marker(&row.root, &root_marker(&row));
        let mut moved_inode = superseded.clone();
        moved_inode.root_ino = Some(observed.ino + 1);
        assert_eq!(
            judge_recorded_root(&moved_inode, &recorded).unwrap_err(),
            "the recorded root's inode changed"
        );
    }

    /// oxide#218: the pre-effect marker verification applies the SAME superseded
    /// -generation rule as the observation, so a preview that named a pre-#152
    /// candidate can perform exactly the effect it named. A changed marker,
    /// another nonce and a generation older than the marker's still refuse.
    #[test]
    fn the_pre_effect_marker_check_accepts_only_the_superseded_generation() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        fs::create_dir_all(&row.root).unwrap();
        write_root_marker(&row.root, &root_marker(&row));
        let observed = observe_fixture_root(&row);
        row.root_dev = Some(observed.dev);
        row.root_ino = Some(observed.ino);
        row.marker_digest = Some(observed.marker_digest.clone());
        let root: OwnedFd = fs::File::open(&row.root).unwrap().into();
        let mut candidate = candidate_of(&row);
        candidate.dev = observed.dev;
        candidate.ino = observed.ino;
        candidate.marker_digest = observed.marker_digest.clone();
        assert_eq!(candidate.generation, 1);
        assert_eq!(verify_root_marker(&root, &candidate), Ok(()));

        // The pre-#152 row's commit generation 2 beside the marker's creation
        // generation 1, with the exact recorded bytes: accepted, because the
        // observation already attributed this candidate.
        let mut superseded = candidate.clone();
        superseded.generation = 2;
        assert_eq!(verify_root_marker(&root, &superseded), Ok(()));

        // A changed marker document is never accepted.
        let mut edited = root_marker(&row);
        edited["created_ms"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &edited);
        assert_eq!(
            verify_root_marker(&root, &superseded).unwrap_err(),
            "the owner marker changed before the quarantine rename"
        );
        write_root_marker(&row.root, &root_marker(&row));
        // Another nonce is another root creation.
        let mut other_nonce = superseded.clone();
        other_nonce.nonce = Some("e".repeat(32));
        assert_eq!(
            verify_root_marker(&root, &other_nonce).unwrap_err(),
            "the owner marker does not match the plan"
        );
        // A candidate generation OLDER than the marker's is never reconciled.
        let mut newer_marker = root_marker(&row);
        newer_marker["generation"] = serde_json::json!(2u64);
        write_root_marker(&row.root, &newer_marker);
        let mut ahead = candidate.clone();
        ahead.marker_digest = outputs::marker_digest(&row.root).unwrap();
        assert_eq!(
            verify_root_marker(&root, &ahead).unwrap_err(),
            "the owner marker does not match the plan"
        );
        // The marker's own creation generation matches.
        let mut matching = ahead.clone();
        matching.generation = 2;
        assert_eq!(verify_root_marker(&root, &matching), Ok(()));
    }

    /// oxide#218: an inactive history reference is never a group's
    /// representative while the group still has a live one, so a superseded
    /// legacy `target` row (an old nonce or generation) cannot make the whole
    /// snapshot incomplete for a group whose current references are fine.
    #[test]
    fn a_group_representative_is_its_first_live_reference() {
        let fixture = Fixture::new();
        let history = |member: &str| {
            let mut row = row_for(&fixture.root);
            row.member = member.to_owned();
            row.selected = false;
            row.pending = false;
            row.nonce = None;
            row
        };
        let live = |member: &str| {
            let mut row = row_for(&fixture.root);
            row.member = member.to_owned();
            row
        };
        // `target` sorts before `zulu/release`, and the history row is not the
        // representative.
        let rows = representatives(vec![history("target"), live("zulu/release")]);
        assert_eq!(
            rows.iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            ["zulu/release"]
        );
        // Two live references keep the first member in sorted order, and one
        // group is still exactly one representative.
        let rows = representatives(vec![live("release"), live("debug"), history("target")]);
        assert_eq!(
            rows.iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            ["debug"]
        );
        // A group with no live reference keeps its first member: a released
        // root has only its recorded history.
        let rows = representatives(vec![history("target"), history("alpha")]);
        assert_eq!(
            rows.iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            ["alpha"]
        );
        // Different roots and different checkouts stay separate groups.
        let mut other_root = live("debug");
        other_root.root = fixture.root.join("elsewhere");
        let mut other_checkout = live("debug");
        other_checkout.checkout_id = "b".repeat(64);
        let rows = representatives(vec![live("debug"), other_root, other_checkout]);
        assert_eq!(rows.len(), 3);
    }

    /// The real-run plan gate: a plan made before a reboot still binds the same
    /// unchanged roots, and every other recorded fact still refuses it.
    #[test]
    fn a_plan_made_before_a_renumbered_device_still_binds_the_same_roots() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        fs::create_dir_all(&row.root).unwrap();
        write_root_marker(&row.root, &root_marker(&row));
        let observed = observe_fixture_root(&row);
        row.root_dev = Some(1);
        row.root_ino = Some(observed.ino);
        row.marker_digest = Some(observed.marker_digest.clone());
        let planned = candidate_of(&row);
        let mut fresh = planned.clone();
        fresh.dev = observed.dev;
        assert_ne!(planned.dev, fresh.dev, "the fixture did not renumber");
        assert!(same_candidates(&[&planned], &[&fresh]));

        // The same root observed under another volume has a different marker
        // digest (the marker records the volume UUID), so the plan refuses it.
        let mut swapped = fresh.clone();
        swapped.marker_digest = "9".repeat(64);
        assert!(!same_candidates(&[&planned], &[&swapped]));
        let mut reused = fresh.clone();
        reused.ino = observed.ino + 1;
        assert!(!same_candidates(&[&planned], &[&reused]));
        let mut recreated = fresh.clone();
        recreated.nonce = Some("d".repeat(32));
        assert!(!same_candidates(&[&planned], &[&recreated]));
        // A root that left the plan is a different set, never a silent match.
        assert!(!same_candidates(&[&planned], &[]));
    }

    /// A pending intent and a crash replay across a renumbering, through the
    /// production replay path. `resume_intent` refuses when it cannot positively
    /// reopen the recorded root; `replay_journaled_root` — the one function it
    /// calls once a reopen succeeds — rolls the journal back exactly, and a
    /// swapped volume is never rolled back.
    #[test]
    fn a_pending_intent_replays_across_a_renumbered_device() {
        let fixture = Fixture::new();
        let mut root = row_for(&fixture.root);
        root.root = fixture
            .root
            .join("mnt")
            .join(outputs::EXTERNAL_DIR)
            .join("app");
        fs::create_dir_all(&root.root).unwrap();
        write_root_marker(&root.root, &root_marker(&root));
        let observed = observe_fixture_root(&root);
        root.root_ino = Some(observed.ino);
        root.marker_digest = Some(observed.marker_digest.clone());
        let mut debug = root.clone();
        debug.member = "debug".to_owned();
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.selected = false;
        let control = ready_control(&fixture, vec![debug.clone(), release.clone()], Vec::new());
        let receipt_path = outputs::receipt_path_public(&control, &debug.checkout_id);
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "oxide.outputs.transition/3",
                "checkout": debug.checkout.to_string_lossy(),
                "checkout_id": debug.checkout_id,
                "generation": 1,
                "state": "committed",
                "layout": "profile",
                "links": [{
                    "member": "debug",
                    "selected": {
                        "kind": "external",
                        "root": debug.root.to_string_lossy(),
                        "volume_uuid": debug.volume_uuid,
                        "nonce": debug.nonce,
                        "generation": 1
                    }
                }],
                "updated_ms": 1
            }))
            .unwrap(),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let journal = |id: &str| -> Intent {
            // The row records the stale number a pre-reboot write left behind; the
            // candidate carries this run's own observation.
            let mut candidate = candidate_of(&debug);
            candidate.dev = observed.dev;
            candidate.ino = observed.ino;
            candidate.marker_digest = observed.marker_digest.clone();
            let quarantine = fixture.root.join(format!("{QUARANTINE_PREFIX}{id}"));
            let revision = ready_registry(&control).unwrap().revision;
            match journal_intent(
                &store,
                &control,
                &candidate,
                &"f".repeat(64),
                id,
                &quarantine,
                revision,
            ) {
                Ok(true) => {}
                Ok(false) => panic!("the journal did not publish"),
                Err(stop) => panic!("the journal refused: {}", stop_text(stop)),
            }
            ready_registry(&control)
                .unwrap()
                .intent(id)
                .cloned()
                .unwrap()
        };
        let id = "a".repeat(32);
        let intent = journal(&id);
        assert_eq!(intent.identity.dev, observed.dev);
        assert_eq!(intent.group.len(), 2);
        assert!(
            intent
                .group
                .iter()
                .all(|row| row.root_dev == Some(observed.dev)),
            "the intent kept a stale device for its own group"
        );
        assert!(
            ready_registry(&control)
                .unwrap()
                .rows
                .iter()
                .filter(|row| row.root == debug.root)
                .all(|row| row.root_dev == Some(observed.dev)),
            "the registry publication did not refresh the group's device"
        );

        // The reboot, after the publication: the journal and every row of the
        // group keep the number they recorded, and the volume reports a new one.
        let live_dev = u64::try_from(fixture_stat(&root.root).st_dev).unwrap();
        let stale_dev = live_dev + 41;
        let mut rebooted = ready_registry(&control).unwrap();
        for row in rebooted
            .rows
            .iter_mut()
            .filter(|row| row.root == debug.root)
        {
            row.root_dev = Some(stale_dev);
        }
        for record in rebooted.intents.iter_mut() {
            record.identity.dev = stale_dev;
            for member in record.group.iter_mut() {
                member.root_dev = Some(stale_dev);
            }
        }
        rebooted.bump().unwrap();
        write_registry(&control, &rebooted).unwrap();
        let intent = ready_registry(&control)
            .unwrap()
            .intent(&id)
            .cloned()
            .unwrap();
        assert_ne!(
            intent.identity.dev, live_dev,
            "the fixture did not renumber"
        );

        // The mount half of a replay: the journal's number is stale, so the reopen
        // binds the recorded inode on the verified volume and a fresh candidate
        // still has to report its own observed device.
        let stat = fixture_stat(&root.root);
        assert_eq!(
            judge_root_identity(&stat, live_dev, None, observed.ino),
            Ok(live_dev)
        );
        assert_eq!(
            judge_root_identity(&stat, live_dev, Some(live_dev + 1), observed.ino).unwrap_err(),
            "the root's device changed"
        );
        assert_eq!(
            judge_root_identity(&stat, live_dev, None, observed.ino + 1).unwrap_err(),
            "the root's inode changed"
        );
        assert_eq!(
            judge_root_identity(&stat, live_dev + 1, None, observed.ino).unwrap_err(),
            "the root is not on the verified volume"
        );

        // The production replay entry point refuses when neither the recorded path
        // nor the quarantine leaf can be positively reopened (no `/Volumes` mount
        // exists here), and it settles nothing.
        let refused = replay_refusal(
            resume_intent(&store, &control, &intent),
            "the real replay settled a journal it could not positively reopen",
        );
        assert!(refused.contains("not at its path"), "{refused}");
        assert!(refused.contains("in its quarantine"), "{refused}");
        let kept = ready_registry(&control).unwrap();
        assert!(
            kept.intent(&id).is_some(),
            "a refused replay settled its journal"
        );
        assert!(
            kept.rows
                .iter()
                .filter(|row| row.root == debug.root)
                .all(|row| row.pruning.as_deref() == Some(id.as_str()) && !row.selected),
            "a refused replay changed the registry"
        );

        // The decision and effects `resume_intent` runs once the reopen succeeds:
        // the root at its recorded path carries the journal's own marker, so both
        // members are restored and the journal is removed.
        let replayed = replay_journaled_root(
            &store,
            &control,
            &intent,
            &journal_candidate(&intent),
            Reopened::Original(OpenedRoot::Fixture(
                fs::File::open(&root.root).unwrap().into(),
            )),
        )
        .unwrap();
        assert!(matches!(replayed, Resumed::RolledBack));
        let rolled = ready_registry(&control).unwrap();
        assert!(
            rolled.intent(&id).is_none(),
            "the rolled-back journal remains"
        );
        for row in rolled.rows.iter().filter(|row| row.root == debug.root) {
            assert!(row.pruning.is_none(), "{} kept a pruning mark", row.member);
            // The rollback restores the journal's own recorded facts; it never
            // invents the live device.
            assert_eq!(row.root_dev, Some(stale_dev));
        }
        assert!(
            rolled
                .rows
                .iter()
                .find(|row| row.member == "debug")
                .unwrap()
                .selected,
            "the rollback did not restore the selected member"
        );

        // The quarantine branch is the deletion path: a replay without a verified
        // session never resumes a deletion, even with the journal's own marker, so
        // the intent stays pending.
        let id = "b".repeat(32);
        let swapped_intent = journal(&id);
        let quarantine_refusal = replay_refusal(
            replay_journaled_root(
                &store,
                &control,
                &swapped_intent,
                &journal_candidate(&swapped_intent),
                Reopened::Quarantine(OpenedRoot::Fixture(
                    fs::File::open(&root.root).unwrap().into(),
                )),
            ),
            "a session-less replay resumed a deletion",
        );
        assert!(
            quarantine_refusal.contains("never resumes a deletion"),
            "{quarantine_refusal}"
        );
        assert!(
            ready_registry(&control).unwrap().intent(&id).is_some(),
            "a session-less deletion replay settled its journal"
        );

        // A swapped volume is never rolled back: the marker now at the recorded
        // path belongs to another volume, so its preimage is not the journal's. The
        // replay refuses and leaves the journal and every row exactly as the
        // publication wrote them.
        let mut swapped = root_marker(&debug);
        swapped["volume_uuid"] = serde_json::json!("99999999-8888-7777-6666-555555555555");
        write_root_marker(&root.root, &swapped);
        assert_ne!(
            observe_fixture_root(&root).marker.volume_uuid(),
            swapped_intent.identity.volume_uuid.as_deref(),
            "the fixture is not a swapped volume"
        );
        let refused = replay_refusal(
            replay_journaled_root(
                &store,
                &control,
                &swapped_intent,
                &journal_candidate(&swapped_intent),
                Reopened::Original(OpenedRoot::Fixture(
                    fs::File::open(&root.root).unwrap().into(),
                )),
            ),
            "a swapped volume was rolled back",
        );
        assert_eq!(
            refused, "the owner marker changed before the quarantine rename",
            "a swapped volume was reconciled"
        );
        let kept = ready_registry(&control).unwrap();
        assert!(
            kept.intent(&id).is_some(),
            "a swapped volume settled its journal"
        );
        assert!(
            kept.rows
                .iter()
                .filter(|row| row.root == debug.root)
                .all(|row| row.pruning.as_deref() == Some(id.as_str()) && !row.selected),
            "a swapped volume changed the registry"
        );
    }

    /// Preparing a new root must not append the already selected member again.
    /// A sibling can still carry a previous session's device observation.
    #[test]
    fn prepared_placement_does_not_duplicate_a_shared_root_with_stale_device() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        fs::create_dir(&debug.root).unwrap();
        fs::write(debug.root.join(".oxide-output.json"), b"fixture marker").unwrap();
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(&debug.root).unwrap();
        debug.root_dev = Some(metadata.dev());
        debug.root_ino = Some(metadata.ino());
        debug.marker_digest = Some(outputs::marker_digest(&debug.root).unwrap());
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.root_dev = Some(metadata.dev().wrapping_add(1));
        let control = ready_control(&fixture, vec![debug.clone(), release.clone()], Vec::new());
        let mut document = snapshot_receipt(
            &debug,
            "oxide.outputs.transition/3",
            "prepared",
            Some(&debug.root),
        );
        document["links"][0]["selected"]["nonce"] = serde_json::json!(debug.nonce);
        document["links"][0]["prepared"] = serde_json::json!({
            "generation": 2,
            "to": {"kind": "external", "root": fixture.root.join("future"),
                   "volume_uuid": debug.volume_uuid, "nonce": "d".repeat(32), "generation": 2},
            "link": debug.checkout.join("target/debug")
        });
        publish_snapshot_receipt(&control, &debug, &document);
        let receipt =
            outputs::read_receipt_for_prune(&control, &debug.checkout, &debug.checkout_id)
                .unwrap()
                .unwrap();
        let mut registry = ready_registry(&control).unwrap();
        note_placement(
            &mut registry,
            &receipt,
            &receipt.links()[0],
            &debug.receipt,
            None,
        )
        .unwrap();
        let members = group_rows(&registry, &debug.checkout_id, &debug.root);
        assert_eq!(members.len(), 2, "preparing duplicated the selected member");
        assert_eq!(members[0].member, "debug");
        assert_eq!(members[1].member, "release");
        assert_eq!(members[1].root_dev, release.root_dev);
        let mut candidate = candidate_of(&debug);
        candidate.dev = metadata.dev();
        candidate.ino = metadata.ino();
        candidate.marker_digest = debug.marker_digest.clone().unwrap();
        assert_eq!(verified_group(&registry, &candidate).unwrap().len(), 2);
        assert_eq!(
            registry
                .rows
                .iter()
                .filter(|row| row.root == fixture.root.join("future") && row.pending)
                .count(),
            1
        );
    }

    /// A group with a stale recorded device is ONE owner. The selected references
    /// may disagree about the observational number (a sibling written in an
    /// earlier session) and still be the same root, while a disagreement about
    /// the inode, nonce or digest still refuses.
    #[test]
    fn a_group_tolerates_only_the_observational_device() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        debug.root_dev = Some(1);
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.root_dev = Some(777);
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 3,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![debug.clone(), release.clone()],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let mut candidate = candidate_of(&debug);
        candidate.dev = 777;
        candidate.ino = 2;
        candidate.marker_digest = "c".repeat(64);
        assert!(verified_group(&registry, &candidate).is_ok());
        let mut changed = registry.clone();
        for row in &mut changed.rows {
            if row.member == "release" {
                row.root_ino = Some(9);
            }
        }
        let refused = verified_group(&changed, &candidate).unwrap_err();
        assert!(refused.contains("disagree about its identity"), "{refused}");
        let mut changed = registry.clone();
        for row in &mut changed.rows {
            if row.member == "release" {
                row.nonce = Some("d".repeat(32));
            }
        }
        assert!(verified_group(&changed, &candidate).is_err());
        // The candidate's own inode and digest are the plan's identity.
        let mut stale_inode = candidate.clone();
        stale_inode.ino = 3;
        assert!(verified_group(&registry, &stale_inode).is_err());
    }

    /// A deleted or resolved intent whose checkout receipt has not committed
    /// its terminal state is still unfinished, so prune, adopt and
    /// resolve-intent all replay that step instead of treating it as settled.
    #[test]
    fn a_terminal_intent_stays_unfinished_until_its_receipt_settles() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        row.selected = false;
        row.pruned_ms = Some(1);
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: "b".repeat(32),
            identity: RootIdentity {
                checkout_id: row.checkout_id.clone(),
                checkout: row.checkout.clone(),
                receipt: row.receipt.clone(),
                member: row.member.clone(),
                root: row.root.clone(),
                root_kind: RootKind::External,
                volume_uuid: row.volume_uuid.clone(),
                nonce: row.nonce.clone(),
                generation: 1,
                dev: 1,
                ino: 2,
                marker_digest: "c".repeat(64),
            },
            phase: IntentPhase::Deleted,
            quarantine: fixture.root.join(".prune-b"),
            preview_sha256: "f".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: true,
            prior_receipt_sha: String::new(),
            group: Vec::new(),
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        let checkout_id = row.checkout_id.clone();
        let control = ready_control(&fixture, vec![row], vec![intent.clone()]);
        assert_eq!(
            unsettled_for(&control, &checkout_id).unwrap(),
            Some(intent.id.clone())
        );
        for phase in [IntentPhase::Deleted, IntentPhase::Resolved] {
            let unsettled = Intent {
                phase,
                ..intent.clone()
            };
            assert!(!unsettled.settled());
            assert!(Intent {
                receipt_settled: true,
                ..unsettled
            }
            .settled());
        }
        assert!(!Intent {
            phase: IntentPhase::Renamed,
            receipt_settled: true,
            ..intent
        }
        .settled());
    }

    /// After durable overflow, a tagged reservation is valid against the
    /// exact sticky-incomplete registry without any row; another registry or
    /// a begun cancellation refuses, and finishing the cancellation clears
    /// only its journal entry. A complete registry refuses an overflow tag.
    #[test]
    fn profile_reservation_overflow_binds_by_tag_and_cancels_without_a_row() {
        let fixture = Fixture::new();
        let control = ready_control(&fixture, Vec::new(), Vec::new());
        let mut registry = ready_registry(&control).unwrap();
        let id = fresh_nonce().unwrap();
        let nonce = fresh_nonce().unwrap();
        assert!(overflow_tag(&control, &id, &nonce).unwrap().is_none());
        mark_incomplete(&mut registry, "test overflow").unwrap();
        write_registry(&control, &registry).unwrap();
        let tag = overflow_tag(&control, &id, &nonce).unwrap().unwrap();
        assert_eq!(tag.registry, registry.created_by);
        let reserved = outputs::Reservation {
            id: id.clone(),
            nonce,
            kind: RootKind::External,
            root: fixture.root.join("vol/oxide-outputs/app"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            generation: 1,
            created_ms: 1,
            overflow: Some(tag.clone()),
        };
        validate_reservation(&control, &reserved).unwrap();
        // An unrelated revision advance does not refuse it.
        let mut later = ready_registry(&control).unwrap();
        later.bump().unwrap();
        write_registry(&control, &later).unwrap();
        validate_reservation(&control, &reserved).unwrap();
        let mut foreign = reserved.clone();
        foreign.overflow.as_mut().unwrap().registry = "f".repeat(32);
        assert!(validate_reservation(&control, &foreign).is_err());
        let checkout_id = "a".repeat(64);
        begin_cancel(&control, &checkout_id, &reserved).unwrap();
        assert!(validate_reservation(&control, &reserved).is_err());
        assert!(ready_registry(&control).unwrap().rows.is_empty());
        finish_cancel(&control, &checkout_id, &reserved).unwrap();
        assert!(ready_registry(&control)
            .unwrap()
            .cancelled_overflow
            .is_empty());
    }

    /// The tombstone ring is diagnostics: oldest entries compact first to the
    /// entry and byte caps, and a corrupt ring is replaced, never trusted.
    #[test]
    fn the_tombstone_ring_compacts_oldest_first_within_its_caps() {
        let fixture = Fixture::new();
        let control = ready_control(&fixture, Vec::new(), Vec::new());
        let entry = |n: usize| Tombstone {
            intent: format!("{n:032x}"),
            checkout_id: "a".repeat(64),
            root: PathBuf::from("/vol/oxide-outputs/app"),
            nonce: None,
            outcome: "deleted".to_owned(),
            at_ms: n as u64,
        };
        let mut ring = Tombstones {
            version: TOMBSTONES_VERSION.to_owned(),
            entries: (0..MAX_TOMBSTONES + 10).map(entry).collect(),
        };
        compact_tombstones(&mut ring).unwrap();
        assert!(ring.entries.len() <= MAX_TOMBSTONES);
        assert!(serde_json::to_vec(&ring).unwrap().len() <= MAX_TOMBSTONE_BYTES);
        assert_eq!(
            ring.entries.last().unwrap().at_ms,
            (MAX_TOMBSTONES + 9) as u64
        );
        assert!(ring.entries.first().unwrap().at_ms >= 10);
        fs::write(registry_dir(&control).join(TOMBSTONES_FILE), b"{corrupt").unwrap();
        append_tombstone(&control, entry(1)).unwrap();
        let bytes = fs::read(registry_dir(&control).join(TOMBSTONES_FILE)).unwrap();
        let read: Tombstones = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(read.entries.len(), 1);
    }

    /// The legacy exception is volume-based: a checkout whose nearest
    /// existing ancestor is on the boot Data volume qualifies (scratch under
    /// /private/tmp is), and a missing ancestry chain is walked up, never
    /// followed. The fixture comes from the explicit internal root
    /// (`OXIDE_TEST_INTERNAL_ROOT`, AGENTS.md's Rust test recipe) because a
    /// leased child's `TMPDIR` is on the selected pool's volume; the product's
    /// own check proves that root qualifies, so a wrong root fails here. The
    /// EACCES fixture is inert without its proof environment.
    #[cfg(target_os = "macos")]
    #[test]
    fn legacy_eligibility_is_proved_by_the_boot_data_volume() {
        let fixture = Fixture::at_internal_root();
        let mut row = row_for(&fixture.root);
        row.checkout = fixture.root.join("gone/deeper/checkout");
        row.checkout_volume_uuid = None;
        let Absence::Absent(witness) = checkout_absence(&row, true).unwrap() else {
            panic!("the legacy exception did not prove the boot Data ancestry");
        };
        assert_eq!(witness.missing, Some(witness.observed.len()));
        let data = witness
            .boot_data
            .clone()
            .expect("the legacy rule holds the boot Data facts");
        assert_eq!(witness.ancestor, data);
        assert_eq!(data.fstype, "apfs");
        // The same legacy row is never eligible as a registered root: the
        // genuine exception is the only one that may lack a recorded volume.
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Unknown(_)
        ));
        assert!(!injected_eacces(&fixture.root));
    }

    /// The missing-parent extension: a checkout whose ancestors are gone is
    /// positively absent when the held nearest existing ancestor is a readable
    /// directory on the boot Data volume. The recipe's explicit internal root
    /// is the premise here (the proof is about the internal volume), never the
    /// pool's own volume.
    #[test]
    fn a_missing_ancestor_chain_on_the_boot_data_volume_is_proved_absent() {
        let fixture = Fixture::at_internal_root();
        let mut row = row_for(&fixture.root);
        row.checkout = fixture.root.join("first").join("second").join("checkout");
        row.checkout_volume_uuid = checkout_volume_uuid(&fixture.root);
        let Absence::Absent(witness) = checkout_absence(&row, false).unwrap() else {
            panic!("a boot Data missing chain did not prove absence");
        };
        assert_eq!(witness.missing, Some(witness.observed.len()));
        assert_eq!(
            witness.observed.last().map(|entry| entry.name.clone()),
            fixture.root.file_name().map(std::ffi::OsStr::to_os_string)
        );
        assert_eq!(
            witness.boot_data.clone().expect("the held boot Data proof"),
            witness.ancestor
        );
        // The genuine legacy exception proves the same chain with no recorded
        // volume, because the boot Data ancestry is its own rule.
        let mut legacy = row.clone();
        legacy.checkout_volume_uuid = None;
        let Absence::Absent(legacy_witness) = checkout_absence(&legacy, true).unwrap() else {
            panic!("the legacy exception did not prove the Data ancestry");
        };
        assert_eq!(legacy_witness, witness);
        // A registered row whose recorded volume conflicts is never waived by
        // today's Data ancestor.
        let mut conflicting = row.clone();
        conflicting.checkout_volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        assert!(matches!(
            checkout_absence(&conflicting, false).unwrap(),
            Absence::Unknown(_)
        ));
        // Recreating the checkout is presence, never absence.
        fs::create_dir_all(&row.checkout).unwrap();
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Present
        ));
        assert!(matches!(
            checkout_absence(&legacy, true).unwrap(),
            Absence::Present
        ));
        fs::remove_dir_all(fixture.root.join("first")).unwrap();
        // An existing nearest ancestor is observed as its own entry, and
        // replacing it by a DIFFERENT empty directory keeps absence provable
        // while changing the witness. That difference is what the real run's
        // comparison refuses; a fresh absence proof alone would not notice.
        let anchor = fixture.root.join("first");
        fs::create_dir(&anchor).unwrap();
        let Absence::Absent(before) = checkout_absence(&row, false).unwrap() else {
            panic!("the observed anchor did not prove absence");
        };
        assert_eq!(
            before.observed.last().map(|entry| entry.name.clone()),
            Some(std::ffi::OsString::from("first"))
        );
        assert_eq!(before.missing, Some(before.observed.len()));
        fs::rename(&anchor, fixture.root.join("first-away")).unwrap();
        fs::create_dir(&anchor).unwrap();
        let Absence::Absent(after) = checkout_absence(&row, false).unwrap() else {
            panic!("the replacement anchor did not prove absence");
        };
        assert_eq!(after.missing, before.missing);
        assert_eq!(after.observed.len(), before.observed.len());
        assert_ne!(after, before);
    }

    /// A RECORDED checkout volume is never waived, whatever the row's age: the
    /// operator list is a distinct permission from volume identity, so a
    /// conflicting recorded volume keeps the root even when the surviving
    /// ancestor sits on the boot Data volume, and the genuine legacy exception
    /// remains an additional requirement rather than a replacement.
    #[test]
    fn a_recorded_checkout_volume_is_never_waived_by_a_listed_legacy_row() {
        let fixture = Fixture::at_internal_root();
        let mut row = row_for(&fixture.root);
        row.checkout = fixture.root.join("one").join("two").join("checkout");
        row.nonce = None;
        row.checkout_volume_uuid = checkout_volume_uuid(&fixture.root);
        // The matching recorded volume and the genuine legacy exception agree.
        let Absence::Absent(witness) = checkout_absence(&row, true).unwrap() else {
            panic!("the matching recorded volume and legacy rule did not prove absence");
        };
        assert!(
            witness.boot_data.is_some(),
            "the legacy permission is still required"
        );
        // A conflicting recorded volume is kept, list or no list.
        let mut conflicting = row.clone();
        conflicting.checkout_volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        for legacy in [true, false] {
            let refused = checkout_absence(&conflicting, legacy).unwrap();
            assert!(
                matches!(
                    refused,
                    Absence::Unknown(ref reason)
                        if reason.contains("not on the checkout's recorded volume")
                ),
                "a conflicting recorded volume was waived (legacy {legacy}): {refused:?}"
            );
        }
        // A row with NO recorded volume keeps the genuine exception's scope:
        // the boot Data rule alone, and only when the exception is granted.
        let mut unrecorded = row.clone();
        unrecorded.checkout_volume_uuid = None;
        assert!(matches!(
            checkout_absence(&unrecorded, false).unwrap(),
            Absence::Unknown(_)
        ));
        assert!(matches!(
            checkout_absence(&unrecorded, true).unwrap(),
            Absence::Absent(_)
        ));
    }

    /// The walk keeps every descriptor of the chain it opened, so a replaced
    /// ancestor is detected without re-resolving any path: the named entry and
    /// the opened descriptor must still be the same object at EVERY level.
    #[test]
    fn a_replaced_ancestor_is_detected_against_the_held_chain() {
        use std::os::unix::fs::MetadataExt;
        let fixture = Fixture::new();
        let top = fixture.root.join("top");
        let inner = top.join("inner");
        fs::create_dir_all(&inner).unwrap();
        let held = |path: &Path| {
            let stat = fs::symlink_metadata(path).unwrap();
            let descriptor = OwnedFd::from(File::open(path).unwrap());
            (descriptor, stat)
        };
        let (root_fd, _) = held(&fixture.root);
        let (top_fd, top_stat) = held(&top);
        let (inner_fd, inner_stat) = held(&inner);
        let entry = |name: &str, stat: &fs::Metadata| ObservedEntry {
            name: std::ffi::OsString::from(name),
            dev: stat.dev(),
            ino: stat.ino(),
        };
        let observed = vec![entry("top", &top_stat), entry("inner", &inner_stat)];
        let descriptors = vec![root_fd, top_fd, inner_fd];
        revalidate_chain(&descriptors, &observed).unwrap();
        // A chain with one descriptor per component is required.
        assert!(revalidate_chain(&descriptors, &observed[..1]).is_err());
        assert!(revalidate_chain(&descriptors[..2], &observed).is_err());
        // Replace the FIRST ancestor by a different empty directory of the same
        // name: the held chain still describes the object it opened.
        fs::rename(&top, fixture.root.join("top-away")).unwrap();
        fs::create_dir(&top).unwrap();
        let refused = revalidate_chain(&descriptors, &observed).unwrap_err();
        assert!(refused.contains("changed while it was read"), "{refused}");
        // And the same replacement is invisible to a witness comparison only
        // if nothing compares identities, which is why the walk keeps them.
        let replacement = fs::symlink_metadata(&top).unwrap();
        assert_ne!(
            (replacement.dev(), replacement.ino()),
            (top_stat.dev(), top_stat.ino())
        );
    }

    /// Non-macOS builds have no held volume facts, so neither a missing chain
    /// nor a plain absent leaf may be proved: every root is kept. Compile and
    /// behaviour are stated honestly and never claimed as a macOS runtime
    /// result. Compiled out on macOS.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn a_missing_chain_is_never_proved_without_apfs_facts() {
        let fixture = Fixture::new();
        let mut row = row_for(&fixture.root);
        row.checkout = fixture.root.join("gone/checkout");
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Unknown(_)
        ));
        assert!(matches!(
            checkout_absence(&row, true).unwrap(),
            Absence::Unknown(_)
        ));
    }

    /// A row evaluated from the snapshot is a clone: it must not count as
    /// "another live reference" to its own root, while a second checkout's
    /// live row naming the same root still keeps it.
    #[test]
    fn a_cloned_row_is_not_another_reference_to_its_own_root() {
        let fixture = Fixture::new();
        let row = row_for(&fixture.root);
        let control = ready_control(&fixture, vec![row.clone()], Vec::new());
        let registry = ready_registry(&control).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let mut budget = SNAPSHOT_MAX_BYTES;
        let mut authority = AuthorityScope::begin().unwrap();
        let reason = eligibility(
            &store,
            &control,
            &registry,
            &row,
            None,
            None,
            &mut authority,
            &mut budget,
            false,
            &mut None,
        )
        .unwrap()
        .unwrap();
        assert!(
            !reason.contains("another checkout's live reference"),
            "a same-owner row kept its own root: {reason}"
        );
        let mut other = row.clone();
        other.checkout_id = "e".repeat(64);
        let control2 = fixture.root.join("second");
        fs::create_dir(&control2).unwrap();
        let mut both = registry.clone();
        both.rows.push(other);
        let reason = eligibility(
            &store,
            &control,
            &both,
            &row,
            None,
            None,
            &mut authority,
            &mut budget,
            false,
            &mut None,
        )
        .unwrap()
        .unwrap();
        assert!(
            reason.contains("another checkout's live reference"),
            "another checkout's reference did not keep the root: {reason}"
        );
    }

    /// Custody that cannot be witnessed — here a control whose admission
    /// lock is missing — is unknown, not closure: the route refuses with a
    /// reason the snapshot classifies as incomplete authority, and it creates
    /// nothing while refusing (oxide#235).
    #[test]
    fn unprovable_custody_refuses_instead_of_reporting_clear() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let checkout = fixture.root.join("gone/checkout");
        let mut authority = AuthorityScope::begin().unwrap();
        let reason = custody_reason(&mut authority, &control, &checkout, false)
            .unwrap()
            .expect("unprovable custody must keep, never report clear");
        assert!(reason.contains("could not be witnessed"), "{reason}");
        // The snapshot classifies this keep as incomplete authority: it is
        // none of the per-root keep shapes evaluate exempts.
        assert_ne!(reason, "the checkout is present, so its root is kept");
        assert_ne!(reason, "an active writer still holds this checkout");
        assert!(!reason.starts_with("a recorded writer still holds this checkout ("));
        assert!(!reason.starts_with("checkout absence is unknown ("));
        assert!(!control.join("admission.lock").exists());
        assert!(!control.join("state.json").exists());
    }

    /// One witnessed custody decision maps onto the keep contract exactly:
    /// an active writer keeps its root with the long-standing text, a
    /// witnessed clean barrier keeps with the barrier holder text, and a
    /// clear witnessed snapshot keeps nothing. The barrier is excluded only
    /// under the caller's own barrier, where the records-only split applies.
    #[test]
    fn witnessed_custody_maps_onto_the_keep_contract() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        store::lock_admission(&control).unwrap();
        let checkout = fixture.root.join("gone/checkout");
        let write_state = |state: &crate::store::State| {
            let bytes = serde_json::to_vec(state).unwrap();
            let temp = control.join("state.json.custody-tmp");
            fs::write(&temp, bytes).unwrap();
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).unwrap();
            fs::rename(&temp, control.join("state.json")).unwrap();
        };
        let mut clean = crate::store::State::default();
        clean.cleans.push(crate::store::Clean {
            id: 1,
            checkout: checkout.clone(),
            process_group: None,
        });
        write_state(&clean);
        let mut authority = AuthorityScope::begin().unwrap();
        let reason = custody_reason(&mut authority, &control, &checkout, false).unwrap();
        assert_eq!(
            reason.as_deref(),
            Some("a recorded writer still holds this checkout (oxide clean barrier)"),
            "{reason:?}"
        );
        // Under the caller's own barrier the recorded barrier is excluded.
        let refusal = custody_clear_under_barrier(&mut authority, &control, &checkout);
        assert!(refusal.is_ok(), "{refusal:?}");
        let mut writing = crate::store::State::default();
        writing.records.push(crate::store::Record {
            id: 2,
            identity: "identity".to_owned(),
            compatibility: "compatibility".to_owned(),
            profile: "quick".to_owned(),
            checkout: Some(checkout.clone()),
            phase: crate::store::Phase::Running,
            process_group: None,
            last_used: 1,
            holder: None,
            lease_id: None,
            pending_recovery: None,
            retired: None,
            lost: None,
            last_used_ms: Some(1),
            class: crate::store::WorkspaceClass::Normal,
            class_reason: None,
        });
        write_state(&writing);
        let reason = custody_reason(&mut authority, &control, &checkout, false).unwrap();
        assert_eq!(
            reason.as_deref(),
            Some("an active writer still holds this checkout"),
            "{reason:?}"
        );
        let refusal = custody_clear_under_barrier(&mut authority, &control, &checkout).unwrap_err();
        assert!(
            refusal.contains("an active writer still holds the checkout"),
            "{refusal}"
        );
        write_state(&crate::store::State::default());
        let reason = custody_reason(&mut authority, &control, &checkout, false).unwrap();
        assert_eq!(reason, None, "{reason:?}");
        let refusal = custody_clear_under_barrier(&mut authority, &control, &checkout);
        assert!(refusal.is_ok(), "{refusal:?}");
    }

    /// A registered `/Volumes` checkout's mount is `/Volumes/<name>`
    /// (component 2), never `/Volumes/Volumes`, and its boot Data alias names
    /// the same mount.
    #[test]
    fn a_volumes_checkout_names_its_own_mount() {
        assert_eq!(
            checkout_path(Path::new("/Volumes/Builds/work/app"))
                .unwrap()
                .0,
            WalkStart::Mount(PathBuf::from("/Volumes/Builds"))
        );
        assert_eq!(
            checkout_path(Path::new("/System/Volumes/Data/Volumes/Builds/work/app"))
                .unwrap()
                .0,
            WalkStart::Mount(PathBuf::from("/Volumes/Builds"))
        );
        assert_eq!(
            checkout_path(Path::new("/Users/tom/app")).unwrap().0,
            WalkStart::Root
        );
        assert!(checkout_path(Path::new("/Volumes")).is_err());
    }

    /// A receiptless legacy root's own `pruning` receipt publication yields
    /// the exact revision the journal then expects, so its own write is not
    /// a conflict; an unrelated registry change before the publication
    /// writes nothing, and one between publication and journal still fails
    /// the journal's compare-and-set, after which the receipt is discarded.
    #[test]
    fn a_pruning_receipt_publication_feeds_the_journal_its_own_revision_only() {
        let fixture = Fixture::new();
        let control = ready_control(&fixture, Vec::new(), Vec::new());
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let plan = "f".repeat(64);
        let legacy = |checkout_id: &str, name: &str| {
            let mut row = row_for(&fixture.root);
            row.checkout_id = checkout_id.to_owned();
            row.checkout = fixture.root.join("gone").join(name);
            row.root = fixture.root.join(format!("vol/oxide-outputs/{name}"));
            row.nonce = None;
            let mut candidate = candidate_of(&row);
            candidate.receiptless = true;
            candidate
        };
        // Own publication, then the journal at exactly that revision.
        let first = legacy(&"a".repeat(64), "first");
        let id = fresh_nonce().unwrap();
        let quarantine = fixture.root.join(format!("{QUARANTINE_PREFIX}{id}"));
        let own = outputs::create_pruning_receipt(
            &control,
            &first.checkout,
            &first.checkout_id,
            &id,
            &|_| false,
            5,
        )
        .unwrap()
        .expect("the receipt was refused at the recheck's own revision");
        assert!(own > 5);
        assert!(matches!(
            journal_intent(&store, &control, &first, &plan, &id, &quarantine, own),
            Ok(true)
        ));
        assert_eq!(
            ready_registry(&control).unwrap().intent(&id).unwrap().phase,
            IntentPhase::Intent
        );

        // An unrelated change before the publication: nothing is written.
        let second = legacy(&"b".repeat(64), "second");
        let stale = ready_registry(&control).unwrap().revision;
        let mut unrelated = ready_registry(&control).unwrap();
        unrelated.bump().unwrap();
        write_registry(&control, &unrelated).unwrap();
        let id2 = fresh_nonce().unwrap();
        assert!(outputs::create_pruning_receipt(
            &control,
            &second.checkout,
            &second.checkout_id,
            &id2,
            &|_| false,
            stale
        )
        .unwrap()
        .is_none());
        assert!(!outputs::receipt_path_public(&control, &second.checkout_id).exists());

        // An unrelated change after the publication: the journal still fails
        // its compare-and-set, journals nothing and the receipt is discarded.
        let current = ready_registry(&control).unwrap().revision;
        let own2 = outputs::create_pruning_receipt(
            &control,
            &second.checkout,
            &second.checkout_id,
            &id2,
            &|_| false,
            current,
        )
        .unwrap()
        .unwrap();
        let mut unrelated = ready_registry(&control).unwrap();
        unrelated.bump().unwrap();
        write_registry(&control, &unrelated).unwrap();
        let quarantine2 = fixture.root.join(format!("{QUARANTINE_PREFIX}{id2}"));
        assert!(matches!(
            journal_intent(&store, &control, &second, &plan, &id2, &quarantine2, own2),
            Ok(false)
        ));
        assert!(ready_registry(&control).unwrap().intent(&id2).is_none());
        assert!(outputs::discard_pruning_receipt(
            &control,
            &second.checkout,
            &second.checkout_id,
            &id2
        )
        .unwrap());
        assert!(!outputs::receipt_path_public(&control, &second.checkout_id).exists());
    }

    /// P2: registry-only inactive history stays in the COMPLETE group and is
    /// never required to agree with the root's current identity. Only the
    /// currently selected rows are the checkout's current authority.
    #[test]
    fn a_group_keeps_inactive_history_beside_current_members() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        let mut release = debug.clone();
        release.member = "release".to_owned();
        // An inactive legacy `target` row preserved as history: no nonce, no
        // recorded device/ino/marker digest, and not selected.
        let mut history = debug.clone();
        history.member = outputs::MEMBER.to_owned();
        history.nonce = None;
        history.root_dev = None;
        history.root_ino = None;
        history.marker_digest = None;
        history.selected = false;
        history.generation = 1;
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 7,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![debug.clone(), release.clone(), history.clone()],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let candidate = candidate_of(&debug);
        let group = verified_group(&registry, &candidate).unwrap();
        assert_eq!(
            group
                .iter()
                .map(|row| row.member.as_str())
                .collect::<Vec<_>>(),
            vec!["debug", "release", "target"]
        );
        // Each row keeps its OWN recorded facts and prior role.
        let target = group.iter().find(|row| row.member == "target").unwrap();
        assert!(!target.selected, "history must stay unselected");
        assert_eq!(target.nonce, None, "history keeps its absent nonce");
        assert_eq!(target.marker_digest, None);
        assert_eq!(target.root_dev, None);
        assert!(group.iter().filter(|row| row.selected).count() == 2);
    }

    /// P2: a complete group with ZERO selected rows is valid history for the
    /// existing released/Detached-root eligibility. It is not a missing group.
    #[test]
    fn a_zero_selected_group_is_valid_history() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        debug.selected = false;
        let mut release = debug.clone();
        release.member = "release".to_owned();
        let registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 8,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![debug.clone(), release.clone()],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let group = verified_group(&registry, &candidate_of(&debug)).unwrap();
        assert_eq!(group.len(), 2);
        assert!(group.iter().all(|row| !row.selected));
        // A group that is empty is still a refusal: zero SELECTED is not zero
        // recorded rows.
        let empty = Registry {
            rows: Vec::new(),
            ..registry
        };
        assert!(verified_group(&empty, &candidate_of(&debug)).is_err());
    }

    /// P2: a group whose recorded HISTORY no longer matches the registry (a
    /// missing or extra row) keeps the root: the rollback refuses instead of
    /// restoring part of it.
    #[test]
    fn a_rollback_refuses_a_group_whose_history_changed() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        let mut release = debug.clone();
        release.member = "release".to_owned();
        release.selected = false;
        let id = "c".repeat(32);
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: id.clone(),
            identity: RootIdentity {
                checkout_id: debug.checkout_id.clone(),
                checkout: debug.checkout.clone(),
                receipt: debug.receipt.clone(),
                member: debug.member.clone(),
                root: debug.root.clone(),
                root_kind: debug.root_kind,
                volume_uuid: debug.volume_uuid.clone(),
                nonce: debug.nonce.clone(),
                generation: debug.generation,
                dev: debug.root_dev.unwrap_or(1),
                ino: debug.root_ino.unwrap_or(2),
                marker_digest: debug.marker_digest.clone().unwrap_or_default(),
            },
            phase: IntentPhase::Intent,
            quarantine: fixture.root.join("quarantine"),
            preview_sha256: "f".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: true,
            prior_receipt_sha: String::new(),
            group: vec![
                GroupRow {
                    member: "debug".to_owned(),
                    generation: debug.generation,
                    nonce: debug.nonce.clone(),
                    root_dev: debug.root_dev,
                    root_ino: debug.root_ino,
                    marker_digest: debug.marker_digest.clone(),
                    selected: true,
                },
                GroupRow {
                    member: "release".to_owned(),
                    generation: release.generation,
                    nonce: release.nonce.clone(),
                    root_dev: release.root_dev,
                    root_ino: release.root_ino,
                    marker_digest: release.marker_digest.clone(),
                    selected: false,
                },
            ],
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        // The registry keeps the group marked by this intent, but the recorded
        // HISTORY row `release` has disappeared.
        let control = ready_control(
            &fixture,
            vec![debug.clone(), release.clone()],
            vec![intent.clone()],
        );
        let mut registry = ready_registry(&control).unwrap();
        for row in registry.rows.iter_mut() {
            row.selected = false;
            row.pruning = Some(id.clone());
        }
        registry.rows.retain(|row| row.member != "release");
        registry.bump().unwrap();
        write_registry(&control, &registry).unwrap();
        let store = crate::store::Store::for_tests(&fixture.root.join("pool"));
        let refused = rollback_intent(&store, &control, &intent).unwrap_err();
        assert!(
            refused.contains("changed since the prune intent"),
            "{refused}"
        );
        let kept = ready_registry(&control).unwrap();
        let row = kept
            .rows
            .iter()
            .find(|row| row.member == "debug")
            .expect("the remaining row disappeared");
        assert_eq!(
            row.pruning.as_deref(),
            Some(id.as_str()),
            "the rollback restored a row for an incomplete group"
        );
    }

    /// P2: pruning a RELEASED root never rewrites the checkout's receipt. When
    /// the receipt currently selects a different root, that selection and the
    /// exact receipt bytes survive while the released root's whole group is
    /// settled and retired.
    #[test]
    fn a_released_root_settles_without_rewriting_the_current_selection() {
        let fixture = Fixture::new();
        let released = row_for(&fixture.root);
        let mut row = released.clone();
        row.member = "debug".to_owned();
        row.selected = false;
        row.pruned_ms = Some(5);
        let current_root = fixture.root.join("current-root");
        let id = "a".repeat(32);
        let intent = Intent {
            version: INTENT_VERSION.to_owned(),
            id: id.clone(),
            identity: RootIdentity {
                checkout_id: row.checkout_id.clone(),
                checkout: row.checkout.clone(),
                receipt: row.receipt.clone(),
                member: row.member.clone(),
                root: row.root.clone(),
                root_kind: row.root_kind,
                volume_uuid: row.volume_uuid.clone(),
                nonce: row.nonce.clone(),
                generation: row.generation,
                dev: row.root_dev.unwrap_or(1),
                ino: row.root_ino.unwrap_or(2),
                marker_digest: row.marker_digest.clone().unwrap_or_default(),
            },
            phase: IntentPhase::Deleted,
            quarantine: fixture.root.join("quarantine"),
            preview_sha256: "f".repeat(64),
            created_ms: 1,
            updated_ms: 1,
            resolved_ms: None,
            receipt_settled: false,
            was_selected: false,
            prior_receipt_sha: String::new(),
            group: vec![GroupRow {
                member: row.member.clone(),
                generation: row.generation,
                nonce: row.nonce.clone(),
                root_dev: row.root_dev,
                root_ino: row.root_ino,
                marker_digest: row.marker_digest.clone(),
                selected: false,
            }],
            expected_terminal_sha: None,
            terminal_ms: None,
        };
        let control = ready_control(&fixture, vec![row.clone()], vec![intent.clone()]);
        // The receipt currently selects a DIFFERENT root.
        let receipt_path = outputs::receipt_path_public(&control, &row.checkout_id);
        fs::create_dir_all(receipt_path.parent().unwrap()).unwrap();
        let receipt = serde_json::json!({
            "version": "oxide.outputs.transition/3",
            "checkout": row.checkout.to_string_lossy(),
            "checkout_id": row.checkout_id,
            "generation": 4,
            "state": "committed",
            "layout": "profile",
            "links": [{
                "member": "debug",
                "selected": {
                    "kind": "external",
                    "root": current_root.to_string_lossy(),
                    "volume_uuid": row.volume_uuid,
                    "nonce": "e".repeat(32),
                    "generation": 3
                }
            }],
            "updated_ms": 1
        });
        let bytes = serde_json::to_vec_pretty(&receipt).unwrap();
        fs::write(&receipt_path, &bytes).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        settle_receipt(&control, &id, "deleted").unwrap();
        // The unrelated current selection is byte-identical.
        assert_eq!(fs::read(&receipt_path).unwrap(), bytes);
        let settled = ready_registry(&control).unwrap();
        assert!(settled.intent(&id).is_none(), "the intent was not settled");
        assert!(
            settled
                .rows
                .iter()
                .all(|existing| existing.root != row.root),
            "the released root's rows were not retired"
        );
    }

    /// P2: a pending, reserve-carrying or already-pruning reference keeps the
    /// root even when its member would otherwise be current authority.
    #[test]
    fn a_group_refuses_pending_or_reserved_references() {
        let fixture = Fixture::new();
        let mut debug = row_for(&fixture.root);
        debug.member = "debug".to_owned();
        let base = Registry {
            version: REGISTRY_VERSION.to_owned(),
            schema: REGISTRY_SCHEMA,
            revision: 9,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: vec![debug.clone()],
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let candidate = candidate_of(&debug);
        verified_group(&base, &candidate).unwrap();
        for broken in [
            Row {
                pending: true,
                ..debug.clone()
            },
            Row {
                reservation: Some("reserve".to_owned()),
                ..debug.clone()
            },
            Row {
                cancelling: Some("cancel".to_owned()),
                ..debug.clone()
            },
            Row {
                pruning: Some("intent".to_owned()),
                ..debug.clone()
            },
        ] {
            let registry = Registry {
                rows: vec![broken.clone()],
                ..base.clone()
            };
            let refused = verified_group(&registry, &candidate).unwrap_err();
            assert!(refused.contains("not an eligible part"), "{refused}");
        }
        // An unusable recorded identity is refused by SHAPE, even in history.
        let mut broken_history = debug.clone();
        broken_history.selected = false;
        broken_history.nonce = Some("zz".to_owned());
        let registry = Registry {
            rows: vec![broken_history],
            ..base.clone()
        };
        let refused = verified_group(&registry, &candidate).unwrap_err();
        assert!(refused.contains("unusable recorded identity"), "{refused}");
    }

    /// P2: the receipt's current references are compared with the SELECTED part
    /// of the group only. Inactive history is never required in the receipt,
    /// and a receipt that selects a historical member is a contradiction.
    #[test]
    fn the_receipt_is_compared_with_the_selected_part_of_the_group() {
        let root = PathBuf::from("/volumes/owned/root");
        let group = vec![
            GroupRow {
                member: "debug".to_owned(),
                generation: 3,
                nonce: Some("b".repeat(32)),
                root_dev: Some(1),
                root_ino: Some(2),
                marker_digest: Some("c".repeat(64)),
                selected: true,
            },
            GroupRow {
                member: "release".to_owned(),
                generation: 3,
                nonce: Some("b".repeat(32)),
                root_dev: Some(1),
                root_ino: Some(2),
                marker_digest: Some("c".repeat(64)),
                selected: true,
            },
            GroupRow {
                member: "target".to_owned(),
                generation: 1,
                nonce: None,
                root_dev: None,
                root_ino: None,
                marker_digest: None,
                selected: false,
            },
        ];
        let current = vec![
            ("debug".to_owned(), root.clone(), 3u64, Some("b".repeat(32))),
            (
                "release".to_owned(),
                root.clone(),
                3u64,
                Some("b".repeat(32)),
            ),
        ];
        // Exactly the two selected members: history is absent and that is fine.
        selected_group_mismatch(&current, &root, &group).unwrap();
        // A historical member appearing as current is a contradiction.
        let mut with_history = current.clone();
        with_history.push(("target".to_owned(), root.clone(), 1, None));
        let refused = selected_group_mismatch(&with_history, &root, &group).unwrap_err();
        assert!(
            refused.contains("do not match the selected part"),
            "{refused}"
        );
        // A missing selected member is a contradiction.
        let missing = vec![current[0].clone()];
        assert!(selected_group_mismatch(&missing, &root, &group).is_err());
        // A changed generation or nonce is a contradiction.
        let mut changed = current.clone();
        changed[1].2 = 4;
        assert!(selected_group_mismatch(&changed, &root, &group).is_err());
        let mut renamed = current.clone();
        renamed[0].3 = Some("d".repeat(32));
        assert!(selected_group_mismatch(&renamed, &root, &group).is_err());
        // Another root's placement never counts as this root's authority.
        let elsewhere = vec![(
            "debug".to_owned(),
            PathBuf::from("/volumes/other/root"),
            3u64,
            Some("b".repeat(32)),
        )];
        assert!(selected_group_mismatch(&elsewhere, &root, &group).is_err());
        // A ZERO-selected group requires the receipt to select nothing here.
        let detached: Vec<GroupRow> = group
            .iter()
            .cloned()
            .map(|row| GroupRow {
                selected: false,
                ..row
            })
            .collect();
        selected_group_mismatch(&[], &root, &detached).unwrap();
        assert!(selected_group_mismatch(&current, &root, &detached).is_err());
    }

    #[test]
    fn status_reads_only_a_bounded_recorded_observation() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            status_lines(&control),
            vec!["prune: no recorded prune observation".to_owned()]
        );
        let directory = registry_dir(&control);
        ensure_private_dir(&directory).unwrap();
        write_private(&observations_path(&control), b"{not json").unwrap();
        assert_eq!(
            status_lines(&control),
            vec!["prune: recorded observation is invalid".to_owned()]
        );
    }
}
