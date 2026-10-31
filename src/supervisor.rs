//! The shared proof supervisor's arming layout (oxide#192).
//!
//! A proof child may recognise the shared proof supervisor's own export: an
//! absolute `CLOCK_UPTIME_RAW` deadline, a canonical owned scratch root and an
//! evidence directory nested in that scratch or its `scratch`/`evidence`
//! sibling. The debug-only product proof hooks and the always-compiled admission
//! marker share this one definition, so no seam can drift from another.
//!
//! Recognising the layout never changes a command's outcome: the marker's own
//! write is best effort and every hook that uses it keeps its own rules.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The supervisor's evidence directory, when the supervisor armed this
/// process. `None` in every ordinary run, and for any recorded value that does
/// not name the supervisor's layout.
pub fn evidence_dir() -> Option<PathBuf> {
    let deadline = std::env::var_os("OXIDE_PROOF_DEADLINE_NS");
    let scratch = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT");
    let evidence = std::env::var_os("OXIDE_PROOF_EVIDENCE");
    armed(deadline.as_deref(), scratch.as_deref(), evidence.as_deref())
}

/// The same decision over explicit values, so the rule is testable without
/// mutating this process's environment.
fn armed(
    deadline: Option<&OsStr>,
    scratch: Option<&OsStr>,
    evidence: Option<&OsStr>,
) -> Option<PathBuf> {
    deadline?.to_str()?.parse::<u64>().ok()?;
    let scratch = std::fs::canonicalize(scratch?).ok()?;
    let evidence = std::fs::canonicalize(evidence?).ok()?;
    let nested = evidence.starts_with(&scratch);
    let sibling = scratch.file_name() == Some(OsStr::new("scratch"))
        && evidence.file_name() == Some(OsStr::new("evidence"))
        && scratch.parent() == evidence.parent();
    (nested || sibling).then_some(evidence)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{armed, evidence_dir};
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    fn fixture() -> PathBuf {
        let template = std::env::temp_dir().join("oxide-supervisor-layout.XXXXXX");
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

    fn os(path: &Path) -> &OsStr {
        path.as_os_str()
    }

    fn deadline() -> &'static OsStr {
        OsStr::new("1700000000000000000")
    }

    #[test]
    fn the_sibling_and_nested_layouts_are_the_supervisor_shape() {
        let root = fixture();
        let scratch = root.join("scratch");
        assert_eq!(
            armed(
                Some(deadline()),
                Some(os(&scratch)),
                Some(os(&root.join("evidence")))
            ),
            Some(root.join("evidence"))
        );
        assert_eq!(
            armed(
                Some(deadline()),
                Some(os(&scratch)),
                Some(os(&scratch.join("evidence")))
            ),
            Some(scratch.join("evidence"))
        );
    }

    #[test]
    fn every_other_shape_is_refused() {
        let root = fixture();
        let scratch = root.join("scratch");
        let evidence = root.join("evidence");
        assert_eq!(
            armed(None, Some(os(&scratch)), Some(os(&evidence))),
            None,
            "no deadline"
        );
        assert_eq!(
            armed(
                Some(OsStr::new("soon")),
                Some(os(&scratch)),
                Some(os(&evidence))
            ),
            None,
            "unparseable deadline"
        );
        assert_eq!(
            armed(Some(deadline()), None, Some(os(&evidence))),
            None,
            "no scratch"
        );
        assert_eq!(
            armed(Some(deadline()), Some(os(&scratch)), None),
            None,
            "no evidence"
        );
        assert_eq!(
            armed(
                Some(deadline()),
                Some(os(&scratch)),
                Some(os(&root.join("elsewhere")))
            ),
            None,
            "an unrelated directory"
        );
        assert_eq!(
            armed(
                Some(deadline()),
                Some(os(&root.join("evidence"))),
                Some(os(&root))
            ),
            None,
            "the leaf names the other way round"
        );
        assert_eq!(
            armed(
                Some(deadline()),
                Some(os(&root.join("missing"))),
                Some(os(&evidence))
            ),
            None,
            "missing scratch"
        );
    }

    /// The exported names and the canonical-layout rule are the whole arming
    /// contract; the admission marker's own write is tested with it.
    #[test]
    fn the_exported_environment_decides_the_arming() {
        let root = fixture();
        let names = [
            "OXIDE_PROOF_DEADLINE_NS",
            "OXIDE_TASK_SCRATCH_ROOT",
            "OXIDE_PROOF_EVIDENCE",
        ];
        let saved: Vec<(String, Option<std::ffi::OsString>)> = names
            .iter()
            .map(|name| ((*name).to_owned(), std::env::var_os(name)))
            .collect();
        std::env::set_var("OXIDE_PROOF_DEADLINE_NS", "1700000000000000000");
        std::env::set_var("OXIDE_TASK_SCRATCH_ROOT", root.join("scratch"));
        std::env::set_var("OXIDE_PROOF_EVIDENCE", root.join("evidence"));
        let armed_dir = evidence_dir();
        std::env::set_var("OXIDE_PROOF_DEADLINE_NS", "not-a-deadline");
        let refused_dir = evidence_dir();
        for (name, value) in &saved {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        assert_eq!(armed_dir, Some(root.join("evidence")));
        assert_eq!(refused_dir, None);
    }
}
