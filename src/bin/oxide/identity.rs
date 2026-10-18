//! Workspace, project identity and the slot compatibility key. No
//! configuration: identity is inferred from Git, then the canonical path.

use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Codegen backend in the compatibility key. Only the default exists here.
const BACKEND: &str = "default";

/// Everything that selects a warm slot for one command.
pub struct SlotKey {
    /// Canonical workspace root of this checkout.
    pub checkout: PathBuf,
    /// Digest of the inferred repository identity.
    pub identity: String,
    /// Digest of identity + Cargo profile + `rustc -vV` + target + backend.
    pub compatibility: String,
}

impl SlotKey {
    pub fn resolve(
        cargo: &str,
        manifest_path: Option<&Path>,
        profile: &str,
        target: Option<&str>,
    ) -> Result<Self, String> {
        let checkout = locate_workspace(cargo, manifest_path)?;
        let identity = digest(&project_identity(&checkout));
        let rustc = rustc_version(&checkout)?;
        let target = match target {
            Some(target) => target.to_owned(),
            None => std::env::var("CARGO_BUILD_TARGET")
                .ok()
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    rustc
                        .lines()
                        .find_map(|line| line.strip_prefix("host: "))
                        .map(str::to_owned)
                })
                .ok_or("rustc -vV did not report a host target")?,
        };
        let compatibility = digest(&format!(
            "{identity}\n{profile}\n{}\n{target}\n{BACKEND}",
            digest(&rustc)
        ));
        Ok(Self {
            checkout,
            identity,
            compatibility,
        })
    }
}

/// Canonical workspace root from `cargo locate-project --workspace`.
pub fn locate_workspace(cargo: &str, manifest_path: Option<&Path>) -> Result<PathBuf, String> {
    let mut command = Command::new(cargo);
    command.args(["locate-project", "--workspace", "--message-format", "plain"]);
    if let Some(manifest) = manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    let output = command
        .output()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not resolve the Cargo workspace: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let manifest = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let root = manifest
        .parent()
        .ok_or("Cargo reported a manifest without a parent directory")?;
    fs::canonicalize(root).map_err(|error| format!("cannot resolve the workspace root: {error}"))
}

/// Which binary `cargo run` would pick without `--bin` or `--example`.
#[derive(Debug, Eq, PartialEq)]
pub enum RunPick {
    /// The single `default-run` among the selected packages, or their only
    /// binary.
    Bin(String),
    /// Several binaries and no single `default-run`: Cargo refuses before
    /// building.
    Ambiguous(Vec<String>),
    /// No selected workspace package or binary resolved; the build reports
    /// what it produced.
    Unresolved,
}

/// Resolve `cargo run`'s target as Cargo does: the selected packages are the
/// `-p` package, else the workspace default members for the current
/// directory (the current package, a virtual root's `default-members`, or
/// every member). One `default-run` among them wins; otherwise exactly one
/// binary must exist.
pub fn run_pick(
    cargo: &str,
    manifest_path: Option<&Path>,
    package: Option<&str>,
) -> Result<RunPick, String> {
    let mut command = Command::new(cargo);
    command.args(["metadata", "--no-deps", "--format-version", "1"]);
    if let Some(manifest) = manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    let output = command
        .output()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not read package metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cargo metadata is not JSON: {error}"))?;
    Ok(select_run(&metadata, package))
}

fn select_run(metadata: &serde_json::Value, package: Option<&str>) -> RunPick {
    let Some(packages) = metadata["packages"].as_array() else {
        return RunPick::Unresolved;
    };
    let defaults = metadata["workspace_default_members"].as_array();
    let selected: Vec<&serde_json::Value> = packages
        .iter()
        .filter(|candidate| match package {
            Some(name) => candidate["name"].as_str() == Some(name),
            None => defaults.is_some_and(|ids| ids.contains(&candidate["id"])),
        })
        .collect();
    let default_runs: Vec<&str> = selected
        .iter()
        .filter_map(|candidate| candidate["default_run"].as_str())
        .collect();
    if let [only] = default_runs.as_slice() {
        return RunPick::Bin((*only).to_owned());
    }
    let mut bins: Vec<String> = selected
        .iter()
        .flat_map(|candidate| candidate["targets"].as_array().into_iter().flatten())
        .filter(|target| {
            target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        })
        .filter_map(|target| target["name"].as_str().map(str::to_owned))
        .collect();
    bins.sort();
    match bins.len() {
        0 => RunPick::Unresolved,
        1 => RunPick::Bin(bins.remove(0)),
        _ => RunPick::Ambiguous(bins),
    }
}

/// `rustc -vV` as rustup resolves it for the workspace. Oxide never selects
/// or pins the toolchain.
fn rustc_version(workspace: &Path) -> Result<String, String> {
    let rustc = std::env::var_os("RUSTC")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("rustc"));
    let output = Command::new(rustc)
        .arg("-vV")
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("could not start rustc: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "rustc -vV failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Git root commits plus the workspace path within the repository; shallow
/// clones fall back to the normalised `origin` URL; otherwise the path.
fn project_identity(workspace: &Path) -> String {
    let path_identity = || format!("path:{}", workspace.display());
    let shallow = git(workspace, &["rev-parse", "--is-shallow-repository"])
        .is_some_and(|value| value == "true");
    if shallow {
        return git(workspace, &["config", "--get", "remote.origin.url"])
            .map(|origin| format!("origin:{}", normalize_origin(&origin)))
            .unwrap_or_else(path_identity);
    }
    let Some(roots) =
        git(workspace, &["rev-list", "--max-parents=0", "HEAD"]).filter(|roots| !roots.is_empty())
    else {
        return path_identity();
    };
    let prefix = git(workspace, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    let mut commits: Vec<&str> = roots.lines().map(str::trim).collect();
    commits.sort_unstable();
    format!("git:{}:{prefix}", commits.join(","))
}

fn git(workspace: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Lowercase scheme and host, drop credentials, a trailing `/` and `.git`.
fn normalize_origin(origin: &str) -> String {
    let normalized = if let Some((scheme, rest)) = origin.split_once("://") {
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        format!(
            "{}://{}{path}",
            scheme.to_ascii_lowercase(),
            host.to_ascii_lowercase()
        )
    } else if let Some((user_host, path)) = origin.split_once(':') {
        let host = user_host.rsplit('@').next().unwrap_or(user_host);
        format!("{}:{path}", host.to_ascii_lowercase())
    } else {
        origin.to_owned()
    };
    let trimmed = normalized.trim_end_matches('/');
    trimmed.strip_suffix(".git").unwrap_or(trimmed).to_owned()
}

pub fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_forms_normalise_to_one_identity() {
        assert_eq!(
            normalize_origin("https://user:token@GitHub.com/acme/demo.git/"),
            "https://github.com/acme/demo"
        );
        assert_eq!(
            normalize_origin("git@GitHub.com:acme/demo.git"),
            "github.com:acme/demo"
        );
        assert_eq!(normalize_origin("/srv/demo.git"), "/srv/demo");
    }

    #[test]
    fn run_pick_follows_cargo_run_selection() {
        let package = |id: &str, default_run: Option<&str>, bins: &[&str]| {
            let mut targets: Vec<serde_json::Value> = bins
                .iter()
                .map(|name| serde_json::json!({"name": name, "kind": ["bin"]}))
                .collect();
            targets.push(serde_json::json!({"name": id, "kind": ["lib"]}));
            serde_json::json!({"id": id, "name": id, "default_run": default_run, "targets": targets})
        };
        let metadata = |defaults: &[&str]| {
            serde_json::json!({
                "packages": [
                    package("app", Some("app"), &["app", "other"]),
                    package("tool", None, &["tool"]),
                    package("pair", None, &["a", "b"]),
                    package("lib", None, &[]),
                ],
                "workspace_default_members": defaults,
            })
        };
        let pick =
            |defaults: &[&str], package: Option<&str>| select_run(&metadata(defaults), package);
        let bin = |name: &str| RunPick::Bin(name.to_owned());
        // A virtual root's default member with default-run, and a package root.
        assert_eq!(pick(&["app"], None), bin("app"));
        assert_eq!(pick(&["tool"], None), bin("tool"));
        // Several default members: one default-run wins; else one binary.
        assert_eq!(pick(&["app", "tool", "pair"], None), bin("app"));
        assert_eq!(pick(&["tool", "lib"], None), bin("tool"));
        assert_eq!(
            pick(&["tool", "pair"], None),
            RunPick::Ambiguous(vec!["a".to_owned(), "b".to_owned(), "tool".to_owned()])
        );
        // `-p` overrides the default members.
        assert_eq!(pick(&["pair"], Some("app")), bin("app"));
        assert_eq!(pick(&["app"], Some("lib")), RunPick::Unresolved);
        assert_eq!(pick(&["app"], Some("missing")), RunPick::Unresolved);
        assert_eq!(pick(&[], None), RunPick::Unresolved);
    }

    #[test]
    fn digest_is_lowercase_sha256_hex() {
        assert_eq!(
            digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
