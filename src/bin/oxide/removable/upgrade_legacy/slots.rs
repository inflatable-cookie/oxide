//! Retire recorded dead legacy slots and dismantle the resulting empty areas.

use super::super::relocation::{check_area_location, checked_area, unheld_listing, Media};
use super::super::{legacy_workspace_missing, Registration};
use super::check_inventory;
use super::directories::{at, Directory};
use crate::store::Store;
use barge::{Area, Listing};
use oxide_storage::lease::managed::{PayloadRelocation, QuietSlot};
use oxide_storage::storage::{project_area_name, NAMESPACE, POOL_AREA, ROOT_MARKER};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::path::{Path, PathBuf};

type Identity = (u64, u64);

pub(super) struct SlotPlan {
    projects: Vec<Project>,
    trash: Vec<Trash>,
    records: Vec<LockedRecord>,
    _account_lock: Option<File>,
}

struct Project {
    name: String,
    path: PathBuf,
    identity: Option<Identity>,
    subarea: bool,
    declaration: Option<(String, Identity)>,
    slots: Vec<DeadSlot>,
}

struct DeadSlot {
    id: u64,
    name: String,
    path: PathBuf,
    identity: Option<Identity>,
    root: Option<String>,
    workspace: PathBuf,
}

struct Trash {
    name: std::ffi::OsString,
    path: PathBuf,
    identity: Identity,
    _lock: File,
}

struct LockedRecord {
    id: u64,
    last_slot: u64,
    _lock: Option<File>,
}

impl SlotPlan {
    pub(super) fn plan(
        media: &Media,
        store: &Store,
        registration: &Registration,
        slots: &Directory,
        ledger: &PayloadRelocation,
    ) -> Result<Self, String> {
        let account_dir = Directory::new(
            store.control().to_owned(),
            crate::store::open_control_directory(store.control())?
                .ok_or("removable pool control disappeared")?,
        )?
        .required_dir(OsStr::new("barge"))?;
        let account_lock = account_dir.lock_file(OsStr::new("lock"))?;
        let declarations = account_dir.required_dir(OsStr::new("areas"))?;
        let barge = media.account(store.control(), true)?;
        let folder = format!("{}/slots", registration.payload_dir());
        let main = checked_area(&barge, POOL_AREA, &folder, "pool", registration)?
            .ok_or("the removable slot area has not been declared")?;
        let listing = unheld_listing(&main)?;
        check_inventory(slots, &listing)?;

        let mut candidates = BTreeMap::<String, Option<Area>>::new();
        for stray in &listing.stragglers {
            let name = folder_name(&stray.path, &slots.path)?;
            candidates.insert(name.to_owned(), None);
        }
        for area in barge.areas().map_err(|error| error.to_string())? {
            let key = area.key();
            if key.namespace() != NAMESPACE || !key.name().starts_with("pool-") {
                continue;
            }
            let legacy_folder = area.record().effective_folder();
            let name = legacy_folder
                .strip_prefix(&folder)
                .and_then(|suffix| suffix.strip_prefix('/'))
                .filter(|suffix| !suffix.is_empty() && !suffix.contains('/'))
                .ok_or_else(|| {
                    format!(
                        "legacy area {} names an unexpected folder {legacy_folder}",
                        key.label()
                    )
                })?;
            if key.name() != project_area_name(POOL_AREA, name) {
                return Err(format!(
                    "legacy area {} does not match folder {legacy_folder}",
                    key.label()
                ));
            }
            check_area_location(&area, &legacy_folder, "pool", registration)?;
            candidates.insert(name.to_owned(), Some(area));
        }
        let mut by_project = BTreeMap::<&str, Vec<&QuietSlot<'_>>>::new();
        let slot_rows: Vec<_> = ledger.slots().collect();
        for slot in &slot_rows {
            if let Some((project, _)) = slot.folder.split_once('/') {
                candidates.entry(project.to_owned()).or_insert(None);
                by_project.entry(project).or_default().push(slot);
            }
        }
        let mut projects = Vec::new();
        for (name, area) in candidates {
            let path = slots.path.join(&name);
            if listing.roots.iter().any(|root| root.path == path)
                || slot_rows.iter().any(|slot| slot.folder == name)
            {
                return Err(at(
                    &path,
                    "referenced as a flat slot; cannot remove it as a legacy project",
                ));
            }
            let directory = slots.directory(OsStr::new(&name))?;
            let project_slots = by_project.get(name.as_str()).map_or(&[][..], Vec::as_slice);
            let declaration = area
                .as_ref()
                .map(|area| {
                    let key = area.key();
                    let filename = format!("{}.{}.json", key.namespace(), key.name());
                    let entry = declarations
                        .entry(OsStr::new(&filename))?
                        .filter(|entry| entry.regular)
                        .ok_or_else(|| {
                            at(
                                &declarations.path.join(&filename),
                                "expected a regular legacy declaration",
                            )
                        })?;
                    if account_lock.is_none() {
                        return Err(at(&account_dir.path, "the Barge account lock is missing"));
                    }
                    Ok((filename, (entry.dev, entry.ino)))
                })
                .transpose()?;
            let dead = plan_project(directory.as_ref(), &path, area.as_ref(), project_slots)?;
            if directory.is_some() || declaration.is_some() || !dead.is_empty() {
                let subarea = declaration.is_some()
                    || !project_slots.is_empty()
                    || directory
                        .as_ref()
                        .map(|dir| dir.entry(OsStr::new(".barge")))
                        .transpose()?
                        .flatten()
                        .is_some();
                projects.push(Project {
                    name,
                    path,
                    identity: directory.as_ref().map(Directory::identity),
                    subarea,
                    declaration,
                    slots: dead,
                });
            }
        }

        let retiring: BTreeSet<_> = projects
            .iter()
            .flat_map(|project| project.slots.iter().map(|slot| slot.id))
            .collect();
        ledger
            .check_legacy_retirement(&retiring)
            .map_err(|error| error.to_string())?;
        let mut records = Vec::new();
        let mut seen = BTreeSet::new();
        for slot in slot_rows.iter().filter(|slot| retiring.contains(&slot.id)) {
            if !seen.insert(slot.project) {
                continue;
            }
            let matching: Vec<_> = slot_rows
                .iter()
                .filter(|row| row.project == slot.project)
                .collect();
            if let Some((id, lock)) = store.lock_legacy_slot_record(slot.project, slot.workspace)? {
                if matching.iter().all(|row| retiring.contains(&row.id)) {
                    // Remove the store affinity at the last removal using it.
                    let last = projects
                        .iter()
                        .flat_map(|project| &project.slots)
                        .filter(|dead| matching.iter().any(|row| row.id == dead.id))
                        .next_back()
                        .ok_or("no planned legacy slot for the store affinity")?;
                    records.push(LockedRecord {
                        id,
                        last_slot: last.id,
                        _lock: lock,
                    });
                }
            }
        }
        let trash = plan_trash(slots)?;
        Ok(Self {
            projects,
            trash,
            records,
            _account_lock: account_lock,
        })
    }

    pub(super) fn empty_projects(&self) -> impl Iterator<Item = &Path> {
        self.projects
            .iter()
            .filter(|item| !item.subarea)
            .map(|item| item.path.as_path())
    }

    pub(super) fn subareas(&self) -> impl Iterator<Item = &Path> {
        self.projects
            .iter()
            .filter(|item| item.subarea)
            .map(|item| item.path.as_path())
    }

    pub(super) fn dead_slots(&self) -> impl Iterator<Item = &Path> {
        self.projects
            .iter()
            .flat_map(|item| item.slots.iter().map(|slot| slot.path.as_path()))
    }

    pub(super) fn trash(&self) -> impl Iterator<Item = &Path> {
        self.trash.iter().map(|item| item.path.as_path())
    }

    pub(super) fn apply(
        &self,
        media: &Media,
        store: &Store,
        slots: &Directory,
        ledger: &mut PayloadRelocation,
    ) -> Result<(), String> {
        let account = media.account(store.control(), false)?;
        let state = Directory::new(
            store.control().to_owned(),
            crate::store::open_control_directory(store.control())?
                .ok_or("removable pool control disappeared")?,
        )?
        .required_dir(OsStr::new("barge"))?
        .required_dir(OsStr::new("areas"))?;
        for project in &self.projects {
            media.verify()?;
            slots.verify_path()?;
            let dir = slots.directory(OsStr::new(&project.name))?;
            if dir.as_ref().map(Directory::identity) != project.identity {
                return Err(at(&project.path, "legacy project changed since planning"));
            }
            for slot in &project.slots {
                if !legacy_workspace_missing(&slot.workspace)? {
                    return Err(at(&slot.workspace, "legacy slot workspace reappeared"));
                }
                if let Some(dir) = &dir {
                    slot.remove(dir, &account, &project.name)?;
                }
                if let Some(record) = self
                    .records
                    .iter()
                    .find(|record| record.last_slot == slot.id)
                {
                    store.forget_legacy_slot_record(record.id)?;
                }
                ledger
                    .forget_legacy_slot(slot.id)
                    .map_err(|error| at(&slot.path, error))?;
            }
            if let Some(dir) = dir {
                dir.require_unmarked()?;
                finish_project(&dir)?;
                slots.remove_empty(OsStr::new(&project.name), dir.identity())?;
                slots.sync()?;
            }
            if let Some((name, expected)) = &project.declaration {
                let name = OsStr::new(name);
                if !state
                    .entry(name)?
                    .is_some_and(|entry| entry.regular && (entry.dev, entry.ino) == *expected)
                {
                    return Err(at(
                        &state.path.join(name),
                        "legacy declaration changed since planning",
                    ));
                }
                state.unlink(name, false)?;
                state.sync()?;
            }
        }
        if !self.trash.is_empty() {
            media.verify()?;
            slots.verify_path()?;
            let trash = slots.required_dir(OsStr::new(".trash"))?;
            let _locks = slot_hold_locks(slots)?;
            for item in &self.trash {
                trash.remove_tree(&item.name, item.identity)?;
            }
            trash.sync()?;
        }
        Ok(())
    }
}

impl DeadSlot {
    fn remove(
        &self,
        project: &Directory,
        account: &barge::Barge,
        name: &str,
    ) -> Result<(), String> {
        let leaf = OsStr::new(&self.name);
        let current = project.directory(leaf)?;
        if current.as_ref().map(Directory::identity) != self.identity {
            return Err(at(&self.path, "legacy slot changed since planning"));
        }
        let Some(current) = current else {
            return Ok(());
        };
        if let Some(id) = &self.root {
            project.verify_path()?;
            let area = account
                .find_area(&format!(
                    "{NAMESPACE}/{}",
                    project_area_name(POOL_AREA, name)
                ))
                .map_err(|error| at(&self.path, error))?;
            let root = area.resolve(id).map_err(|error| at(&self.path, error))?;
            if root.path != self.path {
                return Err(at(&self.path, "Barge slot identity moved"));
            }
            let outcome = area.evict(id).map_err(|error| at(&self.path, error))?;
            if !outcome.deleted {
                return Err(at(
                    &self.path,
                    outcome
                        .kept_reason
                        .as_deref()
                        .unwrap_or("Barge left the slot in trash"),
                ));
            }
        } else {
            let _locks = slot_hold_locks(project)?;
            project.remove_tree(leaf, current.identity())?;
        }
        project.sync()
    }
}

fn folder_name<'a>(path: &'a Path, parent: &Path) -> Result<&'a str, String> {
    if path.parent() == Some(parent) {
        if let Some(leaf) = path.file_name().and_then(OsStr::to_str) {
            return Ok(leaf);
        }
    }
    Err(at(path, "not a named directory inside the bound area"))
}

fn plan_project(
    directory: Option<&Directory>,
    path: &Path,
    area: Option<&Area>,
    rows: &[&QuietSlot<'_>],
) -> Result<Vec<DeadSlot>, String> {
    let listing = area
        .map(unheld_listing)
        .transpose()?
        .unwrap_or_else(Listing::default);
    if let Some(project) = directory {
        project.require_unmarked()?;
        let _locks = slot_hold_locks(project)?;
        check_project_metadata(project)?;
        for name in project.names()? {
            if name == ".barge" || name == ".trash" {
                continue;
            }
            if !rows.iter().any(|row| {
                row.folder
                    .rsplit_once('/')
                    .is_some_and(|(_, leaf)| name == leaf)
            }) {
                return Err(at(
                    &project.path.join(&name),
                    "unowned entry in a legacy slot project",
                ));
            }
        }
    }
    let mut dead = Vec::new();
    let mut names = BTreeSet::new();
    for slot in rows {
        let (_, name) = slot
            .folder
            .rsplit_once('/')
            .ok_or("not a nested ledger slot")?;
        let child_path = path.join(name);
        if !names.insert(name) {
            return Err(at(
                &child_path,
                "more than one ledger slot names this directory",
            ));
        }
        if !legacy_workspace_missing(slot.workspace)? {
            return Err(at(
                &child_path,
                format!(
                    "legacy slot workspace still exists at {}",
                    slot.workspace.display()
                ),
            ));
        }
        let child = directory
            .map(|dir| dir.directory(OsStr::new(name)))
            .transpose()?
            .flatten();
        let root = listing.roots.iter().find(|root| root.path == child_path);
        if let Some(child) = &child {
            match root {
                Some(root) if slot.root == Some(root.id.as_str()) && !root.pinned => {}
                Some(_) => {
                    return Err(at(
                        &child_path,
                        "Barge root has a foreign identity or is pinned",
                    ))
                }
                None if slot.root.is_none() => child.require_unmarked()?,
                None => return Err(at(&child_path, "recorded Barge slot has lost its marker")),
            }
            child.inspect_tree()?;
        }
        dead.push(DeadSlot {
            id: slot.id,
            name: name.to_owned(),
            path: child_path,
            identity: child.as_ref().map(Directory::identity),
            root: root.map(|root| root.id.clone()),
            workspace: slot.workspace.to_owned(),
        });
    }
    Ok(dead)
}

/// Barge may ignore non-directories in its listing. Inspect the legacy area's
/// metadata explicitly before removing the area itself.
fn check_project_metadata(project: &Directory) -> Result<(), String> {
    if let Some(trash) = project.directory(OsStr::new(".trash"))? {
        if !trash.names()?.is_empty() {
            return Err(at(&trash.path, "legacy sub-area trash is not empty"));
        }
    }
    if let Some(metadata) = project.directory(OsStr::new(".barge"))? {
        for name in metadata.names()? {
            match name.to_str() {
                Some("area.lock") => {
                    metadata.lock_file(&name)?;
                }
                Some("locks" | "activity" | "pins") => {
                    let folder = metadata.required_dir(&name)?;
                    for entry in folder.names()? {
                        let extension = if name == "locks" { ".lock" } else { ".json" };
                        let known = name != "pins"
                            && entry.to_str().is_some_and(|file| {
                                file.strip_suffix(extension).is_some_and(barge_id)
                            });
                        if !known || !folder.entry(&entry)?.is_some_and(|entry| entry.regular) {
                            return Err(at(
                                &folder.path.join(&entry),
                                "unexpected Barge metadata entry",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(at(
                        &metadata.path.join(&name),
                        "unexpected Barge metadata entry",
                    ))
                }
            }
        }
    }
    Ok(())
}

fn finish_project(project: &Directory) -> Result<(), String> {
    check_project_metadata(project)?;
    let _locks = slot_hold_locks(project)?;
    if let Some(trash) = project.directory(OsStr::new(".trash"))? {
        project.remove_empty(OsStr::new(".trash"), trash.identity())?;
    }
    if let Some(metadata) = project.directory(OsStr::new(".barge"))? {
        let _lock = metadata.lock_file(OsStr::new("area.lock"))?;
        for name in ["locks", "activity", "pins"] {
            let name = OsStr::new(name);
            if let Some(folder) = metadata.directory(name)? {
                for file in folder.names()? {
                    folder.unlink(&file, false)?;
                }
                metadata.remove_empty(name, folder.identity())?;
            }
        }
        if _lock.is_some() {
            metadata.unlink(OsStr::new("area.lock"), false)?;
        }
        project.remove_empty(OsStr::new(".barge"), metadata.identity())?;
    }
    Ok(())
}

fn slot_hold_locks(parent: &Directory) -> Result<Vec<File>, String> {
    let Some(metadata) = parent.directory(OsStr::new(".barge"))? else {
        return Ok(Vec::new());
    };
    let Some(locks) = metadata.directory(OsStr::new("locks"))? else {
        return Ok(Vec::new());
    };
    let mut held = Vec::new();
    for name in locks.names()? {
        held.extend(locks.lock_file(&name)?);
    }
    Ok(held)
}

fn plan_trash(slots: &Directory) -> Result<Vec<Trash>, String> {
    let Some(trash) = slots.directory(OsStr::new(".trash"))? else {
        return Ok(Vec::new());
    };
    let mut planned = Vec::new();
    for name in trash.names()? {
        let directory = trash.required_dir(&name)?;
        if directory.entry(OsStr::new(ROOT_MARKER))?.is_some() {
            // Marked trash remains Barge's to sweep, including foreign markers.
            continue;
        }
        if !name.to_str().is_some_and(slot_trash_name) {
            return Err(at(
                &directory.path,
                "unrecognized markerless Oxide slot trash",
            ));
        }
        let lock = directory.lock()?;
        directory.inspect_tree()?;
        planned.push(Trash {
            name,
            path: directory.path.clone(),
            identity: directory.identity(),
            _lock: lock,
        });
    }
    if !planned.is_empty() {
        slot_hold_locks(slots)?;
    }
    Ok(planned)
}

fn barge_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Barge renames a slot to its old folder, with an optional numeric collision
/// suffix and, during sweep, a claim prefix. Oxide's generated flat folders
/// end in a 16-digit hex Barge ID, and old slot folders use a decimal slot ID.
fn slot_trash_name(name: &str) -> bool {
    let decimal = |id: &str| {
        id.parse::<u64>()
            .is_ok_and(|n| n > 0 && n.to_string() == id)
    };
    let name = name.strip_prefix(".barge-claim-").unwrap_or(name);
    let name = name
        .rsplit_once('.')
        .filter(|(_, suffix)| decimal(suffix))
        .map_or(name, |(name, _)| name);
    let valid = |folder: &str| {
        decimal(folder)
            || folder.rsplit_once('-').is_some_and(|(label, id)| {
                !label.is_empty()
                    && label.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                    })
                    && (decimal(id)
                        || (id.len() == 16
                            && id
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))))
            })
    };
    valid(name) && name.len() <= 80
}
