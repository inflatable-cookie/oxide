"""oxide#096: qualify sccache 0.18.0 sharing against Oxide slot writer closure.

Models candidate (b) from docs/knowledge/evidence/sccache-096: serialized
server lifetimes over one owned persistent scratch cache. A task-owned
`cargo` launcher, run by the unchanged staged `oxide` as its Cargo group
leader, starts one foreground (SCCACHE_NO_DAEMON=1) server on a private
socket, runs the real Cargo with RUSTC_WRAPPER, then stops the server.
Oxide's shipped closure (killpg(group, 0) ESRCH before release or recovery)
is the only closure signal; server PID exit, idle timeout or Cargo's exit
are never treated as closure. This models a future integration; it is not
shipped support.

Effects, in order: admission (monotonic clock, 20 GiB reserve sampled every
250 ms), offline staging of `oxide` with 1.97.1 from the clean checkout into
a private CARGO_HOME seeded from Cargo.lock's cached archives, one fresh
128 MiB APFS pool, tiny cfg-if 1.0.0 fixtures on a second private
CARGO_HOME, isolated HOME/SCCACHE_CONF/SCCACHE_DIR/socket. Never contacts
the user's or Effigy's sccache server, cache or config. Not a benchmark,
not product compatibility and not full QA.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
SPEC = importlib.util.spec_from_file_location("oxide_cli", os.path.join(HERE, "test-oxide-cli.py"))
cli = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cli)

SCCACHE = "/opt/homebrew/bin/sccache"
SCCACHE_REAL = "/opt/homebrew/Cellar/sccache/0.18.0/bin/sccache"
SCCACHE_SHA256 = "bd984e1857b9587f3574ff2d13cd59acf2d92982d4684e0e40bcbbc0bbf1de6e"
SCCACHE_VERSION = "sccache 0.18.0"
CACHE_LIMIT_BYTES = 64 * 1024 * 1024
LOG_LIMIT_BYTES = 32 * 1024 * 1024
CARGO_LIMIT = 96
IDLE_SECONDS = 12
PAIRS = 3

# Run by `oxide` as `cargo`, so it leads Oxide's Cargo process group. Only
# check/build calls with FIXTURE_SCCACHE set get a server; every other call
# (Oxide's metadata gate) is plain Cargo. The server's stdio goes to files,
# not to Oxide's pipes.
LAUNCHER = """#!/bin/sh
printf '%s|%s|%s\\n' "$$" "$(pwd -P)" "$*" >> "@LOG@"
case " $* " in
  *" check "*|*" build "*) ;;
  *) exec "@REAL_CARGO@" "$@" ;;
esac
[ -n "$FIXTURE_SCCACHE" ] || exec "@REAL_CARGO@" "$@"
s="$FIXTURE_SCCACHE"
export SCCACHE_NO_DAEMON=1 SCCACHE_SERVER_UDS="$s/r/$$.sock" SCCACHE_DIR="$s/cache" \\
  SCCACHE_CACHE_SIZE=64M SCCACHE_CONF="$s/config.toml" SCCACHE_CACHED_CONF="$s/cached-config.toml" \\
  SCCACHE_IDLE_TIMEOUT="${FIXTURE_IDLE:-600}"
SCCACHE_LOG=debug SCCACHE_ERROR_LOG="$s/l/$$.server" "@SCCACHE@" --start-server </dev/null >"$s/l/$$.start" 2>&1 || exit 97
g=$(ps -o pgid= -p $$ | tr -d ' ')
ps -A -o pid=,pgid=,ppid=,lstart=,command= | awk -v g="$g" '$2==g' > "$s/l/$$.group"
if [ -n "$FIXTURE_HOLD" ]; then
  : > "$FIXTURE_HOLD.ready"
  while [ ! -e "$FIXTURE_HOLD.go" ]; do sleep 0.1; done
fi
RUSTC_WRAPPER="@SCCACHE@" "@REAL_CARGO@" "$@"
rc=$?
"@SCCACHE@" --show-stats --stats-format=json </dev/null >"$s/l/$$.stats" 2>&1
"@SCCACHE@" --stop-server </dev/null >"$s/l/$$.stop" 2>&1
exit $rc
"""

FIXTURE = {
    "Cargo.toml": '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\ncfg-if = "=1.0.0"\n',
    "rust-toolchain.toml": '[toolchain]\nchannel = "1.97.1"\n',
    "src/lib.rs": "cfg_if::cfg_if! {\n    if #[cfg(unix)] {\n        pub fn host() -> &'static str { \"unix\" }\n    } else {\n        pub fn host() -> &'static str { \"other\" }\n    }\n}\n",
    "Cargo.lock": "version = 4\n\n"
    + cli.lock_package("app", "0.1.0", deps=["cfg-if"])
    + cli.lock_package("cfg-if", "1.0.0", cli.CRATES_IO_SOURCE, cli.CFG_IF_CHECKSUM),
}


class Monitor:
    """Host reserve every 250 ms; owned bytes, logs and server PIDs."""

    def __init__(self, parent: str):
        self.parent = parent
        self.free_min = None
        self.samples = 0
        self.breach = None
        self.owned_peak = 0
        self.log_bytes = 0
        self.servers = {}
        # Owned writers: Popen children leading their own groups, and every
        # Cargo group leader the launcher log names.
        self.children = []
        self.cargo_log = None
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.loop, daemon=True)

    def loop(self) -> None:
        # Any failed or racing reading is an unknown: it aborts the writers
        # rather than silently ending the monitor.
        try:
            self.watch()
        except Exception as error:
            self.breach = self.breach or f"accounting failed: {error}"
            self.stop_writers()

    def watch(self) -> None:
        while not self.stop.is_set():
            try:
                free = cli.host_free_bytes(self.parent)
            except Exception as error:  # unknown reading stops the proof
                self.breach = f"reserve reading failed: {error}"
                self.stop_writers()
                return
            self.samples += 1
            self.free_min = free if self.free_min is None else min(self.free_min, free)
            if free < cli.HOST_RESERVE_BYTES:
                self.breach = f"host free {free} below the 20 GiB reserve"
            elif self.samples % 4 == 0:
                owned = cli.tree_bytes(self.parent, True)
                self.owned_peak = max(self.owned_peak, owned)
                if owned > cli.SCRATCH_LIMIT_BYTES:
                    self.breach = f"owned {owned} exceeds 512 MiB"
            if self.breach:
                self.stop_writers()
                return
            self.stop.wait(0.25)

    def groups(self) -> set:
        found = {child.pid for child in self.children}
        if self.cargo_log and os.path.exists(self.cargo_log):
            with open(self.cargo_log, encoding="utf-8") as handle:
                found.update(int(line.split("|", 1)[0]) for line in handle if line.split("|", 1)[0].isdigit())
        try:
            with open(os.path.join(self.parent, "pool", "oxide-managed", "state.json"), encoding="utf-8") as handle:
                found.update(r["process_group"] for r in json.load(handle)["records"] if r.get("process_group"))
        except FileNotFoundError:
            pass
        return found

    def stop_writers(self) -> None:
        """KILL every owned group (each launcher and staging build leads one)
        and every registered server."""
        for group in self.groups():
            try:
                os.killpg(group, signal.SIGKILL)
            except OSError:
                pass
        for pid in self.servers:
            try:
                os.kill(pid, signal.SIGKILL)
            except OSError:
                pass

    def closed(self, seconds: float = 10) -> list:
        """Positive closure: every owned group and server ESRCH. Returns what
        is still not ESRCH (or unreadable) after a finite wait."""
        deadline = time.monotonic() + seconds
        while True:
            try:
                open_groups = [g for g in self.groups() if cli.probe(os.killpg, g) != "ESRCH"]
            except Exception as error:
                return [f"unknown: {error}"]
            pending = open_groups + [f"server {pid}" for pid in self.live_servers()]
            if not pending or time.monotonic() > deadline:
                return pending
            time.sleep(0.1)

    def check(self, phase: str) -> None:
        if self.breach:
            cli.fail(f"{phase}: {self.breach}")
        owned = cli.tree_bytes(self.parent, True)
        self.owned_peak = max(self.owned_peak, owned)
        print(f"sample {phase}: {cli.remaining():.0f} s left, owned {owned // 1024} KiB, reserve samples {self.samples}, host free min {(self.free_min or 0) // (1024 * 1024)} MiB")
        if owned > cli.SCRATCH_LIMIT_BYTES:
            cli.fail(f"owned {owned} exceeds 512 MiB at {phase}")

    def emit(self, label: str, text: str) -> None:
        self.log_bytes += len(text.encode())
        if self.log_bytes > LOG_LIMIT_BYTES:
            cli.fail("retained output exceeds 32 MiB")
        print(f"----- {label} ({len(text.encode())} bytes) -----")
        print(text, end="" if text.endswith("\n") or not text else "\n")

    def live_servers(self) -> list:
        return [pid for pid in self.servers if cli.probe(os.kill, pid) != "ESRCH"]


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def clean_env(home: str, rustup_home: str) -> tuple:
    """Process environment without any SCCACHE_* or wrapper route; names of
    removed variables only, never values."""
    env = dict(os.environ)
    removed = []
    for name in list(env):
        if name.startswith("SCCACHE_") or name in (
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "CARGO",
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
            "RUSTC",
            "RUSTUP_TOOLCHAIN",
        ):
            env.pop(name)
            removed.append(name)
    env.update({"HOME": home, "RUSTUP_HOME": rustup_home, "RUSTUP_AUTO_INSTALL": "0", "CARGO_NET_OFFLINE": "true"})
    return env, sorted(removed)


def index_entry(name: str) -> str:
    if len(name) <= 2:
        return os.path.join(str(len(name)), name)
    if len(name) == 3:
        return os.path.join("3", name[0], name)
    return os.path.join(name[:2], name[2:4], name)


def seed_stage_home(repo: str, host: str, home: str) -> int:
    """Copy exactly Cargo.lock's registry archives and index entries from the
    host Cargo home; no download, no host config."""
    with open(os.path.join(repo, "Cargo.lock"), encoding="utf-8") as handle:
        lock = handle.read()
    packages = re.findall(r'name = "([^"]+)"\nversion = "([^"]+)"\nsource = "registry\+[^"]+"\nchecksum = "([0-9a-f]+)"', lock)
    copied = 0
    for index in sorted(os.listdir(os.path.join(host, "registry", "index"))):
        cache = os.path.join(host, "registry", "cache", index)
        if not all(os.path.isfile(os.path.join(cache, f"{n}-{v}.crate")) for n, v, _ in packages):
            continue
        for name, version, checksum in packages:
            crate = os.path.join(cache, f"{name}-{version}.crate")
            if sha256_file(crate) != checksum:
                cli.fail(f"cached {name} {version} does not match Cargo.lock's checksum")
            entry = os.path.join(host, "registry", "index", index, ".cache", index_entry(name.lower()))
            for source in (crate, entry, os.path.join(host, "registry", "index", index, "config.json")):
                if not os.path.isfile(source):
                    if source == entry:
                        cli.fail(f"no cached index entry for {name}")
                    continue
                target = os.path.join(home, os.path.relpath(source, host))
                os.makedirs(os.path.dirname(target), exist_ok=True)
                if not os.path.exists(target):
                    shutil.copyfile(source, target)
                    copied += os.path.getsize(source)
        print(f"staging seed: {len(packages)} locked archives (checksums verified) and index entries from {index}, {copied} bytes, no download")
        return copied
    cli.fail("the host Cargo home lacks Oxide's locked archives; the proof does not fetch")
    return 0


def unit_results(text: str) -> list:
    """Server log lines naming a compile result or a refusal reason."""
    keep = ("compile result", "cannot cache", "CannotCache", "not cacheable", "incremental", "UnhandledCompile")
    return [line for line in text.splitlines() if any(word in line for word in keep)]


class Proof:
    def __init__(self, parent: str, monitor: Monitor, runner, sc: str, real_cargo: str):
        self.parent = parent
        self.monitor = monitor
        self.runner = runner
        self.sc = sc
        self.real_cargo = real_cargo
        self.seen_logs = set()

    def checkout(self, name: str) -> str:
        path = os.path.join(self.parent, "w", name)
        cli.write_fixture(path, FIXTURE)
        return os.path.realpath(path)

    def wrapped(self, extra: dict | None = None) -> dict:
        env = {"FIXTURE_SCCACHE": self.sc}
        env.update(extra or {})
        return env

    def new_launches(self) -> list:
        logs = os.path.join(self.sc, "l")
        launches = sorted({name.split(".")[0] for name in os.listdir(logs)} - self.seen_logs)
        self.seen_logs.update(launches)
        return launches

    def collect(self, label: str) -> dict:
        """Server identities, unit results and stats of launches since the
        last call."""
        out = {"servers": [], "units": [], "stats": []}
        for launch in self.new_launches():
            base = os.path.join(self.sc, "l", launch)
            for suffix in ("group", "start", "stop", "stats", "server"):
                path = f"{base}.{suffix}"
                if not os.path.exists(path):
                    continue
                text = cli.read_text(path)
                if suffix == "server":
                    out["units"] += unit_results(text)
                    self.monitor.emit(f"{label} launcher {launch} server log (unit lines only of {len(text.encode())} bytes)", "\n".join(unit_results(text)) + "\n")
                    continue
                self.monitor.emit(f"{label} launcher {launch} {suffix}", text)
                if suffix == "group":
                    for line in text.splitlines():
                        fields = line.split(None, 8)
                        if len(fields) == 9 and fields[8].split()[0] == SCCACHE_REAL:
                            pid = int(fields[0])
                            self.monitor.servers[pid] = " ".join(fields[3:8])
                            out["servers"].append((pid, int(fields[1]), " ".join(fields[3:8])))
                if suffix == "stats" and text.lstrip().startswith("{"):
                    stats = json.loads(text).get("stats", {})
                    out["stats"].append({key: stats.get(key) for key in ("compile_requests", "requests_executed", "cache_hits", "cache_misses", "non_cacheable_compilations", "not_cached", "cache_writes")})
        return out

    def oxide(self, cwd: str, label: str, extra: dict | None = None, args: tuple = ("check",)) -> tuple:
        started = time.monotonic()
        code, stdout, stderr = self.runner.run(cwd, list(args), extra)
        elapsed = time.monotonic() - started
        self.monitor.emit(f"{label} stdout", stdout)
        self.monitor.emit(f"{label} stderr", stderr)
        cli.expect(code, 0, label, stderr)
        result = cli.summary(stderr, label)
        result["seconds"] = elapsed
        result.update(self.collect(label))
        # Closure: Oxide returned only after release, so every server this
        # command started must already be gone (ESRCH).
        for pid, _, start in result["servers"]:
            if cli.probe(os.kill, pid) != "ESRCH":
                cli.fail(f"{label}: server {pid} ({start}) outlived Oxide's release: decisive NO-GO")
        if any(slot["state"] != "idle" for slot in self.runner.lease_status()["slots"]):
            cli.fail(f"{label}: a slot is still held after the command returned")
        self.monitor.check(label)
        print(f"{label}: slot {result['slot']} {result['warmth']}, {elapsed:.2f} s, servers {result['servers']}, stats {result['stats']}")
        return result

    def held(self, cwd: str, label: str, idle: int):
        hold = os.path.join(self.parent, f"hold-{label}")
        child, log = self.runner.start(cwd, ["check"], label, self.wrapped({"FIXTURE_HOLD": hold, "FIXTURE_IDLE": str(idle)}))
        cli.wait_for(lambda: os.path.exists(hold + ".ready"), f"{label} server start", 60)
        found = self.collect(label)
        if len(found["servers"]) != 1:
            cli.fail(f"{label}: expected one foreground server in the group, found {found['servers']}")
        running = [record for record in self.runner.state()["records"] if record["phase"] == "running"]
        if len(running) != 1 or running[0]["process_group"] != found["servers"][0][1]:
            cli.fail(f"{label}: the server is not in the running record's Cargo group: {running} {found['servers']}")
        return child, log, hold, found["servers"][0], running[0]


def leader_pid(record: dict) -> int:
    return record["process_group"]


def prove(proof: Proof) -> dict:
    report = {}
    cache = os.path.join(proof.sc, "cache")
    # Miss, then a hit in a different checkout path and a fresh slot.
    w1 = proof.checkout("w1")
    miss = proof.oxide(w1, "S1 miss", proof.wrapped())
    w2 = proof.checkout("w2")
    hit = proof.oxide(w2, "S2 cross-checkout", proof.wrapped())
    if miss["slot"] == hit["slot"] or hit["warmth"] != "cold":
        cli.fail("S2 did not use a fresh cold slot")
    report["S1"] = {k: miss[k] for k in ("slot", "warmth", "seconds", "stats", "units", "servers")}
    report["S2"] = {k: hit[k] for k in ("slot", "warmth", "seconds", "stats", "units", "servers")}
    # Changed workspace source: rebuilt client-side, dependency untouched.
    with open(os.path.join(w2, "src", "lib.rs"), "a", encoding="utf-8") as handle:
        handle.write("pub fn changed() -> u8 { 2 }\n")
    changed = proof.oxide(w2, "S3 workspace change", proof.wrapped())
    report["S3"] = {k: changed[k] for k in ("slot", "warmth", "seconds", "stats", "units")}
    # Same checkout without the wrapper: does the slot identity differ?
    w3 = proof.checkout("w3")
    plain = proof.oxide(w3, "S4 no wrapper")
    records = {record["checkout"]: record for record in proof.runner.state()["records"]}
    report["S4"] = {
        "slot": plain["slot"],
        "wrapped_compatibility": records[w1]["compatibility"],
        "plain_compatibility": records[w3]["compatibility"],
        "same_key": records[w1]["compatibility"] == records[w3]["compatibility"],
    }
    # Ctrl-C at Oxide while the group holds a live server.
    w4 = proof.checkout("w4")
    child, log, hold, server, record = proof.held(w4, "S5-interrupt", 30)
    child.send_signal(signal.SIGINT)
    code = child.wait(timeout=min(60, cli.remaining()))
    gone = cli.probe(os.kill, server[0])
    proof.monitor.emit("S5 oxide stderr", cli.read_log(log))
    log.close()
    if gone != "ESRCH" or any(slot["state"] != "idle" for slot in proof.runner.lease_status()["slots"]):
        cli.fail(f"S5: server {server[0]} {gone} after Oxide returned {code}, or slot not released")
    report["S5"] = {"oxide_exit": code, "server": server, "server_after_exit": gone}
    # Cargo-only death: the launcher (group leader, standing as Cargo) is
    # killed; the server survives in the group and the slot stays held until
    # the group is empty.
    w5 = proof.checkout("w5")
    child, log, hold, server, record = proof.held(w5, "S6-cargo-death", IDLE_SECONDS)
    killed = time.monotonic()
    os.kill(leader_pid(record), signal.SIGKILL)
    time.sleep(2)
    alive = cli.probe(os.kill, server[0])
    held = [slot["state"] for slot in proof.runner.lease_status()["slots"] if slot["lease_id"] == record["lease_id"]]
    if child.poll() is not None or alive != "present" or held != ["held"]:
        cli.fail(f"S6: oxide {child.poll()}, server {alive}, slot {held} 2 s after the leader died")
    code = child.wait(timeout=min(IDLE_SECONDS + 30, cli.remaining()))
    released = time.monotonic() - killed
    gone = cli.probe(os.kill, server[0])
    proof.monitor.emit("S6 oxide stderr", cli.read_log(log))
    log.close()
    if gone != "ESRCH" or any(slot["state"] != "idle" for slot in proof.runner.lease_status()["slots"]):
        cli.fail(f"S6: server {gone} after Oxide returned {code}")
    report["S6"] = {"oxide_exit": code, "server": server, "held_while_server_alive": True, "release_after_kill_s": round(released, 2)}
    # Oxide death with the server alive: held, then recovered by the next
    # command once the group is empty.
    w6 = proof.checkout("w6")
    child, log, hold, server, record = proof.held(w6, "S7-oxide-death", IDLE_SECONDS)
    child.kill()
    child.wait(timeout=20)
    log.close()
    os.kill(leader_pid(record), signal.SIGKILL)
    code, stdout, _ = proof.runner.run(w6, ["status"])
    proof.monitor.emit("S7 status while server alive", stdout)
    if cli.probe(os.kill, server[0]) != "present" or f"held by an interrupted command (oxide pid {child.pid}, Cargo group {record['process_group']};" not in stdout:
        cli.fail("S7: the slot was not shown held while the server lived")
    if proof.runner.state().get("recoveries"):
        cli.fail("S7: recovered while the server lived")
    cli.wait_gone(os.killpg, record["process_group"], "S7 group after server idle exit", IDLE_SECONDS + 20)
    code, _, stderr = proof.runner.run(w6, ["check"])
    proof.monitor.emit("S7 follower stderr", stderr)
    cli.expect(code, 0, "S7 follower", stderr)
    if f"after oxide pid {child.pid} died (Cargo group {record['process_group']} empty)" not in stderr:
        cli.fail("S7: follower did not recover the slot")
    proof.collect("S7 follower")
    report["S7"] = {"server": server, "recovered_after_group_empty": True}
    # Three interleaved cold-slot pairs: no wrapper vs warm shared cache.
    pairs = []
    for index in range(PAIRS):
        order = ("plain", "wrapped") if index % 2 == 0 else ("wrapped", "plain")
        pair = {}
        for kind in order:
            path = proof.checkout(f"p{index}{kind[0]}")
            result = proof.oxide(path, f"pair {index} {kind}", proof.wrapped() if kind == "wrapped" else None)
            if result["warmth"] != "cold":
                cli.fail(f"pair {index} {kind} was not a cold slot")
            pair[kind] = {"seconds": round(result["seconds"], 3), "stats": result["stats"], "units": result["units"]}
        pair["difference_s"] = round(pair["wrapped"]["seconds"] - pair["plain"]["seconds"], 3)
        pairs.append(pair)
    differences = sorted(pair["difference_s"] for pair in pairs)
    report["pairs"] = pairs
    report["median_difference_s"] = differences[len(differences) // 2]
    apparent = allocated = files = 0
    for current, _, names in os.walk(cache):
        for name in names:
            metadata = os.lstat(os.path.join(current, name))
            apparent += metadata.st_size
            allocated += metadata.st_blocks * 512
            files += 1
    report["cache"] = {"apparent_bytes": apparent, "allocated_bytes": allocated, "files": files, "write_bytes": "UNKNOWN"}
    if allocated > CACHE_LIMIT_BYTES:
        cli.fail(f"sccache cache {allocated} exceeds 64 MiB")
    return report


def main() -> int:
    if sys.argv[1:]:
        cli.fail("usage: test-sccache-compatibility.py")
    if sys.platform != "darwin":
        cli.fail("the sccache compatibility proof requires macOS")
    cli.STARTED = time.monotonic()
    admitted = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    cli.reserve(os.environ.get("OXIDE_TASK_SCRATCH_ROOT") or "/private/tmp", "admission", "sccache")
    root = cli.scratch_root()
    parent = os.path.join(root, "sc096")
    os.makedirs(parent, mode=0o700, exist_ok=False)
    monitor = Monitor(parent)
    monitor.thread.start()
    print(f"sccache proof admitted {admitted}, scratch {parent}")
    repo = os.path.dirname(HERE)
    host_home = os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    rustup_home = os.environ.get("RUSTUP_HOME") or os.path.join(os.path.expanduser("~"), ".rustup")
    # Tool identity: the installed 0.18.0 only.
    if os.path.realpath(SCCACHE) != SCCACHE_REAL or sha256_file(SCCACHE_REAL) != SCCACHE_SHA256:
        cli.fail(f"{SCCACHE} is not the recorded 0.18.0 binary; the proof never installs or updates")
    home = os.path.join(parent, "home")
    os.makedirs(home, mode=0o700)
    base, removed = clean_env(home, rustup_home)
    print(f"environment: removed {removed} (names only); HOME, CARGO_HOME, SCCACHE_* are private")
    version = subprocess.run(
        [SCCACHE, "--version"], env={**base, "SCCACHE_CONF": os.path.join(home, "none.toml"), "SCCACHE_SERVER_UDS": os.path.join(home, "v.sock")},
        capture_output=True, text=True, timeout=30,
    )
    if version.returncode or version.stdout.strip() != SCCACHE_VERSION:
        cli.fail(f"sccache version {version.stdout!r}")
    # Clean candidate source, then offline staging into the scratch.
    status = subprocess.run(["git", "status", "--porcelain"], cwd=repo, capture_output=True, text=True, timeout=30)
    if status.returncode or status.stdout.strip():
        cli.fail("candidate checkout is not clean")
    commit = subprocess.run(["git", "rev-parse", "HEAD", "HEAD^{tree}"], cwd=repo, capture_output=True, text=True, timeout=30).stdout.split()
    stage_home = os.path.join(parent, "stage-home")
    os.makedirs(stage_home, mode=0o700)
    seed_stage_home(repo, host_home, stage_home)
    stage = dict(base)
    # The checkout's ancestors include ~/.cargo/config.toml, whose machine-wide
    # rustc-wrapper names sccache: an empty RUSTC_WRAPPER overrides it. As a
    # detector, any wrapper that still ran would meet a private socket, config
    # and cache, which must stay empty.
    stage_sc = os.path.join(parent, "stage-sccache")
    os.makedirs(stage_sc, mode=0o700)
    stage.update(
        {
            "RUSTUP_TOOLCHAIN": cli.BUILDER,
            "CARGO_HOME": stage_home,
            "CARGO_TARGET_DIR": os.path.join(parent, "stage-target"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "SCCACHE_NO_DAEMON": "1",
            "SCCACHE_SERVER_UDS": os.path.join(stage_sc, "s.sock"),
            "SCCACHE_DIR": os.path.join(stage_sc, "cache"),
            "SCCACHE_CONF": os.path.join(stage_sc, "none.toml"),
            "SCCACHE_CACHED_CONF": os.path.join(stage_sc, "cached.toml"),
        }
    )
    builder = cli.toolchain_identity(stage, repo)
    if builder != cli.MATRIX[cli.BUILDER]:
        cli.fail(f"builder is not {cli.BUILDER}: {builder}")
    build = subprocess.Popen(
        ["cargo", "build", "--locked", "--offline", "--bin", "oxide", "--bin", "oxide-storage-lease", "--message-format=json-render-diagnostics"],
        cwd=repo, env=stage, stdout=subprocess.PIPE, text=True, process_group=0,
    )
    monitor.children.append(build)
    try:
        out, _ = build.communicate(timeout=cli.remaining())
    except subprocess.TimeoutExpired:
        # Settle the staging group before failing: KILL, reap, then require
        # ESRCH for the whole group.
        os.killpg(build.pid, signal.SIGKILL)
        build.communicate()
        # Own bounded wait: cli.wait_gone consults the expired proof clock.
        deadline = time.monotonic() + 10
        while cli.probe(os.killpg, build.pid) != "ESRCH" and time.monotonic() < deadline:
            time.sleep(0.1)
        state = cli.probe(os.killpg, build.pid)
        print(f"staging timed out; group {build.pid} {state}; scratch {parent} held", file=sys.stderr)
        raise
    if monitor.breach or build.returncode:
        cli.fail(f"staging exit {build.returncode} {monitor.breach or ''}")
    executables = {}
    for line in out.splitlines():
        if line.startswith("{"):
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message.get("executable"):
                executables[message["target"]["name"]] = message["executable"]
    if set(executables) != {"oxide", "oxide-storage-lease"}:
        cli.fail(f"staging artifacts {sorted(executables)}")
    monitor.check("after staging, before seed removal")
    receipt = {
        "source_commit": commit[0],
        "source_tree": commit[1],
        "build": "cargo build --locked --offline --bin oxide --bin oxide-storage-lease (CARGO_PROFILE_DEV_DEBUG=0, private CARGO_HOME, no wrapper)",
        "toolchain": builder,
        "oxide_sha256": sha256_file(executables["oxide"]),
        "lease_sha256": sha256_file(executables["oxide-storage-lease"]),
        "sccache": {"path": SCCACHE, "real": SCCACHE_REAL, "sha256": SCCACHE_SHA256, "version": SCCACHE_VERSION},
        "kind": "staging receipt; the binary embeds no source attribution",
    }
    print("receipt " + json.dumps(receipt, sort_keys=True))
    if os.listdir(stage_sc):
        cli.fail(f"a compiler wrapper ran during staging: {os.listdir(stage_sc)}")
    shutil.rmtree(stage_home)
    monitor.check("after staging")
    monitor.cargo_log = os.path.join(parent, "cargo.log")
    runner = cli.Runner(
        executables["oxide"], executables["oxide-storage-lease"], parent,
        extra={"HOME": home, "RUSTUP_HOME": rustup_home, "RUSTUP_AUTO_INSTALL": "0"},
    )
    for name in removed:
        runner.env.pop(name, None)
    cli.seed_cargo_home(runner.cargo_home)
    sc = os.path.join(parent, "s")
    for sub in ("cache", "r", "l"):
        os.makedirs(os.path.join(sc, sub), mode=0o700)
    with open(os.path.join(sc, "config.toml"), "w", encoding="utf-8"):
        pass
    real_cargo = shutil.which("cargo", path=base.get("PATH"))
    cargo_log = os.path.join(parent, "cargo.log")
    launcher = os.path.join(runner.bin, "cargo")
    with open(launcher, "w", encoding="utf-8") as handle:
        text = LAUNCHER.replace("@REAL_CARGO@", real_cargo).replace("@SCCACHE@", SCCACHE).replace("@LOG@", cargo_log)
        if re.search(r"@[A-Z_]+@", text) or "/sccache_" in text or "SCCACHE_NO_DAEMON=1" not in text or '[ -n "$FIXTURE_SCCACHE" ]' not in text:
            cli.fail("launcher substitution changed more than its placeholders")
        handle.write(text)
    os.chmod(launcher, 0o755)
    inputs = sum(len(text.encode()) for text in FIXTURE.values())
    print(f"fixture inputs: {inputs} bytes per checkout")
    code, _, stderr = runner.run(
        parent, ["--pool", runner.pool, "init", "--cap-bytes", str(cli.IMAGE_BYTES), "--image-bytes", str(cli.IMAGE_BYTES)], program=runner.lease
    )
    cli.expect(code, 0, "pool init", stderr)
    proof = Proof(parent, monitor, runner, sc, real_cargo)
    payload = os.path.join(runner.pool, "payload")
    try:
        report = prove(proof)
    except Exception:
        if monitor.breach:
            monitor.stop_writers()
        pending = monitor.closed()
        if pending:
            print(f"UNKNOWN closure: {pending} not ESRCH; image {payload} and scratch {parent} stay held", file=sys.stderr)
        else:
            detach = subprocess.run(["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60)
            print(f"no owned server alive; detach exit {detach.returncode}; retained scratch {parent}", file=sys.stderr)
        raise
    with open(cargo_log, encoding="utf-8") as handle:
        calls = handle.read().splitlines()
    monitor.emit("launcher Cargo calls", "\n".join(calls) + "\n")
    total = len(calls) + 2
    report["cargo_processes"] = {"staging": 1, "toolchain_version": 1, "fixture": len(calls), "total": total}
    if total > CARGO_LIMIT:
        cli.fail(f"{total} Cargo driver processes exceed {CARGO_LIMIT}")
    pending = monitor.closed(0)
    if pending:
        cli.fail(f"owned groups or servers not closed before cleanup: {pending}")
    monitor.check("before detach")
    detach = subprocess.run(["/usr/bin/hdiutil", "detach", payload], capture_output=True, text=True, timeout=60)
    if detach.returncode:
        cli.fail(f"detach failed after positive closure: {detach.stderr}")
    monitor.stop.set()
    monitor.thread.join(timeout=5)
    if monitor.breach:
        cli.fail(monitor.breach)
    report["servers"] = monitor.servers
    report["resources"] = {"owned_peak_bytes": monitor.owned_peak, "host_free_min_bytes": monitor.free_min, "reserve_samples": monitor.samples, "output_bytes": monitor.log_bytes}
    shutil.rmtree(parent)
    if not os.environ.get("OXIDE_TASK_SCRATCH_ROOT"):
        os.rmdir(root)
    elapsed = time.monotonic() - cli.STARTED
    report["elapsed_s"] = round(elapsed, 1)
    print("report " + json.dumps(report, sort_keys=True))
    if elapsed > cli.PROOF_SECONDS:
        cli.fail(f"proof took {elapsed:.0f} s, over {cli.PROOF_SECONDS} s including cleanup")
    print(f"sccache proof completed: {elapsed:.0f} s including cleanup, scratch removed")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception:
        traceback.print_exc()
        sys.exit(1)
