#!/usr/bin/env python3
"""Prove that leased `oxide` builds bypass an inherited rustc wrapper (oxide#099).

Rewritten for oxide#131: this is one fixed script whose whole command is the
qualified shared proof supervisor (`scripts/bounded-test.py`, oxide#109):

    python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- \
        python3 scripts/test-oxide-wrapper-bypass.py

The supervisor owns the absolute `CLOCK_UPTIME_RAW` deadline, the continuous
20 GiB host-reserve watchdog (through plain-Cargo staging and every fixture
child), the monitored 512 MiB scratch budget and cleanup. This script reads
`OXIDE_TASK_SCRATCH_ROOT`, `OXIDE_PROOF_EVIDENCE` and `OXIDE_PROOF_DEADLINE_NS`
and refuses without them. It creates no scratch, starts no second clock, takes
no reserve samples or owned-byte readings of its own, starts no process
session and sends no group signal: children run in the supervisor's
cooperative session, and a child timeout kills only that direct child PID and
reports closure unknown. It resolves `cargo`, `rustc` and `git` once from the
host PATH and gives children those directories plus the system base, because
its own child environment is otherwise scrubbed. Closure
is the supervisor's post-leader census, which is the only evidence any of this
script's children leave none of their own behind. Cargo may start its own
process groups; neither this script nor the supervisor signals them, and this
script makes no separate-group closure claim. A process that leaves the
session (`setsid`) is outside that census.

Fit: seed 256 MiB, staging 128 MiB, fixtures 32 MiB, evidence 32 MiB, margin
64 MiB of the one 512 MiB allocation; 45 s seed + 170 s staging ceiling + 55 s
other work + 30 s cleanup inside 300 s. These are allocations and observations,
not a measured ceiling for other shapes.

Cases, in order:
1. The shared bounded seeder's synthetic suite (`scripts/offline_seed.py`,
   oxide#131): registry+Git positive, a copied-but-rejected candidate, a
   measured and unattempted oversized candidate, symlinked host config and
   hooks skipped at a candidate's root, a wrong commit, a missing candidate, a
   malformed lock, symlinked inputs and the byte/candidate/entry cap
   negatives, all on tiny fixtures in this proof's own scratch.
2. Staging: this HEAD's debug `oxide`, built offline from a private CARGO_HOME
   seeded from HEAD's `Cargo.lock` by that same seeder (registry and the one
   pinned Longhorn Git source), with an explicit empty `RUSTC_WRAPPER`. The
   private Longhorn checkout is then required at the locked commit.
3. On tiny std-only fixtures with a private HOME, CARGO_HOME, scratch
   directory pool and targets: a fixture Cargo config sets
   `build.rustc-wrapper` to a recording fake wrapper. Plain `cargo check`
   invokes it (control); leased `check`, `build`, `test` and an external
   subcommand's `$CARGO build` never do, nor do environment-only,
   `CARGO_BUILD_RUSTC_WRAPPER` and `--config` wrappers. A recording
   `RUSTC_WORKSPACE_WRAPPER` still runs, `oxide clippy` runs the installed
   clippy-driver, `OXIDE_ALLOW_RUSTC_WRAPPER=1` restores the wrapper with its
   warning, other values are ignored, and summary and `status` lines are
   checked, including a Cargo home holding both `config` and `config.toml`
   (Cargo reads only `config`).

No real sccache, image or download. Every proof child's stdout and stderr are
kept in the evidence directory. Not a benchmark and not full QA.
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import traceback
from pathlib import Path

sys.dont_write_bytecode = True

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import offline_seed  # scripts/offline_seed.py, oxide#131; its hash is in the receipt

PROOF_SECONDS = 300
SEED_BYTES = 256 * 1024 * 1024
STAGING_BYTES = 128 * 1024 * 1024
FIXTURE_BYTES = 32 * 1024 * 1024
EVIDENCE_BYTES = 32 * 1024 * 1024
MARGIN_BYTES = 64 * 1024 * 1024
BUILDER = "1.97.1"
BUILDER_VERSIONS = ("cargo 1.97.1 (c980f4866 2026-06-30)", "rustc 1.97.1 (8bab26f4f 2026-07-14)")
# Shutdown margin kept inside the supervisor's own 20 s stop reserve.
SHUTDOWN_MARGIN_NS = 30 * 1_000_000_000
NS = 1_000_000_000
REPO = HERE.parent
ROOT_VAR = "OXIDE_TASK_SCRATCH_ROOT"
TEMP_VARS = (ROOT_VAR, "TMPDIR", "TMP", "TEMP")
RUNNER_KEEP = ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS")
SCRUB_PREFIXES = ("CARGO", "RUSTC", "RUSTFLAGS", "RUSTDOC", "SCCACHE", "OXIDE_")
PATH_BASE = "/usr/bin:/bin:/usr/sbin:/sbin"

RECORDER = """#!/bin/sh
# Records one line per rustc invocation, then runs it.
printf '%s\\n' "$*" >> "LOG"
exec "$@"
"""

EXTERNAL = """#!/bin/sh
# An external subcommand: report the inherited wrapper, then build via $CARGO.
printf 'child RUSTC_WRAPPER=[%s]\\n' "${RUSTC_WRAPPER-unset}"
exec "$CARGO" build
"""

FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/main.rs": (
        "fn answer() -> i32 {\n    return 42;\n}\n\n"
        "fn main() {\n    println!(\"{}\", answer());\n}\n\n"
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn answers() {\n"
        "        assert_eq!(super::answer(), 42);\n    }\n}\n"
    ),
}


def fail(message: str) -> None:
    raise RuntimeError(message)


def now_ns() -> int:
    return time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)


def owned_bytes(root: Path) -> int:
    """Allocated bytes under root. A walk error raises: no partial total."""
    total = os.lstat(root).st_blocks * 512
    for dirpath, dirnames, filenames in os.walk(root, onerror=_raise):
        for name in dirnames + filenames:
            total += os.lstat(os.path.join(dirpath, name)).st_blocks * 512
    return total


def _raise(error: OSError) -> None:
    raise error


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Proof:
    """This proof's own state: the supervisor's root, evidence and deadline."""

    def __init__(self) -> None:
        if sys.platform != "darwin":
            fail("the wrapper-bypass proof requires macOS")
        missing = [name for name in (ROOT_VAR,) + RUNNER_KEEP if not os.environ.get(name)]
        if missing:
            fail(f"run under the qualified shared supervisor; missing {', '.join(missing)}")
        self.root = Path(os.environ[ROOT_VAR])
        if os.path.realpath(self.root) != str(self.root):
            fail(f"{ROOT_VAR} must be canonical")
        if not self.root.is_dir():
            fail(f"{ROOT_VAR} is not a directory: {self.root}")
        if any(os.path.realpath(os.environ[name]) != str(self.root) for name in TEMP_VARS[1:]):
            fail("TMPDIR, TMP and TEMP must all be the supervisor's scratch root")
        self.evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
        if not self.evidence.is_dir():
            fail("the supervisor did not provide OXIDE_PROOF_EVIDENCE")
        self.deadline_ns = int(os.environ["OXIDE_PROOF_DEADLINE_NS"]) - SHUTDOWN_MARGIN_NS
        self.started = now_ns()
        self.phases: list[dict] = []
        self.closure_known = True
        # Resolve the tools from the host PATH before any child environment
        # replaces it: this script's children run with a scrubbed PATH.
        self.tools = {}
        for tool in ("cargo", "rustc", "git"):
            found = shutil.which(tool)
            if found is None:
                fail(f"{tool} is not on the host PATH")
            self.tools[tool] = found
        self.tool_dirs = os.pathsep.join(
            sorted({os.path.dirname(path) for path in self.tools.values()})
        )

    def remaining(self) -> float:
        left = self.deadline_ns - now_ns()
        if left <= 0:
            fail("the supervisor's absolute work cutoff was reached")
        return left / NS

    def elapsed(self) -> float:
        return (now_ns() - self.started) / NS

    def env(self, extra: dict | None = None) -> dict:
        """Scrubbed host environment with the supervisor's temp variables."""
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(SCRUB_PREFIXES) or key in RUNNER_KEEP
        }
        for name in TEMP_VARS:
            env[name] = str(self.root)
        env["RUSTUP_AUTO_INSTALL"] = "0"
        env["PATH"] = self.tool_dirs + os.pathsep + PATH_BASE
        if extra:
            env.update(extra)
        return env

    def mark_phase(self, label: str) -> None:
        """Record when a phase finished. No sampling of any kind happens here:
        the supervisor owns continuous reserve sampling, monitored owned bytes,
        the deadline and cleanup, and reports them itself."""
        entry = {"phase": label, "elapsed_s": round(self.elapsed(), 1)}
        self.phases.append(entry)
        (self.evidence / "phases.json").write_text(json.dumps(self.phases, indent=2))
        print(f"wrapper-bypass phase {label}: {entry['elapsed_s']} s elapsed")

    def run(
        self,
        argv: list,
        *,
        cwd: Path | str | None = None,
        env: dict | None = None,
        label: str,
        limit: float = 120,
    ) -> tuple[int, str, str]:
        """One child in the supervisor's session, logs in the evidence directory.

        No session and no timeout of its own beyond the deadline: a deadline
        overrun kills only this direct child PID, records closure unknown and
        fails. A Cargo group is never signalled from here.
        """
        out_path = self.evidence / f"{label}.stdout"
        err_path = self.evidence / f"{label}.stderr"
        self.remaining()
        with out_path.open("wb") as out, err_path.open("wb") as err:
            child = subprocess.Popen(argv, env=env, cwd=cwd, stdout=out, stderr=err)
            try:
                code = child.wait(timeout=min(limit, self.remaining()))
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
                self.closure_known = False
                fail(f"{label}: the supervisor's work cutoff was reached; child closure is unknown")
        return (
            code,
            out_path.read_text(errors="replace"),
            err_path.read_text(errors="replace"),
        )


def write(path: Path, text: str, mode: int = 0o644) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.chmod(path, mode)


def lines(path: Path) -> list:
    if not path.exists():
        return []
    with open(path, encoding="utf-8") as handle:
        return handle.read().splitlines()


def git(proof: Proof, *args: str) -> str:
    """Read-only Git over this checkout, in the supervisor's session."""
    code, out, err = proof.run(
        [proof.tools["git"], "-C", str(REPO), *args], label=f"git-{args[0]}", limit=30
    )
    if code:
        fail(f"git {args[0]} failed: {err[-800:]}")
    return out.strip()


def toolchain_channel() -> str:
    text = (REPO / "rust-toolchain.toml").read_text()
    marker = 'channel = "'
    start = text.index(marker) + len(marker)
    return text[start : text.index('"', start)]


def seed_and_stage(proof: Proof) -> tuple[Path, dict]:
    """Seed HEAD's lock into a private Cargo home, then build this HEAD's oxide."""
    seed_root = proof.root / "stage"
    seed_root.mkdir(parents=True, exist_ok=True)
    cargo_home = seed_root / "cargo-home"
    cargo_home.mkdir()
    code, lock_text, lock_err = proof.run(
        [proof.tools["git"], "-C", str(REPO), "show", "HEAD:Cargo.lock"],
        label="git-show-lock",
        limit=30,
    )
    if code:
        fail(f"cannot read HEAD's Cargo.lock: {lock_err[-800:]}")
    lock_bytes = lock_text.encode()
    host_cargo_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    try:
        seed = offline_seed.seed_cargo_home(
            host_cargo_home, cargo_home, lock_bytes, deadline_ns=proof.deadline_ns
        )
    except offline_seed.SeedRefusal as error:
        fail(f"bounded offline seed refused: {error}")
    (proof.evidence / "seed.json").write_text(json.dumps(seed, indent=2))
    print(
        f"wrapper-bypass seed: {seed['packages']} registry packages, "
        f"{len(seed['git']['candidates'])} Git candidates, {seed['aggregate_bytes']} bytes, "
        f"{seed['entries']} entries, lock sha256 {seed['lock_sha256'][:12]}"
    )
    env = proof.env(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTC_WRAPPER": "",
            "CARGO_HOME": str(cargo_home),
            "CARGO_TARGET_DIR": str(seed_root / "target"),
            "CARGO_BUILD_BUILD_DIR": str(seed_root / "build"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_PROFILE_DEV_DEBUG": "0",
        }
    )
    versions = tuple(
        proof.run(
            [proof.tools[tool], "-V"],
            cwd=REPO,
            env=env,
            label=f"stage-{tool}-version",
            limit=30,
        )[1].strip()
        for tool in ("cargo", "rustc")
    )
    print(f"wrapper-bypass setup: {versions}")
    if versions != BUILDER_VERSIONS:
        fail(f"builder is not {BUILDER}: {versions}")
    code, _, stderr = proof.run(
        [proof.tools["cargo"], "build", "--locked", "--offline", "--bin", "oxide"],
        cwd=REPO,
        env=env,
        label="stage-build",
        limit=180,
    )
    if code:
        fail(f"staging build failed: {stderr[-2000:]}")
    git_source = seed["git"]
    if git_source.get("candidates"):
        try:
            chosen = offline_seed.verify_private_checkout(
                cargo_home,
                git_source["rev"],
                [candidate["database"] for candidate in git_source["candidates"]],
                deadline_ns=proof.deadline_ns,
            )
        except offline_seed.SeedRefusal as error:
            fail(f"private Longhorn checkout is not the locked commit: {error}")
        git_source["chosen_ident"] = chosen
        (proof.evidence / "seed.json").write_text(json.dumps(seed, indent=2))
        print(f"wrapper-bypass private checkout: {chosen} at {git_source['rev'][:12]}")
    oxide = (seed_root / "target" / "debug" / "oxide").resolve()
    if not oxide.is_file() or not os.access(oxide, os.X_OK):
        fail("staged oxide is missing")
    if not str(oxide).startswith(str(proof.root)):
        fail("the staged artifact is outside the supervisor's scratch")
    provenance = {
        "commit": git(proof, "rev-parse", "HEAD"),
        "tree": git(proof, "rev-parse", "HEAD^{tree}"),
        "toolchain": toolchain_channel(),
        "lock_sha256": seed["lock_sha256"],
        "artifact_sha256": sha256_file(oxide),
        "seed_aggregate_bytes": seed["aggregate_bytes"],
        "seed_entries": seed["entries"],
    }
    return oxide, provenance


def seed_cases(proof: Proof) -> list:
    """The shared seeder's synthetic cases, in this proof's own scratch."""
    cases_root = proof.root / "seed-cases"
    try:
        records = offline_seed.self_test(cases_root)
    except Exception as error:
        fail(f"the shared seeder's synthetic cases failed: {error}")
    (proof.evidence / "seed-cases.json").write_text(json.dumps(records, indent=2))
    for record in records:
        print(f"wrapper-bypass seed case {record['case']}: {record['detail']}")
    return records


def prove(proof: Proof, oxide: Path, parent: Path) -> None:
    app = parent / "app"
    for relative, text in FIXTURE.items():
        write(app / relative, text)
    bindir = parent / "bin"
    outer_log = parent / "outer.log"
    workspace_log = parent / "workspace.log"
    outer = bindir / "fake-wrapper"
    workspace = bindir / "workspace-wrapper"
    write(outer, RECORDER.replace("LOG", str(outer_log)), 0o755)
    write(workspace, RECORDER.replace("LOG", str(workspace_log)), 0o755)
    write(bindir / "cargo-wrapcheck", EXTERNAL, 0o755)
    home = parent / "home"
    cargo_home = parent / "cargo-home"
    home.mkdir(mode=0o700)
    config = cargo_home / "config.toml"
    write(config, f'[build]\nrustc-wrapper = "{outer}"\n')
    tmp = os.environ["TMPDIR"]
    base = proof.env(
        {
            "HOME": str(home),
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME")
            or os.path.join(os.path.expanduser("~"), ".rustup"),
            "RUSTUP_TOOLCHAIN": BUILDER,
            "CARGO_HOME": str(cargo_home),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "OXIDE_POOL": str(parent / "pool"),
            "OXIDE_POOL_CAP": "64MiB",
            "PATH": str(bindir) + os.pathsep + proof.tool_dirs + os.pathsep + PATH_BASE,
            "OXIDE_TASK_SCRATCH_ROOT": tmp,
        }
    )
    main_rs = app / "src" / "main.rs"
    counter = {"runs": 0}

    def touch() -> None:
        now = time.time() + 2
        os.utime(main_rs, (now, now))

    def label(name: str) -> str:
        counter["runs"] += 1
        return f"case-{counter['runs']:02d}-{name.replace(' ', '-').replace('/', '-')}"

    def leased(args: list, name: str, extra: dict | None = None, wanted_outer: int = 0) -> str:
        name_label = label(name)
        touch()
        before = len(lines(outer_log))
        code, stdout, stderr = proof.run(
            [str(oxide), *args], cwd=app, env={**base, **(extra or {})}, label=f"{name_label}-command"
        )
        if code:
            fail(f"{name}: exit {code}\n{stderr[-3000:]}")
        if "Checking app" not in stderr and "Compiling app" not in stderr:
            fail(f"{name}: the fixture crate was not compiled\n{stderr[-2000:]}")
        calls = len(lines(outer_log)) - before
        summary = [
            line
            for line in stderr.splitlines()
            if line.startswith("oxide: ") and " profile, " in line
        ]
        print(f"wrapper-bypass {name}: exit 0, fake wrapper calls {calls}, summary {summary}")
        if (calls == 0) != (wanted_outer == 0):
            fail(f"{name}: fake wrapper calls {calls}, wanted {'some' if wanted_outer else 'none'}")
        if len(summary) != 1:
            fail(f"{name}: expected one summary line, got {summary}")
        return summary[0] + "\n" + stdout + stderr

    # Control: plain Cargo in the same fixture uses the configured wrapper.
    touch()
    code, _, stderr = proof.run(
        [proof.tools["cargo"], "check"],
        cwd=app,
        env={**base, "CARGO_TARGET_DIR": str(parent / "plain-target")},
        label="plain-cargo-check",
    )
    plain = len(lines(outer_log))
    print(f"wrapper-bypass plain cargo check: exit {code}, fake wrapper calls {plain}")
    if code or plain == 0:
        fail(f"plain cargo did not use the fixture wrapper\n{stderr[-2000:]}")

    named = f"rustc wrapper bypassed: {outer}"
    for args in (["check"], ["build"], ["test"]):
        text = leased(args, f"config wrapper, oxide {args[0]}")
        if named not in text:
            fail(f"oxide {args[0]}: summary does not name the bypassed wrapper")
    text = leased(["wrapcheck"], "external subcommand $CARGO build")
    if "child RUSTC_WRAPPER=[]" not in text or named not in text:
        fail("the external subcommand did not inherit the empty wrapper")
    no_config = parent / "cargo-home-empty"
    no_config.mkdir()
    for name, args, extra in (
        ("RUSTC_WRAPPER only", ["check"], {"RUSTC_WRAPPER": str(outer)}),
        ("CARGO_BUILD_RUSTC_WRAPPER only", ["check"], {"CARGO_BUILD_RUSTC_WRAPPER": str(outer)}),
        ("--config only", ["check", "--config", f"build.rustc-wrapper='{outer}'"], {}),
    ):
        text = leased(args, name, {"CARGO_HOME": str(no_config), **extra})
        if named not in text:
            fail(f"{name}: the bypassed wrapper was not named")
    # Both names in one Cargo home: Cargo reads `config` and ignores the
    # `.toml`, whose empty wrapper must not hide the real one.
    dual = parent / "cargo-home-dual"
    write(dual / "config.toml", '[build]\nrustc-wrapper = ""\n')
    write(dual / "config", f'[build]\nrustc-wrapper = "{outer}"\n')
    text = leased(
        ["check"],
        "dual config files, opt-in",
        {"CARGO_HOME": str(dual), "OXIDE_ALLOW_RUSTC_WRAPPER": "1"},
        wanted_outer=1,
    )
    if f"rustc wrapper active: {outer}, unqualified" not in text:
        fail("dual config files: the opt-in summary does not name the active wrapper")
    text = leased(["check"], "dual config files", {"CARGO_HOME": str(dual)})
    if named not in text:
        fail("dual config files: the bypassed wrapper was not named")
    code, stdout, _ = proof.run(
        [str(oxide), "status"], cwd=app, env={**base, "CARGO_HOME": str(dual)}, label="status-dual"
    )
    wanted_status = (
        f"rustc wrapper: inherited {outer} (from {dual / 'config'}); bypassed"
    )
    if code or wanted_status not in stdout:
        fail(f"dual config files: status is wrong\n{stdout}")
    print("wrapper-bypass dual config files status: names the wrapper from config, not config.toml")
    text = leased(["check"], "no inherited wrapper", {"CARGO_HOME": str(no_config)})
    if "rustc wrapper" in text:
        fail("a summary named a wrapper where none is inherited")

    before = len(lines(workspace_log))
    leased(["check"], "workspace wrapper preserved", {"RUSTC_WORKSPACE_WRAPPER": str(workspace)})
    if len(lines(workspace_log)) == before:
        fail("the recording workspace wrapper did not run")

    text = leased(["clippy"], "oxide clippy")
    if "clippy::needless_return" not in text or named not in text:
        fail(f"oxide clippy did not run clippy-driver's lints\n{text[-2000:]}")
    print("wrapper-bypass oxide clippy: clippy::needless_return reported by clippy-driver")

    text = leased(
        ["check"], "opt-in OXIDE_ALLOW_RUSTC_WRAPPER=1", {"OXIDE_ALLOW_RUSTC_WRAPPER": "1"}, wanted_outer=1
    )
    if f"rustc wrapper active: {outer}, unqualified" not in text:
        fail("the opt-in summary does not warn that the wrapper is active and unqualified")
    text = leased(
        ["check"], "OXIDE_ALLOW_RUSTC_WRAPPER=true ignored", {"OXIDE_ALLOW_RUSTC_WRAPPER": "true"}
    )
    if named not in text or "is not 1, so ignored" not in text:
        fail("a non-1 opt-in value was not reported as ignored")

    code, stdout, _ = proof.run([str(oxide), "status"], cwd=app, env=base, label="status-inherited")
    wanted = f"rustc wrapper: inherited {outer} (from {config}); bypassed for oxide builds"
    print(
        f"wrapper-bypass status: exit {code}, "
        f"{[line for line in stdout.splitlines() if 'rustc wrapper' in line]}"
    )
    if code or wanted not in stdout:
        fail(f"status does not report the inherited wrapper and its state\n{stdout}")
    code, stdout, _ = proof.run(
        [str(oxide), "status"],
        cwd=app,
        env={**base, "OXIDE_ALLOW_RUSTC_WRAPPER": "1"},
        label="status-opt-in",
    )
    if (
        code
        or f"rustc wrapper: inherited {outer} (from {config}); kept for oxide builds by OXIDE_ALLOW_RUSTC_WRAPPER=1, unqualified"
        not in stdout
    ):
        fail(f"opt-in status is wrong\n{stdout}")

    # Plain passthrough is untouched: no lease and no summary.
    code, stdout, stderr = proof.run([str(oxide), "pkgid"], cwd=app, env=base, label="plain-pkgid")
    if code or "oxide:" in stderr or "app" not in stdout:
        fail(f"plain passthrough changed: exit {code} {stderr}")
    print(
        f"wrapper-bypass plain passthrough pkgid: exit 0, no lease; "
        f"total fake wrapper calls {len(lines(outer_log))}"
    )


def main() -> int:
    proof = Proof()
    print(
        "wrapper-bypass allocation: "
        f"seed {SEED_BYTES // (1024 ** 2)} MiB, staging {STAGING_BYTES // (1024 ** 2)} MiB, "
        f"fixtures {FIXTURE_BYTES // (1024 ** 2)} MiB, evidence {EVIDENCE_BYTES // (1024 ** 2)} MiB, "
        f"margin {MARGIN_BYTES // (1024 ** 2)} MiB of the supervisor's 512 MiB"
    )
    print(f"wrapper-bypass scratch: {proof.root}")
    print(f"wrapper-bypass evidence: {proof.evidence}")
    proof.mark_phase("admission")
    helper = {
        name: sha256_file(REPO / "scripts" / name)
        for name in ("bounded-test.py", "offline_seed.py", "test-oxide-wrapper-bypass.py")
    }
    print(f"wrapper-bypass sources: {json.dumps(helper, sort_keys=True)}")

    seed_cases(proof)
    proof.mark_phase("seed cases")
    oxide, provenance = seed_and_stage(proof)
    proof.mark_phase("staging")
    (proof.evidence / "provenance.json").write_text(json.dumps(provenance, indent=2))
    print(f"wrapper-bypass artifact: {oxide}, sha256 {provenance['artifact_sha256']}")
    parent = proof.root / "proof"
    parent.mkdir(mode=0o700)
    prove(proof, oxide, parent)
    proof.mark_phase("fixtures")
    elapsed = proof.elapsed()
    owned = owned_bytes(proof.root)
    receipt = {
        "sources": helper,
        "provenance": provenance,
        "phases": proof.phases,
        "closure": "supervisor census only; this script starts no session and signals no group",
        "elapsed_s": round(elapsed, 1),
        "owned_bytes": owned,
    }
    (proof.evidence / "receipt.json").write_text(json.dumps(receipt, indent=2))
    print(
        f"oxide wrapper-bypass proof held: {elapsed:.0f} s of {PROOF_SECONDS} s, owned "
        f"{owned // (1024 ** 2)} MiB of 512 MiB, closure the supervisor's"
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
