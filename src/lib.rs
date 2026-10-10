//! Durable, bounded ownership for Oxide build intermediates.

#[cfg(unix)]
mod storage;

#[cfg(unix)]
pub use storage::*;

#[cfg(unix)]
mod project;

#[cfg(unix)]
pub use project::*;

/// Inactive-by-default registered workflow boundary; trusted owners are required.
#[cfg(unix)]
pub mod registered;
