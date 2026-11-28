//! Budget-only build-slot leases, read-only target inventory and cold-slot
//! source verification for the `oxide` front door.

#[cfg(all(test, unix))]
mod test_fs;

#[cfg(unix)]
#[doc(hidden)]
pub mod lock_retry;

/// Opt-in diagnostics for lease closeout.
pub mod release_trace;

/// Bounded read-only inventory of one explicitly selected target directory.
#[cfg(unix)]
pub mod inventory;

/// Same-user leases over exclusive slots in a budget-only pool.
#[cfg(unix)]
pub mod lease;

/// Internal directory reads and safe diagnostic names; deletion is Barge's.
#[cfg(unix)]
mod fs_util;

/// Bounded source verification for cold-slot seeding: the registry archive
/// reader, the canonical tree digest and the verification cache (oxide#166,
/// slice 1).
#[cfg(unix)]
pub mod seed_source;

/// Barge-backed storage authority: volumes, areas, marked roots, holds, pins,
/// pruning, trash and scratch. The one place Oxide declares its areas and
/// supplies Barge's platform seam.
#[cfg(unix)]
pub mod storage;

/// Identity, descriptor binding and flush of a removable APFS volume for the
/// `apfs-volume-v1` pool backend.
#[cfg(unix)]
pub mod volume;
