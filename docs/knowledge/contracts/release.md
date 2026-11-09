# Release contract

No release, executable, crate, supported toolchain range or distribution procedure is defined. Publication is not authorized.

Before a first publication, Tom must approve executable/crate/distribution naming, supported toolchain policy, packaging and a repository-owned release procedure. The selected repository name does not settle public product naming: Oxide Computer Company already has a Rust SDK/CLI at <https://github.com/oxidecomputer/oxide.rs>.

Release preparation/execution, tags and workflow changes require explicit scoped authority. An approved implementation brief does not grant release authority. Establish the procedure and required validation evidence before any release action.

## Unreleased changes

Each pull request adds one short entry at `changelog.d/<task-or-PR>-<slug>.<type>.md`. The type is `added`, `changed`, `fixed`, `removed` or `security`; keep the body to one entry of a line or two. Keep the `[Unreleased]` section in `CHANGELOG.md` empty. Run `effigy changelog:check` as part of `effigy docs:check`.

The repository-owned checker is `scripts/changelog.py`, copied unchanged from Northstar's published PR #81 merge `9f5d50a15c71f6f3ee269f65ba0222009996af9b`, path `skills/northstar/scripts/changelog.py`, SHA-256 `e28f482e7ba129b4ada9f6a6c806aac3ab5e9de76cfede203c81e4538e9ad387`. Its synthetic CLI test is based on `scripts/changelog.test.sh` from the same merge; only its temporary-directory allocation was adapted to honor the run's `TMPDIR`.

Release preparation may fold fragments only with existing explicit release authority, using `effigy changelog:fold --version <x.y.z> [--date YYYY-MM-DD]` (add `--dry-run` to preview). The fold command inserts a stable type/name-ordered release section, removes folded fragments and keeps `changelog.d/.gitkeep`. Do not fold fragments as part of ordinary pull requests.
