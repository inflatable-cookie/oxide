#!/usr/bin/env python3
"""oxide#101 evidence apparatus, not a shipped harness.

sccache 0.18.0 client-side mode (SCCACHE_CLIENT_SIDE=1) against Oxide-style
slots, built through a stable per-index alias (`CARGO_BUILD_BUILD_DIR` is a
symlink `alias/<n>` -> `slots/<slot>`), with Oxide's quick recipe
environment. Plain Cargo 1.97.1 stands in for `oxide`; no Oxide binary,
pool, image or lease is involved.

Modes:
  (default)   safety: closure cases C1-C3 and alias cases B1-B8
  --benefit   three interleaved cold-slot pairs on a copy of this repository

Bounds (from the brief): one monotonic 300 s clock from start through
teardown, 1 GiB owned bytes, 96 Cargo launches, 8 MiB fixture inputs,
32 MiB logs, 20 GiB host reserve sampled every 250 ms. Everything lives under
one fresh canonical /private/tmp root. Private HOME, CARGO_HOME, sccache
config, cache and Unix socket; the environment is built from scratch, so no
inherited wrapper, credential or cloud variable reaches any child.
"""

import errno
import hashlib
import io
import json
import os
import re
import resource
import select
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import threading
import time

T0 = time.monotonic()
DEADLINE = 300.0
TEARDOWN_RESERVE = 45.0
STOP_NEW_WORK = DEADLINE - TEARDOWN_RESERVE
OWNED_CAP = 1 << 30
LOG_CAP = 32 << 20
INPUT_CAP = 8 << 20
RESERVE = 20 << 30
CARGO_CAP = 96

SCCACHE = "/opt/homebrew/bin/sccache"
TOOLCHAIN = os.path.expanduser("~/.rustup/toolchains/1.97.1-aarch64-apple-darwin")
CARGO = TOOLCHAIN + "/bin/cargo"
RUSTC = TOOLCHAIN + "/bin/rustc"
REG_CACHE = os.path.expanduser(
    "~/.cargo/registry/cache/index.crates.io-1949cf8c6b5b557f")
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PROFILE = "oxide-quick"
QUICK_ENV = {
    "CARGO_PROFILE_OXIDE_QUICK_INHERITS": "dev",
    "CARGO_PROFILE_OXIDE_QUICK_DEBUG": "line-tables-only",
}
# Client-side mode is disabled whenever SCCACHE_LOG is set (config.rs:1412-1419),
# so clients never log. Hits are counted from the private server's debug log:
# a `storage_get_path` key already present in the cache before the build is a
# hit, otherwise a miss; `handle_client: compile` would mean server-side mode.
GET_RE = re.compile(rb"storage_get_path key=([0-9a-f]+)")
PUT_RE = re.compile(rb"storage_put_raw key=([0-9a-f]+)")
COMPILE_RE = re.compile(rb"handle_client: compile")


class Abort(Exception):
    pass


class State:
    root = None
    ledger = None
    groups = {}          # pgid -> label, every group this harness created
    closed = set()       # pgids positively observed ESRCH
    unknown = {}         # pgid -> reason
    cargo_launches = 0
    reserve_min = None
    reserve_samples = 0
    owned_peak = 0
    abort_reason = None
    lock = threading.Lock()
    stop_monitor = threading.Event()
    preexisting_sccache = set()
    servers = []         # (generation, pid, pgid)
    procs = {}           # pgid -> Popen, so settle can reap
    server_log = None


def elapsed():
    return time.monotonic() - T0


def emit(kind, **fields):
    rec = {"t": round(elapsed(), 3), "kind": kind}
    rec.update(fields)
    line = json.dumps(rec, sort_keys=True)
    print(line, flush=True)
    if State.ledger is not None:
        State.ledger.write(line + "\n")
        State.ledger.flush()


def owned_bytes(path):
    total = 0
    seen = set()
    for dirpath, dirnames, filenames in os.walk(path, followlinks=False):
        for name in dirnames + filenames:
            try:
                st = os.lstat(os.path.join(dirpath, name))
            except OSError:
                continue
            if (st.st_dev, st.st_ino) in seen:
                continue
            seen.add((st.st_dev, st.st_ino))
            total += st.st_blocks * 512
    return total


def group_alive(pgid):
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def kill_all(sig, why):
    """Signal every known owned group; one failure never stops the rest."""
    for pgid in list(State.groups):
        if pgid in State.closed:
            continue
        try:
            os.killpg(pgid, sig)
        except ProcessLookupError:
            pass
        except Exception as exc:  # noqa: BLE001 - keep signalling the others
            State.unknown[pgid] = "signal %s failed: %r (%s)" % (sig, exc, why)


def set_abort(reason):
    with State.lock:
        if State.abort_reason is None:
            State.abort_reason = reason
    kill_all(signal.SIGKILL, reason)


def monitor():
    tick = 0
    while not State.stop_monitor.is_set():
        try:
            st = os.statvfs("/private/tmp")
            free = st.f_bavail * st.f_frsize
            State.reserve_samples += 1
            if State.reserve_min is None or free < State.reserve_min:
                State.reserve_min = free
            if free < RESERVE:
                set_abort("host reserve breach: %d bytes free" % free)
        except Exception as exc:  # noqa: BLE001
            set_abort("reserve unreadable: %r" % exc)
        if tick % 8 == 0 and State.root:
            try:
                used = owned_bytes(State.root)
                State.owned_peak = max(State.owned_peak, used)
                if used > OWNED_CAP:
                    set_abort("owned bytes %d over cap" % used)
            except Exception as exc:  # noqa: BLE001
                set_abort("owned bytes unreadable: %r" % exc)
        if elapsed() > DEADLINE - 8:
            set_abort("300 s deadline approaching")
        tick += 1
        State.stop_monitor.wait(0.25)


def check_budget(need_new_work=True):
    if State.abort_reason:
        raise Abort(State.abort_reason)
    if need_new_work and elapsed() > STOP_NEW_WORK:
        raise Abort("stop-new-work point reached at %.1f s" % elapsed())


def ps_members(pgids):
    out = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,pgid=,stat=,command="],
                         capture_output=True, text=True, timeout=10).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split(None, 4)
        if len(parts) >= 4 and int(parts[2]) in pgids:
            rows.append(line.strip()[:300])
    return rows


def sccache_pids():
    out = subprocess.run(["ps", "-A", "-o", "pid=,pgid=,command="],
                         capture_output=True, text=True, timeout=10).stdout
    found = {}
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and "sccache" in parts[2].split(" ")[0]:
            found[int(parts[0])] = (int(parts[1]), parts[2][:200])
    return found


def settle(pgid, label, grace=5.0):
    """Wait for an owned group to be empty; SIGKILL after grace; record."""
    start = time.monotonic()
    while time.monotonic() - start < grace:
        proc = State.procs.get(pgid)
        if proc is not None:
            proc.poll()
        if not group_alive(pgid):
            State.closed.add(pgid)
            return round((time.monotonic() - start) * 1000, 1)
        time.sleep(0.005)
    members = ps_members({pgid})
    emit("group-lingering", label=label, pgid=pgid, members=members)
    try:
        os.killpg(pgid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    start2 = time.monotonic()
    while time.monotonic() - start2 < 5.0:
        if State.procs.get(pgid) is not None:
            State.procs[pgid].poll()
        if not group_alive(pgid):
            State.closed.add(pgid)
            return "killed-after-grace"
        time.sleep(0.01)
    State.unknown[pgid] = "still alive after SIGKILL: %s" % ps_members({pgid})
    raise Abort("owned group %d (%s) did not close" % (pgid, label))


def spawn(label, argv, env, cwd, logname, cargo=False):
    check_budget()
    if cargo:
        if State.cargo_launches >= CARGO_CAP:
            raise Abort("cargo launch cap reached")
        State.cargo_launches += 1
    out = open(os.path.join(State.root, "logs", logname + ".out"), "wb")
    err = open(os.path.join(State.root, "logs", logname + ".err"), "wb")
    proc = subprocess.Popen(argv, env=env, cwd=cwd, stdout=out, stderr=err,
                            stdin=subprocess.DEVNULL, start_new_session=True)
    State.groups[proc.pid] = label
    State.procs[proc.pid] = proc
    proc._logs = (out, err, logname)
    return proc


def finish(proc, timeout):
    remaining = min(timeout, DEADLINE - 10 - elapsed())
    try:
        rc = proc.wait(timeout=max(remaining, 0.1))
    except subprocess.TimeoutExpired:
        set_abort("command %s exceeded its bound" % proc._logs[2])
        rc = proc.wait(timeout=10)
    for f in proc._logs[:2]:
        f.close()
    closed_ms = settle(proc.pid, State.groups[proc.pid])
    return rc, closed_ms


def run(label, argv, env, cwd, logname, timeout=120, cargo=False):
    start = time.monotonic()
    proc = spawn(label, argv, env, cwd, logname, cargo=cargo)
    rc, closed_ms = finish(proc, timeout)
    secs = round(time.monotonic() - start, 3)
    if State.abort_reason:
        raise Abort(State.abort_reason)
    return rc, secs, closed_ms


def read_log(logname, which="err"):
    path = os.path.join(State.root, "logs", "%s.%s" % (logname, which))
    with open(path, "rb") as f:
        return f.read()


def cache_keys():
    keys = set()
    for _, _, files in os.walk(os.path.join(State.root, "sccache", "cache")):
        keys.update(files)
    return keys


def begin_counts():
    log = State.server_log
    return (cache_keys(), log, os.path.getsize(log) if log and os.path.exists(log) else 0)


def end_counts(token):
    before, log, offset = token
    if not log or log != State.server_log or not os.path.exists(log):
        return {"unknown": "server generation changed or log missing"}
    with open(log, "rb") as f:
        f.seek(offset)
        seg = f.read()
    gets = [m.decode() for m in GET_RE.findall(seg)]
    puts = [m.decode() for m in PUT_RE.findall(seg)]
    return {"hits": sum(1 for k in gets if k in before),
            "misses": sum(1 for k in gets if k not in before),
            "puts": len(puts), "server_side_compiles": len(COMPILE_RE.findall(seg)),
            "hit_keys": sorted(k[:12] for k in gets if k in before)}


def snapshot(path):
    snap = {}
    for dirpath, dirnames, filenames in os.walk(path, followlinks=False):
        for name in dirnames + filenames:
            p = os.path.join(dirpath, name)
            try:
                st = os.lstat(p)
            except OSError:
                continue
            snap[os.path.relpath(p, path)] = (st.st_ino, st.st_size, st.st_mtime_ns,
                                              stat.S_IFMT(st.st_mode))
    return snap


def snap_diff(a, b):
    added = sorted(set(b) - set(a))
    removed = sorted(set(a) - set(b))
    changed = sorted(k for k in set(a) & set(b) if a[k] != b[k])
    return {"added": added[:20], "removed": removed[:20], "changed": changed[:20],
            "counts": [len(added), len(removed), len(changed)]}


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# --------------------------------------------------------------------------
# Setup: root, isolation evidence, vendored inputs, environments

def make_root():
    root = os.path.realpath(tempfile.mkdtemp(prefix="oxide-sc101.", dir="/private/tmp"))
    if not root.startswith("/private/tmp/"):
        raise SystemExit("root is not canonical under /private/tmp: %s" % root)
    State.root = root
    for d in ("logs", "tmp", "home", "cargo-home", "vendor", "proj", "slots",
              "alias", "sccache", "sccache/cache", "gate"):
        os.makedirs(os.path.join(root, d))
    State.ledger = open(os.path.join(root, "logs", "ledger.jsonl"), "w")
    open(os.path.join(root, "sccache", "conf.toml"), "w").close()
    return root


def ancestor_configs(path):
    found = []
    d = path
    while True:
        for name in (".cargo/config", ".cargo/config.toml", "Cargo.toml"):
            if os.path.lexists(os.path.join(d, name)):
                found.append(os.path.join(d, name))
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    return found


def isolation_evidence(root, phase):
    # Cargo reads .cargo/config{,.toml} in every ancestor of its cwd and then
    # $CARGO_HOME/config.toml; a stray ancestor Cargo.toml could also be taken
    # as a workspace root. Every fixture lives below `root`.
    probe = os.path.join(root, "proj")
    anc = [p for p in ancestor_configs(os.path.dirname(probe))]
    emit("isolation", phase=phase, root=root, ancestor_cargo_files=anc,
         cargo_home_config=open(os.path.join(root, "cargo-home", "config.toml")).read()
         if os.path.exists(os.path.join(root, "cargo-home", "config.toml")) else None)
    if anc:
        raise Abort("ancestor Cargo config/manifest found: %s" % anc)


def lock_packages(text):
    return re.findall(
        r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"\n'
        r'source = "registry\+https://github.com/rust-lang/crates.io-index"\n'
        r'checksum = "([0-9a-f]+)"', text)


def vendor(root, packages):
    """Extract named cached .crate archives after checking their checksums."""
    receipts = []
    total = 0
    for name, version, checksum in packages:
        src = os.path.join(REG_CACHE, "%s-%s.crate" % (name, version))
        digest = sha256(src)
        if digest != checksum:
            raise Abort("checksum mismatch for %s %s" % (name, version))
        total += os.path.getsize(src)
        dest = os.path.join(root, "vendor")
        prefix = "%s-%s/" % (name, version)
        with tarfile.open(src, "r:gz") as tar:
            for member in tar.getmembers():
                if not (member.name + "/").startswith(prefix) or ".." in member.name.split("/"):
                    raise Abort("unexpected archive member %s" % member.name)
                if not (member.isfile() or member.isdir()):
                    raise Abort("non-regular archive member %s" % member.name)
            tar.extractall(dest)
        with open(os.path.join(dest, prefix, ".cargo-checksum.json"), "w") as f:
            json.dump({"files": {}, "package": checksum}, f)
        receipts.append("%s %s %s" % (name, version, digest))
    if total > INPUT_CAP:
        raise Abort("vendored input bytes %d over cap" % total)
    emit("vendored", crates=receipts, archive_bytes=total)
    with open(os.path.join(root, "cargo-home", "config.toml"), "w") as f:
        f.write('[source.crates-io]\nreplace-with = "vendored"\n\n'
                '[source.vendored]\ndirectory = "%s"\n\n[net]\noffline = true\n'
                % os.path.join(root, "vendor"))


def base_env(root):
    env = {
        "PATH": TOOLCHAIN + "/bin:/usr/bin:/bin",
        "HOME": os.path.join(root, "home"),
        "CARGO_HOME": os.path.join(root, "cargo-home"),
        "RUSTC": RUSTC,
        "TMPDIR": os.path.join(root, "tmp"),
        "TMP": os.path.join(root, "tmp"),
        "TEMP": os.path.join(root, "tmp"),
        "OXIDE_TASK_SCRATCH_ROOT": os.path.join(root, "tmp"),
        "CARGO_TERM_COLOR": "never",
        "LANG": "C",
        "SCCACHE_CONF": os.path.join(root, "sccache", "conf.toml"),
    }
    env.update(QUICK_ENV)
    return env


def sccache_env(root):
    env = base_env(root)
    env.update({
        "SCCACHE_DIR": os.path.join(root, "sccache", "cache"),
        "SCCACHE_SERVER_UDS": os.path.join(root, "s"),
        "SCCACHE_CACHE_SIZE": "600M",
        "SCCACHE_NO_DAEMON": "1",
        "SCCACHE_IDLE_TIMEOUT": "30",
    })
    return env


def client_env(root, alias, target):
    env = sccache_env(root)
    env.update({
        "RUSTC_WRAPPER": SCCACHE,
        "SCCACHE_CLIENT_SIDE": "1",
        "CARGO_BUILD_BUILD_DIR": alias,
        "CARGO_TARGET_DIR": target,
    })
    return env


def plain_env(root, alias, target):
    env = base_env(root)
    env.update({"CARGO_BUILD_BUILD_DIR": alias, "CARGO_TARGET_DIR": target})
    return env


def start_server(root, generation):
    env = sccache_env(root)
    env.update({"SCCACHE_START_SERVER": "1", "SCCACHE_LOG": "debug",
                "SCCACHE_IDLE_TIMEOUT": "0"})
    sock = env["SCCACHE_SERVER_UDS"]
    if os.path.lexists(sock):
        os.unlink(sock)
    proc = spawn("sccache-server-%d" % generation, [SCCACHE], env, root,
                 "server-%d" % generation)
    import socket
    start = time.monotonic()
    while time.monotonic() - start < 10:
        if proc.poll() is not None:
            raise Abort("server generation %d exited at start" % generation)
        if os.path.exists(sock):
            s = socket.socket(socket.AF_UNIX)
            try:
                s.connect(sock)
                s.close()
                break
            except OSError:
                s.close()
        time.sleep(0.01)
    else:
        raise Abort("server generation %d not ready" % generation)
    ready = round(time.monotonic() - start, 3)
    State.server_log = os.path.join(State.root, "logs", "server-%d.err" % generation)
    State.servers.append((generation, proc.pid, proc.pid))
    emit("server-start", generation=generation, pid=proc.pid, pgid=proc.pid,
         ready_s=ready, members=ps_members({proc.pid}))
    return proc


def stop_server(root, proc, generation):
    if proc.poll() is None:
        env = sccache_env(root)
        rc, secs, _ = run("stop-%d" % generation, [SCCACHE, "--stop-server"], env, root,
                          "stop-%d" % generation, timeout=20)
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait(timeout=5)
    else:
        rc, secs = None, 0
    for f in proc._logs[:2]:
        f.close()
    closed = settle(proc.pid, "sccache-server-%d" % generation)
    emit("server-stop", generation=generation, pid=proc.pid, stop_rc=rc,
         exit=proc.returncode, closed_ms=closed)


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def make_slot(root, name):
    path = os.path.join(root, "slots", name)
    os.makedirs(path)
    write(os.path.join(path, "SLOT_ID"), name + "\n")
    return path


def point(root, index, slot):
    link = os.path.join(root, "alias", str(index))
    tmp = link + ".new"
    if os.path.lexists(tmp):
        os.unlink(tmp)
    os.symlink(slot, tmp)
    os.replace(tmp, link)
    emit("alias", index=index, slot=os.path.basename(slot))
    return link


def cargo_build(root, label, proj, env, extra=()):
    argv = [CARGO, "build", "--profile", PROFILE, "--offline", "-v"] + list(extra)
    token = begin_counts()
    rc, secs, closed = run(label, argv, env, proj, label, timeout=150, cargo=True)
    counts = end_counts(token) if "RUSTC_WRAPPER" in env else None
    return rc, secs, closed, counts


# --------------------------------------------------------------------------
# Fixtures

PROFILE_TOML = ""  # Oxide passes its recipe through the environment only.

HZ_MANIFEST = """[package]
name = "hz"
version = "0.1.0"
edition = "2021"

[dependencies]
cfg-if = "=1.0.5"

[workspace]
"""

HZ_BUILD = r'''use std::{env, fs, path::Path};
fn main() {
    let out = env::var("OUT_DIR").unwrap();
    let canon = fs::canonicalize(&out).unwrap();
    let mut id = String::from("unknown");
    for a in canon.ancestors() {
        if let Ok(s) = fs::read_to_string(a.join("SLOT_ID")) {
            id = s.trim().to_string();
            break;
        }
    }
    fs::write(Path::new(&out).join("marker.txt"), &id).unwrap();
    let mut rec = String::new();
    let mut vars: Vec<_> = env::vars().filter(|(k, _)| k == "OUT_DIR" || k.starts_with("CARGO")).collect();
    vars.sort();
    for (k, v) in vars {
        rec.push_str(&format!("{k}={v}\n"));
    }
    rec.push_str(&format!("CANONICAL_OUT_DIR={}\n", canon.display()));
    fs::write(Path::new(&out).join("buildenv.txt"), rec).unwrap();
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-env=HZ_OUT_DIR={out}");
    println!("cargo:rerun-if-changed=build.rs");
}
'''

HZ_MAIN = r'''const MARK: &str = include_str!(concat!(env!("OUT_DIR"), "/marker.txt"));
fn main() {
    let runtime = std::fs::read_to_string(format!("{}/marker.txt", env!("HZ_OUT_DIR")))
        .unwrap_or_else(|e| format!("ERR {e}"));
    println!("compiled_marker={}", MARK.trim());
    println!("out_dir={}", env!("OUT_DIR"));
    println!("hz_out_dir={}", env!("HZ_OUT_DIR"));
    println!("runtime_marker={}", runtime.trim());
    println!("cargo_manifest_dir={}", env!("CARGO_MANIFEST_DIR"));
    println!("cargo_target_dir={:?}", option_env!("CARGO_TARGET_DIR"));
    println!("cargo_build_build_dir={:?}", option_env!("CARGO_BUILD_BUILD_DIR"));
    let os = { cfg_if::cfg_if! { if #[cfg(unix)] { "unix" } else { "other" } } };
    println!("cfg_if={os}");
}
'''

BLK_MANIFEST = """[workspace]
members = ["gate", "user"]
resolver = "2"

[profile.dev]
incremental = false
"""

GATE_MANIFEST = """[package]
name = "gate"
version = "0.1.0"
edition = "2021"

[lib]
proc-macro = true
"""

GATE_LIB = r'''use proc_macro::TokenStream;
use std::{fs, path::Path, thread, time::{Duration, Instant}};
const DIR: &str = "@GATE@";
#[proc_macro]
pub fn hold(_input: TokenStream) -> TokenStream {
    let d = Path::new(DIR);
    // sccache's hashing runs `rustc --emit dep-info` first (rust.rs:241-268),
    // which expands this macro; hold the next expansion, the real compile.
    // The harness checks the held PID's command line independently.
    let args: Vec<String> = std::env::args().collect();
    if d.join("block").exists() && !d.join("prepass").exists() {
        fs::write(d.join("prepass"), format!("{}\n{}", std::process::id(), args.join(" "))).unwrap();
    } else if d.join("block").exists() {
        fs::write(d.join("entered"), format!("{}\n{}", std::process::id(), args.join(" "))).unwrap();
        let start = Instant::now();
        while !d.join("release").exists() {
            if start.elapsed() > Duration::from_secs(60) {
                panic!("gate timeout");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    "pub const HELD: u32 = 1;".parse().unwrap()
}
'''

USER_MANIFEST = """[package]
name = "user"
version = "0.1.0"
edition = "2021"

[dependencies]
gate = { path = "../gate" }
cfg-if = "=1.0.5"
"""

USER_LIB = "gate::hold!();\npub fn held() -> u32 { HELD }\n"

HP_MANIFEST = """[package]
name = "hp"
version = "0.1.0"
edition = "2021"

[dependencies]
memchr = "=2.8.3"

[workspace]

[profile.dev]
incremental = false
"""

HP_LIB = "pub fn find(h: &[u8]) -> Option<usize> { memchr::memchr(b'x', h) }\n"


def make_hz(root, name):
    p = os.path.join(root, "proj", name)
    write(os.path.join(p, "Cargo.toml"), HZ_MANIFEST)
    write(os.path.join(p, "build.rs"), HZ_BUILD)
    write(os.path.join(p, "src", "main.rs"), HZ_MAIN)
    return p


def make_blk(root):
    p = os.path.join(root, "proj", "blk")
    write(os.path.join(p, "Cargo.toml"), BLK_MANIFEST)
    write(os.path.join(p, "gate", "Cargo.toml"), GATE_MANIFEST)
    write(os.path.join(p, "gate", "src", "lib.rs"),
          GATE_LIB.replace("@GATE@", os.path.join(root, "gate")))
    write(os.path.join(p, "user", "Cargo.toml"), USER_MANIFEST)
    write(os.path.join(p, "user", "src", "lib.rs"), USER_LIB)
    return p


def make_hp(root):
    p = os.path.join(root, "proj", "hp")
    write(os.path.join(p, "Cargo.toml"), HP_MANIFEST)
    write(os.path.join(p, "src", "lib.rs"), HP_LIB)
    return p


def server_open_slot_files(server_pid, slots_root):
    out = subprocess.run(["lsof", "-n", "-P", "-p", str(server_pid), "-Fn"],
                         capture_output=True, text=True, timeout=10).stdout
    names = [l[1:] for l in out.splitlines() if l.startswith("n")]
    return [n for n in names if n.startswith(slots_root)], len(names)


def wait_file(path, proc, bound):
    start = time.monotonic()
    while time.monotonic() - start < bound:
        if os.path.exists(path):
            return round(time.monotonic() - start, 3)
        if proc.poll() is not None:
            return None
        check_budget(need_new_work=False)
        time.sleep(0.005)
    return None


# --------------------------------------------------------------------------
# Safety mode

def safety(root):
    lock = open(os.path.join(REPO, "Cargo.lock")).read()
    vendor(root, lock_packages(lock))
    isolation_evidence(root, "before-cargo")
    slots_root = os.path.join(root, "slots")
    gate = os.path.join(root, "gate")
    alias0 = os.path.join(root, "alias", "0")
    alias1 = os.path.join(root, "alias", "1")
    blk = make_blk(root)
    hp = make_hp(root)
    p1 = make_hz(root, "p1")
    p2 = make_hz(root, "p2")
    server = start_server(root, 1)
    generation = 1
    results = {}

    # C1: SIGKILL the build group while rustc, under a client-side sccache
    # client, is inside a proc-macro expansion of `user` (a cache miss).
    k1 = make_slot(root, "k1")
    point(root, 0, k1)
    write(os.path.join(gate, "block"), "1")
    env = client_env(root, alias0, os.path.join(blk, "target"))
    tok = begin_counts()
    proc = spawn("C1", [CARGO, "build", "--profile", PROFILE, "--offline", "-v"], env, blk,
                 "C1", cargo=True)
    waited = wait_file(os.path.join(gate, "entered"), proc, 60)
    if waited is None:
        rc, _ = finish(proc, 5)
        results["C1"] = {"handshake": False, "rc": rc}
        emit("C1", **results["C1"])
        raise Abort("C1 handshake not reached")
    entered = open(os.path.join(gate, "entered")).read()
    rustc_pid = int(entered.split()[0])
    members = ps_members({proc.pid})
    held_cmd = subprocess.run(["ps", "-o", "pid=,ppid=,pgid=,command=", "-p", str(rustc_pid)],
                              capture_output=True, text=True, timeout=10).stdout.strip()[:2500]
    before = snapshot(k1)
    os.killpg(proc.pid, signal.SIGKILL)
    kill_at = time.monotonic()
    proc.wait(timeout=10)
    for f in proc._logs[:2]:
        f.close()
    closed_ms = settle(proc.pid, "C1")
    after = snapshot(k1)
    open_slot, nopen = server_open_slot_files(server.pid, slots_root)
    later = snapshot(k1)
    rustc_gone = not pid_alive(rustc_pid)
    results["C1"] = {
        "handshake_s": waited, "rustc_pid": rustc_pid, "group_members_at_hold": members,
        "held_rustc_args_from_macro": entered.split("\n", 1)[-1][:1500],
        "held_process_ps": held_cmd,
        "held_is_real_compile": "--emit=dep-info,metadata,link" in held_cmd
        and "deps.d" not in held_cmd,
        "group_empty_ms_after_kill": closed_ms, "rustc_pid_esrch": rustc_gone,
        "slot_diff_hold_to_closure": snap_diff(before, after),
        "slot_diff_closure_to_later": snap_diff(after, later),
        "server_open_files_under_slots": open_slot, "server_open_files_total": nopen,
        "cache_counts": end_counts(tok),
        "kill_to_later_s": round(time.monotonic() - kill_at, 3),
    }
    emit("C1", **results["C1"])

    # C2: kill the private daemon while the client's rustc is held, then
    # release the hold and observe the client's result and group closure.
    for n in ("entered", "prepass"):
        os.unlink(os.path.join(gate, n))
    k2 = make_slot(root, "k2")
    point(root, 0, k2)
    tok = begin_counts()
    proc = spawn("C2", [CARGO, "build", "--profile", PROFILE, "--offline", "-v"], env, blk,
                 "C2", cargo=True)
    waited = wait_file(os.path.join(gate, "entered"), proc, 60)
    if waited is None:
        rc, _ = finish(proc, 5)
        results["C2"] = {"handshake": False, "rc": rc}
        emit("C2", **results["C2"])
        raise Abort("C2 handshake not reached")
    before_pids = sccache_pids()
    os.killpg(server.pid, signal.SIGKILL)
    server.wait(timeout=10)
    for f in server._logs[:2]:
        f.close()
    server_closed = settle(server.pid, "sccache-server-1")
    write(os.path.join(gate, "release"), "1")
    rc, closed_ms = finish(proc, 90)
    after_pids = sccache_pids()
    new_untracked = {p: v for p, v in after_pids.items()
                     if p not in before_pids and v[0] not in State.groups}
    results["C2"] = {
        "handshake_s": waited, "server_killed_closed_ms": server_closed, "cargo_rc": rc,
        "group_empty_ms_after_exit": closed_ms,
        "untracked_new_sccache_processes": {str(k): v for k, v in new_untracked.items()},
        "cache_counts": end_counts(tok),
        "stderr_tail": read_log("C2")[-1500:].decode("utf-8", "replace"),
    }
    emit("C2", **results["C2"])
    for n in ("block", "release", "entered", "prepass"):
        if os.path.exists(os.path.join(gate, n)):
            os.unlink(os.path.join(gate, n))
    generation = 2
    server = start_server(root, generation)

    # C3: SIGKILL while a client-side cache hit is mid-write into the slot.
    # Pre-made outputs in a read-only deps/ force sccache's direct-write
    # fallback (cache_io.rs) so the rlib write blocks on a FIFO.
    h1 = make_slot(root, "h1")
    point(root, 0, h1)
    henv = client_env(root, alias0, os.path.join(hp, "target"))
    rc, secs, closed, counts = cargo_build(root, "C3-populate", hp, henv)
    deps = os.path.join(h1, "debug", "deps")
    if not os.path.isdir(deps):
        deps = os.path.join(h1, PROFILE, "deps")
    names = sorted(n for n in os.listdir(deps) if "memchr" in n)
    rlib = [n for n in names if n.endswith(".rlib")]
    emit("C3-populate", rc=rc, secs=secs, cache_counts=counts, memchr_outputs=names,
         deps=os.path.relpath(deps, h1))
    if rc != 0 or len(rlib) != 1:
        raise Abort("C3 populate failed")
    rlib_size = os.path.getsize(os.path.join(deps, rlib[0]))
    h2 = make_slot(root, "h2")
    deps2 = os.path.join(h2, os.path.relpath(deps, h1))
    os.makedirs(deps2)
    for n in names:
        if n == rlib[0]:
            os.mkfifo(os.path.join(deps2, n))
        else:
            open(os.path.join(deps2, n), "w").close()
    os.chmod(deps2, 0o555)
    point(root, 0, h2)
    fd = os.open(os.path.join(deps2, rlib[0]), os.O_RDONLY | os.O_NONBLOCK)
    tok = begin_counts()
    proc = spawn("C3", [CARGO, "build", "--profile", PROFILE, "--offline", "-v"], henv, hp,
                 "C3", cargo=True)
    start = time.monotonic()
    got = False
    while time.monotonic() - start < 40 and proc.poll() is None:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            got = True
            break
        check_budget(need_new_work=False)
    c3 = {"rlib": rlib[0], "rlib_size": rlib_size}
    if got:
        os.killpg(proc.pid, signal.SIGSTOP)
        stopped_members = ps_members({proc.pid})
        drained, eof = 0, False
        while True:
            try:
                chunk = os.read(fd, 65536)
            except BlockingIOError:
                break
            if not chunk:
                eof = True
                break
            drained += len(chunk)
        before = snapshot(h2)
        os.killpg(proc.pid, signal.SIGKILL)
        proc.wait(timeout=10)
        for f in proc._logs[:2]:
            f.close()
        closed_ms = settle(proc.pid, "C3")
        after = snapshot(h2)
        open_slot, nopen = server_open_slot_files(server.pid, slots_root)
        later = snapshot(h2)
        c3.update({
            "handshake_s": round(time.monotonic() - start, 3),
            "members_stopped": stopped_members, "bytes_received_before_kill": drained,
            "writer_closed_before_kill": eof,
            "mid_write": (not eof) and drained < rlib_size,
            "group_empty_ms_after_kill": closed_ms,
            "slot_diff_hold_to_closure": snap_diff(before, after),
            "slot_diff_closure_to_later": snap_diff(after, later),
            "server_open_files_under_slots": open_slot, "server_open_files_total": nopen,
        })
    else:
        rc, closed_ms = finish(proc, 5)
        c3.update({"handshake": False, "rc": rc})
    os.close(fd)
    os.chmod(deps2, 0o755)
    c3["cache_counts"] = end_counts(tok)
    results["C3"] = c3
    emit("C3", **c3)

    # B: alias index model on the hz fixture (Oxide env: build dir = alias,
    # target dir = checkout).
    def hz(label, proj, index, slot, target_env=True):
        link = point(root, index, slot) if slot else os.path.join(root, "alias", str(index))
        tgt = os.path.join(proj, "target")
        env = client_env(root, link, tgt)
        extra = []
        if not target_env:
            del env["CARGO_TARGET_DIR"]
            extra = ["--config", "build.target-dir=%s" % repr(tgt).replace("'", '"')]
        rc, secs, closed, counts = cargo_build(root, label, proj, env, extra)
        out = subprocess.run([os.path.join(tgt, PROFILE, "hz")], capture_output=True,
                             text=True, timeout=10, env={"PATH": "/usr/bin:/bin"}).stdout \
            if rc == 0 else ""
        verbose = read_log(label).decode("utf-8", "replace")
        status = [l.strip() for l in verbose.splitlines()
                  if re.match(r"\s*(Fresh|Dirty|Compiling|Running)", l)]
        status = [s if not s.startswith("Running") else s[:200] for s in status]
        hz_bin = [l for l in verbose.splitlines()
                  if "Running `" in l and "--crate-name hz" in l and "--crate-type bin" in l]
        paths = None
        if hz_bin:
            line = hz_bin[-1]
            paths = {
                "env": dict(re.findall(r"\b(OUT_DIR|CARGO_MANIFEST_DIR|CARGO_TARGET_DIR|"
                                       r"CARGO_BUILD_BUILD_DIR|HZ_OUT_DIR)=(\S+)", line)),
                "flags": re.findall(r"(--out-dir|-L|--extern) (\S+)", line),
            }
        rec = {"rc": rc, "secs": secs, "closed_ms": closed, "cache_counts": counts,
               "binary": dict(l.split("=", 1) for l in out.splitlines() if "=" in l),
               "cargo_status": status, "hz_rustc_paths": paths,
               "slots_path_in_cargo_output": "/slots/" in verbose,
               "alias_index": index, "slot": os.path.basename(slot) if slot else None}
        emit(label, **rec)
        results[label] = rec
        return rec

    sA, sB, sC, sD = (make_slot(root, n) for n in ("sA", "sB", "sC", "sD"))
    hz("B1-p1-idx0-sA", p1, 0, sA)
    hz("B2-p1-idx0-sB", p1, 0, sB)
    hz("B3-p1-idx1-sC", p1, 1, sC)
    hz("B4-p2-idx0-sD", p2, 0, sD)
    sE, sF = make_slot(root, "sE"), make_slot(root, "sF")
    hz("B4b-p1-idx0-sE-configtarget", p1, 0, sE, target_env=False)
    hz("B4c-p2-idx0-sF-configtarget", p2, 0, sF, target_env=False)

    # Slot evidence from sA before the hazard: build-script output and
    # whether alias or real slot paths are recorded.
    def slot_paths(slot):
        alias_hits, slot_hits, files = 0, 0, 0
        for dirpath, _, filenames in os.walk(slot):
            for n in filenames:
                p = os.path.join(dirpath, n)
                try:
                    data = open(p, "rb").read()
                except OSError:
                    continue
                files += 1
                if b"/alias/" in data:
                    alias_hits += 1
                if slots_root.encode() in data:
                    slot_hits += 1
        return {"files": files, "files_with_alias_path": alias_hits,
                "files_with_real_slot_path": slot_hits}

    build_out = {}
    for dirpath, _, filenames in os.walk(os.path.join(sA, PROFILE, "build")):
        for n in ("output", "root-output"):
            if n in filenames:
                build_out[os.path.relpath(os.path.join(dirpath, n), sA)] = \
                    open(os.path.join(dirpath, n)).read()[:800]
        if "buildenv.txt" in filenames:
            build_out[os.path.relpath(os.path.join(dirpath, "buildenv.txt"), sA)] = \
                open(os.path.join(dirpath, "buildenv.txt")).read()[:3000]
    emit("sA-recorded", files=build_out, path_scan=slot_paths(sA))

    # B5 hazard: sA (built under index 0) now leased under index 1 while
    # index 0 points at sB, which holds p1 built with marker sB.
    point(root, 0, sB)
    hz("B5-hazard-p1-idx1-sA", p1, 1, sA)
    # B6 affinity mitigation: sB keeps index 0.
    hz("B6-affinity-p1-idx0-sB", p1, 0, sB)
    # B7 invalidation mitigation: empty sA before it is built under index 1.
    for n in os.listdir(sA):
        if n != "SLOT_ID":
            p = os.path.join(sA, n)
            shutil.rmtree(p) if os.path.isdir(p) and not os.path.islink(p) else os.unlink(p)
    hz("B7-invalidate-p1-idx1-sA", p1, 1, sA)
    # B8 warm no-op reference for the invalidation cost.
    hz("B8-noop-p1-idx1-sA", p1, 1, sA)

    binary = open(os.path.join(p1, "target", PROFILE, "hz"), "rb").read()
    emit("debuginfo-paths", binary_bytes=len(binary),
         alias0=binary.count(b"/alias/0"), alias1=binary.count(b"/alias/1"),
         real_slot=binary.count(slots_root.encode()),
         slot_scan_sB=slot_paths(sB))
    stop_server(root, server, generation)

    # Pre-declared benefit gate (PROTOCOL.md): both closure handshakes reached
    # with empty groups, no slot change after closure, no server-held slot
    # file, every wrapped build client-side, and both mitigations correct.
    def quiet(case):
        c = results.get(case, {})
        return (c.get("slot_diff_closure_to_later", {}).get("counts") == [0, 0, 0]
                and not c.get("server_open_files_under_slots"))
    def marks(label, want):
        b = results.get(label, {}).get("binary", {})
        return (results.get(label, {}).get("rc") == 0 and b.get("compiled_marker") == want
                and b.get("runtime_marker") == want)
    wrapped = [r.get("cache_counts") for r in results.values() if isinstance(r, dict)]
    server_side = sum(c.get("server_side_compiles", 0) for c in wrapped if isinstance(c, dict))
    gate_checks = {
        "C1": results["C1"].get("held_is_real_compile") is True and quiet("C1"),
        "C3": results["C3"].get("mid_write") is True and quiet("C3"),
        "client_side_everywhere": server_side == 0,
        "B6_affinity_correct": marks("B6-affinity-p1-idx0-sB", "sB"),
        "B7_invalidation_correct": marks("B7-invalidate-p1-idx1-sA", "sA"),
    }
    results["benefit_gate"] = gate_checks
    emit("benefit-gate", checks=gate_checks, server_side_compiles=server_side)
    return results


def lock_subset(names):
    lock = open(os.path.join(REPO, "Cargo.lock")).read()
    pk = [p for p in lock_packages(lock) if p[0] in names]
    if len(pk) != len(names):
        raise Abort("named crates missing from Cargo.lock: %s" % names)
    return pk


def pid_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


# --------------------------------------------------------------------------
# Benefit mode

def benefit(root):
    proj = os.path.join(root, "proj", "oxide")
    os.makedirs(proj)
    head = subprocess.run(["git", "-C", REPO, "rev-parse", "HEAD"], capture_output=True,
                          text=True, check=True).stdout.strip()
    data = subprocess.run(["git", "-C", REPO, "archive", "--format=tar", head, "--",
                           "Cargo.toml", "Cargo.lock", "src", "rust-toolchain.toml"],
                          capture_output=True, check=True).stdout
    if len(data) > INPUT_CAP:
        raise Abort("source input over cap")
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        tar.extractall(proj)
    emit("staged-source", head=head, tar_bytes=len(data),
         tar_sha256=hashlib.sha256(data).hexdigest())
    isolation_evidence(root, "before-cargo")
    alias0 = os.path.join(root, "alias", "0")
    target = os.path.join(proj, "target")
    cache = os.path.join(root, "sccache", "cache")
    samples = []

    def one(label, wrapped):
        check_budget()
        slot = make_slot(root, label)
        point(root, 0, slot)
        ru0 = resource.getrusage(resource.RUSAGE_CHILDREN)
        cache_before = (owned_bytes(cache), sum(len(f) for _, _, f in os.walk(cache)))
        start = time.monotonic()
        server = start_server(root, len(State.servers) + 1) if wrapped else None
        tok = begin_counts()
        env = client_env(root, alias0, target) if wrapped else plain_env(root, alias0, target)
        argv = [CARGO, "build", "--profile", PROFILE, "--locked", "--offline"]
        rc, secs, closed = run(label, argv, env, proj, label, timeout=200, cargo=True)
        counts = end_counts(tok) if wrapped else None
        if server:
            stop_server(root, server, len(State.servers))
        total = round(time.monotonic() - start, 3)
        ru1 = resource.getrusage(resource.RUSAGE_CHILDREN)
        cache_after = (owned_bytes(cache), sum(len(f) for _, _, f in os.walk(cache)))
        rec = {"label": label, "wrapped": wrapped, "rc": rc, "cargo_s": secs,
               "total_s_incl_server": total, "slot_bytes": owned_bytes(slot),
               "cache_allocated_bytes_before_after": [cache_before[0], cache_after[0]],
               "cache_files_before_after": [cache_before[1], cache_after[1]],
               "children_oublock_delta": ru1.ru_oublock - ru0.ru_oublock,
               "children_user_sys_s": round((ru1.ru_utime - ru0.ru_utime)
                                            + (ru1.ru_stime - ru0.ru_stime), 3)}
        if wrapped:
            rec["cache_counts"] = counts
        emit("sample", **rec)
        samples.append(rec)
        if rc != 0:
            raise Abort("%s failed rc=%s" % (label, rc))
        shutil.rmtree(slot)
        return rec

    State.server_log = None
    one("W0-populate", True)
    order = [("P1-U", False), ("P1-W", True), ("P2-W", True), ("P2-U", False),
             ("P3-U", False), ("P3-W", True)]
    for label, wrapped in order:
        one(label, wrapped)
    return samples


# --------------------------------------------------------------------------

def main():
    mode = "safety+conditional-benefit"
    root = make_root()
    State.preexisting_sccache = set(sccache_pids())
    tool = {"sccache": SCCACHE, "sccache_real": os.path.realpath(SCCACHE),
            "sccache_sha256": sha256(os.path.realpath(SCCACHE)),
            "cargo_sha256": sha256(CARGO), "rustc_sha256": sha256(RUSTC),
            "script_sha256": sha256(os.path.abspath(__file__))}
    emit("start", mode=mode, root=root, pid=os.getpid(), tools=tool,
         preexisting_sccache_pids=len(State.preexisting_sccache),
         limits={"deadline_s": DEADLINE, "stop_new_work_s": STOP_NEW_WORK,
                 "owned_cap": OWNED_CAP, "cargo_cap": CARGO_CAP, "reserve": RESERVE})
    mon = threading.Thread(target=monitor, daemon=True)
    mon.start()
    status, result = "ok", None
    try:
        result = safety(root)
        if all(result["benefit_gate"].values()):
            benefit(root)
        else:
            emit("benefit-skipped", reason="pre-declared gate not met")
    except Abort as exc:
        status = "aborted: %s" % exc
    except Exception as exc:  # noqa: BLE001 - always reach teardown
        status = "error: %r" % exc
    # Teardown: settle every known owned group, independently.
    for pgid, label in list(State.groups.items()):
        if pgid in State.closed:
            continue
        try:
            os.killpg(pgid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except Exception as exc:  # noqa: BLE001
            State.unknown[pgid] = "teardown signal failed: %r" % exc
        try:
            settle(pgid, label, grace=3.0)
        except Abort as exc:
            State.unknown.setdefault(pgid, str(exc))
    for dirpath, dirnames, _ in os.walk(os.path.join(root, "slots")):
        for d in dirnames:
            p = os.path.join(dirpath, d)
            try:
                if not os.path.islink(p) and stat.S_IMODE(os.lstat(p).st_mode) == 0o555:
                    os.chmod(p, 0o755)
            except OSError:
                pass
    State.stop_monitor.set()
    mon.join(timeout=2)
    final_bytes = owned_bytes(root)
    logs = owned_bytes(os.path.join(root, "logs"))
    leftover = {str(p): v for p, v in sccache_pids().items()
                if p not in State.preexisting_sccache}
    emit("teardown", status=status, closed_groups=len(State.closed),
         known_groups=len(State.groups), unknown=State.unknown,
         cargo_launches=State.cargo_launches, owned_peak=State.owned_peak,
         owned_final=final_bytes, logs_bytes=logs, log_cap=LOG_CAP,
         reserve_min=State.reserve_min, reserve_samples=State.reserve_samples,
         sccache_processes_not_preexisting=leftover, elapsed_s=round(elapsed(), 3),
         abort_reason=State.abort_reason)
    State.ledger.close()
    ok = status == "ok" and not State.unknown and elapsed() <= DEADLINE
    print("retained scratch %s" % root)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
