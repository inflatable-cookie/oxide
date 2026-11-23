//! Descriptor-relative inspection and removal for markerless legacy directories.

use crate::child_temp::{Dir, Entry};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) struct Directory {
    pub(super) path: PathBuf,
    handle: Dir,
    identity: (u64, u64),
}

impl Directory {
    pub(super) fn new(path: PathBuf, file: File) -> Result<Self, String> {
        Self::from_handle(path, Dir::from_file(file))
    }

    fn from_handle(path: PathBuf, handle: Dir) -> Result<Self, String> {
        let identity = handle.identity().map_err(|error| at(&path, error))?;
        Ok(Self {
            path,
            handle,
            identity,
        })
    }

    pub(super) fn identity(&self) -> (u64, u64) {
        self.identity
    }

    pub(super) fn verify_path(&self) -> Result<(), String> {
        let current = Dir::open(&self.path)
            .and_then(|dir| dir.identity())
            .map_err(|error| at(&self.path, error))?;
        if current != self.identity {
            return Err(at(&self.path, "directory changed during legacy upgrade"));
        }
        Ok(())
    }

    pub(super) fn names(&self) -> Result<Vec<OsString>, String> {
        let mut names = self.handle.names().map_err(|error| at(&self.path, error))?;
        names.sort_unstable();
        Ok(names)
    }

    pub(super) fn entry(&self, name: &OsStr) -> Result<Option<Entry>, String> {
        match self.handle.entry(name) {
            Ok(entry) if entry.dev == self.identity.0 => Ok(Some(entry)),
            Ok(_) => Err(at(&self.path.join(name), "entry is on another filesystem")),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(at(&self.path.join(name), error)),
        }
    }

    pub(super) fn directory(&self, name: &OsStr) -> Result<Option<Self>, String> {
        let Some(entry) = self.entry(name)? else {
            return Ok(None);
        };
        let path = self.path.join(name);
        // SAFETY: geteuid has no preconditions.
        if !entry.dir || entry.uid != unsafe { libc::geteuid() } || entry.mode & 0o022 != 0 {
            return Err(at(&path, "expected a real directory owned by the current user and not group- or other-writable"));
        }
        self.open_child(name, entry).map(Some)
    }

    pub(super) fn required_dir(&self, name: &OsStr) -> Result<Self, String> {
        self.directory(name)?
            .ok_or_else(|| at(&self.path.join(name), "directory disappeared"))
    }

    fn open_child(&self, name: &OsStr, entry: Entry) -> Result<Self, String> {
        let path = self.path.join(name);
        let handle = self
            .handle
            .open_dir(name)
            .map_err(|error| at(&path, error))?;
        let directory = Self::from_handle(path, handle)?;
        if directory.identity != (entry.dev, entry.ino) {
            return Err(at(&directory.path, "directory changed while being opened"));
        }
        Ok(directory)
    }

    pub(super) fn lock_file(&self, name: &OsStr) -> Result<Option<File>, String> {
        let Some(entry) = self.entry(name)? else {
            return Ok(None);
        };
        let path = self.path.join(name);
        // SAFETY: geteuid has no preconditions.
        if !entry.regular || entry.uid != unsafe { libc::geteuid() } || entry.mode & 0o022 != 0 {
            return Err(at(&path, "expected a safe regular lock file"));
        }
        let file = self
            .handle
            .open_file(name)
            .map_err(|error| at(&path, error))?;
        let opened = file.metadata().map_err(|error| at(&path, error))?;
        if (entry.dev, entry.ino) != (opened.dev(), opened.ino()) {
            return Err(at(&path, "lock changed while being opened"));
        }
        lock(file, &path).map(Some)
    }

    pub(super) fn lock(&self) -> Result<File, String> {
        let file = self
            .handle
            .open_file(OsStr::new("."))
            .map_err(|error| at(&self.path, error))?;
        lock(file, &self.path)
    }

    pub(super) fn require_unmarked(&self) -> Result<(), String> {
        if self
            .entry(OsStr::new(oxide_storage::storage::ROOT_MARKER))?
            .is_some()
        {
            return Err(at(&self.path, "unexpected Barge marker"));
        }
        Ok(())
    }

    pub(super) fn inspect_tree(&self) -> Result<(), String> {
        self.walk_tree(false, 0)
    }

    fn walk_tree(&self, remove: bool, depth: usize) -> Result<(), String> {
        if depth >= 128 {
            return Err(at(&self.path, "legacy directory tree is too deep"));
        }
        if depth > 0 {
            self.require_unmarked()?;
        }
        for name in self.names()? {
            let Some(entry) = self.entry(&name)? else {
                continue;
            };
            if entry.dir {
                let child = self.open_child(&name, entry)?;
                child.walk_tree(remove, depth + 1)?;
                if remove {
                    self.remove_empty(&name, child.identity)?;
                }
            } else if remove {
                // unlinkat removes a symlink itself without following it.
                self.unlink(&name, false)?;
            }
        }
        Ok(())
    }

    pub(super) fn remove_tree(&self, name: &OsStr, expected: (u64, u64)) -> Result<(), String> {
        let child = self.required_dir(name)?;
        child.require_unmarked()?;
        if child.identity != expected {
            return Err(at(&child.path, "directory changed since planning"));
        }
        child.walk_tree(true, 0)?;
        self.remove_empty(name, expected)
    }

    pub(super) fn remove_empty(&self, name: &OsStr, expected: (u64, u64)) -> Result<(), String> {
        if !self
            .entry(name)?
            .is_some_and(|entry| entry.dir && (entry.dev, entry.ino) == expected)
        {
            return Err(at(
                &self.path.join(name),
                "directory changed before removal",
            ));
        }
        self.unlink(name, true)
    }

    pub(super) fn sync(&self) -> Result<(), String> {
        self.handle
            .open_file(OsStr::new("."))
            .and_then(|file| file.sync_all())
            .map_err(|error| at(&self.path, error))
    }

    pub(super) fn unlink(&self, name: &OsStr, directory: bool) -> Result<(), String> {
        self.handle
            .remove(name, directory)
            .map_err(|error| at(&self.path.join(name), error))
    }
}

fn lock(file: File, path: &Path) -> Result<File, String> {
    if !crate::store::flock_nonblocking(&file)? {
        return Err(at(path, "lock is held"));
    }
    Ok(file)
}

pub(super) fn at(path: &Path, error: impl std::fmt::Display) -> String {
    format!("{}: {error}", path.display())
}

/// A missing workspace on an absent drive is not evidence of deletion.
pub(super) fn workspace_missing(path: &Path) -> Result<bool, String> {
    if !path.is_absolute()
        || !path.components().all(|part| {
            matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::RootDir
            )
        })
    {
        return Err(at(path, "workspace path is unsafe"));
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if let Ok(relative) = path.strip_prefix("/Volumes") {
                if let Some(name) = relative.iter().next() {
                    let mount = Path::new("/Volumes").join(name);
                    if oxide_storage::storage::is_drive_mount_point(&mount)
                        && barge::Os::volume_identity(&oxide_storage::storage::OxideOs, &mount)
                            .map_err(|error| at(&mount, error))?
                            .is_none()
                    {
                        return Err(at(
                            path,
                            "cannot establish that the workspace volume is present",
                        ));
                    }
                }
            }
            Ok(true)
        }
        Err(error) => Err(at(path, error)),
    }
}
