# Release contract

## Operator ruling (2026-11-20)

Tom ruled how Oxide goes public:

- The repository is public on GitHub as `inflatable-cookie/oxide`, under the MIT licence in `LICENSE` (copyright inflatable-cookie).
- The name `oxide` is kept for the repository and the binary. The package stays `publish = false` and is not published to crates.io.
- Releases are git tags with prebuilt binary tarballs published on GitHub Releases via explicit `workflow_dispatch`.
- The README states that Oxide is not affiliated with Oxide Computer Company or `oxidecomputer/oxide.rs`.

Supported toolchains are stated in the README: Rust 1.100.0 builds Oxide, and consumer projects are supported on Cargo 1.95.0 through 1.100.0.

Creating a tag, bumping the version in `Cargo.toml` and changing release workflows still need explicit scoped authority. An approved implementation brief does not grant that authority.

## GitHub Actions workflows (2026-11-21)

Tom authorized adding `.github/workflows/ci.yml` and `.github/workflows/release-binaries.yml` on 2026-11-21.

### Continuous integration (`.github/workflows/ci.yml`)

- Triggers on `pull_request` (`opened`, `synchronize`, `reopened`), `push` to `main`, and `workflow_dispatch`, cancelling superseded runs per PR or ref.
- Reads the pinned Rust channel from `rust-toolchain.toml` rather than floating `stable`, and installs `rustfmt`.
- Runs three jobs:
  - `lint` (`ubuntu-24.04`): `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --locked`, `RUSTFLAGS='-D warnings' cargo build --release --locked --bin oxide`, `python3 scripts/changelog.py check`, `bash scripts/changelog.test.sh`, and `python3 -m unittest discover -s scripts -p 'test_dogfood_scripts.py'`.
  - `test-macos` (`macos-15`): installs Cargo `1.97.1` alongside the pinned toolchain and runs `cargo test --workspace --locked -- --test-threads=1` with a clean canonical `0700` scratch root (`TMPDIR`, `TMP`, `TEMP`, `OXIDE_TASK_SCRATCH_ROOT`, `OXIDE_TEST_INTERNAL_ROOT`).
  - `check-linux` (`ubuntu-24.04`): runs `cargo check --workspace --all-targets --locked` and `RUSTFLAGS='-D warnings' cargo build --release --locked --bin oxide` (library and integration test suites stay macOS-gated until their macOS-only assumptions are decoupled).

### Binary releases (`.github/workflows/release-binaries.yml`)

- **Explicit dispatch only:** triggered solely by `workflow_dispatch` with required input `tag` (an existing annotated `vX.Y.Z` semver tag). Tag pushes and branch pushes never auto-publish releases. Requires `contents: write`.
- **`gates` job:** checks out the requested tag, verifies it is an annotated `tag` object matching `Cargo.toml`'s `[package]` `version` for `oxide-storage`, refuses if a GitHub Release for that tag already exists, and runs `cargo fmt --all -- --check`, `python3 scripts/changelog.py check`, and `RUSTFLAGS='-D warnings' cargo build --release --locked --bin oxide`.
- **`build` matrix** (fail-fast, pinned toolchain from `rust-toolchain.toml`):
  - `aarch64-apple-darwin` on `macos-15` (native)
  - `x86_64-apple-darwin` on `macos-15-intel` (native)
  - `x86_64-unknown-linux-gnu` on `ubuntu-22.04` (native)
  - `aarch64-unknown-linux-gnu` on `ubuntu-22.04` (cross-compiled via `taiki-e/setup-cross-toolchain-action@v1`)
  Each build compiles `oxide` with `RUSTFLAGS='-D warnings' cargo build --release --locked --bin oxide --target <target>`, smoke-tests native binaries (`--version` and `--help`), packages `oxide-<tag>-<target>.tar.gz` containing `oxide`, `LICENSE` and `README.md`, and uploads the artifact.
- **`release` job:** downloads all four tarballs, writes and verifies `SHA256SUMS.txt` (`sha256sum -c`), extracts the `[<version>]` section from `CHANGELOG.md` (falling back to `--generate-notes` if empty), re-checks that the release does not already exist, and publishes the GitHub Release via `gh release create --verify-tag`.

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
