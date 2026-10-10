//! Test-only producer for the fixed, owned Cargo source-map probe.

use super::source_map::{ResolvedSourceMap, SOURCE_SCAN_VERSION};
use super::CancellationProbe;
use crate::storage::RunBinding;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const MAX_CARGO_LINE_BYTES: usize = 1024 * 1024;
const MAX_SCANNER_BYTES: usize = 1024 * 1024;
const MAX_ARTIFACTS: usize = 16;
const ORIGIN_FILE: &str = "oxide-compile-origins-v1.txt";
const SCANNER_TIMEOUT: Duration = Duration::from_secs(60);
const SCANNER_POLL: Duration = Duration::from_millis(20);

pub(super) struct CargoOutputSummary {
    pub rebuilt_units: usize,
    pub rebuilt_targets: Vec<String>,
    pub probe_values: BTreeMap<String, String>,
    pub artifacts: Vec<CargoArtifact>,
    pub build_script_dirs: Vec<PathBuf>,
}

#[derive(Clone)]
pub(super) struct CargoArtifact {
    pub unit_key: String,
    pub manifest: PathBuf,
    pub target_name: String,
    pub target_kinds: Vec<String>,
    pub fresh: bool,
    pub executable: Option<PathBuf>,
    pub filenames: Vec<PathBuf>,
}

#[derive(Clone)]
struct CompileOrigin {
    root_id: String,
    tree_digest: String,
    artifact_digest: String,
}

pub(super) fn inspect_cargo_output(stdout: &[u8]) -> CargoOutputSummary {
    let text = String::from_utf8_lossy(stdout);
    let mut artifacts = Vec::new();
    let mut build_script_dirs = Vec::new();
    let mut probe_values = BTreeMap::new();
    let mut duplicate_probe_value = false;

    for line in text.lines() {
        if line.len() > MAX_CARGO_LINE_BYTES {
            continue;
        }
        if line.starts_with('{') {
            match json_string(line, "reason").as_deref() {
                Some("compiler-artifact") => {
                    if let Some(artifact) = parse_artifact(line) {
                        artifacts.push(artifact);
                    }
                }
                Some("build-script-executed") => {
                    if let Some(path) = json_string(line, "out_dir") {
                        build_script_dirs.push(PathBuf::from(path));
                    }
                }
                _ => {}
            }
        }
        if let Some((_, value)) = line.split_once("OXIDE_PROBE\t") {
            if let Some((key, value)) = value.split_once('\t') {
                if !key.is_empty()
                    && !value.is_empty()
                    && probe_values
                        .insert(key.to_owned(), value.to_owned())
                        .is_some()
                {
                    duplicate_probe_value = true;
                }
            }
        }
    }

    if duplicate_probe_value {
        probe_values.clear();
    }
    let mut rebuilt_targets: Vec<_> = artifacts
        .iter()
        .filter(|artifact| !artifact.fresh)
        .map(|artifact| {
            format!(
                "{}:{}",
                artifact.target_name,
                artifact.target_kinds.join("+")
            )
        })
        .collect();
    rebuilt_targets.sort();
    rebuilt_targets.dedup();
    let rebuilt_units = artifacts.iter().filter(|artifact| !artifact.fresh).count();
    CargoOutputSummary {
        rebuilt_units,
        rebuilt_targets,
        probe_values,
        artifacts,
        build_script_dirs,
    }
}

pub(super) fn source_tree_digest(root: &Path) -> Result<String, String> {
    tree_digest(root)
}

pub(super) fn produce_preflight_observations(
    binding: &RunBinding,
    lease_id: &str,
    host_run_id: &str,
    workspace_root: &Path,
    slot_root: &Path,
    target_root: &Path,
    capture_root: &Path,
    stdout: &[u8],
    output_references: &mut Vec<String>,
    workflow_definition_digest: &str,
    process_start_identity: &str,
    start_marker_digest: &str,
    process_id: u32,
    environment_digest: &str,
    encoded_rustflags: &str,
    temp_dir: &Path,
    cancellation: &dyn CancellationProbe,
) -> Result<(), String> {
    let observed_tree = tree_digest(workspace_root)?;
    if observed_tree != binding.source_tree_digest {
        return Err("preflight requested tree changed during the owned command".to_owned());
    }
    let summary = inspect_cargo_output(stdout);
    let mut report = format!(
        "oxide-cargo-preflight-observations-v1\nrun={}\nattempt={}\nhost_run={}\nlease={}\ncommit={}\ntree={}\nproducer_source_digest={}\nworkflow_definition_digest={}\nprocess_start_identity={}\nstart_marker_digest={}\nprocess_id={}\nenvironment={}\nartifact_count={}\nrebuild_count={}\n",
        binding.run_id,
        binding.attempt_id,
        host_run_id,
        lease_id,
        binding.commit,
        observed_tree,
        super::cargo::source_digest_for_probe(),
        workflow_definition_digest,
        process_start_identity,
        start_marker_digest,
        process_id,
        environment_digest,
        summary.artifacts.len(),
        summary.rebuilt_units,
    );
    let provenance = format!(
        "oxide-cargo-preflight-provenance-v1\nproducer_source_digest={}\nworkflow_definition_digest={}\nprocess_start_identity={}\nstart_marker_digest={}\nprocess_id={}\nconsumer={}\nproject={}\ncomponent={}\ntask={}\nworkspace={}\ncommit={}\nworkflow={}\nrun={}\nattempt={}\nhost_run={}\nlease={}\ntree={}\nenvironment={}\nencoded_rustflags_digest={}\n",
        super::cargo::source_digest_for_probe(),
        workflow_definition_digest,
        process_start_identity,
        start_marker_digest,
        process_id,
        binding.consumer,
        binding.project,
        binding.component,
        binding.task_id,
        binding.workspace_id,
        binding.commit,
        binding.workflow,
        binding.run_id,
        binding.attempt_id,
        host_run_id,
        lease_id,
        observed_tree,
        environment_digest,
        hex_digest(encoded_rustflags.as_bytes()),
    );
    add_capture(
        output_references,
        capture_root,
        "preflight-provenance.txt",
        provenance.as_bytes(),
    )?;
    let flag_lines = encoded_rustflags
        .split('\u{1f}')
        .filter(|flag| !flag.is_empty())
        .collect::<Vec<_>>();
    add_capture(
        output_references,
        capture_root,
        "preflight-map-flags.txt",
        format!(
            "oxide-cargo-preflight-flags-v1\n{}\n",
            flag_lines.join("\n")
        )
        .as_bytes(),
    )?;

    for artifact in &summary.artifacts {
        report.push_str(&format!(
            "\nartifact\t{}\t{}\t{}\t{}\t{}\t{}",
            artifact.target_name,
            artifact.target_kinds.join("+"),
            artifact.fresh,
            artifact.manifest.display(),
            artifact
                .executable
                .as_deref()
                .unwrap_or(Path::new("missing"))
                .display(),
            artifact
                .filenames
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("|"),
        ));
    }
    for directory in &summary.build_script_dirs {
        report.push_str(&format!("\nbuild-script-out-dir\t{}", directory.display()));
    }
    for (key, value) in &summary.probe_values {
        report.push_str(&format!("\nprobe\t{key}\t{value}"));
    }

    let mut paths = Vec::new();
    for artifact in &summary.artifacts {
        if !artifact.manifest.starts_with(workspace_root) {
            continue;
        }
        let candidate = if artifact
            .target_kinds
            .iter()
            .any(|kind| kind == "proc-macro")
        {
            artifact
                .filenames
                .iter()
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "dylib")
                })
                .cloned()
        } else {
            artifact.executable.clone()
        };
        if let Some(path) = candidate {
            paths.push((path, artifact.target_name.clone()));
        }
    }
    paths.sort();
    paths.dedup_by(|left, right| left.0 == right.0);
    let mut scans_complete = !paths.is_empty() && paths.len() <= MAX_ARTIFACTS;
    if paths.len() > MAX_ARTIFACTS {
        report.push_str("\nscanner-limit\tartifact count exceeded");
    }
    for (index, (path, target_name)) in paths.iter().take(MAX_ARTIFACTS).enumerate() {
        let capture_root_kind = if path.starts_with(slot_root) {
            Some(slot_root)
        } else if path.starts_with(target_root) {
            Some(target_root)
        } else {
            None
        };
        let Some(root) = capture_root_kind else {
            scans_complete = false;
            report.push_str(&format!(
                "\nscanner-path\t{target_name}\tunsupported\t{}",
                path.display()
            ));
            continue;
        };
        let canonical = match fs::canonicalize(path) {
            Ok(canonical)
                if canonical == *path && canonical.is_file() && canonical.starts_with(root) =>
            {
                canonical
            }
            _ => {
                scans_complete = false;
                report.push_str(&format!(
                    "\nscanner-path\t{target_name}\tinvalid\t{}",
                    path.display()
                ));
                continue;
            }
        };
        let size = fs::metadata(&canonical)
            .map_err(|error| error.to_string())?
            .len();
        if size > 512 * 1024 * 1024 {
            scans_complete = false;
            report.push_str(&format!(
                "\nscanner-path\t{target_name}\toversize\t{}\t{size}",
                path.display()
            ));
            continue;
        }
        let reference = format!("preflight-raw-{index}.bin");
        copy_capture(
            &canonical,
            &capture_root.join(&reference),
            size,
            cancellation,
        )?;
        if cancellation.is_cancelled() {
            return Err("preflight binary capture was cancelled by the resource guard".to_owned());
        }
        output_references.push(reference.clone());
        report.push_str(&format!(
            "\nscan-artifact\t{target_name}\t{reference}\t{}\t{}",
            canonical.display(),
            digest_file_with_cancellation(&canonical, cancellation)?
        ));
        for (tool, suffix, arguments) in [
            (
                "dwarfdump",
                "sections",
                vec!["--show-section-sizes", canonical.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-info",
                vec!["--debug-info", canonical.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-line",
                vec!["--debug-line", canonical.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-abbrev",
                vec!["--debug-abbrev", canonical.to_str().unwrap_or("")],
            ),
            (
                "nm",
                "oso",
                vec!["-a", "-m", canonical.to_str().unwrap_or("")],
            ),
        ] {
            let capture_name = format!("preflight-{index}-{suffix}.txt");
            match run_bounded_raw(tool, &arguments, Some(temp_dir), Some(cancellation)) {
                Ok(bytes) => {
                    add_capture(output_references, capture_root, &capture_name, &bytes)?;
                    report.push_str(&format!(
                        "\nscanner\t{target_name}\t{suffix}\tpass\t{}",
                        hex_digest(&bytes)
                    ));
                }
                Err((error, partial)) => {
                    add_capture(output_references, capture_root, &capture_name, &partial)?;
                    add_capture(
                        output_references,
                        capture_root,
                        &format!("preflight-{index}-{suffix}-error.txt"),
                        error.as_bytes(),
                    )?;
                    scans_complete = false;
                    report.push_str(&format!(
                        "\nscanner\t{target_name}\t{suffix}\tinsufficient\t{error}"
                    ));
                    if cancellation.is_cancelled() {
                        return Err(
                            "preflight scanner stopped at a resource or session bound".to_owned()
                        );
                    }
                }
            }
        }
    }
    report.push_str(if scans_complete {
        "\nscan_complete=true\n"
    } else {
        "\nscan_complete=false\n"
    });
    let report = if report.len() <= MAX_SCANNER_BYTES {
        report
    } else {
        format!(
            "oxide-cargo-preflight-observations-v1\nrun={}\nattempt={}\nhost_run={}\nlease={}\ntree={}\nartifact_count={}\nrebuild_count={}\nscan_complete=false\nreport_limit=exceeded\n",
            binding.run_id,
            binding.attempt_id,
            host_run_id,
            lease_id,
            observed_tree,
            summary.artifacts.len(),
            summary.rebuilt_units,
        )
    };
    add_capture(
        output_references,
        capture_root,
        "preflight-observations.txt",
        report.as_bytes(),
    )
}

pub(super) fn produce_registered_observations(
    map: &ResolvedSourceMap,
    lease_id: &str,
    binding: &RunBinding,
    host_run_id: &str,
    workspace_root: &Path,
    slot_root: &Path,
    target_root: &Path,
    capture_root: &Path,
    stdout: &[u8],
    output_references: &mut Vec<String>,
    cancellation: &dyn CancellationProbe,
    temp_dir: &Path,
    workflow_definition_digest: &str,
    process_start_identity: &str,
    start_marker_digest: &str,
    process_id: u32,
    environment_digest: &str,
) -> Result<(), String> {
    if map.requested_tree_digest() != &tree_digest(workspace_root)? {
        return Err("requested tree changed while the owned command ran".to_owned());
    }
    let summary = inspect_cargo_output(stdout);
    let mut rows = vec![
        "oxide-cargo-source-map-observations-v1".to_owned(),
        format!(
            "identity\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            map.policy_digest(),
            map.map_digest(),
            map.build_identity_digest(),
            map.requested_tree_digest(),
            lease_id,
            binding.run_id,
            binding.attempt_id,
            host_run_id,
        ),
    ];

    let provenance = format!(
        "oxide-cargo-probe-provenance-v1\nproducer_source_digest={}\nworkflow_definition_digest={}\nprocess_start_identity={}\nstart_marker_digest={}\nprocess_id={}\nconsumer={}\nproject={}\ncomponent={}\ntask={}\nworkspace={}\ncommit={}\nworkflow={}\nrun={}\nattempt={}\nhost_run={}\nlease={}\nroot_id={}\nroot_path={}\ntree={}\npolicy={}\nmap={}\nbuild_identity={}\nenvironment={}\n",
        super::cargo::source_digest_for_probe(),
        workflow_definition_digest,
        process_start_identity,
        start_marker_digest,
        process_id,
        binding.consumer,
        binding.project,
        binding.component,
        binding.task_id,
        binding.workspace_id,
        binding.commit,
        binding.workflow,
        binding.run_id,
        binding.attempt_id,
        host_run_id,
        lease_id,
        map.requested_root_id(),
        map.requested_root().display(),
        map.requested_tree_digest(),
        map.policy_digest(),
        map.map_digest(),
        map.build_identity_digest(),
        environment_digest,
    );
    add_capture(
        output_references,
        capture_root,
        "producer-provenance.txt",
        provenance.as_bytes(),
    )?;

    let (mut origins, complete_origins) = compile_origins(
        &summary.artifacts,
        map,
        workspace_root,
        slot_root,
        target_root,
        cancellation,
    )?;
    if complete_origins {
        let mut seen = BTreeSet::new();
        origins.sort_by(|left, right| left.root_id.cmp(&right.root_id));
        for origin in origins {
            if seen.insert(origin.root_id.clone()) {
                rows.push(format!(
                    "origin\t{}\t{}",
                    origin.root_id, origin.tree_digest
                ));
            }
        }
    }
    preserve_origin_ledger(slot_root, capture_root, output_references)?;

    let scanner = scanner_identity()?;
    add_capture(
        output_references,
        capture_root,
        "scanner-provenance.txt",
        scanner.as_bytes(),
    )?;
    let (artifacts, scans_complete) = scan_artifacts(
        &summary,
        map,
        workspace_root,
        slot_root,
        target_root,
        capture_root,
        output_references,
        cancellation,
        temp_dir,
    )?;
    let values = &summary.probe_values;
    let output_dir = values.get("out-directory").map(PathBuf::from);
    let generated_path = values.get("generated-output-path").map(PathBuf::from);
    let macro_input_path = values.get("proc-macro-input-path").map(PathBuf::from);
    let generated_by_this_run = match (&output_dir, &generated_path, &macro_input_path) {
        (Some(output_dir), Some(generated), Some(macro_input)) => {
            summary
                .build_script_dirs
                .iter()
                .any(|observed| observed == output_dir)
                && generated == &output_dir.join("generated.rs")
                && macro_input == &output_dir.join("proc-macro-input.txt")
        }
        _ => false,
    };
    if !generated_by_this_run {
        let mut diagnostic = String::from(
            "generated path provenance could not be matched to a build-script-executed event\n",
        );
        for path in &summary.build_script_dirs {
            diagnostic.push_str(&format!("build_script_out_dir={}\n", path.display()));
        }
        add_capture(
            output_references,
            capture_root,
            "generation-provenance-insufficient.txt",
            diagnostic.as_bytes(),
        )?;
    }
    for (class, key) in [
        ("manifest-directory", "manifest-directory"),
        ("include-input", "include-input-path"),
        ("build-script-input", "build-input-path"),
        ("out-directory", "out-directory"),
        ("generated-output", "generated-output-path"),
        ("proc-macro-input", "proc-macro-input-path"),
        ("runtime-resource", "runtime-resource-path"),
    ] {
        if let Some(path) = values.get(key) {
            if matches!(
                class,
                "out-directory" | "generated-output" | "proc-macro-input"
            ) && !generated_by_this_run
            {
                continue;
            }
            let generation = if matches!(
                class,
                "out-directory" | "generated-output" | "proc-macro-input"
            ) {
                lease_id
            } else {
                "-"
            };
            rows.push(format!(
                "behavior\t{class}\t{path}\t{}\t{generation}",
                map.requested_tree_digest()
            ));
        }
    }
    for path in summary.build_script_dirs {
        if path.is_absolute() {
            let observed = path.to_string_lossy();
            if !values.values().any(|value| value == observed.as_ref()) {
                rows.push(format!(
                    "behavior\tout-directory\t{observed}\t{}\t{lease_id}",
                    map.requested_tree_digest()
                ));
            }
        }
    }

    if let Some(test_artifact) = artifacts.iter().find(|artifact| artifact.is_test) {
        if let Some(path) = values.get("file-macro") {
            rows.push(format!(
                "source\t{}\tfile-macro\t{path}",
                test_artifact.name
            ));
        }
        if let Some(path) = values.get("panic-location") {
            let file = path
                .rsplit_once(':')
                .map_or(path.as_str(), |(file, _)| file);
            rows.push(format!(
                "source\t{}\tpanic-location\t{file}",
                test_artifact.name
            ));
        }
    }

    for artifact in &artifacts {
        rows.push(format!(
            "artifact\t{}\t{}\t{}",
            artifact.name, artifact.root_kind, artifact.relative
        ));
        rows.push(format!(
            "scan\t{}\t{}\t{}",
            artifact.name, SOURCE_SCAN_VERSION, artifact.digest
        ));
        for section in &artifact.sections {
            rows.push(format!("section\t{}\t{section}", artifact.name));
        }
        for path in &artifact.source_paths {
            rows.push(format!(
                "source\t{}\tcompiler-debug\t{}",
                artifact.name, path
            ));
        }
        for path in &artifact.oso_paths {
            rows.push(format!("source\t{}\tapple-oso\t{}", artifact.name, path));
        }
    }
    if scans_complete {
        let oso_emitted = artifacts.iter().any(|artifact| artifact.oso_emitted);
        rows.push(format!(
            "apple-oso\t{}",
            if oso_emitted {
                "observed"
            } else {
                "not-emitted"
            }
        ));
    }

    // The schema is intentionally finite. An overfull observation is preserved
    // as a producer diagnostic and left Insufficient by the missing hook.
    let body = rows.join("\n") + "\n";
    let hook_path = target_root.join("source-map-observations-v1.txt");
    if body.len() > 64 * 1024 {
        add_capture(
            output_references,
            capture_root,
            "producer-limit.txt",
            b"source-map observation rows exceeded the registered 64 KiB hook limit\n",
        )?;
        return Ok(());
    }
    write_new(&hook_path, body.as_bytes())
}

struct ScannedArtifact {
    name: String,
    root_kind: &'static str,
    relative: String,
    digest: String,
    sections: Vec<&'static str>,
    source_paths: Vec<String>,
    oso_paths: Vec<String>,
    oso_emitted: bool,
    is_test: bool,
}

fn scan_artifacts(
    summary: &CargoOutputSummary,
    map: &ResolvedSourceMap,
    _workspace_root: &Path,
    slot_root: &Path,
    target_root: &Path,
    capture_root: &Path,
    output_references: &mut Vec<String>,
    cancellation: &dyn CancellationProbe,
    temp_dir: &Path,
) -> Result<(Vec<ScannedArtifact>, bool), String> {
    let mut paths = Vec::new();
    for artifact in &summary.artifacts {
        if map
            .authorized_roots()
            .iter()
            .any(|root| artifact.manifest.starts_with(&root.path))
        {
            let candidate = if artifact
                .target_kinds
                .iter()
                .any(|kind| kind == "proc-macro")
            {
                artifact
                    .filenames
                    .iter()
                    .find(|path| path.extension().is_some_and(|ext| ext == "dylib"))
                    .cloned()
            } else {
                artifact.executable.clone()
            };
            if let Some(path) = candidate {
                paths.push((
                    path,
                    artifact.target_kinds.iter().any(|kind| kind == "test"),
                ));
            }
        }
    }
    paths.sort();
    paths.dedup_by(|left, right| left.0 == right.0);
    if paths.len() > MAX_ARTIFACTS {
        return Ok((Vec::new(), false));
    }

    let mut scanned = Vec::new();
    let mut scanner_failed = false;
    for (index, (path, is_test)) in paths.iter().enumerate() {
        let (root_kind, root) = if path.starts_with(slot_root) {
            ("slot", slot_root)
        } else if path.starts_with(target_root) {
            ("target", target_root)
        } else {
            scanner_failed = true;
            continue;
        };
        let canonical = match fs::canonicalize(path) {
            Ok(path) => path,
            Err(_) => {
                scanner_failed = true;
                continue;
            }
        };
        if canonical != *path || !canonical.is_file() || !canonical.starts_with(root) {
            scanner_failed = true;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_str()
            .ok_or_else(|| "artifact path is not UTF-8".to_owned())?
            .to_owned();
        let bytes = fs::metadata(path).map_err(|error| error.to_string())?.len();
        if bytes > 512 * 1024 * 1024 {
            scanner_failed = true;
            continue;
        }
        let digest = digest_file_with_cancellation(path, cancellation)?;
        let name = format!("probe-{index}");
        let copy_name = format!("raw-{name}.bin");
        copy_capture(path, &capture_root.join(&copy_name), bytes, cancellation)?;
        if cancellation.is_cancelled() {
            return Err("binary scanner capture was cancelled by the resource guard".to_owned());
        }
        output_references.push(copy_name);

        let mut outputs = BTreeMap::new();
        for (tool, suffix, arguments) in [
            (
                "dwarfdump",
                "sections",
                vec!["--show-section-sizes", path.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-info",
                vec!["--debug-info", path.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-line",
                vec!["--debug-line", path.to_str().unwrap_or("")],
            ),
            (
                "dwarfdump",
                "debug-abbrev",
                vec!["--debug-abbrev", path.to_str().unwrap_or("")],
            ),
            ("nm", "oso", vec!["-a", "-m", path.to_str().unwrap_or("")]),
        ] {
            let reference = format!("{name}-{suffix}.txt");
            match run_bounded_raw(tool, &arguments, Some(temp_dir), Some(cancellation)) {
                Ok(output) => {
                    add_capture(output_references, capture_root, &reference, &output)?;
                    outputs.insert(suffix, output);
                }
                Err((reason, partial)) => {
                    add_capture(output_references, capture_root, &reference, &partial)?;
                    add_capture(
                        output_references,
                        capture_root,
                        &format!("{name}-{suffix}-error.txt"),
                        reason.as_bytes(),
                    )?;
                    scanner_failed = true;
                    break;
                }
            }
        }
        if scanner_failed {
            break;
        }
        let Some(section_sizes) = outputs.get("sections") else {
            scanner_failed = true;
            break;
        };
        let Some(info) = outputs.get("debug-info") else {
            scanner_failed = true;
            break;
        };
        let Some(lines) = outputs.get("debug-line") else {
            scanner_failed = true;
            break;
        };
        let Some(_abbrev) = outputs.get("debug-abbrev") else {
            scanner_failed = true;
            break;
        };
        let Some(oso) = outputs.get("oso") else {
            scanner_failed = true;
            break;
        };

        let sections = [".debug_info", ".debug_line", ".debug_abbrev"]
            .into_iter()
            .filter(|section| {
                let short = section.trim_start_matches('.');
                let apple = format!("__{short}");
                let output = String::from_utf8_lossy(section_sizes);
                output.contains(section) || output.contains(&apple) || output.contains(short)
            })
            .collect::<Vec<_>>();
        let mut source_paths = debug_source_paths(info, lines, map);
        source_paths.sort();
        source_paths.dedup();
        let oso_emitted = String::from_utf8_lossy(oso)
            .lines()
            .any(|line| line.contains("OSO "));
        let oso_paths = oso_paths(oso)
            .into_iter()
            .filter(|path| {
                let observed = Path::new(path);
                map.logical_prefix()
                    .is_some_and(|prefix| observed.starts_with(prefix))
                    || map
                        .authorized_roots()
                        .iter()
                        .any(|root| observed.starts_with(&root.path))
            })
            .collect();
        scanned.push(ScannedArtifact {
            name,
            root_kind,
            relative,
            digest,
            sections,
            source_paths,
            oso_paths,
            oso_emitted,
            is_test: *is_test,
        });
    }
    if scanner_failed {
        add_capture(
            output_references,
            capture_root,
            "scanner-incomplete.txt",
            b"one or more bounded binary scans could not complete; no partial scan is qualifying evidence\n",
        )?;
        Ok((Vec::new(), false))
    } else {
        let complete = !scanned.is_empty();
        Ok((scanned, complete))
    }
}

fn compile_origins(
    artifacts: &[CargoArtifact],
    map: &ResolvedSourceMap,
    workspace_root: &Path,
    slot_root: &Path,
    target_root: &Path,
    cancellation: &dyn CancellationProbe,
) -> Result<(Vec<CompileOrigin>, bool), String> {
    let ledger_path = slot_root.join(ORIGIN_FILE);
    let mut known = read_origin_ledger(&ledger_path, map)?;
    let requested_tree = tree_digest(workspace_root)?;
    let mut origins = Vec::new();
    let mut complete = !artifacts.is_empty();
    for artifact in artifacts {
        if !map
            .authorized_roots()
            .iter()
            .any(|root| artifact.manifest.starts_with(&root.path))
        {
            continue;
        }
        let Some(root) = map
            .authorized_roots()
            .iter()
            .find(|root| artifact.manifest.starts_with(root.path.as_path()))
        else {
            complete = false;
            continue;
        };
        let files: Vec<_> = artifact
            .filenames
            .iter()
            .chain(artifact.executable.iter())
            .filter(|path| path.starts_with(slot_root) || path.starts_with(target_root))
            .cloned()
            .collect();
        let Some(digest) = digest_artifact_files(&files, cancellation)? else {
            complete = false;
            continue;
        };
        if artifact.fresh {
            match known.get(&artifact.unit_key) {
                Some(origin) if origin.artifact_digest == digest => {
                    let source_root = map
                        .authorized_roots()
                        .iter()
                        .find(|candidate| candidate.id == origin.root_id);
                    if let Some(source_root) = source_root {
                        if tree_digest(&source_root.path)? == origin.tree_digest {
                            origins.push(origin.clone());
                        } else {
                            complete = false;
                        }
                    } else {
                        complete = false;
                    }
                }
                _ => complete = false,
            }
        } else {
            let origin = CompileOrigin {
                root_id: root.id.clone(),
                tree_digest: requested_tree.clone(),
                artifact_digest: digest,
            };
            known.insert(artifact.unit_key.clone(), origin.clone());
            origins.push(origin);
        }
    }
    if !origins.is_empty() {
        write_origin_ledger(&ledger_path, map, &known)?;
    }
    Ok((origins, complete))
}

fn read_origin_ledger(
    path: &Path,
    map: &ResolvedSourceMap,
) -> Result<BTreeMap<String, CompileOrigin>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            if !fs::canonicalize(path).is_ok_and(|canonical| canonical == path) {
                return Err("compile-origin ledger path is not canonical".to_owned());
            }
        }
        Ok(_) => return Err("compile-origin ledger is not a regular file".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.to_string()),
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return Err(error.to_string()),
    };
    let mut lines = text.lines();
    if lines.next() != Some("oxide-cargo-origin-ledger-v1")
        || lines.next()
            != Some(format!("map\t{}\t{}", map.map_digest(), map.build_identity_digest()).as_str())
    {
        return Err(
            "compile-origin ledger identity is unknown; refusing reuse evidence".to_owned(),
        );
    }
    let mut known = BTreeMap::new();
    for line in lines {
        let fields: Vec<_> = line.split('\t').collect();
        if let ["unit", key, root_id, tree_digest, artifact_digest] = fields.as_slice() {
            let root_allowed = map
                .authorized_roots()
                .iter()
                .any(|root| root.id == *root_id);
            if key.is_empty()
                || key.contains('/')
                || !root_allowed
                || !valid_sha256(tree_digest)
                || !valid_sha256(artifact_digest)
                || known.contains_key(*key)
            {
                return Err(
                    "compile-origin ledger contains an invalid or duplicate unit row".to_owned(),
                );
            }
            known.insert(
                (*key).to_owned(),
                CompileOrigin {
                    root_id: (*root_id).to_owned(),
                    tree_digest: (*tree_digest).to_owned(),
                    artifact_digest: (*artifact_digest).to_owned(),
                },
            );
        } else {
            return Err("compile-origin ledger contains an unrecognized row".to_owned());
        }
    }
    Ok(known)
}

fn write_origin_ledger(
    path: &Path,
    map: &ResolvedSourceMap,
    origins: &BTreeMap<String, CompileOrigin>,
) -> Result<(), String> {
    let mut text = format!(
        "oxide-cargo-origin-ledger-v1\nmap\t{}\t{}\n",
        map.map_digest(),
        map.build_identity_digest()
    );
    for (key, origin) in origins {
        text.push_str(&format!(
            "unit\t{key}\t{}\t{}\t{}\n",
            origin.root_id, origin.tree_digest, origin.artifact_digest
        ));
    }
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    File::open(
        path.parent()
            .ok_or_else(|| "origin ledger has no parent".to_owned())?,
    )
    .and_then(|directory| directory.sync_all())
    .map_err(|error| error.to_string())
}

fn preserve_origin_ledger(
    slot_root: &Path,
    capture_root: &Path,
    output_references: &mut Vec<String>,
) -> Result<(), String> {
    let path = slot_root.join(ORIGIN_FILE);
    if path.is_file() {
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        add_capture(
            output_references,
            capture_root,
            "compile-origins.txt",
            &bytes,
        )?;
    }
    Ok(())
}

fn parse_artifact(line: &str) -> Option<CargoArtifact> {
    let manifest = PathBuf::from(json_string(line, "manifest_path")?);
    let target = json_object(line, "target")?;
    let target_name = json_string(target, "name")?;
    let mut target_kinds = json_string_array(target, "kind")?;
    target_kinds.sort();
    let fresh = json_bool(line, "fresh")?;
    let filenames = json_string_array(line, "filenames")?
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let executable = json_string(line, "executable").map(PathBuf::from);
    let relative_manifest = manifest.file_name()?.to_string_lossy();
    let package_dir = manifest.parent()?;
    let package_key = package_dir.file_name()?.to_string_lossy();
    let unit_key = format!(
        "{package_key}-{relative_manifest}-{target_name}-{}",
        target_kinds.join("+")
    );
    Some(CargoArtifact {
        unit_key,
        manifest,
        target_name,
        target_kinds,
        fresh,
        executable,
        filenames,
    })
}

fn json_string(line: &str, key: &str) -> Option<String> {
    parse_json_string(json_field(line, key)?).map(|(value, _)| value)
}

fn parse_json_string(value: &str) -> Option<(String, usize)> {
    let bytes = value.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut output = String::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((output, index + 1)),
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'"' => output.push('"'),
                    b'\\' => output.push('\\'),
                    b'/' => output.push('/'),
                    b'b' => output.push('\u{8}'),
                    b'f' => output.push('\u{c}'),
                    b'n' => output.push('\n'),
                    b'r' => output.push('\r'),
                    b't' => output.push('\t'),
                    _ => return None,
                }
            }
            byte if byte.is_ascii() => output.push(byte as char),
            _ => return None,
        }
        index += 1;
    }
    None
}

fn json_bool(line: &str, key: &str) -> Option<bool> {
    match json_field(line, key)?
        .split(|character: char| character == ',' || character == '}')
        .next()?
        .trim()
    {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn json_string_array(line: &str, key: &str) -> Option<Vec<String>> {
    let value = json_field(line, key)?;
    let rest = value.strip_prefix('[')?;
    let mut values = Vec::new();
    let mut remaining = rest.trim_start();
    loop {
        if remaining.starts_with(']') {
            return Some(values);
        }
        let (decoded, quoted_end) = parse_json_string(remaining)?;
        values.push(decoded);
        remaining = remaining[quoted_end..].trim_start();
        if let Some(rest) = remaining.strip_prefix(',') {
            remaining = rest.trim_start();
        } else if !remaining.starts_with(']') {
            return None;
        }
    }
}

fn json_object<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let value = json_field(line, key)?;
    if !value.starts_with('{') {
        return None;
    }
    let end = json_container_end(value, b'{', b'}')?;
    Some(&value[..end])
}

fn hex_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn json_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\"");
    let start = line.find(&needle)? + needle.len();
    let after = line[start..].trim_start();
    let value = after.strip_prefix(':')?.trim_start();
    Some(value)
}

fn json_quoted_end(value: &str) -> Option<usize> {
    if !value.starts_with('"') {
        return None;
    }
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return Some(index + 1);
        }
    }
    None
}

fn json_container_end(value: &str, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index + 1);
            }
        }
    }
    None
}

fn debug_source_paths(info: &[u8], lines: &[u8], map: &ResolvedSourceMap) -> Vec<String> {
    let mut paths = BTreeSet::new();
    let mut comp_dir = None;
    let mut names = Vec::new();
    for line in String::from_utf8_lossy(info).lines() {
        if line.contains("DW_TAG_compile_unit") {
            flush_debug_names(&mut names, comp_dir.as_deref(), map, &mut paths);
            comp_dir = None;
        } else if line.contains("DW_AT_comp_dir") {
            comp_dir = quoted_value(line);
        } else if line.contains("DW_AT_name") {
            if let Some(name) = quoted_value(line).filter(|name| name.ends_with(".rs")) {
                names.push(name);
            }
        }
    }
    flush_debug_names(&mut names, comp_dir.as_deref(), map, &mut paths);
    for line in String::from_utf8_lossy(lines).lines() {
        if let Some(value) = quoted_value(line).filter(|value| value.ends_with(".rs")) {
            add_source_path(PathBuf::from(value), map, &mut paths);
        }
    }
    paths.into_iter().collect()
}

fn flush_debug_names(
    names: &mut Vec<String>,
    comp_dir: Option<&str>,
    map: &ResolvedSourceMap,
    paths: &mut BTreeSet<String>,
) {
    for name in names.drain(..) {
        let path = PathBuf::from(&name);
        let path = if path.is_absolute() {
            path
        } else if let Some(comp_dir) = comp_dir {
            PathBuf::from(comp_dir).join(path)
        } else {
            continue;
        };
        add_source_path(path, map, paths);
    }
}

fn add_source_path(path: PathBuf, map: &ResolvedSourceMap, paths: &mut BTreeSet<String>) {
    let candidate = path.to_string_lossy().into_owned();
    if map
        .logical_prefix()
        .is_some_and(|prefix| Path::new(&candidate).starts_with(prefix))
        || map
            .authorized_roots()
            .iter()
            .any(|root| Path::new(&candidate).starts_with(&root.path))
    {
        paths.insert(candidate);
    }
}

fn quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let value = &line[start..];
    let end = json_quoted_end(value)?;
    json_string(&format!("{{\"v\":{}}}", &value[..end]), "v")
}

fn oso_paths(output: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| line.split_once("OSO ").map(|(_, path)| path.trim()))
        .filter(|path| !path.is_empty())
        .filter(|path| Path::new(path).is_absolute())
        .map(str::to_owned)
        .collect()
}

pub(super) fn scanner_identity() -> Result<String, String> {
    let mut lines = vec!["oxide-probe-scanners-v1".to_owned()];
    for tool in ["dwarfdump", "nm"] {
        let path =
            find_program(tool).ok_or_else(|| format!("required scanner is unavailable: {tool}"))?;
        let output = run_bounded(tool, &["--version"])?;
        let version = String::from_utf8_lossy(&output).trim().replace('\n', " ");
        lines.push(format!(
            "{tool}\t{}\t{}\t{}",
            path.display(),
            version,
            digest_file(&path)?
        ));
    }
    Ok(lines.join("\n") + "\n")
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

fn run_bounded(program: &str, arguments: &[&str]) -> Result<Vec<u8>, String> {
    run_bounded_raw(program, arguments, None, None).map_err(|(reason, _)| reason)
}

fn run_bounded_raw(
    program: &str,
    arguments: &[&str],
    temp_dir: Option<&Path>,
    cancellation: Option<&dyn CancellationProbe>,
) -> Result<Vec<u8>, (String, Vec<u8>)> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(temp_dir) = temp_dir {
        command
            .env("TMPDIR", temp_dir)
            .env("TMP", temp_dir)
            .env("TEMP", temp_dir);
    }
    let mut child = command
        .spawn()
        .map_err(|error| (format!("could not run {program}: {error}"), Vec::new()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| (format!("{program} has no stdout pipe"), Vec::new()))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout
            .take((MAX_SCANNER_BYTES + 1) as u64)
            .read_to_end(&mut output);
        let _ = sender.send((result, output));
    });
    let started = Instant::now();
    let mut read_result = None;
    let mut status = None;
    loop {
        if read_result.is_none() {
            match receiver.try_recv() {
                Ok(result) => read_result = Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err((
                        format!("{program} scanner output reader stopped unexpectedly"),
                        Vec::new(),
                    ));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => status = Some(exit),
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err((error.to_string(), Vec::new()));
                }
            }
        }
        if read_result.is_some() && status.is_some() {
            break;
        }
        if cancellation.is_some_and(|probe| probe.is_cancelled()) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let partial = read_result
                .take()
                .or_else(|| receiver.try_recv().ok())
                .map_or_else(Vec::new, |(_, mut bytes)| {
                    bytes.truncate(MAX_SCANNER_BYTES);
                    bytes
                });
            return Err((
                format!("{program} stopped at a resource or session bound"),
                partial,
            ));
        }
        if started.elapsed() >= SCANNER_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let partial = read_result
                .take()
                .or_else(|| receiver.try_recv().ok())
                .map_or_else(Vec::new, |(_, mut bytes)| {
                    bytes.truncate(MAX_SCANNER_BYTES);
                    bytes
                });
            return Err((
                format!("{program} exceeded the {SCANNER_TIMEOUT:?} scanner timeout"),
                partial,
            ));
        }
        thread::sleep(SCANNER_POLL);
    }
    let (read_result, mut output) = read_result.expect("bounded scanner read result");
    let _ = reader.join();
    if read_result.is_err() || output.len() > MAX_SCANNER_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        output.truncate(MAX_SCANNER_BYTES);
        return Err((
            format!("{program} output exceeded the bounded scanner limit or could not be read"),
            output,
        ));
    }
    let status = status.expect("scanner process exit status");
    if !status.success() {
        return Err((format!("{program} exited with {status}"), output));
    }
    Ok(output)
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn add_capture(
    references: &mut Vec<String>,
    capture_root: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if references.iter().any(|reference| reference == name) {
        return Err(format!("duplicate private capture name: {name}"));
    }
    write_new(&capture_root.join(name), bytes)?;
    references.push(name.to_owned());
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())
}

fn copy_capture(
    source: &Path,
    target: &Path,
    length: u64,
    cancellation: &dyn CancellationProbe,
) -> Result<(), String> {
    if length > 512 * 1024 * 1024 {
        return Err("binary exceeds the evidence capture limit".to_owned());
    }
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|error| error.to_string())?;
    let mut copied = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancellation.is_cancelled() {
            output.sync_all().map_err(|error| error.to_string())?;
            return Err("binary capture was cancelled by the resource guard".to_owned());
        }
        let read = input.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|error| error.to_string())?;
        copied = copied.saturating_add(read as u64);
        if copied > length {
            return Err("binary changed while its evidence capture was copied".to_owned());
        }
    }
    if copied != length {
        return Err("binary changed size while its evidence capture was copied".to_owned());
    }
    output.sync_all().map_err(|error| error.to_string())
}

fn digest_artifact_files(
    paths: &[PathBuf],
    cancellation: &dyn CancellationProbe,
) -> Result<Option<String>, String> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    let mut digest = Sha256::new();
    let mut total = 0u64;
    for path in paths {
        if cancellation.is_cancelled() {
            return Err("artifact hashing was cancelled by the resource guard".to_owned());
        }
        let canonical = fs::canonicalize(&path).map_err(|error| error.to_string())?;
        if canonical != path || !canonical.is_file() {
            return Ok(None);
        }
        let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
        total = total.saturating_add(metadata.len());
        if total > 8 * 1024 * 1024 * 1024 {
            return Ok(None);
        }
        digest.update((canonical.file_name().unwrap_or_default().len() as u64).to_le_bytes());
        digest.update(canonical.file_name().unwrap_or_default().as_encoded_bytes());
        let mut file = File::open(canonical).map_err(|error| error.to_string())?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if cancellation.is_cancelled() {
                return Err("artifact hashing was cancelled by the resource guard".to_owned());
            }
            let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            digest.update(&buffer[..read]);
        }
    }
    Ok(Some(format!("sha256:{:x}", digest.finalize())))
}

fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn digest_file_with_cancellation(
    path: &Path,
    cancellation: &dyn CancellationProbe,
) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancellation.is_cancelled() {
            return Err("binary hashing was cancelled by the resource guard".to_owned());
        }
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn tree_digest(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut digest = Sha256::new();
    for relative in files {
        let path = root.join(&relative);
        let name = relative.to_string_lossy();
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "source fixture contains a symlink: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            files.push(
                path.strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf(),
            );
        } else {
            return Err(format!(
                "source fixture contains a non-file: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
struct NeverCancelled;

#[cfg(test)]
impl super::CancellationProbe for NeverCancelled {
    fn is_cancelled(&self) -> bool {
        false
    }
}

#[cfg(test)]
fn producer_scratch(name: &str) -> PathBuf {
    let root = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = root.join(format!(
        "producer-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&path).expect("producer scratch");
    path
}

#[cfg(test)]
fn sample_digest(label: &str) -> String {
    hex_digest(label.as_bytes())
}

#[cfg(test)]
fn sample_map(volume: &Path, workspace: &Path, tree: &str) -> super::source_map::ResolvedSourceMap {
    use super::source_map::{
        AuthorizedSourceRoot, KeyMissControl, MissControlEvidence, SourceMapPolicy,
        SourceMissControl, SourcePathMode, ACCEPTED_LOGICAL_POLICY, SOURCE_MAP_VERSION,
    };
    let policy = SourceMapPolicy {
        version: SOURCE_MAP_VERSION.to_owned(),
        mode: SourcePathMode::ExperimentalLogical {
            accepted_policy: ACCEPTED_LOGICAL_POLICY.to_owned(),
            prefix: PathBuf::from("/oxide-source-map-probe"),
        },
        roots: vec![AuthorizedSourceRoot {
            id: "a".to_owned(),
            path: workspace.to_path_buf(),
        }],
        miss_controls: Some(MissControlEvidence {
            source: Some(SourceMissControl {
                before_tree_digest: sample_digest("tree-before"),
                after_tree_digest: sample_digest("tree-after"),
                partition_digest: sample_digest("partition"),
                rebuilt_units: 1,
            }),
            configuration: Some(KeyMissControl {
                before_input_digest: sample_digest("config-before"),
                after_input_digest: sample_digest("config-after"),
                before_partition_digest: sample_digest("config-part-before"),
                after_partition_digest: sample_digest("config-part-after"),
            }),
            toolchain: Some(KeyMissControl {
                before_input_digest: sample_digest("tool-before"),
                after_input_digest: sample_digest("tool-after"),
                before_partition_digest: sample_digest("tool-part-before"),
                after_partition_digest: sample_digest("tool-part-after"),
            }),
        }),
    };
    super::source_map::resolve(
        &policy,
        volume,
        workspace,
        tree,
        Some(&[] as &[String]),
        true,
    )
    .expect("sample source map")
}

#[test]
fn inspect_cargo_output_parses_artifacts_and_probe_rows() {
    let stdout = concat!(
        r#"{"reason":"compiler-artifact","manifest_path":"/private/var/tmp/a/probe/Cargo.toml","target":{"kind":["test"],"name":"probe"},"filenames":["/private/var/tmp/a/slot/probe-test"],"executable":"/private/var/tmp/a/slot/probe-test","fresh":false}"#,
        "\n",
        r#"{"reason":"compiler-artifact","manifest_path":"/private/var/tmp/a/probe/Cargo.toml","target":{"kind":["lib"],"name":"oxide_source_probe"},"filenames":["/private/var/tmp/a/slot/libprobe.rlib"],"executable":null,"fresh":true}"#,
        "\n",
        r#"{"reason":"build-script-executed","out_dir":"/private/var/tmp/a/slot/build/probe-out"}"#,
        "\n",
        "test probe::emits_observed_paths_and_values ... ok\n",
        "OXIDE_PROBE\tfile-macro\t/oxide-source-map-probe/probe/tests/probe.rs\n",
        "OXIDE_PROBE\tinclude-value\tbaseline-marker\n",
    );
    let summary = inspect_cargo_output(stdout.as_bytes());
    assert_eq!(summary.rebuilt_units, 1);
    assert_eq!(summary.rebuilt_targets, vec!["probe:test".to_owned()]);
    assert_eq!(summary.artifacts.len(), 2);
    assert_eq!(summary.artifacts[0].target_name, "probe");
    assert!(!summary.artifacts[0].fresh);
    assert_eq!(
        summary.artifacts[0].executable.as_deref(),
        Some(Path::new("/private/var/tmp/a/slot/probe-test"))
    );
    assert_eq!(
        summary.build_script_dirs,
        vec![PathBuf::from("/private/var/tmp/a/slot/build/probe-out")]
    );
    assert_eq!(
        summary.probe_values.get("file-macro").map(String::as_str),
        Some("/oxide-source-map-probe/probe/tests/probe.rs")
    );
    assert_eq!(
        summary
            .probe_values
            .get("include-value")
            .map(String::as_str),
        Some("baseline-marker")
    );
}

#[test]
fn inspect_cargo_output_rejects_malformed_and_duplicate_probe_rows() {
    let oversized = format!(
        "{{\"reason\":\"compiler-artifact\"}}{}",
        "x".repeat(MAX_CARGO_LINE_BYTES)
    );
    let stdout = format!(
        "{{not-json\n\
         {{\"reason\":\"compiler-artifact\",\"manifest_path\":\"/missing\"}}\n\
         {oversized}\n\
         OXIDE_PROBE\tfile-macro\t/oxide-source-map-probe/a.rs\n\
         OXIDE_PROBE\tfile-macro\t/oxide-source-map-probe/b.rs\n"
    );
    let summary = inspect_cargo_output(stdout.as_bytes());
    assert!(summary.artifacts.is_empty());
    assert!(summary.probe_values.is_empty());
    assert_eq!(summary.rebuilt_units, 0);
}

#[test]
fn debug_and_oso_scanners_keep_mapped_paths_and_drop_foreign_ones() {
    let scratch = producer_scratch("scanner");
    let volume = scratch.join("volume");
    let workspace = volume.join("a");
    fs::create_dir_all(workspace.join("src")).expect("workspace");
    fs::write(workspace.join("src/lib.rs"), b"pub fn probe() {}\n").expect("source");
    let tree = tree_digest(&workspace).expect("tree");
    let map = sample_map(&volume, &workspace, &tree);
    let info = br#"
0x0000000b: DW_TAG_compile_unit
  DW_AT_comp_dir        ("/oxide-source-map-probe/a")
  DW_AT_name            ("src/lib.rs")
0x0000004a: DW_TAG_compile_unit
  DW_AT_name            ("/oxide-source-map-probe/a/src/lib.rs")
"#;
    let lines = br#"
The file name is "/oxide-source-map-probe/a/src/lib.rs"
The file name is "/private/var/tmp/unrelated/src/lib.rs"
"#;
    let paths = debug_source_paths(info, lines, &map);
    assert_eq!(
        paths,
        vec!["/oxide-source-map-probe/a/src/lib.rs".to_owned()]
    );
    let oso = br#"
0000000000000000 - 00 0000    OSO /oxide-source-map-probe/a/target/debug/deps/probe.o
0000000000000000 - 00 0000    OSO /private/var/tmp/other/probe.o
0000000000000000 - 01 0001    FUN _probe
"#;
    let observed = oso_paths(oso)
        .into_iter()
        .filter(|path| {
            Path::new(path).starts_with("/oxide-source-map-probe")
                || map
                    .authorized_roots()
                    .iter()
                    .any(|root| Path::new(path).starts_with(&root.path))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec!["/oxide-source-map-probe/a/target/debug/deps/probe.o".to_owned()]
    );
    let _ = fs::remove_dir_all(&scratch);
}

#[test]
fn origin_ledger_records_rebuilt_units_and_preserves_fresh_matches() {
    let scratch = producer_scratch("origins");
    let volume = scratch.join("volume");
    let workspace = volume.join("a");
    let slot = scratch.join("slot");
    let target = scratch.join("target");
    let capture = scratch.join("capture");
    fs::create_dir_all(workspace.join("src")).expect("workspace");
    fs::create_dir_all(&slot).expect("slot");
    fs::create_dir_all(&target).expect("target");
    fs::create_dir_all(&capture).expect("capture");
    fs::write(workspace.join("src/lib.rs"), b"pub fn probe() {}\n").expect("source");
    let artifact = slot.join("probe-test");
    fs::write(&artifact, b"fresh-artifact-bytes\n").expect("artifact");
    let tree = tree_digest(&workspace).expect("tree");
    let map = sample_map(&volume, &workspace, &tree);
    let rebuilt = CargoArtifact {
        unit_key: "a-Cargo.toml-probe-test".to_owned(),
        manifest: workspace.join("Cargo.toml"),
        target_name: "probe".to_owned(),
        target_kinds: vec!["test".to_owned()],
        fresh: false,
        executable: Some(artifact.clone()),
        filenames: vec![artifact.clone()],
    };
    let (origins, complete) = compile_origins(
        &[rebuilt.clone()],
        &map,
        &workspace,
        &slot,
        &target,
        &NeverCancelled,
    )
    .expect("rebuilt origin");
    assert!(complete);
    assert_eq!(origins.len(), 1);
    assert_eq!(origins[0].root_id, "a");
    assert_eq!(origins[0].tree_digest, tree);
    let mut references = Vec::new();
    preserve_origin_ledger(&slot, &capture, &mut references).expect("preserve");
    assert_eq!(references, vec!["compile-origins.txt".to_owned()]);
    let preserved = fs::read_to_string(capture.join("compile-origins.txt")).expect("capture");
    assert!(preserved.starts_with("oxide-cargo-origin-ledger-v1\n"));
    assert!(preserved.contains("unit\ta-Cargo.toml-probe-test\ta\t"));

    let mut fresh = rebuilt;
    fresh.fresh = true;
    let (fresh_origins, fresh_complete) =
        compile_origins(&[fresh], &map, &workspace, &slot, &target, &NeverCancelled)
            .expect("fresh origin");
    assert!(fresh_complete);
    assert_eq!(fresh_origins[0].tree_digest, tree);

    fs::write(slot.join(ORIGIN_FILE), b"not-a-ledger\n").expect("malformed ledger");
    let error = match compile_origins(
        &[CargoArtifact {
            unit_key: "a-Cargo.toml-probe-test".to_owned(),
            manifest: workspace.join("Cargo.toml"),
            target_name: "probe".to_owned(),
            target_kinds: vec!["test".to_owned()],
            fresh: true,
            executable: Some(artifact.clone()),
            filenames: vec![artifact],
        }],
        &map,
        &workspace,
        &slot,
        &target,
        &NeverCancelled,
    ) {
        Err(error) => error,
        Ok(_) => panic!("malformed ledger"),
    };
    assert!(error.contains("unknown"));
    let _ = fs::remove_dir_all(&scratch);
}
