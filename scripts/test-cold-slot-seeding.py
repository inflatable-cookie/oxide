#!/usr/bin/env python3
"""Cold-slot seeding: the fixed native proof (oxide#137 §16, oxide#169 slice 4).

One fixed script and the whole command of the qualified shared supervisor:

    python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 \
        --scratch-budget-mib 512 -- python3 scripts/test-cold-slot-seeding.py

`--root-parent /private/tmp` is the owner-accepted internal-placement
exception (decision `bd357a4c`): the proof's private pool is a genuine internal
`directory-budget-v1` pool, which the product correctly refuses on an external
volume, and one case registers a removable pool whose control directory is
likewise internal. Everything else stays inside the supervisor's own monitored
scratch, deadline and evidence directory.

`effigy rust:cold-slot-seeding:diagnostic` appends `--diagnostic`: the required
uncounted keep-going collection. It records every declared case's outcome
(reached/completed/failed/skipped/unexecuted) and the latched first fault in a
bounded `qualified: false` receipt beside the guard's, continues only into
cases whose declared dependencies completed, and aborts on an unknown child,
writer, mount, custody, resource or deadline outcome. `effigy
proof:cold-slot-seeding:test` runs only `--self-check`/`--self-test`: the pure
apparatus, with no child, Cargo, pool, image or scratch. `--plan` prints the
case table, allocation and premise dispositions and touches nothing.

Cases (each one is self-contained: its own pool and checkouts, so one failure
never taints another premise):

01 SQLite/`cc`/`OUT_DIR` at its real scope. A private workspace whose only
   dependency path is `libsqlite3-sys` with `bundled` (the amalgamation compiled
   by `cc`, exposed through `OUT_DIR`). The donor epoch builds it; the
   recipient's run unit reruns, `libsqlite3-sys` recompiles, and the program's
   runtime `env!("OUT_DIR")` and SQLite version are correct after the donor slot
   is deleted. `libsqlite3-sys` has dependency edges and is never offered, so
   this proves Cargo's own rerun for an ineligible package; case 03 supplies the
   non-vacuous exclusion oracle.
02 The leaf registry positive: a dependency-free `cfg-if` is offered from its
   literal-name fingerprint directory with normalized unit and artifacts, two
   checkouts share one lock and Cargo home, the second leases a new slot that is
   seeded, and Cargo reports the offered unit fresh. The `dep-…` counterfactual
   re-runs the same fixture with the debug-only plan control that drops only
   that fingerprint file: the same unit must rebuild.
03 The offered run-unit exclusion (non-vacuous). A dependency-free git
   `seedctl` with a build script whose generated source embeds its absolute
   `OUT_DIR`: the build-script compile unit is offered (both files), the plan
   carries no run payload, the recipient reruns the script and prints the
   recipient's path. The counterfactual admits the run payload and fingerprint
   through the debug-only plan control and must print the donor's path.
04 The leaf/ambient control: a proc-macro crate with a dependency edge (so it
   is ineligible) reading an inherited environment variable at expansion; two
   recipient builds under different values both run cold and produce the two
   different values. Case 02 is the control.
05 A leaf proc-macro crate over git: verified by the sanitized protocol, offered
   as a leaf, and its expansion compiles and runs; a non-leaf proc-macro crate
   is ineligible; a configured `core.fsmonitor` hook on the private Cargo home's
   checkout is never run by the product's verification.
06 The build-script compile unit is offered but inert: the package's lib is
   rebuilt in the recipient (non-vacuous control for the excluded run unit).
07 The donor slot is deleted before the recipient's next build: the seeded files
   are independent inodes, so the recipient still compiles and runs.
08 A changed dependency: a recipient lock naming another cfg-if version offers
   nothing from the donor's catalog, and the build is correct.
09 A held donor: a gated writer holds the only compatible donor, so call 1
   cannot offer it and no catalog exists while it is held; the recipient skips
   seeding, builds cold in its own slot, and the released donor's catalog
   appears only after its own build.
10 A differing producer context: a donor built with a different `RUSTFLAGS` is
   not eligible and the recipient builds cold. Case 02 is the equal-context
   control.
11 A crash after the first clone: the next command evicts the untrusted
   generation, runs no Cargo over the partial slot and builds cold.
12 Each plan cap (units, files, bytes) and the seed's own work deadline skips
   without donor mutation.
13 Git identity negatives: a dirty checkout, a differently-revisioned
   checkout, a submodule (gitlink) checkout and a same-name registry/Git
   collision are ineligible: no donor catalog is produced and nothing seeds.
14 An unsupported Cargo identity makes the command skip seeding while Cargo
   still runs normally.
15 No donor writes: the donor tree digest, its catalog, ledger and control bytes
   and its rank/last-use/recorded figure are unchanged by a seed.
16 The removable path: one owned 128 MiB APFS image (attached with `-owners
   on`, observed) registered as the removable pool seeds a tiny `cfg-if`
   workspace across two of its slots, the recipient's bytes are on the image,
   and the image is detached ordinarily.
17 Registry tamper controls: a wrong cached archive, an extra tree file, a
   tree symlink, an altered file and a missing file each make the package
   ineligible; nothing is ever seeded from a modified tree (the missing file
   may fail Cargo's own build, so only the no-seed claim is asserted).
18 A seeded recipient never donates: its attempt says `seeded`, and a third cold
   slot receives no catalog from it.
19 The reserve gate: an injected below-trigger reading refuses before any child;
   the non-seeding control still runs Cargo; an injected mid-seed breach retains
   the untrusted generation and refuses.
20 Partial-trust negative: a fault-injected partial recipient is never reused as
   warm by a later command.
21 Artifact collision and multi-unit controls: a workspace member declaring
   `[lib] name = "cfg_if"` beside the registry `cfg-if` leaves only the verified
   registry artifact in the plan; `seedctl` is offered from separate per-unit
   fingerprint directories with its run-only directory classified and never
   copied.
22 Competing custody between selection and the pin: a real `managed::lease`
   competitor (a helper linked against the staged library) leases the selected
   donor during the debug-only pause and holds it until the seeded command
   exits. The seeded command prints `seeding skipped: donor busy`, clones
   nothing and builds cold, with the donor tree and catalog unchanged. Without
   the interleaving the same fixture seeds.
23 Ambiguous-name visibility: a donor workspace with an eligible leaf and a
   normalized-name collision pair records the candidate-unit counts; the summary
   and `oxide status` render `ambiguous name (2)`, a clean donor renders no
   suffix, and an unclassifiable rejected package renders `unknown (1)`.
24 The post-clone walk: a complete ownership-clean walk publishes; each injected
   failure (ownership, unreadable, device boundary, entry limit, walk failure)
   publishes nothing, retains the untrusted recipient, and completes as an
   ordinary cold build in a normally admitted slot.

Premise checklist (HIGH; traced through the executable helpers, not this text):

1. Private HOME and rustup: `real_rustup_home()` captures the invoking
   `RUSTUP_HOME` (or `HOME/.rustup`) before `perm.proof_env` replaces `HOME`;
   every child gets it with `RUSTUP_TOOLCHAIN=1.97.1` and
   `RUSTUP_AUTO_INSTALL=0`. Fixture locks are written before any `--locked`
   call (`lock_text`), and the registry input is seeded by the shared bounded
   seeder (`scripts/offline_seed.py`) from one union lock.
2. Account-home control: `perm.proof_env` sets `OXIDE_TEST_HOME_FROM_ENV=1`.
3. Environment isolation: `perm.proof_env` scrubs `OXIDE_*`, `XDG_*`,
   `CARGO_BUILD_*`, `CARGO_TARGET_*`, `GIT_*`; `case_env` re-adds only the
   fixture's own explicit controls.
4. Automatic adoption: the fixture checkouts are `directory-budget` and
   ephemeral (`OXIDE_DRIVE_MODE` is inherited scrub-free), so no permanent
   adoption applies; `assert_local_target` positively requires a real local
   `target` after each build.
5. Cargo external argv: not applicable (no external subcommand fixture).
6. Owned image: `perm.attach_image(..., owners=True)` and `owners_honored()`
   prove the mounted root does not carry `MNT_IGNORE_OWNERSHIP`; the one
   ordinary detach is `perm.run_ordinary_detach`. An uncertain custody fault
   retains the mounted image, and the backing lives inside the supervisor's
   monitored scratch (the proof root), removed only after that completed
   detach.
7. Compiler flags: `freeze_compiler_flags` removes `CARGO_ENCODED_RUSTFLAGS`
   and fixes `RUSTFLAGS` for staging and every fixture child; case 10 changes it
   only for its donor command.
8. Temporary storage: the child env points every temp variable at the
   supervisor's scratch; the private pool is the reviewed internal-placement
   exception. The image backing is inside that same scratch and is removed only
   after a completed ordinary detach. The competing-lease helper's library build
   has its own retained stage under the proof root, seeded from this HEAD's
   Cargo.lock and removed only after the helper linked.
9. macOS `mktemp`: `fixture_dir()` always passes a template under the
   supervisor scratch and verifies the real path.

Debug-only product controls used by this proof (all absent from release builds
and armed only under the supervisor's own exported layout, read through
`oxide_storage::proof::evidence()`): `OXIDE_TEST_SEED_PLAN=<drop-dep|admit-run|units|files|bytes|deadline>`,
`OXIDE_TEST_SEED_PAUSE=between-calls:<gate>`, `OXIDE_TEST_SEED_CRASH=after-first-clone`,
`OXIDE_TEST_SEED_BREACH=<verify|after-clone>`, `OXIDE_TEST_SEED_WALK=<gap>`, and
the existing `OXIDE_TEST_WATCHDOG_READINGS`.

Not a benchmark, not real-project qualification and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import ast
import builtins
import glob
import hashlib
import importlib.util
import inspect
import json
import os
import re
import secrets
import shutil
import signal
import stat
import subprocess
import tempfile
import textwrap
import time
import types

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "permanent_outputs", os.path.join(HERE, "test-permanent-outputs.py")
)
perm = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(perm)
_spec = importlib.util.spec_from_file_location(
    "offline_seed", os.path.join(HERE, "offline_seed.py")
)
offline_seed = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(offline_seed)

Proof = perm.Proof
expect, fail, summary, MIB = perm.expect, perm.fail, perm.summary, perm.MIB
layout = perm.layout
runner = layout.runner

BUILDER = "1.97.1"
SHUTDOWN_MARGIN_S = 30
SEED_ATTEMPT = "attempt.json"
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
CFG_IF_100 = "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd"
CFG_IF_104 = "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801"
SQLITE_028 = "0c10584274047cb335c23d3e61bcef8e323adae7c5c8c760540f73610177fc3f"
CC_1079 = "50d30906286121d95be3d479533b458f87493b30a4b5f79a607db8f5d11aa91f"

# The fixed declared case table: `(case, dependencies, description)`. Each
# case builds its own fixtures, so nothing depends on another's premise; an
# injected failure therefore skips nothing and the diagnostic reaches every
# case it can.
CASES: list[tuple[str, tuple[str, ...], str]] = [
    ("01", (), "SQLite/cc/OUT_DIR at real scope: the run unit reruns, libsqlite3-sys recompiles, OUT_DIR and SQLite version are the recipient's"),
    ("02", (), "leaf cfg-if seeds with the literal-name directory and normalized unit; dropping dep-… rebuilds it"),
    ("03", (), "offered build-script compile unit for git seedctl; no run payload, recipient OUT_DIR; admitting the run payload prints the donor's"),
    ("04", (), "ambient-reading proc macro has an edge and is ineligible: two cold builds see two values"),
    ("05", (), "leaf proc-macro over git seeds and expands; non-leaf proc macro is ineligible; core.fsmonitor never run"),
    ("06", (), "offered build-script compile unit is inert: the package lib is rebuilt"),
    ("07", (), "a deleted donor leaves the recipient compiling and running (independent inodes)"),
    ("08", (), "a changed dependency version offers nothing and builds correctly"),
    ("09", (), "a held donor is not offered, the recipient builds cold, and the released donor publishes its catalog"),
    ("10", (), "a differing RUSTFLAGS producer context is ineligible and builds cold"),
    ("11", (), "a crash after the first clone leaves an untrusted generation the next command evicts"),
    ("12", (), "the units, files and bytes caps and the seed work deadline skip without donor mutation"),
    ("13", (), "dirty, differently-revisioned and submodule git checkouts and a same-name registry/Git collision are ineligible"),
    ("14", (), "an unsupported Cargo identity skips seeding while Cargo still runs"),
    ("15", (), "a seed writes nothing to the donor: tree, catalog, ledger, control, rank and use mark"),
    ("16", (), "the removable apfs-volume-v1 path seeds across two slots on one owned image and detaches"),
    ("17", (), "wrong, extra, symlinked, altered and missing registry-tree controls never produce a seeded copy"),
    ("18", (), "a seeded recipient never donates a catalog"),
    ("19", (), "the reserve start gate refuses before any child; a mid-seed breach retains and refuses"),
    ("20", (), "a partial-trust recipient is never reused as warm"),
    ("21", (), "a workspace cfg_if-named lib leaves only registry artifacts; seedctl is per-unit"),
    ("22", (), "a real competing managed::lease between selection and the pin is a donor-busy skip with no block"),
    ("23", (), "ambiguous names render candidate-unit counts, and unknown is distinguished from zero"),
    ("24", (), "the post-clone walk publishes only when complete and ownership-clean"),
]

PHASES = [
    "setup: stage this HEAD's oxide and seed one private offline Cargo home",
    "cases 01-15: internal directory-budget pools",
    "case 16: one owned 128 MiB APFS image, ordinary detach",
    "cases 17-24: internal pools after the image phase",
]

# Child environment variables this script owns; `perm.proof_env` has already
# scrubbed every inherited OXIDE_* value.
OWN_VARS = (
    "OXIDE_COLD_SEED",
    "OXIDE_TEST_SEED_PLAN",
    "OXIDE_TEST_SEED_PAUSE",
    "OXIDE_TEST_SEED_CRASH",
    "OXIDE_TEST_SEED_BREACH",
    "OXIDE_TEST_SEED_WALK",
    "OXIDE_TEST_SEED_LIMIT",
    "OXIDE_TEST_WATCHDOG_READINGS",
    "OXIDE_POOL_CAP",
)


def work_left() -> float:
    """Seconds left before the shared runner's exported deadline, less the
    script's own shutdown margin (the runner owns the real stop)."""
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the runner's shutdown margin")
    return left


def owned_bytes(root: str) -> int:
    total = 0
    for dirpath, dirnames, filenames in os.walk(root, onerror=_raise):
        for name in dirnames + filenames:
            try:
                total += os.lstat(os.path.join(dirpath, name)).st_blocks * 512
            except FileNotFoundError:
                continue
    return total


def _raise(error: OSError) -> None:
    raise error


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def tree_digest(root: str) -> dict[str, str]:
    """Path -> content/mode digest for every entry under `root`."""
    result: dict[str, str] = {}
    for dirpath, dirnames, filenames in os.walk(root, onerror=_raise):
        dirnames.sort()
        for name in sorted(dirnames) + sorted(filenames):
            path = os.path.join(dirpath, name)
            relative = os.path.relpath(path, root)
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                result[relative] = "link:" + os.readlink(path)
            elif stat.S_ISDIR(info.st_mode):
                result[relative] = "dir"
            else:
                result[relative] = f"file:{sha256_file(path)}"
    return result


def read_json(path: str) -> dict:
    with open(path, "rb") as handle:
        return json.loads(handle.read().decode())


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def real_rustup_home() -> str:
    """Premise 1: the invoking toolchain home, captured before HOME changes."""
    value = os.environ.get("RUSTUP_HOME")
    if value:
        return value
    home = os.environ.get("HOME")
    expect(bool(home), "the invoking HOME is unavailable")
    return os.path.join(home, ".rustup")


FROZEN_RUSTFLAGS = "--cfg oxide_cold_seed_proof"


def freeze_compiler_flags(env: dict) -> None:
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["RUSTFLAGS"] = FROZEN_RUSTFLAGS


def fixture_dir(parent: str, prefix: str) -> str:
    """Premise 9: an explicit template under the intended owned parent."""
    made = tempfile.mkdtemp(prefix=prefix, dir=parent)
    real = os.path.realpath(made)
    expect(real.startswith(os.path.realpath(parent) + os.sep), f"{made} escaped {parent}")
    return real


def write_tree(root: str, files: dict[str, str]) -> str:
    for relative, contents in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(contents)
    return os.path.realpath(root)


def lock_text(packages: list[dict], app: tuple[str, str] = ("app", "0.1.0")) -> str:
    """One Cargo.lock in the fixed v4 grammar; `packages` are the resolved
    dependencies (the only resolution is the one the manifest declares)."""
    def dependencies(names: list[str]) -> list[str]:
        if not names:
            return []
        # Cargo's own v4 shape: one basic string per line. The shared bounded
        # seeder reads exactly this grammar (a single-line array is refused).
        return ["dependencies = ["] + [f'"{name}",' for name in names] + ["]"]

    lines = ["# generated by the cold-slot seeding proof", "version = 4", ""]
    lines += ["[[package]]", f'name = "{app[0]}"', f'version = "{app[1]}"']
    lines += dependencies([package["name"] for package in packages])
    lines.append("")
    for package in packages:
        lines.append("[[package]]")
        lines.append(f'name = "{package["name"]}"')
        lines.append(f'version = "{package["version"]}"')
        if package.get("source"):
            lines.append(f'source = "{package["source"]}"')
        if package.get("checksum"):
            lines.append(f'checksum = "{package["checksum"]}"')
        lines += dependencies(package.get("dependencies") or [])
        lines.append("")
    return "\n".join(lines)


def registry(name: str, version: str, checksum: str, dependencies: list[str] | None = None) -> dict:
    return {
        "name": name,
        "version": version,
        "source": CRATES_IO,
        "checksum": checksum,
        "dependencies": dependencies or [],
    }


def git_package(name: str, url: str, rev: str) -> dict:
    return {"name": name, "version": "0.1.0", "source": f"git+{url}#{rev}"}


def slot_of(stderr: str) -> str:
    slot, _warm = perm.slot_state(stderr)
    return slot


def seeded(stderr: str) -> bool:
    return "oxide: seeded " in stderr


def seeded_units(stderr: str) -> int:
    match = re.search(r"oxide: seeded (\d+) units", stderr)
    expect(match is not None, f"no seeded summary line: {stderr[-2000:]}")
    return int(match.group(1))


def skip_line(stderr: str) -> str:
    lines = [line for line in stderr.splitlines() if "seeding skipped:" in line]
    expect(len(lines) >= 1, f"no seeding skip line: {stderr[-2000:]}")
    return lines[-1]


def seeds_dir(pool: str) -> str:
    return os.path.join(pool, "oxide-managed", "seeds")


def catalog_path(pool: str, slot: str) -> str:
    return os.path.join(seeds_dir(pool), f"{slot}.catalog.json")


def attempt_path(pool: str, slot: str) -> str:
    return os.path.join(seeds_dir(pool), f"{slot}.{SEED_ATTEMPT}")


def attempt_state(pool: str, slot: str) -> str | None:
    path = attempt_path(pool, slot)
    if not os.path.isfile(path):
        return None
    return read_json(path).get("state")


def last_seed(pool: str) -> dict | None:
    path = os.path.join(seeds_dir(pool), "last.json")
    return read_json(path) if os.path.isfile(path) else None


def payload_slots(pool: str) -> str:
    return os.path.join(pool, "payload", "slots")


def slot_leaf(pool: str, slot: str) -> str:
    return runner.labelled_slot(payload_slots(pool), int(slot))


def ledger_bytes(pool: str) -> bytes:
    return read_bytes(os.path.join(pool, "oxide-managed", "ledger.json"))


def control_bytes(pool: str) -> bytes:
    return read_bytes(os.path.join(pool, "oxide-managed", "state.json"))


def slot_tree(pool: str, slot: str) -> dict[str, str]:
    leaf = slot_leaf(pool, slot)
    return tree_digest(os.path.join(leaf, "build"))


def record_figure(pool: str, slot: str) -> dict:
    """The ledger's own row for one slot: rank, last use, recorded bytes."""
    ledger = json.loads(ledger_bytes(pool).decode())
    for row in ledger.get("slots", []):
        if str(row.get("id")) == str(slot):
            return {
                "rank": row.get("rank"),
                "last_used": row.get("last_used"),
                "recorded": row.get("recorded"),
                "state": row.get("state"),
                "last_lease_id": row.get("last_lease_id"),
            }
    fail(f"slot {slot} is not in the ledger")


def assert_local_target(checkout: str) -> None:
    """Premise 4: this fixture's outputs stayed checkout-local."""
    target = os.path.join(checkout, "target")
    expect(os.path.isdir(target) and not os.path.islink(target), f"{target} is not a real directory")


def expect_no_seed_mutation(pool: str, slot: str, before: dict) -> None:
    """The donor's own facts, not the pool's: the recipient's own lease and
    release legitimately change the shared ledger and control state."""
    expect(slot_tree(pool, slot) == before["tree"], "the donor's build tree changed")
    expect(read_bytes(catalog_path(pool, slot)) == before["catalog"], "the donor's catalog changed")
    expect(record_figure(pool, slot) == before["figure"], "the donor's ledger row changed")


def record_donor_figure(pool: str, slot: str) -> dict:
    return {
        "tree": slot_tree(pool, slot),
        "catalog": read_bytes(catalog_path(pool, slot)),
        "figure": record_figure(pool, slot),
        "slot": slot,
    }


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

SEEDCTL_MANIFEST = """[package]
name = "seedctl"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[dependencies]
"""
SEEDCTL_BUILD = r'''use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out = env::var("OUT_DIR").unwrap();
    println!("cargo:rustc-env=SEEDCTL_OUT_DIR={out}");
    println!("cargo:rerun-if-changed=build.rs");
    let generated = format!("pub const EMBEDDED: &str = {out:?};\n");
    fs::write(Path::new(&out).join("generated.rs"), generated).unwrap();
}
'''
SEEDCTL_LIB = '''mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub fn out_dir() -> &'static str {
    env!("SEEDCTL_OUT_DIR")
}

pub fn embedded() -> &'static str {
    generated::EMBEDDED
}
'''
SEEDPM_MANIFEST = """[package]
name = "seedpm"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
"""
SEEDPM_LIB = r'''use proc_macro::TokenStream;

#[proc_macro]
pub fn value(_input: TokenStream) -> TokenStream {
    let text = std::env::var("SEEDPM_VALUE").unwrap_or_else(|_| "unset".to_owned());
    format!("{{ const V: &str = {text:?}; V }}")
        .parse()
        .expect("generated proc-macro body")
}
'''
SQLITE_APP_MANIFEST = """[package]
name = "app"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[dependencies]
libsqlite3-sys = { version = "=0.28.0", default-features = false, features = ["bundled"] }
"""
SQLITE_APP_BUILD = r'''fn main() {
    println!(
        "cargo:rustc-env=APP_OUT={}",
        std::env::var("OUT_DIR").unwrap()
    );
}
'''
SQLITE_APP_MAIN = '''fn main() {
    println!("OUT={}", env!("APP_OUT"));
    println!("SQLITE={}", libsqlite3_sys::SQLITE_VERSION_NUMBER);
}
'''
def git_fixture(
    root: str,
    name: str,
    files: dict[str, str],
    *,
    commits: int = 1,
    submodule: str | None = None,
    package: str | None = None,
) -> dict:
    """One owned local Git repository under the proof's scratch.

    `submodule` adds a real `file://` submodule entry (a gitlink) before the
    first commit; the proof never initialises it, so the checkout carries an
    uninitialised gitlink exactly as an untrusted dependency would.
    """
    repo = os.path.join(root, "git", name)
    os.makedirs(repo, exist_ok=True)
    write_tree(repo, files)
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", "/"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_AUTHOR_NAME": "oxide proof",
        "GIT_AUTHOR_EMAIL": "proof@example.invalid",
        "GIT_COMMITTER_NAME": "oxide proof",
        "GIT_COMMITTER_EMAIL": "proof@example.invalid",
    }

    def git(*args: str) -> str:
        result = subprocess.run(
            ["git", *args], cwd=repo, env=env, capture_output=True, text=True, timeout=60
        )
        expect(result.returncode == 0, f"git {' '.join(args)} failed: {result.stderr[-400:]}")
        return result.stdout.strip()

    git("init", "-q", "-b", "main")
    if submodule is not None:
        added = subprocess.run(
            ["git", "-c", "protocol.file.allow=always", "submodule", "add", submodule, "inner"],
            cwd=repo,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        expect(added.returncode == 0, f"submodule add failed: {added.stderr[-400:]}")
    git("add", "-A")
    git("commit", "-q", "-m", "first")
    revisions = [git("rev-parse", "HEAD")]
    for index in range(1, commits):
        marker = os.path.join(repo, "REVISION")
        with open(marker, "w", encoding="utf-8") as handle:
            handle.write(f"{index}\n")
        git("add", "-A")
        git("commit", "-q", "-m", f"revision {index}")
        revisions.append(git("rev-parse", "HEAD"))
    return {
        "repo": repo,
        "url": f"file://{repo}",
        "package": package or name,
        "revisions": revisions,
    }


def seed_registry_homes(proof: Proof, repo: str) -> None:
    """One private offline CARGO_HOME from the shared bounded seeder.

    Besides the tiny fixture locks, this HEAD's own Cargo.lock is seeded so the
    helper's private `cargo build --lib` needs no host Cargo home. The shared
    seeder copies the registry crates and the one pinned Longhorn Git source
    from the host cache within its own bounds.
    """
    host = os.environ.get("CARGO_HOME") or os.path.join(real_rustup_home(), "..", ".cargo")
    host = os.path.abspath(host)
    expect(os.path.isdir(host), f"the host CARGO_HOME {host} is absent")
    locks = [
        lock_text(
            [registry("cfg-if", "1.0.0", CFG_IF_100), registry("cfg-if", "1.0.4", CFG_IF_104)]
        ).encode(),
        lock_text(
            [
                registry("libsqlite3-sys", "0.28.0", SQLITE_028, ["cc"]),
                registry("cc", "1.0.79", CC_1079),
            ]
        ).encode(),
    ]
    for index, lock in enumerate(locks):
        lock_path = os.path.join(proof.root, f"seed-lock-{index}.lock")
        with open(lock_path, "wb") as handle:
            handle.write(lock)
        manifest = offline_seed.seed_cargo_home(host, proof.cargo_home, lock)
        print(
            f"seeded Cargo home {index}: {manifest['packages']} packages, "
            f"{manifest['aggregate_bytes']} bytes",
            flush=True,
        )
    head_lock = os.path.join(repo, "Cargo.lock")
    head_bytes = read_bytes(head_lock)
    manifest = offline_seed.seed_cargo_home(host, proof.cargo_home, head_bytes)
    print(
        f"seeded this HEAD's lock: {manifest['packages']} packages, "
        f"{manifest['aggregate_bytes']} bytes",
        flush=True,
    )
    expect(
        os.path.isdir(os.path.join(proof.cargo_home, "registry", "cache")),
        "the private Cargo home has no registry cache",
    )


def prime_git_databases(proof: Proof, repositories: dict[str, dict]) -> None:
    """Create Cargo's own Git databases for the owned fixture repositories.

    This is one local `file://` fetch, not a network access: it is the only
    non-offline Cargo call in the proof, it resolves no registry package, and
    every product command afterwards runs `--offline` with the database present.
    """
    prime = fixture_dir(proof.root, "git-prime.")
    lines = ['[package]\nname = "prime"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n']
    for name, repository in repositories.items():
        package = repository.get("package", name)
        entry = f'git = "{repository["url"]}", rev = "{repository["revisions"][0]}"'
        if package == name:
            lines.append(f"{name} = {{ {entry} }}\n")
        else:
            lines.append(f'{name} = {{ package = "{package}", {entry} }}\n')
    write_tree(prime, {"Cargo.toml": "".join(lines), "src/lib.rs": "\n"})
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": proof.home,
        "CARGO_HOME": proof.cargo_home,
        "RUSTUP_HOME": proof.rustup,
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_TERM_COLOR": "never",
        "CARGO_NET_OFFLINE": "false",
    }
    result = proof.run(["cargo", "fetch"], cwd=prime, env=env, timeout=180)
    expect(
        result.returncode == 0,
        f"the local file:// Git fetch failed: {result.stderr[-1000:]!r}",
    )
    print("primed Cargo's Git databases with one local file:// fetch", flush=True)


LEASE_HELPER = r'''use oxide_storage::lease::{managed, LeaseRequest, Profile};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pool = PathBuf::from(&args[1]);
    let project = PathBuf::from(&args[2]);
    let workspace = PathBuf::from(&args[3]);
    let gate = PathBuf::from(&args[4]);
    let grant = managed::lease(LeaseRequest {
        pool: pool.clone(),
        project,
        workspace,
        profile: Profile::Dev,
        holder: "proof-competing-lease".to_owned(),
        budget_bytes: Some(256 * 1024 * 1024),
        reserve_bytes: None,
        layout: None,
        replace_slot: None,
    })
    .expect("competing lease");
    println!("LEASE={}", grant.lease_id);
    std::fs::write(format!("{}.leased", gate.display()), b"leased").expect("lease marker");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !gate.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    managed::release(&pool, &grant.lease_id).expect("competing release");
}
'''


def staged_library(stage_root: str) -> tuple[str, str]:
    """The staged `liboxide_storage` rlib and the dependency directory that
    holds it.

    The helper builds this stage itself: `layout.stage_at` sets
    `CARGO_BUILD_BUILD_DIR=<stage-root>/build` and then deletes its entire stage
    after copying the candidate binary, so no library artifact from it survives.
    A retained `cargo build --lib` stage therefore places the library under
    `build/debug/deps`; a plain target directory without a build dir would place
    it under `target/debug/deps`. Exactly one of those roots may carry the
    library, and it must carry exactly one rlib: zero or ambiguous matches
    refuse rather than linking a wrong or partial artifact.
    """
    roots = [
        os.path.join(stage_root, "build", "debug", "deps"),
        os.path.join(stage_root, "target", "debug", "deps"),
    ]
    found = []
    for deps in roots:
        if not os.path.isdir(deps):
            continue
        rlibs = sorted(glob.glob(os.path.join(deps, "liboxide_storage-*.rlib")))
        if rlibs:
            found.append((deps, rlibs))
    expect(
        len(found) == 1,
        f"expected exactly one staged oxide_storage deps root, got {[deps for deps, _ in found]}",
    )
    deps, rlibs = found[0]
    expect(
        len(rlibs) == 1,
        f"expected exactly one staged oxide_storage rlib under {deps}, got {rlibs}",
    )
    return deps, rlibs[0]


def helper_build_env(proof: Proof, stage_root: str) -> dict:
    """The private, offline environment for the helper's own library build.

    Same pinned toolchain and frozen flags as the proof's fixtures, with the
    private HOME and CARGO_HOME (already seeded from the proofs' locks and from
    this HEAD's Cargo.lock), and both Cargo output directories under the
    retained stage. Inherited wrappers and Cargo output overrides are removed.
    """
    env = dict(os.environ)
    for name in list(env):
        if name.startswith(("SCCACHE_", "CARGO_BUILD_", "CARGO_TARGET_")) or name in (
            "CARGO",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_ENCODED_RUSTFLAGS",
        ):
            env.pop(name, None)
    env.update(
        {
            "HOME": proof.home,
            "CARGO_HOME": proof.cargo_home,
            "RUSTUP_HOME": proof.rustup,
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "RUSTFLAGS": FROZEN_RUSTFLAGS,
            "CARGO_TARGET_DIR": os.path.join(stage_root, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage_root, "build"),
            "CARGO_INCREMENTAL": "0",
            "CARGO_PROFILE_DEV_DEBUG": "0",
        }
    )
    return env


def build_lease_helper(proof: Proof, repo: str) -> str:
    """One tiny helper linked against a private retained library build: a real
    `managed::lease` competitor, not a forged lock or ledger state.

    `layout.stage_at` deletes its stage, so this builds the library itself under
    `<proof.root>/lease-stage` with the pinned offline Cargo and the private
    Cargo home, links the helper from the one staged rlib, and removes the stage
    only after that link succeeded. On any failure the stage is kept as
    evidence and the error propagates.
    """
    stage_root = os.path.join(proof.root, "lease-stage")
    os.makedirs(stage_root, mode=0o700)
    env = helper_build_env(proof, stage_root)
    build = proof.run(
        [
            "cargo",
            "build",
            "--lib",
            "--locked",
            "--offline",
            "--jobs",
            "4",
            "--message-format=json-render-diagnostics",
        ],
        cwd=repo,
        env=env,
        timeout=200,
    )
    print(build.stderr.decode(errors="replace")[-4000:], flush=True)
    expect(build.returncode == 0, f"the helper library build exited {build.returncode}")
    deps, rlib = staged_library(stage_root)
    root = os.path.join(proof.root, "lease-helper")
    os.makedirs(root, mode=0o700)
    source = os.path.join(root, "main.rs")
    with open(source, "w", encoding="utf-8") as handle:
        handle.write(LEASE_HELPER)
    binary = os.path.join(root, "competing-lease")
    result = proof.run(
        [
            "rustc",
            "--edition",
            "2021",
            "-L",
            f"dependency={deps}",
            "--extern",
            f"oxide_storage={rlib}",
            "-o",
            binary,
            source,
        ],
        env=helper_build_env(proof, stage_root),
        timeout=120,
    )
    expect(
        result.returncode == 0,
        f"the competing-lease helper did not build: {result.stderr[-1500:]!r}",
    )
    expect(
        os.path.isfile(binary) and not os.path.islink(binary),
        f"the competing-lease binary is missing at {binary}",
    )
    # Linking consumed the staged artifact: the helper is a standalone binary.
    # Remove the retained stage only now, after that positive child closure.
    shutil.rmtree(stage_root)
    print(f"built the competing-lease helper from {rlib}", flush=True)
    return binary


def setup(proof: Proof, repo: str) -> None:
    proof.root = os.path.realpath(proof.root)
    scratch = os.path.realpath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    expect(os.path.commonpath([proof.root, scratch]) == scratch, "the proof root escaped the scratch")
    proof.rustup = real_rustup_home()
    proof.home = fixture_dir(proof.root, "home.")
    proof.cargo_home = fixture_dir(proof.root, "cargo-home.")
    proof.evidence = os.environ["OXIDE_PROOF_EVIDENCE"]
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "candidate", jobs=4)
    print(f"staged candidate {proof.binary}", flush=True)
    # layout.stage_at deletes its whole stage after copying this binary, so
    # nothing below may rely on a staged artifact; the helper builds its own
    # retained library stage after the private Cargo home is seeded.
    seed_registry_homes(proof, repo)
    proof.lease_helper = build_lease_helper(proof, repo)
    inner = git_fixture(proof.root, "innerrepo", {"README.md": "inner\n"})
    proof.repositories = {
        "seedctl": git_fixture(proof.root, "seedrepo", {
            "Cargo.toml": SEEDCTL_MANIFEST,
            "build.rs": SEEDCTL_BUILD,
            "src/lib.rs": SEEDCTL_LIB,
        }, commits=2),
        "seedpm": git_fixture(proof.root, "pmrepo", {
            "Cargo.toml": SEEDPM_MANIFEST,
            "src/lib.rs": SEEDPM_LIB,
        }),
        "subrepo": git_fixture(proof.root, "subrepo", {
            "Cargo.toml": (
                '[package]\nname = "subcrate"\nversion = "0.1.0"\nedition = "2021"\n'
            ),
            "src/lib.rs": "pub fn value() -> u32 { 4 }\n",
        }, submodule=inner["repo"], package="subcrate"),
        "cfgifgit": git_fixture(proof.root, "cfgifgit", {
            "Cargo.toml": (
                '[package]\nname = "cfg-if"\nversion = "2.0.0"\nedition = "2021"\n'
            ),
            "src/lib.rs": "pub fn from_git() -> u32 { 5 }\n",
        }, package="cfg-if"),
    }
    prime_git_databases(proof, proof.repositories)
    proof.phase("after setup")


class Case:
    """One case's own pool and checkouts, with the fixture env every child gets."""

    def __init__(self, proof: Proof, case: str):
        self.proof = proof
        self.case = case
        self.root = fixture_dir(proof.root, f"case-{case}.")
        self.pool = os.path.join(self.root, "pool")

    def env(
        self,
        *,
        seed: str = "1",
        cap: str = "256MiB",
        extra: dict | None = None,
        offline: bool = True,
        rustflags: str | None = FROZEN_RUSTFLAGS,
    ) -> dict:
        env = perm.proof_env(self.pool, self.proof.home, self.proof.cargo_home)
        for name in OWN_VARS:
            env.pop(name, None)
        env.update(
            {
                "OXIDE_COLD_SEED": seed,
                "OXIDE_POOL_CAP": cap,
                "RUSTUP_HOME": self.proof.rustup,
                "RUSTUP_TOOLCHAIN": BUILDER,
                "RUSTUP_AUTO_INSTALL": "0",
                "CARGO_NET_OFFLINE": "true" if offline else "false",
            }
        )
        if rustflags is None:
            env.pop("RUSTFLAGS", None)
        else:
            env["RUSTFLAGS"] = rustflags
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
        if extra:
            env.update(extra)
        return env

    def checkout(self, name: str, files: dict[str, str], packages: list[dict] | None = None) -> str:
        root = os.path.join(self.root, name)
        os.makedirs(root, exist_ok=True)
        write_tree(root, files)
        if packages is not None:
            with open(os.path.join(root, "Cargo.lock"), "w", encoding="utf-8") as handle:
                handle.write(lock_text(packages))
        return root

    def command(
        self,
        checkout: str,
        args: list[str],
        label: str,
        *,
        env: dict | None = None,
        expect_code: int = 0,
        allow_any_exit: bool = False,
    ) -> tuple[int, str, str]:
        env = env if env is not None else self.env()
        code, stdout, stderr = perm.oxide_env(self.proof, args, label, checkout, env)
        try:
            perm.require_confirmed_closure(label, stderr)
            perm.require_normal_exit(label, code)
        except RuntimeError as fault:
            raise perm.ProofAbort(str(fault)) from fault
        if not allow_any_exit:
            expect(code == expect_code, f"{label}: exit {code}, wanted {expect_code}")
        return code, stdout, stderr

    def build(
        self,
        checkout: str,
        label: str,
        *,
        env: dict | None = None,
        extra_args: list[str] | None = None,
    ) -> tuple[int, str, str]:
        return self.command(checkout, ["build", *(extra_args or [])], label, env=env)


def json_messages(stdout: str) -> list[dict]:
    messages = []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            messages.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return messages


def artifact_fresh(stdout: str, package: str) -> bool | None:
    fresh = None
    for message in json_messages(stdout):
        if message.get("reason") != "compiler-artifact":
            continue
        identity = json.dumps(message.get("package_id", ""))
        if package in identity:
            fresh = message.get("fresh")
    return fresh


def script_execution(stdout: str, package: str) -> dict | None:
    found = None
    for message in json_messages(stdout):
        if message.get("reason") != "build-script-executed":
            continue
        if package in json.dumps(message.get("package_id", "")):
            found = message
    return found


def run_child(proof: Proof, command: list, cwd: str, env: dict, label: str) -> tuple[int, str, str]:
    result = proof.run(command, cwd=cwd, env=env)
    stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
    print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
    try:
        perm.require_normal_exit(label, result.returncode)
    except RuntimeError as fault:
        raise perm.ProofAbort(str(fault)) from fault
    return result.returncode, stdout, stderr


# ---------------------------------------------------------------------------
# Cases 01-12
# ---------------------------------------------------------------------------

def cfg_donor_recipient(case: Case, *, app_version: str = "1.0.0", prefix: str = "") -> tuple[str, str, str, str]:
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        f'[dependencies]\ncfg-if = "={app_version}"\n'
    )
    files = {
        "Cargo.toml": manifest,
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    packages = [registry("cfg-if", app_version, CFG_IF_100 if app_version == "1.0.0" else CFG_IF_104)]
    donor = case.checkout(prefix + "donor", files, packages)
    recipient = case.checkout(prefix + "recipient", files, packages)
    code, _, err = case.build(donor, prefix + "donor build")
    donor_slot = slot_of(err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the donor published no catalog")
    return donor, recipient, donor_slot, recipient


def case_01(proof: Proof) -> None:
    case = Case(proof, "01")
    manifest = SQLITE_APP_MANIFEST
    packages = [
        registry("libsqlite3-sys", "0.28.0", SQLITE_028, ["cc"]),
        registry("cc", "1.0.79", CC_1079),
    ]
    files = {
        "Cargo.toml": manifest,
        "build.rs": SQLITE_APP_BUILD,
        "src/main.rs": SQLITE_APP_MAIN,
    }
    donor = case.checkout("donor", files, packages)
    recipient = case.checkout("recipient", files, packages)
    base_env = case.env()
    _code, donor_out, donor_err = case.command(
        donor, ["build", "--message-format=json"], "sqlite donor", env=base_env
    )
    donor_slot = slot_of(donor_err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the donor published no catalog")
    execution = script_execution(donor_out, "libsqlite3-sys")
    expect(execution is not None, "the donor ran no libsqlite3-sys build script")
    donor_out_dir = execution["out_dir"]
    code, out, err = case.command(
        recipient, ["build", "--message-format=json"], "sqlite recipient", env=base_env
    )
    line = summary(err)
    expect(seeded(err), f"the SQLite recipient did not seed: {err[-2000:]}")
    expect(seeded_units(err) == 1, f"expected only the leaf cc unit to be offered: {line}")
    sqlite = script_execution(out, "libsqlite3-sys")
    expect(sqlite is not None and sqlite.get("fresh") is False, "libsqlite3-sys did not rerun")
    app_script = script_execution(out, "app")
    expect(app_script is not None and app_script.get("fresh") is False, "the app build script did not rerun")
    recipient_slot = slot_of(err)
    expect(recipient_slot != donor_slot, "the recipient reused the donor slot")
    leaf = slot_leaf(case.pool, recipient_slot)
    expect(sqlite["out_dir"].startswith(leaf), f"libsqlite3-sys OUT_DIR {sqlite['out_dir']} is not the recipient's")
    # The donor slot is deleted before the recipient program runs.
    shutil.rmtree(slot_leaf(case.pool, donor_slot))
    code, stdout, stderr = case.command(recipient, ["run"], "sqlite recipient run", env=base_env)
    values = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
    expect(values.get("OUT", "").startswith(leaf), f"APP_OUT {values.get('OUT')} is not the recipient's OUT_DIR")
    expect(values.get("OUT") != donor_out_dir, "the recipient reused the donor's OUT_DIR")
    expect(values.get("SQLITE", "0").isdigit() and int(values["SQLITE"]) >= 3_000_000, f"SQLite version {values.get('SQLITE')!r}")
    print(f"case 01: SQLite reran in the recipient ({values['SQLITE']}), donor removed", flush=True)


def case_02(proof: Proof) -> None:
    case = Case(proof, "02")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    code, out, err = case.command(
        recipient, ["build", "--message-format=json"], "cfg-if recipient", env=case.env()
    )
    expect(seeded(err), f"the leaf did not seed: {err[-2000:]}")
    expect(seeded_units(err) == 1, f"expected one offered unit: {err[-1000:]}")
    expect(artifact_fresh(out, "cfg-if") is True, "Cargo did not report the seeded cfg-if fresh")
    expect(slot_of(err) != donor_slot, "the recipient reused the donor's slot")
    assert_local_target(recipient)
    # Counterfactual: the same fixture with only the dep-… fingerprint file
    # dropped from the plan must rebuild the unit.
    other = Case(proof, "02")
    _donor2, recipient2, _slot2, _ = cfg_donor_recipient(other, prefix="drop-")
    env = other.env(extra={"OXIDE_TEST_SEED_PLAN": "drop-dep"})
    code, out, err = other.command(
        recipient2, ["build", "--message-format=json"], "cfg-if dep-drop", env=env
    )
    expect(seeded(err), f"the dep-departure control did not seed: {err[-2000:]}")
    expect(artifact_fresh(out, "cfg-if") is False, "dropping dep-… did not rebuild cfg-if")
    print("case 02: leaf seeded fresh; dropping dep-… rebuilt it", flush=True)


def seedctl_app(repository: dict, rev: str) -> dict[str, str]:
    return {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            f'[dependencies]\nseedctl = {{ git = "{repository["url"]}", rev = "{rev}" }}\n'
        ),
        "src/main.rs": (
            'fn main() {\n'
            '    println!("OUT={}", seedctl::out_dir());\n'
            '    println!("EMBED={}", seedctl::embedded());\n'
            '}\n'
        ),
    }


def seedctl_fixture(case: Case, prefix: str = "") -> tuple[str, str, str, str]:
    repository = case.proof.repositories["seedctl"]
    rev = repository["revisions"][0]
    files = seedctl_app(repository, rev)
    packages = [git_package("seedctl", repository["url"], rev)]
    donor = case.checkout(prefix + "donor", files, packages)
    recipient = case.checkout(prefix + "recipient", files, packages)
    code, out, err = case.command(
        donor, ["build", "--message-format=json"], prefix + "seedctl donor", env=case.env()
    )
    donor_slot = slot_of(err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the seedctl donor published no catalog")
    execution = script_execution(out, "seedctl")
    expect(execution is not None, "the seedctl donor ran no build script")
    return donor, recipient, donor_slot, execution["out_dir"]


def case_03(proof: Proof) -> None:
    case = Case(proof, "03")
    _donor, recipient, _slot, donor_out = seedctl_fixture(case)
    code, out, err = case.command(
        recipient, ["build", "--message-format=json"], "seedctl recipient", env=case.env()
    )
    expect(seeded(err), f"the seedctl build-script unit did not seed: {err[-2000:]}")
    expect(seeded_units(err) == 1, f"expected one offered compile unit: {err[-1000:]}")
    execution = script_execution(out, "seedctl")
    expect(execution is not None and execution.get("fresh") is False, "the run unit did not rerun")
    leaf = slot_leaf(case.pool, slot_of(err))
    expect(execution["out_dir"].startswith(leaf), f"the run unit's OUT_DIR {execution['out_dir']} is not the recipient's")
    # The plan carries no run payload: the cloned build tree has the compile
    # unit's fingerprint and no run fingerprint, output or out/ directory.
    build = os.path.join(leaf, "build", "debug")
    run_files = []
    for dirpath, _dirnames, filenames in os.walk(os.path.join(build, ".fingerprint")):
        for name in filenames:
            if name.startswith("run-build-script-build"):
                run_files.append(os.path.join(dirpath, name))
    expect(not run_files, f"the plan copied a run fingerprint: {run_files}")
    payload = []
    build_root = os.path.join(build, "build")
    if os.path.isdir(build_root):
        for entry in os.listdir(build_root):
            for name in ("output", "root-output", "out"):
                if os.path.exists(os.path.join(build_root, entry, name)):
                    payload.append(os.path.join(entry, name))
    expect(not payload, f"the plan copied run payload entries: {payload}")
    code, stdout, stderr = case.command(recipient, ["run"], "seedctl recipient run", env=case.env())
    values = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
    expect(values.get("OUT", "").startswith(leaf), f"the program printed OUT={values.get('OUT')}")
    expect(values.get("OUT") != donor_out, "the recipient printed the donor's OUT_DIR")
    expect(values.get("EMBED") == values.get("OUT"), "the embedded path is not the recipient's")
    # Counterfactual: admitting the run payload and fingerprint must print the
    # donor's path, proving the oracle can distinguish a copied run.
    other = Case(proof, "03")
    _donor2, recipient2, _slot2, donor_out2 = seedctl_fixture(other, prefix="admit-")
    env = other.env(extra={"OXIDE_TEST_SEED_PLAN": "admit-run"})
    code, out, err = other.command(recipient2, ["build"], "seedctl admit-run", env=env)
    expect(seeded(err), f"the admit-run control did not seed: {err[-2000:]}")
    code, stdout, stderr = other.command(recipient2, ["run"], "seedctl admit-run program", env=env)
    values = dict(line.split("=", 1) for line in stdout.splitlines() if "=" in line)
    expect(values.get("OUT") == donor_out2, f"the copied run printed OUT={values.get('OUT')}, not the donor's {donor_out2}")
    print("case 03: offered build-script unit excludes the run; admitting it prints the donor path", flush=True)


def case_04(proof: Proof) -> None:
    case = Case(proof, "04")
    workspace = {
        "Cargo.toml": (
            '[workspace]\nmembers = ["app", "ambient", "ambient-pm"]\nresolver = "2"\n'
        ),
    }
    ambient_app = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\nambient = { path = "../ambient" }\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", ambient::value()); }\n',
    }
    ambient = {
        "Cargo.toml": (
            '[package]\nname = "ambient"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\nambient-pm = { path = "../ambient-pm" }\n'
        ),
        "src/lib.rs": 'pub fn value() -> &\'static str { ambient_pm::value!() }\n',
    }
    ambient_pm = {
        "Cargo.toml": (
            '[package]\nname = "ambient-pm"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[lib]\nproc-macro = true\n\n[dependencies]\ncfg-if = "=1.0.0"\n'
        ),
        "src/lib.rs": SEEDPM_LIB,
    }
    packages = [
        {"name": "app", "version": "0.1.0", "dependencies": ["ambient"]},
        {"name": "ambient", "version": "0.1.0", "dependencies": ["ambient-pm"]},
        {"name": "ambient-pm", "version": "0.1.0", "dependencies": ["cfg-if"]},
        registry("cfg-if", "1.0.0", CFG_IF_100),
    ]
    root = case.checkout("workspace", workspace)
    for name, files in (("app", ambient_app), ("ambient", ambient), ("ambient-pm", ambient_pm)):
        write_tree(os.path.join(root, name), files)
    with open(os.path.join(root, "Cargo.lock"), "w", encoding="utf-8") as handle:
        handle.write(lock_text(packages))
    donor = root
    code, _out, err = case.command(
        donor,
        ["build", "--message-format=json"],
        "ambient donor",
        env=case.env(extra={"SEEDPM_VALUE": "donor"}),
    )
    donor_slot = slot_of(err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the ambient donor published no catalog")
    values = []
    for index, value in enumerate(("first", "second")):
        other = Case(proof, "04")
        checkout = other.checkout("checkout", workspace)
        for name, files in (("app", ambient_app), ("ambient", ambient), ("ambient-pm", ambient_pm)):
            write_tree(os.path.join(checkout, name), files)
        with open(os.path.join(checkout, "Cargo.lock"), "w", encoding="utf-8") as handle:
            handle.write(lock_text(packages))
        code, out, stderr = other.command(
            checkout,
            ["build", "--message-format=json"],
            f"ambient recipient {index}",
            env=other.env(extra={"SEEDPM_VALUE": value}),
        )
        # cfg-if is a leaf and may be offered; the ambient proc macro has an
        # edge, so it is never offered and must be compiled in the recipient.
        expect(artifact_fresh(out, "ambient-pm") is False, "an ineligible ambient proc macro was offered")
        code, stdout, stderr = other.command(
            checkout,
            ["run"],
            f"ambient recipient {index} run",
            env=other.env(extra={"SEEDPM_VALUE": value}),
        )
        expect(stdout.strip() == value, f"the ambient value leaked: {stdout.strip()!r}, wanted {value!r}")
    print("case 04: both ambient recipients built cold with their own values", flush=True)


def case_05(proof: Proof) -> None:
    case = Case(proof, "05")
    repository = proof.repositories["seedpm"]
    rev = repository["revisions"][0]
    files = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            f'[dependencies]\nseedpm = {{ git = "{repository["url"]}", rev = "{rev}" }}\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", seedpm::value!()); }\n',
    }
    packages = [git_package("seedpm", repository["url"], rev)]
    donor = case.checkout("donor", files, packages)
    recipient = case.checkout("recipient", files, packages)
    code, _out, err = case.command(
        donor,
        ["build", "--message-format=json"],
        "proc-macro donor",
        env=case.env(extra={"SEEDPM_VALUE": "donor-expansion"}),
    )
    donor_slot = slot_of(err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the proc-macro donor published no catalog")
    env = case.env(extra={"SEEDPM_VALUE": "recipient-expansion"})
    code, out, err = case.command(
        recipient, ["build", "--message-format=json"], "proc-macro recipient", env=env
    )
    expect(seeded(err), f"the leaf proc macro did not seed: {err[-2000:]}")
    expect(artifact_fresh(out, "seedpm") is True, "the seeded proc-macro artifact was not fresh")
    code, stdout, stderr = case.command(recipient, ["run"], "proc-macro recipient run", env=env)
    expect(stdout.strip() == "recipient-expansion", f"the expansion printed {stdout.strip()!r}")
    # A non-leaf proc macro (it depends on cfg-if) is ineligible.
    other = Case(proof, "05")
    pmdep_repo = git_fixture(
        other.root,
        "pmdeprepo",
        {
            "Cargo.toml": (
                '[package]\nname = "pmdep"\nversion = "0.1.0"\nedition = "2021"\n\n'
                '[lib]\nproc-macro = true\n\n[dependencies]\ncfg-if = "=1.0.0"\n'
            ),
            "src/lib.rs": SEEDPM_LIB,
        },
    )
    rev2 = pmdep_repo["revisions"][0]
    files2 = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            f'[dependencies]\npmdep = {{ git = "{pmdep_repo["url"]}", rev = "{rev2}" }}\n'
            'cfg-if = "=1.0.0"\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", pmdep::value!()); }\n',
    }
    packages2 = [
        git_package("pmdep", pmdep_repo["url"], rev2),
        registry("cfg-if", "1.0.0", CFG_IF_100),
    ]
    prime_git_databases(proof, {"pmdep": pmdep_repo})
    donor2 = other.checkout("donor", files2, packages2)
    recipient2 = other.checkout("recipient", files2, packages2)
    code, _out, err = other.command(
        donor2, ["build"], "non-leaf donor", env=other.env(extra={"SEEDPM_VALUE": "donor"})
    )
    env2 = other.env(extra={"SEEDPM_VALUE": "nonleaf"})
    code, out2, err = other.command(recipient2, ["run"], "non-leaf recipient", env=env2)
    expect(stdout_line(out2) == "nonleaf", f"the non-leaf proc macro printed {stdout_line(out2)!r}")
    expect(not seeded(err), "a non-leaf proc macro was seeded")
    # A configured fsmonitor hook on the private checkout is never run by the
    # product's sanitized Git verification.
    checkout = find_checkout(proof.cargo_home, "pmrepo")
    hook = os.path.join(case.root, "fsmonitor-hook.sh")
    record = os.path.join(case.root, "fsmonitor-ran")
    with open(hook, "w", encoding="utf-8") as handle:
        handle.write(f"#!/bin/sh\necho ran >> {record}\nexit 0\n")
    os.chmod(hook, 0o755)
    run_git_config(checkout, "core.fsmonitor", hook)
    other2 = Case(proof, "05")
    recipient3 = other2.checkout("recipient", files, packages)
    donor3 = other2.checkout("donor", files, packages)
    code, _out, derr = other2.command(
        donor3, ["build"], "fsmonitor donor", env=other2.env(extra={"SEEDPM_VALUE": "donor"})
    )
    code, _out, rerr = other2.command(
        recipient3, ["run"], "fsmonitor recipient", env=other2.env(extra={"SEEDPM_VALUE": "fsmonitor"})
    )
    expect(not os.path.exists(record), "the product's Git verification ran a core.fsmonitor hook")
    print("case 05: leaf proc macro seeded; non-leaf ineligible; fsmonitor hook never ran", flush=True)


def stdout_line(stdout: str) -> str:
    lines = [line for line in stdout.splitlines() if line.strip()]
    return lines[-1].strip() if lines else ""


def find_checkout(cargo_home: str, repository: str) -> str:
    base = os.path.join(cargo_home, "git", "checkouts")
    expect(os.path.isdir(base), "the private Cargo home has no Git checkouts")
    for name in sorted(os.listdir(base)):
        if name.startswith(repository + "-"):
            revisions = os.path.join(base, name)
            for revision in sorted(os.listdir(revisions)):
                return os.path.join(revisions, revision)
    fail(f"no checkout for {repository} under {base}")


def run_git_config(checkout: str, key: str, value: str) -> None:
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.environ.get("HOME", "/"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": "/dev/null",
    }
    result = subprocess.run(
        ["git", "config", key, value],
        cwd=checkout,
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    expect(result.returncode == 0, f"git config {key} failed: {result.stderr[-400:]}")


def case_06(proof: Proof) -> None:
    case = Case(proof, "06")
    _donor, recipient, _slot, _out = seedctl_fixture(case)
    code, out, err = case.command(
        recipient, ["build", "--message-format=json"], "inert build-script unit", env=case.env()
    )
    expect(seeded(err) and seeded_units(err) == 1, "the build-script compile unit was not offered")
    expect(artifact_fresh(out, "seedctl") is False, "the package lib was not rebuilt")
    print("case 06: the offered compile unit is inert and the lib rebuilt", flush=True)


def case_07(proof: Proof) -> None:
    case = Case(proof, "07")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    code, _out, err = case.command(recipient, ["build"], "seeded recipient", env=case.env())
    expect(seeded(err), "the recipient did not seed")
    leaf = slot_leaf(case.pool, slot_of(err))
    artifact = os.path.join(leaf, "build", "debug", "deps")
    cloned = [name for name in os.listdir(artifact) if name.startswith("libcfg_if-") and name.endswith(".rmeta")]
    expect(bool(cloned), "no cloned cfg-if artifact")
    info = os.lstat(os.path.join(artifact, cloned[0]))
    expect(info.st_nlink == 1, "the seeded file is a hard link to the donor")
    shutil.rmtree(slot_leaf(case.pool, donor_slot))
    code, stdout, stderr = case.command(recipient, ["run"], "recipient after donor deletion", env=case.env())
    expect(stdout.strip() == "1.0.0", f"the program printed {stdout.strip()!r}")
    print("case 07: the recipient ran after its donor slot was deleted", flush=True)


def case_08(proof: Proof) -> None:
    case = Case(proof, "08")
    _donor, _recipient, donor_slot, _ = cfg_donor_recipient(case)
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.4"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    recipient = case.checkout("changed", files, [registry("cfg-if", "1.0.4", CFG_IF_104)])
    code, _out, err = case.command(recipient, ["run"], "changed dependency", env=case.env())
    expect(not seeded(err), "a different version was offered")
    expect(skip_line(err).endswith("no eligible donor"), f"unexpected skip: {skip_line(err)}")
    print("case 08: a changed dependency offered nothing and built correctly", flush=True)


GATED_BUILD = r'''fn main() {
    println!("cargo:rerun-if-env-changed=OXIDE_SEED_GATE");
    if let Ok(gate) = std::env::var("OXIDE_SEED_GATE") {
        std::fs::write(format!("{gate}.started"), b"started").unwrap();
        let release = format!("{gate}.release");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !std::path::Path::new(&release).exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
'''


def case_09(proof: Proof) -> None:
    case = Case(proof, "09")
    gated = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
        ),
        "build.rs": GATED_BUILD,
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    plain = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
    donor = case.checkout("donor", gated, packages)
    recipient = case.checkout("recipient", plain, packages)
    gate = os.path.join(case.root, "donor-gate")
    env = case.env(extra={"OXIDE_SEED_GATE": gate})
    child = perm.start_child(proof, [proof.binary, "build"], donor, env)
    donor_slot = None
    try:
        wait_for(gate + ".started", "the gated donor build script")
        # The donor is a live writer holding its lease, so call 1 cannot offer
        # it (the ledger state is the first candidate filter). The recipient
        # must not wait for it and must build cold in its own fresh slot.
        code, _out, err = case.command(recipient, ["build"], "held donor", env=case.env())
        line = skip_line(err)
        expect("no eligible donor" in line or "donor busy" in line, f"unexpected skip: {line}")
        expect(not seeded(err), "a held donor was seeded")
        recipient_slot = slot_of(err)
        # No catalog can exist while the donor is still held: publishing is a
        # producing-lease step and happens before release.
        for name in os.listdir(seeds_dir(case.pool)) if os.path.isdir(seeds_dir(case.pool)) else []:
            expect(not name.endswith(".catalog.json"), f"a held donor published {name}")
    finally:
        stop_gate(gate)
        code, _out, donor_err = perm.finish_child(proof, child, "gated donor")
        expect(code == 0, "the gated donor failed")
    donor_slot = slot_of(donor_err)
    expect(donor_slot != recipient_slot, "the recipient reused the held donor's slot")
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the released donor published no catalog")
    print("case 09: a held donor was not offered and the recipient built cold", flush=True)


def wait_for(path: str, label: str) -> None:
    deadline = time.monotonic() + min(120.0, work_left())
    while not os.path.exists(path):
        if time.monotonic() >= deadline:
            fail(f"{label} did not appear")
        time.sleep(0.05)


def stop_gate(gate: str) -> None:
    with open(gate + ".release", "wb") as handle:
        handle.write(b"release")


def case_10(proof: Proof) -> None:
    case = Case(proof, "10")
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
    donor = case.checkout("donor", files, packages)
    recipient = case.checkout("recipient", files, packages)
    code, _out, err = case.command(
        donor, ["build"], "different-context donor", env=case.env(rustflags="--cfg other_context")
    )
    donor_slot = slot_of(err)
    expect(os.path.isfile(catalog_path(case.pool, donor_slot)), "the donor published no catalog")
    code, _out, err = case.command(recipient, ["build"], "different-context recipient", env=case.env())
    expect(not seeded(err), "a different producer context was seeded")
    expect(skip_line(err).endswith("no eligible donor"), f"unexpected skip: {skip_line(err)}")
    print("case 10: a differing RUSTFLAGS context was ineligible", flush=True)


def case_11(proof: Proof) -> None:
    case = Case(proof, "11")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    env = case.env(extra={"OXIDE_TEST_SEED_CRASH": "after-first-clone"})
    result = proof.run([proof.binary, "build"], cwd=recipient, env=env)
    stderr = result.stderr.decode(errors="replace")
    print(f"----- crash injection: exit {result.returncode} -----\n{stderr[-2000:]}", flush=True)
    expect(result.returncode != 0, "the crash injection exited successfully")
    rows = json.loads(ledger_bytes(case.pool).decode())["slots"]
    held = [row for row in rows if row["state"] == "held"]
    expect(len(held) == 1, f"expected one held crashed generation: {rows}")
    crashed_slot = str(held[0]["id"])
    expect(attempt_state(case.pool, crashed_slot) == "intent", "the crash did not leave an intent record")
    expect(leaf_exists(case.pool, crashed_slot), "the crashed generation has no slot leaf")
    shutil.rmtree(slot_leaf(case.pool, donor_slot))
    code, _out, err = case.command(recipient, ["build"], "crash recovery", env=case.env())
    recovered = slot_of(err)
    expect(recovered != crashed_slot, "the crashed generation was reused")
    expect(not leaf_exists(case.pool, crashed_slot), "the untrusted crashed slot was not evicted")
    expect(not seeded(err), "the recovery seeded without a donor")
    print("case 11: the crash left intent evidence and the next command evicted it", flush=True)


def leaf_exists(pool: str, slot: str) -> bool:
    return bool(glob.glob(os.path.join(payload_slots(pool), "*", f"*-{slot}")))


def case_12(proof: Proof) -> None:
    for limit in ("units", "files", "bytes"):
        case = Case(proof, "12")
        manifest = (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
        )
        files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
        packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
        donor = case.checkout("donor", files, packages)
        recipient = case.checkout("recipient", files, packages)
        code, _out, err = case.command(donor, ["build"], f"cap {limit} donor", env=case.env())
        donor_slot = slot_of(err)
        before = record_donor_figure(case.pool, donor_slot)
        env = case.env(extra={"OXIDE_TEST_SEED_LIMIT": limit})
        code, _out, err = case.command(recipient, ["build"], f"cap {limit} recipient", env=env)
        expect(not seeded(err), f"the {limit} cap did not skip")
        expect_no_seed_mutation(case.pool, donor_slot, before)
    case = Case(proof, "12")
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
    donor = case.checkout("donor", files, packages)
    recipient = case.checkout("recipient", files, packages)
    code, _out, err = case.command(donor, ["build"], "deadline donor", env=case.env())
    donor_slot = slot_of(err)
    before = record_donor_figure(case.pool, donor_slot)
    env = case.env(extra={"OXIDE_TEST_SEED_LIMIT": "deadline"})
    code, _out, err = case.command(recipient, ["build"], "deadline recipient", env=env)
    expect(not seeded(err) and "seeding skipped: deadline" in err, f"the work deadline did not skip: {err[-1000:]}")
    expect_no_seed_mutation(case.pool, donor_slot, before)
    print("case 12: each cap and the work deadline skipped without donor mutation", flush=True)


# ---------------------------------------------------------------------------
# Cases 13-24
# ---------------------------------------------------------------------------

COLLECTION = None


def git_app(name: str, url: str, rev: str) -> dict[str, str]:
    return {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            f'[dependencies]\n{name} = {{ git = "{url}", rev = "{rev}" }}\n'
        ),
        "src/main.rs": 'fn main() { println!("git identity"); }\n',
    }


def case_13(proof: Proof) -> None:
    case = Case(proof, "13")
    repository = proof.repositories["seedctl"]
    rev1, rev2 = repository["revisions"]
    files = seedctl_app(repository, rev1)
    packages = [git_package("seedctl", repository["url"], rev1)]
    donor = case.checkout("donor", files, packages)
    code, _out, err = case.command(donor, ["build"], "git identity donor", env=case.env())
    donor_slot = slot_of(err)
    checkout = find_checkout(proof.cargo_home, "seedrepo")
    stray = os.path.join(checkout, "STRAY-FROM-PROOF")
    with open(stray, "w", encoding="utf-8") as handle:
        handle.write("stray\n")
    try:
        dirty = case.checkout("dirty", files, packages)
        code, _out, err = case.command(dirty, ["build"], "dirty git checkout", env=case.env())
        expect(not seeded(err), "a dirty checkout was seeded")
        expect("no eligible donor" in skip_line(err), f"unexpected skip: {skip_line(err)}")
    finally:
        os.remove(stray)
    files2 = seedctl_app(repository, rev2)
    packages2 = [git_package("seedctl", repository["url"], rev2)]
    revision = case.checkout("revision", files2, packages2)
    code, _out, err = case.command(revision, ["build"], "differently-revisioned checkout", env=case.env())
    expect(not seeded(err), "a differently-revisioned checkout was seeded")
    expect("no eligible donor" in skip_line(err), f"unexpected skip: {skip_line(err)}")
    # A submodule (a gitlink in the checkout) is ineligible: the producing
    # epoch records no package, so no catalog exists to donate and the
    # recipient builds cold.
    submodule = proof.repositories["subrepo"]
    sub_files = git_app("subcrate", submodule["url"], submodule["revisions"][0])
    sub_packages = [git_package("subcrate", submodule["url"], submodule["revisions"][0])]
    sub_donor = case.checkout("submodule-donor", sub_files, sub_packages)
    code, _out, err = case.command(sub_donor, ["build"], "submodule donor", env=case.env())
    sub_slot = slot_of(err)
    expect(
        not os.path.isfile(catalog_path(case.pool, sub_slot)),
        "a submodule checkout produced a donor catalog",
    )
    sub_recipient = case.checkout("submodule-recipient", sub_files, sub_packages)
    code, _out, err = case.command(sub_recipient, ["build"], "submodule recipient", env=case.env())
    expect(not seeded(err), "a submodule checkout was seeded")
    # The same literal crate name from a registry and a Git source is
    # ambiguous: the lock has no eligible package, so no catalog is produced.
    collision = proof.repositories["cfgifgit"]
    rev3 = collision["revisions"][0]
    collision_files = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
            f'cfgifgit = {{ package = "cfg-if", git = "{collision["url"]}", rev = "{rev3}" }}\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    collision_donor = case.checkout("collision-donor", collision_files)
    code, _out, err = case.command(collision_donor, ["build"], "collision donor", env=case.env())
    collision_slot = slot_of(err)
    expect(
        not os.path.isfile(catalog_path(case.pool, collision_slot)),
        "a same-name registry/Git collision produced a donor catalog",
    )
    collision_recipient = case.checkout("collision-recipient", collision_files)
    code, _out, err = case.command(
        collision_recipient, ["build"], "collision recipient", env=case.env()
    )
    line = skip_line(err)
    expect(not seeded(err), "a same-name collision was seeded")
    expect("ambiguous name" in line, f"the collision was not named: {line}")
    print("case 13: dirty, re-revisioned, submodule and colliding checkouts are ineligible", flush=True)


def case_14(proof: Proof) -> None:
    case = Case(proof, "14")
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
    donor = case.checkout("donor", files, packages)
    case.command(donor, ["build"], "unsupported-Cargo donor", env=case.env())
    wrapper = os.path.join(case.root, "fake-bin")
    os.makedirs(wrapper)
    real = shutil.which("cargo")
    expect(real is not None, "cargo is not on PATH")
    script = os.path.join(wrapper, "cargo")
    with open(script, "w", encoding="utf-8") as handle:
        handle.write(
            "#!/bin/sh\n"
            'if [ "$1" = "--version" ]; then\n'
            '  echo "cargo 1.90.0 (000000000 2020-01-01)"\n'
            "  exit 0\n"
            "fi\n"
            f'exec "{real}" "$@"\n'
        )
    os.chmod(script, 0o755)
    recipient = case.checkout("recipient", files, packages)
    env = case.env(extra={"PATH": wrapper + ":" + os.environ.get("PATH", "")})
    code, _out, err = case.command(recipient, ["build"], "unsupported Cargo", env=env)
    expect("seeding skipped: unsupported Cargo" in err, f"no unsupported-Cargo skip: {err[-1200:]}")
    expect(os.path.isfile(os.path.join(recipient, "target", "debug", "app")), "Cargo did not build normally")
    print("case 14: an unsupported Cargo identity skipped seeding and still built", flush=True)


def case_15(proof: Proof) -> None:
    case = Case(proof, "15")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    before = record_donor_figure(case.pool, donor_slot)
    code, _out, err = case.command(recipient, ["build"], "no-donor-writes", env=case.env())
    expect(seeded(err), "the recipient did not seed")
    expect(slot_tree(case.pool, donor_slot) == before["tree"], "the donor tree changed")
    expect(read_bytes(catalog_path(case.pool, donor_slot)) == before["catalog"], "the donor catalog changed")
    expect(record_figure(case.pool, donor_slot) == before["figure"], "the donor's ledger row changed")
    print("case 15: the donor tree, catalog and ledger row are unchanged", flush=True)


def removable_seeds_dir(pool: str) -> str:
    return os.path.join(pool, "oxide-managed", "pool", "oxide-managed", "seeds")


def case_16(proof: Proof) -> None:
    global COLLECTION
    case = Case(proof, "16")
    name = "o169-" + secrets.token_hex(2)
    expect(not os.path.lexists(f"/Volumes/{name}"), f"/Volumes/{name} already exists; never touched")
    image = os.path.join(proof.root, "volume.sparseimage")
    mount = perm.attach_image(proof, proof.root, name, owners=True)
    expect(mount == f"/Volumes/{name}", f"the owned image mounted at {mount}")
    perm.owners_honored(proof, mount, "case 16")
    code, out, _err = case.command(".", ["pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"], "register removable")
    expect("registered removable pool" in out, f"registration failed: {out}")
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    packages = [registry("cfg-if", "1.0.0", CFG_IF_100)]
    donor = case.checkout("donor", files, packages)
    recipient = case.checkout("recipient", files, packages)
    code, _out, err = case.command(donor, ["build"], "removable donor", env=case.env())
    expect("apfs-volume-v1" in summary(err), f"the removable pool was not selected: {summary(err)}")
    donor_slot = slot_of(err)
    expect(
        os.path.isfile(os.path.join(removable_seeds_dir(case.pool), f"{donor_slot}.catalog.json")),
        "the removable donor published no catalog",
    )
    code, _out, err = case.command(recipient, ["build"], "removable recipient", env=case.env())
    expect(seeded(err), f"the removable recipient did not seed: {err[-2000:]}")
    expect("apfs-volume-v1" in summary(err), f"the recipient did not use the removable pool: {summary(err)}")
    recipient_slot = slot_of(err)
    leaf = runner.labelled_slot(os.path.join(mount, "oxide-pool", "slots"), int(recipient_slot))
    expect(os.path.realpath(leaf).startswith(os.path.realpath(mount) + os.sep), f"{leaf} is not on {mount}")
    device = os.stat(mount).st_dev
    artifacts = os.path.join(leaf, "build", "debug", "deps")
    cloned = [name for name in os.listdir(artifacts) if name.startswith("libcfg_if-")]
    expect(bool(cloned), f"no seeded artifacts on the image: {os.listdir(artifacts)}")
    expect(os.stat(os.path.join(artifacts, cloned[0])).st_dev == device, "the seeded bytes are not on the image")
    code, stdout, stderr = case.command(recipient, ["run"], "removable recipient run", env=case.env())
    expect(stdout.strip() == "1.0.0", f"the removable program printed {stdout.strip()!r}")
    perm.run_ordinary_detach(proof, COLLECTION)
    expect(not os.path.ismount(mount), f"{mount} is still mounted after the ordinary detach")
    if COLLECTION is not None:
        COLLECTION.mount_present = False
    proof.mount = None
    os.remove(image)
    print(f"case 16: removable seeding on {mount}; ordinary detach; backing removed", flush=True)


def registry_paths(cargo_home: str, name: str, version: str) -> tuple[str, str]:
    cache_root = os.path.join(cargo_home, "registry", "cache")
    src_root = os.path.join(cargo_home, "registry", "src")
    namespaces = [
        entry
        for entry in os.listdir(cache_root)
        if os.path.isfile(os.path.join(cache_root, entry, f"{name}-{version}.crate"))
    ]
    expect(len(namespaces) == 1, f"expected one registry cache namespace, got {namespaces}")
    namespace = namespaces[0]
    return (
        os.path.join(cache_root, namespace, f"{name}-{version}.crate"),
        os.path.join(src_root, namespace, f"{name}-{version}"),
    )


CFG_APP_FILES = {
    "Cargo.toml": (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    ),
    "src/main.rs": 'fn main() { println!("ok"); }\n',
}
CFG_APP_LOCK = [registry("cfg-if", "1.0.0", CFG_IF_100)]


def case_17(proof: Proof) -> None:
    case = Case(proof, "17")
    cfg_donor_recipient(case)
    archive, tree = registry_paths(proof.cargo_home, "cfg-if", "1.0.0")
    original = read_bytes(archive)

    def cold_recipient(name: str, label: str) -> None:
        recipient = case.checkout(name, CFG_APP_FILES, CFG_APP_LOCK)
        code, _out, err = case.command(recipient, ["build"], label, env=case.env())
        expect(not seeded(err), f"{label} was seeded")

    try:
        with open(archive, "wb") as handle:
            handle.write(b"tampered archive bytes\n")
        cold_recipient("wrong-archive", "wrong cached archive")
    finally:
        with open(archive, "wb") as handle:
            handle.write(original)
    extra = os.path.join(tree, "PROOF-EXTRA")
    with open(extra, "w", encoding="utf-8") as handle:
        handle.write("extra\n")
    try:
        cold_recipient("extra", "extra tree file")
    finally:
        os.remove(extra)
    link = os.path.join(tree, "PROOF-LINK")
    os.symlink("/etc/hosts", link)
    try:
        cold_recipient("link", "tree symlink")
    finally:
        os.remove(link)
    lib = os.path.join(tree, "src", "lib.rs")
    original_lib = read_bytes(lib)
    try:
        with open(lib, "ab") as handle:
            handle.write(b"\n// altered by the cold-slot seeding proof\n")
        cold_recipient("altered", "altered tree file")
    finally:
        with open(lib, "wb") as handle:
            handle.write(original_lib)
    try:
        os.rename(lib, lib + ".proof-missing")
        recipient = case.checkout("missing", CFG_APP_FILES, CFG_APP_LOCK)
        code, _out, err = case.command(
            recipient, ["build"], "missing tree file", env=case.env(), allow_any_exit=True
        )
        # A missing source file can fail Cargo's own build; the claim under
        # test is only that the product never seeds a modified tree.
        expect(not seeded(err), "a missing tree file was seeded")
    finally:
        os.rename(lib + ".proof-missing", lib)
    print("case 17: each registry tamper control built cold and never seeded", flush=True)


def case_18(proof: Proof) -> None:
    case = Case(proof, "18")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    code, _out, err = case.command(recipient, ["build"], "seeded recipient", env=case.env())
    expect(seeded(err), "the recipient did not seed")
    seeded_slot = slot_of(err)
    expect(attempt_state(case.pool, seeded_slot) == "seeded", "the attempt is not seeded")
    expect(not os.path.isfile(catalog_path(case.pool, seeded_slot)), "a seeded recipient published a catalog")
    shutil.rmtree(slot_leaf(case.pool, donor_slot))
    manifest = (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    )
    files = {"Cargo.toml": manifest, "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n'}
    third = case.checkout("third", files, [registry("cfg-if", "1.0.0", CFG_IF_100)])
    code, _out, err = case.command(third, ["build"], "third cold slot", env=case.env())
    expect(not seeded(err), "a seeded recipient donated a catalog")
    expect("no eligible donor" in skip_line(err), f"unexpected skip: {skip_line(err)}")
    print("case 18: a seeded recipient never donated", flush=True)


def case_19(proof: Proof) -> None:
    case = Case(proof, "19")
    _donor, recipient, _slot, _ = cfg_donor_recipient(case)
    readings = os.path.join(case.root, "readings")
    with open(readings, "w", encoding="utf-8") as handle:
        handle.write(str(22 * 1024 * 1024 * 1024 - 4096))
    env = case.env(extra={"OXIDE_TEST_WATCHDOG_READINGS": readings})
    code, _out, err = case.command(
        recipient, ["build"], "reserve start gate", env=env, allow_any_exit=True
    )
    expect(code != 0, "a below-trigger reading did not refuse")
    expect("seed reserve stop" in err, f"no reserve-stop diagnostic: {err[-1500:]}")
    expect("Compiling" not in err, "Cargo ran despite the reserve refusal")
    expect(not os.path.isdir(os.path.join(recipient, "target", "debug")), "a child ran")
    control = Case(proof, "19")
    files = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    no_donor = control.checkout("no-donor", files, [registry("cfg-if", "1.0.0", CFG_IF_100)])
    env = control.env(extra={"OXIDE_TEST_WATCHDOG_READINGS": readings})
    code, _out, err = control.command(no_donor, ["build"], "non-seeding control", env=env)
    expect(not seeded(err) and "seed reserve stop" not in err, "the non-seeding control sampled the gate")
    breached = Case(proof, "19")
    _d, recipient2, breached_donor, _ = cfg_donor_recipient(breached)
    env = breached.env(extra={"OXIDE_TEST_SEED_BREACH": "after-clone"})
    code, _out, err = breached.command(
        recipient2, ["build"], "mid-seed breach", env=env, allow_any_exit=True
    )
    expect(code != 0, "a mid-seed breach did not refuse")
    rows = json.loads(ledger_bytes(breached.pool).decode())["slots"]
    untrusted = [
        row
        for row in rows
        if row["state"] == "idle" and str(row["id"]) != str(breached_donor)
    ]
    expect(bool(untrusted), "the breached generation was not retained")
    retained = str(untrusted[0]["id"])
    expect(attempt_state(breached.pool, retained) == "reset", "the breach was not recorded as reset")
    expect(leaf_exists(breached.pool, retained), "the breached generation has no retained leaf")
    print("case 19: the start gate refused before any child; the breach retained and refused", flush=True)


def case_20(proof: Proof) -> None:
    case = Case(proof, "20")
    _donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    env = case.env(extra={"OXIDE_TEST_SEED_BREACH": "after-clone"})
    code, _out, _err = case.command(
        recipient, ["build"], "partial-trust injection", env=env, allow_any_exit=True
    )
    expect(code != 0, "the partial-trust injection did not refuse")
    rows = json.loads(ledger_bytes(case.pool).decode())["slots"]
    partial = [
        str(row["id"])
        for row in rows
        if row["state"] == "idle" and str(row["id"]) != str(donor_slot)
    ]
    expect(len(partial) == 1, f"expected one retained partial generation: {rows}")
    shutil.rmtree(slot_leaf(case.pool, donor_slot))
    code, _out, err = case.command(recipient, ["build"], "partial-trust recovery", env=case.env())
    reused = slot_of(err)
    expect(reused != partial[0], "the partial generation was reused as warm")
    expect(not leaf_exists(case.pool, partial[0]), "the partial generation was not evicted")
    expect(not seeded(err), "the recovery seeded without a donor")
    print("case 20: a partial recipient was never reused", flush=True)


def case_21(proof: Proof) -> None:
    case = Case(proof, "21")
    workspace = {
        "Cargo.toml": '[workspace]\nmembers = ["app", "local-cfg"]\nresolver = "2"\n',
    }
    app = {
        "Cargo.toml": (
            '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[dependencies]\ncfg-if = "=1.0.0"\n'
        ),
        "src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    }
    local = {
        "Cargo.toml": (
            '[package]\nname = "local-cfg"\nversion = "0.1.0"\nedition = "2021"\n\n'
            '[lib]\nname = "cfg_if"\n'
        ),
        "src/lib.rs": "pub fn value() -> u32 { 9 }\n",
    }
    packages = [
        {"name": "app", "version": "0.1.0", "dependencies": ["cfg-if"]},
        registry("cfg-if", "1.0.0", CFG_IF_100),
        {"name": "local-cfg", "version": "0.1.0"},
    ]
    root = case.checkout("workspace", workspace)
    write_tree(os.path.join(root, "app"), app)
    write_tree(os.path.join(root, "local-cfg"), local)
    with open(os.path.join(root, "Cargo.lock"), "w", encoding="utf-8") as handle:
        handle.write(lock_text(packages))
    code, _out, err = case.command(root, ["build", "--workspace"], "collision donor", env=case.env())
    donor_slot = slot_of(err)
    catalog = read_json(catalog_path(case.pool, donor_slot))
    offered = sorted(package["name"] for package in catalog["packages"])
    expect(offered == ["cfg-if"], f"the plan offered {offered}, not only the registry cfg-if")
    recipient = case.checkout("recipient", workspace)
    write_tree(os.path.join(recipient, "app"), app)
    write_tree(os.path.join(recipient, "local-cfg"), local)
    with open(os.path.join(recipient, "Cargo.lock"), "w", encoding="utf-8") as handle:
        handle.write(lock_text(packages))
    code, stdout, err = case.command(recipient, ["run"], "collision recipient", env=case.env())
    expect(seeded(err), "the collision fixture did not seed")
    expect(stdout.strip() == "1.0.0", f"the recipient printed {stdout.strip()!r}")
    # The seedctl catalog offers exactly one build-script compile unit.
    other = Case(proof, "21")
    _donor, _recipient, slot, _out = seedctl_fixture(other)
    catalog = read_json(catalog_path(other.pool, slot))
    units = [unit["role"] for package in catalog["packages"] for unit in package["units"]]
    expect(units == ["build-script-compile"], f"seedctl was offered from {units}")
    print("case 21: only verified registry artifacts were offered; seedctl is per-unit", flush=True)


def case_22(proof: Proof) -> None:
    case = Case(proof, "22")
    donor, recipient, donor_slot, _ = cfg_donor_recipient(case)
    catalog = read_json(catalog_path(case.pool, donor_slot))
    project = os.path.join(case.pool, "oxide-managed", "affinity", str(catalog["record_id"]))
    pause_gate = os.path.join(case.root, "pause-gate")
    release_gate = os.path.join(case.root, "competitor-release")
    before_tree = slot_tree(case.pool, donor_slot)
    before_catalog = read_bytes(catalog_path(case.pool, donor_slot))
    env = case.env(extra={"OXIDE_TEST_SEED_PAUSE": f"between-calls:{pause_gate}"})
    seeded_child = perm.start_child(proof, [proof.binary, "build"], recipient, env)
    competitor = None
    try:
        marker = wait_for_glob(pause_gate + ".between-calls-*", "the between-calls pause")
        print(f"competing lease armed at {marker}", flush=True)
        competitor_env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": proof.home,
        }
        competitor = perm.start_child(
            proof,
            [proof.lease_helper, case.pool, project, donor, release_gate],
            donor,
            competitor_env,
        )
        wait_for(release_gate + ".leased", "the competing library lease")
        with open(pause_gate, "wb") as release:
            release.write(b"release")
        code, _out, err = perm.finish_child(proof, seeded_child, "competing custody")
        expect(code == 0, f"the competing command failed: {err[-1500:]}")
        line = skip_line(err)
        expect("donor busy" in line, f"the competing lease was not a donor-busy skip: {line}")
        expect(not seeded(err), "a competing lease did not stop the seed")
        expect(slot_tree(case.pool, donor_slot) == before_tree, "the donor tree changed")
        expect(read_bytes(catalog_path(case.pool, donor_slot)) == before_catalog, "the donor catalog changed")
    finally:
        with open(release_gate, "wb") as release:
            release.write(b"release")
        if competitor is not None:
            code, _out, err = perm.finish_child(proof, competitor, "competing lease")
            expect(code == 0, f"the competing lease failed: {err[-1500:]}")
    # Non-vacuous control: the same fixture without the interleaving seeds.
    control = Case(proof, "22")
    _d, recipient2, _s, _ = cfg_donor_recipient(control)
    code, _out, err = control.command(recipient2, ["build"], "uninterleaved control", env=control.env())
    expect(seeded(err), "the uninterrupted control did not seed")
    print("case 22: a real competing lease was a donor-busy skip with no block", flush=True)


def wait_for_glob(pattern: str, label: str) -> str:
    deadline = time.monotonic() + min(120.0, work_left())
    while True:
        matches = sorted(glob.glob(pattern))
        if matches:
            return matches[0]
        if time.monotonic() >= deadline:
            fail(f"{label} did not appear")
        time.sleep(0.05)


WORKSPACE_23 = {
    "Cargo.toml": '[workspace]\nmembers = ["app", "foo-bar", "foo_bar", "baz-qux"]\nresolver = "2"\n',
    "app/Cargo.toml": (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\nbuild = "build.rs"\n\n'
        '[dependencies]\ncfg-if = "=1.0.0"\n'
    ),
    "app/build.rs": r'''fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Ok(out) = std::env::var("OUT_DIR") else { return };
    let planted = std::path::Path::new(&out).join("../../../.fingerprint/baz-qux-aaaaaaaaaaaaaaaa");
    let _ = std::fs::create_dir_all(&planted);
    let _ = std::fs::write(planted.join("weird"), b"planted");
}
''',
    "app/src/main.rs": 'fn main() { println!("{}", cfg_if::VERSION); }\n',
    "foo-bar/Cargo.toml": '[package]\nname = "foo-bar"\nversion = "0.1.0"\nedition = "2021"\n',
    "foo-bar/src/lib.rs": "pub fn v() -> u32 { 1 }\n",
    "foo_bar/Cargo.toml": '[package]\nname = "foo_bar"\nversion = "0.1.0"\nedition = "2021"\n',
    "foo_bar/src/lib.rs": "pub fn v() -> u32 { 2 }\n",
    "baz-qux/Cargo.toml": '[package]\nname = "baz-qux"\nversion = "0.1.0"\nedition = "2021"\n',
    "baz-qux/src/lib.rs": "pub fn v() -> u32 { 3 }\n",
}
WORKSPACE_23_LOCK = [
    {"name": "app", "version": "0.1.0", "dependencies": ["cfg-if"]},
    registry("cfg-if", "1.0.0", CFG_IF_100),
    {"name": "foo-bar", "version": "0.1.0"},
    {"name": "foo_bar", "version": "0.1.0"},
    {"name": "baz-qux", "version": "0.1.0"},
]


def write_workspace_23(case: Case, name: str) -> str:
    root = os.path.join(case.root, name)
    os.makedirs(root)
    write_tree(root, WORKSPACE_23)
    with open(os.path.join(root, "Cargo.lock"), "w", encoding="utf-8") as handle:
        handle.write(lock_text(WORKSPACE_23_LOCK))
    return root


def case_23(proof: Proof) -> None:
    case = Case(proof, "23")
    donor = write_workspace_23(case, "donor")
    code, _out, err = case.command(donor, ["build", "--workspace"], "ambiguous donor", env=case.env())
    donor_slot = slot_of(err)
    catalog = read_json(catalog_path(case.pool, donor_slot))
    counts = [
        (rejection["reason"], rejection["units"]) for rejection in catalog.get("rejected", [])
    ]
    ambiguous = [entry for entry in counts if entry[0] == "ambiguous_name" and entry[1] == 1]
    expect(len(ambiguous) == 2, f"no two ambiguous unit counts: {counts}")
    expect(any(units is None for _reason, units in counts), f"no unknown rejection: {counts}")
    recipient = write_workspace_23(case, "recipient")
    code, _out, err = case.command(recipient, ["build", "--workspace"], "ambiguous recipient", env=case.env())
    expect(seeded(err), "the ambiguous fixture did not seed")
    line = summary(err)
    expect("ambiguous name (2)" in line, f"the summary does not name the ambiguous units: {line}")
    expect("unknown (1)" in line, f"the summary does not distinguish unknown from zero: {line}")
    code, out, _err = case.command(recipient, ["status"], "ambiguous status", env=case.env())
    expect("ambiguous name (2)" in out and "unknown (1)" in out, f"status did not render the buckets: {out[-1500:]}")
    last = last_seed(case.pool)
    expect(last is not None and any(bucket["units"] == 2 for bucket in last.get("buckets", [])), f"last.json buckets: {last}")
    # A clean donor renders no suffix and an empty bucket set.
    control = Case(proof, "23")
    _donor, recipient2, _slot, _ = cfg_donor_recipient(control)
    code, _out, err = control.command(recipient2, ["build"], "clean donor control", env=control.env())
    expect(seeded(err), "the clean control did not seed")
    expect("skipped" not in summary(err), f"the clean control rendered a suffix: {summary(err)}")
    last = last_seed(control.pool)
    expect(last is not None and not last.get("buckets") and not last.get("unknown"), f"clean buckets: {last}")
    print("case 23: ambiguous (2) and unknown (1) rendered; the clean donor rendered nothing", flush=True)


def case_24(proof: Proof) -> None:
    case = Case(proof, "24")
    _donor, recipient, _slot, _ = cfg_donor_recipient(case)
    code, _out, err = case.command(recipient, ["build"], "complete walk", env=case.env())
    expect(seeded(err), "the complete walk did not publish")
    slot = slot_of(err)
    expect(attempt_state(case.pool, slot) == "seeded", "the complete walk's attempt is not seeded")
    expect(last_seed(case.pool) is not None, "the complete walk wrote no last-seed sidecar")
    gaps = {
        "foreign-owner": "ownership",
        "unreadable": "unreadable",
        "device-boundary": "device boundary",
        "entry-limit": "entry limit",
        "failed": "walk failure",
    }
    for gap, label in gaps.items():
        other = Case(proof, "24")
        _d, recipient2, _donor_slot, _ = cfg_donor_recipient(other, prefix=f"{gap}-")
        env = other.env(extra={"OXIDE_TEST_SEED_WALK": gap})
        code, _out, err = other.command(recipient2, ["build"], f"walk {label}", env=env)
        expect(not seeded(err), f"the {label} injection published a seed")
        expect("seeding skipped: recipient walk incomplete" in err, f"no abandon line for {label}: {err[-1500:]}")
        expect(last_seed(other.pool) is None, f"the {label} injection wrote a last-seed sidecar")
        rows = json.loads(ledger_bytes(other.pool).decode())["slots"]
        abandoned = [
            str(row["id"])
            for row in rows
            if row["state"] == "idle" and str(row["id"]) != str(_donor_slot)
        ]
        expect(len(abandoned) >= 1, f"the {label} injection retained no untrusted generation")
        expect(any(attempt_state(other.pool, slot) == "reset" for slot in abandoned), f"no reset attempt for {label}")
        expect(slot_of(err) not in abandoned, f"the {label} injection built in the untrusted slot")
    print("case 24: the complete walk published; every injected walk gap abandoned and built cold", flush=True)


# ---------------------------------------------------------------------------
# Dispatch, receipt and pure apparatus
# ---------------------------------------------------------------------------

BODIES = {
    "01": case_01,
    "02": case_02,
    "03": case_03,
    "04": case_04,
    "05": case_05,
    "06": case_06,
    "07": case_07,
    "08": case_08,
    "09": case_09,
    "10": case_10,
    "11": case_11,
    "12": case_12,
    "13": case_13,
    "14": case_14,
    "15": case_15,
    "16": case_16,
    "17": case_17,
    "18": case_18,
    "19": case_19,
    "20": case_20,
    "21": case_21,
    "22": case_22,
    "23": case_23,
    "24": case_24,
}


def plan() -> None:
    """The effect-free table, allocation and premise dispositions."""
    print("cold-slot seeding (oxide#137 §16, oxide#169): 24 fixed native cases")
    print(
        "supervisor: python3 scripts/bounded-test.py --root-parent /private/tmp "
        "--limit 300 --scratch-budget-mib 512 -- python3 scripts/test-cold-slot-seeding.py"
    )
    print("limits: 300 s wall clock, 512 MiB monitored scratch, one owned 128 MiB APFS image")
    print("allocation (source-derived, UNMEASURED): staging 40-60 s, 60-85 MiB; cases 01-24 about 230-300 MiB peak; image phase <= 128 MiB")
    print("phases:")
    for phase_text in PHASES:
        print("  " + phase_text)
    print("cases:")
    for case, requires, text in CASES:
        requirement = f" requires {', '.join(requires)}" if requires else ""
        print(f"  {case}{requirement}: {text}")
    print("premises: private HOME/rustup; OXIDE_TEST_HOME_FROM_ENV; env isolation; local target; owned image -owners on; frozen RUSTFLAGS; supervisor temp; macOS mktemp")
    print("no effect: this mode starts no child, creates no pool and reads no fixture")


def _bound_and_loaded(node: ast.AST) -> tuple[set[str], set[str]]:
    bound: set[str] = set()
    loaded: set[str] = set()
    for child in ast.walk(node):
        if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)):
            bound.add(child.name)
            arguments = [*child.args.posonlyargs, *child.args.args, *child.args.kwonlyargs]
            bound.update(argument.arg for argument in arguments)
            if child.args.vararg is not None:
                bound.add(child.args.vararg.arg)
            if child.args.kwarg is not None:
                bound.add(child.args.kwarg.arg)
        elif isinstance(child, ast.Lambda):
            arguments = [*child.args.posonlyargs, *child.args.args, *child.args.kwonlyargs]
            bound.update(argument.arg for argument in arguments)
            if child.args.vararg is not None:
                bound.add(child.args.vararg.arg)
            if child.args.kwarg is not None:
                bound.add(child.args.kwarg.arg)
        elif isinstance(child, ast.Name):
            target = bound if isinstance(child.ctx, (ast.Store, ast.Del)) else loaded
            target.add(child.id)
        elif isinstance(child, ast.alias):
            bound.add((child.asname or child.name).split(".")[0])
        elif isinstance(child, ast.ExceptHandler) and child.name:
            bound.add(child.name)
    return bound, loaded


def undefined_names(body) -> list[str]:
    """Names a case body loads without binding, a module global or a builtin.

    This is the static half of the pure apparatus check: it catches a body that
    would raise at its first statement (case 09's unbound donor did) without
    starting any fixture.
    """
    tree = ast.parse(textwrap.dedent(inspect.getsource(body)))
    bound, loaded = _bound_and_loaded(tree.body[0])
    known = bound | set(globals()) | set(dir(builtins))
    return sorted(name for name in loaded if name not in known)


def missing_module_helpers() -> list[str]:
    """Attributes this script reads from its imported helper modules that the
    module does not define. The staging helper is `layout.stage_at`, not
    `perm.stage_at`; the pure self-test must catch that without a native run.

    Only the four modules bound at module level are checked, so instance
    attributes (`proof.binary`, `case.pool`) and local names are untouched.
    """
    with open(__file__, encoding="utf-8") as handle:
        tree = ast.parse(handle.read())
    modules = {
        name: value
        for name in ("perm", "layout", "runner", "offline_seed")
        if isinstance(value := globals().get(name), types.ModuleType)
    }
    missing = []
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Attribute)
            and isinstance(node.value, ast.Name)
            and node.value.id in modules
            and not hasattr(modules[node.value.id], node.attr)
        ):
            missing.append(f"{node.value.id}.{node.attr}")
    return sorted(set(missing))


def unused_module_names() -> list[str]:
    """Module-level names that are never loaded: an unused fixture constant is
    a silent proof-source defect."""
    with open(__file__, encoding="utf-8") as handle:
        tree = ast.parse(handle.read())
    assigned: list[str] = []
    for node in tree.body:
        if isinstance(node, ast.Assign):
            assigned.extend(
                target.id for target in node.targets if isinstance(target, ast.Name)
            )
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            assigned.append(node.target.id)
    loaded = {
        child.id
        for child in ast.walk(tree)
        if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Load)
    }
    return sorted(name for name in assigned if name not in loaded)


def self_check() -> None:
    ids = [case for case, _requires, _text in CASES]
    expect(ids == [f"{index:02d}" for index in range(1, 25)], f"the case table drifted: {ids}")
    expect(sorted(BODIES) == sorted(ids), "the case table and the dispatcher disagree")
    for name, body in BODIES.items():
        expect(callable(body), f"case {name} has no body")
        missing = undefined_names(body)
        expect(not missing, f"case {name} loads unbound name(s): {missing}")
    unused = unused_module_names()
    expect(not unused, f"unused module-level fixture name(s): {unused}")
    missing = missing_module_helpers()
    expect(not missing, f"missing imported helper(s): {missing}")
    expect(len(set(ids)) == len(ids), "duplicate case id")
    for relative in ("scripts/offline_seed.py", "scripts/bounded-test.py"):
        path = os.path.join(os.path.dirname(HERE), relative)
        expect(os.path.isfile(path), f"{relative} is absent")
    lock = lock_text(CFG_APP_LOCK)
    budget = offline_seed.Budget(offline_seed.Limits())
    packages, git_packages = offline_seed.parse_lock_packages(lock, budget)
    expect([package["name"] for package in packages] == ["cfg-if"], f"lock fixture drifted: {packages}")
    expect(not git_packages, "the registry-only fixture names a Git source")
    print(f"cold-slot-seeding self-check: {len(ids)} cases, script {sha256_file(__file__)[:16]}")
    print(f"  offline_seed.py {sha256_file(os.path.join(HERE, 'offline_seed.py'))[:16]}")
    print(f"  bounded-test.py {sha256_file(os.path.join(HERE, 'bounded-test.py'))[:16]}")


def self_test() -> None:
    """The pure apparatus: collection semantics, receipt shape and the oracles.

    It starts no child, writes no pool and touches no image; every synthetic
    file is under one temporary directory that it removes.
    """
    self_check()
    base = tempfile.mkdtemp(prefix="cold-seed-selftest.", dir=tempfile.gettempdir())
    try:
        # The diagnostic collection's taint/skip/abort precedence.
        table = [
            ("a", (), lambda: None),
            ("b", ("a",), lambda: fail("recoverable oracle failure")),
            ("c", ("b",), lambda: None),
            ("d", (), lambda: (_ for _ in ()).throw(perm.ProofAbort("unknown custody"))),
            ("e", (), lambda: None),
        ]
        record = perm.collect_cases(
            table, diagnostic=True, work_left=lambda: 100, note=lambda _text: None
        )
        results = {name: row["result"] for name, row in record.outcomes.items()}
        expect(
            results == {"a": "completed", "b": "failed", "c": "skipped", "d": "aborted", "e": "unexecuted"},
            f"collection precedence drifted: {results}",
        )
        expect(record.aborted is not None and record.failures, "the abort or failure was not latched")
        counted = [(name, (), body) for name, body in (("x", lambda: fail("boom")),)]
        try:
            perm.collect_cases(counted, diagnostic=False, work_left=lambda: 100, note=lambda _text: None)
        except RuntimeError:
            pass
        else:
            fail("a counted collection did not fail fast")
        # A deadline before the first case aborts, never runs on.
        expired = perm.collect_cases(
            [("a", (), lambda: None)],
            diagnostic=True,
            work_left=lambda: 0,
            note=lambda _text: None,
        )
        expect(expired.outcomes["a"]["result"] == "aborted", "an exhausted deadline did not abort")

        # The seed-side oracles, on synthetic bytes.
        expect(seeded("oxide: seeded 2 units (0.1 MiB) from slot 3\n"), "seeded() drifted")
        expect(seeded_units("oxide: seeded 2 units (0.1 MiB) from slot 3") == 2, "seeded_units() drifted")
        expect(
            skip_line("x\noxide: seeding skipped: donor busy\n").endswith("donor busy"),
            "skip_line() drifted",
        )
        messages = "\n".join(
            json.dumps(message)
            for message in (
                {"reason": "compiler-artifact", "package_id": "registry#cfg-if@1.0.0", "fresh": True},
                {"reason": "build-script-executed", "package_id": "path+file:///app", "out_dir": "/out"},
            )
        )
        expect(artifact_fresh(messages, "cfg-if") is True, "artifact_fresh() drifted")
        expect(script_execution(messages, "app")["out_dir"] == "/out", "script_execution() drifted")

        # Attempt, ledger row and slot-leaf oracles on one synthetic pool.
        pool = os.path.join(base, "pool")
        seeds = os.path.join(pool, "oxide-managed", "seeds")
        os.makedirs(seeds)
        with open(os.path.join(pool, "oxide-managed", "ledger.json"), "w", encoding="utf-8") as handle:
            json.dump({"slots": [{"id": 7, "rank": 3, "last_used": 9, "recorded": 11, "state": "idle", "last_lease_id": "a"}]}, handle)
        with open(catalog_path(pool, "7"), "w", encoding="utf-8") as handle:
            json.dump({"packages": []}, handle)
        with open(attempt_path(pool, "7"), "w", encoding="utf-8") as handle:
            json.dump({"state": "reset"}, handle)
        expect(record_figure(pool, "7")["recorded"] == 11, "record_figure() drifted")
        expect(attempt_state(pool, "7") == "reset", "attempt_state() drifted")
        leaf_root = os.path.join(pool, "payload", "slots", "label", "quick-7")
        os.makedirs(os.path.join(leaf_root, "build"))
        with open(os.path.join(leaf_root, "build", "file"), "w", encoding="utf-8") as handle:
            handle.write("x")
        expect(slot_leaf(pool, "7") == leaf_root, "slot_leaf() drifted")
        expect(leaf_exists(pool, "7") and not leaf_exists(pool, "8"), "leaf_exists() drifted")
        expect("file" in slot_tree(pool, "7"), "slot_tree() drifted")

        flags = {"CARGO_ENCODED_RUSTFLAGS": "x"}
        freeze_compiler_flags(flags)
        expect("CARGO_ENCODED_RUSTFLAGS" not in flags and flags["RUSTFLAGS"] == FROZEN_RUSTFLAGS, "freeze_compiler_flags() drifted")

        # The staged-library locator: both supported layouts, and zero or
        # ambiguous matches refuse. Synthetic stage roots only.
        def rlib(deps: str, name: str = "liboxide_storage-abc.rlib") -> None:
            os.makedirs(deps, exist_ok=True)
            with open(os.path.join(deps, name), "wb") as handle:
                handle.write(b"synthetic rlib\n")

        build_layout = os.path.join(base, "stage-build")
        rlib(os.path.join(build_layout, "build", "debug", "deps"))
        expect(
            staged_library(build_layout)[0].endswith(os.path.join("build", "debug", "deps")),
            "the build-dir layout was not found",
        )
        target_layout = os.path.join(base, "stage-target")
        rlib(os.path.join(target_layout, "target", "debug", "deps"))
        expect(
            staged_library(target_layout)[0].endswith(os.path.join("target", "debug", "deps")),
            "the plain target layout was not found",
        )
        empty_layout = os.path.join(base, "stage-empty")
        os.makedirs(os.path.join(empty_layout, "build", "debug", "deps"))
        try:
            staged_library(empty_layout)
        except RuntimeError:
            pass
        else:
            fail("an empty stage root did not refuse")
        ambiguous = os.path.join(base, "stage-ambiguous")
        rlib(os.path.join(ambiguous, "build", "debug", "deps"))
        rlib(os.path.join(ambiguous, "target", "debug", "deps"))
        try:
            staged_library(ambiguous)
        except RuntimeError:
            pass
        else:
            fail("two staged library roots did not refuse")
        duplicated = os.path.join(base, "stage-duplicated")
        deps = os.path.join(duplicated, "build", "debug", "deps")
        rlib(deps)
        rlib(deps, "liboxide_storage-def.rlib")
        try:
            staged_library(duplicated)
        except RuntimeError:
            pass
        else:
            fail("two staged rlibs in one root did not refuse")

        # Counterfactual: layout.stage_at deletes its whole stage after copying
        # the candidate binary, so a locator that only looked there would find
        # nothing. Model that deletion and require the refusal.
        deleted_layout = os.path.join(base, "stage-deleted")
        rlib(os.path.join(deleted_layout, "build", "debug", "deps"))
        shutil.rmtree(deleted_layout)
        try:
            staged_library(deleted_layout)
        except RuntimeError:
            pass
        else:
            fail("a deleted stage_at layout did not refuse")

        # The retained-stage lifecycle: locate the artifact, link from it, and
        # only then remove the stage; the helper output survives the removal.
        lease_stage = os.path.join(base, "lease-stage")
        created = os.path.join(lease_stage, "build", "debug", "deps")
        rlib(created)
        deps_root, artifact = staged_library(lease_stage)
        expect(
            os.path.realpath(deps_root).startswith(os.path.realpath(lease_stage) + os.sep),
            "the retained deps root is not inside the helper stage",
        )
        helper_out = os.path.join(base, "competing-lease")
        with open(helper_out, "wb") as handle:
            handle.write(b"synthetic helper\n")
        shutil.rmtree(lease_stage)
        expect(os.path.isfile(helper_out), "the helper output did not survive stage removal")
        expect(not os.path.exists(artifact), "the staged rlib was not consumed by the removed stage")

        # The helper's environment puts both Cargo outputs under its retained
        # stage and uses the private offline home and pinned toolchain.
        fake = types.SimpleNamespace(
            home=os.path.join(base, "fake-home"),
            cargo_home=os.path.join(base, "fake-cargo"),
            rustup=os.path.join(base, "fake-rustup"),
        )
        env = helper_build_env(fake, lease_stage)
        expect(
            env["CARGO_TARGET_DIR"] == os.path.join(lease_stage, "target")
            and env["CARGO_BUILD_BUILD_DIR"] == os.path.join(lease_stage, "build")
            and env["CARGO_HOME"] == fake.cargo_home
            and env["HOME"] == fake.home
            and env["RUSTUP_TOOLCHAIN"] == BUILDER
            and env["CARGO_NET_OFFLINE"] == "true"
            and env["RUSTFLAGS"] == FROZEN_RUSTFLAGS
            and "CARGO_ENCODED_RUSTFLAGS" not in env,
            f"the helper build environment drifted: {env}",
        )

        # The receipt builder, on a fake collection.
        collection = perm.Collection()
        collection.outcomes = {case: {"result": "completed"} for case, _r, _t in CASES[:2]}
        collection.aborted = None
        collection.source = "synthetic"
        collection.detached = "synthetic"
        collection.mount_present = False
        document = diagnostic_document(proof=None, collection=collection, failure=None)
        expect(document["qualified"] is False, "the diagnostic receipt claims qualification")
        expect(document["summary"]["completed"] == 2, f"receipt summary drifted: {document['summary']}")
        expect(len(document["cases"]) == len(CASES), "the receipt does not cover every declared case")
        expect(len(json.dumps(document)) <= 64 * 1024, "the receipt exceeds its byte bound")
    finally:
        shutil.rmtree(base, ignore_errors=True)
    print("cold-slot-seeding self-test: collection, oracles and bounded receipt hold")


def diagnostic_document(*, proof, collection, failure) -> dict:
    rows = []
    for case, requires, text in CASES:
        row = {"case": case, "requires": list(requires), "description": text}
        row.update(collection.outcomes.get(case, {"result": "unexecuted"}))
        rows.append(row)
    summary = {
        result: sum(1 for row in rows if row["result"] == result)
        for result in ("completed", "failed", "skipped", "aborted", "unexecuted")
    }
    detached = collection.detached
    if detached.startswith("not attempted") and proof is not None:
        detached = perm.detach_disposition(collection, proof.unclosed)
    return {
        "qualified": False,
        "proof": "cold-slot-seeding",
        "kind": "uncounted keep-going collection; never a counted native PASS",
        "source": collection.source,
        "cases": rows,
        "summary": summary,
        "failures": collection.failures,
        "aborted": collection.aborted,
        "phases": PHASES,
        "observations": {
            "elapsed_s": round(time.monotonic() - runner.STARTED, 1),
            "peak_owned_mib": 0 if proof is None else proof.peak // MIB,
            "host_free_min_mib": None if proof is None or proof.low is None else proof.low // MIB,
            "detach": detached,
            "mount_path_present_after_detach": collection.mount_present,
            "unclosed_children": [] if proof is None else sorted(proof.unclosed),
        },
        "limits": {
            "scratch_root": os.environ.get("OXIDE_TASK_SCRATCH_ROOT"),
            "evidence": os.environ.get("OXIDE_PROOF_EVIDENCE"),
            "work_deadline_ns": os.environ.get("OXIDE_PROOF_DEADLINE_NS"),
            "supervisor_scratch_budget_mib": 512,
            "wall_clock_s": 300,
        },
        "limitation": (
            "A recoverable case-oracle failure is recorded and the cases that do not "
            "depend on it still run; a case with an incomplete dependency is skipped, and "
            "an abort leaves every remaining case unexecuted. A skipped or unexecuted case "
            "is never a pass, and this receipt is never a counted native PASS."
        ),
    }


def write_diagnostic(proof: Proof, collection: perm.Collection, failure) -> str:
    document = diagnostic_document(proof=proof, collection=collection, failure=failure)
    text = json.dumps(document, indent=2, sort_keys=True)
    expect(len(text) <= 64 * 1024, "the diagnostic receipt exceeds 64 KiB")
    name = "cold-slot-seeding-diagnostic.json"
    path = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    print(
        f"cold-slot-seeding diagnostic receipt {path} (qualified=false, {document['summary']})",
        flush=True,
    )
    return name


def execute(proof: Proof, repo: str, diagnostic: bool) -> perm.Collection:
    global COLLECTION
    collection = perm.Collection()
    COLLECTION = collection
    collection.source = (
        proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30)
        .stdout.decode()
        .strip()
    )
    setup(proof, repo)
    table = [(case, requires, (lambda case=case: BODIES[case](proof))) for case, requires, _text in CASES]

    def after_case(case: str) -> None:
        if proof.unclosed:
            raise perm.ProofAbort(f"unclosed children after case {case}: {sorted(proof.unclosed)}")
        if proof.mount is not None and not os.path.ismount(proof.mount):
            raise perm.ProofAbort(f"the owned image is no longer mounted at {proof.mount} after case {case}")
        proof.phase(f"after case {case}")
        perm.require_case_resources(peak_bytes=proof.peak, low_bytes=proof.low, case=case)

    perm.collect_cases(
        table,
        diagnostic=diagnostic,
        work_left=work_left,
        after_case=after_case,
        record=collection,
    )
    return collection


def main(argv: list[str]) -> int:
    modes = [flag for flag in argv if flag.startswith("--")]
    unknown = [flag for flag in modes if flag not in ("--plan", "--self-check", "--self-test", "--diagnostic")]
    if unknown:
        fail(f"unknown argument {unknown[0]}; --plan, --self-check, --self-test or --diagnostic")
    if "--plan" in modes:
        plan()
        return 0
    if "--self-check" in modes:
        self_check()
        return 0
    if "--self-test" in modes:
        self_test()
        return 0
    diagnostic = "--diagnostic" in modes
    if sys.platform != "darwin":
        fail("the cold-slot seeding proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, "OXIDE_TASK_SCRATCH_ROOT is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} is not the supervisor's scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", "")) and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared supervisor",
    )
    repo = os.path.dirname(HERE)
    root = fixture_dir(scratch, "cold-seed.")
    signal.signal(signal.SIGTERM, terminated)
    proof = Proof(root)
    failure = None
    try:
        execute(proof, repo, diagnostic)
    except BaseException as fault:
        failure = fault
        collection = COLLECTION
        if diagnostic:
            try:
                write_diagnostic(proof, collection, fault)
            except BaseException as receipt_failure:
                print(f"the diagnostic receipt was not written: {receipt_failure!r}", file=sys.stderr)
                raise
        raise
    finally:
        if failure is not None:
            # An escaped fault leaves writer or device custody unknown: the
            # image is retained, never detached a second time or reused.
            if proof.mount is not None:
                print(
                    f"retained the owned image at {proof.mount}: the run ended with {failure!r}",
                    file=sys.stderr,
                )
    # The counted tail: one ordinary detach when the collection is clean.
    collection = COLLECTION
    if proof.mount is not None:
        perm.run_ordinary_detach(proof, collection)
        collection.mount_present = False
    proof.phase("end")
    rows = [collection.outcomes.get(case, {"result": "unexecuted"}) for case, _r, _t in CASES]
    passed = sum(1 for row in rows if row["result"] == "completed")
    elapsed = time.monotonic() - runner.STARTED
    line = (
        f"cold-slot-seeding proof: {passed} cases passed, {elapsed:.0f} s script time, "
        f"peak owned {proof.peak // MIB} MiB at phase checks "
        f"(512 MiB budget, checked between phases, not a ceiling), "
        f"host free minimum {proof.low // MIB} MiB; scratch closure and cleanup are the shared runner's"
    )
    expect(passed == len(CASES), f"only {passed} of {len(CASES)} cases completed")
    print(line)
    if diagnostic:
        write_diagnostic(proof, collection, None)
    else:
        result = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "result.txt")
        with open(result, "w", encoding="utf-8") as handle:
            handle.write(f"passed\n{line}\n")
        print(f"cold-slot-seeding result {result}")
    return 0


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
