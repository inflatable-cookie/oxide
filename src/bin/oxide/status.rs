//! Read-only `oxide status` reporting (recorded state and bounded `--measure`).

use crate::store::{self, Backend, Store};
use crate::{
    cargo_home, cargo_layout, child_temp, cold_seed_active, cold_seed_qualification_notice, config,
    freshness, identity, launch, outputs, process, prune, removable, scratch_io, seed_summary,
    watchdog, wrapper, CARGO, MIB,
};
use oxide_storage::lease::{AccountingCoverage, PoolStatus, SlotOccupancy};
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

pub(crate) fn run(args: &[OsString]) -> Result<u8, String> {
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
    let scratch_status = scratch_io::evaluate(&settings, true);
    print_settings(&settings, &scratch_status);
    println!(
        "{}",
        wrapper::status_line(wrapper::Policy::from_env(), &wrapper::inherited(&[]))
    );
    let mut budget = MeasureBudget::new();
    let mut control = None;
    match scratch_status
        .as_ref()
        .map_err(|e| e.clone())
        .and_then(|_| Store::open_recorded(&settings))
    {
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
                for line in prune::status_lines(internal.control()) {
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
    match oxide_storage::lease::admission_cost::recorded(&store.pool) {
        Ok(Some(line)) => println!("  {line}"),
        Ok(None) => println!("  recent admission costs: none recorded"),
        Err(error) => println!("  recent admission costs: unavailable ({error})"),
    }
    match oxide_storage::lease::managed::pinned_roots(&store.pool) {
        Ok(line) => println!("  {line}"),
        Err(error) => println!("  pinned roots by reason: unavailable ({error})"),
    }
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
                        "{} logical across {} of {} slots{}; used for admission",
                        cargo_home::size(bytes),
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
                "  slot sizes: recorded observations; use `oxide status --measure` for a fresh walk"
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
    format!(
        "  slot {} {state} {} project {} {owner} {}, {place}",
        slot.slot_id,
        slot.profile.as_deref().unwrap_or("-"),
        slot.label.as_deref().unwrap_or("unknown"),
        slot.size.describe()
    )
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

/// Plain Cargo leaves intermediates in the checkout's `target/`: `deps/` and
/// `incremental/` under layout v1, and `build/` and `incremental/` under layout
/// v2 (Cargo 1.100.0). Oxide keeps them in its slot. Finding none means none
/// only for a verified Cargo release, whose layout was checked.
fn report_stray(workspace: &Path, remaining_ms: u64) {
    let target = workspace.join("target");
    if !target.is_dir() {
        println!("stray Cargo output: none ({} absent)", target.display());
        return;
    }
    match cargo_layout::scan_stray(&target, remaining_ms) {
        Ok(stray) => {
            let coverage = if stray.complete {
                ""
            } else {
                " (partial scan)"
            };
            if stray.build_files > 0 {
                let files = stray.files + stray.build_files;
                let bytes = (stray.bytes + stray.build_bytes) / MIB;
                println!(
                    "stray Cargo output: {} holds plain-Cargo deps/, incremental/ or layout v2 build/ ({files} files, {bytes} MiB){coverage}; `oxide clean` removes it",
                    target.display()
                );
            } else if stray.files > 0 {
                println!(
                    "stray Cargo output: {} holds plain-Cargo deps/ or incremental/ ({} files, {} MiB){coverage}; `oxide clean` removes it",
                    target.display(),
                    stray.files,
                    stray.bytes / MIB
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
fn print_settings(settings: &config::Settings, scratch: &Result<Option<String>, String>) {
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
    match scratch {
        Ok(Some(uuid)) => println!(
            "pool: {} (scratch pool, volume {})",
            settings.pool.display(),
            uuid
        ),
        Err(error) => println!("pool: {} unavailable ({})", settings.pool.display(), error),
        Ok(None) => println!(
            "pool: {} (from {})",
            settings.pool.display(),
            settings.pool_source.describe()
        ),
    }
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
        }
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
    fn warm_reuse_sizes_and_historical_observations_are_honest() {
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
}
