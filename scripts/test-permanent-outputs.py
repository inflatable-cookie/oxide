#!/usr/bin/env python3
"""Final outputs on the permanent drive: the owned checkout target link
(oxide#117).

The qualified shared supervisor owns the 300 s deadline, 512 MiB monitored
scratch, process closure and cleanup. This script reuses the reviewed
oxide#104 staging/child helper and writes only inside the supervisor-owned
scratch and its evidence directory. It stages this clean HEAD's `oxide` offline with 1.97.1, models the
permanent drive as one owned 128 MiB APFS sparse image attached at a unique
/Volumes/<name>, and proves:

- an effect-free JSON plan, and refusals that never move, adopt or replace a
  nonempty real target or a foreign symlink;
- explicit conversion of a missing/empty target into profile member links,
  plus builds, `run`, `test` and profiles through a legacy whole-target fixture;
- the checkout-local fallback root while the drive is absent, and redocking
  with both roots retained and nothing copied or deleted;
- a wrong recorded volume UUID selects the local root without writing to the
  external one, and no absent mount point is recreated;
- snapshot/barrier behaviour with an active build script in the checkout;
- crash replay after a prepared receipt, a live sibling link and a completed
  rename; and a contradictory state that stops and keeps both roots;
- `oxide clean` resolving the receipt to the real root and preserving the
  link, the marker and inactive roots;
- `oxide outputs detach` restoring a real directory in ephemeral mode.

The `detach-permanent` mode (oxide#124) proves the per-checkout opt-out under
permanent mode: detach keeps the roots and records a detached receipt, the next
command builds into the real target without relinking or a notice, automatic
adoption stays off for it, and an explicit later adopt re-converts an empty
target through the existing preconditions.

The focused modes reuse this fixture and staging helpers:

- `auto-adopt`: automatic first conversion (oxide#121).
- `ignore-safety`: the first conversion's Git ignore prerequisite
  (oxide#125): a target link is published only when Git ignores its exact
  non-directory repository-relative path, proven for explicit adopt, automatic
  adoption and prepared first-conversion replay, with trailing-slash,
  negated, unignored, tracked, missing-Git, timeout and malformed-answer
  refusals that change nothing.
- `exec-inference`: `oxide exec` output-member inference (oxide#160, S1) and
  the profile-layout automatic first conversion (S4b): a derived `exec` set
  that places a tool's build in the checkout's committed members, a warm
  repeat, the declared `--output` parity, an unrecorded member left local,
  a detached and an unconverted checkout running locally, a spaced checkout
  path, an existing whole-target checkout that stays whole-target, a foreign
  member link, root drift, a receipt changed while the command waits, an
  interrupted tool whose group closes before the lease releases, and a live
  checkout's committed version-1 whole-target receipt: every failing reader
  refuses it naming adopt while recorded `status` reads nothing and `--measure`
  reports the same refusal, one explicit adopt upgrades it through the shipped
  migration with its old outputs intact, and a noncommitted version-1 control
  keeps its exact bytes (oxide#163). It runs
  with a private declared scratch pool under the supervisor-selected root.
  The separate `base-compat` mode keeps /private/tmp for the pre-S3 binary
  (decision de52775e).

Not a benchmark, not real-drive qualification and not full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import errno
import glob
import hashlib
import importlib.util
import json
import os
import plistlib
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    "pool_layout", os.path.join(HERE, "test-oxide-pool-layout.py")
)
layout = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(layout)
expect, fail, summary, MIB = layout.expect, layout.fail, layout.summary, layout.MIB


def positively_absent(path: str) -> bool:
    try:
        os.lstat(path)
    except OSError as error:
        if error.errno == errno.ENOENT:
            return True
        raise
    return False


def same_device_tmp(tmp: str, pool_parent: str) -> bool:
    """Read-only precondition for inherited temporary storage on the pool device."""
    try:
        return (os.path.isdir(tmp) and os.access(tmp, os.W_OK | os.X_OK)
                and os.stat(tmp).st_dev == os.stat(pool_parent).st_dev)
    except OSError:
        return False


def check_child_env(env: dict) -> None:
    pool = env.get("OXIDE_POOL")
    expect(bool(pool) and env.get("OXIDE_SCRATCH_ROOT") == os.path.dirname(pool),
           "fixture pool or scratch declaration is missing")
    expect(same_device_tmp(env.get("TMPDIR", ""), os.path.dirname(pool)),
           "fixture TMPDIR must be writable on the directory pool device")


def image_slot_model(mount: str) -> str:
    # Fixture labels are at most nine bytes (untracked); fresh and ignore
    # checkout labels are shortened below. A private pool starts with eight
    # digest digits and IDs below 100; the observed paths are checked too, so
    # a collision, wider ID or unexpected label never silently weakens this.
    return os.path.join(mount, "oxide-pool", "slots", "untracked-ffffffff", "release-99", "scratch")


def check_image_slot_paths(mount: str) -> None:
    """Check actual image slot names, including collision and ID expansion."""
    for slot in glob.glob(os.path.join(mount, "oxide-pool", "slots", "*", "*")):
        if os.path.isdir(slot):
            path = os.path.join(slot, "scratch")
            expect(len(os.fsencode(path)) <= 80, f"image slot scratch exceeds 80 bytes: {path}")


MIGRATION_HOOKS = ("migration-p0", "migration-staged", "migration-marker", "migration-backed-up", "migration-swapped", "migration-p4")
LEGACY_HOOKS = ("prepared", "linked", "renamed")


def require_migration_fixture(receipt: dict | None, checkout: str, case: str) -> None:
    expect(receipt is not None and receipt.get("state") == "committed"
           and receipt.get("version") in ("oxide.outputs.transition/1", "oxide.outputs.transition/2")
           and receipt.get("checkout") == checkout and os.path.islink(os.path.join(checkout, "target")),
           f"{case}: migration requires a committed whole-target fixture")


def require_legacy_fixture(receipt: dict | None, checkout: str, case: str) -> None:
    expect(receipt is None and not os.path.lexists(os.path.join(checkout, "target")),
           f"{case}: transition hook requires a never-converted checkout")


def mode_allows(mode: str, case: str) -> bool:
    return (case == "historical-base") if mode == "base-compat" else (case != "historical-base")


def require_case(mode: str, case: str) -> None:
    expect(mode_allows(mode, case), f"{case} is not permitted in {mode}")


class Proof(layout.Proof):
    def __init__(self, root: str):
        super().__init__(root)
        self.image = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "volume.sparseimage")
        self.devices = []
        self.detach_confirmed = False
        self.detach_attempted = False
        self.fixture_ready = False
        self.diagnostic = False

    def phase(self, label):
        try:
            return super().phase(label)
        except RuntimeError as fault:
            if self.diagnostic:
                raise ProofAbort(f"resource checkpoint {label}: {fault}") from fault
            raise

    def run(self, command, *, cwd=None, env=None, timeout=120):
        child_env = env or self.env
        if self.fixture_ready:
            stage_target = child_env.get("CARGO_TARGET_DIR", "")
            if command[0] == "cargo" and stage_target.startswith(self.root + os.sep):
                expect(same_device_tmp(child_env.get("TMPDIR", ""), self.root),
                       "staging TMPDIR must be writable on the scratch device")
            else:
                check_child_env(child_env)
        result = super().run(command, cwd=cwd, env=env, timeout=timeout)
        if self.mount and os.path.ismount(self.mount):
            check_image_slot_paths(self.mount)
        return result

    def detach(self) -> None:
        if self.detach_confirmed:
            return
        if self.detach_attempted:
            raise RuntimeError(f"detach already attempted; retained {self.image}")
        self.detach_attempted = True
        if not self.mount or not self.devices:
            raise RuntimeError(f"attachment identity unknown; retained {self.image}")
        if not os.path.ismount(self.mount):
            raise RuntimeError(f"mount not present for ordinary detach; retained {self.image}")
        super().detach()
        if not positively_absent(self.mount) or not all(positively_absent(dev) for dev in self.devices):
            raise RuntimeError(f"ordinary detach did not remove mount and devices; retained {self.image}")
        self.detach_confirmed = True

    def remove_backing(self) -> None:
        if self.detach_confirmed:
            os.unlink(self.image)


BUILDER = "1.97.1"
SHUTDOWN_MARGIN_S = 30
MARKER = ".oxide-output.json"
# The last main commit without owned outputs: its state parser has no fence.
PRE_FEATURE = "8c9b5aeab57b8d0ca285b5358e07c89606724c8d"

APP = {
    "Cargo.toml": (
        '[package]\nname = "poutputs"\nversion = "0.1.0"\nedition = "2021"\n'
        'build = "build.rs"\n'
    ),
    "build.rs": r'''use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=OXIDE_OUTPUT_GATE");
    let Ok(gate) = std::env::var("OXIDE_OUTPUT_GATE") else {
        return;
    };
    let gate = PathBuf::from(gate);
    fs::create_dir_all(&gate).unwrap();
    fs::write(gate.join("started"), b"started").unwrap();
    fs::write(
        gate.join(format!("started-{}", std::process::id())),
        b"started",
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while !gate.join("release").exists() {
        assert!(Instant::now() < deadline, "proof did not release the build");
        std::thread::sleep(Duration::from_millis(25));
    }
}
''',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
    "src/main.rs": r'''use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    // A gated run records the executable it was started as, then waits, or
    // leaves an ordinary child (same process group) waiting and exits first.
    if let Ok(gate) = std::env::var("OXIDE_RUN_GATE") {
        let gate = PathBuf::from(gate);
        let exe = std::env::current_exe().unwrap();
        std::fs::write(gate.join("exe"), exe.to_string_lossy().as_bytes()).unwrap();
        if std::env::var_os("OXIDE_RUN_CHILD").is_some() {
            let release = gate.join("release");
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("n=0; while [ ! -f \"$1\" ] && [ $n -lt 1800 ]; do sleep 0.05; n=$((n+1)); done")
                .arg("sh")
                .arg(&release)
                .spawn()
                .unwrap();
            std::fs::write(gate.join("leader-exited"), b"exited").unwrap();
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        while !gate.join("release").exists() {
            assert!(Instant::now() < deadline, "proof did not release the run");
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    println!("permanent-outputs-ok {}", poutputs::value());
}
''',
    "tests/smoke.rs": (
        "#[test]\nfn smoke() {\n    assert_eq!(poutputs::value(), 7);\n}\n"
    ),
}

PLAIN_PACKAGE = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 5:
        fail("work deadline reached before the runner's shutdown margin")
    return left


def write_tree(root: str, files: dict[str, str]) -> str:
    for relative, contents in files.items():
        path = os.path.join(root, relative)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(contents)
    return os.path.realpath(root)


def write_plain(root: str) -> str:
    os.makedirs(root, exist_ok=True)
    write_tree(root, {"Cargo.toml": PLAIN_PACKAGE, "src/lib.rs": "pub fn v() -> u32 { 1 }\n"})
    return os.path.realpath(root)


def oxide_env(proof: Proof, args: list, label: str, cwd: str, env: dict) -> tuple:
    """`Proof.oxide` under a command-specific environment."""
    result = proof.run([proof.binary, *args], cwd=cwd, env=env)
    stdout, stderr = result.stdout.decode(errors="replace"), result.stderr.decode(errors="replace")
    print(f"----- {label}: exit {result.returncode} -----\n{stdout}{stderr}", flush=True)
    return result.returncode, stdout, stderr


def read_bytes(path: str) -> bytes:
    with open(path, "rb") as handle:
        return handle.read()


def slot_state(stderr: str) -> tuple[str, str]:
    """The leased slot id and whether it was warm, from the summary line."""
    line = summary(stderr)
    match = re.search(r"slot (\S+) (cold|warm)", line)
    expect(match is not None, f"no slot state in {line}")
    return match.group(1), match.group(2)


def started_markers(gate: str) -> list[str]:
    return sorted(glob.glob(os.path.join(gate, "started-*")))


def stop_gate(gate: str, release: str) -> None:
    with open(release, "wb") as handle:
        handle.write(b"release")


def read_json(path: str) -> dict:
    return json.loads(read_bytes(path).decode())


def handover_absent(root: str) -> bool:
    return not os.path.exists(os.path.join(root, "handover"))


def receipt_path(pool: str, checkout: str) -> str:
    """The receipt is keyed by the canonical checkout's SHA-256 digest."""
    key = hashlib.sha256(checkout.encode()).hexdigest()
    path = os.path.join(pool, "oxide-managed", "outputs", key + ".json")
    expect(os.path.isfile(path), f"no output receipt for {checkout} under {pool}")
    return path


def registration_path(pool: str) -> str:
    return os.path.join(pool, "oxide-managed", "removable.json")


def set_recorded_uuid(pool: str, uuid: str) -> None:
    path = registration_path(pool)
    current = read_json(path)
    current["volume_uuid"] = uuid
    with open(path, "wb") as handle:
        handle.write(json.dumps(current, indent=2).encode() + b"\n")
    os.chmod(path, 0o600)


def set_container_uuid(pool: str, uuid: str) -> None:
    path = registration_path(pool)
    current = read_json(path)
    current["container_uuid"] = uuid
    with open(path, "wb") as handle:
        handle.write(json.dumps(current, indent=2).encode() + b"\n")
    os.chmod(path, 0o600)


def force_detach(proof: Proof, image: str, mount: str, uuid: str) -> None:
    """Surprise removal of the owned image only: positive identity of this
    image path's own whole device, its mount and its volume UUID immediately
    before `hdiutil detach -force` of that device."""
    info = plistlib.loads(proof.run(["/usr/bin/hdiutil", "info", "-plist"], timeout=30).stdout)
    own = [
        entry
        for entry in info.get("images", [])
        if os.path.realpath(entry.get("image-path", "")) == os.path.realpath(image)
    ]
    expect(len(own) == 1, f"image {image} attached {len(own)} times")
    entities = own[0]["system-entities"]
    mounts = {entity.get("mount-point") for entity in entities} - {None}
    expect(mounts == {mount}, f"image {image} is mounted at {mounts}, not {mount}")
    wholes = sorted(
        (entity["dev-entry"] for entity in entities if re.fullmatch(r"/dev/disk\d+", entity.get("dev-entry", ""))),
        key=lambda dev: int(dev[len("/dev/disk"):]),
    )
    expect(bool(wholes), f"no whole-disk device in {entities}")
    volume = plistlib.loads(proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], timeout=30).stdout)
    expect(volume.get("VolumeUUID") == uuid, f"{mount} is not volume {uuid}")
    result = proof.run(["/usr/bin/hdiutil", "detach", "-force", wholes[0]], timeout=60)
    expect(result.returncode == 0, f"forced detach of {wholes[0]} failed; evidence kept: {result.stderr!r}")
    proof.mount = None
    expect(not os.path.lexists(mount), f"{mount} still exists after the forced detach")
    print(f"forced detach of {wholes[0]} ({mount})", flush=True)


def sha256_file(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def real_uuid(pool: str) -> str:
    return read_json(registration_path(pool))["volume_uuid"]


def link_target(path: str) -> str:
    return os.path.realpath(os.readlink(path))


def start_child(proof: Proof, command: list, cwd: str, env: dict) -> subprocess.Popen:
    check_child_env(env)
    child = subprocess.Popen(
        command, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    proof.unclosed.add(child.pid)
    return child


# Oxide's own words for a writer group (or a retired slot) whose closure is NOT
# confirmed. They are the product's explicit custody statement, not an oracle
# value: the collection must stop, because the group may still be live and no
# later case may run beside it. The markers are the exact sentences
# `main.rs`/`store.rs` print, never loose keywords.
UNCONFIRMED_CUSTODY_MARKERS = (
    "could not confirm Cargo group",
    "could not confirm the Cargo clean group",
    "stays held, and a later oxide command recovers it",
    "is retired and stays held",
    "quarantined: lost with the removable volume",
)

# Installed by `guard_writer_custody` for the run that needs it, so a mode whose
# crash cases legitimately produce such a message is not affected.
CHILD_CUSTODY_GUARD = None


def require_normal_exit(label: str, code) -> None:
    """Abort when a proof child's exit status cannot be classified.

    A negative status means the leader itself was terminated by a signal
    (`-9` = SIGKILL) and `None` means it was never reaped. Neither says anything
    about the Cargo group, its slot or its writers, so a signalled or unreaped
    leader is unknown custody — never a recoverable case-oracle failure even
    with empty stderr. An ordinary exit, including Oxide's own `128 + signal`
    status (130 after a forwarded interrupt), is classifiable and passes.
    """
    if code is None:
        raise ProofAbort(
            f"{label}: the proof child was not reaped, so its writer custody is unknown"
        )
    if code < 0:
        raise ProofAbort(
            f"{label}: the proof child ended by signal {-code}, so its writer "
            f"custody is unknown"
        )


def reattach_after_case(*, completed: bool, reattach, reason: str) -> str:
    """Reattach this run's owned image after a case that detached it, and ONLY
    after that case COMPLETED (oxide#164 review).

    A custody abort must retain the uncertain resource: reattaching during an
    abort would mount a volume whose writers, mount identity or device binding
    are exactly what the abort could not confirm. So the decision is explicit and
    testable, and a case that aborted or failed leaves the volume detached and
    says so in the receipt. Returns the disposition to record.
    """
    if not completed:
        return f"retained detached: {reason}"
    reattach()
    return "ordinary re-attach after the completed case"


def restore_after_case(*, completed: bool, restore) -> str:
    """Restore a fixture's pool state ONLY when its case COMPLETED.

    Mirrors `reattach_after_case`. A restoration issued after an abort (or any
    failure) is a command against a pool whose writer or device custody is
    unknown: it mutates the shared fixture and can replace the first fault with
    its own. So a case that did not complete retains the fixture's state exactly
    as it is — the collection aborts and the receipt names the retention — and
    only a completed case runs `restore()`. A restore failure is its own fault
    and cannot mask an earlier one, because on this path there is none.
    """
    if not completed:
        return "retained fixture state: the case did not complete"
    restore()
    return "fixture state restored after the completed case"


def require_confirmed_closure(label: str, stderr) -> None:
    """Abort when Oxide explicitly reports UNCONFIRMED writer custody.

    A stopped group whose closure could not be confirmed, a retired slot that
    stays held, or a slot quarantined with a lost volume keeps an unknown writer
    alive. That is an unknown outcome, never a recoverable case-oracle failure:
    `ProofAbort` latches the collection (and, outside a case, ends the run), so
    nothing else runs beside it.
    """
    if isinstance(stderr, bytes):
        stderr = stderr.decode(errors="replace")
    for marker in UNCONFIRMED_CUSTODY_MARKERS:
        if marker in stderr:
            line = next(text for text in stderr.splitlines() if marker in text)
            raise ProofAbort(
                f"{label}: Oxide reported unconfirmed writer closure, so nothing "
                f"else may run beside it: {line.strip()[:400]}"
            )


def guard_writer_custody(proof: Proof) -> None:
    """Classify every command this proof runs (oxide#164 review).

    `Proof.oxide`, `Proof.run` (which carries the `oxide_env` helper, staging,
    git and plan calls) and the gated child's own exit status and stderr through
    `finish_child` are checked for TWO conditions that make writer custody
    unknown, and both are `ProofAbort`s so the collection latches before any
    independent case can run beside them:

    1. an abnormal or unclassifiable leader exit (`require_normal_exit`: `-9`, or
       never reaped), even with empty stderr, and
    2. Oxide's own unconfirmed-closure report on stderr
       (`require_confirmed_closure`).
    """
    global CHILD_CUSTODY_GUARD
    inner_oxide = proof.oxide

    def oxide(args: list, label: str, cwd: str) -> tuple:
        code, stdout, stderr = inner_oxide(args, label, cwd)
        require_confirmed_closure(label, stderr)
        require_normal_exit(label, code)
        return code, stdout, stderr

    proof.oxide = oxide
    inner_run = proof.run

    def run(command: list, *, cwd: str | None = None, env: dict | None = None, timeout: float = 120):
        result = inner_run(command, cwd=cwd, env=env, timeout=timeout)
        label = " ".join(str(part) for part in command[:1]) or "command"
        require_confirmed_closure(label, result.stderr)
        require_normal_exit(label, result.returncode)
        return result

    proof.run = run
    CHILD_CUSTODY_GUARD = lambda label, stderr, code: (
        require_confirmed_closure(label, stderr),
        require_normal_exit(label, code),
    )


def finish_child(proof: Proof, child: subprocess.Popen, label: str) -> tuple[int, str, str]:
    try:
        out, err = child.communicate(timeout=min(60, work_left()))
    except BaseException:
        child.kill()
        try:
            child.communicate(timeout=5)
            proof.unclosed.discard(child.pid)
        except BaseException as error:
            print(f"child {child.pid} not reaped: {error!r}", file=sys.stderr)
        raise
    proof.unclosed.discard(child.pid)
    if proof.mount and os.path.ismount(proof.mount):
        check_image_slot_paths(proof.mount)
    stdout, stderr = out.decode(errors="replace"), err.decode(errors="replace")
    print(f"----- {label}: exit {child.returncode} -----\n{stdout}{stderr}", flush=True)
    if CHILD_CUSTODY_GUARD is not None:
        CHILD_CUSTODY_GUARD(label, stderr, child.returncode)
    return child.returncode, stdout, stderr


def reset_gate(gate: str) -> None:
    """Start each gated build from a clean gate, so a previous case's
    `started`/`release` file cannot make the next one run ungated."""
    shutil.rmtree(gate, ignore_errors=True)


def wait_for(path: str, label: str) -> None:
    deadline = time.monotonic() + min(60, work_left())
    while not os.path.exists(path):
        if time.monotonic() >= deadline:
            fail(f"{label}: {path} did not appear")
        time.sleep(0.05)


def plan(proof: Proof, cwd: str, label: str) -> dict:
    code, stdout, stderr = proof.oxide(["outputs", "plan"], label, cwd)
    expect(code == 0, f"{label}: plan exited {code}: {stderr}")
    report = json.loads(stdout)
    expect(report["version"] == "oxide.outputs.plan/2", f"{label}: {report}")
    expect(report["layout"] in ("whole-target", "profile", "none"), f"{label}: {report}")
    expect(report["data_move"] is False, f"{label}: plan claims a data move: {report}")
    expect(len(stdout) < 64 * 1024, f"{label}: plan exceeds its byte bound")
    return report


def attach_image(
    proof: Proof, root: str, name: str, image_name: str = "volume.sparseimage"
) -> str:
    """Attach the owned image with owners honored and verify the mounted entity.

    The backing remains in the selected evidence directory; all callers,
    including re-attaches, pass through this single mandatory ownership gate.
    """
    image = os.path.join(root, image_name)
    proof.image = image
    proof.detach_confirmed = False
    proof.detach_attempted = False
    proof.mount = None
    proof.devices = []
    if not os.path.exists(image):
        created = proof.run(
            ["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name]
            + ["-type", "SPARSE", image],
            timeout=60,
        )
        expect(created.returncode == 0, f"image creation failed: {created.stderr!r}")
    attach_command = ["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist", "-owners", "on", image]
    attached = proof.run(attach_command, timeout=60)
    expect(
        attached.returncode == 0,
        f"attach failed; attachment unresolved, evidence kept: {attached.stderr!r}",
    )
    entities = plistlib.loads(attached.stdout)["system-entities"]
    proof.devices = [entity["dev-entry"] for entity in entities if entity.get("dev-entry")]
    expect(bool(proof.devices), f"attachment devices unknown; retained {image}")
    mounts = [
        entity["mount-point"]
        for entity in entities
        if entity.get("mount-point")
    ]
    expect(len(mounts) == 1, f"expected one mount, got {mounts}")
    proof.mount = mounts[0]
    owners_honored(proof, proof.mount, "owned image attachment")
    model = image_slot_model(proof.mount)
    expect(len(os.fsencode(model)) <= 80, f"image slot scratch model exceeds 80 bytes: {model}")
    return mounts[0]


# HIGH checklist across all modes: (1) proof_env captures rustup before private
# HOME, pins the toolchain and disables installs; fixture crates are std-only,
# staging is the locked call. (2) proof_env enables OXIDE_TEST_HOME_FROM_ENV.
# (3) It scrubs inherited Oxide/XDG/Git/build controls before setting fixtures.
# (4) Auto-adoption controls establish the target/receipt premise explicitly.
# (5) exec-inference's external tool checks Cargo's subcommand argv; other
# modes have no external Cargo subcommand. (6) Every image attachment and
# re-attachment requests owners-on and observes the mount read-only in
# attach_image; Proof.detach retains custody, and registration is a second check.
# (7) main freezes RUSTFLAGS before staging and proof_env inherits those flags.
# (8) proof_env puts TMPDIR under supervisor scratch and check_child_env checks
# writability and pool device for every fixture child; image_slot_model and
# check_image_slot_paths bind the image limit, /private/tmp is a fallback only.
# The separate base-compat keep mode uses the internal root under de52775e.
# (9) main's mkdtemp has an explicit parent; fixture tool paths live inside it.
def proof_env(pool: str, home: str, cargo_home: str) -> dict:
    """The host-isolated environment every fixture command runs under."""
    env = dict(os.environ)
    real_rustup = env.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    for key in list(env):
        if key in ("OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_DEADLINE_NS"):
            continue
        if key.startswith(
            (
                "SCCACHE_",
                "OXIDE_",
                "XDG_",
                "CARGO_BUILD_",
                "CARGO_TARGET_",
                "GIT_",
            )
        ) or key in (
            "CARGO",
            "RUSTC",
            "RUSTUP_TOOLCHAIN",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
        ):
            env.pop(key, None)
    scratch = os.environ["OXIDE_TASK_SCRATCH_ROOT"]
    private_root = os.path.dirname(pool)
    expect(os.path.isabs(private_root) and os.path.commonpath((scratch, private_root)) == scratch
           and private_root != scratch, "pool is not inside a private supervisor scratch directory")
    env.update(
        {
            "OXIDE_SCRATCH_ROOT": private_root,
            "OXIDE_POOL": pool,
            "OXIDE_POOL_CAP": "64MiB",
            "HOME": home,
            "OXIDE_TEST_HOME_FROM_ENV": "1",
            "CARGO_HOME": cargo_home,
            "RUSTUP_HOME": real_rustup,
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TERM_COLOR": "never",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_BUILD_RUSTC_WRAPPER": "",
            "OXIDE_TASK_SCRATCH_ROOT": scratch,
            "TMPDIR": scratch,
            "TMP": scratch,
            "TEMP": scratch,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_AUTHOR_NAME": "oxide proof",
            "GIT_AUTHOR_EMAIL": "proof@example.invalid",
            "GIT_COMMITTER_NAME": "oxide proof",
            "GIT_COMMITTER_EMAIL": "proof@example.invalid",
        }
    )
    return env


def run(proof: Proof, repo: str, base_compat: bool = False, diagnostic: bool = False) -> None:
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = (
        proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30)
        .stdout.decode()
        .strip()
    )
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxPo" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"permanent-outputs proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")

    candidate = layout.stage_at(proof, repo, "oxide-permanent-outputs")
    proof.binary = candidate
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    gate = os.path.join(root, "gate")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    env = proof_env(pool, home, cargo_home)
    proof.env = env
    proof.fixture_ready = True

    app = write_tree(os.path.join(root, "app"), APP)
    fixtures = [app]
    if not base_compat:
        keep = write_plain(os.path.join(root, "keep"))
        foreign = write_plain(os.path.join(root, "foreign"))
        untracked = write_plain(os.path.join(root, "untracked"))
        fixtures.extend((keep, foreign, untracked))
    for fixture in fixtures:
        proof.run(["git", "init", "-q"], cwd=fixture, timeout=30)
        # A first conversion needs Git to ignore the exact non-directory link
        # path (oxide#125); `/.oxide-outputs/` is still left to `oxide init`.
        with open(os.path.join(fixture, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n/.oxide-outputs/\n" if base_compat else "/target\n")
    if not base_compat:
        expect(
            proof.run(["git", "check-ignore", "-q", "--", ".oxide-outputs"], cwd=app).returncode != 0,
            "the fixture already ignored .oxide-outputs before oxide init",
        )
        code, stdout, stderr = proof.oxide(["init"], "oxide init adds the ignore entry", app)
        expect(
            code == 0 and ("created" in stdout.lower() or "added" in stdout.lower()),
            stdout + stderr,
        )
        ignore_text = read_bytes(os.path.join(app, ".gitignore")).decode()
        expect(
            "/.oxide-outputs/" in ignore_text.splitlines(),
            f"oxide init did not add the local-output ignore entry: {ignore_text!r}",
        )
        code, stdout, stderr = proof.oxide(["init"], "oxide init is idempotent", app)
        expect(code == 0 and "already present" in stdout, stdout + stderr)
        expect(
            read_bytes(os.path.join(app, ".gitignore")).decode().count("/.oxide-outputs/") == 1,
            "oxide init duplicated the ignore entry",
        )
    proof.phase("after fixtures and adoption")

    # A first build creates the internal pool; the checkout stays ephemeral
    # and its real target directory is untouched by output policy.
    code, _, stderr = proof.oxide(["check"], "ephemeral first build", app)
    expect(code == 0, f"first build failed: {stderr}")
    if not base_compat:
        expect(os.path.isdir(os.path.join(app, "target")), "ephemeral target missing")
        expect(
            not os.path.lexists(os.path.join(app, "target", MARKER)),
            "ephemeral build wrote an output marker",
        )
        expect(
            not os.path.exists(os.path.join(app, ".oxide-outputs")),
            "an ephemeral build created the local output root",
        )

        # Registration: permission for permanent output placement.
        code, stdout, stderr = proof.oxide(["outputs", "plan"], "plan before any drive", app)
        expect(code == 0, stderr)
        report = json.loads(stdout)
        expect(report["mode"] == "unregistered" and report["action"] == "refuse", report)
        code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt without permanent mode", app)
        expect(code != 0 and "permanent drive mode" in stderr, stderr)
        expect(
            not os.path.exists(os.path.join(app, ".oxide-outputs")),
            "a refused adopt created the local root",
        )

    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        app,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    wrong = "00000000-0000-0000-0000-000000000000"
    good = real_uuid(pool)
    proof.phase("after drive registration")

    if not base_compat:
        require_case("permanent-outputs", "ordinary-controls")
        # 1. Explicit profile adoption preserves existing target content.
        keep_target = os.path.join(keep, "target")
        os.makedirs(keep_target, exist_ok=True)
        with open(os.path.join(keep_target, "keep.txt"), "wb") as handle:
            handle.write(b"warm build")
        report = plan(proof, keep, "plan with a nonempty target")
        expect(report["current_target"] == "real-nonempty" and report["action"] == "create-owned-link", report)
        expect(report["layout"] == "profile" and not report["data_move"], report)
        expect({m["member"]: m["adoption"] for m in report["members"]} ==
               {"debug": "convert", "release": "convert"}, report)
        code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt a nonempty target", keep)
        expect(code == 0, stderr)
        expect(read_bytes(os.path.join(keep_target, "keep.txt")) == b"warm build", "target mutated")
        expect(os.path.isdir(keep_target), "the real target directory was replaced")
        expect(not os.path.islink(os.path.join(keep_target, "keep.txt")), "adopt replaced keep.txt")
        for member in ("debug", "release"):
            expect(os.path.islink(os.path.join(keep_target, member)), f"{member} was not converted")

        # 2. A foreign symlink refuses, with no mutation.
        outside = os.path.join(root, "outside")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        os.symlink(outside, os.path.join(foreign, "target"))
        report = plan(proof, foreign, "plan with a foreign symlink")
        expect(report["current_target"] == "foreign" and report["action"] == "refuse", report)
        code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt a foreign symlink", foreign)
        expect(code != 0 and "never replaces it" in stderr, stderr)
        expect(os.readlink(os.path.join(foreign, "target")) == outside, "foreign link was replaced")
        expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "foreign target mutated")
        # 2b. Without a receipt, permanent mode never hands Cargo the foreign link:
        #     a build and a selected clean refuse with nothing written or removed
        #     through it, including a Cargo-shaped artifact planted behind it.
        os.makedirs(os.path.join(outside, "debug"))
        with open(os.path.join(outside, "debug", "plain"), "wb") as handle:
            handle.write(b"planted")
        outside_before = sorted(
            os.path.relpath(os.path.join(base, name), outside)
            for base, dirs, files in os.walk(outside)
            for name in dirs + files
        )
        for args, label in (
            (["build"], "build through a receiptless foreign link"),
            (["clean", "-p", "plain"], "selected clean through a receiptless foreign link"),
        ):
            code, _, stderr = proof.oxide(args, label, foreign)
            expect(code != 0 and "never follows" in stderr, stderr)
            after = sorted(
                os.path.relpath(os.path.join(base, name), outside)
                for base, dirs, files in os.walk(outside)
                for name in dirs + files
            )
            expect(after == outside_before, f"{label} changed the link's referent: {after}")
            expect(os.readlink(os.path.join(foreign, "target")) == outside, f"{label} replaced the link")
        expect(read_bytes(os.path.join(outside, "debug", "plain")) == b"planted", "the planted artifact changed")


    # 3. Explicit conversion of an empty real target.
    shutil.rmtree(os.path.join(app, "target"))
    os.makedirs(os.path.join(app, "target"))
    report = plan(proof, app, "plan with an empty real target")
    expect(report["action"] == "create-owned-link", report)
    expect(
        report["proposed_root"]["kind"] == "external"
        and report["proposed_root"]["root"].startswith(mount + "/oxide-outputs/"),
        report,
    )
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt an empty target", app)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    expect(
        "no existing output was moved" in stdout,
        f"the conversion did not state the no-move rule: {stdout}",
    )
    # Current first adoption is profile layout (outputs.rs:2230); preserve a
    # separate legacy whole-target fixture for the historical transition cases.
    external_root = expect_profile_layout(pool, mount, app, "first explicit adoption")
    marker = read_json(os.path.join(external_root, MARKER))
    expect(marker["version"] == "oxide.outputs.root/2", marker)
    receipt = read_json(receipt_path(pool, app))
    expect(receipt["version"] == "oxide.outputs.transition/3" and receipt["state"] == "committed", receipt)
    expect(not os.path.exists(os.path.join(app, ".oxide-outputs")), "adopt created a local root")
    external_root = convert_profile_to_legacy(pool, mount, app, "legacy transition fixture")
    link = os.path.join(app, "target")
    proof.phase("after first conversion")

    if base_compat:
        require_case("base-compat", "historical-base")
        proof.base_case_started = True
        # 4b. Downgrade fence: the last pre-feature Oxide refuses both pools'
        #     fenced state, so it never builds or cleans through the owned link.
        pre_src = os.path.join(root, "pre-feature-src")
        os.makedirs(pre_src)
        archive = proof.run(
            ["/bin/sh", "-c", f'git -C "$0" archive {PRE_FEATURE} | tar -x -C "$1"', repo, pre_src],
            timeout=60,
        )
        expect(archive.returncode == 0, f"pre-feature archive failed: {archive.stderr!r}")
        pre_feature = layout.stage_at(proof, pre_src, "oxide-pre-feature")
        shutil.rmtree(pre_src)
        expect(sha256_file(pre_feature) != sha256_file(proof.binary), "the pre-feature binary is the candidate")
        for state_dir in (os.path.join(pool, "oxide-managed"), os.path.join(pool, "oxide-managed", "removable", "oxide-managed")):
            fenced = read_json(os.path.join(state_dir, "state.json"))
            expect(fenced.get("owned_outputs") == "oxide.outputs/3", f"{state_dir} is not fenced: {fenced}")
        external_before = sorted(os.listdir(external_root))
        for args, label in (
            (["check"], "pre-feature check refuses the fenced pool"),
            (["clean", "-p", "poutputs"], "pre-feature selected clean refuses the fenced pool"),
        ):
            child = start_child(proof, [pre_feature] + args, app, env)
            code, _, stderr = finish_child(proof, child, label)
            # With current main's pool layout (oxide#118) the pre-feature binary
            # refuses the pool at accounting before it parses state; either
            # refusal keeps it off the owned link. The state-parser fence itself
            # is covered by store::tests::the_owned_outputs_fence_refuses_a_pre_feature_parser.
            expect(code != 0, f"{label}: the pre-feature binary did not refuse: {stderr}")
            expect(
                ("Oxide state is invalid" in stderr and "owned_outputs" in stderr)
                or "unknown_accounting" in stderr,
                f"{label}: unexpected refusal: {stderr}",
            )
            print(f"{label}: refused by {'the state fence' if 'owned_outputs' in stderr else 'pool accounting'}", flush=True)
            expect(link_target(link) == external_root, f"{label} moved the link")
            expect(sorted(os.listdir(external_root)) == external_before, f"{label} changed the owned root")
        proof.phase("after the downgrade fence")
        proof.base_case_completed = True

        proof.detach()
        return

    # 4. Builds, run, test and profiles through the owned link.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build through the link", app)
    expect(code == 0, f"build through the link failed: {stderr}")
    binary = os.path.join(app, "target", "debug", "poutputs")
    expect(os.path.isfile(binary), f"no target/debug binary at {binary}")
    expect(os.path.realpath(binary).startswith(external_root + "/"), "binary is not in the root")
    code, stdout, _ = proof.oxide(["run", "--bin", "poutputs"], "run through the link", app)
    expect(code == 0 and "permanent-outputs-ok 7" in stdout, stdout)
    code, _, stderr = proof.oxide(["test"], "test through the link", app)
    expect(code == 0, f"test through the link failed: {stderr}")
    code, _, stderr = proof.oxide(["check", "--profile", "debug"], "debug profile through the link", app)
    expect(code == 0, f"debug profile failed: {stderr}")
    expect(
        os.path.isdir(os.path.join(app, "target", "debug")),
        "the debug profile did not write target/debug",
    )
    code, stdout, _ = proof.oxide(["outputs", "status"], "status of an owned checkout", app)
    expect("outputs: mode permanent" in stdout and "generation" in stdout, stdout)
    expect(
        "local fallback root" in stdout and "not selected" in stdout,
        f"status did not name the unused local root: {stdout}",
    )

    # A separate legacy checkout will migrate its local fallback after redock;
    # the original checkout remains whole-target for the leased-build control.
    migration_app = fresh_checkout(proof, root, "redock-migration", "/target\n/.oxide-outputs/\n")
    migration_external = legacy_whole_target(proof, pool, mount, migration_app, "redock migration")
    with open(os.path.join(migration_external, "external-sentinel"), "wb") as handle:
        handle.write(b"migration external kept")

    # 5. Detaching the drive: the checkout-local fallback root.
    external_before = sorted(os.listdir(external_root))
    with open(os.path.join(external_root, "external-sentinel"), "wb") as handle:
        handle.write(b"external kept")
    proof.detach()
    expect(not os.path.lexists(mount), "the detached mount point still exists")
    code, stdout, stderr = proof.oxide(["build", "--bin", "poutputs"], "build with the drive absent", app)
    expect(code == 0, f"fallback build failed: {stderr}")
    expect("permanent drive mode: outputs use the checkout-local fallback root" in stderr, stderr)
    expect("checkout-local" in stderr, stderr)
    local_root = os.path.join(app, ".oxide-outputs", "local")
    expect(link_target(link) == local_root, f"the link did not fall back: {os.readlink(link)}")
    expect(os.path.isfile(os.path.join(local_root, MARKER)), "the fallback root has no marker")
    fallback_marker = read_json(os.path.join(local_root, MARKER))
    expect(fallback_marker["kind"] == "local" and "volume_uuid" not in fallback_marker, fallback_marker)
    expect(
        os.path.realpath(binary).startswith(local_root + "/"),
        "the fallback binary is not in the local root",
    )
    # The first fallback leased the generation the local root was then created
    # at, so an identical command reuses the same slot warm. A lease keyed from
    # a marker-less g0 would take a second cold slot here.
    fallback_slot, fallback_warmth = slot_state(stderr)
    expect(fallback_warmth == "cold", f"the first fallback was not cold: {fallback_slot} {fallback_warmth}")
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "second fallback build", app)
    expect(code == 0, f"second fallback build failed: {stderr}")
    expect(
        slot_state(stderr) == (fallback_slot, "warm"),
        f"the fallback destination did not keep its slot: {stderr}",
    )
    expect(
        os.path.realpath(binary).startswith(local_root + "/"),
        "the reused fallback binary left the local root",
    )
    with open(os.path.join(local_root, "local-sentinel"), "wb") as handle:
        handle.write(b"local kept")
    code, _, stderr = proof.oxide(["check"], "migration checkout falls back", migration_app)
    expect(code == 0, f"migration fallback failed: {stderr}")
    migration_local = os.path.join(migration_app, ".oxide-outputs", "local")
    expect(link_target(os.path.join(migration_app, "target")) == migration_local,
           "migration checkout did not select its local root")
    migration_marker = read_json(os.path.join(migration_local, MARKER))
    with open(os.path.join(migration_local, "local-sentinel"), "wb") as handle:
        handle.write(b"migration local kept")
    proof.phase("after fallback")

    # 6. Redocking relinks without copying or deleting either root.
    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"re-attached at {mounted}, not {mount}")
    before_redock = read_json(receipt_path(pool, app))
    require_migration_fixture(before_redock, app, "leased redock")
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after redocking", app)
    expect(code == 0, f"redocked build failed: {stderr}")
    expect(link_target(link) == external_root, f"redock did not relink: {os.readlink(link)}")
    redocked_receipt = read_json(receipt_path(pool, app))
    require_migration_fixture(redocked_receipt, app, "leased redock after build")
    expect(read_bytes(os.path.join(external_root, "external-sentinel")) == b"external kept", "root lost")
    expect(
        sorted(os.listdir(external_root)) == sorted(external_before + ["external-sentinel"]),
        "the absent drive's root changed",
    )
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "fallback lost")
    expect(
        os.path.realpath(binary).startswith(external_root + "/"),
        "the redocked binary did not return to the external root",
    )
    code, stdout, _ = proof.oxide(["outputs", "status"], "status with both roots", app)
    expect("local fallback root" in stdout and "present" in stdout, stdout)

    # The other checkout previews and performs explicit migration of its
    # currently selected local root; this must not change the build control.
    before_migration = read_json(receipt_path(pool, migration_app))
    require_migration_fixture(before_migration, migration_app, "redock migration")
    report = plan(proof, migration_app, "plan explicit migration after redocking")
    expect(report["action"] == "migrate" and report["layout"] == "whole-target", report)
    expect(report["proposed_root"]["root"] == migration_local, report)
    expect(set(report["migration_members"]) == {"debug", "release"}, report)
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "migrate local root after redocking", migration_app)
    expect(code == 0, stdout + stderr)
    migration_target = os.path.join(migration_app, "target")
    expect(os.path.isdir(migration_target) and not os.path.islink(migration_target), "migration target not real")
    for member in ("debug", "release"):
        expect(member_link_target(migration_app, member) == os.path.join(migration_local, member),
               f"migration changed {member} root")
    migrated = read_json(receipt_path(pool, migration_app))
    expect(migrated["version"] == "oxide.outputs.transition/3" and migrated["state"] == "committed", migrated)
    expect(sorted(item["member"] for item in migrated["links"]) == ["debug", "release"], migrated)
    expect(all(item.get("selected", {}).get("root") == migration_local for item in migrated["links"]), migrated)
    expect(read_json(os.path.join(migration_local, MARKER))["nonce"] == migration_marker["nonce"],
           "migration changed local root nonce")
    expect(read_bytes(os.path.join(migration_local, "local-sentinel")) == b"migration local kept", "migration local root lost")
    expect(read_bytes(os.path.join(migration_external, "external-sentinel")) == b"migration external kept", "migration external root lost")
    expect(link_target(link) == external_root, "migration changed the separate redock checkout")
    proof.phase("after redock")

    # 6b. The local fallback root refuses to appear where Git would track it.
    code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt the untracked fixture", untracked)
    expect(code == 0 and "now selects" in stdout, stdout + stderr)
    untracked_link = os.path.join(untracked, "target")
    untracked_root = convert_profile_to_legacy(pool, mount, untracked, "untracked legacy fixture")
    set_recorded_uuid(pool, "00000000-0000-0000-0000-000000000000")
    code, _, stderr = proof.oxide(["check"], "local root where Git would track it", untracked)
    expect(code != 0, "a Git-tracked local fallback root was created")
    expect("Git would track it" in stderr, stderr)
    expect(
        not os.path.exists(os.path.join(untracked, ".oxide-outputs")),
        "the refused fallback created a local root",
    )
    expect(link_target(untracked_link) == untracked_root, "the refused fallback changed the link")
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "roll the refused fallback back", untracked)
    expect(code == 0 and link_target(untracked_link) == untracked_root, stderr)
    # A local root that already exists (empty) is still refused and left
    # untouched when Git would track it.
    empty_local = os.path.join(untracked, ".oxide-outputs", "local")
    os.makedirs(empty_local)
    set_recorded_uuid(pool, "00000000-0000-0000-0000-000000000000")
    code, _, stderr = proof.oxide(["check"], "pre-created empty local root", untracked)
    expect(code != 0 and "Git would track it" in stderr, stderr)
    expect(os.path.isdir(empty_local) and os.listdir(empty_local) == [], "the empty local root changed")
    expect(link_target(untracked_link) == untracked_root, "the refused fallback changed the link")
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "roll the empty-local-root refusal back", untracked)
    expect(code == 0 and link_target(untracked_link) == untracked_root, stderr)
    proof.phase("after the tracked-fallback refusal")

    # 7. A wrong recorded volume UUID selects the local root and never writes
    #    to the external one, and no absent mount point is recreated.
    marker_before = read_bytes(os.path.join(external_root, MARKER))
    entries_before = sorted(os.listdir(external_root))
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(["check"], "build with a wrong recorded UUID", app)
    expect(code == 0 and "permanent drive mode: outputs use the checkout-local fallback root" in stderr, stderr)
    expect(link_target(link) == local_root, "a wrong UUID did not fall back to the local root")
    expect(read_bytes(os.path.join(external_root, MARKER)) == marker_before, "the wrong UUID wrote a marker")
    expect(sorted(os.listdir(external_root)) == entries_before, "the wrong UUID wrote into the root")
    set_recorded_uuid(pool, good)
    proof.phase("after wrong UUID")

    # 8a. A second command in the same checkout waits for the running build,
    #     then binds its own destination: the first build keeps the root it
    #     selected and no swap reaches it.
    gate_env = dict(env)
    gate_env["OXIDE_OUTPUT_GATE"] = gate
    expect(link_target(link) == local_root, f"the pre-writer link is {os.readlink(link)}")
    reset_gate(gate)
    first = start_child(proof, [proof.binary, "check"], app, gate_env)
    try:
        wait_for(os.path.join(gate, "started"), "first gated build script")
        expect(link_target(link) == external_root, "the first command did not bind external outputs")
        set_recorded_uuid(pool, wrong)
        second = start_child(proof, [proof.binary, "check"], app, env)
        time.sleep(2.0)
        if second.poll() is not None:
            early_out, early_err = second.communicate(timeout=5)
            proof.unclosed.discard(second.pid)
            fail(
                "a second command did not wait for the running build in its checkout: "
                f"exit {second.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        expect(link_target(link) == external_root, "a swap reached the running command")
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        first_code, _, first_stderr = finish_child(proof, first, "first gated build")
        expect(first_code == 0, f"the first gated build failed: {first_stderr}")
        expect(
            os.path.isdir(os.path.join(external_root, "debug")),
            "the first build did not write to the root it selected",
        )
        second_code, second_stdout, second_stderr = finish_child(
            proof, second, "second command after the writer closed"
        )
        expect(
            second_code == 0,
            f"the second command failed: {second_stdout}{second_stderr}",
        )
        expect(
            link_target(link) == local_root,
            "the second command did not transition to its own destination",
        )
    finally:
        os.makedirs(gate, exist_ok=True)
        with open(os.path.join(gate, "release"), "wb") as handle:
            handle.write(b"release")
        if first.poll() is None:
            first.kill()
    set_recorded_uuid(pool, good)
    proof.phase("after the second-command transition")

    # 8c. A long-lived exec in the checkout does not block a second command
    #     whose destination is unchanged (the second-slot rule), and the exec
    #     is not redirected.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "relink before the exec case", app)
    expect(code == 0 and link_target(link) == external_root, f"pre-exec relink failed: {stderr}")
    exec_release = os.path.join(root, "exec-release")
    exec_child = start_child(
        proof,
        [
            proof.binary,
            "exec",
            "--",
            "/bin/sh",
            "-c",
            f"while [ ! -f {exec_release} ]; do sleep 0.05; done",
        ],
        app,
        env,
    )
    try:
        time.sleep(1.0)
        expect(exec_child.poll() is None, "the long-lived exec exited early")
        code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "command beside a long-lived exec", app)
        expect(
            code == 0 and link_target(link) == external_root,
            f"a command was blocked by the checkout's exec: {stderr}",
        )
        expect(exec_child.poll() is None, "the exec ended before the second command")
    finally:
        with open(exec_release, "wb") as handle:
            handle.write(b"go")
    exec_code, _, exec_stderr = finish_child(proof, exec_child, "long-lived exec")
    expect(exec_code == 0, f"the long-lived exec failed: {exec_stderr}")

    # 8d. Two commands started together with the same destination both reach
    #     Cargo: no post-lease deadlock between starters.
    reset_gate(gate)
    starter_one = start_child(proof, [proof.binary, "check"], app, gate_env)
    starter_two = start_child(proof, [proof.binary, "check"], app, gate_env)
    try:
        deadline = time.monotonic() + min(60, work_left())
        while len(started_markers(gate)) < 2:
            if time.monotonic() >= deadline:
                fail(
                    "only "
                    f"{len(started_markers(gate))} of two simultaneous commands reached Cargo"
                )
            time.sleep(0.05)
        expect(
            starter_one.poll() is None and starter_two.poll() is None,
            "a simultaneous command finished before the gate",
        )
        stop_gate(gate, os.path.join(gate, "release"))
        first_code, _, first_stderr = finish_child(proof, starter_one, "simultaneous command one")
        second_code, _, second_stderr = finish_child(proof, starter_two, "simultaneous command two")
        expect(
            first_code == 0 and second_code == 0,
            f"a simultaneous command failed: {first_stderr}{second_stderr}",
        )
    finally:
        os.makedirs(gate, exist_ok=True)
        stop_gate(gate, os.path.join(gate, "release"))
        for child in (starter_one, starter_two):
            if child.poll() is None:
                child.kill()
    expect(
        os.path.isdir(os.path.join(external_root, "debug")),
        "the simultaneous commands did not use the external root",
    )

    # 8e. A replaced output-directory ancestor refuses an unchanged
    #     destination, so Cargo never writes onto internal disk.
    volume_outputs = os.path.join(mount, "oxide-outputs")
    saved_outputs = os.path.join(mount, "saved-oxide-outputs")
    internal_outputs = os.path.join(root, "internal-outputs")
    os.rename(volume_outputs, saved_outputs)
    shutil.copytree(saved_outputs, internal_outputs)
    os.symlink(internal_outputs, volume_outputs)
    leaf = os.path.basename(external_root)
    before = sorted(os.listdir(os.path.join(internal_outputs, leaf)))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "replaced ancestor refuses", app)
    expect(code != 0, "a replaced output ancestor was written through")
    expect(
        "not on the registered volume" in stderr
        or "does not resolve to itself" in stderr
        or "is a mount point" in stderr,
        stderr,
    )
    expect(
        sorted(os.listdir(os.path.join(internal_outputs, leaf))) == before,
        "the refused build wrote into the internal copy",
    )
    os.remove(volume_outputs)
    os.rename(saved_outputs, volume_outputs)
    shutil.rmtree(internal_outputs)
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after restoring the ancestor", app)
    expect(code == 0 and link_target(link) == external_root, f"ancestor restore failed: {stderr}")
    proof.phase("after the barrier and ancestor controls")

    # A1/A2/L1-L3 use independent checkouts and share only the owned image.
    # A diagnostic records each hook separately and continues only while the
    # common image, registration, writers and resource checks remain certain.
    def writer_migration() -> None:
        # A1: a distinct committed whole-target fixture keeps the later legacy
        # cases independent of explicit adoption's migration.
        migration_app = fresh_checkout(proof, root, "writer-migration")
        migration_root = legacy_whole_target(proof, pool, mount, migration_app, "A1 writer")
        migration_marker = read_json(os.path.join(migration_root, MARKER))
        before = read_json(receipt_file(pool, migration_app))
        require_migration_fixture(before, migration_app, "A1")
        writer_gate = os.path.join(root, "writer-migration-gate")
        writer_env = dict(env, OXIDE_OUTPUT_GATE=writer_gate)
        reset_gate(writer_gate)
        writer = start_child(proof, [proof.binary, "check"], migration_app, writer_env)
        try:
            wait_for(os.path.join(writer_gate, "started"), "A1 gated writer")
            expect(writer.poll() is None, "A1 writer exited early")
            waiting = start_child(proof, [proof.binary, "outputs", "adopt"], migration_app, env)
            time.sleep(2.0)
            expect(waiting.poll() is None, "A1 adopt did not wait for the writer")
            expect(os.path.islink(os.path.join(migration_app, "target")), "A1 changed the target while the writer ran")
            os.makedirs(writer_gate, exist_ok=True)
            with open(os.path.join(writer_gate, "release"), "wb") as handle:
                handle.write(b"release")
            writer_code, _, writer_err = finish_child(proof, writer, "A1 writer")
            expect(writer_code == 0, writer_err)
            adopt_code, adopt_out, adopt_err = finish_child(proof, waiting, "A1 migration")
            expect(adopt_code == 0, adopt_out + adopt_err)
            expect(expect_profile_layout(pool, mount, migration_app, "A1 migration") == migration_root,
                   "A1 changed the selected root")
            after = read_json(receipt_file(pool, migration_app))
            expect(after["version"] == "oxide.outputs.transition/3", "A1 did not publish /3")
            expect(read_json(os.path.join(migration_root, MARKER))["nonce"] == migration_marker["nonce"],
                   "A1 changed the root nonce")
        finally:
            os.makedirs(writer_gate, exist_ok=True)
            with open(os.path.join(writer_gate, "release"), "wb") as handle:
                handle.write(b"release")
            if writer.poll() is None:
                writer.kill()
        proof.phase("after A1 writer and migration")

    def migration_case(point: str) -> None:
        checkout = fresh_checkout(proof, root, "migration-" + point)
        if point == "migration-marker":
            migration_root, _, _ = version_one_whole_target(proof, pool, mount, checkout, point)
        else:
            migration_root = legacy_whole_target(proof, pool, mount, checkout, point)
        before = read_json(receipt_file(pool, checkout))
        require_migration_fixture(before, checkout, point)
        old_marker = read_json(os.path.join(migration_root, MARKER))
        crash_env = dict(env, OXIDE_TEST_OUTPUTS_CRASH=point)
        code, _, stderr = oxide_env(proof, ["outputs", "adopt"], point, checkout, crash_env)
        expect(code != 0 and "injected transition stop" in stderr, f"{point}: {stderr}")
        journal = read_json(receipt_file(pool, checkout))
        expect(journal.get("transition") is not None, f"{point}: no migration journal: {journal}")
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], point + " replay", checkout)
        expect(code == 0, f"{point}: replay failed: {stdout}{stderr}")
        expect(expect_profile_layout(pool, mount, checkout, point) == migration_root,
               f"{point}: migration changed root")
        final = read_json(receipt_file(pool, checkout))
        expect(final["version"] == "oxide.outputs.transition/3" and not final.get("transition"),
               f"{point}: migration did not settle: {final}")
        if "nonce" in old_marker:
            expect(read_json(os.path.join(migration_root, MARKER))["nonce"] == old_marker["nonce"],
                   f"{point}: migration changed nonce")
        print(f"A2 {point}: stopped, replayed and committed", flush=True)
    def legacy_case(point: str) -> None:
        checkout = fresh_checkout(proof, root, "legacy-hook-" + point)
        require_legacy_fixture(None, checkout, point)
        expect(not os.path.exists(receipt_file(pool, checkout)), f"{point}: receipt already exists")
        crash_env = dict(env, OXIDE_TEST_OUTPUTS_CRASH=point)
        code, _, stderr = oxide_env(proof, ["outputs", "adopt"], point, checkout, crash_env)
        expect(code != 0 and "injected transition stop" in stderr, f"{point}: {stderr}")
        pending = read_json(receipt_file(pool, checkout))
        expect(pending["state"] == "prepared", f"{point}: no prepared receipt")
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], point + " replay", checkout)
        expect(code == 0, f"{point}: replay failed: {stdout}{stderr}")
        expect_profile_layout(pool, mount, checkout, point)
        print(f"L {point}: stopped, replayed and committed", flush=True)

    def after_hook_case(case: str) -> None:
        if proof.unclosed or not proof.mount or not os.path.ismount(proof.mount):
            raise ProofAbort(f"{case}: image or writer custody is not closed")
        if not os.path.isfile(registration_path(pool)):
            raise ProofAbort(f"{case}: removable registration is absent")
        phase(proof, f"after {case}")

    cases = [("A1:writer-migration", (), writer_migration)]
    cases += [("A2:" + point, (), lambda point=point: migration_case(point)) for point in MIGRATION_HOOKS]
    cases += [("L:" + point, (), lambda point=point: legacy_case(point)) for point in LEGACY_HOOKS]
    collection = getattr(proof, "collection", None)
    if diagnostic:
        guard_writer_custody(proof)
    collect_cases(cases, diagnostic=diagnostic, work_left=guard_work_left,
                  after_case=after_hook_case, record=collection)
    if diagnostic and (collection.failures or collection.aborted):
        fail("hook diagnostic did not complete cleanly")
    proof.phase("after A1/A2/L1-L3")

    # 9b. A same-device symlink in place of `.oxide-outputs` is not the real
    #     checkout-local root: build and clean refuse and the tree it points
    #     at is unchanged.
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(["check"], "local root before the ancestor swap", app)
    expect(code == 0 and link_target(link) == local_root, stderr)
    local_outer = os.path.join(app, ".oxide-outputs")
    moved_outer = os.path.join(root, "moved-local-outputs")
    os.rename(local_outer, moved_outer)
    os.symlink(moved_outer, local_outer)
    moved_local = os.path.join(moved_outer, "local")
    before = sorted(os.listdir(moved_local))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "symlinked local ancestor refuses", app)
    expect(code != 0 and "not a real directory" in stderr, stderr)
    code, _, stderr = proof.oxide(["clean"], "clean through a symlinked local ancestor refuses", app)
    expect(code != 0 and "clean refused" in stderr, stderr)
    expect(sorted(os.listdir(moved_local)) == before, "the refused commands changed the moved local root")
    os.remove(local_outer)
    os.rename(moved_outer, local_outer)
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "redock after the ancestor control", app)
    expect(code == 0 and link_target(link) == external_root, stderr)
    proof.phase("after the local ancestor control")

    # 9c. Split selection: the drive is present and its output root valid, but
    #     the intermediate pool falls back (container mismatch). Outputs follow
    #     the pool to the local root, so nothing writes to the drive without
    #     its removal watchdog; restoring the container redocks.
    good_container = read_json(registration_path(pool))["container_uuid"]
    set_container_uuid(pool, "00000000-0000-0000-0000-00000000C0DE")
    code, _, stderr = proof.oxide(["check"], "pool fallback with the drive present", app)
    expect(code == 0 and link_target(link) == local_root, f"split selection kept the drive: {stderr}")
    expect("fell back to the internal pool" in stderr, stderr)
    set_container_uuid(pool, good_container)
    code, _, stderr = proof.oxide(["check"], "redock after the split selection", app)
    expect(code == 0 and link_target(link) == external_root, stderr)

    # 9d. A program `oxide run` started keeps the checkout's output custody
    #     until its whole process group closes: it runs from the real external
    #     root, its leader exits first leaving an ordinary child, and `run`
    #     stays held (a relink waits, plan names the holder) until the child
    #     ends too.
    run_gate = os.path.join(root, "run-gate")
    os.makedirs(run_gate)
    run_env = dict(env)
    run_env["OXIDE_RUN_GATE"] = run_gate
    run_env["OXIDE_RUN_CHILD"] = "1"
    runner = start_child(proof, [proof.binary, "run", "--bin", "poutputs"], app, run_env)
    relinker = None
    try:
        wait_for(os.path.join(run_gate, "leader-exited"), "run leader exit")
        exe = read_bytes(os.path.join(run_gate, "exe")).decode()
        expect(
            os.path.realpath(exe).startswith(external_root + os.sep),
            f"the run executable resolved outside the bound real root: {exe}",
        )
        expect(not exe.startswith(link + os.sep), f"the run used the checkout target link: {exe}")
        time.sleep(0.5)
        expect(runner.poll() is None, "oxide run returned while its program's child still runs")
        held = plan(proof, app, "plan while a run child lives")
        expect(held["cooperative_holders"], f"plan names no holder for the run: {held}")
        set_recorded_uuid(pool, wrong)
        relinker = start_child(proof, [proof.binary, "check"], app, env)
        time.sleep(1.5)
        expect(relinker.poll() is None, "a relink did not wait for the run program's child")
        expect(link_target(link) == external_root, "the link moved under a running program")
    finally:
        open(os.path.join(run_gate, "release"), "wb").close()
    code, stdout, stderr = finish_child(proof, runner, "run with an outliving child")
    expect(code == 0 and "permanent-outputs-ok" not in stdout, stdout + stderr)
    code, _, stderr = finish_child(proof, relinker, "relink after the run group closes")
    expect(code == 0 and link_target(link) == local_root, stderr)
    expect("a program `oxide run` started" in stderr, stderr)
    set_recorded_uuid(pool, good)
    code, _, stderr = proof.oxide(["check"], "redock after the run custody case", app)
    expect(code == 0 and link_target(link) == external_root, stderr)

    # 9e. Removal while the program runs from the external root: the removal
    #     watchdog stops the program's group, `run` fails, and after
    #     re-attaching the next command redocks.
    removal_gate = os.path.join(root, "removal-gate")
    os.makedirs(removal_gate)
    removal_env = dict(env)
    removal_env["OXIDE_RUN_GATE"] = removal_gate
    runner = start_child(proof, [proof.binary, "run", "--bin", "poutputs"], app, removal_env)
    try:
        wait_for(os.path.join(removal_gate, "exe"), "gated run before removal")
        force_detach(proof, proof.image, mount, good)
        code, stdout, stderr = finish_child(proof, runner, "run program stopped by removal")
    finally:
        open(os.path.join(removal_gate, "release"), "wb").close()
    expect(code != 0, f"a run whose drive vanished succeeded: {stdout}{stderr}")
    expect(
        re.search(rf"removable volume {re.escape(mount)} is gone; stopped Cargo group \d+: TERM", stderr)
        is not None,
        f"no removal stop during the program phase: {stderr}",
    )
    expect(attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name) == mount, "the drive re-attached elsewhere")
    code, _, stderr = proof.oxide(["check"], "redock after the program-phase removal", app)
    expect(code == 0 and link_target(link) == external_root, stderr)
    proof.phase("after the split selection, run custody and removal cases")

    # 10. A foreign live link refuses and is left untouched.
    os.remove(os.path.join(app, "target"))
    os.symlink(os.path.join(root, "outside"), os.path.join(app, "target"))
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "foreign live link refuses", app)
    expect(code != 0 and "not this checkout's owned link" in stderr, stderr)
    expect(os.path.islink(os.path.join(app, "target")), "the foreign link was replaced")
    expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the foreign target mutated")
    os.remove(os.path.join(app, "target"))
    os.symlink(external_root, os.path.join(app, "target"))

    # 11. Clean resolves the receipt to the real root and preserves it.
    committed_bytes = read_bytes(receipt_path(pool, app))
    # 11a. A selected clean revalidates ownership after its barrier: a link
    #      that turns foreign while it waits is never cleaned through.
    clean_release = os.path.join(root, "clean-release")
    holder = start_child(
        proof,
        [
            proof.binary,
            "exec",
            "--",
            "/bin/sh",
            "-c",
            f"while [ ! -f {clean_release} ]; do sleep 0.05; done",
        ],
        app,
        env,
    )
    try:
        time.sleep(1.0)
        expect(holder.poll() is None, "the clean-wait holder exited early")
        code, status_out, status_err = proof.oxide(["status"], "status during the clean holder", app)
        expect(code == 0, status_err)
        expect(
            any("held by exec /bin/sh" in line for line in status_out.splitlines()),
            f"the exec holder is not recorded as active: {status_out}",
        )
        waiting = start_child(proof, [proof.binary, "clean", "-p", "poutputs"], app, env)
        time.sleep(1.5)
        if waiting.poll() is not None:
            early_out, early_err = waiting.communicate(timeout=5)
            proof.unclosed.discard(waiting.pid)
            fail(
                "selected clean did not wait for the checkout's active command: "
                f"exit {waiting.returncode}\n{early_out.decode(errors='replace')}"
                f"{early_err.decode(errors='replace')}"
            )
        os.remove(link)
        os.symlink(os.path.join(root, "outside"), link)
        with open(clean_release, "wb") as handle:
            handle.write(b"go")
        holder_code, _, holder_stderr = finish_child(proof, holder, "clean-wait holder")
        expect(holder_code == 0, f"the clean-wait holder failed: {holder_stderr}")
        clean_code, clean_out, clean_err = finish_child(
            proof, waiting, "selected clean after the link changed"
        )
        expect(
            clean_code != 0 and "not the owned link" in clean_err,
            f"selected clean ran after its ownership changed: {clean_out}{clean_err}",
        )
        expect(
            read_bytes(os.path.join(outside, "sentinel")) == b"foreign",
            "the refused selected clean removed bytes through the foreign link",
        )
    finally:
        with open(clean_release, "wb") as handle:
            handle.write(b"go")
        if holder.poll() is None:
            holder.kill()
    os.remove(link)
    os.symlink(external_root, link)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean through the owned link", app
    )
    expect(code == 0, f"selected clean failed: {stderr}")
    expect(os.path.islink(link) and link_target(link) == external_root, "selected clean lost the link")
    expect(os.path.isfile(os.path.join(external_root, MARKER)), "selected clean lost the marker")
    # A selected clean never runs against a replaced or unusable volume.
    set_recorded_uuid(pool, wrong)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean with an unusable volume", app
    )
    expect(code != 0 and "registered volume" in stderr, stderr)
    code, stdout, stderr = proof.oxide(
        ["clean", "--dry-run"], "whole clean with an unusable volume", app
    )
    expect(code != 0 and "registered volume" in stderr, stderr)
    expect(stdout.strip() == "" and handover_absent(external_root), "a refused clean printed or removed paths")
    set_recorded_uuid(pool, good)
    # A foreign live link is never cleaned through, selected or whole.
    os.remove(link)
    os.symlink(os.path.join(root, "outside"), link)
    code, _, stderr = proof.oxide(
        ["clean", "-p", "poutputs"], "selected clean through a foreign link", app
    )
    expect(code != 0 and "not the owned link" in stderr, stderr)
    expect(os.readlink(link) == os.path.join(root, "outside"), "a refused selected clean replaced the link")
    expect(os.path.isfile(os.path.join(outside, "sentinel")), "a refused selected clean removed bytes")
    os.remove(link)
    os.symlink(external_root, link)
    os.remove(os.path.join(external_root, "external-sentinel"))
    with open(os.path.join(external_root, "handover"), "wb") as handle:
        handle.write(b"foreign")
    code, _, stderr = proof.oxide(["clean"], "clean refuses a foreign root entry", app)
    expect(code != 0 and "handover" in stderr, stderr)
    expect(os.path.isfile(os.path.join(external_root, "handover")), "the refused clean removed bytes")
    os.remove(os.path.join(external_root, "handover"))
    code, stdout, stderr = proof.oxide(["clean", "--dry-run"], "clean dry run through the receipt", app)
    expect(code == 0, stderr)
    listed = stdout.splitlines()
    expect(
        all(path != external_root and not path.endswith(MARKER) for path in listed),
        f"the dry run would remove the root or its marker: {listed[:5]}",
    )
    expect(any(path.startswith(external_root + "/") for path in listed), listed[:5])
    expect("are kept" in stderr and "owned link" in stderr, stderr)
    code, _, stderr = proof.oxide(["clean"], "clean through the receipt", app)
    expect(code == 0, f"clean failed: {stderr}")
    expect(os.path.islink(link) and link_target(link) == external_root, "clean removed the owned link")
    expect(os.path.isfile(os.path.join(external_root, MARKER)), "clean removed the root marker")
    expect(not os.path.exists(os.path.join(external_root, "debug")), "clean left Cargo output")
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "clean deleted a root")
    expect("are kept" in stderr, stderr)
    code, stdout, _ = proof.oxide(["outputs", "status"], "status after clean", app)
    expect("local fallback root" in stdout and "present" in stdout, stdout)
    proof.phase("after clean")

    # 12. Detach in ephemeral mode, then restore a real directory. Permanent
    #     mode detaches too (oxide#124): `detach-permanent` proves that.
    code, _, stderr = proof.oxide(["pool", "set-mode", "ephemeral"], "set ephemeral mode", app)
    expect(code == 0, stderr)
    # A converted checkout keeps building through its owned link in
    # ephemeral mode until it is detached; nothing is relinked.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "ephemeral build before detach", app)
    expect(code == 0 and link_target(link) == external_root, f"ephemeral build refused: {stderr}")
    expect(os.path.isfile(os.path.join(external_root, "debug", "poutputs")), "no binary in the bound root")
    code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach in ephemeral mode", app)
    expect(code == 0 and "real empty directory again" in stdout, stdout + stderr)
    expect(os.path.isdir(link) and not os.path.islink(link), "detach did not restore a real directory")
    expect(os.listdir(link) == [], "the detached target is not empty")
    expect(os.path.isdir(external_root) and os.path.isdir(local_root), "detach deleted a root")
    expect(read_bytes(os.path.join(local_root, "local-sentinel")) == b"local kept", "detach lost a root")
    code, stdout, _ = proof.oxide(["outputs", "status"], "status after detach", app)
    expect("detached" in stdout, stdout)

    # 13. A detached checkout builds ephemerally; the drive stays untouched.
    code, _, stderr = proof.oxide(["build", "--bin", "poutputs"], "build after detach", app)
    expect(code == 0, f"detached build failed: {stderr}")
    expect(os.path.isdir(os.path.join(app, "target", "debug")), "detached build wrote no target/debug")
    expect(not os.path.islink(link), "a detached build re-created the owned link")
    # 14. Unknown writer custody (a lost slot never positively closed) blocks
    #     relinking and cleaning for this checkout, in either pool.
    state_paths = [
        os.path.join(pool, "oxide-managed", "state.json"),
        os.path.join(pool, "oxide-managed", "removable", "oxide-managed", "state.json"),
    ]
    saved_states = {}
    crafted = False
    for path in state_paths:
        if not os.path.exists(path) or crafted:
            continue
        saved_states[path] = read_bytes(path)
        data = json.loads(saved_states[path])
        record = next(
            (entry for entry in data.get("records", []) if entry.get("checkout") == app), None
        )
        if record is None:
            continue
        record["phase"] = "retired"
        record["retired"] = "quarantined: crafted proof writer"
        record["lost"] = {"slot_id": "77", "at_ms": 1, "unclosed": "crafted unknown writer"}
        data["next_id"] = max(int(data.get("next_id", 1)), 1000)
        with open(path, "wb") as handle:
            handle.write(json.dumps(data).encode())
        os.chmod(path, 0o600)
        crafted = True
    expect(crafted, "no checkout record to quarantine in the proof state")
    try:
        code, _, stderr = proof.oxide(
            ["pool", "set-mode", "permanent"], "permanent mode for the custody case", app
        )
        expect(code == 0, stderr)
        code, _, stderr = proof.oxide(["outputs", "adopt"], "adopt refused on quarantined custody", app)
        expect(code != 0 and "never confirmed closed" in stderr, stderr)
        code, _, stderr = proof.oxide(["clean"], "clean refused on quarantined custody", app)
        expect(code != 0 and "unknown writer custody" in stderr, stderr)
        expect(os.path.isdir(local_root) and os.path.isdir(external_root), "a refused clean removed a root")
    finally:
        for path, contents in saved_states.items():
            with open(path, "wb") as handle:
                handle.write(contents)
            os.chmod(path, 0o600)
    proof.phase("after the quarantined-custody refusal")

    # 15. A prepared first conversion never renames over a link that appeared
    #     after the crash, even when a matching sibling is live.
    code, _, stderr = proof.oxide(
        ["pool", "set-mode", "permanent"], "permanent mode for the crafted replay", app
    )
    expect(code == 0, stderr)
    crafted = json.loads(committed_bytes.decode())
    crafted["state"] = "prepared"
    generation = crafted["generation"] + 1
    crafted["prepared"] = {
        "generation": generation,
        "to": {
            "kind": "local",
            "root": local_root,
            "generation": generation,
        },
        "link": link,
    }
    with open(receipt_path(pool, app), "wb") as handle:
        handle.write(json.dumps(crafted).encode())
    os.chmod(receipt_path(pool, app), 0o600)
    sibling = os.path.join(app, f".target.oxide-new-{generation}")
    os.symlink(local_root, sibling)
    # After the detached build the target is a real directory again.
    shutil.rmtree(link)
    os.symlink(os.path.join(app, "absent-foreign"), link)
    code, stdout, stderr = proof.oxide(
        ["outputs", "adopt"], "replay never renames over a foreign dangling link", app
    )
    expect(code != 0 and "cannot be replayed" in stderr, stdout + stderr)
    expect(
        os.readlink(link) == os.path.join(app, "absent-foreign"),
        "replay replaced a link that appeared after the crash",
    )
    expect(os.path.islink(sibling), "replay removed the prepared sibling")
    expect(os.path.isdir(local_root), "replay removed an output root")
    proof.phase("after the crafted replay refusal")
    proof.phase("before the ordinary detach")
    proof.detach()
    proof.phase("end")
    expect(not os.path.lexists(mount), "the scratch mount point is still present")


AUTO_NOTICE = "oxide: auto-adopted "


def tree_digest(root: str) -> dict:
    """Every entry under `root`: file hashes, link texts and directories, so
    a root's bytes compare exactly before and after a command."""
    digest = {}
    for current, dirs, files in os.walk(root):
        for name in dirs + files:
            full = os.path.join(current, name)
            relative = os.path.relpath(full, root)
            if os.path.islink(full):
                digest[relative] = "link:" + os.readlink(full)
            elif os.path.isfile(full):
                digest[relative] = sha256_file(full)
            else:
                digest[relative] = "dir"
    return digest


def receipt_state(pool: str, checkout: str) -> str:
    with open(receipt_file(pool, checkout), "rb") as handle:
        return json.load(handle)["state"]


def detach_run(proof: Proof, repo: str, diagnostic: bool = False) -> None:
    """Focused per-checkout detach proof under permanent mode (oxide#124): the
    #121 fixture and staging, one owned 128 MiB scratch image, ordinary detach.
    Only the fixture commands' stderr, exit codes, link, receipt, plan and
    root bytes are checked."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxDp" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"detach-permanent proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-detach-permanent")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    proof.fixture_ready = True
    image = proof.image

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "unregistered build keeps a real target", maker)
    expect(code == 0 and not auto_notices(stderr), stderr)

    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    proof.phase("after drive registration")

    # 1. The precondition: a fresh checkout adopts on its first build.
    app = fresh_checkout(proof, root, "detach-adopted")
    link = os.path.join(app, "target")
    code, _, stderr = proof.oxide(["build"], "fresh checkout adopts on its first build", app)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    # The automatic first conversion publishes the profile layout (oxide#160,
    # S4b): a real `target` holding an owned `debug`/`release` pair into one
    # root on the drive.
    adopted = expect_profile_layout(pool, mount, app, "the detach fixture's first conversion")
    adopted_debug = member_link_target(app, "debug")
    expect(os.path.isfile(os.path.join(link, "debug", "poutputs")), "binary not reachable through the link")
    proof.phase("after adoption")

    def case_detach_sequence() -> None:
        # 2. Detach waits for an active build in the checkout, then restores the
        #    real directory once that build has closed. The root keeps the build.
        gate = os.path.join(root, "gate")
        gate_env = dict(proof.env, OXIDE_OUTPUT_GATE=gate)
        reset_gate(gate)
        writer = start_child(proof, [proof.binary, "check"], app, gate_env)
        waiting = None
        try:
            wait_for(os.path.join(gate, "started"), "gated build script")
            expect(writer.poll() is None, "the gated build exited early")
            waiting = start_child(proof, [proof.binary, "outputs", "detach"], app, proof.env)
            time.sleep(2.0)
            if waiting.poll() is not None:
                early_out, early_err = waiting.communicate(timeout=5)
                proof.unclosed.discard(waiting.pid)
                fail(
                    "outputs detach did not wait for the active build in the checkout: "
                    f"exit {waiting.returncode}\n{early_out.decode(errors='replace')}"
                    f"{early_err.decode(errors='replace')}"
                )
            expect(
                member_link_target(app, "debug") == adopted_debug,
                "the owned member link moved while a build was running",
            )
            with open(os.path.join(gate, "release"), "wb") as handle:
                handle.write(b"release")
            writer_code, _, writer_stderr = finish_child(proof, writer, "gated build")
            expect(writer_code == 0, f"the gated build failed: {writer_stderr}")
            expect(os.path.isdir(adopted), "the build wrote nothing to the root")
            before = tree_digest(adopted)
            detach_code, detach_stdout, detach_stderr = finish_child(proof, waiting, "detach after the build")
            waiting = None
            expect(detach_code == 0, f"detach after the build failed: {detach_stdout}{detach_stderr}")
        finally:
            os.makedirs(gate, exist_ok=True)
            with open(os.path.join(gate, "release"), "wb") as handle:
                handle.write(b"release")
            if writer.poll() is None:
                writer.kill()
            if waiting is not None and waiting.poll() is None:
                waiting.kill()
        proof.phase("after the gated detach")

        # 3. The detach restored a real empty directory, kept every root byte and
        #    recorded a detached receipt. Plan and status agree.
        expect("real empty directory again" in detach_stdout, detach_stdout)
        expect(os.path.isdir(link) and not os.path.islink(link), "detach did not restore a real directory")
        # The profile layout's detach restores one real EMPTY directory per owned
        # member and keeps the real target directory (oxide#133/#144).
        for member in ("debug", "release"):
            path = os.path.join(link, member)
            expect(
                os.path.isdir(path) and not os.path.islink(path),
                f"detach did not restore a real {member} directory",
            )
            expect(os.listdir(path) == [], f"the detached {member} is not empty")
        expect(tree_digest(adopted) == before, "detach changed the owned root")
        expect(receipt_state(pool, app) == "detached", "the receipt is not detached")
        code, stdout, stderr = proof.oxide(["outputs", "status"], "status after detach", app)
        expect(code == 0 and "detached" in stdout and "permanent" in stdout, stdout + stderr)
        report = plan(proof, app, "plan after detach")
        expect(report["mode"] == "permanent" and report["state"] == "detached", report)
        # The real target now holds the (empty) member directories, so the target
        # itself classifies as populated even though every member is empty.
        expect(report["current_target"] == "real-nonempty", report)
        expect(report["layout"] == "profile", report)
        expect("selected" not in report and "current_root" not in report, report)
        expect("detached" in (report.get("reason") or ""), report)
        code, stdout, stderr = proof.oxide(["outputs", "detach"], "a second detach changes nothing", app)
        expect(code == 0 and "already detached" in stdout, stdout + stderr)
        proof.phase("after the detach")

        # 4. The next leased builds use the real target: no relink, no notice, and
        #    the detached root is never written.
        code, _, stderr = proof.oxide(["build"], "the next build uses the real target", app)
        expect(code == 0 and not auto_notices(stderr), stderr)
        expect(os.path.isdir(link) and not os.path.islink(link), "a build relinked the target")
        expect(os.path.isfile(os.path.join(link, "debug", "poutputs")), "no binary in the real target")
        expect(tree_digest(adopted) == before, "a build wrote into the detached root")
        code, _, stderr = proof.oxide(["build"], "a repeated build stays real, no notice", app)
        expect(code == 0 and not auto_notices(stderr) and not os.path.islink(link), stderr)
        report = plan(proof, app, "plan after real-target builds")
        expect(report["state"] == "detached" and report["current_target"] == "real-nonempty", report)
        proof.phase("after the real-target builds")

        # 5. Explicit adopt keeps a nonempty real member but can re-adopt
        # another member. A distinct detached checkout supplies the absent-target
        # control, so the first adoption cannot silently invalidate its premise.
        before = tree_digest(adopted)
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], "adopt keeps the nonempty real members", app)
        expect(code == 0, f"adopt refused the nonempty members: {stdout}{stderr}")
        expect("stay real directories" in stderr, stderr)
        expect(
            os.path.isfile(os.path.join(link, "debug", "poutputs")),
            "adopt moved or converted the real member",
        )
        expect(
            os.path.isdir(os.path.join(link, "debug"))
            and not os.path.islink(os.path.join(link, "debug")),
            "adopt linked a nonempty real member",
        )
        expect(tree_digest(adopted) == before, "adopt wrote into the root")
        expect(receipt_state(pool, app) == "committed", "explicit re-adoption did not commit")
        expect(member_link_target(app, "release") == os.path.join(adopted, "release"),
               "explicit re-adoption did not select the release member")
        detached = fresh_checkout(proof, root, "detach-missing-target")
        code, _, stderr = proof.oxide(["build"], "absent-target control first adopts", detached)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        code, _, stderr = proof.oxide(["outputs", "detach"], "absent-target control detaches", detached)
        expect(code == 0 and receipt_state(pool, detached) == "detached", stderr)
        detached_target = os.path.join(detached, "target")
        shutil.rmtree(detached_target)
        expect(not os.path.lexists(detached_target), "detached control target is still present")
        code, _, stderr = proof.oxide(["build"], "a detached receipt with an absent target is never adopted", detached)
        expect(code == 0 and not auto_notices(stderr), stderr)
        expect(os.path.isdir(detached_target) and not os.path.islink(detached_target), "a detached checkout was adopted")
        expect(receipt_state(pool, detached) == "detached", "the receipt left detached state")
        proof.phase("after the absent-target control")


    def case_readopt() -> None:
        # 6. Explicit adopt re-converts an empty detached target, to the same root
        #    the checkout already had, through the existing preconditions.
        again = fresh_checkout(proof, root, "detach-readopt")
        code, _, stderr = proof.oxide(["build"], "re-adopt fixture adopts on its first build", again)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        first = os.path.dirname(member_link_target(again, "debug"))
        code, _, stderr = proof.oxide(["outputs", "detach"], "detach the re-adopt fixture", again)
        expect(code == 0, stderr)
        for member in ("debug", "release"):
            expect(
                os.listdir(os.path.join(again, "target", member)) == [],
                f"the re-adopt {member} is not empty",
            )
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], "explicit adopt converts the empty members", again)
        expect(code == 0 and "now selects the owned external root" in stdout, stdout + stderr)
        expect(
            os.path.dirname(member_link_target(again, "debug")) == first,
            "re-adopt chose another root",
        )
        expect(receipt_state(pool, again) == "committed", "re-adopt did not commit")
        first_debug = member_link_target(again, "debug")
        code, _, stderr = proof.oxide(["build"], "the re-adopted checkout builds through its links", again)
        expect(code == 0 and not auto_notices(stderr), stderr)
        expect(member_link_target(again, "debug") == first_debug, "a build relinked the re-adopted member")
        proof.phase("after explicit re-adoption")


    def case_refusals() -> None:
        # 7. Errors stay errors: a foreign link refuses the detach with no change
        #    and the receipt stays committed; a checkout never converted has no
        #    receipt to detach.
        foreign = fresh_checkout(proof, root, "detach-foreign")
        code, _, stderr = proof.oxide(["build"], "foreign fixture adopts on its first build", foreign)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        outside = os.path.join(root, "outside-detach")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        os.remove(os.path.join(foreign, "target", "debug"))
        os.symlink(outside, os.path.join(foreign, "target", "debug"))
        code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach refuses a foreign link", foreign)
        expect(code != 0 and "left untouched" in stderr, stdout + stderr)
        expect(os.readlink(os.path.join(foreign, "target", "debug")) == outside, "the foreign link was replaced")
        expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the foreign referent changed")
        expect(receipt_state(pool, foreign) == "committed", "a refused detach changed the receipt")

        never = fresh_checkout(proof, root, "detach-never-converted")
        code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach without a receipt refuses", never)
        expect(code != 0 and "nothing to detach" in stderr, stdout + stderr)
        expect(not os.path.lexists(os.path.join(never, "target")), "a refused detach created a target")
        proof.phase("after the error controls")


    cases = [
        ("detach_sequence", (), case_detach_sequence),
        ("readopt", (), case_readopt),
        ("refusals", (), case_refusals),
    ]
    finish_fixture_cases(proof, pool, mount, cases, diagnostic, "detach-permanent")
    proof.detach()


def auto_notices(stderr: str) -> list[str]:
    return [line for line in stderr.splitlines() if line.startswith(AUTO_NOTICE)]


def receipt_file(pool: str, checkout: str) -> str:
    """Where a receipt is, present or not (`receipt_path` requires it)."""
    key = hashlib.sha256(checkout.encode()).hexdigest()
    return os.path.join(pool, "oxide-managed", "outputs", key + ".json")


def fresh_checkout(proof: Proof, root: str, name: str, ignore: str | None = "/target\n") -> str:
    """A fresh checkout in its own Git work tree. `ignore` is its whole
    `.gitignore`; a first conversion needs Git to ignore the exact
    non-directory link path (oxide#125)."""
    short = "f" + hashlib.sha256(name.encode()).hexdigest()[:6]
    if " " in name:
        short += " x"
    path = write_tree(os.path.join(root, "checkouts", short), APP)
    proof.run(["git", "init", "-q"], cwd=path, timeout=30)
    if ignore is not None:
        with open(os.path.join(path, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write(ignore)
    expect(not os.path.lexists(os.path.join(path, "target")), f"{name} already has a target")
    return path


def set_mode(proof: Proof, mode: str, cwd: str) -> None:
    code, _, stderr = proof.oxide(["pool", "set-mode", mode], f"drive mode {mode}", cwd)
    expect(code == 0, stderr)


def auto_run(proof: Proof, repo: str, diagnostic: bool = False) -> None:
    """Focused auto-adopt proof (oxide#121): one fixture, one volume, the
    #117 scenario's staging and helpers. Only the fixture commands' stderr,
    exit codes and link, root, receipt and source bytes are checked."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxAd" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"auto-adopt proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-auto-adopt")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    proof.fixture_ready = True
    image = proof.image

    # The internal pool exists before any drive is registered: a build then
    # keeps its real target, as it always has.
    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "unregistered build keeps a real target", maker)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(not os.path.islink(os.path.join(maker, "target")), "unregistered build linked target")

    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    uuid = real_uuid(pool)
    wrong = "00000000-0000-0000-0000-000000000000"
    proof.phase("after drive registration")

    # 1. Eligible: a fresh checkout adopts on its first leased build. One
    #    notice, the profile layout the explicit adoption publishes (oxide#160,
    #    S4b: a real `target` holding an owned `debug`/`release` pair into one
    #    root on the drive), output through the pair, a receipt, then a warm
    #    rebuild with no notice and the same links.
    fresh = fresh_checkout(proof, root, "fresh")
    link = os.path.join(fresh, "target")
    code, _, stderr = proof.oxide(["build"], "fresh checkout adopts on its first build", fresh)
    expect(code == 0, stderr)
    notes = auto_notices(stderr)
    expect(len(notes) == 1, f"expected exactly one auto notice: {stderr}")
    expect(link in notes[0] and "no existing outputs moved" in notes[0], notes[0])
    expect(
        os.path.isdir(link) and not os.path.islink(link),
        "the fresh target is not a real directory: the first conversion did not "
        "create profile members (oxide#160, S4b)",
    )
    fresh_root = expect_profile_layout(pool, mount, fresh, "the auto-adopted fresh checkout")
    binary = os.path.join(link, "debug", "poutputs")
    expect(os.path.isfile(binary), "the binary is not reachable through the owned member link")
    expect(os.path.realpath(binary).startswith(mount + os.sep), "the binary is not on the drive")
    expect(slot_state(stderr)[1] == "cold", stderr)
    code, stdout, stderr = proof.oxide(["outputs", "status"], "output status shows the adopted root", fresh)
    expect(code == 0 and fresh_root in stdout, stdout + stderr)
    adopted_receipt = read_json(receipt_file(pool, fresh))
    expect(adopted_receipt["state"] == "committed", f"adopted receipt: {adopted_receipt}")
    code, _, stderr = proof.oxide(["build"], "adopted checkout rebuilds warm, no notice", fresh)
    expect(code == 0 and not auto_notices(stderr), stderr)
    expect(slot_state(stderr)[1] == "warm", f"adopted destination key did not reuse: {stderr}")
    expect(
        member_link_target(fresh, "debug") == os.path.join(fresh_root, "debug"),
        "a warm build relinked the debug member",
    )
    expect(
        member_link_target(fresh, "release") == os.path.join(fresh_root, "release"),
        "a warm build relinked the release member",
    )
    proof.phase("after eligible adoption")

    # A second fresh checkout gets its own root, leaf and receipt.
    second = fresh_checkout(proof, root, "second")
    code, _, stderr = proof.oxide(["build"], "second fresh checkout adopts its own root", second)
    expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
    expect(
        os.path.dirname(member_link_target(second, "debug")) != fresh_root,
        "two checkouts share a root",
    )

    def case_controls() -> None:
        # 2. Controls: every present target is kept, and no notice is printed.
        keep = fresh_checkout(proof, root, "real-target")
        os.makedirs(os.path.join(keep, "target"))
        with open(os.path.join(keep, "target", "keep.txt"), "wb") as handle:
            handle.write(b"warm build")
        code, _, stderr = proof.oxide(["build"], "a non-empty real target is never adopted", keep)
        expect(not auto_notices(stderr), stderr)
        expect(
            os.path.isdir(os.path.join(keep, "target"))
            and not os.path.islink(os.path.join(keep, "target")),
            "the real target was replaced",
        )
        expect(read_bytes(os.path.join(keep, "target", "keep.txt")) == b"warm build", "target mutated")
        expect(not os.path.exists(receipt_file(pool, keep)), "a control wrote a receipt")

        empty = fresh_checkout(proof, root, "empty-target")
        os.makedirs(os.path.join(empty, "target"))
        code, _, stderr = proof.oxide(["build"], "an empty real target is never adopted", empty)
        expect(not auto_notices(stderr) and not os.path.islink(os.path.join(empty, "target")), stderr)
        expect(not os.path.exists(receipt_file(pool, empty)), "an empty target got a receipt")

        outside = os.path.join(root, "outside")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        foreign = fresh_checkout(proof, root, "foreign-link")
        os.symlink(outside, os.path.join(foreign, "target"))
        code, _, stderr = proof.oxide(["build"], "a foreign link is refused, not adopted", foreign)
        expect(code != 0 and not auto_notices(stderr), stderr)
        expect(os.readlink(os.path.join(foreign, "target")) == outside, "foreign link was replaced")
        expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "foreign referent mutated")

        absent = os.path.join(root, "absent-root")
        dangling = fresh_checkout(proof, root, "dangling-link")
        os.symlink(absent, os.path.join(dangling, "target"))
        code, _, stderr = proof.oxide(["build"], "a dangling link is refused, not adopted", dangling)
        expect(code != 0 and not auto_notices(stderr), stderr)
        expect(os.readlink(os.path.join(dangling, "target")) == absent, "dangling link was replaced")
        expect(not os.path.lexists(absent), "the dangling link's root was created")

        filed = fresh_checkout(proof, root, "file-target")
        with open(os.path.join(filed, "target"), "wb") as handle:
            handle.write(b"not a directory")
        code, _, stderr = proof.oxide(["build"], "a file at target is never adopted", filed)
        expect(not auto_notices(stderr), stderr)
        expect(read_bytes(os.path.join(filed, "target")) == b"not a directory", "file target mutated")
        proof.phase("after controls")


    def case_corrupt_receipt() -> None:
        # 3. A corrupt receipt is an error, never an absent receipt: the command
        #    refuses before any slot is taken, and the missing target stays missing.
        bad = fresh_checkout(proof, root, "bad-receipt")
        os.makedirs(os.path.dirname(receipt_file(pool, bad)), exist_ok=True)
        corrupt = receipt_file(pool, bad)
        with open(corrupt, "wb") as handle:
            handle.write(b"{not a receipt")
        # Owner-only, so the product reaches its parse refusal, not the mode check.
        os.chmod(corrupt, 0o600)
        code, _, stderr = proof.oxide(["build"], "a corrupt receipt refuses, never adopts", bad)
        expect(code != 0 and "is invalid" in stderr and not auto_notices(stderr), stderr)
        expect(not os.path.lexists(os.path.join(bad, "target")), "a corrupt receipt let a target appear")
        os.remove(receipt_file(pool, bad))


    def case_shared_lifecycle() -> None:
        try:
            # 4. A detached checkout with its target missing is excluded: the receipt
            #    of any state refuses automatic adoption.
            det = fresh_checkout(proof, root, "detached")
            code, _, stderr = proof.oxide(["build"], "detached checkout starts adopted", det)
            expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
            set_mode(proof, "ephemeral", det)
            code, _, stderr = proof.oxide(["outputs", "detach"], "detach the checkout", det)
            expect(code == 0, stderr)
            shutil.rmtree(os.path.join(det, "target"))
            set_mode(proof, "permanent", det)
            code, _, stderr = proof.oxide(["build"], "missing target with a detached receipt is excluded", det)
            expect(code == 0 and not auto_notices(stderr), stderr)
            expect(
                os.path.isdir(os.path.join(det, "target")) and not os.path.islink(os.path.join(det, "target")),
                "a detached checkout was adopted",
            )
            with open(receipt_file(pool, det), "rb") as handle:
                expect(json.load(handle)["state"] == "detached", "the receipt left detached state")
            proof.phase("after detached exclusion")

            # 5. Ephemeral registration: no adoption, a real target, then permanent.
            set_mode(proof, "ephemeral", fresh)
            eph = fresh_checkout(proof, root, "ephemeral-drive")
            code, _, stderr = proof.oxide(["build"], "ephemeral registration never adopts", eph)
            expect(code == 0 and not auto_notices(stderr), stderr)
            expect(not os.path.islink(os.path.join(eph, "target")), "ephemeral build linked its target")
            set_mode(proof, "permanent", fresh)

            # 6. Absent drive: no adoption, the internal real target. The drive is
            #    then re-attached at the same mount point.
            force_detach(proof, image, mount, uuid)
            gone = fresh_checkout(proof, root, "absent-drive")
            code, _, stderr = proof.oxide(["build"], "an absent drive never adopts", gone)
            expect(code == 0 and not auto_notices(stderr), stderr)
            expect(not os.path.islink(os.path.join(gone, "target")), "absent drive linked its target")
            expect(not os.path.exists(receipt_file(pool, gone)), "absent drive wrote a receipt")
            mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
            expect(mounted == mount, f"re-attached at {mounted}, not {mount}")
            proof.phase("after absent drive")

            # 7. Internal fallback (recorded volume UUID mismatch): no adoption.
            set_recorded_uuid(pool, wrong)
            mismatch = fresh_checkout(proof, root, "uuid-mismatch")
            code, _, stderr = proof.oxide(["build"], "a fallback to the internal pool never adopts", mismatch)
            expect(code == 0 and not auto_notices(stderr), stderr)
            expect(not os.path.islink(os.path.join(mismatch, "target")), "fallback linked its target")
            set_recorded_uuid(pool, uuid)
            proof.phase("after fallback control")

            # 8. Crash replay. A stop after the sibling link leaves a prepared receipt;
            #    a creator that appears at the target afterwards is never replaced.
            crashed = fresh_checkout(proof, root, "crash-linked")
            crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="linked")
            code, _, stderr = oxide_env(proof, ["build"], "crash after the sibling link", crashed, crash_env)
            expect(code != 0 and "injected transition stop at linked" in stderr, stderr)
            expect(not auto_notices(stderr), "a crashed adoption printed its notice")
            # The first conversion's FIRST member transition is what crashes, so the
            # sibling link belongs to `target/debug` and the real `target` directory
            # already exists (oxide#160, S4b).
            expect(
                os.path.isdir(os.path.join(crashed, "target")),
                "the crash did not leave the profile target directory",
            )
            sibling = os.path.join(crashed, "target", ".debug.oxide-new-1")
            expect(os.path.islink(sibling), "the crash left no sibling link")
            with open(os.path.join(crashed, "target", "debug"), "wb") as handle:
                handle.write(b"creator")
            code, _, stderr = proof.oxide(["build"], "replay refuses a creator at the member link", crashed)
            expect(code != 0 and os.path.join(crashed, "target", "debug") in stderr
                   and "never follows or replaces it" in stderr, stderr)
            expect(
                read_bytes(os.path.join(crashed, "target", "debug")) == b"creator",
                "the creator was replaced",
            )
            expect(os.path.islink(sibling), "the prepared sibling was removed")
            os.remove(os.path.join(crashed, "target", "debug"))
            code, _, stderr = proof.oxide(["outputs", "adopt"], "replay after removing the creator", crashed)
            expect(code == 0, stderr)
            expect(os.path.islink(os.path.join(crashed, "target", "debug")), "replay did not publish debug")
            expect(not os.path.lexists(sibling), "replay left the prepared sibling")
            expect(read_json(receipt_file(pool, crashed))["state"] == "committed", "replay did not commit")

            # A stop before any link is published: the next command's replay rolls the
            # preparation back to detached and refuses once (its slot was keyed for the
            # root). The command after it builds as an unconverted checkout.
            prepared = fresh_checkout(proof, root, "crash-prepared")
            crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="prepared")
            code, _, stderr = oxide_env(proof, ["build"], "crash before publication", prepared, crash_env)
            expect(code != 0 and "injected transition stop at prepared" in stderr, stderr)
            expect(
                not os.path.lexists(os.path.join(prepared, "target", "debug")),
                "a stop published a member link",
            )
            code, _, stderr = proof.oxide(["build"], "replay rolls back an unpublished adoption", prepared)
            expect(code != 0 and "Run the command again" in stderr and not auto_notices(stderr), stderr)
            code, _, stderr = proof.oxide(["build"], "the command after the rollback is unconverted", prepared)
            expect(code == 0 and not auto_notices(stderr), stderr)
            expect(not os.path.islink(os.path.join(prepared, "target")), "rollback left a link")
            with open(receipt_file(pool, prepared), "rb") as handle:
                expect(json.load(handle)["state"] == "detached", "rollback did not detach")
            proof.phase("after crash replay")

        except Exception as fault:
            raise ProofAbort(f"shared fixture state uncertain: {fault}") from fault

    def case_failed_build() -> None:
        # 9. Committed adoption survives a failed build: one notice, the link and
        #    receipt stay, and a repaired source builds through them.
        failing = fresh_checkout(proof, root, "build-fails")
        with open(os.path.join(failing, "src", "main.rs"), "w", encoding="utf-8") as handle:
            handle.write('fn main() { let value: u32 = "not a number"; }\n')
        code, _, stderr = proof.oxide(["build"], "adoption survives a failed build", failing)
        expect(code != 0 and len(auto_notices(stderr)) == 1, stderr)
        expect(
            os.path.isdir(os.path.join(failing, "target"))
            and os.path.islink(os.path.join(failing, "target", "debug")),
            "the committed member links were lost",
        )
        with open(os.path.join(failing, "src", "main.rs"), "w", encoding="utf-8") as handle:
            handle.write(APP["src/main.rs"])
        code, _, stderr = proof.oxide(["build"], "the repaired source builds through the link", failing)
        expect(code == 0 and not auto_notices(stderr), stderr)
        proof.phase("after failed build")


    cases = [
        ("controls", (), case_controls),
        ("corrupt_receipt", (), case_corrupt_receipt),
        ("failed_build", (), case_failed_build),
        ("shared_lifecycle", (), case_shared_lifecycle),
    ]
    finish_fixture_cases(proof, pool, mount, cases, diagnostic, "auto-adopt")
    proof.detach()


def roots_on(mount: str) -> list[str]:
    """The output roots currently on the drive's oxide-outputs directory."""
    root = os.path.join(mount, "oxide-outputs")
    return sorted(os.listdir(root)) if os.path.isdir(root) else []


def repo_relative_target(proof: Proof, checkout: str) -> str:
    """This checkout's `target` relative to its repository root."""
    result = proof.run(["git", "rev-parse", "--show-prefix"], cwd=checkout, timeout=30)
    expect(result.returncode == 0, f"git rev-parse --show-prefix failed: {result.stderr!r}")
    return result.stdout.decode(errors="replace").strip() + "target"


def status_target_entries(raw: bytes, expected: str) -> list[str]:
    """Select the exact target or descendants, never a merely similar prefix."""
    named = []
    for part in raw.split(b"\0"):
        if not part:
            continue
        entry = part.decode(errors="replace")
        expect(len(entry) > 3 and entry[2] == " ", f"unexpected git status entry {entry!r}")
        path = entry[3:]
        if any(path == root or path.startswith(root + "/") for root in (expected, "target")):
            named.append(entry)
    return named


def git_status_names_target(proof: Proof, checkout: str) -> list[str]:
    """Return NUL-delimited status entries for the target and its members."""
    expected = repo_relative_target(proof, checkout)
    result = proof.run(
        ["git", "status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"],
        cwd=checkout,
        timeout=30,
    )
    expect(result.returncode == 0, f"git status failed in {checkout}: {result.stderr!r}")
    return status_target_entries(result.stdout, expected)


def git_index_names_target(proof: Proof, checkout: str) -> bytes:
    result = proof.run(["git", "ls-files", "-z", "--", "target"], cwd=checkout, timeout=30)
    expect(result.returncode == 0, f"git ls-files failed in {checkout}: {result.stderr!r}")
    return result.stdout


def ignore_fixture(
    proof: Proof,
    root: str,
    name: str,
    ignore: str | None,
    *,
    empty: bool = False,
    relative: str = "",
    git: bool = True,
) -> tuple[str, str]:
    """A tiny Cargo checkout in its own Git work tree. `ignore` is the whole
    repository `.gitignore`; `relative` nests the checkout under that root so
    its owned link has a nested repository-relative path."""
    repo = os.path.join(root, "ignore", "f" + hashlib.sha256(name.encode()).hexdigest()[:6])
    os.makedirs(repo)
    checkout = write_tree(os.path.join(repo, relative) if relative else repo, APP)
    if git:
        proof.run(["git", "init", "-q"], cwd=repo, timeout=30)
    if ignore is not None:
        with open(os.path.join(repo, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write(ignore)
    if empty:
        os.makedirs(os.path.join(checkout, "target"))
    expect(not os.path.islink(os.path.join(checkout, "target")), f"{name}: target is already a link")
    return repo, checkout


def refused_untouched(
    proof: Proof,
    pool: str,
    mount: str,
    checkout: str,
    label: str,
    args: list,
    reasons: tuple,
    *,
    empty: bool = False,
    env: dict | None = None,
    keeps_target: bool = False,
) -> str:
    """One refused conversion: the reason names the fix and the target, the
    receipt and the drive's roots are exactly as they were."""
    before = roots_on(mount)
    if env is None:
        code, stdout, stderr = proof.oxide(args, label, checkout)
    else:
        code, stdout, stderr = oxide_env(proof, args, label, checkout, env)
    expect(code != 0, f"{label}: nothing was refused (exit {code})\n{stdout}{stderr}")
    expect(
        any(reason in stderr for reason in reasons),
        f"{label}: unexpected refusal for {reasons}: {stderr}",
    )
    # Presentation-insensitive: the product closes some refusals with
    # "Nothing was changed" and others with "; nothing was changed".
    expect(
        "nothing was changed" in stderr.casefold(),
        f"{label}: the refusal omits the no-change promise: {stderr}",
    )
    link = os.path.join(checkout, "target")
    if empty:
        expect(
            os.path.isdir(link) and not os.path.islink(link) and os.listdir(link) == [],
            f"{label}: the real empty target changed",
        )
    elif not keeps_target:
        expect(not os.path.lexists(link), f"{label}: a target appeared")
    expect(not os.path.exists(receipt_file(pool, checkout)), f"{label}: a receipt was written")
    expect(roots_on(mount) == before, f"{label}: the drive's roots changed")
    return stderr


def expect_published(proof: Proof, pool: str, mount: str, checkout: str, label: str) -> str:
    """One accepted conversion's profile layout, receipt and Git invisibility.

    Since oxide#160 (S4b) a first conversion publishes profile members: a real
    `<checkout>/target` holding an owned `<root>/<member>` pair."""
    link = os.path.join(checkout, "target")
    expect(
        os.path.isdir(link) and not os.path.islink(link),
        f"{label}: target is not a real directory",
    )
    root = expect_profile_layout(pool, mount, checkout, label)
    expect(os.path.isdir(link), f"{label}: the converted target is missing")
    receipt = read_json(receipt_file(pool, checkout))
    expect(
        receipt["state"] == "committed" and receipt["generation"] >= 1,
        f"{label}: the receipt is not a committed first conversion: {receipt}",
    )
    expect(
        git_status_names_target(proof, checkout) == [],
        f"{label}: git status lists the published target",
    )
    expect(
        git_index_names_target(proof, checkout) == b"",
        f"{label}: git ls-files shows target tracked",
    )
    return root


def fake_git(root: str, kind: str, script: str) -> str:
    """A private `git` first on PATH for one oxide invocation."""
    directory = os.path.join(root, "fake-git", kind)
    os.makedirs(directory)
    path = os.path.join(directory, "git")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(script)
    os.chmod(path, 0o755)
    return directory


def private_path(root: str) -> str:
    """A PATH with the tools oxide needs to reach a real build - Cargo, rustc,
    rustup and the C linker it invokes - and no git at all. Without `git` the
    ignore prerequisite cannot be confirmed, which must leave the build usable
    rather than failing at link time for a missing `cc`."""
    directory = os.path.join(root, "no-git-bin")
    os.makedirs(directory)
    for tool in ("cargo", "rustc", "rustup", "cc"):
        found = shutil.which(tool)
        expect(found is not None, f"{tool} is not on the host PATH")
        os.symlink(found, os.path.join(directory, tool))
    return directory


def ignore_run(proof: Proof, repo: str, diagnostic: bool = False) -> None:
    """Focused ignore-safety proof (oxide#125, updated for oxide#160/S4b): a
    first conversion publishes owned `<checkout>/target/<member>` links only
    when Git ignores the exact repository-relative path of the link it is about
    to publish, and refuses — changing nothing at all, not even creating the
    real target directory — for a rule that re-includes the member, an
    unignored path, a tracked member, no Git, no repository, a timeout, a
    command error and a malformed answer. A directory-only rule such as
    `/target/` genuinely covers a member path inside the real target directory,
    so it is accepted; the whole-target link's stricter non-directory rule no
    longer applies to a first conversion. Explicit adopt, automatic adoption
    and the replay of a prepared first conversion are all covered, and detach
    still works."""
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxIg" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"ignore-safety proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-ignore-safety")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    proof.fixture_ready = True

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "internal pool before the drive", maker)
    expect(code == 0, stderr)

    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    proof.phase("after drive registration")

    def case_directory_rules() -> None:
        # 1. A directory-only rule genuinely covers a profile MEMBER path: the
        #    published link is `target/<member>` inside a real target directory, so
        #    `/target/`, `target/` and `**/target/` are accepted (oxide#160, S4b).
        #    The single non-directory-path rule is accepted the same way.
        for index, rule in enumerate(("/target/", "target/", "**/target/", "/target", "target")):
            _, checkout = ignore_fixture(proof, root, f"rule-{index}", rule + "\n/.oxide-outputs/\n")
            code, _, stderr = proof.oxide(
                ["outputs", "adopt"], f"explicit adopt with {rule!r}", checkout
            )
            expect(code == 0, f"{rule!r}: an explicit adopt refused the rule: {stderr}")
            expect_published(proof, pool, mount, checkout, f"explicit adopt with {rule!r}")
        for index, rule in enumerate(("/target/", "target")):
            _, checkout = ignore_fixture(proof, root, f"auto-{index}", rule + "\n/.oxide-outputs/\n")
            code, _, stderr = proof.oxide(["build"], f"automatic adoption with {rule!r}", checkout)
            expect(code == 0 and len(auto_notices(stderr)) == 1, f"{rule!r}: {stderr}")
            expect_published(proof, pool, mount, checkout, f"automatic adoption with {rule!r}")
        #    A real empty target takes the same accepted path.
        _, empty = ignore_fixture(
            proof, root, "empty-accepted", "/target/\n/.oxide-outputs/\n", empty=True
        )
        code, _, stderr = proof.oxide(["outputs", "adopt"], "explicit adopt of an empty target", empty)
        expect(code == 0, f"an empty target was refused: {stderr}")
        expect_published(proof, pool, mount, empty, "explicit adopt of an empty target")
        proof.phase("after the directory-rule matrix")


    def case_nested() -> None:
        # 2. A nested workspace names its exact repository-relative path, and a
        #    directory-only nested rule covers the nested member too.
        _, nested = ignore_fixture(
            proof,
            root,
            "nested-empty",
            "/rust/target/\n.oxide-outputs/\n",
            relative="rust",
            empty=True,
        )
        code, _, stderr = proof.oxide(["outputs", "adopt"], "nested /rust/target/", nested)
        expect(code == 0, f"the nested directory rule was refused: {stderr}")
        expect_published(proof, pool, mount, nested, "the nested directory rule")
        _, nested_missing = ignore_fixture(
            proof, root, "nested-unignored", ".oxide-outputs/\n", relative="rust"
        )
        refused_untouched(
            proof,
            pool,
            mount,
            nested_missing,
            "a nested checkout with no target rule",
            ["outputs", "adopt"],
            ("does not ignore",),
        )
        expect(
            not os.path.lexists(os.path.join(nested_missing, "target")),
            "a refused nested conversion created a target",
        )


    def case_negated() -> None:
        # 3. Negated and unignored paths refuse; nothing links.
        _, negated = ignore_fixture(
            proof, root, "negated", "/target/*\n!/target/debug\n/.oxide-outputs/\n"
        )
        stderr = refused_untouched(
            proof,
            pool,
            mount,
            negated,
            "a rule that re-includes the debug member",
            ["outputs", "adopt"],
            ("re-includes",),
        )
        expect("target/debug" in stderr, stderr)
        _, unignored = ignore_fixture(proof, root, "unignored", "/.oxide-outputs/\n")
        refused_untouched(
            proof,
            pool,
            mount,
            unignored,
            "an unignored target",
            ["outputs", "adopt"],
            ("does not ignore",),
        )


    def case_tracked() -> None:
        # 4. A tracked path refuses even when the working tree has no target.
        _, tracked = ignore_fixture(proof, root, "tracked", "/target\n/.oxide-outputs/\n")
        os.makedirs(os.path.join(tracked, "target"))
        with open(os.path.join(tracked, "target", "debug"), "wb") as handle:
            handle.write(b"tracked")
        added = proof.run(["git", "add", "-f", "target/debug"], cwd=tracked, timeout=30)
        expect(added.returncode == 0, f"git add failed: {added.stderr!r}")
        os.remove(os.path.join(tracked, "target", "debug"))
        stderr = refused_untouched(
            proof,
            pool,
            mount,
            tracked,
            "a tracked member path",
            ["outputs", "adopt"],
            ("already tracks",),
            empty=True,
        )
        expect("target/debug" in stderr, stderr)


    def case_unknown_git() -> None:
        # 5. Unknown Git is never permission: no repository, no Git binary, a
        #    timeout, a command error and a malformed answer all refuse.
        _, no_repo = ignore_fixture(proof, root, "no-repository", "/target\n", git=False)
        refused_untouched(
            proof,
            pool,
            mount,
            no_repo,
            "no Git repository",
            ["outputs", "adopt"],
            ("could not confirm",),
        )
        no_git_env = dict(proof.env, PATH=private_path(root))
        # A leased *build* in this class is ineligibility, not a refusal
        # (oxide#154), so this control asks the explicit conversion over a real
        # empty target adopt never creates: the member's missing prerequisite
        # refuses before any receipt, root or link, and the target, receipt and
        # drive roots are unchanged.
        _, no_git = ignore_fixture(
            proof, root, "no-git", "/target\n/.oxide-outputs/\n", empty=True
        )
        refused_untouched(
            proof,
            pool,
            mount,
            no_git,
            "no Git binary on PATH",
            ["outputs", "adopt"],
            ("could not confirm", "cannot run git"),
            env=no_git_env,
            empty=True,
        )
        _, slow = ignore_fixture(proof, root, "slow-git", "/target\n/.oxide-outputs/\n")
        slow_dir = fake_git(root, "timeout", "#!/bin/sh\nexec sleep 30\n")
        refused_untouched(
            proof,
            pool,
            mount,
            slow,
            "a Git that does not answer in time",
            ["outputs", "adopt"],
            ("did not finish in time",),
            env=dict(proof.env, PATH=slow_dir + ":" + proof.env["PATH"]),
        )
        real_git = shutil.which("git")
        expect(real_git is not None, "git is not on the host PATH")
        _, invalid = ignore_fixture(proof, root, "invalid-git", "/target\n/.oxide-outputs/\n")
        invalid_dir = fake_git(
            root,
            "invalid",
            "#!/bin/sh\ncase \"$*\" in\n  *check-ignore*) printf 'not-a-rule'; exit 0 ;;\nesac\n"
            f'exec "{real_git}" "$@"\n',
        )
        refused_untouched(
            proof,
            pool,
            mount,
            invalid,
            "a malformed Git ignore answer",
            ["outputs", "adopt"],
            ("not one complete record",),
            env=dict(proof.env, PATH=invalid_dir + ":" + proof.env["PATH"]),
        )
        _, failing = ignore_fixture(proof, root, "failing-git", "/target\n/.oxide-outputs/\n")
        failing_dir = fake_git(
            root,
            "error",
            "#!/bin/sh\ncase \"$*\" in\n  *check-ignore*) exit 128 ;;\nesac\n"
            f'exec "{real_git}" "$@"\n',
        )
        refused_untouched(
            proof,
            pool,
            mount,
            failing,
            "a failing Git ignore call",
            ["outputs", "adopt"],
            ("did not answer",),
            env=dict(proof.env, PATH=failing_dir + ":" + proof.env["PATH"]),
        )
        proof.phase("after the unknown-Git controls")


    def case_present_target() -> None:
        # 6. Present but unadoptable targets keep their content and are not
        #    covered by this guard.
        _, nonempty = ignore_fixture(proof, root, "nonempty", "/target\n/.oxide-outputs/\n")
        os.makedirs(os.path.join(nonempty, "target"))
        with open(os.path.join(nonempty, "target", "keep.txt"), "wb") as handle:
            handle.write(b"warm build")
        code, _, stderr = proof.oxide(["outputs", "adopt"], "a nonempty target keeps its file", nonempty)
        expect(code == 0, f"adopt refused a nonempty target: {stderr}")
        expect(
            read_bytes(os.path.join(nonempty, "target", "keep.txt")) == b"warm build",
            "the top-level target file was moved",
        )
        expect(os.path.isdir(os.path.join(nonempty, "target")), "the real target was replaced")
        expect_published(proof, pool, mount, nonempty, "a nonempty target's members")

        outside = os.path.join(root, "outside")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        _, foreign = ignore_fixture(proof, root, "foreign", "/target\n/.oxide-outputs/\n")
        os.symlink(outside, os.path.join(foreign, "target"))
        code, _, stderr = proof.oxide(["outputs", "adopt"], "a foreign link is kept", foreign)
        expect(code != 0 and "never replaces it" in stderr, stderr)
        expect(os.readlink(os.path.join(foreign, "target")) == outside, "the foreign link was replaced")
        expect(read_bytes(os.path.join(outside, "sentinel")) == b"foreign", "the referent mutated")

        absent = os.path.join(root, "absent-root")
        _, dangling = ignore_fixture(proof, root, "dangling", "/target\n/.oxide-outputs/\n")
        os.symlink(absent, os.path.join(dangling, "target"))
        code, _, stderr = proof.oxide(["outputs", "adopt"], "a dangling link is kept", dangling)
        expect(code != 0 and "left untouched" in stderr, stderr)
        expect(os.readlink(os.path.join(dangling, "target")) == absent, "the dangling link was replaced")
        expect(not os.path.lexists(absent), "the dangling link's root was created")
        proof.phase("after present-target controls")


    def case_replay() -> None:
        # 7. Prepared first-conversion replay rechecks the prerequisite: a rule
        #    removed after the crash refuses to replay and keeps the prepared
        #    receipt; restoring the rule replays and commits.
        _, replay = ignore_fixture(proof, root, "replay", "/target\n/.oxide-outputs/\n")
        crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="prepared")
        code, _, stderr = oxide_env(proof, ["outputs", "adopt"], "crash after the prepared receipt", replay, crash_env)
        expect(code != 0 and "injected transition stop at prepared" in stderr, stderr)
        link = os.path.join(replay, "target", "debug")
        expect(not os.path.lexists(link), "the crash published a member link")
        prepared = read_json(receipt_file(pool, replay))
        expect(prepared["state"] == "prepared", prepared)
        expect(prepared["layout"] == "profile", prepared)
        before_roots = roots_on(mount)
        with open(os.path.join(replay, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/.oxide-outputs/\n")
        code, _, stderr = proof.oxide(["outputs", "adopt"], "replay refuses an unignored target", replay)
        expect(code != 0 and "does not ignore" in stderr, stderr)
        expect(not os.path.lexists(link), "the refused replay published a link")
        expect(
            read_json(receipt_file(pool, replay))["state"] == "prepared",
            "the refused replay rewrote the prepared receipt",
        )
        expect(roots_on(mount) == before_roots, "the refused replay created a root")
        with open(os.path.join(replay, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n/.oxide-outputs/\n")
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], "replay after restoring the rule", replay)
        expect(code == 0 and "now selects" in stdout, stdout + stderr)
        replay_root = expect_published(proof, pool, mount, replay, "the replayed first conversion")
        expect(os.path.isdir(replay_root), "the replayed root is missing")
        proof.phase("after the replay case")


    def case_accepted_detach() -> None:
        try:
            # 8. Accepted publications: an automatic first conversion, an explicit one
            #    and a nested workspace all hide the link from Git.
            _, automatic = ignore_fixture(proof, root, "auto-accepted", "/target\n/.oxide-outputs/\n")
            code, _, stderr = proof.oxide(["build"], "automatic adoption publishes an ignored link", automatic)
            expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
            expect_published(proof, pool, mount, automatic, "the automatic first conversion")
            expect(
                os.path.isfile(os.path.join(automatic, "target", "debug", "poutputs")),
                "the automatic conversion built nothing through the link",
            )

            _, explicit = ignore_fixture(proof, root, "explicit-accepted", "target\n/.oxide-outputs/\n")
            code, stdout, stderr = proof.oxide(["outputs", "adopt"], "explicit adoption with `target`", explicit)
            expect(code == 0 and "now selects" in stdout, stdout + stderr)
            explicit_root = expect_published(proof, pool, mount, explicit, "the explicit first conversion")

            nested_repo, nested_ok = ignore_fixture(
                proof, root, "nested-accepted", "/rust/target\n.oxide-outputs/\n", relative="rust"
            )
            code, stdout, stderr = proof.oxide(["outputs", "adopt"], "nested /rust/target", nested_ok)
            expect(code == 0 and "now selects" in stdout, stdout + stderr)
            expect_published(proof, pool, mount, nested_ok, "the nested first conversion")
            # The nested check is non-vacuous: a wholly untracked workspace collapses to
            # `rust/` in plain `git status`, but with every untracked file listed and
            # exact NUL-delimited paths, removing the rule exposes `rust/target`.
            nested_ignore = os.path.join(nested_repo, ".gitignore")
            nested_saved = read_bytes(nested_ignore)
            with open(nested_ignore, "wb") as handle:
                handle.write(b"")
            try:
                expected = repo_relative_target(proof, nested_ok)
                exposed = git_status_names_target(proof, nested_ok)
                expect(
                    all(any(entry[3:] in (expected + "/" + member, "target/" + member)
                            for entry in exposed) for member in ("debug", "release")),
                    f"the nested detector missed an exposed member: {exposed}",
                )
            finally:
                with open(nested_ignore, "wb") as handle:
                    handle.write(nested_saved)
            expect_published(proof, pool, mount, nested_ok, "the nested first conversion restored")
            proof.phase("after accepted publications")

            # 9. Detach still works for an existing owned link.
            set_mode(proof, "ephemeral", explicit)
            code, stdout, stderr = proof.oxide(["outputs", "detach"], "detach an owned ignored link", explicit)
            expect(code == 0 and "real empty directory again" in stdout, stdout + stderr)
            detached = os.path.join(explicit, "target")
            expect(os.path.isdir(detached) and not os.path.islink(detached), "detach left a link")
            for member in ("debug", "release"):
                path = os.path.join(detached, member)
                expect(
                    os.path.isdir(path) and not os.path.islink(path),
                    f"detach did not restore a real {member} directory",
                )
                expect(os.listdir(path) == [], f"the detached {member} is not empty")
            expect(os.path.isdir(explicit_root), "detach deleted the output root")
            set_mode(proof, "permanent", explicit)
            proof.phase("after detach")
        except Exception as fault:
            raise ProofAbort(f"shared fixture state uncertain: {fault}") from fault

    cases = [
        ("directory_rules", (), case_directory_rules),
        ("nested", (), case_nested),
        ("negated", (), case_negated),
        ("tracked", (), case_tracked),
        ("unknown_git", (), case_unknown_git),
        ("present_target", (), case_present_target),
        ("replay", (), case_replay),
        ("accepted_detach", (), case_accepted_detach),
    ]
    finish_fixture_cases(proof, pool, mount, cases, diagnostic, "ignore-safety")
    proof.detach()


# Premise 7 (HIGH checklist): the staged binary and every fixture child build
# under ONE frozen compiler-flag environment. `layout.stage_at` copies
# `os.environ`, so the freeze is applied to the process environment before
# staging and then to the fixture environment, which is built from the same
# values. `CARGO_ENCODED_RUSTFLAGS` is removed everywhere: it takes precedence
# over `RUSTFLAGS`, so an inherited value would unfreeze the "empty" flag set.
FROZEN_RUSTFLAGS = ""


def freeze_compiler_flags(env) -> None:
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    env["RUSTFLAGS"] = FROZEN_RUSTFLAGS


# Premise 6 (HIGH checklist): every attach and reattach of this proof's own
# image requests filesystem owners. The product registration that follows is
# the second control: it refuses an ignore-ownership mount.
def owners_honored(proof: Proof, mount: str, label: str) -> None:
    """Bounded read-only observation that the private image's mounted root does
    not carry the ignore-ownership flag."""
    result = proof.run(["/sbin/mount"], timeout=30)
    expect(result.returncode == 0, f"{label}: mount listing failed: {result.stderr!r}")
    line = next(
        (
            entry
            for entry in result.stdout.decode(errors="replace").splitlines()
            if f" on {mount} " in entry
        ),
        None,
    )
    expect(line is not None, f"{label}: {mount} is not in the mount listing")
    expect(
        "noowners" not in line,
        f"{label}: {mount} was mounted with owners ignored (MNT_IGNORE_OWNERSHIP): {line}",
    )


# The fixed exec-inference case table: `(case, dependencies, description)`.
# `--plan` prints exactly this and touches nothing. `exec_run` binds one body
# to each case id and runs them in this order; the dependencies decide what a
# keep-going diagnostic must skip, so a case whose dependency did not complete
# is recorded `skipped` and never runs against a premise its dependency was
# supposed to establish (oxide#165's apparatus correction).
EXEC_CASES: list[tuple[str, tuple[str, ...], str]] = [
    ("s4b", (), "a fresh eligible checkout's first leased build creates the profile layout, an exec with no --output places its output in the root, and an existing whole-target fixture (case 09) stays whole-target"),
    ("01", (), "explicit adoption publishes the same profile layout"),
    ("02", ("01",), "exec without --output runs a tool that builds through the owned pair"),
    ("03", ("02",), "a repeat reuses the same warm slot and relinks nothing"),
    ("04", ("01",), "the tool's release build is placed under the same root"),
    ("05", ("02",), "the declared --output override reaches the same keyed slot and placement"),
    ("06", ("01",), "a member the receipt does not place stays real and local"),
    ("07+14", ("01",), "exec on a detached checkout runs locally; detach restored real empty members and kept the root"),
    ("08", (), "an ineligible (empty-target) checkout is never converted"),
    ("09", (), "an existing whole-target checkout stays whole-target through a build and an exec"),
    ("16", ("09",), "a whole-target root holding only debug migrates so both member directories exist before any release build, and a derived exec runs"),
    ("17", ("16",), "an exec with no --output repairs a release member link an older migration left dangling, and its release build lands in the root"),
    ("18", ("17",), "a link into an unverified root still refuses and creates nothing"),
    ("13", (), "a checkout path containing a space converts and runs end to end"),
    ("10", (), "a foreign member link refuses before Cargo and changes nothing"),
    ("12", (), "root drift refuses and names the explicit adoption"),
    ("11", (), "a receipt changed while the command waits refuses"),
    ("15", (), "Ctrl-C reaches the tool group, the lease holds until the group closes, then releases"),
    ("19", (), "a committed version-1 whole-target receipt makes every failing reader refuse naming adopt (recorded status reads nothing; --measure reports outputs unknown naming adopt), an explicit adopt upgrades it through the shipped migration with the old outputs intact, and a noncommitted version-1 control keeps its bytes"),
    ("20", (), "a converted checkout whose target directory was deleted recreates it: the next leased build exits 0 with its own member link back inside the same root and the root's old outputs intact, a derived exec then places the whole pair there, and the receipt still names that root"),
    ("21", (), "an external subcommand (cargo-oxfixture on a private PATH) derives the same receipt-keyed pair as exec, shares exec's warm slot, exits 0, writes its build through the owned debug member, keeps target/debug a link, reports its own non-member directory once with the bounded tool-directories notice and no unplaced-member notice, and keeps the receipt bytes"),
    ("22", (), "an external subcommand in a profile checkout whose receipt records a real member (detach, content, re-adopt) prints that one bounded unplaced-member notice naming only the receipt's member, beside the separate tool-directories notice for its own directory"),
    ("23", (), "the same external subcommand is unchanged in a whole-target checkout (link and receipt bytes), and a foreign member link still refuses before the tool runs"),
    ("24", (), "a whole-target checkout whose root is gone plans a FUTURE root, and the external subcommand reserves that root's keyed nonce first and binds the local fallback instead of refusing with no nonce"),
    ("25", (), "the tool-directory notice appears only on the receipt-derived profile layout: a never-converted real target and an ephemeral real target run the external subcommand, write their own directory locally, and print no tool-directories notice; the ephemeral drive mode is restored only after the child and every oracle completed, and a fault retains the fixture state and the first abort"),
]


def plan_exec_inference() -> None:
    """The effect-free table and allocation, printed by `--plan`."""
    print("exec-inference (oxide#160): S1 derived exec member sets + S4b profile first conversion")
    print(
        "supervisor: python3 scripts/bounded-test.py --limit 300 "
        "--scratch-budget-mib 512 -- python3 scripts/test-permanent-outputs.py exec-inference"
    )
    print("limits: 300 s wall clock, 512 MiB monitored scratch, one owned 128 MiB APFS image")
    print("        attached with hdiutil -owners on, ordinary detach")
    print("root: supervisor-selected scratch; private OXIDE_SCRATCH_ROOT and explicit 64 MiB pool cap")
    print("        fallback to /private/tmp exercises the internal path, not scratch-pool coverage")
    print("        directory-pool children require same-device writable TMPDIR; image slot scratch is at most 80 bytes")
    print("        image-backed removable pool is registered as the operating pool afterwards")
    print("fixture work, sequential, one Cargo child at a time: 4 Cargo jobs for the single")
    print("        staging and 1 per fixture child; planning demand 4 CPU / 6 GiB")
    print("cases (counted fail-fast; the diagnostic records each one, in this order):")
    for case, requires, text in EXEC_CASES:
        depends = f" [depends on {', '.join(requires)}]" if requires else ""
        print(f"  {case}: {text}{depends}")
    print("nothing was created, mounted, spawned or written")


# Phase labels this script reached, for the uncounted diagnostic's bounded
# receipt. The counted run uses the proof's own phase checks.
PHASES: list[str] = []


def phase(proof: Proof, label: str) -> None:
    """Sample one phase checkpoint and record its label.

    The shared `Proof.phase` reads the run's monitored resources and fails when
    the host reserve is gone or the owned scratch is over its budget. That is a
    resource failure of the RUN, never a case-oracle failure, so it is raised as
    a `ProofAbort` here: a keep-going collection must stop, not spend another
    case on a host or scratch state the supervisor would refuse.
    """
    PHASES.append(label)
    try:
        proof.phase(label)
    except ProofAbort:
        raise
    except RuntimeError as fault:
        raise ProofAbort(f"a monitored resource checkpoint failed at {label}: {fault}") from fault


class ProofAbort(RuntimeError):
    """An outcome a keep-going collection must not continue past: an unknown
    result, a lost resource or writer custody, a tainted shared fixture, or an
    exhausted guard deadline. A recoverable case-oracle failure is the ordinary
    `RuntimeError` an `expect` raises, which does not stop the collection."""


class IneligibleAbort(ProofAbort):
    """The auto-ineligible mode's historical name for the abort class above."""


# The work margin a collection keeps before it will start another case.
CASE_RESERVE_S = 5.0


class Collection:
    """The bounded record of one exec-inference run: one outcome row per case,
    the recoverable failures, the latched abort and the detach disposition.

    It is filled IN PLACE, so a fault that ends the run early — a cancellation,
    an interrupt, a fault that escapes the classification — still leaves every
    case already reached and every failure already recorded for the receipt.
    """

    def __init__(self) -> None:
        self.outcomes: dict[str, dict] = {}
        self.failures: list[dict] = []
        self.aborted: str | None = None
        self.detached = "not attempted (the collection did not reach the detach decision)"
        self.mount_present: bool | None = None
        self.reattach: str | None = None
        self.source: str | None = None


def detach_decision(collection: "Collection", unclosed) -> str | None:
    """Why this run must NOT make its ordinary detach, or `None` when it may.

    The diagnostic detaches only a clean collection whose custody is positively
    whole. A recorded failure, a latched abort or any unclosed child means the
    image (or the writer that outlived its leader) is not ours to touch, so the
    resource is retained and the receipt names the refusal (oxide#164 review).
    """
    if collection.failures:
        return "the collection recorded a failure"
    if collection.aborted is not None:
        return f"the collection aborted: {collection.aborted}"[:600]
    if unclosed:
        return f"unclosed children {sorted(unclosed)}"
    return None


def uncertain_outcome(failure: BaseException) -> bool:
    """Whether a fault that escaped the collector leaves custody unknown, so
    BOTH modes latch it as the collection's abort (oxide#164 review).

    A recoverable case-oracle failure is the ordinary `RuntimeError` an `expect`
    raises; custody is unaffected, so the counted run keeps its shipped cleanup.
    A `ProofAbort` (unknown outcome, lost resource or custody, a failed resource
    checkpoint, an abnormal child exit) and every non-`RuntimeError` fault (an
    interrupt, a signalled helper, an unexpected exception) leave the image, its
    device or its writers in an unknown state: nothing may operate on them again.
    """
    if isinstance(failure, ProofAbort):
        return True
    return not isinstance(failure, RuntimeError)


def run_ordinary_detach(proof: Proof, collection: "Collection") -> None:
    """Make this run's ONE ordinary detach and latch its failure in either mode.

    A detach that failed, timed out or ended by a signal (for example `hdiutil`
    killed with an empty direct-child census) leaves the device in an unknown
    state. A second detach must never be attempted, so the fault both latches the
    collection — which the failure cleanup's retention gate reads — and
    propagates unchanged.
    """
    try:
        proof.detach()
    except BaseException as fault:
        collection.aborted = f"the ordinary detach could not be confirmed: {fault!r}"[:600]
        raise


def detach_disposition(collection: "Collection", unclosed) -> str:
    """The disposition to record when a run ends before its own detach decision.

    It reads the SAME gate the clean tail and the failure cleanup use, so the
    receipt never claims a resource state the cleanup contradicts: a retained
    image is named as retained, and a clean run that failed before its decision
    says so instead of implying nothing was attempted.
    """
    refusal = detach_decision(collection, unclosed)
    if refusal is not None:
        return f"retained: {refusal}"
    return "not reached: the run failed before its own detach decision"


def require_case_custody(*, unclosed, mount: str, registration: str, case: str) -> None:
    """The shared post-case custody check of a keep-going collection.

    An unclosed child still holds the lease, a lost mount is a lost resource and
    a vanished drive registration is lost custody: each is a `ProofAbort`, never
    a recoverable case-oracle failure, because no later case may be trusted
    after it. Read-only; it creates nothing.
    """
    if unclosed:
        raise ProofAbort(f"unclosed children after {case}: {sorted(unclosed)}")
    if not os.path.ismount(mount):
        raise ProofAbort(f"the owned image is no longer mounted at {mount} after {case}")
    if not os.path.exists(registration):
        raise ProofAbort(f"the drive registration is gone after {case}")


def require_case_resources(*, peak_bytes: int, low_bytes: int | None, case: str) -> None:
    """The run's own phase samples are checkpoints of its monitored resources.

    The limits are the shared runner's own constants: owned scratch over the
    512 MiB budget, or host free space below the 20 GiB reserve, is a resource
    failure of the RUN — an abort, never a recoverable case-oracle failure — so
    no later case may run against it. Read-only; it creates nothing.
    """
    limit = layout.runner.SCRATCH_LIMIT_BYTES
    reserve = layout.runner.HOST_RESERVE_BYTES
    if peak_bytes > limit:
        raise ProofAbort(
            f"owned scratch {peak_bytes // MIB} MiB exceeds the {limit // MIB} MiB budget after {case}"
        )
    if low_bytes is not None and low_bytes < reserve:
        raise ProofAbort(
            f"host free {low_bytes // MIB} MiB is below the {reserve // MIB} MiB reserve after {case}"
        )


def where_of(error):
    """Bounded innermost traceback function and line for an abort record.

    The original exception is preserved; this only augments the diagnostic.
    No traceback, a malformed one or a frame lookup failure reports `?`.
    """
    try:
        traceback = error.__traceback__
        for _ in range(64):
            if traceback is None or traceback.tb_next is None:
                break
            traceback = traceback.tb_next
        if traceback is None:
            return "?"
        return f"{traceback.tb_frame.f_code.co_name}:{traceback.tb_lineno}"[:120]
    except BaseException:
        return "?"


def collect_cases(
    cases, *, diagnostic: bool, work_left, after_case=None, note=None, record=None
):
    """Run one case table, recording every case's outcome (oxide#165).

    `cases` is the ordered `(case, dependencies, body)` list the proof declares.
    The counted run is fail-fast: the first fault propagates unchanged, exactly
    as the proof's counted contract requires. `diagnostic` mode keeps going: a
    recoverable case-oracle failure is recorded as `failed` and every case whose
    dependencies all completed still runs; a case with an incomplete dependency
    is recorded `skipped` with those ids and never runs; a `ProofAbort` (an
    unknown outcome, a lost resource or custody, a failed resource checkpoint, a
    tainted shared fixture) and any other unexpected exception are recorded as
    `aborted` and every remaining case is `unexecuted`, because no later case
    may be trusted after them. An exhausted guard deadline aborts the same way
    in either mode.

    `after_case` is the run's shared post-case seam: writer and device custody
    plus the run's own resource samples. It runs after EVERY case that has not
    already aborted the collection, including a case whose own oracle failed,
    because such a case can leave an unclosed child, a lost mount or an
    over-budget scratch footprint behind.

    `record` is the caller's `Collection` and is filled IN PLACE, so a fault that
    ends the run early (a cancellation, an interrupt, anything that escapes the
    classification) still leaves every case already reached and every failure
    already recorded in it; a case whose outcome is unknown is rendered
    `unexecuted` and is never a pass. The case a cancellation interrupted is
    recorded `aborted` with its fault before the cancellation is re-raised, so an
    executed case is never misreported as unexecuted. Returns the record.
    """
    if note is None:
        note = lambda text: print(text, flush=True)  # noqa: E731 - the default sink
    if record is None:
        record = Collection()

    completed: set[str] = set()
    for case, requires, body in cases:
        if record.aborted is not None:
            record.outcomes[case] = {"result": "unexecuted"}
            continue
        missing = [dependency for dependency in requires if dependency not in completed]
        if missing:
            note(f"case {case}: skipped (depends on {', '.join(missing)})")
            record.outcomes[case] = {"result": "skipped", "because": missing}
            continue
        if work_left() <= CASE_RESERVE_S:
            record.aborted = f"{case}: no guard work deadline left before the case started"
            record.outcomes[case] = {"result": "aborted", "fault": record.aborted[:600]}
            continue
        note(f"case {case}")
        result: dict = {"result": "completed"}
        try:
            try:
                body()
            except ProofAbort as fault:
                if not diagnostic:
                    raise
                result = {"result": "aborted", "fault": str(fault)[:600]}
            except RuntimeError as fault:
                if not diagnostic:
                    raise
                if "deadline reached" in str(fault) or work_left() <= CASE_RESERVE_S:
                    # An exhausted deadline is not a recoverable oracle failure.
                    result = {"result": "aborted", "fault": str(fault)[:600]}
                else:
                    record.failures.append({"case": case, "fault": str(fault)[:600]})
                    result = {"result": "failed", "fault": str(fault)[:600]}
            except Exception as fault:
                # Any other exception is an unknown outcome, never a recoverable
                # case-oracle failure, so it aborts like the custody seam below.
                if not diagnostic:
                    raise
                result = {
                    "result": "aborted",
                    "fault": f"unexpected {fault!r} at {where_of(fault)}"[:600],
                }
            # Custody and the run's own resource samples are checked after the
            # case, whatever its own oracle did.
            if after_case is not None and result["result"] != "aborted":
                try:
                    after_case(case)
                except ProofAbort as fault:
                    if not diagnostic:
                        raise
                    result = {"result": "aborted", "fault": str(fault)[:600]}
        except BaseException as fault:
            # A cancellation (SIGTERM/SIGINT) or any fault that escapes the
            # classification above ends the run. The case it interrupted is
            # recorded as `aborted` and everything already recorded stays in the
            # receipt; the fault itself is re-raised, because the run must still
            # end the way the supervisor saw it.
            record.outcomes[case] = {
                "result": "aborted",
                "fault": f"ended during the case: {fault!r}"[:600],
            }
            record.aborted = f"{case}: ended during the case: {fault!r}"
            raise
        record.outcomes[case] = result
        if result["result"] == "completed":
            completed.add(case)
        elif result["result"] == "aborted":
            record.aborted = f"{case}: {result['fault']}"
    return record



FIXTURE_GROUPS = {
    "detach-permanent": ("detach_sequence", "readopt", "refusals"),
    "auto-adopt": ("controls", "corrupt_receipt", "failed_build", "shared_lifecycle"),
    "ignore-safety": ("directory_rules", "nested", "negated", "tracked", "unknown_git", "present_target", "replay", "accepted_detach"),
}


def validate_fixture_groups(mode: str, cases) -> None:
    expect(tuple(case for case, _, _ in cases) == FIXTURE_GROUPS[mode], f"{mode}: case table drifted")
    expect(all(callable(body) for _, _, body in cases), f"{mode}: a case body is not callable")


def finish_fixture_cases(proof: Proof, pool: str, mount: str, cases, diagnostic: bool, mode: str) -> None:
    """Collect independent fixture groups without proceeding on unknown custody."""
    validate_fixture_groups(mode, cases)
    collection = proof.collection
    if diagnostic:
        guard_writer_custody(proof)

    def after_case(case: str) -> None:
        if proof.unclosed or not proof.mount or not os.path.ismount(proof.mount):
            raise ProofAbort(f"{case}: image or writer custody is not closed")
        if not os.path.isfile(registration_path(pool)):
            raise ProofAbort(f"{case}: removable registration is absent")
        phase(proof, f"after {case}")

    collect_cases(cases, diagnostic=diagnostic, work_left=guard_work_left,
                  after_case=after_case, record=collection)
    if diagnostic and (collection.failures or collection.aborted):
        fail("fixture diagnostic did not complete cleanly")


def exec_case_rows(outcomes: dict[str, dict]) -> list[dict]:
    """One row per declared case, in table order. A case with no recorded
    outcome — a collection that never started, or one an abort cut short — is
    `unexecuted`, which is never a pass."""
    return [
        {"case": case, **outcomes.get(case, {"result": "unexecuted"})}
        for case, _requires, _text in EXEC_CASES
    ]


def diagnostic_document(
    *,
    source: str | None,
    rows: list[dict],
    failures: list[dict],
    aborted: str | None,
    detached: str,
    mount_present: bool | None,
    reattach: str | None,
    elapsed_s: float,
    peak_owned_mib: int,
    host_free_min_mib: int | None,
    unclosed: list[str],
) -> dict:
    """The uncounted diagnostic's bounded receipt. It is `qualified: false` and
    never a counted native PASS: it records every declared case's outcome, the
    recoverable failures, the latched abort (if any) and this run's detach
    disposition so a later counted run has evidence of what actually ran."""
    summary = {
        result: sum(1 for row in rows if row["result"] == result)
        for result in ("completed", "failed", "skipped", "aborted", "unexecuted")
    }
    return {
        "qualified": False,
        "mode": "exec-inference",
        "kind": (
            "uncounted keep-going collection: independent cases continue after a "
            "recoverable case-oracle failure"
        ),
        "source": source,
        "cases": rows,
        "summary": summary,
        "failures": failures,
        "aborted": aborted,
        "phases": PHASES,
        "observations": {
            "elapsed_s": elapsed_s,
            "peak_owned_mib": peak_owned_mib,
            "host_free_min_mib": host_free_min_mib,
            "detach": detached,
            "mount_path_present_after_detach": mount_present,
            "reattach": reattach,
            "unclosed_children": unclosed,
        },
        "limits": {
            "scratch_root": os.environ.get("OXIDE_TASK_SCRATCH_ROOT"),
            "work_deadline_ns": os.environ.get("OXIDE_PROOF_DEADLINE_NS"),
            "supervisor_scratch_budget_mib": 512,
            "wall_clock_s": 300,
        },
        "limitation": (
            "A recoverable case-oracle failure is recorded and the cases that do "
            "not depend on it still run; a case with a failed dependency is "
            "skipped, and an abort leaves every remaining case unexecuted. A "
            "skipped or unexecuted case is never a pass, and this receipt is "
            "never a counted native PASS."
        ),
    }


def finish_diagnostic(proof: Proof, collection: Collection, failure, writer=None) -> None:
    """Write the bounded receipt of one diagnostic run, in every outcome.

    A diagnostic whose receipt was not written is NOT a completed collection, so
    a receipt failure ends the run when it is the only fault. When a case fault
    is already in flight the receipt failure cannot replace it: it is reported
    and the primary failure is preserved.
    """
    write = write_diagnostic if writer is None else writer
    try:
        write(proof, collection)
    except BaseException as receipt_failure:  # noqa: BLE001 - decided below
        if failure is None:
            raise
        print(
            f"exec-inference diagnostic receipt was not written: {receipt_failure!r} "
            f"(the primary failure is preserved)",
            file=sys.stderr,
            flush=True,
        )


def write_diagnostic(proof: Proof, collection: Collection) -> str:
    """Write the bounded receipt of one keep-going collection, in every outcome,
    including an abort or a setup failure before the first case ran."""
    if collection.detached.startswith("not attempted"):
        # The run ended before its own detach decision (a setup failure, an
        # abort, an interrupt): record what the retention gate says the failure
        # cleanup will do, so the receipt and the actual resource state agree.
        collection.detached = detach_disposition(collection, proof.unclosed)
    document = diagnostic_document(
        source=collection.source,
        rows=exec_case_rows(collection.outcomes),
        failures=collection.failures,
        aborted=collection.aborted,
        detached=collection.detached,
        mount_present=collection.mount_present,
        reattach=collection.reattach,
        elapsed_s=round(time.monotonic() - layout.runner.STARTED, 1),
        peak_owned_mib=proof.peak // MIB,
        host_free_min_mib=None if proof.low is None else proof.low // MIB,
        unclosed=sorted(proof.unclosed),
    )
    text = json.dumps(document, indent=2, sort_keys=True)[: 64 * 1024]
    name = "exec-inference-diagnostic.json"
    handle = open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8")
    with handle:
        handle.write(text)
    print(
        f"exec-inference diagnostic receipt {os.environ['OXIDE_PROOF_EVIDENCE']}/{name} "
        f"(qualified=false, {document['summary']})",
        flush=True,
    )
    return name


def exec_self_test() -> None:
    """The pure apparatus check of the keep-going collection.

    It drives the real collector, the real case table, the real custody and
    resource gates and the real receipt document builder with fake bodies and
    seams: no scratch, child, toolchain, image, pool or supervisor is touched. A
    dispatch, dependency-skip, keep-going, abort-latch, cancellation,
    resource-gate or receipt regression is therefore found without spending a
    native activation on it.
    """
    # Pure environment contract: inherited host configuration cannot override the
    # private pool, its explicit cap or the supervisor-selected scratch root.
    from unittest.mock import patch
    with patch.dict(os.environ, {"OXIDE_TASK_SCRATCH_ROOT": "/fixture/root", "OXIDE_POOL": "/host/pool", "OXIDE_SCRATCH_ROOT": "/host"}, clear=True):
        env = proof_env("/fixture/root/private/pool", "/fixture/root/private/home", "/fixture/root/private/cargo")
    expect(env["OXIDE_SCRATCH_ROOT"] == "/fixture/root/private", "scratch declaration drifted")
    expect(env["OXIDE_POOL"] == "/fixture/root/private/pool", "pool selection drifted")
    expect(env["OXIDE_POOL_CAP"] == "64MiB", "pool cap drifted")
    with patch("os.path.isdir", return_value=True), patch("os.access", return_value=True), \
         patch("os.stat", side_effect=lambda path: type("Stat", (), {"st_dev": 1 if path == "/fixture/root" else 2})()):
        try:
            check_child_env(env)
        except RuntimeError:
            pass
        else:
            fail("a fixture child on another device was accepted")
    with patch.object(sys.modules[__name__], "same_device_tmp", return_value=True):
        check_child_env(env)
        incomplete = dict(env)
        incomplete.pop("OXIDE_SCRATCH_ROOT")
        try:
            check_child_env(incomplete)
        except RuntimeError:
            pass
        else:
            fail("a fixture child without a scratch declaration was accepted")
    expect(all(env[k] == "/fixture/root" for k in ("TMPDIR", "TMP", "TEMP")), "temporary root drifted")

    with patch("os.path.isdir", return_value=True), patch("os.access", return_value=True), \
         patch("os.stat", side_effect=lambda path: type("Stat", (), {"st_dev": 1 if path == "/tmp-a" else 2})()):
        expect(same_device_tmp("/tmp-a", "/tmp-a"), "same-device control refused")
        expect(not same_device_tmp("/tmp-a", "/pool-b"), "cross-device TMPDIR accepted")
    expect(mode_allows("base-compat", "historical-base"), "base case refused")
    expect(not mode_allows("base-compat", "ordinary-controls"), "base mode accepted other cases")
    for mode in ("permanent-outputs", "auto-adopt", "detach-permanent", "ignore-safety", "exec-inference", "auto-ineligible"):
        expect(not mode_allows(mode, "historical-base"), f"{mode} accepted historical base")

    # Current-main oracle sweep (caf44ac): outputs.rs:64-76 and :2230
    # name whole-target /2, profile /3, plan /2 and profile first adoption;
    # src/lease/control.rs:121 names the /3 fence. /1 and PRE_FEATURE remain
    # historical controls. The candidate source is read only, with no child.
    with open(os.path.join(HERE, "..", "src", "bin", "oxide", "outputs.rs"), encoding="utf-8") as handle:
        output_source = handle.read()
    with open(os.path.join(HERE, "..", "src", "lease", "control.rs"), encoding="utf-8") as handle:
        control_source = handle.read()
    for pin in ('const RECEIPT_VERSION: &str = "oxide.outputs.transition/2"',
                'const PROFILE_RECEIPT_VERSION: &str = "oxide.outputs.transition/3"',
                'const PLAN_VERSION: &str = "oxide.outputs.plan/2"',
                'Only the explicit first conversion selects the profile layout',
                'permanent drive mode: outputs use the checkout-local fallback root',
                'is left untouched'):
        expect(pin in output_source, f"current product oracle changed: {pin}")
    expect('OWNED_OUTPUTS_FENCE: &str = "oxide.outputs/3"' in control_source, "fence version changed")
    expect(PRE_FEATURE == "8c9b5aeab57b8d0ca285b5358e07c89606724c8d", "historical base changed")
    expect(len(os.fsencode(image_slot_model("/Volumes/OxEx12345678"))) <= 80, "image model exceeds 80 bytes")
    expect(len(os.fsencode(image_slot_model("/Volumes/" + "x" * 40))) > 80, "overlong image model accepted")
    with patch("glob.glob", return_value=["/Volumes/x/oxide-pool/slots/f-12345678/quick-1"]), \
         patch("os.path.isdir", return_value=True):
        check_image_slot_paths("/Volumes/x")
    with patch("glob.glob", return_value=["/Volumes/x/oxide-pool/slots/" + "f" * 70 + "/quick-1"]), \
         patch("os.path.isdir", return_value=True):
        try:
            check_image_slot_paths("/Volumes/x")
        except (RuntimeError, SystemExit):
            pass
        else:
            fail("overlong image slot scratch was accepted")

    expect(set(MIGRATION_HOOKS).isdisjoint(LEGACY_HOOKS), "hook families overlap")
    committed = {"state": "committed", "version": "oxide.outputs.transition/2", "checkout": "/c"}
    with patch("os.path.islink", return_value=True), patch("os.path.lexists", return_value=False):
        require_migration_fixture(committed, "/c", "A1")
        for case in MIGRATION_HOOKS:
            require_migration_fixture(committed, "/c", case)
        for case in LEGACY_HOOKS:
            require_legacy_fixture(None, "/c", case)
            try:
                require_legacy_fixture(committed, "/c", case)
            except RuntimeError:
                pass
            else:
                fail(f"{case} accepted a committed migration fixture")
        try:
            require_migration_fixture(None, "/c", "A2")
        except RuntimeError:
            pass
        else:
            fail("A2 accepted a never-converted fixture")

    for mode, groups in FIXTURE_GROUPS.items():
        table = [(name, (), lambda: None) for name in groups]
        validate_fixture_groups(mode, table)
        try:
            validate_fixture_groups(mode, table[:-1])
        except RuntimeError:
            pass
        else:
            fail(f"{mode}: incomplete diagnostic table accepted")
        try:
            validate_fixture_groups(mode, [(groups[0], (), None)] + table[1:])
        except RuntimeError:
            pass
        else:
            fail(f"{mode}: unbound diagnostic body accepted")

    status = b"?? rust/target/debug\0?? rust/target/release\0?? rust/targeted/debug\0"
    expect(status_target_entries(status, "rust/target") ==
           ["?? rust/target/debug", "?? rust/target/release"],
           "nested status member detection or component boundary drifted")
    expect(status_target_entries(b"?? target/debug\0", "rust/target") == ["?? target/debug"],
           "checkout-relative status member was missed")

    ran: list[str] = []

    def quiet(_text: str) -> None:
        return None

    def table(*rows):
        def body(
            case: str,
            *,
            fault: str | None = None,
            abort: str | None = None,
            raises: BaseException | None = None,
        ):
            def run() -> None:
                ran.append(case)
                if abort is not None:
                    raise ProofAbort(abort)
                if raises is not None:
                    raise raises
                if fault is not None:
                    raise RuntimeError(fault)

            return run

        return [
            (case, requires, body(case, **faults)) for case, requires, faults in rows
        ]

    always = lambda: 300.0  # noqa: E731 - a pure seam, never called for its value

    def collect(cases, *, diagnostic=True, after_case=None, work_left=always):
        """One collection on a fresh record, the way the run drives it."""
        record = Collection()
        collect_cases(
            cases,
            diagnostic=diagnostic,
            work_left=work_left,
            after_case=after_case,
            note=quiet,
            record=record,
        )
        return record

    # 1. The counted run is fail-fast: the first fault propagates unchanged and
    #    no later case runs.
    ran.clear()
    try:
        collect(table(("a", (), {}), ("b", (), {"fault": "oracle b"}), ("c", (), {})), diagnostic=False)
        raise AssertionError("the counted collection continued past a fault")
    except RuntimeError as fault:
        expect("oracle b" in str(fault), f"the counted fault changed: {fault!r}")
    expect(ran == ["a", "b"], f"the counted collection ran {ran}")

    # 2. Keep-going records the failure, runs the cases that do not depend on it
    #    and skips the dependents (transitively) with the failed ids.
    ran.clear()
    record = collect(
        table(
            ("a", (), {}),
            ("b", (), {"fault": "oracle b"}),
            ("c", ("b",), {}),
            ("d", (), {}),
            ("e", ("c",), {}),
        )
    )
    expect(ran == ["a", "b", "d"], f"keep-going ran {ran}")
    expect(record.outcomes["a"]["result"] == "completed", f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "failed", f"{record.outcomes['b']}")
    expect("oracle b" in record.outcomes["b"]["fault"], f"{record.outcomes['b']}")
    expect(
        record.outcomes["c"]["result"] == "skipped"
        and record.outcomes["c"]["because"] == ["b"],
        f"{record.outcomes['c']}",
    )
    expect(record.outcomes["d"]["result"] == "completed", f"{record.outcomes['d']}")
    expect(
        record.outcomes["e"]["result"] == "skipped"
        and record.outcomes["e"]["because"] == ["c"],
        f"{record.outcomes['e']}",
    )
    expect([failure["case"] for failure in record.failures] == ["b"], f"{record.failures}")
    expect(record.aborted is None, f"a recoverable failure aborted the collection: {record.aborted}")

    # 3. An unknown outcome aborts: the case is recorded `aborted`, nothing
    #    later runs, even an independent case, and every remaining case is
    #    `unexecuted` — never a pass.
    ran.clear()
    record = collect(
        table(("a", (), {"abort": "unknown writer custody"}), ("b", (), {}), ("c", (), {}))
    )
    expect(ran == ["a"], f"an aborting collection ran {ran}")
    expect(record.outcomes["a"]["result"] == "aborted", f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "unexecuted", f"{record.outcomes['b']}")
    expect(record.outcomes["c"]["result"] == "unexecuted", f"{record.outcomes['c']}")
    expect(record.failures == [], f"an abort was recorded as a recoverable failure: {record.failures}")
    expect(
        record.aborted is not None and "unknown writer custody" in record.aborted,
        f"{record.aborted}",
    )

    # 4. A lost resource or custody after a case (an unclosed child, a lost
    #    mount) is the abort seam, not a case-oracle failure.
    ran.clear()

    def lost(case: str) -> None:
        raise ProofAbort(f"lost custody after {case}")

    record = collect(table(("a", (), {}), ("b", (), {})), after_case=lost)
    expect(ran == ["a"], f"the lost-custody collection ran {ran}")
    expect(record.outcomes["a"]["result"] == "aborted", f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "unexecuted", f"{record.outcomes['b']}")
    expect(record.aborted is not None and "lost custody" in record.aborted, f"{record.aborted}")

    # 4a. The real post-case custody check aborts on each lost resource and
    #     accepts a run where all three are present. It is read-only: "/" is an
    #     existing mount point and an existing path stands in for the
    #     registration, so nothing is created.
    for kwargs, expected in (
        ({"unclosed": {"123"}, "mount": "/", "registration": "/"}, "unclosed children"),
        ({"unclosed": set(), "mount": "/nonexistent-oxide-165", "registration": "/"}, "no longer mounted"),
        ({"unclosed": set(), "mount": "/", "registration": "/nonexistent-oxide-165"}, "registration is gone"),
    ):
        try:
            require_case_custody(case="x", **kwargs)
            raise AssertionError(f"the custody check accepted {kwargs}")
        except ProofAbort as fault:
            expect(expected in str(fault), f"the custody abort changed: {fault}")
    require_case_custody(unclosed=set(), mount="/", registration="/", case="x")

    # 4b. The custody seam runs after a case whose own oracle FAILED, and it is
    #     still an abort: a failed case can leave an unclosed child or a lost
    #     mount behind, and no later case may run against that. Its recoverable
    #     oracle failure stays in `failures`.
    ran.clear()
    custody_seen: list[str] = []

    def custody_after_failure(case: str) -> None:
        custody_seen.append(case)
        if case == "b":
            raise ProofAbort("unclosed child after the failed case")

    record = collect(
        table(("a", (), {}), ("b", (), {"fault": "oracle b"}), ("c", (), {}), ("d", (), {})),
        after_case=custody_after_failure,
    )
    expect(
        custody_seen == ["a", "b"],
        f"the custody seam did not run after the failed case: {custody_seen}",
    )
    expect(ran == ["a", "b"], f"a case ran after the failed case's custody check: {ran}")
    expect(record.outcomes["b"]["result"] == "aborted", f"{record.outcomes['b']}")
    expect("unclosed child" in record.outcomes["b"]["fault"], f"{record.outcomes['b']}")
    expect(
        [failure["case"] for failure in record.failures] == ["b"],
        f"the failed case's oracle failure was lost: {record.failures}",
    )
    expect(
        record.outcomes["c"]["result"] == "unexecuted"
        and record.outcomes["d"]["result"] == "unexecuted",
        f"{record.outcomes['c']} {record.outcomes['d']}",
    )
    expect(record.aborted is not None and "unclosed child" in record.aborted, f"{record.aborted}")

    # 4c. An unexpected exception is an unknown outcome, not a recoverable
    #     oracle failure: it aborts, and every case already reached keeps its
    #     row.
    ran.clear()
    record = collect(
        table(("a", (), {}), ("b", (), {"raises": KeyError("boom")}), ("c", (), {}))
    )
    expect(ran == ["a", "b"], f"an unexpected exception did not abort: {ran}")
    expect(record.outcomes["a"]["result"] == "completed", f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "aborted", f"{record.outcomes['b']}")
    expect("KeyError" in record.outcomes["b"]["fault"], f"{record.outcomes['b']}")
    expect(record.outcomes["c"]["result"] == "unexecuted", f"{record.outcomes['c']}")
    expect(record.failures == [], f"an unexpected exception was recorded as recoverable: {record.failures}")
    expect(record.aborted is not None and "KeyError" in record.aborted, f"{record.aborted}")

    # 4d. An actual monitored-resource failure aborts: the run's own phase
    #     samples are checkpoints, so a sample over the 512 MiB scratch budget or
    #     under the 20 GiB host reserve must stop the collection instead of
    #     spending another case. Both directions, plus the healthy control.
    budget = layout.runner.SCRATCH_LIMIT_BYTES
    reserve = layout.runner.HOST_RESERVE_BYTES
    require_case_resources(peak_bytes=budget, low_bytes=reserve, case="healthy")
    for peak, low, expected in (
        (budget + MIB, reserve, "exceeds the 512 MiB budget"),
        (MIB, reserve - 1, "is below the 20480 MiB reserve"),
        (MIB, 0, "is below the 20480 MiB reserve"),
    ):
        try:
            require_case_resources(peak_bytes=peak, low_bytes=low, case="x")
            raise AssertionError(f"the resource gate accepted peak {peak} low {low}")
        except ProofAbort as fault:
            expect(expected in str(fault), f"the resource abort changed: {fault}")
    ran.clear()
    samples = {"peak": MIB, "low": reserve}

    def overspent(case: str) -> None:
        samples["peak"] = budget + MIB
        require_case_resources(peak_bytes=samples["peak"], low_bytes=samples["low"], case=case)

    record = collect(table(("a", (), {}), ("b", (), {}), ("c", (), {})), after_case=overspent)
    expect(ran == ["a"], f"the collection continued past an over-budget sample: {ran}")
    expect(record.outcomes["a"]["result"] == "aborted", f"{record.outcomes['a']}")
    expect("512 MiB budget" in record.outcomes["a"]["fault"], f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "unexecuted", f"{record.outcomes['b']}")

    # 4e. The phase seam turns the shared runner's resource failure into that
    #     same abort, so the real `Proof.phase` checkpoints cannot be mistaken
    #     for recoverable case-oracle failures.
    class ResourcePhase:
        def __init__(self, fault: str | None) -> None:
            self.fault = fault

        def phase(self, _label: str) -> None:
            if self.fault is not None:
                raise RuntimeError(self.fault)

    before = len(PHASES)
    phase(ResourcePhase(None), "healthy checkpoint")
    expect(len(PHASES) == before + 1, "the phase seam recorded no label")
    try:
        phase(
            ResourcePhase("host free 0 MiB is below the 20 GiB reserve at case 20"),
            "unhealthy checkpoint",
        )
        raise AssertionError("the phase seam passed a resource failure through")
    except ProofAbort as fault:
        expect("monitored resource checkpoint failed" in str(fault), f"{fault}")
    del PHASES[before:]

    # 4f. A cancellation records the case it interrupted as `aborted` (an
    #     executed case is never misreported as unexecuted) and keeps every
    #     outcome and failure already recorded, while still ending the run.
    ran.clear()
    record = Collection()
    try:
        collect_cases(
            table(
                ("a", (), {"fault": "oracle a"}),
                ("b", (), {"raises": KeyboardInterrupt()}),
                ("c", (), {}),
            ),
            diagnostic=True,
            work_left=always,
            note=quiet,
            record=record,
        )
        raise AssertionError("a cancellation did not end the collection")
    except KeyboardInterrupt:
        pass
    expect(record.outcomes["a"]["result"] == "failed", f"{record.outcomes.get('a')}")
    expect(
        [failure["case"] for failure in record.failures] == ["a"],
        f"the accumulated failure history was lost: {record.failures}",
    )
    expect(record.outcomes["b"]["result"] == "aborted", f"{record.outcomes.get('b')}")
    expect("KeyboardInterrupt" in record.outcomes["b"]["fault"], f"{record.outcomes.get('b')}")
    expect("c" not in record.outcomes, f"an unreached case was recorded: {record.outcomes}")
    expect(record.aborted is not None and "KeyboardInterrupt" in record.aborted, f"{record.aborted}")

    # 4g. A diagnostic whose receipt was not written is not a completed
    #     collection: the receipt failure ends the run when it is the only
    #     fault, and is secondary (reported, primary preserved) when a case
    #     fault is already in flight.
    import contextlib
    import io

    def broken_writer(_proof, _collection) -> str:
        raise OSError("evidence directory full")

    wrote: list[str] = []

    def good_writer(_proof, _collection) -> str:
        wrote.append("written")
        return "receipt.json"

    try:
        finish_diagnostic(object(), Collection(), None, writer=broken_writer)
        raise AssertionError("a missing diagnostic receipt was reported as success")
    except OSError as fault:
        expect("evidence directory full" in str(fault), f"{fault}")
    finish_diagnostic(object(), Collection(), None, writer=good_writer)
    expect(wrote == ["written"], f"the receipt writer did not run: {wrote}")
    noise = io.StringIO()
    primary = RuntimeError("a case failed")
    with contextlib.redirect_stderr(noise):
        finish_diagnostic(object(), Collection(), primary, writer=broken_writer)
    expect(
        "receipt was not written" in noise.getvalue(),
        f"the secondary receipt failure was not reported: {noise.getvalue()!r}",
    )
    expect(
        "primary failure is preserved" in noise.getvalue(),
        f"the report did not name the preserved primary failure: {noise.getvalue()!r}",
    )

    # 5. An exhausted guard deadline before a case aborts it and runs nothing;
    #    a `deadline reached` fault inside a case is an abort, never a
    #    recoverable oracle failure.
    ran.clear()
    record = collect(table(("a", (), {})), work_left=lambda: 0.0)
    expect(ran == [], f"a case ran without a work deadline: {ran}")
    expect(record.outcomes["a"]["result"] == "aborted", f"{record.outcomes['a']}")
    expect(record.aborted is not None and "deadline" in record.aborted, f"{record.aborted}")
    ran.clear()
    record = collect(
        table(("a", (), {"fault": "work deadline reached (SIGALRM fallback)"}), ("b", (), {}))
    )
    expect(record.outcomes["a"]["result"] == "aborted", f"{record.outcomes['a']}")
    expect(record.outcomes["b"]["result"] == "unexecuted", f"{record.outcomes['b']}")
    expect(record.failures == [], f"a deadline was recorded as recoverable: {record.failures}")

    # 6. Receipt rows follow the declared table, and a case the collection never
    #    reached is `unexecuted` — a partial collection is never implied to have
    #    passed.
    ids = [case for case, _requires, _text in EXEC_CASES]
    expect(len(ids) == len(set(ids)), f"duplicate exec-inference case id in {ids}")
    for case, requires, _text in EXEC_CASES:
        for dependency in requires:
            expect(dependency in ids, f"{case} depends on the unknown case {dependency}")
            expect(
                ids.index(dependency) < ids.index(case),
                f"{case} depends on the later case {dependency}",
            )
    rows = exec_case_rows({"01": {"result": "completed"}})
    expect([row["case"] for row in rows] == ids, "the receipt rows are not in table order")
    expect(
        [row["result"] for row in rows if row["case"] != "01"] == ["unexecuted"] * (len(ids) - 1),
        f"undone cases are not unexecuted: {rows}",
    )

    # 7. The case bodies share values through exec_inference's own scope with
    #    `nonlocal`. A body that assigns one of those names WITHOUT the
    #    declaration would silently read the placeholder its dependency left
    #    behind, so the mechanism and every assignment are checked here (AST
    #    only: nothing is imported from the run).
    import ast

    shared = {
        "app",
        "app_root",
        "debug_link",
        "release_link",
        "receipt_before",
        "root_before",
        "slot",
        "legacy",
        "legacy_root",
        "legacy_target",
        "legacy_receipt",
        "migrated_root",
        "migrated_receipt",
    }
    cellvars = set(exec_inference.__code__.co_cellvars)
    expect(
        shared <= cellvars,
        f"the shared case values are not captured cells: {sorted(shared - cellvars)}",
    )
    with open(__file__, encoding="utf-8") as handle:
        tree = ast.parse(handle.read())
    inference = next(
        node
        for node in tree.body
        if isinstance(node, ast.FunctionDef) and node.name == "exec_inference"
    )

    def assigned_names(node) -> set:
        names: set = set()
        for inner in ast.walk(node):
            if not isinstance(inner, ast.Assign):
                continue
            targets = list(inner.targets)
            while targets:
                target = targets.pop()
                if isinstance(target, ast.Name):
                    names.add(target.id)
                elif isinstance(target, ast.Tuple):
                    targets.extend(target.elts)
        return names

    # The shared fixture setup must actually be there: the bodies depend on it
    # and a mechanical rewrite that dropped it would otherwise only fail
    # natively. It is the top-level statements before the first case body.
    setup_statements = []
    for statement in inference.body:
        if isinstance(statement, ast.FunctionDef) and statement.name.startswith("case_"):
            break
        setup_statements.append(statement)
    setup_names = assigned_names(ast.Module(body=setup_statements, type_ignores=[]))
    required = {"root", "pool", "home", "cargo_home", "maker", "mount", "name", "source", "tools"}
    expect(
        required <= setup_names,
        f"the shared fixture setup is missing {sorted(required - setup_names)}",
    )
    calls = set()
    for statement in setup_statements:
        for inner in ast.walk(statement):
            if not isinstance(inner, ast.Call):
                continue
            if isinstance(inner.func, ast.Name):
                calls.add(inner.func.id)
            elif isinstance(inner.func, ast.Attribute):
                calls.add(inner.func.attr)
    for call in (
        "write_tools",
        "attach_image",
        "proof_env",
        "fresh_checkout",
        "stage_at",
        "freeze_compiler_flags",
    ):
        expect(call in calls, f"the shared fixture setup no longer calls {call}")

    # The bodies map is the runtime binding the proof itself asserts; checking
    # it here means a dropped or renamed body is found before a native run.
    body_map = {}
    for statement in inference.body:
        if not isinstance(statement, ast.Assign):
            continue
        if not any(isinstance(target, ast.Name) and target.id == "bodies" for target in statement.targets):
            continue
        expect(isinstance(statement.value, ast.Dict), "the bodies map is not a literal")
        body_map = {
            key.value: value.id
            for key, value in zip(statement.value.keys, statement.value.values)
            if isinstance(key, ast.Constant) and isinstance(value, ast.Name)
        }
    expect(
        sorted(body_map) == sorted(case for case, _requires, _text in EXEC_CASES),
        f"the bodies map and the case table disagree: {sorted(set(body_map) ^ set(case for case, _r, _t in EXEC_CASES))}",
    )
    for case, _requires, _text in EXEC_CASES:
        expect(
            body_map[case] == "case_" + case.replace("+", "_"),
            f"{case} is bound to {body_map[case]}",
        )
    for nested in inference.body:
        if not isinstance(nested, ast.FunctionDef) or not nested.name.startswith("case_"):
            continue
        declared = {
            name
            for statement in nested.body
            if isinstance(statement, ast.Nonlocal)
            for name in statement.names
        }
        leaked = (assigned_names(nested) & shared) - declared
        expect(
            not leaked,
            f"{nested.name} assigns the shared {sorted(leaked)} without nonlocal",
        )

    # 8. The collection is wired to the mode, not merely capable of it: the
    #    diagnostic flag, the real post-case seam and the in-place record are
    #    `exec_inference`'s own call arguments, and `exec_run` passes the flag
    #    through.
    collect_calls = [
        inner
        for statement in inference.body
        for inner in ast.walk(statement)
        if isinstance(inner, ast.Call)
        and isinstance(inner.func, ast.Name)
        and inner.func.id == "collect_cases"
    ]
    expect(len(collect_calls) == 1, f"exec_inference calls collect_cases {len(collect_calls)} times")
    keywords = {keyword.arg: keyword.value for keyword in collect_calls[0].keywords}
    expect(
        isinstance(keywords.get("diagnostic"), ast.Name)
        and keywords["diagnostic"].id == "diagnostic",
        "the collection does not receive this run's diagnostic flag",
    )
    expect(
        isinstance(keywords.get("after_case"), ast.IfExp)
        and isinstance(keywords["after_case"].test, ast.Name)
        and keywords["after_case"].test.id == "diagnostic",
        "the custody seam is not scoped to the diagnostic run",
    )
    accumulator = keywords.get("record")
    expect(
        isinstance(accumulator, ast.Name) and accumulator.id == "collection",
        "the collection does not fill the run's receipt record in place",
    )
    wrapper = next(
        node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "exec_run"
    )
    forwarded = [
        inner
        for inner in ast.walk(wrapper)
        if isinstance(inner, ast.Call)
        and isinstance(inner.func, ast.Name)
        and inner.func.id == "exec_inference"
    ]
    expect(len(forwarded) == 1, "exec_run does not call exec_inference exactly once")
    expect(
        len(forwarded[0].args) == 4
        and isinstance(forwarded[0].args[2], ast.Name)
        and forwarded[0].args[2].id == "diagnostic",
        "exec_run does not forward the diagnostic flag",
    )
    # The real seam must run both gates for every case.
    seam = next(
        node
        for node in inference.body
        if isinstance(node, ast.FunctionDef) and node.name == "after_case"
    )
    seam_calls = [
        inner.func.id
        for inner in ast.walk(seam)
        if isinstance(inner, ast.Call) and isinstance(inner.func, ast.Name)
    ]
    for gate in ("require_case_custody", "require_case_resources"):
        expect(gate in seam_calls, f"the post-case seam does not call {gate}")

    # 7b. An abnormal or unclassifiable proof-child exit is a CUSTODY ABORT, not
    #     a recoverable oracle failure. A leader killed by a signal (`-9`) with
    #     EMPTY stderr says nothing about the Cargo group it may have left behind,
    #     so no independent case may run beside it. The counterfactual: treating
    #     `-9` as recoverable would run the later cases, which these checks
    #     reject. An ordinary exit, including Oxide's own 128+signal status, is
    #     classifiable and passes.
    for code in (0, 1, 101, 130, 137):
        require_normal_exit("classifiable", code)
    for code in (-9, -15, -2, None):
        try:
            require_normal_exit("abnormal", code)
            raise AssertionError(f"an abnormal exit status {code!r} was accepted")
        except ProofAbort as fault:
            expect("writer custody is unknown" in str(fault), f"{fault}")

    class ExitProof:
        """The two guarded seams with a chosen exit status and stderr."""

        def __init__(self, code: int, stderr: bytes = b"") -> None:
            self.code = code
            self.stderr = stderr
            self.unclosed: set = set()
            self.peak = 0
            self.low = None
            self.binary = None

        def phase(self, _label: str) -> None:
            return None

        def oxide(self, _args, _label, _cwd):
            return self.code, "", self.stderr.decode(errors="replace")

        def run(self, command, *, cwd=None, env=None, timeout=120):
            class Result:
                returncode = self.code
                stdout = b""
                stderr = self.stderr

            return Result

    # The real guard raises for -9 with empty stderr through BOTH helpers, and
    # for Oxide's own unconfirmed-closure line at a classifiable status.
    for code, stderr, expected in (
        (-9, b"", "writer custody is unknown"),
        (-15, b"", "writer custody is unknown"),
        (1, b"oxide: could not confirm Cargo group 42 closed after the stop; slot 7 stays held, and a later oxide command recovers it\n", "unconfirmed writer closure"),
    ):
        for helper in ("oxide", "run"):
            guarded = ExitProof(code, stderr)
            guard_writer_custody(guarded)
            try:
                if helper == "oxide":
                    guarded.oxide([], "case 21", ".")
                else:
                    guarded.run(["oxide", "build"])
                raise AssertionError(f"{helper} accepted exit {code} stderr {stderr!r}")
            except ProofAbort as fault:
                expect(expected in str(fault), f"{helper}: {fault}")
    # A clean command passes and its result is unchanged.
    guarded = ExitProof(130, b"oxide: quick profile, build strategy, slot 7\n")
    guard_writer_custody(guarded)
    code, _stdout, stderr = guarded.oxide([], "case 15", ".")
    expect(code == 130 and "quick profile" in stderr, f"{code} {stderr!r}")
    expect(guarded.run(["oxide", "status"]).returncode == 130, "the guarded run changed its status")

    # The collector latches such an abort before any independent case, and the
    # FIRST recoverable failure recorded earlier is preserved in the receipt.
    ran.clear()

    def abnormal_leader() -> None:
        ran.append("21")
        require_normal_exit("case 21", -9)

    record = collect(
        [
            ("20", (), lambda: ran.append("20")),
            ("21", (), abnormal_leader),
            ("22", (), lambda: ran.append("22")),
        ]
    )
    expect(ran == ["20", "21"], f"the collection continued past an abnormal leader: {ran}")
    expect(record.outcomes["20"]["result"] == "completed", f"{record.outcomes['20']}")
    expect(record.outcomes["21"]["result"] == "aborted", f"{record.outcomes['21']}")
    expect("writer custody is unknown" in record.outcomes["21"]["fault"], f"{record.outcomes['21']}")
    expect(record.outcomes["22"]["result"] == "unexecuted", f"{record.outcomes['22']}")
    expect(record.failures == [], f"an abnormal exit was recorded as recoverable: {record.failures}")
    expect(record.aborted is not None, "an abnormal exit did not latch the abort")

    ran.clear()
    record = collect(
        [
            ("20", (), lambda: (_ for _ in ()).throw(RuntimeError("oracle 20"))),
            ("21", (), abnormal_leader),
            ("22", (), lambda: ran.append("22")),
        ]
    )
    expect(ran == ["21"], f"a case ran after the abnormal leader: {ran}")
    expect(
        [failure["case"] for failure in record.failures] == ["20"],
        f"the first failure was lost: {record.failures}",
    )
    expect(record.outcomes["20"]["result"] == "failed", f"{record.outcomes['20']}")
    expect(record.outcomes["21"]["result"] == "aborted", f"{record.outcomes['21']}")
    expect(record.outcomes["22"]["result"] == "unexecuted", f"{record.outcomes['22']}")

    # 7c. Case 24's abort-time resource path: the OWNED IMAGE IS NOT REATTACHED
    #     while a custody abort is in flight. The counterfactual is the same
    #     decision taken on a completed case, which does call the reattach.
    reattached: list[str] = []
    disposition = reattach_after_case(
        completed=False,
        reattach=lambda: reattached.append("reattached"),
        reason="case 24 aborted before its oracles held",
    )
    expect(reattached == [], f"an aborted case reattached the image: {reattached}")
    expect("retained detached" in disposition, f"{disposition}")
    disposition = reattach_after_case(
        completed=True, reattach=lambda: reattached.append("reattached"), reason="unused"
    )
    expect(reattached == ["reattached"], f"a completed case did not reattach: {reattached}")
    expect("ordinary re-attach" in disposition, f"{disposition}")

    # The real case body has no `finally` that could reattach during an abort:
    # the call sits on the success path only, after every oracle.
    import ast as _ast

    module_source = open(__file__, encoding="utf-8").read()
    case_24_node = next(
        node
        for node in _ast.walk(_ast.parse(module_source))
        if isinstance(node, _ast.FunctionDef) and node.name == "case_24"
    )
    expect(
        not [
            node
            for node in _ast.walk(case_24_node)
            if isinstance(node, _ast.Try) and node.finalbody
        ],
        "case 24 regained a try/finally that can reattach during an abort",
    )
    case_24_source = _ast.get_source_segment(module_source, case_24_node) or ""
    expect(
        case_24_source.index("reattach_after_case(") > case_24_source.index("future_receipt = "),
        "case 24 reattaches before its oracles held",
    )

    # 7d. The ordinary detach is gated on positively whole custody: a recorded
    #     failure, a latched abort OR any unclosed child refuses it, and only a
    #     clean collection with closed custody may detach.
    clean = Collection()
    expect(detach_decision(clean, set()) is None, "a clean collection refused its detach")
    failed = Collection()
    failed.failures.append({"case": "x", "fault": "x"})
    expect(detach_decision(failed, set()) == "the collection recorded a failure", "a failure detached")
    aborted_record = Collection()
    aborted_record.aborted = "x: custody"
    expect(
        detach_decision(aborted_record, set()).startswith("the collection aborted"),
        "an abort detached",
    )
    expect(
        detach_decision(clean, {"4242"}) == "unclosed children ['4242']",
        "an unclosed child did not refuse the detach",
    )
    # The refusal is recorded as the receipt's detach disposition, never a detach.
    kept = Collection()
    kept.failures.append({"case": "x", "fault": "x"})
    refusal = detach_decision(kept, set())
    expect(refusal is not None, "the gated detach decision changed")
    kept.detached = f"retained: {refusal}"
    document = diagnostic_document(
        source="source",
        rows=exec_case_rows({}),
        failures=kept.failures,
        aborted=None,
        detached=kept.detached,
        mount_present=None,
        reattach="retained detached: case 24 aborted",
        elapsed_s=1.0,
        peak_owned_mib=1,
        host_free_min_mib=2,
        unclosed=[],
    )
    expect(
        document["observations"]["detach"].startswith("retained:"),
        f"{document['observations']}",
    )
    expect(
        document["observations"]["reattach"].startswith("retained detached:"),
        f"{document['observations']}",
    )

    # 7e. Runtime binding of every case body (oxide#164 review). A bare-name call
    #     in a case must resolve to a module global, the `exec_inference` scope or
    #     a name the body itself binds. The counterfactual is the reviewed case 23
    #     calling a helper that does not exist (`external_whole_whole_target`):
    #     that is caught here, with no native activation.
    module_names = set(exec_inference.__globals__)
    scope_names = set(exec_inference.__code__.co_cellvars) | set(
        exec_inference.__code__.co_varnames
    )
    for case, _requires, _text in EXEC_CASES:
        node = next(
            inner
            for inner in inference.body
            if isinstance(inner, _ast.FunctionDef) and inner.name == body_map[case]
        )
        bound = module_names | scope_names | set(dir(__builtins__ if isinstance(__builtins__, dict) else __builtins__)) | {"__builtins__"}
        for inner in _ast.walk(node):
            if isinstance(inner, (_ast.FunctionDef, _ast.AsyncFunctionDef)):
                bound.add(inner.name)
                for argument in (
                    list(inner.args.args)
                    + list(inner.args.kwonlyargs)
                    + [inner.args.vararg, inner.args.kwarg]
                ):
                    if argument is not None:
                        bound.add(argument.arg)
            elif isinstance(inner, _ast.Name) and isinstance(
                inner.ctx, (_ast.Store, _ast.Del)
            ):
                bound.add(inner.id)
            elif isinstance(inner, _ast.ExceptHandler) and inner.name:
                bound.add(inner.name)
            elif isinstance(inner, (_ast.Import, _ast.ImportFrom)):
                for alias in inner.names:
                    bound.add((alias.asname or alias.name).split(".")[0])
            elif isinstance(inner, _ast.comprehension):
                for target in _ast.walk(inner.target):
                    if isinstance(target, _ast.Name):
                        bound.add(target.id)
        unbound = sorted(
            {
                inner.func.id
                for inner in _ast.walk(node)
                if isinstance(inner, _ast.Call)
                and isinstance(inner.func, _ast.Name)
                and inner.func.id not in bound
            }
        )
        expect(
            not unbound,
            f"{body_map[case]} calls helpers that do not exist at runtime: {unbound}",
        )

    # The shared attachment helper respects each caller's directory and image
    # name, including the profile proof's isolated-image control.
    from unittest.mock import patch
    from types import SimpleNamespace
    fake_attach = SimpleNamespace(image=None, mount=None, devices=[], detach_confirmed=False,
                                  detach_attempted=False)
    commands = []
    def fake_run(command, **kwargs):
        commands.append(command)
        if command == ["/sbin/mount"]:
            return SimpleNamespace(returncode=0, stdout=b"/dev/disk-test on /Volumes/fake (apfs, local)\n", stderr=b"")
        payload = plistlib.dumps({"system-entities": [
            {"mount-point": "/Volumes/fake", "dev-entry": "/dev/disk-test"}
        ]})
        return SimpleNamespace(returncode=0, stdout=payload, stderr=b"")
    fake_attach.run = fake_run
    with patch("os.path.exists", return_value=False):
        attach_image(fake_attach, "/evidence/owned-image", "fake",
                     image_name="isolated.sparseimage")
    expected_image = "/evidence/owned-image/isolated.sparseimage"
    expect(fake_attach.image == expected_image, "attachment lost caller's image path")
    expect(commands[0][-1] == expected_image and commands[1][-1] == expected_image,
           "creation and attachment disagree with caller's image path")
    expect(commands[1][-3:] == ["-owners", "on", expected_image], "attachment lost mandatory owners-on")
    expect(commands[2] == ["/sbin/mount"], "attachment skipped mount observation")

    # Every mode and every re-attach calls the same helper; no caller can
    # supply an ownership override, including the base-compat route through run.
    import ast
    with open(__file__, encoding="utf-8") as handle:
        syntax = ast.parse(handle.read())
    functions = {node.name: node for node in syntax.body if isinstance(node, ast.FunctionDef)}
    expected_attach_counts = {"run": 3, "auto_run": 2, "detach_run": 1,
                              "ignore_run": 1, "exec_inference": 2, "auto_ineligible_run": 1}
    for mode, expected_count in expected_attach_counts.items():
        calls = [node for node in ast.walk(functions[mode]) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == "attach_image"]
        expect(len(calls) == expected_count, f"{mode}: attach and re-attach paths changed")
        expect(all(len(call.args) == 3 and not call.keywords for call in calls),
               f"{mode}: an attachment bypasses the owners-on helper")
    expect("base_compat" in {arg.arg for arg in functions["run"].args.args},
           "base-compat no longer uses run's shared attach")
    try:
        attach_image(fake_attach, "/evidence/owned-image", "fake", owners=False)
    except TypeError:
        pass
    else:
        fail("owners-off override was accepted")
    def fake_noowners(command, **kwargs):
        if command == ["/sbin/mount"]:
            return SimpleNamespace(returncode=0, stdout=b"/dev/disk-test on /Volumes/fake (apfs, noowners)\n", stderr=b"")
        return fake_run(command, **kwargs)
    fake_attach.run = fake_noowners
    with patch("os.path.exists", return_value=True):
        try:
            attach_image(fake_attach, "/evidence/owned-image", "fake")
        except RuntimeError:
            expect(fake_attach.mount == "/Volumes/fake" and not fake_attach.detach_confirmed,
                   "ownership refusal lost image custody")
        else:
            fail("the shared attachment accepted a noowners mount")
    noowners = SimpleNamespace(run=lambda *args, **kwargs: SimpleNamespace(
        returncode=0, stdout=b"/dev/disk-test on /Volumes/fake (apfs, local, noowners)\n", stderr=b""))
    try:
        owners_honored(noowners, "/Volumes/fake", "counterfactual")
    except RuntimeError:
        pass
    else:
        fail("a noowners mount was accepted")

    # The shared helper must retain the backing if detach reports success but
    # either the mount or a recorded device survives.
    fake = object.__new__(Proof)
    fake.mount, fake.devices, fake.image = "/mount", ["/dev/disk-test"], "/evidence/image"
    fake.detach_confirmed = False
    for survivor in (fake.mount, fake.devices[0]):
        fake.detach_attempted = False
        with patch.object(layout.Proof, "detach"), patch("os.path.ismount", return_value=True), patch(
            __name__ + ".positively_absent", side_effect=lambda path: path != survivor
        ), patch("os.unlink") as unlink:
            try:
                fake.detach()
            except RuntimeError as fault:
                expect(fake.image in str(fault), "retained image not named")
            else:
                raise RuntimeError("surviving attachment accepted")
            fake.remove_backing()
            expect(not unlink.called, "surviving attachment lost its backing")
    fake.detach_attempted = False
    with patch.object(layout.Proof, "detach"), patch("os.path.ismount", return_value=True), patch(
        __name__ + ".positively_absent", return_value=True
    ), patch("os.unlink") as unlink:
        fake.detach()
        fake.remove_backing()
        unlink.assert_called_once_with(fake.image)

    # 7f. Failure cleanup obeys the same retention gate as the clean tail: after
    #     a custody abort (or with an unclosed child) it must NOT detach the run's
    #     image, and the collection records the retention so the receipt matches
    #     the resource that is actually still attached. A clean collection still
    #     detaches through Proof.detach.
    class CleanupProof:
        def __init__(self) -> None:
            self.root = tempfile.mkdtemp(prefix="oxide-cleanup-check.")
            self.mount = "/"
            self.unclosed: set = set()
            self.detached: list[str] = []

        def detach(self) -> None:
            self.detached.append(self.mount)

    aborted_cleanup = CleanupProof()
    held = Collection()
    held.aborted = "case 21: Oxide reported unconfirmed writer closure"
    message = cleanup_after_failure(aborted_cleanup, held)
    expect(
        aborted_cleanup.detached == [],
        f"failure cleanup detached the image after a custody abort: {message}",
    )
    expect("retained" in message, f"{message}")
    expect(
        held.detached.startswith("retained:") and "aborted" in held.detached,
        f"the receipt's detach disposition does not match the retention: {held.detached}",
    )
    shutil.rmtree(aborted_cleanup.root, ignore_errors=True)

    unclosed_cleanup = CleanupProof()
    unclosed_cleanup.unclosed = {"4242"}
    unclosed_record = Collection()
    message = cleanup_after_failure(unclosed_cleanup, unclosed_record)
    expect(
        unclosed_cleanup.detached == [],
        f"failure cleanup detached the image with an unclosed child: {message}",
    )
    expect("unclosed" in message, f"{message}")
    shutil.rmtree(unclosed_cleanup.root, ignore_errors=True)

    clean_cleanup = CleanupProof()
    clean_record = Collection()
    message = cleanup_after_failure(clean_cleanup, clean_record)
    expect(
        clean_cleanup.detached == ["/"],
        f"a clean failure cleanup did not detach: {message}",
    )
    expect(
        not clean_record.detached.startswith("retained:"),
        f"a clean failure cleanup recorded a retention: {clean_record.detached}",
    )
    shutil.rmtree(clean_cleanup.root, ignore_errors=True)

    # 7g. The COUNTED run's failure cleanup (oxide#164 review). A fault that
    #     escapes the collector latches the collection as an abort whenever
    #     custody is uncertain — in either mode — so `cleanup_after_failure`
    #     must refuse its detach instead of retrying one whose device outcome is
    #     unknown. A recoverable counted oracle failure keeps the shipped cleanup.
    expect(uncertain_outcome(ProofAbort("abnormal child exit")), "a counted abort was classed recoverable")
    expect(uncertain_outcome(KeyboardInterrupt()), "an interrupt was classed recoverable")
    expect(uncertain_outcome(OSError(5, "detach failed")), "an OSError was classed recoverable")
    expect(
        not uncertain_outcome(RuntimeError("an oracle")),
        "a counted oracle failure was treated as unknown custody",
    )

    # A counted run whose ORDINARY DETACH ends by signal (hdiutil killed, empty
    # direct-child census) latches the abort and never detaches again: the
    # counterfactual is the same cleanup on a clean collection, which does.
    class SignalDetachProof:
        def __init__(self) -> None:
            self.root = tempfile.mkdtemp(prefix="oxide-detach-check.")
            self.mount = "/"
            self.unclosed: set = set()
            self.attempts: list[str] = []

        def detach(self) -> None:
            self.attempts.append(self.mount)
            if len(self.attempts) == 1:
                raise ProofAbort("hdiutil ended by signal 9, so the device outcome is unknown")

    signalled = SignalDetachProof()
    counted = Collection()
    try:
        run_ordinary_detach(signalled, counted)
        raise AssertionError("a signalled detach did not propagate")
    except ProofAbort as fault:
        expect("signal 9" in str(fault), f"{fault}")
    expect(
        counted.aborted is not None and "could not be confirmed" in counted.aborted,
        f"the counted detach abort was not latched: {counted.aborted}",
    )
    expect(signalled.unclosed == set(), "the counterfactual did not use an empty child census")
    message = cleanup_after_failure(signalled, counted)
    expect(
        signalled.attempts == ["/"],
        f"the failure cleanup made a SECOND detach after a signalled one: {signalled.attempts}",
    )
    expect("retained" in message, f"{message}")
    expect(
        counted.detached.startswith("retained:"),
        f"the receipt disposition does not match the retention: {counted.detached}",
    )
    shutil.rmtree(signalled.root, ignore_errors=True)

    # The same wrapper propagates a classifiable nonzero detach and latches it,
    # so a second attempt is never made for that case either.
    class FailingDetachProof(SignalDetachProof):
        def detach(self) -> None:
            self.attempts.append(self.mount)
            raise RuntimeError("hdiutil detach exited 1")

    failing = FailingDetachProof()
    counted_failing = Collection()
    try:
        run_ordinary_detach(failing, counted_failing)
        raise AssertionError("a failing detach did not propagate")
    except RuntimeError as fault:
        expect("exited 1" in str(fault), f"{fault}")
    expect(
        counted_failing.aborted is not None,
        "a classifiable failing detach was not latched",
    )
    cleanup_after_failure(failing, counted_failing)
    expect(failing.attempts == ["/"], f"a second detach was attempted: {failing.attempts}")
    shutil.rmtree(failing.root, ignore_errors=True)

    # A clean counted run still detaches once through the wrapper.
    class CleanDetachProof(SignalDetachProof):
        def detach(self) -> None:
            self.attempts.append(self.mount)

    clean_detach = CleanDetachProof()
    clean_collection = Collection()
    run_ordinary_detach(clean_detach, clean_collection)
    expect(
        clean_detach.attempts == ["/"] and clean_collection.aborted is None,
        f"a clean counted detach changed: {clean_detach.attempts} {clean_collection.aborted}",
    )
    shutil.rmtree(clean_detach.root, ignore_errors=True)

    # A counted SETUP abort now latches, so its failure cleanup retains the
    # image; the diagnostic reports the matching disposition.
    held_counted = Collection()
    setup_fault = ProofAbort("staging failed before the first case")
    if uncertain_outcome(setup_fault):
        held_counted.aborted = f"before the first case: {setup_fault!r}"
    expect(
        detach_decision(held_counted, set()).startswith("the collection aborted"),
        "a counted setup abort did not refuse the detach",
    )
    expect(
        detach_disposition(held_counted, set()).startswith("retained: the collection aborted"),
        f"{detach_disposition(held_counted, set())}",
    )
    # A clean collection that failed before its own decision says exactly that.
    expect(
        detach_disposition(Collection(), set())
        == "not reached: the run failed before its own detach decision",
        f"{detach_disposition(Collection(), set())}",
    )
    # The receipt written after such a failure carries the gate's disposition,
    # never the placeholder that implies nothing was attempted.
    pending = Collection()
    pending.aborted = "before the first case: staging failed"
    pending.detached = detach_disposition(pending, set())
    document = diagnostic_document(
        source="source",
        rows=exec_case_rows({}),
        failures=[],
        aborted=pending.aborted,
        detached=pending.detached,
        mount_present=None,
        reattach=None,
        elapsed_s=1.0,
        peak_owned_mib=1,
        host_free_min_mib=2,
        unclosed=[],
    )
    expect(
        document["observations"]["detach"].startswith("retained: the collection aborted"),
        f"{document['observations']}",
    )

    # 7h. The ACTUAL counted-branch tail, driven end to end through the same
    #     function `exec_inference` calls and then the outer failure cleanup: a
    #     first detach that exits nonzero must latch unknown custody, propagate
    #     the ORIGINAL fault, and never be retried. The diagnostic branch of the
    #     same tail records the retention without touching the image.
    class NonzeroDetachProof:
        def __init__(self) -> None:
            self.root = tempfile.mkdtemp(prefix="oxide-counted-tail.")
            self.mount = "/"
            self.unclosed: set = set()
            self.attempts: list[str] = []
            self.first_fault = RuntimeError("hdiutil detach exited 1")

        def detach(self) -> None:
            self.attempts.append(self.mount)
            raise self.first_fault

    counted_tail = NonzeroDetachProof()
    tail_collection = Collection()
    try:
        finish_exec_inference(counted_tail, tail_collection, diagnostic=False, mount="/")
        raise AssertionError("the counted tail swallowed a nonzero detach")
    except RuntimeError as fault:
        expect(
            fault is counted_tail.first_fault,
            f"the counted tail replaced the original fault: {fault!r}",
        )
    expect(
        counted_tail.attempts == ["/"],
        f"the counted tail made more than one attempt: {counted_tail.attempts}",
    )
    expect(
        tail_collection.aborted is not None
        and "could not be confirmed" in tail_collection.aborted,
        f"the counted tail did not latch the detach: {tail_collection.aborted}",
    )
    message = cleanup_after_failure(counted_tail, tail_collection)
    expect(
        counted_tail.attempts == ["/"],
        f"the failure cleanup retried the counted detach: {counted_tail.attempts}",
    )
    expect("retained" in message, f"{message}")
    expect(
        tail_collection.detached.startswith("retained:"),
        f"the retained disposition is missing: {tail_collection.detached}",
    )
    expect(
        "could not be confirmed" in tail_collection.detached,
        f"the disposition lost the original detach fault: {tail_collection.detached}",
    )
    shutil.rmtree(counted_tail.root, ignore_errors=True)

    # A detach that RETURNS while the image is still mounted is unknown device
    # custody too: the counted tail must abort (never report a pass), latch, and
    # leave the image to the retaining cleanup with no second attempt. "/" is a
    # real mount, so the fake detach "succeeds" without unmounting anything.
    class SurvivingMountProof:
        def __init__(self) -> None:
            self.root = tempfile.mkdtemp(prefix="oxide-mounted-tail.")
            self.mount = "/"
            self.unclosed: set = set()
            self.attempts: list[str] = []

        def detach(self) -> None:
            self.attempts.append(self.mount)

    mounted_tail = SurvivingMountProof()
    mounted_collection = Collection()
    try:
        finish_exec_inference(mounted_tail, mounted_collection, diagnostic=False, mount="/")
        raise AssertionError("the counted tail reported success with its image still mounted")
    except ProofAbort as fault:
        expect("still mounted" in str(fault), f"{fault}")
        expect("device outcome is unknown" in str(fault), f"{fault}")
    expect(
        mounted_tail.attempts == ["/"],
        f"the mounted tail made more than one attempt: {mounted_tail.attempts}",
    )
    expect(
        mounted_collection.aborted is not None
        and "could not be confirmed" in mounted_collection.aborted,
        f"the mounted tail did not latch custody: {mounted_collection.aborted}",
    )
    expect(
        mounted_collection.detached.startswith("retained:"),
        f"the mounted tail did not retain the image: {mounted_collection.detached}",
    )
    message = cleanup_after_failure(mounted_tail, mounted_collection)
    expect(
        mounted_tail.attempts == ["/"],
        f"the failure cleanup retried a detach whose mount survived: {mounted_tail.attempts}",
    )
    expect("retained" in message, f"{message}")
    shutil.rmtree(mounted_tail.root, ignore_errors=True)

    # The clean control for the counted tail: a detach that removes the mount
    # records the ordinary disposition and never aborts. A plain temporary
    # directory is not a mount point, so the observation is a real one.
    class UnmountedProof(SurvivingMountProof):
        pass

    unmounted_tail = UnmountedProof()
    unmounted_tail.mount = tempfile.mkdtemp(prefix="oxide-unmounted-tail.")
    unmounted_collection = Collection()
    finish_exec_inference(
        unmounted_tail, unmounted_collection, diagnostic=False, mount=unmounted_tail.mount
    )
    expect(
        unmounted_tail.attempts == [unmounted_tail.mount],
        f"the clean counted tail did not detach once: {unmounted_tail.attempts}",
    )
    expect(
        unmounted_collection.aborted is None
        and unmounted_collection.detached == "ordinary detach through Proof.detach; mount absent",
        f"the clean counted tail disposition changed: {unmounted_collection.detached}",
    )
    shutil.rmtree(unmounted_tail.mount, ignore_errors=True)
    shutil.rmtree(unmounted_tail.root, ignore_errors=True)

    retained_tail = NonzeroDetachProof()
    retained_collection = Collection()
    retained_collection.aborted = "case 21: Oxide reported unconfirmed writer closure"
    try:
        finish_exec_inference(retained_tail, retained_collection, diagnostic=True, mount="/")
        raise AssertionError("the diagnostic tail passed an aborted collection")
    except RuntimeError as fault:
        expect("did not all complete" in str(fault), f"{fault}")
    expect(
        retained_tail.attempts == [],
        f"the diagnostic tail detached while custody was unknown: {retained_tail.attempts}",
    )
    expect(
        retained_collection.detached.startswith("retained: the collection aborted"),
        f"{retained_collection.detached}",
    )
    shutil.rmtree(retained_tail.root, ignore_errors=True)

    # 7i. Case 25's fixture-mode restoration (oxide#164 review): the ephemeral
    #     drive mode may be restored ONLY after the external child and every
    #     oracle completed. A fault retains the fixture's state and propagates as
    #     the FIRST fault; a restoration failure on the clean path is its own
    #     fault and cannot mask an earlier one, because there is none.
    restored: list[str] = []

    def restore_ok() -> None:
        restored.append("restore")

    expect(
        restore_after_case(completed=False, restore=restore_ok)
        == "retained fixture state: the case did not complete",
        "an incomplete case restored the fixture state",
    )
    expect(restored == [], f"an incomplete case restored the fixture state: {restored}")

    restored.clear()
    expect(
        restore_after_case(completed=True, restore=restore_ok)
        == "fixture state restored after the completed case",
        "a completed case did not restore the fixture state",
    )
    expect(restored == ["restore"], f"a completed case did not restore once: {restored}")

    # The shape case 25 uses: the body runs to completion, then the restoration.
    # A body fault propagates unchanged and the restoration is never attempted.
    def run_case_then_restore(body, restore) -> None:
        body()
        restore_after_case(completed=True, restore=restore)

    restored.clear()
    first = ProofAbort("unknown writer custody during the ephemeral case")
    try:
        run_case_then_restore(
            lambda: (_ for _ in ()).throw(first), restore_ok
        )
        raise AssertionError("an aborting case did not propagate its fault")
    except ProofAbort as fault:
        expect(fault is first, f"the first abort was replaced: {fault!r}")
    expect(
        restored == [],
        f"a pool command ran after an abort: {restored}",
    )

    # A restoration failure is reported as its own fault (nothing earlier exists
    # on that path), never hidden.
    class RestoreFault(RuntimeError):
        pass

    restore_fault = RestoreFault("set-mode failed")
    try:
        run_case_then_restore(
            lambda: restored.append("body"),
            lambda: (_ for _ in ()).throw(restore_fault),
        )
        raise AssertionError("a failing restoration was swallowed")
    except RestoreFault as fault:
        expect(fault is restore_fault, f"the restore fault changed: {fault!r}")
    expect(restored == ["body"], f"the body did not complete before the restore: {restored}")

    # Case 25 has no `finally` and restores only after its oracles: AST-checked so
    # a reintroduced recovery restore fails the pure check.
    case_25_node = next(
        node
        for node in _ast.walk(_ast.parse(module_source))
        if isinstance(node, _ast.FunctionDef) and node.name == "case_25"
    )
    expect(
        not [
            node
            for node in _ast.walk(case_25_node)
            if isinstance(node, _ast.Try) and node.finalbody
        ],
        "case 25 regained a try/finally that can restore the pool after an abort",
    )
    case_25_source = _ast.get_source_segment(module_source, case_25_node) or ""
    expect(
        case_25_source.index('set_mode(proof, "permanent"')
        > case_25_source.index('unchanged_layout(ephemeral, "25 ephemeral"'),
        "case 25 restores the drive mode before its ephemeral oracles held",
    )

    # 9. The receipt document itself: `qualified: false`, counted outcomes,
    #    a skipped case never counted as completed, and the limitation stated.
    document = diagnostic_document(
        source="source",
        rows=exec_case_rows(
            {
                "01": {"result": "completed"},
                "02": {"result": "failed", "fault": "x"},
                "03": {"result": "skipped", "because": ["02"]},
            }
        ),
        failures=[{"case": "02", "fault": "x"}],
        aborted=None,
        detached="not attempted",
        mount_present=None,
        reattach=None,
        elapsed_s=1.0,
        peak_owned_mib=1,
        host_free_min_mib=2,
        unclosed=[],
    )
    expect(document["qualified"] is False, "the diagnostic receipt qualified itself")
    expect(
        document["summary"]
        == {
            "completed": 1,
            "failed": 1,
            "skipped": 1,
            "aborted": 0,
            "unexecuted": len(ids) - 3,
        },
        f"{document['summary']}",
    )
    expect("skipped" in document["limitation"], document["limitation"])
    expect("never a counted native PASS" in document["limitation"], document["limitation"])
    expect(
        json.dumps(document) and document["cases"][0]["case"] == ids[0],
        "the receipt document does not serialise",
    )
    # A run that failed before the first case still reports every case
    # unexecuted and keeps the abort reason.
    closed = diagnostic_document(
        source=None,
        rows=exec_case_rows({}),
        failures=[],
        aborted="setup failed",
        detached="not attempted",
        mount_present=None,
        reattach=None,
        elapsed_s=0.0,
        peak_owned_mib=0,
        host_free_min_mib=None,
        unclosed=[],
    )
    expect(closed["summary"]["unexecuted"] == len(ids), f"{closed['summary']}")
    expect(closed["aborted"] == "setup failed", f"{closed['aborted']}")


EXEC_TOOLS = {
    # A tiny tool that calls the real Cargo. `oxide exec` sets CARGO_TARGET_DIR
    # and the build directory, so the tool's own build is what writes the
    # profile members; Oxide reads no argv from it.
    "tool-build.sh": '#!/bin/sh\nset -eu\nexec cargo build "$@"\n',
    # A Cargo external subcommand (oxide#164): Cargo resolves `cargo-oxfixture`
    # for `oxide oxfixture` and passes the subcommand name as the tool's first
    # argument. It records that exact argv, writes a directory the receipt does
    # not place (nextest's own `target/nextest` analogue), then builds through
    # the inherited Cargo, which is what writes the profile members.
    "cargo-oxfixture": (
        '#!/bin/sh\nset -eu\n'
        '[ "${1:-}" = "oxfixture" ] || { echo "unexpected external argv: $*" >&2; exit 64; }\n'
        'if [ -n "${OXIDE_EXTERNAL_MARKER:-}" ]; then\n'
        '  printf "%s" "$*" > "$OXIDE_EXTERNAL_MARKER"\n'
        'fi\n'
        'shift\n'
        'mkdir -p target/oxfixture\n'
        'printf local > target/oxfixture/out\n'
        'exec "${CARGO:-cargo}" build "$@"\n'
    ),
    # A tool that writes a member-shaped directory the receipt does not place.
    "tool-staging.sh": (
        '#!/bin/sh\nset -eu\n'
        'mkdir -p target/staging\n'
        'printf local > target/staging/out\n'
    ),
    # A gated tool whose LEADER exits 130 on the forwarded interrupt (the
    # shell's own "terminated by SIGINT" convention, so the interruption is
    # visible in the tool's status and in Oxide's propagated exit code) while a
    # group member that ignores the signal keeps waiting for the release file,
    # so the lease must stay held until that member closes (case 15). The
    # nonzero status oracle is preserved, not weakened.
    "tool-gated.sh": (
        '#!/bin/sh\nset -eu\n'
        'gate="$1"\n'
        'printf started > "$gate.started"\n'
        '(\n'
        "  trap '' INT TERM\n"
        '  while [ ! -f "$gate.release" ]; do sleep 0.05; done\n'
        ') &\n'
        "trap 'exit 130' INT TERM\n"
        'printf armed > "$gate.armed"\n'
        'while [ ! -f "$gate.release" ]; do sleep 0.05; done\n'
        'wait\n'
    ),
}


def write_tools(root: str) -> str:
    """The fixture's tool directory, outside every checkout so a tool's own
    file never appears in `git status`."""
    directory = os.path.join(root, "exec-tools")
    os.makedirs(directory, exist_ok=True)
    for name, body in EXEC_TOOLS.items():
        path = os.path.join(directory, name)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(body)
        os.chmod(path, 0o755)
    return directory


def member_link_target(checkout: str, member: str) -> str:
    """The destination one profile member link selects."""
    path = os.path.join(checkout, "target", member)
    expect(os.path.islink(path), f"{path} is not an owned member link")
    return os.readlink(path)


def expect_profile_layout(pool: str, mount: str, checkout: str, label: str) -> str:
    """A committed first conversion in the profile layout: a real `target`, an
    owned `debug`/`release` pair into ONE root on the drive, that root's marker
    and its committed profile receipt."""
    target = os.path.join(checkout, "target")
    expect(
        os.path.isdir(target) and not os.path.islink(target),
        f"{label}: target is not a real directory",
    )
    debug = member_link_target(checkout, "debug")
    release = member_link_target(checkout, "release")
    for member, destination in (("debug", debug), ("release", release)):
        expect(
            destination.startswith(os.path.join(mount, "oxide-outputs") + os.sep),
            f"{label}: {member} selects {destination}, not a root on the drive",
        )
    root = os.path.dirname(debug)
    expect(
        release == os.path.join(root, "release"),
        f"{label}: the pair does not share one root: {debug} {release}",
    )
    expect(os.path.isfile(os.path.join(root, MARKER)), f"{label}: the root has no marker")
    receipt = read_json(receipt_file(pool, checkout))
    expect(
        receipt["layout"] == "profile" and receipt["state"] == "committed",
        f"{label}: the receipt is not a committed profile receipt: {receipt}",
    )
    expect(
        sorted(placement["member"] for placement in receipt["links"]) == ["debug", "release"],
        f"{label}: the receipt places {receipt['links']}",
    )
    return root


def legacy_whole_target(proof: Proof, pool: str, mount: str, checkout: str, label: str) -> str:
    """An EXISTING whole-target checkout, built without hand-computing any
    marker fact: the product's own explicit conversion creates the root and its
    `oxide.outputs.root/2` marker for this checkout, and the fixture then
    records the legacy version-2 whole-target receipt that selects that same
    root and replaces the real `target` with the legacy owned link. The root
    keeps the payload and the marker; only the layout the receipt records is
    the pre-oxide#133 one."""
    code, _, stderr = proof.oxide(
        ["outputs", "adopt"], f"{label}: convert to obtain a real root and marker", checkout
    )
    expect(code == 0, stderr)
    return convert_profile_to_legacy(pool, mount, checkout, label)


def convert_profile_to_legacy(pool: str, mount: str, checkout: str, label: str) -> str:
    root = expect_profile_layout(pool, mount, checkout, label)
    receipt = read_json(receipt_file(pool, checkout))
    selected = next(
        (placement["selected"] for placement in receipt["links"] if placement.get("selected")),
        None,
    )
    expect(selected is not None, f"{label}: the conversion selected no root")
    target = os.path.join(checkout, "target")
    for member in ("debug", "release"):
        path = os.path.join(target, member)
        if os.path.islink(path):
            os.remove(path)
    os.rmdir(target)
    os.symlink(root, target)
    # The document copies the product's own `checkout` and `checkout_id` bytes,
    # so no fixture-side path or digest derivation can disagree with it.
    legacy = {
        "version": "oxide.outputs.transition/2",
        "checkout": receipt["checkout"],
        "checkout_id": receipt["checkout_id"],
        "generation": receipt["generation"],
        "state": "committed",
        "links": [{"member": "target", "selected": selected}],
        "updated_ms": int(time.time() * 1000),
    }
    with open(receipt_file(pool, checkout), "w", encoding="utf-8") as handle:
        json.dump(legacy, handle, indent=2)
    return root


def file_proof(path: str) -> tuple:
    """A receipt's exact bytes and the metadata a read must never touch."""
    with open(path, "rb") as handle:
        data = handle.read()
    stat = os.stat(path)
    return (data, stat.st_ino, stat.st_size, stat.st_mode, stat.st_mtime_ns)


def version_one_whole_target(proof: Proof, pool: str, mount: str, checkout: str, label: str) -> tuple:
    """Turn a product-created profile checkout into a live version-1
    whole-target one (oxide#163). No fact is hand-derived: the product's own
    explicit conversion creates the root, its marker and the receipt bytes, and
    the fixture then records the shape oxide#117 published — a root/1 marker
    (no nonce, no checkout volume) and a `oxide.outputs.transition/1` receipt
    (no layout, placements, root-level reservation, prune state or nonce)
    selecting that same root behind the legacy whole-target link. It returns
    the root, the marker's pinned creation timestamp and the generation."""
    code, _, stderr = proof.oxide(["outputs", "adopt"], f"{label}: product conversion", checkout)
    expect(code == 0, stderr)
    root = expect_profile_layout(pool, mount, checkout, label)
    receipt = read_json(receipt_file(pool, checkout))
    selected = next(
        (
            placement["selected"]
            for placement in receipt["links"]
            if placement.get("selected")
        ),
        None,
    )
    expect(selected is not None, f"{label}: the conversion selected no root")
    # Old payload the upgrade must neither move, copy nor delete.
    with open(os.path.join(root, "sentinel"), "wb") as handle:
        handle.write(b"root")
    with open(os.path.join(root, "debug", "old-artifact"), "wb") as handle:
        handle.write(b"old")
    # The legacy whole-target link this receipt selects.
    target = os.path.join(checkout, "target")
    for member in ("debug", "release"):
        path = os.path.join(target, member)
        if os.path.islink(path):
            os.remove(path)
    os.rmdir(target)
    os.symlink(root, target)
    # The root/1 marker: its immutable fields, without the successor surfaces.
    marker_path = os.path.join(root, MARKER)
    marker = read_json(marker_path)
    created_ms = marker["created_ms"]
    marker.pop("nonce", None)
    marker.pop("checkout_volume_uuid", None)
    marker["version"] = "oxide.outputs.root/1"
    with open(marker_path, "w", encoding="utf-8") as handle:
        json.dump(marker, handle)
    # The version-1 receipt, copying the product's own checkout and identity
    # bytes so no fixture-side derivation can disagree with it.
    legacy_selected = {
        "kind": selected["kind"],
        "root": selected["root"],
        "generation": selected["generation"],
    }
    if selected.get("volume_uuid"):
        legacy_selected["volume_uuid"] = selected["volume_uuid"]
    legacy = {
        "version": "oxide.outputs.transition/1",
        "checkout": receipt["checkout"],
        "checkout_id": receipt["checkout_id"],
        "generation": receipt["generation"],
        "state": "committed",
        "selected": legacy_selected,
        "roots": [legacy_selected],
        "updated_ms": int(time.time() * 1000),
    }
    with open(receipt_file(pool, checkout), "w", encoding="utf-8") as handle:
        json.dump(legacy, handle, indent=2)
    return root, created_ms, receipt["generation"]


def detached_version_one(pool: str, checkout: str) -> str:
    """A valid NONCOMMITTED version-1 receipt for a never-converted checkout:
    the control that an explicit adopt must refuse with every byte unchanged."""
    path = receipt_file(pool, checkout)
    receipt = {
        "version": "oxide.outputs.transition/1",
        "checkout": checkout,
        "checkout_id": hashlib.sha256(checkout.encode()).hexdigest(),
        "generation": 1,
        "state": "detached",
        "updated_ms": int(time.time() * 1000),
    }
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2)
    os.chmod(path, 0o600)
    return path


def finish_exec_inference(
    proof: Proof, collection: Collection, *, diagnostic: bool, mount: str
) -> None:
    """The one tail both exec-inference modes share (oxide#164 review).

    The counted run is fail-fast and makes exactly ONE ordinary detach through
    `run_ordinary_detach`, so an unsuccessful detach (a nonzero `hdiutil`, a
    signal, a timeout) latches unknown custody BEFORE any outer cleanup can run.
    The diagnostic run applies the retention gate, detaches only when the gate
    allows it, verifies the mount is gone and records the result, then reports
    every recorded failure or abort. Neither mode forces, retries or re-operates
    an image whose device outcome is unconfirmed.
    """
    if not diagnostic:
        # A fault has already propagated, so a clean return means every case
        # completed.
        expect(
            not collection.failures and collection.aborted is None,
            "the counted collection did not complete",
        )
        run_ordinary_detach(proof, collection)
        if os.path.ismount(mount):
            # The detach command RETURNED but the image is still mounted, so its
            # device outcome is unknown: this is a custody abort, never a counted
            # pass. The abort is latched (so the outer cleanup retains the image
            # and does not detach again) and re-raised as the first fault.
            fault = ProofAbort(
                f"the ordinary detach returned but the owned image is still "
                f"mounted at {mount}, so its device outcome is unknown"
            )
            collection.aborted = f"the ordinary detach could not be confirmed: {fault!r}"[:600]
            collection.detached = detach_disposition(collection, proof.unclosed)
            raise fault
        collection.mount_present = os.path.lexists(mount)
        collection.detached = "ordinary detach through Proof.detach; mount absent"
        return
    # The owned image is this run's custody. Only a collection that recorded no
    # failure, did not abort and left no child unclosed makes its ordinary
    # detach; anything else retains the resource and the receipt names the
    # refusal. A failed detach latches and raises into the existing failure
    # cleanup, which then retains rather than retries.
    refusal = detach_decision(collection, proof.unclosed)
    if refusal is None:
        run_ordinary_detach(proof, collection)
        expect(
            not os.path.ismount(mount),
            f"the owned image is still mounted at {mount} after its ordinary detach",
        )
        expect(
            not proof.unclosed,
            f"unclosed children after detach: {sorted(proof.unclosed)}",
        )
        collection.detached = "ordinary detach through Proof.detach; mount absent"
        collection.mount_present = os.path.lexists(mount)
    else:
        collection.detached = f"retained: {refusal}"
    if collection.failures or collection.aborted:
        fail(
            "exec-inference cases did not all complete: "
            + json.dumps(
                {
                    "failures": collection.failures,
                    "aborted": collection.aborted,
                    "cases": collection.outcomes,
                }
            )
        )


def exec_inference(proof: Proof, repo: str, diagnostic: bool, collection: Collection) -> None:
    """Focused exec-output-inference proof (oxide#160): S1 derives an `exec`
    member set from the checkout's committed placement, and S4b makes the
    automatic first conversion create that profile layout.

    Every case is a positive oracle plus its own control: a real derived run
    and the foreign member link that differs only in destination, an
    unrecorded real member and the root bytes it must not reach, a detached
    checkout that runs locally and the root it must not write, an existing
    whole-target checkout (case 09) that must stay whole-target beside the
    profile conversion, and an interrupted tool (case 15) whose surviving group
    member must keep the lease held. The oxide#162 cases (16-18) reuse that
    migrated whole-target checkout: 16 removes the root's `release` directory
    so the fixture holds only `debug`, migrates, and proves both member
    directories exist — with both committed links resolving — before any
    release build, then runs a derived `exec`; 17 removes the root's `release`
    directory the way the pre-fix migrations left it, leaves `target/release`
    dangling, and proves a derived `exec` creates that one directory, builds
    `--release` into it and changes neither the link nor the receipt; 18 points
    `target/release` at an unverified root and proves the refusal creates
    nothing there. Case 20 (oxide#165) deletes a converted checkout's whole
    `target` directory — what a task-local build-output cleanup does — and
    proves the next leased build recreates it exclusively and republishes the
    member that build places into the SAME root, whose payload and committed
    placement survive, and that a derived `exec` then places the whole owned
    pair there. The oxide#164 cases (21-24) add the external Cargo subcommand:
    21 proves it is Cargo running `cargo-<name>` — the same derivation, key and
    warm slot as `exec` — and that its own non-member `target/oxfixture` stays a
    real local directory reported once by the bounded tool-directories notice;
    22 proves the separate unplaced-member notice for a receipt-RECORDED real
    member (detach, content, re-adopt); 23 proves the unchanged whole-target
    checkout and the foreign-member refusal before the tool runs; 24 proves a
    whole-target checkout whose root is gone reserves the FUTURE root's keyed
    nonce and binds the first local fallback. `--plan` prints the same table and
    allocation with no effect.

    With `diagnostic` the same cases run keep-going (oxide#165's apparatus
    correction): a recoverable case-oracle failure is recorded and the cases
    that do not depend on it still run, a case whose dependency did not complete
    is recorded `skipped` and never runs, and an unknown outcome, a lost
    resource or custody (an unclosed child, a lost mount, a vanished drive
    registration) or an exhausted guard deadline aborts and leaves the rest
    unexecuted. Every declared case is recorded, and a skipped or unexecuted
    case is never a pass. Any command result whose stderr carries Oxide's own
    unconfirmed-closure report (a group that `could not confirm Cargo group …`,
    a slot that `stays held, and a later oxide command recovers it` or `is
    retired and stays held`, or `quarantined: lost with the removable volume`)
    also aborts: the group's custody is unknown, so no later case may run beside
    it. It is never a counted native PASS.

    The HIGH premise checklist is traced through the executable bodies:

    1. private HOME and rustup: `proof_env` captures the invoking `RUSTUP_HOME`
       before replacing `HOME`, pins `RUSTUP_TOOLCHAIN` 1.97.1 and sets
       `RUSTUP_AUTO_INSTALL=0`; every fixture crate is std-only, so the only
       `--locked` call is this repository's staging.
    2. account-home control: `OXIDE_TEST_HOME_FROM_ENV=1` in `proof_env`.
    3. environment isolation: `proof_env` scrubs `OXIDE_*`, `XDG_*`,
       `CARGO_BUILD_*`, `CARGO_TARGET_*`, `GIT_*`, the wrapper variables and
       `CARGO`, then sets only this fixture's values; the pause and crash hooks
       are added per case.
    4. automatic adoption: every case states its receipt premise, and case 08
       positively creates an ineligible (real empty target) checkout and
       asserts the receipt is absent.
    5. Cargo external argv: the oxide#164 cases run a real external subcommand
       (`cargo-oxfixture` from a private `PATH` directory outside every
       checkout), so Cargo's convention is exercised and asserted: the tool is
       invoked as `<tool> oxfixture …`, refuses any other first argument, and
       records that exact argv before it calls the inherited `$CARGO build`.
    6. owned images: every attach goes through `attach_image`, which always
       requests owners-on and observes the mount read-only; the product's own
       `pool register-removable` is the second control, since it refuses an
       ignore-ownership mount.
    7. compiler flags: `freeze_compiler_flags` sets one `RUSTFLAGS` (empty) and
       removes `CARGO_ENCODED_RUSTFLAGS` on `os.environ` BEFORE staging and on
       the fixture environment afterwards, so the staged binary and every
       fixture child build under the same flag set.
    8. temporary storage: `proof_env` places the declared pool and private
       HOME under the supervisor-selected run root; TMPDIR is that root. The
       same-device writable precondition and image-slot path limit are checked
       by the fixture. A /private/tmp fallback is not scratch-pool coverage.
    9. macOS `mktemp`: every fixture temp root is `tempfile.mkdtemp` under
       `OXIDE_TASK_SCRATCH_ROOT` and the gate/release paths are fixed names
       inside it; no fixture or tool calls `mktemp`.
    """
    # oxide#164 review: any command result that carries Oxide's own
    # unconfirmed-closure report stops the collection; the group's custody is
    # unknown, so no later case may run beside it.
    guard_writer_custody(proof)
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    collection.source = source
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxEx" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"exec-output-inference proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount}"
    )
    phase(proof, "start")
    # Premise 7: staging copies os.environ, so the freeze must precede it.
    freeze_compiler_flags(os.environ)
    proof.binary = layout.stage_at(proof, repo, "oxide-exec-inference")
    phase(proof, "after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    proof.fixture_ready = True
    # The same frozen flag set for every fixture child (premise 7).
    freeze_compiler_flags(proof.env)
    expect(proof.env["RUSTFLAGS"] == FROZEN_RUSTFLAGS, "the fixture environment is not frozen")
    expect("CARGO_ENCODED_RUSTFLAGS" not in proof.env, "an encoded rustflags value survives")
    # Premises 8 and 9: the fixture's own temp paths are templates under the
    # supervisor's selected monitored root, and no fixture
    # calls `mktemp` outside it.
    expect(proof.env["TMPDIR"] == os.environ["OXIDE_TASK_SCRATCH_ROOT"], "the fixture TMPDIR is not the supervisor scratch")
    tools = write_tools(root)
    build_tool = os.path.join(tools, "tool-build.sh")
    staging_tool = os.path.join(tools, "tool-staging.sh")
    gated_tool = os.path.join(tools, "tool-gated.sh")

    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "internal pool before the drive", maker)
    expect(code == 0, stderr)
    # Premise 6: this proof's own image is attached with filesystem owners on
    # and observed read-only; the product registration that follows is the
    # second control (it refuses an ignore-ownership mount).
    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)

    # Values one case publishes for the cases the table says depend on it. A
    # dependent case is skipped when its dependency did not complete, so no body
    # ever reads a placeholder.
    app = ""
    app_root = ""
    debug_link = ""
    release_link = ""
    receipt_before = b""
    root_before: dict = {}
    slot = ""
    legacy = ""
    legacy_root = ""
    legacy_target = ""
    legacy_receipt = b""
    migrated_root = ""
    migrated_receipt = b""

    def case_s4b() -> None:
        # S4b: a fresh eligible checkout's first leased build creates the profile
        # layout, and an `exec` with no `--output` places its output in that root.
        first = fresh_checkout(proof, root, "first-conversion")
        code, _, stderr = proof.oxide(["build"], "S4b first leased build converts", first)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        first_root = expect_profile_layout(pool, mount, first, "S4b conversion")
        expect(
            os.path.isfile(os.path.join(first, "target", "debug", "poutputs")),
            "S4b: the build wrote no binary through the owned pair",
        )
        expect(
            os.path.isdir(os.path.join(first_root, "release")),
            "S4b: the conversion left the release member directory out",
        )
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "S4b exec after the conversion", first)
        expect(code == 0, stderr)
        expect("through profile member links" in stderr, stderr)
        expect(
            member_link_target(first, "debug") == os.path.join(first_root, "debug"),
            "S4b: the exec relinked debug",
        )
        expect(
            os.path.isfile(os.path.join(first_root, "debug", "poutputs")),
            "S4b: the exec's output is not in the owned root",
        )
        phase(proof, "after the first conversion")

    def case_01() -> None:
        nonlocal app, app_root, debug_link, release_link, receipt_before
        # 01: an explicit adoption of a fresh checkout publishes the same layout.
        app = fresh_checkout(proof, root, "exec-adopted")
        code, stdout, stderr = proof.oxide(["outputs", "adopt"], "01 explicit adoption", app)
        expect(code == 0 and "now selects the owned external root" in stdout, stdout + stderr)
        app_root = expect_profile_layout(pool, mount, app, "01 adoption")
        debug_link = member_link_target(app, "debug")
        release_link = member_link_target(app, "release")
        receipt_before = read_bytes(receipt_file(pool, app))

    def case_02() -> None:
        nonlocal slot
        # 02: exec with no --output runs a tool that builds through the pair.
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "02 exec without --output", app)
        expect(code == 0, stderr)
        expect(member_link_target(app, "debug") == debug_link, "02: the exec relinked debug")
        expect(member_link_target(app, "release") == release_link, "02: the exec relinked release")
        expect(
            os.path.isfile(os.path.join(app_root, "debug", "poutputs")),
            "02: the tool's output is not in the owned debug member",
        )
        expect("through profile member links (debug, release)" in stderr, stderr)
        slot, _ = slot_state(stderr)

    def case_03() -> None:
        # 03: a repeat reuses the same warm slot and relinks nothing.
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "03 repeat exec", app)
        expect(code == 0, stderr)
        again, state = slot_state(stderr)
        expect(again == slot and state == "warm", f"03: slot {again} {state}\n{stderr}")
        expect(member_link_target(app, "debug") == debug_link, "03: the repeat relinked debug")

    def case_04() -> None:
        # 04: the tool's release build is placed under the same root.
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool, "--release"], "04 exec release build", app
        )
        expect(code == 0, stderr)
        expect(
            os.path.isfile(os.path.join(app_root, "release", "poutputs")),
            "04: the release output is not in the owned release member",
        )

    def case_05() -> None:
        # 05: the declared override reaches the same keyed slot and placement.
        code, _, stderr = proof.oxide(
            ["exec", "--output", "debug", "--output", "release", "--", build_tool],
            "05 declared parity",
            app,
        )
        expect(code == 0, stderr)
        declared_slot, declared_state = slot_state(stderr)
        expect(
            declared_slot == slot and declared_state == "warm",
            f"05: the declared set did not share the derived key: {declared_slot} "
            f"{declared_state}\n{stderr}",
        )
        expect(member_link_target(app, "debug") == debug_link, "05: the declared override relinked debug")
        expect(read_bytes(receipt_file(pool, app)) == receipt_before, "05: an exec changed the receipt")
        phase(proof, "after the derived and declared runs")

    def case_06() -> None:
        nonlocal root_before, receipt_before
        # 06: a member the receipt does not place stays real and local.
        root_before = tree_digest(app_root)
        receipt_before = read_bytes(receipt_file(pool, app))
        code, _, stderr = proof.oxide(["exec", "--", staging_tool], "06 unrecorded member", app)
        expect(code == 0, stderr)
        staging = os.path.join(app, "target", "staging")
        expect(
            os.path.isdir(staging) and not os.path.islink(staging),
            "06: the unrecorded member is not a real directory",
        )
        expect(read_bytes(os.path.join(staging, "out")) == b"local", "06: the tool's write was lost")
        expect(tree_digest(app_root) == root_before, "06: an unrecorded member reached the owned root")
        expect(
            read_bytes(receipt_file(pool, app)) == receipt_before,
            "06: an unrecorded member changed the receipt",
        )
        phase(proof, "after the unrecorded member")

    def case_07_14() -> None:
        nonlocal root_before
        # 07 + 14: detach restores real empty members and keeps the root (the
        # shipped detach, reused); an exec on the detached checkout then runs
        # locally and writes nothing into the root.
        code, _, stderr = proof.oxide(["outputs", "detach"], "07 detach", app)
        expect(code == 0, stderr)
        target = os.path.join(app, "target")
        expect(os.path.isdir(target) and not os.path.islink(target), "07: detach left a link")
        for member in ("debug", "release"):
            path = os.path.join(target, member)
            expect(
                os.path.isdir(path) and not os.path.islink(path),
                f"07: {member} is not a real directory after detach",
            )
            expect(os.listdir(path) == [], f"07: {member} is not empty after detach")
        expect(receipt_state(pool, app) == "detached", "07: the receipt is not detached")
        expect(os.path.isdir(app_root), "07: detach deleted the owned root")
        root_before = tree_digest(app_root)
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "07 exec on a detached checkout", app)
        expect(code == 0, stderr)
        expect(
            os.path.isdir(os.path.join(target, "debug"))
            and not os.path.islink(os.path.join(target, "debug")),
            "07: the detached exec linked a member",
        )
        expect(
            os.path.isfile(os.path.join(target, "debug", "poutputs")),
            "07: the detached exec built nothing locally",
        )
        expect(tree_digest(app_root) == root_before, "07: a detached exec wrote into the root")
        phase(proof, "after the detached run")

    def case_08() -> None:
        # 08: a checkout whose first conversion is ineligible is never converted.
        plain = fresh_checkout(proof, root, "exec-unconverted")
        os.makedirs(os.path.join(plain, "target"))
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "08 unconverted exec", plain)
        expect(code == 0 and not auto_notices(stderr), stderr)
        expect(
            not os.path.islink(os.path.join(plain, "target")),
            "08: an unconverted target became a link",
        )
        expect(not os.path.exists(receipt_file(pool, plain)), "08: an unconverted checkout got a receipt")

    def case_09() -> None:
        nonlocal legacy, legacy_root, legacy_target, legacy_receipt
        # 09: an EXISTING whole-target checkout stays whole-target through a build
        # and through an exec. This is also the S4b "existing whole-target fixture
        # is untouched" half of the brief's native case.
        legacy = fresh_checkout(proof, root, "legacy-whole-target")
        legacy_root = legacy_whole_target(proof, pool, mount, legacy, "09 legacy fixture")
        legacy_target = os.path.join(legacy, "target")
        expect(os.path.islink(legacy_target), "09: the fixture's target is not the legacy link")
        legacy_receipt = read_bytes(receipt_file(pool, legacy))
        legacy_report = plan(proof, legacy, "09 the legacy fixture plans the whole target")
        expect(
            legacy_report["layout"] == "whole-target",
            f"09: the product does not read the fixture as a whole-target checkout: {legacy_report}",
        )
        code, _, stderr = proof.oxide(["build"], "09 a build keeps the whole-target link", legacy)
        expect(code == 0, stderr)
        expect(os.path.islink(legacy_target), "09: a build replaced the whole-target link")
        expect(os.readlink(legacy_target) == legacy_root, "09: a build relinked the whole target")
        expect(
            os.path.isfile(os.path.join(legacy_target, "debug", "poutputs")),
            "09: the build wrote no binary through the whole-target link",
        )
        expect(
            read_bytes(receipt_file(pool, legacy)) == legacy_receipt,
            "09: a build rewrote the whole-target receipt",
        )
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool], "09 an exec keeps the whole-target link", legacy
        )
        expect(code == 0, stderr)
        expect(os.path.islink(legacy_target), "09: an exec replaced the whole-target link")
        expect(
            os.readlink(legacy_target) == legacy_root,
            "09: an exec relinked the whole target",
        )
        expect(
            read_bytes(receipt_file(pool, legacy)) == legacy_receipt,
            "09: an exec rewrote the whole-target receipt",
        )
        phase(proof, "after the legacy whole-target fixture")

    def case_16() -> None:
        nonlocal migrated_root
        # 16 (oxide#162): the fixture's root holds only `debug`. The migration must
        # create BOTH member directories before it publishes their links, so the
        # receipt's `release` placement never dangles, and a derived `exec` runs
        # before any release build.
        os.rmdir(os.path.join(legacy_root, "release"))
        expect(
            os.path.isdir(os.path.join(legacy_root, "debug")),
            "16: the fixture root has no debug directory",
        )
        expect(
            not os.path.exists(os.path.join(legacy_root, "release")),
            "16: the fixture root still has a release directory",
        )
        code, stdout, stderr = proof.oxide(
            ["outputs", "adopt"], "16 migrate a debug-only whole-target root", legacy
        )
        expect(code == 0, stdout + stderr)
        migrated_root = expect_profile_layout(pool, mount, legacy, "16 migration")
        expect(migrated_root == legacy_root, f"16: the migration moved the root to {migrated_root}")
        for member in ("debug", "release"):
            destination = os.path.join(migrated_root, member)
            expect(
                os.path.isdir(destination) and not os.path.islink(destination),
                f"16: {member} is not a real member directory after the migration",
            )
            expect(
                member_link_target(legacy, member) == destination,
                f"16: {member} does not select its migrated member destination",
            )
            expect(
                os.path.isdir(os.path.join(legacy, "target", member)),
                f"16: target/{member} dangles after the migration",
            )
        expect(
            not os.path.exists(os.path.join(migrated_root, "release", "poutputs")),
            "16: a release build happened before the derived exec",
        )
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool], "16 a derived exec follows the migrated pair", legacy
        )
        expect(code == 0, stderr)
        expect("through profile member links (debug, release)" in stderr, stderr)
        expect(
            os.path.isfile(os.path.join(migrated_root, "debug", "poutputs")),
            "16: the derived exec built nothing through the migrated debug member",
        )
        expect(
            not os.path.exists(os.path.join(migrated_root, "release", "poutputs")),
            "16: the derived exec ran a release build",
        )
        phase(proof, "after the migration publication")

    def case_17() -> None:
        nonlocal release_link, migrated_receipt
        # 17 (oxide#162): the exact shape the live pre-fix migrations left behind —
        # the member link published, the root's member directory absent. A derived
        # exec creates that one directory under its verified root and builds
        # through it; the link and the receipt do not change.
        os.rmdir(os.path.join(migrated_root, "release"))
        release_link = os.path.join(legacy, "target", "release")
        expect(
            os.path.islink(release_link),
            "17: target/release is not the owned member link",
        )
        expect(
            not os.path.exists(release_link),
            "17: target/release does not dangle",
        )
        migrated_receipt = read_bytes(receipt_file(pool, legacy))
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool, "--release"],
            "17 an exec repairs the absent release member",
            legacy,
        )
        expect(code == 0, stderr)
        expect(
            os.path.isdir(os.path.join(migrated_root, "release"))
            and not os.path.islink(os.path.join(migrated_root, "release")),
            "17: the exec did not create the release member directory",
        )
        expect(
            os.path.isfile(os.path.join(migrated_root, "release", "poutputs")),
            "17: the release build did not land in the root member",
        )
        expect(
            member_link_target(legacy, "release") == os.path.join(migrated_root, "release"),
            "17: the repair relinked release",
        )
        expect(
            read_bytes(receipt_file(pool, legacy)) == migrated_receipt,
            "17: the repair changed the receipt",
        )
        phase(proof, "after the absent member repair")

    def case_18() -> None:
        nonlocal migrated_receipt
        # 18 (oxide#162): a link into a root that is not this checkout's verified,
        # committed root still refuses before Cargo, and nothing is created there.
        unverified = os.path.join(root, "unverified-output-root")
        os.makedirs(unverified)
        os.remove(release_link)
        os.symlink(os.path.join(unverified, "release"), release_link)
        migrated_receipt = read_bytes(receipt_file(pool, legacy))
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool], "18 an unverified root refuses", legacy
        )
        expect(code != 0, f"18: a link into an unverified root was accepted\n{stderr}")
        expect("target/release" in stderr, stderr)
        expect("oxide outputs adopt" in stderr, stderr)
        expect(
            not os.path.exists(os.path.join(unverified, "release")),
            "18: the unverified root got a member directory",
        )
        expect(
            os.readlink(release_link) == os.path.join(unverified, "release"),
            "18: a refused exec relinked release",
        )
        expect(
            read_bytes(receipt_file(pool, legacy)) == migrated_receipt,
            "18: a refused exec changed the receipt",
        )
        expect(
            os.path.isdir(os.path.join(migrated_root, "release")),
            "18: a refused exec removed the root's member directory",
        )
        phase(proof, "after the unverified root refusal")

    def case_13() -> None:
        # 13: a checkout path containing a space converts and runs end to end.
        spaced = fresh_checkout(proof, root, "exec spaced")
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "13 spaced path exec", spaced)
        expect(code == 0, stderr)
        spaced_root = expect_profile_layout(pool, mount, spaced, "13 spaced path")
        expect(
            os.path.isfile(os.path.join(spaced_root, "debug", "poutputs")),
            "13: the tool's output is not in the spaced checkout's root",
        )

    def case_10() -> None:
        # 10: a foreign member link refuses before Cargo and changes nothing.
        foreign = fresh_checkout(proof, root, "exec-foreign")
        code, _, stderr = proof.oxide(["build"], "10 foreign fixture adopts", foreign)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        expect_profile_layout(pool, mount, foreign, "10 adoption")
        outside = os.path.join(root, "outside-exec")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        foreign_debug = member_link_target(foreign, "debug")
        foreign_receipt = read_bytes(receipt_file(pool, foreign))
        os.remove(os.path.join(foreign, "target", "release"))
        os.symlink(outside, os.path.join(foreign, "target", "release"))
        code, _, stderr = proof.oxide(
            ["exec", "--", build_tool], "10 a foreign member link refuses", foreign
        )
        expect(code != 0, f"10: a foreign member link was followed\n{stderr}")
        expect("target/release" in stderr, stderr)
        expect("oxide outputs adopt" in stderr, stderr)
        expect(
            read_bytes(os.path.join(outside, "sentinel")) == b"foreign",
            "10: the foreign referent mutated",
        )
        expect(member_link_target(foreign, "debug") == foreign_debug, "10: a refused exec relinked debug")
        expect(
            read_bytes(receipt_file(pool, foreign)) == foreign_receipt,
            "10: a refused exec changed the receipt",
        )
        phase(proof, "after the foreign member link")

    def case_12() -> None:
        # 12: root drift (this checkout's own root marker removed) refuses and
        # names the explicit adoption.
        drift = fresh_checkout(proof, root, "exec-drift")
        code, _, stderr = proof.oxide(["build"], "12 drift fixture adopts", drift)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        drift_root = expect_profile_layout(pool, mount, drift, "12 adoption")
        os.remove(os.path.join(drift_root, MARKER))
        drift_debug = member_link_target(drift, "debug")
        code, _, stderr = proof.oxide(["exec", "--", build_tool], "12 root drift refuses", drift)
        expect(code != 0 and "oxide outputs adopt" in stderr, stderr)
        expect(member_link_target(drift, "debug") == drift_debug, "12: a refused exec relinked debug")
        phase(proof, "after root drift")

    def case_11() -> None:
        # 11: a receipt changed while the command waits refuses rather than
        # building into a placement the receipt no longer selects.
        waiting = fresh_checkout(proof, root, "exec-changed")
        code, _, stderr = proof.oxide(["build"], "11 wait fixture adopts", waiting)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        expect_profile_layout(pool, mount, waiting, "11 adoption")
        gate = os.path.join(root, "exec-pause")
        pause_env = dict(proof.env, OXIDE_TEST_PAUSE="keyed:" + gate)
        child = start_child(proof, [proof.binary, "exec", "--", build_tool], waiting, pause_env)
        try:
            wait_for(f"{gate}.keyed-{child.pid}", "11 the exec command's key pause")
            receipt = read_json(receipt_file(pool, waiting))
            receipt["state"] = "detached"
            for placement in receipt["links"]:
                placement["selected"] = None
                placement.pop("prepared", None)
            with open(receipt_file(pool, waiting), "w", encoding="utf-8") as handle:
                json.dump(receipt, handle, indent=2)
            with open(gate, "wb") as handle:
                handle.write(b"release")
            code, _, stderr = finish_child(proof, child, "11 the waiting exec")
            child = None
        finally:
            if child is not None and child.poll() is None:
                child.kill()
            if not os.path.exists(gate):
                with open(gate, "wb") as handle:
                    handle.write(b"release")
        expect(code != 0 and "Run the command again" in stderr, stderr)
        phase(proof, "after the changed receipt")

    def case_15() -> None:
        # 15: Ctrl-C reaches the tool's group; the lease stays held while a group
        # member outlives the leader, and the slot is released once the group is
        # empty and a later build proceeds.
        interrupted = fresh_checkout(proof, root, "exec-interrupt")
        code, _, stderr = proof.oxide(["build"], "15 interrupt fixture adopts", interrupted)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        expect_profile_layout(pool, mount, interrupted, "15 adoption")
        gate = os.path.join(root, "interrupt-gate")
        exec_child = start_child(
            proof,
            [proof.binary, "exec", "--", gated_tool, gate],
            interrupted,
            proof.env,
        )
        try:
            wait_for(f"{gate}.started", "15 the gated tool started")
            # A `dash` in a non-foreground process group starts with SIGINT
            # ignored, so the fixture publishes `armed` only after installing its
            # trap; signalling before that window would be silently dropped.
            wait_for(f"{gate}.armed", "15 the gated tool armed its interrupt trap")
            exec_child.send_signal(signal.SIGINT)
            # The leader exits on the forwarded signal; the surviving group member
            # ignores it, so the exec process must still be running and `status`
            # must still name the holder.
            deadline = time.monotonic() + min(40, work_left())
            held = False
            while time.monotonic() < deadline:
                code, stdout, stderr = proof.oxide(
                    ["status"], "15 status while the group member lives", interrupted
                )
                expect(code == 0, stderr)
                if "exec " in stdout:
                    held = True
                    break
                time.sleep(0.2)
            expect(
                held,
                "15: the interrupted exec's lease was not held while its group member lived",
            )
            expect(exec_child.poll() is None, "15: the exec returned before its group closed")
            with open(f"{gate}.release", "wb") as handle:
                handle.write(b"release")
            code, stdout, stderr = finish_child(proof, exec_child, "15 the interrupted exec")
            exec_child = None
        finally:
            if not os.path.exists(f"{gate}.release"):
                with open(f"{gate}.release", "wb") as handle:
                    handle.write(b"release")
            if exec_child is not None and exec_child.poll() is None:
                exec_child.kill()
        expect(code != 0, f"15: an interrupted exec reported success: {stdout}")
        code, _, stderr = proof.oxide(["build"], "15 the slot is released after the group closes", interrupted)
        expect(code == 0, stderr)
        phase(proof, "after the interrupt control")

    def case_19() -> None:
        nonlocal root_before, debug_link, release_link
        # 19: a live checkout whose receipt is still the committed version-1
        # whole-target shape (oxide#163). Every failing reader refuses it and names
        # adopt, `status` reports the same refusal in its own report grammar, and
        # the one explicit adopt upgrades it through the shipped layout migration
        # with the old outputs intact.
        live = fresh_checkout(proof, root, "legacy-v1")
        (v1_root, v1_created_ms, v1_generation) = version_one_whole_target(
            proof, pool, mount, live, "19 v1 fixture"
        )
        v1_target = os.path.join(live, "target")
        v1_receipt = receipt_file(pool, live)
        v1_before = file_proof(v1_receipt)
        root_before = tree_digest(v1_root)
        for arguments, reader in (
            (["build"], "bind/lease"),
            (["outputs", "detach"], "detach"),
            (["clean"], "clean"),
        ):
            code, _, stderr = proof.oxide(
                arguments, f"19 {reader} refuses a version-1 receipt", live
            )
            expect(code != 0, f"19: {reader} accepted a version-1 receipt\n{stderr}")
            expect(
                "unsupported version" in stderr and "oxide.outputs.transition/1" in stderr,
                f"19: {reader} did not name the version: {stderr}",
            )
            expect(
                "oxide outputs adopt" in stderr,
                f"19: {reader} did not name the fix: {stderr}",
            )
            expect(file_proof(v1_receipt) == v1_before, f"19: {reader} rewrote the receipt")
            expect(tree_digest(v1_root) == root_before, f"19: {reader} changed the root")
            expect(
                os.path.islink(v1_target) and os.readlink(v1_target) == v1_root,
                f"19: {reader} changed the whole-target link",
            )
        # `status` never fails on an unreadable receipt: recorded status reads no
        # checkout state at all (it says so), and measured status renders the same
        # refusal as unknown outputs. Both exit 0 and change nothing.
        code, stdout, stderr = proof.oxide(
            ["status"], "19 recorded status does not read the version-1 receipt", live
        )
        expect(code == 0, f"19: recorded status exited {code}: {stdout}{stderr}")
        expect(
            "outputs: not measured by status" in stdout,
            f"19: recorded status read the checkout receipt: {stdout}{stderr}",
        )
        expect(
            "unsupported version" not in stdout + stderr,
            f"19: recorded status reported the receipt: {stdout}{stderr}",
        )
        code, stdout, stderr = proof.oxide(
            ["status", "--measure"], "19 measured status reports the version-1 refusal", live
        )
        expect(code == 0, f"19: measured status exited {code}: {stdout}{stderr}")
        expect(
            "outputs: unknown" in stdout,
            f"19: measured status did not report unknown outputs: {stdout}{stderr}",
        )
        expect(
            "unsupported version" in stdout and "oxide.outputs.transition/1" in stdout,
            f"19: measured status did not name the version: {stdout}{stderr}",
        )
        expect(
            "oxide outputs adopt" in stdout,
            f"19: measured status did not name the fix: {stdout}{stderr}",
        )
        expect(file_proof(v1_receipt) == v1_before, "19: status rewrote the receipt")
        expect(tree_digest(v1_root) == root_before, "19: status changed the root")
        expect(
            os.path.islink(v1_target) and os.readlink(v1_target) == v1_root,
            "19: status changed the whole-target link",
        )
        phase(proof, "after the version-1 refusals")

        # The noncommitted version-1 control: no adopt, no target, no byte change.
        uncommitted = fresh_checkout(proof, root, "legacy-v1-detached")
        uncommitted_receipt = detached_version_one(pool, uncommitted)
        uncommitted_before = file_proof(uncommitted_receipt)
        code, _, stderr = proof.oxide(
            ["outputs", "adopt"], "19 adopt refuses a noncommitted version-1 receipt", uncommitted
        )
        expect(code != 0, f"19: adopt accepted a noncommitted version-1 receipt\n{stderr}")
        expect(
            "only a committed version-1 receipt can be upgraded" in stderr and "detached" in stderr,
            stderr,
        )
        expect(
            file_proof(uncommitted_receipt) == uncommitted_before,
            "19: the refused adopt rewrote the noncommitted receipt",
        )
        expect(
            not os.path.lexists(os.path.join(uncommitted, "target")),
            "19: the refused adopt created a target",
        )
        phase(proof, "after the noncommitted version-1 control")

        # The upgrade: one explicit adopt, the shipped migration, the current
        # receipt, the same root and the old outputs still there.
        code, stdout, stderr = proof.oxide(
            ["outputs", "adopt"], "19 explicit adopt upgrades the version-1 receipt", live
        )
        expect(code == 0 and "now selects the owned external root" in stdout, stdout + stderr)
        upgraded_root = expect_profile_layout(pool, mount, live, "19 upgrade")
        expect(
            upgraded_root == v1_root,
            f"19: the upgrade selected {upgraded_root}, not the recorded root {v1_root}",
        )
        marker = read_json(os.path.join(v1_root, MARKER))
        expect(marker["version"] == "oxide.outputs.root/2", f"19: the marker is {marker}")
        expect(
            isinstance(marker.get("nonce"), str) and len(marker["nonce"]) == 32,
            f"19: the upgraded marker has no 128-bit nonce: {marker}",
        )
        expect(
            marker["created_ms"] == v1_created_ms,
            "19: the upgrade rewrote the root's creation timestamp",
        )
        receipt = read_json(v1_receipt)
        expect(
            receipt["version"] == "oxide.outputs.transition/3"
            and receipt["layout"] == "profile"
            and receipt["state"] == "committed",
            f"19: the upgraded receipt is not the current profile shape: {receipt}",
        )
        expect(
            receipt["generation"] == v1_generation + 1,
            f"19: the upgrade advanced {v1_generation} to {receipt['generation']}",
        )
        expect(
            sorted(placement["member"] for placement in receipt["links"]) == ["debug", "release"],
            f"19: the upgraded receipt places {receipt['links']}",
        )
        expect(
            read_bytes(os.path.join(v1_root, "sentinel")) == b"root",
            "19: the upgrade lost the root's old payload",
        )
        expect(
            read_bytes(os.path.join(v1_target, "debug", "old-artifact")) == b"old",
            "19: the old output is not visible through the migrated member link",
        )
        debug_link = member_link_target(live, "debug")
        release_link = member_link_target(live, "release")
        receipt_after = read_bytes(v1_receipt)

        # A leased build and an exec now work in the upgraded checkout, without
        # relinking anything and without rewriting the receipt.
        code, _, stderr = proof.oxide(["build"], "19 the upgraded checkout builds", live)
        expect(code == 0, stderr)
        expect(
            member_link_target(live, "debug") == debug_link
            and member_link_target(live, "release") == release_link,
            "19: the build relinked an upgraded member",
        )
        expect(
            os.path.isfile(os.path.join(v1_target, "debug", "poutputs")),
            "19: the build wrote no binary through the upgraded member link",
        )
        expect(
            read_bytes(os.path.join(v1_target, "debug", "old-artifact")) == b"old",
            "19: the build lost the old output",
        )
        code, _, stderr = proof.oxide(["exec", "--", "/usr/bin/true"], "19 the upgraded checkout execs", live)
        expect(code == 0, stderr)
        expect("through profile member links" in stderr, stderr)
        expect(read_bytes(v1_receipt) == receipt_after, "19: a command rewrote the upgraded receipt")
        plan_report = plan(proof, live, "19 the upgraded checkout plans the profile layout")
        expect(plan_report["layout"] == "profile", f"19: the plan reports {plan_report['layout']}")
        phase(proof, "after the version-1 upgrade")

    def case_20() -> None:
        # 20 (oxide#165): a converted checkout whose `target` directory was deleted
        # — what a task-local build-output cleanup, or a manual `rm -rf target`,
        # does — must not be a checkout that can never build again. Every leased
        # command used to fail before compiling with ENOENT while creating the
        # transition link inside the absent directory. The binding now recreates
        # that one real directory exclusively and republishes the owned pair
        # through the existing transition, into the SAME root: the receipt's
        # committed root and nonce and the root's payload survive.
        deleted = fresh_checkout(proof, root, "deleted-target")
        code, _, stderr = proof.oxide(["build"], "20 deleted-target fixture adopts", deleted)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        deleted_root = expect_profile_layout(pool, mount, deleted, "20 adoption")
        with open(os.path.join(deleted_root, "sentinel"), "wb") as handle:
            handle.write(b"kept")
        with open(os.path.join(deleted_root, "debug", "old-output"), "wb") as handle:
            handle.write(b"old")
        deleted_receipt = read_json(receipt_file(pool, deleted))
        deleted_selected = next(
            (
                placement["selected"]
                for placement in deleted_receipt["links"]
                if placement.get("selected")
            ),
            None,
        )
        expect(deleted_selected is not None, "20: the fixture's receipt selects no root")
        deleted_target = os.path.join(deleted, "target")
        expect(
            os.path.isdir(deleted_target) and not os.path.islink(deleted_target),
            "20: the fixture's target is not a real directory",
        )
        shutil.rmtree(deleted_target)
        expect(not os.path.lexists(deleted_target), "20: the fixture's target was not deleted")
        code, _, stderr = proof.oxide(
            ["build"], "20 a build recreates the deleted target directory", deleted
        )
        expect(code == 0, f"20: a leased build failed on the deleted target directory: {stderr}")
        expect(
            "through profile member links" in stderr,
            f"20: the build did not bind the owned profile members: {stderr}",
        )
        # The deleted directory is back as a real directory, and this command's own
        # `debug` member is republished into the SAME root the receipt records.
        # `release` is not this build's member: the derived `exec` below places it.
        expect(
            os.path.isdir(deleted_target) and not os.path.islink(deleted_target),
            "20: the build did not recreate a real target directory",
        )
        expect(
            member_link_target(deleted, "debug") == os.path.join(deleted_root, "debug"),
            f"20: the recreated debug link does not select the recorded root: "
            f"{os.readlink(os.path.join(deleted_target, 'debug'))}",
        )
        expect(
            os.path.isfile(os.path.join(deleted_root, "debug", "poutputs")),
            "20: the build wrote no binary through the recreated member link",
        )
        expect(
            read_bytes(os.path.join(deleted_root, "sentinel")) == b"kept",
            "20: the recreation lost the root's old file",
        )
        expect(
            read_bytes(os.path.join(deleted_root, "debug", "old-output")) == b"old",
            "20: the recreation lost the root's old output",
        )
        recreated_receipt = read_json(receipt_file(pool, deleted))
        expect(
            recreated_receipt["layout"] == "profile" and recreated_receipt["state"] == "committed",
            f"20: the receipt is not a committed profile receipt: {recreated_receipt}",
        )
        for placement in recreated_receipt["links"]:
            expect(
                placement.get("selected") is not None
                and placement["selected"]["root"] == deleted_selected["root"]
                and placement["selected"].get("nonce") == deleted_selected.get("nonce"),
                f"20: {placement['member']} moved off the recorded root or nonce: {placement}",
            )
        code, _, stderr = proof.oxide(
            ["exec", "--", "/usr/bin/true"], "20 an exec after the recreation", deleted
        )
        expect(code == 0, f"20: an exec failed after the recreation: {stderr}")
        expect("through profile member links" in stderr, stderr)
        # The derived `exec` places every member its receipt records through the
        # recreated directory, so the whole owned pair is live again in one root.
        exec_root = expect_profile_layout(pool, mount, deleted, "20 recreation")
        expect(
            exec_root == deleted_root,
            f"20: the exec bound {exec_root}, not the recorded root {deleted_root}",
        )
        expect(
            read_bytes(os.path.join(deleted_root, "debug", "old-output")) == b"old",
            "20: the exec lost the root's old output",
        )
        phase(proof, "after the deleted target recreation")

    def case_21() -> None:
        # 21 (oxide#164): an external Cargo subcommand is Cargo running
        # `cargo-<name>`, which is exactly what `oxide exec -- cargo <name>`
        # runs, so a profile checkout derives its member set from the receipt
        # instead of refusing. The shared key is proved positively: the exec
        # derivation warms the slot first, then the external subcommand must
        # reuse that exact slot warm.
        external = fresh_checkout(proof, root, "exec-external")
        code, _, stderr = proof.oxide(["build"], "21 external fixture converts", external)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        external_root = expect_profile_layout(pool, mount, external, "21 conversion")
        external_debug = member_link_target(external, "debug")
        external_release = member_link_target(external, "release")
        external_receipt = read_bytes(receipt_file(pool, external))
        code, _, stderr = proof.oxide(
            ["exec", "--", "/usr/bin/true"], "21 warm the exec derivation's key", external
        )
        expect(code == 0, stderr)
        exec_slot, _ = slot_state(stderr)
        # Remove the first build's binary, so the external tool's own nested
        # `$CARGO build` is what has to recreate it through the member link.
        os.remove(os.path.join(external_root, "debug", "poutputs"))
        tool_marker = os.path.join(root, "external-oxfixture-argv")
        external_env = {
            **proof.env,
            "PATH": tools + os.pathsep + proof.env.get("PATH", ""),
            "OXIDE_EXTERNAL_MARKER": tool_marker,
        }
        code, _, stderr = oxide_env(
            proof,
            ["oxfixture"],
            "21 external subcommand in a profile checkout",
            external,
            external_env,
        )
        expect(code == 0, stderr)
        expect(
            os.path.isfile(tool_marker) and read_bytes(tool_marker) == b"oxfixture",
            "21: the external tool did not run with Cargo's own external argv: "
            f"{read_bytes(tool_marker) if os.path.isfile(tool_marker) else 'no recorded argv'}",
        )
        expect(
            member_link_target(external, "debug") == external_debug
            and member_link_target(external, "release") == external_release,
            "21: the external subcommand relinked a member",
        )
        expect(
            os.path.isfile(os.path.join(external_root, "debug", "poutputs")),
            "21: the external tool's build is not in the owned debug member",
        )
        local_tool_dir = os.path.join(external, "target", "oxfixture")
        expect(
            os.path.isdir(local_tool_dir) and not os.path.islink(local_tool_dir),
            "21: the tool's non-member directory is not a real local directory",
        )
        expect(
            read_bytes(os.path.join(local_tool_dir, "out")) == b"local",
            "21: the tool's local write was lost",
        )
        expect(
            not os.path.exists(os.path.join(external_root, "oxfixture")),
            "21: a non-member tool directory reached the owned root",
        )
        expect(
            read_bytes(receipt_file(pool, external)) == external_receipt,
            "21: the external subcommand changed the receipt",
        )
        external_slot, external_state = slot_state(stderr)
        expect(
            external_slot == exec_slot and external_state == "warm",
            f"21: the external subcommand did not share exec's key: {external_slot} "
            f"{external_state} (exec {exec_slot})\n{stderr}",
        )
        # The brief's tool-directory notice: the tool created `target/oxfixture`,
        # a directory that is not a profile member, so the product reports it
        # once, naming only that directory, and never places it.
        tool_notices = [
            line for line in stderr.splitlines() if "tool directories kept local" in line
        ]
        expect(
            len(tool_notices) == 1,
            f"21: the tool-directory notice is not printed exactly once: {tool_notices}",
        )
        expect(
            "oxfixture" in tool_notices[0]
            and "debug" not in tool_notices[0]
            and "release" not in tool_notices[0],
            f"21: the tool-directory notice names the wrong directory: {tool_notices[0]}",
        )
        expect(
            not [line for line in stderr.splitlines() if "and are used locally" in line],
            f"21: a fully placed checkout reported an unplaced member: {stderr}",
        )
        phase(proof, "after the external subcommand")

    def case_22() -> None:
        # 22 (oxide#164): the one bounded notice for an unplaced member is the
        # existing bind report for a receipt-RECORDED real member, reached the
        # way the product reaches it: detach restores real empty members and
        # keeps their unselected placements, content is written, and the
        # explicit re-adopt keeps that nonempty member real. The tool's own
        # directory is the SEPARATE bounded tool-directory notice and is never
        # named by the unplaced-member line.
        unplaced = fresh_checkout(proof, root, "exec-external-real-member")
        code, _, stderr = proof.oxide(["build"], "22 convert the notice fixture", unplaced)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        expect_profile_layout(pool, mount, unplaced, "22 conversion")
        code, _, stderr = proof.oxide(
            ["outputs", "detach"], "22 detach the notice fixture", unplaced
        )
        expect(code == 0, stderr)
        expect(
            receipt_state(pool, unplaced) == "detached",
            "22: the notice fixture did not detach",
        )
        local_member = os.path.join(unplaced, "target", "release")
        expect(
            os.path.isdir(local_member) and not os.path.islink(local_member),
            "22: detach did not restore a real release member",
        )
        with open(os.path.join(local_member, "local-artifact"), "wb") as handle:
            handle.write(b"local")
        code, stdout, stderr = proof.oxide(
            ["outputs", "adopt"], "22 re-adopt with a real nonempty member", unplaced
        )
        expect(code == 0, stdout + stderr)
        unplaced_root = os.path.dirname(member_link_target(unplaced, "debug"))
        expect(
            unplaced_root.startswith(os.path.join(mount, "oxide-outputs") + os.sep),
            f"22: debug selects {unplaced_root}, not a root on the drive",
        )
        expect(
            os.path.isdir(local_member) and not os.path.islink(local_member),
            "22: the real member was converted",
        )
        unplaced_receipt = read_json(receipt_file(pool, unplaced))
        expect(
            unplaced_receipt["layout"] == "profile"
            and unplaced_receipt["state"] == "committed",
            f"22: the receipt is not a committed profile receipt: {unplaced_receipt}",
        )
        placements = {
            entry["member"]: entry.get("selected") for entry in unplaced_receipt["links"]
        }
        expect(
            "release" in placements
            and placements.get("release") is None
            and isinstance(placements.get("debug"), dict),
            f"22: the receipt places {unplaced_receipt['links']}",
        )
        notice_receipt_before = read_bytes(receipt_file(pool, unplaced))
        tool_marker = os.path.join(root, "external-oxfixture-real-member-argv")
        external_env = {
            **proof.env,
            "PATH": tools + os.pathsep + proof.env.get("PATH", ""),
            "OXIDE_EXTERNAL_MARKER": tool_marker,
        }
        code, _, stderr = oxide_env(
            proof,
            ["oxfixture"],
            "22 external subcommand with a receipt-recorded real member",
            unplaced,
            external_env,
        )
        expect(code == 0, stderr)
        expect(os.path.isfile(tool_marker), "22: the external tool did not run")
        unplaced_notices = [
            line for line in stderr.splitlines() if "and are used locally" in line
        ]
        expect(
            len(unplaced_notices) == 1,
            f"22: the unplaced-member notice is not printed exactly once: {unplaced_notices}",
        )
        expect(
            "release" in unplaced_notices[0],
            f"22: the notice does not name the receipt's real member: {unplaced_notices[0]}",
        )
        expect(
            "oxfixture" not in unplaced_notices[0],
            f"22: the notice names a directory the receipt never placed: {unplaced_notices[0]}",
        )
        tool_notices = [
            line for line in stderr.splitlines() if "tool directories kept local" in line
        ]
        expect(
            len(tool_notices) == 1,
            f"22: the tool-directory notice is not printed exactly once: {tool_notices}",
        )
        expect(
            "oxfixture" in tool_notices[0] and "release" not in tool_notices[0],
            f"22: the tool-directory notice names the wrong directory: {tool_notices[0]}",
        )
        expect(
            os.path.isfile(os.path.join(unplaced_root, "debug", "poutputs")),
            "22: the external tool's build is not in the owned debug member",
        )
        tool_dir = os.path.join(unplaced, "target", "oxfixture")
        expect(
            os.path.isdir(tool_dir)
            and not os.path.islink(tool_dir)
            and os.path.isfile(os.path.join(tool_dir, "out")),
            "22: the tool's own directory is not a real local directory",
        )
        expect(
            not os.path.exists(os.path.join(unplaced_root, "oxfixture")),
            "22: a tool directory the receipt never placed reached the owned root",
        )
        expect(
            read_bytes(os.path.join(local_member, "local-artifact")) == b"local",
            "22: the real member's content changed",
        )
        expect(
            os.path.isdir(local_member) and not os.path.islink(local_member),
            "22: the real member became a link",
        )
        expect(
            read_bytes(receipt_file(pool, unplaced)) == notice_receipt_before,
            "22: the external subcommand changed the receipt",
        )
        phase(proof, "after the unplaced-member notice")

    def case_23() -> None:
        # 23 (oxide#164): the external subcommand is unchanged in a whole-target
        # checkout, and the derivation's foreign-member refusal is preserved
        # before the tool runs.
        external_whole = fresh_checkout(proof, root, "external-whole-target")
        external_whole_root = legacy_whole_target(
            proof, pool, mount, external_whole, "23 external whole-target fixture"
        )
        external_whole_target = os.path.join(external_whole, "target")
        external_whole_receipt = read_bytes(receipt_file(pool, external_whole))
        expect(
            not os.path.exists(os.path.join(external_whole_target, "debug", "poutputs")),
            "23: the whole-target fixture already built its binary",
        )
        tool_marker = os.path.join(root, "external-oxfixture-whole-target-argv")
        external_env = {
            **proof.env,
            "PATH": tools + os.pathsep + proof.env.get("PATH", ""),
            "OXIDE_EXTERNAL_MARKER": tool_marker,
        }
        code, _, stderr = oxide_env(
            proof,
            ["oxfixture"],
            "23 external subcommand on a whole-target checkout",
            external_whole,
            external_env,
        )
        expect(code == 0, stderr)
        expect(
            os.path.islink(external_whole_target) and os.readlink(external_whole_target) == external_whole_root,
            "23: the external subcommand replaced or relinked the whole target",
        )
        expect(
            os.path.isfile(os.path.join(external_whole_target, "debug", "poutputs")),
            "23: the external tool built nothing through the whole-target link",
        )
        expect(
            read_bytes(receipt_file(pool, external_whole)) == external_whole_receipt,
            "23: the external subcommand rewrote the whole-target receipt",
        )
        expect(
            os.path.isfile(tool_marker) and read_bytes(tool_marker) == b"oxfixture",
            "23: the external tool did not run in the whole-target checkout",
        )
        expect(
            not [
                line
                for line in stderr.splitlines()
                if "tool directories kept local" in line
            ],
            f"23: a whole-target checkout printed the tool-directory notice: {stderr}",
        )
        # The refusal half: a profile checkout with a foreign release link
        # refuses before the tool runs.
        foreign = fresh_checkout(proof, root, "external-foreign")
        code, _, stderr = proof.oxide(["build"], "23 foreign fixture converts", foreign)
        expect(code == 0 and len(auto_notices(stderr)) == 1, stderr)
        expect_profile_layout(pool, mount, foreign, "23 adoption")
        outside = os.path.join(root, "outside-external")
        os.makedirs(outside)
        with open(os.path.join(outside, "sentinel"), "wb") as handle:
            handle.write(b"foreign")
        foreign_debug = member_link_target(foreign, "debug")
        foreign_receipt = read_bytes(receipt_file(pool, foreign))
        os.remove(os.path.join(foreign, "target", "release"))
        os.symlink(outside, os.path.join(foreign, "target", "release"))
        if os.path.exists(tool_marker):
            os.remove(tool_marker)
        code, _, stderr = oxide_env(
            proof,
            ["oxfixture"],
            "23 external subcommand against a foreign member link",
            foreign,
            external_env,
        )
        expect(
            code != 0,
            f"23: an external subcommand was accepted through a foreign member link\n{stderr}",
        )
        expect("target/release" in stderr, stderr)
        expect("oxide outputs adopt" in stderr, stderr)
        expect(
            not os.path.exists(tool_marker),
            "23: the external tool ran through a refused layout",
        )
        expect(
            read_bytes(os.path.join(outside, "sentinel")) == b"foreign",
            "23: the foreign referent mutated",
        )
        expect(
            member_link_target(foreign, "debug") == foreign_debug,
            "23: a refused external subcommand relinked debug",
        )
        expect(
            read_bytes(receipt_file(pool, foreign)) == foreign_receipt,
            "23: a refused external subcommand changed the receipt",
        )
        phase(proof, "after the external whole-target and refusal controls")

    def case_24() -> None:
        # 24 (oxide#164): a whole-target checkout whose own root is gone plans a
        # FUTURE root, and binding accepts that plan only through the exact
        # reservation its slot was keyed with (#117). A derived command must
        # therefore still reserve: skipping it because it derives refuses the
        # first whole-target fallback with no nonce.
        #
        # The checkout-local fallback root is only created when Git ignores it
        # (oxide#125), so this fixture states both rules its two layouts need.
        future = fresh_checkout(
            proof, root, "external-future-root", ignore="/target\n/.oxide-outputs/\n"
        )
        future_root = legacy_whole_target(
            proof, pool, mount, future, "24 future-root fixture"
        )
        future_target = os.path.join(future, "target")
        expect(
            os.path.islink(future_target) and os.readlink(future_target) == future_root,
            "24: the fixture's target is not the whole-target link",
        )
        tool_marker = os.path.join(root, "external-oxfixture-future-root-argv")
        external_env = {
            **proof.env,
            "PATH": tools + os.pathsep + proof.env.get("PATH", ""),
            "OXIDE_EXTERNAL_MARKER": tool_marker,
        }
        volume_uuid = real_uuid(pool)
        force_detach(proof, proof.image, mount, volume_uuid)

        def reattach() -> None:
            mounted_now = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
            expect(
                mounted_now == mount,
                f"24: the scratch volume re-attached at {mounted_now}, not {mount}",
            )

        # No `finally`: the re-attach happens only after this case COMPLETED.
        # A custody abort inside it retains the detached volume instead of
        # mounting a resource whose writers or device binding are unconfirmed.
        if os.path.exists(tool_marker):
            os.remove(tool_marker)
        code, _, stderr = oxide_env(
            proof,
            ["oxfixture"],
            "24 external subcommand onto a future whole-target root",
            future,
            external_env,
        )
        expect(
            code == 0,
            "24: the first whole-target fallback refused (a derived command "
            f"skipped its reservation): {stderr}",
        )
        local_root = os.path.join(future, ".oxide-outputs", "local")
        expect(
            os.path.isdir(local_root) and not os.path.islink(local_root),
            "24: the fallback created no local root",
        )
        expect(
            os.path.islink(future_target)
            and os.path.realpath(future_target) == os.path.realpath(local_root),
            "24: the whole-target link does not select the local fallback",
        )
        expect(
            os.path.isfile(os.path.join(future_target, "debug", "poutputs")),
            "24: the external tool built nothing through the fallback root",
        )
        expect(os.path.isfile(tool_marker), "24: the external tool did not run")
        future_receipt = read_json(receipt_file(pool, future))
        expect(
            future_receipt["state"] == "committed",
            f"24: the receipt is {future_receipt['state']}",
        )
        expect(
            future_receipt.get("reserved") is None,
            "24: the future root's reservation was not consumed by the transition",
        )
        selected = (future_receipt.get("links") or [{}])[0].get("selected") or {}
        expect(
            str(selected.get("root", "")).startswith(future),
            f"24: the receipt does not select the local fallback root: {future_receipt['links']}",
        )
        # Every oracle above held, so this case COMPLETED and only now may the
        # owned image come back; `reattach_after_case` records the disposition.
        collection.reattach = reattach_after_case(
            completed=True, reattach=reattach, reason="case 24 did not complete"
        )
        phase(proof, "after the future whole-target root")

    def case_25() -> None:
        # 25 (oxide#164 review): the tool-directory notice belongs to the
        # receipt-derived PROFILE layout only. A never-converted real target and
        # an ephemeral real target keep their prior stderr: the external
        # subcommand runs, writes its own directory locally, and prints no
        # tool-directories notice.
        tool_marker = os.path.join(root, "external-oxfixture-layouts-argv")
        external_env = {
            **proof.env,
            "PATH": tools + os.pathsep + proof.env.get("PATH", ""),
            "OXIDE_EXTERNAL_MARKER": tool_marker,
        }

        def unchanged_layout(checkout: str, label: str, *, real_target_first: bool) -> None:
            """Run the external subcommand FIRST, then assert this layout's
            expected outcome: a real local target with the tool's own directory,
            no receipt, and no tool-directories notice.

            `real_target_first` states the premise of the never-converted
            control (a real target directory with no receipt is the per-checkout
            opt-out, so it exists before the command). The ephemeral checkout
            starts with NO target: that command's own Cargo build creates the
            real local one, which is exactly the unchanged behaviour to prove.
            """
            target = os.path.join(checkout, "target")
            if real_target_first:
                expect(
                    os.path.isdir(target) and not os.path.islink(target),
                    f"{label}: the never-converted control has no real target",
                )
            else:
                expect(
                    not os.path.lexists(target),
                    f"{label}: the ephemeral fixture already has a target",
                )
            if os.path.exists(tool_marker):
                os.remove(tool_marker)
            code, _, stderr = oxide_env(
                proof, ["oxfixture"], f"{label}: external subcommand", checkout, external_env
            )
            expect(code == 0, f"{label}: the external subcommand failed: {stderr}")
            expect(
                os.path.isfile(tool_marker) and read_bytes(tool_marker) == b"oxfixture",
                f"{label}: the external tool did not run",
            )
            expect(
                os.path.isdir(target) and not os.path.islink(target),
                f"{label}: the checkout's target is not a real local directory",
            )
            expect(
                os.path.isfile(os.path.join(target, "oxfixture", "out")),
                f"{label}: the tool wrote no local directory",
            )
            expect(
                not os.path.exists(receipt_file(pool, checkout)),
                f"{label}: the checkout gained an output receipt",
            )
            expect(
                not [
                    line
                    for line in stderr.splitlines()
                    if "tool directories kept local" in line
                ],
                f"{label}: the tool-directory notice changed an unchanged layout's stderr: {stderr}",
            )

        # a) Never converted: a real target with no receipt at all.
        unconverted = fresh_checkout(proof, root, "external-unconverted")
        os.makedirs(os.path.join(unconverted, "target"))
        unchanged_layout(unconverted, "25 unconverted", real_target_first=True)

        # b) Ephemeral drive mode: the command's own build creates a real target.
        # The mode is restored ONLY after the external child and every oracle
        # above completed: a `finally` would issue a pool command while writer or
        # device custody is unknown and could mask the first abort.
        set_mode(proof, "ephemeral", maker)
        ephemeral = fresh_checkout(proof, root, "external-ephemeral")
        unchanged_layout(ephemeral, "25 ephemeral", real_target_first=False)
        restore_after_case(
            completed=True, restore=lambda: set_mode(proof, "permanent", maker)
        )
        phase(proof, "after the unchanged-layout notice controls")

    bodies = {
        "s4b": case_s4b,
        "01": case_01,
        "02": case_02,
        "03": case_03,
        "04": case_04,
        "05": case_05,
        "06": case_06,
        "07+14": case_07_14,
        "08": case_08,
        "09": case_09,
        "16": case_16,
        "17": case_17,
        "18": case_18,
        "13": case_13,
        "10": case_10,
        "12": case_12,
        "11": case_11,
        "15": case_15,
        "19": case_19,
        "20": case_20,
        "21": case_21,
        "22": case_22,
        "23": case_23,
        "24": case_24,
        "25": case_25,
    }
    expect(
        sorted(bodies) == sorted(case for case, _requires, _text in EXEC_CASES),
        "the exec-inference case table and the case bodies disagree",
    )
    cases = [(case, requires, bodies[case]) for case, requires, _text in EXEC_CASES]

    def after_case(case: str) -> None:
        require_case_custody(
            unclosed=proof.unclosed,
            mount=mount,
            registration=registration_path(pool),
            case=case,
        )
        require_case_resources(peak_bytes=proof.peak, low_bytes=proof.low, case=case)

    # Filled in place: a fault that escapes the collector still leaves every
    # case already reached and every failure already recorded in the receipt.
    collect_cases(
        cases,
        diagnostic=diagnostic,
        work_left=guard_work_left,
        after_case=after_case if diagnostic else None,
        record=collection,
    )
    finish_exec_inference(proof, collection, diagnostic=diagnostic, mount=mount)


def exec_run(proof: Proof, repo: str, diagnostic: bool = False) -> None:
    """One exec-inference run. The counted run is fail-fast and detaches once at
    the end; the uncounted diagnostic collects every case, detaches only when it
    recorded no failure and did not abort, and always writes its bounded receipt
    — including when the run fails or aborts before the first case ran."""
    collection = Collection()
    proof.collection = collection
    failure: BaseException | None = None
    try:
        exec_inference(proof, repo, diagnostic, collection)
    except BaseException as error:
        failure = error
        if collection.aborted is None and (diagnostic or uncertain_outcome(error)):
            # The collection latches its own abort and records its own failures;
            # anything that escaped it (an interrupt, staging, the image, the
            # drive registration, the detach check) is named here, so the
            # receipt never implies a clean run — and, in EITHER mode, so the
            # failure cleanup cannot re-detach or re-operate an image whose
            # device or writer outcome is unknown.
            where = (
                "after the last recorded case"
                if collection.outcomes
                else "before the first case"
            )
            collection.aborted = f"{where}: {error!r}"
        raise
    finally:
        if diagnostic:
            finish_diagnostic(proof, collection, failure)


UNCONVERTED_NOTICE = "not converted to owned outputs"


def unconverted_notices(stderr: str) -> list[str]:
    """The one notice an unconverted checkout prints in permanent mode."""
    return [line for line in stderr.splitlines() if UNCONVERTED_NOTICE in line]


def guard_work_left() -> float:
    """Seconds left before the supervised guard's own work deadline, less this
    script's shutdown margin. Never a fresh process-relative budget."""
    deadline = int(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "0"))
    if not deadline:
        return 0.0
    return (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S


def track_member(proof: Proof, checkout: str, member: str = "debug") -> None:
    """Track `target/<member>` in the index while the working tree holds no such
    file.

    The `target` directory may be absent (the ineligible-build fixture) or a
    real empty directory the explicit-adopt refusal fixture created on purpose
    (`ignore_fixture(..., empty=True)`). This creates the directory only when it
    is absent and leaves a pre-existing one exactly as it is, so both callers'
    target oracles stay unchanged; a non-directory path still raises the
    ordinary error. Only the member file is staged and then removed."""
    target = os.path.join(checkout, "target")
    created = False
    if not os.path.isdir(target):
        # A file, link or other non-directory path keeps raising here.
        os.makedirs(target)
        created = True
    path = os.path.join(target, member)
    with open(path, "wb") as handle:
        handle.write(b"tracked")
    added = proof.run(["git", "add", "-f", os.path.join("target", member)], cwd=checkout, timeout=30)
    expect(added.returncode == 0, f"git add failed: {added.stderr!r}")
    os.remove(path)
    if created:
        os.rmdir(target)


def auto_ineligible_run(proof: Proof, repo: str, diagnostic: bool) -> None:
    """Focused ineligibility proof (oxide#154): expected automatic-adoption
    ineligibility must not fail an ordinary build.

    Since oxide#160 the automatic first conversion creates the profile layout,
    so its ignore prerequisite is asked for every member link it would publish
    (`target/debug`, `target/release`). When Git cannot confirm one of them -
    no repository, no Git on PATH, an unignored or negated path, an already
    tracked member, or a Git that does not answer - the command is an ordinary
    unconverted checkout: the build reaches Cargo into a real local target,
    prints exactly one unconverted notice, and writes no receipt, owned member
    link, owned root or downgrade fence. A covering rule, including a
    directory-only one that genuinely covers the member, still converts.
    Explicit adoption over a real empty target still refuses with the target,
    receipt and drive roots unchanged; a corrupt receipt and a partially
    converted checkout still fail closed; an existing real target stays the
    per-checkout opt-out; and `oxide init` names the rule that covers the
    member links without rewriting any rule. Every case uses its own checkout,
    so a failure never taints another case's premise.

    With `diagnostic` the same cases run keep-going: a recoverable oracle
    failure is recorded and independent cases continue, an unclosed child or an
    exhausted guard deadline aborts, and the receipt is `qualified=false`. A
    clean collection makes exactly one ordinary detach of this run's owned
    image and verifies the mount is gone before its receipt is written. It is
    never a counted PASS.
    """
    source = proof.run(["git", "rev-parse", "HEAD"], cwd=repo, timeout=30).stdout.decode().strip()
    dirty = proof.run(["git", "status", "--porcelain"], cwd=repo, timeout=30).stdout.decode().strip()
    expect(not dirty, f"candidate checkout is not clean: {dirty}")
    name = "OxAi" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    print(
        f"auto-ineligible proof {os.getpid()} source {source} "
        f"({layout.source_facts(repo)}) mount {mount} diagnostic {diagnostic}"
    )
    proof.phase("start")
    proof.binary = layout.stage_at(proof, repo, "oxide-auto-ineligible")
    proof.phase("after staging")

    root = proof.root
    pool = os.path.join(root, "pool")
    home = os.path.join(root, "home")
    cargo_home = os.path.join(root, "cargo-home")
    for path in (home, cargo_home):
        os.makedirs(path, mode=0o700)
    proof.env = proof_env(pool, home, cargo_home)
    proof.fixture_ready = True
    freeze_compiler_flags(proof.env)

    # The shared premises: an internal pool, then one owned 128 MiB image
    # registered as the permanent removable pool. Without them no command
    # selects a permanent removable pool, so nothing would be attempted.
    maker = fresh_checkout(proof, root, "pool-maker")
    code, _, stderr = proof.oxide(["build"], "internal pool before the drive", maker)
    expect(code == 0, stderr)
    mounted = attach_image(proof, os.environ["OXIDE_PROOF_EVIDENCE"], name)
    expect(mounted == mount, f"scratch volume mounted at {mounted}, not {mount}")
    code, stdout, stderr = proof.oxide(
        [
            "pool",
            "register-removable",
            mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "register permanent drive",
        maker,
    )
    expect(code == 0 and "registered removable pool" in stdout, stderr)
    proof.phase("after drive registration")

    def ineligible(label: str, checkout: str, env: dict | None = None) -> None:
        """One ineligible lease: the build reaches Cargo into a real local
        target with exactly one notice and no owned-output effect."""
        before = roots_on(mount)
        if env is None:
            code, _, stderr = proof.oxide(["build"], label, checkout)
        else:
            code, _, stderr = oxide_env(proof, ["build"], label, checkout, env)
        expect(code == 0, f"{label}: ineligibility failed the build: exit {code}: {stderr}")
        expect(not auto_notices(stderr), f"{label}: an ineligible checkout was converted: {stderr}")
        expect(
            len(unconverted_notices(stderr)) == 1,
            f"{label}: expected exactly one unconverted notice: {stderr}",
        )
        target = os.path.join(checkout, "target")
        expect(
            os.path.isdir(target) and not os.path.islink(target),
            f"{label}: the build left no real local target",
        )
        debug = os.path.join(target, "debug")
        expect(
            os.path.isdir(debug) and not os.path.islink(debug),
            f"{label}: the debug member is not a real local directory",
        )
        expect(
            os.path.isfile(os.path.join(debug, "poutputs")),
            f"{label}: the build produced no binary in the real local target",
        )
        expect(not os.path.exists(receipt_file(pool, checkout)), f"{label}: a receipt was written")
        expect(roots_on(mount) == before, f"{label}: the drive's roots changed")

    def adopt_refuses(
        label: str, checkout: str, reasons: tuple, env: dict | None = None
    ) -> None:
        """Explicit adoption over a real empty target still refuses: the
        member's prerequisite is checked before any receipt, root or link."""
        refused_untouched(
            proof, pool, mount, checkout, label, ["outputs", "adopt"], reasons, empty=True, env=env
        )

    def no_repository() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-no-repository", None, git=False)
        ineligible("no Git repository", checkout)
        _, adopt = ignore_fixture(proof, root, "ai-no-repository-adopt", None, git=False, empty=True)
        adopt_refuses("no repository: explicit adopt still refuses", adopt, ("could not confirm",))

    def no_git() -> None:
        env = dict(proof.env, PATH=private_path(root))
        _, checkout = ignore_fixture(proof, root, "ai-no-git", "/target\n/.oxide-outputs/\n")
        ineligible("no Git binary on PATH", checkout, env)
        _, adopt = ignore_fixture(
            proof, root, "ai-no-git-adopt", "/target\n/.oxide-outputs/\n", empty=True
        )
        adopt_refuses(
            "no Git on PATH: explicit adopt still refuses",
            adopt,
            ("could not confirm", "cannot run git"),
            env,
        )

    def unignored() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-unignored", "/.oxide-outputs/\n")
        ineligible("an unignored target", checkout)
        _, adopt = ignore_fixture(proof, root, "ai-unignored-adopt", "/.oxide-outputs/\n", empty=True)
        stderr = refused_untouched(
            proof,
            pool,
            mount,
            adopt,
            "unignored target: explicit adopt still refuses",
            ["outputs", "adopt"],
            ("does not ignore",),
            empty=True,
        )
        expect(
            "`/target/debug`" in stderr,
            f"the refusal did not name the exact ignored member path: {stderr}",
        )

    def negated() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-negated", "target\n!/target\n/.oxide-outputs/\n")
        ineligible("a negated target rule", checkout)
        _, adopt = ignore_fixture(
            proof, root, "ai-negated-adopt", "target\n!/target\n/.oxide-outputs/\n", empty=True
        )
        adopt_refuses(
            "negated target rule: explicit adopt still refuses", adopt, ("does not ignore",)
        )

    def tracked() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-tracked", "/target\n/.oxide-outputs/\n")
        track_member(proof, checkout)
        ineligible("an already tracked member path", checkout)
        _, adopt = ignore_fixture(proof, root, "ai-tracked-adopt", "/target\n/.oxide-outputs/\n", empty=True)
        track_member(proof, adopt)
        adopt_refuses(
            "tracked member path: explicit adopt still refuses", adopt, ("already tracks",)
        )

    def git_timeout() -> None:
        slow_dir = fake_git(root, "ineligible-timeout", "#!/bin/sh\nexec sleep 8\n")
        env = dict(proof.env, PATH=slow_dir + ":" + proof.env["PATH"])
        _, checkout = ignore_fixture(proof, root, "ai-slow-git", "/target\n/.oxide-outputs/\n")
        ineligible("a Git that does not answer in time", checkout, env)
        _, adopt = ignore_fixture(
            proof, root, "ai-slow-git-adopt", "/target\n/.oxide-outputs/\n", empty=True
        )
        adopt_refuses(
            "a Git that does not answer in time: explicit adopt still refuses",
            adopt,
            ("did not finish in time",),
            env,
        )

    def real_target_opt_out() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-real-target", "/.oxide-outputs/\n", empty=True)
        before = roots_on(mount)
        code, _, stderr = proof.oxide(["build"], "an existing real target is never adopted", checkout)
        expect(code == 0, f"a real-target checkout failed to build: {stderr}")
        expect(not auto_notices(stderr), stderr)
        expect(
            len(unconverted_notices(stderr)) == 1,
            f"expected exactly one unconverted notice: {stderr}",
        )
        target = os.path.join(checkout, "target")
        expect(
            os.path.isdir(target) and not os.path.islink(target),
            "the existing real target was replaced",
        )
        expect(not os.path.exists(receipt_file(pool, checkout)), "a real-target checkout wrote a receipt")
        expect(roots_on(mount) == before, "the drive's roots changed")

    def corrupt_receipt() -> None:
        _, checkout = ignore_fixture(
            proof, root, "ai-corrupt", "/.oxide-outputs/\n"
        )
        os.makedirs(os.path.dirname(receipt_file(pool, checkout)), exist_ok=True)
        corrupt = receipt_file(pool, checkout)
        with open(corrupt, "wb") as handle:
            handle.write(b"{not a receipt")
        os.chmod(corrupt, 0o600)
        before = roots_on(mount)
        code, _, stderr = proof.oxide(["build"], "a corrupt receipt still fails closed", checkout)
        expect(
            code != 0 and "is invalid" in stderr,
            f"a corrupt receipt was swallowed: exit {code}: {stderr}",
        )
        expect(not auto_notices(stderr), stderr)
        expect(
            not os.path.lexists(os.path.join(checkout, "target")),
            "a corrupt receipt let a target appear",
        )
        expect(roots_on(mount) == before, "a corrupt receipt created a root")
        os.remove(corrupt)

    def partial_conversion() -> None:
        _, checkout = ignore_fixture(proof, root, "ai-partial", "/target\n/.oxide-outputs/\n")
        crash_env = dict(proof.env, OXIDE_TEST_OUTPUTS_CRASH="prepared")
        code, _, stderr = oxide_env(
            proof, ["build"], "crash after the prepared receipt", checkout, crash_env
        )
        expect(code != 0 and "injected transition stop at prepared" in stderr, stderr)
        target = os.path.join(checkout, "target")
        debug = os.path.join(target, "debug")
        expect(
            os.path.isdir(target) and not os.path.islink(target),
            "the crash left no real target directory",
        )
        expect(not os.path.lexists(debug), "the crash published a member link")
        expect(read_json(receipt_file(pool, checkout))["state"] == "prepared", "no prepared receipt")
        before = roots_on(mount)
        with open(os.path.join(checkout, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/.oxide-outputs/\n")
        code, _, stderr = proof.oxide(
            ["build"], "an interrupted conversion refuses within a broken rule", checkout
        )
        expect(
            code != 0 and "does not ignore" in stderr,
            f"a partially converted checkout was continued: exit {code}: {stderr}",
        )
        expect(not os.path.lexists(debug), "the refused replay published a member link")
        expect(
            read_json(receipt_file(pool, checkout))["state"] == "prepared",
            "the refused replay rewrote the prepared receipt",
        )
        expect(roots_on(mount) == before, "the refused replay created a root")
        with open(os.path.join(checkout, ".gitignore"), "w", encoding="utf-8") as handle:
            handle.write("/target\n/.oxide-outputs/\n")
        code, stdout, stderr = proof.oxide(
            ["outputs", "adopt"], "the documented explicit adopt recovers the replay", checkout
        )
        expect(code == 0, f"the recovery adopt failed: exit {code}: {stdout}{stderr}")
        expect_profile_layout(pool, mount, checkout, "the recovered first conversion")

    def init_diagnostic() -> None:
        # A previous build's real `target/` beside a directory-only rule is
        # covered by the member links, so init stays silent (oxide#160); an
        # unignored path gets the exact rule that covers them, and no rule is
        # ever rewritten.
        repo_dir, checkout = ignore_fixture(
            proof, root, "ai-init-directory-only", "target/\n"
        )
        os.makedirs(os.path.join(checkout, "target"))
        code, stdout, stderr = proof.oxide(["init"], "init is silent for a directory-only rule", checkout)
        expect(code == 0, f"oxide init failed: exit {code}: {stderr}")
        expect(
            "does not ignore" not in stderr and "matches directories only" not in stderr,
            f"init diagnosed a rule that covers the member links: {stderr}",
        )
        expect(
            read_bytes(os.path.join(repo_dir, ".gitignore")) == b"target/\n/.oxide-outputs/\n",
            "oxide init rewrote the user's rule",
        )
        bare_repo, bare = ignore_fixture(proof, root, "ai-init-unignored", "")
        code, stdout, stderr = proof.oxide(["init"], "init names the covering rule", bare)
        expect(code == 0, f"oxide init failed on an unignored checkout: {stderr}")
        expect(
            "does not ignore" in stderr and "`/target`" in stderr,
            f"init did not name the rule that covers the member links: {stderr}",
        )
        expect(
            "leaves every .gitignore rule exactly as it is" in stderr,
            f"init did not promise not to rewrite: {stderr}",
        )
        expect(
            read_bytes(os.path.join(bare_repo, ".gitignore")) == b"/.oxide-outputs/\n",
            "init wrote a rule of its own",
        )
        ready_repo, ready = ignore_fixture(proof, root, "ai-init-ready", "/target\n")
        code, stdout, stderr = proof.oxide(["init"], "init is silent for a covering rule", ready)
        expect(code == 0, f"oxide init failed on a ready checkout: {stderr}")
        expect(
            "does not ignore" not in stderr and "matches directories only" not in stderr,
            f"init diagnosed a covering rule: {stderr}",
        )
        expect(
            read_bytes(os.path.join(ready_repo, ".gitignore")) == b"/target\n/.oxide-outputs/\n",
            "init changed a covering rule",
        )

    cases = [
        ("no-repository", no_repository),
        ("no-git", no_git),
        ("unignored", unignored),
        ("negated", negated),
        ("tracked", tracked),
        ("git-timeout", git_timeout),
        ("real-target-opt-out", real_target_opt_out),
        ("corrupt-receipt", corrupt_receipt),
        ("partial-conversion", partial_conversion),
        ("init-diagnostic", init_diagnostic),
    ]

    results: list[dict] = []
    failures: list[dict] = []
    aborted: str | None = None
    for case_name, body in cases:
        if aborted is not None:
            results.append({"case": case_name, "result": "unexecuted"})
            continue
        if guard_work_left() <= 5:
            fail("the guard's work deadline leaves no room for another case")
        print(f"case {case_name}", flush=True)
        try:
            body()
            if proof.unclosed:
                raise IneligibleAbort(f"unclosed children {sorted(proof.unclosed)}")
            results.append({"case": case_name, "result": "completed"})
        except IneligibleAbort as fault:
            aborted = f"{case_name}: {fault}"
            results.append({"case": case_name, "result": "aborted", "fault": str(fault)[:600]})
        except RuntimeError as fault:
            # Fail-fast is the counted run's contract. Keep-going records a
            # recoverable oracle failure, but an exhausted guard deadline is
            # not recoverable and stops the collection.
            if not diagnostic:
                raise
            if "deadline reached" in str(fault) or guard_work_left() <= 5:
                raise
            failures.append({"case": case_name, "fault": str(fault)[:600]})
            results.append({"case": case_name, "result": "failed", "fault": str(fault)[:600]})
        proof.phase(f"after {case_name}")

    # The owned image is this run's custody. A collection that recorded no
    # failure makes exactly one ordinary detach through the shared helper and
    # verifies the mount is gone before its receipt can report success; a failed
    # detach raises into the existing failure cleanup, which makes that same
    # single attempt and keeps the evidence. No path forces a detach, detaches
    # twice on the success path, or deletes the backing.
    detached = "not attempted (collection recorded a failure or an abort)"
    mount_present = None
    if not failures and aborted is None:
        proof.detach()
        expect(
            not os.path.ismount(mount),
            f"the owned image is still mounted at {mount} after its ordinary detach",
        )
        expect(
            not proof.unclosed,
            f"unclosed children after detach: {sorted(proof.unclosed)}",
        )
        mount_present = os.path.lexists(mount)
        detached = "ordinary detach through Proof.detach; mount absent"
    elapsed = time.monotonic() - layout.runner.STARTED
    document = {
        "proof": "rust:auto-adopt-ineligible",
        "mode": "diagnostic" if diagnostic else "counted",
        "source": source,
        "binary_sha256": sha256_file(proof.binary),
        "cases": results,
        "failures": failures,
        "aborted": aborted,
        "observations": {
            "elapsed_s": round(elapsed, 1),
            "peak_owned_mib": proof.peak // MIB,
            "host_free_min_mib": None if proof.low is None else proof.low // MIB,
            "detach": detached,
            "mount_path_present_after_detach": mount_present,
            "unclosed_children": sorted(proof.unclosed),
        },
        "limits": {"wall_clock_s": 300, "scratch_mib": 512},
    }
    if diagnostic:
        # This script's own collection receipt is never qualification evidence:
        # the guard's receipt and the counted run decide that.
        document["qualified"] = False
    text = json.dumps(document, indent=2, sort_keys=True)[: 64 * 1024]
    receipt = os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "auto-ineligible-receipt.json")
    with open(receipt, "w", encoding="utf-8") as handle:
        handle.write(text)
    label = "qualified=false" if diagnostic else "counted"
    print(f"auto-ineligible receipt {receipt} ({label})", flush=True)
    if failures or aborted:
        fail(
            "auto-ineligible cases did not all complete: "
            + json.dumps({"failures": failures, "aborted": aborted})
        )


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")

def overdue(signum, frame) -> None:
    raise RuntimeError("work deadline reached (SIGALRM fallback)")


def record(name: str, contents: str) -> None:
    with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], name), "w", encoding="utf-8") as handle:
        handle.write(contents)


def cleanup_after_failure(proof: Proof, collection: "Collection | None" = None) -> str:
    """Ordinary detach of this run's own mount after a failure, through
    Proof.detach's checks: refused while any child is unclosed, never forced,
    and bounded by the proof's own clock. A closed process group is not taken
    as a detached device; the mount path is checked afterwards.

    It applies the SAME retention gate as the clean tail (oxide#164 review): a
    latched abort, a recorded failure or an unclosed child means the image and
    its writers are not ours to touch, so failure cleanup retains the resource
    and records the refusal in the collection's detach disposition instead of
    detaching behind a custody abort. `collection` is `None` for a mode that has
    no keep-going collection, which keeps that mode's shipped behaviour."""
    # The alarm would cut the cleanup short; the proof clock still bounds it.
    signal.setitimer(signal.ITIMER_REAL, 0)
    image = getattr(proof, "image", os.path.join(proof.root, "volume.sparseimage"))
    mount = proof.mount
    if not mount:
        return f"no mount attached by this run (image {image})"
    if collection is not None:
        refusal = detach_decision(collection, proof.unclosed)
        if refusal is not None:
            collection.detached = f"retained: {refusal}"
            return f"retained {mount} (image {image}): {refusal}"
    try:
        proof.detach()
    except BaseException as error:
        if collection is not None:
            collection.detached = (
                f"unknown: failure cleanup could not confirm the detach of {mount} ({error!r})"
            )
        return f"detach of {mount} (image {image}) not completed: {error!r}"
    if os.path.ismount(mount):
        if collection is not None:
            collection.detached = (
                f"unknown: failure cleanup returned but {mount} is still mounted"
            )
        return f"detach of {mount} (image {image}) returned but the mount is still present"
    if collection is not None:
        collection.detached = "ordinary detach during failure cleanup; mount absent"
        collection.mount_present = os.path.lexists(mount)
    return f"ordinary detach of {mount} (image {image}) confirmed; mount no longer present"


def main() -> int:
    # `--plan` is effect-free and needs no supervisor: it prints the fixed
    # exec-inference table and allocation, then returns.
    if sys.argv[1:] == ["base-compat", "--plan"]:
        print("base-compat (de52775e): internal /private/tmp, 300 s, 512 MiB; pinned pre-S3 base downgrade case only")
        return 0
    if "--plan" in sys.argv[1:]:
        expect(
            [argument for argument in sys.argv[1:] if argument != "--plan"] == ["exec-inference"],
            "only the exec-inference mode has a --plan table",
        )
        plan_exec_inference()
        return 0
    # `--self-test` is the pure apparatus check of the keep-going collection: it
    # stages nothing, starts no child and touches no scratch, image or pool.
    if "--self-test" in sys.argv[1:]:
        expect(
            [argument for argument in sys.argv[1:] if argument != "--self-test"] == ["exec-inference"],
            "only the exec-inference mode has a --self-test",
        )
        exec_self_test()
        print(
            "exec-inference apparatus self-test passed: case outcomes, dependency "
            "skips, keep-going, abort latching, the receipt document, the runtime "
            "binding of every case body and the counted/diagnostic cleanup and "
            "device-outcome retention gates (no scratch, child, toolchain or image)"
        )
        return 0
    if sys.platform != "darwin":
        fail("the permanent-outputs proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    expect(
        os.path.isdir(os.environ.get("OXIDE_PROOF_EVIDENCE", ""))
        and "OXIDE_PROOF_DEADLINE_NS" in os.environ,
        "not under the qualified shared proof supervisor",
    )
    # Each focused mode is one scenario on the same fixture: `auto-adopt`
    # (oxide#121), `detach-permanent` (oxide#124), `ignore-safety` (oxide#125),
    # `exec-inference` (oxide#160) and `auto-ineligible` (oxide#154). No
    # argument runs the full #117 scenario. `--diagnostic` appends the
    # uncounted keep-going collection to `exec-inference` or to
    # `auto-ineligible`; it is never a counted native PASS. `--self-test` is the
    # pure apparatus check of that collection and touches nothing.
    arguments = list(sys.argv[1:])
    diagnostic = "--diagnostic" in arguments
    if diagnostic:
        arguments.remove("--diagnostic")
        expect(
            tuple(arguments) in ((), ("exec-inference",), ("auto-ineligible",), ("base-compat",), ("auto-adopt",), ("detach-permanent",), ("ignore-safety",)),
            "--diagnostic belongs to permanent-outputs, exec-inference, auto-ineligible or base-compat",
        )
    modes = {
        (): (lambda proof, repo: run(proof, repo, diagnostic=diagnostic), "permanent-outputs"),
        ("base-compat",): (lambda proof, repo: run(proof, repo, True), "base-compat"),
        ("auto-adopt",): (lambda proof, repo: auto_run(proof, repo, diagnostic), "auto-adopt"),
        ("detach-permanent",): (lambda proof, repo: detach_run(proof, repo, diagnostic), "detach-permanent"),
        ("ignore-safety",): (lambda proof, repo: ignore_run(proof, repo, diagnostic), "ignore-safety"),
        ("exec-inference",): (
            lambda proof, repo: exec_run(proof, repo, diagnostic),
            "exec-inference",
        ),
        ("auto-ineligible",): (
            lambda proof, repo: auto_ineligible_run(proof, repo, diagnostic),
            "auto-ineligible",
        ),
    }
    expect(tuple(arguments) in modes, f"unknown mode {arguments!r}")
    scenario, label = modes[tuple(arguments)]
    root = tempfile.mkdtemp(prefix="permanent-outputs.", dir=scratch)
    freeze_compiler_flags(os.environ)
    repo = os.path.dirname(HERE)
    signal.signal(signal.SIGTERM, terminated)
    signal.signal(signal.SIGALRM, overdue)
    signal.setitimer(signal.ITIMER_REAL, work_left())
    proof = Proof(root)
    proof.diagnostic = diagnostic
    if label in ("permanent-outputs", "auto-adopt", "detach-permanent", "ignore-safety", "base-compat"):
        proof.collection = Collection()
    if diagnostic and label == "base-compat":
        guard_writer_custody(proof)
    try:
        scenario(proof, repo)
    except BaseException as failure:
        if label in ("permanent-outputs", "auto-adopt", "detach-permanent", "ignore-safety"):
            record(label + "-cases.json", json.dumps({"mode": label, "kind": "diagnostic" if diagnostic else "counted", "qualified": False, "cases": getattr(proof.collection, "outcomes", {}), "failures": proof.collection.failures, "aborted": proof.collection.aborted, "failure": repr(failure)[:600]}))
        if label == "base-compat":
            if uncertain_outcome(failure):
                proof.collection.aborted = repr(failure)[:600]
            else:
                proof.collection.failures.append({"case": "historical-base", "fault": repr(failure)[:600]})
            record("base-compat-receipt.json", json.dumps({"mode": "base-compat", "kind": "diagnostic" if diagnostic else "counted", "cases": ([{"case": "historical-base", "outcome": "failed"}] if getattr(proof, "base_case_started", False) else []), "qualified": False, "failure": repr(failure)[:600]}))
        cleanup = cleanup_after_failure(proof, getattr(proof, "collection", None))
        record(
            "result.txt",
            f"failed: {failure!r}\nunclosed children: {sorted(proof.unclosed)}\n"
            f"root: {root}\ncleanup: {cleanup}\n",
        )
        print(
            f"{label} proof failed; scratch {root} left to the shared supervisor",
            file=sys.stderr,
        )
        if diagnostic:
            with open(os.path.join(os.environ["OXIDE_PROOF_EVIDENCE"], "custody-failure.json"), "w", encoding="utf-8") as handle:
                json.dump({"qualified": False, "failure": repr(failure), "cleanup": cleanup, "image": proof.image}, handle)
        proof.remove_backing()
        # The first fault is re-raised; a cleanup failure is only recorded.
        raise
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    if label in ("permanent-outputs", "auto-adopt", "detach-permanent", "ignore-safety"):
        record(label + "-cases.json", json.dumps({"mode": label, "kind": "diagnostic" if diagnostic else "counted", "qualified": False, "cases": proof.collection.outcomes, "failures": proof.collection.failures, "aborted": proof.collection.aborted}))
    if label == "base-compat":
        record("base-compat-receipt.json", json.dumps({"mode": "base-compat", "kind": "diagnostic" if diagnostic else "counted", "cases": [{"case": "historical-base", "outcome": "completed"}], "qualified": False}))
    expect(proof.detach_confirmed, f"ordinary detach unconfirmed; retained {proof.image}")
    proof.remove_backing()
    elapsed = time.monotonic() - layout.runner.STARTED
    summary_line = (
        f"{label} proof held: {elapsed:.0f} s script time, peak owned "
        f"{proof.peak // MIB} MiB at phase checks, phase-sampled host free minimum "
        f"{proof.low // MIB} MiB; one owned 128 MiB scratch image, ordinary detach, "
        f"shared supervisor owns cleanup"
    )
    outcome = (
        "diagnostic completed; uncounted, qualified=false"
        if diagnostic
        else "passed"
    )
    record("result.txt", f"{outcome}\n{summary_line}\n")
    print(summary_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
