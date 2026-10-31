#!/usr/bin/env python3
"""Focused proof for project-named slots and pool status (oxide#104).

A short fixed script under Effigy heavy admission. It reuses the oxide#105
script's guarded child runner unchanged (`Proof`: deadline from one 300 s
budget checked before each child, the 20 GiB host reserve sampled every
250 ms while a child runs, owned scratch checked between phases against
512 MiB, child sessions killed on breach or deadline, ordinary detach);
it adds no supervision of its own.

Stages the base commit's `oxide` and this HEAD's `oxide` offline with
1.97.1 into owned scratch. Over a private HOME, CARGO_HOME and budget-only
internal pool: the base binary creates a legacy `slots/1`; the candidate
reuses it warm at the same path, then two same-named checkouts (`one/app`,
`two/app`) get new slots under separate `slots/app-<id>/<profile>-<n>`
folders. One 128 MiB scratch APFS image at a unique `/Volumes/<name>` is
registered as the removable pool; new removable slots are labelled too,
including slot 1 beside the internal slot 1. `status` must put the selected
pool first, keep each pool's slots under its own header and name the most
recently used slot's pool and project; after an ordinary detach it falls
back to the internal pool and reports the removable slots as unknown, not
empty. Not a benchmark and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import importlib.util
import json
import os
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("finder_metadata", os.path.join(HERE, "test-oxide-finder-metadata.py"))
runner = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(runner)
expect, fail, summary, MIB = runner.expect, runner.fail, runner.summary, runner.MIB
# Work stops this long before the shared runner's absolute deadline, inside
# its 20 s stop reserve plus margin for detach.
SHUTDOWN_MARGIN_S = 30


def work_left() -> float:
    """Seconds of work left before the shared runner's exported absolute
    CLOCK_UPTIME_RAW deadline (OXIDE_PROOF_DEADLINE_NS) less the shutdown
    margin; never a fresh process-relative budget."""
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the runner's shutdown margin")
    return left


class Proof(runner.Proof):
    """The #105 helper's phase checks and detach, with children kept in the
    shared runner's cooperative session: no new session per child and no
    session kill. On a timeout only the pinned child itself is killed and,
    unless reaped, stays in `unclosed`, which blocks detach and keeps
    evidence; the runner decides closure and cleanup."""

    def run(self, command: list, *, cwd: str | None = None, env: dict | None = None, timeout: float = 120) -> subprocess.CompletedProcess:
        limit = min(timeout, work_left())
        child = subprocess.Popen(command, cwd=cwd, env=env or self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.unclosed.add(child.pid)
        try:
            out, err = child.communicate(timeout=limit)
        except BaseException:
            child.kill()
            try:
                child.communicate(timeout=5)
                self.unclosed.discard(child.pid)
            except BaseException as error:
                print(f"child {child.pid} not reaped: {error!r}", file=sys.stderr)
            raise
        self.unclosed.discard(child.pid)
        return subprocess.CompletedProcess(command, child.returncode, out, err)
BASE = "850260c"
FIXTURE = runner.FIXTURE


# A string only the candidate's status prints; checked in the staged bytes
# before any pool or image fixture exists.
CANDIDATE_MARKER = b"selected for the next build: internal pool (no removable volume registered)"


def stage_at(proof: Proof, source: str, label: str, jobs: int | None = None,
             retain_stage_root: bool = False) -> str:
    """Build `oxide` from `source` into its own fresh target and build
    directories under the owned scratch, never shared with another source
    tree: one shared target let Cargo reuse the base binary for the
    candidate in the first session (identical SHA-256 46db66ce...).

    `jobs` is an OPTIONAL explicit Cargo job bound applied after the
    inherited-environment sanitization (which removes `CARGO_BUILD_*`); the
    default `None` keeps every existing selector's behaviour unchanged.

    `retain_stage_root` is an OPT-IN retention path for a caller whose staging
    child may leave a descendant writing: the direct Cargo leader's exit does
    not prove its group is closed, so such a caller keeps `stage_root` inside
    the owned root for the qualified guard's post-leader census instead of
    deleting it here. The default `False` preserves every other selector's
    behaviour exactly."""
    stage_root = os.path.join(proof.root, f"stage-{label}")
    os.makedirs(stage_root)
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")) or name in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"):
            env.pop(name)
    env.update(
        {
            "RUSTUP_TOOLCHAIN": runner.BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage_root, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage_root, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_INCREMENTAL": "0",
        }
    )
    command = [
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--bin",
        "oxide",
        "--message-format=json-render-diagnostics",
    ]
    if jobs is not None:
        expect(isinstance(jobs, int) and jobs > 0, f"{label}: unusable staging job bound {jobs!r}")
        # Explicit after sanitization: an inherited CARGO_BUILD_JOBS was
        # removed above, so the bound cannot come from the environment.
        env["CARGO_BUILD_JOBS"] = str(jobs)
        command.extend(["--jobs", str(jobs)])
    result = proof.run(command, cwd=source, env=env, timeout=200)
    print(result.stderr.decode(errors="replace")[-4000:])
    expect(result.returncode == 0, f"{label} staging exit {result.returncode}")
    built = None
    for line in result.stdout.decode(errors="replace").splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("target", {}).get("name") == "oxide" and message.get("executable"):
            built = message["executable"]
    expect(built is not None and os.path.realpath(built).startswith(stage_root + "/"), f"{label} executable {built} is not in {stage_root}")
    copy = os.path.join(proof.root, "bin", label)
    os.makedirs(os.path.dirname(copy), exist_ok=True)
    shutil.copy2(built, copy)
    if retain_stage_root:
        # The staged tree stays for the guard's post-leader census and its
        # retention policy; removing it here would delete that evidence first.
        print(f"{label}: staging tree {stage_root} retained for the guard's census")
    else:
        shutil.rmtree(stage_root)
    return copy


def sha256(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def source_facts(source: str) -> str:
    """Manifest and lock digests of a staged source tree."""
    return ", ".join(f"{name} {sha256(os.path.join(source, name))[:16]}" for name in ("Cargo.toml", "Cargo.lock"))


# Status's exact line for no registered removable pool: an absence, not a
# pool section. It still ends the block before it.
NO_REMOVABLE = "removable pool: none registered"


def sections(stdout: str) -> list:
    """(header, slot lines) per pool block, in display order."""
    blocks = []
    for line in stdout.splitlines():
        if line.startswith(("internal pool:", "removable pool:")):
            blocks.append((line, []))
        elif re.match(r"  slot \d+ ", line) and blocks:
            blocks[-1][1].append(line)
        elif not line.startswith(" ") and blocks and not line.startswith(("most recent use", "selected for")):
            blocks.append((line, []))
    return [block for block in blocks if block[0].startswith(("internal pool:", "removable pool:")) and block[0] != NO_REMOVABLE]


def slot_place(lines: list, slot: str) -> str:
    found = [line for line in lines if line.startswith(f"  slot {slot} ")]
    expect(len(found) == 1, f"slot {slot} listed {len(found)} times: {lines}")
    return found[0]


def run(proof: Proof, repo: str) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    name = "OxPl" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    print(f"pool-layout proof {os.getpid()} source {source} base {BASE} volume {name}")
    expect(not os.path.lexists(mount), f"{mount} already exists; never touched")
    proof.phase("start")
    root = proof.root
    base_src = os.path.join(root, "base-src")
    os.makedirs(base_src)
    archive = proof.run(["/bin/sh", "-c", f'git -C "$0" archive {BASE} | tar -x -C "$1"', repo, base_src], timeout=60)
    expect(archive.returncode == 0, f"base archive failed: {archive.stderr!r}")
    base_tree = proof.run(["git", "rev-parse", f"{BASE}^{{commit}}", f"{BASE}^{{tree}}"], cwd=repo, timeout=30).stdout.decode().split()
    head_tree = proof.run(["git", "rev-parse", "HEAD^{tree}"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    print(f"base commit {base_tree[0]} tree {base_tree[1]} ({source_facts(base_src)})")
    print(f"candidate commit {source} tree {head_tree} ({source_facts(repo)})")
    base = stage_at(proof, base_src, "oxide-base")
    candidate = stage_at(proof, repo, "oxide-candidate")
    shutil.rmtree(base_src)
    base_hash, candidate_hash = sha256(base), sha256(candidate)
    print(f"staged binaries: base sha256 {base_hash}, candidate sha256 {candidate_hash}")
    expect(base_hash != candidate_hash, "base and candidate binaries are identical; no pool or image is touched")
    with open(candidate, "rb") as handle:
        expect(CANDIDATE_MARKER in handle.read(), "candidate binary lacks the #104 status marker")
    with open(base, "rb") as handle:
        expect(CANDIDATE_MARKER not in handle.read(), "base binary carries the #104 status marker")
    proof.phase("after staging")

    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue  # owner-supplied by the qualified runner, not ambient config
        if key.startswith(("SCCACHE_", "OXIDE_", "XDG_")) or key in ("CARGO", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "RUSTC", "RUSTUP_TOOLCHAIN"):
            env.pop(key)
    tmp = os.environ["TMPDIR"]
    env.update(
        {
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": "64MiB",
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "RUSTUP_AUTO_INSTALL": "0",
            "OXIDE_TASK_SCRATCH_ROOT": tmp,
            "TMPDIR": tmp,
            "TMP": tmp,
            "TEMP": tmp,
        }
    )
    proof.env = env
    apps = {}
    for parent in ("legacy", "one", "two"):
        app = os.path.join(root, parent, "app")
        for relative, text in FIXTURE.items():
            path = os.path.join(app, relative)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(text)
        apps[parent] = os.path.realpath(app)
    slots = os.path.join(pool, "payload", "slots")

    # Legacy: the base binary's slot 1 at slots/1.
    proof.binary = base
    code, _, stderr = proof.oxide(["check"], "base cold (legacy layout)", apps["legacy"])
    expect(code == 0 and "slot 1 cold" in summary(stderr), "base build failed")
    expect(sorted(os.listdir(slots)) == ["1"], f"base layout is not slots/1: {os.listdir(slots)}")
    legacy_inode = os.stat(os.path.join(slots, "1")).st_ino

    proof.binary = candidate
    code, stdout, _ = proof.oxide(["status"], "status, legacy slot only", apps["legacy"])
    expect(code == 0, "status failed")
    expect(stdout.splitlines().count("selected for the next build: internal pool (no removable volume registered)") == 1, "selection line missing")
    blocks = sections(stdout)
    expect([block[0].split(":")[0] for block in blocks] == ["internal pool"], f"blocks {blocks}")
    expect(stdout.splitlines().count(NO_REMOVABLE) == 1, "the no-registration line is not shown exactly once")
    line = slot_place(blocks[0][1], "1")
    expect(" project app " in line and line.endswith("slots/1 (legacy path)"), f"legacy slot line: {line}")
    code, _, stderr = proof.oxide(["check"], "candidate warm on legacy slot", apps["legacy"])
    expect(code == 0 and "slot 1 warm" in summary(stderr), "legacy slot not reused warm")
    expect(os.stat(os.path.join(slots, "1")).st_ino == legacy_inode and sorted(os.listdir(slots)) == ["1"], "legacy slot moved or a folder appeared")

    # Same-named checkouts: separate labelled folders.
    folders = {}
    for parent, slot in (("one", "2"), ("two", "3")):
        code, _, stderr = proof.oxide(["check"], f"internal cold {parent}/app", apps[parent])
        expect(code == 0 and f"slot {slot} cold" in summary(stderr), f"{parent} cold failed")
        made = [entry for entry in os.listdir(slots) if entry != "1" and entry not in folders.values()]
        expect(len(made) == 1 and re.fullmatch(r"app-[0-9a-f]{8}", made[0]), f"new folder {made}")
        folders[parent] = made[0]
        expect(os.path.isdir(os.path.join(slots, made[0], f"quick-{slot}", "build")), f"slot {slot} not at {made[0]}/quick-{slot}")
    expect(folders["one"] != folders["two"], "same-named repositories share a folder")
    code, _, stderr = proof.oxide(["check", "--profile", "debug"], "internal debug one/app", apps["one"])
    expect(code == 0 and "slot 4 cold" in summary(stderr), "debug slot failed")
    expect(os.path.isdir(os.path.join(slots, folders["one"], "debug-4", "build")), "debug slot did not join the project folder")
    proof.phase("after internal pool")

    # Removable: one owned scratch APFS image at a unique /Volumes name.
    image = os.path.join(root, "volume.sparseimage")
    created = proof.run(["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name, "-type", "SPARSE", image], timeout=60)
    expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    attached = proof.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist", image], timeout=60)
    expect(attached.returncode == 0, f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}")
    import plistlib

    mounts = [entity["mount-point"] for entity in plistlib.loads(attached.stdout)["system-entities"] if entity.get("mount-point")]
    expect(len(mounts) == 1, f"expected one mount, got {mounts}")
    proof.mount = mounts[0]
    expect(proof.mount == mount, f"scratch volume mounted at {proof.mount}, not {mount}")
    code, stdout, _ = proof.oxide(["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "register", apps["two"])
    expect(code == 0 and "registered removable pool" in stdout, "registration failed")
    volume_slots = os.path.join(mount, "oxide-pool", "slots")
    code, _, stderr = proof.oxide(["check"], "removable cold two/app", apps["two"])
    line = summary(stderr)
    expect(code == 0 and "apfs-volume-v1" in line and "slot 1 cold" in line, f"removable not selected: {line}")
    code, _, stderr = proof.oxide(["check"], "removable cold one/app", apps["one"])
    expect(code == 0 and "slot 2 cold" in summary(stderr), "second removable slot failed")
    expect(os.path.isdir(os.path.join(volume_slots, folders["two"], "quick-1", "build")), "removable slot 1 not labelled")
    expect(os.path.isdir(os.path.join(volume_slots, folders["one"], "quick-2", "build")), "removable slot 2 not labelled")

    code, stdout, _ = proof.oxide(["status"], "status, removable selected", apps["one"])
    lines = stdout.splitlines()
    expect(code == 0 and "selected for the next build: removable pool" in lines, "removable not selected in status")
    recent = [line for line in lines if line.startswith("most recent use: ")]
    expect(len(recent) == 1 and recent[0].startswith(f"most recent use: removable pool slot 2, project {folders['one']} ({apps['one']})"), f"recent use: {recent}")
    blocks = sections(stdout)
    expect([block[0].split(":")[0] for block in blocks] == ["removable pool", "internal pool"], f"order {[b[0] for b in blocks]}")
    expect(blocks[0][0].endswith("(selected for the next build)") and "fallback" in blocks[1][0], f"headers {blocks[0][0]} / {blocks[1][0]}")
    expect(len(blocks[0][1]) == 2 and len(blocks[1][1]) == 4, f"slot grouping {blocks}")
    expect(slot_place(blocks[0][1], "1").endswith(f"slots/{folders['two']}/quick-1"), "removable slot 1 line")
    expect(slot_place(blocks[1][1], "1").endswith("slots/1 (legacy path)"), "internal slot 1 line")
    expect(slot_place(blocks[1][1], "4").endswith(f"slots/{folders['one']}/debug-4"), "internal slot 4 line")
    proof.phase("after removable pool")

    # Ordinary detach: internal selected, removable slots unknown, not empty.
    proof.detach()
    proof.mount = None
    code, stdout, _ = proof.oxide(["status"], "status, removable detached", apps["one"])
    lines = stdout.splitlines()
    expect(code == 0 and any(line.startswith("selected for the next build: internal pool (removable volume not used:") for line in lines), "fallback not selected")
    blocks = sections(stdout)
    expect([block[0].split(":")[0] for block in blocks] == ["internal pool", "removable pool"], f"order {[b[0] for b in blocks]}")
    expect("  slots: not read while the volume is not usable (unknown, not empty)" in lines, "unavailable slots shown as known")
    recent = [line for line in lines if line.startswith("most recent use: ")]
    expect(len(recent) == 1 and recent[0].startswith("most recent use: removable pool slot unknown, project app"), f"recent use: {recent}")
    code, _, stderr = proof.oxide(["check"], "internal warm two/app after detach", apps["two"])
    line = summary(stderr)
    expect(code == 0 and "slot 3 warm" in line and "directory-budget-v1" in line, f"fallback not warm: {line}")
    code, stdout, _ = proof.oxide(["status"], "status after internal use", apps["two"])
    recent = [line for line in stdout.splitlines() if line.startswith("most recent use: ")]
    expect(len(recent) == 1 and recent[0].startswith(f"most recent use: internal pool slot 3, project {folders['two']}"), f"recent use: {recent}")
    expect(os.stat(os.path.join(slots, "1")).st_ino == legacy_inode, "legacy slot moved")
    proof.phase("after detach")


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, text: str) -> None:
    """Evidence goes to the runner's OXIDE_PROOF_EVIDENCE, which it keeps."""
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(text)


def main() -> int:
    if sys.platform != "darwin":
        fail("the pool layout proof requires macOS")
    # All writes go under the qualified runner's monitored scratch, which it
    # binds to all four temp variables; it owns closure and cleanup.
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ, "not under the qualified runner")
    root = tempfile.mkdtemp(prefix="pool-layout.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    try:
        run(proof, repo)
        proof.detach()
    except BaseException as failure:
        try:
            proof.detach()
        except BaseException as error:
            print(f"cleanup after failure: {error!r}", file=sys.stderr)
        record("result.txt", f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\nmount: {proof.mount}\nroot: {root}\n")
        print(f"pool-layout proof failed after {time.monotonic() - runner.STARTED:.0f} s; scratch {root} left to the runner", file=sys.stderr)
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    proof.phase("end")
    elapsed = time.monotonic() - runner.STARTED
    summary_line = (
        f"pool-layout proof held: {elapsed:.0f} s script time, peak owned {proof.peak // MIB} MiB at phase checks, "
        f"host free minimum {proof.low // MIB} MiB; scratch closure and cleanup are the shared runner's"
    )
    record("result.txt", f"passed\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
