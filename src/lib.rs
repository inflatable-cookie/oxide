//! Budget-only build-slot leases, read-only target inventory and a bounded
//! Cargo dep-info decoder for the `oxide` front door.

#[cfg(all(test, unix))]
mod test_fs;

/// The library-side observation tape for the prune capture harness (X-S1);
/// see `src/tape.rs` and `docs/knowledge/contracts/barge-adoption.md`.
#[cfg(all(test, unix))]
mod tape;

/// Opt-in diagnostics for lease closeout.
pub mod release_trace;

/// Bounded read-only inventory of one explicitly selected target directory.
#[cfg(unix)]
pub mod inventory;

/// Same-user leases over exclusive slots in a budget-only pool.
#[cfg(unix)]
pub mod lease;

/// Descriptor-relative deletion of a verified owned tree, with the fixed
/// Finder-metadata race bound (oxide#155).
#[cfg(unix)]
pub mod owned_delete;

/// Debug-only guard for product proof hooks (oxide#155).
#[cfg(debug_assertions)]
pub mod proof;

/// The shared proof supervisor's arming layout: its deadline, its canonical
/// owned scratch root and its evidence directory nested in it or its
/// `scratch`/`evidence` sibling (oxide#192). One definition decides every
/// proof seam, in every profile.
#[cfg(unix)]
pub mod supervisor;

/// Bounded source verification for cold-slot seeding: the registry archive
/// reader, the canonical tree digest, the verification cache and the git
/// adapter (oxide#166, slice 1).
#[cfg(unix)]
pub mod seed_source;

/// Bounded decoder for Cargo's encoded dep-info, kept as input for the
/// cross-checkout freshness gate.
pub mod dep_info;

/// Barge-backed storage authority: volumes, areas, marked roots, holds, pins,
/// pruning, trash and scratch. The one place Oxide declares its areas and
/// supplies Barge's platform seam.
#[cfg(unix)]
pub mod storage;

/// Identity, descriptor binding and flush of a removable APFS volume for the
/// `apfs-volume-v1` pool backend.
#[cfg(unix)]
pub mod volume;
