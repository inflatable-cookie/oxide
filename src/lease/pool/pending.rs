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
use crate::lease::{check_control_writer, io_error, random_hex};
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

/// Preserve an unreadable hand-off list beside itself and report that this
/// open quarantined it. One corrupt file must not refuse every later lease,
/// release and prepare (the outage-2255 class), and no deletion decision can
/// be read from it, so its roots stay exactly as they are recorded: nothing
/// here evicts, and each row it named keeps whatever generation it had.
pub(super) fn read_quarantining(pool: &Path) -> Result<(Vec<PendingEviction>, bool), LeaseError> {
    match read(pool) {
        Ok(records) => Ok((records, false)),
        Err(error)
            if matches!(
                error,
                LeaseError::UnknownAccounting
                    | LeaseError::UnexpectedEntry { .. }
                    | LeaseError::LedgerExhausted
            ) =>
        {
            let aside = pool
                .join(MANAGED_NAME)
                .join(format!("{RECORDS_NAME}.quarantine-{}", random_hex(8)?));
            fs::rename(records_path(pool), aside).map_err(io_error)?;
            // The rename is durable before any later open can read the list as
            // empty and decide from it.
            fs::File::open(pool.join(MANAGED_NAME))
                .and_then(|control| control.sync_all())
                .map_err(io_error)?;
            Ok((Vec::new(), true))
        }
        Err(error) => Err(error),
    }
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

/// Remove the hand-off records `settled` names. The caller holds the pool
/// lock; every removal is durable before the next command reads the list.
pub(super) fn remove_settled(pool: &Path, settled: &[PendingEviction]) -> Result<(), LeaseError> {
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
            // The unlink is the durable end of the hand-off list: without this
            // sync a crash could replay a removal whose deletion already
            // completed (oxide#337).
            Ok(()) => sync_control_dir(pool),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    } else {
        write(pool, &store_id, &records)
    }
}

fn sync_control_dir(pool: &Path) -> Result<(), LeaseError> {
    fs::File::open(pool.join(MANAGED_NAME))
        .and_then(|control| control.sync_all())
        .map_err(io_error)
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
    // The pool path and its recorded shape are judged read-only first, before
    // this pass reads the config or creates and takes the pool lock: a
    // symlinked, foreign or retired pool must refuse untouched, and no other
    // window may leave a lock file in one (oxide#337).
    crate::lease::canonical_existing_dir(pool, true)?;
    super::refuse_foreign_pool_entries(pool)?;
    super::refuse_retired_image_pool(pool)?;
    let config = read_config(pool)?;
    let records = {
        let _lock = lock_pool(pool)?;
        // The decision was committed at one writer generation; a later binary
        // must not act on it. This is the same fence every other mutating
        // open checks, and it runs in the first pool-lock window.
        check_control_writer(&pool.join(MANAGED_NAME))?;
        let records = read(pool)?;
        if records
            .iter()
            .any(|record| record.store_id != config.store_id)
        {
            return Err(LeaseError::BackingMismatch);
        }
        records
    };
    #[cfg(test)]
    maybe_bump_writer(pool);
    let backing = pool_backing(pool, &config)?;
    let storage = crate::storage::PoolStorage::open(&pool.join(MANAGED_NAME), &backing)
        // Barge's own failure keeps its own message: this is custody that could
        // not be read or changed, never a pool recording another backing
        // (oxide#337).
        .map_err(LeaseError::Barge)?;
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
        // The first window's writer check is stale: another binary may have
        // published a newer writer generation while the evictions ran. Refuse
        // before removing any record rather than acting on the old decision.
        check_control_writer(&pool.join(MANAGED_NAME))?;
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
    /// Publish a newer writer generation between the two lock windows of the
    /// next settle on this thread.
    static BUMP_WRITER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test seam: make the next settle on this thread see a newer writer generation
/// after its first pool-lock window, as a concurrently installed binary would.
#[cfg(test)]
fn bump_writer_on_next_settle() {
    BUMP_WRITER.with(|flag| flag.set(true));
}

#[cfg(test)]
fn maybe_bump_writer(pool: &Path) {
    if !BUMP_WRITER.with(|flag| flag.replace(false)) {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let control = pool.join(MANAGED_NAME);
    fs::create_dir_all(&control).expect("the control directory");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("control mode");
    let path = control.join("state.json");
    let mut state: serde_json::Value = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(
            || serde_json::json!({"version": crate::lease::control::WRITER_STATE_VERSION}),
        );
    state["version"] = serde_json::json!(crate::lease::control::WRITER_STATE_VERSION);
    state["minimum_writer_generation"] =
        serde_json::json!(crate::lease::control::WRITER_GENERATION + 1);
    fs::write(&path, serde_json::to_vec(&state).expect("control JSON")).expect("write state");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("state mode");
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

#[cfg(test)]
mod regressions {
    use super::*;
    use crate::lease::managed;
    use crate::lease::{LeaseRequest, Profile, SlotLayout};
    use std::os::unix::fs::PermissionsExt;

    fn fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let root = base.join(format!(
            "lease-pending-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture");
        let root = fs::canonicalize(&root).expect("canonical fixture");
        let project = root.join("project");
        fs::create_dir_all(&project).expect("project");
        (root.clone(), root.join("pool"), project)
    }

    fn request(pool: &Path, project: &Path) -> LeaseRequest {
        LeaseRequest {
            pool: pool.to_path_buf(),
            project: project.to_path_buf(),
            workspace: project.to_path_buf(),
            profile: Profile::Dev,
            holder: "holder".to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout: Some(SlotLayout {
                folders: vec!["repo".to_owned()],
                profile: "dev".to_owned(),
            }),
            replace_slot: None,
        }
    }

    /// Init a pool, commit one idle-eviction decision for a released slot, and
    /// leave the record for a settle to finish.
    fn ready_pool(pool: &Path, project: &Path) {
        managed::init_budget_pool(pool, 8 << 20).expect("pool");
        let leased = managed::lease(request(pool, project)).expect("lease");
        let slot = leased.grant.slot_id.clone();
        managed::release(pool, &leased.grant.lease_id).expect("release");
        drop(leased);
        managed::evict_idle_slot(pool, &slot).expect("decision");
        assert!(records_path(pool).is_file(), "a decision is recorded");
    }

    /// Regression: settle accepted a pool directory that every other mutating
    /// open refuses, so it could lock and write in a group-writable directory.
    #[test]
    fn settle_refuses_a_pool_directory_that_is_not_owner_only() {
        if crate::lease::current_uid() == 0 {
            return; // The superuser is not stopped by the group-writable bit.
        }
        let (root, pool, _project) = fixture("owner-only");
        managed::init_budget_pool(&pool, 8 << 20).expect("pool");
        fs::set_permissions(&pool, fs::Permissions::from_mode(0o770)).expect("mode");
        let refused = managed::settle_pending_evictions(&pool);
        assert!(
            matches!(refused, Err(LeaseError::UnsafePath(_))),
            "{refused:?}"
        );
        fs::set_permissions(&pool, fs::Permissions::from_mode(0o700)).expect("mode");
        fs::remove_dir_all(&root).expect("remove own fixture");
    }

    /// Regression: the first pool-lock window's writer check was stale, so a
    /// settle that outlived another binary's writer fence removed records on a
    /// decision it must no longer act on.
    #[test]
    fn settle_rechecks_the_writer_generation_before_removing_records() {
        let (root, pool, project) = fixture("writer-recheck");
        ready_pool(&pool, &project);
        bump_writer_on_next_settle();
        let refused = managed::settle_pending_evictions(&pool);
        assert!(
            matches!(refused, Err(LeaseError::WriterUpdated)),
            "{refused:?}"
        );
        let bytes = fs::read(records_path(&pool)).expect("the record survives");
        let records: serde_json::Value = serde_json::from_slice(&bytes).expect("records");
        assert!(
            records["records"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty()),
            "{records}"
        );
        fs::remove_dir_all(&root).expect("remove own fixture");
    }
}
