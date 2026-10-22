//! Removable build volume, part 1: registration, selection when a build
//! starts, and status. Flush before a slot goes idle lives in the store.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Removable build
//! volume). The registration, the removable pool's config, ledger, lock and
//! Oxide state all stay in the internal pool's control directory; only slot
//! payload lives on the volume, under [`PAYLOAD_DIR`]. Identity comes from
//! the kernel ([`Binding`]) and, at registration only, from `diskutil`
//! (`diskutil info -plist`, `diskutil apfs list -plist <container>`) for the
//! container UUID, APFS roles and quota.

use crate::config::{self, Facts};
use crate::process;
use crate::store::{self, Backend, Store};
use crate::MIB;
use oxide_storage::lease::managed::init_volume_pool;
use oxide_storage::volume::{self, Binding};
use serde::{Deserialize, Serialize};
use std::ffi::{CString, OsString};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Registration file in the internal control directory.
pub(crate) const REGISTRATION: &str = "removable.json";
/// The removable pool's internal root (config, ledger, lock, Oxide state).
pub const POOL_DIR: &str = "removable";
/// The payload directory directly under the volume's mount root.
pub const PAYLOAD_DIR: &str = "oxide-pool";
const LEGACY_REGISTRATION_VERSION: u32 = 1;
pub(crate) const REGISTRATION_VERSION: u32 = 2;
pub(crate) const MAX_REGISTRATION_BYTES: u64 = 16 * 1024;
pub const MAX_PLIST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriveMode {
    Ephemeral,
    Permanent,
}

impl DriveMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "ephemeral" => Ok(Self::Ephemeral),
            "permanent" => Ok(Self::Permanent),
            _ => Err(format!(
                "unknown drive mode {value:?}; use ephemeral or permanent"
            )),
        }
    }
}

impl std::fmt::Display for DriveMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Ephemeral => "ephemeral",
            Self::Permanent => "permanent",
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub version: u32,
    pub mount_point: PathBuf,
    pub volume_uuid: String,
    pub container_uuid: String,
    /// Selection refuses the volume when its available space is at or below
    /// this. Chosen by the operator; Oxide never picks it.
    pub reserve_bytes: u64,
    /// Oxide's admission budget on the volume. Not an APFS quota.
    pub cap_bytes: u64,
    /// The volume's APFS quota at registration; `None` when it has none.
    pub quota_bytes: Option<u64>,
    /// The volume's physical capacity at registration, shared with every
    /// other directory on it.
    pub capacity_bytes: Option<u64>,
    pub registered_ms: u64,
    pub mode: DriveMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRegistration {
    version: u32,
    mount_point: PathBuf,
    volume_uuid: String,
    container_uuid: String,
    reserve_bytes: u64,
    cap_bytes: u64,
    quota_bytes: Option<u64>,
    capacity_bytes: Option<u64>,
    registered_ms: u64,
}

impl From<LegacyRegistration> for Registration {
    fn from(registration: LegacyRegistration) -> Self {
        Self {
            version: registration.version,
            mount_point: registration.mount_point,
            volume_uuid: registration.volume_uuid,
            container_uuid: registration.container_uuid,
            reserve_bytes: registration.reserve_bytes,
            cap_bytes: registration.cap_bytes,
            quota_bytes: registration.quota_bytes,
            capacity_bytes: registration.capacity_bytes,
            registered_ms: registration.registered_ms,
            mode: DriveMode::Ephemeral,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredRegistration {
    Legacy(LegacyRegistration),
    Current(Registration),
}

/// Which pool a build uses, and why.
pub struct Selected {
    pub store: Store,
    /// `None` when nothing is registered: the internal pool, unchanged.
    pub note: Option<String>,
    pub removable: bool,
    pub mode: Option<DriveMode>,
    /// The reason this command selected the internal pool, from the same
    /// registration snapshot as `mode`.
    pub fallback_reason: Option<String>,
}

const USAGE: &str = "\
usage: oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ephemeral|permanent]
       oxide pool set-mode <ephemeral|permanent>
       oxide pool recover-interrupted <slot>
       oxide pool describe --json

register-removable registers a dedicated APFS volume mounted directly under
/Volumes as the preferred build pool. The default mode is ephemeral. <size> is bytes or a whole number of
KB, MB, GB, TB, KiB, MiB, GiB or TiB. --reserve is the free space a build
start must find above it; --cap is Oxide's admission budget on the volume.
Both are required.

set-mode updates the existing removable registration only. Permanent mode
warns when a build falls back to the internal pool; ephemeral mode keeps the
existing fallback diagnostic.

recover-interrupted releases a slot `oxide status` shows held by an
interrupted command whose Cargo group was not recorded or whose lease is not
provably its own. Run it only when nothing still writes in that slot.

describe --json prints the registered drive as stable, read-only JSON on
stdout (schema oxide.pool.describe/1). It changes nothing.";

/// `oxide pool ...`
pub fn run(args: &[OsString]) -> Result<u8, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        ["describe", "--json"] => crate::describe::print_json(),
        ["describe", ..] => Err(USAGE.to_owned()),
        ["register-removable", rest @ ..] => {
            let mut mount = None;
            let mut reserve = None;
            let mut cap = None;
            let mut mode = None;
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match *arg {
                    "--reserve" => reserve = Some(*rest.next().ok_or("--reserve needs a size")?),
                    "--cap" => cap = Some(*rest.next().ok_or("--cap needs a size")?),
                    "--mode" => {
                        mode = Some(DriveMode::parse(
                            rest.next().ok_or("--mode needs ephemeral or permanent")?,
                        )?)
                    }
                    other if other.starts_with('-') => {
                        return Err(format!("unknown option {other}\n{USAGE}"))
                    }
                    other if mount.is_none() => mount = Some(other),
                    _ => return Err(USAGE.to_owned()),
                }
            }
            let (Some(mount), Some(reserve), Some(cap)) = (mount, reserve, cap) else {
                return Err(USAGE.to_owned());
            };
            let reserve = size("--reserve", reserve)?;
            let cap = size("--cap", cap)?;
            register(
                Path::new(mount),
                reserve,
                cap,
                mode.unwrap_or(DriveMode::Ephemeral),
            )
        }
        ["set-mode", mode] => set_mode(DriveMode::parse(mode)?),
        ["set-mode", ..] => Err(USAGE.to_owned()),
        ["recover-interrupted", slot] => {
            let Some(internal) = Store::open_existing()? else {
                return Err("no pool exists".to_owned());
            };
            let target = match registration(&internal)? {
                Some(_) if !internal_holds(&internal, slot)? => Store::open_removable(
                    &internal.control().join(POOL_DIR),
                    registration(&internal)?
                        .map(|registration| registration.mount_point.join(PAYLOAD_DIR))
                        .ok_or("the registration disappeared")?,
                    internal.max_active(),
                )?,
                _ => internal,
            };
            target.recover_interrupted(slot)?;
            println!("released slot {slot} in {}", target.pool.display());
            Ok(0)
        }
        // Debug builds only: the focused proof applies the real mount-parent
        // rule to an owned scratch directory.
        #[cfg(debug_assertions)]
        ["debug-check-parent", parent] => match volume::refuse_writable_parent(Path::new(parent)) {
            Ok(()) => {
                println!("parent accepted: {parent}");
                Ok(0)
            }
            Err(reason) => {
                println!("parent refused: {reason}");
                Ok(1)
            }
        },
        ["-h" | "--help"] | [] => {
            println!("{USAGE}");
            Ok(0)
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn size(flag: &str, value: &str) -> Result<u64, String> {
    store::parse_cap(value, Backend::ApfsVolume)
        .map_err(|error| error.replace("OXIDE_POOL_CAP", flag))
}

fn register(mount: &Path, reserve: u64, cap: u64, mode: DriveMode) -> Result<u8, String> {
    if !cfg!(target_os = "macos") {
        return Err("the removable apfs-volume-v1 backend is macOS-only".to_owned());
    }
    // Every check before the first write.
    let binding = Binding::open(mount, None)?;
    if !volume::writable(binding.mount_point())? {
        return Err(format!("{} is not writable", mount.display()));
    }
    let identity = disk_identity(binding.mount_point(), binding.uuid(), &plist)?;
    check_identity(&identity, None)?;
    let internal = Store::open_or_create()?;
    let control = internal.control().to_path_buf();
    let _admission = store::lock_admission(&control)?;
    if let Some(previous) = previous_registration(&internal) {
        return Err(previous_registration_refusal(&previous));
    }
    if read_registration(&control)?.is_some()
        || fs::symlink_metadata(control.join(POOL_DIR)).is_ok()
    {
        return Err(format!(
            "a removable pool is already registered for {}",
            internal.pool.display()
        ));
    }
    let facts = binding.verify()?;
    init_volume_pool(&control.join(POOL_DIR), &binding, PAYLOAD_DIR, cap)
        .map_err(|error| format!("cannot create the removable pool: {error}"))?;
    let registration = Registration {
        version: REGISTRATION_VERSION,
        mount_point: binding.mount_point().to_path_buf(),
        volume_uuid: binding.uuid().to_owned(),
        container_uuid: identity.container_uuid,
        reserve_bytes: reserve,
        cap_bytes: cap,
        quota_bytes: identity.quota_bytes,
        capacity_bytes: facts.capacity_bytes,
        registered_ms: process::now_ms(),
        mode,
    };
    write_registration(&control, &registration)?;
    println!(
        "registered removable pool on {} (volume {}, container {}): reserve {} MiB, admission cap {} MiB; payload {}/{PAYLOAD_DIR}",
        registration.mount_point.display(),
        registration.volume_uuid,
        registration.container_uuid,
        reserve / MIB,
        cap / MIB,
        registration.mount_point.display()
    );
    Ok(0)
}

fn set_mode(mode: DriveMode) -> Result<u8, String> {
    let Some(control) = Store::existing_control()? else {
        return Err("no pool exists".to_owned());
    };
    set_mode_in_control(&control, mode)?;
    println!("removable pool mode set to {mode}");
    Ok(0)
}

fn set_mode_in_control(control: &Path, mode: DriveMode) -> Result<(), String> {
    let _admission = store::lock_admission(control)?;
    let mut registration =
        read_registration(control)?.ok_or_else(|| "no removable pool is registered".to_owned())?;
    registration.version = REGISTRATION_VERSION;
    registration.mode = mode;
    write_registration(control, &registration)
}

/// What `diskutil` reports about a mounted volume, as checked at
/// registration and again whenever selection considers it.
pub struct DiskIdentity {
    pub container_uuid: String,
    container_ref: String,
    /// The container holding `/System/Volumes/Data`.
    data_container_ref: String,
    roles: Vec<String>,
    quota_bytes: Option<u64>,
}

/// `read` runs one `diskutil` query as JSON: the build path's [`plist`], or
/// `oxide pool describe`'s bounded reader.
pub fn disk_identity(
    mount: &Path,
    uuid: &str,
    read: &dyn Fn(&[&str]) -> Result<serde_json::Value, String>,
) -> Result<DiskIdentity, String> {
    let info = read(&["info", "-plist", path_str(mount)?])?;
    let field = |key: &str| info.get(key).and_then(serde_json::Value::as_str);
    if field("VolumeUUID") != Some(uuid) {
        return Err(format!(
            "diskutil reports volume {:?} at {}, not the kernel's {uuid}",
            field("VolumeUUID"),
            mount.display()
        ));
    }
    if field("MountPoint") != Some(path_str(mount)?) || field("FilesystemType") != Some("apfs") {
        return Err(format!(
            "diskutil does not report {} as a mounted APFS volume",
            mount.display()
        ));
    }
    let container_ref = field("APFSContainerReference")
        .ok_or_else(|| format!("diskutil reports no APFS container for {}", mount.display()))?
        .to_owned();
    let data = read(&["info", "-plist", "/System/Volumes/Data"])?;
    let data_container_ref = data
        .get("APFSContainerReference")
        .and_then(serde_json::Value::as_str)
        .ok_or("diskutil reports no container for /System/Volumes/Data")?
        .to_owned();
    let list = read(&["apfs", "list", "-plist", &container_ref])?;
    let entry = list["Containers"]
        .as_array()
        .and_then(|containers| containers.first())
        .ok_or("diskutil listed no container")?;
    let container_uuid = entry["APFSContainerUUID"]
        .as_str()
        .ok_or("diskutil reports no container UUID")?
        .to_owned();
    let volume = entry["Volumes"]
        .as_array()
        .and_then(|volumes| {
            volumes
                .iter()
                .find(|volume| volume["APFSVolumeUUID"] == uuid)
        })
        .ok_or("diskutil does not list the volume in its container")?;
    let roles = volume["Roles"]
        .as_array()
        .ok_or("diskutil reports no role list for the volume")?
        .iter()
        .map(|role| role.as_str().map(str::to_owned).ok_or("a role is not text"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DiskIdentity {
        container_uuid,
        container_ref,
        data_container_ref,
        roles,
        quota_bytes: volume["CapacityQuota"].as_u64().filter(|quota| *quota > 0),
    })
}

/// The boot/data container and every APFS role refuse; at selection the
/// container must also be the registered one.
pub fn check_identity(identity: &DiskIdentity, registered: Option<&str>) -> Result<(), String> {
    if identity.container_ref == identity.data_container_ref {
        return Err("the volume is in the internal boot/data container".to_owned());
    }
    let roles: Vec<&str> = identity.roles.iter().map(String::as_str).collect();
    refuse_roles(&roles)?;
    if let Some(registered) = registered {
        if identity.container_uuid != registered {
            return Err(format!(
                "the volume is in container {}, not the registered {registered}",
                identity.container_uuid
            ));
        }
    }
    Ok(())
}

/// Only a plain data volume with no APFS role is accepted: a Time Machine
/// Backup volume, the system, data, preboot, recovery, VM and every other
/// role refuse.
fn refuse_roles(roles: &[&str]) -> Result<(), String> {
    if roles.contains(&"Backup") {
        return Err("the volume has the APFS Backup (Time Machine) role".to_owned());
    }
    if !roles.is_empty() {
        return Err(format!(
            "the volume has APFS role {}; only a volume without a role is accepted",
            roles.join(", ")
        ));
    }
    Ok(())
}

/// `diskutil <args>` as JSON, via `plutil`. Bounded output.
fn plist(args: &[&str]) -> Result<serde_json::Value, String> {
    let output = Command::new("/usr/sbin/diskutil")
        .args(args)
        .output()
        .map_err(|error| format!("cannot run diskutil: {error}"))?;
    if !output.status.success() || output.stdout.len() > MAX_PLIST_BYTES {
        return Err(format!(
            "diskutil {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut convert = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run plutil: {error}"))?;
    convert
        .stdin
        .take()
        .ok_or("plutil stdin")?
        .write_all(&output.stdout)
        .map_err(|error| format!("cannot feed plutil: {error}"))?;
    let json = convert
        .wait_with_output()
        .map_err(|error| format!("plutil failed: {error}"))?;
    if !json.status.success() || json.stdout.len() > MAX_PLIST_BYTES {
        return Err("plutil could not convert diskutil's output".to_owned());
    }
    serde_json::from_slice(&json.stdout).map_err(|error| format!("diskutil output: {error}"))
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "the mount point must be valid UTF-8".to_owned())
}

fn write_registration(control: &Path, registration: &Registration) -> Result<(), String> {
    let temp_name = format!(
        "{REGISTRATION}.tmp.{}.{}.{}",
        std::process::id(),
        process::now_ms(),
        std::sync::atomic::AtomicU64::fetch_add(
            &REGISTRATION_TEMP_SEQUENCE,
            1,
            std::sync::atomic::Ordering::Relaxed,
        )
    );
    write_registration_with_temp_name(control, registration, &temp_name)
}

static REGISTRATION_TEMP_SEQUENCE: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

fn write_registration_with_temp_name(
    control: &Path,
    registration: &Registration,
    temp_name: &str,
) -> Result<(), String> {
    if temp_name.is_empty() || temp_name.contains('/') || temp_name.len() > 240 {
        return Err("the registration temporary name is invalid".to_owned());
    }
    let mut current = registration.clone();
    current.version = REGISTRATION_VERSION;
    let bytes = serde_json::to_vec_pretty(&current)
        .map_err(|error| format!("cannot encode the removable registration: {error}"))?;
    if bytes.len() as u64 > MAX_REGISTRATION_BYTES {
        return Err("the removable registration exceeds its byte limit".to_owned());
    }
    let directory = store::open_control_directory(control)?
        .ok_or_else(|| format!("{} does not exist", control.display()))?;
    let temp = CString::new(temp_name).map_err(|error| error.to_string())?;
    let destination = CString::new(REGISTRATION).map_err(|error| error.to_string())?;
    validate_registration_destination(&directory, &destination)?;
    // SAFETY: `temp` is a live CString with initialized bytes through its
    // terminator, and `directory` owns a live descriptor for this call.
    // O_EXCL and O_NOFOLLOW make temporary creation exclusive and owned.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(format!(
            "cannot create the registration temporary file: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `fd` is a newly opened descriptor owned by this function.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let write_result = file.write_all(&bytes).and_then(|()| file.sync_all());
    if let Err(error) = write_result {
        drop(file);
        return Err(with_temp_cleanup(
            &directory,
            &temp,
            format!("cannot sync the removable registration: {error}"),
        ));
    }
    drop(file);
    if let Err(error) = validate_registration_destination(&directory, &destination) {
        return Err(with_temp_cleanup(&directory, &temp, error));
    }
    // SAFETY: both live CStrings contain initialized bytes through their
    // terminators, and `directory` owns a live descriptor. Both paths are
    // relative to the same already-open private control directory.
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temp.as_ptr(),
            directory.as_raw_fd(),
            destination.as_ptr(),
        )
    } != 0
    {
        let error = format!(
            "cannot publish the removable registration: {}",
            std::io::Error::last_os_error()
        );
        return Err(with_temp_cleanup(&directory, &temp, error));
    }
    directory.sync_all().map_err(|error| {
        format!(
            "the registration was published, but syncing its directory failed; durability is uncertain: {error}"
        )
    })
}

fn validate_registration_destination(
    directory: &File,
    destination: &CString,
) -> Result<(), String> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `destination` is a live CString with initialized bytes through
    // its terminator; `directory` owns a live descriptor; and `metadata` is
    // aligned writable storage for one libc::stat. AT_SYMLINK_NOFOLLOW checks
    // the leaf without following it.
    let result = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            destination.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(format!(
                "cannot inspect the removable registration: {error}"
            ))
        };
    }
    // SAFETY: fstatat initialized metadata after returning 0.
    let metadata = unsafe { metadata.assume_init() };
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_uid != uid
        || metadata.st_mode & 0o077 != 0
    {
        return Err("the removable registration is not an owner-only regular file".to_owned());
    }
    Ok(())
}

fn with_temp_cleanup(directory: &File, temp: &CString, error: String) -> String {
    // SAFETY: `temp` is a live CString with initialized bytes through its
    // terminator, and `directory` owns a live descriptor. The caller created
    // this exclusive temporary entry in that directory.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), temp.as_ptr(), 0) } == 0 {
        error
    } else {
        format!(
            "{error}; cannot remove the registration temporary file: {}",
            std::io::Error::last_os_error()
        )
    }
}

pub fn registration(internal: &Store) -> Result<Option<Registration>, String> {
    read_registration(internal.control())
}

/// Read an existing private registration without creating or migrating it.
pub(crate) fn read_registration(control: &Path) -> Result<Option<Registration>, String> {
    let Some(directory) = store::open_control_directory(control)? else {
        return Ok(None);
    };
    let name = CString::new(REGISTRATION).map_err(|error| error.to_string())?;
    // SAFETY: `name` is a live CString with initialized bytes through its
    // terminator, and `directory` owns a live descriptor that anchors this
    // lookup inside the verified private control directory.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(format!("cannot read the removable registration: {error}"))
        };
    }
    // SAFETY: `fd` is a newly opened descriptor owned by this function.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot read the removable registration: {error}"))?;
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_REGISTRATION_BYTES
    {
        return Err("the removable registration is not a small owner-only regular file".to_owned());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_REGISTRATION_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read the removable registration: {error}"))?;
    if bytes.len() as u64 > MAX_REGISTRATION_BYTES {
        return Err("the removable registration exceeds its byte limit".to_owned());
    }
    parse_registration(&bytes).map(Some)
}

fn parse_registration(bytes: &[u8]) -> Result<Registration, String> {
    let stored: StoredRegistration = serde_json::from_slice(bytes)
        .map_err(|error| format!("the removable registration is invalid: {error}"))?;
    match stored {
        StoredRegistration::Legacy(registration)
            if registration.version == LEGACY_REGISTRATION_VERSION =>
        {
            Ok(registration.into())
        }
        StoredRegistration::Current(registration)
            if registration.version == REGISTRATION_VERSION =>
        {
            Ok(registration)
        }
        _ => Err("the removable registration has an unsupported version".to_owned()),
    }
}

/// A removable registration (or its pool state) under another pool Oxide
/// knows of: the retired image pool's default path, the built-in default or
/// a pool created before a path change. Read-only: names only.
pub fn previous_registration(internal: &Store) -> Option<PathBuf> {
    let facts = Facts::from_env();
    let current = fs::canonicalize(&internal.pool).unwrap_or_else(|_| internal.pool.clone());
    config::legacy_default_pool(&facts)
        .into_iter()
        .chain(config::default_pool(&facts))
        .chain(config::known_pools(&facts))
        .find(|path| {
            let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let control = path.join(oxide_storage::lease::MANAGED_NAME);
            resolved != current
                && (fs::symlink_metadata(control.join(REGISTRATION)).is_ok()
                    || fs::symlink_metadata(control.join(POOL_DIR)).is_ok())
        })
}

/// Whether the internal pool holds `slot`; otherwise a removable one may.
fn internal_holds(internal: &Store, slot: &str) -> Result<bool, String> {
    Ok(internal.ledger()?.slots.iter().any(|held| {
        held.slot_id == slot && held.state == oxide_storage::lease::SlotOccupancy::Held
    }))
}

/// Oxide never moves a registration or its ledger, and never creates a
/// second one that could claim the same volume.
pub fn previous_registration_refusal(previous: &Path) -> String {
    format!(
        "a removable volume is registered under the previous pool {}; Oxide does not move that registration or its ledger and will not create a second one. Before registering here: with the oxide build that used that pool, confirm `oxide status` shows no held slots or waiting commands for its removable pool, then retire the old registration as a separate step; its files stay until then",
        previous.display()
    )
}

/// Decide, when a build starts, whether it uses the removable pool: the
/// registered volume is the mounted root at its mount point (rechecked on an
/// open descriptor), `/Volumes` stays non-writable, the volume is writable
/// and its available space is above the reserve. Otherwise the internal
/// pool. Nothing is created here; the removable pool was created at
/// registration and the library rebinds the volume on every open.
pub fn select(internal: Store) -> Result<Selected, String> {
    let Some(registration) = registration(&internal)? else {
        let note = previous_registration(&internal).map(|previous| {
            format!(
                "internal pool: the removable registration under the previous pool {} is not used; see `oxide status`",
                previous.display()
            )
        });
        return Ok(Selected {
            store: internal,
            note,
            removable: false,
            mode: None,
            fallback_reason: None,
        });
    };
    let control = internal
        .control()
        .join(POOL_DIR)
        .join(oxide_storage::lease::MANAGED_NAME);
    match usable(&internal, &registration) {
        Ok(store) => {
            let note = format!(
                "removable volume {} selected",
                registration.mount_point.display()
            );
            Ok(Selected {
                store: store.with_sibling(internal.control(), true),
                note: Some(note),
                removable: true,
                mode: Some(registration.mode),
                fallback_reason: None,
            })
        }
        Err(reason) => Ok(Selected {
            store: internal.with_sibling(&control, false),
            note: Some(format!(
                "internal pool: removable volume not used ({reason})"
            )),
            removable: false,
            mode: Some(registration.mode),
            fallback_reason: Some(reason),
        }),
    }
}

/// The removable store if the registered volume is usable now.
fn usable(internal: &Store, registration: &Registration) -> Result<Store, String> {
    if !cfg!(target_os = "macos") {
        return Err("the removable backend is macOS-only".to_owned());
    }
    let binding = Binding::open(&registration.mount_point, Some(&registration.volume_uuid))?;
    if !volume::writable(binding.mount_point())? {
        return Err("the volume is not writable".to_owned());
    }
    // Container, boot/data and role exclusions again: a registered volume
    // may since have gained a role or moved. Unknown metadata refuses.
    let identity = disk_identity(binding.mount_point(), binding.uuid(), &plist)?;
    check_identity(&identity, Some(&registration.container_uuid))?;
    let available = binding
        .verify()?
        .available_bytes
        .ok_or("the volume's free space is unknown")?;
    if available <= registration.reserve_bytes {
        return Err(format!(
            "{} MiB available is not above the {} MiB reserve",
            available / MIB,
            registration.reserve_bytes / MIB
        ));
    }
    Store::open_removable(
        &internal.control().join(POOL_DIR),
        registration.mount_point.join(PAYLOAD_DIR),
        internal.max_active(),
    )
}

/// Pair the internal store with a registered removable pool's state, so
/// `oxide clean` waits for the checkout's removable builds too. Without a
/// registration the store is returned unchanged.
pub fn coordinate(internal: Option<Store>) -> Result<Option<Store>, String> {
    let Some(internal) = internal else {
        return Ok(None);
    };
    if registration(&internal)?.is_none() {
        return Ok(Some(internal));
    }
    let control = removable_control(internal.control());
    Ok(Some(internal.with_sibling(&control, false)))
}

/// The removable pool's control directory, kept in the internal pool's
/// control directory so it stays readable while the volume is away.
pub fn removable_control(internal_control: &Path) -> PathBuf {
    internal_control
        .join(POOL_DIR)
        .join(oxide_storage::lease::MANAGED_NAME)
}

/// A registered removable pool as `status` shows it, before printing.
pub struct RemovableStatus {
    /// Registration lines: the first is the pool's identity, the rest are
    /// indented details.
    pub lines: Vec<String>,
    /// The opened store when the volume is usable for the next build, else
    /// why not. Selection is the same check builds use.
    pub selected: Result<Store, String>,
    /// Its control directory, internal, readable while the volume is away.
    pub control: PathBuf,
}

/// The registration and selection of the removable pool, `None` when none
/// is registered. Prints nothing.
pub fn status(internal: &Store) -> Result<Option<RemovableStatus>, String> {
    let Some(registration) = registration(internal)? else {
        return Ok(None);
    };
    let mut lines = vec![
        format!(
            "{} volume {} container {} (mode: {})",
            registration.mount_point.display(),
            registration.volume_uuid,
            registration.container_uuid,
            registration.mode
        ),
        format!(
            "  backend {}: {}",
            Backend::ApfsVolume.tag(),
            Backend::ApfsVolume.bound()
        ),
    ];
    let quota = registration
        .quota_bytes
        .map_or_else(|| "none".to_owned(), |bytes| format!("{} MiB", bytes / MIB));
    let capacity = registration.capacity_bytes.map_or_else(
        || "unknown".to_owned(),
        |bytes| format!("{} MiB", bytes / MIB),
    );
    lines.push(format!(
        "  reserve {} MiB, Oxide admission cap {} MiB; at registration: volume capacity {capacity} (shared, not an Oxide quota), APFS quota {quota}",
        registration.reserve_bytes / MIB,
        registration.cap_bytes / MIB
    ));
    let selected = usable(internal, &registration);
    if selected.is_ok() {
        let facts = volume::observe_path(&registration.mount_point).ok();
        let now = facts.map_or_else(
            || "unknown".to_owned(),
            |facts| {
                format!(
                    "{} of {} MiB available",
                    facts.available_bytes.unwrap_or(0) / MIB,
                    facts.capacity_bytes.unwrap_or(0) / MIB
                )
            },
        );
        lines.push(format!("  volume now: {now}"));
    }
    Ok(Some(RemovableStatus {
        lines,
        selected,
        control: internal
            .control()
            .join(POOL_DIR)
            .join(oxide_storage::lease::MANAGED_NAME),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Barrier};

    static TEST_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestControl(PathBuf);

    impl TestControl {
        fn new() -> Self {
            loop {
                let id = TEST_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
                let root = std::env::temp_dir()
                    .join(format!("oxide-drive-mode-{}-{id}", std::process::id()));
                match fs::create_dir(&root) {
                    Ok(()) => {
                        let control = root.join("oxide-managed");
                        fs::create_dir(&control).unwrap();
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
                        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
                        return Self(control);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create drive-mode test fixture: {error}"),
                }
            }
        }
    }

    impl Drop for TestControl {
        fn drop(&mut self) {
            if let Some(root) = self.0.parent() {
                let _ = fs::remove_dir_all(root);
            }
        }
    }

    fn legacy_registration_bytes() -> Vec<u8> {
        br#"{
            "version": 1,
            "mount_point": "/Volumes/OxideDriveModeTest",
            "volume_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
            "container_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
            "reserve_bytes": 1048576,
            "cap_bytes": 1048576,
            "quota_bytes": null,
            "capacity_bytes": null,
            "registered_ms": 17
        }"#
        .to_vec()
    }

    fn registration(mode: DriveMode) -> Registration {
        Registration {
            version: REGISTRATION_VERSION,
            mount_point: PathBuf::from("/Volumes/OxideDriveModeTest"),
            volume_uuid: "A6AC3ED7-33EA-46B4-8082-7254CF291ED2".to_owned(),
            container_uuid: "A6AC3ED7-33EA-46B4-8082-7254CF291ED2".to_owned(),
            reserve_bytes: 1 << 20,
            cap_bytes: 1 << 20,
            quota_bytes: None,
            capacity_bytes: None,
            registered_ms: 17,
            mode,
        }
    }

    fn store_registration(control: &Path, bytes: &[u8]) {
        let path = control.join(REGISTRATION);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn only_role_less_volumes_register() {
        assert!(refuse_roles(&[]).is_ok());
        let backup = refuse_roles(&["Backup"]).unwrap_err();
        assert!(backup.contains("Backup"), "{backup}");
        for role in ["Data", "System", "Preboot", "Recovery", "VM", "Update"] {
            assert!(refuse_roles(&[role]).is_err(), "{role}");
        }
    }

    fn identity(container: &str, roles: &[&str]) -> DiskIdentity {
        DiskIdentity {
            container_uuid: container.to_owned(),
            container_ref: "disk13".to_owned(),
            data_container_ref: "disk3".to_owned(),
            roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            quota_bytes: None,
        }
    }

    /// Injected metadata: selection refuses a registered volume that later
    /// gained the Backup role, moved to another container or sits in the
    /// boot/data container.
    #[test]
    fn selection_rechecks_container_and_roles() {
        let registered = "7CADDF63-E2EA-4EF5-8D0B-889B7E829171";
        assert!(check_identity(&identity(registered, &[]), Some(registered)).is_ok());
        let backup =
            check_identity(&identity(registered, &["Backup"]), Some(registered)).unwrap_err();
        assert!(backup.contains("Backup"), "{backup}");
        let moved = check_identity(&identity("OTHER", &[]), Some(registered)).unwrap_err();
        assert!(moved.contains("not the registered"), "{moved}");
        let mut internal = identity(registered, &[]);
        internal.container_ref = "disk3".to_owned();
        assert!(check_identity(&internal, Some(registered)).is_err());
        assert!(check_identity(&internal, None).is_err());
    }

    #[test]
    fn sizes_are_explicit_units() {
        assert_eq!(size("--reserve", "1GiB"), Ok(1024 * MIB));
        assert_eq!(size("--reserve", "100GB"), Ok(100_000_000_000));
        assert_eq!(size("--cap", "750000000000"), Ok(750_000_000_000));
        for invalid in [
            "",
            "0",
            "1.5GiB",
            "-1",
            "1gb",
            "18446744073709551616",
            "99999999999TiB",
        ] {
            let error = size("--reserve", invalid).unwrap_err();
            assert!(error.contains("--reserve"), "{error}");
        }
    }

    #[test]
    fn legacy_v1_reads_as_ephemeral_without_migrating_bytes() {
        let control = TestControl::new();
        let bytes = legacy_registration_bytes();
        store_registration(&control.0, &bytes);

        let loaded = read_registration(&control.0).unwrap().unwrap();
        assert_eq!(loaded.version, LEGACY_REGISTRATION_VERSION);
        assert_eq!(loaded.mode, DriveMode::Ephemeral);
        assert_eq!(fs::read(control.0.join(REGISTRATION)).unwrap(), bytes);
    }

    #[test]
    fn v2_requires_a_known_mode_and_rejects_legacy_mode_fields() {
        let current = serde_json::to_value(registration(DriveMode::Permanent)).unwrap();
        let parsed = parse_registration(&serde_json::to_vec(&current).unwrap()).unwrap();
        assert_eq!(parsed.mode, DriveMode::Permanent);

        let mut missing_mode = current.clone();
        missing_mode.as_object_mut().unwrap().remove("mode");
        assert!(parse_registration(&serde_json::to_vec(&missing_mode).unwrap()).is_err());

        let mut unknown_mode = current.clone();
        unknown_mode["mode"] = serde_json::Value::String("sometimes".to_owned());
        assert!(parse_registration(&serde_json::to_vec(&unknown_mode).unwrap()).is_err());

        let mut unknown_version = current.clone();
        unknown_version["version"] = serde_json::Value::Number(3.into());
        let error = parse_registration(&serde_json::to_vec(&unknown_version).unwrap()).unwrap_err();
        assert!(error.contains("unsupported version"), "{error}");

        let mut unknown_field = current;
        unknown_field["new_field"] = serde_json::Value::Bool(true);
        assert!(parse_registration(&serde_json::to_vec(&unknown_field).unwrap()).is_err());

        let mut legacy_with_mode: serde_json::Value =
            serde_json::from_slice(&legacy_registration_bytes()).unwrap();
        legacy_with_mode["mode"] = serde_json::Value::String("permanent".to_owned());
        assert!(parse_registration(&serde_json::to_vec(&legacy_with_mode).unwrap()).is_err());

        let v2_bytes = serde_json::to_vec(&registration(DriveMode::Permanent)).unwrap();
        assert!(serde_json::from_slice::<LegacyRegistration>(&v2_bytes).is_err());
    }

    #[test]
    fn writing_a_legacy_record_publishes_v2_with_explicit_mode() {
        let control = TestControl::new();
        let legacy = parse_registration(&legacy_registration_bytes()).unwrap();
        write_registration(&control.0, &legacy).unwrap();

        let bytes = fs::read(control.0.join(REGISTRATION)).unwrap();
        let persisted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(persisted["version"], REGISTRATION_VERSION);
        assert_eq!(persisted["mode"], "ephemeral");
        assert_eq!(read_registration(&control.0).unwrap().unwrap().version, 2);
    }

    #[test]
    fn failed_update_before_rename_preserves_registration_and_temp_bytes() {
        let control = TestControl::new();
        let original = serde_json::to_vec_pretty(&registration(DriveMode::Ephemeral)).unwrap();
        store_registration(&control.0, &original);
        let blocked_temp = control.0.join("removable.json.tmp.blocked");
        fs::write(&blocked_temp, b"owned by a prior attempt").unwrap();
        fs::set_permissions(&blocked_temp, fs::Permissions::from_mode(0o600)).unwrap();

        let error = write_registration_with_temp_name(
            &control.0,
            &registration(DriveMode::Permanent),
            "removable.json.tmp.blocked",
        )
        .unwrap_err();
        assert!(error.contains("cannot create"), "{error}");
        assert_eq!(fs::read(control.0.join(REGISTRATION)).unwrap(), original);
        assert_eq!(fs::read(blocked_temp).unwrap(), b"owned by a prior attempt");
    }

    #[test]
    fn registration_writer_refuses_to_replace_a_symlink() {
        let control = TestControl::new();
        let target = control.0.join("registration-target");
        fs::write(&target, b"outside registration bytes").unwrap();
        let destination = control.0.join(REGISTRATION);
        std::os::unix::fs::symlink(&target, &destination).unwrap();

        let error =
            write_registration(&control.0, &registration(DriveMode::Permanent)).unwrap_err();
        assert!(error.contains("owner-only regular file"), "{error}");
        assert!(fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read(&target).unwrap(), b"outside registration bytes");
    }

    #[test]
    fn set_mode_refuses_missing_registration_without_creating_one() {
        let control = TestControl::new();
        let error = set_mode_in_control(&control.0, DriveMode::Permanent).unwrap_err();
        assert!(error.contains("no removable pool is registered"), "{error}");
        assert!(!control.0.join(REGISTRATION).exists());
    }

    #[test]
    fn concurrent_mode_updates_publish_one_complete_v2_record() {
        let control = TestControl::new();
        let initial = serde_json::to_vec_pretty(&registration(DriveMode::Ephemeral)).unwrap();
        store_registration(&control.0, &initial);
        let path = Arc::new(control.0.clone());
        let start = Arc::new(Barrier::new(3));
        let mut writers = Vec::new();
        for mode in [DriveMode::Permanent, DriveMode::Ephemeral] {
            let path = Arc::clone(&path);
            let start = Arc::clone(&start);
            writers.push(std::thread::spawn(move || {
                start.wait();
                set_mode_in_control(&path, mode)
            }));
        }
        start.wait();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }

        let final_registration = read_registration(&control.0).unwrap().unwrap();
        assert_eq!(final_registration.version, REGISTRATION_VERSION);
        assert!(matches!(
            final_registration.mode,
            DriveMode::Permanent | DriveMode::Ephemeral
        ));
        assert_eq!(
            final_registration.mount_point,
            registration(DriveMode::Ephemeral).mount_point
        );
        assert_eq!(final_registration.reserve_bytes, 1 << 20);
        assert_eq!(final_registration.cap_bytes, 1 << 20);
        assert!(parse_registration(&fs::read(control.0.join(REGISTRATION)).unwrap()).is_ok());
    }
}
