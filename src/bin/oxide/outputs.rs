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
/// The bound successor receipt (oxide#133): `target` stays a real directory
/// and each positively identified profile member is an owned link. A legacy
/// receipt is never written in this form without an explicit migration.
const PROFILE_RECEIPT_VERSION: &str = "oxide.outputs.transition/3";
/// The whole-target receipt oxide#117 published. Prune reads a valid one as
/// its current in-memory whole-target form (oxide#152) so a legacy root is
/// not globally unprunable; every other reader still refuses it, and reading
/// or a dry run never rewrites it.
const LEGACY_RECEIPT_VERSION: &str = "oxide.outputs.transition/1";
const MARKER_VERSION: &str = "oxide.outputs.root/2";
const LEGACY_MARKER_VERSION: &str = "oxide.outputs.root/1";
const PLAN_VERSION: &str = "oxide.outputs.plan/2";
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
/// Debug-only rollback oracle for an uncommitted layout migration.
const ROLLBACK_ENVIRONMENT: &str = "OXIDE_TEST_MIGRATION_ROLLBACK";
/// Siblings of `<checkout>/target` that a layout migration owns.
const STAGE_PREFIX: &str = ".target.oxide-new-";
const BACKUP_PREFIX: &str = ".target.oxide-old-";
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
usage: oxide outputs plan      effect-free JSON plan for this checkout (oxide.outputs.plan/2)
       oxide outputs adopt     explicitly convert <checkout>/target to an owned link
                               (or upgrade a committed version-1 whole-target receipt)
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

/// Where one explicit whole-target-to-profile migration stands. Every phase is
/// durable before its effect; a replay advances only exact recorded
/// identities and leaves anything ambiguous pending.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MigrationPhase {
    /// P0: the journal and pinned old state are durable. No target effect yet.
    Prepared,
    /// P1: `.target.oxide-new-<g>` exists with exactly the member links.
    Staged,
    /// The root's legacy marker was replaced by its root/2 successor.
    MarkerUpgraded,
    /// P2: the old target link is at its `.target.oxide-old-<g>` backup.
    BackedUp,
    /// P3: the staging directory is the new real `target`.
    Swapped,
    /// P4: the receipt, root marker and member references are committed.
    Committed,
}

/// The migration journal of one explicit `oxide outputs adopt`. It pins the
/// exact old target link, parent, marker and receipt so no step ever acts on
/// anything it did not positively classify.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Migration {
    /// The receipt generation this migration commits at.
    generation: u64,
    phase: MigrationPhase,
    /// The root the migration keeps; for a root/1 marker this carries the ONE
    /// freshly reserved nonce every proposed member shares.
    root: Selected,
    /// The sorted v1 member names the new layout links.
    members: Vec<String>,
    link: PathBuf,
    link_text: PathBuf,
    link_dev: u64,
    link_ino: u64,
    parent_dev: u64,
    parent_ino: u64,
    stage: PathBuf,
    backup: PathBuf,
    /// Exact staging-directory identity, recorded when P1 is durable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage_dev: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage_ino: Option<u64>,
    /// A root/1 marker being upgraded: its pinned identity and backup leaf.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marker_pin: Option<MarkerPin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    marker_backup: Option<PathBuf>,
    /// The member directories this migration created inside the root before it
    /// could publish their links, recorded in the same durable phase write as
    /// P1. A rollback removes exactly these, and only while each is still the
    /// real empty directory the migration made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    created_members: Vec<String>,
    /// The receipt bytes this migration started from.
    prior_receipt_sha: String,
    /// The registry revision the journal was published at.
    registry_revision: u64,
    created_ms: u64,
    updated_ms: u64,
}

/// An old root marker's exact identity, so a rollback restores those bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkerPin {
    dev: u64,
    ino: u64,
    sha256: String,
}

impl Migration {
    fn validate(&self, receipt: &Receipt) -> Result<(), String> {
        let expected = match self.phase {
            MigrationPhase::Committed => receipt.generation,
            _ => receipt.generation.saturating_add(1),
        };
        if self.generation != expected || self.generation == 0 {
            return Err("the layout migration names an unusable generation".to_owned());
        }
        if self.link != receipt.checkout.join("target")
            || !safe_absolute(&self.root.root)
            || !safe_absolute(&self.stage)
            || !safe_absolute(&self.backup)
        {
            return Err("the layout migration names an unusable path".to_owned());
        }
        let mut names = self.members.clone();
        names.sort();
        names.dedup();
        if names != self.members || names.is_empty() {
            return Err("the layout migration names an unusable member set".to_owned());
        }
        for member in &names {
            crate::layout::Member::parse(member)
                .map_err(|reason| format!("a migrated member is unusable: {reason}"))?;
        }
        if let Some(unknown) = self
            .created_members
            .iter()
            .find(|member| !names.contains(member))
        {
            return Err(format!(
                "the layout migration records a created member directory it does not link: {unknown}"
            ));
        }
        crate::layout::validate_set(&names, crate::layout::MAX_RECEIPT_MEMBERS)
            .map_err(|reason| format!("the migrated member set is unusable: {reason}"))?;
        if self.root.nonce().is_none() || self.root.nonce().is_some_and(|nonce| nonce.len() != 32) {
            return Err("the layout migration needs one 128-bit root nonce".to_owned());
        }
        // Exactly one lowercase SHA-256 hex digest. A 128-bit root nonce is a
        // different value type and never satisfies this field.
        if !is_receipt_digest(&self.prior_receipt_sha) {
            return Err("the layout migration pins an unusable receipt digest".to_owned());
        }
        Ok(())
    }
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

/// How a receipt places this checkout's final outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LayoutKind {
    /// oxide#117/#129: `<checkout>/target` is one owned link to the root.
    /// Legacy receipts keep exactly this format and destination key until an
    /// explicit `oxide outputs adopt` migrates them.
    WholeTarget,
    /// The bound profile layout: `<checkout>/target` stays a real directory
    /// and each profile member is an owned link into the root.
    Profile,
}

impl LayoutKind {
    /// The receipt format this layout is published in: a legacy receipt is
    /// never silently rewritten as a successor.
    fn receipt_version(self) -> &'static str {
        match self {
            Self::WholeTarget => RECEIPT_VERSION,
            Self::Profile => PROFILE_RECEIPT_VERSION,
        }
    }
}

/// The checkout's owned-output receipt, keyed by canonical checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Receipt {
    version: String,
    checkout: PathBuf,
    checkout_id: String,
    /// Last committed generation; zero before the first conversion.
    generation: u64,
    state: ReceiptState,
    layout: LayoutKind,
    /// The owned placements of this layout: exactly one `target` placement
    /// for a legacy receipt, one per positively identified profile member for
    /// the bound profile layout. Roots hold immutable nonces; placements only
    /// reference them.
    links: Vec<LinkPlacement>,
    /// A durable random nonce reserved for a future root before a command's
    /// slot is keyed (oxide#129). Reservations are root-level: every member
    /// of this checkout shares one future root and its nonce. Commands that
    /// want the identical future root share it; it is consumed by the commit
    /// that creates the root or removed only by the cancellation journal.
    reserved: Option<Reservation>,
    /// Bounded history of roots this checkout has selected, newest last.
    roots: Vec<Selected>,
    /// The terminal outcome of an explicit prune, when this receipt was
    /// pruned or resolved.
    pruned: Option<PruneRecord>,
    /// The durable journal of an explicit whole-target-to-profile migration.
    /// Present from P0 until its committed cleanup finishes.
    transition: Option<Migration>,
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
pub(crate) struct LinkPlacement {
    member: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<Selected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prepared: Option<Prepared>,
}

impl LinkPlacement {
    pub(crate) fn member(&self) -> &str {
        &self.member
    }

    pub(crate) fn selected_ref(&self) -> Option<&Selected> {
        self.selected.as_ref()
    }

    pub(crate) fn prepared_ref(&self) -> Option<&Prepared> {
        self.prepared.as_ref()
    }
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
    /// A receipt for a checkout that has never converted: the legacy
    /// whole-target layout, selected by nothing.
    fn first(checkout: &Path, checkout_id: &str) -> Self {
        Self::first_with(checkout, checkout_id, LayoutKind::WholeTarget)
    }

    fn first_with(checkout: &Path, checkout_id: &str, layout: LayoutKind) -> Self {
        let links = match layout {
            LayoutKind::WholeTarget => vec![LinkPlacement {
                member: MEMBER.to_owned(),
                selected: None,
                prepared: None,
            }],
            LayoutKind::Profile => Vec::new(),
        };
        Self {
            version: layout.receipt_version().to_owned(),
            checkout: checkout.to_path_buf(),
            checkout_id: checkout_id.to_owned(),
            generation: 0,
            state: ReceiptState::Detached,
            layout,
            links,
            reserved: None,
            roots: Vec::new(),
            pruned: None,
            transition: None,
            updated_ms: process::now_ms(),
        }
    }

    /// The first placement: the legacy `target` link, or the first profile
    /// member in sorted order. Profile paths address members explicitly.
    #[allow(dead_code)]
    fn link(&self) -> &LinkPlacement {
        &self.links[0]
    }

    fn link_mut(&mut self) -> &mut LinkPlacement {
        &mut self.links[0]
    }

    pub(crate) fn layout(&self) -> LayoutKind {
        self.layout
    }

    pub(crate) fn links(&self) -> &[LinkPlacement] {
        &self.links
    }

    pub(crate) fn placement(&self, member: &str) -> Option<&LinkPlacement> {
        self.links
            .iter()
            .find(|placement| placement.member == member)
    }

    /// The placement a prepared transition belongs to, by its recorded link.
    /// The link path is authoritative, so a replay never depends on a file's
    /// contents to find its own transition.
    fn placement_for_link(&self, link: &Path) -> Option<&LinkPlacement> {
        self.links
            .iter()
            .find(|placement| self.link_path(&placement.member).as_path() == link)
    }

    fn placement_for_link_mut(&mut self, link: &Path) -> Option<&mut LinkPlacement> {
        let checkout = self.checkout.clone();
        self.links
            .iter_mut()
            .find(|placement| link_path(&checkout, placement.member.as_str()).as_path() == link)
    }

    /// The live link path of one placement member.
    fn link_path(&self, member: &str) -> PathBuf {
        link_path(&self.checkout, member)
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

    /// The layout's representative selected root, whether or not the state is
    /// committed. A profile layout may let two members select different
    /// roots (per-member fallback), so member-specific paths use
    /// [`Receipt::selected_for`].
    pub(crate) fn current(&self) -> Option<&Selected> {
        self.links
            .iter()
            .find_map(|placement| placement.selected.as_ref())
    }

    /// The root one member's placement selects.
    pub(crate) fn selected_for(&self, member: &str) -> Option<&Selected> {
        self.placement(member)
            .and_then(|placement| placement.selected.as_ref())
    }

    pub(crate) fn prepared_ref(&self) -> Option<&Prepared> {
        self.links
            .iter()
            .find_map(|placement| placement.prepared.as_ref())
    }

    pub(crate) fn reserved(&self) -> Option<&Reservation> {
        self.reserved.as_ref()
    }

    fn reserved_mut(&mut self) -> &mut Option<Reservation> {
        &mut self.reserved
    }

    /// Whether this receipt places the given profile member.
    #[allow(dead_code)]
    pub(crate) fn holds_member(&self, member: &str) -> bool {
        self.placement(member).is_some()
    }

    /// Add a placement for one profile member, keeping members sorted.
    fn place_member(&mut self, member: &str) -> &mut LinkPlacement {
        let position = self
            .links
            .iter()
            .position(|placement| placement.member == member)
            .unwrap_or_else(|| {
                self.links.push(LinkPlacement {
                    member: member.to_owned(),
                    selected: None,
                    prepared: None,
                });
                self.links
                    .sort_by(|left, right| left.member.cmp(&right.member));
                self.links
                    .iter()
                    .position(|placement| placement.member == member)
                    .unwrap_or(0)
            });
        &mut self.links[position]
    }

    /// Drop one member's placement after an explicit detach of that member.
    #[allow(dead_code)]
    fn forget_member(&mut self, member: &str) {
        self.links.retain(|placement| placement.member != member);
    }

    fn validate(&self, checkout: &Path, checkout_id: &str) -> Result<(), String> {
        let format_ok = match (self.version.as_str(), self.layout) {
            (RECEIPT_VERSION, LayoutKind::WholeTarget) => true,
            (PROFILE_RECEIPT_VERSION, LayoutKind::Profile) => true,
            // A migration journal is published in the successor format before
            // its commit; its placement is still the legacy whole target.
            (PROFILE_RECEIPT_VERSION, LayoutKind::WholeTarget) => self.transition.is_some(),
            _ => false,
        };
        if !format_ok {
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
        match self.layout {
            LayoutKind::WholeTarget => {
                if self.links.len() != 1 || self.links[0].member != MEMBER {
                    return Err(
                        "a whole-target owned-output receipt must hold exactly one `target` placement"
                            .to_owned(),
                    );
                }
            }
            LayoutKind::Profile => {
                if self.links.is_empty() || self.links.len() > crate::layout::MAX_RECEIPT_MEMBERS {
                    return Err(
                        "a profile owned-output receipt holds an unsupported number of members"
                            .to_owned(),
                    );
                }
                let mut names = Vec::with_capacity(self.links.len());
                for placement in &self.links {
                    crate::layout::Member::parse(&placement.member)
                        .map_err(|reason| format!("a profile placement is unusable: {reason}"))?;
                    names.push(placement.member.clone());
                }
                let mut sorted = names.clone();
                sorted.sort();
                sorted.dedup();
                if sorted != names {
                    return Err(
                        "the profile placements are not unique and sorted by member".to_owned()
                    );
                }
                crate::layout::validate_set(&sorted, crate::layout::MAX_RECEIPT_MEMBERS)
                    .map_err(|reason| format!("the profile placements are unusable: {reason}"))?;
            }
        }
        for placement in &self.links {
            if let Some(selected) = placement.selected.as_ref() {
                if !safe_absolute(&selected.root) || selected.generation > self.generation {
                    return Err(
                        "the owned-output receipt selects an unusable root or generation"
                            .to_owned(),
                    );
                }
                if let Some(nonce) = selected.nonce() {
                    if nonce.len() != 32 {
                        return Err(
                            "the owned-output receipt has an unusable root nonce".to_owned()
                        );
                    }
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
        let prepared = self.prepared_ref();
        if self
            .links
            .iter()
            .filter(|placement| placement.prepared.is_some())
            .count()
            > 1
        {
            return Err(
                "the owned-output receipt prepares more than one placement; it is not followed"
                    .to_owned(),
            );
        }
        if let Some(transition) = &self.transition {
            if self.version != PROFILE_RECEIPT_VERSION {
                return Err(
                    "a layout migration journal needs the successor receipt format".to_owned(),
                );
            }
            match transition.phase {
                MigrationPhase::Committed => {
                    if self.layout != LayoutKind::Profile {
                        return Err(
                            "a committed layout migration must select the profile layout"
                                .to_owned(),
                        );
                    }
                }
                _ => {
                    if self.layout != LayoutKind::WholeTarget {
                        return Err(
                            "a pending layout migration keeps the whole-target placement"
                                .to_owned(),
                        );
                    }
                }
            }
            transition.validate(self)?;
        }
        if let Some(prepared) = prepared {
            if prepared.generation != self.generation.saturating_add(1)
                || prepared.generation == 0
                || !safe_absolute(&prepared.to.root)
                || self.placement_for_link(&prepared.link).is_none()
            {
                return Err(
                    "the owned-output receipt prepares an unusable transition; it is not followed"
                        .to_owned(),
                );
            }
        }
        let consistent = match self.state {
            // A whole-target receipt names its one committed link. A profile
            // receipt places each member independently: a member with no
            // selection is a real local directory, or one left unselected by
            // a detach that a later adoption converts again, so only the
            // receipt as a whole must still select one place.
            ReceiptState::Committed => {
                prepared.is_none()
                    && !self.links.is_empty()
                    && match self.layout {
                        LayoutKind::Profile => self
                            .links
                            .iter()
                            .any(|placement| placement.selected.is_some()),
                        LayoutKind::WholeTarget => self
                            .links
                            .iter()
                            .all(|placement| placement.selected.is_some()),
                    }
            }
            ReceiptState::Detached => {
                prepared.is_none()
                    && self
                        .links
                        .iter()
                        .all(|placement| placement.selected.is_none())
            }
            ReceiptState::Pruned => prepared.is_none(),
            ReceiptState::Pruning => prepared.is_none() && self.pruned.is_some(),
            ReceiptState::Prepared => match &self.transition {
                Some(transition) => {
                    transition.validate(self)?;
                    prepared.is_none()
                        && self
                            .links
                            .iter()
                            .all(|placement| placement.prepared.is_none())
                }
                None => {
                    // The write-ahead transition names exactly one placement;
                    // a profile receipt's other members may be unselected
                    // (detached, awaiting their own re-adoption) without
                    // contradicting the pending transition.
                    prepared.is_some()
                        && (self.layout == LayoutKind::Profile
                            || self.links.iter().all(|placement| {
                                placement.selected.is_some() || placement.prepared.is_some()
                            }))
                }
            },
        };
        if !consistent {
            return Err(
                "the owned-output receipt is internally inconsistent; it is not followed"
                    .to_owned(),
            );
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

/// Ensure the member's real destination directory exists beneath the root.
/// The root itself is already validated as a real directory owned by this
/// checkout; a symlink or other entry at the member path is refused, never
/// followed.
fn ensure_member_directory(root: &Path, member: &str) -> Result<PathBuf, String> {
    let destination = member_destination(root, member);
    if member == MEMBER {
        return Ok(destination);
    }
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            return Ok(destination)
        }
        Ok(_) => {
            return Err(format!(
                "the output root's member entry {} is not a real directory; it is never followed",
                destination.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", destination.display())),
    }
    fs::create_dir_all(&destination).map_err(|error| {
        format!(
            "cannot create the member directory {}: {error}",
            destination.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&destination)
        .map_err(|error| format!("cannot inspect {}: {error}", destination.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "the created member destination {} is not a real directory",
            destination.display()
        ));
    }
    Ok(destination)
}

/// Create the real ancestor directories a nested profile member needs inside
/// the checkout's own `target` (`target/<triple>` for `<triple>/debug`). The
/// target root stays a real directory, so a member link at a second component
/// is published only after that ancestor exists as a real directory: a
/// symlink, a file or any other entry there is refused, never followed or
/// replaced, and nothing outside the checkout's target is created.
fn ensure_member_ancestors(context: &Context, link: &Path) -> Result<(), String> {
    let target = context.link();
    // The legacy whole-target placement IS the target directory, and a
    // one-component profile member sits directly in it: neither needs an
    // ancestor, but every profile member is published inside the real target
    // directory, so that root itself is inspected first (oxide#165).
    if link == target.as_path() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&target).map_err(|error| {
        format!(
            "cannot inspect {}: {error}; a profile member is placed only inside a real target directory",
            target.display()
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a real directory; a profile member is placed only inside one",
            target.display()
        ));
    }
    let Some(parent) = link.parent() else {
        return Ok(());
    };
    if parent == target.as_path() {
        return Ok(());
    }
    let relative = parent.strip_prefix(&target).map_err(|_| {
        format!(
            "{} is not beneath the checkout's target directory {}",
            link.display(),
            target.display()
        )
    })?;
    let mut current = target.clone();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "{} is not a real directory; Oxide never follows or replaces it",
                    current.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {
                        fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).map_err(
                            |error| {
                                format!(
                                    "cannot set permissions on the member directory {}: {error}",
                                    current.display()
                                )
                            },
                        )?;
                        sync_directory(&target)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&current).map_err(|error| {
                            format!("cannot inspect {}: {error}", current.display())
                        })?;
                        if !metadata.is_dir() || metadata.file_type().is_symlink() {
                            return Err(format!(
                                "{} is not a real directory; Oxide never follows or replaces it",
                                current.display()
                            ));
                        }
                    }
                    Err(error) => {
                        return Err(format!(
                            "cannot create the member directory {}: {error}",
                            current.display()
                        ))
                    }
                }
            }
            Err(error) => return Err(format!("cannot inspect {}: {error}", current.display())),
        }
    }
    Ok(())
}

/// Recreate a profile checkout's real `target` directory when it is
/// positively absent (oxide#165).
///
/// The profile layout keeps `<checkout>/target` itself a real directory and
/// publishes its owned member links inside it, so a checkout whose `target`
/// was deleted (a manual `rm -rf target`, a task-local build-output cleanup)
/// would otherwise fail every leased command with ENOENT while creating the
/// transition link, and never recreate the directory. The create is one
/// exclusive `mkdirat` beneath the checkout's own directory descriptor, so the
/// final component is never followed and an entry that appeared since the
/// caller's classification is never replaced; only a real directory counts as
/// that concurrent creator's result. The caller holds this checkout's binding
/// mutex and has cleared writer custody, so no cooperative command can be
/// racing this, and the missing-member transitions run immediately after it.
fn recreate_target_directory(checkout: &Path) -> Result<(), String> {
    let target = checkout.join("target");
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(checkout)
        .map_err(|error| format!("cannot open {}: {error}", checkout.display()))?;
    let named = fs::symlink_metadata(checkout)
        .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
    // A checkout path swapped for another directory between the classification
    // and this open is refused: the new directory is never written into.
    if !opened.is_dir() || opened.dev() != named.dev() || opened.ino() != named.ino() {
        return Err(format!(
            "{} changed while it was being opened",
            checkout.display()
        ));
    }
    let name = std::ffi::CString::new("target").expect("the fixed name has no NUL");
    // SAFETY: `directory` is a live descriptor for the checkout directory and
    // `name` is a live NUL-terminated string that outlives the call.
    let status = unsafe {
        libc::mkdirat(
            std::os::unix::io::AsRawFd::as_raw_fd(&directory),
            name.as_ptr(),
            0o700,
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("cannot create {}: {error}", target.display()));
        }
        // Something appeared between the classification and this exclusive
        // create. It is the same state the classification accepts when it is
        // already there — a real directory — or a refusal; a link, a file or
        // any other entry is never replaced or followed.
        return match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
            Ok(_) => Err(format!(
                "{} appeared while the missing target directory was being recreated and is not a real directory; it is never replaced",
                target.display()
            )),
            Err(error) => Err(format!("cannot inspect {}: {error}", target.display())),
        };
    }
    // Widen to the mode a conversion creates (oxide#121, oxide#133), through
    // the new directory's own descriptor so nothing else at that path is ever
    // chmod'ed.
    // SAFETY: `directory` is a live descriptor and the path is a fixed,
    // NUL-terminated single component.
    let created = unsafe {
        libc::openat(
            std::os::unix::io::AsRawFd::as_raw_fd(&directory),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if created < 0 {
        return Err(format!(
            "cannot open the recreated {}: {}",
            target.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `created` is a newly owned descriptor.
    let created = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(created) };
    if unsafe { libc::fchmod(std::os::unix::io::AsRawFd::as_raw_fd(&created), 0o755) } != 0 {
        return Err(format!(
            "cannot set permissions on the recreated {}: {}",
            target.display(),
            std::io::Error::last_os_error()
        ));
    }
    drop(created);
    directory
        .sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", checkout.display()))?;
    Ok(())
}

/// The real destination one member's link selects: the approved distinct
/// subpath beneath the shared root (`R/debug`, `R/release`,
/// `R/<triple>/debug`, ...). Only the legacy whole-target placement, member
/// `target`, selects the root directory itself.
pub(crate) fn member_destination(root: &Path, member: &str) -> PathBuf {
    if member == MEMBER {
        root.to_path_buf()
    } else {
        root.join(member)
    }
}

/// The live link path of one placement member under one checkout.
fn link_path(checkout: &Path, member: &str) -> PathBuf {
    if member == MEMBER {
        checkout.join("target")
    } else {
        checkout.join("target").join(member)
    }
}

impl Serialize for Receipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Doc<'a> {
            version: &'a str,
            checkout: &'a Path,
            checkout_id: &'a str,
            generation: u64,
            state: ReceiptState,
            #[serde(skip_serializing_if = "Option::is_none")]
            layout: Option<LayoutKind>,
            #[serde(skip_serializing_if = "Option::is_none")]
            transition: Option<&'a Migration>,
            links: Vec<PlacementDoc<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            reserved: Option<&'a Reservation>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            roots: &'a Vec<Selected>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pruned: Option<&'a PruneRecord>,
            updated_ms: u64,
        }
        #[derive(Serialize)]
        struct PlacementDoc<'a> {
            member: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            selected: Option<&'a Selected>,
            #[serde(skip_serializing_if = "Option::is_none")]
            prepared: Option<&'a Prepared>,
            /// A legacy receipt keeps its reservation inside its one
            /// placement, byte for byte as published.
            #[serde(skip_serializing_if = "Option::is_none")]
            reserved: Option<&'a Reservation>,
        }
        let legacy = self.version == RECEIPT_VERSION;
        let links = self
            .links
            .iter()
            .map(|placement| PlacementDoc {
                member: &placement.member,
                selected: placement.selected.as_ref(),
                prepared: placement.prepared.as_ref(),
                reserved: legacy.then_some(self.reserved.as_ref()).flatten(),
            })
            .collect();
        Doc {
            version: &self.version,
            checkout: &self.checkout,
            checkout_id: &self.checkout_id,
            generation: self.generation,
            state: self.state,
            layout: (!legacy).then_some(self.layout),
            transition: (!legacy).then_some(self.transition.as_ref()).flatten(),
            links,
            reserved: (!legacy).then_some(self.reserved.as_ref()).flatten(),
            roots: &self.roots,
            pruned: self.pruned.as_ref(),
            updated_ms: self.updated_ms,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Receipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PlacementDoc {
            member: String,
            #[serde(default)]
            selected: Option<Selected>,
            #[serde(default)]
            prepared: Option<Prepared>,
            #[serde(default)]
            reserved: Option<Reservation>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Doc {
            version: String,
            checkout: PathBuf,
            checkout_id: String,
            generation: u64,
            state: ReceiptState,
            #[serde(default)]
            layout: Option<LayoutKind>,
            #[serde(default)]
            transition: Option<Migration>,
            #[serde(default)]
            links: Vec<PlacementDoc>,
            #[serde(default)]
            reserved: Option<Reservation>,
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
        let legacy_reservations: Vec<Reservation> = doc
            .links
            .iter()
            .filter_map(|placement| placement.reserved.clone())
            .collect();
        let (version, layout, links) = match doc.version.as_str() {
            RECEIPT_VERSION => {
                if doc.layout.is_some() || doc.transition.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-2 receipt carries no successor layout or migration",
                    ));
                }
                if doc.selected.is_some() || doc.prepared.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-2 receipt carries placements, not top-level references",
                    ));
                }
                let links = doc
                    .links
                    .into_iter()
                    .map(|placement| LinkPlacement {
                        member: placement.member,
                        selected: placement.selected,
                        prepared: placement.prepared,
                    })
                    .collect();
                (RECEIPT_VERSION.to_owned(), LayoutKind::WholeTarget, links)
            }
            LEGACY_RECEIPT_VERSION => {
                // oxide#152: the whole-target shape oxide#117 published. It
                // predates layouts, placements, root-level reservations,
                // prune states and root nonces, so a document that carries
                // one of those is contradictory rather than merely old. It
                // is decoded into the current in-memory whole-target form;
                // only the prune readers accept it, and reading writes
                // nothing.
                if doc.layout.is_some()
                    || doc.transition.is_some()
                    || !doc.links.is_empty()
                    || !legacy_reservations.is_empty()
                    || doc.reserved.is_some()
                    || doc.pruned.is_some()
                    || matches!(doc.state, ReceiptState::Pruned | ReceiptState::Pruning)
                    || doc
                        .selected
                        .as_ref()
                        .is_some_and(|selected| selected.nonce.is_some())
                    || doc
                        .prepared
                        .as_ref()
                        .is_some_and(|prepared| prepared.to.nonce.is_some())
                    || doc.roots.iter().any(|root| root.nonce.is_some())
                {
                    return Err(serde::de::Error::custom(
                        "a version-1 receipt carries only its whole-target selection",
                    ));
                }
                (
                    RECEIPT_VERSION.to_owned(),
                    LayoutKind::WholeTarget,
                    vec![LinkPlacement {
                        member: MEMBER.to_owned(),
                        selected: doc.selected,
                        prepared: doc.prepared,
                    }],
                )
            }
            PROFILE_RECEIPT_VERSION => {
                if doc.selected.is_some() || doc.prepared.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-3 receipt carries placements, not top-level references",
                    ));
                }
                let layout = match doc.layout {
                    Some(LayoutKind::Profile) => LayoutKind::Profile,
                    Some(LayoutKind::WholeTarget) if doc.transition.is_some() => {
                        LayoutKind::WholeTarget
                    }
                    _ => {
                        return Err(serde::de::Error::custom(
                            "a version-3 receipt must declare its layout",
                        ))
                    }
                };
                if doc
                    .links
                    .iter()
                    .any(|placement| placement.reserved.is_some())
                {
                    return Err(serde::de::Error::custom(
                        "a version-3 reservation is root-level, not per placement",
                    ));
                }
                let links = doc
                    .links
                    .into_iter()
                    .map(|placement| LinkPlacement {
                        member: placement.member,
                        selected: placement.selected,
                        prepared: placement.prepared,
                    })
                    .collect();
                (PROFILE_RECEIPT_VERSION.to_owned(), layout, links)
            }
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unsupported version {other:?} of the owned-output receipt"
                )))
            }
        };
        // A legacy receipt keeps a reservation inside its placement; the
        // in-memory form holds one root-level reservation.
        let mut reserved = doc.reserved;
        if legacy_reservations.len() > 1 || (reserved.is_some() && !legacy_reservations.is_empty())
        {
            return Err(serde::de::Error::custom(
                "an owned-output receipt carries more than one reservation",
            ));
        }
        if let Some(placement) = legacy_reservations.into_iter().next() {
            reserved = Some(placement);
        }
        Ok(Receipt {
            version,
            checkout: doc.checkout,
            checkout_id: doc.checkout_id,
            generation: doc.generation,
            state: doc.state,
            layout,
            links,
            reserved,
            roots: doc.roots,
            pruned: doc.pruned,
            transition: doc.transition,
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

    /// The checkout's volume UUID recorded when the root was created, if the
    /// marker carries one (shared `oxide.outputs.root/2`). A version-1 marker
    /// has no such field and reports `None`; a recorded value is never waived.
    pub(crate) fn checkout_volume_uuid(&self) -> Option<&str> {
        self.checkout_volume_uuid.as_deref()
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
    /// The placement this checkout publishes: `none`, `whole-target` (a
    /// legacy receipt awaiting its explicit migration) or `profile`.
    layout: &'static str,
    /// Every member this checkout's receipt places, with its live state.
    /// Reading it never grants ownership of a real or foreign directory.
    members: Vec<MemberPlanView>,
    cooperative_holders: Vec<String>,
    next_generation: u64,
    /// Set only for a guarded one-use workspace: one line stating that final
    /// outputs are local and no owned root is selected (design §4.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    one_use: Option<&'static str>,
}

#[derive(Serialize)]
struct MemberPlanView {
    member: String,
    /// `owned-link`, `real-empty`, `real-nonempty`, `missing` or `foreign`.
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<PathBuf>,
}

fn plan_report(context: &Context) -> Result<PlanReport, String> {
    let mode = context.mode()?;
    let receipt = context.receipt()?;
    let target = context.target()?;
    let state = receipt
        .as_ref()
        .map_or("none", |receipt| receipt_state_label(receipt.state));
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
        layout: match receipt.as_ref().map(Receipt::layout) {
            None => "none",
            Some(LayoutKind::WholeTarget) => "whole-target",
            Some(LayoutKind::Profile) => "profile",
        },
        members: match &receipt {
            None => Vec::new(),
            Some(receipt) => receipt
                .links()
                .iter()
                .map(|placement| {
                    let link = link_path(&context.workspace, placement.member());
                    let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                    Ok(MemberPlanView {
                        member: placement.member().to_owned(),
                        state: state.code(),
                        root: state.link_root().map(Path::to_path_buf),
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        },
        cooperative_holders: holders,
        next_generation,
        one_use: None,
    };
    // A guarded workspace's plan is a local plan: it names no owned root and
    // never offers adopt or conversion advice (design §4.5).
    if guarded_workspace(context.control(), &context.workspace).is_some() {
        report.action = "local";
        report.one_use = Some(crate::one_use::PLAN_LOCAL);
        report.reason = Some(
            "this workspace is one-use (temporary): final outputs stay in its own target directory"
                .to_owned(),
        );
        return Ok(report);
    }
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
        if receipt.layout() == LayoutKind::Profile {
            lines.push(format!(
                "outputs: layout profile members; {} is a real directory",
                context.link().display()
            ));
            for placement in receipt.links() {
                let link = link_path(&context.workspace, placement.member());
                let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                let text = match (&state, placement.selected_ref()) {
                    (TargetState::OwnedLink(root), Some(selected)) if root == &selected.root => {
                        format!("owned link to {}", root.display())
                    }
                    (TargetState::RealEmpty, _) => "real empty directory, not owned".to_owned(),
                    (TargetState::RealNonEmpty, _) => "real directory, not owned".to_owned(),
                    (TargetState::Missing, _) => {
                        "missing; linked when a declaring command runs".to_owned()
                    }
                    (state, _) => format!("{} (not this receipt's link)", state.label()),
                };
                lines.push(format!("outputs: member {} {}", placement.member(), text));
            }
        } else if let Some(selected) = receipt.selected() {
            if target.link_root() != Some(selected.root.as_path()) {
                lines.push(format!(
                    "outputs: the target is {} rather than the owned link selecting {}; run `oxide outputs adopt`",
                    target.label(),
                    selected.root.display()
                ));
            }
        }
    }
    // One bounded line for a guarded one-use checkout (design §4.5, §10.5):
    // final outputs are local and no owned root is selected. Never a savings
    // claim.
    if guarded_workspace(context.control(), &context.workspace).is_some() {
        lines.push(crate::one_use::STATUS_LOCAL.to_owned());
    }
    Ok(lines)
}

/// `oxide outputs adopt`: the explicit first conversion and later relinks, and
/// the one command that upgrades a committed version-1 whole-target receipt
/// (oxide#163) by running the shipped layout migration transaction.
fn adopt_command() -> Result<u8, String> {
    let context = Context::open()?;
    adopt_at(&context, output_store)
}

/// `oxide outputs adopt` against one already-opened context and a **lazy** pool
/// provider, so the guarded refusal is decided before the pool is opened or
/// coordinated (design §4.1: nothing at all happens first) and the verb-level
/// regressions can hand in a provider that fails, proving the refusal wins.
fn adopt_at(
    context: &Context,
    pool: impl FnOnce() -> Result<Option<Store>, String>,
) -> Result<u8, String> {
    // A guarded workspace always refuses `adopt`, whatever the receipt state,
    // and it refuses here: effect-free, before the drive-mode, pool and
    // barrier work below, so the verb never enters custody (design §4.1,
    // §4.5). The state selects only the text.
    if guarded_workspace(context.control(), &context.workspace).is_some() {
        return Err(guarded_adopt_refusal(context));
    }
    // Otherwise the shipped order is kept **exactly**, including its error
    // precedence: drive mode, then the control directory, then the pool. The
    // pool provider is therefore called last, so a dormant build reports the
    // same refusal a pre-S1 build reported when more than one precondition is
    // unmet.
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
    let internal = pool()?;
    let Some(internal) = internal else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let internal = &internal;
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(internal, &context.workspace, &mut waits)?;
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
        if read_receipt_for_adopt(&control, &context.workspace, &context.checkout_id)?.is_some()
            || !matches!(
                context.target()?,
                TargetState::RealNonEmpty | TargetState::Foreign
            )
        {
            crate::prune::ensure_ready(&internal, &context.workspace)?;
        }
        let _admission = store::publication_guard(&control)?;
        let stored = read_receipt_for_adopt(&control, &context.workspace, &context.checkout_id)?;
        if let Some(receipt) = stored.as_ref() {
            if receipt.transition.is_some() {
                let replayed = replay_migration(&context, &control, receipt.clone())?;
                if let Some(selected) = replayed.selected().cloned() {
                    let converted = replayed.transition.is_none();
                    return Ok((replayed, selected, converted));
                }
            }
        }
        let fresh = stored.is_none();
        let receipt = match stored {
            // Only the explicit first conversion selects the profile layout.
            // A checkout that already has a receipt keeps the placement it
            // published, so a legacy whole-target link is never rewritten
            // without its own migration transaction.
            None => Receipt::first_with(
                &context.workspace,
                &context.checkout_id,
                LayoutKind::Profile,
            ),
            Some(receipt) => replay(&context, receipt)?,
        };
        if receipt.layout() == LayoutKind::Profile || fresh {
            adopt_profile(&context, receipt)
        } else if receipt.state == ReceiptState::Committed
            && context.mode()? == Some(DriveMode::Permanent)
            && matches!(context.target()?, TargetState::OwnedLink(_))
        {
            migrate_layout(&context, &control, receipt)
        } else {
            adopt_under_barrier(&context, receipt)
        }
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
            let committed = transition(
                context,
                &receipt,
                &spec,
                Start::Any,
                NonceAuthority::Unkeyed,
            )?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // A committed receipt whose link is missing: recreate it.
        (TargetState::Missing, Some(_)) => {
            let committed = transition(
                context,
                &receipt,
                &spec,
                Start::Any,
                NonceAuthority::Unkeyed,
            )?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // Explicit first conversion, or one after `detach`.
        (TargetState::Missing | TargetState::RealEmpty, _) => {
            let committed = transition(
                context,
                &receipt,
                &spec,
                Start::Any,
                NonceAuthority::Unkeyed,
            )?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        _ => Err(refuse_existing_target(context, &target)),
    }
}

/// The v1 members a migration links: the host debug/release pair, plus every
/// other v1-form directory the old whole-target root already holds. Nothing is
/// invented and no other directory is classified.
fn migration_members(root: &Path) -> Result<Vec<String>, String> {
    let mut names = vec!["debug".to_owned(), "release".to_owned()];
    for entry in fs::read_dir(root)
        .map_err(|error| format!("cannot read the output root {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // `file_type` does not follow a symlink, so a linked entry is never
        // classified as a directory here.
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        if crate::layout::Member::parse(&name).is_ok() {
            names.push(name);
            continue;
        }
        // One bounded second component: a positively identified plain triple
        // directory contributes only its own `debug` and `release` profile
        // directories. No third level and no other profile name is walked, so
        // a custom profile, a JSON target specification and anything else the
        // old root holds stay exactly as they are.
        if !crate::layout::plain_triple(&name) {
            continue;
        }
        for child in fs::read_dir(entry.path())
            .map_err(|error| format!("cannot read {}: {error}", entry.path().display()))?
        {
            let child = child.map_err(|error| error.to_string())?;
            let Some(leaf) = child.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if leaf != "debug" && leaf != "release" {
                continue;
            }
            if !child
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let member = format!("{name}/{leaf}");
            if crate::layout::Member::parse(&member).is_ok() {
                names.push(member);
            }
        }
    }
    names.sort();
    names.dedup();
    crate::layout::validate_set(&names, crate::layout::MAX_RECEIPT_MEMBERS)?;
    Ok(names)
}

/// Debug-only: `OXIDE_TEST_MIGRATION_ROLLBACK=1` asks `adopt` to restore the
/// exact pre-migration state instead of continuing. Absent from release
/// builds.
fn migration_rollback_requested(phase: MigrationPhase) -> bool {
    #[cfg(debug_assertions)]
    {
        let Some(value) = std::env::var(ROLLBACK_ENVIRONMENT).ok() else {
            return false;
        };
        // `1` rolls back at the first durable phase; a phase name rolls back
        // exactly when the journal reaches it, so both marker boundaries and
        // the pre-P4 target are exercisable.
        return value == "1"
            || (value == "marker" && phase == MigrationPhase::MarkerUpgraded)
            || (value == "staged" && phase == MigrationPhase::Staged)
            || (value == "swapped" && phase == MigrationPhase::Swapped);
    }
    #[allow(unreachable_code)]
    {
        let _ = phase;
        false
    }
}

/// SHA-256 of one file's exact bytes.
fn file_digest(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
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

/// SHA-256 of the receipt file's exact bytes, pinned before P0.
fn receipt_digest(control: &Path, receipt: &Receipt) -> Result<String, String> {
    file_digest(&receipt_path(control, &receipt.checkout_id))
}

/// A migration's pinned receipt preimage is exactly one lowercase SHA-256 hex
/// digest: 64 characters of `0-9a-f`. Empty, wrong-length, non-hex, uppercase
/// and 32-character nonce values are all unusable, and the nonce test is not a
/// substitute for the digest test.
fn is_receipt_digest(value: &str) -> bool {
    crate::prune::hex64(value)
}

/// Identity facts a migration step records in the same durable phase write as
/// the phase itself, so a replay never guesses them.
#[derive(Default)]
struct MigrationPatch {
    stage: Option<(u64, u64)>,
    marker_backup: Option<PathBuf>,
    created_members: Option<Vec<String>>,
}

/// Persist the journal with one new phase and the identities the step
/// established (durable before the next effect).
fn advance_migration(
    control: &Path,
    receipt: &Receipt,
    phase: MigrationPhase,
    patch: &MigrationPatch,
) -> Result<Receipt, String> {
    let mut writing = receipt.clone();
    let Some(transition) = writing.transition.as_mut() else {
        return Err("the receipt lost its migration journal".to_owned());
    };
    transition.phase = phase;
    if let Some((dev, ino)) = patch.stage {
        transition.stage_dev = Some(dev);
        transition.stage_ino = Some(ino);
    }
    if let Some(backup) = &patch.marker_backup {
        transition.marker_backup = Some(backup.clone());
    }
    if let Some(created) = &patch.created_members {
        transition.created_members = created.clone();
    }
    transition.updated_ms = process::now_ms();
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    Ok(writing)
}

/// Verify a staged directory's recorded identity when the journal has one.
fn verify_stage_identity(dir: &Path, migration: &Migration) -> Result<(), String> {
    if let (Some(dev), Some(ino)) = (migration.stage_dev, migration.stage_ino) {
        let meta = fs::symlink_metadata(dir)
            .map_err(|error| format!("cannot inspect {}: {error}", dir.display()))?;
        if meta.dev() != dev || meta.ino() != ino {
            return Err(pending_migration(
                migration,
                "the staging directory's identity changed",
            ));
        }
    }
    Ok(())
}

/// P3: the required pre-P4 backup of the old whole-target link. It must be the
/// exact link the journal pinned at P0 — same inode and same text — at its
/// recorded backup path. A missing, foreign or substituted link is never
/// accepted as the pinned original.
fn verify_target_backup(migration: &Migration) -> Result<(), String> {
    let meta = fs::symlink_metadata(&migration.backup).map_err(|error| {
        pending_migration(
            migration,
            &format!("the recorded target backup is missing ({error})"),
        )
    })?;
    if !meta.file_type().is_symlink()
        || meta.dev() != migration.link_dev
        || meta.ino() != migration.link_ino
    {
        return Err(pending_migration(
            migration,
            "the target backup is not the pinned old link",
        ));
    }
    let text = fs::read_link(&migration.backup).map_err(|error| {
        pending_migration(
            migration,
            &format!("cannot read the target backup: {error}"),
        )
    })?;
    if text != migration.link_text {
        return Err(pending_migration(
            migration,
            "the target backup link text changed",
        ));
    }
    Ok(())
}

/// P3: while a rollback is still possible, a root/1 upgrade's pinned old-marker
/// backup must be POSITIVELY present under the recorded root with its exact
/// inode and bytes. A journaled pathname is not proof that a required rollback
/// backup survives, and this is checked before any pre-P4 target rename or
/// staging publication.
fn verify_marker_backup(migration: &Migration) -> Result<(), String> {
    let Some(pin) = migration.marker_pin.as_ref() else {
        return Ok(());
    };
    let backup = migration.marker_backup.as_ref().ok_or_else(|| {
        pending_migration(migration, "the migration lost its old-marker backup record")
    })?;
    if backup.parent() != Some(migration.root.root.as_path()) {
        return Err(pending_migration(
            migration,
            "the old-marker backup is not under the recorded root",
        ));
    }
    let meta = fs::symlink_metadata(backup).map_err(|error| {
        pending_migration(
            migration,
            &format!("the recorded old-marker backup is missing ({error})"),
        )
    })?;
    let digest = file_digest(backup).map_err(|error| {
        pending_migration(
            migration,
            &format!("cannot digest the old-marker backup: {error}"),
        )
    })?;
    if meta.dev() != pin.dev || meta.ino() != pin.ino || digest != pin.sha256 {
        return Err(pending_migration(
            migration,
            "the old-marker backup is not the pinned marker",
        ));
    }
    Ok(())
}

/// Verify the live member link set of one real profile directory: exactly the
/// journal's sorted members, each a symlink into the journal's root. A
/// `<triple>/<profile>` member lives one level down in its own real triple
/// directory, which is walked exactly one component deep.
fn verify_member_links(dir: &Path, migration: &Migration) -> Result<(), String> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| format!("cannot inspect {}: {error}", dir.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", dir.display()));
    }
    let mut found = Vec::new();
    for entry in
        fs::read_dir(dir).map_err(|error| format!("cannot read {}: {error}", dir.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or("the migrated directory holds a non-UTF-8 entry")?
            .to_owned();
        if migration.members.contains(&name) {
            verify_member_link(&entry.path(), &name, migration)?;
            found.push(name);
            continue;
        }
        // Only a triple directory that carries recorded members is walked, so
        // an unrecorded directory refuses instead of being descended into.
        let nested: Vec<&String> = migration
            .members
            .iter()
            .filter(|member| {
                member
                    .split_once('/')
                    .is_some_and(|(triple, _)| triple == name.as_str())
            })
            .collect();
        if nested.is_empty() {
            return Err(format!(
                "{} holds {:?}, which this migration did not create",
                dir.display(),
                name
            ));
        }
        let nested_dir = entry.path();
        let nested_metadata = fs::symlink_metadata(&nested_dir)
            .map_err(|error| format!("cannot inspect {}: {error}", nested_dir.display()))?;
        if !nested_metadata.is_dir() || nested_metadata.file_type().is_symlink() {
            return Err(format!("{} is not a real directory", nested_dir.display()));
        }
        for child in fs::read_dir(&nested_dir)
            .map_err(|error| format!("cannot read {}: {error}", nested_dir.display()))?
        {
            let child = child.map_err(|error| error.to_string())?;
            let leaf = child
                .file_name()
                .to_str()
                .ok_or("the migrated directory holds a non-UTF-8 entry")?
                .to_owned();
            let member = format!("{name}/{leaf}");
            if !nested.iter().any(|known| known.as_str() == member) {
                return Err(format!(
                    "{} holds {:?}, which this migration did not create",
                    nested_dir.display(),
                    leaf
                ));
            }
            verify_member_link(&child.path(), &member, migration)?;
            found.push(member);
        }
    }
    found.sort();
    if found != migration.members {
        return Err(format!(
            "{} holds members {:?}, not the recorded {:?}",
            dir.display(),
            found,
            migration.members
        ));
    }
    Ok(())
}

/// One member entry must be a symlink to that member's own destination.
fn verify_member_link(path: &Path, member: &str, migration: &Migration) -> Result<(), String> {
    let link =
        fs::read_link(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if link != member_destination(&migration.root.root, member) {
        return Err(format!(
            "{} selects {}, not its migrated member destination",
            path.display(),
            link.display()
        ));
    }
    Ok(())
}

/// `oxide outputs adopt` on a committed whole-target checkout: the explicit
/// P0-P4 migration to profile member links. macOS only; Linux is compile-only.
fn migrate_layout(
    context: &Context,
    control: &Path,
    receipt: Receipt,
) -> Result<(Receipt, Selected, bool), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "outputs adopt: migrating a whole-target checkout to profile members needs the macOS removable backend; Linux remains compile-only"
                .to_owned(),
        );
    }
    let target = context.target()?;
    let selected = receipt
        .selected()
        .cloned()
        .ok_or("the receipt selects no root; there is nothing to migrate")?;
    match &target {
        TargetState::OwnedLink(root) if root == &selected.root => {}
        _ => {
            return Err(format!(
                "outputs adopt (migration) refused: {} is {}, not this checkout's owned whole-target link; nothing was changed",
                context.link().display(),
                target.label()
            ))
        }
    }
    let link = context.link();
    let link_meta = fs::symlink_metadata(&link)
        .map_err(|error| format!("cannot inspect {}: {error}", link.display()))?;
    if !link_meta.file_type().is_symlink() {
        return Err("the owned whole-target link is not a symlink; nothing was changed".to_owned());
    }
    let link_text =
        fs::read_link(&link).map_err(|error| format!("cannot read {}: {error}", link.display()))?;
    if link_text != selected.root {
        return Err(format!(
            "outputs adopt (migration) refused: {} selects {}, not the recorded {}; nothing was changed",
            link.display(),
            link_text.display(),
            selected.root.display()
        ));
    }
    let parent = context.workspace.clone();
    let parent_meta = fs::symlink_metadata(&parent)
        .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    // The marker: an existing root/2 marker keeps its nonce; a root/1 marker
    // gets ONE freshly reserved nonce in this journal before any marker effect.
    let marker = read_marker(&selected.root)?;
    check_marker(&marker, &context.workspace, &context.checkout_id)?;
    let root = match marker.nonce() {
        Some(nonce) => selected.clone().with_nonce(Some(nonce.to_owned())),
        None => selected
            .clone()
            .with_nonce(Some(crate::prune::fresh_nonce()?)),
    };
    let marker_pin = if marker.is_legacy() {
        let path = selected.root.join(MARKER);
        let meta = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot inspect the owner marker: {error}"))?;
        Some(MarkerPin {
            dev: meta.dev(),
            ino: meta.ino(),
            sha256: file_digest(&path)?,
        })
    } else {
        None
    };
    let migration = Migration {
        generation,
        phase: MigrationPhase::Prepared,
        root,
        members: migration_members(&selected.root)?,
        link: link.clone(),
        link_text,
        link_dev: link_meta.dev(),
        link_ino: link_meta.ino(),
        parent_dev: parent_meta.dev(),
        parent_ino: parent_meta.ino(),
        stage: parent.join(format!("{STAGE_PREFIX}{generation}")),
        backup: parent.join(format!("{BACKUP_PREFIX}{generation}")),
        stage_dev: None,
        stage_ino: None,
        marker_pin,
        marker_backup: None,
        created_members: Vec::new(),
        prior_receipt_sha: receipt_digest(control, &receipt)?,
        registry_revision: crate::prune::registry_revision(control)?,
        created_ms: process::now_ms(),
        updated_ms: process::now_ms(),
    };
    migration.validate(&receipt)?;
    // P0: the durable journal in the successor format. The whole-target
    // placement stays selected, so its key is unchanged until P4.
    let writing = publish_migration_journal(control, &receipt, migration)?;
    crash_point("migration-p0")?;
    let done = replay_migration(context, control, writing)?;
    let selected = done
        .selected()
        .cloned()
        .ok_or("the migration committed no root")?;
    Ok((done, selected, true))
}

/// Publish the P0 journal durably. Ordinary commands refuse while it is
/// pending; nothing on the file system has changed yet.
fn publish_migration_journal(
    control: &Path,
    receipt: &Receipt,
    migration: Migration,
) -> Result<Receipt, String> {
    let mut writing = receipt.clone();
    writing.version = PROFILE_RECEIPT_VERSION.to_owned();
    writing.state = ReceiptState::Prepared;
    writing.transition = Some(migration);
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    Ok(writing)
}

/// Advance or finish an explicit layout migration from its durable journal.
/// Only exact recorded identities are acted on; anything else stays pending.
fn replay_migration(
    context: &Context,
    control: &Path,
    receipt: Receipt,
) -> Result<Receipt, String> {
    let Some(migration) = receipt.transition.clone() else {
        return Ok(receipt);
    };
    if !cfg!(target_os = "macos") {
        return Err(
            "outputs: a layout migration is pending and needs the macOS removable backend"
                .to_owned(),
        );
    }
    // The journal is revalidated against the receipt it is replayed from, so a
    // malformed pinned digest (or any other unusable pinned field) refuses here
    // instead of being acted on.
    migration.validate(&receipt)?;
    if migration.phase == MigrationPhase::Committed {
        return finish_migration_cleanup(context, control, receipt, &migration);
    }
    if migration_rollback_requested(migration.phase) {
        return rollback_migration(context, control, receipt, &migration);
    }
    // Every step rechecks the exact identities the journal pinned before it
    // acts; a contradicting observation is a pending migration, never a guess.
    let parent = context.workspace.clone();
    let parent_meta = fs::symlink_metadata(&parent)
        .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
    if parent_meta.dev() != migration.parent_dev || parent_meta.ino() != migration.parent_ino {
        return Err(pending_migration(
            &migration,
            "the checkout directory identity changed",
        ));
    }
    // The pre-P0 receipt digest is an audit pin. Check that the live receipt is
    // this exact journal, not a different transition that reused the name.
    match &receipt.transition {
        Some(live) if live == &migration => {}
        _ => {
            return Err(pending_migration(
                &migration,
                "the live receipt is not this journal",
            ))
        }
    }
    let mut current = receipt;
    loop {
        let migration = current
            .transition
            .clone()
            .ok_or("the migration journal disappeared")?;
        if migration_rollback_requested(migration.phase) {
            return rollback_migration(context, control, current, &migration);
        }
        let (next, patch) = match migration.phase {
            MigrationPhase::Prepared => stage_migration(context, &parent, &migration)?,
            MigrationPhase::Staged if migration.marker_pin.is_some() => {
                upgrade_marker(context, &migration)?
            }
            MigrationPhase::Staged | MigrationPhase::MarkerUpgraded => {
                (back_up_target(&migration)?, MigrationPatch::default())
            }
            MigrationPhase::BackedUp => (swap_in_staging(&migration)?, MigrationPatch::default()),
            MigrationPhase::Swapped => {
                return commit_migration(context, control, current, &migration);
            }
            MigrationPhase::Committed => {
                return finish_migration_cleanup(context, control, current, &migration)
            }
        };
        current = advance_migration(control, &current, next, &patch)?;
        crash_point(&format!("migration-{}", migration_phase_label(next)))?;
    }
}

fn migration_phase_label(phase: MigrationPhase) -> &'static str {
    match phase {
        MigrationPhase::Prepared => "prepared",
        MigrationPhase::Staged => "staged",
        MigrationPhase::MarkerUpgraded => "marker",
        MigrationPhase::BackedUp => "backed-up",
        MigrationPhase::Swapped => "swapped",
        MigrationPhase::Committed => "committed",
    }
}

fn pending_migration(migration: &Migration, reason: &str) -> String {
    format!(
        "outputs: the layout migration at phase {:?} cannot proceed: {reason}. The journal, {}, the backup and both layouts are kept exactly as they are",
        migration.phase,
        migration.link.display()
    )
}

/// The migration's root, positively verified as THIS checkout's unchanged
/// owned root before anything is created inside it. A root/2 marker must carry
/// the journal's nonce; a root/1 marker must be the exact legacy document the
/// journal pinned (its upgrade happens only after P1). The root's kind and
/// volume, its live device and ancestry are checked too, so a replaced or
/// misplaced directory is never written into.
fn verify_migration_root(context: &Context, migration: &Migration) -> Result<(), String> {
    let root = &migration.root.root;
    let binding = match migration.root.kind {
        RootKind::External => Some(
            binding_for(
                context,
                migration.root.volume_uuid.as_deref().unwrap_or_default(),
            )
            .map_err(|reason| pending_migration(migration, &reason))?,
        ),
        RootKind::Local => None,
    };
    validate_selected(
        &context.workspace,
        &context.checkout_id,
        &migration.root,
        binding.as_ref(),
        migration.generation,
    )
    .map_err(|reason| pending_migration(migration, &reason))?;
    let marker = read_marker(root)?;
    check_marker(&marker, &context.workspace, &context.checkout_id)
        .map_err(|reason| pending_migration(migration, &reason))?;
    if marker.is_legacy() {
        let Some(pin) = migration.marker_pin.as_ref() else {
            return Err(pending_migration(
                migration,
                "the root still carries a legacy marker the journal never pinned",
            ));
        };
        if file_digest(&root.join(MARKER))? != pin.sha256 {
            return Err(pending_migration(
                migration,
                "the legacy root marker changed since the journal pin",
            ));
        }
    } else if marker.nonce() != migration.root.nonce() {
        return Err(pending_migration(
            migration,
            "the root does not carry the migrated nonce",
        ));
    }
    Ok(())
}

/// P1: the transaction-owned staging directory with only classified member
/// links into the same root.
///
/// No link is ever published to an absent member directory: every classified
/// member's real directory inside the root exists (verified as this checkout's
/// owned root carrying the journal's marker) before the staged link can name
/// it. The directories this step had to create are recorded in the same P1
/// phase write, so a rollback removes exactly those and nothing else.
fn stage_migration(
    context: &Context,
    parent: &Path,
    migration: &Migration,
) -> Result<(MigrationPhase, MigrationPatch), String> {
    verify_migration_root(context, migration)?;
    let mut created = Vec::new();
    for member in &migration.members {
        let destination = member_destination(&migration.root.root, member);
        let absent = matches!(
            fs::symlink_metadata(&destination),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        );
        ensure_member_directory(&migration.root.root, member).map_err(|reason| {
            format!(
                "the migration cannot publish the member directory for {member}: {reason}; the journal and both layouts are kept exactly as they are"
            )
        })?;
        if absent {
            created.push(member.clone());
        }
    }
    if !created.is_empty() {
        sync_directory(&migration.root.root)?;
    }
    match fs::symlink_metadata(&migration.stage) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&migration.stage)
                .map_err(|error| format!("cannot create {}: {error}", migration.stage.display()))?;
            fs::set_permissions(&migration.stage, fs::Permissions::from_mode(0o755)).map_err(
                |error| format!("cannot set permissions on the staging directory: {error}"),
            )?;
            for member in &migration.members {
                let link = migration.stage.join(member);
                // A `<triple>/<profile>` member needs its own real ancestor
                // directory inside the transaction-owned staging directory.
                if let Some(parent) = link.parent() {
                    if parent != migration.stage.as_path() {
                        // The first member of a triple creates its directory;
                        // the second one legitimately finds it already there.
                        match fs::create_dir(parent) {
                            Ok(()) => {
                                fs::set_permissions(
                                    parent,
                                    fs::Permissions::from_mode(0o755),
                                )
                                .map_err(|error| {
                                    format!(
                                        "cannot set permissions on the staged member directory: {error}"
                                    )
                                })?;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                            Err(error) => {
                                return Err(format!(
                                    "cannot create the staged member directory {}: {error}",
                                    parent.display()
                                ))
                            }
                        }
                    }
                }
                symlink(&member_destination(&migration.root.root, member), &link).map_err(
                    |error| {
                        format!(
                            "cannot create the staged member link {}: {error}",
                            link.display()
                        )
                    },
                )?;
            }
            sync_directory(&migration.stage)?;
            sync_directory(parent)?;
        }
        Ok(_) => {
            // A crash after the links but before the phase record.
            verify_member_links(&migration.stage, migration)?;
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect {}: {error}",
                migration.stage.display()
            ))
        }
    }
    verify_member_links(&migration.stage, migration)?;
    verify_stage_identity(&migration.stage, migration)?;
    let meta = fs::symlink_metadata(&migration.stage)
        .map_err(|error| format!("cannot inspect {}: {error}", migration.stage.display()))?;
    Ok((
        MigrationPhase::Staged,
        MigrationPatch {
            stage: Some((meta.dev(), meta.ino())),
            marker_backup: None,
            created_members: Some(created),
        },
    ))
}

/// The root/1 to root/2 marker upgrade, through verified identities. The old
/// marker is exclusively renamed to a transaction-owned backup and the
/// successor is published without replacement.
fn upgrade_marker(
    context: &Context,
    migration: &Migration,
) -> Result<(MigrationPhase, MigrationPatch), String> {
    let pin = migration
        .marker_pin
        .as_ref()
        .ok_or("the migration has no marker pin")?;
    let root = &migration.root.root;
    let marker_path = root.join(MARKER);
    let backup = root.join(format!("{MARKER}.oxide-old-{}", migration.generation));
    let successor = read_marker(root)
        .ok()
        .filter(|marker| marker.nonce() == migration.root.nonce());
    match fs::symlink_metadata(&marker_path) {
        Ok(meta) if successor.is_some() => {
            // A crash after the successor was published: the pinned old
            // marker must be at its backup (or already journaled there).
            let _ = meta;
            match fs::symlink_metadata(&backup) {
                Ok(backup_meta) => {
                    if backup_meta.dev() != pin.dev || backup_meta.ino() != pin.ino {
                        return Err(pending_migration(
                            migration,
                            "the successor marker is live but its backup is not the pinned marker",
                        ));
                    }
                    if file_digest(&backup)? != pin.sha256 {
                        return Err(pending_migration(
                            migration,
                            "the successor marker's backup bytes changed",
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // A journaled pathname is NOT proof that the required
                    // rollback backup survives: the successor is live and the
                    // pinned old marker is positively gone.
                    return Err(pending_migration(
                        migration,
                        "the successor marker is live and the pinned old-marker backup is gone",
                    ));
                }
                Err(error) => {
                    return Err(pending_migration(
                        migration,
                        &format!("cannot inspect the marker backup: {error}"),
                    ))
                }
            }
            sync_directory(root)?;
            return Ok((
                MigrationPhase::MarkerUpgraded,
                MigrationPatch {
                    stage: None,
                    marker_backup: Some(backup),
                    created_members: None,
                },
            ));
        }
        Ok(meta) => {
            if meta.dev() != pin.dev || meta.ino() != pin.ino {
                return Err(pending_migration(
                    migration,
                    "the root marker's identity changed",
                ));
            }
            if file_digest(&marker_path)? != pin.sha256 {
                return Err(pending_migration(
                    migration,
                    "the root marker's bytes changed",
                ));
            }
            rename_no_replace(&marker_path, &backup).map_err(|error| {
                format!(
                    "cannot move the legacy owner marker to its backup: {error}; nothing was replaced"
                )
            })?;
            sync_directory(root)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A crash after the rename and before the successor was
            // published: the exact pinned backup is the only acceptable
            // reason for a missing marker.
            match fs::symlink_metadata(&backup) {
                Ok(meta) if meta.dev() == pin.dev && meta.ino() == pin.ino => {
                    // The pinned inode must still carry the pinned bytes.
                    let digest = file_digest(&backup).map_err(|error| {
                        pending_migration(
                            migration,
                            &format!("cannot digest the old marker backup: {error}"),
                        )
                    })?;
                    if digest != pin.sha256 {
                        return Err(pending_migration(
                            migration,
                            "the old marker backup bytes changed",
                        ));
                    }
                }
                _ => {
                    return Err(pending_migration(
                        migration,
                        "the old root marker and its backup are both absent",
                    ))
                }
            }
        }
        Err(error) => {
            return Err(pending_migration(
                migration,
                &format!("cannot inspect the root marker: {error}"),
            ))
        }
    }
    // Publish the root/2 marker without replacement, preserving owner,
    // recipe, kind, volume and the ORIGINAL root-creation generation.
    let current = match read_marker(root) {
        Ok(current) => current,
        Err(_) => {
            // The marker path is empty because this step renamed the old one
            // away; publish the successor now.
            let bytes = fs::read(&backup).map_err(|error| {
                pending_migration(
                    migration,
                    &format!("cannot read the old marker backup: {error}"),
                )
            })?;
            let before: Marker = serde_json::from_slice(&bytes).map_err(|error| {
                pending_migration(
                    migration,
                    &format!("the old marker backup is invalid: {error}"),
                )
            })?;
            // The successor carries the pinned marker's immutable fields
            // exactly, including its ORIGINAL created_ms; only the version and
            // the reserved nonce change.
            publish_successor_marker(context, migration, &before)?
        }
    };
    if current.nonce() != migration.root.nonce() {
        return Err(pending_migration(
            migration,
            "the root carries another nonce than the reserved one",
        ));
    }
    sync_directory(root)?;
    Ok((
        MigrationPhase::MarkerUpgraded,
        MigrationPatch {
            stage: None,
            marker_backup: Some(backup),
            created_members: None,
        },
    ))
}

/// P2: exclusively rename the old whole-target link to its backup.
fn back_up_target(migration: &Migration) -> Result<MigrationPhase, String> {
    // P3: no pre-P4 target rename happens until the pinned old-marker backup is
    // positively present with its recorded identity.
    verify_marker_backup(migration)?;
    match fs::symlink_metadata(&migration.backup) {
        Ok(_) => {
            // An earlier attempt renamed it. Only the exact pinned old link at
            // the recorded backup path may stand in, and the live target must
            // not have reappeared beside it.
            verify_target_backup(migration)?;
            if fs::symlink_metadata(&migration.link).is_ok() {
                return Err(pending_migration(
                    migration,
                    "the target reappeared beside its recorded backup",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let link_meta = fs::symlink_metadata(&migration.link).map_err(|error| {
                pending_migration(
                    migration,
                    &format!("cannot inspect the target link: {error}"),
                )
            })?;
            if !link_meta.file_type().is_symlink()
                || link_meta.dev() != migration.link_dev
                || link_meta.ino() != migration.link_ino
            {
                return Err(pending_migration(
                    migration,
                    "the target link is no longer the pinned symlink",
                ));
            }
            let text = fs::read_link(&migration.link).map_err(|error| {
                pending_migration(migration, &format!("cannot read the target link: {error}"))
            })?;
            if text != migration.link_text {
                return Err(pending_migration(migration, "the target link text changed"));
            }
            rename_no_replace(&migration.link, &migration.backup).map_err(|error| {
                format!(
                    "cannot move the old target link to its backup: {error}; nothing else was changed"
                )
            })?;
            if let Some(parent) = migration.link.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) => {
            return Err(pending_migration(
                migration,
                &format!("cannot inspect the backup: {error}"),
            ))
        }
    }
    Ok(MigrationPhase::BackedUp)
}

/// P3: exclusively rename the staging directory onto `target`. A creator that
/// appeared in between is preserved and leaves the migration pending.
fn swap_in_staging(migration: &Migration) -> Result<MigrationPhase, String> {
    // P3: no staging publication happens until the pinned old-marker backup is
    // positively present with its recorded identity.
    verify_marker_backup(migration)?;
    match fs::symlink_metadata(&migration.link) {
        Ok(_) => {
            // A creator (or an earlier replay) is at the target. It is only
            // acceptable when it is EXACTLY this transaction's staged inode
            // with the staged member set, and the staging path is gone.
            if verify_stage_identity(&migration.link, migration).is_ok()
                && verify_member_links(&migration.link, migration).is_ok()
                && fs::symlink_metadata(&migration.stage).is_err()
            {
                return Ok(MigrationPhase::Swapped);
            }
            return Err(pending_migration(
                migration,
                "the target is occupied by something this migration did not create",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(pending_migration(
                migration,
                &format!("cannot inspect the target: {error}"),
            ))
        }
    }
    verify_member_links(&migration.stage, migration)?;
    // P3: publish only the staging inode the journal recorded at P1. A
    // directory that was replaced but holds identically shaped links is not
    // this transaction's staging directory.
    verify_stage_identity(&migration.stage, migration)?;
    rename_no_replace(&migration.stage, &migration.link).map_err(|error| {
        format!(
            "cannot publish the migrated target {}: {error}; the old link is kept at its backup",
            migration.link.display()
        )
    })?;
    if let Some(parent) = migration.link.parent() {
        sync_directory(parent)?;
    }
    Ok(MigrationPhase::Swapped)
}

/// P4: the COMMIT point. The committed target is verified as a real directory
/// with exactly the member links; the receipt, root marker and member
/// references are committed and are never rolled back afterwards.
fn commit_migration(
    context: &Context,
    control: &Path,
    receipt: Receipt,
    migration: &Migration,
) -> Result<Receipt, String> {
    // P3: the COMMIT is reachable only through this transaction's own staged
    // inode, with the pinned pre-P4 backup of the old link still in place. A
    // lookalike target directory or a lost/substituted backup leaves the
    // migration pending instead of committing.
    verify_stage_identity(&migration.link, migration)?;
    verify_target_backup(migration)?;
    verify_migrated_target(&migration.link, migration)?;
    // P3: while a rollback is still possible, a root/1 upgrade must keep the
    // exact old marker it pinned.
    verify_marker_backup(migration)?;
    let marker = read_marker(&migration.root.root)?;
    check_marker(&marker, &context.workspace, &context.checkout_id)?;
    if marker.nonce() != migration.root.nonce() {
        return Err(pending_migration(
            migration,
            "the root does not carry the migrated nonce",
        ));
    }
    if marker.generation > receipt.generation.saturating_add(1) {
        return Err(pending_migration(
            migration,
            "the root marker is ahead of the receipt",
        ));
    }
    // P4 never commits a link whose member directory is absent. P1 created
    // them; this re-establishes the same invariant under the verified marker
    // immediately before the receipt commits, so no committed placement can
    // dangle.
    for member in &migration.members {
        ensure_member_directory(&migration.root.root, member).map_err(|reason| {
            pending_migration(
                migration,
                &format!(
                    "the committed member directory for {member} cannot be published: {reason}"
                ),
            )
        })?;
    }
    let selected = migration
        .root
        .clone()
        .at(marker.generation)
        .with_nonce(marker.nonce().map(str::to_owned));
    let mut committed = receipt.clone();
    committed.version = PROFILE_RECEIPT_VERSION.to_owned();
    committed.layout = LayoutKind::Profile;
    committed.state = ReceiptState::Committed;
    committed.generation = migration.generation;
    committed.links = migration
        .members
        .iter()
        .map(|member| LinkPlacement {
            member: member.clone(),
            selected: Some(selected.clone()),
            prepared: None,
        })
        .collect();
    committed.roots.retain(|known| known.root != selected.root);
    committed.roots.push(selected.clone());
    let mut journal = migration.clone();
    journal.phase = MigrationPhase::Committed;
    journal.updated_ms = process::now_ms();
    committed.transition = Some(journal);
    committed.updated_ms = process::now_ms();
    write_receipt(control, &committed)?;
    crash_point("migration-p4")?;
    finish_migration_cleanup(context, control, committed, migration)
}

/// Remove the journaled backups after the commit and then drop the journal.
/// Only exact recorded identities are removed; a mismatch keeps it pending.
/// Verify a committed migrated target tolerantly: every recorded member is
/// still this migration's link into the root, while Cargo's own real top-level
/// entries (`CACHEDIR.TAG`, `.rustc_info.json`, `tmp`, tool directories) are
/// Cargo's and are never treated as members.
fn verify_migrated_target(dir: &Path, migration: &Migration) -> Result<(), String> {
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| format!("cannot inspect {}: {error}", dir.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", dir.display()));
    }
    for member in &migration.members {
        let link = dir.join(member);
        let target = fs::read_link(&link)
            .map_err(|error| format!("cannot read {}: {error}", link.display()))?;
        if target != member_destination(&migration.root.root, member) {
            return Err(format!(
                "{} selects {}, not its migrated member destination",
                link.display(),
                target.display()
            ));
        }
    }
    Ok(())
}

fn finish_migration_cleanup(
    context: &Context,
    control: &Path,
    receipt: Receipt,
    migration: &Migration,
) -> Result<Receipt, String> {
    // The committed target must still be the migrated real directory with its
    // member links; Cargo's real top-level files are expected and ignored.
    verify_migrated_target(&migration.link, migration)?;
    match fs::symlink_metadata(&migration.backup) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(meta) => {
            if !meta.file_type().is_symlink()
                || meta.dev() != migration.link_dev
                || meta.ino() != migration.link_ino
            {
                return Err(pending_migration(
                    migration,
                    "the backup is not the pinned old link",
                ));
            }
            let text = fs::read_link(&migration.backup)
                .map_err(|error| format!("cannot read the backup link: {error}"))?;
            if text != migration.link_text {
                return Err(pending_migration(migration, "the backup link text changed"));
            }
            fs::remove_file(&migration.backup)
                .map_err(|error| format!("cannot remove the migration backup: {error}"))?;
            if let Some(parent) = migration.link.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect the migration backup {}: {error}",
                migration.backup.display()
            ))
        }
    }
    if let Some(backup) = &migration.marker_backup {
        remove_marker_backup(&migration.root.root, backup, migration)?;
    } else if let Some(pin) = &migration.marker_pin {
        // A crash may have left the marker backup without its journal record.
        let candidate = migration
            .root
            .root
            .join(format!("{MARKER}.oxide-old-{}", migration.generation));
        remove_marker_backup(&migration.root.root, &candidate, migration)?;
        let _ = pin;
    }
    let mut finished = receipt.clone();
    finished.transition = None;
    finished.updated_ms = process::now_ms();
    if receipt.transition.is_some() {
        write_receipt(control, &finished)?;
    }
    let _ = context;
    Ok(finished)
}

fn remove_marker_backup(root: &Path, backup: &Path, migration: &Migration) -> Result<(), String> {
    let pin = migration
        .marker_pin
        .as_ref()
        .ok_or("the migration has no marker pin")?;
    match fs::symlink_metadata(backup) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(meta) => {
            if meta.dev() != pin.dev || meta.ino() != pin.ino {
                return Err(pending_migration(
                    migration,
                    "the marker backup is not the pinned marker",
                ));
            }
            if file_digest(backup)? != pin.sha256 {
                return Err(pending_migration(
                    migration,
                    "the marker backup's bytes changed",
                ));
            }
            fs::remove_file(backup)
                .map_err(|error| format!("cannot remove the marker backup: {error}"))?;
            sync_directory(root)
        }
        Err(error) => Err(format!("cannot inspect the marker backup: {error}")),
    }
}

/// Restore the exact pre-migration state for an uncommitted journal (the
/// debug-only rollback oracle). The old marker is restored from its pinned
/// backup, the old target link returns, and the receipt returns to its
/// published /2 form; the reserved nonce was never published, so it retires
/// with the journal.
fn rollback_migration(
    context: &Context,
    control: &Path,
    receipt: Receipt,
    migration: &Migration,
) -> Result<Receipt, String> {
    // A committed target is never rolled back.
    if migration.phase == MigrationPhase::Committed {
        return Err("a committed layout migration is never rolled back".to_owned());
    }
    // P3: no rollback effect happens before the recorded parent is verified,
    // so nothing is removed or renamed beneath a directory this transaction
    // did not pin.
    let parent = migration
        .link
        .parent()
        .ok_or("the migration journal names no parent")?;
    let parent_meta = fs::symlink_metadata(parent)
        .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
    if parent_meta.dev() != migration.parent_dev || parent_meta.ino() != migration.parent_ino {
        return Err(pending_migration(
            migration,
            "the parent directory changed identity",
        ));
    }
    // 1. If the staging directory was swapped in, remove exactly it: only the
    // inode this transaction created and staged is ever removed here.
    match fs::symlink_metadata(&migration.link) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                // The old link is still (or again) in place.
            } else if verify_stage_identity(&migration.link, migration).is_ok()
                && verify_member_links(&migration.link, migration).is_ok()
            {
                for member in &migration.members {
                    let link = migration.link.join(member);
                    fs::remove_file(&link).map_err(|error| {
                        format!("cannot remove the migrated member link: {error}")
                    })?;
                }
                fs::remove_dir(&migration.link).map_err(|error| {
                    format!(
                        "cannot remove the migrated target directory {}: {error}",
                        migration.link.display()
                    )
                })?;
            } else {
                return Err(pending_migration(
                    migration,
                    "the target is neither the old link nor the migrated directory",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect the target: {error}")),
    }
    // 2. The backup link goes back to the target.
    match fs::symlink_metadata(&migration.backup) {
        Ok(meta) => {
            if !meta.file_type().is_symlink()
                || meta.dev() != migration.link_dev
                || meta.ino() != migration.link_ino
            {
                return Err(pending_migration(
                    migration,
                    "the backup is not the pinned old link",
                ));
            }
            if fs::symlink_metadata(&migration.link).is_ok() {
                return Err(pending_migration(
                    migration,
                    "the target appeared again while rolling back",
                ));
            }
            rename_no_replace(&migration.backup, &migration.link)
                .map_err(|error| format!("cannot restore the old target link: {error}"))?;
            if let Some(parent) = migration.link.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect the backup: {error}")),
    }
    // 3. The old marker returns from its pinned backup.
    if let Some(backup) = migration.marker_backup.clone().or_else(|| {
        migration.marker_pin.as_ref().map(|_| {
            migration
                .root
                .root
                .join(format!("{MARKER}.oxide-old-{}", migration.generation))
        })
    }) {
        let pin = migration
            .marker_pin
            .as_ref()
            .ok_or("the migration has no marker pin")?;
        if let Ok(meta) = fs::symlink_metadata(&backup) {
            if meta.dev() != pin.dev || meta.ino() != pin.ino || file_digest(&backup)? != pin.sha256
            {
                return Err(pending_migration(
                    migration,
                    "the marker backup is not the pinned marker",
                ));
            }
            let marker_path = migration.root.root.join(MARKER);
            if fs::symlink_metadata(&marker_path).is_ok() {
                let current = read_marker(&migration.root.root)
                    .map_err(|reason| pending_migration(migration, &reason))?;
                if current.nonce() != migration.root.nonce() {
                    return Err(pending_migration(
                        migration,
                        "the live marker is not the prepared successor",
                    ));
                }
                fs::remove_file(&marker_path).map_err(|error| {
                    format!("cannot remove the prepared marker before rollback: {error}")
                })?;
            }
            rename_no_replace(&backup, &marker_path)
                .map_err(|error| format!("cannot restore the old marker: {error}"))?;
            sync_directory(&migration.root.root)?;
        } else {
            // No backup: either the marker was never touched (still root/1) or
            // it is unknowable.
            match read_marker(&migration.root.root) {
                Ok(marker) if marker.is_legacy() => {}
                _ => {
                    return Err(pending_migration(
                        migration,
                        "the old marker and its backup are both absent",
                    ))
                }
            }
        }
    }
    // 4. The staging directory is removed only when it is exactly the
    //    journal's member links.
    if fs::symlink_metadata(&migration.stage).is_ok() {
        verify_member_links(&migration.stage, migration)?;
        for member in &migration.members {
            fs::remove_file(migration.stage.join(member))
                .map_err(|error| format!("cannot remove a staged member link: {error}"))?;
        }
        fs::remove_dir(&migration.stage)
            .map_err(|error| format!("cannot remove the staging directory: {error}"))?;
        sync_directory(&context.workspace)?;
    }
    // 4b. The member directories this migration created inside the root go
    //     back to absent, so the pre-migration state is restored exactly: only
    //     the recorded ones, and only while each is still the real, empty
    //     directory the migration made. A directory holding content, a link a
    //     creator put there, or one the migration found already present is
    //     kept untouched.
    let mut removed = false;
    for member in migration.created_members.iter().rev() {
        let destination = member_destination(&migration.root.root, member);
        let empty_real_directory = fs::symlink_metadata(&destination).is_ok_and(|metadata| {
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && fs::read_dir(&destination)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(false)
        });
        if empty_real_directory {
            fs::remove_dir(&destination).map_err(|error| {
                format!(
                    "cannot remove the migration's member directory {}: {error}",
                    destination.display()
                )
            })?;
            removed = true;
        }
    }
    if removed {
        sync_directory(&migration.root.root)?;
    }
    // 5. The receipt returns to its published /2 form with the whole-target
    //    placement selected and no journal.
    let mut restored = receipt.clone();
    restored.version = RECEIPT_VERSION.to_owned();
    restored.layout = LayoutKind::WholeTarget;
    restored.state = ReceiptState::Committed;
    restored.transition = None;
    let link_text = migration.link_text.clone();
    let root = migration.root.clone().with_nonce(
        read_marker(&migration.root.root)
            .ok()
            .and_then(|marker| marker.nonce),
    );
    let _ = link_text;
    if restored.links.is_empty() {
        restored.links.push(LinkPlacement {
            member: MEMBER.to_owned(),
            selected: Some(root),
            prepared: None,
        });
    } else {
        restored.links.truncate(1);
        restored.links[0] = LinkPlacement {
            member: MEMBER.to_owned(),
            selected: Some(root),
            prepared: None,
        };
    }
    restored.updated_ms = process::now_ms();
    write_receipt(control, &restored)?;
    Ok(restored)
}

/// `oxide outputs adopt` for the bound profile layout: `target` stays a real
/// directory and every positively identified member becomes an owned link into
/// one root. A real nonempty member directory is never moved or converted; an
/// empty one converts only here.
fn adopt_profile(context: &Context, receipt: Receipt) -> Result<(Receipt, Selected, bool), String> {
    adopt_profile_from(context, receipt, Start::Adopt, None)
}

/// The ONE profile-layout creator, shared by the explicit `oxide outputs adopt`
/// and by the automatic first conversion of a fresh checkout on the permanent
/// drive (oxide#160, S4b). `start` is the only difference: explicit adoption
/// may convert an empty real directory (and relink a stale one), while the
/// automatic conversion creates its members only where nothing exists.
fn adopt_profile_from(
    context: &Context,
    receipt: Receipt,
    start: Start,
    captured: Option<&RootSpec>,
) -> Result<(Receipt, Selected, bool), String> {
    // The members this conversion establishes: the host debug/release pair
    // plus every member the receipt already places, sorted and prefix-free.
    let mut members = vec!["debug".to_owned(), "release".to_owned()];
    for placement in receipt.links() {
        members.push(placement.member().to_owned());
    }
    let parsed = members
        .iter()
        .map(|name| crate::layout::Member::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    let declaration = crate::layout::Declaration::new(parsed)?;
    let target = context.target()?;
    let action = if start == Start::Adopt {
        "outputs adopt"
    } else {
        "auto-adopt"
    };
    match &target {
        TargetState::Missing => {
            // The first conversion's ignore prerequisite (oxide#125) is checked
            // for EVERY member link it is about to publish before the real
            // target directory exists, so a refused conversion changes nothing
            // at all (oxide#152's rule). `transition_at` rechecks each one
            // under its own prepared receipt.
            for member in declaration.members() {
                refuse_link_git_exposure_at(
                    context,
                    &link_path(&context.workspace, member.name()),
                    Some(member.name()),
                )?;
            }
            // The lexical CARGO_TARGET_DIR stays real. Adoption creates it so
            // Cargo's own top-level files land in a real directory.
            create_owned_directory(&context.link())?;
            fs::set_permissions(&context.link(), fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("cannot set permissions on the target: {error}"))?;
            sync_directory(&context.workspace)?;
        }
        TargetState::RealEmpty | TargetState::RealNonEmpty if start == Start::Adopt => {}
        TargetState::OwnedLink(root) => {
            return Err(format!(
                "{action} refused: {} is an owned whole-target link to {}; it is kept exactly as it is. Migrating a whole-target checkout to profile members is a separate journaled transition and is refused by this build",
                context.link().display(),
                root.display()
            ))
        }
        TargetState::Dangling(root) => {
            return Err(format!(
                "{action} refused: {} links to {}, which is not available; it is left untouched",
                context.link().display(),
                root.display()
            ))
        }
        TargetState::Foreign => {
            return Err(format!(
                "{action} refused: {} is {}; Oxide never replaces it",
                context.link().display(),
                target.label()
            ))
        }
        // The automatic first conversion creates its members only where
        // nothing exists: an empty or populated real directory appearing where
        // `target` was positively absent is not adopted, converted or kept.
        state => {
            return Err(format!(
                "{action} refused: {} is {}; an automatic first conversion creates no member where anything exists. Nothing was changed. Run `oxide outputs adopt` yourself if you accept converting an empty directory",
                context.link().display(),
                state.label()
            ))
        }
    }
    // The explicit adoption decides its own place from the live registration.
    // The automatic conversion uses exactly the root it captured and verified
    // twice before this point, so it can never be redirected to the
    // checkout-local fallback behind the owner's back.
    let desired = match start {
        Start::MissingOnly => None,
        Start::Any | Start::Adopt => desired_selection(context, &receipt)?,
    };
    let spec = match (start, captured) {
        (Start::MissingOnly, Some(spec)) => spec.clone(),
        (Start::MissingOnly, None) => {
            return Err(
                "outputs: auto-adopt has no captured root selection; nothing was changed"
                    .to_owned(),
            )
        }
        (_, _) => {
            let desired = match &desired {
                Some(desired) => desired.clone(),
                None => {
                    let usable = usable_external(context);
                    let spec = match &usable {
                        Ok(registration) => external_spec(registration, &context.workspace),
                        Err(_) => local_spec(&context.workspace),
                    };
                    spec.selected(receipt.generation.saturating_add(1))
                }
            };
            spec_for(&desired)
        }
    };
    let wanted = match &desired {
        Some(desired) => Some(desired.clone()),
        None => Some(spec.selected(receipt.generation.saturating_add(1))),
    };
    let mut committed = receipt;
    let mut converted = false;
    let mut kept = Vec::new();
    for member in declaration.members() {
        let link = link_path(&context.workspace, member.name());
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        match state {
            // A real nonempty member is kept local, but only an explicit
            // adoption may do that; the automatic conversion never leaves a
            // half-converted member behind.
            TargetState::RealNonEmpty if start == Start::Adopt => {
                kept.push(member.name().to_owned());
                continue;
            }
            TargetState::RealEmpty if start == Start::Adopt => {}
            TargetState::Missing => {}
            TargetState::OwnedLink(root)
                if start == Start::Adopt
                    && wanted.as_ref().is_some_and(|desired| {
                        root == member_destination(&desired.root, member.name())
                            && committed
                                .selected_for(member.name())
                                .is_some_and(|selected| selected.same_place(desired))
                    }) =>
            {
                // Already the desired destination this adoption would select:
                // idempotent, nothing is written. A link that still matches an
                // older receipt but not the desired place is relinked below.
                continue;
            }
            TargetState::OwnedLink(_) | TargetState::Dangling(_) if start == Start::Adopt => {}
            TargetState::Foreign => {
                return Err(format!(
                    "{action} refused: {} is {}; it is never followed or replaced",
                    link.display(),
                    state.label()
                ))
            }
            state => {
                return Err(format!(
                    "{action} refused: {} is {}; an automatic first conversion creates no member where anything exists. Nothing was changed. Run `oxide outputs adopt` yourself if you accept converting an empty directory",
                    link.display(),
                    state.label()
                ))
            }
        }
        committed = transition_at(
            &context,
            &committed,
            member.name(),
            &spec,
            start,
            NonceAuthority::Unkeyed,
        )?;
        converted = true;
    }
    let selected = match committed.selected().cloned().or(desired) {
        Some(selected) => selected,
        None => return Err("the adoption committed no root".to_owned()),
    };
    if converted {
        ensure_root(context, &spec, committed.generation)?;
    }
    if !kept.is_empty() {
        eprintln!(
            "oxide: profile outputs: {} stay real directories under {} (an existing nonempty directory is never moved or converted)",
            kept.join(", "),
            context.link().display()
        );
    }
    Ok((committed, selected, converted))
}

fn refuse_existing_target(context: &Context, target: &TargetState) -> String {
    let link = context.link();
    refuse_existing_target_at(target, &link)
}

fn refuse_existing_target_at(target: &TargetState, link: &Path) -> String {
    format!(
        "outputs adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed. {}",
        link.display(),
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

/// The member links an automatic first conversion publishes: `debug` and
/// `release` (`adopt_profile_from`'s declaration for a fresh receipt).
const AUTO_ADOPT_MEMBERS: [&str; 2] = ["debug", "release"];

/// Automatic first conversion (oxide#121), run once per leased command before
/// its destination is planned, its key is built or its slot is taken. The
/// command's own selection must be a removable permanent one, and the volume
/// in force now must be the one that selection captured. A checkout with no
/// receipt of any state and a positively absent target then converts under the
/// same barrier, fences, admission lock and receipt as `oxide outputs adopt`,
/// with the missing-target-only transition. Any other state is left to the
/// existing paths untouched. A metadata error refuses; it is never read as
/// absence.
///
/// The first conversion's ignore prerequisite decides *eligibility*, not the
/// command (oxide#154): when Git cannot confirm a link path this conversion
/// would publish is ignored by a rule that covers it, the command is an
/// ordinary unconverted checkout. Nothing is written — no receipt, root,
/// link, notice or downgrade fence — so the build keeps its real local
/// target and the binding prints its one unconverted warning. An explicit
/// `oxide outputs adopt` still refuses with the exact fix, and the transition
/// rechecks the same prerequisite before its first effect, so a concurrent
/// change refuses rather than publishing a link Git does not cover.
///
/// Since oxide#160 the first conversion creates the profile layout, so
/// eligibility is asked for every member link it would publish
/// (`adopt_profile_from`'s declaration for a fresh receipt), exactly as that
/// creator checks them before it creates the real `target` directory. Asking
/// the member paths also keeps a directory-only rule such as `target/`
/// eligible, because it genuinely covers `target/<member>`.
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
    // Expected automatic-adoption ineligibility must leave the build usable
    // (oxide#154). The prerequisite is asked before the barrier, the
    // downgrade fence or the admission lock, so an ineligible checkout gets
    // no side effect at all: it is exactly the checkout this command found.
    // It asks every member link the profile-layout first conversion would
    // publish (oxide#160), the same set `adopt_profile_from` checks before it
    // creates the real target directory.
    for member in AUTO_ADOPT_MEMBERS {
        if first_link_git_answer(&context, Some(member)).verdict != LinkGitVerdict::Eligible {
            return Ok(());
        }
    }
    let mut waits = WaitAccounting::default();
    let guard = barrier(store, checkout, &mut waits)?;
    let outcome: Result<Option<Selected>, String> = (|| {
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
        // The first conversion creates the profile layout through the SAME
        // creator an explicit `oxide outputs adopt` uses, with the
        // missing-target-only start this path has always required (oxide#160,
        // S4b): a real `target` directory holding an owned `debug`/`release`
        // pair into one new owned root, and a receipt and root marker
        // identical in form to a fresh explicit adoption.
        let receipt = Receipt::first_with(checkout, &context.checkout_id, LayoutKind::Profile);
        let (_committed, selected, _converted) =
            adopt_profile_from(&context, receipt, Start::MissingOnly, Some(&spec))?;
        Ok(Some(selected))
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
    detach_at(&context, output_store)
}

/// `oxide outputs detach` against one already-opened context and a **lazy**
/// pool provider, so the guard's effect-free fast precheck runs before the pool
/// is opened: a guarded workspace refuses a `Prepared` transition, an unsettled
/// prune intent or a listed root with literally nothing done.
fn detach_at(
    context: &Context,
    pool: impl FnOnce() -> Result<Option<Store>, String>,
) -> Result<u8, String> {
    guarded_command_precheck(context, "outputs detach refused")?;
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = pool()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let internal = &internal;
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    // A guarded workspace takes the same barrier, but through the guarded
    // order: receipt and migration journal revalidated under the checkout
    // binding mutex before any mark exists, then a bounded non-blocking
    // `Barrier`-mode acquisition (design §4.1 steps 3-5, §4.5). A normal
    // workspace keeps the shipped order byte for byte.
    let guarded = guarded_workspace(Some(&control), &context.workspace).is_some();
    let guard = if guarded {
        guarded_barrier(internal, context, &mut waits)?
    } else {
        barrier(internal, &context.workspace, &mut waits)?
    };
    let outcome = (|| {
        // Belt and braces: a `Prepared` transition that appeared while the
        // barrier was taken is a custody defect, so the verb stops before its
        // effects rather than replaying (design §4.1 step 6).
        if guarded {
            guarded_prepared_refusal(&context)?;
        }
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
        if receipt.layout() == LayoutKind::Profile {
            return detach_profile(&context, &control, receipt);
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

/// `oxide outputs detach` for the profile layout: every owned member link
/// becomes a real empty directory again and `target` stays real. No root is
/// moved or deleted; the receipt keeps its identity and known roots.
fn detach_profile(
    context: &Context,
    control: &Path,
    receipt: Receipt,
) -> Result<(Receipt, bool), String> {
    if receipt.state != ReceiptState::Committed {
        return Err(
            "outputs detach: an interrupted transition is pending; nothing was changed".to_owned(),
        );
    }
    let mut removals = Vec::new();
    for placement in receipt.links() {
        let Some(selected) = placement.selected_ref() else {
            continue;
        };
        let link = link_path(&context.workspace, placement.member());
        match link_state(&link, &context.workspace, &context.checkout_id)? {
            TargetState::Missing => {}
            TargetState::OwnedLink(root)
                if root == member_destination(selected.root(), placement.member()) =>
            {
                removals.push(link);
            }
            state => {
                return Err(format!(
                    "outputs detach refused: {} is {}, not this checkout's owned link to {}; it is left untouched",
                    link.display(),
                    state.label(),
                    selected.root.display()
                ))
            }
        }
    }
    match context.target()? {
        TargetState::Missing => create_owned_directory(&context.link())?,
        TargetState::RealEmpty | TargetState::RealNonEmpty => {}
        state => {
            return Err(format!(
                "outputs detach refused: {} is {}; it is left untouched",
                context.link().display(),
                state.label()
            ))
        }
    }
    for link in &removals {
        fs::remove_file(link)
            .map_err(|error| format!("cannot remove the owned link {}: {error}", link.display()))?;
        create_owned_directory(link)?;
        fs::set_permissions(link, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot set permissions on {}: {error}", link.display()))?;
    }
    sync_directory(&context.workspace)?;
    let mut previous = Vec::new();
    for placement in receipt.links() {
        if let Some(selected) = placement.selected_ref() {
            previous.push(selected.clone());
        }
    }
    let mut detached = receipt.clone();
    for placement in &mut detached.links {
        placement.selected = None;
        placement.prepared = None;
    }
    for root in previous {
        remember_root(&mut detached, root);
    }
    detached.state = ReceiptState::Detached;
    detached.updated_ms = process::now_ms();
    write_receipt(control, &detached)?;
    Ok((detached, true))
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
///
/// `repair_absent_members` is set by a derived `oxide exec` only (oxide#160's
/// member inference, oxide#162): a receipt-recorded member link whose one
/// defect is an absent destination directory beneath the committed, verified
/// root has that directory created under this checkout's custody. Every other
/// command keeps the existing refusal, so a dangling member link never
/// transitions automatically.
pub fn bind_for_command(
    store: &Store,
    checkout: &Path,
    own_record: u64,
    expected: Option<&Plan>,
    pool_fallback: Option<&str>,
    repair_absent_members: bool,
    guarded: Option<&LocalOutputs<'_>>,
) -> Result<OutputBinding, String> {
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        checkout.to_path_buf(),
    )
    .with_pool_fallback(pool_fallback);
    // A guarded workspace owns no output root, so none of the owned-output
    // machinery below applies to it: no prune intent, no drive mode, no
    // receipt transition, no link. The authoritative decision is taken under
    // the checkout binding mutex in `bind_local_outputs`, and the command
    // starts with a real local `target`.
    if let Some(local) = guarded {
        return bind_local_outputs(store, &context, expected, local);
    }
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
        match bind_under_binding(
            &context,
            store,
            own_record,
            &receipt,
            expected,
            repair_absent_members,
        ) {
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
fn keyed_state_matches(receipt: Option<&Receipt>, expected: Option<&Plan>) -> Result<(), String> {
    let converted = receipt.is_some_and(|receipt| receipt.state != ReceiptState::Detached);
    keyed_state_refusal(converted, expected)
}

/// The shipped key mismatch refusal for one already-decided `converted` fact,
/// so a guarded binding can use the same text with its own notion of a receipt
/// that selects nothing (design §4.2's `Pruned` row).
fn keyed_state_refusal(converted: bool, expected: Option<&Plan>) -> Result<(), String> {
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
fn bind_ephemeral(context: &Context, expected: Option<&Plan>) -> Result<Outcome, String> {
    let expected_selected = expected.and_then(Plan::selected);
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
            if receipt.layout() == LayoutKind::Profile {
                // Ephemeral mode never relinks a converted checkout. The
                // profile layout is validated exactly as keyed: each owned
                // member link selects the receipt's root and each real member
                // stays real.
                return bind_profile_sealed(context, &receipt, expected);
            }
            let wanted = match expected_selected {
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

/// Ephemeral-mode binding for a converted profile checkout: no member is
/// created or relinked, so every keyed placement must already be live.
fn bind_profile_sealed(
    context: &Context,
    receipt: &Receipt,
    expected: Option<&Plan>,
) -> Result<Outcome, String> {
    let Some(plan) = expected else {
        return Err(
            "outputs: this checkout places outputs at profile members but its slot was keyed with no owned destination; run the command again"
                .to_owned(),
        );
    };
    if plan.whole_target {
        return Err(
            "outputs: this checkout's layout changed to profile members while its slot was keyed for the whole target; nothing was built. Run the command again"
                .to_owned(),
        );
    }
    let mut outcome = Outcome::default();
    let mut real = Vec::new();
    for planned in &plan.members {
        let link = link_path(&context.workspace, &planned.member);
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        match (&planned.selected, &state) {
            (Some(wanted), TargetState::OwnedLink(root))
                if root == &member_destination(&wanted.root, &planned.member) =>
            {
                check_bound_root(context, receipt.generation, wanted)?;
                outcome.root.get_or_insert_with(|| wanted.root.clone());
            }
            (None, TargetState::RealEmpty | TargetState::RealNonEmpty) => {
                real.push(planned.member.clone());
            }
            _ => {
                return Err(format!(
                    "outputs: {} is {}, not the keyed placement; the drive mode is ephemeral, so Oxide will not relink it. Run `oxide outputs adopt` or `oxide outputs detach`",
                    link.display(),
                    state.label()
                ))
            }
        }
    }
    if let Some(root) = outcome.root.clone() {
        outcome.note = Some(format!(
            "owned outputs {} through profile member links",
            root.display()
        ));
    }
    if !real.is_empty() {
        let list = real.join(", ");
        outcome.warnings.push(format!(
            "profile outputs: {list} stay real directories and are used locally; only an explicit `oxide outputs adopt` converts an empty one"
        ));
        if outcome.note.is_none() {
            outcome.note = Some(format!("real profile directories kept ({list})"));
        }
    }
    Ok(outcome)
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
    expected: Option<&Plan>,
    repair_absent_members: bool,
) -> Bind {
    if receipt.layout() == LayoutKind::Profile {
        return bind_profile(
            context,
            store,
            own_record,
            receipt,
            expected,
            repair_absent_members,
        );
    }
    let expected = expected.and_then(Plan::selected);
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
            // The slot's own keyed nonce is the only future-root authority a
            // leased command may bind (oxide#152).
            let committed = match transition(
                context,
                receipt,
                &spec_for(&wanted),
                Start::Any,
                NonceAuthority::Keyed(wanted.nonce()),
            ) {
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

/// Bind a profile-layout checkout's declared members under its binding
/// mutex. Every member either keeps the owned link its slot was keyed with, is
/// linked to the keyed root now (under positive custody closure), or is a real
/// profile directory this command keeps local. Anything else changed since the
/// key was built and refuses before Cargo.
fn bind_profile(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: &Receipt,
    expected: Option<&Plan>,
    repair_absent_members: bool,
) -> Bind {
    // A detached profile receipt owns no destination: the command runs locally
    // through real directories, exactly as the legacy detached state does
    // (oxide#160, S1). Nothing is planned, so nothing may be keyed as owned,
    // and a link at `target` is still never followed.
    if receipt.state == ReceiptState::Detached {
        if expected.is_some_and(|plan| !plan.members.is_empty()) {
            return Bind::Refuse(
                "outputs: the receipt says this checkout is detached but its slot was keyed for an owned root; nothing was built. Run the command again"
                    .to_owned(),
            );
        }
        return match refuse_unowned_link(context, "outputs") {
            Err(reason) => Bind::Refuse(reason),
            Ok(()) => Bind::Bound(Outcome {
                warnings: vec![
                    "permanent drive mode: this checkout is detached from owned outputs; `oxide outputs adopt` converts it again. Nothing was moved"
                        .to_owned(),
                ],
                note: Some("outputs detached".to_owned()),
                root: None,
            }),
        };
    }
    let Some(plan) = expected else {
        return Bind::Refuse(
            "outputs: this checkout places outputs at profile members but this command's slot was keyed with no owned destination; nothing was built. Run the command again"
                .to_owned(),
        );
    };
    if plan.whole_target {
        return Bind::Refuse(
            "outputs: this checkout changed to profile members while this command waited, so its slot was keyed for the whole target; nothing was built. Run the command again"
                .to_owned(),
        );
    }
    if receipt.state != ReceiptState::Committed {
        return Bind::Refuse(
            "outputs: this checkout's profile receipt selects no root (it is detached or pending); run `oxide outputs adopt` or the command again"
                .to_owned(),
        );
    }
    let mut needed: Vec<(String, Selected)> = Vec::new();
    let mut absent: Vec<(String, Selected)> = Vec::new();
    let mut real: Vec<String> = Vec::new();
    let mut owned_root: Option<PathBuf> = None;
    // oxide#165: the profile layout keeps `<checkout>/target` itself a real
    // directory, and its owned member links are published inside it. A
    // committed profile receipt whose target directory was deleted (a manual
    // `rm -rf target`, a task-local build-output cleanup) must never be a
    // checkout whose every command fails with ENOENT from the transition link:
    // the binding recreates that one real directory exclusively before the
    // first missing-member transition publishes into it. Anything else at that
    // path — a link of any kind, a file or another type — still refuses and is
    // never replaced or followed.
    let recreate_target = match context.target() {
        Ok(TargetState::RealEmpty | TargetState::RealNonEmpty) => false,
        Ok(TargetState::Missing) => true,
        Ok(state) => {
            return Bind::Refuse(format!(
                "outputs: this checkout places outputs at profile members inside a real target directory, but {} is {}; it is never replaced or followed. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                context.link().display(),
                state.label()
            ))
        }
        Err(reason) => return Bind::Refuse(reason),
    };
    for planned in &plan.members {
        let link = link_path(&context.workspace, &planned.member);
        let state = match link_state(&link, &context.workspace, &context.checkout_id) {
            Ok(state) => state,
            Err(reason) => return Bind::Refuse(reason),
        };
        // P4: this checkout's committed receipt still places the member on an
        // EXTERNAL root, but this command's fresh plan does not (a still-mounted
        // volume became unusable after the slot was keyed). Relinking,
        // recreating or linking an absent member under an internal root would be
        // an automatic external -> local transition behind the owner's back, so
        // it refuses before Cargo whatever the live link looks like and whether
        // or not the intermediate slot fell back. Only the explicit
        // `oxide outputs adopt` may change the recorded place.
        if let Some(committed) = receipt
            .links
            .iter()
            .find(|placement| placement.member == planned.member && placement.selected.is_some())
            .and_then(|placement| placement.selected.as_ref())
        {
            let planned_external = planned
                .selected
                .as_ref()
                .is_some_and(|wanted| wanted.kind == RootKind::External);
            if committed.kind == RootKind::External && !planned_external {
                let now = planned.selected.as_ref().map_or_else(
                    || "a real profile directory".to_owned(),
                    |wanted| {
                        member_destination(&wanted.root, &planned.member)
                            .display()
                            .to_string()
                    },
                );
                // Name the fallback reason too when this command's own
                // intermediate slot came from the internal pool, so the
                // refusal states both why the slot is internal and why the
                // member is not moved there.
                let fallback = context
                    .pool_fallback
                    .as_deref()
                    .map(|reason| {
                        format!(
                            "; this command's intermediate slot fell back to the internal pool ({reason})"
                        )
                    })
                    .unwrap_or_default();
                return Bind::Refuse(format!(
                    "outputs: {} keeps the external member root {} from this checkout's committed receipt, but this command would place it at {now}{fallback}; Oxide never moves an owned external output to another pool automatically, so nothing was built. Restore the drive, or run `oxide outputs adopt` to recheck this checkout's owned outputs",
                    link.display(),
                    member_destination(&committed.root, &planned.member).display()
                ));
            }
        }
        match (&planned.selected, &state) {
            (Some(wanted), TargetState::OwnedLink(root))
                if root == &member_destination(&wanted.root, &planned.member) =>
            {
                // The legacy #117 rule: an internal-pool fallback slot must
                // never write through an unsupervised external owned link.
                if wanted.kind == RootKind::External {
                    if let Some(reason) = context.pool_fallback.as_deref() {
                        return Bind::Refuse(format!(
                            "outputs: this command's intermediate slot fell back to the internal pool ({reason}) while {} selects the external member root {}; the drive's removal is not supervised, so nothing was built. Restore the drive or run `oxide outputs adopt`",
                            link_path(&context.workspace, &planned.member).display(),
                            wanted.root.display()
                        ));
                    }
                }
                if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                    return Bind::Refuse(format!(
                        "outputs: {reason}; nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                    ));
                }
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Missing) => {
                if let Err(reason) = member_root_authority(context, receipt, wanted) {
                    return Bind::Refuse(reason);
                }
                needed.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::OwnedLink(_)) => {
                needed.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Dangling(destination))
                if repair_absent_members
                    && destination == &member_destination(&wanted.root, &planned.member) =>
            {
                // A derived `oxide exec` only (oxide#162): the link selects
                // exactly this member's destination under the root this
                // command's slot was keyed for, and the only defect is that the
                // member directory inside that verified root is absent (the
                // shape an older migration left: the link was published before
                // its member directory existed). The directory is created under
                // the root's own verified identity and this checkout's writer
                // custody; nothing is relinked, replaced or followed. A
                // missing, unverified or foreign root fails `check_bound_root`
                // here and refuses below, and every other command keeps the
                // existing never-transition-automatically refusal.
                if wanted.kind == RootKind::External {
                    if let Some(reason) = context.pool_fallback.as_deref() {
                        return Bind::Refuse(format!(
                            "outputs: this command's intermediate slot fell back to the internal pool ({reason}) while {} selects the external member root {}; the drive's removal is not supervised, so nothing was built. Restore the drive or run `oxide outputs adopt`",
                            link.display(),
                            wanted.root.display()
                        ));
                    }
                }
                if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                    return Bind::Refuse(format!(
                        "outputs: {} links to {}, whose owned root {} is not verified ({reason}); an absent member directory is created only under this checkout's verified, committed root. Nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                        link.display(),
                        destination.display(),
                        wanted.root.display()
                    ));
                }
                absent.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Dangling(_)) => {
                // Never transition automatically through a dangling link.
                // With an internal-pool fallback slot this is the #117 refusal;
                // otherwise the owner must run the explicit adopt.
                let fallback = context
                    .pool_fallback
                    .as_deref()
                    .map(|reason| format!("; this command's intermediate slot fell back to the internal pool ({reason})"))
                    .unwrap_or_default();
                return Bind::Refuse(format!(
                    "outputs: {} selects the member root {} which is not available now{fallback}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                    link.display(),
                    wanted.root.display()
                ));
            }
            (Some(_), _) => {
                return Bind::Refuse(format!(
                    "outputs: {} is now {}, but this command's slot was keyed for an owned member link; nothing was built. Run the command again",
                    link.display(),
                    state.label()
                ))
            }
            (None, TargetState::RealEmpty | TargetState::RealNonEmpty) => {
                real.push(planned.member.clone());
            }
            (None, _) => {
                return Bind::Refuse(format!(
                    "outputs: {} is now {}, but this command's slot was keyed for a real profile directory; nothing was built. Run the command again",
                    link.display(),
                    state.label()
                ))
            }
        }
    }
    if !absent.is_empty() {
        // Directory creation inside the owned root is a write, so it takes the
        // same writer custody as a relink: an unknown writer or a command that
        // may still be writing refuses or waits before the first creation.
        match store.checkout_custody(&context.workspace, own_record) {
            Err(reason) => return Bind::Refuse(reason),
            Ok(CheckoutCustody::Unknown(reason)) => {
                return Bind::Refuse(format!(
                    "outputs: refusing to create the absent profile member directory: {reason}. Nothing was changed"
                ))
            }
            Ok(CheckoutCustody::Writers) => return Bind::Wait,
            Ok(CheckoutCustody::Clear) => {}
        }
        for (member, wanted) in &absent {
            // Re-verify immediately before the write: the root's marker,
            // kind, volume, nonce, generation and device are all this
            // command's, not merely a directory this path names.
            if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                return Bind::Refuse(format!(
                    "outputs: the absent profile member {member} is not created because {reason}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                ));
            }
            if let Err(reason) = ensure_member_directory(&wanted.root, member) {
                return Bind::Refuse(format!(
                    "outputs: cannot create the absent profile member directory for {member}: {reason}; nothing was built"
                ));
            }
            // The link must still select exactly the directory just created:
            // a link repointed in between is never written through.
            let destination = member_destination(&wanted.root, member);
            match link_state(
                &link_path(&context.workspace, member),
                &context.workspace,
                &context.checkout_id,
            ) {
                Ok(TargetState::OwnedLink(live)) if live == destination => {}
                Ok(state) => {
                    return Bind::Refuse(format!(
                        "outputs: the absent profile member directory {} was created, but {} is now {}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                        destination.display(),
                        link_path(&context.workspace, member).display(),
                        state.label()
                    ))
                }
                Err(reason) => return Bind::Refuse(reason),
            }
        }
    }
    if !needed.is_empty() {
        match store.checkout_custody(&context.workspace, own_record) {
            Err(reason) => return Bind::Refuse(reason),
            Ok(CheckoutCustody::Unknown(reason)) => {
                return Bind::Refuse(format!(
                    "outputs: refusing to link profile members: {reason}. Nothing was changed"
                ))
            }
            Ok(CheckoutCustody::Writers) => return Bind::Wait,
            Ok(CheckoutCustody::Clear) => {}
        }
        // The missing member links live inside the target directory this layout
        // keeps real, so a positively absent one is recreated before the first
        // transition publishes inside it (oxide#165). The create is exclusive
        // and never replaces or follows anything that appeared meanwhile.
        if recreate_target {
            if let Err(reason) = recreate_target_directory(&context.workspace) {
                return Bind::Refuse(format!(
                    "outputs: this checkout's profile members are placed inside the real target directory {}, which is absent and was not recreated: {reason}. Nothing was built",
                    context.link().display()
                ));
            }
        }
        let mut current = receipt.clone();
        for (member, wanted) in &needed {
            let spec = spec_for(wanted);
            // The member's keyed nonce is the only future-root authority this
            // command may bind; a cancelled, replaced or absent reservation
            // refuses inside the transition before any write (oxide#152).
            match transition_at(
                context,
                &current,
                member,
                &spec,
                Start::Any,
                NonceAuthority::Keyed(wanted.nonce()),
            ) {
                Ok(committed) => current = committed,
                Err(reason) => return Bind::Refuse(reason),
            }
            let bound = match current.selected_for(member) {
                Some(bound) => bound.clone(),
                None => {
                    return Bind::Refuse(format!(
                        "outputs: the transition for {member} committed no root; nothing was built"
                    ))
                }
            };
            if bound.root != wanted.root
                || bound.generation != wanted.generation
                || bound.nonce() != wanted.nonce()
            {
                return Bind::Refuse(format!(
                    "outputs: the transition bound {} for {member} but this command's slot was keyed for {}; nothing is built into the wrong root. Run the command again",
                    bound.root.display(),
                    wanted.root.display()
                ));
            }
            if let Err(reason) = check_bound_root(context, current.generation, &bound) {
                return Bind::Refuse(format!(
                    "outputs: the transition bound {} for {member} but {reason}; run the command again",
                    bound.root.display()
                ));
            }
        }
    }
    let mut outcome = Outcome::default();
    outcome.root = owned_root;
    if let Some(root) = outcome.root.clone() {
        let linked = plan
            .members
            .iter()
            .filter(|member| member.selected.is_some())
            .map(|member| member.member.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        outcome.note = Some(format!(
            "owned outputs {} through profile member links ({linked})",
            root.display()
        ));
    }
    if !real.is_empty() {
        let list = real.join(", ");
        outcome.warnings.push(format!(
            "profile outputs: {list} stay real directories in {} and are used locally; an empty one is converted only by `oxide outputs adopt`",
            context.link().display()
        ));
        if outcome.note.is_none() {
            outcome.note = Some(format!("real profile directories kept ({list})"));
        } else if let Some(note) = outcome.note.as_mut() {
            note.push_str(&format!("; real directories kept for {list}"));
        }
    }
    Bind::Bound(outcome)
}

/// The authority a MISSING member link is created under. A member that joins a
/// root this checkout already owns is placed by the receipt itself: the commit
/// of that root's first member consumed the reservation, so adding a later
/// member (a host `--target` triple after `debug`, say) must not demand one.
/// The keyed root must still be exactly a root the receipt currently selects
/// and pass the full marker/kind/volume/generation/device check. Only a root
/// that does not exist yet binds the durable reservation its slot was keyed
/// with.
fn member_root_authority(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> Result<(), String> {
    let committed = receipt
        .links
        .iter()
        .filter_map(|placement| placement.selected.as_ref())
        .any(|selected| {
            selected.root == wanted.root
                && selected.kind == wanted.kind
                && selected.volume_uuid == wanted.volume_uuid
                && selected.nonce() == wanted.nonce()
        });
    if committed {
        return check_bound_root(context, receipt.generation, wanted).map_err(|reason| {
            format!(
                "outputs: this command's new profile member joins the committed root {} but {reason}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                wanted.root.display()
            )
        });
    }
    reservation_matches(context, receipt, wanted)
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
    // One notice per leased command on a legacy whole-target checkout: the
    // profile layout is selected only by the explicit migration.
    outcome.warnings.push(
        "outputs: this checkout uses the whole-target layout; `oxide outputs adopt` migrates it to profile member links (existing outputs are kept)"
            .to_owned(),
    );
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

/// One declared member's planned placement for this command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedMember {
    pub(crate) member: String,
    /// The owned root the member will write through, or `None` for a real
    /// profile directory this command keeps local (it is never converted by a
    /// build).
    pub(crate) selected: Option<Selected>,
}

/// This command's planned destination, read-only and computed before the slot
/// is keyed. The legacy whole-target layout has exactly one `target`
/// placement; the profile layout has one entry per declared member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub(crate) whole_target: bool,
    pub(crate) members: Vec<PlannedMember>,
}

impl Plan {
    fn selected(&self) -> Option<&Selected> {
        self.members
            .iter()
            .find_map(|member| member.selected.as_ref())
    }
}

/// The root the next command must select, read-only and without any lock: the
/// external root when the registered volume is usable now, else the
/// checkout-local fallback. Its `generation` is the generation the root has or
/// will be created at, so the slot key and the binding agree. `None` when this
/// checkout has no owned output link (never converted, or detached).
pub fn planned_destination(
    control: Option<&Path>,
    checkout: &Path,
    declaration: &crate::layout::Declaration,
    pool_fallback: Option<&str>,
    guarded: bool,
) -> Result<Option<Plan>, String> {
    // A guarded workspace's final outputs are local: the plan selects no owned
    // destination, so the slot key is exactly an unconverted checkout's key
    // (design §4.1). Nothing below is read and no root is proposed.
    if guarded {
        return Ok(None);
    }
    let Some(control) = control else {
        return Ok(None);
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    let Some(receipt) = context.receipt()? else {
        return Ok(None);
    };
    // A detached profile checkout owns no destination at all: its command runs
    // locally through real directories, exactly as an unconverted checkout
    // does. Planning the receipt's (unselected) members as owned placements is
    // what used to make this state refuse; oxide#160 (S1) removes that
    // over-refusal, so the key carries no destination and the binding accepts
    // no link.
    if receipt.layout() == LayoutKind::Profile && receipt.state == ReceiptState::Detached {
        return Ok(None);
    }
    match receipt.layout() {
        LayoutKind::WholeTarget => {
            let Some(selected) = desired_selection(&context, &receipt)? else {
                return Ok(None);
            };
            Ok(Some(Plan {
                whole_target: true,
                members: vec![PlannedMember {
                    member: MEMBER.to_owned(),
                    selected: Some(selected),
                }],
            }))
        }
        LayoutKind::Profile => {
            if declaration.is_empty() {
                // A committed profile receipt is always planned from a member
                // set: `oxide exec` derives one from the receipt itself
                // (oxide#160), so an empty set here is a caller bug, not a
                // checkout state.
                return Err("outputs: this checkout places outputs at profile members, but this command declares none; declare its outputs with `oxide exec --output <member>` or run `oxide outputs detach`"
                    .to_owned());
            }
            let desired = desired_selection(&context, &receipt)?;
            let mut members = Vec::with_capacity(declaration.members().len());
            for member in declaration.members() {
                let state = link_state(
                    &link_path(&context.workspace, member.name()),
                    &context.workspace,
                    &context.checkout_id,
                )?;
                let keeps_real =
                    matches!(state, TargetState::RealEmpty | TargetState::RealNonEmpty);
                if !keeps_real && !matches!(state, TargetState::Missing) {
                    // An owned or foreign link: the owned root (or the desired
                    // one) is planned; a foreign link refuses here, before the
                    // slot is keyed or any Cargo call runs.
                    if !matches!(state, TargetState::OwnedLink(_) | TargetState::Dangling(_)) {
                        return Err(format!(
                            "outputs: {} is {}; Oxide never follows or replaces it. Remove it or restore the owned link, then run the command again",
                            link_path(&context.workspace, member.name()).display(),
                            state.label()
                        ));
                    }
                }
                members.push(PlannedMember {
                    member: member.name().to_owned(),
                    selected: if keeps_real { None } else { desired.clone() },
                });
            }
            Ok(Some(Plan {
                whole_target: false,
                members,
            }))
        }
    }
}

/// The profile-output declaration one leased command uses (oxide#164).
///
/// An external subcommand is Cargo running `cargo-<name>`, which is exactly
/// what `oxide exec -- cargo <name>` runs, so when the caller declares no
/// `--output` both use the SAME derivation, [`exec_declaration`]: one member
/// set, one slot key, one set of bindings and refusals. Nothing here adds a
/// second path; `exec`'s explicit `--output` set and the classification of a
/// modelled command or building built-in are unchanged, and a classification
/// this layout cannot model (a JSON target specification) still refuses in a
/// profile checkout.
pub fn lease_declaration(
    invocation: &crate::cli::Invocation,
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<crate::layout::Declaration, String> {
    let command = invocation.command.as_str();
    if crate::cli::derives_from_receipt(command, &invocation.declared_outputs) {
        return exec_declaration(control, checkout, pool_fallback);
    }
    if command == "exec" {
        // The caller's `--output` declaration is the complete set of the
        // tool's profile outputs, exactly as before; it stays an optional
        // override and keeps its byte-identical slot key.
        return crate::layout::declared(&invocation.declared_outputs);
    }
    match crate::layout::classify(
        command,
        invocation.profile,
        invocation.explicit_profile,
        &invocation.targets,
    ) {
        crate::layout::Layout::Declared(declaration) => Ok(declaration),
        crate::layout::Layout::Unknown(reason) => {
            refuse_unknown_layout(control, checkout, &reason)?;
            Ok(crate::layout::Declaration::empty())
        }
    }
}

/// The declaration a guarded one-use workspace's leased command uses (design
/// §4.1, §4.3).
///
/// A guarded workspace owns no output root, so it never derives a member set
/// from a receipt and nothing here reads one or refuses: the set exists only
/// to name the components §4.3's guard inspects. A command whose layout this
/// build cannot model — a JSON target specification, or a derived `oxide exec`
/// with no `--output` — declares no member, and the guard then inspects the
/// checkout's `target` directory alone; the command still builds locally.
pub fn guarded_declaration(
    invocation: &crate::cli::Invocation,
) -> Result<crate::layout::Declaration, String> {
    let command = invocation.command.as_str();
    if command == "exec" || crate::cli::derives_from_receipt(command, &invocation.declared_outputs)
    {
        return crate::layout::declared(&invocation.declared_outputs);
    }
    Ok(
        match crate::layout::classify(
            command,
            invocation.profile,
            invocation.explicit_profile,
            &invocation.targets,
        ) {
            crate::layout::Layout::Declared(declaration) => declaration,
            crate::layout::Layout::Unknown(_) => crate::layout::Declaration::empty(),
        },
    )
}

/// The profile member set `oxide exec` derives when the caller declares none
/// (oxide#160, S1; design `exec-output-inference.md`).
///
/// The keyed set is exactly what this checkout's committed receipt places,
/// plus every receipt-recorded member it keeps real; a member-shaped path the
/// receipt does not record is never planned, keyed or converted. Every
/// candidate path is classified read-only, with the shipped
/// [`migration_members`] traversal and [`link_state`] pair: neither follows a
/// link target, lists a payload or walks a third level, and the classification
/// can only refuse. Nothing here creates, links, reserves or deletes anything.
///
/// `Ok(Declaration::empty())` is the honest answer for a checkout with no
/// owned placement (never converted, legacy whole target, or detached): those
/// commands run exactly as they do today, with no owned destination in the
/// key.
pub fn exec_declaration(
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<crate::layout::Declaration, String> {
    let Some(control) = control else {
        return Ok(crate::layout::Declaration::empty());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    let Some(receipt) = context.receipt()? else {
        return Ok(crate::layout::Declaration::empty());
    };
    if receipt.layout() != LayoutKind::Profile {
        // A legacy whole-target checkout's declaration is never consulted.
        return Ok(crate::layout::Declaration::empty());
    }
    match receipt.state {
        ReceiptState::Committed | ReceiptState::Detached => {}
        ReceiptState::Prepared => {
            return Err("outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before running an exec tool".to_owned())
        }
        ReceiptState::Pruning => {
            return Err("outputs: a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it".to_owned())
        }
        ReceiptState::Pruned => {
            return Err("outputs: this checkout's owned output root was pruned; run `oxide outputs adopt` to convert it again".to_owned())
        }
    }
    // The destination the plan will use, computed exactly as
    // `planned_destination` computes it: a receipt-selected member must already
    // be this checkout's owned link AT THAT destination, so a derived `exec`
    // never relinks, not even between two roots the receipt knows.
    let desired = desired_selection(&context, &receipt)?;
    let members = exec_members(&context, &receipt, desired.as_ref())?;
    if receipt.state == ReceiptState::Detached {
        // A detached checkout owns no destination: its command runs locally
        // through real directories, so the derived set is empty and the key
        // carries no destination. The classification above still refused any
        // member-shaped link or file it must not write through.
        return Ok(crate::layout::Declaration::empty());
    }
    if members.len() > crate::layout::MAX_MEMBERS {
        return Err(format!(
            "outputs: this checkout places {} profile output members, more than the {} one command binds; run `oxide outputs detach` for a real target directory, or declare the members this tool writes with `oxide exec --output <member>`. Nothing was changed",
            members.len(),
            crate::layout::MAX_MEMBERS
        ));
    }
    let parsed = members
        .iter()
        .map(|name| crate::layout::Member::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    crate::layout::Declaration::new(parsed)
}

/// Classify every candidate profile member of a profile-layout checkout for a
/// derived `exec` plan. Only this checkout's own owned links at the receipt's
/// committed destinations are accepted, plus the two absent-link shapes the
/// permanent-mode binding repairs (an absent member directory beneath a
/// verified root, oxide#162, and an absent member link of a verified root whose
/// real `target` directory went with it, oxide#165); a real member the receipt
/// keeps local is planned `real`; everything else that occupies a member-shaped
/// path refuses before Cargo, with the link, receipt and root unchanged.
fn exec_members(
    context: &Context,
    receipt: &Receipt,
    desired: Option<&Selected>,
) -> Result<Vec<String>, String> {
    let live_target = target_state(&context.workspace, &context.checkout_id)?;
    let mut candidates: Vec<String> = Vec::new();
    match &live_target {
        TargetState::RealEmpty | TargetState::RealNonEmpty => {
            // The shipped migration traversal: member names are matched first
            // and `file_type()` never follows a link, so no profile payload is
            // listed and no linked entry is descended.
            candidates = migration_members(&context.link())?;
        }
        TargetState::Missing => {}
        state => {
            return Err(format!(
                "outputs: {} is {}; a checkout that places outputs at profile members keeps a real target directory, so an exec tool never writes through it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                context.link().display(),
                state.label()
            ))
        }
    }
    // Every recorded placement is a candidate too: a member the receipt places
    // but whose path is absent still has to be checked, and it refuses below
    // rather than being guessed into the plan.
    for placement in receipt.links() {
        if !candidates.iter().any(|name| name == placement.member()) {
            candidates.push(placement.member().to_owned());
        }
    }
    candidates.sort();
    candidates.dedup();
    // The absent-destination repair belongs to the permanent-mode binding's
    // custody; every other mode keeps exactly today's refusal.
    let permanent = context.mode()? == Some(DriveMode::Permanent);
    let mut members = Vec::new();
    for name in &candidates {
        let link = link_path(&context.workspace, name);
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        match receipt.placement(name) {
            Some(placement) => match placement.selected_ref() {
                Some(_) => {
                    let Some(desired) = desired else {
                        return Err(format!(
                            "outputs: this checkout's receipt places the profile member {name} at a root this command would no longer select; an exec command never relinks it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                        ));
                    };
                    let destination = member_destination(&desired.root, name);
                    match &state {
                        TargetState::OwnedLink(root) if root == &destination => {
                            members.push(name.clone());
                        }
                        // oxide#165: the member link is positively ABSENT
                        // while its committed destination's root fully
                        // verifies — the shape a deleted `<checkout>/target`
                        // directory leaves, because the member links live
                        // inside it. The permanent-mode binding recreates that
                        // real directory and publishes this member through the
                        // unchanged transition (which creates the member
                        // directory inside the root first). A missing,
                        // unverified or other root keeps the refusal below.
                        TargetState::Missing
                            if permanent
                                && missing_member_of_verified_root(context, receipt, desired) =>
                        {
                            members.push(name.clone());
                        }
                        // The only absent-directory case a derived exec may
                        // place: the link selects exactly this member's
                        // destination under the committed, verified root (same
                        // root, nonce, volume and generation) and only that
                        // member's directory is missing. The permanent-mode
                        // binding creates it under the root's custody; a link
                        // into a missing, unverified or foreign root still
                        // refuses exactly as before (its root fails the check).
                        state
                            if permanent
                                && absent_member_of_verified_root(
                                    context,
                                    receipt,
                                    desired,
                                    name,
                                    state,
                                ) =>
                        {
                            members.push(name.clone());
                        }
                        state => {
                            return Err(format!(
                                "outputs: this checkout's receipt places the profile member {name} at {}, but {} is {}; an exec command never relinks, converts or replaces it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs, then run the command again",
                                destination.display(),
                                link.display(),
                                state.label()
                            ))
                        }
                    }
                }
                None => match &state {
                    TargetState::RealEmpty | TargetState::RealNonEmpty => {
                        members.push(name.clone());
                    }
                    TargetState::Missing => {}
                    state => {
                        return Err(format!(
                            "outputs: this checkout's receipt records the profile member {name} as a real directory, but {} is {}; an exec command never relinks, replaces or writes through it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                            link.display(),
                            state.label()
                        ))
                    }
                },
            },
            None => match &state {
                // A real member outside the receipt is local and path-stable:
                // it is not planned and not keyed, and the tool writes into it
                // as it is.
                TargetState::RealEmpty | TargetState::RealNonEmpty | TargetState::Missing => {}
                state => {
                    return Err(format!(
                        "outputs: {} is {}, and this checkout's receipt does not place it; an exec command never writes through a link or file it does not own. Remove it or run `oxide outputs adopt` to record the placement, then run the command again. Nothing was changed",
                        link.display(),
                        state.label()
                    ))
                }
            },
        }
    }
    Ok(members)
}

/// Whether a member link is positively ABSENT while the receipt's committed
/// placement for it is a root that fully verifies right now (oxide#165). This
/// is the shape a deleted `<checkout>/target` directory leaves: the member
/// links lived inside it, so they are gone with it, while the owned root, its
/// marker, its nonce and its payload are untouched. The permanent-mode binding
/// recreates the target directory and publishes such a member; nothing is
/// replaced or followed here. A missing, another-nonce or unverified root
/// fails this check and keeps the existing refusal.
fn missing_member_of_verified_root(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> bool {
    check_bound_root(context, receipt.generation, wanted).is_ok()
}

/// Whether this member link's only defect is its ABSENT destination directory
/// beneath the committed, desired owned root. The link must select exactly
/// `<root>/<member>` and the root itself must pass the full bound check: this
/// checkout's real marker, its kind and volume, the receipt's generation, the
/// desired nonce and the live device/ancestry. Nothing is created here; the
/// permanent-mode binding does that under the checkout's custody. A missing
/// root, another root, another nonce and an unverified directory all fail,
/// which keeps every existing refusal intact.
fn absent_member_of_verified_root(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
    name: &str,
    state: &TargetState,
) -> bool {
    let TargetState::Dangling(destination) = state else {
        return false;
    };
    destination == &member_destination(&wanted.root, name)
        && check_bound_root(context, receipt.generation, wanted).is_ok()
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

/// The EXACT P4 precondition binding enforces, evaluated before the lease.
///
/// `bind_profile` refuses per REQUESTED member: that member's committed
/// placement is on an EXTERNAL root while this command's fresh selection for it
/// is not external (the registered volume is away or became unusable). Only a
/// profile receipt carries that per-member rule — a whole-target receipt never
/// refuses this way, and its future local root needs its reservation so
/// `reservation_matches` can accept it — so the layout is part of the
/// condition. A member with no committed placement (a new or nested member)
/// does not trigger the refusal either, and keeps its reservation.
///
/// Returns true only when binding will refuse because of this condition, so the
/// pre-lease reservation can leave the receipt untouched.
fn refuses_committed_external(
    receipt: &Receipt,
    wanted: &Selected,
    declaration: &crate::layout::Declaration,
) -> bool {
    if receipt.layout() != LayoutKind::Profile || wanted.kind == RootKind::External {
        return false;
    }
    declaration.members().iter().any(|member| {
        receipt
            .placement(member.name())
            .and_then(|placement| placement.selected.as_ref())
            .is_some_and(|committed| committed.kind == RootKind::External)
    })
}

/// The dangling member link binding refuses, evaluated before the lease.
///
/// `bind_profile` refuses a REQUESTED profile member whose owned member link
/// dangles (the committed root is missing at that member's destination). A
/// future-root reservation published for that destination is dead on arrival:
/// its registry row is the committed SELECTED row, which keeps the root's old
/// nonce, so `validate_reservation` refuses it — and it then blocks the
/// explicit `oxide outputs adopt` that the binding refusal names. Reserving
/// nothing keeps that refusal at zero authority effect, and the adopt recovers
/// with a fresh nonce through the ordinary rules (oxide#152).
///
/// Only the requested member set is classified, and only the exact
/// `TargetState::Dangling` binding refuses: a member link that still resolves
/// (`OwnedLink`, the local→external redock) and a missing link (a new member
/// joining an existing root) keep their reservation, as does any whole-target
/// layout.
fn refuses_dangling_member(
    context: &Context,
    receipt: &Receipt,
    declaration: &crate::layout::Declaration,
) -> Result<bool, String> {
    if receipt.layout() != LayoutKind::Profile {
        return Ok(false);
    }
    for member in declaration.members() {
        if receipt.selected_for(member.name()).is_none() {
            continue;
        }
        let link = link_path(&context.workspace, member.name());
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        if matches!(state, TargetState::Dangling(_)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether one leased command must durably reserve a future root nonce before
/// its slot is keyed (oxide#129).
///
/// A derived plan over this checkout's committed placement is a subset of that
/// placement, so it reserves nothing: the root it binds already exists and no
/// member is new. A derived set that is EMPTY names no placement at all — a
/// whole-target checkout, a detached receipt, an unconverted checkout — and a
/// whole-target plan can still name a FUTURE root (the first local fallback to
/// an absent root, oxide#117), which binding accepts only through the exact
/// surviving reservation its slot was keyed with. Those keep the shipped
/// reservation path, exactly as every command did before oxide#164: a derived
/// `exec` in the same state now does too (oxide#164 review finding).
///
/// Callers pass whether the command derived its set
/// ([`crate::cli::derives_from_receipt`]) and the declaration it will use.
pub fn needs_reservation(derived: bool, declaration: &crate::layout::Declaration) -> bool {
    !derived || declaration.is_empty()
}

/// Upper bound on the entries one bounded target observation considers.
const TARGET_ENTRY_CAP: usize = 4096;
/// Upper bound on the directory names one notice prints.
const TOOL_DIRECTORY_CAP: usize = 8;
/// Cargo's own top-level directory inside a real target. It is never a profile
/// member and never a tool's output directory, so it is never reported.
const CARGO_TARGET_TEMP: &str = "tmp";

/// The entry names directly under one checkout's real target directory: one
/// bounded, non-recursive listing that never follows a link.
///
/// `None` when `target` is not a real directory (the whole-target layout links
/// it and an absent target holds nothing), when it cannot be read, or when it
/// holds more than [`TARGET_ENTRY_CAP`] entries. An unexpected or oversized
/// tree is left unreported rather than walked.
pub fn target_entries(checkout: &Path) -> Option<Vec<String>> {
    let target = checkout.join("target");
    let metadata = fs::symlink_metadata(&target).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&target).ok()? {
        if names.len() >= TARGET_ENTRY_CAP {
            return None;
        }
        let entry = entry.ok()?;
        names.push(entry.file_name().to_str()?.to_owned());
    }
    names.sort();
    Some(names)
}

/// Whether one command's bound layout is the receipt-derived PROFILE-MEMBER
/// layout, the only path that prints the tool-directory notice (oxide#164).
///
/// A derived command (`oxide exec` without `--output`, or an external
/// subcommand) that planned profile members is the checkout shape the brief's
/// "reported once" belongs to. Every other shape keeps its shipped stderr: a
/// never-converted or ephemeral checkout plans nothing, a legacy whole-target
/// checkout plans the whole target, a detached profile receipt plans no
/// destination, and a modelled command or an explicit `--output` declaration is
/// not a derived set. None of them gets the extra line.
pub fn tool_notice_applies(derived: bool, plan: Option<&Plan>) -> bool {
    // The checkout's layout decides, not which members this command happened to
    // place: a profile plan whose member is a receipt-recorded real directory is
    // still the profile layout, and the tool may create a non-member directory
    // beside it.
    derived && plan.is_some_and(|plan| !plan.whole_target && !plan.members.is_empty())
}

/// One bounded notice naming the directories a tool created directly under its
/// real target that are NOT profile members — the tool's own `target/nextest`
/// or `target/llvm-cov` (oxide#164). Callers print it only when
/// [`tool_notice_applies`] holds, so an unconverted, ephemeral, whole-target or
/// detached checkout keeps its previous stderr.
///
/// `before` is [`target_entries`] captured before the writer started. The
/// comparison is two listings of ONE directory: no recursion, no payload walk,
/// no link followed, and at most [`TOOL_DIRECTORY_CAP`] names are printed.
/// Cargo's own top-level `tmp` is never reported, a member-shaped path is the
/// member layout's business (never this notice's), and a `target` that is not a
/// real directory is not observed at all.
pub fn tool_directories_note(checkout: &Path, before: Option<&[String]>) -> Option<String> {
    let before = before?;
    let after = target_entries(checkout)?;
    let mut created: Vec<String> = after
        .into_iter()
        .filter(|name| !before.iter().any(|known| known == name))
        .filter(|name| name.as_str() != CARGO_TARGET_TEMP)
        .filter(|name| crate::layout::Member::parse(name).is_err())
        .filter(|name| {
            fs::symlink_metadata(checkout.join("target").join(name))
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        })
        .collect();
    created.sort();
    if created.is_empty() {
        return None;
    }
    let shown = created.len().min(TOOL_DIRECTORY_CAP);
    let mut list = created[..shown].join(", ");
    if created.len() > shown {
        list.push_str(&format!(" and {} more", created.len() - shown));
    }
    Some(format!(
        "tool directories kept local ({list}): a directory that is not a profile member stays a real directory of this checkout and is never placed"
    ))
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
    declaration: &crate::layout::Declaration,
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
        // oxide#133 case14: a command that binding will refuse because of a
        // committed external member must not reserve (or cancel) anything
        // first. Reserving a root here used to rewrite the receipt — a real
        // authority effect on a command that builds nothing. The condition is
        // binding's own P4 precondition, per requested member and profile
        // layout only, so every command binding accepts still reserves.
        if refuses_committed_external(&receipt, &wanted, declaration) {
            return Ok(());
        }
        // oxide#152: a requested member whose owned link dangles is a
        // destination binding refuses, so this command publishes no
        // reservation at all. Its receipt and registry stay byte-identical and
        // the explicit `oxide outputs adopt` the refusal names can recover.
        if refuses_dangling_member(&context, &receipt, declaration)? {
            return Ok(());
        }
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
        *receipt.reserved_mut() = Some(Reservation {
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
    *receipt.reserved_mut() = None;
    receipt.updated_ms = process::now_ms();
    write_receipt(control, receipt)?;
    crate::prune::finish_cancel(control, &context.checkout_id, &reserved)
}

/// `oxide outputs cancel-reservation`: the explicit cancellation of this
/// checkout's unused reservation, under its clean barrier and binding lock.
fn cancel_command() -> Result<u8, String> {
    let context = Context::open()?;
    cancel_at(&context, output_store)
}

/// `oxide outputs cancel-reservation` against one already-opened context and a
/// **lazy** pool provider: the same effect-free fast precheck as `detach`, and
/// it runs before the pool is opened.
fn cancel_at(
    context: &Context,
    pool: impl FnOnce() -> Result<Option<Store>, String>,
) -> Result<u8, String> {
    guarded_command_precheck(context, "outputs cancel-reservation refused")?;
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = pool()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let internal = &internal;
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    // The same guarded order as `detach`: revalidate the receipt and the
    // migration journal under the binding mutex before any mark, then a
    // bounded `Barrier`-mode acquisition (design §4.5).
    let guarded = guarded_workspace(Some(&control), &context.workspace).is_some();
    let guard = if guarded {
        guarded_barrier(internal, context, &mut waits)?
    } else {
        barrier(internal, &context.workspace, &mut waits)?
    };
    let outcome = (|| -> Result<Option<Reservation>, String> {
        if guarded {
            guarded_prepared_refusal(&context)?;
        }
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

/// Refuse a command whose profile outputs cannot be classified from its name
/// in a checkout that already places outputs at profile members. An unlinked
/// checkout runs such a command locally exactly as before. External
/// subcommands no longer reach this refusal: they derive their member set from
/// the receipt like `oxide exec` (oxide#164), so only a modelled command whose
/// target or profile the layout cannot model (a JSON target specification)
/// refuses here.
pub fn refuse_unknown_layout(
    control: Option<&Path>,
    checkout: &Path,
    reason: &str,
) -> Result<(), String> {
    let Some(control) = control else {
        return Ok(());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf());
    let profile = context.receipt()?.is_some_and(|receipt| {
        receipt.layout() == LayoutKind::Profile && receipt.state != ReceiptState::Detached
    });
    if profile {
        return Err(format!(
            "outputs: {reason}. This checkout places outputs at profile members and Oxide will not guess where this command writes; declare its outputs with `oxide exec --output <member>` or run `oxide outputs detach`"
        ));
    }
    Ok(())
}

/// The real executable for a Cargo-reported program path, when that program's
/// member is an owned link into `root`: `target/<member>/<rest>` maps to
/// `root/<member>/<rest>`. A real (unlinked) member keeps Cargo's own path, so
/// a mixed checkout never runs another member's artifact.
pub fn program_target(checkout: &Path, root: &Path, program: &Path) -> Option<PathBuf> {
    let relative = program.strip_prefix(&checkout.join("target")).ok()?;
    let mut components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        });
    let first = components.next()?;
    let member = if first == "debug" || first == "release" {
        first.to_owned()
    } else {
        let second = components.next()?;
        if second != "debug" && second != "release" {
            return None;
        }
        format!("{first}/{second}")
    };
    crate::layout::Member::parse(&member).ok()?;
    let target = fs::read_link(link_path(checkout, &member)).ok()?;
    if target != member_destination(root, &member) {
        return None;
    }
    let mapped = root.join(relative);
    mapped.is_file().then_some(mapped)
}

/// The final-output destination, as a component of a slot's compatibility
/// identity. Empty for an ephemeral or detached checkout, so those checkouts
/// keep exactly the slot keys they had before this feature.
///
/// A legacy whole-target plan renders the published `outputs-2` line
/// unchanged: its key never changes without an explicit migration. A profile
/// plan renders one sorted `outputs-3` line per owned member and `real
/// <member>` for a real directory kept local, so a linked member and a real
/// member never share a slot.
pub fn destination_facts(plan: Option<&Plan>) -> String {
    let Some(plan) = plan else {
        return String::new();
    };
    plan.members
        .iter()
        .map(|planned| match &planned.selected {
            Some(selected) if plan.whole_target => {
                format!("outputs-2 {}", selected_facts(selected))
            }
            Some(selected) => format!("outputs-3 {} {}", selected_facts(selected), planned.member),
            None => format!("real {}", planned.member),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One owned root's identity inside a slot key, without the layout's
/// namespace: a legacy key prefixes `outputs-2`, a profile key `outputs-3` and
/// the member.
fn selected_facts(selected: &Selected) -> String {
    format!(
        "{} {} {} {} g{}",
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

/// What a guarded leased command needs from the local-output guard: the
/// member set design §4.3 inspects and the bounded reason its one note
/// prints.
pub struct LocalOutputs<'a> {
    pub members: &'a [String],
    pub reason: &'a str,
}

/// The one-use classification of `workspace` when the feature is active and
/// the workspace is guarded; `None` when the feature is dormant or the
/// workspace is normal, which is every production command in this slice.
///
/// Effect-free: the classification reads the environment and the registered
/// pool facts only. Dropped-root notes are printed by the leased-command path
/// alone, so one command reports them once.
pub(crate) fn guarded_workspace(
    control: Option<&Path>,
    workspace: &Path,
) -> Option<crate::one_use::Classification> {
    if !crate::one_use::is_active() {
        return None;
    }
    let classification =
        crate::one_use::classify(workspace, &crate::one_use::Facts::from_control(control));
    #[cfg(test)]
    crate::one_use::note_classification();
    classification.guarded().then_some(classification)
}

/// Whether this receipt records an unresolved `Prepared` transition, in any
/// shape: the state itself, a pending layout-migration journal, or a prepared
/// placement (design §4.1, §4.4).
fn prepared_like(receipt: &Receipt) -> bool {
    receipt.state == ReceiptState::Prepared
        || receipt.transition.is_some()
        || receipt
            .links()
            .iter()
            .any(|placement| placement.prepared_ref().is_some())
}

/// The exact guarded `Prepared` refusal for one checkout (design §4.4). Every
/// guarded verb revalidates through this before any effect, under its own
/// custody, so a `Prepared` receipt or a pending migration journal is never
/// replayed and never repaired from a guarded workspace.
fn guarded_prepared_refusal(context: &Context) -> Result<(), String> {
    match context.receipt()? {
        Some(receipt) if prepared_like(&receipt) => {
            Err(crate::one_use::PREPARED_GUARDED.to_owned())
        }
        _ => Ok(()),
    }
}

/// The effect-free fast precheck of a guarded output verb (design §4.1 part 1,
/// §4.5): the checkout's authority state is read, and nothing else happens — no
/// pool open, no signal handler, no binding mutex, no barrier, no clean mark,
/// no id. It may refuse early; it never authorises the verb, whose
/// authoritative revalidation still runs under the binding mutex.
fn guarded_command_precheck(context: &Context, verb: &str) -> Result<(), String> {
    if guarded_workspace(context.control(), &context.workspace).is_none() {
        return Ok(());
    }
    unsettled_refusal(context.control(), &context.checkout_id)?;
    guarded_prepared_refusal(context)?;
    // The shipped `Pruning` refusal is kept: a guarded workspace never replays
    // or repairs a listed root either.
    if let Some(receipt) = context.receipt()? {
        if receipt.state == ReceiptState::Pruning {
            return Err(format!("{verb}: {PRUNING_REFUSAL}"));
        }
    }
    Ok(())
}

/// The guarded `adopt` refusal and its precedence (design §4.1, §4.5): a
/// `Prepared` transition (the state, a prepared placement or a pending
/// migration journal) selects the exact §4.4 diagnostic; **every** other state
/// — absent, `Committed`, `Detached`, `Pruned`, `Pruning`, an unsettled prune
/// intent, a malformed or mismatched receipt — selects the general refusal.
/// A malformed document is never the reply and never becomes the decision,
/// because a guarded workspace cannot adopt at all.
fn guarded_adopt_refusal(context: &Context) -> String {
    if matches!(context.receipt(), Ok(Some(receipt)) if prepared_like(&receipt)) {
        crate::one_use::PREPARED_GUARDED.to_owned()
    } else {
        crate::one_use::adopt_refusal(&context.workspace)
    }
}

/// The shipped refusal text for a receipt whose listed legacy root is being
/// pruned, reused by the guard so a guarded command refuses before any effect
/// exactly as it does today.
const PRUNING_REFUSAL: &str = "a listed legacy root of this checkout is being pruned; nothing changes until `oxide outputs prune` replays it or `oxide outputs resolve-intent` settles it";

/// The guarded local-output decision for one leased command (design §4.2),
/// read-only and effect-free.
///
/// `Ok(())` means the command runs with local outputs; `Err` is the exact
/// refusal text and means nothing was created, linked, reserved or deleted.
fn local_outputs_decision(
    context: &Context,
    members: &[String],
    prefix: &str,
) -> Result<(), String> {
    unsettled_refusal(context.control(), &context.checkout_id)?;
    if let Some(receipt) = context.receipt()? {
        if prepared_like(&receipt) {
            return Err(crate::one_use::PREPARED_GUARDED.to_owned());
        }
        match receipt.state {
            // A committed receipt selects an owned root in every member shape,
            // so a guarded workspace never builds locally through it.
            ReceiptState::Committed => {
                let root = receipt
                    .selected()
                    .map_or_else(|| context.link(), |selected| selected.root.clone());
                return Err(crate::one_use::committed_refusal(&root));
            }
            ReceiptState::Pruning => return Err(format!("{prefix}: {PRUNING_REFUSAL}")),
            // `Detached` and `Pruned` own no selection: the live components
            // below decide, and neither is a repair opportunity.
            ReceiptState::Prepared | ReceiptState::Detached | ReceiptState::Pruned => {}
        }
    }
    inspect_components(&context.link(), members, prefix)
}

/// The bounded component inspection of design §4.3: `<workspace>/target` and
/// every profile member this command declares, each reached through its
/// parent chain with `symlink_metadata` only. No listing, no recursion, no
/// link followed, no payload walk.
fn inspect_components(target: &Path, members: &[String], prefix: &str) -> Result<(), String> {
    if !inspect_component(target, prefix)? {
        // An absent target holds nothing below it.
        return Ok(());
    }
    for member in members {
        let mut path = target.to_path_buf();
        for component in member.split('/') {
            path.push(component);
            if !inspect_component(&path, prefix)? {
                break;
            }
        }
    }
    Ok(())
}

/// One inspected component: `Ok(false)` absent, `Ok(true)` a real directory,
/// and a refusal (or an unreadable-path error) for anything else.
fn inspect_component(path: &Path, prefix: &str) -> Result<bool, String> {
    let shown = || path.display().to_string();
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "{prefix}: cannot inspect {}: {error}; nothing was changed",
            shown()
        )),
        Ok(metadata) => {
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                return Ok(true);
            }
            let kind = if metadata.file_type().is_symlink() {
                "a symbolic link".to_owned()
            } else if metadata.is_file() {
                "a file".to_owned()
            } else {
                "not a real directory".to_owned()
            };
            Err(format!(
                "{prefix}: {} is {kind}; a one-use (temporary) workspace keeps final outputs local and Oxide never follows, adopts or replaces it; nothing was changed",
                shown()
            ))
        }
    }
}

/// The guard's effect-free fast precheck for a leased command (design §4.1
/// part 1). It may refuse early, with literally nothing created, and it never
/// authorises the command: the authoritative decision is taken again inside
/// [`bind_for_command`]'s checkout binding mutex.
pub fn guarded_precheck(
    control: Option<&Path>,
    checkout: &Path,
    members: &[String],
) -> Result<(), String> {
    let context = Context::new(control.map(Path::to_path_buf), checkout.to_path_buf());
    local_outputs_decision(&context, members, "outputs")
}

/// The guarded binding branch: local outputs only, with the authoritative
/// revalidation under the checkout binding mutex and no output effect at all
/// (design §4.1 row 4).
///
/// The mutex excludes the one writer that can publish a `Prepared` receipt
/// under the mutex alone (a peer command's own binding), so a transition that
/// appeared while this command waited is refused here rather than replayed.
fn bind_local_outputs(
    store: &Store,
    context: &Context,
    expected: Option<&Plan>,
    local: &LocalOutputs<'_>,
) -> Result<OutputBinding, String> {
    let lock = store.lock_binding(&context.workspace)?;
    unsettled_refusal(context.control(), &context.checkout_id)?;
    let receipt = context.receipt()?;
    if let Some(receipt) = &receipt {
        if prepared_like(receipt) {
            return Err(crate::one_use::PREPARED_GUARDED.to_owned());
        }
        match receipt.state {
            ReceiptState::Committed => {
                let root = receipt
                    .selected()
                    .map_or_else(|| context.link(), |selected| selected.root.clone());
                return Err(crate::one_use::committed_refusal(&root));
            }
            ReceiptState::Pruning => return Err(format!("outputs: {PRUNING_REFUSAL}")),
            ReceiptState::Prepared | ReceiptState::Detached | ReceiptState::Pruned => {}
        }
    }
    inspect_components(&context.link(), local.members, "outputs")?;
    // A slot is keyed for a local target, so a checkout converted while this
    // command waited still refuses through the shipped key check rather than
    // binding another destination. `Detached` and `Pruned` receipts select
    // nothing and are unconverted for this purpose (design §4.2); a
    // `Committed`, `Prepared` or `Pruning` receipt was refused above.
    let converted = receipt.as_ref().is_some_and(|receipt| {
        !matches!(receipt.state, ReceiptState::Detached | ReceiptState::Pruned)
    });
    keyed_state_refusal(converted, expected)?;
    Ok(OutputBinding {
        outcome: local_outcome(local.reason),
        _lock: Some(lock),
    })
}

/// The one bounded note a guarded command prints, through the shipped warning
/// surface (design §4.5). No owned root is selected and no adoption warning is
/// added.
fn local_outcome(reason: &str) -> Outcome {
    Outcome {
        warnings: vec![format!("one-use workspace: local outputs ({reason})")],
        note: None,
        root: None,
    }
}

/// The guarded verbs' barrier acquisition (design §4.1 steps 3-5).
///
/// The checkout binding mutex is held across **both** the revalidation and the
/// non-blocking `Barrier` attempt, so no peer command's binding can publish a
/// `Prepared` receipt in the gap between them; it is released between attempts
/// and before the poll. Every lock the attempt takes is non-blocking, so this
/// order can never deadlock with a shipped verb that holds the barrier and then
/// wants the binding mutex. The attempt budget is
/// [`crate::store::barrier_attempts`]; expiry is the bounded busy refusal with
/// nothing written.
fn guarded_barrier(
    store: &Store,
    context: &Context,
    waits: &mut WaitAccounting,
) -> Result<CleanGuard, String> {
    for _ in 0..crate::store::barrier_attempts() {
        if let Some(signal) = process::received_signal() {
            return Err(format!(
                "interrupted by signal {signal} while waiting for this checkout's commands to finish; nothing was changed"
            ));
        }
        let binding = store.lock_binding(&context.workspace)?;
        let attempt = store.begin_clean_barrier(&context.workspace, waits, || {
            guarded_prepared_refusal(context)
        })?;
        drop(binding);
        match attempt {
            Cleaning::Guard(guard) => return Ok(guard),
            Cleaning::Interrupted(signal) => {
                return Err(format!(
                    "interrupted by signal {signal} while waiting for this checkout's commands to finish; nothing was changed"
                ))
            }
            // A dead fact: no attempt can make progress, so refuse now.
            Cleaning::Busy(reason) => return Err(reason),
            Cleaning::Retry(_) => std::thread::sleep(BINDING_POLL),
        }
    }
    Err(crate::one_use::BARRIER_BUSY.to_owned())
}

/// Refuse a guarded command while a prune intent for this checkout is
/// unsettled: the shipped refusal, kept for the guarded branch (design §4.2's
/// "refuses through the existing validation/refusal"), because the intent owns
/// authority over this checkout's roots.
fn unsettled_refusal(control: Option<&Path>, checkout_id: &str) -> Result<(), String> {
    let Some(control) = control else {
        return Ok(());
    };
    match crate::prune::unsettled_for(control, checkout_id)? {
        Some(intent) => Err(format!(
            "outputs: prune intent {intent} is unsettled for this checkout; nothing is built until the exact intent completes or `oxide outputs resolve-intent {intent}` settles it"
        )),
        None => Ok(()),
    }
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
        // `Recovering` mode never returns `Busy` or `Retry`; only a guarded
        // verb's `Barrier` mode does, through [`guarded_barrier`]'s own loop.
        Cleaning::Busy(reason) | Cleaning::Retry(reason) => Err(reason),
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
    link_state(&checkout.join("target"), checkout, checkout_id)
}

/// Classify one link path — the legacy `<checkout>/target` or one profile
/// member below it — without following a link away from ownership.
fn link_state(link: &Path, checkout: &Path, checkout_id: &str) -> Result<TargetState, String> {
    let target = link.to_path_buf();
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
        if owned_root(checkout, checkout_id, &root).is_ok() {
            return Ok(TargetState::OwnedLink(root));
        }
        // A profile member link selects a distinct subpath beneath the owned
        // root, whose marker lives at the root itself. Ownership is decided by
        // that root marker, never by following an arbitrary ancestor, and the
        // destination's own shape (`<profile>` or `<triple>/<profile>`) is only
        // a candidate list: it grants nothing without the marker.
        for member_root in member_root_candidates(&root) {
            if owned_root(checkout, checkout_id, &member_root).is_ok() {
                // The root is ours; the member destination is only an owned
                // link while it actually exists.
                return Ok(if fs::symlink_metadata(&root).is_ok() {
                    TargetState::OwnedLink(root)
                } else {
                    TargetState::Dangling(root)
                });
            }
        }
        return Ok(if fs::symlink_metadata(&root).is_err() {
            TargetState::Dangling(root)
        } else {
            TargetState::Foreign
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

/// The candidate owned roots one member destination can belong to: strip a
/// valid v1 member suffix and return every root that shape could name, most
/// specific first. A plain `<root>/<profile>` destination names its parent; a
/// `<root>/<triple>/<profile>` destination names its grandparent, whose child
/// directory is the triple. Both are only candidates — ownership is decided by
/// the marker at the root, and a wrong root simply fails that check.
fn member_root_candidates(target: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
        return roots;
    };
    if name != "debug" && name != "release" {
        return roots;
    }
    let Some(parent) = target.parent() else {
        return roots;
    };
    // <root>/<triple>/<profile>, then <root>/<profile>. The triple form is
    // tried first so a nested link is reported against the root that really
    // owns it when a checkout's root is itself named like a member component.
    if let (Some(triple), Some(grandparent)) = (
        parent.file_name().and_then(|triple| triple.to_str()),
        parent.parent(),
    ) {
        if crate::layout::Member::parse(&format!("{triple}/{name}")).is_ok() {
            roots.push(grandparent.to_path_buf());
        }
    }
    roots.push(parent.to_path_buf());
    roots
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

/// P3: publish the root/2 successor from the PINNED old marker, changing ONLY
/// the version and the reserved nonce. Every immutable field — checkout
/// identity, label, recipe, kind, volume, the ORIGINAL root-creation
/// generation and the original `created_ms` — is copied from the pinned root/1
/// document. An existing root never goes through [`create_marker`], whose
/// fresh-root path stamps a new `created_ms`.
fn publish_successor_marker(
    context: &Context,
    migration: &Migration,
    before: &Marker,
) -> Result<Marker, String> {
    // The only field the migration is allowed to change besides the version.
    if before.nonce.is_some() {
        return Err(pending_migration(
            migration,
            "the pinned old marker already carries a nonce",
        ));
    }
    let nonce = migration
        .root
        .nonce()
        .ok_or_else(|| pending_migration(migration, "the migration reserved no root nonce"))?;
    let spec = spec_for(&migration.root);
    let selected = spec
        .selected(before.generation)
        .with_nonce(Some(nonce.to_owned()));
    // The root must be on the intended device before the successor is written.
    let binding = match migration.root.kind {
        RootKind::External => Some(binding_for(
            context,
            migration.root.volume_uuid.as_deref().unwrap_or_default(),
        )?),
        RootKind::Local => None,
    };
    validate_root_device(&context.workspace, &selected, binding.as_ref())?;
    let successor = Marker {
        version: MARKER_VERSION.to_owned(),
        nonce: Some(nonce.to_owned()),
        ..before.clone()
    };
    let path = spec.root().join(MARKER);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            return Err(pending_migration(
                migration,
                &format!("{} appeared before publication", path.display()),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(pending_migration(
                migration,
                &format!("cannot inspect {}: {error}", path.display()),
            ))
        }
    }
    let bytes = serde_json::to_vec_pretty(&successor)
        .map_err(|error| format!("cannot encode the output root marker: {error}"))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err("the output root marker exceeds its byte limit".to_owned());
    }
    let temp = spec
        .root()
        .join(format!(".{MARKER}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    write_private(&temp, &bytes)?;
    rename_no_replace(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output root marker: {error}")
    })?;
    sync_directory(spec.root())?;
    Ok(successor)
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
    /// A command's own relink: a missing destination link or the receipt's
    /// own link. Only the legacy whole-target layout may still replace an
    /// empty real directory here.
    Any,
    /// `oxide outputs adopt`: like [`Start::Any`], and additionally the only
    /// way a real empty profile member directory is converted.
    Adopt,
    /// The automatic first conversion of oxide#121: a positively absent target
    /// only. An empty directory, a link and anything else refuse.
    MissingOnly,
}

/// The nonce authority one transition runs under (oxide#152).
///
/// A leased command's slot is keyed with the destination nonce it committed to
/// today, and that keyed nonce is the only authority a transition may bind: a
/// cancelled, replaced or absent reservation refuses before the prepared
/// receipt, the root, the link or any registry row exists. Only an explicit
/// adoption (or the automatic first conversion) never keyed a slot, so only it
/// may draw a fresh nonce for a root it decides to create.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NonceAuthority<'a> {
    /// A keyed command's destination nonce (its reservation nonce, or `None`
    /// when no reservation existed when the slot was keyed).
    Keyed(Option<&'a str>),
    /// No slot was keyed: an explicit adoption or an automatic conversion.
    Unkeyed,
}

/// Write the committed receipt for one transition (the legacy whole-target
/// link).
fn transition(
    context: &Context,
    receipt: &Receipt,
    spec: &RootSpec,
    start: Start,
    authority: NonceAuthority<'_>,
) -> Result<Receipt, String> {
    transition_at(context, receipt, MEMBER, spec, start, authority)
}

/// Write the committed receipt for one member's transition: the legacy
/// `target` link, or one profile member link below the real `target`
/// directory.
fn transition_at(
    context: &Context,
    receipt: &Receipt,
    member: &str,
    spec: &RootSpec,
    start: Start,
    authority: NonceAuthority<'_>,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    crate::prune::require_registry(control)?;
    let link = link_path(&context.workspace, member);
    let profile_member = member != MEMBER;
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    // Only the receipt's own owned link, a missing destination, or (for the
    // explicit conversion) an empty real directory may be replaced. A build
    // never converts a real profile directory: only `oxide outputs adopt`
    // does, through `Start::Adopt`.
    let target = link_state(&link, &context.workspace, &context.checkout_id)?;
    let committed = receipt.selected_for(member);
    let converts_empty =
        target == TargetState::RealEmpty && (!profile_member || start == Start::Adopt);
    // A first conversion publishes an owned link, so Git must ignore that
    // exact path first — before the prepared receipt, the root or either link
    // exists. Later generations keep their committed link's eligibility and
    // are not re-guarded.
    let first_link = committed.is_none() && (target == TargetState::Missing || converts_empty);
    if first_link {
        refuse_link_git_exposure_at(context, &link, profile_member.then_some(member))?;
    }
    // The publish below replaces nothing when the link was absent (or an empty
    // directory was just removed): a creator that appears in between is kept.
    let exclusive = matches!(target, TargetState::Missing) || converts_empty;
    match (&target, committed, start) {
        (TargetState::Missing, _, _) => {}
        (TargetState::RealEmpty, _, Start::Adopt) if profile_member => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty profile directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (TargetState::RealEmpty, _, Start::Any) if !profile_member => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty target directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (_, Some(selected), Start::Any | Start::Adopt)
            if target.link_root()
                == Some(member_destination(selected.root(), member).as_path()) => {}
        (_, _, Start::MissingOnly) => {
            return Err(format!(
                "outputs: auto-adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed",
                link.display(),
                target.label()
            ))
        }
        (TargetState::RealEmpty, _, Start::Any) if profile_member => {
            return Err(format!(
                "outputs: {} is an empty real directory; only an explicit `oxide outputs adopt` converts it. It is used locally for this command",
                link.display()
            ))
        }
        _ => return Err(refuse_existing_target_at(&target, &link)),
    }
    // A newly created root gets a fresh random nonce; an existing root keeps
    // the nonce in its marker. A missing root is never re-created with the
    // receipt's old nonce (oxide#129). A selection keys the root's OWN
    // immutable creation generation, which is the placement generation only
    // for the transition that creates the root; a member joining an existing
    // root must not claim a newer one.
    let existing_marker = read_marker(spec.root()).ok();
    let existing_nonce = existing_marker
        .as_ref()
        .and_then(|marker| marker.nonce.clone());
    let root_generation = existing_marker
        .as_ref()
        .map_or(generation, |marker| marker.generation);
    // A future root takes the nonce reserved for exactly it, after the
    // reservation is validated against the registry (or its overflow tag).
    // A slot that was KEYED for a future root binds only the exact nonce it
    // named: a cancelled, replaced or absent reservation, or a root that now
    // carries another nonce, refuses here — before the prepared receipt, the
    // root, the link or any registry write — so a command that refuses leaves
    // no authority effect (oxide#152). Only an explicit adoption without a
    // keyed slot draws a fresh one.
    let reserved = receipt
        .reserved()
        .filter(|reserved| reserved.is_for(&spec.selected(generation)))
        .cloned();
    let nonce = match existing_nonce {
        Some(nonce) => {
            if let NonceAuthority::Keyed(keyed) = authority {
                if keyed != Some(nonce.as_str()) {
                    return Err(format!(
                        "outputs: this command's slot was keyed for root nonce {} of {}, but the root now carries {nonce}; nothing was built. Run the command again",
                        keyed.unwrap_or("(none)"),
                        spec.root().display()
                    ));
                }
            }
            Some(nonce)
        }
        None if read_marker(spec.root()).is_ok() => None,
        None => match authority {
            NonceAuthority::Keyed(keyed) => {
                let wanted = spec
                    .selected(root_generation)
                    .with_nonce(keyed.map(str::to_owned));
                reservation_matches(context, receipt, &wanted)?;
                wanted.nonce().map(str::to_owned)
            }
            NonceAuthority::Unkeyed => match &reserved {
                Some(reserved) => {
                    crate::prune::validate_reservation(control, reserved)?;
                    Some(reserved.nonce.clone())
                }
                None => Some(crate::prune::fresh_nonce()?),
            },
        },
    };
    let to = spec.selected(root_generation).with_nonce(nonce);
    // Write-ahead: prepared first, then the destination root, then the
    // sibling link, then the rename, then the committed receipt.
    let mut writing = receipt.clone();
    writing.state = ReceiptState::Prepared;
    writing.place_member(member).prepared = Some(Prepared {
        generation,
        to: to.clone(),
        link: link.clone(),
    });
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    crash_point("prepared")?;

    ensure_root_with(context, spec, generation, to.nonce())?;
    ensure_member_directory(&to.root, member)?;
    ensure_member_ancestors(context, &link)?;
    let sibling = sibling_link(&link, generation);
    match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(&member_destination(&to.root, member), &sibling).map_err(|error| {
                format!(
                    "cannot create the transition link {}: {error}",
                    sibling.display()
                )
            })?;
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing = fs::read_link(&sibling)
                .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
            if existing != member_destination(&to.root, member) {
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
    if sibling_read != member_destination(&to.root, member) {
        return Err(format!(
            "outputs: {} does not read back as the transition root",
            sibling.display()
        ));
    }
    sync_directory(link.parent().unwrap_or(&context.workspace))?;
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
    sync_directory(link.parent().unwrap_or(&context.workspace))?;
    crash_point("renamed")?;
    let prepared = writing
        .prepared_ref()
        .clone()
        .ok_or("the prepared receipt lost its transition")?;
    // Commit from the write-ahead receipt: it is the one that carries this
    // placement, which a first member link adds here.
    finish_commit(context, &writing, &prepared)
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
    let link = prepared.link.clone();
    let member = receipt
        .placement_for_link(&link)
        .map(|placement| placement.member.clone())
        .ok_or("the prepared transition names no placement of this receipt")?;
    let expected = member_destination(&prepared.to.root, &member);
    let live =
        fs::read_link(&link).map_err(|error| format!("cannot read {}: {error}", link.display()))?;
    if live != expected {
        return Err(format!(
            "outputs: {} selects {}, not the prepared {}; both roots are kept",
            link.display(),
            live.display(),
            expected.display()
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
    ensure_member_directory(&prepared.to.root, &member)?;
    // The committed selection carries the root's own creation generation, not
    // the placement revision that committed it.
    let selected = prepared
        .to
        .at(marker.generation)
        .with_nonce(marker.nonce.clone());
    let mut committed = receipt.clone();
    committed.state = ReceiptState::Committed;
    committed.generation = prepared.generation;
    let placement = committed
        .placement_for_link_mut(&link)
        .ok_or("the prepared transition names no placement of this receipt")?;
    placement.selected = Some(selected.clone());
    placement.prepared = None;
    // The commit that creates the reserved root consumes its reservation.
    if committed.reserved().is_some_and(|reserved| {
        reserved.root == selected.root && Some(reserved.nonce.as_str()) == selected.nonce()
    }) {
        *committed.reserved_mut() = None;
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
    if receipt.transition.is_some() {
        return Err(
            "outputs: an explicit layout migration is pending; `oxide outputs adopt` replays its exact recorded phases and every other command refuses until it commits or rolls back"
                .to_owned(),
        );
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
    // recovery bypass. A receipt that already committed this placement keeps
    // it.
    let member = receipt
        .placement_for_link(&prepared.link)
        .map(|placement| placement.member.clone())
        .ok_or_else(|| {
            stop_unknown(
                &prepared,
                "its link is not this checkout's target or a profile member of this receipt",
            )
        })?;
    let committed_here = receipt.selected_for(&member).is_some();
    if !committed_here {
        refuse_link_git_exposure_at(
            context,
            &prepared.link,
            (member != MEMBER).then_some(member.as_str()),
        )?;
    }
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    // Every replay write records into the registry: refuse before the first
    // effect when it is not ready.
    crate::prune::require_registry(control)?;
    // The prepared link is validated to be this checkout's own link path; the
    // rename destination comes from the receipt's placement, never from a
    // file's contents.
    let link = prepared.link.clone();
    if link_path(&context.workspace, &member) != link {
        return Err(stop_unknown(
            &prepared,
            "its link is not this checkout's link for that placement",
        ));
    }
    let target = link_state(&link, &context.workspace, &context.checkout_id)?;
    let expected = member_destination(&prepared.to.root, &member);
    let sibling = sibling_link(&link, prepared.generation);
    // The sibling is classified before any replay mutation: positively
    // absent, or exactly the prepared link. Anything else (another kind, a
    // link to another root, unreadable) is unknown transition state and
    // stops with the receipt, links and roots unchanged.
    let sibling_live = match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::read_link(&sibling) {
            Ok(read) if read == expected => true,
            Ok(read) => {
                return Err(stop_unknown(
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
                    &prepared,
                    &format!("cannot read {}: {error}", sibling.display()),
                ))
            }
        },
        Ok(_) => {
            return Err(stop_unknown(
                &prepared,
                &format!("{} exists and is not a link", sibling.display()),
            ))
        }
        Err(error) => {
            return Err(stop_unknown(
                &prepared,
                &format!("cannot inspect {}: {error}", sibling.display()),
            ))
        }
    };
    let owned_new = owned_root(&context.workspace, &context.checkout_id, &prepared.to.root).is_ok();
    let renamed = target.link_root() == Some(expected.as_path());
    // After the rename the sibling no longer exists: a live sibling beside an
    // already-renamed target is contradictory.
    if renamed && sibling_live {
        return Err(stop_unknown(
            &prepared,
            "the target already selects the prepared root but the prepared sibling still exists",
        ));
    }
    // A first conversion has no committed selection, so its only valid old
    // state is a missing target: a link that appeared after the crash (even a
    // dangling one) is not this receipt's and is never renamed over. A
    // prepared receipt keeps the selection it is replacing in `selected`,
    // which is why the stored field is read here rather than the accessor.
    let old_live = match (&target, receipt.selected_for(&member)) {
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
                .map_err(|reason| stop_unknown(&prepared, &reason))?,
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
        publish_link(&sibling, &link, exclusive).map_err(|error| {
            format!(
                "cannot replay the prepared link rename {}: {error}; both roots are kept",
                link.display()
            )
        })?;
        sync_directory(link.parent().unwrap_or(&context.workspace))?;
        let live = fs::read_link(&link)
            .map_err(|error| format!("cannot read {}: {error}", link.display()))?;
        if live != expected {
            return Err(stop_unknown(
                &prepared,
                "the replayed link does not read back as the prepared member destination",
            ));
        }
        return finish_commit(context, &receipt, &prepared);
    }
    // 3. Nothing was published: roll the preparation back, keeping the old
    //    selection and any root the attempt created. Only a positively absent
    //    sibling means nothing was published.
    if sibling_live {
        return Err(stop_unknown(
            &prepared,
            "the prepared sibling exists but neither its rename nor a rollback is safe",
        ));
    }
    if !old_live {
        return Err(stop_unknown(
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
    if let Some(placement) = rolled.placement_for_link_mut(&link) {
        placement.prepared = None;
    }
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

fn stop_unknown(prepared: &Prepared, reason: &str) -> String {
    format!(
        "outputs: the prepared transition to generation {} cannot be replayed: {reason}. Both roots and {} are kept exactly as they are; inspect `oxide outputs status` and the file system before acting",
        prepared.generation,
        prepared.link.display()
    )
}

/// Whether a receipt reader accepts the version-1 whole-target document.
/// Prune reads any valid document (oxide#152) and explicit adopt reads a
/// committed one to upgrade it (oxide#163); every other path keeps refusing
/// it, so decoding never widens a bind, detach, clean, replay, status or plan
/// format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyReceiptReading {
    /// Ordinary readers refuse a version-1 receipt, as before oxide#152.
    Refuse,
    /// Prune readers decode a valid version-1 receipt into the current
    /// in-memory whole-target form. Reading writes nothing.
    Read,
    /// Explicit `oxide outputs adopt` decodes a valid COMMITTED version-1
    /// receipt into the current in-memory whole-target form so its shipped
    /// whole-target-to-profile migration can run. A noncommitted version-1
    /// document stays refused, and reading still writes nothing.
    Upgrade,
}

/// The lowercase state name a receipt document carries.
fn receipt_state_label(state: ReceiptState) -> &'static str {
    match state {
        ReceiptState::Committed => "committed",
        ReceiptState::Prepared => "prepared",
        ReceiptState::Detached => "detached",
        ReceiptState::Pruned => "pruned",
        ReceiptState::Pruning => "pruning",
    }
}

/// Only the version field, so a reader can decide about a version-1 document
/// before decoding the rest of it.
#[derive(Deserialize)]
struct ReceiptVersion {
    version: String,
}

fn read_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt_with(LegacyReceiptReading::Refuse, control, checkout, checkout_id)
}

/// Read one checkout's receipt for explicit `oxide outputs adopt`, which is
/// the ONE command that upgrades a committed version-1 whole-target receipt
/// (oxide#163). Every other reader keeps refusing it.
fn read_receipt_for_adopt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt_with(
        LegacyReceiptReading::Upgrade,
        control,
        checkout,
        checkout_id,
    )
}

/// Read one checkout's receipt without creating or repairing anything.
/// `legacy` decides whether a valid version-1 whole-target document is read or
/// refused; every reader keeps the byte limit, the unknown-field strictness and
/// the checkout identity check unchanged.
fn read_receipt_with(
    legacy: LegacyReceiptReading,
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    let path = receipt_path(control, checkout_id);
    let Some(file) = store::private_file(&path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    crate::prune::authority_read(file.take(MAX_RECEIPT_BYTES + 1), &mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(format!(
            "{} exceeds its {MAX_RECEIPT_BYTES}-byte limit",
            path.display()
        ));
    }
    let version: ReceiptVersion = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    let legacy_document = version.version == LEGACY_RECEIPT_VERSION;
    if legacy_document && legacy == LegacyReceiptReading::Refuse {
        return Err(format!(
            "{} is invalid: unsupported version {LEGACY_RECEIPT_VERSION:?} of the owned-output receipt; run `oxide outputs adopt` to upgrade a committed version-1 receipt",
            path.display()
        ));
    }
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    receipt.validate(checkout, checkout_id)?;
    // A version-1 document records no transition journal, so only its
    // committed form can be upgraded: a detached or prepared one stays
    // refused and nothing is rewritten.
    if legacy_document
        && legacy == LegacyReceiptReading::Upgrade
        && receipt.state != ReceiptState::Committed
    {
        return Err(format!(
            "{} is invalid: only a committed version-1 receipt can be upgraded, and this one is {}. Nothing was changed",
            path.display(),
            receipt_state_label(receipt.state)
        ));
    }
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
/// repairs anything. A valid version-1 whole-target receipt is read as its
/// current whole-target form (oxide#152); its bytes stay exactly as they are.
pub(crate) fn read_receipt_for_prune(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt_with(LegacyReceiptReading::Read, control, checkout, checkout_id)
}

/// The SHA-256 of the exact marker bytes, for the recorded root identity.
pub(crate) fn marker_digest(root: &Path) -> Result<String, String> {
    let path = root.join(MARKER);
    let file =
        File::open(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    crate::prune::authority_read(file.take(MAX_MARKER_BYTES + 1), &mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
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
/// Publish the receipt's terminal prune image with an exact FIXED timestamp.
pub(crate) fn mark_pruned_at(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
    outcome: &str,
    at_ms: u64,
) -> Result<(), String> {
    let receipt = read_receipt_with(LegacyReceiptReading::Read, control, checkout, checkout_id)?
        .ok_or_else(|| "the checkout's receipt disappeared before its prune receipt".to_owned())?;
    let terminal = pruned_receipt(receipt, intent, outcome, at_ms)?;
    if terminal.pruned.as_ref().is_none() {
        // Already the terminal image for this exact intent: nothing to write.
        return Ok(());
    }
    write_receipt(control, &terminal)
}

/// The exact terminal receipt image for one prune intent, with its fixed
/// timestamp. The caller persists its digest before publishing it.
pub(crate) fn terminal_receipt_image(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
    outcome: &str,
    at_ms: u64,
) -> Result<Vec<u8>, String> {
    let receipt = read_receipt_with(LegacyReceiptReading::Read, control, checkout, checkout_id)?
        .ok_or_else(|| "the checkout's receipt disappeared before its prune receipt".to_owned())?;
    let terminal = pruned_receipt(receipt, intent, outcome, at_ms)?;
    serde_json::to_vec_pretty(&terminal)
        .map_err(|error| format!("cannot encode the terminal receipt image: {error}"))
}

/// Apply the terminal prune state to one receipt, clearing every owned
/// placement of the deleted root and preserving all other placements,
/// selections and history. This is the ONE place a version-1 receipt's
/// in-memory whole-target form becomes a current-format document: it runs only
/// for the exact root the real run prunes, after the normal custody checks,
/// and its bytes are pinned in the intent before publication (oxide#152).
fn pruned_receipt(
    mut receipt: Receipt,
    intent: &str,
    outcome: &str,
    at_ms: u64,
) -> Result<Receipt, String> {
    if receipt.state == ReceiptState::Pruned {
        if receipt
            .pruned
            .as_ref()
            .is_some_and(|record| record.intent == intent)
        {
            return Ok(receipt);
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
    if receipt.state == ReceiptState::Prepared {
        return Err(
            "the receipt has a pending transition; a prune terminal image refuses".to_owned(),
        );
    }
    match receipt.current().map(|selected| selected.root.clone()) {
        Some(root) => {
            // Clear only the deleted root's placements; another root's
            // selection and unrelated history stay exactly as they are.
            for placement in &mut receipt.links {
                if placement
                    .selected
                    .as_ref()
                    .is_some_and(|selected| selected.root == root)
                {
                    placement.selected = None;
                    placement.prepared = None;
                }
            }
            if receipt
                .links
                .iter()
                .all(|placement| placement.selected.is_none())
            {
                receipt.state = ReceiptState::Pruned;
            }
        }
        None => {
            // A receiptless legacy root's pruning receipt selects nothing; its
            // terminal image is still this intent's evidence.
            receipt.state = ReceiptState::Pruned;
        }
    }
    receipt.pruned = Some(PruneRecord {
        intent: intent.to_owned(),
        outcome: outcome.to_owned(),
        at_ms,
    });
    receipt.updated_ms = at_ms;
    Ok(receipt)
}

/// Exclusively create the `pruning` receipt that protects an operator-listed
/// receiptless legacy root before its prune intent is journaled (oxide#129).
/// Under the internal publication lock: no receipt may exist, except this
/// writer's own interrupted creation (a `pruning` receipt whose intent no
/// registry journal records), which is rebound to the new intent. Any other
/// existing receipt refuses. No adoption history is reconstructed.
///
/// The publication is compare-and-set on the registry revision: it happens
/// only while the registry still has `expected_revision` (the revision the
/// locked recheck read) and returns the exact revision its own publication
/// produced, read under the same internal publication lock. `Ok(None)`
/// means an unrelated writer changed the registry first: nothing was
/// written.
pub(crate) fn create_pruning_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
    intent: &str,
    journaled: &dyn Fn(&str) -> bool,
    expected_revision: u64,
) -> Result<Option<u64>, String> {
    let _guard = store::publication_guard(control)?;
    if crate::prune::registry_revision(control)? != expected_revision {
        return Ok(None);
    }
    let mut receipt = match read_receipt_with(
        LegacyReceiptReading::Read,
        control,
        checkout,
        checkout_id,
    )? {
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
    *receipt.reserved_mut() = None;
    receipt.pruned = Some(PruneRecord {
        intent: intent.to_owned(),
        outcome: "pruning".to_owned(),
        at_ms: process::now_ms(),
    });
    receipt.updated_ms = process::now_ms();
    write_receipt(control, &receipt)?;
    // Still under the publication lock: this is exactly our own revision.
    crate::prune::registry_revision(control).map(Some)
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
    let file =
        File::open(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    crate::prune::authority_read(file.take(MAX_MARKER_BYTES + 1), &mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
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
/// The ignore prerequisite for one link path: the legacy whole-target link
/// (`suffix` `None`) or one profile member below `target/` (`suffix` is the
/// member). A profile member is a path inside the real `target` directory, so
/// a winning parent rule such as `/target/` genuinely ignores it and is
/// accepted; the legacy link is itself replaced, so a directory-only rule
/// cannot cover it.
fn refuse_link_git_exposure_at(
    context: &Context,
    link: &Path,
    suffix: Option<&str>,
) -> Result<(), String> {
    let link = link.to_path_buf();
    let answer = first_link_git_answer(context, suffix);
    let Some(fix) = answer.fix.as_deref() else {
        let LinkGitVerdict::Unknown(reason) = &answer.verdict else {
            // A confirmed work tree always yields the exact rule to add.
            return Err("outputs: the ignore prerequisite answered without a fix".to_owned());
        };
        return Err(link_git_unknown_refusal(&link, reason, None));
    };
    let relative = answer.relative.as_deref().unwrap_or_default();
    match &answer.verdict {
        LinkGitVerdict::Eligible => Ok(()),
        LinkGitVerdict::Tracked => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git already tracks `{relative}`, so the published link would be a tracked path. Remove it from the index (`git rm --cached {relative}`) and {fix}; nothing was changed",
            link.display()
        )),
        LinkGitVerdict::Unignored => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git does not ignore `{relative}`, so the published link would appear as an untracked path. {fix}; nothing was changed",
            link.display()
        )),
        LinkGitVerdict::Negated(rule) => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) re-includes `{relative}`. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        )),
        LinkGitVerdict::DirectoryOnly(rule) => Err(format!(
            "outputs: refusing to convert {} to an owned link: Git's winning rule `{}` ({}:{}) matches directories only, so it would not cover the published link. {fix}; nothing was changed",
            link.display(),
            rule.pattern,
            rule.source,
            rule.line
        )),
        LinkGitVerdict::Unknown(reason) => Err(link_git_unknown_refusal(&link, reason, Some(fix))),
    }
}

/// One positive answer from the ignore prerequisite, with the exact rule to
/// add and the repository-relative link path Git's work-tree answer makes
/// available. Both are `None` only when the work tree itself could not be
/// confirmed.
#[derive(Debug)]
struct LinkGitAnswer {
    verdict: LinkGitVerdict,
    relative: Option<String>,
    fix: Option<String>,
    /// The repository `.gitignore` the fix names, when Git confirmed the work
    /// tree. `None` only with an unconfirmed work tree.
    gitignore: Option<PathBuf>,
}

/// What Git positively established about one exact non-directory link path.
#[derive(Debug, Eq, PartialEq)]
enum LinkGitVerdict {
    /// Untracked, and its winning rule covers the link.
    Eligible,
    /// Git's index already tracks the path.
    Tracked,
    /// No rule ignores the path.
    Unignored,
    /// The winning rule re-includes the path.
    Negated(IgnoreRule),
    /// The winning rule matches directories only, so it never covers a link.
    DirectoryOnly(IgnoreRule),
    /// Git could not answer; unknown is never permission.
    Unknown(String),
}

/// The ignore prerequisite's answer for one link path (oxide#125): the legacy
/// whole-target link (`suffix` `None`) or one profile member below `target/`
/// (`suffix` is the member). A profile member is a path inside the real
/// `target` directory, so a winning parent rule such as `/target/` genuinely
/// ignores it and is accepted; the legacy link is itself replaced, so a
/// directory-only rule cannot cover it.
///
/// This is a positive answer rather than a message because three callers ask
/// the same question with different consequences: a conversion refuses
/// (oxide#125), an automatic first conversion treats it as ineligibility
/// (oxide#154), and `oxide init` names the exact rule to add without editing
/// anything (oxide#154).
fn first_link_git_answer(context: &Context, suffix: Option<&str>) -> LinkGitAnswer {
    let (root, relative) = match git_link_path(context, suffix) {
        Ok(found) => found,
        Err(reason) => {
            return LinkGitAnswer {
                verdict: LinkGitVerdict::Unknown(reason),
                relative: None,
                fix: None,
                gitignore: None,
            }
        }
    };
    let fix = format!(
        "Ignore the non-directory path: add `/{relative}` (or `{relative}`), not `{relative}/`, to {}",
        root.join(".gitignore").display()
    );
    let verdict = first_link_git_verdict(&root, &relative, suffix.is_some())
        .unwrap_or_else(LinkGitVerdict::Unknown);
    LinkGitAnswer {
        verdict,
        relative: Some(relative),
        fix: Some(fix),
        gitignore: Some(root.join(".gitignore")),
    }
}

/// Ask Git about the exact non-directory path in a confirmed work tree. Every
/// error is an unknown answer, never permission.
fn first_link_git_verdict(
    root: &Path,
    relative: &str,
    directory_rule_covers: bool,
) -> Result<LinkGitVerdict, String> {
    // A tracked path would publish a tracked symlink once the link is there.
    let tracked = git_probe(root, &["ls-files", "-z", "--", relative], &[], true)?;
    if tracked.code != Some(0) {
        return Err(format!(
            "git ls-files did not answer ({})",
            exit_label(tracked.code)
        ));
    }
    if !tracked.stdout.is_empty() {
        return Ok(LinkGitVerdict::Tracked);
    }
    // The exact non-directory path, decided by Git itself: `--no-index`
    // reads the rules rather than the index (tracked is refused above), and
    // `-v` names the winning rule, whose form is part of the answer.
    let input = format!("{relative}\0");
    let check = git_probe(
        root,
        &["check-ignore", "--no-index", "-v", "-z", "--stdin"],
        input.as_bytes(),
        false,
    )?;
    let rule = match check.code {
        Some(0) => parse_ignore_rule(&check.stdout, relative)?,
        Some(1) => return Ok(LinkGitVerdict::Unignored),
        code => {
            return Err(format!(
                "git check-ignore did not answer ({})",
                exit_label(code)
            ))
        }
    };
    if rule.pattern.starts_with('!') {
        return Ok(LinkGitVerdict::Negated(rule));
    }
    if !directory_rule_covers && (rule.pattern.ends_with('/') || rule.pattern.ends_with("\\/")) {
        return Ok(LinkGitVerdict::DirectoryOnly(rule));
    }
    Ok(LinkGitVerdict::Eligible)
}

/// Why a published link would be unsafe for this exact path, as one sentence
/// without the fix. `None` when Git positively confirmed the path.
fn link_git_problem(answer: &LinkGitAnswer) -> Option<String> {
    let relative = answer.relative.as_deref().unwrap_or("target");
    Some(match &answer.verdict {
        LinkGitVerdict::Eligible => return None,
        LinkGitVerdict::Tracked => format!(
            "Git already tracks `{relative}`, so the published link would be a tracked path; remove it from the index with `git rm --cached {relative}`"
        ),
        LinkGitVerdict::Unignored => format!(
            "Git does not ignore `{relative}`, so the published link would appear as an untracked path"
        ),
        LinkGitVerdict::Negated(rule) => format!(
            "Git's winning rule `{}` ({}:{}) re-includes `{relative}`",
            rule.pattern, rule.source, rule.line
        ),
        LinkGitVerdict::DirectoryOnly(rule) => format!(
            "Git's winning rule `{}` ({}:{}) matches directories only, so it would not cover the published link",
            rule.pattern, rule.source, rule.line
        ),
        LinkGitVerdict::Unknown(reason) => format!(
            "Git could not confirm its decision for the exact non-directory path ({reason})"
        ),
    })
}

/// `oxide init`'s `.gitignore` diagnostic for a first conversion (oxide#154).
///
/// A first permanent-drive conversion publishes owned `<checkout>/target/<member>`
/// links only when Git ignores each exact repository-relative path, so this
/// asks the same prerequisite for the first member link the automatic
/// conversion would publish (`target/debug`) — the same question the creator
/// asks, which is why a directory-only rule such as `target/` that covers the
/// member is accepted and stays silent. Rather than rewrite a user's rule,
/// `oxide init` names the exact rule to add (for a nested workspace, its
/// repository-relative spelling). It is effect-free: the same bounded Git
/// probe the conversion uses, and no write of any kind. `None` when Git
/// positively confirmed the path, or when this checkout is not inside a Git
/// work tree at all (init adopts it outside Git, where nothing is ignored and
/// nothing is converted).
pub fn gitignore_diagnostic(workspace: &Path) -> Option<String> {
    let context = Context::new(None, workspace.to_path_buf());
    let link = context.link();
    let answer = first_link_git_answer(&context, Some(AUTO_ADOPT_MEMBERS[0]));
    let problem = link_git_problem(&answer)?;
    // No relative path or .gitignore means Git never confirmed a work tree.
    let member = answer.relative.as_deref()?;
    let gitignore = answer.gitignore.as_deref()?;
    // The rule that covers every member link this first conversion publishes:
    // the member's parent path (`/target`, or a nested `/rust/target`).
    let parent = member.rsplit_once('/').map_or(member, |(parent, _)| parent);
    Some(format!(
        "permanent-drive outputs will not convert {} yet: {problem}. Add `/{parent}` (or `{parent}`) to {}: it covers the `{member}` member link this first conversion publishes. `oxide init` leaves every .gitignore rule exactly as it is",
        link.display(),
        gitignore.display()
    ))
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
fn git_link_path(context: &Context, suffix: Option<&str>) -> Result<(PathBuf, String), String> {
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
    if let Some(suffix) = suffix {
        for component in suffix.split('/') {
            if !crate::layout::Member::parse(suffix).is_ok_and(|member| member.name() == suffix)
                || component.is_empty()
            {
                return Err("the profile member is not a plain relative path".to_owned());
            }
            parts.push(component.to_owned());
        }
    }
    Ok((root, parts.join("/")))
}

/// One winning ignore rule from `git check-ignore --no-index -v -z --stdin`:
/// `<source>\0<line>\0<pattern>\0<pathname>\0`.
#[derive(Debug, Eq, PartialEq)]
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
    let deadline = Instant::now() + crate::budget::clamp(GIT_PROBE_TIME);
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
                let mut probe_command = Command::new("git");
                probe_command
                    .arg("-C")
                    .arg(checkout)
                    .args(["rev-parse", "--is-inside-work-tree"]);
                let probe = crate::budget::output(&mut probe_command);
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
    /// The bound profile layout: every owned member link must survive a
    /// whole clean, and its real root's payload is what gets cleaned.
    members: Vec<(PathBuf, PathBuf)>,
}

impl CleanPlacement {
    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn keep_root(&self) -> bool {
        self.keep_root
    }

    /// Owned member links and their real roots, for the profile layout.
    pub fn members(&self) -> &[(PathBuf, PathBuf)] {
        &self.members
    }

    /// Every path a whole clean must keep, or nothing when this is not a
    /// profile placement. In the profile layout `target` is a REAL directory,
    /// so it is kept as well as each owned member link and each real ancestor
    /// directory between the target and a nested link (a triple directory).
    /// Keeping a directory never stops the walk inside it.
    pub fn preserved(&self) -> Vec<PathBuf> {
        if self.members.is_empty() {
            return Vec::new();
        }
        let mut paths = vec![self.target.clone()];
        for (link, _) in &self.members {
            if !paths.iter().any(|known| known == link) {
                paths.push(link.clone());
            }
            let mut ancestor = link.parent();
            while let Some(directory) = ancestor {
                if directory == self.target.as_path() || !directory.starts_with(&self.target) {
                    break;
                }
                if !paths.iter().any(|known| known == directory) {
                    paths.push(directory.to_path_buf());
                }
                ancestor = directory.parent();
            }
        }
        paths
    }

    /// What the clean keeps, for its report; empty for an ephemeral checkout.
    pub fn preserved_note(&self) -> Option<String> {
        if !self.members.is_empty() {
            return Some(format!(
                "{} profile member link(s) and their ownership marker are kept; each owned member root's payload is cleaned instead: {}",
                self.members.len(),
                self.members
                    .iter()
                    .map(|(link, root)| format!("{} -> {}", link.display(), root.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
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
    // A guarded workspace keeps clean's refusal but loses its adopt advice
    // (design §4.4): the guarded diagnostic is the only text, and nothing is
    // changed. Every normal checkout keeps the shipped wording.
    let guarded = guarded_workspace(control.as_deref(), workspace).is_some();
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
        ReceiptState::Prepared => Err(if guarded {
            crate::one_use::PREPARED_GUARDED.to_owned()
        } else {
            "clean refused: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before cleaning"
                .to_owned()
        }),
        ReceiptState::Committed => {
            // The profile layout is validated BEFORE the whole-target link
            // pre-gate: `target` is a real directory there, never the link the
            // legacy path requires.
            if receipt.layout() == LayoutKind::Profile {
                let target = context.target()?;
                if !matches!(target, TargetState::RealEmpty | TargetState::RealNonEmpty) {
                    return Err(format!(
                        "clean refused: {} is {}, not a real profile target directory",
                        context.link().display(),
                        target.label()
                    ));
                }
                let mut members = Vec::new();
                let mut validated: Vec<PathBuf> = Vec::new();
                for placement in receipt.links() {
                    let Some(selected) = placement.selected_ref() else {
                        continue;
                    };
                    let link = link_path(workspace, placement.member());
                    let destination = member_destination(selected.root(), placement.member());
                    match link_state(&link, workspace, &context.checkout_id)? {
                        TargetState::OwnedLink(root) if root == destination => {
                            if !validated.iter().any(|known| known == selected.root()) {
                                let marker = read_marker(selected.root())?;
                                check_marker(&marker, workspace, &context.checkout_id)?;
                                if marker.kind != selected.kind
                                    || marker.volume_uuid != selected.volume_uuid
                                {
                                    return Err(format!(
                                        "clean refused: {} carries a marker for another root kind or volume",
                                        selected.root().display()
                                    ));
                                }
                                if marker.generation > receipt.generation {
                                    return Err(format!(
                                        "clean refused: {} carries generation {} ahead of the receipt's {}",
                                        selected.root().display(),
                                        marker.generation,
                                        receipt.generation
                                    ));
                                }
                                let binding = match selected.kind {
                                    RootKind::External => Some(binding_for(&context, selected.volume_uuid.as_deref().unwrap_or_default()).map_err(|reason| {
                                        format!(
                                            "clean refused: the registered volume for {} is not usable ({reason})",
                                            selected.root().display()
                                        )
                                    })?),
                                    RootKind::Local => None,
                                };
                                validate_root_device(workspace, selected, binding.as_ref())
                                    .map_err(|reason| format!("clean refused: {reason}"))?;
                                validated.push(selected.root().to_path_buf());
                            }
                            if !members.iter().any(|(_, known): &(PathBuf, PathBuf)| known == &destination) {
                                members.push((link, destination));
                            }
                        }
                        TargetState::Missing => {}
                        state => {
                            return Err(format!(
                                "clean refused: {} is {}, not this checkout's owned member link; Oxide never cleans through a foreign link",
                                link.display(),
                                state.label()
                            ))
                        }
                    }
                }
                let inactive = receipt
                    .roots
                    .iter()
                    .filter(|root| {
                        !validated.iter().any(|known| known == &root.root) && root.root.is_dir()
                    })
                    .map(|root| root.root.clone())
                    .collect();
                return Ok(Some(CleanPlacement {
                    target: context.link(),
                    keep_root: false,
                    link: context.link(),
                    inactive,
                    members,
                }));
            }
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
                members: Vec::new(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    /// The debug-only migration environment is process-global, so the tests
    /// that set it run one at a time.
    static MIGRATION_ENVIRONMENT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// P5: the pinned receipt preimage is a SHA-256 digest and nothing else.
    /// These are pure string-shape assertions and touch no file system.
    #[test]
    fn migration_receipt_digest_is_strictly_sha256() {
        assert!(is_receipt_digest(&"a".repeat(64)));
        assert!(is_receipt_digest(&"0123456789abcdef".repeat(4)));
        // Empty and wrong lengths, in both directions.
        assert!(!is_receipt_digest(""));
        assert!(!is_receipt_digest(&"a".repeat(63)));
        assert!(!is_receipt_digest(&"a".repeat(65)));
        // A 128-bit root nonce is a different value type, not a digest.
        assert!(!is_receipt_digest(&"a".repeat(32)));
        assert!(!is_receipt_digest(&"0123456789abcdef".repeat(2)));
        // 64 characters that are not hex, and uppercase hex.
        assert!(!is_receipt_digest(&"g".repeat(64)));
        assert!(!is_receipt_digest(&"A".repeat(64)));
        assert!(!is_receipt_digest(&"aB".repeat(32)));
        // Malformed separators or prefixes never stand in for a digest.
        assert!(!is_receipt_digest(&format!("sha256:{}", "a".repeat(57))));
        assert!(!is_receipt_digest(&format!("{}\n", "a".repeat(63))));
    }

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

    /// A legacy whole-target plan around one selected root, for the binding
    /// tests that predate profile members.
    fn whole_target_plan(selected: Option<Selected>) -> Plan {
        Plan {
            whole_target: true,
            members: vec![PlannedMember {
                member: MEMBER.to_owned(),
                selected,
            }],
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

    fn context_id(fixture: &Fixture) -> String {
        identity::digest(&fixture.checkout.to_string_lossy())
    }

    /// The whole-target receipt oxide#117 published, byte for byte: no
    /// layout, placements, reservation, prune state or root nonce.
    fn version_one_receipt(
        fixture: &Fixture,
        state: &str,
        selected: Option<&Path>,
    ) -> serde_json::Value {
        let mut document = serde_json::json!({
            "version": LEGACY_RECEIPT_VERSION,
            "checkout": fixture.checkout.to_string_lossy(),
            "checkout_id": context_id(fixture),
            "generation": 1,
            "state": state,
            "updated_ms": 1700000000000u64,
        });
        if let Some(root) = selected {
            let selection = serde_json::json!({
                "kind": "local",
                "root": root,
                "generation": 1,
            });
            document["selected"] = selection.clone();
            document["roots"] = serde_json::json!([selection]);
        }
        document
    }

    /// Publish a raw receipt document into a control directory.
    fn write_receipt_document_at(fixture: &Fixture, control: &Path, document: &serde_json::Value) {
        let path = receipt_path(control, &context_id(fixture));
        fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// Publish a raw receipt document into the fixture's own control directory.
    fn write_receipt_document(fixture: &Fixture, document: &serde_json::Value) {
        write_receipt_document_at(fixture, &fixture.control, document);
    }

    /// A receipt's exact bytes and the metadata a read must never touch.
    fn receipt_proof_at(control: &Path, checkout_id: &str) -> (Vec<u8>, u64, u64, u32, i64) {
        let path = receipt_path(control, checkout_id);
        let bytes = fs::read(&path).unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        (
            bytes,
            metadata.ino(),
            metadata.len(),
            metadata.mode(),
            metadata.mtime(),
        )
    }

    /// A receipt's exact bytes and metadata in the fixture's own control
    /// directory.
    fn receipt_proof(fixture: &Fixture) -> (Vec<u8>, u64, u64, u32, i64) {
        receipt_proof_at(&fixture.control, &context_id(fixture))
    }

    /// oxide#152: prune reads a valid version-1 whole-target receipt as the
    /// current in-memory whole-target form, every other reader keeps refusing
    /// it, and a read never rewrites a byte.
    #[test]
    fn a_version_one_receipt_reads_for_prune_only_and_is_never_rewritten() {
        let fixture = Fixture::new();
        let root = fixture.local().root().to_path_buf();
        write_receipt_document(
            &fixture,
            &version_one_receipt(&fixture, "committed", Some(&root)),
        );
        let before = receipt_proof(&fixture);
        let checkout = fixture.checkout.clone();
        let id = context_id(&fixture);

        // The pre-oxide#152 refusal is the control: every ordinary reader
        // still rejects the same bytes as an unsupported version.
        let refused = read_receipt(&fixture.control, &checkout, &id).unwrap_err();
        assert!(refused.contains("unsupported version"), "{refused}");
        assert!(refused.contains(LEGACY_RECEIPT_VERSION), "{refused}");

        let receipt =
            read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
                .unwrap()
                .expect("the version-1 receipt was not read");
        assert_eq!(receipt.version, RECEIPT_VERSION);
        assert_eq!(receipt.layout(), LayoutKind::WholeTarget);
        assert_eq!(receipt.state, ReceiptState::Committed);
        assert_eq!(receipt.checkout(), checkout.as_path());
        assert_eq!(receipt.checkout_id(), id);
        assert_eq!(receipt.roots.len(), 1);
        assert_eq!(receipt.roots[0].root(), root.as_path());
        assert_eq!(receipt.roots[0].generation(), 1);
        assert!(receipt
            .selected()
            .is_some_and(|selected| selected.root() == root.as_path()));
        assert!(receipt
            .selected()
            .is_some_and(|selected| selected.nonce().is_none()));
        assert!(receipt.reserved().is_none());
        assert!(receipt.transition.is_none());
        assert!(receipt.pruned.is_none());
        assert_eq!(receipt.links.len(), 1);
        assert_eq!(receipt.links[0].member(), MEMBER);
        assert!(receipt.prepared_ref().is_none());
        // Reading is not a migration: the published bytes, inode, length,
        // mode and mtime are exactly as oxide#117 left them.
        assert_eq!(receipt_proof(&fixture), before);

        // A valid noncommitted version-1 state is read, not malformed.
        write_receipt_document(&fixture, &version_one_receipt(&fixture, "detached", None));
        let detached =
            read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
                .unwrap()
                .expect("the detached version-1 receipt was not read");
        assert_eq!(detached.state, ReceiptState::Detached);
        assert!(detached.current().is_none());
        let mut prepared = version_one_receipt(&fixture, "prepared", Some(&root));
        prepared["prepared"] = serde_json::json!({
            "generation": 2,
            "to": {"kind": "local", "root": root, "generation": 2},
            "link": fixture.checkout.join("target").to_string_lossy(),
        });
        prepared["generation"] = serde_json::json!(1);
        write_receipt_document(&fixture, &prepared);
        let transition =
            read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
                .unwrap()
                .expect("the prepared version-1 receipt was not read");
        assert_eq!(transition.state, ReceiptState::Prepared);
        assert!(transition.prepared_ref().is_some());
        assert!(transition.current().is_some());
    }

    /// oxide#163: explicit `oxide outputs adopt` is the ONE command that
    /// upgrades a committed version-1 whole-target receipt, and it does so by
    /// running the shipped whole-target-to-profile migration transaction. The
    /// same bytes stay refused by every other reader (the pre-#163 arm is the
    /// counterfactual), and reading the receipt writes nothing.
    #[test]
    fn an_explicit_adopt_upgrades_a_committed_version_one_receipt() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let id = context.checkout_id.clone();
        // A pool whose control directory is the one this checkout's receipt
        // lives in, so the binding path reads exactly these bytes.
        let store = pool_store(&fixture.root.join("pool"));
        let control = store.owner_control().to_path_buf();
        let root = legacy.selected().unwrap().root().to_path_buf();
        make_root_one_marker(&fixture, &legacy);
        // Payload the upgrade must neither move, copy nor delete.
        fs::create_dir_all(root.join("debug")).unwrap();
        fs::write(root.join("debug").join("old-binary"), b"old").unwrap();
        fs::write(root.join("sentinel"), b"root").unwrap();
        write_receipt_document_at(
            &fixture,
            &control,
            &version_one_receipt(&fixture, "committed", Some(&root)),
        );
        let before = receipt_proof_at(&control, &id);

        // The counterfactual: without the upgrade arm the committed document
        // is the pre-#163 refusal, which now names the one command that fixes
        // it.
        let counterfactual = read_receipt_with(
            LegacyReceiptReading::Refuse,
            &control,
            &fixture.checkout,
            &id,
        )
        .unwrap_err();
        assert!(
            counterfactual.contains("unsupported version"),
            "{counterfactual}"
        );
        assert!(
            counterfactual.contains("oxide outputs adopt"),
            "{counterfactual}"
        );
        // bind (every leased build), status, detach and clean all read through
        // the ordinary reader, so each still refuses and names adopt; two of
        // them are called directly here and the CLI cases are in the native
        // proof. The derived-exec repair path changes nothing for a receipt it
        // cannot read.
        let refused = read_receipt(&control, &fixture.checkout, &id).unwrap_err();
        assert!(refused.contains("unsupported version"), "{refused}");
        assert!(refused.contains(LEGACY_RECEIPT_VERSION), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        let status = output_status_lines(&Context::new(
            Some(control.clone()),
            fixture.checkout.clone(),
        ))
        .unwrap_err();
        assert!(status.contains("oxide outputs adopt"), "{status}");
        let bind = match bind_for_command(&store, &fixture.checkout, 0, None, None, false, None) {
            Err(reason) => reason,
            Ok(_) => panic!("binding accepted a version-1 receipt"),
        };
        assert!(bind.contains("oxide outputs adopt"), "{bind}");
        assert_eq!(receipt_proof_at(&control, &id), before);

        // The upgrade arm reads the same bytes into the current in-memory
        // whole-target form, and reading is still not a migration.
        let upgraded = read_receipt_for_adopt(&control, &fixture.checkout, &id)
            .unwrap()
            .expect("the committed version-1 receipt was not read for adopt");
        assert_eq!(upgraded.version, RECEIPT_VERSION);
        assert_eq!(upgraded.layout(), LayoutKind::WholeTarget);
        assert_eq!(upgraded.state, ReceiptState::Committed);
        assert_eq!(upgraded.generation, legacy.generation);
        assert!(upgraded
            .selected()
            .is_some_and(|selected| selected.root() == root.as_path()));
        assert!(upgraded
            .selected()
            .is_some_and(|selected| selected.nonce().is_none()));
        assert!(upgraded.transition.is_none());
        assert_eq!(receipt_proof_at(&control, &id), before);

        // The shipped migration transaction commits the profile layout and
        // publishes the current receipt.
        let (migrated, selected, converted) = migrate_layout(&context, &control, upgraded).unwrap();
        assert!(converted);
        assert_eq!(migrated.version, PROFILE_RECEIPT_VERSION);
        assert_eq!(migrated.layout(), LayoutKind::Profile);
        assert_eq!(migrated.state, ReceiptState::Committed);
        assert!(migrated.transition.is_none());
        assert_eq!(migrated.generation, legacy.generation + 1);
        assert_eq!(selected.root(), root.as_path());
        assert_eq!(selected.nonce().map(str::len), Some(32));
        let marker = read_marker(&root).unwrap();
        assert_eq!(marker.version, MARKER_VERSION);
        assert_eq!(marker.nonce(), selected.nonce());
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(receipt_path(&control, &id)).unwrap()).unwrap();
        assert_eq!(document["version"], PROFILE_RECEIPT_VERSION);
        assert_eq!(document["layout"], "profile");
        // The target is a real directory of member links into the SAME root,
        // and the old payload is visible and intact through it.
        let target = context.link();
        assert!(fs::symlink_metadata(&target).unwrap().is_dir());
        for member in ["debug", "release"] {
            assert_eq!(
                fs::read_link(target.join(member)).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
        }
        assert_eq!(
            fs::read(target.join("debug").join("old-binary")).unwrap(),
            b"old"
        );
        assert_eq!(fs::read(root.join("sentinel")).unwrap(), b"root");
        assert!(!root
            .join(format!("{MARKER}.oxide-old-{}", migrated.generation))
            .exists());
        // The receipt is now ordinary for every reader, and a replay of the
        // finished migration is a no-op.
        let reread = read_receipt(&control, &fixture.checkout, &id)
            .unwrap()
            .unwrap();
        assert_eq!(reread, migrated);
        assert_eq!(
            replay_migration(&context, &control, migrated.clone()).unwrap(),
            migrated
        );
    }

    /// oxide#163: a version-1 document that is not committed, carries a
    /// successor-only surface, an unreadable identity or an unknown field is
    /// never upgraded on explicit adopt, and its bytes never change.
    #[test]
    fn an_explicit_adopt_refuses_noncommitted_or_contradictory_version_one_receipts() {
        let fixture = Fixture::new();
        let root = fixture.local().root().to_path_buf();
        let id = context_id(&fixture);

        // A valid detached version-1 receipt: nothing to migrate from.
        write_receipt_document(&fixture, &version_one_receipt(&fixture, "detached", None));
        let before = receipt_proof(&fixture);
        let refused = read_receipt_for_adopt(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(
            refused.contains("only a committed version-1 receipt can be upgraded"),
            "{refused}"
        );
        assert!(refused.contains("detached"), "{refused}");
        assert_eq!(receipt_proof(&fixture), before);

        // A valid prepared version-1 receipt: its own transition is not this
        // command's migration.
        let mut prepared = version_one_receipt(&fixture, "prepared", Some(&root));
        prepared["prepared"] = serde_json::json!({
            "generation": 2,
            "to": {"kind": "local", "root": root, "generation": 2},
            "link": fixture.checkout.join("target"),
        });
        write_receipt_document(&fixture, &prepared);
        let refused = read_receipt_for_adopt(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(
            refused.contains("only a committed version-1 receipt can be upgraded"),
            "{refused}"
        );
        assert!(refused.contains("prepared"), "{refused}");

        // Successor-only surfaces and unknown fields are contradictory, not
        // merely old.
        let mut layout = version_one_receipt(&fixture, "committed", Some(&root));
        layout["layout"] = serde_json::json!("profile");
        let mut reserved = version_one_receipt(&fixture, "committed", Some(&root));
        reserved["reserved"] = serde_json::json!({
            "id": "reservation",
            "nonce": "a".repeat(32),
            "kind": "local",
            "root": root,
            "generation": 2,
            "created_ms": 1,
        });
        let mut nonce = version_one_receipt(&fixture, "committed", Some(&root));
        nonce["selected"]["nonce"] = serde_json::json!("a".repeat(32));
        let mut unknown = version_one_receipt(&fixture, "committed", Some(&root));
        unknown["surprise"] = serde_json::json!(true);
        let mut other = version_one_receipt(&fixture, "committed", Some(&root));
        other["checkout"] = serde_json::json!("/somewhere/another-checkout");
        for document in [&layout, &reserved, &nonce, &unknown] {
            write_receipt_document(&fixture, document);
            let before = receipt_proof(&fixture);
            let refused =
                read_receipt_for_adopt(&fixture.control, &fixture.checkout, &id).unwrap_err();
            assert!(
                refused.contains("is invalid"),
                "{document} was read: {refused}"
            );
            assert_eq!(receipt_proof(&fixture), before, "{document}");
        }
        // An unreadable identity is refused by the same checkout check every
        // reader applies, and nothing is rewritten.
        write_receipt_document(&fixture, &other);
        let before = receipt_proof(&fixture);
        let refused = read_receipt_for_adopt(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(refused.contains("another checkout"), "{refused}");
        assert_eq!(receipt_proof(&fixture), before);
    }

    /// oxide#163: an upgrade stopped at any recorded phase leaves the P0
    /// journal in the successor format, so ordinary readers see a pending
    /// migration and the next explicit adopt replays the same exact phases.
    #[test]
    fn a_version_one_upgrade_replays_every_recorded_crash_phase() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        for boundary in [
            "migration-p0",
            "migration-staged",
            "migration-marker",
            "migration-backed-up",
            "migration-swapped",
            "migration-p4",
        ] {
            let (fixture, legacy) = legacy_checkout();
            let context = fixture.context();
            let root = legacy.selected().unwrap().root().to_path_buf();
            make_root_one_marker(&fixture, &legacy);
            write_receipt_document(
                &fixture,
                &version_one_receipt(&fixture, "committed", Some(&root)),
            );
            let upgraded =
                read_receipt_for_adopt(&fixture.control, &fixture.checkout, &context.checkout_id)
                    .unwrap()
                    .expect("the committed version-1 receipt was not read for adopt");
            std::env::set_var(CRASH_ENVIRONMENT, boundary);
            let stopped = migrate_layout(&context, &fixture.control, upgraded);
            std::env::remove_var(CRASH_ENVIRONMENT);
            assert!(
                stopped.is_err(),
                "{boundary}: the injected stop did not stop"
            );
            // No version-1 document survives the P0 write: the journal is the
            // successor format, so an ordinary reader reports a pending
            // migration instead of an unsupported version.
            let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .expect("the P0 journal is not readable by an ordinary reader");
            assert_eq!(stored.version, PROFILE_RECEIPT_VERSION, "{boundary}");
            assert!(stored.transition.is_some(), "{boundary}");
            // The next explicit adopt replays the recorded identities only.
            let done = replay_migration(&context, &fixture.control, stored).unwrap();
            assert_eq!(done.layout(), LayoutKind::Profile, "{boundary}");
            assert!(done.transition.is_none(), "{boundary}");
            assert_eq!(done.generation, legacy.generation + 1, "{boundary}");
            let target = context.link();
            assert!(
                fs::symlink_metadata(&target).unwrap().is_dir(),
                "{boundary}"
            );
            for member in ["debug", "release"] {
                assert_eq!(
                    fs::read_link(target.join(member)).unwrap(),
                    member_destination(&root, member),
                    "{boundary}: {member}"
                );
            }
            assert!(
                read_marker(&root).unwrap().nonce().is_some(),
                "{boundary}: the root marker was not upgraded"
            );
            assert!(!fixture
                .checkout
                .join(format!("{BACKUP_PREFIX}{}", done.generation))
                .exists());
            assert!(!fixture
                .checkout
                .join(format!("{STAGE_PREFIX}{}", done.generation))
                .exists());
        }
    }

    /// A committed profile receipt over the fixture's own local root: every
    /// `placed` member is a live owned link into one root and every
    /// `unselected` member is recorded without a selection (the state a
    /// per-member history keeps after a partial detach).
    fn placed_profile(
        fixture: &Fixture,
        placed: &[&str],
        unselected: &[&str],
    ) -> (Context, Receipt, PathBuf) {
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let selected = spec.selected(1);
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        for member in placed {
            receipt.place_member(member).selected = Some(selected.clone());
            let destination = member_destination(spec.root(), member);
            fs::create_dir_all(&destination).unwrap();
            let link = fixture.checkout.join("target").join(member);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&destination, &link).unwrap();
        }
        for member in unselected {
            receipt.place_member(member).selected = None;
        }
        write_receipt(&fixture.control, &receipt).unwrap();
        (context, receipt, spec.root().to_path_buf())
    }

    /// The derived `exec` member names of one fixture, in declaration order.
    fn derived_exec(fixture: &Fixture) -> Vec<String> {
        exec_declaration(Some(&fixture.control), &fixture.checkout, None)
            .unwrap()
            .members()
            .iter()
            .map(|member| member.name().to_owned())
            .collect()
    }

    fn declaration_of(names: &[String]) -> crate::layout::Declaration {
        crate::layout::Declaration::new(
            names
                .iter()
                .map(|name| crate::layout::Member::parse(name).unwrap()),
        )
        .unwrap()
    }

    /// S1 counterfactual 1 + 10: the derived set is the receipt's placement,
    /// rendered exactly as the equivalent complete declaration is, so the two
    /// share one slot key.
    #[test]
    fn exec_derives_the_receipt_pair_and_matches_a_complete_declaration() {
        let fixture = Fixture::new();
        placed_profile(&fixture, &["debug", "release"], &[]);
        assert_eq!(derived_exec(&fixture), ["debug", "release"]);
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let facts = destination_facts(Some(&plan));
        assert_eq!(facts.lines().count(), 2, "{facts}");
        assert!(
            facts.lines().all(|line| line.starts_with("outputs-3 ")),
            "{facts}"
        );
        assert!(facts.contains(" debug"), "{facts}");
        assert!(facts.contains(" release"), "{facts}");
        let declared =
            crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
        let declared_plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declared,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(destination_facts(Some(&declared_plan)), facts);
    }

    /// S1 counterfactual 2 + 15: a receipt-recorded unselected member is
    /// planned `real`, an unrecorded real member is in neither the plan nor the
    /// key, and the existing bounded notice names the real directory.
    #[test]
    fn exec_keeps_a_recorded_real_member_local_and_ignores_an_unrecorded_one() {
        let fixture = Fixture::new();
        let (context, receipt, _root) = placed_profile(&fixture, &["debug"], &["release"]);
        fs::create_dir(fixture.checkout.join("target/release")).unwrap();
        // An unrecorded real triple member is local and path-stable: a
        // declared command would key it, a derived one never does.
        let triple = "x86_64-unknown-linux-gnu/debug";
        fs::create_dir_all(fixture.checkout.join("target").join(triple)).unwrap();
        assert_eq!(derived_exec(&fixture), ["debug", "release"]);
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let facts = destination_facts(Some(&plan));
        assert!(facts.contains("real release"), "{facts}");
        assert!(!facts.contains(triple), "{facts}");
        assert_eq!(facts.lines().count(), 2, "{facts}");
        let declared = crate::layout::declared(&[triple.to_owned()]).unwrap();
        let declared_plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declared,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(destination_facts(Some(&declared_plan)).contains(triple));
        // The binding keeps `release` local and reports it once, in the
        // existing grammar.
        let store = pool_store(&fixture.root.join("pool"));
        let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Bound(outcome) => outcome,
            Bind::Refuse(reason) => panic!("a recorded real member refused the command: {reason}"),
            Bind::Wait => panic!("a recorded real member waited"),
        };
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("release") && warning.contains("real")),
            "{:?}",
            outcome.warnings
        );
    }

    /// S1 counterfactual 3: a detached profile receipt owns no destination, so
    /// the derived set is empty, the plan is `None` and binding runs locally
    /// with no link.
    #[test]
    fn a_detached_profile_receipt_plans_no_destination_and_binds_locally() {
        let fixture = Fixture::new();
        let (context, mut receipt, _root) = placed_profile(&fixture, &["debug", "release"], &[]);
        for member in ["debug", "release"] {
            fs::remove_file(fixture.checkout.join("target").join(member)).unwrap();
            fs::create_dir(fixture.checkout.join("target").join(member)).unwrap();
            receipt.place_member(member).selected = None;
        }
        receipt.state = ReceiptState::Detached;
        write_receipt(&fixture.control, &receipt).unwrap();
        assert!(derived_exec(&fixture).is_empty());
        assert!(planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap()
        .is_none());
        let store = pool_store(&fixture.root.join("pool"));
        match bind_profile(&context, &store, 0, &receipt, None, false) {
            Bind::Bound(outcome) => {
                assert!(outcome.root.is_none());
                assert_eq!(outcome.note.as_deref(), Some("outputs detached"));
            }
            Bind::Refuse(reason) => panic!("a detached exec was refused: {reason}"),
            Bind::Wait => panic!("a detached exec waited"),
        }
        // A slot keyed while the checkout was still committed refuses rather
        // than running through a link the receipt no longer selects.
        let stale = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "debug".to_owned(),
                selected: None,
            }],
        };
        assert!(matches!(
            bind_profile(&context, &store, 0, &receipt, Some(&stale), false),
            Bind::Refuse(_)
        ));
    }

    /// S1 counterfactual 4: a receipt in any custody state other than committed
    /// or detached refuses the derivation, naming the existing rule.
    #[test]
    fn exec_refuses_a_pending_or_pruned_profile_receipt() {
        let fixture = Fixture::new();
        let (_, receipt, _root) = placed_profile(&fixture, &["debug"], &[]);
        let spec = fixture.local();
        let mut prepared = receipt.clone();
        prepared.state = ReceiptState::Prepared;
        prepared.place_member("debug").prepared = Some(Prepared {
            generation: 2,
            to: spec.selected(2),
            link: fixture.checkout.join("target/debug"),
        });
        prepared.generation = 1;
        let mut pruning = receipt.clone();
        pruning.state = ReceiptState::Pruning;
        pruning.pruned = Some(PruneRecord {
            intent: "9".repeat(32),
            outcome: "pruning".to_owned(),
            at_ms: 1,
        });
        let mut pruned = receipt.clone();
        pruned.state = ReceiptState::Pruned;
        for (candidate, expected) in [
            (prepared, "interrupted"),
            (pruning, "being pruned"),
            (pruned, "was pruned"),
        ] {
            let label = candidate.state;
            write_receipt(&fixture.control, &candidate).unwrap();
            let refused =
                exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap_err();
            assert!(refused.contains(expected), "{label:?}: {refused}");
        }
        // The unmodified committed receipt still derives: a control.
        write_receipt(&fixture.control, &receipt).unwrap();
        assert_eq!(derived_exec(&fixture), ["debug"]);
    }

    /// S1 counterfactuals 5 and 6: a whole-target receipt and a checkout with
    /// no receipt derive nothing, so their behaviour is unchanged.
    #[test]
    fn exec_derives_nothing_for_a_whole_target_or_unconverted_checkout() {
        let fixture = Fixture::new();
        // Never converted.
        assert!(derived_exec(&fixture).is_empty());
        assert!(planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap()
        .is_none());
        let context = fixture.context();
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        assert_eq!(committed.layout(), LayoutKind::WholeTarget);
        assert!(derived_exec(&fixture).is_empty());
        // A modelled command keeps the legacy plan and key, untouched.
        let declaration = declaration_of(&["debug".to_owned()]);
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(plan.whole_target);
        let facts = destination_facts(Some(&plan));
        assert_eq!(facts.lines().count(), 1, "{facts}");
        assert!(facts.starts_with("outputs-2 "), "{facts}");
        // A legacy key renders only `outputs-2` lines. The check is on the line
        // prefix, not a substring: a fixture path may contain `outputs-3`.
        assert!(
            facts.lines().all(|line| line.starts_with("outputs-2 ")),
            "{facts}"
        );
    }

    /// S1 counterfactuals 7, 8, 13 and 16: the classification is non-vacuous.
    /// An owned member link is accepted while the same path with another
    /// destination, a link the receipt does not place, and a member-shaped
    /// regular file all refuse before Cargo with nothing changed.
    #[test]
    fn exec_refuses_every_member_shaped_path_it_does_not_own() {
        // The accepted control first.
        let accepted = Fixture::new();
        placed_profile(&accepted, &["debug"], &[]);
        assert_eq!(derived_exec(&accepted), ["debug"]);
        // Another destination, everything else identical.
        let elsewhere = Fixture::new();
        let (_, receipt, root) = placed_profile(&elsewhere, &["debug"], &[]);
        let outside = elsewhere.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let link = elsewhere.checkout.join("target/debug");
        fs::remove_file(&link).unwrap();
        symlink(&outside, &link).unwrap();
        let refused =
            exec_declaration(Some(&elsewhere.control), &elsewhere.checkout, None).unwrap_err();
        assert!(refused.contains("target/debug"), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert_eq!(fs::read_link(&link).unwrap(), outside);
        assert!(root.join("debug").read_dir().unwrap().next().is_none());
        let _ = receipt;
        // A link the receipt does not place refuses as well, owned or foreign.
        let unplaced = Fixture::new();
        let (context, _receipt, root) = placed_profile(&unplaced, &["debug"], &[]);
        fs::create_dir_all(member_destination(&root, "release")).unwrap();
        symlink(
            &member_destination(&root, "release"),
            &unplaced.checkout.join("target/release"),
        )
        .unwrap();
        let refused =
            exec_declaration(Some(&unplaced.control), &unplaced.checkout, None).unwrap_err();
        assert!(refused.contains("target/release"), "{refused}");
        assert!(refused.contains("does not place it"), "{refused}");
        let _ = context;
        // A member-shaped regular file refuses too.
        let filed = Fixture::new();
        let (_context, _receipt, _root) = placed_profile(&filed, &["debug"], &[]);
        fs::write(filed.checkout.join("target/release"), b"file").unwrap();
        let refused = exec_declaration(Some(&filed.control), &filed.checkout, None).unwrap_err();
        assert!(refused.contains("target/release"), "{refused}");
        assert_eq!(
            fs::read(filed.checkout.join("target/release")).unwrap(),
            b"file"
        );
    }

    /// oxide#162: a receipt-recorded member link whose destination directory is
    /// absent inside the checkout's own verified, committed root is created by
    /// the binding instead of being refused as "root not available". The
    /// observed shape: a migration left `target/release` linked before the root
    /// had ever held a release build.
    ///
    /// Counterfactual: reverting the derivation change makes `derived_exec`
    /// return an error here, and reverting the binding change makes it refuse.
    #[test]
    fn exec_creates_an_absent_member_of_the_verified_owned_root() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Permanent);
        let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
        fs::remove_dir(root.join("release")).unwrap();
        let link = fixture.checkout.join("target/release");
        assert_eq!(fs::read_link(&link).unwrap(), root.join("release"));
        assert!(!root.join("release").exists());
        assert_eq!(
            link_state(&link, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .code(),
            "dangling-link"
        );
        let before = fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
        assert_eq!(derived_exec(&fixture), ["debug", "release"]);
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(destination_facts(Some(&plan)).contains("release"));
        let store = pool_store(&fixture.root.join("pool"));
        let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
            Bind::Bound(outcome) => outcome,
            Bind::Refuse(reason) => {
                panic!("an absent member of the verified root refused: {reason}")
            }
            Bind::Wait => panic!("an absent member of the verified root waited"),
        };
        assert_eq!(outcome.root.as_deref(), Some(root.as_path()));
        let metadata = fs::symlink_metadata(root.join("release")).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "the binding did not create a real member directory"
        );
        assert!(fs::read_dir(root.join("release")).unwrap().next().is_none());
        // Nothing was relinked and the receipt bytes are untouched.
        assert_eq!(fs::read_link(&link).unwrap(), root.join("release"));
        assert!(fs::metadata(&link).unwrap().is_dir());
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
            before
        );
        // A second binding sees the member as an ordinary owned link.
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
            Bind::Bound(outcome) => assert_eq!(outcome.root.as_deref(), Some(root.as_path())),
            Bind::Refuse(reason) => panic!("the created member refused a second binding: {reason}"),
            Bind::Wait => panic!("the created member waited"),
        }
    }

    /// oxide#165: a converted checkout's whole `target` directory was deleted,
    /// so every member link is positively absent while the owned root verifies.
    /// The derived `exec` plans the receipt's members and the binding recreates
    /// the real directory and republishes the pair into the same root; an
    /// unverified root still refuses and creates nothing.
    #[test]
    fn exec_plans_deleted_member_links_of_the_verified_owned_root() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        register(&fixture, DriveMode::Permanent);
        let context = fixture.context();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
        let root = selected.root().to_path_buf();
        fs::remove_dir_all(fixture.checkout.join("target")).unwrap();
        assert_eq!(
            target_state(&fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Missing
        );
        assert_eq!(derived_exec(&fixture), ["debug", "release"]);
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(
            plan.members.iter().all(|member| member.selected.is_some()),
            "the derived plan did not key the owned members"
        );
        let store = pool_store(&fixture.root.join("pool"));
        let outcome = match bind_profile(&context, &store, 0, &committed, Some(&plan), true) {
            Bind::Bound(outcome) => outcome,
            Bind::Refuse(reason) => panic!("a deleted target directory refused: {reason}"),
            Bind::Wait => panic!("a deleted target directory waited"),
        };
        assert_eq!(outcome.root.as_deref(), Some(root.as_path()));
        let target = fixture.checkout.join("target");
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "the binding did not recreate a real target directory"
        );
        for member in ["debug", "release"] {
            assert_eq!(
                fs::read_link(target.join(member)).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
            assert!(
                root.join(member).is_dir(),
                "{member}: the member directory is not real"
            );
        }

        // Control: with the root's marker gone the root is not verified, so the
        // absent links still refuse and nothing is created.
        let unverified = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        register(&unverified, DriveMode::Permanent);
        let unverified_context = unverified.context();
        let unverified_receipt = Receipt::first_with(
            &unverified.checkout,
            &unverified_context.checkout_id,
            LayoutKind::Profile,
        );
        let (_committed, unverified_selected, _) =
            adopt_profile(&unverified_context, unverified_receipt).unwrap();
        fs::remove_dir_all(unverified.checkout.join("target")).unwrap();
        fs::remove_file(unverified_selected.root().join(MARKER)).unwrap();
        let refused =
            exec_declaration(Some(&unverified.control), &unverified.checkout, None).unwrap_err();
        assert!(refused.contains("target/debug"), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert_eq!(
            target_state(&unverified.checkout, &unverified_context.checkout_id).unwrap(),
            TargetState::Missing,
            "a refused derived exec created the target directory"
        );
    }

    /// The same shape with a keyed plan whose nonce the root does not carry: a
    /// different root nonce is a contradiction, so nothing is created and the
    /// link stays dangling. The receipt and the link are unchanged.
    #[test]
    fn exec_creates_no_member_directory_for_a_root_with_another_nonce() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Permanent);
        let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
        fs::remove_dir(root.join("release")).unwrap();
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let mut plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        for member in &mut plan.members {
            if let Some(selected) = &mut member.selected {
                selected.nonce = Some("0123456789abcdef0123456789abcdef".to_owned());
            }
        }
        let store = pool_store(&fixture.root.join("pool"));
        let refused = match bind_profile(&context, &store, 0, &receipt, Some(&plan), true) {
            Bind::Refuse(reason) => reason,
            Bind::Bound(_) => panic!("another root nonce was accepted"),
            Bind::Wait => panic!("another root nonce waited"),
        };
        assert!(refused.contains("nonce"), "{refused}");
        assert!(!root.join("release").exists());
        assert!(
            fs::symlink_metadata(fixture.checkout.join("target/release"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// A link into an unverified root, or into a root that is gone, is still
    /// refused by the derived `exec` before Cargo, and nothing is created.
    #[test]
    fn exec_still_refuses_an_absent_member_of_an_unverified_or_missing_root() {
        // An unmarked directory is not this checkout's root: the link text
        // names its own absent `release`, and no directory is created there.
        let unverified = Fixture::new();
        register(&unverified, DriveMode::Permanent);
        let (unverified_context, _receipt, _root) =
            placed_profile(&unverified, &["debug", "release"], &[]);
        let outside = unverified.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let link = unverified.checkout.join("target/release");
        fs::remove_file(&link).unwrap();
        symlink(outside.join("release"), &link).unwrap();
        let refused =
            exec_declaration(Some(&unverified.control), &unverified.checkout, None).unwrap_err();
        assert!(refused.contains("target/release"), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert!(!outside.join("release").exists());
        assert_eq!(fs::read_link(&link).unwrap(), outside.join("release"));
        let _ = unverified_context;
        // A root that is gone entirely: the link names its own destination,
        // but the root no longer passes the bound check, so nothing is made.
        let missing = Fixture::new();
        register(&missing, DriveMode::Permanent);
        let (missing_context, _receipt, missing_root) =
            placed_profile(&missing, &["debug", "release"], &[]);
        fs::remove_dir(missing_root.join("release")).unwrap();
        fs::rename(&missing_root, missing.root.join("moved-root")).unwrap();
        let refused =
            exec_declaration(Some(&missing.control), &missing.checkout, None).unwrap_err();
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert!(!missing_root.exists());
        assert_eq!(
            fs::read_link(missing.checkout.join("target/release")).unwrap(),
            missing_root.join("release")
        );
        let _ = missing_context;
    }

    /// S1 counterfactual 9: more receipt members than one command binds refuse,
    /// naming the explicit detach.
    #[test]
    fn exec_refuses_a_checkout_placing_more_members_than_one_command_binds() {
        let fixture = Fixture::new();
        let names = (0..9)
            .map(|index| format!("t{index}/debug"))
            .collect::<Vec<_>>();
        let borrowed = names.iter().map(String::as_str).collect::<Vec<_>>();
        placed_profile(&fixture, &borrowed, &[]);
        let refused =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap_err();
        assert!(refused.contains("oxide outputs detach"), "{refused}");
        assert!(refused.contains('9'), "{refused}");
        // The explicit override still declares a bounded subset.
        let declared = crate::layout::declared(&["t0/debug".to_owned()]).unwrap();
        assert_eq!(declared.members().len(), 1);
    }

    /// S1 counterfactuals 11 and 12: an explicit `--output` set stays complete
    /// and may name a member the receipt does not place, while a checkout path
    /// containing a space derives exactly the same set.
    #[test]
    fn exec_declarations_stay_complete_and_derivation_ignores_the_checkout_path() {
        let fixture = Fixture::new();
        let (_, _receipt, root) = placed_profile(&fixture, &["debug"], &[]);
        // The derived set never invents the unplaced member.
        assert_eq!(derived_exec(&fixture), ["debug"]);
        // A declared override is still complete: it names the member the
        // receipt does not place and plans the owned root for it.
        let declared = crate::layout::declared(&["release".to_owned()]).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declared,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.members.len(), 1);
        assert_eq!(plan.selected().map(Selected::root), Some(root.as_path()));
        // A checkout path with a space changes nothing: member names carry no
        // checkout path.
        let spaced = Fixture::in_tree("check out", "/target\n.oxide-outputs/\n");
        placed_profile(&spaced, &["debug", "release"], &[]);
        assert_eq!(derived_exec(&spaced), ["debug", "release"]);
        assert_eq!(
            destination_facts(
                planned_destination(
                    Some(&spaced.control),
                    &spaced.checkout,
                    &exec_declaration(Some(&spaced.control), &spaced.checkout, None).unwrap(),
                    None,
                    false,
                )
                .unwrap()
                .as_ref()
            )
            .lines()
            .count(),
            2
        );
    }

    /// oxide#164: an external subcommand (`cargo-<name>` run by Cargo) uses the
    /// SAME derivation as a derived `oxide exec`, so it derives the receipt's
    /// member set and renders a byte-identical slot key.
    ///
    /// Counterfactual: restoring the pre-#164 rule — derive only for
    /// `exec`, and call `refuse_unknown_layout` for every other unmodelled
    /// command — makes `lease_declaration` return an error here instead of a
    /// declaration, so this test fails.
    #[test]
    fn an_external_subcommand_derives_the_receipt_pair_like_exec() {
        let fixture = Fixture::new();
        placed_profile(&fixture, &["debug", "release"], &[]);
        // The real parser, so the routing input is the one the CLI produces:
        // an external subcommand's argv is untouched and it declares nothing.
        let external = crate::cli::parse_leased("oxfixture", &[OsString::from("run")]).unwrap();
        assert_eq!(
            external.cargo_args,
            [OsString::from("oxfixture"), OsString::from("run")]
        );
        assert!(external.declared_outputs.is_empty());
        assert!(crate::cli::derives_from_receipt(
            &external.command,
            &external.declared_outputs
        ));
        let declaration =
            lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
        let names: Vec<&str> = declaration
            .members()
            .iter()
            .map(crate::layout::Member::name)
            .collect();
        assert_eq!(names, ["debug", "release"]);
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap();
        let facts = destination_facts(plan.as_ref());
        assert_eq!(facts.lines().count(), 2, "{facts}");
        assert!(
            facts.lines().all(|line| line.starts_with("outputs-3 ")),
            "{facts}"
        );
        // One derivation, one key: the shipped `exec` path renders the same
        // bytes for the same checkout.
        let exec = exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let exec_plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &exec,
            None,
            false,
        )
        .unwrap();
        assert_eq!(destination_facts(exec_plan.as_ref()), facts);
        // The old refusal still exists for a classification this layout cannot
        // model, and it would have refused this profile checkout — which is
        // exactly why the routing no longer calls it here.
        assert!(refuse_unknown_layout(
            Some(&fixture.control),
            &fixture.checkout,
            "`oxfixture` is not a command whose profile outputs Oxide models",
        )
        .is_err());
    }

    /// oxide#164: a whole-target receipt and a never-converted checkout are
    /// unchanged for an external subcommand: the derivation adds nothing, the
    /// external command plans exactly the legacy whole-target key a modelled
    /// command plans, and an unconverted checkout plans no destination at all.
    #[test]
    fn an_external_subcommand_is_unchanged_for_whole_target_and_unconverted_checkouts() {
        let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
        // Never converted: no declaration, no destination, no keyed member.
        let plain = Fixture::new();
        assert!(
            lease_declaration(&external, Some(&plain.control), &plain.checkout, None)
                .unwrap()
                .is_empty()
        );
        assert!(planned_destination(
            Some(&plain.control),
            &plain.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap()
        .is_none());
        // A committed whole-target receipt: still empty, so the legacy plan
        // and its `outputs-2` key are exactly a modelled command's.
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        assert_eq!(committed.layout(), LayoutKind::WholeTarget);
        let declaration =
            lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
        assert!(declaration.is_empty());
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(plan.whole_target);
        let facts = destination_facts(Some(&plan));
        assert!(
            facts.lines().all(|line| line.starts_with("outputs-2 ")),
            "{facts}"
        );
        let modelled = declaration_of(&["debug".to_owned()]);
        let modelled_plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &modelled,
            None,
            false,
        )
        .unwrap();
        assert_eq!(destination_facts(modelled_plan.as_ref()), facts);
    }

    /// oxide#164: the #162 absent-member repair is a property of the derivation,
    /// not of the command name, so an external subcommand in the same shape
    /// creates the one absent member directory under its verified root.
    #[test]
    fn an_external_subcommand_creates_an_absent_member_of_the_verified_owned_root() {
        let fixture = Fixture::new();
        register(&fixture, DriveMode::Permanent);
        let (context, receipt, root) = placed_profile(&fixture, &["debug", "release"], &[]);
        fs::remove_dir(root.join("release")).unwrap();
        let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
        let repair =
            crate::cli::derives_from_receipt(&external.command, &external.declared_outputs);
        assert!(
            repair,
            "an external subcommand is not routed through the derivation"
        );
        let declaration =
            lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(destination_facts(Some(&plan)).contains("release"));
        let store = pool_store(&fixture.root.join("pool"));
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), repair) {
            Bind::Bound(outcome) => assert_eq!(outcome.root.as_deref(), Some(root.as_path())),
            Bind::Refuse(reason) => {
                panic!("an absent member of the verified root refused: {reason}")
            }
            Bind::Wait => panic!("an absent member of the verified root waited"),
        }
        assert!(root.join("release").is_dir());
        assert_eq!(
            fs::read_link(fixture.checkout.join("target/release")).unwrap(),
            root.join("release")
        );
    }

    /// oxide#164: every derivation refusal is preserved for an external
    /// subcommand — a foreign or unowned member link, a member-shaped file and
    /// a receipt over the member cap all refuse before Cargo, with nothing
    /// changed.
    #[test]
    fn an_external_subcommand_keeps_the_derivation_refusals() {
        let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
        // A foreign member link into another destination.
        let foreign = Fixture::new();
        let (_context, _receipt, _root) = placed_profile(&foreign, &["debug"], &[]);
        let outside = foreign.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let link = foreign.checkout.join("target/debug");
        fs::remove_file(&link).unwrap();
        symlink(&outside, &link).unwrap();
        let refused = lease_declaration(&external, Some(&foreign.control), &foreign.checkout, None)
            .unwrap_err();
        assert!(refused.contains("target/debug"), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        assert_eq!(fs::read_link(&link).unwrap(), outside);
        // A member-shaped regular file at a receipt-recorded member path.
        let filed = Fixture::new();
        let (_context, _receipt, _root) = placed_profile(&filed, &["debug"], &[]);
        fs::write(filed.checkout.join("target/release"), b"file").unwrap();
        let refused =
            lease_declaration(&external, Some(&filed.control), &filed.checkout, None).unwrap_err();
        assert!(refused.contains("target/release"), "{refused}");
        assert_eq!(
            fs::read(filed.checkout.join("target/release")).unwrap(),
            b"file"
        );
        // More receipt members than one command binds.
        let capped = Fixture::new();
        let names = (0..9)
            .map(|index| format!("t{index}/debug"))
            .collect::<Vec<_>>();
        let borrowed = names.iter().map(String::as_str).collect::<Vec<_>>();
        placed_profile(&capped, &borrowed, &[]);
        let refused = lease_declaration(&external, Some(&capped.control), &capped.checkout, None)
            .unwrap_err();
        assert!(refused.contains("oxide outputs detach"), "{refused}");
        assert!(refused.contains('9'), "{refused}");
    }

    /// oxide#164: the tool-directory notice is bounded and precise. It names
    /// only directories that APPEARED directly under a real target, never a
    /// profile member, never Cargo's own `tmp`, never a file, and never through
    /// a linked target; its list is capped.
    #[test]
    fn the_tool_directory_notice_names_only_new_non_member_directories() {
        let fixture = Fixture::new();
        placed_profile(&fixture, &["debug"], &[]);
        let before = target_entries(&fixture.checkout).unwrap();
        // Nothing new: no notice at all.
        assert!(tool_directories_note(&fixture.checkout, Some(&before)).is_none());
        // A tool's own directory is named, once.
        fs::create_dir(fixture.checkout.join("target/nextest")).unwrap();
        let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
        assert!(note.contains("(nextest)"), "{note}");
        assert_eq!(note.matches("nextest").count(), 1, "{note}");
        // Cargo's own `tmp`, a member-shaped directory, and a file are not
        // this notice's business.
        fs::create_dir(fixture.checkout.join("target/tmp")).unwrap();
        fs::create_dir(fixture.checkout.join("target/release")).unwrap();
        fs::write(fixture.checkout.join("target/.rustc_info.json"), b"{}").unwrap();
        let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
        assert!(note.contains("(nextest)"), "{note}");
        assert!(!note.contains("tmp"), "{note}");
        assert!(!note.contains("release"), "{note}");
        assert!(!note.contains("rustc_info"), "{note}");
        // The printed list is capped at eight names plus a bounded remainder.
        for index in 0..9 {
            fs::create_dir(fixture.checkout.join(format!("target/tool{index}"))).unwrap();
        }
        let note = tool_directories_note(&fixture.checkout, Some(&before)).unwrap();
        assert!(note.contains("and 2 more"), "{note}");
        // A target that is a link (the whole-target layout) is not observed.
        let linked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let outside = linked.root.join("outside");
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, linked.checkout.join("target")).unwrap();
        assert!(target_entries(&linked.checkout).is_none());
        assert!(tool_directories_note(&linked.checkout, Some(&[])).is_none());
        // An absent target is not observed either, and no observation means no
        // notice.
        assert!(target_entries(&linked.root.join("absent")).is_none());
        assert!(tool_directories_note(&fixture.checkout, None).is_none());
    }

    /// oxide#164 review: the tool-directory notice belongs to the
    /// receipt-derived PROFILE-MEMBER layout only. An unconverted, ephemeral,
    /// whole-target or detached checkout plans nothing (or the whole target), a
    /// modelled command and an explicit `--output` declaration are not derived,
    /// so none of them may gain the extra line: their shipped stderr stays.
    #[test]
    fn the_tool_directory_notice_applies_only_to_a_derived_profile_plan() {
        let fixture = Fixture::new();
        let selected = fixture.local().selected(1);
        let owned = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "debug".to_owned(),
                selected: Some(selected),
            }],
        };
        let real = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "release".to_owned(),
                selected: None,
            }],
        };
        let whole = Plan {
            whole_target: true,
            members: vec![PlannedMember {
                member: MEMBER.to_owned(),
                selected: Some(fixture.local().selected(1)),
            }],
        };
        let empty = Plan {
            whole_target: false,
            members: Vec::new(),
        };
        // The profile layout, owned or receipt-recorded real.
        assert!(tool_notice_applies(true, Some(&owned)));
        assert!(tool_notice_applies(true, Some(&real)));
        // Unconverted, ephemeral and detached checkouts plan nothing.
        assert!(!tool_notice_applies(true, None));
        // A legacy whole-target checkout plans the whole target.
        assert!(!tool_notice_applies(true, Some(&whole)));
        // An explicit `--output` declaration and every modelled command are not
        // derived sets.
        assert!(!tool_notice_applies(false, Some(&owned)));
        // A plan with no member places nothing to report against.
        assert!(!tool_notice_applies(true, Some(&empty)));
    }

    /// S1 counterfactual 14: member names are matched before triple descent, so
    /// a real or linked `target/debug` is never enumerated as a triple ancestor
    /// and its payload is never listed.
    #[test]
    fn a_real_member_directory_is_never_treated_as_a_triple_ancestor() {
        let fixture = Fixture::new();
        placed_profile(&fixture, &["release"], &[]);
        let debug = fixture.checkout.join("target/debug");
        fs::create_dir(&debug).unwrap();
        fs::write(debug.join("profileapp"), b"payload").unwrap();
        fs::create_dir(debug.join("debug")).unwrap();
        fs::create_dir(debug.join("release")).unwrap();
        let members = migration_members(&fixture.checkout.join("target")).unwrap();
        assert!(members.contains(&"debug".to_owned()), "{members:?}");
        assert!(
            !members
                .iter()
                .any(|name| name == "debug/debug" || name == "debug/release"),
            "{members:?}"
        );
        // The derived set neither lists the payload as a member nor plans it.
        assert_eq!(derived_exec(&fixture), ["release"]);
        let declaration =
            exec_declaration(Some(&fixture.control), &fixture.checkout, None).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let facts = destination_facts(Some(&plan));
        assert!(!facts.contains("debug/debug"), "{facts}");
        assert!(!facts.contains("debug/release"), "{facts}");
    }

    /// S4b: the automatic first conversion creates the profile layout through
    /// the same creator an explicit adoption uses, so both publish one receipt
    /// form and one root marker.
    #[test]
    fn automatic_first_conversion_creates_the_profile_layout() {
        let automatic = Fixture::new();
        let context = automatic.context();
        let (_committed, selected, converted) = adopt_profile_from(
            &context,
            Receipt::first_with(
                &automatic.checkout,
                &context.checkout_id,
                LayoutKind::Profile,
            ),
            Start::MissingOnly,
            Some(&automatic.local()),
        )
        .unwrap();
        assert!(converted);
        let target = automatic.checkout.join("target");
        assert!(fs::symlink_metadata(&target).unwrap().is_dir());
        assert!(!fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        for member in ["debug", "release"] {
            let link = target.join(member);
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{member} is not an owned link"
            );
            assert_eq!(
                fs::read_link(&link).unwrap(),
                member_destination(selected.root(), member)
            );
            // The first conversion creates the member's real directory before
            // it publishes the link, so no link ever dangles (oxide#162).
            let destination = selected.root().join(member);
            let metadata = fs::symlink_metadata(&destination).unwrap();
            assert!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "{member} is not a real member directory"
            );
            assert!(
                fs::metadata(&link).unwrap().is_dir(),
                "{member} is a dangling member link"
            );
        }
        assert!(selected.root().join(MARKER).is_file());
        let stored = read_receipt(
            &automatic.control,
            &automatic.checkout,
            &context.checkout_id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(stored.state, ReceiptState::Committed);
        // The explicit adoption of a twin checkout publishes the same form.
        let explicit = Fixture::new();
        let explicit_context = explicit.context();
        let (explicit_receipt, explicit_selected, _) = adopt_profile(
            &explicit_context,
            Receipt::first_with(
                &explicit.checkout,
                &explicit_context.checkout_id,
                LayoutKind::Profile,
            ),
        )
        .unwrap();
        assert_eq!(stored.version, explicit_receipt.version);
        assert_eq!(stored.layout, explicit_receipt.layout);
        assert_eq!(
            stored
                .links()
                .iter()
                .map(|placement| placement.member().to_owned())
                .collect::<Vec<_>>(),
            explicit_receipt
                .links()
                .iter()
                .map(|placement| placement.member().to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(stored.generation, explicit_receipt.generation);
        assert_eq!(selected.kind, explicit_selected.kind);
        // Every placed member shares the root's one nonce, as explicit adoption
        // does, and the marker is the same recipe and format.
        let nonces = stored
            .links()
            .iter()
            .filter_map(|placement| placement.selected_ref())
            .map(|selected| selected.nonce().map(str::to_owned))
            .collect::<Vec<_>>();
        let explicit_nonces = explicit_receipt
            .links()
            .iter()
            .filter_map(|placement| placement.selected_ref())
            .map(|selected| selected.nonce().map(str::to_owned))
            .collect::<Vec<_>>();
        assert_eq!(nonces.len(), 2);
        assert!(nonces.iter().all(|nonce| nonce.is_some()));
        assert_eq!(nonces[0], nonces[1], "the pair shares one root nonce");
        // Explicit adoption publishes the same one-nonce pair (the nonce value
        // itself is random per conversion).
        assert_eq!(explicit_nonces.len(), 2);
        assert_eq!(explicit_nonces[0], explicit_nonces[1]);
        let marker = read_marker(selected.root()).unwrap();
        let explicit_marker = read_marker(explicit_selected.root()).unwrap();
        assert_eq!(marker.version, explicit_marker.version);
        assert_eq!(marker.recipe, explicit_marker.recipe);
        assert_eq!(marker.kind, explicit_marker.kind);
        // The next leased build keys exactly the derived member set.
        assert_eq!(derived_exec(&automatic), ["debug", "release"]);
    }

    /// S4b: the automatic conversion never converts or keeps anything that
    /// already exists, and it never falls back to another root.
    #[test]
    fn automatic_first_conversion_refuses_an_ineligible_checkout() {
        // An empty real target is not a first conversion.
        let empty = Fixture::new();
        fs::create_dir(empty.checkout.join("target")).unwrap();
        let context = empty.context();
        let refused = adopt_profile_from(
            &context,
            Receipt::first_with(&empty.checkout, &context.checkout_id, LayoutKind::Profile),
            Start::MissingOnly,
            Some(&empty.local()),
        )
        .unwrap_err();
        assert!(refused.contains("auto-adopt refused"), "{refused}");
        assert!(
            fs::read_dir(empty.checkout.join("target"))
                .unwrap()
                .next()
                .is_none(),
            "a refused conversion wrote into the target"
        );
        assert!(
            read_receipt(&empty.control, &empty.checkout, &context.checkout_id)
                .unwrap()
                .is_none(),
            "a refused conversion wrote a receipt"
        );
        assert!(!empty.local().root().exists());
        // An owned whole-target link is left exactly as it is.
        let linked = Fixture::new();
        let linked_context = linked.context();
        transition(
            &linked_context,
            &linked.receipt(),
            &linked.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let before = fs::read_link(linked.checkout.join("target")).unwrap();
        let refused = adopt_profile_from(
            &linked_context,
            Receipt::first_with(
                &linked.checkout,
                &linked_context.checkout_id,
                LayoutKind::Profile,
            ),
            Start::MissingOnly,
            Some(&linked.local()),
        )
        .unwrap_err();
        assert!(refused.contains("whole-target link"), "{refused}");
        assert_eq!(
            fs::read_link(linked.checkout.join("target")).unwrap(),
            before
        );
    }

    /// S4b: a whole-target receipt is untouched by a build and by a derived
    /// `exec`; only an explicit adoption migrates it.
    #[test]
    fn a_whole_target_receipt_is_untouched_by_a_build() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        assert_eq!(committed.layout(), LayoutKind::WholeTarget);
        let path = receipt_path(&fixture.control, &context.checkout_id);
        let before = fs::read(&path).unwrap();
        assert!(derived_exec(&fixture).is_empty());
        let declaration = declaration_of(&["debug".to_owned(), "release".to_owned()]);
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(plan.whole_target);
        assert!(destination_facts(Some(&plan)).starts_with("outputs-2 "));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(
            fs::read_link(fixture.checkout.join("target")).unwrap(),
            fixture.local().root()
        );
    }

    /// A document that uses a successor-only surface under the version-1 label
    /// is contradictory, never "old": both readers refuse it.
    #[test]
    fn a_contradictory_version_one_receipt_is_refused_by_both_readers() {
        let fixture = Fixture::new();
        let root = fixture.local().root().to_path_buf();
        let checkout = fixture.checkout.clone();
        let id = context_id(&fixture);
        let base = version_one_receipt(&fixture, "committed", Some(&root));
        let mut cases: Vec<(&str, serde_json::Value)> = Vec::new();
        for (label, key, value) in [
            ("layout", "layout", serde_json::json!("profile")),
            (
                "links",
                "links",
                serde_json::json!([{"member": "target", "selected": {"kind": "local", "root": root, "generation": 1}}]),
            ),
            (
                "reservation",
                "reserved",
                serde_json::json!({
                    "id": "5".repeat(32),
                    "nonce": "6".repeat(32),
                    "kind": "local",
                    "root": root,
                    "generation": 2,
                    "created_ms": 1,
                }),
            ),
            (
                "prune record",
                "pruned",
                serde_json::json!({"intent": "7".repeat(32), "outcome": "deleted", "at_ms": 1}),
            ),
            ("unknown field", "surprise", serde_json::json!(true)),
        ] {
            let mut document = base.clone();
            document[key] = value;
            cases.push((label, document));
        }
        for (label, mut document) in [
            ("pruned state", base.clone()),
            ("pruning state", base.clone()),
        ] {
            let state = if label == "pruned state" {
                "pruned"
            } else {
                "pruning"
            };
            document["state"] = serde_json::json!(state);
            cases.push((label, document));
        }
        let mut nonce = base.clone();
        nonce["selected"]["nonce"] = serde_json::json!("8".repeat(32));
        cases.push(("fabricated root nonce", nonce));
        let mut unknown = base.clone();
        unknown["version"] = serde_json::json!("oxide.outputs.transition/9");
        cases.push(("unknown version", unknown));
        for (label, document) in cases {
            write_receipt_document(&fixture, &document);
            let refused = read_receipt(&fixture.control, &checkout, &id).unwrap_err();
            assert!(refused.contains("is invalid"), "{label}: {refused}");
            let refused =
                read_receipt_with(LegacyReceiptReading::Read, &fixture.control, &checkout, &id)
                    .unwrap_err();
            assert!(refused.contains("is invalid"), "{label}: {refused}");
        }
    }

    /// The real run's terminal image for an actually pruned version-1 root is
    /// the current receipt shape; it is deterministic, pinned by the caller's
    /// digest, and published only through the existing terminal transaction.
    #[test]
    fn a_version_one_terminal_image_publishes_the_current_shape() {
        let fixture = Fixture::new();
        let root = fixture.local().root().to_path_buf();
        write_receipt_document(
            &fixture,
            &version_one_receipt(&fixture, "committed", Some(&root)),
        );
        let published = receipt_proof(&fixture);
        let checkout = fixture.checkout.clone();
        let id = context_id(&fixture);
        let intent = "e".repeat(32);
        let image =
            terminal_receipt_image(&fixture.control, &checkout, &id, &intent, "deleted", 42)
                .unwrap();
        assert_eq!(
            image,
            terminal_receipt_image(&fixture.control, &checkout, &id, &intent, "deleted", 42)
                .unwrap(),
            "the terminal image is not deterministic"
        );
        // Computing the image reads only: the version-1 bytes are untouched.
        assert_eq!(receipt_proof(&fixture), published);
        let document: serde_json::Value = serde_json::from_slice(&image).unwrap();
        assert_eq!(document["version"], RECEIPT_VERSION);
        assert_eq!(document["state"], "pruned");
        assert_eq!(document["pruned"]["intent"], intent);
        assert_eq!(document["pruned"]["outcome"], "deleted");
        assert_eq!(document["pruned"]["at_ms"], 42);
        assert_eq!(
            document["roots"][0]["root"],
            root.to_string_lossy().as_ref()
        );
        assert!(document["links"][0].get("selected").is_none());
        mark_pruned_at(&fixture.control, &checkout, &id, &intent, "deleted", 42).unwrap();
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &id)).unwrap(),
            image,
            "the published image is not the recorded one"
        );
        // A replay of the same intent republishes exactly the recorded image
        // and another intent refuses, changing nothing.
        mark_pruned_at(&fixture.control, &checkout, &id, &intent, "deleted", 42).unwrap();
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &id)).unwrap(),
            image
        );
        let settled = fs::read(receipt_path(&fixture.control, &id)).unwrap();
        assert!(terminal_receipt_image(
            &fixture.control,
            &checkout,
            &id,
            &"f".repeat(32),
            "deleted",
            43,
        )
        .is_err());
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &id)).unwrap(),
            settled
        );
    }

    /// A committed profile receipt for `debug` and `release` on one root.
    fn profile_receipt(fixture: &Fixture) -> Receipt {
        let mut receipt = Receipt::first_with(
            &fixture.checkout,
            &identity::digest(&fixture.checkout.to_string_lossy()),
            LayoutKind::Profile,
        );
        let selected = fixture
            .local()
            .selected(1)
            .with_nonce(Some("22222222222222222222222222222222".to_owned()));
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(selected.clone());
        receipt.place_member("release").selected = Some(selected);
        receipt
    }

    /// A profile receipt places members independently: a detach leaves every
    /// member unselected, and the per-member transitions that re-adopt them
    /// must be able to write a receipt where one member is committed while the
    /// others are still unselected. The whole-target receipt keeps the strict
    /// rule, and no profile receipt may select nothing at all.
    #[test]
    fn profile_receipts_place_members_independently_but_always_select_one() {
        let fixture = Fixture::new();
        let id = identity::digest(&fixture.checkout.to_string_lossy());
        let mut profile = profile_receipt(&fixture);
        profile.place_member("release").selected = None;
        profile
            .validate(&fixture.checkout, &id)
            .expect("a profile receipt with one unselected member is coherent");
        let mut unselected = profile.clone();
        unselected.place_member("debug").selected = None;
        assert!(
            unselected.validate(&fixture.checkout, &id).is_err(),
            "a profile receipt that selects nothing was accepted"
        );
        // A whole-target receipt still requires its one placement selected.
        let mut whole = fixture.receipt();
        whole.state = ReceiptState::Committed;
        whole.generation = 1;
        whole.link_mut().selected = Some(fixture.local().selected(1));
        whole.validate(&fixture.checkout, &id).unwrap();
        whole.link_mut().selected = None;
        assert!(
            whole.validate(&fixture.checkout, &id).is_err(),
            "a whole-target receipt with no selection was accepted"
        );
    }

    #[test]
    fn profile_destinations_separate_linked_and_real_members() {
        let fixture = Fixture::new();
        let mut receipt = Receipt::first_with(
            &fixture.checkout,
            &identity::digest(&fixture.checkout.to_string_lossy()),
            LayoutKind::Profile,
        );
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(
            fixture
                .local()
                .selected(1)
                .with_nonce(Some("77777777777777777777777777777777".to_owned())),
        );
        write_receipt(&fixture.control, &receipt).unwrap();
        // `release` is a real, nonempty profile directory: it stays local.
        fs::create_dir(fixture.checkout.join("target")).unwrap();
        fs::create_dir(fixture.checkout.join("target/release")).unwrap();
        fs::write(fixture.checkout.join("target/release/keep"), b"x").unwrap();
        let declaration = crate::layout::Declaration::new([
            crate::layout::Member::parse("debug").unwrap(),
            crate::layout::Member::parse("release").unwrap(),
        ])
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let facts = destination_facts(Some(&plan));
        assert!(facts.contains(" debug"), "{facts}");
        assert!(facts.contains("real release"), "{facts}");
        // A layout in which `release` is linked as well keys differently, so a
        // linked and a real member never share a warm slot.
        fs::remove_file(fixture.checkout.join("target/release/keep")).unwrap();
        fs::remove_dir(fixture.checkout.join("target/release")).unwrap();
        let linked = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_ne!(destination_facts(Some(&linked)), facts);
        // A profile checkout refuses a command that declares no members.
        let refused = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap_err();
        assert!(refused.contains("declares none"), "{refused}");
    }

    /// P5: the members an explicit migration links are discovered with one
    /// bounded second component. Only a positively identified plain triple
    /// contributes its own `debug`/`release` directories; a linked triple
    /// directory, a JSON target specification, a custom profile and any other
    /// root entry are never classified as members.
    #[test]
    fn migration_members_discover_only_bounded_triple_pairs() {
        let fixture = Fixture::new();
        let root = fixture.root.join("volume/oxide-outputs/checkout-0000");
        for directory in [
            "debug",
            "release",
            "aarch64-apple-darwin/debug",
            "aarch64-apple-darwin/release",
            // A custom profile under a real triple is not a v1 member.
            "aarch64-apple-darwin/bench",
            // Three components are outside the bounded traversal.
            "aarch64-apple-darwin/debug/incremental",
            // A JSON target specification is not a plain triple.
            "x86_64-unknown-linux-gnu.json/debug",
            // A profile name no v1 layout links.
            "custom-profile",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        // A plain triple that is a SYMLINK is not positively identified: it is
        // never descended into.
        fs::create_dir_all(fixture.root.join("elsewhere/riscv64-unknown-elf/debug")).unwrap();
        symlink(
            fixture.root.join("elsewhere"),
            root.join("riscv64-unknown-elf"),
        )
        .unwrap();
        fs::write(root.join("CACHEDIR.TAG"), b"signature").unwrap();
        assert_eq!(
            migration_members(&root).unwrap(),
            vec![
                "aarch64-apple-darwin/debug".to_owned(),
                "aarch64-apple-darwin/release".to_owned(),
                "debug".to_owned(),
                "release".to_owned(),
            ]
        );
    }

    /// P5: the same discovery end to end. A migrated root that already holds a
    /// plain triple's profile directories gets those pairs linked one level
    /// down under a REAL triple directory, while every other root entry is
    /// left exactly where it was.
    #[test]
    fn a_migration_links_discovered_triple_pairs_one_level_down() {
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        let triple = "aarch64-apple-darwin";
        for profile in ["debug", "release"] {
            fs::create_dir_all(root.join(triple).join(profile)).unwrap();
        }
        // Root entries that are not v1 profile pairs stay untouched.
        fs::write(root.join(triple).join("keep-me"), b"peer").unwrap();
        fs::create_dir_all(root.join("doc")).unwrap();
        let (migrated, _, converted) =
            migrate_layout(&context, &fixture.control, legacy.clone()).unwrap();
        assert!(converted);
        let target = context.link();
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        let members = vec![
            "debug".to_owned(),
            "release".to_owned(),
            format!("{triple}/debug"),
            format!("{triple}/release"),
        ];
        for member in &members {
            assert_eq!(
                fs::read_link(target.join(member)).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
            assert_eq!(
                migrated.selected_for(member).map(Selected::root),
                Some(root.as_path()),
                "{member}"
            );
        }
        // The triple directory is real, and the pair sits inside it.
        let triple_dir = target.join(triple);
        let nested = fs::symlink_metadata(&triple_dir).unwrap();
        assert!(nested.is_dir() && !nested.file_type().is_symlink());
        // The root keeps every entry this migration does not own, and those
        // entries are never copied into the new real target.
        assert_eq!(
            fs::read(root.join(triple).join("keep-me")).unwrap(),
            b"peer"
        );
        assert!(root.join("doc").is_dir());
        assert!(!triple_dir.join("keep-me").exists());
        // The journaled staging and backup are gone and a replay is a no-op.
        assert!(!fixture
            .checkout
            .join(format!("{STAGE_PREFIX}{}", migrated.generation))
            .exists());
        assert_eq!(
            replay_migration(&context, &fixture.control, migrated.clone()).unwrap(),
            migrated
        );
    }

    /// A committed whole-target checkout in a Git work tree that ignores
    /// `target/`, with a root that carries its marker.
    fn legacy_checkout() -> (Fixture, Receipt) {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let context = fixture.context();
        let receipt = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        (fixture, receipt)
    }

    /// Rewrite the root marker as a source-shaped root/1 document: no nonce,
    /// no checkout volume, same owner, recipe, kind and generation.
    fn make_root_one_marker(fixture: &Fixture, receipt: &Receipt) {
        let root = receipt.selected().unwrap().root();
        let marker = read_marker(root).unwrap();
        let legacy = Marker {
            version: LEGACY_MARKER_VERSION.to_owned(),
            checkout: marker.checkout,
            checkout_id: marker.checkout_id,
            label: marker.label,
            recipe: marker.recipe,
            kind: marker.kind,
            volume_uuid: marker.volume_uuid,
            nonce: None,
            checkout_volume_uuid: None,
            generation: marker.generation,
            created_ms: marker.created_ms,
        };
        fs::write(
            root.join(MARKER),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();
        let _ = fixture;
    }

    #[test]
    fn an_explicit_migration_publishes_profile_links_and_resumes() {
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        let nonce = read_marker(&root).unwrap().nonce().unwrap().to_owned();
        let (migrated, selected, converted) =
            migrate_layout(&context, &fixture.control, legacy.clone()).unwrap();
        assert!(converted);
        assert_eq!(migrated.layout(), LayoutKind::Profile);
        assert_eq!(migrated.generation, legacy.generation + 1);
        assert_eq!(migrated.state, ReceiptState::Committed);
        assert_eq!(selected.root(), root.as_path());
        assert_eq!(selected.nonce(), Some(nonce.as_str()));
        assert!(migrated.transition.is_none());
        // The target is a real directory whose only entries are the member
        // links into the same root.
        let target = context.link();
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        for member in ["debug", "release"] {
            let link = target.join(member);
            assert_eq!(
                fs::read_link(&link).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
            assert_eq!(
                migrated.selected_for(member).map(Selected::root),
                Some(root.as_path())
            );
        }
        // The journaled backups and staging directory are gone.
        assert!(!fixture
            .checkout
            .join(format!("{BACKUP_PREFIX}{}", migrated.generation))
            .exists());
        assert!(!fixture
            .checkout
            .join(format!("{STAGE_PREFIX}{}", migrated.generation))
            .exists());
        // Replaying the committed migration is a no-op.
        let again = replay_migration(&context, &fixture.control, migrated.clone()).unwrap();
        assert_eq!(again, migrated);
        // A legacy starter keyed for the whole target refuses after migration.
        let mut facts =
            crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()])
                .unwrap();
        assert!(facts.members().len() == 1);
        facts = crate::layout::Declaration::empty();
        assert!(planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &facts,
            None,
            false
        )
        .is_err());
    }

    /// The observed defect (oxide#162): a migration that linked the host
    /// `debug`/`release` pair published a `target/release` link into a root
    /// that had never held a release build, so the link dangled. Publication
    /// now creates every member directory before its link exists, and never
    /// touches a member directory the root already holds.
    ///
    /// Counterfactual: without the creation step at P1 the `release`
    /// destination is absent here and this test's first assertion fails.
    #[test]
    fn a_migration_creates_every_member_directory_before_its_link() {
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        // Only `debug` exists, with payload; the root has never held a release
        // build, exactly the shape the live migrations had.
        fs::create_dir(root.join("debug")).unwrap();
        fs::write(root.join("debug").join("poutputs"), b"debug-only").unwrap();
        assert!(!root.join("release").exists());
        assert_eq!(
            migration_members(&root).unwrap(),
            vec!["debug".to_owned(), "release".to_owned()]
        );
        let (migrated, _, converted) =
            migrate_layout(&context, &fixture.control, legacy.clone()).unwrap();
        assert!(converted);
        let target = context.link();
        for member in ["debug", "release"] {
            let destination = root.join(member);
            let metadata = fs::symlink_metadata(&destination).unwrap();
            assert!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "{member} is not a real directory after the migration"
            );
            let link = target.join(member);
            assert_eq!(fs::read_link(&link).unwrap(), destination, "{member}");
            // No published link dangles.
            assert!(
                fs::metadata(&link).unwrap().is_dir(),
                "{member} is a dangling member link"
            );
            assert_eq!(
                migrated.selected_for(member).map(Selected::root),
                Some(root.as_path())
            );
        }
        // The pre-existing member directory keeps its payload byte for byte;
        // the created one starts empty.
        assert_eq!(
            fs::read(root.join("debug").join("poutputs")).unwrap(),
            b"debug-only"
        );
        assert!(fs::read_dir(root.join("release")).unwrap().next().is_none());
    }

    #[test]
    fn a_root_one_marker_is_upgraded_once_and_rolled_back_exactly() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        make_root_one_marker(&fixture, &legacy);
        // A DETERMINISTIC original creation timestamp, so a rewrite cannot pass
        // by millisecond coincidence.
        let mut pinned = read_marker(&root).unwrap();
        pinned.created_ms = 1234;
        fs::write(
            root.join(MARKER),
            serde_json::to_vec_pretty(&pinned).unwrap(),
        )
        .unwrap();
        let old_bytes = fs::read(root.join(MARKER)).unwrap();
        let old_digest = file_digest(&root.join(MARKER)).unwrap();
        // A pre-P4 rollback at the marker boundary restores the exact old
        // marker, the old whole-target link and the /2 receipt.
        std::env::set_var(ROLLBACK_ENVIRONMENT, "marker");
        let (restored, selected, _) =
            migrate_layout(&context, &fixture.control, legacy.clone()).unwrap();
        std::env::remove_var(ROLLBACK_ENVIRONMENT);
        assert_eq!(restored.layout(), LayoutKind::WholeTarget);
        assert_eq!(restored.version, RECEIPT_VERSION);
        assert!(restored.transition.is_none());
        assert_eq!(restored.generation, legacy.generation);
        assert_eq!(selected.root(), root.as_path());
        assert_eq!(selected.nonce(), None, "the reserved nonce retires");
        assert_eq!(fs::read(root.join(MARKER)).unwrap(), old_bytes);
        assert_eq!(file_digest(&root.join(MARKER)).unwrap(), old_digest);
        let target = context.link();
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.file_type().is_symlink());
        assert_eq!(fs::read_link(&target).unwrap(), root);
        assert!(!target.join("debug").exists());
        assert!(!fixture
            .checkout
            .join(format!("{BACKUP_PREFIX}{}", legacy.generation + 1))
            .exists());
        assert!(!fixture
            .checkout
            .join(format!("{STAGE_PREFIX}{}", legacy.generation + 1))
            .exists());
        // Now migrate for real: one fresh nonce, shared by every member, and
        // the root marker becomes root/2 while keeping its creation generation.
        let legacy_generation = legacy.generation;
        let (migrated, _, _) = migrate_layout(&context, &fixture.control, legacy).unwrap();
        let marker = read_marker(&root).unwrap();
        assert_eq!(marker.version, MARKER_VERSION);
        assert_eq!(marker.nonce(), migrated.selected().unwrap().nonce());
        assert!(marker.nonce().is_some());
        assert_eq!(marker.generation, legacy_generation);
        // The successor preserves EVERY immutable field of the pinned root/1
        // marker, including its ORIGINAL creation timestamp: only the version
        // and the reserved nonce may change.
        let pinned: Marker = serde_json::from_slice(&old_bytes).unwrap();
        assert_eq!(pinned.version, LEGACY_MARKER_VERSION);
        assert!(pinned.nonce.is_none());
        assert_eq!(pinned.created_ms, 1234, "the fixture pin was not applied");
        assert_eq!(
            marker.created_ms, pinned.created_ms,
            "the root/1 upgrade rewrote created_ms"
        );
        assert_eq!(marker.checkout, pinned.checkout);
        assert_eq!(marker.checkout_id, pinned.checkout_id);
        assert_eq!(marker.label, pinned.label);
        assert_eq!(marker.recipe, pinned.recipe);
        assert_eq!(marker.kind, pinned.kind);
        assert_eq!(marker.volume_uuid, pinned.volume_uuid);
        assert_eq!(marker.checkout_volume_uuid, pinned.checkout_volume_uuid);
        assert_eq!(marker.generation, pinned.generation);
        assert_ne!(
            marker.version, pinned.version,
            "the successor is not the root/2 document"
        );
        assert!(!root
            .join(format!("{MARKER}.oxide-old-{}", migrated.generation))
            .exists());
    }

    #[test]
    fn a_crashed_migration_replays_from_its_recorded_identities() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        for boundary in ["migration-staged", "migration-marker"] {
            let (fixture, legacy) = legacy_checkout();
            let context = fixture.context();
            make_root_one_marker(&fixture, &legacy);
            std::env::set_var(CRASH_ENVIRONMENT, boundary);
            let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
            std::env::remove_var(CRASH_ENVIRONMENT);
            assert!(
                stopped.is_err(),
                "{boundary}: the injected stop did not stop"
            );
            let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            let journal = stored.transition.clone().unwrap();
            assert!(
                journal.stage_dev.is_some() && journal.stage_ino.is_some(),
                "{boundary}: the journal did not record the staging identity"
            );
            if boundary == "migration-marker" {
                assert!(
                    journal.marker_backup.is_some(),
                    "the journal did not record the marker backup"
                );
                assert!(!journal.marker_pin.is_none(), "no marker pin");
            }
            // A replay commits from the recorded identities only.
            let done = replay_migration(&context, &fixture.control, stored).unwrap();
            assert_eq!(done.layout(), LayoutKind::Profile, "{boundary}");
            // P1 created the member directories before it created their links,
            // so a crash at either boundary leaves no dangling link behind and
            // the replay publishes every member into a real directory.
            let root = legacy.selected().unwrap().root().to_path_buf();
            let target = fixture.checkout.join("target");
            for member in ["debug", "release"] {
                let destination = root.join(member);
                let metadata = fs::symlink_metadata(&destination).unwrap();
                assert!(
                    metadata.is_dir() && !metadata.file_type().is_symlink(),
                    "{boundary}: {member} is not a real directory"
                );
                assert_eq!(
                    fs::read_link(target.join(member)).unwrap(),
                    destination,
                    "{boundary}: {member}"
                );
                assert!(
                    fs::metadata(target.join(member)).unwrap().is_dir(),
                    "{boundary}: {member} is a dangling member link"
                );
            }
            assert!(
                done.transition.is_none(),
                "{boundary}: the journal was kept"
            );
            assert!(fs::symlink_metadata(&target).unwrap().is_dir());
            assert!(
                !fixture
                    .checkout
                    .join(format!("{BACKUP_PREFIX}{}", done.generation))
                    .exists(),
                "{boundary}: the target backup was kept"
            );
        }
    }

    /// The member directories belong to the transaction's PUBLICATION step:
    /// a crash at P0 (the durable journal, before any target effect) leaves
    /// every member directory absent, and the replay that follows creates them
    /// together with their links.
    #[test]
    fn a_migration_creates_no_member_directory_before_the_journal() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        std::env::set_var(CRASH_ENVIRONMENT, "migration-p0");
        let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
        std::env::remove_var(CRASH_ENVIRONMENT);
        assert!(stopped.is_err(), "the injected P0 stop did not stop");
        assert!(!root.join("debug").exists(), "P0 created debug");
        assert!(!root.join("release").exists(), "P0 created release");
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ReceiptState::Prepared);
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_symlink());
        let done = replay_migration(&context, &fixture.control, stored).unwrap();
        assert_eq!(done.layout(), LayoutKind::Profile);
        for member in ["debug", "release"] {
            assert!(
                fs::symlink_metadata(root.join(member)).unwrap().is_dir(),
                "the replay left {member} absent"
            );
            assert!(
                fs::metadata(context.link().join(member)).unwrap().is_dir(),
                "the replay left {member} dangling"
            );
        }
    }

    /// P3: only the staging inode recorded at P1 is ever published as the real
    /// target. A directory that replaced the staging directory but holds
    /// identically shaped links is refused, and the transaction stays pending
    /// rather than being committed.
    #[test]
    fn a_replaced_staging_directory_is_never_swapped_in() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        std::env::set_var(CRASH_ENVIRONMENT, "migration-staged");
        let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
        std::env::remove_var(CRASH_ENVIRONMENT);
        assert!(stopped.is_err(), "the injected stop did not stop");
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        let journal = stored.transition.clone().unwrap();
        assert_eq!(journal.phase, MigrationPhase::Staged);
        assert!(journal.stage_dev.is_some() && journal.stage_ino.is_some());
        // Replace the staging DIRECTORY: same links, different inode.
        let replaced = journal.stage.with_extension("replaced");
        fs::rename(&journal.stage, &replaced).unwrap();
        fs::create_dir(&journal.stage).unwrap();
        fs::set_permissions(&journal.stage, fs::Permissions::from_mode(0o755)).unwrap();
        for member in &journal.members {
            symlink(
                member_destination(&journal.root.root, member),
                journal.stage.join(member),
            )
            .unwrap();
        }
        let refused = match replay_migration(&context, &fixture.control, stored) {
            Ok(_) => panic!("a replaced staging directory was published as the target"),
            Err(reason) => reason,
        };
        assert!(refused.contains("cannot proceed"), "{refused}");
        // Pending, never committed: the old link is at its backup, the target
        // was not published and the journal is kept.
        let target = context.link();
        assert!(
            fs::symlink_metadata(&target).is_err(),
            "a replaced staging directory became the target"
        );
        let backup = fs::symlink_metadata(&journal.backup).unwrap();
        assert!(backup.file_type().is_symlink());
        assert_eq!(fs::read_link(&journal.backup).unwrap(), journal.link_text);
        let kept = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert!(kept.transition.is_some(), "the pending journal was dropped");
        assert_eq!(kept.state, ReceiptState::Prepared);
        fs::remove_dir_all(&replaced).unwrap();
    }

    /// P3: a lookalike REAL directory at the target, holding the staged member
    /// links but a different inode, is never accepted as the already-swapped
    /// staging directory. Without the recorded staging identity this shape
    /// alone would be taken as a completed swap.
    #[test]
    fn a_lookalike_target_is_not_accepted_as_the_staged_directory() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        std::env::set_var(CRASH_ENVIRONMENT, "migration-staged");
        let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
        std::env::remove_var(CRASH_ENVIRONMENT);
        assert!(stopped.is_err(), "the injected stop did not stop");
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        let journal = stored.transition.clone().unwrap();
        assert_eq!(journal.phase, MigrationPhase::Staged);
        // P2 really moves the old link aside, then a lookalike directory takes
        // the target and the staging path is emptied: the occupied-target
        // branch must still refuse it.
        back_up_target(&journal).unwrap();
        fs::create_dir(&journal.link).unwrap();
        fs::set_permissions(&journal.link, fs::Permissions::from_mode(0o755)).unwrap();
        for member in &journal.members {
            symlink(
                member_destination(&journal.root.root, member),
                journal.link.join(member),
            )
            .unwrap();
        }
        fs::remove_dir_all(&journal.stage).unwrap();
        let refused = swap_in_staging(&journal).unwrap_err();
        assert!(refused.contains("cannot proceed"), "{refused}");
        // The lookalike is neither replaced nor removed.
        assert!(fs::symlink_metadata(&journal.link).unwrap().is_dir());
    }

    /// P3: the commit point requires the pinned pre-P4 backup of the old link.
    /// A backup that disappeared or was substituted leaves the migration
    /// pending instead of committing on the target's shape alone.
    #[test]
    fn a_missing_or_substituted_target_backup_blocks_commit() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        for substitute in [false, true] {
            let (fixture, legacy) = legacy_checkout();
            let context = fixture.context();
            std::env::set_var(CRASH_ENVIRONMENT, "migration-swapped");
            let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
            std::env::remove_var(CRASH_ENVIRONMENT);
            assert!(stopped.is_err(), "the injected stop did not stop");
            let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            let journal = stored.transition.clone().unwrap();
            assert_eq!(journal.phase, MigrationPhase::Swapped);
            fs::remove_file(&journal.backup).unwrap();
            if substitute {
                // A different inode with the same text is still not the pinned
                // original.
                symlink(&journal.link_text, &journal.backup).unwrap();
                assert_eq!(fs::read_link(&journal.backup).unwrap(), journal.link_text);
            }
            let refused = match replay_migration(&context, &fixture.control, stored) {
                Ok(_) => panic!("a migration committed without its pinned target backup"),
                Err(reason) => reason,
            };
            assert!(refused.contains("cannot proceed"), "{refused}");
            let kept = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            assert_eq!(
                kept.state,
                ReceiptState::Prepared,
                "the commit was not blocked"
            );
            assert!(kept.transition.is_some(), "the pending journal was dropped");
        }
    }

    /// P3: a root/1 upgrade whose pinned old-marker backup disappeared cannot
    /// reach the commit point, because a rollback must stay possible until P4.
    #[test]
    fn a_missing_marker_backup_blocks_commit() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        for stolen in [false, true] {
            let (fixture, legacy) = legacy_checkout();
            let context = fixture.context();
            make_root_one_marker(&fixture, &legacy);
            std::env::set_var(CRASH_ENVIRONMENT, "migration-marker");
            let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
            std::env::remove_var(CRASH_ENVIRONMENT);
            assert!(stopped.is_err(), "the injected stop did not stop");
            let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            let journal = stored.transition.clone().unwrap();
            assert_eq!(journal.phase, MigrationPhase::MarkerUpgraded);
            let backup = journal
                .marker_backup
                .clone()
                .expect("no recorded marker backup");
            // The pre-replay state that must survive untouched.
            let link_meta = fs::symlink_metadata(&journal.link).unwrap();
            let link_text = fs::read_link(&journal.link).unwrap();
            let staging = fs::symlink_metadata(&journal.stage).unwrap();
            if stolen {
                // STOLEN: moved away, so the recorded path is gone.
                fs::rename(&backup, backup.with_extension("stolen")).unwrap();
            } else {
                fs::remove_file(&backup).unwrap();
            }
            let refused = match replay_migration(&context, &fixture.control, stored) {
                Ok(_) => panic!("a root/1 upgrade proceeded without its pinned marker backup"),
                Err(reason) => reason,
            };
            assert!(refused.contains("cannot proceed"), "{refused}");
            // No pre-P4 effect happened: the old target link keeps its text and
            // inode (it was neither renamed nor swapped), the staging directory
            // keeps its inode, and the journal phase is still MarkerUpgraded.
            let after = fs::symlink_metadata(&journal.link).unwrap();
            assert_eq!(
                fs::read_link(&journal.link).unwrap(),
                link_text,
                "{refused}"
            );
            assert_eq!(after.ino(), link_meta.ino(), "{refused}");
            assert!(!after.file_type().is_dir(), "the target was swapped anyway");
            let staging_after = fs::symlink_metadata(&journal.stage).unwrap();
            assert_eq!(staging_after.ino(), staging.ino(), "staging changed");
            assert!(
                fs::symlink_metadata(&journal.backup).is_err(),
                "the target link was backed up anyway"
            );
            let kept = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
                .unwrap()
                .unwrap();
            assert_eq!(
                kept.state,
                ReceiptState::Prepared,
                "the operation was not kept pending"
            );
            assert_eq!(
                kept.transition.as_ref().map(|journal| journal.phase),
                Some(MigrationPhase::MarkerUpgraded),
                "the journal phase advanced"
            );
        }
    }

    /// P3: an identical-looking REPLACEMENT backup (same bytes, new inode) is not
    /// the pinned marker and cannot unlock the pre-P4 target effects.
    #[test]
    fn a_replaced_marker_backup_blocks_the_pre_p4_effects() {
        let _serial = MIGRATION_ENVIRONMENT_LOCK.lock().unwrap();
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        make_root_one_marker(&fixture, &legacy);
        std::env::set_var(CRASH_ENVIRONMENT, "migration-marker");
        let stopped = migrate_layout(&context, &fixture.control, legacy.clone());
        std::env::remove_var(CRASH_ENVIRONMENT);
        assert!(stopped.is_err(), "the injected stop did not stop");
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        let journal = stored.transition.clone().unwrap();
        let backup = journal
            .marker_backup
            .clone()
            .expect("no recorded marker backup");
        let bytes = fs::read(&backup).unwrap();
        fs::remove_file(&backup).unwrap();
        fs::write(&backup, &bytes).unwrap();
        let link_meta = fs::symlink_metadata(&journal.link).unwrap();
        let refused = match replay_migration(&context, &fixture.control, stored) {
            Ok(_) => panic!("a replaced marker backup unlocked the migration"),
            Err(reason) => reason,
        };
        assert!(
            refused.contains("not the pinned marker"),
            "the refusal did not name the replaced backup: {refused}"
        );
        assert_eq!(
            fs::symlink_metadata(&journal.link).unwrap().ino(),
            link_meta.ino(),
            "the target was renamed anyway"
        );
        assert!(
            fs::symlink_metadata(&journal.backup).is_err(),
            "the target link was backed up anyway"
        );
    }

    #[test]
    fn a_migration_waits_for_a_creator_of_the_target() {
        let (fixture, legacy) = legacy_checkout();
        let context = fixture.context();
        let root = legacy.selected().unwrap().root().to_path_buf();
        let journal_receipt = publish_migration_journal(
            &fixture.control,
            &legacy,
            Migration {
                generation: legacy.generation + 1,
                phase: MigrationPhase::Prepared,
                root: legacy.selected().unwrap().clone(),
                members: vec!["debug".to_owned(), "release".to_owned()],
                link: context.link(),
                link_text: root.clone(),
                link_dev: fs::symlink_metadata(context.link()).unwrap().dev(),
                link_ino: fs::symlink_metadata(context.link()).unwrap().ino(),
                parent_dev: fs::symlink_metadata(&fixture.checkout).unwrap().dev(),
                parent_ino: fs::symlink_metadata(&fixture.checkout).unwrap().ino(),
                stage: fixture
                    .checkout
                    .join(format!("{STAGE_PREFIX}{}", legacy.generation + 1)),
                backup: fixture
                    .checkout
                    .join(format!("{BACKUP_PREFIX}{}", legacy.generation + 1)),
                stage_dev: None,
                stage_ino: None,
                marker_pin: None,
                marker_backup: None,
                created_members: Vec::new(),
                prior_receipt_sha: receipt_digest(&fixture.control, &legacy).unwrap(),
                registry_revision: crate::prune::registry_revision(&fixture.control).unwrap(),
                created_ms: 1,
                updated_ms: 1,
            },
        )
        .unwrap();
        assert_eq!(journal_receipt.state, ReceiptState::Prepared);
        // A creator appears where the staging directory would be published.
        let stage = journal_receipt.transition.clone().unwrap().stage;
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("creator"), b"x").unwrap();
        let refused =
            replay_migration(&context, &fixture.control, journal_receipt.clone()).unwrap_err();
        assert!(
            refused.contains("cannot proceed") || refused.contains("did not create"),
            "{refused}"
        );
        // The old target link, the creator and the journal are all kept.
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(stage.join("creator")).unwrap(), b"x");
        let stored = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, ReceiptState::Prepared);
        // Every other command refuses while the journal is pending.
        assert!(replay(&context, journal_receipt).is_err());
    }

    #[test]
    fn adoption_selects_the_profile_layout_and_keeps_real_members() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let local = fixture.local();
        // A real nonempty member directory is never converted.
        fs::create_dir(context.link()).unwrap();
        fs::create_dir(context.link().join("release")).unwrap();
        fs::write(context.link().join("release/keep"), b"x").unwrap();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, selected, converted) = adopt_profile(&context, receipt).unwrap();
        assert!(converted);
        assert_eq!(committed.layout(), LayoutKind::Profile);
        assert_eq!(selected.root(), local.root());
        // target stays a real directory; debug is an owned link into the root
        // and release is still the real directory we left there.
        assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_dir());
        let debug = fs::symlink_metadata(context.link().join("debug")).unwrap();
        assert!(debug.file_type().is_symlink());
        assert_eq!(
            fs::read_link(context.link().join("debug")).unwrap(),
            member_destination(local.root(), "debug")
        );
        let release = fs::symlink_metadata(context.link().join("release")).unwrap();
        assert!(release.is_dir() && !release.file_type().is_symlink());
        assert_eq!(fs::read(context.link().join("release/keep")).unwrap(), b"x");
        // Both owned members share one immutable root nonce.
        let debug_nonce = committed
            .selected_for("debug")
            .unwrap()
            .nonce()
            .unwrap()
            .to_owned();
        assert_eq!(debug_nonce.len(), 32);
        assert_eq!(committed.selected_for("release").map(Selected::nonce), None);
        // Adoption is idempotent.
        let (again, _, converted) = adopt_profile(&context, committed.clone()).unwrap();
        assert!(!converted);
        assert_eq!(again.generation, committed.generation);
        // Detach restores a real empty member directory and keeps the root.
        let (detached, changed) = detach_profile(&context, &fixture.control, committed).unwrap();
        assert!(changed);
        assert_eq!(detached.state, ReceiptState::Detached);
        let debug = fs::symlink_metadata(context.link().join("debug")).unwrap();
        assert!(debug.is_dir() && !debug.file_type().is_symlink());
        assert!(local.root().is_dir());
        // ... and re-adoption converts that empty directory again.
        let (readopted, _, converted) = adopt_profile(&context, detached).unwrap();
        assert!(converted);
        assert!(fs::symlink_metadata(context.link().join("debug"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(readopted.selected().map(Selected::root), Some(local.root()));
    }

    #[test]
    fn adoption_relinks_to_the_desired_place_instead_of_trusting_the_old_receipt() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        // An older receipt selects an EXTERNAL root whose member links are
        // live. With no registration the desired place is now the local
        // fallback, so adoption must relink even though the links still match
        // the receipt.
        // A registered permanent drive that is NOT usable now (its mount point
        // is absent), so the desired place is the local fallback while the old
        // receipt still selects the external root.
        let registration = serde_json::json!({
            "version": 2,
            "mount_point": fixture.root.join("volume"),
            "volume_uuid": "11111111-2222-3333-4444-555555555555",
            "container_uuid": "22222222-3333-4444-5555-666666666666",
            "reserve_bytes": 16 * 1024 * 1024,
            "cap_bytes": 64 * 1024 * 1024,
            "quota_bytes": null,
            "capacity_bytes": null,
            "registered_ms": 1,
            "mode": "permanent"
        });
        let registration_path = fixture.control.join("removable.json");
        fs::write(
            &registration_path,
            serde_json::to_vec_pretty(&registration).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&registration_path, fs::Permissions::from_mode(0o600)).unwrap();
        let external = fixture.root.join("volume/oxide-outputs/checkout-0000");
        fs::create_dir_all(external.join("debug")).unwrap();
        fs::create_dir_all(external.join("release")).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        for member in ["debug", "release"] {
            symlink(
                external.join(member),
                fixture.checkout.join("target").join(member),
            )
            .unwrap();
        }
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        let selected = Selected {
            kind: RootKind::External,
            root: external.clone(),
            volume_uuid: marker.volume_uuid.clone(),
            nonce: marker.nonce.clone(),
            generation: 1,
        };
        receipt.place_member("debug").selected = Some(selected.clone());
        receipt.place_member("release").selected = Some(selected);
        let (adopted, desired, converted) = adopt_profile(&context, receipt.clone()).unwrap();
        assert!(converted, "a stale external placement was not relinked");
        assert_eq!(desired.kind, RootKind::Local);
        for member in ["debug", "release"] {
            assert_eq!(
                fs::read_link(fixture.checkout.join("target").join(member)).unwrap(),
                member_destination(desired.root(), member),
                "{member}"
            );
        }
        // A second adoption to the same desired place is a no-op.
        let (again, _, converted) = adopt_profile(&context, adopted.clone()).unwrap();
        assert!(
            !converted,
            "a matching desired placement was relinked again"
        );
        assert_eq!(again.generation, adopted.generation);
    }

    #[test]
    fn a_member_link_is_owned_only_through_its_root_marker() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        fs::create_dir_all(spec.root().join("debug")).unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        symlink(spec.root().join("debug"), &link).unwrap();
        assert_eq!(
            link_state(&link, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::OwnedLink(spec.root().join("debug"))
        );
        // A member-looking path whose root carries no marker is foreign: the
        // classifier never grants ownership by following an ancestor.
        let foreign = fixture.root.join("foreign/debug");
        fs::create_dir_all(&foreign).unwrap();
        let other = fixture.checkout.join("target/release");
        symlink(&foreign, &other).unwrap();
        assert_eq!(
            link_state(&other, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Foreign
        );
    }

    /// The exact accepted-proof counterexample (oxide#133 case 15): after a
    /// `--target <host triple>` build, `target/<triple>/debug` selects
    /// `R/<triple>/debug` and must be classified as owned through the marker at
    /// `R`, never as foreign.
    #[test]
    fn a_host_triple_member_link_is_owned_through_the_marker_at_its_root() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        for member in [
            "debug",
            "release",
            "aarch64-apple-darwin/debug",
            "aarch64-apple-darwin/release",
            "x86_64-unknown-linux-gnu/debug",
        ] {
            let destination = member_destination(spec.root(), member);
            fs::create_dir_all(&destination).unwrap();
            let link = fixture.checkout.join("target").join(member);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&destination, &link).unwrap();
            assert_eq!(
                link_state(&link, &fixture.checkout, &context.checkout_id).unwrap(),
                TargetState::OwnedLink(destination),
                "{member}"
            );
        }
        // The same shape under a root that carries no marker stays foreign: the
        // member suffix is a candidate, never a grant.
        let unmarked = fixture.root.join("unmarked");
        fs::create_dir_all(unmarked.join("aarch64-apple-darwin/debug")).unwrap();
        let unmarked_link = fixture.checkout.join("target/aarch64-apple-darwin/release");
        let _ = fs::remove_file(&unmarked_link);
        symlink(unmarked.join("aarch64-apple-darwin/debug"), &unmarked_link).unwrap();
        assert_eq!(
            link_state(&unmarked_link, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Foreign
        );
        // A root of another checkout's marker is equally not this checkout's.
        let other = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let other_context = other.context();
        let other_spec = other.local();
        ensure_root(&other_context, &other_spec, 1).unwrap();
        fs::create_dir_all(other_spec.root().join("aarch64-apple-darwin/release")).unwrap();
        let foreign_link = fixture
            .checkout
            .join("target/x86_64-unknown-linux-gnu/release");
        fs::create_dir_all(foreign_link.parent().unwrap()).unwrap();
        symlink(
            other_spec.root().join("aarch64-apple-darwin/release"),
            &foreign_link,
        )
        .unwrap();
        assert_eq!(
            link_state(&foreign_link, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Foreign
        );
        // A root whose marker exists but whose member destination does not is
        // dangling, not owned: detach must not act on it.
        let dangling = fixture
            .checkout
            .join("target/aarch64-unknown-linux-gnu/debug");
        fs::create_dir_all(dangling.parent().unwrap()).unwrap();
        symlink(
            member_destination(spec.root(), "aarch64-unknown-linux-gnu/debug"),
            &dangling,
        )
        .unwrap();
        assert_eq!(
            link_state(&dangling, &fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Dangling(member_destination(
                spec.root(),
                "aarch64-unknown-linux-gnu/debug"
            ))
        );
    }

    /// A committed profile receipt that places the host-triple member beside
    /// `debug` and `release`, with every link live and the root marked.
    fn host_triple_profile(fixture: &Fixture) -> (Context, Receipt, PathBuf) {
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let selected = spec.selected(1);
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
            receipt.place_member(member).selected = Some(selected.clone());
            let destination = member_destination(spec.root(), member);
            fs::create_dir_all(&destination).unwrap();
            let link = fixture.checkout.join("target").join(member);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(&destination, &link).unwrap();
        }
        write_receipt(&fixture.control, &receipt).unwrap();
        (context, receipt, spec.root().to_path_buf())
    }

    /// Detach recognises the host-triple and the plain members, converts each
    /// owned link to a real empty directory, and keeps the root, its marker and
    /// its payload byte for byte. Re-adoption restores every placement.
    #[test]
    fn detach_recognizes_host_triple_member_links_and_preserves_every_root_byte() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let (context, receipt, root) = host_triple_profile(&fixture);
        let marker = fs::read(root.join(MARKER)).unwrap();
        fs::write(root.join("debug/profileapp"), b"payload").unwrap();
        fs::write(
            root.join("aarch64-apple-darwin/debug/profileapp"),
            b"payload",
        )
        .unwrap();
        let root_metadata = fs::symlink_metadata(&root).unwrap();
        let (detached, changed) = detach_profile(&context, &fixture.control, receipt).unwrap();
        assert!(changed);
        assert_eq!(detached.state, ReceiptState::Detached);
        for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
            let link = fixture.checkout.join("target").join(member);
            let metadata = fs::symlink_metadata(&link).unwrap();
            assert!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "{member} is not a real directory after detach"
            );
            assert!(
                fs::read_dir(&link).unwrap().next().is_none(),
                "{member} is not empty after detach"
            );
            // The placement survives the detach as authority for re-adoption.
            assert!(
                detached
                    .placement(member)
                    .is_some_and(|placement| placement.selected.is_none()),
                "{member} lost its placement"
            );
        }
        assert!(fs::symlink_metadata(context.link()).unwrap().is_dir());
        // No root, marker or payload byte moved.
        assert_eq!(fs::read(root.join(MARKER)).unwrap(), marker);
        assert_eq!(fs::read(root.join("debug/profileapp")).unwrap(), b"payload");
        assert_eq!(
            fs::read(root.join("aarch64-apple-darwin/debug/profileapp")).unwrap(),
            b"payload"
        );
        assert_eq!(
            fs::symlink_metadata(&root).unwrap().ino(),
            root_metadata.ino()
        );
        // Re-adoption converts every recorded placement again, the triple one
        // included, and the triple link selects its own destination.
        let (readopted, _, converted) = adopt_profile(&context, detached).unwrap();
        assert!(converted);
        assert_eq!(readopted.state, ReceiptState::Committed);
        for member in ["debug", "release", "aarch64-apple-darwin/debug"] {
            let link = fixture.checkout.join("target").join(member);
            assert_eq!(
                fs::read_link(&link).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
        }
    }

    /// Every failure mode around a host-triple member refuses with no mutation:
    /// a wrong destination, an unmarked referent, a dangling destination and a
    /// relative (malformed) link each leave the link, the receipt and the other
    /// members exactly as they were.
    #[test]
    fn detach_refuses_unowned_host_triple_links_without_mutating_anything() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let (context, receipt, root) = host_triple_profile(&fixture);
        let triple_link = fixture.checkout.join("target/aarch64-apple-darwin/debug");
        let receipt_before =
            fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
        let check_untouched = |label: &str, link: &Path, text: &Path| {
            let error = detach_profile(&context, &fixture.control, receipt.clone())
                .expect_err(&format!("{label}: detach accepted an unowned member"));
            assert!(
                error.contains("target/aarch64-apple-darwin/debug"),
                "{label}: the refusal did not name the member: {error}"
            );
            assert_eq!(fs::read_link(link).unwrap(), text, "{label}");
            assert_eq!(
                fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
                receipt_before,
                "{label}: the refusal rewrote the receipt"
            );
            for member in ["debug", "release"] {
                let path = fixture.checkout.join("target").join(member);
                assert_eq!(
                    fs::read_link(&path).unwrap(),
                    member_destination(&root, member),
                    "{label}: {member} was changed"
                );
            }
        };
        // A wrong destination: the link names `R/debug` while the placement
        // promises `R/<triple>/debug`.
        fs::remove_file(&triple_link).unwrap();
        let wrong = member_destination(&root, "debug");
        symlink(&wrong, &triple_link).unwrap();
        check_untouched("wrong destination", &triple_link, &wrong);
        // An unmarked referent of the right shape.
        fs::remove_file(&triple_link).unwrap();
        let unmarked = root.join("unmarked/aarch64-apple-darwin/debug");
        fs::create_dir_all(&unmarked).unwrap();
        symlink(&unmarked, &triple_link).unwrap();
        check_untouched("unmarked referent", &triple_link, &unmarked);
        // A destination that does not exist although the root is ours.
        fs::remove_file(&triple_link).unwrap();
        let vanished = member_destination(&root, "aarch64-apple-darwin/release");
        symlink(&vanished, &triple_link).unwrap();
        check_untouched("dangling destination", &triple_link, &vanished);
        // A relative, malformed link text is never treated as owned.
        fs::remove_file(&triple_link).unwrap();
        let relative = PathBuf::from("../../.oxide-outputs/local/aarch64-apple-darwin/debug");
        symlink(&relative, &triple_link).unwrap();
        check_untouched("relative link", &triple_link, &relative);
    }

    #[test]
    fn profile_members_select_distinct_destinations_and_run_maps_only_owned_members() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (_, selected, _) = adopt_profile(&context, receipt).unwrap();
        let root = selected.root().to_path_buf();
        // Each member link selects its own approved subpath, never the root.
        for member in ["debug", "release"] {
            let link = context.link().join(member);
            assert_eq!(
                fs::read_link(&link).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
            assert!(root.join(member).is_dir(), "{member}: no member directory");
        }
        // `run` maps a program beneath its own owned member only.
        let owned = root.join("debug").join("profileapp");
        fs::write(&owned, b"program").unwrap();
        let mapped = program_target(
            &fixture.checkout,
            &root,
            &context.link().join("debug").join("profileapp"),
        );
        assert_eq!(mapped.as_deref(), Some(owned.as_path()));
        // A real member keeps Cargo's path; it never borrows another member's
        // root.
        let real = fixture.checkout.join("target/triple/debug/app");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, b"real").unwrap();
        assert_eq!(
            program_target(&fixture.checkout, &root, &real),
            None,
            "a real member program must keep Cargo's path"
        );
        assert!(owned.is_file());
    }

    #[test]
    fn a_dangling_member_link_never_transitions_automatically() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        // The external ROOT exists and carries this checkout's marker, but the
        // member directory it should contain does not: the member link dangles.
        fs::create_dir_all(&external.root).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        let dangling = member_destination(&external.root, "debug");
        symlink(&dangling, &link).unwrap();
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        let plan = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "debug".to_owned(),
                selected: Some(external.clone()),
            }],
        };
        let store = pool_store(&fixture.root.join("pool"));
        // With an internal fallback slot the refusal names the fallback.
        let fallback = Context::new(Some(fixture.control.clone()), fixture.checkout.clone())
            .with_pool_fallback(Some("container mismatch"));
        let refused = match bind_profile(&fallback, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => reason,
            Bind::Bound(_) => panic!("a dangling member link was bound"),
            Bind::Wait => panic!("a dangling refusal waited"),
        };
        assert!(refused.contains("not available"), "{refused}");
        assert!(refused.contains("fell back"), "{refused}");
        assert!(refused.contains("oxide outputs adopt"), "{refused}");
        // Without a fallback it still refuses rather than auto-transitioning.
        let refused = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => reason,
            Bind::Bound(_) => panic!("a dangling member link was bound without a fallback"),
            Bind::Wait => panic!("a dangling refusal waited"),
        };
        assert!(refused.contains("not available"), "{refused}");
        // The link and the receipt are untouched.
        assert_eq!(fs::read_link(&link).unwrap(), dangling);
        write_receipt(&fixture.control, &receipt).unwrap();
    }

    #[test]
    fn a_profile_fallback_slot_refuses_a_live_external_member_before_cargo() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        let plan = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "debug".to_owned(),
                selected: Some(external.clone()),
            }],
        };
        // A real external root carrying this checkout's marker, with its
        // member directory; the link is live and matches the keyed external
        // destination, but the command's slot fell back to the internal pool.
        fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        symlink(member_destination(&external.root, "debug"), &link).unwrap();
        let context = context.with_pool_fallback(Some("container mismatch"));
        let store = pool_store(&fixture.root.join("pool"));
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => {
                assert!(
                    reason.contains("fell back to the internal pool"),
                    "{reason}"
                )
            }
            Bind::Bound(_) => panic!("an external member link was bound on an internal fallback"),
            Bind::Wait => panic!("a fallback refusal waited"),
        }
    }

    /// P4: a still-mounted volume becoming unusable makes the fresh plan stop
    /// selecting the external root, so the legacy fallback refusal above never
    /// sees an external destination. Binding must still refuse: relinking the
    /// live external member to the internal root would be an automatic
    /// external -> local transition. No fallback slot is present here, so the
    /// refusal cannot come from the legacy #117 rule, and nothing changes.
    #[test]
    fn a_local_plan_never_relinks_a_committed_external_member() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        let recorded = fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap();
        // The mounted volume's root is now unusable, so the fresh plan places
        // the member on the internal fallback instead.
        let local = Selected {
            kind: RootKind::Local,
            root: fixture.checkout.join(".oxide-outputs/local"),
            volume_uuid: None,
            nonce: Some("11111111111111111111111111111111".to_owned()),
            generation: 1,
        };
        let plan = Plan {
            whole_target: false,
            members: vec![PlannedMember {
                member: "debug".to_owned(),
                selected: Some(local.clone()),
            }],
        };
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        let destination = member_destination(&external.root, "debug");
        symlink(&destination, &link).unwrap();
        let store = pool_store(&fixture.root.join("pool"));
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => {
                assert!(
                    reason.contains(
                        "never moves an owned external output to another pool automatically"
                    ),
                    "{reason}"
                );
                assert!(reason.contains("nothing was built"), "{reason}");
                assert!(
                    reason.contains(&destination.display().to_string()),
                    "{reason}"
                );
                assert!(
                    reason.contains(
                        &member_destination(&local.root, "debug")
                            .display()
                            .to_string()
                    ),
                    "{reason}"
                );
                assert!(reason.contains("oxide outputs adopt"), "{reason}");
            }
            Bind::Bound(_) => panic!("an external member was relinked to the internal pool"),
            Bind::Wait => panic!("an external-to-local refusal waited"),
        }
        // Nothing changed: the link still names the external member and the
        // committed receipt bytes are exactly what was recorded.
        assert_eq!(fs::read_link(&link).unwrap(), destination);
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
            recorded
        );
        assert!(!fixture.checkout.join(".oxide-outputs").exists());
        // When this command's own intermediate slot came from the internal
        // pool, the refusal names that reason too (the proof's absence oracle)
        // and still changes nothing.
        let fallback = fixture
            .context()
            .with_pool_fallback(Some("the volume is absent"));
        match bind_profile(&fallback, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => {
                assert!(
                    reason.contains("fell back to the internal pool (the volume is absent)"),
                    "{reason}"
                );
                assert!(
                    reason.contains(
                        "never moves an owned external output to another pool automatically"
                    ),
                    "{reason}"
                );
                assert!(reason.contains("nothing was built"), "{reason}");
            }
            Bind::Bound(_) => panic!("a fallback relinked an external member to the internal pool"),
            Bind::Wait => panic!("a fallback external-to-local refusal waited"),
        }
        assert_eq!(fs::read_link(&link).unwrap(), destination);
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &context.checkout_id)).unwrap(),
            recorded
        );
    }

    /// oxide#133 case 04 product regression. A member that JOINS a root this
    /// checkout already owns needs no fresh reservation: the commit of that
    /// root's first member consumed it. Demanding one made every later member
    /// of the same root -- a host `--target` triple after `debug`/`release` --
    /// refuse with "that reservation was cancelled or replaced", so the parent
    /// of this fix fails and this test distinguishes it from the correct
    /// behaviour instead of accepting any nonzero exit.
    #[test]
    fn a_new_member_joins_an_existing_owned_root_without_a_fresh_reservation() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let store = pool_store(&fixture.root.join("pool"));
        // Committed debug + release on one owned root, exactly as a first
        // conversion leaves them: no reservation survives that commit.
        let mut receipt = profile_receipt(&fixture);
        receipt.reserved = None;
        write_receipt(&fixture.control, &receipt).unwrap();
        let selected = receipt.selected().cloned().unwrap();
        let root = selected.root.clone();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::Local,
            volume_uuid: None,
            nonce: selected.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        for member in ["debug", "release"] {
            fs::create_dir_all(member_destination(&root, member)).unwrap();
            symlink(
                member_destination(&root, member),
                fixture.checkout.join("target").join(member),
            )
            .unwrap();
        }
        // The production planner keys the new triple member on the SAME owned
        // root, its existing nonce and the root's own generation.
        let triple_member = "aarch64-apple-darwin/debug";
        let declaration =
            crate::layout::Declaration::new([crate::layout::Member::parse(triple_member).unwrap()])
                .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let wanted = plan.selected().cloned().unwrap();
        assert_eq!(
            wanted.root, root,
            "the planner did not reuse the owned root"
        );
        assert_eq!(
            wanted.nonce(),
            selected.nonce(),
            "the planner did not reuse the root nonce"
        );
        assert_eq!(wanted.generation, 1, "the planner moved the generation");
        let outcome = match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Bound(outcome) => outcome,
            Bind::Refuse(reason) => panic!("a new member of an owned root was refused: {reason}"),
            Bind::Wait => panic!("a new member of an owned root waited"),
        };
        assert_eq!(outcome.root, Some(root.clone()));
        let note = outcome.note.unwrap_or_default();
        assert!(note.contains(triple_member), "{note}");
        assert!(note.contains("profile member links"), "{note}");
        // The new link names its own member destination under the shared root,
        // and the two committed links are untouched.
        assert_eq!(
            fs::read_link(fixture.checkout.join("target").join(triple_member)).unwrap(),
            member_destination(&root, triple_member)
        );
        for member in ["debug", "release"] {
            assert_eq!(
                fs::read_link(fixture.checkout.join("target").join(member)).unwrap(),
                member_destination(&root, member)
            );
        }
        // The receipt now places all three members on that one root and nonce.
        let committed = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        let mut members: Vec<&str> = Vec::new();
        for member in ["debug", "release", triple_member] {
            let placed = committed
                .selected_for(member)
                .unwrap_or_else(|| panic!("the receipt lost {member}"));
            assert_eq!(placed.root, root, "{member}");
            assert_eq!(placed.nonce(), selected.nonce(), "{member}");
            members.push(member);
        }
        assert_eq!(committed.links.len(), 3, "{members:?}");
    }

    /// oxide#165 counterfactual: with no recreate step, a converted checkout
    /// whose `target` directory was deleted (`rm -rf target`, a task-local
    /// build-output cleanup) fails inside the member transition with ENOENT on
    /// the path beneath the absent directory and never recreates it. This is
    /// the exact path the binding fix guards, driven directly here so the
    /// failure mode stays recorded: the target is still absent afterwards and
    /// the receipt is left prepared, which is why every later command kept
    /// retrying and failing the same way.
    #[test]
    fn a_deleted_target_directory_fails_the_member_transition_with_enoent() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
        fs::remove_dir_all(context.link()).unwrap();
        assert_eq!(
            target_state(&fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Missing
        );
        let refused = transition_at(
            &context,
            &committed,
            "debug",
            &spec_for(&selected),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .expect_err("a member transition published a link inside an absent target");
        assert!(refused.contains("No such file or directory"), "{refused}");
        assert!(
            refused.contains(&context.link().display().to_string()),
            "{refused}"
        );
        assert_eq!(
            target_state(&fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Missing,
            "the failed transition recreated the target directory"
        );
        let after = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            after.state,
            ReceiptState::Prepared,
            "the write-ahead receipt is gone"
        );
    }

    /// oxide#165: a converted checkout's `target` directory was deleted. The
    /// binding recreates that one real directory exclusively and publishes the
    /// owned member links through the existing transition, into the SAME root:
    /// the root's payload, the committed root and nonce are kept and nothing is
    /// relinked to a new place.
    #[test]
    fn a_deleted_target_directory_is_recreated_and_both_members_bind() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let store = pool_store(&fixture.root.join("pool"));
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, selected, _) = adopt_profile(&context, receipt).unwrap();
        let root = selected.root().to_path_buf();
        fs::write(root.join("sentinel"), b"root").unwrap();
        fs::write(
            member_destination(&root, "debug").join("old-output"),
            b"old",
        )
        .unwrap();
        let declaration =
            crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        fs::remove_dir_all(context.link()).unwrap();
        assert_eq!(
            target_state(&fixture.checkout, &context.checkout_id).unwrap(),
            TargetState::Missing
        );
        let outcome = match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
            Bind::Bound(outcome) => outcome,
            Bind::Refuse(reason) => panic!("a deleted target directory was refused: {reason}"),
            Bind::Wait => panic!("a deleted target directory waited"),
        };
        let target = context.link();
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "the recreated target is not a real directory"
        );
        assert_eq!(
            metadata.mode() & 0o777,
            0o755,
            "the recreated target does not carry the conversion's mode"
        );
        assert_eq!(outcome.root, Some(root.clone()));
        assert!(
            outcome
                .note
                .unwrap_or_default()
                .contains("profile member links"),
            "the binding did not report the owned profile members"
        );
        for member in ["debug", "release"] {
            assert_eq!(
                fs::read_link(target.join(member)).unwrap(),
                member_destination(&root, member),
                "{member}"
            );
        }
        // The owned root is the same one: its payload and the old output are
        // exactly as they were.
        assert_eq!(fs::read(root.join("sentinel")).unwrap(), b"root");
        assert_eq!(
            fs::read(member_destination(&root, "debug").join("old-output")).unwrap(),
            b"old"
        );
        let after = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(after.state, ReceiptState::Committed);
        assert!(after.generation > committed.generation);
        for member in ["debug", "release"] {
            let placed = after
                .selected_for(member)
                .unwrap_or_else(|| panic!("the receipt lost {member}"));
            assert_eq!(placed.root, root, "{member}");
            assert_eq!(placed.nonce(), selected.nonce(), "{member}");
        }
    }

    /// oxide#165 refusals: an `target` that is a file or a foreign link still
    /// refuses before Cargo even when the command was keyed while the target
    /// directory was real, and nothing at that path or behind a link is
    /// replaced, followed or written into.
    #[test]
    fn a_file_or_foreign_link_at_the_target_refuses_a_profile_binding() {
        for outside in [false, true] {
            let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
            let context = fixture.context();
            let store = pool_store(&fixture.root.join("pool"));
            let receipt =
                Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
            let (committed, _selected, _) = adopt_profile(&context, receipt).unwrap();
            let declaration =
                crate::layout::declared(&["debug".to_owned(), "release".to_owned()]).unwrap();
            let plan = planned_destination(
                Some(&fixture.control),
                &fixture.checkout,
                &declaration,
                None,
                false,
            )
            .unwrap()
            .unwrap();
            let target = context.link();
            let proof = receipt_proof(&fixture);
            fs::remove_dir_all(&target).unwrap();
            let referent = fixture.root.join("elsewhere");
            fs::create_dir(&referent).unwrap();
            fs::write(referent.join("sentinel"), b"foreign").unwrap();
            if outside {
                symlink(&referent, &target).unwrap();
            } else {
                fs::write(&target, b"file").unwrap();
            }
            let refused = match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
                Bind::Refuse(reason) => reason,
                Bind::Bound(_) => panic!("a target that is not a real directory was bound"),
                Bind::Wait => panic!("a target that is not a real directory waited"),
            };
            assert!(
                refused.contains("is never replaced or followed"),
                "{refused}"
            );
            assert!(refused.contains("Nothing was changed"), "{refused}");
            // The path is exactly what it was, and nothing was written behind
            // a link.
            if outside {
                assert_eq!(fs::read_link(&target).unwrap(), referent);
                assert_eq!(fs::read(referent.join("sentinel")).unwrap(), b"foreign");
                assert_eq!(fs::read_dir(&referent).unwrap().count(), 1);
            } else {
                assert_eq!(fs::read(&target).unwrap(), b"file");
            }
            // The refusal changed no receipt byte.
            assert_eq!(receipt_proof(&fixture), proof);
        }
    }

    /// oxide#165 race and scope: a `target` that appears while the absent
    /// directory is being recreated is kept — only a real directory is accepted
    /// as the creator's result, a file or a link is refused, never replaced —
    /// and the recreate belongs to the profile layout alone, so a whole-target
    /// checkout's deleted link is still recreated by the legacy transition and
    /// a never-converted checkout is left with no target at all.
    #[test]
    fn a_concurrent_creator_is_kept_and_whole_target_is_unchanged() {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let target = fixture.checkout.join("target");
        recreate_target_directory(&fixture.checkout).unwrap();
        let metadata = fs::symlink_metadata(&target).unwrap();
        assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
        assert_eq!(metadata.mode() & 0o777, 0o755);
        // The creator that already made the real directory wins: its content is
        // left exactly as it is.
        fs::write(target.join("sentinel"), b"creator").unwrap();
        recreate_target_directory(&fixture.checkout).unwrap();
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"creator");
        // Anything else at that path is refused and kept.
        fs::remove_dir_all(&target).unwrap();
        fs::write(&target, b"file").unwrap();
        let refused = recreate_target_directory(&fixture.checkout).unwrap_err();
        assert!(refused.contains("is not a real directory"), "{refused}");
        assert_eq!(fs::read(&target).unwrap(), b"file");
        fs::remove_file(&target).unwrap();
        let referent = fixture.root.join("elsewhere");
        fs::create_dir(&referent).unwrap();
        fs::write(referent.join("sentinel"), b"foreign").unwrap();
        symlink(&referent, &target).unwrap();
        let refused = recreate_target_directory(&fixture.checkout).unwrap_err();
        assert!(refused.contains("is not a real directory"), "{refused}");
        assert_eq!(fs::read_link(&target).unwrap(), referent);
        assert_eq!(fs::read(referent.join("sentinel")).unwrap(), b"foreign");

        // Scope: a committed whole-target checkout whose owned link was
        // deleted still binds its own root through the legacy transition.
        let whole = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let context = whole.context();
        let store = pool_store(&whole.root.join("pool"));
        let receipt = transition(
            &context,
            &whole.receipt(),
            &whole.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let whole_target = context.link();
        fs::remove_file(&whole_target).unwrap();
        let wanted = desired_selection(&context, &receipt).unwrap().unwrap();
        let plan = whole_target_plan(Some(wanted));
        match bind_under_binding(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Bound(outcome) => {
                assert_eq!(outcome.root.as_deref(), Some(whole.local().root()))
            }
            Bind::Refuse(reason) => panic!("a deleted whole-target link refused: {reason}"),
            Bind::Wait => panic!("a deleted whole-target link waited"),
        }
        assert_eq!(fs::read_link(&whole_target).unwrap(), whole.local().root());

        // Scope: a never-converted checkout keeps no target, no receipt and no
        // owned root — the recreate is never reached.
        let plain = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let plain_store = pool_store(&plain.root.join("pool"));
        let binding =
            bind_for_command(&plain_store, &plain.checkout, 0, None, None, false, None).unwrap();
        assert_eq!(binding.outcome.root, None);
        assert!(!plain.checkout.join("target").exists());
        assert!(!receipt_path(plain_store.control(), &context_id(&plain)).exists());
    }

    /// P4, ROOT check: the same refusal through the PRODUCTION planning path.
    /// The fixture registers a permanent drive whose volume cannot be verified,
    /// so `planned_destination` itself stops selecting the committed external
    /// root and returns a local plan; binding that derived plan must still
    /// refuse before Cargo. The manual `Plan` in the regression above is a
    /// binding-only control, not this one.
    #[test]
    fn a_planner_derived_local_plan_refuses_a_committed_external_member() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        register(&fixture, DriveMode::Permanent);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        // A real external root carrying this checkout's marker and member
        // directory, with the live owned link into it.
        fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        let destination = member_destination(&external.root, "debug");
        symlink(&destination, &link).unwrap();
        let declaration =
            crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()])
                .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.selected().map(Selected::kind),
            Some(RootKind::Local),
            "the production planner did not fall back to the local spec"
        );
        let store = pool_store(&fixture.root.join("pool"));
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => {
                assert!(
                    reason.contains(
                        "never moves an owned external output to another pool automatically"
                    ),
                    "{reason}"
                );
                assert!(reason.contains("nothing was built"), "{reason}");
                assert!(
                    reason.contains(&destination.display().to_string()),
                    "{reason}"
                );
            }
            Bind::Bound(_) => panic!("a planner-derived local plan relinked an external member"),
            Bind::Wait => panic!("a planner-derived refusal waited"),
        }
        assert_eq!(fs::read_link(&link).unwrap(), destination);
    }

    /// oxide#133 case14 CAUSAL regression. The diagnostic's case14 refused
    /// with the right reason and then found the committed receipt bytes
    /// changed. Traced through the production preflight: `reserve` runs before
    /// the lease and, with the registered volume away, the fresh selection is
    /// the local fallback root, so it durably reserved a nonce for a root this
    /// command then never builds into and rewrote the receipt of a command that
    /// builds nothing. The parent fails the first byte comparison below; the
    /// refusal itself (and every authority field) must be unchanged.
    #[test]
    fn a_refusal_after_the_volume_is_away_reserves_nothing() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        register(&fixture, DriveMode::Permanent);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let link = fixture.checkout.join("target/debug");
        let destination = member_destination(&external.root, "debug");
        symlink(&destination, &link).unwrap();
        let declaration =
            crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()])
                .unwrap();
        let receipt_file = receipt_path(&fixture.control, &context.checkout_id);
        let before = fs::read(&receipt_file).unwrap();
        let store = pool_store(&fixture.root.join("pool"));
        // Production order: the pre-lease reservation, then the plan, then bind.
        reserve(
            &store,
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            Some("the registered volume is away"),
        )
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.selected().map(Selected::kind),
            Some(RootKind::Local),
            "the production planner did not fall back to the local spec"
        );
        assert_eq!(
            fs::read(&receipt_file).unwrap(),
            before,
            "the pre-lease reservation rewrote the committed receipt of a refusing command"
        );
        // The command still refuses for the exact P4 reason with no effect.
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => {
                assert!(
                    reason.contains(
                        "never moves an owned external output to another pool automatically"
                    ),
                    "{reason}"
                );
                assert!(reason.contains("nothing was built"), "{reason}");
                assert!(
                    reason.contains(&destination.display().to_string()),
                    "{reason}"
                );
            }
            Bind::Bound(_) => panic!("a fallback refusal relinked an external member"),
            Bind::Wait => panic!("a fallback refusal waited"),
        }
        assert_eq!(
            fs::read(&receipt_file).unwrap(),
            before,
            "the refusal changed the committed receipt"
        );
        assert_eq!(fs::read_link(&link).unwrap(), destination);
        let recorded = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert!(
            recorded.reserved().is_none(),
            "a refusing command left a root reservation behind"
        );
        assert_eq!(recorded.generation, 1, "the refusal moved the generation");
        assert_eq!(recorded.selected_for("debug"), Some(&external));
    }

    /// oxide#133 review finding (b): the pre-lease predicate must use binding's
    /// REQUESTED member set, not any committed external placement. In a profile
    /// checkout whose `debug` stays on an external root, a command that requests
    /// only a NEW nested member is not refused by binding's P4 rule, so its
    /// local future root keeps its reservation and the transition proceeds. The
    /// checkout-wide predicate skipped that reservation and left
    /// `reservation_matches` refusing.
    #[test]
    fn a_new_member_beside_a_committed_external_member_keeps_its_reservation() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        register(&fixture, DriveMode::Permanent);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        let mut receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.place_member("debug").selected = Some(external.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        fs::create_dir_all(member_destination(&external.root, "debug")).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(fixture.checkout.join("target")).unwrap();
        let debug_link = fixture.checkout.join("target/debug");
        symlink(member_destination(&external.root, "debug"), &debug_link).unwrap();
        let nested = "x86_64-unknown-linux-gnu/debug";
        let declaration =
            crate::layout::Declaration::new([crate::layout::Member::parse(nested).unwrap()])
                .unwrap();
        let store = pool_store(&fixture.root.join("pool"));
        // Production order: reservation, then plan, then bind.
        reserve(
            &store,
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            Some("the registered volume is away"),
        )
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.selected().map(Selected::kind),
            Some(RootKind::Local),
            "the production planner did not fall back to the local spec"
        );
        let reserved = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        let reservation = reserved
            .reserved()
            .expect("a requested new member lost its required reservation");
        assert_eq!(
            Some(reservation.kind),
            plan.selected().map(Selected::kind),
            "the reservation is not for the planned local root"
        );
        assert_eq!(
            Some(reservation.nonce.as_str()),
            plan.selected().and_then(Selected::nonce),
            "the reservation does not carry the keyed nonce"
        );
        // The exact gate binding applies to a MISSING member link
        // (`member_root_authority`): the committed sibling's external placement
        // must not block the new member's local reservation. On the parent this
        // is the refusal ("reservation was cancelled or replaced") that the
        // checkout-wide predicate caused.
        let wanted = plan.selected().unwrap();
        assert!(
            !refuses_committed_external(&reserved, wanted, &declaration),
            "the pre-lease predicate still claims this accepted command will refuse"
        );
        reservation_matches(&context, &reserved, wanted)
            .expect("the new member's local reservation is not usable at binding");
        assert_eq!(
            fs::read_link(&debug_link).unwrap(),
            member_destination(&external.root, "debug"),
            "the committed external member link moved"
        );
    }

    /// oxide#133 review finding (a): a whole-target receipt never refuses through
    /// binding's per-member P4 rule, so its local fallback root keeps the
    /// reservation `bind_under_binding` requires. The checkout-wide predicate
    /// skipped it and left the #117 first local fallback refusing.
    #[test]
    fn a_whole_target_local_fallback_keeps_its_reservation() {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let (context, _external) = committed_whole_target_external(&fixture);
        let declaration =
            crate::layout::Declaration::new([crate::layout::Member::parse("debug").unwrap()])
                .unwrap();
        let store = pool_store(&fixture.root.join("pool"));
        // Production order: reservation, then plan, then bind.
        reserve(
            &store,
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            Some("the registered volume is away"),
        )
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert!(
            plan.whole_target,
            "the legacy checkout planned a profile layout"
        );
        assert_eq!(
            plan.selected().map(Selected::kind),
            Some(RootKind::Local),
            "the production planner did not fall back to the local spec"
        );
        let reserved = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert!(
            reserved.reserved().is_some_and(
                |reservation| Some(reservation.kind) == plan.selected().map(Selected::kind)
            ),
            "a whole-target local fallback lost its required reservation"
        );
        match bind_under_binding(&context, &store, 0, &reserved, Some(&plan), false) {
            Bind::Bound(outcome) => {
                assert_eq!(
                    outcome.root.as_deref(),
                    Some(fixture.local().root()),
                    "the whole-target fallback did not bind the local root"
                );
            }
            Bind::Refuse(reason) => panic!("the whole-target local fallback refused: {reason}"),
            Bind::Wait => panic!("the whole-target local fallback waited"),
        }
    }

    /// The committed whole-target fixture both reservation tests start from: a
    /// permanent-mode receipt selecting a marker-carrying EXTERNAL root with the
    /// legacy `target` link published, so a later command's desired selection
    /// falls back to the (absent) local root. Returns its context and selection.
    fn committed_whole_target_external(fixture: &Fixture) -> (Context, Selected) {
        let context = fixture.context();
        register(fixture, DriveMode::Permanent);
        let external = Selected {
            kind: RootKind::External,
            root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
            volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            nonce: Some("99999999999999999999999999999999".to_owned()),
            generation: 1,
        };
        let mut receipt = Receipt::first(&fixture.checkout, &context.checkout_id);
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(external.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        fs::create_dir_all(&external.root).unwrap();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::External,
            volume_uuid: external.volume_uuid.clone(),
            nonce: external.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::write(
            external.root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        let legacy_link = fixture.checkout.join("target");
        symlink(&external.root, &legacy_link).unwrap();
        (context, external)
    }

    /// oxide#164 review finding: an external subcommand derives an EMPTY set in
    /// a whole-target checkout, and the shipped whole-target plan there can
    /// name a FUTURE root (the first local fallback to an absent root,
    /// oxide#117). The reservation must therefore still happen, or binding
    /// refuses the keyed plan with no nonce.
    ///
    /// Counterfactual: skipping the reservation for every derived command (the
    /// oxide#164 head `87d9ff1`) makes this test's binding refuse.
    #[test]
    fn an_external_subcommand_reserves_a_future_whole_target_root() {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let (context, _external) = committed_whole_target_external(&fixture);
        let external = crate::cli::parse_leased("oxfixture", &[]).unwrap();
        let declaration =
            lease_declaration(&external, Some(&fixture.control), &fixture.checkout, None).unwrap();
        assert!(
            declaration.is_empty(),
            "a whole-target checkout derived a member set"
        );
        // The decision main.rs makes: an empty derived set keeps the shipped
        // reservation path, a non-empty derived set skips it.
        assert!(needs_reservation(true, &declaration));
        assert!(!needs_reservation(
            true,
            &declaration_of(&["debug".to_owned()])
        ));
        assert!(needs_reservation(false, &declaration));
        let store = pool_store(&fixture.root.join("pool"));
        // Production order: reservation, then plan, then bind.
        reserve(
            &store,
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            Some("the registered volume is away"),
        )
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            Some("the registered volume is away"),
            false,
        )
        .unwrap()
        .unwrap();
        assert!(
            plan.whole_target,
            "the legacy checkout planned a profile layout"
        );
        assert_eq!(
            plan.selected().map(Selected::kind),
            Some(RootKind::Local),
            "the production planner did not fall back to the local spec"
        );
        let reserved = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert!(
            reserved.reserved().is_some_and(
                |reservation| Some(reservation.kind) == plan.selected().map(Selected::kind)
            ),
            "the external subcommand's future whole-target root has no reservation"
        );
        match bind_under_binding(&context, &store, 0, &reserved, Some(&plan), false) {
            Bind::Bound(outcome) => assert_eq!(
                outcome.root.as_deref(),
                Some(fixture.local().root()),
                "the whole-target fallback did not bind the local root"
            ),
            Bind::Refuse(reason) => {
                panic!("the external subcommand's future whole-target root refused: {reason}")
            }
            Bind::Wait => panic!("the external subcommand's future whole-target root waited"),
        }
    }

    #[test]
    fn adoption_refuses_a_whole_target_link_without_touching_it() {
        let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        let context = fixture.context();
        let legacy = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let target = context.link();
        assert!(fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink());
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let refused = adopt_profile(&context, receipt).unwrap_err();
        assert!(refused.contains("whole-target"), "{refused}");
        assert_eq!(fs::read_link(&target).unwrap(), fixture.local().root());
        assert_eq!(legacy.state, ReceiptState::Committed);
        assert!(!target.join("debug").exists());
    }

    #[test]
    fn a_profile_receipt_round_trips_in_its_own_version() {
        let fixture = Fixture::new();
        let mut receipt = profile_receipt(&fixture);
        receipt.reserved = None;
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(doc["version"], PROFILE_RECEIPT_VERSION);
        assert_eq!(doc["layout"], "profile");
        assert_eq!(doc["links"][0]["member"], "debug");
        assert_eq!(doc["links"][1]["member"], "release");
        let back: Receipt = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, receipt);
        assert_eq!(back.layout(), LayoutKind::Profile);
        assert_eq!(
            back.selected_for("debug").map(Selected::root),
            Some(fixture.local().root())
        );
        // A version-3 receipt without its layout, or with a per-placement
        // reservation, is refused rather than guessed.
        let mut missing = doc.clone();
        missing.as_object_mut().unwrap().remove("layout");
        assert!(serde_json::from_value::<Receipt>(missing).is_err());
        let mut misplaced = doc;
        misplaced["links"][0]["reserved"] = serde_json::json!({
            "id": "33333333333333333333333333333333",
            "nonce": "44444444444444444444444444444444",
            "kind": "local",
            "root": "/nowhere",
            "generation": 2,
            "created_ms": 1,
        });
        assert!(serde_json::from_value::<Receipt>(misplaced).is_err());
    }

    #[test]
    fn a_legacy_receipt_keeps_its_published_shape_and_reservation() {
        let fixture = Fixture::new();
        let mut receipt = Receipt::first(
            &fixture.checkout,
            &identity::digest(&fixture.checkout.to_string_lossy()),
        );
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.link_mut().selected = Some(fixture.local().selected(1));
        receipt.reserved = Some(Reservation {
            id: "55555555555555555555555555555555".to_owned(),
            nonce: "66666666666666666666666666666666".to_owned(),
            kind: RootKind::Local,
            root: fixture.local().root().to_path_buf(),
            volume_uuid: None,
            generation: 2,
            created_ms: 1,
            overflow: None,
        });
        let doc = serde_json::to_value(&receipt).unwrap();
        assert_eq!(doc["version"], RECEIPT_VERSION);
        assert!(doc.get("layout").is_none());
        assert!(doc.get("reserved").is_none());
        assert_eq!(
            doc["links"][0]["reserved"]["id"],
            "55555555555555555555555555555555"
        );
        let back: Receipt = serde_json::from_value(doc).unwrap();
        assert_eq!(back, receipt);
        assert_eq!(back.layout(), LayoutKind::WholeTarget);
    }

    #[test]
    fn profile_slot_keys_use_the_outputs_3_namespace_only() {
        let fixture = Fixture::new();
        let local = fixture.local();
        let selected = local
            .selected(3)
            .with_nonce(Some("88888888888888888888888888888888".to_owned()));
        let profile = Plan {
            whole_target: false,
            members: vec![
                PlannedMember {
                    member: "debug".to_owned(),
                    selected: Some(selected.clone()),
                },
                PlannedMember {
                    member: "release".to_owned(),
                    selected: None,
                },
            ],
        };
        let facts = destination_facts(Some(&profile));
        let lines: Vec<&str> = facts.lines().collect();
        assert_eq!(lines.len(), 2, "{facts}");
        assert!(lines[0].starts_with("outputs-3 "), "{facts}");
        // The token after the namespace is the root nonce, never another
        // namespace prefix (the fixture path itself contains "outputs-").
        let tokens: Vec<&str> = lines[0].split_whitespace().collect();
        assert_eq!(tokens[0], "outputs-3", "{facts}");
        assert_eq!(
            tokens[1], "88888888888888888888888888888888",
            "the second token is not the root nonce: {facts}"
        );
        assert!(lines[0].ends_with(" debug"), "{facts}");
        assert_eq!(lines[1], "real release");
        // The legacy namespace is exactly the published one, with no profile
        // suffix or extra token.
        let legacy = Plan {
            whole_target: true,
            members: vec![PlannedMember {
                member: MEMBER.to_owned(),
                selected: Some(selected.clone()),
            }],
        };
        let legacy_facts = destination_facts(Some(&legacy));
        // The namespace is the FIRST TOKEN only. The fixture path itself can
        // contain "outputs-3" (its random temporary suffix), so a substring
        // test is wrong: only a leading namespace token is a namespace.
        let legacy_tokens: Vec<&str> = legacy_facts.split_whitespace().collect();
        assert_eq!(legacy_tokens[0], "outputs-2", "{legacy_facts}");
        assert!(
            !legacy_facts
                .lines()
                .any(|line| line.split_whitespace().next() == Some("outputs-3")),
            "{legacy_facts}"
        );
        assert_eq!(legacy_facts.lines().count(), 1);
        assert!(legacy_facts.ends_with("g3"), "{legacy_facts}");
    }

    #[test]
    fn ephemeral_mode_binds_a_converted_profile_checkout_without_relinking() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (receipt, selected, _) = adopt_profile(&context, receipt).unwrap();
        let declaration = crate::layout::Declaration::new([
            crate::layout::Member::parse("debug").unwrap(),
            crate::layout::Member::parse("release").unwrap(),
        ])
        .unwrap();
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        // The live member links are exactly what the slot was keyed for.
        let outcome = bind_profile_sealed(&context, &receipt, Some(&plan)).unwrap();
        assert_eq!(outcome.root.as_deref(), Some(selected.root()));
        assert!(outcome.note.is_some());
        // A member link that selects another root refuses: ephemeral mode
        // never relinks it.
        let debug = fixture.checkout.join("target/debug");
        fs::remove_file(&debug).unwrap();
        symlink("/tmp", &debug).unwrap();
        let refused = match bind_profile_sealed(&context, &receipt, Some(&plan)) {
            Ok(_) => panic!("a foreign member link was bound"),
            Err(reason) => reason,
        };
        assert!(refused.contains("ephemeral"), "{refused}");
        assert_eq!(
            fs::read_link(&debug).unwrap(),
            std::path::PathBuf::from("/tmp")
        );
    }

    #[test]
    fn the_terminal_prune_image_is_fixed_and_replay_accepts_exactly_it() {
        let fixture = Fixture::new();
        let mut receipt = profile_receipt(&fixture);
        write_receipt(&fixture.control, &receipt).unwrap();
        let id = "e".repeat(32);
        let first = terminal_receipt_image(
            &fixture.control,
            &fixture.checkout,
            &context_id(&fixture),
            &id,
            "deleted",
            42,
        )
        .unwrap();
        let again = terminal_receipt_image(
            &fixture.control,
            &fixture.checkout,
            &context_id(&fixture),
            &id,
            "deleted",
            42,
        )
        .unwrap();
        assert_eq!(first, again, "the terminal image is not deterministic");
        mark_pruned_at(
            &fixture.control,
            &fixture.checkout,
            &context_id(&fixture),
            &id,
            "deleted",
            42,
        )
        .unwrap();
        let published = fs::read(receipt_path(&fixture.control, &context_id(&fixture))).unwrap();
        assert_eq!(
            published, first,
            "the published image is not the recorded one"
        );
        let terminal: Receipt = serde_json::from_slice(&published).unwrap();
        assert_eq!(terminal.state, ReceiptState::Pruned);
        assert_eq!(
            terminal
                .pruned
                .as_ref()
                .map(|record| (record.intent.as_str(), record.at_ms)),
            Some((id.as_str(), 42))
        );
        // A later replay of the same intent accepts the exact image and does
        // not rewrite it; a different fixed timestamp is refused.
        mark_pruned_at(
            &fixture.control,
            &fixture.checkout,
            &context_id(&fixture),
            &id,
            "deleted",
            42,
        )
        .unwrap();
        assert_eq!(
            fs::read(receipt_path(&fixture.control, &context_id(&fixture))).unwrap(),
            first
        );
        assert!(terminal_receipt_image(
            &fixture.control,
            &fixture.checkout,
            &context_id(&fixture),
            &"f".repeat(32),
            "deleted",
            43,
        )
        .is_err());
        receipt.state = ReceiptState::Committed;
        let _ = receipt;
    }

    #[test]
    fn a_profile_receipt_refuses_unsorted_overlapping_or_foreign_members() {
        let fixture = Fixture::new();
        let id = identity::digest(&fixture.checkout.to_string_lossy());
        let mut receipt = profile_receipt(&fixture);
        receipt.links.reverse();
        assert!(receipt.validate(&fixture.checkout, &id).is_err());
        let mut receipt = profile_receipt(&fixture);
        let duplicate = receipt.links[0].clone();
        receipt.links.insert(1, duplicate);
        assert!(receipt.validate(&fixture.checkout, &id).is_err());
        let mut receipt = profile_receipt(&fixture);
        receipt.place_member("debug/debug");
        assert!(receipt.validate(&fixture.checkout, &id).is_err());
        let mut receipt = profile_receipt(&fixture);
        receipt.links.push(LinkPlacement {
            member: "../../escape".to_owned(),
            selected: None,
            prepared: None,
        });
        receipt
            .links
            .sort_by(|left, right| left.member.cmp(&right.member));
        assert!(receipt.validate(&fixture.checkout, &id).is_err());
    }

    #[test]
    fn a_prepared_profile_placement_keeps_the_others_committed() {
        let fixture = Fixture::new();
        let id = identity::digest(&fixture.checkout.to_string_lossy());
        let mut receipt = profile_receipt(&fixture);
        let link = link_path(&fixture.checkout, "release");
        receipt.generation = 1;
        receipt.state = ReceiptState::Prepared;
        receipt.place_member("release").prepared = Some(Prepared {
            generation: 2,
            to: fixture.local().selected(2),
            link: link.clone(),
        });
        receipt.validate(&fixture.checkout, &id).unwrap();
        assert_eq!(receipt.prepared_ref().map(|p| p.link.clone()), Some(link));
        // A prepared transition on a link no placement owns is unusable.
        let mut orphan = receipt.clone();
        orphan.place_member("release").prepared = Some(Prepared {
            generation: 2,
            to: fixture.local().selected(2),
            link: fixture.checkout.join("target/elsewhere"),
        });
        assert!(orphan.validate(&fixture.checkout, &id).is_err());
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
            NonceAuthority::Unkeyed,
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
        let second = transition(
            &fixture.context(),
            &receipt,
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
                NonceAuthority::Unkeyed,
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
                NonceAuthority::Unkeyed,
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
                NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
        )
        .unwrap_err();
        assert!(refused.contains("could not confirm"), "{refused}");
        assert!(!plain.local().root().exists());
        assert!(!os_symlink_exists(&plain.checkout.join("target")));
    }

    /// The automatic first conversion asks the same ignore prerequisite for
    /// every member link it publishes (oxide#160) and treats every
    /// non-`Eligible` answer as ineligibility (oxide#154): an expected refusal
    /// must leave the build usable, so it cannot be a transition error. Each
    /// class is the answer an explicit adoption refuses with, so the two
    /// presentations cannot drift.
    #[test]
    fn automatic_adoption_is_ineligible_for_every_ignore_safety_refusal() {
        // Rules that genuinely cover `target/debug` are eligible, including
        // the directory-only forms the partner link cannot use.
        for rule in ["/target", "target", "target/", "/target/", "**/target/"] {
            let fixture = Fixture::in_tree("", &format!("{rule}\n/.oxide-outputs/\n"));
            for member in ["debug", "release"] {
                assert_eq!(
                    first_link_git_answer(&fixture.context(), Some(member)).verdict,
                    LinkGitVerdict::Eligible,
                    "{rule} {member}: a covering rule is not eligible"
                );
            }
        }
        for (label, fixture) in [
            ("unignored", Fixture::in_tree("", "/.oxide-outputs/\n")),
            (
                "negated",
                Fixture::in_tree("", "target\n!/target\n/.oxide-outputs/\n"),
            ),
        ] {
            let answer = first_link_git_answer(&fixture.context(), Some("debug"));
            assert_ne!(
                answer.verdict,
                LinkGitVerdict::Eligible,
                "{label}: a refused conversion is eligible for automatic adoption"
            );
            assert!(answer.fix.is_some(), "{label}: no fix was derived");
        }
        // A tracked member link is never eligible.
        let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        create_tracked_member(&tracked.checkout);
        assert_eq!(
            first_link_git_answer(&tracked.context(), Some("debug")).verdict,
            LinkGitVerdict::Tracked
        );
        // A non-repository has no work tree to confirm: unknown, never
        // permission, and no fix is available because there is no root.
        let plain = Fixture::without_git();
        let answer = first_link_git_answer(&plain.context(), Some("debug"));
        assert!(
            matches!(answer.verdict, LinkGitVerdict::Unknown(_)),
            "{answer:?}"
        );
        assert!(answer.fix.is_none(), "{answer:?}");
        assert!(
            !plain.local().root().exists() && !os_symlink_exists(&plain.checkout.join("target")),
            "the eligibility answer had an effect"
        );
    }

    /// Track `target/debug` in the index while the working tree has neither
    /// the file nor the `target` directory.
    fn create_tracked_member(checkout: &Path) {
        fs::create_dir_all(checkout.join("target")).unwrap();
        fs::write(checkout.join("target").join("debug"), b"tracked").unwrap();
        let added = Command::new("git")
            .args(["add", "-f", "target/debug"])
            .current_dir(checkout)
            .status()
            .unwrap();
        assert!(added.success());
        fs::remove_file(checkout.join("target").join("debug")).unwrap();
        fs::remove_dir(checkout.join("target")).unwrap();
    }

    /// `oxide init` names the rule that covers the member links a first
    /// conversion publishes and never edits the file (oxide#154): an
    /// unignored path gets the exact non-slash rule, a nested workspace names
    /// its repository-relative spelling, a tracked member is named, and a
    /// covering rule (including a directory-only one, which genuinely covers
    /// `target/<member>`) is silent.
    #[test]
    fn the_init_diagnostic_names_the_exact_non_directory_rule() {
        let ready = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        assert_eq!(gitignore_diagnostic(&ready.checkout), None);
        // A directory-only parent rule covers the member link, so it is not
        // diagnosed (oxide#160's profile layout).
        let directory_only = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        assert_eq!(gitignore_diagnostic(&directory_only.checkout), None);

        let unignored = Fixture::in_tree("", "/.oxide-outputs/\n");
        let warning = gitignore_diagnostic(&unignored.checkout).expect("no diagnostic");
        assert!(warning.contains("`/target`"), "{warning}");
        assert!(
            warning.contains(&unignored.repo().join(".gitignore").display().to_string()),
            "{warning}"
        );
        assert!(
            warning.contains("leaves every .gitignore rule exactly as it is"),
            "{warning}"
        );
        assert_eq!(
            fs::read(unignored.repo().join(".gitignore")).unwrap(),
            b"/.oxide-outputs/\n",
            "the diagnostic edited the file"
        );

        let nested = Fixture::in_tree("apps/farmyard", "/.oxide-outputs/\n");
        let warning = gitignore_diagnostic(&nested.checkout).expect("no diagnostic");
        assert!(warning.contains("`/apps/farmyard/target`"), "{warning}");
        assert!(
            warning.contains("`apps/farmyard/target/debug`"),
            "{warning}"
        );

        let tracked = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
        create_tracked_member(&tracked.checkout);
        let warning = gitignore_diagnostic(&tracked.checkout).expect("no diagnostic");
        assert!(warning.contains("already tracks"), "{warning}");
        assert!(
            warning.contains("git rm --cached target/debug"),
            "{warning}"
        );

        // Not a repository at all: init adopts it outside Git and there is
        // nothing for a first conversion to ignore.
        let plain = Fixture::without_git();
        assert_eq!(gitignore_diagnostic(&plain.checkout), None);
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
            NonceAuthority::Unkeyed,
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
        // A prepared whole-target first conversion keeps its recorded layout
        // (oxide#160, S4b): replay never rewrites it into a profile receipt.
        assert_eq!(replayed.layout(), LayoutKind::WholeTarget);
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
        assert_eq!(replayed.layout(), LayoutKind::WholeTarget);
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
        let empty = crate::layout::Declaration::empty();
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &empty,
            None,
            false,
        )
        .unwrap();
        assert!(planned.is_none());
        assert_eq!(destination_facts(None), "");
        let receipt = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &empty,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let selected = planned.selected().unwrap();
        assert_eq!(selected.kind, RootKind::Local);
        assert_eq!(selected.root, fixture.local().root());
        let first = destination_facts(Some(&planned));
        assert!(first.starts_with("outputs-2 "), "{first}");
        assert!(first.ends_with("g1"), "{first}");
        // Relinking to the same root keeps the same destination identity, so
        // a warm slot survives a relink.
        let again = transition(
            &context,
            &receipt,
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        assert_eq!(again.generation, 2);
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &empty,
            None,
            false,
        )
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
        let first = transition(
            &context,
            &fixture.receipt(),
            &spec,
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        let receipt = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let foreign = fixture.root.join("foreign-absent");
        fs::remove_file(context.link()).unwrap();
        symlink(&foreign, context.link()).unwrap();
        let refused = transition(
            &context,
            &receipt,
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap_err();
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
        let committed = transition(
            &context,
            &receipt,
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        let refused = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap_err();
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
        let receipt = transition(
            &context,
            &fixture.receipt(),
            &spec,
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(planned.selected().unwrap().kind, RootKind::External);
        assert_eq!(planned.selected().unwrap().root, external);
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
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap();
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
            &crate::layout::Declaration::empty(),
            Some("container mismatch"),
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(planned.selected().unwrap().kind, RootKind::Local);
        assert_ne!(planned.selected().unwrap().root, external);
    }

    /// A live link to another owned root (an older selection restored by
    /// hand) disagrees with the committed receipt: binding refuses, even
    /// though that root's marker is valid, and nothing changes.
    #[test]
    fn a_link_restored_to_an_older_owned_root_is_refused() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let local = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        let plan = whole_target_plan(Some(expected.clone()));
        match bind_under_binding(&context, &store, 0, &receipt, Some(&plan), false) {
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
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
            let committed = transition(
                &context,
                &fixture.receipt(),
                &fixture.local(),
                Start::Any,
                NonceAuthority::Unkeyed,
            )
            .unwrap();
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
        let bind = |label: &str| match bind_for_command(
            &store,
            &fixture.checkout,
            0,
            None,
            None,
            false,
            None,
        ) {
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
            let keyed = planned_destination(
                Some(store.owner_control()),
                &fixture.checkout,
                &crate::layout::Declaration::empty(),
                None,
                false,
            )
            .unwrap();
            assert!(keyed.is_none());
            // Adoption completes while the command waits.
            transition(
                &context,
                &fixture.receipt(),
                &fixture.local(),
                Start::Any,
                NonceAuthority::Unkeyed,
            )
            .unwrap();
            match bind_for_command(
                &store,
                &fixture.checkout,
                0,
                keyed.as_ref(),
                None,
                false,
                None,
            ) {
                Ok(_) => panic!("{mode:?}: bound an owned root with an unconverted slot key"),
                Err(reason) => assert!(reason.contains("converted to owned outputs"), "{reason}"),
            }
            // Control: a command keyed after conversion binds that root.
            let rekeyed = planned_destination(
                Some(store.owner_control()),
                &fixture.checkout,
                &crate::layout::Declaration::empty(),
                None,
                false,
            )
            .unwrap();
            assert!(rekeyed.is_some());
            match bind_for_command(
                &store,
                &fixture.checkout,
                0,
                rekeyed.as_ref(),
                None,
                false,
                None,
            ) {
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
        transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let keyed = planned_destination(
            Some(store.owner_control()),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap();
        let binding = bind_for_command(
            &store,
            &fixture.checkout,
            0,
            keyed.as_ref(),
            None,
            false,
            None,
        )
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
        let committed = transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        *reserved.reserved_mut() = Some(reservation.clone());
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
        let created = transition(
            &context,
            &reserved,
            &fixture.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
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
        *held.reserved_mut() = Some(unused.clone());
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
        *dropped.reserved_mut() = Some(orphan);
        write_receipt(&fixture.control, &dropped).unwrap();
        *dropped.reserved_mut() = None;
        let refused = write_receipt(&fixture.control, &dropped).unwrap_err();
        assert!(refused.contains("cancellation journal"), "{refused}");
    }

    /// The future-root shape this seam can build without a registered volume:
    /// the checkout's own local root, held by a durable reservation on the same
    /// fixture receipt the existing reservation test uses. The only variable
    /// these tests add is the nonce AUTHORITY.
    fn keyed_fixture(fixture: &Fixture) -> (Receipt, RootSpec) {
        let spec = fixture.local();
        let future = spec.root().to_path_buf();
        let mut receipt = fixture.receipt();
        publish_reservation(fixture, &mut receipt, &future);
        (receipt, spec)
    }

    /// Every regular file's relative path, bytes and mode under `root`, plus
    /// every symlink's target: the authority oracle for a refused transition.
    /// Bounded by the fixture's own tiny control tree.
    fn authority_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>, u32)> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap() {
                let path = entry.unwrap().path();
                let facts = fs::symlink_metadata(&path).unwrap();
                if facts.is_dir() {
                    pending.push(path);
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                if facts.file_type().is_symlink() {
                    let target = fs::read_link(&path).unwrap();
                    found.push((relative, target.as_os_str().as_encoded_bytes().to_vec(), 0));
                } else {
                    let mode = facts.permissions().mode();
                    found.push((relative, fs::read(&path).unwrap(), mode));
                }
            }
        }
        found.sort();
        found
    }

    /// Publish the fixture's reservation for `root` in its receipt.
    fn publish_reservation(fixture: &Fixture, receipt: &mut Receipt, root: &Path) -> Reservation {
        let reservation = reservation_for(root, receipt.generation + 1);
        *receipt.reserved_mut() = Some(reservation.clone());
        write_receipt(&fixture.control, receipt).unwrap();
        crate::prune::validate_reservation(&fixture.control, &reservation).unwrap();
        reservation
    }

    /// A keyed future-root slot binds only the exact uncancelled reservation it
    /// was keyed with (oxide#152). The positive arm: the surviving reservation's
    /// nonce creates the root and the owned link, and the commit consumes it.
    /// No Cargo process exists below this seam, so the whole authority surface a
    /// lease would expose is the root, the receipt and the registry.
    #[test]
    fn a_keyed_future_root_binds_exactly_its_surviving_reservation() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let (reserved, spec) = keyed_fixture(&fixture);
        let future = spec.root().to_path_buf();
        let reservation = reserved.reserved().cloned().unwrap();
        let bound = transition(
            &context,
            &reserved,
            &spec,
            Start::Any,
            NonceAuthority::Keyed(Some(&reservation.nonce)),
        )
        .unwrap();
        assert_eq!(
            read_marker(&future).unwrap().nonce.as_deref(),
            Some(reservation.nonce.as_str()),
            "the created root did not carry the reserved nonce"
        );
        assert!(
            bound.reserved().is_none(),
            "the commit kept its consumed reservation"
        );
        assert_eq!(
            fs::read_link(context.link()).unwrap(),
            member_destination(&future, MEMBER),
            "the owned link does not select the reserved root"
        );
        assert!(
            crate::prune::validate_reservation(&fixture.control, &reservation).is_err(),
            "the consumed reservation is still valid"
        );
    }

    /// A keyed slot whose reservation was cancelled refuses with the
    /// cancelled-or-replaced reason inside the transition, before the prepared
    /// receipt, the root, the link or any registry write: the control tree and
    /// the absent root are byte-identical afterwards.
    #[test]
    fn a_cancelled_keyed_reservation_refuses_before_any_authority_effect() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let (reserved, spec) = keyed_fixture(&fixture);
        let future = spec.root().to_path_buf();
        let reservation = reserved.reserved().cloned().unwrap();
        let mut held = reserved.clone();
        let store = Store::for_tests(&fixture.root.join("pool"));
        cancel_reservation(&store, &fixture.control, &context, &mut held).unwrap();
        assert!(
            held.reserved().is_none(),
            "the cancellation kept the reservation"
        );
        let before = authority_tree(&fixture.control);
        let refused = transition(
            &context,
            &held,
            &spec,
            Start::Any,
            NonceAuthority::Keyed(Some(&reservation.nonce)),
        )
        .unwrap_err();
        assert!(refused.contains("cancelled or replaced"), "{refused}");
        assert!(
            !future.exists() && read_marker(&future).is_err(),
            "a refused keyed transition created the future root"
        );
        assert_eq!(
            authority_tree(&fixture.control),
            before,
            "a refused keyed transition wrote receipt or registry authority"
        );
        assert!(
            fs::symlink_metadata(context.link()).is_err(),
            "a refused keyed transition published the owned link"
        );
    }

    /// The two zero-effect negatives a keyed future-root slot must refuse: no
    /// reservation at all (the shape the old fresh-nonce fallback silently
    /// converted into a root plus a committed receipt), and a reservation
    /// replaced by another nonce after a cancellation. Both refuse with the
    /// cancelled-or-replaced reason and leave every authority byte unchanged;
    /// the surviving replacement still binds when the slot names it.
    #[test]
    fn an_absent_or_replaced_keyed_reservation_refuses_before_any_authority_effect() {
        // Absent: the receipt holds no reservation at all.
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        let future = spec.root().to_path_buf();
        let receipt = fixture.receipt();
        let unbacked = crate::prune::fresh_nonce().unwrap();
        let before = authority_tree(&fixture.control);
        let refused = transition(
            &context,
            &receipt,
            &spec,
            Start::Any,
            NonceAuthority::Keyed(Some(&unbacked)),
        )
        .unwrap_err();
        assert!(refused.contains("cancelled or replaced"), "{refused}");
        assert!(
            !future.exists() && read_marker(&future).is_err(),
            "an unbacked keyed nonce created the future root"
        );
        assert_eq!(
            authority_tree(&fixture.control),
            before,
            "an unbacked keyed nonce wrote receipt or registry authority"
        );
        assert!(
            fs::symlink_metadata(context.link()).is_err(),
            "an unbacked keyed nonce published the owned link"
        );

        // Replaced: the same future root, cancelled and reserved again under a
        // new nonce; the stale keyed nonce refuses with no further effect.
        let (mut reserved, spec) = keyed_fixture(&fixture);
        let first = reserved.reserved().cloned().unwrap();
        let store = Store::for_tests(&fixture.root.join("pool"));
        cancel_reservation(&store, &fixture.control, &context, &mut reserved).unwrap();
        let second = publish_reservation(&fixture, &mut reserved, &future);
        assert_ne!(
            first.nonce, second.nonce,
            "the replacement reused its nonce"
        );
        let before = authority_tree(&fixture.control);
        let refused = transition(
            &context,
            &reserved,
            &spec,
            Start::Any,
            NonceAuthority::Keyed(Some(&first.nonce)),
        )
        .unwrap_err();
        assert!(refused.contains("cancelled or replaced"), "{refused}");
        assert!(
            !future.exists() && read_marker(&future).is_err(),
            "a replaced keyed nonce created the future root"
        );
        assert_eq!(
            authority_tree(&fixture.control),
            before,
            "a replaced keyed nonce wrote receipt or registry authority"
        );
        // The surviving replacement still binds when the slot names it.
        let bound = transition(
            &context,
            &reserved,
            &spec,
            Start::Any,
            NonceAuthority::Keyed(Some(&second.nonce)),
        )
        .unwrap();
        assert_eq!(
            read_marker(&future).unwrap().nonce.as_deref(),
            Some(second.nonce.as_str())
        );
        assert!(bound.reserved().is_none());
    }

    /// A committed profile checkout whose owned local root and both member
    /// destinations exist, so every member link resolves (`OwnedLink`).
    fn committed_local_profile(fixture: &Fixture, context: &Context) -> Receipt {
        let mut receipt = profile_receipt(fixture);
        receipt.reserved = None;
        write_receipt(&fixture.control, &receipt).unwrap();
        let selected = receipt.selected().cloned().unwrap();
        let root = selected.root.clone();
        let marker = Marker {
            version: MARKER_VERSION.to_owned(),
            checkout: fixture.checkout.clone(),
            checkout_id: context.checkout_id.clone(),
            label: "checkout".to_owned(),
            recipe: RECIPE.to_owned(),
            kind: RootKind::Local,
            volume_uuid: None,
            nonce: selected.nonce.clone(),
            checkout_volume_uuid: None,
            generation: 1,
            created_ms: 1,
        };
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(MARKER),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(context.link()).unwrap();
        for member in ["debug", "release"] {
            fs::create_dir_all(member_destination(&root, member)).unwrap();
            symlink(
                member_destination(&root, member),
                context.link().join(member),
            )
            .unwrap();
        }
        receipt
    }

    fn declared(members: &[&str]) -> crate::layout::Declaration {
        crate::layout::Declaration::new(
            members
                .iter()
                .map(|name| crate::layout::Member::parse(name).unwrap()),
        )
        .unwrap()
    }

    /// oxide#152 publication seam: a requested member whose owned link dangles
    /// is a destination binding refuses, so the pre-lease reservation publishes
    /// NOTHING — every control byte, the receipt bytes and the registry revision
    /// stay identical, no root appears and no Cargo runs. The explicit
    /// `oxide outputs adopt` the refusal names then recovers with a FRESH nonce
    /// (superseding the old committed row through the ordinary rules) and the
    /// next ordinary build binds the recovered root.
    #[test]
    fn a_dangling_requested_member_reserves_nothing_and_adopt_recovers() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let store = pool_store(&fixture.root.join("pool"));
        let receipt = committed_local_profile(&fixture, &context);
        let old_nonce = receipt
            .selected()
            .and_then(Selected::nonce)
            .unwrap()
            .to_owned();
        let root = receipt.selected().unwrap().root().to_path_buf();
        let declaration = declared(&["debug", "release"]);
        let receipt_file = receipt_path(&fixture.control, &context.checkout_id);
        // The committed root disappears under its live member links: binding's
        // exact Dangling precondition.
        fs::remove_dir_all(&root).unwrap();
        for member in ["debug", "release"] {
            let state = link_state(
                &link_path(&context.workspace, member),
                &context.workspace,
                &context.checkout_id,
            )
            .unwrap();
            assert!(
                matches!(state, TargetState::Dangling(_)),
                "{member}: {state:?}"
            );
        }
        assert!(refuses_dangling_member(&context, &receipt, &declaration).unwrap());
        let before = authority_tree(&fixture.control);
        let receipt_bytes = fs::read(&receipt_file).unwrap();
        let revision = crate::prune::registry_revision(&fixture.control).unwrap();
        reserve(
            &store,
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
        )
        .unwrap();
        assert!(
            context.receipt().unwrap().unwrap().reserved().is_none(),
            "a dangling requested member published a reservation"
        );
        assert_eq!(
            authority_tree(&fixture.control),
            before,
            "the refused reservation wrote receipt or registry authority"
        );
        assert_eq!(fs::read(&receipt_file).unwrap(), receipt_bytes);
        assert_eq!(
            crate::prune::registry_revision(&fixture.control).unwrap(),
            revision
        );
        assert!(
            read_marker(&root).is_err() && !root.exists(),
            "the refused reservation created the root"
        );
        // Binding then refuses exactly as before, still with no authority effect.
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        match bind_profile(&context, &store, 0, &receipt, Some(&plan), false) {
            Bind::Refuse(reason) => assert!(reason.contains("not available now"), "{reason}"),
            _ => panic!("a dangling member bound"),
        }
        assert_eq!(
            authority_tree(&fixture.control),
            before,
            "binding wrote authority alongside its dangling refusal"
        );
        // The explicit adopt the refusal names recovers with a fresh nonce.
        let (committed, recovered, converted) = adopt_profile(&context, receipt.clone()).unwrap();
        assert!(converted, "the explicit adopt converted nothing");
        let fresh = committed
            .selected_for("debug")
            .and_then(Selected::nonce)
            .unwrap()
            .to_owned();
        assert_ne!(
            fresh, old_nonce,
            "the recovery reused the old committed nonce"
        );
        assert_eq!(recovered.root(), root.as_path());
        assert_eq!(
            read_marker(&root).unwrap().nonce.as_deref(),
            Some(fresh.as_str())
        );
        assert!(committed.reserved().is_none());
        let rows = crate::prune::test_rows(&fixture.control).unwrap();
        let root_rows: Vec<_> = rows
            .iter()
            .filter(|(row_root, ..)| row_root == &root)
            .collect();
        assert_eq!(root_rows.len(), 2, "{rows:?}");
        assert!(
            root_rows
                .iter()
                .all(|(_, selected, pending, _)| *selected && !*pending),
            "the recovery left a pending or unselected row: {rows:?}"
        );
        // The next ordinary build binds the recovered root with its fresh nonce.
        let plan = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &declaration,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        match bind_profile(&context, &store, 0, &committed, Some(&plan), false) {
            Bind::Bound(outcome) => assert_eq!(outcome.root, Some(root.clone())),
            Bind::Refuse(reason) => panic!("the recovered root did not bind: {reason}"),
            Bind::Wait => panic!("the recovered root waited"),
        }
    }

    /// The publication guard is scoped exactly like binding: only the REQUESTED
    /// member set is classified, and only the exact dangling state refuses. An
    /// unrequested dangling sibling cannot block another member's reservation, a
    /// resolving owned link (the local→external redock) and a new member's
    /// absent link keep theirs, and a whole-target layout is never guarded here.
    #[test]
    fn the_dangling_publication_guard_is_scoped_to_requested_members() {
        let fixture = Fixture::in_tree("", "/target/\n/.oxide-outputs/\n");
        let context = fixture.context();
        let receipt = committed_local_profile(&fixture, &context);
        let root = receipt.selected().unwrap().root().to_path_buf();
        assert!(!refuses_dangling_member(&context, &receipt, &declared(&["debug"])).unwrap());
        assert!(!refuses_dangling_member(&context, &receipt, &declared(&["release"])).unwrap());
        // Only release's destination disappears: a debug-only command still
        // reserves, and a command that requests release is refused.
        fs::remove_dir_all(member_destination(&root, "release")).unwrap();
        assert!(
            !refuses_dangling_member(&context, &receipt, &declared(&["debug"])).unwrap(),
            "an unrequested dangling sibling blocked the reservation"
        );
        assert!(refuses_dangling_member(&context, &receipt, &declared(&["release"])).unwrap());
        // A new member has no committed placement and its link is absent: it
        // joins the owned root and is never treated as dangling.
        let new_member = "aarch64-apple-darwin/debug";
        assert!(
            !refuses_dangling_member(&context, &receipt, &declared(&[new_member])).unwrap(),
            "a new member was treated as a dangling committed member"
        );
        // A whole-target layout keeps its reservation: `reserve` for the legacy
        // layout needs it so `reservation_matches` can accept the transition.
        let whole = fixture.receipt();
        assert_eq!(whole.layout(), LayoutKind::WholeTarget);
        assert!(!refuses_dangling_member(&context, &whole, &declared(&["debug"])).unwrap());
    }

    /// Only an unkeyed explicit adoption may draw a fresh nonce for a future
    /// root: `oxide outputs adopt` has no keyed slot, and the root it creates
    /// carries a fresh 128-bit nonce that no reservation names.
    #[test]
    fn an_unkeyed_adoption_draws_a_fresh_future_root_nonce() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let receipt =
            Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
        let (committed, _local, converted) = adopt_profile(&context, receipt).unwrap();
        assert!(converted, "the first adoption converted nothing");
        assert!(
            committed.reserved().is_none(),
            "an unkeyed adoption published a reservation"
        );
        assert_eq!(
            committed
                .selected_for("debug")
                .and_then(Selected::nonce)
                .map(str::len),
            Some(32),
            "the adoption did not draw a fresh 128-bit nonce"
        );
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
            crate::prune::registry_revision(&fixture.control).unwrap(),
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
            &|_| true,
            crate::prune::registry_revision(&fixture.control).unwrap(),
        )
        .is_err());
        // This writer's own unjournaled creation is rebound to a new intent.
        create_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &second,
            &|_| false,
            crate::prune::registry_revision(&fixture.control).unwrap(),
        )
        .unwrap();
        assert!(mark_pruned_at(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &first,
            "deleted",
            5
        )
        .is_err());
        assert!(!discard_pruning_receipt(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &first
        )
        .unwrap());
        mark_pruned_at(
            &fixture.control,
            &fixture.checkout,
            &checkout_id,
            &second,
            "deleted",
            5,
        )
        .unwrap();
        assert_eq!(
            context.receipt().unwrap().unwrap().state,
            ReceiptState::Pruned
        );
        // A committed receipt is never replaced by the creator.
        let other = Fixture::new();
        let other_context = other.context();
        transition(
            &other_context,
            &other.receipt(),
            &other.local(),
            Start::Any,
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let other_id = identity::digest(&other.checkout.to_string_lossy());
        assert!(create_pruning_receipt(
            &other.control,
            &other.checkout,
            &other_id,
            &first,
            &|_| false,
            crate::prune::registry_revision(&other.control).unwrap(),
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
            NonceAuthority::Unkeyed,
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
            NonceAuthority::Unkeyed,
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
            Start::MissingOnly,
            NonceAuthority::Unkeyed
        )
        .is_err());
        assert_eq!(fs::read(&link).unwrap(), b"kept");
        fs::remove_file(&link).unwrap();

        symlink(&dangling, &link).unwrap();
        assert!(transition(
            &context,
            &fixture.receipt(),
            &fixture.local(),
            Start::MissingOnly,
            NonceAuthority::Unkeyed
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
            NonceAuthority::Unkeyed,
        )
        .unwrap();
        let planned = planned_destination(
            Some(&fixture.control),
            &fixture.checkout,
            &crate::layout::Declaration::empty(),
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(planned.selected(), committed.selected());
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

    /// A one-use S1 fixture: a checkout and a pool whose control directory is
    /// the one its receipts live in, so `bind_for_command`'s own context finds
    /// them. The registry is ready, as every receipt write requires.
    struct GuardFixture {
        root: PathBuf,
        checkout_root: PathBuf,
        checkout: PathBuf,
        store: Store,
    }

    impl GuardFixture {
        /// A fixture whose classification is deterministic: the **checkout**
        /// lives under the literal `/private/tmp` (a §3.2 candidate root) and
        /// the **pool** lives under the process temporary directory, which is
        /// never inside `/private/tmp` in the AGENTS.md recipe (its `TMPDIR` is
        /// on the selected volume). Neither directory is a fixed path: each is
        /// created fresh and removed by `Drop`.
        ///
        /// The alternative — both under one scratch root — would make the
        /// pool's own payload sit inside the candidate root, which §3.3
        /// deliberately drops as unsafe-broad (the design's §12.1
        /// counterexample), so the fixture could never be guarded.
        fn new(label: &str) -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let name = format!("oxide-guard-{label}-{}-{id}", std::process::id());
            let pool_base = std::env::temp_dir()
                .canonicalize()
                .expect("the process temporary directory resolves");
            assert!(
                !pool_base.starts_with("/private/tmp"),
                "the fixture pool must live outside /private/tmp so the checkout's \
                 temporary root stays safe-broad; set TMPDIR to a directory on the \
                 selected volume (the AGENTS.md recipe does): {pool_base:?}"
            );
            let root = pool_base.join(&name);
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let temporary = std::env::var_os("OXIDE_TEST_INTERNAL_ROOT")
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| PathBuf::from("/private/tmp"));
            let checkout_root = temporary.canonicalize().unwrap().join(&name);
            fs::create_dir(&checkout_root).unwrap();
            fs::set_permissions(&checkout_root, fs::Permissions::from_mode(0o700)).unwrap();
            let checkout = checkout_root.join("checkout");
            fs::create_dir(&checkout).unwrap();
            let store = pool_store(&root);
            Self {
                root,
                checkout_root,
                checkout,
                store,
            }
        }

        fn control(&self) -> PathBuf {
            self.store.owner_control().to_path_buf()
        }

        fn context(&self) -> Context {
            Context::new(Some(self.control()), self.checkout.clone())
        }

        fn checkout_id(&self) -> String {
            identity::digest(&self.checkout.to_string_lossy())
        }

        fn write(&self, receipt: &Receipt) {
            write_receipt(&self.control(), receipt).unwrap();
        }

        /// A committed profile receipt whose members the live links would have
        /// to select. The guard refuses before it reads them.
        fn committed(&self) -> Receipt {
            let mut receipt =
                Receipt::first_with(&self.checkout, &self.checkout_id(), LayoutKind::Profile);
            let selected = local_spec(&self.checkout).selected(1);
            receipt.generation = 1;
            receipt.state = ReceiptState::Committed;
            receipt.links = vec![LinkPlacement {
                member: "debug".to_owned(),
                selected: Some(selected.clone()),
                prepared: None,
            }];
            receipt.roots = vec![selected];
            receipt
        }

        /// A `Prepared` receipt in the shipped write-ahead shape: exactly one
        /// placement carries a prepared transition, at the next generation.
        fn prepared(&self) -> Receipt {
            let mut receipt = self.committed();
            let to = local_spec(&self.checkout).selected(2);
            let link = link_path(&self.checkout, "debug");
            receipt.state = ReceiptState::Prepared;
            receipt.links[0].prepared = Some(Prepared {
                generation: receipt.generation + 1,
                to,
                link,
            });
            receipt
        }

        fn state_bytes(&self) -> Option<Vec<u8>> {
            fs::read(self.control().join("state.json")).ok()
        }
    }

    impl Drop for GuardFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_dir_all(&self.checkout_root);
        }
    }

    fn members(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// The guarded receipt/path table of design §4.2 in every shape, and the
    /// hierarchy rule of §4.3. Every refusal is checked for its text and for
    /// leaving the live paths untouched.
    #[test]
    fn the_guarded_local_output_table_holds() {
        let fixture = GuardFixture::new("table");
        let context = fixture.context();
        let debug = members(&["debug"]);

        // Absent receipt: absent target and a real (even nonempty) target both
        // run locally.
        assert!(local_outputs_decision(&context, &debug, "outputs").is_ok());
        fs::create_dir(fixture.checkout.join("target")).unwrap();
        fs::create_dir(fixture.checkout.join("target/debug")).unwrap();
        fs::write(fixture.checkout.join("target/debug/binary"), b"x").unwrap();
        assert!(local_outputs_decision(&context, &debug, "outputs").is_ok());

        // A link of any kind refuses and names the path.
        fs::remove_dir_all(fixture.checkout.join("target/debug")).unwrap();
        let elsewhere = fixture.root.join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, fixture.checkout.join("target/debug")).unwrap();
        let refused = local_outputs_decision(&context, &debug, "outputs").unwrap_err();
        assert!(refused.contains("target/debug"), "{refused}");
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(refused.contains("nothing was changed"), "{refused}");
        assert!(fixture.checkout.join("target/debug").is_symlink());
        fs::remove_file(fixture.checkout.join("target/debug")).unwrap();

        // The hierarchy: a linked triple refuses by its own name, so the
        // profile member below it is never resolved through the link.
        let triple = fixture.checkout.join("target/x86_64-unknown-linux-gnu");
        symlink(&elsewhere, &triple).unwrap();
        let refused = local_outputs_decision(
            &context,
            &members(&["x86_64-unknown-linux-gnu/debug"]),
            "outputs",
        )
        .unwrap_err();
        assert!(
            refused.contains("x86_64-unknown-linux-gnu"),
            "the triple itself is the refusing component: {refused}"
        );
        fs::remove_file(&triple).unwrap();

        // A detached receipt with real directories runs; the same receipt with
        // an inserted link refuses: detach is not a repair.
        let mut detached = Receipt::first_with(
            &fixture.checkout,
            &fixture.checkout_id(),
            LayoutKind::Profile,
        );
        detached.links = vec![LinkPlacement {
            member: "debug".to_owned(),
            selected: None,
            prepared: None,
        }];
        fixture.write(&detached);
        assert!(local_outputs_decision(&context, &debug, "outputs").is_ok());

        // A committed receipt refuses in every shape, naming detach.
        fixture.write(&fixture.committed());
        let refused = local_outputs_decision(&context, &debug, "outputs").unwrap_err();
        assert!(refused.contains("committed receipt"), "{refused}");
        assert!(refused.contains("oxide outputs detach"), "{refused}");
        assert!(!refused.contains("outputs adopt"), "{refused}");

        // A prepared receipt (any placement or a migration journal) refuses
        // with the exact §4.4 diagnostic.
        fixture.write(&fixture.prepared());
        assert_eq!(
            local_outputs_decision(&context, &debug, "outputs").unwrap_err(),
            crate::one_use::PREPARED_GUARDED
        );

        // A pruned receipt owns no selection, so the live paths decide.
        let mut pruned = detached.clone();
        pruned.state = ReceiptState::Pruned;
        fixture.write(&pruned);
        assert!(local_outputs_decision(&context, &debug, "outputs").is_ok());

        // A pruning receipt keeps the shipped refusal. Its terminal evidence
        // is what makes the state consistent.
        let mut pruning = detached.clone();
        pruning.state = ReceiptState::Pruning;
        pruning.pruned = Some(PruneRecord {
            intent: "prune".to_owned(),
            outcome: "pending".to_owned(),
            at_ms: 1,
        });
        fixture.write(&pruning);
        let refused = local_outputs_decision(&context, &debug, "outputs").unwrap_err();
        assert!(refused.contains("being pruned"), "{refused}");
    }

    /// The exact `Prepared` diagnostic is the same for every guarded mutation
    /// site's revalidation, and it is byte-identical on repetition: the state,
    /// the receipt, the links and `next_id` do not change.
    #[test]
    fn repeated_guarded_prepared_refusals_change_nothing() {
        let fixture = GuardFixture::new("prepared");
        fixture.write(&fixture.prepared());
        let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&receipt_path).unwrap();
        let state_before = fixture.state_bytes();
        let context = fixture.context();
        let mut digests = Vec::new();
        for _ in 0..3 {
            let refused =
                match guarded_barrier(&fixture.store, &context, &mut WaitAccounting::default()) {
                    Err(reason) => reason,
                    Ok(_) => panic!("a prepared receipt refuses the guarded barrier"),
                };
            assert_eq!(refused, crate::one_use::PREPARED_GUARDED);
            assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
            assert_eq!(fixture.state_bytes(), state_before);
            digests.push((
                fs::read(&receipt_path).unwrap(),
                fixture.state_bytes(),
                fixture
                    .store
                    .owner_control()
                    .join("waiters")
                    .read_dir()
                    .map(|entries| entries.count())
                    .unwrap_or(0),
            ));
        }
        assert!(digests.windows(2).all(|pair| pair[0] == pair[1]));
        let context_state = fixture.context();
        assert!(guarded_prepared_refusal(&context_state).is_err());
        assert!(guarded_prepared_refusal(&context_state).is_err());
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
    }

    /// A bounded durable digest of one fixture's authority state: the control
    /// state bytes, every receipt file, and the checkout's `target` tree with
    /// each entry's kind and (for a link) its exact text. Small trees only;
    /// never a payload walk.
    fn durable_digest(fixture: &GuardFixture) -> Vec<String> {
        let mut entries = Vec::new();
        let state = fixture.control().join("state.json");
        if let Ok(bytes) = fs::read(&state) {
            entries.push(format!("state:{}", file_digest(&state).unwrap()));
            assert!(!String::from_utf8_lossy(&bytes).contains("\"class\""));
        }
        let outputs = fixture.control().join("outputs");
        if let Ok(listing) = fs::read_dir(&outputs) {
            let mut names: Vec<String> = listing
                .filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned()))
                .collect();
            names.sort();
            for name in names {
                entries.push(format!(
                    "receipt {name}:{}",
                    file_digest(&outputs.join(&name)).unwrap()
                ));
            }
        }
        let mut pending = vec![fixture.checkout.join("target")];
        while let Some(path) = pending.pop() {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            let shown = path
                .strip_prefix(&fixture.checkout)
                .unwrap_or(&path)
                .display()
                .to_string();
            if metadata.file_type().is_symlink() {
                let text = fs::read_link(&path).unwrap();
                entries.push(format!("link {shown} -> {}", text.display()));
            } else if metadata.is_dir() {
                entries.push(format!("dir {shown}"));
                if let Ok(listing) = fs::read_dir(&path) {
                    for entry in listing.flatten() {
                        pending.push(entry.path());
                    }
                }
            } else {
                entries.push(format!("file {shown}"));
            }
        }
        entries.sort();
        entries
    }

    /// The pool state's `next_id`, read from its own document (the pre-S1
    /// durable fact the shipped verbs advance one clean mark at a time).
    fn next_id(fixture: &GuardFixture) -> Option<u64> {
        let bytes = fixture.state_bytes()?;
        let text = String::from_utf8(bytes).ok()?;
        text.split("\"next_id\":")
            .nth(1)?
            .split([',', '}'])
            .next()?
            .parse()
            .ok()
    }

    /// **Dormant byte identity at the actual command path, with the pre-S1
    /// behaviour as the reference** (the brief's requirement and gate
    /// `dormant_byte_identity`).
    ///
    /// The subject is the real guarded verb entry points (`detach_at`,
    /// `cancel_at`, `adopt_at`) and the leased command's binding, entered
    /// exactly as production enters them, while the one temporary switch is
    /// off — which is every production binary. The reference is the pre-S1
    /// answer: the literal shipped texts these verbs and the binding return
    /// today (they predate this slice; e.g. `bind_ephemeral`'s pending-transition
    /// refusal), plus the durable state those paths leave behind. The control
    /// is the same fixture with the switch on, which refuses with the guarded
    /// diagnostic instead — so the dormant answer is provably *not* the guarded
    /// one, and the comparison is not two invocations of the same helper.
    ///
    /// While dormant nothing is classified at all: the test also asserts that
    /// the classification counter does not move.
    #[test]
    fn the_dormant_command_path_answers_exactly_as_the_shipped_verbs_do() {
        assert!(!crate::one_use::is_active());
        let fixture = GuardFixture::new("dormant-command-path");
        let start_classifications = crate::one_use::classifications();

        // The verbs answer exactly as the release before this slice did, and
        // their durable effect is exactly that release's: each of the two verbs
        // that reach the shipped barrier allocates and abandons one clean mark
        // (so `next_id` advances once per verb), `adopt` refuses before the
        // barrier, no receipt is written, and the only other write is the
        // shipped recorded-accounting fence.
        let detach = detach_at(&fixture.context(), || {
            Ok(Some(fixture.store.handle_for_tests()))
        })
        .unwrap_err();
        let cancel = cancel_at(&fixture.context(), || {
            Ok(Some(fixture.store.handle_for_tests()))
        })
        .unwrap_err();
        let adopt = adopt_at(&fixture.context(), || Ok(None)).unwrap_err();
        assert_eq!(
            detach,
            "this checkout has no owned-output receipt; nothing to detach"
        );
        assert_eq!(
            cancel,
            "this checkout has no owned-output receipt; nothing to cancel"
        );
        assert!(
            adopt.starts_with("outputs adopt needs permanent drive mode (mode is unregistered)"),
            "{adopt}"
        );
        for answer in [&detach, &cancel, &adopt] {
            assert!(!answer.contains("one-use"), "{answer}");
            assert!(
                !answer.contains("prepared output transition unresolved"),
                "{answer}"
            );
        }
        assert_eq!(
            next_id(&fixture),
            Some(3),
            "one clean mark per shipped verb"
        );
        let state = fixture.state_bytes().expect("the shipped fence write");
        let text = String::from_utf8_lossy(&state);
        assert!(!text.contains("\"class\""), "{text}");
        assert!(
            text.contains("\"recorded_accounting\":\"oxide.accounting/1\""),
            "{text}"
        );
        assert!(text.contains("\"cleans\":[]"), "{text}");
        assert!(text.contains("\"records\":[]"), "{text}");
        assert!(
            !fixture
                .control()
                .join("outputs")
                .join(format!("{}.json", fixture.checkout_id()))
                .exists(),
            "no receipt was written"
        );
        // Nothing was classified while dormant, at the actual command path:
        // the switch is the only gate.
        assert_eq!(crate::one_use::classifications(), start_classifications);
        assert!(!crate::one_use::is_active());

        // Control (the other rollout, not the shipped path): with the switch on
        // the same entries answer as follows. For a checkout with **no**
        // receipt the guard has nothing of its own to refuse, so `detach` and
        // `cancel-reservation` answer with the very same shipped texts and take
        // the same barrier — the guard only changes owned-output states (the
        // prepared case below). The guarded `adopt` is the exception: it refuses
        // unconditionally, whatever the receipt state.
        let guarded_digest_before = durable_digest(&fixture);
        let guarded_next_before = next_id(&fixture);
        let guarded = crate::one_use::with_active(|| {
            assert!(
                guarded_workspace(Some(&fixture.control()), &fixture.checkout).is_some(),
                "the fixture's checkout is under a temporary root, so it is guarded"
            );
            (
                detach_at(&fixture.context(), || {
                    Ok(Some(fixture.store.handle_for_tests()))
                }),
                cancel_at(&fixture.context(), || {
                    Ok(Some(fixture.store.handle_for_tests()))
                }),
                adopt_at(&fixture.context(), || Ok(None)).unwrap_err(),
            )
        });
        assert_eq!(
            guarded.0,
            Err("this checkout has no owned-output receipt; nothing to detach".to_owned()),
            "the guard changes no state it has nothing to refuse"
        );
        assert_eq!(
            guarded.1,
            Err("this checkout has no owned-output receipt; nothing to cancel".to_owned())
        );
        assert!(
            guarded.2.contains("one-use (temporary) workspace"),
            "{}",
            guarded.2
        );
        // Two more shipped barriers ran, so exactly two more ids were taken;
        // the guard differs only where it has authority, and no receipt or link
        // appeared either way.
        assert_eq!(next_id(&fixture), guarded_next_before.map(|id| id + 2));
        let guarded_digest = durable_digest(&fixture);
        assert_eq!(guarded_digest.len(), guarded_digest_before.len());
        assert!(guarded_digest
            .iter()
            .all(|entry| entry.starts_with("state:") || entry.starts_with("receipt ")));
        // The leased command's binding: a `Prepared` receipt in a checkout
        // with no registered drive is refused by the shipped code with its own
        // text, which is what the dormant command must produce.
        fixture.write(&fixture.prepared());
        let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&receipt_path).unwrap();
        let prepared_digest = durable_digest(&fixture);
        let refused = match bind_for_command(
            &fixture.store,
            &fixture.checkout,
            0,
            None,
            None,
            false,
            None,
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("the shipped binding refuses a prepared transition"),
        };
        assert_eq!(
            refused,
            "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building"
        );
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(durable_digest(&fixture), prepared_digest);

        assert!(!crate::one_use::is_active());

        // Control (the other rollout, not the shipped path): the same fixture
        // with the switch on is guarded, so the very same entries answer with
        // the guarded texts and still write nothing.
        let active = crate::one_use::with_active(|| {
            assert!(
                guarded_workspace(Some(&fixture.control()), &fixture.checkout).is_some(),
                "the fixture's checkout is under a temporary root, so it is guarded"
            );
            let refused = detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err();
            let refused_adopt = adopt_at(&fixture.context(), || Ok(None)).unwrap_err();
            let refused_bind = match bind_for_command(
                &fixture.store,
                &fixture.checkout,
                0,
                None,
                None,
                false,
                Some(&LocalOutputs {
                    members: &[],
                    reason: "one-use",
                }),
            ) {
                Err(reason) => reason,
                Ok(_) => panic!("the guarded binding refuses a prepared transition"),
            };
            (refused, refused_adopt, refused_bind)
        });
        assert_eq!(active.0, crate::one_use::PREPARED_GUARDED);
        // The fixture still holds the prepared receipt, so the guarded `adopt`
        // refuses with the same exact §4.4 diagnostic (precedence: a prepared
        // transition outranks the general adopt refusal).
        assert_eq!(active.1, crate::one_use::PREPARED_GUARDED);
        assert_eq!(active.2, crate::one_use::PREPARED_GUARDED);
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(durable_digest(&fixture), prepared_digest);
        // The two rollouts really differ: the dormant answer is the shipped
        // one and the guarded answer is the guard's, on the same bytes.
        assert_ne!(refused, active.0);
        assert!(!crate::one_use::is_active());
    }

    /// A guarded refusal must not create the pool's admission lock: every lock
    /// a guarded verb touches before it decides is the checkout binding mutex
    /// (design §4.1 step 4). A pool that has no admission lock is a bounded
    /// retry, never a creation; with an existing lock the dead and unknown
    /// refusals leave the control directory and its files exactly as they were.
    #[test]
    fn a_guarded_refusal_creates_no_admission_lock() {
        // No admission lock at all: the verb retries and creates nothing.
        let fixture = GuardFixture::new("no-admission-lock");
        fixture.write(&fixture.committed());
        let admission = fixture.control().join("admission.lock");
        let _ = fs::remove_file(&admission);
        crate::store::dead_record_for_tests(&fixture.store, &fixture.checkout, 7);
        let refused = crate::one_use::with_active(|| {
            detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert_eq!(refused, crate::one_use::BARRIER_BUSY);
        assert!(!admission.exists(), "a refusal created the admission lock");

        // With the pool's existing admission lock, a dead-custody refusal
        // leaves every file (that lock included) byte-identical.
        let fixture = GuardFixture::new("existing-admission-lock");
        fixture.write(&fixture.committed());
        let admission = fixture.control().join("admission.lock");
        assert!(admission.is_file(), "a converted pool carries the lock");
        let admission_before = fs::read(&admission).unwrap();
        let listing = |dir: &Path| {
            let mut names: Vec<String> = fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .filter_map(|entry| Some(entry.file_name().to_str()?.to_owned()))
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        };
        // The plant itself writes state, so the comparison starts after it.
        crate::store::dead_record_for_tests(&fixture.store, &fixture.checkout, 7);
        let control_before = listing(&fixture.control());
        let refused = crate::one_use::with_active(|| {
            cancel_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("record 7"), "{refused}");
        assert_eq!(fs::read(&admission).unwrap(), admission_before);
        let mut control_after = listing(&fixture.control());
        control_after.sort();
        let mut expected = control_before.clone();
        expected.sort();
        assert_eq!(control_after, expected, "no file was created");

        // Non-vacuity: a clear checkout takes the barrier and completes the
        // verb through the existing lock.
        let fixture = GuardFixture::new("existing-admission-lock-clear");
        fixture.write(&fixture.committed());
        let _ = crate::one_use::with_active(|| {
            detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
        });
        assert!(fixture.control().join("admission.lock").is_file());
    }

    /// The shipped (dormant) `adopt` keeps its **error precedence**: drive mode
    /// first, then the control directory, then the pool. A dormant build must
    /// report the pre-S1 refusal even when the pool provider also fails, while
    /// the guarded build refuses before any of them.
    #[test]
    fn a_dormant_adopt_keeps_the_shipped_error_precedence() {
        let fixture = GuardFixture::new("dormant-adopt-precedence");
        let failing = || -> Result<Option<Store>, String> {
            Err("the removable registration is unreadable".to_owned())
        };
        assert!(!crate::one_use::is_active());
        let refused = adopt_at(&fixture.context(), failing).unwrap_err();
        assert!(
            refused.starts_with("outputs adopt needs permanent drive mode (mode is unregistered)"),
            "the shipped drive-mode refusal wins: {refused}"
        );
        // With the switch on the guarded refusal comes first, whatever the
        // registry or pool says.
        let refused = crate::one_use::with_active(|| {
            adopt_at(&fixture.context(), || -> Result<Option<Store>, String> {
                Err("the removable registration is unreadable".to_owned())
            })
            .unwrap_err()
        });
        assert!(
            refused.contains("one-use (temporary) workspace"),
            "{refused}"
        );
    }

    /// The guarded verbs decide **before** the pool is opened or coordinated:
    /// a provider that fails (or refuses to run at all) cannot shadow the
    /// guard's refusal, so an unreadable registration can never replace the
    /// exact `Prepared` diagnostic or the guarded `adopt` refusal.
    #[test]
    fn the_guarded_verbs_refuse_before_the_pool_is_opened() {
        let fixture = GuardFixture::new("precheck-before-pool");
        fixture.write(&fixture.prepared());
        let before = durable_digest(&fixture);
        let never =
            || -> Result<Option<Store>, String> { panic!("a guarded verb must not open the pool") };
        let failing = || -> Result<Option<Store>, String> {
            Err("the removable registration is unreadable".to_owned())
        };
        crate::one_use::with_active(|| {
            assert_eq!(
                adopt_at(&fixture.context(), never).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
            assert_eq!(
                adopt_at(&fixture.context(), failing).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
            assert_eq!(
                detach_at(&fixture.context(), never).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
            assert_eq!(
                detach_at(&fixture.context(), failing).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
            assert_eq!(
                cancel_at(&fixture.context(), never).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
            assert_eq!(
                cancel_at(&fixture.context(), failing).unwrap_err(),
                crate::one_use::PREPARED_GUARDED
            );
        });
        assert_eq!(durable_digest(&fixture), before);
        // The general guarded `adopt` refusal wins over a pool error too.
        let fixture = GuardFixture::new("precheck-before-pool-general");
        crate::one_use::with_active(|| {
            assert!(adopt_at(&fixture.context(), failing)
                .unwrap_err()
                .contains("one-use (temporary) workspace"));
        });
    }

    /// Verb-level: a guarded `detach` and `cancel-reservation` refuse a
    /// `Prepared` transition with the exact diagnostic, repeatedly and
    /// byte-identically including `next_id`, and the refusal happens at the
    /// effect-free fast precheck — before the binding mutex is even created
    /// (design §4.1 part 1, §4.4, §4.5).
    #[test]
    fn the_guarded_verbs_refuse_a_prepared_transition_before_any_lock() {
        let fixture = GuardFixture::new("verbs-prepared");
        fixture.write(&fixture.prepared());
        let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&receipt_path).unwrap();
        let state_before = fixture.state_bytes();
        let classify_before = crate::one_use::classifications();
        let mut traces = Vec::new();
        crate::one_use::with_active(|| {
            for _ in 0..3 {
                let refused = detach_at(&fixture.context(), || {
                    Ok(Some(fixture.store.handle_for_tests()))
                })
                .unwrap_err();
                assert_eq!(refused, crate::one_use::PREPARED_GUARDED);
                let refused = cancel_at(&fixture.context(), || {
                    Ok(Some(fixture.store.handle_for_tests()))
                })
                .unwrap_err();
                assert_eq!(refused, crate::one_use::PREPARED_GUARDED);
                traces.push((
                    fs::read(&receipt_path).unwrap(),
                    fixture.state_bytes(),
                    fs::read_dir(fixture.control().join("waiters"))
                        .map(|entries| entries.count())
                        .unwrap_or(0),
                ));
            }
        });
        assert!(traces.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(fixture.state_bytes(), state_before);
        // The fast precheck runs before this checkout's binding mutex is even
        // created (the directory itself is made by `Store::for_tests`).
        let binding_lock = fixture.control().join("bindings").join(format!(
            "{}.lock",
            identity::digest(&fixture.checkout.to_string_lossy())
        ));
        assert!(
            !binding_lock.exists(),
            "the refusal precedes the checkout binding mutex"
        );
        assert!(
            crate::one_use::classifications() > classify_before,
            "the guarded verb did classify its workspace"
        );
    }

    /// Verb-level: the guarded `detach` covers the busy classifier's rows — a
    /// dead record, a dead clean mark, a live holder (the bounded busy
    /// refusal) and unknown custody — and every refusal leaves the receipt,
    /// the state (`next_id` included) and the `waiters/` listing unchanged.
    #[test]
    fn the_guarded_verbs_cover_the_busy_classifier() {
        // A dead record.
        let fixture = GuardFixture::new("verbs-dead-record");
        fixture.write(&fixture.committed());
        crate::store::dead_record_for_tests(&fixture.store, &fixture.checkout, 7);
        let stored_receipt = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&stored_receipt).unwrap();
        let state_before = fixture.state_bytes();
        let refused = crate::one_use::with_active(|| {
            detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("record 7"), "{refused}");
        assert_eq!(fs::read(&stored_receipt).unwrap(), receipt_bytes);
        assert_eq!(fixture.state_bytes(), state_before);

        // A dead clean mark.
        let fixture = GuardFixture::new("verbs-dead-mark");
        fixture.write(&fixture.committed());
        crate::store::dead_clean_mark_for_tests(&fixture.store, &fixture.checkout, 9);
        let stored_receipt = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&stored_receipt).unwrap();
        let state_before = fixture.state_bytes();
        let refused = crate::one_use::with_active(|| {
            cancel_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert!(refused.contains("clean mark 9"), "{refused}");
        assert_eq!(fs::read(&stored_receipt).unwrap(), receipt_bytes);
        assert_eq!(fixture.state_bytes(), state_before);

        // A live holder: retried on the shipped poll, then the bounded busy
        // refusal, with nothing written.
        let fixture = GuardFixture::new("verbs-live-holder");
        fixture.write(&fixture.committed());
        let _held = crate::store::hold_record_for_tests(&fixture.store, &fixture.checkout, 5);
        let state_before = fixture.state_bytes();
        let refused = crate::one_use::with_active(|| {
            detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert_eq!(refused, crate::one_use::BARRIER_BUSY);
        assert_eq!(fixture.state_bytes(), state_before);
        assert_eq!(
            fs::read_dir(fixture.control().join("waiters"))
                .map(|entries| entries.count())
                .unwrap_or(0),
            0,
            "no waiter lock was created"
        );

        // Unknown custody: an unreadable state document is protected.
        let fixture = GuardFixture::new("verbs-unknown");
        fixture.write(&fixture.committed());
        crate::store::save_default_state_for_tests(&fixture.store);
        fs::write(fixture.control().join("state.json"), b"{not json").unwrap();
        let stored_receipt = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&stored_receipt).unwrap();
        let refused = crate::one_use::with_active(|| {
            detach_at(&fixture.context(), || {
                Ok(Some(fixture.store.handle_for_tests()))
            })
            .unwrap_err()
        });
        assert!(refused.contains("clean barrier refused"), "{refused}");
        assert_eq!(fs::read(&stored_receipt).unwrap(), receipt_bytes);
        assert_eq!(
            fs::read(fixture.control().join("state.json")).unwrap(),
            b"{not json"
        );
    }

    /// The guarded branch keeps the shipped unsettled-prune-intent refusal, at
    /// the fast precheck and under the binding mutex (design §4.2).
    #[test]
    fn an_unsettled_prune_intent_refuses_a_guarded_command() {
        let fixture = GuardFixture::new("unsettled");
        let id = crate::prune::seed_test_intent(&fixture.control(), &fixture.checkout_id(), false);
        let members = members(&["debug"]);
        let before = durable_digest(&fixture);
        let refused =
            guarded_precheck(Some(&fixture.control()), &fixture.checkout, &members).unwrap_err();
        assert!(refused.contains(&id), "{refused}");
        assert!(refused.contains("unsettled"), "{refused}");
        let local = LocalOutputs {
            members: &members,
            reason: "one-use",
        };
        let refused = match bind_for_command(
            &fixture.store,
            &fixture.checkout,
            0,
            None,
            None,
            false,
            Some(&local),
        ) {
            Err(reason) => reason,
            Ok(_) => panic!("an unsettled prune intent refuses the guarded binding"),
        };
        assert!(refused.contains(&id), "{refused}");
        assert_eq!(durable_digest(&fixture), before);
    }

    /// The guarded prune-intent check follows the **shipped** definition, and
    /// every failure of the registry read is a refusal:
    ///
    /// - no registry document at all -> clear (a pool that has never published
    ///   output authority cannot hold a published intent; this is exactly what
    ///   `prune::unsettled_for` answers for the shipped verbs);
    /// - a malformed or unreadable registry -> refusal, never "unset, so
    ///   clear";
    /// - a present intent whose phase is terminal **and** whose checkout
    ///   receipt committed (`settled()`) -> clear, because that is the shipped
    ///   definition of settled;
    /// - an unsettled intent -> the shipped refusal naming its ID.
    #[test]
    fn the_guarded_prune_intent_check_follows_the_shipped_definition() {
        let members = members(&["debug"]);

        // No registry document: clear, and nothing is written.
        let fixture = GuardFixture::new("intent-absent");
        let before = durable_digest(&fixture);
        crate::one_use::with_active(|| {
            assert!(guarded_precheck(Some(&fixture.control()), &fixture.checkout, &members).is_ok())
        });
        assert_eq!(durable_digest(&fixture), before);

        // Malformed registry: a refusal, not a silent clear.
        let fixture = GuardFixture::new("intent-malformed");
        let registry = fixture.control().join("outputs").join("registry.json");
        fs::write(&registry, b"{not a registry").unwrap();
        let refused = crate::one_use::with_active(|| {
            match guarded_precheck(Some(&fixture.control()), &fixture.checkout, &members) {
                Err(reason) => reason,
                Ok(()) => panic!("a malformed registry must refuse"),
            }
        });
        assert!(!refused.is_empty());
        assert_eq!(fs::read(&registry).unwrap(), b"{not a registry");

        // A settled intent is clear; an unsettled one refuses.
        let fixture = GuardFixture::new("intent-settled");
        let id = crate::prune::seed_test_intent(&fixture.control(), &fixture.checkout_id(), true);
        crate::one_use::with_active(|| {
            assert!(
                guarded_precheck(Some(&fixture.control()), &fixture.checkout, &members).is_ok(),
                "a settled intent is clear"
            )
        });
        assert!(!id.is_empty());
        let fixture = GuardFixture::new("intent-unsettled");
        let id = crate::prune::seed_test_intent(&fixture.control(), &fixture.checkout_id(), false);
        let refused = crate::one_use::with_active(|| {
            match guarded_precheck(Some(&fixture.control()), &fixture.checkout, &members) {
                Err(reason) => reason,
                Ok(()) => panic!("an unsettled intent must refuse"),
            }
        });
        assert!(refused.contains(&id), "{refused}");
        assert!(refused.contains("unsettled"), "{refused}");
    }

    /// A `Pruned` receipt selects nothing, so a guarded binding runs locally
    /// through it (design §4.2's `Pruned` row), writing no receipt or state
    /// byte.
    #[test]
    fn a_pruned_receipt_still_binds_locally() {
        let fixture = GuardFixture::new("pruned-binding");
        let mut receipt = Receipt::first_with(
            &fixture.checkout,
            &fixture.checkout_id(),
            LayoutKind::Profile,
        );
        receipt.links = vec![LinkPlacement {
            member: "debug".to_owned(),
            selected: None,
            prepared: None,
        }];
        receipt.state = ReceiptState::Pruned;
        receipt.pruned = Some(PruneRecord {
            intent: "prune".to_owned(),
            outcome: "deleted".to_owned(),
            at_ms: 1,
        });
        fixture.write(&receipt);
        let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&receipt_path).unwrap();
        let before = durable_digest(&fixture);
        let members = members(&["debug"]);
        let local = LocalOutputs {
            members: &members,
            reason: "one-use",
        };
        let binding = bind_for_command(
            &fixture.store,
            &fixture.checkout,
            0,
            None,
            None,
            false,
            Some(&local),
        )
        .unwrap();
        assert_eq!(
            binding.outcome.warnings,
            vec!["one-use workspace: local outputs (one-use)".to_owned()]
        );
        assert!(binding.outcome.root.is_none());
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(durable_digest(&fixture), before);
    }

    /// A malformed receipt never becomes a guarded `adopt` reply: the verb
    /// refuses unconditionally, so the general guarded refusal is the answer and
    /// only a `Prepared` transition selects the exact diagnostic (design §4.1
    /// precedence, §4.5).
    #[test]
    fn a_malformed_receipt_keeps_the_general_guarded_adopt_refusal() {
        let fixture = GuardFixture::new("malformed-adopt");
        let path = receipt_path(&fixture.control(), &fixture.checkout_id());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{not a receipt").unwrap();
        let context = fixture.context();
        assert!(
            context.receipt().is_err(),
            "the malformed document is refused by the reader"
        );
        let refused = guarded_adopt_refusal(&context);
        assert_eq!(refused, crate::one_use::adopt_refusal(&fixture.checkout));
        assert!(
            refused.contains("one-use (temporary) workspace"),
            "{refused}"
        );
        assert!(refused.contains("cannot own an output root"), "{refused}");
        // A prepared receipt still selects the exact §4.4 diagnostic.
        fixture.write(&fixture.prepared());
        assert_eq!(
            guarded_adopt_refusal(&fixture.context()),
            crate::one_use::PREPARED_GUARDED
        );
    }

    /// The bounded barrier budget: with a live holder the guarded verb retries
    /// on the shipped poll and then refuses finitely, having written nothing
    /// (no mark, no id, no `waiters/` file, `next_id` unchanged).
    #[test]
    fn the_guarded_barrier_gives_up_after_its_attempt_budget() {
        let fixture = GuardFixture::new("barrier-budget");
        let _held = crate::store::hold_record_for_tests(&fixture.store, &fixture.checkout, 5);
        fixture.write(&fixture.committed());
        let receipt_path = receipt_path(&fixture.control(), &fixture.checkout_id());
        let receipt_bytes = fs::read(&receipt_path).unwrap();
        let state_before = fixture.state_bytes();
        let context = fixture.context();
        let refused =
            match guarded_barrier(&fixture.store, &context, &mut WaitAccounting::default()) {
                Err(reason) => reason,
                Ok(_) => panic!("a live holder exhausts the attempt budget"),
            };
        assert_eq!(refused, crate::one_use::BARRIER_BUSY);
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(fixture.state_bytes(), state_before);
        let waiters = fixture.control().join("waiters");
        let listed = fs::read_dir(&waiters)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(listed, 0, "no waiter lock was created");
    }

    /// The guarded branch of the shipped verbs that need no live link: a
    /// prepared receipt refuses at the effect-free `adopt` precheck and in
    /// `clean`, and a guarded clean never prints adopt advice.
    #[test]
    fn prepared_refusals_are_the_exact_diagnostic_without_advice() {
        let fixture = GuardFixture::new("advice");
        fixture.write(&fixture.prepared());
        let context = fixture.context();
        assert_eq!(
            guarded_prepared_refusal(&context).unwrap_err(),
            crate::one_use::PREPARED_GUARDED
        );
        assert!(!crate::one_use::PREPARED_GUARDED.contains("adopt"));
        assert!(!crate::one_use::PREPARED_GUARDED.contains("detach"));
        // The general adopt text is a different, bounded line that offers no
        // repair for a prepared transition.
        let general = crate::one_use::adopt_refusal(&fixture.checkout);
        assert!(
            general.contains("one-use (temporary) workspace"),
            "{general}"
        );
        assert!(general.contains("nothing was changed"), "{general}");
        // The committed refusal names the one safe verb.
        let committed = crate::one_use::committed_refusal(&fixture.root);
        assert!(committed.contains("oxide outputs detach"), "{committed}");
    }

    /// The plan and status wording is bounded and never recommends adopt.
    #[test]
    fn the_guarded_plan_and_status_lines_are_bounded() {
        for line in [crate::one_use::PLAN_LOCAL, crate::one_use::STATUS_LOCAL] {
            assert!(line.contains("one-use (temporary) workspace"), "{line}");
            assert!(line.contains("local"), "{line}");
            assert!(!line.contains("adopt"), "{line}");
            assert!(line.len() < 200, "{line}");
        }
    }
}
