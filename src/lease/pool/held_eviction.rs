//! One pending held retirement per pool. Barge owns the actual eviction.
//! Recovery can retire a ledger row only from an intent for its exact lease
//! and root, under the original backing directory.

use super::{pool_backing, validate_lease_id, LeaseError, Pool, SlotPersistState, SlotRecord};
use crate::lease::{
    canonical_existing_dir, control::WRITER_GENERATION, current_uid, fence_admission_writer,
    io_error, persist_private_file, read_private_file_bounded, reserve_eviction_fence,
    HeldEvictionFailure, MANAGED_NAME,
};
use crate::storage::Storage;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const INTENT_NAME: &str = "held-eviction.json";
const INTENT_VERSION: u32 = 1;
const INTENT_MAX_BYTES: u64 = 4096;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeldEvictionIntent {
    version: u32,
    writer_generation: u32,
    store_id: String,
    slot_id: u64,
    incarnation: u64,
    lease_id: String,
    root_id: String,
    dir: Option<String>,
    volume_uuid: String,
    parent_inode: u64,
    root_inode: u64,
}

impl HeldEvictionIntent {
    fn path(pool: &Path) -> PathBuf {
        pool.join(MANAGED_NAME).join(INTENT_NAME)
    }

    pub(super) fn read(pool: &Path) -> Result<Option<Self>, LeaseError> {
        let path = Self::path(pool);
        match fs::symlink_metadata(&path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }
        let bytes = read_private_file_bounded(&path, INTENT_MAX_BYTES)?;
        let intent: Self =
            serde_json::from_slice(&bytes).map_err(|_| LeaseError::UnknownAccounting)?;
        if intent.version != INTENT_VERSION || intent.writer_generation == 0 || intent.slot_id == 0
        {
            return Err(LeaseError::UnknownAccounting);
        }
        if intent.writer_generation > WRITER_GENERATION {
            return Err(LeaseError::WriterUpdated);
        }
        validate_lease_id(&intent.lease_id)?;
        Ok(Some(intent))
    }

    fn write(&self, pool: &Path) -> Result<(), LeaseError> {
        let bytes = serde_json::to_vec(self).map_err(|_| LeaseError::UnknownAccounting)?;
        if bytes.len() as u64 > INTENT_MAX_BYTES {
            return Err(LeaseError::LedgerExhausted);
        }
        persist_private_file(&Self::path(pool), &bytes)
    }

    fn clear(&self, pool: &Path) -> Result<(), LeaseError> {
        fs::remove_file(Self::path(pool)).map_err(io_error)?;
        File::open(pool.join(MANAGED_NAME))
            .and_then(|control| control.sync_all())
            .map_err(io_error)
    }

    fn matches(&self, slot: &SlotRecord) -> bool {
        slot.state == SlotPersistState::Held
            && slot.owned
            && slot.incarnation == self.incarnation
            && slot.lease_id.as_deref() == Some(&self.lease_id)
            && slot.root.as_deref() == Some(&self.root_id)
            && slot.dir == self.dir
    }

    fn remains_at(
        &self,
        area: &barge::Area,
        path: &Path,
        parent_device: u64,
    ) -> Result<bool, LeaseError> {
        match fs::symlink_metadata(path) {
            Ok(metadata)
                if metadata.is_dir()
                    && metadata.uid() == current_uid()
                    && metadata.dev() == parent_device
                    && metadata.ino() == self.root_inode
                    && area
                        .resolve(&self.root_id)
                        .is_ok_and(|root| root.path == path) =>
            {
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match area.resolve(&self.root_id) {
                    Err(error) if error.kind() == barge::ErrorKind::NotFound => Ok(false),
                    _ => Err(LeaseError::UnknownAccounting),
                }
            }
            _ => Err(LeaseError::UnknownAccounting),
        }
    }
}

/// Bind the parent to the recorded Barge location before considering any
/// missing leaf. An absent, substituted or unreadable parent proves nothing.
fn open_parent(area: &barge::Area, path: &Path) -> Result<File, LeaseError> {
    let parent = path.parent().ok_or(LeaseError::UnknownAccounting)?;
    let location = area.location().map_err(|_| LeaseError::UnknownAccounting)?;
    if location.path() != Some(parent) {
        return Err(LeaseError::UnknownAccounting);
    }
    let parent = canonical_existing_dir(parent, true)?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(parent)
        .map_err(io_error)
}

impl Pool {
    /// Settle only the recorded lease. A still-present original root cancels
    /// the intent. Recovery never asks Barge to evict or delete anything.
    pub(super) fn recover_held_eviction(
        &mut self,
        intent: &HeldEvictionIntent,
        publish_fence: bool,
    ) -> Result<(), LeaseError> {
        if intent.store_id != self.ledger.store_id || intent.slot_id >= self.ledger.next_slot {
            return Err(LeaseError::UnknownAccounting);
        }
        let Some(slot) = self.slot(intent.slot_id) else {
            // The ledger replacement already committed; only cleanup remains.
            return intent.clear(&self.root);
        };
        if !intent.matches(slot) {
            return Err(LeaseError::UnknownAccounting);
        }
        let path = self.slot_path(slot.id)?;
        let control = self.root.join(MANAGED_NAME);
        let storage =
            Storage::open_read_only(&control).map_err(|_| LeaseError::UnknownAccounting)?;
        let area = pool_backing(&self.root, &self.config)?
            .existing_pool_area(&storage, slot.area_folder())
            .map_err(|_| LeaseError::UnknownAccounting)?;
        let parent = open_parent(&area, &path)?;
        let metadata = parent.metadata().map_err(io_error)?;
        if !area
            .volume()
            .spec()
            .uuid
            .eq_ignore_ascii_case(&intent.volume_uuid)
            || metadata.ino() != intent.parent_inode
        {
            return Err(LeaseError::UnknownAccounting);
        }
        if intent.remains_at(&area, &path, metadata.dev())? {
            return intent.clear(&self.root);
        }
        self.settle_held_eviction(intent, &parent, publish_fence)
    }

    fn settle_held_eviction(
        &mut self,
        intent: &HeldEvictionIntent,
        parent: &File,
        publish_fence: bool,
    ) -> Result<(), LeaseError> {
        parent.sync_all().map_err(io_error)?;
        #[cfg(test)]
        interrupt_at(Interruption::BeforeFence)?;
        if publish_fence {
            fence_admission_writer(&self.root.join(MANAGED_NAME))?;
        }
        #[cfg(test)]
        interrupt_at(Interruption::BeforeLedger)?;
        self.ledger.slots.retain(|slot| slot.id != intent.slot_id);
        self.persist_revised()?;
        #[cfg(test)]
        interrupt_at(Interruption::BeforeCleanup)?;
        intent.clear(&self.root)
    }

    pub(super) fn evict_held(
        &mut self,
        slot_id: &str,
        lease_id: &str,
    ) -> Result<Vec<String>, HeldEvictionFailure> {
        let pre = HeldEvictionFailure::PreCommit;
        let id = slot_id
            .parse::<u64>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or_else(|| {
                pre(LeaseError::InvalidInput(
                    "slot ID must be a positive integer",
                ))
            })?;
        let post = |error| {
            HeldEvictionFailure::PostCommit(format!(
                "held eviction of slot {id} requires settlement: {error}"
            ))
        };
        let control = self.root.join(MANAGED_NAME);
        if let Some(intent) = HeldEvictionIntent::read(&self.root).map_err(pre)? {
            if intent.slot_id != id || intent.lease_id != lease_id {
                return Err(pre(LeaseError::Busy));
            }
            let admission = reserve_eviction_fence(&control).map_err(pre)?;
            self.recover_held_eviction(&intent, admission.is_some())
                .map_err(post)?;
            if self.slot(id).is_none() {
                return Ok(Vec::new());
            }
        }
        let slot = self.slot(id).ok_or(pre(LeaseError::Busy))?;
        if slot.state != SlotPersistState::Held || slot.lease_id.as_deref() != Some(lease_id) {
            return Err(pre(LeaseError::Busy));
        }
        let root_id = slot
            .root
            .as_deref()
            .filter(|_| slot.owned)
            .ok_or(pre(LeaseError::UnknownAccounting))?;
        let path = self.slot_path(id).map_err(pre)?;
        let backing = pool_backing(&self.root, &self.config).map_err(pre)?;
        let storage =
            Storage::open_read_only(&control).map_err(|_| pre(LeaseError::UnknownAccounting))?;
        let area = backing
            .existing_pool_area(&storage, slot.area_folder())
            .map_err(|_| pre(LeaseError::UnknownAccounting))?;
        let parent = open_parent(&area, &path).map_err(pre)?;
        let parent_metadata = parent.metadata().map_err(io_error).map_err(pre)?;
        let root_metadata = fs::symlink_metadata(&path).map_err(io_error).map_err(pre)?;
        if !root_metadata.is_dir()
            || root_metadata.uid() != current_uid()
            || root_metadata.dev() != parent_metadata.dev()
            || !area.resolve(root_id).is_ok_and(|root| root.path == path)
        {
            return Err(pre(LeaseError::UnknownAccounting));
        }
        let intent = HeldEvictionIntent {
            version: INTENT_VERSION,
            writer_generation: WRITER_GENERATION,
            store_id: self.ledger.store_id.clone(),
            slot_id: id,
            incarnation: slot.incarnation,
            lease_id: lease_id.to_owned(),
            root_id: root_id.to_owned(),
            dir: slot.dir.clone(),
            volume_uuid: area.volume().spec().uuid.clone(),
            parent_inode: parent_metadata.ino(),
            root_inode: root_metadata.ino(),
        };
        // The read-only lookup proved the account exists. The writable lookup
        // uses its recorded declaration without registering or creating areas.
        let admission = reserve_eviction_fence(&control).map_err(pre)?;
        let storage = Storage::open(&control).map_err(|_| pre(LeaseError::UnknownAccounting))?;
        let area = backing
            .existing_pool_area(&storage, slot.area_folder())
            .map_err(|_| pre(LeaseError::UnknownAccounting))?;
        self.validate_revised_ledger().map_err(pre)?;
        if let Err(error) = intent.write(&self.root) {
            // An error syncing the directory can leave the new intent visible.
            return if HeldEvictionIntent::path(&self.root)
                .symlink_metadata()
                .is_err_and(|io| io.kind() == std::io::ErrorKind::NotFound)
            {
                Err(pre(error))
            } else {
                Err(post(error))
            };
        }
        #[cfg(test)]
        interrupt_at(Interruption::BeforeRename).map_err(post)?;
        if area.evict(&intent.root_id).is_err() {
            intent.clear(&self.root).map_err(post)?;
            return Err(pre(LeaseError::UnknownAccounting));
        }
        self.settle_held_eviction(&intent, &parent, admission.is_some())
            .map_err(post)?;
        Ok(Vec::new())
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq)]
enum Interruption {
    BeforeRename,
    BeforeFence,
    BeforeLedger,
    BeforeCleanup,
}

#[cfg(test)]
thread_local! {
    static INTERRUPT: std::cell::Cell<Option<Interruption>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn interrupt_at(point: Interruption) -> Result<(), LeaseError> {
    if INTERRUPT.with(|fault| fault.get() == Some(point) && fault.take().is_some()) {
        return Err(LeaseError::Io(
            "injected held-eviction interruption".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::control::{fenced_state, parse_state, State, HOLDING_STATE_VERSION};
    use crate::lease::managed;
    use crate::lease::tests::{request, Fixture};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn interrupted_held_eviction_settles_only_its_recorded_root() {
        for (label, interruption, renamed, fenced, ledger_saved) in [
            (
                "before-rename",
                Interruption::BeforeRename,
                false,
                false,
                false,
            ),
            (
                "before-fence",
                Interruption::BeforeFence,
                true,
                false,
                false,
            ),
            (
                "before-ledger",
                Interruption::BeforeLedger,
                true,
                true,
                false,
            ),
            (
                "before-cleanup",
                Interruption::BeforeCleanup,
                true,
                true,
                true,
            ),
        ] {
            let fixture = Fixture::new(label);
            let pool = fixture.pool();
            let leased = managed::lease_measured(request(
                &pool,
                &fixture.dir("project"),
                &fixture.dir("workspace"),
                label,
            ))
            .unwrap();
            let grant = &leased.grant;
            let slot = grant.slot_id.clone();
            let lease = grant.lease_id.clone();
            let root = grant
                .env
                .cargo_build_build_dir
                .parent()
                .unwrap()
                .to_path_buf();
            let control = pool.join(MANAGED_NAME);
            let state_path = control.join("state.json");
            let admission = control.join("admission.lock");
            let ledger = pool.join("ledger.json");
            let before = fs::read(&ledger).unwrap();
            let mut old_state = fenced_state(State::default(), WRITER_GENERATION).unwrap();
            old_state.version = HOLDING_STATE_VERSION;
            old_state.minimum_writer_generation = None;
            fs::write(&state_path, serde_json::to_vec(&old_state).unwrap()).unwrap();
            fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(&admission, []).unwrap();
            fs::set_permissions(&admission, fs::Permissions::from_mode(0o600)).unwrap();
            drop(leased);

            INTERRUPT.set(Some(interruption));
            assert!(
                matches!(managed::evict_held_slot(&pool, &slot, &lease),
                    Err(HeldEvictionFailure::PostCommit(reason)) if reason.contains("injected")),
                "{label}"
            );
            assert!(HeldEvictionIntent::path(&pool).is_file(), "{label}");
            assert_eq!(!root.exists(), renamed, "{label}");
            assert_eq!(
                fs::read(&ledger).unwrap() != before,
                ledger_saved,
                "{label}"
            );
            let after = parse_state(&fs::read(&state_path).unwrap()).unwrap();
            assert_eq!(
                after.minimum_writer_generation,
                fenced.then_some(WRITER_GENERATION),
                "{label}"
            );
            assert_eq!(
                managed::snapshot(&pool).unwrap().slots.is_empty(),
                ledger_saved,
                "{label}"
            );
            assert!(
                HeldEvictionIntent::path(&pool).is_file(),
                "passive status consumed the intent"
            );

            if interruption == Interruption::BeforeFence {
                let parent = root.parent().unwrap();
                let original = fixture.dir("retained").join("original-parent");
                fs::rename(parent, &original).unwrap();
                fs::create_dir(parent).unwrap();
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
                assert!(matches!(
                    managed::prepare_pool(&pool),
                    Err(LeaseError::UnknownAccounting)
                ));
                assert_eq!(fs::read(&ledger).unwrap(), before);
                assert!(HeldEvictionIntent::path(&pool).is_file());
                fs::remove_dir(parent).unwrap();
                fs::rename(original, parent).unwrap();
            }
            if interruption == Interruption::BeforeLedger {
                assert!(matches!(
                    managed::evict_held_slot(&pool, &slot, "different lease"),
                    Err(HeldEvictionFailure::PreCommit(LeaseError::Busy))
                ));
                managed::evict_held_slot(&pool, &slot, &lease).unwrap();
            } else {
                managed::prepare_pool(&pool).unwrap();
            }
            assert!(!HeldEvictionIntent::path(&pool).exists(), "{label}");
            assert_eq!(!root.exists(), renamed, "{label}");
            let settled = managed::prepare_pool(&pool).unwrap();
            assert_eq!(settled.slots.is_empty(), renamed, "{label}");
            let after = parse_state(&fs::read(&state_path).unwrap()).unwrap();
            assert_eq!(
                after.minimum_writer_generation,
                Some(WRITER_GENERATION),
                "{label}"
            );
        }
    }

    #[test]
    fn a_missing_held_root_without_an_intent_stays_recorded() {
        let fixture = Fixture::new("missing-held-root");
        let pool = fixture.pool();
        let leased = managed::lease_measured(request(
            &pool,
            &fixture.dir("project"),
            &fixture.dir("workspace"),
            "missing",
        ))
        .unwrap();
        let grant = &leased.grant;
        let root = grant
            .env
            .cargo_build_build_dir
            .parent()
            .unwrap()
            .to_path_buf();
        let retained = fixture.dir("retained").join("root");
        drop(leased);
        fs::rename(root, &retained).unwrap();
        let before = fs::read(pool.join("ledger.json")).unwrap();
        managed::prepare_pool(&pool).unwrap();
        assert_eq!(fs::read(pool.join("ledger.json")).unwrap(), before);
        assert!(retained.join(barge::root::MARKER_NAME).is_file());
    }
}
