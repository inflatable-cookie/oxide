"""Focused account-home proof for oxide#143.

One fixed script and the whole command under the qualified shared proof
supervisor (`bounded-test.py --limit 300 --scratch-budget-mib 512`): 300 s
including staging, fixtures, one owned scratch image and cleanup, 512 MiB
monitored scratch. `--plan` prints the phases, budgets and limits and does
nothing else; any other argument refuses before a subprocess.

It stages this clean HEAD's `oxide` twice, sequentially, with plain Cargo,
1.97.1, offline and a fixed eight jobs, each into its own scratch target that
is deleted once the binary is copied out. Every Cargo call, staging and
fixture, runs one at a time with the same eight jobs. Phase time and disk
figures in `--plan` are planning allocations, not measurements; the receipt
records the actual figures beside them:

- the controlled artifact: the dev profile with debug assertions, so the
  proof controls are compiled in (`test_controls: true`);
- the normal artifact: the release profile (debug assertions off, as the
  installer builds it) at opt-level 1 to stay inside the bound; the
  capability depends only on debug assertions.

The installer's own capability check (`scripts/oxide-install.py`,
imported, effect-free) refuses the controlled artifact and accepts the normal
one. The normal artifact refuses either control, whatever its value, before
any configuration is read; its probe is `pool set-mode` with explicit,
absent scratch `OXIDE_POOL` and `OXIDE_CONFIG`, which reads no account root
even if the refusal regressed.

The controlled artifact then runs only with a private `HOME` under
`OXIDE_TEST_HOME_FROM_ENV=1` or a stand-in account home under
`OXIDE_TEST_ACCOUNT_HOME`, a canonical directory inside this run's scratch:
never the operator's account, config, state, pool or Builds drive. Cases:
the account default under a differing `HOME` (config, state and default pool
from the account, the `home:` line, one notice, a tiny offline check creating
the account pool, `pools.json` in the account state root, the harness `HOME`
left empty); one owned 128 MiB scratch APFS image at a unique `/Volumes`
name, owners enabled, registered as the removable pool of the
account-selected pool and selected by real builds under a differing and a
matching `HOME`; `OXIDE_POOL` isolating a build; ordinary detach; a planted
account config choosing pool and budget without a notice; lazy needed facts
with an unavailable account home; and the malformed, mixed, unsupervised,
symlinked, outside and relative control refusals.

A uniquely named helper receipt in the evidence directory records sources,
artifacts, cases, phases, resources and image custody; it never claims
closure, which is the supervisor's. Not a benchmark, real-drive or
real-account qualification, or full QA.
"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import hashlib
import importlib.util
import json
import os
import plistlib
import resource
import secrets
import shutil
import signal
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)


def load(name: str, file: str):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, file))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BUILDER = "1.97.1"
JOBS = "8"
MEMORY_GIB = 6
LIMIT_S = 300
WORK_SECONDS = 270
# (phase, allocated seconds, allocated peak owned MiB at that phase). Owned
# bytes count the supervisor scratch: staged trees, copied binaries, the
# sparse image backing file, fixtures and evidence.
ALLOCATIONS = (
    ("stage-controlled", 60, 200),
    ("stage-normal", 90, 260),
    ("normal-refusal", 5, 60),
    ("controls", 10, 60),
    ("account-default", 15, 80),
    ("removable", 45, 230),
    ("account-config", 3, 230),
    ("unavailable", 5, 230),
)
SCRATCH_MIB = 512
STATE_VERSION = 3
REGISTRATION_VERSION = 2
PROBE = "__oxide-build-capabilities"
HOME_FROM_ENV = "OXIDE_TEST_HOME_FROM_ENV"
ACCOUNT_HOME = "OXIDE_TEST_ACCOUNT_HOME"
NO_CONTROLS = "is set, but this build has no proof controls"
APP = {
    "Cargo.toml": '[package]\nname = "account_app"\nversion = "0.1.0"\nedition = "2021"\n',
    "src/lib.rs": "pub fn value() -> u32 {\n    7\n}\n",
}
PLAN = f"""account-home proof plan (oxide#143)
command: python3 scripts/bounded-test.py --limit {LIMIT_S} --scratch-budget-mib {SCRATCH_MIB} -- python3 scripts/test-account-home.py
limits: {LIMIT_S} s including staging, fixtures, detach and cleanup; work stops 30 s before the supervisor deadline; {SCRATCH_MIB} MiB monitored scratch
cargo: plain Cargo {BUILDER}, --offline --locked, -j {JOBS} for staging and CARGO_BUILD_JOBS={JOBS} for fixture builds; one Cargo call at a time; RUSTC_WRAPPER empty
demand: {JOBS} CPUs and {MEMORY_GIB} GiB planning allocation (unmeasured; the receipt records the children's peak RSS and CPU time)
staging: sequential, each target deleted after its verified copy
  1 controlled: dev profile, debug assertions on, debuginfo off, symbols stripped (rust:everyday-cli measured ~82 MiB for two debug stages from one target; not a fit for this one)
  2 normal: release profile, debug assertions off, debuginfo off, opt-level 1, symbols stripped (unmeasured)
allocations (planning, unmeasured; {WORK_SECONDS} s work window before the supervisor's 30 s margin):
{"".join(f"  {name:<17} {seconds:>3} s  peak owned {mib:>3} MiB" + chr(10) for name, seconds, mib in ALLOCATIONS)}  total             {sum(a[1] for a in ALLOCATIONS):>3} s  peak owned {max(a[2] for a in ALLOCATIONS):>3} MiB of {SCRATCH_MIB}
  peak parts: normal staging tree <=200 MiB plus the controlled copy; image phases hold the 128 MiB sparse backing file at most, two copied binaries <=40 MiB, fixtures <=30 MiB, evidence <=1 MiB
phases:
  stage-controlled  capability {{control_state_version {STATE_VERSION}, registration_version {REGISTRATION_VERSION}, test_controls true}}; installer capability check refuses it
  stage-normal      capability test_controls false; installer capability check accepts it; binaries differ
  normal-refusal    4 control requests x `pool set-mode ephemeral` with absent scratch OXIDE_POOL/OXIDE_CONFIG: refused, no stdout, nothing created
  controls          HOME_FROM_ENV=1 private HOME; malformed and mixed refusals before config even with explicit pool and config; unsupervised, symlinked, outside, relative stand-ins refused
  account-default   status from the stand-in under a differing HOME; one tiny offline check creates the account default pool; pools.json in the account state root; harness HOME empty
  removable         one owned 128 MiB sparse APFS image at a unique /Volumes name, owners enabled, registered (reserve 16 MiB, cap 64 MiB) under the account pool; builds select it under a differing and a matching HOME; OXIDE_POOL isolates; ordinary detach; status after detach
  account-config    planted config.toml chooses pool and budget with no notice
  unavailable       !unavailable refuses where a home is needed and succeeds with explicit pool and config
fixtures: private CARGO_HOME, stand-in account home and harness HOME inside the supervisor scratch; nothing outside it except the one image mount
evidence: one uniquely named helper receipt beside the supervisor's own; closure and cleanup are the supervisor's
"""

STARTED = time.monotonic()
CASES: list = []
PHASES: list = []


def fail(message: str) -> None:
    raise RuntimeError(message)


def expect(condition: bool, message: str) -> None:
    if not condition:
        fail(message)


def case(name: str, condition: bool, detail: str = "") -> None:
    CASES.append({"case": name, "passed": bool(condition)})
    print(("PASS " if condition else "FAIL ") + name, flush=True)
    expect(condition, f"{name}: {detail[-3000:]}")


def sha256(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def tree(root: str) -> list:
    found = []
    for directory, dirs, files in os.walk(root):
        for name in dirs + files:
            found.append(os.path.relpath(os.path.join(directory, name), root))
    return sorted(found)


def write(path: str, text: str) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)


def line_with(text: str, prefix: str) -> str:
    return next((line for line in text.splitlines() if line.startswith(prefix)), "")


def summary(stderr: str) -> str:
    lines = [line for line in stderr.splitlines() if " profile, " in line and " strategy, slot " in line]
    expect(len(lines) == 1, f"expected one summary line: {stderr[-3000:]}")
    return lines[0]


class Session:
    def __init__(self, layout, root: str):
        self.layout = layout
        self.proof = layout.Proof(root)
        self.root = root

    def phase(self, label: str) -> None:
        self.proof.phase(label)
        PHASES.append({
            "phase": label,
            "seconds": round(time.monotonic() - STARTED, 1),
            "peak_owned_bytes": self.proof.peak,
            "host_free_min_bytes": self.proof.low,
        })

    def run(self, command: list, cwd: str, env: dict, timeout: float = 120) -> tuple:
        result = self.proof.run(command, cwd=cwd, env=env, timeout=timeout)
        out = result.stdout.decode(errors="replace")
        err = result.stderr.decode(errors="replace")
        print(f"----- {' '.join(command[1:3])} (exit {result.returncode}) -----\n{out}{err}", flush=True)
        return result.returncode, out, err


def stage(session: Session, label: str, release: bool) -> str:
    """Build this HEAD's `oxide` into a fresh owned target, copy the binary
    out and delete the target before the next stage."""
    root = os.path.join(session.root, f"stage-{label}")
    os.makedirs(root)
    profile = "RELEASE" if release else "DEV"
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("CARGO_TARGET_", "CARGO_BUILD_", "CARGO_PROFILE_", "SCCACHE_", "OXIDE_TEST_"))
        and key not in ("CARGO", "RUSTC", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "OXIDE_POOL", "OXIDE_CONFIG")
    }
    env.update({
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "RUSTC_WRAPPER": "",
        "RUSTC_WORKSPACE_WRAPPER": "",
        "CARGO_BUILD_RUSTC_WRAPPER": "",
        "CARGO_TARGET_DIR": os.path.join(root, "target"),
        "CARGO_BUILD_BUILD_DIR": os.path.join(root, "build"),
        f"CARGO_PROFILE_{profile}_DEBUG": "0",
        "CARGO_INCREMENTAL": "0",
        "RUSTFLAGS": "-C strip=symbols",
    })
    if release:
        env["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = "1"
    command = ["cargo", "build", "-j", JOBS, "--locked", "--offline", "--bin", "oxide"]
    if release:
        command.append("--release")
    code, _, err = session.run(command, REPO, env, timeout=150)
    expect(code == 0, f"{label} staging exit {code}: {err[-2000:]}")
    built = os.path.join(root, "target", "release" if release else "debug", "oxide")
    expect(os.path.isfile(built), f"{label} staging produced no binary at {built}")
    copy = os.path.join(session.root, "bin", f"oxide-{label}")
    os.makedirs(os.path.dirname(copy), exist_ok=True)
    shutil.copy2(built, copy)
    expect(sha256(copy) == sha256(built), f"{label} copy differs from the built binary")
    shutil.rmtree(root)
    return copy


def capability(path: str) -> dict:
    probe = subprocess.run([path, PROBE], capture_output=True, text=True, timeout=10)
    expect(probe.returncode == 0, f"capability probe exit {probe.returncode}: {probe.stderr!r}")
    return json.loads(probe.stdout)


def fixture_env(root: str) -> dict:
    """The inherited environment without any storage selector or control,
    plus private Cargo state and the supervisor context the account control
    needs."""
    invoking_home = os.environ.get("HOME", "")
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("SCCACHE_", "OXIDE_", "XDG_", "CARGO_BUILD_", "CARGO_TARGET_", "CARGO_PROFILE_", "GIT_"))
        and key not in ("CARGO", "RUSTC", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "HOME")
    }
    scratch = os.environ["OXIDE_TASK_SCRATCH_ROOT"]
    cargo_home = os.path.join(root, "cargo-home")
    os.makedirs(cargo_home, mode=0o700)
    env.update({
        "OXIDE_TASK_SCRATCH_ROOT": scratch,
        "OXIDE_PROOF_EVIDENCE": os.environ["OXIDE_PROOF_EVIDENCE"],
        "OXIDE_PROOF_DEADLINE_NS": os.environ["OXIDE_PROOF_DEADLINE_NS"],
        "TMPDIR": scratch,
        "TMP": scratch,
        "TEMP": scratch,
        "CARGO_HOME": cargo_home,
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME") or os.path.join(invoking_home, ".rustup"),
        "RUSTUP_TOOLCHAIN": BUILDER,
        "RUSTUP_AUTO_INSTALL": "0",
        "CARGO_NET_OFFLINE": "true",
        "CARGO_TERM_COLOR": "never",
        "CARGO_BUILD_JOBS": JOBS,
        "RUSTC_WRAPPER": "",
        "RUSTC_WORKSPACE_WRAPPER": "",
        "CARGO_BUILD_RUSTC_WRAPPER": "",
        "GIT_CONFIG_NOSYSTEM": "1",
    })
    return env


def normal_refusal(session: Session, normal: str, base: dict, stand_in: str) -> None:
    root = session.root
    home = os.path.join(root, "normal-home")
    os.makedirs(home, mode=0o700)
    pool = os.path.join(root, "normal-pool-absent")
    config = os.path.join(root, "normal-config-absent.toml")
    for name, value in ((HOME_FROM_ENV, "1"), (HOME_FROM_ENV, "0"), (ACCOUNT_HOME, stand_in), (ACCOUNT_HOME, "!unavailable")):
        env = {**base, "HOME": home, "OXIDE_POOL": pool, "OXIDE_CONFIG": config, name: value}
        code, out, err = session.run([normal, "pool", "set-mode", "ephemeral"], root, env, timeout=30)
        case(f"the normal artifact refuses {name}={value!r} before configuration", code != 0
             and f"{name} {NO_CONTROLS}" in err and not out, out + err)
    case("the normal artifact's refusals created nothing",
         not os.path.lexists(pool) and not os.path.lexists(config) and tree(home) == [] and tree(stand_in) == [])


def controls(session: Session, controlled: str, base: dict, stand_in: str) -> None:
    root = session.root
    private = os.path.join(root, "private-home")
    os.makedirs(private, mode=0o700)
    env = {**base, "HOME": private, HOME_FROM_ENV: "1"}
    code, out, err = session.run([controlled, "status"], root, env, timeout=60)
    case(f"{HOME_FROM_ENV}=1 restores a private HOME", code == 0
         and f"pool: {private}/Library/Caches/oxide/build-pool (from built-in default)" in out
         and line_with(out, "  home: ") == f"  home: HOME {private} (proof control {HOME_FROM_ENV})"
         and "oxide: HOME is" not in err, out + err)
    explicit = {**base, "HOME": private, "OXIDE_POOL": os.path.join(root, "explicit-pool"),
                "OXIDE_CONFIG": os.path.join(root, "explicit-config.toml")}
    for label, extra, refusal in (
        ("HOME control value 0", {HOME_FROM_ENV: "0"}, "must be exactly 1"),
        ("HOME control value true", {HOME_FROM_ENV: "true"}, "must be exactly 1"),
        ("mixed controls", {HOME_FROM_ENV: "1", ACCOUNT_HOME: stand_in}, "are both set"),
    ):
        code, out, err = session.run([controlled, "status"], root, {**explicit, **extra}, timeout=60)
        case(f"{label} refuses before config even with explicit pool and config",
             code != 0 and refusal in err and not out, out + err)
    unsupervised = {k: v for k, v in base.items() if k != "OXIDE_PROOF_EVIDENCE"}
    for value in (stand_in, "!unavailable"):
        code, out, err = session.run([controlled, "status"], root, {**unsupervised, "HOME": private, ACCOUNT_HOME: value}, timeout=60)
        case(f"{ACCOUNT_HOME}={'stand-in' if value == stand_in else value} refuses outside the supervisor context",
             code != 0 and "OXIDE_PROOF_EVIDENCE" in err and not out, out + err)
    link = os.path.join(root, "account-link")
    os.symlink(stand_in, link)
    for label, value, why in (
        ("a symlinked stand-in", link, "not canonical"),
        ("a stand-in outside scratch", "/Users", "not strictly inside"),
        ("a relative stand-in", "account-home", "not an absolute path"),
    ):
        code, out, err = session.run([controlled, "status"], root, {**base, "HOME": private, ACCOUNT_HOME: value}, timeout=60)
        case(f"{label} refuses", code != 0 and why in err and not out, out + err)
    case("the control refusals created nothing in the stand-in", tree(stand_in) == [])


def account_default(session: Session, controlled: str, account_env: dict, account: str, harness: str) -> tuple:
    root = session.root
    config_dir = os.path.join(account, "Library", "Application Support", "oxide", "config")
    state = os.path.join(account, "Library", "Application Support", "oxide", "state", "pools.json")
    pool = os.path.join(account, "Library", "Caches", "oxide", "build-pool")
    notice = f"oxide: HOME is {harness}; using the account pool at {pool} (account home {account}); set OXIDE_POOL to isolate"
    code, out, err = session.run([controlled, "status"], root, account_env, timeout=60)
    case("status reads config and the default pool from the account home", code == 0
         and f"config: {config_dir}/config.toml (platform location, absent)" in out
         and f"pool: {pool} (from built-in default)" in out, out + err)
    case("status names the account-home source and the differing HOME",
         line_with(out, "  home: ") == f"  home: account home {account} (proof control {ACCOUNT_HOME}); HOME is {harness}", out)
    case("status prints the account-pool notice once", err.count("oxide: HOME is") == 1 and notice in err, err)
    app = os.path.join(root, "account-app")
    for relative, text in APP.items():
        write(os.path.join(app, relative), text)
    code, out, err = session.run([controlled, "check", "--offline"], app, account_env, timeout=120)
    case("a tiny offline check under a differing HOME builds in the account default pool", code == 0
         and "directory-budget-v1" in summary(err) and os.path.isdir(os.path.join(pool, "oxide-managed")), err)
    case("the build printed the account-pool notice once", err.count("oxide: HOME is") == 1 and notice in err, err)
    recorded = json.load(open(state, encoding="utf-8")) if os.path.isfile(state) else None
    case("pools.json in the account state root records the account pool", recorded == [pool], repr(recorded))
    case("nothing was created under the harness HOME", tree(harness) == [], repr(tree(harness)))
    return app, pool


def removable(session: Session, controlled: str, account_env: dict, account: str, harness: str, app: str, pool: str) -> dict:
    root = session.root
    name = "OxAh" + secrets.token_hex(4)
    mount = f"/Volumes/{name}"
    expect(not os.path.lexists(mount), f"{mount} exists before the proof; never touched")
    image = os.path.join(root, "volume.sparseimage")
    custody = {"image": image, "mount": mount, "attached": False, "detached": False}
    code, _, err = session.run(["/usr/bin/hdiutil", "create", "-size", "128m", "-fs", "APFS", "-volname", name,
                                "-type", "SPARSE", image], root, os.environ.copy(), timeout=60)
    expect(code == 0, f"image creation failed: {err}")
    result = session.proof.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-plist", image], cwd=root, env=os.environ.copy(), timeout=60)
    expect(result.returncode == 0, f"attach failed; attachment unresolved, evidence kept: {result.stderr!r}")
    mounts = [entity["mount-point"] for entity in plistlib.loads(result.stdout)["system-entities"] if entity.get("mount-point")]
    expect(mounts == [mount], f"scratch volume mounted at {mounts}, not {mount}")
    session.proof.mount = mount
    custody["attached"] = True
    info = session.proof.run(["/usr/sbin/diskutil", "info", "-plist", mount], cwd=root, env=os.environ.copy(), timeout=30)
    expect(info.returncode == 0, f"diskutil info failed: {info.stderr!r}")
    case("the owned image has owners enabled", plistlib.loads(info.stdout).get("GlobalPermissionsEnabled") is True)
    session.phase("image attached")

    code, out, err = session.run([controlled, "pool", "register-removable", mount, "--reserve", "16MiB", "--cap", "64MiB"],
                                 app, account_env, timeout=60)
    registration = os.path.join(pool, "oxide-managed", "removable.json")
    case("registration under a differing HOME lands in the account-selected pool", code == 0
         and "registered removable pool" in out and os.path.isfile(registration), out + err)
    for label, home in (("a differing", harness), ("a matching", account)):
        code, out, err = session.run([controlled, "check", "--offline"], app, {**account_env, "HOME": home}, timeout=120)
        case(f"a build under {label} HOME selects the registered removable pool", code == 0
             and "apfs-volume-v1" in summary(err), err)
    case("the matching HOME build printed no notice", "oxide: HOME is" not in err, err)
    code, out, err = session.run([controlled, "status"], app, account_env, timeout=60)
    case("status under a differing HOME records the removable pool as selected", code == 0
         and line_with(out, "removable pool: ").startswith(f"removable pool: {mount} volume ")
         and line_with(out, "recorded selection: ").startswith(f"recorded selection: removable pool (volume present at {mount};"),
         out + err)
    isolated = os.path.join(root, "isolated-pool")
    code, out, err = session.run([controlled, "check", "--offline"], app, {**account_env, "OXIDE_POOL": isolated}, timeout=120)
    case("OXIDE_POOL isolates a build from the account pool and its drive", code == 0
         and "directory-budget-v1" in summary(err) and "oxide: HOME is" not in err
         and os.path.isdir(os.path.join(isolated, "oxide-managed")), err)
    session.phase("removable selected")

    session.proof.detach()
    custody["detached"] = not os.path.ismount(mount)
    case("ordinary detach of the owned image", custody["detached"])
    code, out, err = session.run([controlled, "status"], app, account_env, timeout=60)
    case("after detach the account registration remains and the internal pool is next", code == 0
         and line_with(out, "removable pool: ").startswith(f"removable pool: {mount} volume ")
         and line_with(out, "recorded selection: ").startswith(f"recorded selection: removable pool, but no volume is present at {mount};"),
         out + err)
    case("nothing was created under the harness HOME after the drive cases", tree(harness) == [], repr(tree(harness)))
    return custody


def account_config(session: Session, controlled: str, account_env: dict, account: str) -> None:
    config = os.path.join(account, "Library", "Application Support", "oxide", "config", "config.toml")
    write(config, '[pool]\npath = "~/acct-pool"\nbudget = 3000000000\n')
    code, out, err = session.run([controlled, "status"], session.root, account_env, timeout=60)
    case("a planted account config chooses pool and budget under a differing HOME, with no notice", code == 0
         and f"pool: {account}/acct-pool (from config file)" in out
         and "soft budget 3000 MB (from config file)" in out and "oxide: HOME is" not in err, out + err)
    os.unlink(config)


def unavailable(session: Session, controlled: str, base: dict, harness: str) -> None:
    root = session.root
    env = {**base, "HOME": harness, ACCOUNT_HOME: "!unavailable"}
    code, out, err = session.run([controlled, "status"], root, env, timeout=60)
    case("an unavailable account home refuses where a home is needed, never using HOME", code != 0
         and "the account home directory is unavailable (injected)" in err and harness not in out, out + err)
    pool = os.path.join(root, "explicit-unavailable-pool")
    config = os.path.join(root, "explicit-unavailable-config.toml")
    code, out, err = session.run([controlled, "status"], root, {**env, "OXIDE_POOL": pool, "OXIDE_CONFIG": config}, timeout=60)
    case("explicit pool and config still resolve with an unavailable account home", code == 0
         and f"pool: {pool} (from OXIDE_POOL)" in out
         and line_with(out, "  home: ").startswith("  home: unavailable (the account home directory is unavailable (injected))"),
         out + err)
    case("the unavailable cases created nothing under the harness HOME", tree(harness) == [], repr(tree(harness)))


def receipt(evidence: str, body: dict) -> str:
    path = os.path.join(evidence, f"account-home-receipt-{os.getpid()}-{secrets.token_hex(4)}.json")
    expect(not os.path.lexists(path), f"{path} exists")
    with open(path, "x", encoding="utf-8") as handle:
        json.dump(body, handle, indent=2, sort_keys=True)
        handle.write("\n")
    return path


def terminated(signum, frame) -> None:
    raise KeyboardInterrupt(f"signal {signum}")


def main() -> int:
    arguments = sys.argv[1:]
    if arguments == ["--plan"]:
        print(PLAN, end="")
        return 0
    if arguments:
        print(f"account-home proof: unknown arguments {arguments!r}; only --plan is accepted", file=sys.stderr)
        return 2
    expect(sys.platform == "darwin", "the account-home proof requires macOS")
    scratch = os.environ.get("OXIDE_TASK_SCRATCH_ROOT", "")
    evidence = os.environ.get("OXIDE_PROOF_EVIDENCE", "")
    expect(bool(scratch) and os.path.realpath(scratch) == scratch, f"scratch {scratch!r} is not canonical")
    expect(os.path.basename(scratch) == "scratch" and os.path.basename(os.path.dirname(scratch)).startswith("oxide-proof."),
           "OXIDE_TASK_SCRATCH_ROOT is not the supervisor's own <oxide-proof.*>/scratch")
    expect(os.path.realpath(evidence) == os.path.join(os.path.dirname(scratch), "evidence"),
           "OXIDE_PROOF_EVIDENCE is not the supervisor's sibling evidence directory")
    expect(os.environ.get("OXIDE_PROOF_DEADLINE_NS", "").isdigit(), "not under the qualified shared proof supervisor")
    for name in ("TMPDIR", "TMP", "TEMP"):
        expect(os.environ.get(name) == scratch, f"{name} differs from the supervisor scratch")
    layout = load("pool_layout", "test-oxide-pool-layout.py")
    installer = load("oxide_install", "oxide-install.py")
    head = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, timeout=30).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=REPO, capture_output=True, text=True, timeout=30).stdout.strip()
    expect(len(head) == 40 and not dirty, f"candidate checkout {head!r} is not a clean commit: {dirty}")
    signal.signal(signal.SIGTERM, terminated)
    root = tempfile.mkdtemp(prefix="account-home.", dir=scratch)
    session = Session(layout, root)
    body = {
        "helper": "scripts/test-account-home.py",
        "helper_sha256": sha256(os.path.abspath(__file__)),
        "source_head": head,
        "limits": {"seconds": LIMIT_S, "work_seconds": WORK_SECONDS, "scratch_mib": SCRATCH_MIB,
                   "cargo_jobs": int(JOBS), "planning_memory_gib": MEMORY_GIB},
        "allocations": [{"phase": name, "seconds": seconds, "peak_owned_mib": mib}
                        for name, seconds, mib in ALLOCATIONS],
        "closure": "not claimed; decided by the shared supervisor",
        "limitations": [
            "tiny fixtures; not a benchmark, real-drive or real-account qualification",
            "the normal artifact is the release profile at opt-level 1, not the installer's opt-level 3",
            "the stand-in replaces the passwd lookup; the real lookup is covered by unit tests only",
        ],
    }
    custody = None
    try:
        session.phase("start")
        controlled = stage(session, "controlled", release=False)
        controlled_caps = capability(controlled)
        case("the controlled artifact reports compiled proof controls", controlled_caps == {
            "control_state_version": STATE_VERSION, "registration_version": REGISTRATION_VERSION, "test_controls": True}, repr(controlled_caps))
        try:
            installer.state_capabilities(controlled)
            refused = None
        except installer.Refusal as refusal:
            refused = str(refusal)
        case("the installer's capability check refuses the controlled artifact", refused is not None and "proof controls" in refused, repr(refused))
        session.phase("stage-controlled")

        normal = stage(session, "normal", release=True)
        normal_caps = capability(normal)
        case("the normal artifact reports no proof controls", normal_caps == {
            "control_state_version": STATE_VERSION, "registration_version": REGISTRATION_VERSION, "test_controls": False}, repr(normal_caps))
        accepted = installer.state_capabilities(normal)
        case("the installer's capability check accepts the normal artifact", accepted == {
            "control_state_version": STATE_VERSION, "registration_version": REGISTRATION_VERSION}, repr(accepted))
        case("the two artifacts differ", sha256(controlled) != sha256(normal))
        body["artifacts"] = {
            label: {"sha256": sha256(path), "bytes": os.path.getsize(path), "capability": caps}
            for label, path, caps in (("controlled", controlled, controlled_caps), ("normal", normal, normal_caps))
        }
        session.phase("stage-normal")

        base = fixture_env(root)
        account = os.path.realpath(os.path.join(root, "account-home"))
        harness = os.path.join(root, "harness-home")
        for path in (account, harness):
            os.makedirs(path, mode=0o700)
        normal_refusal(session, normal, base, account)
        session.phase("normal-refusal")
        controls(session, controlled, base, account)
        session.phase("controls")
        account_env = {**base, "HOME": harness, ACCOUNT_HOME: account}
        app, pool = account_default(session, controlled, account_env, account, harness)
        session.phase("account-default")
        custody = removable(session, controlled, account_env, account, harness, app, pool)
        account_config(session, controlled, account_env, account)
        session.phase("account-config")
        unavailable(session, controlled, base, harness)
        session.phase("unavailable")
    except BaseException as failure:
        if session.proof.mount and os.path.ismount(session.proof.mount):
            try:
                session.proof.detach()
            except BaseException as error:
                print(f"detach after failure not completed: {error!r}", file=sys.stderr)
        body.update({"result": f"failed: {failure!r}", "cases": CASES, "phases": PHASES, "image": custody,
                     "unclosed_children": sorted(session.proof.unclosed)})
        print(f"account-home proof failed; receipt {receipt(evidence, body)}; scratch {root} left to the supervisor", file=sys.stderr)
        raise
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    body.update({
        "result": "passed",
        "cases": CASES,
        "phases": PHASES,
        "image": custody,
        "resources": {
            "script_seconds": round(time.monotonic() - STARTED, 1),
            "peak_owned_bytes_at_phase_checks": session.proof.peak,
            "host_free_min_bytes": session.proof.low,
            "children_max_rss_bytes": usage.ru_maxrss,
            "children_cpu_seconds": round(usage.ru_utime + usage.ru_stime, 1),
        },
    })
    path = receipt(evidence, body)
    print(f"account-home proof: {len(CASES)}/{len(CASES)} cases passed in {body['resources']['script_seconds']} s; "
          f"receipt {path}; closure and cleanup are the supervisor's")
    return 0


if __name__ == "__main__":
    sys.exit(main())
