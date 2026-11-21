# Open questions

This file owns unresolved design decisions only. Settled rulings live in [vision](vision.md) and their owning contracts; a question leaves this file once it is settled. Question numbers are stable identifiers and may have gaps.

## Q-001 — What implementation shape best supports durable exclusive ownership?

Open. A per-user local daemon remains the recommended production hub for one durable lease ledger. The kept implementation is the same-user lease library in [local lease](contracts/local-lease.md). Still open: proof on the selected host that every slot writer and outstanding write capability ends or is fenced, and daemon failure and restart behaviour. macOS process scans are not descendant-complete proof, so the cooperative foreground contract stands.

## Q-002 — What exact toolchain and checkout-path portability can be supported?

Open for useful cross-path reuse. The mapped-root and logical-path route is retired. Ordinary Cargo reuse across fresh checkout paths is limited to the narrower result in [project adoption](contracts/project-adoption.md). Open: whether a consumer can supply a finite pool of physical task-worktree roots with exclusive allocation and recycling. Remapping stays off until a supported Cargo demonstrates reuse without a per-root map; each Cargo or rustc upgrade needs reassessment.

## Q-005 — What names are safe to publish?

Settled by Tom's 2026-11-20 ruling: the public GitHub repository and binary keep the name `oxide`, the package stays `publish = false` and is not published to crates.io, and distribution is by git tag for now with prebuilt binaries later. The [README](../../README.md#name) states that Oxide is not affiliated with Oxide Computer Company or `oxidecomputer/oxide.rs`. Owner: [release](contracts/release.md).

## Q-006 — What experience and budgets define light Rust development?

Open for numeric targets. Tom's direction is confirmed. Still to choose: representative workflows, acceptable disk footprint, write volume, feedback latency, memory and operator-effort targets. Owner: [vision](vision.md).

## Q-008 — Which host/container division preserves required production parity?

Open. Native feedback, Linux builds and tests, and Colima storage need separate measurements and ownership. Owner: [vision](vision.md).

## Q-009 — Which alternative execution modes improve the daily loop?

Open research. Compare fast ahead-of-time compilation, JIT and hot reload by complete edit-to-feedback cost and supported semantics. Mechanisms, compatibility and validation mapping are not yet chosen. Owner: [Rust DX landscape](rust-dx-landscape.md).

## Q-011 — How should slot waiting coordinate with host admission?

Open in part. The design direction (non-reserving, cancellable waiting, advisory readiness, authoritative admission before exclusive acquire) is in [project adoption](contracts/project-adoption.md#readiness-admission-and-acquire-ordering). Consumer lifecycle mapping is Q-012; numeric waiting targets are Q-006.

## Q-012 — How does the consumer bind task admission, storage acquire and host-run scheduling?

Open. The consumer boundary and core-test work are tracked in Nucleus. Open gates: an installed, authenticated Oxide binding; a qualified source pool; agreement on the opt-in, allocation epochs, bounded retention and exact source identity before root recycling; third-party and native input audit; and a successful live recovery. Storage busy creates no host run.

## Q-017 — Can sccache sharing preserve Oxide's slot writer closure?

Open, with a no-go for the current shared-daemon route. The normal detached sccache daemon runs outside Oxide's process group, so closure cannot be established (the sccache-096 receipts were removed from the tree on 2026-11-20 and survive in git history). A serial foreground server per build over one cache may be source-attributable, but its closure after cancellation or crash and its concurrency are unproven. Cache keys also depend on slot paths. Compatibility is unknown, the benefit is insufficient, and no production wrapper is enabled. A future attempt needs a cancellation and crash closure proof, a crash-safe cache lock, concurrency handling and a cache key independent of slot and checkout paths.

## Q-018 — How do per-command admission and execution budgets behave after freeze?

Implemented as the frozen v1 budget contract in [workflow](contracts/workflow.md). Still open: an inner-deadline inheritance interface for nested commands, which never inherit or reset a deadline today, and whether a future event transport should carry more than the v1 start and terminal records.

## Q-019 — How should `oxide exec` know a tool's profile output members?

Open, narrowed. The receipt-derived member set is implemented: [exec output inference](contracts/exec-output-inference.md) derives it from the checkout's committed placement, so `--output` need not be declared. Still open: whether a derived member may ever be newly linked (this needs a keyed reservation and a custody change), whether a derived set may exceed the eight-member declaration cap, and whether a later slice adds a `cargo` launcher guard.

## Q-020 — How should nested Oxide lease joining and permit borrowing work?

Open. Nested calls currently take independent leases, and idle exec sessions hold their host permit throughout. The future joining and permit-borrowing design is tracked in Nucleus. Open: the broker interface; implementation order; deadline inheritance (Q-018); hostile same-UID code; and non-macOS and Linux platforms.

## Q-022 — Can a held zombie establish its group's lifetime?

Open. The positive proposition that a held zombie's process group lifetime is established remains unproved. Zombie-row PGID equality is an unverified premise. A weaker sampled held-identity observation was authorized, but it does not establish group or session membership and grants no signal authority.
