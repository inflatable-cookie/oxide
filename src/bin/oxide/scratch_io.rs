//! Read-only scratch admission facts and routing.
use crate::config::{self, PathRefusal, Settings};
use crate::scratch_admission::{self as admission, Decision, Document, Record, Refusal};
use oxide_storage::volume::Binding;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

enum ReadDocument {
    Missing,
    Invalid,
    Bytes(Vec<u8>),
}
impl ReadDocument {
    fn fact(&self) -> Document<'_> {
        match self {
            Self::Missing => Document::Missing,
            Self::Invalid => Document::Invalid,
            Self::Bytes(b) => Document::Bytes(b),
        }
    }
}
fn read_document(path: &Path, limit: u64) -> ReadDocument {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return ReadDocument::Missing,
        Err(_) => return ReadDocument::Invalid,
    };
    let Ok(meta) = file.metadata() else {
        return ReadDocument::Invalid;
    };
    // SAFETY: getuid has no preconditions.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::getuid() }
        || meta.mode() & 0o077 != 0
        || meta.len() > limit
    {
        return ReadDocument::Invalid;
    }
    let mut bytes = Vec::new();
    if file.take(limit + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > limit {
        return ReadDocument::Invalid;
    }
    ReadDocument::Bytes(bytes)
}

fn record_path(pool: &Path) -> Result<PathBuf, String> {
    let hash = Sha256::digest(pool.as_os_str().as_bytes());
    let name = hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    Ok(config::Facts::from_env()
        .state_dir()?
        .join("scratch-pools")
        .join(format!("{name}.json")))
}
fn nearest(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut p = path;
    loop {
        match fs::symlink_metadata(p) {
            Ok(_) => return Some((p.to_path_buf(), fs::canonicalize(p).ok()?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => p = p.parent()?,
            Err(_) => return None,
        }
    }
}
fn mount_for(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    if parts.next()? != Component::RootDir || parts.next()?.as_os_str() != "Volumes" {
        return None;
    }
    Some(Path::new("/Volumes").join(parts.next()?.as_os_str()))
}
fn safe_root(root: &Path, mount: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(root) else {
        return false;
    };
    root.is_absolute()
        && root != mount
        && meta.is_dir()
        && !meta.file_type().is_symlink()
        && meta.uid() == unsafe { libc::getuid() }
        && meta.mode() & 0o022 == 0
        && fs::canonicalize(root).is_ok_and(|p| p == root)
        && fs::metadata(mount).is_ok_and(|m| m.dev() == meta.dev())
}
fn refusal(reason: Refusal, pool: &Path, status: bool) -> String {
    match reason {
        Refusal::Path(e) => e.message(),
        Refusal::Class("scratch-incomplete") => format!("scratch-incomplete: prepared pool at {} requires operator cleanup", config::bounded(&pool.display().to_string())),
        Refusal::Class(c) => c.to_owned(),
        Refusal::VolumeAbsent { uuid: Some(uuid), .. } if status => format!("scratch volume {} absent", config::bounded(&uuid)),
        Refusal::VolumeAbsent { uuid: Some(uuid), mount: Some(mount) } => format!("the scratch pool at {} is bound to volume {} at {}; that volume is absent", config::bounded(&pool.display().to_string()), config::bounded(&uuid), config::bounded(&mount.display().to_string())),
        Refusal::VolumeAbsent { .. } => format!("the scratch pool's volume is absent and no host record matches {}; nothing was created", config::bounded(&pool.display().to_string())),
    }
}
/// A verified scratch UUID, or no scratch fact. Performs no writes or pool walk.
pub(crate) fn evaluate(settings: &Settings, status: bool) -> Result<Option<String>, String> {
    let normalized: PathBuf = settings.pool.components().collect();
    let pool = &normalized;
    // Status consults the host record before judging a potentially absent volume.
    let status_record = status.then(|| match record_path(pool) {
        Ok(p) => read_document(&p, 4096),
        Err(_) => ReadDocument::Invalid,
    });
    if matches!(status_record, Some(ReadDocument::Missing)) {
        return Ok(None);
    }
    let ordinary = config::judge_pool_path(&settings.pool);
    let declaration = if status {
        None
    } else {
        std::env::var_os("OXIDE_SCRATCH_ROOT").map(PathBuf::from)
    };
    if !status && declaration.is_none() {
        return ordinary.map(|()| None).map_err(PathRefusal::message);
    }
    if !status && ordinary.is_err() && !matches!(ordinary, Err(PathRefusal::ExternalVolume { .. }))
    {
        return ordinary.map(|()| None).map_err(PathRefusal::message);
    }
    let target =
        nearest(pool).and_then(|(p, c)| pool.strip_prefix(p).ok().map(|rest| c.join(rest)));
    let inside = declaration
        .as_ref()
        .zip(target.as_ref())
        .is_some_and(|(r, p)| {
            p != r && p.starts_with(r) && fs::canonicalize(r).is_ok_and(|c| c == *r)
        });
    let declared_non_boot_mount = if !status && ordinary.is_ok() && inside {
        non_boot_device(pool)
    } else {
        false
    };
    if !status && ordinary.is_ok() && !declared_non_boot_mount {
        return Ok(None);
    }
    let record = status_record.unwrap_or_else(|| match record_path(pool) {
        Ok(p) => read_document(&p, 4096),
        Err(_) => ReadDocument::Invalid,
    });
    let parsed = match &record {
        ReadDocument::Bytes(b) => serde_json::from_slice::<Record>(b)
            .ok()
            .filter(|r| r.schema == "oxide.scratch-pool-record/1" && r.pool == *pool),
        _ => None,
    };
    let root = if status {
        parsed.as_ref().map(|r| r.scratch_root.clone())
    } else {
        declaration.clone()
    };
    let mount = mount_for(pool)
        .or_else(|| parsed.as_ref().map(|r| r.mount_point.clone()))
        .unwrap_or_default();
    let present = fs::metadata(&mount)
        .ok()
        .zip(fs::metadata("/Volumes").ok())
        .is_some_and(|(a, b)| a.dev() != b.dev());
    let verified = Binding::open(&mount, None).ok();
    let uuid = verified
        .as_ref()
        .map(|v| v.uuid().to_owned())
        .unwrap_or_default();
    let exists = fs::symlink_metadata(pool).is_ok();
    let binding = if exists && present {
        read_document(&pool.join("oxide-managed/scratch.json"), 16384)
    } else {
        ReadDocument::Missing
    };
    let ancestor = match &ordinary {
        Err(PathRefusal::ExternalVolume { canonical, .. }) => canonical.clone(),
        _ => nearest(pool).map(|(_, p)| p).unwrap_or_default(),
    };
    let git = config::judge_git_work_tree(pool, &ancestor);
    let decision = admission::decide(admission::Facts {
        ordinary,
        declared_non_boot_mount,
        git_recheck: (ancestor, git),
        declaration,
        source: settings.pool_source,
        budget_source: settings.budget_source,
        pool: pool.clone(),
        root_safe: root.as_ref().is_some_and(|r| safe_root(r, &mount))
            && target.as_ref().is_some_and(|p| p == pool),
        parent_exists: pool.parent().is_some_and(|p| p.is_dir()),
        volume_present: present,
        volume_verified: verified.is_some(),
        volume_uuid: uuid.clone(),
        mount_point: mount,
        binding: binding.fact(),
        record: record.fact(),
        pool_exists: exists,
        incomplete: incomplete_pool(pool, exists),
        record_write_failed_after_creation: false,
        status,
    });
    match decision {
        Decision::Ordinary => config::refuse_unsafe_pool_path(pool).map(|()| None),
        Decision::Admit => Ok(Some(config::bounded(&uuid))),
        Decision::Refuse(r) | Decision::Unavailable(r) | Decision::RefuseAndRemoveOwnLeaf(r) => {
            Err(refusal(r, pool, status))
        }
    }
}

/// Derive row 20 from the real leaf and pool configuration on every open.
fn incomplete_pool(pool: &Path, exists: bool) -> bool {
    exists && !pool.join("config.json").is_file()
}

/// Publish a bounded private identity document without exposing partial bytes.
fn write_document(path: &Path, bytes: &[u8], limit: usize, exclusive: bool) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if bytes.len() > limit {
        return Err("identity document exceeds its bound".into());
    }
    let parent = path.parent().ok_or("identity path has no parent")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::getuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err("identity directory is not private".into());
    }
    for n in 0..100u32 {
        let tmp = parent.join(format!(".scratch-{}-{n}.tmp", std::process::id()));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let result = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| {
                if exclusive {
                    fs::hard_link(&tmp, path)
                } else {
                    fs::rename(&tmp, path)
                }
            });
        if exclusive || result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        return result.map_err(|e| e.to_string());
    }
    Err("cannot allocate identity temporary file".into())
}

fn identity_bytes(
    pool: &Path,
    root: &Path,
    mount: &Path,
    uuid: &str,
    schema: &str,
) -> Result<Vec<u8>, String> {
    let created_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis()
        .try_into()
        .map_err(|_| "clock overflow")?;
    let created_ms: u64 = created_ms;
    serde_json::to_vec(
        &serde_json::json!({"schema": schema, "pool": pool, "scratch_root": root,
        "mount_point": mount, "volume_uuid": uuid, "created_ms": created_ms}),
    )
    .map_err(|e| e.to_string())
}

/// Repair a missing host record only for an admitted leased open.
pub(crate) fn repair(settings: &Settings, uuid: &str) -> Result<(), String> {
    let pool: PathBuf = settings.pool.components().collect();
    let path = record_path(&pool)?;
    if !matches!(read_document(&path, 4096), ReadDocument::Missing) {
        return evaluate(settings, false).map(|_| ());
    }
    let root = std::env::var_os("OXIDE_SCRATCH_ROOT").ok_or("scratch-root-unsafe")?;
    let mount = mount_for(&pool).ok_or("scratch-volume-unverified")?;
    let volume = Binding::open(&mount, Some(uuid)).map_err(|_| "scratch-binding-mismatch")?;
    volume.verify().map_err(|_| "scratch-binding-mismatch")?;
    let bytes = identity_bytes(
        &pool,
        Path::new(&root),
        &mount,
        uuid,
        "oxide.scratch-pool-record/1",
    )?;
    match write_document(&path, &bytes, 4096, true) {
        Ok(()) => {}
        Err(_) if !matches!(read_document(&path, 4096), ReadDocument::Missing) => {}
        Err(_) => return Err("scratch-record-unwritable".into()),
    }
    evaluate(settings, false).map(|_| ())
}

/// Create this invocation's bound leaf before the pool library initializes it.
pub(crate) fn create(settings: &Settings, uuid: &str, budget: u64) -> Result<(), String> {
    let pool: PathBuf = settings.pool.components().collect();
    let root = std::env::var_os("OXIDE_SCRATCH_ROOT").ok_or("scratch-root-unsafe")?;
    let mount = mount_for(&pool).ok_or("scratch-volume-unverified")?;
    let volume = Binding::open(&mount, Some(uuid)).map_err(|_| "scratch-binding-mismatch")?;
    volume.verify().map_err(|_| "scratch-binding-mismatch")?;
    let binding = identity_bytes(
        &pool,
        Path::new(&root),
        &mount,
        uuid,
        "oxide.scratch-pool/1",
    )?;
    let record = identity_bytes(
        &pool,
        Path::new(&root),
        &mount,
        uuid,
        "oxide.scratch-pool-record/1",
    )?;
    let leaf =
        oxide_storage::lease::managed::create_scratch_leaf(&pool).map_err(|e| e.to_string())?;
    mark_leaf(&pool, &binding)?;
    publish_record_or_compensate(&leaf, &binding, record_path(&pool), &record)?;
    volume.verify().map_err(|_| "scratch-binding-mismatch")?;
    eprintln!(
        "oxide: creating a scratch build pool on volume {}; not for ordinary project builds",
        config::bounded(uuid)
    );
    crate::handoff_trace::phase("scratch-init", || {
        oxide_storage::lease::managed::init_scratch_pool(&leaf, &binding, budget)
    })
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Write the binding marker into the leaf this invocation has just created
/// exclusively. A failed write unwinds what it made: the marker, its directory
/// and, while still empty, the leaf. A leaf that stays is named in the refusal,
/// because an unmarked leaf makes every later open refuse with `binding-missing`.
fn mark_leaf(pool: &Path, binding: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt;
    let control = pool.join("oxide-managed");
    let written = fs::DirBuilder::new()
        .mode(0o700)
        .create(&control)
        .map_err(|e| e.to_string())
        .and_then(|()| write_document(&control.join("scratch.json"), binding, 16384, false));
    let Err(reason) = written else {
        return Ok(());
    };
    let _ = fs::remove_file(control.join("scratch.json"));
    let _ = fs::remove_dir(&control);
    match fs::remove_dir(pool) {
        Ok(()) => Err(reason),
        Err(error) => Err(format!(
            "{reason}; the unmarked leaf {} remains ({error}); remove it before retrying",
            pool.display()
        )),
    }
}

/// If host publication fails, remove only the leaf still owned by this invocation.
fn publish_record_or_compensate(
    leaf: &oxide_storage::lease::managed::ScratchLeaf,
    binding: &[u8],
    path: Result<PathBuf, String>,
    record: &[u8],
) -> Result<(), String> {
    let result = path.and_then(|p| write_document(&p, record, 4096, true));
    if result.is_ok() {
        return Ok(());
    }
    leaf.remove_if_owned(binding)
        .map_err(|_| "scratch-record-unwritable; scratch-incomplete requires operator cleanup")?;
    Err("scratch-record-unwritable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::fs::symlink;

    fn fixture() -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("supervised scratch root");
        let mut name = CString::new(
            Path::new(&base)
                .join("scratch-io.XXXXXX")
                .as_os_str()
                .as_bytes(),
        )
        .unwrap()
        .into_bytes_with_nul();
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        PathBuf::from(OsStr::from_bytes(
            CStr::from_bytes_until_nul(&name).unwrap().to_bytes(),
        ))
    }

    fn prepared(path: &Path, binding: &[u8]) -> oxide_storage::lease::managed::ScratchLeaf {
        let leaf = oxide_storage::lease::managed::create_scratch_leaf(path).unwrap();
        fs::create_dir(path.join("oxide-managed")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path.join("oxide-managed"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        write_document(
            &path.join("oxide-managed/scratch.json"),
            binding,
            16384,
            false,
        )
        .unwrap();
        leaf
    }

    #[test]
    fn a_failed_binding_marker_unwinds_the_new_leaf() {
        let root = fixture();
        let pool = root.join("pool");
        let leaf = oxide_storage::lease::managed::create_scratch_leaf(&pool).unwrap();
        // Over the marker's bound, so the write fails after the directory exists.
        let oversized = vec![b'x'; 16385];
        assert_eq!(
            mark_leaf(&pool, &oversized).unwrap_err(),
            "identity document exceeds its bound"
        );
        assert!(
            !pool.exists(),
            "the empty leaf is removed with its marker dir"
        );
        drop(leaf);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_write_failure_compensates_only_the_owned_leaf() {
        let root = fixture();
        let pool = root.join("pool");
        let binding = b"binding";
        let leaf = prepared(&pool, binding);
        let blocked = root.join("not-a-directory");
        fs::write(&blocked, b"keep").unwrap();
        let error =
            publish_record_or_compensate(&leaf, binding, Ok(blocked.join("record")), b"record")
                .unwrap_err();
        assert_eq!(error, "scratch-record-unwritable");
        assert!(!pool.exists());
        assert_eq!(fs::read(&blocked).unwrap(), b"keep");
        let pool = root.join("swapped");
        let leaf = prepared(&pool, binding);
        fs::rename(&pool, root.join("original")).unwrap();
        fs::create_dir(&pool).unwrap();
        let error = publish_record_or_compensate(&leaf, binding, Err("injected".into()), b"record")
            .unwrap_err();
        assert!(error.contains("scratch-incomplete"));
        assert!(pool.is_dir() && root.join("original/oxide-managed/scratch.json").is_file());
        let pool = root.join("changed-binding");
        let leaf = prepared(&pool, binding);
        fs::write(pool.join("oxide-managed/scratch.json"), b"foreign").unwrap();
        let error = publish_record_or_compensate(&leaf, binding, Err("injected".into()), b"record")
            .unwrap_err();
        assert!(error.contains("scratch-incomplete"));
        assert_eq!(
            fs::read(pool.join("oxide-managed/scratch.json")).unwrap(),
            b"foreign"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_leaf_retry_refuses_incomplete_without_writes() {
        let root = fixture();
        let pool = root.join("pool");
        let uuid = "fixture-volume";
        let binding = identity_bytes(&pool, &root, &root, uuid, "oxide.scratch-pool/1").unwrap();
        let leaf = prepared(&pool, &binding);
        assert!(oxide_storage::lease::managed::init_scratch_pool(&leaf, &binding, 0).is_err());
        assert!(!pool.join("config.json").exists());
        let binding_path = pool.join("oxide-managed/scratch.json");
        let before = fs::read(&binding_path).unwrap();
        let pool_meta = fs::metadata(&pool).unwrap();
        let binding_meta = fs::metadata(&binding_path).unwrap();
        let document = read_document(&binding_path, 16384);
        let facts = admission::Facts {
            ordinary: Err(PathRefusal::ExternalVolume {
                message: "external".into(),
                canonical: root.clone(),
            }),
            declared_non_boot_mount: false,
            git_recheck: (root.clone(), Ok(())),
            declaration: Some(root.clone()),
            source: config::Source::Environment("OXIDE_POOL"),
            budget_source: config::Source::Environment("OXIDE_POOL_CAP"),
            pool: pool.clone(),
            root_safe: true,
            parent_exists: true,
            volume_present: true,
            volume_verified: true,
            volume_uuid: uuid.into(),
            mount_point: root.clone(),
            binding: document.fact(),
            record: admission::Document::Missing,
            pool_exists: fs::symlink_metadata(&pool).is_ok(),
            incomplete: incomplete_pool(&pool, fs::symlink_metadata(&pool).is_ok()),
            record_write_failed_after_creation: false,
            status: false,
        };
        let Decision::Refuse(reason) = admission::decide(facts) else {
            panic!("retry must refuse")
        };
        let message = refusal(reason, &pool, false);
        assert!(message.contains("scratch-incomplete") && message.contains("operator cleanup"));
        assert!(message.contains(&pool.display().to_string()));
        assert_eq!(fs::read(&binding_path).unwrap(), before);
        let after_pool = fs::metadata(&pool).unwrap();
        let after_binding = fs::metadata(&binding_path).unwrap();
        assert_eq!(
            (pool_meta.ino(), pool_meta.mtime(), pool_meta.mtime_nsec()),
            (
                after_pool.ino(),
                after_pool.mtime(),
                after_pool.mtime_nsec()
            )
        );
        assert_eq!(
            (
                binding_meta.ino(),
                binding_meta.mtime(),
                binding_meta.mtime_nsec()
            ),
            (
                after_binding.ino(),
                after_binding.mtime(),
                after_binding.mtime_nsec()
            )
        );
        assert_eq!(fs::read_dir(&pool).unwrap().count(), 1);
        assert!(!pool.join("config.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn private_document_publication_is_bounded_and_does_not_replace_record() {
        let root = fixture();
        let path = root.join("state/record.json");
        write_document(&path, b"first", 4096, true).unwrap();
        assert!(write_document(&path, b"second", 4096, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert!(write_document(&root.join("oversized"), &vec![0; 4097], 4096, true).is_err());
        let link = root.join("link");
        symlink(&path, &link).unwrap();
        assert!(matches!(read_document(&link, 4096), ReadDocument::Invalid));
        assert_eq!(fs::read(&path).unwrap(), b"first");
        fs::remove_dir_all(root).unwrap();
    }
}

fn non_boot_device(pool: &Path) -> bool {
    #[cfg(test)]
    if config::internal_fixture() {
        return false;
    }
    let dev = nearest(pool)
        .and_then(|(_, p)| fs::metadata(p).ok())
        .map(|m| m.dev());
    let boot = fs::metadata(if cfg!(target_os = "macos") {
        "/private/tmp"
    } else {
        "/"
    })
    .ok()
    .map(|m| m.dev());
    dev.is_none() || boot.is_none() || dev != boot
}
