//! Output selection workflow records and Barge area pruning.
//!
//! Receipts and their bounded index describe Oxide member links and pending
//! transitions. Barge alone decides storage ownership, holds, pins and trash.
//! Historical unfinished prune intents refuse adoption; they are never replayed
//! by the replacement and their bytes remain available for explicit recovery.

use crate::outputs::{self, Receipt, RootKind, Selected};
use crate::process;
use crate::store::{self, Store};
use oxide_storage::storage::{AreaConfig, Storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const REGISTRY_VERSION: &str = "oxide.outputs.registry/2";
/// Schema 1 named only a whole `target` placement; schema 2 names one
/// reference per profile member and is read from a version-1 document without
/// changing its meaning.
const LEGACY_REGISTRY_VERSION: &str = "oxide.outputs.registry/1";
pub const INTENT_VERSION: &str = "oxide.outputs.prune-intent/1";
const REGISTRY_FILE: &str = "registry.json";
const REGISTRY_SCHEMA: u32 = 2;
const LEGACY_REGISTRY_SCHEMA: u32 = 1;
const OBSERVATIONS_FILE: &str = "prune-observations.json";
const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ROWS: usize = 4096;
const MAX_INTENTS: usize = 256;
const MAX_TEXT_BYTES: usize = 1024;
const SNAPSHOT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SNAPSHOT_TIME: Duration = Duration::from_secs(10);

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    barge_id: Option<String>,
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

fn registry_dir(control: &Path) -> PathBuf {
    control.join("outputs")
}

/// The internal pool for an output operation, refusing when there is none.
fn internal_store() -> Result<Store, String> {
    let settings = store::settings()?;
    Store::open_existing_with(&settings)?
        .ok_or_else(|| "no internal pool exists; run a building oxide command first".to_owned())
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
                    barge_id: None,
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
            );
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
    pin_references(control, receipt)?;
    // A recorded ID is the generation's identity. Backfill missing IDs from
    // Barge's markers once per area, even when many rows share that area.
    let mut roots_by_area = HashMap::new();
    for row in &mut registry.rows {
        if !row.live()
            || row.barge_id.is_some()
            || !row.root.is_dir()
            || !row.root.join(oxide_storage::storage::ROOT_MARKER).exists()
        {
            continue;
        }
        let selected = Selected {
            kind: row.root_kind,
            root: row.root.clone(),
            volume_uuid: row.volume_uuid.clone(),
            nonce: row.nonce.clone(),
            generation: row.generation,
        };
        let (account, config) = output_area_config(control, &selected)?;
        let roots = match roots_by_area.entry((account, config.uuid.clone())) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let area = Storage::open(&entry.key().0)?.declare(&config)?;
                entry.insert(
                    output_roots(&area)?
                        .into_iter()
                        .map(|root| (root.path, root.id))
                        .collect::<HashMap<_, _>>(),
                )
            }
        };
        row.barge_id = roots.get(&row.root).cloned();
    }
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
                    barge_id: None,
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
                    barge_id: None,
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
        barge_id: None,
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

/// A separate account per physical output location keeps the declaration
/// literally `oxide/outputs`, including checkout-local roots. The identity is
/// the location's stable path, not whether a removable drive is present today.
fn output_area_config(
    control: &Path,
    selected: &Selected,
) -> Result<(PathBuf, AreaConfig), String> {
    let parent = selected
        .root()
        .parent()
        .ok_or("an output root has no parent")?;
    let mount = parent.parent().ok_or("an output area has no mount")?;
    let folder = parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("an output area folder is not UTF-8")?;
    let location = sha256_hex(parent.as_os_str().as_encoded_bytes());
    let account = control.join("output-storage").join(location);
    let uuid = if oxide_storage::storage::is_drive_mount_point(mount) {
        selected.volume_uuid.clone()
    } else {
        None
    };
    Ok((
        account,
        AreaConfig::new("outputs", mount, "outputs")
            .uuid(uuid)
            .folder(folder)
            .floor_bytes(0),
    ))
}

fn output_area(control: &Path, selected: &Selected) -> Result<barge::Area, String> {
    let (account, config) = output_area_config(control, selected)?;
    Storage::open(&account)?.declare(&config)
}

/// Inventory for identity and protection only. Callers may hold admission.
fn output_roots(area: &barge::Area) -> Result<Vec<barge::RootInfo>, String> {
    let listing = area
        .list_without_bytes()
        .map_err(|error| error.to_string())?;
    #[cfg(test)]
    observe_output_listing(&listing);
    Ok(listing.roots)
}

#[cfg(test)]
thread_local! {
    static OUTPUT_LISTINGS: std::cell::Cell<Option<(usize, usize)>> = const {
        std::cell::Cell::new(None)
    };
}

#[cfg(test)]
fn observe_output_listing(listing: &barge::Listing) {
    OUTPUT_LISTINGS.with(|counts| {
        if let Some((calls, sized)) = counts.get() {
            // Every root in the regression has nonempty contents. Measuring
            // one produces bytes (or an approximation on an incomplete walk).
            let measured = listing
                .roots
                .iter()
                .filter(|root| root.bytes != 0 || root.bytes_approximate)
                .count();
            counts.set(Some((calls + 1, sized + measured)));
        }
    });
}

#[cfg(test)]
pub(crate) fn count_output_listings<T>(action: impl FnOnce() -> T) -> (T, (usize, usize)) {
    OUTPUT_LISTINGS.with(|counts| {
        assert!(counts.replace(Some((0, 0))).is_none());
        let result = action();
        (result, counts.take().unwrap())
    })
}

fn existing_output_area(
    control: &Path,
    selected: &Selected,
) -> Result<Option<barge::Area>, String> {
    let (account, config) = output_area_config(control, selected)?;
    Storage::open_read_only(&account)?.existing_area(&config)
}

fn output_id(selected: &Selected) -> String {
    sha256_hex(format!("{}:{}", selected.root().display(), selected.generation()).as_bytes())
}

/// Re-pin an already owned Barge identity without using mutable Oxide marker
/// fields as new storage authority. A first import requires the Oxide marker
/// and selection to agree; a retired ID can never re-import its former path.
/// An imported output root. `pinned` is true only when Barge already protects
/// the root: its inventory showed a pin, or its pin record is present. The
/// caller then skips the pin write; every other case reports `false` and pins.
struct Imported {
    area: barge::Area,
    id: String,
    pinned: bool,
}

/// Whether Barge's durable pin record exists for a root. Barge keeps it at
/// `<root parent>/.barge/pins/<id>.json` and counts its presence as a pin.
/// That layout is Barge-private, so a test pins through Barge and checks this
/// reader against it. Only a positively present record answers `true`; a
/// missing or unreadable one answers `false`, so the caller pins again.
fn pin_record_exists(root: &Path, id: &str) -> bool {
    let Some(parent) = root.parent() else {
        return false;
    };
    let record = parent
        .join(".barge")
        .join("pins")
        .join(format!("{id}.json"));
    fs::symlink_metadata(record).is_ok()
}

/// Pin a selected root unless Barge already protects it. A second pin only
/// rewrites the pin record and its timestamp.
fn pin_selected(imported: &Imported) -> Result<(), String> {
    if imported.pinned {
        return Ok(());
    }
    imported
        .area
        .pin(&imported.id, "selected by an Oxide output receipt")
        .map_err(|error| error.to_string())
}

/// `registry` is the caller's single read of the registry for this command.
fn import_output(
    control: &Path,
    registry: Option<&Registry>,
    checkout: &Path,
    checkout_id: &str,
    selected: &Selected,
) -> Result<Imported, String> {
    if registry.is_some_and(|registry| {
        registry.rows.iter().any(|row| {
            row.root == selected.root()
                && row.nonce.as_deref() == selected.nonce()
                && row.pruned_ms.is_some()
        })
    }) {
        return Err("a retired output generation cannot adopt its old path".to_owned());
    }
    let area = output_area(control, selected)?;
    if let Some(id) = registry.and_then(|registry| {
        registry
            .rows
            .iter()
            .find(|row| row.root == selected.root() && row.barge_id.is_some())
            .and_then(|row| row.barge_id.clone())
    }) {
        let root = area.resolve(&id).map_err(|error| {
            format!("the recorded Barge output generation is unavailable: {error}")
        })?;
        if root.path != selected.root() {
            return Err("the recorded Barge output ID moved".to_owned());
        }
        // This path reads no inventory. Its pin state comes from the pin
        // record's presence alone, which is cheap and never a listing.
        let pinned = pin_record_exists(&root.path, &id);
        return Ok(Imported { area, id, pinned });
    }
    // The Barge marker and account can already own this path even before
    // its first Oxide index row is committed. Re-pinning it is conservative;
    // the caller still validates every Oxide link and marker before use.
    let existing = output_roots(&area)?
        .into_iter()
        .find(|root| root.path == selected.root());
    if let Some(root) = existing {
        return Ok(Imported {
            area,
            id: root.id,
            pinned: root.pinned,
        });
    }
    let marker = outputs::read_marker_for_prune(selected.root())?;
    if !marker.is_owned_by(checkout, checkout_id)
        || marker.kind() != selected.kind()
        || marker.volume_uuid() != selected.volume_uuid()
        || selected
            .nonce()
            .is_some_and(|nonce| marker.nonce() != Some(nonce))
    {
        return Err(format!(
            "{} has a foreign output identity; retained",
            selected.root().display()
        ));
    }
    let mut identity = selected.clone();
    identity.generation = marker.generation();
    let id = output_id(&identity);
    let folder = selected
        .root()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("an output root folder is not UTF-8")?;
    // Resolve first: an existing matching import keeps exactly its path.
    match area.resolve(&id) {
        Ok(root) if root.path == selected.root() => {}
        Ok(_) => return Err("the output ID resolves to another path".to_owned()),
        Err(error) if error.kind() == barge::ErrorKind::NotFound => {
            area.adopt_root(folder, "outputs", &id)
                .map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(Imported {
        area,
        id,
        pinned: false,
    })
}

fn pin_references(control: &Path, receipt: &Receipt) -> Result<(), String> {
    let registry = read_registry(control)?;
    for placement in receipt.links() {
        for (selected, pending) in placement
            .selected_ref()
            .filter(|_| receipt.holds_selection())
            .map(|selected| (selected, false))
            .into_iter()
            .chain(
                placement
                    .prepared_ref()
                    .map(|prepared| (&prepared.to, true)),
            )
        {
            match fs::symlink_metadata(selected.root()) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.to_string()),
                Ok(_) => {}
            }
            if (pending
                || !selected
                    .root()
                    .join(oxide_storage::storage::ROOT_MARKER)
                    .exists())
                && fs::symlink_metadata(selected.root().join(outputs::MARKER))
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                continue;
            }
            let imported = import_output(
                control,
                registry.as_ref(),
                receipt.checkout(),
                receipt.checkout_id(),
                selected,
            )?;
            pin_selected(&imported)?;
        }
    }
    Ok(())
}

/// Pin the newly established root before a member link can expose it.
pub(crate) fn pin_output(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    selected: &Selected,
) -> Result<(), String> {
    let registry = read_registry(control)?;
    let imported = import_output(control, registry.as_ref(), checkout, checkout_id, selected)?;
    pin_selected(&imported)?;
    record_import(control, selected.root(), &imported.id)
}

/// Release old selections only after their replacement receipt is durable.
/// A crash before this step leaves an extra pin, never an exposed selection.
pub(crate) fn finish_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    let registry = ready_registry(control)?;
    for row in registry.rows.iter().filter(|row| {
        row.checkout_id == receipt.checkout_id()
            && !row.selected
            && !row.pending
            && row.reservation.is_none()
            && row.pruned_ms.is_none()
    }) {
        if registry
            .rows
            .iter()
            .any(|other| other.root == row.root && other.live())
        {
            continue;
        }
        let Some(id) = row.barge_id.as_deref() else {
            continue;
        };
        let selected = Selected {
            kind: row.root_kind,
            root: row.root.clone(),
            volume_uuid: row.volume_uuid.clone(),
            nonce: row.nonce.clone(),
            generation: row.generation,
        };
        let area = output_area(control, &selected)?;
        match area.unpin(&id) {
            Ok(()) => {}
            Err(error) if error.kind() == barge::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

/// Command holds use the same Barge roots as pruning. Descriptors survive the
/// real child exec; a child that closes them leaves this cooperative contract.
pub(crate) fn hold_outputs(
    control: &Path,
    receipt: &Receipt,
    uses_link: impl Fn(&str) -> bool,
) -> Result<Vec<barge::Hold>, String> {
    let mut held = Vec::new();
    let mut ids = std::collections::HashSet::new();
    let registry = read_registry(control)?;
    for selected in receipt
        .links()
        .iter()
        .filter(|placement| uses_link(placement.member()))
        .filter_map(|placement| placement.selected_ref())
    {
        if !receipt.holds_selection() || !ids.insert(selected.root().to_path_buf()) {
            continue;
        }
        let imported = import_output(
            control,
            registry.as_ref(),
            receipt.checkout(),
            receipt.checkout_id(),
            selected,
        )?;
        pin_selected(&imported)?;
        let hold = imported
            .area
            .try_hold(&imported.id, barge::Mode::Shared)
            .map_err(|error| error.to_string())?
            .ok_or("the output root is held exclusively")?;
        held.push(hold.into_inheritable().map_err(|error| error.to_string())?);
    }
    Ok(held)
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlannedRoot {
    area: String,
    id: String,
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    version: String,
    registry_revision: u64,
    roots: Vec<PlannedRoot>,
}

/// The reason a root is kept while a historical prune intent names it. Barge
/// never replays or settles old deletion journals, so there is no automatic
/// path: an operator with writers stopped recovers the bytes by hand. This is
/// the one wording the outputs refusals reuse.
pub(crate) const RETAINED_BY_INTENT: &str = "a historical prune intent names this root, so it is retained. Barge does not replay or settle old deletion journals; with writers stopped, inspect the paths recorded for the intent in outputs/registry.json and recover them by hand";

fn preview(control: &Path, registry: &Registry) -> Result<Preview, String> {
    let mut roots = Vec::new();
    let mut locations = std::collections::HashSet::new();
    // Only the roots an unfinished intent names are kept; the rest of the
    // registry still previews and executes.
    let retained: std::collections::HashSet<&Path> = registry
        .unfinished_intents()
        .map(|intent| intent.identity.root.as_path())
        .collect();
    for row in &registry.rows {
        if row.pruned_ms.is_some() || row.barge_id.is_none() {
            continue;
        }
        let selected = Selected {
            kind: row.root_kind,
            root: row.root.clone(),
            volume_uuid: row.volume_uuid.clone(),
            nonce: row.nonce.clone(),
            generation: row.generation,
        };
        let Some(parent) = selected.root().parent() else {
            continue;
        };
        if !locations.insert(parent.to_path_buf()) {
            continue;
        }
        let Some(area) = existing_output_area(control, &selected)? else {
            continue;
        };
        for root in output_roots(&area)? {
            if retained.contains(root.path.as_path()) {
                continue;
            }
            let recorded = registry
                .rows
                .iter()
                .filter(|row| {
                    row.root == root.path && row.pruned_ms.is_none() && row.barge_id.is_some()
                })
                .collect::<Vec<_>>();
            if recorded.is_empty() {
                continue;
            }
            if recorded
                .iter()
                .any(|row| row.barge_id.as_deref() != Some(&root.id))
            {
                return Err(format!(
                    "{} no longer names its recorded Barge root; retained",
                    root.path.display()
                ));
            }
            if root.idle_and_unpinned() {
                roots.push(PlannedRoot {
                    area: area.key().label(),
                    id: root.id,
                    path: root.path,
                });
            }
        }
    }
    roots.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
    roots.dedup();
    Ok(Preview {
        version: "oxide.outputs.barge-prune/2".to_owned(),
        registry_revision: registry.revision,
        roots,
    })
}

/// What Barge did with one planned root.
enum Disposition {
    /// The root and its trash copy are gone.
    Removed,
    /// The root left its path, but its trash copy could not be deleted. Its
    /// bytes still exist and free no space; the registry settles it anyway.
    MovedToTrash(String),
}

fn evict_planned_root(
    control: &Path,
    registry: &Registry,
    planned: &PlannedRoot,
) -> Result<Disposition, String> {
    let row = registry
        .rows
        .iter()
        .find(|row| {
            row.root == planned.path
                && row.barge_id.as_deref() == Some(&planned.id)
                && row.pruned_ms.is_none()
        })
        .ok_or("the planned Barge identity is no longer recorded; retained")?;
    let selected = Selected {
        kind: row.root_kind,
        root: row.root.clone(),
        volume_uuid: row.volume_uuid.clone(),
        nonce: row.nonce.clone(),
        generation: row.generation,
    };
    let area = output_area(control, &selected)?;
    if area.key().label() != planned.area {
        return Err("the planned Barge area changed; retained".to_owned());
    }
    let root = area
        .resolve(&planned.id)
        .map_err(|error| error.to_string())?;
    if root.path != planned.path {
        return Err("the planned Barge root moved; retained".to_owned());
    }
    // The supplier must validate this ID's current marker under its locks
    // through rename; an outer path check cannot supply that authority.
    match area.evict(&planned.id) {
        Ok(evicted) if evicted.deleted => Ok(Disposition::Removed),
        Ok(evicted) => Ok(Disposition::MovedToTrash(
            evicted
                .kept_reason
                .unwrap_or_else(|| "reason unknown".to_owned()),
        )),
        // The same post-rename failure the pool settles: the root has left its
        // path, so the registry must not keep referencing it.
        Err(error) if oxide_storage::storage::eviction_moved(&error) => {
            Ok(Disposition::MovedToTrash(error.to_string()))
        }
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug)]
enum EvictError {
    Retained(String),
    Publication(String),
}

fn evict_planned(
    control: &Path,
    registry: &mut Registry,
    planned: &PlannedRoot,
) -> Result<Disposition, EvictError> {
    let disposition =
        evict_planned_root(control, registry, planned).map_err(EvictError::Retained)?;
    for row in registry
        .rows
        .iter_mut()
        .filter(|row| row.root == planned.path && row.barge_id.as_deref() == Some(&planned.id))
    {
        row.pruned_ms = Some(process::now_ms());
    }
    registry
        .bump()
        .and_then(|()| write_registry(control, registry))
        .map_err(|error| {
            EvictError::Publication(format!(
                "Barge evicted {} ({}), but the output registry could not be published: {error}",
                planned.path.display(),
                planned.id
            ))
        })?;
    Ok(disposition)
}

/// Evict each planned root and return how many were not freed: kept by Barge,
/// or moved to trash with its deletion failed.
fn execute_plan(
    control: &Path,
    registry: &mut Registry,
    planned: &Preview,
) -> Result<usize, String> {
    let mut not_freed = 0;
    for root in &planned.roots {
        match evict_planned(control, registry, root) {
            Ok(Disposition::Removed) => println!("{}: removed", root.path.display()),
            Ok(Disposition::MovedToTrash(reason)) => {
                not_freed += 1;
                println!(
                    "{}: moved to trash; deletion failed: {reason}",
                    root.path.display()
                )
            }
            Err(EvictError::Retained(error)) => {
                not_freed += 1;
                eprintln!("oxide: kept {}: {error}", root.path.display())
            }
            Err(EvictError::Publication(error)) => return Err(error),
        }
    }
    Ok(not_freed)
}

/// One accepted command shape. Parsing happens before any store is opened, so
/// a malformed invocation changes nothing.
enum Command<'a> {
    DryRun,
    Apply { path: &'a str, sha256: &'a str },
}

const USAGE: &str = "usage: oxide outputs prune --dry-run | --plan FILE --plan-sha256 HEX";

fn parse_command<'a>(words: &[&'a str]) -> Result<Command<'a>, String> {
    match words {
        ["prune", "--dry-run"] => Ok(Command::DryRun),
        ["prune", "--plan", path, "--plan-sha256", sha256] => Ok(Command::Apply { path, sha256 }),
        _ => Err(USAGE.to_owned()),
    }
}

pub fn run(args: &[std::ffi::OsString]) -> Result<u8, String> {
    let words = args
        .iter()
        .map(|word| word.to_str().ok_or("arguments must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    if matches!(words.as_slice(), ["prune", "--help" | "-h"]) {
        println!("usage: oxide outputs prune --dry-run\n       oxide outputs prune --plan FILE --plan-sha256 HEX\n\nBarge prunes idle unpinned output roots; selected member roots stay pinned.");
        return Ok(0);
    }
    let command = parse_command(&words)?;
    let dry_run = matches!(command, Command::DryRun);
    let internal = if dry_run {
        Store::open_recorded(&store::settings()?)?
            .ok_or("no internal pool exists; run a building oxide command first")?
    } else {
        internal_store()?
    };
    let control = internal.owner_control();
    let _guard = if dry_run {
        None
    } else {
        Some(store::publication_guard(control)?)
    };
    let mut registry = require_ready(&internal, control)?;
    // Only the roots an unfinished intent names are refused, below in preview.
    for intent in registry.unfinished_intents() {
        eprintln!(
            "oxide: kept {}: {RETAINED_BY_INTENT}",
            intent.identity.root.display()
        );
    }
    let planned = preview(control, &registry)?;
    let bytes = serde_json::to_vec_pretty(&planned).map_err(|error| error.to_string())?;
    match command {
        Command::DryRun => {
            println!("{}", String::from_utf8_lossy(&bytes));
            eprintln!("plan-sha256: {}", sha256_hex(&bytes));
            Ok(0)
        }
        Command::Apply { path, sha256 } => {
            let mut given = Vec::new();
            File::open(path)
                .and_then(|file| file.take(MAX_REGISTRY_BYTES + 1).read_to_end(&mut given))
                .map_err(|error| error.to_string())?;
            if given.len() as u64 > MAX_REGISTRY_BYTES {
                return Err("the output prune plan exceeds its byte limit".to_owned());
            }
            let given = given.strip_suffix(b"\n").unwrap_or(&given);
            if sha256_hex(given) != sha256 || given != bytes {
                return Err("the output prune plan changed; take a new preview".to_owned());
            }
            let not_freed = execute_plan(control, &mut registry, &planned)?;
            if not_freed == 0 {
                return Ok(0);
            }
            eprintln!("oxide: {not_freed} planned root(s) were not freed");
            Ok(1)
        }
    }
}

pub fn status_lines(control: &Path) -> Vec<String> {
    match read_registry(control) {
        Ok(Some(registry)) => vec![format!(
            "outputs: {} references; pruning uses Barge holds and pins",
            registry.rows.len()
        )],
        Ok(None) => vec!["outputs: no selections recorded".to_owned()],
        Err(reason) => vec![format!("outputs: unavailable ({reason})")],
    }
}

#[cfg(test)]
pub(crate) fn test_output_storage(control: &Path, selected: &Selected) -> (barge::Area, String) {
    let area = output_area(control, selected).unwrap();
    let id = output_roots(&area)
        .unwrap()
        .into_iter()
        .find(|root| root.path == selected.root())
        .unwrap()
        .id;
    (area, id)
}

/// Fill the adoption fixture's area with roots and live registry rows, some
/// written before Barge IDs were recorded. Return the expected identities.
#[cfg(test)]
pub(crate) fn seed_output_neighbors(
    control: &Path,
    selected: &Selected,
    count: usize,
) -> HashMap<PathBuf, String> {
    let area = output_area(control, selected).unwrap();
    let mut registry = ready_registry(control).unwrap();
    let mut expected = HashMap::new();
    for index in 0..count {
        let root = area.create_root("neighbor").unwrap();
        fs::create_dir(root.path.join("debug")).unwrap();
        fs::write(root.path.join("debug/payload"), [42; 4096]).unwrap();
        let checkout = control.join(format!("neighbor-{index}"));
        let checkout_id = sha256_hex(checkout.as_os_str().as_encoded_bytes());
        registry.rows.push(Row {
            receipt: outputs::receipt_path_public(control, &checkout_id),
            checkout_id,
            checkout,
            checkout_volume_uuid: None,
            member: "debug".to_owned(),
            root: root.path.clone(),
            barge_id: (index % 2 == 0).then(|| root.id.clone()),
            root_kind: selected.kind(),
            volume_uuid: selected.volume_uuid().map(str::to_owned),
            recipe: outputs::RECIPE_PUBLIC.to_owned(),
            nonce: None,
            generation: 1,
            root_dev: None,
            root_ino: None,
            marker_digest: None,
            selected: true,
            pending: false,
            pruning: None,
            pruned_ms: None,
            reservation: None,
            cancelling: None,
        });
        expected.insert(root.path, root.id);
    }
    write_registry(control, &registry).unwrap();
    // Positive control: the same inventory observer sees every sized root
    // when Barge is asked to measure the fixture.
    let ((), counts) = count_output_listings(|| observe_output_listing(&area.list().unwrap()));
    assert_eq!(counts, (1, count));
    expected
}

#[cfg(test)]
pub(crate) fn assert_output_neighbors(control: &Path, expected: &HashMap<PathBuf, String>) {
    let registry = ready_registry(control).unwrap();
    let mut matched = 0;
    for row in &registry.rows {
        if let Some(id) = expected.get(&row.root) {
            assert_eq!(row.barge_id.as_ref(), Some(id));
            assert_eq!(
                fs::read(row.root.join("debug/payload")).unwrap(),
                [42; 4096]
            );
            matched += 1;
        }
    }
    assert_eq!(matched, expected.len());
}

/// Whether an earlier owned generation occupied this path. A missing or
/// replaced old marker is never ownership for another generation.
pub(crate) fn needs_successor(control: &Path, root: &Path) -> Result<bool, String> {
    let Some(registry) = read_registry(control)? else {
        return Ok(false);
    };
    for row in registry.rows.iter().filter(|row| row.root == root) {
        if row.pruned_ms.is_some() {
            return Ok(true);
        }
        if let Some(id) = &row.barge_id {
            let selected = Selected {
                kind: row.root_kind,
                root: row.root.clone(),
                volume_uuid: row.volume_uuid.clone(),
                nonce: row.nonce.clone(),
                generation: row.generation,
            };
            match output_area(control, &selected)?.resolve(id) {
                Ok(found) if found.path == root => {}
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == barge::ErrorKind::NotFound => return Ok(true),
                Err(error) => return Err(error.to_string()),
            }
        } else if row.marker_digest.is_some()
            && fs::symlink_metadata(root.join(outputs::MARKER))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn fresh_output(control: &Path, selected: &Selected) -> Result<barge::Root, String> {
    let area = output_area(control, selected)?;
    let root = area
        .create_root("outputs")
        .map_err(|error| error.to_string())?;
    area.pin(&root.id, "Oxide output transition")
        .map_err(|error| error.to_string())?;
    Ok(root)
}

fn record_import(control: &Path, root: &Path, id: &str) -> Result<(), String> {
    let Some(mut registry) = read_registry(control)? else {
        return Ok(());
    };
    let mut changed = false;
    for row in registry.rows.iter_mut().filter(|row| row.root == root) {
        if row.barge_id.as_deref() != Some(id) {
            row.barge_id = Some(id.to_owned());
            changed = true;
        }
    }
    if changed {
        registry.bump()?;
        write_registry(control, &registry)?;
    }
    Ok(())
}

#[cfg(test)]
mod plan_identity_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture {
        _temp: barge::testing::TempDir,
        control: PathBuf,
        area: barge::Area,
        original: barge::Root,
        registry: Registry,
        plan: Preview,
    }

    fn fixture() -> Fixture {
        let fixture = barge::testing::TempDir::new("oxide-prune-identity").unwrap();
        let control = fixture.path().join("control");
        fs::create_dir(&control).unwrap();
        oxide_storage::storage::make_private(&control).unwrap();
        seed_test_registry(&control).unwrap();
        let selected = Selected {
            kind: RootKind::Local,
            root: fixture.path().join("outputs/placeholder"),
            volume_uuid: None,
            nonce: None,
            generation: 1,
        };
        let area = output_area(&control, &selected).unwrap();
        let original = area.create_root("victim").unwrap();
        let mut registry = ready_registry(&control).unwrap();
        registry.rows.push(Row {
            checkout_id: "a".repeat(64),
            checkout: fixture.path().join("checkout"),
            checkout_volume_uuid: None,
            receipt: control.join("receipt.json"),
            member: "debug".to_owned(),
            root: original.path.clone(),
            root_kind: RootKind::Local,
            volume_uuid: None,
            nonce: None,
            generation: 1,
            root_dev: None,
            root_ino: None,
            marker_digest: None,
            selected: false,
            pending: false,
            pruning: None,
            pruned_ms: None,
            reservation: None,
            cancelling: None,
            barge_id: Some(original.id.clone()),
            recipe: "test".to_owned(),
        });
        // An older unmapped history row supplies no storage authority.
        let mut historical = registry.rows[0].clone();
        historical.member = "target".to_owned();
        historical.barge_id = None;
        registry.rows.push(historical);
        write_registry(&control, &registry).unwrap();
        let plan = preview(&control, &registry).unwrap();
        assert_eq!(plan.roots.len(), 1);
        assert_eq!(plan.roots[0].id, original.id);
        assert_eq!(plan.roots[0].area, "oxide/outputs");
        Fixture {
            _temp: fixture,
            control,
            area,
            original,
            registry,
            plan,
        }
    }

    #[test]
    fn registry_write_failure_after_eviction_is_a_failed_execution() {
        let Fixture {
            _temp,
            control,
            area,
            original,
            mut registry,
            plan,
        } = fixture();
        let path = registry_path(&control);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("block-publication"), b"keep").unwrap();
        let error = execute_plan(&control, &mut registry, &plan).unwrap_err();
        assert!(error.contains("Barge evicted"), "{error}");
        assert!(error.contains("could not be published"), "{error}");
        assert!(!original.path.exists());
        assert!(area.resolve(&original.id).is_err());
        assert_eq!(fs::read(path.join("block-publication")).unwrap(), b"keep");
    }

    #[test]
    fn replacing_a_root_at_the_planned_path_never_deletes_the_replacement() {
        let Fixture {
            _temp,
            control,
            area,
            original,
            mut registry,
            plan,
        } = fixture();
        let revision = registry.revision;
        area.evict(&original.id).unwrap();
        fs::create_dir(&original.path).unwrap();
        let folder = original.path.file_name().unwrap().to_str().unwrap();
        let replacement = area
            .adopt_root(folder, "replacement", "replacement-id")
            .unwrap();
        fs::write(replacement.path.join("keep"), b"replacement bytes").unwrap();
        assert!(evict_planned(&control, &mut registry, &plan.roots[0]).is_err());
        assert_eq!(registry.revision, revision);
        assert_eq!(
            fs::read(replacement.path.join("keep")).unwrap(),
            b"replacement bytes"
        );
        assert!(preview(&control, &registry)
            .unwrap_err()
            .contains("recorded Barge root"));
    }

    /// Restore owner write access under `path` so a fixture can be removed
    /// after a test deliberately made a tree undeletable.
    fn make_writable(path: &Path) {
        if fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            for entry in fs::read_dir(path).unwrap() {
                make_writable(&entry.unwrap().path());
            }
        }
    }

    /// Barge moves the root out of its path before a deletion can fail. The
    /// registry must settle that root like any eviction, and the plan must not
    /// count the still-present trash copy as freed.
    #[test]
    fn a_root_moved_to_trash_is_settled_and_not_counted_as_freed() {
        // SAFETY: geteuid has no preconditions and cannot fail. Root deletes
        // through an unwritable directory, so the failure cannot be forced.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let Fixture {
            _temp,
            control,
            area,
            original,
            mut registry,
            plan,
        } = fixture();
        let protected = original.path.join("protected");
        fs::create_dir(&protected).unwrap();
        fs::write(protected.join("file"), b"kept after failed unlink").unwrap();
        fs::set_permissions(&protected, fs::Permissions::from_mode(0o500)).unwrap();

        let not_freed = execute_plan(&control, &mut registry, &plan).unwrap();

        assert_eq!(not_freed, 1);
        assert!(!original.path.exists());
        assert!(area.resolve(&original.id).is_err());
        let settled = ready_registry(&control)
            .unwrap()
            .rows
            .into_iter()
            .find(|row| row.root == original.path)
            .unwrap();
        assert!(settled.pruned_ms.is_some());
        make_writable(_temp.path());
    }

    /// A root Barge keeps (here: held) is not freed, and the plan says so.
    #[test]
    fn a_kept_root_is_counted_as_not_freed() {
        let Fixture {
            _temp,
            control,
            area,
            original,
            mut registry,
            plan,
        } = fixture();
        let _hold = area
            .try_hold(&original.id, barge::Mode::Shared)
            .unwrap()
            .expect("the root is free to hold");

        let not_freed = execute_plan(&control, &mut registry, &plan).unwrap();

        assert_eq!(not_freed, 1);
        assert!(original.path.is_dir());
        let row = ready_registry(&control)
            .unwrap()
            .rows
            .into_iter()
            .find(|row| row.root == original.path)
            .unwrap();
        assert!(row.pruned_ms.is_none());
    }

    /// An unsettled legacy intent keeps its own root out of the plan, and no
    /// other root is refused with it.
    #[test]
    fn an_unsettled_intent_retains_only_the_root_it_names() {
        let Fixture {
            _temp,
            control,
            area,
            original,
            ..
        } = fixture();
        let neighbor = area.create_root("neighbor").unwrap();
        let mut registry = ready_registry(&control).unwrap();
        let mut row = registry.rows[0].clone();
        row.root = neighbor.path.clone();
        row.barge_id = Some(neighbor.id.clone());
        registry.rows.push(row);
        write_registry(&control, &registry).unwrap();
        seed_test_intent(&control, &"a".repeat(64), false);
        let mut registry = ready_registry(&control).unwrap();
        registry.intents[0].identity.root = original.path.clone();
        registry.bump().unwrap();
        write_registry(&control, &registry).unwrap();

        let plan = preview(&control, &ready_registry(&control).unwrap()).unwrap();

        let planned: Vec<&str> = plan.roots.iter().map(|root| root.id.as_str()).collect();
        assert_eq!(planned, [neighbor.id.as_str()]);
    }

    #[test]
    fn malformed_invocations_fail_before_any_store_is_touched() {
        for words in [
            vec!["prune"],
            vec!["prune", "--plan", "plan.json"],
            vec!["resolve-intent", "0123"],
        ] {
            let error = parse_command(&words).err().unwrap();
            assert_eq!(error, USAGE);
        }
        assert!(matches!(
            parse_command(&["prune", "--dry-run"]),
            Ok(Command::DryRun)
        ));
        assert!(!USAGE.contains("resolve-intent"));
    }

    /// Importing an unrecorded root reads the inventory that identifies it,
    /// and that read reports whether a pin already protects the root. A pinned
    /// root is reported as such, so its pin write is skipped; an unpinned root
    /// is not, so it is pinned.
    #[test]
    fn import_reports_the_pin_its_inventory_already_shows() {
        let Fixture {
            _temp,
            control,
            area,
            ..
        } = fixture();
        let pinned = area.create_root("already-pinned").unwrap();
        area.pin(&pinned.id, "earlier pin").unwrap();
        let plain = area.create_root("unpinned").unwrap();
        let checkout = _temp.path().join("checkout");
        let checkout_id = "a".repeat(64);
        let import = |root: &barge::Root| {
            let selected = Selected {
                kind: RootKind::Local,
                root: root.path.clone(),
                volume_uuid: None,
                nonce: None,
                generation: 1,
            };
            import_output(&control, None, &checkout, &checkout_id, &selected).unwrap()
        };

        let already = import(&pinned);
        assert_eq!(already.id, pinned.id);
        assert!(already.pinned);
        let fresh = import(&plain);
        assert_eq!(fresh.id, plain.id);
        assert!(!fresh.pinned);
    }

    /// The pin-record reader follows Barge's own pin and unpin. If Barge moves
    /// its record, this fails before a skip could trust a stale answer.
    #[test]
    fn the_pin_record_reader_agrees_with_barge_pins() {
        let Fixture { _temp, area, .. } = fixture();
        let root = area.create_root("reader").unwrap();
        assert!(!pin_record_exists(&root.path, &root.id));
        area.pin(&root.id, "reader pin").unwrap();
        assert!(pin_record_exists(&root.path, &root.id));
        area.unpin(&root.id).unwrap();
        assert!(!pin_record_exists(&root.path, &root.id));
    }

    /// A recorded root reads no inventory, so its pin state must come from its
    /// pin record: an existing pin is skipped, and an unpinned root is pinned.
    #[test]
    fn a_recorded_import_reports_the_pin_its_record_shows() {
        let Fixture {
            _temp,
            control,
            area,
            original,
            registry,
            ..
        } = fixture();
        let checkout = _temp.path().join("checkout");
        let checkout_id = "a".repeat(64);
        let selected = Selected {
            kind: RootKind::Local,
            root: original.path.clone(),
            volume_uuid: None,
            nonce: None,
            generation: 1,
        };
        let import = || {
            import_output(
                &control,
                Some(&registry),
                &checkout,
                &checkout_id,
                &selected,
            )
            .unwrap()
        };

        assert!(!import().pinned);
        area.pin(&original.id, "recorded pin").unwrap();
        let recorded = import();
        assert_eq!(recorded.id, original.id);
        assert!(recorded.pinned);
        area.unpin(&original.id).unwrap();
        assert!(!import().pinned);
    }
}
