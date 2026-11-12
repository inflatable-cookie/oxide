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
    // The derive round trip must reproduce the record exactly: parse the
    // payload member back into a Record, re-value it and require structural
    // equality with the payload member (never string equality, whose two
    // spellings differ in key order). A mismatch panics with a nonzero exit
    // instead of printing a wrong line.
    let back: Record =
        serde_json::from_value(payload["record"].clone()).expect("record round trip");
    // Compare as Values, not as strings: serde_json's default Map sorts keys,
    // while direct struct serialization keeps declaration order, so byte
    // equality between those two spellings would always fail. Value equality
    // is structural and proves the derive round trip preserved the record.
    let back_value = serde_json::to_value(&back).expect("record re-value");
    assert_eq!(back_value, payload["record"]);
    println!("{payload}");
}
