# Historical Nucleus input-root observations

These records preserve bounded, read-only inspection of one pinned Nucleus
planning/diagnostics workflow. They do not establish acquisition readiness,
complete input closure, an accepted production issuer, or a successful
Nucleus Cargo/test/profile result. The Oxide archive verifier and acquisition
implementation have been retired; this file is evidence only.

## Pinned source and registry correspondence

The inspected Git object was commit
`576ba7904d58e4bf829ddddda89a06833b8c5087`, tree
`0aa379b840c2f8990adeecc1fc49d9e57ab5869b`. Reads used the pinned Git blobs;
the checkout was not changed, fetched, checked out, built or status-scanned.
The requested test selector was `effigy check:core:test -- -p nucleus-core
--test planning --test build_diagnostics`, which resolved to an offline Cargo
test command. No such command was run.

Tasks 037–039 compared locked registry archives and extracted roots under
bounded metadata and byte limits. The final recorded results covered all 58
locked registry tuples with archive checksum and extracted file/directory
correspondence. The retained receipts include the per-package outcomes,
cumulative counters and the two packages that required streaming beyond the
legacy decoded-container cap:

- [Task 037](../evidence/nucleus-registry-acquisition-037.json)
- [Task 038 initial attempt](../evidence/nucleus-registry-acquisition-038-initial-attempt.json)
- [Task 038 corrected attempt](../evidence/nucleus-registry-acquisition-038.json)
- [Task 039 streaming update](../evidence/nucleus-registry-acquisition-039.json)

This filesystem correspondence did not establish effective Cargo source
selection/configuration, enabled units, a registry issuer, a no-replacement
agreement, complete native/SDK/tool closure, or a successful Nucleus build.
No Nucleus package source, script, macro, SDK content or toolchain content was
run. The receipts remain immutable historical evidence, not callable APIs or
permission to inspect current consumer installations.
