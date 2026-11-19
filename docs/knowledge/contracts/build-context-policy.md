# Build-context policy

This contract owns the proposed rule for choosing a Rust build context: the
incremental state, debug level and partition a lease is bound to. It is a
design, not a shipped feature. Oxide does not select a context for any workflow,
and no resolver, flag or config key exists in source. Current lease behaviour is
owned by [local lease](local-lease.md); the opt-in profiles are owned by
[stable Cargo profiles](stable-cargo-profiles.md).

## Status

- Inert. Nothing in this document is activated.
- Consumer build policy stays consumer-owned. Incremental on or off for a
  consumer is that consumer's decision, not Oxide's.
- The local lease already retires known idle slots at lease time. The profile
  label on a lease does not set Cargo debug or incremental values.

## Principle

Choose the context from the expected lifetime of the edit and cache state, and
from the diagnostics the workflow needs. Do not choose it from whether the caller
is a human or an agent.

| Context | Expected lifetime | Diagnostics | Candidate default |
| --- | --- | --- | --- |
| `one-shot` | Throwaway or CI-style build; little reuse | Tests, assertions, file:line | Incremental off; line tables |
| `iterative` | Sustained edit loop in one retained exclusive slot | Same as one-shot | Incremental on; line tables |
| `full-debug` | Explicit capture or debugger session | Full debug info, source lookup | Explicit opt-in only; never inferred |

## Proposed resolution rules

1. A workflow declaration names its context.
2. `full-debug` applies only when requested explicitly per invocation. It is
   never a fallback or a side effect of another context.
3. Oxide resolves what Cargo and rustc will actually see: profile values, `CARGO_*`
   environment, configuration, rustflags, features and toolchain.
4. An explicit effective input beats a context default. A conflict with the
   declared context refuses; Oxide never silently picks a winner.
5. The resolved tuple selects the slot. There is no adaptive switching by
   history, size or time.

Unknown or ambiguous inputs fail closed.

Two requests that differ only in incremental state are different partitions,
because Cargo treats a changed compile input as different unit state. A context
name, a human or agent label, or a stopgap flag is never identity. Effective
values are fixed at acquire; a change is a new request and a new partition.

## Storage semantics

Keep these quantities separate. Do not add them.

- **Shared warm reuse.** Unchanged-unit hits inside one partition. Measure as
  rebuilt-unit counts and latency, not bytes.
- **Bounded growth.** Retained occupancy per partition. This is not a write count.
- **Actual writes.** Unknown unless an attributable measurement exists. Never
  inferred from occupancy, final binary size or SSD-life estimates.

Incremental off removes per-run incremental state. It does not promise lower
cumulative writes, because a changed crate may recompile without incremental
reuse. APFS clones and hardlinks can confound allocation; report allocated bytes
by unique inode.

## Caps, peak and retention

- The only hard cap is an enforced OS ceiling, such as a fixed-size sparse image
  or a filesystem quota. A sampled threshold refuses new acquires but is not a
  guarantee.
- A budget must cover peak space during an in-flight run, not only settled size.
- Only released, idle slots are retention candidates. Leased, uncertain or
  evidence-bearing storage never is.
- Evidence outranks budget. Budget pressure refuses or reports; it never deletes
  evidence or consumer output.
- Context-aware budgets or retirement beyond the local lease's idle-slot rules
  need their own brief.

## Ownership and release

- Oxide releases a slot only through the writer-closed `release` or `recover`
  rules of [local lease](local-lease.md).
- Unknown ownership, a crash, PID absence, age, compiler `.lock` files or a
  vanished process never authorise release or evidence removal.
- Budget pressure returns an actionable refusal with an inventory. It does not
  clean up.

Upstream garbage collection does not cover Oxide's needs. rustc session GC acts
only inside incremental session directories. Cargo's global cache GC covers
registry and Git sources, not build artifacts. Neither manages Oxide slots,
Cargo variants or debug-referenced objects. Oxide does not run periodic clean
churn or delete individual `*.o` files, because unpacked debug info refers to
them.

## Debug context

- The everyday line-table default keeps backtrace file:line. Context does not
  change assertions or test execution.
- `full-debug` is required for debugger variable and type inspection and for
  native evidence claims. It is not required to measure ordinary Cargo cost.
- Packed-at-capture or dSYM output is a candidate only. No retained mechanism
  binds an executable to its dSYM.

## Not claimed

- No shipped resolver, CLI or protocol.
- No shared warm slot or generation retirement beyond the local lease rules.
- No global Cargo or rustc configuration change.
- No consumer source, task or admission ownership.
