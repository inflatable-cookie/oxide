# Bounded payload storage backend

The hard-bounded APFS payload capability remains in use by the unpublished
[local lease CLI](local-lease.md). This contract records its retained storage
semantics; [architecture](../architecture.md) owns the wider invariants. The
former daemon lifecycle and protocol have been retired.

## Boundary

The supported hard-bound shape is a fixed-size private APFS volume inside a
sparse disk image. The lease CLI provisions and opens it and mints the
`BoundedBacking` capability only after live verification. The operating system
refuses allocation beyond the verified volume capacity. The declared pool cap
must equal the image size; a smaller declared cap on a larger volume refuses,
because admission accounting alone is not a quota.

The bound is aggregate, not per-slot. All owned slots share one finite payload
filesystem. There is no per-lease reservation: lease-time admission compares
the owned slots' measured unique allocated bytes with the pinned cap, evicts
only known idle whole slots, and refuses on held occupancy or unknown
accounting. A lease does not constrain writes after it is granted; a build can
still hit ENOSPC, which is not writer closure or release, and the lease stays
held. [Local lease](local-lease.md#capacity-and-eviction) owns the exact
admission, half-cap recycling and eviction rules.

Pool config, ledger and capacity receipt stay outside a payload that can become
full. The payload holds the owned `slots` directory plus the known
macOS-created metadata directories (`.fseventsd`, `.Spotlight-V100`,
`.Trashes`, `.TemporaryItems`); any other entry, a symlink or a foreign-volume
entry refuses.

## Retained capability

- `BoundedBacking` and `BoundedBackendDescriptor` represent the verified
  capability and the `apfs-sparse-image-v1` backend. Construction is
  crate-private, so caller input cannot turn a directory into a hard-bound
  claim. Structural checks refuse an unknown backend, zero capacity, a
  non-hex or oversized identity, and a relative, unreadable, non-private,
  symlinked or non-canonical payload root.
- The pool config records the backend tag, store ID, cap and image size. The
  version-2 capacity receipt records the image path, mount point, image size,
  payload ceiling, verified capacity and image identity. Reopen refuses a
  mismatch instead of reinterpreting the pool.
- Control state is finite (`MAX_LEDGER_BYTES` 256 KiB, `MAX_SLOTS` 32); new
  slots refuse at exhaustion rather than dropping recorded ownership.
- Provisioning creates and attaches a fresh image, makes the volume root
  owner-only, verifies APFS at the exact mount point and writes the receipt.
  Opening rechecks those facts and never attaches a replacement.
- Directory-only roots are not a product mode of the lease CLI. Unsupported
  platforms and unknown backend tags refuse without fallback.

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
