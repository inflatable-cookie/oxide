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
        candidate = Path(directory or ".") / "oxide"
        if os.path.lexists(candidate) and candidate not in found:
            found.append(candidate)
    return found


def newest_receipt():
    if not RECEIPTS.is_dir():
        return None
    receipts = sorted(RECEIPTS.glob("*.json"))
    return (receipts[-1], json.loads(receipts[-1].read_text())) if receipts else None


def describe(path):
    info = os.lstat(path)
    return {
        "path": str(path),
        "owner_uid": info.st_uid,
        "mode": oct(stat.S_IMODE(info.st_mode)),
        "size": info.st_size,
        "mtime": datetime.datetime.fromtimestamp(info.st_mtime, datetime.timezone.utc).isoformat(),
        "sha256": sha256(path),
    }


def preflight(replace):
    """Return the receipt being replaced, or None in initial mode."""
    entries = path_entries()
    if not replace:
        if entries:
            raise Refusal("oxide already on PATH: " + ", ".join(map(str, entries)))
        if os.path.lexists(DESTINATION):
            raise Refusal(f"{DESTINATION} already exists")
        return None
    others = [entry for entry in entries if entry.resolve() != DESTINATION.resolve()]
    if others:
        raise Refusal("another oxide on PATH: " + ", ".join(map(str, others)))
    previous = newest_receipt()
    if previous is None:
        raise Refusal(f"no install receipt under {RECEIPTS}; use initial mode")
    info = os.lstat(DESTINATION)
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
        raise Refusal(f"{DESTINATION} is not a regular file owned by this user")
    if sha256(DESTINATION) != previous[1]["installed"]["sha256"]:
        raise Refusal(f"{DESTINATION} does not match receipt {previous[0].name}")
    return previous


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


def install(built, previous):
    """Copy through a private sibling, then link (initial) or rename (replace)."""
    built_hash = sha256(built)
    descriptor, temporary = tempfile.mkstemp(prefix=".oxide.install.", dir=DESTINATION.parent)
    try:
        with os.fdopen(descriptor, "wb") as output, open(built, "rb") as source:
            shutil.copyfileobj(source, output)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o755)
        if sha256(temporary) != built_hash:
            raise Refusal("staged copy hash differs from the build output")
        if previous is None:
            # link() refuses an existing destination: no-clobber.
            os.link(temporary, DESTINATION)
        else:
            if sha256(DESTINATION) != previous[1]["installed"]["sha256"]:
                raise Refusal(f"{DESTINATION} changed during the build")
            os.rename(temporary, DESTINATION)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)
    installed = describe(DESTINATION)
    if installed["sha256"] != built_hash:
        raise Refusal("installed hash differs from the build output")
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
        previous = preflight(arguments.replace)
        print(f"pin {pin} tree {tree} (origin main {tip})")
        print(f"destination {DESTINATION}: " + ("absent" if previous is None else f"matches {previous[0].name}"))
        if arguments.check:
            print("check only: nothing built or installed")
            return 0
        staging = Path(tempfile.mkdtemp(prefix="oxide-install.", dir="/private/tmp"))
        print(f"staging {staging}")
        built, versions = build(pin, staging)
        built_hash, installed = install(built, previous)
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
