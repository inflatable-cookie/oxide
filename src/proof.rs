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

/// Debug-only deterministic pause for the deletion-custody protocol
/// (oxide#181). With `OXIDE_TEST_DELETION_PAUSE=<point>:<gate>` a deletion at
/// `point` publishes `<gate>.<point>-<pid>` and waits (at most 120 s) until
/// `<gate>` exists. It arms only inside the shared supervisor's own layout
/// ([`evidence`]), so an ordinary debug command never pauses, and it is absent
/// from release builds.
pub fn deletion_pause(point: &str) {
    #[cfg(debug_assertions)]
    {
        if evidence().is_none() {
            return;
        }
        let Some(gate) = std::env::var("OXIDE_TEST_DELETION_PAUSE")
            .ok()
            .and_then(|value| value.strip_prefix(&format!("{point}:")).map(str::to_owned))
        else {
            return;
        };
        let marker = format!("{gate}.{point}-{}", std::process::id());
        let staged = format!("{gate}.staging-{point}-{}", std::process::id());
        if std::fs::write(&staged, point.as_bytes()).is_ok() {
            let _ = std::fs::rename(&staged, &marker);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        while !std::path::Path::new(&gate).exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let _ = point;
}
