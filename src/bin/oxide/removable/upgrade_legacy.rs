//! Quiet-window migration for the legacy entries that block pool relocation.

mod directories;
mod slots;

use super::relocation::{checked_area, unheld_listing, Media};
use super::{positively_missing, read_registration, Registration, POOL_DIR, SLOTS_FOLDER};
use crate::prune::{self, LegacyOutputAction};
use crate::store::{self, Store};
use barge::{Area, Listing};
use directories::{at, Directory};
use oxide_storage::lease::{managed::PayloadRelocation, RecordedVolume};
use oxide_storage::storage::OUTPUTS_AREA;
use serde::Serialize;
use slots::SlotPlan;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const USAGE: &str = "usage: oxide pool upgrade-legacy [--dry-run] [--json]";

#[derive(Debug, Serialize)]
pub(super) struct Report {
    pub(super) dry_run: bool,
    pub(super) adopt_outputs: Paths,
    pub(super) remove_dead_outputs: Paths,
    pub(super) forget_missing_outputs: Paths,
    pub(super) remove_empty_projects: Paths,
    pub(super) remove_legacy_areas: Paths,
    pub(super) retire_dead_slots: Paths,
    pub(super) remove_markerless_trash: Paths,
}

#[derive(Debug, Serialize)]
pub(super) struct Paths {
    pub(super) count: usize,
    pub(super) paths: Vec<PathBuf>,
}

impl Paths {
    fn of<'a>(paths: impl Iterator<Item = &'a Path>) -> Self {
        let paths: Vec<_> = paths.map(Path::to_owned).collect();
        Self {
            count: paths.len(),
            paths,
        }
    }
}

pub(super) fn run(args: &[&str]) -> Result<u8, String> {
    if matches!(args, ["--help" | "-h"]) {
        println!("{USAGE}");
        return Ok(0);
    }
    let mut dry_run = false;
    let mut json = false;
    for &arg in args {
        match arg {
            "--dry-run" if !dry_run => dry_run = true,
            "--json" if !json => json = true,
            _ => return Err(USAGE.to_owned()),
        }
    }
    let internal = Store::open_recorded(&store::settings()?)?.ok_or("no internal pool exists")?;
    let report = upgrade(&internal, dry_run, Media::bind)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        let action = if dry_run { "Would" } else { "Did" };
        for (label, paths) in [
            ("adopt legacy outputs", &report.adopt_outputs),
            ("remove dead outputs", &report.remove_dead_outputs),
            ("forget missing outputs", &report.forget_missing_outputs),
            ("remove empty slot projects", &report.remove_empty_projects),
            ("remove legacy slot areas", &report.remove_legacy_areas),
            ("retire dead slots", &report.retire_dead_slots),
            (
                "remove markerless slot trash",
                &report.remove_markerless_trash,
            ),
        ] {
            println!("{action} {label} ({})", paths.count);
            for path in &paths.paths {
                println!("  {}", path.display());
            }
        }
    }
    Ok(0)
}

pub(super) fn upgrade(
    internal: &Store,
    dry_run: bool,
    bind: impl FnOnce(&Registration) -> Result<Media, String>,
) -> Result<Report, String> {
    let control = internal.control();
    // Maintenance only uses established locks. Taking an ordinary admission
    // lock could create a lock file even on a dry run or refused plan.
    let _internal_admission = store::lock_existing_admission(control)?;
    let old = read_registration(control)?.ok_or("no removable pool is registered")?;
    let media = bind(&old)?;
    media.verify()?;
    let removable = Store::open_recorded_removable(&control.join(POOL_DIR), internal.max_active())?;
    let _removable_admission = store::lock_existing_admission(removable.control())?;
    for pool in [internal, &removable] {
        pool.require_quiet_for_relocation()?;
    }
    let mut ledger = PayloadRelocation::begin(
        &removable.pool,
        &RecordedVolume {
            mount_point: old.mount_point.clone(),
            volume_uuid: old.volume_uuid.clone(),
            payload_dir: old.payload_dir.clone(),
        },
    )
    .map_err(|error| format!("cannot lock the removable ledger: {error}"))?;

    let slot_folder = format!("{}/{SLOTS_FOLDER}", old.payload_dir());
    let slots = Directory::new(
        old.mount_point.join(&slot_folder),
        media.directory(&slot_folder)?,
    )?;
    let slots_plan = SlotPlan::plan(&media, &removable, &old, &slots, &ledger)?;

    let outputs_path = old.mount_point.join(old.outputs_dir());
    let output_account = prune::output_account(control, &outputs_path);
    // A hold can outlive its Oxide record, including in the fallback pool
    // or in a checkout-local outputs account.
    for area in media
        .account(control, true)?
        .areas()
        .map_err(|error| error.to_string())?
    {
        unheld_listing(&area)?;
    }
    for account in prune::output_accounts(control)?
        .into_iter()
        .filter(|account| account != &output_account)
    {
        for area in media
            .account(&account, true)?
            .areas()
            .map_err(|error| error.to_string())?
        {
            unheld_listing(&area)?;
        }
    }
    let output_area = checked_area(
        &media.account(&output_account, true)?,
        OUTPUTS_AREA,
        old.outputs_dir(),
        "outputs",
        &old,
    )?;
    let output_dir = if !positively_missing(&outputs_path)? {
        Some(Directory::new(
            outputs_path.clone(),
            media.directory(old.outputs_dir())?,
        )?)
    } else {
        None
    };
    let listing = match (&output_area, &output_dir) {
        (Some(area), Some(directory)) => {
            let listing = unheld_listing(area)?;
            check_inventory(directory, &listing)?;
            listing
        }
        (None, None) => Listing::default(),
        (None, Some(_)) => {
            return Err(at(
                &outputs_path,
                "the removable outputs area is undeclared",
            ))
        }
        (Some(_), None) => return Err(at(&outputs_path, "the removable outputs area is missing")),
    };
    let outputs_plan = if output_dir.is_some() {
        prune::plan_legacy_outputs(control, &outputs_path, &old.volume_uuid, &listing)?
    } else {
        Default::default()
    };
    let checkouts: Vec<_> = outputs_plan.checkouts.iter().cloned().collect();
    let _bindings = internal.lock_relocation_bindings(&checkouts, true)?;

    let mut identities = std::collections::BTreeMap::new();
    if let Some(directory) = &output_dir {
        for (path, action) in &outputs_plan.actions {
            if matches!(action, LegacyOutputAction::ForgetRemoved) {
                continue;
            }
            let child = directory.required_dir(leaf(path)?)?;
            if listing.stragglers.iter().any(|stray| stray.path == *path) {
                child.require_unmarked()?;
            }
            if matches!(action, LegacyOutputAction::Adopt { .. }) {
                adoption_name(path)?;
            }
            if matches!(action, LegacyOutputAction::Remove) {
                child.inspect_tree()?;
            }
            identities.insert(path, child.identity());
        }
    }
    let output_paths = |matches: fn(&LegacyOutputAction) -> bool| {
        Paths::of(
            outputs_plan
                .actions
                .iter()
                .filter(|(_, action)| matches(action))
                .map(|(path, _)| path.as_path()),
        )
    };
    let report = Report {
        dry_run,
        adopt_outputs: output_paths(|action| matches!(action, LegacyOutputAction::Adopt { .. })),
        remove_dead_outputs: output_paths(|action| matches!(action, LegacyOutputAction::Remove)),
        forget_missing_outputs: output_paths(|action| {
            matches!(action, LegacyOutputAction::ForgetRemoved)
        }),
        remove_empty_projects: Paths::of(slots_plan.empty_projects()),
        remove_legacy_areas: Paths::of(slots_plan.subareas()),
        retire_dead_slots: Paths::of(slots_plan.dead_slots()),
        remove_markerless_trash: Paths::of(slots_plan.trash()),
    };
    if dry_run {
        return Ok(report);
    }

    if let (Some(directory), Some(registry)) = (&output_dir, &outputs_plan.registry) {
        store::publish_writer_generation(control)?;
        let area = checked_area(
            &media.account(&output_account, false)?,
            OUTPUTS_AREA,
            old.outputs_dir(),
            "outputs",
            &old,
        )?
        .ok_or("the removable outputs area disappeared")?;
        for (path, action) in &outputs_plan.actions {
            media.verify()?;
            directory.verify_path()?;
            let name = leaf(path)?;
            match action {
                LegacyOutputAction::Adopt { id, pin } => {
                    let child = directory.required_dir(name)?;
                    if child.identity() != identities[path] {
                        return Err(at(path, "output root changed since planning"));
                    }
                    adopt(&area, &child, id, *pin)?;
                }
                LegacyOutputAction::Remove => {
                    outputs_plan.require_dead(path)?;
                    directory.remove_tree(name, identities[path])?;
                }
                LegacyOutputAction::ForgetRemoved => {
                    outputs_plan.require_dead(path)?;
                    if directory.entry(name)?.is_some() {
                        return Err(at(path, "legacy output root reappeared"));
                    }
                }
            }
        }
        directory.sync()?;
        registry.write(false)?;
    }
    if slots_plan.empty_projects().next().is_some()
        || slots_plan.subareas().next().is_some()
        || slots_plan.trash().next().is_some()
    {
        store::publish_writer_generation(removable.control())?;
        slots_plan.apply(&media, &removable, &slots, &mut ledger)?;
    }
    Ok(report)
}

fn leaf(path: &Path) -> Result<&OsStr, String> {
    path.file_name()
        .ok_or_else(|| at(path, "missing directory name"))
}

fn adopt(area: &Area, child: &Directory, id: &str, pin: bool) -> Result<(), String> {
    match area.resolve(id) {
        Ok(root) if root.path == child.path => {}
        Ok(_) => return Err(at(&child.path, "the Barge output ID names another path")),
        Err(error) if error.kind() == barge::ErrorKind::NotFound => {
            child.require_unmarked()?;
            area.adopt_root(adoption_name(&child.path)?, "outputs", id)
                .map_err(|error| at(&child.path, error))?;
        }
        Err(error) => return Err(at(&child.path, error)),
    }
    if pin {
        area.pin(id, "selected by an Oxide output receipt")
            .map_err(|error| at(&child.path, error))?;
    }
    child.sync()
}

fn adoption_name(path: &Path) -> Result<&str, String> {
    leaf(path)?
        .to_str()
        .filter(|name| !name.contains('\\') && !name.chars().any(char::is_control))
        .ok_or_else(|| at(path, "invalid Barge output folder name"))
}

/// Barge lists directories and skips its own metadata. A symlink or file
/// cannot stand in for an expected directory, even when Barge ignored it.
fn check_inventory(directory: &Directory, listing: &Listing) -> Result<(), String> {
    for name in directory.names()? {
        let path = directory.path.join(&name);
        directory.required_dir(&name)?;
        if name != ".barge"
            && name != ".trash"
            && !listing.roots.iter().any(|root| root.path == path)
            && !listing.stragglers.iter().any(|stray| stray.path == path)
        {
            return Err(at(&path, "entry is outside the Barge inventory"));
        }
    }
    check_barge_layout(directory)?;
    for path in listing
        .roots
        .iter()
        .map(|root| &root.path)
        .chain(listing.stragglers.iter().map(|stray| &stray.path))
    {
        if path.parent() != Some(directory.path.as_path()) {
            return Err(at(path, "entry is outside the bound volume area"));
        }
    }
    Ok(())
}

fn check_barge_layout(directory: &Directory) -> Result<(), String> {
    if let Some(metadata) = directory.directory(OsStr::new(".barge"))? {
        for name in ["locks", "pins", "activity"] {
            metadata.directory(OsStr::new(name))?;
        }
        metadata.lock_file(OsStr::new("area.lock"))?;
    }
    Ok(())
}
