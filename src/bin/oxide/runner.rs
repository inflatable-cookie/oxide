//! Leased Cargo/`exec` invocation lifecycle, slot custody and summary reporting.

use crate::cli::{Invocation, Profile, LEGACY_PROFILE_ENVIRONMENT};
use crate::freshness::{Gate, Refusal};
use crate::identity::{RunPick, SlotKey};
use crate::session_broker::{BorrowStop, JoinedSession, PermitMode, SessionBroker};
use crate::store::{Acquired, Backend, Store};
use crate::watchdog::Watchdog;
use crate::{
    budget, capacity, cargo_prune, child_temp, cli, cold_seed_active,
    cold_seed_qualification_notice, freshness, identity, incremental, outputs, process, removable,
    seed_lock, seed_sources, seed_summary, store, wrapper, CARGO, MIB,
};
use oxide_storage::lease::ReuseMeasurement;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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

pub(crate) fn run_invocation(invocation: Invocation) -> Result<u8, String> {
    let budgeted = invocation.budget.any();
    if budgeted {
        budget::install(invocation.budget)?;
    }
    process::install_signal_handlers()?;
    let joined = JoinedSession::from_env()?;
    if let Some(deadline) = joined.as_ref().and_then(|session| session.deadline) {
        budget::cap_to_outer(deadline)?;
    }
    let outcome = run_session(invocation, joined.as_ref(), budgeted);
    if let Some(joined) = joined {
        // The slot is already released or retained according to writer
        // closure. EOF also clears membership if the acknowledgement is lost.
        let _ = joined.release();
    }
    outcome
}

fn run_session(
    invocation: Invocation,
    joined: Option<&JoinedSession>,
    budgeted: bool,
) -> Result<u8, String> {
    if budget::admission_expired() {
        return Ok(stop_for_admission(budget::Lease::Released));
    }
    if let Some(signal) = process::received_signal() {
        return Ok(process::signal_exit_code(signal));
    }
    if let Some(joined) = joined {
        joined.check_live()?;
    }
    // An exec hand-off would discard the membership and restart its clock.
    if let Some(code) = crate::handoff::preflight(budgeted || joined.is_some()) {
        return Ok(code);
    }
    oxide_storage::lease::admission_cost::start_summary();
    let outcome = run_invocation_inner(invocation, joined);
    if let Some(summary) = oxide_storage::lease::admission_cost::take_summary() {
        eprintln!("oxide: {summary}");
    }
    outcome
}

/// Result of a single runner stage: either continue with state or exit early.
enum Step<T> {
    Continue(T),
    Exit(u8),
}

struct PreparedInvocation {
    store: Store,
    key: SlotKey,
    planned: Option<outputs::Plan>,
    pool_fallback: Option<String>,
    derives_from_receipt: bool,
    selection_mode: Option<removable::DriveMode>,
    fallback_reason: Option<String>,
    exec: bool,
    run: bool,
}

struct LeaseRequest<'a> {
    store: &'a Store,
    key: &'a SlotKey,
    profile: Profile,
    resolution: &'a cli::Resolution,
    label: Option<&'a str>,
    joined: Option<&'a JoinedSession>,
}

struct OutputBindPlan<'a> {
    planned: Option<&'a outputs::Plan>,
    pool_fallback: Option<&'a str>,
    derives_from_receipt: bool,
}

struct BoundSession {
    held: store::Lease,
    gate: Gate,
    binding: outputs::OutputBinding,
    production: Option<seed_sources::Production>,
}

struct PreparedPrimary {
    cargo: Command,
    chosen_temp: child_temp::Chosen,
    wrapper_note: Option<String>,
    prune_session: Option<cargo_prune::Session>,
}

struct PrimaryExecution<'session> {
    outcome: Result<(Option<ExitStatus>, Option<RunTarget>, bool), String>,
    execution_started: Option<Instant>,
    budget_stop: process::Stop<'session>,
    watchdog: Option<Watchdog>,
    target_entries_before: Option<Vec<String>>,
}

struct OwnedProgram<'a> {
    program: &'a Path,
    manifest: &'a Path,
    args: &'a [OsString],
}

struct ProgramCustody<'a> {
    store: &'a Store,
    held: store::Lease,
    origin: Option<freshness::Built>,
    execution_started: Option<Instant>,
    child_temp: &'a child_temp::Chosen,
    production: Option<seed_sources::Production>,
    seed_profile: &'a str,
    prune_session: Option<cargo_prune::Session>,
}

impl ProgramCustody<'_> {
    /// Publish seed catalog (for fresh producing builds), prune stale layout
    /// v2 units while still holding the slot's exclusive Barge hold, and
    /// release the lease.
    fn finish_and_release(
        self,
        seed_successful: bool,
        prune_successful: bool,
    ) -> Result<store::ReleaseDisposition, String> {
        if let Some(producing) = self.production {
            // A catalog failure never changes the command's result or origin.
            oxide_storage::release_trace::phase("seed publication", || {
                producing.publish(
                    self.store,
                    &self.held,
                    seed_successful,
                    self.origin.as_ref(),
                    self.seed_profile,
                )
            });
        }
        oxide_storage::release_trace::phase("stale unit pruning", || {
            let _ = cargo_prune::finish(
                &self.held,
                self.store.control(),
                self.prune_session,
                prune_successful,
            );
        });
        self.store.release(self.held, self.origin)
    }
}

struct SummaryContext<'a> {
    store: &'a Store,
    invocation: &'a Invocation,
    slot: &'a str,
    warm: bool,
    measured: Option<&'a ReuseMeasurement>,
    started: Instant,
    note: Option<&'a str>,
    waits: &'a store::WaitAccounting,
}

fn run_invocation_inner(
    mut invocation: Invocation,
    joined: Option<&JoinedSession>,
) -> Result<u8, String> {
    let prepared = match prepare_invocation(&mut invocation)? {
        Step::Continue(prepared) => prepared,
        Step::Exit(code) => return Ok(code),
    };
    let started = Instant::now();
    let mut waits = store::WaitAccounting::default();
    let mut grants = LeaseGrants::new();
    let resolution = cli::resolution(&invocation.cargo_args);
    let label = exec_label(&invocation.tool);
    // The settle worker starts before the admission so trash an earlier
    // command could not finish is deleted outside the locks this command is
    // about to take, and a later kick picks up this admission's own evictions
    // (oxide#324). It never deletes under the pool lock.
    let mut trash = TrashSettler::start(prepared.store.pool.clone());
    let request = LeaseRequest {
        store: &prepared.store,
        key: &prepared.key,
        profile: invocation.profile,
        resolution: &resolution,
        label: label.as_deref(),
        joined,
    };
    let bind_plan = OutputBindPlan {
        planned: prepared.planned.as_ref(),
        pool_fallback: prepared.pool_fallback.as_deref(),
        derives_from_receipt: prepared.derives_from_receipt,
    };
    let session =
        match acquire_and_bind(&request, &bind_plan, &invocation, &mut waits, &mut grants)? {
            Step::Continue(session) => session,
            Step::Exit(code) => return Ok(code),
        };
    // The lease this command finally holds may itself have evicted idle slots
    // (including through the seeding re-acquire above). The admission recorded
    // each decision under its pool lock; this kicks the worker so Barge's
    // eviction and trash deletion run beside this command's own writer, with
    // the pool and admission locks released (oxide#324).
    trash.kick();
    for warning in &session.binding.outcome.warnings {
        eprintln!("oxide: WARNING: {warning}");
    }
    let output_note = session.binding.outcome.note.clone();
    let output_root = session.binding.outcome.root.clone();
    let (held, mut primary) =
        match prepare_primary_command(&prepared, &invocation, session.held, &waits)? {
            Step::Continue(ready) => ready,
            Step::Exit(code) => return Ok(code),
        };
    let (held, mut execution) = match execute_primary_group(
        &prepared,
        &invocation,
        held,
        session.binding,
        &mut primary,
        joined,
    )? {
        Step::Continue(executed) => executed,
        Step::Exit(code) => return Ok(code),
    };
    let slot = held.slot_id.clone();
    let warm = held.reused;
    let measured = held.measured.clone();
    // The one bounded notice for what this tool created in the checkout's real
    // target (oxide#164). Printed after the writer's group closed and before
    // the summary, once, and only on the receipt-derived profile-member layout.
    if let Some(note) = oxide_storage::release_trace::phase("tool directory notice", || {
        outputs::tool_directories_note(
            &prepared.key.checkout,
            execution.target_entries_before.as_deref(),
        )
    }) {
        eprintln!("oxide: {note}");
    }
    // The command's own deletion finishes before the summary, inside its
    // bounded wait, so the pool is not left holding a tree this command could
    // have removed (oxide#324).
    trash.finish();
    let note = compose_summary_note(
        &prepared,
        &session.gate,
        primary.wrapper_note,
        output_note,
        &primary.chosen_temp,
    );
    let origin = oxide_storage::release_trace::phase("freshness finish", || {
        session.gate.finish(&prepared.key.checkout)
    });
    let seed_profile = if invocation.profile == Profile::Release
        || (invocation.command == "bench" && !invocation.explicit_profile)
    {
        "release"
    } else {
        "debug"
    };
    let summary = SummaryContext {
        store: &prepared.store,
        invocation: &invocation,
        slot: &slot,
        warm,
        measured: measured.as_ref(),
        started,
        note: note.as_deref(),
        waits: &waits,
    };
    let custody = ProgramCustody {
        store: &prepared.store,
        held,
        origin,
        execution_started: execution.execution_started,
        child_temp: &primary.chosen_temp,
        production: session.production,
        seed_profile,
        prune_session: primary.prune_session,
    };
    if let Some((program, report_pre_launch_stop)) =
        resolve_run_program(&prepared, &execution, &output_root, &primary.chosen_temp)
    {
        print_summary(&summary);
        if report_pre_launch_stop {
            if let Some(origin) = budget::origin() {
                budget::report_stop(origin);
            }
        }
        return run_owned_program(
            custody,
            OwnedProgram {
                program: &program.program,
                manifest: &program.manifest,
                args: &invocation.program_args,
            },
            &mut execution.budget_stop,
        );
    }
    finish_released_invocation(custody, &summary, execution)
}

fn prepare_invocation(invocation: &mut Invocation) -> Result<Step<PreparedInvocation>, String> {
    if let Some(reason) =
        cold_seed_qualification_notice(std::env::var_os("OXIDE_COLD_SEED").as_deref())
    {
        eprintln!("oxide: seeding skipped: {reason}");
    }
    let exec = invocation.command == "exec";
    let run = invocation.command == "run";
    // A preparation step stopped by the admission window is the admission
    // deadline (124), never an internal error: the window is what refused it.
    macro_rules! prep {
        ($value:expr) => {
            match $value {
                Ok(value) => value,
                Err(error) => {
                    if budget::admission_expired() {
                        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
                    }
                    return Err(error);
                }
            }
        };
    }
    if let Some(code) = prep!(pick_run_target(invocation)) {
        return Ok(Step::Exit(code));
    }
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
    }
    let internal = prep!(Store::open_or_create());
    let output_control = internal.control().to_path_buf();
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
    }
    let checkout =
        prep!({ identity::locate_workspace(CARGO, invocation.manifest_path.as_deref()) });
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
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
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
    }
    let selection_mode = selected.mode;
    let fallback_reason = selected.fallback_reason.clone();
    let pool_fallback = (!selected.removable && selection_mode.is_some()).then(|| {
        fallback_reason
            .clone()
            .unwrap_or_else(|| "the removable pool is not selected".to_owned())
    });
    // A fresh checkout on the selected permanent drive converts its target
    // before the destination is planned and keyed (oxide#121). Its barrier is
    // released before this command takes a slot.
    prep!(outputs::auto_adopt(&selected.store, &checkout, &selected));
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
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
    let declaration = outputs::lease_declaration(
        invocation,
        Some(&output_control),
        &checkout,
        pool_fallback.as_deref(),
    )?;
    // A future owned root gets its durable random nonce reserved before the
    // destination is planned and keyed (oxide#129); the binding lock is
    // released again before this command waits for a slot. A derived profile
    // plan is a subset of the receipt's own placements, so it reserves
    // nothing: the root it binds already exists and no member is new. A derived
    // set that is EMPTY names no placement, and a whole-target plan there can
    // name a FUTURE root (the first local fallback), so it keeps the shipped
    // reservation path (oxide#164 review finding).
    if outputs::needs_reservation(derives_from_receipt, &declaration) {
        prep!(outputs::reserve(
            &store,
            Some(&output_control),
            &checkout,
            &declaration,
            pool_fallback.as_deref(),
        ));
    }
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
    }
    // #133 supplies the profile declaration; #140 keeps the admission window
    // around the same preparation step.
    let planned = prep!(outputs::planned_destination(
        Some(&output_control),
        &checkout,
        &declaration,
        pool_fallback.as_deref(),
    ));
    let destination = outputs::destination_facts(planned.as_ref());
    let incremental_key = prep!({ incremental::key_component(&checkout, invocation) });
    let key = prep!(SlotKey::resolve_in(
        &checkout,
        invocation.profile.recipe_key(),
        invocation.target.as_deref(),
        &destination,
        &incremental_key,
    ));
    if budget::admission_expired() {
        return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)));
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
    Ok(Step::Continue(PreparedInvocation {
        store,
        key,
        planned,
        pool_fallback,
        derives_from_receipt,
        selection_mode,
        fallback_reason,
        exec,
        run,
    }))
}

fn acquire_and_bind(
    request: &LeaseRequest<'_>,
    bind_plan: &OutputBindPlan<'_>,
    invocation: &Invocation,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
) -> Result<Step<BoundSession>, String> {
    let (held, gate) = match lease_with_gate(request, waits, grants)? {
        Ok(leased) => leased,
        Err(Stop::Signal(signal)) => return Ok(Step::Exit(process::signal_exit_code(signal))),
        Err(Stop::Cargo(code)) => return Ok(Step::Exit(code)),
        Err(Stop::Admission) => return Ok(Step::Exit(stop_for_admission(budget::Lease::Released))),
    };
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        return match stop_after_release(request.store.release(held, origin)) {
            Ok(code) => Ok(Step::Exit(code)),
            Err(reason) => Err(reason),
        };
    }
    if let Some(signal) = process::received_signal() {
        let origin = held.prior.unchanged();
        request.store.release(held, origin)?;
        return Ok(Step::Exit(process::signal_exit_code(signal)));
    }
    // Bind the checkout's owned output root once, before Cargo starts. The
    // lease just taken is this checkout's exclusive barrier: `oxide clean`
    // and another command wait for it, so the swap cannot redirect a running
    // command.
    // The transition runs under this checkout's output barrier, which waits
    // for every other cooperative command in the checkout to close and blocks
    // new ones until the link is published. It binds exactly the destination
    // the slot was keyed for, or refuses.
    let binding = match outputs::bind_for_command(
        request.store,
        &request.key.checkout,
        held.record_id(),
        bind_plan.planned,
        bind_plan.pool_fallback,
        // A derived command — `oxide exec` without `--output`, or any external
        // subcommand — may create the absent member directory of its own
        // verified, committed root (oxide#162, oxide#164); every other command
        // keeps the existing refusal (`derives_from_receipt` is false for
        // them).
        bind_plan.derives_from_receipt,
    ) {
        Ok(binding) => binding,
        Err(reason) => {
            let origin = held.prior.unchanged();
            let release = request.store.release(held, origin);
            if budget::admission_expired() {
                return match stop_after_release(release) {
                    Ok(code) => Ok(Step::Exit(code)),
                    Err(released) => Err(released),
                };
            }
            match release {
                Ok(_) => return Err(reason),
                Err(released) => return Err(release_failed(released)),
            }
        }
    };
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        return match stop_after_release(request.store.release(held, origin)) {
            Ok(code) => Ok(Step::Exit(code)),
            Err(reason) => Err(reason),
        };
    }
    seed_recipient(
        request,
        bind_plan,
        invocation,
        waits,
        grants,
        (held, gate, binding),
    )
}

fn seed_recipient(
    request: &LeaseRequest<'_>,
    bind_plan: &OutputBindPlan<'_>,
    invocation: &Invocation,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
    leased: (store::Lease, Gate, outputs::OutputBinding),
) -> Result<Step<BoundSession>, String> {
    // The seed point is after the output binding and before any recipient
    // scratch or writer. The gate is the default-on qualification constant
    // with `OXIDE_COLD_SEED=0` as the opt-out.
    let (mut held, mut gate, mut binding) = leased;
    let mut production = None;
    if cold_seed_active(std::env::var_os("OXIDE_COLD_SEED").as_deref()) {
        match seed_sources::Production::prepare(
            invocation,
            &request.key.checkout,
            &held,
            request.store,
        ) {
            Ok(mut prepared) => match prepared.seed(request.store, &held) {
                Ok(store::SeedRun::Skipped(reason)) => {
                    let reason = seed_lock::skip_reason(reason, &prepared.lock);
                    eprintln!("oxide: seeding skipped: {reason}");
                    production = Some(prepared);
                }
                Ok(store::SeedRun::Seeded(_, last)) => {
                    eprintln!("{}", seed_summary::summary(&last));
                    // The recipient was populated by a seed, so it is not a
                    // producing generation: publishing its catalog would spend
                    // the post-build verification pass only to discard the
                    // result.
                }
                Ok(store::SeedRun::Abandoned) => {
                    (held, gate, binding) = match abandon_seed_and_reacquire(
                        request, bind_plan, waits, grants, held, binding,
                    )? {
                        Ok(leased) => leased,
                        Err(Stop::Signal(signal)) => {
                            return Ok(Step::Exit(process::signal_exit_code(signal)))
                        }
                        Err(Stop::Cargo(code)) => return Ok(Step::Exit(code)),
                        Err(Stop::Admission) => {
                            return Ok(Step::Exit(stop_for_admission(budget::Lease::Released)))
                        }
                    };
                    eprintln!("oxide: seeding skipped: recipient walk incomplete");
                }
                Err(reason) => {
                    return refuse_seed(request.store, held, binding, reason).map(Step::Exit);
                }
            },
            Err(seed_sources::PreparationError::Skip(reason)) => {
                eprintln!("oxide: seeding skipped: {reason}");
            }
            Err(seed_sources::PreparationError::SkipNamed(reason)) => {
                eprintln!("oxide: seeding skipped: {reason}");
            }
        }
    }
    Ok(Step::Continue(BoundSession {
        held,
        gate,
        binding,
        production,
    }))
}

fn prepare_primary_command(
    prepared: &PreparedInvocation,
    invocation: &Invocation,
    mut held: store::Lease,
    waits: &store::WaitAccounting,
) -> Result<Step<(store::Lease, PreparedPrimary)>, String> {
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
        &prepared.key.checkout,
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
            &prepared.store.payload,
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
            return match prepared.store.release(held, origin) {
                Ok(_) => Err(reason),
                Err(released) => Err(release_failed(released)),
            };
        }
    };
    chosen_temp.apply(&mut cargo);
    // The owned directory is a lease property: release removes it after
    // positive writer closure, and a crash keeps it with the held slot.
    held.scratch = chosen_temp.owned();
    if prepared.run {
        cargo.stdout(Stdio::piped());
    }
    waits.print_completion("slot ready");
    if let (true, Some(queued)) = (budget::has_events(), waits.admission_queue_wait()) {
        eprintln!("oxide: admission queue wait {} ns", queued.as_nanos());
    }
    // The last check before the launch attempt: an already-observed expiry
    // forbids starting a primary child.
    if budget::admission_expired() {
        let origin = held.prior.unchanged();
        return match stop_after_release(prepared.store.release(held, origin)) {
            Ok(code) => Ok(Step::Exit(code)),
            Err(reason) => Err(reason),
        };
    }
    let prune_session = cargo_prune::prepare(&held);
    Ok(Step::Continue((
        held,
        PreparedPrimary {
            cargo,
            chosen_temp,
            wrapper_note,
            prune_session,
        },
    )))
}

fn execute_primary_group<'session>(
    prepared: &PreparedInvocation,
    invocation: &Invocation,
    mut held: store::Lease,
    binding: outputs::OutputBinding,
    primary: &mut PreparedPrimary,
    joined: Option<&'session JoinedSession>,
) -> Result<Step<(store::Lease, PrimaryExecution<'session>)>, String> {
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
        outputs::tool_notice_applies(prepared.derives_from_receipt, prepared.planned.as_ref());
    let target_entries_before = reports_tool_directories
        .then(|| outputs::target_entries(&prepared.key.checkout))
        .flatten();
    let mut budget_stop = process::Stop::with_session(joined);
    let relay = joined.and_then(JoinedSession::relay);
    let yields_permit =
        cfg!(target_os = "macos") && prepared.exec && (joined.is_none() || relay.is_some());
    let broker = budget_stop.check_session().and_then(|()| {
        let root = primary.chosen_temp.path();
        let command = &mut primary.cargo;
        if !yields_permit {
            return SessionBroker::for_child(root, command);
        }
        let mode = PermitMode::BorrowRequired;
        match relay {
            Some(relay) => SessionBroker::for_permits(root, command, mode, relay),
            None => {
                SessionBroker::for_permits(root, command, mode, prepared.store.exec_permits(&held))
            }
        }
        .map(Some)
    });
    if budget::admission_expired() {
        drop(broker);
        drop(binding);
        let origin = held.prior.unchanged();
        return stop_after_release(prepared.store.release(held, origin)).map(Step::Exit);
    }
    let mut broker = match broker {
        Ok(broker) => broker,
        Err(error) => {
            drop(binding);
            let origin = held.prior.unchanged();
            return Err(release_error_after(
                prepared.store.release(held, origin),
                error,
            ));
        }
    };
    let started_group = prepared
        .store
        .mark_spawning(&held)
        .map_err(Unstarted::Record)
        .and_then(|()| process::start_group(&mut primary.cargo).map_err(Unstarted::Spawn));
    let mut group = match started_group {
        Ok(group) => group,
        Err(error) => {
            let slot = held.slot_id.clone();
            // A shell's convention for a tool Oxide could not start: 127 not
            // found, 126 not runnable. Anything else is an internal failure.
            let code = match &error {
                Unstarted::Spawn(spawn) if prepared.exec => {
                    if spawn.kind() == std::io::ErrorKind::NotFound {
                        127
                    } else {
                        126
                    }
                }
                _ => 1,
            };
            let origin = held.prior.unchanged();
            // The terminal event is emitted whichever way the release ends,
            // and the code it records is the one this invocation returns.
            let released = release_and_emit(
                prepared.store.release(held, origin),
                Terminal {
                    outcome: budget::Outcome::SpawnFailure,
                    child: None,
                    execution_started: None,
                    drain: budget::Drain::NotReached,
                    exit_code: code,
                    lease: None,
                },
            );
            let also_failed = released
                .map(|released| format!("; releasing slot {slot} also failed: {released}"))
                .unwrap_or_default();
            return match error {
                Unstarted::Record(error) => Err(format!("{error}{also_failed}")),
                Unstarted::Spawn(error) if prepared.exec => {
                    eprintln!(
                        "oxide: could not start {}: {error}{also_failed}",
                        wrapper::shown(&invocation.tool[0].to_string_lossy())
                    );
                    Ok(Step::Exit(code))
                }
                Unstarted::Spawn(error) => {
                    Err(format!("could not start Cargo: {error}{also_failed}"))
                }
            };
        }
    };
    // The execution window arms once here, at the observed start. A start
    // after the admission deadline is recorded truthfully and stopped at
    // once: it never receives a fresh usable interval.
    let execution_started = Some(Instant::now());
    budget::arm_execution(
        execution_started.unwrap(),
        if prepared.exec {
            budget::Phase::ExecTool
        } else {
            budget::Phase::PrimaryCargo
        },
    );
    let armed = if yields_permit {
        Ok(())
    } else {
        arm_broker(broker.as_mut())
    };
    let mut watchdog = prepared.store.watchdog();
    // The binding mutex is released only once this command is recorded as
    // running, so no peer can repoint the link between the check and Cargo.
    let marked = prepared.store.mark_running(&held, group.id).and(armed);
    drop(binding);
    let marked = marked.and_then(|()| {
        if yields_permit {
            prepared.store.yield_exec_permit(&mut held)?;
            arm_broker(broker.as_mut())?;
        }
        Ok(())
    });
    // Set only when the group could not be recorded and was stopped here: the
    // bounded stop's own verdict on closure replaces the waits below.
    let mut unrecorded_closed = None;
    let outcome = match marked {
        Ok(()) if prepared.run => {
            collect_run(&mut group, invocation, watchdog.as_mut(), &mut budget_stop)
        }
        Ok(()) => match watchdog.as_mut() {
            Some(watchdog) => process::wait_leader_watched(&mut group, watchdog, &mut budget_stop)
                .map(|status| (status, None, true)),
            None => process::wait_leader_budgeted(&mut group, &mut budget_stop)
                .map(|status| (status, None, true)),
        },
        Err(error) => {
            // Startup did not settle; stop the group with bounded escalation.
            unrecorded_closed = Some(process::stop_unrecorded(&mut group));
            Err(error)
        }
    };
    // Release only after positive closure. A watchdog that cannot confirm
    // the stopped group closed leaves the lease held and the record naming
    // the group, so a later command recovers it once the group is empty.
    let closed = match unrecorded_closed {
        Some(true) => Some(Instant::now()),
        Some(false) => None,
        None => oxide_storage::release_trace::phase("cargo group closure", || {
            match (watchdog.as_mut(), &outcome) {
                (Some(_), Ok((None, _, _))) => None,
                (Some(watchdog), _) => {
                    process::wait_group_empty_watched(group.id, watchdog, &mut budget_stop)
                }
                (None, _) => process::wait_group_empty_budgeted(group.id, &mut budget_stop),
            }
        }),
    };
    let stopped = watchdog.as_ref().filter(|watchdog| watchdog.stopped());
    if let Some(watchdog) = stopped {
        report_stop(watchdog, group.id, closed, &prepared.store.payload);
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
            prepared.store.quarantine_lost(
                &held,
                &format!("Cargo group {} did not close after KILL", group.id),
            )?;
            return Ok(Step::Exit(1));
        }
        eprintln!(
            "oxide: could not confirm Cargo group {} closed after the stop; slot {} stays held, and a later oxide command recovers it once the group is empty",
            group.id, held.slot_id
        );
        return Ok(Step::Exit(1));
    }
    let outcome = outcome.and_then(|result| {
        if budget::origin().is_none() {
            budget_stop.check_session()?;
        }
        Ok(result)
    });
    Ok(Step::Continue((
        held,
        PrimaryExecution {
            outcome,
            execution_started,
            budget_stop,
            watchdog,
            target_entries_before,
        },
    )))
}

fn arm_broker(broker: Option<&mut SessionBroker>) -> Result<(), String> {
    if let Some(broker) = broker {
        broker
            .arm(budget::execution_deadline())
            .map_err(|error| format!("cannot publish the Oxide session deadline: {error}"))?;
    }
    Ok(())
}

fn compose_summary_note(
    prepared: &PreparedInvocation,
    gate: &Gate,
    wrapper_note: Option<String>,
    output_note: Option<String>,
    chosen_temp: &child_temp::Chosen,
) -> Option<String> {
    let mut notes = [gate.describe(), wrapper_note, output_note]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    notes.push(chosen_temp.note());
    if prepared.selection_mode == Some(removable::DriveMode::Permanent) {
        if let Some(reason) = prepared.fallback_reason.as_deref() {
            notes.push(format!(
                "permanent removable fallback to internal pool: {reason}"
            ));
        }
    }
    let note = notes.join(", ");
    (!note.is_empty()).then_some(note)
}

fn resolve_run_program(
    prepared: &PreparedInvocation,
    execution: &PrimaryExecution<'_>,
    output_root: &Option<PathBuf>,
    chosen_temp: &child_temp::Chosen,
) -> Option<(RunProgram, bool)> {
    let stopped = execution
        .watchdog
        .as_ref()
        .filter(|watchdog| watchdog.stopped());
    // A converted checkout's `run` keeps its lease through the program: the
    // program runs as this command's own process group from the real root
    // it bound, under the same custody (a relink waits, a clean waits),
    // removal watchdog and positive group closure as Cargo, and the slot is
    // released only once that group is empty.
    // A bounded `run` keeps the lease through the program with ONE cumulative
    // execution deadline: no fresh clock at release or program launch. The
    // unbudgeted path keeps today's release-then-exec hand-off.
    let bounded_run = prepared.run && budget::has_execution();
    if let (Ok((Some(status), Some(Ok(target)), _drained)), root) =
        (&execution.outcome, output_root)
    {
        if prepared.run
            && status.success()
            && stopped.is_none()
            && budget::origin().is_none()
            && process::received_signal().is_none()
            && (root.is_some() || bounded_run || execution.budget_stop.is_joined())
        {
            // #133 maps only a linked member beneath its owned root and
            // requires the mapped file to be a real executable; #140 keeps the
            // Cargo path when no root was bound (a bounded run without a
            // converted checkout).
            let program = match root {
                Some(root) => {
                    oxide_storage::release_trace::phase("program target resolution", || {
                        outputs::program_target(&prepared.key.checkout, root, &target.program)
                    })
                    .unwrap_or_else(|| target.program.clone())
                }
                None => target.program.clone(),
            };
            return Some((
                RunProgram {
                    program,
                    manifest: target.manifest.clone(),
                },
                true,
            ));
        }
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
    if let (Ok((Some(status), Some(Ok(target)), _drained)), Some(_)) =
        (&execution.outcome, chosen_temp.owned())
    {
        if prepared.run
            && status.success()
            && stopped.is_none()
            && budget::origin().is_none()
            && process::received_signal().is_none()
        {
            return Some((target.clone(), false));
        }
    }
    None
}

fn finish_released_invocation(
    custody: ProgramCustody<'_>,
    summary: &SummaryContext<'_>,
    execution: PrimaryExecution<'_>,
) -> Result<u8, String> {
    let stopped = execution
        .watchdog
        .as_ref()
        .filter(|watchdog| watchdog.stopped());
    let successful = execution
        .outcome
        .as_ref()
        .is_ok_and(|(status, _, _)| status.is_some_and(|status| status.success()))
        && stopped.is_none()
        && budget::origin().is_none()
        && process::received_signal().is_none();
    let store = custody.store;
    let slot = custody.held.slot_id.clone();
    let execution_started = custody.execution_started;
    let child_temp = custody.child_temp;
    // The release happens here, but its failure never ends the invocation
    // before the terminal event: the event is emitted below with the code this
    // command returns, and the release error travels after it.
    let release = custody.finish_and_release(successful, successful);
    // A failed wait or a missing status is this command's own error, reported
    // after its terminal record rather than past it.
    let outcome = execution.outcome.and_then(|(status, executable, drained)| {
        let status = status.ok_or_else(|| "Cargo ended without a status".to_owned())?;
        Ok((status, executable, drained))
    });
    let (status, executable, drained) = match outcome {
        Ok(parts) => parts,
        Err(reason) => return Err(release_error_after(release, reason)),
    };
    print_summary(summary);
    let mut retired = false;
    if store.backend == Backend::ApfsVolume {
        match store.last_flush() {
            Some(flush) if flush.slot_id == summary.slot => match flush.micros {
                Some(micros) => eprintln!(
                    "oxide: flushed the removable volume before releasing slot {} in {:.1} ms",
                    summary.slot,
                    micros as f64 / 1000.0
                ),
                None if flush.lost => eprintln!(
                    "oxide: slot {} was lost with the removable volume; the next command uses the internal pool until it returns",
                    summary.slot
                ),
                None => {
                    retired = true;
                    eprintln!("oxide: slot {} is retired: its flush failed", summary.slot);
                }
            },
            _ => {}
        }
    }
    if let Some(origin) = budget::origin() {
        budget::report_stop(origin);
    }
    // The code this command returns is settled before the budget event is
    // reported, so the event and the exit status always agree. `stopped` is a
    // reserve stop: an ordinary failed command, never a success.
    let mut code = process::exit_code(status);
    if stopped.is_some() {
        code = code.max(1);
    }
    let run = summary.invocation.command == "run";
    // A signal that arrived while Cargo ran ends this command, and a `run`
    // never launches its program afterwards.
    let interrupted = (run && status.success())
        .then(process::received_signal)
        .flatten();
    if let Some(signal) = interrupted {
        code = process::signal_exit_code(signal);
    }
    let code = budget::exit_code(code);
    let released = release_and_emit(
        release,
        Terminal {
            outcome: if budget::origin() == Some(budget::Origin::Admission) {
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
            child: Some(status),
            execution_started,
            drain: if drained {
                budget::Drain::Complete
            } else {
                budget::Drain::Gapped
            },
            exit_code: code,
            lease: retired.then_some(budget::Lease::Retired),
        },
    );
    if let Some(released) = released {
        return Err(format!(
            "releasing slot {slot} failed after Cargo ran: {released}"
        ));
    }
    // A latched budget stop (an expired window) ends the command the same way:
    // the program is never launched after it.
    if stopped.is_some() || interrupted.is_some() || budget::origin().is_some() {
        return Ok(code);
    }
    if !run || !status.success() {
        return Ok(code);
    }
    if budget::has_execution() && budget::origin().is_none() {
        // A bounded `run` never reaches the release-then-exec hand-off.
        eprintln!("oxide: error: no bounded run target was recorded");
        return Ok(101);
    }
    match executable.unwrap_or_else(|| Err("no run target was recorded".to_owned())) {
        Ok(target) => {
            // A signal that arrived after Cargo's status was reported still
            // ends the command instead of launching a program nobody asked
            // for any more.
            if let Some(signal) = process::received_signal() {
                return Ok(process::signal_exit_code(signal));
            }
            // Lease released and this command owns no temporary directory,
            // so the program runs from the private target dir with the kept
            // inherited directory, in the environment `cargo run` gives it.
            let mut command = Command::new(&target.program);
            command.args(&summary.invocation.program_args);
            child_temp.apply(&mut command);
            if let Err(error) =
                identity::apply_run_environment(&mut command, &target.program, &target.manifest)
            {
                eprintln!(
                    "oxide: warning: {error}; the program runs without Cargo's full run environment"
                );
            }
            // A signal can arrive while the post-build environment queries run.
            if let Some(signal) = process::received_signal() {
                return Ok(process::signal_exit_code(signal));
            }
            let error = std::os::unix::process::CommandExt::exec(&mut command);
            Err(format!(
                "could not execute {}: {error}",
                target.program.display()
            ))
        }
        Err(message) => {
            eprintln!("oxide: error: {message}");
            Ok(101)
        }
    }
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
///
/// `lease` is what became of this command's slot: [`budget::Lease::Released`]
/// when none was taken yet, and otherwise the state its release left behind,
/// so the terminal record never claims a slot went back when it did not.
fn stop_for_admission(lease: budget::Lease) -> u8 {
    budget::latch(budget::Origin::Admission);
    budget::report_stop(budget::Origin::Admission);
    budget::finished(
        budget::Outcome::AdmissionDeadline,
        None,
        None,
        budget::Drain::NotReached,
        lease,
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

/// The lease state a finished release reports: `Released` only when the
/// release proved the slot went back, `Held` for a deferred one (owned scratch
/// cleanup could not confirm closure, so a later command keeps it).
fn release_lease_state(disposition: store::ReleaseDisposition) -> budget::Lease {
    match disposition {
        store::ReleaseDisposition::Released => budget::Lease::Released,
        store::ReleaseDisposition::Deferred => budget::Lease::Held,
    }
}

/// What one terminal budget record reports besides the lease state.
struct Terminal {
    outcome: budget::Outcome,
    child: Option<ExitStatus>,
    execution_started: Option<Instant>,
    drain: budget::Drain,
    exit_code: u8,
    /// A lease state this command's own outcome decides (a retired slot),
    /// used instead of the release's disposition.
    lease: Option<budget::Lease>,
}

/// Emit the invocation's one terminal budget event.
fn emit_terminal(lease: budget::Lease, terminal: Terminal) {
    budget::finished(
        terminal.outcome,
        terminal.child,
        terminal.execution_started,
        terminal.drain,
        terminal.lease.unwrap_or(lease),
        terminal.exit_code,
    );
}

/// Emit the invocation's one terminal budget event for a release that just
/// happened, and return the release's own error when it failed.
///
/// The lease state is the release's own disposition: `Released` only when the
/// release proved the slot went back, `Held` for a deferred or failed one. The
/// event is emitted before the error is looked at, so no release path can
/// `?`-return past it.
fn release_and_emit(
    release: Result<store::ReleaseDisposition, String>,
    terminal: Terminal,
) -> Option<String> {
    let lease = match &release {
        Ok(disposition) => release_lease_state(*disposition),
        Err(_) => budget::Lease::Held,
    };
    emit_terminal(lease, terminal);
    release.err()
}

/// Stop for the admission window after releasing the held slot, recording
/// what the release left behind. A failed release ends the command as an
/// internal error instead, and its record says the slot stays held.
fn stop_after_release(release: Result<store::ReleaseDisposition, String>) -> Result<u8, String> {
    match release {
        Ok(disposition) => Ok(stop_for_admission(release_lease_state(disposition))),
        Err(reason) => Err(release_failed(reason)),
    }
}

/// Keep the caller's own outcome after a release; a failed release becomes
/// this command's error, with the terminal record it deserves. The `?` this
/// backs can therefore never skip the event.
fn release_ok(release: Result<store::ReleaseDisposition, String>) -> Result<(), String> {
    match release {
        Ok(_) => Ok(()),
        Err(reason) => Err(release_failed(reason)),
    }
}

/// An error that ends the invocation after its lease was released: emit the
/// one terminal record (`internal_error` with the release's lease state and
/// exit code 1) and keep the release's own error beside the caller's.
fn release_error_after(
    release: Result<store::ReleaseDisposition, String>,
    error: String,
) -> String {
    let released = release_and_emit(
        release,
        Terminal {
            outcome: budget::Outcome::InternalError,
            child: None,
            execution_started: None,
            drain: budget::Drain::NotReached,
            exit_code: 1,
            lease: None,
        },
    );
    match released {
        None => error,
        Some(released) => format!("{error}; the release also failed: {released}"),
    }
}

/// Report a release that failed outside a stop: the terminal record says the
/// slot stays held with exit code 1, matching the error this command returns.
fn release_failed(reason: String) -> String {
    emit_terminal(
        budget::Lease::Held,
        Terminal {
            outcome: budget::Outcome::InternalError,
            child: None,
            execution_started: None,
            drain: budget::Drain::NotReached,
            exit_code: 1,
            lease: None,
        },
    );
    reason
}

fn fail_unstarted(custody: ProgramCustody<'_>, error: String) -> Result<u8, String> {
    let slot = custody.held.slot_id.clone();
    let released = release_and_emit(
        custody.store.release(custody.held, custody.origin),
        Terminal {
            outcome: budget::Outcome::SpawnFailure,
            child: None,
            execution_started: custody.execution_started,
            drain: budget::Drain::NotReached,
            exit_code: 1,
            lease: None,
        },
    );
    match released {
        None => Err(error),
        Some(release) => Err(format!(
            "{error}; releasing slot {slot} also failed: {release}"
        )),
    }
}

/// Run a converted checkout's program under the held lease, as `oxide exec`
/// runs a tool: its own process group, the record naming that group (so a
/// crash leaves the slot held until the group is empty), signals forwarded,
/// the removable pool's removal watchdog polled, and the slot released only
/// after the group is observed empty. A removal stop whose group cannot be
/// confirmed closed quarantines the slot as unknown custody.
fn run_owned_program(
    custody: ProgramCustody<'_>,
    target: OwnedProgram<'_>,
    budget_stop: &mut process::Stop<'_>,
) -> Result<u8, String> {
    let mut command = Command::new(target.program);
    command.args(target.args).env(
        "CARGO_MANIFEST_DIR",
        target.manifest.parent().unwrap_or(Path::new("")),
    );
    custody.child_temp.apply(&mut command);
    if let Err(error) =
        identity::apply_run_environment(&mut command, target.program, target.manifest)
    {
        eprintln!("oxide: warning: {error}; the program runs without Cargo's full run environment");
    }
    let broker = budget_stop.check_session().and_then(|()| {
        let mut broker = SessionBroker::for_child(custody.child_temp.path(), &mut command)?;
        arm_broker(broker.as_mut())?;
        Ok(broker)
    });
    let broker = match broker {
        Ok(broker) => broker,
        Err(error) => return fail_unstarted(custody, error),
    };
    if let Err(error) = custody.store.mark_spawning(&custody.held) {
        return fail_unstarted(custody, error);
    }
    let (mut group, foreground) = match process::start_foreground_group(&mut command) {
        Ok(started) => started,
        Err(error) => {
            return fail_unstarted(
                custody,
                format!("could not execute {}: {error}", target.program.display()),
            )
        }
    };
    budget::program_started();
    // Only a removable slot is watched, and only for removal: a program on
    // the internal disk is never stopped for host free space.
    let mut watchdog = match custody.store.backend {
        Backend::ApfsVolume => custody.store.watchdog(),
        Backend::DirectoryBudget => None,
    };
    if let Err(error) = custody.store.mark_running(&custody.held, group.id) {
        // The group never recorded as running. It is stopped with a bounded
        // escalation; the lease is released only once the group is confirmed
        // empty, and otherwise stays held for a later command to recover.
        let closed = process::stop_unrecorded(&mut group);
        drop(foreground);
        if !closed {
            budget::finished(
                budget::Outcome::UnconfirmedClosure,
                None,
                custody.execution_started,
                budget::Drain::NotReached,
                budget::Lease::Held,
                1,
            );
            return Err(format!(
                "{error}; program group {} did not close after KILL, so slot {} stays held",
                group.id, custody.held.slot_id
            ));
        }
        let slot = custody.held.slot_id.clone();
        let released = release_and_emit(
            custody.store.release(custody.held, None),
            Terminal {
                outcome: budget::Outcome::InternalError,
                child: None,
                execution_started: custody.execution_started,
                drain: budget::Drain::NotReached,
                exit_code: 1,
                lease: None,
            },
        );
        return Err(match released {
            None => error,
            Some(released) => format!("{error}; releasing slot {slot} also failed: {released}"),
        });
    }
    let status = match watchdog.as_mut() {
        Some(watchdog) => process::wait_leader_watched(&mut group, watchdog, budget_stop)?,
        None => process::wait_leader_budgeted(&mut group, budget_stop)?,
    };
    let closed = oxide_storage::release_trace::phase("run group closure", || {
        match (watchdog.as_mut(), status) {
            (Some(_), None) => None,
            (Some(watchdog), Some(_)) => {
                process::wait_group_empty_watched(group.id, watchdog, budget_stop)
            }
            (None, _) => process::wait_group_empty_budgeted(group.id, budget_stop),
        }
    });
    drop(foreground);
    drop(broker);
    let stopped = watchdog.as_ref().filter(|watchdog| watchdog.stopped());
    if let Some(watchdog) = stopped {
        report_stop(watchdog, group.id, closed, &custody.store.payload);
    }
    if closed.is_none() {
        if let Some(origin) = budget::origin() {
            budget::report_stop(origin);
        }
        budget::finished(
            budget::Outcome::UnconfirmedClosure,
            status,
            custody.execution_started,
            budget::Drain::NotReached,
            budget::Lease::Held,
            1,
        );
        // Only a removable volume's removal stop is unknown custody worth a
        // quarantine. An internal pool keeps the lease held, and the record
        // names the group, so a later command recovers it once it is empty.
        if stopped.is_some_and(Watchdog::removal) {
            custody.store.quarantine_lost(
                &custody.held,
                &format!("run program group {} did not close after KILL", group.id),
            )?;
            return Ok(1);
        }
        eprintln!(
            "oxide: could not confirm program group {} closed after the stop; slot {} stays held, and a later oxide command recovers it once the group is empty",
            group.id, custody.held.slot_id
        );
        return Ok(1);
    }
    if budget::origin().is_none() {
        if let Err(error) = budget_stop.check_session() {
            return Err(release_error_after(
                custody.finish_and_release(false, false),
                error,
            ));
        }
    }
    let seed_successful = status.is_some_and(|status| status.success())
        && stopped.is_none()
        && budget::origin().is_none();
    // Cargo already succeeded before `run_owned_program` launched the built binary.
    let prune_successful =
        stopped.is_none() && budget::origin().is_none() && process::received_signal().is_none();
    let execution_started = custody.execution_started;
    let slot = custody.held.slot_id.clone();
    // The release runs before the code is settled, but its failure never
    // suppresses the terminal event: the code this command returns and the
    // event always agree, and the release's own error is preserved.
    let release = custody.finish_and_release(seed_successful, prune_successful);
    if let Some(origin) = budget::origin() {
        budget::report_stop(origin);
    }
    let code = status.map_or(1, process::exit_code);
    let code = if stopped.is_some() { code.max(1) } else { code };
    let code = budget::exit_code(code);
    let released = release_and_emit(
        release,
        Terminal {
            outcome: if budget::origin().is_some() {
                budget::Outcome::ExecutionDeadline
            } else if status.is_some_and(|status| {
                std::os::unix::process::ExitStatusExt::signal(&status).is_some()
            }) {
                budget::Outcome::ChildSignal
            } else {
                budget::Outcome::ChildExit
            },
            child: status,
            execution_started,
            drain: budget::Drain::Complete,
            exit_code: code,
            lease: None,
        },
    );
    match released {
        None => Ok(code),
        Some(released) => Err(format!(
            "releasing slot {slot} failed after the program ran: {released}"
        )),
    }
}

/// One reserve-stop line, then a read-only capacity diagnostic for the same
/// filesystem once the stopped group has positively closed (oxide#130). An
/// unconfirmed closure, and a removable volume's own removal stop, probe
/// nothing: the diagnostic runs after writers are closed, never during
/// polling.
pub(crate) fn report_stop(
    watchdog: &Watchdog,
    group: i32,
    closed: Option<Instant>,
    payload: &Path,
) {
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
    let release = store.release(held, None);
    if budget::admission_expired() {
        return stop_after_release(release);
    }
    match release {
        Ok(_) => Err(reason),
        Err(released) => Err(release_failed(released)),
    }
}

/// Release a failed-walk recipient before using the ordinary admission loop.
fn abandon_seed_and_reacquire(
    request: &LeaseRequest<'_>,
    bind_plan: &OutputBindPlan<'_>,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
    held: store::Lease,
    binding: outputs::OutputBinding,
) -> Result<Result<(store::Lease, Gate, outputs::OutputBinding), Stop>, String> {
    drop(binding);
    release_ok(request.store.release(held, None))?;
    if let Some(joined) = request.joined {
        joined.return_permit();
    }
    grants.share = false;
    let (held, gate) = match lease_with_gate(request, waits, grants)? {
        Ok(leased) => leased,
        Err(stop) => return Ok(Err(stop)),
    };
    match outputs::bind_for_command(
        request.store,
        &request.key.checkout,
        held.record_id(),
        bind_plan.planned,
        bind_plan.pool_fallback,
        bind_plan.derives_from_receipt,
    ) {
        Ok(binding) => Ok(Ok((held, gate, binding))),
        Err(reason) => {
            let origin = held.prior.unchanged();
            release_ok(request.store.release(held, origin))?;
            if budget::admission_expired() {
                Ok(Err(Stop::Admission))
            } else {
                Err(reason)
            }
        }
    }
}

fn lease_with_gate(
    request: &LeaseRequest<'_>,
    waits: &mut store::WaitAccounting,
    grants: &mut LeaseGrants,
) -> Result<Result<(store::Lease, Gate), Stop>, String> {
    // A refusal leaves this checkout without a record, so the next lease is
    // a new or evicted slot. At most one grant per project slot plus one cold
    // fallback (oxide#118, oxide#177).
    while grants.remaining > 0 {
        let mut held = match request.store.acquire(
            request.key,
            request.profile,
            grants.share,
            request.label,
            waits,
            request.joined,
        )? {
            Acquired::Lease(held) => held,
            Acquired::Interrupted(signal) => return Ok(Err(Stop::Signal(signal))),
            Acquired::AdmissionExpired => return Ok(Err(Stop::Admission)),
        };
        grants.taken();
        let refusal = freshness::prepare(&request.key.checkout, &held.prior, request.resolution);
        // Taken just now, or left on this checkout's record by a command
        // that died before Cargo.
        let foreign = held.prior.foreign(&request.key.checkout);
        let reason = match refusal {
            Ok(gate) => {
                // First get a usable slot, then compete for the host permit.
                // Taking it before the checkout's output binding lock lets
                // any current permit holder finish starting.
                if let Some(joined) = request.joined.filter(|_| request.label.is_none()) {
                    if let Err(stop) = joined.borrow() {
                        let origin = held.prior.unchanged();
                        release_ok(request.store.release(held, origin))?;
                        return match stop {
                            BorrowStop::Admission => Ok(Err(Stop::Admission)),
                            BorrowStop::Signal(signal) => Ok(Err(Stop::Signal(signal))),
                            BorrowStop::Failed(error) => Err(error),
                        };
                    }
                }
                return Ok(Ok((held, gate)));
            }
            Err(Refusal::Unvouched(reason)) => reason,
            Err(Refusal::Lock { code, message }) => {
                eprint!("{message}");
                if held.shared_from.is_none() && foreign.is_some() {
                    held.shared_from = foreign;
                }
                // Metadata runs only for another checkout's slot.
                if held.shared_from.is_some() {
                    request.store.unshare(held)?;
                } else {
                    let origin = held.prior.unchanged();
                    request.store.release(held, origin)?;
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
            request.store.unshare(held)?;
        } else {
            eprintln!(
                "oxide: freshness gate cannot vouch for slot {} ({reason}); evicting it and building cold",
                held.slot_id
            );
            request.store.discard(held)?;
        }
    }
    Err("the freshness gate kept refusing slots".to_owned())
}

/// The program `run` built and the package manifest it belongs to.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RunProgram {
    program: PathBuf,
    manifest: PathBuf,
}

type RunTarget = Result<RunProgram, String>;

/// `run`: wait for the build while reading Cargo's JSON for the executable.
fn collect_run(
    group: &mut process::Group,
    invocation: &Invocation,
    mut watchdog: Option<&mut Watchdog>,
    budget_stop: &mut process::Stop<'_>,
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
    // A closed stdout (a caller that went away, a full pipe) must not panic
    // while this command holds the lease: stop forwarding and keep draining,
    // so Cargo still gets its stdout read and its group still closes.
    let mut forwarding = invocation.json_messages;
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
                if forwarding && writeln!(std::io::stdout().lock(), "{line}").is_err() {
                    forwarding = false;
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
            let (_, program, manifest) = found.remove(0);
            Ok(RunProgram { program, manifest })
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
    let manifest = PathBuf::from(message["manifest_path"].as_str()?);
    Some((name.to_owned(), PathBuf::from(executable), manifest))
}

/// One line: profile, strategy, slot warmth, elapsed time, the slot's size
/// when leased and this command's budget. It walks nothing: the size is the
/// library's reuse walk of that exact slot generation, never a walk after
/// release, when another command may hold the slot.
fn print_summary(ctx: &SummaryContext<'_>) {
    let strategy = match ctx.invocation.command.as_str() {
        "run" | "test" | "exec" => "build+execute",
        _ => "build",
    };
    let size = leased_size(ctx.measured, ctx.warm);
    let pool = format!(
        "{size}, {} MiB budget, pool total: see oxide status",
        ctx.store.command_budget() / MIB
    );
    let backend = match ctx.store.backend {
        Backend::DirectoryBudget => format!(" ({}, budget-only)", ctx.store.backend.tag()),
        Backend::ApfsVolume => format!(
            " ({}, budget-only, removable {})",
            ctx.store.backend.tag(),
            ctx.store.payload.display()
        ),
    };
    let note = ctx.note.map(|note| format!(", {note}")).unwrap_or_default();
    let wait = ctx.waits.summary_suffix().unwrap_or_default();
    let admission = oxide_storage::lease::admission_cost::take_summary()
        .map_or_else(String::new, |summary| format!(", {summary}"));
    eprintln!(
        "oxide: {} profile, {strategy} strategy, slot {} {}, {:.1}s, {pool}{backend}{note}{wait}{admission}",
        ctx.invocation.profile.label(),
        ctx.slot,
        if ctx.warm { "warm" } else { "cold" },
        ctx.started.elapsed().as_secs_f64()
    );
}

/// An acquisition without a walk has no fresh size, even on warm reuse.
pub(crate) fn leased_size(measured: Option<&ReuseMeasurement>, warm: bool) -> String {
    match measured {
        Some(measured) => format!(
            "slot {} was {} MiB when leased",
            measured.slot_id,
            measured.unique_allocated_bytes / MIB
        ),
        None if !warm => "new slot".to_owned(),
        None => "slot size unknown".to_owned(),
    }
}

/// How long a command waits at exit for the post-rename trash deletion of an
/// idle slot it evicted. The deletion itself runs beside the writer and is
/// resumed by a later command; this bound only keeps one exit from hanging on a
/// huge tree.
const TRASH_SETTLE_WAIT: Duration = Duration::from_secs(60);
/// How often the bounded exit wait reports its progress.
const TRASH_SETTLE_REPORT: Duration = Duration::from_secs(2);

/// Deletes the Barge trash of idle slots this command evicted, and of any an
/// earlier command left, with the pool and admission locks released
/// (oxide#324).
///
/// The admission commits the decision (the root leaves the ledger) under its
/// pool lock and records it durably; Barge's eviction, which rechecks the
/// marker, the hold and the pin and renames into `.trash/`, then runs here,
/// beside the writer. A command that exits mid-deletion leaves the durable
/// record and the trash, and the next command resumes it.
struct TrashSettler {
    worker: Option<std::thread::JoinHandle<()>>,
    kick: Option<std::sync::mpsc::Sender<()>>,
}

impl TrashSettler {
    /// Start the settle worker for this command's pool. It settles at once and
    /// again after every [`Self::kick`].
    fn start(pool: PathBuf) -> Self {
        let (kick, waits) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("oxide-trash".to_owned())
            .spawn(move || {
                report_trash_settle(&pool);
                while waits.recv().is_ok() {
                    report_trash_settle(&pool);
                }
            })
            .ok();
        Self {
            worker,
            kick: Some(kick),
        }
    }

    /// Ask for another pass: this command's admission has committed more
    /// evictions since the worker's last one.
    fn kick(&self) {
        if let Some(kick) = self.kick.as_ref() {
            let _ = kick.send(());
        }
    }

    /// Wait for the worker with a bounded wait, printing its progress. A
    /// worker still running at the bound is detached: its durable record stays,
    /// and the next command resumes it.
    fn finish(&mut self) {
        self.finish_while(|| process::received_signal().is_some() || budget::origin().is_some());
    }

    /// [`Self::finish`] giving up as soon as `interrupted` reports that this
    /// command is already ending. A signal or a latched budget stop has asked
    /// it to stop, so it never waits out the settle bound: the deletion's
    /// durable record stays and the next command resumes it.
    fn finish_while(&mut self, interrupted: impl Fn() -> bool) {
        // Ending the channel lets the worker leave its loop after the pass it
        // is running.
        self.kick.take();
        if self.worker.is_none() {
            return;
        }
        let started = Instant::now();
        let bound = settle_bound(
            budget::clamp(TRASH_SETTLE_WAIT),
            budget::execution_remaining(),
            budget::origin().is_some(),
        );
        let mut next_report = TRASH_SETTLE_REPORT;
        loop {
            let finished = self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.is_finished());
            if finished {
                if let Some(worker) = self.worker.take() {
                    let _ = worker.join();
                }
                return;
            }
            if interrupted() {
                self.worker = None;
                eprintln!("{}", trash_interrupted_message(started.elapsed()));
                return;
            }
            let elapsed = started.elapsed();
            if elapsed >= bound {
                self.worker = None;
                eprintln!("{}", trash_wait_message(elapsed.as_secs()));
                return;
            }
            if elapsed >= next_report {
                eprintln!("{}", trash_progress_message(elapsed.as_secs()));
                next_report += TRASH_SETTLE_REPORT;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TrashSettler {
    /// Every return path of a command joins its own deletion, never only the
    /// happy one.
    fn drop(&mut self) {
        self.finish();
    }
}

/// The longest an exiting command waits for its settle worker. A latched budget
/// stop leaves no wait at all: the command is already over its window, and the
/// durable record carries the deletion to the next command.
fn settle_bound(
    bound: Duration,
    execution_left: Option<Duration>,
    budget_stopped: bool,
) -> Duration {
    if budget_stopped {
        return Duration::ZERO;
    }
    match execution_left {
        Some(left) => bound.min(left),
        None => bound,
    }
}

/// The bounded exit wait ran out while an evicted idle slot's trash was still
/// being deleted.
fn trash_wait_message(seconds: u64) -> String {
    format!(
        "oxide: an evicted idle slot's trash is still being deleted after {seconds}s; its durable record stays, and the next oxide command resumes it"
    )
}

/// A signal ended this command while an evicted idle slot's trash was still
/// being deleted: the same durable record resumes it.
fn trash_interrupted_message(elapsed: Duration) -> String {
    format!(
        "oxide: an evicted idle slot's trash is still being deleted ({}s); the signal ends this command, its durable record stays, and the next oxide command resumes it",
        elapsed.as_secs()
    )
}

/// One progress line while the trash of an evicted idle slot is deleted.
fn trash_progress_message(seconds: u64) -> String {
    format!(
        "oxide: still deleting an evicted idle slot's Barge trash outside the pool lock ({seconds}s; the build and every other command are not held)"
    )
}

/// One settle pass: Barge evicts each root a command decided to retire and
/// deletes its trash. A failure is reported; the record stays for a later pass.
fn report_trash_settle(pool: &Path) {
    match oxide_storage::lease::managed::settle_pending_evictions(pool) {
        Ok(messages) => {
            for message in messages {
                eprintln!("oxide: {message}");
            }
        }
        // A deletion that cannot continue is reported, never fatal: the root's
        // durable record stays and a later command asks Barge again.
        Err(error) => {
            eprintln!("oxide: an evicted idle slot's trash could not be settled: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;

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
        let binding =
            outputs::bind_for_command(&store, &key.checkout, held.record_id(), None, None, false)
                .unwrap();
        let mut grants = LeaseGrants::new();
        grants.taken();
        let mut waits = store::WaitAccounting::default();
        let resolution = cli::Resolution::default();
        let request = LeaseRequest {
            store: &store,
            key: &key,
            profile: cli::Profile::Quick,
            resolution: &resolution,
            label: None,
            joined: None,
        };
        let bind_plan = OutputBindPlan {
            planned: None,
            pool_fallback: None,
            derives_from_receipt: false,
        };
        let result = abandon_seed_and_reacquire(
            &request,
            &bind_plan,
            &mut waits,
            &mut grants,
            held,
            binding,
        )
        .unwrap();
        let Ok((cold, _, binding)) = result else {
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
        let binding =
            outputs::bind_for_command(&store, &key.checkout, held.record_id(), None, None, false)
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

    fn event_pipe() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let mut fds = [0_i32; 2];
        // SAFETY: two valid output slots.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both descriptors are ours and valid.
        unsafe {
            libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
            (
                std::fs::File::from_raw_fd(fds[0]),
                std::fs::File::from_raw_fd(fds[1]),
            )
        }
    }

    fn drain_event_records(reader: &mut std::fs::File) -> Vec<serde_json::Value> {
        use std::io::Read;
        let mut text = String::new();
        let mut chunk = [0_u8; 1024];
        for _ in 0..50 {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => text.push_str(&String::from_utf8_lossy(&chunk[..read])),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        }
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    /// Regression: the spawn-failure path released the lease with `?` before
    /// `budget::finished`, so a failing release ended the invocation with no
    /// terminal event at all. The event is emitted first, the release error is
    /// preserved, and the lease is reported held.
    #[test]
    fn a_failed_release_still_emits_the_terminal_budget_event() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } == 0 {
            return; // The superuser can write in a read-only directory.
        }
        let (mut reader, writer) = event_pipe();
        budget::clear();
        budget::install(budget::Options {
            admission: None,
            execution: None,
            events: Some(writer.as_raw_fd()),
        })
        .expect("a clock");
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        let chosen = child_temp::Chosen::Inherited(std::env::temp_dir());
        let custody = ProgramCustody {
            store: &store,
            held,
            origin: None,
            execution_started: None,
            child_temp: &chosen,
            production: None,
            seed_profile: "debug",
            prune_session: None,
        };
        // A control directory the caller cannot write makes the release's
        // final state write fail after the child never started.
        let control = store.pool.join(oxide_storage::lease::MANAGED_NAME);
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o500))
            .expect("read-only control");
        let result = fail_unstarted(custody, "could not start the program".to_owned());
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o700))
            .expect("restore control");
        let error = result.expect_err("the release failure is preserved");
        assert!(error.contains("could not start the program"), "{error}");
        assert!(error.contains("also failed"), "{error}");
        let records = drain_event_records(&mut reader);
        assert!(
            records.iter().any(|record| record["event"] == "finished"
                && record["outcome"] == "spawn_failure"
                && record["lease"] == "held"),
            "{records:?}"
        );
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// Arm the budget with an event sink. The writer is returned too: the
    /// installed state holds only its descriptor, so it must outlive the event.
    fn install_event_sink() -> (std::fs::File, std::fs::File) {
        use std::os::fd::AsRawFd;
        let (reader, writer) = event_pipe();
        budget::clear();
        budget::install(budget::Options {
            admission: None,
            execution: None,
            events: Some(writer.as_raw_fd()),
        })
        .expect("a clock");
        (reader, writer)
    }

    fn finished_leases(mut reader: std::fs::File) -> Vec<String> {
        let records = drain_event_records(&mut reader);
        records
            .iter()
            .filter(|record| record["event"] == "finished")
            .filter_map(|record| record["lease"].as_str().map(str::to_owned))
            .collect()
    }

    /// Regression: a deferred release returns `Ok` from `Store::release` while
    /// keeping the lease held (owned scratch cleanup could not confirm
    /// closure), so treating every `Ok` as `released` emitted a false terminal
    /// event. Only a ledger without this lease proves the slot went back.
    #[test]
    fn a_deferred_release_is_reported_held() {
        let (reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let mut held = store::tests::seed_fixture_lease(&store, &key);
        let slot_dir = held.build_dir.parent().expect("slot dir").to_path_buf();
        let scratch = child_temp::choose_with(
            None,
            &store.payload,
            &slot_dir,
            child_temp::Identity {
                store_id: held.store_id.clone(),
                slot_id: held.slot_id.clone(),
                incarnation: held.incarnation.clone(),
                lease_id: held.lease_id().to_owned(),
            },
            4096,
        )
        .expect("the owned directory");
        let owned = scratch.owned().expect("owned directory");
        // Removing the ownership marker makes cleanup refuse, so the release
        // defers instead of handing the slot back.
        std::fs::remove_file(owned.path().join("owner")).expect("the marker");
        held.scratch = Some(owned.clone());
        let chosen = child_temp::Chosen::Inherited(std::env::temp_dir());
        let custody = ProgramCustody {
            store: &store,
            held,
            origin: None,
            execution_started: None,
            child_temp: &chosen,
            production: None,
            seed_profile: "debug",
            prune_session: None,
        };
        let error = fail_unstarted(custody, "could not start the program".to_owned())
            .expect_err("the spawn failure is still an error");
        assert_eq!(error, "could not start the program");
        assert_eq!(finished_leases(reader), ["held"]);
        assert!(owned.path().is_dir(), "a deferred scratch is kept");
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// The ordinary success: the release reports `Released`, so the terminal
    /// event says so. This keeps the deferred case from being "fixed" by
    /// labelling every spawn failure held.
    #[test]
    fn a_completed_release_is_reported_released() {
        let (reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        let chosen = child_temp::Chosen::Inherited(std::env::temp_dir());
        let custody = ProgramCustody {
            store: &store,
            held,
            origin: None,
            execution_started: None,
            child_temp: &chosen,
            production: None,
            seed_profile: "debug",
            prune_session: None,
        };
        let error = fail_unstarted(custody, "could not start the program".to_owned())
            .expect_err("the spawn failure is still an error");
        assert_eq!(error, "could not start the program");
        assert_eq!(finished_leases(reader), ["released"]);
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// The terminal record carries the code the invocation returns, so a tool
    /// that could not be started is reported 127 (not found) or 126 (not
    /// runnable) rather than a generic 1.
    #[test]
    fn the_spawn_failure_record_carries_the_code_the_command_returns() {
        let (mut reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        let origin = held.prior.unchanged();
        assert_eq!(
            release_and_emit(
                store.release(held, origin),
                Terminal {
                    outcome: budget::Outcome::SpawnFailure,
                    child: None,
                    execution_started: None,
                    drain: budget::Drain::NotReached,
                    exit_code: 127,
                    lease: None,
                },
            ),
            None
        );
        let records = drain_event_records(&mut reader);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["outcome"], "spawn_failure");
        assert_eq!(records[0]["lease"], "released");
        assert_eq!(records[0]["exit_code"], 127);
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// Regression: `stop_for_admission` always reported `released`, so an
    /// admission stop after a deferred release claimed the slot went back
    /// while it was still held.
    #[test]
    fn an_admission_stop_reports_a_deferred_release_as_held() {
        let (reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let mut held = store::tests::seed_fixture_lease(&store, &key);
        let slot_dir = held.build_dir.parent().expect("slot dir").to_path_buf();
        let scratch = child_temp::choose_with(
            None,
            &store.payload,
            &slot_dir,
            child_temp::Identity {
                store_id: held.store_id.clone(),
                slot_id: held.slot_id.clone(),
                incarnation: held.incarnation.clone(),
                lease_id: held.lease_id().to_owned(),
            },
            4096,
        )
        .expect("the owned directory");
        let owned = scratch.owned().expect("owned directory");
        // Removing the ownership marker makes cleanup refuse, so the release
        // defers instead of handing the slot back.
        std::fs::remove_file(owned.path().join("owner")).expect("the marker");
        held.scratch = Some(owned.clone());
        let origin = held.prior.unchanged();
        assert_eq!(
            stop_after_release(store.release(held, origin)).expect("the admission stop"),
            124
        );
        assert_eq!(finished_leases(reader), ["held"]);
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// The ordinary admission stop: the release reported `Released`, so the
    /// terminal record says so.
    #[test]
    fn an_admission_stop_reports_a_completed_release_as_released() {
        let (reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        let origin = held.prior.unchanged();
        assert_eq!(
            stop_after_release(store.release(held, origin)).expect("the admission stop"),
            124
        );
        assert_eq!(finished_leases(reader), ["released"]);
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    /// A release that fails at an admission stop ends the command as an
    /// internal error whose record says the slot stays held; the stop is not
    /// reported as if the slot had gone back.
    #[test]
    fn a_failed_release_at_an_admission_stop_is_an_internal_error() {
        use std::os::unix::fs::PermissionsExt;

        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } == 0 {
            return; // The superuser can write in a read-only directory.
        }
        let (mut reader, _writer) = install_event_sink();
        let (root, store, key) = store::tests::seed_fixture();
        let held = store::tests::seed_fixture_lease(&store, &key);
        let origin = held.prior.unchanged();
        let control = store.pool.join(oxide_storage::lease::MANAGED_NAME);
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o500))
            .expect("read-only control");
        let result = stop_after_release(store.release(held, origin));
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o700))
            .expect("restore control");
        assert!(result.is_err(), "a failed release is not a stop");
        let records = drain_event_records(&mut reader);
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0]["outcome"], "internal_error");
        assert_eq!(records[0]["lease"], "held");
        assert_eq!(records[0]["exit_code"], 1);
        budget::clear();
        std::fs::remove_dir_all(root).expect("remove own fixture");
    }

    #[test]
    fn leased_size_is_honest_without_a_walk() {
        assert_eq!(leased_size(None, true), "slot size unknown");
        assert_eq!(leased_size(None, false), "new slot");
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

    /// A signal that already ends this command never waits out the settle
    /// bound for an evicted slot's deletion: the worker is detached and its
    /// durable record resumes on the next command. Counterfactual: the wait
    /// polled only the worker and the bound.
    #[test]
    fn a_signal_stops_waiting_for_an_evicted_slots_deletion() {
        let mut settler = TrashSettler {
            worker: Some(
                std::thread::Builder::new()
                    .name("oxide-test-trash".to_owned())
                    .spawn(|| std::thread::sleep(Duration::from_secs(3)))
                    .expect("a worker"),
            ),
            kick: None,
        };
        let started = Instant::now();
        settler.finish_while(|| true);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the settle wait did not stop for the ending command"
        );
        assert!(
            settler.worker.is_none(),
            "the still-running worker stays detached"
        );
    }

    /// Regression: an exit after a latched budget stop (an expired window)
    /// waited out the whole settle bound for the deletion. It now detaches at
    /// once, and the bound itself is zero while the stop is latched.
    #[test]
    fn a_latched_budget_stop_does_not_wait_out_the_settle_bound() {
        assert_eq!(settle_bound(TRASH_SETTLE_WAIT, None, true), Duration::ZERO);
        assert_eq!(
            settle_bound(TRASH_SETTLE_WAIT, None, false),
            TRASH_SETTLE_WAIT
        );
        // An execution window that is nearly spent cuts the wait to what is left.
        assert_eq!(
            settle_bound(TRASH_SETTLE_WAIT, Some(Duration::from_millis(200)), false),
            Duration::from_millis(200)
        );
        budget::clear();
        budget::install(budget::Options {
            admission: None,
            execution: Some(Duration::from_secs(60)),
            events: None,
        })
        .expect("a budget");
        budget::latch(budget::Origin::Execution);
        let mut settler = TrashSettler {
            worker: Some(
                std::thread::Builder::new()
                    .name("oxide-test-trash-latched".to_owned())
                    .spawn(|| std::thread::sleep(Duration::from_secs(3)))
                    .expect("a worker"),
            ),
            kick: None,
        };
        let started = Instant::now();
        settler.finish_while(|| budget::origin().is_some());
        budget::clear();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the settle wait ran out the bound after a budget stop"
        );
        assert!(settler.worker.is_none());
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

    fn lease_with_deferred_scratch(store: &Store, key: &SlotKey) -> (store::Lease, PathBuf) {
        let mut held = store::tests::seed_fixture_lease(store, key);
        let slot_dir = held.build_dir.parent().expect("slot dir").to_path_buf();
        let scratch = child_temp::choose_with(
            None,
            &store.payload,
            &slot_dir,
            child_temp::Identity {
                store_id: held.store_id.clone(),
                slot_id: held.slot_id.clone(),
                incarnation: held.incarnation.clone(),
                lease_id: held.lease_id().to_owned(),
            },
            4096,
        )
        .expect("the owned directory");
        let owned = scratch.owned().expect("owned directory");
        std::fs::remove_file(owned.path().join("owner")).expect("the marker");
        let owned_path = owned.path().to_path_buf();
        held.scratch = Some(owned);
        (held, owned_path)
    }

    #[test]
    fn finish_released_invocation_reports_deferred_release_as_held() {
        for deferred in [true, false] {
            let (reader, _writer) = install_event_sink();
            let (root, store, key) = store::tests::seed_fixture();
            let (held, owned_path) = if deferred {
                let (held, path) = lease_with_deferred_scratch(&store, &key);
                (held, Some(path))
            } else {
                (store::tests::seed_fixture_lease(&store, &key), None)
            };
            let slot = held.slot_id.clone();
            let chosen = child_temp::Chosen::Inherited(std::env::temp_dir());
            let mut invocation = budget_invocation();
            invocation.command = "check".to_owned();
            let waits = store::WaitAccounting::default();
            let summary = SummaryContext {
                store: &store,
                invocation: &invocation,
                slot: &slot,
                warm: false,
                measured: None,
                started: Instant::now(),
                note: None,
                waits: &waits,
            };
            let custody = ProgramCustody {
                store: &store,
                held,
                origin: None,
                execution_started: None,
                child_temp: &chosen,
                production: None,
                seed_profile: "debug",
                prune_session: None,
            };
            let status = Command::new("/bin/sh")
                .args(["-c", "exit 0"])
                .status()
                .unwrap();
            let execution = PrimaryExecution {
                outcome: Ok((Some(status), None, true)),
                execution_started: None,
                budget_stop: process::Stop::new(),
                watchdog: None,
                target_entries_before: None,
            };
            assert_eq!(
                finish_released_invocation(custody, &summary, execution).unwrap(),
                0
            );
            let expected = if deferred { "held" } else { "released" };
            assert_eq!(finished_leases(reader), [expected]);
            if let Some(path) = owned_path {
                assert!(path.is_dir(), "a deferred scratch is kept");
            }
            budget::clear();
            std::fs::remove_dir_all(root).expect("remove own fixture");
        }
    }

    #[test]
    fn run_owned_program_reports_deferred_release_as_held() {
        for deferred in [true, false] {
            let (reader, _writer) = install_event_sink();
            let (root, store, key) = store::tests::seed_fixture();
            let (held, owned_path) = if deferred {
                let (held, path) = lease_with_deferred_scratch(&store, &key);
                (held, Some(path))
            } else {
                (store::tests::seed_fixture_lease(&store, &key), None)
            };
            let chosen = child_temp::Chosen::Inherited(std::env::temp_dir());
            let custody = ProgramCustody {
                store: &store,
                held,
                origin: None,
                execution_started: None,
                child_temp: &chosen,
                production: None,
                seed_profile: "debug",
                prune_session: None,
            };
            let manifest = root.join("Cargo.toml");
            let args = [OsString::from("-c"), OsString::from("exit 0")];
            let mut stop = process::Stop::new();
            let code = run_owned_program(
                custody,
                OwnedProgram {
                    program: Path::new("/bin/sh"),
                    manifest: &manifest,
                    args: &args,
                },
                &mut stop,
            )
            .unwrap();
            assert_eq!(code, 0);
            let expected = if deferred { "held" } else { "released" };
            assert_eq!(finished_leases(reader), [expected]);
            if let Some(path) = owned_path {
                assert!(path.is_dir(), "a deferred scratch is kept");
            }
            budget::clear();
            std::fs::remove_dir_all(root).expect("remove own fixture");
        }
    }

    fn toolchain_installed_matching(toolchain: &str, version_prefix: &str) -> bool {
        Command::new("rustup")
            .args(["run", toolchain, "cargo", "-V"])
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).starts_with(version_prefix)
            })
    }

    fn clean_cargo_for(toolchain: &str, dir: &Path) -> Command {
        let mut command = Command::new(CARGO);
        command.current_dir(dir).env("RUSTUP_TOOLCHAIN", toolchain);
        command.env("CARGO_TERM_COLOR", "never");
        for name in [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_PROFILE_DEV_DEBUG",
            "CARGO_PROFILE_DEV_BUILD_OVERRIDE_DEBUG",
            "CARGO_PROFILE_DEBUG_DEBUG",
            "CARGO_PROFILE_DEBUG_BUILD_OVERRIDE_DEBUG",
            "CARGO_INCREMENTAL",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn create_profile_probe_workspace(root: &Path, extra_app_toml: &str) -> (PathBuf, PathBuf) {
        let dep = root.join("dep");
        let app = root.join("app");
        let slot_build = root.join("slot/build");
        std::fs::create_dir_all(dep.join("src")).expect("dep src");
        std::fs::create_dir_all(app.join("src")).expect("app src");
        std::fs::create_dir_all(&slot_build).expect("slot build");
        std::fs::write(
            dep.join("Cargo.toml"),
            "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("dep manifest");
        std::fs::write(dep.join("src/lib.rs"), "pub fn value() -> u32 { 7 }\n").expect("dep lib");
        std::fs::write(
            app.join("Cargo.toml"),
            format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ndep = {{ path = \"../dep\" }}\n{extra_app_toml}"
            ),
        )
        .expect("app manifest");
        std::fs::write(app.join("build.rs"), "fn main() {}\n").expect("build script");
        std::fs::write(
            app.join("src/main.rs"),
            "fn main() { println!(\"{}\", dep::value()); }\n#[test]\nfn ok() { assert_eq!(dep::value(), 7); }\n",
        )
        .expect("app main");
        (app, slot_build)
    }

    fn rustc_line<'a>(stderr: &'a str, crate_name: &str) -> &'a str {
        let marker = format!("--crate-name {crate_name} ");
        stderr
            .lines()
            .find(|line| line.trim_start().starts_with("Running `") && line.contains(&marker))
            .unwrap_or_else(|| panic!("missing rustc line for {crate_name} in:\n{stderr}"))
    }

    #[test]
    fn cargo_1_100_native_debug_profile_keeps_dev_recipe_requirements() {
        const VERSION_PREFIX: &str = "cargo 1.100.0 (3acc79c0c ";
        if !toolchain_installed_matching("1.100.0", VERSION_PREFIX) {
            eprintln!("skipped: Cargo 1.100.0 is not installed");
            return;
        }
        let fixture = crate::cargo_layout::support::Fixture::new("profile-1-100");
        let (app, _slot_unused) = create_profile_probe_workspace(&fixture.0, "");

        // 1. Native `cargo build --profile debug` on 1.100.0 outputs to
        // `target/debug/`, yet omits debuginfo on build scripts (`build-override`).
        let native = clean_cargo_for("1.100.0", &app)
            .args(["build", "--profile", "debug", "-v"])
            .output()
            .expect("cargo 1.100.0 --profile debug");
        let native_stderr = String::from_utf8_lossy(&native.stderr);
        assert!(native.status.success(), "{native_stderr}");
        assert!(app.join("target/debug/app").is_file());
        assert!(
            !rustc_line(&native_stderr, "build_script_build").contains("debuginfo="),
            "expected native --profile debug to omit build-script debuginfo:\n{native_stderr}"
        );
        assert!(rustc_line(&native_stderr, "dep").contains("-C debuginfo=2"));
        assert!(rustc_line(&native_stderr, "app").contains("-C debuginfo=2"));

        // 2. Native `--profile debug` inherits `dev` (`CARGO_PROFILE_DEV_DEBUG`).
        std::fs::remove_dir_all(app.join("target")).expect("clean target");
        let inherited = clean_cargo_for("1.100.0", &app)
            .args(["build", "--profile", "debug", "-v"])
            .env("CARGO_PROFILE_DEV_DEBUG", "line-tables-only")
            .output()
            .expect("cargo 1.100.0 --profile debug with dev override");
        let inherited_stderr = String::from_utf8_lossy(&inherited.stderr);
        assert!(inherited.status.success(), "{inherited_stderr}");
        assert!(rustc_line(&inherited_stderr, "dep").contains("-C debuginfo=line-tables-only"));
        assert!(rustc_line(&inherited_stderr, "app").contains("-C debuginfo=line-tables-only"));

        // 3. Oxide's `dev` recipe (`Profile::Debug`) overrides `[profile.dev]`
        // settings, compiles workspace, dependency and build-script units with
        // `-C debuginfo=2`, preserves `[profile.test]` inheritance on `test`,
        // and keeps final outputs in `target/debug/` while slot intermediates
        // stay in `slot_build`.
        let (app_custom, slot_custom) = create_profile_probe_workspace(
            &fixture.0.join("custom"),
            "\n[profile.dev]\ndebug = false\n\n[profile.test]\nopt-level = 1\n",
        );
        let mut debug_build = clean_cargo_for("1.100.0", &app_custom);
        debug_build.args(["build", "-v"]);
        apply_storage(&mut debug_build, &slot_custom, &app_custom, Profile::Debug);
        let debug_out = debug_build.output().expect("oxide debug build");
        let debug_stderr = String::from_utf8_lossy(&debug_out.stderr);
        assert!(debug_out.status.success(), "{debug_stderr}");
        assert!(app_custom.join("target/debug/app").is_file());
        assert!(slot_custom.join("debug").is_dir());
        for unit in ["build_script_build", "dep", "app"] {
            assert!(
                rustc_line(&debug_stderr, unit).contains("-C debuginfo=2"),
                "expected full debuginfo for {unit}:\n{debug_stderr}"
            );
        }

        // `test` inherits `[profile.test]` (`opt-level=1`) alongside `dev` debuginfo,
        // whereas native `cargo test --profile debug` drops `[profile.test]`.
        let slot_test = fixture.0.join("slot-test/build");
        std::fs::create_dir_all(&slot_test).expect("test slot");
        let mut debug_test = clean_cargo_for("1.100.0", &app_custom);
        debug_test.args(["test", "-v"]);
        apply_storage(&mut debug_test, &slot_test, &app_custom, Profile::Debug);
        let test_out = debug_test.output().expect("oxide debug test");
        let test_stderr = String::from_utf8_lossy(&test_out.stderr);
        assert!(test_out.status.success(), "{test_stderr}");
        assert!(rustc_line(&test_stderr, "app").contains("-C opt-level=1"));
        assert!(rustc_line(&test_stderr, "app").contains("-C debuginfo=2"));
        assert!(rustc_line(&test_stderr, "build_script_build").contains("-C debuginfo=2"));

        let native_test = clean_cargo_for("1.100.0", &app_custom)
            .args(["test", "--profile", "debug", "-v"])
            .output()
            .expect("cargo test --profile debug");
        let native_test_stderr = String::from_utf8_lossy(&native_test.stderr);
        assert!(native_test.status.success(), "{native_test_stderr}");
        assert!(
            !rustc_line(&native_test_stderr, "app").contains("opt-level=1"),
            "native test --profile debug bypassed [profile.test]:\n{native_test_stderr}"
        );

        // 4. `Profile::Quick` produces `line-tables-only` for library/binary
        // units, omits build-script debuginfo, and outputs to `target/debug/`.
        std::fs::remove_dir_all(app_custom.join("target")).expect("clean target");
        let slot_quick = fixture.0.join("slot-quick/build");
        std::fs::create_dir_all(&slot_quick).expect("quick slot");
        let mut quick_build = clean_cargo_for("1.100.0", &app_custom);
        quick_build.args(["build", "-v"]);
        apply_storage(&mut quick_build, &slot_quick, &app_custom, Profile::Quick);
        let quick_out = quick_build.output().expect("oxide quick build");
        let quick_stderr = String::from_utf8_lossy(&quick_out.stderr);
        assert!(quick_out.status.success(), "{quick_stderr}");
        assert!(app_custom.join("target/debug/app").is_file());
        assert!(!rustc_line(&quick_stderr, "build_script_build").contains("debuginfo="));
        assert!(rustc_line(&quick_stderr, "dep").contains("-C debuginfo=line-tables-only"));
        assert!(rustc_line(&quick_stderr, "app").contains("-C debuginfo=line-tables-only"));
    }

    #[test]
    fn cargo_1_97_1_rejects_profile_debug_and_keeps_dev_recipe() {
        const VERSION_PREFIX: &str = "cargo 1.97.1 (c980f4866 ";
        if !toolchain_installed_matching("1.97.1", VERSION_PREFIX) {
            eprintln!("skipped: Cargo 1.97.1 is not installed");
            return;
        }
        let fixture = crate::cargo_layout::support::Fixture::new("profile-1-97-1");
        let (app, slot_build) = create_profile_probe_workspace(&fixture.0, "");

        let rejected = clean_cargo_for("1.97.1", &app)
            .args(["build", "--profile", "debug"])
            .output()
            .expect("cargo 1.97.1 --profile debug");
        assert!(!rejected.status.success());
        let rejected_stderr = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            rejected_stderr.contains("profile name `debug` is reserved"),
            "{rejected_stderr}"
        );

        let mut debug_build = clean_cargo_for("1.97.1", &app);
        debug_build.args(["build", "-v"]);
        apply_storage(&mut debug_build, &slot_build, &app, Profile::Debug);
        let debug_out = debug_build.output().expect("1.97.1 oxide debug build");
        let debug_stderr = String::from_utf8_lossy(&debug_out.stderr);
        assert!(debug_out.status.success(), "{debug_stderr}");
        assert!(app.join("target/debug/app").is_file());
        for unit in ["build_script_build", "dep", "app"] {
            assert!(
                rustc_line(&debug_stderr, unit).contains("-C debuginfo=2"),
                "expected full debuginfo for {unit} on 1.97.1:\n{debug_stderr}"
            );
        }

        let slot_quick = fixture.0.join("slot-quick/build");
        std::fs::create_dir_all(&slot_quick).expect("quick slot");
        let mut quick_build = clean_cargo_for("1.97.1", &app);
        quick_build.args(["build", "-v"]);
        apply_storage(&mut quick_build, &slot_quick, &app, Profile::Quick);
        let quick_out = quick_build.output().expect("1.97.1 oxide quick build");
        let quick_stderr = String::from_utf8_lossy(&quick_out.stderr);
        assert!(quick_out.status.success(), "{quick_stderr}");
        assert!(app.join("target/debug/app").is_file());
        assert!(!rustc_line(&quick_stderr, "build_script_build").contains("debuginfo="));
        assert!(rustc_line(&quick_stderr, "dep").contains("-C debuginfo=line-tables-only"));
        assert!(rustc_line(&quick_stderr, "app").contains("-C debuginfo=line-tables-only"));
    }
}
