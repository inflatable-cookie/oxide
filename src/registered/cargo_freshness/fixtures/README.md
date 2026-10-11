# Pinned Cargo freshness fixtures

These small records are hand-annotated for the persisted `Fingerprint` shape
and `LocalFingerprint` variants at Cargo
[`c980f4866`](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/mod.rs#L3345-L3439)
and [`LocalFingerprint`](https://github.com/rust-lang/cargo/blob/c980f4866/src/cargo/core/compiler/fingerprint/mod.rs#L3660-L3735).
They are parser and mapping fixtures, not output captured from a Cargo graph.
`rerun-files-environment.json` combines file and environment locals. The
collector fixture uses `check-dep-info-rerun-environment.json` to exercise
dep-info conversion and absent/empty rerun environment values on one unit.
The binary dep-info fixture remains the source-linked
[`UPSTREAM_V1`](../../dep_info.rs) byte vector and is decoded by the existing
registered dep-info decoder.
