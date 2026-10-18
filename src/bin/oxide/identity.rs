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
    fn digest_is_lowercase_sha256_hex() {
        assert_eq!(
            digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
