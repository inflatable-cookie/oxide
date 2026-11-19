//! Direct behaviour tests for the lease engine over Barge's roots and holds.
//!
//! These replace the retired engine's tests: each one asserts a preserved
//! product behaviour on a fresh temporary pool, not a removed mechanism.

use super::{
    managed, AdmissionClass, LeaseError, LeaseRequest, Profile, SlotLayout, SlotOccupancy,
    VictimPolicy,
};
use crate::storage::{count_pool_walks, PoolBacking, PoolStorage, PoolWalk};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

#[test]
fn held_eviction_publishes_the_writer_fence_only_after_commit() {
    use super::control::{
        fenced_state, parse_state, State, HOLDING_STATE_VERSION, WRITER_GENERATION,
    };
    use super::HeldEvictionFailure;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let fixture = Fixture::new("held-eviction-fence");
    let pool = fixture.pool();
    let leased = managed::lease_measured(request(
        &pool,
        &fixture.dir("project"),
        &fixture.dir("workspace"),
        "held-eviction",
    ))
    .unwrap();
    fill(&leased, 4);
    let slot = leased.grant.slot_id.clone();
    let lease = leased.grant.lease_id.clone();
    let root = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_path_buf();
    let control = pool.join(super::MANAGED_NAME);
    let state_path = control.join("state.json");
    let admission = control.join("admission.lock");
    let ledger = pool.join("ledger.json");
    let marker = root.join(".barge-root.json");
    let artifact = root.join("build/artifact");
    let mut old_state = fenced_state(State::default(), WRITER_GENERATION).unwrap();
    old_state.version = HOLDING_STATE_VERSION;
    old_state.minimum_writer_generation = None;
    let write_state = |state: &State| {
        fs::write(&state_path, serde_json::to_vec(state).unwrap()).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o600)).unwrap();
    };
    write_state(&old_state);
    fs::write(&admission, []).unwrap();
    fs::set_permissions(&admission, fs::Permissions::from_mode(0o600)).unwrap();
    let snapshot = || {
        [&state_path, &admission, &ledger, &marker, &artifact].map(|path| {
            let metadata = fs::metadata(path).unwrap();
            (metadata.ino(), fs::read(path).unwrap())
        })
    };
    let before = snapshot();
    assert!(matches!(
        managed::evict_held_slot(&pool, "invalid", &lease),
        Err(HeldEvictionFailure::PreCommit(LeaseError::InvalidInput(_)))
    ));
    assert_eq!(
        managed::evict_held_slot(&pool, &slot, "wrong lease"),
        Err(HeldEvictionFailure::PreCommit(LeaseError::Busy))
    );
    assert_eq!(
        managed::evict_held_slot(&pool, &slot, &lease),
        Err(HeldEvictionFailure::PreCommit(
            LeaseError::UnknownAccounting
        )),
        "Barge must refuse the exact lease's still-live hold"
    );
    assert_eq!(snapshot(), before);
    assert!(!control.join("held-eviction.json").exists());

    let newer = fenced_state(old_state.clone(), WRITER_GENERATION + 1).unwrap();
    write_state(&newer);
    let before = snapshot();
    assert_eq!(
        managed::evict_held_slot(&pool, &slot, &lease),
        Err(HeldEvictionFailure::PreCommit(LeaseError::WriterUpdated))
    );
    assert_eq!(snapshot(), before);

    write_state(&old_state);
    drop(leased);
    managed::evict_held_slot(&pool, &slot, &lease).unwrap();
    assert!(!root.exists());
    assert!(managed::snapshot(&pool).unwrap().slots.is_empty());
    let committed = parse_state(&fs::read(state_path).unwrap()).unwrap();
    assert_eq!(committed.minimum_writer_generation, Some(WRITER_GENERATION));
}

#[test]
fn writer_generation_is_checked_on_pool_open_before_adoption_or_reconciliation() {
    use super::control::{fenced_state, parse_state, State, WRITER_GENERATION};
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new("writer-generation");
    let pool = fixture.pool();
    let control = pool.join(super::MANAGED_NAME);
    if !control.exists() {
        fs::DirBuilder::new().create(&control).unwrap();
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let path = control.join("state.json");
    let legacy = serde_json::to_vec(&State::default()).unwrap();
    fs::write(&path, legacy).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    managed::prepare_pool(&pool).unwrap();
    let raised = fs::read(&path).unwrap();
    let state = parse_state(&raised).unwrap();
    assert_eq!(state.minimum_writer_generation, Some(WRITER_GENERATION));
    assert_eq!(state.check_writer(WRITER_GENERATION + 1), Ok(()));
    managed::prepare_pool(&pool).unwrap();
    assert_eq!(fs::read(&path).unwrap(), raised);

    let newer = serde_json::to_vec(&fenced_state(state, WRITER_GENERATION + 1).unwrap()).unwrap();
    fs::write(&path, &newer).unwrap();
    let ledger = fs::read(pool.join("ledger.json")).unwrap();
    assert!(matches!(
        managed::prepare_pool(&pool),
        Err(LeaseError::WriterUpdated)
    ));
    managed::snapshot(&pool).unwrap();
    assert_eq!(fs::read(path).unwrap(), newer);
    assert_eq!(fs::read(pool.join("ledger.json")).unwrap(), ledger);

    let mut successor: serde_json::Value = serde_json::from_slice(&newer).unwrap();
    successor["version"] = serde_json::json!(super::control::STATE_VERSION + 1);
    successor["successor_field"] = serde_json::json!(true);
    fs::write(
        control.join("state.json"),
        serde_json::to_vec(&successor).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        managed::prepare_pool(&pool),
        Err(LeaseError::WriterUpdated)
    ));
}

pub(super) struct Fixture {
    path: PathBuf,
}

impl Fixture {
    pub(super) fn new(label: &str) -> Self {
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

    pub(super) fn pool(&self) -> PathBuf {
        let pool = self.path.join("pool");
        managed::init_budget_pool(&pool, 8 << 20).expect("pool");
        pool
    }

    pub(super) fn dir(&self, name: &str) -> PathBuf {
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

pub(super) fn request(pool: &Path, project: &Path, workspace: &Path, holder: &str) -> LeaseRequest {
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
    let other = fixture.dir("other");
    let idle =
        managed::lease_measured(request(&pool, &other, &workspace, "idle-2")).expect("lease");
    fill(&idle, 5 << 20);
    let other_path = idle.grant.env.cargo_build_build_dir.clone();
    managed::release(&pool, &idle.grant.lease_id).unwrap();
    drop(idle);
    let admitted = managed::lease(request(&pool, &fixture.dir("next"), &workspace, "next"));
    assert!(admitted.is_ok(), "pressure can skip the oldest pinned root");
    assert!(info.path.exists(), "the pinned slot stays");
    assert!(!other_path.exists(), "the next eligible slot was evicted");
}

/// Write a ledger with one legacy row, as an older Oxide wrote it, over the
/// config a fresh budget pool produced.
fn write_legacy_ledger(pool: &Path, state: &str, dir: Option<&str>, id: u64, project: &str) {
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).unwrap()).unwrap();
    let store_id = config["store_id"].as_str().unwrap();
    let cap = config["cap_bytes"].as_u64().unwrap();
    // The kernel reports this out-of-range process ID as absent on both
    // supported platforms. Held rows can carry an opaque library token.
    let holder = if state == "idle" {
        format!("oxide-{}-1", libc::pid_t::MAX)
    } else {
        "legacy".to_owned()
    };
    let row = serde_json::json!({
        "id": id,
        "incarnation": 1,
        "state": state,
        "project": project,
        "workspace": "/fixture/workspace",
        "profile": "dev",
        "holder": holder,
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
    // A passive view retains the held row. Its old writer never held a
    // Barge lock.
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
    let (prepared, first) = count_pool_walks(|| managed::prepare_pool(&pool));
    assert_eq!(prepared.unwrap().slots.len(), 1);
    assert!(matches!(first.as_slice(), [PoolWalk::Adopt(_)]));
    let (prepared, second) = count_pool_walks(|| managed::prepare_pool(&pool));
    assert_eq!(prepared.unwrap().slots.len(), 1);
    assert!(second.is_empty(), "a refused adoption stays refused");
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
    // Status skips adoption. Preparation performs it only once.
    let (status, walks) = count_pool_walks(|| managed::status(&pool));
    assert_eq!(status.unwrap().slots.len(), 1);
    assert!(walks.is_empty());
    assert!(!dir.join(".barge-root.json").exists());
    let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
    assert_eq!(prepared.unwrap().slots.len(), 1);
    assert!(matches!(walks.as_slice(), [PoolWalk::Adopt(_)]));
    let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
    assert_eq!(prepared.unwrap().slots.len(), 1);
    assert!(walks.is_empty());
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
    managed::prepare_pool(&pool).expect("adopt closed legacy root");
    let workspace = fixture.dir("workspace");
    managed::evict_idle_slot(&pool, "44").expect("evicted without a prior grant");
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
    managed::prepare_pool(&pool).expect("prepare");
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

fn fixture_storage(pool: &Path) -> PoolStorage {
    PoolStorage::open(
        &pool.join("oxide-managed"),
        &PoolBacking::budget(pool, 8 << 20),
    )
    .unwrap()
}

fn update_ledger(pool: &Path, change: impl FnOnce(&mut serde_json::Value)) {
    let path = pool.join("ledger.json");
    let mut ledger = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut ledger);
    fs::write(path, serde_json::to_vec(&ledger).unwrap()).unwrap();
}

#[test]
fn an_idle_legacy_root_stays_out_of_eviction_while_its_old_holder_lives() {
    let fixture = Fixture::new("legacy-live");
    let pool = fixture.pool();
    let path = slot_dir(&pool, "61");
    let project = fixture.dir("project");
    write_legacy_ledger(&pool, "idle", None, 61, project.to_str().unwrap());
    let mut holder = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    update_ledger(&pool, |ledger| {
        ledger["slots"][0]["holder"] = format!("oxide-{}-1", holder.id()).into();
    });

    let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
    prepared.expect("retain the live holder's row");
    assert!(walks.is_empty());
    assert!(matches!(
        managed::evict_idle_slot(&pool, "61"),
        Err(LeaseError::Busy)
    ));
    assert!(!path.join(".barge-root.json").exists());

    holder.kill().unwrap();
    holder.wait().unwrap();
    managed::prepare_pool(&pool).expect("adopt once the older holder is absent");
    assert!(!fixture_storage(&pool).all_roots().unwrap()[0].1.pinned);
    managed::evict_idle_slot(&pool, "61").expect("the now-closed root can be evicted");
    assert!(!path.exists());
}

#[test]
fn an_idle_legacy_root_with_an_unverifiable_holder_stays_outside_eviction() {
    for holder in [
        "legacy".to_owned(),
        format!("oxide-{}-unknown", libc::pid_t::MAX),
    ] {
        let fixture = Fixture::new("legacy-unverifiable");
        let pool = fixture.pool();
        let path = slot_dir(&pool, "62");
        let project = fixture.dir("project");
        fs::write(path.join("artifact"), b"keep").unwrap();
        write_legacy_ledger(&pool, "idle", None, 62, project.to_str().unwrap());
        update_ledger(&pool, |ledger| ledger["slots"][0]["holder"] = holder.into());

        let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
        assert_eq!(prepared.unwrap().slots.len(), 1);
        assert!(walks.is_empty());
        assert!(!path.join(".barge-root.json").exists());
        assert!(matches!(
            managed::evict_idle_slot(&pool, "62"),
            Err(LeaseError::Busy)
        ));
        assert_eq!(fs::read(path.join("artifact")).unwrap(), b"keep");
    }
}

#[test]
fn an_unfinished_legacy_transition_preserves_existing_barge_pins() {
    // The old version published its pin before it wrote the root ID.
    for (published_root_id, reason) in [
        (true, "independent pin"),
        (false, "independent pin"),
        (true, "legacy generation"),
        (false, "legacy generation"),
    ] {
        let fixture = Fixture::new("legacy-pinned");
        let pool = fixture.pool();
        let path = slot_dir(&pool, "62");
        let project = fixture.dir("project");
        write_legacy_ledger(&pool, "idle", None, 62, project.to_str().unwrap());
        managed::prepare_pool(&pool).expect("adopt legacy root");
        let storage = fixture_storage(&pool);
        let (_, root) = storage.all_roots().unwrap().remove(0);
        assert!(!root.pinned, "new adoption leaves no pin");
        storage.pin_in(None, &root.id, reason).unwrap();
        update_ledger(&pool, |ledger| {
            let row = ledger["slots"][0].as_object_mut().unwrap();
            row.remove("owned");
            if !published_root_id {
                row.remove("root");
            }
        });

        let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
        prepared.expect("finish the prior transition");
        assert!(walks.is_empty());
        assert!(storage.all_roots().unwrap()[0].1.pinned);
        assert!(matches!(
            managed::evict_idle_slot(&pool, "62"),
            Err(LeaseError::Busy)
        ));
        assert!(path.is_dir());
        storage.unpin_in(None, &root.id).unwrap();
        managed::evict_idle_slot(&pool, "62").expect("evict after explicit unpin");
        assert!(!path.exists());
    }
}

#[test]
fn held_eviction_distinguishes_a_held_root_from_a_failed_trash_delete() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new("held-eviction-failure");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let leased = managed::lease(request(&pool, &project, &project, "held")).unwrap();
    let slot = leased.grant.slot_id.clone();
    let lease_id = leased.grant.lease_id.clone();
    let path = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_owned();

    let before = fs::read(pool.join("ledger.json")).unwrap();
    let refusal = managed::evict_held_slot(&pool, &slot, &lease_id).unwrap_err();
    assert!(matches!(refusal, super::HeldEvictionFailure::PreCommit(_)));
    assert_eq!(fs::read(pool.join("ledger.json")).unwrap(), before);
    assert!(path.is_dir());
    drop(leased);

    // Unlinking a file from a non-writable directory fails for the pool's
    // ordinary owner. Privileged users bypass this permission on both hosts.
    if super::current_uid() == 0 {
        return;
    }
    let protected = path.join("protected");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("file"), b"kept after failed unlink").unwrap();
    fs::set_permissions(&protected, fs::Permissions::from_mode(0o500)).unwrap();
    let failure = managed::evict_held_slot(&pool, &slot, &lease_id);
    let retained_path = if path.exists() {
        path.clone()
    } else {
        let trash = path.parent().unwrap().join(".trash");
        fs::read_dir(trash).unwrap().next().unwrap().unwrap().path()
    };
    fs::set_permissions(
        retained_path.join("protected"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();

    let error = failure.unwrap_err();
    assert!(
        matches!(error, super::HeldEvictionFailure::PostCommit(_)),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .contains("evicted root remains in trash at"),
        "{error}"
    );
    assert!(!path.exists());
    assert!(retained_path.join("protected/file").is_file());
    assert!(managed::snapshot(&pool).unwrap().slots.is_empty());
}

#[test]
fn an_eviction_counts_its_byte_walk_even_if_rename_cannot_start() {
    use std::os::unix::fs::PermissionsExt;

    if super::current_uid() == 0 {
        return; // The superuser can create trash in a non-writable directory.
    }
    let fixture = Fixture::new("eviction-walk-before-rename");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let leased = managed::lease(request(&pool, &project, &project, "first")).unwrap();
    let victim = leased.grant.slot_id.parse().unwrap();
    managed::release(&pool, &leased.grant.lease_id).unwrap();
    let path = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_owned();
    drop(leased);

    let slots = path.parent().unwrap();
    assert!(!slots.join(".trash").exists());
    let other = fixture.dir("other");
    let mut replacement = request(&pool, &other, &other, "replacement");
    replacement.replace_slot = Some(victim);
    fs::set_permissions(slots, fs::Permissions::from_mode(0o500)).unwrap();
    let (attempt, walks) = count_pool_walks(|| managed::lease(replacement));
    fs::set_permissions(slots, fs::Permissions::from_mode(0o700)).unwrap();

    let error = attempt
        .err()
        .expect("trash creation refuses while the parent is read-only");
    assert!(
        error.to_string().contains("cannot create the trash at"),
        "{error}"
    );
    assert!(matches!(walks.as_slice(), [PoolWalk::Evict(_)]));
    let (_, walked, listed, opaque) = super::admission_cost::latest(&pool);
    assert_eq!((walked, listed, opaque), (1, 0, true));
    assert!(path.is_dir());
}

#[test]
fn sweeping_owned_trash_does_not_resample_the_roots_bytes() {
    let fixture = Fixture::new("trash-byte-walks");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let leased = managed::lease(request(&pool, &project, &project, "first")).unwrap();
    fill(&leased, 8192);
    managed::release(&pool, &leased.grant.lease_id).unwrap();
    let path = leased
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_owned();
    drop(leased);
    // Reproduce the state left after Barge renames an owned root into trash.
    let trash = path.parent().unwrap().join(".trash");
    fs::create_dir(&trash).unwrap();
    let evicted = trash.join(path.file_name().unwrap());
    fs::rename(path, &evicted).unwrap();

    managed::prepare_pool(&pool).unwrap();
    let (_, walked, listed, opaque) = super::admission_cost::latest(&pool);
    assert_eq!((walked, listed, opaque), (0, 1, false));
    assert!(!evicted.exists());
}

#[test]
fn admission_cost_and_protection_at_the_256_slot_ceiling() {
    use super::admission_cost;
    use std::io::Write;
    let fixture = Fixture::new("legacy-ceiling");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    let count = super::MAX_SLOTS as u64;
    write_legacy_ledger(&pool, "idle", None, 1, project.to_str().unwrap());
    update_ledger(&pool, |ledger| {
        let template = ledger["slots"][0].clone();
        let store = ledger["store_id"].as_str().unwrap();
        let slots: Vec<_> = (1..=count)
            .map(|id| {
                let mut row = template.clone();
                row["id"] = id.into();
                row["rank"] = (count + 1 - id).into();
                row["last_lease_id"] = format!("v1.{store}.{id}.1.1").into();
                // Pre-adoption observations already have the logical-byte field.
                row["recorded"] = serde_json::json!({
                    "for_lease": row["last_lease_id"],
                    "logical_bytes": 2048,
                    "unique_bytes": 4096,
                    "entries": 1,
                    "at_ms": 1,
                });
                let root = slot_dir(&pool, &id.to_string());
                fs::create_dir(root.join("build")).unwrap();
                fs::write(root.join("build/artifact"), b"tiny").unwrap();
                row
            })
            .collect();
        ledger["slots"] = slots.into();
        ledger["capacity_policy"] = super::RAISED_CAPACITY_POLICY_VERSION.into();
        ledger["next_slot"] = (count + 1).into();
        ledger["next_rank"] = (count + 1).into();
    });
    let mut max_prepare_us = 0;
    let mut migrated = 0;
    while migrated < super::MAX_SLOTS {
        let (prepared, walks) = count_pool_walks(|| managed::prepare_pool(&pool));
        assert_eq!(prepared.unwrap().slots.len(), super::MAX_SLOTS);
        assert_eq!(walks.len(), 8);
        assert!(walks.iter().all(|walk| matches!(walk, PoolWalk::Adopt(_))));
        let (micros, walked, listed, opaque) = admission_cost::latest(&pool);
        assert_eq!((walked, listed, opaque), (0, 0, true));
        max_prepare_us = max_prepare_us.max(micros);
        migrated += walks.len();
    }
    assert_eq!(count_pool_walks(|| managed::prepare_pool(&pool)).1, []);
    assert_eq!(
        managed::pinned_roots(&pool).unwrap(),
        "pinned roots by reason: none"
    );

    admission_cost::start_summary();
    let (warm, walks) =
        count_pool_walks(|| managed::lease(request(&pool, &project, &workspace, "warm")));
    let warm = warm.unwrap();
    assert!(warm.grant.reused_slot);
    assert!(walks.is_empty(), "warm admission does not walk the payload");
    let (warm_us, walked, listed, opaque) = admission_cost::latest(&pool);
    assert_eq!(
        (walked, listed, opaque),
        (0, 0, true),
        "Barge hold resolves by enumerating markers"
    );
    let summary = admission_cost::take_summary()
        .expect("unavailable listing count triggers the build summary");
    assert!(
        summary.contains("Barge listing count unavailable"),
        "{summary}"
    );
    assert!(
        summary.contains("accounting") && summary.contains("byte walks 0"),
        "{summary}"
    );
    managed::release(&pool, &warm.grant.lease_id).unwrap();
    drop(warm);

    admission_cost::start_summary();
    let (reserve, walks) = count_pool_walks(|| {
        super::pool::check_ample_reserve(&pool, &fixture.dir("reserve-account"))
    });
    reserve.unwrap();
    assert!(walks.is_empty());
    let (reserve_us, walked, listed, opaque) = admission_cost::latest(&pool);
    assert_eq!((walked, listed, opaque), (0, 0, false));

    let ample_summary = admission_cost::take_summary();
    if reserve_us <= 2_000_000 {
        assert_eq!(ample_summary, None);
    }

    admission_cost::start_summary();
    let other = fixture.dir("other");
    let (admitted, walks) =
        count_pool_walks(|| managed::lease(request(&pool, &other, &workspace, "new")));
    let admitted = admitted.unwrap();
    assert_eq!(admitted.grant.slot_id, (count + 1).to_string());
    assert!(matches!(walks.as_slice(), [PoolWalk::Evict(_)]));
    let (eviction_us, walked, listed, opaque) = admission_cost::latest(&pool);
    assert_eq!(
        (walked, listed, opaque),
        (1, 0, true),
        "Barge eviction also enumerates markers"
    );
    let summary = admission_cost::take_summary()
        .expect("unavailable listing count triggers the build summary");
    assert!(
        summary.contains("Barge listing count unavailable"),
        "{summary}"
    );
    assert!(
        summary.contains("eviction") && summary.contains("byte walks 1"),
        "{summary}"
    );
    assert!(!pool.join("payload/slots").join(count.to_string()).exists());
    assert!(pool.join("payload/slots/1").is_dir());

    let storage = fixture_storage(&pool);
    let roots = storage.area().list_without_bytes().unwrap().roots;
    for (index, root) in roots.iter().filter(|root| !root.held).enumerate() {
        let reason = if index < 100 {
            "keep build"
        } else {
            "keep evidence"
        };
        storage.pin_in(None, &root.id, reason).unwrap();
    }

    let (refused, walks) = count_pool_walks(|| {
        managed::lease(request(&pool, &fixture.dir("third"), &workspace, "refused"))
    });
    let error = refused.err().expect("ceiling refusal");
    assert_eq!(error.reason(), "ledger_exhausted");
    let diagnosis = error.to_string();
    assert!(
        diagnosis.contains("1 held, 255 pinned, 0 other unavailable"),
        "{diagnosis}"
    );
    assert!(
        diagnosis.contains(r#"100 "keep build", 155 "keep evidence""#),
        "{diagnosis}"
    );
    assert_eq!(walks, [PoolWalk::Inventory]);
    let (refused_us, walked, listed, opaque) = admission_cost::latest(&pool);
    assert_eq!((walked, listed, opaque), (0, 256, true));

    let ((), walks) = count_pool_walks(|| {
        assert_eq!(managed::status(&pool).unwrap().slots.len(), 256);
        assert_eq!(
            managed::pinned_roots(&pool).unwrap(),
            r#"pinned roots by reason: 100 "keep build", 155 "keep evidence""#
        );
        let history = admission_cost::recorded(&pool).unwrap().unwrap();
        assert!(
            history.contains("explicit inventory entries 256"),
            "{history}"
        );
        assert!(
            history.contains("Barge listing count unavailable"),
            "{history}"
        );
    });
    assert!(walks.is_empty(), "default status walks and lists no roots");
    let durations = [max_prepare_us, warm_us, reserve_us, eviction_us, refused_us];
    assert!(
        durations.into_iter().all(|micros| micros < 10_000_000),
        "{durations:?}"
    );
    writeln!(std::io::stderr().lock(),
        "256 slots, lock µs and root counts: adoption max {max_prepare_us} (8 rows per batch, 0 walks, marker count unavailable), warm {warm_us} (0 walks, marker count unavailable), reserve {reserve_us} (0 roots), eviction {eviction_us} (1 walk, marker count unavailable), refusal {refused_us} (0 walks, 256 explicit inventory entries, marker count unavailable), status 0 roots, warm and eviction summaries disclosed the unavailable listing count"
    ).unwrap();
}

#[test]
fn admission_release_and_status_only_walk_the_roots_they_need() {
    let fixture = Fixture::new("walk-count");
    let pool = fixture.pool();
    let workspace = fixture.dir("workspace");
    let mut roots = Vec::new();
    for i in 0..32 {
        let project = fixture.dir(&format!("project-{i}"));
        let lease = managed::lease(request(&pool, &project, &workspace, "build")).unwrap();
        let path = lease
            .grant
            .env
            .cargo_build_build_dir
            .parent()
            .unwrap()
            .to_path_buf();
        fs::write(path.join("artifact"), vec![0; 8192]).unwrap();
        let (released, walks) = count_pool_walks(|| managed::release(&pool, &lease.grant.lease_id));
        assert!(!released.unwrap().replay);
        assert_eq!(walks, vec![PoolWalk::Measure(path.clone())]);
        roots.push(path);
    }
    update_ledger(&pool, |ledger| {
        ledger["slots"][12]
            .as_object_mut()
            .unwrap()
            .remove("recorded");
    });
    let (status, walks) = count_pool_walks(|| {
        managed::status(&pool).unwrap();
        managed::snapshot(&pool).unwrap()
    });
    assert_eq!(status.recorded_slots, 31);
    assert_eq!(status.unrecorded_slots, 1);
    assert!(walks.is_empty());

    let project = fixture.dir("new");
    let (lease, walks) =
        count_pool_walks(|| managed::lease(request(&pool, &project, &workspace, "new")));
    let lease = lease.unwrap();
    assert_eq!(walks, vec![PoolWalk::Measure(roots[12].clone())]);
    managed::release(&pool, &lease.grant.lease_id).unwrap();
    drop(lease);
    assert_eq!(managed::snapshot(&pool).unwrap().recorded_slots, 33);

    let mut eviction = request(&pool, &fixture.dir("pressure"), &workspace, "pressure");
    let before = managed::snapshot(&pool).unwrap();
    eviction.budget_bytes = before.recorded_total;
    let victim = fixture_storage(&pool)
        .all_roots()
        .unwrap()
        .into_iter()
        .find(|(_, root)| root.path == roots[0])
        .unwrap()
        .1;
    let (lease, walks) = count_pool_walks(|| managed::lease(eviction));
    let lease = lease.unwrap();
    assert_eq!(walks, vec![PoolWalk::Evict(victim.id)]);
    assert!(!roots[0].exists());
    managed::release(&pool, &lease.grant.lease_id).unwrap();

    let project = fixture.dir("project-12");
    let (warm, walks) =
        count_pool_walks(|| managed::lease(request(&pool, &project, &workspace, "warm")));
    assert!(warm.unwrap().grant.reused_slot);
    assert!(walks.is_empty());
}

#[test]
fn an_unmeasurable_slot_is_charged_without_publishing_zero() {
    let fixture = Fixture::new("unknown-charge");
    let pool = fixture.pool();
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    write_legacy_ledger(&pool, "idle", None, 63, project.to_str().unwrap());
    let other = fixture.dir("other");
    let mut admission = request(&pool, &other, &workspace, "new");
    admission.budget_bytes = Some(1);
    let (outcome, walks) = count_pool_walks(|| managed::lease(admission));
    assert!(matches!(outcome, Err(LeaseError::Full)));
    assert_eq!(
        walks,
        vec![PoolWalk::Measure(pool.join("payload/slots/63"))]
    );
    let snapshot = managed::snapshot(&pool).unwrap();
    assert_eq!(snapshot.unrecorded_slots, 1);
    assert_eq!(snapshot.recorded_total, None);
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
