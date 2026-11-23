//! Prepare the receipt and link changes while the original roots still exist.

use super::receipt::{
    link_path, member_destination, read_marker, read_receipt_with, safe_absolute, sync_directory,
    LegacyReceiptReading, Receipt, RootKind, MAX_RECEIPT_BYTES,
};
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

pub(crate) struct FileEdit {
    directory: PathBuf,
    name: String,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl FileEdit {
    pub(crate) fn new(directory: &Path, name: String, before: Vec<u8>, after: Vec<u8>) -> Self {
        Self {
            directory: directory.to_owned(),
            name,
            before,
            after,
        }
    }

    pub(crate) fn write(&self, rollback: bool) -> Result<(), String> {
        let bytes = if rollback { &self.before } else { &self.after };
        crate::store::atomic_write_private(&self.directory, &self.name, bytes).map_err(|error| {
            format!(
                "cannot publish {}: {error}",
                self.directory.join(&self.name).display()
            )
        })
    }
}

pub(crate) struct LinkEdit {
    path: PathBuf,
    before: PathBuf,
    after: PathBuf,
}

impl LinkEdit {
    /// Replace only the exact link observed during planning. On rollback the
    /// old value also covers a forward step that failed before its rename.
    pub(crate) fn write(&self, rollback: bool) -> Result<(), String> {
        let (before, after) = if rollback {
            (&self.after, &self.before)
        } else {
            (&self.before, &self.after)
        };
        let parent = self.path.parent().ok_or("output link has no parent")?;
        let verify = || {
            if fs::canonicalize(parent).as_deref().ok() != Some(parent) {
                return Err(format!(
                    "output link parent changed at {}",
                    parent.display()
                ));
            }
            fs::read_link(&self.path).map_err(|error| {
                format!("cannot read output link {}: {error}", self.path.display())
            })
        };
        let observed = verify()?;
        if rollback && observed == *after {
            return Ok(());
        }
        if observed != *before {
            return Err(format!("output link changed at {}", self.path.display()));
        }
        let staged = parent.join(format!(".oxide-relocate-{}", crate::prune::fresh_nonce()?));
        symlink(after, &staged).map_err(|error| {
            format!("cannot stage output link {}: {error}", self.path.display())
        })?;
        let result = verify().and_then(|observed| {
            if observed != *before {
                return Err(format!("output link changed at {}", self.path.display()));
            }
            fs::rename(&staged, &self.path).map_err(|error| {
                format!(
                    "cannot replace output link {}: {error}",
                    self.path.display()
                )
            })
        });
        if result.is_err() && fs::canonicalize(parent).as_deref().ok() == Some(parent) {
            let _ = fs::remove_file(&staged);
        }
        result?;
        sync_directory(parent)
    }
}

#[derive(Default)]
pub(crate) struct ReceiptEdits {
    pub(crate) files: Vec<FileEdit>,
    pub(crate) links: Vec<LinkEdit>,
    pub(crate) skipped: Vec<PathBuf>,
    pub(crate) checkouts: Vec<PathBuf>,
}

pub(crate) fn plan_relocated_receipts(
    control: &Path,
    old: &Path,
    new: &Path,
    uuid: &str,
) -> Result<ReceiptEdits, String> {
    let directory = control.join("outputs");
    let mut edits = ReceiptEdits::default();
    if crate::store::open_control_directory(&directory)?.is_none() {
        return Ok(edits);
    }
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("cannot read receipts in {}: {error}", directory.display()))?;
    let mut remaining = crate::prune::SNAPSHOT_MAX_BYTES;
    // Receipts may remember an earlier root even when it has no live row.
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate receipts: {error}"))?;
        let name = entry.file_name();
        let Some((name, id)) = name.to_str().and_then(|name| {
            let id = name.strip_suffix(".json")?;
            (id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then_some((name, id))
        }) else {
            continue;
        };
        let path = entry.path();
        let Some(file) = crate::store::private_file(&path, false)? else {
            return Err(format!("receipt {} disappeared", path.display()));
        };
        let mut before = Vec::new();
        let limit = MAX_RECEIPT_BYTES.min(remaining);
        file.take(limit + 1)
            .read_to_end(&mut before)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if before.len() as u64 > limit {
            return Err(format!(
                "{} exceeds the relocation receipt byte budget",
                path.display()
            ));
        }
        remaining -= before.len() as u64;
        let stored: Receipt = serde_json::from_slice(&before)
            .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
        if !safe_absolute(&stored.checkout)
            || crate::identity::digest(&stored.checkout.to_string_lossy()) != id
        {
            return Err(format!("{} names an invalid checkout", path.display()));
        }
        let mut receipt =
            read_receipt_with(LegacyReceiptReading::Read, control, &stored.checkout, id)?
                .ok_or_else(|| format!("receipt {} disappeared", path.display()))?;
        edits.checkouts.push(stored.checkout);
        if relocate_receipt(&mut receipt, old, new, uuid, &mut edits)? {
            receipt.validate(&receipt.checkout, id)?;
            let after = serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?;
            if after.len() as u64 > MAX_RECEIPT_BYTES {
                return Err(format!(
                    "relocated receipt {} exceeds its byte limit",
                    path.display()
                ));
            }
            edits
                .files
                .push(FileEdit::new(&directory, name.to_owned(), before, after));
        }
    }
    edits.skipped.sort();
    edits.skipped.dedup();
    Ok(edits)
}

fn relocate_receipt(
    receipt: &mut Receipt,
    old: &Path,
    new: &Path,
    uuid: &str,
    edits: &mut ReceiptEdits,
) -> Result<bool, String> {
    let relocated = |kind, volume: Option<&str>, root: &Path| {
        let tail = root.strip_prefix(old).ok()?;
        (kind == RootKind::External
            && volume.is_some_and(|volume| volume.eq_ignore_ascii_case(uuid))
            && !tail.as_os_str().is_empty())
        .then(|| new.join(tail))
    };
    let mut changed = false;
    for placement in &mut receipt.links {
        let Some(selected) = &mut placement.selected else {
            continue;
        };
        let Some(root) = relocated(
            selected.kind,
            selected.volume_uuid.as_deref(),
            &selected.root,
        ) else {
            continue;
        };
        let path = link_path(&receipt.checkout, &placement.member);
        let before = member_destination(&selected.root, &placement.member);
        if owned_link(
            &path,
            &before,
            &selected.root,
            &receipt.checkout,
            &receipt.checkout_id,
        )? {
            edits.links.push(LinkEdit {
                path,
                before,
                after: member_destination(&root, &placement.member),
            });
        } else {
            edits.skipped.push(path);
        }
        selected.root = root;
        changed = true;
    }
    for root in &mut receipt.roots {
        if let Some(path) = relocated(root.kind, root.volume_uuid.as_deref(), &root.root) {
            root.root = path;
            changed = true;
        }
    }
    if let Some(reservation) = &mut receipt.reserved {
        if let Some(path) = relocated(
            reservation.kind,
            reservation.volume_uuid.as_deref(),
            &reservation.root,
        ) {
            reservation.root = path;
            changed = true;
        }
    }
    if receipt.prepared_ref().is_some_and(|prepared| {
        let to = &prepared.to;
        changed || relocated(to.kind, to.volume_uuid.as_deref(), &to.root).is_some()
    }) {
        return Err(format!(
            "finish the prepared output transition for {} before relocating",
            receipt.checkout.display()
        ));
    }
    Ok(changed)
}

fn owned_link(
    path: &Path,
    expected: &Path,
    root: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<bool, String> {
    let parent = path.parent().ok_or("output link has no parent")?;
    if fs::canonicalize(parent).as_deref().ok() != Some(parent) {
        return Ok(false);
    }
    match fs::read_link(path) {
        Ok(target) if target == expected => {
            if fs::canonicalize(root).as_deref().ok() != Some(root) {
                return Err(format!("output root changed at {}", root.display()));
            }
            let marker = read_marker(root)?;
            if !marker.is_owned_by(checkout, checkout_id) || marker.kind() != RootKind::External {
                return Err(format!("cannot establish ownership of {}", root.display()));
            }
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::InvalidInput) => {
            Ok(false)
        }
        Err(error) => Err(format!(
            "cannot read output link {}: {error}",
            path.display()
        )),
    }
}
