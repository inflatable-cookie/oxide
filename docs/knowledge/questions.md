# Open questions

This file owns unresolved design decisions only. Settled rulings live in [vision](vision.md) and their owning contracts; a question leaves this file once it is settled. Headings keep their numbers so existing links still resolve.

## Q-001 — What implementation shape best supports durable exclusive ownership?

Open. A per-user local daemon remains the recommended production hub for one durable lease ledger. The kept implementation is the same-user lease library in [local lease](contracts/local-lease.md). Still open: proof on the selected host that every slot writer and outstanding write capability ends or is fenced, and daemon failure and restart behaviour. macOS process scans are not descendant-complete proof, so the cooperative foreground contract stands.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Open for useful cross-path reuse. The mapped-root and logical-path route is retired. Ordinary Cargo reuse across fresh checkout paths is limited to the narrower result in [project adoption](contracts/project-adoption.md). Open: whether a consumer can supply a finite pool of physical task-worktree roots with exclusive allocation and recycling. Remapping stays off until a supported Cargo passes the full qualification gate without a per-root map, and each Cargo or rustc upgrade needs a reassessment.

## Q-003 — What partitions, budgets and reconciliation proofs are sufficient?

Open for production capacity. Still needed: an enforced disposable-footprint cap, the production host locking and process containment, and later product budgets. The 20 GiB host free-space threshold is a monitored start and abort threshold, not a guarantee; see [architecture](architecture.md#storage-invariants).

## Q-005 — What names are safe to publish?

Open. Review executable, crate and distribution names against the existing Oxide Computer Company SDK and CLI before publication. Owner: [release](contracts/release.md).

## Q-006 — What experience and budgets define light Rust development?

Open for numeric targets. Tom's direction is confirmed. Still to choose: representative workflows, acceptable disk footprint, write volume, feedback latency, memory and operator-effort targets. Owner: [vision](vision.md).

## Q-008 — Which host/container division preserves required production parity?

Open. Native feedback, Linux builds and tests, and Colima storage need separate measurements and ownership. Owner: [vision](vision.md).

## Q-009 — Which alternative execution modes improve the daily loop?

Open research. Compare fast ahead-of-time compilation, JIT and hot reload by complete edit-to-feedback cost and supported semantics. Mechanisms, compatibility and validation mapping are not yet chosen. Owner: [Rust DX landscape](rust-dx-landscape.md).

## Q-011 — How should slot waiting coordinate with host admission?

Open in part. The design direction (non-reserving, cancellable waiting, advisory readiness, authoritative admission before exclusive acquire) is in [project adoption](contracts/project-adoption.md#readiness-admission-and-acquire-ordering). Consumer lifecycle mapping is Q-012; numeric waiting targets are Q-006.

## Q-012 — How does the consumer bind task admission, storage acquire and host-run scheduling?

Open. The consumer boundary and first core-test contract are in [Nucleus core reuse](contracts/nucleus-core-reuse.md). Open gates: an installed, authenticated Oxide binding; a qualified source pool; agreement on the opt-in, allocation epochs, bounded retention and exact source identity before root recycling; third-party and native input audit; and a successful live recovery. Storage busy creates no host run.

## Q-015 — What does the foundation workflow need settled before implementation?

Open decisions W-1 to W-8 (default pool, waiting and concurrency, project file, crash-recovery qualification, Linux backend, command naming and the cross-checkout freshness gate) are listed in [workflow](contracts/workflow.md). Retirement execution and crash-recovery direction are settled.

## Q-016 — How should Oxide proceed after the #086 no-go?

Paused. Tom paused the build-script skip path and withdrew its readers ([vision](vision.md#product-workflow-direction), 2026-10-19). No skip or reader coverage is admitted. The shipped freshness gate is qualified on the Cargo pins listed in the [workflow matrix](contracts/workflow.md#cargo-version-matrix); future Rust qualification covers those shipped features only. The #086 normal-path overhead result is a final no-go.

## Q-017 — Can sccache sharing preserve Oxide's slot writer closure?

Open, with a no-go for the current shared-daemon route. The normal detached sccache daemon runs outside Oxide's process group, so closure cannot be established ([evidence](evidence/sccache-096/README.md)). A serial foreground server per build over one cache may be source-attributable, but its closure after cancellation or crash and its concurrency are unproven. Cache keys also depend on slot paths. Compatibility is unknown, the benefit is insufficient, and no production wrapper is enabled. A future attempt needs a cancellation and crash closure proof, a crash-safe cache lock, concurrency handling and a cache key independent of slot and checkout paths.

## Q-018 — How do per-command admission and execution budgets behave after freeze?

Implemented as the frozen v1 budget contract in [workflow](contracts/workflow.md). Still open: an inner-deadline inheritance interface for nested commands, which never inherit or reset a deadline today, and whether a future event transport should carry more than the v1 start and terminal records.

## Q-019 — How should `oxide exec` know a tool's profile output members?

Open leanings, not settled. The design in [exec output inference](contracts/exec-output-inference.md) derives the member set from the checkout's committed placement, so `--output` need not be declared. Open: whether a derived member may ever be newly linked (this needs a keyed reservation and a custody change), whether a derived set may exceed the eight-member declaration cap, and whether a later slice adds a `cargo` launcher guard. No code is activated by the design.

## Q-020 — How should nested Oxide lease joining and permit borrowing work?

Open. The proposal is in [nested lease](contracts/nested-lease.md) and is not implemented or frozen. Open: the broker interface for a joined compile; which of the nested and [idle exec](contracts/idle-exec-sessions.md) designs lands first; deadline inheritance (Q-018); hostile same-UID code; and non-macOS and Linux platforms.

## Q-021 — How should Oxide's output roots move to Barge?

Open, with a proposed design in [Barge adoption](contracts/barge-adoption.md). Operator choices remain: the Builds root directory, whether outputs reserve space, release from maintenance, handling of the 91 legacy roots, parity differences D-1 and D-2, pool-slot ownership, declaration authority, and whether to run the read-only census first. Barge-side gaps need the Barge owner's ruling first.

## Q-022 — Can a held zombie establish its group's lifetime?

Open. The positive proposition that a held zombie's process group lifetime is established remains unproved. Zombie-row PGID equality is an unverified premise. A weaker sampled held-identity observation was authorized, but it does not establish group or session membership and grants no signal authority.
