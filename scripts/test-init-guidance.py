#!/usr/bin/env python3
"""Focused init-guidance placement proof (oxide#127).

A short fixed script under the qualified shared proof supervisor
(`scripts/bounded-test.py`, SHA-256
d89115f1ca03587e1d9134efa153cc9c94860d2904f52e2a11bc8c8fbe21f25e), which
owns the 300 s deadline, 512 MiB monitored scratch, process closure and
cleanup. It reuses the reviewed #104 staging helper and guarded child runner.

It stages this HEAD's `oxide` offline with 1.97.1 and runs `oxide init`,
`oxide clean --cargo-cache` (adoption, refused before any effect) and
readiness on tiny owned Git fixtures, with a private HOME, CARGO_HOME and an
absent pool path. Covered: nested Git placement, two workspaces, idempotent
repeats, a root workspace, outside Git and a Cargo-created repository,
legacy nested retained, legacy root upgrade, listed and unlisted adoption,
malformed list and markers, unsafe paths, symlink and oversize refusals, a
corrupt Git config refusing, and two concurrent inits keeping both paths.
Not a benchmark, not a real-consumer run and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import importlib.util
import os
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py"))
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
Proof = layout.Proof
expect, fail = layout.expect, layout.fail

BUILDER = "1.97.1"
PROOF_SECONDS = 300
STARTED = time.monotonic()
AGENTS = "AGENTS.md"
BEGIN = "<!-- oxide:begin -->"
END = "<!-- oxide:end -->"
# The outside-Git block, byte for byte as oxide has always written it.
OUTSIDE = """<!-- oxide:begin -->
## Rust commands: use `oxide`

- Write `oxide` wherever you would write `cargo`: `oxide check`, `oxide test`, `oxide run -- <args>`, `oxide add <crate>`.
- Profiles: `quick` (default), `--profile debug` for a debugger, `--release`. Oxide places build output, so it refuses `--target-dir`.
- A tool that runs Cargo itself (Tauri CLI, npm or bun scripts): `oxide exec -- <command>`.
- A `waiting for …` line means another build holds a slot; let it wait. `oxide status` shows the pool, slots and waits.
- Use `oxide clean`, not `cargo clean`.

Managed by `oxide init`; edits between these markers are replaced.
<!-- oxide:end -->
"""
MANIFEST = '[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n'
CANDIDATE_MARKER = b"covered paths: "
results = []


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def read(path: str) -> str:
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def digest(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def manifest(directory: str, name: str) -> None:
    # A target is required: without one, `cargo locate-project` refuses the
    # manifest before oxide reaches the guidance step.
    write(os.path.join(directory, "Cargo.toml"), MANIFEST.format(name=name))
    write(os.path.join(directory, "src", "lib.rs"), "pub fn value() -> u32 {\n    7\n}\n")


def git(directory: str) -> None:
    os.makedirs(directory, exist_ok=True)
    env = {"PATH": os.environ["PATH"], "HOME": ENV["HOME"], "GIT_CONFIG_NOSYSTEM": "1"}
    result = subprocess.run(["git", "init", "-q"], cwd=directory, env=env, capture_output=True, timeout=30)
    expect(result.returncode == 0, f"git init failed in {directory}")


def environment(root: str) -> dict:
    real_home = os.path.expanduser("~")
    scratch = os.environ["OXIDE_TASK_SCRATCH_ROOT"]
    env = {
        "PATH": os.environ["PATH"],
        "HOME": os.path.join(root, "home"),
        "CARGO_HOME": os.path.join(root, "cargo-home"),
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(real_home, ".rustup")),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "OXIDE_POOL": os.path.join(root, "pool-absent"),
        "OXIDE_TASK_SCRATCH_ROOT": scratch,
        "TMPDIR": scratch,
        "TMP": scratch,
        "TEMP": scratch,
        "LANG": "C",
    }
    for name in ("HOME", "CARGO_HOME"):
        os.makedirs(env[name], exist_ok=True)
    return env


ENV: dict = {}


def oxide(proof: Proof, args: list, cwd: str, timeout: float = 60) -> tuple:
    result = proof.run([proof.binary, *args], cwd=cwd, env=ENV, timeout=timeout)
    out = result.stdout.decode(errors="replace")
    err = result.stderr.decode(errors="replace")
    return result.returncode, out + err


def case(name: str, condition: bool, detail: str = "") -> None:
    results.append((name, condition))
    print(("PASS " if condition else "FAIL ") + name + (f": {detail}" if detail and not condition else ""), flush=True)
    expect(condition, f"{name} {detail}")


def root_list(root: str) -> str:
    text = read(os.path.join(root, AGENTS))
    lines = [line for line in text.splitlines() if line.startswith("<!-- oxide:workspaces:")]
    return lines[0] if len(lines) == 1 else f"{len(lines)} lists"


def run(proof: Proof, root: str, staged: str) -> None:
    proof.binary = staged
    work = os.path.join(root, "work")
    os.makedirs(work)

    # Nested Git workspace: the root gets the one block; the member gets none.
    nested = os.path.join(work, "nested")
    git(nested)
    manifest(os.path.join(nested, "crates", "app"), "app")
    code, text = oxide(proof, ["init"], os.path.join(nested, "crates", "app"))
    case("nested placement exit", code == 0, text)
    root_agents = os.path.join(nested, AGENTS)
    case("nested block at Git root", os.path.isfile(root_agents) and not os.path.exists(os.path.join(nested, "crates", "app", AGENTS)))
    case("nested list names member", "<!-- oxide:workspaces: crates/app -->" in read(root_agents), root_list(nested))
    case("nested readiness names root", "guidance root: " in text and "covered paths: crates/app" in text, text)
    case("nested local-output ignore kept in workspace", "/.oxide-outputs/" in read(os.path.join(nested, "crates", "app", ".gitignore")))
    first = digest(root_agents)
    code, text = oxide(proof, ["init"], os.path.join(nested, "crates", "app"))
    case("nested repeat is byte-identical", code == 0 and digest(root_agents) == first and "already current" in text, text)
    code, text = oxide(proof, ["clean", "--cargo-cache"], os.path.join(nested, "crates", "app"))
    case("listed workspace adopted", "is missing" in text and "is not adopted" not in text, text)

    # Second workspace joins the same list; a third stays unlisted.
    manifest(os.path.join(nested, "tools", "cli"), "cli")
    code, text = oxide(proof, ["init"], os.path.join(nested, "tools", "cli"))
    case("second workspace joins list", code == 0 and "<!-- oxide:workspaces: crates/app tools/cli -->" in read(root_agents), root_list(nested))
    manifest(os.path.join(nested, "extra", "pkg"), "pkg")
    _, text = oxide(proof, ["clean", "--cargo-cache"], os.path.join(nested, "extra", "pkg"))
    case("unlisted workspace not adopted", "is not adopted" in text, text)

    # Root workspace: the list carries `.`.
    rootws = os.path.join(work, "rootws")
    git(rootws)
    manifest(rootws, "rootws")
    code, text = oxide(proof, ["init"], rootws)
    case("root workspace lists dot", code == 0 and "<!-- oxide:workspaces: . -->" in read(os.path.join(rootws, AGENTS)), text)

    # Outside Git: today's plain block in the workspace's own file.
    plain = os.path.join(work, "plain")
    manifest(plain, "plain")
    code, text = oxide(proof, ["init"], plain)
    case("outside Git keeps the plain block", code == 0 and read(os.path.join(plain, AGENTS)) == OUTSIDE, text)
    case("outside Git readiness", "no Git work tree" in text, text)
    _, text = oxide(proof, ["clean", "--cargo-cache"], plain)
    case("outside Git block adopts", "is missing" in text and "is not adopted" not in text, text)

    # A repository Cargo creates is Git, so its root holds the block.
    fresh_parent = os.path.join(work, "fresh-parent")
    os.makedirs(fresh_parent)
    code, text = oxide(proof, ["init", "--lib", "fresh"], fresh_parent, timeout=120)
    fresh = os.path.join(fresh_parent, "fresh")
    case("cargo-created repository counts as Git", code == 0 and os.path.isdir(os.path.join(fresh, ".git")) and "<!-- oxide:workspaces: . -->" in read(os.path.join(fresh, AGENTS)), text)

    # Legacy nested block: not adoption, retained and reported, root adopts.
    legacy = os.path.join(work, "legacy")
    git(legacy)
    manifest(os.path.join(legacy, "crates", "app"), "legacy-app")
    write(os.path.join(legacy, "crates", "app", AGENTS), OUTSIDE)
    member_before = digest(os.path.join(legacy, "crates", "app", AGENTS))
    _, text = oxide(proof, ["clean", "--cargo-cache"], os.path.join(legacy, "crates", "app"))
    case("legacy nested block is not adoption", "older Oxide block" in text, text)
    code, text = oxide(proof, ["init"], os.path.join(legacy, "crates", "app"))
    case("legacy nested block retained and reported", code == 0 and digest(os.path.join(legacy, "crates", "app", AGENTS)) == member_before and "retained: " in text, text)

    # Legacy root upgrade: user prose survives, `.` survives with a manifest.
    upgrade = os.path.join(work, "upgrade")
    git(upgrade)
    manifest(upgrade, "upgrade-root")
    manifest(os.path.join(upgrade, "crates", "app"), "upgrade-app")
    prose_before, prose_after = "# Project notes\n\nKeep this.\n\n", "\n## Team\nKeep this too.\n"
    write(os.path.join(upgrade, AGENTS), prose_before + OUTSIDE + prose_after)
    code, text = oxide(proof, ["init"], os.path.join(upgrade, "crates", "app"))
    upgraded = read(os.path.join(upgrade, AGENTS))
    case(
        "legacy root upgrade keeps prose and dot",
        code == 0
        and upgraded.startswith(prose_before)
        and upgraded.endswith(prose_after)
        and "<!-- oxide:workspaces: . crates/app -->" in upgraded,
        text,
    )

    # Malformed list and markers refuse without clobber.
    damaged = os.path.join(work, "damaged")
    git(damaged)
    manifest(os.path.join(damaged, "crates", "app"), "damaged-app")
    for label, body in (
        ("duplicate list", "<!-- oxide:begin -->\n<!-- oxide:workspaces: a -->\n<!-- oxide:workspaces: b -->\n<!-- oxide:end -->\n"),
        ("unbalanced marker", "<!-- oxide:begin -->\nno end\n"),
    ):
        write(os.path.join(damaged, AGENTS), body)
        before = digest(os.path.join(damaged, AGENTS))
        code, text = oxide(proof, ["init"], os.path.join(damaged, "crates", "app"))
        case(f"{label} refuses without clobber", code != 0 and digest(os.path.join(damaged, AGENTS)) == before, text)
        _, text = oxide(proof, ["clean", "--cargo-cache"], os.path.join(damaged, "crates", "app"))
        case(f"{label} is not adoption", "cannot confirm" in text, text)

    # Unsafe path: a space in a covered component refuses before any write.
    unsafe = os.path.join(work, "unsafe")
    git(unsafe)
    manifest(os.path.join(unsafe, "my apps", "app"), "unsafe-app")
    code, text = oxide(proof, ["init"], os.path.join(unsafe, "my apps", "app"))
    case("unsafe path refuses before writing", code != 0 and "unsafe" in text and not os.path.exists(os.path.join(unsafe, AGENTS)) and not os.path.exists(os.path.join(unsafe, "my apps", "app", ".gitignore")), text)

    # A symlinked or oversize root AGENTS.md is never followed or rewritten.
    linked = os.path.join(work, "linked")
    git(linked)
    manifest(os.path.join(linked, "crates", "app"), "linked-app")
    target = os.path.join(work, "target.md")
    write(target, "outside text\n")
    os.symlink(target, os.path.join(linked, AGENTS))
    code, text = oxide(proof, ["init"], os.path.join(linked, "crates", "app"))
    case("symlinked root AGENTS refuses", code != 0 and os.path.islink(os.path.join(linked, AGENTS)) and read(target) == "outside text\n", text)
    oversize = os.path.join(work, "oversize")
    git(oversize)
    manifest(os.path.join(oversize, "crates", "app"), "oversize-app")
    with open(os.path.join(oversize, AGENTS), "wb") as handle:
        handle.write(b"x\n" * (512 * 1024 + 1))
    before = digest(os.path.join(oversize, AGENTS))
    code, text = oxide(proof, ["init"], os.path.join(oversize, "crates", "app"))
    case("oversize root AGENTS refuses unchanged", code != 0 and digest(os.path.join(oversize, AGENTS)) == before, text)

    # A corrupt repository config is a Git error, not positive absence.
    corrupt = os.path.join(work, "corrupt")
    git(corrupt)
    manifest(os.path.join(corrupt, "crates", "app"), "corrupt-app")
    write(os.path.join(corrupt, ".git", "config"), "[core\n")
    code, text = oxide(proof, ["init"], os.path.join(corrupt, "crates", "app"))
    case("corrupt Git config refuses with nothing written", code != 0 and not os.path.exists(os.path.join(corrupt, AGENTS)) and not os.path.exists(os.path.join(corrupt, "crates", "app", ".gitignore")), text)

    # Two concurrent inits in one root keep both paths.
    concurrent = os.path.join(work, "concurrent")
    git(concurrent)
    manifest(os.path.join(concurrent, "crates", "a"), "a")
    manifest(os.path.join(concurrent, "crates", "b"), "b")
    children = [
        subprocess.Popen([proof.binary, "init"], cwd=os.path.join(concurrent, "crates", name), env=ENV, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        for name in ("a", "b")
    ]
    codes = []
    texts = []
    for child in children:
        out, _ = child.communicate(timeout=120)
        codes.append(child.returncode)
        texts.append(out.decode(errors="replace"))
    listed = read(os.path.join(concurrent, AGENTS)) if os.path.exists(os.path.join(concurrent, AGENTS)) else ""
    case("concurrent inits keep both paths", codes == [0, 0] and "<!-- oxide:workspaces: crates/a crates/b -->" in listed, " | ".join(texts))


def stage_candidate(proof: Proof, repo: str) -> str:
    staged = layout.stage_at(proof, repo, "candidate")
    expect(os.path.realpath(staged).startswith(proof.root + "/"), "staged binary is outside the owned scratch")
    with open(staged, "rb") as handle:
        expect(CANDIDATE_MARKER in handle.read(), "staged binary lacks the guidance readiness text")
    return staged


def main() -> int:
    if sys.platform != "darwin":
        fail("the init guidance proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"OXIDE_TASK_SCRATCH_ROOT {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the runner's scratch")
    expect(os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ, "not under the qualified runner")
    root = tempfile.mkdtemp(prefix="init-guidance.", dir=scratch)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, lambda signum, frame: sys.exit(128 + signum))
    global ENV
    ENV = environment(root)
    proof = Proof(root)
    proof.env = ENV
    try:
        staged = stage_candidate(proof, repo)
        run(proof, root, staged)
    except BaseException as failure:
        print(f"init guidance proof failed after {time.monotonic() - STARTED:.0f} s: {failure!r}; scratch {root} left to the runner", file=sys.stderr)
        raise
    passed = sum(1 for _, ok in results if ok)
    elapsed = time.monotonic() - STARTED
    print(f"init guidance proof: {passed}/{len(results)} cases passed in {elapsed:.0f} s; scratch closure and cleanup are the shared runner's")
    expect(elapsed <= PROOF_SECONDS, f"proof exceeded {PROOF_SECONDS} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
