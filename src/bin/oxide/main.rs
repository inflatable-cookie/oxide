//! Unpublished `oxide` front door: Cargo with managed storage and profiles.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md`. Storage is the
//! existing lease pool; Oxide adds checkout affinity, FIFO waiting, a
//! host-wide limit and own-command crash recovery in a private control
//! directory. A checkout may take another checkout's idle compatible slot
//! only through the freshness gate. Plain `cargo` behavior is untouched.

mod adopt;
mod budget;
mod capacity;
mod cargo_home;
mod cargo_layout;
mod cargo_prune;
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
mod outputs;
mod process;
mod prune;
mod removable;
mod runner;
mod scratch_admission;
mod scratch_io;
mod seed_catalog;
mod seed_context;
mod seed_lock;
mod seed_plan;
mod seed_sources;
mod seed_summary;
mod status;
mod store;
mod version;
mod watchdog;
mod wrapper;

use cli::Class;
#[cfg(test)]
pub(crate) use runner::leased_size;
pub(crate) use runner::{apply_storage, report_stop};
use std::ffi::{OsStr, OsString};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

/// The `cargo` rustup resolves for the current directory. Oxide never selects
/// a toolchain.
pub(crate) const CARGO: &str = "cargo";
pub(crate) const MIB: u64 = 1024 * 1024;
const USAGE: &str = "\
oxide: Cargo with managed build storage and profiles

USAGE:
    oxide <check|test|clippy|build|run> [budget options] [--profile quick|debug|release] [cargo args] [-- args]
    oxide <other cargo command> [args]   passed to Cargo; leased when it can build
    oxide exec [budget options] [--manifest-path <Cargo.toml>] [--profile quick|debug|release] [--output <member>]... -- <command> [args]
                                         run a tool that calls Cargo itself, leased
    oxide clean [--dry-run] [--include-foreign]
                                         remove <workspace>/target and slot affinity; slots stay
    oxide clean <-p|--doc|--profile ...> Cargo's selected clean, same storage rules
    oxide clean --cargo-cache            one pass of Cargo's own home cleanup, no network
    oxide outputs plan|adopt|detach|cancel-reservation|status|prune
                                         permanent drive: plan, explicitly convert,
                                         detach, cancel an unused reservation,
                                         inspect or prune owned outputs
    oxide status                         recorded pool, slots and waits; no walk or reconcile
    oxide status --measure               bounded payload, slot, checkout and Cargo-home walks
    oxide status --launch-check          macOS: time a new executable's first launch
    oxide pool register-removable <mount-point> --reserve <size> --cap <size> [--mode ...]
                                         macOS: register an APFS volume; mode defaults to ephemeral
    oxide pool set-mode <ephemeral|permanent>
                                         change mode on an existing removable registration
    oxide pool recover-interrupted <slot>
                                         release an interrupted slot once writers stopped
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
OXIDE_POOL_CAP sets a new pool's soft budget (bytes, KB, MB, GB, TB, KiB, MiB,
GiB or TiB; default 100 GB); an existing pool keeps its recorded budget.
OXIDE_POOL_BACKEND=directory-budget is the only internal backend: a budget-only
directory pool on every platform. The APFS image backend is retired.
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
    let result = run(std::env::args_os().skip(1).collect());
    if store::writer_updated() {
        let refusal = oxide_storage::lease::LeaseError::WriterUpdated;
        eprintln!("oxide: {refusal}");
        return ExitCode::from(refusal.exit_code());
    }
    match result {
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
    // request. The whole request is forwarded unchanged: an external
    // subcommand such as `oxide nextest run --help` takes its own arguments
    // before `--help`, and dropping them asks Cargo for a different help page
    // (or none). Help after `--` belongs to the program and keeps the owned
    // path.
    if matches!(
        class,
        Class::Modelled | Class::Building | Class::Clean | Class::Init | Class::New
    ) && cli::cargo_help(rest)
    {
        // Oxide's leading budget options are Oxide's, not Cargo's, so a help
        // request drops them. Malformed ones keep the old answer: Cargo's help.
        let kept = match budget::scan(rest) {
            Ok((_, kept)) => kept,
            Err(_) => rest.to_vec(),
        };
        let mut forwarded = Vec::with_capacity(kept.len() + 1);
        forwarded.push(first.clone());
        forwarded.extend(kept);
        return Err(exec_cargo(&forwarded));
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
    // `exec` is parsed once, before any budget is installed: a usage request
    // (`oxide exec --help`, or `oxide exec` with no `--`) prints Oxide's usage
    // and ends before an event descriptor, clock or hand-off check exists, and
    // nothing below parses the arguments again.
    let exec = if class == Class::Exec {
        match cli::parse_exec(rest)? {
            Some(invocation) => Some(invocation),
            None => {
                print!("{}", cli::EXEC_USAGE);
                return Ok(0);
            }
        }
    } else {
        None
    };
    let budgeted = leased && budget::reserved_prefix(rest);
    if budgeted {
        // Syntax and descriptor shape first, then the one monotonic owner:
        // both precede the hand-off check, `run` selection and all other
        // preparation (oxide#140 freeze 1).
        let (options, _rest) = budget::scan(rest)?;
        budget::install(options)?;
    }
    // One pre-effect hand-off check (oxide#122), after help, version, list and
    // toolchain handling and before any arm opens a store. Plain passthrough
    // and `status --launch-check` do no work of their own.
    let no_effect = class == Class::Plain
        || (class == Class::Status && matches!(rest, [flag] if flag == "--launch-check"));
    if !no_effect {
        if let Some(code) = handoff::preflight(budgeted) {
            return Ok(code);
        }
    }
    match class {
        Class::Plain => Err(exec_cargo(&args)),
        Class::Status => status::run(rest),
        Class::Pool => removable::run(rest),
        Class::Outputs => outputs::run(rest),
        Class::Clean => clean::run(rest),
        Class::Init => adopt::run_init(rest),
        Class::New => adopt::run_new(rest),
        Class::Exec => match exec {
            Some(invocation) => runner::run_invocation(invocation),
            None => unreachable!("a usage-only exec returned above"),
        },
        Class::Modelled | Class::Building => runner::run_leased(command, rest),
    }
}

/// Replace Oxide with plain Cargo. Returns only on failure.
fn exec_cargo(args: &[OsString]) -> String {
    let error = Command::new(CARGO).args(args).exec();
    format!("could not start Cargo: {error}")
}

/// The pinned Cargo layout is qualified (oxide#169). The one-line flip of this
/// constant is the enablement: seeding is on by default once it is true, and
/// `OXIDE_COLD_SEED=0` is the opt-out.
const COLD_SEED_QUALIFIED: bool = true;

pub(crate) fn cold_seed_active(value: Option<&OsStr>) -> bool {
    cold_seed_active_when(COLD_SEED_QUALIFIED, value)
}

/// Once qualified, seeding is enabled by default and only the exact value `0`
/// opts out. Before qualification it is never active.
fn cold_seed_active_when(qualified: bool, value: Option<&OsStr>) -> bool {
    qualified && value != Some(OsStr::new("0"))
}

pub(crate) fn cold_seed_qualification_notice(value: Option<&OsStr>) -> Option<&'static str> {
    cold_seed_notice_when(COLD_SEED_QUALIFIED, value)
}

/// The skip notice is only for an opt-in that a pre-qualification hold refuses.
/// A qualified build prints no notice for `1` or an unset value; `0` is silent.
fn cold_seed_notice_when(qualified: bool, value: Option<&OsStr>) -> Option<&'static str> {
    (!qualified && value == Some(OsStr::new("1")))
        .then_some("seeding not yet qualified on this Cargo; enabled after the qualification proof")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn usage_lists_current_outputs_pool_and_exec_options() {
        assert!(USAGE.contains("outputs plan|adopt|detach|cancel-reservation|status|prune"));
        assert!(USAGE.contains("pool recover-interrupted <slot>"));
        assert!(USAGE.contains("--output <member>"));
        assert!(!USAGE.contains("owned <checkout>/target link"));
    }
}
