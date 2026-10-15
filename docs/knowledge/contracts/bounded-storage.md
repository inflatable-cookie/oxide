# Bounded payload storage backend

Status: the opt-in hard-bounded macOS payload backend is implemented behind an
explicit verified capability. One real filesystem-full proof ran under its
recorded fixture bounds; synthetic fixtures cover the refusal and accounting
surface. No consumer activation, installation, detach/cleanup facility, or
production deployment follows from this contract. This document owns the
supported semantics; [architecture](../architecture.md) owns the storage
invariants and [ordinary-Cargo storage](ordinary-cargo-storage.md) owns the
versioned seam that this ledger change touches.

## Decision and boundary

The directory-only daemon checks admission against a caller-supplied budget
but nothing constrains a writer after acquire. The bounded backend closes that
gap with one supported shape: a fixed-size private APFS volume inside a sparse
disk image, provisioned and attached by the protected storage runtime. The
volume's finite size is the enforced aggregate-store ceiling — the operating
system refuses allocation beyond it while a cooperative writer holds an open
lease — and admission still reserves against the recorded ceiling so a
partition cannot silently consume the remaining pool.

The enforced bound is aggregate-store, never per-slot. Every owned slot shares
one finite payload filesystem and there is no per-slot isolation. Reservation
accounting is an acquire-time estimate: because the OS bound sits at the
verified volume capacity, a fully written payload can measure more than its
reservation. Reuse of an existing settled generation adds no bytes and stays
admissible; any real growth evicts proven-idle slots first or refuses with an
actionable known/unknown accounting result. ENOSPC during a leased workflow is
not writer closure or release: pending/uncertain state and the existing
authenticated settlement rules are unchanged.

Control state (ledger, socket, temporary files) and the capacity receipt stay
on the host filesystem, outside a payload that can become full, so a full
payload can never wedge durable ownership updates. The payload holds only the
slot directories; unknown entries refuse.

## What exists

- `oxide_storage::bounded` owns the verified capability
  (`BoundedBacking`, `BoundedBackendDescriptor`) and the backend tag
  `apfs-sparse-image-v1`. Construction is crate-private: only code that
  provisioned or opened the backing and verified it can mint one, and caller
  JSON cannot upgrade a directory root to a hard-bound claim. Old
  directory-only roots remain explicitly budget-only.
- The durable ledger is version 6 and records the backend mode: budget-only
  directory, or the bounded descriptor (backend tag, canonical payload root,
  recorded image identity, verified capacity). Restart refuses any mismatch
  between the configured backing and the recorded descriptor instead of
  reinterpreting the store. Version-5 ledgers refuse without migration.
- Finite control-history limits refuse new slots, leases, settlement, or
  retirement records at exhaustion instead of pruning required evidence or
  growing the ledger indefinitely; recorded ownership state is preserved.
- The protected runtime accepts an optional `bounded` bootstrap section
  (backend tag, image, receipt, mount point, nominal image bytes, ceiling).
  The ceiling must equal the bootstrap budget cap, so every bounded ceiling is
  bound to startup. Unsupported platforms and unknown backend tags refuse
  before any host effect; there is no unbounded fallback.
- Provisioning creates one fresh image, attaches it at the declared mount
  point, makes the volume root owner-only, samples the mounted filesystem
  (APFS at the exact mount point), records a capacity proof (image identity by
  device/inode, verified capacity, ceiling), and supplies the verified
  capability to the actual store. Opening re-derives the same identity and
  refuses a replaced image, a foreign or non-private volume, or a mismatched
  receipt. An image without its receipt, or a receipt without its image,
  refuses.
- If the daemon crashes, loses a reply, restarts, or disappears, existing
  ordinary ownership semantics are untouched: startup turns active leases
  `uncertain`, and nothing frees or replaces storage. This backend adds no
  eviction, no automatic detach, no source manager, and no run executor.

## Verified property and fixture record

The opt-in Effigy selector `rust:bounded-native-proof` (heavy admission,
`scripts/test-bounded-native.py`) runs one actual macOS filesystem-full proof
per participant: exactly one fresh owned 128 MiB APFS sparse image, a writer
payload bounded by 128 MiB with 1 MiB IO buffers, 512 MiB owned scratch, a
20 GiB host reserve, and a 120-second native fixture phase. It proves a
genuine OS allocation refusal (ENOSPC) under an open lease, control-ledger
durability with the payload full, authenticated settlement, daemon restart
reopening the same backing, and warm reuse. Ordinary detach and cleanup run
only after positive owned-writer completion; a failed run retains every
resource and prints its path. This is a real quota proof on a real volume; the
repository's synthetic suite exercises injected-backend refusals and
accounting separately and never claims quota enforcement. The selector is not
part of ordinary validation and is not an alias for full QA.

## Handover to retirement

The supported backend lifecycle operations are: provision (create, attach,
verify, receipt), open (verify against the receipt and live volume), and the
daemon bind that places slots on the verified payload. There is intentionally
no detach or cleanup API: teardown of a settled owned backing is the
dependent retirement task's authority, and until then an owner may perform an
ordinary `hdiutil detach` of a positively settled test-owned fixture only.
Failed or uncertain resources are retained, never force-cleared.

## Non-goals and limits

No consumer activation, no installation or service, no global host settings,
no per-slot quota isolation, no live cache cleanup of prior Oxide roots, and
no Linux or non-APFS support (explicit refusal, no fallback). The write
attribution of the image backing file remains unavailable, as everywhere else
in Oxide. Real managed builds, adoption, and production capacity policy stay
separately gated.
