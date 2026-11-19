//! Idle eviction handed from the pool lock to Barge's trash (oxide#324).
//!
//! Barge's public eviction is one call: it rechecks the mark, the cooperative
//! hold and the pin, renames the root into the area's `.trash/` and deletes the
//! tree there. Deleting a large slot tree took seconds while the pool lock was
//! held, so every other admission waited behind it. The decision is therefore
//! committed here as a durable record under the pool lock, and Barge's eviction
//! (its identity, hold and pin checks included) runs later, when the pool and
//! front-door admission locks are released.
//!
//! The record is the commit point. Its root is no longer in the ledger, so no
//! admission can grant or reuse it, and a pass that never ran (a crash, a
//! failed deletion) is resumed by a later command. Recovery stays conservative:
//! a recorded root that is still present is left to Barge's own checks, and one
//! whose path is absent is only dropped because the rename already happened.

use super::{lock_pool, pool_backing, read_config, require_deleted, LeaseError, MANAGED_NAME};
use crate::lease::io_error;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const RECORDS_NAME: &str = "idle-evictions.json";
const RECORDS_VERSION: u32 = 1;
const RECORDS_MAX_BYTES: u64 = 128 * 1024;
/// One pool's bounded hand-off list. A full list makes the admission fall back
/// to the inline eviction rather than grow the file without bound; the
/// front door's own pass normally empties the list after every command.
pub(super) const RECORDS_MAX: usize = 32;

/// One idle slot whose eviction this pool already decided, with everything a
/// later pass needs to let Barge evict it and report the deletion.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PendingEviction {
    /// The pool this record belongs to; a record from another store is refused.
    pub store_id: String,
    pub slot_id: u64,
    pub incarnation: u64,
    /// The Barge root id the slot was recorded as.
    pub root_id: String,
    /// The slot's area folder for a legacy nested slot, `None` for the flat
    /// `oxide/pool` area.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area_folder: Option<String>,
    /// The root path at the decision. Recovery drops the record only when this
    /// path is absent, which means Barge's rename already committed.
    pub path: PathBuf,
    /// The generation's recorded logical bytes. Zero with `approximate` set
    /// means no current observation existed; Barge's own eviction figure is not
    /// used because it costs a sizing walk under the lock.
    pub bytes: u64,
    pub bytes_approximate: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Records {
    version: u32,
    store_id: String,
    records: Vec<PendingEviction>,
}

fn records_path(pool: &Path) -> PathBuf {
    pool.join(MANAGED_NAME).join(RECORDS_NAME)
}

/// Read this pool's hand-off list. A missing file is an empty list.
pub(super) fn read(pool: &Path) -> Result<Vec<PendingEviction>, LeaseError> {
    let path = records_path(pool);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(LeaseError::UnexpectedEntry {
                place: "the pool control directory",
                entry: RECORDS_NAME.to_owned(),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    }
    let bytes = crate::lease::read_private_file_bounded(&path, RECORDS_MAX_BYTES)?;
    let records: Records =
        serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
    if records.version != RECORDS_VERSION || records.records.len() > RECORDS_MAX {
        return Err(LeaseError::UnknownAccounting);
    }
    for record in &records.records {
        if record.store_id != records.store_id
            || record.slot_id == 0
            || record.incarnation == 0
            || record.root_id.is_empty()
            || !record.path.is_absolute()
        {
            return Err(LeaseError::UnknownAccounting);
        }
    }
    Ok(records.records)
}

fn write(pool: &Path, store_id: &str, records: &[PendingEviction]) -> Result<(), LeaseError> {
    if records.len() > RECORDS_MAX {
        return Err(LeaseError::LedgerExhausted);
    }
    let document = Records {
        version: RECORDS_VERSION,
        store_id: store_id.to_owned(),
        records: records.to_vec(),
    };
    let bytes = serde_json::to_vec(&document).map_err(|_| LeaseError::UnknownAccounting)?;
    if bytes.len() as u64 > RECORDS_MAX_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    crate::lease::persist_private_file(&records_path(pool), &bytes)
}

/// Append one decision. The caller holds the pool lock; the record is durable
/// before the ledger row is dropped.
pub(super) fn push(pool: &Path, store_id: &str, record: PendingEviction) -> Result<(), LeaseError> {
    let mut records = read(pool)?;
    // A record from another store means this path was recreated; the old roots'
    // custody is unknown, so neither mix them nor claim them.
    if records.iter().any(|known| known.store_id != store_id) {
        return Err(LeaseError::BackingMismatch);
    }
    if records.iter().any(|known| {
        known.slot_id == record.slot_id
            && known.incarnation == record.incarnation
            && known.root_id == record.root_id
    }) {
        return Ok(());
    }
    if records.len() >= RECORDS_MAX {
        return Err(LeaseError::LedgerExhausted);
    }
    records.push(record);
    write(pool, store_id, &records)
}

fn remove_settled(pool: &Path, settled: &[PendingEviction]) -> Result<(), LeaseError> {
    let mut records = read(pool)?;
    let store_id = records
        .first()
        .map(|record| record.store_id.clone())
        .unwrap_or_else(|| String::new());
    records.retain(|known| {
        !settled.iter().any(|done| {
            done.slot_id == known.slot_id
                && done.incarnation == known.incarnation
                && done.root_id == known.root_id
        })
    });
    if records.is_empty() {
        match fs::remove_file(records_path(pool)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    } else {
        write(pool, &store_id, &records)
    }
}

/// Drop records whose recorded root path is gone: Barge's rename committed, and
/// the trash entry (if any) is the sweep's work. Called under the pool lock on
/// a mutating open. A still-present root stays recorded for a later pass.
pub(super) fn drop_completed(pool: &Path) -> Result<(), LeaseError> {
    let records = read(pool)?;
    if records.is_empty() {
        return Ok(());
    }
    let mut settled = Vec::new();
    for record in &records {
        match fs::symlink_metadata(&record.path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                settled.push(record.clone());
            }
            // Still present, or unreadable: a later pass lets Barge decide.
            _ => {}
        }
    }
    if settled.is_empty() {
        return Ok(());
    }
    remove_settled(pool, &settled)
}

/// Evict every recorded idle root and report what could not be deleted.
///
/// This is Barge's eviction, not a second implementation: `evict` revalidates
/// the marker, the cooperative hold and the pin and renames into `.trash/`
/// itself. Neither the pool nor the front door's admission lock is held here;
/// only the short reads and the record removal take the pool lock.
pub fn settle(pool: &Path) -> Result<Vec<String>, LeaseError> {
    let config = read_config(pool)?;
    let records = {
        let _lock = lock_pool(pool)?;
        let records = read(pool)?;
        if records
            .iter()
            .any(|record| record.store_id != config.store_id)
        {
            return Err(LeaseError::BackingMismatch);
        }
        records
    };
    let backing = pool_backing(pool, &config)?;
    let storage = crate::storage::PoolStorage::open(&pool.join(MANAGED_NAME), &backing)
        .map_err(|_| LeaseError::BackingMismatch)?;
    let mut messages = Vec::new();
    // Recovery trash may remain even after its pending record was dropped.
    // Sweep it here, never while opening the pool under its lock.
    #[cfg(test)]
    POOL_LOCK_FREE.with(|free| free.set(!crate::lease::pool::pool_lock_is_held(pool)));
    #[cfg(test)]
    let swept = if FAIL_SWEEP.with(|flag| flag.replace(false)) {
        Err("injected sweep failure".to_owned())
    } else {
        storage.sweep_all_trash()
    };
    #[cfg(not(test))]
    let swept = storage.sweep_all_trash();
    match swept {
        Ok(swept) => {
            for kept in swept.kept {
                messages.push(format!(
                    "trash at {} was kept: {}",
                    kept.path.display(),
                    kept.reason
                ));
            }
        }
        Err(error) => messages.push(format!("could not sweep Barge trash: {error}")),
    }
    let mut settled: Vec<PendingEviction> = Vec::new();
    for record in records {
        #[cfg(test)]
        {
            // The regression seam: this pass must never run while the pool lock
            // is held, so a large tree's deletion cannot delay another
            // admission (oxide#324). A non-blocking exclusive flock from this
            // same process succeeds only when nobody holds it.
            let free = !crate::lease::pool::pool_lock_is_held(pool);
            POOL_LOCK_FREE.with(|flag| flag.set(flag.get() && free));
        }
        match storage.evict_in(record.area_folder.as_deref(), &record.root_id) {
            Ok(evicted) => {
                if let Err(error) = require_deleted(evicted) {
                    // The root is out of the way; only the trash copy stayed.
                    messages.push(error.to_string());
                }
                settled.push(record);
            }
            Err(error) if error.kind() == barge::ErrorKind::NotFound => {
                // Already renamed or swept: nothing left to evict.
                settled.push(record);
            }
            Err(error) if crate::storage::eviction_moved(&error) => {
                messages.push(error.to_string());
                settled.push(record);
            }
            Err(error) => {
                // A hold, a pin or an unreadable marker: keep the record, and
                // let a later pass ask Barge again once the writer is gone.
                messages.push(format!(
                    "evicting idle slot {} from Barge trash: {error}",
                    record.slot_id
                ));
            }
        }
    }
    if !settled.is_empty() {
        let _lock = lock_pool(pool)?;
        remove_settled(pool, &settled)?;
    }
    Ok(messages)
}

#[cfg(test)]
thread_local! {
    /// Whether every `evict_in` this thread's last settle made ran with the
    /// pool lock free. The regression reads it after a settle; a settle with no
    /// records leaves it `true`.
    static FAIL_SWEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static POOL_LOCK_FREE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether the last settle on this thread made every Barge eviction without
/// the pool lock held.
#[cfg(test)]
pub(crate) fn last_settle_held_no_pool_lock() -> bool {
    POOL_LOCK_FREE.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn fail_next_sweep() {
    FAIL_SWEEP.with(|flag| flag.set(true));
}
