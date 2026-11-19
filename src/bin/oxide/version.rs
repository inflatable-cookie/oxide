//! `oxide --version` / `oxide -V`: this build's identity, its actual source
//! pin and whether it is the installer's authenticated artifact, then Cargo's
//! own version.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Version identity).
//! The installer stamps `OXIDE_BUILD_PIN` and a fresh `OXIDE_BUILD_INSTANCE`
//! into the build, which `option_env!` embeds below; Cargo's dep-info tracks
//! those names, so a changed stamp recompiles. A build without a well-formed
//! stamp says `unknown` rather than guessing at the checkout it might be in.
//!
//! Verified provenance needs all of: a well-formed embedded pin and instance;
//! the newest receipt in this HOME under the hand-off's bounded, no-follow,
//! owner-only reader; that receipt's `pin` and `build_instance` equal the
//! embedded ones and its `installed` facts name the canonical running
//! executable; the destination is a stable, private, owned regular file whose
//! size and SHA-256 equal the receipt's. The receipt digest is over exactly
//! the receipt bytes that were parsed, and is reported separately from the
//! binary digest. Anything uncertain — a missing, stale, foreign, malformed,
//! oversized or replaced receipt, a symlinked destination, a different pin or
//! instance, a hash or size mismatch — is `unverified`, never the current
//! checkout's HEAD. Nothing here reads a pool, discovers a workspace, runs
//! `git` or touches the network.

use crate::handoff::{self, CANDIDATE_MAX_BYTES};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

// The installer sets `OXIDE_BUILD_PIN` (the authenticated source commit) and
// `OXIDE_BUILD_INSTANCE` (a fresh 128-bit nonce per build, so a rebuilt
// binary with the same pin never matches an older receipt) for the build it
// authenticates. `option_env!` embeds them, and Cargo's dep-info tracks both
// names, so a changed stamp recompiles.
const EMBEDDED_PIN: Option<&str> = option_env!("OXIDE_BUILD_PIN");
const EMBEDDED_INSTANCE: Option<&str> = option_env!("OXIDE_BUILD_INSTANCE");

/// The hidden, effect-free probe the installer runs on the built binary, so
/// it only writes a receipt for a build whose stamp is the one it supplied.
pub const IDENTITY_PROBE: &str = "__oxide-build-identity";

/// `main` answers the installer's build-identity probe before any other work.
pub fn hidden() -> Option<u8> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != IDENTITY_PROBE {
        return None;
    }
    if args.next().is_some() {
        eprintln!("oxide: {IDENTITY_PROBE} takes no arguments");
        return Some(2);
    }
    let stamp = |value: Option<&str>| match value {
        Some(value) => serde_json::Value::String(value.to_owned()).to_string(),
        None => "null".to_owned(),
    };
    println!(
        "{{\"build_pin\":{},\"build_instance\":{}}}",
        stamp(EMBEDDED_PIN),
        stamp(EMBEDDED_INSTANCE)
    );
    Some(0)
}

/// Print the identity lines and flush them before Cargo runs. Write and flush
/// errors (a closed pipe above all) are ignored: Cargo still owns stdout,
/// stderr, signals and the exit status.
pub fn print_identity() {
    let mut text = format!(
        "oxide {} (package {})\n",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_NAME")
    );
    match (wellformed_pin(EMBEDDED_PIN), wellformed_instance(EMBEDDED_INSTANCE)) {
        (Some(pin), Some(instance)) => {
            text.push_str(&format!(
                "  build: source pin {pin}, installer instance {instance}\n"
            ));
            let running = std::env::current_exe().ok();
            let home = crate::config::Facts::from_env().account_home();
            let provenance = provenance_in(running.as_deref(), home.as_deref(), pin, instance);
            text.push_str(&format!("  provenance: {}\n", provenance.describe()));
        }
        _ => text.push_str(
            "  build: source pin unknown (this build carries no installer stamp)\n  provenance: unknown (an unstamped build cannot name its source)\n",
        ),
    }
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// A lowercase 40-hex commit, the only shape the installer writes.
fn wellformed_pin(pin: Option<&str>) -> Option<&str> {
    pin.filter(|pin| pin.len() == 40 && hex(pin))
}

/// A lowercase 32-hex (128-bit) nonce.
fn wellformed_instance(instance: Option<&str>) -> Option<&str> {
    instance.filter(|instance| instance.len() == 32 && hex(instance))
}

fn hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// What the executing artifact proved about itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Provenance {
    /// The installer's own binary, bound by receipt, instance and digest.
    Verified {
        receipt: String,
        receipt_sha256: String,
        binary_sha256: String,
    },
    Unverified {
        reason: String,
    },
}

impl Provenance {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Verified {
                receipt,
                receipt_sha256,
                binary_sha256,
            } => format!(
                "verified install receipt {receipt}: installed binary sha256 {binary_sha256}, receipt sha256 {receipt_sha256}"
            ),
            Self::Unverified { reason } => format!(
                "unverified ({reason}); this build is not authenticated as the receipt's installed binary"
            ),
        }
    }
}

fn unverified(reason: impl Into<String>) -> Provenance {
    Provenance::Unverified {
        reason: reason.into(),
    }
}

/// Provenance with the running executable and HOME injected, so unit tests
/// never touch the process environment or a real installation.
pub(crate) fn provenance_in(
    running: Option<&Path>,
    home: Option<&Path>,
    pin: &str,
    instance: &str,
) -> Provenance {
    let Some(running) = running else {
        return unverified("the running executable is unknown");
    };
    let Some(home) = home.filter(|home| home.is_absolute()) else {
        return unverified(
            "the account home is not an absolute path, so no install receipt can be checked",
        );
    };
    let destination = home.join(".local/bin/oxide");
    let (receipt, bytes, private) = match handoff::newest_receipt_bytes(home) {
        Ok(found) => found,
        Err(error) => return unverified(format!("no install receipt is readable ({error})")),
    };
    if !private {
        return unverified("the newest install receipt is not owner-only");
    }
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return unverified("the newest install receipt is not valid JSON"),
    };
    if value.get("pin").and_then(serde_json::Value::as_str) != Some(pin) {
        return unverified("the newest install receipt was written for another source pin");
    }
    if value
        .get("build_instance")
        .and_then(serde_json::Value::as_str)
        != Some(instance)
    {
        return unverified("the newest install receipt was written for another build instance");
    }
    let installed = value.get("installed").filter(|value| value.is_object());
    let Some(installed) = installed else {
        return unverified("the newest install receipt has no installed facts");
    };
    let recorded_path = installed
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    let Some(recorded_path) = recorded_path else {
        return unverified("the newest install receipt names no installed path");
    };
    let recorded_sha = installed
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if recorded_sha.len() != 64 || !hex(recorded_sha) {
        return unverified("the newest install receipt has no valid binary digest");
    }
    let Some(recorded_size) = installed.get("size").and_then(serde_json::Value::as_u64) else {
        return unverified("the newest install receipt has no installed size");
    };
    let Ok(running) = fs::canonicalize(running) else {
        return unverified("the running executable's path cannot be resolved");
    };
    let Ok(recorded_canonical) = fs::canonicalize(&recorded_path) else {
        return unverified("the receipt's installed binary is not present");
    };
    if running != recorded_canonical {
        return unverified("the running executable is not the receipt's installed binary");
    }
    let Ok(opened) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&destination)
    else {
        return unverified("the installed binary cannot be opened");
    };
    let Ok(opened_facts) = opened.metadata() else {
        return unverified("the installed binary cannot be inspected");
    };
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !opened_facts.is_file()
        || opened_facts.uid() != uid
        || opened_facts.mode() & 0o022 != 0
        || opened_facts.mode() & 0o100 == 0
    {
        return unverified("the installed binary is not a private, executable, owned file");
    }
    // The named path and the opened descriptor must still be one file, and
    // the executing image must be that file.
    let Ok(by_path) = fs::symlink_metadata(&destination) else {
        return unverified("the installed binary disappeared while it was checked");
    };
    if by_path.file_type().is_symlink()
        || (by_path.dev(), by_path.ino()) != (opened_facts.dev(), opened_facts.ino())
    {
        return unverified("the installed binary changed while it was checked");
    }
    let Ok(running_facts) = fs::metadata(&running) else {
        return unverified("the running executable cannot be inspected");
    };
    if (running_facts.dev(), running_facts.ino()) != (opened_facts.dev(), opened_facts.ino()) {
        return unverified("the running executable is a copy, not the receipt's installed file");
    }
    if opened_facts.size() != recorded_size || recorded_size > CANDIDATE_MAX_BYTES {
        return unverified("the installed binary's size differs from the receipt");
    }
    let Ok(binary_sha256) = handoff::digest_bounded(&opened, CANDIDATE_MAX_BYTES) else {
        return unverified("the installed binary cannot be read within its byte limit");
    };
    if binary_sha256 != recorded_sha {
        return unverified("the installed binary's digest differs from the receipt");
    }
    let receipt_sha256: String = sha256(&bytes);
    Provenance::Verified {
        receipt,
        receipt_sha256,
        binary_sha256,
    }
}

/// SHA-256 over exactly these receipt bytes.
fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        root: PathBuf,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = loop {
                let id = NEXT.fetch_add(1, Ordering::Relaxed);
                let root =
                    std::env::temp_dir().join(format!("oxide-version-{}-{id}", std::process::id()));
                match fs::create_dir(&root) {
                    Ok(()) => {
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
                        break root;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create version fixture: {error}"),
                }
            };
            let home = root.join("home");
            fs::create_dir_all(home.join(".local/bin")).unwrap();
            fs::create_dir_all(home.join(".local/state/oxide/install")).unwrap();
            for directory in [&home, &home.join(".local"), &home.join(".local/bin")] {
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
            fs::set_permissions(
                home.join(".local/state/oxide"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::set_permissions(
                home.join(".local/state/oxide/install"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            Self { root, home }
        }

        fn destination(&self) -> PathBuf {
            self.home.join(".local/bin/oxide")
        }

        /// Install `bytes` as the destination and write its receipt.
        fn install(&self, bytes: &[u8], pin: &str, instance: &str, file: &str) -> PathBuf {
            let destination = self.destination();
            fs::write(&destination, bytes).unwrap();
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
            let receipt = format!(
                "{{\"pin\":\"{pin}\",\"build_instance\":\"{instance}\",\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}}}}",
                destination.display(),
                sha256(bytes),
                bytes.len()
            );
            self.receipt(file, &receipt)
        }

        fn receipt(&self, file: &str, receipt: &str) -> PathBuf {
            let path = self.home.join(".local/state/oxide/install").join(file);
            fs::write(&path, receipt).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        }

        fn provenance(&self, pin: &str, instance: &str) -> Provenance {
            let destination = self.destination();
            provenance_in(Some(&destination), Some(&self.home), pin, instance)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    const PIN: &str = "0123456789abcdef0123456789abcdef01234567";
    const INSTANCE: &str = "fedcba9876543210fedcba9876543210";
    const BYTES: &[u8] = b"oxide binary fixture";

    #[test]
    fn a_matching_receipt_pin_instance_and_digest_verify() {
        let fixture = Fixture::new();
        fixture.install(BYTES, PIN, INSTANCE, "1.json");
        let provenance = fixture.provenance(PIN, INSTANCE);
        let Provenance::Verified {
            receipt,
            receipt_sha256,
            binary_sha256,
        } = &provenance
        else {
            panic!("{provenance:?}");
        };
        assert_eq!(receipt, "1.json");
        assert_eq!(binary_sha256, &sha256(BYTES));
        // The receipt digest covers exactly the receipt bytes, not the binary.
        let receipt_bytes =
            fs::read(fixture.home.join(".local/state/oxide/install/1.json")).unwrap();
        assert_eq!(receipt_sha256, &sha256(&receipt_bytes));
        assert_ne!(receipt_sha256, binary_sha256);
        let text = provenance.describe();
        assert!(text.starts_with("verified install receipt 1.json"));
    }

    #[test]
    fn a_stale_newer_receipt_wins_and_unverifies() {
        let fixture = Fixture::new();
        fixture.install(BYTES, PIN, INSTANCE, "1.json");
        // A later receipt for another pin is newest, so the older matching
        // one is never searched.
        fixture.receipt(
            "2.json",
            &format!(
                "{{\"pin\":\"{}\",\"build_instance\":\"{INSTANCE}\",\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}}}}",
                "a".repeat(40),
                fixture.destination().display(),
                sha256(BYTES),
                BYTES.len()
            ),
        );
        let provenance = fixture.provenance(PIN, INSTANCE);
        assert!(
            matches!(&provenance, Provenance::Unverified { reason } if reason.contains("another source pin")),
            "{provenance:?}"
        );
    }

    #[test]
    fn a_missing_malformed_or_foreign_receipt_is_unverified() {
        let fixture = Fixture::new();
        let provenance = fixture.provenance(PIN, INSTANCE);
        assert!(matches!(provenance, Provenance::Unverified { .. }));
        fixture.receipt("1.json", "not json");
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
        // A receipt with no instance (an old receipt) never verifies.
        fixture.receipt(
            "2.json",
            &format!(
                "{{\"pin\":\"{PIN}\",\"installed\":{{\"path\":\"{}\",\"sha256\":\"{}\",\"size\":{}}}}}",
                fixture.destination().display(),
                sha256(BYTES),
                BYTES.len()
            ),
        );
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
        // A receipt whose installed binary is elsewhere never verifies.
        fixture.receipt(
            "3.json",
            &format!(
                "{{\"pin\":\"{PIN}\",\"build_instance\":\"{INSTANCE}\",\"installed\":{{\"path\":\"/elsewhere/oxide\",\"sha256\":\"{}\",\"size\":{}}}}}",
                sha256(BYTES),
                BYTES.len()
            ),
        );
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
    }

    #[test]
    fn a_replaced_binary_mismatched_size_or_symlink_is_unverified() {
        let fixture = Fixture::new();
        fixture.install(BYTES, PIN, INSTANCE, "1.json");
        // Same pin, different bytes: the size or digest differs.
        fs::write(fixture.destination(), b"oxide binary fixture, rebuilt").unwrap();
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o755)).unwrap();
        let provenance = fixture.provenance(PIN, INSTANCE);
        assert!(
            matches!(&provenance, Provenance::Unverified { reason } if reason.contains("size")),
            "{provenance:?}"
        );
        let same_size = b"oxide binary fixture";
        fs::write(fixture.destination(), same_size).unwrap();
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o755)).unwrap();
        // A different instance (a rebuilt binary with the same pin) refuses.
        let provenance = fixture.provenance(PIN, &"0".repeat(32));
        assert!(
            matches!(&provenance, Provenance::Unverified { reason } if reason.contains("build instance")),
            "{provenance:?}"
        );
        // A symlinked destination is never authenticated.
        fixture.install(BYTES, PIN, INSTANCE, "2.json");
        let moved = fixture.root.join("moved");
        fs::rename(fixture.destination(), &moved).unwrap();
        std::os::unix::fs::symlink(&moved, fixture.destination()).unwrap();
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
    }

    #[test]
    fn a_running_copy_elsewhere_is_unverified() {
        let fixture = Fixture::new();
        fixture.install(BYTES, PIN, INSTANCE, "1.json");
        let copy = fixture.root.join("copy");
        fs::write(&copy, BYTES).unwrap();
        fs::set_permissions(&copy, fs::Permissions::from_mode(0o755)).unwrap();
        let provenance = provenance_in(Some(&copy), Some(&fixture.home), PIN, INSTANCE);
        assert!(
            matches!(&provenance, Provenance::Unverified { reason } if reason.contains("not the receipt")),
            "{provenance:?}"
        );
    }

    #[test]
    fn a_non_private_or_group_writable_binary_is_unverified() {
        let fixture = Fixture::new();
        fixture.install(BYTES, PIN, INSTANCE, "1.json");
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o664)).unwrap();
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
        // A receipt the user cannot read privately is unverified too.
        fs::set_permissions(fixture.destination(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(
            fixture.home.join(".local/state/oxide/install/1.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(matches!(
            fixture.provenance(PIN, INSTANCE),
            Provenance::Unverified { .. }
        ));
    }

    #[test]
    fn stamps_must_be_lowercase_hex_of_the_expected_length() {
        assert_eq!(wellformed_pin(Some(PIN)), Some(PIN));
        assert_eq!(wellformed_pin(Some(&PIN.to_uppercase())), None);
        assert_eq!(wellformed_pin(Some(&PIN[..39])), None);
        assert_eq!(wellformed_pin(None), None);
        assert_eq!(wellformed_instance(Some(INSTANCE)), Some(INSTANCE));
        assert_eq!(wellformed_instance(Some(&INSTANCE[..31])), None);
        assert_eq!(wellformed_instance(Some(&"A".repeat(32))), None);
        assert_eq!(wellformed_instance(Some(&"g".repeat(32))), None);
    }
}
