#!/usr/bin/env python3
"""Focused prune authority proof (oxide#134), under bounded-test.py.

One candidate binary; one private 128 MiB APFS image; no installed binary or
live pool. The supervisor owns reserve, deadline, scratch accounting and
closure. No new session or signal timer is introduced here. The private
CARGO_HOME is seeded by offline_seed from this HEAD's lock, then the existing
supervised staging helper uses exactly six Cargo jobs. Tiny fixtures build
sequentially with at most two jobs.

Prospective, UNMEASURED allocation: setup/source binding 10 s; offline seed and
stage 90 s; image/register/fixtures 25 s; accounting and injected 11 s delay
25 s; refusals and revalidation 50 s; positive pruning 30 s; ordinary detach,
custody and evidence 35 s; supervisor margin 35 s. Total 300 s. Scratch:
288 MiB seed/stage, 128 MiB image, 32 MiB fixtures, 32 MiB evidence, 32 MiB
margin. Staging requests 6 CPU / 8 GiB; fixture builds use at most two jobs.
All allocations include cleanup and are unmeasured, not proof of fit.

Premise checklist: (1) seed_and_stage and permanent.proof_env retain the host
rustup home, use private Cargo homes and 1.97.1 without install; the bootstrap
uses no --locked fixture call. (2) proof_env sets OXIDE_TEST_HOME_FROM_ENV=1.
(3) proof_env scrubs inherited Oxide, XDG and Cargo build controls before its
private settings. (4) run_cases asserts target absent and adopts explicitly.
(5) external Cargo subcommand argv: not used. (6) ImageCustody attaches with
-owners on and records attachment and one ordinary detach. (7) seed_and_stage
scrubs encoded and plain Rust flags; fixture RUSTFLAGS is explicitly empty and
encoded flags removed. (8) selector fixes /private/tmp for a genuine internal
pool and the supervisor monitors its scratch. (9) mktemp: not invoked; the
supervisor supplies the canonical scratch root. Direct children remain in its
session. Each case uses the World adapter under the same private environment.

Cases: (1) two registry groups, ordered delay target before a later group,
control plus 11-second fired delay and hardlink logical/allocated oracle;
(2) missing/corrupt/oversized receipt, sticky registry and positively fired
unknown-custody control all keep the entire batch; (3) changed identity before
real run and positively fired locked recheck refusal keep bytes; (4) exact plan
prunes a genuine grouped root and an operator-listed receiptless root, with
terminal receipts, tombstones, closed intents, kept bytes and a walk log proving
no additional receiptless locked walk. Diagnostic mode is uncounted; it records
reached/completed/failed/skipped and stops dependent cases on any fault. Each
native invocation needs independent source-fit, headroom and ROOT activation.
"""
from __future__ import annotations
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys
import traceback

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


def require(value, message):
    if not value:
        raise RuntimeError(message)


class PruneProof(wrapper.Proof):
    def env(self, extra=None):
        env = super().env(extra)
        for key in list(env):
            if key.startswith(("XDG_", "GIT_")):
                env.pop(key)
        return env


class Adapter:
    """Use the existing qualified-session child helper with World’s API."""
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


CASE_NAMES = ("accounting", "authority-refusals", "revalidation", "positive-group-and-receiptless")


def run_cases(proof, custody, diagnostic, cases):
    oxide, provenance = wrapper.seed_and_stage(proof, jobs=6)
    (proof.evidence / "prune134-provenance.json").write_text(json.dumps(provenance, indent=2))
    adapter = Adapter(proof, oxide)
    world = prior.World(adapter, adapter.root)
    home, cargo_home = proof.root / "home", proof.root / "fixture-cargo-home"
    home.mkdir(mode=0o700)
    cargo_home.mkdir(mode=0o700)
    adapter.env = permanent.proof_env(world.pool, str(home), str(cargo_home))
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
    tainted = False

    def case(name, body):
        nonlocal tainted
        if tainted:
            cases.append({"case": name, "state": "skipped", "reason": "earlier fixture state uncertain"})
            return
        cases.append({"case": name, "state": "reached"})
        try:
            body()
            cases[-1]["state"] = "completed"
        except BaseException as error:
            cases[-1].update(state="failed", reason=f"{type(error).__name__}: {error}")
            tainted = True
            if not diagnostic or isinstance(error, (prior.Aborted, KeyboardInterrupt, SystemExit)) or not proof.closure_known:
                raise

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
                stat = os.lstat(os.path.join(directory, name))
                if not os.path.isfile(os.path.join(directory, name)) or os.path.islink(os.path.join(directory, name)):
                    continue
                logical += stat.st_size
                key = (stat.st_dev, stat.st_ino)
                if key not in seen:
                    allocated += stat.st_blocks * 512
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
    case("accounting", accounting)

    def refusals():
        path = Path(permanent.receipt_path(world.pool, checkouts[second]))
        original = path.read_bytes()
        for label, content in (("missing", None), ("corrupt", b"{"), ("oversized", b"x" * 65537)):
            try:
                if content is None: path.unlink()
                else: path.write_bytes(content)
                code, out, _ = world.dry_run(label, boot)
                doc = json.loads(out)
                require(code == 0 and not doc["coverage"]["complete"] and not doc["candidates"], label)
            finally:
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
            require(code == 0 and not doc["coverage"]["complete"] and not doc["candidates"], "sticky")
        finally:
            registry.write_bytes(original)
        fired = proof.evidence / "prune-unknown-custody-fired"
        code, out, _ = world.dry_run("unknown custody", boot,
            extra={"OXIDE_TEST_PRUNE_UNKNOWN_CUSTODY": checkouts[first]})
        doc = json.loads(out)
        require(code == 0 and not doc["coverage"]["complete"] and not doc["candidates"] and fired.read_text().strip() == checkouts[first], "unknown custody")
        require(prior.tree_snapshot(roots[2]) == keep_before and (os.lstat(roots[2]).st_dev, os.lstat(roots[2]).st_ino) == keep_identity, "kept root changed")
    case("authority-refusals", refusals)

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
    case("revalidation", revalidation)

    def positive():
        legacy_checkout = os.path.join(adapter.root, "never", "legacy")
        os.makedirs(os.path.dirname(legacy_checkout), exist_ok=True)
        legacy_id = hashlib.sha256(legacy_checkout.encode()).hexdigest()
        legacy_root = os.path.join(world.mount, "oxide-outputs", "legacy-" + legacy_id[:8])
        os.mkdir(legacy_root, 0o700)
        legacy_marker = dict(marker)
        legacy_marker.pop("nonce", None)
        legacy_marker.pop("checkout_volume_uuid", None)
        legacy_marker.update(version="oxide.outputs.root/1", checkout=legacy_checkout, checkout_id=legacy_id, generation=1)
        Path(legacy_root, ".oxide-output.json").write_text(json.dumps(legacy_marker))
        Path(legacy_root, "payload").write_bytes(b"legacy")
        listed = Path(adapter.root, "operator-list.json")
        listed.write_text(json.dumps([legacy_root]))
        plan, sha, doc = world.plan_file("positive plan", boot, str(listed))
        require(world.candidates(doc) == {roots[first], legacy_root}, str(doc))
        walk = proof.root / "prune-walk.log"
        code, _, err = world.real_run("positive prune", boot, plan, sha, str(listed),
            {"OXIDE_TEST_WALK_LOG": str(walk)})
        require(code == 0, err)
        require(not os.path.lexists(roots[first]) and not os.path.lexists(legacy_root), "candidate survives")
        for checkout in (checkouts[first], legacy_checkout):
            receipt = permanent.read_json(permanent.receipt_file(world.pool, checkout))
            require(receipt["state"] == "pruned" and receipt["pruned"]["outcome"] == "deleted", str(receipt))
        tombstones = permanent.read_json(os.path.join(world.outputs, "tombstones.json"))["entries"]
        require({roots[first], legacy_root}.issubset({entry["root"] for entry in tombstones}), "tombstones")
        require(not world.intents() and not world.quarantines(), "intents remain")
        require(prior.tree_snapshot(roots[2]) == keep_before and (os.lstat(roots[2]).st_dev, os.lstat(roots[2]).st_ino) == keep_identity, "kept root changed")
        require(walk.read_text().splitlines().count(legacy_root) == 1, "receiptless locked recheck walked payload")
    case("positive-group-and-receiptless", positive)
    return cases


def main():
    diagnostic = sys.argv[1:] == ["--diagnostic"]
    if sys.argv[1:] not in ([], ["--diagnostic"]):
        raise SystemExit("expected no argument or --diagnostic")
    proof = PruneProof()
    require(wrapper.git(proof, "status", "--porcelain") == "", "source is not clean")
    custody = prior.ImageCustody("Ox134" + secrets.token_hex(4))
    cases, error = [], None
    try:
        run_cases(proof, custody, diagnostic, cases)
    except BaseException as exc:
        error = exc
    finally:
        disposition = custody.detach_once(sessions_closed=proof.closure_known)
    recorded = {item["case"] for item in cases}
    cases.extend({"case": name, "state": "unexecuted"} for name in CASE_NAMES if name not in recorded)
    report = {"qualified": False if diagnostic else None, "cases": cases,
        "error": None if error is None else f"{type(error).__name__}: {error}",
        "custody": custody.state, "detach": disposition}
    if diagnostic:
        (proof.evidence / "prune134-diagnostic.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2), flush=True)
    if error is not None:
        raise error
    require(disposition == "completed" and all(case["state"] == "completed" for case in cases), "proof incomplete")


if __name__ == "__main__":
    main()
