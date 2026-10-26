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
    path: PathBuf,
    hits: usize,
}

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static ACTIVE: RefCell<Option<Injection>> = const { RefCell::new(None) };
}

/// Injects `PermissionDenied` at one operation on one canonical fixture path.
#[must_use = "dropping the guard ends the PermissionDenied injection scope"]
pub(crate) struct PermissionDeniedGuard {
    id: u64,
    _thread_bound: PhantomData<Rc<()>>,
}

impl PermissionDeniedGuard {
    pub(crate) fn read_dir(path: &Path) -> Self {
        Self::arm(Operation::ReadDir, path)
    }

    pub(crate) fn symlink_metadata(path: &Path) -> Self {
        Self::arm(Operation::SymlinkMetadata, path)
    }

    fn arm(operation: Operation, path: &Path) -> Self {
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

impl Drop for PermissionDeniedGuard {
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
        Some(io::Error::from(io::ErrorKind::PermissionDenied))
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
