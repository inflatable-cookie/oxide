//! Cargo home reporting and `oxide clean --cargo-cache`.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Cargo home).
//! Oxide never deletes in the Cargo home itself. `--cargo-cache` asks
//! Cargo 1.97.1 for one pass of its own automatic cleanup through
//! `cargo fetch`, with every HTTP request forced through a closed loopback
//! port, and reports the outcome Cargo's `gc` trace shows. Routes the proxy
//! cannot contain refuse before Cargo runs.

use crate::{adopt, freshness, identity, process, CARGO, MIB};
use oxide_storage::inventory::{inventory_directory, InventoryLimits};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The only Cargo whose cleanup trigger and trace text were qualified
/// (oxide#084, Cargo `c980f4866`).
const QUALIFIED_CARGO: &str = "cargo 1.97.1 ";
/// One `--config` argument each, Cargo's highest-precedence form.
const ALWAYS: &str = "cache.auto-clean-frequency=\"always\"";
/// Cargo's `gc` trace plus warnings from its cleanup module.
const TRACE: &str = "gc=trace,cargo::core::gc=trace";
/// crates.io as Cargo.lock records it; nothing else is contained.
const CRATES_IO: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];
/// Proxy variables libcurl reads; `no_proxy` would bypass a forced proxy.
const PROXY_ENVIRONMENT: [&str; 8] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
];
const MAX_LOCK_BYTES: u64 = 16 * MIB;
const MAX_CONFIG_BYTES: u64 = MIB;
const LIMITS: InventoryLimits = InventoryLimits {
    max_entries: 200_000,
    max_depth: 64,
    max_output_bytes: 64,
    max_elapsed_ms: 10_000,
};

/// `CARGO_HOME` as Cargo resolves it: the variable (relative to the current
/// directory) when set and non-empty, else `$HOME/.cargo`.
pub fn resolve() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot read the current directory: {error}"))?;
    resolve_from(
        std::env::var_os("CARGO_HOME"),
        std::env::var_os("HOME"),
        &cwd,
    )
}

fn resolve_from(
    cargo_home: Option<OsString>,
    home: Option<OsString>,
    cwd: &Path,
) -> Result<PathBuf, String> {
    if let Some(value) = cargo_home.filter(|value| !value.is_empty()) {
        return Ok(cwd.join(value));
    }
    match home.filter(|value| !value.is_empty()) {
        Some(home) => Ok(Path::new(&home).join(".cargo")),
        None => {
            Err("HOME and CARGO_HOME are unset; Oxide does not guess the Cargo home".to_owned())
        }
    }
}

/// One bounded walk: names, distinct inodes and allocation counted once per
/// inode. Symlinks are never followed and mounts are not crossed.
pub struct Measure {
    pub files: u64,
    pub distinct: u64,
    pub allocated: u64,
    pub complete: bool,
}

impl Measure {
    pub fn describe(&self) -> String {
        format!(
            "{} allocated (hard links counted once), {} files ({} distinct){}",
            size(self.allocated),
            self.files,
            self.distinct,
            if self.complete {
                ""
            } else {
                ", partial: a walk limit was reached or an entry was unreadable"
            }
        )
    }
}

/// Walk `root`, which must be a real directory: a symlink root refuses, so
/// the walk never leaves the named tree.
pub fn measure(root: &Path) -> Result<Option<Measure>, String> {
    match std::fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) if !metadata.is_dir() => {
            return Err("not a directory (a symlink is not followed)".to_owned())
        }
        Ok(_) => {}
    }
    let report = inventory_directory(root, LIMITS).map_err(|error| error.to_string())?;
    Ok(Some(Measure {
        files: report.total.files,
        distinct: report.total.distinct_files,
        allocated: report.total.allocated_bytes,
        complete: report.coverage.complete,
    }))
}

pub fn size(bytes: u64) -> String {
    if bytes < 10 * MIB {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

/// `oxide status` lines for the Cargo home.
pub fn report_status() {
    let home = match resolve() {
        Ok(home) => home,
        Err(error) => {
            println!("cargo home: {error}");
            return;
        }
    };
    match measure(&home) {
        Ok(Some(measure)) => println!("cargo home: {} {}", home.display(), measure.describe()),
        Ok(None) => println!("cargo home: {} absent", home.display()),
        Err(error) => println!("cargo home: {} not measured ({error})", home.display()),
    }
    println!(
        "  Cargo cleans it itself only at the end of an online build or fetch, at most once a day; --offline, --frozen or net.offline, a held cache lock or an earlier failure skip it. `oxide clean --cargo-cache` asks for one pass"
    );
}

/// Why the closed proxy cannot contain this run, or `None`. Takes names
/// only: a refused variable can hold a registry token, so no value is ever
/// read into a message.
fn refuse_environment<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    for name in names {
        let reason = match name {
            "CARGO_NET_RETRY" | "CARGO_REGISTRY_TOKEN" => continue,
            "CARGO_NET_OFFLINE" => "offline mode disables Cargo's cleanup",
            "CARGO_NET_GIT_FETCH_WITH_CLI" => {
                "net.git-fetch-with-cli runs the git CLI, which the closed HTTP proxy cannot contain"
            }
            _ if name.starts_with("CARGO_NET_") => "an unqualified network setting",
            _ if name.starts_with("CARGO_REGISTRIES_") => {
                "a custom registry setting the closed proxy is not proven to cover"
            }
            _ if name.starts_with("CARGO_REGISTRY_") => {
                "a registry setting (default registry or credential provider)"
            }
            _ if name.starts_with("CARGO_SOURCE_") => "a source replacement setting",
            _ if name.starts_with("CARGO_UNSTABLE_") => "an unstable Cargo feature",
            _ => continue,
        };
        return Some(format!("{name} is set: {reason}"));
    }
    None
}

/// Why one Cargo source cannot be contained, or `None` for crates.io.
fn refuse_source(source: &str) -> Option<&'static str> {
    if CRATES_IO.contains(&source) {
        return None;
    }
    Some(if let Some(url) = source.strip_prefix("git+") {
        if url.starts_with("https://") {
            "git sources are not qualified for the closed proxy"
        } else {
            "SSH and other non-HTTP git transports cannot be contained by the closed HTTP proxy"
        }
    } else if source.starts_with("registry+") || source.starts_with("sparse+") {
        "a custom registry the closed proxy is not proven to cover"
    } else {
        "an unsupported source kind"
    })
}

/// A `source` key's value on one Cargo.lock line: `Some(Ok)` for a plain
/// basic or literal string, `Some(Err)` for any other spelling, which is
/// refused rather than guessed.
fn lock_source(line: &str) -> Option<Result<&str, ()>> {
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if !matches!(key, "source" | "\"source\"" | "'source'") {
        return None;
    }
    let value = value.trim();
    let plain = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|inner| !inner.contains(['"', '\\']))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
                .filter(|inner| !inner.contains('\''))
        });
    Some(plain.ok_or(()))
}

/// Every source line in Cargo.lock must be crates.io. A screen only: the
/// authority is Cargo's own offline resolution, [`offline_resolution`].
fn refuse_lock(text: &str) -> Option<String> {
    for line in text.lines() {
        match lock_source(line) {
            None => {}
            Some(Err(())) => {
                return Some(format!(
                    "Cargo.lock line {:?} spells a source Oxide does not read",
                    line.trim()
                ))
            }
            Some(Ok(source)) => {
                if let Some(reason) = refuse_source(source) {
                    return Some(format!("Cargo.lock source {source}: {reason}"));
                }
            }
        }
    }
    None
}

/// Cargo's own parse, offline and locked, in the fetch's directory and
/// environment. It fails when Cargo.lock is stale, when a manifest names a
/// dependency the lock lacks, or when any source, index entry or package is
/// not already local, so the online fetch has nothing to resolve or
/// download that this resolve did not name. Every resolved package must
/// then come from crates.io or a path.
fn offline_resolution(cwd: &Path, cargo_home: Option<&Path>) -> Result<(), String> {
    let mut command = Command::new(CARGO);
    command
        .args(["metadata", "--format-version", "1", "--offline", "--locked"])
        .current_dir(cwd)
        .env_remove("CARGO_LOG");
    for name in PROXY_ENVIRONMENT {
        command.env_remove(name);
    }
    if let Some(home) = cargo_home {
        command.env("CARGO_HOME", home);
    }
    let output = freshness::run_cargo(command, freshness::MAX_METADATA_BYTES)?;
    if !output.status.success() {
        let error = output
            .stderr
            .lines()
            .find(|line| line.starts_with("error"))
            .unwrap_or("no error line")
            .to_owned();
        return Err(format!(
            "Cargo cannot resolve this workspace offline from Cargo.lock ({error}); the lock is stale or needs a source, index entry or package that is not already local, and resolving it online could take a route the closed proxy cannot contain"
        ));
    }
    if output.overflow {
        return Err("cargo metadata output exceeds 64 MiB".to_owned());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Cargo metadata is not JSON: {error}"))?;
    match refuse_resolved(&metadata) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// The first resolved package whose source cannot be contained.
fn refuse_resolved(metadata: &serde_json::Value) -> Option<String> {
    let Some(packages) = metadata.get("packages").and_then(|value| value.as_array()) else {
        return Some("Cargo metadata has no package list".to_owned());
    };
    for package in packages {
        match package.get("source") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(source)) => {
                if let Some(reason) = refuse_source(source) {
                    return Some(format!("resolved package source {source}: {reason}"));
                }
            }
            Some(_) => return Some("Cargo metadata has an unreadable package source".to_owned()),
        }
    }
    None
}

/// Cargo config files Cargo would read: `.cargo/config.toml` and
/// `.cargo/config` in the current directory and every ancestor, then in the
/// Cargo home. Each is parsed; see [`refuse_config`].
fn config_files(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut candidates: Vec<PathBuf> = cwd
        .ancestors()
        .flat_map(|dir| [dir.join(".cargo/config.toml"), dir.join(".cargo/config")])
        .collect();
    candidates.push(home.join("config.toml"));
    candidates.push(home.join("config"));
    for candidate in candidates {
        if std::fs::symlink_metadata(&candidate).is_ok() && !found.contains(&candidate) {
            found.push(candidate);
        }
    }
    found
}

/// Read one config file as Cargo would (following a symlink) and parse it
/// as TOML 1.0. Unreadable, oversized, non-UTF-8 or unparseable files
/// refuse; parser text is never shown, since it can quote a value.
fn read_config(path: &Path) -> Result<serde_json::Value, String> {
    let unreadable = |reason: String| format!("Cargo config file {} {reason}", path.display());
    let metadata =
        std::fs::metadata(path).map_err(|error| unreadable(format!("cannot be read ({error})")))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(unreadable(
            "is not a regular file of at most 1 MiB".to_owned(),
        ));
    }
    let bytes =
        std::fs::read(path).map_err(|error| unreadable(format!("cannot be read ({error})")))?;
    let text = String::from_utf8(bytes).map_err(|_| unreadable("is not UTF-8".to_owned()))?;
    basic_toml::from_str(&text).map_err(|error| {
        let line = error
            .line_col()
            .map(|(line, _)| format!(" (line {})", line + 1))
            .unwrap_or_default();
        unreadable(format!(
            "cannot be parsed as TOML 1.0{line}; Oxide refuses a config it cannot read"
        ))
    })
}

/// A key name for a message: bounded, never a value.
fn key_name(name: &str) -> String {
    let shown: String = name.chars().take(64).collect();
    format!("{shown:?}")
}

/// Settings `cargo fetch` and `cargo metadata` ignore or Oxide overrides;
/// every other key refuses. Whole tables: aliases cannot shadow built-in
/// commands; `term`, `cargo-new`, `doc`, `install`, `future-incompat-report`
/// and `profile` affect other commands or output; `target` and `env` apply
/// when Cargo compiles or runs; `resolver` only selects among versions the
/// lock already fixes.
const CONTAINED_TABLES: [&str; 10] = [
    "alias",
    "term",
    "cargo-new",
    "doc",
    "install",
    "future-incompat-report",
    "profile",
    "target",
    "env",
    "resolver",
];
/// Single keys inside otherwise refused tables. `http.proxy` and
/// `cache.auto-clean-frequency` are overridden by Oxide's own `--config`.
const CONTAINED_KEYS: [(&str, &[&str]); 5] = [
    ("net", &["retry"]),
    (
        "http",
        &[
            "proxy",
            "timeout",
            "low-speed-limit",
            "multiplexing",
            "debug",
            "user-agent",
            "check-revoke",
            "ssl-version",
            "cainfo",
            "proxy-cainfo",
        ],
    ),
    (
        "build",
        &[
            "jobs",
            "target",
            "target-dir",
            "build-dir",
            "rustflags",
            "rustdocflags",
            "incremental",
            "dep-info-basedir",
            "pipelining",
            "warnings",
        ],
    ),
    ("cache", &["auto-clean-frequency"]),
    ("registry", &["token"]),
];

/// The reason a named key cannot be contained.
fn config_reason(table: &str, key: Option<&str>) -> &'static str {
    match (table, key) {
        ("source", _) => {
            "source replacement can route downloads the closed proxy is not proven to cover"
        }
        ("registries", _) => "a custom registry the closed proxy is not proven to cover",
        ("registry", Some("credential-provider" | "global-credential-providers"))
        | ("credential-alias", _) => {
            "credential providers run programs the closed proxy cannot contain"
        }
        ("registry", Some("default")) => "it changes the default registry",
        ("net", Some("git-fetch-with-cli")) => {
            "net.git-fetch-with-cli runs the git CLI, which the closed HTTP proxy cannot contain"
        }
        ("net", Some("offline")) => "offline mode disables Cargo's cleanup",
        ("net", Some("ssh")) => "SSH transports cannot be contained by the closed HTTP proxy",
        ("include", _) => "it includes another config file",
        ("patch", _) => "[patch] can name git or registry sources outside crates.io",
        ("paths", _) => "path overrides change what Cargo resolves",
        ("unstable", _) => "an unstable Cargo feature",
        ("build", Some("rustc" | "rustc-wrapper" | "rustc-workspace-wrapper" | "rustdoc")) => {
            "it names a program Cargo runs, which the closed proxy cannot contain"
        }
        _ => "a setting Oxide has not shown the closed proxy contains",
    }
}

/// The first key in one parsed config that the closed proxy cannot contain.
/// TOML has already joined quoted, dotted and nested spellings into one
/// table tree.
fn refuse_config(config: &serde_json::Value) -> Option<String> {
    let Some(tables) = config.as_object() else {
        return Some("the document is not a table".to_owned());
    };
    for (table, value) in tables {
        if CONTAINED_TABLES.contains(&table.as_str()) {
            continue;
        }
        let refuse = |key: Option<&str>| {
            let name = match key {
                Some(key) => format!("{}.{}", key_name(table), key_name(key)),
                None => key_name(table),
            };
            Some(format!("key {name}: {}", config_reason(table, key)))
        };
        let Some((_, allowed)) = CONTAINED_KEYS.iter().find(|(name, _)| name == table) else {
            return refuse(None);
        };
        let Some(keys) = value.as_object() else {
            return refuse(None);
        };
        for key in keys.keys() {
            if !allowed.contains(&key.as_str()) {
                return refuse(Some(key));
            }
        }
    }
    None
}

/// Oxide's own proxy on 127.0.0.1: it accepts and closes every
/// connection and forwards nothing, so no request reaches a network. A
/// merely bound port is not enough: macOS drops connections to it until
/// they time out. Holding the listener keeps another process off the port.
struct ClosedProxy {
    port: u16,
    connections: Arc<AtomicU64>,
}

fn closed_proxy() -> Result<ClosedProxy, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|error| format!("cannot open the closed loopback proxy: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("cannot read the closed proxy's port: {error}"))?
        .port();
    let connections = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&connections);
    // The thread lives until the process exits, holding the port.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    Ok(ClosedProxy { port, connections })
}

/// What Cargo's `gc` trace showed for one run.
#[derive(Debug, Default)]
struct Signals {
    running: bool,
    frequency_skip: bool,
    lock_skip: bool,
    offline_skip: bool,
    disabled: bool,
    cleanup_error: bool,
}

impl Signals {
    fn observe(&mut self, line: &str) {
        if line.contains("failed to auto-clean cache data") {
            self.cleanup_error = true;
        } else if line.contains("gc: unable to acquire mutate lock, auto gc disabled") {
            self.lock_skip = true;
        } else if line.contains("gc: running offline, auto gc disabled") {
            self.offline_skip = true;
        } else if line.contains("gc: auto gc disabled") {
            self.disabled = true;
        } else if line.contains("gc: last auto gc was ") {
            if line.ends_with(", running") {
                self.running = true;
            } else if line.ends_with(", skipping") {
                self.frequency_skip = true;
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Ran,
    Skipped(&'static str),
    Failed(String),
}

/// Exactly one outcome, from Cargo's own evidence. Zero or several cleanup
/// decisions mean the trace is not understood, which is a failure.
fn classify(signals: &Signals, cargo_code: Option<i32>) -> Outcome {
    if signals.cleanup_error {
        return Outcome::Failed("Cargo reported \"failed to auto-clean cache data\"".to_owned());
    }
    if cargo_code != Some(0) {
        return Outcome::Failed(match cargo_code {
            Some(code) => format!("cargo fetch exited {code}"),
            None => "cargo fetch ended on a signal".to_owned(),
        });
    }
    let decisions = [
        signals.running,
        signals.frequency_skip,
        signals.lock_skip,
        signals.offline_skip,
        signals.disabled,
    ];
    if decisions.iter().filter(|seen| **seen).count() != 1 {
        return Outcome::Failed(
            "Cargo exited 0 but its gc trace held no single cleanup decision, so the outcome is unknown"
                .to_owned(),
        );
    }
    if signals.running {
        Outcome::Ran
    } else if signals.lock_skip {
        Outcome::Skipped("another Cargo holds the package cache lock")
    } else if signals.frequency_skip {
        Outcome::Skipped(
            "its frequency gate: the last pass or the cache database's creation was this same second",
        )
    } else if signals.offline_skip {
        Outcome::Skipped("Cargo ran offline")
    } else {
        Outcome::Skipped("automatic cleanup is disabled")
    }
}

/// Cargo's log lines (`<elapsed>s LEVEL target: ...`) are evidence, not
/// output; everything else on stderr passes through.
fn is_log_line(line: &str) -> bool {
    let mut words = line.split_whitespace();
    let elapsed = words
        .next()
        .and_then(|word| word.strip_suffix('s'))
        .is_some_and(|number| number.parse::<f64>().is_ok());
    elapsed
        && matches!(
            words.next(),
            Some("TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR")
        )
}

/// `oxide clean --cargo-cache`.
pub fn run_cache_clean(args: &[OsString]) -> Result<u8, String> {
    if !args.is_empty() {
        return Err("clean --cargo-cache takes no other arguments".to_owned());
    }
    let refuse = |reason: String| format!("clean --cargo-cache refused: {reason}");
    let home = resolve()?;
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot read the current directory: {error}"))?;
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .collect();
    if let Some(reason) = refuse_environment(names.iter().map(String::as_str)) {
        return Err(refuse(reason));
    }
    for file in config_files(&cwd, &home) {
        let config = read_config(&file).map_err(refuse)?;
        if let Some(reason) = refuse_config(&config) {
            return Err(refuse(format!(
                "Cargo config file {} sets {reason}",
                file.display()
            )));
        }
    }
    let version = Command::new(CARGO)
        .arg("-V")
        .env_remove("CARGO_LOG")
        .output()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    let version = String::from_utf8_lossy(&version.stdout);
    if !version.starts_with(QUALIFIED_CARGO) {
        return Err(refuse(format!(
            "{} is not the qualified Cargo 1.97.1, whose cleanup trigger and trace Oxide reads",
            version.trim()
        )));
    }
    let workspace = identity::locate_workspace(CARGO, None)?;
    if !adopt::is_adopted(&workspace) {
        return Err(refuse(format!(
            "{} is not adopted; run `oxide init` first",
            workspace.display()
        )));
    }
    let lock = workspace.join("Cargo.lock");
    match std::fs::symlink_metadata(&lock) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_LOCK_BYTES => {}
        Ok(_) => {
            return Err(refuse(format!(
                "{} is not a regular file of at most 16 MiB",
                lock.display()
            )))
        }
        Err(_) => {
            return Err(refuse(format!(
                "{} is missing; without a lockfile Cargo would resolve over the network",
                lock.display()
            )))
        }
    }
    let text = std::fs::read_to_string(&lock)
        .map_err(|error| format!("cannot read {}: {error}", lock.display()))?;
    if let Some(reason) = refuse_lock(&text) {
        return Err(refuse(reason));
    }
    // Debug builds only: the fixture proof skips this gate once, so the
    // fetch itself meets the closed proxy. Release builds always run it.
    let skip_gate =
        cfg!(debug_assertions) && std::env::var_os("OXIDE_TEST_SKIP_OFFLINE_RESOLUTION").is_some();
    if !skip_gate {
        offline_resolution(&cwd, None).map_err(refuse)?;
    }

    let before = measure(&home).ok().flatten();
    let proxy = closed_proxy()?;
    eprintln!(
        "oxide: asking Cargo to clean {} (cargo fetch --locked, cleanup frequency \"always\", every HTTP request through closed proxy 127.0.0.1:{})",
        home.display(),
        proxy.port
    );
    process::install_signal_handlers()?;
    let mut cargo = Command::new(CARGO);
    cargo
        .args(["fetch", "--locked", "--config", ALWAYS, "--config"])
        .arg(format!("http.proxy=\"http://127.0.0.1:{}\"", proxy.port))
        .env("CARGO_LOG", TRACE)
        .stderr(Stdio::piped());
    for name in PROXY_ENVIRONMENT {
        cargo.env_remove(name);
    }
    let mut group = process::spawn_group(&mut cargo)?;
    let stderr = group
        .child
        .stderr
        .take()
        .ok_or("Cargo stderr was not captured")?;
    let reader = std::thread::spawn(move || {
        let mut signals = Signals::default();
        let mut reader = BufReader::new(stderr);
        let mut bytes = Vec::new();
        while reader
            .by_ref()
            .take(64 * 1024)
            .read_until(b'\n', &mut bytes)
            .is_ok_and(|read| read > 0)
        {
            let line = String::from_utf8_lossy(&bytes);
            let line = line.trim_end();
            signals.observe(line);
            if !is_log_line(line) {
                eprintln!("{line}");
            }
            bytes.clear();
        }
        signals
    });
    let status = process::wait_leader(&mut group);
    process::wait_group_empty(group.id);
    let signals = reader
        .join()
        .map_err(|_| "the Cargo stderr reader panicked")?;
    let status = status?;
    eprintln!(
        "oxide: closed proxy 127.0.0.1:{} closed {} connection(s) and forwarded nothing",
        proxy.port,
        proxy.connections.load(Ordering::SeqCst)
    );
    let after = measure(&home).ok().flatten();

    let outcome = classify(&signals, status.code());
    let code = match &outcome {
        Outcome::Ran => {
            eprintln!("oxide: Cargo cleanup ran (Cargo's gc trace shows the pass running and no cleanup error; fetch exit 0)");
            0
        }
        Outcome::Skipped(reason) => {
            eprintln!("oxide: Cargo skipped cleanup: {reason}");
            75
        }
        Outcome::Failed(reason) => {
            eprintln!("oxide: Cargo cleanup failed: {reason}");
            match process::exit_code(status) {
                0 => 1,
                code => code,
            }
        }
    };
    let describe = |measure: &Option<Measure>| {
        measure
            .as_ref()
            .map_or_else(|| "not measured".to_owned(), Measure::describe)
    };
    eprintln!("oxide: cargo home before: {}", describe(&before));
    eprintln!("oxide: cargo home after: {}", describe(&after));
    eprintln!("oxide: a size difference is not proof of what the cleanup deleted; a completed pass may delete nothing, and other writers may change the home");
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_home_resolution_matches_cargo() {
        let cwd = Path::new("/work");
        let home = Some(OsString::from("/users/me"));
        assert_eq!(
            resolve_from(Some("rel/home".into()), home.clone(), cwd).unwrap(),
            PathBuf::from("/work/rel/home")
        );
        assert_eq!(
            resolve_from(Some("/abs".into()), home.clone(), cwd).unwrap(),
            PathBuf::from("/abs")
        );
        assert_eq!(
            resolve_from(Some("".into()), home, cwd).unwrap(),
            PathBuf::from("/users/me/.cargo")
        );
        assert!(resolve_from(None, Some("".into()), cwd).is_err());
    }

    #[test]
    fn uncontained_environment_refuses_with_its_reason() {
        let refuse = |name: &str| refuse_environment([name]);
        assert_eq!(
            refuse("CARGO_REGISTRIES_MINE_TOKEN").as_deref(),
            Some("CARGO_REGISTRIES_MINE_TOKEN is set: a custom registry setting the closed proxy is not proven to cover")
        );
        assert!(refuse("CARGO_NET_GIT_FETCH_WITH_CLI")
            .unwrap()
            .contains("git-fetch-with-cli"));
        assert!(refuse("CARGO_NET_OFFLINE").unwrap().contains("offline"));
        assert!(refuse("CARGO_REGISTRIES_MINE_INDEX")
            .unwrap()
            .contains("custom registry"));
        assert!(refuse("CARGO_SOURCE_CRATES_IO_REPLACE_WITH").is_some());
        assert!(refuse("CARGO_REGISTRY_GLOBAL_CREDENTIAL_PROVIDERS").is_some());
        assert!(refuse("CARGO_UNSTABLE_GITOXIDE").is_some());
        for allowed in [
            "CARGO_NET_RETRY",
            "CARGO_REGISTRY_TOKEN",
            "CARGO_HTTP_PROXY",
            "PATH",
        ] {
            assert_eq!(refuse(allowed), None, "{allowed}");
        }
    }

    #[test]
    fn only_crates_io_and_path_lock_sources_are_contained() {
        let lock = |source: &str| {
            format!("[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"{source}\"\n")
        };
        assert_eq!(refuse_lock(&lock(CRATES_IO[0])), None);
        assert_eq!(refuse_lock(&lock(CRATES_IO[1])), None);
        assert_eq!(
            refuse_lock("[[package]]\nname = \"local\"\nversion = \"0.1.0\"\n"),
            None
        );
        assert!(
            refuse_lock(&lock("git+ssh://git@example.invalid/a.git#0123"))
                .unwrap()
                .contains("SSH")
        );
        assert!(refuse_lock(&lock("git+https://example.invalid/a.git#0123"))
            .unwrap()
            .contains("git sources"));
        assert!(refuse_lock(&lock("sparse+https://example.invalid/index/"))
            .unwrap()
            .contains("custom registry"));
        assert!(refuse_lock(&lock("registry+https://example.invalid/index"))
            .unwrap()
            .contains("custom registry"));
    }

    #[test]
    fn lock_sources_in_either_quote_style_are_read_and_others_refuse() {
        assert_eq!(
            lock_source("source = 'git+ssh://h/r.git#1'"),
            Some(Ok("git+ssh://h/r.git#1"))
        );
        assert_eq!(lock_source("  source=\"x\"  "), Some(Ok("x")));
        assert_eq!(lock_source("\"source\" = 'x'"), Some(Ok("x")));
        assert_eq!(lock_source("name = \"source\""), None);
        assert_eq!(lock_source("source = \"git+\\u0073sh://h\""), Some(Err(())));
        assert_eq!(lock_source("source = \"\"\"x\"\"\""), Some(Err(())));
        assert!(
            refuse_lock("[[package]]\nsource = 'git+ssh://git@example.invalid/a.git#1'\n")
                .unwrap()
                .contains("SSH")
        );
        assert!(refuse_lock("source = \"git+\\u0073sh://h\"\n")
            .unwrap()
            .contains("does not read"));
    }

    #[test]
    fn resolved_sources_come_from_cargo_metadata() {
        let metadata = |source: serde_json::Value| serde_json::json!({"packages": [{"name": "a", "source": null}, {"name": "b", "source": source}]});
        assert_eq!(
            refuse_resolved(&metadata(serde_json::json!(CRATES_IO[1]))),
            None
        );
        assert!(
            refuse_resolved(&metadata(serde_json::json!("git+ssh://h/r.git?rev=1#1")))
                .unwrap()
                .contains("SSH")
        );
        assert!(refuse_resolved(&metadata(serde_json::json!(7))).is_some());
        assert!(refuse_resolved(&serde_json::json!({})).is_some());
    }

    /// A tiny offline fixture under the scratch temp directory, with an
    /// empty private Cargo home; nothing here may use the network.
    fn offline_fixture(label: &str, manifest_deps: &str, lock: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("oxide-offline-{label}-{}", std::process::id()));
        let package = root.join("app");
        std::fs::create_dir_all(package.join("src")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::write(
            package.join("Cargo.toml"),
            format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{manifest_deps}"),
        )
        .unwrap();
        std::fs::write(
            package.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.97.1\"\n",
        )
        .unwrap();
        std::fs::write(package.join("src/lib.rs"), "").unwrap();
        std::fs::write(package.join("Cargo.lock"), format!("version = 4\n\n{lock}")).unwrap();
        (root.clone(), package)
    }

    #[test]
    fn a_stale_lock_with_an_ssh_dependency_refuses_offline() {
        // The lock names only crates.io; the manifest adds an SSH git
        // dependency the online resolve would fetch.
        let (root, package) = offline_fixture(
            "stale",
            "remote = { git = \"ssh://git@127.0.0.1:9/remote.git\" }\n",
            "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
        );
        let refused = offline_resolution(&package, Some(&root.join("home"))).unwrap_err();
        assert!(
            refused.contains("cannot resolve this workspace offline"),
            "{refused}"
        );
        let (clean, local) = offline_fixture(
            "local",
            "",
            "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
        );
        assert_eq!(
            offline_resolution(&local, Some(&clean.join("home"))),
            Ok(())
        );
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&clean).unwrap();
    }

    #[test]
    fn config_files_are_found_in_ancestors_and_the_home() {
        let root = std::env::temp_dir().join(format!("oxide-config-{}", std::process::id()));
        let deep = root.join("a/b");
        std::fs::create_dir_all(deep.join(".cargo")).unwrap();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(root.join(".cargo/config"), "").unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        assert_eq!(config_files(&deep, &home), vec![root.join(".cargo/config")]);
        std::fs::write(home.join("config.toml"), "").unwrap();
        assert_eq!(config_files(&deep, &home).len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn parsed(text: &str) -> Result<serde_json::Value, ()> {
        basic_toml::from_str(text).map_err(|_| ())
    }

    #[test]
    fn contained_configs_proceed() {
        for text in [
            "",
            "# [build]\n#jobs = 8\n",
            "[build]\njobs = 4\nrustflags = [\"-Cdebuginfo=1\"]\n[alias]\nb = \"build\"\n[term]\ncolor = \"never\"\n",
            "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n[net]\nretry = 3\n[http]\ntimeout = 30\n[registry]\ntoken = \"t\"\n",
            "[env]\nRUST_TEST_THREADS = \"1\"\n[profile.dev]\ndebug = 1\n[cache]\nauto-clean-frequency = \"never\"\n",
        ] {
            assert_eq!(refuse_config(&parsed(text).unwrap()), None, "{text}");
        }
    }

    #[test]
    fn risky_config_keys_refuse_in_every_spelling() {
        for (text, needle) in [
            (
                "[source.crates-io]\nreplace-with = \"m\"\n",
                "source replacement",
            ),
            (
                "source.crates-io.replace-with = \"m\"\n",
                "source replacement",
            ),
            (
                "[registries.mine]\nindex = \"sparse+https://r.invalid/\"\n",
                "custom registry",
            ),
            (
                "[registry]\ncredential-provider = \"cargo:token\"\n",
                "credential providers",
            ),
            (
                "registry.global-credential-providers = [\"x\"]\n",
                "credential providers",
            ),
            ("[credential-alias]\nx = \"y\"\n", "credential providers"),
            ("[net]\ngit-fetch-with-cli = true\n", "git CLI"),
            ("net.\"git-fetch-with-cli\" = true\n", "git CLI"),
            ("\"net\".'git-fetch-with-cli' = true\n", "git CLI"),
            ("net = { git-fetch-with-cli = true }\n", "git CLI"),
            ("[net]\noffline = true\n", "offline"),
            ("[net.ssh]\nknown-hosts = []\n", "SSH"),
            ("include = \"other.toml\"\n", "includes another config file"),
            (
                "[patch.crates-io]\nx = { git = \"ssh://h/x\" }\n",
                "[patch]",
            ),
            ("paths = [\"/x\"]\n", "path overrides"),
            ("[unstable]\ngc = true\n", "unstable"),
            ("[build]\nrustc-wrapper = \"w\"\n", "names a program"),
            ("[registry]\ndefault = \"mine\"\n", "default registry"),
            ("[http]\nunknown = 1\n", "not shown"),
            ("[made-up]\nx = 1\n", "not shown"),
            ("net = 3\n", "not shown"),
        ] {
            let reason = refuse_config(&parsed(text).unwrap()).unwrap_or_default();
            assert!(reason.contains(needle), "{text}: {reason}");
        }
    }

    #[test]
    fn unreadable_configs_refuse_without_values() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("oxide-config-read-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("config.toml");
        // Malformed, and TOML 1.1-only syntax (a newline in an inline table).
        for text in [
            "[net\ngit-fetch-with-cli = true\n",
            "token = \"secret-value\n",
            "net = {\n  retry = 2\n}\n",
        ] {
            std::fs::write(&file, text).unwrap();
            let refused = read_config(&file).unwrap_err();
            assert!(
                refused.contains("cannot be parsed as TOML 1.0"),
                "{refused}"
            );
            assert!(!refused.contains("secret"), "{refused}");
        }
        std::fs::write(
            &file,
            "[registry]\ntoken = \"secret-value\"\n[registries.x]\ntoken = \"secret-value\"\n",
        )
        .unwrap();
        let reason = refuse_config(&read_config(&file).unwrap()).unwrap();
        assert!(
            reason.contains("\"registries\"") && !reason.contains("secret"),
            "{reason}"
        );
        std::fs::write(
            &file,
            vec![b'#'; usize::try_from(MAX_CONFIG_BYTES).unwrap() + 1],
        )
        .unwrap();
        assert!(read_config(&file).unwrap_err().contains("at most 1 MiB"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn signals(lines: &[&str]) -> Signals {
        let mut signals = Signals::default();
        for line in lines {
            signals.observe(line);
        }
        signals
    }

    #[test]
    fn outcomes_come_from_cargo_evidence_only() {
        let ran = signals(&[
            "   0.0047s TRACE gc: should_run_auto_gc",
            "   0.0048s TRACE gc: last auto gc was 1792358425, running",
        ]);
        assert_eq!(classify(&ran, Some(0)), Outcome::Ran);
        assert!(matches!(classify(&ran, Some(101)), Outcome::Failed(_)));
        let skipped = signals(&["   0.27s TRACE gc: last auto gc was 1792358423, skipping"]);
        assert!(
            matches!(classify(&skipped, Some(0)), Outcome::Skipped(reason) if reason.contains("frequency"))
        );
        let locked =
            signals(&["   0.1s TRACE gc: unable to acquire mutate lock, auto gc disabled"]);
        assert!(
            matches!(classify(&locked, Some(0)), Outcome::Skipped(reason) if reason.contains("lock"))
        );
        let offline = signals(&["   0.1s TRACE gc: running offline, auto gc disabled"]);
        assert!(
            matches!(classify(&offline, Some(0)), Outcome::Skipped(reason) if reason.contains("offline"))
        );
        let errored = signals(&[
            "   0.0048s TRACE gc: last auto gc was 1, running",
            "warning: failed to auto-clean cache data",
        ]);
        assert!(matches!(classify(&errored, Some(0)), Outcome::Failed(_)));
        // Exit 0 alone is never success.
        assert!(matches!(
            classify(&Signals::default(), Some(0)),
            Outcome::Failed(_)
        ));
        let both = signals(&[
            "   0.1s TRACE gc: last auto gc was 1, running",
            "   0.2s TRACE gc: last auto gc was 1, skipping",
        ]);
        assert!(matches!(classify(&both, Some(0)), Outcome::Failed(_)));
    }

    #[test]
    fn log_lines_are_told_from_cargo_output() {
        assert!(is_log_line(
            "   0.004795250s TRACE gc: last auto gc was 1, running"
        ));
        assert!(is_log_line("   0.0048s DEBUG gc: starting db sync"));
        assert!(!is_log_line("warning: failed to auto-clean cache data"));
        assert!(!is_log_line("error: failed to download from `https://...`"));
    }

    #[test]
    fn the_closed_proxy_closes_and_counts_connections() {
        let proxy = closed_proxy().unwrap();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
        assert_eq!(proxy.connections.load(Ordering::SeqCst), 1);
    }
}
