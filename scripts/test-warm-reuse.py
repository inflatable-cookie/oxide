#!/usr/bin/env python3
"""#280 HIGH/Elevated correctness, whole command of qualified 300s/512MiB guard.

Native-premise checklist, including the child:
1. Ordinary managed compilation, invoking homes/toolchain, offline locked 1.97.1
   with auto-install disabled. The repository lock is the manifest's own lock.
2. No account API or private HOME: account-home override is inapplicable.
3. Child drops inherited OXIDE_*/XDG_* test/config controls except this guard's
   root/evidence/run protocol; fixture and diagnostic controls are explicit.
4. Compilation may use ordinary checkout adoption; no adoption oracle is claimed.
5. No external Cargo subcommand/fake tool. 6. No mounted image/root/volume action:
   the Rust volume adapter is synthetic verification, not kernel qualification.
7. Both flag encodings are fixed empty for ordinary managed compilation.
8. Tiny fixture trees are exclusive children of current evidence, within the
   guard's whole-root byte sampling. Temp uses fresh scratch. Compiler output
   remains ordinary managed storage. No process escapes the guard's custody.
9. tempfile.mkdtemp always has an explicit current owned parent.

No per-task supervision, timeout reset, cleanup, metadata-actor probe, pool scan
or cost benchmark. Diagnostic retains independent case failures, always exits1
and qualified:false. Unknown child/infrastructure outcome aborts. The guard
alone provides the fixed deadline, host reserve, custody and scratch cleanup.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

GUARD = "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b"
COMMAND = ["oxide", "test", "--lib", "--locked", "--offline",
           "lease::pool::warm_reuse_tests::warm_reuse_correctness_proof", "--",
           "--ignored", "--exact", "--nocapture", "--test-threads=1"]


FRONTDOOR = ["oxide", "test", "--bin", "oxide", "--locked", "--offline",
             "store::tests::warm_reuse_has_no_measurement_sidecar", "--",
             "--exact", "--nocapture", "--test-threads=1"]


def main():
    if sys.argv[1:] not in ([], ["--diagnostic"]):
        raise SystemExit("only --diagnostic is supported")
    repo = Path(__file__).resolve().parent.parent
    guard = repo / "scripts/bounded-test.py"
    if hashlib.sha256(guard.read_bytes()).hexdigest() != GUARD:
        raise SystemExit("qualified guard source differs")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo, text=True)
    if len(head) != 40 or dirty:
        raise SystemExit("proof requires clean published source")
    root = Path(os.environ["OXIDE_TASK_SCRATCH_ROOT"])
    evidence = Path(os.environ["OXIDE_PROOF_EVIDENCE"])
    if (not os.environ.get("OXIDE_PROOF_RUN_ID") or root.resolve() != root
            or evidence.resolve() != evidence or evidence.parent != root.parent
            or root.is_symlink() or evidence.is_symlink()):
        raise SystemExit("guard root/evidence identity unavailable")
    fixture = Path(tempfile.mkdtemp(prefix="warm-correctness-", dir=evidence))
    temporary = Path(tempfile.mkdtemp(prefix="warm-temp-", dir=root))
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("OXIDE_", "XDG_"))}
    for key in ("OXIDE_TASK_SCRATCH_ROOT", "OXIDE_PROOF_EVIDENCE", "OXIDE_PROOF_RUN_ID"):
        env[key] = os.environ[key]
    env.update(OXIDE_WARM_REUSE_FIXTURE=str(fixture), RUSTUP_TOOLCHAIN="1.97.1",
               RUSTUP_AUTO_INSTALL="0", CARGO_NET_OFFLINE="true",
               RUSTFLAGS="", CARGO_ENCODED_RUSTFLAGS="", TMPDIR=str(temporary),
               TMP=str(temporary), TEMP=str(temporary))
    diagnostic = sys.argv[1:] == ["--diagnostic"]
    if diagnostic:
        env["OXIDE_WARM_REUSE_DIAGNOSTIC"] = "1"
    log = evidence / "warm-reuse-child.log"
    # Foreground direct child, ordinary guard session/group inherited. Do not
    # add another deadline, process monitor, kill or closure inference here.
    with log.open("xb") as output:
        result = subprocess.run(COMMAND, cwd=repo, env=env, stdout=output,
                                stderr=subprocess.STDOUT, check=False)
    frontdoor_exit = None
    # Failure before the Rust packet is infrastructure, not an independent
    # oracle failure: preserve it without starting another child.
    cases_path = evidence / "warm-reuse-cases.json"
    if result.returncode == 0 or (diagnostic and cases_path.exists()):
        with log.open("ab") as output:
            frontdoor_exit = subprocess.run(FRONTDOOR, cwd=repo, env=env, stdout=output,
                                           stderr=subprocess.STDOUT, check=False).returncode
    frontend_path = evidence / "warm-reuse-frontdoor.json"
    frontend = json.loads(frontend_path.read_text()) if frontend_path.exists() else None
    cases = json.loads(cases_path.read_text()) if cases_path.exists() else None
    passed = (result.returncode == 0 and frontdoor_exit == 0 and frontend == {"passed": True}
              and cases is not None and cases.get("passed") is True
              and len(cases.get("cases", [])) == 31
              and all(row.get("passed") is True for row in cases["cases"]))
    receipt = dict(diagnostic=diagnostic, oracles_passed=passed,
                   guard_closure="requires separate parent receipt",
                   head=head, child_exit=result.returncode, cases=cases, command=COMMAND,
                   frontdoor_command=FRONTDOOR, frontdoor_exit=frontdoor_exit, frontdoor=frontend,
                   fixture=str(fixture), run=os.environ["OXIDE_PROOF_RUN_ID"],
                   guard_sha256=GUARD, synthetic_volume_binding=True,
                   kernel_mount_qualification=False,
                   sources={str(path.relative_to(repo)): hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in [repo / "src/lease/pool.rs",
                                         repo / "src/lease/pool/warm_reuse_tests.rs",
                                         repo / "src/bin/oxide/store.rs", repo / "src/bin/oxide/main.rs",
                                         Path(__file__).resolve()]})
    if diagnostic:
        receipt["qualified"] = False
    (evidence / "warm-reuse.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt), flush=True)
    return 0 if passed and not diagnostic else 1


if __name__ == "__main__":
    raise SystemExit(main())
