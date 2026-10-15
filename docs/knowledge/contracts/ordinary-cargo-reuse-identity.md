# Ordinary-Cargo reuse identity

This contract defines the versioned identity boundary for the ordinary-Cargo
storage API and its private qualification route. It supplements the
[ordinary-Cargo storage contract](ordinary-cargo-storage.md) and does not
qualify Cargo freshness.

## Policy versions

`ORDINARY_CARGO_POLICY_VERSION` remains version 1. Version 1 keeps its existing
semantics: a reusable generation accepts another request only when that
request has the same `input_boundary_digest` as the generation's recorded
attempt. Its opaque `acceptance_reference` is not reinterpreted.

`ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION` is version 2. It separates the
generation's stable compatibility eligibility from each exact attempt. A v1
generation cannot be relabelled or upgraded by a v2 request; mismatched
versions are incompatible. Unsupported policy versions and malformed v2
references refuse.

## Stable eligibility reference

Version 2 uses the existing `OrdinaryCargoAcceptance.acceptance_reference`
slot with this strict form:

```text
oxide-ordinary-cargo-stable-eligibility-v2/<invalidation-sha256>/<compatibility-sha256>
```

Each digest is `sha256:` followed by 64 lowercase hexadecimal digits. The
compatibility digest hashes a typed, fixed-field-order JSON object with schema
`oxide.ordinary-cargo.stable-reuse.v2`. The public
`ordinary_cargo_stable_reuse_reference(request,
qualified_invalidation_declaration_digest)` helper constructs it; storage
parses and recomputes it before admission and when validating ledger history.

The typed compatibility material binds:

- the ordinary partition label and the full Cargo `PartitionKey`: project
  component, workflow, Cargo and rustc provenance, host and target triples,
  profile, sorted features, configuration and environment digests, and path
  identity;
- the strict physical source-root path bytes, consumer root ID and allocation
  epoch;
- policy version, installation epoch, and the accepting authority's digest
  for its qualified supported-invalidation declaration.

The installed policy must include any behavior-relevant native compiler,
linker, SDK, and environment identities in the accepted provenance,
configuration, or environment digests. The reference binds those exact
values; it does not infer their completeness or qualify them for a real
workflow.

The reference excludes task, workspace, run and attempt IDs; commit and source
tree/content digests; branch; command and argv; the per-run target directory;
and per-run temporary and capture paths. The target triple and compatibility
digests remain stable inputs. These excluded attempt details remain bound by
the exact `input_boundary_digest` and original authenticated request.

The helper only computes a reference. It does not install policy or certify
the declaration. Policy v2 admission requires both the existing
`EvidenceAuthority::verify_ordinary_cargo_admission` exact-request check and
the new `verify_ordinary_cargo_stable_reuse_policy` check. The new hook has a
rejecting default. An accepting authority must verify that this exact stable
reference and invalidation declaration were installed for the current
consumer/root/installation; task JSON, a caller-generated digest, or the
helper alone cannot grant eligibility. Existing generic admission and
ordinary settlement checks remain necessary.

## Exact attempts and durable history

Each request keeps its own exact `input_boundary_digest`. Storage hashes the
complete original request, including that digest and the credentials, for
acquire replay and lookup. Finalize and finalize lookup repeat and authenticate
the original request and exact settlement evidence. A stable eligibility
reference never authorizes replay under another run, source, command,
credential, or capture. Exact lookup remains access-free.

The ledger retains each historical lease's own request and request digest.
For v1, generation validation continues to require the generation digest to
match each associated lease request. For v2, the generation retains its first
attempt digest as evidence, while validation checks that every historical
request recomputes to the same stable reference; it does not compare a later
attempt's digest with the generation's first digest. The generation still
has one root allocation and ordinary partition, one exclusive pending lease,
the existing success/failure/no-spawn transitions, fixed partition capacity,
and no eviction. Restart, old exact lookup, exact replay and current finalize
lookup retain their existing token and uncertainty rules.

The storage request encoding and ledger record shape do not change. The
transport protocol stays version 2; ledger version 5 now also records the
durable backend mode, and version-4 ledgers refuse without migration. The
explicit per-record `policy_version` selects the semantics.
Version 1 records retain their old checks. A v2 reference is strictly parsed,
and any unsupported policy version or ledger/protocol version is refused
without migration or mutation.

## Integration boundary and evidence limits

The feature-gated private route keeps its version-1 contract and 16 MiB cap
unchanged. Its separately versioned version-2 operation accepts storage policy
v2 only with an owner-installed finite capture budget and an exact per-attempt
input-boundary digest. The boundary includes the budget version, aggregate
ceiling, exact role ceilings, full run/source/command identity, and output
paths. Storage admission authenticates the complete ordinary request; the
protected owner's existing exact-request digest allowlist therefore binds the
stable reference and that attempt's capture policy together. The
`QualificationAuthority` stable-policy check recomputes the reference and
requires that exact request to be owner-allowlisted. Its helper and task data
cannot install policy.

The acquired tuple, launch digest, callback request, capture-budget digest,
callback HMAC, and finalize request retain the same policy and exact attempt
boundary. A budget change requires a new signed input boundary and exact
request authorization. It cannot be substituted after acquire or replayed
under a prior request. This route integration does not install it in Nucleus or
establish an end-to-end consumer fix.

The feature-scoped storage fixture calls the existing
`ordinary_cargo_input_boundary_digest` helper for distinct synthetic runs,
source revisions, and target/temp/capture paths. It proves storage semantics
for accepted metadata only. It does not establish that Cargo observed every
relevant input, that the invalidation declaration is qualified for a real
project, that source/debug evidence was retained by a consumer, or that real
build work or SSD writes are reduced.
