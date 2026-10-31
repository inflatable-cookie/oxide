//! Debug-only guard for product proof hooks (oxide#155).
//!
//! A debug hook that changes product behaviour must only arm inside the shared
//! proof supervisor's own layout: its deadline, its canonical owned scratch
//! root, and its evidence directory nested in that scratch or its `evidence/`
//! sibling. This is the same rule the prune hooks apply; it is here so the
//! deletion seam's race injection and the prune hooks share one definition.
//! Absent from release builds; the arming rule itself lives in the
//! always-compiled admission marker module, which needs it in every profile.

use std::path::PathBuf;

/// The supervisor's evidence directory, when the supervisor armed this
/// process. `None` in every ordinary run.
pub fn evidence() -> Option<PathBuf> {
    // One definition decides every proof seam: the always-compiled arming rule
    // behind the oxide#192 admission marker.
    crate::supervisor::evidence_dir()
}
