//! Narrow filesystem-error injection for library unit tests.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    ReadDir,
    SymlinkMetadata,
}

struct Injection {
    id: u64,
    operation: Operation,
    kind: io::ErrorKind,
    path: PathBuf,
    hits: usize,
}

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static ACTIVE: RefCell<Option<Injection>> = const { RefCell::new(None) };
}

/// Injects one error kind at one operation on one canonical fixture path.
#[must_use = "dropping the guard ends the injection scope"]
pub(crate) struct InjectedErrorGuard {
    id: u64,
    _thread_bound: PhantomData<Rc<()>>,
}

impl InjectedErrorGuard {
    pub(crate) fn read_dir(path: &Path) -> Self {
        Self::arm(Operation::ReadDir, io::ErrorKind::PermissionDenied, path)
    }

    pub(crate) fn symlink_metadata(path: &Path) -> Self {
        Self::arm(
            Operation::SymlinkMetadata,
            io::ErrorKind::PermissionDenied,
            path,
        )
    }

    /// An `lstat` that finds the entry already renamed away or removed, as a
    /// concurrent ledger rename does between a listing and its lookup.
    pub(crate) fn not_found_symlink_metadata(path: &Path) -> Self {
        Self::arm(Operation::SymlinkMetadata, io::ErrorKind::NotFound, path)
    }

    fn arm(operation: Operation, kind: io::ErrorKind, path: &Path) -> Self {
        let path = fs::canonicalize(path).expect("injected fixture path must exist");
        let id = NEXT_ID.with(|next_id| {
            let id = next_id.get();
            next_id.set(id.checked_add(1).expect("test injection ID exhausted"));
            id
        });
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            assert!(active.is_none(), "overlapping filesystem injections");
            *active = Some(Injection {
                id,
                operation,
                kind,
                path,
                hits: 0,
            });
        });
        Self {
            id,
            _thread_bound: PhantomData,
        }
    }

    /// Number of injected errors, saturated at `usize::MAX`.
    pub(crate) fn hits(&self) -> usize {
        ACTIVE.with(|active| {
            let active = active.borrow();
            let injection = active
                .as_ref()
                .expect("injection guard is no longer active");
            assert_eq!(injection.id, self.id, "injection guard identity changed");
            injection.hits
        })
    }
}

impl Drop for InjectedErrorGuard {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            if active
                .as_ref()
                .is_some_and(|injection| injection.id == self.id)
            {
                let _ = active.take();
            }
        });
    }
}

fn injected_error(operation: Operation, path: &Path) -> Option<io::Error> {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let injection = active.as_mut()?;
        if injection.operation != operation || injection.path.as_path() != path {
            return None;
        }
        injection.hits = injection.hits.saturating_add(1);
        Some(io::Error::from(injection.kind))
    })
}

pub(crate) fn read_dir(path: &Path) -> io::Result<fs::ReadDir> {
    match injected_error(Operation::ReadDir, path) {
        Some(error) => Err(error),
        None => fs::read_dir(path),
    }
}

/// `lstat` of the enumerated entry, read relative to the live directory entry
/// the walker holds. It keeps the same path-keyed injection identity as
/// [`symlink_metadata`], so one fixture fault serves either read.
pub(crate) fn entry_metadata(entry: &fs::DirEntry, path: &Path) -> io::Result<fs::Metadata> {
    match injected_error(Operation::SymlinkMetadata, path) {
        Some(error) => Err(error),
        None => entry.metadata(),
    }
}

thread_local! {
    static METADATA_SOURCES: RefCell<Option<Vec<crate::inventory::MetadataSource>>> =
        const { RefCell::new(None) };
}

/// Record which source one enumerated entry's metadata read used. This is
/// test-only accounting of the source selection, not a syscall count or a
/// performance measurement.
pub(crate) fn note_metadata_source(source: crate::inventory::MetadataSource) {
    METADATA_SOURCES.with(|cell| {
        if let Some(sources) = cell.borrow_mut().as_mut() {
            sources.push(source);
        }
    });
}

/// Run `body` and return the metadata-source selections it made, in order.
pub(crate) fn observe_metadata_sources<T>(
    body: impl FnOnce() -> T,
) -> (T, Vec<crate::inventory::MetadataSource>) {
    let outer = METADATA_SOURCES.with(|cell| cell.replace(Some(Vec::new())));
    let result = body();
    let sources = METADATA_SOURCES
        .with(|cell| cell.replace(outer))
        .unwrap_or_default();
    (result, sources)
}

pub(crate) fn symlink_metadata(path: &Path) -> io::Result<fs::Metadata> {
    match injected_error(Operation::SymlinkMetadata, path) {
        Some(error) => Err(error),
        None => fs::symlink_metadata(path),
    }
}

thread_local! {
    static RACE: RefCell<Option<RaceArm>> = const { RefCell::new(None) };
}

/// One armed Finder-metadata race for the deletion seam (oxide#155): the
/// labelled directory gets the named entry created between the caller's own
/// listing and its removal.
struct RaceArm {
    label: std::ffi::OsString,
    kind: &'static str,
    remaining: usize,
}

/// Arm one deterministic race and remove it again on drop. `kind` is one of
/// the seam's debug-hook kinds; `every_round` recreates the entry on each
/// removal attempt so the fixed bound can be exercised.
#[must_use = "dropping the guard ends the armed race"]
pub(crate) struct RaceGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn arm_race(
    label: &std::ffi::OsStr,
    kind: &'static str,
    every_round: bool,
) -> RaceGuard {
    RACE.with(|race| {
        let mut race = race.borrow_mut();
        assert!(race.is_none(), "overlapping race injections");
        *race = Some(RaceArm {
            label: label.to_os_string(),
            kind,
            remaining: if every_round { usize::MAX } else { 1 },
        });
    });
    RaceGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for RaceGuard {
    fn drop(&mut self) {
        RACE.with(|race| {
            let _ = race.borrow_mut().take();
        });
    }
}

/// One firing for this label, in the form the debug hook reads, if the arm
/// still has one. The arm owns its own count, so one test never consumes
/// another's firing.
pub(crate) fn race_firing(label: &std::ffi::OsStr) -> Option<&'static str> {
    RACE.with(|race| {
        let mut race = race.borrow_mut();
        let arm = race.as_mut()?;
        if arm.label != label && arm.label != "*" {
            return None;
        }
        if arm.remaining == 0 {
            return None;
        }
        if arm.remaining != usize::MAX {
            arm.remaining -= 1;
        }
        Some(arm.kind)
    })
}

struct ArmedUnlink {
    /// Unlinks still to skip before the action fires.
    remaining: u32,
    action: Box<dyn FnOnce()>,
}

thread_local! {
    static BEFORE_UNLINK: RefCell<Option<ArmedUnlink>> = const { RefCell::new(None) };
}

/// Run `action` once, inside the deletion seam, immediately before its first
/// per-entry unlink. This is how a unit test rebinds the directory name at
/// exactly the boundary the seam revalidates, instead of racing on timing.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeUnlinkGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_unlink(action: impl FnOnce() + 'static) -> BeforeUnlinkGuard {
    before_unlink_at(1, action)
}

/// The same, armed for the `index`-th per-entry unlink of the drain (1-based),
/// so a test can swap the *last* verified entry for something else and observe
/// the accepted boundary.
pub(crate) fn before_unlink_at(index: u32, action: impl FnOnce() + 'static) -> BeforeUnlinkGuard {
    BEFORE_UNLINK.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-unlink injections"
        );
        *cell.borrow_mut() = Some(ArmedUnlink {
            remaining: index.max(1),
            action: Box::new(action),
        });
    });
    BeforeUnlinkGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeUnlinkGuard {
    fn drop(&mut self) {
        BEFORE_UNLINK.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed action, if any. Called by the seam before each per-entry
/// unlink; a no-op everywhere else.
/// Fire the armed capture action for this 1-based entry ordinal, if any.
pub(crate) fn run_before_capture(ordinal: u32) {
    let action = BEFORE_CAPTURE.with(|cell| {
        let mut cell = cell.borrow_mut();
        let armed = cell.as_mut()?;
        if armed.ordinal != ordinal {
            return None;
        }
        cell.take().map(|armed| armed.action)
    });
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static BEFORE_CAPTURE: RefCell<Option<ArmedCapture>> = const { RefCell::new(None) };
}

struct ArmedCapture {
    ordinal: u32,
    action: Box<dyn FnOnce()>,
}

/// Arm one action that runs immediately *before* the drain captures the
/// `ordinal`-th verified entry. This is how a test puts a replacement at the
/// original name after its checks and before the capture.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeCaptureGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_capture(ordinal: u32, action: impl FnOnce() + 'static) -> BeforeCaptureGuard {
    BEFORE_CAPTURE.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-capture injections"
        );
        *cell.borrow_mut() = Some(ArmedCapture {
            ordinal: ordinal.max(1),
            action: Box::new(action),
        });
    });
    BeforeCaptureGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeCaptureGuard {
    fn drop(&mut self) {
        BEFORE_CAPTURE.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

pub(crate) fn run_before_unlink() {
    let action = BEFORE_UNLINK.with(|cell| {
        let mut cell = cell.borrow_mut();
        let armed = cell.as_mut()?;
        armed.remaining = armed.remaining.saturating_sub(1);
        if armed.remaining == 0 {
            cell.take().map(|armed| armed.action)
        } else {
            None
        }
    });
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static BEFORE_REPAIR: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside the lease deletion, immediately before an
/// unreadable owned directory's permission repair. This is how a unit test
/// rebinds that name at exactly the repair boundary the no-follow rule has to
/// survive.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeRepairGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_repair(action: impl FnOnce() + 'static) -> BeforeRepairGuard {
    BEFORE_REPAIR.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-repair injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    BeforeRepairGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeRepairGuard {
    fn drop(&mut self) {
        BEFORE_REPAIR.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed repair action, if any.
pub(crate) fn run_before_repair() {
    let action = BEFORE_REPAIR.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

/// The exact point at which a test models an abrupt process end inside the
/// journaled root capture of a whole-slot deletion. A real crash runs no
/// put-back, no journal write and no ledger write, so the armed action panics
/// and the test unwinds without any of that.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootCaptureCrash {
    /// The planned record is durable and the exclusive rename has not run.
    Planned,
    /// The rename is durable and the captured phase is not recorded.
    Renamed,
    /// The captured root's mode was widened and the tree was not walked.
    ModeWidened,
    /// The captured root lost its first child deletion.
    MidDelete,
}

thread_local! {
    static ROOT_CRASH: RefCell<Option<RootCaptureCrash>> = const { RefCell::new(None) };
}

/// Arm one root-capture crash. The armed stage fires once; the guard clears it
/// for a test that does not reach the stage.
#[must_use = "dropping the guard ends the armed crash"]
pub(crate) struct RootCaptureCrashGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn root_capture_crash(stage: RootCaptureCrash) -> RootCaptureCrashGuard {
    ROOT_CRASH.with(|cell| {
        assert!(cell.borrow().is_none(), "overlapping root-capture crashes");
        *cell.borrow_mut() = Some(stage);
    });
    RootCaptureCrashGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for RootCaptureCrashGuard {
    fn drop(&mut self) {
        ROOT_CRASH.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed root-capture crash for this stage, if any.
pub(crate) fn run_root_capture(stage: RootCaptureCrash) {
    let armed = ROOT_CRASH.with(|cell| {
        let mut cell = cell.borrow_mut();
        if cell.as_ref() != Some(&stage) {
            return None;
        }
        cell.take()
    });
    if armed.is_some() {
        panic!("test: modelled crash at root capture stage {stage:?}");
    }
}

thread_local! {
    static BEFORE_CHILD_REOPEN: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside the lease deletion, immediately after an
/// unreadable owned directory's access was repaired and strictly before the
/// deleted private name is reopened and its identity verified. This is how a
/// unit test replaces that name at the exact boundary where a restoration must
/// only ever act on the verified descriptor.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeChildReopenGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_child_reopen(action: impl FnOnce() + 'static) -> BeforeChildReopenGuard {
    BEFORE_CHILD_REOPEN.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-child-reopen injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    BeforeChildReopenGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeChildReopenGuard {
    fn drop(&mut self) {
        BEFORE_CHILD_REOPEN.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed before-child-reopen action, if any.
pub(crate) fn run_before_child_reopen() {
    let action = BEFORE_CHILD_REOPEN.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static BEFORE_WALK_CAPTURE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside the lease's whole-tree walk, immediately
/// after an entry was observed and strictly before it is captured under a
/// private name. This is how a unit test races a replacement into the observed
/// name so the capture picks up an object that is not the observed one.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeWalkCaptureGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_walk_capture(action: impl FnOnce() + 'static) -> BeforeWalkCaptureGuard {
    BEFORE_WALK_CAPTURE.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-walk-capture injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    BeforeWalkCaptureGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeWalkCaptureGuard {
    fn drop(&mut self) {
        BEFORE_WALK_CAPTURE.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed before-walk-capture action, if any.
pub(crate) fn run_before_walk_capture() {
    let action = BEFORE_WALK_CAPTURE.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static FORCED_INVALID_NAME: Cell<bool> = const { Cell::new(false) };
}

/// Arm the walk's name check as if the entry's name were not valid UTF-8.
/// APFS refuses to create such a name at all, so this injects the *input* of
/// the check — the production refusal path itself is what a test then asserts.
#[must_use = "dropping the guard ends the forced name"]
pub(crate) struct ForcedInvalidNameGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn force_invalid_name() -> ForcedInvalidNameGuard {
    FORCED_INVALID_NAME.with(|cell| {
        assert!(!cell.get(), "overlapping forced invalid names");
        cell.set(true);
    });
    ForcedInvalidNameGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for ForcedInvalidNameGuard {
    fn drop(&mut self) {
        FORCED_INVALID_NAME.with(|cell| cell.set(false));
    }
}

pub(crate) fn forced_invalid_name() -> bool {
    FORCED_INVALID_NAME.with(std::cell::Cell::get)
}

thread_local! {
    static AFTER_RESTORED_PARENT_WIDEN: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside replay, immediately after a restored parent
/// directory was given access through its verified descriptor and strictly
/// before its recorded children are read. This is how a test rebinds that name
/// in the window the repair must survive without touching the replacement.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct AfterRestoredParentWidenGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn after_restored_parent_widen(
    action: impl FnOnce() + 'static,
) -> AfterRestoredParentWidenGuard {
    AFTER_RESTORED_PARENT_WIDEN.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping after-restored-parent-widen injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    AfterRestoredParentWidenGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for AfterRestoredParentWidenGuard {
    fn drop(&mut self) {
        AFTER_RESTORED_PARENT_WIDEN.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

pub(crate) fn run_after_restored_parent_widen() {
    let action = AFTER_RESTORED_PARENT_WIDEN.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static AFTER_STEP_CHMOD: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static STEP_OPEN_FAULT: Cell<bool> = const { Cell::new(false) };
}

/// Arm one action that runs immediately after a replay step directory was
/// widened through its verified descriptor and strictly before it is reopened;
/// the action may also arm the injected open failure for that step.
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct AfterStepChmodGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn after_step_chmod(action: impl FnOnce() + 'static) -> AfterStepChmodGuard {
    AFTER_STEP_CHMOD.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping after-step-chmod injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    AfterStepChmodGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for AfterStepChmodGuard {
    fn drop(&mut self) {
        AFTER_STEP_CHMOD.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
        STEP_OPEN_FAULT.with(|cell| cell.set(false));
    }
}

pub(crate) fn run_after_step_chmod() {
    let action = AFTER_STEP_CHMOD.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

/// Arm the injected post-widening reopen failure for the next step.
pub(crate) fn arm_step_open_failure() {
    STEP_OPEN_FAULT.with(|cell| cell.set(true));
}

pub(crate) fn injected_step_open_failure() -> bool {
    STEP_OPEN_FAULT.with(std::cell::Cell::get)
}

thread_local! {
    static READDIR_FAULT: Cell<Option<(u64, libc::c_int)>> = const { Cell::new(None) };
}

/// Arm one injected `readdir` failure that fires once the listing has already
/// collected `after_entries` names. APFS does not fail a listing on demand, so
/// this injects the *input* of the seam's end-of-directory classification — the
/// production null-return-and-`errno` path is what a test then asserts.
#[must_use = "dropping the guard ends the armed readdir fault"]
pub(crate) struct ReaddirFaultGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn readdir_fault_after(after_entries: u64, code: libc::c_int) -> ReaddirFaultGuard {
    READDIR_FAULT.with(|cell| {
        assert!(cell.get().is_none(), "overlapping readdir faults");
        cell.set(Some((after_entries, code)));
    });
    ReaddirFaultGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for ReaddirFaultGuard {
    fn drop(&mut self) {
        READDIR_FAULT.with(|cell| cell.set(None));
    }
}

/// The errno code the armed `readdir` fault reports, once the listing has
/// collected `listed` names, if that fault is still armed.
pub(crate) fn faulted_readdir(listed: u64) -> Option<libc::c_int> {
    READDIR_FAULT.with(|cell| {
        let armed = cell.get()?;
        if listed < armed.0 {
            return None;
        }
        cell.set(None);
        Some(armed.1)
    })
}
