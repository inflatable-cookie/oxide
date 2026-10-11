"""One bounded, read-only acquisition for the pinned Nucleus input contract."""

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
import urllib.parse
from pathlib import Path

import pip._vendor.tomli as tomli

ROOT = Path("/Users/tom/Dev/projects/nucleus")
COMMIT = "576ba7904d58e4bf829ddddda89a06833b8c5087"
TREE = "0aa379b840c2f8990adeecc1fc49d9e57ab5869b"
ORIGIN = "git@github.com:inflatable-cookie/nucleus.git"
CARGO_HOME = "/Users/tom/.cargo"
LOCK_SHA256 = "ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d"
EXPECTED_LEAVES = (
    "Cargo.lock",
    "Cargo.toml",
    "crates/nucleus-core/Cargo.toml",
    "crates/nucleus-observer/Cargo.toml",
    "effigy.toml",
)
MAX_SOURCE_LEAF = 256 * 1024
MAX_SOURCE_TOTAL = 2 * 1024 * 1024
MAX_CONFIG = 64 * 1024
MAX_ARCHIVE_PHASE = 256 * 1024 * 1024
MAX_DIAGNOSTIC_OUTPUT = 256 * 1024
MAX_PACKAGE_INPUT = 16 * 1024 * 1024
MAX_DIAGNOSTIC_HEADERS = 64
MAX_TRIAGE_LEAVES = 128
MAX_TRIAGE_LEAF = 256 * 1024
MAX_TRIAGE_TOTAL = 16 * 1024 * 1024
MAX_OUTPUT = 8 * 1024 * 1024
RESERVE = 20 * 1024 * 1024 * 1024
# Bounded pinned-object/config reads performed during task preflight before this
# callable entry was added. They are included in the task's aggregate ledger.
PREFLIGHT_SOURCE_BYTES = 48600
PREFLIGHT_CONFIG_BYTES = 20


def run(argv, *, limit=None):
    result = subprocess.run(argv, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if limit is not None and len(result.stdout) > limit:
        raise RuntimeError(f"bounded subprocess output exceeded {limit} bytes")
    return result.stdout


def file_identity(st):
    return (st.st_dev, st.st_ino, st.st_mode, st.st_size, st.st_mtime_ns, st.st_ctime_ns)


def metadata(st):
    return {
        "device": st.st_dev,
        "inode": st.st_ino,
        "mode": st.st_mode,
        "size": st.st_size,
        "modified_ns": st.st_mtime_ns,
        "changed_ns": st.st_ctime_ns,
        "links": st.st_nlink,
    }


def source_blobs():
    if run(["git", "-C", str(ROOT), "config", "--get", "remote.origin.url"], limit=2048).decode().strip() != ORIGIN:
        raise RuntimeError("pinned source origin mismatch")
    tree = run(["git", "-C", str(ROOT), "rev-parse", f"{COMMIT}^{{tree}}"], limit=128).decode().strip()
    if tree != TREE:
        raise RuntimeError("pinned source tree mismatch")
    leaves = {}
    total = 0
    for path in EXPECTED_LEAVES:
        object_id = f"{COMMIT}:{path}"
        tree_entry = run(["git", "-C", str(ROOT), "ls-tree", COMMIT, "--", path], limit=2048).decode().strip()
        metadata, found_path = tree_entry.split("\t", 1)
        mode, object_type, _ = metadata.split()
        if found_path != path or mode != "100644" or object_type != "blob":
            raise RuntimeError(f"pinned source leaf is not a blob: {path}")
        size = int(run(["git", "-C", str(ROOT), "cat-file", "-s", object_id], limit=32))
        if size > MAX_SOURCE_LEAF or PREFLIGHT_SOURCE_BYTES + total + size > MAX_SOURCE_TOTAL:
            raise RuntimeError(f"pinned source leaf budget exceeded: {path}")
        raw = run(["git", "-C", str(ROOT), "cat-file", "blob", object_id], limit=MAX_SOURCE_LEAF)
        if len(raw) != size:
            raise RuntimeError(f"pinned source leaf size changed: {path}")
        total += size
        leaves[path] = {
            "bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
            "raw": raw,
        }
    if hashlib.sha256(leaves["Cargo.lock"]["raw"]).hexdigest() != LOCK_SHA256:
        raise RuntimeError("pinned lock digest mismatch")
    return leaves, total


def safe_mapping_value(value):
    value = str(value)
    parsed = urllib.parse.urlsplit(value)
    if parsed.username is not None or parsed.password is not None:
        host = parsed.hostname or ""
        if parsed.port:
            host += f":{parsed.port}"
        value = urllib.parse.urlunsplit((parsed.scheme, f"<redacted>@{host}", parsed.path, "<redacted>", ""))
    elif parsed.query:
        value = urllib.parse.urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "<redacted>", ""))
    return value


def read_configs():
    observations = []
    mappings = []
    custom = False
    total = 0
    for path in (Path(CARGO_HOME) / "config", Path(CARGO_HOME) / "config.toml"):
        try:
            before = os.lstat(path)
        except FileNotFoundError:
            observations.append({"path": str(path), "state": "absent"})
            continue
        if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode) or before.st_size > MAX_CONFIG:
            observations.append({"path": str(path), "state": "refused_type_or_size", "metadata": metadata(before)})
            custom = True
            continue
        with path.open("rb") as stream:
            raw = stream.read(MAX_CONFIG + 1)
        after = os.lstat(path)
        stable = file_identity(before) == file_identity(after) and len(raw) == before.st_size
        observations.append({
            "path": str(path), "state": "read" if stable else "refused_unstable",
            "bytes": len(raw), "metadata_before": metadata(before),
            "metadata_after": metadata(after), "stable": stable,
        })
        total += len(raw)
        if not stable:
            custom = True
            continue
        config = tomli.loads(raw.decode("utf-8"))
        registry = config.get("registry", {})
        if isinstance(registry, dict):
            for key in ("default",):
                if key in registry:
                    mappings.append({"table": "registry", "key": key, "value": safe_mapping_value(registry[key])})
                    custom = True
        elif registry:
            custom = True
        registries = config.get("registries", {})
        if registries:
            custom = True
            if isinstance(registries, dict):
                for name, values in sorted(registries.items()):
                    if isinstance(values, dict) and "index" in values:
                        mappings.append({"table": f"registries.{name}", "key": "index", "value": safe_mapping_value(values["index"])})
        sources = config.get("source", {})
        if sources:
            if not isinstance(sources, dict):
                custom = True
                continue
            for name, values in sorted(sources.items()):
                if name != "crates-io":
                    custom = True
                if not isinstance(values, dict):
                    custom = True
                    continue
                for key, value in sorted(values.items()):
                    if key in {"replace-with", "registry", "local-registry", "directory"}:
                        mappings.append({"table": f"source.{name}", "key": key, "value": safe_mapping_value(value)})
                        custom = True
                    elif key.lower() in {"token", "password", "credential-provider", "credential-providers"}:
                        mappings.append({"table": f"source.{name}", "key": key, "value": "<redacted>"})
                    else:
                        mappings.append({"table": f"source.{name}", "key": key, "value": "<omitted>"})
                        custom = True
    if PREFLIGHT_CONFIG_BYTES + total > 2 * MAX_CONFIG:
        raise RuntimeError("Cargo config read budget exceeded")
    return observations, mappings, custom, total


def lock_inventory(raw):
    lock = tomli.loads(raw.decode("utf-8"))
    entries = lock.get("package")
    if lock.get("version") != 4 or not isinstance(entries, list) or len(entries) != 62:
        raise RuntimeError("unsupported or changed Cargo.lock shape")
    registry = []
    local = []
    for entry in entries:
        if not all(key in entry for key in ("name", "version")):
            raise RuntimeError("malformed lock package entry")
        item = {
            "name": entry["name"],
            "version": entry["version"],
            "source": entry.get("source"),
            "checksum": entry.get("checksum"),
            "dependencies": entry.get("dependencies", []),
        }
        if item["source"] is None:
            local.append(item)
        elif item["source"] == "registry+https://github.com/rust-lang/crates.io-index" and isinstance(item["checksum"], str) and re.fullmatch(r"[0-9a-f]{64}", item["checksum"]):
            registry.append(item)
        else:
            raise RuntimeError("non-crates.io or unchecksummed lock source refuses acquisition")
    if len(registry) != 58 or len(local) != 4:
        raise RuntimeError("lock package inventory count changed")
    return registry, local


def manifest_summary(raw, key):
    parsed = tomli.loads(raw.decode("utf-8"))
    if key == "Cargo.toml":
        return {
            "workspace_members": parsed.get("workspace", {}).get("members", []),
            "resolver": parsed.get("workspace", {}).get("resolver"),
            "profile_dev_debug": parsed.get("profile", {}).get("dev", {}).get("debug"),
        }
    package = parsed.get("package", {})
    dependencies = {}
    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        dependencies[section] = parsed.get(section, {})
    return {
        "name": package.get("name"), "version": package.get("version"),
        "edition": package.get("edition"), "rust_version": package.get("rust-version"),
        "dependencies": dependencies,
    }


def locked_reachable(registry, local):
    entries = registry + [dict(item, source=None, checksum=None) for item in local]
    by_key = {(item["name"], item["version"]): item for item in entries}
    by_name = {}
    for key in by_key:
        by_name.setdefault(key[0], []).append(key)
    start = ("nucleus-core", "0.1.0")
    pending = [start]
    reached = set()
    unresolved = []
    while pending:
        key = pending.pop()
        if key in reached or key not in by_key:
            continue
        reached.add(key)
        for reference in by_key[key].get("dependencies", []):
            parts = reference.split()
            if not parts:
                unresolved.append(reference)
                continue
            if len(parts) > 1 and re.fullmatch(r"\d[^ ]*", parts[1]):
                target = (parts[0], parts[1])
                if target in by_key:
                    pending.append(target)
                else:
                    unresolved.append(reference)
            elif len(by_name.get(parts[0], [])) == 1:
                pending.append(by_name[parts[0]][0])
            else:
                unresolved.append(reference)
    return [f"{name} {version}" for name, version in sorted(reached)], unresolved


def checked_triage_read(package, relative, role, triage):
    manifest = package["files"]
    expected = manifest.get(relative)
    if expected is None:
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "not_in_verified_manifest", "role": role})
        return None
    key = f"{package['package']['name']}@{package['package']['version']}:{relative}"
    if key in triage["cache"]:
        found = triage["by_key"][key]
        if role not in found["roles"]:
            found["roles"].append(role)
        return triage["cache"][key]
    if len(triage["leaves"]) >= MAX_TRIAGE_LEAVES or triage["bytes"] >= MAX_TRIAGE_TOTAL:
        triage["truncated"] = True
        return None
    root = Path(package["extracted_root"])
    path = root.joinpath(*relative.split("/"))
    try:
        before = os.lstat(path)
    except OSError as exc:
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": f"metadata_{type(exc).__name__}", "role": role})
        return None
    if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "unsafe_type_or_link", "role": role})
        return None
    if before.st_size > MAX_TRIAGE_LEAF:
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "leaf_over_256_kib", "size": before.st_size, "role": role})
        return None
    canonical = path.resolve(strict=True)
    if not canonical.is_relative_to(root.resolve(strict=True)):
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "escaped_root", "role": role})
        return None
    if len(triage["leaves"]) >= MAX_TRIAGE_LEAVES or triage["bytes"] + before.st_size > MAX_TRIAGE_TOTAL:
        triage["truncated"] = True
        return None
    with path.open("rb") as stream:
        raw = stream.read(MAX_TRIAGE_LEAF + 1)
    after = os.lstat(path)
    if len(raw) != before.st_size or file_identity(before) != file_identity(after):
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "unstable_read", "role": role})
        return None
    digest = "sha256:" + hashlib.sha256(raw).hexdigest()
    if digest != expected:
        triage["unresolved"].append({"package": package["package"]["name"], "path": relative, "reason": "digest_mismatch", "role": role})
        return None
    found = {"package": package["package"]["name"], "version": package["package"]["version"], "path": relative, "sha256": digest, "bytes": len(raw), "roles": [role]}
    triage["leaves"].append(found)
    triage["by_key"][key] = found
    triage["cache"][key] = raw
    triage["bytes"] += len(raw)
    return raw


def behavior_triage(report, reachable):
    triage = {"leaves": [], "unresolved": [], "findings": [], "bytes": 0, "truncated": False, "by_key": {}, "cache": {}}
    reachable_set = set(reachable)
    package_rows = []
    selected_names = []
    for item in report["packages"]:
        if item["status"] != "supported":
            continue
        locked = item["package"]
        identity = f"{locked['name']} {locked['version']}"
        if identity in reachable_set:
            selected_names.append(identity)
        manifest_bytes = checked_triage_read(item, "Cargo.toml", "cargo_manifest", triage)
        if manifest_bytes is None:
            continue
        try:
            manifest = tomli.loads(manifest_bytes.decode("utf-8"))
        except (UnicodeDecodeError, tomli.TOMLDecodeError):
            triage["unresolved"].append({"package": identity, "path": "Cargo.toml", "reason": "unsupported_manifest"})
            continue
        package_table = manifest.get("package", {})
        if package_table.get("name") != locked["name"] or package_table.get("version") != locked["version"]:
            triage["unresolved"].append({"package": identity, "path": "Cargo.toml", "reason": "manifest_lock_identity_mismatch"})
            continue
        library = manifest.get("lib", {})
        is_macro = bool(library.get("proc-macro", False))
        lib_path = library.get("path", "src/lib.rs")
        build_value = package_table.get("build")
        if build_value is False:
            build_path = None
        elif build_value is True:
            build_path = "build.rs"
        elif build_value is None:
            build_path = "build.rs" if "build.rs" in item["files"] else None
        else:
            build_path = build_value
        if build_path and build_path in item["files"]:
            work = [(build_path, "build_script")]
        else:
            work = []
            if build_path and build_path not in item["files"]:
                triage["unresolved"].append({"package": identity, "path": build_path, "reason": "declared_build_script_missing"})
        if is_macro:
            work.append((lib_path, "proc_macro_entry"))
        features = manifest.get("features", {})
        package_rows.append({
            "package": identity, "reachable_from_nucleus_core_lock_graph": identity in reachable_set,
            "proc_macro": is_macro, "proc_macro_entry": lib_path if is_macro else None,
            "build_script": build_path if build_path and build_path in item["files"] else None,
            "links": package_table.get("links"),
            "declared_features": sorted(features),
            "default_features_declared": features.get("default", []),
        })
        seen = set()
        while work and not triage["truncated"]:
            relative, role = work.pop(0)
            if relative in seen:
                continue
            seen.add(relative)
            raw = checked_triage_read(item, relative, role, triage)
            if raw is None:
                continue
            text = raw.decode("utf-8", errors="replace")
            selected_lines = []
            for line_number, line in enumerate(text.splitlines(), 1):
                if re.search(r"(?i)(include|read_to_string|File::open|Command::new|rerun-if|cc::Build|pkg.config|sqlite|proc_macro|CARGO_FEATURE|bindgen)", line):
                    selected_lines.append({"line": line_number, "text": line[:240]})
                for match in re.finditer(r"(?:include(?:_str|_bytes)?\s*!\s*\(\s*|#\[path\s*=\s*)\"([^\"]+)\"", line):
                    work.append(((Path(relative).parent / match.group(1)).as_posix(), "literal_behavior_reference"))
                module = re.search(r"^\s*mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", line)
                if module:
                    parent = Path(relative).parent
                    base = parent if role in {"build_script", "proc_macro_entry"} or Path(relative).name == "mod.rs" else parent / Path(relative).stem
                    module_file = (base / f"{module.group(1)}.rs").as_posix()
                    module_directory = (base / module.group(1) / "mod.rs").as_posix()
                    if module_file in item["files"] and module_directory in item["files"]:
                        triage["unresolved"].append({"package": identity, "path": module.group(1), "reason": "ambiguous_module_files"})
                    elif module_file in item["files"]:
                        work.append((module_file, "literal_module_reference"))
                    elif module_directory in item["files"]:
                        work.append((module_directory, "literal_module_reference"))
                    else:
                        triage["unresolved"].append({"package": identity, "path": module.group(1), "reason": "module_source_missing_from_verified_manifest"})
            if selected_lines:
                triage["findings"].append({"package": identity, "path": relative, "evidence_lines": selected_lines[:12]})
            for literal in re.findall(r"(?:read_to_string|File::open|fs::read|fs::metadata)\s*\(\s*\"([^\"]+)\"", text):
                if literal not in item["files"]:
                    triage["unresolved"].append({"package": identity, "path": literal, "reason": "literal_build_or_macro_read_not_in_verified_package"})
    triage.pop("by_key")
    triage.pop("cache")
    return {
        "status": "bounded_or_truncated" if triage["truncated"] else "bounded_observation",
        "locked_packages_with_verified_manifests": package_rows,
        "reachable_locked_packages_observed": sorted(set(selected_names)),
        "bytes_read": triage["bytes"],
        "leaves_read": len(triage["leaves"]),
        "leaves": triage["leaves"],
        "source_backed_behavior_lines": triage["findings"],
        "unresolved_or_referenced_evidence": triage["unresolved"],
        "enabled_unit_graph": "not established by Cargo.lock or manifests",
        "native_sdk_header_and_hidden_read_closure": "not established",
        "scripts_macros_and_helpers_executed": False,
    }


def diagnostic_receipt(registry):
    path_text = os.environ.get("OXIDE_TASK038_DIAGNOSTICS_PATH")
    expected_digest = os.environ.get("OXIDE_TASK038_DIAGNOSTICS_SHA256")
    if not path_text or not expected_digest or not re.fullmatch(r"[0-9a-f]{64}", expected_digest):
        raise RuntimeError("task 038 requires its bounded diagnostic receipt and SHA-256")
    path = Path(path_text)
    if not path.is_absolute() or path.name != "task038-archive-diagnostics.json":
        raise RuntimeError("diagnostic receipt path is outside the task-owned shape")
    before = os.lstat(path)
    scratch_root = Path("/private/tmp").resolve(strict=True)
    canonical = path.resolve(strict=True)
    if (
        stat.S_ISLNK(before.st_mode)
        or not stat.S_ISREG(before.st_mode)
        or before.st_size > MAX_DIAGNOSTIC_OUTPUT
        or not canonical.is_relative_to(scratch_root)
        or canonical != path
        or not path.parent.name.startswith("oxide-038-diag.")
    ):
        raise RuntimeError("diagnostic receipt type, location, or size refused")
    raw = path.read_bytes()
    after = os.lstat(path)
    if file_identity(before) != file_identity(after) or len(raw) != before.st_size:
        raise RuntimeError("diagnostic receipt changed while being read")
    digest = hashlib.sha256(raw).hexdigest()
    if digest != expected_digest:
        raise RuntimeError("diagnostic receipt digest does not match the charged phase record")
    diagnostic = json.loads(raw)
    if diagnostic.get("schema") != "oxide.registry-archive-diagnostics.task038.v1":
        raise RuntimeError("unsupported diagnostic receipt schema")
    rows = diagnostic.get("packages")
    by_name = {(item["name"], item["version"]): item for item in registry}
    if not isinstance(rows, list) or {row.get("package", {}).get("name") for row in rows} != {"bitflags", "cc"}:
        raise RuntimeError("diagnostic receipt does not cover the two observed refusal samples")
    namespaces = {"index.crates.io-1949cf8c6b5b557f", "index.crates.io-6f17d22bba15001f"}
    archive_bytes = decompressed_bytes = entries = 0
    for row in rows:
        package = row["package"]
        locked = by_name.get((package.get("name"), package.get("version")))
        if locked is None or any(package.get(key) != locked.get(key) for key in ("source", "checksum")):
            raise RuntimeError("diagnostic sample differs from the pinned lock tuple")
        namespace = row.get("recorded_namespace")
        expected_path = Path(CARGO_HOME) / "registry/cache" / namespace / f"{package['name']}-{package['version']}.crate"
        if namespace not in namespaces or row.get("archive") != str(expected_path):
            raise RuntimeError("diagnostic archive is not a literal recorded registry candidate")
        if (
            row.get("metadata_before") != row.get("metadata_after")
            or row.get("compressed_sha256") != package["checksum"]
            or row.get("locked_checksum_match") is not True
            or row.get("gzip_trailing_bytes") != 0
            or row.get("all_described_headers_regular_octal_and_bounded_path") is not True
        ):
            raise RuntimeError("diagnostic archive identity or bounded header result refused")
        compressed = row.get("compressed_bytes_read", MAX_PACKAGE_INPUT + 1)
        decompressed = row.get("decompressed_bytes_read", MAX_PACKAGE_INPUT + 1)
        headers = row.get("header_descriptions", [])
        if compressed > MAX_PACKAGE_INPUT or decompressed > MAX_PACKAGE_INPUT or len(headers) > MAX_DIAGNOSTIC_HEADERS:
            raise RuntimeError("diagnostic package input or header-description limit exceeded")
        archive_bytes += compressed
        decompressed_bytes += decompressed
        entries += len(headers)
    counters = {
        "archive_bytes": archive_bytes,
        "decompressed_bytes": decompressed_bytes,
        "extracted_bytes": 0,
        "entries": entries,
    }
    if diagnostic.get("phase_counters") != counters or diagnostic.get("candidate_path_metadata_observations") != 2:
        raise RuntimeError("diagnostic phase counters do not match the bounded receipt contents")
    if diagnostic.get("serde_pairing_observation", {}).get("complete_pair_count") != 1:
        raise RuntimeError("recorded serde pairing observation is not unique")
    return diagnostic, len(raw), digest, counters


def package_counter_ledger(report):
    previous = report["phase_start"]["counters"]
    rows = []
    for item in report["packages"]:
        after = item["counters_after"]
        delta = {key: after[key] - previous[key] for key in previous}
        rows.append({
            "package": f"{item['package']['name']} {item['package']['version']}",
            "refusal": item["refusal"],
            "before_cumulative": previous,
            "package_delta": delta,
            "after_cumulative": after,
        })
        previous = after
    if previous != report["counters"]:
        raise RuntimeError("per-package cumulative counters do not close to the phase total")
    return rows


def pairing_audit(report):
    results = []
    for item in report["packages"]:
        archives = [c for c in item["candidates"] if c["kind"] == "archive" and c["state"] == "present"]
        extractions = [c for c in item["candidates"] if c["kind"] == "extracted" and c["state"] == "present"]
        pairs = [(a, s) for a in archives for s in extractions if a["namespace"] == s["namespace"]]
        selected = next((pair for pair in pairs if pair[0]["path"] == item["archive"] and pair[1]["path"] == item["extracted_root"]), None)
        results.append({
            "package": f"{item['package']['name']} {item['package']['version']}",
            "complete_pair_namespaces": sorted({a["namespace"] for a, _ in pairs}),
            "selected_namespace": selected[0]["namespace"] if selected else None,
            "selected_pair_is_unique": len(pairs) == 1 and selected is not None,
            "unmatched_archive_namespaces": sorted({a["namespace"] for a in archives} - {a["namespace"] for a, _ in pairs}),
            "refusal": item["refusal"],
        })
    serde = next(item for item in results if item["package"] == "serde 1.0.229")
    serde["effective_cargo_selection"] = "not established by filesystem correspondence"
    return results


def main():
    if os.environ.get("CARGO_HOME") != CARGO_HOME:
        raise RuntimeError("explicit CARGO_HOME mismatch; stop for scope correction")
    implementation_commit = run(["git", "rev-parse", "HEAD"], limit=128).decode().strip()
    implementation_tree = run(["git", "rev-parse", "HEAD^{tree}"], limit=128).decode().strip()
    leaves, source_bytes = source_blobs()
    configs, mappings, custom_config, config_bytes = read_configs()
    if custom_config:
        raise RuntimeError("allowed Cargo config contains custom or unresolved registry/source mapping")
    root_manifest = tomli.loads(leaves["Cargo.toml"]["raw"].decode("utf-8"))
    core_manifest = tomli.loads(leaves["crates/nucleus-core/Cargo.toml"]["raw"].decode("utf-8"))
    observer_manifest = tomli.loads(leaves["crates/nucleus-observer/Cargo.toml"]["raw"].decode("utf-8"))
    effigy_manifest = tomli.loads(leaves["effigy.toml"]["raw"].decode("utf-8"))
    if effigy_manifest.get("tasks", {}).get("check:core:test") != "cargo test --offline {args}":
        raise RuntimeError("pinned Effigy workflow selector changed")
    registry, local = lock_inventory(leaves["Cargo.lock"]["raw"])
    reachable, unresolved_deps = locked_reachable(registry, local)
    diagnostics, diagnostic_output_bytes, diagnostic_digest, phase_counters = diagnostic_receipt(registry)
    before_free = shutil.disk_usage(CARGO_HOME).free
    if before_free < RESERVE:
        raise RuntimeError("host free space is below the 20 GiB acquisition reserve")
    source_identity = {
        "commit": COMMIT, "tree": TREE, "lock_sha256": LOCK_SHA256,
        "leaves": {path: item["sha256"] for path, item in leaves.items()},
    }
    request = {
        "schema": "oxide.registry-acquisition.request.v2",
        "source": source_identity,
        "phase_start": {
            "counters": phase_counters,
            "candidate_observations": diagnostics["candidate_path_metadata_observations"],
        },
        "packages": [{key: item[key] for key in ("name", "version", "source", "checksum")} for item in registry],
    }
    binary = Path("target/debug/oxide-registry-acquisition")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("private acquisition binary is absent; run the covering Effigy private-process selector")
    result = subprocess.run([str(binary.resolve())], input=json.dumps(request, separators=(",", ":")).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=300)
    if result.returncode != 0:
        raise RuntimeError(f"Oxide verifier process refused with exit {result.returncode}: {result.stderr.decode(errors='replace')[:512]}")
    if len(result.stdout) > MAX_OUTPUT:
        raise RuntimeError("Oxide verifier report exceeds the 8 MiB output ceiling")
    report = json.loads(result.stdout)
    if report.get("schema") != "oxide.registry-acquisition.v2" or report.get("refusal"):
        raise RuntimeError("Oxide verifier report refused output retention")
    expected_packages = [(item["name"], item["version"], item["checksum"]) for item in registry]
    observed_packages = [
        (item["package"]["name"], item["package"]["version"], item["package"]["checksum"])
        for item in report["packages"]
    ]
    if observed_packages != expected_packages or report["phase_start"]["counters"] != phase_counters:
        raise RuntimeError("refreshed report differs from the pinned 58-package request or phase start")
    after_free = shutil.disk_usage(CARGO_HOME).free
    if after_free < RESERVE:
        raise RuntimeError("host free space fell below the 20 GiB acquisition reserve")
    triage = behavior_triage(report, reachable)
    after_triage_free = shutil.disk_usage(CARGO_HOME).free
    if after_triage_free < RESERVE:
        raise RuntimeError("host free space fell below the 20 GiB reserve during triage")
    archive_reads = sum(report["counters"][key] for key in ("archive_bytes", "decompressed_bytes", "extracted_bytes"))
    diagnostic_archive_reads = sum(phase_counters[key] for key in ("archive_bytes", "decompressed_bytes", "extracted_bytes"))
    acquisition_archive_reads = archive_reads - diagnostic_archive_reads
    total_source_bytes = PREFLIGHT_SOURCE_BYTES + source_bytes
    total_config_bytes = PREFLIGHT_CONFIG_BYTES + config_bytes
    total_reads = total_source_bytes + total_config_bytes + archive_reads + triage["bytes_read"] + diagnostic_output_bytes
    if (
        total_source_bytes > MAX_SOURCE_TOTAL
        or total_config_bytes > 2 * MAX_CONFIG
        or archive_reads > MAX_ARCHIVE_PHASE
        or triage["bytes_read"] > MAX_TRIAGE_TOTAL
        or report["candidate_observations"] > 512
        or total_reads > 275 * 1024 * 1024
    ):
        raise RuntimeError("aggregate acquisition read ceiling exceeded")
    package_counters = package_counter_ledger(report)
    pairings = pairing_audit(report)
    pairing_failures = [
        item["package"] for item in pairings
        if next(package for package in report["packages"]
                if f"{package['package']['name']} {package['package']['version']}" == item["package"])["status"] == "supported"
        and not item["selected_pair_is_unique"]
    ]
    diagnostic_paths = {row["package"]["name"]: row for row in diagnostics["packages"]}
    diagnostic_refresh_match = []
    for name, diagnostic in diagnostic_paths.items():
        refreshed = next(item for item in report["packages"] if item["package"]["name"] == name)
        observed_archive_metadata = (refreshed.get("metadata_before") or [None])[0]
        diagnostic_refresh_match.append({
            "package": f"{name} {diagnostic['package']['version']}",
            "same_recorded_archive_path": refreshed.get("archive") == diagnostic["archive"],
            "same_lock_checksum": refreshed["package"]["checksum"] == diagnostic["package"]["checksum"],
            "metadata_matches_diagnostic": observed_archive_metadata == diagnostic["metadata_before"],
            "refresh_refusal": refreshed["refusal"],
            "verification_detail": refreshed.get("verification_detail"),
        })
    manifests = {
        key: manifest_summary(leaves[key]["raw"], key)
        for key in ("Cargo.toml", "crates/nucleus-core/Cargo.toml", "crates/nucleus-observer/Cargo.toml")
    }
    manifests["crates/nucleus-core/Cargo.toml"]["test_selector_dependencies"] = manifests["crates/nucleus-core/Cargo.toml"]["dependencies"]
    evidence = {
        "schema": "oxide.nucleus-registry-acquisition.evidence.v2",
        "scope": {
            "origin": ORIGIN, "source_commit": COMMIT, "source_tree": TREE,
            "selector": ["effigy", "check:core:test", "--", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"],
            "effective_command_from_pinned_effigy": ["cargo", "test", "--offline", "-p", "nucleus-core", "--test", "planning", "--test", "build_diagnostics"],
            "nucleus_cargo_invocations": 0,
        },
        "oxide_implementation": {
            "commit": implementation_commit,
            "tree": implementation_tree,
        },
        "source": {
            "identity_kind": "Git commit and tree plus per-leaf SHA-256; not Oxide framed source-content digest",
            "commit": COMMIT, "tree": TREE,
            "lock_bytes": leaves["Cargo.lock"]["bytes"], "lock_sha256": LOCK_SHA256,
            "source_leaf_count": len(leaves), "source_bytes_read": total_source_bytes,
            "source_bytes_in_acquisition_entry": source_bytes,
            "bounded_pinned_source_preflight_bytes": PREFLIGHT_SOURCE_BYTES,
            "leaves": {path: {"bytes": item["bytes"], "sha256": item["sha256"]} for path, item in leaves.items()},
            "workspace_manifests": manifests,
            "locked_registry_package_count": len(registry),
            "locked_local_package_count": len(local),
            "registry_packages": [{key: item[key] for key in ("name", "version", "source", "checksum")} for item in registry],
            "local_packages": [{key: item[key] for key in ("name", "version")} for item in local],
            "lock_graph_reachable_from_nucleus_core": reachable,
            "unresolved_lock_dependency_references": unresolved_deps,
            "features_and_enabled_units": "Cargo.lock and manifests do not establish the complete enabled unit/feature graph",
        },
        "configuration": {
            "explicit_cargo_home": CARGO_HOME,
            "files": configs,
            "config_preflight_bytes": PREFLIGHT_CONFIG_BYTES,
            "literal_registry_source_mappings": mappings,
            "mapping_result": "no replacement/custom source mapping in the two permitted Cargo config files",
            "effective_configuration_limit": "environment overrides and other Cargo config locations were not read; final effective Cargo configuration is not established",
        },
        "host_free_space": {
            "path_sampled": CARGO_HOME,
            "before_bytes": before_free,
            "after_registry_acquisition_bytes": after_free,
            "after_triage_bytes": after_triage_free,
            "reserve_bytes": RESERVE,
            "note": "free-space deltas are not SSD writes",
        },
        "archive_diagnostics": {
            "receipt_sha256": diagnostic_digest,
            "receipt_bytes_read": diagnostic_output_bytes,
            "receipt": diagnostics,
            "refresh_continuity": diagnostic_refresh_match,
        },
        "archive_verification": report,
        "pairing_audit": {
            "packages": pairings,
            "supported_packages_without_one_selected_complete_pair": pairing_failures,
            "filesystem_observation_is_not_effective_cargo_registry_selection": True,
        },
        "behavior_triage": triage,
        "read_ledger": {
            "source_bytes": total_source_bytes, "config_bytes": total_config_bytes,
            "source_bytes_in_acquisition_entry": source_bytes,
            "config_bytes_in_acquisition_entry": config_bytes,
            "archive_compressed_decompressed_and_extracted_bytes": archive_reads,
            "diagnostic_archive_bytes": diagnostic_archive_reads,
            "refreshed_58_package_archive_bytes": acquisition_archive_reads,
            "diagnostic_output_bytes": diagnostic_output_bytes,
            "triage_bytes": triage["bytes_read"], "aggregate_bytes": total_reads,
            "candidate_path_metadata_observations": report["candidate_observations"],
            "diagnostic_candidate_path_metadata_observations": diagnostics["candidate_path_metadata_observations"],
            "refreshed_candidate_path_metadata_observations": report["acquisition_candidate_observations"],
            "per_package_cumulative_counters_and_deltas": package_counters,
            "limits": {"source_total": MAX_SOURCE_TOTAL, "per_source_leaf": MAX_SOURCE_LEAF,
                       "archive_decode_extract_total": MAX_ARCHIVE_PHASE,
                       "per_package_archive_or_tar_input": MAX_PACKAGE_INPUT,
                       "diagnostic_output": MAX_DIAGNOSTIC_OUTPUT,
                       "diagnostic_headers_per_package": MAX_DIAGNOSTIC_HEADERS,
                       "triage_total": MAX_TRIAGE_TOTAL, "per_triage_leaf": MAX_TRIAGE_LEAF,
                       "triage_leaves": MAX_TRIAGE_LEAVES, "candidate_observations": 512,
                       "retained_output": MAX_OUTPUT},
        },
        "authority": {
            "verified_correspondence_is_not_an_issuer": True,
            "future_no_replacement_or_lease_time_immutability": "not established",
            "accepted_registry_owner": "none assumed",
        },
    }
    output = json.dumps(evidence, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    if len(output) > MAX_OUTPUT:
        raise RuntimeError("combined evidence exceeds the 8 MiB retained-output ceiling")
    scratch = Path(tempfile.mkdtemp(prefix="oxide-registry-acquisition-038-", dir="/private/tmp"))
    if not scratch.resolve().is_relative_to(Path("/private/tmp").resolve()):
        raise RuntimeError("task scratch escaped /private/tmp")
    target = scratch / "nucleus-registry-acquisition.json"
    target.write_bytes(output)
    if target.stat().st_size != len(output):
        raise RuntimeError("retained evidence size changed")
    supported = sum(item["status"] == "supported" for item in report["packages"])
    refused = len(report["packages"]) - supported
    print(json.dumps({
        "evidence": str(target), "bytes": len(output), "locked_registry_packages": len(registry),
        "supported": supported, "refused_or_unobserved": refused,
        "candidate_observations": report["candidate_observations"],
        "archive_counters": report["counters"], "triage_bytes": triage["bytes_read"],
        "diagnostic_output_bytes": diagnostic_output_bytes,
        "archive_reads_in_refresh": acquisition_archive_reads,
        "aggregate_bytes": total_reads,
    }, separators=(",", ":")))


if __name__ == "__main__":
    main()
