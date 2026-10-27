//! Debug-only guard for product proof hooks (oxide#155).
//!
//! A debug hook that changes product behaviour must only arm inside the shared
//! proof supervisor's own layout: its deadline, its canonical owned scratch
//! root, and its evidence directory nested in that scratch or its `evidence/`
//! sibling. This is the same rule the prune hooks apply; it is here so the
//! deletion seam's race injection and the prune hooks share one definition.
//! Absent from release builds.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The supervisor's evidence directory, when the supervisor armed this
/// process. `None` in every ordinary run.
pub fn evidence() -> Option<PathBuf> {
    std::env::var("OXIDE_PROOF_DEADLINE_NS")
        .ok()?
        .parse::<u64>()
        .ok()?;
    let scratch = std::fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")?).ok()?;
    let evidence = std::fs::canonicalize(std::env::var_os("OXIDE_PROOF_EVIDENCE")?).ok()?;
    let nested = evidence.starts_with(&scratch);
    let sibling = scratch.file_name() == Some(OsStr::new("scratch"))
        && evidence.file_name() == Some(OsStr::new("evidence"))
        && scratch.parent() == evidence.parent();
    if !(nested || sibling) {
        return None;
    }
    Some(evidence)
}
