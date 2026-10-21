"""`oxide exec` and the second-slot rule (oxide#107), on one private pool.

Stages this clean HEAD's `oxide` offline with plain Cargo and 1.97.1, then
runs a short fixed fixture tool (`tool.sh`) that calls the inherited `cargo`
itself, under a private HOME, CARGO_HOME and OXIDE_POOL. Cases, in order:

- environment and outputs: the tool's own `cargo build` writes
  intermediates in the held slot and the binary in <workspace>/target/debug;
  its `cargo metadata` reports target_directory <workspace>/target; the
  tool keeps its working directory, from the workspace and with
  --manifest-path from outside it; quick, debug and release recipes;
- #099 policy through the nested Cargo: a recording fake RUSTC_WRAPPER never
  runs by default and runs with OXIDE_ALLOW_RUSTC_WRAPPER=1;
- exit codes: a nonzero exit, a self-TERM (143) and a missing program (127);
- Ctrl-C to oxide reaches the tool, exit 130, slot released;
- the tool's leader exits while a group member lives: oxide keeps the slot
  held until that member exits;
- oxide killed while the tool's group lives: the slot stays held and is not
  reused (a further exec takes another slot); once the group exits by
  itself, the next command recovers it;
- concurrency in one checkout: quick and debug in distinct slots, a second
  quick command in another slot beside a held one, the host limit and the
  no-safe-slot wait, the waiting command then taking the released slot,
  and `oxide clean` waiting for the held exec.

Every fixture child is started by the tool and exits by itself when its
release file appears; nothing is signalled by number except oxide processes
this script started. Bounds: 300 s including cleanup under
`scripts/bounded-test.py` and Effigy heavy admission; the 20 GiB host reserve
and owned scratch (512 MiB) are sampled at phase boundaries only. Not a
benchmark and not full QA. Scratch is removed only after positive completion.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time

BUILDER = "1.97.1"
SCRATCH_LIMIT_BYTES = 512 * 1024 * 1024
HOST_RESERVE_BYTES = 20 * 1024 * 1024 * 1024
PROOF_SECONDS = 300
MIB = 1024 * 1024
STARTED = time.monotonic()
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PEAK = {"bytes": 0, "phase": "none"}

TOOL = r"""#!/bin/sh
# Fixture tool: calls the inherited cargo itself. Modes:
#   build OUT [--release] [--manifest-path P]  record cwd/env, cargo build, cargo metadata
#   exit N | selfterm                          exit codes
#   hold OUT                                   wait for OUT/release; INT records and exits 130
#   orphan OUT                                 leave a group member waiting for OUT/release
mode=$1
out=$2
case "$mode" in
build)
    shift 2
    release=
    manifest=
    while [ $# -gt 0 ]; do
        case "$1" in
        --release) release=--release ;;
        --manifest-path) manifest="--manifest-path=$2"; shift ;;
        esac
        shift
    done
    pwd -P > "$out/cwd"
    env | grep -E '^(CARGO_BUILD_BUILD_DIR|CARGO_TARGET_DIR|CARGO_PROFILE_DEV_DEBUG|CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG|RUSTC_WRAPPER)=' | sort > "$out/env"
    cargo build $release $manifest || exit $?
    cargo metadata --format-version 1 --no-deps $manifest > "$out/metadata.json" || exit $?
    ;;
exit) exit "$2" ;;
selfterm) kill -TERM $$ ;;
hold)
    trap 'touch "$out/interrupted"; exit 130' INT
    echo $$ > "$out/pid.tmp" && mv "$out/pid.tmp" "$out/pid"
    while [ ! -e "$out/release" ]; do sleep 0.05; done
    ;;
orphan)
    ( while [ ! -e "$out/release" ]; do sleep 0.05; done; touch "$out/member-done" ) &
    echo $! > "$out/member.tmp" && mv "$out/member.tmp" "$out/member"
    ;;
esac
"""

WRAPPER = """#!/bin/sh
echo "$@" >> "$OXIDE_FIXTURE_WRAPPER_LOG"
exec "$@"
"""


def fail(message: str) -> None:
    raise RuntimeError(message)


def remaining() -> float:
    left = PROOF_SECONDS - (time.monotonic() - STARTED)
    if left <= 0:
        fail(f"proof exceeded {PROOF_SECONDS} s")
    return left


def say(message: str) -> None:
    print(f"exec {time.monotonic() - STARTED:6.1f}s {message}", flush=True)


def reserve(path: str, phase: str) -> None:
    stat = os.statvfs(path)
    free = stat.f_bavail * stat.f_frsize
    say(f"reserve {phase}: host free {free // MIB} MiB")
    if free < HOST_RESERVE_BYTES:
        fail(f"host free {free} is below the 20 GiB reserve at {phase}")


def tree_bytes(root: str) -> int:
    total = 0
    seen = set()
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            stat = os.lstat(os.path.join(current, name))
            if (stat.st_dev, stat.st_ino) not in seen:
                seen.add((stat.st_dev, stat.st_ino))
                total += stat.st_blocks * 512
    return total


def owned(parent: str, phase: str) -> None:
    used = tree_bytes(parent)
    if used > PEAK["bytes"]:
        PEAK.update(bytes=used, phase=phase)
    say(f"owned scratch {phase}: {used // MIB} MiB")
    if used > SCRATCH_LIMIT_BYTES:
        fail(f"owned scratch {used} exceeds 512 MiB at {phase}")


def sha256(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def stage(parent: str, env: dict) -> str:
    target = os.path.join(parent, "stage")
    log = os.path.join(parent, "stage.log")
    with open(log, "w") as handle:
        build = subprocess.run(
            ["cargo", "build", "--locked", "--offline", "--bin", "oxide"],
            cwd=REPO, env={**env, "CARGO_TARGET_DIR": target, "CARGO_BUILD_BUILD_DIR": target},
            stdout=handle, stderr=subprocess.STDOUT, timeout=remaining(),
        )
    if build.returncode:
        with open(log) as handle:
            fail(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    out = os.path.join(parent, "bin", "oxide")
    os.makedirs(os.path.dirname(out))
    shutil.copy2(os.path.join(target, "debug", "oxide"), out)
    say(f"staged {out}: sha256 {sha256(out)}")
    owned(parent, "after staging")
    shutil.rmtree(target)
    return out


def write(path: str, text: str, mode: int = 0o600) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as handle:
        handle.write(text)
    os.chmod(path, mode)


def write_fixture(root: str) -> None:
    write(os.path.join(root, "Cargo.toml"), '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n')
    write(os.path.join(root, "rust-toolchain.toml"), f'[toolchain]\nchannel = "{BUILDER}"\n')
    write(os.path.join(root, "src", "main.rs"), 'fn main() {\n    println!("app");\n}\n')


def wait_until(predicate, label: str, process=None, seconds: float = 60) -> None:
    deadline = time.monotonic() + min(seconds, remaining())
    while not predicate():
        if process is not None and process.poll() is not None:
            fail(f"{label}: oxide exited {process.returncode}")
        if time.monotonic() > deadline:
            fail(f"{label}: timeout")
        time.sleep(0.05)


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True


class Proof:
    def __init__(self, parent: str, oxide: str, env: dict, app: str, tool: str):
        self.parent = parent
        self.oxide = oxide
        self.pool = os.path.join(parent, "pool")
        self.env = {**env, "OXIDE_POOL": self.pool}
        self.app = app
        self.tool = tool
        self.started: list = []
        self.outs = 0

    def out(self, name: str) -> str:
        self.outs += 1
        path = os.path.join(self.parent, "out", f"{self.outs:02d}-{name}")
        os.makedirs(path)
        return path

    def start(self, args: list, cwd: str | None = None, extra: dict | None = None, name: str = "oxide"):
        log = os.path.join(self.parent, "logs", f"{len(self.started):02d}-{name}.err")
        os.makedirs(os.path.dirname(log), exist_ok=True)
        handle = open(log, "w")
        process = subprocess.Popen([self.oxide, *args], cwd=cwd or self.app, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=handle, text=True,
                                   env={**self.env, **(extra or {})})
        handle.close()
        self.started.append(process)
        return process, log

    def finish(self, process, log: str, label: str, seconds: float = 120) -> tuple:
        try:
            stdout, _ = process.communicate(timeout=min(seconds, remaining()))
        except subprocess.TimeoutExpired:
            fail(f"{label}: oxide did not finish")
        with open(log) as handle:
            stderr = handle.read()
        return process.returncode, stdout, stderr

    def call(self, args: list, label: str, cwd: str | None = None, extra: dict | None = None) -> tuple:
        process, log = self.start(args, cwd, extra, label.replace(" ", "-"))
        return self.finish(process, log, label)

    def exec(self, args: list, label: str, cwd: str | None = None, extra: dict | None = None) -> tuple:
        return self.call(["exec", *args], label, cwd, extra)

    def ledger(self) -> dict:
        with open(os.path.join(self.pool, "ledger.json")) as handle:
            return {str(slot["id"]): slot for slot in json.load(handle)["slots"]}

    def held(self) -> list:
        return sorted(slot for slot, record in self.ledger().items() if record["state"] == "held")

    def status(self, label: str) -> str:
        code, stdout, stderr = self.call(["status"], f"{label} status")
        if code:
            fail(f"{label}: status exit {code}: {stderr[-1200:]}")
        return stdout


def summary(stderr: str, label: str) -> tuple:
    match = re.search(r"oxide: (\w+) profile, ([\w+]+) strategy, slot (\d+) (warm|cold), [\d.]+s, (.*)", stderr)
    if not match:
        fail(f"{label}: no summary line in {stderr[-2000:]!r}")
    return match.group(1), match.group(2), match.group(3), match.group(4), match.group(5)


def read_env(out: str) -> dict:
    with open(os.path.join(out, "env")) as handle:
        return dict(line.rstrip("\n").split("=", 1) for line in handle if "=" in line)


def built(proof: Proof, args: list, label: str, profile: str, cwd: str | None = None,
          extra: dict | None = None, tool_args: list | None = None) -> tuple:
    out = proof.out(label.replace(" ", "-"))
    code, _, stderr = proof.exec([*args, "--", proof.tool, "build", out, *(tool_args or [])], label, cwd, extra)
    if code:
        fail(f"{label}: exit {code}: {stderr[-2000:]}")
    shown, strategy, slot, warmth, tail = summary(stderr, label)
    env = read_env(out)
    with open(os.path.join(out, "cwd")) as handle:
        tool_cwd = handle.read().strip()
    with open(os.path.join(out, "metadata.json")) as handle:
        metadata = json.load(handle)
    target = os.path.join(proof.app, "target")
    build_dir = env.get("CARGO_BUILD_BUILD_DIR", "")
    say(f"{label}: slot {slot} {warmth}, {shown} profile, {strategy}; tool cwd {tool_cwd}; "
        f"build dir {build_dir}; metadata target_directory {metadata['target_directory']}; tail: {tail}")
    if shown != profile or strategy != "build+execute":
        fail(f"{label}: summary names {shown} profile, {strategy} strategy")
    if env.get("CARGO_TARGET_DIR") != target or metadata["target_directory"] != target:
        fail(f"{label}: target directory {env.get('CARGO_TARGET_DIR')} / {metadata['target_directory']}")
    # oxide#104: a new slot sits at its recorded `dir`.
    slot_dir = os.path.realpath(os.path.join(proof.pool, "payload", "slots", proof.ledger()[slot].get("dir") or slot))
    if not os.path.realpath(build_dir).startswith(slot_dir + "/") and os.path.realpath(build_dir) != slot_dir:
        fail(f"{label}: build dir {build_dir} is not in slot {slot} ({slot_dir})")
    output = "release" if "--release" in (tool_args or []) else "debug"
    deps = os.path.join(build_dir, output, "deps")
    if not any(name.startswith("app-") for name in os.listdir(deps)):
        fail(f"{label}: no app intermediates in {deps}")
    if not os.access(os.path.join(target, output, "app"), os.X_OK):
        fail(f"{label}: no binary in {target}/{output}")
    for stray in ("deps", "incremental", "build"):
        if os.path.exists(os.path.join(target, output, stray)):
            fail(f"{label}: plain-Cargo {stray}/ in {target}/{output}")
    if slot in proof.held():
        fail(f"{label}: slot {slot} still held after exit")
    return slot, warmth, env, tool_cwd, stderr


def prove_environment(proof: Proof, elsewhere: str, wrapper: str, wrapper_log: str) -> dict:
    slots = {}
    slot, _, env, tool_cwd, _ = built(proof, [], "default quick", "quick")
    if tool_cwd != proof.app:
        fail(f"default: tool ran in {tool_cwd}")
    if env.get("CARGO_PROFILE_DEV_DEBUG") != "line-tables-only" or "CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG" in env:
        fail(f"default: quick recipe missing: {env}")
    if env.get("RUSTC_WRAPPER") != "":
        fail(f"default: RUSTC_WRAPPER is {env.get('RUSTC_WRAPPER')!r}, not the empty bypass")
    slots["quick"] = slot
    manifest = os.path.join(proof.app, "Cargo.toml")
    slot, warmth, _, tool_cwd, _ = built(proof, ["--manifest-path", manifest], "manifest path", "quick",
                                         cwd=elsewhere, tool_args=["--manifest-path", manifest])
    if tool_cwd != elsewhere or slot != slots["quick"] or warmth != "warm":
        fail(f"manifest path: cwd {tool_cwd}, slot {slot} {warmth}; expected {elsewhere}, slot {slots['quick']} warm")
    slot, _, env, _, _ = built(proof, ["--profile", "debug"], "debug", "debug")
    if env.get("CARGO_PROFILE_DEV_DEBUG") != "full" or env.get("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG") != "full":
        fail(f"debug: recipe missing: {env}")
    if slot == slots["quick"]:
        fail("debug shared the quick slot")
    slots["debug"] = slot
    slot, _, env, _, stderr = built(proof, ["--profile=release"], "release", "release", tool_args=["--release"])
    if "CARGO_PROFILE_DEV_DEBUG" in env or slot in (slots["quick"], slots["debug"]):
        fail(f"release: env {env}, slot {slot}")
    slots["release"] = slot
    evicted = re.findall(r"oxide: evicted idle slot \d+[^\n]*", stderr)
    say(f"release: eviction lines {evicted}")

    with open(wrapper_log, "w"):
        pass
    _, _, env, _, stderr = built(proof, [], "fake wrapper bypassed", "quick", extra={"RUSTC_WRAPPER": wrapper})
    if env.get("RUSTC_WRAPPER") != "" or os.path.getsize(wrapper_log):
        fail(f"wrapper bypass: env {env.get('RUSTC_WRAPPER')!r}, log {os.path.getsize(wrapper_log)} bytes")
    if f"rustc wrapper bypassed: {wrapper}" not in stderr:
        fail(f"wrapper bypass: summary does not name it: {stderr[-800:]}")
    with open(os.path.join(proof.app, "src", "main.rs"), "a") as handle:
        handle.write("// edited so rustc runs\n")
    _, _, env, _, stderr = built(proof, [], "fake wrapper opted in", "quick",
                                 extra={"RUSTC_WRAPPER": wrapper, "OXIDE_ALLOW_RUSTC_WRAPPER": "1"})
    calls = os.path.getsize(wrapper_log)
    say(f"wrapper opt-in: env {env.get('RUSTC_WRAPPER')!r}, log {calls} bytes")
    if env.get("RUSTC_WRAPPER") != wrapper or not calls or "unqualified" not in stderr:
        fail(f"wrapper opt-in: wrapper did not run or no warning: {stderr[-800:]}")
    return slots


def prove_exits(proof: Proof) -> None:
    for args, expected in ((["exit", "7"], 7), (["selfterm"], 128 + signal.SIGTERM),
                           (None, 127)):
        command = [proof.tool, *args] if args else [os.path.join(proof.parent, "absent-tool")]
        code, _, stderr = proof.exec(["--", *command], f"exit {expected}")
        say(f"exit case {command[1:] or command}: oxide exit {code}")
        if code != expected:
            fail(f"exit {expected}: oxide exited {code}: {stderr[-800:]}")
        if proof.held():
            fail(f"exit {expected}: slots held after exit: {proof.held()}")


def prove_interrupt(proof: Proof) -> None:
    out = proof.out("interrupt")
    process, log = proof.start(["exec", "--", proof.tool, "hold", out], name="interrupt")
    wait_until(lambda: os.path.exists(os.path.join(out, "pid")), "interrupt start", process)
    held = proof.held()
    status = proof.status("interrupt")
    line = next((line for line in status.splitlines() if f" held by exec {proof.tool} " in line), None)
    say(f"interrupt: held {held}; status line {line!r}")
    if len(held) != 1 or line is None:
        fail(f"interrupt: no readable exec holder in status:\n{status}")
    process.send_signal(signal.SIGINT)
    code, _, stderr = proof.finish(process, log, "interrupt", 30)
    say(f"interrupt: oxide exit {code}, tool interrupted {os.path.exists(os.path.join(out, 'interrupted'))}")
    if code != 130 or not os.path.exists(os.path.join(out, "interrupted")) or proof.held():
        fail(f"interrupt: exit {code}, held {proof.held()}: {stderr[-800:]}")


def prove_member(proof: Proof) -> None:
    out = proof.out("member")
    process, log = proof.start(["exec", "--", proof.tool, "orphan", out], name="member")
    wait_until(lambda: os.path.exists(os.path.join(out, "member")), "member start", process)
    with open(os.path.join(out, "member")) as handle:
        member = int(handle.read())
    time.sleep(1.0)
    held = proof.held()
    say(f"member: leader exited, member {member} alive {alive(member)}, oxide running {process.poll() is None}, held {held}")
    if process.poll() is not None or len(held) != 1 or not alive(member):
        fail(f"member: oxide exit {process.returncode} or slot not held while the member lives")
    write(os.path.join(out, "release"), "")
    code, _, stderr = proof.finish(process, log, "member", 30)
    say(f"member: oxide exit {code} after member closure, held {proof.held()}")
    if code or not os.path.exists(os.path.join(out, "member-done")) or proof.held():
        fail(f"member: exit {code}: {stderr[-800:]}")


def prove_crash(proof: Proof) -> None:
    out = proof.out("crash")
    process, log = proof.start(["exec", "--", proof.tool, "hold", out], name="crash")
    wait_until(lambda: os.path.exists(os.path.join(out, "pid")), "crash start", process)
    with open(os.path.join(out, "pid")) as handle:
        tool = int(handle.read())
    held = proof.held()
    process.send_signal(signal.SIGKILL)
    process.wait(timeout=30)
    if len(held) != 1 or not alive(tool):
        fail(f"crash: held {held}, tool alive {alive(tool)}")
    crashed = held[0]
    status = proof.status("crash")
    line = next((line for line in status.splitlines() if f"slot {crashed} " in line), "")
    say(f"crash: oxide {process.pid} killed, tool {tool} alive; status {line.strip()!r}")
    if f"held by an interrupted exec {proof.tool} (oxide pid {process.pid}" not in line \
            or "recovered once the group is empty" not in line:
        fail(f"crash: status does not show the interrupted exec holder:\n{status}")
    slot, warmth, _, _, stderr = built(proof, [], "beside crashed", "quick")
    say(f"crash: further exec took slot {slot} {warmth}; crashed slot {crashed} held {proof.held()}")
    if slot == crashed or proof.held() != [crashed] or "recovered slot" in stderr:
        fail(f"crash: the held slot was reused or recovered early: {stderr[-1200:]}")
    write(os.path.join(out, "release"), "")
    wait_until(lambda: not alive(tool), "crash tool exit", seconds=30)
    slot, warmth, _, _, stderr = built(proof, [], "after group exit", "quick")
    recovered = re.search(rf"recovered slot {crashed} after oxide pid {process.pid} died \(Cargo group (\d+) empty\)", stderr)
    say(f"crash: next exec slot {slot} {warmth}; recovery {recovered.group(0) if recovered else None}")
    if not recovered or proof.held():
        fail(f"crash: no ordinary recovery once the group exited: {stderr[-1200:]}")
    status = proof.status("after recovery")
    if not re.search(rf"recovered slot {crashed} \d+s ago: oxide pid {process.pid} died \(Cargo group \d+ empty\)", status):
        fail(f"crash: status does not log the recovery:\n{status}")


def prove_concurrency(proof: Proof) -> None:
    three = {"OXIDE_MAX_ACTIVE": "3"}
    out_a = proof.out("hold-a")
    a, a_log = proof.start(["exec", "--", proof.tool, "hold", out_a], extra=three, name="hold-a")
    wait_until(lambda: os.path.exists(os.path.join(out_a, "pid")), "hold a", a)
    slot_a = proof.held()[0]
    slot_b, _, _, _, stderr = built(proof, ["--profile", "debug"], "debug beside quick", "debug", extra=three)
    say(f"concurrency: quick exec holds slot {slot_a}; debug took slot {slot_b}")
    if slot_b == slot_a:
        fail("concurrency: debug shared the held quick slot")
    out_c = proof.out("hold-c")
    c, c_log = proof.start(["exec", "--", proof.tool, "hold", out_c], extra=three, name="hold-c")
    wait_until(lambda: os.path.exists(os.path.join(out_c, "pid")), "hold c", c)
    held = proof.held()
    slot_c = next(slot for slot in held if slot != slot_a)
    with open(c_log) as handle:
        c_err = handle.read()
    evicted = re.findall(r"oxide: evicted idle slot [^\n]*", c_err)
    say(f"concurrency: second quick exec took slot {slot_c} beside held {slot_a}; held {held}; {evicted}")
    if len(held) != 2 or "waiting for" in c_err:
        fail(f"concurrency: second quick command did not get its own slot: {c_err[-800:]}")
    status = proof.status("two holders")
    holders = [line for line in status.splitlines() if f" held by exec {proof.tool} " in line]
    if len(holders) != 2:
        fail(f"concurrency: status does not name both exec holders:\n{status}")

    out_d = proof.out("host-limit")
    d1, d1_log = proof.start(["exec", "--", proof.tool, "build", out_d], name="host-limit")
    wait_until(lambda: "waiting for a build slot (2 of 2 host-wide in use)" in open(d1_log).read(), "host limit", d1)
    d1.send_signal(signal.SIGINT)
    code, _, _ = proof.finish(d1, d1_log, "host-limit", 30)
    say(f"concurrency: host limit 2 waited, Ctrl-C cancelled with exit {code}")
    if code != 130 or proof.held() != held:
        fail(f"concurrency: host-limit wait cancel exit {code}, held {proof.held()}")

    out_e = proof.out("no-safe-slot")
    e, e_log = proof.start(["exec", "--", proof.tool, "build", out_e], extra=three, name="no-safe-slot")
    wait_until(lambda: "waiting for another oxide command in this checkout" in open(e_log).read(), "no safe slot", e)
    time.sleep(1.0)
    if e.poll() is not None or proof.held() != held:
        fail("concurrency: the waiting command did not keep waiting")
    write(os.path.join(out_c, "release"), "")
    code, _, c_err = proof.finish(c, c_log, "hold c", 30)
    code_e, _, e_err = proof.finish(e, e_log, "no safe slot", 120)
    _, _, slot_e, warmth_e, _ = summary(e_err, "no safe slot")
    say(f"concurrency: released slot {slot_c} (exit {code}); waiter took slot {slot_e} {warmth_e} (exit {code_e})")
    if code or code_e or slot_e != slot_c:
        fail(f"concurrency: waiter exit {code_e} took slot {slot_e}, expected {slot_c}: {e_err[-800:]}")

    clean, clean_log = proof.start(["clean"], name="clean")
    wait_until(lambda: "waiting for active oxide commands in this checkout" in open(clean_log).read(), "clean barrier", clean)
    time.sleep(0.5)
    if clean.poll() is not None or not os.path.isdir(os.path.join(proof.app, "target")):
        fail("concurrency: clean did not wait for the held exec")
    write(os.path.join(out_a, "release"), "")
    code_a, _, _ = proof.finish(a, a_log, "hold a", 30)
    code_clean, clean_out, clean_err = proof.finish(clean, clean_log, "clean", 60)
    say(f"concurrency: clean waited for exec slot {slot_a}, then exit {code_clean}; exec exit {code_a}")
    if code_a or code_clean or os.path.exists(os.path.join(proof.app, "target")) or proof.held():
        fail(f"concurrency: clean exit {code_clean}: {clean_out[-400:]} {clean_err[-800:]}")


def main() -> int:
    if sys.platform != "darwin":
        fail("the exec proof requires macOS")
    if subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True,
                      text=True, timeout=30).stdout.strip():
        fail("the checkout is not clean, so HEAD does not name its source")
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True,
                          text=True, timeout=30).stdout.strip()
    reserve("/private/tmp", "admission")
    made = subprocess.run(["mktemp", "-d", "/private/tmp/oxide-exec.XXXXXX"],
                          capture_output=True, text=True, timeout=30)
    if made.returncode:
        fail("mktemp failed")
    parent = os.path.realpath(made.stdout.strip())
    os.chmod(parent, 0o700)
    say(f"source {head}, scratch {parent}")
    proof = None
    try:
        stage_env = {name: value for name, value in os.environ.items()
                     if not name.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "CARGO_PROFILE_", "RUSTC",
                                             "SCCACHE", "OXIDE_"))
                     and name not in ("CARGO", "CARGO_INCREMENTAL", "RUSTUP_TOOLCHAIN")}
        stage_env.update({"RUSTUP_TOOLCHAIN": BUILDER, "RUSTC_WRAPPER": "", "CARGO_PROFILE_DEV_DEBUG": "0",
                          "CARGO_NET_OFFLINE": "true"})
        oxide = stage(parent, stage_env)
        reserve(parent, "after staging")

        home = os.path.join(parent, "home")
        for path in (home, os.path.join(parent, "cargo-home"), os.path.join(parent, "tmp"),
                     os.path.join(parent, "elsewhere")):
            os.makedirs(path, mode=0o700)
        # The caller's directory selects the toolchain (oxide#094): pin the
        # same one outside the workspace so --manifest-path keeps the key.
        write(os.path.join(parent, "elsewhere", "rust-toolchain.toml"), f'[toolchain]\nchannel = "{BUILDER}"\n')
        env = {name: value for name, value in stage_env.items()
               if not name.startswith("CARGO_") and name not in ("RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER")}
        env.update({"HOME": home, "CARGO_HOME": os.path.join(parent, "cargo-home"),
                    "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup"),
                    "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never",
                    "TMPDIR": os.path.join(parent, "tmp"),
                    "OXIDE_FIXTURE_WRAPPER_LOG": os.path.join(parent, "wrapper.log")})
        app = os.path.join(parent, "checkouts", "app")
        write_fixture(app)
        tool = os.path.join(parent, "tools", "tool.sh")
        write(tool, TOOL, 0o700)
        wrapper = os.path.join(parent, "tools", "wrapper.sh")
        write(wrapper, WRAPPER, 0o700)
        say(f"fixture tool sha256 {sha256(tool)}, wrapper sha256 {sha256(wrapper)}")
        proof = Proof(parent, oxide, env, app, tool)
        prove_environment(proof, os.path.join(parent, "elsewhere"), wrapper, os.path.join(parent, "wrapper.log"))
        owned(parent, "after environment")
        reserve(parent, "after environment")
        prove_exits(proof)
        prove_interrupt(proof)
        prove_member(proof)
        owned(parent, "after closure")
        prove_crash(proof)
        owned(parent, "after crash")
        reserve(parent, "after crash")
        prove_concurrency(proof)
        owned(parent, "after concurrency")
        reserve(parent, "after concurrency")
    except Exception:
        if proof is not None:
            open_processes = [process.pid for process in proof.started if process.poll() is None]
            if open_processes:
                print(f"exec proof: oxide processes still running: {open_processes}", file=sys.stderr)
        print(f"exec proof retained its owned scratch at {parent}", file=sys.stderr)
        raise
    shutil.rmtree(parent)
    elapsed = time.monotonic() - STARTED
    if elapsed > PROOF_SECONDS:
        fail(f"exec proof took {elapsed:.0f} s, over {PROOF_SECONDS} s")
    print(f"oxide exec proof held: source {head}, {elapsed:.0f} s including cleanup, "
          f"peak owned scratch sampled {PEAK['bytes'] // MIB} MiB ({PEAK['phase']}), scratch removed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
