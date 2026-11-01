//! Read-only scratch admission facts and routing.
use crate::config::{self, PathRefusal, Settings};
use crate::scratch_admission::{self as admission, Decision, Document, Record, Refusal};
use oxide_storage::volume::Binding;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

const CREATION: &str = "scratch pool creation on an external volume is not available in this build (oxide#173 S3); nothing was created";
const REPAIR: &str = "the scratch pool's host identity record is missing; repair lands in oxide#173 S3; nothing was written";

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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadDocument::Missing,
        Err(_) => return ReadDocument::Invalid,
    };
    let Ok(meta) = file.metadata() else {
        return ReadDocument::Invalid;
    };
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
        Refusal::CreationUnavailable => CREATION.into(),
        Refusal::RecordRepairUnavailable => REPAIR.into(),
        Refusal::Path(e) => e.message(),
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
    let decision = admission::decide_for_open(admission::Facts {
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
        volume_uuid: uuid,
        mount_point: mount,
        binding: binding.fact(),
        record: record.fact(),
        pool_exists: exists,
        incomplete: exists && !pool.join("config.json").is_file(),
        record_write_failed_after_creation: false,
        status,
    });
    match decision {
        Decision::Ordinary => config::refuse_unsafe_pool_path(pool).map(|()| None),
        Decision::Admit => Ok(parsed.map(|r| config::bounded(&r.volume_uuid))),
        Decision::Refuse(r) | Decision::Unavailable(r) | Decision::RefuseAndRemoveOwnLeaf(r) => {
            Err(refusal(r, pool, status))
        }
    }
}
