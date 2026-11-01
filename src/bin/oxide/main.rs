//! Unpublished `oxide` front door: Cargo with managed storage and profiles.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md`. Storage is the
//! existing lease pool; Oxide adds checkout affinity, FIFO waiting, a
//! host-wide limit and own-command crash recovery in a private control
//! directory. A checkout may take another checkout's idle compatible slot
//! only through the freshness gate. Plain `cargo` behavior is untouched.

mod admission;
mod adopt;
mod budget;
mod capacity;
mod cargo_home;
mod child_temp;
mod clean;
mod cli;
mod config;
mod describe;
mod freshness;
mod guidance;
mod handoff;
mod identity;
mod incremental;
mod launch;
mod layout;
mod one_use;
mod outputs;
mod process;
mod prune;
mod removable;
#[allow(dead_code)] // Integrated by the cold-seeding front door.
mod seed_catalog;
#[allow(dead_code)] // Integrated by the cold-seeding front door.
mod seed_context;
#[allow(dead_code)] // Integrated by the cold-seeding front door.
mod seed_lock;
#[allow(dead_code)] // Integrated by the cold-seeding front door.
mod seed_plan;
#[allow(dead_code)] // Integrated by the cold-seeding front door.
mod seed_sources;
#[allow(dead_code)] // Enabled after the cold-seeding qualification.
mod seed_summary;
mod store;
mod version;
mod watchdog;
mod wrapper;

use cli::{Class, Invocation, Profile, LEGACY_PROFILE_ENVIRONMENT};
use freshness::{Gate, Refusal};
use identity::{RunPick, SlotKey};
use oxide_storage::inventory::{inventory_directory, Category, InventoryLimits};
use oxide_storage::lease::{AccountingCoverage, PoolStatus, ReuseMeasurement, SlotOccupancy};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
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
    oxide <check|test|clippy|build|run> [budget options] [--profile quick|debug|release] [cargo args] [-- args]
    oxide <other cargo command> [args]   passed to Cargo; leased when it can build
    oxide exec [budget options] [--manifest-path <Cargo.toml>] [--profile quick|debug|release] -- <command> [args]
                                         run a tool that calls Cargo itself, leased
    oxide clean [--dry-run] [--include-foreign]
                                         remove <workspace>/target and slot affinity; slots stay
    oxide clean <-p|--doc|--profile ...> Cargo's selected clean, same storage rules
    oxide clean --cargo-cache            one pass of Cargo's own home cleanup, no network
    oxide outputs plan|adopt|detach|status
                                         permanent drive: plan, explicitly convert or
                                         detach the owned <checkout>/target link
    oxide status                         recorded pool, slots and waits; no walk or reconcile
    oxide status --measure               bounded payload, slot, checkout and Cargo-home walks
    oxide status --launch-check          macOS: time a new executable's first launch
    oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ...]
                                         macOS: register an APFS volume; mode defaults to ephemeral
    oxide pool set-mode <ephemeral|permanent>
                                         change mode on an existing removable registration
    oxide pool describe --json           the registered drive as read-only JSON
    oxide init [cargo init args] [path]  cargo init if no Cargo.toml, then adopt Oxide
    oxide new <path> [cargo new args]    cargo new, then adopt Oxide

Budget options (opt-in, leased commands only): --admission-budget <dur> bounds
everything before a successful primary child start (preparation, slot waits,
freshness); --execution-budget <dur> bounds compilation, Cargo locks, tests,
drain and closeout from the observed start, and a bounded `run` keeps one
cumulative window through build and program. Each is independently optional;
an omitted axis is unbounded. dur is a positive whole number of
milliseconds or seconds (30s, 1500ms); minutes and hours are refused.
An expired admission window starts no child (exit 124); an expired execution
window stops the owned group with the existing TERM/KILL escalation (exit
125). --budget-events <fd> writes one bounded JSON record per line to an
already-nonblocking writable pipe; a lost stream is reported once and a run
without a complete terminal record is unknown, never a typed success.

Profiles: quick (default, line tables) and debug (full debug info) tune Cargo's
dev profile, so outputs land in target/debug as with plain Cargo; release is the
project's own. --cargo-profile oxide-quick/oxide-debug still build but are
deprecated: external tools get the quick recipe without them. Oxide places build output, so --target-dir and --config
build.build-dir/target-dir are refused. Adoption writes one marked block in the
Git work-tree root's AGENTS.md (the workspace's own AGENTS.md outside Git),
listing the Cargo workspaces it covers, and prints readiness; adoption does not
create an Oxide config file.
A whole-target clean refuses entries Cargo does not create in target/ unless
--include-foreign is given; --dry-run prints every path it would remove.
OXIDE_POOL selects the pool; OXIDE_MAX_ACTIVE sets the host-wide active-build
limit (1–32), ahead of [pool] max_active in the user config. An absent pool
named by OXIDE_POOL refuses instead of being created silently; set
OXIDE_POOL_CREATE=1 (or place the path inside OXIDE_TASK_SCRATCH_ROOT) to
create one there. The config file and the default pool still create on first
use. The automatic
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
    // The installer's effect-free capability probe, before any other work.
    if let Some(code) = handoff::hidden() {
        return ExitCode::from(code);
    }
    // The installer's effect-free build-identity probe (oxide#130).
    if let Some(code) = version::hidden() {
        return ExitCode::from(code);
    }
    // The bounded read-only capacity probe child (oxide#130), before any
    // other work: the parent runs it only for a capacity refusal.
    if let Some(code) = capacity::hidden() {
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
        "-V" | "--version" => {
            // Oxide's own identity first, then Cargo's version and exit status
            // exactly as the caller's Cargo gives them. `--list` stays a
            // direct pass to Cargo with no identity lines.
            version::print_identity();
            return Err(exec_cargo(&args));
        }
        "--list" => return Err(exec_cargo(&args)),
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
    let class = cli::classify(command);
    // `oxide <cmd> --help` / `-h` is Cargo's help, before any budget, hand-off
    // check, pool open, adoption, workspace lookup or lease (oxide#182). It
    // runs exactly `cargo <cmd> --help`. `clean`, `init` and `new` forward
    // too, so neither clean's own planning and workspace discovery nor the
    // hand-off check that reads pool control state can run for a help
    // request. Help after `--` belongs to the program and keeps the owned
    // path.
    if matches!(
        class,
        Class::Modelled | Class::Building | Class::Clean | Class::Init | Class::New
    ) && cli::cargo_help(rest)
    {
        return Err(exec_cargo(&[first.clone(), OsString::from("--help")]));
    }
    // The budget prefix is Oxide's on leased commands only (oxide#140). Any
    // other class refuses a reserved name by name instead of forwarding it.
    let leased = matches!(class, Class::Modelled | Class::Building | Class::Exec);
    if budget::reserved_prefix(rest) && !leased {
        return Err(
            "--admission-budget, --execution-budget and --budget-events are for leased commands (check, build, test, clippy, run, doc, bench, exec and external subcommands)"
                .to_owned(),
        );
    }
    let budgeted = leased && budget::reserved_prefix(rest);
    if budgeted {
        // Syntax and descriptor shape first, then the one monotonic owner:
        // both precede the hand-off check, `run` selection and all other
        // preparation (oxide#140 freeze 1).
        let (options, _rest) = budget::scan(rest)?;
        budget::install(options)?;
    }
    budget::refuse_absent_test_hook()?;
    // One pre-effect hand-off check (oxide#122), after help, version, list and
    // toolchain handling and before any arm opens a store. Plain passthrough,
    // usage-only `exec` and `status --launch-check` do no work of their own.
    let no_effect = class == Class::Plain
        || (class == Class::Exec && !matches!(cli::parse_exec(rest), Ok(Some(_))))
        || (class == Class::Status && matches!(rest, [flag] if flag == "--launch-check"));
    if !no_effect {
        if let Some(code) = handoff::preflight(budgeted) {
            return Ok(code);
        }
    }
    match class {
        Class::Plain => Err(exec_cargo(&args)),
        Class::Status => run_status(rest),
        Class::Pool => removable::run(rest),
        Class::Outputs => outputs::run(rest),
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
    run_invocation(cli::parse_leased(command, args)?)
}

/// `run` without a `--bin`/`--example` builds only what `cargo run` would run.
/// This asks Cargo for metadata, so it happens after the budget clock exists
/// and is bounded by the admission window.
fn pick_run_target(invocation: &mut Invocation) -> Result<Option<u8>, String> {
    if invocation.command != "run" || cli::run_selection(&invocation.cargo_args).is_some() {
        return Ok(None);
    }
    match identity::run_pick(
        CARGO,
        invocation.manifest_path.as_deref(),
        invocation.package.as_deref(),
    )? {
        RunPick::Bin(name) => {
            invocation
                .cargo_args
                .splice(1..1, [OsString::from("--bin"), OsString::from(name)]);
            Ok(None)
        }
        RunPick::Ambiguous(names) => {
            eprintln!(
                "error: `cargo run` could not determine which binary to run. Use the `--bin` option to specify a binary, or the `default-run` manifest key.\navailable binaries: {}",
                names.join(", ")
            );
            Ok(Some(101))
        }
        RunPick::Unresolved => Ok(None),
    }
}

/// Latch and report an admission-budget stop before any primary child starts.
fn stop_for_admission() -> u8 {
    budget::latch(budget::Origin::Admission);
    budget::report_stop(budget::Origin::Admission);
    budget::finished(
        budget::Outcome::AdmissionDeadline,
        None,
        None,
        budget::Drain::NotReached,
        budget::Lease::Released,
        124,
    );
    124
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

/// Debug-only proof pause (oxide#129): with `OXIDE_TEST_PAUSE=<point>:<gate>`
/// the command writes `<gate>.<point>-<pid>` at `point` and waits (at most
/// 120 s) until `<gate>` exists. Absent from release builds.
fn test_pause(point: &str) {
    test_pause_with(point, "");
}

/// [`test_pause`] whose marker file holds `detail` (for `keyed`, the
/// destination facts the slot key captured, including the root nonce).
fn test_pause_with(point: &str, detail: &str) {
    #[cfg(debug_assertions)]
    if let Some(gate) = std::env::var("OXIDE_TEST_PAUSE")
        .ok()
        .and_then(|value| value.strip_prefix(&format!("{point}:")).map(str::to_owned))
    {
        // Published atomically: the content is complete before the marker
        // name (which the proof globs for) exists.
        let marker = format!("{gate}.{point}-{}", std::process::id());
        let staged = format!("{gate}.staging-{point}-{}", std::process::id());
        if std::fs::write(&staged, detail.as_bytes()).is_ok() {
            let _ = std::fs::rename(&staged, &marker);
        }
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        while !std::path::Path::new(&gate).exists() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let _ = (point, detail);
}

/// The pinned Cargo layout is qualified (oxide#169). The one-line flip of this
/// constant is the enablement: seeding is on by default once it is true, and
/// `OXIDE_COLD_SEED=0` is the opt-out.
const COLD_SEED_QUALIFIED: bool = true;

fn cold_seed_active(value: Option<&std::ffi::OsStr>) -> bool {
    cold_seed_active_when(COLD_SEED_QUALIFIED, value)
}

/// Once qualified, seeding is enabled by default and only the exact value `0`
/// opts out. Before qualification it is never active.
fn cold_seed_active_when(qualified: bool, value: Option<&std::ffi::OsStr>) -> bool {
    qualified && value != Some(std::ffi::OsStr::new("0"))
}

fn cold_seed_qualification_notice(value: Option<&std::ffi::OsStr>) -> Option<&'static str> {
    cold_seed_notice_when(COLD_SEED_QUALIFIED, value)
}

/// The skip notice is only for an opt-in that a pre-qualification hold refuses.
/// A qualified build prints no notice for `1` or an unset value; `0` is silent.
fn cold_seed_notice_when(qualified: bool, value: Option<&std::ffi::OsStr>) -> Option<&'static str> {
    (!qualified && value == Some(std::ffi::OsStr::new("1")))
        .then_some("seeding not yet qualified on this Cargo; enabled after the qualification proof")
}

fn run_invocation(mut invocation: Invocation) -> Result<u8, String> {
    if let Some(reason) =
        cold_seed_qualification_notice(std::env::var_os("OXIDE_COLD_SEED").as_deref())
    {
        eprintln!("oxide: seeding skipped: {reason}");
    }
    // The clock was installed by `run` before the hand-off check; nothing here
    // may re-arm it (oxide#140).
    let exec = invocation.command == "exec";
    process::install_signal_handlers()?;
    // A preparation step stopped by the admission window is the admission
    // deadline (124), never an internal error: the window is what refused it.
    macro_rules! prep {
        ($value:expr) => {
            match $value {
                Ok(value) => value,
                Err(error) => {
                    if budget::admission_expired() {
                        return Ok(stop_for_admission());
                    }
                    return Err(error);
                }
            }
        };
    }
    if let Some(code) = prep!(pick_run_target(&mut invocation)) {
        return Ok(code);
    }
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    let internal = prep!(Store::open_or_create());
    let output_control = internal.control().to_path_buf();
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    let checkout = prep!(identity::locate_workspace(
        CARGO,
        invocation.manifest_path.as_deref()
    ));
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    // The final-output destination is part of the slot's compatibility
    // identity: a slot's uplifted artifacts belong to one real root. The
    // destination is the one the binding below will select, so the key and the
    // bound root cannot disagree. An ephemeral or detached checkout adds
    // nothing, so its keys are unchanged.
    // The intermediate pool is selected first: outputs select the external
    // root only when this command's slot comes from the removable pool, whose
    // watchdog supervises the drive's removal.
    let selected = prep!(removable::select(internal));
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    let selection_mode = selected.mode;
    let fallback_reason = selected.fallback_reason.clone();
    let pool_fallback = (!selected.removable && selection_mode.is_some()).then(|| {
        fallback_reason
            .clone()
            .unwrap_or_else(|| "the removable pool is not selected".to_owned())
    });
    // One-use workspace classification (design §3.1): once per command, after
    // the pool selection and before the first step that can write an owned
    // output effect. It reads no receipt, slot, child temp path or payload,
    // and it is computed only while the feature is active, so a dormant build
    // does exactly what it did before.
    let classification = one_use::is_active().then(|| {
        one_use::classify(
            &checkout,
            &one_use::Facts::from_control(Some(&output_control)),
        )
    });
    if let Some(classification) = &classification {
        for note in &classification.notes {
            eprintln!("oxide: {note}");
        }
    }
    let guarded = classification
        .as_ref()
        .is_some_and(one_use::Classification::guarded);
    let local_guard: Option<String> = classification
        .as_ref()
        .filter(|classification| classification.guarded())
        .map(one_use::Classification::reason);
    // The record class of the slot this command leases (design §3.6, §5.1).
    // Classification is only computed while the feature is active, so a
    // dormant build always admits a normal record and writes no class field.
    let class_reason = classification
        .as_ref()
        .filter(|classification| classification.one_use())
        .map(one_use::Classification::reason);
    let admission = match class_reason.as_deref() {
        Some(reason) => store::WorkspaceAdmission::one_use(reason),
        None => store::WorkspaceAdmission::NORMAL,
    };
    // A fresh checkout on the selected permanent drive converts its target
    // before the destination is planned and keyed (oxide#121). Its barrier is
    // released before this command takes a slot. A guarded workspace skips it
    // entirely, silently: no barrier, fence, receipt, root, link or notice
    // (design §4.1).
    if !guarded {
        prep!(outputs::auto_adopt(&selected.store, &checkout, &selected));
    }
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    let store = selected.store;
    // The declaration is pure parsing and is computed BEFORE the reservation:
    // the pre-lease reservation needs the same requested member set binding
    // will use, and a command this checkout refuses on layout or an unmodelled
    // command name now refuses before any authority write (oxide#133).
    //
    // oxide#164: an external subcommand is Cargo running `cargo-<name>`, which
    // is exactly what `oxide exec -- cargo <name>` runs, so it derives its
    // member set from this checkout's committed receipt through the SAME
    // function as a derived `exec`. One path, one key, one set of refusals.
    let derives_from_receipt =
        cli::derives_from_receipt(&invocation.command, &invocation.declared_outputs);
    // A guarded workspace declares the members its own command writes, from
    // its command line alone: it never derives a set from a receipt (design
    // §4.1, §4.3). The set is used only to name the components the guard
    // inspects, so an unmodelled command simply declares none.
    let declaration = if guarded {
        outputs::guarded_declaration(&invocation)?
    } else {
        outputs::lease_declaration(
            &invocation,
            Some(&output_control),
            &checkout,
            pool_fallback.as_deref(),
        )?
    };
    let guarded_members: Vec<String> = if guarded {
        declaration
            .members()
            .iter()
            .map(|member| member.name().to_owned())
            .collect()
    } else {
        Vec::new()
    };
    let local_outputs = local_guard.as_deref().map(|reason| outputs::LocalOutputs {
        members: &guarded_members,
        reason,
    });
    if let Some(local) = &local_outputs {
        // The guard's fast precheck (design §4.1 part 1): a read-only decision
        // that may refuse here, before any reservation, lease or ticket.
        prep!(outputs::guarded_precheck(
            Some(&output_control),
            &checkout,
            local.members,
        ));
    }
    // A future owned root gets its durable random nonce reserved before the
    // destination is planned and keyed (oxide#129); the binding lock is
    // released again before this command waits for a slot. A derived profile
    // plan is a subset of the receipt's own placements, so it reserves
    // nothing: the root it binds already exists and no member is new. A derived
    // set that is EMPTY names no placement, and a whole-target plan there can
    // name a FUTURE root (the first local fallback), so it keeps the shipped
    // reservation path (oxide#164 review finding).
    // A guarded command never reserves a future root, so it never occupies a
    // nonce or a registry row (design §4.1).
    if !guarded && outputs::needs_reservation(derives_from_receipt, &declaration) {
        prep!(outputs::reserve(
            &store,
            Some(&output_control),
            &checkout,
            &declaration,
            pool_fallback.as_deref(),
        ));
    }
    test_pause("reserved");
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    // #133 supplies the profile declaration; #140 keeps the admission window
    // around the same preparation step.
    let planned = prep!(outputs::planned_destination(
        Some(&output_control),
        &checkout,
        &declaration,
        pool_fallback.as_deref(),
        guarded,
    ));
    let destination = outputs::destination_facts(planned.as_ref());
    let key = prep!(SlotKey::resolve_in(
        &checkout,
        invocation.profile.recipe_key(),
        invocation.target.as_deref(),
        &destination,
        &prep!(incremental::key_component(&checkout, &invocation)),
    ));
    test_pause_with("keyed", &destination);
    if budget::admission_expired() {
        return Ok(stop_for_admission());
    }
    if !selected.removable {
        if selection_mode == Some(removable::DriveMode::Permanent) {
            if let Some(reason) = fallback_reason.as_deref() {
                eprintln!(
                    "oxide: WARNING: permanent removable mode fell back to the internal pool ({reason})"
                );
            }
        } else if let Some(note) = &selected.note {
            eprintln!("oxide: {note}");
        }
    }
    let started = Instant::now();
    let mut waits = store::WaitAccounting::default();
    let mut grants = LeaseGrants::new();
    let resolution = cli::resolution(&invocation.cargo_args);
    let label = exec_label(&invocation.tool);
    // The sweep runs before the admission as well as beside the writer
    // (oxide#190): a pool over its budget because of a pending holding row
    // refuses the very admission that could delete it, so this command's pass
    // takes that entry (at most one, under a brief lock, deleted outside it)
    // and its own evicted tree is deleted the same way once the lease is held.
    let mut holding_sweep = HoldingSweep::start(store.pool.clone(), None);
    let lease = lease_with_gate(
        &store,
        &key,
        invocation.profile,
        admission,
        &resolution,
        label.as_deref(),
        &mut waits,
        &mut grants,
    );
    let lease = match lease {
        Ok(inner) => Ok(inner),
        Err(error) => {
            // A capacity refusal can be caused by holding bytes this command is
            // able to free. The eviction decision takes **one** bounded victim
            // per attempt, so this command publishes one pending tree outside
            // the pool lock, retries, and re-evaluates from the resulting
            // figure. The bound is small; a round that removes nothing, or the
            // bound, leaves the last refusal standing.
            let first_removed = holding_sweep.finish().is_some_and(|drain| drain.gone);
            reacquire_after_capacity(
                error,
                first_removed,
                HOLDING_RECLAIM_ROUNDS,
                || {
                    let mut pass = HoldingSweep::start(store.pool.clone(), None);
                    pass.finish().is_some_and(|drain| drain.gone)
                },
                || {
                    lease_with_gate(
                        &store,
                        &key,
                        invocation.profile,
                        admission,
                        &resolution,
                        label.as_deref(),
                        &mut waits,
                        &mut grants,
                    )
                },
            )
        }
    };
    let (mut held, mut gate, mut pending) = match lease? {
        Ok(leased) => leased,
        Err(Stop::Signal(signal)) => return Ok(process::signal_exit_code(signal)),
        Err(Stop::Cargo(code)) => return Ok(code),
        Err(Stop::Admission) => return Ok(stop_for_admission()),
    };
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(stop_for_admission());
    }
    if let Some(signal) = process::received_signal() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(process::signal_exit_code(signal));
    }
    // Bind the checkout's owned output root once, before Cargo starts. The
    // lease just taken is this checkout's exclusive barrier: `oxide clean`
    // and another command wait for it, so the swap cannot redirect a running
    // command.
    // The transition runs under this checkout's output barrier, which waits
    // for every other cooperative command in the checkout to close and blocks
    // new ones until the link is published. It binds exactly the destination
    // the slot was keyed for, or refuses.
    let mut binding = match outputs::bind_for_command(
        &store,
        &key.checkout,
        held.record_id(),
        planned.as_ref(),
        pool_fallback.as_deref(),
        // A derived command — `oxide exec` without `--output`, or any external
        // subcommand — may create the absent member directory of its own
        // verified, committed root (oxide#162, oxide#164); every other command
        // keeps the existing refusal (`derives_from_receipt` is false for
        // them).
        derives_from_receipt,
        local_outputs.as_ref(),
    ) {
        Ok(binding) => binding,
        Err(reason) => {
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            if budget::admission_expired() {
                return Ok(stop_for_admission());
            }
            return Err(reason);
        }
    };
    test_pause("bound");
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(stop_for_admission());
    }
    // The seed point is after the output binding and before any recipient
    // scratch or writer. The gate is the default-on qualification constant
    // with `OXIDE_COLD_SEED=0` as the opt-out.
    let mut production = None;
    if cold_seed_active(std::env::var_os("OXIDE_COLD_SEED").as_deref()) {
        match seed_sources::Production::prepare(&invocation, &key.checkout, &held, &store) {
            Ok(mut prepared) => match prepared.seed(&store, &held) {
                Ok(store::SeedRun::Skipped(reason)) => {
                    let reason = seed_lock::skip_reason(reason, &prepared.lock);
                    eprintln!("oxide: seeding skipped: {reason}");
                    production = Some(prepared);
                }
                Ok(store::SeedRun::Seeded(_, last)) => {
                    eprintln!("{}", seed_summary::summary(&last));
                    production = Some(prepared);
                }
                Ok(store::SeedRun::Abandoned) => {
                    (held, gate, binding, pending) = match abandon_seed_and_reacquire(
                        &store,
                        &key,
                        invocation.profile,
                        admission,
                        &resolution,
                        label.as_deref(),
                        &mut waits,
                        &mut grants,
                        held,
                        binding,
                        planned.as_ref(),
                        pool_fallback.as_deref(),
                        derives_from_receipt,
                        local_outputs.as_ref(),
                    )? {
                        Ok(leased) => leased,
                        Err(Stop::Signal(signal)) => return Ok(process::signal_exit_code(signal)),
                        Err(Stop::Cargo(code)) => return Ok(code),
                        Err(Stop::Admission) => return Ok(stop_for_admission()),
                    };
                    eprintln!("oxide: seeding skipped: recipient walk incomplete");
                }
                Err(reason) => {
                    return refuse_seed(&store, held, binding, reason);
                }
            },
            Err(seed_sources::PreparationError::Skip(reason)) => {
                eprintln!("oxide: seeding skipped: {reason}");
            }
            Err(seed_sources::PreparationError::SkipNamed(reason)) => {
                eprintln!("oxide: seeding skipped: {reason}");
            }
            Err(seed_sources::PreparationError::Refuse(reason)) => {
                drop(binding);
                let origin = held.prior.unchanged();
                store.release(held, origin)?;
                return Err(reason);
            }
        }
    }
    // The lease this command finally holds may itself have evicted idle slots
    // (including through the seeding re-acquire above), whose trees are already
    // renamed into the holding area and recorded. The library reports exactly
    // the entries it created and `discard` names the one it evicted by ID, so
    // this command publishes its own eviction — named, never selected by
    // rotation — beside its own writer (oxide#190). Several entries can be
    // pending from one admission; the pass takes the first and a later command
    // takes the rest.
    pending.extend(held.created_holding.iter().cloned());
    let mut holding_sweep = HoldingSweep::start(store.pool.clone(), pending.first().cloned());
    for warning in &binding.outcome.warnings {
        eprintln!("oxide: WARNING: {warning}");
    }
    let output_note = binding.outcome.note.clone();
    let output_root = binding.outcome.root.clone();
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
    // Where this command's children put temporary files (oxide#136): an
    // inherited `TMPDIR` that is a writable directory on the selected pool's
    // own device is kept for every child; otherwise the command gets one
    // Oxide-owned directory inside the held slot. The choice is made before
    // any writer starts and published through the shared child environment.
    // The slot directory is the build directory's parent; a command whose
    // children can be given neither a qualified inherited directory nor an
    // owned one refuses here, before any writer starts, instead of leaving
    // them on the machine default this decision rejected.
    let chosen_temp = match held.build_dir.parent() {
        Some(slot_dir) => child_temp::choose(
            std::env::var_os("TMPDIR"),
            &store.payload,
            slot_dir,
            child_temp::Identity {
                store_id: held.store_id.clone(),
                slot_id: held.slot_id.clone(),
                incarnation: held.incarnation.clone(),
                lease_id: held.lease_id().to_owned(),
            },
        ),
        None => Err("the held slot path has no directory".to_owned()),
    };
    let chosen_temp = match chosen_temp {
        Ok(chosen) => chosen,
        Err(reason) => {
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            return Err(reason);
        }
    };
    chosen_temp.apply(&mut cargo);
    // The owned directory is a lease property: release removes it after
    // positive writer closure, and a crash keeps it with the held slot.
    held.scratch = chosen_temp.owned();
    if run {
        cargo.stdout(Stdio::piped());
    }
    waits.print_completion("slot ready");
    // The last check before the launch attempt: an already-observed expiry
    // forbids starting a primary child.
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        store.release(held, origin)?;
        return Ok(stop_for_admission());
    }
    if let Some(delay) = budget::spawn_delay() {
        // Debug proof control: hold the last admission check open so the
        // deadline can pass before the launch attempt (the in-flight case).
        std::thread::sleep(delay);
    }
    // oxide#164: a tool's own directory that is not a profile member (the
    // nextest `target/nextest` or llvm-cov analogue) stays a real local
    // directory, and is reported once. This is a bounded observation of ONE
    // directory — the real target's direct entries, before the writer starts
    // and after its group closes — with no recursion, no payload walk, no link
    // followed and no new authority. It belongs to the receipt-derived
    // PROFILE-MEMBER layout only: a Cargo-only command, an unconverted,
    // ephemeral, whole-target or detached checkout, a modelled command and an
    // explicit `--output` declaration all keep their shipped stderr.
    let reports_tool_directories =
        outputs::tool_notice_applies(derives_from_receipt, planned.as_ref());
    let target_entries_before = reports_tool_directories
        .then(|| outputs::target_entries(&key.checkout))
        .flatten();
    let started_group = store
        .mark_spawning(&held)
        .map_err(Unstarted::Record)
        .and_then(|()| process::start_group(&mut cargo).map_err(Unstarted::Spawn));
    let mut group = match started_group {
        Ok(group) => group,
        Err(error) => {
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            budget::finished(
                budget::Outcome::SpawnFailure,
                None,
                None,
                budget::Drain::NotReached,
                budget::Lease::Released,
                1,
            );
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
    // The execution window arms once here, at the observed start. A start
    // after the admission deadline is recorded truthfully and stopped at
    // once: it never receives a fresh usable interval.
    let execution_started = Some(Instant::now());
    budget::arm_execution(
        execution_started.unwrap(),
        if exec {
            budget::Phase::ExecTool
        } else {
            budget::Phase::PrimaryCargo
        },
    );
    let mut budget_stop = process::Stop::new();
    let mut watchdog = store.watchdog();
    // The binding mutex is released only once this command is recorded as
    // running, so no peer can repoint the link between the check and Cargo.
    let marked = store.mark_running(&held, group.id);
    drop(binding);
    let outcome = match marked {
        Ok(()) if run => collect_run(&mut group, &invocation, watchdog.as_mut(), &mut budget_stop),
        Ok(()) => match watchdog.as_mut() {
            Some(watchdog) => process::wait_leader_watched(&mut group, watchdog, &mut budget_stop)
                .map(|status| (status, None, true)),
            None => process::wait_leader_budgeted(&mut group, &mut budget_stop)
                .map(|status| (status, None, true)),
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
    let mut closed = oxide_storage::release_trace::phase("cargo group closure", || {
        match (watchdog.as_mut(), &outcome) {
            (Some(_), Ok((None, _, _))) => None,
            (Some(watchdog), _) => {
                process::wait_group_empty_watched(group.id, watchdog, &mut budget_stop)
            }
            (None, _) => process::wait_group_empty_budgeted(group.id, &mut budget_stop),
        }
    });
    if budget::closure_stuck() {
        // Debug proof control: report the group and drain as unconfirmed even
        // though the kernel says otherwise, so the held-custody path is
        // exercisable without a false custody claim.
        closed = None;
    }
    let stopped = watchdog.as_ref().filter(|watchdog| watchdog.stopped());
    if let Some(watchdog) = stopped {
        report_stop(watchdog, group.id, closed, &store.payload);
    }
    if closed.is_none() {
        if let Some(origin) = budget::origin() {
            budget::report_stop(origin);
        }
        budget::finished(
            budget::Outcome::UnconfirmedClosure,
            outcome.as_ref().ok().and_then(|(status, _, _)| *status),
            execution_started,
            budget::Drain::NotReached,
            budget::Lease::Held,
            1,
        );
        if stopped.is_some_and(Watchdog::removal) {
            store.quarantine_lost(
                &held,
                &format!("Cargo group {} did not close after KILL", group.id),
            )?;
            return Ok(1);
        }
        eprintln!(
            "oxide: could not confirm Cargo group {} closed after the stop; slot {} stays held, and a later oxide command recovers it once the group is empty",
            group.id, held.slot_id
        );
        return Ok(1);
    }
    let slot = held.slot_id.clone();
    let warm = held.reused;
    let measured = held.measured.clone();
    // The one bounded notice for what this tool created in the checkout's real
    // target (oxide#164). Printed after the writer's group closed and before
    // the summary, once, and only on the receipt-derived profile-member layout.
    if let Some(note) = oxide_storage::release_trace::phase("tool directory notice", || {
        outputs::tool_directories_note(&key.checkout, target_entries_before.as_deref())
    }) {
        eprintln!("oxide: {note}");
    }
    // The command's own deletion finishes before the summary, inside its
    // bounded wait, so the pool is not left holding a tree this command could
    // have removed (oxide#190).
    holding_sweep.finish();
    let mut notes = [gate.describe(), wrapper_note, output_note]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    notes.push(chosen_temp.note());
    if selection_mode == Some(removable::DriveMode::Permanent) {
        if let Some(reason) = fallback_reason.as_deref() {
            notes.push(format!(
                "permanent removable fallback to internal pool: {reason}"
            ));
        }
    }
    let note = notes.join(", ");
    let note = (!note.is_empty()).then_some(note);
    let origin =
        oxide_storage::release_trace::phase("freshness finish", || gate.finish(&key.checkout));
    let seed_profile = if invocation.profile == Profile::Release
        || (invocation.command == "bench" && !invocation.explicit_profile)
    {
        "release"
    } else {
        "debug"
    };
    // A converted checkout's `run` keeps its lease through the program: the
    // program runs as this command's own process group from the real root
    // it bound, under the same custody (a relink waits, a clean waits),
    // removal watchdog and positive group closure as Cargo, and the slot is
    // released only once that group is empty.
    // A bounded `run` keeps the lease through the program with ONE cumulative
    // execution deadline: no fresh clock at release or program launch. The
    // unbudgeted path keeps today's release-then-exec hand-off.
    let bounded_run = run && budget::has_execution();
    let owned_program = match (&outcome, &output_root) {
        (Ok((Some(status), Some(Ok((program, manifest_dir))), _drained)), root)
            if run
                && status.success()
                && stopped.is_none()
                && budget::origin().is_none()
                && (root.is_some() || bounded_run) =>
        {
            // #133 maps only a linked member beneath its owned root and
            // requires the mapped file to be a real executable; #140 keeps the
            // Cargo path when no root was bound (a bounded run without a
            // converted checkout).
            let program = match root {
                Some(root) => {
                    oxide_storage::release_trace::phase("program target resolution", || {
                        outputs::program_target(&key.checkout, root, &program)
                    })
                    .unwrap_or_else(|| program.clone())
                }
                None => program.clone(),
            };
            Some((program, manifest_dir.clone()))
        }
        _ => None,
    };
    if let Some((program, manifest_dir)) = owned_program {
        print_summary(
            &store,
            &invocation,
            &slot,
            warm,
            measured.as_ref(),
            started,
            note.as_deref(),
            &waits,
        );
        if let Some(origin) = budget::origin() {
            budget::report_stop(origin);
        }
        return run_owned_program(
            &store,
            held,
            origin,
            &program,
            &manifest_dir,
            &invocation.program_args,
            execution_started,
            &mut budget_stop,
            &chosen_temp,
            production,
            seed_profile,
        );
    }
    // An unconverted `run` program is an Oxide-launched child too, and when
    // this command owns a temporary directory the program is still its
    // writer. The lease is the only fence that keeps a competing lease or
    // whole-slot eviction from reconciling that directory, so such a program
    // runs under the same custody as a converted checkout's program
    // (`run_owned_program`): its own group, the record naming it, forwarded
    // signals, and release — which removes the owned directory — only after
    // the group is observed empty. A program with no owned directory to
    // protect keeps the plain release-then-exec path.
    let fenced_program = match (&outcome, chosen_temp.owned()) {
        (Ok((Some(status), Some(Ok((program, manifest_dir))), _drained)), Some(_))
            if run && status.success() && stopped.is_none() =>
        {
            Some((program.clone(), manifest_dir.clone()))
        }
        _ => None,
    };
    if let Some((program, manifest_dir)) = fenced_program {
        print_summary(
            &store,
            &invocation,
            &slot,
            warm,
            measured.as_ref(),
            started,
            note.as_deref(),
            &waits,
        );
        return run_owned_program(
            &store,
            held,
            origin,
            &program,
            &manifest_dir,
            &invocation.program_args,
            execution_started,
            &mut budget_stop,
            &chosen_temp,
            production,
            seed_profile,
        );
    }
    if let Some(producing) = production {
        let successful = outcome
            .as_ref()
            .is_ok_and(|(status, _, _)| status.is_some_and(|status| status.success()))
            && stopped.is_none()
            && budget::origin().is_none();
        // A catalog failure never changes the command's result or origin.
        oxide_storage::release_trace::phase("seed publication", || {
            producing.publish(&store, &held, successful, origin.as_ref(), seed_profile)
        });
    }
    store.release(held, origin)?;
    let (status, executable, drained) = outcome?;
    let status = status.ok_or("Cargo ended without a status")?;
    print_summary(
        &store,
        &invocation,
        &slot,
        warm,
        measured.as_ref(),
        started,
        note.as_deref(),
        &waits,
    );
    let mut retired = false;
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
                None => {
                    retired = true;
                    eprintln!("oxide: slot {slot} is retired: its flush failed");
                }
            },
            _ => {}
        }
    }
    if let Some(origin) = budget::origin() {
        budget::report_stop(origin);
    }
    budget::finished(
        if budget::origin() == Some(budget::Origin::Admission) {
            budget::Outcome::AdmissionDeadline
        } else if budget::origin() == Some(budget::Origin::Execution) {
            budget::Outcome::ExecutionDeadline
        } else if status.code() == Some(0) {
            budget::Outcome::ChildExit
        } else if std::os::unix::process::ExitStatusExt::signal(&status).is_some() {
            budget::Outcome::ChildSignal
        } else {
            budget::Outcome::ChildExit
        },
        Some(status),
        execution_started,
        if drained {
            budget::Drain::Complete
        } else {
            budget::Drain::Gapped
        },
        if retired {
            budget::Lease::Retired
        } else {
            budget::Lease::Released
        },
        budget::exit_code(process::exit_code(status)),
    );
    if stopped.is_some() {
        // A reserve stop is an ordinary failed command, never a success.
        return Ok(process::exit_code(status).max(1));
    }
    if !run || !status.success() {
        return Ok(budget::exit_code(process::exit_code(status)));
    }
    if budget::has_execution() && budget::origin().is_none() {
        // A bounded `run` never reaches the release-then-exec hand-off.
        eprintln!("oxide: error: no bounded run target was recorded");
        return Ok(101);
    }
    match executable.unwrap_or_else(|| Err("no run target was recorded".to_owned())) {
        Ok((program, manifest_dir)) => {
            // Lease released and this command owns no temporary directory,
            // so the program runs from the private target dir with the kept
            // inherited directory.
            let mut command = Command::new(&program);
            command
                .args(&invocation.program_args)
                .env("CARGO_MANIFEST_DIR", manifest_dir);
            chosen_temp.apply(&mut command);
            let error = command.exec();
            Err(format!("could not execute {}: {error}", program.display()))
        }
        Err(message) => {
            eprintln!("oxide: error: {message}");
            Ok(101)
        }
    }
}

/// Run a converted checkout's program under the held lease, as `oxide exec`
/// runs a tool: its own process group, the record naming that group (so a
/// crash leaves the slot held until the group is empty), signals forwarded,
/// the removable pool's removal watchdog polled, and the slot released only
/// after the group is observed empty. A removal stop whose group cannot be
/// confirmed closed quarantines the slot as unknown custody.
#[allow(clippy::too_many_arguments)]
fn run_owned_program(
    store: &Store,
    held: store::Lease,
    origin: Option<freshness::Built>,
    program: &Path,
    manifest_dir: &Path,
    args: &[OsString],
    execution_started: Option<Instant>,
    budget_stop: &mut process::Stop,
    child_temp: &child_temp::Chosen,
    production: Option<seed_sources::Production>,
    seed_profile: &str,
) -> Result<u8, String> {
    let mut command = Command::new(program);
    command.args(args).env("CARGO_MANIFEST_DIR", manifest_dir);
    child_temp.apply(&mut command);
    if let Err(error) = store.mark_spawning(&held) {
        store.release(held, origin)?;
        return Err(error);
    }
    let (mut group, foreground) = match process::start_foreground_group(&mut command) {
        Ok(started) => started,
        Err(error) => {
            store.release(held, origin)?;
            return Err(format!("could not execute {}: {error}", program.display()));
        }
    };
    budget::program_started();
    // Only a removable slot is watched, and only for removal: a program on
    // the internal disk is never stopped for host free space.
    let mut watchdog = match store.backend {
        Backend::ApfsVolume => store.watchdog(),
        Backend::DirectoryBudget => None,
    };
    if let Err(error) = store.mark_running(&held, group.id) {
        // SAFETY: the negative PID addresses the group this command created.
        unsafe {
            libc::kill(-group.id, libc::SIGTERM);
        }
        let _ = process::wait_leader(&mut group);
        process::wait_group_empty(group.id);
        drop(foreground);
        store.release(held, None)?;
        budget::finished(
            budget::Outcome::InternalError,
            None,
            execution_started,
            budget::Drain::NotReached,
            budget::Lease::Released,
            1,
        );
        return Err(error);
    }
    let status = match watchdog.as_mut() {
        Some(watchdog) => process::wait_leader_watched(&mut group, watchdog, budget_stop)?,
        None => process::wait_leader_budgeted(&mut group, budget_stop)?,
    };
    let mut closed = oxide_storage::release_trace::phase("run group closure", || {
        match (watchdog.as_mut(), status) {
            (Some(_), None) => None,
            (Some(watchdog), Some(_)) => {
                process::wait_group_empty_watched(group.id, watchdog, budget_stop)
            }
            (None, _) => process::wait_group_empty_budgeted(group.id, budget_stop),
        }
    });
    if budget::closure_stuck() {
        closed = None;
    }
    drop(foreground);
    let stopped = watchdog.as_ref().filter(|watchdog| watchdog.stopped());
    if let Some(watchdog) = stopped {
        report_stop(watchdog, group.id, closed, &store.payload);
    }
    if closed.is_none() {
        if let Some(origin) = budget::origin() {
            budget::report_stop(origin);
        }
        budget::finished(
            budget::Outcome::UnconfirmedClosure,
            status,
            execution_started,
            budget::Drain::NotReached,
            budget::Lease::Held,
            1,
        );
        store.quarantine_lost(
            &held,
            &format!("run program group {} did not close after KILL", group.id),
        )?;
        return Ok(1);
    }
    if let Some(producing) = production {
        let successful = status.is_some_and(|status| status.success())
            && stopped.is_none()
            && budget::origin().is_none();
        // A catalog failure never changes the command's result or origin.
        producing.publish(store, &held, successful, origin.as_ref(), seed_profile);
    }
    store.release(held, origin)?;
    if let Some(origin) = budget::origin() {
        budget::report_stop(origin);
    }
    let code = status.map_or(1, process::exit_code);
    let code = if stopped.is_some() { code.max(1) } else { code };
    let code = budget::exit_code(code);
    budget::finished(
        if budget::origin().is_some() {
            budget::Outcome::ExecutionDeadline
        } else if status
            .is_some_and(|status| std::os::unix::process::ExitStatusExt::signal(&status).is_some())
        {
            budget::Outcome::ChildSignal
        } else {
            budget::Outcome::ChildExit
        },
        status,
        execution_started,
        budget::Drain::Complete,
        budget::Lease::Released,
        code,
    );
    Ok(code)
}

/// One reserve-stop line, then a read-only capacity diagnostic for the same
/// filesystem once the stopped group has positively closed (oxide#130). An
/// unconfirmed closure, and a removable volume's own removal stop, probe
/// nothing: the diagnostic runs after writers are closed, never during
/// polling.
fn report_stop(watchdog: &Watchdog, group: i32, closed: Option<Instant>, payload: &Path) {
    eprintln!("{}", watchdog.report(group, closed));
    if closed.is_none() || watchdog.removal() {
        return;
    }
    if let Some(note) = capacity::diagnose(payload) {
        eprintln!("{note}");
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
    /// The admission window closed before a slot was granted (oxide#140).
    Admission,
}

/// Lease a slot and run the freshness gate. Cargo never runs on a slot the
/// gate cannot vouch for: a slot holding another checkout's content goes back
/// to that checkout unchanged; this checkout's own untrusted or unreadable
/// slot is evicted. Either way the command leases again without sharing and
/// builds cold. A `--locked` or `--frozen` lock refusal is the command's own
/// failure: the slot goes back unchanged and the command ends as plain Cargo
/// would.
struct LeaseGrants {
    remaining: u8,
    share: bool,
}

impl LeaseGrants {
    fn new() -> Self {
        Self {
            // The twelve project slots a refused command can try plus one
            // cold fallback (oxide#118, oxide#177).
            remaining: (store::PROJECT_SLOT_CAP + 1) as u8,
            share: true,
        }
    }

    fn taken(&mut self) {
        self.remaining -= 1;
    }
}

fn refuse_seed(
    store: &Store,
    held: store::Lease,
    binding: outputs::OutputBinding,
    reason: String,
) -> Result<u8, String> {
    drop(binding);
    store.release(held, None)?;
    if budget::admission_expired() {
        Ok(stop_for_admission())
    } else {
        Err(reason)
    }
}

/// Release a failed-walk recipient before using the ordinary admission loop.
#[allow(clippy::too_many_arguments)]
fn abandon_seed_and_reacquire(
    store: &Store,
    key: &SlotKey,
    profile: cli::Profile,
    admission: store::WorkspaceAdmission<'_>,
    resolution: &cli::Resolution,
    label: Option<&str>,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
    held: store::Lease,
    binding: outputs::OutputBinding,
    planned: Option<&outputs::Plan>,
    pool_fallback: Option<&str>,
    derives_from_receipt: bool,
    // The guarded local-output decision of this command, exactly as its first
    // binding received it (oxide#170 added this parameter; the cold re-bind
    // must not lose it).
    local_outputs: Option<&outputs::LocalOutputs<'_>>,
) -> Result<Result<(store::Lease, Gate, outputs::OutputBinding, Vec<String>), Stop>, String> {
    drop(binding);
    store.release(held, None)?;
    grants.share = false;
    let (held, gate, pending) = match lease_with_gate(
        store, key, profile, admission, resolution, label, waits, grants,
    )? {
        Ok(leased) => leased,
        Err(stop) => return Ok(Err(stop)),
    };
    match outputs::bind_for_command(
        store,
        &key.checkout,
        held.record_id(),
        planned,
        pool_fallback,
        derives_from_receipt,
        local_outputs,
    ) {
        Ok(binding) => Ok(Ok((held, gate, binding, pending))),
        Err(reason) => {
            let origin = held.prior.unchanged();
            store.release(held, origin)?;
            if budget::admission_expired() {
                Ok(Err(Stop::Admission))
            } else {
                Err(reason)
            }
        }
    }
}

fn lease_with_gate(
    store: &Store,
    key: &SlotKey,
    profile: cli::Profile,
    admission: store::WorkspaceAdmission<'_>,
    resolution: &cli::Resolution,
    label: Option<&str>,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
) -> Result<Result<(store::Lease, Gate, Vec<String>), Stop>, String> {
    // A refusal leaves this checkout without a record, so the next lease is
    // a new or evicted slot. At most one grant per project slot plus one cold
    // fallback (oxide#118, oxide#177). `pending` collects the holding entries
    // this call evicted by ID, so the command publishes exactly those.
    let mut pending: Vec<String> = Vec::new();
    while grants.remaining > 0 {
        let mut held = match store.acquire(key, profile, admission, grants.share, label, waits)? {
            Acquired::Lease(held) => held,
            Acquired::Interrupted(signal) => return Ok(Err(Stop::Signal(signal))),
            Acquired::AdmissionExpired => return Ok(Err(Stop::Admission)),
        };
        grants.taken();
        let refusal = freshness::prepare(&key.checkout, &held.prior, resolution);
        // Taken just now, or left on this checkout's record by a command
        // that died before Cargo.
        let foreign = held.prior.foreign(&key.checkout);
        let reason = match refusal {
            Ok(gate) => return Ok(Ok((held, gate, pending))),
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
        grants.share = false;
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
            let entry = held.holding_entry_name();
            store.discard(held)?;
            pending.extend(entry);
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
    budget_stop: &mut process::Stop,
) -> Result<(Option<ExitStatus>, Option<RunTarget>, bool), String> {
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
        // Freeze A2 item 6 order, with the drain slice first: the line read is
        // the only wait here, and the leader is harvested immediately after it
        // and before any latch decision. A child that exited during the slice
        // is this iteration's natural outcome, never a controlled stop. Once
        // the child is reaped and its stream settled, the loop ends before the
        // deadline is even looked at; a still-running leader or an open drain
        // can still be stopped.
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
        process::forward_pending(group.id);
        if status.is_some() && closed {
            // The child exited and its stream settled in this iteration:
            // nothing is left that a deadline could stop or a signal could end.
            break;
        }
        if budget_stop.poll(group.id) {
            // A stopped drain is reported as such, never as a settled stream.
            return Ok((None, None, false));
        }
        if let Some(watchdog) = watchdog.as_deref_mut() {
            watchdog.poll(group.id);
            if status.is_none() && watchdog.gave_up() {
                return Ok((None, None, false));
            }
        }
    }
    // The reader reported end-of-file before the loop ended, so joining it is
    // bounded; a stopped drain returned above without joining.
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
    Ok((status, Some(target), closed))
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
    waits: &store::WaitAccounting,
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
    let wait = waits.summary_suffix().unwrap_or_default();
    eprintln!(
        "oxide: {} profile, {strategy} strategy, slot {slot} {}, {:.1}s, {pool}{backend}{note}{wait}",
        invocation.profile.label(),
        if warm { "warm" } else { "cold" },
        started.elapsed().as_secs_f64()
    );
}

/// What one `oxide status` run asks for (oxide#128). `Recorded` is the
/// default: recorded facts only, no walk, no lock and no reconciliation.
/// `Measure` runs the existing bounded walks without reconciling or writing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusMode {
    Recorded,
    Measure,
}

impl StatusMode {
    fn accounting(self) -> store::Accounting {
        match self {
            Self::Recorded => store::Accounting::Recorded,
            Self::Measure => store::Accounting::Measured,
        }
    }
}

/// How long a command waits for its own holding deletion before it reports the
/// tree as left for a later command. The deletion itself is unbounded in the
/// product (it runs beside the build and is resumed by the next command); this
/// is only the wait at exit, so one huge tree never holds a command open.
const HOLDING_SWEEP_WAIT: Duration = Duration::from_secs(60);
/// How many publish-and-re-evaluate rounds one command makes after a capacity
/// refusal. Each round deletes exactly one pending tree outside the pool lock
/// and then lets the admission decide again: the deletion is what changes the
/// figure, so nothing is evicted ahead of the decision that needs it.
const HOLDING_RECLAIM_ROUNDS: usize = 4;
/// How often the bounded exit wait reports its progress.
const HOLDING_SWEEP_REPORT: Duration = Duration::from_secs(2);

/// The asynchronous deletion of one held slot tree, owned by the command that
/// evicted it (oxide#190).
///
/// The worker claims at most one recorded holding entry under a brief pool
/// lock, then deletes that tree outside the lock while this command's own
/// writer runs. It never waits for another deleter: the per-entry custody lock
/// is taken without blocking, so a command that loses the race simply leaves
/// the entry to the process that holds it.
///
/// The guard joins the worker before this command exits, with a bounded wait
/// whose progress is printed. A worker that is still running at the bound is
/// detached: the entry stays recorded and charged, and a later command's own
/// pass resumes it from whatever is left.
struct HoldingSweep {
    worker: Option<std::thread::JoinHandle<Option<oxide_storage::lease::HoldingDrain>>>,
}

impl HoldingSweep {
    /// Start the pass for this command's pool.
    ///
    /// `prefer` names the holding entry this command itself just created. When
    /// it is set, the pass claims exactly that entry — not whichever row a
    /// rotation happens to select — so a publishing command always deletes its
    /// own eviction. A pool with nothing of its own pending costs one short
    /// locked open that finds no entry.
    fn start(pool: PathBuf, prefer: Option<String>) -> Self {
        let worker = std::thread::Builder::new()
            .name("oxide-holding".to_owned())
            .spawn(move || report_holding_pass(&pool, prefer.as_deref()))
            .ok();
        Self { worker }
    }

    /// Wait for the pass with a bounded wait, printing its progress. Returns
    /// the pass's own result when it finished inside the bound: `Some` means
    /// this command claimed and worked on one holding entry, which is the
    /// evidence a caller uses to retry an admission the pending charge refused.
    fn finish(&mut self) -> Option<oxide_storage::lease::HoldingDrain> {
        let started = Instant::now();
        let mut next_report = HOLDING_SWEEP_REPORT;
        loop {
            let finished = self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.is_finished());
            if finished {
                return self
                    .worker
                    .take()
                    .and_then(|worker| worker.join().ok().flatten());
            }
            let elapsed = started.elapsed();
            if elapsed >= HOLDING_SWEEP_WAIT {
                // Detach the worker: the process is about to exit, and the
                // entry keeps its record and charge for the next command.
                self.worker = None;
                eprintln!(
                    "oxide: a held slot tree is still being deleted after {}s; it stays recorded in the holding area, and the next oxide command resumes it",
                    elapsed.as_secs()
                );
                return None;
            }
            if elapsed >= next_report {
                eprintln!(
                    "oxide: still deleting a held slot tree outside the pool lock ({}s; the build and every other command are not held)",
                    elapsed.as_secs()
                );
                next_report += HOLDING_SWEEP_REPORT;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for HoldingSweep {
    /// Every return path of a command joins its own deletion, never only the
    /// happy one.
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let _ = self.finish();
    }
}

/// The bounded publish-and-re-evaluate loop after a capacity refusal.
///
/// `first_removed` is the **pre-admission** pass's own outcome. That pass runs
/// beside the refused admission, so it may positively remove a tree *after* the
/// refusal was decided; the charge has then changed even though a later pass
/// would find nothing. The admission is therefore re-evaluated for that removal
/// too — the figure, not a pass, decides. Each later round publishes one more
/// tree and re-evaluates again, so a command never retries against an unchanged
/// figure and never gives up while a removal has already changed it.
fn reacquire_after_capacity<T, E>(
    first_error: E,
    first_removed: bool,
    rounds: usize,
    mut publish: impl FnMut() -> bool,
    mut attempt: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut changed = first_removed;
    let mut last = Err(first_error);
    for _ in 0..rounds {
        if !changed {
            // Nothing has been removed since the refusal: publish one tree.
            changed = publish();
            if !changed {
                break;
            }
            eprintln!(
                "oxide: deleted a held slot tree in the holding area; re-evaluating the admission"
            );
        }
        // A removal has changed the figure, so re-evaluate before demanding
        // another one.
        last = attempt();
        if last.is_ok() {
            break;
        }
        changed = false;
    }
    last
}

/// One pass: claim a pending holding entry and delete it outside the pool
/// lock. Progress goes to stderr, bounded to one line per pass.
fn report_holding_pass(
    pool: &Path,
    prefer: Option<&str>,
) -> Option<oxide_storage::lease::HoldingDrain> {
    let started = Instant::now();
    match oxide_storage::lease::managed::drain_holding(pool, prefer) {
        Ok(pass) => {
            let Some(name) = pass.claimed.clone() else {
                return None;
            };
            if pass.gone {
                eprintln!(
                    "oxide: deleted the held slot tree {name} outside the pool lock ({} entries, {} in {:.2}s)",
                    pass.entries,
                    cargo_home::size(pass.bytes),
                    started.elapsed().as_secs_f64()
                );
            } else if !pass.refused.is_empty() {
                eprintln!(
                    "oxide: kept the held slot tree {name} in the holding area ({}); it stays recorded and charged, and a later command reports it again",
                    pass.refused.join(", ")
                );
            }
            Some(pass)
        }
        // A deletion that cannot continue is reported, never fatal: the tree
        // stays recorded, charged and visible in `oxide status`.
        Err(error) => {
            eprintln!("oxide: the held slot deletion could not continue: {error}");
            None
        }
    }
}

/// One cumulative budget for every walk a measured status makes (payload,
/// slots, checkout, Cargo home), so no combination of walks runs unbounded.
struct MeasureBudget {
    deadline: Instant,
}

/// The existing pool accounting deadline, shared by every measured walk.
const MEASURE_BUDGET: Duration = Duration::from_secs(60);

impl MeasureBudget {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + MEASURE_BUDGET,
        }
    }

    /// Milliseconds left, at least one so every walk stays bounded but runs.
    fn remaining_ms(&self) -> u64 {
        let left = self.deadline.saturating_duration_since(Instant::now());
        u64::try_from(left.as_millis()).unwrap_or(u64::MAX).max(1)
    }
}

fn run_status(args: &[OsString]) -> Result<u8, String> {
    let mode = match args {
        [] => StatusMode::Recorded,
        [flag] if flag == "--measure" => StatusMode::Measure,
        [flag] if flag == "--launch-check" => return launch::run_check(),
        _ => return Err("status takes no arguments except --measure or --launch-check".to_owned()),
    };
    if let Some(reason) =
        cold_seed_qualification_notice(std::env::var_os("OXIDE_COLD_SEED").as_deref())
    {
        println!("seeding skipped: {reason}");
    }
    let settings = store::settings()?;
    print_settings(&settings);
    println!(
        "{}",
        wrapper::status_line(wrapper::Policy::from_env(), &wrapper::inherited(&[]))
    );
    let mut budget = MeasureBudget::new();
    let mut control = None;
    match Store::open_recorded(&settings) {
        Ok(None) => println!(
            "  not created; the first building oxide command creates it ({}: {})",
            Backend::DirectoryBudget.tag(),
            Backend::DirectoryBudget.bound()
        ),
        Ok(Some(store)) => {
            control = Some(store.control().to_path_buf());
            print_pools(&store, mode, &mut budget)?;
        }
        Err(error) => println!("  cannot open: {error}"),
    }
    print_other_pools(&settings);
    print_checkout(control, mode, &mut budget);
    Ok(0)
}

/// The checkout-local lines. Recorded status runs no Cargo, no inventory and
/// no repository discovery: it says so instead of measuring. Measured status
/// locates the workspace once and walks it inside the shared budget.
fn print_checkout(control: Option<PathBuf>, mode: StatusMode, budget: &mut MeasureBudget) {
    if mode == StatusMode::Recorded {
        println!("outputs: not measured by status; use `oxide status --measure`");
        println!("stray Cargo output: not measured by status; use `oxide status --measure`");
        cargo_home::report_status(false, budget.remaining_ms());
        return;
    }
    match identity::locate_workspace(CARGO, None) {
        Ok(workspace) => {
            report_stray(&workspace, budget.remaining_ms());
            match outputs::status_lines_for(control, &workspace) {
                Ok(lines) => {
                    for line in lines {
                        println!("{line}");
                    }
                }
                Err(reason) => println!("outputs: unknown ({reason})"),
            }
        }
        Err(_) => println!("stray Cargo output: no Cargo workspace here"),
    }
    cargo_home::report_status(true, budget.remaining_ms());
}

/// The pool selected for the next build first, then the other; the most
/// recent recorded use across both; each pool's own slots under its header.
/// A recorded status reads the ledger and control state without the pool
/// lock and without reconciling anything; a measured status walks the
/// payload and every slot once, under one cumulative budget.
fn print_pools(
    internal: &Store,
    mode: StatusMode,
    budget: &mut MeasureBudget,
) -> Result<(), String> {
    let measured = mode == StatusMode::Measure;
    let removable = removable::status(internal, measured)?;
    let removable_report = match removable.as_ref().map(|status| &status.recorded) {
        Some(Ok(store)) => Some(store.report(mode.accounting(), budget.remaining_ms())?),
        _ => None,
    };
    let internal_report = internal.report(mode.accounting(), budget.remaining_ms())?;
    let reason = removable
        .as_ref()
        .and_then(|status| status.recorded.as_ref().err())
        .map(String::as_str);
    println!("{}", selection_line(removable.as_ref(), reason, measured));
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
    println!(
        "{}",
        temp_line(internal, removable.as_ref(), reason, measured)
    );

    for section in sections(removable.as_ref(), reason, measured) {
        match section {
            Section::Internal(note) => {
                let header = format!("internal pool: {}{note}", internal.pool.display());
                print_pool(internal, &header, &[], &internal_report, mode)?;
                let seed_enabled = cold_seed_active(std::env::var_os("OXIDE_COLD_SEED").as_deref());
                if let Some(line) = seed_summary::recorded_if_enabled(&internal.pool, seed_enabled)
                {
                    println!("{line}");
                }
                for line in crate::prune::status_lines(internal.control()) {
                    println!("  {line}");
                }
            }
            Section::NoRemovable => println!("removable pool: none registered"),
            Section::Removable => {
                let (Some(status), Some(report)) = (&removable, &removable_report) else {
                    return Err("the removable pool was not opened".to_owned());
                };
                let Ok(store) = &status.recorded else {
                    return Err("the removable pool was not opened".to_owned());
                };
                let header = format!("removable pool: {}", status.lines[0]);
                print_pool(store, &header, &status.lines[1..], report, mode)?;
                let seed_enabled = cold_seed_active(std::env::var_os("OXIDE_COLD_SEED").as_deref());
                if let Some(line) = seed_summary::recorded_if_enabled(&store.pool, seed_enabled) {
                    println!("{line}");
                }
            }
            Section::RemovableUnread(reason) => {
                let Some(status) = &removable else {
                    return Err("the removable pool is not registered".to_owned());
                };
                println!("removable pool: {}", status.lines[0]);
                for line in &status.lines[1..] {
                    println!("{line}");
                }
                println!("  slots: not read ({reason})");
                println!(
                    "  recorded selection: {} at {}; status does not verify the volume",
                    status.mode,
                    status.mount_point.display()
                );
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
    /// Registered but its recorded pool could not be read.
    RemovableUnread(String),
    NoRemovable,
}

/// The pool selected for the next build comes first; each pool is one
/// block holding only its own slots. A recorded status cannot verify the
/// volume, so it only reports which pool the recording selects.
fn sections(
    removable: Option<&removable::RemovableStatus>,
    reason: Option<&str>,
    measured: bool,
) -> Vec<Section> {
    let Some(status) = removable else {
        return vec![
            Section::Internal(" (selected for the next build)"),
            Section::NoRemovable,
        ];
    };
    let selected = if measured {
        reason.is_none()
    } else {
        status.present
    };
    let internal = if selected {
        if status.present {
            " (fallback while the removable volume is not usable)"
        } else {
            " (fallback while the removable volume is absent)"
        }
    } else {
        " (selected for the next build)"
    };
    let removable_section = match &status.recorded {
        Ok(_) => Section::Removable,
        Err(error) => Section::RemovableUnread(error.clone()),
    };
    if selected {
        vec![removable_section, Section::Internal(internal)]
    } else {
        vec![Section::Internal(internal), removable_section]
    }
}

/// Which pool the next build uses. A measured status makes the same check
/// builds make; a recorded status says what the registration records and
/// whether the mount point is present, and never presents that as admission.
fn selection_line(
    removable: Option<&removable::RemovableStatus>,
    reason: Option<&str>,
    measured: bool,
) -> String {
    let Some(status) = removable else {
        return "selected for the next build: internal pool (no removable volume registered)"
            .to_owned();
    };
    let mount = status.mount_point.display();
    if measured {
        return match reason {
            None => "selected for the next build: removable pool".to_owned(),
            Some(reason) => format!(
                "selected for the next build: internal pool (removable volume not used: {reason})"
            ),
        };
    }
    if status.present {
        format!(
            "recorded selection: removable pool (volume present at {mount}; status does not verify it, the next build does)"
        )
    } else {
        format!(
            "recorded selection: removable pool, but no volume is present at {mount}; the next build uses the internal pool until it returns"
        )
    }
}

/// The child temporary-directory fact for `status` (oxide#136). Recorded
/// status never opens the selected volume, so it states the rule without
/// pretending to know the answer; a measured status verifies the selected
/// pool exactly as a build would and names the choice a leased command there
/// would make. Nothing here creates, reads or removes a temporary directory.
fn temp_line(
    internal: &Store,
    removable: Option<&removable::RemovableStatus>,
    reason: Option<&str>,
    measured: bool,
) -> String {
    if !measured {
        return "temp: chosen per leased command (an inherited TMPDIR only on the selected pool's volume, else owned per-command scratch in the slot); --measure names it".to_owned();
    }
    let selected = match removable {
        Some(status) if reason.is_none() => status.recorded.as_ref().ok(),
        _ => None,
    };
    let store = selected.unwrap_or(internal);
    match std::fs::metadata(&store.payload) {
        Ok(meta) => child_temp::describe(std::env::var_os("TMPDIR").as_deref(), meta.dev()),
        Err(error) => format!(
            "temp: unknown (cannot read {}: {error})",
            store.payload.display()
        ),
    }
}

/// The most recently used slot across pools, from each pool's recorded
/// latest use. Pools are listed in display order; equal times name the
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

/// Backend, use, slots and recoveries of one recorded or measured pool,
/// under `header`.
fn print_pool(
    store: &Store,
    header: &str,
    details: &[String],
    report: &store::Report,
    mode: StatusMode,
) -> Result<(), String> {
    let measured = mode == StatusMode::Measure;
    println!("{header}");
    for line in details {
        println!("{line}");
    }
    println!(
        "  backend {}: {}",
        store.backend.tag(),
        store.backend.bound()
    );
    let used = match &report.measured {
        Some(pool) => slot_use(pool),
        None => "slot allocation not measured".to_owned(),
    };
    match store.backend {
        Backend::DirectoryBudget => {
            let available = watchdog::available_bytes(&store.payload).map_or_else(
                || "unknown".to_owned(),
                |bytes| format!("{} MiB", bytes / MIB),
            );
            println!(
                "  soft budget {} MB (checked at admission; a running build may grow past it), {used}",
                store.budget(report.cap_bytes) / 1_000_000
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
            println!(
                "  Time Machine: {}",
                if measured {
                    backup_line(&store.pool)
                } else {
                    "not checked by status; use `oxide status --measure`".to_owned()
                }
            );
            println!("  Spotlight: {}", config::spotlight_advice(&store.pool));
            println!(
                "  reserve watchdog: stops a build below {} GiB available ({} GiB reserve + {} GiB margin), best effort; pool filesystem available {available}",
                (store::HOST_RESERVE_BYTES + watchdog::MARGIN_BYTES) / store::GIB,
                store::HOST_RESERVE_BYTES / store::GIB,
                watchdog::MARGIN_BYTES / store::GIB
            );
        }
        Backend::ApfsVolume => {
            println!(
                "  payload {} (recorded), admission cap {} MiB, {used}",
                store.payload.display(),
                report.cap_bytes / MIB
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
    match &report.measured {
        Some(pool) => {
            println!("  slot accounting walk: {}", pool.payload_accounting);
            if pool.coverage != pool.payload_accounting.coverage {
                println!(
                    "  pool coverage: {:?} (a recorded deletion is pending reconciliation or a slot walk was incomplete)",
                    pool.coverage
                );
            }
        }
        None => {
            println!(
                "  recorded slot sizes (ledger): {}",
                match report.recorded_total {
                    Some(bytes) => format!(
                        "{} across {} of {} idle slots{}; the admission basis, not free space",
                        crate::cargo_home::size(bytes),
                        report.recorded_slots,
                        report.recorded_slots + report.unrecorded_slots,
                        if report.unrecorded_slots > 0 {
                            format!(
                                " ({} unrecorded: a lower bound the next admission walks)",
                                report.unrecorded_slots
                            )
                        } else {
                            String::new()
                        }
                    ),
                    None =>
                        "none recorded yet; the next admission walks the slots it needs".to_owned(),
                }
            );
            println!(
                "  slot sizes: recorded observations only (a separate diagnostic source from the ledger total above); `oxide status --measure` walks the payload and every slot without reconciling"
            );
        }
    }
    if !report.pending_reconciliation.is_empty() {
        let pending: Vec<String> = report
            .pending_reconciliation
            .iter()
            .map(|slot| format!("slot {slot}"))
            .collect();
        println!(
            "  recorded deletions pending reconciliation: {} (left in place; status never reconciles or deletes)",
            pending.join(", ")
        );
    }
    // Held trees are deleted outside the pool lock by the command that evicted
    // them or by a later one (oxide#190). This reports the recorded facts and
    // one presence check each, never a walk; it starts no deletion.
    for held in &report.holding {
        let age = process::now_ms().saturating_sub(held.at_ms) / 1000;
        println!(
            "  slot {} held for deletion: {} recorded, {}s old, {}",
            held.slot_id,
            crate::cargo_home::size(held.recorded_bytes),
            age,
            match held.present {
                Some(true) => "tree still present, deletion pending outside the pool lock",
                Some(false) => "tree gone, record pending its next locked open",
                None => "presence not probed",
            }
        );
    }
    if let Some(error) = &report.state_error {
        println!(
            "  control state: unknown ({error}); holders, waits, cleaning and flushes are not reported"
        );
    }
    println!(
        "  slots {}, waiting commands {}",
        report.slots.len(),
        report.waiting
    );
    // One bounded class line, only for a pool that actually holds a one-use
    // member (design §5.6, §10.5). It never claims a byte figure: this view
    // measures nothing, and an unattributed row makes any figure a lower bound.
    if let Some(line) = one_use_slots_line(&report.class) {
        println!("  {line}");
    }
    for waiter in &report.waiters {
        let age = waiter.age.as_ref().map_or_else(
            || "age unknown".to_owned(),
            |age| {
                format!(
                    "age approximately {}s ({})",
                    age.seconds,
                    age.source.label()
                )
            },
        );
        println!(
            "  recorded lease-ticket waiter PID {}, {age} (may be stale)",
            waiter.pid
        );
    }
    for checkout in &report.cleaning {
        println!("  cleaning {}", checkout.display());
    }
    for slot in &report.slots {
        let state = if report.pending_reconciliation.contains(&slot.slot_id) {
            "recorded deletion pending reconciliation (not idle or held)".to_owned()
        } else {
            match (slot.state, &slot.interrupted) {
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
                        store::Return::GroupEmpty(group) => {
                            format!("Cargo group {group}; recovered once the group is empty")
                        }
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
            }
        };
        let owner = match (&slot.checkout, slot.managed) {
            (Some(checkout), _) => checkout.display().to_string(),
            (None, true) => "no checkout affinity".to_owned(),
            (None, false) => "not managed by oxide".to_owned(),
        };
        println!("{}", slot_line(slot, &state, &owner));
        if let Some(reason) = &slot.class_reason {
            // The stored classification reason, one bounded line (design
            // §5.1, §10.5). It explains why this record is one-use and is
            // never a savings or free-space claim.
            println!("    one-use reason: {reason}");
        }
        let diagnostic_scan = report
            .measured
            .as_ref()
            .and_then(|pool| {
                pool.slots
                    .iter()
                    .find(|pooled| pooled.slot_id == slot.slot_id)
            })
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
/// checkout, size and its recorded place below the pool's `slots`. The
/// place is the ledger's recorded directory, so a nested layout is never
/// confused with a sibling `slots/<id>` directory.
fn slot_line(slot: &store::SlotView, state: &str, owner: &str) -> String {
    let place = slot.dir.as_deref().map_or_else(
        || format!("slots/{} (legacy path)", slot.slot_id),
        |dir| format!("slots/{dir}"),
    );
    // The bounded per-slot class label (design §10.5). A normal record adds
    // nothing, so an ordinary pool's status is unchanged.
    let class = match slot.class {
        Some(store::WorkspaceClass::OneUse) => " (one_use)",
        Some(store::WorkspaceClass::Normal) | None => "",
    };
    format!(
        "  slot {} {state}{class} {} project {} {owner} {}, {place}",
        slot.slot_id,
        slot.profile.as_deref().unwrap_or("-"),
        slot.label.as_deref().unwrap_or("unknown"),
        slot.size.describe()
    )
}

/// The one bounded class line of `status`: the sanctioned `one-use slots:
/// unmeasured (...)` form of design §10.5, with the member count and an
/// explicit lower-bound note when any ledger row has no owning record (§5.6).
/// `None` when the pool holds no one-use member, which is every pool a dormant
/// build can produce.
fn one_use_slots_line(class: &store::ClassMembership) -> Option<String> {
    if class.members.is_empty() {
        return None;
    }
    let mut facts = format!(
        "status does not measure the slice; {} member slot(s)",
        class.members.len()
    );
    if !class.unknown.is_empty() {
        facts.push_str(&format!(
            "; {} class-unknown ledger row(s) make any figure a lower bound",
            class.unknown.len()
        ));
    }
    Some(format!("one-use slots: unmeasured ({facts})"))
}

/// Slot allocation from the payload walk. The pool's own coverage is used, so
/// a pending reconciliation or any incomplete slot walk makes the total a
/// lower bound even when the payload walk itself completed.
fn slot_use(pool: &PoolStatus) -> String {
    match (pool.payload_unique_allocated_bytes, pool.coverage) {
        (Some(used), AccountingCoverage::Complete) => {
            format!("{} MiB allocated by slots", used / MIB)
        }
        (Some(used), _) => format!("at least {} MiB allocated by slots", used / MIB),
        (None, _) => "slot allocation unknown".to_owned(),
    }
}

/// Plain Cargo leaves `deps/` or `incremental/` in the checkout's `target/`;
/// Oxide keeps intermediates in its slot. Finding none means none only for a
/// verified Cargo release, whose layout was checked.
fn report_stray(workspace: &Path, remaining_ms: u64) {
    let target = workspace.join("target");
    if !target.is_dir() {
        println!("stray Cargo output: none ({} absent)", target.display());
        return;
    }
    let limits = InventoryLimits {
        max_entries: 50_000,
        max_depth: 24,
        max_output_bytes: 4096,
        max_elapsed_ms: remaining_ms.min(2_000),
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
    println!("  home: {}", settings.home_line);
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

    /// The pre-admission pass runs beside the refused admission, so it can
    /// remove a tree *after* that refusal was decided. That removal changed the
    /// charge, so the admission must be re-evaluated even if a later pass finds
    /// nothing at all — a pass is not the trigger, a removal is.
    #[test]
    fn a_pre_admission_removal_re_evaluates_the_refused_admission() {
        let mut attempts = 0;
        let mut publishes = 0;
        let outcome: Result<&'static str, &'static str> = super::reacquire_after_capacity(
            "full",
            // The pre-admission pass positively removed a tree.
            true,
            4,
            || {
                publishes += 1;
                // Every later pass finds nothing.
                false
            },
            || {
                attempts += 1;
                Ok("leased")
            },
        );
        assert_eq!(outcome, Ok("leased"));
        assert_eq!(attempts, 1, "the changed charge was not re-evaluated");
        assert_eq!(
            publishes, 0,
            "a pass ran although nothing needed publishing"
        );
    }

    /// A refused attempt whose pass removes nothing keeps its refusal and never
    /// invents progress; a pass that removes a tree is re-evaluated once and then
    /// one more tree is published before the next decision. The bound holds.
    #[test]
    fn the_capacity_loop_publishes_one_tree_per_round_and_is_bounded() {
        // Nothing removed anywhere: no retry at all.
        let mut attempts = 0;
        let outcome: Result<(), &'static str> = super::reacquire_after_capacity(
            "full",
            false,
            4,
            || false,
            || {
                attempts += 1;
                Ok(())
            },
        );
        assert_eq!(outcome, Err("full"));
        assert_eq!(attempts, 0, "an unchanged figure was retried");

        // One removal, still refused, no further tree: exactly one retry.
        let mut attempts = 0;
        let mut publishes = 0;
        let outcome: Result<(), &'static str> = super::reacquire_after_capacity(
            "full",
            false,
            4,
            || {
                publishes += 1;
                publishes == 1
            },
            || {
                attempts += 1;
                Err("full")
            },
        );
        assert_eq!(outcome, Err("full"));
        assert_eq!(attempts, 1);
        assert_eq!(
            publishes, 2,
            "the second round published instead of retrying"
        );

        // Always over budget, always another tree: the bound stops it.
        let mut attempts = 0;
        let mut publishes = 0;
        let outcome: Result<(), &'static str> = super::reacquire_after_capacity(
            "full",
            false,
            3,
            || {
                publishes += 1;
                true
            },
            || {
                attempts += 1;
                Err("full")
            },
        );
        assert_eq!(outcome, Err("full"));
        assert!(
            attempts <= 3 && publishes <= 3,
            "the loop exceeded its bound"
        );
    }

    #[test]
    fn cold_seed_gate_is_off_before_qualification_and_default_on_after() {
        // Before the one-line qualification flip: never active, and an
        // explicit opt-in gets the hold's notice instead.
        assert!(!cold_seed_active_when(false, None));
        assert!(!cold_seed_active_when(false, Some(OsStr::new("1"))));
        assert!(!cold_seed_active_when(false, Some(OsStr::new("0"))));
        assert_eq!(
            cold_seed_notice_when(false, Some(OsStr::new("1"))),
            Some("seeding not yet qualified on this Cargo; enabled after the qualification proof")
        );
        assert_eq!(cold_seed_notice_when(false, None), None);
        assert_eq!(cold_seed_notice_when(false, Some(OsStr::new("0"))), None);
        // After the flip: enabled by default (and for `1`), and only the exact
        // `0` opts out. No notice is printed in any qualified case.
        assert!(cold_seed_active_when(true, None));
        assert!(cold_seed_active_when(true, Some(OsStr::new("1"))));
        assert!(!cold_seed_active_when(true, Some(OsStr::new("0"))));
        assert_eq!(cold_seed_notice_when(true, None), None);
        assert_eq!(cold_seed_notice_when(true, Some(OsStr::new("1"))), None);
        assert_eq!(cold_seed_notice_when(true, Some(OsStr::new("0"))), None);
        // The outer gate is exactly the constant this slice flips last.
        assert_eq!(cold_seed_active(None), COLD_SEED_QUALIFIED);
    }

    #[test]
    fn lease_grant_limit_is_retained_across_admission_passes() {
        let mut grants = LeaseGrants::new();
        assert!(grants.share);
        for _ in 0..store::PROJECT_SLOT_CAP {
            grants.taken();
        }
        grants.share = false;
        assert_eq!(grants.remaining, 1);
        grants.taken();
        assert_eq!(grants.remaining, 0);
        assert!(!grants.share);
    }

    #[test]
    fn failed_walk_releases_then_reacquires_under_the_original_grant_limit() {
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        std::fs::write(held.build_dir.join("partial"), b"untrusted").unwrap();
        let binding = outputs::bind_for_command(
            &store,
            &key.checkout,
            held.record_id(),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let mut grants = LeaseGrants::new();
        grants.taken();
        let mut waits = store::WaitAccounting::default();
        let result = abandon_seed_and_reacquire(
            &store,
            &key,
            cli::Profile::Quick,
            store::WorkspaceAdmission::NORMAL,
            &cli::Resolution::default(),
            None,
            &mut waits,
            &mut grants,
            held,
            binding,
            None,
            None,
            false,
            None,
        )
        .unwrap();
        let Ok((cold, _, binding, _pending)) = result else {
            panic!("cold admission stopped")
        };
        assert!(matches!(cold.prior, freshness::Prior::Fresh));
        assert!(!cold.build_dir.join("partial").exists());
        assert!(grants.remaining < (store::PROJECT_SLOT_CAP + 1) as u8);
        assert!(!grants.share);
        drop(binding);
        store.release(cold, None).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_seed_refuses_and_releases_an_untrusted_generation() {
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        std::fs::write(held.build_dir.join("partial"), b"untrusted").unwrap();
        let binding = outputs::bind_for_command(
            &store,
            &key.checkout,
            held.record_id(),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(
            matches!(refuse_seed(&store, held, binding, "seed fault".into()), Err(ref reason) if reason == "seed fault")
        );
        let held = store::tests::seed_fixture_lease(&store, &key);
        assert!(matches!(held.prior, freshness::Prior::Unknown));
        assert!(held.build_dir.join("partial").exists());
        store.discard(held).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    fn const_scan() -> oxide_storage::lease::AccountingScan {
        oxide_storage::lease::AccountingScan {
            coverage: AccountingCoverage::Complete,
            gaps: Vec::new(),
            vanished: 0,
            entries_visited: 1,
            elapsed_ms: 1,
            max_entries: 2_000_000,
            max_depth: 64,
            max_elapsed_ms: 60_000,
        }
    }

    fn removable_status(
        recorded: Result<Store, String>,
        present: bool,
    ) -> removable::RemovableStatus {
        removable::RemovableStatus {
            lines: vec!["/Volumes/Drive volume U container C (mode: permanent)".to_owned()],
            recorded,
            present,
            mode: removable::DriveMode::Permanent,
            mount_point: PathBuf::from("/Volumes/Drive"),
            control: PathBuf::from("/pool/oxide-managed/pool/oxide-managed"),
        }
    }

    /// A recorded status says what the registration records; a measured one
    /// makes the same selection check builds make.
    #[test]
    fn selected_pool_comes_first() {
        let verified = removable_status(Err("not used".to_owned()), true);
        assert_eq!(
            sections(Some(&verified), None, true),
            vec![
                Section::RemovableUnread("not used".to_owned()),
                Section::Internal(" (fallback while the removable volume is not usable)")
            ]
        );
        let absent = removable_status(Err("recorded pool unreadable".to_owned()), false);
        assert_eq!(
            sections(Some(&absent), Some("recorded pool unreadable"), false),
            vec![
                Section::Internal(" (selected for the next build)"),
                Section::RemovableUnread("recorded pool unreadable".to_owned())
            ]
        );
        assert_eq!(
            sections(None, None, false),
            vec![
                Section::Internal(" (selected for the next build)"),
                Section::NoRemovable
            ]
        );
        assert_eq!(
            selection_line(None, None, false),
            "selected for the next build: internal pool (no removable volume registered)"
        );
        let present = removable_status(Err("not used".to_owned()), true);
        assert_eq!(
            selection_line(Some(&present), None, true),
            "selected for the next build: removable pool"
        );
        assert!(selection_line(Some(&present), Some("below reserve"), true)
            .ends_with("(removable volume not used: below reserve)"));
        // A recorded status never claims admission: it reports the recording
        // and the mount point's presence.
        assert!(selection_line(Some(&present), None, false)
            .starts_with("recorded selection: removable pool (volume present at /Volumes/Drive"));
        assert!(selection_line(Some(&absent), Some("not used"), false)
            .contains("no volume is present at /Volumes/Drive"));
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
            size: store::SlotSize::NotMeasured("not measured".to_owned()),
            class: Some(store::WorkspaceClass::Normal),
            class_reason: None,
        }
    }

    /// The one-use labels of `status` (design §5.6, §10.5): a normal slot adds
    /// nothing, a one-use slot carries the bounded `one_use` label beside the
    /// state, and the pool line is the sanctioned `unmeasured` form — never a
    /// byte figure and never free quota. A pool with no one-use member prints
    /// no line, which is every pool a dormant build can produce.
    #[test]
    fn one_use_status_labels_are_bounded_and_normal_pools_add_nothing() {
        let normal = view("1", None, "/work/tmp/repo");
        let normal_line = slot_line(&normal, "idle", "/work/tmp/repo");
        assert!(!normal_line.contains("one_use"), "{normal_line}");
        assert_eq!(one_use_slots_line(&store::ClassMembership::default()), None);

        let mut one_use = view("2", Some("tmp-0a1b2c3d/quick-2"), "/work/tmp/repo");
        one_use.class = Some(store::WorkspaceClass::OneUse);
        one_use.class_reason = Some("this workspace is under the temporary root '/tmp'".to_owned());
        let one_use_line = slot_line(&one_use, "idle", "/work/tmp/repo");
        assert!(
            one_use_line.contains("idle (one_use) quick project tmp-0a1b2c3d "),
            "{one_use_line}"
        );
        assert!(
            one_use_line.ends_with("slots/tmp-0a1b2c3d/quick-2"),
            "{one_use_line}"
        );

        let member = || store::ClassMember {
            slot_id: "2".to_owned(),
            incarnation: "incarnation".to_owned(),
        };
        let members_only = store::ClassMembership {
            members: vec![member()],
            unknown: Vec::new(),
        };
        let line = one_use_slots_line(&members_only).expect("a member pool has a line");
        assert_eq!(
            line,
            "one-use slots: unmeasured (status does not measure the slice; 1 member slot(s))"
        );
        let with_unknown = store::ClassMembership {
            members: vec![member()],
            unknown: vec![store::ClassMember {
                slot_id: "9".to_owned(),
                incarnation: "other".to_owned(),
            }],
        };
        let line = one_use_slots_line(&with_unknown).expect("a member pool has a line");
        assert!(
            line.contains("1 class-unknown ledger row(s) make any figure a lower bound"),
            "{line}"
        );
        assert!(!line.contains("MiB") && !line.contains("bytes"), "{line}");
    }

    /// A slot whose last walk stopped partial is rendered as
    /// `not measured (walk incomplete)` and never with a figure (oxide#186).
    #[test]
    fn an_incomplete_walk_slot_line_says_not_measured() {
        let mut slot = view("7", None, "/work/tmp/repo");
        slot.size = store::SlotSize::NotMeasured(store::INCOMPLETE_WALK.to_owned());
        let line = slot_line(&slot, "idle", "/work/tmp/repo");
        assert!(
            line.contains("not measured (walk incomplete)"),
            "the status line does not report the incomplete walk: {line}"
        );
        assert!(
            !line.contains("MiB"),
            "the status line claimed a size: {line}"
        );
    }

    /// Equal slot numbers in two pools stay distinct by label and place;
    /// a legacy slot shows its checkout's name and legacy path. A nested
    /// recorded directory is always shown as recorded, never as a sibling
    /// `slots/<id>`.
    #[test]
    fn slot_lines_name_project_and_place() {
        let internal = view("1", None, "/work/My App");
        let removable = view("1", Some("underlay-0a1b2c3d/quick-1"), "/work/underlay");
        let internal_line = slot_line(&internal, "idle", "/work/My App");
        let removable_line = slot_line(&removable, "idle", "/work/underlay");
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
        assert!(slot_line(&unknown, "idle", "no checkout affinity").contains("project unknown"));
        // A nested directory whose leaf carries the ID is never rendered as
        // `slots/<id>`: the recorded directory is the path.
        let nested = view("7", Some("oxide-aaaa/quick-7"), "/work/oxide");
        let nested_line = slot_line(&nested, "idle", "/work/oxide");
        assert!(
            nested_line.ends_with("slots/oxide-aaaa/quick-7"),
            "{nested_line}"
        );
        assert!(!nested_line.contains("slots/7 "), "{nested_line}");
    }

    /// A recorded size says it is recorded, with its age and source, and a
    /// missing, unreadable or differently generated fact never claims a size.
    #[test]
    fn recorded_sizes_are_honest() {
        use store::{ObservationSource, RecordedAge, SlotSize};
        let recorded = SlotSize::Recorded {
            allocated_bytes: 12 * 1024 * 1024,
            age: RecordedAge::Ago(42),
            source: ObservationSource::LeaseReuse,
            lease_id: Some("v1.a.1.1.1".to_owned()),
        };
        let text = recorded.describe();
        assert!(
            text.contains("12 MiB") && text.contains("recorded 42s ago"),
            "{text}"
        );
        assert!(
            text.contains("measured by the reuse walk when leased")
                && text.contains("may have changed since measurement"),
            "{text}"
        );
        let skew = SlotSize::Recorded {
            allocated_bytes: 0,
            age: RecordedAge::ClockSkew,
            source: ObservationSource::LeaseReuse,
            lease_id: None,
        };
        assert!(
            skew.describe().contains("age is unknown"),
            "{}",
            skew.describe()
        );
        let missing = SlotSize::NotMeasured(
            "not measured (no recorded observation); use `oxide status --measure`".to_owned(),
        );
        assert!(missing.describe().contains("not measured"));
        let measured = SlotSize::Measured {
            logical: Some(2 * 1024 * 1024),
            unique: Some(1024 * 1024),
            scan: const_scan(),
        };
        assert!(measured.describe().contains("measured now"));
        assert!(store::age_text(0) == "0s" && store::age_text(600) == "10m");
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

    fn budget_invocation() -> Invocation {
        Invocation {
            command: "run".to_owned(),
            profile: Profile::Quick,
            cargo_args: Vec::new(),
            program_args: Vec::new(),
            json_messages: false,
            manifest_path: None,
            target: None,
            targets: Vec::new(),
            explicit_profile: false,
            declared_outputs: Vec::new(),
            tool: Vec::new(),
            budget: budget::Options::default(),
            package: None,
        }
    }

    #[test]
    fn collect_run_reports_a_natural_exit_that_lands_inside_the_drain_slice() {
        budget::clear();
        budget::install(budget::Options {
            admission: None,
            execution: Some(Duration::from_millis(1)),
            events: None,
        })
        .expect("a budget clock");
        budget::arm_execution(Instant::now(), budget::Phase::PrimaryCargo);
        std::thread::sleep(Duration::from_millis(10));
        // The deadline is already due when the drain starts. A child still
        // running when the first line-read slice ends is entitled to a stop, so
        // the natural exit must be observed before the drain begins. Waiting
        // here keeps the test deterministic under load (it was a 20 ms race).
        // The invariant checked below is the one this ordering guarantees: a
        // natural exit observed with a settled stream carries no latched
        // deadline.
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]).stdout(Stdio::piped());
        let mut group = process::start_group(&mut command).expect("a child");
        let exited = (0..500).any(|_| {
            let done = group.child.try_wait().ok().flatten().is_some();
            if !done {
                std::thread::sleep(Duration::from_millis(10));
            }
            done
        });
        assert!(exited, "the natural exit was not observed within 5 s");
        let mut stop = process::Stop::new();
        let (status, _target, drained) =
            collect_run(&mut group, &budget_invocation(), None, &mut stop).expect("a result");
        if status.and_then(|status| status.code()) == Some(0) && drained {
            assert_eq!(
                budget::origin(),
                None,
                "a natural exit observed after the slice must not become a 125 stop"
            );
        }
        budget::clear();
    }
}
