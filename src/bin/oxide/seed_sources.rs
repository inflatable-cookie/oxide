//! Source adapter calls used by the cold-seed front door.
//!
//! Each pass has one shared budget. A source is returned only after its lock
//! identity, authenticated content and supported library target are checked.

use crate::seed_lock::Package;
use crate::seed_plan::VerifiedSource;
use oxide_storage::lease::managed::SeedSourceKind;
use oxide_storage::seed_source::{
    GitRefusal, GitVerifier, RegistrySource, SourceCache, SourceLimits,
};
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

const MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Manifest {
    package: ManifestPackage,
    lib: Option<ManifestLib>,
}

#[derive(Deserialize)]
struct ManifestPackage {
    name: String,
    version: String,
    #[serde(default = "yes")]
    autolib: bool,
}

#[derive(Deserialize)]
struct ManifestLib {
    name: Option<String>,
    path: Option<String>,
    #[serde(rename = "proc-macro", default)]
    proc_macro: bool,
}

fn yes() -> bool {
    true
}

fn manifest_matches(root: &Path, package: &Package) -> Option<bool> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("Cargo.toml"))
    {
        Ok(file) => file,
        Err(_) => return None,
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() <= MANIFEST_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    if file
        .by_ref()
        .take((MANIFEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MANIFEST_BYTES
    {
        return None;
    }
    let Some(manifest) = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| basic_toml::from_str::<Manifest>(text).ok())
    else {
        return None;
    };
    if manifest.package.name != package.name || manifest.package.version != package.version {
        return None;
    }
    let normalized = package.name.replace('-', "_");
    let (path, proc_macro) = match manifest.lib {
        Some(lib) if lib.name.as_deref().is_none_or(|name| name == normalized) => (
            lib.path.unwrap_or_else(|| "src/lib.rs".into()),
            lib.proc_macro,
        ),
        Some(_) => return None,
        None if manifest.package.autolib => ("src/lib.rs".into(), false),
        None => return None,
    };
    let relative = Path::new(&path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    // Source verification forbids links. This further binds the selected lib
    // path to the verified tree, including all of its parent components.
    let Ok(root) = std::fs::canonicalize(root) else {
        return None;
    };
    let mut path = root;
    for component in relative.components() {
        path.push(component.as_os_str());
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            return None;
        };
        if metadata.file_type().is_symlink() {
            return None;
        }
    }
    std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|_| proc_macro)
}

/// A single pass's limits are shared across packages rather than reset for
/// every adapter call. An unknown git child closure is a refusal, not a miss.
pub(crate) struct Pass {
    started: Instant,
    duration: Duration,
    entries: u64,
    content: u64,
    derived: u64,
    git: GitVerifier,
}

impl Pass {
    pub fn new(duration: Duration) -> Self {
        Self {
            started: Instant::now(),
            duration,
            entries: 0,
            content: 0,
            derived: 0,
            git: GitVerifier::new(),
        }
    }

    fn limits(&self) -> Option<SourceLimits> {
        let defaults = SourceLimits::default();
        let remaining = self.duration.checked_sub(self.started.elapsed())?;
        if remaining.is_zero() {
            return None;
        }
        Some(SourceLimits {
            max_entries: defaults.max_entries.checked_sub(self.entries)?,
            max_content_bytes: defaults.max_content_bytes.checked_sub(self.content)?,
            max_derived_bytes: defaults.max_derived_bytes.checked_sub(self.derived)?,
            deadline: remaining.min(defaults.deadline),
            ..defaults
        })
    }

    /// Post-build producing-window check. Every visited entry is charged to
    /// the same pass budget as source verification; symlinks and unreadable
    /// timestamps never become evidence of an unchanged source.
    pub fn unchanged_since(
        &mut self,
        root: &Path,
        kind: SeedSourceKind,
        started: SystemTime,
    ) -> bool {
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            if self.started.elapsed() >= self.duration {
                return false;
            }
            let Ok(entries) = std::fs::read_dir(directory) else {
                return false;
            };
            for entry in entries {
                if self.started.elapsed() >= self.duration {
                    return false;
                }
                let Ok(entry) = entry else {
                    return false;
                };
                if entry.path().parent() == Some(root)
                    && (entry.file_name() == ".cargo-ok"
                        || (kind == SeedSourceKind::Git && entry.file_name() == ".git"))
                {
                    continue;
                }
                self.entries = match self.entries.checked_add(1) {
                    Some(value) if value <= SourceLimits::default().max_entries => value,
                    _ => return false,
                };
                let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                    return false;
                };
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else if metadata.is_file() {
                    if !metadata.modified().is_ok_and(|mtime| mtime <= started) {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        }
        true
    }

    pub fn registry(
        &mut self,
        home: &Path,
        package: &Package,
        cache: &mut SourceCache,
        now: u64,
    ) -> Option<VerifiedSource> {
        if package.source.as_deref()
            != Some("registry+https://github.com/rust-lang/crates.io-index")
        {
            return None;
        }
        let source =
            RegistrySource::locate(home, &format!("{}-{}", package.name, package.version)).ok()?;
        let verified = source
            .verify(package.checksum.as_deref()?, &self.limits()?, cache, now)
            .ok()?;
        self.entries = self.entries.checked_add(verified.entries)?;
        self.content = self.content.checked_add(verified.content_bytes)?;
        self.derived = self.derived.checked_add(verified.derived_bytes)?;
        let proc_macro = manifest_matches(source.tree(), package)?;
        Some(VerifiedSource {
            package: package.clone(),
            tree_digest: verified.digest.to_hex(),
            kind: SeedSourceKind::Registry,
            root: source.tree().to_path_buf(),
            proc_macro,
        })
    }

    pub fn git(
        &mut self,
        root: &Path,
        package: &Package,
    ) -> Result<Option<VerifiedSource>, &'static str> {
        let Some(source) = package
            .source
            .as_deref()
            .filter(|source| source.starts_with("git+"))
        else {
            return Ok(None);
        };
        let Some((_, revision)) = source.rsplit_once('#') else {
            return Ok(None);
        };
        let Some(limits) = self.limits() else {
            return Ok(None);
        };
        let verified = match self.git.verify(root, revision, &limits) {
            Ok(verified) => verified,
            Err(GitRefusal::Unconfirmed) => return Err("git source verification closure unknown"),
            Err(_) => return Ok(None),
        };
        self.entries = self
            .entries
            .checked_add(verified.files)
            .ok_or("source cap")?;
        self.content = self
            .content
            .checked_add(verified.content_bytes)
            .ok_or("source cap")?;
        let Some(proc_macro) = manifest_matches(root, package) else {
            return Ok(None);
        };
        Ok(Some(VerifiedSource {
            package: package.clone(),
            tree_digest: verified.digest.to_hex(),
            kind: SeedSourceKind::Git,
            root: PathBuf::from(root),
            proc_macro,
        }))
    }
}
