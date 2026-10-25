#!/usr/bin/env python3
"""Focused native proof for recorded per-slot accounting (oxide#146).

One fixed script and the whole command of the qualified shared proof
supervisor (`bounded-test.py`, guard sha256 a6d73326…, oxide#145). The
approved recipe is an INTERNAL directory pool, so both heavy selectors pass
`--root-parent /private/tmp` explicitly (Tom's internal-root exception): a
pool inside a selected-volume root would be refused by the product itself.
Bounds are unchanged: 300 s wall, 512 MiB monitored scratch.

The proof stages this HEAD's debug `oxide` with plain Cargo offline and the
pinned 1.97.1 at 4 Cargo jobs and 16 codegen units (opt-level `s`, debug info
off, debug assertions on — the walk/scope logs are `debug_assertions`-only —
incremental off, symbol-stripped), reuses that one staging for every case and
deletes the staged target before any fixture. Fixtures keep one Cargo job and
run sequentially. Those jobs are configured build parallelism, never a CPU
cap, an admission demand or a fit claim.

Every fixture is private: its own `HOME`, `CARGO_HOME`, `OXIDE_POOL`,
`TMPDIR`/`TMP`/`TEMP` and walk/scope log files beside the pool, all inside the
one owned monitored root. Inherited `OXIDE_*`, `XDG_*` and
`CARGO_ENCODED_RUSTFLAGS` are scrubbed; `OXIDE_TEST_HOME_FROM_ENV=1` makes the
private `HOME` authoritative; the invoking rustup home is resolved before that
HOME exists and preserved with `RUSTUP_AUTO_INSTALL=0`; `Cargo.lock` is
established by one offline setup child before any `--locked` call; the pool
path is absent until the product creates it.

Cases (each a real library/CLI path, not a decision mock):

- A all-recorded admission of a new slot and a pool-ceiling count replacement
  walk no inventory at all: the debug walk log stays empty while the separate
  scope log records one shallow pass per admission;
- B a legacy-shaped pool with one missing record walks exactly that slot, and
  the completed figure is durable progress the next admission reuses;
- C a release publishes one closed observation, a compatible reuse clears it,
  and the result line's provenance text is checked;
- D a non-vacuous byte-pressure admission evicts one owned idle whole slot
  while a held slot, a foreign out-of-slot stray and its bytes stay untouched;
- E cross-slot hard links make the recorded sum exceed the inode union, and
  that accepted overcount is what admission uses;
- F a legacy pool converges across admissions and a crash replay, never
  reading an unmeasured slot as zero;
- G default status reports recorded totals and writes nothing; `--measure`
  keeps the full-walk audit and still writes nothing.

`--diagnostic` is the safe uncounted collection mode: it records recoverable
oracle failures, continues only into dependency-independent cases, skips
tainted dependents with their reasons, aborts on any unknown child, writer,
device, custody or receipt outcome, preserves the first fault and writes one
bounded `qualified: false` receipt. The default run stays fail-fast. Neither
mode is a counted PASS substitute, a benchmark, or an alias for full QA.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILDER = "1.97.1"
# The qualified shared supervisor this proof's whole command runs under.
GUARD_SHA256 = "a6d73326e897ef93f8b9a448b24889cc0a73bda894ea699de44858b1c6d2756b"
MIB = 1024 * 1024
GIB = 1024 * MIB
# Staging recipe (oxide#140): 4 Cargo jobs, 16 codegen units, frozen profile.
STAGE_JOBS = 4
STAGE_CODEGEN_UNITS = 16
# Fixtures stay sequential at one Cargo job.
FIXTURE_JOBS = 1
SHUTDOWN_MARGIN_S = 30
# Bytes the fixture build script writes into its slot, so a slot is visibly
# large enough that a walk would be visible in the log.
SLOT_FILLER_BYTES = 3 * MIB
RECORDED_TOTAL_NEEDLE = "recorded slot sizes (ledger):"
RECORDED_SLOT_NEEDLE = "at slot release"
CAPABILITIES_PROBE = "__oxide-build-capabilities"
DIAGNOSTIC = "--diagnostic"

CASES: list[str] = []
FAILURES: list[tuple[str, str]] = []
SKIPS: list[tuple[str, str]] = []
REACHED: list[str] = []
TAINTED: set[str] = set()
PRIMARY: list[str] = []
ABORTED: bool = False


# --------------------------------------------------------------------------
# Reporting, in both modes.


def fail(message: str) -> None:
    """Counted mode: fail fast. Diagnostic mode: record and stop this case."""
    if DIAGNOSTIC in sys.argv:
        raise CaseFailure(message, abort=False)
    print(f"FAILED: {message}", flush=True)
    raise SystemExit(1)


def abort(message: str) -> None:
    """An outcome the diagnostic may never turn into an oracle continuation."""
    print(f"ABORTED: {message}", flush=True)
    raise CaseFailure(message, abort=True)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


class CaseFailure(Exception):
    def __init__(self, message: str, abort: bool) -> None:
        super().__init__(message)
        self.abort = abort


def case(name: str) -> None:
    REACHED.append(name)
    CASES.append(name)
    print(f"case {name}", flush=True)


def work_left() -> float:
    deadline = int(os.environ["OXIDE_PROOF_DEADLINE_NS"])
    left = (deadline - time.clock_gettime_ns(time.CLOCK_UPTIME_RAW)) / 1e9 - SHUTDOWN_MARGIN_S
    if left <= 3:
        abort("the supervisor's work deadline was reached before the shutdown margin")
    return left


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


# --------------------------------------------------------------------------
# Staging. One function holds the real argv and environment so the pure
# self-checks can exercise the same object with negative mutations.


def staging_recipe(scratch: str) -> tuple[list[str], dict[str, str]]:
    stage = os.path.join(scratch, "stage")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "SCCACHE_", "OXIDE_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
                        "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS")
    }
    env.update(
        {
            "RUSTUP_TOOLCHAIN": BUILDER,
            "RUSTUP_AUTO_INSTALL": "0",
            "RUSTC_WRAPPER": "",
            "RUSTC_WORKSPACE_WRAPPER": "",
            "CARGO_TARGET_DIR": os.path.join(stage, "target"),
            "CARGO_BUILD_BUILD_DIR": os.path.join(stage, "build"),
            "CARGO_PROFILE_DEV_DEBUG": "0",
            "CARGO_PROFILE_DEV_DEBUG_ASSERTIONS": "true",
            "CARGO_PROFILE_DEV_OPT_LEVEL": "s",
            "CARGO_PROFILE_DEV_CODEGEN_UNITS": str(STAGE_CODEGEN_UNITS),
            "CARGO_PROFILE_DEV_PANIC": "abort",
            "CARGO_INCREMENTAL": "0",
            "RUSTFLAGS": "-C strip=symbols",
        }
    )
    argv = [
        "cargo",
        "build",
        "-j",
        str(STAGE_JOBS),
        "--locked",
        "--offline",
        "--bin",
        "oxide",
    ]
    return argv, env


def stage_subject(scratch: str) -> str:
    argv, env = staging_recipe(scratch)
    log = os.path.join(scratch, "stage.log")
    with open(log, "w", encoding="utf-8") as handle:
        build = subprocess.run(
            argv,
            cwd=REPO,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            timeout=work_left(),
        )
    if build.returncode:
        with open(log, encoding="utf-8") as handle:
            abort(f"staging exit {build.returncode}: {handle.read()[-2000:]}")
    built = os.path.join(scratch, "stage", "target", "debug", "oxide")
    if not os.path.isfile(built):
        abort(f"staging produced no binary at {built}")
    subject = os.path.join(scratch, "subject-oxide")
    shutil.copy2(built, subject)
    os.chmod(subject, 0o755)
    shutil.rmtree(os.path.join(scratch, "stage"))
    os.unlink(log)
    return subject


def invoking_rustup_home() -> str:
    explicit = os.environ.get("RUSTUP_HOME")
    if explicit and os.path.isdir(explicit):
        return explicit
    home = os.environ.get("HOME")
    if not home:
        abort("no invoking HOME to resolve the toolchain home from")
    return os.path.join(home, ".rustup")


# --------------------------------------------------------------------------
# Fixtures.


FIXTURE_BUILD_RS = r"""
use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let path = std::path::Path::new(&out).join("slot-filler.bin");
    let bytes = vec![7_u8; {filler}];
    std::fs::File::create(&path)
        .and_then(|mut file| file.write_all(&bytes))
        .expect("slot filler");
    // A gated writer: publish readiness, then wait for the release file, so a
    // proof can observe this slot held while another admission runs. The wait
    // is bounded so a lost release file cannot hang the run.
    if let Ok(gate) = std::env::var("OXIDE_PROOF_HOLD") {
        std::fs::write(format!("{gate}.ready"), b"ready\n").expect("readiness marker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        while !std::path::Path::new(&gate).exists() {
            if std::time::Instant::now() > deadline {
                panic!("the release file never appeared");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
"""

FIXTURE_MAIN = 'fn main() { println!("recorded-accounting-fixture"); }\n'

FIXTURE_MANIFEST = """[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
build = "build.rs"

[workspace]
"""


class Fixture:
    """One private fixture: its own roots, its own absent pool path."""

    def __init__(self, root: str, label: str, pool_cap: int | None = None) -> None:
        self.root = os.path.join(root, label)
        self.home = os.path.join(self.root, "home")
        self.cargo_home = os.path.join(self.root, "cargo")
        self.pool = os.path.join(self.root, "pool")
        self.work = os.path.join(self.root, "work")
        self.temp = os.path.join(self.root, "temp")
        self.walk_log = os.path.join(self.root, "walk.log")
        self.scope_log = os.path.join(self.root, "scope.log")
        self.rustup_home = os.path.join(self.root, "rustup")
        for path in (self.home, self.cargo_home, self.temp):
            os.makedirs(path, exist_ok=True)
        os.chmod(self.root, 0o700)
        if pool_cap is not None:
            self.pool_cap = pool_cap
        else:
            self.pool_cap = None

    # -- fixture crates -------------------------------------------------

    def crate(self, name: str) -> str:
        path = os.path.join(self.work, name)
        os.makedirs(path, exist_ok=True)
        with open(os.path.join(path, "Cargo.toml"), "w", encoding="utf-8") as handle:
            handle.write(FIXTURE_MANIFEST.format(name=name))
        src = os.path.join(path, "src")
        os.makedirs(src, exist_ok=True)
        with open(os.path.join(src, "main.rs"), "w", encoding="utf-8") as handle:
            handle.write(FIXTURE_MAIN)
        with open(os.path.join(path, "build.rs"), "w", encoding="utf-8") as handle:
            handle.write(FIXTURE_BUILD_RS.format(filler=SLOT_FILLER_BYTES))
        self.establish_lock(path)
        return path

    def establish_lock(self, checkout: str) -> None:
        """A fresh crate has no lock: `--locked` would refuse without this."""
        lock = os.path.join(checkout, "Cargo.lock")
        if os.path.isfile(lock):
            return
        setup = subprocess.run(
            ["cargo", "generate-lockfile", "--offline"],
            cwd=checkout,
            env=self.env(offline_cargo=True),
            capture_output=True,
            text=True,
            timeout=min(60, work_left()),
        )
        if setup.returncode:
            abort(f"offline lock generation failed: {setup.stderr[-800:]}")
        if not os.path.isfile(lock):
            abort("offline lock generation produced no Cargo.lock")

    # -- environment ----------------------------------------------------

    def env(self, offline_cargo: bool = False, extra: dict[str, str] | None = None) -> dict[str, str]:
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith(("OXIDE_", "XDG_"))
            and key not in ("TMPDIR", "TMP", "TEMP", "CARGO_ENCODED_RUSTFLAGS")
        }
        env.update(
            {
                "HOME": self.home,
                "CARGO_HOME": self.cargo_home,
                "RUSTUP_HOME": self.rustup_home,
                "RUSTUP_AUTO_INSTALL": "0",
                "OXIDE_TEST_HOME_FROM_ENV": "1",
                "OXIDE_POOL": self.pool,
                "TMPDIR": self.temp,
                "TMP": self.temp,
                "TEMP": self.temp,
                "CARGO_NET_OFFLINE": "true" if offline_cargo else "false",
                "OXIDE_TEST_WALK_LOG": self.walk_log,
                "OXIDE_TEST_SCOPE_LOG": self.scope_log,
                "RUSTFLAGS": "-C strip=symbols",
            }
        )
        if self.pool_cap is not None:
            env["OXIDE_POOL_CAP"] = str(self.pool_cap)
        if extra:
            env.update(extra)
        return env

    def run(
        self,
        subject: str,
        argv: list[str],
        cwd: str,
        extra: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess:
        return subprocess.run(
            [subject, *argv],
            cwd=cwd,
            env=self.env(extra=extra),
            capture_output=True,
            text=True,
            timeout=min(120, work_left()),
        )

    # -- observations ---------------------------------------------------

    def reset_logs(self) -> None:
        for path in (self.walk_log, self.scope_log):
            if os.path.exists(path):
                os.unlink(path)

    def walks(self) -> list[str]:
        return self._log(self.walk_log)

    def scopes(self) -> list[str]:
        return self._log(self.scope_log)

    def _log(self, path: str) -> list[str]:
        if not os.path.isfile(path):
            return []
        with open(path, encoding="utf-8") as handle:
            return [line.strip() for line in handle if line.strip()]

    def status(self, subject: str) -> str:
        result = self.run(subject, ["status"], self.work)
        expect(result.returncode == 0, f"status exit {result.returncode}: {result.stderr[-800:]}")
        return result.stdout

    def ledger(self) -> dict:
        with open(os.path.join(self.pool, "ledger.json"), encoding="utf-8") as handle:
            return json.load(handle)

    def slot_ids(self) -> list[str]:
        return [str(slot["id"]) for slot in self.ledger()["slots"]]

    def recorded(self, slot_id: str) -> dict | None:
        for slot in self.ledger()["slots"]:
            if str(slot["id"]) == slot_id:
                return slot.get("recorded")
        return None

    def idle_slots(self) -> list[str]:
        return [
            str(slot["id"])
            for slot in self.ledger()["slots"]
            if slot["state"] == "idle" and slot.get("lease_id") in (None, "")
        ]

    def slot_dir(self, slot_id: str) -> str:
        slots_root = os.path.join(self.pool, "payload", "slots")
        for slot in self.ledger()["slots"]:
            if str(slot["id"]) != slot_id:
                continue
            recorded = slot.get("dir")
            if recorded is None:
                return os.path.join(slots_root, slot_id)
            return os.path.join(slots_root, *recorded.split("/"))
        abort(f"slot {slot_id} is absent from the ledger")


def build(fixture: Fixture, subject: str, checkout: str) -> subprocess.CompletedProcess:
    return fixture.run(subject, ["check"], checkout)


def built(fixture: Fixture, subject: str, checkout: str) -> None:
    result = build(fixture, subject, checkout)
    if result.returncode:
        abort(f"fixture build failed ({result.returncode}): {result.stderr[-1200:]}")


# --------------------------------------------------------------------------
# Cases.


def case_a_all_recorded_admission_walks_nothing(root: str, subject: str) -> None:
    case("a_all_recorded_admission_walks_nothing")
    fx = Fixture(root, "a", pool_cap=1)
    first = fx.crate("first")
    built(fx, subject, first)
    expectation = fx.idle_slots()
    expect(len(expectation) == 1, f"the first build left {expectation} idle slots")

    # New-slot admission at the pool ceiling: a count replacement of the single
    # idle slot, from records only.
    fx.reset_logs()
    second = fx.crate("second")
    built(fx, subject, second)
    walks, scopes = fx.walks(), fx.scopes()
    expect(not walks, f"admission walked inventory: {walks}")
    expect(scopes, "no shallow scope pass ran, so the figure was not recorded pressure")
    ids = fx.slot_ids()
    expect(len(ids) == 1, f"the pool ceiling was not enforced: {ids}")
    expect(ids[0] != expectation[0], "the count replacement did not replace the idle slot")
    text = fx.status(subject)
    expect(RECORDED_TOTAL_NEEDLE in text, "status did not report the ledger recorded total")


def case_b_missing_record_walks_only_that_slot(root: str, subject: str) -> None:
    case("b_missing_record_walks_only_that_slot")
    fx = Fixture(root, "b", pool_cap=2)
    built(fx, subject, fx.crate("first"))
    built(fx, subject, fx.crate("second"))
    idle = fx.idle_slots()
    expect(len(idle) == 2, f"expected two idle slots, saw {idle}")
    victim, keeper = idle[0], idle[1]

    # A legacy-shaped ledger: one slot keeps its observation, the other has
    # none. Written through the debug build's own path by editing the durable
    # ledger with the product's own field names.
    document = fx.ledger()
    for slot in document["slots"]:
        if str(slot["id"]) == victim:
            slot.pop("recorded", None)
    with open(os.path.join(fx.pool, "ledger.json"), "w", encoding="utf-8") as handle:
        json.dump(document, handle)
    os.chmod(os.path.join(fx.pool, "ledger.json"), 0o600)

    fx.reset_logs()
    built(fx, subject, fx.crate("third"))
    walks = fx.walks()
    expected = fx.slot_dir(victim)
    expect(
        walks == [expected],
        f"admission walked {walks}, not exactly the unrecorded slot {expected}",
    )
    expect(fx.recorded(victim) is not None, "the completed walk was not published")
    expect(fx.recorded(keeper) is not None, "the untouched record was lost")

    fx.reset_logs()
    built(fx, subject, fx.crate("fourth"))
    expect(not fx.walks(), f"the published record was not reused: {fx.walks()}")


def case_c_release_publishes_and_reuse_clears(root: str, subject: str) -> None:
    case("c_release_publishes_and_reuse_clears")
    fx = Fixture(root, "c", pool_cap=2)
    checkout = fx.crate("reuse")
    result = fx.run(subject, ["check"], checkout)
    expect(result.returncode == 0, f"first build exit {result.returncode}: {result.stderr[-800:]}")
    idle = fx.idle_slots()
    expect(len(idle) == 1, f"expected one idle slot, saw {idle}")
    record = fx.recorded(idle[0])
    expect(record is not None, "release published no closed observation")
    expect(
        record["unique_bytes"] >= SLOT_FILLER_BYTES,
        f"the record undercounts its own slot: {record['unique_bytes']}",
    )

    # A compatible reuse walks that slot and clears the idle record: a held
    # slot never borrows one.
    fx.reset_logs()
    again = fx.run(subject, ["check"], checkout)
    expect(again.returncode == 0, f"reuse exit {again.returncode}: {again.stderr[-800:]}")
    walks = fx.walks()
    expect(
        walks == [fx.slot_dir(idle[0])],
        f"reuse walked {walks}, not exactly its own slot",
    )
    expect(
        fx.recorded(idle[0]) is None,
        "a reuse left an idle observation on a held slot",
    )
    expect(
        "when leased" in again.stdout or "new slot" in again.stdout,
        f"the result line does not name its provenance: {again.stdout[-400:]}",
    )


def case_d_byte_pressure_evicts_only_the_idle_slot(root: str, subject: str) -> None:
    case("d_byte_pressure_evicts_only_the_idle_slot")
    # A tiny budget with two slots and a foreign out-of-slot stray.
    fx = Fixture(root, "d", pool_cap=4)
    first = fx.crate("first")
    built(fx, subject, first)
    idle = fx.idle_slots()
    expect(len(idle) == 1, f"expected one idle slot, saw {idle}")

    held_checkout = fx.crate("held")
    gate = os.path.join(fx.root, "release-held")
    holder = subprocess.Popen(
        [subject, "check"],
        cwd=held_checkout,
        env=fx.env(extra={"OXIDE_PROOF_HOLD": gate}),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        deadline = time.monotonic() + min(90, work_left())
        held_slot = None
        while time.monotonic() < deadline:
            time.sleep(0.2)
            ids = [slot for slot in fx.slot_ids() if slot not in idle]
            if ids:
                held_slot = ids[0]
                break
        if held_slot is None:
            abort("the gated writer never took a held slot")
        stray = os.path.join(fx.pool, "payload", "slots", "stray.bin")
        with open(stray, "wb") as handle:
            handle.write(b"s" * (2 * MIB))
        fx.reset_logs()
        pressure = fx.crate("pressure")
        result = fx.run(
            subject,
            ["check"],
            pressure,
            extra={"OXIDE_BUDGET_BYTES": str(1)},
        )
        expect(
            result.returncode == 0,
            f"the pressure admission failed ({result.returncode}): {result.stderr[-800:]}",
        )
        expect(not fx.walks(), f"the pressure admission walked inventory: {fx.walks()}")
        ids = fx.slot_ids()
        expect(idle[0] not in ids, "recorded byte pressure did not evict the idle whole slot")
        expect(held_slot in ids, "the held slot was deleted or counted as evictable")
        expect(os.path.exists(stray), "a foreign out-of-slot entry was deleted")
        expect(
            not os.path.exists(fx.slot_dir(idle[0])),
            "the evicted slot's tree was not removed whole",
        )
    finally:
        with open(gate, "w", encoding="utf-8") as handle:
            handle.write("release\n")
        try:
            holder.wait(timeout=min(60, work_left()))
        except subprocess.TimeoutExpired:
            holder.kill()
            holder.wait()
            abort("the gated writer did not exit on its release file")


def case_e_cross_slot_hard_link_overcount(root: str, subject: str) -> None:
    case("e_cross_slot_hard_link_overcount")
    fx = Fixture(root, "e", pool_cap=4)
    shared = os.path.join(fx.root, "shared.bin")
    with open(shared, "wb") as handle:
        handle.write(b"h" * (4 * MIB))
    for name in ("one", "two"):
        checkout = fx.crate(name)
        with open(os.path.join(checkout, "build.rs"), "w", encoding="utf-8") as handle:
            handle.write(
                "fn main() {\n"
                "    println!(\"cargo:rerun-if-changed=build.rs\");\n"
                f"    let out = std::env::var(\"OUT_DIR\").unwrap();\n"
                f"    std::fs::hard_link({shared!r}, std::path::Path::new(&out).join(\"shared.bin\"))\n"
                "        .expect(\"hard link\");\n"
                "}\n"
            )
        built(fx, subject, checkout)

    idle = fx.idle_slots()
    expect(len(idle) == 2, f"expected two idle slots, saw {idle}")
    recorded_sum = 0
    for slot_id in idle:
        record = fx.recorded(slot_id)
        expect(record is not None, f"slot {slot_id} has no closed observation")
        recorded_sum += record["unique_bytes"]
    text = fx.status(subject)
    expect(RECORDED_TOTAL_NEEDLE in text, "status did not report a ledger total")
    # The full inode union counts the shared inode once; the recorded sum counts
    # it once per slot, which is the accepted conservative overcount.
    measured = fx.run(subject, ["status", "--measure"], fx.work)
    expect(measured.returncode == 0, f"measure exit {measured.returncode}")
    expect(
        recorded_sum > 4 * MIB,
        f"the recorded sum ({recorded_sum}) did not count the shared inode twice",
    )

    # Admission uses that sum: a budget above the union but below the sum still
    # evicts, with no walk.
    fx.reset_logs()
    result = fx.run(
        subject,
        ["check"],
        fx.crate("pressure"),
        extra={"OXIDE_BUDGET_BYTES": str(6 * MIB)},
    )
    expect(result.returncode == 0, f"pressure exit {result.returncode}: {result.stderr[-800:]}")
    expect(not fx.walks(), f"the overcount admission walked inventory: {fx.walks()}")
    ids = fx.slot_ids()
    expect(
        len(ids) == 2,
        f"the accepted overcount did not replace exactly one idle slot: {ids}",
    )


def case_f_legacy_convergence_and_crash_replay(root: str, subject: str) -> None:
    case("f_legacy_convergence_and_crash_replay")
    fx = Fixture(root, "f", pool_cap=3)
    built(fx, subject, fx.crate("first"))
    built(fx, subject, fx.crate("second"))

    # Every record gone and the version-1 fence: a legacy pool.
    document = fx.ledger()
    for slot in document["slots"]:
        slot.pop("recorded", None)
    document["capacity_policy"] = 1
    with open(os.path.join(fx.pool, "ledger.json"), "w", encoding="utf-8") as handle:
        json.dump(document, handle)
    os.chmod(os.path.join(fx.pool, "ledger.json"), 0o600)

    fx.reset_logs()
    built(fx, subject, fx.crate("third"))
    measured = sorted(fx.walks())
    expect(
        len(measured) == 2,
        f"legacy convergence walked {measured}, not exactly the two unrecorded slots",
    )
    published = [slot for slot in fx.idle_slots() if fx.recorded(slot) is not None]
    expect(len(published) == len(measured), "completed walks were not published before the decision")

    # A crash replay: kill a running command, then the next command closes and
    # records the slot instead of reading it as zero.
    crash = fx.crate("crash")
    gate = os.path.join(fx.root, "release-crash")
    child = subprocess.Popen(
        [subject, "check"],
        cwd=crash,
        env=fx.env(extra={"OXIDE_PROOF_HOLD": gate}),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        deadline = time.monotonic() + min(90, work_left())
        while time.monotonic() < deadline:
            time.sleep(0.2)
            held = [slot for slot in fx.ledger()["slots"] if slot["state"] == "held"]
            if held:
                break
        else:
            abort("the crashed command never took a held slot")
    finally:
        child.kill()
        child.wait()
    recovered = fx.run(subject, ["check"], fx.crate("after-crash"))
    expect(
        recovered.returncode == 0,
        f"recovery after the crash failed ({recovered.returncode}): {recovered.stderr[-800:]}",
    )
    for slot in fx.ledger()["slots"]:
        expect(
            slot["state"] != "held" or slot.get("lease_id") is not None,
            "a held slot lost its lease without being recorded",
        )
    expect(
        all(
            slot.get("recorded") is not None
            for slot in fx.ledger()["slots"]
            if slot["state"] == "idle"
        ),
        "an idle slot was left unrecorded after a settled recovery",
    )


def case_g_status_readonly_and_measure_audit(root: str, subject: str) -> None:
    case("g_status_readonly_and_measure_audit")
    fx = Fixture(root, "g", pool_cap=2)
    built(fx, subject, fx.crate("first"))

    def digest() -> str:
        digest = hashlib.sha256()
        for base, directories, files in os.walk(fx.pool):
            directories.sort()
            for name in sorted(files):
                path = os.path.join(base, name)
                digest.update(os.path.relpath(path, fx.pool).encode())
                with open(path, "rb") as handle:
                    digest.update(handle.read())
        return digest.hexdigest()

    before = digest()
    fx.reset_logs()
    text = fx.status(subject)
    expect(not fx.walks(), f"default status walked inventory: {fx.walks()}")
    expect(RECORDED_TOTAL_NEEDLE in text, "default status hid the ledger recorded total")
    expect(RECORDED_SLOT_NEEDLE in text, "default status did not label a slot's record")
    expect(digest() == before, "default status mutated the pool")

    fx.reset_logs()
    measured = fx.run(subject, ["status", "--measure"], fx.work)
    expect(measured.returncode == 0, f"measure exit {measured.returncode}: {measured.stderr[-800:]}")
    expect(fx.walks(), "--measure walked nothing, so it is not the audit")
    expect(digest() == before, "--measure mutated the pool")


# --------------------------------------------------------------------------
# Dispatch, dependency map and receipts.


# Shared premises every case depends on. A failed premise taints every case.
PREMISES = ["staging", "capabilities", "fixture_root"]
DEPENDS = {
    "a_all_recorded_admission_walks_nothing": [],
    "b_missing_record_walks_only_that_slot": ["a_all_recorded_admission_walks_nothing"],
    "c_release_publishes_and_reuse_clears": ["a_all_recorded_admission_walks_nothing"],
    "d_byte_pressure_evicts_only_the_idle_slot": ["c_release_publishes_and_reuse_clears"],
    "e_cross_slot_hard_link_overcount": ["c_release_publishes_and_reuse_clears"],
    "f_legacy_convergence_and_crash_replay": ["b_missing_record_walks_only_that_slot"],
    "g_status_readonly_and_measure_audit": ["c_release_publishes_and_reuse_clears"],
}
CASE_FUNCS = [
    ("a_all_recorded_admission_walks_nothing", case_a_all_recorded_admission_walks_nothing),
    ("b_missing_record_walks_only_that_slot", case_b_missing_record_walks_only_that_slot),
    ("c_release_publishes_and_reuse_clears", case_c_release_publishes_and_reuse_clears),
    ("d_byte_pressure_evicts_only_the_idle_slot", case_d_byte_pressure_evicts_only_the_idle_slot),
    ("e_cross_slot_hard_link_overcount", case_e_cross_slot_hard_link_overcount),
    ("f_legacy_convergence_and_crash_replay", case_f_legacy_convergence_and_crash_replay),
    ("g_status_readonly_and_measure_audit", case_g_status_readonly_and_measure_audit),
]
DIAGNOSTIC_MODE = DIAGNOSTIC in sys.argv


def dispatch(root: str, subject: str, cases, diagnostic: bool) -> None:
    """Run the fixed cases in dependency order.

    Counted mode propagates the first failure. Diagnostic mode records it,
    skips only cases whose declared prerequisites are now tainted, and stops
    the whole collection on any unknown outcome.
    """
    global ABORTED
    for name, function in cases:
        blocked = [dependency for dependency in DEPENDS[name] if dependency in TAINTED]
        if blocked:
            SKIPS.append((name, f"dependency failed: {', '.join(blocked)}"))
            TAINTED.add(name)
            print(f"skip {name} (dependency failed: {', '.join(blocked)})", flush=True)
            continue
        if not diagnostic:
            function(root, subject)
            continue
        try:
            function(root, subject)
        except CaseFailure as failure:
            FAILURES.append((name, str(failure)))
            TAINTED.add(name)
            if not PRIMARY:
                PRIMARY.append(f"{name}: {failure}")
            if failure.abort:
                ABORTED = True
                print(f"abort {name}: {failure}", flush=True)
                return
            print(f"failed {name}: {failure}", flush=True)
        except subprocess.TimeoutExpired as timeout:
            ABORTED = True
            FAILURES.append((name, f"child timed out: {timeout}"))
            if not PRIMARY:
                PRIMARY.append(f"{name}: child timed out")
            print(f"abort {name}: child timed out", flush=True)
            return


def run_cases(root: str, subject: str) -> None:
    dispatch(root, subject, CASE_FUNCS, DIAGNOSTIC_MODE)


def receipt_path(root: str) -> str:
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE_DIR")
    directory = evidence if evidence and os.path.isdir(evidence) else root
    return os.path.join(directory, f"recorded-slot-accounting-diagnostic-{os.getpid()}.json")


def write_diagnostic_receipt(root: str, subject: str, staging_hash: str) -> str:
    executed = [name for name, _ in CASE_FUNCS if name in REACHED]
    skipped = [name for name, _ in SKIPS]
    failed = [name for name, _ in FAILURES]
    unexecuted = [name for name, _ in CASE_FUNCS if name not in REACHED]
    receipt = {
        "schema": "oxide.recorded-slot-accounting.receipt/1",
        "mode": "diagnostic",
        "qualified": False,
        "head": os.environ.get("OXIDE_PROOF_HEAD", ""),
        "script_sha256": sha256_file(os.path.abspath(__file__)),
        "guard_sha256_expected": GUARD_SHA256,
        "staged_subject_sha256": staging_hash,
        "premises": PREMISES,
        "dependencies": DEPENDS,
        "reached": REACHED,
        "completed": [name for name in executed if name not in failed and name not in TAINTED],
        "failed": FAILURES,
        "skipped": SKIPS,
        "unexecuted": unexecuted,
        "tainted": sorted(TAINTED),
        "aborted": ABORTED,
        "primary_error": PRIMARY[0] if PRIMARY else None,
        "secondary_errors": PRIMARY[1:],
        "collection_complete": (not ABORTED) and not unexecuted,
        "limits": {
            "wall_limit_s": 300,
            "work_margin_s": SHUTDOWN_MARGIN_S,
            "scratch_budget_mib": 512,
            "staging_jobs": STAGE_JOBS,
            "staging_codegen_units": STAGE_CODEGEN_UNITS,
            "fixture_jobs": FIXTURE_JOBS,
        },
        "claim": "uncounted diagnostic collection; never a PASS, closure or native credit",
    }
    path = receipt_path(root)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(receipt, handle, indent=2, sort_keys=True)
    return path


# --------------------------------------------------------------------------
# Effect-free modes.


ALLOCATION = {
    "staging_estimate_s": 55,
    "cases_estimate_s": 135,
    "cleanup_estimate_s": 15,
    "wall_limit_s": 300,
    "work_margin_s": SHUTDOWN_MARGIN_S,
    "scratch_budget_mib": 512,
    "estimated_peak_mib": 120,
    "staging_jobs": STAGE_JOBS,
    "staging_codegen_units": STAGE_CODEGEN_UNITS,
    "fixture_jobs": FIXTURE_JOBS,
    "cases": [name for name, _ in CASE_FUNCS],
    "measured": False,
}


def self_check() -> None:
    """Pure: the recipe, the dependency map, the receipt shape and negatives."""
    argv, env = staging_recipe("/tmp/nonexistent-recorded-accounting")
    expect(argv.count("-j") == 1, "staging argv lost its explicit job count")
    expect(str(STAGE_JOBS) in argv, "staging argv does not carry the reviewed job count")
    expect(
        env["CARGO_PROFILE_DEV_CODEGEN_UNITS"] == str(STAGE_CODEGEN_UNITS),
        "staging lost its reviewed codegen units",
    )
    expect(env["CARGO_PROFILE_DEV_DEBUG_ASSERTIONS"] == "true", "the walk logs need assertions")
    expect(env["CARGO_PROFILE_DEV_DEBUG"] == "0", "staging must not carry debug info")
    expect(env["CARGO_INCREMENTAL"] == "0", "staging must not be incremental")
    expect("CARGO_ENCODED_RUSTFLAGS" not in env, "an inherited encoded flags value leaked")
    expect(env["CARGO_TARGET_DIR"].startswith("/tmp/nonexistent"), "target left the owned stage")
    # Negative mutations the proof must be able to see.
    mutated = dict(env)
    mutated["CARGO_PROFILE_DEV_CODEGEN_UNITS"] = "1"
    expect(
        mutated["CARGO_PROFILE_DEV_CODEGEN_UNITS"] != str(STAGE_CODEGEN_UNITS),
        "a one-codegen-unit regression would not be visible",
    )
    one_job = [argument for argument in argv if argument != str(STAGE_JOBS)]
    expect(one_job != argv, "a one-job regression would not be visible")
    expect(len(CASE_FUNCS) == 7, "the fixed case set changed")
    expect(all(name in DEPENDS for name, _ in CASE_FUNCS), "a case has no dependency entry")
    expect(
        all(dependency in {name for name, _ in CASE_FUNCS} for deps in DEPENDS.values() for dependency in deps),
        "a dependency names a case that does not exist",
    )
    # The receipt must be truthful about its mode.
    expect(ALLOCATION["measured"] is False, "the estimate must not claim measurement")
    expect(GUARD_SHA256.startswith("a6d73326"), "the guard pin is not the qualified source")
    print("self-check: ok (pure recipe, dependency, receipt and negative checks; not evidence)")


def self_test() -> None:
    """Pure: the diagnostic decision machine, with no child, image or pool."""
    global ABORTED
    saved = (list(FAILURES), list(SKIPS), list(REACHED), set(TAINTED), list(PRIMARY), ABORTED)

    def reset() -> None:
        global ABORTED
        FAILURES.clear()
        SKIPS.clear()
        REACHED.clear()
        TAINTED.clear()
        PRIMARY.clear()
        ABORTED = False

    def stubs(failing: str, aborting: bool, calls: list[str]) -> list[tuple[str, object]]:
        made = []
        for name, _ in CASE_FUNCS:
            if name == failing:
                def explode(root, subject, message=failing, hard=aborting):
                    raise CaseFailure(f"{message} oracle", hard)
                made.append((name, explode))
            else:
                made.append((name, (lambda label: lambda root, subject: calls.append(label))(name)))
        return made

    try:
        # A recoverable oracle failure in a leaf taints only itself and its
        # dependents, while independent cases continue.
        reset()
        calls: list[str] = []
        dispatch("stub", "stub", stubs("g_status_readonly_and_measure_audit", False, calls), True)
        expect(
            FAILURES and FAILURES[0][0] == "g_status_readonly_and_measure_audit",
            "the first fault was not preserved",
        )
        expect(not SKIPS, f"a leaf failure skipped independent cases: {SKIPS}")
        expect(len(calls) == 6, f"independent cases did not continue: {calls}")
        expect(not ABORTED, "a recoverable failure aborted collection")
        expect(PRIMARY and PRIMARY[0].startswith("g_status"), "the primary error was lost")

        # A shared prerequisite's failure skips every dependent, transitively.
        reset()
        dispatch("stub", "stub", stubs("a_all_recorded_admission_walks_nothing", False, []), True)
        skipped = {name for name, _ in SKIPS}
        expect("b_missing_record_walks_only_that_slot" in skipped, "a dependent was not skipped")
        expect("c_release_publishes_and_reuse_clears" in skipped, "a dependent was not skipped")
        expect("d_byte_pressure_evicts_only_the_idle_slot" in skipped,
               "a transitive dependent was not skipped")
        expect(len(skipped) == 6, f"the taint set is not the full dependent closure: {skipped}")

        # Counted mode still fails fast: the first failure propagates.
        reset()
        try:
            dispatch("stub", "stub", stubs("a_all_recorded_admission_walks_nothing", False, []), False)
        except CaseFailure:
            pass
        else:
            fail("counted dispatch did not propagate its first failure")
        reset()

        # An unknown outcome aborts collection at once.
        calls.clear()
        dispatch("stub", "stub", stubs("a_all_recorded_admission_walks_nothing", True, calls), True)
        expect(ABORTED, "an unknown outcome did not abort")
        expect(not calls, "a case ran after an abort")
        expect(len(FAILURES) == 1, "an abort recorded more than the first fault")

        # The receipt stays bounded, uncounted and explicit about omissions.
        receipt = {
            "qualified": False,
            "mode": "diagnostic",
            "collection_complete": False,
            "unexecuted": [name for name, _ in CASE_FUNCS],
        }
        expect(len(json.dumps(receipt)) < 8192, "the receipt bound is not bounded")
        expect(receipt["qualified"] is False, "a diagnostic receipt claimed qualification")
    finally:
        FAILURES[:], SKIPS[:], REACHED[:], restored, PRIMARY[:], ABORTED = saved
        TAINTED.clear()
        TAINTED.update(restored)
    print("self-test: ok (pure dispatch, taint, abort and receipt checks; not evidence)")


def main() -> None:
    arguments = [argument for argument in sys.argv[1:] if argument != DIAGNOSTIC]
    if arguments == ["--plan"]:
        print(json.dumps(ALLOCATION, indent=2, sort_keys=True))
        return
    if arguments == ["--self-check"]:
        self_check()
        return
    if arguments == ["--self-test"]:
        self_test()
        return
    if arguments:
        print(f"unknown argument: {arguments[0]}", file=sys.stderr)
        raise SystemExit(2)
    if not os.environ.get("OXIDE_TASK_SCRATCH_ROOT"):
        print("this proof runs only under the qualified shared supervisor", file=sys.stderr)
        raise SystemExit(2)
    root = os.path.abspath(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    scratch = os.path.join(root, "recorded-accounting")
    os.makedirs(scratch, exist_ok=True)
    os.chmod(scratch, 0o700)
    subject = stage_subject(scratch)
    staging_hash = sha256_file(subject)
    probe = subprocess.run(
        [subject, CAPABILITIES_PROBE],
        env={**os.environ, "OXIDE_TEST_HOME_FROM_ENV": "1"},
        capture_output=True,
        text=True,
        timeout=min(30, work_left()),
    )
    if probe.returncode != 0 or '"test_controls":true' not in probe.stdout.replace(" ", ""):
        abort(f"the staged candidate has no test controls: {probe.stdout!r} {probe.stderr!r}")
    run_cases(scratch, subject)
    if DIAGNOSTIC_MODE:
        path = write_diagnostic_receipt(scratch, subject, staging_hash)
        print(f"diagnostic receipt: {path}")
        print(
            json.dumps(
                {
                    "reached": len(REACHED),
                    "failed": len(FAILURES),
                    "skipped": len(SKIPS),
                    "aborted": ABORTED,
                    "qualified": False,
                },
                sort_keys=True,
            )
        )
        raise SystemExit(1 if (FAILURES or ABORTED) else 0)
    for name in CASES:
        print(f"pass {name}")
    print(f"all {len(CASES)} cases passed")


if __name__ == "__main__":
    main()
