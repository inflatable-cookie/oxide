//! Unpublished `oxide` front door: Cargo with managed storage and profiles.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md`. Storage is the
//! existing lease pool; Oxide adds checkout affinity, FIFO waiting, a
//! host-wide limit and own-command crash recovery in a private control
//! directory. A checkout may take another checkout's idle compatible slot
//! only through the freshness gate. Plain `cargo` behavior is untouched.

mod adopt;
mod cargo_home;
mod clean;
mod cli;
mod config;
mod freshness;
mod identity;
mod launch;
mod process;
mod removable;
mod store;
mod watchdog;
mod wrapper;

use cli::{Class, Invocation, PROFILE_ENVIRONMENT};
use freshness::{Gate, Refusal};
use identity::{RunPick, SlotKey};
use oxide_storage::inventory::{inventory_directory, Category, InventoryLimits};
use oxide_storage::lease::{AccountingCoverage, PoolStatus, SlotOccupancy};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use store::{Acquired, Backend, Store};
use watchdog::Watchdog;

/// The `cargo` rustup resolves for the current directory. Oxide never selects
/// a toolchain.
pub(crate) const CARGO: &str = "cargo";
pub(crate) const MIB: u64 = 1024 * 1024;
const USAGE: &str = "\
oxide: Cargo with managed build storage and profiles

USAGE:
    oxide <check|test|clippy|build|run> [--profile quick|debug|release] [cargo args] [-- args]
    oxide <other cargo command> [args]   passed to Cargo; leased when it can build
    oxide clean [--dry-run] [--include-foreign]
                                         remove <workspace>/target and slot affinity; slots stay
    oxide clean <-p|--doc|--profile ...> Cargo's selected clean, same storage rules
    oxide clean --cargo-cache            one pass of Cargo's own home cleanup, no network
    oxide status                         pool, slots, waits, stray output and Cargo home
    oxide status --launch-check          macOS: time a new executable's first launch
    oxide pool register-removable <mount-point> --reserve <size> --cap <size>
                                         macOS: prefer a dedicated APFS volume under /Volumes
    oxide init [cargo init args] [path]  cargo init if no Cargo.toml, then adopt Oxide
    oxide new <path> [cargo new args]    cargo new, then adopt Oxide

Profiles: quick (default, line tables), debug (full debug info), release (the
project's own). Oxide places build output, so --target-dir and --config
build.build-dir/target-dir are refused. Adoption writes one marked block in the
workspace-root AGENTS.md and prints readiness; there is no Oxide config file.
A whole-target clean refuses entries Cargo does not create in target/ unless
--include-foreign is given; --dry-run prints every path it would remove.
OXIDE_POOL selects the pool; OXIDE_MAX_ACTIVE the host-wide build limit (2).
Leased builds bypass any inherited rustc wrapper (such as sccache);
OXIDE_ALLOW_RUSTC_WRAPPER=1 keeps it, unqualified for Oxide's interruption
and toolchain identity.
OXIDE_POOL_CAP sets a new pool's cap (bytes, KiB, MiB, GiB or TiB; default
24 GiB); an existing pool keeps its recorded cap.
OXIDE_POOL_BACKEND=directory-budget creates a budget-only directory pool (the
Linux default; macOS defaults to a hard-capped APFS image).
";

fn main() -> ExitCode {
    // The launch-check probe exits, and its measuring helper runs, before
    // any other work.
    if let Some(code) = launch::hidden() {
        return ExitCode::from(code);
    }
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
        Class::Pool => removable::run(rest),
        Class::Clean => clean::run(rest),
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

pub(crate) fn apply_storage(command: &mut Command, build_dir: &Path, checkout: &Path) {
    command
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .env("CARGO_TARGET_DIR", checkout.join("target"))
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (name, value) in PROFILE_ENVIRONMENT {
        command.env(name, value);
    }
    wrapper::apply(command);
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
    let selected = removable::select(Store::open_or_create()?)?;
    let store = selected.store;
    if let (false, Some(note)) = (selected.removable, &selected.note) {
        eprintln!("oxide: {note}");
    }
    let started = Instant::now();
    let resolution = cli::resolution(&invocation.cargo_args);
    let (held, gate) = match lease_with_gate(&store, &key, invocation.profile, &resolution)? {
        Ok(leased) => leased,
        Err(Stop::Signal(signal)) => return Ok(process::signal_exit_code(signal)),
        Err(Stop::Cargo(code)) => return Ok(code),
    };
    if let Some(signal) = process::received_signal() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(process::signal_exit_code(signal));
    }
    let run = command == "run";
    let config_args = if cli::reads_config_args(command) {
        &invocation.cargo_args[..]
    } else {
        &[]
    };
    let wrapper_note = wrapper::summary(
        wrapper::Policy::from_env(),
        &wrapper::inherited(config_args),
    );
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
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            return Err(error);
        }
    };
    let mut watchdog = store.watchdog();
    let outcome = match store.mark_running(&held, group.id) {
        Ok(()) if run => collect_run(&mut group, &invocation, watchdog.as_mut()),
        Ok(()) => match watchdog.as_mut() {
            Some(watchdog) => {
                process::wait_leader_watched(&mut group, watchdog).map(|status| (status, None))
            }
            None => process::wait_leader(&mut group).map(|status| (Some(status), None)),
        },
        Err(error) => {
            // SAFETY: the negative PID addresses the group this command created.
            unsafe {
                libc::kill(-group.id, libc::SIGTERM);
            }
            let _ = process::wait_leader(&mut group);
            Err(error)
        }
    };
    // Release only after positive closure. A watchdog that cannot confirm
    // the stopped group closed leaves the lease held and the record naming
    // the group, so a later command recovers it once the group is empty.
    let closed = match (watchdog.as_mut(), &outcome) {
        (Some(_), Ok((None, _))) => None,
        (Some(watchdog), _) => process::wait_group_empty_watched(group.id, watchdog),
        (None, _) => {
            process::wait_group_empty(group.id);
            Some(Instant::now())
        }
    };
    let stopped = watchdog.as_ref().filter(|watchdog| watchdog.stopped());
    if let Some(watchdog) = stopped {
        eprintln!("{}", watchdog.report(group.id, closed));
    }
    if closed.is_none() {
        eprintln!(
            "oxide: could not confirm Cargo group {} closed after the reserve stop; slot {} stays held, and a later oxide command recovers it once the group is empty",
            group.id, held.slot_id
        );
        return Ok(1);
    }
    let slot = held.slot_id.clone();
    let warm = held.reused;
    let note = [gate.describe(), wrapper_note]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let note = (!note.is_empty()).then_some(note);
    let origin = gate.finish(&key.checkout);
    store.release(held, origin)?;
    let (status, executable) = outcome?;
    let status = status.ok_or("Cargo ended without a status")?;
    print_summary(&store, &invocation, &slot, warm, started, note.as_deref());
    if store.backend == Backend::ApfsVolume {
        match store.last_flush() {
            Some(flush) if flush.slot_id == slot => match flush.micros {
                Some(micros) => eprintln!(
                    "oxide: flushed the removable volume before releasing slot {slot} in {:.1} ms",
                    micros as f64 / 1000.0
                ),
                None => eprintln!("oxide: slot {slot} is retired: its flush failed"),
            },
            _ => {}
        }
    }
    if stopped.is_some() {
        // A reserve stop is an ordinary failed command, never a success.
        return Ok(process::exit_code(status).max(1));
    }
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

/// Why a leased command ends before its Cargo runs.
enum Stop {
    /// A signal received while waiting.
    Signal(i32),
    /// The gate's metadata call hit the command's own lock refusal; Cargo's
    /// message is printed and this is its exit code.
    Cargo(u8),
}

/// Lease a slot and run the freshness gate. Cargo never runs on a slot the
/// gate cannot vouch for: a slot holding another checkout's content goes back
/// to that checkout unchanged; this checkout's own untrusted or unreadable
/// slot is evicted. Either way the command leases again without sharing and
/// builds cold. A `--locked` or `--frozen` lock refusal is the command's own
/// failure: the slot goes back unchanged and the command ends as plain Cargo
/// would.
fn lease_with_gate(
    store: &Store,
    key: &SlotKey,
    profile: cli::Profile,
    resolution: &cli::Resolution,
) -> Result<Result<(store::Lease, Gate), Stop>, String> {
    let mut share = true;
    // A refusal leaves this checkout without a record, so the next lease is
    // a new or evicted slot; the bound only guards against a defect.
    for _ in 0..4 {
        let mut held = match store.acquire(key, profile, share)? {
            Acquired::Lease(held) => held,
            Acquired::Interrupted(signal) => return Ok(Err(Stop::Signal(signal))),
        };
        let refusal = freshness::prepare(&key.checkout, &held.prior, resolution);
        // Taken just now, or left on this checkout's record by a command
        // that died before Cargo.
        let foreign = held.prior.foreign(&key.checkout);
        let reason = match refusal {
            Ok(gate) => return Ok(Ok((held, gate))),
            Err(Refusal::Unvouched(reason)) => reason,
            Err(Refusal::Lock { code, message }) => {
                eprint!("{message}");
                if held.shared_from.is_none() && foreign.is_some() {
                    held.shared_from = foreign;
                }
                // Metadata runs only for another checkout's slot.
                if held.shared_from.is_some() {
                    store.unshare(held)?;
                } else {
                    let origin = held.prior.unchanged();
                    store.release(held, origin)?;
                }
                return Ok(Err(Stop::Cargo(code)));
            }
        };
        share = false;
        if held.shared_from.is_some() || foreign.is_some() {
            eprintln!(
                "oxide: freshness gate refused another checkout's slot ({reason}); building cold"
            );
            if held.shared_from.is_none() {
                held.shared_from = foreign;
            }
            store.unshare(held)?;
        } else {
            eprintln!(
                "oxide: freshness gate cannot vouch for slot {} ({reason}); evicting it and building cold",
                held.slot_id
            );
            store.discard(held)?;
        }
    }
    Err("the freshness gate kept refusing slots".to_owned())
}

type RunTarget = Result<(PathBuf, PathBuf), String>;

/// `run`: wait for the build while reading Cargo's JSON for the executable.
fn collect_run(
    group: &mut process::Group,
    invocation: &Invocation,
    mut watchdog: Option<&mut Watchdog>,
) -> Result<(Option<ExitStatus>, Option<RunTarget>), String> {
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
        if let Some(watchdog) = watchdog.as_deref_mut() {
            watchdog.poll(group.id);
            if status.is_none() && watchdog.gave_up() {
                return Ok((None, None));
            }
        }
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
    let pool = match store.payload_use() {
        Some((used, budget)) => format!(
            "pool {} MiB allocated of {} MiB budget",
            used / MIB,
            budget / MIB
        ),
        None => "pool use unknown".to_owned(),
    };
    let backend = match store.backend {
        Backend::DirectoryBudget => format!(" ({}, budget-only)", store.backend.tag()),
        Backend::ApfsVolume => format!(
            " ({}, budget-only, removable {})",
            store.backend.tag(),
            store.payload.display()
        ),
    };
    let note = note.map(|note| format!(", {note}")).unwrap_or_default();
    eprintln!(
        "oxide: {} profile, {strategy} strategy, slot {slot} {}, {:.1}s, {pool}{backend}{note}",
        invocation.profile.label(),
        if warm { "warm" } else { "cold" },
        started.elapsed().as_secs_f64()
    );
}

fn run_status(args: &[OsString]) -> Result<u8, String> {
    match args {
        [] => {}
        [flag] if flag == "--launch-check" => return launch::run_check(),
        _ => return Err("status takes no arguments except --launch-check".to_owned()),
    }
    let settings = store::settings()?;
    print_settings(&settings);
    println!(
        "{}",
        wrapper::status_line(wrapper::Policy::from_env(), &wrapper::inherited(&[]))
    );
    match Store::open_existing() {
        Ok(None) => {
            println!(
                "  not created; the first building oxide command creates it ({}: {})",
                Backend::DirectoryBudget.tag(),
                Backend::DirectoryBudget.bound()
            );
        }
        Ok(Some(store)) => {
            print_pool(&store)?;
            if let Some(removable) = removable::status(&store)? {
                print_pool(&removable)?;
            }
        }
        Err(error) => println!("  cannot open: {error}"),
    }
    print_other_pools(&settings);
    match identity::locate_workspace(CARGO, None) {
        Ok(workspace) => report_stray(&workspace),
        Err(_) => println!("stray Cargo output: no Cargo workspace here"),
    }
    cargo_home::report_status();
    Ok(0)
}

/// Backend, use, slots and recoveries of one opened pool.
fn print_pool(store: &Store) -> Result<(), String> {
    let report = store.report()?;
    println!(
        "  backend {}: {}",
        store.backend.tag(),
        store.backend.bound()
    );
    match store.backend {
        Backend::DirectoryBudget => {
            let used = slot_use(&report.pool);
            let available = watchdog::available_bytes(&store.payload).map_or_else(
                || "unknown".to_owned(),
                |bytes| format!("{} MiB", bytes / MIB),
            );
            println!(
                "  soft budget {} MB (checked at admission; a running build may grow past it), {used}, slot accounting {:?}",
                store.budget(report.pool.cap_bytes) / 1_000_000,
                report.pool.coverage
            );
            let physical = store::statfs(&store.payload).and_then(|sample| {
                let block = u64::try_from(sample.f_bsize).ok()?;
                Some((
                    sample.f_blocks.checked_mul(block)?,
                    sample.f_bavail.checked_mul(block)?,
                ))
            });
            match physical {
                Some((capacity, available)) => println!(
                    "  filesystem: {} MiB available of {} MiB capacity (shared; no Oxide quota or hard ceiling)",
                    available / MIB,
                    capacity / MIB
                ),
                None => println!("  filesystem: capacity unknown (no Oxide quota or hard ceiling)"),
            }
            println!("  Time Machine: {}", backup_line(&store.pool));
            println!("  Spotlight: {}", config::spotlight_advice(&store.pool));
            println!(
                "  reserve watchdog: stops a build below {} GiB available ({} GiB reserve + {} GiB margin), best effort; pool filesystem available {available}",
                (store::HOST_RESERVE_BYTES + watchdog::MARGIN_BYTES) / store::GIB,
                store::HOST_RESERVE_BYTES / store::GIB,
                watchdog::MARGIN_BYTES / store::GIB
            );
        }
        Backend::ApfsVolume => {
            let used = slot_use(&report.pool);
            println!(
                "  payload {}, admission cap {} MiB, {used}, slot accounting {:?}",
                store.payload.display(),
                report.pool.cap_bytes / MIB,
                report.pool.coverage
            );
            for flush in &report.flushes {
                let outcome = match (flush.micros, &flush.error) {
                    (Some(micros), _) => format!("{:.1} ms", micros as f64 / 1000.0),
                    (None, Some(error)) => format!("failed ({error}); slot retired"),
                    (None, None) => "failed; slot retired".to_owned(),
                };
                println!(
                    "  flush before releasing slot {}: {outcome}, {}s ago",
                    flush.slot_id,
                    process::now_ms().saturating_sub(flush.at_ms) / 1000
                );
            }
        }
    }
    println!("  slot accounting walk: {}", report.pool.payload_accounting);
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
            (SlotOccupancy::Held, _) if slot.retired.is_some() => format!(
                "retired and held, never reused ({})",
                slot.retired.as_deref().unwrap_or_default()
            ),
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
                        "Cargo group not recorded; `oxide pool recover-interrupted <slot>` releases it"
                            .to_owned()
                    }
                    store::Return::NotOwn => {
                        "lease not provably this command's; `oxide pool recover-interrupted <slot>` releases it"
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
        let bytes = slot_size(&store.payload, &slot.slot_id);
        println!(
            "  slot {} {state} {} {owner} {bytes}",
            slot.slot_id,
            slot.profile.as_deref().unwrap_or("-")
        );
        let incomplete = report
            .pool
            .slots
            .iter()
            .find(|pooled| pooled.slot_id == slot.slot_id)
            .map(|pooled| &pooled.accounting)
            .filter(|scan| scan.coverage != AccountingCoverage::Complete);
        if let Some(scan) = incomplete {
            println!(
                "    slot {} accounting walk: {scan}; it is not reused while incomplete",
                slot.slot_id
            );
        }
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
    Ok(())
}

/// Slot allocation from the payload walk; an incomplete walk is a lower
/// bound and says so.
fn slot_use(pool: &PoolStatus) -> String {
    match (
        pool.payload_unique_allocated_bytes,
        pool.payload_accounting.coverage,
    ) {
        (Some(used), AccountingCoverage::Complete) => {
            format!("{} MiB allocated by slots", used / MIB)
        }
        (Some(used), _) => format!("at least {} MiB allocated by slots", used / MIB),
        (None, _) => "slot allocation unknown".to_owned(),
    }
}

/// File names, distinct inodes and allocation counted once per inode for
/// one slot. Reporting only: nothing inside a slot is ever deleted. The
/// slot must resolve to itself under the canonical pool, so a link never
/// leads the walk elsewhere.
fn slot_size(payload: &Path, slot_id: &str) -> String {
    if slot_id.is_empty() || !slot_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return "size unknown".to_owned();
    }
    let slot = payload.join("slots").join(slot_id);
    match std::fs::canonicalize(&slot) {
        Ok(resolved) if resolved == slot => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return "empty".to_owned(),
        _ => return "size unknown (the slot path does not resolve to itself)".to_owned(),
    }
    match cargo_home::measure(&slot) {
        Ok(Some(measure)) => measure.describe(),
        Ok(None) => "empty".to_owned(),
        Err(error) => format!("size unknown ({error})"),
    }
}

/// Plain Cargo leaves `deps/` or `incremental/` in the checkout's `target/`;
/// Oxide keeps intermediates in its slot. Finding none means none only for a
/// verified Cargo release, whose layout was checked.
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
            if files > 0 {
                println!(
                    "stray Cargo output: {} holds plain-Cargo deps/ or incremental/ ({files} files, {} MiB){coverage}; `oxide clean` removes it",
                    target.display(),
                    bytes / MIB
                );
            } else if let Err(reason) = freshness::verified_cargo() {
                // Another layout may keep intermediates elsewhere.
                println!(
                    "stray Cargo output: unknown in {} ({reason}; its layout is unchecked)",
                    target.display()
                );
            } else {
                println!("stray Cargo output: none in {}{coverage}", target.display());
            }
        }
        Err(error) => println!("stray Cargo output: scan unavailable ({error})"),
    }
}

/// Config file, path and budget with where each came from. Never prints
/// file contents or unrelated environment values.
fn print_settings(settings: &config::Settings) {
    let file = &settings.file;
    println!(
        "config: {} ({}{})",
        file.path.display(),
        if file.explicit {
            "selected by OXIDE_CONFIG"
        } else {
            "platform location"
        },
        if file.present { "" } else { ", absent" }
    );
    if let Some(ignored) = config::ignored_legacy_config(&config::Facts::from_env()) {
        println!(
            "  ignored: {} exists but is never read on macOS; move its settings to the file above",
            ignored.display()
        );
    }
    println!(
        "pool: {} (from {})",
        settings.pool.display(),
        settings.pool_source.describe()
    );
    println!(
        "  soft budget {} MB (from {})",
        settings.budget / 1_000_000,
        settings.budget_source.describe()
    );
}

fn backup_line(pool: &Path) -> String {
    match config::backup_state(pool) {
        config::Backup::Excluded => "excluded (tmutil isexcluded)".to_owned(),
        config::Backup::NotExcluded(reason) => format!("NOT excluded ({reason})"),
        config::Backup::NotApplicable => "not applicable on this platform".to_owned(),
    }
}

/// Pools Oxide knows of at other paths: the retired image pool's default
/// and pools it created before a path change. Read-only and informational;
/// Oxide never moves or deletes them.
fn print_other_pools(settings: &config::Settings) {
    let facts = config::Facts::from_env();
    let current = std::fs::canonicalize(&settings.pool).unwrap_or_else(|_| settings.pool.clone());
    let mut seen = Vec::new();
    let candidates = config::legacy_default_pool(&facts)
        .into_iter()
        .chain(config::default_pool(&facts))
        .chain(config::known_pools(&facts));
    for path in candidates {
        let resolved = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if resolved == current || seen.contains(&resolved) {
            continue;
        }
        seen.push(resolved);
        let registered = path
            .join(oxide_storage::lease::MANAGED_NAME)
            .join("removable.json");
        let removable = if std::fs::symlink_metadata(&registered).is_ok() {
            "; holds a removable registration Oxide does not use from here"
        } else {
            ""
        };
        match config::inspect(&path) {
            config::Found::Absent => {}
            config::Found::LegacyImage => println!(
                "legacy pool: {} (retired apfs-sparse-image-v1; not used, attached or written; pending planner retirement{removable})",
                path.display()
            ),
            config::Found::Directory => println!(
                "orphaned pool: {} (not the current pool; Oxide never moves or deletes it{removable})",
                path.display()
            ),
            config::Found::Unknown => println!(
                "other pool path: {} (unrecognised; left alone)",
                path.display()
            ),
        }
    }
}
