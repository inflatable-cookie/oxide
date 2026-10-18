//! `oxide run` no-op fast path.
//!
//! After a successful `oxide run` build, Oxide records a skip key for the
//! canonical working directory and the modelled build arguments: the full
//! environment, the Cargo config, manifest and toolchain files Cargo and
//! rustup read from the working directory upward, the toolchain's own files,
//! `Cargo.lock`, the checkout's Git content and the built binary's identity.
//! A later `oxide run` whose key matches exactly executes that binary with no
//! Cargo, lease, slot or pool access. Any doubt falls through to the normal
//! leased path, which records again on success.

use crate::cli::{Profile, PROFILE_ENVIRONMENT};
use crate::freshness::{self, Content};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const RECORD_VERSION: u32 = 1;
/// Records live in the checkout's private target directory, so `oxide clean`
/// drops them with the binaries they describe.
const RECORD_DIR: &str = "target/oxide-run-skip";
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
/// Past this many records the directory is cleared before publishing.
const MAX_RECORDS: usize = 64;
const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_DEP_INFO_BYTES: u64 = 4 * 1024 * 1024;
const MAX_DEPENDENCIES: usize = 32_768;
const MAX_ANCESTORS: usize = 64;
const STRATEGY: &str = "build+execute";
/// Files Cargo or rustup read from each directory between the working
/// directory and `/`.
const ANCESTOR_FILES: [&str; 5] = [
    ".cargo/config",
    ".cargo/config.toml",
    "Cargo.toml",
    "rust-toolchain",
    "rust-toolchain.toml",
];
/// Config text naming another input source or compiler. The key does not
/// follow them, so a config file containing one refuses the skip.
const UNFOLLOWED_CONFIG: [&str; 4] = ["include", "replace-with", "paths", "rustc"];
/// A compiler or wrapper chosen by the environment, outside the toolchain
/// files the key observes.
const UNFOLLOWED_ENVIRONMENT: [&str; 6] = [
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
];
/// Toolchain files whose identity stands for the resolved toolchain.
const TOOLCHAIN_FILES: [&str; 4] = [
    "bin/rustc",
    "bin/cargo",
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileIdentity {
    size: u64,
    mtime_ns: i128,
    inode: u64,
    device: u64,
}

impl FileIdentity {
    fn of(metadata: &fs::Metadata) -> Self {
        Self {
            size: metadata.len(),
            mtime_ns: i128::from(metadata.mtime()) * 1_000_000_000
                + i128::from(metadata.mtime_nsec()),
            inode: metadata.ino(),
            device: metadata.dev(),
        }
    }
}

/// The key inputs Oxide can read without Cargo or rustc.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    environment: String,
    files: String,
    lock: String,
    toolchain: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    cwd: PathBuf,
    args: Vec<String>,
    checkout: PathBuf,
    strategy: String,
    sysroot: PathBuf,
    inputs: Inputs,
    top: PathBuf,
    content: Content,
    binary: PathBuf,
    binary_identity: FileIdentity,
    manifest_dir: PathBuf,
}

/// Execute the recorded binary when the key matches exactly. Returns only
/// when the skip does not apply or the program cannot start.
pub fn try_skip(request: &Request) -> Result<(), String> {
    if request.forced {
        return Ok(());
    }
    let started = Instant::now();
    let Some(record) = matching_record(request) else {
        return Ok(());
    };
    eprintln!(
        "oxide: {} profile, {STRATEGY} strategy, unchanged — skipped Cargo, {:.1}s (`oxide run --rebuild` or OXIDE_NO_SKIP=1 forces Cargo)",
        request.profile.label(),
        started.elapsed().as_secs_f64()
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
    {
        return None;
    }
    let binary = fs::symlink_metadata(&record.binary).ok()?;
    if !binary.is_file() || FileIdentity::of(&binary) != record.binary_identity {
        return None;
    }
    if inputs(&cwd, checkout, &record.sysroot).ok()? != record.inputs {
        return None;
    }
    let (top, content) = freshness::snapshot(&cwd).ok()?;
    (top == record.top && content == record.content).then_some(record)
}

fn read_record(path: &Path) -> Option<Record> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Key inputs taken before Cargo runs, published only after it succeeds.
pub struct Pending {
    cwd: PathBuf,
    checkout: PathBuf,
    path: PathBuf,
    args: Vec<String>,
    sysroot: PathBuf,
    inputs: Inputs,
    top: PathBuf,
    content: Content,
}

impl Pending {
    /// Drop this key's record, then take the inputs. `None`: no record will
    /// be published for this command.
    pub fn capture(request: &Request, checkout: &Path) -> Option<Self> {
        let cwd = fs::canonicalize(std::env::current_dir().ok()?).ok()?;
        let checkout = fs::canonicalize(checkout).ok()?;
        let path = checkout.join(RECORD_DIR).join(request.record_name(&cwd));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        if !cwd.starts_with(&checkout) {
            return None;
        }
        let sysroot = sysroot(&cwd)?;
        let inputs = inputs(&cwd, &checkout, &sysroot).ok()?;
        let (top, content) = freshness::snapshot(&cwd).ok()?;
        Some(Self {
            cwd,
            checkout,
            path,
            args: request.args.clone(),
            sysroot,
            inputs,
            top,
            content,
        })
    }

    /// Record the key for a successful build of `binary`. Publishes nothing
    /// when an input changed while Cargo ran or the binary's dependencies
    /// reach outside what the key observes.
    pub fn publish(self, binary: &Path, manifest_dir: &Path) -> Result<(), String> {
        if inputs(&self.cwd, &self.checkout, &self.sysroot)? != self.inputs {
            return Err("an input changed while Cargo ran".to_owned());
        }
        let (top, content) = freshness::snapshot(&self.cwd)?;
        if top != self.top || content != self.content {
            return Err("the checkout changed while Cargo ran".to_owned());
        }
        let binary = fs::canonicalize(binary).map_err(|error| error.to_string())?;
        let metadata = fs::symlink_metadata(&binary).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("the binary is not a regular file".to_owned());
        }
        covered_dependencies(&binary, &self.top, &self.checkout)?;
        let record = Record {
            version: RECORD_VERSION,
            cwd: self.cwd,
            args: self.args,
            checkout: self.checkout,
            strategy: STRATEGY.to_owned(),
            sysroot: self.sysroot,
            inputs: self.inputs,
            top: self.top,
            content: self.content,
            binary,
            binary_identity: FileIdentity::of(&metadata),
            manifest_dir: manifest_dir.to_path_buf(),
        };
        let bytes = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_RECORD_BYTES {
            return Err("the record exceeds its size bound".to_owned());
        }
        let dir = self.path.parent().ok_or("record path has no parent")?;
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        let count = fs::read_dir(dir)
            .map_err(|error| error.to_string())?
            .count();
        if count >= MAX_RECORDS {
            for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
                let _ = fs::remove_file(entry.map_err(|error| error.to_string())?.path());
            }
        }
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let written = fs::File::create(&temporary)
            .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
            .and_then(|()| fs::rename(&temporary, &self.path));
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }
}

/// The toolchain's sysroot as rustup resolves it here. Recording only.
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

/// Every key input read without Cargo or rustc.
fn inputs(cwd: &Path, checkout: &Path, sysroot: &Path) -> Result<Inputs, String> {
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    if let Some((name, _)) = variables.iter().find(|(name, _)| {
        UNFOLLOWED_ENVIRONMENT
            .iter()
            .any(|unfollowed| name == unfollowed)
    }) {
        return Err(format!("{} is set", name.to_string_lossy()));
    }
    variables.sort();
    let mut environment = Sha256::new();
    for (name, value) in &variables {
        environment.update(name.as_bytes());
        environment.update(b"=");
        environment.update(value.as_bytes());
        environment.update(b"\0");
    }
    // Oxide's own profile definitions reach Cargo through the environment.
    for (name, value) in PROFILE_ENVIRONMENT {
        environment.update(format!("oxide:{name}={value}\0").as_bytes());
    }

    let mut files = Sha256::new();
    for dir in cwd.ancestors().take(MAX_ANCESTORS) {
        for name in ANCESTOR_FILES {
            digest_file(&mut files, &dir.join(name), name.starts_with(".cargo"))?;
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".cargo")))
        .ok_or("neither CARGO_HOME nor HOME is set")?;
    for name in ["config", "config.toml"] {
        digest_file(&mut files, &cargo_home.join(name), true)?;
    }
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|home| home.join(".rustup")))
        .ok_or("neither RUSTUP_HOME nor HOME is set")?;
    digest_file(&mut files, &rustup_home.join("settings.toml"), false)?;

    let mut lock = Sha256::new();
    digest_file(&mut lock, &checkout.join("Cargo.lock"), false)?;

    let mut toolchain = Sha256::new();
    toolchain.update(sysroot.as_os_str().as_bytes());
    for name in TOOLCHAIN_FILES {
        toolchain.update(b"\0");
        toolchain.update(name.as_bytes());
        if let Ok(metadata) = fs::symlink_metadata(sysroot.join(name)) {
            let identity = FileIdentity::of(&metadata);
            toolchain.update(
                format!(
                    "={}:{}:{}:{}",
                    identity.size, identity.mtime_ns, identity.inode, identity.device
                )
                .as_bytes(),
            );
        }
    }
    Ok(Inputs {
        environment: hex(&environment.finalize()),
        files: hex(&files.finalize()),
        lock: hex(&lock.finalize()),
        toolchain: hex(&toolchain.finalize()),
    })
}

/// Add a file's path and content (or its absence) to `hasher`. A Cargo
/// config naming an input the key does not follow refuses.
fn digest_file(hasher: &mut Sha256, path: &Path, config: bool) -> Result<(), String> {
    hasher.update(path.as_os_str().as_bytes());
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hasher.update(b"\0absent\0");
            return Ok(());
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err(format!("{} exceeds 1 MiB", path.display()));
    }
    if config {
        let text = String::from_utf8_lossy(&bytes);
        if let Some(word) = UNFOLLOWED_CONFIG.iter().find(|word| text.contains(*word)) {
            return Err(format!("{} names `{word}`", path.display()));
        }
    }
    hasher.update(format!("\0{}\0", bytes.len()).as_bytes());
    hasher.update(&bytes);
    Ok(())
}

/// Every file in the binary's Cargo dep-info (`<binary>.d`, documented next
/// to each artifact) must be a file the checkout's Git content covers: under
/// the repository and not ignored. So must every `Cargo.toml` between each
/// listed source and the repository top (each local package's manifest) and
/// the workspace root's. A path dependency outside the checkout, a generated
/// source in the build directory, an ignored source or manifest, or a
/// declared build-script input elsewhere refuses the record.
fn covered_dependencies(binary: &Path, top: &Path, checkout: &Path) -> Result<(), String> {
    let mut dep_info = binary.as_os_str().to_owned();
    dep_info.push(".d");
    let file = fs::File::open(&dep_info).map_err(|error| format!("no dep-info: {error}"))?;
    let mut text = String::new();
    file.take(MAX_DEP_INFO_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("cannot read dep-info: {error}"))?;
    if text.len() as u64 > MAX_DEP_INFO_BYTES {
        return Err("dep-info exceeds its size bound".to_owned());
    }
    let mut sources = BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (_, dependencies) = split_rule(line).ok_or("unexpected dep-info line")?;
        for dependency in dependencies {
            let path = fs::canonicalize(&dependency)
                .map_err(|error| format!("dep-info names {dependency}: {error}"))?;
            if !path.starts_with(top) {
                return Err(format!("{} is outside the checkout", path.display()));
            }
            sources.insert(path);
            if sources.len() > MAX_DEPENDENCIES {
                return Err("dep-info lists too many files".to_owned());
            }
        }
    }
    if sources.is_empty() {
        return Err("dep-info lists no files".to_owned());
    }
    let mut covered = sources.clone();
    covered.insert(checkout.join("Cargo.toml"));
    for source in &sources {
        for dir in source
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(top))
        {
            let manifest = dir.join("Cargo.toml");
            if fs::symlink_metadata(&manifest).is_ok() {
                covered.insert(manifest);
            }
        }
    }
    let mut relative = Vec::with_capacity(covered.len());
    for path in &covered {
        if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file()) {
            return Err(format!("{} is not a regular file", path.display()));
        }
        let inside = path
            .strip_prefix(top)
            .map_err(|_| format!("{} is outside the checkout", path.display()))?;
        let inside = inside.to_str().ok_or("a dependency path is not UTF-8")?;
        if inside.contains('\0') {
            return Err("a dependency path contains NUL".to_owned());
        }
        relative.push(inside.to_owned());
    }
    if relative.is_empty() {
        return Err("dep-info lists no files".to_owned());
    }
    let mut child = Command::new("git")
        .args(["check-ignore", "--stdin", "-z"])
        .current_dir(top)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start git: {error}"))?;
    let mut input = child.stdin.take().ok_or("git stdin was not captured")?;
    let list: Vec<u8> = relative
        .iter()
        .flat_map(|path| path.bytes().chain([0]))
        .collect();
    let writer = std::thread::spawn(move || input.write_all(&list));
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not wait for git: {error}"))?;
    let written = writer.join().map_err(|_| "git input writer panicked")?;
    // check-ignore exits 1 when no path is ignored.
    match (output.status.code(), written) {
        (Some(1), Ok(())) if output.stdout.is_empty() => Ok(()),
        (Some(0), _) => Err("the binary depends on a file Git ignores".to_owned()),
        _ => Err("git check-ignore failed".to_owned()),
    }
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

    #[test]
    fn dep_info_rules_unescape_spaces() {
        let (target, dependencies) =
            split_rule("/w/target/app: /w/src/main.rs /w/my\\ dir/lib.rs").expect("rule");
        assert_eq!(target, "/w/target/app");
        assert_eq!(dependencies, ["/w/src/main.rs", "/w/my dir/lib.rs"]);
        assert!(split_rule("no rule here").is_none());
    }
}
