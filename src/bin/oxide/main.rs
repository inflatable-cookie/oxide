//! Unpublished `oxide` front door: Cargo with managed storage and profiles.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md`. Storage is the
//! existing lease pool; Oxide adds checkout affinity, FIFO waiting, a
//! host-wide limit and own-command crash recovery in a private control
//! directory. A checkout may take another checkout's idle compatible slot
//! only through the freshness gate. Plain `cargo` behavior is untouched.

mod adopt;
mod cli;
mod freshness;
mod identity;
mod process;
mod store;

use cli::{Class, Invocation, PROFILE_ENVIRONMENT};
use freshness::{Gate, Prior};
use identity::{RunPick, SlotKey};
use oxide_storage::inventory::{inventory_directory, Category, InventoryLimits};
use oxide_storage::lease::SlotOccupancy;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use store::{Acquired, Cleaning, Store};

/// The `cargo` rustup resolves for the current directory. Oxide never selects
/// a toolchain.
pub(crate) const CARGO: &str = "cargo";
pub(crate) const MIB: u64 = 1024 * 1024;
const USAGE: &str = "\
oxide: Cargo with managed build storage and profiles

USAGE:
    oxide <check|test|clippy|build|run> [--profile quick|debug|release] [cargo args] [-- args]
    oxide <other cargo command> [args]   passed to Cargo; leased when it can build
    oxide clean [cargo clean args]       private outputs and slot affinity; slots stay
    oxide status                         pool, slots, waits and stray Cargo output
    oxide init [cargo init args] [path]  cargo init if no Cargo.toml, then adopt Oxide
    oxide new <path> [cargo new args]    cargo new, then adopt Oxide

Profiles: quick (default, line tables), debug (full debug info), release (the
project's own). Oxide places build output, so --target-dir and --config
build.build-dir/target-dir are refused. Adoption writes one marked block in the
workspace-root AGENTS.md and prints readiness; there is no Oxide config file.
OXIDE_POOL selects the pool; OXIDE_MAX_ACTIVE the host-wide build limit (2).
";

fn main() -> ExitCode {
    process::started_ms();
    match run(std::env::args_os().skip(1).collect()) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("oxide: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<OsString>) -> Result<u8, String> {
    let Some(first) = args.first() else {
        print!("{USAGE}");
        return Ok(0);
    };
    let command = first.to_str().ok_or("the command must be valid UTF-8")?;
    let rest = &args[1..];
    match command {
        "-h" | "--help" => {
            print!("{USAGE}");
            return Ok(0);
        }
        "-V" | "--version" | "--list" => return Err(exec_cargo(&args)),
        _ if command.starts_with('+') => {
            return Err(
                "toolchain overrides are refused: Oxide uses the toolchain rustup resolves for the project"
                    .to_owned(),
            );
        }
        _ if command.starts_with('-') => {
            return Err(format!(
                "{command}: put Cargo options after the command, for example `oxide build {command}`"
            ));
        }
        _ => {}
    }
    match cli::classify(command) {
        Class::Plain => Err(exec_cargo(&args)),
        Class::Status => run_status(rest),
        Class::Clean => run_clean(rest),
        Class::Init => adopt::run_init(rest),
        Class::New => adopt::run_new(rest),
        Class::Modelled | Class::Building => run_leased(command, rest),
    }
}

/// Replace Oxide with plain Cargo. Returns only on failure.
fn exec_cargo(args: &[OsString]) -> String {
    let error = Command::new(CARGO).args(args).exec();
    format!("could not start Cargo: {error}")
}

fn apply_storage(command: &mut Command, build_dir: &Path, checkout: &Path) {
    command
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .env("CARGO_TARGET_DIR", checkout.join("target"))
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (name, value) in PROFILE_ENVIRONMENT {
        command.env(name, value);
    }
}

fn run_leased(command: &str, args: &[OsString]) -> Result<u8, String> {
    let mut invocation = cli::parse_leased(command, args)?;
    if command == "run" && cli::run_selection(&invocation.cargo_args).is_none() {
        // Build only what `cargo run` would run: `default-run`, else the
        // package's only binary.
        match identity::run_pick(
            CARGO,
            invocation.manifest_path.as_deref(),
            cli::package_selection(args).as_deref(),
        )? {
            RunPick::Bin(name) => {
                invocation
                    .cargo_args
                    .splice(1..1, [OsString::from("--bin"), OsString::from(name)]);
            }
            RunPick::Ambiguous(names) => {
                eprintln!(
                    "error: `cargo run` could not determine which binary to run. Use the `--bin` option to specify a binary, or the `default-run` manifest key.\navailable binaries: {}",
                    names.join(", ")
                );
                return Ok(101);
            }
            RunPick::Unresolved => {}
        }
    }
    let key = SlotKey::resolve(
        CARGO,
        invocation.manifest_path.as_deref(),
        invocation.profile.label(),
        invocation.target.as_deref(),
    )?;
    process::install_signal_handlers()?;
    let store = Store::open_or_create()?;
    let started = Instant::now();
    let (held, gate) = match lease_with_gate(&store, &key, invocation.profile)? {
        Ok(leased) => leased,
        Err(signal) => return Ok(process::signal_exit_code(signal)),
    };
    if let Some(signal) = process::received_signal() {
        let origin = unchanged_origin(&held.prior);
        store.release(held, origin)?;
        return Ok(process::signal_exit_code(signal));
    }
    let run = command == "run";
    let mut cargo = Command::new(CARGO);
    cargo.args(&invocation.cargo_args);
    apply_storage(&mut cargo, &held.build_dir, &key.checkout);
    if run {
        cargo.stdout(Stdio::piped());
    }
    let mut group = match store
        .mark_spawning(&held)
        .and_then(|()| process::spawn_group(&mut cargo))
    {
        Ok(group) => group,
        Err(error) => {
            let origin = unchanged_origin(&held.prior);
            store.release(held, origin)?;
            return Err(error);
        }
    };
    let outcome = match store.mark_running(&held, group.id) {
        Ok(()) if run => collect_run(&mut group, &invocation),
        Ok(()) => process::wait_leader(&mut group).map(|status| (status, None)),
        Err(error) => {
            // SAFETY: the negative PID addresses the group this command created.
            unsafe {
                libc::kill(-group.id, libc::SIGTERM);
            }
            let _ = process::wait_leader(&mut group);
            Err(error)
        }
    };
    process::wait_group_empty(group.id);
    let slot = held.slot_id.clone();
    let warm = held.reused;
    let note = gate.describe();
    let origin = gate.finish(&key.checkout);
    store.release(held, origin)?;
    let (status, executable) = outcome?;
    print_summary(&store, &invocation, &slot, warm, started, note.as_deref());
    if !run || !status.success() {
        return Ok(process::exit_code(status));
    }
    match executable.unwrap_or_else(|| Err("no run target was recorded".to_owned())) {
        Ok((program, manifest_dir)) => {
            // Lease released: the program runs from the private target dir.
            let error = Command::new(&program)
                .args(&invocation.program_args)
                .env("CARGO_MANIFEST_DIR", manifest_dir)
                .exec();
            Err(format!("could not execute {}: {error}", program.display()))
        }
        Err(message) => {
            eprintln!("oxide: error: {message}");
            Ok(101)
        }
    }
}

/// Lease a slot and run the freshness gate. A slot taken from another
/// checkout whose gate refuses is given back, and the command leases again
/// without sharing, so it builds cold rather than unchecked. `Err` is a
/// signal received while waiting.
fn lease_with_gate(
    store: &Store,
    key: &SlotKey,
    profile: cli::Profile,
) -> Result<Result<(store::Lease, Gate), i32>, String> {
    let mut share = true;
    loop {
        let held = match store.acquire(key, profile, share)? {
            Acquired::Lease(held) => held,
            Acquired::Interrupted(signal) => return Ok(Err(signal)),
        };
        // The slot's content came from another checkout: taken just now, or
        // left on this checkout's record by a command that died before Cargo.
        let foreign = match &held.prior {
            Prior::Known(origin) if origin.checkout != key.checkout => {
                Some(origin.checkout.clone())
            }
            _ => None,
        };
        match freshness::prepare(&key.checkout, &held.prior) {
            Ok(gate) => return Ok(Ok((held, gate))),
            Err(reason) if foreign.is_some() || held.shared_from.is_some() => {
                eprintln!(
                    "oxide: freshness gate refused another checkout's slot ({reason}); building cold"
                );
                let mut held = held;
                // Hand the slot back to the checkout whose content it holds.
                if held.shared_from.is_none() {
                    held.shared_from = foreign;
                }
                store.unshare(held)?;
                share = false;
            }
            // This checkout's own build history: Cargo's own mtime checks hold
            // as before, but the slot is not shared afterwards.
            Err(_) => return Ok(Ok((held, Gate::untrusted()))),
        }
    }
}

/// Cargo never ran: the slot keeps the origin it had.
fn unchanged_origin(prior: &Prior) -> Option<freshness::Origin> {
    match prior {
        Prior::Known(origin) => Some(origin.clone()),
        Prior::Fresh | Prior::Unknown => None,
    }
}

type RunTarget = Result<(PathBuf, PathBuf), String>;

/// `run`: wait for the build while reading Cargo's JSON for the executable.
fn collect_run(
    group: &mut process::Group,
    invocation: &Invocation,
) -> Result<(ExitStatus, Option<RunTarget>), String> {
    let stdout = group
        .child
        .stdout
        .take()
        .ok_or("Cargo stdout was not captured")?;
    let (sender, receiver) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let selection = cli::run_selection(&invocation.cargo_args);
    let mut found: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    let mut status = None;
    let mut closed = false;
    while status.is_none() || !closed {
        process::forward_pending(group.id);
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(line) => {
                if invocation.json_messages {
                    println!("{line}");
                }
                if let Some(artifact) = run_artifact(&line, selection.as_deref()) {
                    if !found.iter().any(|known| known.1 == artifact.1) {
                        found.push(artifact);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => closed = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if status.is_none() {
            status = group
                .child
                .try_wait()
                .map_err(|error| format!("could not wait for Cargo: {error}"))?;
        }
    }
    let _ = reader.join();
    let _ = std::io::stdout().flush();
    let status = status.ok_or("Cargo ended without a status")?;
    let target = match found.len() {
        1 => {
            let (_, program, manifest_dir) = found.remove(0);
            Ok((program, manifest_dir))
        }
        0 => Err("the build produced no binary to run; pass --bin or --example".to_owned()),
        _ => {
            let names: Vec<&str> = found.iter().map(|(name, _, _)| name.as_str()).collect();
            Err(format!(
                "several binaries were built ({}); pass --bin <name>",
                names.join(", ")
            ))
        }
    };
    Ok((status, Some(target)))
}

/// A bin or example artifact with an executable, matching any selection.
fn run_artifact(line: &str, selection: Option<&str>) -> Option<(String, PathBuf, PathBuf)> {
    let message: serde_json::Value = serde_json::from_str(line).ok()?;
    if message["reason"] != "compiler-artifact" {
        return None;
    }
    let executable = message["executable"].as_str()?;
    let name = message["target"]["name"].as_str()?;
    let runnable = message["target"]["kind"]
        .as_array()?
        .iter()
        .any(|kind| matches!(kind.as_str(), Some("bin" | "example")));
    if !runnable || selection.is_some_and(|selected| selected != name) {
        return None;
    }
    let manifest_dir = Path::new(message["manifest_path"].as_str()?).parent()?;
    Some((
        name.to_owned(),
        PathBuf::from(executable),
        manifest_dir.to_path_buf(),
    ))
}

/// One line: profile, strategy, slot warmth, elapsed time and pool use.
fn print_summary(
    store: &Store,
    invocation: &Invocation,
    slot: &str,
    warm: bool,
    started: Instant,
    note: Option<&str>,
) {
    let strategy = match invocation.command.as_str() {
        "run" | "test" => "build+execute",
        _ => "build",
    };
    let pool = store
        .payload_use()
        .map(|(used, total)| format!("pool {} of {} MiB used", used / MIB, total / MIB))
        .unwrap_or_else(|| "pool use unknown".to_owned());
    let note = note.map(|note| format!(", {note}")).unwrap_or_default();
    eprintln!(
        "oxide: {} profile, {strategy} strategy, slot {slot} {}, {:.1}s, {pool}{note}",
        invocation.profile.label(),
        if warm { "warm" } else { "cold" },
        started.elapsed().as_secs_f64()
    );
}

/// `cargo clean` against the private target directory and an empty private
/// build directory, then drop this checkout's slot affinity. Plain
/// `cargo clean` would delete an injected slot build directory. It waits for
/// active commands in the checkout and blocks new ones until it finishes.
fn run_clean(args: &[OsString]) -> Result<u8, String> {
    cli::refuse_storage_overrides(args)?;
    let manifest = cli::manifest_path(args);
    let workspace = identity::locate_workspace(CARGO, manifest.as_deref())?;
    process::install_signal_handlers()?;
    let store = Store::open_existing()?;
    let guard = match &store {
        Some(store) => match store.begin_clean(&workspace)? {
            Cleaning::Guard(guard) => Some(guard),
            Cleaning::Interrupted(signal) => return Ok(process::signal_exit_code(signal)),
        },
        None => None,
    };
    let scratch = private_scratch()?;
    let mut cargo = Command::new(CARGO);
    cargo.arg("clean").args(args);
    apply_storage(&mut cargo, &scratch.join("build"), &workspace);
    // The clean barrier must outlive this process if Cargo does: the child
    // inherits the clean lock and runs as a recorded group whose end is
    // awaited before the barrier lifts.
    if let Some(guard) = &guard {
        guard.share_with_children()?;
    }
    let status = process::spawn_group(&mut cargo).and_then(|mut group| {
        let recorded = match (&store, &guard) {
            (Some(store), Some(guard)) => store.clean_running(guard, group.id),
            _ => Ok(()),
        };
        let status = process::wait_leader(&mut group);
        process::wait_group_empty(group.id);
        recorded.and(status)
    });
    let _ = std::fs::remove_dir(scratch.join("build"));
    let _ = std::fs::remove_dir(&scratch);
    let detached = match (&store, guard) {
        (Some(store), Some(guard)) => store.finish_clean(guard, &workspace)?,
        _ => 0,
    };
    let status = status?;
    if detached > 0 {
        eprintln!(
            "oxide: dropped this checkout's slot affinity; {detached} slot(s) kept until evicted"
        );
    }
    Ok(process::exit_code(status))
}

fn private_scratch() -> Result<PathBuf, String> {
    let root = std::env::temp_dir();
    for attempt in 0..16_u32 {
        let path = root.join(format!("oxide-clean-{}-{attempt}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create a private clean directory: {error}")),
        }
    }
    Err("cannot allocate a private clean directory".to_owned())
}

fn run_status(args: &[OsString]) -> Result<u8, String> {
    if !args.is_empty() {
        return Err("status takes no arguments".to_owned());
    }
    println!("pool: {}", store::pool_path()?.display());
    match Store::open_existing()? {
        None => println!("  not created; the first building oxide command creates it"),
        Some(store) => {
            let report = store.report()?;
            let used = store
                .payload_use()
                .map(|(used, total)| format!("{} of {} MiB used", used / MIB, total / MIB))
                .unwrap_or_else(|| "use unknown".to_owned());
            println!(
                "  cap {} MiB, {used}, slot accounting {:?}",
                report.pool.cap_bytes / MIB,
                report.pool.coverage
            );
            println!(
                "  slots {}, waiting commands {}",
                report.slots.len(),
                report.waiting
            );
            for checkout in &report.cleaning {
                println!("  cleaning {}", checkout.display());
            }
            for slot in &report.slots {
                let state = match (slot.state, &slot.interrupted) {
                    (SlotOccupancy::Held, Some(interrupted)) => {
                        let holder = interrupted
                            .holder_pid
                            .map_or_else(|| "unknown".to_owned(), |pid| pid.to_string());
                        let path = match interrupted.path {
                            store::Return::NeverStarted => {
                                "Cargo never started; the next command recovers it".to_owned()
                            }
                            store::Return::GroupEmpty(group) => format!(
                                "Cargo group {group}; recovered once the group is empty"
                            ),
                            store::Return::GroupUnrecorded => {
                                "Cargo group not recorded; `oxide-storage-lease recover` releases it"
                                    .to_owned()
                            }
                            store::Return::NotOwn => {
                                "lease not provably this command's; `oxide-storage-lease recover` releases it"
                                    .to_owned()
                            }
                        };
                        format!("held by an interrupted command (oxide pid {holder}, {path})")
                    }
                    (SlotOccupancy::Held, None) => "held".to_owned(),
                    _ => "idle".to_owned(),
                };
                let owner = match (&slot.checkout, slot.managed) {
                    (Some(checkout), _) => checkout.display().to_string(),
                    (None, true) => "no checkout affinity".to_owned(),
                    (None, false) => "not managed by oxide".to_owned(),
                };
                let bytes = slot
                    .bytes
                    .map(|bytes| format!("{} KiB", bytes / 1024))
                    .unwrap_or_else(|| "size unknown".to_owned());
                println!(
                    "  slot {} {state} {} {owner} {bytes}",
                    slot.slot_id,
                    slot.profile.as_deref().unwrap_or("-")
                );
            }
            let now = process::now_ms();
            for recovery in &report.recoveries {
                let group = recovery.process_group.map_or_else(
                    || "Cargo never started".to_owned(),
                    |group| format!("Cargo group {group} empty"),
                );
                let checkout = recovery.checkout.as_deref().map_or_else(
                    || "no checkout affinity".to_owned(),
                    |checkout| checkout.display().to_string(),
                );
                println!(
                    "  recovered slot {} {}s ago: oxide pid {} died ({group}), {checkout}",
                    recovery.slot_id,
                    now.saturating_sub(recovery.recovered_ms) / 1000,
                    recovery.holder_pid
                );
            }
        }
    }
    match identity::locate_workspace(CARGO, None) {
        Ok(workspace) => report_stray(&workspace),
        Err(_) => println!("stray Cargo output: no Cargo workspace here"),
    }
    Ok(0)
}

/// Plain Cargo leaves `deps/` or `incremental/` in the checkout's `target/`;
/// Oxide keeps intermediates in its slot.
fn report_stray(workspace: &Path) {
    let target = workspace.join("target");
    if !target.is_dir() {
        println!("stray Cargo output: none ({} absent)", target.display());
        return;
    }
    let limits = InventoryLimits {
        max_entries: 50_000,
        max_depth: 24,
        max_output_bytes: 4096,
        max_elapsed_ms: 2_000,
    };
    match inventory_directory(&target, limits) {
        Ok(report) => {
            let stray: Vec<_> = report
                .categories
                .iter()
                .filter(|entry| matches!(entry.category, Category::Deps | Category::Incremental))
                .filter(|entry| entry.accounting.files > 0)
                .collect();
            let files: u64 = stray.iter().map(|entry| entry.accounting.files).sum();
            let bytes: u64 = stray
                .iter()
                .map(|entry| entry.accounting.allocated_bytes)
                .sum();
            let coverage = if report.coverage.complete {
                ""
            } else {
                " (partial scan)"
            };
            if files == 0 {
                println!("stray Cargo output: none in {}{coverage}", target.display());
            } else {
                println!(
                    "stray Cargo output: {} holds plain-Cargo deps/ or incremental/ ({files} files, {} MiB){coverage}; `oxide clean` removes it",
                    target.display(),
                    bytes / MIB
                );
            }
        }
        Err(error) => println!("stray Cargo output: scan unavailable ({error})"),
    }
}
