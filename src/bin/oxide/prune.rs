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

pub const REGISTRY_VERSION: &str = "oxide.outputs.registry/1";
pub const OBSERVATIONS_VERSION: &str = "oxide.outputs.prune-observations/1";
pub const PREVIEW_VERSION: &str = "oxide.outputs.prune-preview/1";
pub const INTENT_VERSION: &str = "oxide.outputs.prune-intent/1";
pub const RESOLUTION_VERSION: &str = "oxide.outputs.prune-resolution/1";

const REGISTRY_FILE: &str = "registry.json";
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
const MAX_OPERATOR_LIST_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const DELETE_BATCH: u64 = 512;
const OBSERVATION_ENTRIES: u64 = 50_000;
const OBSERVATION_MS: u64 = 5_000;
/// Debug-only fault injection for the native proof: name a prune phase to stop
/// before its effect.
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
        let member = outputs::MEMBER;
        let checkout_volume_uuid = volume_uuid_of(&stub.checkout).ok().flatten();
        for (selected, is_selected) in receipt
            .current()
            .map(|selected| (selected, true))
            .into_iter()
            .chain(receipt.prepared_ref().map(|prepared| (&prepared.to, false)))
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
            let identity = row_identity(selected)?;
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
                generation: selected.generation(),
                root_dev: identity.0,
                root_ino: identity.1,
                marker_digest: identity.2,
                selected: is_selected,
                pending: !is_selected,
                pruning: None,
                pruned_ms: None,
                reservation: None,
                cancelling: None,
            });
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
    file.take(MAX_REGISTRY_BYTES + 1)
        .read_to_end(&mut bytes)
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
            return Err(
                "the output registry is not initialized; run `oxide outputs adopt` once to establish the shared forward format, then prune"
                    .to_owned(),
            )
        }
    }
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
        if self.version != REGISTRY_VERSION {
            return Err(format!(
                "the output registry has an unsupported version {:?}",
                self.version
            ));
        }
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

fn hex64(value: &str) -> bool {
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
pub fn note_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    require_registry(control)?;
    let mut registry = ready_registry(control)?;
    registry.validate()?;
    let checkout_id = receipt.checkout_id();
    let checkout = receipt.checkout();
    let receipt_path = outputs::receipt_path_public(control, checkout_id);
    let member = "target";
    let selected = receipt.current();
    let prepared = receipt.prepared_ref();
    // A prepared transition still selects its committed root until the commit
    // is durable; a missing reference is never treated as non-selection.
    // Detach and pruned states retain identity but select nothing.
    let keep_selected = selected.is_some() && receipt.holds_selection();
    let proposed = prepared.map(|prepared| &prepared.to);
    registry.bump()?;
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
                    // prune proof matches (a pending row had none yet).
                    let identity = row_identity(selected)?;
                    row.root_dev = identity.0;
                    row.root_ino = identity.1;
                    row.marker_digest = identity.2;
                    row.nonce = selected.nonce().map(str::to_owned);
                    row.generation = selected.generation();
                    row.volume_uuid = selected.volume_uuid().map(str::to_owned);
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
        // The receipt is authoritative for this checkout's placement: a
        // receipt with no prepared transition cannot still have a pending
        // reference. This is what clears a rolled-back preparation.
        for row in registry
            .rows
            .iter_mut()
            .filter(|row| row.checkout_id == checkout_id && row.member == member)
        {
            row.pending = false;
        }
    }
    let checkout_volume_uuid = volume_uuid_of(checkout).ok().flatten();
    if let Some(selected) = selected {
        if (keep_selected && !matched_selected) || (proposed.is_some() && !matched_pending) {
            let identity = row_identity(selected)?;
            if registry.rows.len() >= MAX_ROWS {
                mark_incomplete(&mut registry, "the registry row limit was reached")?;
            } else {
                let row = Row {
                    checkout_id: checkout_id.to_owned(),
                    checkout: checkout.to_path_buf(),
                    checkout_volume_uuid: checkout_volume_uuid.clone(),
                    receipt: receipt_path.clone(),
                    member: member.to_owned(),
                    root: selected.root().to_path_buf(),
                    root_kind: selected.kind(),
                    volume_uuid: selected.volume_uuid().map(str::to_owned),
                    recipe: outputs::RECIPE_PUBLIC.to_owned(),
                    nonce: selected.nonce().map(str::to_owned),
                    generation: selected.generation(),
                    root_dev: identity.0,
                    root_ino: identity.1,
                    marker_digest: identity.2,
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
                mark_incomplete(&mut registry, "the registry row limit was reached")?;
            } else {
                let identity = row_identity(proposed)?;
                registry.rows.push(Row {
                    checkout_id: checkout_id.to_owned(),
                    checkout: checkout.to_path_buf(),
                    checkout_volume_uuid: checkout_volume_uuid.clone(),
                    receipt: receipt_path.clone(),
                    member: member.to_owned(),
                    root: proposed.root().to_path_buf(),
                    root_kind: proposed.kind(),
                    volume_uuid: proposed.volume_uuid().map(str::to_owned),
                    recipe: outputs::RECIPE_PUBLIC.to_owned(),
                    nonce: proposed.nonce().map(str::to_owned),
                    generation: proposed.generation(),
                    root_dev: identity.0,
                    root_ino: identity.1,
                    marker_digest: identity.2,
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
    note_reservation(&mut registry, receipt, &receipt_path, checkout_volume_uuid)?;
    write_registry(control, &registry)
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
    let member = outputs::MEMBER;
    let reserved = receipt.reserved();
    let current_id = reserved.map(|reserved| reserved.id.as_str());
    let selected_root = receipt
        .current()
        .map(|selected| selected.root().to_path_buf());
    let proposed_root = receipt
        .prepared_ref()
        .map(|prepared| prepared.to.root().to_path_buf());
    for row in registry
        .rows
        .iter_mut()
        .filter(|row| row.checkout_id == checkout_id && row.member == member)
    {
        let Some(id) = row.reservation.clone() else {
            continue;
        };
        if Some(id.as_str()) == current_id || row.cancelling.is_some() {
            continue;
        }
        // Consumed: the commit (or the prepared transition) now references
        // the reserved root itself.
        if Some(&row.root) == selected_root.as_ref() || Some(&row.root) == proposed_root.as_ref() {
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
    if let Some(row) = registry.rows.iter_mut().find(|row| {
        row.checkout_id == checkout_id && row.member == member && row.root == reserved.root
    }) {
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

fn row_identity(selected: &Selected) -> Result<(Option<u64>, Option<u64>, Option<String>), String> {
    let Ok(metadata) = fs::symlink_metadata(selected.root()) else {
        return Ok((None, None, None));
    };
    use std::os::unix::fs::MetadataExt;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Ok((None, None, None));
    }
    let digest = outputs::marker_digest(selected.root()).ok();
    Ok((Some(metadata.dev()), Some(metadata.ino()), digest))
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
        lines.push(format!(
            "  {} {} {} ({}, {} logical / {} allocated bytes{})",
            root.outcome,
            root.checkout_id,
            root.root.display(),
            root.intent.as_deref().unwrap_or("no intent"),
            root.logical_bytes,
            root.allocated_bytes,
            if root.accounting_complete {
                ""
            } else {
                ", partial accounting"
            }
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

/// One evaluated root: either a candidate or an explicit keep reason.
struct Evaluated {
    row: Row,
    candidate: Option<Candidate>,
    kept: Option<String>,
    legacy: bool,
}

/// `oxide outputs prune --dry-run`
fn dry_run(operator_list: Option<&Path>) -> Result<u8, String> {
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
    let evaluated = evaluate(&store, &control, &registry, list.as_ref())?;
    let preview = build_preview(&registry, &evaluated, list.as_ref());
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
    eprintln!(
        "oxide: prune preview SHA-256 {sha} ({} candidate root(s), {} kept); nothing was changed. Write these exact bytes to a file and re-run with --plan FILE --plan-sha256 {sha}",
        preview.totals.candidates, preview.totals.kept
    );
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
    if !completed_roots.is_empty() || rolled_back > 0 {
        registry = match ready_registry(&control) {
            Ok(registry) => registry,
            Err(reason) => {
                return Ok(unsettled(
                    &control,
                    &format!(
                        "a replayed intent settled but the registry became unreadable afterwards ({reason})"
                    ),
                ))
            }
        };
    }
    // Fresh revalidation: the plan's root identity set must match one-to-one,
    // with roots completed by an exact intent replay excluded.
    let evaluated = match evaluate(&store, &control, &registry, list.as_ref()) {
        Ok(evaluated) => evaluated,
        Err(reason) if !completed_roots.is_empty() => {
            return Ok(partial(
                &completed_roots,
                &format!("the fresh snapshot failed ({reason}), so every other root is kept"),
            ))
        }
        Err(reason) => return Err(reason),
    };
    let fresh = build_preview(&registry, &evaluated, list.as_ref());
    let plan_set: Vec<&Candidate> = preview
        .candidates
        .iter()
        .filter(|candidate| !completed_roots.contains(&candidate.root))
        .collect();
    let fresh_set: Vec<&Candidate> = fresh.candidates.iter().collect();
    if !same_candidates(&plan_set, &fresh_set) {
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
    }
    if fresh.candidates.is_empty() {
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
    for candidate in &fresh.candidates {
        match prune_one(&store, &control, candidate, sha, list.as_ref()) {
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

fn same_candidates(plan: &[&Candidate], fresh: &[&Candidate]) -> bool {
    if plan.len() != fresh.len() {
        return false;
    }
    plan.iter().all(|planned| {
        fresh.iter().any(|current| {
            planned.checkout_id == current.checkout_id
                && planned.member == current.member
                && planned.root == current.root
                && planned.nonce == current.nonce
                && planned.dev == current.dev
                && planned.ino == current.ino
                && planned.marker_digest == current.marker_digest
        })
    })
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
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("the {what} {} is not a real file", path.display()));
    }
    if metadata.len() > limit as u64 {
        return Err(format!(
            "the {what} {} exceeds its {limit}-byte limit",
            path.display()
        ));
    }
    let file = File::open(path)
        .map_err(|error| format!("cannot open the {what} {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
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
) -> Result<Vec<Evaluated>, String> {
    let deadline = Instant::now() + SNAPSHOT_TIME;
    let mut budget = SNAPSHOT_MAX_BYTES;
    let mut receipts: std::collections::HashMap<String, Result<Receipt, String>> =
        std::collections::HashMap::new();
    let mut evaluated = Vec::new();
    let rows: Vec<Row> = registry
        .rows
        .iter()
        .filter(|row| row.live() || row.pruned_ms.is_none())
        .cloned()
        .collect();
    if rows.len() > SNAPSHOT_MAX_RECEIPTS {
        // A cap is not permission to consider part of the registry.
        return Ok(rows
            .into_iter()
            .map(|row| Evaluated {
                row,
                candidate: None,
                kept: Some(
                    "the registry exceeds the snapshot receipt cap; the whole batch is kept"
                        .to_owned(),
                ),
                legacy: false,
            })
            .collect());
    }
    let mut snapshot_error: Option<String> = None;
    for row in rows {
        let entry = receipts.entry(row.checkout_id.clone()).or_insert_with(|| {
            match outputs::read_receipt_for_prune(control, &row.checkout, &row.checkout_id) {
                Ok(Some(receipt)) => Ok(receipt),
                Ok(None) => Err("the receipt is missing".to_owned()),
                Err(reason) => Err(reason),
            }
        });
        let receipt = match entry.as_ref() {
            Ok(receipt) => Some(receipt),
            Err(reason) => {
                snapshot_error.get_or_insert_with(|| reason.clone());
                None
            }
        };
        if Instant::now() > deadline {
            snapshot_error.get_or_insert_with(|| "the snapshot exceeded its time limit".to_owned());
        }
        let mut out = Evaluated {
            row: row.clone(),
            candidate: None,
            kept: None,
            legacy: row.nonce.is_none(),
        };
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
                &mut budget,
                false,
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
                        out.kept = Some(reason);
                        evaluated.push(out);
                        continue;
                    }
                };
                if facts.legacy {
                    out.legacy = true;
                }
                let (logical, allocated, complete) = account_root(&row.root);
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
                    logical_bytes: logical,
                    allocated_bytes: allocated,
                    accounting_complete: complete,
                    receiptless: false,
                });
            }
            Ok(Some(reason)) | Err(reason) => out.kept = Some(reason),
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
            let mut entry = evaluate_receiptless(store, control, registry, root, false);
            if entry.candidate.is_some() {
                extra += 1;
                if registry.rows.len() + extra > MAX_ROWS {
                    entry.candidate = None;
                    entry.kept = Some(
                        "a temporary pruning row would exceed the registry row limit".to_owned(),
                    );
                }
            }
            evaluated.push(entry);
        }
    }
    if let Some(reason) = snapshot_error {
        for entry in &mut evaluated {
            entry.candidate = None;
            entry.kept = Some(format!(
                "the registry snapshot is incomplete ({reason}); the whole batch is kept"
            ));
        }
        return Ok(evaluated);
    }
    // One root may not be selected by two live references.
    let mut counts: std::collections::HashMap<PathBuf, usize> = std::collections::HashMap::new();
    for entry in &evaluated {
        if let Some(candidate) = &entry.candidate {
            *counts.entry(candidate.root.clone()).or_insert(0) += 1;
        }
    }
    for entry in &mut evaluated {
        let duplicated = entry
            .candidate
            .as_ref()
            .is_some_and(|candidate| counts.get(&candidate.root).copied().unwrap_or(0) > 1);
        if duplicated {
            entry.kept = Some("more than one live registry reference names this root".to_owned());
            entry.candidate = None;
        }
    }
    Ok(evaluated)
}

/// The eligibility predicates for one row. `Ok(None)` means every predicate
/// holds; `Ok(Some(reason))` is an explicit keep; `Err` keeps the root and
/// refuses the batch where the snapshot itself is untrustworthy.
#[allow(clippy::too_many_arguments)]
fn eligibility(
    store: &Store,
    control: &Path,
    registry: &Registry,
    row: &Row,
    receipt: Option<&Receipt>,
    list: Option<&OperatorList>,
    budget: &mut u64,
    under_barrier: bool,
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
    if !row.selected {
        return Ok(Some("the registry does not select this root".to_owned()));
    }
    if registry
        .live_rows()
        .any(|other| other.root == row.root && !std::ptr::eq(other, row))
    {
        return Ok(Some(
            "another live reference also names this root".to_owned(),
        ));
    }
    let Some(receipt) = receipt else {
        return Ok(Some(
            "the checkout's receipt is missing or unreadable".to_owned(),
        ));
    };
    if !receipt.selects() {
        return Ok(Some(
            "the checkout's receipt is detached, pruning or prepared".to_owned(),
        ));
    }
    let Some(selected) = receipt.current() else {
        return Ok(Some("the checkout's receipt selects nothing".to_owned()));
    };
    if selected.root() != row.root
        || selected.nonce() != row.nonce.as_deref()
        || selected.generation() != row.generation
        || selected.kind() != row.root_kind
    {
        return Ok(Some(
            "the registry row does not match the checkout's committed receipt".to_owned(),
        ));
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
    // Checkout absence: no-follow walk, actual leaf ENOENT only.
    match checkout_absence(row, listed) {
        Ok(Absence::Absent) => {}
        Ok(Absence::Present) => {
            return Ok(Some(
                "the checkout is present, so its root is kept".to_owned(),
            ))
        }
        Ok(Absence::Unknown(reason)) | Err(reason) => {
            return Ok(Some(format!("checkout absence is unknown ({reason})")))
        }
    }
    // Writer custody across both pools, and library ledger holders. A
    // caller holding the checkout's barrier excludes only its own barrier.
    if under_barrier {
        if let Err(reason) = custody_clear_under_barrier(store, control, &row.checkout) {
            return Ok(Some(reason));
        }
        if *budget > 0 {
            *budget = budget.saturating_sub(row.root.to_string_lossy().len() as u64);
        }
        return Ok(None);
    }
    match store.checkout_custody(&row.checkout, 0)? {
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
    for holder in store::active_holders(control, &row.checkout)? {
        return Ok(Some(format!(
            "a recorded writer still holds this checkout ({holder})"
        )));
    }
    // An unreadable removable state is not closure: the root is kept.
    let removable = removable::removable_control(control);
    match store::active_holders(&removable, &row.checkout) {
        Ok(holders) => {
            if let Some(holder) = holders.into_iter().next() {
                return Ok(Some(format!(
                    "a recorded writer in the removable pool still holds this checkout ({holder})"
                )));
            }
        }
        Err(reason) => {
            return Ok(Some(format!(
                "the removable pool's writer records are unreadable ({reason})"
            )))
        }
    }
    if *budget > 0 {
        *budget = budget.saturating_sub(row.root.to_string_lossy().len() as u64);
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

enum Absence {
    Absent,
    Present,
    Unknown(String),
}

/// Positive absence of a checkout: every ancestor opens as a real directory
/// without following a link, and the actual leaf returns ENOENT from
/// `fstatat(AT_SYMLINK_NOFOLLOW)`. Any other answer is uncertainty.
/// The path components a checkout walk visits after its start descriptor.
/// For `/Volumes/<mount>/...` the walk starts at the verified mount root, so
/// the volume name itself is not visited again; for any other absolute path
/// it starts at `/`.
fn checkout_walk(checkout: &Path) -> Result<(bool, Vec<&std::ffi::OsStr>), String> {
    let components: Vec<_> = checkout.components().collect();
    // An absolute path is `RootDir` then `Volumes`; the mount root is the
    // next component and is already the walk's start descriptor.
    let under_volumes = matches!(
        components.get(1),
        Some(std::path::Component::Normal(first)) if first.to_str() == Some("Volumes")
    );
    let skip = if under_volumes { 3 } else { 1 };
    let rest: Vec<&std::ffi::OsStr> = components
        .iter()
        .skip(skip)
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(*name),
            _ => None,
        })
        .collect();
    if rest.is_empty() {
        return Err("the checkout path has no components below its start".to_owned());
    }
    Ok((under_volumes, rest))
}

fn checkout_absence(row: &Row, operator_legacy: bool) -> Result<Absence, String> {
    use std::os::unix::ffi::OsStrExt;
    let checkout = &row.checkout;
    let components: Vec<_> = checkout.components().collect();
    let (under_volumes, rest) = match checkout_walk(checkout) {
        Ok(found) => found,
        Err(reason) => return Ok(Absence::Unknown(reason)),
    };
    let mut cursor: OwnedFd = open_root()?;
    if under_volumes {
        // A recorded checkout below /Volumes needs the registered volume's
        // UUID and a live matching mount; a legacy row without one is kept.
        let Some(uuid) = row.checkout_volume_uuid.as_deref() else {
            return Ok(Absence::Unknown(
                "a /Volumes checkout has no recorded volume UUID".to_owned(),
            ));
        };
        let Some(std::path::Component::Normal(volume)) = components.get(1) else {
            return Ok(Absence::Unknown(
                "the /Volumes path is malformed".to_owned(),
            ));
        };
        let mount = Path::new("/Volumes").join(volume);
        let binding = match Binding::open(&mount, Some(uuid)) {
            Ok(binding) => binding,
            Err(error) => {
                return Ok(Absence::Unknown(format!(
                    "the recorded volume is not mounted and matching ({error})"
                )))
            }
        };
        // Continue the walk from the mount root descriptor.
        // SAFETY: `dup` returns a new owned descriptor on success.
        let opened = unsafe { libc::dup(binding.fd().as_raw_fd()) };
        if opened < 0 {
            return Ok(Absence::Unknown(
                "cannot duplicate the mount descriptor".to_owned(),
            ));
        }
        // SAFETY: `opened` is a newly owned descriptor.
        cursor = unsafe { OwnedFd::from_raw_fd(opened) };
    } else if operator_legacy {
        // The explicit legacy exception allows only a positively mounted,
        // readable APFS data-volume ancestry below /Users.
        let facts = match oxide_storage::volume::observe_path(Path::new("/Users")) {
            Ok(facts) => facts,
            Err(error) => return Ok(Absence::Unknown(format!("/Users is unreadable ({error})"))),
        };
        if facts.fstype != "apfs" || facts.read_only || facts.uuid.is_none() {
            return Ok(Absence::Unknown(
                "/Users is not on a writable APFS data volume".to_owned(),
            ));
        }
    }
    for (index, name) in rest.iter().enumerate() {
        let last = index + 1 == rest.len();
        let name = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| "the checkout path contains a NUL".to_owned())?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor and C string are live for the call.
        let status = unsafe {
            libc::fstatat(
                cursor.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            return Ok(if error.kind() != std::io::ErrorKind::NotFound {
                Absence::Unknown(error.to_string())
            } else if last {
                // Only the actual checkout leaf may be absent by ENOENT.
                Absence::Absent
            } else {
                Absence::Unknown(
                    "an ancestor of the checkout is missing, so the checkout path cannot be proved absent"
                        .to_owned(),
                )
            });
        }
        if last {
            return Ok(Absence::Present);
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Ok(Absence::Unknown(format!(
                "an ancestor of the checkout is not a directory"
            )));
        }
        // SAFETY: the parent descriptor and C string are live for the call.
        let next = unsafe {
            libc::openat(
                cursor.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next < 0 {
            return Ok(Absence::Unknown(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // SAFETY: `next` is a newly owned descriptor.
        cursor = unsafe { OwnedFd::from_raw_fd(next) };
    }
    Ok(Absence::Unknown("the checkout walk ended early".to_owned()))
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
        Ok(binding) => binding,
        Err(error) => {
            return Ok(RootState::Unknown(format!(
                "the recorded volume is not usable ({error})"
            )))
        }
    };
    let outer = binding
        .dir_at(binding.fd(), outputs::EXTERNAL_DIR, false)
        .map_err(|error| format!("cannot open the outputs directory: {error}"))?;
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
        return Ok(if error.kind() == std::io::ErrorKind::NotFound {
            RootState::Absent
        } else {
            RootState::Unknown(error.to_string())
        });
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Ok(RootState::Unknown(
            "the recorded root is not a real directory".to_owned(),
        ));
    }
    let dev = u64::try_from(stat.st_dev).unwrap_or(u64::MAX);
    let ino = stat.st_ino;
    if row.root_dev != Some(dev) || row.root_ino != Some(ino) {
        return Ok(RootState::Unknown(
            "the recorded root's device or inode changed".to_owned(),
        ));
    }
    let root = binding
        .dir_at(outer.as_fd(), &name.to_string_lossy(), false)
        .map_err(|error| format!("cannot open the recorded root: {error}"))?;
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
        return Ok(RootState::Unknown(format!(
            "the root has no readable owner marker ({})",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `marker_fd` is a newly owned descriptor.
    let marker_file = unsafe { File::from_raw_fd(marker_fd) };
    let mut bytes = Vec::new();
    marker_file
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
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
    let legacy = marker.is_legacy();
    if !marker.is_owned_by(&row.checkout, &row.checkout_id) {
        return Ok(RootState::Unknown(
            "the owner marker names another checkout".to_owned(),
        ));
    }
    if marker.kind() != row.root_kind
        || marker.generation() != row.generation
        || marker.volume_uuid() != row.volume_uuid.as_deref()
        || marker.nonce() != row.nonce.as_deref()
    {
        return Ok(RootState::Unknown(
            "the owner marker does not match the recorded root identity".to_owned(),
        ));
    }
    let digest = sha256_hex(&bytes);
    if row
        .marker_digest
        .as_deref()
        .is_some_and(|recorded| recorded != digest)
    {
        return Ok(RootState::Unknown(
            "the owner marker changed since it was recorded".to_owned(),
        ));
    }
    Ok(RootState::Present(RootFacts {
        dev,
        ino,
        marker_digest: digest,
        legacy,
    }))
}

fn account_root(root: &Path) -> (u64, u64, bool) {
    let limits = InventoryLimits {
        max_entries: OBSERVATION_ENTRIES,
        max_depth: 64,
        max_output_bytes: 4096,
        max_elapsed_ms: OBSERVATION_MS,
    };
    match inventory::inventory_directory(root, limits) {
        Ok(report) => (
            report.total.logical_bytes_by_name,
            report.total.allocated_bytes,
            report.coverage.complete,
        ),
        Err(_) => (0, 0, false),
    }
}

fn build_preview(
    registry: &Registry,
    evaluated: &[Evaluated],
    list: Option<&OperatorList>,
) -> Preview {
    let candidates: Vec<Candidate> = evaluated
        .iter()
        .filter_map(|entry| entry.candidate.clone())
        .collect();
    let mut kept: Vec<Kept> = evaluated
        .iter()
        .filter_map(|entry| {
            entry.kept.as_ref().map(|reason| Kept {
                checkout_id: Some(entry.row.checkout_id.clone()),
                root: Some(entry.row.root.clone()),
                reason: reason.clone(),
            })
        })
        .collect();
    kept.truncate(MAX_CANDIDATES);
    Preview {
        version: PREVIEW_VERSION.to_owned(),
        generated_ms: process::now_ms(),
        registry_revision: registry.revision,
        registry_complete: registry.complete,
        coverage: Coverage {
            roots_scanned: evaluated.len() as u64,
            registry_roots: registry.rows.len() as u64,
            candidate_roots: candidates.len() as u64,
            kept_roots: kept.len() as u64,
            complete: registry.complete,
            source: "internal output registry".to_owned(),
            observed_ms: process::now_ms(),
        },
        operator_list_sha256: list.map(|list| list.sha256.clone()),
        totals: Totals {
            candidates: candidates.len() as u64,
            kept: kept.len() as u64,
            logical_bytes: candidates.iter().map(|root| root.logical_bytes).sum(),
            allocated_bytes: candidates.iter().map(|root| root.allocated_bytes).sum(),
            accounting_complete: candidates.iter().all(|root| root.accounting_complete),
        },
        candidates,
        kept,
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
        let Some(uuid) = candidate.volume_uuid.as_deref() else {
            return Err("the root has no recorded volume UUID".to_owned());
        };
        let root_path = &candidate.root;
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
        let dev = u64::try_from(facts.st_dev).unwrap_or(u64::MAX);
        if facts.st_ino != candidate.ino || dev != candidate.dev {
            return Err("the root's device or inode changed".to_owned());
        }
        if dev != binding.verify()?.dev {
            return Err("the root is not on the verified volume".to_owned());
        }
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
        let name = std::ffi::CString::new(outputs::MARKER).map_err(|error| error.to_string())?;
        // SAFETY: the root descriptor and C string are live for the call.
        let fd = unsafe {
            libc::openat(
                self.root.as_raw_fd(),
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
        if !marker.is_owned_by(&candidate.checkout, &candidate.checkout_id)
            || marker.nonce() != candidate.nonce.as_deref()
            || marker.generation() != candidate.generation
        {
            return Err("the owner marker does not match the plan".to_owned());
        }
        Ok(())
    }

    /// Recheck the volume and its reserve between bounded work batches.
    fn check(&self) -> Result<(), Stop> {
        let facts = self.binding.verify().map_err(|error| {
            Stop::Pending(format!(
                "the registered volume is no longer verified ({error})"
            ))
        })?;
        if let Some(available) = facts.available_bytes {
            if available <= self.registration.reserve_bytes {
                return Err(Stop::Pending(format!(
                    "the registered volume's free space {available} bytes is at or below the recorded {} byte reserve",
                    self.registration.reserve_bytes
                )));
            }
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

    /// Remove the empty quarantine leaf itself.
    fn remove_quarantine(&self, quarantine: &str) -> Result<(), String> {
        let name =
            std::ffi::CString::new(quarantine.as_bytes()).map_err(|error| error.to_string())?;
        // SAFETY: the descriptor and C string are live for the call.
        let status =
            unsafe { libc::unlinkat(self.outputs.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
        if status != 0 {
            return Err(format!(
                "cannot remove the quarantine leaf ({})",
                std::io::Error::last_os_error()
            ));
        }
        sync_fd(&self.outputs)
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
        let dev = u64::try_from(facts.st_dev).unwrap_or(u64::MAX);
        // The rename kept the root's inode: anything else under the
        // intent's quarantine name is not the recorded root.
        if dev != identity.dev || facts.st_ino != identity.ino {
            return Err(
                "the quarantine is not the recorded root (device or inode differs); it is kept"
                    .to_owned(),
            );
        }
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

/// One bounded delete pass over a verified quarantine directory.
struct Deleter<'a> {
    session: &'a RootSession,
    deadline: Instant,
    entries: u64,
}

fn delete_contents(deleter: &mut Deleter, dir: BorrowedFd, top: bool) -> Result<(), Stop> {
    delete_contents_at(deleter, dir, top, 0)
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
    let mut names = list_directory(dir).map_err(Stop::Refuse)?;
    if names.len() > 200_000 {
        return Err(Stop::Pending(
            "one quarantined directory holds more entries than one bounded batch; it is kept"
                .to_owned(),
        ));
    }
    names.sort();
    for name in names {
        if top && name == outputs::MARKER {
            continue;
        }
        if Instant::now() > deleter.deadline {
            return Err(Stop::Pending(
                "the delete batch exceeded its time window; the quarantine is kept".to_owned(),
            ));
        }
        let name_c = std::ffi::CString::new(name.as_bytes())
            .map_err(|_| Stop::Refuse("a quarantined name contains a NUL".to_owned()))?;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor and C string are live for the call.
        let status = unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name_c.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                continue;
            }
            return Err(Stop::Refuse(format!(
                "cannot inspect a quarantined entry: {error}"
            )));
        }
        let dev = u64::try_from(stat.st_dev).unwrap_or(u64::MAX);
        if dev != deleter.session.dev {
            return Err(Stop::Pending(
                "a quarantined entry is on another device; no further delete".to_owned(),
            ));
        }
        // A symbolic link inside the quarantine is never followed or removed:
        // the delete stops and the intent stays pending (exit 3).
        if stat.st_mode & libc::S_IFMT == libc::S_IFLNK {
            return Err(Stop::Pending(format!(
                "the quarantine holds a symbolic link ({name}); deletion stops and the intent stays pending"
            )));
        }
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            // SAFETY: the parent descriptor and C string are live for the call.
            let child = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    name_c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if child < 0 {
                return Err(Stop::Pending(format!(
                    "cannot open a quarantined directory ({})",
                    std::io::Error::last_os_error()
                )));
            }
            // SAFETY: `child` is a newly owned descriptor.
            let child = unsafe { OwnedFd::from_raw_fd(child) };
            delete_contents_at(deleter, child.as_fd(), false, depth + 1)?;
            // SAFETY: the descriptor and C string are live for the call.
            let removed =
                unsafe { libc::unlinkat(dir.as_raw_fd(), name_c.as_ptr(), libc::AT_REMOVEDIR) };
            if removed != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(Stop::Pending(format!(
                    "a quarantined directory is not empty after deletion ({error}); new content appeared"
                )));
            }
        } else {
            // SAFETY: the descriptor and C string are live for the call.
            let removed = unsafe { libc::unlinkat(dir.as_raw_fd(), name_c.as_ptr(), 0) };
            if removed != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::NotFound {
                    continue;
                }
                return Err(Stop::Pending(format!(
                    "cannot unlink a quarantined entry ({error})"
                )));
            }
        }
        deleter.entries += 1;
        if deleter.entries % DELETE_BATCH == 0 {
            deleter.session.check()?;
        }
    }
    Ok(())
}

fn list_directory(dir: BorrowedFd) -> Result<Vec<String>, String> {
    // SAFETY: `dup` returns a new descriptor.
    let dup = unsafe { libc::dup(dir.as_raw_fd()) };
    if dup < 0 {
        return Err(format!(
            "cannot read a directory ({})",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `dup` is a live descriptor that `fdopendir` takes ownership of.
    let stream = unsafe { libc::fdopendir(dup) };
    if stream.is_null() {
        // SAFETY: the duplicate is still owned here.
        unsafe { libc::close(dup) };
        return Err(format!(
            "cannot read a directory ({})",
            std::io::Error::last_os_error()
        ));
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: `stream` is a live directory stream.
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            break;
        }
        // SAFETY: `d_name` is a live NUL-terminated array for this entry.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name = name.to_string_lossy().into_owned();
        if name == "." || name == ".." {
            continue;
        }
        names.push(name);
    }
    // SAFETY: `stream` is live and closed once.
    unsafe { libc::closedir(stream) };
    Ok(names)
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
fn prune_one(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    plan_sha: &str,
    list: Option<&OperatorList>,
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
    };
    let outcome = prune_one_barrier(store, control, candidate, plan_sha, list);
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
) -> Result<(), Stop> {
    let _binding = store
        .lock_binding(&candidate.checkout)
        .map_err(Stop::Refuse)?;
    // Everything up to the journal is a per-root refusal: the root, its
    // registry row and every receipt are unchanged.
    let (session, id, quarantine) = (|| {
        let session = RootSession::open(candidate).map_err(Stop::Refuse)?;
        session.verify_marker(candidate).map_err(Stop::Refuse)?;
        session.check()?;
        match checkout_absence_for_candidate(candidate, false) {
            Ok(Absence::Absent) => {}
            Ok(Absence::Present) => {
                return Err(Stop::Refuse(
                    "the checkout reappeared before the prune".to_owned(),
                ))
            }
            Ok(Absence::Unknown(reason)) | Err(reason) => return Err(Stop::Refuse(reason)),
        }
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
            recheck_receiptless(store, control, candidate)
        } else {
            recheck_candidate(store, control, candidate, list)
        }
        .map_err(Stop::Refuse)?;
        // A receiptless legacy root is protected by its exclusive `pruning`
        // receipt before the intent exists; if the intent is not published
        // the receipt is discarded again, with no output effect.
        if candidate.receiptless {
            outputs::create_pruning_receipt(
                control,
                &candidate.checkout,
                &candidate.checkout_id,
                &id,
                &|intent| journaled(control, intent),
            )
            .map_err(Stop::Refuse)?;
        }
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
) -> Result<u64, String> {
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
    if let Some(reason) = eligibility(
        store,
        control,
        &registry,
        &row,
        Some(&receipt),
        list,
        &mut budget,
        true,
    )? {
        return Err(reason);
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
        let row = registry
            .rows
            .iter_mut()
            .find(|row| {
                row.checkout_id == candidate.checkout_id
                    && row.member == candidate.member
                    && row.root == candidate.root
            })
            .ok_or_else(|| Stop::Refuse("the registry row disappeared".to_owned()))?;
        if !row.selected || row.pending || row.pruning.is_some() {
            return Err(Stop::Refuse(
                "the registry row is no longer an eligible selection".to_owned(),
            ));
        }
        row.selected = false;
        row.pruning = Some(id.to_owned());
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
        session,
        deadline: Instant::now() + Duration::from_secs(120),
        entries: 0,
    };
    crash_point("deleting").map_err(Stop::Pending)?;
    delete_contents(&mut deleter, session.root.as_fd(), true)?;
    update_intent(control, id, IntentPhase::MarkerLast)
        .map_err(|error| Stop::Pending(format!("cannot record marker-last: {error}")))?;
    unlink_marker(&session.root).map_err(Stop::Refuse)?;
    if !list_directory(session.root.as_fd())
        .map_err(Stop::Refuse)?
        .is_empty()
    {
        return Err(Stop::Pending(
            "the quarantine gained content after its marker was removed".to_owned(),
        ));
    }
    session
        .remove_quarantine(&format!("{QUARANTINE_PREFIX}{id}"))
        .map_err(Stop::Pending)?;
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
            if let Some(row) = registry.rows.iter_mut().find(|row| {
                row.checkout_id == identity.checkout_id
                    && row.member == identity.member
                    && row.root == identity.root
            }) {
                row.selected = false;
                row.pending = false;
                row.pruning = None;
                row.pruned_ms = Some(process::now_ms());
            }
            registry.bump().map_err(Stop::Refuse)?;
            write_registry(control, &registry).map_err(Stop::Refuse)?;
        }
    }
    crash_point("terminal").map_err(Stop::Pending)?;
    settle_receipt(control, id, outcome).map_err(Stop::Pending)
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
    outputs::mark_pruned(
        control,
        &intent.identity.checkout,
        &intent.identity.checkout_id,
        id,
        outcome,
    )?;
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
                && row.member == record.identity.member
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

fn checkout_absence_for_candidate(candidate: &Candidate, legacy: bool) -> Result<Absence, String> {
    let row = Row {
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
    };
    checkout_absence(&row, legacy)
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
            // any other state is uncertain and stays pending.
            let candidate = Candidate {
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
            };
            let session = match RootSession::open(&candidate) {
                Ok(session) => session,
                Err(original) => {
                    // A crash between the exclusive rename and its journal
                    // phase leaves the root under the intent's own random
                    // quarantine leaf. Only that exact device and inode,
                    // with the recorded marker, resumes deletion.
                    let session = RootSession::open_quarantine(
                        &intent.identity,
                        &quarantine_leaf(intent)?,
                    )
                    .map_err(|quarantine| {
                        format!("the root is not at its path ({original}) or in its quarantine ({quarantine})")
                    })?;
                    session.verify_marker(&candidate)?;
                    update_intent(control, &intent.id, IntentPhase::Renamed).map_err(|error| {
                        format!("cannot record the observed quarantine rename: {error}")
                    })?;
                    return resume_quarantine(store, control, intent, &session);
                }
            };
            session.verify_marker(&candidate)?;
            rollback_intent(store, control, intent)?;
            Ok(Resumed::RolledBack)
        }
    }
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
        } else if let Some(row) = registry.rows.iter_mut().find(|row| matches(row)) {
            row.selected = true;
            row.pending = false;
            row.pruning = None;
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
/// its clean barrier. `Some(reason)` keeps the root.
fn custody_reason(
    store: &Store,
    control: &Path,
    checkout: &Path,
    under_barrier: bool,
) -> Result<Option<String>, String> {
    if under_barrier {
        return Ok(custody_clear_under_barrier(store, control, checkout).err());
    }
    match store.checkout_custody(checkout, 0)? {
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
    for pool_control in [control.to_path_buf(), removable::removable_control(control)] {
        match store::active_holders(&pool_control, checkout) {
            Ok(holders) => {
                if let Some(holder) = holders.into_iter().next() {
                    return Ok(Some(format!(
                        "a recorded writer still holds this checkout ({holder})"
                    )));
                }
            }
            Err(reason) => return Ok(Some(format!("writer records are unreadable ({reason})"))),
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
    let outcome = receiptless_candidate(store, control, registry, root, under_barrier, &mut row);
    match outcome {
        Ok(candidate) => Evaluated {
            row,
            candidate: Some(candidate),
            kept: None,
            legacy: true,
        },
        Err(reason) => Evaluated {
            row,
            candidate: None,
            kept: Some(reason),
            legacy: true,
        },
    }
}

fn receiptless_candidate(
    store: &Store,
    control: &Path,
    registry: &Registry,
    root: &Path,
    under_barrier: bool,
    row: &mut Row,
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
    if !hex64(&row.checkout_id) {
        return Err("the listed root's marker names an unusable checkout identity".to_owned());
    }
    row.receipt = outputs::receipt_path_public(control, &row.checkout_id);
    row.volume_uuid = Some(registration.volume_uuid.clone());
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
        Absence::Absent => {}
        Absence::Present => return Err("the checkout is present, so its root is kept".to_owned()),
        Absence::Unknown(reason) => return Err(format!("checkout absence is unknown ({reason})")),
    }
    if let Some(reason) = custody_reason(store, control, &row.checkout, under_barrier)? {
        return Err(reason);
    }
    let facts = match observe_row_root(row)? {
        RootState::Present(facts) => facts,
        RootState::Absent => return Err("the listed root is already absent".to_owned()),
        RootState::Unknown(reason) => return Err(reason),
    };
    let (logical, allocated, complete) = account_root(root);
    Ok(Candidate {
        checkout_id: row.checkout_id.clone(),
        checkout: row.checkout.clone(),
        receipt: row.receipt.clone(),
        member: row.member.clone(),
        root: row.root.clone(),
        root_kind: RootKind::External,
        volume_uuid: row.volume_uuid.clone(),
        checkout_volume_uuid: None,
        nonce: row.nonce.clone(),
        generation: row.generation,
        dev: facts.dev,
        ino: facts.ino,
        marker_digest: facts.marker_digest,
        logical_bytes: logical,
        allocated_bytes: allocated,
        accounting_complete: complete,
        receiptless: true,
    })
}

/// The locked recheck for a receiptless candidate: the same evaluation under
/// the checkout's barrier, with the root's identity unchanged.
fn recheck_receiptless(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
) -> Result<u64, String> {
    let registry = ready_registry(control)?;
    registry.validate()?;
    let entry = evaluate_receiptless(store, control, &registry, &candidate.root, true);
    let Some(current) = entry.candidate else {
        return Err(entry
            .kept
            .unwrap_or_else(|| "the listed root is no longer eligible".to_owned()));
    };
    if !same_candidates(&[candidate], &[&current]) {
        return Err("the listed root's identity changed".to_owned());
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
    store: &Store,
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
    custody_clear_under_barrier(store, control, &intent.identity.checkout)
}

/// Writer closure for a checkout whose clean barrier and binding lock the
/// caller holds: cooperative custody in both pools is clear and neither
/// pool's state records an active command for it. The caller's own barrier
/// is the only one `begin_clean` admits, so barriers are not counted. An
/// unreadable state in either pool is not closure.
fn custody_clear_under_barrier(
    store: &Store,
    control: &Path,
    checkout: &Path,
) -> Result<(), String> {
    match store.checkout_custody(checkout, 0)? {
        CheckoutCustody::Clear => {}
        CheckoutCustody::Writers => {
            return Err("an active writer still holds the checkout".to_owned())
        }
        CheckoutCustody::Unknown(reason) => {
            return Err(format!("writer custody is unknown ({reason})"))
        }
    }
    if let Some(holder) = store::active_records(control, checkout)?.into_iter().next() {
        return Err(format!(
            "a recorded writer still holds this checkout ({holder})"
        ));
    }
    let removable = removable::removable_control(control);
    if let Some(holder) = store::active_records(&removable, checkout)?
        .into_iter()
        .next()
    {
        return Err(format!(
            "a recorded writer in the removable pool still holds this checkout ({holder})"
        ));
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
        let Some(row) = registry.rows.iter_mut().find(|row| {
            row.checkout_id == intent.identity.checkout_id
                && row.member == intent.identity.member
                && row.root == intent.identity.root
        }) else {
            return Err("the intent's blocking registry row disappeared".to_owned());
        };
        if row.pruning.as_deref() != Some(intent.id.as_str()) {
            return Err("the intent's blocking registry reference changed".to_owned());
        }
        row.selected = false;
        row.pending = false;
        row.pruning = None;
        row.pruned_ms = Some(process::now_ms());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("oxide-prune-{}-{id}", std::process::id()));
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
            revision: 7,
            complete: true,
            incomplete: None,
            created_by: "a".repeat(32),
            rows: Vec::new(),
            intents: Vec::new(),
            cancelled_overflow: Vec::new(),
        };
        let preview = build_preview(&registry, &[], None);
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

    #[test]
    fn the_registry_refuses_unknown_schemas_and_contradictory_incompleteness() {
        let mut registry = Registry {
            version: "oxide.outputs.registry/9".to_owned(),
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
        let (under, rest) = checkout_walk(Path::new("/Volumes/Builds/oxide-outputs/app")).unwrap();
        assert!(under);
        assert_eq!(
            rest,
            vec![
                std::ffi::OsStr::new("oxide-outputs"),
                std::ffi::OsStr::new("app")
            ]
        );
        let (under, rest) = checkout_walk(Path::new("/Users/tom/src/app")).unwrap();
        assert!(!under);
        assert_eq!(
            rest,
            vec![
                std::ffi::OsStr::new("Users"),
                std::ffi::OsStr::new("tom"),
                std::ffi::OsStr::new("src"),
                std::ffi::OsStr::new("app")
            ]
        );
        assert!(checkout_walk(Path::new("/Volumes/Builds")).is_err());
        assert!(checkout_walk(Path::new("/")).is_err());
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
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Absent
        ));
        fs::create_dir_all(&missing).unwrap();
        assert!(matches!(
            checkout_absence(&row, false).unwrap(),
            Absence::Present
        ));
        // A missing ancestor is uncertainty, never absence.
        let mut deep = row_for(&fixture.root);
        deep.checkout = fixture.root.join("missing-ancestor/checkout");
        assert!(matches!(
            checkout_absence(&deep, false).unwrap(),
            Absence::Unknown(_)
        ));
        // A symlinked ancestor is never followed.
        let link = fixture.root.join("linked");
        std::os::unix::fs::symlink(&missing, &link).unwrap();
        let mut linked = row_for(&fixture.root);
        linked.checkout = link.join("child");
        assert!(matches!(
            checkout_absence(&linked, false).unwrap(),
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
        let (dev, ino, digest) = row_identity(&selected).unwrap();
        assert!(dev.is_some() && ino.is_some() && digest.is_none());
        selected = Selected {
            kind: RootKind::Local,
            root: root.clone(),
            volume_uuid: None,
            nonce: Some("a".repeat(32)),
            generation: 1,
        };
        let (_, _, digest) = row_identity(&selected).unwrap();
        assert!(digest.is_none());
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
        let evaluated = evaluate(&store, &control, &registry, None).unwrap();
        assert!(evaluated.iter().all(|entry| entry.candidate.is_none()));
        assert!(evaluated.iter().all(|entry| entry
            .kept
            .as_deref()
            .is_some_and(|reason| reason.contains("whole batch"))));
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
        };
        let mut registry = Registry {
            version: REGISTRY_VERSION.to_owned(),
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
        let control = ready_control(&fixture, vec![row], Vec::new());
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
        assert!(matches!(
            journal_intent(&store, &control, &candidate, &plan, &id, &quarantine, 5),
            Ok(true)
        ));
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
    fn overflow_reservations_bind_by_tag_and_cancel_without_a_row() {
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
