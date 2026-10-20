//! Budget-only build-slot leases, read-only target inventory and a bounded
//! Cargo dep-info decoder for the `oxide` front door.

/// Bounded read-only inventory of one explicitly selected target directory.
#[cfg(unix)]
pub mod inventory;

/// Same-user leases over exclusive slots in a budget-only pool.
#[cfg(unix)]
pub mod lease;

/// Bounded decoder for Cargo's encoded dep-info, kept as input for the
/// cross-checkout freshness gate.
pub mod dep_info;

/// Identity, descriptor binding and flush of a removable APFS volume for the
/// `apfs-volume-v1` pool backend.
#[cfg(unix)]
pub mod volume;
