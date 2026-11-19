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
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    #[serde(rename = "crate-type", default)]
    crate_type: Option<Vec<String>>,
}

fn yes() -> bool {
    true
}

/// Classify a declared library target from its crate types, which the pinned
/// layout depends on. A plain Rust library is `lib` (rustc's rlib) or `rlib`;
/// a proc-macro target is classified as one so the caller never offers it;
/// every other crate type emits outputs outside the pinned layout, so the
/// package is ineligible.
fn lib_target(lib: &ManifestLib) -> Option<(String, bool)> {
    let path = lib.path.clone().unwrap_or_else(|| "src/lib.rs".to_owned());
    let declared: &[String] = lib.crate_type.as_deref().unwrap_or(&[]);
    let single = |kind: &str| matches!(declared, [only] if only == kind);
    if lib.proc_macro || declared.iter().any(|kind| kind == "proc-macro") {
        // `proc-macro = true` with a conflicting crate type, or a mixed list,
        // is not a shape Cargo would build; it is never classified here.
        return (declared.is_empty() || single("proc-macro")).then_some((path, true));
    }
    (declared.is_empty() || single("lib") || single("rlib")).then_some((path, false))
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
        Some(lib) if lib.name.as_deref().is_none_or(|name| name == normalized) => lib_target(&lib)?,
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
    std::fs::symlink_metadata(path)
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

/// A discovered Git checkout is only a hint. The adapter still checks its
/// full locked revision, worktree and manifest before accepting it.
pub(crate) struct GitHint {
    pub package: Package,
    pub checkout: PathBuf,
    pub package_root: PathBuf,
}

/// Evidence captured before a producing Cargo command and rechecked after its
/// writer group has closed. Creating it never publishes a sidecar.
pub(crate) struct Production {
    pub producer: oxide_storage::lease::managed::SeedProducer,
    pub lock: crate::seed_lock::Analysis,
    cargo_home: PathBuf,
    cache: SourceCache,
    before: Vec<VerifiedSource>,
    started: SystemTime,
    started_ms: u64,
}

pub(crate) enum PreparationError {
    Skip(&'static str),
    SkipNamed(String),
    Refuse(String),
}

impl Production {
    /// Prepare from the same working directory and environment Cargo will
    /// inherit. This is called only after the qualification gate is open.
    pub fn prepare(
        invocation: &crate::cli::Invocation,
        checkout: &Path,
        held: &crate::store::Lease,
        store: &crate::store::Store,
    ) -> Result<Self, PreparationError> {
        use crate::seed_context::{collect, producer, ProducerFacts};
        if held.reused || !matches!(held.prior, crate::freshness::Prior::Fresh) {
            return Err(PreparationError::Skip("recipient not fresh"));
        }
        if !invocation.tool.is_empty()
            || invocation.targets.len() > 1
            || invocation.cargo_args.iter().any(|arg| {
                use std::os::unix::ffi::OsStrExt;
                arg.as_os_str().as_bytes().starts_with(b"-Z")
            })
        {
            return Err(PreparationError::Skip("unsupported command"));
        }
        let lock =
            crate::seed_lock::read(&checkout.join("Cargo.lock")).map_err(PreparationError::Skip)?;
        if lock.eligible.is_empty() {
            return Err(PreparationError::SkipNamed(crate::seed_lock::skip_reason(
                "no eligible locked packages",
                &lock,
            )));
        }
        let cwd = std::env::current_dir()
            .map_err(|_| PreparationError::Skip("working directory unavailable"))?;
        let cargo_home = crate::cargo_home::resolve()
            .ok()
            .and_then(|path| std::fs::canonicalize(path).ok())
            .ok_or(PreparationError::Skip("Cargo home unavailable"))?;
        let environment: Vec<_> = std::env::vars_os().collect();
        let context = collect(&cwd, &cargo_home, &invocation.cargo_args, &environment)
            .map_err(PreparationError::Skip)?;
        let cargo = crate::freshness::verified_cargo()
            .map_err(|_| PreparationError::Skip("unsupported Cargo"))?;
        let rustc = crate::identity::rustc_version(None)
            .map_err(|_| PreparationError::Skip("compiler unavailable"))?;
        let target = invocation
            .target
            .clone()
            .or_else(|| {
                std::env::var("CARGO_BUILD_TARGET")
                    .ok()
                    .filter(|value| !value.is_empty())
            })
            .or_else(|| {
                rustc
                    .lines()
                    .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
            })
            .ok_or(PreparationError::Skip("target unavailable"))?;
        let compiler = std::env::var_os("RUSTC");
        let producer = producer(
            ProducerFacts {
                cargo_version: &cargo,
                rustc_version: &rustc,
                rustc: compiler.as_deref(),
                target: &target,
                recipe: invocation.profile.recipe_key(),
                backend: store.backend.tag(),
                cargo_home: &cargo_home,
                command: &invocation.command,
            },
            context,
        )
        .map_err(PreparationError::Skip)?;
        let cache_path = store
            .pool
            .join(oxide_storage::lease::MANAGED_NAME)
            .join("seeds/sources.json");
        let cache = SourceCache::read(&cache_path, &held.store_id);
        Self::begin(producer, lock, cargo_home, cache).map_err(|reason| {
            if reason == "git source verification closure unknown" {
                PreparationError::Refuse(reason.to_owned())
            } else {
                PreparationError::Skip(reason)
            }
        })
    }

    pub fn begin(
        producer: oxide_storage::lease::managed::SeedProducer,
        lock: crate::seed_lock::Analysis,
        cargo_home: PathBuf,
        mut cache: SourceCache,
    ) -> Result<Self, &'static str> {
        let before = verify_pass(&cargo_home, &lock.eligible, &mut cache)?;
        Ok(Self {
            producer,
            lock,
            cargo_home,
            cache,
            before,
            started: SystemTime::now(),
            started_ms: crate::process::now_ms(),
        })
    }

    /// Attempt seeding using the same lock, context and verified Cargo home
    /// that will be used for producing evidence. Store::seed_verified owns
    /// the single admission lifetime and re-verifies sources inside it.
    pub fn seed(
        &mut self,
        store: &crate::store::Store,
        held: &crate::store::Lease,
    ) -> Result<crate::store::SeedRun, String> {
        store.seed_verified(
            held,
            &self.producer,
            &self.lock,
            &self.cargo_home,
            &mut self.cache,
        )
    }

    /// Called only after positive writer closure. Every failure here (an
    /// unreadable tree, a sidecar cap, a lost record) writes no catalog and
    /// returns: the build that already succeeded keeps its result and its
    /// trusted origin, and the slot simply never donates (§4.4, §9.1).
    /// Incomplete source evidence makes individual packages ineligible.
    pub fn publish(
        mut self,
        store: &crate::store::Store,
        held: &crate::store::Lease,
        successful: bool,
        origin: Option<&crate::freshness::Built>,
        profile: &str,
    ) {
        if !successful || origin.is_none() {
            return;
        }
        let Ok(after) = verify_pass(&self.cargo_home, &self.lock.eligible, &mut self.cache) else {
            return;
        };
        let _ = store.publish_seed_catalog(
            held,
            self.producer,
            &self.lock,
            &self.before,
            &after,
            self.started,
            self.started_ms,
            successful,
            origin,
            profile,
            &self.cache,
        );
    }
}

fn verify_pass(
    cargo_home: &Path,
    packages: &[Package],
    cache: &mut SourceCache,
) -> Result<Vec<VerifiedSource>, &'static str> {
    let mut pass = Pass::new(Duration::from_secs(4));
    let hints = pass
        .discover_git_hints(cargo_home, packages)
        .unwrap_or_default();
    pass.locked(
        cargo_home,
        packages,
        &hints,
        cache,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock before Unix epoch")?
            .as_secs(),
    )
}

fn git_hint<'a>(cargo_home: &Path, package: &Package, hints: &'a [GitHint]) -> Option<&'a GitHint> {
    let base = cargo_home.join("git/checkouts");
    let mut found = None;
    for hint in hints.iter().filter(|hint| hint.package == *package) {
        let path = &hint.checkout;
        // Hints are required to be canonical: otherwise a link in a checkout
        // path could silently redirect Git outside the selected Cargo home.
        let canonical = std::fs::canonicalize(path).ok()?;
        let package_root = std::fs::canonicalize(&hint.package_root).ok()?;
        if canonical != *path
            || !canonical.starts_with(&base)
            || canonical == base
            || package_root != hint.package_root
            || !package_root.starts_with(&canonical)
        {
            return None;
        }
        if found.replace(hint).is_some() {
            return None;
        }
    }
    found
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

    /// Discover candidate Cargo Git packages without following links or
    /// reading a donor slot. These remain hints until `locked` verifies the
    /// checkout's commit, content and manifest.
    pub fn discover_git_hints(
        &mut self,
        cargo_home: &Path,
        packages: &[Package],
    ) -> Result<Vec<GitHint>, &'static str> {
        if packages.len() > 2048
            || std::fs::canonicalize(cargo_home).ok().as_deref() != Some(cargo_home)
        {
            return Err("unresolved Cargo home");
        }
        if !packages.iter().any(|package| {
            package
                .source
                .as_deref()
                .is_some_and(|s| s.starts_with("git+"))
        }) {
            return Ok(Vec::new());
        }
        let base = cargo_home.join("git/checkouts");
        let meta = match std::fs::symlink_metadata(&base) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("unreadable Git checkouts"),
        };
        if !meta.is_dir() || std::fs::canonicalize(&base).ok().as_deref() != Some(base.as_path()) {
            return Err("unsafe Git checkouts");
        }
        let mut checkouts = Vec::new();
        for repository in self.directories(&base, meta.dev())? {
            for checkout in self.directories(&repository, meta.dev())? {
                checkouts.push(checkout);
                if checkouts.len() > 2048 {
                    return Err("source cap");
                }
            }
        }
        let mut hints = Vec::new();
        for checkout in checkouts {
            let mut pending = vec![(checkout.clone(), 0usize)];
            while let Some((directory, depth)) = pending.pop() {
                if self.limits().is_none() || depth > 64 {
                    return Err("source cap");
                }
                for package in packages.iter().filter(|package| {
                    package
                        .source
                        .as_deref()
                        .is_some_and(|s| s.starts_with("git+"))
                }) {
                    if self.limits().is_none() {
                        return Err("source cap");
                    }
                    if manifest_matches(&directory, package).is_some() {
                        if hints.len() == 2048 {
                            return Err("source cap");
                        }
                        hints.push(GitHint {
                            package: package.clone(),
                            checkout: checkout.clone(),
                            package_root: directory.clone(),
                        });
                    }
                }
                for child in self.directories(&directory, meta.dev())?.into_iter().rev() {
                    if child.file_name() != Some(std::ffi::OsStr::new(".git")) {
                        pending.push((child, depth + 1));
                    }
                }
            }
        }
        Ok(hints)
    }

    fn directories(&mut self, parent: &Path, device: u64) -> Result<Vec<PathBuf>, &'static str> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(parent).map_err(|_| "unreadable Git checkouts")? {
            if self.limits().is_none() {
                return Err("source cap");
            }
            self.entries = self.entries.checked_add(1).ok_or("source cap")?;
            if self.entries > SourceLimits::default().max_entries {
                return Err("source cap");
            }
            let entry = entry.map_err(|_| "unreadable Git checkouts")?;
            let meta =
                std::fs::symlink_metadata(entry.path()).map_err(|_| "unreadable Git checkouts")?;
            if meta.is_dir() && meta.dev() == device && meta.uid() == unsafe { libc::geteuid() } {
                result.push(entry.path());
            }
        }
        result.sort();
        Ok(result)
    }

    /// Verify eligible locked sources in lock order under one shared pass
    /// budget. Missing, ambiguous or unverifiable sources are omitted; an
    /// unconfirmed Git child closure refuses the entire operation.
    pub fn locked(
        &mut self,
        cargo_home: &Path,
        packages: &[Package],
        git_hints: &[GitHint],
        cache: &mut SourceCache,
        now: u64,
    ) -> Result<Vec<VerifiedSource>, &'static str> {
        if packages.len() > 2048 || git_hints.len() > 2048 {
            return Err("source cap");
        }
        if std::fs::canonicalize(cargo_home).ok().as_deref() != Some(cargo_home) {
            return Err("unresolved Cargo home");
        }
        let mut verified = Vec::new();
        for package in packages {
            if self.limits().is_none() {
                break;
            }
            let source = match package.source.as_deref() {
                Some(source) if source.starts_with("git+") => {
                    match git_hint(cargo_home, package, git_hints) {
                        Some(hint) => self.git(&hint.checkout, &hint.package_root, package)?,
                        None => None,
                    }
                }
                _ => self.registry(cargo_home, package, cache, now),
            };
            if let Some(source) = source {
                verified.push(source);
            }
        }
        Ok(verified)
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
        let Ok(root_metadata) = std::fs::symlink_metadata(root) else {
            return false;
        };
        let uid = unsafe { libc::geteuid() };
        if !root_metadata.is_dir() || root_metadata.uid() != uid {
            return false;
        }
        let device = root_metadata.dev();
        let mut pending = vec![(root.to_path_buf(), 0usize)];
        while let Some((directory, depth)) = pending.pop() {
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
                if metadata.uid() != uid || metadata.dev() != device {
                    return false;
                }
                if metadata.is_dir() {
                    if depth >= SourceLimits::default().max_depth {
                        return false;
                    }
                    pending.push((entry.path(), depth + 1));
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
            verification_root: source.tree().to_path_buf(),
            proc_macro,
        })
    }

    pub fn git(
        &mut self,
        checkout: &Path,
        package_root: &Path,
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
        let verified = match self.git.verify(checkout, revision, &limits) {
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
        let Some(proc_macro) = manifest_matches(package_root, package) else {
            return Ok(None);
        };
        Ok(Some(VerifiedSource {
            package: package.clone(),
            tree_digest: verified.digest.to_hex(),
            kind: SeedSourceKind::Git,
            root: package_root.to_path_buf(),
            verification_root: checkout.to_path_buf(),
            proc_macro,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let template = std::env::temp_dir().join("oxide-seed-hints.XXXXXX");
            let mut name = CString::new(template.as_os_str().as_bytes())
                .unwrap()
                .into_bytes_with_nul();
            // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
            assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
            let name = CStr::from_bytes_until_nul(&name).unwrap();
            Self(std::fs::canonicalize(PathBuf::from(OsStr::from_bytes(name.to_bytes()))).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn git_hints_must_be_unique_canonical_cargo_checkouts() {
        let fixture = Fixture::new();
        let package = crate::seed_lock::test_git_package(
            "sample",
            "1",
            "https://example.invalid/r",
            &"a".repeat(40),
        );
        let root = fixture.0.join("git/checkouts/repo/revision");
        std::fs::create_dir_all(&root).unwrap();
        let hint = || GitHint {
            package: package.clone(),
            checkout: root.clone(),
            package_root: root.clone(),
        };
        assert_eq!(
            git_hint(&fixture.0, &package, &[hint()]).map(|value| value.checkout.clone()),
            Some(root.clone())
        );
        assert!(git_hint(&fixture.0, &package, &[hint(), hint()]).is_none());
        let outside = GitHint {
            package: package.clone(),
            checkout: fixture.0.clone(),
            package_root: fixture.0.clone(),
        };
        assert!(git_hint(&fixture.0, &package, &[outside]).is_none());
        let alias = fixture.0.join("git/checkouts/alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let linked = GitHint {
            package: package.clone(),
            checkout: alias,
            package_root: root.clone(),
        };
        assert!(git_hint(&fixture.0, &package, &[linked]).is_none());
    }

    #[test]
    fn discovery_finds_nested_packages_and_ignores_symlinked_checkouts() {
        let fixture = Fixture::new();
        let package = crate::seed_lock::test_git_package(
            "sample",
            "1",
            "https://example.invalid/r",
            &"a".repeat(40),
        );
        let checkout = fixture.0.join("git/checkouts/repo/revision");
        let root = checkout.join("member");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname='sample'\nversion='1'\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::os::unix::fs::symlink(&checkout, fixture.0.join("git/checkouts/repo/alias")).unwrap();
        let mut pass = Pass::new(Duration::from_secs(4));
        let hints = pass.discover_git_hints(&fixture.0, &[package]).unwrap();
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].checkout, checkout);
        assert_eq!(hints[0].package_root, root);
    }

    #[test]
    fn manifest_matches_honours_crate_types_and_proc_macro() {
        let fixture = Fixture::new();
        let package = crate::seed_lock::eligible(
            b"version = 4\n[[package]]\nname = 'sample'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'\n",
        )
        .unwrap()
        .remove(0);
        let root = fixture.0.join("sample");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        let with = |lib: &str| {
            std::fs::write(
                root.join("Cargo.toml"),
                format!("[package]\nname='sample'\nversion='1'\n{lib}"),
            )
            .unwrap();
            manifest_matches(&root, &package)
        };
        // A plain library is `lib` (rustc's rlib) or `rlib`.
        assert_eq!(with(""), Some(false));
        std::fs::remove_file(root.join("src/lib.rs")).unwrap();
        std::os::unix::fs::symlink(root.join("Cargo.toml"), root.join("src/lib.rs")).unwrap();
        assert_eq!(with(""), None);
        std::fs::remove_file(root.join("src/lib.rs")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        assert_eq!(with("[lib]\ncrate-type=['rlib']\n"), Some(false));
        // Any other crate type emits outputs outside the pinned layout.
        assert_eq!(with("[lib]\ncrate-type=['cdylib']\n"), None);
        assert_eq!(with("[lib]\ncrate-type=['lib','rlib']\n"), None);
        // A proc macro is classified as one, so v1 never offers it.
        assert_eq!(with("[lib]\nproc-macro=true\n"), Some(true));
        assert_eq!(
            with("[lib]\ncrate-type=['proc-macro']\nproc-macro=true\n"),
            Some(true)
        );
        assert_eq!(with("[lib]\ncrate-type=['proc-macro']\n"), Some(true));
        // A proc-macro flag with a conflicting crate type is not a shape
        // Cargo would build, so it is never classified.
        assert_eq!(
            with("[lib]\nproc-macro=true\ncrate-type=['cdylib']\n"),
            None
        );
    }

    #[test]
    fn unchanged_source_refuses_excessive_depth_and_symlinks() {
        let fixture = Fixture::new();
        let mut path = fixture.0.clone();
        for _ in 0..=SourceLimits::default().max_depth {
            path.push("d");
            std::fs::create_dir(&path).unwrap();
        }
        let mut pass = Pass::new(Duration::from_secs(4));
        assert!(!pass.unchanged_since(&fixture.0, SeedSourceKind::Registry, SystemTime::now()));
        std::fs::remove_dir_all(fixture.0.join("d")).unwrap();
        std::os::unix::fs::symlink(&fixture.0, fixture.0.join("link")).unwrap();
        let mut pass = Pass::new(Duration::from_secs(4));
        assert!(!pass.unchanged_since(&fixture.0, SeedSourceKind::Registry, SystemTime::now()));
    }

    #[test]
    fn empty_producing_source_pass_is_read_only() {
        let fixture = Fixture::new();
        let mut cache = SourceCache::new("store-1").unwrap();
        assert!(verify_pass(&fixture.0, &[], &mut cache).unwrap().is_empty());
        assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 0);
    }
}
