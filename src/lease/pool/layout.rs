//! Pool directory layout: preparing a new pool and refusing foreign or retired entries.

use super::*;

pub(super) fn prepare_new_pool(pool: &Path) -> Result<PathBuf, LeaseError> {
    super::require_absolute(pool)?;
    if pool.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput("path is too long"));
    }
    match fs::symlink_metadata(pool) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(LeaseError::UnsafePath("symlinks are refused"));
            }
            if !metadata.file_type().is_dir() {
                return Err(LeaseError::InvalidInput("pool must be a directory"));
            }
            if metadata.uid() != current_uid() {
                return Err(LeaseError::UnsafePath(
                    "pool must be an owner-only directory",
                ));
            }
            refuse_occupied_init_directory(pool)?;
            if metadata.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(pool, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let name = pool
                .file_name()
                .ok_or(LeaseError::InvalidInput("pool has no name"))?;
            let parent = pool
                .parent()
                .ok_or(LeaseError::InvalidInput("pool has no parent"))?;
            let parent = canonical_existing_dir(parent, false)?;
            let created = parent.join(name);
            // The mode is set at creation, never after it: a `create_dir`
            // followed by `set_permissions` leaves the umask-controlled mode
            // readable to others until the second call (oxide#337).
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&created).map_err(io_error)?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(io_error)?;
            return canonical_existing_dir(&created, true);
        }
        Err(error) => return Err(io_error(error)),
    }
    canonical_existing_dir(pool, true)
}

/// Check the creator's held leaf and the complete prepared shape under the init lock.
pub(super) fn verify_scratch_initializer(
    pool: &Path,
    leaf: &File,
    expected: &[u8],
    locked: bool,
) -> Result<(), LeaseError> {
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let incomplete =
        || LeaseError::InvalidInput("scratch-incomplete: prepared pool requires operator cleanup");
    if expected.len() > 16384 {
        return Err(incomplete());
    }
    let current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(pool)
        .map_err(|_| incomplete())?;
    let held_meta = leaf.metadata().map_err(|_| incomplete())?;
    let meta = current.metadata().map_err(|_| incomplete())?;
    if !meta.is_dir()
        || meta.uid() != current_uid()
        || meta.mode() & 0o7777 != 0o700
        || (meta.dev(), meta.ino()) != (held_meta.dev(), held_meta.ino())
    {
        return Err(incomplete());
    }
    let list = |path: &Path| -> Result<Vec<String>, LeaseError> {
        std::fs::read_dir(path)
            .map_err(|_| incomplete())?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .map_err(|_| incomplete())
            })
            .collect()
    };
    let names = list(pool)?;
    let has_managed = names.iter().any(|n| n == "oxide-managed");
    let has_lock = names.iter().any(|n| n == "lock");
    if !has_managed || names.len() != 1 + usize::from(has_lock) || (locked && !has_lock) {
        return Err(incomplete());
    }
    let managed_names = list(&pool.join("oxide-managed"))?;
    if managed_names.len() != 1 || managed_names[0] != "scratch.json" {
        return Err(incomplete());
    }
    let managed = open_directory_at(current.as_fd(), c"oxide-managed").map_err(|_| incomplete())?;
    let raw = unsafe {
        libc::openat(
            managed.as_raw_fd(),
            c"scratch.json".as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(incomplete());
    }
    let file = unsafe { File::from_raw_fd(raw) };
    let binding_meta = file.metadata().map_err(|_| incomplete())?;
    if !binding_meta.is_file() || binding_meta.uid() != current_uid() || binding_meta.len() > 16384
    {
        return Err(incomplete());
    }
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| incomplete())?;
    if bytes != expected {
        return Err(incomplete());
    }
    Ok(())
}

/// Empty, or only the CLI's leftover `lock` from a failed init.
fn refuse_occupied_init_directory(pool: &Path) -> Result<(), LeaseError> {
    let mut leftover_lock = false;
    for entry in fs::read_dir(pool).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        if leftover_lock || name != LOCK_NAME {
            return Err(LeaseError::InvalidInput(
                "init requires an empty pool directory",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
        if !metadata.file_type().is_file() || metadata.uid() != current_uid() {
            return Err(LeaseError::InvalidInput(
                "init requires an empty pool directory",
            ));
        }
        leftover_lock = true;
    }
    Ok(())
}

/// Read-only, before the pool lock is created or taken: a pool holding the
/// retired image or receipt, or whose config records any backend but the
/// two current ones, refuses untouched. A missing config is left to the
/// locked read, which waits for a concurrent creation.
pub(super) fn refuse_retired_image_pool(pool: &Path) -> Result<(), LeaseError> {
    let paths = BackingPaths::under(pool);
    if paths.image.symlink_metadata().is_ok() || paths.receipt.symlink_metadata().is_ok() {
        return Err(LeaseError::BackingMismatch);
    }
    if pool.join(CONFIG_NAME).symlink_metadata().is_err() {
        return Ok(());
    }
    let config = read_config(pool)?;
    if config.backend == BUDGET_BACKEND_DIRECTORY_V1 || config.backend == VOLUME_BACKEND_APFS_V1 {
        Ok(())
    } else {
        Err(LeaseError::BackingMismatch)
    }
}

/// `lstat` of a pool control entry that the listing just named. `NotFound`
/// means the entry was renamed away or removed in between: a ledger writer
/// renames its `ledger.tmp-*` onto `ledger.json`, and Finder removes its
/// metadata files. The caller skips such an entry. Every other error refuses.
fn listed_control_entry(path: &Path) -> Result<Option<fs::Metadata>, LeaseError> {
    let metadata = fs::symlink_metadata(path);
    match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

pub(super) fn refuse_foreign_pool_entries(pool: &Path) -> Result<(), LeaseError> {
    refuse_foreign_pool_entries_before(pool, None)
}

pub(super) fn refuse_foreign_pool_entries_before(
    pool: &Path,
    deadline: Option<std::time::Instant>,
) -> Result<(), LeaseError> {
    if let Some(deadline) = deadline {
        measurement_remaining(deadline)?;
    }
    let device = fs::symlink_metadata(pool).map_err(io_error)?.dev();
    for entry in fs::read_dir(pool).map_err(io_error)? {
        if let Some(deadline) = deadline {
            measurement_remaining(deadline)?;
        }
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        if is_finder_metadata(&name) {
            // Control-directory bytes, like config and ledger: not payload.
            let Some(metadata) = listed_control_entry(&entry.path())? else {
                continue;
            };
            if !is_tolerated_metadata_file(&metadata, device) {
                return Err(unexpected_entry(POOL_PLACE, &name));
            }
            continue;
        }
        let text = name.to_string_lossy();
        let allowed = matches!(
            text.as_ref(),
            super::CONFIG_NAME
                | super::LEDGER_NAME
                | super::LOCK_NAME
                | super::IMAGE_NAME
                | super::RECEIPT_NAME
                | super::PAYLOAD_NAME
                | super::MANAGED_NAME
        ) || text.starts_with("ledger.tmp-");
        if !allowed {
            return Err(unexpected_entry(POOL_PLACE, &name));
        }
        let Some(metadata) = listed_control_entry(&entry.path())? else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(LeaseError::UnsafePath("symlinks are refused"));
        }
    }
    Ok(())
}
