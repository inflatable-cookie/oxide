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
    /// Digest of identity + Oxide recipe key + `rustc -vV` + target + backend.
    /// The compiler comes from the caller's context, never the checkout.
    pub compatibility: String,
}

impl SlotKey {
    /// Resolve for an already-located workspace. `destination` is the
    /// owned-output destination identity (kind, real root, volume and root
    /// generation) from [`crate::outputs::destination_facts`]; it enters the
    /// key only when it is non-empty, so an ephemeral or detached checkout
    /// keeps exactly the slot keys it had.
    pub fn resolve_in(
        checkout: &Path,
        recipe: &str,
        target: Option<&str>,
        destination: &str,
    ) -> Result<Self, String> {
        let checkout = checkout.to_path_buf();
        let identity = digest(&Identity::infer(&checkout).key());
        // The guarded Cargo inherits this process's working directory and
        // environment, so rustup selects its toolchain there, not at the
        // workspace root. `--manifest-path` moves neither.
        let rustc = rustc_version(None)?;
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
        let destination = if destination.is_empty() {
            String::new()
        } else {
            format!("\n{destination}")
        };
        let compatibility = digest(&format!(
            "{identity}\n{recipe}\n{}\n{target}\n{BACKEND}{destination}",
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
    crate::wrapper::apply(&mut command);
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

/// `rustc -vV` of `RUSTC`, else of `rustc` as rustup resolves it. `None`
/// runs it in the inherited working directory and environment, where the
/// guarded Cargo resolves its compiler (a member's `rust-toolchain.toml`
/// included); readiness names a directory instead. Oxide never selects or
/// pins the toolchain.
pub fn rustc_version(directory: Option<&Path>) -> Result<String, String> {
    let rustc = std::env::var_os("RUSTC")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("rustc"));
    let output = rustc_command(rustc, directory)
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

fn rustc_command(rustc: OsString, directory: Option<&Path>) -> Command {
    let mut command = Command::new(rustc);
    command.arg("-vV");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    command
}

/// Inferred repository identity: Git root commits plus the workspace path
/// within the repository; shallow clones fall back to the normalised
/// `origin` URL; otherwise the canonical path.
pub enum Identity {
    Git { roots: Vec<String>, prefix: String },
    Origin(String),
    Path(PathBuf),
}

impl Identity {
    pub fn infer(workspace: &Path) -> Self {
        let path_identity = || Self::Path(workspace.to_path_buf());
        let shallow = git(workspace, &["rev-parse", "--is-shallow-repository"])
            .is_some_and(|value| value == "true");
        if shallow {
            return git(workspace, &["config", "--get", "remote.origin.url"])
                .map(|origin| Self::Origin(normalize_origin(&origin)))
                .unwrap_or_else(path_identity);
        }
        let Some(roots) = git(workspace, &["rev-list", "--max-parents=0", "HEAD"])
            .filter(|roots| !roots.is_empty())
        else {
            return path_identity();
        };
        let prefix = git(workspace, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let mut roots: Vec<String> = roots.lines().map(|root| root.trim().to_owned()).collect();
        roots.sort_unstable();
        Self::Git { roots, prefix }
    }

    /// The string the slot key digests. Its format must not change: existing
    /// pool records hold digests of it.
    fn key(&self) -> String {
        match self {
            Self::Git { roots, prefix } => format!("git:{}:{prefix}", roots.join(",")),
            Self::Origin(origin) => format!("origin:{origin}"),
            Self::Path(path) => format!("path:{}", path.display()),
        }
    }

    /// One line for `oxide init` readiness.
    pub fn describe(&self) -> String {
        match self {
            Self::Git { roots, prefix } => {
                let short: Vec<&str> = roots
                    .iter()
                    .map(|root| root.get(..12).unwrap_or(root))
                    .collect();
                let place = if prefix.is_empty() {
                    "the repository root".to_owned()
                } else {
                    format!("`{}`", prefix.trim_end_matches('/'))
                };
                format!(
                    "Git root commit {} at {place}; worktrees and clones of this repository match",
                    short.join(", ")
                )
            }
            Self::Origin(origin) => format!("shallow clone of {origin}"),
            Self::Path(path) => format!(
                "the path {} (no Git commit yet), so other checkouts do not match",
                path.display()
            ),
        }
    }
}

/// Longest project label in a slot folder name.
const MAX_LABEL_BYTES: usize = 40;

/// Human-readable project label for a new slot's folder: the last segment
/// of the normalised `origin` URL, else the repository directory name,
/// else the workspace directory name; sanitised by [`sanitize_label`].
/// Presentation only: never part of the slot key.
pub fn project_label(workspace: &Path) -> String {
    let origin = git(workspace, &["config", "--get", "remote.origin.url"])
        .map(|origin| normalize_origin(&origin))
        .and_then(|origin| {
            origin
                .rsplit(['/', ':'])
                .next()
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
        });
    let directory = || {
        git(workspace, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.to_path_buf())
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    sanitize_label(&origin.or_else(directory).unwrap_or_default())
}

/// Lowercase ASCII letters and digits, `_` kept; every other run becomes
/// one `-`; no leading or trailing `-`; at most [`MAX_LABEL_BYTES`];
/// `project` when nothing remains.
pub fn sanitize_label(name: &str) -> String {
    let mut label = String::new();
    for character in name.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_' {
            label.push(character);
        } else if !label.is_empty() && !label.ends_with('-') {
            label.push('-');
        }
    }
    label.truncate(MAX_LABEL_BYTES);
    let label = label.trim_end_matches('-');
    if label.is_empty() {
        "project".to_owned()
    } else {
        label.to_owned()
    }
}

/// Folder names for a new slot of one repository, preferred first:
/// `<label>-<identity digest prefix>` with 8, 12, 16 and all 64 hex digits.
/// A short prefix is display, not a uniqueness proof: the pool skips a
/// folder that holds another repository's slot and tries the next.
pub fn slot_folders(label: &str, identity: &str) -> Vec<String> {
    [8, 12, 16, identity.len()]
        .into_iter()
        .filter_map(|digits| identity.get(..digits))
        .map(|prefix| format!("{label}-{prefix}"))
        .fold(Vec::new(), |mut folders, folder| {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
            folders
        })
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
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn labels_are_sanitized_plain_names() {
        assert_eq!(sanitize_label("Oxide"), "oxide");
        assert_eq!(sanitize_label("my app (copy).rs"), "my-app-copy-rs");
        assert_eq!(sanitize_label("--__weird__--"), "__weird__");
        assert_eq!(sanitize_label("../.."), "project");
        assert_eq!(sanitize_label("日本"), "project");
        assert_eq!(sanitize_label(""), "project");
        let long = sanitize_label(&"a-".repeat(60));
        assert!(
            long.len() <= MAX_LABEL_BYTES && !long.ends_with('-'),
            "{long}"
        );
        for label in [
            "oxide",
            "my-app-copy-rs",
            "__weird__",
            "project",
            long.as_str(),
        ] {
            assert!(!label.starts_with('.') && !label.contains('/'), "{label}");
        }
    }

    /// Same-name repositories differ by identity; an injected collision of
    /// the shortest prefix still differs at a longer one, ending at the full
    /// digest.
    #[test]
    fn folders_disambiguate_same_name_repositories() {
        let first = digest("git:aaa:");
        let second = digest("git:bbb:");
        assert_ne!(
            slot_folders("app", &first)[0],
            slot_folders("app", &second)[0]
        );
        let collide_a = format!("deadbeef1111{}", "0".repeat(52));
        let collide_b = format!("deadbeef2222{}", "0".repeat(52));
        let a = slot_folders("app", &collide_a);
        let b = slot_folders("app", &collide_b);
        assert_eq!(a[0], "app-deadbeef");
        assert_eq!(a[0], b[0]);
        assert_ne!(a[1], b[1]);
        assert_eq!(a.last().unwrap(), &format!("app-{collide_a}"));
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn label_prefers_the_origin_segment_then_the_directory() {
        let dir = scratch("label");
        let checkout = dir.join("My Checkout");
        fs::create_dir(&checkout).expect("checkout");
        assert_eq!(project_label(&checkout), "my-checkout");
        let git_ok = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&checkout)
            .status()
            .is_ok_and(|status| status.success());
        if git_ok {
            assert_eq!(project_label(&checkout), "my-checkout");
            Command::new("git")
                .args([
                    "remote",
                    "add",
                    "origin",
                    "git@GitHub.com:owner/Some.Repo.git",
                ])
                .current_dir(&checkout)
                .status()
                .expect("git remote");
            assert_eq!(project_label(&checkout), "some-repo");
        }
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    fn scratch(label: &str) -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("identity-{label}-{:x}", std::process::id()));
        fs::create_dir(&dir).expect("scratch dir");
        fs::canonicalize(dir).expect("canonical scratch")
    }

    /// A stand-in `rustc` that prints where it ran. The slot key's query runs
    /// in the inherited directory and environment, as the guarded Cargo
    /// does; readiness runs where it was asked.
    #[test]
    fn rustc_query_runs_in_the_callers_context() {
        let dir = scratch("rustc-context");
        let fake = dir.join("rustc");
        fs::write(&fake, "#!/bin/sh\npwd -P\n").expect("stand-in rustc");
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("executable");
        let member = dir.join("member");
        fs::create_dir(&member).expect("member dir");
        let query =
            |directory: Option<&Path>| rustc_command(fake.clone().into_os_string(), directory);
        let ran = |directory: Option<&Path>| {
            let output = query(directory).output().expect("stand-in rustc ran");
            assert!(output.status.success());
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        };
        let caller =
            fs::canonicalize(std::env::current_dir().expect("cwd")).expect("canonical cwd");
        assert_eq!(ran(None), caller);
        assert_eq!(ran(Some(&member)), member);
        let inherited = query(None);
        assert_eq!(inherited.get_current_dir(), None);
        assert_eq!(inherited.get_envs().count(), 0);
        assert_eq!(inherited.get_args().collect::<Vec<_>>(), ["-vV"]);
        fs::remove_dir_all(&dir).expect("remove scratch");
    }

    /// `--manifest-path` names the workspace that owns the slot; the
    /// compiler in the key stays the caller's.
    #[test]
    fn manifest_path_moves_ownership_not_the_compiler() {
        let dir = scratch("manifest-path");
        let workspace = dir.join("other");
        fs::create_dir_all(workspace.join("src")).expect("package dir");
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("manifest");
        fs::write(workspace.join("src/lib.rs"), "").expect("lib");
        let owned =
            locate_workspace(crate::CARGO, Some(&workspace.join("Cargo.toml"))).expect("workspace");
        let key = SlotKey::resolve_in(&owned, "quick", Some("x-target"), "").expect("slot key");
        assert_eq!(key.checkout, workspace);
        assert!(matches!(Identity::infer(&workspace), Identity::Path(path) if path == workspace));
        let identity = digest(&format!("path:{}", workspace.display()));
        assert_eq!(key.identity, identity);
        let rustc = rustc_version(None).expect("caller rustc");
        assert_eq!(
            key.compatibility,
            digest(&format!(
                "{identity}\nquick\n{}\nx-target\ndefault",
                digest(&rustc)
            ))
        );
        fs::remove_dir_all(&dir).expect("remove scratch");
    }

    /// A real final-output destination is part of the compatibility key; an
    /// empty one leaves the key exactly as it was.
    #[test]
    fn a_destination_partitions_the_compatibility_key() {
        let dir = scratch("destination");
        let workspace = dir.join("app");
        fs::create_dir_all(workspace.join("src")).expect("package dir");
        fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("manifest");
        fs::write(workspace.join("src/lib.rs"), "").expect("lib");
        let owned =
            locate_workspace(crate::CARGO, Some(&workspace.join("Cargo.toml"))).expect("workspace");
        let ephemeral =
            SlotKey::resolve_in(&owned, "quick", Some("x-target"), "").expect("slot key");
        let external = SlotKey::resolve_in(&owned, "quick", Some("x-target"), "outputs-1 x")
            .expect("slot key");
        let local = SlotKey::resolve_in(&owned, "quick", Some("x-target"), "outputs-1 y")
            .expect("slot key");
        assert_eq!(ephemeral.identity, external.identity);
        assert_ne!(ephemeral.compatibility, external.compatibility);
        assert_ne!(external.compatibility, local.compatibility);
        fs::remove_dir_all(&dir).expect("remove scratch");
    }

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
    fn identity_key_format_is_stable() {
        let git = Identity::Git {
            roots: vec!["a1".to_owned(), "b2".to_owned()],
            prefix: "crates/app/".to_owned(),
        };
        assert_eq!(git.key(), "git:a1,b2:crates/app/");
        assert_eq!(
            Identity::Origin("github.com:acme/demo".to_owned()).key(),
            "origin:github.com:acme/demo"
        );
        assert_eq!(Identity::Path(PathBuf::from("/w")).key(), "path:/w");
        assert!(git.describe().contains("`crates/app`"));
    }

    #[test]
    fn digest_is_lowercase_sha256_hex() {
        assert_eq!(
            digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// FIPS 180-2 vectors through every sha2 entry point Oxide relies on:
    /// one-shot, bytes, hex and streaming (oxide#103, sha2 0.10.9 to 0.11.0).
    #[test]
    fn sha256_published_vectors() {
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        let bytes = Sha256::digest(b"abc");
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[..4], &[0xba, 0x78, 0x16, 0xbf]);
        let mut streaming = Sha256::new();
        let block = [b'a'; 1000];
        for _ in 0..1000 {
            streaming.update(block);
        }
        let million: String = streaming
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            million,
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Slot identity and compatibility keys already stored in ledgers keep
    /// their values, so existing slots and their folders stay warm.
    /// Expected values are independent hashlib digests of these exact keys.
    #[test]
    fn stored_slot_keys_are_unchanged() {
        let identity = digest("git:https://github.com/inflatable-cookie/oxide:");
        assert_eq!(
            identity,
            "4344f9285b31bb4b9e695c75b8af04a5f4a97a99705393e8d0126c94832eec4f"
        );
        assert_eq!(
            digest("path:/Users/Zoë/work space/app"),
            "ee1f4d90c2ba0b15755ecf407d5adede49f2162bdf5fe849275ef7cb2f992480"
        );
        let rustc = "rustc 1.97.1 (5f8b4d3a5 2026-09-17)\nbinary: rustc\nhost: aarch64-apple-darwin\nrelease: 1.97.1\n";
        assert_eq!(
            digest(rustc),
            "8a4dd343d46011358e461397f0686700184525ab1e416ced746b5bef64c87fd7"
        );
        assert_eq!(
            digest(&format!(
                "{identity}\nquick\n{}\naarch64-apple-darwin\n{BACKEND}",
                digest(rustc)
            )),
            "12d9fc45c5c84a2e41a4e96d4918b08d421b41fd92114cbecd1eecb4b0a16936"
        );
        assert_eq!(slot_folders("oxide", &identity)[0], "oxide-4344f928");
    }
}
