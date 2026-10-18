# Bounded payload storage backend

The hard-bounded APFS payload capability remains in use by the unpublished
[local lease CLI](local-lease.md). This contract records its retained storage
semantics; [architecture](../architecture.md) owns the wider invariants. The
former daemon lifecycle and protocol have been retired.

## Boundary

The supported hard-bound shape is a fixed-size private APFS volume inside a
sparse disk image. The lease CLI provisions and opens it through the shared
`BoundedBacking` capability. The operating system refuses allocation beyond
the volume capacity, while the ledger separately reserves against its
configured ceiling.

The bound is aggregate, not per-slot. All owned slots share one finite payload
filesystem. Reservation accounting is an acquire-time estimate: a fully
written payload can use more than its reservation while remaining within the
verified volume capacity. Growth either uses settled idle capacity or refuses
with a known or unknown accounting result. ENOSPC during a lease is not writer
closure or release; uncertain ownership remains held.

The lease ledger and capacity receipt stay outside a payload that can become
full. The bounded payload contains only owned slot directories; unknown entries
refuse.

## Retained capability

- `BoundedBacking` and `BoundedBackendDescriptor` represent the verified
  capability and the `apfs-sparse-image-v1` backend. Construction is
  crate-private, so caller input cannot turn a directory into a hard-bound
  claim.
- The lease ledger records the backend tag, canonical payload root, image
  identity and verified capacity. Reopen refuses a mismatch instead of
  reinterpreting the pool.
- Finite control-history limits refuse new records at exhaustion instead of
  pruning ownership evidence or growing the ledger indefinitely.
- Provisioning creates and attaches a fresh image, makes the volume root
  owner-only, verifies APFS at the exact mount point, and records a capacity
  receipt bound to image identity and capacity. Opening rechecks those facts.
- Directory-only roots are budgeted but are not a product mode of the lease
  CLI. Unsupported platforms and unknown backend tags refuse without fallback.

## Historical filesystem-full proof

oxide#055 ran one real macOS filesystem-full proof through the since-retired
storage daemon: one fresh owned 128 MiB APFS sparse image, a writer bounded by
128 MiB with 1 MiB IO buffers, 512 MiB owned scratch, the 20 GiB host reserve
and a 120-second native phase. It observed genuine ENOSPC under an open lease,
control-ledger durability with the payload full, settlement, restart reopening
the same backing and warm reuse. The `BoundedBacking` structural checks it
relied on are unchanged and keep a unit test; the daemon-side ledger binding
and its selector were retired.

## Validation and limits

`effigy rust:storage-lease-cli` owns the current fresh 128 MiB APFS pool proof
for lease, release, recovery and status. It does not fill the filesystem or
repeat the historical daemon proof. The [local lease contract](local-lease.md)
records its bounds and evidence.

Failed or uncertain task-owned fixtures are retained rather than force-cleared.
No consumer activation, installation, global host settings, live cache cleanup,
per-slot quota isolation, or Linux backend follows from this contract. The
image backing file's host write attribution remains unavailable. Real managed
builds, adoption and production capacity policy stay separately gated.
