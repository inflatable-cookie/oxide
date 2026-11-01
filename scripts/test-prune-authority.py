#!/usr/bin/env python3
"""Focused prune authority proof (oxide#134; S4 migration oxide#211), under bounded-test.py.

One candidate binary; one private 128 MiB APFS image attached `-owners on`
and ordinary-detached; no installed binary or live pool. The supervisor owns
reserve, deadline, scratch accounting and closure. No session or signal timer
is introduced here. The private CARGO_HOME is seeded by offline_seed from this
HEAD's lock, then the supervised staging helper uses six Cargo jobs. Fixtures
build sequentially with at most two jobs.

Two modes share the shape above and differ only in where the directory pool
and the legacy checkout live (decision 7aedd5af):

- builds (`rust:prune-authority` and `:diagnostic`, no `--root-parent`): the
  pool sits in a private scratch-pools directory under the supervisor run root
  on the selected volume, declared as OXIDE_SCRATCH_ROOT. Cases: accounting,
  authority-refusals, revalidation, receiptless-external-unknown (a receiptless
  legacy root on the external volume is classified Unknown and kept), and
  positive-grouped (a recorded-UUID group is pruned).
- internal (`rust:prune-authority:internal` and `:diagnostic`,
  `--root-parent /private/tmp`): ONLY receiptless-legacy-positive. That
  oracle needs the legacy checkout's nearest existing ancestor on the boot Data
  volume (src/bin/oxide/prune.rs:3276-3287); a /Volumes placement cannot
  satisfy it. The pool stays inside the run root with no OXIDE_SCRATCH_ROOT.

Bounds are unchanged for both: 300 s and 512 MiB including cleanup. The
allocation is unmeasured and not proof of fit. Staging requests 6 CPU / 8 GiB;
fixture builds use at most two jobs.

`--plan` prints each mode's ordered case table, selectors, placement and the
nine premise dispositions below, with no effect. `--self-check` is the pure
pin: it checks the case tables and counterfactuals, the scratch layout, the
unknown-external oracle, the plan document and the exact effigy selector text.
Each native invocation needs independent source-fit review and separate owner
activation. Mode and selector names come from the table near the top of the file.
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

sys.dont_write_bytecode = True
HERE = Path(__file__).resolve().parent


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


wrapper = load("proof_wrapper134", "test-oxide-wrapper-bypass.py")
prior = load("proof_prune134", "test-outputs-prune.py")
permanent = prior._load("permanent134", "test-permanent-outputs.py")
prior.permanent = permanent
prior.expect = permanent.expect
prior.fail = permanent.fail
prior.DIAG = prior.Diagnostics(enabled=False)

# The fixed case tables. Order is the execution order; a mode rejects any case
# outside its own table or out of order (see Collector.case and check_case).
MODE_CASES = {
    "builds": (
        "accounting",
        "authority-refusals",
        "revalidation",
        "receiptless-external-unknown",
        "positive-grouped",
    ),
    "internal": ("receiptless-legacy-positive",),
}
SELECTORS = {
    "builds": ("rust:prune-authority", "rust:prune-authority:diagnostic"),
    "internal": ("rust:prune-authority:internal", "rust:prune-authority:internal:diagnostic"),
}
PLACEMENT = {
    "builds": (
        "no --root-parent; OXIDE_SCRATCH_ROOT = <run root>/scratch-pools (0700); "
        "pool <run root>/scratch-pools/pool; OXIDE_POOL_CAP 64MiB; selected-volume scratch"
    ),
    "internal": (
        "--root-parent /private/tmp (decision 7aedd5af; bd357a4c); pool <run root>/pool "
        "with no OXIDE_SCRATCH_ROOT; the legacy checkout keeps its boot Data ancestor"
    ),
}
# The nine HIGH proof-authoring premise dispositions (workflow.md "Proof-authoring
# premise checklist"), with the executable source location of each.
PREMISE = (
    "1 private HOME and rustup: seed_and_stage (test-oxide-wrapper-bypass.py:276-330) "
    "and permanent.proof_env (test-permanent-outputs.py:565-605) keep the host rustup "
    "home, a private CARGO_HOME, RUSTUP_TOOLCHAIN 1.97.1 and RUSTUP_AUTO_INSTALL=0; "
    "no fixture call uses --locked. Both modes: unchanged.",
    "2 account-home control: proof_env sets OXIDE_TEST_HOME_FROM_ENV=1 "
    "(test-permanent-outputs.py:595). Both modes use the private home.",
    "3 environment isolation: proof_env pops inherited OXIDE_, XDG_, CARGO_BUILD_, "
    "CARGO_TARGET_ and GIT_ keys (test-permanent-outputs.py:572-588); PruneProof.env "
    "drops XDG_ and GIT_. Builds sets OXIDE_SCRATCH_ROOT and OXIDE_POOL under the run "
    "root (prepare); internal pops OXIDE_SCRATCH_ROOT (prepare).",
    "4 automatic adoption: run_builds and run_internal assert each fixture checkout has "
    "no target before the explicit `outputs adopt`; legacy roots are hand-built, never built.",
    "5 Cargo external argv: not used; no external-subcommand fixture.",
    "6 owned images: ImageCustody attaches with `hdiutil attach -nobrowse -owners on` "
    "(test-outputs-prune.py:992); one ordinary detach, recorded by detach_once.",
    "7 compiler flags: seed_and_stage scrubs encoded and plain flags; prepare sets "
    "RUSTFLAGS to empty and pops CARGO_ENCODED_RUSTFLAGS for every fixture child.",
    "8 temporary storage: PruneProof.env points TMPDIR, TMP and TEMP at the supervisor "
    "root (test-oxide-wrapper-bypass.py:194-195). Builds: that root is on the selected "
    "volume (no --root-parent). Internal: /private/tmp is the boot-Data ancestor the "
    "legacy oracle needs (prune.rs:3276-3287), declared by 7aedd5af.",
    "9 macOS mktemp: not invoked by this script. Every fixture path is os.path.join "
    "under the supervisor root or its scratch-pools directory, created with mkdir or "
    "makedirs; the scratch directory is created 0700.",
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
    """Return ("self-check" | "plan" | "run", mode, diagnostic). Refuse anything else."""
    if argv == ["--self-check"]:
        return "self-check", None, False
    if argv == ["--plan"]:
        return "plan", None, False
    flags = set(argv)
    if len(argv) != len(flags) or not flags <= {"--internal", "--diagnostic"}:
        raise SystemExit(
            "expected --self-check, --plan, or any of --internal and --diagnostic, each at most once"
        )
    mode = "internal" if "--internal" in flags else "builds"
    return "run", mode, "--diagnostic" in flags


def scratch_layout(task_root: str) -> tuple[str, str]:
    """S4: one private scratch root inside the run root, and the one pool inside it."""
    scratch = os.path.join(task_root, "scratch-pools")
    return scratch, os.path.join(scratch, "pool")


def declare_scratch(env: dict, mode: str, task_root: str) -> dict:
    """Return a copy of a fixture env with this mode's scratch declaration applied.

    Pure. Builds declares OXIDE_SCRATCH_ROOT and the pool inside it; internal
    declares none. Any inherited OXIDE_SCRATCH_ROOT is dropped first, so a host
    value can never leak into either mode."""
    if mode not in MODE_CASES:
        raise RuntimeError(f"unknown mode {mode!r}")
    env = dict(env)
    env.pop("OXIDE_SCRATCH_ROOT", None)
    env["OXIDE_POOL_CAP"] = "64MiB"
    if mode == "builds":
        scratch, pool = scratch_layout(task_root)
        env["OXIDE_SCRATCH_ROOT"] = scratch
        env["OXIDE_POOL"] = pool
    return env


def place_pool(world, pool: str) -> None:
    """Move a World's pool, and every path derived from it, under the scratch root."""
    world.pool = pool
    world.control = os.path.join(pool, "oxide-managed")
    world.outputs = os.path.join(world.control, "outputs")
    world.registry = os.path.join(world.outputs, "registry.json")


def canonical_layout(registry: Path, git: Path) -> Path:
    """Only the same canonical parent with the seeder's exact names qualifies."""
    if registry.name != "registry" or git.name != "git" or registry.parent != git.parent:
        raise RuntimeError("registry and git do not share one canonical Cargo cache root")
    return registry.parent


def require_batch_refusal(doc, expected):
    """Every kept root must identify the planted batch fault."""
    require(not doc["coverage"]["complete"] and not doc["candidates"], "batch was not kept")
    require(bool(doc["kept"]) and all(expected in item["reason"] for item in doc["kept"]),
            f"batch refusal did not identify {expected!r}: {doc['kept']}")


def require_unknown_kept(doc, root, reason_part):
    """A receiptless legacy root on /Volumes is Unknown and kept, and only it is kept."""
    require(doc["coverage"]["complete"], f"unknown-external plan incomplete: {doc}")
    require(root not in {candidate["root"] for candidate in doc["candidates"]},
            f"legacy root on the external volume was pruned: {doc}")
    reason = next((item["reason"] for item in doc["kept"] if item.get("root") == root), "")
    require("checkout absence is unknown (" in reason and reason_part in reason,
            f"legacy root kept for an unexpected reason: {reason!r}")


def verified_seed_source(home: Path) -> dict:
    """Resolve the two account-home links, then positively bind one real root.

    The strict seeder still opens that root and every descendant no-follow on
    the same device. This function never writes to the host cache.
    """
    if not home.is_absolute():
        raise RuntimeError("host Cargo home is not absolute")
    registry = (home / "registry").resolve(strict=True)
    git = (home / "git").resolve(strict=True)
    root = canonical_layout(registry, git)
    if not root.is_absolute() or root.resolve(strict=True) != root:
        raise RuntimeError("Cargo cache root is not canonical")
    identity = {}
    with wrapper.offline_seed.Anchor(root, "canonical host Cargo cache") as anchor:
        root_metadata = os.lstat(root)
        if not stat.S_ISDIR(root_metadata.st_mode) or (root_metadata.st_dev, root_metadata.st_ino) != anchor.identity:
            raise RuntimeError("Cargo cache root changed while binding")
        identity["root"] = list(anchor.identity)
        for name, path in (("registry", registry), ("git", git)):
            metadata = os.lstat(path)
            if not stat.S_ISDIR(metadata.st_mode) or metadata.st_dev != anchor.dev:
                raise RuntimeError(f"{name} is not a real directory on the cache device")
            descriptor = anchor.open_dir((name,))
            try:
                opened = os.fstat(descriptor)
                if (opened.st_dev, opened.st_ino) != (metadata.st_dev, metadata.st_ino):
                    raise RuntimeError(f"{name} changed while binding the cache")
                followed = os.stat(home / name)
                if (followed.st_dev, followed.st_ino) != (opened.st_dev, opened.st_ino):
                    raise RuntimeError(f"{name} account-home link changed")
                identity[name] = [opened.st_dev, opened.st_ino]
            finally:
                os.close(descriptor)
    return {"root": str(root), "home": str(home), "identity": identity}


def restore_missing_receipt(path: Path, original: bytes, mode: int) -> None:
    if mode != 0o600:
        raise RuntimeError("fixture receipt did not have mode 0600")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(original)
    if stat.S_IMODE(os.lstat(path).st_mode) != mode or path.read_bytes() != original:
        raise RuntimeError("receipt restoration changed mode or bytes")


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
    "rust:prune-authority": "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-prune-authority.py",
    "rust:prune-authority:diagnostic": "python3 scripts/bounded-test.py --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-prune-authority.py --diagnostic",
    "rust:prune-authority:internal": "python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-prune-authority.py --internal",
    "rust:prune-authority:internal:diagnostic": "python3 scripts/bounded-test.py --root-parent /private/tmp --limit 300 --scratch-budget-mib 512 -- python3 scripts/test-prune-authority.py --internal --diagnostic",
    "proof:prune-authority:test": "python3 scripts/test-prune-authority.py --self-check",
    "proof:prune-authority:plan": "python3 scripts/test-prune-authority.py --plan",
}


def plan_document() -> dict:
    """The effect-free plan: both modes, their tables, bounds and premises."""
    return {
        "script": "scripts/test-prune-authority.py",
        "effects": "none",
        "bounds": {"limit_s": 300, "scratch_budget_mib": 512, "includes": "cleanup"},
        "modes": {
            mode: {"selectors": list(SELECTORS[mode]), "placement": PLACEMENT[mode],
                   "cases": list(MODE_CASES[mode])}
            for mode in MODE_CASES
        },
        "premise_checklist": list(PREMISE),
    }


def legacy_fixture(world, base: str, marker: dict) -> tuple[str, str]:
    """A v1 receiptless legacy root on the image, with its checkout under `base`.

    The marker drops nonce and checkout_volume_uuid: version-1 markers carry
    neither (src/bin/oxide/outputs.rs:1390-1393)."""
    legacy_checkout = os.path.join(base, "never", "legacy")
    os.makedirs(os.path.dirname(legacy_checkout), exist_ok=True)
    legacy_id = hashlib.sha256(legacy_checkout.encode()).hexdigest()
    legacy_root = os.path.join(world.mount, "oxide-outputs", "legacy-" + legacy_id[:8])
    os.mkdir(legacy_root, 0o700)
    legacy_marker = dict(marker)
    legacy_marker.pop("nonce", None)
    legacy_marker.pop("checkout_volume_uuid", None)
    legacy_marker.update(version="oxide.outputs.root/1", checkout=legacy_checkout,
                         checkout_id=legacy_id, generation=1)
    Path(legacy_root, ".oxide-output.json").write_text(json.dumps(legacy_marker))
    Path(legacy_root, "payload").write_bytes(b"legacy")
    return legacy_checkout, legacy_root


class PruneProof(wrapper.Proof):
    def env(self, extra=None):
        env = super().env(extra)
        for key in list(env):
            if key.startswith(("XDG_", "GIT_")):
                env.pop(key)
        return env


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
        code, out, err = self.inner.run(argv, cwd=cwd, env=env,
            label=f"prune134-{self.serial:03d}", limit=timeout)
        return subprocess.CompletedProcess(argv, code, out.encode(), err.encode())


class Collector:
    """One mode's ordered cases. A failure taints later fixture state."""

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
            self.cases.append({"case": name, "state": "skipped", "reason": "earlier fixture state uncertain"})
            return
        self.cases.append({"case": name, "state": "reached"})
        try:
            body()
            self.cases[-1]["state"] = "completed"
        except BaseException as error:
            self.cases[-1].update(state="failed", reason=f"{type(error).__name__}: {error}")
            self.tainted = True
            if (not self.diagnostic or isinstance(error, (prior.Aborted, KeyboardInterrupt, SystemExit))
                    or not self.proof.closure_known):
                raise


def prepare(proof, custody, mode):
    """Shared staging, bootstrap, image attach and registration for either mode."""
    host_home = Path(os.environ.get("CARGO_HOME") or os.path.join(os.path.expanduser("~"), ".cargo"))
    source = verified_seed_source(host_home)
    oxide, provenance = wrapper.seed_and_stage(proof, jobs=6, host_cargo_home=source["root"])
    if verified_seed_source(host_home) != source:
        raise RuntimeError("canonical Cargo cache identity changed during staging")
    provenance["seed_source"] = source
    provenance["mode"] = mode
    (proof.evidence / "prune134-provenance.json").write_text(json.dumps(provenance, indent=2))
    adapter = Adapter(proof, oxide)
    world = prior.World(adapter, adapter.root)
    home, cargo_home = proof.root / "home", proof.root / "fixture-cargo-home"
    home.mkdir(mode=0o700)
    cargo_home.mkdir(mode=0o700)
    if mode == "builds":
        scratch, pool = scratch_layout(adapter.root)
        os.mkdir(scratch, 0o700)
        place_pool(world, pool)
    adapter.env = declare_scratch(permanent.proof_env(world.pool, str(home), str(cargo_home)),
                                  mode, adapter.root)
    require(adapter.env.get("OXIDE_POOL") == world.pool, "fixture pool is not the placed pool")
    if mode == "builds":
        require(adapter.env.get("OXIDE_SCRATCH_ROOT") == scratch, "builds fixture scratch declaration drifted")
    adapter.env["CARGO_BUILD_JOBS"] = "2"
    adapter.env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    adapter.env["RUSTFLAGS"] = ""
    boot = world.checkout("bootstrap")
    code, _, err = world.oxide(["check"], "private pool bootstrap", boot)
    require(code == 0, err)
    custody.allocate(str(proof.evidence), str(proof.root))
    world.mount = custody.attach()
    code, out, err = world.oxide(["pool", "register-removable", world.mount,
        "--reserve", "16MiB", "--cap", "64MiB", "--mode", "permanent"], "private register", boot)
    require(code == 0 and "registered removable pool" in out, err)
    return adapter, world, boot


def run_builds(proof, adapter, world, boot, collector):
    checkouts = [world.checkout("first"), world.checkout("second"), world.checkout("kept")]
    for checkout in checkouts:
        require(not os.path.lexists(os.path.join(checkout, "target")), "fixture target already exists")
        world.adopt(checkout)
    roots = [world.selected(checkout)["root"] for checkout in checkouts]
    rows = world.registry_doc()["rows"]
    for root in roots[:2]:
        members = sorted(row["member"] for row in rows if row["root"] == root and row["selected"])
        require(members == ["debug", "release"], f"profile group premise: {members}")
    first, second = sorted(range(2), key=lambda i: hashlib.sha256(checkouts[i].encode()).hexdigest())
    marker = prior.marker(roots[first])
    data = Path(roots[first]) / "payload"
    data.write_bytes(b"x" * 4096)
    os.link(data, data.with_name("payload-link"))
    aways = {i: world.vanish(checkouts[i]) for i in (first, second)}
    keep_before = prior.tree_snapshot(roots[2])
    keep_identity = (os.lstat(roots[2]).st_dev, os.lstat(roots[2]).st_ino)

    def accounting():
        _, _, control = world.plan_file("uninjected control", boot)
        require(control["coverage"]["complete"] and world.candidates(control) == {roots[first], roots[second]}, str(control))
        measured = next(c for c in control["candidates"] if c["root"] == roots[first])
        require(measured["accounting_complete"] and measured["logical_bytes"] >= 8192, str(measured))
        blocks = os.stat(data).st_blocks * 512
        # No other regular file has this inode. The measured allocation must
        # include it once; inspect the complete tiny root for the exact oracle.
        logical = allocated = 0
        seen = set()
        for directory, _, files in os.walk(roots[first]):
            for name in files:
                stat_result = os.lstat(os.path.join(directory, name))
                if not os.path.isfile(os.path.join(directory, name)) or os.path.islink(os.path.join(directory, name)):
                    continue
                logical += stat_result.st_size
                key = (stat_result.st_dev, stat_result.st_ino)
                if key not in seen:
                    allocated += stat_result.st_blocks * 512
                    seen.add(key)
        require(blocks > 0 and measured["logical_bytes"] == logical and measured["allocated_bytes"] == allocated, str(measured))
        fired = proof.evidence / "prune-diagnostic-delay-fired"
        require(not fired.exists(), "delay marker exists before injection")
        _, _, delayed = world.plan_file("delayed accounting", boot,
            extra={"OXIDE_TEST_PRUNE_DIAGNOSTIC_DELAY": roots[first]})
        require(fired.read_text().strip() == roots[first], "delay did not fire on first authority row")
        require(delayed["coverage"]["complete"] and world.candidates(delayed) == {roots[first], roots[second]}, str(delayed))
        require(not delayed["totals"]["accounting_complete"] and
            all(not c["accounting_complete"] for c in delayed["candidates"]), str(delayed))
    collector.case("accounting", accounting)

    def refusals():
        path = Path(permanent.receipt_path(world.pool, checkouts[second]))
        original = path.read_bytes()
        original_mode = stat.S_IMODE(os.lstat(path).st_mode)
        require(original_mode == 0o600, "fixture receipt must be owner-only")
        for label, content in (("missing", None), ("corrupt", b"{"), ("oversized", b"x" * 65537)):
            try:
                if content is None: path.unlink()
                else: path.write_bytes(content)
                code, out, _ = world.dry_run(label, boot)
                doc = json.loads(out)
                require(code == 0, label)
                expected = {"missing": f"the receipt {path} is missing",
                            "corrupt": f"{path} is invalid:",
                            "oversized": f"{path} exceeds its 65536-byte limit"}[label]
                require_batch_refusal(doc, expected)
            finally:
                if content is None:
                    restore_missing_receipt(path, original, original_mode)
                else:
                    path.write_bytes(original)
        registry = Path(world.registry)
        original = registry.read_bytes()
        try:
            doc = json.loads(original)
            doc["complete"] = False
            doc["incomplete"] = {"reason": "fixture", "since_ms": 1, "transaction": "a" * 32}
            registry.write_text(json.dumps(doc))
            code, out, _ = world.dry_run("sticky", boot)
            doc = json.loads(out)
            require(code == 0, "sticky")
            require_batch_refusal(doc, "the registry is sticky-incomplete")
        finally:
            registry.write_bytes(original)
        fired = proof.evidence / "prune-unknown-custody-fired"
        code, out, _ = world.dry_run("unknown custody", boot,
            extra={"OXIDE_TEST_PRUNE_UNKNOWN_CUSTODY": checkouts[first]})
        doc = json.loads(out)
        require(code == 0 and fired.read_text().strip() == checkouts[first], "unknown custody")
        require_batch_refusal(doc, "writer custody is unknown (injected fixture)")
        require(prior.tree_snapshot(roots[2]) == keep_before and (os.lstat(roots[2]).st_dev, os.lstat(roots[2]).st_ino) == keep_identity, "kept root changed")
    collector.case("authority-refusals", refusals)

    def revalidation():
        plan, sha, _ = world.plan_file("identity plan", boot)
        moved = roots[first] + "-moved"
        os.rename(roots[first], moved)
        try:
            code, _, _ = world.real_run("changed identity", boot, plan, sha)
            require(code != 0 and os.path.isdir(moved), "identity change allowed an effect")
        finally:
            os.rename(moved, roots[first])
        for kind in ("registry", "receipt"):
            plan, sha, _ = world.plan_file(f"{kind} identity plan", boot)
            path = Path(world.registry if kind == "registry" else permanent.receipt_path(world.pool, checkouts[first]))
            original = path.read_bytes()
            try:
                document = json.loads(original)
                if kind == "registry":
                    rows = [row for row in document["rows"] if row["root"] == roots[first]]
                    require(bool(rows), "registry fixture has no group")
                    rows[0]["generation"] += 1
                else:
                    document["links"][0]["selected"]["generation"] += 1
                path.write_text(json.dumps(document))
                code, _, _ = world.real_run(f"changed {kind}", boot, plan, sha)
                require(code != 0 and os.path.isdir(roots[first]) and os.path.isdir(roots[second]), f"{kind} change allowed an effect")
            finally:
                path.write_bytes(original)
        world.restore(checkouts[second], aways[second])
        plan, sha, doc = world.plan_file("locked plan", boot)
        require(world.candidates(doc) == {roots[first]}, str(doc))
        before = prior.tree_snapshot(roots[first])
        code, _, _ = world.real_run("locked refusal", boot, plan, sha,
            extra={"OXIDE_TEST_PRUNE_LOCKED_REFUSE": roots[first]})
        fired = proof.evidence / "prune-locked-refuse-fired"
        require(code != 0 and fired.read_text().strip() == roots[first] and prior.tree_snapshot(roots[first]) == before, "locked recheck")
    collector.case("revalidation", revalidation)

    def external_unknown():
        # Candidate state here: roots[first] is still vanished, roots[second] is restored.
        _, legacy_root = legacy_fixture(world, adapter.root, marker)
        listed = os.path.join(adapter.root, "operator-unknown.json")
        Path(listed).write_text(json.dumps([legacy_root]))
        _, _, doc = world.plan_file("external legacy plan", boot, listed)
        require_unknown_kept(doc, legacy_root, "no recorded volume UUID")
        require(world.candidates(doc) == {roots[first]}, str(doc))
        require(os.path.isdir(legacy_root), "the unknown legacy root was not kept")
    collector.case("receiptless-external-unknown", external_unknown)

    def positive():
        plan, sha, doc = world.plan_file("positive plan", boot)
        require(world.candidates(doc) == {roots[first]}, str(doc))
        code, _, err = world.real_run("positive prune", boot, plan, sha)
        require(code == 0, err)
        require(not os.path.lexists(roots[first]), "candidate survives")
        receipt = permanent.read_json(permanent.receipt_file(world.pool, checkouts[first]))
        require(receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted", str(receipt))
        tombstones = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))["entries"]
        require(roots[first] in {entry["root"] for entry in tombstones}, "tombstones")
        require(not world.intents() and not world.quarantines(), "intents remain")
        require(prior.tree_snapshot(roots[2]) == keep_before and (os.lstat(roots[2]).st_dev, os.lstat(roots[2]).st_ino) == keep_identity, "kept root changed")
    collector.case("positive-grouped", positive)


def run_internal(proof, adapter, world, boot, collector):
    template = world.checkout("template")
    require(not os.path.lexists(os.path.join(template, "target")), "fixture target already exists")
    world.adopt(template)
    marker = prior.marker(world.selected(template)["root"])
    legacy_checkout, legacy_root = legacy_fixture(world, adapter.root, marker)
    listed = os.path.join(adapter.root, "operator-list.json")
    Path(listed).write_text(json.dumps([legacy_root]))

    def legacy_positive():
        plan, sha, doc = world.plan_file("legacy plan", boot, listed)
        require(doc["coverage"]["complete"] and world.candidates(doc) == {legacy_root}, str(doc))
        walk = proof.root / "prune-walk.log"
        code, _, err = world.real_run("legacy prune", boot, plan, sha, listed,
            {"OXIDE_TEST_WALK_LOG": str(walk)})
        require(code == 0, err)
        require(not os.path.lexists(legacy_root), "legacy root survives")
        receipt = permanent.read_json(permanent.receipt_file(world.pool, legacy_checkout))
        require(receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted", str(receipt))
        tombstones = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))["entries"]
        require(legacy_root in {entry["root"] for entry in tombstones}, "tombstones")
        require(not world.intents() and not world.quarantines(), "intents remain")
        require(walk.read_text().splitlines().count(legacy_root) == 1, "receiptless locked recheck walked payload")
    collector.case("receiptless-legacy-positive", legacy_positive)


def run_mode(proof, custody, mode, diagnostic, cases):
    adapter, world, boot = prepare(proof, custody, mode)
    collector = Collector(proof, mode, diagnostic, cases)
    if mode == "builds":
        run_builds(proof, adapter, world, boot, collector)
    else:
        run_internal(proof, adapter, world, boot, collector)
    return cases


def self_check() -> int:
    """Pure counterfactuals: exercise the real seams with fakes, no child or fixture."""
    import errno
    from types import SimpleNamespace
    from unittest.mock import patch
    import tempfile

    # Mode and argument boundaries, both directions.
    require(parse_args(["--self-check"]) == ("self-check", None, False), "self-check argument")
    require(parse_args(["--plan"]) == ("plan", None, False), "plan argument")
    require(parse_args([]) == ("run", "builds", False), "builds argument")
    require(parse_args(["--diagnostic"]) == ("run", "builds", True), "builds diagnostic argument")
    require(parse_args(["--internal"]) == ("run", "internal", False), "internal argument")
    require(parse_args(["--internal", "--diagnostic"]) == ("run", "internal", True), "internal diagnostic argument")
    for bad in (["--internal", "--internal"], ["--plan", "--internal"], ["--self-check", "--diagnostic"], ["--extra"]):
        try:
            parse_args(bad)
        except SystemExit:
            pass
        else:
            raise AssertionError(f"argument {bad} was accepted")

    # Case tables: each mode accepts its own cases in order and refuses the other's.
    check_case("builds", "accounting")
    check_case("builds", "receiptless-external-unknown")
    check_case("internal", "receiptless-legacy-positive")
    for mode, name in (("internal", "accounting"), ("builds", "receiptless-legacy-positive"), ("nope", "accounting")):
        try:
            check_case(mode, name)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"{mode} accepted {name}")
    collector = Collector(SimpleNamespace(closure_known=True), "internal", False, [])
    try:
        collector.case("accounting", lambda: None)
    except RuntimeError:
        pass
    else:
        raise AssertionError("internal collector accepted a builds case")
    ordered = Collector(SimpleNamespace(closure_known=True), "builds", False, [])
    ordered.case("accounting", lambda: None)
    try:
        ordered.case("revalidation", lambda: None)
    except RuntimeError:
        pass
    else:
        raise AssertionError("builds collector accepted an out-of-order case")

    # The scratch layout is inside the run root and the pool inside the scratch root.
    scratch, pool = scratch_layout("/fake/run")
    require(scratch == "/fake/run/scratch-pools" and pool == "/fake/run/scratch-pools/pool", "scratch layout")
    require(pool.startswith(scratch + os.sep) and scratch.startswith("/fake/run" + os.sep), "pool escaped the scratch root")
    world = SimpleNamespace(pool="/old", control="", outputs="", registry="")
    place_pool(world, pool)
    require(world.registry == pool + "/oxide-managed/outputs/registry.json", "placed registry path")

    # The fixture environment contract. Builds declares OXIDE_SCRATCH_ROOT and the
    # pool inside it, whatever the host or base env carried; internal declares none.
    import inspect
    hostile = {"OXIDE_SCRATCH_ROOT": "/hostile/scratch", "OXIDE_POOL": "/hostile/pool",
               "OXIDE_POOL_CAP": "1MiB", "PATH": "/usr/bin"}
    built = declare_scratch(hostile, "builds", "/fake/run")
    require(built["OXIDE_SCRATCH_ROOT"] == "/fake/run/scratch-pools", f"builds scratch: {built}")
    require(built["OXIDE_POOL"] == "/fake/run/scratch-pools/pool", f"builds pool: {built}")
    require(built["OXIDE_POOL"].startswith(built["OXIDE_SCRATCH_ROOT"] + os.sep), "builds pool escaped the scratch root")
    require(built["OXIDE_POOL_CAP"] == "64MiB" and built["PATH"] == "/usr/bin", "builds cap or base env drifted")
    require(hostile["OXIDE_SCRATCH_ROOT"] == "/hostile/scratch", "declare_scratch mutated its input")
    internal = declare_scratch(hostile, "internal", "/fake/run")
    require("OXIDE_SCRATCH_ROOT" not in internal, "internal mode declared a scratch root")
    require(internal["OXIDE_POOL"] == "/hostile/pool", "internal mode rewrote the pool it was given")
    require(internal["OXIDE_POOL_CAP"] == "64MiB", "internal cap drifted")
    try:
        declare_scratch(hostile, "nope", "/fake/run")
    except RuntimeError:
        pass
    else:
        raise AssertionError("an unknown mode was accepted by declare_scratch")
    # prepare() must route the real fixture env through declare_scratch, so the
    # pure contract above is the one the run uses (not a parallel copy).
    prepare_source = inspect.getsource(prepare)
    require("declare_scratch(permanent.proof_env(" in prepare_source,
            "prepare() does not apply the scratch declaration")
    require("scratch_layout(" in inspect.getsource(declare_scratch), "declare_scratch does not use scratch_layout")

    # The unknown-external oracle: kept for the planted reason, never a candidate.
    good = {"coverage": {"complete": True}, "candidates": [{"root": "/r1"}],
            "kept": [{"root": "/legacy", "reason": "checkout absence is unknown (a /Volumes checkout has no recorded volume UUID), so its root is kept"}]}
    require_unknown_kept(good, "/legacy", "no recorded volume UUID")
    counterfactuals = [
        ({**good, "candidates": [{"root": "/legacy"}]}, "pruned"),
        ({**good, "coverage": {"complete": False}}, "incomplete"),
        ({**good, "kept": [{"root": "/legacy", "reason": "the checkout is present, so its root is kept"}]}, "present"),
        ({**good, "kept": [{"root": "/legacy", "reason": "checkout absence is unknown (an unrelated reason), so its root is kept"}]}, "unrelated"),
        ({**good, "kept": []}, "not kept"),
    ]
    for doc, label in counterfactuals:
        try:
            require_unknown_kept(doc, "/legacy", "no recorded volume UUID")
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"unknown-external oracle accepted: {label}")

    # The batch-refusal oracle still rejects an unrelated incomplete snapshot.
    for expected in ("the receipt /fixture is missing", "/fixture is invalid:",
                     "/fixture exceeds its 65536-byte limit", "the registry is sticky-incomplete",
                     "writer custody is unknown (injected fixture)"):
        doc = {"coverage": {"complete": False}, "candidates": [],
               "kept": [{"reason": "the registry snapshot is incomplete (unrelated fault); the whole batch is kept"}]}
        try:
            require_batch_refusal(doc, expected)
        except RuntimeError:
            pass
        else:
            raise AssertionError(f"unrelated fault satisfied {expected}")
        doc["kept"][0]["reason"] = f"the registry snapshot is incomplete ({expected}); the whole batch is kept"
        require_batch_refusal(doc, expected)

    # Exercise the actual host Python API, without creating a fixture: this
    # script exists and a child below the script file cannot exist.
    existing = Path(__file__).resolve(strict=True)
    require(existing.is_file(), "strict resolution of an existing file")
    try:
        (existing / "missing").resolve(strict=True)
    except OSError:
        pass
    else:
        raise AssertionError("strict resolution accepted a missing path")

    # A private temporary fixture exercises the actual host umask and the same
    # exclusive restoration used after the missing-receipt case.
    with tempfile.TemporaryDirectory(prefix="prune-receipt-") as directory_path:
        receipt = Path(directory_path) / "receipt"
        original = b"receipt bytes\n"
        fd = os.open(receipt, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(original)
        mode = stat.S_IMODE(os.lstat(receipt).st_mode)
        receipt.unlink()
        restore_missing_receipt(receipt, original, mode)
        require(receipt.read_bytes() == original and stat.S_IMODE(os.lstat(receipt).st_mode) == mode, "receipt restore")
        try:
            restore_missing_receipt(receipt, b"replacement", mode)
        except FileExistsError:
            pass
        else:
            raise AssertionError("restoration replaced an existing receipt")
        require(receipt.read_bytes() == original, "existing receipt changed")

    seed = wrapper.offline_seed
    directory = SimpleNamespace(st_mode=stat.S_IFDIR | 0o700, st_dev=7, st_ino=11)
    def fake_open(path, flags, **kwargs):
        if path == "registry":
            raise OSError(errno.ENOTDIR, "synthetic symlink")
        return 10
    with patch.object(seed.os, "open", side_effect=fake_open), \
         patch.object(seed.os, "fstat", return_value=directory), \
         patch.object(seed.os, "dup", return_value=11), \
         patch.object(seed.os, "close"):
        with seed.Anchor(Path("/fake/home"), "host") as anchor:
            try:
                anchor.open_dir(("registry",))
            except seed.SeedRefusal as error:
                require("not a real directory" in str(error), str(error))
            else:
                raise AssertionError("the symlinked-home counterfactual was accepted")
    require(canonical_layout(Path("/cache/registry"), Path("/cache/git")) == Path("/cache"), "canonical root")
    try:
        canonical_layout(Path("/cache/registry"), Path("/other/git"))
    except RuntimeError:
        pass
    else:
        raise AssertionError("different cache roots were accepted")
    registry = SimpleNamespace(st_mode=stat.S_IFDIR | 0o700, st_dev=7, st_ino=12)
    git = SimpleNamespace(st_mode=stat.S_IFDIR | 0o700, st_dev=7, st_ino=13)
    resolved = {"/fake/home/registry": "/cache/registry", "/fake/home/git": "/cache/git", "/cache": "/cache"}
    metadata = {"/cache": directory, "/cache/registry": registry, "/cache/git": git,
                "/fake/home/registry": registry, "/fake/home/git": git}
    def open_real(path, flags, **kwargs):
        return {"/cache": 10, "registry": 12, "git": 13}[str(path)]
    with patch.object(Path, "resolve", autospec=True, side_effect=lambda path, **kwargs: Path(resolved[str(path)])), \
         patch.object(os, "lstat", side_effect=lambda path: metadata[str(path)]), \
         patch.object(os, "stat", side_effect=lambda path: metadata[str(path)]), \
         patch.object(os, "open", side_effect=open_real), \
         patch.object(os, "fstat", side_effect=lambda fd: {10: directory, 12: registry, 13: git}[fd]), \
         patch.object(os, "dup", return_value=11), patch.object(os, "close"):
        bound = verified_seed_source(Path("/fake/home"))
        require(bound["root"] == "/cache" and bound["identity"]["git"] == [7, 13], "verified cache binding")
        metadata["/fake/home/git"] = directory
        try:
            verified_seed_source(Path("/fake/home"))
        except RuntimeError:
            pass
        else:
            raise AssertionError("changed account-home link was accepted")

    # The effect-free plan: both modes, their tables and nine premise dispositions.
    document = plan_document()
    require(document["effects"] == "none" and len(document["premise_checklist"]) == 9, "plan premise count")
    require(document["modes"]["builds"]["cases"] == list(MODE_CASES["builds"]), "builds plan table")
    require(document["modes"]["internal"]["cases"] == ["receiptless-legacy-positive"], "internal plan table")
    require("OXIDE_SCRATCH_ROOT" in PLACEMENT["builds"] and "7aedd5af" in PLACEMENT["internal"], "placement text")

    # The effigy selectors: exact text, so a /private/tmp root cannot drift back into the Builds mode.
    for name, expected in EXPECTED_SELECTORS.items():
        require(selector_run(name) == expected, f"{name} run line drifted: {selector_run(name)}")
    require("--root-parent" not in selector_run("rust:prune-authority"), "builds selector names an internal root")

    print("prune authority seed-source and mode self-check: OK")
    return 0


def plan() -> int:
    print(json.dumps(plan_document(), indent=2), flush=True)
    return 0


def main():
    kind, mode, diagnostic = parse_args(sys.argv[1:])
    if kind == "self-check":
        return self_check()
    if kind == "plan":
        return plan()
    proof = PruneProof()
    require(wrapper.git(proof, "status", "--porcelain") == "", "source is not clean")
    custody = prior.ImageCustody("Ox134" + secrets.token_hex(4))
    cases, error = [], None
    try:
        run_mode(proof, custody, mode, diagnostic, cases)
    except BaseException as exc:
        error = exc
    finally:
        disposition = custody.detach_once(sessions_closed=proof.closure_known)
    recorded = {item["case"] for item in cases}
    cases.extend({"case": name, "state": "unexecuted"} for name in MODE_CASES[mode] if name not in recorded)
    report = {"mode": mode, "qualified": False if diagnostic else None, "cases": cases,
        "error": None if error is None else f"{type(error).__name__}: {error}",
        "custody": custody.state, "detach": disposition}
    if diagnostic:
        (proof.evidence / f"prune134-{mode}-diagnostic.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2), flush=True)
    if error is not None:
        raise error
    require(disposition == "completed" and all(case["state"] == "completed" for case in cases), "proof incomplete")


if __name__ == "__main__":
    main()
