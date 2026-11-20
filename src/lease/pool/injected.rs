//! Test-only entry points that drive pool operations and inject faults.

use super::*;

// Test entry points: the library tests drive a budget-only directory pool.
#[cfg(test)]
pub(in crate::lease) fn create_injected(
    pool: &Path,
    cap_bytes: u64,
) -> Result<crate::lease::PoolInfo, LeaseError> {
    managed::init_budget_pool(pool, cap_bytes)
}

#[cfg(test)]
pub(in crate::lease) fn lease_injected(request: LeaseRequest) -> Result<LeaseGrant, LeaseError> {
    let mut leased = managed::lease(request)?;
    if let Some(hold) = leased.hold.take() {
        if let Ok(mut holds) = injected_holds().lock() {
            holds.insert(leased.grant.lease_id.clone(), hold);
        }
    }
    Ok(leased.grant)
}

/// Holds the test helpers took, keyed by lease. A test release or recovery
/// drops the hold first, exactly as the front door drops its `Lease` hold.
#[cfg(test)]
fn injected_holds() -> &'static std::sync::Mutex<std::collections::HashMap<String, barge::Hold>> {
    static HOLDS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, barge::Hold>>,
    > = std::sync::OnceLock::new();
    HOLDS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn drop_injected_hold(lease_id: &str) {
    let _ = injected_holds()
        .lock()
        .map(|mut holds| holds.remove(lease_id));
}

#[cfg(test)]
pub(in crate::lease) fn release_injected(
    pool: &Path,
    lease_id: &str,
) -> Result<ReleaseReceipt, LeaseError> {
    drop_injected_hold(lease_id);
    managed::release(pool, lease_id)
}

#[cfg(test)]
pub(in crate::lease) fn prune_injected(pool: &Path) -> Result<(), LeaseError> {
    Pool::open(pool)?.prune_storage()
}

/// Test-only: record a removable `apfs-volume-v1` backend with a deliberately
/// absent mount point, so a seeding call can be shown to gate the removable
/// path from the backend alone without opening or probing any volume.
#[cfg(test)]
pub(in crate::lease) fn rewrite_removable_backend_injected(
    pool: &Path,
    mount_point: &Path,
) -> Result<(), LeaseError> {
    let mut config = read_config(pool)?;
    config.backend = VOLUME_BACKEND_APFS_V1.to_owned();
    config.volume = Some(VolumeConfig {
        mount_point: mount_point.to_path_buf(),
        volume_uuid: "1".repeat(36),
        payload_dir: "payload".to_owned(),
    });
    write_config(pool, &config)
}

/// Test-only: give every idle slot a different `last_lease_id`, so a seed that
/// already holds the pool lock can be shown to re-check the donor's recorded
/// generation instead of trusting its own snapshot.
#[cfg(test)]
pub(in crate::lease) fn tamper_idle_generations_injected(pool: &Path) -> Result<(), LeaseError> {
    let mut ledger = read_ledger(pool)?;
    for slot in ledger.slots.iter_mut() {
        if slot.state == SlotPersistState::Idle && slot.lease_id.is_none() {
            slot.last_lease_id = "0".repeat(32);
        }
    }
    write_ledger(pool, &ledger)
}

#[cfg(test)]
thread_local! {
    pub(super) static PASSIVE_OPEN_DELAY: std::cell::Cell<std::time::Duration> = const {
        std::cell::Cell::new(std::time::Duration::ZERO)
    };
    /// One injected Barge failure on the next legacy adoption attempt, so a
    /// test can prove a refused adoption quarantines its row instead of
    /// refusing every command.
    pub(super) static FAIL_ADOPTION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(in crate::lease) fn fail_next_adoption() {
    FAIL_ADOPTION.with(|flag| flag.set(true));
}

#[cfg(test)]
pub(in crate::lease) fn with_passive_open_delay<T>(
    delay: std::time::Duration,
    body: impl FnOnce() -> T,
) -> T {
    struct Restore(std::time::Duration);
    impl Drop for Restore {
        fn drop(&mut self) {
            PASSIVE_OPEN_DELAY.with(|slot| slot.set(self.0));
        }
    }
    let _restore = Restore(PASSIVE_OPEN_DELAY.with(|slot| slot.replace(delay)));
    body()
}
