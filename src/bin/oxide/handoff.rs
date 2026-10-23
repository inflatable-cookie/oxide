//! One hand-off to the installed `oxide` when this binary is older than the
//! control state it finds (oxide#122).
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Installed-binary
//! hand-off). The check runs in the front door after help, version, `--list`
//! and toolchain handling and before any classified arm opens a store, takes a
//! lock, installs a signal handler, starts Cargo or writes state. It reads at
//! most the legacy state file, the removable pool's state file and the
//! removable registration, and triggers only on a numeric `u32` version above
//! the version this binary understands. Everything else keeps the existing
//! errors: a malformed, foreign, oversized or unreadable document is not proof
//! of a newer format, and later `Store::load` errors are never caught.
//!
//! A hand-off needs the receipt-matching installed binary, reached through
//! `PATH`, whose receipt records capabilities covering every newer version
//! observed. Exactly one `exec` happens, with the caller's arguments and
//! environment plus [`INTERNAL_MARKER`]; the marker makes a successor refuse
//! rather than hand off again. With nothing to hand off to, or a candidate
//! that does not prove itself, the command refuses with exit 75.
//!
//! The final `execve` is by path, so a same-user rename between the last check
//! and the call can still substitute the executed inode. The check is not an
//! atomic bind to the bytes Cargo sees; see the contract's residual note.

use crate::removable::{MAX_REGISTRATION_BYTES, POOL_DIR, REGISTRATION, REGISTRATION_VERSION};
use crate::store::{Store, MAX_STATE_BYTES, STATE_VERSION};
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Added only to an approved hand-off's environment. Its presence prohibits
/// another hand-off; it never authorizes one.
pub const INTERNAL_MARKER: &str = "OXIDE_INTERNAL_HANDED_OFF";
/// Hidden, effect-free probe the installer runs on the authenticated pinned
/// build, so a receipt records that build's own capability numbers.
pub const CAPABILITIES_PROBE: &str = "__oxide-build-capabilities";
/// Exit code for a newer state this binary cannot serve.
pub const REFUSAL_EXIT: u8 = 75;
const REFUSAL_LINE: &str = "oxide: oxide was upgraded during this command; re-run.";
const NOTICE: &str = "oxide: handing off to the installed binary (state is newer than this one).";
/// Control files this check may read.
const STATE_FILE: &str = "state.json";
pub(crate) const RECEIPT_MAX_BYTES: u64 = 64 * 1024;
pub(crate) const RECEIPT_MAX_ENTRIES: usize = 4096;
pub(crate) const CANDIDATE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// `main` answers the installer's capability probe before any other work.
pub fn hidden() -> Option<u8> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != CAPABILITIES_PROBE {
        return None;
    }
    if args.next().is_some() {
        eprintln!("oxide: {CAPABILITIES_PROBE} takes no arguments");
        return Some(2);
    }
    println!(
        "{{\"control_state_version\":{STATE_VERSION},\"registration_version\":{REGISTRATION_VERSION}}}"
    );
    Some(0)
}

/// The front-door check. `None` means "continue as this binary normally
/// would"; `Some(code)` is a finished invocation.
pub fn preflight() -> Option<u8> {
    let control = match Store::existing_control() {
        Ok(Some(control)) => control,
        // No pool or no recognizable control directory: nothing to compare.
        // A read error here is not proof of a newer format.
        Ok(None) | Err(_) => return None,
    };
    let observed = observe(&control);
    if !observed.any() {
        return None;
    }
    let Some(inputs) = Inputs::from_environment() else {
        return Some(refuse());
    };
    match attempt(&inputs, observed) {
        Ok(prepared) => {
            let error = exec(&prepared);
            eprintln!("oxide: could not start the installed oxide: {error}");
            Some(1)
        }
        Err(_reason) => Some(refuse()),
    }
}

fn refuse() -> u8 {
    eprintln!("{REFUSAL_LINE}");
    REFUSAL_EXIT
}

/// Newer numeric versions found on disk. `None` for a file means "no proof".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Observed {
    state: Option<u32>,
    registration: Option<u32>,
}

impl Observed {
    fn any(self) -> bool {
        self.state.is_some() || self.registration.is_some()
    }
}

fn observe(control: &Path) -> Observed {
    let mut state = peek_version(&control.join(STATE_FILE), MAX_STATE_BYTES, STATE_VERSION);
    let removable = peek_version(
        &control.join(POOL_DIR).join(STATE_FILE),
        MAX_STATE_BYTES,
        STATE_VERSION,
    );
    state = state.max(removable);
    Observed {
        state,
        registration: peek_version(
            &control.join(REGISTRATION),
            MAX_REGISTRATION_BYTES,
            REGISTRATION_VERSION,
        ),
    }
}

/// The numeric `version` of one bounded control document, when it is above
/// `current`. Absence, an IO or permission error, an oversized file, a
/// non-object, a non-numeric, non-`u32` or missing version, and anything else
/// means no proof of a newer format.
fn peek_version(path: &Path, cap: u64, current: u32) -> Option<u32> {
    let file = open_control_file(path, cap).ok()?;
    let mut bytes = Vec::new();
    file.take(cap + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > cap {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let version = u32::try_from(value.get("version")?.as_u64()?).ok()?;
    (version > current).then_some(version)
}

/// A private, user-owned regular control file, opened without following a
/// final symlink. `version`'s identity line reads install receipts with the
/// same rule.
pub(crate) fn open_control_file(path: &Path, cap: u64) -> Result<File, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(format!(
            "{} is not an owner-only regular file",
            path.display()
        ));
    }
    if metadata.size() > cap {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(file)
}

/// One receipt's candidate facts.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Receipt {
    sha256: String,
    size: u64,
    state_version: u32,
    registration_version: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

impl Identity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            mtime: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
        }
    }
}

/// Everything the decision needs, injected so unit tests leave the process
/// environment alone.
struct Inputs {
    home: PathBuf,
    path: OsString,
    marker: bool,
    /// Device and inode of the running binary, when known.
    current: Option<(u64, u64)>,
}

impl Inputs {
    fn from_environment() -> Option<Self> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        if !home.is_absolute() {
            return None;
        }
        let current = std::env::current_exe()
            .ok()
            .and_then(|exe| fs::metadata(exe).ok())
            .map(|metadata| (metadata.dev(), metadata.ino()));
        Some(Self {
            home,
            path: std::env::var_os("PATH").unwrap_or_default(),
            marker: std::env::var_os(INTERNAL_MARKER).is_some(),
            current,
        })
    }

    fn destination(&self) -> PathBuf {
        self.home.join(".local/bin/oxide")
    }
}

/// An approved hand-off's verified candidate. The descriptor stays open so
/// the last check compares the very file that was hashed.
#[derive(Debug)]
struct Prepared {
    destination: PathBuf,
    _file: File,
}

/// Why a hand-off was refused. The front door prints one pinned line for all
/// of them; the precise reason stays available to tests and diagnosis.
#[derive(Debug)]
struct Refusal(#[allow(dead_code)] &'static str);

/// Decide whether this invocation hands off. Only a marker, a missing or
/// foreign receipt, a mismatching path, hash, size or capability, an
/// untrusted destination, another `oxide` on `PATH`, the running binary
/// itself, or a replaced candidate refuses.
fn attempt(inputs: &Inputs, observed: Observed) -> Result<Prepared, Refusal> {
    if inputs.marker {
        return Err(Refusal("the internal hand-off marker is already set"));
    }
    let destination = inputs.destination();
    let receipt = newest_receipt(&inputs.home, &destination)?;
    if receipt.state_version < observed.state.unwrap_or(0)
        || receipt.registration_version < observed.registration.unwrap_or(0)
    {
        return Err(Refusal("the receipt does not cover the newer versions"));
    }
    verify_owned_directories(&inputs.home, &destination)?;
    let file = open_candidate(&destination)?;
    let opened = file
        .metadata()
        .map_err(|_| Refusal("the candidate cannot be inspected"))?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !opened.is_file() || opened.uid() != uid || opened.mode() & 0o100 == 0 {
        return Err(Refusal("the candidate is not an owned executable file"));
    }
    if inputs.current == Some((opened.dev(), opened.ino())) {
        return Err(Refusal("the candidate is the running binary"));
    }
    only_candidate_on_path(&inputs.path, (opened.dev(), opened.ino()))?;
    let identity = Identity::of(&opened);
    if identity.size != receipt.size {
        return Err(Refusal("the candidate size differs from the receipt"));
    }
    let digest = digest(&file)?;
    if digest != receipt.sha256 {
        return Err(Refusal("the candidate hash differs from the receipt"));
    }
    #[cfg(debug_assertions)]
    replace_before_final_check(&destination);
    // Last check: the opened file and the path still name one inode with the
    // same metadata. This is not an atomic bind for the following `execve`
    // by path.
    verify_unchanged(&file, &destination, identity)?;
    Ok(Prepared {
        destination,
        _file: file,
    })
}

fn verify_unchanged(file: &File, destination: &Path, identity: Identity) -> Result<(), Refusal> {
    let opened = file
        .metadata()
        .map_err(|_| Refusal("the candidate changed before the hand-off"))?;
    if Identity::of(&opened) != identity {
        return Err(Refusal("the candidate changed before the hand-off"));
    }
    let linked = fs::symlink_metadata(destination)
        .map_err(|_| Refusal("the candidate disappeared before the hand-off"))?;
    if linked.file_type().is_symlink() || Identity::of(&linked) != identity {
        return Err(Refusal("the candidate was replaced before the hand-off"));
    }
    Ok(())
}

/// Copy the caller's arguments and environment, add the internal marker and
/// replace this process. Returns only the `exec` failure.
fn exec(prepared: &Prepared) -> std::io::Error {
    let mut argv = std::env::args_os();
    let arg0 = argv.next().unwrap_or_else(|| OsString::from("oxide"));
    let rest: Vec<OsString> = argv.collect();
    eprintln!("{NOTICE}");
    Command::new(&prepared.destination)
        .arg0(arg0)
        .args(rest)
        .env(INTERNAL_MARKER, "1")
        .exec()
}

/// The newest timestamp-sort receipt in the install directory, with the
/// fields a hand-off needs. Only `*.json` files are considered; the
/// directory is read whole under a fixed entry bound.
fn newest_receipt(home: &Path, destination: &Path) -> Result<Receipt, Refusal> {
    let (_, bytes, _) =
        newest_receipt_bytes(home).map_err(|_| Refusal("the receipt could not be read"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| Refusal("the newest receipt is invalid"))?;
    let installed = value
        .get("installed")
        .filter(|installed| installed.is_object())
        .ok_or(Refusal("the newest receipt has no installed facts"))?;
    let path = installed
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or(Refusal("the newest receipt names no path"))?;
    if Path::new(path) != destination {
        return Err(Refusal("the newest receipt is for another destination"));
    }
    let sha256 = installed
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .ok_or(Refusal("the newest receipt has no hash"))?;
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Refusal(
            "the newest receipt hash is not lowercase SHA-256 hex",
        ));
    }
    let size = installed
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or(Refusal("the newest receipt has no size"))?;
    let state_version = capability(&value, "control_state_version")?;
    let registration_version = capability(&value, "registration_version")?;
    Ok(Receipt {
        sha256: sha256.to_owned(),
        size,
        state_version,
        registration_version,
    })
}

/// The newest receipt's file name, exact bytes and whether it is owner-only.
/// One owned real directory under a fixed entry bound and one bounded,
/// no-follow, owned regular file: the rule the hand-off has always applied to
/// receipts an older installer left at 0644. `version`'s identity line reads
/// the same bytes, so its receipt digest is over exactly what was parsed, and
/// it alone requires the private mode a new receipt gets.
pub(crate) fn newest_receipt_bytes(home: &Path) -> Result<(String, Vec<u8>, bool), String> {
    let directory = home.join(".local/state/oxide/install");
    let metadata =
        fs::symlink_metadata(&directory).map_err(|_| "no install receipt directory".to_owned())?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid {
        return Err("the receipt directory is not an owned real directory".to_owned());
    }
    let mut names = Vec::new();
    let entries = fs::read_dir(&directory).map_err(|_| "cannot read receipts".to_owned())?;
    for (index, entry) in entries.enumerate() {
        if index >= RECEIPT_MAX_ENTRIES {
            return Err("too many receipt directory entries".to_owned());
        }
        let entry = entry.map_err(|_| "cannot read receipts".to_owned())?;
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".json") {
            names.push(name);
        }
    }
    names.sort();
    let name = names.last().ok_or("no install receipt".to_owned())?;
    let path = directory.join(name);
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|_| "cannot open the newest receipt".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "cannot inspect the newest receipt".to_owned())?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.size() > RECEIPT_MAX_BYTES {
        return Err("the newest receipt is not a bounded owned regular file".to_owned());
    }
    let private = metadata.mode() & 0o077 == 0;
    let mut bytes = Vec::new();
    file.take(RECEIPT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read the newest receipt".to_owned())?;
    if bytes.len() as u64 > RECEIPT_MAX_BYTES {
        return Err("the newest receipt exceeds its byte limit".to_owned());
    }
    Ok((name.to_string_lossy().into_owned(), bytes, private))
}

/// A `u32` capability field. A receipt without one cannot prove what the
/// installed binary understands.
fn capability(value: &serde_json::Value, name: &'static str) -> Result<u32, Refusal> {
    value
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
        .ok_or(Refusal(match name {
            "control_state_version" => "the receipt has no control-state capability",
            _ => "the receipt has no registration capability",
        }))
}

/// Every component from `HOME` to the destination's parent is a real
/// directory owned by this user; none below `HOME` is group or world
/// writable. The same predicate the installer applies.
fn verify_owned_directories(home: &Path, destination: &Path) -> Result<(), Refusal> {
    let parent = destination
        .parent()
        .ok_or(Refusal("the destination has no directory"))?;
    let relative = parent
        .strip_prefix(home)
        .map_err(|_| Refusal("the destination is outside HOME"))?;
    let parts: Vec<&OsStr> = relative.iter().collect();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    for count in 0..=parts.len() {
        // `HOME` itself is checked without a trailing separator, so a
        // symlinked home is inspected rather than followed (as the installer
        // does).
        let directory = if count == 0 {
            home.to_path_buf()
        } else {
            home.join(parts[..count].iter().collect::<PathBuf>())
        };
        let metadata = fs::symlink_metadata(&directory)
            .map_err(|_| Refusal("a destination directory is missing"))?;
        let writable = count > 0 && metadata.mode() & 0o022 != 0;
        if !metadata.is_dir() || metadata.uid() != uid || writable {
            return Err(Refusal(
                "a destination directory is not owned only by this user",
            ));
        }
    }
    Ok(())
}

fn open_candidate(destination: &Path) -> Result<File, Refusal> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination)
        .map_err(|_| Refusal("the candidate cannot be opened"))
}

/// Every `oxide` entry on `PATH` must be the same file. A missing entry is
/// not a candidate; another executable file is refused.
fn only_candidate_on_path(path: &OsStr, identity: (u64, u64)) -> Result<(), Refusal> {
    let mut reached = false;
    for directory in std::env::split_paths(path) {
        let directory = if directory.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            directory
        };
        let candidate = directory.join("oxide");
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if (metadata.dev(), metadata.ino()) != identity {
            return Err(Refusal("another oxide is on PATH"));
        }
        reached = true;
    }
    if !reached {
        return Err(Refusal("the installed oxide is not reachable through PATH"));
    }
    Ok(())
}

/// SHA-256 of the opened descriptor, bounded before the whole file is read.
fn digest(file: &File) -> Result<String, Refusal> {
    digest_bounded(file, CANDIDATE_MAX_BYTES)
        .map_err(|_| Refusal("the candidate exceeds its byte limit"))
}

/// SHA-256 of an opened descriptor, counting only up to `cap` bytes. The
/// same reader serves the hand-off and `version`'s installed-binary digest.
pub(crate) fn digest_bounded(file: &File, cap: u64) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut reader = file;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| "the file cannot be read".to_owned())?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > cap {
            return Err("the file exceeds its byte limit".to_owned());
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Debug-only fault injection for the fixture proof: replace the destination
/// between the hash and the final check. It acts only when both paths are
/// inside `OXIDE_TASK_SCRATCH_ROOT`, so no real installation is touched.
#[cfg(debug_assertions)]
fn replace_before_final_check(destination: &Path) {
    let source = std::env::var_os("OXIDE_TEST_HANDOFF_REPLACE").map(PathBuf::from);
    let root = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").map(PathBuf::from);
    let (Some(source), Some(root)) = (source, root) else {
        return;
    };
    let Ok(root) = fs::canonicalize(&root) else {
        return;
    };
    let inside =
        |path: &Path| fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(&root));
    if !inside(&source) || !inside(destination) {
        return;
    }
    let _ = fs::rename(&source, destination);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            loop {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let root =
                    std::env::temp_dir().join(format!("oxide-handoff-{}-{id}", std::process::id()));
                match fs::create_dir(&root) {
                    Ok(()) => {
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
                        return Self { root };
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create hand-off fixture: {error}"),
                }
            }
        }

        fn home(&self) -> PathBuf {
            self.root.join("home")
        }

        fn destination(&self) -> PathBuf {
            self.home().join(".local/bin/oxide")
        }

        fn prepare_home(&self) {
            let bin = self.home().join(".local/bin");
            fs::create_dir_all(&bin).unwrap();
            fs::set_permissions(&self.home(), fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(
                self.home().join(".local"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        }

        fn candidate(&self, bytes: &[u8], mode: u32) -> Identity {
            self.prepare_home();
            let path = self.destination();
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            Identity::of(&fs::metadata(&path).unwrap())
        }

        fn receipt(&self, name: &str, receipt: &str) {
            let directory = self.home().join(".local/state/oxide/install");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join(name), receipt).unwrap();
        }

        fn receipt_for(&self, name: &str, sha256: &str, size: u64, state: u32, registration: u32) {
            let receipt = format!(
                "{{\"installed\":{{\"path\":\"{}\",\"sha256\":\"{sha256}\",\"size\":{size}}},\"control_state_version\":{state},\"registration_version\":{registration}}}",
                self.destination().display()
            );
            self.receipt(name, &receipt);
        }

        fn inputs(&self) -> Inputs {
            Inputs {
                home: self.home(),
                path: self.destination().parent().unwrap().as_os_str().to_owned(),
                marker: false,
                current: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn newer_state() -> Observed {
        Observed {
            state: Some(STATE_VERSION + 1),
            registration: None,
        }
    }

    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn a_numeric_version_above_this_binary_is_the_only_trigger() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir(&control).unwrap();
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        let state = control.join(STATE_FILE);
        let write = |bytes: &str| {
            fs::write(&state, bytes).unwrap();
            fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();
        };
        write(&format!("{{\"version\":{STATE_VERSION}}}"));
        assert_eq!(observe(&control), Observed::default());
        write(&format!("{{\"version\":{}}}", STATE_VERSION + 1));
        assert_eq!(
            observe(&control),
            Observed {
                state: Some(STATE_VERSION + 1),
                registration: None
            }
        );
        for no_proof in [
            "{}".to_owned(),
            "[]".to_owned(),
            "not json".to_owned(),
            format!("{{\"version\":\"{}\"}}", STATE_VERSION + 1),
            "{\"version\":-1}".to_owned(),
            "{\"version\":4294967296}".to_owned(),
            "{\"version\":1.5}".to_owned(),
            format!("{{\"version\":1}}"),
        ] {
            write(&no_proof);
            assert_eq!(observe(&control), Observed::default(), "{no_proof}");
        }
        // A symlinked, oversized or foreign-mode document is not proof.
        fs::remove_file(&state).unwrap();
        let elsewhere = fixture.root.join("elsewhere.json");
        fs::write(&elsewhere, format!("{{\"version\":{}}}", STATE_VERSION + 1)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &state).unwrap();
        assert_eq!(observe(&control), Observed::default());
        fs::remove_file(&state).unwrap();
        write(&format!("{{\"version\":{}}}", STATE_VERSION + 1));
        fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(observe(&control), Observed::default());
    }

    #[test]
    fn a_newer_removable_state_or_registration_triggers_without_the_internal_state() {
        let fixture = Fixture::new();
        let control = fixture.root.join("control");
        fs::create_dir_all(control.join(POOL_DIR)).unwrap();
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(control.join(POOL_DIR), fs::Permissions::from_mode(0o700)).unwrap();
        let removable = control.join(POOL_DIR).join(STATE_FILE);
        fs::write(&removable, format!("{{\"version\":{}}}", STATE_VERSION + 2)).unwrap();
        fs::set_permissions(&removable, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            observe(&control),
            Observed {
                state: Some(STATE_VERSION + 2),
                registration: None
            }
        );
        fs::remove_file(&removable).unwrap();
        let registration = control.join(REGISTRATION);
        fs::write(
            &registration,
            format!("{{\"version\":{}}}", REGISTRATION_VERSION + 1),
        )
        .unwrap();
        fs::set_permissions(&registration, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            observe(&control),
            Observed {
                state: None,
                registration: Some(REGISTRATION_VERSION + 1)
            }
        );
    }

    #[test]
    fn a_matching_receipt_and_candidate_are_prepared_for_one_hand_off() {
        let fixture = Fixture::new();
        let bytes = b"#!/bin/sh\nexit 0\n";
        let candidate = fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "20260101T000000Z-aaaaaaaaaaaa.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        let prepared = attempt(&fixture.inputs(), newer_state()).unwrap();
        assert_eq!(prepared.destination, fixture.destination());
        assert_eq!(
            candidate.dev,
            fs::metadata(fixture.destination()).unwrap().dev()
        );
    }

    #[test]
    fn the_newest_receipt_wins_and_an_older_matching_one_is_not_searched() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "20260101T000000Z-aaaaaaaaaaaa.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        // The newest receipt refuses; the earlier matching one is not used.
        fixture.receipt(
            "20260201T000000Z-bbbbbbbbbbbb.json",
            "{\"installed\":{\"path\":\"/elsewhere/oxide\",\"sha256\":\"x\",\"size\":1}}",
        );
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        let mut inputs = fixture.inputs();
        inputs.marker = true;
        assert!(attempt(&inputs, newer_state()).is_err());
    }

    #[test]
    fn a_receipt_that_does_not_cover_the_newer_version_refuses() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "1.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION,
            REGISTRATION_VERSION,
        );
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        fixture.receipt_for(
            "2.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        assert!(attempt(&fixture.inputs(), newer_state()).is_ok());
    }

    #[test]
    fn a_receipt_without_capabilities_refuses() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        let receipt = format!(
            "{{\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}}}}",
            fixture.destination().display(),
            hash(bytes),
            bytes.len()
        );
        fixture.receipt("1.json", &receipt);
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        let one = format!(
            "{{\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}},\"control_state_version\":3}}",
            fixture.destination().display(),
            hash(bytes),
            bytes.len()
        );
        fixture.receipt("2.json", &one);
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        let stringly = format!(
            "{{\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}},\"control_state_version\":\"3\",\"registration_version\":2}}",
            fixture.destination().display(),
            hash(bytes),
            bytes.len()
        );
        fixture.receipt("3.json", &stringly);
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
    }

    #[test]
    fn a_mismatching_hash_size_or_shape_refuses() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        for (sha256, size) in [
            (hash(b"other"), bytes.len() as u64),
            (hash(bytes), bytes.len() as u64 + 1),
        ] {
            fixture.receipt_for(
                "1.json",
                &sha256,
                size,
                STATE_VERSION + 1,
                REGISTRATION_VERSION,
            );
            assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        }
        // A non-executable or symlinked candidate refuses.
        fixture.receipt_for(
            "2.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o755)).unwrap();
        let real = fixture.destination();
        let elsewhere = fixture.root.join("elsewhere");
        fs::rename(&real, &elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &real).unwrap();
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
    }

    #[test]
    fn the_running_binary_and_a_missing_path_entry_refuse() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        let candidate = fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "1.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        let mut inputs = fixture.inputs();
        inputs.current = Some((candidate.dev, candidate.ino));
        assert!(attempt(&inputs, newer_state()).is_err());
        let mut inputs = fixture.inputs();
        inputs.path = fixture.root.join("empty").into_os_string();
        assert!(attempt(&inputs, newer_state()).is_err());
        // Another oxide earlier on PATH refuses even when the destination is
        // also reachable.
        let other = fixture.root.join("other");
        fs::create_dir(&other).unwrap();
        fs::write(other.join("oxide"), b"other").unwrap();
        fs::set_permissions(other.join("oxide"), fs::Permissions::from_mode(0o755)).unwrap();
        let mut joined = other.clone().into_os_string();
        joined.push(":");
        joined.push(fixture.destination().parent().unwrap());
        let mut inputs = fixture.inputs();
        inputs.path = joined;
        assert!(attempt(&inputs, newer_state()).is_err());
    }

    #[test]
    fn a_linked_or_writable_home_directory_refuses() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "1.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        fs::set_permissions(
            fixture.home().join(".local/bin"),
            fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
        fs::set_permissions(
            fixture.home().join(".local/bin"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        // A symlinked `.local` refuses.
        let bin = fixture.home().join(".local/bin");
        let moved = fixture.root.join("moved-bin");
        fs::rename(&bin, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &bin).unwrap();
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
    }

    #[test]
    fn a_symlinked_home_refuses() {
        let fixture = Fixture::new();
        let bytes = b"candidate";
        fixture.candidate(bytes, 0o755);
        fixture.receipt_for(
            "1.json",
            &hash(bytes),
            bytes.len() as u64,
            STATE_VERSION + 1,
            REGISTRATION_VERSION,
        );
        let home = fixture.home();
        let real = fixture.root.join("real-home");
        fs::rename(&home, &real).unwrap();
        std::os::unix::fs::symlink(&real, &home).unwrap();
        assert!(attempt(&fixture.inputs(), newer_state()).is_err());
    }
}
