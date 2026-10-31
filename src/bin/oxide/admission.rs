//! Admission marker for the shared proof supervisor (oxide#192).
//!
//! A leased command that waited for admission publishes one bounded marker in
//! the supervisor's evidence directory, so the supervisor can report that wait
//! separately and keep its work clock for execution. The write is armed only by
//! the supervisor's own exported layout ([`oxide_storage::supervisor`]) and is
//! best effort: it never changes the command's outcome, its exit status or any
//! slot state, and it is compiled in every profile because an installed `oxide`
//! runs the reported case too.
//!
//! The marker's fixed shape is `{"schema":1,"source":"oxide","waited_ns":N}` in
//! at most 512 bytes. A fresh file is published by rename, so one inode is one
//! admission report.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::Duration;

/// The marker's fixed file name inside the supervisor's evidence directory.
const MARKER_NAME: &str = "admission.json";
const MARKER_SCHEMA: u64 = 1;
const MARKER_SOURCE: &str = "oxide";
const MARKER_MAX_BYTES: usize = 512;
/// A source token in the supervisor's schema is at most 32 characters.
const _: () = assert!(MARKER_SOURCE.len() <= 32);

/// Publish one bounded admission marker, best effort.
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
    let _ = publish(&evidence, body.as_bytes());
}

/// Write one marker by rename, so a reader never sees a partial document. A
/// failed rename removes its own temporary file and reports the error to the
/// caller, which drops it.
fn publish(evidence: &Path, body: &[u8]) -> std::io::Result<()> {
    let temp = evidence.join(format!(".admission.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(body)?;
    drop(file);
    let marker = evidence.join(MARKER_NAME);
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
    use super::{publish, MARKER_MAX_BYTES, MARKER_NAME};
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

    #[test]
    fn one_marker_is_published_with_the_exact_bounded_document() {
        let root = fixture();
        let evidence = root.join("evidence");
        let body = format!(
            "{{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":{}}}",
            126_900_000_000u64
        );
        publish(&evidence, body.as_bytes()).unwrap();
        let marker = evidence.join(MARKER_NAME);
        let written = std::fs::read(&marker).unwrap();
        assert_eq!(written, body.as_bytes());
        assert!(written.len() <= MARKER_MAX_BYTES);
        let document: serde_json::Value = serde_json::from_slice(&written).unwrap();
        assert_eq!(document["schema"], serde_json::json!(1));
        assert_eq!(document["source"], serde_json::json!("oxide"));
        assert_eq!(document["waited_ns"], serde_json::json!(126_900_000_000u64));
        // No temporary file survives a successful publication.
        let names: Vec<String> = std::fs::read_dir(&evidence)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![MARKER_NAME.to_owned()]);
    }

    #[test]
    fn a_second_report_is_a_new_file_identity() {
        let root = fixture();
        let evidence = root.join("evidence");
        let marker = evidence.join(MARKER_NAME);
        publish(
            &evidence,
            b"{\"schema\":1,\"source\":\"oxide\",\"waited_ns\":1}",
        )
        .unwrap();
        let first = std::fs::metadata(&marker).unwrap();
        publish(
            &evidence,
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
    fn an_unusable_directory_reports_an_error_and_leaves_nothing() {
        let root = fixture();
        let missing = root.join("missing");
        assert!(publish(&missing, b"{}").is_err());
        assert!(!missing.exists());
    }
}
