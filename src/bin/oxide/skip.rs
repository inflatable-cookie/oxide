//! `oxide run` no-op fast path.
//!
//! After a successful leased `oxide run` build, Oxide records, per canonical
//! working directory and modelled build arguments, a file-metadata tuple
//! (device, inode, size, mtime, ctime, or absent) for every input it knows
//! Cargo considers: the final binary's documented `.d` dep-info, every
//! participating manifest, `Cargo.lock`, every Cargo config location, the
//! toolchain-resolution inputs and the resolved toolchain binaries, plus the
//! built binary and a digest of the full environment. A later `oxide run`
//! whose tuples all match exactly, with no Oxide command active on the
//! checkout, executes that binary in-process: no Cargo, rustc, rustup, Git or
//! shell child. Any doubt falls through to the normal leased path, which
//! records again on success.
//!
//! Concurrency matches Cargo's: the skip sweeps the tuples, then executes. A
//! write landing after the sweep is caught by the next `oxide run`.

use crate::cli::{Profile, PROFILE_ENVIRONMENT};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Version 4: file-metadata tuples. Records of any other version never skip.
const RECORD_VERSION: u32 = 4;
/// Records live in the checkout's private target directory, so `oxide clean`
/// drops them with the binaries they describe.
const RECORD_DIR: &str = "target/oxide-run-skip";
/// Every leased command on the checkout holds this shared; the skip and a
/// publishing `run` take it exclusively without waiting.
const ACTIVITY_LOCK: &str = "active.lock";
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;
/// Past this many records the directory is cleared before publishing.
const MAX_RECORDS: usize = 64;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_DEP_INFO_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SCRIPT_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INPUTS: usize = 32_768;
const MAX_ANCESTORS: usize = 64;
const MAX_PATH_ENTRIES: usize = 256;
const STRATEGY: &str = "build+execute";
const NANOS: i128 = 1_000_000_000;
/// Files Cargo or rustup read from each directory between the working
/// directory and `/`: config, workspace discovery and toolchain overrides.
const ANCESTOR_FILES: [&str; 5] = [
    ".cargo/config",
    ".cargo/config.toml",
    "Cargo.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
];
/// Config text naming another input source or compiler. The key does not
/// follow them, so a config file containing one records nothing.
const UNFOLLOWED_CONFIG: [&str; 4] = ["include", "replace-with", "paths", "rustc"];
/// A compiler or wrapper chosen by the environment, outside the toolchain
/// the key resolves.
const UNFOLLOWED_ENVIRONMENT: [&str; 6] = [
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
];
/// Toolchain files under the resolved sysroot. `bin/rustc` and `bin/cargo`
/// must exist; the rest are recorded present or absent.
const TOOLCHAIN_FILES: [&str; 5] = [
    "bin/rustc",
    "bin/cargo",
    "lib",
    "lib/rustlib/multirust-channel-manifest.toml",
    "lib/rustlib/components",
];

/// An `oxide run` whose build arguments Oxide models.
pub struct Request {
    /// Build-affecting arguments before `--`, in order, without `--rebuild`.
    args: Vec<String>,
    pub program_args: Vec<OsString>,
    pub profile: Profile,
    /// `--rebuild` or `OXIDE_NO_SKIP`: Cargo runs, and the key is recorded.
    pub forced: bool,
}

impl Request {
    /// `None` when any argument before `--` is outside the modelled set.
    pub fn parse(args: &[OsString]) -> Option<Self> {
        let mut key = Vec::new();
        let mut program_args = Vec::new();
        let mut profile = Profile::Quick;
        let mut forced = std::env::var_os("OXIDE_NO_SKIP").is_some_and(|value| !value.is_empty());
        let mut iter = args.iter();
        while let Some(argument) = iter.next() {
            let text = argument.to_str()?;
            if text == "--" {
                program_args.extend(iter.by_ref().cloned());
                break;
            }
            match text {
                "--rebuild" => forced = true,
                "--all-features" | "--no-default-features" => key.push(text.to_owned()),
                "--release" | "-r" => {
                    profile = Profile::Release;
                    key.push(text.to_owned());
                }
                "--bin" | "--example" | "-p" | "--package" | "--features" | "-F" | "--target"
                | "--profile" => {
                    let value = iter.next()?.to_str()?;
                    if text == "--profile" {
                        profile = profile_named(value)?;
                    }
                    key.push(text.to_owned());
                    key.push(value.to_owned());
                }
                _ => {
                    let (flag, value) = text.split_once('=')?;
                    if !matches!(
                        flag,
                        "--bin"
                            | "--example"
                            | "--package"
                            | "--features"
                            | "--target"
                            | "--profile"
                    ) {
                        return None;
                    }
                    if flag == "--profile" {
                        profile = profile_named(value)?;
                    }
                    key.push(text.to_owned());
                }
            }
        }
        Some(Self {
            args: key,
            program_args,
            profile,
            forced,
        })
    }

    fn record_name(&self, cwd: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(cwd.as_os_str().as_bytes());
        for argument in &self.args {
            hasher.update(b"\0");
            hasher.update(argument.as_bytes());
        }
        format!("{}.json", hex(&hasher.finalize()))
    }
}

fn profile_named(value: &str) -> Option<Profile> {
    match value {
        "quick" => Some(Profile::Quick),
        "debug" => Some(Profile::Debug),
        "release" => Some(Profile::Release),
        _ => None,
    }
}

/// One file's metadata identity, following symbolic links.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stat {
    device: u64,
    inode: u64,
    size: u64,
    mtime_ns: i128,
    ctime_ns: i128,
    regular: bool,
}

impl Stat {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            mtime_ns: i128::from(metadata.mtime()) * NANOS + i128::from(metadata.mtime_nsec()),
            ctime_ns: i128::from(metadata.ctime()) * NANOS + i128::from(metadata.ctime_nsec()),
            regular: metadata.is_file(),
        }
    }

    /// Both times strictly before `started_ns`. A whole-second time may come
    /// from a filesystem that truncates, so it must precede the start's
    /// whole second.
    fn before(&self, started_ns: i128) -> bool {
        [self.mtime_ns, self.ctime_ns].into_iter().all(|time| {
            let bound = if time.rem_euclid(NANOS) == 0 {
                started_ns - started_ns.rem_euclid(NANOS)
            } else {
                started_ns
            };
            time < bound
        })
    }
}

/// `None`: the path is absent. Any other read error refuses.
fn stat(path: &Path) -> Result<Option<Stat>, String> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(Stat::of(&metadata))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot stat {}: {error}", path.display())),
    }
}

/// One recorded input: its path as Cargo or rustup names it, and its tuple
/// (`null`: absent).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    stat: Option<Stat>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    cwd: PathBuf,
    args: Vec<String>,
    checkout: PathBuf,
    strategy: String,
    /// SHA-256 over every variable name and its value's SHA-256.
    environment: String,
    started_ns: i128,
    inputs: Vec<Input>,
    binary: PathBuf,
    binary_stat: Stat,
    manifest_dir: PathBuf,
}

/// Execute the recorded binary when the key matches exactly. Returns only
/// when the skip does not apply or the program cannot start.
pub fn try_skip(request: &Request) -> Result<(), String> {
    if request.forced {
        return Ok(());
    }
    let Some(record) = matching_record(request) else {
        return Ok(());
    };
    eprintln!(
        "oxide: {} profile, {STRATEGY} strategy, unchanged — skipped Cargo (`oxide run --rebuild` or OXIDE_NO_SKIP=1 forces Cargo)",
        request.profile.label(),
    );
    let error = Command::new(&record.binary)
        .args(&request.program_args)
        .env("CARGO_MANIFEST_DIR", &record.manifest_dir)
        .exec();
    Err(format!(
        "could not execute {}: {error}",
        record.binary.display()
    ))
}

fn matching_record(request: &Request) -> Option<Record> {
    let cwd = fs::canonicalize(std::env::current_dir().ok()?).ok()?;
    let name = request.record_name(&cwd);
    let (checkout, record) = cwd
        .ancestors()
        .take(MAX_ANCESTORS)
        .find_map(|dir| Some((dir, read_record(&dir.join(RECORD_DIR).join(&name))?)))?;
    if record.version != RECORD_VERSION
        || record.cwd != cwd
        || record.args != request.args
        || record.checkout != checkout
        || record.strategy != STRATEGY
        || record.inputs.is_empty()
        || environment().ok()? != record.environment
    {
        return None;
    }
    // No Oxide command may be active on the checkout during the sweep.
    let lock = File::open(checkout.join(RECORD_DIR).join(ACTIVITY_LOCK)).ok()?;
    try_exclusive(&lock).then_some(())?;
    for input in &record.inputs {
        if stat(&input.path).ok()? != input.stat {
            return None;
        }
    }
    let binary = stat(&record.binary).ok()??;
    (binary.regular && binary == record.binary_stat).then_some(record)
}

fn read_record(path: &Path) -> Option<Record> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn try_exclusive(file: &File) -> bool {
    loop {
        // SAFETY: the descriptor is open for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return true;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// A leased command's shared hold on its checkout's activity lock, released
/// when dropped.
pub struct Activity(File);

impl Activity {
    /// Wait for any skip sweep or publishing `run`, then hold the lock
    /// shared until the command's lease is released.
    pub fn hold(checkout: &Path) -> Result<Self, String> {
        let dir = checkout.join(RECORD_DIR);
        fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let path = dir.join(ACTIVITY_LOCK);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
        loop {
            // SAFETY: the descriptor is open for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } == 0 {
                return Ok(Self(file));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(format!("cannot lock {}: {error}", path.display()));
            }
        }
    }

    /// Convert to exclusive without waiting: `false` when another Oxide
    /// command is active on the checkout.
    pub fn exclusive(&self) -> bool {
        try_exclusive(&self.0)
    }
}

/// What one `run` build reported: every `compiler-artifact` package and
/// every `build-script-executed` output directory, fresh units included.
#[derive(Debug, Default)]
pub struct Build {
    pub packages: BTreeSet<Package>,
    /// `package_id` to `out_dir`.
    pub scripts: BTreeMap<String, PathBuf>,
}

/// One package from a `compiler-artifact` message of the build.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Package {
    pub id: String,
    pub manifest: PathBuf,
    /// The message was the package's build-script compile unit.
    pub build_script: bool,
}

/// Key inputs observed before Cargo runs, published only after it succeeds.
pub struct Pending {
    cwd: PathBuf,
    checkout: PathBuf,
    path: PathBuf,
    args: Vec<String>,
    environment: String,
    /// Toolchain resolution, the resolved toolchain, config and lock: bound
    /// before Cargo and verified unchanged after it.
    observed: BTreeMap<PathBuf, Option<Stat>>,
    started_ns: i128,
}

impl Pending {
    /// Drop this key's record, then observe the pre-build inputs. An error:
    /// no record will be published for this command.
    pub fn capture(request: &Request, checkout: &Path) -> Result<Self, String> {
        let cwd = std::env::current_dir()
            .and_then(fs::canonicalize)
            .map_err(|error| format!("cannot resolve the working directory: {error}"))?;
        let checkout = fs::canonicalize(checkout).map_err(|error| error.to_string())?;
        let path = checkout.join(RECORD_DIR).join(request.record_name(&cwd));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot drop the old record: {error}")),
        }
        if !cwd.starts_with(&checkout) {
            return Err("the working directory is outside the workspace".to_owned());
        }
        let environment = environment()?;
        // Resolution inputs bracket the toolchain query: any change between
        // the two observations leaves the resolved toolchain ambiguous.
        let resolution = resolution_inputs(&cwd)?;
        let sysroot = sysroot(&cwd).ok_or("the toolchain sysroot did not resolve")?;
        if resolution_inputs(&cwd)? != resolution {
            return Err("toolchain resolution changed during its query".to_owned());
        }
        let mut observed = resolution;
        for name in TOOLCHAIN_FILES {
            let file = sysroot.join(name);
            let found = stat(&file)?;
            if name.starts_with("bin/") && !found.is_some_and(|stat| stat.regular) {
                return Err(format!("{} is not a regular file", file.display()));
            }
            observed.insert(file, found);
        }
        let lock = checkout.join("Cargo.lock");
        observed.insert(lock.clone(), stat(&lock)?);
        Ok(Self {
            cwd,
            checkout,
            path,
            args: request.args.clone(),
            environment,
            observed,
            started_ns: now_ns().ok_or("the clock is before 1970")?,
        })
    }

    /// Record the key for a successful build of `binary`. Runs while the
    /// command still holds its lease and the checkout's activity lock
    /// exclusively. Publishes nothing when an input changed or is not older
    /// than the build start, or the build reaches outside what the key
    /// observes.
    pub fn publish(self, binary: &Path, manifest_dir: &Path, build: &Build) -> Result<(), String> {
        if self.environment != environment()? {
            return Err("the environment changed".to_owned());
        }
        for (path, found) in &self.observed {
            if stat(path)? != *found {
                return Err(format!("{} changed while Cargo ran", path.display()));
            }
        }
        // Inputs known only from the build: each must be a regular file.
        let mut built = dep_info_paths(binary)?;
        built.insert(self.checkout.join("Cargo.toml"));
        built.extend(participating_manifests(build)?);
        // Cross-check: every manifest between a listed source and the
        // checkout, so a manifest the build messages left out still counts.
        let sources: Vec<PathBuf> = built.iter().cloned().collect();
        for source in &sources {
            for dir in source
                .ancestors()
                .skip(1)
                .take_while(|dir| dir.starts_with(&self.checkout))
            {
                let manifest = dir.join("Cargo.toml");
                if stat(&manifest)?.is_some() {
                    built.insert(manifest);
                }
            }
        }
        let mut inputs = self.observed.clone();
        for path in built {
            if inputs.contains_key(&path) {
                continue;
            }
            let found = stat(&path)?.ok_or_else(|| format!("{} is missing", path.display()))?;
            if !found.regular {
                return Err(format!("{} is not a regular file", path.display()));
            }
            inputs.insert(path, Some(found));
            if inputs.len() > MAX_INPUTS {
                return Err("the build lists too many inputs".to_owned());
            }
        }
        let binary = fs::canonicalize(binary).map_err(|error| error.to_string())?;
        if inputs.contains_key(&binary) {
            return Err("the binary is one of its own inputs".to_owned());
        }
        let mut recorded = Vec::with_capacity(inputs.len());
        for (path, found) in inputs {
            if found.is_some_and(|found| !found.before(self.started_ns)) {
                return Err(format!(
                    "{} is not older than the build start",
                    path.display()
                ));
            }
            recorded.push(Input { path, stat: found });
        }
        let binary_stat = stat(&binary)?.ok_or("the binary is missing")?;
        if !binary_stat.regular {
            return Err("the binary is not a regular file".to_owned());
        }
        let record = Record {
            version: RECORD_VERSION,
            cwd: self.cwd,
            args: self.args,
            checkout: self.checkout,
            strategy: STRATEGY.to_owned(),
            environment: self.environment,
            started_ns: self.started_ns,
            inputs: recorded,
            binary,
            binary_stat,
            manifest_dir: manifest_dir.to_path_buf(),
        };
        write_record(&self.path, &record)
    }
}

fn write_record(path: &Path, record: &Record) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("the record exceeds its size bound".to_owned());
    }
    let dir = path.parent().ok_or("record path has no parent")?;
    let count = fs::read_dir(dir)
        .map_err(|error| error.to_string())?
        .count();
    if count >= MAX_RECORDS {
        for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?.path();
            if entry.extension().is_some_and(|extension| extension == "json") {
                let _ = fs::remove_file(entry);
            }
        }
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let written = File::create(&temporary)
        .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

fn now_ns() -> Option<i128> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i128::try_from(elapsed.as_nanos()).ok()
}

/// The toolchain's sysroot as rustup resolves it here. Normal path only.
fn sysroot(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    fs::canonicalize(text.trim()).ok()
}

/// SHA-256 over every variable's name and its value's SHA-256, plus Oxide's
/// own profile definitions, which reach Cargo through the environment. Raw
/// values are never stored.
fn environment() -> Result<String, String> {
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if let Some((name, _)) = variables.iter().find(|(name, _)| {
        UNFOLLOWED_ENVIRONMENT
            .iter()
            .any(|unfollowed| name == unfollowed)
    }) {
        return Err(format!("{} is set", name.to_string_lossy()));
    }
    variables.sort();
    let mut digest = Sha256::new();
    for (name, value) in &variables {
        digest.update(name.as_bytes());
        digest.update(b"=");
        digest.update(Sha256::digest(value.as_bytes()));
        digest.update(b"\0");
    }
    for (name, value) in PROFILE_ENVIRONMENT {
        digest.update(format!("oxide:{name}={value}\0").as_bytes());
    }
    Ok(hex(&digest.finalize()))
}

/// What decides the toolchain and Cargo config here, present or absent:
/// each ancestor's config, manifest and toolchain files, Cargo home config,
/// rustup settings, and every `PATH` candidate for `cargo` and `rustc` up to
/// the one that resolves. Config text naming an unfollowed input refuses.
fn resolution_inputs(cwd: &Path) -> Result<BTreeMap<PathBuf, Option<Stat>>, String> {
    let mut inputs = BTreeMap::new();
    let mut configs = Vec::new();
    let ancestors: Vec<&Path> = cwd.ancestors().collect();
    if ancestors.len() > MAX_ANCESTORS {
        return Err("the working directory is too deep".to_owned());
    }
    for dir in ancestors {
        for name in ANCESTOR_FILES {
            let path = dir.join(name);
            if name.starts_with(".cargo/") {
                configs.push(path.clone());
            }
            inputs.insert(path.clone(), stat(&path)?);
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".cargo")))
        .ok_or("neither CARGO_HOME nor HOME is set")?;
    for name in ["config", "config.toml"] {
        let path = cargo_home.join(name);
        configs.push(path.clone());
        inputs.insert(path.clone(), stat(&path)?);
    }
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".rustup")))
        .ok_or("neither RUSTUP_HOME nor HOME is set")?;
    let settings = rustup_home.join("settings.toml");
    inputs.insert(settings.clone(), stat(&settings)?);
    for program in ["cargo", "rustc"] {
        path_candidates(cwd, program, &mut inputs)?;
    }
    for config in configs {
        if inputs.get(&config).copied().flatten().is_some() {
            check_config(&config)?;
        }
    }
    Ok(inputs)
}

/// Record `PATH` lookups for `program` until one resolves to a regular
/// executable file. An empty or relative entry resolves against the working
/// directory, which the key already names.
fn path_candidates(
    cwd: &Path,
    program: &str,
    inputs: &mut BTreeMap<PathBuf, Option<Stat>>,
) -> Result<(), String> {
    let path = std::env::var_os("PATH").ok_or("PATH is not set")?;
    for dir in std::env::split_paths(&path).take(MAX_PATH_ENTRIES) {
        let candidate = cwd.join(dir).join(program);
        let found = stat(&candidate)?;
        inputs.insert(candidate.clone(), found);
        if found.is_some_and(|stat| stat.regular) {
            let mode = fs::metadata(&candidate)
                .map_err(|error| error.to_string())?
                .mode();
            if mode & 0o111 != 0 {
                return Ok(());
            }
        }
    }
    Err(format!("{program} does not resolve on PATH"))
}

fn check_config(path: &Path) -> Result<(), String> {
    let file =
        File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    let text = String::from_utf8_lossy(&bytes);
    match UNFOLLOWED_CONFIG.iter().find(|word| text.contains(*word)) {
        Some(word) => Err(format!("{} names `{word}`", path.display())),
        None => Ok(()),
    }
}

/// The manifests the key must cover, and the packages it cannot.
///
/// Pinned Cargo 1.97.1 (c980f4866) follows a registry or Git package's
/// sources through its source identity rather than file metadata: the
/// compile call site passes the unit's local-source classification as
/// `translate_dep_info`'s `allow_package`
/// (`core/compiler/mod.rs` L454-L465), which skips package-relative files
/// for non-local units (`fingerprint/dep_info.rs` L262-L267), and
/// `output_depinfo` leaves them out of the final `.d` "under the assumption
/// that changes to them can be detected via changes to `Cargo.lock`". The key
/// records `Cargo.lock`, so such a package's sources are covered as Cargo
/// covers them. Its build script is not: Cargo follows the script's
/// `rerun-if-changed` paths wherever they point, and no documented output
/// reports them, so a non-local package with a build script publishes
/// nothing. A path package's build script must declare what it follows
/// (`rerun-if-changed` or `rerun-if-env-changed`); without either, Cargo
/// fingerprints the whole package listing, which the key does not observe.
/// Any other source kind publishes nothing.
fn participating_manifests(build: &Build) -> Result<Vec<PathBuf>, String> {
    if build.packages.is_empty() {
        return Err("the build's package list is missing".to_owned());
    }
    let mut manifests = Vec::new();
    for package in &build.packages {
        let id = &package.id;
        if id.starts_with("path+file://") {
            if !package.manifest.is_absolute() {
                return Err(format!("{id} reported a relative manifest path"));
            }
            manifests.push(package.manifest.clone());
            if package.build_script {
                let out_dir = build
                    .scripts
                    .get(id)
                    .ok_or_else(|| format!("{id} built a build script that reported no run"))?;
                declared_inputs(out_dir)?;
            }
        } else if !(id.starts_with("registry+")
            || id.starts_with("sparse+")
            || id.starts_with("git+"))
        {
            return Err(format!("{id} has a source kind the key does not model"));
        } else if package.build_script || build.scripts.contains_key(id) {
            return Err(format!(
                "{id} has a build script whose rerun-if-changed inputs Cargo follows and the key cannot observe"
            ));
        }
    }
    Ok(manifests)
}

/// A build script's saved output (`output`, beside its `out_dir`) declares
/// `rerun-if-changed` or `rerun-if-env-changed`.
fn declared_inputs(out_dir: &Path) -> Result<(), String> {
    let output = out_dir
        .parent()
        .ok_or("a build script out_dir has no parent")?
        .join("output");
    let file = File::open(&output)
        .map_err(|error| format!("cannot read {}: {error}", output.display()))?;
    let mut text = String::new();
    file.take(MAX_SCRIPT_OUTPUT_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read {}: {error}", output.display()))?;
    if text.len() as u64 > MAX_SCRIPT_OUTPUT_BYTES {
        return Err("a build script output exceeds its size bound".to_owned());
    }
    let declares = text.lines().any(|line| {
        let directive = line
            .strip_prefix("cargo::")
            .or_else(|| line.strip_prefix("cargo:"));
        directive.is_some_and(|directive| {
            directive.starts_with("rerun-if-changed=")
                || directive.starts_with("rerun-if-env-changed=")
        })
    });
    if declares {
        Ok(())
    } else {
        Err("a path package's build script declares no rerun inputs".to_owned())
    }
}

/// Every file in the binary's Cargo dep-info (`<binary>.d`, documented next
/// to each artifact), as Cargo names it. Each must be absolute; regular-file
/// checks follow when the paths are stat-recorded.
fn dep_info_paths(binary: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let mut dep_info = binary.as_os_str().to_owned();
    dep_info.push(".d");
    let file = File::open(&dep_info).map_err(|error| format!("no dep-info: {error}"))?;
    let mut text = String::new();
    file.take(MAX_DEP_INFO_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read dep-info: {error}"))?;
    if text.len() as u64 > MAX_DEP_INFO_BYTES {
        return Err("dep-info exceeds its size bound".to_owned());
    }
    let mut paths = BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (_, dependencies) = split_rule(line).ok_or("unexpected dep-info line")?;
        for dependency in dependencies {
            let path = PathBuf::from(dependency);
            if !path.is_absolute() {
                return Err(format!("dep-info names relative {}", path.display()));
            }
            paths.insert(path);
            if paths.len() > MAX_INPUTS {
                return Err("dep-info lists too many files".to_owned());
            }
        }
    }
    if paths.is_empty() {
        return Err("dep-info lists no files".to_owned());
    }
    Ok(paths)
}

/// `target: dep dep …` with Makefile `\ ` escapes.
fn split_rule(line: &str) -> Option<(String, Vec<String>)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    let mut target = None;
    while let Some(character) = chars.next() {
        match character {
            '\\' => word.push(chars.next()?),
            ':' if target.is_none() && chars.peek().is_none_or(|next| *next == ' ') => {
                target = Some(std::mem::take(&mut word));
            }
            ' ' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some((target?, words))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn modelled_arguments_form_the_key_and_program_arguments_do_not() {
        let request = Request::parse(&args(&[
            "--bin",
            "app",
            "--features=fast",
            "--profile",
            "debug",
            "--",
            "--rebuild",
            "x",
        ]))
        .expect("modelled");
        assert_eq!(
            request.args,
            ["--bin", "app", "--features=fast", "--profile", "debug"]
        );
        assert_eq!(request.program_args, args(&["--rebuild", "x"]));
        assert_eq!(request.profile, Profile::Debug);
        let other = Request::parse(&args(&[
            "--bin",
            "app",
            "--features=fast",
            "--profile",
            "debug",
        ]))
        .expect("modelled");
        let cwd = Path::new("/w");
        assert_eq!(request.record_name(cwd), other.record_name(cwd));
        assert_ne!(
            request.record_name(cwd),
            other.record_name(Path::new("/w/member"))
        );
    }

    #[test]
    fn rebuild_forces_and_unmodelled_arguments_refuse() {
        let forced = Request::parse(&args(&["--rebuild", "-r"])).expect("modelled");
        assert!(forced.forced);
        assert_eq!(forced.args, ["-r"]);
        assert_eq!(forced.profile, Profile::Release);
        for refused in [
            &["-v"][..],
            &["--manifest-path", "x/Cargo.toml"],
            &["--message-format=json"],
            &["--config", "net.offline=true"],
            &["--locked"],
            &["--profile", "dev"],
            &["--bin"],
        ] {
            assert!(Request::parse(&args(refused)).is_none(), "{refused:?}");
        }
    }

    fn package(id: &str, build_script: bool) -> Package {
        Package {
            id: id.to_owned(),
            manifest: PathBuf::from("/nonexistent/Cargo.toml"),
            build_script,
        }
    }

    #[test]
    fn registry_and_git_packages_are_eligible_without_build_scripts() {
        let eligible = Build {
            packages: [
                package(
                    "registry+https://github.com/rust-lang/crates.io-index#cfg-if@1.0.0",
                    false,
                ),
                package("sparse+https://index.crates.io/#itoa@1.0.15", false),
                package(
                    "git+https://example.invalid/repo?rev=0123abc#helper@0.1.0",
                    false,
                ),
            ]
            .into_iter()
            .collect(),
            scripts: BTreeMap::new(),
        };
        assert_eq!(participating_manifests(&eligible), Ok(Vec::new()));
        for refused in [
            package(
                "registry+https://github.com/rust-lang/crates.io-index#libc@0.2.0",
                true,
            ),
            package("git+https://example.invalid/repo#helper@0.1.0", true),
            package("directory+file:///vendor#cfg-if@1.0.0", false),
            package("path+file:///nonexistent#app@0.1.0", true),
        ] {
            let build = Build {
                packages: [refused].into_iter().collect(),
                scripts: BTreeMap::new(),
            };
            assert!(participating_manifests(&build).is_err(), "{build:?}");
        }
        assert!(participating_manifests(&Build::default()).is_err());
    }

    #[test]
    fn inputs_must_precede_the_build_start() {
        let at = |mtime_ns: i128, ctime_ns: i128| Stat {
            device: 1,
            inode: 1,
            size: 1,
            mtime_ns,
            ctime_ns,
            regular: true,
        };
        let start = 10 * NANOS + 500;
        assert!(at(10 * NANOS + 499, 10 * NANOS + 1).before(start));
        assert!(!at(10 * NANOS + 500, 1).before(start));
        // A restored old mtime is still caught by ctime.
        assert!(!at(1, 10 * NANOS + 501).before(start));
        // A whole-second time may be truncated: it must precede the second.
        assert!(!at(10 * NANOS, 1).before(start));
        assert!(at(9 * NANOS, 1).before(start));
    }

    #[test]
    fn dep_info_rules_unescape_spaces() {
        let (target, dependencies) =
            split_rule("/w/target/app: /w/src/main.rs /w/my\\ dir/lib.rs").expect("rule");
        assert_eq!(target, "/w/target/app");
        assert_eq!(dependencies, ["/w/src/main.rs", "/w/my dir/lib.rs"]);
        assert!(split_rule("no rule here").is_none());
    }
}
