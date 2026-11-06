# Canonical capture-wire vectors

Hashed JSON fixtures for `oxide.capture.transcript/1` and
`oxide.capture.manifest/1`. Schema and version are those strings. SHA-256
of each file is in `SHA256SUMS` and in `src/bin/oxide/capture_wire.rs`.

Parser-positive proves structure. Completeness is separate: path-only
`parent_required` fails; `handle` parent still fails while K1 is unqualified.
`invalid/` also holds parser-positive completeness failures (wrong parent,
wrong inode, omitted obligation, omitted/extra/substituted effects,
collision, source/run mismatch) and parse-fail vectors (string `device`,
string `parent_inode`, occurrence 0). Cap overflow (8 MiB, 8193 reads,
occurrence 65, 513 process rows) is constructed in the self-check, not as
files. Barge copies these files; there is no runtime cross-repo dependency.
