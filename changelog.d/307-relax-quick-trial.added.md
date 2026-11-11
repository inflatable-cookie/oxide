Added the Relax quick-run trial apparatus (oxide#307): the effect-free
`research:relax:test`/`research:relax:plan` selectors and the
planner-activation-gated heavy `research:relax:diagnostic`/`research:relax:measure`
selectors around `scripts/relax-quick-trial.py`, with committed fixture
crates under `fixtures/relax-quick-trial/`, comparing the pinned Relax
engine (makepad/relax `aa540c1468844af159e2f106d31966b84be9fdf1`) with
stable-LLVM quick execution on isolated fixtures under the qualified shared
supervisor. No product behaviour, recipe, default or dependency changes, no
engine build, acquisition or measurement is activated by this preparation.
