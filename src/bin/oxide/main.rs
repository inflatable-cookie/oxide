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

use cli::{Class, Invocation, Profile, LEGACY_PROFILE_ENVIRONMENT};
use freshness::{Gate, Refusal};
use identity::{RunPick, SlotKey};
use oxide_storage::inventory::{inventory_directory, Category, InventoryLimits};
use oxide_storage::lease::{AccountingCoverage, PoolStatus, ReuseMeasurement, SlotOccupancy};
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
    oxide exec [--manifest-path <Cargo.toml>] [--profile quick|debug|release] -- <command> [args]
                                         run a tool that calls Cargo itself, leased
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

Profiles: quick (default, line tables) and debug (full debug info) tune Cargo's
dev profile, so outputs land in target/debug as with plain Cargo; release is the
project's own. --cargo-profile oxide-quick/oxide-debug still build but are
deprecated: external tools get the quick recipe without them. Oxide places build output, so --target-dir and --config
build.build-dir/target-dir are refused. Adoption writes one marked block in the
workspace-root AGENTS.md and prints readiness; adoption does not create an
Oxide config file.
A whole-target clean refuses entries Cargo does not create in target/ unless
--include-foreign is given; --dry-run prints every path it would remove.
OXIDE_POOL selects the pool; OXIDE_MAX_ACTIVE sets the host-wide active-build
limit (1–32), ahead of [pool] max_active in the user config. The automatic
macOS default is min(4, max(2, floor(physical cores/4))); other platforms and
unavailable core counts use 2.
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
        Class::Exec => match cli::parse_exec(rest)? {
            Some(invocation) => run_invocation(invocation),
            None => {
                print!("{}", cli::EXEC_USAGE);
                Ok(0)
            }
        },
        Class::Modelled | Class::Building => run_leased(command, rest),
    }
}

/// Replace Oxide with plain Cargo. Returns only on failure.
fn exec_cargo(args: &[OsString]) -> String {
    let error = Command::new(CARGO).args(args).exec();
    format!("could not start Cargo: {error}")
}

pub(crate) fn apply_storage(
    command: &mut Command,
    build_dir: &Path,
    checkout: &Path,
    profile: Profile,
) {
    command
        .env("CARGO_BUILD_BUILD_DIR", build_dir)
        .env("CARGO_TARGET_DIR", checkout.join("target"))
        .env_remove("CARGO_BUILD_TARGET_DIR");
    for (name, value) in profile.dev_recipe() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    for (name, value) in LEGACY_PROFILE_ENVIRONMENT {
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
    run_invocation(invocation)
}

/// What `oxide status` shows as an exec holder: `exec <program>`, escaped
/// and bounded to one line.
fn exec_label(tool: &[OsString]) -> Option<String> {
    let program = tool.first()?;
    Some(format!(
        "exec {}",
        wrapper::shown(&program.to_string_lossy())
    ))
}

/// The tool's standard input: a terminal stays with Oxide, because the
/// tool's own process group is in the background and a terminal read would
/// stop it. Anything else (a pipe, a file, nothing) passes through.
fn exec_stdin() -> Stdio {
    // SAFETY: isatty only inspects descriptor 0.
    if unsafe { libc::isatty(0) } == 1 {
        Stdio::null()
    } else {
        Stdio::inherit()
    }
}

fn run_invocation(invocation: Invocation) -> Result<u8, String> {
    let exec = invocation.command == "exec";
    let key = SlotKey::resolve(
        CARGO,
        invocation.manifest_path.as_deref(),
        invocation.profile.recipe_key(),
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
    let label = exec_label(&invocation.tool);
    let (held, gate) = match lease_with_gate(
        &store,
        &key,
        invocation.profile,
        &resolution,
        label.as_deref(),
    )? {
        Ok(leased) => leased,
        Err(Stop::Signal(signal)) => return Ok(process::signal_exit_code(signal)),
        Err(Stop::Cargo(code)) => return Ok(code),
    };
    if let Some(signal) = process::received_signal() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(process::signal_exit_code(signal));
    }
    let run = invocation.command == "run";
    let config_args = if cli::reads_config_args(&invocation.command) {
        &invocation.cargo_args[..]
    } else {
        &[]
    };
    let wrapper_note = wrapper::summary(
        wrapper::Policy::from_env(),
        &wrapper::inherited(config_args),
    );
    let mut cargo = match invocation.tool.split_first() {
        Some((program, args)) => {
            let mut tool = Command::new(program);
            tool.args(args).stdin(exec_stdin());
            tool
        }
        None => {
            let mut cargo = Command::new(CARGO);
            cargo.args(&invocation.cargo_args);
            cargo
        }
    };
    apply_storage(
        &mut cargo,
        &held.build_dir,
        &key.checkout,
        invocation.profile,
    );
    if run {
        cargo.stdout(Stdio::piped());
    }
    let started_group = store
        .mark_spawning(&held)
        .map_err(Unstarted::Record)
        .and_then(|()| process::start_group(&mut cargo).map_err(Unstarted::Spawn));
    let mut group = match started_group {
        Ok(group) => group,
        Err(error) => {
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            return match error {
                Unstarted::Record(error) => Err(error),
                Unstarted::Spawn(error) if exec => {
                    eprintln!(
                        "oxide: could not start {}: {error}",
                        wrapper::shown(&invocation.tool[0].to_string_lossy())
                    );
                    // A shell's convention: 127 not found, 126 not runnable.
                    Ok(if error.kind() == std::io::ErrorKind::NotFound {
                        127
                    } else {
                        126
                    })
                }
                Unstarted::Spawn(error) => Err(format!("could not start Cargo: {error}")),
            };
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
        if stopped.is_some_and(Watchdog::removal) {
            store.quarantine_lost(
                &held,
                &format!("Cargo group {} did not close after KILL", group.id),
            )?;
            return Ok(1);
        }
        eprintln!(
            "oxide: could not confirm Cargo group {} closed after the reserve stop; slot {} stays held, and a later oxide command recovers it once the group is empty",
            group.id, held.slot_id
        );
        return Ok(1);
    }
    let slot = held.slot_id.clone();
    let warm = held.reused;
    let measured = held.measured.clone();
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
    print_summary(
        &store,
        &invocation,
        &slot,
        warm,
        measured.as_ref(),
        started,
        note.as_deref(),
    );
    if store.backend == Backend::ApfsVolume {
        match store.last_flush() {
            Some(flush) if flush.slot_id == slot => match flush.micros {
                Some(micros) => eprintln!(
                    "oxide: flushed the removable volume before releasing slot {slot} in {:.1} ms",
                    micros as f64 / 1000.0
                ),
                None if flush.lost => eprintln!(
                    "oxide: slot {slot} was lost with the removable volume; the next command uses the internal pool until it returns"
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

/// Why the leased child did not start.
enum Unstarted {
    /// Recording the spawn failed; nothing was started.
    Record(String),
    /// The program could not be started.
    Spawn(std::io::Error),
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
    label: Option<&str>,
) -> Result<Result<(store::Lease, Gate), Stop>, String> {
    let mut share = true;
    // A refusal leaves this checkout without a record, so the next lease is
    // a new or evicted slot; the bound only guards against a defect.
    for _ in 0..4 {
        let mut held = match store.acquire(key, profile, share, label)? {
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

/// One line: profile, strategy, slot warmth, elapsed time, the slot's size
/// when leased and this command's budget. It walks nothing: the size is the
/// library's reuse walk of that exact slot generation, never a walk after
/// release, when another command may hold the slot.
fn print_summary(
    store: &Store,
    invocation: &Invocation,
    slot: &str,
    warm: bool,
    measured: Option<&ReuseMeasurement>,
    started: Instant,
    note: Option<&str>,
) {
    let strategy = match invocation.command.as_str() {
        "run" | "test" | "exec" => "build+execute",
        _ => "build",
    };
    let size = match measured {
        Some(measured) => format!(
            "slot {} was {} MiB when leased",
            measured.slot_id,
            measured.unique_allocated_bytes / MIB
        ),
        None if !warm => "new slot".to_owned(),
        None => "slot size unknown".to_owned(),
    };
    let pool = format!(
        "{size}, {} MiB budget, pool total: see oxide status",
        store.command_budget() / MIB
    );
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
    match Store::open_existing_with(&settings) {
        Ok(None) => {
            println!(
                "  not created; the first building oxide command creates it ({}: {})",
                Backend::DirectoryBudget.tag(),
                Backend::DirectoryBudget.bound()
            );
        }
        Ok(Some(store)) => print_pools(&store)?,
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

/// The pool selected for the next build first, then the other; the most
/// recent recorded use across both; each pool's own slots under its header.
/// Read-only: the same selection check builds use, no acquisition.
fn print_pools(internal: &Store) -> Result<(), String> {
    let removable = removable::status(internal)?;
    let internal_report = internal.report()?;
    let removable_report = match removable.as_ref().map(|status| &status.selected) {
        Some(Ok(store)) => Some(store.report()?),
        _ => None,
    };
    let reason = removable
        .as_ref()
        .and_then(|status| status.selected.as_ref().err());
    println!(
        "{}",
        selection_line(removable.is_some(), reason.map(String::as_str))
    );
    let removable_use = match (&removable, &removable_report) {
        (None, _) => None,
        (Some(_), Some(report)) => Some(Ok(report.last_use.clone())),
        (Some(status), None) => Some(store::recorded_last_use(&status.control)),
    };
    let mut uses = vec![("internal", Ok(internal_report.last_use.clone()))];
    if let Some(removable_use) = removable_use {
        uses.push(("removable", removable_use));
    }
    println!("{}", recent_use_line(&uses, process::now_ms()));

    for section in sections(removable.is_some(), reason.map(String::as_str)) {
        match section {
            Section::Internal(note) => {
                let header = format!("internal pool: {}{note}", internal.pool.display());
                print_pool(internal, &header, &[], &internal_report)?;
            }
            Section::NoRemovable => println!("removable pool: none registered"),
            Section::Removable => {
                let (Some(status), Some(report)) = (&removable, &removable_report) else {
                    return Err("the removable pool was not opened".to_owned());
                };
                let Ok(store) = &status.selected else {
                    return Err("the removable pool was not opened".to_owned());
                };
                let header = format!(
                    "removable pool: {} (selected for the next build)",
                    status.lines[0]
                );
                print_pool(store, &header, &status.lines[1..], report)?;
            }
            Section::RemovableUnread(reason) => {
                let Some(status) = &removable else {
                    return Err("the removable pool is not registered".to_owned());
                };
                println!(
                    "removable pool: {} (not used for the next build: {reason})",
                    status.lines[0]
                );
                for line in &status.lines[1..] {
                    println!("{line}");
                }
                println!("  slots: not read while the volume is not usable (unknown, not empty)");
                match store::recorded_lost(&status.control) {
                    Ok(lost) => {
                        for (slot, checkout, what) in lost {
                            let checkout = checkout.map_or_else(
                                || "no checkout affinity".to_owned(),
                                |checkout| checkout.display().to_string(),
                            );
                            println!("  slot {slot} ({checkout}): {what}");
                        }
                    }
                    Err(error) => println!("  lost slots: unknown ({error})"),
                }
            }
        }
    }
    Ok(())
}

/// One pool block of `status`, in display order.
#[derive(Debug, Eq, PartialEq)]
enum Section {
    /// The internal pool, with its role note.
    Internal(&'static str),
    Removable,
    /// Registered but not selected: its slots are not read.
    RemovableUnread(String),
    NoRemovable,
}

/// The pool selected for the next build comes first; each pool is one
/// block holding only its own slots.
fn sections(registered: bool, refused: Option<&str>) -> Vec<Section> {
    const SELECTED: &str = " (selected for the next build)";
    match (registered, refused) {
        (false, _) => vec![Section::Internal(SELECTED), Section::NoRemovable],
        (true, None) => vec![
            Section::Removable,
            Section::Internal(" (fallback while the removable volume is not usable)"),
        ],
        (true, Some(reason)) => vec![
            Section::Internal(SELECTED),
            Section::RemovableUnread(reason.to_owned()),
        ],
    }
}

/// Which pool the next build uses, from the same check builds make.
fn selection_line(registered: bool, refused: Option<&str>) -> String {
    match (registered, refused) {
        (false, _) => {
            "selected for the next build: internal pool (no removable volume registered)".to_owned()
        }
        (true, None) => "selected for the next build: removable pool".to_owned(),
        (true, Some(reason)) => format!(
            "selected for the next build: internal pool (removable volume not used: {reason})"
        ),
    }
}

/// The most recently used slot across pools, from each pool's recorded
/// latest use. Pools are listed in display order; equal times name the
/// first and say so. An unreadable pool or a use with no recorded time
/// makes the cross-pool answer unknown rather than guessed.
fn recent_use_line(uses: &[(&str, Result<Option<store::LastUse>, String>)], now_ms: u64) -> String {
    let describe = |pool: &str, last: &store::LastUse| {
        let slot = last
            .slot_id
            .as_deref()
            .map_or_else(|| "slot unknown".to_owned(), |slot| format!("slot {slot}"));
        let label = last.label.as_deref().unwrap_or("project unknown");
        let checkout = last.checkout.as_deref().map_or_else(
            || "no checkout affinity".to_owned(),
            |path| path.display().to_string(),
        );
        let age = last.at_ms.map_or_else(
            || "time not recorded".to_owned(),
            |at| format!("{}s ago", now_ms.saturating_sub(at) / 1000),
        );
        format!("{pool} pool {slot}, project {label} ({checkout}), {age}")
    };
    let mut known = Vec::new();
    for (pool, last) in uses {
        match last {
            Err(error) => {
                return format!("most recent use: unknown ({pool} pool use not readable: {error})")
            }
            Ok(None) => {}
            Ok(Some(last)) => known.push((*pool, last)),
        }
    }
    match known.as_slice() {
        [] => "most recent use: none recorded".to_owned(),
        [(pool, last)] => format!("most recent use: {}", describe(pool, last)),
        _ => {
            if known.iter().any(|(_, last)| last.at_ms.is_none()) {
                let each: Vec<String> = known
                    .iter()
                    .map(|(pool, last)| describe(pool, last))
                    .collect();
                return format!(
                    "most recent use: unknown across pools (a use time was not recorded); latest per pool: {}",
                    each.join("; ")
                );
            }
            let latest = known
                .iter()
                .map(|(_, last)| last.at_ms.unwrap_or(0))
                .max()
                .unwrap_or(0);
            let tied: Vec<&(&str, &store::LastUse)> = known
                .iter()
                .filter(|(_, last)| last.at_ms == Some(latest))
                .collect();
            let (pool, last) = tied[0];
            let tie = if tied.len() > 1 {
                " (tied with the other pool's latest use)"
            } else {
                ""
            };
            format!("most recent use: {}{tie}", describe(pool, last))
        }
    }
}

/// Backend, use, slots and recoveries of one opened pool, under `header`.
fn print_pool(
    store: &Store,
    header: &str,
    details: &[String],
    report: &store::Report,
) -> Result<(), String> {
    println!("{header}");
    for line in details {
        println!("{line}");
    }
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
                let command = slot.holder.as_deref().unwrap_or("command");
                format!("held by an interrupted {command} (oxide pid {holder}, {path})")
            }
            (SlotOccupancy::Held, None) => match &slot.holder {
                Some(command) => format!("held by {command}"),
                None => "held".to_owned(),
            },
            _ => "idle".to_owned(),
        };
        let owner = match (&slot.checkout, slot.managed) {
            (Some(checkout), _) => checkout.display().to_string(),
            (None, true) => "no checkout affinity".to_owned(),
            (None, false) => "not managed by oxide".to_owned(),
        };
        let bytes = slot_size(&store.payload, &slot.slot_id, slot.dir.as_deref());
        println!("{}", slot_line(slot, &state, &owner, &bytes));
        let diagnostic_scan = report
            .pool
            .slots
            .iter()
            .find(|pooled| pooled.slot_id == slot.slot_id)
            .map(|pooled| &pooled.accounting)
            .filter(|scan| scan.coverage != AccountingCoverage::Complete || scan.vanished > 0);
        if let Some(scan) = diagnostic_scan {
            if scan.coverage != AccountingCoverage::Complete {
                println!(
                    "    slot {} accounting walk: {scan}; it is not reused while incomplete",
                    slot.slot_id
                );
            } else {
                println!(
                    "    slot {} accounting walk: {scan}; this best-effort observation does not infer freed allocation",
                    slot.slot_id
                );
            }
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

/// One slot under its pool's header: ID, state, profile, project label,
/// checkout, size and its place below the pool's `slots`.
fn slot_line(slot: &store::SlotView, state: &str, owner: &str, bytes: &str) -> String {
    let place = slot.dir.as_deref().map_or_else(
        || format!("slots/{} (legacy path)", slot.slot_id),
        |dir| format!("slots/{dir}"),
    );
    format!(
        "  slot {} {state} {} project {} {owner} {bytes}, {place}",
        slot.slot_id,
        slot.profile.as_deref().unwrap_or("-"),
        slot.label.as_deref().unwrap_or("unknown")
    )
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
fn slot_size(payload: &Path, slot_id: &str, dir: Option<&str>) -> String {
    if slot_id.is_empty() || !slot_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return "size unknown".to_owned();
    }
    let slot = match dir {
        None => payload.join("slots").join(slot_id),
        Some(dir)
            if dir.split('/').count() == 2
                && !dir
                    .split('/')
                    .any(|part| part.is_empty() || part.starts_with('.')) =>
        {
            payload.join("slots").join(dir)
        }
        Some(_) => return "size unknown".to_owned(),
    };
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
    if let Some(ignored) = &settings.ignored_legacy_config {
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
    println!(
        "host-wide active-build limit: {} (from {})",
        settings.active_build_limit.value,
        settings.active_build_limit.provenance(&settings.file.path)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;

    #[test]
    fn selected_pool_comes_first() {
        assert_eq!(
            sections(true, None),
            vec![
                Section::Removable,
                Section::Internal(" (fallback while the removable volume is not usable)")
            ]
        );
        assert_eq!(
            sections(false, None),
            vec![
                Section::Internal(" (selected for the next build)"),
                Section::NoRemovable
            ]
        );
        assert_eq!(
            sections(true, Some("volume not mounted")),
            vec![
                Section::Internal(" (selected for the next build)"),
                Section::RemovableUnread("volume not mounted".to_owned())
            ]
        );
        assert_eq!(
            selection_line(true, None),
            "selected for the next build: removable pool"
        );
        assert!(selection_line(true, Some("below reserve"))
            .ends_with("(removable volume not used: below reserve)"));
        assert!(
            selection_line(false, None).contains("internal pool (no removable volume registered)")
        );
    }

    fn view(slot_id: &str, dir: Option<&str>, checkout: &str) -> store::SlotView {
        store::SlotView {
            slot_id: slot_id.to_owned(),
            dir: dir.map(str::to_owned),
            label: store::slot_label(dir, Some(Path::new(checkout))),
            state: SlotOccupancy::Idle,
            profile: Some("quick".to_owned()),
            checkout: Some(PathBuf::from(checkout)),
            managed: true,
            interrupted: None,
            retired: None,
            holder: None,
        }
    }

    /// Equal slot numbers in two pools stay distinct by label and place;
    /// a legacy slot shows its checkout's name and legacy path.
    #[test]
    fn slot_lines_name_project_and_place() {
        let internal = view("1", None, "/work/My App");
        let removable = view("1", Some("underlay-0a1b2c3d/quick-1"), "/work/underlay");
        let internal_line = slot_line(&internal, "idle", "/work/My App", "empty");
        let removable_line = slot_line(&removable, "idle", "/work/underlay", "empty");
        assert!(
            internal_line.contains("project my-app ")
                && internal_line.ends_with("slots/1 (legacy path)"),
            "{internal_line}"
        );
        assert!(
            removable_line.contains("project underlay-0a1b2c3d ")
                && removable_line.ends_with("slots/underlay-0a1b2c3d/quick-1"),
            "{removable_line}"
        );
        let unknown = store::SlotView {
            label: None,
            checkout: None,
            ..view("2", None, "/x")
        };
        assert!(slot_line(&unknown, "idle", "no checkout affinity", "empty")
            .contains("project unknown"));
    }

    fn last(slot: &str, label: &str, at_ms: Option<u64>) -> store::LastUse {
        store::LastUse {
            slot_id: Some(slot.to_owned()),
            label: Some(label.to_owned()),
            checkout: Some(PathBuf::from(format!("/work/{label}"))),
            at_ms,
        }
    }

    #[test]
    fn most_recent_use_names_pool_and_project() {
        let now = 100_000;
        let line = recent_use_line(
            &[
                ("internal", Ok(Some(last("1", "oxide-aa", Some(50_000))))),
                (
                    "removable",
                    Ok(Some(last("3", "underlay-bb", Some(90_000)))),
                ),
            ],
            now,
        );
        assert_eq!(
            line,
            "most recent use: removable pool slot 3, project underlay-bb (/work/underlay-bb), 10s ago"
        );
        let line = recent_use_line(
            &[
                (
                    "removable",
                    Ok(Some(last("3", "underlay-bb", Some(10_000)))),
                ),
                ("internal", Ok(Some(last("1", "oxide-aa", Some(50_000))))),
            ],
            now,
        );
        assert!(
            line.starts_with("most recent use: internal pool slot 1, project oxide-aa"),
            "{line}"
        );
        // Ties name the first listed pool, the selected one, and say so.
        let line = recent_use_line(
            &[
                ("removable", Ok(Some(last("1", "b", Some(70_000))))),
                ("internal", Ok(Some(last("1", "a", Some(70_000))))),
            ],
            now,
        );
        assert!(
            line.contains("removable pool slot 1, project b")
                && line.ends_with("(tied with the other pool's latest use)"),
            "{line}"
        );
        // Unknowns are said, never guessed.
        let line = recent_use_line(
            &[
                ("internal", Ok(Some(last("1", "a", Some(70_000))))),
                ("removable", Ok(Some(last("2", "b", None)))),
            ],
            now,
        );
        assert!(
            line.starts_with("most recent use: unknown across pools"),
            "{line}"
        );
        let line = recent_use_line(
            &[
                ("internal", Ok(Some(last("1", "a", Some(70_000))))),
                ("removable", Err("no Oxide control state".to_owned())),
            ],
            now,
        );
        assert_eq!(
            line,
            "most recent use: unknown (removable pool use not readable: no Oxide control state)"
        );
        let unread = store::LastUse {
            slot_id: None,
            ..last("9", "b", Some(99_000))
        };
        let line = recent_use_line(
            &[("internal", Ok(None)), ("removable", Ok(Some(unread)))],
            now,
        );
        assert!(
            line.contains("removable pool slot unknown, project b"),
            "{line}"
        );
        assert_eq!(
            recent_use_line(&[("internal", Ok(None))], now),
            "most recent use: none recorded"
        );
        let legacy = recent_use_line(&[("internal", Ok(Some(last("1", "a", None))))], now);
        assert!(legacy.ends_with("time not recorded"), "{legacy}");
    }

    fn environment(profile: Profile) -> HashMap<String, Option<String>> {
        let mut command = Command::new(CARGO);
        command.env("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG", "full");
        apply_storage(
            &mut command,
            Path::new("/pool/slot/build"),
            Path::new("/work"),
            profile,
        );
        command
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(OsStr::to_string_lossy).map(Into::into),
                )
            })
            .collect()
    }

    #[test]
    fn leased_children_get_the_dev_recipe_and_legacy_definitions() {
        let set = |value: &str| Some(Some(value.to_owned()));
        let quick = environment(Profile::Quick);
        assert_eq!(quick.get("CARGO_TARGET_DIR"), set("/work/target").as_ref());
        assert_eq!(
            quick.get("CARGO_PROFILE_DEV_DEBUG"),
            set("line-tables-only").as_ref()
        );
        // An outer debug command's build override never reaches quick.
        assert_eq!(
            quick.get("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG"),
            Some(&None)
        );
        let debug = environment(Profile::Debug);
        assert_eq!(debug.get("CARGO_PROFILE_DEV_DEBUG"), set("full").as_ref());
        assert_eq!(
            debug.get("CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG"),
            set("full").as_ref()
        );
        let release = environment(Profile::Release);
        assert_eq!(release.get("CARGO_PROFILE_DEV_DEBUG"), None);
        for profile in [quick, debug, release] {
            for (name, value) in LEGACY_PROFILE_ENVIRONMENT {
                assert_eq!(profile.get(name), set(value).as_ref(), "{name}");
            }
        }
    }
}
