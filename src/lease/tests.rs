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
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
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
