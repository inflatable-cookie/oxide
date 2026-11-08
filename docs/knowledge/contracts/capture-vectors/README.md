# Canonical capture-wire vectors

Current proof input is `v2/` (`oxide.capture.transcript/2`,
`oxide.capture.manifest/2`). Its `SHA256SUMS` binds every file, and
`src/bin/oxide/capture_wire.rs` binds the same digests. The contract is in
`docs/knowledge/contracts/barge-adoption.md` under "Current shared capture
wire `/2`".

- `v2/valid/`: transcripts that parse and whose completeness issues are
  exactly the expected list (unproven gaps stay issues).
- `v2/invalid/`: parser refusals (`/1` history, apparatus diagnostic,
  unknown or missing fields, wrong types, impossible combinations, reused
  epochs, bad outcome or provenance) and parser-positive completeness
  failures (`source-mismatch`, `wrong-inode`, `omitted-obligation`,
  `omitted-effect`, `extra-effect`, `substituted-effect`,
  `attempted-as-completed`, `collision`).
- `v2/manifest/`: valid and invalid `/2` manifests.

The `/1` files at this directory's root and their `SHA256SUMS` are historical
evidence, unchanged. They are refused as proof input. Barge copies these
files; there is no runtime cross-repo dependency.
