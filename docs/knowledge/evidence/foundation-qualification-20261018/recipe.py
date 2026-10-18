"""Foundation qualification Q1-Q7 on Oxide's own repository (oxide#074).

One-shot, disposable recipe for the single approved session. It runs under a
task-owned scratch Effigy manifest with heavy admission. It is evidence, not
a product launcher or harness.

Arguments: scratch root, evidence directory, caller monotonic ns (process-relative on
this Python, recorded only), session start (wall ns). The caller takes the start before creating the
scratch root, so the 30-minute clock covers every task-owned effect.

Hard counts: one setup Cargo build (both binaries), one lease init, nine
`oxide` invocations, one plain `cargo check`. No retries. A failed step stops
the session and leaves every resource in place for the report.
"""

import hashlib
import json
import os
import signal
import subprocess
import sys
import tempfile
import time

PIN = "dc84d039cd397c8292f37f4201e3916fa2ecd1d3"
PIN_TREE = "448bed9adba200dc4489060637a3bdb05addaddf"
SOURCE = os.path.realpath(os.path.join(os.path.dirname(__file__), "..", "..", "..", ".."))
GIB = 1024**3
IMAGE_BYTES = 4 * GIB
RESERVE_BYTES = 20 * GIB
SESSION_NS = 30 * 60 * 10**9
LOG_LIMIT_BYTES = 32 * 1024 * 1024
CAPS = {"setup-cargo-build": 1, "lease-init": 1, "oxide": 9, "plain-cargo-check": 1}
TOOLCHAIN = os.path.expanduser("~/.rustup/toolchains/1.97.1-aarch64-apple-darwin")

ROOT, EVID = sys.argv[1], sys.argv[2]
START_MONO, START_WALL = int(sys.argv[3]), int(sys.argv[4])
DEADLINE_WALL = START_WALL + SESSION_NS
STAGING = os.path.join(ROOT, "staging")
A = os.path.join(ROOT, "A")
B = os.path.join(ROOT, "B")
POOL = os.path.join(ROOT, "pool")
PAYLOAD = os.path.join(POOL, "payload")
MARKER = "tests/foundation_marker.txt"
counts = {name: 0 for name in CAPS}
sequence = [0]


class Stop(Exception):
    pass


# Python 3.9's monotonic clock is process-relative on macOS, so the session
# clock is the caller's wall-clock start; per-call durations stay monotonic.
def elapsed_s():
    return round((time.time_ns() - START_WALL) / 1e9, 3)


def remaining_s():
    return (DEADLINE_WALL - time.time_ns()) / 1e9


def record(name, entry):
    entry = dict(entry, t_s=elapsed_s())
    with open(os.path.join(EVID, name), "a") as handle:
        handle.write(json.dumps(entry, sort_keys=True) + "\n")


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def base_env(pool=False):
    env = {
        "HOME": os.environ["HOME"],
        "USER": os.environ.get("USER", ""),
        "LOGNAME": os.environ.get("LOGNAME", ""),
        "PATH": f"{os.environ['HOME']}/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "CARGO_HOME": os.environ.get("CARGO_HOME", f"{os.environ['HOME']}/.cargo"),
        "CARGO_NET_OFFLINE": "true",
        "CARGO_TERM_COLOR": "never",
        "OXIDE_TASK_SCRATCH_ROOT": TESTTMP,
        "TMPDIR": TESTTMP,
        "TMP": TESTTMP,
        "TEMP": TESTTMP,
    }
    if pool:
        env["OXIDE_POOL"] = POOL
    return env


def tree_bytes(path):
    """Allocated bytes under path, each inode once. Read-only."""
    if not os.path.lexists(path):
        return None
    seen, total, files = set(), 0, 0
    for top, dirs, names in os.walk(path):
        for name in dirs + names:
            stat = os.lstat(os.path.join(top, name))
            if (stat.st_dev, stat.st_ino) in seen:
                continue
            seen.add((stat.st_dev, stat.st_ino))
            total += stat.st_blocks * 512
            files += 1
    return {"allocated": total, "entries": files}


def fs_sample(path):
    stat = os.statvfs(path)
    return {
        "available": stat.f_bavail * stat.f_frsize,
        "used": (stat.f_blocks - stat.f_bfree) * stat.f_frsize,
        "size": stat.f_blocks * stat.f_frsize,
    }


def mtime_ns(path):
    return os.stat(path).st_mtime_ns if os.path.exists(path) else None


def boundary(label):
    host = fs_sample(ROOT)
    sample = {
        "label": label,
        "host_available": host["available"],
        "remaining_s": round(remaining_s(), 1),
        "marker_mtime_ns": {"A": mtime_ns(os.path.join(A, MARKER)), "B": mtime_ns(os.path.join(B, MARKER))},
        "checkout_target": {"A": tree_bytes(os.path.join(A, "target")), "B": tree_bytes(os.path.join(B, "target"))},
        "staging": tree_bytes(STAGING),
        "test_tmp": tree_bytes(TESTTMP),
        "evidence": tree_bytes(EVID),
    }
    if os.path.ismount(PAYLOAD):
        sample["payload_fs"] = fs_sample(PAYLOAD)
        slots = os.path.join(PAYLOAD, "slots")
        sample["slots"] = {name: tree_bytes(os.path.join(slots, name)) for name in sorted(os.listdir(slots))}
        image = os.path.join(POOL, "image.sparseimage")
        sample["image_backing_allocated"] = os.lstat(image).st_blocks * 512 if os.path.exists(image) else None
        sample["pool_outside_payload"] = {
            name: tree_bytes(os.path.join(POOL, name)) for name in sorted(os.listdir(POOL)) if name != "payload"
        }
    record("boundaries.jsonl", sample)
    if host["available"] < RESERVE_BYTES:
        raise Stop(f"host free {host['available']} below the 20 GiB reserve at {label}")
    if remaining_s() <= 0:
        raise Stop(f"30-minute session deadline passed at {label}")
    return sample


def rows(stdout_path):
    """Cargo compiler-artifact rows and test-harness lines from one stdout."""
    artifacts, results, markers = [], [], []
    with open(stdout_path, errors="replace") as handle:
        for line in handle:
            if line.startswith("{"):
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if message.get("reason") == "compiler-artifact":
                    artifacts.append(
                        {
                            "package": message["package_id"],
                            "target": message["target"]["name"],
                            "kind": message["target"]["kind"],
                            "test": message["profile"]["test"],
                            "fresh": message["fresh"],
                        }
                    )
            elif line.startswith("test result:"):
                results.append(line.strip())
            elif "FOUNDATION-MARKER" in line:
                markers.append(line.strip())
    local = [row for row in artifacts if row["package"].startswith("path+")]
    registry = [row for row in artifacts if not row["package"].startswith("path+")]
    return {
        "local_fresh": sum(row["fresh"] for row in local),
        "local_rebuilt": sum(not row["fresh"] for row in local),
        "registry_fresh": sum(row["fresh"] for row in registry),
        "registry_rebuilt": sum(not row["fresh"] for row in registry),
        "artifacts": artifacts,
        "test_results": results,
        "markers": markers,
    }


def spawn(kind, label, argv, cwd, env):
    if counts[kind] >= CAPS[kind]:
        raise Stop(f"{kind} cap {CAPS[kind]} reached before {label}")
    counts[kind] += 1
    sequence[0] += 1
    stem = os.path.join(EVID, f"{sequence[0]:02d}-{label}")
    record(
        "calls.jsonl",
        {"event": "start", "label": label, "kind": kind, "count": counts[kind], "argv": argv, "cwd": cwd, "env": env},
    )
    out, err = open(stem + ".stdout", "wb"), open(stem + ".stderr", "wb")
    started = time.monotonic_ns()
    process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=out, stderr=err, stdin=subprocess.DEVNULL)
    return process, stem, started, (out, err)


def finish(label, kind, process, stem, started, handles):
    try:
        code = process.wait(timeout=max(remaining_s(), 1))
    except subprocess.TimeoutExpired:
        # Ctrl-C semantics: Oxide forwards it to its own Cargo group.
        process.send_signal(signal.SIGINT)
        code = process.wait(timeout=120)
        record("calls.jsonl", {"event": "deadline-interrupt", "label": label, "exit": code})
        raise Stop(f"deadline reached during {label}")
    finally:
        for handle in handles:
            handle.close()
    seconds = round((time.monotonic_ns() - started) / 1e9, 3)
    entry = {
        "event": "end",
        "label": label,
        "kind": kind,
        "pid": process.pid,
        "exit": code,
        "elapsed_s": seconds,
        "stdout_bytes": os.path.getsize(stem + ".stdout"),
        "stderr_bytes": os.path.getsize(stem + ".stderr"),
    }
    record("calls.jsonl", entry)
    return code, stem


def run(kind, label, argv, cwd, env):
    process, stem, started, handles = spawn(kind, label, argv, cwd, env)
    return finish(label, kind, process, stem, started, handles)


def git(*args, cwd=None):
    result = subprocess.run(
        ["git", *args], cwd=cwd, env=base_env(), capture_output=True, text=True, timeout=120
    )
    record("git.jsonl", {"args": list(args), "cwd": cwd, "exit": result.returncode, "stdout": result.stdout[-2000:], "stderr": result.stderr[-2000:]})
    if result.returncode:
        raise Stop(f"git {' '.join(args)} failed")
    return result.stdout.strip()


def oxide_test(label, cwd):
    argv = [OXIDE, "test", "--locked", "--offline", "--message-format=json", "--", "--show-output"]
    code, stem = run("oxide", label, argv, cwd, base_env(pool=True))
    summary = rows(stem + ".stdout")
    with open(stem + ".stderr", errors="replace") as handle:
        summary["oxide_lines"] = [line.rstrip() for line in handle if line.startswith("oxide:")]
    summary["exit"] = code
    record("scenarios.jsonl", {"label": label, **{k: v for k, v in summary.items() if k != "artifacts"}})
    with open(stem + ".rows.json", "w") as handle:
        json.dump(summary, handle, indent=1, sort_keys=True)
    return code, summary


def oxide_status(label, cwd):
    code, stem = run("oxide", label, [OXIDE, "status"], cwd, base_env(pool=True))
    with open(stem + ".stdout", errors="replace") as handle:
        text = handle.read()
    record("scenarios.jsonl", {"label": label, "exit": code, "status": text})
    return code, text


def ps(pids):
    if not pids:
        return ""
    result = subprocess.run(
        ["/bin/ps", "-o", "pid,ppid,pgid,stat,command", "-p", ",".join(map(str, pids))],
        capture_output=True, text=True, timeout=30,
    )
    return result.stdout


def children(pid):
    result = subprocess.run(["/usr/bin/pgrep", "-P", str(pid)], capture_output=True, text=True, timeout=30)
    return [int(line) for line in result.stdout.split()]


def group_alive(pgid):
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False


def stage():
    git("clone", "--quiet", "--no-local", "--no-checkout", SOURCE, STAGING)
    git("checkout", "--quiet", "--detach", PIN, cwd=STAGING)
    head, tree = git("rev-parse", "HEAD", cwd=STAGING), git("rev-parse", "HEAD^{tree}", cwd=STAGING)
    if (head, tree) != (PIN, PIN_TREE) or git("status", "--porcelain", cwd=STAGING):
        raise Stop("staging checkout is not the pristine pin")
    argv = ["cargo", "build", "--release", "--locked", "--offline", "--bin", "oxide", "--bin", "oxide-storage-lease"]
    code, _ = run("setup-cargo-build", "setup-build", argv, STAGING, base_env())
    if code:
        raise Stop(f"setup build exited {code}")
    binaries = {}
    for name in ("oxide", "oxide-storage-lease"):
        path = os.path.join(STAGING, "target", "release", name)
        binaries[name] = {"path": path, "sha256": sha256(path), "bytes": os.path.getsize(path)}
    toolchain = {
        name: sha256(os.path.join(TOOLCHAIN, "bin", name)) for name in ("cargo", "rustc")
    }
    record("session.jsonl", {"event": "staged", "commit": head, "tree": tree, "binaries": binaries, "toolchain_sha256": toolchain, "toolchain": TOOLCHAIN})
    return binaries


MARKER_TEST = r'''//! Disposable oxide#074 foundation-qualification marker. Never merged.
//!
//! The checkout path and marker text are compiled in; the runtime values come
//! from the checkout the test runs in. A test binary compiled in another
//! checkout, or before the marker last changed, disagrees and fails.

use std::path::Path;

const COMPILED_DIR: &str = env!("CARGO_MANIFEST_DIR");
const COMPILED_MARKER: &str = include_str!("foundation_marker.txt");

#[test]
fn foundation_marker_matches_this_checkout() {
    let runtime_dir = std::env::var("CARGO_MANIFEST_DIR").expect("runtime CARGO_MANIFEST_DIR");
    let cwd = std::env::current_dir().expect("current directory");
    let runtime_marker = std::fs::read_to_string(Path::new(&runtime_dir).join("tests/foundation_marker.txt"))
        .expect("runtime marker");
    println!(
        "FOUNDATION-MARKER compiled_dir={COMPILED_DIR} runtime_dir={runtime_dir} cwd={} compiled={:?} runtime={:?}",
        cwd.display(),
        COMPILED_MARKER.trim_end(),
        runtime_marker.trim_end()
    );
    assert_eq!(COMPILED_DIR, runtime_dir, "compiled in another checkout");
    assert_eq!(
        Path::new(COMPILED_DIR).canonicalize().expect("compiled dir"),
        cwd.canonicalize().expect("cwd"),
        "compiled in another checkout"
    );
    assert_eq!(COMPILED_MARKER, runtime_marker, "compiled from other marker content");
}
'''


def checkouts():
    git("clone", "--quiet", "--no-local", "--no-checkout", SOURCE, A)
    git("checkout", "--quiet", "-b", "foundation-qual", PIN, cwd=A)
    with open(os.path.join(A, "tests", "foundation_marker.rs"), "w") as handle:
        handle.write(MARKER_TEST)
    with open(os.path.join(A, MARKER), "w") as handle:
        handle.write("original marker\n")
    git("add", "tests/foundation_marker.rs", MARKER, cwd=A)
    git("-c", "user.name=oxide074", "-c", "user.email=oxide074@invalid", "-c", "commit.gpgsign=false",
        "-c", "core.hooksPath=/dev/null",
        "commit", "--quiet", "-m", "Disposable foundation-qualification marker", cwd=A)
    with open(os.path.join(EVID, "marker.patch"), "w") as handle:
        handle.write(git("format-patch", "--stdout", "-1", "HEAD", cwd=A) + "\n")
    git("worktree", "add", "--quiet", "--detach", B, "foundation-qual", cwd=A)
    for checkout in (A, B):
        if git("status", "--porcelain", cwd=checkout):
            raise Stop(f"{checkout} is not clean after setup")
    record("session.jsonl", {"event": "checkouts", "marker_commit": git("rev-parse", "HEAD", cwd=A), "B_head": git("rev-parse", "HEAD", cwd=B)})


def q5(results):
    argv = [OXIDE, "test", "--locked", "--offline", "--message-format=json", "--", "--show-output"]
    process, stem, started, handles = spawn("oxide", "q5a-test-A-killed", argv, A, base_env(pool=True))
    compiling = False
    while time.monotonic_ns() - started < 120 * 10**9 and process.poll() is None:
        with open(stem + ".stderr", errors="replace") as handle:
            if "Compiling oxide-storage" in handle.read():
                compiling = True
                break
        time.sleep(0.05)
    kids = children(process.pid)
    table = ps([process.pid, *kids])
    groups = set()
    for line in table.splitlines()[1:]:
        fields = line.split()
        if int(fields[0]) in kids:
            groups.add(int(fields[2]))
    record("session.jsonl", {"event": "q5-before-kill", "compiling_seen": compiling, "oxide_pid": process.pid, "children": kids, "groups": sorted(groups), "ps": table})
    if not compiling or process.poll() is not None:
        for handle in handles:
            handle.close()
        process.wait(timeout=max(remaining_s(), 1))
        raise Stop("Q5: Cargo was not seen compiling before the kill window")
    os.kill(process.pid, signal.SIGKILL)  # The qualified owned oxide process only.
    code = process.wait(timeout=30)
    for handle in handles:
        handle.close()
    alive = {group: group_alive(group) for group in groups}
    record("calls.jsonl", {"event": "end", "label": "q5a-test-A-killed", "kind": "oxide", "pid": process.pid, "exit": code, "elapsed_s": round((time.monotonic_ns() - started) / 1e9, 3)})
    record("session.jsonl", {"event": "q5-after-kill", "exit": code, "groups_alive": alive, "ps": ps(kids)})
    code, summary = oxide_test("q5b-test-A-retry", A)
    after = {group: group_alive(group) for group in groups}
    record("session.jsonl", {"event": "q5-after-retry", "groups_alive": after})
    results["Q5"] = {"killed_exit": process.returncode, "groups_alive_at_kill": alive, "groups_alive_after": after, "retry": summary["exit"]}
    if any(after.values()):
        raise Stop("Q5: the killed command's Cargo group survived the retry")
    killed_rows = rows(stem + ".stdout")
    with open(stem + ".rows.json", "w") as handle:
        json.dump(killed_rows, handle, indent=1, sort_keys=True)
    return code


def main():
    global OXIDE, LEASE
    os.makedirs(EVID, exist_ok=True)
    record("session.jsonl", {"event": "start", "start_wall_ns": START_WALL, "deadline_wall_ns": START_WALL + SESSION_NS, "root": ROOT, "test_tmp": TESTTMP, "recipe_sha256": sha256(__file__), "admitted_at_s": elapsed_s(), "pin": PIN, "source": SOURCE, "caps": CAPS})
    results = {}
    try:
        boundary("preflight")
        binaries = stage()
        OXIDE, LEASE = binaries["oxide"]["path"], binaries["oxide-storage-lease"]["path"]
        boundary("staged")
        code, stem = run("lease-init", "pool-init", [LEASE, "--pool", POOL, "init", "--cap-bytes", str(IMAGE_BYTES), "--image-bytes", str(IMAGE_BYTES)], ROOT, base_env())
        if code:
            raise Stop(f"lease init exited {code}")
        payload = fs_sample(PAYLOAD)
        if payload["size"] > IMAGE_BYTES:
            raise Stop("payload volume exceeds the 4 GiB ceiling")
        with open(os.path.join(POOL, "config.json")) as handle:
            config = json.load(handle)
        record("session.jsonl", {"event": "pool", "config": config, "payload_fs": payload, "receipt_present": os.path.exists(os.path.join(POOL, "receipt.json"))})
        checkouts()
        boundary("checkouts")

        results["Q1"] = oxide_test("q1-test-A", A)[0]
        boundary("after-Q1")
        results["Q2"] = oxide_test("q2-test-B", B)[0]
        boundary("after-Q2")
        with open(os.path.join(A, MARKER), "w") as handle:
            handle.write("edited in A\n")
        boundary("Q3-edited-A")
        results["Q3a"] = oxide_test("q3a-test-A", A)[0]
        boundary("after-Q3a")
        results["Q3b"] = oxide_test("q3b-test-B", B)[0]
        boundary("after-Q3b")
        results["Q4"] = oxide_test("q4-test-B", B)[0]
        boundary("after-Q4")
        results["Q5b"] = q5(results)
        boundary("after-Q5")
        argv = ["cargo", "check", "--offline", "--locked", "--message-format=json"]
        code, stem = run("plain-cargo-check", "q6a-plain-check-B", argv, B, base_env())
        check_rows = rows(stem + ".stdout")
        with open(stem + ".rows.json", "w") as handle:
            json.dump(check_rows, handle, indent=1, sort_keys=True)
        results["Q6check"] = code
        boundary("after-Q6-check")
        results["Q6status"] = oxide_status("q6b-status-B", B)[0]
        boundary("after-Q6")
        code, text = oxide_status("q7-status-A", A)
        results["Q7"] = code
        boundary("after-Q7")
        teardown(code, text)
    except Stop as stop:
        record("session.jsonl", {"event": "stop", "reason": str(stop), "counts": counts, "results": results})
        print(f"session stopped: {stop}", file=sys.stderr)
        return 2
    finally:
        record("session.jsonl", {"event": "end", "counts": counts, "results": results, "elapsed_s": elapsed_s()})
    return 0


def teardown(code, text):
    lines = [line.strip() for line in text.splitlines()]
    slot_lines = [line for line in lines if line.startswith("slot ")]
    settled = (
        code == 0
        and "slots {}, waiting commands 0".format(len(slot_lines)) in lines
        and all(line.split()[2] == "idle" for line in slot_lines)
        and not any(line.startswith("cleaning ") for line in lines)
        and any("slot accounting Complete" in line for line in lines)
    )
    writers = subprocess.run(["/usr/sbin/lsof", "-n", "-t", PAYLOAD], capture_output=True, text=True, timeout=60)
    record("session.jsonl", {"event": "teardown-check", "settled": settled, "lsof_exit": writers.returncode, "lsof_pids": writers.stdout.split()})
    if not settled or writers.stdout.strip():
        raise Stop("pool not positively settled; preserved for the report")
    final = boundary("before-detach")
    detach = subprocess.run(["/usr/bin/hdiutil", "detach", PAYLOAD], capture_output=True, text=True, timeout=120)
    record("session.jsonl", {"event": "detach", "exit": detach.returncode, "stdout": detach.stdout, "stderr": detach.stderr})
    if detach.returncode:
        raise Stop("detach failed; pool preserved")
    owned = tree_bytes(ROOT)
    record("session.jsonl", {"event": "owned-before-removal", "scratch_root": owned, "test_tmp": tree_bytes(TESTTMP), "image_allocated": final.get("image_backing_allocated")})
    subprocess.run(["/bin/rm", "-rf", ROOT, TESTTMP], check=True, timeout=300)
    record("session.jsonl", {"event": "removed", "root_exists": os.path.exists(ROOT), "test_tmp_exists": os.path.exists(TESTTMP)})


if __name__ == "__main__":
    TESTTMP = os.path.realpath(tempfile.mkdtemp(prefix="oxide-fq-", dir="/private/tmp"))
    sys.exit(main())
