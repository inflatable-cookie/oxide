# Run-overhead negative record

The two JSON files are byte-for-byte copies from preserved source commit
`b313e9bd3bd2f941c7d04a795bb2fe5233840471`. `sha256.json` records their
content hashes; it is a preservation manifest, not a new execution receipt.
The source branch and Queue workspace remain unmerged research material.

`baseline.json` records an instrumentation-only tree; `candidate-1.json`
records the first candidate at `33497f8`. The latter JSON discloses that its
stdout line was rejoined after interleaving with Effigy output. It is the
author's retained structured observation, not a newly recovered original log.
No value or historical result has been repaired by this knowledge change.

Both fixed candidate timing gates failed. The focused proof then stopped at
its project-slot-count assertion, before the new registry and outside-path
correctness cases. There is no complete correctness proof, independent product
acceptance, or performance adoption. Neither the unused candidate proof nor
reviewer proof is spent by this evidence-only record.

Phase spans may overlap. A marker immediately before `exec` does not measure
all of the time after a successful `exec` alone: its residual includes handoff,
loader/OS/program execution, scheduling and completion observation. No cause
was isolated, and no plain `cargo run` timing was taken. The agreed baseline
was build-only; that limitation does not rescore the failed gate.
