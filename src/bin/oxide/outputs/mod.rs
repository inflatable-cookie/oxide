//! Final outputs on a permanent drive: one owned output root and its profile
//! member links.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Final outputs on
//! the permanent drive). Ephemeral drive mode keeps today's real
//! `<checkout>/target`. Permanent mode keeps `<checkout>/target` a real
//! directory and replaces each positively identified profile member (`debug`,
//! `release`, `<triple>/debug`, ...) with an owned symlink into one real output
//! root outside the pool payload, either
//! `<mount>/oxide-outputs/<label>-<checkout id>/` on the registered volume or
//! the checkout-local fallback `<checkout>/.oxide-outputs/local/` while the
//! volume is absent. `CARGO_TARGET_DIR` stays the lexical
//! `<checkout>/target`, so `target/debug` and the profiles of #106 keep
//! working. The older whole-target layout made `target` itself a link; a
//! checkout switches from it via `oxide outputs detach` followed by `oxide
//! outputs adopt`.
//!
//! Nothing is moved, copied or deleted by a transition. Ownership is a marker
//! inside the root plus a committed receipt in the internal pool's control
//! directory; a link whose target does not carry this checkout's marker is
//! foreign and is never followed, adopted, swapped or cleaned through.
//! Transitions are written ahead: a `prepared` receipt, then the destination
//! root and marker, then a unique sibling link, then an atomic rename onto the
//! member's own path, then the committed receipt. A crash replays only the
//! exact generations the receipt and the live links name; any contradiction
//! stops with both roots retained.
//!
//! The owning lock is the internal pool's existing admission lock, together
//! with the checkout's existing clean/admission barrier: `oxide clean` and
//! `oxide outputs adopt|detach` use [`Store::begin_clean`], and a building
//! command holds its checkout's lease, which that barrier waits for. Receipt
//! writes happen under the admission lock, so one transition is published at
//! a time.

mod adopt;
mod bind;
mod clean;
mod exec;
mod git;
mod plan;
mod receipt;
mod root;
mod transition;

#[cfg(test)]
mod tests;

pub use adopt::auto_adopt;
pub use bind::{bind_for_command, reserve, OutputBinding};
pub use clean::{clean_placement, CleanPlacement};
pub use exec::{
    destination_facts, lease_declaration, needs_reservation, planned_destination, program_target,
    target_entries, tool_directories_note, tool_notice_applies, Plan,
};
pub use git::gitignore_diagnostic;
pub use plan::status_lines_for;
pub(crate) use receipt::{
    marker_digest, read_marker_for_prune, read_receipt_for_prune, receipt_path_public, LayoutKind,
    LinkPlacement, Receipt, Reservation,
};
pub use receipt::{RootKind, Selected};

#[allow(unused_imports)]
pub use bind::Outcome;
#[allow(unused_imports)]
pub(crate) use exec::PlannedMember;
#[allow(unused_imports)]
pub use exec::{exec_declaration, refuse_unknown_layout};
#[allow(unused_imports)]
pub(crate) use receipt::{member_destination, Marker, Prepared};

use crate::identity;
use crate::removable::{self, DriveMode};
use crate::store::{self, CheckoutCustody, CleanGuard, Cleaning, Store, WaitAccounting};
use crate::CARGO;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Directory under the volume mount that holds every checkout's output root.
pub const EXTERNAL_DIR: &str = "oxide-outputs";
/// Directory inside the checkout that holds the local fallback root.
pub const LOCAL_DIR: &str = ".oxide-outputs";
/// The local fallback root's name inside [`LOCAL_DIR`].
pub(in crate::outputs) const LOCAL_ROOT: &str = "local";
/// Ownership marker inside every output root.
pub const MARKER: &str = ".oxide-output.json";
/// The gitignore entry `oxide init` adds for the local fallback root.
pub const GITIGNORE_LINE: &str = "/.oxide-outputs/";

/// The placement recipe a marker binds: `target/` with `target/debug`.
pub(in crate::outputs) const RECIPE: &str = "target-debug-v1";
/// The receipt's one placement member (oxide#129): the whole checkout target.
pub(crate) const RECIPE_PUBLIC: &str = RECIPE;
pub(crate) const MEMBER: &str = "target";

const USAGE: &str = "\
usage: oxide outputs plan      effect-free JSON plan for this checkout (oxide.outputs.plan/2)
       oxide outputs adopt     explicitly convert this checkout's profile members
       oxide outputs detach    restore a real <checkout>/target directory; roots are kept
       oxide outputs status    read-only state of this checkout's owned outputs
       oxide outputs prune     explicitly prune roots whose checkout is gone

Permanent drive mode keeps `<checkout>/target` a real directory and owns one
symlink per profile member (`debug`, `release`, `<triple>/debug`, ...). Those
members select their own subpaths of one real root:
<mount>/oxide-outputs/<project>-<checkout id>/ while the registered volume is
usable, else the checkout-local fallback <checkout>/.oxide-outputs/local/.
`plan` and `adopt` never move, copy or delete an existing target directory or
an arbitrary symlink: a nonempty real target, a nonempty real member and a
foreign symlink refuse without any change. Only an explicit `adopt` converts
an empty real member. A missing target converts by `adopt` or on the first
leased build of a fresh checkout (no receipt of any state) on the registered
permanent drive, with one notice. `detach` works in either mode: it restores
this checkout's real target and keeps every root, and automatic conversion
never re-links it.";

/// Why an owned-output operation stopped. The kind is decided where the
/// condition is found, so no caller ever matches on message text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutputsError {
    /// A precondition of the command is not met; nothing was changed.
    Refused(String),
    /// A file-system or coordination operation failed.
    Io(String),
    /// A durable record is malformed or contradicts the live tree.
    Corrupt(String),
}

impl OutputsError {
    pub(in crate::outputs) fn refused(reason: impl Into<String>) -> Self {
        Self::Refused(reason.into())
    }

    pub(in crate::outputs) fn io(reason: impl Into<String>) -> Self {
        Self::Io(reason.into())
    }

    pub(in crate::outputs) fn corrupt(reason: impl Into<String>) -> Self {
        Self::Corrupt(reason.into())
    }

    fn message(&self) -> &str {
        match self {
            Self::Refused(reason) | Self::Io(reason) | Self::Corrupt(reason) => reason,
        }
    }
}

impl std::fmt::Display for OutputsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl From<OutputsError> for String {
    fn from(error: OutputsError) -> Self {
        error.message().to_owned()
    }
}

/// What `<checkout>/target` is right now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::outputs) enum TargetState {
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
    pub(in crate::outputs) fn label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::RealEmpty => "an empty directory",
            Self::RealNonEmpty => "a directory with content",
            Self::OwnedLink(_) => "an owned Oxide link",
            Self::Dangling(_) => "a link whose root is not available",
            Self::Foreign => "a symlink or file Oxide does not own",
        }
    }

    pub(in crate::outputs) fn code(&self) -> &'static str {
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
    pub(in crate::outputs) fn link_root(&self) -> Option<&Path> {
        match self {
            Self::OwnedLink(root) | Self::Dangling(root) => Some(root),
            _ => None,
        }
    }

    /// The verified real root: an owned link whose root is available.
    pub(in crate::outputs) fn root(&self) -> Option<&Path> {
        match self {
            Self::OwnedLink(root) => Some(root),
            _ => None,
        }
    }
}

/// `oxide outputs ...`
pub fn run(args: &[OsString]) -> Result<u8, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        ["plan"] => plan::plan_command(),
        ["prune", ..] => crate::prune::run(
            &args
                .iter()
                .map(|arg| OsString::from(*arg))
                .collect::<Vec<_>>(),
        ),
        ["adopt"] => adopt::adopt_command(),
        ["detach"] => adopt::detach_command(),
        ["cancel-reservation"] => bind::cancel_command(),
        ["status"] => plan::status_command(),
        ["-h" | "--help"] | [] => {
            println!("{USAGE}");
            Ok(0)
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Test-only injection points for a write-ahead crash. They live on the one
/// `Context` a test drives, so a parallel `cargo test` can never inject a
/// crash into another test's fixture, and a release build carries neither the
/// hook nor its name. Each hook fires once and then clears itself, so the
/// replay the test drives afterwards runs uninjected.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(in crate::outputs) struct TestHooks {
    /// Stop at this write-ahead point, exactly as a killed process would.
    crash: Option<String>,
}

/// One checkout's output context. Read-only: a receipt read never creates a
/// directory.
pub(in crate::outputs) struct Context {
    pub(in crate::outputs) control: Option<PathBuf>,
    pub(in crate::outputs) workspace: PathBuf,
    pub(in crate::outputs) checkout_id: String,
    /// Why this command's intermediate slot comes from the internal pool
    /// although a removable drive is registered. Outputs then never select
    /// the external root: only a removable store supervises the drive's
    /// removal, so a command never writes to it unwatched.
    pub(in crate::outputs) pool_fallback: Option<String>,
    #[cfg(test)]
    pub(in crate::outputs) hooks: std::cell::RefCell<TestHooks>,
}

impl Context {
    pub(in crate::outputs) fn open() -> Result<Self, String> {
        let settings = store::settings()?;
        let workspace = identity::locate_workspace(CARGO, None)?;
        let control = Store::open_recorded(&settings)?.map(|store| store.control().to_path_buf());
        Ok(Self::new(control, workspace))
    }

    pub(in crate::outputs) fn new(control: Option<PathBuf>, workspace: PathBuf) -> Self {
        let checkout_id = identity::digest(&workspace.to_string_lossy());
        Self {
            control,
            workspace,
            checkout_id,
            pool_fallback: None,
            #[cfg(test)]
            hooks: std::cell::RefCell::new(TestHooks::default()),
        }
    }

    pub(in crate::outputs) fn with_pool_fallback(mut self, reason: Option<&str>) -> Self {
        self.pool_fallback = reason.map(str::to_owned);
        self
    }

    /// Ask this context to stop at a write-ahead `point`, once.
    #[cfg(test)]
    pub(in crate::outputs) fn inject_crash(&self, point: &str) {
        self.hooks.borrow_mut().crash = Some(point.to_owned());
    }

    /// Stop at a write-ahead point without further state, exactly as a killed
    /// process would leave the fixture. Only a test's own context can ask for
    /// it, and only once.
    pub(in crate::outputs) fn crash_point(&self, point: &str) -> Result<(), String> {
        #[cfg(test)]
        {
            let mut hooks = self.hooks.borrow_mut();
            if hooks.crash.as_deref() == Some(point) {
                hooks.crash = None;
                return Err(format!(
                    "outputs: injected transition stop at {point}; the receipt stays prepared"
                ));
            }
        }
        let _ = point;
        Ok(())
    }

    pub(in crate::outputs) fn control(&self) -> Option<&Path> {
        self.control.as_deref()
    }

    pub(in crate::outputs) fn mode(&self) -> Result<Option<DriveMode>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => Ok(removable::read_registration(control)?.map(|item| item.mode)),
        }
    }

    pub(in crate::outputs) fn receipt(&self) -> Result<Option<Receipt>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => receipt::read_receipt(control, &self.workspace, &self.checkout_id),
        }
    }

    pub(in crate::outputs) fn target(&self) -> Result<TargetState, String> {
        root::target_state(&self.workspace, &self.checkout_id)
    }

    pub(in crate::outputs) fn link(&self) -> PathBuf {
        self.workspace.join("target")
    }
}

pub(in crate::outputs) fn mode_label(mode: Option<DriveMode>) -> &'static str {
    match mode {
        Some(DriveMode::Permanent) => "permanent",
        Some(DriveMode::Ephemeral) => "ephemeral",
        None => "unregistered",
    }
}

/// Refuse an explicit output command while this checkout has unknown writer
/// custody: a lost slot whose writers were never positively closed may still
/// write through the output front door, so no link is replaced or restored.
pub(in crate::outputs) fn refuse_unknown_custody(
    store: &Store,
    checkout: &Path,
    action: &str,
) -> Result<(), String> {
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
pub(in crate::outputs) fn output_store() -> Result<Option<Store>, String> {
    let settings = store::settings()?;
    removable::coordinate(Store::open_existing_with(&settings)?)
}

/// Acquire the checkout's clean barrier: no other command in this checkout
/// holds or takes a slot, and no clean runs, until the guard is released.
pub(in crate::outputs) fn barrier(
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
pub(in crate::outputs) fn release_barrier(store: &Store, guard: CleanGuard) -> Result<(), String> {
    store.abandon_clean(guard)
}
