"""Build `oxide` from a pushed-main pin and install it at ~/.local/bin/oxide.

Initial mode refuses any `oxide` on PATH and any existing destination entry.
`--replace` swaps only the regular, user-owned binary whose SHA-256 matches
the newest install receipt. Each run writes a new receipt; earlier receipts
are kept for rollback. `--check` stops after the preflight and builds nothing.
Effigy does not forward selector arguments, so `OXIDE_INSTALL_PIN` also names
the pin.
"""
import argparse
import datetime
import hashlib
import json
import os
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HOME = Path(os.environ["HOME"])
DESTINATION = HOME / ".local/bin/oxide"
RECEIPTS = HOME / ".local/state/oxide/install"
BUILD = ["cargo", "build", "--release", "--locked", "--offline", "--bin", "oxide"]
REPOSITORY = Path(__file__).resolve().parent.parent
REMOTE_MAIN = "refs/heads/main"


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


def authenticate_pin(requested):
    """Return (pin, tree, remote tip); the pin must be reachable from origin main."""
    listing = git("ls-remote", "origin", REMOTE_MAIN).split()
    if len(listing) != 2 or listing[1] != REMOTE_MAIN:
        raise Refusal(f"origin {REMOTE_MAIN} not found")
    tip = listing[0]
    git("fetch", "--quiet", "origin", REMOTE_MAIN)
    if git("rev-parse", "FETCH_HEAD") != tip:
        raise Refusal("fetched main does not match ls-remote; retry")
    pin = git("rev-parse", "--verify", f"{requested or tip}^{{commit}}")
    if subprocess.run(
        ["git", "-C", str(REPOSITORY), "merge-base", "--is-ancestor", pin, tip]
    ).returncode != 0:
        raise Refusal(f"{pin} is not on pushed origin main ({tip})")
    return pin, git("rev-parse", f"{pin}^{{tree}}"), tip


def path_entries():
    """Every `oxide` entry in a PATH directory, links included."""
    found = []
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(os.path.abspath(Path(directory or ".") / "oxide"))
        if os.path.lexists(candidate) and candidate not in found:
            found.append(candidate)
    return found


def newest_receipt():
    if not RECEIPTS.is_dir():
        return None
    receipts = sorted(RECEIPTS.glob("*.json"))
    return (receipts[-1], json.loads(receipts[-1].read_text())) if receipts else None


def open_destination_directory():
    """Open the destination's parent after refusing any link or foreign directory.

    Every component from HOME down must be a real directory owned by this
    user, and none below HOME may be group or world writable.
    """
    parts = DESTINATION.parent.relative_to(HOME).parts
    for count in range(len(parts) + 1):
        directory = HOME.joinpath(*parts[:count])
        info = os.lstat(directory)
        writable = count > 0 and info.st_mode & 0o022
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or writable:
            raise Refusal(f"{directory} is not a real directory owned only by this user")
    descriptor = os.open(DESTINATION.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    opened, checked = os.fstat(descriptor), os.lstat(DESTINATION.parent)
    if (opened.st_dev, opened.st_ino) != (checked.st_dev, checked.st_ino):
        os.close(descriptor)
        raise Refusal(f"{DESTINATION.parent} changed while it was checked")
    return descriptor


def identity(directory, name):
    """(device, inode, SHA-256) of a regular user-owned entry, or None if absent.

    Links, other file types and foreign owners refuse.
    """
    try:
        info = os.stat(name, dir_fd=directory, follow_symlinks=False)
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
        raise Refusal(f"{DESTINATION.parent / name} is not a regular file owned by this user")
    descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=directory)
    with os.fdopen(descriptor, "rb") as file:
        opened = os.fstat(file.fileno())
        if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
            raise Refusal(f"{DESTINATION.parent / name} changed while it was read")
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


def build(pin, staging):
    source = staging / "source"
    source.mkdir()
    archive = subprocess.Popen(
        ["git", "-C", str(REPOSITORY), "archive", pin], stdout=subprocess.PIPE
    )
    subprocess.run(["tar", "-x", "-C", str(source)], stdin=archive.stdout, check=True)
    if archive.wait() != 0:
        raise Refusal("git archive failed")
    environment = {k: v for k, v in os.environ.items() if not k.startswith("CARGO_TARGET_")}
    environment["CARGO_TARGET_DIR"] = str(staging / "target")
    versions = {
        tool: subprocess.run(
            [tool, "-vV" if tool == "rustc" else "-V"],
            cwd=source, env=environment, check=True, capture_output=True, text=True,
        ).stdout.strip()
        for tool in ("rustc", "cargo")
    }
    subprocess.run(BUILD, cwd=source, env=environment, check=True)
    return staging / "target/release/oxide", versions


def install(built, expected):
    """Copy through a private sibling, then link (initial) or rename (replace).

    The destination is rechecked through the opened parent directly before the
    link or rename: absent for link, the preflight's exact identity for rename.
    """
    built_hash = sha256(built)
    directory = open_destination_directory()
    temporary = f".oxide.install.{os.getpid()}.{secrets.token_hex(8)}"
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    try:
        with os.fdopen(os.open(temporary, flags, 0o700, dir_fd=directory), "wb") as output:
            with open(built, "rb") as source:
                shutil.copyfileobj(source, output)
            output.flush()
            os.fchmod(output.fileno(), 0o755)
            os.fsync(output.fileno())
        staged = identity(directory, temporary)
        if staged[2] != built_hash:
            raise Refusal("staged copy hash differs from the build output")
        if identity(directory, DESTINATION.name) != expected:
            raise Refusal(f"{DESTINATION} changed during the build")
        if expected is None:
            # link() refuses an existing destination: no-clobber.
            os.link(temporary, DESTINATION.name, src_dir_fd=directory, dst_dir_fd=directory)
        else:
            os.rename(temporary, DESTINATION.name, src_dir_fd=directory, dst_dir_fd=directory)
        if identity(directory, DESTINATION.name) != staged:
            raise Refusal(f"{DESTINATION} is not the staged file after installation")
        info = os.stat(DESTINATION.name, dir_fd=directory, follow_symlinks=False)
    finally:
        try:
            os.unlink(temporary, dir_fd=directory)
        except FileNotFoundError:
            pass
        os.close(directory)
    installed = {
        "path": str(DESTINATION),
        "owner_uid": info.st_uid,
        "mode": oct(stat.S_IMODE(info.st_mode)),
        "size": info.st_size,
        "mtime": datetime.datetime.fromtimestamp(info.st_mtime, datetime.timezone.utc).isoformat(),
        "sha256": staged[2],
    }
    return built_hash, installed


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--pin",
        default=os.environ.get("OXIDE_INSTALL_PIN") or None,
        help="commit on origin main (default: OXIDE_INSTALL_PIN, else its tip)",
    )
    parser.add_argument("--replace", action="store_true", help="replace the receipt-matching binary")
    parser.add_argument("--check", action="store_true", help="preflight only; build nothing")
    arguments = parser.parse_args()
    started = datetime.datetime.now(datetime.timezone.utc)
    try:
        pin, tree, tip = authenticate_pin(arguments.pin)
        previous, expected = preflight(arguments.replace)
        print(f"pin {pin} tree {tree} (origin main {tip})")
        print(f"destination {DESTINATION}: " + ("absent" if previous is None else f"matches {previous[0].name}"))
        if arguments.check:
            print("check only: nothing built or installed")
            return 0
        staging = Path(tempfile.mkdtemp(prefix="oxide-install.", dir="/private/tmp"))
        print(f"staging {staging}")
        built, versions = build(pin, staging)
        built_hash, installed = install(built, expected)
    except (Refusal, subprocess.CalledProcessError, OSError) as error:
        print(f"oxide:install refused: {error}", file=sys.stderr)
        return 1
    receipt = {
        "mode": "initial" if previous is None else "replace",
        "started": started.isoformat(),
        "finished": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "pin": pin,
        "tree": tree,
        "origin_main": tip,
        "build": " ".join(BUILD),
        "toolchain": versions,
        "built_sha256": built_hash,
        "installed": installed,
        "prior": "absent" if previous is None else {
            "receipt": previous[0].name, "sha256": previous[1]["installed"]["sha256"],
        },
    }
    RECEIPTS.mkdir(parents=True, exist_ok=True)
    info = os.lstat(RECEIPTS)
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        print(f"oxide:install installed but cannot write a receipt: {RECEIPTS} is not a real owned directory", file=sys.stderr)
        print(json.dumps(receipt, indent=2))
        return 1
    name = f"{started.strftime('%Y%m%dT%H%M%SZ')}-{pin[:12]}.json"
    with open(RECEIPTS / name, "x") as file:
        json.dump(receipt, file, indent=2)
        file.write("\n")
    shutil.rmtree(staging)
    print(json.dumps(receipt, indent=2))
    print(f"receipt {RECEIPTS / name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
