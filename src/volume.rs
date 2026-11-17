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
        if uuid != expected {
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

/// Test-only *modelled* verified-volume facts.
///
/// Unit tests cannot mount an APFS volume, so this adapter models the outcome
/// of a successful (or failed) verification rather than exercising it: when it
/// is installed, `Binding::verify()` returns these fabricated facts and
/// therefore **skips** `observe()`, the mount-root rule, the device comparison
/// and the mount-point path re-check for that call. Production has no such
/// path at all — every production `verify()` runs those checks against a real
/// descriptor — and this module is compiled only under `cfg(test)` with no
/// environment, public or debug-build entry point. It is never evidence that a
/// real volume, mount or device was verified.
#[cfg(test)]
pub(crate) mod verified_facts {
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Clone, Debug, Default)]
    pub(crate) struct VolumeFacts {
        /// Substitute for the volume's reported capacity.
        pub capacity_bytes: Option<u64>,
        /// Substitute for the volume's reported available bytes.
        pub available_bytes: Option<u64>,
        /// Model a volume that fails verification outright.
        pub verify_error: Option<String>,
        /// Successive available-byte readings, consumed one per verification,
        /// so a test can model a re-read after a prune. When exhausted the
        /// last value repeats.
        pub available_sequence: Vec<Option<u64>>,
        /// Fail verification once this many successful readings have happened.
        pub verify_error_after: Option<u64>,
        /// How many substituted verifications have happened (test sequencing).
        pub reads: u64,
    }

    thread_local! {
        static CURRENT: RefCell<Option<VolumeFacts>> = const { RefCell::new(None) };
    }

    /// Run `body` with these facts installed on this thread.
    pub(crate) fn with<R>(facts: VolumeFacts, body: impl FnOnce() -> R) -> R {
        let previous = CURRENT.with(|cell| cell.replace(Some(facts)));
        let result = body();
        CURRENT.with(|cell| cell.replace(previous));
        result
    }

    /// Whether a caller in this thread installed modelled facts.
    pub(crate) fn armed() -> bool {
        CURRENT.with(|cell| cell.borrow().is_some())
    }

    /// Replace the available bytes mid-run, as a re-read after a prune would.
    pub(crate) fn set_available(bytes: Option<u64>) {
        CURRENT.with(|cell| {
            if let Some(facts) = cell.borrow_mut().as_mut() {
                facts.available_bytes = bytes;
            }
        });
    }

    pub(crate) fn reads() -> u64 {
        CURRENT.with(|cell| cell.borrow().as_ref().map_or(0, |facts| facts.reads))
    }

    /// The modelled verification result, or `None` when no adapter is set.
    pub(super) fn modelled() -> Result<Option<super::Facts>, String> {
        let current = CURRENT.with(|cell| cell.borrow().clone());
        let Some(current) = current else {
            return Ok(None);
        };
        if let Some(error) = current.verify_error {
            return Err(error);
        }
        let read = CURRENT.with(|cell| {
            let mut borrowed = cell.borrow_mut();
            let installed = borrowed.as_mut().expect("adapter installed");
            installed.reads += 1;
            installed.reads
        });
        let available = CURRENT.with(|cell| {
            let mut borrowed = cell.borrow_mut();
            let installed = borrowed.as_mut().expect("adapter installed");
            if installed.available_sequence.is_empty() {
                return installed.available_bytes;
            }
            let index = (read as usize - 1).min(installed.available_sequence.len() - 1);
            installed.available_sequence[index]
        });
        if current.verify_error_after.is_some_and(|limit| read > limit) {
            return Err("the fixture volume stopped verifying".to_owned());
        }
        let mut facts = super::Facts {
            mounted_on: PathBuf::new(),
            device: String::new(),
            fstype: String::new(),
            read_only: false,
            root_fs: false,
            uuid: None,
            dev: 0,
            capacity_bytes: current.capacity_bytes,
            available_bytes: available,
        };
        facts.available_bytes = available;
        Ok(Some(facts))
    }
}

/// Build a `Binding` over an owned fixture directory.
///
/// Test-only: the descriptor really opens the fixture directory, but the
/// binding is only meaningful together with [`verified_facts`], which models
/// the verification result; without that adapter `verify()` would (correctly)
/// reject the directory as not being a mount root. Production never constructs
/// a binding this way.
#[cfg(test)]
pub(crate) fn binding_for_test(root: &Path, uuid: &str) -> Result<Binding, String> {
    let fd = open_dir(None, root)?;
    Ok(Binding {
        fd,
        mount_point: root.to_path_buf(),
        uuid: uuid.to_owned(),
        device: "fixture".to_owned(),
    })
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
        let fd = open_dir(Some(parent_fd.as_fd_ref()), Path::new(name))
            .map_err(|error| format!("{} is not available: {error}", mount_point.display()))?;
        let facts = observe(fd.as_fd())?;
        judge(&mount_point, &facts, expected_uuid)?;
        if u64::try_from(fstat(parent_fd.as_fd_ref())?.st_dev).unwrap_or(u64::MAX) == facts.dev {
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
        self.fd.as_fd_ref()
    }

    /// Recheck the held descriptor and the path: the descriptor is still the
    /// same volume's root, and the path still resolves to that root rather
    /// than a directory left behind after an unmount.
    ///
    /// Under `cfg(test)` only, an installed [`verified_facts`] adapter returns
    /// modelled facts instead and the structural checks below are skipped for
    /// that call; production always runs them.
    pub fn verify(&self) -> Result<Facts, String> {
        #[cfg(test)]
        if let Some(modelled) = verified_facts::modelled()? {
            return Ok(modelled);
        }
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
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EACCES | libc::EPERM | libc::EROFS) => Ok(false),
        _ => Err(format!(
            "cannot check write access to {}: {}",
            path.display(),
            std::io::Error::last_os_error()
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
    let child = fstat(fd.as_fd_ref())?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if child.st_uid != uid || u32::from(child.st_mode) & 0o077 != 0 {
        return Err(format!(
            "{name} must be an owner-only directory of this user"
        ));
    }
    if child.st_dev != parent.st_dev {
        return Err(format!("{name} is on another device"));
    }
    Ok(fd)
}

trait AsFdRef {
    fn as_fd_ref(&self) -> BorrowedFd<'_>;
}

impl AsFdRef for OwnedFd {
    fn as_fd_ref(&self) -> BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(self)
    }
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
        let created = dir_at(fd.as_fd_ref(), "slots", true).unwrap();
        let inner = fstat(created.as_fd_ref()).unwrap();
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
        assert!(dir_at(fd.as_fd_ref(), "link", false).is_err());
        assert!(dir_at(fd.as_fd_ref(), "..", true).is_err());
        assert!(dir_at(fd.as_fd_ref(), "a/b", true).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }

}
