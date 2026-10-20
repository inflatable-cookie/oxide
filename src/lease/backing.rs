//! Fixed names under a pool root.
//!
//! The APFS sparse-image backend that provisioned `image.sparseimage` and its
//! capacity `receipt.json` is retired (oxide#102). The names remain only so a
//! budget-only or removable open refuses a pool that still holds them: a
//! legacy image pool is never reinterpreted as a directory.

use super::{IMAGE_NAME, PAYLOAD_NAME, RECEIPT_NAME};
use std::path::{Path, PathBuf};

pub(super) struct BackingPaths {
    pub image: PathBuf,
    pub receipt: PathBuf,
    pub mount: PathBuf,
}

impl BackingPaths {
    pub(super) fn under(pool: &Path) -> Self {
        Self {
            image: pool.join(IMAGE_NAME),
            receipt: pool.join(RECEIPT_NAME),
            mount: pool.join(PAYLOAD_NAME),
        }
    }
}
