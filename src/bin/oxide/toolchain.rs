//! Toolchain identity without a `rustc` spawn when nothing that selects the
//! toolchain changed.
//!
//! The slot key needs `rustc -vV` as rustup resolves it for the workspace.
//! A stat-only observation covers what decides that answer: the toolchain
//! environment, every `rust-toolchain` / `rust-toolchain.toml` from the
//! workspace up to `/` (absence included), rustup's `settings.toml`, the
//! `rustc` that `RUSTC` or `PATH` names, and, once known, the resolved
//! toolchain's `bin/rustc` and `lib`. A cache entry under the workspace's
//! `target/` pairs that observation with the output; it is used only when
//! the observation is identical, never on age. `rustc -vV` runs again
//! whenever any input differs. A `rustc` that is neither a rustup proxy nor
//! the toolchain's own binary (a wrapper script, a relative path) is never
//! cached: every command spawns it, as before.

use sha2::{Digest, Sha256};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CACHE_FILE: &str = "oxide-toolchain.json";
const CACHE_VERSION: u32 = 1;
const MAX_CACHE_BYTES: u64 = 64 * 1024;
const TOOLCHAIN_FILES: [&str; 2] = ["rust-toolchain", "rust-toolchain.toml"];

/// `rustc -vV` and, when every input was observable, the stat-only
/// observation it was keyed on.
pub struct Toolchain {
    pub version: String,
    /// Digest of the complete observation, resolved toolchain included: the
    /// shared stat-only toolchain identity for keys beyond the slot's.
    /// `None`: an input was not observable, so nothing was cached.
    #[expect(dead_code, reason = "the approved skip-path key consumes it next")]
    pub observation: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32,
    inputs: String,
    sysroot: PathBuf,
    resolved: String,
    rustc: String,
}

/// `rustc -vV` for `workspace`, from the cache when the observation matches.
pub fn identify(workspace: &Path) -> Result<Toolchain, String> {
    let Some(inputs) = observe_inputs(workspace) else {
        return spawn_version(workspace).map(|version| Toolchain {
            version,
            observation: None,
        });
    };
    let cache = workspace.join("target").join(CACHE_FILE);
    if let Some(entry) = read_entry(&cache).filter(|entry| entry.inputs == inputs.digest) {
        if observe_sysroot(&entry.sysroot).as_deref() == Some(entry.resolved.as_str()) {
            return Ok(Toolchain {
                observation: Some(digest(&[
                    entry.inputs.as_bytes(),
                    entry.resolved.as_bytes(),
                ])),
                version: entry.rustc,
            });
        }
    }
    let (version, sysroot) = std::thread::scope(|scope| {
        let sysroot = scope.spawn(|| spawn_sysroot(workspace));
        let version = spawn_version(workspace);
        (version, sysroot.join())
    });
    let version = version?;
    let Ok(Some(sysroot)) = sysroot else {
        return Ok(Toolchain {
            version,
            observation: None,
        });
    };
    let resolved = observe_sysroot(&sysroot);
    let own_binary = || {
        let real = fs::canonicalize(sysroot.join("bin").join("rustc")).ok();
        real.is_some() && real == fs::canonicalize(&inputs.rustc).ok()
    };
    // Unchanged across the spawns, and either rustup's proxy or the
    // toolchain's own binary: anything else stays uncached.
    let stable = observe_inputs(workspace).is_some_and(|again| again.digest == inputs.digest);
    match resolved {
        Some(resolved) if stable && (inputs.proxy || own_binary()) => {
            let observation = digest(&[inputs.digest.as_bytes(), resolved.as_bytes()]);
            write_entry(
                &cache,
                &Entry {
                    version: CACHE_VERSION,
                    inputs: inputs.digest,
                    sysroot,
                    resolved,
                    rustc: version.clone(),
                },
            );
            Ok(Toolchain {
                version,
                observation: Some(observation),
            })
        }
        _ => Ok(Toolchain {
            version,
            observation: None,
        }),
    }
}

/// The program Oxide runs for `rustc`: `RUSTC` when set, else `rustc`.
fn rustc_program() -> OsString {
    std::env::var_os("RUSTC")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("rustc"))
}

fn spawn_version(workspace: &Path) -> Result<String, String> {
    let output = Command::new(rustc_program())
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

/// The resolved toolchain's sysroot; `None` when it cannot be read.
fn spawn_sysroot(workspace: &Path) -> Option<PathBuf> {
    let output = Command::new(rustc_program())
        .args(["--print", "sysroot"])
        .current_dir(workspace)
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(text.trim());
    (output.status.success() && path.is_absolute()).then_some(path)
}

struct Inputs {
    digest: String,
    /// The `rustc` program as found, before following links.
    rustc: PathBuf,
    /// It is rustup's proxy.
    proxy: bool,
}

/// Everything that selects the toolchain, by stat only. `None` when an input
/// cannot be observed exactly.
fn observe_inputs(workspace: &Path) -> Option<Inputs> {
    let mut parts: Vec<Vec<u8>> = Vec::new();
    let mut environment: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.as_bytes();
            name.starts_with(b"RUSTUP_")
                || [&b"RUSTC"[..], b"PATH", b"HOME", b"CARGO_HOME"].contains(&name)
        })
        .collect();
    environment.sort();
    for (name, value) in &environment {
        parts.push([name.as_bytes(), b"=", value.as_bytes()].concat());
    }
    parts.push(format!("workspace={}", workspace.display()).into_bytes());
    for directory in workspace.ancestors() {
        for name in TOOLCHAIN_FILES {
            let path = directory.join(name);
            parts.push(stat_line(&path)?);
        }
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty());
    let rustup_home = match std::env::var_os("RUSTUP_HOME").filter(|home| !home.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => Path::new(&home?).join(".rustup"),
    };
    if !rustup_home.is_absolute() {
        return None;
    }
    parts.push(stat_line(&rustup_home.join("settings.toml"))?);
    let rustc = find_program(&rustc_program())?;
    parts.push(stat_line(&rustc)?);
    let proxy = is_rustup_proxy(&rustc);
    parts.push(format!("proxy={proxy}").into_bytes());
    let parts: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    Some(Inputs {
        digest: digest(&parts),
        rustc,
        proxy,
    })
}

/// The toolchain binary and library directory under `sysroot`.
fn observe_sysroot(sysroot: &Path) -> Option<String> {
    let binary = stat_line(&sysroot.join("bin").join("rustc"))?;
    let library = stat_line(&sysroot.join("lib"))?;
    let path = sysroot.as_os_str().as_bytes();
    Some(digest(&[path, binary.as_slice(), library.as_slice()]))
}

/// Resolve `program` as the spawn will: an absolute path as is, a bare name
/// through `PATH`. A relative path or a relative `PATH` entry depends on the
/// working directory, so it is not observed.
fn find_program(program: &OsStr) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if program.as_bytes().contains(&b'/') {
        return None;
    }
    let search = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&search) {
        if !directory.is_absolute() {
            return None;
        }
        let candidate = directory.join(program);
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() && metadata.mode() & 0o111 != 0 => {
                return Some(candidate)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

/// rustup installs its proxies as hard links or symlinks to `rustup` (or
/// `rustup-init`).
fn is_rustup_proxy(rustc: &Path) -> bool {
    let named_rustup = |path: &Path| {
        path.file_name()
            .is_some_and(|name| name == "rustup" || name == "rustup-init")
    };
    if fs::canonicalize(rustc).is_ok_and(|real| named_rustup(&real)) {
        return true;
    }
    let Some(directory) = rustc.parent() else {
        return false;
    };
    match (fs::metadata(rustc), fs::metadata(directory.join("rustup"))) {
        (Ok(one), Ok(two)) => one.dev() == two.dev() && one.ino() == two.ino(),
        _ => false,
    }
}

/// Identity of a path after following links, or its absence. Any other
/// error makes the observation unknown.
fn stat_line(path: &Path) -> Option<Vec<u8>> {
    let state = match fs::metadata(path) {
        Ok(metadata) => format!(
            "{} {} {} {} {}.{} {}.{}",
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent".to_owned(),
        Err(_) => return None,
    };
    let canonical = fs::canonicalize(path).ok();
    Some(
        [
            path.as_os_str().as_bytes(),
            b"\0",
            canonical
                .as_deref()
                .map_or(&b""[..], |real| real.as_os_str().as_bytes()),
            b"\0",
            state.as_bytes(),
        ]
        .concat(),
    )
}

fn read_entry(path: &Path) -> Option<Entry> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_CACHE_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_CACHE_BYTES {
        return None;
    }
    serde_json::from_slice::<Entry>(&bytes)
        .ok()
        .filter(|entry| entry.version == CACHE_VERSION)
}

/// Best effort: a cache that cannot be written only costs the next command
/// a spawn. Written only where Cargo already made `target/`.
fn write_entry(path: &Path, entry: &Entry) {
    let Some(target) = path.parent().filter(|target| target.is_dir()) else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(entry) else {
        return;
    };
    let temp = target.join(format!("{CACHE_FILE}.{}.tmp", std::process::id()));
    let written = fs::File::create(&temp)
        .and_then(|mut file| file.write_all(&bytes))
        .and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
}

fn digest(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_present_files_observe_differently() {
        let missing = Path::new("/nonexistent-oxide-toolchain-probe/rust-toolchain");
        let line = stat_line(missing).expect("absence is observable");
        assert!(line.ends_with(b"absent"));
        let present = stat_line(Path::new("/")).expect("root is observable");
        assert!(!present.ends_with(b"absent"));
    }

    #[test]
    fn relative_programs_are_not_observed() {
        assert!(find_program(OsStr::new("bin/rustc")).is_none());
        assert_eq!(
            find_program(OsStr::new("/opt/rustc")),
            Some(PathBuf::from("/opt/rustc"))
        );
    }

    #[test]
    fn digest_separates_parts() {
        assert_ne!(digest(&[b"ab", b"c"]), digest(&[b"a", b"bc"]));
    }
}
