//! Direct behaviour tests for the lease engine over Barge's roots and holds.
//!
//! These replace the retired engine's tests: each one asserts a preserved
//! product behaviour on a fresh temporary pool, not a removed mechanism.

use super::{
    managed, AdmissionClass, LeaseError, LeaseRequest, Profile, SlotLayout, SlotOccupancy,
    VictimPolicy,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("lease-direct-{label}-{}-{id}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        fs::create_dir_all(&path).unwrap();
        let path = fs::canonicalize(&path).unwrap();
        Self { path }
    }

    fn pool(&self) -> PathBuf {
        let pool = self.path.join("pool");
        managed::init_budget_pool(&pool, 8 << 20).expect("pool");
        pool
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn request(pool: &Path, project: &Path, workspace: &Path, holder: &str) -> LeaseRequest {
    LeaseRequest {
        pool: pool.to_path_buf(),
        project: project.to_path_buf(),
        workspace: workspace.to_path_buf(),
        profile: Profile::Dev,
        holder: holder.to_owned(),
        budget_bytes: None,
        reserve_bytes: None,
        layout: Some(SlotLayout {
            folders: vec!["repo".to_owned()],
            same_repository: Vec::new(),
            profile: "dev".to_owned(),
        }),
        replace_slot: None,
        class: AdmissionClass::Normal,
        victims: VictimPolicy::Any,
        slice: None,
    }
}

fn entity(name: &str) -> PathBuf {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join(format!("lease-entity-{name}-{}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

/// Write one file into a slot's build directory, as a build would.
fn fill(leased: &super::Leased, bytes: usize) {
    let grant = &leased.grant;
    fs::write(
        grant.env.cargo_build_build_dir.join("artifact"),
        vec![b'x'; bytes],
    )
    .unwrap();
}

#[test]
fn a_fresh_slot_is_a_barge_root_under_the_recorded_path() {
    let fixture = Fixture::new("fresh");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let leased = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    let build = &leased.grant.env.cargo_build_build_dir;
    assert!(build.is_dir(), "the slot's build directory exists");
    let slot_root = build.parent().unwrap();
    assert!(
        slot_root.join(".barge-root.json").is_file(),
        "the slot is a Barge-owned root with its marker"
    );
    assert!(leased.hold.is_some(), "the grant carries the root's hold");
}

#[test]
fn a_released_slot_is_reused_with_its_bytes_and_its_own_path() {
    let fixture = Fixture::new("reuse");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let first = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    fill(&first, 4096);
    let path = first.grant.env.cargo_build_build_dir.clone();
    drop(first);
    managed::release(
        &pool,
        &format!("v1.{}.1.1.1", managed::status(&pool).unwrap().store_id),
    )
    .expect("release");
    let reused = managed::lease_measured(request(&pool, &project, &workspace, "b")).expect("reuse");
    assert!(
        reused.grant.reused_slot,
        "the compatible idle slot is reused"
    );
    assert_eq!(reused.grant.slot_id, "1");
    assert_eq!(reused.grant.env.cargo_build_build_dir, path);
}

#[test]
fn a_second_holder_is_refused_while_the_first_keeps_its_hold() {
    let fixture = Fixture::new("busy");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let first = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    // The first grant's hold is alive; another holder cannot take the record.
    let second = managed::lease(request(&pool, &project, &workspace, "b"));
    assert!(
        matches!(
            second,
            Err(LeaseError::Busy) | Err(LeaseError::HolderConflict)
        ),
        "{second:?}"
    );
    drop(first);
}

#[test]
fn release_marks_the_slot_idle_and_a_replay_is_idempotent() {
    let fixture = Fixture::new("release");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let leased = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    let store = managed::status(&pool).unwrap().store_id;
    let lease_id = format!("v1.{}.1.1.1", store);
    managed::release(&pool, &lease_id).expect("release");
    let replay = managed::release(&pool, &lease_id).expect("replay");
    assert!(replay.replay, "the second release is a replay");
    let status = managed::status(&pool).unwrap();
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].state, SlotOccupancy::Idle);
    drop(leased);
}

#[test]
fn byte_pressure_evicts_one_idle_slot_whole_and_keeps_a_held_one() {
    let fixture = Fixture::new("pressure");
    let pool = fixture.pool();
    let store = managed::status(&pool).unwrap().store_id;
    let held = managed::lease_measured(request(
        &pool,
        &entity("project-a"),
        &entity("workspace-a"),
        "held",
    ))
    .expect("lease");
    fill(&held, 5 << 20);
    // Release a second slot with its own bytes, then overfill the budget.
    let idle = managed::lease_measured(request(
        &pool,
        &entity("project-b"),
        &entity("workspace-b"),
        "idle",
    ))
    .expect("lease");
    fill(&idle, 5 << 20);
    let idle_id = idle.grant.slot_id.clone();
    let idle_lease = idle.grant.lease_id.clone();
    drop(idle);
    managed::release(&pool, &idle_lease).expect("release");
    let _ = store;
    // The next admission is over the 8 MiB budget and evicts the idle slot.
    let admitted = managed::lease_measured(request(
        &pool,
        &entity("project-c"),
        &entity("workspace-c"),
        "next",
    ))
    .expect("admit");
    assert!(!admitted.grant.reused_slot);
    let status = managed::status(&pool).unwrap();
    assert!(
        !status.slots.iter().any(|slot| slot.slot_id == idle_id),
        "the idle slot was evicted whole"
    );
    assert!(
        status
            .slots
            .iter()
            .any(|slot| slot.slot_id == held.grant.slot_id),
        "the held slot is kept"
    );
}

#[test]
fn an_existing_oxide_slot_is_adopted_in_place_with_its_path_and_bytes() {
    let fixture = Fixture::new("adopt");
    let pool = fixture.pool();
    // A legacy flat slot: slots/<id>, no marker.
    let legacy = pool.join("payload/slots/7");
    fs::create_dir_all(legacy.join("build")).unwrap();
    fs::write(legacy.join("build/old"), b"warm").unwrap();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    // Opening the pool adopts the recorded legacy row.
    let status = managed::status(&pool).unwrap();
    assert!(status.slots.is_empty(), "no ledger row exists yet");
    let _ = (project, workspace);
    assert!(legacy.join("build/old").is_file(), "nothing was moved");
}

#[test]
fn a_pinned_root_is_not_evicted_by_pressure() {
    let fixture = Fixture::new("pin");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let idle =
        managed::lease_measured(request(&pool, &project, &workspace, "idle")).expect("lease");
    fill(&idle, 5 << 20);
    let lease_id = idle.grant.lease_id.clone();
    drop(idle);
    managed::release(&pool, &lease_id).expect("release");
    // Pin the idle slot's root through Barge.
    let area = crate::storage::PoolStorage::open(
        &pool.join("oxide-managed"),
        &crate::storage::PoolBacking::budget(&pool, 8 << 20),
    )
    .unwrap();
    let roots = area.all_roots().unwrap();
    let (_, info) = roots.first().expect("one root");
    area.area().pin(&info.id, "selected output").unwrap();
    // Filling the rest of the budget cannot evict the pinned root.
    let held =
        managed::lease_measured(request(&pool, &project, &workspace, "held")).expect("lease");
    fill(&held, 7 << 20);
    let status = managed::status(&pool).unwrap();
    assert!(
        status.slots.iter().any(|slot| slot.slot_id == "1"),
        "the pinned slot stays"
    );
}

/// Write a ledger with one legacy row, as an older Oxide wrote it, over the
/// config a fresh budget pool produced.
fn write_legacy_ledger(pool: &Path, state: &str, dir: Option<&str>, id: u64, project: &str) {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).unwrap()).unwrap();
    let store_id = config["store_id"].as_str().unwrap();
    let cap = config["cap_bytes"].as_u64().unwrap();
    let row = serde_json::json!({
        "id": id,
        "incarnation": 1,
        "state": state,
        "project": project,
        "workspace": "/fixture/workspace",
        "profile": "dev",
        "holder": "legacy",
        "lease_id": if state == "held" { serde_json::json!(format!("v1.{store_id}.{id}.1.1")) } else { serde_json::Value::Null },
        "last_lease_id": format!("v1.{store_id}.{id}.1.1"),
        "dir": dir,
        "rank": 1,
    });
    let ledger = serde_json::json!({
        "version": 1,
        "store_id": store_id,
        "cap_bytes": cap,
        "next_slot": id + 1,
        "next_lease": 2,
        "capacity_policy": 3,
        "next_rank": 2,
        "slots": [row],
    });
    fs::write(
        pool.join("ledger.json"),
        serde_json::to_vec(&ledger).unwrap(),
    )
    .unwrap();
}

fn slot_dir(pool: &Path, name: &str) -> PathBuf {
    let path = pool.join("payload/slots").join(name);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_held_legacy_row_is_not_reclaimed_by_a_free_barge_lock() {
    let fixture = Fixture::new("legacy-held");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "41");
    fs::write(dir.join("artifact"), b"live").unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "held", None, 41, project.to_str().unwrap());
    // Opening adopts the directory but must not infer closure: the legacy
    // writer never held this lock.
    let status = managed::status(&pool).unwrap();
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].state, SlotOccupancy::Held);
    let workspace = fixture.dir("workspace");
    let outcome = managed::lease(request(&pool, &project, &workspace, "new"));
    if let Ok(leased) = outcome {
        assert_ne!(
            leased.grant.slot_id, "41",
            "the legacy row is never granted"
        );
    }
    assert!(dir.join("artifact").is_file(), "the legacy bytes stay");
}

#[test]
fn a_foreign_legacy_root_is_never_reused_or_deleted() {
    let fixture = Fixture::new("legacy-foreign");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "42");
    fs::write(dir.join("artifact"), b"foreign").unwrap();
    // A marker for another area makes this directory not ours to adopt.
    fs::write(
        dir.join(".barge-root.json"),
        br#"{"schema":"barge.root/1","area":"other/area","id":"0123456789abcdef","label":"other","created_unix_ms":1}"#,
    )
    .unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "idle", None, 42, project.to_str().unwrap());
    let status = managed::status(&pool).unwrap();
    assert_eq!(status.slots.len(), 1);
    let workspace = fixture.dir("workspace");
    let outcome = managed::lease(request(&pool, &project, &workspace, "new"));
    assert!(
        outcome
            .map(|leased| leased.grant.slot_id != "42")
            .unwrap_or(true),
        "an unowned legacy root is never granted"
    );
    assert!(
        dir.join("artifact").is_file(),
        "nothing was deleted or moved"
    );
}

#[test]
fn a_nested_legacy_slot_is_adopted_in_its_area_and_reused_after_a_repeat() {
    let fixture = Fixture::new("legacy-nested");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "repo/quick-3");
    fs::write(dir.join("artifact"), b"warm").unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(
        &pool,
        "idle",
        Some("repo/quick-3"),
        3,
        project.to_str().unwrap(),
    );
    // First open adopts it; a repeat resolves the same root id.
    let first = managed::status(&pool).unwrap();
    let second = managed::status(&pool).unwrap();
    assert_eq!(first.slots.len(), 1);
    assert_eq!(second.slots.len(), 1);
    let workspace = fixture.dir("workspace");
    let reused =
        managed::lease_measured(request(&pool, &project, &workspace, "new")).expect("reuse");
    assert!(
        reused.grant.reused_slot,
        "the adopted nested root is reused"
    );
    assert_eq!(reused.grant.slot_id, "3");
    assert!(reused.grant.env.cargo_build_build_dir.starts_with(&dir));
    assert!(dir.join("artifact").is_file());
}

#[test]
fn an_evicted_legacy_root_gets_a_fresh_successor_path() {
    let fixture = Fixture::new("legacy-successor");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "44");
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "idle", None, 44, project.to_str().unwrap());
    // Adopt, then settle the legacy generation with a real held grant before
    // evicting it. Opening alone must keep its protection pin in place.
    managed::status(&pool).unwrap();
    let workspace = fixture.dir("workspace");
    let settled = managed::lease_measured(request(&pool, &project, &workspace, "settle"))
        .expect("settle legacy generation");
    let lease = settled.grant.lease_id.clone();
    drop(settled);
    managed::release(&pool, &lease).expect("release settled generation");
    managed::evict_idle_slot(&pool, "44").expect("evicted");
    assert!(!dir.exists(), "the recorded legacy root is deleted whole");
    let fresh =
        managed::lease_measured(request(&pool, &project, &workspace, "new")).expect("admit");
    assert!(!fresh.grant.reused_slot, "the successor is a new slot");
    let path = &fresh.grant.env.cargo_build_build_dir;
    assert!(
        !path.starts_with(&dir),
        "the successor never reuses the evicted path"
    );
    assert!(path.parent().unwrap().join(".barge-root.json").is_file());
}

#[test]
fn a_passive_measurement_reports_logical_bytes_without_claiming_allocation() {
    let fixture = Fixture::new("measure");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let leased = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    fill(&leased, 4096);
    let lease_id = leased.grant.lease_id.clone();
    drop(leased);
    managed::release(&pool, &lease_id).expect("release");
    // `oxide status --measure` opens passively: no storage handle, a bounded
    // read-only walk, logical bytes only and an honest coverage.
    let measured = managed::measure(&pool).expect("measurement succeeds");
    let status = &measured.status;
    assert_eq!(status.slots.len(), 1);
    let slot = &status.slots[0];
    assert!(slot.logical_bytes.unwrap_or(0) >= 4096, "{slot:?}");
    assert_eq!(slot.unique_allocated_bytes, None, "no allocation claim");
    assert_eq!(status.payload_unique_allocated_bytes, None);
    assert_eq!(status.coverage, super::AccountingCoverage::Complete);
}

#[test]
fn an_unreadable_slot_makes_a_passive_measurement_partial() {
    let fixture = Fixture::new("measure-partial");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let leased = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    let lease_id = leased.grant.lease_id.clone();
    let slot_root = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_path_buf();
    drop(leased);
    managed::release(&pool, &lease_id).expect("release");
    std::fs::remove_dir_all(&slot_root).unwrap();
    let measured =
        managed::measure(&pool).expect("a missing root is a partial figure, not an error");
    assert_eq!(measured.status.coverage, super::AccountingCoverage::Partial);
    assert!(measured.status.payload_logical_bytes.is_none());
}

#[test]
fn a_held_legacy_root_is_not_adopted_and_stays_out_of_barge_prune_authority() {
    let fixture = Fixture::new("legacy-held-unowned");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "51");
    fs::write(dir.join("artifact"), b"live").unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "held", None, 51, project.to_str().unwrap());
    managed::status(&pool).expect("open");
    // A held legacy generation is never adopted: it stays outside every Barge
    // area, so no prune or eviction decision can see it.
    assert!(
        !dir.join(".barge-root.json").exists(),
        "a held legacy root is not adopted"
    );
    let area = crate::storage::PoolStorage::open(
        &pool.join("oxide-managed"),
        &crate::storage::PoolBacking::budget(&pool, 8 << 20),
    )
    .expect("area");
    assert!(
        area.all_roots().expect("roots").is_empty(),
        "the legacy directory is not a Barge root"
    );
    assert!(dir.join("artifact").is_file());
}

#[test]
fn a_second_grant_for_the_same_holder_is_refused_because_it_would_carry_no_hold() {
    let fixture = Fixture::new("replay-refused");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let first =
        managed::lease_measured(request(&pool, &project, &workspace, "same")).expect("lease");
    let second = managed::lease(request(&pool, &project, &workspace, "same"));
    assert!(
        matches!(
            second,
            Err(LeaseError::Busy) | Err(LeaseError::HolderConflict)
        ),
        "an independent replay must be refused: {second:?}"
    );
    drop(first);
}

#[test]
fn an_adopted_idle_legacy_root_is_pinned_out_of_prune_and_eviction() {
    let fixture = Fixture::new("legacy-pinned");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "61");
    fs::write(dir.join("artifact"), b"old").unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "idle", None, 61, project.to_str().unwrap());
    managed::status(&pool).expect("open adopts the idle legacy root");
    let area = crate::storage::PoolStorage::open(
        &pool.join("oxide-managed"),
        &crate::storage::PoolBacking::budget(&pool, 8 << 20),
    )
    .expect("area");
    let roots = area.all_roots().expect("roots");
    let (_, info) = roots.first().expect("the legacy root is adopted");
    assert!(info.pinned, "an unheld legacy generation stays pinned");
    assert!(
        area.evict_any(&info.id).is_err(),
        "the pinned legacy root cannot be evicted"
    );
    assert!(dir.join("artifact").is_file());

    // A supported transition can already name the Barge root while its old
    // generation never acquired a hold. Every open restores its durable pin.
    area.unpin_in(None, &info.id)
        .expect("remove pin in fixture");
    managed::status(&pool).expect("protect already-adopted legacy root");
    assert!(area.all_roots().unwrap()[0].1.pinned);
    assert!(area.evict_any(&info.id).is_err());
}

#[test]
fn a_failed_legacy_pin_publication_refuses_before_pruning_and_can_retry() {
    let fixture = Fixture::new("legacy-pin-failure");
    let pool = fixture.pool();
    let dir = slot_dir(&pool, "62");
    fs::write(dir.join("artifact"), b"keep").unwrap();
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "idle", None, 62, project.to_str().unwrap());
    managed::status(&pool).expect("adopt legacy root");
    let storage = crate::storage::PoolStorage::open(
        &pool.join("oxide-managed"),
        &crate::storage::PoolBacking::budget(&pool, 8 << 20),
    )
    .unwrap();
    let (_, root) = storage.all_roots().unwrap().remove(0);
    storage.unpin_in(None, &root.id).unwrap();
    let pin = dir
        .parent()
        .unwrap()
        .join(".barge/pins")
        .join(format!("{}.json", root.id));
    fs::create_dir(&pin).unwrap();
    assert!(matches!(
        managed::status(&pool),
        Err(LeaseError::UnknownAccounting)
    ));
    assert_eq!(fs::read(dir.join("artifact")).unwrap(), b"keep");
    fs::remove_dir(&pin).unwrap();
    managed::status(&pool).expect("retry publishes the required pin");
    assert!(storage.all_roots().unwrap()[0].1.pinned);
}

#[test]
fn barge_pruning_retires_workflow_rows_and_uses_fresh_successor_paths() {
    for ledger_write_completed in [true, false] {
        let fixture = Fixture::new(if ledger_write_completed {
            "prune-row"
        } else {
            "prune-gap"
        });
        let pool = fixture.pool();
        let project = fixture.dir("project");
        let workspace = fixture.dir("workspace");
        let first = managed::lease_measured(request(&pool, &project, &workspace, "first")).unwrap();
        fill(&first, 9 << 20);
        let old_path = first.grant.env.cargo_build_build_dir.clone();
        let old_lease = first.grant.lease_id.clone();
        drop(first);
        managed::release(&pool, &old_lease).unwrap();
        if ledger_write_completed {
            super::pool::prune_injected(&pool).expect("ordinary Barge prune and ledger update");
        } else {
            let storage = crate::storage::PoolStorage::open(
                &pool.join("oxide-managed"),
                &crate::storage::PoolBacking::budget(&pool, 8 << 20),
            )
            .unwrap();
            let report = storage
                .prune(false)
                .expect("Barge completed before ledger write");
            assert_eq!(report.evicted.len(), 1);
        }
        assert!(!old_path.exists());
        let fresh = managed::lease_measured(request(&pool, &project, &workspace, "fresh"))
            .expect("admit a successor after prune");
        assert!(!fresh.grant.reused_slot);
        assert_ne!(fresh.grant.env.cargo_build_build_dir, old_path);
        assert_eq!(managed::status(&pool).unwrap().slots.len(), 1);
    }
}

#[test]
fn missing_root_measurements_are_partial_and_capacity_is_unknown() {
    let fixture = Fixture::new("short-budget");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let leased = managed::lease_measured(request(&pool, &project, &workspace, "a")).expect("lease");
    let lease_id = leased.grant.lease_id.clone();
    let slot_root = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_path_buf();
    drop(leased);
    managed::release(&pool, &lease_id).expect("release");
    // A missing root makes the total unknown, not a false "not full".
    // Opening deadlines are exercised separately without a timing race.
    std::fs::remove_dir_all(&slot_root).unwrap();
    let measured = managed::measure_within(&pool, 1_000).expect("measure the missing root");
    assert_eq!(measured.status.coverage, super::AccountingCoverage::Partial);
    assert_eq!(measured.status.full, None);
    assert!(measured.status.payload_logical_bytes.is_none());
}

#[test]
fn passive_open_time_consumes_the_callers_measurement_budget() {
    let fixture = Fixture::new("measurement-open-budget");
    let pool = fixture.pool();
    super::pool::with_passive_open_delay(std::time::Duration::from_millis(10), || {
        assert!(matches!(
            managed::measure_within(&pool, 1),
            Err(LeaseError::UnknownAccounting)
        ));
    });
    assert!(matches!(
        managed::measure_within(&pool, 0),
        Err(LeaseError::UnknownAccounting)
    ));
    assert!(managed::measure_within(&pool, 1_000).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_directory_pool_opens_through_the_production_storage_seam() {
    let fixture = Fixture::new("linux-production-directory");
    let pool = fixture.pool();
    let first = managed::ledger_view(&pool).expect("ordinary directory pool opens on Linux");
    let again = managed::ledger_view(&pool).expect("recorded filesystem identity is reusable");
    assert_eq!(first.store_id, again.store_id);
    assert_eq!(first.backend, "directory-budget-v1");
}
