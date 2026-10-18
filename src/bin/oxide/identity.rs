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
    /// The package's `default-run`, or its only binary.
    Bin(String),
    /// Several binaries and no `default-run`: Cargo refuses before building.
    Ambiguous(Vec<String>),
    /// No single package resolved (for example a virtual manifest); the
    /// build reports what it produced.
    Unresolved,
}

/// Resolve `cargo run`'s target from `cargo metadata --no-deps`: the named
/// package, else the package whose manifest is nearest the current directory.
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
    let current = match package {
        Some(_) => None,
        None => Some(nearest_manifest(cargo, manifest_path)?),
    };
    Ok(select_run(&metadata, package, current.as_deref()))
}

fn nearest_manifest(cargo: &str, manifest_path: Option<&Path>) -> Result<PathBuf, String> {
    let mut command = Command::new(cargo);
    command.args(["locate-project", "--message-format", "plain"]);
    if let Some(manifest) = manifest_path {
        command.arg("--manifest-path").arg(manifest);
    }
    let output = command
        .output()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not locate the package: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let manifest = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    fs::canonicalize(&manifest).map_err(|error| format!("cannot resolve the manifest: {error}"))
}

fn select_run(
    metadata: &serde_json::Value,
    package: Option<&str>,
    current_manifest: Option<&Path>,
) -> RunPick {
    let Some(packages) = metadata["packages"].as_array() else {
        return RunPick::Unresolved;
    };
    let selected = packages.iter().find(|candidate| match package {
        Some(name) => candidate["name"].as_str() == Some(name),
        None => candidate["manifest_path"]
            .as_str()
            .and_then(|path| fs::canonicalize(path).ok())
            .is_some_and(|path| Some(path.as_path()) == current_manifest),
    });
    let Some(selected) = selected else {
        return RunPick::Unresolved;
    };
    if let Some(default_run) = selected["default_run"].as_str() {
        return RunPick::Bin(default_run.to_owned());
    }
    let mut bins: Vec<String> = selected["targets"]
        .as_array()
        .into_iter()
        .flatten()
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
        let metadata = serde_json::json!({"packages": [
            {"name": "app", "manifest_path": "/nonexistent/app/Cargo.toml", "default_run": "app",
             "targets": [{"name": "app", "kind": ["bin"]}, {"name": "other", "kind": ["bin"]}]},
            {"name": "tool", "manifest_path": "/nonexistent/tool/Cargo.toml", "default_run": null,
             "targets": [{"name": "tool", "kind": ["bin"]}, {"name": "tool", "kind": ["lib"]}]},
            {"name": "pair", "manifest_path": "/nonexistent/pair/Cargo.toml", "default_run": null,
             "targets": [{"name": "a", "kind": ["bin"]}, {"name": "b", "kind": ["bin"]}]},
            {"name": "lib", "manifest_path": "/nonexistent/lib/Cargo.toml", "default_run": null,
             "targets": [{"name": "lib", "kind": ["lib"]}]}
        ]});
        assert_eq!(
            select_run(&metadata, Some("app"), None),
            RunPick::Bin("app".to_owned())
        );
        assert_eq!(
            select_run(&metadata, Some("tool"), None),
            RunPick::Bin("tool".to_owned())
        );
        assert_eq!(
            select_run(&metadata, Some("pair"), None),
            RunPick::Ambiguous(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            select_run(&metadata, Some("lib"), None),
            RunPick::Unresolved
        );
        assert_eq!(
            select_run(&metadata, Some("missing"), None),
            RunPick::Unresolved
        );
        assert_eq!(
            select_run(
                &metadata,
                None,
                Some(Path::new("/nonexistent/app/Cargo.toml"))
            ),
            RunPick::Unresolved
        );
    }

    #[test]
    fn digest_is_lowercase_sha256_hex() {
        assert_eq!(
            digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
