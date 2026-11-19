"""Build `oxide` from a pushed-main pin and install it at ~/.local/bin/oxide.

Initial mode refuses any `oxide` on PATH and any existing destination entry.
`--replace` swaps only the regular, user-owned binary whose SHA-256 matches
the newest install receipt. A replacement saves that binary beside its
receipt. `--rollback` restores the verified prior binary without building.
`--check` stops after the preflight and builds nothing.
Effigy does not forward selector arguments, so `OXIDE_INSTALL_PIN` also names
the pin.

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
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from contextlib import contextmanager
from pathlib import Path

HOME = Path(os.environ["HOME"])
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


class Refusal(Exception):
    pass


def git(*args):
    return subprocess.run(
        ["git", "-C", str(REPOSITORY), *args], check=True, capture_output=True, text=True
    ).stdout.strip()


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
    archive = subprocess.Popen(
        ["git", "-C", str(REPOSITORY), "archive", pin], stdout=subprocess.PIPE
    )
    subprocess.run(["tar", "-x", "-C", str(source)], stdin=archive.stdout, check=True)
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
    print("Before resuming Oxide, follow the 'Downgrading persisted state' step in "
          "docs/knowledge/contracts/workflow.md if the restored binary is from an older generation "
          "or has older format capabilities.")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--pin", help="commit on origin main (default: OXIDE_INSTALL_PIN, else its tip)")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--replace", action="store_true", help="replace the receipt-matching binary")
    mode.add_argument("--rollback", action="store_true", help="restore the verified previous binary")
    parser.add_argument("--check", action="store_true", help="preflight only; build nothing")
    arguments = parser.parse_args()
    started = datetime.datetime.now(datetime.timezone.utc)
    try:
        if arguments.rollback:
            if arguments.pin is not None:
                raise Refusal("rollback selects the prior receipt and takes no pin")
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

        pin, tree, tip = authenticate_pin(arguments.pin or os.environ.get("OXIDE_INSTALL_PIN") or None)
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
            if preflight(arguments.replace) != (previous, expected):
                raise Refusal("the installation changed during the build")
            commit_install(built, receipt["built_sha256"], receipt, previous, expected, directory)
    except (Refusal, subprocess.CalledProcessError, OSError) as error:
        action = "rollback" if arguments.rollback else "install"
        print(f"oxide:{action} refused: {error}", file=sys.stderr)
        return 1
    shutil.rmtree(staging)
    return 0


if __name__ == "__main__":
    sys.exit(main())
