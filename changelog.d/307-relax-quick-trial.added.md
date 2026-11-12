Added the Relax quick-run trial apparatus (oxide#307): the effect-free
`research:relax:test`/`research:relax:plan` selectors and the
planner-activation-gated heavy `research:relax:diagnostic`/`research:relax:measure`
selectors around `scripts/relax-quick-trial.py`, with committed fixture
crates under `fixtures/relax-quick-trial/`, comparing the pinned Relax
engine (makepad/relax `aa540c1468844af159e2f106d31966b84be9fdf1`) with
stable-LLVM quick execution on isolated fixtures under the qualified shared
supervisor. Final scoped outcome: a compatibility no-go for the tested
pin — the one reached snapshot case recorded 20 unresolved
`core::arch::aarch64` intrinsics in `zmij-1.0.23` under the pinned engine
(hash-bound retained stderr), the resource occurrence was budget-stopped
UNCLOSED at 4100.6/4096 MiB with no receipt, samples or timings, and the
recommendation is not to integrate this pinned engine; scoped to this
tested pin/platform, preserving the apparatus and histories. No product
behaviour, recipe, default or dependency changes.
