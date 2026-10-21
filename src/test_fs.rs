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
            let injection = active.as_ref().expect("injection guard is no longer active");
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

pub(crate) fn symlink_metadata(path: &Path) -> io::Result<fs::Metadata> {
    match injected_error(Operation::SymlinkMetadata, path) {
        Some(error) => Err(error),
        None => fs::symlink_metadata(path),
    }
}
