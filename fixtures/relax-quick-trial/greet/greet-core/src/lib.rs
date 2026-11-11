//! Workload 1's library (oxide#307). The two predeclared trial edits touch
//! this file: the body edit changes `GREETING`, and the interface edit changes
//! `Record::count`'s type from `u32` to `u64`. Neither edit may change the
//! expected observable output of the body case beyond its declared value, and
//! the interface edit must keep the same output bytes.

use serde::{Deserialize, Serialize};

/// The greeting the predeclared body edit changes to `"howdy"`.
pub const GREETING: &str = "hello";

/// The exchanged record. The predeclared interface edit changes `count` from
/// `u32` to `u64`, which must recompile the `greet` dependant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub count: u32,
}

/// Render the fixed observable phrase.
pub fn render(record: &Record) -> String {
    format!("{} {} count={}", GREETING, record.name, record.count)
}
