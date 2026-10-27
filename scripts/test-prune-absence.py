#!/usr/bin/env python3
"""Focused checked-out-absence proof (oxide#135), under bounded-test.py.

One staged candidate, one private 128 MiB APFS image, no installed binary and no
live pool. The qualified shared supervisor owns the deadline, continuous host
reserve sampling, monitored scratch accounting, session closure and cleanup; this
script starts no session, signals no group, keeps no timer and owns no scratch of
its own. The private Cargo home is seeded from this HEAD's lock by the shared
bounded seeder, and the existing supervised staging helper builds the candidate.

Premise checklist (mandatory for a proof-authoring brief):
(1) seed_and_stage uses the invoking Cargo home, a private Cargo home and 1.97.1
    with no install, and no `--locked` fixture call.
(2) proof_env sets OXIDE_TEST_HOME_FROM_ENV=1 for every fixture command.
(3) proof_env scrubs inherited Oxide, XDG, CARGO_* and GIT_* controls before its
    private settings.
(4) every fixture checkout's `target` is asserted absent before its adopt.
(5) no external Cargo subcommand is exercised, so no argv convention is assumed.
(6) ImageCustody attaches with `-owners on` and records exactly one ordinary
    detach in a finally path, before which the backing is never removed.
(7) staging scrubs the encoded and plain Rust-wrapper flags; fixtures set
    RUSTFLAGS empty and drop CARGO_ENCODED_RUSTFLAGS.
(8) the selector fixes --root-parent /private/tmp: the missing-ancestor premise
    is the boot Data volume, which /private/tmp supplies, and the fixture's
    directory-budget pool is a genuine internal pool the product correctly
    refuses on an external volume. Both are the owner-accepted
    internal-placement exception (decision bd357a4c), inside the same monitored
    owned root; require_data_volume() refuses before any effect otherwise.
(9) no mktemp is invoked: the supervisor supplies the canonical scratch root and
    the image backing lives in its evidence directory. Every direct child is one
    short-lived `oxide` call in the supervisor's session under one private env.

What this proof is about (oxide#135): a checkout path outside /Volumes may be
proved absent when its ancestors are missing, but only from a no-follow
descriptor walk whose held nearest existing ancestor is a readable directory on
the same device and APFS volume UUID as /System/Volumes/Data. A missing chain
never waives a mount boundary, a recorded checkout volume, or any other
authority predicate, and the real run's own ancestry witness is compared again
under the checkout's barrier and binding and immediately before publication.

Cases, in execution order (counted fail-fast; the diagnostic records each one):
  1 data-missing-chains   a registered profile group and an operator-listed
                          receiptless legacy root, each with MULTIPLE missing
                          ancestors on the boot Data volume, are candidates; the
                          receiptless root needs the exact list; one real private
                          run completes both through the journal, terminal
                          receipts and tombstones with no intent left, while a
                          live checkout stays kept with unchanged bytes.
  2 mount-boundaries      a registered checkout inside the attached image whose
                          ancestor below the mount is missing stays KEEP naming
                          the leaf-only rule, and a missing chain under an absent
                          /Volumes/<name> stays KEEP in BOTH spellings.
  3 symlink-and-nondir    a symlinked ancestor and a non-directory component each
                          stay KEEP with unchanged root bytes.
  4 witness-races         after the real run's own snapshot: recreating the
                          checkout refuses, and replacing an existing ancestor by
                          a DIFFERENT empty directory while the checkout stays
                          absent refuses with the witness comparison, which a
                          fresh absence proof alone would not notice.
  5 eacces                the debug-only scoped EACCES injection fires and keeps
                          the root, with the uninjected counterpart proving the
                          same chain absent.
  6 custody-and-group     the unknown-custody fault keeps the whole batch with no
                          stale intent, and the kept root's group rows, receipt
                          bytes and registry bytes are unchanged.

Prospective, UNMEASURED fit (one staged candidate, fixtures run no Cargo child;
the only Cargo call is staging at two jobs, planning demand 2 CPU / 4 GiB):
  launch and source binding                                   6 s
  offline seed + constrained debug staging (2 Cargo jobs)    90 s
  image allocate/attach/register + all fixture checkouts     25 s
  case 1 (two dry runs + one positive real run)              30 s
  case 2 (one registered control + two listed controls)      10 s
  case 3 (two listed controls)                               8 s
  case 4 (two 11-second armed races + fixtures)              40 s
  case 5 (unarmed control + armed control)                    8 s
  case 6 (armed control + unchanged-authority checks)         8 s
  ordinary detach, custody record and helper cleanup         25 s
  supervisor closure, reserve and contingency                50 s
  total                                                     300 s
Scratch: 288 MiB staging + 128 MiB image + 32 MiB fixtures + 32 MiB evidence +
32 MiB margin = 512 MiB. Every figure includes cleanup, is prospective and is
NOT a measured fit: a native run must establish it at the published source.

Each native invocation needs independent published source-fit, fresh finite
headroom and a SEPARATE explicit ROOT activation. Diagnostic mode is uncounted
and never a PASS.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import sys
import time
import traceback

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


wrapper = load("proof_wrapper135", "test-oxide-wrapper-bypass.py")
prior = load("proof_prune135", "test-outputs-prune.py")
permanent = prior._load("permanent135", "test-permanent-outputs.py")
prior.permanent = permanent
prior.expect = permanent.expect
prior.fail = permanent.fail
prior.DIAG = prior.Diagnostics(enabled=False)

PLAIN = '[package]\nname = "plain"\nversion = "0.1.0"\nedition = "2021"\n'
TOTAL_SECONDS = 300
TOTAL_MIB = 512
ALLOCATION_SECONDS = [
    ("launch and source binding", 6),
    ("offline seed and staging", 90),
    ("image, registration and fixtures", 25),
    ("data-missing-chains", 30),
    ("mount-boundaries", 10),
    ("symlink-and-nondir", 8),
    ("witness-races", 40),
    ("eacces", 8),
    ("custody-and-group", 8),
    ("ordinary detach and cleanup", 25),
    ("supervisor closure and contingency", 50),
]
ALLOCATION_MIB = [
    ("staging", 288),
    ("APFS sparse image", 128),
    ("fixtures", 32),
    ("evidence", 32),
    ("margin", 32),
]
CASE_NAMES = (
    "data-missing-chains",
    "mount-boundaries",
    "symlink-and-nondir",
    "witness-races",
    "eacces",
    "custody-and-group",
)
# The product's debug-only fixture seams used here, with the marker file each
# writes. A firing is an observation of the fixture, never a gate.
EACCES_VAR, EACCES_MARKER = "OXIDE_TEST_PRUNE_EACCES", "prune-eacces-fired"
DELAY_VAR, DELAY_MARKER = "OXIDE_TEST_PRUNE_DIAGNOSTIC_DELAY", "prune-diagnostic-delay-fired"
CUSTODY_VAR, CUSTODY_MARKER = (
    "OXIDE_TEST_PRUNE_UNKNOWN_CUSTODY",
    "prune-unknown-custody-fired",
)
# The product's own words for each refusal this proof expects: exact source
# sentences, never loose keywords.
PRESENT_KEEP = "the checkout is present, so its root is kept"
LEAF_ONLY_REFUSAL = "an ancestor of the checkout below its mount is missing"
NO_RECORDED_VOLUME = "a /Volumes checkout has no recorded volume UUID"
NOT_A_DIRECTORY = "an ancestor of the checkout is not a directory"
WITNESS_REFUSAL = "the checkout's ancestry changed after the real run's snapshot"
UNKNOWN_CUSTODY = "writer custody is unknown (injected fixture)"
UNREADABLE = "Permission denied"


def require(value, message):
    if not value:
        raise RuntimeError(message)


def require_batch_refusal(doc, expected):
    """The whole batch was kept and the named fault is what refused it."""
    require(
        doc["coverage"]["complete"] is False and not doc["candidates"],
        f"the batch was not kept: {doc['coverage']} {doc['candidates']}",
    )
    reasons = [item["reason"] for item in doc["kept"]]
    require(
        any(expected in reason for reason in reasons),
        f"the batch refusal did not name {expected!r}: {reasons}",
    )


def mount_branch(path: str) -> str:
    """Which absence branch an exact absolute checkout path takes.

    A mirror of the product's component classifier, used only so a case can
    prove it exercises the branch it intends: the same missing chain under
    /Volumes and under the boot Data alias of /Volumes is a MOUNT spelling, so
    an absent mount name is a mount refusal rather than a plain missing chain.
    """
    if not path.startswith("/") or path.endswith("/") or "//" in path:
        return "invalid"
    parts = path.split("/")[1:]
    if any(part in ("", ".", "..") for part in parts):
        return "invalid"
    if parts[:1] == ["Volumes"]:
        return "mount" if len(parts) > 2 else "invalid"
    if parts[:4] == ["System", "Volumes", "Data", "Volumes"]:
        return "mount" if len(parts) > 5 else "invalid"
    return "root"


def require_data_volume(root: str, mount: str) -> None:
    """The fixture premise: fixture checkouts are on the boot Data volume.

    The product proves the same fact from held descriptors and volume UUIDs; the
    fixture only compares devices so a wrong `--root-parent` refuses before any
    effect instead of silently proving nothing.
    """
    data = os.stat("/System/Volumes/Data")
    here = os.stat(root)
    require(
        here.st_dev == data.st_dev,
        f"{root} is not on the boot Data volume; run this proof with "
        "--root-parent /private/tmp (the owner-accepted internal-placement exception)",
    )
    if mount:
        require(
            os.stat(mount).st_dev != data.st_dev,
            f"the attached image {mount} is not a separate device, so it cannot be "
            "the non-Data control",
        )


class Adapter:
    """The existing qualified-session child helper behind World's own API."""

    def __init__(self, proof, binary):
        self.inner = proof
        self.root = str(proof.root)
        self.binary = str(binary)
        self.env = {}
        self.serial = 0
        self.unclosed = set()

    def run(self, argv, cwd=None, env=None, timeout=120):
        self.serial += 1
        code, out, err = self.inner.run(
            argv, cwd=cwd, env=env, label=f"prune135-{self.serial:03d}", limit=timeout
        )
        return subprocess.CompletedProcess(argv, code, out.encode(), err.encode())


def chain_checkout(world, path: Path) -> str:
    """One tiny Git checkout at an ARBITRARY path, so its ancestors can vanish
    together later. Its `target` is asserted absent before its adopt."""
    path.mkdir(parents=True)
    (path / "src").mkdir()
    (path / "Cargo.toml").write_text(PLAIN)
    (path / "src" / "lib.rs").write_text("pub fn v() -> u32 { 1 }\n")
    world.proof.run(["git", "init", "-q"], cwd=str(path), timeout=30)
    (path / ".gitignore").write_text("/target\n/.oxide-outputs/\n")
    require(not os.path.lexists(path / "target"), f"{path} already has a target")
    return os.path.realpath(path)


def listed_legacy_root(world, marker_template: dict, checkout: str, label: str) -> tuple:
    """One operator-listed receiptless legacy root (root/1, no nonce, no
    recorded checkout volume) whose marker names `checkout`. Only one leaf is
    created: the container is the product's own owner-only oxide-outputs
    directory."""
    checkout_id = hashlib.sha256(checkout.encode()).hexdigest()
    root = os.path.join(world.mount, "oxide-outputs", f"{label}-{checkout_id[:8]}")
    os.mkdir(root, 0o700)
    document = dict(marker_template)
    document.pop("nonce", None)
    document.pop("checkout_volume_uuid", None)
    document.update(
        version="oxide.outputs.root/1",
        checkout=checkout,
        checkout_id=checkout_id,
        generation=1,
        label=label,
    )
    Path(root, ".oxide-output.json").write_text(json.dumps(document))
    Path(root, "payload").write_bytes(b"legacy-payload" * 512)
    return root, checkout_id


def operator_list(world, roots: list, name: str) -> str:
    path = os.path.join(world.root, f"operator-list-{name}.json")
    Path(path).write_text(json.dumps(roots))
    return path


def vanished(world, path: str) -> str:
    """Move a whole ancestor subtree away, so the recorded checkout path and
    every component below it are missing while its parent stays on Data."""
    away = os.path.join(world.gone, os.path.basename(path) + "-" + secrets.token_hex(3))
    os.rename(path, away)
    require(not os.path.lexists(path), f"{path} still exists after vanishing")
    return away


def fired(path: Path, expected: str) -> None:
    require(path.exists(), f"{path.name} never fired")
    recorded = path.read_text(errors="replace").strip().splitlines()
    require(
        recorded and recorded[-1] == expected,
        f"{path.name} did not record {expected}: {recorded}",
    )


def armed_real_run(world, plan: str, sha: str, extra: dict):
    """Start one real run in the supervisor's session, so the proof can mutate
    the fixture while the product holds its own snapshot."""
    env = dict(world.proof.env)
    env.update(extra)
    argv = [world.proof.binary, "outputs", "prune", "--plan", plan, "--plan-sha256", sha]
    return permanent.start_child(world.proof, argv, world.root, env)


def await_firing(world, marker: Path, child, label: str) -> None:
    """Wait, bounded by the supervisor's own deadline, for the fixture seam to
    fire. An early exit is reported with its custody, never silently skipped."""
    deadline = time.monotonic() + min(30.0, permanent.work_left())
    while not marker.exists():
        if child.poll() is not None:
            code, out, err = world.finish(child, label)
            raise RuntimeError(f"{label} ended before {marker.name} fired (exit {code}): {out}{err}")
        if time.monotonic() >= deadline:
            raise RuntimeError(f"{label} never reached {marker.name}")
        time.sleep(0.05)


def finish_armed(world, child, proof, label: str):
    """Finish one armed child; a child that will not be reaped keeps closure
    unknown and blocks the ordinary detach."""
    if child.poll() is None:
        child.kill()
        world.finish(child, f"{label} (killed)")
        proof.closure_known = False
        raise RuntimeError(f"{label} did not end and was killed; closure is unknown")


def verified_seed_source(home: Path) -> dict:
    """Resolve the two account-home links, then positively bind one real root.

    The strict seeder still opens that root and every descendant no-follow on
    the same device. This function never writes to the host cache.
    """
    if not home.is_absolute():
        raise RuntimeError("the host Cargo home is not absolute")
    registry = (home / "registry").resolve(strict=True)
    git = (home / "git").resolve(strict=True)
    require(
        registry.name == "registry" and git.name == "git",
        "the host Cargo cache has an unexpected layout",
    )
    require(
        registry.parent == git.parent,
        "the registry and Git caches do not share one canonical root",
    )
    root = registry.parent
    require(
        root.is_absolute() and root.resolve(strict=True) == root,
        "the Cargo cache root is not canonical",
    )
    identity = {}
    with wrapper.offline_seed.Anchor(root, "canonical host Cargo cache") as anchor:
        root_metadata = os.lstat(root)
        require(
            stat.S_ISDIR(root_metadata.st_mode)
            and (root_metadata.st_dev, root_metadata.st_ino) == anchor.identity,
            "the Cargo cache root changed while binding",
        )
        identity["root"] = list(anchor.identity)
        for name, path in (("registry", registry), ("git", git)):
            metadata = os.lstat(path)
            require(
                stat.S_ISDIR(metadata.st_mode) and metadata.st_dev == anchor.dev,
                f"{name} is not a real directory on the cache device",
            )
            descriptor = anchor.open_dir((name,))
            try:
                opened = os.fstat(descriptor)
                require(
                    (opened.st_dev, opened.st_ino) == (metadata.st_dev, metadata.st_ino),
                    f"{name} changed while binding the cache",
                )
                followed = os.stat(home / name)
                require(
                    (followed.st_dev, followed.st_ino) == (opened.st_dev, opened.st_ino),
                    f"{name} account-home link changed",
                )
                identity[name] = [opened.st_dev, opened.st_ino]
            finally:
                os.close(descriptor)
    return {"root": str(root), "home": str(home), "identity": identity}


def run_cases(proof, custody, diagnostic, cases):
    host_home = Path(os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo"))
    source = verified_seed_source(host_home)
    oxide, provenance = wrapper.seed_and_stage(proof, jobs=2, host_cargo_home=source["root"])
    if verified_seed_source(host_home) != source:
        raise RuntimeError("the canonical Cargo cache identity changed during staging")
    provenance["seed_source"] = source
    (proof.evidence / "prune135-provenance.json").write_text(json.dumps(provenance, indent=2))
    adapter = Adapter(proof, oxide)
    world = prior.World(adapter, adapter.root)
    home, cargo_home = proof.root / "home", proof.root / "fixture-cargo-home"
    home.mkdir(mode=0o700)
    cargo_home.mkdir(mode=0o700)
    adapter.env = permanent.proof_env(world.pool, str(home), str(cargo_home))
    adapter.env["CARGO_BUILD_JOBS"] = "2"
    adapter.env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    adapter.env["RUSTFLAGS"] = ""
    # The premise refuses BEFORE any pool, image or child.
    require_data_volume(adapter.root, "")
    boot = world.checkout("bootstrap")
    code, _, err = world.oxide(["check"], "private pool bootstrap", boot)
    require(code == 0, err)
    custody.allocate(str(proof.evidence), str(proof.root))
    world.mount = custody.attach()
    require_data_volume(adapter.root, world.mount)
    code, out, err = world.oxide(
        [
            "pool",
            "register-removable",
            world.mount,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ],
        "private register",
        boot,
    )
    require(code == 0 and "registered removable pool" in out, err)

    chains = Path(adapter.root) / "chains"
    chains.mkdir(mode=0o700)

    def adopt_chain(tail: tuple) -> dict:
        """One tiny Git checkout at `chains/<token>/<tail...>`, adopted while
        its ancestors are present. `token` is the chain root a case may move
        away, so the recorded checkout path keeps its exact bytes while every
        component below the token becomes missing."""
        token = chains / secrets.token_hex(4)
        path = token
        for component in tail:
            path = path / component
        checkout = chain_checkout(world, path)
        world.adopt(checkout)
        return {"checkout": checkout, "path": path, "token": token}

    group = adopt_chain(("a", "b", "checkout"))
    live = adopt_chain(("checkout",))
    kept_group = adopt_chain(("checkout",))
    recreate_race = adopt_chain(("checkout",))
    replace_race = adopt_chain(("anc", "child", "checkout"))
    custody_case = adopt_chain(("checkout",))
    image = {"checkout": world.checkout("image-chain", base=world.mount)}
    world.adopt(image["checkout"])
    marker_template = prior.marker(world.selected(group["checkout"])["root"])
    # A receiptless legacy root whose checkout has TWO missing ancestors plus
    # the missing leaf, and which never existed as a checkout at all.
    legacy_checkout = os.path.join(
        adapter.root, "chains", secrets.token_hex(4), "x", "y", "checkout"
    )
    legacy_root, _ = listed_legacy_root(world, marker_template, legacy_checkout, "legacy")
    # The debug-only EACCES control: an existing directory the walk must read,
    # with the same shape of missing chain below it.
    listed_dir = Path(adapter.root) / "listed"
    listed_dir.mkdir(mode=0o700)
    eacces_target = str(listed_dir)
    eacces_checkout = str(listed_dir / "eacces" / "chain" / "checkout")
    eacces_root, _ = listed_legacy_root(world, marker_template, eacces_checkout, "eacces")
    # A symlinked ancestor and a non-directory component, each its own fixture.
    links = Path(adapter.root) / "links"
    (links / "real").mkdir(parents=True, mode=0o700)
    os.symlink(links / "real", links / "parent")
    symlink_checkout = str(links / "parent" / "checkout")
    symlink_root, _ = listed_legacy_root(world, marker_template, symlink_checkout, "symlink")
    files = Path(adapter.root) / "files"
    files.mkdir(mode=0o700)
    (files / "plain").write_bytes(b"not a directory")
    file_checkout = str(files / "plain" / "checkout")
    file_root, _ = listed_legacy_root(world, marker_template, file_checkout, "file")
    # Two absent-mount controls, one per /Volumes spelling, under a mount name
    # unique to this run that is never mounted.
    absent_mount = "Ox135Absent" + secrets.token_hex(3)
    plain_mount_checkout = f"/Volumes/{absent_mount}/deep/checkout"
    plain_mount_root, _ = listed_legacy_root(world, marker_template, plain_mount_checkout, "mount")
    alias_mount_checkout = f"/System/Volumes/Data/Volumes/{absent_mount}/deep/checkout"
    alias_mount_root, _ = listed_legacy_root(world, marker_template, alias_mount_checkout, "aliased")

    roots = {
        key: world.selected(value["checkout"])["root"]
        for key, value in (
            ("group", group),
            ("live", live),
            ("kept_group", kept_group),
            ("recreate_race", recreate_race),
            ("replace_race", replace_race),
            ("custody", custody_case),
            ("image", image),
        )
    }
    members = sorted(
        row["member"]
        for row in world.registry_doc()["rows"]
        if row["root"] == roots["group"] and row["selected"]
    )
    require(members == ["debug", "release"], f"the profile group premise is {members}")
    for key in ("group", "live", "custody"):
        Path(roots[key], "payload").write_bytes(b"x" * 4096)
    live_before = prior.tree_snapshot(roots["live"])
    live_identity = (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino)
    tainted = False

    def case(name, body):
        nonlocal tainted
        if tainted:
            cases.append(
                {"case": name, "state": "skipped", "reason": "earlier fixture state uncertain"}
            )
            return
        cases.append({"case": name, "state": "reached"})
        try:
            body()
            cases[-1]["state"] = "completed"
        except BaseException as error:
            cases[-1].update(state="failed", reason=f"{type(error).__name__}: {error}")
            tainted = True
            if (
                not diagnostic
                or isinstance(error, (prior.Aborted, KeyboardInterrupt, SystemExit))
                or not proof.closure_known
            ):
                raise

    def data_missing_chains():
        # The checkout is present first: the same root is then kept for the
        # ordinary reason, which is this case's non-vacuous control.
        _, out, _ = world.dry_run("present group", boot)
        present = json.loads(out)
        require(world.kept(present, roots["group"]) == PRESENT_KEEP, str(present["kept"]))
        vanished(world, str(group["token"]))
        # The receiptless root needs the exact list: without it, it is not even
        # considered, so the batch is exactly the registered candidate.
        _, out, _ = world.dry_run("no list", boot)
        without_list = json.loads(out)
        require(
            without_list["coverage"]["complete"]
            and world.candidates(without_list) == {roots["group"]},
            f"the listless batch is not exactly the registered candidate: {without_list}",
        )
        listed = operator_list(world, [legacy_root], "data")
        plan, sha, doc = world.plan_file("data chains", boot, listed)
        require(
            world.candidates(doc) == {roots["group"], legacy_root},
            f"a boot Data missing chain was not a candidate: {doc['candidates']} {doc['kept']}",
        )
        require(
            world.kept(doc, roots["live"]) == PRESENT_KEEP,
            f"the live checkout was not kept: {doc['kept']}",
        )
        code, out, err = world.real_run("positive prune", boot, plan, sha, listed)
        require(code == 0, err)
        for root in (roots["group"], legacy_root):
            require(not os.path.lexists(root), f"{root} survived the positive prune")
        for checkout in (group["checkout"], legacy_checkout):
            receipt = permanent.read_json(permanent.receipt_file(world.pool, checkout))
            require(
                receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted",
                str(receipt),
            )
        entries = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))["entries"]
        require(
            {roots["group"], legacy_root}.issubset({entry["root"] for entry in entries}),
            f"a pruned root has no tombstone: {entries}",
        )
        require(not world.intents() and not world.quarantines(), "an intent or quarantine remains")
        require(
            prior.tree_snapshot(roots["live"]) == live_before
            and (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino) == live_identity,
            "the kept live root changed",
        )
        require("positively absent" in out, out)

    case("data-missing-chains", data_missing_chains)

    def mount_boundaries():
        payload = prior.tree_snapshot(roots["image"])
        checkouts = os.path.join(world.mount, "checkouts")
        away = os.path.join(world.mount, "gone-checkouts")
        os.rename(checkouts, away)
        try:
            _, out, _ = world.dry_run("image missing ancestor", boot)
            require_batch_refusal(json.loads(out), LEAF_ONLY_REFUSAL)
            require(prior.tree_snapshot(roots["image"]) == payload, "the kept image root changed")
        finally:
            # Restored: the row is an ordinary present-checkout keep again, so
            # no later batch inherits an unprovable absence.
            os.rename(away, checkouts)
        for label, spelling, root in (
            ("absent mount", plain_mount_checkout, plain_mount_root),
            ("aliased absent mount", alias_mount_checkout, alias_mount_root),
        ):
            require(mount_branch(spelling) == "mount", f"{spelling} is not a mount spelling")
            untouched = prior.tree_snapshot(root)
            listed = operator_list(world, [root], "mount")
            _, out, _ = world.dry_run(label, boot, listed)
            doc = json.loads(out)
            require_batch_refusal(doc, NO_RECORDED_VOLUME)
            require(prior.tree_snapshot(root) == untouched, f"the {label} root changed")
            require(not doc["candidates"], f"{label}: a mount chain became a candidate")

    case("mount-boundaries", mount_boundaries)

    def symlink_and_nondir():
        for label, spelling, root in (
            ("symlinked ancestor", symlink_checkout, symlink_root),
            ("non-directory component", file_checkout, file_root),
        ):
            untouched = prior.tree_snapshot(root)
            listed = operator_list(world, [root], label)
            _, out, _ = world.dry_run(label, boot, listed)
            require_batch_refusal(json.loads(out), NOT_A_DIRECTORY)
            require(prior.tree_snapshot(root) == untouched, f"the {label} root changed")
            require(not world.intents() and not world.quarantines(), f"{label} left an intent")

    case("symlink-and-nondir", symlink_and_nondir)

    def witness_races():
        # (a) The checkout recreated after the real run's own snapshot. A fresh
        # absence proof already sees it present, so this proves the lock-time
        # revalidation rather than the witness comparison.
        vanished(world, str(recreate_race["token"]))
        plan, sha, doc = world.plan_file("recreate plan", boot)
        require(
            world.candidates(doc) == {roots["recreate_race"]},
            f"the recreate race fixture is not one candidate: {doc['candidates']}",
        )
        before = prior.tree_snapshot(roots["recreate_race"])
        child = armed_real_run(world, plan, sha, {DELAY_VAR: roots["recreate_race"]})
        try:
            await_firing(world, proof.evidence / DELAY_MARKER, child, "recreate race")
            os.makedirs(recreate_race["checkout"], exist_ok=True)
            code, out, err = world.finish(child, "recreate race")
        finally:
            finish_armed(world, child, proof, "recreate race")
        require(code != 0, f"a recreated checkout was pruned: {out}{err}")
        require(PRESENT_KEEP in err, f"the recreated checkout was not refused as present: {err}")
        require(prior.tree_snapshot(roots["recreate_race"]) == before, "the refused root changed")
        require(not world.intents() and not world.quarantines(), "the recreate race left an intent")
        # (b) An EXISTING ancestor replaced by a DIFFERENT empty directory while
        # the checkout stays absent. A fresh absence proof still succeeds, so
        # only the real run's own witness comparison can refuse this.
        vanished(world, str(replace_race["path"].parent))
        plan, sha, doc = world.plan_file("replace plan", boot)
        require(
            world.candidates(doc) == {roots["replace_race"]},
            f"the replace race fixture is not one candidate: {doc['candidates']}",
        )
        before = prior.tree_snapshot(roots["replace_race"])
        ancestor = str(replace_race["path"].parent.parent)
        child = armed_real_run(world, plan, sha, {DELAY_VAR: roots["replace_race"]})
        try:
            await_firing(world, proof.evidence / DELAY_MARKER, child, "replace race")
            os.rename(ancestor, ancestor + "-away")
            os.mkdir(ancestor, 0o700)
            code, out, err = world.finish(child, "replace race")
        finally:
            finish_armed(world, child, proof, "replace race")
        require(code != 0, f"a replaced ancestor was pruned: {out}{err}")
        require(WITNESS_REFUSAL in err, f"the witness comparison did not refuse: {err}")
        require(prior.tree_snapshot(roots["replace_race"]) == before, "the refused root changed")
        require(not world.intents() and not world.quarantines(), "the replace race left an intent")

    case("witness-races", witness_races)

    def eacces():
        listed = operator_list(world, [eacces_root], "eacces")
        _, out, _ = world.dry_run("uninjected control", boot, listed)
        control = json.loads(out)
        require(
            control["coverage"]["complete"] and eacces_root in world.candidates(control),
            f"the uninjected chain was not proven absent: {control['kept']}",
        )
        untouched = prior.tree_snapshot(eacces_root)
        _, out, _ = world.dry_run("eacces", boot, listed, {EACCES_VAR: eacces_target})
        require_batch_refusal(json.loads(out), UNREADABLE)
        fired(proof.evidence / EACCES_MARKER, eacces_target)
        require(prior.tree_snapshot(eacces_root) == untouched, "the unreadable-ancestor root changed")
        require(not world.intents() and not world.quarantines(), "the EACCES refusal left an intent")

    case("eacces", eacces)

    def custody_and_group():
        vanished(world, str(custody_case["token"]))
        authority = world.authority(kept_group["checkout"])
        payload = prior.tree_snapshot(roots["kept_group"])
        _, out, _ = world.dry_run(
            "unknown custody", boot, None, {CUSTODY_VAR: custody_case["checkout"]}
        )
        require_batch_refusal(json.loads(out), UNKNOWN_CUSTODY)
        fired(proof.evidence / CUSTODY_MARKER, custody_case["checkout"])
        require(not world.intents() and not world.quarantines(), "a custody refusal left an intent")
        require(prior.tree_snapshot(roots["kept_group"]) == payload, "the kept group root changed")
        require(
            world.authority(kept_group["checkout"]) == authority,
            "the kept group's receipt or registry changed",
        )
        require(os.path.isdir(roots["custody"]), "the custody fixture root vanished")
        group = [
            row
            for row in world.registry_doc()["rows"]
            if row["root"] == roots["kept_group"] and row["selected"]
        ]
        require(
            sorted(row["member"] for row in group) == ["debug", "release"],
            f"the kept root's group rows changed: {group}",
        )
        require(
            prior.tree_snapshot(roots["live"]) == live_before
            and (os.lstat(roots["live"]).st_dev, os.lstat(roots["live"]).st_ino) == live_identity,
            "the live checkout root changed",
        )

    case("custody-and-group", custody_and_group)
    require(not adapter.unclosed, f"a proof child was never reaped: {sorted(adapter.unclosed)}")
    return cases


def self_check() -> int:
    """Pure apparatus check: no child, no scratch, no pool, no image.

    It drives the real oracles, the branch classifier and the case/allocation
    table with in-memory counterfactuals, so a fixture that names the wrong
    refusal, the wrong branch or the wrong seam cannot pass.
    """
    require(
        sum(item[1] for item in ALLOCATION_SECONDS) == TOTAL_SECONDS,
        "the prospective time allocation does not sum to the published limit",
    )
    require(
        sum(item[1] for item in ALLOCATION_MIB) == TOTAL_MIB,
        "the prospective scratch allocation does not sum to the published budget",
    )
    allocated = {name for name, _ in ALLOCATION_SECONDS}
    require(
        all(name in allocated for name in CASE_NAMES),
        "a case has no time allocation",
    )
    require(all(seconds > 0 for _, seconds in ALLOCATION_SECONDS), "a phase has no time")
    # A different incomplete-authority reason never satisfies an oracle, even
    # though it also keeps the whole batch.
    for expected in (LEAF_ONLY_REFUSAL, NO_RECORDED_VOLUME, NOT_A_DIRECTORY, UNKNOWN_CUSTODY):
        doc = {
            "coverage": {"complete": False},
            "candidates": [],
            "kept": [
                {"reason": "the registry snapshot is incomplete (unrelated); the whole batch is kept"}
            ],
        }
        try:
            require_batch_refusal(doc, expected)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"an unrelated fault satisfied {expected}")
        doc["kept"][0][
            "reason"
        ] = f"the registry snapshot is incomplete ({expected}); the whole batch is kept"
        require_batch_refusal(doc, expected)
    for doc in (
        {"coverage": {"complete": True}, "candidates": [], "kept": [{"reason": LEAF_ONLY_REFUSAL}]},
        {
            "coverage": {"complete": False},
            "candidates": [{"root": "/x"}],
            "kept": [{"reason": LEAF_ONLY_REFUSAL}],
        },
        {"coverage": {"complete": False}, "candidates": [], "kept": [{"reason": "kept"}]},
    ):
        try:
            require_batch_refusal(doc, LEAF_ONLY_REFUSAL)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"a non-refusing batch passed: {doc}")
    # Both /Volumes spellings are mounts, including an absent mount name; the
    # mount root itself and every non-normal spelling are invalid, and a plain
    # path is not a mount.
    for spelling, branch in (
        ("/Volumes/Ox135Absent/deep/checkout", "mount"),
        ("/System/Volumes/Data/Volumes/Ox135Absent/deep/checkout", "mount"),
        ("/Volumes/Ox135Absent", "invalid"),
        ("/System/Volumes/Data/Volumes/Ox135Absent", "invalid"),
        ("/Volumes", "invalid"),
        ("/private/tmp/chains/token/a/checkout", "root"),
        ("/System/Volumes/Data/x", "root"),
        ("relative/checkout", "invalid"),
        ("/private//tmp/x", "invalid"),
        ("/private/tmp/x/", "invalid"),
        ("/private/./tmp/x", "invalid"),
        ("/private/tmp/../tmp/x", "invalid"),
    ):
        require(mount_branch(spelling) == branch, f"{spelling} was not classified {branch}")
    # The two races must be distinguishable: a recreated checkout and a replaced
    # ancestor cannot be refused by the same sentence, and the fixture seams
    # must not alias.
    require(
        WITNESS_REFUSAL != PRESENT_KEEP and WITNESS_REFUSAL not in PRESENT_KEEP,
        "the two race refusals must be distinguishable sentences",
    )
    require(EACCES_VAR != DELAY_VAR and EACCES_MARKER != DELAY_MARKER, "the seams alias")
    require(CUSTODY_VAR not in (EACCES_VAR, DELAY_VAR), "the custody seam aliases")
    print("prune absence self-check: OK")
    return 0


def main():
    if sys.argv[1:] == ["--self-check"]:
        return self_check()
    diagnostic = sys.argv[1:] == ["--diagnostic"]
    if sys.argv[1:] not in ([], ["--diagnostic"]):
        raise SystemExit("expected no argument or --diagnostic")
    proof = wrapper.Proof()
    require(wrapper.git(proof, "status", "--porcelain") == "", "source is not clean")
    custody = prior.ImageCustody("Ox135" + secrets.token_hex(4))
    cases, error = [], None
    try:
        run_cases(proof, custody, diagnostic, cases)
    except BaseException as exc:
        error = exc
        traceback.print_exc()
    finally:
        disposition = custody.detach_once(sessions_closed=proof.closure_known)
    recorded = {item["case"] for item in cases}
    cases.extend(
        {"case": name, "state": "unexecuted"} for name in CASE_NAMES if name not in recorded
    )
    report = {
        "qualified": False if diagnostic else None,
        "cases": cases,
        "error": None if error is None else f"{type(error).__name__}: {error}",
        "custody": custody.state,
        "detach": disposition,
    }
    if diagnostic:
        (proof.evidence / "prune135-diagnostic.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2), flush=True)
    if error is not None:
        raise error
    require(disposition == "completed", f"the ordinary detach was {disposition}")
    require(all(case["state"] == "completed" for case in cases), "proof incomplete")


if __name__ == "__main__":
    main()
