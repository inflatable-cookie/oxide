//! Transparent descriptor operations with guarded durable-point pauses.
//! Every filesystem fact/effect delegates to the real OS. No evaluator is replaced.
use barge::error::Result;
use barge::os::*;
use std::path::{Path, PathBuf};
pub(super) struct ObservedDirs<'a> {
    pub inner: &'a dyn DirOps,
    pub control: &'a Path,
    pub intent: &'a super::Intent,
    pub renamed: std::cell::Cell<bool>,
}
impl ObservedDirs<'_> {
    fn pause(&self, phase: &str) -> Result<()> {
        if std::env::var("OXIDE_TEST_BARGE_PAUSE_PHASE").as_deref() == Ok(phase)
            && super::super::prune_fixture_fault(
                self.control,
                "OXIDE_TEST_BARGE_DURABLE_PAUSE",
                &self.intent.identity.root,
                &format!("barge-{phase}-{}", self.intent.id),
            )
        {
            let evidence = super::super::proof_evidence().ok_or_else(|| {
                barge::error::Error::new(barge::error::ErrorKind::Io, "proof evidence absent")
            })?;
            let stop = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !evidence
                .join(format!("barge-phase-release-{}", self.intent.id))
                .exists()
            {
                if std::time::Instant::now() >= stop {
                    return Err(barge::error::Error::new(
                        barge::error::ErrorKind::Io,
                        "durable-point fixture pause expired",
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        Ok(())
    }
}
impl DirOps for ObservedDirs<'_> {
    fn open_path(&self, path: &Path) -> Result<DirHandle> {
        self.inner.open_path(path)
    }
    fn path_facts(&self, path: &Path) -> Result<Option<EntryFacts>> {
        self.inner.path_facts(path)
    }
    fn open_child_dir(&self, parent: &DirHandle, name: &str) -> Result<DirHandle> {
        self.inner.open_child_dir(parent, name)
    }
    fn duplicate_dir(&self, handle: &DirHandle) -> Result<DirHandle> {
        self.inner.duplicate_dir(handle)
    }
    fn child_facts(&self, parent: &DirHandle, name: &str) -> Result<Option<EntryFacts>> {
        self.inner.child_facts(parent, name)
    }
    fn create_child_dir(&self, parent: &DirHandle, name: &str, mode: u32) -> Result<DirId> {
        self.inner.create_child_dir(parent, name, mode)
    }
    fn publish_child_file(
        &self,
        parent: &DirHandle,
        name: &str,
        bytes: &[u8],
        mode: u32,
    ) -> Result<()> {
        self.inner.publish_child_file(parent, name, bytes, mode)
    }
    fn sync_dir(&self, directory: &DirHandle) -> Result<()> {
        let result = self.inner.sync_dir(directory);
        if result.is_ok() && self.renamed.replace(false) {
            self.pause("renamed")?;
        }
        result
    }
    fn read_child_file(
        &self,
        parent: &DirHandle,
        name: &str,
        cap: usize,
    ) -> Result<Option<Vec<u8>>> {
        self.inner.read_child_file(parent, name, cap)
    }
    fn list_children(&self, parent: &DirHandle, max_names: usize) -> Result<ChildListing> {
        self.inner.list_children(parent, max_names)
    }
    fn list_names(&self, parent: &DirHandle, max_names: usize) -> Result<DirNames> {
        self.inner.list_names(parent, max_names)
    }
    fn remove_child(
        &self,
        parent: &DirHandle,
        name: &str,
        kind: RemoveKind,
    ) -> Result<ChildRemoval> {
        let result = self.inner.remove_child(parent, name, kind);
        if matches!(result, Ok(ChildRemoval::Removed))
            && kind == RemoveKind::File
            && name != super::super::outputs::MARKER
        {
            self.pause("partial")?;
        }
        result
    }
    fn rename_child(&self, parent: &DirHandle, from: &str, to: &str) -> Result<RenameOutcome> {
        if to.starts_with(".barge-prune-") {
            self.pause("intent")?;
        }
        let result = self.inner.rename_child(parent, from, to)?;
        if to.starts_with(".barge-prune-") && matches!(result, RenameOutcome::Renamed) {
            self.renamed.set(true);
        }
        Ok(result)
    }
    fn set_dir_mode(&self, handle: &DirHandle, mode: u32) -> Result<()> {
        self.inner.set_dir_mode(handle, mode)
    }
    fn dir_facts(&self, handle: &DirHandle) -> Result<EntryFacts> {
        self.inner.dir_facts(handle)
    }
    fn recheck_dir(&self, handle: &DirHandle) -> Result<DirId> {
        self.inner.recheck_dir(handle)
    }
    fn create_child_symlink(&self, parent: &DirHandle, name: &str, target: &Path) -> Result<()> {
        self.inner.create_child_symlink(parent, name, target)
    }
    fn read_link(&self, path: &Path) -> Result<Option<PathBuf>> {
        self.inner.read_link(path)
    }
    fn random_bytes(&self, length: usize) -> Result<Vec<u8>> {
        self.inner.random_bytes(length)
    }
}
