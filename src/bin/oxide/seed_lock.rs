//! Bounded lockfile eligibility for cold-slot seeding.
//!
//! This is only the first filter: source verification and the manifest/dep-info
//! join must still establish that an eligible lock entry produced a unit.

use serde::Deserialize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

const MAX_LOCK_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKAGES: usize = 2048;
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

#[derive(Debug, Deserialize)]
struct Lock {
    version: u32,
    #[serde(default)]
    package: Vec<Package>,
    // Patch replacements are not modeled by the v1 adapter.
    patch: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct Package {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub checksum: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Read a regular owner-owned lock without following the final symlink. A
/// bounded read also handles a file that grows after metadata was sampled.
pub(crate) fn read(path: &Path) -> Result<Vec<Package>, &'static str> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "lock unavailable")?;
    let metadata = file.metadata().map_err(|_| "lock unavailable")?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.len() > MAX_LOCK_BYTES as u64
    {
        return Err("lock unavailable");
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_LOCK_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "lock unavailable")?;
    eligible(&bytes)
}

/// Filter by normalized-name uniqueness across *all* lock entries, leafness,
/// and supported immutable source identity. The order remains lock order.
pub(crate) fn eligible(bytes: &[u8]) -> Result<Vec<Package>, &'static str> {
    if bytes.len() > MAX_LOCK_BYTES {
        return Err("lock too large");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid lock")?;
    let lock: Lock = basic_toml::from_str(text).map_err(|_| "invalid lock")?;
    if !matches!(lock.version, 3 | 4) || lock.patch.is_some() || lock.package.len() > MAX_PACKAGES {
        return Err("unsupported lock");
    }
    let mut names = HashMap::new();
    for package in &lock.package {
        if package.name.is_empty()
            || package.name.len() > 255
            || package.version.is_empty()
            || package.version.len() > 255
            || !package
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || package.source.as_ref().is_some_and(|s| s.len() > 1024)
        {
            return Err("invalid lock");
        }
        *names
            .entry(package.name.replace('-', "_"))
            .or_insert(0usize) += 1;
    }
    Ok(lock
        .package
        .into_iter()
        .filter(|package| {
            if names[&package.name.replace('-', "_")] != 1 || !package.dependencies.is_empty() {
                return false;
            }
            match package.source.as_deref() {
                Some(CRATES_IO) => package
                    .checksum
                    .as_deref()
                    .is_some_and(|value| hex(value, 64)),
                Some(source) if source.starts_with("git+") => source
                    .rsplit_once('#')
                    .is_some_and(|(_, revision)| hex(revision, 40)),
                _ => false,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_includes_path_packages_and_non_leaves_are_excluded() {
        let input = format!(
            r#"version = 4
[[package]]
name = "cfg-if"
version = "1.0.0"
source = "{CRATES_IO}"
checksum = "{}"
[[package]]
name = "foo-bar"
version = "1"
source = "{CRATES_IO}"
checksum = "{}"
[[package]]
name = "foo_bar"
version = "1"
[[package]]
name = "nonleaf"
version = "1"
source = "{CRATES_IO}"
checksum = "{}"
dependencies = ["cfg-if"]
"#,
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64)
        );
        let packages = eligible(input.as_bytes()).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "cfg-if");
    }

    #[test]
    fn invalid_lock_and_unpinned_sources_are_ineligible() {
        assert!(eligible(b"[[package").is_err());
        let input = b"version = 4\n[[package]]\nname = 'git'\nversion = '1'\nsource = 'git+https://example.invalid/repo#abc'\n";
        assert!(eligible(input).unwrap().is_empty());
        assert!(eligible(&vec![b' '; MAX_LOCK_BYTES + 1]).is_err());
    }
}
