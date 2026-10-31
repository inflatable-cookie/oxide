//! Admission reports for the shared proof supervisor (oxide#192).
//!
//! A leased command reports its own admission wait in the supervisor's evidence
//! directory, so the supervisor can report that wait separately and keep its
//! work clock for execution. The writes are armed only by the supervisor's own
//! exported layout ([`oxide_storage::supervisor`]) and are best effort: they
//! never change the command's outcome, its exit status or any slot state, and
//! they are compiled in every profile because an installed `oxide` runs the
//! reported case too.
//!
//! Two reports share that layout, its arming rule and its bounds (at most 512
//! bytes, published by rename so one inode is one report):
//!
//! - `admission.json`, `{"schema":1,"source":"oxide","waited_ns":N}`, published
//!   once when a grant ends this command's own admission wait, so the supervisor
//!   can grant bounded time for that completed wait;
//! - `admission-waiting.json`, `{"schema":1,"source":"oxide","reason":"<token>"}`,
//!   published once when the command first observes that it must wait, so a wait
//!   that never ends before the supervisor's stop is still reported and
//!   classified instead of leaving no trace. It grants nothing.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

/// The completed-wait report's fixed file name inside the evidence directory.
const MARKER_NAME: &str = "admission.json";
/// The waiting report's fixed file name: a wait that has started and not ended.
const WAITING_NAME: &str = "admission-waiting.json";
const MARKER_SCHEMA: u64 = 1;
const MARKER_SOURCE: &str = "oxide";
const MARKER_MAX_BYTES: usize = 512;
/// A token in the supervisor's schema is at most 32 printable characters.
const TOKEN_MAX: usize = 32;
const _: () = assert!(MARKER_SOURCE.len() <= TOKEN_MAX);

/// A token is printable, non-empty and bounded, so a report never carries text
/// the supervisor would have to interpret.
fn token_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= TOKEN_MAX
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' ')
}

/// Publish one bounded completed-wait report, best effort.
///
/// Unarmed, an unusable directory or any I/O failure does nothing: the wait is
/// then simply not reported, and the supervisor keeps its unextended deadline.
pub(crate) fn report_wait(waited: Duration) {
    let Some(evidence) = oxide_storage::supervisor::evidence_dir() else {
        return;
    };
    let waited_ns = u64::try_from(waited.as_nanos()).unwrap_or(u64::MAX);
    let body = format!(
        "{{\"schema\":{MARKER_SCHEMA},\"source\":\"{MARKER_SOURCE}\",\"waited_ns\":{waited_ns}}}"
    );
    if body.len() > MARKER_MAX_BYTES {
        return;
    }
    let _ = publish(&evidence, MARKER_NAME, body.as_bytes());
}

/// Publish one bounded waiting report, best effort: this command is waiting for
/// admission and no grant has ended that wait. It never moves a supervisor
/// deadline; it exists so a wait that never ends is still classified. The
/// reason is this command's own wait label, for example `queue` or `host limit`.
pub(crate) fn report_waiting(reason: &str) {
    let Some(evidence) = oxide_storage::supervisor::evidence_dir() else {
        return;
    };
    if !token_ok(reason) {
        return;
    }
    let body = format!(
        "{{\"schema\":{MARKER_SCHEMA},\"source\":\"{MARKER_SOURCE}\",\"reason\":\"{reason}\"}}"
    );
    if body.len() > MARKER_MAX_BYTES {
        return;
    }
    let _ = publish(&evidence, WAITING_NAME, body.as_bytes());
}

/// Write one report by rename, so a reader never sees a partial document. A
/// failed rename removes its own temporary file and reports the error to the
/// caller, which drops it.
fn publish(evidence: &Path, name: &str, body: &[u8]) -> std::io::Result<()> {
    let temp = evidence.join(format!(".{name}.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(body)?;
    drop(file);
    let marker = evidence.join(name);
    match std::fs::rename(&temp, &marker) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{publish, token_ok, MARKER_MAX_BYTES, MARKER_NAME, TOKEN_MAX, WAITING_NAME};
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        let template = std::env::temp_dir().join("oxide-admission.XXXXXX");
        let mut name = CString::new(template.as_os_str().as_bytes())
            .unwrap()
            .into_bytes_with_nul();
        // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
        assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
        let name = CStr::from_bytes_until_nul(&name).unwrap();
        let root =
            std::fs::canonicalize(PathBuf::from(OsStr::from_bytes(name.to_bytes()))).unwrap();
        std::fs::create_dir(root.join("scratch")).unwrap();
        std::fs::create_dir(root.join("evidence")).unwrap();
        std::fs::create_dir(root.join("scratch").join("evidence")).unwrap();
        root
    }

    fn documents(evidence: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(evidence)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn one_marker_is_published_with_the_exact_bounded_document() {
        let root = fixture();
        let evidence = root.join("evidence");
        let body = format!(
            "{{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":{}}}",
            126_900_000_000u64
        );
        publish(&evidence, MARKER_NAME, body.as_bytes()).unwrap();
        let marker = evidence.join(MARKER_NAME);
        let written = std::fs::read(&marker).unwrap();
        assert_eq!(written, body.as_bytes());
        assert!(written.len() <= MARKER_MAX_BYTES);
        let document: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(document["schema"], serde_json::json!(1));
        assert_eq!(document["source"], serde_json::json!("oxide"));
        assert_eq!(document["waited_ns"], serde_json::json!(126_900_000_000u64));
        // No temporary file survives a successful publication.
        assert_eq!(documents(&evidence), vec![MARKER_NAME.to_owned()]);
    }

    #[test]
    fn a_second_report_is_a_new_file_identity() {
        let root = fixture();
        let evidence = root.join("evidence");
        let marker = evidence.join(MARKER_NAME);
        publish(
            &evidence,
            MARKER_NAME,
            b"{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":1}",
        )
        .unwrap();
        let first = std::fs::metadata(&marker).unwrap();
        publish(
            &evidence,
            MARKER_NAME,
            b"{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":2}",
        )
        .unwrap();
        let second = std::fs::metadata(&marker).unwrap();
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            b"{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":2}"
        );
        assert_ne!(
            (first.dev(), first.ino()),
            (second.dev(), second.ino()),
            "a rename must give the supervisor a new identity"
        );
    }

    #[test]
    fn a_waiting_report_is_its_own_bounded_document_beside_the_grant() {
        let root = fixture();
        let evidence = root.join("evidence");
        publish(
            &evidence,
            WAITING_NAME,
            b"{\"schema\":1,\"source\":\"oxide\",\"reason\":\"queue\"}",
        )
        .unwrap();
        let waiting = evidence.join(WAITING_NAME);
        let written = std::fs::read(&waiting).unwrap();
        assert!(written.len() <= MARKER_MAX_BYTES);
        let document: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(document["schema"], serde_json::json!(1));
        assert_eq!(document["source"], serde_json::json!("oxide"));
        assert_eq!(document["reason"], serde_json::json!("queue"));
        // The two report kinds never overwrite each other.
        publish(
            &evidence,
            MARKER_NAME,
            b"{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":7}",
        )
        .unwrap();
        assert_eq!(
            documents(&evidence),
            vec![WAITING_NAME.to_owned(), MARKER_NAME.to_owned()]
        );
        assert_eq!(std::fs::read(waiting).unwrap(), written);
    }

    #[test]
    fn a_reason_token_is_bounded_and_printable() {
        for good in ["queue", "host limit", "uncertain slot", "capacity"] {
            assert!(token_ok(good), "{good}");
        }
        assert!(token_ok(&"x".repeat(TOKEN_MAX)));
        for bad in [
            "",
            "line\nbreak",
            "tab\there",
            "\u{00e9}",
            "x".repeat(TOKEN_MAX + 1).as_str(),
        ] {
            assert!(!token_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn an_unusable_directory_reports_an_error_and_leaves_nothing() {
        let root = fixture();
        let missing = root.join("missing");
        assert!(publish(&missing, MARKER_NAME, b"{}").is_err());
        assert!(!missing.exists());
    }
}
