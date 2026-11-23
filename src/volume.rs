//! Platform mount facts, descriptor-relative directory opens and volume flush.
//!
//! Barge owns registration, location/fallback and root ownership. This seam
//! supplies macOS mount-root and writability facts that a generic containing
//! filesystem UUID does not provide, and keeps descriptors for ordinary I/O.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The only parent a registered mount root may have.
pub const VOLUMES: &str = "/Volumes";

/// What the kernel reports about one opened directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Facts {
    /// `f_mntonname`: the mount point of the file system holding it.
    pub mounted_on: PathBuf,
    /// `f_mntfromname`: the device node, for example `/dev/disk12s1`.
    pub device: String,
    pub fstype: String,
    pub read_only: bool,
    pub root_fs: bool,
    /// `ATTR_VOL_UUID` of the volume holding the directory, if reported.
    pub uuid: Option<String>,
    /// `st_dev` of the directory.
    pub dev: u64,
    pub capacity_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
}

/// Why `path` cannot be used as the registered volume, judged from facts
/// observed on its opened descriptor. Pure, so stale-directory and
/// device-change cases are testable with injected facts.
pub fn judge(path: &Path, facts: &Facts, expected_uuid: Option<&str>) -> Result<(), String> {
    if facts.mounted_on != path {
        return Err(format!(
            "{} is not a mounted volume root (it is a directory on the volume mounted at {}); a stale mount point is never used",
            path.display(),
            facts.mounted_on.display()
        ));
    }
    if facts.fstype != "apfs" {
        return Err(format!("{} is {}, not APFS", path.display(), facts.fstype));
    }
    if facts.root_fs {
        return Err(format!("{} is the boot volume", path.display()));
    }
    if facts.read_only {
        return Err(format!("{} is mounted read-only", path.display()));
    }
    let Some(uuid) = facts.uuid.as_deref() else {
        return Err(format!(
            "{} reports no volume UUID, so it is not a volume root",
            path.display()
        ));
    };
    if let Some(expected) = expected_uuid {
        // A volume UUID is case-insensitive; the platform and a registration
        // may report the same identity in different case.
        if !uuid.eq_ignore_ascii_case(expected) {
            return Err(format!(
                "{} is volume {uuid}, not the registered volume {expected}",
                path.display()
            ));
        }
    }
    Ok(())
}

/// The canonical one-component name of a mount root directly under
/// [`VOLUMES`]. Nested paths, `..`, trailing separators, hidden names and
/// prefix look-alikes refuse.
pub fn mount_name(path: &Path) -> Result<&str, String> {
    let text = path.to_str().ok_or("the mount point must be valid UTF-8")?;
    let mut components = path.components();
    let shape = (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    );
    let name = match shape {
        (
            Some(Component::RootDir),
            Some(Component::Normal(parent)),
            Some(Component::Normal(name)),
            None,
        ) if parent == "Volumes" => name.to_str().ok_or("the mount point must be valid UTF-8")?,
        _ => {
            return Err(format!(
                "{text}: only a volume mounted directly under {VOLUMES} is accepted"
            ))
        }
    };
    if text != format!("{VOLUMES}/{name}") || name.starts_with('.') || name.contains('\0') {
        return Err(format!(
            "{text}: only a volume mounted directly under {VOLUMES} is accepted"
        ));
    }
    Ok(name)
}

/// A verified mount root held open. Identity is rechecked on the descriptor
/// before each use; nothing re-resolves the path to create.
pub struct Binding {
    fd: OwnedFd,
    mount_point: PathBuf,
    uuid: String,
    device: String,
}

impl std::fmt::Debug for Binding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Binding")
            .field("mount_point", &self.mount_point)
            .field("uuid", &self.uuid)
            .field("device", &self.device)
            .finish()
    }
}

impl Binding {
    /// Open a direct child of the platform mount directory, retaining its
    /// descriptor for mount facts and ordinary I/O. Registration and location
    /// decisions belong to Barge.
    pub fn open(mount_point: &Path, expected_uuid: Option<&str>) -> Result<Self, String> {
        let name = mount_name(mount_point)?;
        Self::open_in(Path::new(VOLUMES), name, expected_uuid)
    }

    /// [`Binding::open`] under another parent, without the parent rule.
    /// Tests use it to bind a plain directory standing in for a stale mount
    /// point.
    pub fn open_in(parent: &Path, name: &str, expected_uuid: Option<&str>) -> Result<Self, String> {
        let mount_point = parent.join(name);
        let parent_fd = open_dir(None, parent)?;
        let fd = open_dir(Some(parent_fd.as_fd()), Path::new(name))
            .map_err(|error| format!("{} is not available: {error}", mount_point.display()))?;
        let facts = observe(fd.as_fd())?;
        judge(&mount_point, &facts, expected_uuid)?;
        if u64::try_from(fstat(parent_fd.as_fd())?.st_dev).unwrap_or(u64::MAX) == facts.dev {
            return Err(format!(
                "{} is on its parent's device, so it is not a mounted volume root",
                mount_point.display()
            ));
        }
        let uuid = facts.uuid.clone().ok_or("no volume UUID")?;
        let binding = Self {
            fd,
            mount_point,
            uuid,
            device: facts.device,
        };
        binding.verify()?;
        Ok(binding)
    }

    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    pub fn device(&self) -> &str {
        &self.device
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Recheck the held descriptor and the path: the descriptor is still the
    /// same volume's root, and the path still resolves to that root rather
    /// than a directory left behind after an unmount.
    ///
    pub fn verify(&self) -> Result<Facts, String> {
        let facts = observe(self.fd.as_fd())?;
        judge(&self.mount_point, &facts, Some(&self.uuid))?;
        if facts.device != self.device {
            return Err(format!(
                "{} moved from {} to {}",
                self.mount_point.display(),
                self.device,
                facts.device
            ));
        }
        let by_path = std::fs::symlink_metadata(&self.mount_point)
            .map_err(|error| format!("{} is gone: {error}", self.mount_point.display()))?;
        if by_path.file_type().is_symlink()
            || std::os::unix::fs::MetadataExt::dev(&by_path) != facts.dev
        {
            return Err(format!(
                "{} no longer names volume {}",
                self.mount_point.display(),
                self.uuid
            ));
        }
        Ok(facts)
    }

    /// Flush the whole volume to the device and return the time taken. The
    /// binding is verified before and after, so a volume that left in
    /// between is a failure, never a success.
    pub fn flush(&self) -> Result<Duration, String> {
        self.verify()?;
        let started = std::time::Instant::now();
        flush_volume(&self.fd)?;
        let elapsed = started.elapsed();
        self.verify()
            .map_err(|error| format!("the volume changed during its flush: {error}"))?;
        Ok(elapsed)
    }

    /// Open (creating owner-only if missing) `name` under `dir`, a
    /// descriptor on this volume, after reverifying the volume. The result
    /// must be an owner-only directory of the caller on the same device.
    pub fn dir_at(&self, dir: BorrowedFd<'_>, name: &str, create: bool) -> Result<OwnedFd, String> {
        self.verify()?;
        dir_at(dir, name, create)
    }

    /// Open (creating owner-only each missing component if `create`) the
    /// nested directory `rel` under this volume's mount point. The path is
    /// validated before any effect, the volume is reverified, and every
    /// component is walked from the held descriptor. Intermediate components
    /// are shared parents checked with [`parent_dir_at`]; only the leaf is
    /// Oxide's own directory and keeps the strict [`dir_at`] check.
    pub fn dir_path_at(&self, rel: &str, create: bool) -> Result<OwnedFd, String> {
        relative_components(rel)?;
        self.verify()?;
        walk_dir_path(self.fd.as_fd(), rel, create)
    }

    /// Open (creating owner-only each missing component if `create`) the
    /// nested shared parent prefix `rel` under this volume's mount point.
    /// Every component is checked with [`parent_dir_at`], so callers that
    /// open or validate a shared parent itself (rather than Oxide's own
    /// leaf) accept user-owned `0755` directories while refusing
    /// group- or world-writable and foreign-owned ones.
    pub fn parent_dir_path_at(&self, rel: &str, create: bool) -> Result<OwnedFd, String> {
        relative_components(rel)?;
        self.verify()?;
        walk_parent_dir_path(self.fd.as_fd(), rel, create)
    }
}

/// The deepest nested relative directory a removable pool or outputs root may
/// use under the volume's mount root.
pub const MAX_RELATIVE_DEPTH: usize = 8;

/// Split a volume-relative directory path into its segments. It must be one
/// or more clean `/`-separated names: not empty, not absolute, with no empty
/// segment (leading, trailing or doubled `/`), no `.` or `..`, no NUL, and at
/// most [`MAX_RELATIVE_DEPTH`] segments.
pub fn relative_components(rel: &str) -> Result<Vec<&str>, String> {
    if rel.is_empty() {
        return Err("the relative path is empty".to_owned());
    }
    if rel.contains('\0') {
        return Err(format!("{rel:?} contains NUL"));
    }
    if rel.starts_with('/') {
        return Err(format!(
            "{rel:?} must be relative to the volume root, not absolute"
        ));
    }
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts {
        match *part {
            "" => {
                return Err(format!(
                    "{rel:?} has an empty segment; use clean names separated by single '/'"
                ))
            }
            "." | ".." => return Err(format!("{rel:?} must not contain '.' or '..'")),
            _ => {}
        }
    }
    if parts.len() > MAX_RELATIVE_DEPTH {
        return Err(format!(
            "{rel:?} is deeper than {MAX_RELATIVE_DEPTH} directories"
        ));
    }
    Ok(parts)
}

/// Walk `rel` from `dir`, one [`parent_dir_at`] per intermediate component
/// and one [`dir_at`] for the leaf, so every step keeps the no-follow and
/// same-device checks. Shared parents such as `builds/` may be user-owned
/// and readable (`0755`); only Oxide's own leaf (the payload or outputs
/// directory) must be owner-only (`0700`). The caller validates `rel` first.
pub fn walk_dir_path(dir: BorrowedFd<'_>, rel: &str, create: bool) -> Result<OwnedFd, String> {
    let parts = relative_components(rel)?;
    let (leaf, parents) = parts.split_last().expect("a relative path has a leaf");
    let mut current: Option<OwnedFd> = None;
    for part in parents {
        current = Some(match &current {
            None => parent_dir_at(dir, part, create)?,
            Some(fd) => parent_dir_at(fd.as_fd(), part, create)?,
        });
    }
    match &current {
        None => dir_at(dir, leaf, create),
        Some(fd) => dir_at(fd.as_fd(), leaf, create),
    }
}

/// Walk `rel` from `dir`, one [`parent_dir_at`] per component, so every step
/// keeps the no-follow and same-device checks. For callers that open or
/// validate a shared parent prefix itself (such as `builds` or
/// `builds/oxide`), where no component is Oxide's own leaf. The caller
/// validates `rel` first.
pub fn walk_parent_dir_path(
    dir: BorrowedFd<'_>,
    rel: &str,
    create: bool,
) -> Result<OwnedFd, String> {
    let parts = relative_components(rel)?;
    let mut current: Option<OwnedFd> = None;
    for part in &parts {
        current = Some(match &current {
            None => parent_dir_at(dir, part, create)?,
            Some(fd) => parent_dir_at(fd.as_fd(), part, create)?,
        });
    }
    Ok(current.expect("a relative path has a component"))
}

/// Whether the effective user may write `path`, ACLs included.
pub fn writable(path: &Path) -> Result<bool, String> {
    let text = c_path(path)?;
    // SAFETY: the path is NUL-terminated; faccessat reads it only.
    let status =
        unsafe { libc::faccessat(libc::AT_FDCWD, text.as_ptr(), libc::W_OK, libc::AT_EACCESS) };
    if status == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EACCES | libc::EPERM | libc::EROFS) => Ok(false),
        _ => Err(format!(
            "cannot check write access to {}: {error}",
            path.display()
        )),
    }
}

/// Facts about the directory `path` names, read through a descriptor.
pub fn observe_path(path: &Path) -> Result<Facts, String> {
    let fd = open_dir(None, path)?;
    observe(fd.as_fd())
}

/// Facts about the directory an already-held descriptor names, read through
/// that same descriptor. This is the observation the absence protocol binds
/// to: it never reopens a path, so an ancestor is never resolved a second time
/// and cannot be swapped between the open and the volume reading. Non-macOS
/// builds stay conservative and report no facts.
pub fn observe_held(fd: BorrowedFd<'_>) -> Result<Facts, String> {
    observe(fd)
}

/// Open `name` under `dir`, optionally creating it owner-only. It must be a
/// real directory owned by the caller, owner-only, on `dir`'s device.
pub fn dir_at(dir: BorrowedFd<'_>, name: &str, create: bool) -> Result<OwnedFd, String> {
    open_checked(
        dir,
        name,
        create,
        0o077,
        "an owner-only directory of this user",
    )
}

/// Open `name` under `dir`, optionally creating it owner-only. It must be a
/// real directory owned by the caller, on `dir`'s device, and not group- or
/// world-writable. This is the check for shared intermediate parents (such
/// as a class directory like `builds/`): `0755` and `0700` pass, `0775`,
/// `0777` and foreign-owned directories refuse.
pub fn parent_dir_at(dir: BorrowedFd<'_>, name: &str, create: bool) -> Result<OwnedFd, String> {
    open_checked(
        dir,
        name,
        create,
        0o022,
        "a directory of this user that is not group- or world-writable",
    )
}

fn open_checked(
    dir: BorrowedFd<'_>,
    name: &str,
    create: bool,
    group_other_mask: u32,
    expectation: &str,
) -> Result<OwnedFd, String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(format!("{name:?} is not one directory name"));
    }
    let text = std::ffi::CString::new(name).map_err(|_| "a name contains NUL".to_owned())?;
    if create {
        // SAFETY: valid descriptor and NUL-terminated name.
        if unsafe { libc::mkdirat(dir.as_raw_fd(), text.as_ptr(), 0o700) } == 0 {
            // SAFETY: valid descriptor; fsync has no other precondition.
            unsafe { libc::fsync(dir.as_raw_fd()) };
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EEXIST) {
                return Err(format!("cannot create {name}: {error}"));
            }
        }
    }
    let fd = open_dir(Some(dir), Path::new(name))?;
    let parent = fstat(dir)?;
    let child = fstat(fd.as_fd())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if child.st_uid != uid || u32::from(child.st_mode) & group_other_mask != 0 {
        return Err(format!("{name} must be {expectation}"));
    }
    if child.st_dev != parent.st_dev {
        return Err(format!("{name} is on another device"));
    }
    Ok(fd)
}

fn c_path(path: &Path) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{} contains NUL", path.display()))
}

fn open_dir(dir: Option<BorrowedFd<'_>>, path: &Path) -> Result<OwnedFd, String> {
    let text = c_path(path)?;
    let at = dir.map_or(libc::AT_FDCWD, |dir| dir.as_raw_fd());
    // SAFETY: NUL-terminated path; the flags open a directory read-only
    // without following a final symlink.
    let raw = unsafe {
        libc::openat(
            at,
            text.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return Err(format!(
            "cannot open {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: openat returned a new descriptor this function owns.
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

fn fstat(fd: BorrowedFd<'_>) -> Result<libc::stat, String> {
    // SAFETY: zeroed stat is valid output storage; the descriptor is open.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(format!("fstat failed: {}", std::io::Error::last_os_error()));
    }
    Ok(stat)
}

#[cfg(target_os = "macos")]
fn c_text(field: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "macos")]
fn observe(fd: BorrowedFd<'_>) -> Result<Facts, String> {
    // SAFETY: zeroed statfs is valid output storage; the descriptor is open.
    let mut sample: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatfs(fd.as_raw_fd(), &mut sample) } != 0 {
        return Err(format!(
            "fstatfs failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let block = u64::from(sample.f_bsize);
    let stat = fstat(fd)?;
    Ok(Facts {
        mounted_on: PathBuf::from(c_text(&sample.f_mntonname)),
        device: c_text(&sample.f_mntfromname),
        fstype: c_text(&sample.f_fstypename),
        read_only: sample.f_flags & libc::MNT_RDONLY as u32 != 0,
        root_fs: sample.f_flags & libc::MNT_ROOTFS as u32 != 0,
        uuid: volume_uuid(fd),
        dev: u64::try_from(stat.st_dev).unwrap_or(u64::MAX),
        capacity_bytes: sample.f_blocks.checked_mul(block),
        available_bytes: sample.f_bavail.checked_mul(block),
    })
}

/// `ATTR_VOL_UUID` of the volume holding `fd`'s directory, if reported. Not
/// a mount-root proof: see the module notes.
#[cfg(target_os = "macos")]
fn volume_uuid(fd: BorrowedFd<'_>) -> Option<String> {
    #[repr(C, align(4))]
    struct Reply {
        length: u32,
        uuid: [u8; 16],
    }
    let mut request = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut reply = Reply {
        length: 0,
        uuid: [0; 16],
    };
    // SAFETY: request and reply are valid for the sizes passed.
    let status = unsafe {
        libc::fgetattrlist(
            fd.as_raw_fd(),
            std::ptr::addr_of_mut!(request).cast(),
            std::ptr::addr_of_mut!(reply).cast(),
            std::mem::size_of::<Reply>(),
            0,
        )
    };
    if status != 0 || reply.length as usize != std::mem::size_of::<Reply>() || reply.uuid == [0; 16]
    {
        return None;
    }
    let hex: String = reply
        .uuid
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

#[cfg(target_os = "macos")]
fn flush_volume(fd: &OwnedFd) -> Result<(), String> {
    // unistd.h: SYNC_VOLUME_FULLSYNC 0x01, SYNC_VOLUME_WAIT 0x02.
    const SYNC_VOLUME_FULLSYNC: libc::c_int = 0x01;
    const SYNC_VOLUME_WAIT: libc::c_int = 0x02;
    extern "C" {
        fn fsync_volume_np(fd: libc::c_int, flags: libc::c_int) -> libc::c_int;
    }
    #[cfg(debug_assertions)]
    if std::env::var_os("OXIDE_TEST_FAIL_FLUSH").is_some_and(|value| value == "1") {
        return Err("injected flush failure (OXIDE_TEST_FAIL_FLUSH)".to_owned());
    }
    // SAFETY: the descriptor is open; the flags are the documented ones.
    let status =
        unsafe { fsync_volume_np(fd.as_raw_fd(), SYNC_VOLUME_FULLSYNC | SYNC_VOLUME_WAIT) };
    if status == 0 {
        Ok(())
    } else {
        // fsync_volume_np(3): "a non-zero error code on error".
        let error = std::io::Error::from_raw_os_error(status);
        Err(format!("fsync_volume_np failed: {error}"))
    }
}

#[cfg(not(target_os = "macos"))]
fn observe(_fd: BorrowedFd<'_>) -> Result<Facts, String> {
    Err("the apfs-volume-v1 backend is macOS-only".to_owned())
}

#[cfg(not(target_os = "macos"))]
fn flush_volume(_fd: &OwnedFd) -> Result<(), String> {
    Err("the apfs-volume-v1 backend is macOS-only".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn facts(mounted_on: &str, uuid: Option<&str>) -> Facts {
        Facts {
            mounted_on: PathBuf::from(mounted_on),
            device: "/dev/disk12s1".to_owned(),
            fstype: "apfs".to_owned(),
            read_only: false,
            root_fs: false,
            uuid: uuid.map(str::to_owned),
            dev: 7,
            capacity_bytes: Some(1 << 30),
            available_bytes: Some(1 << 29),
        }
    }

    const UUID: &str = "11111111-2222-3333-4444-555555555555";

    #[test]
    fn mount_name_accepts_only_direct_children_of_volumes() {
        assert_eq!(
            mount_name(Path::new("/Volumes/OxideBuild")),
            Ok("OxideBuild")
        );
        for refused in [
            "/Volumes",
            "/Volumes/",
            "/Volumes/a/b",
            "/Volumes/../tmp",
            "/Volumes/.hidden",
            "/VolumesX/a",
            "/private/Volumes/a",
            "Volumes/a",
            "/Volumes/a/",
            "/Volumes/./a",
        ] {
            assert!(mount_name(Path::new(refused)).is_err(), "{refused}");
        }
    }

    /// A volume UUID is case-insensitive: the platform and a registration may
    /// report the same identity in different case, and that is not a mismatch.
    #[test]
    fn judge_compares_volume_uuids_case_insensitively() {
        let path = Path::new("/Volumes/OxideBuild");
        let lower = UUID.to_ascii_lowercase();
        assert!(judge(
            path,
            &facts("/Volumes/OxideBuild", Some(&lower)),
            Some(UUID)
        )
        .is_ok());
        assert!(judge(
            path,
            &facts("/Volumes/OxideBuild", Some(UUID)),
            Some(&lower)
        )
        .is_ok());
        let other = "99999999-2222-3333-4444-555555555555";
        let swapped =
            judge(path, &facts("/Volumes/OxideBuild", Some(other)), Some(UUID)).unwrap_err();
        assert!(swapped.contains("not the registered volume"), "{swapped}");
    }

    #[test]
    fn judge_refuses_stale_directories_and_other_devices() {
        let path = Path::new("/Volumes/OxideBuild");
        assert!(judge(path, &facts("/Volumes/OxideBuild", Some(UUID)), Some(UUID)).is_ok());
        // A stale mount point is a directory on the internal data volume.
        let stale = judge(path, &facts("/System/Volumes/Data", None), Some(UUID)).unwrap_err();
        assert!(stale.contains("stale mount point"), "{stale}");
        // Another volume mounted under the registered name.
        let other = "99999999-2222-3333-4444-555555555555";
        let swapped =
            judge(path, &facts("/Volumes/OxideBuild", Some(other)), Some(UUID)).unwrap_err();
        assert!(swapped.contains("not the registered volume"), "{swapped}");
        let mut read_only = facts("/Volumes/OxideBuild", Some(UUID));
        read_only.read_only = true;
        assert!(judge(path, &read_only, Some(UUID)).is_err());
        let mut hfs = facts("/Volumes/OxideBuild", Some(UUID));
        hfs.fstype = "hfs".to_owned();
        assert!(judge(path, &hfs, Some(UUID)).is_err());
        let mut boot = facts("/Volumes/OxideBuild", Some(UUID));
        boot.root_fs = true;
        assert!(judge(path, &boot, Some(UUID)).is_err());
    }

    fn scratch(label: &str) -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "oxide-volume-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    /// A plain directory standing in for a stale `/Volumes/<name>`: real
    /// kernel facts refuse it and nothing is written into it.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stale_directory_is_refused_without_writes() {
        let parent = scratch("stale");
        let stale = parent.join("OxideBuild");
        std::fs::create_dir(&stale).unwrap();
        let error = Binding::open_in(&parent, "OxideBuild", Some(UUID)).unwrap_err();
        assert!(error.contains("not a mounted volume root"), "{error}");
        assert_eq!(std::fs::read_dir(&stale).unwrap().count(), 0);
        let observed = observe_path(&stale).unwrap();
        assert_ne!(observed.mounted_on, stale);
        // The descriptor still reports the containing volume's UUID, which
        // is why the UUID alone never proves a mount root.
        assert!(observed.uuid.is_some());
        // Even with the stale directory's own UUID expected, it refuses.
        let own = observed.uuid.as_deref();
        let error = Binding::open_in(&parent, "OxideBuild", own).unwrap_err();
        assert!(error.contains("not a mounted volume root"), "{error}");
        assert_eq!(std::fs::read_dir(&stale).unwrap().count(), 0);
        std::fs::remove_dir_all(parent).unwrap();
    }

    /// Check/use race: after a directory is opened, replacing its path with
    /// another directory does not redirect creation through the
    /// descriptor; nothing lands in the replacement.
    #[test]
    fn creation_through_a_descriptor_ignores_a_swapped_path() {
        let parent = scratch("swap");
        let bound = parent.join("bound");
        std::fs::create_dir(&bound).unwrap();
        std::fs::set_permissions(&bound, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let fd = open_dir(None, &bound).unwrap();
        let moved = parent.join("moved");
        std::fs::rename(&bound, &moved).unwrap();
        std::fs::create_dir(&bound).unwrap();
        let created = dir_at(fd.as_fd(), "slots", true).unwrap();
        let inner = fstat(created.as_fd()).unwrap();
        assert!(moved.join("slots").is_dir());
        assert!(!bound.join("slots").exists());
        assert_eq!(
            u64::try_from(inner.st_ino).unwrap(),
            std::fs::metadata(moved.join("slots")).unwrap().ino()
        );
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn dir_at_refuses_links_and_traversal() {
        let parent = scratch("links");
        let fd = open_dir(None, &parent).unwrap();
        std::os::unix::fs::symlink("/tmp", parent.join("link")).unwrap();
        assert!(dir_at(fd.as_fd(), "link", false).is_err());
        assert!(dir_at(fd.as_fd(), "..", true).is_err());
        assert!(dir_at(fd.as_fd(), "a/b", true).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn relative_paths_are_clean_and_bounded() {
        for ok in ["oxide-pool", "builds/oxide/pool", "a/b/c/d/e/f/g/h"] {
            assert!(relative_components(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            relative_components("builds/oxide/pool").unwrap(),
            vec!["builds", "oxide", "pool"]
        );
        for bad in [
            "",
            "/abs",
            "../escape",
            "a/../b",
            "a/./b",
            "a//b",
            "a/",
            ".",
            "..",
            "a\0b",
            "a/b/c/d/e/f/g/h/i",
        ] {
            assert!(relative_components(bad).is_err(), "{bad:?} must refuse");
        }
    }

    #[test]
    fn shared_readable_parents_pass_but_writable_ones_and_loose_leaves_refuse() {
        use std::os::unix::fs::PermissionsExt;
        let parent = scratch("shared-parents");
        let fd = open_dir(None, &parent).unwrap();
        std::fs::create_dir(parent.join("builds")).unwrap();
        std::fs::set_permissions(parent.join("builds"), PermissionsExt::from_mode(0o755)).unwrap();
        // A user-owned `0755` intermediate is accepted; missing components
        // below it are created owner-only.
        walk_dir_path(fd.as_fd(), "builds/oxide/pool", true).unwrap();
        assert!(parent.join("builds/oxide/pool").is_dir());
        assert_eq!(
            std::fs::metadata(parent.join("builds/oxide/pool"))
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
        // Reopening without creation also accepts the `0755` parent.
        walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap();
        walk_parent_dir_path(fd.as_fd(), "builds", false).unwrap();
        walk_parent_dir_path(fd.as_fd(), "builds/oxide", false).unwrap();
        // The leaf itself must stay owner-only: `0755` refuses.
        std::fs::set_permissions(
            parent.join("builds/oxide/pool"),
            PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let error = walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap_err();
        assert!(error.contains("owner-only"), "{error}");
        std::fs::set_permissions(
            parent.join("builds/oxide/pool"),
            PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        // A group-writable intermediate refuses, even with a private leaf.
        std::fs::set_permissions(parent.join("builds"), PermissionsExt::from_mode(0o775)).unwrap();
        let error = walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap_err();
        assert!(error.contains("not group- or world-writable"), "{error}");
        let error = walk_parent_dir_path(fd.as_fd(), "builds", false).unwrap_err();
        assert!(error.contains("not group- or world-writable"), "{error}");
        // So does a world-writable one.
        std::fs::set_permissions(parent.join("builds"), PermissionsExt::from_mode(0o777)).unwrap();
        let error = walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap_err();
        assert!(error.contains("not group- or world-writable"), "{error}");
        // Restoring `0755` accepts the whole path again.
        std::fs::set_permissions(parent.join("builds"), PermissionsExt::from_mode(0o755)).unwrap();
        walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn nested_directory_path_walks_each_component_without_following_links() {
        let parent = scratch("nested");
        let fd = open_dir(None, &parent).unwrap();
        walk_dir_path(fd.as_fd(), "builds/oxide/pool", true).unwrap();
        assert!(parent.join("builds/oxide/pool").is_dir());
        walk_dir_path(fd.as_fd(), "builds/oxide/pool", false).unwrap();
        assert!(walk_dir_path(fd.as_fd(), "builds/missing", false).is_err());
        assert!(walk_dir_path(fd.as_fd(), "../escape", true).is_err());
        let target = parent.join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, parent.join("linked")).unwrap();
        assert!(walk_dir_path(fd.as_fd(), "linked/nested", true).is_err());
        assert!(!target.join("nested").exists());
        std::fs::remove_dir_all(parent).unwrap();
    }
}
