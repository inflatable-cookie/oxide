//! Workload 1's binary (oxide#307). Deterministic observable output from
//! argv, serde's derive round trip and `serde_json`. The binary never names
//! `Record::count`'s type, so the predeclared `u32` -> `u64` interface edit
//! in `greet-core` compiles here unchanged and must invalidate this unit.

use greet_core::{Record, render};
use serde_json::json;

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "world".to_string());
    let record = Record { name, count: 42 };
    let rendered = render(&record);
    let payload = json!({ "record": record, "rendered": rendered });
    // The derive round trip must reproduce the record exactly: re-serialize
    // the parsed record and require byte-equality with the payload member. A
    // mismatch panics with a nonzero exit instead of printing a wrong line.
    let back: Record =
        serde_json::from_value(payload["record"].clone()).expect("record round trip");
    let redone = serde_json::to_string(&back).expect("record re-serialize");
    assert_eq!(redone, payload["record"].to_string());
    println!("{payload}");
}
