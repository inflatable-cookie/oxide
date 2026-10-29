//! `oxide pool describe --json`: the registered drive as stable, read-only
//! JSON for other tools.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (`oxide pool
//! describe --json`). It never uses the build path's pool openers: no
//! control directory is created, no recorded deletion is reconciled, no
//! ledger is written and no accounting is walked. Every `diskutil` and
//! `plutil` read is bounded by a timeout and an output cap, and a refusal of
//! any of them is unknown, never available.

use crate::process;
use crate::removable::{self, DriveMode, Registration, MAX_PLIST_BYTES, POOL_DIR};
use crate::store;
use oxide_storage::lease::{managed, LeaseError, VolumeDescription, MANAGED_NAME};
use oxide_storage::volume::{self, Binding};
use serde::Serialize;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const SCHEMA: &str = "oxide.pool.describe/1";
/// The volume-root directory suggested for a consumer's own scratch. It is
/// a sibling of the payload, never inside it.
const SCRATCH_DIR: &str = "scratch";
/// Per `diskutil` or `plutil` call.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
struct Description {
    schema: &'static str,
    observed_at: String,
    internal: Internal,
    selected: &'static str,
    removable: Option<Removable>,
}

#[derive(Serialize)]
struct Internal {
    pool: String,
}

/// Identity fields are `null` only when the registration itself cannot be
/// read: then nothing identifies the drive and none is invented.
#[derive(Serialize)]
struct Removable {
    mount_point: Option<String>,
    volume_uuid: Option<String>,
    mode: DriveMode,
    /// The registered reserve. A writer rechecks free space against it at use
    /// time: `available` is only a snapshot.
    reserve_bytes: Option<u64>,
    available: bool,
    /// Ledger records in `Evicting`: deletions a build's open reconciles. A
    /// count, never a refusal, so `available` is unaffected. `null` when the
    /// config and ledger were not read.
    pending_reconciliation_slots: Option<usize>,
    /// Recorded slot IDs; unknown when the ledger could not be read.
    pending_reconciliation_slot_ids: Option<Vec<String>>,
    unavailable_reason: Option<&'static str>,
    unavailable_detail: Option<String>,
    payload: Option<String>,
    suggested_scratch_root: Option<String>,
}

/// Why the drive is not available. Consumers branch on `available`; these
/// tags explain it and may gain values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    /// Nothing is mounted at the registered mount point.
    Absent,
    /// The mount point, or the volume the config or ledger records, is not
    /// the registered volume or container.
    IdentityMismatch,
    /// The volume is mounted but is refused by a selection rule: mount
    /// parent, read-only, role, boot/data container or the like.
    VolumeUnusable,
    BelowReserve,
    /// A probe timed out, its output was over the cap, or it could not be
    /// parsed or read.
    MetadataUnknown,
    /// The internal control directory, config or ledger is absent.
    ControlMissing,
    PayloadMissing,
}

impl Reason {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::IdentityMismatch => "identity_mismatch",
            Self::VolumeUnusable => "volume_unusable",
            Self::BelowReserve => "below_reserve",
            Self::MetadataUnknown => "metadata_unknown",
            Self::ControlMissing => "control_missing",
            Self::PayloadMissing => "payload_missing",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Unavailable {
    pub reason: Reason,
    pub detail: String,
}

fn unavailable(reason: Reason, detail: impl Into<String>) -> Unavailable {
    Unavailable {
        reason,
        detail: detail.into(),
    }
}

/// The registration as far as it could be read.
enum Registered {
    None,
    Known(Registration),
    /// A registration file exists but cannot be read: the drive is unknown.
    Unknown(String),
}

/// `oxide pool describe --json`: print the description on stdout. Returns
/// an error, and so exit non-zero, only when Oxide itself fails.
pub fn print_json() -> Result<u8, String> {
    let configured = store::settings()?.pool;
    let description = describe(&configured)?;
    let text = serde_json::to_string_pretty(&description).map_err(|error| error.to_string())?;
    println!("{text}");
    Ok(0)
}

fn describe(configured: &Path) -> Result<Description, String> {
    let observed_at = rfc3339(process::now_ms() / 1000);
    let internal = Internal {
        pool: configured.to_string_lossy().into_owned(),
    };
    // A pool that does not exist has no registration. Only a missing path
    // means that; another failure to look is Oxide failing.
    let (control, registered) = match fs::canonicalize(configured) {
        Err(error) if error.kind() == ErrorKind::NotFound => (None, Registered::None),
        Err(error) => return Err(format!("cannot read the internal pool: {error}")),
        Ok(canonical) => {
            let control = canonical.join(MANAGED_NAME);
            let registered = match removable::read_registration(&control) {
                Ok(None) => Registered::None,
                Ok(Some(registration)) => Registered::Known(registration),
                Err(detail) => Registered::Unknown(detail),
            };
            (Some(control.join(POOL_DIR)), registered)
        }
    };
    let (removable, selected) = match registered {
        Registered::None => (None, "internal"),
        Registered::Unknown(detail) => (
            Some(Removable {
                mount_point: None,
                volume_uuid: None,
                mode: DriveMode::Ephemeral,
                reserve_bytes: None,
                available: false,
                pending_reconciliation_slots: None,
                pending_reconciliation_slot_ids: None,
                unavailable_reason: Some(Reason::MetadataUnknown.tag()),
                unavailable_detail: Some(detail),
                payload: None,
                suggested_scratch_root: None,
            }),
            "internal",
        ),
        Registered::Known(registration) => {
            let control = control.ok_or("the registered control directory is missing")?;
            let outcome = availability(&registration, &control);
            let available = outcome.is_ok();
            let (pending, pending_ids, reason, detail) = match outcome {
                Ok((count, ids)) => (Some(count), Some(ids), None, None),
                Err(Unavailable { reason, detail }) => {
                    (None, None, Some(reason.tag()), Some(detail))
                }
            };
            let removable = Removable {
                mount_point: Some(lossy(&registration.mount_point)),
                volume_uuid: Some(registration.volume_uuid.clone()),
                mode: registration.mode,
                reserve_bytes: Some(registration.reserve_bytes),
                available,
                pending_reconciliation_slots: pending,
                pending_reconciliation_slot_ids: pending_ids,
                unavailable_reason: reason,
                unavailable_detail: detail,
                payload: Some(lossy(
                    &registration.mount_point.join(removable::PAYLOAD_DIR),
                )),
                suggested_scratch_root: Some(lossy(&registration.mount_point.join(SCRATCH_DIR))),
            };
            (
                Some(removable),
                if available { "removable" } else { "internal" },
            )
        }
    };
    Ok(Description {
        schema: SCHEMA,
        observed_at,
        internal,
        selected,
        removable,
    })
}

/// Passive eligibility: every check a build's selection makes, probed
/// read-only, plus the pool's config and ledger. Opening for a build can
/// still differ, so this is a snapshot, not a promise. On success it returns
/// the pending reconciliation count: a build's open reconciles those records
/// and selects the drive anyway, so they never refuse.
fn availability(
    registration: &Registration,
    control: &Path,
) -> Result<(usize, Vec<String>), Unavailable> {
    let binding = probe_volume(registration)?;
    let description = probe_control(control, registration)?;
    probe_payload(&binding)?;
    Ok(pending(&description))
}

/// The mount rules, read-only: the registered volume is mounted at its
/// mount point, and nothing else at that path is accepted.
fn probe_volume(registration: &Registration) -> Result<Binding, Unavailable> {
    let mount = &registration.mount_point;
    let uuid = registration.volume_uuid.as_str();
    match fs::symlink_metadata(mount) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(unavailable(
                Reason::Absent,
                format!("nothing is mounted at {}", mount.display()),
            ))
        }
        Err(error) => return Err(unavailable(Reason::MetadataUnknown, error.to_string())),
        Ok(_) => {}
    }
    // A directory at the path that is not this volume's root, such as a
    // stale mount point on the internal disk, is never the drive.
    let facts =
        volume::observe_path(mount).map_err(|error| unavailable(Reason::MetadataUnknown, error))?;
    if facts.mounted_on != *mount || facts.uuid.as_deref() != Some(uuid) {
        return Err(unavailable(
            Reason::IdentityMismatch,
            format!("{} is not volume {uuid}", mount.display()),
        ));
    }
    let binding = Binding::open(mount, Some(uuid))
        .map_err(|error| unavailable(Reason::VolumeUnusable, error))?;
    let writable = volume::writable(binding.mount_point())
        .map_err(|error| unavailable(Reason::MetadataUnknown, error))?;
    if !writable {
        return Err(unavailable(
            Reason::VolumeUnusable,
            "the volume is not writable",
        ));
    }
    let identity = removable::disk_identity(binding.mount_point(), binding.uuid(), &bounded_plist)
        .map_err(|error| unavailable(Reason::MetadataUnknown, error))?;
    if identity.container_uuid != registration.container_uuid {
        return Err(unavailable(
            Reason::IdentityMismatch,
            format!(
                "the volume is in container {}, not the registered {}",
                identity.container_uuid, registration.container_uuid
            ),
        ));
    }
    removable::check_identity(&identity, None)
        .map_err(|error| unavailable(Reason::VolumeUnusable, error))?;
    let available = binding
        .verify()
        .map_err(|error| unavailable(Reason::IdentityMismatch, error))?
        .available_bytes
        .ok_or_else(|| unavailable(Reason::MetadataUnknown, "the free space is unknown"))?;
    if available <= registration.reserve_bytes {
        return Err(unavailable(
            Reason::BelowReserve,
            format!(
                "{} MiB available is not above the {} MiB reserve",
                available / MIB,
                registration.reserve_bytes / MIB
            ),
        ));
    }
    Ok(binding)
}

/// The config and ledger, read-only, against the registration.
fn probe_control(
    control: &Path,
    registration: &Registration,
) -> Result<VolumeDescription, Unavailable> {
    match fs::symlink_metadata(control) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Err(unavailable(Reason::ControlMissing, "no control directory"))
        }
        Err(error) => return Err(unavailable(Reason::MetadataUnknown, error.to_string())),
        Ok(_) => {}
    }
    let description = managed::describe_volume(control).map_err(|error| match error {
        LeaseError::NotFound => unavailable(Reason::ControlMissing, "no config or ledger"),
        other => unavailable(Reason::MetadataUnknown, other.reason()),
    })?;
    if description.mount_point != registration.mount_point
        || description.volume_uuid != registration.volume_uuid
        || description.payload_dir != removable::PAYLOAD_DIR
        || description.cap_bytes != registration.cap_bytes
    {
        return Err(unavailable(
            Reason::IdentityMismatch,
            "the config records a different volume or budget than the registration",
        ));
    }
    Ok(description)
}

/// The payload directory on the volume, opened read-only.
fn probe_payload(binding: &Binding) -> Result<(), Unavailable> {
    match binding.dir_at(binding.fd(), removable::PAYLOAD_DIR, false) {
        Ok(_) => Ok(()),
        Err(error) => {
            let path = binding.mount_point().join(removable::PAYLOAD_DIR);
            match fs::symlink_metadata(path) {
                Err(missing) if missing.kind() == ErrorKind::NotFound => {
                    Err(unavailable(Reason::PayloadMissing, error))
                }
                _ => Err(unavailable(Reason::MetadataUnknown, error)),
            }
        }
    }
}

/// Recorded slot deletions awaiting reconciliation at the next build's open.
/// Reported beside `available`, never a reason to refuse.
fn pending(description: &VolumeDescription) -> (usize, Vec<String>) {
    (
        description.evicting_slots,
        description.evicting_slot_ids.clone(),
    )
}

/// `diskutil <args>` as JSON through `plutil`, each process bounded by
/// [`READ_TIMEOUT`] and [`MAX_PLIST_BYTES`].
fn bounded_plist(args: &[&str]) -> Result<serde_json::Value, String> {
    let plist = bounded("/usr/sbin/diskutil", args, None)?;
    let json = bounded(
        "/usr/bin/plutil",
        &["-convert", "json", "-o", "-", "-"],
        Some(&plist),
    )?;
    serde_json::from_slice(&json).map_err(|error| format!("diskutil output: {error}"))
}

/// Run `program args`, optionally feeding `input` on stdin, and return its
/// stdout. A timeout kills the child; output over the cap, a failure to
/// read, or a non-zero exit is an error.
fn bounded(program: &str, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program} has no stdout"))?;
    // One byte past the cap tells an overrun apart from a complete read.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = stdout
            .take(MAX_PLIST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(read);
    });
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        let input = input.to_vec();
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let deadline = Instant::now() + READ_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program} timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot wait for {program}: {error}"));
            }
        }
    };
    let bytes = receiver
        .recv_timeout(READ_TIMEOUT)
        .map_err(|_| format!("{program} output did not finish"))?
        .map_err(|error| format!("cannot read {program}: {error}"))?;
    if bytes.len() > MAX_PLIST_BYTES {
        return Err(format!("{program} output is over the cap"));
    }
    if !status.success() {
        return Err(format!("{program} exited with {status}"));
    }
    Ok(bytes)
}

const MIB: u64 = 1024 * 1024;

fn lossy(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// RFC 3339 in UTC, whole seconds, for a Unix time (proleptic Gregorian).
fn rfc3339(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(i64::MAX);
    let seconds = unix_seconds % 86_400;
    // Days since 0000-03-01, split into 400-year eras (Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_based_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_based_month + 2) / 5 + 1;
    let month = if march_based_month < 10 {
        march_based_month + 3
    } else {
        march_based_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
    }

    #[test]
    fn unregistered_description_is_null_removable_and_internal() {
        let description = Description {
            schema: SCHEMA,
            observed_at: rfc3339(0),
            internal: Internal {
                pool: "/pool".to_owned(),
            },
            selected: "internal",
            removable: None,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&description).unwrap()).unwrap();
        assert_eq!(json["schema"], "oxide.pool.describe/1");
        assert_eq!(json["selected"], "internal");
        assert!(json["removable"].is_null());
        assert_eq!(json["internal"]["pool"], "/pool");
    }

    #[test]
    fn unknown_registration_invents_no_identity_and_escapes_detail() {
        let removable = Removable {
            mount_point: None,
            volume_uuid: None,
            mode: DriveMode::Ephemeral,
            reserve_bytes: None,
            available: false,
            pending_reconciliation_slots: None,
            pending_reconciliation_slot_ids: None,
            unavailable_reason: Some(Reason::MetadataUnknown.tag()),
            unavailable_detail: Some("bad \"json\"\\\n".to_owned()),
            payload: None,
            suggested_scratch_root: None,
        };
        let text = serde_json::to_string(&removable).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json["mount_point"].is_null());
        assert!(json["volume_uuid"].is_null());
        assert!(json["reserve_bytes"].is_null());
        assert_eq!(json["mode"], "ephemeral");
        assert_eq!(json["available"], false);
        assert_eq!(json["unavailable_reason"], "metadata_unknown");
        assert_eq!(json["unavailable_detail"], "bad \"json\"\\\n");
    }

    #[test]
    fn registration_versions_and_modes_are_read_only_snapshots() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("oxide-describe-map-{}-{nonce}", std::process::id()));
        let pool = root.join("pool");
        let control = pool.join("oxide-managed");
        fs::create_dir_all(&control).unwrap();
        for path in [&root, &pool, &control] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mount = format!("/Volumes/oxide-describe-map-{}-{nonce}", std::process::id());
        assert!(!Path::new(&mount).exists());
        let legacy = serde_json::json!({
            "version": 1,
            "mount_point": mount,
            "volume_uuid": "A6AC3ED7-33EA-46B4-8082-7254CF291ED2",
            "container_uuid": "B7BD4FE8-44FB-47C5-9193-8365DF302FE3",
            "reserve_bytes": 4096,
            "cap_bytes": 8192,
            "quota_bytes": null,
            "capacity_bytes": null,
            "registered_ms": 0
        });
        let mut ephemeral = legacy.clone();
        ephemeral["version"] = 2.into();
        ephemeral["mode"] = "ephemeral".into();
        let mut permanent = ephemeral.clone();
        permanent["mode"] = "permanent".into();
        let mut unknown_version = permanent.clone();
        unknown_version["version"] = 3.into();
        let mut missing_mode = legacy.clone();
        missing_mode["version"] = 2.into();
        let mut unknown_mode = permanent.clone();
        unknown_mode["mode"] = "sometimes".into();
        let mut legacy_mode = legacy.clone();
        legacy_mode["mode"] = "permanent".into();
        let mut unknown_field = permanent.clone();
        unknown_field["unknown"] = true.into();
        let mut cases = vec![(b"{malformed".to_vec(), None)];
        for (registration, mode) in [
            (legacy, Some("ephemeral")),
            (ephemeral, Some("ephemeral")),
            (permanent, Some("permanent")),
            (unknown_version, None),
            (missing_mode, None),
            (unknown_mode, None),
            (legacy_mode, None),
            (unknown_field, None),
        ] {
            cases.push((serde_json::to_vec(&registration).unwrap(), mode));
        }
        let registration_path = control.join("removable.json");
        let snapshot = || {
            let metadata = [&root, &pool, &control, &registration_path].map(|path| {
                let metadata = fs::symlink_metadata(path).unwrap();
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.mode(),
                    metadata.len(),
                    metadata.modified().unwrap(),
                )
            });
            let entries = [&root, &pool, &control].map(|path| {
                let mut names: Vec<_> = fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect();
                names.sort();
                names
            });
            (fs::read(&registration_path).unwrap(), metadata, entries)
        };
        for (bytes, mode) in cases {
            fs::write(&registration_path, &bytes).unwrap();
            fs::set_permissions(&registration_path, fs::Permissions::from_mode(0o600)).unwrap();
            let before = snapshot();
            let json = serde_json::to_value(describe(&pool).unwrap()).unwrap();
            assert_eq!(snapshot(), before, "describe changed {bytes:?}");
            assert_eq!(json["selected"], "internal");
            let removable = &json["removable"];
            assert_eq!(removable["available"], false);
            if let Some(mode) = mode {
                assert_eq!(removable["mode"], mode);
                assert_eq!(removable["mount_point"], mount.as_str());
                assert_eq!(
                    removable["volume_uuid"],
                    "A6AC3ED7-33EA-46B4-8082-7254CF291ED2"
                );
                assert_eq!(removable["reserve_bytes"], 4096);
                assert_eq!(removable["unavailable_reason"], "absent");
                assert_eq!(removable["payload"], format!("{mount}/oxide-pool"));
                assert_eq!(
                    removable["suggested_scratch_root"],
                    format!("{mount}/scratch")
                );
            } else {
                assert_eq!(removable["unavailable_reason"], "metadata_unknown");
                for field in [
                    "mount_point",
                    "volume_uuid",
                    "reserve_bytes",
                    "payload",
                    "suggested_scratch_root",
                ] {
                    assert!(removable[field].is_null(), "{field}: {removable}");
                }
            }
            assert!(!Path::new(&mount).exists());
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn registered_reserve_is_reported_as_a_number() {
        let removable = Removable {
            mount_point: Some("/Volumes/Builds".to_owned()),
            volume_uuid: Some("A6AC3ED7-33EA-46B4-8082-7254CF291ED2".to_owned()),
            mode: DriveMode::Ephemeral,
            reserve_bytes: Some(20 * 1024 * 1024 * 1024),
            available: false,
            pending_reconciliation_slots: None,
            pending_reconciliation_slot_ids: None,
            unavailable_reason: Some(Reason::Absent.tag()),
            unavailable_detail: None,
            payload: Some("/Volumes/Builds/oxide-pool".to_owned()),
            suggested_scratch_root: Some("/Volumes/Builds/scratch".to_owned()),
        };
        let json: serde_json::Value = serde_json::to_value(&removable).unwrap();
        assert_eq!(json["reserve_bytes"], 20_u64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn reason_tags_are_the_documented_strings() {
        let tags: Vec<&str> = [
            Reason::Absent,
            Reason::IdentityMismatch,
            Reason::VolumeUnusable,
            Reason::BelowReserve,
            Reason::MetadataUnknown,
            Reason::ControlMissing,
            Reason::PayloadMissing,
        ]
        .into_iter()
        .map(Reason::tag)
        .collect();
        assert_eq!(
            tags,
            [
                "absent",
                "identity_mismatch",
                "volume_unusable",
                "below_reserve",
                "metadata_unknown",
                "control_missing",
                "payload_missing",
            ]
        );
    }

    #[test]
    fn pending_records_are_counted_not_refused() {
        let mut description = VolumeDescription {
            store_id: "id".to_owned(),
            cap_bytes: 1,
            mount_point: "/Volumes/Builds".into(),
            volume_uuid: "uuid".to_owned(),
            payload_dir: removable::PAYLOAD_DIR.to_owned(),
            evicting_slots: 0,
            evicting_slot_ids: Vec::new(),
        };
        assert_eq!(pending(&description), (0, vec![]));
        description.evicting_slots = 2;
        description.evicting_slot_ids = vec!["1".into(), "2".into()];
        assert_eq!(pending(&description), (2, vec!["1".into(), "2".into()]));
    }

    /// The JSON reports the count as a number, `0` when no record is pending.
    #[test]
    fn pending_count_is_a_number_beside_available() {
        let removable = Removable {
            mount_point: Some("/Volumes/Builds".to_owned()),
            volume_uuid: Some("A6AC3ED7-33EA-46B4-8082-7254CF291ED2".to_owned()),
            mode: DriveMode::Permanent,
            reserve_bytes: Some(20 * 1024 * 1024 * 1024),
            available: true,
            pending_reconciliation_slots: Some(3),
            pending_reconciliation_slot_ids: Some(vec!["1".into(), "2".into(), "3".into()]),
            unavailable_reason: None,
            unavailable_detail: None,
            payload: Some("/Volumes/Builds/oxide-pool".to_owned()),
            suggested_scratch_root: Some("/Volumes/Builds/scratch".to_owned()),
        };
        let json: serde_json::Value = serde_json::to_value(&removable).unwrap();
        assert_eq!(json["available"], true);
        assert_eq!(json["pending_reconciliation_slots"], 3);
        assert_eq!(
            json["pending_reconciliation_slot_ids"],
            serde_json::json!(["1", "2", "3"])
        );
        assert!(json["unavailable_reason"].is_null());
        let none = Removable {
            pending_reconciliation_slots: Some(0),
            pending_reconciliation_slot_ids: Some(vec![]),
            ..removable
        };
        let json: serde_json::Value = serde_json::to_value(&none).unwrap();
        assert_eq!(json["pending_reconciliation_slots"], 0);
    }

    #[test]
    fn bounded_reads_refuse_output_over_the_cap() {
        // Exactly one byte over the cap: the reader stops there and the
        // overrun is refused, not truncated.
        let over = (MAX_PLIST_BYTES + 1).to_string();
        let error = bounded("/usr/bin/head", &["-c", &over, "/dev/zero"], None).unwrap_err();
        assert!(error.contains("over the cap"), "{error}");
    }

    #[test]
    fn bounded_reads_return_small_output() {
        let bytes = bounded("/bin/echo", &["hello"], None).unwrap();
        assert_eq!(bytes, b"hello\n");
    }
}
