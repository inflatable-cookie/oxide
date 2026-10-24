//! Where a leased command's children put temporary files (oxide#136).
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md#child-temporary-files`.
//! One rule decides the whole child tree's temporary directory:
//!
//! - an inherited `TMPDIR` that canonically resolves to an existing,
//!   user-writable directory on the same device as the selected pool is
//!   retained for every child, and nothing of Oxide's is created or cleaned;
//! - otherwise the command gets one Oxide-owned directory inside the held
//!   slot (`<slot>/scratch`), created and removed under the exclusive lease,
//!   after positive writer closure.
//!
//! `TMPDIR`, `TMP` and `TEMP` are set to the chosen value on every child
//! Oxide launches for a leased command, so descendants (test and `run`
//! programs, build scripts, tools under `oxide exec`) inherit it. Oxide never
//! writes management data into, or removes, an inherited directory: it is not
//! Oxide's, and the owning tool keeps its own retention rules. Effect-free
//! commands (plain passthrough, probes, `status`) create nothing.
//!
//! The owned directory is bound to pool identity, slot and incarnation by a
//! marker file written before any writer starts, so a later lease of the same
//! incarnation may reconcile it and no other shape is ever deleted. It lives
//! inside the slot, so the existing whole-slot accounting counts it once and
//! whole-slot eviction, crash recovery and lost-volume retirement remove or
//! keep it with the slot they already own.
//!
//! Path length is a hard limit, not a preference: keep the injected path
//! short enough for a Unix socket. A longer derived path is reported and not
//! used rather than claimed to fit every configured pool.

use crate::wrapper;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

/// The Oxide-owned directory inside the held slot.
const SCRATCH_NAME: &str = "scratch";
/// Ownership marker inside that directory, written before any writer starts.
const OWNER_NAME: &str = "owner";
const OWNER_VERSION: &str = "oxide.scratch/1";
const MAX_OWNER_BYTES: u64 = 1024;
/// Variables a child's temporary directory is published through.
pub(crate) const VARIABLES: [&str; 3] = ["TMPDIR", "TMP", "TEMP"];
/// Longest owned scratch path Oxide injects. macOS `sun_path` is 104 bytes
/// including the terminating NUL; a Unix socket needs a `/` and a short name
/// inside the directory, so 91 + 1 + 11 + NUL is the whole budget.
pub(crate) const MAX_SCRATCH_PATH_BYTES: usize = 91;

/// The durable binding of one owned scratch directory: the pool's recorded
/// store identity, the slot number and its incarnation. The lease that
/// created it is recorded for evidence only; the binding decides ownership
/// because no two commands ever hold one slot at the same time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Identity {
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
}

impl Identity {
    fn binding(&self) -> (&str, &str, &str) {
        (&self.store_id, &self.slot_id, &self.incarnation)
    }
}

/// One owned per-command temporary directory.
#[derive(Clone, Debug)]
pub(crate) struct Owned {
    path: std::path::PathBuf,
    /// The device the directory was created on; removal refuses another.
    dev: u64,
    identity: Identity,
}

impl Owned {
    #[cfg(test)]
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// What a leased command's children get.
#[derive(Clone, Debug)]
pub(crate) enum Chosen {
    /// Retained inherited `TMPDIR`, byte for byte. Oxide owns nothing.
    Inherited(OsString),
    /// Oxide's own per-command directory inside the held slot.
    Oxide(Owned),
    /// Neither was usable; children keep this machine's default. The reason
    /// was already reported; the command is not failed for it.
    Unavailable(String),
}

impl Chosen {
    /// The bounded, escaped summary and status fragment, or nothing when the
    /// contract could not be applied.
    pub(crate) fn note(&self) -> Option<String> {
        match self {
            Self::Inherited(value) => Some(format!(
                "temp: inherited {}",
                wrapper::shown(&value.to_string_lossy())
            )),
            Self::Oxide(owned) => Some(format!(
                "temp: oxide {}",
                wrapper::shown(&owned.path.to_string_lossy())
            )),
            Self::Unavailable(_) => None,
        }
    }

    /// The owned directory, when this command owns one.
    pub(crate) fn owned(&self) -> Option<Owned> {
        match self {
            Self::Oxide(owned) => Some(owned.clone()),
            Self::Inherited(_) | Self::Unavailable(_) => None,
        }
    }

    /// Publish the choice on one child's environment.
    pub(crate) fn apply(&self, command: &mut std::process::Command) {
        let value: &OsStr = match self {
            Self::Inherited(value) => value.as_os_str(),
            Self::Oxide(owned) => owned.path.as_os_str(),
            Self::Unavailable(_) => return,
        };
        for name in VARIABLES {
            command.env(name, value);
        }
    }

    /// The one-line warning for an unusable choice, printed once per command.
    pub(crate) fn warning(&self) -> Option<String> {
        match self {
            Self::Unavailable(reason) => Some(format!(
                "{reason}; child temporary files keep this machine's default"
            )),
            Self::Inherited(_) | Self::Oxide(_) => None,
        }
    }
}

/// Choose the child temporary directory for one leased command.
///
/// `inherited` is the caller's `TMPDIR`, `pool` the selected pool's payload
/// root (its device is the pool's) and `slot` the held slot directory.
pub(crate) fn choose(
    inherited: Option<OsString>,
    pool: &std::path::Path,
    slot: &std::path::Path,
    identity: Identity,
) -> Chosen {
    choose_with(inherited, pool, slot, identity, MAX_SCRATCH_PATH_BYTES)
}

/// [`choose`] with an explicit path bound. The bound is a parameter so a
/// unit test's fixture root length cannot silently change which branch runs;
/// every product call uses [`MAX_SCRATCH_PATH_BYTES`].
pub(crate) fn choose_with(
    inherited: Option<OsString>,
    pool: &std::path::Path,
    slot: &std::path::Path,
    identity: Identity,
    bound: usize,
) -> Chosen {
    let pool_dev = match fs::metadata(pool) {
        Ok(meta) => meta.dev(),
        Err(error) => {
            return Chosen::Unavailable(format!(
                "cannot read the selected pool {}: {error}",
                pool.display()
            ))
        }
    };
    if let Some(value) = inherited {
        if inheritable(&value, pool_dev) {
            return Chosen::Inherited(value);
        }
    }
    match prepare(slot, pool_dev, &identity, bound) {
        Ok(owned) => Chosen::Oxide(owned),
        Err(reason) => Chosen::Unavailable(reason),
    }
}

/// Whether an inherited `TMPDIR` is kept: it must canonically resolve to an
/// existing directory that this user can write and that lives on the pool's
/// own device. No prefix comparison, no mount mutation.
pub(crate) fn inheritable(value: &OsStr, pool_dev: u64) -> bool {
    if value.is_empty() {
        return false;
    }
    let Ok(canonical) = fs::canonicalize(value) else {
        return false;
    };
    let Ok(meta) = fs::metadata(&canonical) else {
        return false;
    };
    meta.is_dir() && meta.dev() == pool_dev && writable(&canonical)
}

/// Effective-user write access, without touching anything.
fn writable(path: &std::path::Path) -> bool {
    let Ok(text) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `text` is a valid NUL-terminated path and the call only asks
    // the kernel about access.
    unsafe { libc::faccessat(libc::AT_FDCWD, text.as_ptr(), libc::W_OK, libc::AT_EACCESS) == 0 }
}

fn reported(path: &std::path::Path) -> String {
    wrapper::shown(&path.to_string_lossy())
}

/// Create (or reconcile this incarnation's leftover) owned scratch inside the
/// held slot. Every failure is a refusal to redirect, never a deletion of
/// something Oxide cannot positively identify.
fn prepare(
    slot: &std::path::Path,
    pool_dev: u64,
    identity: &Identity,
    bound: usize,
) -> Result<Owned, String> {
    let slot_meta = fs::symlink_metadata(slot)
        .map_err(|error| format!("cannot read the held slot {}: {error}", reported(slot)))?;
    if !slot_meta.is_dir() || slot_meta.file_type().is_symlink() {
        return Err(format!(
            "the held slot {} is not a real directory",
            reported(slot)
        ));
    }
    if slot_meta.uid() != effective_uid() {
        return Err(format!(
            "the held slot {} is not owned by this user",
            reported(slot)
        ));
    }
    if slot_meta.dev() != pool_dev {
        return Err(format!(
            "the held slot {} is not on the selected pool's device",
            reported(slot)
        ));
    }
    let path = slot.join(SCRATCH_NAME);
    let bytes = path.as_os_str().as_bytes().len();
    if bytes > bound {
        return Err(format!(
            "the per-command temporary directory {} is {bytes} bytes, over the {bound}-byte Unix-socket bound",
            reported(&path)
        ));
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "cannot read the per-command temporary directory {}: {error}",
                reported(&path)
            ))
        }
        Ok(meta) => {
            // A directory inside the slot this lease now owns can only be a
            // previous command's leftover: reconcile it when, and only when,
            // its marker binds it to this same slot incarnation.
            if let Err(reason) = owned_directory(&path, &meta, pool_dev) {
                return Err(reason);
            }
            let text = read_owner(&path)?;
            let found = parse_owner(&text)
                .ok_or_else(|| format!("{} has no Oxide ownership marker", reported(&path)))?;
            if found.binding() != identity.binding() {
                return Err(format!(
                    "{} belongs to another slot incarnation, so it is kept",
                    reported(&path)
                ));
            }
            remove_owned_tree(&path, pool_dev)?;
        }
    }
    DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|error| {
            format!(
                "cannot create the per-command temporary directory {}: {error}",
                reported(&path)
            )
        })?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path.join(OWNER_NAME))
        .map_err(|error| {
            let _ = fs::remove_dir_all(&path);
            format!(
                "cannot record the per-command temporary directory {}: {error}",
                reported(&path)
            )
        })?;
    let text = owner_text(identity);
    let mut file = file;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(error) = written {
        let _ = fs::remove_dir_all(&path);
        return Err(format!(
            "cannot record the per-command temporary directory {}: {error}",
            reported(&path)
        ));
    }
    Ok(Owned {
        path,
        dev: pool_dev,
        identity: identity.clone(),
    })
}

/// Remove one owned directory after positive writer closure. Returns a
/// warning instead of failing the command; the slot's own accounting and
/// deletion are unchanged, so a kept leftover is never counted twice.
pub(crate) fn discard(owned: &Owned) -> Option<String> {
    match remove(&owned.path, owned.dev, &owned.identity) {
        Ok(()) => None,
        Err(reason) => Some(reason),
    }
}

/// Remove this command's scratch, refusing anything that is no longer the
/// positively identified directory (marker, owner, device, real directory).
pub(crate) fn remove(path: &std::path::Path, dev: u64, identity: &Identity) -> Result<(), String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot read the per-command temporary directory {}: {error}",
                reported(path)
            ))
        }
    };
    owned_directory(path, &meta, dev)?;
    let text = read_owner(path)?;
    let found = parse_owner(&text).ok_or_else(|| {
        format!(
            "{} has no Oxide ownership marker, so it is kept",
            reported(path)
        )
    })?;
    if found.binding() != identity.binding() {
        return Err(format!(
            "{} belongs to another slot incarnation, so it is kept",
            reported(path)
        ));
    }
    remove_owned_tree(path, dev)
}

/// A real directory owned by this user, on the expected device.
fn owned_directory(path: &std::path::Path, meta: &fs::Metadata, dev: u64) -> Result<(), String> {
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(format!(
            "{} is not a real directory, so it is kept",
            reported(path)
        ));
    }
    if meta.uid() != effective_uid() {
        return Err(format!(
            "{} is not owned by this user, so it is kept",
            reported(path)
        ));
    }
    if meta.dev() != dev {
        return Err(format!(
            "{} is not on the pool's device, so it is kept",
            reported(path)
        ));
    }
    Ok(())
}

/// Read the marker without following a link and without trusting a size the
/// caller did not write.
fn read_owner(dir: &std::path::Path) -> Result<String, String> {
    let path = dir.join(OWNER_NAME);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    let meta = file
        .metadata()
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    if !meta.is_file() || meta.uid() != effective_uid() || meta.len() > MAX_OWNER_BYTES {
        return Err(format!(
            "the marker {} is not a bounded file of this user",
            reported(&path)
        ));
    }
    let mut text = String::new();
    file.take(MAX_OWNER_BYTES)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read the marker {}: {error}", reported(&path)))?;
    Ok(text)
}

fn owner_text(identity: &Identity) -> String {
    format!(
        "{OWNER_VERSION}\nstore={}\nslot={}\nincarnation={}\nlease={}\n",
        identity.store_id, identity.slot_id, identity.incarnation, identity.lease_id
    )
}

fn parse_owner(text: &str) -> Option<Identity> {
    let mut lines = text.lines();
    if lines.next()? != OWNER_VERSION {
        return None;
    }
    let mut field = |name: &str| -> Option<String> {
        let line = lines.next()?;
        line.strip_prefix(name)?
            .strip_prefix('=')
            .map(str::to_owned)
    };
    let store_id = field("store")?;
    let slot_id = field("slot")?;
    let incarnation = field("incarnation")?;
    let lease_id = field("lease")?;
    if lines.next().is_some() {
        return None;
    }
    Some(Identity {
        store_id,
        slot_id,
        incarnation,
        lease_id,
    })
}

/// Remove one owned tree without following a link and without crossing to
/// another device. A child on another device (a mount point inside the
/// scratch) refuses and is kept.
fn remove_owned_tree(path: &std::path::Path, dev: u64) -> Result<(), String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(format!(
                "cannot read the per-command temporary directory {}: {error}",
                reported(path)
            ))
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Err(format!(
                    "cannot read the per-command temporary directory {}: {error}",
                    reported(path)
                ))
            }
        };
        let child = entry.path();
        let meta = match fs::symlink_metadata(&child) {
            Ok(meta) => meta,
            Err(error) => return Err(format!("cannot read {}: {error}", reported(&child))),
        };
        if meta.dev() != dev {
            return Err(format!(
                "{} is on another device, so the directory is kept",
                reported(&child)
            ));
        }
        if meta.is_dir() && !meta.file_type().is_symlink() {
            remove_owned_tree(&child, dev)?;
        } else if let Err(error) = fs::remove_file(&child) {
            return Err(format!("cannot remove {}: {error}", reported(&child)));
        }
    }
    fs::remove_dir(path).map_err(|error| {
        format!(
            "cannot remove the per-command temporary directory {}: {error}",
            reported(path)
        )
    })
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The status line for the selected pool's device, without creating
/// anything: the decision `choose` would make for a leased command there.
pub(crate) fn describe(inherited: Option<&OsStr>, pool_dev: u64) -> String {
    match inherited.filter(|value| inheritable(value, pool_dev)) {
        Some(value) => format!(
            "temp: inherited {}",
            wrapper::shown(&value.to_string_lossy())
        ),
        None => "temp: oxide (per leased command, inside the selected pool's slot)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    /// A fresh private fixture directory under the test temp root.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(label: &str) -> Self {
            // Short, like a real pool: the fixture must leave room for a
            // slot path under the injected temporary-directory bound.
            let base = std::env::temp_dir().join(format!(
                "ot{}-{}-{label}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&base).expect("fixture root");
            Self(fs::canonicalize(&base).expect("canonical fixture root"))
        }

        fn pool(&self) -> PathBuf {
            let pool = self.0.join("payload");
            fs::create_dir_all(&pool).expect("payload");
            pool
        }

        fn slot(&self, pool: &std::path::Path) -> PathBuf {
            let slot = pool.join("slots").join("oxide").join("quick-1");
            fs::create_dir_all(slot.join("build")).expect("slot");
            slot
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The plan under test with a fixture-safe path bound: the ambient
    /// temporary root is long, and these cases are about ownership and
    /// lifecycle, not the length limit.
    fn choose(
        inherited: Option<OsString>,
        pool: &std::path::Path,
        slot: &std::path::Path,
        identity: Identity,
    ) -> Chosen {
        choose_with(inherited, pool, slot, identity, 4096)
    }

    fn identity(incarnation: &str, lease: &str) -> Identity {
        Identity {
            store_id: "store".to_owned(),
            slot_id: "1".to_owned(),
            incarnation: incarnation.to_owned(),
            lease_id: lease.to_owned(),
        }
    }

    fn device(path: &std::path::Path) -> u64 {
        fs::metadata(path).expect("metadata").dev()
    }

    #[test]
    fn an_unset_or_unusable_tmpdir_gets_one_owned_directory_in_the_slot() {
        let fixture = Fixture::new("owned");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = choose(None, &pool, &slot, identity("1", "lease-a"));
        let owned = chosen.owned().expect("owned directory");
        assert_eq!(owned.path(), slot.join("scratch"));
        assert!(owned.path().is_dir());
        assert_eq!(
            owned.path().metadata().expect("metadata").mode() & 0o777,
            0o700
        );
        assert_eq!(
            chosen.note().as_deref(),
            Some(format!("temp: oxide {}", owned.path().display()).as_str())
        );
        let mut command = std::process::Command::new("true");
        chosen.apply(&mut command);
        let envs: std::collections::HashMap<_, _> = command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.expect("set").to_string_lossy().into_owned(),
                )
            })
            .collect();
        for name in VARIABLES {
            assert_eq!(envs.get(name), Some(&owned.path().display().to_string()));
        }
    }

    #[test]
    fn an_inherited_tmpdir_on_the_pool_device_is_retained_and_never_owned() {
        let fixture = Fixture::new("inherited");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let inherited = pool.join("effigy-run");
        fs::create_dir_all(&inherited).expect("inherited");
        let value = inherited.clone().into_os_string();
        let chosen = choose(Some(value.clone()), &pool, &slot, identity("1", "lease-a"));
        match &chosen {
            Chosen::Inherited(retained) => assert_eq!(retained, &value),
            other => panic!("expected the inherited directory, got {other:?}"),
        }
        assert!(chosen.owned().is_none());
        // Nothing was created inside the slot and nothing inside the
        // inherited directory.
        assert!(!slot.join(SCRATCH_NAME).exists());
        assert_eq!(fs::read_dir(&inherited).expect("read").count(), 0);
        assert!(chosen
            .note()
            .as_deref()
            .is_some_and(|note| note.starts_with("temp: inherited ")));
        // Aliases follow the chosen value, not their own.
        let mut command = std::process::Command::new("true");
        command.env("TMP", "/somewhere/else");
        chosen.apply(&mut command);
        let tmp = command
            .get_envs()
            .find(|(name, _)| *name == "TMP")
            .and_then(|(_, value)| value)
            .map(|value| value.to_owned());
        assert_eq!(tmp, Some(value));
    }

    #[test]
    fn a_tmpdir_on_another_device_missing_or_unwritable_is_replaced() {
        let fixture = Fixture::new("replaced");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let other = std::env::temp_dir();
        // The test scratch root and the pool are normally on one device; an
        // absent path and an explicit empty value are refused regardless.
        assert!(!inheritable(OsStr::new(""), device(&pool)));
        assert!(!inheritable(
            &pool.join("missing").into_os_string(),
            device(&pool)
        ));
        assert!(!inheritable(OsStr::new("relative/tmp"), device(&pool)));
        let replaced = choose(
            Some(pool.join("missing").into_os_string()),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        assert!(replaced.owned().is_some());
        if device(&other) != device(&pool) {
            assert!(!inheritable(other.as_os_str(), device(&pool)));
            let other_device = choose(
                Some(other.clone().into_os_string()),
                &pool,
                &slot,
                identity("1", "lease-a"),
            );
            assert!(other_device.owned().is_some());
        }
        let unreadable = fixture.0.join("closed");
        fs::create_dir_all(&unreadable).expect("closed");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o500)).expect("chmod");
        let unwritable = inheritable(unreadable.as_os_str(), device(&pool));
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o700)).expect("chmod");
        assert!(
            !unwritable,
            "a directory this user cannot write is replaced"
        );
    }

    #[test]
    fn a_symlink_escape_is_replaced_but_a_same_device_target_is_kept() {
        let fixture = Fixture::new("symlink");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let real = pool.join("real-tmp");
        fs::create_dir_all(&real).expect("real");
        let link = fixture.0.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        assert!(inheritable(link.as_os_str(), device(&pool)));
        let escaped = fixture.0.join("escaped");
        std::os::unix::fs::symlink(pool.join("absent"), &escaped).expect("symlink");
        assert!(!inheritable(escaped.as_os_str(), device(&pool)));
        let chosen = choose(
            Some(escaped.into_os_string()),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        assert!(chosen.owned().is_some());
    }

    #[test]
    fn an_inherited_directory_is_never_written_or_cleaned_by_oxide() {
        let fixture = Fixture::new("untouched");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let inherited = pool.join("effigy-run");
        fs::create_dir_all(&inherited).expect("inherited");
        let keep = inherited.join("payload");
        fs::write(&keep, b"evidence").expect("payload");
        let chosen = choose(
            Some(inherited.clone().into_os_string()),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        assert!(matches!(chosen, Chosen::Inherited(_)));
        assert_eq!(fs::read(&keep).expect("read"), b"evidence");
        assert_eq!(fs::read_dir(&inherited).expect("read").count(), 1);
    }

    #[test]
    fn the_owner_marker_binds_pool_slot_and_incarnation() {
        let fixture = Fixture::new("marker");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = choose(None, &pool, &slot, identity("7", "lease-a"));
        let owned = chosen.owned().expect("owned");
        let text = fs::read_to_string(owned.path().join(OWNER_NAME)).expect("marker");
        let parsed = parse_owner(&text).expect("parse");
        assert_eq!(parsed.binding(), ("store", "1", "7"));
        assert_eq!(parsed.lease_id, "lease-a");
        assert_eq!(parse_owner(""), None);
        assert_eq!(parse_owner("oxide.scratch/1\nstore=x\n"), None);
        assert_eq!(
            parse_owner("oxide.scratch/2\nstore=a\nslot=1\nincarnation=1\nlease=l\n"),
            None
        );
        assert_eq!(
            parse_owner("oxide.scratch/1\nstore=a\nslot=1\nincarnation=1\nlease=l\nextra=1\n"),
            None
        );
    }

    #[test]
    fn a_same_incarnation_leftover_is_reconciled_and_another_incarnation_is_kept() {
        let fixture = Fixture::new("reconcile");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let first = choose(None, &pool, &slot, identity("1", "lease-a"));
        let owned = first.owned().expect("owned");
        fs::write(owned.path().join("leftover"), b"x").expect("leftover");
        // The lease that created it may remove it; ownership binds to the
        // slot and incarnation, not to one lease ID.
        assert!(remove(owned.path(), owned.dev, &identity("1", "lease-b")).is_ok());
        assert!(!owned.path().exists());

        // A second command of the same incarnation creates its own.
        let again = choose(None, &pool, &slot, identity("1", "lease-b"));
        let owned = match &again {
            Chosen::Oxide(owned) => owned.clone(),
            other => panic!("expected a fresh owned directory, got {other:?}"),
        };
        fs::write(owned.path().join("leftover"), b"x").expect("leftover");
        assert!(discard(&owned).is_none());
        assert!(!owned.path().exists());

        // A leftover bound to another incarnation is refused, not deleted.
        let scratch = slot.join(SCRATCH_NAME);
        fs::create_dir_all(&scratch).expect("leftover");
        fs::write(scratch.join("leftover"), b"keep").expect("leftover");
        fs::write(
            scratch.join(OWNER_NAME),
            owner_text(&identity("2", "lease-c")).as_bytes(),
        )
        .expect("marker");
        match choose(None, &pool, &slot, identity("3", "lease-d")) {
            Chosen::Unavailable(reason) => {
                assert!(reason.contains("another slot incarnation"), "{reason}")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(fs::read(scratch.join("leftover")).expect("read"), b"keep");

        // The incarnation that owns it reconciles it as its own.
        let owned = match choose(None, &pool, &slot, identity("2", "lease-e")) {
            Chosen::Oxide(owned) => owned,
            other => panic!("expected reconciliation by the owning incarnation, got {other:?}"),
        };
        assert!(!owned.path().join("leftover").exists());
        assert!(discard(&owned).is_none());
        assert!(!owned.path().exists());
    }

    #[test]
    fn a_foreign_entry_in_the_scratch_position_is_kept() {
        let fixture = Fixture::new("foreign");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let scratch = slot.join(SCRATCH_NAME);
        fs::create_dir_all(&scratch).expect("scratch");
        fs::write(scratch.join("not-ours"), b"keep").expect("foreign");
        let chosen = choose(None, &pool, &slot, identity("1", "lease-a"));
        match chosen {
            Chosen::Unavailable(reason) => {
                assert!(reason.contains("marker"), "{reason}")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(fs::read(scratch.join("not-ours")).expect("read"), b"keep");

        // A link where the directory belongs is refused, never followed.
        let fixture = Fixture::new("linked");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let elsewhere = pool.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        fs::write(elsewhere.join("keep"), b"keep").expect("keep");
        std::os::unix::fs::symlink(&elsewhere, slot.join(SCRATCH_NAME)).expect("symlink");
        let chosen = choose(None, &pool, &slot, identity("1", "lease-a"));
        assert!(matches!(chosen, Chosen::Unavailable(_)));
        assert_eq!(fs::read(elsewhere.join("keep")).expect("read"), b"keep");
    }

    #[test]
    fn a_path_over_the_socket_bound_is_reported_not_used() {
        let fixture = Fixture::new("long");
        let pool = fixture.pool();
        let slot = pool.join("slots").join("x".repeat(40)).join("quick-1");
        fs::create_dir_all(slot.join("build")).expect("slot");
        let chosen = choose_with(
            None,
            &pool,
            &slot,
            identity("1", "lease-a"),
            MAX_SCRATCH_PATH_BYTES,
        );
        match chosen {
            Chosen::Unavailable(ref reason) => {
                assert!(reason.contains("Unix-socket bound"), "{reason}");
                assert!(reason.contains("bytes"), "{reason}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(!slot.join(SCRATCH_NAME).exists());
        assert!(chosen.note().is_none());
        assert!(chosen
            .warning()
            .is_some_and(|text| text.contains("default")));
    }

    #[test]
    fn removal_refuses_a_device_that_does_not_match_the_recorded_one() {
        let fixture = Fixture::new("device");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = choose(None, &pool, &slot, identity("1", "lease-a"));
        let owned = chosen.owned().expect("owned");
        let wrong = owned.dev.wrapping_add(1);
        let reason = remove(owned.path(), wrong, &identity("1", "lease-a")).expect_err("refuse");
        assert!(reason.contains("not on the pool's device"), "{reason}");
        assert!(owned.path().exists());
        assert!(discard(&owned).is_none());
        assert!(!owned.path().exists());
    }

    #[test]
    fn a_hard_linked_file_is_removed_once() {
        let fixture = Fixture::new("hardlink");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = choose(None, &pool, &slot, identity("1", "lease-a"));
        let owned = chosen.owned().expect("owned");
        let first = owned.path().join("a");
        fs::write(&first, b"x").expect("write");
        fs::hard_link(&first, owned.path().join("b")).expect("hard link");
        assert!(discard(&owned).is_none());
        assert!(!owned.path().exists());
    }

    #[test]
    fn a_slot_on_another_device_or_a_link_is_refused() {
        let fixture = Fixture::new("slot");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let link = fixture.0.join("slot-link");
        std::os::unix::fs::symlink(&slot, &link).expect("symlink");
        let chosen = choose(None, &pool, &link, identity("1", "lease-a"));
        match chosen {
            Chosen::Unavailable(reason) => {
                assert!(reason.contains("not a real directory"), "{reason}")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        let elsewhere = pool.parent().expect("parent").join("other-slot");
        fs::create_dir_all(&elsewhere).expect("other slot");
        let chosen = choose(None, &pool, &elsewhere, identity("1", "lease-a"));
        if device(&elsewhere) != device(&pool) {
            assert!(matches!(chosen, Chosen::Unavailable(_)));
        }
    }

    #[test]
    fn the_status_line_names_the_decision_without_creating_anything() {
        let fixture = Fixture::new("status");
        let pool = fixture.pool();
        let inherited = pool.join("effigy-run");
        fs::create_dir_all(&inherited).expect("inherited");
        assert_eq!(
            describe(Some(inherited.as_os_str()), device(&pool)),
            format!("temp: inherited {}", inherited.display())
        );
        assert_eq!(fs::read_dir(&inherited).expect("read").count(), 0);
        assert!(describe(None, device(&pool)).starts_with("temp: oxide"));
        assert!(describe(Some(OsStr::new("")), device(&pool)).starts_with("temp: oxide"));
    }

    #[test]
    fn the_note_is_one_escaped_bounded_line() {
        let fixture = Fixture::new("note");
        let pool = fixture.pool();
        let slot = fixture.slot(&pool);
        let chosen = choose(
            Some(OsString::from("/tmp/with\nnewline")),
            &pool,
            &slot,
            identity("1", "lease-a"),
        );
        // Not on the pool device (and not a directory), so it is replaced.
        assert!(chosen.owned().is_some());
        let owned = Chosen::Inherited(OsString::from("/some/very/long/\npath"));
        let note = owned.note().expect("note");
        assert!(!note.contains('\n'), "{note}");
        assert!(note.starts_with("temp: inherited /some/very/long/\\npath"));
    }

    #[test]
    fn the_socket_bound_is_below_the_platform_path() {
        // macOS `sun_path` is 104 bytes including the NUL; Linux is 108.
        assert!(MAX_SCRATCH_PATH_BYTES + 1 + 11 + 1 <= 104);
    }
}
