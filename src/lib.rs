//! Durable, bounded ownership for Oxide build intermediates.

#[cfg(unix)]
mod storage;

#[cfg(unix)]
pub use storage::*;

#[cfg(unix)]
mod project;

#[cfg(unix)]
pub use project::*;

/// Private ordinary-Cargo qualification route, available only in explicit
/// qualification builds.
#[cfg(all(unix, feature = "private-qualification"))]
pub use project::ordinary_cargo;

/// Inactive-by-default registered workflow boundary; trusted owners are required.
#[cfg(unix)]
pub mod registered;
