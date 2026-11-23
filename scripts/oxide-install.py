"""Build `oxide` from a pushed-main pin and install it at ~/.local/bin/oxide.

Initial mode refuses any `oxide` on PATH and any existing destination entry.
`--replace` swaps only the regular, user-owned binary whose SHA-256 matches
the newest install receipt. A replacement saves that binary beside its
receipt. `--rollback` restores the verified prior binary without building.
`--check` stops after the preflight and builds nothing.
`--canary` builds the same way and tests the staged binary on the live pool.
It refuses when the staged control-state version differs from the installed
one. `--canary --scratch-root DIR` instead runs against a fresh pool under DIR
with no live-version check and no live contact. `--canary --format-change`
upgrades a read-only copy of the live control and ledger documents, for a
format-changing build, and never writes the live pool.
`--require-canary` requires a fresh PASS for the same pin, whatever the mode.
`OXIDE_INSTALL_PIN` also names the pin.

The receipt records the pinned build's own `control_state_version` and
`registration_version` capabilities, read by an effect-free probe of the
built binary (oxide#122). A build that cannot report them is refused before
anything is installed, so no receipt wrongly claims a hand-off target.

Every build is stamped with the authenticated source pin and a fresh
128-bit installer instance (oxide#130), so `oxide --version` can name the
source it came from and prove the running binary is this receipt's. The
stamp is verified in the built binary before installation; a receipt without
one could not be authenticated.
"""
import argparse
import datetime
import fcntl
import hashlib
import json
import math
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from contextlib import ExitStack, contextmanager
from pathlib import Path

HOME = Path.home()
DESTINATION = HOME / ".local/bin/oxide"
RECEIPTS = HOME / ".local/state/oxide/install"
BUILD = ["cargo", "build", "--release", "--locked", "--offline", "--bin", "oxide"]
CAPABILITIES_ARGUMENT = "__oxide-build-capabilities"
IDENTITY_ARGUMENT = "__oxide-build-identity"
BUILD_PIN_VARIABLE = "OXIDE_BUILD_PIN"
BUILD_INSTANCE_VARIABLE = "OXIDE_BUILD_INSTANCE"
REPOSITORY = Path(__file__).resolve().parent.parent
REMOTE_MAIN = "refs/heads/main"
PRIVATE_REF_PREFIX = "refs/oxide-install-auth/"
RECEIPT_VERSION = 2
RECEIPT_MAX_BYTES = 64 * 1024
DIGEST = re.compile(r"[a-f0-9]{64}")
PIN = re.compile(r"[a-f0-9]{40}")
# Every Git command here is local and small; a bounded wait keeps a wedged
# index, hook or credential prompt from holding the install lock forever.
GIT_SECONDS = 120


class Refusal(Exception):
    pass


def git(*args):
    """Run one bounded Git command in this repository.

    A command that exceeds `GIT_SECONDS` refuses instead of surfacing a raw
    `TimeoutExpired` traceback: the install lock is held around this, so a
    hung reader must end as a reportable refusal.
    """
    try:
        return subprocess.run(
            ["git", "-C", str(REPOSITORY), *args], check=True, capture_output=True,
            text=True, timeout=GIT_SECONDS,
        ).stdout.strip()
    except subprocess.TimeoutExpired as error:
        raise Refusal(f"git {' '.join(args)} exceeded {GIT_SECONDS}s: {error}") from error


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def verify_commit(revision):
    """The commit id `revision` names, or a refusal.

    Missing, malformed and non-commit objects refuse here, so nothing
    downstream decides from an object this run has not authenticated.
    """
    result = subprocess.run(
        ["git", "-C", str(REPOSITORY), "rev-parse", "--verify", revision],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise Refusal(
            f"{revision} is not a commit this run can authenticate: {result.stderr.strip()}"
        )
    return result.stdout.strip()


def authenticate_pin(requested, interlude=None):
    """Return (pin, tree, remote tip); the pin must be reachable from origin main.

    The fetch lands the advertised main tip in a private per-invocation ref
    under `refs/oxide-install-auth/`, deleted again before this returns, and
    every decision below reads that ref: shared `FETCH_HEAD` and the mutable
    remote-tracking refs are never consulted, so concurrent fetch activity
    cannot change the source identity this run decides from. The fetched
    object is accepted only as a commit whose id equals the advertised tip,
    so a main that advanced between the advertisement and the fetch, or a
    tip that is not a commit, refuses and the install can simply be retried.
    `interlude(stage)` is the dogfood suite's deterministic seam, called with
    "advertised" between reading the advertisement and fetching, and with
    "fetched" immediately after the fetch completes and BEFORE this run
    decides anything, so a regression that decided from shared `FETCH_HEAD`
    would observe exactly the concurrent interleaving at its deciding read.
    Production never passes one.
    """
    ref = PRIVATE_REF_PREFIX + secrets.token_hex(16)
    try:
        try:
            listing = git("ls-remote", "origin", REMOTE_MAIN).split()
        except subprocess.CalledProcessError as error:
            raise Refusal(f"cannot list origin {REMOTE_MAIN}: {error}")
        if len(listing) != 2 or listing[1] != REMOTE_MAIN:
            raise Refusal(f"origin {REMOTE_MAIN} not found")
        tip = listing[0]
        if interlude is not None:
            interlude("advertised")
        try:
            git("fetch", "--quiet", "origin", f"{REMOTE_MAIN}:{ref}")
        except subprocess.CalledProcessError as error:
            raise Refusal(f"fetching {REMOTE_MAIN} from origin failed: {error}")
        if interlude is not None:
            interlude("fetched")
        fetched = verify_commit(f"{ref}^{{commit}}")
        if fetched != tip:
            raise Refusal("fetched main does not match ls-remote; retry")
        pin = verify_commit(f"{requested or tip}^{{commit}}")
        if subprocess.run(
            ["git", "-C", str(REPOSITORY), "merge-base", "--is-ancestor", pin, tip]
        ).returncode != 0:
            raise Refusal(f"{pin} is not on pushed origin main ({tip})")
        return pin, git("rev-parse", f"{pin}^{{tree}}"), tip
    finally:
        # Narrow cleanup of this invocation's own ref. A ref that still exists
        # but cannot be removed refuses the run rather than polluting the
        # repository; a ref the failed body never created is simply absent.
        present = subprocess.run(
            ["git", "-C", str(REPOSITORY), "rev-parse", "--verify", "--quiet", ref],
            capture_output=True, text=True,
        )
        if present.returncode == 0:
            removed = subprocess.run(
                ["git", "-C", str(REPOSITORY), "update-ref", "-d", ref],
                capture_output=True, text=True,
            )
            if removed.returncode != 0:
                raise Refusal(f"cannot remove the private ref {ref}: {removed.stderr.strip()}")


def path_entries():
    """Every `oxide` entry in a PATH directory, links included."""
    found = []
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(os.path.abspath(Path(directory or ".") / "oxide"))
        if os.path.lexists(candidate) and candidate not in found:
            found.append(candidate)
    return found


def newest_receipt():
    try:
        directory = open_owned_directory(RECEIPTS)
    except FileNotFoundError:
        return None
    try:
        names = sorted(name for name in os.listdir(directory) if name.endswith(".json"))
        return read_receipt(names[-1], directory) if names else None
    finally:
        os.close(directory)


def read_receipt(name, directory):
    if not isinstance(name, str) or Path(name).name != name or not name.endswith(".json"):
        raise Refusal("invalid prior receipt name")
    path = RECEIPTS / name
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    with os.fdopen(os.open(name, flags, dir_fd=directory), "rb") as file:
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o022:
            raise Refusal(f"{path} is not a regular receipt writable only by this user")
        data = file.read(RECEIPT_MAX_BYTES + 1)
    if len(data) > RECEIPT_MAX_BYTES:
        raise Refusal(f"{path} exceeds the receipt byte limit")
    try:
        receipt = json.loads(data)
        installed = receipt["installed"]
        version = receipt.get("version", 1)
        valid = (
            type(version) is int and version in (1, RECEIPT_VERSION)
            and (version == 1 or not info.st_mode & 0o077)
            and installed["path"] == str(DESTINATION)
            and DIGEST.fullmatch(installed["sha256"])
        )
    except (KeyError, TypeError, ValueError) as error:
        raise Refusal(f"{path} is an invalid install receipt") from error
    if not valid:
        raise Refusal(f"{path} is an invalid install receipt")
    return path, receipt, hashlib.sha256(data).hexdigest()


def open_owned_directory(path, *, create=False):
    """Open a directory after refusing any link or foreign component.

    Every component from HOME down must be a real directory owned by this
    user, and none below HOME may be group or world writable.
    """
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    descriptor = os.open(HOME, flags)
    try:
        if os.fstat(descriptor).st_uid != os.getuid():
            raise Refusal(f"{HOME} is not owned by this user")
        for part in path.relative_to(HOME).parts:
            if create:
                try:
                    os.mkdir(part, 0o700, dir_fd=descriptor)
                except FileExistsError:
                    pass
            child = os.open(part, flags, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
            info = os.fstat(descriptor)
            if info.st_uid != os.getuid() or info.st_mode & 0o022:
                raise Refusal(f"{path} has a directory writable by another user")
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def open_destination_directory():
    try:
        return open_owned_directory(DESTINATION.parent)
    except OSError as error:
        raise Refusal(f"cannot open the installation directory: {error}") from error


@contextmanager
def receipt_lock(*, create=True):
    directory = open_owned_directory(RECEIPTS, create=create)
    try:
        flags = os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK
        if create:
            flags |= os.O_CREAT
        with os.fdopen(os.open(".lock", flags, 0o600, dir_fd=directory), "r+b") as lock:
            info = os.fstat(lock.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise Refusal("the install lock must be an owner-only regular file")
            fcntl.flock(lock, fcntl.LOCK_EX)
            yield directory
    finally:
        os.close(directory)


def identity(directory, name, *, private=False):
    """(device, inode, SHA-256) of a regular user-owned entry, or None if absent.

    Links, other file types and foreign owners refuse.
    """
    try:
        info = os.stat(name, dir_fd=directory, follow_symlinks=False)
    except FileNotFoundError:
        return None
    if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
            or private and stat.S_IMODE(info.st_mode) != 0o700):
        raise Refusal(f"{name} is not a regular file with the required owner and mode")
    descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    with os.fdopen(descriptor, "rb") as file:
        opened = os.fstat(file.fileno())
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            raise Refusal(f"{name} changed while it was read")
        digest = hashlib.sha256()
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return info.st_dev, info.st_ino, digest.hexdigest()


def preflight(replace):
    """Return (replaced receipt, expected destination identity), or Nones in initial mode."""
    entries = path_entries()
    if not replace:
        if entries:
            raise Refusal("oxide already on PATH: " + ", ".join(map(str, entries)))
        if os.path.lexists(DESTINATION):
            raise Refusal(f"{DESTINATION} already exists")
        os.close(open_destination_directory())
        return None, None
    others = [entry for entry in entries if entry != DESTINATION]
    if others:
        raise Refusal("another oxide on PATH: " + ", ".join(map(str, others)))
    previous = newest_receipt()
    if previous is None:
        raise Refusal(f"no install receipt under {RECEIPTS}; use initial mode")
    directory = open_destination_directory()
    try:
        expected = identity(directory, DESTINATION.name)
    finally:
        os.close(directory)
    if expected is None or expected[2] != previous[1]["installed"]["sha256"]:
        raise Refusal(f"{DESTINATION} does not match receipt {previous[0].name}")
    return previous, expected


def new_instance():
    """A fresh 128-bit lowercase-hex nonce for exactly this build."""
    return secrets.token_hex(16)


def build_environment(staging, pin, instance):
    """The build's environment: private target, authenticated pin, fresh instance.

    Inherited `CARGO_TARGET_*` names are dropped so the build cannot write to
    a caller's storage. The stamp names are read by `option_env!`, which
    Cargo tracks, so a changed stamp recompiles the binary.
    """
    environment = {k: v for k, v in os.environ.items() if not k.startswith("CARGO_TARGET_")}
    environment["CARGO_TARGET_DIR"] = str(staging / "target")
    environment[BUILD_PIN_VARIABLE] = pin
    environment[BUILD_INSTANCE_VARIABLE] = instance
    return environment


def build(pin, instance, staging):
    source = staging / "source"
    source.mkdir()
    # The context manager owns the reader: it closes the parent's copy of the
    # pipe and reaps the process on every path, so a refused `tar` cannot leave
    # an archive writer running or a child waiting on a pipe this run holds.
    with subprocess.Popen(
        ["git", "-C", str(REPOSITORY), "archive", pin], stdout=subprocess.PIPE
    ) as archive:
        try:
            subprocess.run(["tar", "-x", "-C", str(source)], stdin=archive.stdout, check=True)
        except BaseException:
            archive.terminate()
            raise
        finally:
            archive.stdout.close()
        if archive.wait() != 0:
            raise Refusal("git archive failed")
    environment = build_environment(staging, pin, instance)
    versions = {
        tool: subprocess.run(
            [tool, "-vV" if tool == "rustc" else "-V"],
            cwd=source, env=environment, check=True, capture_output=True, text=True,
        ).stdout.strip()
        for tool in ("rustc", "cargo")
    }
    subprocess.run(BUILD, cwd=source, env=environment, check=True)
    return staging / "target/release/oxide", versions


def state_capabilities(built):
    """The built binary's own control-state and registration versions.

    The probe is effect-free and runs before installation. A pin that cannot
    report them is refused: a receipt without capabilities cannot prove a
    hand-off target, and this script never guesses from host settings.
    """
    try:
        result = subprocess.run(
            [str(built), CAPABILITIES_ARGUMENT], check=True, capture_output=True, text=True, timeout=10
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise Refusal(f"the built oxide cannot report its state capabilities: {error}")
    try:
        reported = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise Refusal(f"the built oxide printed no capability JSON: {error}")
    values = {}
    for name in ("control_state_version", "registration_version"):
        value = reported.get(name) if isinstance(reported, dict) else None
        if type(value) is not int or not 0 <= value <= 0xFFFFFFFF:
            raise Refusal(f"the built oxide reported no valid {name}")
        values[name] = value
    for name in ("ledger_version", "writer_generation"):
        value = reported.get(name)
        if value is None and values["control_state_version"] < 10 and name not in reported:
            continue  # Historical pinned builds predate these two capabilities.
        if type(value) is not int or not 1 <= value <= 0xFFFFFFFF:
            raise Refusal(f"the built oxide reported no valid {name}")
        values[name] = value
    # An artifact that honours the debug-only proof controls (OXIDE_TEST_*)
    # could be steered off the account's pool; it is never installed.
    controls = reported.get("test_controls") if isinstance(reported, dict) else None
    if controls is not False:
        raise Refusal(
            "the built oxide reports proof controls compiled in (or no test_controls answer); "
            "build a release artifact without debug assertions"
        )
    return values


def build_identity(built, pin, instance):
    """The stamp the built binary actually carries, or a refusal.

    This runs the built binary's own effect-free probe, so a build that missed
    its stamp or kept a stale one can never be installed with a receipt that
    claims it.
    """
    try:
        result = subprocess.run(
            [str(built), IDENTITY_ARGUMENT], check=True, capture_output=True, text=True, timeout=10
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise Refusal(f"the built oxide cannot report its build identity: {error}")
    try:
        reported = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise Refusal(f"the built oxide printed no build identity JSON: {error}")
    if not isinstance(reported, dict):
        raise Refusal("the built oxide printed no build identity object")
    for name, expected in (("build_pin", pin), ("build_instance", instance)):
        if reported.get(name) != expected:
            raise Refusal(f"the built oxide's {name} does not match this build")
    return reported


def temporary_name():
    return f".oxide.install.{os.getpid()}.{secrets.token_hex(8)}"


def write_private(directory, name, mode, write):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    with os.fdopen(os.open(name, flags, mode, dir_fd=directory), "wb") as output:
        write(output)
        output.flush()
        os.fchmod(output.fileno(), mode)
        os.fsync(output.fileno())


def write_canary_receipt(pin, result, mode="live"):
    directory = open_owned_directory(RECEIPTS / "canary", create=True)
    temporary = temporary_name()
    try:
        receipt = {"pin": pin, "result": result, "mode": mode,
                   "finished": datetime.datetime.now(datetime.timezone.utc).isoformat()}
        write_private(directory, temporary, 0o600,
                      lambda output: output.write((json.dumps(receipt) + "\n").encode()))
        os.replace(temporary, f"{pin}.json", src_dir_fd=directory, dst_dir_fd=directory)
        os.fsync(directory)
    finally:
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
        os.close(directory)


def require_canary(pin):
    try:
        directory = open_owned_directory(RECEIPTS / "canary")
        try:
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            with os.fdopen(os.open(f"{pin}.json", flags, dir_fd=directory), "rb") as file:
                info = os.fstat(file.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                    raise ValueError("receipt must be an owner-only regular file")
                data = file.read(RECEIPT_MAX_BYTES + 1)
        finally:
            os.close(directory)
        if len(data) > RECEIPT_MAX_BYTES:
            raise ValueError("receipt exceeds the byte limit")
        receipt = json.loads(data)
        finished = datetime.datetime.fromisoformat(receipt["finished"])
        age = datetime.datetime.now(datetime.timezone.utc) - finished
        if (receipt["pin"] == pin and receipt["result"] == "PASS"
                and datetime.timedelta(0) <= age < datetime.timedelta(hours=1)):
            return
        raise ValueError("receipt must record a PASS less than an hour old")
    except (KeyError, TypeError, ValueError, OSError) as error:
        raise Refusal(f"no fresh canary PASS for {pin}; run effigy oxide:canary -- --pin {pin} ({error})") from error


def canary_compatibility(candidate):
    # Admission can publish writer fences. Keep the installed writer usable
    # and leave format transitions to the planner's compatibility window.
    if path_entries() or os.path.lexists(DESTINATION):
        previous, _ = preflight(True)
        for name, version in candidate.items():
            installed = previous[1].get(name)
            if type(installed) is not int or version != installed:
                raise Refusal(f"live canary needs matching {name}: installed {installed}, staged {version}")


def verify_scratch_dir(path, expected_dev=None):
    """Validate a scratch directory through one no-follow descriptor.

    Ownership, mode and volume facts come from an `fstat` of the opened
    directory rather than a second path lookup, so a path swapped between
    checks cannot be validated as one directory and used as another. Returns
    the directory's `(device, inode)` identity, the same pair
    `directory_identity` reports.
    """
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        raise Refusal(
            f"scratch must be a directory this run can open without following links: {path} ({error})"
        )
    try:
        info = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if info.st_uid != os.getuid():
        raise Refusal(f"scratch must be owned by the user running the installer: {path}")
    if info.st_mode & 0o022:
        raise Refusal(f"scratch must be privately writable: {path}")
    if expected_dev is not None and info.st_dev != expected_dev:
        raise Refusal(f"scratch must be on the selected pool's volume: {path}")
    return info.st_dev, info.st_ino


def _removable_payload_on_volume(payload, mount, device):
    """Check a registered removable payload names a path on the mounted volume.

    Flat (`<mount>/oxide-pool`) and nested (`<mount>/builds/oxide/pool`)
    layouts are both valid: the payload must be absolute, strictly beneath
    `mount`, and free of `..` components, and its canonical location must
    stay on the mount's device strictly beneath `mount` (so a symlink
    escaping the volume is refused). The payload exists on a live
    registered pool, so it resolves strictly; a missing payload falls back
    to its nearest existing ancestor's canonical location.
    """
    if not payload.is_absolute() or payload == mount or not payload.is_relative_to(mount):
        return False
    if ".." in payload.parts:
        return False
    anchor = payload
    while not anchor.exists():
        if anchor == anchor.parent:
            return False
        anchor = anchor.parent
    try:
        resolved_anchor = anchor.resolve(strict=True)
    except OSError:
        return False
    resolved = resolved_anchor / payload.relative_to(anchor)
    if resolved == mount or not resolved.is_relative_to(mount):
        return False
    try:
        return anchor.stat().st_dev == device
    except OSError:
        return False


def canary_scratch(description, override):
    if description["schema"] != "oxide.pool.describe/1":
        raise Refusal("the staged binary returned an unknown pool description")
    pool = Path(description["internal"]["pool"]).resolve(strict=True)
    if not (pool / "config.json").is_file():
        raise Refusal(f"no existing live pool at {pool}")
    removable = description["removable"]
    payload = None
    if description["selected"] == "removable" and removable["available"] is True:
        if override is not None:
            raise Refusal("the removable pool uses its registered volume's scratch directory")
        mount = Path(removable["mount_point"])
        payload = Path(removable["payload"])
        scratch = Path(removable["suggested_scratch_root"])
        device = mount.stat().st_dev
        if (not mount.is_absolute() or mount.resolve(strict=True) != mount
                or device == mount.parent.stat().st_dev
                or scratch != mount / "scratch"
                or not _removable_payload_on_volume(payload, mount, device)):
            raise Refusal("removable scratch does not name the registered mounted volume")
    elif description["selected"] == "internal" and (
        removable is None or removable.get("unavailable_reason") == "absent"
    ):
        scratch = override or pool.parent / "scratch"
        device = pool.stat().st_dev
    else:
        raise Refusal("the staged binary could not identify an eligible live pool")
    if not scratch.is_absolute() or scratch.resolve(strict=True) != scratch or scratch == scratch.parent:
        raise Refusal(f"scratch must be an existing canonical directory: {scratch}")
    if scratch == pool or pool in scratch.parents or scratch in pool.parents:
        raise Refusal(f"scratch must be outside the pool and must not contain it: {scratch}")
    return pool, payload, scratch, verify_scratch_dir(scratch, device)


def format_canary_scratch(description, override):
    """Scratch for `--format-change`: always on the internal pool's volume.

    The copy is an explicit internal pool (`OXIDE_POOL`), so it must live on
    the internal pool's volume even when a removable pool is registered: the
    staged binary refuses an explicit internal pool on an external volume.
    An explicit `--scratch-root` is honored here (unlike the live canary's
    removable refusal) because only a copy of the live documents is exercised.
    """
    if description["schema"] != "oxide.pool.describe/1":
        raise Refusal("the staged binary returned an unknown pool description")
    pool = Path(description["internal"]["pool"]).resolve(strict=True)
    if not (pool / "config.json").is_file():
        raise Refusal(f"no existing live pool at {pool}")
    scratch = override or pool.parent / "scratch"
    if not scratch.is_absolute() or scratch.resolve(strict=True) != scratch or scratch == scratch.parent:
        raise Refusal(f"scratch must be an existing canonical directory: {scratch}")
    if scratch == pool or pool in scratch.parents or scratch in pool.parents:
        raise Refusal(f"scratch must be outside the pool and must not contain it: {scratch}")
    return pool, scratch, verify_scratch_dir(scratch, os.stat(pool).st_dev)


def directory_identity(path):
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode):
        raise Refusal(f"{path} is no longer a real directory")
    return info.st_dev, info.st_ino


def canary_check(built, crate, environment):
    reader, writer = os.pipe()
    try:
        os.set_blocking(writer, False)
        # A surviving descendant must not tie up the reader via an output pipe.
        with tempfile.TemporaryFile(mode="w+t", encoding="utf-8", errors="replace") as output:
            started = time.monotonic()
            result = subprocess.run(
                [str(built), "check", "--admission-budget", "300s", "--execution-budget", "60s",
                 "--budget-events", str(writer), "--offline"],
                cwd=crate, env=environment, pass_fds=(writer,), stderr=output,
            )
            wall = time.monotonic() - started
            output.seek(0)
            result.stderr = output.read()
        # One start and one terminal record fit in the pipe. Read after exit
        # without waiting for EOF or a descriptor held by another process.
        os.set_blocking(reader, False)
        try:
            events = [json.loads(line) for line in os.read(reader, 4096).splitlines()]
        except (ValueError, BlockingIOError):
            events = []
        return result, wall, events
    finally:
        os.close(writer)
        os.close(reader)


def assess_canary(label, result, wall, events, expected, payload, limit):
    print(result.stderr, end="", file=sys.stderr)
    print(f"{label}: wall {wall:.3f}s")
    queued = re.findall(r"^oxide: admission queue wait ([0-9]{1,20}) ns$", result.stderr, re.MULTILINE)
    if len(queued) > 1:
        raise Refusal(f"{label}: ambiguous queue wait")
    queue_ns = int(queued[0]) if queued else 0
    if queued:
        print(f"{label}: queue wait {queue_ns / 1_000_000_000:.3f}s")
    else:
        print(f"{label}: precise queue wait unavailable; no wait deducted")
    if result.returncode != 0:
        raise Refusal(f"{label}: staged check exited {result.returncode}")
    if len(events) != 2 or not all(isinstance(event, dict) for event in events):
        raise Refusal(f"{label}: incomplete admission telemetry")
    start, finish = events
    admission_ms = start.get("admission_elapsed_ms")
    if (start.get("v") != 1 or finish.get("v") != 1 or start.get("seq") != 1 or finish.get("seq") != 2
            or not start.get("invocation") or start["invocation"] != finish.get("invocation")
            or start.get("event") != "admitted_child_started" or start.get("phase") != "primary_cargo"
            or start.get("spawn_late") is not False or type(admission_ms) is not int or admission_ms < 0
            or finish.get("event") != "finished" or finish.get("outcome") != "child_exit"
            or finish.get("exit_code") != 0 or finish.get("drain") != "complete"
            or finish.get("lease") != "released"):
        raise Refusal(f"{label}: staged check did not report a complete, released admission")
    elapsed_ns = admission_ms * 1_000_000
    if queue_ns >= elapsed_ns + 1_000_000:
        raise Refusal(f"{label}: queue wait exceeds pre-child time")
    admission = max(0, elapsed_ns - queue_ns) / 1_000_000_000
    print(f"{label}: admission {admission:.3f}s excluding queue wait (limit {limit:g}s)")
    slots = list(re.finditer(
        r"^oxide: .* profile, build strategy, slot (\S+) (cold|warm), .*?$", result.stderr, re.MULTILINE
    ))
    if len(slots) != 1:
        raise Refusal(f"{label}: staged check did not report exactly one slot")
    slot = slots[0]
    marker = ", budget-only, removable "
    correct_pool = f"{marker}{payload})" in slot[0] if payload is not None else marker not in slot[0]
    if not correct_pool or slot[2] != expected:
        raise Refusal(f"{label}: expected a {expected} slot on the selected pool")
    return slot[1], admission


LIVE_CONFIG = "config.json"
LIVE_LEDGER = "ledger.json"
LIVE_CONTROL = "oxide-managed/state.json"
LIVE_DOCUMENTS = (LIVE_CONFIG, LIVE_LEDGER, LIVE_CONTROL)
DOCUMENT_BYTES = 64 * 1024 * 1024


def canary_environment():
    # Prevent the staged binary's normal older-binary hand-off and keep
    # caller-supplied scratch/test settings from routing the canary elsewhere.
    excluded = {"OXIDE_POOL_CREATE", "OXIDE_TASK_SCRATCH_ROOT", "OXIDE_SCRATCH_ROOT",
                "OXIDE_ONE_USE_WORKSPACE", "GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"}
    environment = {key: value for key, value in os.environ.items()
                   if key not in excluded and not key.startswith(("OXIDE_TEST_", "OXIDE_PROOF_"))}
    environment["OXIDE_INTERNAL_HANDED_OFF"] = "1"
    return environment


def exercise(built, environment, scratch, scratch_id, payload, arguments, closure, after_new=None):
    """Run the new-slot and warm-reuse checks in one fresh crate under scratch.

    `closure["safe"]` records whether writer closure is confirmed, so a caller
    can decide whether its own directories may be removed.
    """
    crate = Path(tempfile.mkdtemp(prefix="oxide-canary.", dir=scratch))
    created = directory_identity(crate)
    missed_limits = []
    cleanup_refusal = None
    try:
        (crate / "src").mkdir()
        (crate / "target").mkdir()  # Keep final output local in permanent mode.
        (crate / "Cargo.toml").write_text(
            '[package]\nname = "oxide-canary"\nversion = "0.0.0"\nedition = "2021"\n[workspace]\n'
        )
        (crate / "src/lib.rs").write_text("pub fn canary() {}\n")
        cold_slot = None
        for label, expected, limit in (("new slot", "cold", arguments.new_seconds),
                                       ("warm reuse", "warm", arguments.warm_seconds)):
            if (directory_identity(scratch) != scratch_id or directory_identity(crate) != created
                    or created[0] != scratch_id[0]):
                raise Refusal("the canary crate or selected volume changed")
            closure["safe"] = False
            result, wall, events = canary_check(built, crate, environment)
            terminal = events[-1] if events and isinstance(events[-1], dict) else {}
            closure["safe"] = (terminal.get("v") == 1 and terminal.get("event") == "finished"
                               and terminal.get("lease") == "released"
                               and terminal.get("drain") in ("complete", "not_reached"))
            slot, admission = assess_canary(label, result, wall, events, expected, payload, limit)
            if admission > limit:
                missed_limits.append(f"{label} admission exceeded {limit:g}s")
            if expected == "cold":
                cold_slot = slot
                if after_new is not None:
                    after_new()
            elif slot != cold_slot:
                raise Refusal("warm check used a different slot")
    finally:
        # A raise here would replace an exception already in flight, so the
        # refusal is recorded and raised only after a clean finish.
        if closure["safe"]:
            if directory_identity(scratch) != scratch_id or directory_identity(crate) != created:
                cleanup_refusal = f"the canary crate or its scratch directory changed; skipped cleanup of {crate}"
            else:
                shutil.rmtree(crate)
        else:
            print(f"Canary crate cleanup skipped because writer closure is unconfirmed: {crate}",
                  file=sys.stderr)
        if cleanup_refusal is not None:
            print(cleanup_refusal, file=sys.stderr)
    if cleanup_refusal is not None:
        raise Refusal(cleanup_refusal)
    if missed_limits:
        raise Refusal(", ".join(missed_limits))


def run_canary(built, arguments):
    """The live canary: the selected live pool, refused unless versions match."""
    environment = canary_environment()
    print("Staged oxide status:", flush=True)
    started = time.monotonic()
    subprocess.run([str(built), "status"], env=environment, check=True, timeout=60)
    print(f"status: wall {time.monotonic() - started:.3f}s")
    described = subprocess.run([str(built), "pool", "describe", "--json"], env=environment,
                               check=True, stdout=subprocess.PIPE, text=True, timeout=60)
    pool, payload, scratch, scratch_id = canary_scratch(json.loads(described.stdout), arguments.scratch_root)
    # A disappearing pool cannot authorize creation at the explicit path.
    environment.update(OXIDE_POOL=str(pool), GIT_CEILING_DIRECTORIES=str(scratch))
    if directory_identity(scratch) != scratch_id:
        raise Refusal("the selected scratch directory changed")
    exercise(built, environment, scratch, scratch_id, payload, arguments, {"safe": True})


def run_scratch_canary(built, arguments):
    """`--scratch-root` alone: a fresh pool under that directory.

    No live pool is read or written and no live-version check applies, so the
    staged binary's own control state is exercised from creation onward.
    """
    scratch = arguments.scratch_root
    if (not scratch.is_absolute() or scratch.resolve(strict=True) != scratch
            or scratch == scratch.parent):
        raise Refusal(f"scratch must be an existing canonical, privately writable directory: {scratch}")
    scratch_id = verify_scratch_dir(scratch)
    root = Path(tempfile.mkdtemp(prefix="oxide-canary-pool.", dir=scratch))
    root_id = directory_identity(root)
    pool = root / "pool"  # Created by the staged binary at this explicit path.
    environment = canary_environment()
    environment.update(OXIDE_POOL=str(pool), OXIDE_POOL_CREATE="1", GIT_CEILING_DIRECTORIES=str(scratch))
    print(f"fresh scratch pool: {pool}", flush=True)
    closure = {"safe": True}
    try:
        exercise(built, environment, scratch, scratch_id, None, arguments, closure)
    finally:
        if closure["safe"] and directory_identity(scratch) == scratch_id and directory_identity(root) == root_id:
            shutil.rmtree(root)
        else:
            print(f"Fresh scratch pool cleanup skipped because writer closure is unconfirmed: {root}",
                  file=sys.stderr)


def state_version(data):
    """The control-state version a serialized state document declares, if any."""
    try:
        value = json.loads(data).get("version")
    except (ValueError, AttributeError):
        return None
    return value if type(value) is int else None


def read_live_document(directory, name, optional=False):
    """Read one live document relative to its already-opened pool directory.

    `directory` is a descriptor for the pool itself, so every name resolves
    under the pool this run opened and no path component can be re-pointed
    between names. A document keeps its bytes and file identity, so the same
    read can prove afterwards that nothing changed.
    """
    try:
        descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
    except FileNotFoundError:
        if optional:
            return None
        raise Refusal(f"the live pool has no {name}")
    except OSError as error:
        raise Refusal(f"the live pool's {name} cannot be read without following links: {error}")
    with os.fdopen(descriptor, "rb") as file:
        info = os.fstat(file.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise Refusal(f"the live pool's {name} is not a regular file")
        data = file.read(DOCUMENT_BYTES + 1)
    if len(data) > DOCUMENT_BYTES:
        raise Refusal(f"the live pool's {name} exceeds the byte limit")
    identity = (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)
    return data, identity


def read_live_control(directory):
    """The optional control document, through a no-follow control directory."""
    try:
        control = os.open("oxide-managed", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                          dir_fd=directory)
    except FileNotFoundError:
        return None
    except OSError as error:
        raise Refusal(f"the live control directory cannot be opened without following links: {error}")
    try:
        return read_live_document(control, "state.json", optional=True)
    finally:
        os.close(control)


def read_live_documents(pool):
    """Read the live control and ledger documents without writing or following links.

    Config and ledger are required. The control state is optional: a pool the
    front door has never used has none. The pool is opened once as a directory
    descriptor and every document is read relative to it.
    """
    try:
        directory = os.open(pool, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    except OSError as error:
        raise Refusal(f"the live pool cannot be opened without following links: {error}")
    try:
        documents = {name: read_live_document(directory, name)
                     for name in (LIVE_CONFIG, LIVE_LEDGER)}
        control = read_live_control(directory)
    finally:
        os.close(directory)
    if control is not None:
        documents[LIVE_CONTROL] = control
    return documents


def verify_live_documents(pool, before):
    """Refuse when any live document changed since `read_live_documents`."""
    if read_live_documents(pool) != before:
        raise Refusal("live pool documents changed during the format-change canary; no PASS")


def write_document_copy(root, documents):
    """Write the copied documents into a scratch pool layout, owner-only.

    The copy always carries a budget-pool payload with an empty `slots/`
    directory: without it the staged binary refuses the copy with
    unknown_accounting before the copied ledger is even read, because a
    payload with no slots is not valid accounting. The staged binary then
    initializes the copy's own Barge state under the copied
    `oxide-managed/` directory on first open and reconciles the copied
    ledger (retiring the absent roots) into the copy alone; the live pool
    is never written.
    """
    copy = root / "pool"
    copy.mkdir(mode=0o700)
    (copy / "payload").mkdir(mode=0o700)
    (copy / "payload" / "slots").mkdir(mode=0o700)
    if LIVE_CONTROL in documents:
        (copy / "oxide-managed").mkdir(mode=0o700)
    for name, (data, _) in documents.items():
        descriptor = os.open(copy / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "wb") as file:
            file.write(data)
    return copy


def run_format_canary(built, arguments, capabilities):
    """`--format-change`: the staged binary upgrades a read-only copy of the live documents.

    The live pool is never a `status`, admission or upgrade target. The only
    contact is one `pool describe` to find its documents and one read-only
    copy; the staged binary runs against the copy alone, and the live documents
    must match their first read byte for byte afterwards.
    """
    environment = canary_environment()
    described = subprocess.run([str(built), "pool", "describe", "--json"], env=environment,
                               check=True, stdout=subprocess.PIPE, text=True, timeout=60)
    pool, scratch, scratch_id = format_canary_scratch(json.loads(described.stdout), arguments.scratch_root)
    before = read_live_documents(pool)
    staged = capabilities["control_state_version"]
    live_version = state_version(before[LIVE_CONTROL][0]) if LIVE_CONTROL in before else None
    if live_version is not None and live_version > staged:
        raise Refusal(f"the live control state (version {live_version}) is newer than the staged build ({staged})")
    print(f"format-change: read {len(before)} live documents read-only: {', '.join(before)}", flush=True)
    root = Path(tempfile.mkdtemp(prefix="oxide-format-canary.", dir=scratch))
    root_id = directory_identity(root)
    closure = {"safe": True}
    try:
        copy = write_document_copy(root, before)
        environment.update(OXIDE_POOL=str(copy), GIT_CEILING_DIRECTORIES=str(scratch))
        if directory_identity(scratch) != scratch_id:
            raise Refusal("the selected scratch directory changed")
        print("Staged oxide status on the copy:", flush=True)
        started = time.monotonic()
        subprocess.run([str(built), "status"], env=environment, check=True, timeout=60)
        print(f"format-change status: wall {time.monotonic() - started:.3f}s")

        def after_new_slot():
            # The first mutating admission on the copy is the upgrade. The copy
            # is an explicit internal pool, so its slots name no removable volume.
            state = copy / "oxide-managed" / "state.json"
            upgraded = state_version(state.read_bytes()) if state.exists() else None
            if live_version is not None and upgraded != staged:
                raise Refusal(f"format-change: the upgrade reached control state {upgraded}, not the staged {staged}")
            print(f"format-change: control state {live_version if live_version is not None else 'absent'}"
                  f" -> {upgraded if upgraded is not None else 'absent'} (staged {staged})")

        exercise(built, environment, scratch, scratch_id, None, arguments, closure, after_new=after_new_slot)
    finally:
        try:
            verify_live_documents(pool, before)
        finally:
            if closure["safe"] and directory_identity(scratch) == scratch_id and directory_identity(root) == root_id:
                shutil.rmtree(root)
            else:
                print(f"Format-change copy cleanup skipped because writer closure is unconfirmed: {root}",
                      file=sys.stderr)
    print("format-change: live documents unchanged")


def positive_seconds(value):
    seconds = float(value)
    if not math.isfinite(seconds) or seconds <= 0:
        raise argparse.ArgumentTypeError("expected a positive number of seconds")
    return seconds


def canary_mode(arguments):
    if arguments.format_change:
        return "format-change"
    return "scratch-root" if arguments.scratch_root is not None else "live"


def canary(pin, arguments):
    # Hold the install lock across the live exercise and its receipt.
    mode = canary_mode(arguments)
    with ExitStack() as lock:
        with tempfile.TemporaryDirectory(prefix="oxide-canary-build.") as staging:
            print(f"staging {staging}", flush=True)
            instance = new_instance()
            built, _ = build(pin, instance, Path(staging))
            capabilities = state_capabilities(built)
            build_identity(built, pin, instance)
            lock.enter_context(receipt_lock())
            write_canary_receipt(pin, "FAIL", mode)
            if mode == "live":
                canary_compatibility(capabilities)
                run_canary(built, arguments)
            elif mode == "scratch-root":
                run_scratch_canary(built, arguments)
            else:
                run_format_canary(built, arguments, capabilities)
        write_canary_receipt(pin, "PASS", mode)
    print(f"oxide:canary PASS for {pin} ({mode})")


def write_receipt(receipt, finished, pin, directory=None):
    """Write one complete owner-only receipt, without replacing an old one."""
    name = f"{finished.strftime('%Y%m%dT%H%M%SZ_%f')}-{pin[:12]}.json"
    contents = (json.dumps(receipt, indent=2) + "\n").encode()
    if len(contents) > RECEIPT_MAX_BYTES:
        raise Refusal("the new install receipt exceeds its byte limit")
    own_directory = directory is None
    if own_directory:
        directory = open_owned_directory(RECEIPTS, create=True)
    temporary = temporary_name()
    published = False
    try:
        if any(other.endswith(".json") and other > name for other in os.listdir(directory)):
            raise Refusal("the receipt timestamp sorts before existing history; check the system clock")
        write_private(directory, temporary, 0o600, lambda output: output.write(contents))
        os.link(temporary, name, src_dir_fd=directory, dst_dir_fd=directory)
        published = True
        os.fsync(directory)
    except BaseException:
        if published:
            os.unlink(name, dir_fd=directory)
        raise
    finally:
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
        if own_directory:
            os.close(directory)
    return RECEIPTS / name


def archive_previous(directory, receipt_directory, receipt_name, expected):
    """Save a private copy so it cannot change with the installed inode."""
    name = receipt_name + ".binary"
    archived = identity(receipt_directory, name, private=True)
    if archived is None:
        temporary = temporary_name()
        try:
            flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
            with os.fdopen(os.open(DESTINATION.name, flags, dir_fd=directory), "rb") as source:
                info = os.fstat(source.fileno())
                if not stat.S_ISREG(info.st_mode) or (info.st_dev, info.st_ino) != expected[:2]:
                    raise Refusal("the installed binary changed before it could be archived")
                write_private(
                    receipt_directory, temporary, 0o700,
                    lambda output: shutil.copyfileobj(source, output),
                )
            if identity(receipt_directory, temporary, private=True)[2] != expected[2]:
                raise Refusal("the archived copy does not match the prior receipt")
            os.link(temporary, name, src_dir_fd=receipt_directory, dst_dir_fd=receipt_directory)
            os.fsync(receipt_directory)
        finally:
            try:
                os.unlink(temporary, dir_fd=receipt_directory)
            except FileNotFoundError:
                pass
    elif archived[2] != expected[2]:
        raise Refusal("the existing archived binary does not match its receipt")


def install(source, expected, *, source_hash=None, previous=None, receipt_directory=None, record=None):
    """Install one staged, hash-verified copy and commit its receipt.

    Keep the old inode until receipt publication succeeds, so a receipt write
    error can undo the swap without changing a concurrent destination.
    """
    if source_hash is None:
        source_hash = sha256(source)
    directory = open_destination_directory()
    temporary = temporary_name()
    original = temporary_name()
    try:
        flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
        with os.fdopen(os.open(source, flags), "rb") as binary:
            if not stat.S_ISREG(os.fstat(binary.fileno()).st_mode):
                raise Refusal("the source binary must be a regular file")
            write_private(directory, temporary, 0o755, lambda output: shutil.copyfileobj(binary, output))
        staged = identity(directory, temporary)
        if staged[2] != source_hash:
            raise Refusal("staged copy hash differs from the expected binary")
        if identity(directory, DESTINATION.name) != expected:
            raise Refusal(f"{DESTINATION} changed during preparation")
        if previous is not None:
            archive_previous(directory, receipt_directory, previous[0].name, expected)
            os.link(DESTINATION.name, original, src_dir_fd=directory, dst_dir_fd=directory,
                    follow_symlinks=False)
            if identity(directory, original) != expected:
                raise Refusal("the installed binary changed while making the rollback copy")
            if identity(directory, DESTINATION.name) != expected:
                raise Refusal("the installed binary changed before replacement")
        replaced_at = datetime.datetime.now(datetime.timezone.utc)
        if expected is None:
            # link() refuses an existing destination: no-clobber.
            os.link(temporary, DESTINATION.name, src_dir_fd=directory, dst_dir_fd=directory)
        else:
            os.rename(temporary, DESTINATION.name, src_dir_fd=directory, dst_dir_fd=directory)
        try:
            if identity(directory, DESTINATION.name) != staged:
                raise Refusal(f"{DESTINATION} is not the staged file after installation")
            info = os.stat(DESTINATION.name, dir_fd=directory, follow_symlinks=False)
            installed = {
                "path": str(DESTINATION),
                "owner_uid": info.st_uid,
                "mode": oct(stat.S_IMODE(info.st_mode)),
                "size": info.st_size,
                "mtime": datetime.datetime.fromtimestamp(info.st_mtime, datetime.timezone.utc).isoformat(),
                "sha256": staged[2],
            }
            if record is not None:
                os.fsync(directory)
                record(installed, replaced_at)
        except BaseException:
            if record is not None and identity(directory, DESTINATION.name) == staged:
                if previous is None:
                    os.unlink(DESTINATION.name, dir_fd=directory)
                else:
                    os.rename(original, DESTINATION.name, src_dir_fd=directory, dst_dir_fd=directory)
                os.fsync(directory)
            raise
    finally:
        for name in (temporary, original):
            try:
                os.unlink(name, dir_fd=directory)
            except FileNotFoundError:
                pass
        os.close(directory)
    return staged[2], installed


def ps_rows(column):
    result = subprocess.run(
        ["ps", "-axww", "-o", "pid=", "-o", "uid=", "-o", "lstart=", "-o", column + "="],
        check=True, capture_output=True, text=True, errors="replace", timeout=10,
        env={**os.environ, "LC_ALL": "C", "TZ": "UTC"},
    )
    for line in result.stdout.splitlines():
        fields = line.split(maxsplit=7)
        if len(fields) != 8 or not fields[0].isdigit() or fields[1] != str(os.getuid()):
            continue
        try:
            started = datetime.datetime.strptime(" ".join(fields[2:7]), "%a %b %d %H:%M:%S %Y")
        except ValueError:
            continue
        yield (int(fields[0]), started.replace(tzinfo=datetime.timezone.utc)), fields[7]


def report_old_processes(replaced_at):
    """Report live Oxide processes observed near the swap, using read-only ps."""
    try:
        old = {key for key, comm in ps_rows("comm")
               if Path(comm).name == "oxide" and key[1] <= replaced_at}
        if not old:
            print("No pre-install oxide processes observed.")
            return
        matches = [(key, argv) for key, argv in ps_rows("args") if key in old]
    except (OSError, subprocess.SubprocessError) as error:
        print(f"Could not list pre-install oxide processes: {error}", file=sys.stderr)
        return
    if not matches:
        print("No pre-install oxide processes observed.")
        return
    print("Live oxide processes started before or in the same second as the swap:")
    for (pid, started), argv in sorted(matches):
        print(f"  PID {pid}  {started.isoformat()}  {argv!r}")


def commit_install(source, source_hash, receipt, previous, expected, directory):
    receipt_path = None

    def record(installed, replaced_at):
        nonlocal receipt_path
        receipt.update(
            version=RECEIPT_VERSION,
            finished=replaced_at.isoformat(),
            installed=installed,
            prior="absent" if previous is None else {
                "receipt": previous[0].name,
                "receipt_sha256": previous[2],
                "sha256": previous[1]["installed"]["sha256"],
            },
        )
        receipt_path = write_receipt(receipt, replaced_at, receipt["pin"], directory)

    if not isinstance(receipt.get("pin"), str) or not PIN.fullmatch(receipt["pin"]):
        raise Refusal("the install receipt has no valid source pin")
    install(source, expected, source_hash=source_hash, previous=previous,
            receipt_directory=directory, record=record)
    print(json.dumps(receipt, indent=2))
    print(f"receipt {receipt_path}")
    report_old_processes(datetime.datetime.fromisoformat(receipt["finished"]))


def rollback_target(current, directory):
    prior = current[1].get("prior")
    if current[1].get("version") != RECEIPT_VERSION or not isinstance(prior, dict):
        raise Refusal("the newest receipt has no saved previous binary")
    path, receipt, receipt_hash = read_receipt(prior.get("receipt"), directory)
    if path.name >= current[0].name:
        raise Refusal("the prior receipt does not precede the current receipt")
    expected_hash = receipt["installed"]["sha256"]
    if prior.get("sha256") != expected_hash or prior.get("receipt_sha256") != receipt_hash:
        raise Refusal("the prior receipt hash does not match the newest receipt")
    name = path.name + ".binary"
    archived = identity(directory, name, private=True)
    if archived is None or archived[2] != expected_hash:
        raise Refusal("the saved previous binary does not match its receipt")
    return RECEIPTS / name, expected_hash, receipt


def rollback(directory, check=False):
    current, expected = preflight(True)
    saved, saved_hash, prior = rollback_target(current, directory)
    # Carry the old executable's authenticated build facts into the new receipt.
    receipt = {**prior, "mode": "rollback",
               "started": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    if not check:
        commit_install(saved, saved_hash, receipt, current, expected, directory)
    print("Downgrading persisted state: before resuming Oxide, follow the 'Persisted state versions and downgrade' section in "
          "docs/knowledge/contracts/workflow.md if the restored binary is from an older generation "
          "or has older format capabilities.")


def remove_staging(path):
    """Remove a build staging directory. A failure is reported, not raised, so it
    cannot replace the install's own outcome."""
    try:
        shutil.rmtree(path)
    except OSError as error:
        print(f"staging {path} not removed: {error}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pin", help="commit on origin main (default: OXIDE_INSTALL_PIN, else its tip)")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--replace", action="store_true", help="replace the receipt-matching binary")
    mode.add_argument("--rollback", action="store_true", help="restore the verified previous binary")
    mode.add_argument("--canary", action="store_true", help="run the staged binary on the live pool")
    parser.add_argument("--check", action="store_true", help="preflight only; build nothing")
    parser.add_argument("--require-canary", action="store_true", help="require a same-pin PASS from the last hour")
    # The reported admission ends when the child starts, and an idle slot's Barge
    # trash deletion runs beside the writer with the pool and admission locks
    # released (oxide#324). A live pool at the 256-slot ceiling therefore no longer
    # charges its post-rename deletion to this figure, so the default stays tight.
    parser.add_argument("--new-seconds", type=positive_seconds,
                        help="canary new-slot limit (default: 10); a ceiling eviction's "
                             "post-rename trash deletion is not counted")
    parser.add_argument("--warm-seconds", type=positive_seconds, help="canary warm-reuse limit (default: 5)")
    parser.add_argument("--scratch-root", type=Path,
                        help="existing scratch directory: a fresh scratch pool with no live-version check; "
                             "with --format-change, where the copied documents live")
    parser.add_argument("--format-change", action="store_true",
                        help="canary a format-changing build: upgrade a read-only copy of the live documents")
    arguments = parser.parse_args()
    if arguments.canary and (arguments.check or arguments.require_canary):
        parser.error("--canary cannot be combined with --check or --require-canary")
    if not arguments.canary and any(value is not None for value in (
        arguments.new_seconds, arguments.warm_seconds, arguments.scratch_root,
    )) or (not arguments.canary and arguments.format_change):
        parser.error("--new-seconds, --warm-seconds, --scratch-root and --format-change require --canary")
    started = datetime.datetime.now(datetime.timezone.utc)
    staging = None
    try:
        if arguments.rollback:
            if arguments.pin is not None or arguments.require_canary:
                raise Refusal("rollback selects the prior receipt and takes no pin or --require-canary")
            if arguments.check:
                directory = open_owned_directory(RECEIPTS)
                try:
                    rollback(directory, check=True)
                finally:
                    os.close(directory)
            else:
                with receipt_lock(create=False) as directory:
                    rollback(directory)
            return 0

        if not arguments.canary:
            print("Run effigy oxide:canary before installing the same pin. Use --require-canary to enforce it.",
                  flush=True)
        pin, tree, tip = authenticate_pin(arguments.pin or os.environ.get("OXIDE_INSTALL_PIN") or None)
        if arguments.canary:
            arguments.new_seconds = arguments.new_seconds or 10
            arguments.warm_seconds = arguments.warm_seconds or 5
            canary(pin, arguments)
            return 0
        if arguments.require_canary:
            require_canary(pin)
        previous, expected = preflight(arguments.replace)
        print(f"pin {pin} tree {tree} (origin main {tip})")
        print(f"destination {DESTINATION}: " + ("absent" if previous is None else f"matches {previous[0].name}"))
        if arguments.check:
            print("check only: nothing built or installed")
            return 0
        staging = Path(tempfile.mkdtemp(prefix="oxide-install."))
        print(f"staging {staging}")
        instance = new_instance()
        built, versions = build(pin, instance, staging)
        capabilities = state_capabilities(built)
        build_identity(built, pin, instance)
        receipt = {
            "mode": "initial" if previous is None else "replace",
            "started": started.isoformat(),
            "pin": pin,
            "tree": tree,
            "origin_main": tip,
            "build": " ".join(BUILD),
            "toolchain": versions,
            "built_sha256": sha256(built),
            "build_instance": instance,
            **capabilities,
        }
        with receipt_lock() as directory:
            if arguments.require_canary:
                require_canary(pin)
            if preflight(arguments.replace) != (previous, expected):
                raise Refusal("the installation changed during the build")
            commit_install(built, receipt["built_sha256"], receipt, previous, expected, directory)
    except (Refusal, subprocess.SubprocessError, OSError) as error:
        if arguments.canary:
            print(f"oxide:canary FAIL: {error}", file=sys.stderr)
            return 1
        action = "rollback" if arguments.rollback else "install"
        print(f"oxide:{action} refused: {error}", file=sys.stderr)
        return 1
    except (KeyError, TypeError, ValueError, KeyboardInterrupt) as error:
        if not arguments.canary:
            raise
        print(f"oxide:canary FAIL: {error or 'interrupted'}", file=sys.stderr)
        return 1
    finally:
        if staging is not None:
            remove_staging(staging)
    return 0


if __name__ == "__main__":
    sys.exit(main())
