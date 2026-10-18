//! Bounded storage primitives, a local lease CLI, and read-only target inventory.

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
