//! The registry and private accounts follow the external outputs area.

use super::{read_registry_with_bytes, registry_dir, RootKind, MAX_REGISTRY_BYTES, REGISTRY_FILE};
use crate::outputs::FileEdit;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) fn output_accounts(control: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = control.join("output-storage");
    if crate::store::open_control_directory(&directory)?.is_none() {
        return Ok(Vec::new());
    }
    let mut accounts = Vec::new();
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate output accounts: {error}"))?;
        if entry.file_name().to_str().is_some_and(super::hex64) {
            let path = entry.path();
            crate::store::open_control_directory(&path)?
                .ok_or_else(|| format!("output account {} disappeared", path.display()))?;
            accounts.push(path);
        }
    }
    Ok(accounts)
}

pub(crate) fn plan_registry_relocation(
    control: &Path,
    old: &Path,
    new: &Path,
    uuid: &str,
) -> Result<Option<FileEdit>, String> {
    let Some((mut registry, before)) = read_registry_with_bytes(control)? else {
        return Ok(None);
    };
    let mut changed = false;
    for row in &mut registry.rows {
        if row.root_kind != RootKind::External
            || !row
                .volume_uuid
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(uuid))
        {
            continue;
        }
        let Ok(tail) = row.root.strip_prefix(old) else {
            continue;
        };
        if tail.as_os_str().is_empty()
            || !tail
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "output registry names an invalid root {}",
                row.root.display()
            ));
        }
        if row.cancelling.is_some() {
            return Err(format!(
                "finish cancelling the output reservation for {}",
                row.checkout.display()
            ));
        }
        row.root = new.join(tail);
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    registry.bump()?;
    registry.validate()?;
    let after = serde_json::to_vec(&registry).map_err(|error| error.to_string())?;
    if after.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("the relocated output registry exceeds its byte limit".to_owned());
    }
    Ok(Some(FileEdit::new(
        &registry_dir(control),
        REGISTRY_FILE.to_owned(),
        before,
        after,
    )))
}
