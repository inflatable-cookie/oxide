//! Final outputs on a permanent drive: one owned checkout `target` link.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Final outputs on
//! the permanent drive). Ephemeral drive mode keeps today's real
//! `<checkout>/target`. Permanent mode needs one explicit conversion
//! ([`adopt`]): `<checkout>/target` becomes an owned symlink pointing at a
//! real output root outside the pool payload, either
//! `<mount>/oxide-outputs/<label>-<checkout id>/` on the registered volume or
//! the checkout-local fallback `<checkout>/.oxide-outputs/local/` while the
//! volume is absent. `CARGO_TARGET_DIR` stays the lexical
//! `<checkout>/target`, so `target/debug` and the profiles of #106 keep
//! working.
//!
//! Nothing is moved, copied or deleted by a transition. Ownership is a marker
//! inside the root plus a committed receipt in the internal pool's control
//! directory; a link whose target does not carry this checkout's marker is
//! foreign and is never followed, adopted, swapped or cleaned through.
//! Transitions are written ahead: a `prepared` receipt, then the destination
//! root and marker, then a unique sibling link, then an atomic rename onto
//! `target`, then the committed receipt. A crash replays only the exact
//! generations the receipt and the live links name; any contradiction stops
//! with both roots retained.
//!
//! The owning lock is the internal pool's existing admission lock, together
//! with the checkout's existing clean/admission barrier: `oxide clean` and
//! `oxide outputs adopt|detach` use [`Store::begin_clean`], and a building
//! command holds its checkout's lease, which that barrier waits for. Receipt
//! writes happen under the admission lock, so one transition is published at
//! a time.

use crate::identity;
use crate::process;
use crate::removable::{self, DriveMode};
use crate::store::{self, CheckoutCustody, CleanGuard, Cleaning, Store, WaitAccounting};
use crate::CARGO;
use oxide_storage::volume::{self, Binding};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsFd;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Directory under the volume mount that holds every checkout's output root.
pub const EXTERNAL_DIR: &str = "oxide-outputs";
/// Directory inside the checkout that holds the local fallback root.
pub const LOCAL_DIR: &str = ".oxide-outputs";
/// The local fallback root's name inside [`LOCAL_DIR`].
const LOCAL_ROOT: &str = "local";
/// Ownership marker inside every output root.
pub const MARKER: &str = ".oxide-output.json";
/// The gitignore entry `oxide init` adds for the local fallback root.
pub const GITIGNORE_LINE: &str = "/.oxide-outputs/";

const RECEIPT_VERSION: &str = "oxide.outputs.transition/2";
/// Version-1 receipts are read as legacy and upgraded on the next write,
/// only behind the version-2 forward fence (oxide#129).
const LEGACY_RECEIPT_VERSION: &str = "oxide.outputs.transition/1";
const MARKER_VERSION: &str = "oxide.outputs.root/2";
const LEGACY_MARKER_VERSION: &str = "oxide.outputs.root/1";
const PLAN_VERSION: &str = "oxide.outputs.plan/1";
/// The placement recipe a marker binds: `target/` with `target/debug`.
const RECIPE: &str = "target-debug-v1";
/// The receipt's one placement member (oxide#129): the whole checkout target.
pub(crate) const RECIPE_PUBLIC: &str = RECIPE;
pub(crate) const MEMBER: &str = "target";
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const MAX_MARKER_BYTES: u64 = 64 * 1024;
const MAX_PLAN_BYTES: usize = 64 * 1024;
/// Known roots kept per receipt; the oldest is dropped first.
const MAX_ROOTS: usize = 8;
/// Cooperative holders named in a plan.
const MAX_HOLDERS: usize = 16;
/// Debug-only crash injection, naming a write-ahead point.
const CRASH_ENVIRONMENT: &str = "OXIDE_TEST_OUTPUTS_CRASH";
/// One bounded Git probe's time limit.
const GIT_PROBE_TIME: Duration = Duration::from_secs(5);
/// Byte cap for one Git probe's stdout.
const MAX_GIT_PROBE_BYTES: usize = 64 * 1024;
/// Repository-addressing variables a probe removes: a first conversion asks
/// about this checkout's real work tree, never an inherited one.
const GIT_REPOSITORY_ENVIRONMENT: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

const USAGE: &str = "\
usage: oxide outputs plan      effect-free JSON plan for this checkout (oxide.outputs.plan/1)
       oxide outputs adopt     explicitly convert <checkout>/target to an owned link
       oxide outputs detach    restore a real <checkout>/target directory; roots are kept
       oxide outputs status    read-only state of this checkout's owned outputs
       oxide outputs prune     explicitly prune roots whose checkout is gone
       oxide outputs resolve-intent ID  settle one pending prune intent

Permanent drive mode owns one `<checkout>/target` symlink. Its real root is
<mount>/oxide-outputs/<project>-<checkout id>/ while the registered volume is
usable, else the checkout-local fallback <checkout>/.oxide-outputs/local/.
`plan` and `adopt` never move, copy or delete an existing target directory or
an arbitrary symlink: a nonempty real target and a foreign symlink refuse
without any change. Only an explicit `adopt` converts an empty target. A
missing target converts by `adopt` or on the first leased build of a fresh
checkout (no receipt of any state) on the registered permanent drive, with
one notice. `detach` works in either mode: it restores this checkout's real
target and keeps every root, and automatic conversion never re-links it.";

/// Which real root a link selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    External,
    Local,
}

impl RootKind {
    fn label(self) -> &'static str {
        match self {
            Self::External => "external",
            Self::Local => "local fallback",
        }
    }
}

/// One selected real output root and the volume it is bound to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub(crate) kind: RootKind,
    pub(crate) root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) volume_uuid: Option<String>,
    /// The root's immutable 128-bit nonce. Absent only for a root created
    /// before oxide#129; a recreated root always gets a fresh one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nonce: Option<String>,
    /// The generation this selection was committed at.
    pub(crate) generation: u64,
}

impl Selected {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn kind(&self) -> RootKind {
        self.kind
    }

    pub(crate) fn volume_uuid(&self) -> Option<&str> {
        self.volume_uuid.as_deref()
    }

    pub(crate) fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    fn same_place(&self, other: &Self) -> bool {
        self.kind == other.kind && self.root == other.root && self.volume_uuid == other.volume_uuid
    }

    fn view(&self) -> SelectedView {
        SelectedView {
            kind: self.kind,
            root: self.root.clone(),
            volume_uuid: self.volume_uuid.clone(),
            generation: self.generation,
        }
    }

    /// Bind a fresh nonce to a newly created root, or keep the root's own.
    fn with_nonce(mut self, nonce: Option<String>) -> Self {
        self.nonce = nonce;
        self
    }

    fn at(&self, generation: u64) -> Self {
        Self {
            generation,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct SelectedView {
    kind: RootKind,
    root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    generation: u64,
}

/// A transition written ahead but not committed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prepared {
    pub(crate) generation: u64,
    pub(crate) to: Selected,
    link: PathBuf,
}

/// Receipt state. `Prepared` is the only state a crash can leave behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptState {
    Committed,
    Prepared,
    Detached,
    /// The selected root was deleted or resolved by an explicit operator
    /// prune; the receipt keeps the exact intent as evidence.
    Pruned,
    /// An operator-listed receiptless legacy root of this checkout is being
    /// pruned under the recorded intent (oxide#129). Every bind, adopt,
    /// automatic adoption, detach, clean and replay refuses until it settles.
    Pruning,
}

/// The checkout's owned-output receipt, keyed by canonical checkout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Receipt {
    version: String,
    checkout: PathBuf,
    checkout_id: String,
    /// Last committed generation; zero before the first conversion.
    generation: u64,
    state: ReceiptState,
    /// Exactly one placement, `target`, in this tranche (oxide#129). Roots
    /// hold immutable nonces; placements only reference them.
    links: Vec<LinkPlacement>,
    /// Bounded history of roots this checkout has selected, newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roots: Vec<Selected>,
    /// The terminal outcome of an explicit prune, when this receipt was
    /// pruned or resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pruned: Option<PruneRecord>,
    updated_ms: u64,
}

/// The terminal prune evidence a checkout's receipt keeps: the exact intent
/// and whether its root was deleted or settled by an operator.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PruneRecord {
    pub(crate) intent: String,
    pub(crate) outcome: String,
    pub(crate) at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkPlacement {
    member: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<Selected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prepared: Option<Prepared>,
    /// A durable random nonce reserved for a future root before a command's
    /// slot is keyed (oxide#129). Commands that want the identical future
    /// root share it; it is consumed by the commit that creates the root or
    /// removed only by the cancellation journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserved: Option<Reservation>,
}

/// A reserved future root: its transaction ID, root nonce and the exact
/// destination it was reserved for.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub(crate) id: String,
    pub(crate) nonce: String,
    pub(crate) kind: RootKind,
    pub(crate) root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) volume_uuid: Option<String>,
    pub(crate) generation: u64,
    pub(crate) created_ms: u64,
    /// Present only when the registry was already durably sticky-incomplete:
    /// the reservation is backed by this receipt transaction, never by an
    /// invented registry row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) overflow: Option<OverflowTag>,
}

/// The explicit overflow binding of an unindexed reservation: the registry
/// identity, the reservation transaction and nonce, and the global revision
/// it was made at (provenance, not a freshness requirement).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverflowTag {
    pub(crate) registry: String,
    pub(crate) transaction: String,
    pub(crate) nonce: String,
    pub(crate) revision: u64,
}

impl Reservation {
    /// Whether this reservation is for exactly `wanted`'s future root.
    fn is_for(&self, wanted: &Selected) -> bool {
        self.kind == wanted.kind
            && self.root == wanted.root
            && self.volume_uuid == wanted.volume_uuid
            && self.generation == wanted.generation
    }
}

impl Receipt {
    fn first(checkout: &Path, checkout_id: &str) -> Self {
        Self {
            version: RECEIPT_VERSION.to_owned(),
            checkout: checkout.to_path_buf(),
            checkout_id: checkout_id.to_owned(),
            generation: 0,
            state: ReceiptState::Detached,
            links: vec![LinkPlacement {
                member: MEMBER.to_owned(),
                selected: None,
                prepared: None,
                reserved: None,
            }],
            roots: Vec::new(),
            pruned: None,
            updated_ms: process::now_ms(),
        }
    }

    fn link(&self) -> &LinkPlacement {
        &self.links[0]
    }

    fn link_mut(&mut self) -> &mut LinkPlacement {
        &mut self.links[0]
    }

    pub(crate) fn checkout(&self) -> &Path {
        &self.checkout
    }

    pub(crate) fn checkout_id(&self) -> &str {
        &self.checkout_id
    }

    /// Whether this receipt currently selects a root.
    pub(crate) fn selects(&self) -> bool {
        self.state == ReceiptState::Committed
    }

    /// Whether the selected placement is still live: a committed receipt and
    /// a prepared transition both keep their committed root selected.
    pub(crate) fn holds_selection(&self) -> bool {
        matches!(self.state, ReceiptState::Committed | ReceiptState::Prepared)
    }

    /// The placement's selected root, whether or not the state is committed.
    pub(crate) fn current(&self) -> Option<&Selected> {
        self.link().selected.as_ref()
    }

    pub(crate) fn prepared_ref(&self) -> Option<&Prepared> {
        self.link().prepared.as_ref()
    }

    pub(crate) fn reserved(&self) -> Option<&Reservation> {
        self.link().reserved.as_ref()
    }

    fn validate(&self, checkout: &Path, checkout_id: &str) -> Result<(), String> {
        if self.version != RECEIPT_VERSION {
            return Err(format!(
                "the owned-output receipt has an unsupported version {:?}",
                self.version
            ));
        }
        if self.checkout != checkout || self.checkout_id != checkout_id {
            return Err(format!(
                "the owned-output receipt names another checkout ({}); it is not followed",
                self.checkout.display()
            ));
        }
        if self.roots.len() > MAX_ROOTS {
            return Err("the owned-output receipt holds too many roots".to_owned());
        }
        if self.links.len() != 1 || self.links[0].member != MEMBER {
            return Err(
                "the owned-output receipt must hold exactly one `target` placement".to_owned(),
            );
        }
        let selected = self.link().selected.as_ref();
        let prepared = self.link().prepared.as_ref();
        if let Some(selected) = selected {
            if !safe_absolute(&selected.root) || selected.generation > self.generation {
                return Err(
                    "the owned-output receipt selects an unusable root or generation".to_owned(),
                );
            }
            if let Some(nonce) = selected.nonce() {
                if nonce.len() != 32 {
                    return Err("the owned-output receipt has an unusable root nonce".to_owned());
                }
            }
        }
        if let Some(reserved) = self.reserved() {
            let tagged = reserved
                .overflow
                .as_ref()
                .is_none_or(|tag| tag.transaction == reserved.id && tag.nonce == reserved.nonce);
            if !crate::prune::is_nonce(&reserved.id)
                || !crate::prune::is_nonce(&reserved.nonce)
                || !safe_absolute(&reserved.root)
                || !tagged
            {
                return Err("the owned-output receipt has an unusable reservation".to_owned());
            }
        }
        for root in &self.roots {
            // A rolled-back preparation may have created a root at the next
            // generation without advancing the committed one.
            if !safe_absolute(&root.root) || root.generation > self.generation.saturating_add(1) {
                return Err(
                    "the owned-output receipt names an unusable known root or generation"
                        .to_owned(),
                );
            }
        }
        match (self.state, selected, prepared) {
            (ReceiptState::Committed, Some(_), None) => {}
            (ReceiptState::Detached, None, None) => {}
            (ReceiptState::Pruned, _, None) => {}
            (ReceiptState::Pruning, None, None) if self.pruned.is_some() => {}
            (ReceiptState::Prepared, _, Some(prepared))
                if prepared.generation == self.generation.saturating_add(1)
                    && prepared.generation > 0
                    && prepared.link == checkout.join("target")
                    && safe_absolute(&prepared.to.root) => {}
            _ => {
                return Err(
                    "the owned-output receipt is internally inconsistent; it is not followed"
                        .to_owned(),
                )
            }
        }
        Ok(())
    }

    /// The root the current owned link must select, when committed.
    fn selected(&self) -> Option<&Selected> {
        match self.state {
            ReceiptState::Committed => self.current(),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for Receipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Doc {
            version: String,
            checkout: PathBuf,
            checkout_id: String,
            generation: u64,
            state: ReceiptState,
            #[serde(default)]
            links: Vec<LinkPlacement>,
            #[serde(default)]
            selected: Option<Selected>,
            #[serde(default)]
            prepared: Option<Prepared>,
            #[serde(default)]
            roots: Vec<Selected>,
            #[serde(default)]
            pruned: Option<PruneRecord>,
            updated_ms: u64,
        }
        let doc = Doc::deserialize(deserializer)?;
        let version = match doc.version.as_str() {
            RECEIPT_VERSION => RECEIPT_VERSION.to_owned(),
            LEGACY_RECEIPT_VERSION => RECEIPT_VERSION.to_owned(),
            other => other.to_owned(),
        };
        let legacy = doc.version == LEGACY_RECEIPT_VERSION;
        let links = if legacy {
            if !doc.links.is_empty() {
                return Err(serde::de::Error::custom(
                    "a version-1 receipt cannot carry placements",
                ));
            }
            vec![LinkPlacement {
                member: MEMBER.to_owned(),
                selected: doc.selected,
                prepared: doc.prepared,
                reserved: None,
            }]
        } else {
            if doc.selected.is_some() || doc.prepared.is_some() {
                return Err(serde::de::Error::custom(
                    "a version-2 receipt carries placements, not top-level references",
                ));
            }
            doc.links
        };
        Ok(Receipt {
            version,
            checkout: doc.checkout,
            checkout_id: doc.checkout_id,
            generation: doc.generation,
            state: doc.state,
            links,
            roots: doc.roots,
            pruned: doc.pruned,
            updated_ms: doc.updated_ms,
        })
    }
}

/// Ownership marker inside an output root. Written once, at root creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Marker {
    version: String,
    checkout: PathBuf,
    checkout_id: String,
    label: String,
    recipe: String,
    kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    /// The root's immutable nonce. Version-1 markers predate it; a version-2
    /// marker always carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    /// The APFS volume UUID of the checkout when the root was created
    /// (shared `oxide.outputs.root/2`); absent in version-1 markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkout_volume_uuid: Option<String>,
    /// The generation the root was created at; never rewritten.
    generation: u64,
    created_ms: u64,
}

impl Marker {
    pub(crate) fn is_legacy(&self) -> bool {
        self.version == LEGACY_MARKER_VERSION
    }

    pub(crate) fn is_owned_by(&self, checkout: &Path, checkout_id: &str) -> bool {
        self.checkout == checkout && self.checkout_id == checkout_id
    }

    pub(crate) fn kind(&self) -> RootKind {
        self.kind
    }

    pub(crate) fn checkout(&self) -> &Path {
        &self.checkout
    }

    pub(crate) fn checkout_id(&self) -> &str {
        &self.checkout_id
    }

    pub(crate) fn volume_uuid(&self) -> Option<&str> {
        self.volume_uuid.as_deref()
    }

    pub(crate) fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != MARKER_VERSION && self.version != LEGACY_MARKER_VERSION {
            return Err(format!(
                "owner marker version {:?} is not supported",
                self.version
            ));
        }
        if self.version == MARKER_VERSION
            && self.nonce.as_deref().is_none_or(|nonce| {
                nonce.len() != 32
                    || !nonce
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err("a version-2 owner marker needs a 128-bit lowercase hex nonce".to_owned());
        }
        Ok(())
    }
}

/// How an output root is found and created.
#[derive(Clone, Debug)]
enum RootSpec {
    External {
        root: PathBuf,
        uuid: String,
        /// One directory name under `<mount>/oxide-outputs`.
        leaf: String,
    },
    Local {
        root: PathBuf,
    },
}

impl RootSpec {
    fn root(&self) -> &Path {
        match self {
            Self::External { root, .. } | Self::Local { root } => root,
        }
    }

    fn selected(&self, generation: u64) -> Selected {
        match self {
            Self::External { root, uuid, .. } => Selected {
                kind: RootKind::External,
                root: root.clone(),
                volume_uuid: Some(uuid.clone()),
                nonce: None,
                generation,
            },
            Self::Local { root } => Selected {
                kind: RootKind::Local,
                root: root.clone(),
                volume_uuid: None,
                nonce: None,
                generation,
            },
        }
    }
}

/// What `<checkout>/target` is right now.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TargetState {
    Missing,
    RealEmpty,
    RealNonEmpty,
    /// A symlink whose target is a real directory carrying this checkout's
    /// ownership marker.
    OwnedLink(PathBuf),
    /// A symlink whose target does not exist at all (the registered volume is
    /// away, or a fresh fallback root has not been created yet). Its text is
    /// only acted on when it matches the receipt's committed selection.
    Dangling(PathBuf),
    /// Any other symlink or non-directory.
    Foreign,
}

impl TargetState {
    fn label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::RealEmpty => "an empty directory",
            Self::RealNonEmpty => "a directory with content",
            Self::OwnedLink(_) => "an owned Oxide link",
            Self::Dangling(_) => "a link whose root is not available",
            Self::Foreign => "a symlink or file Oxide does not own",
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::RealEmpty => "real-empty",
            Self::RealNonEmpty => "real-nonempty",
            Self::OwnedLink(_) => "owned-link",
            Self::Dangling(_) => "dangling-link",
            Self::Foreign => "foreign",
        }
    }

    /// The root the link names when it is this checkout's owned link, whether
    /// or not that root is available right now.
    fn link_root(&self) -> Option<&Path> {
        match self {
            Self::OwnedLink(root) | Self::Dangling(root) => Some(root),
            _ => None,
        }
    }

    /// The verified real root: an owned link whose root is available.
    fn root(&self) -> Option<&Path> {
        match self {
            Self::OwnedLink(root) => Some(root),
            _ => None,
        }
    }
}

/// What one command did to its checkout's owned outputs.
#[derive(Default)]
pub struct Outcome {
    /// Bounded, single-line warnings, printed before Cargo starts.
    pub warnings: Vec<String>,
    /// One clause for the command's summary line.
    pub note: Option<String>,
    /// The real owned root the checkout's owned link selects for this
    /// command; `None` for a real `target` directory.
    pub root: Option<PathBuf>,
}

impl Outcome {
    fn with_note(mut self, note: String) -> Self {
        self.note = Some(note);
        self
    }
}

/// `oxide outputs ...`
pub fn run(args: &[OsString]) -> Result<u8, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        ["plan"] => plan_command(),
        ["prune", ..] | ["resolve-intent", ..] => crate::prune::run(
            &args
                .iter()
                .map(|arg| OsString::from(*arg))
                .collect::<Vec<_>>(),
        ),
        ["adopt"] => adopt_command(),
        ["detach"] => detach_command(),
        ["cancel-reservation"] => cancel_command(),
        ["status"] => status_command(),
        ["-h" | "--help"] | [] => {
            println!("{USAGE}");
            Ok(0)
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// One checkout's output context. Read-only: a receipt read never creates a
/// directory.
struct Context {
    control: Option<PathBuf>,
    workspace: PathBuf,
    checkout_id: String,
    /// Why this command's intermediate slot comes from the internal pool
    /// although a removable drive is registered. Outputs then never select
    /// the external root: only a removable store supervises the drive's
    /// removal, so a command never writes to it unwatched.
    pool_fallback: Option<String>,
}

impl Context {
    fn open() -> Result<Self, String> {
        let settings = store::settings()?;
        let workspace = identity::locate_workspace(CARGO, None)?;
        let control =
            Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
        Ok(Self::new(control, workspace))
    }

    fn new(control: Option<PathBuf>, workspace: PathBuf) -> Self {
        let checkout_id = identity::digest(&workspace.to_string_lossy());
        Self {
            control,
            workspace,
            checkout_id,
            pool_fallback: None,
        }
    }

    fn with_pool_fallback(mut self, reason: Option<&str>) -> Self {
        self.pool_fallback = reason.map(str::to_owned);
        self
    }

    fn control(&self) -> Option<&Path> {
        self.control.as_deref()
    }

    fn mode(&self) -> Result<Option<DriveMode>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => Ok(removable::read_registration(control)?.map(|item| item.mode)),
        }
    }

    fn receipt(&self) -> Result<Option<Receipt>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => read_receipt(control, &self.workspace, &self.checkout_id),
        }
    }

    fn target(&self) -> Result<TargetState, String> {
        target_state(&self.workspace, &self.checkout_id)
    }

    fn link(&self) -> PathBuf {
        self.workspace.join("target")
    }
}

fn mode_label(mode: Option<DriveMode>) -> &'static str {
    match mode {
        Some(DriveMode::Permanent) => "permanent",
        Some(DriveMode::Ephemeral) => "ephemeral",
        None => "unregistered",
    }
}

fn plan_command() -> Result<u8, String> {
    let context = Context::open()?;
    let report = plan_report(&context)?;
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("cannot encode the output plan: {error}"))?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(format!(
            "the output plan exceeds its {MAX_PLAN_BYTES}-byte limit; nothing was changed"
        ));
    }
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(0)
}

/// The effect-free conversion plan: bounded JSON, no writes, no barrier.
#[derive(Serialize)]
struct PlanReport {
    version: &'static str,
    checkout: PathBuf,
    mode: &'static str,
    state: &'static str,
    current_target: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<SelectedView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposed_root: Option<SelectedView>,
    proposed_fallback_root: SelectedView,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Always false: a conversion never moves existing output.
    data_move: bool,
    cooperative_holders: Vec<String>,
    next_generation: u64,
}

fn plan_report(context: &Context) -> Result<PlanReport, String> {
    let mode = context.mode()?;
    let receipt = context.receipt()?;
    let target = context.target()?;
    let state = receipt
        .as_ref()
        .map_or("none", |receipt| match receipt.state {
            ReceiptState::Committed => "committed",
            ReceiptState::Prepared => "prepared",
            ReceiptState::Detached => "detached",
            ReceiptState::Pruned => "pruned",
            ReceiptState::Pruning => "pruning",
        });
    let next_generation = receipt
        .as_ref()
        .map_or(1, |receipt| receipt.generation.saturating_add(1));
    let holders = match context.control() {
        None => Vec::new(),
        Some(control) => {
            // Holders in either pool: a removable command's record lives in
            // the removable pool's control directory.
            let mut holders = store::active_holders(control, &context.workspace)?;
            holders.extend(store::active_holders(
                &removable::removable_control(control),
                &context.workspace,
            )?);
            holders.truncate(MAX_HOLDERS);
            holders
        }
    };
    let usable = usable_external(context);
    let next = next_generation;
    let external = usable
        .as_ref()
        .ok()
        .map(|registration| external_spec(registration, &context.workspace));
    let local = local_spec(&context.workspace);
    let proposed = external
        .as_ref()
        .map(|spec| spec.selected(destination_generation(spec.root(), next)));
    let fallback = local.selected(destination_generation(local.root(), next));
    let mut report = PlanReport {
        version: PLAN_VERSION,
        checkout: context.workspace.clone(),
        mode: mode_label(mode),
        state,
        current_target: target.code(),
        current_root: target.link_root().map(Path::to_path_buf),
        selected: receipt
            .as_ref()
            .and_then(Receipt::selected)
            .map(Selected::view),
        proposed_root: proposed.as_ref().map(Selected::view),
        proposed_fallback_root: fallback.view(),
        action: "refuse",
        reason: None,
        data_move: false,
        cooperative_holders: holders,
        next_generation,
    };
    if mode != Some(DriveMode::Permanent) {
        report.reason = Some(
            "the registered drive is not in permanent mode, so there is no owned-output policy to apply"
                .to_owned(),
        );
        return Ok(report);
    }
    let Some(receipt) = receipt else {
        match target {
            TargetState::Missing | TargetState::RealEmpty => {
                report.action = "create-owned-link";
                if let Err(reason) = &usable {
                    report.reason = Some(format!(
                        "the volume is not usable ({reason}); the link would select the checkout-local fallback root"
                    ));
                }
            }
            TargetState::RealNonEmpty => {
                report.reason = Some(
                    "the existing target directory has content; Oxide never moves, adopts or deletes it. Move it aside yourself if you accept losing its warm build, then re-run `oxide outputs adopt`"
                        .to_owned(),
                );
            }
            TargetState::Foreign => {
                report.reason = Some(
                    "the existing target is a symlink or file Oxide does not own; it is never followed, adopted or replaced"
                        .to_owned(),
                );
            }
            TargetState::OwnedLink(_) | TargetState::Dangling(_) => {
                report.reason = Some(
                    "the target is a link but this checkout has no receipt; remove it yourself before adopting"
                        .to_owned(),
                );
            }
        }
        return Ok(report);
    };
    if receipt.state == ReceiptState::Prepared {
        report.reason = Some(format!(
            "an interrupted transition to {} is pending; run `oxide outputs adopt` to replay it (its exact generations are rechecked)",
            receipt.prepared_ref().map_or_else(
                || "a new root".to_owned(),
                |prepared| prepared.to.root.display().to_string()
            )
        ));
        return Ok(report);
    }
    if receipt.state == ReceiptState::Detached {
        report.reason = Some(
            "the checkout is detached from owned outputs; `oxide outputs adopt` converts its target again"
                .to_owned(),
        );
        return Ok(report);
    }
    let Some(selected) = receipt.selected().cloned() else {
        report.reason = Some(
            "the receipt selects no root; `oxide outputs adopt` starts a conversion".to_owned(),
        );
        return Ok(report);
    };
    match (&target, target.link_root()) {
        (_, Some(root)) if root == selected.root.as_path() => {
            let wanted = match &usable {
                Ok(registration) => {
                    external_spec(registration, &context.workspace).selected(next_generation)
                }
                Err(_) => local.selected(next_generation),
            };
            report.action = if wanted.same_place(&selected) {
                "noop"
            } else {
                "relink"
            };
            if let Err(reason) = &usable {
                report.reason = Some(format!(
                    "the volume is not usable ({reason}); the next command would select {}",
                    local.root().display()
                ));
            }
            Ok(report)
        }
        _ => {
            report.reason = Some(format!(
                "the receipt selects {} but the target is {}; `oxide outputs adopt` rechecks and relinks it",
                selected.root.display(),
                target.label()
            ));
            Ok(report)
        }
    }
}

fn status_command() -> Result<u8, String> {
    for line in status_lines_for_current()? {
        println!("{line}");
    }
    Ok(0)
}

fn status_lines_for_current() -> Result<Vec<String>, String> {
    let context = Context::open()?;
    output_status_lines(&context)
}

/// Read-only owned-output lines for `oxide status --measure`. The control
/// directory comes from the recorded pool the command already read, so this
/// opens no pool and never creates a pool, directory or receipt.
pub fn status_lines_for(control: Option<PathBuf>, workspace: &Path) -> Result<Vec<String>, String> {
    output_status_lines(&Context::new(control, workspace.to_path_buf()))
}

fn output_status_lines(context: &Context) -> Result<Vec<String>, String> {
    let mode = context.mode()?;
    let target = context.target()?;
    let receipt = context.receipt()?;
    let clause = |selected: &Selected| match selected.kind {
        RootKind::External => format!(
            "{} (volume {})",
            selected.root.display(),
            selected.volume_uuid.as_deref().unwrap_or("unknown")
        ),
        RootKind::Local => format!("{} (checkout-local fallback)", selected.root.display()),
    };
    let mut lines = Vec::new();
    match &receipt {
        None => lines.push(format!(
            "outputs: mode {}, this checkout is not converted (target {}); `oxide outputs plan` shows the conversion plan",
            mode_label(mode),
            target.label()
        )),
        Some(receipt) => {
            let state = match receipt.state {
                ReceiptState::Committed => receipt
                    .selected()
                    .map_or_else(|| "no selected root".to_owned(), clause),
                ReceiptState::Pruned => receipt
                    .pruned
                    .as_ref()
                    .map(|record| {
                        format!(
                            "the recorded root was pruned ({}, intent {})",
                            record.outcome, record.intent
                        )
                    })
                    .unwrap_or_else(|| "the recorded root was pruned".to_owned()),
                ReceiptState::Pruning => {
                    "a listed legacy root of this checkout is being pruned".to_owned()
                }
                ReceiptState::Prepared => format!(
                    "an interrupted transition to {} is pending",
                    receipt
                        .prepared_ref()
                        .as_ref()
                        .map_or_else(|| "a new root".to_owned(), |prepared| clause(&prepared.to))
                ),
                ReceiptState::Detached => "detached; no root is selected".to_owned(),
            };
            lines.push(format!(
                "outputs: mode {}, generation {}, {state}, target {}",
                mode_label(mode),
                receipt.generation,
                target.label()
            ));
        }
    }
    let local = local_spec(&context.workspace);
    let selected_local = receipt
        .as_ref()
        .and_then(Receipt::selected)
        .is_some_and(|selected| selected.kind == RootKind::Local);
    lines.push(format!(
        "outputs: local fallback root {} ({}{})",
        local.root().display(),
        if local.root().is_dir() {
            "present"
        } else {
            "absent"
        },
        if selected_local {
            ", selected"
        } else {
            ", not selected"
        }
    ));
    if let Some(receipt) = &receipt {
        for root in &receipt.roots {
            if receipt.selected() == Some(root) {
                continue;
            }
            lines.push(format!(
                "outputs: preserved {} root {} ({}, never deleted)",
                root.kind.label(),
                root.root.display(),
                if root.root.is_dir() {
                    "present"
                } else {
                    "absent"
                }
            ));
        }
        if let Some(selected) = receipt.selected() {
            if target.link_root() != Some(selected.root.as_path()) {
                lines.push(format!(
                    "outputs: the target is {} rather than the owned link selecting {}; run `oxide outputs adopt`",
                    target.label(),
                    selected.root.display()
                ));
            }
        }
    }
    Ok(lines)
}

/// `oxide outputs adopt`: the explicit first conversion and later relinks.
fn adopt_command() -> Result<u8, String> {
    let context = Context::open()?;
    let mode = context.mode()?;
    if mode != Some(DriveMode::Permanent) {
        return Err(format!(
            "outputs adopt needs permanent drive mode (mode is {}): register the drive with `oxide pool register-removable <mount> --reserve <size> --cap <size> --mode permanent` or `oxide pool set-mode permanent`",
            mode_label(mode)
        ));
    }
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err(
            "no internal pool exists; run a building oxide command once so the control state exists"
                .to_owned(),
        );
    };
    let Some(internal) = output_store()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(&internal, &context.workspace, &mut waits)?;
    if let Some(intent) = crate::prune::unsettled_for(&control, &context.checkout_id)? {
        return Err(format!(
            "outputs: prune intent {intent} is unsettled for this checkout; `adopt` refuses until it completes or `oxide outputs resolve-intent {intent}` settles it"
        ));
    }
    let outcome = (|| {
        refuse_unknown_custody(&internal, &context.workspace, "relink")?;
        // Fence a pre-feature Oxide out of both pools before any link or
        // receipt can be published: an existing receipt may need a replay
        // write. A target adopt refuses outright with no receipt changes
        // nothing, so it initializes nothing.
        if read_receipt(&control, &context.workspace, &context.checkout_id)?.is_some()
            || !matches!(
                context.target()?,
                TargetState::RealNonEmpty | TargetState::Foreign
            )
        {
            crate::prune::ensure_ready(&internal, &context.workspace)?;
        }
        let _admission = store::publication_guard(&control)?;
        let receipt = match read_receipt(&control, &context.workspace, &context.checkout_id)? {
            None => Receipt::first(&context.workspace, &context.checkout_id),
            Some(receipt) => replay(&context, receipt)?,
        };
        adopt_under_barrier(&context, receipt)
    })();
    release_barrier(&internal, guard)?;
    let (receipt, selected, linked) = outcome?;
    if linked {
        println!(
            "outputs: {} now selects the owned {} root {} (generation {})",
            context.link().display(),
            selected.kind.label(),
            selected.root.display(),
            selected.generation
        );
        println!(
            "outputs: no existing output was moved, copied or deleted; CARGO_TARGET_DIR stays {}",
            context.link().display()
        );
    } else {
        println!(
            "outputs: {} already selects the owned {} root {} (generation {})",
            context.link().display(),
            selected.kind.label(),
            selected.root.display(),
            selected.generation
        );
    }
    println!(
        "outputs: receipt generation {}, {} known root(s)",
        receipt.generation,
        receipt.roots.len()
    );
    Ok(0)
}

fn adopt_under_barrier(
    context: &Context,
    receipt: Receipt,
) -> Result<(Receipt, Selected, bool), String> {
    let target = context.target()?;
    let usable = usable_external(context);
    let spec = match &usable {
        Ok(registration) => external_spec(registration, &context.workspace),
        Err(_) => local_spec(&context.workspace),
    };
    let current = receipt.selected().cloned();
    match (&target, &current) {
        // Already converted to the place this command would select.
        (_, Some(selected))
            if target.link_root() == Some(selected.root.as_path())
                && spec.selected(0).same_place(selected) =>
        {
            ensure_root(context, &spec, receipt.generation)?;
            let selected = selected.clone();
            Ok((receipt, selected, false))
        }
        // A committed receipt whose owned link selects another known place:
        // relink under the barrier.
        (_, Some(selected)) if target.link_root() == Some(selected.root.as_path()) => {
            let committed = transition(context, &receipt, &spec, Start::Any)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // A committed receipt whose link is missing: recreate it.
        (TargetState::Missing, Some(_)) => {
            let committed = transition(context, &receipt, &spec, Start::Any)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // Explicit first conversion, or one after `detach`.
        (TargetState::Missing | TargetState::RealEmpty, _) => {
            let committed = transition(context, &receipt, &spec, Start::Any)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        _ => Err(refuse_existing_target(context, &target)),
    }
}

fn refuse_existing_target(context: &Context, target: &TargetState) -> String {
    format!(
        "outputs adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed. {}",
        context.link().display(),
        target.label(),
        match target {
            TargetState::RealNonEmpty =>
                "Move it aside yourself if you accept losing its warm build, then re-run `oxide outputs adopt`.",
            TargetState::OwnedLink(_) | TargetState::Dangling(_) =>
                "Its receipt does not select this root, so Oxide will not touch it.",
            _ =>
                "Remove or restore it yourself, then re-run `oxide outputs adopt`.",
        }
    )
}

/// The facts the automatic first conversion decides on. Every field must hold;
/// any other state keeps today's behaviour.
#[derive(Debug, Eq, PartialEq)]
struct AutoFacts {
    /// This command's intermediate slot came from the removable pool under a
    /// registration in permanent mode, as selected at its start.
    selected_permanent: bool,
    /// The external root the registration in force now selects is the one the
    /// command's selection captured: same volume UUID and root.
    same_selection: bool,
    /// The registered permanent volume is usable right now.
    external_usable: bool,
    /// The registration in force now is in permanent mode.
    permanent: bool,
    /// No owned-output receipt of any state, including `detached`.
    receipt_absent: bool,
    /// `<checkout>/target` is positively absent: no file, directory or link.
    target_absent: bool,
}

fn auto_adopts(facts: &AutoFacts) -> bool {
    facts.selected_permanent
        && facts.same_selection
        && facts.external_usable
        && facts.permanent
        && facts.receipt_absent
        && facts.target_absent
}

/// Whether the external root a registration selects is the one a command
/// captured at its selection (same volume, same root).
fn selection_matches(captured: &Selected, current: &Selected) -> bool {
    captured.same_place(current)
}

/// Automatic first conversion (oxide#121), run once per leased command before
/// its destination is planned, its key is built or its slot is taken. The
/// command's own selection must be a removable permanent one, and the volume
/// in force now must be the one that selection captured. A checkout with no
/// receipt of any state and a positively absent target then converts under the
/// same barrier, fences, admission lock and receipt as `oxide outputs adopt`,
/// with the missing-target-only transition. Any other state is left to the
/// existing paths untouched. A metadata error refuses; it is never read as
/// absence.
pub fn auto_adopt(
    store: &Store,
    checkout: &Path,
    selection: &removable::Selected,
) -> Result<(), String> {
    let selected_permanent = selection.removable && selection.mode == Some(DriveMode::Permanent);
    let Some(captured_registration) = selection
        .registration
        .as_ref()
        .filter(|_| selected_permanent)
    else {
        return Ok(());
    };
    let captured = external_spec(captured_registration, checkout).selected(0);
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        checkout.to_path_buf(),
    );
    let current = usable_external(&context);
    let facts = AutoFacts {
        selected_permanent,
        same_selection: current.as_ref().is_ok_and(|registration| {
            selection_matches(
                &captured,
                &external_spec(registration, checkout).selected(0),
            )
        }),
        external_usable: current.is_ok(),
        permanent: context.mode()? == Some(DriveMode::Permanent),
        receipt_absent: context.receipt()?.is_none(),
        target_absent: context.target()? == TargetState::Missing,
    };
    if !auto_adopts(&facts) {
        return Ok(());
    }
    let mut waits = WaitAccounting::default();
    let guard = barrier(store, checkout, &mut waits)?;
    let outcome = (|| {
        refuse_unknown_custody(store, checkout, "auto-adopt")?;
        // Fence a pre-feature Oxide out of both pools before any link can be
        // published, as an explicit adopt does for a missing target.
        crate::prune::ensure_ready(store, checkout)?;
        let control = context
            .control()
            .ok_or("no internal pool exists, so no removable drive is registered")?;
        let _admission = store::publication_guard(control)?;
        // Every fact is rechecked under the admission lock: a concurrent
        // creator, receipt, mode or volume switch makes this a no-op.
        if context.receipt()?.is_some() || context.target()? != TargetState::Missing {
            return Ok(None);
        }
        let Ok(registration) = usable_external(&context) else {
            return Ok(None);
        };
        let spec = external_spec(&registration, checkout);
        if !selection_matches(&captured, &spec.selected(0)) {
            return Ok(None);
        }
        let receipt = Receipt::first(checkout, &context.checkout_id);
        let committed = transition(&context, &receipt, &spec, Start::MissingOnly)?;
        committed
            .selected()
            .cloned()
            .map(Some)
            .ok_or_else(|| "the transition committed no root".to_owned())
    })();
    release_barrier(store, guard)?;
    if let Some(selected) = outcome? {
        eprintln!(
            "oxide: auto-adopted {} -> {} for permanent-drive outputs; no existing outputs moved",
            context.link().display(),
            selected.root.display()
        );
    }
    Ok(())
}

/// `oxide outputs detach`: back to a real target directory, keeping roots.
/// It is a per-checkout opt-out in either drive mode (oxide#124): the
/// detached receipt stays, so automatic first conversion never follows it.
fn detach_command() -> Result<u8, String> {
    let context = Context::open()?;
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = output_store()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(&internal, &context.workspace, &mut waits)?;
    let outcome = (|| {
        refuse_unknown_custody(&internal, &context.workspace, "detach")?;
        // A pool adopted before the shared output format upgrades behind
        // the forward fence before its first receipt write (a no-op once
        // ready); a missing receipt changes nothing.
        if read_receipt(&control, &context.workspace, &context.checkout_id)?.is_some() {
            crate::prune::ensure_ready(&internal, &context.workspace)?;
        }
        // Detach selects nothing, so an unused reservation is cancelled
        // through its journal first, before the admission lock.
        if let Some(mut reserved) =
            read_receipt(&control, &context.workspace, &context.checkout_id)?
                .filter(|receipt| receipt.reserved().is_some())
        {
            let _binding = internal.lock_binding(&context.workspace)?;
            cancel_reservation(&internal, &control, &context, &mut reserved)?;
        }
        let _admission = store::publication_guard(&control)?;
        let receipt = read_receipt(&control, &context.workspace, &context.checkout_id)?
            .ok_or("this checkout has no owned-output receipt; nothing to detach")?;
        let mut receipt = replay(&context, receipt)?;
        if receipt.state == ReceiptState::Detached {
            return Ok((receipt, false));
        }
        let selected = receipt
            .selected()
            .cloned()
            .ok_or("the receipt selects no root; nothing to detach")?;
        let target = context.target()?;
        if target.link_root() != Some(selected.root.as_path()) {
            return Err(format!(
                "outputs detach refused: {} is {}, not the owned link selecting {}; it is left untouched",
                context.link().display(),
                target.label(),
                selected.root.display()
            ));
        }
        let link = context.link();
        fs::remove_file(&link)
            .map_err(|error| format!("cannot remove the owned link {}: {error}", link.display()))?;
        if let Err(error) = fs::create_dir(&link) {
            return Err(format!(
                "the owned link was removed, but creating the real {} failed: {error}; re-run `oxide outputs adopt` to restore the link",
                link.display()
            ));
        }
        fs::set_permissions(&link, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot set permissions on {}: {error}", link.display()))?;
        sync_directory(&context.workspace)?;
        if let Some(previous) = receipt.link_mut().selected.take() {
            remember_root(&mut receipt, previous);
        }
        receipt.state = ReceiptState::Detached;
        receipt.link_mut().prepared = None;
        receipt.updated_ms = process::now_ms();
        write_receipt(&control, &receipt)?;
        Ok((receipt, true))
    })();
    release_barrier(&internal, guard)?;
    let (receipt, detached) = outcome?;
    if detached {
        println!(
            "outputs: {} is a real empty directory again; no output root was moved or deleted",
            context.link().display()
        );
    } else {
        println!("outputs: this checkout is already detached from owned outputs");
    }
    for root in &receipt.roots {
        println!(
            "outputs: preserved {} root {} ({})",
            root.kind.label(),
            root.root.display(),
            if root.root.is_dir() {
                "present"
            } else {
                "absent"
            }
        );
    }
    Ok(0)
}

/// A command's bound output destination for the life of its start-up. The
/// lock is the checkout binding mutex, held from the committed link check (or
/// the transition) until Cargo has started, so a peer's transition cannot
/// repoint the link in between.
pub struct OutputBinding {
    pub outcome: Outcome,
    _lock: Option<File>,
}

impl OutputBinding {
    fn unchecked(outcome: Outcome) -> Self {
        Self {
            outcome,
            _lock: None,
        }
    }
}

/// How long a command waits before re-checking a checkout whose other command
/// must finish before an output relink.
const BINDING_POLL: Duration = Duration::from_millis(200);

/// A build binds its checkout's output root once, before Cargo starts. The
/// check and any transition run under the checkout's binding mutex, a plain
/// per-checkout lock that no holder can wait on: second-slot commands still
/// start concurrently when their destination is unchanged, and two commands
/// can never block each other through it. A relink waits (without the mutex)
/// until no other command in the checkout may be writing Cargo, and refuses
/// outright on unknown writer custody.
///
/// `expected` is the destination the slot key was computed from, including the
/// root generation. The command binds exactly that root and generation or
/// refuses, so a slot and its uplifted artifacts never belong to different
/// roots.
pub fn bind_for_command(
    store: &Store,
    checkout: &Path,
    own_record: u64,
    expected: Option<&Selected>,
    pool_fallback: Option<&str>,
) -> Result<OutputBinding, String> {
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        checkout.to_path_buf(),
    )
    .with_pool_fallback(pool_fallback);
    if let Some(intent) = crate::prune::unsettled_for(store.owner_control(), &context.checkout_id)?
    {
        return Err(format!(
            "outputs: prune intent {intent} is unsettled for this checkout; binding refuses until `oxide outputs resolve-intent {intent}` settles it or the exact intent completes"
        ));
    }
    let mode = context.mode()?;
    if mode != Some(DriveMode::Permanent) {
        if context.receipt()?.is_none() {
            keyed_state_matches(None, expected)?;
            return Ok(OutputBinding::unchecked(Outcome::default()));
        }
        // A converted checkout's command holds the binding mutex until it is
        // recorded as running, in either mode, so a permanent-mode relink
        // never repoints the link between this check and Cargo's start.
        let lock = store.lock_binding(checkout)?;
        let outcome = bind_ephemeral(&context, expected)?;
        return Ok(OutputBinding {
            outcome,
            _lock: Some(lock),
        });
    }
    if context.receipt()?.is_none() {
        keyed_state_matches(None, expected)?;
        refuse_unowned_link(&context, "outputs")?;
        return Ok(OutputBinding::unchecked(unconverted_outcome()));
    }
    let mut announced = false;
    loop {
        if let Some(signal) = process::received_signal() {
            return Err(format!(
                "interrupted by signal {signal} while waiting for this checkout's other commands; nothing was changed"
            ));
        }
        let lock = store.lock_binding(checkout)?;
        let Some(receipt) = context.receipt()? else {
            keyed_state_matches(None, expected)?;
            refuse_unowned_link(&context, "outputs")?;
            return Ok(OutputBinding {
                outcome: unconverted_outcome(),
                _lock: Some(lock),
            });
        };
        keyed_state_matches(Some(&receipt), expected)?;
        let receipt = match gated_replay(&context, store, own_record, receipt)? {
            Some(receipt) => receipt,
            None => {
                drop(lock);
                if !announced {
                    eprintln!(
                        "oxide: waiting for another oxide command in this checkout (a build, an exec tool or a program `oxide run` started) to finish before relinking outputs"
                    );
                    announced = true;
                }
                std::thread::sleep(BINDING_POLL);
                continue;
            }
        };
        match bind_under_binding(&context, store, own_record, &receipt, expected) {
            Bind::Bound(outcome) => {
                return Ok(OutputBinding {
                    outcome,
                    _lock: Some(lock),
                })
            }
            Bind::Refuse(reason) => return Err(reason),
            Bind::Wait => {
                drop(lock);
                if !announced {
                    eprintln!(
                        "oxide: waiting for another oxide command in this checkout (a build, an exec tool or a program `oxide run` started) to finish before relinking outputs"
                    );
                    announced = true;
                }
                std::thread::sleep(BINDING_POLL);
            }
        }
    }
}

/// Replay a prepared receipt under the binding mutex only after the same
/// cross-pool custody rule as a fresh transition: unknown writer custody
/// refuses with the link, sibling, receipt and roots unchanged, and a
/// command that may be writing must close first (`None`: wait). A committed
/// or detached receipt passes through untouched.
fn gated_replay(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: Receipt,
) -> Result<Option<Receipt>, String> {
    if receipt.state != ReceiptState::Prepared {
        return Ok(Some(receipt));
    }
    match store.checkout_custody(&context.workspace, own_record)? {
        CheckoutCustody::Unknown(reason) => Err(format!(
            "outputs: refusing to replay the interrupted transition for {}: {reason}. The link, the prepared receipt and both roots are kept until that custody is resolved",
            context.link().display()
        )),
        CheckoutCustody::Writers => Ok(None),
        CheckoutCustody::Clear => replay(context, receipt).map(Some),
    }
}

/// A slot is keyed with or without an owned destination when its command
/// starts. If the checkout was converted (or detached) while the command
/// waited for admission, the slot's key no longer describes where Cargo
/// would write: refuse before Cargo so the next attempt keys afresh.
fn keyed_state_matches(
    receipt: Option<&Receipt>,
    expected: Option<&Selected>,
) -> Result<(), String> {
    let converted = receipt.is_some_and(|receipt| receipt.state != ReceiptState::Detached);
    if converted == expected.is_some() {
        return Ok(());
    }
    Err(format!(
        "outputs: this checkout was {} while this command waited, so its slot was keyed for {}; nothing was built. Run the command again",
        if converted { "converted to owned outputs" } else { "detached from owned outputs" },
        if converted { "an unconverted target" } else { "an owned root" }
    ))
}

/// In permanent mode a checkout without a committed owned link keeps a real
/// `target`: a link there was never established as owned (a foreign link, or
/// a marked link whose receipt is missing), so Cargo is never handed it and
/// clean never follows it. Ephemeral mode keeps today's behaviour.
fn refuse_unowned_link(context: &Context, action: &str) -> Result<(), String> {
    let link = context.link();
    match fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "{action} refused: {} is a link that no committed owned-output receipt established; permanent drive mode never follows it. Nothing was changed. Replace it with a real directory, or remove it and run `oxide outputs adopt`",
            link.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{action} refused: cannot inspect {}: {error}", link.display())),
    }
}

fn unconverted_outcome() -> Outcome {
    Outcome {
        warnings: vec![
            "permanent drive mode: this checkout is not converted to owned outputs; `oxide outputs plan` shows the conversion plan and `oxide outputs adopt` performs it. Nothing was moved"
                .to_owned(),
        ],
        note: Some("outputs not converted".to_owned()),
        root: None,
    }
}

/// Ephemeral mode keeps today's real checkout/target. A checkout that was
/// converted keeps its owned link, is never relinked automatically, and
/// refuses when the link no longer matches the receipt or the root's full
/// identity.
fn bind_ephemeral(context: &Context, expected: Option<&Selected>) -> Result<Outcome, String> {
    let receipt = context.receipt()?;
    if receipt
        .as_ref()
        .is_none_or(|receipt| receipt.state != ReceiptState::Prepared)
    {
        keyed_state_matches(receipt.as_ref(), expected)?;
    }
    let Some(receipt) = receipt else {
        return Ok(Outcome::default());
    };
    match receipt.state {
        ReceiptState::Detached | ReceiptState::Pruned => Ok(Outcome::default()),
        ReceiptState::Pruning => Err("outputs: a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it".to_owned()),
        ReceiptState::Prepared => Err(
            "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            let wanted = match expected {
                Some(expected) => expected.clone(),
                None => desired_selection(context, &receipt)?
                    .ok_or("outputs: the receipt selects no root")?,
            };
            if wanted.kind == RootKind::External {
                if let Some(reason) = context.pool_fallback.as_deref() {
                    return Err(format!(
                        "outputs: {} selects the external root {}, but this command's intermediate slot fell back to the internal pool ({reason}) and the drive's removal would not be supervised; the drive mode is ephemeral, so Oxide will not relink it. Restore the drive or run `oxide outputs detach`",
                        context.link().display(),
                        wanted.root.display()
                    ));
                }
            }
            let target = context.target()?;
            if target.root() != Some(wanted.root.as_path()) {
                return Err(format!(
                    "outputs: {} is {}, not the owned link selecting {}; the drive mode is ephemeral, so Oxide will not relink it. Run `oxide outputs detach` for a real target directory, or set the drive mode to permanent and run `oxide outputs adopt`",
                    context.link().display(),
                    target.label(),
                    wanted.root.display()
                ));
            }
            check_bound_root(context, receipt.generation, &wanted)?;
            let outcome = Outcome {
                root: Some(wanted.root.clone()),
                ..Outcome::default()
            };
            Ok(outcome.with_note(format!(
                "owned outputs {} through an owned link",
                wanted.root.display()
            )))
        }
    }
}

/// The result of one binding attempt under the mutex.
enum Bind {
    Bound(Outcome),
    Refuse(String),
    /// Another command may be writing in this checkout; release the mutex,
    /// wait, and try again.
    Wait,
}

fn bind_under_binding(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: &Receipt,
    expected: Option<&Selected>,
) -> Bind {
    let generation = receipt.generation;
    let wanted = match receipt.state {
        ReceiptState::Pruning => return Bind::Refuse("outputs: a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it".to_owned()),
        ReceiptState::Detached | ReceiptState::Pruned => {
            return match expected {
                Some(_) => Bind::Refuse(
                    "outputs: the receipt says this checkout is detached but its slot was keyed for an owned root; run the command again"
                        .to_owned(),
                ),
                None => match refuse_unowned_link(context, "outputs") {
                    Err(reason) => Bind::Refuse(reason),
                    Ok(()) => Bind::Bound(Outcome {
                    warnings: vec![
                        "permanent drive mode: this checkout is detached from owned outputs; `oxide outputs adopt` converts it again. Nothing was moved"
                            .to_owned(),
                    ],
                    note: Some("outputs detached".to_owned()),
                    root: None,
                }),
                },
            }
        }
        ReceiptState::Prepared => {
            return Bind::Refuse(
                "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building"
                    .to_owned(),
            )
        }
        ReceiptState::Committed => match expected {
            Some(expected) => expected.clone(),
            None => match desired_selection(context, receipt) {
                Ok(Some(wanted)) => wanted,
                Ok(None) => {
                    return Bind::Refuse("outputs: the receipt selects no root".to_owned());
                }
                Err(reason) => return Bind::Refuse(reason),
            },
        },
    };
    let usable = usable_external(context);
    if wanted.kind == RootKind::External {
        if let Err(reason) = &usable {
            return Bind::Refuse(format!(
                "outputs: this command's slot is keyed for the external root {} but the registered volume is no longer usable ({reason}); nothing was changed. Run the command again to select the checkout-local root",
                wanted.root.display()
            ));
        }
    }
    let target = match context.target() {
        Ok(target) => target,
        Err(reason) => return Bind::Refuse(reason),
    };
    // The live link must agree with the committed selection: a link to
    // another owned root (an older selection restored by hand) is foreign,
    // even when that root's marker is valid.
    let committed_root = receipt.selected().map(|selected| selected.root.as_path());
    let link_selects_wanted = target.link_root() == Some(wanted.root.as_path());
    if link_selects_wanted && committed_root != Some(wanted.root.as_path()) {
        return Bind::Refuse(format!(
            "outputs: {} selects {}, but the committed receipt selects {}; Oxide treats the link as foreign and changes nothing. Run `oxide outputs status` and restore the link",
            context.link().display(),
            wanted.root.display(),
            committed_root.map_or_else(|| "no root".to_owned(), |root| root.display().to_string())
        ));
    }
    match check_root(context, generation, &wanted) {
        RootCheck::Valid if link_selects_wanted => {
            return Bind::Bound(bound_outcome(&usable, &wanted, false));
        }
        RootCheck::Valid => {}
        RootCheck::Missing => {
            // A future root binds only the exact surviving reservation its
            // slot was keyed with: a cancelled, replaced or absent one
            // refuses before Cargo, with no automatic retry.
            if let Err(reason) = reservation_matches(context, receipt, &wanted) {
                return Bind::Refuse(reason);
            }
        }
        RootCheck::Contradiction(reason) => {
            return Bind::Refuse(format!(
                "outputs: {reason}; nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
            ))
        }
    }
    // A transition is needed: only this checkout's own link may be replaced.
    let old_link = match (&target, receipt.current()) {
        (TargetState::Missing, _) => true,
        (_, Some(selected)) => target.link_root() == Some(selected.root.as_path()),
        _ => false,
    };
    if !old_link {
        return Bind::Refuse(format!(
            "outputs: {} is {}, not this checkout's owned link; Oxide will not replace it. Run `oxide outputs detach` or restore the link yourself",
            context.link().display(),
            target.label()
        ));
    }
    match store.checkout_custody(&context.workspace, own_record) {
        Err(reason) => Bind::Refuse(reason),
        Ok(CheckoutCustody::Unknown(reason)) => Bind::Refuse(format!(
            "outputs: refusing to relink {}: {reason}. The owned link and both roots are kept until that custody is resolved",
            context.link().display()
        )),
        Ok(CheckoutCustody::Writers) => Bind::Wait,
        Ok(CheckoutCustody::Clear) => {
            let committed = match transition(context, receipt, &spec_for(&wanted), Start::Any) {
                Ok(committed) => committed,
                Err(reason) => return Bind::Refuse(reason),
            };
            let bound = match committed.selected() {
                Some(bound) => bound.clone(),
                None => return Bind::Refuse("outputs: the transition committed no root".to_owned()),
            };
            if !bound.same_place(&wanted) {
                return Bind::Refuse(format!(
                    "outputs: the transition bound {} but this command's slot was keyed for {}; nothing is built into the wrong root. Run the command again",
                    bound.root.display(),
                    wanted.root.display()
                ));
            }
            // The root must carry exactly the generation this command's slot
            // was keyed with, whichever generation the transition committed.
            match check_root(context, committed.generation, &wanted) {
                RootCheck::Valid => Bind::Bound(bound_outcome(&usable, &bound, true)),
                RootCheck::Missing => Bind::Refuse(format!(
                    "outputs: the transition bound {} but its root is missing; run the command again",
                    bound.root.display()
                )),
                RootCheck::Contradiction(reason) => Bind::Refuse(format!(
                    "outputs: the transition bound {} but {reason}; run the command again",
                    bound.root.display()
                )),
            }
        }
    }
}

/// The keyed future root's reservation must survive exactly: the receipt's
/// reservation is for this root and generation with the keyed nonce, and the
/// registry row (or, after durable overflow, the receipt's overflow tag)
/// still backs it with no cancellation recorded.
fn reservation_matches(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> Result<(), String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    crate::prune::require_registry(control)?;
    let stale = || {
        format!(
            "outputs: this command's slot was keyed for root nonce {} of {}, but that reservation was cancelled or replaced; nothing was built. Run the command again",
            wanted.nonce().unwrap_or("(none)"),
            wanted.root.display()
        )
    };
    let Some(nonce) = wanted.nonce() else {
        return Err(stale());
    };
    let Some(reserved) = receipt
        .reserved()
        .filter(|reserved| reserved.is_for(wanted) && reserved.nonce == nonce)
    else {
        return Err(stale());
    };
    crate::prune::validate_reservation(control, reserved)
        .map_err(|reason| format!("{} ({reason})", stale()))
}

/// Whether a root can be used as the keyed destination right now.
#[derive(Debug)]
enum RootCheck {
    /// A real, fully owned root with the expected generation and device.
    Valid,
    /// The root does not exist yet, or is an empty directory with no marker; a
    /// transition may create it.
    Missing,
    /// Ownership, kind, volume, generation or device contradicts the receipt.
    Contradiction(String),
}

/// The complete check a reused-root command performs: marker identity, kind,
/// volume, generation and current device/ancestry. Any contradiction refuses
/// without mutation; only a wholly absent or empty unmarked root may be
/// created by a transition.
fn check_root(context: &Context, receipt_generation: u64, wanted: &Selected) -> RootCheck {
    let root = &wanted.root;
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RootCheck::Missing,
        Err(error) => {
            return RootCheck::Contradiction(format!("cannot inspect {}: {error}", root.display()))
        }
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return RootCheck::Contradiction(format!("{} is not a real directory", root.display()))
        }
        Ok(_) => {}
    }
    if fs::symlink_metadata(root.join(MARKER)).is_err() {
        let empty = fs::read_dir(root)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        return if empty {
            RootCheck::Missing
        } else {
            RootCheck::Contradiction(format!(
                "{} holds content and carries no Oxide owner marker",
                root.display()
            ))
        };
    }
    match check_bound_root(context, receipt_generation, wanted) {
        Ok(()) => RootCheck::Valid,
        Err(reason) => RootCheck::Contradiction(reason),
    }
}

/// Validate a selected real root in full: this checkout's marker, the root
/// kind and volume, the generation it was created at, its device and that it
/// is not a mount root. Refuses any contradiction.
fn check_bound_root(
    context: &Context,
    receipt_generation: u64,
    wanted: &Selected,
) -> Result<(), String> {
    let root = &wanted.root;
    let binding = match wanted.kind {
        RootKind::External => Some(binding_for(
            context,
            wanted.volume_uuid.as_deref().unwrap_or_default(),
        )?),
        RootKind::Local => None,
    };
    validate_selected(
        &context.workspace,
        &context.checkout_id,
        wanted,
        binding.as_ref(),
        receipt_generation,
    )?;
    let marker = read_marker(root)?;
    if wanted.nonce.is_some() && marker.nonce != wanted.nonce {
        return Err(format!(
            "{} carries another root nonce than the keyed one; run the command again",
            root.display()
        ));
    }
    if marker.generation != wanted.generation {
        return Err(format!(
            "{} was created at generation {}, not the keyed {}; run the command again",
            root.display(),
            marker.generation,
            wanted.generation
        ));
    }
    Ok(())
}

fn bound_outcome(
    usable: &Result<removable::Registration, String>,
    bound: &Selected,
    relinked: bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    if let Err(reason) = usable {
        if bound.kind == RootKind::Local {
            outcome.warnings.push(format!(
                "permanent drive mode: outputs use the checkout-local fallback root {} ({reason}){}",
                bound.root.display(),
                if relinked { "; the external root is kept" } else { "" }
            ));
        }
    }
    outcome.root = Some(bound.root.clone());
    outcome.with_note(format!(
        "{} outputs {}{}",
        if bound.kind == RootKind::Local {
            "fallback"
        } else {
            "owned"
        },
        bound.root.display(),
        if relinked { " (relinked)" } else { "" }
    ))
}

/// The root the next command must select, read-only and without any lock: the
/// external root when the registered volume is usable now, else the
/// checkout-local fallback. Its `generation` is the generation the root has or
/// will be created at, so the slot key and the binding agree. `None` when this
/// checkout has no owned output link (never converted, or detached).
pub fn planned_destination(
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<Option<Selected>, String> {
    let Some(control) = control else {
        return Ok(None);
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    let Some(receipt) = context.receipt()? else {
        return Ok(None);
    };
    desired_selection(&context, &receipt)
}

/// The destination rule itself: in permanent mode, external when the volume
/// is usable, else the local fallback; in any other mode, the committed
/// selection, which is never relinked automatically. Each comes with the
/// generation its root has or will be created at. `None` for a detached or
/// absent receipt.
fn desired_selection(context: &Context, receipt: &Receipt) -> Result<Option<Selected>, String> {
    if receipt.state == ReceiptState::Detached {
        return Ok(None);
    }
    let next = receipt.generation.saturating_add(1);
    let spec = if context.mode()? == Some(DriveMode::Permanent) {
        match usable_external(context) {
            Ok(registration) => external_spec(&registration, &context.workspace),
            Err(_) => local_spec(&context.workspace),
        }
    } else {
        match receipt.selected() {
            Some(selected) => spec_for(selected),
            None => return Ok(None),
        }
    };
    let marker = read_marker(spec.root()).ok();
    let generation = marker.as_ref().map_or(next, |marker| marker.generation);
    let wanted = spec.selected(generation);
    // An existing root keys its own marker nonce; a future root keys the
    // nonce reserved for exactly it, never a missing root's old nonce.
    let nonce = match marker {
        Some(marker) => marker.nonce,
        None => receipt
            .reserved()
            .filter(|reserved| reserved.is_for(&wanted))
            .map(|reserved| reserved.nonce.clone()),
    };
    Ok(Some(wanted.with_nonce(nonce)))
}

/// Reserve the nonce of this command's future root before its slot is keyed
/// (oxide#129). Runs after automatic adoption and before
/// `planned_destination`/`SlotKey::resolve_in`, under the checkout's binding
/// lock (held only here, never across the lease wait) and, through
/// `write_receipt`, the internal publication lock. An existing root, an
/// identical uncancelled reservation, a pool without a ready registry (whose
/// transition refuses later and names the upgrade) or a checkout with no
/// committed receipt reserves nothing.
pub fn reserve(
    store: &Store,
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<(), String> {
    let Some(control) = control else {
        return Ok(());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    if context.receipt()?.is_none() || crate::prune::require_registry(control).is_err() {
        return Ok(());
    }
    // A recreated checkout never reserves or binds while a prune intent for
    // it is unsettled.
    if let Some(intent) = crate::prune::unsettled_for(control, &context.checkout_id)? {
        return Err(format!(
            "outputs: prune intent {intent} is unsettled for this checkout; nothing is built until the exact intent completes or `oxide outputs resolve-intent {intent}` settles it"
        ));
    }
    let _binding = store.lock_binding(checkout)?;
    // Overflow publication needs at most one retry: the first attempt
    // durably records sticky incompleteness and the second tags the receipt.
    for _ in 0..2 {
        let Some(mut receipt) = context.receipt()? else {
            return Ok(());
        };
        // Finish any exact cancellation a crash interrupted after its
        // receipt committed.
        crate::prune::finish_dropped_cancels(
            control,
            &context.checkout_id,
            receipt.reserved().map(|reserved| reserved.id.as_str()),
        )?;
        if receipt.state != ReceiptState::Committed {
            return Ok(());
        }
        let Some(wanted) = desired_selection(&context, &receipt)? else {
            return Ok(());
        };
        if read_marker(&wanted.root).is_ok() {
            return Ok(());
        }
        if let Some(reserved) = receipt.reserved().cloned() {
            if reserved.is_for(&wanted)
                && crate::prune::validate_reservation(control, &reserved).is_ok()
            {
                return Ok(());
            }
            // A different or invalidated reservation is cancelled through
            // its journal before a new one is made.
            cancel_reservation(store, control, &context, &mut receipt)?;
        }
        let id = crate::prune::fresh_nonce()?;
        let nonce = crate::prune::fresh_nonce()?;
        let overflow = crate::prune::overflow_tag(control, &id, &nonce)?;
        receipt.link_mut().reserved = Some(Reservation {
            id,
            nonce,
            kind: wanted.kind,
            root: wanted.root.clone(),
            volume_uuid: wanted.volume_uuid.clone(),
            generation: wanted.generation,
            created_ms: process::now_ms(),
            overflow,
        });
        receipt.updated_ms = process::now_ms();
        match write_receipt(control, &receipt) {
            Ok(()) => return Ok(()),
            Err(reason) if reason.contains(crate::prune::OVERFLOW_RETRY) => continue,
            Err(reason) => return Err(reason),
        }
    }
    Err("outputs: the output registry overflowed twice while reserving a root nonce; nothing was built. Run the command again".to_owned())
}

/// Cancel this checkout's unused reservation (oxide#129). Only under the
/// binding lock with every writer positively clear, the reserved root absent
/// (no marker, no entry) and no prune intent for the checkout. The durable
/// cancellation record invalidates its authority first, then the receipt
/// commits without it, then the abandoned registry row is removed. A stale
/// starter holding the old nonce refuses at binding; the nonce is never
/// reused.
fn cancel_reservation(
    store: &Store,
    control: &Path,
    context: &Context,
    receipt: &mut Receipt,
) -> Result<(), String> {
    let Some(reserved) = receipt.reserved().cloned() else {
        return Ok(());
    };
    if crate::prune::unsettled_for(control, &context.checkout_id)?.is_some() {
        return Err("outputs: a prune intent is unsettled for this checkout, so its root reservation is kept; nothing was built".to_owned());
    }
    match fs::symlink_metadata(&reserved.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(format!(
                "outputs: the reserved root {} exists or cannot be inspected, so its reservation is kept; nothing was built",
                reserved.root.display()
            ))
        }
    }
    match store.checkout_custody(&context.workspace, 0)? {
        CheckoutCustody::Clear => {}
        _ => {
            return Err("outputs: this checkout's root reservation changed while another command may hold it; nothing was built. Run the command again once other commands in this checkout finish".to_owned())
        }
    }
    crate::prune::begin_cancel(control, &context.checkout_id, &reserved)?;
    receipt.link_mut().reserved = None;
    receipt.updated_ms = process::now_ms();
    write_receipt(control, receipt)?;
    crate::prune::finish_cancel(control, &context.checkout_id, &reserved)
}

/// `oxide outputs cancel-reservation`: the explicit cancellation of this
/// checkout's unused reservation, under its clean barrier and binding lock.
fn cancel_command() -> Result<u8, String> {
    let context = Context::open()?;
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = output_store()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(&internal, &context.workspace, &mut waits)?;
    let outcome = (|| -> Result<Option<Reservation>, String> {
        let _binding = internal.lock_binding(&context.workspace)?;
        let mut receipt = read_receipt(&control, &context.workspace, &context.checkout_id)?
            .ok_or("this checkout has no owned-output receipt; nothing to cancel")?;
        let Some(reserved) = receipt.reserved().cloned() else {
            return Ok(None);
        };
        cancel_reservation(&internal, &control, &context, &mut receipt)?;
        Ok(Some(reserved))
    })();
    release_barrier(&internal, guard)?;
    match outcome? {
        Some(reserved) => println!(
            "outputs: cancelled the unused reservation {} for {}; its nonce is never reused",
            reserved.id,
            reserved.root.display()
        ),
        None => println!("outputs: this checkout holds no root reservation"),
    }
    Ok(0)
}

/// The generation a destination root has, or will be created at: its marker's
/// creation generation, else the generation the next transition would use.
fn destination_generation(root: &Path, next: u64) -> u64 {
    read_marker(root).map_or(next, |marker| marker.generation)
}

/// The final-output destination, as a component of a slot's compatibility
/// identity: the selected real root's kind, path, bound volume and the
/// generation it has or will be created at. Empty for an ephemeral or
/// detached checkout, so those checkouts keep exactly the slot keys they had
/// before this feature.
pub fn destination_facts(selected: Option<&Selected>) -> String {
    let Some(selected) = selected else {
        return String::new();
    };
    format!(
        "outputs-2 {} {} {} {} g{}",
        selected.nonce().unwrap_or("-"),
        match selected.kind {
            RootKind::External => "external",
            RootKind::Local => "local",
        },
        selected.root.display(),
        selected.volume_uuid.as_deref().unwrap_or("-"),
        selected.generation
    )
}

/// Refuse an explicit output command while this checkout has unknown writer
/// custody: a lost slot whose writers were never positively closed may still
/// write through the output front door, so no link is replaced or restored.
fn refuse_unknown_custody(store: &Store, checkout: &Path, action: &str) -> Result<(), String> {
    match store.checkout_custody(checkout, 0)? {
        CheckoutCustody::Unknown(reason) => Err(format!(
            "outputs: refusing to {action} {}: {reason}. The owned link and both roots are kept until that custody is resolved",
            checkout.join("target").display()
        )),
        CheckoutCustody::Clear | CheckoutCustody::Writers => Ok(()),
    }
}

/// The internal pool paired with a registered removable pool, so the clean
/// barrier an output transition takes spans a checkout's builds in either
/// pool (the same pairing `oxide clean` uses).
fn output_store() -> Result<Option<Store>, String> {
    let settings = store::settings()?;
    removable::coordinate(Store::open_existing_with(&settings)?)
}

/// Acquire the checkout's clean barrier: no other command in this checkout
/// holds or takes a slot, and no clean runs, until the guard is released.
fn barrier(
    store: &Store,
    checkout: &Path,
    waits: &mut WaitAccounting,
) -> Result<CleanGuard, String> {
    match store.begin_clean(checkout, waits)? {
        Cleaning::Guard(guard) => Ok(guard),
        Cleaning::Interrupted(signal) => Err(format!(
            "interrupted by signal {signal} while waiting for this checkout's commands to finish; nothing was changed"
        )),
    }
}

/// End the barrier without dropping slot affinity: an output transition is
/// not a clean.
fn release_barrier(store: &Store, guard: CleanGuard) -> Result<(), String> {
    store.abandon_clean(guard)
}

/// The registered volume when the permanent drive is usable right now.
fn usable_external(context: &Context) -> Result<removable::Registration, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists, so no removable drive is registered")?;
    let registration =
        removable::read_registration(control)?.ok_or("no removable drive is registered")?;
    if registration.mode != DriveMode::Permanent {
        return Err("the drive is not in permanent mode".to_owned());
    }
    if let Some(reason) = context.pool_fallback.as_deref() {
        return Err(format!(
            "this command's intermediate slot fell back to the internal pool ({reason}), so the drive's removal is not supervised"
        ));
    }
    if !cfg!(target_os = "macos") {
        return Err("the removable backend is macOS-only".to_owned());
    }
    let binding = Binding::open(&registration.mount_point, Some(&registration.volume_uuid))?;
    if !volume::writable(binding.mount_point())? {
        return Err("the volume is not writable".to_owned());
    }
    let facts = binding.verify()?;
    let available = facts
        .available_bytes
        .ok_or("the volume's free space is unknown")?;
    if available <= registration.reserve_bytes {
        return Err(format!(
            "{} MiB available is not above the {} MiB reserve",
            available / crate::MIB,
            registration.reserve_bytes / crate::MIB
        ));
    }
    Ok(registration)
}

fn external_spec(registration: &removable::Registration, checkout: &Path) -> RootSpec {
    let label = identity::project_label(checkout);
    let id = identity::digest(&checkout.to_string_lossy());
    let leaf = format!("{label}-{}", &id[..8]);
    RootSpec::External {
        root: registration.mount_point.join(EXTERNAL_DIR).join(&leaf),
        uuid: registration.volume_uuid.clone(),
        leaf,
    }
}

fn local_spec(checkout: &Path) -> RootSpec {
    RootSpec::Local {
        root: checkout.join(LOCAL_DIR).join(LOCAL_ROOT),
    }
}

/// Classify `<checkout>/target` without following a link away from ownership.
fn target_state(checkout: &Path, checkout_id: &str) -> Result<TargetState, String> {
    let target = checkout.join("target");
    let metadata = match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TargetState::Missing)
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", target.display())),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        let root = fs::read_link(&target)
            .map_err(|error| format!("cannot read the link {}: {error}", target.display()))?;
        return Ok(match owned_root(checkout, checkout_id, &root) {
            Ok(()) => TargetState::OwnedLink(root),
            Err(_) if fs::symlink_metadata(&root).is_err() => TargetState::Dangling(root),
            Err(_) => TargetState::Foreign,
        });
    }
    if !metadata.is_dir() {
        return Ok(TargetState::Foreign);
    }
    let mut entries = fs::read_dir(&target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?;
    Ok(if entries.next().is_none() {
        TargetState::RealEmpty
    } else {
        TargetState::RealNonEmpty
    })
}

/// A root is owned by this checkout when it is a real directory carrying this
/// checkout's marker. Volume binding and generation are checked against the
/// receipt by [`validate_selected`].
fn owned_root(checkout: &Path, checkout_id: &str, root: &Path) -> Result<(), String> {
    if !safe_absolute(root) {
        return Err(format!("{} is not an absolute path", root.display()));
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("cannot inspect {}: {error}", root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", root.display()));
    }
    let marker = read_marker(root)?;
    check_marker(&marker, checkout, checkout_id)
}

fn check_marker(marker: &Marker, checkout: &Path, checkout_id: &str) -> Result<(), String> {
    let _ = marker;
    marker.validate().map_err(|reason| {
        format!(
            "{} carries an unusable owner marker ({reason})",
            checkout.display()
        )
    })?;
    if marker.checkout != checkout || marker.checkout_id != checkout_id {
        return Err(format!(
            "{} belongs to {}, not this checkout",
            marker.checkout.display(),
            checkout.display()
        ));
    }
    if marker.recipe != RECIPE {
        return Err(format!(
            "{} carries owner recipe {:?}, not {RECIPE}",
            checkout.display(),
            marker.recipe
        ));
    }
    Ok(())
}

/// Validate a selected real root against the receipt's identity: marker,
/// kind, volume UUID, device and generation.
fn validate_selected(
    checkout: &Path,
    checkout_id: &str,
    selected: &Selected,
    binding: Option<&Binding>,
    receipt_generation: u64,
) -> Result<(), String> {
    owned_root(checkout, checkout_id, &selected.root)?;
    let marker = read_marker(&selected.root)?;
    if marker.kind != selected.kind || marker.volume_uuid != selected.volume_uuid {
        return Err(format!(
            "{} carries a marker for another root kind or volume",
            selected.root.display()
        ));
    }
    if marker.generation > receipt_generation {
        return Err(format!(
            "{} carries generation {} ahead of the receipt's {receipt_generation}",
            selected.root.display(),
            marker.generation
        ));
    }
    if let (Some(nonce), Some(recorded)) = (marker.nonce(), selected.nonce()) {
        if recorded != nonce {
            return Err(format!(
                "{} carries root nonce {nonce}, not the receipt's {:?}",
                selected.root.display(),
                selected.nonce()
            ));
        }
    }
    validate_root_device(checkout, selected, binding)?;
    Ok(())
}

/// A root must be a real directory on the intended device, and never a mount
/// root of its own.
fn validate_root_device(
    checkout: &Path,
    selected: &Selected,
    binding: Option<&Binding>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(&selected.root)
        .map_err(|error| format!("cannot inspect {}: {error}", selected.root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a real directory",
            selected.root.display()
        ));
    }
    refuse_mount_root(&selected.root)?;
    match (selected.kind, binding) {
        (RootKind::Local, _) => {
            validate_local_ancestry(checkout, &selected.root)?;
            let checkout_metadata = fs::symlink_metadata(checkout)
                .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
            if metadata.dev() != checkout_metadata.dev() {
                return Err(format!(
                    "{} is on another device than the checkout",
                    selected.root.display()
                ));
            }
        }
        (RootKind::External, Some(binding)) => {
            let facts = binding.verify()?;
            let observed = volume::observe_path(&selected.root)?;
            if observed.dev != facts.dev || observed.mounted_on != facts.mounted_on {
                return Err(format!(
                    "{} is not on the registered volume {}",
                    selected.root.display(),
                    binding.mount_point().display()
                ));
            }
            if observed.uuid.as_deref() != selected.volume_uuid.as_deref() {
                return Err(format!(
                    "{} is on volume {:?}, not the recorded {:?}",
                    selected.root.display(),
                    observed.uuid,
                    selected.volume_uuid
                ));
            }
            let canonical = fs::canonicalize(&selected.root)
                .map_err(|error| format!("cannot resolve {}: {error}", selected.root.display()))?;
            if canonical != selected.root {
                return Err(format!(
                    "{} does not resolve to itself",
                    selected.root.display()
                ));
            }
        }
        (RootKind::External, None) => {
            return Err(format!(
                "{} is an external root and the registered volume is not usable now",
                selected.root.display()
            ))
        }
    }
    Ok(())
}

/// The local root is exactly `<checkout>/.oxide-outputs/local` with a real
/// `.oxide-outputs` directory and no link anywhere between the checkout and
/// the leaf: a same-device symlinked ancestor would place outputs (and a
/// clean's deletions) outside the checkout.
fn validate_local_ancestry(checkout: &Path, root: &Path) -> Result<(), String> {
    let outer = checkout.join(LOCAL_DIR);
    if root != outer.join(LOCAL_ROOT) {
        return Err(format!(
            "{} is not this checkout's local root {}",
            root.display(),
            outer.join(LOCAL_ROOT).display()
        ));
    }
    let metadata = fs::symlink_metadata(&outer)
        .map_err(|error| format!("cannot inspect {}: {error}", outer.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", outer.display()));
    }
    let canonical_checkout = fs::canonicalize(checkout)
        .map_err(|error| format!("cannot resolve {}: {error}", checkout.display()))?;
    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;
    if canonical != canonical_checkout.join(LOCAL_DIR).join(LOCAL_ROOT) {
        return Err(format!(
            "{} resolves to {}, outside this checkout",
            root.display(),
            canonical.display()
        ));
    }
    Ok(())
}

/// A root that is itself a mount point is never written through.
fn refuse_mount_root(root: &Path) -> Result<(), String> {
    let parent = root
        .parent()
        .ok_or_else(|| format!("{} has no parent", root.display()))?;
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("cannot inspect {}: {error}", root.display()))?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
    if root_metadata.dev() != parent_metadata.dev() {
        return Err(format!(
            "{} is a mount point; Oxide does not write outputs on a mount root",
            root.display()
        ));
    }
    Ok(())
}

/// Create the root and its marker when absent, or verify an existing owned
/// root. Nothing is deleted; a nonempty root without a marker is left alone.
fn ensure_root(context: &Context, spec: &RootSpec, generation: u64) -> Result<Marker, String> {
    ensure_root_with(context, spec, generation, None)
}

/// Establish or validate an output root. `nonce` is the immutable root nonce
/// already recorded in the write-ahead transition; a new root without one
/// gets a fresh random value, and an existing root keeps its marker's.
fn ensure_root_with(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
    nonce: Option<&str>,
) -> Result<Marker, String> {
    let checkout = &context.workspace;
    let root = spec.root();
    let binding = match spec {
        RootSpec::External { uuid, .. } => Some(binding_for(context, uuid)?),
        RootSpec::Local { .. } => None,
    };
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            let marker = match read_marker(root) {
                Ok(marker) => marker,
                Err(reason) => {
                    let empty = fs::read_dir(root)
                        .map_err(|error| format!("cannot read {}: {error}", root.display()))?
                        .next()
                        .is_none();
                    if !empty {
                        return Err(format!(
                            "outputs: {reason}; {} holds content and is left untouched",
                            root.display()
                        ));
                    }
                    // Ownership is about to be established here, whether the
                    // empty directory was just created or already existed.
                    if matches!(spec, RootSpec::Local { .. }) {
                        refuse_tracked_local_root(context)?;
                    }
                    create_marker(context, spec, generation, binding.as_ref(), nonce)?
                }
            };
            let selected = spec.selected(generation).with_nonce(marker.nonce.clone());
            validate_selected(
                checkout,
                &context.checkout_id,
                &selected,
                binding.as_ref(),
                generation,
            )
            .map_err(|reason| {
                format!(
                    "outputs: {reason}; {root} is left untouched",
                    root = root.display()
                )
            })?;
            Ok(marker)
        }
        Ok(_) => Err(format!(
            "outputs: {} exists and is not a real directory; it is never replaced",
            root.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_root(context, spec, &binding)?;
            create_marker(context, spec, generation, binding.as_ref(), nonce)
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", root.display())),
    }
}

fn binding_for(context: &Context, uuid: &str) -> Result<Binding, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists, so no removable drive is registered")?;
    let registration =
        removable::read_registration(control)?.ok_or("no removable drive is registered")?;
    if registration.volume_uuid != uuid {
        return Err(format!(
            "the registered volume is {}, not the recorded {uuid}",
            registration.volume_uuid
        ));
    }
    Binding::open(&registration.mount_point, Some(uuid))
}

fn create_root(
    context: &Context,
    spec: &RootSpec,
    binding: &Option<Binding>,
) -> Result<(), String> {
    match (spec, binding) {
        (RootSpec::External { leaf, .. }, Some(binding)) => {
            // Recheck identity, then create through the verified volume
            // descriptor: never through a re-resolved path, and never on
            // another device.
            binding.verify()?;
            let outer = binding
                .dir_at(binding.fd(), EXTERNAL_DIR, true)
                .map_err(|error| {
                    format!("cannot create the output directory on the volume: {error}")
                })?;
            binding
                .dir_at(outer.as_fd(), leaf, true)
                .map_err(|error| format!("cannot create the output root {leaf}: {error}"))?;
            Ok(())
        }
        (RootSpec::External { .. }, None) => Err(
            "the registered volume is not usable; refusing to create an external root".to_owned(),
        ),
        (RootSpec::Local { root }, _) => {
            refuse_tracked_local_root(context)?;
            create_owned_directory(&context.workspace.join(LOCAL_DIR))?;
            create_owned_directory(root)
        }
    }
}

/// Refuse to establish or write into a checkout-local output root that Git
/// would track. Checked whenever ownership is about to be established, so an
/// empty `.oxide-outputs/local` that already exists (and is not ignored) is
/// refused and left unchanged.
fn refuse_tracked_local_root(context: &Context) -> Result<(), String> {
    let checkout = &context.workspace;
    // Ask about a path *inside* the fallback root: Git only matches a
    // trailing-slash directory pattern when the path is a directory, which
    // `.oxide-outputs` may not be yet.
    let probe = format!("{LOCAL_DIR}/{LOCAL_ROOT}");
    if git_would_track(checkout, &probe)? {
        return Err(format!(
            "outputs: refusing to create the checkout-local root {} because Git would track it; add `{GITIGNORE_LINE}` to {} (oxide init does this) and re-run",
            checkout.join(LOCAL_DIR).join(LOCAL_ROOT).display(),
            checkout.join(".gitignore").display()
        ));
    }
    Ok(())
}

/// Create one owner-only real directory, refusing a symlink or another kind.
fn create_owned_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a real directory; it is never replaced",
                path.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))?;
    sync_directory(path.parent().unwrap_or(path))
}

fn create_marker(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
    binding: Option<&Binding>,
    nonce: Option<&str>,
) -> Result<Marker, String> {
    let root = spec.root();
    let selected = spec.selected(generation).with_nonce(match nonce {
        Some(nonce) => Some(nonce.to_owned()),
        None => Some(crate::prune::fresh_nonce()?),
    });
    // The root must be on the intended device before the marker is written.
    validate_root_device(&context.workspace, &selected, binding)?;
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: context.workspace.clone(),
        checkout_id: context.checkout_id.clone(),
        label: identity::project_label(&context.workspace),
        recipe: RECIPE.to_owned(),
        kind: selected.kind,
        volume_uuid: selected.volume_uuid.clone(),
        nonce: selected.nonce.clone(),
        checkout_volume_uuid: crate::prune::checkout_volume_uuid(&context.workspace),
        generation,
        created_ms: process::now_ms(),
    };
    let path = root.join(MARKER);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            return Err(format!(
                "{} already exists; it is never replaced",
                path.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|error| format!("cannot encode the output root marker: {error}"))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err("the output root marker exceeds its byte limit".to_owned());
    }
    let temp = root.join(format!(".{MARKER}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    write_private(&temp, &bytes)?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output root marker: {error}")
    })?;
    sync_directory(root)?;
    Ok(marker)
}

/// What a transition may do with `<checkout>/target` when it starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Start {
    /// `adopt`, relink and restore: a missing target, an empty real directory
    /// (explicit conversion only) or the receipt's own link.
    Any,
    /// The automatic first conversion of oxide#121: a positively absent target
    /// only. An empty directory, a link and anything else refuse.
    MissingOnly,
}

/// Write the committed receipt for one transition.
fn transition(
    context: &Context,
    receipt: &Receipt,
    spec: &RootSpec,
    start: Start,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    crate::prune::require_registry(control)?;
    let link = context.link();
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    // Only the receipt's own owned link, a missing target, or (for the
    // explicit conversion) an empty real directory may be replaced.
    let target = context.target()?;
    // A first conversion publishes an owned link, so Git must ignore that
    // exact non-directory path first — before the prepared receipt, the root
    // or either link exists. Later generations keep their committed link's
    // eligibility and are not re-guarded.
    let first_link = receipt.current().is_none()
        && (target == TargetState::Missing
            || (target == TargetState::RealEmpty && start == Start::Any));
    if first_link {
        refuse_link_git_exposure(context)?;
    }
    // The publish below replaces nothing when the link was absent (or an empty
    // directory was just removed): a creator that appears in between is kept.
    let exclusive = matches!(target, TargetState::Missing | TargetState::RealEmpty);
    match (&target, receipt.selected(), start) {
        (TargetState::Missing, _, _) => {}
        (TargetState::RealEmpty, _, Start::Any) => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty target directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (_, Some(selected), Start::Any)
            if target.link_root() == Some(selected.root.as_path()) => {}
        (_, _, Start::MissingOnly) => {
            return Err(format!(
                "outputs: auto-adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed",
                link.display(),
                target.label()
            ))
        }
        _ => return Err(refuse_existing_target(context, &target)),
    }
    // A newly created root gets a fresh random nonce; an existing root keeps
    // the nonce in its marker. A missing root is never re-created with the
    // receipt's old nonce (oxide#129).
    let existing_nonce = read_marker(spec.root())
        .ok()
        .and_then(|marker| marker.nonce);
    // A future root takes the nonce reserved for exactly it, after the
    // reservation is validated against the registry (or its overflow tag);
    // only an explicit adoption without a keyed slot draws a fresh one.
    let reserved = receipt
        .reserved()
        .filter(|reserved| reserved.is_for(&spec.selected(generation)))
        .cloned();
    let nonce = match existing_nonce {
        Some(nonce) => Some(nonce),
        None if read_marker(spec.root()).is_ok() => None,
        None => match &reserved {
            Some(reserved) => {
                crate::prune::validate_reservation(control, reserved)?;
                Some(reserved.nonce.clone())
            }
            None => Some(crate::prune::fresh_nonce()?),
        },
    };
    let to = spec.selected(generation).with_nonce(nonce);
    // Write-ahead: prepared first, then the destination root, then the
    // sibling link, then the rename, then the committed receipt.
    let mut writing = receipt.clone();
    writing.state = ReceiptState::Prepared;
    writing.link_mut().prepared = Some(Prepared {
        generation,
        to: to.clone(),
        link: link.clone(),
    });
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    crash_point("prepared")?;

    ensure_root_with(context, spec, generation, to.nonce())?;
    let sibling = sibling_link(&link, generation);
    match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(&to.root, &sibling).map_err(|error| {
                format!(
                    "cannot create the transition link {}: {error}",
                    sibling.display()
                )
            })?;
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing = fs::read_link(&sibling)
                .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
            if existing != to.root {
                return Err(format!(
                    "outputs: {} exists and selects {}, not {}; both are left as they are",
                    sibling.display(),
                    existing.display(),
                    to.root.display()
                ));
            }
        }
        Ok(_) => {
            return Err(format!(
                "outputs: {} is occupied by something Oxide does not own; it is not replaced",
                sibling.display()
            ))
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", sibling.display())),
    }
    let sibling_read = fs::read_link(&sibling)
        .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
    if sibling_read != to.root {
        return Err(format!(
            "outputs: {} does not read back as the transition root",
            sibling.display()
        ));
    }
    sync_directory(&context.workspace)?;
    crash_point("linked")?;

    publish_link(&sibling, &link, exclusive).map_err(|error| {
        format!(
            "cannot publish the owned link {}: {error}; the previous link is unchanged{}",
            link.display(),
            if exclusive {
                "; nothing that appeared at the target was replaced"
            } else {
                ""
            }
        )
    })?;
    sync_directory(&context.workspace)?;
    crash_point("renamed")?;
    let prepared = writing
        .prepared_ref()
        .clone()
        .ok_or("the prepared receipt lost its transition")?;
    finish_commit(context, receipt, &prepared)
}

/// Commit a prepared transition whose new link is live.
fn finish_commit(
    context: &Context,
    receipt: &Receipt,
    prepared: &Prepared,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    let link = context.link();
    let live =
        fs::read_link(&link).map_err(|error| format!("cannot read {}: {error}", link.display()))?;
    if live != prepared.to.root {
        return Err(format!(
            "outputs: {} selects {}, not the prepared {}; both roots are kept",
            link.display(),
            live.display(),
            prepared.to.root.display()
        ));
    }
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    let marker = ensure_root_with(
        context,
        &spec_for(&prepared.to),
        generation,
        prepared.to.nonce(),
    )?;
    let selected = prepared
        .to
        .at(prepared.generation)
        .with_nonce(marker.nonce.clone());
    let mut committed = receipt.clone();
    committed.state = ReceiptState::Committed;
    committed.generation = prepared.generation;
    committed.link_mut().selected = Some(selected.clone());
    committed.link_mut().prepared = None;
    // The commit that creates the reserved root consumes its reservation.
    if committed.reserved().is_some_and(|reserved| {
        reserved.root == selected.root && Some(reserved.nonce.as_str()) == selected.nonce()
    }) {
        committed.link_mut().reserved = None;
    }
    remember_root(&mut committed, selected);
    committed.updated_ms = process::now_ms();
    write_receipt(control, &committed)?;
    Ok(committed)
}

/// The spec that verifies an already committed selection.
fn spec_for(selected: &Selected) -> RootSpec {
    match selected.kind {
        RootKind::External => RootSpec::External {
            leaf: selected
                .root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            root: selected.root.clone(),
            uuid: selected.volume_uuid.clone().unwrap_or_default(),
        },
        RootKind::Local => RootSpec::Local {
            root: selected.root.clone(),
        },
    }
}

fn remember_root(receipt: &mut Receipt, root: Selected) {
    receipt.roots.retain(|known| known.root != root.root);
    receipt.roots.push(root);
    while receipt.roots.len() > MAX_ROOTS {
        receipt.roots.remove(0);
    }
}

fn sibling_link(link: &Path, generation: u64) -> PathBuf {
    let name = link
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_owned());
    link.with_file_name(format!(".{name}.oxide-new-{generation}"))
}

/// Publish a staged link at `link`. An `exclusive` publish is for a link that
/// was positively absent when classified: it fails rather than replacing
/// anything that has appeared since, and has no replacing fallback.
fn publish_link(sibling: &Path, link: &Path, exclusive: bool) -> std::io::Result<()> {
    if exclusive {
        rename_no_replace(sibling, link)
    } else {
        fs::rename(sibling, link)
    }
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from = path_c_string(from)?;
    let to = path_c_string(to)?;
    // SAFETY: both pointers come from live NUL-terminated strings that outlive
    // the call, and RENAME_EXCL never replaces an existing entry.
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_no_replace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    // The removable backend is macOS-only, so no automatic conversion reaches
    // here elsewhere; an explicit refusal, never a replacing rename.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a no-replace rename is unavailable on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn path_c_string(path: &Path) -> std::io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
}

/// Debug-only crash injection: stop at a write-ahead point without further
/// state, exactly as a killed process would leave the fixture.
fn crash_point(point: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if std::env::var(CRASH_ENVIRONMENT).is_ok_and(|value| value == point) {
        return Err(format!(
            "outputs: injected transition stop at {point} ({CRASH_ENVIRONMENT}={point}); the receipt stays prepared"
        ));
    }
    let _ = point;
    Ok(())
}

/// Replay a prepared receipt: the exact new/old generations only.
fn replay(context: &Context, receipt: Receipt) -> Result<Receipt, String> {
    if receipt.state == ReceiptState::Pruning {
        return Err("outputs: a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it".to_owned());
    }
    if receipt.state != ReceiptState::Prepared {
        return Ok(receipt);
    }
    let prepared = receipt
        .prepared_ref()
        .clone()
        .ok_or("the prepared receipt names no transition")?;
    // A first conversion publishes its link during replay too: the same
    // prerequisite is rechecked under the caller's binding mutex, with no
    // recovery bypass. A receipt that already committed a root keeps it.
    if receipt.current().is_none() {
        refuse_link_git_exposure(context)?;
    }
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    // Every replay write records into the registry: refuse before the first
    // effect when it is not ready.
    crate::prune::require_registry(control)?;
    // The receipt's link is validated to be this checkout's own target; the
    // rename destination is derived from the checkout, never from the file.
    let link = &context.link();
    if prepared.link != *link {
        return Err(stop_unknown(
            context,
            &prepared,
            "its link is not this checkout's target",
        ));
    }
    let target = context.target()?;
    let sibling = sibling_link(link, prepared.generation);
    // The sibling is classified before any replay mutation: positively
    // absent, or exactly the prepared link. Anything else (another kind, a
    // link to another root, unreadable) is unknown transition state and
    // stops with the receipt, links and roots unchanged.
    let sibling_live = match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::read_link(&sibling) {
            Ok(read) if read == prepared.to.root => true,
            Ok(read) => {
                return Err(stop_unknown(
                    context,
                    &prepared,
                    &format!(
                        "{} selects {}, not the prepared root {}",
                        sibling.display(),
                        read.display(),
                        prepared.to.root.display()
                    ),
                ))
            }
            Err(error) => {
                return Err(stop_unknown(
                    context,
                    &prepared,
                    &format!("cannot read {}: {error}", sibling.display()),
                ))
            }
        },
        Ok(_) => {
            return Err(stop_unknown(
                context,
                &prepared,
                &format!("{} exists and is not a link", sibling.display()),
            ))
        }
        Err(error) => {
            return Err(stop_unknown(
                context,
                &prepared,
                &format!("cannot inspect {}: {error}", sibling.display()),
            ))
        }
    };
    let owned_new = owned_root(&context.workspace, &context.checkout_id, &prepared.to.root).is_ok();
    let renamed = target.link_root() == Some(prepared.to.root.as_path());
    // After the rename the sibling no longer exists: a live sibling beside an
    // already-renamed target is contradictory.
    if renamed && sibling_live {
        return Err(stop_unknown(
            context,
            &prepared,
            "the target already selects the prepared root but the prepared sibling still exists",
        ));
    }
    // A first conversion has no committed selection, so its only valid old
    // state is a missing target: a link that appeared after the crash (even a
    // dangling one) is not this receipt's and is never renamed over. A
    // prepared receipt keeps the selection it is replacing in `selected`,
    // which is why the stored field is read here rather than the accessor.
    let old_live = match (&target, receipt.current()) {
        (TargetState::Missing, None) => true,
        (TargetState::Missing, Some(_)) => true,
        (_, Some(selected)) if target.link_root() == Some(selected.root.as_path()) => true,
        _ => false,
    };
    // An owned new root is validated in full (marker kind, volume UUID,
    // generation, live registered device and ancestry) before anything is
    // committed or renamed: a contradiction keeps the old link, the sibling,
    // both roots and the prepared receipt exactly as they are.
    let _binding = if owned_new && (renamed || (sibling_live && old_live)) {
        Some(
            validate_prepared(context, &prepared)
                .map_err(|reason| stop_unknown(context, &prepared, &reason))?,
        )
    } else {
        None
    };
    // 1. The rename already happened: commit.
    if owned_new && renamed {
        return finish_commit(context, &receipt, &prepared);
    }
    // 2. The sibling link is live: retry the rename under the barrier.
    if owned_new && sibling_live && old_live {
        // A link that was absent when classified is published without
        // replacement: a creator that appeared since the crash is kept.
        let exclusive = matches!(target, TargetState::Missing);
        publish_link(&sibling, link, exclusive).map_err(|error| {
            format!(
                "cannot replay the prepared link rename {}: {error}; both roots are kept",
                link.display()
            )
        })?;
        sync_directory(&context.workspace)?;
        let live = fs::read_link(link)
            .map_err(|error| format!("cannot read {}: {error}", link.display()))?;
        if live != prepared.to.root {
            return Err(stop_unknown(
                context,
                &prepared,
                "the replayed link does not read back as the prepared root",
            ));
        }
        return finish_commit(context, &receipt, &prepared);
    }
    // 3. Nothing was published: roll the preparation back, keeping the old
    //    selection and any root the attempt created. Only a positively absent
    //    sibling means nothing was published.
    if sibling_live {
        return Err(stop_unknown(
            context,
            &prepared,
            "the prepared sibling exists but neither its rename nor a rollback is safe",
        ));
    }
    if !old_live {
        return Err(stop_unknown(
            context,
            &prepared,
            "neither the prepared sibling nor the receipt's old target link is live",
        ));
    }
    let mut rolled = receipt.clone();
    rolled.state = if rolled.current().is_some() {
        ReceiptState::Committed
    } else {
        ReceiptState::Detached
    };
    rolled.link_mut().prepared = None;
    if prepared.to.root.is_dir() {
        remember_root(&mut rolled, prepared.to.clone());
    }
    rolled.updated_ms = process::now_ms();
    write_receipt(control, &rolled)?;
    Ok(rolled)
}

/// The complete check of a prepared destination root, held (with its volume
/// binding) across the replayed rename and commit.
fn validate_prepared(context: &Context, prepared: &Prepared) -> Result<Option<Binding>, String> {
    let to = &prepared.to;
    let binding = match to.kind {
        RootKind::External => Some(binding_for(
            context,
            to.volume_uuid.as_deref().unwrap_or_default(),
        )?),
        RootKind::Local => None,
    };
    validate_selected(
        &context.workspace,
        &context.checkout_id,
        to,
        binding.as_ref(),
        prepared.generation,
    )?;
    Ok(binding)
}

fn stop_unknown(context: &Context, prepared: &Prepared, reason: &str) -> String {
    format!(
        "outputs: the prepared transition to generation {} cannot be replayed: {reason}. Both roots and {} are kept exactly as they are; inspect `oxide outputs status` and the file system before acting",
        prepared.generation,
        context.link().display()
    )
}

fn read_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    let path = receipt_path(control, checkout_id);
    let Some(file) = store::private_file(&path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(format!(
            "{} exceeds its {MAX_RECEIPT_BYTES}-byte limit",
            path.display()
        ));
    }
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    receipt.validate(checkout, checkout_id)?;
    Ok(Some(receipt))
}

fn receipt_path(control: &Path, checkout_id: &str) -> PathBuf {
    control.join("outputs").join(format!("{checkout_id}.json"))
}

fn write_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    receipt.validate(&receipt.checkout, &receipt.checkout_id)?;
    // One critical section for the registry and the receipt. The registry's
    // revision advances before the changed receipt becomes visible.
    let _guard = store::publication_guard(control)?;
    crate::prune::note_receipt(control, receipt)?;
    let directory = control.join("outputs");
    create_owned_directory(&directory)?;
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("cannot encode the output receipt: {error}"))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err("the output receipt exceeds its byte limit".to_owned());
    }
    let path = receipt_path(control, &receipt.checkout_id);
    // Only the admission-lock holder writes, so one temp name suffices.
    let temp = directory.join(format!(
        ".{}.tmp-{}",
        receipt.checkout_id,
        std::process::id()
    ));
    write_private(&temp, &bytes)?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output receipt: {error}")
    })?;
    sync_directory(&directory)
}

pub(crate) fn receipt_path_public(control: &Path, checkout_id: &str) -> PathBuf {
    receipt_path(control, checkout_id)
}

/// Read one checkout's receipt for the prune snapshot. Never creates or
/// repairs anything.
pub(crate) fn read_receipt_for_prune(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt(control, checkout, checkout_id)
}

/// The SHA-256 of the exact marker bytes, for the recorded root identity.
pub(crate) fn marker_digest(root: &Path) -> Result<String, String> {
    let path = root.join(MARKER);
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

/// Record the terminal prune outcome in the checkout's receipt. The registry
/// terminal state is already durable when this runs.
pub(crate) fn mark_pruned(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
    outcome: &str,
) -> Result<(), String> {
    let mut receipt = read_receipt(control, checkout, checkout_id)?
        .ok_or_else(|| "the checkout's receipt disappeared before its prune receipt".to_owned())?;
    if receipt.state == ReceiptState::Pruned {
        if receipt
            .pruned
            .as_ref()
            .is_some_and(|record| record.intent == intent)
        {
            return Ok(());
        }
        return Err("the receipt records a different prune intent".to_owned());
    }
    if receipt.state == ReceiptState::Pruning
        && receipt
            .pruned
            .as_ref()
            .is_none_or(|record| record.intent != intent)
    {
        return Err("the pruning receipt records a different prune intent".to_owned());
    }
    receipt.state = ReceiptState::Pruned;
    receipt.link_mut().selected = None;
    receipt.link_mut().prepared = None;
    receipt.pruned = Some(PruneRecord {
        intent: intent.to_owned(),
        outcome: outcome.to_owned(),
        at_ms: process::now_ms(),
    });
    receipt.updated_ms = process::now_ms();
    write_receipt(control, &receipt)
}

/// Exclusively create the `pruning` receipt that protects an operator-listed
/// receiptless legacy root before its prune intent is journaled (oxide#129).
/// Under the internal publication lock: no receipt may exist, except this
/// writer's own interrupted creation (a `pruning` receipt whose intent no
/// registry journal records), which is rebound to the new intent. Any other
/// existing receipt refuses. No adoption history is reconstructed.
pub(crate) fn create_pruning_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
    journaled: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let _guard = store::publication_guard(control)?;
    let mut receipt = match read_receipt(control, checkout, checkout_id)? {
        None => Receipt::first(checkout, checkout_id),
        Some(existing)
            if existing.state == ReceiptState::Pruning
                && existing
                    .pruned
                    .as_ref()
                    .is_some_and(|record| !journaled(&record.intent)) =>
        {
            existing
        }
        Some(_) => return Err(
            "the checkout already has an owned-output receipt; a receiptless legacy root is kept"
                .to_owned(),
        ),
    };
    receipt.state = ReceiptState::Pruning;
    receipt.link_mut().selected = None;
    receipt.link_mut().prepared = None;
    receipt.link_mut().reserved = None;
    receipt.pruned = Some(PruneRecord {
        intent: intent.to_owned(),
        outcome: "pruning".to_owned(),
        at_ms: process::now_ms(),
    });
    receipt.updated_ms = process::now_ms();
    write_receipt(control, &receipt)
}

/// Remove this writer's own `pruning` receipt for exactly `intent`, when its
/// journal was never published or was rolled back with no root effect.
pub(crate) fn discard_pruning_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
) -> Result<bool, String> {
    let _guard = store::publication_guard(control)?;
    let Some(receipt) = read_receipt(control, checkout, checkout_id)? else {
        return Ok(false);
    };
    if receipt.state != ReceiptState::Pruning
        || receipt
            .pruned
            .as_ref()
            .is_none_or(|record| record.intent != intent)
    {
        return Ok(false);
    }
    let path = receipt_path(control, checkout_id);
    fs::remove_file(&path).map_err(|error| {
        format!(
            "cannot remove the pruning receipt {}: {error}",
            path.display()
        )
    })?;
    sync_directory(&control.join("outputs"))?;
    Ok(true)
}

/// A marker read for prune discovery of a listed receiptless root.
pub(crate) fn read_marker_for_prune(root: &Path) -> Result<Marker, String> {
    read_marker(root)
}

pub(crate) fn receipt_is_pruning(receipt: &Receipt) -> bool {
    receipt.state == ReceiptState::Pruning
}

fn read_marker(root: &Path) -> Result<Marker, String> {
    let path = root.join(MARKER);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("{} has no readable owner marker ({error})", root.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not an owner marker file", path.display()));
    }
    if metadata.len() > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let marker: Marker = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    marker
        .validate()
        .map_err(|reason| format!("{} is invalid ({reason})", path.display()))?;
    Ok(marker)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    };
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(path);
            format!("cannot write {}: {error}", path.display())
        })
}

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

/// The first conversion's ignore prerequisite (oxide#125).
///
/// Before any receipt, root or link effect, positively establish this
/// checkout's real Git work tree and the exact repository-relative
/// non-directory link path, then ask Git itself about that path. The index
/// must not already track it, and Git's winning ignore rule for it must be
/// neither a negation nor a trailing-slash (directory-only) rule: an
/// existing real empty target lets Git answer for a directory, so the losing
/// answer would wrongly cover a link. Trailing-slash and unsupported rule
/// forms are refused even when that is a false negative. Missing Git, a
/// non-repository, a timeout, a command error and malformed, oversized or
/// ambiguous output all refuse: unknown is never permission.
fn refuse_link_git_exposure(context: &Context) -> Result<(), String> {
    let link = context.link();
    let (root, relative) =
        git_link_path(context).map_err(|reason| link_git_unknown_refusal(&link, &reason, None))?;
    let gitignore = root.join(".gitignore");
    let fix = format!(
        "Ignore the non-directory path: add `/{relative}` (or `{relative}`), not `{relative}/`, to {}",
        gitignore.display()
    );
    // A tracked path would publish a tracked symlink once the link is there.
    let tracked = git_probe(&root, &["ls-files", "-z", "--", &relative], &[], true)
        .map_err(|reason| link_git_unknown_refusal(&link, &reason, Some(&fix)))?;
    if tracked.code != Some(0) {
        return Err(link_git_unknown_refusal(
            &link,
            &format!("git ls-files did not answer ({})", exit_label(tracked.code)),
            Some(&fix),
        ));
    }
    if !tracked.stdout.is_empty() {
        return Err(format!(
            "outputs: refusing to convert {} to an owned link: Git already tracks `{relative}`, so the published link would be a tracked path. Remove it from the index (`git rm --cached {relative}`) and {fix}; nothing was changed",
            link.display()
        ));
    }
    // The exact non-directory path, decided by Git itself: `--no-index`
    // reads the rules rather than the index (tracked is refused above), and
    // `-v` names the winning rule, whose form is part of the answer.
    let input = format!("{relative}\0");
    let check = git_probe(
        &root,
        &["check-ignore", "--no-index", "-v", "-z", "--stdin"],
        input.as_bytes(),
        false,
    )
    .map_err(|reason| link_git_unknown_refusal(&link, &reason, Some(&fix)))?;
    let rule = match check.code {
        Some(0) => parse_ignore_rule(&check.stdout, &relative)
            .map_err(|reason| link_git_unknown_refusal(&link, &reason, Some(&fix)))?,
        Some(1) => {
            return Err(format!(
                "outputs: refusing to convert {} to an owned link: Git does not ignore `{relative}`, so the published link would appear as an untracked path. {fix}; nothing was changed",
                link.display()
            ))
        }
        code => {
            return Err(link_git_unknown_refusal(
                &link,
                &format!("git check-ignore did not answer ({})", exit_label(code)),
                Some(&fix),
            ))
        }
    };
    if rule.pattern.starts_with('!') {
        return Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) re-includes `{relative}`. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        ));
    }
    if rule.pattern.ends_with('/') || rule.pattern.ends_with("\\/") {
        return Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) matches directories only, so it would not cover the published link. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        ));
    }
    Ok(())
}

fn link_git_unknown_refusal(link: &Path, reason: &str, fix: Option<&str>) -> String {
    match fix {
        Some(fix) => format!(
            "outputs: refusing to convert {} to an owned link: Git could not confirm its decision for the exact non-directory path ({reason}), which the first conversion needs. {fix}; nothing was changed",
            link.display()
        ),
        None => format!(
            "outputs: refusing to convert {} to an owned link: Git could not confirm this checkout's work tree and the exact non-directory path to ignore ({reason}), which the first conversion needs. Nothing was changed",
            link.display()
        ),
    }
}

fn exit_label(code: Option<i32>) -> String {
    code.map_or_else(|| "by signal".to_owned(), |code| format!("exit {code}"))
}

/// The real work tree and the exact repository-relative path of this
/// checkout's owned link: positively inside that work tree, no `..`, no
/// escape and no trailing slash.
fn git_link_path(context: &Context) -> Result<(PathBuf, String), String> {
    let workspace = fs::canonicalize(&context.workspace)
        .map_err(|error| format!("cannot resolve {}: {error}", context.workspace.display()))?;
    let probe = git_probe(&workspace, &["rev-parse", "--show-toplevel"], &[], false)?;
    if probe.code != Some(0) {
        return Err(format!(
            "git rev-parse did not confirm a work tree ({})",
            exit_label(probe.code)
        ));
    }
    let text = std::str::from_utf8(&probe.stdout)
        .map_err(|_| "Git reported a work tree root that is not UTF-8".to_owned())?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() || text.contains('\n') {
        return Err("Git reported an empty or multiple work tree roots".to_owned());
    }
    let root = fs::canonicalize(text)
        .map_err(|error| format!("cannot resolve the Git work tree root {text}: {error}"))?;
    let relative = workspace
        .strip_prefix(&root)
        .map_err(|_| "the checkout is not inside the Git work tree Git reported".to_owned())?;
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("the checkout path is not a plain relative path".to_owned());
        };
        let name = name
            .to_str()
            .ok_or("the checkout path is not valid UTF-8")?;
        if name.is_empty() || name.contains('/') || name.contains('\0') {
            return Err("the checkout path has an unusable component".to_owned());
        }
        parts.push(name.to_owned());
    }
    parts.push("target".to_owned());
    Ok((root, parts.join("/")))
}

/// One winning ignore rule from `git check-ignore --no-index -v -z --stdin`:
/// `<source>\0<line>\0<pattern>\0<pathname>\0`.
struct IgnoreRule {
    source: String,
    line: u64,
    pattern: String,
}

fn parse_ignore_rule(output: &[u8], expected: &str) -> Result<IgnoreRule, String> {
    let fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    // Exactly one complete record, ending with its NUL separator.
    if fields.len() != 5 || !fields[4].is_empty() {
        return Err("Git's verbose ignore output is not one complete record".to_owned());
    }
    let text = |index: usize, what: &str| -> Result<&str, String> {
        let bytes = fields[index];
        if bytes.is_empty() || bytes.contains(&b'\n') || bytes.contains(&b'\0') {
            return Err(format!("Git's ignore output has an unusable {what}"));
        }
        std::str::from_utf8(bytes)
            .map_err(|_| format!("Git's ignore output has a non-UTF-8 {what}"))
    };
    let source = text(0, "rule source")?;
    let line = text(1, "rule line")?;
    let pattern = text(2, "rule pattern")?;
    let pathname = text(3, "pathname")?;
    if pathname != expected {
        return Err("Git answered for another path".to_owned());
    }
    if !line.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Git's ignore output has an unusable rule line".to_owned());
    }
    let line = line
        .parse::<u64>()
        .map_err(|_| "Git's ignore output has an unusable rule line".to_owned())?;
    Ok(IgnoreRule {
        source: source.to_owned(),
        line,
        pattern: pattern.to_owned(),
    })
}

/// One bounded, effect-free Git call in `dir`, with `input` on stdin.
/// Optional locks and prompts are off, repository overrides are removed,
/// output is capped and a child that outlives the probe is killed. Every
/// failure is an error, never an answer. `literal_pathspecs` makes `--` paths
/// literal for `ls-files`; commands that reject that magic (check-ignore's
/// stdin paths) are called without it.
fn git_probe(
    dir: &Path,
    args: &[&str],
    input: &[u8],
    literal_pathspecs: bool,
) -> Result<GitProbe, String> {
    let name = args.first().copied().unwrap_or("git");
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(["-c", "submodule.recurse=0", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for variable in GIT_REPOSITORY_ENVIRONMENT {
        command.env_remove(variable);
    }
    // Inherited config injection could change the ignore decision (for
    // example `core.excludesFile`) from the one a plain `git status` in the
    // same checkout would make.
    for (name, _) in std::env::vars_os() {
        let Some(name) = name.to_str() else { continue };
        if name == "GIT_CONFIG_COUNT"
            || name == "GIT_CONFIG_PARAMETERS"
            || name.starts_with("GIT_CONFIG_KEY_")
            || name.starts_with("GIT_CONFIG_VALUE_")
        {
            command.env_remove(name);
        }
    }
    if literal_pathspecs {
        command.env("GIT_LITERAL_PATHSPECS", "1");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run git {name}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("git {name} kept no stdout"))?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .take(MAX_GIT_PROBE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(read);
    });
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let deadline = Instant::now() + GIT_PROBE_TIME;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("git {name} did not finish in time"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for git {name}: {error}"));
            }
        }
    };
    let settle = Duration::from_secs(1);
    let bytes = receiver
        .recv_timeout(settle)
        .map_err(|_| format!("git {name} did not close its output in time"))?
        .map_err(|error| format!("cannot read git {name} output: {error}"))?;
    if bytes.len() > MAX_GIT_PROBE_BYTES {
        return Err(format!("git {name} output is over its byte cap"));
    }
    Ok(GitProbe {
        code,
        stdout: bytes,
    })
}

/// One bounded Git probe's answer; a non-zero exit is an answer, not an error.
struct GitProbe {
    code: Option<i32>,
    stdout: Vec<u8>,
}

/// Whether Git would track a path inside the checkout. A checkout that is
/// not a work tree has no tracking risk.
fn git_would_track(checkout: &Path, relative: &str) -> Result<bool, String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["check-ignore", "-q", "--", relative])
        .status();
    match status {
        Err(_) => Ok(false),
        Ok(status) => match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => {
                let probe = Command::new("git")
                    .arg("-C")
                    .arg(checkout)
                    .args(["rev-parse", "--is-inside-work-tree"])
                    .output();
                match probe {
                    Ok(probe)
                        if probe.status.success()
                            && String::from_utf8_lossy(&probe.stdout).trim() == "true" =>
                    {
                        Err(
                            "git could not decide whether the local output root is ignored; refusing to create it"
                                .to_owned(),
                        )
                    }
                    Ok(_) => Ok(false),
                    Err(_) => Ok(false),
                }
            }
        },
    }
}

/// A path must be absolute and free of `..` for use in a receipt.
fn safe_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

/// Where `oxide clean` enumerates. Permanent owned outputs resolve through
/// the receipt to the selected real root; a clean never follows a link by
/// path text and never removes the link or the root's ownership marker.
pub struct CleanPlacement {
    target: PathBuf,
    keep_root: bool,
    link: PathBuf,
    inactive: Vec<PathBuf>,
}

impl CleanPlacement {
    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn keep_root(&self) -> bool {
        self.keep_root
    }

    /// What the clean keeps, for its report; empty for an ephemeral checkout.
    pub fn preserved_note(&self) -> Option<String> {
        if !self.keep_root {
            return None;
        }
        let inactive = if self.inactive.is_empty() {
            String::new()
        } else {
            format!(
                "; inactive roots kept: {}",
                self.inactive
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Some(format!(
            "{} and its ownership marker are kept, the owned link {} is kept{inactive}",
            self.target.display(),
            self.link.display()
        ))
    }
}

/// Resolve this checkout's clean target. `None` keeps today's ephemeral
/// behaviour; an owned checkout resolves to its selected real root and
/// refuses when the link and receipt disagree.
pub fn clean_placement(workspace: &Path) -> Result<Option<CleanPlacement>, String> {
    let settings = store::settings()?;
    let control = Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
    let context = Context::new(control, workspace.to_path_buf());
    let receipt = context.receipt()?;
    let unconverted = receipt
        .as_ref()
        .is_none_or(|receipt| receipt.state == ReceiptState::Detached);
    if unconverted && context.mode()? == Some(DriveMode::Permanent) {
        refuse_unowned_link(&context, "clean")?;
    }
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    match receipt.state {
        ReceiptState::Detached | ReceiptState::Pruned => Ok(None),
        ReceiptState::Pruning => Err("outputs: a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it".to_owned()),
        ReceiptState::Prepared => Err(
            "clean refused: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before cleaning"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            let selected = receipt
                .selected()
                .cloned()
                .ok_or("clean refused: the owned-output receipt selects no root")?;
            let target = context.target()?;
            if target.root() != Some(selected.root.as_path()) {
                return Err(format!(
                    "clean refused: {} is {}, not the owned link selecting {}; Oxide never cleans through a link or a foreign target. Run `oxide outputs adopt` or restore the link",
                    context.link().display(),
                    target.label(),
                    selected.root.display()
                ));
            }
            let marker = read_marker(&selected.root)?;
            check_marker(&marker, workspace, &context.checkout_id)?;
            if marker.kind != selected.kind || marker.volume_uuid != selected.volume_uuid {
                return Err(format!(
                    "clean refused: {} carries a marker for another root kind or volume",
                    selected.root.display()
                ));
            }
            if marker.generation > receipt.generation {
                return Err(format!(
                    "clean refused: {} carries generation {} ahead of the receipt's {}",
                    selected.root.display(),
                    marker.generation,
                    receipt.generation
                ));
            }
            // The root must be on the volume the receipt names *now*: a
            // replaced drive at the same mount point never lends its contents
            // to a clean.
            let binding = match selected.kind {
                RootKind::External => Some(binding_for(&context, selected.volume_uuid.as_deref().unwrap_or_default()).map_err(|reason| {
                    format!(
                        "clean refused: the registered volume for {} is not usable ({reason})",
                        selected.root.display()
                    )
                })?),
                RootKind::Local => None,
            };
            validate_root_device(workspace, &selected, binding.as_ref()).map_err(|reason| {
                format!("clean refused: {reason}")
            })?;
            let inactive = receipt
                .roots
                .iter()
                .filter(|root| root.root != selected.root && root.root.is_dir())
                .map(|root| root.root.clone())
                .collect();
            Ok(Some(CleanPlacement {
                target: selected.root,
                keep_root: true,
                link: context.link(),
                inactive,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        checkout: PathBuf,
        control: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            Self::in_tree("", "/target\n/.oxide-outputs/\n")
        }

        /// A checkout `relative` inside its own Git work tree whose
        /// `.gitignore` holds exactly `ignore`. A first conversion needs Git to
        /// ignore the exact non-directory link path (oxide#125).
        fn in_tree(relative: &str, ignore: &str) -> Self {
            let mut fixture = Self::without_git();
            let repo = fixture.root.join("repo");
            fs::create_dir(&repo).unwrap();
            let status = Command::new("git")
                .arg("init")
                .arg("-q")
                .current_dir(&repo)
                .status()
                .expect("git init in the fixture");
            assert!(status.success(), "git init failed: {status}");
            fs::write(repo.join(".gitignore"), ignore).unwrap();
            let checkout = if relative.is_empty() {
                repo
            } else {
                let checkout = repo.join(relative);
                fs::create_dir_all(&checkout).unwrap();
                checkout
            };
            fixture.checkout = checkout;
            fixture
        }

        /// A checkout that is not inside any Git work tree this fixture made.
        fn without_git() -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("oxide-outputs-{}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let checkout = root.join("checkout");
            fs::create_dir(&checkout).unwrap();
            let control = root.join("control");
            fs::create_dir(&control).unwrap();
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
            // Receipt writes publish through the registry authority, so every
            // fixture starts with a ready, empty registry.
            crate::prune::seed_test_registry(&control).unwrap();
            Self {
                root,
                checkout,
                control,
            }
        }

        fn context(&self) -> Context {
            Context::new(Some(self.control.clone()), self.checkout.clone())
        }

        /// The Git work tree `in_tree` made for this fixture.
        fn repo(&self) -> PathBuf {
            self.root.join("repo")
        }

        fn local(&self) -> RootSpec {
            local_spec(&self.checkout)
        }

        fn receipt(&self) -> Receipt {
            Receipt::first(
                &self.checkout,
                &identity::digest(&self.checkout.to_string_lossy()),
            )
        }
    }

    /// A store whose control directory also carries a ready empty registry, so
    /// tests can exercise the same receipt write-through as production.
    fn pool_store(root: &Path) -> Store {
        let store = Store::for_tests(root);
        crate::prune::seed_test_registry(store.control()).unwrap();
        store
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn receipts_reject_unknown_versions_fields_and_generations() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(fixture.local().selected(1));
        receipt.roots = vec![fixture.local().selected(1)];
        let id = fixture.context().checkout_id.clone();
        write_receipt(&fixture.control, &receipt).unwrap();
        assert_eq!(
            read_receipt(&fixture.control, &fixture.checkout, &id)
                .unwrap()
                .unwrap(),
            receipt
        );

        let path = receipt_path(&fixture.control, &id);
        let mut unknown = serde_json::to_value(&receipt).unwrap();
        unknown["version"] = serde_json::Value::String("oxide.outputs.transition/9".to_owned());
        fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        let refused = read_receipt(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(refused.contains("unsupported version"), "{refused}");

        let mut extra = serde_json::to_value(&receipt).unwrap();
        extra["surprise"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

        let mut ahead = receipt.clone();
        ahead.roots = vec![fixture.local().selected(3)];
        ahead.link_mut().selected = None;
        ahead.state = ReceiptState::Detached;
        fs::write(&path, serde_json::to_vec(&ahead).unwrap()).unwrap();
        assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

        let other = Fixture::new();
        let refused = read_receipt(&fixture.control, &other.checkout, &id).unwrap_err();
        assert!(refused.contains("another checkout"), "{refused}");
        let other_id = identity::digest(&other.checkout.to_string_lossy());
        assert!(read_receipt(&fixture.control, &other.checkout, &other_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_empty_root_gets_a_marker_and_a_nonempty_foreign_root_is_left_alone() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        assert!(spec.root().join(MARKER).is_file());
        let marker = read_marker(spec.root()).unwrap();
        assert_eq!(marker.kind, RootKind::Local);
        assert_eq!(marker.generation, 1);
        // Reusing the root keeps the marker and its creation generation.
        assert_eq!(ensure_root(&context, &spec, 4).unwrap().generation, 1);

        let other = Fixture::new();
        let foreign = other.local();
        fs::create_dir_all(foreign.root()).unwrap();
        fs::write(foreign.root().join("keep"), b"content").unwrap();
        let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
        assert!(refused.contains("holds content"), "{refused}");
        assert_eq!(fs::read(foreign.root().join("keep")).unwrap(), b"content");
    }

    #[test]
    fn a_marker_for_another_checkout_is_not_owned() {
        let fixture = Fixture::new();
        let context = fixture.context();
        ensure_root(&context, &fixture.local(), 1).unwrap();
        let other = Fixture::new();
        let foreign = other.local();
        fs::create_dir_all(foreign.root()).unwrap();
        fs::write(
            foreign.root().join(MARKER),
            serde_json::to_vec(&Marker {
                version: MARKER_VERSION.to_owned(),
                checkout: fixture.checkout.clone(),
                checkout_id: context.checkout_id.clone(),
                label: "x".to_owned(),
                recipe: RECIPE.to_owned(),
                kind: RootKind::Local,
                volume_uuid: None,
                nonce: Some("11111111111111111111111111111111".to_owned()),
                checkout_volume_uuid: None,
                generation: 1,
                created_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
        assert!(refused.contains("not this checkout"), "{refused}");
    }

    #[test]
    fn a_transition_publishes_an_owned_link_and_keeps_the_root() {
        let fixture = Fixture::new();
        let receipt = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
        )
        .unwrap();
        let link = fixture.checkout.join("target");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), fixture.local().root());
        assert_eq!(
            fixture.context().target().unwrap(),
            TargetState::OwnedLink(fixture.local().root().to_path_buf())
        );
        assert_eq!(receipt.state, ReceiptState::Committed);
        assert_eq!(receipt.generation, 1);
        assert_eq!(receipt.roots.len(), 1);
        // A second transition to the same place is a new generation, not a
        // new root.
        let second =
            transition(&fixture.context(), &receipt, &fixture.local(), Start::Any).unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.roots.len(), 1);
        assert!(!sibling_link(&link, 2).exists());
        assert!(fixture.local().root().join(MARKER).is_file());
    }

    #[test]
    fn a_nonempty_real_target_and_a_foreign_link_are_never_replaced() {
        let fixture = Fixture::new();
        let target = fixture.checkout.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"content").unwrap();
        let refused = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"content");
        assert!(fs::symlink_metadata(&target).unwrap().is_dir());
        assert!(!fixture.local().root().exists());

        let linked = Fixture::new();
        let outside = linked.root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"keep").unwrap();
        symlink(&outside, linked.checkout.join("target")).unwrap();
        let refused = transition(
            &linked.context(),
            &linked.receipt(),
            &linked.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"keep");
        assert!(!linked.local().root().exists());
        assert_eq!(
            fs::read_link(linked.checkout.join("target")).unwrap(),
            outside
        );
    }

    #[test]
    fn a_first_conversion_refuses_a_rule_that_would_not_cover_the_link() {
        // A trailing-slash rule matches directories only, so it never covers
        // the link. A missing target leaves Git answering "not ignored"; a
        // real empty target makes Git answer for a *directory* with that rule,
        // and that answer must not be read as eligibility either.
        for (label, rule, empty, reason) in [
            (
                "missing target, /target/",
                "/target/",
                false,
                "does not ignore",
            ),
            (
                "empty target, /target/",
                "/target/",
                true,
                "directories only",
            ),
            (
                "missing target, target/",
                "target/",
                false,
                "does not ignore",
            ),
            (
                "missing target, **/target/",
                "**/target/",
                false,
                "does not ignore",
            ),
            ("empty target, target/", "target/", true, "directories only"),
            (
                "empty target, **/target/",
                "**/target/",
                true,
                "directories only",
            ),
        ] {
            let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
            if empty {
                fs::create_dir(fixture.checkout.join("target")).unwrap();
            }
            let refused = transition(
                &fixture.context(),
                &fixture.receipt(),
                &fixture.local(),
                Start::Any,
            )
            .unwrap_err();
            assert!(refused.contains(reason), "{label}: {refused}");
            assert!(refused.contains("not `target/`"), "{label}: {refused}");
            assert!(
                refused.ends_with("nothing was changed"),
                "{label}: {refused}"
            );
            // No receipt, no root and no link.
            let loaded = read_receipt(
                &fixture.control,
                &fixture.checkout,
                &fixture.context().checkout_id,
            )
            .unwrap();
            assert!(
                loaded.is_none(),
                "{label}: a refused conversion wrote a receipt"
            );
            assert!(
                !fixture.local().root().exists(),
                "{label}: a refused conversion created a root"
            );
            let link = fixture.checkout.join("target");
            if empty {
                // The real empty directory is exactly as it was.
                assert!(
                    fs::symlink_metadata(&link).unwrap().is_dir(),
                    "{label}: the empty target was removed or replaced"
                );
                assert!(
                    fs::read_dir(&link).unwrap().next().is_none(),
                    "{label}: the empty target changed"
                );
            } else {
                assert!(!os_symlink_exists(&link), "{label}: a target appeared");
            }
        }

        for (label, ignore) in [
            ("negated", "target\n!/target\n/.oxide-outputs/\n"),
            ("unignored", "/.oxide-outputs/\n"),
        ] {
            let fixture = Fixture::in_tree("", ignore);
            let refused = transition(
                &fixture.context(),
                &fixture.receipt(),
                &fixture.local(),
                Start::Any,
            )
            .unwrap_err();
            let expected = if label == "negated" {
                "re-includes"
            } else {
                "does not ignore"
            };
            assert!(refused.contains(expected), "{label}: {refused}");
            assert!(
                refused.contains("nothing was changed"),
                "{label}: {refused}"
            );
            assert!(
                !fixture.local().root().exists(),
                "{label}: a refused conversion created a root"
            );
        }
    }

    #[test]
    fn a_first_conversion_accepts_a_non_directory_ignore_rule() {
        for rule in ["/target", "target"] {
            let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
            let receipt = transition(
                &fixture.context(),
                &fixture.receipt(),
                &fixture.local(),
                Start::Any,
            )
            .unwrap();
            assert_eq!(receipt.state, ReceiptState::Committed, "{rule}");
            assert!(
                os_symlink_exists(&fixture.checkout.join("target")),
                "{rule}"
            );
            let repo = fixture.repo();
            let ignore = fs::read(repo.join(".gitignore")).unwrap();
            let listed = status_entries_naming(&fixture.checkout, "target");
            assert!(
                listed.is_empty(),
                "{rule}: Git status lists the owned link: {listed:?}"
            );
            // The check is non-vacuous: with the rule gone the published link
            // is detected at its exact path.
            fs::write(repo.join(".gitignore"), b"").unwrap();
            let exposed = status_entries_naming(&fixture.checkout, "target");
            assert!(
                exposed.iter().any(|entry| entry.get(3..) == Some("target")),
                "{rule}: the detector missed an exposed link: {exposed:?}"
            );
            fs::write(repo.join(".gitignore"), &ignore).unwrap();
        }
    }

    #[test]
    fn a_nested_first_conversion_uses_the_exact_repository_relative_path() {
        let relative = "apps/farmyard/target";
        let allowed = Fixture::in_tree("apps/farmyard", "/apps/farmyard/target\n.oxide-outputs/\n");
        transition(
            &allowed.context(),
            &allowed.receipt(),
            &allowed.local(),
            Start::Any,
        )
        .unwrap();
        assert!(os_symlink_exists(&allowed.checkout.join("target")));
        let repo = allowed.repo();
        let ignore = fs::read(repo.join(".gitignore")).unwrap();
        let listed = status_entries_naming(&allowed.checkout, relative);
        assert!(
            listed.is_empty(),
            "Git status lists the nested link: {listed:?}"
        );
        // A wholly untracked workspace collapses to one entry such as `apps/`
        // in plain `git status`; every untracked file is listed here, so the
        // control detects the exact exposed nested link.
        fs::write(repo.join(".gitignore"), b"").unwrap();
        let exposed = status_entries_naming(&allowed.checkout, relative);
        assert!(
            exposed.iter().any(|entry| entry.get(3..) == Some(relative)),
            "the nested detector missed an exposed link: {exposed:?}"
        );
        fs::write(repo.join(".gitignore"), &ignore).unwrap();

        // The same rule with a real empty target makes Git answer for a
        // directory, which the winning rule's form then exposes.
        let empty = Fixture::in_tree("apps/farmyard", "/apps/farmyard/target/\n.oxide-outputs/\n");
        fs::create_dir(empty.checkout.join("target")).unwrap();
        let reason = transition(
            &empty.context(),
            &empty.receipt(),
            &empty.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(reason.contains("directories only"), "{reason}");
        assert!(
            reason.contains(&format!("not `{relative}/`"))
                && reason.contains(&format!("`/{relative}`")),
            "{reason}"
        );
        assert!(!empty.local().root().exists());

        // A missing target with the trailing-slash form refuses as unignored.
        let refused =
            Fixture::in_tree("apps/farmyard", "/apps/farmyard/target/\n.oxide-outputs/\n");
        let reason = transition(
            &refused.context(),
            &refused.receipt(),
            &refused.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(reason.contains("does not ignore"), "{reason}");
        assert!(
            reason.contains(&format!("not `{relative}/`"))
                && reason.contains(&format!("`/{relative}`")),
            "{reason}"
        );
        assert!(!refused.local().root().exists());
    }

    #[test]
    fn a_tracked_or_nongit_checkout_refuses_a_first_conversion() {
        // Tracked in the index but absent from the working tree: a link there
        // would be a tracked path.
        let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        fs::write(tracked.checkout.join("target"), b"tracked").unwrap();
        let added = Command::new("git")
            .args(["add", "-f", "target"])
            .current_dir(&tracked.checkout)
            .status()
            .unwrap();
        assert!(added.success());
        fs::remove_file(tracked.checkout.join("target")).unwrap();
        let refused = transition(
            &tracked.context(),
            &tracked.receipt(),
            &tracked.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(refused.contains("already tracks"), "{refused}");
        assert!(refused.contains("git rm --cached target"), "{refused}");
        assert!(!tracked.local().root().exists());

        let plain = Fixture::without_git();
        let refused = transition(
            &plain.context(),
            &plain.receipt(),
            &plain.local(),
            Start::Any,
        )
        .unwrap_err();
        assert!(refused.contains("could not confirm"), "{refused}");
        assert!(!plain.local().root().exists());
        assert!(!os_symlink_exists(&plain.checkout.join("target")));
    }

    #[test]
    fn the_verbose_ignore_record_parser_is_strict() {
        let rule =
            parse_ignore_rule(b".gitignore\x0010\x00/target\x00target\x00", "target").unwrap();
        assert_eq!(rule.source, ".gitignore");
        assert_eq!(rule.line, 10);
        assert_eq!(rule.pattern, "/target");
        // Another pathname, a missing terminator, extra records and empty
        // fields are all malformed, never a rule.
        assert!(parse_ignore_rule(b".gitignore\x001\x00/target\x00other\x00", "target").is_err());
        assert!(parse_ignore_rule(b".gitignore\x001\x00/target\x00target", "target").is_err());
        assert!(
            parse_ignore_rule(b".gitignore\x001\x00/target\x00target\x00x\x00", "target").is_err()
        );
        assert!(parse_ignore_rule(b"\x001\x00/target\x00target\x00", "target").is_err());
        assert!(parse_ignore_rule(b".gitignore\x00x\x00/target\x00target\x00", "target").is_err());
    }

    #[test]
    fn an_empty_real_target_is_converted_explicitly() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.checkout.join("target")).unwrap();
        let receipt = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
        )
        .unwrap();
        assert_eq!(receipt.generation, 1);
        assert!(fs::symlink_metadata(fixture.checkout.join("target"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_prepared_transition_with_a_live_sibling_is_retried() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let prepared = Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(prepared.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&prepared.to.root, sibling_link(&prepared.link, 1)).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
        assert_eq!(replayed.generation, 1);
        assert_eq!(fs::read_link(context.link()).unwrap(), prepared.to.root);
        assert!(!sibling_link(&prepared.link, 1).exists());
    }

    #[test]
    fn a_prepared_transition_after_the_rename_commits() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let prepared = Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(prepared.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&prepared.to.root, &prepared.link).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
        assert_eq!(replayed.selected().unwrap().root, prepared.to.root);
    }

    #[test]
    fn a_prepared_transition_with_nothing_published_rolls_back() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Detached);
        assert_eq!(replayed.generation, 0);
        assert_eq!(replayed.roots.len(), 1);
        assert!(!context.link().exists());
        assert!(spec.root().is_dir());
    }

    #[test]
    fn a_contradictory_prepared_transition_stops_and_keeps_both_roots() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let outside = fixture.root.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, context.link()).unwrap();
        let refused = replay(&context, receipt).unwrap_err();
        assert!(refused.contains("cannot be replayed"), "{refused}");
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(spec.root().is_dir());
        let loaded = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ReceiptState::Prepared);
    }

    #[test]
    fn the_slot_destination_is_empty_until_a_checkout_is_converted() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None).unwrap();
        assert!(planned.is_none());
        assert_eq!(destination_facts(None), "");
        let receipt =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None)
            .unwrap()
            .unwrap();
        assert_eq!(planned.kind, RootKind::Local);
        assert_eq!(planned.root, fixture.local().root());
        let first = destination_facts(Some(&planned));
        assert!(first.starts_with("outputs-2 "), "{first}");
        assert!(first.ends_with("g1"), "{first}");
        // Relinking to the same root keeps the same destination identity, so
        // a warm slot survives a relink.
        let again = transition(&context, &receipt, &fixture.local(), Start::Any).unwrap();
        assert_eq!(again.generation, 2);
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None)
            .unwrap()
            .unwrap();
        assert_eq!(destination_facts(Some(&planned)), first);
    }

    /// A crash during the first conversion leaves a prepared receipt with no
    /// committed selection. A link that appeared afterwards is not this
    /// receipt's: replay never renames over it, even when it dangles and a
    /// matching prepared sibling exists.
    /// The complete reused-root check: marker identity, kind, volume and the
    /// generation the slot was keyed with. A root that does not exist (or is
    /// empty and unmarked) may be created; anything contradictory refuses.
    #[test]
    fn the_reused_root_check_refuses_contradictions_and_allows_creation() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        assert!(matches!(
            check_root(&context, 1, &spec.selected(1)),
            RootCheck::Missing
        ));
        fs::create_dir_all(spec.root()).unwrap();
        assert!(matches!(
            check_root(&context, 1, &spec.selected(1)),
            RootCheck::Missing
        ));
        fs::write(spec.root().join("content"), b"x").unwrap();
        assert!(matches!(
            check_root(&context, 1, &spec.selected(1)),
            RootCheck::Contradiction(_)
        ));
        fs::remove_file(spec.root().join("content")).unwrap();
        ensure_root(&context, &spec, 1).unwrap();
        assert!(matches!(
            check_root(&context, 1, &spec.selected(1)),
            RootCheck::Valid
        ));
        // A slot keyed for another generation, kind or volume refuses.
        assert!(matches!(
            check_root(&context, 1, &spec.selected(2)),
            RootCheck::Contradiction(_)
        ));
        let mut external = spec.selected(1);
        external.kind = RootKind::External;
        assert!(matches!(
            check_root(&context, 1, &external),
            RootCheck::Contradiction(_)
        ));
        let mut other = spec.selected(1);
        other.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        assert!(matches!(
            check_root(&context, 1, &other),
            RootCheck::Contradiction(_)
        ));
        // A marker ahead of the receipt refuses even at the keyed generation.
        assert!(matches!(
            check_root(&context, 0, &spec.selected(1)),
            RootCheck::Contradiction(_)
        ));
    }

    #[test]
    fn a_first_conversion_never_renames_over_a_link_that_appeared_after_the_crash() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let mut receipt = fixture.receipt();
        let prepared = Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(prepared.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&prepared.to.root, sibling_link(&prepared.link, 1)).unwrap();
        let foreign = fixture.root.join("foreign-absent");
        symlink(&foreign, context.link()).unwrap();

        let refused = replay(&context, receipt).unwrap_err();
        assert!(refused.contains("cannot be replayed"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
        assert!(
            os_symlink_exists(&sibling_link(&prepared.link, 1)),
            "the prepared sibling was removed"
        );
        assert!(spec.root().is_dir());
    }

    fn os_symlink_exists(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    }

    /// Exact `git status` entries naming `relative`, read NUL delimited with
    /// every untracked file listed: a wholly untracked workspace would
    /// otherwise collapse to one entry such as `apps/` and hide an exposed
    /// `apps/farmyard/target`. Paths are compared exactly, never by suffix.
    fn status_entries_naming(checkout: &Path, relative: &str) -> Vec<String> {
        let output = Command::new("git")
            .args([
                "status",
                "--porcelain",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ])
            .current_dir(checkout)
            .output()
            .expect("git status in the fixture");
        assert!(output.status.success(), "git status: {output:?}");
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| String::from_utf8_lossy(entry).into_owned())
            .filter(|entry| {
                entry.get(2..3) == Some(" ")
                    && (entry.get(3..) == Some(relative) || entry.get(3..) == Some("target"))
            })
            .collect()
    }

    /// A prepared receipt replacing a committed external root rolls back when
    /// its target is still the exact old link (the ordinary crash-before-work
    /// case), and never touches a different link.
    #[test]
    fn a_prepared_replacement_of_a_committed_root_rolls_back_only_over_its_old_link() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        let first = transition(&context, &fixture.receipt(), &spec, Start::Any).unwrap();
        let old = first.selected().unwrap().root.clone();
        let mut receipt = first.clone();
        let prepared = Prepared {
            generation: first.generation + 1,
            to: spec.selected(first.generation + 1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(prepared);
        write_receipt(&fixture.control, &receipt).unwrap();
        // The old link is still live: roll back to it.
        let replayed = replay(&context, receipt.clone()).unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
        assert_eq!(replayed.selected().unwrap().root, old);
        // A foreign dangling link in its place is not the old generation.
        fs::remove_file(context.link()).unwrap();
        let foreign = fixture.root.join("elsewhere");
        symlink(&foreign, context.link()).unwrap();
        let refused = replay(&context, receipt).unwrap_err();
        assert!(refused.contains("cannot be replayed"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
    }

    /// With a committed selection, a dangling link that is not that selection
    /// is foreign for a transition too.
    #[test]
    fn a_committed_receipt_refuses_a_dangling_link_that_is_not_its_own() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let receipt =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let foreign = fixture.root.join("foreign-absent");
        fs::remove_file(context.link()).unwrap();
        symlink(&foreign, context.link()).unwrap();
        let refused = transition(&context, &receipt, &fixture.local(), Start::Any).unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
    }

    #[test]
    fn a_dangling_link_names_its_root_but_is_not_owned() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let absent = fixture.root.join("absent-root");
        symlink(&absent, context.link()).unwrap();
        let state = context.target().unwrap();
        assert_eq!(state, TargetState::Dangling(absent.clone()));
        assert_eq!(state.link_root(), Some(absent.as_path()));
        assert_eq!(state.root(), None);
        assert!(state.label().contains("not available"), "{}", state.label());
    }

    #[test]
    fn a_transition_away_from_the_receipts_dangling_link_is_allowed() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let absent = fixture.root.join("absent-root");
        let previous = Selected {
            kind: RootKind::Local,
            root: absent.clone(),
            volume_uuid: None,
            nonce: None,
            generation: 1,
        };
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(previous.clone());
        receipt.roots = vec![previous];
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&absent, context.link()).unwrap();
        let committed = transition(&context, &receipt, &fixture.local(), Start::Any).unwrap();
        assert_eq!(committed.generation, 2);
        assert_eq!(
            fs::read_link(context.link()).unwrap(),
            fixture.local().root()
        );
        assert!(!absent.exists(), "the absent root was created");
    }

    #[test]
    fn a_dangling_link_without_the_receipts_selection_is_never_replaced() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let absent = fixture.root.join("absent-root");
        symlink(&absent, context.link()).unwrap();
        let refused =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), absent);
    }

    #[test]
    fn a_root_with_a_wrong_volume_or_kind_is_not_owned() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let mut selected = spec.selected(1);
        selected.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        let refused =
            validate_selected(&fixture.checkout, &context.checkout_id, &selected, None, 1)
                .unwrap_err();
        assert!(refused.contains("another root kind or volume"), "{refused}");
        let mut external = spec.selected(1);
        external.kind = RootKind::External;
        let refused =
            validate_selected(&fixture.checkout, &context.checkout_id, &external, None, 1)
                .unwrap_err();
        let text = refused.to_string();
        assert!(
            text.contains("another root kind or volume") || text.contains("not usable"),
            "{text}"
        );
    }

    #[test]
    fn a_marker_generation_ahead_of_the_receipt_refuses() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 3).unwrap();
        let refused = validate_selected(
            &fixture.checkout,
            &context.checkout_id,
            &spec.selected(3),
            None,
            2,
        )
        .unwrap_err();
        assert!(refused.contains("ahead of the receipt"), "{refused}");
    }

    /// Replay validates the prepared root in full before it renames: a marker
    /// ahead of the prepared generation, or one for another volume, stops
    /// with the target, the sibling, the root and the prepared receipt kept.
    #[test]
    fn replay_refuses_a_contradictory_prepared_root_before_renaming() {
        for contradiction in ["ahead", "volume"] {
            let fixture = Fixture::new();
            let context = fixture.context();
            let spec = fixture.local();
            let mut to = spec.selected(1);
            match contradiction {
                "ahead" => {
                    ensure_root(&context, &spec, 4).unwrap();
                }
                _ => {
                    ensure_root(&context, &spec, 1).unwrap();
                    to.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
                }
            }
            let prepared = Prepared {
                generation: 1,
                to,
                link: context.link(),
            };
            let mut receipt = fixture.receipt();
            receipt.state = ReceiptState::Prepared;
            receipt.link_mut().prepared = Some(prepared.clone());
            write_receipt(&fixture.control, &receipt).unwrap();
            let sibling = sibling_link(&prepared.link, 1);
            symlink(&prepared.to.root, &sibling).unwrap();
            let refused = replay(&context, receipt.clone()).unwrap_err();
            assert!(
                refused.contains("cannot be replayed"),
                "{contradiction}: {refused}"
            );
            assert!(!os_symlink_exists(&context.link()), "{contradiction}");
            assert_eq!(fs::read_link(&sibling).unwrap(), prepared.to.root);
            assert!(prepared.to.root.join(MARKER).is_file());
            let kept = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            assert_eq!(kept, receipt, "{contradiction}");
        }
    }

    /// A symlinked `.oxide-outputs` on the same device is not the real local
    /// root: reuse, establishment and clean's device check refuse, and the
    /// tree it points at is left exactly as it was.
    #[test]
    fn a_symlinked_local_ancestor_is_refused_without_mutation() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        let receipt = transition(&context, &fixture.receipt(), &spec, Start::Any).unwrap();
        let selected = receipt.selected().unwrap().clone();
        let outer = fixture.checkout.join(LOCAL_DIR);
        let moved = fixture.root.join("moved-outputs");
        fs::rename(&outer, &moved).unwrap();
        symlink(&moved, &outer).unwrap();
        let listing = |path: &Path| {
            let mut names: Vec<_> = fs::read_dir(path)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let before = listing(&moved.join(LOCAL_ROOT));

        match check_root(&context, receipt.generation, &selected) {
            RootCheck::Contradiction(reason) => {
                assert!(reason.contains("not a real directory"), "{reason}")
            }
            _ => panic!("a symlinked local ancestor must contradict"),
        }
        let refused = validate_root_device(&fixture.checkout, &selected, None).unwrap_err();
        assert!(refused.contains("not a real directory"), "{refused}");
        assert!(ensure_root(&context, &spec, receipt.generation).is_err());
        assert_eq!(listing(&moved.join(LOCAL_ROOT)), before);
        assert_eq!(fs::read_link(&outer).unwrap(), moved);

        // Establishing a new root through a symlinked ancestor writes nothing.
        let fresh = Fixture::new();
        let context = fresh.context();
        let elsewhere = fresh.root.join("elsewhere");
        fs::create_dir_all(elsewhere.join(LOCAL_ROOT)).unwrap();
        symlink(&elsewhere, fresh.checkout.join(LOCAL_DIR)).unwrap();
        assert!(ensure_root(&context, &fresh.local(), 1).is_err());
        assert!(listing(&elsewhere.join(LOCAL_ROOT)).is_empty());
    }

    fn register(fixture: &Fixture, mode: DriveMode) {
        register_at(fixture, &fixture.control, mode);
    }

    fn register_at(fixture: &Fixture, control: &Path, mode: DriveMode) {
        let registration = removable::Registration {
            version: 2,
            mount_point: fixture.root.join("absent-volume"),
            volume_uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
            container_uuid: "66666666-7777-8888-9999-000000000000".to_owned(),
            reserve_bytes: 1,
            cap_bytes: 1 << 30,
            quota_bytes: None,
            capacity_bytes: None,
            registered_ms: 1,
            mode,
        };
        let path = control.join("removable.json");
        fs::write(&path, serde_json::to_vec(&registration).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn committed_external(fixture: &Fixture) -> (Receipt, PathBuf) {
        let root = fixture.root.join("volume/oxide-outputs/checkout-0000");
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(Selected {
            kind: RootKind::External,
            root: root.clone(),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: None,
            generation: 1,
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        (receipt, root)
    }

    /// Ephemeral mode never relinks, so a converted checkout plans the root
    /// its committed link selects (here external), not the local fallback.
    #[test]
    fn ephemeral_mode_plans_the_committed_root() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Ephemeral);
        let (_, external) = committed_external(&fixture);
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None)
            .unwrap()
            .unwrap();
        assert_eq!(planned.kind, RootKind::External);
        assert_eq!(planned.root, external);
        // With the intermediate slot on the internal pool, the external root
        // would be written unsupervised: ephemeral binding refuses.
        let context = fixture
            .context()
            .with_pool_fallback(Some("container mismatch"));
        let refused = match bind_ephemeral(&context, Some(&planned)) {
            Ok(_) => panic!("an unsupervised external root was bound"),
            Err(reason) => reason,
        };
        assert!(refused.contains("would not be supervised"), "{refused}");
    }

    /// A detached checkout stays detached under permanent mode (oxide#124):
    /// its receipt plans no owned root, so a command is keyed unconverted and
    /// the auto-adoption rule (any receipt blocks it) keeps it that way.
    #[test]
    fn a_detached_receipt_plans_no_root_in_permanent_mode() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Permanent);
        let (mut receipt, _) = committed_external(&fixture);
        receipt.state = ReceiptState::Detached;
        receipt.link_mut().selected = None;
        write_receipt(&fixture.control, &receipt).unwrap();
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None).unwrap();
        assert!(planned.is_none(), "{planned:?}");
        let mut facts = all_facts();
        facts.receipt_absent = false;
        assert!(!auto_adopts(&facts));
    }

    /// Permanent mode never selects the external root for a command whose
    /// intermediate slot fell back to the internal pool.
    #[test]
    fn a_pool_fallback_never_selects_the_external_root() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Permanent);
        let context = fixture
            .context()
            .with_pool_fallback(Some("container mismatch"));
        let refused = usable_external(&context).unwrap_err();
        assert!(
            refused.contains("fell back to the internal pool"),
            "{refused}"
        );
        let (_, external) = committed_external(&fixture);
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            Some("container mismatch"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(planned.kind, RootKind::Local);
        assert_ne!(planned.root, external);
    }

    /// A live link to another owned root (an older selection restored by
    /// hand) disagrees with the committed receipt: binding refuses, even
    /// though that root's marker is valid, and nothing changes.
    #[test]
    fn a_link_restored_to_an_older_owned_root_is_refused() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let local = transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let expected = local.selected().unwrap().clone();
        let mut receipt = local.clone();
        receipt.link_mut().selected = Some(Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: None,
            generation: 1,
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let store = pool_store(&fixture.root.join("pool"));
        match bind_under_binding(&context, &store, 0, &receipt, Some(&expected)) {
            Bind::Refuse(reason) => {
                assert!(reason.contains("committed receipt selects"), "{reason}")
            }
            Bind::Bound(_) => panic!("a link contradicting the committed receipt was bound"),
            Bind::Wait => panic!("a contradictory link waited instead of refusing"),
        }
        assert_eq!(fs::read_link(context.link()).unwrap(), expected.root);
        assert_eq!(
            read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap(),
            receipt
        );
    }

    /// A prepared receipt whose link names another checkout's target is
    /// refused when read, and replay never renames over that target.
    #[test]
    fn a_prepared_link_outside_this_checkout_is_never_renamed() {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let context = fixture.context();
        let committed =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let mut receipt = committed.clone();
        let foreign_link = other.checkout.join("target");
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(Prepared {
            generation: committed.generation + 1,
            to: committed.selected().unwrap().clone(),
            link: foreign_link.clone(),
        });
        assert!(receipt
            .validate(&fixture.checkout, &context.checkout_id)
            .is_err());
        let elsewhere = other.root.join("elsewhere");
        symlink(&elsewhere, &foreign_link).unwrap();
        let sibling = sibling_link(&foreign_link, committed.generation + 1);
        symlink(&committed.selected().unwrap().root, &sibling).unwrap();
        let refused = replay(&context, receipt).unwrap_err();
        assert!(refused.contains("not this checkout's target"), "{refused}");
        assert_eq!(fs::read_link(&foreign_link).unwrap(), elsewhere);
        assert_eq!(
            fs::read_link(&sibling).unwrap(),
            committed.selected().unwrap().root
        );
        assert_eq!(
            fs::read_link(context.link()).unwrap(),
            committed.selected().unwrap().root
        );
    }

    /// Automatic replay is gated on cross-pool custody: with a quarantined
    /// writer in the other pool it refuses before committing (or renaming),
    /// and the link, prepared receipt and root stay unchanged. Without the
    /// gate this prepared state commits at once.
    #[test]
    fn automatic_replay_refuses_unknown_custody_in_the_other_pool() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let root = committed.selected().unwrap().root.clone();
        let mut receipt = committed.clone();
        receipt.state = ReceiptState::Prepared;
        receipt.link_mut().prepared = Some(Prepared {
            generation: committed.generation + 1,
            to: committed.selected().unwrap().clone(),
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let path = receipt_path(&fixture.control, &context.checkout_id);
        let receipt_bytes = fs::read(&path).unwrap();

        let managed = oxide_storage::lease::MANAGED_NAME;
        let removable = pool_store(&fixture.root.join("removable"));
        removable.quarantine_for_tests(&fixture.checkout);
        let internal = pool_store(&fixture.root.join("internal"))
            .with_sibling(&fixture.root.join("removable").join(managed), false);
        let refused = gated_replay(&context, &internal, 0, receipt.clone()).unwrap_err();
        assert!(refused.contains("never confirmed closed"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), root);
        assert_eq!(fs::read(&path).unwrap(), receipt_bytes);
        assert!(root.join(MARKER).is_file());

        // The control: with clear custody the same state commits.
        let clear = pool_store(&fixture.root.join("clear"));
        let replayed = gated_replay(&context, &clear, 0, receipt).unwrap().unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
    }

    /// Rollback needs a positively absent sibling: a sibling selecting
    /// another root, or a regular file in its place, stops with the receipt,
    /// target, sibling and roots unchanged. A genuinely absent sibling still
    /// rolls back.
    #[test]
    fn a_contradictory_sibling_with_the_old_link_live_is_never_rolled_back() {
        for occupant in ["other-link", "file", "absent"] {
            let fixture = Fixture::new();
            let context = fixture.context();
            let committed =
                transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
            let old = committed.selected().unwrap().root.clone();
            let new_root = fixture.root.join("new-root");
            let mut receipt = committed.clone();
            receipt.state = ReceiptState::Prepared;
            receipt.link_mut().prepared = Some(Prepared {
                generation: committed.generation + 1,
                to: Selected {
                    kind: RootKind::Local,
                    root: new_root.clone(),
                    volume_uuid: None,
                    nonce: None,
                    generation: committed.generation + 1,
                },
                link: context.link(),
            });
            write_receipt(&fixture.control, &receipt).unwrap();
            let path = receipt_path(&fixture.control, &context.checkout_id);
            let bytes = fs::read(&path).unwrap();
            let sibling = sibling_link(&context.link(), committed.generation + 1);
            let elsewhere = fixture.root.join("elsewhere");
            match occupant {
                "other-link" => symlink(&elsewhere, &sibling).unwrap(),
                "file" => fs::write(&sibling, b"occupied").unwrap(),
                _ => {}
            }
            let result = replay(&context, receipt);
            if occupant == "absent" {
                assert_eq!(result.unwrap().state, ReceiptState::Committed);
                continue;
            }
            let refused = result.unwrap_err();
            assert!(
                refused.contains("cannot be replayed"),
                "{occupant}: {refused}"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes, "{occupant}");
            assert_eq!(fs::read_link(context.link()).unwrap(), old);
            match occupant {
                "other-link" => assert_eq!(fs::read_link(&sibling).unwrap(), elsewhere),
                _ => assert_eq!(fs::read(&sibling).unwrap(), b"occupied"),
            }
            assert!(old.join(MARKER).is_file());
        }
    }

    /// With the target already selecting the prepared root, the commit runs
    /// only when the sibling is positively absent: a sibling selecting
    /// another root, a regular file, or the prepared link itself (which a
    /// rename would have consumed) stops with receipt bytes, target, sibling
    /// and both roots unchanged. An absent sibling still commits.
    #[test]
    fn an_already_renamed_replay_commits_only_without_a_sibling() {
        for occupant in ["other-link", "file", "prepared-link", "absent"] {
            let fixture = Fixture::new();
            let context = fixture.context();
            let old_root = fixture.root.join("old-root");
            fs::create_dir(&old_root).unwrap();
            let spec = fixture.local();
            ensure_root(&context, &spec, 2).unwrap();
            let new_root = spec.root().to_path_buf();
            let mut receipt = fixture.receipt();
            receipt.state = ReceiptState::Prepared;
            receipt.generation = 1;
            receipt.link_mut().selected = Some(Selected {
                kind: RootKind::Local,
                root: old_root.clone(),
                volume_uuid: None,
                nonce: None,
                generation: 1,
            });
            receipt.link_mut().prepared = Some(Prepared {
                generation: 2,
                to: spec.selected(2),
                link: context.link(),
            });
            write_receipt(&fixture.control, &receipt).unwrap();
            symlink(&new_root, context.link()).unwrap();
            let path = receipt_path(&fixture.control, &context.checkout_id);
            let bytes = fs::read(&path).unwrap();
            let sibling = sibling_link(&context.link(), 2);
            let elsewhere = fixture.root.join("elsewhere");
            match occupant {
                "other-link" => symlink(&elsewhere, &sibling).unwrap(),
                "file" => fs::write(&sibling, b"occupied").unwrap(),
                "prepared-link" => symlink(&new_root, &sibling).unwrap(),
                _ => {}
            }
            let result = replay(&context, receipt);
            if occupant == "absent" {
                let committed = result.unwrap();
                assert_eq!(committed.state, ReceiptState::Committed);
                assert_eq!(committed.selected().unwrap().root, new_root);
                continue;
            }
            let refused = result.unwrap_err();
            assert!(
                refused.contains("cannot be replayed"),
                "{occupant}: {refused}"
            );
            assert_eq!(fs::read(&path).unwrap(), bytes, "{occupant}");
            assert_eq!(fs::read_link(context.link()).unwrap(), new_root);
            match occupant {
                "other-link" => assert_eq!(fs::read_link(&sibling).unwrap(), elsewhere),
                "prepared-link" => assert_eq!(fs::read_link(&sibling).unwrap(), new_root),
                _ => assert_eq!(fs::read(&sibling).unwrap(), b"occupied"),
            }
            assert!(old_root.is_dir() && new_root.join(MARKER).is_file());
        }
    }

    /// In permanent mode a checkout with no receipt never hands Cargo a
    /// link: a foreign link and a marked link whose receipt is missing both
    /// refuse with nothing written through them, while a real or missing
    /// target keeps the unconverted behaviour.
    #[test]
    fn a_receiptless_link_is_never_followed_in_permanent_mode() {
        let fixture = Fixture::new();
        let store = pool_store(&fixture.root.join("pool"));
        register_at(&fixture, store.owner_control(), DriveMode::Permanent);
        let context = fixture.context();
        let bind = |label: &str| match bind_for_command(&store, &fixture.checkout, 0, None, None) {
            Ok(_) => Ok(()),
            Err(reason) => Err(format!("{label}: {reason}")),
        };
        assert!(bind("missing").is_ok());
        fs::create_dir(context.link()).unwrap();
        assert!(bind("real").is_ok());
        fs::remove_dir(context.link()).unwrap();

        let foreign = fixture.root.join("foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("sentinel"), b"foreign").unwrap();
        symlink(&foreign, context.link()).unwrap();
        let refused = bind("foreign").unwrap_err();
        assert!(refused.contains("never follows"), "{refused}");
        assert_eq!(fs::read_link(context.link()).unwrap(), foreign);
        assert_eq!(fs::read_dir(&foreign).unwrap().count(), 1);

        // A marked root linked without its receipt is not owned either.
        fs::remove_file(context.link()).unwrap();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        symlink(spec.root(), context.link()).unwrap();
        let refused = bind("marked").unwrap_err();
        assert!(refused.contains("never follows"), "{refused}");
        assert!(context.receipt().unwrap().is_none());
    }

    /// A command keyed while its checkout was unconverted (no destination)
    /// refuses if the checkout was converted while it waited, in either
    /// mode; keyed for the owned root it binds normally.
    #[test]
    fn a_conversion_while_a_command_waits_refuses_its_unconverted_key() {
        for mode in [DriveMode::Permanent, DriveMode::Ephemeral] {
            let fixture = Fixture::new();
            let store = pool_store(&fixture.root.join("pool"));
            register_at(&fixture, store.owner_control(), mode);
            let context = Context::new(
                Some(store.owner_control().to_path_buf()),
                fixture.checkout.clone(),
            );
            // Keyed before conversion: no destination.
            let keyed =
                planned_destination(Some(store.owner_control()), &fixture.checkout, None).unwrap();
            assert!(keyed.is_none());
            // Adoption completes while the command waits.
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
            match bind_for_command(&store, &fixture.checkout, 0, keyed.as_ref(), None) {
                Ok(_) => panic!("{mode:?}: bound an owned root with an unconverted slot key"),
                Err(reason) => assert!(reason.contains("converted to owned outputs"), "{reason}"),
            }
            // Control: a command keyed after conversion binds that root.
            let rekeyed =
                planned_destination(Some(store.owner_control()), &fixture.checkout, None).unwrap();
            assert!(rekeyed.is_some());
            match bind_for_command(&store, &fixture.checkout, 0, rekeyed.as_ref(), None) {
                Ok(binding) => assert_eq!(
                    binding.outcome.root.as_deref(),
                    Some(fixture.local().root())
                ),
                Err(reason) => panic!("{mode:?}: {reason}"),
            }
        }
    }

    /// A converted command in ephemeral mode holds the binding mutex until
    /// it drops its binding (after it is recorded running), so a relinker
    /// started by a switch to permanent mode waits for it.
    #[test]
    fn a_converted_ephemeral_command_holds_the_binding_mutex() {
        let fixture = Fixture::new();
        let store = pool_store(&fixture.root.join("pool"));
        register_at(&fixture, store.owner_control(), DriveMode::Ephemeral);
        let context = Context::new(
            Some(store.owner_control().to_path_buf()),
            fixture.checkout.clone(),
        );
        transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let keyed =
            planned_destination(Some(store.owner_control()), &fixture.checkout, None).unwrap();
        let binding = bind_for_command(&store, &fixture.checkout, 0, keyed.as_ref(), None)
            .unwrap_or_else(|reason| panic!("{reason}"));
        // The mode switches to permanent and a relinker starts.
        register_at(&fixture, store.owner_control(), DriveMode::Permanent);
        let acquired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let relinker = {
            let acquired = acquired.clone();
            let root = fixture.root.join("pool");
            let checkout = fixture.checkout.clone();
            std::thread::spawn(move || {
                let store = pool_store(&root);
                let _lock = store.lock_binding(&checkout).unwrap();
                acquired.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !acquired.load(Ordering::SeqCst),
            "a relinker took the binding mutex while the converted command was starting"
        );
        // Control: once the command's binding is dropped, the relinker runs.
        drop(binding);
        relinker.join().unwrap();
        assert!(acquired.load(Ordering::SeqCst));
    }

    /// A receipt write publishes through the registry authority: a committed
    /// transition has one selected row, and a prepared transition keeps that
    /// committed root selected while the proposed root is pending.
    #[test]
    fn a_receipt_write_publishes_registry_rows_and_prepared_keeps_selection() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed =
            transition(&context, &fixture.receipt(), &fixture.local(), Start::Any).unwrap();
        let selected = committed.selected().unwrap().clone();
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(
            rows.iter().any(|(root, selected_flag, pending, pruning)| {
                root == &selected.root && *selected_flag && !*pending && pruning.is_none()
            }),
            "{rows:?}"
        );
        let mut prepared = committed.clone();
        prepared.state = ReceiptState::Prepared;
        prepared.link_mut().prepared = Some(Prepared {
            generation: committed.generation + 1,
            to: RootSpec::Local {
                root: fixture.root.join("next-root"),
            }
            .selected(committed.generation + 1),
            link: context.link(),
        });
        write_receipt(&fixture.control, &prepared).unwrap();
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(
            rows.iter()
                .any(|(root, selected_flag, _, _)| { root == &selected.root && *selected_flag }),
            "the prepared transition dropped the committed selection: {rows:?}"
        );
        assert!(
            rows.iter().any(|(root, selected_flag, pending, _)| {
                root == &fixture.root.join("next-root") && !*selected_flag && *pending
            }),
            "the proposed root is not pending: {rows:?}"
        );
        // A rolled-back preparation clears the pending reference; the
        // committed root stays selected.
        let mut rolled = committed.clone();
        rolled.link_mut().prepared = None;
        write_receipt(&fixture.control, &rolled).unwrap();
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(
            rows.iter().all(|(_, _, pending, _)| !*pending),
            "a rolled-back preparation left a pending reference: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|(root, selected_flag, _, _)| { root == &selected.root && *selected_flag }),
            "the rollback dropped the committed selection: {rows:?}"
        );
    }

    fn reservation_for(root: &Path, generation: u64) -> Reservation {
        Reservation {
            id: crate::prune::fresh_nonce().unwrap(),
            nonce: crate::prune::fresh_nonce().unwrap(),
            kind: RootKind::Local,
            root: root.to_path_buf(),
            volume_uuid: None,
            generation,
            created_ms: 1,
            overflow: None,
        }
    }

    /// A future root's nonce is reserved durably (receipt plus pending
    /// registry row), the transition that creates the root uses exactly that
    /// nonce and consumes the reservation, and cancellation runs through its
    /// journal so a stale starter holding the old nonce refuses.
    #[test]
    fn a_reserved_root_nonce_is_created_exactly_and_cancels_through_its_journal() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let RootSpec::Local { root: future } = fixture.local() else {
            panic!("the fixture's local spec is not local");
        };
        let mut reserved = fixture.receipt();
        let reservation = reservation_for(&future, 1);
        reserved.link_mut().reserved = Some(reservation.clone());
        write_receipt(&fixture.control, &reserved).unwrap();
        crate::prune::validate_reservation(&fixture.control, &reservation).unwrap();
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(
            rows.iter()
                .any(|(root, selected, pending, _)| root == &future && !*selected && *pending),
            "{rows:?}"
        );
        let wanted = fixture
            .local()
            .selected(1)
            .with_nonce(Some(reservation.nonce.clone()));
        reservation_matches(&context, &reserved, &wanted).unwrap();
        let created = transition(&context, &reserved, &fixture.local(), Start::Any).unwrap();
        assert_eq!(
            read_marker(&future).unwrap().nonce,
            Some(reservation.nonce.clone())
        );
        assert!(
            created.reserved().is_none(),
            "the commit kept its consumed reservation"
        );
        assert!(crate::prune::validate_reservation(&fixture.control, &reservation).is_err());

        // A second, unused reservation is cancelled through its journal; its
        // stale starter refuses and the abandoned row is gone.
        let later = fixture.root.join("later-root");
        let mut held = created.clone();
        let unused = reservation_for(&later, created.generation + 1);
        held.link_mut().reserved = Some(unused.clone());
        write_receipt(&fixture.control, &held).unwrap();
        let store = Store::for_tests(&fixture.root.join("pool"));
        cancel_reservation(&store, &fixture.control, &context, &mut held).unwrap();
        assert!(held.reserved().is_none());
        assert!(crate::prune::validate_reservation(&fixture.control, &unused).is_err());
        let stale = RootSpec::Local {
            root: later.clone(),
        }
        .selected(created.generation + 1)
        .with_nonce(Some(unused.nonce.clone()));
        let refused = reservation_matches(&context, &held, &stale).unwrap_err();
        assert!(refused.contains("cancelled or replaced"), "{refused}");
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        assert!(rows.iter().all(|(root, ..)| root != &later), "{rows:?}");
        // Dropping a live reservation without its journal is refused.
        let mut dropped = held.clone();
        let orphan = reservation_for(&later, created.generation + 1);
        dropped.link_mut().reserved = Some(orphan);
        write_receipt(&fixture.control, &dropped).unwrap();
        dropped.link_mut().reserved = None;
        let refused = write_receipt(&fixture.control, &dropped).unwrap_err();
        assert!(refused.contains("cancellation journal"), "{refused}");
    }

    /// A receiptless legacy root's checkout gets an exclusive `pruning`
    /// receipt: every replay/bind path refuses it, an existing receipt
    /// refuses the creator, only this writer's unjournaled creation is
    /// rebound, and the terminal receipt needs the same intent.
    #[test]
    fn a_pruning_receipt_is_exclusive_and_refuses_every_binding_path() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let checkout_id = identity::digest(&fixture.checkout.to_string_lossy());
        crate::prune::seed_test_registry(&fixture.control).unwrap();
        let first = crate::prune::fresh_nonce().unwrap();
        create_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &first,
            &|_| true,
        )
        .unwrap();
        let receipt = context.receipt().unwrap().unwrap();
        assert!(receipt_is_pruning(&receipt));
        assert!(replay(&context, receipt.clone())
            .unwrap_err()
            .contains("being pruned"));
        // Journaled: another creation refuses.
        let second = crate::prune::fresh_nonce().unwrap();
        assert!(create_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &second,
            &|_| true
        )
        .is_err());
        // This writer's own unjournaled creation is rebound to a new intent.
        create_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &second,
            &|_| false,
        )
        .unwrap();
        assert!(mark_pruned(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &first,
            "deleted"
        )
        .is_err());
        assert!(!discard_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &first
        )
        .unwrap());
        mark_pruned(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &second,
            "deleted",
        )
        .unwrap();
        assert_eq!(
            context.receipt().unwrap().unwrap().state,
            ReceiptState::Pruned
        );
        // A committed receipt is never replaced by the creator.
        let other = Fixture::new();
        let other_context = other.context();
        transition(&other_context, &other.receipt(), &other.local(), Start::Any).unwrap();
        let other_id = identity::digest(&other.checkout.to_string_lossy());
        assert!(create_pruning_receipt(
            &other.control,
            &other.checkout,
            &other_id,
            &first,
            &|_| false
        )
        .is_err());
    }

    #[test]
    fn paths_in_receipts_are_absolute() {
        assert!(safe_absolute(Path::new("/work/app/target")));
        assert!(!safe_absolute(Path::new("target")));
        assert!(!safe_absolute(Path::new("/work/../target")));
    }

    fn all_facts() -> AutoFacts {
        AutoFacts {
            selected_permanent: true,
            same_selection: true,
            external_usable: true,
            permanent: true,
            receipt_absent: true,
            target_absent: true,
        }
    }

    #[test]
    fn auto_adoption_needs_every_fact_and_nothing_else() {
        for bits in 0..64u8 {
            let facts = AutoFacts {
                selected_permanent: bits & 1 != 0,
                same_selection: bits & 2 != 0,
                external_usable: bits & 4 != 0,
                permanent: bits & 8 != 0,
                receipt_absent: bits & 16 != 0,
                target_absent: bits & 32 != 0,
            };
            assert_eq!(auto_adopts(&facts), bits == 63, "{facts:?}");
        }
    }

    /// A command whose slot came from the internal pool, or from a registration
    /// that was ephemeral when it selected, never converts, even when the drive
    /// is permanent by the time it checks. The all-true facts are the control.
    #[test]
    fn an_ephemeral_selection_never_converts_after_a_switch_to_permanent() {
        let switched = AutoFacts {
            selected_permanent: false,
            ..all_facts()
        };
        assert!(!auto_adopts(&switched));
        assert!(auto_adopts(&all_facts()), "positive control");
    }

    /// A command whose slot came from volume A never converts for volume B,
    /// even when B is the usable permanent registration now. The same volume
    /// in force now is the positive control.
    #[test]
    fn a_selection_of_one_volume_never_converts_another_registration() {
        let external = |volume: &str, uuid: &str| Selected {
            kind: RootKind::External,
            root: PathBuf::from(format!("/Volumes/{volume}/oxide-outputs/app-0000")),
            volume_uuid: Some(uuid.to_owned()),
            nonce: None,
            generation: 0,
        };
        let captured = external("a", "uuid-a");
        let switched = external("b", "uuid-b");
        assert!(!selection_matches(&captured, &switched));
        assert!(!auto_adopts(&AutoFacts {
            same_selection: selection_matches(&captured, &switched),
            ..all_facts()
        }));
        assert!(selection_matches(&captured, &captured.clone()));
        assert!(auto_adopts(&AutoFacts {
            same_selection: selection_matches(&captured, &captured.clone()),
            ..all_facts()
        }));
    }

    /// A `detached` receipt is a receipt: a missing target beside it is never
    /// a fresh checkout, and the receipt is read, not treated as absent.
    #[test]
    fn a_detached_receipt_blocks_auto_adoption_of_a_missing_target() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Detached;
        write_receipt(&fixture.control, &receipt).unwrap();
        let context = fixture.context();
        let receipt_absent = context.receipt().unwrap().is_none();
        assert!(!receipt_absent);
        assert!(!auto_adopts(&AutoFacts {
            selected_permanent: true,
            same_selection: true,
            external_usable: true,
            permanent: true,
            receipt_absent,
            target_absent: context.target().unwrap() == TargetState::Missing,
        }));
    }

    #[test]
    fn an_automatic_conversion_converts_only_an_absent_target() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly,
        )
        .unwrap();
        assert_eq!(committed.generation, 1);
        assert_eq!(
            fs::read_link(context.link()).unwrap(),
            fixture.local().root()
        );
    }

    /// Every present target (empty directory, file, dangling link) refuses
    /// before any receipt or sibling is written.
    #[test]
    fn an_automatic_conversion_refuses_every_present_target_unchanged() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let link = context.link();
        let dangling = fixture.root.join("absent-root");

        fs::create_dir(&link).unwrap();
        let refused = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly,
        )
        .unwrap_err();
        assert!(refused.contains("auto-adopt refused"), "{refused}");
        assert!(fs::symlink_metadata(&link).unwrap().is_dir());
        fs::remove_dir(&link).unwrap();

        fs::write(&link, b"kept").unwrap();
        assert!(transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly
        )
        .is_err());
        assert_eq!(fs::read(&link).unwrap(), b"kept");
        fs::remove_file(&link).unwrap();

        symlink(&dangling, &link).unwrap();
        assert!(transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly
        )
        .is_err());
        assert_eq!(fs::read_link(&link).unwrap(), dangling);
        assert!(
            read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .is_none()
        );
    }

    /// A staged link never replaces a file, directory or link that appeared
    /// after the target was classified absent.
    #[test]
    fn a_no_replace_publish_keeps_whatever_appeared_at_the_link() {
        let fixture = Fixture::new();
        let sibling = fixture.root.join("staged");
        symlink(fixture.root.join("new-root"), &sibling).unwrap();
        let link = fixture.root.join("target");
        for creator in ["file", "directory", "link"] {
            match creator {
                "file" => fs::write(&link, b"kept").unwrap(),
                "directory" => fs::create_dir(&link).unwrap(),
                _ => symlink(fixture.root.join("foreign"), &link).unwrap(),
            }
            assert!(publish_link(&sibling, &link, true).is_err(), "{creator}");
            assert!(os_symlink_exists(&sibling), "{creator}: staged link lost");
            assert!(fs::symlink_metadata(&link).is_ok(), "{creator} removed");
            if creator == "directory" {
                fs::remove_dir(&link).unwrap();
            } else {
                fs::remove_file(&link).unwrap();
            }
        }
        // Control: with nothing at the link, the exclusive publish succeeds on
        // macOS; elsewhere it refuses (no replacing fallback).
        let published = publish_link(&sibling, &link, true);
        assert_eq!(published.is_ok(), cfg!(target_os = "macos"));
    }

    #[test]
    fn a_changed_volume_or_root_is_not_the_captured_destination() {
        let captured = Selected {
            kind: RootKind::External,
            root: PathBuf::from("/Volumes/a/oxide-outputs/app-0000"),
            volume_uuid: Some("uuid-a".to_owned()),
            nonce: None,
            generation: 0,
        };
        assert!(captured.same_place(&captured));
        let other_volume = Selected {
            volume_uuid: Some("uuid-b".to_owned()),
            ..captured.clone()
        };
        let other_root = Selected {
            root: PathBuf::from("/Volumes/b/oxide-outputs/app-0000"),
            ..captured.clone()
        };
        assert!(!captured.same_place(&other_volume));
        assert!(!captured.same_place(&other_root));
    }

    /// The automatic conversion's destination is the one a later command keys:
    /// the planned destination is the committed selection itself.
    #[test]
    fn an_automatic_conversion_keys_the_committed_destination() {
        let fixture = Fixture::new();
        let committed = transition(
            &fixture.context(),
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly,
        )
        .unwrap();
        let planned = planned_destination(Some(&fixture.control), &fixture.checkout, None)
            .unwrap()
            .unwrap();
        assert_eq!(Some(&planned), committed.selected());
    }

    /// A metadata error on the target is an error, never a missing target.
    #[test]
    fn an_unreadable_checkout_is_an_error_not_an_absent_target() {
        // SAFETY: geteuid has no preconditions. Root reads through mode 000.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let fixture = Fixture::new();
        let id = fixture.context().checkout_id;
        fs::set_permissions(&fixture.checkout, fs::Permissions::from_mode(0o000)).unwrap();
        let result = target_state(&fixture.checkout, &id);
        fs::set_permissions(&fixture.checkout, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err(), "{result:?}");
    }
}
