# Release contract

## Operator ruling (2026-11-20)

Tom ruled how Oxide goes public:

- The repository is public on GitHub as `inflatable-cookie/oxide`, under the MIT licence in `LICENSE` (copyright inflatable-cookie).
- The name `oxide` is kept for the repository and the binary. The package stays `publish = false` and is not published to crates.io.
- Releases are git tags for now. Prebuilt binaries come later.
- The README states that Oxide is not affiliated with Oxide Computer Company or `oxidecomputer/oxide.rs`.

Supported toolchains are stated in the README: Rust 1.100.0 builds Oxide, and consumer projects are supported on Cargo 1.95.0 through 1.100.0.

Creating a tag, bumping the version in `Cargo.toml` and changing release workflows still need explicit scoped authority. An approved implementation brief does not grant that authority. There is no repository-owned release procedure or prebuilt-binary packaging yet.

## Local installation

The planner canaries a staged, pushed-main pin on the live pool before installing
a storage-affecting build. Install and reinstall accept `--require-canary`
to enforce a same-pin PASS less than an hour old. See
[dogfooding in Oxide's repository](workflow.md#dogfooding-in-oxides-repository)
for the timing limits, receipt rules and compatibility check.

## Unreleased changes

Each pull request adds one short entry at `changelog.d/<task-or-PR>-<slug>.<type>.md`. The type is `added`, `changed`, `fixed`, `removed` or `security`; keep the body to one entry of a line or two. Keep the `[Unreleased]` section in `CHANGELOG.md` empty. Run `effigy changelog:check` as part of `effigy docs:check`.

The repository-owned checker is `scripts/changelog.py`, copied unchanged from Northstar's published PR #81 merge `9f5d50a15c71f6f3ee269f65ba0222009996af9b`, path `skills/northstar/scripts/changelog.py`, SHA-256 `e28f482e7ba129b4ada9f6a6c806aac3ab5e9de76cfede203c81e4538e9ad387`. Its synthetic CLI test is based on `scripts/changelog.test.sh` from the same merge; only its temporary-directory allocation was adapted to honor the run's `TMPDIR`.

Release preparation may fold fragments only with existing explicit release authority, using `effigy changelog:fold --version <x.y.z> [--date YYYY-MM-DD]` (add `--dry-run` to preview). The fold command inserts a stable type/name-ordered release section, removes folded fragments and keeps `changelog.d/.gitkeep`. Do not fold fragments as part of ordinary pull requests.
