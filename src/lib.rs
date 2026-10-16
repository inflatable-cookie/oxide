//! Durable, bounded ownership for Oxide build intermediates.

#[cfg(unix)]
mod storage;

#[cfg(unix)]
pub use storage::*;

#[cfg(unix)]
mod project;

#[cfg(unix)]
pub use project::*;

/// Bounded read-only inventory of one explicitly selected target directory.
#[cfg(unix)]
pub mod inventory;

/// Unpublished same-user local lease CLI over bounded exclusive slots.
#[cfg(unix)]
pub mod lease;

/// Private ordinary-Cargo qualification route, available only in explicit
/// qualification builds.
#[cfg(all(unix, feature = "private-qualification"))]
pub use project::ordinary_cargo;

/// Inactive-by-default registered workflow boundary; trusted owners are required.
#[cfg(unix)]
pub mod registered;
