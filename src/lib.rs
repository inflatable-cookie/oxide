//! Budget-only build-slot leases, read-only target inventory and a bounded
//! Cargo dep-info decoder for the `oxide` front door.

#[cfg(all(test, unix))]
mod test_fs;

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

/// Bounded decoder for Cargo's encoded dep-info, kept as input for the
/// cross-checkout freshness gate.
pub mod dep_info;

/// Identity, descriptor binding and flush of a removable APFS volume for the
/// `apfs-volume-v1` pool backend.
#[cfg(unix)]
pub mod volume;
