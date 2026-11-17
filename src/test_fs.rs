//! Narrow filesystem-error injection for library unit tests.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::rc::Rc;

struct Injection {
    id: u64,
    path: PathBuf,
    hits: usize,
}

thread_local! {
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static ACTIVE: RefCell<Option<Injection>> = const { RefCell::new(None) };
}

/// Injects a read-directory refusal at one canonical fixture path.
#[must_use = "dropping the guard ends the injection scope"]
pub(crate) struct InjectedErrorGuard {
    id: u64,
    _thread_bound: PhantomData<Rc<()>>,
}

impl InjectedErrorGuard {
    pub(crate) fn read_dir(path: &Path) -> Self {
        let path = fs::canonicalize(path).expect("injected fixture path must exist");
        let id = NEXT_ID.with(|next_id| {
            let id = next_id.get();
            next_id.set(id.checked_add(1).expect("test injection ID exhausted"));
            id
        });
        ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            assert!(active.is_none(), "overlapping filesystem injections");
            *active = Some(Injection { id, path, hits: 0 });
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

fn injected_error(path: &Path) -> Option<io::Error> {
    ACTIVE.with(|active| {
        let mut active = active.borrow_mut();
        let injection = active.as_mut()?;
        if injection.path.as_path() != path {
            return None;
        }
        injection.hits = injection.hits.saturating_add(1);
        Some(io::Error::from(io::ErrorKind::PermissionDenied))
    })
}

pub(crate) fn read_dir(path: &Path) -> io::Result<fs::ReadDir> {
    match injected_error(path) {
        Some(error) => Err(error),
        None => fs::read_dir(path),
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
