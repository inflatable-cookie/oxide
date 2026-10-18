//! Cargo home reporting and `oxide clean --cargo-cache`.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Cargo home).
//! Oxide never deletes in the Cargo home itself. `--cargo-cache` asks
//! Cargo 1.97.1 for one pass of its own automatic cleanup through
//! `cargo fetch`, with every HTTP request forced through a closed loopback
//! port, and reports the outcome Cargo's `gc` trace shows. Routes the proxy
//! cannot contain refuse before Cargo runs.

use crate::{adopt, identity, process, CARGO, MIB};
use oxide_storage::inventory::{inventory_directory, InventoryLimits};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Why the closed proxy cannot contain this run, or `None`.
fn refuse_environment<'a>(
    variables: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    for (name, value) in variables {
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
        return Some(format!("{name}={value} is set: {reason}"));
    }
    None
}

/// Every source in Cargo.lock must be crates.io or a path (no source line).
fn refuse_lock(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(source) = line
            .trim()
            .strip_prefix("source = \"")
            .and_then(|rest| rest.strip_suffix('"'))
        else {
            continue;
        };
        if CRATES_IO.contains(&source) {
            continue;
        }
        let reason = if let Some(url) = source.strip_prefix("git+") {
            if url.starts_with("https://") {
                "git sources are not qualified for the closed proxy"
            } else {
                "SSH and other non-HTTP git transports cannot be contained by the closed HTTP proxy"
            }
        } else if source.starts_with("registry+") || source.starts_with("sparse+") {
            "a custom registry the closed proxy is not proven to cover"
        } else {
            "an unsupported source kind"
        };
        return Some(format!("Cargo.lock source {source}: {reason}"));
    }
    None
}

/// Cargo config files Cargo would read: `.cargo/config.toml` and
/// `.cargo/config` in the current directory and every ancestor, then in the
/// Cargo home. Oxide does not parse them; any one refuses.
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

/// A bound TCP socket on 127.0.0.1 that never listens: connections to its
/// port are refused, and no other process can take the port while it is
/// held.
struct ClosedPort {
    _socket: OwnedFd,
    port: u16,
}

fn closed_port() -> Result<ClosedPort, String> {
    let error = |what: &str| {
        format!(
            "cannot reserve a closed loopback port ({what}): {}",
            std::io::Error::last_os_error()
        )
    };
    // SAFETY: plain socket creation; the result is checked before use.
    let raw = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(error("socket"));
    }
    // SAFETY: `raw` is a fresh descriptor this function owns.
    let socket = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: zeroed `sockaddr_in` is a valid value to fill in.
    let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    address.sin_family = libc::AF_INET as libc::sa_family_t;
    address.sin_port = 0;
    address.sin_addr.s_addr = u32::from(std::net::Ipv4Addr::LOCALHOST).to_be();
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::sockaddr_in>())
        .map_err(|_| "sockaddr size")?;
    // SAFETY: `address` and `length` describe one valid `sockaddr_in`.
    let bound = unsafe {
        libc::bind(
            socket.as_raw_fd(),
            std::ptr::addr_of!(address).cast(),
            length,
        )
    };
    if bound != 0 {
        return Err(error("bind"));
    }
    // SAFETY: as above; the kernel writes back the bound address.
    let named = unsafe {
        libc::getsockname(
            socket.as_raw_fd(),
            std::ptr::addr_of_mut!(address).cast(),
            &mut length,
        )
    };
    if named != 0 {
        return Err(error("getsockname"));
    }
    Ok(ClosedPort {
        _socket: socket,
        port: u16::from_be(address.sin_port),
    })
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
    let variables: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| {
            Some((
                name.into_string().ok()?,
                value.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    if let Some(reason) =
        refuse_environment(variables.iter().map(|(n, v)| (n.as_str(), v.as_str())))
    {
        return Err(refuse(reason));
    }
    if let Some(file) = config_files(&cwd, &home).first() {
        return Err(refuse(format!(
            "Cargo config file {} exists; it could set source replacement, registries, git-fetch-with-cli, credential providers or includes, and Oxide does not parse it",
            file.display()
        )));
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

    let before = measure(&home).ok().flatten();
    let port = closed_port()?;
    eprintln!(
        "oxide: asking Cargo to clean {} (cargo fetch --locked, cleanup frequency \"always\", every HTTP request through closed proxy 127.0.0.1:{})",
        home.display(),
        port.port
    );
    process::install_signal_handlers()?;
    let mut cargo = Command::new(CARGO);
    cargo
        .args(["fetch", "--locked", "--config", ALWAYS, "--config"])
        .arg(format!("http.proxy=\"http://127.0.0.1:{}\"", port.port))
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
    drop(port);
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
        let refuse = |name: &str| refuse_environment([(name, "x")]);
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
    fn the_closed_port_refuses_connections() {
        let port = closed_port().unwrap();
        let refused = std::net::TcpStream::connect(("127.0.0.1", port.port)).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
    }
}
