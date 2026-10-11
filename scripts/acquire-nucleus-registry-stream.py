"""One bounded, read-only streaming correspondence observation for the two
registry bundles Task 038 refused before tar parsing.

This is a distinct private opt-in caller, not a second verifier: it builds one
schema-v3 streaming request and delegates all archive/extraction checks to the
feature-gated Rust entry through its `--stream` route. Only the exact two pinned
SQLite tuples are eligible; the Rust entry refuses any other package set, mode
or schema, and no caller field can raise a member or aggregate ceiling.
"""

import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

CARGO_HOME = "/Users/tom/.cargo"
COMMIT = "576ba7904d58e4bf829ddddda89a06833b8c5087"
TREE = "0aa379b840c2f8990adeecc1fc49d9e57ab5869b"
LOCK_SHA256 = "ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d"
SOURCE_LEAVES = {
    "Cargo.lock": LOCK_SHA256,
    "Cargo.toml": "81f90a4e389cef850a26446900173d3bbe70ae6a31371abe5aae650a99ecca5a",
    "crates/nucleus-core/Cargo.toml": "3d0e9fd5ead4e189ccb8d9dfd3e87254b46a84f8ca3e741d38e045a0da6c2e48",
    "crates/nucleus-observer/Cargo.toml": "3bc428a6b5c2ede9f526e178f90d71da4f7a38222160514e7f6509ff8fa552a9",
    "effigy.toml": "f666615645cb59f24fea78fbc7c1627e7d76cb647355dd791f1c57a2207ab8c2",
}
PACKAGES = (
    {
        "name": "libsqlite3-sys",
        "version": "0.38.2",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "checksum": "f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8",
    },
    {
        "name": "sqlite-wasm-rs",
        "version": "0.5.5",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "checksum": "dc3efc0da82635d7e1ced0053bbbfa8c7ab9645d0bf36ceb4f7127bb85315d75",
    },
)
PRIOR_EVIDENCE = "docs/knowledge/evidence/nucleus-registry-acquisition-038.json"
MAX_STREAM_PHASE = 256 * 1024 * 1024
MAX_TOTAL_READS = 260 * 1024 * 1024
MAX_PRIOR_EVIDENCE = 1024 * 1024
MAX_OUTPUT = 8 * 1024 * 1024
RESERVE = 20 * 1024 * 1024 * 1024


def run_git(*arguments):
    return subprocess.run(
        ["git", *arguments], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    ).stdout


def prior_evidence():
    path = Path(PRIOR_EVIDENCE)
    before = os.lstat(path)
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
        raise RuntimeError("prior evidence is not a regular file")
    if before.st_size > MAX_PRIOR_EVIDENCE:
        raise RuntimeError("prior evidence exceeds the 1 MiB read bound")
    raw = path.read_bytes()
    after = os.lstat(path)
    if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
        after.st_dev,
        after.st_ino,
        after.st_size,
        after.st_mtime_ns,
    ) or len(raw) != before.st_size:
        raise RuntimeError("prior evidence changed while being read")
    receipt = json.loads(raw)
    report = receipt.get("archive_verification", {})
    return {
        "path": PRIOR_EVIDENCE,
        "bytes": len(raw),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "schema": receipt.get("schema"),
        "archive_schema": report.get("schema"),
        "archive_counters": report.get("counters"),
        "sqlite_refusals": [
            {
                "package": f"{item['package']['name']} {item['package']['version']}",
                "status": item["status"],
                "refusal": item["refusal"],
                "counters_after": item["counters_after"],
            }
            for item in report.get("packages", [])
            if item["package"]["name"] in {"libsqlite3-sys", "sqlite-wasm-rs"}
        ],
    }


def package_deltas(report):
    previous = report["phase_start"]["counters"]
    rows = []
    for item in report["packages"]:
        after = item["counters_after"]
        delta = {key: after[key] - previous[key] for key in previous}
        rows.append(
            {
                "package": f"{item['package']['name']} {item['package']['version']}",
                "status": item["status"],
                "refusal": item["refusal"],
                "archive": item.get("archive"),
                "extracted_root": item.get("extracted_root"),
                "selected_namespace_pair_unique": item["status"] == "supported",
                "files": len(item["files"]),
                "directories": len(item["directories"]),
                "before_cumulative": previous,
                "package_delta": delta,
                "after_cumulative": after,
            }
        )
        previous = after
    if previous != report["counters"]:
        raise RuntimeError("per-package cumulative counters do not close to the phase total")
    return rows


def main():
    if os.environ.get("CARGO_HOME") != CARGO_HOME:
        raise RuntimeError("explicit CARGO_HOME mismatch; stop for scope correction")
    binary = Path("target/debug/oxide-registry-acquisition")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError(
            "private streaming binary is absent; run the covering private-process selector"
        )
    implementation_commit = run_git("rev-parse", "HEAD").decode().strip()
    implementation_tree = run_git("rev-parse", "HEAD^{tree}").decode().strip()
    prior = prior_evidence()
    before_free = shutil.disk_usage(CARGO_HOME).free
    if before_free < RESERVE:
        raise RuntimeError("host free space is below the 20 GiB observation reserve")
    request = {
        "schema": "oxide.registry-acquisition.request.v3",
        "mode": "streaming",
        "source": {
            "commit": COMMIT,
            "tree": TREE,
            "lock_sha256": LOCK_SHA256,
            "leaves": dict(SOURCE_LEAVES),
        },
        "phase_start": {
            "counters": {
                "archive_bytes": 0,
                "decompressed_bytes": 0,
                "extracted_bytes": 0,
                "entries": 0,
            },
            "candidate_observations": 0,
        },
        "packages": list(PACKAGES),
    }
    result = subprocess.run(
        [str(binary.resolve()), "--stream"],
        input=json.dumps(request, separators=(",", ":")).encode(),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=300,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "streaming verifier process refused with exit "
            f"{result.returncode}: {result.stderr.decode(errors='replace')[:512]}"
        )
    if len(result.stdout) > MAX_OUTPUT:
        raise RuntimeError("streaming verifier report exceeds the 8 MiB output ceiling")
    report = json.loads(result.stdout)
    if (
        report.get("schema") != "oxide.registry-acquisition.v3"
        or report.get("mode") != "streaming"
        or report.get("refusal")
    ):
        raise RuntimeError("streaming verifier report does not carry schema-v3 streaming output")
    observed = [
        (item["package"]["name"], item["package"]["version"], item["package"]["checksum"])
        for item in report["packages"]
    ]
    expected = [(item["name"], item["version"], item["checksum"]) for item in PACKAGES]
    if observed != expected:
        raise RuntimeError("streaming report differs from the exact two-package request")
    if report["phase_start"]["counters"] != request["phase_start"]["counters"]:
        raise RuntimeError("streaming report did not start from the declared zero phase")
    archive_reads = sum(
        report["counters"][key]
        for key in ("archive_bytes", "decompressed_bytes", "extracted_bytes")
    )
    if archive_reads > MAX_STREAM_PHASE:
        raise RuntimeError("streaming archive reads exceed the 256 MiB owner-phase ceiling")
    total_reads = archive_reads + prior["bytes"] + len(result.stdout)
    if total_reads > MAX_TOTAL_READS:
        raise RuntimeError("aggregate streaming reads exceed the 260 MiB ceiling")
    after_free = shutil.disk_usage(CARGO_HOME).free
    if after_free < RESERVE:
        raise RuntimeError("host free space fell below the 20 GiB observation reserve")
    deltas = package_deltas(report)
    evidence = {
        "schema": "oxide.nucleus-registry-stream.evidence.v1",
        "scope": {
            "origin": "git@github.com:inflatable-cookie/nucleus.git",
            "source_commit": COMMIT,
            "source_tree": TREE,
            "route": "private streaming correspondence (schema v3)",
            "nucleus_cargo_invocations": 0,
            "legacy_verify_calls": 0,
        },
        "oxide_implementation": {
            "commit": implementation_commit,
            "tree": implementation_tree,
            "callable_api": "oxide_storage::registered::archive::stream_verify",
            "caller_entry": "oxide_storage::registered::archive_acquisition::run_streaming",
            "binary": "oxide-registry-acquisition --stream",
            "request_schema": "oxide.registry-acquisition.request.v3",
            "report_schema": "oxide.registry-acquisition.v3",
        },
        "source": {
            "identity_kind": "pinned Git commit/tree plus per-leaf SHA-256 constants",
            "commit": COMMIT,
            "tree": TREE,
            "lock_sha256": LOCK_SHA256,
            "leaves": SOURCE_LEAVES,
        },
        "request": request,
        "archive_verification": report,
        "per_package": deltas,
        "read_ledger": {
            "phase_start_counters": request["phase_start"]["counters"],
            "phase_final_counters": report["counters"],
            "archive_compressed_decompressed_and_extracted_bytes": archive_reads,
            "report_bytes_read": len(result.stdout),
            "prior_evidence": prior,
            "prior_evidence_bytes": prior["bytes"],
            "aggregate_bytes": total_reads,
            "limits": {
                "archive_decode_extract_total": MAX_STREAM_PHASE,
                "member_and_extracted_file": 16 * 1024 * 1024,
                "entries": 32768,
                "path_bytes": 2048,
                "depth": 32,
                "total_reads": MAX_TOTAL_READS,
                "prior_evidence": MAX_PRIOR_EVIDENCE,
                "retained_output": MAX_OUTPUT,
            },
        },
        "host_free_space": {
            "path_sampled": CARGO_HOME,
            "before_bytes": before_free,
            "after_bytes": after_free,
            "reserve_bytes": RESERVE,
            "note": "free-space deltas are not SSD-write measurements",
        },
        "authority": {
            "verified_correspondence_is_not_an_issuer": True,
            "native_sdk_header_and_hidden_read_closure": "not established",
            "accepted_registry_owner": "none assumed",
            "task_037_and_038_records_preserved": True,
            "legacy_verify_container_cap_unchanged": "16 MiB decoded container",
        },
    }
    output = json.dumps(evidence, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(output) > MAX_OUTPUT:
        raise RuntimeError("combined streaming evidence exceeds the 8 MiB output ceiling")
    scratch = Path(tempfile.mkdtemp(prefix="oxide-registry-stream-039-", dir="/private/tmp"))
    if not scratch.resolve().is_relative_to(Path("/private/tmp").resolve()):
        raise RuntimeError("task scratch escaped /private/tmp")
    target = scratch / "nucleus-registry-stream-039.json"
    target.write_bytes(output)
    if target.stat().st_size != len(output):
        raise RuntimeError("retained streaming evidence size changed")
    print(
        json.dumps(
            {
                "evidence": str(target),
                "bytes": len(output),
                "packages": [
                    {
                        "package": row["package"],
                        "status": row["status"],
                        "refusal": row["refusal"],
                        "files": row["files"],
                        "directories": row["directories"],
                        "delta": row["package_delta"],
                    }
                    for row in deltas
                ],
                "counters": report["counters"],
                "archive_reads": archive_reads,
                "aggregate_bytes": total_reads,
                "candidate_observations": report["candidate_observations"],
            },
            separators=(",", ":"),
        )
    )


if __name__ == "__main__":
    main()
