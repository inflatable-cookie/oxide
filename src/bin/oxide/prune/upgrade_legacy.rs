//! Match unmarked removable outputs against the existing registry.

use super::{
    output_id, read_registry_with_bytes, registry_dir, RootKind, Row, MAX_REGISTRY_BYTES,
    REGISTRY_FILE,
};
use crate::outputs::{self, FileEdit};
use barge::Listing;
use std::collections::{BTreeMap, BTreeSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(crate) enum LegacyOutputAction {
    Adopt { id: String, pin: bool },
    Remove,
    ForgetRemoved,
}

#[derive(Default)]
pub(crate) struct LegacyOutputs {
    pub(crate) actions: BTreeMap<PathBuf, LegacyOutputAction>,
    pub(crate) checkouts: BTreeSet<PathBuf>,
    pub(crate) registry: Option<FileEdit>,
    dead_rows: BTreeMap<PathBuf, Vec<Row>>,
}

impl LegacyOutputs {
    pub(crate) fn require_dead(&self, path: &Path) -> Result<(), String> {
        let rows = self
            .dead_rows
            .get(path)
            .ok_or("missing planned output rows")?;
        for row in rows {
            if checkout_exists(row)? {
                return Err(format!(
                    "checkout {} reappeared for {}",
                    row.checkout.display(),
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

/// The caller has checked the area's paths and holds the publication gate.
pub(crate) fn plan_legacy_outputs(
    control: &Path,
    parent: &Path,
    uuid: &str,
    listing: &Listing,
) -> Result<LegacyOutputs, String> {
    let mut plan = LegacyOutputs::default();
    crate::store::open_control_directory(&registry_dir(control))?;
    let Some((mut registry, before)) = read_registry_with_bytes(control)? else {
        if let Some(stray) = listing.stragglers.first() {
            return Err(format!(
                "{} has no output registry row",
                stray.path.display()
            ));
        }
        return Ok(plan);
    };
    let mut by_root = BTreeMap::<PathBuf, Vec<usize>>::new();
    for (index, row) in registry.rows.iter().enumerate() {
        if row.root.parent() == Some(parent) {
            by_root.entry(row.root.clone()).or_default().push(index);
        }
    }
    let strays: BTreeSet<_> = listing.stragglers.iter().map(|stray| &stray.path).collect();
    for path in &strays {
        if !by_root.contains_key(*path) {
            return Err(format!("{} has no output registry row", path.display()));
        }
    }

    let mut removed = BTreeSet::new();
    for (path, indexes) in by_root {
        let rows: Vec<_> = indexes.iter().map(|&i| &registry.rows[i]).collect();
        let is_stray = strays.contains(&path);
        let is_legacy = rows
            .iter()
            .all(|row| row.barge_id.is_none() && row.pruned_ms.is_none());
        if !is_stray && !is_legacy {
            continue;
        }
        if is_stray && !is_legacy {
            return Err(format!(
                "{} has a retired or recorded Barge identity",
                path.display()
            ));
        }
        let listed = listing.roots.iter().find(|root| root.path == path);
        let missing = if !is_stray && listed.is_none() {
            positively_missing(&path)?
        } else {
            false
        };
        if !is_stray && !missing && listed.is_none() {
            return Err(format!("{} is outside the Barge inventory", path.display()));
        }

        for row in &rows {
            validate_row(row, uuid)?;
        }
        let live = rows
            .iter()
            .map(|row| checkout_exists(row))
            .collect::<Result<Vec<_>, _>>()?;
        let choice = if live.iter().any(|&exists| exists) {
            if missing {
                continue;
            }
            let id = match adoption_id(&path, &rows) {
                Ok(id) => id,
                Err(_) if !is_stray => continue,
                Err(reason) => return Err(reason),
            };
            match listed {
                // An interrupted upgrade may have written the marker before
                // pinning the selection and publishing its registry identity.
                Some(root) if root.id != id => continue,
                None if listing.roots.iter().any(|root| root.id == id) => {
                    return Err(format!(
                        "{} has a conflicting Barge ID {id}",
                        path.display()
                    ));
                }
                _ => {}
            }
            let pin = rows
                .iter()
                .zip(&live)
                .any(|(row, &exists)| exists && row.live());
            for &index in &indexes {
                registry.rows[index].barge_id = Some(id.clone());
            }
            LegacyOutputAction::Adopt { id, pin }
        } else if is_stray || missing {
            plan.dead_rows.insert(
                path.clone(),
                rows.iter().map(|row| (*row).clone()).collect(),
            );
            removed.extend(indexes.iter().copied());
            if missing {
                LegacyOutputAction::ForgetRemoved
            } else {
                LegacyOutputAction::Remove
            }
        } else {
            continue;
        };
        plan.checkouts
            .extend(indexes.iter().map(|&i| registry.rows[i].checkout.clone()));
        plan.actions.insert(path, choice);
    }
    if plan.actions.is_empty() {
        return Ok(plan);
    }
    if !registry.complete {
        return Err(
            "the output registry is incomplete; legacy upgrade requires its complete rows"
                .to_owned(),
        );
    }
    registry.rows = registry
        .rows
        .into_iter()
        .enumerate()
        .filter_map(|(i, row)| (!removed.contains(&i)).then_some(row))
        .collect();
    registry.bump()?;
    registry.validate()?;
    let after = serde_json::to_vec(&registry).map_err(|error| error.to_string())?;
    if after.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("the upgraded output registry exceeds its byte limit".to_owned());
    }
    plan.registry = Some(FileEdit::new(
        &registry_dir(control),
        REGISTRY_FILE.to_owned(),
        before,
        after,
    ));
    Ok(plan)
}

fn validate_row(row: &Row, uuid: &str) -> Result<(), String> {
    if row.root_kind != RootKind::External
        || !row
            .volume_uuid
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case(uuid))
        || row.cancelling.is_some()
        || !row.checkout.components().all(|part| {
            matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    {
        return Err(format!(
            "{} has an incompatible legacy output registry row",
            row.root.display()
        ));
    }
    Ok(())
}

fn adoption_id(path: &Path, rows: &[&Row]) -> Result<String, String> {
    let marker = outputs::read_marker_for_prune(path)?;
    if !rows.iter().any(|row| {
        marker.is_owned_by(&row.checkout, &row.checkout_id)
            && marker.kind() == row.root_kind
            && marker.volume_uuid() == row.volume_uuid.as_deref()
            && row
                .nonce
                .as_deref()
                .is_none_or(|nonce| marker.nonce() == Some(nonce))
    }) {
        return Err(format!("{} has a foreign output identity", path.display()));
    }
    Ok(output_id(path, marker.generation()))
}

fn checkout_exists(row: &Row) -> Result<bool, String> {
    if !positively_missing(&row.checkout)? {
        return Ok(true);
    }
    // A recorded checkout drive being absent is insufficient evidence that
    // the checkout was deleted. Verify the closest surviving ancestor's UUID.
    if let Some(expected) = &row.checkout_volume_uuid {
        for parent in row.checkout.ancestors().skip(1) {
            if !positively_missing(parent)? {
                let current = super::checkout_volume_uuid(parent);
                if !current.is_some_and(|value| value.eq_ignore_ascii_case(expected)) {
                    return Err(format!(
                        "cannot establish the checkout volume at {}",
                        row.checkout.display()
                    ));
                }
                break;
            }
        }
    }
    Ok(false)
}

pub(crate) fn positively_missing(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}
