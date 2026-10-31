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
    static BEFORE_HOLDING_RENAME: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside a holding rename, immediately after the
/// source root's descriptor identity was verified and strictly before the
/// `renameat`. This is how a unit test swaps the observed source name so the
/// rename moves an object that is not the verified one — the containment path
/// (oxide#190).
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct BeforeHoldingRenameGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn before_holding_rename(action: impl FnOnce() + 'static) -> BeforeHoldingRenameGuard {
    BEFORE_HOLDING_RENAME.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping before-holding-rename injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    BeforeHoldingRenameGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for BeforeHoldingRenameGuard {
    fn drop(&mut self) {
        BEFORE_HOLDING_RENAME.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed before-holding-rename action, if any.
pub(crate) fn run_before_holding_rename() {
    let action = BEFORE_HOLDING_RENAME.with(|cell| cell.borrow_mut().take());
    if let Some(action) = action {
        action();
    }
}

thread_local! {
    static AFTER_HOLDING_RENAME: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Arm one action that runs inside a holding rename, immediately after the
/// rename and its moved-identity verification and strictly before the caller's
/// durable row write. This is how a unit test models the crash window between
/// an eviction's rename and its row (oxide#190).
#[must_use = "dropping the guard ends the armed action"]
pub(crate) struct AfterHoldingRenameGuard {
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn after_holding_rename(action: impl FnOnce() + 'static) -> AfterHoldingRenameGuard {
    AFTER_HOLDING_RENAME.with(|cell| {
        assert!(
            cell.borrow().is_none(),
            "overlapping after-holding-rename injections"
        );
        *cell.borrow_mut() = Some(Box::new(action));
    });
    AfterHoldingRenameGuard {
        _thread_bound: PhantomData,
    }
}

impl Drop for AfterHoldingRenameGuard {
    fn drop(&mut self) {
        AFTER_HOLDING_RENAME.with(|cell| {
            let _ = cell.borrow_mut().take();
        });
    }
}

/// Fire the armed after-holding-rename action, if any.
pub(crate) fn run_after_holding_rename() {
    let action = AFTER_HOLDING_RENAME.with(|cell| cell.borrow_mut().take());
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

thread_local! {
    /// One injected holding-rename failure, armed by a library test after the
    /// durable hold intent so the post-commit window can be exercised.
    static HOLDING_RENAME_FAILURE: Cell<bool> = const { Cell::new(false) };
}

/// Arm one injected holding-rename failure. It fires after the durable hold
/// intent, at the rename itself, so the failure is post-commit.
pub(crate) fn arm_holding_rename_failure() {
    HOLDING_RENAME_FAILURE.with(|armed| armed.set(true));
}

/// Consume the armed holding-rename failure, if any.
pub(crate) fn take_holding_rename_failure() -> bool {
    HOLDING_RENAME_FAILURE.with(|armed| armed.replace(false))
}
