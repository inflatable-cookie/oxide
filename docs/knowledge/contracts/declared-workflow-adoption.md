# Declared Rust workflow opt-in

Status: implemented, unpublished, private-qualification only. One versioned
project/workflow declaration and one callable front end now exist over the
reviewed ordinary-Cargo storage route; the front end resolves a read-only plan
and binds that declaration to one exact owner-issued attempt before storage is
acquired. No consumer adoption, installation, published executable or real
project run is established. The storage mechanics remain owned by
[ordinary-Cargo storage](ordinary-cargo-storage.md); product authority remains
in [vision](../vision.md) and [architecture](../architecture.md).

This is the first explicit opt-in entry point of the "unify the Rust developer
experience" lane. It is a product front end and a worked path, not another
transport, scheduler or independent executor.

## What a project declares

A project owns exactly one declaration per supported workflow. The declaration
is reusable, non-secret policy intent and carries no task, run, source,
allocation, epoch, credential, endpoint, slot, retention or host-token
identity. The committed example is
`fixtures/ordinary-cargo-declaration-v1.json`:

```json
{
  "format": "oxide-workflow-declaration-v1",
  "project": {
    "root": "/absolute/path/to/project",
    "consumer": "nucleus",
    "project": "nucleus-core",
    "component": "nucleus-core",
    "workflow": "core-test"
  },
  "workspace": "workspace",
  "workflow": {
    "selector": "check:core:test",
    "argv": ["check:core:test", "--", "--message-format=json-render-diagnostics"]
  },
  "intent": { "profile": "dev", "incremental": true, "debug": "line-tables" },
  "environment_names": ["CARGO_HOME", "RUSTFLAGS"],
  "limits": {
    "storage_bytes": 1073741824,
    "capture": {
      "version": 1,
      "total_bytes": 268435456,
      "role_bytes": { "test-result": 268435456 }
    }
  }
}
```

- `format` is the only accepted declaration version. An unknown, older or
  newer format refuses; a declaration is never reinterpreted across versions.
- `project.root` is the declared project root. It must be an absolute,
  already-canonical, real directory. `workspace` is a relative path of normal
  components beneath it; the resolved workspace must canonicalize to itself and
  stay inside the declared root.
- `workflow.selector` is one exact token and `workflow.argv` is the exact
  argument array. A shell-command string, a hidden flag, quoting or an
  `environment` value map cannot be expressed and refuses as an unknown field.
- `environment_names` are policy names only. They must exactly match the
  environment key set of the owner-issued attempt; values never appear in the
  declaration, the plan or any refusal.
- `intent` maps onto the only two compatibility partitions: `dev` +
  `line-tables` selects `ordinary-test`, and `full-debug` + `full` selects
  `full-debug`. Non-incremental intent is unsupported and refuses, because the
  ordinary route depends on Cargo's incremental intermediates.
- `limits` are finite. `storage_bytes` is the requested acquire reservation and
  `capture` is the owner-installable per-role artifact receipt budget over the
  fixed role set `source-archive`, `cargo-log`, `test-result`, `binary` and
  `full-debug`.

Finite entry ceilings are enforced: declaration at most 64 KiB, rendered plan
at most 64 KiB, at most 256 argv entries of at most 16 KiB each, at most 64
environment names and at most 32 capture roles. Unknown fields, duplicate JSON
keys, mixed versions, oversized values, path/symlink escape and ambiguous
command forms all refuse before any effect.

## The read-only plan

`WorkflowPlan::resolve` produces one effective plan with no side effect: no
storage acquire, daemon provisioning, directory creation, compiler execution,
installation or host scan. `WorkflowPlan::render` prints the effective source
path, the exact command and environment-policy names, the selected
profile/partition, the requested finite limits, the owner grants the route
needs and every honest unresolved item. It never fabricates a compatibility
digest or a warm hit. The unresolved items it always reports are
`owner-grant-missing`, `compatibility-unknown`, `warm-reuse-unknown` and
`consumer-adapter-absent`.

The plan is explicit-caller-root only. Oxide performs no global file, host or
tool discovery; every relevant identity comes from the accepted owner boundary.

## What an operator or consumer must supply

The declaration is policy intent, never credential, admission, source, closure
or retention authority. Before an attempt can run, the owner must install:

- the protected oxide-storage runtime instance and its `OXSTOR01` version-2
  endpoint;
- an ordinary-Cargo **policy-3** acceptance credential plus the exact ordinary
  request digest in the installed declared-workflow allowlist;
- the owner callback key and the `ordinary-cargo-attempt-v3` callback context;
- the consumer's existing `ordinary_cargo.submit`, `ordinary_cargo.capture` and
  `ordinary_cargo.not_submitted` owner; and
- the task/run/source, root-allocation and allocation-epoch identities that the
  declaration deliberately does not carry.

## Execution binding

`run_declared_attempt(bootstrap, declaration)` resolves the declaration, then
binds it to the protected owner-issued attempt **before** any acquire:

1. the bootstrap must carry the exact resolved declaration digest, and that
   digest must be inside the v3 `input_boundary_digest` the owner signed;
2. the observed source root, consumer/project/component/workflow identity,
   selector, argv, profile, partition, storage reservation, capture ceilings and
   environment-policy name set must equal the declaration exactly;
3. a changed declaration, selected profile/incremental/debug intent or capture
   ceiling is a different exact boundary and refuses.

Only then does it delegate to the unchanged single ordinary-Cargo route with
its one submit, one capture and authenticated finalize. There is no second
executor, no shell-command concatenation, no hidden flag, no ambient
environment inheritance, no alternate Effigy submission, no process-wide Cargo
override and no automatic fallback. The selected tests still execute through
the consumer callback; the front end does not itself run Cargo.

Mismatch, forged or absent authority, busy/full/held/version refusal,
cancellation or lost replies produce bounded outcomes. Uncertainty never frees
a lease or starts a second run.

## Policy-3 route

The private route now has three explicitly versioned routes that are never
relabelled, migrated or reinterpreted:

| Route | Version | Operation | Storage policy | Boundary subject |
| --- | --- | --- | --- | --- |
| exact | 1 | `ordinary-cargo-attempt-v1` | 1 | version-1 exact digest |
| capture-budget | 2 | `ordinary-cargo-attempt-v2` | 2 | version-2 digest plus capture budget |
| declared workflow | 3 | `ordinary-cargo-attempt-v3` | 3 | `oxide-ordinary-cargo-input-boundary-v3` |

Policy 3 admits and settles exactly like policy 2 within one allocation epoch
and additionally opts its generation in to one explicit owner-authenticated
cross-allocation handoff. The v3 input boundary carries its own schema marker
and the exact declaration digest, and v3 has its own callback context domain, so
a v1/v2 signature, request digest, eligibility reference or callback receipt is
never accepted as v3. The legacy 16 MiB v1 capture cap, the v2 owner-installed
capture policy and every existing v1/v2 signature stay byte-for-byte unchanged;
`fixtures/ordinary-cargo-auth-v1.json` and `ordinary-cargo-auth-v2.json` are
untouched and the new vectors live in `ordinary-cargo-auth-v3.json`.

The installed qualification authority holds a separate declared-workflow digest
allowlist. An empty allowlist keeps the declared route refused; the policy-2
ordinary helper does not grant policy 3.

Handoff stays an explicit, separately authenticated owner operation. The front
end can report required handoff/retention evidence and can run an accepted
successor allocation; it does not issue handoff proofs, recycle roots or relabel
storage.

## Callables

In Unix builds with the `private-qualification` feature:

- library: `oxide_storage::WorkflowPlan`,
  `oxide_storage::ordinary_cargo::verify_declared_bootstrap` and
  `oxide_storage::ordinary_cargo::run_declared_attempt`;
- CLI: the unpublished `oxide-declared-workflow` binary, with
  `oxide-declared-workflow plan <declaration>` (read-only, exit 0 or 1) and
  `oxide-declared-workflow run <declaration> <bootstrap>` (exit 0 finalized,
  1 refused, 2 held, 3 process error).

Some declaration validation and the whole plan surface are also available
without the private feature through the same library types.

## Worked fixture recipe

`project::tests::private_ordinary_cargo_process::declared_workflow_front_end_plans_binds_and_reuses_across_handoff`
is a complete runnable private recipe, run by `effigy rust:private-process`:

1. write the declaration and run the `plan` CLI, asserting a bounded,
   effect-free plan and unchanged fixture directory contents;
2. build three exact owner-issued policy-3 attempts at one physical root — two
   in allocation A and one in allocation B — plus the separate declared-workflow
   digest allowlist;
3. prove a declaration whose resolved argv differs is refused **before** any
   acquire (the daemon inventory stays empty);
4. run the compiled `oxide-declared-workflow run` child for the cold attempt,
   score one synthetic submit/capture callback exchange, and finalize against
   the real in-process daemon;
5. repeat the same allocation with a new run/attempt identity and exact
   boundary, asserting the same slot, generation and storage instance with
   `reused=true`;
6. perform one explicit owner-authenticated handoff to the successor allocation,
   asserting the same bytes and identity survive and a forged credential is
   unauthorized;
7. run the accepted successor-allocation attempt, asserting reuse of the
   handed-off generation, then prove prior history stays access-free.

The source, host-run and capture authorities in that fixture are synthetic and
absent in production. It is not a real Cargo or Nucleus adoption and measures no
saving. The callback receipt is authenticated fixture metadata; no artifact is
created, read, hashed or retained.

## What still refuses

- No current Nucleus or Effigy adapter consumes a declaration. A consumer
  integration must build on Oxide's actual reviewed handover and the
  consumer's own owners; the declaration does not install one.
- No production authentication issuer, installation policy, accepted real
  input class or full-debug/source retention lookup exists.
- Handoff, retirement, invalidation, older/equal-mtime source transition and
  installation/no-replacement acceptance remain installed owner duties with
  their own separate grants.
- Real-project qualification, consumer adoption, host installation and any
  published executable naming remain separately gated.
