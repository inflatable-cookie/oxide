//! Bounded storage primitives, a local lease CLI, read-only target inventory
//! and a bounded Cargo dep-info decoder.

#[cfg(unix)]
mod storage;

#[cfg(unix)]
pub use storage::*;

/// Bounded read-only inventory of one explicitly selected target directory.
#[cfg(unix)]
pub mod inventory;

/// Unpublished same-user local lease CLI over bounded exclusive slots.
#[cfg(unix)]
pub mod lease;

/// Bounded decoder for Cargo's encoded dep-info, kept as input for the
/// cross-checkout freshness gate.
pub mod dep_info;
