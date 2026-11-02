#!/usr/bin/env python3
"""Fresh per-row custody witnesses for the prune authority window (oxide#235).

One candidate binary; one private 128 MiB APFS image attached `-owners on`
and submitted for guard-owned detach; no installed binary or live pool. The whole command is
the QUALIFIED candidate runner at its fixed path
(`scripts/bounded-test-candidate.py`, SHA-256 c5782431…, planner decision
9575afc9): the supervisor owns reserve, deadline, scratch accounting and
closure. No session or signal timer is introduced here. The private
CARGO_HOME is seeded by offline_seed from this HEAD's lock (including O1's
pinned Barge source), then the supervised staging helper uses six Cargo jobs.
Fixture builds run sequentially with at most two jobs.

Placement (the only mode): no `--root-parent`; the pool sits in a private
scratch-pools directory under the supervisor run root on the selected volume,
declared as OXIDE_SCRATCH_ROOT with an explicit 64 MiB cap.

Bounds: 360 s wall and 512 MiB monitored scratch including staging and
cleanup (fixed brief bounds; fit unmeasured). Native runs need independent
source-fit review and separate owner activation; the counted run is fail-fast
and `--diagnostic` is the uncounted keep-going twin that never passes.

What the proof establishes, per the frozen design (workflow.md "Fresh custody
reads", oxide#229):

- the read-only route is truly no-effect: a dry run changes no byte, name,
  mode, inode or timestamp of the control tree, creates no admission lock or
  state document, and leaves every checkout's member links and their targets
  untouched;
- custody is recomputed per row from freshly witnessed durable states under
  both admission locks: absent-checkout roots become candidates with a
  registered, proved sibling; a real running writer keeps its own root and
  only its own root; a writer recorded between windows is seen by the next
  decision;
- the metadata witness forces a re-read: an in-place mutation of the same
  state inode is seen on the next row even though device and inode match,
  which a dev/ino-only cache would miss;
- both existing locks are taken internal-first, non-blocking, without
  creating a missing lock: a busy internal or removable lock refuses the
  whole batch (naming the pool), releases the first lock, and the next
  window succeeds;
- a registered sibling without a provable control, lock or state document is
  unknown, never empty: missing, invalid and oversized sibling states refuse
  the whole batch;
- unchanged-state scaling: a synthetic 709-root / 1189-row registry at the
  historical state sizes (about 5 KiB internal, about 150 KiB removable)
  completes a complete preview, where a per-row state re-read route would
  charge about 113 MB against the 64 MiB aggregate budget;
- one synthetic eligible effect and its reproof under private fixture
  authority: a real run deletes exactly the witnessed candidates, then the
  same plan is refused with the eligible set changed and nothing else moved.

`--plan` prints the case table, bounds and the nine premise dispositions with
no effect. `--self-check` is the pure pin: case tables and counterfactual
oracles, the scratch layout and declaration, the scale arithmetic, the
source-literal pins (writer phases, control paths, snapshot bounds), the plan
document and the exact effigy selector text, including the qualified runner
path. Fixture identity and provenance are recorded in the evidence directory.
"""
from __future__ import annotations
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import stat
import subprocess
import sys
import time

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent

# The qualified runner this selector must be the whole command of, and the
# ordinary default it must not replace (planner decision 9575afc9).
CANDIDATE_RUNNER = "scripts/bounded-test-candidate.py"
CANDIDATE_RUNNER_SHA256 = (
    "c578243126a2470cde0dfd19530254b3ec61b4ea5bb1fa43c6bbd2d41853c2b7"
)
DEFAULT_RUNNER = "scripts/bounded-test.py"
DEFAULT_RUNNER_SHA256 = (
    "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
)


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


wrapper = load("proof_wrapper235", "test-oxide-wrapper-bypass.py")
prior = load("proof_prune235", "test-outputs-prune.py")
permanent = prior._load("permanent235", "test-permanent-outputs.py")
prior.permanent = permanent
prior.expect = permanent.expect
prior.fail = permanent.fail
prior.DIAG = prior.Diagnostics(enabled=False)

# The fixed case table; order is the execution order and a Collector refuses
# any case outside it or out of order.
MODE_CASES = {
    "builds": (
        "baseline-no-effect",
        "absent-custody",
        "dynamic-writer",
        "in-place-witness",
        "lock-refusals",
        "unprovable-sibling",
        "large-state-budget",
        "scale-unchanged",
        "effect-reproof",
    ),
}
SELECTORS = {
    "builds": ("rust:prune-fresh-custody", "rust:prune-fresh-custody:diagnostic"),
}
PLACEMENT = {
    "builds": (
        "no --root-parent; OXIDE_SCRATCH_ROOT = <run root>/scratch-pools (0700); "
        "pool <run root>/scratch-pools/pool; OXIDE_POOL_CAP 64MiB; selected-volume scratch"
    ),
}
SCALE_ROOTS = 709
SCALE_ROWS = 1189
SCALE_WRITERS = 650
SNAPSHOT_BUDGET_BYTES = 64 * 1024 * 1024
MANAGED_NAME = "oxide-managed"

# The nine HIGH proof-authoring premise dispositions (workflow.md
# "Proof-authoring premise checklist"), with the executable source location of
# each.
PREMISE = (
    "1 private HOME and rustup: seed_and_stage (test-oxide-wrapper-bypass.py) and "
    "permanent.proof_env (test-permanent-outputs.py) keep the host rustup home, a "
    "private CARGO_HOME, RUSTUP_TOOLCHAIN 1.97.1 and RUSTUP_AUTO_INSTALL=0; no "
    "fixture call uses --locked. Unchanged from the reviewed prune proofs.",
    "2 account-home control: proof_env sets OXIDE_TEST_HOME_FROM_ENV=1 "
    "(test-permanent-outputs.py). The private fixture home is used throughout.",
    "3 environment isolation: proof_env pops inherited OXIDE_, XDG_, CARGO_BUILD_, "
    "CARGO_TARGET_ and GIT_ keys; PruneProof.env drops XDG_ and GIT_; prepare "
    "declares OXIDE_SCRATCH_ROOT and OXIDE_POOL under the run root through "
    "declare_scratch, dropping any inherited value first.",
    "4 automatic adoption: prepare asserts each fixture checkout has no target "
    "before the explicit `outputs adopt`; synthetic scale roots and receipts are "
    "hand-built owned fixture data, never built or adopted.",
    "5 Cargo external argv: not used; no external-subcommand fixture.",
    "6 owned images: publish image custody before attach, then request one "
    "guard-owned detach only after known child closure; uncertainty keeps the backing.",
    "7 compiler flags: seed_and_stage scrubs encoded and plain flags; prepare "
    "sets RUSTFLAGS to empty and pops CARGO_ENCODED_RUSTFLAGS for every fixture "
    "child.",
    "8 temporary storage: PruneProof.env points TMPDIR, TMP and TEMP at the "
    "supervisor root, which sits on the selected volume (no --root-parent).",
    "9 macOS mktemp: not invoked by this script. Every fixture path is "
    "os.path.join under the supervisor root or its scratch-pools directory, "
    "created with mkdir/makedirs; the scratch directory is created 0700.",
)


def require(value, message):
    if not value:
        raise RuntimeError(message)


def check_case(mode, name):
    """Refuse a case outside the mode's own table. Order is checked by Collector."""
    if mode not in MODE_CASES:
        raise RuntimeError(f"unknown mode {mode!r}")
    if name not in MODE_CASES[mode]:
        raise RuntimeError(f"case {name!r} is outside the {mode} table")


def parse_args(argv):
    """Return ("self-check" | "plan" | "run", diagnostic). Refuse anything else."""
    if argv == ["--self-check"]:
        return "self-check", False
    if argv == ["--plan"]:
        return "plan", False
    flags = set(argv)
    if len(argv) != len(flags) or not flags <= {"--diagnostic"}:
        raise SystemExit("expected --self-check, --plan, or at most one --diagnostic")
    return "run", "--diagnostic" in flags


def scratch_layout(task_root: str) -> tuple[str, str]:
    """S4: one private scratch root inside the run root, and the one pool inside it."""
    scratch = os.path.join(task_root, "scratch-pools")
    return scratch, os.path.join(scratch, "pool")


def declare_scratch(env: dict, task_root: str) -> dict:
    """Return a copy of a fixture env with the scratch declaration applied.

    Pure. Any inherited OXIDE_SCRATCH_ROOT is dropped first, so a host value
    can never leak into the fixture."""
    env = dict(env)
    env.pop("OXIDE_SCRATCH_ROOT", None)
    env["OXIDE_POOL_CAP"] = "64MiB"
    scratch, pool = scratch_layout(task_root)
    env["OXIDE_SCRATCH_ROOT"] = scratch
    env["OXIDE_POOL"] = pool
    return env


def place_pool(world, pool: str) -> None:
    """Move a World's pool, and every path derived from it, under the scratch root."""
    world.pool = pool
    world.control = os.path.join(pool, MANAGED_NAME)
    world.outputs = os.path.join(world.control, "outputs")
    world.registry = os.path.join(world.outputs, "registry.json")


def control_paths(world) -> dict:
    """The control-tree paths this proof witnesses, derived from the same
    layout constants the product uses (pinned in --self-check)."""
    removable_control = os.path.join(world.control, "removable", MANAGED_NAME)
    return {
        "internal_lock": os.path.join(world.control, "admission.lock"),
        "internal_state": os.path.join(world.control, "state.json"),
        "registration": os.path.join(world.control, "removable.json"),
        "removable_control": removable_control,
        "removable_lock": os.path.join(removable_control, "admission.lock"),
        "removable_state": os.path.join(removable_control, "state.json"),
    }


def require_control_files(paths: dict) -> None:
    """The already-created fixture inputs must be private regular files.

    This read-only check never creates or repairs a missing control file.
    """
    for name in ("internal_lock", "internal_state", "removable_lock", "removable_state"):
        path = paths[name]
        facts = os.lstat(path)
        require(stat.S_ISREG(facts.st_mode) and facts.st_uid == os.geteuid()
                and stat.S_IMODE(facts.st_mode) == 0o600 and facts.st_nlink == 1,
                f"fixture {name} is not a private regular file: {path}")


def entry_digest(path: str) -> tuple:
    """One entry's observed shape: directories pin mode and identity; regular
    files pin mode, size, content digest and identity; a missing path is
    positively absent."""
    try:
        metadata = os.lstat(path)
    except FileNotFoundError:
        return ("absent",)
    if stat.S_ISDIR(metadata.st_mode):
        return ("dir", stat.S_IMODE(metadata.st_mode), metadata.st_dev, metadata.st_ino)
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digest.update(block)
    return (
        stat.S_IMODE(metadata.st_mode),
        metadata.st_size,
        digest.hexdigest(),
        metadata.st_dev,
        metadata.st_ino,
    )


def tree_digest(root: str) -> dict:
    """Snapshot the complete tree without following links."""
    return prior.owned_tree_snapshot(root)


def links_snapshot(checkouts: list) -> dict:
    """Each checkout's owned member links and their targets, so a refused run
    can be proven to have moved no link and no root."""
    snapshot = {}
    for checkout in checkouts:
        target_dir = os.path.join(checkout, "target")
        snapshot[checkout] = {}
        for member in ("debug", "release"):
            link = os.path.join(target_dir, member)
            facts = os.lstat(link)
            require(stat.S_ISLNK(facts.st_mode), f"expected member symlink: {link}")
            target = os.readlink(link)
            resolved = os.path.realpath(link)
            snapshot[checkout][member] = (
                target, facts.st_dev, facts.st_ino,
                prior.owned_tree_snapshot(resolved) if os.path.isdir(resolved) else entry_digest(resolved),
            )
    return snapshot


def read_json_bounded(path: str, attempts: int = 5):
    """Read one product document, tolerating a concurrent atomic replacement:
    a read that fails to parse is retried a bounded number of times before it
    is an oracle failure."""
    last = None
    for _ in range(attempts):
        try:
            with open(path, "rb") as handle:
                return json.loads(handle.read())
        except (FileNotFoundError, json.JSONDecodeError, OSError) as error:
            last = error
            time.sleep(0.05)
    raise RuntimeError(f"cannot read {path}: {last}")


def atomic_write(path: str, data: bytes, mode: int = 0o600) -> None:
    """One temporary file in the same directory, renamed over the document:
    the replacement shape every cooperative writer uses."""
    temp = f"{path}.custody-tmp-{secrets.token_hex(4)}"
    descriptor = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(data)
    os.chmod(temp, mode)
    os.replace(temp, path)


def image_document(evidence: Path, name: str, body: dict) -> None:
    target = evidence / name
    require(not target.exists(), f"image protocol document already exists: {target}")
    atomic_write(str(target), json.dumps(body).encode())


def request_guard_detach(proof, custody, error):
    """Keep failures as data so the diagnostic receipt is still published."""
    try:
        if error is not None or not proof.closure_known or proof.unclosed or custody.state != "attached":
            return "retained", error
        require(custody.volume_uuid and custody.device and custody.mount, "image identity unknown")
        image_document(proof.evidence, "detach-request.json", {
            "version": 1, "backing": os.path.realpath(custody.backing),
            "mount": custody.mount, "device": custody.device,
            "volume_uuid": custody.volume_uuid.lower(), "note": "fresh custody proof",
        })
        return "requested", None
    except BaseException as exc:
        return "retained", error or exc


def in_place_write(path: str, data: bytes) -> int:
    """Rewrite an existing document on the SAME inode, which no cooperative
    writer ever does: the tamper the metadata witness exists to catch.
    Returns the unchanged inode number."""
    before = os.lstat(path)
    descriptor = os.open(path, os.O_WRONLY)
    try:
        os.ftruncate(descriptor, 0)
        os.write(descriptor, data)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    after = os.lstat(path)
    require(
        (before.st_dev, before.st_ino) == (after.st_dev, after.st_ino),
        "the in-place tamper changed the document's identity",
    )
    return after.st_ino


def custody_record(checkout: str, phase: str, pid: int) -> dict:
    """One synthetic control-state record for a fixture checkout. The phase
    names are the product's own serde spellings, pinned in --self-check."""
    return {
        "id": pid,
        "identity": "identity",
        "compatibility": "compatibility",
        "profile": "quick",
        "checkout": checkout,
        "phase": phase,
        "process_group": None,
        "last_used": 1,
        "holder": {"pid": pid, "started_ms": 1, "lease_holder": "lease"},
        "last_used_ms": 1,
    }


def writer_seen(doc, checkout: str) -> bool:
    """Whether the parsed state records a possibly-writing command for
    `checkout`: the spawning or running phases, exactly as the product's
    custody verdict counts them."""
    for record in doc.get("records", []):
        if record.get("checkout") == checkout and record.get("phase") in ("spawning", "running"):
            return True
    return False


def require_batch_refusal(doc, expected):
    """Every kept root must identify the planted batch fault, and no root is
    a candidate: an incomplete snapshot offers nothing."""
    require(
        not doc["coverage"]["complete"] and not doc["candidates"],
        f"batch was not kept: {doc.get('coverage')}",
    )
    require(
        bool(doc["kept"])
        and all(expected in item["reason"] for item in doc["kept"]),
        f"batch refusal did not identify {expected!r}: {doc['kept'][:2]}",
    )


def require_refusal_names(doc, *parts):
    """The batch refusal must name every given fragment."""
    require(
        not doc["coverage"]["complete"] and not doc["candidates"],
        f"batch was not kept: {doc.get('coverage')}",
    )
    joined = " ".join(item["reason"] for item in doc["kept"])
    for part in parts:
        require(part in joined, f"batch refusal did not name {part!r}: {joined[:400]}")


def selector_run(name: str) -> str:
    """The `run` line of one effigy task, read from the repository's effigy.toml."""
    lines = (HERE.parent / "effigy.toml").read_text(encoding="utf-8").splitlines()
    start = lines.index(f'[tasks."{name}"]')
    for line in lines[start + 1:]:
        if line.startswith("["):
            break
        if line.startswith("run = "):
            return line.strip()[len('run = "'):-1]
    raise RuntimeError(f"{name} has no run line")


EXPECTED_SELECTORS = {
    "rust:prune-fresh-custody": (
        "python3 scripts/bounded-test-candidate.py --limit 360 --scratch-budget-mib 512"
        " -- python3 scripts/test-prune-fresh-custody.py"
    ),
    "rust:prune-fresh-custody:diagnostic": (
        "python3 scripts/bounded-test-candidate.py --limit 360 --scratch-budget-mib 512"
        " -- python3 scripts/test-prune-fresh-custody.py --diagnostic"
    ),
    "proof:prune-fresh-custody:test": "python3 scripts/test-prune-fresh-custody.py --self-check",
    "proof:prune-fresh-custody:plan": "python3 scripts/test-prune-fresh-custody.py --plan",
}


def plan_document() -> dict:
    """The effect-free plan: the mode's table, bounds and premises."""
    return {
        "script": "scripts/test-prune-fresh-custody.py",
        "effects": "none",
        "bounds": {"limit_s": 360, "scratch_budget_mib": 512, "includes": "cleanup"},
        "runner": {
            "qualified_candidate": CANDIDATE_RUNNER,
            "candidate_sha256": CANDIDATE_RUNNER_SHA256,
            "ordinary_default_unchanged": DEFAULT_RUNNER,
            "default_sha256": DEFAULT_RUNNER_SHA256,
        },
        "modes": {
            mode: {
                "selectors": list(SELECTORS[mode]),
                "placement": PLACEMENT[mode],
                "cases": list(MODE_CASES[mode]),
            }
            for mode in MODE_CASES
        },
        "scale_oracle": scale_oracle(),
        "premise_checklist": list(PREMISE),
    }


def scale_oracle() -> dict:
    """The arithmetic that makes the scale case discriminating: one witnessed
    read of each state document fits the aggregate budget, while re-reading
    the states once per custody-reaching row does not."""
    internal_bytes = 96 * 1024
    removable_bytes = 150 * 1024
    per_row = internal_bytes + removable_bytes
    old_route = SCALE_ROOTS * per_row
    new_route = internal_bytes + removable_bytes + SCALE_ROOTS * 700
    return {
        "roots": SCALE_ROOTS,
        "rows": SCALE_ROWS,
        "writer_keeps": SCALE_WRITERS,
        "estimated_internal_state_bytes": internal_bytes,
        "estimated_removable_state_bytes": removable_bytes,
        "old_route_per_row_re-read_bytes": old_route,
        "old_route_exceeds_budget": old_route > SNAPSHOT_BUDGET_BYTES,
        "witnessed_route_bytes": new_route,
        "witnessed_route_fits_budget": new_route < SNAPSHOT_BUDGET_BYTES,
        "budget_bytes": SNAPSHOT_BUDGET_BYTES,
    }


class PruneProof(wrapper.Proof):
    def __init__(self):
        super().__init__()
        self.unclosed = set()
        self.mount = None

    def env(self, extra=None):
        env = super().env(extra)
        for key in list(env):
            if key.startswith(("XDG_", "GIT_")):
                env.pop(key)
        return env

    def run(self, argv, **kwargs):
        code, out, err = super().run(argv, **kwargs)
        try:
            permanent.require_normal_exit(str(argv[0]), code)
            permanent.require_confirmed_closure(str(argv[0]), err)
        except permanent.ProofAbort:
            self.closure_known = False
            raise
        return code, out, err


class Adapter:
    """Use the existing qualified-session child helper with World's API."""
    def __init__(self, proof, binary):
        self.inner = proof
        self.root = str(proof.root)
        self.binary = str(binary)
        self.env = {}
        self.serial = 0

    def run(self, argv, cwd=None, env=None, timeout=120):
        self.serial += 1
        code, out, err = self.inner.run(
            argv, cwd=cwd, env=env, label=f"prune235-{self.serial:03d}", limit=timeout
        )
        return subprocess.CompletedProcess(argv, code, out.encode(), err.encode())


class Collector:
    """One ordered case stream. A failure taints later fixture state; a
    diagnostic keeps going only through dependency-independent cases and
    aborts on unknown closure."""

    def __init__(self, proof, mode, diagnostic, cases):
        self.proof = proof
        self.mode = mode
        self.diagnostic = diagnostic
        self.cases = cases
        self.tainted = False
        self.position = 0

    def case(self, name, body):
        check_case(self.mode, name)
        expected = MODE_CASES[self.mode][self.position]
        require(name == expected, f"case {name!r} out of order; the table expects {expected!r}")
        self.position += 1
        if self.tainted:
            self.cases.append(
                {"case": name, "state": "skipped", "reason": "earlier fixture state uncertain"}
            )
            return
        self.cases.append({"case": name, "state": "reached"})
        try:
            body()
            permanent.require_case_custody(
                unclosed=self.proof.unclosed, mount=self.proof.mount,
                registration=self.proof.registration, case=name,
            )
            self.cases[-1]["state"] = "completed"
        except BaseException as error:
            self.cases[-1].update(state="failed", reason=f"{type(error).__name__}: {error}")
            self.tainted = True
            try:
                permanent.require_case_custody(
                    unclosed=self.proof.unclosed, mount=self.proof.mount,
                    registration=self.proof.registration, case=name,
                )
            except BaseException:
                self.proof.closure_known = False
                raise
            if (
                not self.diagnostic
                or isinstance(error, (prior.Aborted, permanent.ProofAbort, KeyboardInterrupt, SystemExit))
                or not self.proof.closure_known
                or self.proof.unclosed
            ):
                raise


def prepare(proof, custody):
    """Shared staging, bootstrap, image attach and registration."""
    host_home = Path(
        os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo")
    )
    registry = (host_home / "registry").resolve(strict=True)
    git = (host_home / "git").resolve(strict=True)
    if registry.name != "registry" or git.name != "git" or registry.parent != git.parent:
        raise RuntimeError("registry and git do not share one canonical Cargo cache root")
    source = str(registry.parent)
    oxide, provenance = wrapper.seed_and_stage(proof, jobs=6, host_cargo_home=source)
    provenance["seed_source"] = source
    provenance["head"] = wrapper.git(proof, "rev-parse", "HEAD")
    provenance["proof_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    provenance["manifest_sha256"] = hashlib.sha256((HERE.parent / "effigy.toml").read_bytes()).hexdigest()
    provenance["candidate_runner_sha256"] = hashlib.sha256(
        (HERE.parent / CANDIDATE_RUNNER).read_bytes()
    ).hexdigest()
    provenance["default_runner_sha256"] = hashlib.sha256(
        (HERE.parent / DEFAULT_RUNNER).read_bytes()
    ).hexdigest()
    require(
        provenance["candidate_runner_sha256"] == CANDIDATE_RUNNER_SHA256,
        "the qualified candidate runner bytes changed",
    )
    require(
        provenance["default_runner_sha256"] == DEFAULT_RUNNER_SHA256,
        "the ordinary default runner bytes changed",
    )
    (proof.evidence / "prune235-provenance.json").write_text(
        json.dumps(provenance, indent=2)
    )
    adapter = Adapter(proof, oxide)
    world = prior.World(adapter, adapter.root)
    home, cargo_home = proof.root / "home", proof.root / "fixture-cargo-home"
    home.mkdir(mode=0o700)
    cargo_home.mkdir(mode=0o700)
    scratch, pool = scratch_layout(adapter.root)
    os.mkdir(scratch, 0o700)
    place_pool(world, pool)
    adapter.env = declare_scratch(permanent.proof_env(world.pool, str(home), str(cargo_home)),
                                  adapter.root)
    require(adapter.env.get("OXIDE_POOL") == world.pool, "fixture pool is not the placed pool")
    require(
        adapter.env.get("OXIDE_SCRATCH_ROOT") == scratch,
        "fixture scratch declaration drifted",
    )
    adapter.env["CARGO_BUILD_JOBS"] = "2"
    adapter.env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    adapter.env["RUSTFLAGS"] = ""
    boot = world.checkout("bootstrap")
    code, _, err = world.oxide(["check"], "private pool bootstrap", boot)
    require(code == 0, err)
    custody.allocate(str(proof.evidence), str(proof.root))
    image_document(proof.evidence, "image-custody.json",
                   {"version": 1, "backings": [os.path.realpath(custody.backing)]})
    world.mount = custody.attach()
    proof.mount = world.mount
    proof.registration = control_paths(world)["registration"]
    code, out, err = world.oxide(
        [
            "pool", "register-removable", world.mount,
            "--reserve", "16MiB", "--cap", "64MiB", "--mode", "permanent",
        ],
        "private register",
        boot,
    )
    require(code == 0 and "registered removable pool" in out, err)
    return adapter, world, boot


def run_cases(proof, adapter, world, boot, collector):
    paths = control_paths(world)
    checkouts = [world.checkout("one"), world.checkout("two"), world.checkout("three")]
    for checkout in checkouts:
        require(
            not os.path.lexists(os.path.join(checkout, "target")),
            "fixture target already exists",
        )
        world.adopt(checkout)
    require_control_files(paths)
    roots = [world.selected(checkout)["root"] for checkout in checkouts]
    link_locations = list(checkouts)
    for checkout, root in zip(checkouts, roots):
        for member in ("debug", "release"):
            link = os.path.join(checkout, "target", member)
            require(os.path.islink(link), f"fixture member is not a link: {link}")
            destination = os.path.realpath(link)
            require(os.path.commonpath((root, destination)) == root and destination != root,
                    f"fixture member escaped its owned root: {destination}")
            os.makedirs(destination, mode=0o700, exist_ok=True)
            Path(destination, "custody-sentinel").write_bytes(b"untouched")
    rows = world.registry_doc()["rows"]
    for root in roots:
        members = sorted(
            row["member"] for row in rows if row["root"] == root and row["selected"]
        )
        require(members == ["debug", "release"], f"profile group premise: {members}")

    def baseline_no_effect():
        before_control = tree_digest(world.control)
        before_links = links_snapshot(checkouts)
        code, out, _ = world.dry_run("baseline", boot)
        require(code == 0, "baseline dry run failed")
        doc = json.loads(out)
        world.complete(doc, "baseline")
        require(
            not doc["candidates"]
            and all(
                item["reason"] == "the checkout is present, so its root is kept"
                for item in doc["kept"]
            ),
            "baseline keeps were not the present-checkout keeps",
        )
        require(
            tree_digest(world.control) == before_control,
            "the dry run changed the control tree: the route is not no-effect",
        )
        require(links_snapshot(checkouts) == before_links, "a member link moved")
    collector.case("baseline-no-effect", baseline_no_effect)

    def absent_custody():
        # The checkouts stay vanished for the rest of the run: every later
        # case's custody decision must be reached, and presence keeps a root
        # before custody is consulted.
        before_links = links_snapshot(checkouts)
        for index, path in enumerate(checkouts[:2]):
            link_locations[index] = world.vanish(path)
        before_control = tree_digest(world.control)
        code, out, _ = world.dry_run("absent custody", boot)
        require(code == 0, "absent-custody dry run failed")
        doc = json.loads(out)
        world.complete(doc, "absent custody")
        require(
            world.candidates(doc) == {roots[0], roots[1]},
            f"absent checkouts did not become witnessed candidates: {world.report(doc)}",
        )
        require(
            world.kept(doc, roots[2]) == "the checkout is present, so its root is kept",
            world.report(doc),
        )
        require(
            tree_digest(world.control) == before_control,
            "the custody decisions changed the control tree",
        )
        require(list(links_snapshot(link_locations).values()) == list(before_links.values()), "a member link moved")
    collector.case("absent-custody", absent_custody)

    def dynamic_writer():
        # One real writer, seen per row: its own absent checkout keeps while
        # the sibling absent checkouts stay candidates, and once the writer
        # settles the next window offers the same root again.
        build_script = os.path.join(checkouts[2], "build.rs")
        Path(build_script).write_text(prior.GATED_BUILD)
        gate = os.path.join(adapter.root, "writer-gate")
        os.mkdir(gate, 0o700)
        writer_env = dict(adapter.env, PRUNE_GATE=gate)
        child = permanent.start_child(
            proof, [adapter.binary, "check"], checkouts[2], writer_env
        )
        away = None
        try:
            deadline = time.monotonic() + min(60, permanent.work_left())
            while time.monotonic() < deadline:
                if writer_seen(read_json_bounded(paths["removable_state"]), checkouts[2]):
                    break
                if child.poll() is not None:
                    raise RuntimeError("the writer child finished before its record was active")
                time.sleep(0.1)
            else:
                raise RuntimeError("the writer's record never became active")
            # The checkout stays vanished with its writer alive: absence and
            # active custody are decided about the same row.
            permanent.wait_for(os.path.join(gate, "started"), "running writer")
            away = world.vanish(checkouts[2])
            link_locations[2] = away
            code, out, _ = world.dry_run("writer holds", boot)
            require(code == 0, "writer-holds dry run failed")
            doc = json.loads(out)
            world.complete(doc, "writer holds")
            require(
                world.candidates(doc) == {roots[0], roots[1]},
                f"the writer's siblings were not witnessed clear: {world.report(doc)}",
            )
            require(
                world.kept(doc, roots[2]) == "an active writer still holds this checkout",
                world.report(doc),
            )
        finally:
            try:
                if away is not None:
                    world.restore(checkouts[2], away)
                    link_locations[2] = checkouts[2]
            except BaseException:
                proof.closure_known = False
                raise
            finally:
                Path(gate, "release").write_bytes(b"release")
                try:
                    code, _, stderr = permanent.finish_child(proof, child, "writer child")
                    permanent.require_normal_exit("writer child", code)
                    permanent.require_confirmed_closure("writer child", stderr)
                    require(code == 0, f"writer child failed: {stderr[-400:]}")
                except BaseException:
                    proof.closure_known = False
                    raise
        link_locations[2] = world.vanish(checkouts[2])
        deadline = time.monotonic() + min(60, permanent.work_left())
        while time.monotonic() < deadline:
            state = read_json_bounded(paths["removable_state"])
            if not any(
                record.get("checkout") == checkouts[2]
                and record.get("phase") in ("spawning", "running", "acquiring")
                for record in state.get("records", [])
            ):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("the writer's record never settled after the child finished")
        code, out, _ = world.dry_run("writer settled", boot)
        require(code == 0, "writer-settled dry run failed")
        doc = json.loads(out)
        world.complete(doc, "writer settled")
        require(
            world.candidates(doc) == {roots[0], roots[1], roots[2]},
            f"the settled writer's root was not re-offered by a fresh window: {world.report(doc)}",
        )
    collector.case("dynamic-writer", dynamic_writer)

    def in_place_witness():
        # Pause after the first row's decision and mutate the same open inode
        # before the next row. The debug-only pause is guarded by the existing
        # supervised private-fixture gate and the original authority deadline.
        import fcntl

        state_path = paths["removable_state"]
        original = Path(state_path).read_bytes()
        ordered = sorted(range(3), key=lambda i: hashlib.sha256(checkouts[i].encode()).hexdigest())
        first, later = ordered[:2]
        env = dict(adapter.env, OXIDE_TEST_PRUNE_WITNESS_PAUSE=checkouts[first])
        child = permanent.start_child(
            proof, [adapter.binary, "outputs", "prune", "--dry-run"], boot, env
        )
        try:
            permanent.wait_for(str(proof.evidence / "witness-paused"), "first custody decision")
            state = json.loads(original)
            state["records"].append(
                custody_record(checkouts[later], "spawning", 4_100_000)
            )
            locks = []
            try:
                for lock_path in (paths["internal_lock"], paths["removable_lock"]):
                    descriptor = os.open(lock_path, os.O_RDWR)
                    locks.append(descriptor)
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                inode = in_place_write(state_path, json.dumps(state).encode())
            finally:
                for descriptor in reversed(locks):
                    os.close(descriptor)
            (proof.evidence / "witness-release").write_bytes(b"release")
            code, out, stderr = permanent.finish_child(proof, child, "in-place preview")
            permanent.require_normal_exit("in-place preview", code)
            permanent.require_confirmed_closure("in-place preview", stderr)
            require(code == 0, "in-place dry run failed")
            doc = json.loads(out)
            world.complete(doc, "in-place writer")
            require(
                roots[first] in world.candidates(doc) and roots[later] not in world.candidates(doc),
                "an in-place state mutation was not witnessed: the stale parse was reused",
            )
            require(
                world.kept(doc, roots[later]) == "an active writer still holds this checkout",
                world.report(doc),
            )
            require(os.lstat(state_path).st_ino == inode, "the tamper changed the inode")
        finally:
            if child.pid in proof.unclosed:
                (proof.evidence / "witness-release").write_bytes(b"release")
                try:
                    code, _, stderr = permanent.finish_child(proof, child, "in-place preview cleanup")
                    permanent.require_normal_exit("in-place preview cleanup", code)
                    permanent.require_confirmed_closure("in-place preview cleanup", stderr)
                except BaseException:
                    proof.closure_known = False
                    raise
            atomic_write(state_path, original)
        code, out, _ = world.dry_run("restored", boot)
        require(code == 0, "restored dry run failed")
        doc = json.loads(out)
        world.complete(doc, "restored")
        require(
            world.candidates(doc) == {roots[0], roots[1], roots[2]},
            f"the restored state did not re-offer every root: {world.report(doc)}",
        )
        for pool_name in ("internal", "removable"):
            (proof.evidence / "witness-paused").unlink()
            (proof.evidence / "witness-release").unlink()
            replacement_path = paths[pool_name + "_state"]
            original = Path(replacement_path).read_bytes()
            child = permanent.start_child(
                proof, [adapter.binary, "outputs", "prune", "--dry-run"], boot, env
            )
            try:
                permanent.wait_for(str(proof.evidence / "witness-paused"), "first custody decision")
                changed = json.loads(original)
                changed["records"].append(custody_record(checkouts[later], "spawning", 4_100_001))
                locks = []
                try:
                    for lock_path in (paths["internal_lock"], paths["removable_lock"]):
                        descriptor = os.open(lock_path, os.O_RDWR)
                        locks.append(descriptor)
                        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    before = os.stat(replacement_path)
                    atomic_write(replacement_path, json.dumps(changed).encode())
                    after = os.stat(replacement_path)
                    require((before.st_dev, before.st_ino) != (after.st_dev, after.st_ino),
                            "replacement did not change state identity")
                finally:
                    for descriptor in reversed(locks):
                        os.close(descriptor)
                (proof.evidence / "witness-release").write_bytes(b"release")
                code, out, stderr = permanent.finish_child(proof, child, f"{pool_name} replacement preview")
                permanent.require_normal_exit(pool_name, code)
                permanent.require_confirmed_closure(pool_name, stderr)
                require(code == 0, f"{pool_name} replacement preview failed")
                doc = json.loads(out)
                world.complete(doc, pool_name)
                require(roots[first] in world.candidates(doc)
                        and world.kept(doc, roots[later]) == "an active writer still holds this checkout",
                        f"{pool_name} replacement was not witnessed: {world.report(doc)}")
            finally:
                if child.pid in proof.unclosed:
                    (proof.evidence / "witness-release").write_bytes(b"release")
                    try:
                        code, _, stderr = permanent.finish_child(proof, child, f"{pool_name} cleanup")
                        permanent.require_normal_exit(pool_name, code)
                        permanent.require_confirmed_closure(pool_name, stderr)
                    except BaseException:
                        proof.closure_known = False
                        raise
                atomic_write(replacement_path, original)
    collector.case("in-place-witness", in_place_witness)

    def lock_refusals():
        import fcntl

        before_links = links_snapshot(link_locations)
        def held_refusal(lock_path, *parts):
            descriptor = os.open(lock_path, os.O_RDWR)
            try:
                before_control = tree_digest(world.control)
                fcntl.flock(descriptor, fcntl.LOCK_EX)
                code, out, _ = world.dry_run("busy lock", boot)
                require(code == 0, "a busy lock must keep the batch, not fail")
                doc = json.loads(out)
                require_refusal_names(
                    doc,
                    "writer custody could not be witnessed",
                    *parts,
                )
                require(tree_digest(world.control) == before_control, "busy refusal changed control")
            finally:
                os.close(descriptor)

        held_refusal(
            paths["internal_lock"], "the internal pool's admission lock is busy"
        )
        held_refusal(
            paths["removable_lock"], "the removable pool's admission lock is busy"
        )
        # Both refusals released the locks they did take: the next window
        # succeeds immediately instead of deadlocking behind them.
        code, out, _ = world.dry_run("after refusals", boot)
        require(code == 0, "the refusals left a lock held")
        require(json.loads(out)["coverage"]["complete"], "post-refusal snapshot incomplete")

        # A missing admission lock refuses and is never created by this
        # read-only route.
        lock = paths["internal_lock"]
        os.rename(lock, lock + ".custody-held")
        try:
            before_control = tree_digest(world.control)
            code, out, _ = world.dry_run("missing lock", boot)
            require(code == 0, "a missing lock must keep the batch, not fail")
            doc = json.loads(out)
            require_refusal_names(doc, "the internal pool's admission lock is missing")
            require(
                not os.path.lexists(lock),
                "the route created the missing admission lock",
            )
            require(tree_digest(world.control) == before_control, "missing-lock refusal changed control")
        finally:
            os.rename(lock + ".custody-held", lock)
        require(links_snapshot(link_locations) == before_links, "a member link moved")
    collector.case("lock-refusals", lock_refusals)

    def unprovable_sibling():
        state_path = paths["removable_state"]
        original_mode = stat.S_IMODE(os.lstat(state_path).st_mode)
        require(original_mode == 0o600, "fixture state must be owner-only")
        before_links = links_snapshot(link_locations)
        try:
            os.rename(state_path, state_path + ".custody-held")
            before_control = tree_digest(world.control)
            code, out, _ = world.dry_run("sibling without state", boot)
            require(code == 0, "a sibling without state must keep the batch, not fail")
            require_refusal_names(
                doc := json.loads(out),
                "the removable pool's state document is missing",
            )
            require(tree_digest(world.control) == before_control, "missing-state refusal changed control")
            Path(state_path).write_bytes(b"{not json")
            os.chmod(state_path, 0o600)
            before_control = tree_digest(world.control)
            code, out, _ = world.dry_run("sibling with invalid state", boot)
            require(code == 0, "an invalid sibling state must keep the batch, not fail")
            require_refusal_names(
                json.loads(out), "the removable pool's state document is invalid"
            )
            require(tree_digest(world.control) == before_control, "invalid-state refusal changed control")
            os.truncate(state_path, 0)
            with open(state_path, "wb") as handle:
                handle.truncate(32 * 1024 * 1024 + 1)
            os.chmod(state_path, 0o600)
            before_control = tree_digest(world.control)
            code, out, _ = world.dry_run("sibling with oversized state", boot)
            require(code == 0, "an oversized sibling state must keep the batch, not fail")
            require_refusal_names(
                json.loads(out), "the removable pool's state document exceeds its byte limit"
            )
            require(tree_digest(world.control) == before_control, "oversized-state refusal changed control")
        finally:
            os.replace(state_path + ".custody-held", state_path)
        code, out, _ = world.dry_run("sibling restored", boot)
        require(code == 0, "the restored sibling did not complete")
        require(json.loads(out)["coverage"]["complete"], "restored snapshot incomplete")
        require(links_snapshot(link_locations) == before_links, "a member link moved")
    collector.case("unprovable-sibling", unprovable_sibling)

    def large_state_budget():
        import fcntl

        state_paths = [paths["internal_state"], paths["removable_state"]]
        originals = [Path(path).read_bytes() for path in state_paths]
        padded = [data.ljust(30 * 1024 * 1024, b" ") for data in originals]
        byte_marker = proof.evidence / "prune-bytes-observed"
        pause_marker = proof.evidence / "witness-paused"
        release_marker = proof.evidence / "witness-release"
        child = None
        try:
            for path, data in zip(state_paths, padded):
                atomic_write(path, data)
            byte_marker.unlink(missing_ok=True)
            code, out, stderr = world.dry_run(
                "large valid states", boot, extra={"OXIDE_TEST_PRUNE_BYTES": world.control}
            )
            require(code == 0, "large valid state preview failed")
            doc = json.loads(out)
            world.complete(doc, "large valid states")
            require(world.candidates(doc) == set(roots), "large valid states changed custody")
            observed = [line for line in stderr.splitlines() if line.startswith("oxide proof: authority bytes ")]
            require(len(observed) == 1, "large-state byte observation missing")
            used = int(observed[0].rsplit(" ", 1)[1])
            require(60 * 1024 * 1024 <= used < SNAPSHOT_BUDGET_BYTES,
                    f"large-state byte charge unexpected: {used}")
            byte_marker.unlink()
            pause_marker.unlink(missing_ok=True)
            release_marker.unlink(missing_ok=True)
            first = min(checkouts, key=lambda c: hashlib.sha256(c.encode()).hexdigest())
            env = dict(adapter.env, OXIDE_TEST_PRUNE_WITNESS_PAUSE=first,
                       OXIDE_TEST_PRUNE_BYTES=world.control)
            child = permanent.start_child(
                proof, [adapter.binary, "outputs", "prune", "--dry-run"], boot, env
            )
            permanent.wait_for(str(pause_marker), "large-state first row")
            locks = []
            try:
                for lock_path in (paths["internal_lock"], paths["removable_lock"]):
                    descriptor = os.open(lock_path, os.O_RDWR)
                    locks.append(descriptor)
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                atomic_write(state_paths[1], padded[1])
            finally:
                for descriptor in reversed(locks):
                    os.close(descriptor)
            before_control = tree_digest(world.control)
            before_links = links_snapshot(link_locations)
            release_marker.write_bytes(b"release")
            code, out, stderr = permanent.finish_child(proof, child, "replacement budget preview")
            permanent.require_normal_exit("replacement budget preview", code)
            permanent.require_confirmed_closure("replacement budget preview", stderr)
            require(code == 0, "budget refusal did not produce a preview")
            require_refusal_names(json.loads(out), "the snapshot exceeded its input or time budget")
            observed = [line for line in stderr.splitlines() if line.startswith("oxide proof: authority bytes ")]
            require(len(observed) == 1 and int(observed[0].rsplit(" ", 1)[1]) == SNAPSHOT_BUDGET_BYTES,
                    f"partial reread was not charged through exhaustion: {observed}")
            require(tree_digest(world.control) == before_control, "budget refusal changed control")
            require(links_snapshot(link_locations) == before_links, "budget refusal changed links or targets")
        finally:
            if child is not None and child.pid in proof.unclosed:
                release_marker.write_bytes(b"release")
                try:
                    code, _, stderr = permanent.finish_child(proof, child, "budget cleanup")
                    permanent.require_normal_exit("budget cleanup", code)
                    permanent.require_confirmed_closure("budget cleanup", stderr)
                except BaseException:
                    proof.closure_known = False
                    raise
            for path, original in zip(state_paths, originals):
                atomic_write(path, original)
            byte_marker.unlink(missing_ok=True)
    collector.case("large-state-budget", large_state_budget)

    def scale_unchanged():
        # Historical scale, synthetic owned data: 709 roots, 1189 rows, with
        # 650 custody-reaching writer keeps so the candidate count stays under
        # the preview cap. Both synthetic state documents are large enough
        # that a route that re-read states per row would exhaust the 64 MiB
        # aggregate budget; the witnessed route reads each document once.
        registry_original = Path(world.registry).read_bytes()
        internal_path = paths["internal_state"]
        internal_original = Path(internal_path).read_bytes()
        state_path = paths["removable_state"]
        state_original = Path(state_path).read_bytes()
        synthetic_roots = []
        synthetic_receipts = []
        # The checkout parent must EXIST so each synthetic leaf's absence is
        # the ordinary leaf-only ENOENT: the missing-ancestor extension never
        # applies below a volume mount (workflow.md "Pruning owned output
        # roots", oxide#135).
        synthetic_parent = os.path.join(world.mount, "synthetic-checkouts")
        os.mkdir(synthetic_parent, 0o700)
        try:
            template_receipt = world.receipt(checkouts[2])
            template_marker = prior.marker(roots[2])
            image_uuid = template_marker["volume_uuid"]
            registry_doc = json.loads(registry_original)
            state = json.loads(state_original)
            internal_state = json.loads(internal_original)
            written = 0
            for index in range(SCALE_ROOTS):
                checkout = os.path.join(world.mount, "synthetic-checkouts", f"s{index}")
                checkout_id = hashlib.sha256(checkout.encode()).hexdigest()
                root = os.path.join(world.mount, "oxide-outputs", f"syn-{checkout_id[:8]}")
                members = ["debug", "release"] if index < 480 else ["debug"]
                require(
                    not os.path.lexists(checkout),
                    "a synthetic checkout path already exists",
                )
                os.mkdir(root, 0o700)
                marker = dict(template_marker)
                marker.update(
                    checkout=checkout,
                    checkout_id=checkout_id,
                    label=f"syn-{index}",
                    generation=1,
                    created_ms=1,
                    volume_uuid=image_uuid,
                    checkout_volume_uuid=image_uuid,
                )
                marker_bytes = json.dumps(marker).encode()
                Path(root, ".oxide-output.json").write_bytes(marker_bytes)
                os.chmod(Path(root, ".oxide-output.json"), 0o600)
                Path(root, "payload").write_bytes(b"x" * 64)
                os.chmod(Path(root, "payload"), 0o600)
                root_identity = os.lstat(root)
                receipt = json.loads(json.dumps(template_receipt))
                receipt["checkout"] = checkout
                receipt["checkout_id"] = checkout_id
                receipt["links"] = [p for p in receipt["links"] if p["member"] in members]
                for placement in receipt["links"]:
                    selected = placement.get("selected") or {}
                    if selected.get("root") == roots[2]:
                        selected["root"] = root
                        selected["nonce"] = marker.get("nonce")
                        selected["generation"] = 1
                    prepared = placement.get("prepared")
                    if prepared and prepared.get("to") == roots[2]:
                        prepared["to"] = root
                for selected in receipt.get("roots", []):
                    if selected.get("root") == roots[2]:
                        selected.update(root=root, nonce=marker.get("nonce"), generation=1)
                receipt_path = os.path.join(world.outputs, f"{checkout_id}.json")
                atomic_write(receipt_path, json.dumps(receipt).encode())
                synthetic_receipts.append(receipt_path)
                base_row = next(
                    row for row in registry_doc["rows"] if row["root"] == roots[2]
                )
                for member in members:
                    row = dict(base_row)
                    row.update(
                        checkout_id=checkout_id,
                        checkout=checkout,
                        receipt=receipt_path,
                        member=member,
                        root=root,
                        volume_uuid=image_uuid,
                        checkout_volume_uuid=image_uuid,
                        nonce=marker.get("nonce"),
                        generation=1,
                        root_dev=root_identity.st_dev,
                        root_ino=root_identity.st_ino,
                        marker_digest=hashlib.sha256(marker_bytes).hexdigest(),
                        selected=True,
                        pending=False,
                        pruning=None,
                        pruned_ms=None,
                        reservation=None,
                        cancelling=None,
                    )
                    registry_doc["rows"].append(row)
                    written += 1
                if index < SCALE_WRITERS:
                    destination = internal_state if index < SCALE_WRITERS // 2 else state
                    destination["records"].append(
                        custody_record(checkout, "spawning", 4_200_000 + index)
                    )
                synthetic_roots.append(root)
            require(written == SCALE_ROWS, f"the scale fixture wrote {written} rows")
            registry_doc["revision"] = registry_doc.get("revision", 0) + 1
            atomic_write(world.registry, json.dumps(registry_doc).encode())
            require(len(state["records"]) <= 512 and len(internal_state["records"]) <= 512,
                    "synthetic state exceeds MAX_RECORDS")
            atomic_write(internal_path, json.dumps(internal_state).encode())
            # Whitespace is valid JSON and provides the historical byte size
            # without exceeding the state format's per-record field bounds.
            state_bytes = json.dumps(state).encode().ljust(150 * 1024, b" ")
            atomic_write(state_path, state_bytes)
            internal_size = os.lstat(paths["internal_state"]).st_size
            removable_size = os.lstat(state_path).st_size
            require(
                2 * 1024 <= internal_size <= 32 * 1024 * 1024,
                f"the internal state is outside its valid size: {internal_size}",
            )
            require(
                removable_size >= 96 * 1024,
                f"the removable state is not at its historical size: {removable_size}",
            )
            require((internal_size + removable_size) * SCALE_ROOTS > SNAPSHOT_BUDGET_BYTES,
                    "actual state sizes do not discriminate per-row rereads")
            started = time.monotonic()
            code, out, stderr = world.dry_run(
                "scale", boot, extra={"OXIDE_TEST_PRUNE_BYTES": world.control}
            )
            elapsed = time.monotonic() - started
            require(code == 0, f"the scale dry run failed after {elapsed:.1f}s")
            byte_lines = [line for line in stderr.splitlines()
                          if line.startswith("oxide proof: authority bytes ")]
            require(len(byte_lines) == 1, f"missing authority byte observation: {byte_lines}")
            charged = int(byte_lines[0].rsplit(" ", 1)[1])
            require(internal_size + removable_size <= charged < SNAPSHOT_BUDGET_BYTES,
                    f"unexpected actual authority byte charge: {charged}")
            print(f"----- scale actual authority bytes: {charged}; "
                  f"per-row state reread: {(internal_size + removable_size) * SCALE_ROOTS}", flush=True)
            doc = json.loads(out)
            world.complete(doc, "scale")
            coverage = doc["coverage"]
            require(
                coverage["registry_roots"] == len(registry_doc["rows"])
                and coverage["roots_scanned"] == SCALE_ROOTS + 3,
                f"the scale snapshot did not scan every root: {coverage}",
            )
            expected_candidates = {
                *synthetic_roots[SCALE_WRITERS:],
                *roots,
            }
            require(
                set(doc["candidates"] and {c["root"] for c in doc["candidates"]})
                == expected_candidates,
                "custody at scale did not separate writers from clear rows",
            )
            require(
                coverage["kept_roots"] == 512 and coverage["candidate_roots"] == len(expected_candidates),
                f"scale coverage drifted: {coverage}",
            )
            print(f"----- scale preview: {elapsed:.2f}s for {SCALE_ROOTS} roots", flush=True)
        finally:
            atomic_write(world.registry, registry_original)
            atomic_write(internal_path, internal_original)
            atomic_write(state_path, state_original)
            for receipt in synthetic_receipts:
                os.unlink(receipt)
            for root in synthetic_roots:
                shutil.rmtree(root)
            os.rmdir(synthetic_parent)
    collector.case("scale-unchanged", scale_unchanged)

    def effect_reproof():
        import fcntl

        # Give the CAS exercise exactly one eligible root, so a refusal cannot
        # be hidden by a successful deletion of a different candidate.
        for index in (1, 2):
            world.restore(checkouts[index], link_locations[index])
            link_locations[index] = checkouts[index]
        cas_child = None
        try:
            cas_plan, cas_sha, cas_doc = world.plan_file("CAS plan", boot)
            require(world.candidates(cas_doc) == {roots[0]}, "CAS fixture is not single-root")
            root_before = prior.owned_tree_snapshot(os.path.join(world.mount, "oxide-outputs"))
            receipt_path = permanent.receipt_file(world.pool, checkouts[0])
            receipt_before = Path(receipt_path).read_bytes()
            env = dict(adapter.env, OXIDE_TEST_PRUNE_CAS_PAUSE=roots[0])
            cas_child = permanent.start_child(
                proof, [adapter.binary, "outputs", "prune", "--plan", cas_plan,
                        "--plan-sha256", cas_sha], boot, env
            )
            binding_path = os.path.join(world.control, "bindings",
                                        hashlib.sha256(checkouts[0].encode()).hexdigest() + ".lock")
            for attempt in range(2):
                permanent.wait_for(str(proof.evidence / f"prune-cas-paused-{attempt}"), "CAS pause")
                binding = os.open(binding_path, os.O_RDWR)
                try:
                    try:
                        fcntl.flock(binding, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        pass
                    else:
                        raise RuntimeError("prune did not hold checkout binding during its recheck")
                finally:
                    os.close(binding)
                admission = os.open(paths["internal_lock"], os.O_RDWR)
                try:
                    fcntl.flock(admission, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    registry = read_json_bounded(world.registry)
                    registry["revision"] += 1
                    atomic_write(world.registry, json.dumps(registry).encode())
                finally:
                    os.close(admission)
                (proof.evidence / f"prune-cas-release-{attempt}").write_bytes(b"release")
            code, _, stderr = permanent.finish_child(proof, cas_child, "CAS refusal")
            permanent.require_normal_exit("CAS refusal", code)
            permanent.require_confirmed_closure("CAS refusal", stderr)
            require(code != 0 and "registry changed twice" in stderr, f"CAS refusal missing: {stderr[-800:]}")
            require(not world.intents() and not world.quarantines(), "CAS race published an intent")
            require(Path(receipt_path).read_bytes() == receipt_before, "CAS race changed receipt")
            require(prior.owned_tree_snapshot(os.path.join(world.mount, "oxide-outputs")) == root_before,
                    "CAS race changed output tree")
        finally:
            if cas_child is not None and cas_child.pid in proof.unclosed:
                for attempt in range(2):
                    (proof.evidence / f"prune-cas-release-{attempt}").write_bytes(b"release")
                try:
                    code, _, stderr = permanent.finish_child(proof, cas_child, "CAS cleanup")
                    permanent.require_normal_exit("CAS cleanup", code)
                    permanent.require_confirmed_closure("CAS cleanup", stderr)
                except BaseException:
                    proof.closure_known = False
                    raise
            for index in (1, 2):
                link_locations[index] = world.vanish(checkouts[index])
        plan, sha, doc = world.plan_file("effect plan", boot)
        require(
            world.candidates(doc) == {roots[0], roots[1], roots[2]},
            f"the effect plan did not offer exactly the witnessed candidates: {world.report(doc)}",
        )
        receipt_path = permanent.receipt_file(world.pool, checkouts[0])
        marker_path = os.path.join(roots[0], ".oxide-output.json")
        registry_original = Path(world.registry).read_bytes()
        receipt_original = Path(receipt_path).read_bytes()
        marker_original = Path(marker_path).read_bytes()

        def refuse_changed(label):
            before_control = tree_digest(world.control)
            before_links = links_snapshot(link_locations)
            output_tree = os.path.join(world.mount, "oxide-outputs")
            before_roots = prior.owned_tree_snapshot(output_tree)
            code, _, err = world.real_run(label, boot, plan, sha)
            require(code != 0 and ("fresh authority snapshot is incomplete" in err
                                  or "eligible root set changed" in err),
                    f"{label}: no fresh-authority refusal: {err[:400]}")
            require(tree_digest(world.control) == before_control, f"{label}: control changed")
            require(links_snapshot(link_locations) == before_links, f"{label}: links or targets changed")
            require(prior.owned_tree_snapshot(output_tree) == before_roots,
                    f"{label}: output tree changed")
            require(not world.intents() and not world.quarantines(), f"{label}: intent created")

        # These are fixture-owned contradictions planted after the preview;
        # every real run must re-read authority rather than trust the plan.
        try:
            changed = json.loads(receipt_original)
            changed["links"][0]["selected"]["nonce"] = "f" * 32
            atomic_write(receipt_path, json.dumps(changed).encode())
            refuse_changed("changed receipt nonce")
        finally:
            atomic_write(receipt_path, receipt_original)
        try:
            changed = json.loads(receipt_original)
            changed["links"][0]["selected"]["root"] = roots[0] + ".other"
            atomic_write(receipt_path, json.dumps(changed).encode())
            refuse_changed("changed receipt reference")
        finally:
            atomic_write(receipt_path, receipt_original)
        try:
            changed = json.loads(marker_original)
            changed["nonce"] = "e" * 32
            atomic_write(marker_path, json.dumps(changed).encode())
            refuse_changed("changed owner marker")
        finally:
            atomic_write(marker_path, marker_original)
        try:
            changed = json.loads(registry_original)
            for row in changed["rows"]:
                if row["root"] == roots[0]:
                    row["generation"] += 1
            changed["revision"] += 1
            atomic_write(world.registry, json.dumps(changed).encode())
            refuse_changed("changed registry generation and revision")
        finally:
            atomic_write(world.registry, registry_original)
        try:
            changed_receipt = json.loads(receipt_original)
            selected = changed_receipt["links"][0]["selected"]
            changed_receipt["reserved"] = {
                "id": "a" * 32, "nonce": selected["nonce"], "kind": selected["kind"],
                "root": selected["root"], "volume_uuid": selected.get("volume_uuid"),
                "generation": selected["generation"], "created_ms": 1,
            }
            changed_registry = json.loads(registry_original)
            for row in changed_registry["rows"]:
                if row["root"] == roots[0]:
                    row["reservation"] = "a" * 32
                    row["pending"] = True
            changed_registry["revision"] += 1
            atomic_write(receipt_path, json.dumps(changed_receipt).encode())
            atomic_write(world.registry, json.dumps(changed_registry).encode())
            code, out, _ = world.dry_run("reservation preview", boot)
            require(code == 0, "reservation preview failed")
            preview = json.loads(out)
            require(roots[0] not in world.candidates(preview), "reserved root became a candidate")
            refuse_changed("new reservation")
        finally:
            atomic_write(receipt_path, receipt_original)
            atomic_write(world.registry, registry_original)
        state_path = paths["internal_state"]
        state_original = Path(state_path).read_bytes()
        try:
            changed = json.loads(state_original)
            require(changed["version"] >= 6, "fixture control cannot represent one-use records")
            changed["one_use_workspaces"] = "oxide.one-use/1"
            record = custody_record(checkouts[0], "spawning", 4_300_000)
            record.update({"class": "one_use", "class_reason": "fixture writer"})
            changed["records"].append(record)
            atomic_write(state_path, json.dumps(changed).encode())
            code, out, _ = world.dry_run("one-use writer preview", boot)
            require(code == 0, "one-use preview failed")
            preview = json.loads(out)
            world.complete(preview, "one-use writer preview")
            require(world.kept(preview, roots[0]) == "an active writer still holds this checkout",
                    "one-use writer was not classified as active")
            refuse_changed("new one-use writer")
        finally:
            atomic_write(state_path, state_original)
        world.restore(checkouts[0], link_locations[0])
        try:
            # The plan's absent-checkout binding cannot authorize a checkout
            # which has reappeared before the real run.
            link_locations[0] = checkouts[0]
            refuse_changed("checkout reappeared")
        finally:
            link_locations[0] = world.vanish(checkouts[0])
        held_root = roots[0] + ".fixture-held"
        os.rename(roots[0], held_root)
        try:
            refuse_changed("changed root identity")
        finally:
            os.rename(held_root, roots[0])
        code, _, err = world.real_run("private effect", boot, plan, sha)
        require(code == 0, err)
        for root in roots:
            require(not os.path.lexists(root), "a candidate survived the real run")
        for checkout in checkouts:
            receipt = permanent.read_json(permanent.receipt_file(world.pool, checkout))
            require(
                receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted",
                f"{checkout}'s receipt did not commit the prune: {receipt['state']}",
            )
        tombstones = permanent.read_json(
            os.path.join(world.outputs, "tombstones.json")
        )["entries"]
        require(
            set(roots) <= {entry["root"] for entry in tombstones},
            "the deleted roots are not recorded in the tombstones",
        )
        require(not world.intents() and not world.quarantines(), "intents remain")
        after_effect = tree_digest(world.control)
        # The reproof: the same plan bytes can never run twice. The fresh
        # window re-derives custody and absence; the eligible set changed, so
        # the run refuses before any effect.
        code, _, err = world.real_run("reproof", boot, plan, sha)
        require(code != 0, "the same plan was accepted twice")
        require(
            "the eligible root set changed" in err or "no eligible roots" in err,
            f"the reproof refused for an unexpected reason: {err[:400]}",
        )
        require(
            tree_digest(world.control) == after_effect,
            "the refused reproof changed the control tree",
        )
        require(not world.quarantines(), "a reproof left a quarantine behind")
        require(
            all(not os.path.lexists(root) for root in roots),
            "the reproof restored a deleted root",
        )
    collector.case("effect-reproof", effect_reproof)


def self_check() -> int:
    """Pure counterfactuals: the real seams with fakes, no child or fixture."""
    import errno
    import inspect
    import tempfile
    from types import SimpleNamespace
    from unittest.mock import patch

    # Argument boundaries, both directions.
    require(parse_args(["--self-check"]) == ("self-check", False), "self-check argument")
    require(parse_args(["--plan"]) == ("plan", False), "plan argument")
    require(parse_args([]) == ("run", False), "builds argument")
    require(parse_args(["--diagnostic"]) == ("run", True), "diagnostic argument")
    for bad in (["--diagnostic", "--diagnostic"], ["--plan", "--diagnostic"],
                ["--self-check", "--diagnostic"], ["--extra"], ["--internal"]):
        try:
            parse_args(bad)
        except SystemExit:
            pass
        else:
            raise AssertionError(f"argument {bad} was accepted")

    # The case table is the execution order and accepts nothing else.
    for name in MODE_CASES["builds"]:
        check_case("builds", name)
    for mode, name in (("internal", "baseline-no-effect"), ("nope", "baseline-no-effect")):
        try:
            check_case(mode, name)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"{mode} accepted {name}")
    collector = Collector(SimpleNamespace(closure_known=True, unclosed=set()), "builds", False, [])
    try:
        collector.case("absent-custody", lambda: None)
    except RuntimeError:
        pass
    else:
        raise AssertionError("the collector accepted an out-of-order case")
    ordered = Collector(SimpleNamespace(closure_known=True, unclosed=set(), mount="/", registration=__file__), "builds", False, [])
    ordered.case("baseline-no-effect", lambda: None)
    ordered.case("absent-custody", lambda: None)
    require(ordered.cases[-1]["state"] == "completed", "collector state")
    proof_stub = SimpleNamespace(closure_known=True, unclosed=set(), evidence=Path("/unused"))
    image_stub = SimpleNamespace(state="attached", volume_uuid=None, device="/dev/disk1",
                                 mount="/Volumes/fixture", backing="/unused")
    disposition, fault = request_guard_detach(proof_stub, image_stub, None)
    require(disposition == "retained" and "image identity unknown" in str(fault),
            "unknown image identity must be retained as a reportable fault")
    source = inspect.getsource(main)
    require(source.index("request_guard_detach(") < source.index('"qualified": False if diagnostic'),
            "diagnostic receipt must follow the guarded detach decision")

    # The scratch layout is inside the run root and the pool inside it.
    scratch, pool = scratch_layout("/fake/run")
    require(
        scratch == "/fake/run/scratch-pools" and pool == "/fake/run/scratch-pools/pool",
        "scratch layout",
    )
    require(
        pool.startswith(scratch + os.sep) and scratch.startswith("/fake/run" + os.sep),
        "pool escaped the scratch root",
    )
    world = SimpleNamespace(pool="/old", control="", outputs="", registry="")
    place_pool(world, pool)
    require(world.registry == pool + "/oxide-managed/outputs/registry.json", "placed registry")

    # The fixture environment contract, against a hostile inherited value.
    hostile = {
        "OXIDE_SCRATCH_ROOT": "/hostile/scratch",
        "OXIDE_POOL": "/hostile/pool",
        "OXIDE_POOL_CAP": "1MiB",
        "PATH": "/usr/bin",
    }
    built = declare_scratch(hostile, "/fake/run")
    require(built["OXIDE_SCRATCH_ROOT"] == "/fake/run/scratch-pools", f"scratch: {built}")
    require(built["OXIDE_POOL"] == "/fake/run/scratch-pools/pool", f"pool: {built}")
    require(built["OXIDE_POOL_CAP"] == "64MiB" and built["PATH"] == "/usr/bin", "cap or base env")
    require(hostile["OXIDE_SCRATCH_ROOT"] == "/hostile/scratch", "declare_scratch mutated input")
    prepare_source = inspect.getsource(prepare)
    require("declare_scratch(permanent.proof_env(" in prepare_source,
            "prepare() does not apply the scratch declaration")

    # The control paths this proof witnesses are the product's own layout.
    placed = SimpleNamespace(pool=pool, control="", outputs="", registry="")
    place_pool(placed, pool)
    paths = control_paths(placed)
    require(paths["internal_lock"] == pool + "/oxide-managed/admission.lock", "internal lock path")
    require(paths["registration"] == pool + "/oxide-managed/removable.json", "registration path")
    require(
        paths["removable_state"] == pool + "/oxide-managed/removable/oxide-managed/state.json",
        "removable state path",
    )
    require(
        paths["removable_lock"] == pool + "/oxide-managed/removable/oxide-managed/admission.lock",
        "removable lock path",
    )
    source = (HERE.parent / "src" / "bin" / "oxide" / "removable.rs").read_text()
    require('POOL_DIR: &str = "removable"' in source, "the removable pool directory literal")
    require('REGISTRATION: &str = "removable.json"' in source, "the registration file literal")
    removable_control = source.split("pub fn removable_control", 1)[1].split("\n}", 1)[0]
    require('.join(POOL_DIR)' in removable_control
            and '.join(oxide_storage::lease::MANAGED_NAME)' in removable_control,
            "the witness does not derive its removable control from both product constants")
    store_source = (HERE.parent / "src" / "bin" / "oxide" / "store.rs").read_text()
    lease_source = (HERE.parent / "src" / "lease" / "mod.rs").read_text()
    require(f'pub const MANAGED_NAME: &str = "{MANAGED_NAME}";' in lease_source,
            "the shared managed-control directory literal")
    removable_store = store_source.split("pub fn open_recorded_removable", 1)[1].split("\n    }", 1)[0]
    require('control: root.join(MANAGED_NAME)' in removable_store,
            "the removable Store does not use the shared managed-control name")
    require('const LOCK_NAME: &\'static str = "admission.lock"' in store_source, "lock literal")
    require('const STATE_NAME: &\'static str = "state.json"' in store_source, "state literal")
    with tempfile.TemporaryDirectory(prefix="prune-control-paths-") as directory:
        fixture = SimpleNamespace(pool=os.path.join(directory, "pool"), control="", outputs="", registry="")
        place_pool(fixture, fixture.pool)
        fixture_paths = control_paths(fixture)
        for name in ("internal_lock", "internal_state", "removable_lock", "removable_state"):
            target = Path(fixture_paths[name])
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"{}" if name.endswith("state") else b"")
            target.chmod(0o600)
        require_control_files(fixture_paths)
        wrong = dict(fixture_paths)
        wrong["removable_state"] = os.path.join(fixture.control, "removable", "managed", "state.json")
        try:
            require_control_files(wrong)
        except FileNotFoundError:
            pass
        else:
            raise AssertionError("the historical incorrect removable state path was accepted")
        require(not os.path.lexists(os.path.dirname(wrong["removable_state"])),
                "control validation created the historical incorrect directory")
    prune_source = (HERE.parent / "src" / "bin" / "oxide" / "prune.rs").read_text()
    require("const SNAPSHOT_MAX_BYTES: u64 = 64 * 1024 * 1024;" in prune_source, "budget literal")
    require("const SNAPSHOT_TIME: Duration = Duration::from_secs(10);" in prune_source, "time literal")
    require(
        'matches!(record.phase, Phase::Spawning | Phase::Running)' in store_source,
        "the writer-phase arm the custody verdict counts",
    )
    verdict_body = store_source.split("fn custody_verdict", 1)[1][:2400]
    require("Phase::Acquiring" not in verdict_body,
            "acquiring must not be a writer phase in the shared verdict")
    require("Phase::Spawning | Phase::Running" in verdict_body,
            "the shared verdict does not count spawning and running writers")

    # The writer-phase oracle agrees with the product's own serde spellings.
    require(writer_seen({"records": [custody_record("/c", "spawning", 1)]}, "/c"), "spawning")
    require(writer_seen({"records": [custody_record("/c", "running", 1)]}, "/c"), "running")
    for phase in ("idle", "acquiring", "retired"):
        require(
            not writer_seen({"records": [custody_record("/c", phase, 1)]}, "/c"),
            f"{phase} must not count as a writer",
        )
    require(
        not writer_seen({"records": [custody_record("/other", "spawning", 1)]}, "/c"),
        "another checkout's record must not count",
    )

    # The batch-refusal oracle rejects an unrelated incomplete snapshot.
    for expected in (
        "writer custody could not be witnessed",
        "the internal pool's admission lock is busy",
        "the removable pool's admission lock is busy",
        "the internal pool's admission lock is missing",
        "the removable pool's state document is missing",
        "the removable pool's state document is invalid",
        "the removable pool's state document exceeds its byte limit",
    ):
        doc = {
            "coverage": {"complete": False},
            "candidates": [],
            "kept": [
                {
                    "reason": "the registry snapshot is incomplete (unrelated fault);"
                    " the whole batch is kept"
                }
            ],
        }
        try:
            require_refusal_names(doc, expected)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"an unrelated fault satisfied {expected!r}")
        doc["kept"][0]["reason"] = (
            "the registry snapshot is incomplete (writer custody could not be witnessed"
            f" ({expected})); the whole batch is kept"
        )
        require_refusal_names(doc, expected)
    complete_doc = {
        "coverage": {"complete": True},
        "candidates": [{"root": "/r1"}],
        "kept": [{"reason": "the registry snapshot is incomplete (x); the whole batch is kept"}],
    }
    try:
        require_refusal_names(complete_doc, "x")
    except RuntimeError:
        pass
    else:
        raise AssertionError("a complete snapshot passed the batch oracle")
    candidate_doc = {
        "coverage": {"complete": False},
        "candidates": [{"root": "/r1"}],
        "kept": [{"reason": "writer custody could not be witnessed (x)"}],
    }
    try:
        require_refusal_names(candidate_doc, "x")
    except RuntimeError:
        pass
    else:
        raise AssertionError("a candidate survived the batch oracle")

    # The scale arithmetic discriminates: per-row re-reads exceed the budget,
    # one witnessed read per pool fits it.
    oracle = scale_oracle()
    require(oracle["rows"] == SCALE_ROWS and oracle["roots"] == SCALE_ROOTS, "scale shape")
    require(oracle["old_route_exceeds_budget"], "old-route arithmetic does not exceed the budget")
    require(oracle["witnessed_route_fits_budget"], "witnessed-route arithmetic does not fit")
    require(oracle["budget_bytes"] == 64 * 1024 * 1024, "budget figure")
    require(SCALE_WRITERS < SCALE_ROOTS, "every synthetic checkout must not be a writer")
    control_source = (HERE.parent / "src" / "lease" / "control.rs").read_text()
    require("pub const MAX_RECORDS: usize = 512;" in control_source, "record limit drifted")
    require(SCALE_WRITERS // 2 < 512 and SCALE_WRITERS - SCALE_WRITERS // 2 < 512,
            "synthetic writer distribution exceeds a pool's limit")
    require(
        (SCALE_ROOTS - SCALE_WRITERS) + 3 < 512,
        "the preview candidate cap would truncate the scale candidates",
    )
    require(
        oracle["writer_keeps"] > 512,
        "the scale kept list must exercise the preview's own truncation",
    )

    # The atomic and in-place writers behave on a private temporary fixture.
    with tempfile.TemporaryDirectory(prefix="prune-custody-") as directory:
        document = Path(directory) / "state.json"
        with open(document, "wb") as handle:
            handle.write(b"{}")
        os.chmod(document, 0o600)
        before = os.lstat(document)
        inode = in_place_write(str(document), b'{"records": []}')
        after = os.lstat(document)
        require(
            (before.st_dev, before.st_ino) == (after.st_dev, after.st_ino),
            "in_place_write changed the inode",
        )
        require(document.read_bytes() == b'{"records": []}', "in_place_write bytes")
        atomic_write(str(document), b'{"records": [1]}')
        replaced = os.lstat(document)
        require(
            (before.st_dev, before.st_ino) != (replaced.st_dev, replaced.st_ino),
            "atomic_write kept the same inode",
        )
        require(document.read_bytes() == b'{"records": [1]}', "atomic_write bytes")
        require(stat.S_IMODE(replaced.st_mode) == 0o600, "atomic_write mode")
        try:
            atomic_write(str(document), b"x", 0o400)
            require(stat.S_IMODE(os.lstat(document).st_mode) == 0o400, "mode argument")
            atomic_write(str(document), b'{"records": [1]}')
        finally:
            pass

    # tree_digest pins names, bytes, modes, sizes and identities, and notices
    # a created file.
    with tempfile.TemporaryDirectory(prefix="prune-tree-") as directory:
        nested = Path(directory) / "outputs"
        nested.mkdir()
        (nested / "registry.json").write_bytes(b"registry")
        os.chmod(nested / "registry.json", 0o600)
        first = tree_digest(directory)
        (nested / "extra.json").write_bytes(b"extra")
        second = tree_digest(directory)
        require(set(second) - set(first) == {"./outputs/extra.json"}, "creation oracle")
        (nested / "registry.json").write_bytes(b"changed")
        require(tree_digest(directory) != second, "byte oracle")
        os.chmod(nested / "registry.json", 0o644)
        require(stat.S_IMODE(tree_digest(directory)["./outputs/registry.json"][0]) == 0o644, "mode oracle")
        os.rename(nested / "registry.json", nested / "moved.json")
        (nested / "registry.json").write_bytes(b"changed")
        require(
            tree_digest(directory)["./outputs/registry.json"][3:5]
            != second["./outputs/registry.json"][3:5],
            "identity oracle: a replaced file changes its device and inode",
        )

    with tempfile.TemporaryDirectory(prefix="prune-links-") as directory:
        checkout = Path(directory, "checkout")
        target = checkout / "target"
        target.mkdir(parents=True)
        destination = Path(directory, "destination")
        destination.mkdir()
        (destination / "sentinel").write_bytes(b"original")
        for member in ("debug", "release"):
            (target / member).symlink_to(destination)
        before = links_snapshot([str(checkout)])
        (destination / "sentinel").write_bytes(b"changed")
        require(links_snapshot([str(checkout)]) != before, "link target mutation was missed")
        (target / "debug").unlink()
        try:
            links_snapshot([str(checkout)])
        except FileNotFoundError:
            pass
        else:
            raise AssertionError("missing member link was treated as a valid snapshot")

    # The effect-free plan: bounds, runner pins, table and nine premises.
    document = plan_document()
    require(document["effects"] == "none" and len(document["premise_checklist"]) == 9, "plan premises")
    require(document["modes"]["builds"]["cases"] == list(MODE_CASES["builds"]), "plan table")
    require(document["bounds"] == {"limit_s": 360, "scratch_budget_mib": 512,
                                   "includes": "cleanup"}, "plan bounds")
    require(
        document["runner"]["candidate_sha256"] == CANDIDATE_RUNNER_SHA256
        and document["runner"]["default_sha256"] == DEFAULT_RUNNER_SHA256,
        "runner pins",
    )

    # The effigy selectors: exact text, so the qualified candidate runner and
    # its bounds cannot drift, and the ordinary default is never repointed.
    for name, expected in EXPECTED_SELECTORS.items():
        require(selector_run(name) == expected, f"{name} run line drifted: {selector_run(name)}")
    require(
        "bounded-test-candidate.py" in selector_run("rust:prune-fresh-custody"),
        "the selector must run the qualified candidate",
    )
    require(
        "bounded-test.py --limit" not in selector_run("rust:prune-fresh-custody"),
        "the selector must not repoint the ordinary default runner",
    )
    require("--root-parent" not in selector_run("rust:prune-fresh-custody"), "internal root")

    # The two runner files exist at their fixed paths with the pinned bytes.
    require(
        hashlib.sha256((HERE.parent / CANDIDATE_RUNNER).read_bytes()).hexdigest()
        == CANDIDATE_RUNNER_SHA256,
        "the qualified candidate runner bytes changed",
    )
    require(
        hashlib.sha256((HERE.parent / DEFAULT_RUNNER).read_bytes()).hexdigest()
        == DEFAULT_RUNNER_SHA256,
        "the ordinary default runner bytes changed",
    )

    # Exercise the actual host Python API, without creating a fixture.
    existing = Path(__file__).resolve(strict=True)
    require(existing.is_file(), "strict resolution of an existing file")
    try:
        (existing / "missing").resolve(strict=True)
    except OSError:
        pass
    else:
        raise AssertionError("strict resolution accepted a missing path")

    seed = wrapper.offline_seed
    directory_stat = SimpleNamespace(st_mode=stat.S_IFDIR | 0o700, st_dev=7, st_ino=11)
    def fake_open(path, flags, **kwargs):
        if path == "registry":
            raise OSError(errno.ENOTDIR, "synthetic symlink")
        return 10
    with patch.object(seed.os, "open", side_effect=fake_open), \
         patch.object(seed.os, "fstat", return_value=directory_stat), \
         patch.object(seed.os, "dup", return_value=11), \
         patch.object(seed.os, "close"):
        with seed.Anchor(Path("/fake/home"), "host") as anchor:
            try:
                anchor.open_dir(("registry",))
            except seed.SeedRefusal as error:
                require("not a real directory" in str(error), str(error))
            else:
                raise AssertionError("the symlinked-home counterfactual was accepted")

    print("prune fresh-custody apparatus self-check: OK")
    return 0


def plan() -> int:
    print(json.dumps(plan_document(), indent=2), flush=True)
    return 0


def main():
    kind, diagnostic = parse_args(sys.argv[1:])
    if kind == "self-check":
        return self_check()
    if kind == "plan":
        return plan()
    proof = PruneProof()
    require(wrapper.git(proof, "status", "--porcelain") == "", "source is not clean")
    custody = prior.ImageCustody("Ox235" + secrets.token_hex(4), custody_protocol=True)
    cases, error = [], None
    try:
        adapter, world, boot = prepare(proof, custody)
        collector = Collector(proof, "builds", diagnostic, cases)
        run_cases(proof, adapter, world, boot, collector)
    except BaseException as exc:
        error = exc
    finally:
        disposition, error = request_guard_detach(proof, custody, error)
    recorded = {item["case"] for item in cases}
    cases.extend(
        {"case": name, "state": "unexecuted"}
        for name in MODE_CASES["builds"]
        if name not in recorded
    )
    report = {
        "mode": "builds",
        "qualified": False if diagnostic else None,
        "cases": cases,
        "error": None if error is None else f"{type(error).__name__}: {error}",
        "custody": custody.state,
        "detach": disposition,
    }
    if diagnostic:
        (proof.evidence / "prune235-fresh-custody-diagnostic.json").write_text(
            json.dumps(report, indent=2)
        )
    print(json.dumps(report, indent=2), flush=True)
    if error is not None:
        raise error
    require(
        disposition == "requested" and all(case["state"] == "completed" for case in cases),
        "proof incomplete",
    )
    return 1 if diagnostic else 0


if __name__ == "__main__":
    sys.exit(main())
