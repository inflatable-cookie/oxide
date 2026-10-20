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

use crate::process;
use crate::store::{self, Backend, Store};
use crate::MIB;
use oxide_storage::lease::managed::init_volume_pool;
use oxide_storage::volume::{self, Binding};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Registration file in the internal control directory.
const REGISTRATION: &str = "removable.json";
/// The removable pool's internal root (config, ledger, lock, Oxide state).
const POOL_DIR: &str = "removable";
/// The payload directory directly under the volume's mount root.
pub const PAYLOAD_DIR: &str = "oxide-pool";
const REGISTRATION_VERSION: u32 = 1;
const MAX_REGISTRATION_BYTES: u64 = 16 * 1024;
const MAX_PLIST_BYTES: usize = 4 * 1024 * 1024;

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
}

/// Which pool a build uses, and why.
pub struct Selected {
    pub store: Store,
    /// `None` when nothing is registered: the internal pool, unchanged.
    pub note: Option<String>,
    pub removable: bool,
}

const USAGE: &str = "\
usage: oxide pool register-removable <mount-point> --reserve <size> --cap <size>

Registers a dedicated APFS volume mounted directly under /Volumes as the
preferred build pool. <size> is bytes or a whole number of KiB, MiB, GiB or
TiB. --reserve is the free space a build start must find above it; --cap is
Oxide's admission budget on the volume. Both are required.";

/// `oxide pool ...`
pub fn run(args: &[OsString]) -> Result<u8, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        ["register-removable", rest @ ..] => {
            let mut mount = None;
            let mut reserve = None;
            let mut cap = None;
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match *arg {
                    "--reserve" => reserve = Some(*rest.next().ok_or("--reserve needs a size")?),
                    "--cap" => cap = Some(*rest.next().ok_or("--cap needs a size")?),
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
            register(Path::new(mount), reserve, cap)
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

fn register(mount: &Path, reserve: u64, cap: u64) -> Result<u8, String> {
    if !cfg!(target_os = "macos") {
        return Err("the removable apfs-volume-v1 backend is macOS-only".to_owned());
    }
    // Every check before the first write.
    let binding = Binding::open(mount, None)?;
    if !volume::writable(binding.mount_point())? {
        return Err(format!("{} is not writable", mount.display()));
    }
    let identity = disk_identity(binding.mount_point(), binding.uuid())?;
    check_identity(&identity, None)?;
    let internal = Store::open_or_create()?;
    let control = internal.control().to_path_buf();
    if fs::symlink_metadata(control.join(REGISTRATION)).is_ok()
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

/// What `diskutil` reports about a mounted volume, as checked at
/// registration and again whenever selection considers it.
struct DiskIdentity {
    container_uuid: String,
    container_ref: String,
    /// The container holding `/System/Volumes/Data`.
    data_container_ref: String,
    roles: Vec<String>,
    quota_bytes: Option<u64>,
}

fn disk_identity(mount: &Path, uuid: &str) -> Result<DiskIdentity, String> {
    let info = plist(&["info", "-plist", path_str(mount)?])?;
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
    let data = plist(&["info", "-plist", "/System/Volumes/Data"])?;
    let data_container_ref = data
        .get("APFSContainerReference")
        .and_then(serde_json::Value::as_str)
        .ok_or("diskutil reports no container for /System/Volumes/Data")?
        .to_owned();
    let list = plist(&["apfs", "list", "-plist", &container_ref])?;
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
fn check_identity(identity: &DiskIdentity, registered: Option<&str>) -> Result<(), String> {
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
    let bytes = serde_json::to_vec_pretty(registration).map_err(|error| error.to_string())?;
    let temp = control.join("removable.json.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)
        .map_err(|error| format!("cannot write the registration: {error}"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temp, control.join(REGISTRATION)))
        .map_err(|error| format!("cannot write the registration: {error}"))
}

pub fn registration(internal: &Store) -> Result<Option<Registration>, String> {
    let path = internal.control().join(REGISTRATION);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read the removable registration: {error}")),
    };
    if !metadata.is_file() || metadata.len() > MAX_REGISTRATION_BYTES {
        return Err("the removable registration is not a small regular file".to_owned());
    }
    let bytes = fs::read(&path)
        .map_err(|error| format!("cannot read the removable registration: {error}"))?;
    let registration: Registration = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the removable registration is invalid: {error}"))?;
    if registration.version != REGISTRATION_VERSION {
        return Err("the removable registration has an unsupported version".to_owned());
    }
    Ok(Some(registration))
}

/// Decide, when a build starts, whether it uses the removable pool: the
/// registered volume is the mounted root at its mount point (rechecked on an
/// open descriptor), `/Volumes` stays non-writable, the volume is writable
/// and its available space is above the reserve. Otherwise the internal
/// pool. Nothing is created here; the removable pool was created at
/// registration and the library rebinds the volume on every open.
pub fn select(internal: Store) -> Result<Selected, String> {
    let Some(registration) = registration(&internal)? else {
        return Ok(Selected {
            store: internal,
            note: None,
            removable: false,
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
            })
        }
        Err(reason) => Ok(Selected {
            store: internal.with_sibling(&control, false),
            note: Some(format!(
                "internal pool: removable volume not used ({reason})"
            )),
            removable: false,
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
    let identity = disk_identity(binding.mount_point(), binding.uuid())?;
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
    let control = internal
        .control()
        .join(POOL_DIR)
        .join(oxide_storage::lease::MANAGED_NAME);
    Ok(Some(internal.with_sibling(&control, false)))
}

/// `status` lines for the registration and the removable pool. Returns the
/// removable store when it could be opened, for its slot listing.
pub fn status(internal: &Store) -> Result<Option<Store>, String> {
    let Some(registration) = registration(internal)? else {
        println!("removable pool: none registered");
        return Ok(None);
    };
    println!(
        "removable pool: {} volume {} container {}",
        registration.mount_point.display(),
        registration.volume_uuid,
        registration.container_uuid
    );
    println!(
        "  backend {}: {}",
        Backend::ApfsVolume.tag(),
        Backend::ApfsVolume.bound()
    );
    let quota = registration
        .quota_bytes
        .map_or_else(|| "none".to_owned(), |bytes| format!("{} MiB", bytes / MIB));
    let capacity = registration.capacity_bytes.map_or_else(
        || "unknown".to_owned(),
        |bytes| format!("{} MiB", bytes / MIB),
    );
    println!(
        "  reserve {} MiB, Oxide admission cap {} MiB; at registration: volume capacity {capacity} (shared, not an Oxide quota), APFS quota {quota}",
        registration.reserve_bytes / MIB,
        registration.cap_bytes / MIB
    );
    let selection = usable(internal, &registration);
    match &selection {
        Ok(_) => {
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
            println!("  volume now: {now}");
            println!("selected for the next build: removable pool");
        }
        Err(reason) => {
            println!(
                "selected for the next build: internal pool (removable volume not used: {reason})"
            );
        }
    }
    Ok(selection.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn sizes_are_explicit_whole_mib() {
        assert_eq!(size("--reserve", "1GiB"), Ok(1024 * MIB));
        for invalid in [
            "",
            "0",
            "1.5GiB",
            "-1",
            "1GB",
            "18446744073709551616",
            "99999999999TiB",
        ] {
            let error = size("--reserve", invalid).unwrap_err();
            assert!(error.contains("--reserve"), "{error}");
        }
    }
}
