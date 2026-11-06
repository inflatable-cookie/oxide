# Canonical capture-wire vectors

Hashed JSON fixtures for `oxide.capture.transcript/1` and
`oxide.capture.manifest/1`. Schema and version are those strings. SHA-256
of each file is in `SHA256SUMS` and in `src/bin/oxide/capture_wire.rs`.

Parser-positive proves structure. Completeness is separate: path-only
`parent_required` fails; `handle` parent still fails while K1 is unqualified.
Barge copies these files; there is no runtime cross-repo dependency.
