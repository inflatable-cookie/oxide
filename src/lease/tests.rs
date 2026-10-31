use super::managed;
use super::pool::{
    create_injected, evict_held_slot_injected, evict_idle_slot_injected, lease_injected,
    mark_evicting_for_test, recover_injected, release_injected, status_injected,
    with_accounting_limits,
};
use super::{
    AccountingCoverage, AccountingGapKind, AccountingScan, AdmissionClass, LeaseError,
    LeaseRequest, Profile, SlotLayout, SlotOccupancy, VictimPolicy, BUDGET_BACKEND_DIRECTORY_V1,
    MANAGED_NAME, MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_LEDGER_BYTES, MAX_REFUSAL_SLOTS,
    RAISED_CAPACITY_POLICY_VERSION,
};
use std::fs::{self, File};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        // Ordinary unit fixtures live on the recipe's ordinary test temp
        // (`TMPDIR`, on the selected volume), never inside
        // `OXIDE_TASK_SCRATCH_ROOT`: the shared proof supervisor measures that
        // root while tests run, and these fixtures deliberately hold
        // owner-unreadable directories (0100/0300/0000) to prove the repair and
        // refusal paths. A fixture the supervisor cannot walk would fault its
        // own measurement, so the unreadable windows stay outside it.
        let base = std::env::temp_dir();
        let parent = base.join("lease");
        fs::create_dir_all(&parent).expect("lease fixture parent");
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{label}-{:x}-{id:x}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .expect("private fixture");
                    return Self { path };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create lease fixture: {error}"),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir(&path).expect("fixture dir");
        path
    }
}

/// Give the owner back read and search on every directory below `path`, so a
/// fixture whose subject legitimately kept a search-only directory can still be
/// removed. Only this fixture's own tree is touched.
fn make_removable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if !metadata.file_type().is_dir() {
        return;
    }
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            make_removable(&entry.path());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        if self.path.exists() {
            make_removable(&self.path);
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// [`lease_publishing`] for the public `managed::lease` entry point.
fn lease_after_publishing(
    pool: &Path,
    request: LeaseRequest,
) -> Result<super::LeaseGrant, LeaseError> {
    match managed::lease(request.clone()) {
        Ok(grant) => Ok(grant),
        Err(error @ (LeaseError::Full | LeaseError::CapacityCharged { .. })) => {
            let mut last = Err(error);
            for _ in 0..4 {
                let removed = managed::drain_holding(pool, None)
                    .map(|pass| pass.gone)
                    .unwrap_or(false);
                if !removed {
                    break;
                }
                last = managed::lease(request.clone());
                if last.is_ok() {
                    break;
                }
            }
            last
        }
        Err(other) => Err(other),
    }
}

/// The product's two-step byte-pressure admission (oxide#190).
///
/// Every held tree stays charged in the capacity decision until it is
/// positively removed, so an admission that evicts cannot admit in the same
/// attempt: it refuses with a capacity result while its trees are still held.
/// The command then deletes one pending tree outside the pool lock and retries
/// exactly once. This helper performs that shape for a test that wants the
/// end state, and returns the first refusal so a caller can assert it happened.
fn lease_publishing(
    pool: &Path,
    request: LeaseRequest,
) -> (Result<super::LeaseGrant, LeaseError>, Option<LeaseError>) {
    match lease_injected(request.clone()) {
        Ok(grant) => (Ok(grant), None),
        Err(error @ (LeaseError::Full | LeaseError::CapacityCharged { .. })) => {
            // The product publishes one tree outside the lock and re-evaluates,
            // up to a small bound; a pass that removes nothing ends it.
            let mut last = Err(error.clone());
            for _ in 0..4 {
                let removed = managed::drain_holding(pool, None)
                    .map(|pass| pass.gone)
                    .unwrap_or(false);
                if !removed {
                    break;
                }
                last = lease_injected(request.clone());
                if last.is_ok() {
                    break;
                }
            }
            (last, Some(error))
        }
        Err(other) => (Err(other.clone()), Some(other)),
    }
}

fn request(pool: &Path, project: &Path, workspace: &Path, holder: &str) -> LeaseRequest {
    request_with_profile(pool, project, workspace, holder, Profile::Dev)
}

fn request_with_profile(
    pool: &Path,
    project: &Path,
    workspace: &Path,
    holder: &str,
    profile: Profile,
) -> LeaseRequest {
    LeaseRequest {
        pool: pool.to_path_buf(),
        project: project.to_path_buf(),
        workspace: workspace.to_path_buf(),
        profile,
        holder: holder.to_owned(),
        budget_bytes: None,
        reserve_bytes: None,
        layout: None,
        replace_slot: None,
        class: AdmissionClass::Normal,
        victims: VictimPolicy::Any,
        slice: None,
    }
}

fn unexpected(place: &'static str, entry: &str) -> LeaseError {
    LeaseError::UnexpectedEntry {
        place,
        entry: entry.to_owned(),
    }
}

/// The scan of a refusal for incomplete accounting.
fn incomplete<T: std::fmt::Debug>(result: Result<T, LeaseError>) -> AccountingScan {
    match result {
        Err(LeaseError::AccountingIncomplete(scan)) => {
            assert_eq!(
                LeaseError::AccountingIncomplete(scan.clone()).reason(),
                "unknown_accounting"
            );
            *scan
        }
        other => panic!("expected incomplete accounting, got {other:?}"),
    }
}

fn gap_kinds(scan: &AccountingScan) -> Vec<AccountingGapKind> {
    scan.gaps.iter().map(|gap| gap.kind).collect()
}

/// `count` empty files directly under `dir`.
fn fill(dir: &Path, count: usize) {
    for index in 0..count {
        File::create(dir.join(format!("f{index}"))).expect("fixture file");
    }
}

fn lease_sequence(lease_id: &str) -> u64 {
    lease_id
        .rsplit('.')
        .next()
        .expect("lease sequence")
        .parse()
        .expect("numeric lease sequence")
}

fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(directory).expect("snapshot directory") {
            let path = entry.expect("snapshot entry").path();
            let metadata = fs::symlink_metadata(&path).expect("snapshot metadata");
            if metadata.is_dir() {
                visit(root, &path, files);
            } else if metadata.is_file() {
                files.push((
                    path.strip_prefix(root)
                        .expect("relative snapshot path")
                        .to_path_buf(),
                    fs::read(path).expect("snapshot file bytes"),
                ));
            } else {
                panic!("unexpected fixture entry type: {path:?}");
            }
        }
    }

    let mut files = Vec::new();
    visit(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn duplicate_holder_reconciles_same_lease() {
    let fx = Fixture::new("dup");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let first = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let second = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("replay");
    assert_eq!(first.lease_id, second.lease_id);
    assert_eq!(first.slot_id, second.slot_id);
    assert_eq!(first.env.cargo_target_dir, workspace.join("target"));
    assert!(first
        .env
        .cargo_build_build_dir
        .starts_with(pool.join("payload").join("slots")));
}

#[test]
fn lost_reply_preserves_held_lease() {
    let fx = Fixture::new("lost");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let first = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    drop(first.clone());
    let again = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("reopen");
    assert_eq!(first.lease_id, again.lease_id);
}

#[test]
fn other_holder_is_busy_and_release_is_idempotent() {
    let fx = Fixture::new("busy");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let granted = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let busy = lease_injected(request(&pool, &project, &workspace, "holder-b"));
    assert!(matches!(busy, Err(LeaseError::Busy)));
    let released = release_injected(&pool, &granted.lease_id).expect("release");
    assert_eq!(released.state, SlotOccupancy::Idle);
    assert!(!released.replay);
    let replay = release_injected(&pool, &granted.lease_id).expect("replay");
    assert!(replay.replay);
    let recovered = recover_injected(&pool, &granted.lease_id).expect("recover replay");
    assert!(recovered.replay);
}

#[test]
fn recover_closes_held_occupancy() {
    let fx = Fixture::new("recover");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let granted = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let receipt = recover_injected(&pool, &granted.lease_id).expect("recover");
    assert!(!receipt.replay);
    assert_eq!(receipt.state, SlotOccupancy::Idle);
    let other = lease_injected(request(&pool, &project, &workspace, "holder-b")).expect("reuse");
    assert_eq!(other.slot_id, granted.slot_id);
    assert_ne!(other.lease_id, granted.lease_id);
}

#[test]
fn holder_conflict_and_unknown_lease() {
    let fx = Fixture::new("conflict");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let other = fx.dir("other");
    let workspace = fx.dir("workspace");
    lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let conflict = lease_injected(request(&pool, &other, &workspace, "holder-a"));
    assert!(matches!(conflict, Err(LeaseError::HolderConflict)));
    assert!(matches!(
        release_injected(&pool, "v1.deadbeef.1.1.1"),
        Err(LeaseError::NotFound)
    ));
}

#[test]
fn idle_slot_keeps_workspace_observation_on_reuse() {
    let fx = Fixture::new("idle");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let ws1 = fx.dir("ws1");
    let ws2 = fx.dir("ws2");
    let first = lease_injected(request(&pool, &project, &ws1, "holder-a")).expect("lease");
    File::create(first.env.cargo_build_build_dir.join("keep"))
        .unwrap()
        .write_all(b"cached")
        .unwrap();
    release_injected(&pool, &first.lease_id).expect("release");
    let second = lease_injected(request(&pool, &project, &ws2, "holder-b")).expect("reuse");
    assert_eq!(second.slot_id, first.slot_id);
    assert!(second.reused_slot);
    assert_eq!(second.workspace, ws2);
    assert_eq!(
        fs::read(second.env.cargo_build_build_dir.join("keep")).unwrap(),
        b"cached"
    );
    let report = status_injected(&pool).expect("status");
    let reused_size = report
        .slots
        .iter()
        .find(|slot| slot.slot_id == first.slot_id)
        .and_then(|slot| slot.unique_allocated_bytes)
        .expect("complete warm-slot allocation");
    assert!(reused_size < 512 * 1024);
    assert_eq!(report.slots[0].workspace, ws2);
    assert_eq!(report.coverage, AccountingCoverage::Complete);
}

#[test]
fn compatible_idle_slot_is_reused_without_half_budget_recycling() {
    let fx = Fixture::new("reuse-compatible");
    let pool = fx.dir("pool");
    let cap = 8 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let project = fx.dir("project");
    let first_workspace = fx.dir("first-workspace");
    let replacement_workspace = fx.dir("replacement-workspace");
    let first = lease_injected(request(&pool, &project, &first_workspace, "holder-a"))
        .expect("first lease");
    assert!(!first.reused_slot);
    let payload = first.env.cargo_build_build_dir.join("generated.bin");
    fs::write(&payload, vec![b'x'; 5 * 1024 * 1024]).expect("allocated payload");
    release_injected(&pool, &first.lease_id).expect("release first slot");

    let idle_status = status_injected(&pool).expect("inventory idle slot");
    let allocated = idle_status
        .slots
        .iter()
        .find(|slot| slot.slot_id == first.slot_id)
        .and_then(|slot| slot.unique_allocated_bytes)
        .expect("complete allocated-byte inventory");
    assert!(allocated >= cap.div_ceil(2));

    // Byte pressure is the only removal trigger; a compatible idle slot above
    // the old half-budget threshold stays warm.
    let reused = lease_injected(request(&pool, &project, &replacement_workspace, "holder-b"))
        .expect("reuse compatible idle slot");
    assert!(reused.reused_slot);
    assert_eq!(reused.slot_id, first.slot_id);
    assert_eq!(
        fs::read(&payload).expect("payload remains"),
        vec![b'x'; 5 * 1024 * 1024]
    );
    assert!(lease_sequence(&reused.lease_id) > lease_sequence(&first.lease_id));

    let status = status_injected(&pool).expect("status after reuse");
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].workspace, replacement_workspace);
    assert_eq!(status.slots[0].rank, 3);
}

#[test]
fn byte_pressure_evicts_the_rank_lru_idle_whole_slot_only() {
    let fx = Fixture::new("byte-lru");
    let pool = fx.dir("pool");
    let cap = 4 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");

    // A held slot first, so it is the oldest rank and would be the first victim
    // if held slots were eligible.
    let c = lease_injected(request(&pool, &fx.dir("project-c"), &workspace, "c")).expect("c");
    let c_marker = c.env.cargo_build_build_dir.join("blob");
    fs::write(&c_marker, vec![b'c'; 512 * 1024]).expect("c payload");
    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    let a_marker = a.env.cargo_build_build_dir.join("blob");
    fs::write(&a_marker, vec![b'a'; 3 * 1024 * 1024]).expect("a payload");
    release_injected(&pool, &a.lease_id).expect("release a");
    let b = lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b")).expect("b");
    let b_marker = b.env.cargo_build_build_dir.join("blob");
    fs::write(&b_marker, vec![b'b'; 1024 * 1024]).expect("b payload");
    release_injected(&pool, &b.lease_id).expect("release b");
    // c stays held: a held slot is never an eviction candidate.

    // Every held tree stays charged until it is positively removed, so byte
    // pressure retires every eligible idle slot in rank order before the
    // attempt refuses; the publishing pass then frees one tree and the retry
    // admits. `a` is the oldest idle slot, so it is retired first, and the held
    // slot is never a victim. The single-decision LRU order is asserted where
    // one decision is observable (`evict_one_idle_with` in the pool tests).
    let d = lease_publishing(&pool, request(&pool, &fx.dir("project-d"), &workspace, "d"))
        .0
        .expect("byte pressure retires idle slots and admits after a published pass");
    assert!(!d.reused_slot);
    assert!(!a_marker.exists(), "the rank-LRU idle slot was not evicted");
    assert_eq!(
        fs::read(&c_marker).expect("held slot survives"),
        vec![b'c'; 512 * 1024]
    );
    let status = status_injected(&pool).expect("status after byte eviction");
    let ids: Vec<&str> = status
        .slots
        .iter()
        .map(|slot| slot.slot_id.as_str())
        .collect();
    assert!(!ids.contains(&a.slot_id.as_str()));
    assert!(ids.contains(&c.slot_id.as_str()));
    // A newer idle slot can only be gone after the older one was.
    if !b_marker.exists() {
        assert!(
            !a_marker.exists(),
            "a newer slot was retired before the older"
        );
    }
}

#[test]
fn below_both_count_ceilings_and_without_byte_pressure_nothing_is_evicted() {
    let fx = Fixture::new("no-churn");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let mut granted = Vec::new();
    for index in 0..4 {
        let lease = lease_injected(request(
            &pool,
            &fx.dir(&format!("project-{index}")),
            &workspace,
            &format!("holder-{index}"),
        ))
        .expect("new slot below every ceiling");
        if index < 3 {
            release_injected(&pool, &lease.lease_id).expect("idle slot");
        }
        granted.push(lease);
    }
    let status = status_injected(&pool).expect("status");
    assert_eq!(status.slots.len(), 4);
    for lease in &granted {
        assert!(status
            .slots
            .iter()
            .any(|slot| slot.slot_id == lease.slot_id));
    }
}

#[test]
fn pool_ceiling_replaces_the_oldest_idle_slot_and_keeps_the_count() {
    let fx = Fixture::new("pool-ceiling");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let mut first = None;
    for index in 0..super::MAX_SLOTS {
        let lease = lease_injected(request(
            &pool,
            &fx.dir(&format!("project-{index}")),
            &workspace,
            &format!("holder-{index}"),
        ))
        .expect("fill slot table");
        release_injected(&pool, &lease.lease_id).expect("idle slot");
        first.get_or_insert(lease);
    }
    let first = first.expect("first slot");
    let first_marker = first.env.cargo_build_build_dir.join("blob");
    fs::write(&first_marker, b"oldest idle slot").expect("oldest payload");

    // A full 256-slot pool's recorded status path is membership only: it
    // walks no payload and no slot (oxide#146, oxide#177).
    let (view, status_walks) = walked(&pool, || ledger_injected(&pool));
    let view = view.expect("recorded ledger view");
    assert_eq!(view.slots.len(), super::MAX_SLOTS);
    assert!(
        status_walks.is_empty(),
        "recorded status walked inventory at 256 slots: {status_walks:?}"
    );

    // Every record is current, so the 257th admission is a count replacement
    // from records only: it walks no inventory at all, and the evicted slot
    // is never walked either (oxide#146).
    let (extra, walks) = crate::inventory::walk_observer::observe(|| {
        lease_injected(request(
            &pool,
            &fx.dir("project-extra"),
            &workspace,
            "extra",
        ))
    });
    let extra = extra.expect("the 257th slot replaces the oldest idle one");
    assert!(!extra.reused_slot);
    assert!(
        walks.is_empty(),
        "the count replacement walked inventory: {walks:?}"
    );
    assert!(!first_marker.exists());

    // Releasing the new slot walks exactly that slot, never the payload.
    let (_, release_walks) = walked(&pool, || release_injected(&pool, &extra.lease_id));
    assert_eq!(release_walks.len(), 1, "release walks: {release_walks:?}");
    assert!(
        release_walks[0].starts_with("slots/"),
        "release walked more than its own slot: {release_walks:?}"
    );

    let status = status_injected(&pool).expect("status");
    assert_eq!(status.slots.len(), super::MAX_SLOTS);
    assert!(status
        .slots
        .iter()
        .all(|slot| slot.slot_id != first.slot_id));
    assert!(status
        .slots
        .iter()
        .any(|slot| slot.slot_id == extra.slot_id));
    assert!(status.slots.iter().all(|slot| slot.rank > 0));

    // The raised-ceiling fence is durable in the same ledger as the count.
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert_eq!(
        stored["capacity_policy"],
        serde_json::json!(RAISED_CAPACITY_POLICY_VERSION),
        "a 256-slot ledger must carry the raised-ceiling fence"
    );
}

#[test]
fn legacy_ledger_reads_without_migrating_and_the_first_revised_write_fences_it() {
    let fx = Fixture::new("legacy-fence");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).expect("config")).expect("json");
    let legacy = serde_json::json!({
        "version": 1,
        "store_id": config["store_id"],
        "cap_bytes": 8 * 1024 * 1024,
        "next_slot": 1,
        "next_lease": 1,
        "slots": [],
    });
    let bytes = serde_json::to_vec(&legacy).expect("legacy ledger");
    fs::write(pool.join("ledger.json"), &bytes).expect("legacy ledger file");
    status_injected(&pool).expect("legacy ledger reads for status");
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("unchanged"),
        bytes,
        "a read-only observer migrated the legacy representation"
    );

    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let granted = lease_injected(request(&pool, &project, &workspace, "holder"))
        .expect("first revised grant");
    let after_grant = fs::read(pool.join("ledger.json")).expect("fenced ledger");
    let stored: serde_json::Value = serde_json::from_slice(&after_grant).expect("json");
    assert_eq!(
        stored["capacity_policy"],
        serde_json::json!(super::CAPACITY_POLICY_VERSION)
    );
    assert_eq!(stored["next_rank"], serde_json::json!(2));
    assert_eq!(stored["slots"][0]["rank"], serde_json::json!(1));
    status_injected(&pool).expect("fenced ledger reads");
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("still fenced"),
        after_grant,
        "a read-only observer changed the fenced ledger"
    );
    release_injected(&pool, &granted.lease_id).expect("ranked release");
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert_eq!(stored["next_rank"], serde_json::json!(3));
    assert_eq!(stored["slots"][0]["rank"], serde_json::json!(2));
}

#[test]
fn rank_exhaustion_refuses_before_any_transition() {
    let fx = Fixture::new("rank-exhaustion");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let first =
        lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("held slot");
    release_injected(&pool, &first.lease_id).expect("idle slot");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    stored["next_rank"] = serde_json::json!(u64::MAX);
    let bytes = serde_json::to_vec(&stored).expect("exhausted ledger");
    fs::write(pool.join("ledger.json"), &bytes).expect("write exhausted ledger");
    assert!(matches!(
        lease_injected(request(&pool, &project, &workspace, "holder-b")),
        Err(LeaseError::LedgerExhausted)
    ));
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("unchanged"),
        bytes,
        "an exhausted rank sequence changed the pool"
    );
    let status = status_injected(&pool).expect("status still reads");
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].state, SlotOccupancy::Idle);
}

#[test]
fn held_oversized_compatible_slot_keeps_same_holder_replay_and_other_holder_busy() {
    let fx = Fixture::new("recycle-held");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let first =
        lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("held lease");
    let payload = first.env.cargo_build_build_dir.join("held-data");
    fs::write(&payload, vec![b'h'; 5 * 1024 * 1024]).expect("held payload");

    let replay = lease_injected(request(&pool, &project, &workspace, "holder-a"))
        .expect("same-holder replay");
    assert_eq!(replay.lease_id, first.lease_id);
    assert_eq!(replay.slot_id, first.slot_id);
    assert_eq!(
        replay.env.cargo_build_build_dir,
        first.env.cargo_build_build_dir
    );
    assert_eq!(
        fs::metadata(&payload).expect("payload remains").len(),
        5 * 1024 * 1024
    );
    assert!(matches!(
        lease_injected(request(&pool, &project, &workspace, "holder-b")),
        Err(LeaseError::Busy)
    ));
    assert_eq!(
        fs::metadata(&payload).expect("busy does not delete").len(),
        5 * 1024 * 1024
    );
}

#[test]
fn partial_accounting_and_unsafe_slot_root_refuse_compatible_reuse() {
    let fx = Fixture::new("recycle-unsafe");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let partial_project = fx.dir("partial-project");
    let partial = lease_injected(request(
        &pool,
        &partial_project,
        &workspace,
        "partial-holder",
    ))
    .expect("partial candidate");
    let mut nested = partial.env.cargo_build_build_dir.clone();
    for _ in 0..30 {
        nested.push("nested");
        fs::create_dir(&nested).expect("nested fixture directory");
    }
    release_injected(&pool, &partial.lease_id).expect("release partial candidate");
    // The old depth bound (24) stops this slot's walk: reuse refuses.
    let refused = with_accounting_limits(50_000, 24, 5_000, || {
        lease_injected(request(
            &pool,
            &partial_project,
            &workspace,
            "partial-retry",
        ))
    });
    let scan = incomplete(refused);
    assert_eq!(scan.coverage, AccountingCoverage::Partial);
    assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::DepthLimit]);
    assert!(pool
        .join("payload")
        .join("slots")
        .join(&partial.slot_id)
        .exists());
    // The pool bounds cover it: the same slot generation is reused.
    let reused = lease_injected(request(
        &pool,
        &partial_project,
        &workspace,
        "partial-retry",
    ))
    .expect("deep slot reused under the pool bounds");
    assert!(reused.reused_slot);
    assert_eq!(
        (reused.slot_id.as_str(), reused.incarnation.as_str()),
        (partial.slot_id.as_str(), partial.incarnation.as_str())
    );
    release_injected(&pool, &reused.lease_id).expect("release reused");

    let unsafe_pool = fx.dir("unsafe-pool");
    create_injected(&unsafe_pool, 8 * 1024 * 1024).expect("init unsafe pool");
    let unsafe_project = fx.dir("unsafe-project");
    let unsafe_slot = lease_injected(request(
        &unsafe_pool,
        &unsafe_project,
        &workspace,
        "unsafe-holder",
    ))
    .expect("unsafe candidate");
    release_injected(&unsafe_pool, &unsafe_slot.lease_id).expect("release unsafe candidate");
    let external = fx.dir("external-target");
    let sentinel = external.join("sentinel");
    fs::write(&sentinel, b"foreign target survives").expect("sentinel");
    let slot_root = unsafe_pool
        .join("payload")
        .join("slots")
        .join(&unsafe_slot.slot_id);
    fs::remove_dir_all(&slot_root).expect("remove fixture slot root");
    symlink(&external, &slot_root).expect("unsafe slot root");
    assert!(matches!(
        lease_injected(request(
            &unsafe_pool,
            &unsafe_project,
            &workspace,
            "unsafe-retry"
        )),
        Err(LeaseError::UnsafePath(_)) | Err(LeaseError::UnknownAccounting)
    ));
    assert_eq!(
        fs::read(&sentinel).expect("external target survives"),
        b"foreign target survives"
    );
}

#[test]
fn concurrent_acquire_serializes_one_winner() {
    let fx = Fixture::new("conc");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for holder in ["h1", "h2"] {
        let barrier = Arc::clone(&barrier);
        let req = request(&pool, &project, &workspace, holder);
        joins.push(thread::spawn(move || {
            barrier.wait();
            lease_injected(req)
        }));
    }
    let results: Vec<_> = joins.into_iter().map(|join| join.join().unwrap()).collect();
    let wins = results.iter().filter(|result| result.is_ok()).count();
    let busy = results
        .iter()
        .filter(|result| matches!(result, Err(LeaseError::Busy)))
        .count();
    assert_eq!(wins, 1);
    assert_eq!(busy, 1);
}

#[test]
fn symlink_pool_and_foreign_entry_refuse() {
    let fx = Fixture::new("path");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let link = fx.path().join("link");
    symlink(&pool, &link).expect("symlink");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let result = lease_injected(request(&link, &project, &workspace, "holder-a"));
    assert!(matches!(result, Err(LeaseError::UnsafePath(_))));
    File::create(pool.join("foreign")).unwrap();
    let result = lease_injected(request(&pool, &project, &workspace, "holder-a"));
    assert_eq!(result, Err(unexpected("pool directory", "foreign")));
    assert_eq!(result.unwrap_err().reason(), "unknown_accounting");
}

#[test]
fn eviction_removes_only_idle_owned_slots() {
    let fx = Fixture::new("evict");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024).expect("init");
    let project_a = fx.dir("project-a");
    let project_b = fx.dir("project-b");
    let workspace = fx.dir("workspace");
    let first = lease_injected(request(&pool, &project_a, &workspace, "holder-a")).expect("a");
    let marker = first.env.cargo_build_build_dir.join("blob");
    File::create(&marker)
        .unwrap()
        .write_all(&vec![b'x'; 4096])
        .unwrap();
    let sentinel = project_a.join("src");
    fs::create_dir(&sentinel).unwrap();
    release_injected(&pool, &first.lease_id).expect("release a");
    let second = lease_publishing(&pool, request(&pool, &project_b, &workspace, "holder-b"))
        .0
        .expect("b");
    assert_ne!(second.slot_id, first.slot_id);
    assert!(!marker.exists());
    assert!(sentinel.exists());
}

#[test]
fn idle_eviction_unlinks_direct_dangling_and_nested_symlinks() {
    let fx = Fixture::new("evict-symlinks");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let mut grants = Vec::new();
    for index in 0..super::MAX_SLOTS {
        let project = fx.dir(&format!("project-{index}"));
        let granted = lease_injected(request(
            &pool,
            &project,
            &workspace,
            &format!("holder-{index}"),
        ))
        .expect("fill slot table");
        if index > 0 {
            release_injected(&pool, &granted.lease_id).expect("idle slot");
        }
        grants.push(granted);
    }
    let held = grants[0].clone();
    let held_marker = held.env.cargo_build_build_dir.join("held");
    fs::write(&held_marker, b"held slot stays byte-for-byte").expect("held marker");

    let sentinel = fx.path().join("external-sentinel");
    fs::write(&sentinel, b"external target survives byte-for-byte").expect("sentinel");
    let dangling_target = fx.path().join("absent-sentinel");
    let mut links = Vec::new();
    for (index, kind) in ["direct", "dangling", "nested"].into_iter().enumerate() {
        let build = &grants[index + 1].env.cargo_build_build_dir;
        let link = match kind {
            "direct" => build.join("external-link"),
            "dangling" => build.join("dangling-link"),
            "nested" => {
                let nested = build.join("nested");
                fs::create_dir(&nested).expect("nested directory");
                nested.join("external-link")
            }
            _ => unreachable!(),
        };
        symlink(
            if kind == "dangling" {
                &dangling_target
            } else {
                &sentinel
            },
            &link,
        )
        .expect("symlink entry");
        links.push(link);
    }

    for index in 0..links.len() {
        let project = fx.dir(&format!("new-project-{index}"));
        let granted = lease_injected(request(
            &pool,
            &project,
            &workspace,
            &format!("new-holder-{index}"),
        ))
        .expect("evict the oldest idle slot and allocate");
        assert_ne!(granted.slot_id, grants[index + 1].slot_id);
        assert_eq!(
            fs::symlink_metadata(&links[index])
                .expect_err("owned symlink entry was unlinked")
                .kind(),
            std::io::ErrorKind::NotFound
        );
        assert_eq!(
            fs::read(&sentinel).expect("external sentinel remains"),
            b"external target survives byte-for-byte"
        );
        let status = status_injected(&pool).expect("status after eviction");
        let held_status = status
            .slots
            .iter()
            .find(|slot| slot.slot_id == held.slot_id)
            .expect("held slot remains recorded");
        assert_eq!(held_status.state, SlotOccupancy::Held);
        assert_eq!(
            held_status.lease_id.as_deref(),
            Some(held.lease_id.as_str())
        );
        assert_eq!(
            fs::read(&held_marker).expect("held slot contents remain"),
            b"held slot stays byte-for-byte"
        );
        assert!(matches!(
            release_injected(&pool, &grants[index + 1].lease_id),
            Err(LeaseError::NotFound)
        ));
    }
}

#[test]
fn interrupted_idle_eviction_resumes_after_partial_removal() {
    let fx = Fixture::new("evict-resume");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let other_project = fx.dir("other-project");
    let workspace = fx.dir("workspace");
    let idle =
        lease_injected(request(&pool, &project, &workspace, "idle-holder")).expect("idle lease");
    let slot_root = pool.join("payload").join("slots").join(&idle.slot_id);
    let partial = idle.env.cargo_build_build_dir.join("partial");
    fs::write(&partial, b"removed before restart").expect("partial entry");
    let sentinel = fx.path().join("external-sentinel");
    fs::write(&sentinel, b"external target survives byte-for-byte").expect("sentinel");
    let link = idle.env.cargo_build_build_dir.join("external-link");
    symlink(&sentinel, &link).expect("nested link");
    release_injected(&pool, &idle.lease_id).expect("idle slot");

    let held = lease_injected(request(&pool, &other_project, &workspace, "held-holder"))
        .expect("held lease");
    mark_evicting_for_test(&pool, idle.slot_id.parse().expect("numeric slot id"))
        .expect("persist eviction intent");
    fs::remove_file(&partial).expect("simulate partial removal after durable intent");

    let status = status_injected(&pool).expect("restart reconciles recorded deletion");
    assert!(status.slots.iter().all(|slot| slot.slot_id != idle.slot_id));
    assert!(status.slots.iter().any(
        |slot| slot.slot_id == held.slot_id && slot.lease_id.as_deref() == Some(&held.lease_id)
    ));
    assert!(!slot_root.exists());
    assert!(matches!(
        release_injected(&pool, &idle.lease_id),
        Err(LeaseError::NotFound)
    ));
    assert_eq!(
        fs::read(&sentinel).expect("external target survives"),
        b"external target survives byte-for-byte"
    );
    release_injected(&pool, &held.lease_id).expect("unrelated held lease closes");
    let next_project = fx.dir("next-project");
    let next = lease_injected(request(&pool, &next_project, &workspace, "next-holder"))
        .expect("safe new generation after deletion settles");
    assert_ne!(next.slot_id, idle.slot_id);
}

#[test]
fn unsafe_evicting_slot_root_stays_unavailable_without_blocking_status_or_release() {
    let fx = Fixture::new("evict-unsafe-root");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let other_project = fx.dir("other-project");
    let workspace = fx.dir("workspace");
    let idle =
        lease_injected(request(&pool, &project, &workspace, "idle-holder")).expect("idle lease");
    release_injected(&pool, &idle.lease_id).expect("idle slot");
    let held = lease_injected(request(&pool, &other_project, &workspace, "held-holder"))
        .expect("held lease");
    mark_evicting_for_test(&pool, idle.slot_id.parse().expect("numeric slot id"))
        .expect("persist eviction intent");

    let target = fx.dir("external-target");
    let sentinel = target.join("sentinel");
    fs::write(&sentinel, b"root target survives byte-for-byte").expect("sentinel");
    let slot_root = pool.join("payload").join("slots").join(&idle.slot_id);
    fs::remove_dir_all(&slot_root).expect("remove slot directory in unsafe fixture");
    symlink(&target, &slot_root).expect("replace slot root with symlink");

    let status = status_injected(&pool).expect("unknown slot does not block status");
    assert_eq!(status.coverage, AccountingCoverage::Unknown);
    let abandoned = status
        .slots
        .iter()
        .find(|slot| slot.slot_id == idle.slot_id)
        .expect("unsafe generation remains recorded");
    assert_eq!(abandoned.state, SlotOccupancy::Abandoned);
    assert_eq!(abandoned.unique_allocated_bytes, None);
    assert_eq!(
        gap_kinds(&abandoned.accounting),
        vec![AccountingGapKind::RootSymlink]
    );
    let held_status = status
        .slots
        .iter()
        .find(|slot| slot.slot_id == held.slot_id)
        .expect("held lease remains recorded");
    assert_eq!(
        held_status.lease_id.as_deref(),
        Some(held.lease_id.as_str())
    );
    let other = lease_injected(request(
        &pool,
        &fx.dir("blocked-project"),
        &workspace,
        "blocked-holder",
    ))
    .expect("unrelated slot is admitted");
    assert_ne!(other.slot_id, idle.slot_id);
    release_injected(&pool, &held.lease_id).expect("unrelated held lease closes");
    assert_eq!(
        fs::read(&sentinel).expect("root symlink target is untouched"),
        b"root target survives byte-for-byte"
    );
    assert!(fs::symlink_metadata(&slot_root)
        .expect("unsafe slot root remains present")
        .file_type()
        .is_symlink());
    assert!(matches!(
        release_injected(&pool, &idle.lease_id),
        Err(LeaseError::UnknownAccounting)
    ));
}

#[test]
fn unknown_or_non_directory_macos_metadata_entries_refuse() {
    for case in ["unknown-name", "known-file", "known-symlink"] {
        let fx = Fixture::new(case);
        let pool = fx.dir("pool");
        create_injected(&pool, 1024 * 1024).expect("init");
        let payload = pool.join("payload");
        match case {
            "unknown-name" => {
                fs::create_dir(payload.join(".unrecognized")).expect("unknown dot directory")
            }
            "known-file" => {
                File::create(payload.join(".fseventsd")).expect("metadata file");
            }
            "known-symlink" => {
                let target = fx.dir("metadata-target");
                symlink(target, payload.join(".fseventsd")).expect("metadata symlink");
            }
            _ => unreachable!(),
        };
        let name = if case == "unknown-name" {
            ".unrecognized"
        } else {
            ".fseventsd"
        };
        assert_eq!(
            status_injected(&pool),
            Err(unexpected("payload directory", name))
        );
    }
}

#[test]
fn failed_init_lock_only_retry_succeeds() {
    let fx = Fixture::new("init-retry");
    let pool = fx.dir("pool");
    let zero = create_injected(&pool, 0);
    assert!(matches!(zero, Err(LeaseError::InvalidInput(_))), "{zero:?}");
    assert!(
        !pool.join("lock").exists(),
        "budget check must run before creating lock"
    );

    File::create(pool.join("lock")).unwrap();
    fs::set_permissions(pool.join("lock"), fs::Permissions::from_mode(0o600)).unwrap();
    create_injected(&pool, 1024 * 1024).expect("lock-only leftover must be init-able");

    let occupied = fx.dir("occupied");
    File::create(occupied.join("lock")).unwrap();
    File::create(occupied.join("foreign")).unwrap();
    let refused = create_injected(&occupied, 1024 * 1024);
    assert!(
        matches!(refused, Err(LeaseError::InvalidInput(_))),
        "{refused:?}"
    );
}

#[test]
fn full_debug_is_a_distinct_profile() {
    let fx = Fixture::new("profile");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let dev = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("dev");
    let mut debug = request(&pool, &project, &workspace, "holder-b");
    debug.profile = Profile::FullDebug;
    let debug = lease_injected(debug).expect("debug");
    assert_ne!(dev.slot_id, debug.slot_id);
}

#[test]
fn managed_eviction_retires_only_idle_slots_and_tolerates_managed_control() {
    let fx = Fixture::new("managed-evict");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let managed = pool.join(MANAGED_NAME);
    fs::create_dir(&managed).expect("managed control");
    fs::set_permissions(&managed, fs::Permissions::from_mode(0o700)).expect("private control");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let granted = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let slot_path = pool.join("payload").join("slots").join(&granted.slot_id);
    assert!(matches!(
        evict_idle_slot_injected(&pool, &granted.slot_id),
        Err(LeaseError::Busy)
    ));
    assert!(matches!(
        evict_idle_slot_injected(&pool, "0"),
        Err(LeaseError::InvalidInput(_))
    ));
    release_injected(&pool, &granted.lease_id).expect("release");
    evict_idle_slot_injected(&pool, &granted.slot_id).expect("evict idle");
    assert!(!slot_path.exists());
    assert!(status_injected(&pool).expect("status").slots.is_empty());
    assert!(managed.is_dir());

    fs::remove_dir(&managed).expect("remove control");
    symlink(&project, &managed).expect("managed symlink");
    assert!(matches!(
        status_injected(&pool),
        Err(LeaseError::UnsafePath(_))
    ));
}

/// oxide#196: the operator-confirmed release retires a **held** slot by its
/// exact lease in one transaction. A wrong lease refuses with the row still
/// held and the tree in place; the matching lease moves the tree whole into
/// the holding area, drops the held row and leaves no idle intermediate.
#[test]
fn a_held_slot_is_evicted_whole_only_by_its_exact_lease() {
    let fx = Fixture::new("held-evict");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let granted = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let slot_path = pool.join("payload").join("slots").join(&granted.slot_id);
    // A slot that is not held is not this path.
    let idle = lease_injected(request(&pool, &project, &workspace, "holder-b")).expect("idle");
    release_injected(&pool, &idle.lease_id).expect("release idle");
    assert!(matches!(
        evict_held_slot_injected(&pool, &idle.slot_id, &idle.lease_id),
        Err(LeaseError::Busy)
    ));
    // A wrong lease refuses with the held row and the tree unchanged.
    let wrong = "0".repeat(32);
    assert!(matches!(
        evict_held_slot_injected(&pool, &granted.slot_id, &wrong),
        Err(LeaseError::Busy)
    ));
    assert!(slot_path.is_dir(), "a refused held eviction moved the tree");
    let view = managed::ledger_view(&pool).expect("ledger");
    assert_eq!(
        view.slots
            .iter()
            .find(|slot| slot.slot_id == granted.slot_id)
            .map(|slot| slot.state),
        Some(SlotOccupancy::Held),
        "a refused held eviction released the row"
    );
    // The exact lease moves the tree whole; no idle row is left behind.
    let holding =
        evict_held_slot_injected(&pool, &granted.slot_id, &granted.lease_id).expect("evict held");
    assert_eq!(holding.len(), 1);
    assert!(
        !slot_path.exists(),
        "the held tree stayed in the slots area"
    );
    let view = managed::ledger_view(&pool).expect("ledger");
    assert!(
        view.slots
            .iter()
            .all(|slot| slot.slot_id != granted.slot_id),
        "the held row survived its eviction"
    );
    assert!(
        view.holding
            .iter()
            .any(|entry| entry.name == holding[0] && entry.slot_id == granted.slot_id),
        "the holding row does not name the held eviction"
    );
}

#[test]
fn budget_pool_admits_against_the_budget_and_evicts_whole_idle_slots() {
    let fx = Fixture::new("budget");
    let pool = fx.dir("pool");
    let budget = 64 * 1024;
    let info = managed::init_budget_pool(&pool, budget).expect("init budget pool");
    assert_eq!(info.backend, BUDGET_BACKEND_DIRECTORY_V1);
    assert_eq!(info.cap_bytes, budget);
    assert!(!pool.join("image.sparseimage").exists());
    let project_a = fx.dir("project-a");
    let project_b = fx.dir("project-b");
    let project_c = fx.dir("project-c");
    let workspace = fx.dir("workspace");

    let first =
        managed::lease(request(&pool, &project_a, &workspace, "holder-a")).expect("first lease");
    // Nothing enforces the budget while a lease writes.
    let blob = first.env.cargo_build_build_dir.join("blob");
    fs::write(&blob, vec![b'x'; 256 * 1024]).expect("write past the budget");
    let status = managed::status(&pool).expect("status");
    assert_eq!(status.backend, BUDGET_BACKEND_DIRECTORY_V1);
    assert_eq!(status.full, Some(true));

    // Over budget with the only slot held: admission refuses a new slot.
    assert!(matches!(
        managed::lease(request(&pool, &project_b, &workspace, "holder-b")),
        Err(LeaseError::Full)
    ));
    managed::release(&pool, &first.lease_id).expect("release");

    // Over budget with an idle slot: it is evicted whole, then admitted.
    let second = lease_after_publishing(&pool, request(&pool, &project_b, &workspace, "holder-b"))
        .expect("evict, publish, then admit");
    assert_ne!(second.slot_id, first.slot_id);
    assert!(!blob.exists());
    assert!(!pool.join("payload/slots").join(&first.slot_id).exists());
    // Nothing is left held: the eviction's tree was published and positively
    // removed, which is what freed the bytes the retry admitted against.
    assert!(
        managed::ledger_view(&pool)
            .expect("ledger view")
            .holding
            .is_empty(),
        "a held tree survived the publishing command"
    );
    managed::release(&pool, &second.lease_id).expect("release second");
    let third =
        managed::lease(request(&pool, &project_c, &workspace, "holder-c")).expect("under budget");
    assert_ne!(third.slot_id, second.slot_id);
    assert_eq!(managed::status(&pool).expect("status").slots.len(), 2);
}

#[test]
fn budget_pool_refuses_image_files_foreign_payload_entries_and_a_zero_budget() {
    let fx = Fixture::new("budget-shape");
    assert!(matches!(
        managed::init_budget_pool(&fx.path().join("zero"), 0),
        Err(LeaseError::InvalidInput(_))
    ));
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 1024 * 1024).expect("init budget pool");
    managed::status(&pool).expect("opens");
    let image = pool.join("image.sparseimage");
    fs::write(&image, b"").expect("stray image");
    assert!(matches!(
        managed::status(&pool),
        Err(LeaseError::BackingMismatch)
    ));
    fs::remove_file(&image).expect("remove image");
    let foreign = pool.join("payload/foreign");
    fs::create_dir(&foreign).expect("foreign entry");
    assert_eq!(
        managed::status(&pool),
        Err(unexpected("payload directory", "foreign"))
    );
    fs::remove_dir(&foreign).expect("remove foreign");
    assert!(matches!(
        managed::init_budget_pool(&pool, 1024 * 1024),
        Err(LeaseError::InvalidInput(_))
    ));
}

#[test]
fn a_pool_recording_the_retired_image_backend_refuses_untouched() {
    // A legacy `apfs-sparse-image-v1` pool: config, ledger, receipt and an
    // image file, as the retired backend left them. Every entry point
    // refuses before writing; nothing is attached, created or removed.
    let fx = Fixture::new("legacy-image");
    let pool = fx.dir("pool");
    fs::set_permissions(&pool, fs::Permissions::from_mode(0o700)).expect("private pool");
    let write = |name: &str, bytes: &[u8]| {
        let path = pool.join(name);
        fs::write(&path, bytes).expect("legacy file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private file");
    };
    write(
        "config.json",
        br#"{"version":1,"backend":"apfs-sparse-image-v1","store_id":"00","cap_bytes":134217728,"image_bytes":134217728}"#,
    );
    write(
        "ledger.json",
        br#"{"version":1,"store_id":"00","cap_bytes":134217728,"next_slot":1,"next_lease":1,"slots":[]}"#,
    );
    write("receipt.json", b"{}");
    write("image.sparseimage", b"");
    let before = snapshot_tree(&pool);
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    assert!(matches!(
        managed::status(&pool),
        Err(LeaseError::BackingMismatch)
    ));
    assert!(matches!(
        managed::lease(request(&pool, &project, &workspace, "holder-a")),
        Err(LeaseError::BackingMismatch)
    ));
    assert!(matches!(
        managed::release(&pool, "v1.00.1.1.1"),
        Err(LeaseError::BackingMismatch)
    ));
    // Refused before the lock: no lock file is created and every recorded
    // byte is unchanged, through the pool path or a symlink alias.
    assert_eq!(snapshot_tree(&pool), before);
    let alias = fx.path().join("alias");
    symlink(&pool, &alias).expect("alias");
    assert!(managed::status(&alias).is_err());
    assert!(!pool.join("lock").exists());
    assert_eq!(snapshot_tree(&pool), before);
    // A config recording the image backend refuses alone, before the lock.
    let recorded = fx.dir("recorded");
    fs::set_permissions(&recorded, fs::Permissions::from_mode(0o700)).expect("private pool");
    fs::write(
        recorded.join("config.json"),
        br#"{"version":1,"backend":"apfs-sparse-image-v1","store_id":"00","cap_bytes":134217728,"image_bytes":134217728}"#,
    )
    .expect("config");
    fs::set_permissions(
        recorded.join("config.json"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("private config");
    assert!(matches!(
        managed::status(&recorded),
        Err(LeaseError::BackingMismatch)
    ));
    assert!(!recorded.join("lock").exists());
}

#[test]
fn a_changed_soft_budget_applies_at_the_next_admission() {
    let fx = Fixture::new("soft-budget");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 64 * 1024 * 1024).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let project_a = fx.dir("project-a");
    let first = managed::lease(request(&pool, &project_a, &workspace, "holder-a")).expect("a");
    // The build grows past any later budget; nothing stops it while held.
    let mut file =
        File::create(first.env.cargo_build_build_dir.join("payload.bin")).expect("payload file");
    file.write_all(&vec![7_u8; 256 * 1024])
        .expect("grow past budget");
    managed::release(&pool, &first.lease_id).expect("release a");
    // The recorded 64 MiB budget admits a second project beside it.
    let project_b = fx.dir("project-b");
    let second = managed::lease(request(&pool, &project_b, &workspace, "holder-b")).expect("b");
    managed::release(&pool, &second.lease_id).expect("release b");
    assert_eq!(managed::status(&pool).expect("status").slots.len(), 2);
    // A smaller budget applies at the next admission: whole idle slots go.
    let project_c = fx.dir("project-c");
    let mut smaller = request(&pool, &project_c, &workspace, "holder-c");
    smaller.budget_bytes = Some(64 * 1024);
    // Every held tree stays charged until it is positively removed, so this
    // admission first refuses while its evicted trees are held; publishing the
    // pending trees outside the lock is what lets the next attempt admit.
    assert!(
        matches!(
            managed::lease(smaller.clone()),
            Err(LeaseError::Full) | Err(LeaseError::CapacityCharged { .. })
        ),
        "an admission over budget decided as if its held trees were already gone"
    );
    while managed::drain_holding(&pool, None)
        .map(|pass| pass.gone)
        .unwrap_or(false)
    {}
    let third = managed::lease(smaller).expect("admitted once the evicted trees are gone");
    let status = managed::status(&pool).expect("status after eviction");
    assert!(status
        .slots
        .iter()
        .all(|slot| slot.slot_id == third.slot_id || slot.state == SlotOccupancy::Idle));
    assert!(!status
        .slots
        .iter()
        .any(|slot| slot.slot_id == first.slot_id));
    // The recorded budget is unchanged.
    assert_eq!(status.cap_bytes, 64 * 1024 * 1024);
    let mut zero = request(&pool, &project_a, &workspace, "holder-z");
    zero.budget_bytes = Some(0);
    assert!(matches!(
        managed::lease(zero),
        Err(LeaseError::InvalidInput(_))
    ));
}

/// Finder's `.DS_Store` and AppleDouble `._*` regular files at the pool
/// root, payload root, `slots`, a slot and its build tree: every open
/// succeeds, payload bytes count, a hard link counts once, and an idle slot
/// is reused. Nothing is deleted.
#[test]
fn finder_metadata_is_counted_and_tolerated_at_every_level() {
    let fx = Fixture::new("finder");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 64 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let first = lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let build = first.env.cargo_build_build_dir.clone();
    let object = build.join("object");
    fs::write(&object, vec![7_u8; 64 * 1024]).expect("build output");
    release_injected(&pool, &first.lease_id).expect("release");
    let before = status_injected(&pool).expect("status before");
    assert_eq!(before.coverage, AccountingCoverage::Complete);

    let payload = pool.join("payload");
    let slots = payload.join("slots");
    let slot = slots.join(&first.slot_id);
    let mut added = 0_u64;
    let mut metadata = Vec::new();
    for (dir, counted) in [
        (pool.clone(), false),
        (payload.clone(), true),
        (slots.clone(), true),
        (slot.clone(), true),
        (build.clone(), true),
    ] {
        for (name, len) in [(".DS_Store", 6148_usize), ("._object", 4096)] {
            let path = dir.join(name);
            fs::write(&path, vec![0_u8; len]).expect("finder metadata");
            if counted {
                added += len as u64;
            }
            metadata.push(path);
        }
    }
    let after = status_injected(&pool).expect("metadata tolerated");
    assert_eq!(after.coverage, AccountingCoverage::Complete);
    assert_eq!(
        after.payload_logical_bytes,
        before.payload_logical_bytes.map(|bytes| bytes + added)
    );
    assert!(after.payload_unique_allocated_bytes > before.payload_unique_allocated_bytes);

    let unique = after.payload_unique_allocated_bytes;
    fs::hard_link(&object, payload.join("._linked")).expect("hard-linked sidecar");
    let linked = status_injected(&pool).expect("hard link tolerated");
    assert_eq!(
        linked.payload_logical_bytes,
        after.payload_logical_bytes.map(|bytes| bytes + 64 * 1024)
    );
    assert_eq!(linked.payload_unique_allocated_bytes, unique);

    let again = lease_injected(request(&pool, &project, &workspace, "holder-b")).expect("reuse");
    assert!(again.reused_slot);
    assert_eq!(again.slot_id, first.slot_id);
    release_injected(&pool, &again.lease_id).expect("release reuse");
    for path in &metadata {
        assert!(path.is_file(), "{} kept", path.display());
    }
}

/// A Finder-looking name is no exemption: a symlink, directory or special
/// file under it refuses, named, at the pool and payload roots, and nothing
/// is removed. Near-miss names refuse the same way.
#[test]
fn finder_looking_non_files_and_near_miss_names_refuse_by_name() {
    let cases: [(&str, &str, &str); 9] = [
        ("pool", ".DS_Store", "symlink"),
        ("pool", "._x", "dir"),
        ("pool", "._", "file"),
        ("payload", ".DS_Store", "dir"),
        ("payload", "._x", "symlink"),
        ("payload", "._x", "fifo"),
        ("payload", "._", "file"),
        ("payload", ".ds_store", "file"),
        ("payload", ".DS_Store.bak", "file"),
    ];
    for (place, name, kind) in cases {
        let fx = Fixture::new("finder-refuse");
        let pool = fx.dir("pool");
        managed::init_budget_pool(&pool, 1024 * 1024).expect("init");
        let dir = if place == "pool" {
            pool.clone()
        } else {
            pool.join("payload")
        };
        let path = dir.join(name);
        let target = fx.dir("target");
        match kind {
            "file" => fs::write(&path, b"x").expect("file"),
            "dir" => fs::create_dir(&path).expect("dir"),
            "symlink" => symlink(&target, &path).expect("symlink"),
            "fifo" => {
                let text =
                    std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("path");
                // SAFETY: a valid NUL-terminated path; mkfifo has no other
                // precondition.
                assert_eq!(unsafe { libc::mkfifo(text.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        let place = if place == "pool" {
            "pool directory"
        } else {
            "payload directory"
        };
        assert_eq!(
            status_injected(&pool),
            Err(unexpected(place, name)),
            "{place} {name} {kind}"
        );
        assert!(fs::symlink_metadata(&path).is_ok(), "nothing removed");
    }
}

/// The refusal names the entry with control, quote, backslash and
/// non-ASCII bytes escaped, and elides a long name. APFS refuses a
/// non-UTF-8 name, so that byte form is checked on the label directly.
#[test]
fn unexpected_entry_names_are_escaped_and_bounded() {
    use std::os::unix::ffi::OsStrExt;
    let fx = Fixture::new("finder-escape");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 1024 * 1024).expect("init");
    assert_eq!(
        super::entry_label(std::ffi::OsStr::from_bytes(b"\xff\x80ok")),
        r"\xff\x80ok"
    );
    let raw = std::ffi::OsStr::from_bytes("a\n\x1b\"\\\u{e9} b".as_bytes());
    let path = pool.join("payload").join(raw);
    fs::write(&path, b"x").expect("odd name");
    let refused = status_injected(&pool).unwrap_err();
    assert_eq!(
        refused,
        unexpected("payload directory", r#"a\x0a\x1b\x22\x5c\xc3\xa9 b"#)
    );
    assert_eq!(
        refused.to_string(),
        r#"the payload directory holds an unexpected entry "a\x0a\x1b\x22\x5c\xc3\xa9 b""#
    );
    fs::remove_file(&path).expect("remove odd name");
    let long = "y".repeat(100);
    fs::write(pool.join("payload").join(&long), b"x").expect("long name");
    assert_eq!(
        status_injected(&pool),
        Err(unexpected(
            "payload directory",
            &format!("{}...", "y".repeat(64))
        ))
    );
}

#[test]
fn entry_bound_refuses_reuse_and_allocation_with_its_cause_then_pool_bounds_admit() {
    let fx = Fixture::new("entry-bound");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 64 * 1024 * 1024).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let big = fx.dir("big");
    let small_a = fx.dir("small-a");
    let small_b = fx.dir("small-b");
    let fresh = fx.dir("fresh");
    let mut slots = Vec::new();
    for (project, files) in [(&big, 30), (&small_a, 12), (&small_b, 12)] {
        let grant = managed::lease(request(&pool, project, &workspace, "fill")).expect("lease");
        fill(&grant.env.cargo_build_build_dir, files);
        managed::release(&pool, &grant.lease_id).expect("release");
        slots.push(grant);
    }
    let before = managed::status(&pool).expect("status");
    with_accounting_limits(20, 64, 60_000, || {
        // A single slot over the bound: its reuse walk stops at the bound.
        let scan = incomplete(managed::lease(request(&pool, &big, &workspace, "reuse")));
        assert_eq!(scan.coverage, AccountingCoverage::Partial);
        assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::EntryLimit]);
        assert_eq!((scan.entries_visited, scan.max_entries), (20, 20));
        let message = LeaseError::AccountingIncomplete(Box::new(scan)).to_string();
        assert!(message.contains("partial, entry_limit x1, 20 entries in"));
        assert!(message.contains("(bounds 20 entries, depth 64, 60000 ms)"));
        // Each slot under the bound but the pool over it: a slot under the
        // bound is still reused, and a new slot's admission walk refuses.
        let warm = managed::lease(request(&pool, &small_a, &workspace, "warm")).expect("warm");
        assert!(warm.reused_slot);
        assert_eq!(warm.slot_id, slots[1].slot_id);
        managed::release(&pool, &warm.lease_id).expect("release warm");
        // A new slot needs no whole-payload walk any more (oxide#146): every
        // record is current, so admission is arithmetic and succeeds even
        // under an entry bound that no payload walk could finish.
        let admitted =
            managed::lease(request(&pool, &fresh, &workspace, "new")).expect("recorded admission");
        assert!(!admitted.reused_slot);
        managed::release(&pool, &admitted.lease_id).expect("release the recorded admission");
        // Status reports each walk's own cause; nothing changed.
        let status = managed::status(&pool).expect("status under the bound");
        assert_eq!(
            status.payload_accounting.coverage,
            AccountingCoverage::Partial
        );
        let own = |id: &str| {
            status
                .slots
                .iter()
                .find(|slot| slot.slot_id == id)
                .map(|slot| slot.accounting.coverage)
        };
        assert_eq!(own(&slots[0].slot_id), Some(AccountingCoverage::Partial));
        assert_eq!(own(&slots[1].slot_id), Some(AccountingCoverage::Complete));
        // The recorded admission above is a new member; nothing was removed.
        assert_eq!(status.slots.len(), before.slots.len() + 1);
    });
    // The pool bounds cover the same tree: the big slot is reused as the
    // same generation and a new slot is admitted.
    let reused = managed::lease(request(&pool, &big, &workspace, "reuse")).expect("reuse");
    assert!(reused.reused_slot);
    assert_eq!(
        (reused.slot_id.as_str(), reused.incarnation.as_str()),
        (slots[0].slot_id.as_str(), slots[0].incarnation.as_str())
    );
    // The `fresh` slot the recorded admission above created is reused here,
    // under the generous pool bounds and with its own complete walk.
    let admitted = managed::lease(request(&pool, &fresh, &workspace, "new")).expect("reuse");
    assert!(admitted.reused_slot);
    let status = managed::status(&pool).expect("status");
    assert_eq!(status.coverage, AccountingCoverage::Complete);
    assert_eq!(status.payload_accounting.gaps, Vec::new());
    assert_eq!(status.slots.len(), 4);
}

#[test]
fn elapsed_deadline_refuses_with_its_cause() {
    let fx = Fixture::new("elapsed-bound");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 64 * 1024 * 1024).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let project = fx.dir("project");
    let grant = managed::lease(request(&pool, &project, &workspace, "a")).expect("lease");
    managed::release(&pool, &grant.lease_id).expect("release");
    with_accounting_limits(2_000_000, 64, 0, || {
        let scan = incomplete(managed::lease(request(&pool, &project, &workspace, "b")));
        assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::ElapsedDeadline]);
        let scan = incomplete(managed::lease(request(
            &pool,
            &fx.dir("other"),
            &workspace,
            "c",
        )));
        assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::ElapsedDeadline]);
    });
    assert!(
        managed::lease(request(&pool, &project, &workspace, "b"))
            .expect("reuse")
            .reused_slot
    );
}

#[test]
fn unreadable_slot_entry_refuses_reuse_admission_and_eviction() {
    let fx = Fixture::new("unreadable");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 1024).expect("init tiny budget pool");
    let workspace = fx.dir("workspace");
    let project = fx.dir("project");
    let grant = managed::lease(request(&pool, &project, &workspace, "a")).expect("lease");
    fs::write(
        grant.env.cargo_build_build_dir.join("blob"),
        vec![1_u8; 8192],
    )
    .expect("blob");
    let locked = grant.env.cargo_build_build_dir.join("locked");
    fs::create_dir(&locked).expect("locked dir");
    managed::release(&pool, &grant.lease_id).expect("release");
    let denied = crate::test_fs::InjectedErrorGuard::read_dir(&locked);
    let reuse = managed::lease(request(&pool, &project, &workspace, "b"));
    // Over the tiny budget the recorded idle slot is byte pressure. The
    // accepted stale-idle policy trusts a current record, so a new-slot
    // admission does not re-walk that slot: it evicts the whole idle slot it
    // owns instead of refusing on an entry only a fresh walk would notice.
    let admit = lease_after_publishing(&pool, request(&pool, &fx.dir("other"), &workspace, "c"));
    let status = managed::status(&pool);
    assert!(denied.hits() >= 1, "permission injection was not reached");
    drop(denied);
    let scan = incomplete(reuse);
    assert_eq!(scan.coverage, AccountingCoverage::Partial);
    assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::UnreadableEntry]);
    let admitted = admit.expect("the trusted record admits and the idle slot is evicted whole");
    assert_ne!(admitted.slot_id, grant.slot_id);
    assert!(!grant.env.cargo_build_build_dir.exists());
    let status = status.expect("status");
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].slot_id, admitted.slot_id);
    // The evicted slot is gone, so no walk of it remains to report a gap.
    assert!(!gap_kinds(&status.slots[0].accounting).contains(&AccountingGapKind::UnreadableEntry));
}

#[test]
fn vanished_diagnostic_reaches_status_and_preserves_seen_hardlink_allocation() {
    let fx = Fixture::new("vanished-status");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 16 * 1024 * 1024).expect("init budget pool");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let held =
        managed::lease(request(&pool, &project, &workspace, "held")).expect("create held slot");
    let stable = held.env.cargo_build_build_dir.join("stable");
    fs::write(&stable, vec![7_u8; 4096]).expect("stable inode");
    let vanished_dir = held.env.cargo_build_build_dir.join("nested");
    fs::create_dir(&vanished_dir).expect("nested dir");
    let vanished_link = vanished_dir.join("hardlink");
    fs::hard_link(&stable, &vanished_link).expect("hardlink into nested dir");
    let expected = fs::symlink_metadata(&stable).unwrap().blocks() * 512;

    let status = crate::inventory::walk_observer::with_mutations(
        fx.path(),
        serde_json::json!([
            {"path": vanished_link, "action": "remove_before_symlink_metadata"}
        ])
        .to_string(),
        || managed::status(&pool),
    )
    .expect("status completes after a path vanishes");

    assert_eq!(status.coverage, AccountingCoverage::Complete);
    assert_eq!(status.payload_accounting.vanished, 1);
    assert_eq!(status.payload_accounting.gaps, Vec::new());
    assert_eq!(status.payload_unique_allocated_bytes, Some(expected));
    assert_eq!(status.payload_logical_bytes, Some(4096));
    assert!(status
        .payload_accounting
        .to_string()
        .contains("vanished x1"));
    assert!(stable.exists());
    assert!(!vanished_link.exists());

    let mut old_status = serde_json::to_value(&status).expect("serialize status");
    old_status["payload_accounting"]
        .as_object_mut()
        .unwrap()
        .remove("vanished");
    let decoded: super::PoolStatus =
        serde_json::from_value(old_status).expect("new reader accepts old scan JSON");
    assert_eq!(decoded.payload_accounting.vanished, 0);
}

#[test]
fn vanished_paths_allow_only_the_needed_whole_idle_slot_eviction() {
    let fx = Fixture::new("vanished-eviction");
    let pool = fx.dir("pool");
    let budget = 1024 * 1024;
    managed::init_budget_pool(&pool, budget).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let held_project = fx.dir("held-project");
    let idle_project = fx.dir("idle-project");
    let held = managed::lease(request(&pool, &held_project, &workspace, "held"))
        .expect("create held slot");
    let idle = managed::lease(request(&pool, &idle_project, &workspace, "idle"))
        .expect("create idle slot beside held");
    let held_stable = held.env.cargo_build_build_dir.join("stable");
    fs::write(&held_stable, vec![1_u8; 600 * 1024]).expect("held stable occupancy");
    let vanished_file = held.env.cargo_build_build_dir.join("temporary");
    fs::write(&vanished_file, vec![2_u8; 128 * 1024]).expect("held temporary file");
    let vanished_dir = held.env.cargo_build_build_dir.join("temporary-dir");
    fs::create_dir(&vanished_dir).expect("held temporary directory");
    fs::write(vanished_dir.join("payload"), vec![3_u8; 64 * 1024])
        .expect("held temporary directory payload");
    let idle_file = idle.env.cargo_build_build_dir.join("occupancy");
    fs::write(&idle_file, vec![4_u8; 512 * 1024]).expect("idle occupancy");
    managed::release(&pool, &idle.lease_id).expect("release idle slot");

    let held_stable_bytes = fs::symlink_metadata(&held_stable).unwrap().blocks() * 512;
    let idle_bytes = fs::symlink_metadata(&idle_file).unwrap().blocks() * 512;
    assert!(held_stable_bytes < budget);
    assert!(held_stable_bytes + idle_bytes > budget);
    let next_project = fx.dir("next-project");
    let admitted = crate::inventory::walk_observer::with_mutations(
        fx.path(),
        serde_json::json!([
            {"path": vanished_file, "action": "remove_before_symlink_metadata"},
            {"path": vanished_dir, "action": "remove_before_read_dir"}
        ])
        .to_string(),
        || lease_after_publishing(&pool, request(&pool, &next_project, &workspace, "next")),
    )
    .expect("complete observation permits admission");

    assert!(!admitted.reused_slot);
    assert_ne!(admitted.slot_id, held.slot_id);
    assert_ne!(admitted.slot_id, idle.slot_id);
    assert!(held.env.cargo_build_build_dir.exists());
    assert!(held_stable.exists());
    assert!(!vanished_file.exists());
    assert!(!vanished_dir.exists());
    assert!(!idle.env.cargo_build_build_dir.parent().unwrap().exists());
    let status = managed::status(&pool).expect("status after admission");
    let ids: Vec<_> = status
        .slots
        .iter()
        .map(|slot| slot.slot_id.as_str())
        .collect();
    assert_eq!(ids, vec![held.slot_id.as_str(), admitted.slot_id.as_str()]);
    assert_eq!(
        status.slots[0].state,
        SlotOccupancy::Held,
        "the held slot remains in place"
    );
}

#[test]
fn a_real_gap_still_refuses_before_whole_idle_slot_deletion() {
    let fx = Fixture::new("vanished-gap");
    let pool = fx.dir("pool");
    let budget = 1024 * 1024;
    managed::init_budget_pool(&pool, budget).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let held_project = fx.dir("held-project");
    let idle_project = fx.dir("idle-project");
    let held = managed::lease(request(&pool, &held_project, &workspace, "held"))
        .expect("create held slot");
    let idle = managed::lease(request(&pool, &idle_project, &workspace, "idle"))
        .expect("create idle slot beside held");
    let held_stable = held.env.cargo_build_build_dir.join("stable");
    fs::write(&held_stable, vec![1_u8; 600 * 1024]).expect("held stable occupancy");
    let blocked_dir = held.env.cargo_build_build_dir.join("blocked");
    fs::create_dir(&blocked_dir).expect("blocked directory");
    fs::write(blocked_dir.join("payload"), vec![2_u8; 64 * 1024])
        .expect("blocked directory payload");
    let vanished_file = held.env.cargo_build_build_dir.join("vanished");
    fs::write(&vanished_file, vec![3_u8; 32 * 1024]).expect("vanished fixture file");
    let idle_file = idle.env.cargo_build_build_dir.join("occupancy");
    fs::write(&idle_file, vec![3_u8; 512 * 1024]).expect("idle occupancy");
    managed::release(&pool, &idle.lease_id).expect("release idle slot");
    let next_project = fx.dir("next-project");

    let refused = crate::inventory::walk_observer::with_mutations(
        fx.path(),
        serde_json::json!([
            {"path": blocked_dir, "action": "permission_denied_before_read_dir"},
            {"path": vanished_file, "action": "remove_before_symlink_metadata"}
        ])
        .to_string(),
        || lease_after_publishing(&pool, request(&pool, &next_project, &workspace, "next")),
    );
    let scan = incomplete(refused);
    assert_eq!(scan.coverage, AccountingCoverage::Partial);
    assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::UnreadableEntry]);
    assert_eq!(scan.vanished, 1);
    assert!(scan.to_string().contains("unreadable_entry x1"));
    assert!(scan.to_string().contains("vanished x1"));
    assert!(idle.env.cargo_build_build_dir.exists());
    assert!(held.env.cargo_build_build_dir.exists());
    assert!(blocked_dir.exists());
    assert!(!vanished_file.exists());

    let status = managed::status(&pool).expect("status after conservative refusal");
    assert_eq!(status.slots.len(), 2);
    assert!(status
        .slots
        .iter()
        .any(|slot| { slot.slot_id == idle.slot_id && slot.state == SlotOccupancy::Idle }));
    assert!(status
        .slots
        .iter()
        .any(|slot| { slot.slot_id == held.slot_id && slot.state == SlotOccupancy::Held }));
}

#[test]
fn hard_links_are_counted_once_by_admission() {
    let fx = Fixture::new("hardlinks");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 64 * 1024 * 1024).expect("init budget pool");
    let workspace = fx.dir("workspace");
    let project = fx.dir("project");
    let grant = managed::lease(request(&pool, &project, &workspace, "a")).expect("lease");
    let original = grant.env.cargo_build_build_dir.join("original");
    fs::write(&original, vec![3_u8; 64 * 1024]).expect("original");
    for name in ["link-1", "link-2", "link-3"] {
        fs::hard_link(&original, grant.env.cargo_build_build_dir.join(name)).expect("link");
    }
    managed::release(&pool, &grant.lease_id).expect("release");
    let status = managed::status(&pool).expect("status");
    let unique = status.payload_unique_allocated_bytes.expect("unique");
    let logical = status.payload_logical_bytes.expect("logical");
    assert_eq!(logical, 4 * 64 * 1024);
    assert!(unique < 2 * 64 * 1024, "unique {unique}");
    // A budget between the unique and per-name totals admits a new slot
    // without evicting the linked one.
    let mut between = request(&pool, &fx.dir("other"), &workspace, "b");
    between.budget_bytes = Some(3 * 64 * 1024);
    let admitted = managed::lease(between).expect("admitted beside the linked slot");
    assert_ne!(admitted.slot_id, grant.slot_id);
    assert!(original.exists());
}

#[test]
fn pool_bounds_are_separate_from_the_inventory_defaults() {
    assert_eq!(crate::inventory::DEFAULT_MAX_ENTRIES, 200_000);
    assert_eq!(crate::inventory::DEFAULT_MAX_DEPTH, 64);
    assert_eq!(crate::inventory::DEFAULT_MAX_ELAPSED_MS, 30_000);
    let fx = Fixture::new("bounds");
    let pool = fx.dir("pool");
    managed::init_budget_pool(&pool, 1024 * 1024).expect("init budget pool");
    let status = managed::status(&pool).expect("status");
    let walk = &status.payload_accounting;
    // The payload walk starts one level above `slots` (oxide#105) and a
    // slot may sit in a project folder (oxide#104), so its depth bound is
    // two deeper: slot contents keep 64.
    assert_eq!(
        (walk.max_entries, walk.max_depth, walk.max_elapsed_ms),
        (2_000_000, 66, 60_000)
    );
    assert_eq!(walk.coverage, AccountingCoverage::Complete);
}

/// Roots `body` walked, relative to the pool's payload, in walk order.
#[cfg(debug_assertions)]
fn walked<T>(pool: &Path, body: impl FnOnce() -> T) -> (T, Vec<String>) {
    let (result, roots) = crate::inventory::walk_observer::observe(body);
    let payload = pool.join("payload");
    let roots = roots
        .iter()
        .map(|root| match root.strip_prefix(&payload) {
            Ok(relative) if relative.as_os_str().is_empty() => "payload".to_owned(),
            Ok(relative) => relative.display().to_string(),
            Err(_) => root.display().to_string(),
        })
        .collect();
    (result, roots)
}

fn ledger_injected(pool: &Path) -> Result<super::LedgerView, LeaseError> {
    managed::ledger_view(pool)
}

/// The view is status's membership, field for field, with no walk; status
/// keeps its whole-payload walk and one walk per slot.
#[cfg(debug_assertions)]
#[test]
fn ledger_view_is_status_membership_without_walks() {
    let fx = Fixture::new("view");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let idle_project = fx.dir("idle-project");
    let held_project = fx.dir("held-project");
    let idle = lease_injected(request(&pool, &idle_project, &workspace, "idle")).expect("idle");
    fill(&idle.env.cargo_build_build_dir, 200);
    release_injected(&pool, &idle.lease_id).expect("release");
    let held = lease_injected(request(&pool, &held_project, &workspace, "held")).expect("held");

    let (view, view_walks) = walked(&pool, || ledger_injected(&pool));
    let view = view.expect("view");
    assert!(view_walks.is_empty(), "{view_walks:?}");
    let (status, status_walks) = walked(&pool, || status_injected(&pool));
    let status = status.expect("status");
    assert_eq!(
        status_walks,
        vec![
            "payload".to_owned(),
            format!("slots/{}", idle.slot_id),
            format!("slots/{}", held.slot_id),
        ]
    );
    let shown: Vec<super::LedgerSlot> = status.slots.iter().map(Into::into).collect();
    assert_eq!(
        membership(&view),
        shown,
        "the view and status disagree on membership"
    );
    assert_eq!(
        (&view.store_id, &view.backend, view.cap_bytes),
        (&status.store_id, &status.backend, status.cap_bytes)
    );
    assert_eq!(view.backend, BUDGET_BACKEND_DIRECTORY_V1);
    assert!(!view.reconciliation_blocked);
    let held_member = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == held.slot_id)
        .expect("held member");
    assert_eq!(held_member.state, SlotOccupancy::Held);
    assert_eq!(held_member.holder.as_deref(), Some("held"));
    assert_eq!(
        held_member.lease_id.as_deref(),
        Some(held.lease_id.as_str())
    );
    let idle_member = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == idle.slot_id)
        .expect("idle member");
    assert_eq!(
        (idle_member.holder.as_ref(), idle_member.lease_id.as_ref()),
        (None, None)
    );
}

/// Reuse walks only its target slot and returns that walk; a held replay
/// walks and measures nothing; a new slot keeps the whole-payload admission
/// walk and has no measurement.
#[cfg(debug_assertions)]
#[test]
fn reuse_walks_only_its_slot_and_returns_that_measurement() {
    let fx = Fixture::new("measured");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let target = fx.dir("target-project");
    let other = fx.dir("other-project");
    let (first, first_walks) = walked(&pool, || {
        managed::lease_measured(request(&pool, &target, &workspace, "a"))
    });
    let first = first.expect("new slot");
    assert_eq!(first.measured, None);
    assert!(!first.grant.reused_slot);
    // A new slot's admission derives its figure from the ledger: the only
    // shallow pass is the scope walk, which is not an inventory walk.
    assert!(first_walks.is_empty(), "{first_walks:?}");
    fs::write(
        first.grant.env.cargo_build_build_dir.join("blob"),
        vec![7_u8; 256 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &first.grant.lease_id).expect("release");
    let unrelated = lease_injected(request(&pool, &other, &workspace, "b")).expect("other");
    fill(&unrelated.env.cargo_build_build_dir, 300);
    release_injected(&pool, &unrelated.lease_id).expect("release other");
    let expected = status_injected(&pool)
        .expect("status")
        .slots
        .iter()
        .find(|slot| slot.slot_id == first.grant.slot_id)
        .and_then(|slot| slot.unique_allocated_bytes)
        .expect("target size");

    let (reused, reuse_walks) = walked(&pool, || {
        managed::lease_measured(request(&pool, &target, &workspace, "c"))
    });
    let reused = reused.expect("reuse");
    assert!(reused.grant.reused_slot);
    assert_eq!(reuse_walks, vec![format!("slots/{}", first.grant.slot_id)]);
    assert_eq!(
        reused.measured,
        Some(super::ReuseMeasurement {
            slot_id: first.grant.slot_id.clone(),
            incarnation: first.grant.incarnation.clone(),
            unique_allocated_bytes: expected,
            dir: None,
        })
    );

    let (replay, replay_walks) = walked(&pool, || {
        managed::lease_measured(request(&pool, &target, &workspace, "c"))
    });
    let replay = replay.expect("replay");
    assert_eq!(replay.grant.lease_id, reused.grant.lease_id);
    assert!(replay.grant.reused_slot);
    assert_eq!(replay.measured, None);
    assert!(replay_walks.is_empty(), "{replay_walks:?}");
}

/// The view refuses where status does: a zero slot ID and a slot-root
/// lookup error, with the same errors. A missing held root stays a member,
/// and partial accounting or a symlinked root does not gate it, as in status.
#[test]
fn ledger_view_keeps_status_refusals_and_non_gating_gaps() {
    let fx = Fixture::new("view-refusals");
    let workspace = fx.dir("workspace");

    let zero = fx.dir("zero-pool");
    create_injected(&zero, 1024 * 1024).expect("init zero");
    let zero_project = fx.dir("zero-project");
    lease_injected(request(&zero, &zero_project, &workspace, "z")).expect("lease");
    let ledger_path = zero.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).expect("ledger")).expect("ledger json");
    ledger["slots"][0]["id"] = serde_json::json!(0);
    fs::write(&ledger_path, serde_json::to_vec(&ledger).expect("encode")).expect("rewrite");
    assert_eq!(
        status_injected(&zero).map(|_| ()),
        Err(LeaseError::UnknownAccounting)
    );
    assert_eq!(
        ledger_injected(&zero).map(|_| ()),
        Err(LeaseError::UnknownAccounting)
    );

    let denied = fx.dir("denied-pool");
    create_injected(&denied, 1024 * 1024).expect("init denied");
    let denied_project = fx.dir("denied-project");
    let denied_grant =
        lease_injected(request(&denied, &denied_project, &workspace, "d")).expect("lease");
    let slots = denied.join("payload").join("slots");
    let denied_slot_root = slots.join(&denied_grant.slot_id);
    let denied_lookup = crate::test_fs::InjectedErrorGuard::symlink_metadata(&denied_slot_root);
    let from_status = status_injected(&denied).map(|_| ());
    let from_view = ledger_injected(&denied).map(|_| ());
    assert_eq!(denied_lookup.hits(), 2);
    drop(denied_lookup);
    assert!(
        matches!(from_status, Err(LeaseError::Io(_))),
        "{from_status:?}"
    );
    assert_eq!(from_view, from_status);

    let gaps = fx.dir("gaps-pool");
    create_injected(&gaps, 64 * 1024 * 1024).expect("init gaps");
    let missing_project = fx.dir("missing-project");
    let deep_project = fx.dir("deep-project");
    let linked_project = fx.dir("linked-project");
    let missing = lease_injected(request(&gaps, &missing_project, &workspace, "m")).expect("m");
    let deep = lease_injected(request(&gaps, &deep_project, &workspace, "p")).expect("p");
    fill(&deep.env.cargo_build_build_dir, 50);
    release_injected(&gaps, &deep.lease_id).expect("release deep");
    let linked = lease_injected(request(&gaps, &linked_project, &workspace, "l")).expect("l");
    release_injected(&gaps, &linked.lease_id).expect("release linked");
    let gap_slots = gaps.join("payload").join("slots");
    fs::remove_dir_all(gap_slots.join(&missing.slot_id)).expect("remove held root");
    let external = fx.dir("external");
    fs::remove_dir_all(gap_slots.join(&linked.slot_id)).expect("remove linked root");
    symlink(&external, gap_slots.join(&linked.slot_id)).expect("symlinked root");
    let (status, view) = with_accounting_limits(10, 64, 5_000, || {
        (status_injected(&gaps), ledger_injected(&gaps))
    });
    let status = status.expect("status stays readable");
    let view = view.expect("view stays readable");
    assert_eq!(status.coverage, AccountingCoverage::Unknown);
    let shown: Vec<super::LedgerSlot> = status.slots.iter().map(Into::into).collect();
    assert_eq!(
        membership(&view),
        shown,
        "the view and status disagree on membership"
    );
    let held = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == missing.slot_id)
        .expect("missing held root is still a member");
    assert_eq!(held.state, SlotOccupancy::Held);
    assert_eq!(held.lease_id.as_deref(), Some(missing.lease_id.as_str()));
    // Reuse still needs its own walk complete.
    let refused = with_accounting_limits(10, 64, 5_000, || {
        lease_injected(request(&gaps, &deep_project, &workspace, "p2"))
    });
    assert_eq!(incomplete(refused).coverage, AccountingCoverage::Partial);
}

/// A foreign pool entry refuses both before any slot is read.
#[test]
fn ledger_view_refuses_a_foreign_pool_entry_like_status() {
    let fx = Fixture::new("view-foreign");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    fs::write(pool.join("stray"), b"foreign").expect("stray");
    let expected = unexpected("pool directory", "stray");
    assert_eq!(status_injected(&pool).map(|_| ()), Err(expected.clone()));
    assert_eq!(ledger_injected(&pool).map(|_| ()), Err(expected));
}

/// A ledger writer renames `ledger.tmp-*` onto `ledger.json` between the
/// pool's listing and its lookup, and Finder can remove its metadata the same
/// way. The lookup then finds the name gone: the entry is neither foreign nor
/// a symlink, so the open proceeds.
#[test]
fn entry_renamed_away_during_pool_open_does_not_fail_the_open() {
    let fx = Fixture::new("view-renamed-entry");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    for name in ["ledger.tmp-0123abcd", ".DS_Store"] {
        let entry = pool.join(name);
        fs::write(&entry, b"{}").expect("listed entry");
        let vanished = crate::test_fs::InjectedErrorGuard::not_found_symlink_metadata(&entry);
        assert_eq!(ledger_injected(&pool).map(|_| ()), Ok(()), "{name}");
        assert_eq!(vanished.hits(), 1, "{name} lookup must be exercised");
        drop(vanished);
        fs::remove_file(&entry).expect("cleanup listed entry");
    }
}

/// Only a vanished entry is skipped. A `ledger.tmp-*` symlink still refuses,
/// and any other lookup error on a listed temp still refuses as I/O.
#[test]
fn ledger_temp_refusals_survive_the_vanished_entry_rule() {
    let fx = Fixture::new("view-temp-refusals");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let target = fx.path().join("elsewhere");
    fs::write(&target, b"{}").expect("symlink target");
    let link = pool.join("ledger.tmp-0123abcd");
    symlink(&target, &link).expect("temp symlink");
    assert_eq!(
        ledger_injected(&pool).map(|_| ()),
        Err(LeaseError::UnsafePath("symlinks are refused"))
    );
    fs::remove_file(&link).expect("remove temp symlink");

    let temp = pool.join("ledger.tmp-0123abcd");
    fs::write(&temp, b"{}").expect("temp file");
    let denied = crate::test_fs::InjectedErrorGuard::symlink_metadata(&temp);
    assert_eq!(
        ledger_injected(&pool).map(|_| ()),
        Err(LeaseError::Io(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied).to_string()
        ))
    );
    assert_eq!(denied.hits(), 1, "the temp lookup must be exercised");
}

/// An unknown name refuses from the listing alone, before any lookup, so a
/// vanished-entry lookup armed on it is never reached.
#[test]
fn unknown_entry_refuses_before_any_lookup_during_pool_open() {
    let fx = Fixture::new("view-unknown-lookup");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let stray = pool.join("stray");
    fs::write(&stray, b"foreign").expect("stray");
    let vanished = crate::test_fs::InjectedErrorGuard::not_found_symlink_metadata(&stray);
    assert_eq!(
        ledger_injected(&pool).map(|_| ()),
        Err(unexpected("pool directory", "stray"))
    );
    assert_eq!(vanished.hits(), 0, "an unknown name is refused unlooked-up");
}

const VOLUME_UUID: &str = "A6AC3ED7-33EA-46B4-8082-7254CF291ED2";

const VOLUME_CONFIG: &str = r#"{"version":1,"backend":"apfs-volume-v1","store_id":"store","cap_bytes":1000,"image_bytes":0,"volume":{"mount_point":"/Volumes/Builds","volume_uuid":"A6AC3ED7-33EA-46B4-8082-7254CF291ED2","payload_dir":"oxide-pool"}}"#;

const LEDGER_WITH_EVICTING: &str = r#"{"version":1,"store_id":"store","cap_bytes":1000,"next_slot":3,"next_lease":3,"slots":[{"id":1,"incarnation":1,"state":"evicting","project":"/p","workspace":"/p","profile":"dev","holder":"h","lease_id":null,"last_lease_id":"l1"},{"id":2,"incarnation":1,"state":"idle","project":"/p","workspace":"/p","profile":"dev","holder":"h","lease_id":null,"last_lease_id":"l2"}]}"#;

/// A control directory with private config and ledger files, as init writes.
fn control_fixture(fixture: &Fixture, config: &str, ledger: &str) -> PathBuf {
    let control = fixture.dir("control");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("private control");
    for (name, text) in [("config.json", config), ("ledger.json", ledger)] {
        let path = control.join(name);
        fs::write(&path, text).expect("fixture file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private file");
    }
    control
}

/// The read-only description reads config and ledger and changes nothing,
/// pending eviction records included: no lock, no reconciliation, no write.
#[test]
fn volume_description_reads_config_and_ledger_without_mutation() {
    let fixture = Fixture::new("describe");
    let control = control_fixture(&fixture, VOLUME_CONFIG, LEDGER_WITH_EVICTING);
    let before = snapshot_tree(&control);
    let description = managed::describe_volume(&control).expect("describe");
    assert_eq!(description.evicting_slots, 1);
    assert_eq!(description.volume_uuid, VOLUME_UUID);
    assert_eq!(description.mount_point, PathBuf::from("/Volumes/Builds"));
    assert_eq!(description.payload_dir, "oxide-pool");
    assert_eq!(description.cap_bytes, 1000);
    assert_eq!(description.store_id, "store");
    assert_eq!(
        snapshot_tree(&control),
        before,
        "describe must not change the control directory"
    );
    assert!(!control.join("lock").exists(), "describe takes no lock");
}

/// A missing control directory's files are `NotFound`, not a guess.
#[test]
fn volume_description_reports_missing_config_as_not_found() {
    let fixture = Fixture::new("describe-missing");
    let control = fixture.dir("control");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("private control");
    assert_eq!(
        managed::describe_volume(&control),
        Err(LeaseError::NotFound)
    );
    assert!(fs::read_dir(&control).expect("read").next().is_none());
}

/// An unreadable control file is unknown metadata, not a missing file. The
/// existing test-only seam leaves permissions and the control tree unchanged.
#[test]
fn volume_description_reports_unreadable_control_metadata_as_io() {
    let fixture = Fixture::new("describe-unreadable");
    let control = control_fixture(&fixture, VOLUME_CONFIG, LEDGER_WITH_EVICTING);
    let before = snapshot_tree(&control);
    for name in ["config.json", "ledger.json"] {
        let denied = crate::test_fs::InjectedErrorGuard::symlink_metadata(&control.join(name));
        assert_eq!(
            managed::describe_volume(&control),
            Err(LeaseError::Io(
                std::io::Error::from(std::io::ErrorKind::PermissionDenied).to_string()
            ))
        );
        assert_eq!(
            denied.hits(),
            1,
            "the control-file lookup must be exercised"
        );
        drop(denied);
        assert_eq!(snapshot_tree(&control), before);
    }
    assert!(managed::describe_volume(&control).is_ok());
}

/// Records that are not an apfs-volume-v1 pool, or whose config and ledger
/// disagree, refuse without touching anything.
#[test]
fn volume_description_refuses_other_backends_and_mismatched_ledgers() {
    let fixture = Fixture::new("describe-refuse");
    let budget_config = VOLUME_CONFIG.replace("apfs-volume-v1", BUDGET_BACKEND_DIRECTORY_V1);
    let other_store =
        LEDGER_WITH_EVICTING.replace("\"store_id\":\"store\"", "\"store_id\":\"other\"");
    for (config, ledger) in [
        (budget_config.as_str(), LEDGER_WITH_EVICTING),
        (VOLUME_CONFIG, other_store.as_str()),
    ] {
        let control = control_fixture(&fixture, config, ledger);
        let before = snapshot_tree(&control);
        assert_eq!(
            managed::describe_volume(&control),
            Err(LeaseError::BackingMismatch)
        );
        assert_eq!(snapshot_tree(&control), before);
        fs::remove_dir_all(&control).expect("remove fixture control");
    }
}

fn labelled(
    pool: &Path,
    project: &Path,
    workspace: &Path,
    holder: &str,
    folders: &[&str],
    same_repository: &[&Path],
) -> LeaseRequest {
    let mut request = request(pool, project, workspace, holder);
    request.layout = Some(SlotLayout {
        folders: folders.iter().map(|folder| (*folder).to_owned()).collect(),
        same_repository: same_repository
            .iter()
            .map(|path| path.to_path_buf())
            .collect(),
        profile: "quick".to_owned(),
    });
    request
}

fn build_dir_below_slots(pool: &Path, grant: &super::LeaseGrant) -> String {
    grant
        .env
        .cargo_build_build_dir
        .strip_prefix(pool.join("payload").join("slots"))
        .expect("build dir under slots")
        .display()
        .to_string()
}

/// A new slot goes to `slots/<folder>/<profile>-<id>`; a request without a
/// layout keeps `slots/<id>`; a legacy ledger record has no `dir` field.
#[test]
fn new_slots_use_the_project_folder_and_legacy_keeps_its_path() {
    let fx = Fixture::new("layout");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024 * 1024).expect("init");
    let legacy_project = fx.dir("legacy");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let legacy =
        lease_injected(request(&pool, &legacy_project, &workspace, "legacy")).expect("legacy");
    assert_eq!(build_dir_below_slots(&pool, &legacy), "1/build");
    let ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert!(ledger["slots"][0].get("dir").is_none(), "{ledger}");

    let grant = lease_injected(labelled(
        &pool,
        &project,
        &workspace,
        "new",
        &["oxide-0123abcd"],
        &[&project],
    ))
    .expect("labelled");
    assert_eq!(
        build_dir_below_slots(&pool, &grant),
        "oxide-0123abcd/quick-2/build"
    );
    assert!(pool
        .join("payload/slots/oxide-0123abcd/quick-2/build")
        .is_dir());
    let status = status_injected(&pool).expect("status");
    assert_eq!(status.slots[0].dir, None);
    assert_eq!(
        status.slots[1].dir.as_deref(),
        Some("oxide-0123abcd/quick-2")
    );
    assert_eq!(status.coverage, AccountingCoverage::Complete);

    // A labelled request reusing the legacy slot keeps the legacy path.
    release_injected(&pool, &legacy.lease_id).expect("release legacy");
    let reused = lease_injected(labelled(
        &pool,
        &legacy_project,
        &workspace,
        "again",
        &["legacy-89abcdef"],
        &[&legacy_project],
    ))
    .expect("reuse");
    assert_eq!(reused.slot_id, legacy.slot_id);
    assert_eq!(build_dir_below_slots(&pool, &reused), "1/build");
    assert!(!pool.join("payload/slots/legacy-89abcdef").exists());
}

/// Two repositories with the same label and an injected shortened-ID
/// collision get separate folders; a second slot of the first repository
/// joins its folder; a folder holding unknown entries is never adopted; with
/// no usable folder the slot keeps the legacy path.
#[test]
fn folder_collisions_are_skipped_before_creation() {
    let fx = Fixture::new("collide");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024 * 1024).expect("init");
    let first = fx.dir("first");
    let first_other = fx.dir("first-other");
    let second = fx.dir("second");
    let workspace = fx.dir("workspace");
    let a = lease_injected(labelled(
        &pool,
        &first,
        &workspace,
        "a",
        &["app-aaaaaaaa", "app-aaaaaaaa1111"],
        &[&first],
    ))
    .expect("first");
    assert_eq!(
        build_dir_below_slots(&pool, &a),
        "app-aaaaaaaa/quick-1/build"
    );
    // Same short prefix, another repository: the next folder.
    let b = lease_injected(labelled(
        &pool,
        &second,
        &workspace,
        "b",
        &["app-aaaaaaaa", "app-aaaaaaaa2222"],
        &[&second],
    ))
    .expect("second");
    assert_eq!(
        build_dir_below_slots(&pool, &b),
        "app-aaaaaaaa2222/quick-2/build"
    );
    // Another record of the first repository shares its folder.
    let c = lease_injected(labelled(
        &pool,
        &first_other,
        &workspace,
        "c",
        &["app-aaaaaaaa"],
        &[&first, &first_other],
    ))
    .expect("same repository");
    assert_eq!(
        build_dir_below_slots(&pool, &c),
        "app-aaaaaaaa/quick-3/build"
    );
    // An existing folder with no recorded slot and unknown contents.
    let stranger = pool.join("payload/slots/stray-00000000");
    fs::create_dir(&stranger).expect("stray folder");
    fs::write(stranger.join("note"), b"x").expect("stray entry");
    let other = fx.dir("other");
    let d = lease_injected(labelled(
        &pool,
        &other,
        &workspace,
        "d",
        &["stray-00000000", "app-aaaaaaaa"],
        &[&other],
    ))
    .expect("legacy fallback");
    assert_eq!(build_dir_below_slots(&pool, &d), "4/build");
    assert_eq!(fs::read(stranger.join("note")).expect("untouched"), b"x");
    // Unsafe names refuse rather than escaping the slots root.
    let bad = fx.dir("bad");
    for folder in ["../escape", ".hidden", "Upper", ""] {
        let error = lease_injected(labelled(&pool, &bad, &workspace, "e", &[folder], &[&bad]))
            .expect_err(folder);
        assert_eq!(
            error,
            LeaseError::InvalidInput("invalid slot project folder"),
            "{folder}"
        );
    }
}

/// Idle eviction of a labelled slot removes only that slot; its folder and
/// neighbours stay. A malformed recorded `dir` refuses the ledger.
#[test]
fn labelled_slot_eviction_and_malformed_dir() {
    let fx = Fixture::new("evict-label");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024 * 1024).expect("init");
    let one = fx.dir("one");
    let two = fx.dir("two");
    let workspace = fx.dir("workspace");
    let a = lease_injected(labelled(
        &pool,
        &one,
        &workspace,
        "a",
        &["p-1"],
        &[&one, &two],
    ))
    .expect("a");
    let b = lease_injected(labelled(
        &pool,
        &two,
        &workspace,
        "b",
        &["p-1"],
        &[&one, &two],
    ))
    .expect("b");
    fill(&a.env.cargo_build_build_dir, 2);
    release_injected(&pool, &a.lease_id).expect("release");
    evict_idle_slot_injected(&pool, &a.slot_id).expect("evict");
    assert!(!pool.join("payload/slots/p-1/quick-1").exists());
    assert!(pool.join("payload/slots/p-1/quick-2/build").is_dir());
    assert_eq!(status_injected(&pool).expect("status").slots.len(), 1);
    release_injected(&pool, &b.lease_id).expect("release b");

    let ledger_path = pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).expect("ledger")).expect("json");
    for bad in ["p-1/quick-9", "../x/quick-2", "p-1/quick-2/extra", "p-1"] {
        ledger["slots"][0]["dir"] = serde_json::json!(bad);
        fs::write(&ledger_path, serde_json::to_vec(&ledger).expect("bytes")).expect("write");
        assert_eq!(
            status_injected(&pool).expect_err(bad),
            LeaseError::UnknownAccounting,
            "{bad}"
        );
    }
}

#[test]
fn worst_valid_ledger_roundtrips_within_the_byte_cap() {
    let fx = Fixture::new("worst-ledger");
    let pool = fx.dir("pool");
    // The supported budget is a u64, so its widest decimal form is 20 digits;
    // the config records the same value so the pair still matches.
    create_injected(&pool, u64::MAX).expect("init");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).expect("config")).expect("json");
    let path = format!("/{}", "\u{1}".repeat(1023));
    let holder = "\u{1}".repeat(MAX_HOLDER_BYTES);
    let lease_id = "\u{1}".repeat(MAX_LEASE_ID_BYTES);
    // The maximal all-idle serialization (oxide#177): every slot idle with a
    // closed observation whose generation fence is at its own cap, so the
    // bound covers the record (oxide#146). Every numeric field is at its
    // widest too: 20-digit ids, incarnations and ranks and the maximum `u64`
    // in every recorded figure, so the measured state is a real maximum for
    // this shape rather than a short-number fixture.
    let mut slots = Vec::new();
    for index in 0..super::MAX_SLOTS as u64 {
        let id = u64::MAX - 1 - index;
        let rank = u64::MAX - 1 - index;
        let id_text = id.to_string();
        assert_eq!(id_text.len(), 20, "fixture ids must use the full width");
        let leaf = format!("{}-{id_text}", "b".repeat(80 - id_text.len() - 1));
        slots.push(serde_json::json!({
            "id": id,
            "incarnation": u64::MAX,
            "state": "idle",
            "project": path,
            "workspace": path,
            "profile": "full-debug",
            "holder": holder,
            // The widest accepted lease id: the write bound is 160 bytes and
            // the validator imposes no state/lease-id pairing.
            "lease_id": lease_id,
            "last_lease_id": lease_id,
            "dir": format!("{}/{leaf}", "a".repeat(80)),
            "rank": rank,
            "recorded": {
                "for_lease": lease_id,
                "unique_bytes": u64::MAX,
                "logical_bytes": u64::MAX,
                "entries": u64::MAX,
                "at_ms": u64::MAX,
            },
        }));
    }
    let ledger = serde_json::json!({
        "version": 1,
        "store_id": config["store_id"],
        "cap_bytes": u64::MAX,
        "next_slot": u64::MAX,
        "next_lease": u64::MAX,
        "capacity_policy": RAISED_CAPACITY_POLICY_VERSION,
        "next_rank": u64::MAX,
        "slots": slots,
    });
    let input = serde_json::to_vec(&ledger).expect("maximal all-idle ledger");
    let stored = super::pool::write_ledger_bytes_injected(&pool, &input)
        .expect("the maximal all-idle ledger is accepted");
    let headroom = MAX_LEDGER_BYTES - stored.len() as u64;
    assert!(
        headroom > 0,
        "the writer-reachable all-idle maximum {} does not fit {MAX_LEDGER_BYTES}",
        stored.len()
    );
    assert!(
        stored.len() > 3_800_000,
        "fixture is not substantial: {}",
        stored.len()
    );
    // Reported evidence for the oxide#177 accounting-cost gate.
    println!(
        "measured ordinary-path writer-reachable 256-slot ledger: {} bytes, {headroom} bytes below the {MAX_LEDGER_BYTES}-byte worst-valid cap",
        stored.len()
    );
    assert_eq!(
        fs::read(pool.join("ledger.json"))
            .expect("ledger file")
            .len(),
        stored.len()
    );
    let status = status_injected(&pool).expect("the worst valid ledger reads");
    assert_eq!(status.slots.len(), super::MAX_SLOTS);

    // The same shape with one field over its new cap refuses the revised
    // write and leaves the stored bytes untouched. The negative fixtures use
    // ordinary (not maximum-width) numbers so the admission reaches the field
    // check instead of stopping at exhausted rank headroom.
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let one_slot = |holder: &str, for_lease: &str| {
        serde_json::json!({
            "version": 1,
            "store_id": config["store_id"],
            "cap_bytes": config["cap_bytes"],
            "next_slot": 2,
            "next_lease": 1_000_000,
            "capacity_policy": super::CAPACITY_POLICY_VERSION,
            "next_rank": 2,
            "slots": [{
                "id": 1,
                "incarnation": 1,
                "state": "idle",
                "project": path,
                "workspace": path,
                "profile": "full-debug",
                "holder": holder,
                "lease_id": null,
                "last_lease_id": lease_id,
                "dir": format!("{}/{}", "a".repeat(80), format!("{}-1", "b".repeat(78))),
                "rank": 1,
                // Small figures: an enormous recorded size would trigger
                // byte-pressure eviction of this very slot before the field
                // check the case is about.
                "recorded": {
                    "for_lease": for_lease,
                    "unique_bytes": 4_096,
                    "logical_bytes": 8_192,
                    "entries": 16,
                    "at_ms": 1,
                },
            }],
        })
    };
    for (label, document) in [
        (
            "an oversized holder",
            one_slot(&"x".repeat(MAX_HOLDER_BYTES + 1), &lease_id),
        ),
        (
            "an oversized record generation fence",
            one_slot("holder", &"x".repeat(MAX_LEASE_ID_BYTES + 1)),
        ),
    ] {
        let before = fs::read(pool.join("ledger.json")).expect("stored ledger");
        let bytes = serde_json::to_vec(&document).expect("oversized ledger");
        fs::write(pool.join("ledger.json"), &bytes).expect("oversized field fixture");
        let outcome = lease_injected(request(&pool, &project, &workspace, "holder"));
        assert!(
            matches!(outcome, Err(LeaseError::UnknownAccounting)),
            "{label} did not refuse the revised write: {outcome:?}"
        );
        assert_eq!(
            fs::read(pool.join("ledger.json")).expect("untouched"),
            bytes,
            "{label} was truncated or rewritten"
        );
        fs::write(pool.join("ledger.json"), &before).expect("restore");
    }
}

/// A maximal valid deletion journal (oxide#155): one root plan plus the most
/// capture and foreign records the validator allows, each with the longest
/// chain of the longest capture names and the longest observed original name a
/// writer can produce, so a byte bound measured with it covers everything a
/// recorded deletion can durably hold.
fn maximal_journal_json(tag: u64) -> serde_json::Value {
    let name = |value: u64| {
        format!(
            "{}{:032x}",
            crate::owned_delete::CAPTURE_PREFIX,
            tag * 10_000 + value
        )
    };
    let chain: Vec<String> = (0..64).map(&name).collect();
    assert!(chain.iter().all(|item| item.len() <= 47));
    // The longest bounded journal name: exactly PATH_MAX bytes as NAME_MAX
    // components, all control bytes so compact JSON's sixfold escape is
    // maximal.
    let component = "\u{1}".repeat(204);
    let original = std::iter::repeat_with(|| component.clone())
        .take(5)
        .collect::<Vec<_>>()
        .join("/");
    assert_eq!(original.len(), 1024);
    assert!(original.split('/').all(|part| part.len() <= 255));
    let found = original.clone();
    let captures: Vec<serde_json::Value> = (0..96)
        .map(|index| {
            serde_json::json!({
                "path": chain,
                "private": name(1_000 + index),
                "original": original,
                "device": u64::MAX,
                "inode": u64::MAX,
                // The largest valid capture mode: symbolic link bits plus every
                // permission bit.
                "mode": 0o120777,
            })
        })
        .collect();
    let foreign: Vec<serde_json::Value> = (0..32)
        .map(|index| {
            serde_json::json!({
                "path": chain,
                "private": name(2_000 + index),
                "original": original,
                "expected_device": u64::MAX,
                "expected_inode": u64::MAX,
                // `is_valid` does not constrain this field, so its widest
                // accepted form is the u32 maximum.
                "expected_mode": u32::MAX,
                "found": found,
                "restored": false,
            })
        })
        .collect();
    serde_json::json!({
        "root": {
            "private": name(0),
            // The longest phase name the enum can carry.
            "phase": "captured",
            "device": u64::MAX,
            "inode": u64::MAX,
            "parent_device": u64::MAX,
            "parent_inode": u64::MAX,
            // The largest valid directory mode (type bits plus every
            // permission bit); JSON prints it in decimal.
            "mode": 0o040777,
        },
        "captures": captures,
        "foreign": foreign,
    })
}

/// The maximal accepted ledger carrying a legacy journal (oxide#177): the
/// raised-ceiling representation at 256 slots with every field at its cap, where
/// one slot carries a maximal deletion journal. Since oxide#190 no path in this
/// binary starts a journal, so this is a **read-side** bound — an older writer
/// left the journal, `retire_idle_slot` and the reader both still honour it, and
/// the replay rewrites or drops it. `MAX_LEDGER_BYTES` is
/// the *worst-valid byte bound* — a larger ledger is corrupt and refused, and
/// the reader accepts any otherwise-valid ledger at or below it, including one
/// an older writer left with more than one journal.
#[test]
fn worst_valid_journaled_ledger_roundtrips_within_the_byte_cap() {
    let fx = Fixture::new("worst-journal-ledger");
    let pool = fx.dir("pool");
    // The supported budget is a u64, so its widest decimal form is 20 digits;
    // the config records the same value so the pair still matches.
    create_injected(&pool, u64::MAX).expect("init");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).expect("config")).expect("json");
    let path = format!("/{}", "\u{1}".repeat(1023));
    let holder = "\u{1}".repeat(MAX_HOLDER_BYTES);
    let lease_id = "\u{1}".repeat(MAX_LEASE_ID_BYTES);
    let mut slots = Vec::new();
    for index in 0..super::MAX_SLOTS as u64 {
        let id = u64::MAX - 1 - index;
        let rank = u64::MAX - 1 - index;
        let id_text = id.to_string();
        let leaf = format!("{}-{id_text}", "b".repeat(80 - id_text.len() - 1));
        let mut slot = serde_json::json!({
            "id": id,
            "incarnation": u64::MAX,
            "state": "idle",
            "project": path,
            "workspace": path,
            "profile": "full-debug",
            "holder": holder,
            // The widest accepted lease id: the write bound is 160 bytes and
            // the validator imposes no state/lease-id pairing.
            "lease_id": lease_id,
            "last_lease_id": lease_id,
            "dir": format!("{}/{leaf}", "a".repeat(80)),
            "rank": rank,
            "recorded": {
                "for_lease": lease_id,
                "unique_bytes": u64::MAX,
                "logical_bytes": u64::MAX,
                "entries": u64::MAX,
                "at_ms": u64::MAX,
            },
        });
        if index == 0 {
            // The raised-ceiling representation retains the closed size
            // observation on an `Evicting` record (oxide#175), and this slot
            // carries the maximal deletion journal. Both together are this
            // writer's reachable ordinary-path maximum (see
            // `MAX_LEDGER_BYTES`).
            slot["state"] = serde_json::json!("evicting");
            slot["capture"] = maximal_journal_json(0);
        }
        slots.push(slot);
    }
    let ledger = serde_json::json!({
        "version": 1,
        "store_id": config["store_id"],
        "cap_bytes": u64::MAX,
        "next_slot": u64::MAX,
        "next_lease": u64::MAX,
        "capacity_policy": RAISED_CAPACITY_POLICY_VERSION,
        "next_rank": u64::MAX,
        "slots": slots,
    });
    let input = serde_json::to_vec(&ledger).expect("maximal writer-reachable journaled ledger");
    let stored = super::pool::write_ledger_bytes_injected(&pool, &input)
        .expect("the maximal writer-reachable journaled ledger is accepted");
    let headroom = MAX_LEDGER_BYTES - stored.len() as u64;
    assert!(
        headroom > 0,
        "the writer-reachable journaled maximum {} does not fit {MAX_LEDGER_BYTES}",
        stored.len()
    );
    assert!(
        headroom >= 2 * 1024 * 1024,
        "the writer-reachable maximum leaves only {headroom} bytes of headroom"
    );
    assert!(
        stored.len() > 4_500_000,
        "the journaled fixture did not reach the journal's cost: {}",
        stored.len()
    );
    // The journal really round-tripped, not just its slot row.
    let journal = super::pool::capture_record_injected(&pool, u64::MAX - 1)
        .expect("the journal did not survive");
    assert_eq!(journal.captures.len(), 96);
    assert_eq!(journal.foreign.len(), 32);
    assert_eq!(journal.captures[0].path.len(), 64);
    assert_eq!(journal.captures[0].original.len(), 1024);
    assert_eq!(journal.foreign[0].found.len(), 1024);
    assert_eq!(journal.foreign[0].expected_mode, u32::MAX);
    println!(
        "measured ordinary-path writer-reachable 256-slot journaled ledger: {} bytes, {headroom} bytes below the {MAX_LEDGER_BYTES}-byte worst-valid cap",
        stored.len()
    );
    // A passive snapshot reads the journals without reconciling them.
    let snapshot = super::pool::snapshot_injected(&pool).expect("the journaled ledger reads");
    assert_eq!(snapshot.slots.len(), super::MAX_SLOTS);
    assert_eq!(snapshot.evicting_slot_ids.len(), 1);
}

/// Recovery moves a legacy pending root whole and never starts a journal
/// (oxide#190): `Pool::open` reconciles the holding area and then every
/// `Evicting` record, `reconcile_slot_capture` resolves a legacy #155 journal by
/// #155's own rules (`Concluded` drops the record, `Resume` continues only after
/// the root is restored, `Blocked` keeps it and starts nothing), and a resumed
/// deletion verifies its root in O(1) and moves it into the holding area.
/// Because the per-entry journal start is retired, no open can leave a new
/// journal behind: this test proves the legacy shape stays readable and is
/// adopted, and that a blocked record keeps its journal without starting
/// another. The admission gate and the library's locked open make the
/// version-9 control fence durable before the open, so this path is
/// preceded by the hand-off fence (oxide#177, fence first).
#[test]
fn recovery_keeps_a_second_pending_deletion_without_a_journal() {
    let fixture = Fixture::new("recovery-journal");
    let pool = fixture.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fixture.dir("workspace");
    let first =
        lease_injected(request(&pool, &fixture.dir("project-a"), &workspace, "a")).expect("a");
    let second =
        lease_injected(request(&pool, &fixture.dir("project-b"), &workspace, "b")).expect("b");
    release_injected(&pool, &first.lease_id).expect("release a");
    release_injected(&pool, &second.lease_id).expect("release b");
    let first_id: u64 = first.slot_id.parse().expect("slot id");
    let second_id: u64 = second.slot_id.parse().expect("slot id");
    let first_path = pool.join("payload/slots").join(&first.slot_id);
    // Slot A: an unclassifiable journal, so reconciliation blocks and keeps it.
    super::pool::mark_evicting_for_test(&pool, first_id).expect("recorded deletion");
    fs::remove_dir_all(&first_path).expect("the recorded root is absent");
    super::pool::write_capture_record_injected(
        &pool,
        first_id,
        super::pool::DeletionJournal {
            root: crate::owned_delete::RootCapturePlan {
                private: format!("{}{:032x}", crate::owned_delete::CAPTURE_PREFIX, 31),
                phase: crate::owned_delete::RootCapturePhase::Planned,
                device: 1,
                inode: 2,
                parent_device: 1,
                parent_inode: 3,
                mode: 0o040700,
            },
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("journal");

    // Slot B: `Evicting` with no journal. Its migration is an O(1) root
    // verification plus a whole-tree move, so it completes in the same open
    // instead of waiting on a locked inventory.
    super::pool::mark_evicting_for_test(&pool, second_id).expect("recorded deletion");

    // One open reconciles them: A stays blocked with its unclassifiable
    // journal, and B moves whole into the holding area with no journal
    // started at all.
    let _ = status_injected(&pool);
    assert!(
        super::pool::capture_record_injected(&pool, first_id).is_some(),
        "the blocked journal is retained"
    );
    assert!(
        super::pool::capture_record_injected(&pool, second_id).is_none(),
        "recovery started a journal for the migrated deletion"
    );
    let snapshot = super::pool::snapshot_injected(&pool).expect("snapshot");
    assert_eq!(
        snapshot.evicting_slot_ids,
        vec![first_id.to_string()],
        "only the unclassifiable record stays pending: {:?}",
        snapshot.evicting_slot_ids
    );
    assert_eq!(
        snapshot.holding.len(),
        1,
        "the migrated deletion is held whole: {:?}",
        snapshot.holding
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A control-state lookup that fails for any reason other than absence must
/// refuse the open before any reconciliation (oxide#177, fence first). Only
/// `NotFound` means "a library fixture with nothing to fence"; every other
/// metadata failure refuses, so recovery never runs unfenced (Supervisor
/// ruling 4c9984a1). A regular file where the control directory belongs makes
/// `oxide-managed/state.json` fail with ENOTDIR without leaving an
/// unsearchable directory in the monitored scratch root.
#[test]
fn an_unreadable_control_state_refuses_before_recovery() {
    let fixture = Fixture::new("unreadable-control");
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "unreadable-control");
    // The slot is `Evicting` with its root ready: a reconciling open deletes
    // it and rewrites the ledger, so byte identity proves no reconciliation
    // ran.
    super::pool::mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    let control = pool.join(MANAGED_NAME);
    if control.is_dir() {
        fs::remove_dir_all(&control).expect("control dir");
    }
    fs::write(&control, b"not a control directory").expect("control placeholder");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).expect("control mode");

    let ledger_path = pool.join("ledger.json");
    let before = fs::read(&ledger_path).expect("ledger");
    let result = lease_injected(request(
        &pool,
        &fixture.dir("other-project"),
        &fixture.dir("other-workspace"),
        "other",
    ));
    assert!(
        result.is_err(),
        "an unreadable control state admitted a lease"
    );
    assert_eq!(
        fs::read(&ledger_path).expect("ledger"),
        before,
        "recovery ran although the control state could not be inspected"
    );
    let ledger: serde_json::Value = serde_json::from_slice(&before).expect("ledger json");
    assert_eq!(
        ledger["slots"][0]["state"], "evicting",
        "the pending record was dropped"
    );
    assert!(
        ledger["slots"][0].get("capture").is_none(),
        "a recovery journal was started"
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A control document at the fence version that carries a present-but-invalid
/// fence value is corrupt, not an older document to upgrade. The fence writer
/// refuses and repairs nothing, so no recovery journal starts (oxide#177,
/// Supervisor ruling 4c9984a1).
#[test]
fn an_invalid_control_fence_value_refuses_before_recovery() {
    let (fixture, pool, state, state_before, ledger_before) = control_refusal_fixture(
        "invalid-control-fence",
        br#"{"version":8,"next_id":1,"next_use":1,"tickets":[],"records":[],"cleans":[],"recorded_accounting":"oxide.accounting/1","eviction_accounting":"oxide.eviction-accounting/1","slot_ceiling":42}"#,
    );
    assert_control_refusal(
        &fixture,
        &pool,
        &state,
        &state_before,
        &ledger_before,
        "an invalid control fence admitted a lease",
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// Version 0 is not a document the front door's typed reader accepts, so the
/// fence writer refuses it instead of upgrading it to version 8 (oxide#177).
#[test]
fn an_unsupported_control_version_refuses_before_recovery() {
    let (fixture, pool, state, state_before, ledger_before) = control_refusal_fixture(
        "unsupported-control-version",
        br#"{"version":0,"next_id":1,"next_use":1,"tickets":[],"records":[],"cleans":[]}"#,
    );
    assert_control_refusal(
        &fixture,
        &pool,
        &state,
        &state_before,
        &ledger_before,
        "an unsupported control version admitted a lease",
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// Version 6 requires the one-use fence, so a version-6 document without it
/// is corrupt; the fence writer refuses rather than advancing it to version 8
/// (oxide#177).
#[test]
fn a_version_six_control_without_its_one_use_fence_refuses_before_recovery() {
    let (fixture, pool, state, state_before, ledger_before) = control_refusal_fixture(
        "missing-one-use-fence",
        br#"{"version":6,"next_id":1,"next_use":1,"tickets":[],"records":[],"cleans":[],"recorded_accounting":"oxide.accounting/1"}"#,
    );
    assert_control_refusal(
        &fixture,
        &pool,
        &state,
        &state_before,
        &ledger_before,
        "a version-6 control without its one-use fence admitted a lease",
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A structurally malformed control document (a non-array `records` field) is
/// corrupt, not an older shape to upgrade: the fence and the front door's
/// `Store::load` share one parser, and it refuses before any write.
#[test]
fn a_malformed_records_field_refuses_before_recovery() {
    let (fixture, pool, state, state_before, ledger_before) = control_refusal_fixture(
        "malformed-records",
        br#"{"version":5,"next_id":1,"next_use":1,"tickets":[],"records":42,"cleans":[],"recorded_accounting":"oxide.accounting/1"}"#,
    );
    assert_control_refusal(
        &fixture,
        &pool,
        &state,
        &state_before,
        &ledger_before,
        "a malformed records field admitted a lease",
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// An explicit JSON `null` on a fence is a present non-string value, not an
/// absent field: a complete version-5 document otherwise valid must refuse
/// rather than have the fence inserted (oxide#177).
#[test]
fn an_explicit_null_fence_refuses_before_recovery() {
    let (fixture, pool, state, state_before, ledger_before) = control_refusal_fixture(
        "null-fence",
        br#"{"version":5,"next_id":1,"next_use":1,"tickets":[],"records":[],"cleans":[],"recorded_accounting":"oxide.accounting/1","slot_ceiling":null}"#,
    );
    assert_control_refusal(
        &fixture,
        &pool,
        &state,
        &state_before,
        &ledger_before,
        "an explicit null fence admitted a lease",
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A pool with a ready pending `Evicting` record and the given control-state
/// bytes: a reconciling open would rewrite the ledger, so byte identity proves
/// no reconciliation ran. Returns the fixture, the pool, the control path, the
/// control bytes and the ledger bytes.
fn control_refusal_fixture(
    label: &str,
    control_bytes: &[u8],
) -> (Fixture, PathBuf, PathBuf, Vec<u8>, Vec<u8>) {
    let fixture = Fixture::new(label);
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, label);
    super::pool::mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    let control = pool.join(MANAGED_NAME);
    fs::create_dir_all(&control).expect("control dir");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("control mode");
    let state = control.join("state.json");
    fs::write(&state, control_bytes).expect("control state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).expect("state mode");
    let ledger = fs::read(pool.join("ledger.json")).expect("ledger");
    (fixture, pool, state, control_bytes.to_vec(), ledger)
}

fn assert_control_refusal(
    fixture: &Fixture,
    pool: &Path,
    state: &Path,
    state_before: &[u8],
    ledger_before: &[u8],
    admitted: &str,
) {
    let result = lease_injected(request(
        pool,
        &fixture.dir("other-project"),
        &fixture.dir("other-workspace"),
        "other",
    ));
    assert!(result.is_err(), "{admitted}");
    assert_eq!(
        fs::read(state).expect("state"),
        state_before,
        "the fence writer changed a refused control state"
    );
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("ledger"),
        ledger_before,
        "recovery ran under a refused control state"
    );
    let ledger: serde_json::Value = serde_json::from_slice(ledger_before).expect("ledger json");
    assert_eq!(ledger["slots"][0]["state"], "evicting");
    assert!(
        ledger["slots"][0].get("capture").is_none(),
        "a recovery journal was started"
    );
}

/// A journaled deletion that cannot be classified keeps the pool blocked:
/// every new lease is refused with `unknown_accounting`, so no second eviction
/// (and no second journal) can be started while one is outstanding. This is the
/// writer's single-journal guard (oxide#177); the reader's worst-valid byte
/// bound is the cap itself, not a journal count.
#[test]
fn a_blocked_journaled_deletion_refuses_every_new_lease() {
    let fixture = Fixture::new("blocked-journal");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "blocked-journal");
    super::pool::mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    let plan = crate::owned_delete::RootCapturePlan {
        private: format!("{}{:032x}", crate::owned_delete::CAPTURE_PREFIX, 0),
        phase: crate::owned_delete::RootCapturePhase::Planned,
        device: 1,
        inode: 2,
        parent_device: 1,
        parent_inode: 3,
        mode: 0o040700,
    };
    // The recorded root is gone and neither its original nor its planned
    // private name holds it, so replay must keep the deletion pending.
    fs::remove_dir_all(&slot_path).expect("the recorded root is absent");
    super::pool::write_capture_record_injected(
        &pool,
        slot_id,
        super::pool::DeletionJournal {
            root: plan,
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("journal");
    let workspace = fixture.dir("workspace");
    assert!(
        matches!(
            lease_injected(request(&pool, &fixture.dir("project-b"), &workspace, "b")),
            Err(LeaseError::UnknownAccounting)
        ),
        "a blocked journaled deletion admitted a new lease"
    );
    // The journaled record is retained: no second deletion was started.
    let snapshot = super::pool::snapshot_injected(&pool).expect("snapshot");
    assert_eq!(snapshot.evicting_slot_ids.len(), 1);
}

/// The single-journal guard covers the explicit eviction path too, which does
/// not pass through `lease()`'s blocked-reconciliation refusal: a second
/// `evict_idle_slot` refuses while the blocked journal is retained, so no
/// second journal can accumulate.
#[test]
fn a_blocked_journaled_deletion_refuses_a_second_explicit_eviction() {
    let fixture = Fixture::new("blocked-second-eviction");
    let pool = fixture.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fixture.dir("workspace");
    let first =
        lease_injected(request(&pool, &fixture.dir("project-a"), &workspace, "a")).expect("a");
    let second =
        lease_injected(request(&pool, &fixture.dir("project-b"), &workspace, "b")).expect("b");
    release_injected(&pool, &first.lease_id).expect("release a");
    release_injected(&pool, &second.lease_id).expect("release b");
    let first_id: u64 = first.slot_id.parse().expect("slot id");
    let second_id: u64 = second.slot_id.parse().expect("slot id");
    let first_path = pool.join("payload/slots").join(&first.slot_id);
    let second_path = pool.join("payload/slots").join(&second.slot_id);

    // Block the first deletion: its recorded root is gone and neither its
    // original nor its planned private name holds it, so replay cannot
    // classify it.
    super::pool::mark_evicting_for_test(&pool, first_id).expect("recorded deletion");
    fs::remove_dir_all(&first_path).expect("the recorded root is absent");
    super::pool::write_capture_record_injected(
        &pool,
        first_id,
        super::pool::DeletionJournal {
            root: crate::owned_delete::RootCapturePlan {
                private: format!("{}{:032x}", crate::owned_delete::CAPTURE_PREFIX, 11),
                phase: crate::owned_delete::RootCapturePhase::Planned,
                device: 1,
                inode: 2,
                parent_device: 1,
                parent_inode: 3,
                mode: 0o040700,
            },
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("journal");

    // The explicit eviction path must refuse while that journal is pending.
    let outcome = super::pool::evict_idle_slot_injected(&pool, &second_id.to_string());
    assert!(
        matches!(outcome, Err(LeaseError::UnknownAccounting)),
        "a second explicit eviction started while a journal was outstanding: {outcome:?}"
    );

    // Both records and the journal survive: nothing was deleted or resolved.
    let snapshot = super::pool::snapshot_injected(&pool).expect("snapshot");
    assert_eq!(snapshot.evicting_slot_ids, vec![first.slot_id.clone()]);
    assert!(snapshot
        .slots
        .iter()
        .any(|slot| slot.slot_id == second.slot_id && slot.state == SlotOccupancy::Idle));
    assert!(second_path.is_dir(), "the second slot was touched");
    assert!(super::pool::capture_record_injected(&pool, first_id).is_some());
}

/// oxide#177's accounting-cost gate as one fixed measurement: one whole round
/// trip of the real recorded representation — the bounded ledger read, the
/// ranked ordering admission uses, and the actual atomic ledger write that
/// reads its result back — at the legacy 128-slot ceiling and at the raised
/// 256-slot ceiling. Both figures are printed and each whole round trip must
/// stay under one generous fixed test ceiling.
///
/// The round trip includes the atomic replacement's durable `fsync` of a
/// ledger that now reaches the journaled worst-valid size, which is host I/O
/// and dominates the figure on a loaded machine (195.7 ms was observed for the
/// 256-slot round trip there), so the ceiling is 1000 ms: a fixed sanity bound
/// on the requested write round trip, not on the representation work alone. It
/// still catches a whole-payload walk or a per-slot quadratic regression,
/// which is what the gate is for. This is not a benchmark or a claim about a
/// large live pool.
#[test]
fn a_256_slot_ledger_round_trip_stays_under_the_fixed_cost_ceiling() {
    use std::time::{Duration, Instant};

    const CEILING: Duration = Duration::from_millis(1_000);
    let fx = Fixture::new("ledger-cost");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("config.json")).expect("config")).expect("json");
    let store_id = config["store_id"].as_str().expect("store id").to_owned();
    let documented = |slots: usize| {
        let path = format!("/{}", "p".repeat(200));
        let mut rows = Vec::with_capacity(slots);
        for id in 1..=slots as u64 {
            let last_lease_id = format!("v1.{store_id}.{id}.1.1");
            rows.push(serde_json::json!({
                "id": id,
                "incarnation": 1,
                "state": "idle",
                "project": path,
                "workspace": path,
                "profile": "dev",
                "holder": "holder",
                "lease_id": null,
                "last_lease_id": last_lease_id,
                "dir": format!("label/quick-{id}"),
                "rank": id,
                // The admission basis is the recorded representation, so the
                // measured round trip carries one current observation per slot.
                "recorded": {
                    "for_lease": last_lease_id,
                    "unique_bytes": 4_096,
                    "logical_bytes": 8_192,
                    "entries": 16,
                    "at_ms": 1,
                },
            }));
        }
        // One recorded deletion carries a maximal journal, so the measured
        // read, rank and write also cover journal parsing and serialization.
        rows[0]["state"] = serde_json::json!("evicting");
        rows[0].as_object_mut().expect("object").remove("recorded");
        rows[0]["capture"] = maximal_journal_json(0);
        let policy = if slots > super::LEGACY_MAX_SLOTS {
            RAISED_CAPACITY_POLICY_VERSION
        } else {
            super::CAPACITY_POLICY_VERSION
        };
        serde_json::json!({
            "version": 1,
            "store_id": config["store_id"],
            "cap_bytes": 64 * 1024 * 1024,
            "next_slot": slots as u64 + 1,
            "next_lease": 1_000,
            "capacity_policy": policy,
            "next_rank": slots as u64 + 1,
            "slots": rows,
        })
    };
    let mut measured = Vec::new();
    for slots in [super::LEGACY_MAX_SLOTS, super::MAX_SLOTS] {
        let input = serde_json::to_vec(&documented(slots)).expect("ledger");
        // Warm the code paths once (this write is not measured).
        super::pool::write_ledger_bytes_injected(&pool, &input).expect("warm write");
        // Whole round trips for this size: read the recorded ledger, rank it
        // exactly as admission does, then perform the actual atomic ledger
        // write and read its result back. The gate is the whole round trip, not
        // its parts; the best of three keeps one scheduler stall from turning a
        // generous fixed ceiling into a flaky assertion.
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let started = Instant::now();
            // The passive snapshot reads and validates the ledger (including
            // the journal) without reconciling the recorded deletion.
            let view = super::pool::snapshot_injected(&pool).expect("recorded read");
            assert_eq!(view.slots.len(), slots);
            assert_eq!(view.evicting_slot_ids.len(), 1);
            let mut order: Vec<(u64, u64)> = view
                .slots
                .iter()
                .map(|slot| (slot.rank, slot.slot_id.parse::<u64>().unwrap_or(u64::MAX)))
                .collect();
            order.sort();
            assert_eq!(order.first(), Some(&(1, 1)));
            super::pool::write_ledger_bytes_injected(&pool, &input).expect("valid write");
            best = best.min(started.elapsed());
        }
        println!(
            "{slots}-slot ledger round trip, best of 3 (read, rank, atomic write and read-back): {} us",
            best.as_micros()
        );
        assert!(best < CEILING, "{slots}-slot round trip took {best:?}");
        measured.push((slots, best));
    }
    assert_eq!(measured.len(), 2);
}

#[test]
fn project_replacement_requires_complete_accounting_before_deletion() {
    let fx = Fixture::new("replace-accounting");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let idle = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    release_injected(&pool, &idle.lease_id).expect("release");
    let idle_root = pool.join("payload").join("slots").join(&idle.slot_id);
    let mut replace = request(&pool, &fx.dir("project-b"), &workspace, "b");
    replace.replace_slot = Some(idle.slot_id.parse().expect("slot id"));
    // The whole-payload walk hits the entry bound: the replacement must not
    // delete its victim before the accounting gate refuses.
    let refused = with_accounting_limits(1, 64, 60_000, || lease_injected(replace.clone()));
    let scan = incomplete(refused);
    assert_eq!(scan.coverage, AccountingCoverage::Partial);
    assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::EntryLimit]);
    assert!(
        idle_root.exists(),
        "the victim was deleted before complete accounting"
    );
    let status = status_injected(&pool).expect("status");
    assert!(status.slots.iter().any(|slot| slot.slot_id == idle.slot_id));

    // With complete accounting the same request retires the victim as part
    // of admission and allocates a fresh slot.
    let granted = lease_injected(replace).expect("replacement admission");
    assert!(!granted.reused_slot);
    assert_ne!(granted.slot_id, idle.slot_id);
    assert!(
        !idle_root.exists(),
        "the requested idle whole slot was not retired"
    );
    let status = status_injected(&pool).expect("status after replacement");
    assert_eq!(status.slots.len(), 1);
    assert!(status.slots.iter().all(|slot| slot.slot_id != idle.slot_id));
    assert!(status
        .slots
        .iter()
        .any(|slot| slot.slot_id == granted.slot_id));
}

#[test]
fn project_replacement_checks_rank_exhaustion_before_deletion() {
    let fx = Fixture::new("replace-rank");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let idle = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    release_injected(&pool, &idle.lease_id).expect("release");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    stored["next_rank"] = serde_json::json!(u64::MAX);
    let bytes = serde_json::to_vec(&stored).expect("exhausted");
    fs::write(pool.join("ledger.json"), &bytes).expect("write exhausted");
    let idle_root = pool.join("payload").join("slots").join(&idle.slot_id);
    let mut replace = request(&pool, &fx.dir("project-b"), &workspace, "b");
    replace.replace_slot = Some(idle.slot_id.parse().expect("slot id"));
    assert!(matches!(
        lease_injected(replace),
        Err(LeaseError::LedgerExhausted)
    ));
    assert!(
        idle_root.exists(),
        "the victim was deleted after rank exhaustion"
    );
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("unchanged"),
        bytes
    );
}

#[test]
fn project_replacement_refuses_a_no_longer_idle_victim() {
    let fx = Fixture::new("replace-held");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let held = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    let mut replace = request(&pool, &fx.dir("project-b"), &workspace, "b");
    replace.replace_slot = Some(held.slot_id.parse().expect("slot id"));
    assert!(matches!(lease_injected(replace), Err(LeaseError::Busy)));
    assert!(
        pool.join("payload")
            .join("slots")
            .join(&held.slot_id)
            .exists(),
        "a held slot was deleted"
    );
    let status = status_injected(&pool).expect("status");
    let held_status = status
        .slots
        .iter()
        .find(|slot| slot.slot_id == held.slot_id)
        .expect("held slot");
    assert_eq!(held_status.state, SlotOccupancy::Held);
}

#[test]
fn explicit_idle_eviction_checks_rank_headroom_before_deleting() {
    let fx = Fixture::new("explicit-evict-rank");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    let idle = lease_injected(request(&pool, &fx.dir("project"), &workspace, "a")).expect("a");
    release_injected(&pool, &idle.lease_id).expect("release");
    let slot_root = pool.join("payload").join("slots").join(&idle.slot_id);
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    stored["next_rank"] = serde_json::json!(u64::MAX);
    let bytes = serde_json::to_vec(&stored).expect("exhausted");
    fs::write(pool.join("ledger.json"), &bytes).expect("write exhausted ledger");
    assert!(matches!(
        evict_idle_slot_injected(&pool, &idle.slot_id),
        Err(LeaseError::LedgerExhausted)
    ));
    assert!(
        slot_root.exists(),
        "an exhausted sequence deleted an idle slot"
    );
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("unchanged"),
        bytes,
        "an exhausted sequence changed the ledger"
    );
    let status = status_injected(&pool).expect("status still reads");
    let slot = status
        .slots
        .iter()
        .find(|slot| slot.slot_id == idle.slot_id)
        .expect("idle slot remains a member");
    assert_eq!(slot.state, SlotOccupancy::Idle);
}

/// Read a pool's control files exactly, for a before/after comparison.
fn control_bytes(pool: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for name in ["config.json", "ledger.json"] {
        files.push((
            name.to_owned(),
            fs::read(pool.join(name)).unwrap_or_default(),
        ));
    }
    files
}

fn walk_log() -> Option<PathBuf> {
    std::env::var_os("OXIDE_TEST_WALK_LOG").map(PathBuf::from)
}

/// A passive snapshot reads membership and a recorded deletion without
/// taking the pool lock, reconciling it, walking anything or writing. A
/// mismatched config/ledger pair refuses instead of being repaired.
#[test]
fn snapshot_is_read_only_and_does_not_wait_for_the_pool_lock() {
    let fx = Fixture::new("snapshot");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    fs::write(leased.env.cargo_build_build_dir.join("blob"), b"bytes").expect("blob");
    release_injected(&pool, &leased.lease_id).expect("release");

    // Another holder's exclusive pool lock is not something a snapshot may
    // wait for. The channel timeout proves it returns while the lock is held.
    let held = super::pool::lock_injected(&pool).expect("lock");
    let (sender, receiver) = std::sync::mpsc::channel();
    let pool_copy = pool.clone();
    let log = walk_log();
    let reader = thread::spawn(move || {
        let before = control_bytes(&pool_copy);
        let result = super::pool::snapshot_injected(&pool_copy);
        let after = control_bytes(&pool_copy);
        let slot_dirs: Vec<String> = match &result {
            Ok(snapshot) => snapshot
                .slots
                .iter()
                .map(|slot| slot.dir.clone().unwrap_or_else(|| slot.slot_id.clone()))
                .collect(),
            Err(_) => Vec::new(),
        };
        sender
            .send((result, before, after, slot_dirs))
            .expect("send");
    });
    let (result, before, after, _) = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("a passive snapshot must not wait for the pool lock");
    drop(held);
    reader.join().expect("reader");
    let snapshot = result.expect("snapshot");
    assert_eq!(before, after, "a snapshot wrote a control file");
    assert_eq!(snapshot.slots.len(), 1);
    assert_eq!(snapshot.slots[0].slot_id, leased.slot_id);
    assert_eq!(snapshot.slots[0].state, SlotOccupancy::Idle);
    assert_eq!(snapshot.slots[0].lease_id, None);
    assert!(snapshot.evicting_slot_ids.is_empty());
    assert_eq!(snapshot.backend, BUDGET_BACKEND_DIRECTORY_V1);
    assert_eq!(
        snapshot.payload,
        fs::canonicalize(pool.join("payload")).expect("payload")
    );
    if let Some(log) = log {
        let contents = fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !contents.contains(&format!("snapshot-walks-{}", std::process::id())),
            "the walk log is shared; no snapshot walk is expected here"
        );
    }

    // A recorded version that disagrees with the config is unknown, never
    // silently repaired by a read.
    super::pool::mismatched_store_id_injected(&pool).expect("mismatch");
    assert_eq!(
        super::pool::snapshot_injected(&pool),
        Err(LeaseError::BackingMismatch)
    );
    assert!(status_injected(&pool).is_err());
}

/// A passive measurement walks the recorded payload and slots but never
/// reconciles: a recorded `Evicting` slot stays intact, its bytes are walked
/// and coverage is honestly unknown because nothing finished its deletion.
#[test]
fn measure_walks_without_reconciling_a_recorded_deletion() {
    let fx = Fixture::new("measure");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    fs::write(
        leased.env.cargo_build_build_dir.join("blob"),
        vec![3_u8; 64 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &leased.lease_id).expect("release");
    let slot = leased.slot_id.parse::<u64>().expect("slot id");
    super::pool::mark_evicting_for_test(&pool, slot).expect("mark evicting");
    let ledger_before = fs::read(pool.join("ledger.json")).expect("ledger");
    let slot_path = pool.join("payload/slots").join(&leased.slot_id);
    assert!(slot_path.is_dir());

    let measured = super::pool::measure_injected(&pool).expect("measure");
    assert_eq!(
        measured.status.coverage,
        AccountingCoverage::Unknown,
        "a pending recorded deletion makes coverage unknown"
    );
    assert!(
        measured.status.payload_unique_allocated_bytes.unwrap_or(0) >= 64 * 1024,
        "the walk covers the pending slot's bytes"
    );
    assert_eq!(measured.status.slots.len(), 1);
    assert_eq!(measured.status.slots[0].state, SlotOccupancy::Abandoned);
    assert_eq!(
        measured.snapshot.evicting_slot_ids,
        vec![leased.slot_id.clone()]
    );
    assert!(slot_path.is_dir(), "measure deleted the recorded slot");
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("ledger"),
        ledger_before,
        "measure wrote the ledger"
    );

    // The mutating status reconciles it; the passive measurement must not.
    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "status reconciles the deletion");
    assert!(!slot_path.exists());
}

/// A measured status and the mutating status agree on a pool with nothing
/// pending, so the passive path is not a weaker measurement in disguise.
#[test]
fn measure_matches_status_for_a_settled_pool() {
    let fx = Fixture::new("measure-settled");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    fs::write(
        leased.env.cargo_build_build_dir.join("blob"),
        vec![1_u8; 4096],
    )
    .expect("blob");
    release_injected(&pool, &leased.lease_id).expect("release");

    let measured = super::pool::measure_injected(&pool).expect("measure");
    let walked = status_injected(&pool).expect("status");
    assert_eq!(measured.status.coverage, walked.coverage);
    assert_eq!(
        measured.status.payload_unique_allocated_bytes,
        walked.payload_unique_allocated_bytes
    );
    assert_eq!(
        measured.status.slots[0].unique_allocated_bytes,
        walked.slots[0].unique_allocated_bytes
    );
}

/// Reuse's measurement carries the exact recorded slot directory, so a
/// recorded observation binds to that directory and not to an assumed
/// `slots/<id>` path.
#[test]
fn reuse_measurement_names_the_recorded_slot_directory() {
    let fx = Fixture::new("reuse-dir");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let mut first = request(&pool, &project, &workspace, "a");
    first.layout = Some(SlotLayout {
        folders: vec!["project-aaaa".to_owned()],
        same_repository: Vec::new(),
        profile: "quick".to_owned(),
    });
    let leased = lease_injected(first).expect("new slot");
    release_injected(&pool, &leased.lease_id).expect("release");
    let reused = managed::lease_measured(request(&pool, &project, &workspace, "b")).expect("reuse");
    let measured = reused.measured.expect("reuse measures its slot");
    assert_eq!(measured.slot_id, leased.slot_id);
    assert_eq!(
        measured.dir.as_deref(),
        Some("project-aaaa/quick-1"),
        "the measurement names the recorded directory"
    );
}

/// One measurement has one cumulative elapsed deadline: every walk
/// recomputes what is left of it, so a pool cannot multiply the budget by
/// its slot count and a slow first walk shortens the rest.
#[test]
fn measure_walks_share_one_cumulative_deadline() {
    let fx = Fixture::new("measure-deadline");
    let pool = fx.dir("pool");
    create_injected(&pool, 32 * 1024 * 1024).expect("init");
    for index in 0..3 {
        let project = fx.dir(&format!("project-{index}"));
        let workspace = fx.dir(&format!("workspace-{index}"));
        let leased = lease_injected(request(&pool, &project, &workspace, &format!("h{index}")))
            .expect("lease");
        release_injected(&pool, &leased.lease_id).expect("release");
    }
    // Wait well inside the bound before walking: a fresh per-walk bound would
    // still report the full figure, while a shared deadline can only report
    // what is left.
    let bounds = super::pool::measure_after_delay_injected(&pool, 2_000, 300).expect("measure");
    assert_eq!(bounds.len(), 4, "the payload and three slots are measured");
    assert!(
        bounds.iter().all(|bound| *bound < 2_000),
        "a walk restarted the full bound instead of sharing the deadline: {bounds:?}"
    );
    assert!(
        bounds.iter().all(|bound| *bound >= 1),
        "every walk keeps a positive bound: {bounds:?}"
    );
    assert!(
        bounds.windows(2).all(|pair| pair[0] >= pair[1]),
        "the remaining bound must not grow between walks: {bounds:?}"
    );
}

// ---------------------------------------------------------------------------
// Recorded per-slot accounting (oxide#146)
// ---------------------------------------------------------------------------

/// Ledger membership with the size observation removed: a status walk reports no
/// record, so membership is what the two views must agree on.
fn membership(view: &super::LedgerView) -> Vec<super::LedgerSlot> {
    view.slots
        .iter()
        .map(|slot| {
            let mut shown = slot.clone();
            shown.recorded = None;
            shown
        })
        .collect()
}

/// The recorded figure the ledger holds for one slot, and whether it is current.
fn recorded_of(view: &super::LedgerView, slot_id: &str) -> Option<(u64, bool)> {
    view.slots
        .iter()
        .find(|slot| slot.slot_id == slot_id)
        .and_then(|slot| slot.recorded)
        .map(|fact| (fact.unique_allocated_bytes, fact.current))
}

#[test]
fn release_records_the_slot_and_all_recorded_admission_walks_nothing() {
    use crate::inventory::walk_observer;

    let fx = Fixture::new("recorded-release");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");

    let first = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    let payload = first.env.cargo_build_build_dir.join("generated.bin");
    fs::write(&payload, vec![b'x'; 2 * 1024 * 1024]).expect("payload");
    release_injected(&pool, &first.lease_id).expect("release a");

    // Release published one closed observation, bound to this generation.
    let view = managed::ledger_view(&pool).expect("ledger view");
    let (recorded, current) = recorded_of(&view, &first.slot_id).expect("recorded size");
    assert!(current, "the released slot's record is not current");
    assert!(
        recorded >= 2 * 1024 * 1024,
        "the record undercounts its own payload: {recorded}"
    );

    // A new-slot admission with every record current walks no inventory at
    // all: no payload walk, no slot walk. It does make one shallow scope pass.
    let (second, walks) = walk_observer::observe(|| {
        lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b"))
    });
    let second = second.expect("second admission");
    assert!(walks.is_empty(), "admission walked inventory: {walks:?}");
    assert!(!second.reused_slot);

    let (walked, scopes) = walk_observer::observe_scopes(|| {
        lease_injected(request(&pool, &fx.dir("project-c"), &workspace, "c")).expect("c")
    });
    assert!(!walked.reused_slot);
    assert_eq!(
        scopes.len(),
        1,
        "expected one shallow scope pass: {scopes:?}"
    );

    // A held slot never reports an idle observation.
    let held = managed::ledger_view(&pool).expect("ledger view");
    let held_fact = held
        .slots
        .iter()
        .find(|slot| slot.slot_id == walked.slot_id)
        .and_then(|slot| slot.recorded);
    assert!(held_fact.is_none(), "a held slot published an idle record");
}

#[test]
fn missing_records_walk_only_the_unrecorded_slot_and_publish_progress() {
    use crate::inventory::walk_observer;

    let fx = Fixture::new("recorded-missing");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");

    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    fs::write(
        a.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 1024 * 1024],
    )
    .expect("a blob");
    release_injected(&pool, &a.lease_id).expect("release a");
    let b = lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b")).expect("b");
    fs::write(
        b.env.cargo_build_build_dir.join("blob"),
        vec![b'b'; 1024 * 1024],
    )
    .expect("b blob");
    release_injected(&pool, &b.lease_id).expect("release b");

    // A legacy-shaped ledger: one slot keeps its record, the other has none.
    super::pool::drop_record_for_test(&pool, b.slot_id.parse().expect("numeric slot id"))
        .expect("drop one record");

    let b_path = pool.join("payload").join("slots").join(&b.slot_id);
    let (third, walks) = walk_observer::observe(|| {
        lease_injected(request(&pool, &fx.dir("project-c"), &workspace, "c"))
    });
    let third = third.expect("admission with one missing record");
    assert!(!third.reused_slot);
    assert_eq!(
        walks,
        vec![b_path.clone()],
        "admission walked something other than exactly the unrecorded slot"
    );

    release_injected(&pool, &third.lease_id).expect("release the recorded admission");
    // The completed walk is durable progress: the next admission walks nothing.
    let (_, again) = walk_observer::observe(|| {
        lease_injected(request(&pool, &fx.dir("project-d"), &workspace, "d"))
    });
    assert!(
        again.is_empty(),
        "the published record was not reused: {again:?}"
    );
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        recorded_of(&view, &b.slot_id)
            .is_some_and(|(bytes, current)| current && bytes >= 1024 * 1024),
        "the missing record was not published with its measured figure"
    );
}

#[test]
fn a_stale_record_is_never_current_and_is_remeasured_instead() {
    use crate::inventory::walk_observer;

    let fx = Fixture::new("recorded-stale");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");
    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    fs::write(
        a.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 1024 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &a.lease_id).expect("release a");

    // A record for a different lease generation: never current, never zero.
    // The absurd figure would be visible if a stale record were borrowed.
    super::pool::with_stale_record_for_test(&pool, a.slot_id.parse().expect("numeric slot id"))
        .expect("stale record");

    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        recorded_of(&view, &a.slot_id).is_some_and(|(_, current)| !current),
        "a stale record read as current"
    );
    let slot_path = pool.join("payload").join("slots").join(&a.slot_id);
    let (b, walks) = walk_observer::observe(|| {
        lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b"))
    });
    let b = b.expect("admission");
    assert!(!b.reused_slot);
    assert_eq!(walks, vec![slot_path], "the stale slot was not remeasured");
}

#[test]
fn a_bounded_capacity_refusal_names_at_most_eight_slots() {
    let unmeasured: Vec<super::UnmeasuredSlot> = (1..=128)
        .map(|id| super::UnmeasuredSlot {
            slot_id: id.to_string(),
            incarnation: "1".to_owned(),
            dir: None,
        })
        .collect();
    let error = LeaseError::CapacityCharged {
        measured: Some(4096),
        limit: Some(2048),
        unmeasured: unmeasured.clone(),
    };
    assert_eq!(error.reason(), "full");
    assert_eq!(error.exit_code(), 3);
    let text = error.to_string();
    assert!(
        text.contains("unmeasured slot(s) 1, 2, 3, 4, 5, 6, 7, 8 and 120 more"),
        "{text}"
    );
    assert!(text.contains("never evicted on a partial walk"), "{text}");
    assert!(text.len() < 400, "the refusal is not bounded: {text}");
    // A count replacement the admission could not honour is a count decision:
    // it prints no byte figure, still names at most eight slots, and refuses
    // in the same capacity class.
    let error = LeaseError::CapacityCharged {
        measured: None,
        limit: None,
        unmeasured,
    };
    assert_eq!(error.reason(), "full");
    assert_eq!(error.exit_code(), 3);
    let text = error.to_string();
    assert!(
        text.starts_with("cannot admit a new slot: unmeasured slot(s)"),
        "{text}"
    );
    assert!(
        text.contains("unmeasured slot(s) 1, 2, 3, 4, 5, 6, 7, 8 and 120 more"),
        "{text}"
    );
    assert!(
        !text.contains("bytes"),
        "a count refusal claimed a byte figure: {text}"
    );
    assert!(text.len() < 400, "the refusal is not bounded: {text}");
}

/// One released idle slot with a current record on a fresh pool: the oxide#186
/// fixtures take its record away (or leave it) to control which slots an
/// admission must walk.
fn released_idle_slot(
    fixture: &Fixture,
    label: &str,
    cap: u64,
) -> (PathBuf, u64, PathBuf, PathBuf) {
    let pool = fixture.dir(&format!("{label}-pool"));
    create_injected(&pool, cap).expect("init");
    let workspace = fixture.dir(&format!("{label}-workspace"));
    let project = fixture.dir(&format!("{label}-project"));
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    release_injected(&pool, &leased.lease_id).expect("release");
    let slot_id = leased.slot_id.parse::<u64>().expect("slot id");
    let slot_path = pool.join("payload/slots").join(&leased.slot_id);
    (pool, slot_id, slot_path, workspace)
}

/// Run one admission under the debug walk delay on `delayed`, so that walk
/// stops at its elapsed bound.
fn elapsed_admission(
    fixture: &Fixture,
    delayed: &Path,
    body: impl FnOnce() -> Result<(super::Leased, Vec<(u64, u64)>), LeaseError>,
) -> Result<(super::Leased, Vec<(u64, u64)>), LeaseError> {
    crate::inventory::walk_observer::with_mutations(
        fixture.path(),
        serde_json::json!([{
            "path": delayed, "action": "delay_before_read_dir", "delay_ms": 300
        }])
        .to_string(),
        || with_accounting_limits(2_000_000, 64, 100, body),
    )
}

#[test]
fn an_unrecorded_slot_whose_admission_walk_elapses_is_charged_and_kept() {
    let fx = Fixture::new("unmeasured-elapsed");
    let (pool, slot_id, slot_path, workspace) =
        released_idle_slot(&fx, "elapsed", 64 * 1024 * 1024);
    super::pool::drop_record_for_test(&pool, slot_id).expect("drop the record");
    let before = snapshot_tree(&slot_path);
    // The delay fires when the slot's own walk lists its root, so that walk
    // stops at its 100 ms bound having observed nothing. The scope pass
    // hard-stops at a recorded slot root and never reads it, so it stays
    // complete: only this one slot is unmeasured.
    let (admitted, charges) = elapsed_admission(&fx, &slot_path, || {
        super::pool::lease_with_charges_injected(request(
            &pool,
            &fx.dir("other-project"),
            &workspace,
            "other",
        ))
    })
    .expect("a partial slot walk no longer refuses the admission");
    assert!(!admitted.grant.reused_slot);
    assert_ne!(admitted.grant.slot_id, slot_id.to_string());
    assert_eq!(
        charges,
        vec![(slot_id, 1)],
        "the unmeasured slot is charged, never read as free space"
    );
    assert_eq!(
        snapshot_tree(&slot_path),
        before,
        "the unmeasured slot was touched"
    );
    assert!(slot_path.is_dir());
    let view = managed::ledger_view(&pool).expect("ledger view");
    let generation = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == slot_id.to_string())
        .expect("the unmeasured slot is still in the ledger");
    assert_eq!(
        admitted.unmeasured.len(),
        1,
        "the caller cannot see the fact"
    );
    assert_eq!(admitted.unmeasured[0].slot_id, slot_id.to_string());
    assert_eq!(admitted.unmeasured[0].incarnation, generation.incarnation);
    assert_eq!(admitted.unmeasured[0].dir, generation.dir);
    assert_eq!(
        admitted.admission_note, None,
        "an ordinary admission gained a note"
    );
    assert!(
        recorded_of(&view, &slot_id.to_string()).is_none(),
        "a partial walk published a record"
    );
    assert_eq!(
        recorded_of(&view, &admitted.grant.slot_id),
        None,
        "the new slot was never measured"
    );
    let measured = status_injected(&pool).expect("status");
    assert!(measured
        .slots
        .iter()
        .any(|slot| slot.slot_id == slot_id.to_string() && slot.state == SlotOccupancy::Idle));
}

#[test]
fn a_partial_slot_walk_is_charged_the_bytes_it_observed() {
    let fx = Fixture::new("unmeasured-observed");
    let (pool, slot_id, slot_path, workspace) =
        released_idle_slot(&fx, "observed", 64 * 1024 * 1024);
    let blob = slot_path.join("build").join("blob");
    fs::write(&blob, vec![b'o'; 128 * 1024]).expect("blob");
    let nested = slot_path.join("build").join("nested");
    fs::create_dir(&nested).expect("nested dir");
    fs::write(nested.join("payload"), b"unscanned").expect("nested payload");
    let observed = fs::symlink_metadata(&blob).expect("blob metadata").blocks() * 512;
    assert!(observed > 1);
    super::pool::drop_record_for_test(&pool, slot_id).expect("drop the record");
    let before = snapshot_tree(&slot_path);
    // A depth bound of one reaches the slot's own files and stops at the nested
    // directory: the walk is partial with the bytes it saw.
    let (admitted, charges) = with_accounting_limits(2_000_000, 1, 60_000, || {
        super::pool::lease_with_charges_injected(request(
            &pool,
            &fx.dir("other-project"),
            &workspace,
            "other",
        ))
    })
    .expect("a bounded slot walk no longer refuses the admission");
    assert!(!admitted.grant.reused_slot);
    assert_eq!(
        charges,
        vec![(slot_id, observed)],
        "the charge is the bytes the partial walk observed"
    );
    assert_eq!(
        snapshot_tree(&slot_path),
        before,
        "the unmeasured slot was touched"
    );
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(recorded_of(&view, &slot_id.to_string()).is_none());
}

#[test]
fn byte_pressure_evicts_a_verified_idle_slot_never_the_unmeasured_one() {
    let fx = Fixture::new("unmeasured-pressure");
    let cap = 1024 * 1024;
    let (pool, unmeasured, unmeasured_path, workspace) = released_idle_slot(&fx, "pressure", cap);
    // A second idle slot whose closed record is current: the only verified
    // candidate, and it is newer than the slot the walk cannot measure.
    let recorded = lease_injected(request(
        &pool,
        &fx.dir("recorded-project"),
        &workspace,
        "recorded",
    ))
    .expect("recorded slot");
    let blob = recorded.env.cargo_build_build_dir.join("blob");
    fs::write(&blob, vec![b'r'; 700 * 1024]).expect("blob");
    release_injected(&pool, &recorded.lease_id).expect("release");
    let recorded_bytes = fs::symlink_metadata(&blob).expect("blob metadata").blocks() * 512;
    assert!(recorded_bytes < cap);
    super::pool::drop_record_for_test(&pool, unmeasured).expect("drop the record");
    let recorded_path = pool.join("payload/slots").join(&recorded.slot_id);
    let unmeasured_before = snapshot_tree(&unmeasured_path);
    // A byte-pressure attempt that evicts refuses while its tree is charged;
    // one publishing pass outside the lock changes the figure and the retry
    // admits. An attempt that admits directly needs no pass.
    let other_project = fx.dir("other-project");
    if matches!(
        super::pool::lease_with_charges_injected(request(
            &pool,
            &other_project,
            &workspace,
            "other",
        )),
        Err(LeaseError::Full) | Err(LeaseError::CapacityCharged { .. })
    ) {
        let _ = managed::drain_holding(&pool, None);
    }
    let (admitted, charges) = elapsed_admission(&fx, &unmeasured_path, || {
        super::pool::lease_with_charges_injected(request(
            &pool,
            &other_project,
            &workspace,
            "other",
        ))
    })
    .expect("byte pressure evicts the verified candidate and admits");
    // The retry admits once the evicted tree is positively removed, so the
    // conservative charge appears only on the attempt that walked the pool;
    // either way the unmeasured slot is never free and never the victim.
    assert!(
        charges.is_empty() || charges == vec![(unmeasured, recorded_bytes)],
        "the charge is the largest recorded slot: {charges:?} (largest {recorded_bytes})"
    );
    assert_ne!(admitted.grant.slot_id, unmeasured.to_string());
    assert!(
        unmeasured_path.is_dir(),
        "the unmeasured slot was evicted on a partial walk"
    );
    assert_eq!(snapshot_tree(&unmeasured_path), unmeasured_before);
    let view = managed::ledger_view(&pool).expect("ledger view");
    // The unmeasured slot is still a member: it is kept and never the victim.
    assert!(
        view.slots
            .iter()
            .any(|slot| slot.slot_id == unmeasured.to_string()),
        "the unmeasured slot left the ledger"
    );
    // The verified idle slot is the only possible victim: if this admission
    // evicted at all, that slot is the one that went and its row went with it.
    if !recorded_path.exists() {
        assert!(
            recorded_of(&view, &recorded.slot_id).is_none(),
            "the evicted slot kept its ledger row"
        );
    }
}

#[test]
fn a_conservative_charge_that_cannot_fit_refuses_only_that_admission() {
    let fx = Fixture::new("unmeasured-refusal");
    let cap = 1024 * 1024;
    let (pool, unmeasured, unmeasured_path, workspace) = released_idle_slot(&fx, "refusal", cap);
    // An over-cap slot is a legal soft-cap state, and its closed record is the
    // conservative stand-in the unmeasured slot is charged with. Evicting it
    // could not bring the figure under the cap, so it must not be deleted.
    let over = lease_injected(request(&pool, &fx.dir("over-project"), &workspace, "over"))
        .expect("over-cap slot");
    fs::write(
        over.env.cargo_build_build_dir.join("blob"),
        vec![b'x'; 1536 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &over.lease_id).expect("release");
    let view = managed::ledger_view(&pool).expect("ledger view");
    let (recorded_bytes, current) = recorded_of(&view, &over.slot_id).expect("the over-cap record");
    assert!(current);
    assert!(recorded_bytes >= cap);
    super::pool::drop_record_for_test(&pool, unmeasured).expect("drop the record");
    let over_path = pool.join("payload/slots").join(&over.slot_id);
    let unmeasured_before = snapshot_tree(&unmeasured_path);
    let over_before = snapshot_tree(&over_path);
    let refused = elapsed_admission(&fx, &unmeasured_path, || {
        super::pool::lease_with_charges_injected(request(
            &pool,
            &fx.dir("other-project"),
            &workspace,
            "other",
        ))
    })
    .expect_err("a charge that cannot fit refuses this admission");
    let view = managed::ledger_view(&pool).expect("ledger view");
    let generation = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == unmeasured.to_string())
        .expect("the unmeasured slot is still in the ledger");
    match &refused {
        LeaseError::CapacityCharged {
            measured,
            limit,
            unmeasured: named,
        } => {
            assert_eq!(*limit, Some(cap));
            assert!(
                measured.is_some_and(|measured| measured >= cap),
                "the refusal does not report the charged figure: {measured:?}"
            );
            assert_eq!(named.len(), 1, "the refusal names exactly the charged slot");
            assert_eq!(named[0].slot_id, unmeasured.to_string());
            assert_eq!(
                named[0].incarnation, generation.incarnation,
                "the reported slot is not bound to the generation that was walked"
            );
            assert_eq!(named[0].dir, generation.dir);
            let text = refused.to_string();
            assert!(
                text.contains(&format!("slot(s) {unmeasured}")),
                "the refusal does not name the unmeasured slot: {text}"
            );
            assert!(text.len() < 400, "the refusal is not bounded: {text}");
        }
        other => panic!("expected a bounded capacity refusal, got {other:?}"),
    }
    assert_eq!(refused.reason(), "full");
    assert_eq!(refused.exit_code(), 3);
    assert_eq!(
        snapshot_tree(&unmeasured_path),
        unmeasured_before,
        "the unmeasured slot was touched"
    );
    assert!(
        over_path.is_dir(),
        "a verified idle slot was deleted for a charge that cannot fit"
    );
    assert_eq!(snapshot_tree(&over_path), over_before);
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(recorded_of(&view, &unmeasured.to_string()).is_none());
}

/// At the pool-wide slot ceiling (`MAX_SLOTS`) the admission can only proceed
/// by retiring a verified idle whole slot. When the only idle slot is one a
/// partial walk left unmeasured, the ceiling cannot be met: that admission
/// alone refuses with a bounded capacity reason naming the slot, nothing is
/// deleted or allocated, and the ledger is byte-identical. With every slot
/// measured the shipped `LedgerExhausted` stands (the control in the same
/// fixture).
#[test]
fn a_slot_ceiling_refusal_names_the_unmeasured_idle_slot() {
    let fx = Fixture::new("ceiling-unmeasured");
    let pool = fx.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    let workspace = fx.dir("workspace");
    // One write fills the table with held slots: at the ceiling no verified
    // idle victim exists, which is the shipped ceiling refusal.
    let (last_id, last_lease) =
        super::pool::fill_held_slot_table_for_test(&pool).expect("a full held slot table");
    let ledger_full = fs::read(pool.join("ledger.json")).expect("stored ledger");

    // Control: every slot is measured (held slots are walked fresh), so the
    // ceiling keeps the shipped `LedgerExhausted` refusal and changes nothing.
    let exhausted = lease_injected(request(
        &pool,
        &fx.dir("extra-project"),
        &workspace,
        "extra",
    ));
    assert!(
        matches!(exhausted, Err(LeaseError::LedgerExhausted)),
        "{exhausted:?}"
    );
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("stored ledger"),
        ledger_full
    );
    assert_eq!(
        status_injected(&pool).expect("status").slots.len(),
        super::MAX_SLOTS
    );

    // The last slot becomes the only idle one, and its record is taken away so
    // admission must walk it. That walk stops at its elapsed bound.
    release_injected(&pool, &last_lease).expect("release the last slot");
    super::pool::drop_record_for_test(&pool, last_id).expect("drop the record");
    let idle_id = last_id.to_string();
    let idle_path = pool.join("payload").join("slots").join(&idle_id);
    let ledger_before = fs::read(pool.join("ledger.json")).expect("stored ledger");
    let payload_before = snapshot_tree(&idle_path);
    let refused = crate::inventory::walk_observer::with_mutations(
        fx.path(),
        serde_json::json!([{
            "path": idle_path, "action": "delay_before_read_dir", "delay_ms": 300
        }])
        .to_string(),
        || {
            with_accounting_limits(2_000_000, 64, 100, || {
                super::pool::lease_with_charges_injected(request(
                    &pool,
                    &fx.dir("next-project"),
                    &workspace,
                    "next",
                ))
            })
        },
    )
    .expect_err("the ceiling refuses this admission");
    match &refused {
        LeaseError::CapacityCharged {
            measured,
            limit,
            unmeasured: named,
        } => {
            assert_eq!(*measured, None, "a count refusal claimed a byte figure");
            assert_eq!(*limit, None, "a count refusal claimed a byte budget");
            assert_eq!(named.len(), 1, "the refusal names more than the kept slot");
            assert_eq!(named[0].slot_id, idle_id);
            let generation = managed::ledger_view(&pool)
                .expect("ledger view")
                .slots
                .iter()
                .find(|slot| slot.slot_id == idle_id)
                .cloned()
                .expect("the unmeasured slot is still in the ledger");
            assert_eq!(named[0].incarnation, generation.incarnation);
            assert_eq!(named[0].dir, generation.dir);
            let text = refused.to_string();
            assert!(
                text.contains(&format!("slot(s) {idle_id}")),
                "the refusal does not name the unmeasured slot: {text}"
            );
            assert!(text.len() < 400, "the refusal is not bounded: {text}");
        }
        other => panic!("expected a bounded capacity refusal, got {other:?}"),
    }
    assert_eq!(refused.reason(), "full");
    assert_eq!(refused.exit_code(), 3);
    // Nothing was deleted, no slot was added and no record was published.
    assert_eq!(
        fs::read(pool.join("ledger.json")).expect("stored ledger"),
        ledger_before,
        "the refused admission wrote the ledger"
    );
    assert_eq!(snapshot_tree(&idle_path), payload_before);
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert_eq!(view.slots.len(), super::MAX_SLOTS);
    assert!(recorded_of(&view, &idle_id).is_none());
    assert!(view.slots.iter().any(|slot| slot.slot_id == idle_id));
}

/// The front door names an idle slot to retire when a project is at its
/// twelve-slot cap (`PROJECT_SLOT_CAP` is a front-door policy). The library
/// contract that keeps that cap is: a count replacement either retires exactly
/// the named slot and allocates the new one, or the whole admission refuses.
/// It never allocates without the replacement and never substitutes a slot the
/// caller did not choose. `project_replacement_requires_complete_accounting_
/// before_deletion` covers the honoured path; this test covers the refusal a
/// partial walk forces, and asserts the slot count is unchanged either way.
#[test]
fn a_named_count_replacement_refuses_rather_than_exceeding_the_project_cap() {
    // Control: the same request against a slot with a current record retires
    // it and allocates one slot, so the count decision itself still works.
    let control_fx = Fixture::new("replacement-control");
    let (control_pool, control_target, control_path, control_workspace) =
        released_idle_slot(&control_fx, "control", 64 * 1024 * 1024);
    let mut control = request(
        &control_pool,
        &control_fx.dir("control-other-project"),
        &control_workspace,
        "control",
    );
    control.replace_slot = Some(control_target);
    let granted = lease_injected(control).expect("a verified replacement is honoured");
    assert!(
        !control_path.exists(),
        "the verified victim was not retired"
    );
    let view = managed::ledger_view(&control_pool).expect("ledger view");
    assert_eq!(view.slots.len(), 1);
    assert!(view
        .slots
        .iter()
        .any(|slot| slot.slot_id == granted.slot_id));

    let fx = Fixture::new("unmeasured-replacement");
    let (pool, unmeasured, unmeasured_path, workspace) =
        released_idle_slot(&fx, "replacement", 64 * 1024 * 1024);
    super::pool::drop_record_for_test(&pool, unmeasured).expect("drop the record");
    let before = snapshot_tree(&unmeasured_path);
    let mut replacement = request(&pool, &fx.dir("other-project"), &workspace, "other");
    replacement.replace_slot = Some(unmeasured);
    // The caller named this slot to stay at its project cap. Its own partial
    // walk never authorises its deletion (oxide#186), and substituting another
    // slot or admitting without the replacement would break the caller's count
    // decision, so only this admission refuses and names the slot.
    let refused = elapsed_admission(&fx, &unmeasured_path, || {
        super::pool::lease_with_charges_injected(replacement)
    })
    .expect_err("a replacement whose walk was partial refuses this admission");
    match &refused {
        LeaseError::CapacityCharged {
            measured,
            limit,
            unmeasured: named,
        } => {
            assert_eq!(*measured, None, "a count refusal claimed a byte figure");
            assert_eq!(*limit, None, "a count refusal claimed a byte budget");
            assert_eq!(named.len(), 1);
            assert_eq!(named[0].slot_id, unmeasured.to_string());
            let text = refused.to_string();
            assert!(
                text.starts_with("cannot admit a new slot: unmeasured slot(s)"),
                "{text}"
            );
            assert!(
                text.contains(&format!("slot(s) {unmeasured}")),
                "the refusal does not name the unmeasured slot: {text}"
            );
        }
        other => panic!("expected a bounded capacity refusal, got {other:?}"),
    }
    assert_eq!(refused.reason(), "full");
    assert_eq!(refused.exit_code(), 3);
    assert!(
        unmeasured_path.is_dir(),
        "a named replacement deleted a slot whose walk was partial"
    );
    assert_eq!(snapshot_tree(&unmeasured_path), before);
    let view = managed::ledger_view(&pool).expect("ledger view");
    // Nothing was allocated and nothing was deleted: the project cannot gain a
    // slot beyond the ceiling the caller asked to respect.
    assert_eq!(
        view.slots.len(),
        1,
        "the refused admission still added or removed a slot"
    );
    assert!(view.slots.iter().any(|slot| {
        slot.slot_id == unmeasured.to_string() && slot.state == SlotOccupancy::Idle
    }));
    assert!(recorded_of(&view, &unmeasured.to_string()).is_none());
}

#[test]
fn a_corrupt_ledger_still_refuses_globally_even_where_a_walk_was_partial() {
    let fx = Fixture::new("unmeasured-corrupt");
    let (pool, slot_id, slot_path, workspace) =
        released_idle_slot(&fx, "corrupt", 64 * 1024 * 1024);
    super::pool::drop_record_for_test(&pool, slot_id).expect("drop the record");
    let before = snapshot_tree(&slot_path);
    let stored = fs::read(pool.join("ledger.json")).expect("stored ledger");
    fs::write(pool.join("ledger.json"), b"{ not a ledger").expect("corrupt ledger");
    let refused = elapsed_admission(&fx, &slot_path, || {
        super::pool::lease_with_charges_injected(request(
            &pool,
            &fx.dir("other-project"),
            &workspace,
            "other",
        ))
    });
    fs::write(pool.join("ledger.json"), &stored).expect("restore the ledger");
    assert!(
        matches!(refused, Err(LeaseError::UnknownAccounting)),
        "a corrupt ledger was not refused globally: {refused:?}"
    );
    assert_eq!(snapshot_tree(&slot_path), before);
}

#[test]
fn the_recorded_sum_counts_cross_slot_hard_links_once_per_slot() {
    use crate::inventory::walk_observer;

    let fx = Fixture::new("recorded-hardlink");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");

    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    let a_blob = a.env.cargo_build_build_dir.join("blob");
    fs::write(&a_blob, vec![b'h'; 3 * 1024 * 1024]).expect("a blob");
    release_injected(&pool, &a.lease_id).expect("release a");

    let b = lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b")).expect("b");
    // The same inode, linked into the second slot: one inode, two slots.
    fs::hard_link(&a_blob, b.env.cargo_build_build_dir.join("blob")).expect("hard link");
    release_injected(&pool, &b.lease_id).expect("release b");

    let view = managed::ledger_view(&pool).expect("ledger view");
    let sum: u64 = view
        .slots
        .iter()
        .filter_map(|slot| slot.recorded.filter(|fact| fact.current))
        .map(|fact| fact.unique_allocated_bytes)
        .sum();
    let union = status_injected(&pool)
        .expect("status")
        .payload_unique_allocated_bytes
        .expect("complete union walk");
    assert!(
        sum >= union,
        "the recorded sum ({sum}) is below the inode union ({union})"
    );
    assert!(
        sum > union,
        "the accepted cross-slot overcount did not happen: sum {sum}, union {union}"
    );

    // The overcount is what admission uses: a budget between the union and the
    // sum evicts one idle whole slot, and it does so without any walk.
    let budget = union + 1024 * 1024;
    let mut small = request(&pool, &fx.dir("project-c"), &workspace, "c");
    small.budget_bytes = Some(budget);
    let (third, walks) = walk_observer::observe(|| lease_publishing(&pool, small).0);
    let third = third.expect("admission under recorded byte pressure");
    assert!(!third.reused_slot);
    assert!(
        walks.is_empty(),
        "byte pressure walked inventory: {walks:?}"
    );
    let after = managed::ledger_view(&pool).expect("ledger view");
    let ids: Vec<&str> = after
        .slots
        .iter()
        .map(|slot| slot.slot_id.as_str())
        .collect();
    assert!(
        !(ids.contains(&a.slot_id.as_str()) && ids.contains(&b.slot_id.as_str())),
        "the accepted overcount did not cause the earlier idle eviction"
    );
}

#[test]
fn disjoint_recorded_slots_are_not_double_counted_by_the_scope_pass() {
    let fx = Fixture::new("recorded-disjoint");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");

    // One labelled slot and one legacy `<id>` slot, so both stop-predicate
    // branches are live: a scope pass that failed to stop would add their
    // content a second time and evict.
    let mut labelled = request(&pool, &fx.dir("project-a"), &workspace, "a");
    labelled.layout = Some(SlotLayout {
        folders: vec!["oxide-aa".to_owned()],
        profile: "quick".to_owned(),
        same_repository: Vec::new(),
    });
    let a = lease_injected(labelled).expect("labelled slot");
    fs::write(
        a.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 2 * 1024 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &a.lease_id).expect("release a");

    let b = lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b")).expect("legacy");
    fs::write(
        b.env.cargo_build_build_dir.join("blob"),
        vec![b'b'; 2 * 1024 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &b.lease_id).expect("release b");

    let union = status_injected(&pool)
        .expect("status")
        .payload_unique_allocated_bytes
        .expect("complete union");
    // A budget one MiB above the union is above the recorded sum too, unless
    // the scope pass counted slot content again.
    let mut third = request(&pool, &fx.dir("project-c"), &workspace, "c");
    third.budget_bytes = Some(union + 1024 * 1024);
    let third = lease_injected(third).expect("admission");
    assert!(!third.reused_slot);
    let after = managed::ledger_view(&pool).expect("ledger view");
    assert_eq!(
        after.slots.len(),
        3,
        "a slot was evicted under a budget above the union, so slot content was counted twice"
    );
}

#[test]
fn an_out_of_slot_stray_counts_and_grants_no_ownership() {
    let fx = Fixture::new("recorded-stray");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");
    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    fs::write(
        a.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 1024 * 1024],
    )
    .expect("blob");
    release_injected(&pool, &a.lease_id).expect("release a");

    // A stray file directly under `slots/`, plus tolerated Finder metadata.
    let slots_root = pool.join("payload").join("slots");
    fs::write(slots_root.join("stray.bin"), vec![b's'; 3 * 1024 * 1024]).expect("stray");
    fs::write(slots_root.join(".DS_Store"), vec![b'd'; 4096]).expect("finder metadata");

    let union = status_injected(&pool)
        .expect("status")
        .payload_unique_allocated_bytes
        .expect("complete union");
    // The stray is counted by the scope pass, so a budget just under the union
    // puts admission under byte pressure and it evicts the only idle slot.
    let mut second = request(&pool, &fx.dir("project-b"), &workspace, "b");
    second.budget_bytes = Some(union.saturating_sub(512 * 1024));
    let second = lease_publishing(&pool, second).0.expect("admission");
    assert!(!second.reused_slot);
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        !view.slots.iter().any(|slot| slot.slot_id == a.slot_id),
        "out-of-slot bytes were not counted, so no pressure evicted the idle slot"
    );
    assert!(
        slots_root.join("stray.bin").exists(),
        "the stray is not owned and must survive"
    );
}

#[test]
fn an_incomplete_release_walk_records_nothing_and_never_zero() {
    let fx = Fixture::new("recorded-incomplete");
    let pool = fx.dir("pool");
    let cap = 64 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let workspace = fx.dir("workspace");
    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    fs::write(
        a.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 1024 * 1024],
    )
    .expect("blob");

    // An entry bound of 1 cannot complete the slot's own walk.
    with_accounting_limits(1, 64, 60_000, || {
        release_injected(&pool, &a.lease_id).expect("release still succeeds")
    });
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        recorded_of(&view, &a.slot_id).is_none(),
        "an incomplete release walk published a figure"
    );

    // The next admission must refuse, never read the slot as zero bytes.
    let scan = with_accounting_limits(1, 64, 60_000, || {
        incomplete(lease_injected(request(
            &pool,
            &fx.dir("project-b"),
            &workspace,
            "b",
        )))
    });
    assert!(gap_kinds(&scan).contains(&AccountingGapKind::EntryLimit));
    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        recorded_of(&view, &a.slot_id).is_none(),
        "a refused admission published a figure"
    );
}

// ---------------------------------------------------------------------------
// Bounded Finder-metadata races in owned-tree deletion (oxide#155)
// ---------------------------------------------------------------------------

/// A recorded `Evicting` slot whose directory races Finder metadata is
/// reconciled at the next open, and its record leaves the ledger only after
/// that positive removal.
#[test]
fn an_evicting_slot_raced_by_finder_metadata_is_reconciled() {
    let fx = Fixture::new("race-evicting");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    fs::write(
        leased.env.cargo_build_build_dir.join("blob"),
        vec![7_u8; 4096],
    )
    .expect("blob");
    release_injected(&pool, &leased.lease_id).expect("release");
    let slot = leased.slot_id.parse::<u64>().expect("slot id");
    mark_evicting_for_test(&pool, slot).expect("mark evicting");
    let slot_path = pool.join("payload/slots").join(&leased.slot_id);

    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "metadata", false);
    let after = status_injected(&pool).expect("status");
    assert!(
        after.slots.is_empty(),
        "the reconciled slot leaves the ledger"
    );
    assert!(!slot_path.exists(), "the raced recorded deletion completed");
}

/// The falsifying control for that test: the *following* repair this code used
/// to perform, applied to the same raced state, does chmod the out-of-tree
/// referent. The no-follow refusal above is therefore not vacuous.
#[test]
fn the_previous_following_repair_would_have_chmodded_the_referent() {
    let fixture = Fixture::new("repair-falsification");
    let outside = fixture.dir("outside");
    let referent = outside.join("referent");
    fs::create_dir(&referent).expect("referent");
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o000)).expect("referent mode");
    let slot = fixture.dir("slot");
    let locked = slot.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");

    // Exactly the raced state: the name is rebound to a link between the entry
    // being verified and the repair.
    fs::rename(&locked, slot.join("locked-moved")).expect("rename away");
    symlink(&referent, &locked).expect("replacement link");
    // The old repair was `fs::set_permissions(path, mode | 0o700)`, which
    // follows a link at that path.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("old repair");
    assert_eq!(
        fs::symlink_metadata(&referent)
            .expect("referent metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700,
        "the falsifying control did not reproduce the out-of-tree chmod"
    );
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
    fs::set_permissions(slot.join("locked-moved"), fs::Permissions::from_mode(0o700))
        .expect("fixture cleanup");
}

// ---------------------------------------------------------------------------
// Intent-first root-capture journal and two-name recovery (oxide#155).
// ---------------------------------------------------------------------------

/// One idle whole slot in a fresh budget pool, ready for a recorded deletion.
fn idle_slot_for_eviction(fixture: &Fixture, label: &str) -> (PathBuf, u64, PathBuf) {
    let pool = fixture.dir(&format!("{label}-pool"));
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fixture.dir(&format!("{label}-project"));
    let workspace = fixture.dir(&format!("{label}-workspace"));
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    fs::write(
        leased.env.cargo_build_build_dir.join("blob"),
        vec![7_u8; 4096],
    )
    .expect("blob");
    release_injected(&pool, &leased.lease_id).expect("release");
    let slot_id = leased.slot_id.parse::<u64>().expect("slot id");
    let slot_path = pool.join("payload/slots").join(&leased.slot_id);
    (pool, slot_id, slot_path)
}

/// Every capture-named entry directly inside `parent`.
fn capture_names(parent: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(parent)
        .expect("parent listed")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(".oxide-capture-").then_some(name)
        })
        .collect();
    names.sort();
    names
}

fn mode_of(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o7777
}

/// A journal that records what the filesystem held at each step, so the
/// ordering is asserted from the real deletion rather than from a mirror of it.
struct RecordingJournal<'a> {
    slot: &'a Path,
    events: Vec<(
        &'static str,
        crate::owned_delete::RootCapturePlan,
        bool,
        usize,
    )>,
    /// Every per-capture and root-removal record, in the order it was written.
    records: Vec<String>,
    /// The live mode of each recorded capture at the moment it was recorded.
    seen_modes: Vec<(String, u32)>,
}

impl crate::owned_delete::RootCaptureJournal for RecordingJournal<'_> {
    fn record_captured(
        &mut self,
        record: &crate::owned_delete::CaptureRecord,
    ) -> Result<(), String> {
        self.records
            .push(format!("captured {} {}", record.original, record.private));
        let mode = find_entry(self.slot.parent().expect("fixture"), &record.private)
            .and_then(|path| fs::symlink_metadata(path).ok())
            .map(|metadata| metadata.permissions().mode() & 0o7777)
            .unwrap_or(u32::MAX);
        self.seen_modes.push((record.original.clone(), mode));
        Ok(())
    }

    fn record_removed(
        &mut self,
        record: &crate::owned_delete::CaptureRecord,
    ) -> Result<(), String> {
        self.records
            .push(format!("removed {} {}", record.original, record.private));
        Ok(())
    }

    fn record_foreign(
        &mut self,
        capture: &crate::owned_delete::ForeignCapture,
    ) -> Result<(), String> {
        self.records.push(format!(
            "foreign {} restored={}",
            capture.private, capture.restored
        ));
        Ok(())
    }

    fn record_root_removed(
        &mut self,
        _plan: &crate::owned_delete::RootCapturePlan,
    ) -> Result<(), String> {
        self.records.push("root removed".to_owned());
        Ok(())
    }

    fn planned(&mut self, plan: &crate::owned_delete::RootCapturePlan) -> Result<(), String> {
        let parent = self.slot.parent().expect("slot parent");
        self.events.push((
            "planned",
            plan.clone(),
            self.slot.exists(),
            capture_names(parent).len(),
        ));
        Ok(())
    }

    fn captured(&mut self, plan: &crate::owned_delete::RootCapturePlan) -> Result<(), String> {
        let parent = self.slot.parent().expect("slot parent");
        self.events.push((
            "captured",
            plan.clone(),
            self.slot.exists(),
            capture_names(parent).len(),
        ));
        Ok(())
    }
}

/// A crash after the rename but before its phase is recorded: the record names
/// the planned private name and the original name is free. Recovery finds the
/// captured root under the recorded private name, puts it back and finishes.
/// Model one crash state through the same primitives the product uses: plan and
/// capture the slot root, then write the journal phase the crashed run had
/// reached. Returns the plan.
fn capture_root_state(
    slot_path: &Path,
    slot_id: u64,
    pool: &Path,
    captured_phase: bool,
) -> crate::owned_delete::RootCapturePlan {
    mark_evicting_for_test(pool, slot_id).expect("recorded deletion");
    let parent_path = slot_path.parent().expect("slot parent").to_path_buf();
    let held = File::open(&parent_path).expect("parent descriptor");
    let leaf = slot_path.file_name().expect("slot leaf");
    let name = std::ffi::CString::new(leaf.as_encoded_bytes()).expect("leaf name");
    let observed = crate::owned_delete::observe_entry(held.as_fd(), &name).expect("observed root");
    let plan = crate::owned_delete::plan_root_capture(held.as_fd(), &observed).expect("plan");
    write_journal(
        pool,
        slot_id,
        plan.clone(),
        captured_phase,
        Vec::new(),
        Vec::new(),
    );
    crate::owned_delete::capture_planned(held.as_fd(), &name, &plan).expect("captured");
    plan
}

/// Write one modelled journal into the ledger.
fn write_journal(
    pool: &Path,
    slot_id: u64,
    plan: crate::owned_delete::RootCapturePlan,
    captured_phase: bool,
    captures: Vec<crate::owned_delete::CaptureRecord>,
    foreign: Vec<crate::owned_delete::ForeignCapture>,
) {
    let root = crate::owned_delete::RootCapturePlan {
        phase: if captured_phase {
            crate::owned_delete::RootCapturePhase::Captured
        } else {
            crate::owned_delete::RootCapturePhase::Planned
        },
        ..plan
    };
    super::pool::write_capture_record_injected(
        pool,
        slot_id,
        super::pool::DeletionJournal {
            root,
            captures,
            foreign,
        },
    )
    .expect("journal written");
}

/// A crash after the rename but before its phase is recorded: the record names
/// the planned private name and the original name is free. Recovery finds the
/// captured root under the recorded private name, puts it back and finishes.
#[test]
fn a_crash_between_the_rename_and_its_phase_recovers_the_capture() {
    let fixture = Fixture::new("crash-renamed");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "crash-renamed");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, false);
    assert!(!slot_path.exists(), "the root left its recorded name");
    assert_eq!(
        capture_names(&parent),
        vec![plan.private.clone()],
        "the recorded private name holds the captured root"
    );

    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded deletion completed");
    assert!(!slot_path.exists(), "the slot is gone");
    assert!(
        capture_names(&parent).is_empty(),
        "the capture name is gone"
    );
}

/// A recorded root removal is the only completion proof: with the root already
/// deleted and its `removed` phase durable the record goes; without that phase
/// the same two absent names stay pending, because they may be a move.
#[test]
fn only_a_recorded_root_removal_completes_the_deletion() {
    for recorded in [false, true] {
        let label = if recorded {
            "crash-removed-post"
        } else {
            "crash-removed-pre"
        };
        let fixture = Fixture::new(label);
        let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, label);
        let parent = slot_path.parent().expect("slot parent").to_path_buf();
        let plan = capture_root_state(&slot_path, slot_id, &pool, true);
        fs::remove_dir_all(parent.join(&plan.private)).expect("the captured tree is gone");
        let root = crate::owned_delete::RootCapturePlan {
            phase: if recorded {
                crate::owned_delete::RootCapturePhase::Removed
            } else {
                crate::owned_delete::RootCapturePhase::Captured
            },
            ..plan
        };
        super::pool::write_capture_record_injected(
            &pool,
            slot_id,
            super::pool::DeletionJournal {
                root,
                captures: Vec::new(),
                foreign: Vec::new(),
            },
        )
        .expect("journal written");

        let after = status_injected(&pool).expect("status");
        if recorded {
            assert!(after.slots.is_empty(), "the recorded removal completed");
            assert!(
                super::pool::capture_record_injected(&pool, slot_id).is_none(),
                "the record goes"
            );
        } else {
            assert_eq!(
                after.slots.len(),
                1,
                "two absent names without a recorded removal are never completion"
            );
            assert_eq!(after.coverage, AccountingCoverage::Unknown);
            assert!(
                super::pool::capture_record_injected(&pool, slot_id).is_some(),
                "the record is retained"
            );
        }
    }
}

/// A recorded mismatch keeps the deletion pending: replay never removes a name
/// the foreign-capture record names, whatever the two names now hold.
#[test]
fn a_recorded_foreign_capture_keeps_the_deletion_pending() {
    let fixture = Fixture::new("crash-foreign");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "crash-foreign");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::write(captured.join("blob"), b"x").expect("content");
    let foreign = crate::owned_delete::ForeignCapture {
        path: Vec::new(),
        private: ".oxide-capture-fedcba9876543210".to_owned(),
        original: "blob".to_owned(),
        expected_device: plan.device,
        expected_inode: plan.inode,
        expected_mode: u32::from(libc::S_IFREG) | 0o600,
        found: "directory device 1 inode 2 mode 40700 owner 501".to_owned(),
        restored: false,
    };
    write_journal(&pool, slot_id, plan, true, Vec::new(), vec![foreign]);

    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "a recorded mismatch keeps it pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(slot_path.is_dir(), "the recorded root was put back");
    assert!(
        captured.exists() == false && capture_names(&parent).is_empty(),
        "the recorded root's private name is resolved"
    );
    assert!(
        super::pool::capture_record_injected(&pool, slot_id).is_some(),
        "the record is retained"
    );
}

/// The first entry named `name` anywhere below `dir`, for a test observation.
fn find_entry(dir: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy() == name {
            return Some(entry.path());
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if let Some(found) = find_entry(&entry.path(), name) {
                return Some(found);
            }
        }
    }
    None
}

/// Remove every private-name file below `dir` (the modelled live vanish).
fn remove_private_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_private = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"));
        if path.is_dir() {
            remove_private_files(&path);
        } else if is_private {
            let _ = fs::remove_file(&path);
        }
    }
}

/// The private capture names directly inside `dir`, files or directories.
fn private_names_in(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .expect("directory listed")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"))
        })
        .collect()
}

/// The incident journal has a root, build/debug/deps captures, and one
/// unlinked file whose removal has not yet been dropped from the journal.
#[test]
fn replay_concludes_an_unlinked_file_in_the_2255_capture_shape() {
    let fixture = Fixture::new("nested-file-2255");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "nested-file-2255");
    let parent = slot_path.parent().expect("parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let mut directory = parent.join(&plan.private);
    let mut chain = Vec::new();
    let mut records = Vec::new();
    for name in [
        "build",
        "debug",
        "deps",
        "arrow_schema-8fafd6ba14ed7d09.arrow_schema.ec65cfc77a09ada8-cgu.13.rcgu.o",
    ] {
        let leaf = directory.join(name);
        if name.ends_with(".o") {
            fs::write(&leaf, b"fixture").expect("file");
        } else if !leaf.exists() {
            fs::create_dir(&leaf).expect("directory");
        }
        let mode = if name == "build" {
            0o700
        } else if name.ends_with(".o") {
            0o644
        } else {
            0o755
        };
        fs::set_permissions(&leaf, fs::Permissions::from_mode(mode)).expect("incident mode");
        let held = File::open(&directory).expect("directory descriptor");
        let original = std::ffi::CString::new(name).expect("name");
        let capture = crate::owned_delete::capture_entry(held.as_fd(), &original).expect("capture");
        let facts =
            crate::owned_delete::observe_entry(held.as_fd(), &capture.private).expect("facts");
        let private = String::from_utf8(capture.private.as_bytes().to_vec()).expect("private name");
        records.push(crate::owned_delete::CaptureRecord {
            path: chain.clone(),
            private: private.clone(),
            original: name.to_owned(),
            device: facts.device,
            inode: facts.inode,
            mode: facts.mode as u32,
        });
        directory.push(&private);
        chain.push(private);
    }
    fs::remove_file(&directory).expect("modelled unlink");
    write_journal(&pool, slot_id, plan, true, records, Vec::new());
    let after = status_injected(&pool).expect("replay");
    assert!(after.slots.is_empty());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
}

/// A missing captured directory is never treated as an unlinked file.
#[test]
fn an_absent_nested_directory_stays_pending() {
    let fixture = Fixture::new("absent-captured-directory");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "absent-captured-directory");
    let parent = slot_path.parent().expect("parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(captured.join("nested")).expect("nested directory");
    let held = File::open(&captured).expect("captured root descriptor");
    let name = std::ffi::CString::new("nested").expect("name");
    let entry = crate::owned_delete::capture_entry(held.as_fd(), &name).expect("capture");
    let facts = crate::owned_delete::observe_entry(held.as_fd(), &entry.private).expect("facts");
    let private = String::from_utf8(entry.private.as_bytes().to_vec()).expect("private name");
    let record = crate::owned_delete::CaptureRecord {
        path: Vec::new(),
        private: private.clone(),
        original: "nested".to_owned(),
        device: facts.device,
        inode: facts.inode,
        mode: facts.mode as u32,
    };
    fs::remove_dir(captured.join(&private)).expect("modelled missing directory");
    write_journal(&pool, slot_id, plan, true, vec![record], Vec::new());
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1);
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(captured.is_dir());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
}

/// A recorded nested capture whose put-back collides (a racer took its observed
/// name) keeps both names, the record and the refusal; nothing is removed.
#[test]
fn a_nested_put_back_collision_keeps_both_names_and_the_record() {
    let fixture = Fixture::new("nested-collision");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "nested-collision");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(captured.join("nested")).expect("nested directory");
    let held = File::open(&captured).expect("captured root descriptor");
    let nested = std::ffi::CString::new("nested").expect("name");
    let entry = crate::owned_delete::capture_entry(held.as_fd(), &nested).expect("nested capture");
    let facts = crate::owned_delete::observe_entry(held.as_fd(), &entry.private).expect("facts");
    let record = crate::owned_delete::CaptureRecord {
        path: Vec::new(),
        private: String::from_utf8_lossy(entry.private.as_bytes()).into_owned(),
        original: "nested".to_owned(),
        device: facts.device,
        inode: facts.inode,
        mode: facts.mode as u32,
    };
    // A racer takes the observed name back with a different object.
    fs::write(captured.join("nested"), b"racer").expect("collision");
    write_journal(&pool, slot_id, plan, true, vec![record.clone()], Vec::new());

    let after = status_injected(&pool).expect("status");
    assert_eq!(
        after.slots.len(),
        1,
        "a collision keeps the deletion pending"
    );
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    // The rollback stopped at the collision, so neither name was consumed: the
    // root is still under its recorded private name and the racer's object is
    // untouched under the capture's observed name.
    assert!(
        !slot_path.exists(),
        "no put-back happened past the collision"
    );
    assert!(
        captured.join("nested").is_file(),
        "the racer's object is untouched"
    );
    assert!(
        captured.join(&record.private).is_dir(),
        "the recorded capture is kept under its private name"
    );
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
}

/// A blocked deletion keeps its own slot and its closed size, while a
/// different project can obtain a fresh slot under the remaining budget.
#[test]
fn blocked_eviction_is_charged_but_does_not_block_other_admissions() {
    let fixture = Fixture::new("blocked-isolation");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "blocked-isolation");
    let parent = slot_path.parent().expect("parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(&slot_path).expect("collision");
    let project = fixture.dir("other-project");
    let workspace = fixture.dir("other-workspace");
    let granted =
        lease_injected(request(&pool, &project, &workspace, "other")).expect("unrelated admission");
    assert_ne!(granted.slot_id, slot_id.to_string());
    assert!(captured.is_dir());
    assert!(slot_path.is_dir());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
}

/// The retained recorded charge prevents another allocation when it exhausts
/// the budget; the blocked slot must stay intact and cannot become a victim.
#[test]
fn blocked_eviction_charge_prevents_over_budget_admission() {
    let fixture = Fixture::new("blocked-charge");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "blocked-charge");
    let parent = slot_path.parent().expect("parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(&slot_path).expect("collision");
    let project = fixture.dir("other-project");
    let workspace = fixture.dir("other-workspace");
    let mut req = request(&pool, &project, &workspace, "other");
    req.budget_bytes = Some(1);
    assert!(matches!(lease_injected(req), Err(LeaseError::Full)));
    assert!(captured.is_dir());
    assert!(slot_path.is_dir());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
}

/// A pending root without a capture journal cannot be distinguished from a
/// replacement when its contents are unreadable. It is isolated and charged,
/// while an independent unreadable subtree remains an accounting error.
#[test]
fn an_unjournaled_inaccessible_eviction_is_a_conservative_refusal() {
    for unrelated in [false, true] {
        let fixture = Fixture::new(if unrelated {
            "pending-other-gap"
        } else {
            "pending-gap"
        });
        let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "pending");
        mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
        // The non-private root is refused before a capture journal is made.
        // The inventory hook supplies a deterministic unreadable subtree even
        // on volumes whose mount ignores ordinary owner permissions.
        fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o755))
            .expect("non-private slot");
        let blocked = slot_path.join("blocked");
        fs::create_dir(&blocked).expect("blocked directory");
        let mut faults = vec![serde_json::json!({
            "path": blocked, "action": "permission_denied_before_read_dir"
        })];
        if unrelated {
            let other = pool.join("payload/slots/unrelated");
            fs::create_dir(&other).expect("unrelated directory");
            faults.push(serde_json::json!({
                "path": other, "action": "permission_denied_before_read_dir"
            }));
        }
        let project = fixture.dir("other-project");
        let workspace = fixture.dir("other-workspace");
        let result = crate::inventory::walk_observer::with_mutations(
            fixture.path(),
            serde_json::Value::Array(faults).to_string(),
            || lease_injected(request(&pool, &project, &workspace, "other")),
        );
        if unrelated {
            assert!(
                matches!(result, Err(LeaseError::AccountingIncomplete(_))),
                "{result:?}"
            );
        } else {
            assert!(result.is_ok(), "unrelated admission: {result:?}");
        }
        assert!(slot_path.is_dir(), "pending root was removed");
    }
}

/// A pending unjournaled root that exhausts the scope walk's entry bound is
/// charged by the largest recorded or observed size without changing it.
#[test]
fn an_unjournaled_eviction_over_the_entry_bound_admits_other_work() {
    fn snapshot(path: &Path, base: &Path, entries: &mut Vec<(PathBuf, u32, Vec<u8>)>) {
        let mut children: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        children.sort();
        for child in children {
            let metadata = fs::symlink_metadata(&child).unwrap();
            let content = if metadata.is_file() {
                fs::read(&child).unwrap()
            } else {
                Vec::new()
            };
            entries.push((
                child.strip_prefix(base).unwrap().to_path_buf(),
                metadata.mode(),
                content,
            ));
            if metadata.is_dir() {
                snapshot(&child, base, entries);
            }
        }
    }
    let fixture = Fixture::new("pending-entry-bound");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "pending");
    mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o755)).expect("non-private slot");
    for index in 0..40 {
        fs::write(slot_path.join(format!("entry-{index}")), b"x").expect("fixture entry");
    }
    let project = fixture.dir("other-project");
    let workspace = fixture.dir("other-workspace");
    let mut before = Vec::new();
    let root_mode = fs::symlink_metadata(&slot_path).unwrap().mode();
    snapshot(&slot_path, &slot_path, &mut before);
    let mutation = serde_json::json!([{
        "path": slot_path, "action": "remove_before_file_type"
    }]);
    crate::inventory::walk_observer::with_mutations(fixture.path(), mutation.to_string(), || {
        with_accounting_limits(20, 64, 60_000, || {
            let (charge, largest, observed) =
                super::pool::pending_charge_injected(&pool, slot_id).unwrap();
            assert_eq!(charge, largest.max(observed));
            let result = lease_injected(request(&pool, &project, &workspace, "other"));
            assert!(result.is_ok(), "unrelated admission: {result:?}");
        });
    });
    let mut after = Vec::new();
    snapshot(&slot_path, &slot_path, &mut after);
    assert_eq!(before, after);
    assert_eq!(fs::symlink_metadata(&slot_path).unwrap().mode(), root_mode);
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
}

/// A replacement at the recorded name stays untouched while the recorded
/// pending root remains isolated; replacement sizing is outside this charge.
#[test]
fn blocked_capture_collision_stays_untouched() {
    let fixture = Fixture::new("blocked-collision-bytes");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "blocked-collision-bytes");
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    fs::create_dir(&slot_path).expect("collision");
    fs::write(slot_path.join("foreign"), vec![1_u8; 9 * 1024 * 1024]).expect("foreign bytes");
    let project = fixture.dir("other-project");
    let workspace = fixture.dir("other-workspace");
    let result = lease_injected(request(&pool, &project, &workspace, "other"));
    assert!(result.is_ok(), "unrelated admission: {result:?}");
    assert!(slot_path.join("foreign").is_file());
    assert!(slot_path
        .parent()
        .expect("parent")
        .join(plan.private)
        .is_dir());
}

/// A descendant that cannot be resolved after a **nested** directory was
/// widened for the rollback must not leave that directory widened: the replay
/// restores its recorded search-only mode through the verified descriptor,
/// keeps the journal and stays pending.
#[test]
fn a_nested_descendant_failure_restores_the_search_only_ancestor() {
    let fixture = Fixture::new("nested-widen");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "nested-widen");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    // A search-only nested directory, captured, holding one captured child.
    fs::create_dir(captured.join("nested")).expect("nested directory");
    fs::create_dir(captured.join("nested").join("inner")).expect("inner");
    let held = File::open(&captured).expect("captured root descriptor");
    let nested = std::ffi::CString::new("nested").expect("name");
    let outer = crate::owned_delete::capture_entry(held.as_fd(), &nested).expect("nested capture");
    let outer_facts =
        crate::owned_delete::observe_entry(held.as_fd(), &outer.private).expect("outer facts");
    let nested_dir = captured.join(outer.private.to_string_lossy().as_ref());
    let nested_held = File::open(&nested_dir).expect("nested descriptor");
    let inner = std::ffi::CString::new("inner").expect("name");
    let child = crate::owned_delete::capture_entry(nested_held.as_fd(), &inner).expect("inner");
    let child_facts =
        crate::owned_delete::observe_entry(nested_held.as_fd(), &child.private).expect("facts");
    let outer_private = String::from_utf8_lossy(outer.private.as_bytes()).into_owned();
    let records = vec![
        crate::owned_delete::CaptureRecord {
            path: Vec::new(),
            private: outer_private.clone(),
            original: "nested".to_owned(),
            device: outer_facts.device,
            inode: outer_facts.inode,
            // The owner left the ancestor search-only; the record keeps that.
            mode: u32::from(outer_facts.file_type()) | 0o100,
        },
        crate::owned_delete::CaptureRecord {
            path: vec![outer_private.clone()],
            private: String::from_utf8_lossy(child.private.as_bytes()).into_owned(),
            original: "inner".to_owned(),
            device: child_facts.device,
            inode: child_facts.inode,
            mode: child_facts.mode as u32,
        },
    ];
    // The descendant vanishes, so the rollback cannot resolve it, and the owner
    // left the ancestor search-only.
    fs::remove_dir(nested_dir.join(child.private.to_string_lossy().as_ref()))
        .expect("descendant removed");
    write_journal(&pool, slot_id, plan, true, records, Vec::new());
    // The owner left the ancestor search-only, so the replay has to widen it
    // and restore the recorded mode. This window is as short as the product
    // step it tests.
    fs::set_permissions(&nested_dir, fs::Permissions::from_mode(0o100))
        .expect("search-only ancestor");

    let after = status_injected(&pool).expect("status");
    let restored_mode = mode_of(&nested_dir);
    // Give this test's own fixture back its access before anything else: the
    // shared supervisor measures the scratch it owns, and only the oracle above
    // needs the search-only mode.
    make_removable(fixture.path());
    assert_eq!(
        restored_mode, 0o100,
        "the widened ancestor's recorded mode was restored on the error path"
    );
    assert_eq!(
        after.slots.len(),
        1,
        "the unresolved descendant stays pending"
    );
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(
        captured.exists(),
        "the captured tree is kept, not put back past the failure"
    );
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
}

/// The first directory below `root` whose name is a private capture name.
fn find_private_dir(root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_private = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"));
        if is_private && path.is_dir() {
            return Some(path);
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if let Some(found) = find_private_dir(&path) {
                return Some(found);
            }
        }
    }
    None
}

/// The first directory below `root` whose owner mode is exactly `mode`.
fn find_dir_by_mode(root: &Path, mode: u32) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        if metadata.permissions().mode() & 0o7777 == mode {
            return Some(path);
        }
        if let Some(found) = find_dir_by_mode(&path, mode) {
            return Some(found);
        }
    }
    None
}

/// A journaled parent capture that was already put back, holding its own
/// journaled child capture, resolves through the verified **restored** parent:
/// an obstruction keeps it pending, and the next open after the obstruction is
/// removed restores and replays both and completes.
#[test]
fn a_restored_parent_with_a_child_capture_resolves_once_the_obstruction_goes() {
    let fixture = Fixture::new("restored-parent");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "restored-parent");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    // A parent directory and its child, both captured and recorded…
    fs::create_dir(captured.join("par")).expect("parent directory");
    fs::write(captured.join("par").join("inner"), b"x").expect("child");
    let held = File::open(&captured).expect("captured root descriptor");
    let par = std::ffi::CString::new("par").expect("name");
    let outer = crate::owned_delete::capture_entry(held.as_fd(), &par).expect("parent capture");
    let outer_facts =
        crate::owned_delete::observe_entry(held.as_fd(), &outer.private).expect("parent facts");
    let par_dir = captured.join(outer.private.to_string_lossy().as_ref());
    let par_held = File::open(&par_dir).expect("parent descriptor");
    let inner = std::ffi::CString::new("inner").expect("name");
    let child =
        crate::owned_delete::capture_entry(par_held.as_fd(), &inner).expect("child capture");
    let child_facts =
        crate::owned_delete::observe_entry(par_held.as_fd(), &child.private).expect("child facts");
    let outer_private = outer.private.to_string_lossy().into_owned();
    let records = vec![
        crate::owned_delete::CaptureRecord {
            path: Vec::new(),
            private: outer_private.clone(),
            original: "par".to_owned(),
            device: outer_facts.device,
            inode: outer_facts.inode,
            // The owner left the restored parent search-only.
            mode: u32::from(outer_facts.file_type()) | 0o100,
        },
        crate::owned_delete::CaptureRecord {
            path: vec![outer_private.clone()],
            private: child.private.to_string_lossy().into_owned(),
            original: "inner".to_owned(),
            device: child_facts.device,
            inode: child_facts.inode,
            mode: child_facts.mode as u32,
        },
    ];
    // …and both put back before the crash, exactly as a refusal leaves them.
    fs::rename(
        par_dir.join(child.private.to_string_lossy().as_ref()),
        par_dir.join("inner"),
    )
    .expect("child put back");
    fs::rename(&par_dir, captured.join("par")).expect("parent put back");
    // The owner left the restored parent search-only, so a replay that descends
    // through it needs the same identity-bound access repair as a still-private
    // capture and must restore exactly this mode afterwards.
    fs::set_permissions(captured.join("par"), fs::Permissions::from_mode(0o100))
        .expect("search-only restored parent");
    write_journal(&pool, slot_id, plan, true, records, Vec::new());

    // An obstruction on the parent's observed name keeps the replay pending.
    fs::rename(captured.join("par"), captured.join("par-aside")).expect("parent aside");
    fs::create_dir(captured.join("par")).expect("obstruction");
    let blocked = status_injected(&pool).expect("status");
    assert_eq!(blocked.slots.len(), 1, "the obstruction keeps it pending");
    assert_eq!(blocked.coverage, AccountingCoverage::Unknown);
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
    assert_eq!(
        mode_of(&captured.join("par-aside")),
        0o100,
        "the search-only restored parent keeps its recorded mode across the blocked replay"
    );

    // Removing it lets the next open resolve the parent and its child through
    // the verified restored directory, then replay and complete.
    fs::remove_dir(captured.join("par")).expect("obstruction removed");
    fs::rename(captured.join("par-aside"), captured.join("par")).expect("parent restored");
    let after = status_injected(&pool).expect("status");
    assert!(
        after.slots.is_empty(),
        "both recorded captures resolved and the deletion completed"
    );
    assert!(!slot_path.exists());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
}

/// A restored parent's name rebound **after** the repair widened it stops the
/// step pending: the original object still gets its recorded 0100 mode back
/// through the descriptor opened for the step, and the replacement is never
/// modified.
#[test]
fn a_rebound_restored_parent_is_not_chmodded_and_stays_pending() {
    let fixture = Fixture::new("rebound-parent");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "rebound-parent");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(captured.join("par")).expect("parent directory");
    fs::write(captured.join("par").join("inner"), b"x").expect("child");
    let held = File::open(&captured).expect("captured root descriptor");
    let par = std::ffi::CString::new("par").expect("name");
    let outer = crate::owned_delete::capture_entry(held.as_fd(), &par).expect("parent capture");
    let outer_facts =
        crate::owned_delete::observe_entry(held.as_fd(), &outer.private).expect("parent facts");
    let par_dir = captured.join(outer.private.to_string_lossy().as_ref());
    let par_held = File::open(&par_dir).expect("parent descriptor");
    let inner = std::ffi::CString::new("inner").expect("name");
    let child =
        crate::owned_delete::capture_entry(par_held.as_fd(), &inner).expect("child capture");
    let child_facts =
        crate::owned_delete::observe_entry(par_held.as_fd(), &child.private).expect("child facts");
    let outer_private = outer.private.to_string_lossy().into_owned();
    let records = vec![
        crate::owned_delete::CaptureRecord {
            path: Vec::new(),
            private: outer_private.clone(),
            original: "par".to_owned(),
            device: outer_facts.device,
            inode: outer_facts.inode,
            mode: u32::from(outer_facts.file_type()) | 0o100,
        },
        crate::owned_delete::CaptureRecord {
            path: vec![outer_private.clone()],
            private: child.private.to_string_lossy().into_owned(),
            original: "inner".to_owned(),
            device: child_facts.device,
            inode: child_facts.inode,
            mode: child_facts.mode as u32,
        },
    ];
    // Both put back, the parent left search-only, exactly as a refusal leaves it.
    fs::rename(
        par_dir.join(child.private.to_string_lossy().as_ref()),
        par_dir.join("inner"),
    )
    .expect("child put back");
    fs::rename(&par_dir, captured.join("par")).expect("parent put back");
    fs::set_permissions(captured.join("par"), fs::Permissions::from_mode(0o100))
        .expect("search-only restored parent");
    write_journal(&pool, slot_id, plan, true, records, Vec::new());

    // After the repair widened it, a racer takes the name and leaves its own
    // directory there.
    let fixture_path = fixture.path().to_path_buf();
    let _rebind = crate::test_fs::after_restored_parent_widen(move || {
        let restored_parent = find_private_dir(&fixture_path)
            .and_then(|captured| {
                fs::read_dir(&captured)
                    .ok()?
                    .flatten()
                    .map(|entry| entry.path())
                    .find(|path| path.file_name().is_some_and(|name| name == "par"))
            })
            .expect("the restored parent");
        fs::rename(
            &restored_parent,
            restored_parent.with_file_name("par-aside"),
        )
        .expect("the restored parent is moved aside");
        fs::create_dir(&restored_parent).expect("replacement at the observed name");
        fs::set_permissions(&restored_parent, fs::Permissions::from_mode(0o500))
            .expect("replacement mode");
    });
    let after = status_injected(&pool).expect("status");
    drop(_rebind);

    assert_eq!(
        after.slots.len(),
        1,
        "the rebind keeps the deletion pending"
    );
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    let original = find_dir_by_mode(fixture.path(), 0o100).expect("the verified original");
    assert_eq!(
        original.file_name().and_then(|name| name.to_str()),
        Some("par-aside"),
        "the verified original keeps its recorded mode through its own descriptor"
    );
    let replacement = find_dir_by_mode(fixture.path(), 0o500).expect("the replacement");
    assert_eq!(
        replacement.file_name().and_then(|name| name.to_str()),
        Some("par"),
        "the replacement is never chmodded and never renamed"
    );
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).expect("cleanup");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).expect("cleanup");
    if let Some(root) = find_private_dir(fixture.path()) {
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).expect("cleanup");
    }
    let _ = &slot_path;
}

/// An injected failure of the post-widening reopen restores the **faulted**
/// directory's recorded 0100 mode through the descriptor that was widened, never
/// mutates a replacement created in that window, and keeps the production
/// Evicting record pending. The faulted object is the first replay step (the
/// captured whole slot, itself search-only), so the injected failure fires after
/// that exact object's mode was widened.
#[test]
fn a_post_chmod_step_failure_restores_the_original_mode_and_stays_pending() {
    let fixture = Fixture::new("step-fault");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "step-fault");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    // The owner left the captured slot search-only, and the record says so.
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o100))
        .expect("search-only captured slot");
    write_journal(
        &pool,
        slot_id,
        crate::owned_delete::RootCapturePlan {
            mode: u32::from(plan.mode & u32::from(libc::S_IFMT)) | 0o100,
            ..plan.clone()
        },
        true,
        Vec::new(),
        Vec::new(),
    );

    // In the window after that object's widening, a racer claims the recorded
    // name and the reopen fails.
    let replacement = slot_path.clone();
    let _fault = crate::test_fs::after_step_chmod(move || {
        fs::create_dir(&replacement).expect("replacement at the recorded name");
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o500))
            .expect("replacement mode");
        crate::test_fs::arm_step_open_failure();
    });
    let after = status_injected(&pool).expect("status");
    drop(_fault);

    assert_eq!(
        after.slots.len(),
        1,
        "the failed step keeps Evicting pending"
    );
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert_eq!(
        mode_of(&captured),
        0o100,
        "the faulted directory's recorded mode was restored through the descriptor that was widened"
    );
    assert_eq!(
        mode_of(&slot_path),
        0o500,
        "the replacement created in that window was never mutated"
    );
    assert!(
        super::pool::capture_record_injected(&pool, slot_id).is_some(),
        "the production Evicting record is retained"
    );

    // Counterfactual control: a search-only directory that is widened and then
    // *not* restored reads 0700, so the 0100 assertion above fails if the
    // restoration is omitted.
    let control = fixture.dir("counterfactual");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o100)).expect("search-only control");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700))
        .expect("widened, not restored");
    assert_eq!(
        mode_of(&control),
        0o700,
        "an unrestored widening is observable, which is what the control asserts"
    );
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o700)).expect("cleanup");
}

// ---------------------------------------------------------------------------
// Whole-tree holding deletion (oxide#190)
// ---------------------------------------------------------------------------

/// The payload's private holding area.
fn holding_area(pool: &Path) -> PathBuf {
    pool.join("payload").join(".oxide-holding")
}

/// Create the private holding area the way the product first does.
fn prepare_holding_area(pool: &Path) {
    let area = holding_area(pool);
    if area.symlink_metadata().is_err() {
        fs::create_dir(&area).expect("holding area");
        fs::set_permissions(&area, fs::Permissions::from_mode(0o700)).expect("holding mode");
    }
}

/// The recorded holding rows of one pool, from its ledger view.
fn holding_rows(pool: &Path) -> Vec<super::HoldingEntry> {
    managed::ledger_view(pool).expect("ledger view").holding
}

/// One lease on a fresh project, for a rotation step or a hold count.
fn lease_once(fixture: &Fixture, pool: &Path, label: &str) -> super::LeaseGrant {
    let project = fixture.dir(&format!("{label}-project"));
    let workspace = fixture.dir(&format!("{label}-workspace"));
    lease_injected(request(pool, &project, &workspace, label)).expect("lease")
}

/// Evicting an idle slot under the pool lock is one rename and one durable
/// write: the tree leaves the slots root whole, one holding row records its
/// identity and bytes, the slot row is gone, and the charge follows the tree
/// until a deletion finishes it.
#[test]
fn an_eviction_moves_the_whole_tree_into_the_holding_area() {
    let fixture = Fixture::new("holding-evict");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "holding-evict");
    let payload = fs::read(slot_path.join("build/blob")).expect("payload file");

    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");

    let view = managed::ledger_view(&pool).expect("ledger view");
    assert!(
        view.slots
            .iter()
            .all(|slot| slot.slot_id != slot_id.to_string()),
        "the slot row survived the eviction"
    );
    assert_eq!(
        view.holding.len(),
        1,
        "one holding row: {:#?}",
        view.holding
    );
    let row = &view.holding[0];
    assert_eq!(row.present, Some(true), "the tree is in the holding area");
    assert!(row.recorded_bytes > 0, "the row is charged its bytes");
    assert!(!slot_path.exists(), "the tree left the slots root whole");
    let held = holding_area(&pool).join(&row.name);
    assert_eq!(
        fs::read(held.join("build/blob")).expect("held payload"),
        payload,
        "the held tree is the slot's own tree, not a copy"
    );
    // A measured status walks the payload: the held tree is not a slot and the
    // payload figure still counts every byte of it.
    let status = status_injected(&pool).expect("status");
    assert_eq!(status.slots.len(), 0);
    assert_eq!(status.holding.len(), 1);
    assert!(status.payload_unique_allocated_bytes.unwrap_or(0) >= payload.len() as u64);

    // The charge is the ledger total the next admission uses, and it is not a
    // slot: neither the slot table nor a project's cap counts it.
    let snapshot = managed::snapshot(&pool).expect("snapshot");
    assert_eq!(snapshot.holding.len(), 1);
    assert_eq!(snapshot.slots.len(), 0);

    // Deleting it drops the row, the bytes and the tree; the charge is gone.
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the held tree is gone: {pass:?}");
    assert!(pass.entries >= 1, "the pass counted what it removed");
    assert!(!held.exists(), "the holding entry is removed");
    assert!(
        holding_area(&pool).is_dir(),
        "the holding area itself stays for the next eviction"
    );
    assert!(holding_rows(&pool).is_empty(), "the row is dropped");
    assert_eq!(
        managed::snapshot(&pool).expect("snapshot").recorded_total,
        None
    );
}

/// The open a crash between the rename and its durable row leaves: the slot row
/// is still recorded with the identity its own intent bound, its root is gone,
/// and the tree sits under the name that generation derives. The next locked
/// open adopts it as a holding row instead of charging an absent slot or
/// deleting blindly.
#[test]
fn a_tree_renamed_before_its_row_is_adopted_by_the_next_open() {
    let fixture = Fixture::new("holding-adopt");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "holding-adopt");
    prepare_holding_area(&pool);
    let name = super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived name");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    super::pool::hold_intent_for_test(&pool, slot_id, verified.0, verified.1).expect("intent");
    fs::rename(&slot_path, holding_area(&pool).join(&name)).expect("modelled rename");
    assert!(!slot_path.exists());

    let view = managed::ledger_view(&pool).expect("the next open");
    assert!(
        view.slots.is_empty(),
        "the adopted generation is not a slot: {:#?}",
        view.slots
    );
    assert_eq!(view.holding.len(), 1, "one adopted row");
    assert_eq!(view.holding[0].name, name);
    assert_eq!(view.holding[0].slot_id, slot_id.to_string());
    assert!(view.holding[0].recorded_bytes > 0);

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "{pass:?}");
    assert!(!holding_area(&pool).join(&name).exists());
}

/// A crash mid-delete leaves a partly emptied tree and its row. The next pass
/// resumes from what is left, is idempotent about the entries already gone, and
/// finishes the same entry.
#[test]
fn a_partly_deleted_tree_is_resumed_and_finished() {
    let fixture = Fixture::new("holding-resume");
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "holding-resume");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    let row = holding_rows(&pool).remove(0);
    let held = holding_area(&pool).join(&row.name);
    fs::create_dir_all(held.join("build/deps")).expect("deeper tree");
    fs::write(held.join("build/deps/object"), b"object").expect("object");
    // A killed deleter's progress: one file is already gone.
    fs::remove_file(held.join("build/blob")).expect("removed before the crash");

    let pass = managed::drain_holding(&pool, None).expect("resume");
    assert!(pass.gone, "the resumed pass finished the entry: {pass:?}");
    assert!(pass.entries >= 2, "it removed the rest: {pass:?}");
    assert!(!held.exists());
    assert!(holding_rows(&pool).is_empty(), "the row is dropped");
}

/// Custody is a non-blocking lock on the entry's own descriptor: while another
/// process holds it, a pass claims nothing and deletes nothing. Once that
/// holder is gone, the next pass takes the entry and finishes it.
#[test]
fn a_live_deleter_keeps_custody_and_the_next_pass_takes_over() {
    let fixture = Fixture::new("holding-custody");
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "holding-custody");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    let row = holding_rows(&pool).remove(0);
    let held = holding_area(&pool).join(&row.name);

    // Stand in for a live deleter: the entry's own descriptor, locked.
    let owner = File::open(&held).expect("entry descriptor");
    assert_eq!(
        // SAFETY: `owner` is a live descriptor for the entry directory.
        unsafe {
            libc::flock(
                std::os::fd::AsRawFd::as_raw_fd(&owner),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        },
        0,
        "the stand-in deleter took custody"
    );
    let blocked = managed::drain_holding(&pool, None).expect("a pass that loses the race");
    assert_eq!(blocked.claimed, None, "another deleter's entry was claimed");
    assert!(held.exists(), "the entry under custody was deleted");
    assert_eq!(holding_rows(&pool).len(), 1, "its row stays charged");

    drop(owner);
    let pass = managed::drain_holding(&pool, None).expect("takeover");
    assert_eq!(pass.claimed.as_deref(), Some(row.name.as_str()));
    assert!(pass.gone, "the takeover finished the entry: {pass:?}");
    assert!(holding_rows(&pool).is_empty());
}

/// An entry this pass cannot delete stays recorded and charged and never keeps
/// another entry from being deleted: the rotation starts each pass at a
/// different row, using the pool's own monotonic rank sequence.
#[test]
fn a_refused_entry_is_kept_and_does_not_block_another_entry() {
    let fixture = Fixture::new("holding-refused");
    let pool = fixture.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    // Two idle slots, the first of which holds an entry the traversal refuses.
    let first = lease_once(&fixture, &pool, "first");
    let first_slot = first.slot_id.parse::<u64>().expect("slot id");
    let fifo = first.env.cargo_build_build_dir.join("pipe");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).expect("fifo name");
    // SAFETY: a fresh path inside the fixture, mode 0600.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    release_injected(&pool, &first.lease_id).expect("release first");
    let second = lease_once(&fixture, &pool, "second");
    release_injected(&pool, &second.lease_id).expect("release second");
    evict_idle_slot_injected(&pool, &first_slot.to_string()).expect("evict the fifo slot");
    let fifo_row = holding_rows(&pool)
        .into_iter()
        .find(|row| row.slot_id == first_slot.to_string())
        .expect("the fifo slot's row");
    let fifo_held = holding_area(&pool).join(&fifo_row.name);

    // The recorded entry's own pass refuses the unknown entry type and keeps
    // everything: nothing is silently dropped and the row stays charged.
    let refused = managed::drain_holding(&pool, Some(&fifo_row.name)).expect("refused pass");
    assert_eq!(refused.claimed.as_deref(), Some(fifo_row.name.as_str()));
    assert!(
        !refused.gone,
        "the entry with a fifo completed: {refused:?}"
    );
    assert!(
        refused
            .refused
            .iter()
            .any(|item| item.contains("unknown entry type")),
        "the refusal names the entry type: {refused:?}"
    );
    assert!(fifo_held.exists(), "the refused tree is kept");
    assert_eq!(holding_rows(&pool).len(), 1, "the row stays charged");

    // A later command's own pass (the rank sequence advanced) takes the other
    // entry and deletes it; the refused one is untouched.
    let other_slot = lease_once(&fixture, &pool, "third");
    let other_slot_id = other_slot.slot_id.parse::<u64>().expect("slot id");
    release_injected(&pool, &other_slot.lease_id).expect("release third");
    evict_idle_slot_injected(&pool, &other_slot_id.to_string()).expect("evict the healthy slot");
    let mut removed = false;
    for _ in 0..4 {
        let pass = managed::drain_holding(&pool, None).expect("pass");
        if pass.claimed.as_deref() != Some(fifo_row.name.as_str()) && pass.gone {
            removed = true;
            break;
        }
        // Another command: its own grant and release advance the rotation.
        let step = lease_once(&fixture, &pool, "rotation");
        release_injected(&pool, &step.lease_id).expect("release rotation");
    }
    assert!(removed, "the healthy entry was never deleted");
    let rows = holding_rows(&pool);
    assert!(
        rows.iter().any(|row| row.name == fifo_row.name),
        "the refused entry is still recorded: {rows:#?}"
    );
    assert!(fifo_held.exists(), "the refused tree is still there");
}

/// The traversal never follows a link: a symlink inside a held tree is unlinked
/// as a leaf, and its target outside the pool survives.
#[test]
fn a_symlink_in_a_held_tree_is_unlinked_never_followed() {
    let fixture = Fixture::new("holding-symlink");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "holding-symlink");
    let outside = fixture.dir("outside");
    fs::write(outside.join("keep"), b"outside").expect("outside file");
    symlink(&outside, slot_path.join("build/link")).expect("symlink");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "{pass:?}");
    assert!(pass.refused.is_empty(), "a symlink is deleted: {pass:?}");
    assert_eq!(
        fs::read(outside.join("keep")).expect("the target survives"),
        b"outside"
    );
    assert!(holding_rows(&pool).is_empty());
}

/// An owned directory the owner left read-only (a test or build scratch tree)
/// is still deleted whole through its own descriptor.
#[test]
fn an_owned_read_only_tree_is_deleted_whole() {
    let fixture = Fixture::new("holding-readonly");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "holding-readonly");
    let objects = slot_path.join("build/scratch/.effigy/state/objects/4bcd");
    fs::create_dir_all(&objects).expect("object dir");
    fs::write(objects.join("data"), b"x").expect("object file");
    fs::set_permissions(objects.join("data"), fs::Permissions::from_mode(0o444))
        .expect("read-only file");
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o500)).expect("read-only dir");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the read-only tree is deleted: {pass:?}");
    assert!(holding_rows(&pool).is_empty());
}

/// The claimed tree's **own root** may be owner-no-write: the drain restores
/// the owner's access through the descriptor it already holds, so a slot tree
/// the owner left read-only is still deleted whole instead of staying charged
/// forever.
#[test]
fn a_claimed_root_the_owner_left_read_only_is_still_drained() {
    let fixture = Fixture::new("holding-readonly-root");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "holding-readonly-root");
    let nested = slot_path.join("build/out");
    fs::create_dir_all(&nested).expect("nested dir");
    fs::write(nested.join("object"), b"x").expect("nested object");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    let row = holding_rows(&pool).pop().expect("a holding row");
    let held = holding_area(&pool).join(&row.name);
    // The tree the claim will open: its root loses the owner's write bit, and so
    // does one nested directory, so both the root's children and the nested
    // level need the owner's access restored.
    fs::set_permissions(held.join("build/out"), fs::Permissions::from_mode(0o500))
        .expect("nested read-only");
    fs::set_permissions(&held, fs::Permissions::from_mode(0o500)).expect("root read-only");

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the read-only root is deleted whole: {pass:?}");
    assert!(pass.refused.is_empty(), "nothing is refused: {pass:?}");
    assert!(!held.exists(), "the held tree is gone");
    assert!(holding_rows(&pool).is_empty(), "the row is finished");
}

/// The traversal never enters a directory on another device: the same tree is
/// proved both ways, so only the injected device differs. A regular file is
/// still removed (no device rule applies to it), the child directory is refused
/// by name and kept, and the real device then empties the tree — the branch is
/// reachable and falsifiable, not merely unreachable code.
#[test]
fn a_child_directory_on_another_device_is_refused_and_kept() {
    let fixture = Fixture::new("holding-device");
    let root = fixture.dir("tree");
    let child = root.join("child");
    fs::create_dir(&child).expect("child dir");
    fs::write(child.join("kept"), b"kept").expect("kept file");
    fs::write(root.join("leaf"), b"leaf").expect("leaf file");
    let held = File::open(&root).expect("open the tree");
    let device = fs::symlink_metadata(&root).expect("root metadata").dev();

    let (emptied, stats) =
        super::pool::delete_holding_children_for_test(held.as_fd(), device.wrapping_add(1))
            .expect("traversal");
    assert!(
        !emptied,
        "a tree with a foreign-device child completed: {stats:?}"
    );
    assert!(
        stats
            .refused
            .iter()
            .any(|item| item.contains("another device")),
        "the refusal names the device boundary: {stats:?}"
    );
    assert_eq!(
        fs::read(child.join("kept")).expect("the foreign subtree survives"),
        b"kept"
    );
    assert!(
        !root.join("leaf").exists(),
        "an ordinary file is still removed: {stats:?}"
    );

    let (emptied, stats) =
        super::pool::delete_holding_children_for_test(held.as_fd(), device).expect("traversal");
    assert!(
        emptied,
        "the same tree with its real device is empty: {stats:?}"
    );
    assert!(stats.refused.is_empty(), "nothing is refused: {stats:?}");
}

/// A recorded row whose holding area is positively absent is finished, not
/// charged forever: the open that reads the payload sees no area, and no tree
/// can live in an area that does not exist. Nothing is deleted here — the rows
/// simply stop being charged.
#[test]
fn a_recorded_row_is_finished_when_the_holding_area_is_gone() {
    let fixture = Fixture::new("holding-area-gone");
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "holding-area-gone");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    assert_eq!(holding_rows(&pool).len(), 1, "the eviction recorded a row");

    // The area and the tree vanish outside Oxide's control (a manual cleanup, an
    // external tool, a lost directory): the row must not stay charged forever.
    fs::remove_dir_all(holding_area(&pool)).expect("remove the holding area");

    let pass = managed::drain_holding(&pool, None).expect("a locked open");
    assert!(!pass.gone, "no entry was deleted: {pass:?}");
    assert!(
        holding_rows(&pool).is_empty(),
        "the row is finished once its area is positively absent"
    );
}

/// An unrecorded entry in the holding area is never adopted and never deleted:
/// it stays charged by the payload walk, while the recorded row beside it is
/// deleted normally.
#[test]
fn an_unrecorded_holding_entry_is_kept_and_charged_never_deleted() {
    let fixture = Fixture::new("holding-unrecorded");
    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "holding-unrecorded");
    prepare_holding_area(&pool);
    let stranger = holding_area(&pool).join("0000-not-a-recorded-generation");
    fs::create_dir(&stranger).expect("stranger");
    fs::write(stranger.join("bytes"), vec![9_u8; 64 * 1024]).expect("stranger bytes");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the recorded entry is deleted: {pass:?}");
    assert!(stranger.is_dir(), "an unrecorded entry is never deleted");
    assert!(stranger.join("bytes").exists());
    assert!(holding_rows(&pool).is_empty());
    // Its bytes are still counted by the pool's own payload walk.
    let status = status_injected(&pool).expect("status");
    assert!(
        status.payload_unique_allocated_bytes.unwrap_or(0) >= 64 * 1024,
        "the unrecorded entry was counted as free: {status:?}"
    );
}

/// A legacy `Evicting` record is migrated on the first locked open: its tree is
/// moved whole into the holding area and deleted outside the lock, with no
/// per-entry journal started. Both legacy shapes migrate — the root still at its
/// recorded name and the root under its captured private name.
#[test]
fn a_legacy_pending_deletion_migrates_into_the_holding_area() {
    for captured in [false, true] {
        let fixture = Fixture::new(if captured {
            "holding-legacy-captured"
        } else {
            "holding-legacy-planned"
        });
        let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "legacy");
        let plan = capture_root_state(&slot_path, slot_id, &pool, captured);
        assert_eq!(
            super::pool::capture_record_injected(&pool, slot_id).is_some(),
            true,
            "the modelled legacy journal is durable"
        );

        let view = managed::ledger_view(&pool).expect("the next open migrates it");
        assert!(
            view.slots.is_empty(),
            "the migrated record is not a slot (captured: {captured})"
        );
        assert_eq!(
            view.holding.len(),
            1,
            "the tree is held (captured: {captured})"
        );
        assert!(
            super::pool::capture_record_injected(&pool, slot_id).is_none(),
            "no per-entry journal survives the migration"
        );
        assert!(!slot_path.exists());
        assert!(
            capture_names(slot_path.parent().expect("slots root")).is_empty(),
            "the private capture name is gone with the tree"
        );
        let pass = managed::drain_holding(&pool, None).expect("drain");
        assert!(pass.gone, "the migrated tree is deleted: {pass:?}");
        let _ = plan;
    }
}

/// A legacy journal that cannot be classified keeps its record and blocks no
/// new holding row: the migration never reinterprets an uncertain record.
#[test]
fn an_unclassifiable_legacy_journal_stays_pending_and_starts_no_holding_row() {
    let fixture = Fixture::new("holding-legacy-blocked");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "legacy-blocked");
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    // A recorded mismatch is exactly what replay must never reinterpret.
    write_journal(
        &pool,
        slot_id,
        plan.clone(),
        true,
        Vec::new(),
        vec![crate::owned_delete::ForeignCapture {
            path: Vec::new(),
            private: ".oxide-capture-fedcba9876543210".to_owned(),
            original: "blob".to_owned(),
            expected_device: plan.device,
            expected_inode: plan.inode,
            expected_mode: u32::from(libc::S_IFREG) | 0o600,
            found: "directory device 1 inode 2 mode 40700 owner 501".to_owned(),
            restored: false,
        }],
    );

    let view = managed::ledger_view(&pool).expect("open");
    assert_eq!(view.slots.len(), 1, "the uncertain record is kept");
    assert!(view.holding.is_empty(), "no holding row was invented");
    assert!(view.reconciliation_blocked);
}

/// The legacy-migration crash window: the resolved root was renamed into the
/// holding area but the durable write that records the row never happened, so
/// the ledger still carries an `Evicting` row with no journal and no root. The
/// next locked open adopts that generation's own entry — the record leaves the
/// slot table, the charge follows it, and the deletion finishes as a whole tree.
#[test]
fn an_interrupted_legacy_migration_is_adopted_by_the_next_open() {
    let fixture = Fixture::new("migration-interrupted");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "migration-interrupted");
    mark_evicting_for_test(&pool, slot_id).expect("a legacy pending deletion");
    prepare_holding_area(&pool);
    let name = super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived name");
    // The migration binds the verified identity durably before its rename, so
    // the interrupted state carries that intent as well.
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    super::pool::write_hold_intent_injected(&pool, slot_id, verified.0, verified.1)
        .expect("durable hold intent");
    fs::rename(&slot_path, holding_area(&pool).join(&name)).expect("modelled migration rename");
    assert!(!slot_path.exists(), "the root left its recorded name");

    let view = managed::ledger_view(&pool).expect("the next open adopts it");
    assert!(
        view.slots.is_empty(),
        "the interrupted legacy row is not adopted: {:#?}",
        view.slots
    );
    assert!(
        !view.reconciliation_blocked,
        "adoption left the pool blocked"
    );
    assert_eq!(view.holding.len(), 1, "one adopted holding row");
    assert_eq!(view.holding[0].name, name);
    assert_eq!(view.holding[0].slot_id, slot_id.to_string());
    assert!(
        view.holding[0].recorded_bytes > 0,
        "the row carries a charge"
    );

    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the adopted tree is deleted: {pass:?}");
    assert!(holding_rows(&pool).is_empty());
}

/// The rename boundary is not satisfied by a name alone: the identity the
/// caller verified is compared with the held descriptor *before* the rename, so
/// a root replaced in that window refuses and nothing is moved at all.
#[test]
fn a_replaced_root_refuses_before_the_rename() {
    let fixture = Fixture::new("replaced-root");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "replaced-root");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    // Replace the verified root with a different owned directory of the same
    // name, which is exactly what a racer between the preflight and the rename
    // produces.
    let aside = slot_path.with_extension("race-aside");
    fs::rename(&slot_path, &aside).expect("move the verified root aside");
    fs::create_dir(&slot_path).expect("replacement root");
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("replacement mode");
    fs::write(slot_path.join("replacement"), b"not the slot").expect("replacement payload");

    let refused = super::pool::hold_expecting_injected(&pool, slot_id, Some(verified));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "a replaced root was moved: {refused:?}"
    );
    assert!(slot_path.is_dir(), "the replacement is still where it was");
    assert_eq!(
        fs::read(slot_path.join("replacement")).expect("replacement payload"),
        b"not the slot"
    );
    assert!(
        holding_rows(&pool).is_empty(),
        "the replacement was recorded as a held tree"
    );
    match holding_area(&pool).read_dir() {
        // The refusal happens before the holding area is created, so an absent
        // area is itself proof that nothing was moved.
        Err(error) => assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "the holding area is unreadable: {error}"
        ),
        Ok(entries) => assert!(
            entries.flatten().next().is_none(),
            "the replacement was moved into the holding area"
        ),
    }
    assert!(aside.is_dir(), "the verified root is untouched");
}

/// The half a name cannot catch: the source name is swapped after the descriptor
/// identity was verified and exactly before the rename, so the rename moves an
/// object that is not the verified one. The eviction has already bound that
/// identity durably, so no recovery pass may adopt the entry; the foreign tree
/// is additionally contained under a name no generation can derive. Nothing
/// foreign is ever deleted and the generation's row keeps its charge.
#[test]
fn a_swap_at_the_rename_is_contained_and_never_adopted() {
    let fixture = Fixture::new("rename-swap");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "rename-swap");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    let aside = slot_path.with_extension("race-aside");
    let slot_for_race = slot_path.clone();
    let aside_for_race = aside.clone();
    // Swap the source name after the identity check and before the rename.
    let guard = crate::test_fs::before_holding_rename(move || {
        fs::rename(&slot_for_race, &aside_for_race).expect("move the verified root aside");
        fs::create_dir(&slot_for_race).expect("replacement root");
        fs::set_permissions(&slot_for_race, fs::Permissions::from_mode(0o700))
            .expect("replacement mode");
        fs::write(slot_for_race.join("replacement"), b"not the slot").expect("replacement payload");
    });
    let refused = evict_idle_slot_injected(&pool, &slot_id.to_string());
    drop(guard);
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "a swapped rename was recorded: {refused:?}"
    );

    // Containment: the foreign tree is not at the adoptable derived name.
    let area = holding_area(&pool);
    let derived = super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived");
    assert!(
        !area.join(&derived).exists(),
        "the swapped tree stayed at the adoptable derived name"
    );
    let contained: Vec<PathBuf> = fs::read_dir(&area)
        .expect("holding area")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".unverified-"))
        })
        .collect();
    assert_eq!(
        contained.len(),
        1,
        "the swapped tree is contained: {contained:?}"
    );
    assert_eq!(
        fs::read(contained[0].join("replacement")).expect("contained payload"),
        b"not the slot"
    );
    // A later open neither adopts nor deletes it, and the generation's row is
    // still recorded, so its slot is never silently freed.
    let view = managed::ledger_view(&pool).expect("the next open");
    assert!(
        view.holding.is_empty(),
        "a contained tree was adopted: {:#?}",
        view.holding
    );
    assert!(
        view.slots
            .iter()
            .any(|slot| slot.slot_id == slot_id.to_string()),
        "the generation's row survived the refused rename"
    );
    assert!(
        contained[0].is_dir(),
        "the contained tree was deleted: {contained:?}"
    );
    assert!(aside.is_dir(), "the verified root is untouched");
    // The containment is charged, not free: the payload walk still counts it.
    let status = status_injected(&pool).expect("status");
    assert!(
        status.payload_unique_allocated_bytes.unwrap_or(0) > 0,
        "the contained bytes were counted as free: {status:?}"
    );
    let _ = verified;
}

/// The crash window the ordinary pressure eviction really has: the identity
/// intent is durable, the tree has been renamed whole and verified, and the row
/// write never happens. The ledger is the one the production path wrote — no
/// synthetic intent — and the next locked open resumes the generation through
/// `reconcile_holding` instead of stranding its tree.
#[test]
fn an_ordinary_eviction_binds_its_intent_before_the_rename() {
    let fixture = Fixture::new("pressure-intent");
    let pool = fixture.dir("pool");
    create_injected(&pool, 4 * 1024 * 1024).expect("init");
    let workspace = fixture.dir("workspace");
    let first = lease_injected(request(&pool, &fixture.dir("project-a"), &workspace, "a"))
        .expect("first slot");
    fs::write(
        first.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 5 * 1024 * 1024],
    )
    .expect("payload past the cap");
    release_injected(&pool, &first.lease_id).expect("release");
    let slot_id = first.slot_id.parse::<u64>().expect("slot id");
    let slot_path = pool.join("payload/slots").join(&first.slot_id);
    let derived =
        super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived name");

    // The interruption fires after the rename and its verification, before the
    // row write: a panic is this test's model of the process dying there.
    let armed = crate::test_fs::after_holding_rename(|| panic!("modelled crash after the rename"));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lease_injected(request(&pool, &fixture.dir("project-b"), &workspace, "b"))
    }));
    drop(armed);
    assert!(outcome.is_err(), "the armed interruption did not fire");

    // What the production path left: the generation's row, its durable intent,
    // the tree under the derived name and no slot root left behind.
    let bytes = fs::read_to_string(pool.join("ledger.json")).expect("ledger bytes");
    assert!(
        bytes.contains("hold_intent"),
        "the ordinary eviction wrote no intent before its rename: {bytes}"
    );
    assert!(
        holding_area(&pool).join(&derived).is_dir(),
        "the tree was not renamed into the holding area"
    );
    assert!(
        !slot_path.exists(),
        "the slot root was left in the slots root"
    );

    // The next locked open resumes it, and the deletion finishes.
    let view = managed::ledger_view(&pool).expect("the next open adopts it");
    assert_eq!(
        view.holding.len(),
        1,
        "the interrupted generation was stranded: {:#?}",
        view.slots
    );
    assert!(view.slots.is_empty(), "the slot row survived the adoption");
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "the adopted tree is deleted: {pass:?}");
}

/// An idle rename interrupted after the durable intent and after the rename (the
/// crash window the brief accepts) is adopted by the next open **because** the
/// intent names the very object now in the holding area — never merely because
/// the entry carries the derived name. A stranger at that name with the real
/// root elsewhere is kept and charged instead.
#[test]
fn an_idle_rename_interruption_is_adopted_only_by_its_intent() {
    let fixture = Fixture::new("idle-interrupt");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "idle-interrupt");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    prepare_holding_area(&pool);
    let name = super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived name");
    // The durable intent an interrupted eviction leaves behind.
    super::pool::hold_intent_for_test(&pool, slot_id, verified.0, verified.1)
        .expect("durable intent");
    // A stranger appears at the derived name and the verified root is where a
    // failed containment would have left it (out of its recorded path).
    let aside = slot_path.with_extension("race-aside");
    fs::rename(&slot_path, &aside).expect("the verified root left its path");
    let stranger = holding_area(&pool).join(&name);
    fs::create_dir(&stranger).expect("stranger");
    fs::write(stranger.join("replacement"), b"not the slot").expect("stranger payload");
    let view = managed::ledger_view(&pool).expect("open");
    assert!(
        view.holding.is_empty(),
        "a stranger at the derived name was adopted: {:#?}",
        view.holding
    );
    assert!(view
        .slots
        .iter()
        .any(|slot| slot.slot_id == slot_id.to_string()));
    assert!(
        stranger.join("replacement").exists(),
        "the stranger was touched"
    );

    // Now the verified root itself is the entry under that name: the intent
    // binds it and the adoption completes.
    fs::remove_dir_all(&stranger).expect("clear the stranger");
    fs::rename(&aside, holding_area(&pool).join(&name)).expect("modelled rename");
    let view = managed::ledger_view(&pool).expect("open adopts it");
    assert_eq!(
        view.holding.len(),
        1,
        "the bound idle interruption is adopted"
    );
    assert!(view.slots.is_empty());
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "{pass:?}");
}

/// An intent whose root is still at its recorded path names a rename that never
/// happened (the command refused or crashed before it). The next open clears it
/// and treats the generation as an ordinary idle slot again: nothing is moved,
/// adopted or deleted.
#[test]
fn a_stale_intent_is_cleared_and_its_slot_stays_an_ordinary_idle_slot() {
    let fixture = Fixture::new("stale-intent");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "stale-intent");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    super::pool::hold_intent_for_test(&pool, slot_id, verified.0, verified.1).expect("intent");
    let view = managed::ledger_view(&pool).expect("open");
    assert!(view.holding.is_empty(), "a stale intent adopted something");
    assert!(slot_path.is_dir(), "the root was moved");
    let ledger = fs::read(pool.join("ledger.json")).expect("ledger bytes");
    let text = String::from_utf8_lossy(&ledger);
    assert!(
        !text.contains("hold_intent"),
        "the stale intent survived the open: {text}"
    );
    // The slot is a normal idle slot again: a fresh project cannot reuse it, but
    // an explicit eviction still retires it whole.
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("the slot is still evictable");
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "{pass:?}");
}

/// A legacy pending deletion is adopted only against the identity its own
/// durable intent recorded before the rename. Without an intent, or with one
/// that names a different object, the derived-name directory is never adopted:
/// the record and its charge stay, the pool stays blocked, and the tree is left
/// alone for a replay to decide.
#[test]
fn a_legacy_entry_is_adopted_only_against_its_recorded_intent() {
    let fixture = Fixture::new("legacy-intent");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "legacy-intent");
    mark_evicting_for_test(&pool, slot_id).expect("a legacy pending deletion");
    prepare_holding_area(&pool);
    let name = super::pool::holding_name_for_slot_injected(&pool, slot_id).expect("derived name");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    fs::rename(&slot_path, holding_area(&pool).join(&name)).expect("modelled rename");

    // No intent: nothing may be adopted on a name alone.
    let view = managed::ledger_view(&pool).expect("open");
    assert!(
        view.holding.is_empty(),
        "a derived name without an intent was adopted: {:#?}",
        view.holding
    );
    assert!(view
        .slots
        .iter()
        .any(|slot| slot.slot_id == slot_id.to_string()));
    assert!(
        holding_area(&pool).join(&name).is_dir(),
        "the tree was touched"
    );

    // An intent for a different object: still nothing is adopted.
    super::pool::write_hold_intent_injected(&pool, slot_id, verified.0, verified.1 + 1)
        .expect("a mismatched intent");
    let view = managed::ledger_view(&pool).expect("open");
    assert!(
        view.holding.is_empty(),
        "a mismatched intent was adopted: {:#?}",
        view.holding
    );
    assert!(holding_area(&pool).join(&name).is_dir());

    // The intent the preflight really verified: the migration is recoverable.
    super::pool::write_hold_intent_injected(&pool, slot_id, verified.0, verified.1)
        .expect("the verified intent");
    let view = managed::ledger_view(&pool).expect("open adopts it");
    assert_eq!(view.holding.len(), 1, "the bound migration is adopted");
    assert!(view.slots.is_empty());
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(pass.gone, "{pass:?}");
}

/// Only a positively classified miss concludes that a held tree is gone. An
/// `lstat` failure that says nothing about absence (here an over-bound entry
/// name) classifies as unknown, and an unreadable holding area refuses the open
/// instead of dropping a row: the row and its charge survive in the ledger.
#[test]
fn an_unclassified_holding_entry_error_never_drops_the_row_or_charge() {
    let fixture = Fixture::new("holding-unknown");
    let area = fixture.dir("area");
    assert_eq!(
        super::pool::holding_presence(&area),
        Some(true),
        "a present entry must classify as present"
    );
    assert_eq!(
        super::pool::holding_presence(&area.join("absent")),
        Some(false),
        "a missing name must classify as a positive miss"
    );
    let over_bound = area.join("x".repeat(300));
    assert_eq!(
        super::pool::holding_presence(&over_bound),
        None,
        "an over-bound name must classify as unknown, never as absence"
    );

    let (pool, slot_id, _slot_path) = idle_slot_for_eviction(&fixture, "holding-unknown");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    let row = holding_rows(&pool).remove(0);
    assert!(row.recorded_bytes > 0, "a recorded charge");
    // The area becomes unusable without saying the tree is gone.
    fs::set_permissions(holding_area(&pool), fs::Permissions::from_mode(0o000))
        .expect("unsearchable area");
    let refused = managed::ledger_view(&pool);
    assert!(
        refused.is_err(),
        "an unreadable holding area must refuse, not conclude"
    );
    let snapshot = managed::snapshot(&pool).expect("a passive snapshot reads the ledger");
    assert_eq!(snapshot.holding.len(), 1, "the row survived: {snapshot:?}");
    assert_eq!(
        snapshot.holding[0].recorded_bytes, row.recorded_bytes,
        "the charge survived"
    );
    fs::set_permissions(holding_area(&pool), fs::Permissions::from_mode(0o700))
        .expect("searchable area again");
    let view = managed::ledger_view(&pool).expect("the pool opens again");
    assert_eq!(view.holding.len(), 1, "the tree is still held and charged");
    assert_eq!(view.holding[0].present, Some(true));
    let pass = managed::drain_holding(&pool, None).expect("drain");
    assert!(
        pass.gone,
        "the tree is deleted once it is reachable again: {pass:?}"
    );
    assert!(holding_rows(&pool).is_empty());
}

/// Every held tree stays charged in the admission's capacity decision until it
/// is positively removed — including one whose deletion refuses. A pool whose
/// held row alone exhausts its budget refuses a new slot instead of deciding as
/// if those bytes were already free, and the refusal does not depend on the
/// tree being deletable.
#[test]
fn a_refused_tree_stays_charged_until_it_is_positively_removed() {
    let fixture = Fixture::new("held-charge");
    let pool = fixture.dir("pool");
    create_injected(&pool, 4 * 1024 * 1024).expect("init");
    let workspace = fixture.dir("workspace");
    let first = lease_injected(request(&pool, &fixture.dir("project-a"), &workspace, "a"))
        .expect("first slot");
    fs::write(
        first.env.cargo_build_build_dir.join("blob"),
        vec![b'a'; 5 * 1024 * 1024],
    )
    .expect("payload past the cap");
    release_injected(&pool, &first.lease_id).expect("release");
    let slot_id = first.slot_id.parse::<u64>().expect("slot id");
    let created = evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict by id");
    assert_eq!(created.len(), 1, "one entry is created: {created:?}");
    let held = holding_area(&pool).join(&created[0]);
    assert!(held.is_dir(), "the tree is in the holding area");

    // The held bytes are charged: a second project's admission cannot decide as
    // if the eviction had already freed them.
    assert!(
        matches!(
            lease_injected(request(&pool, &fixture.dir("project-b"), &workspace, "b")),
            Err(LeaseError::Full) | Err(LeaseError::CapacityCharged { .. })
        ),
        "a held tree's bytes were treated as free"
    );

    // An undeletable item keeps the tree, its row and its charge.
    let fifo = held.join("pipe");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).expect("fifo name");
    // SAFETY: a fresh path inside the fixture, mode 0600.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    let pass = managed::drain_holding(&pool, Some(&created[0])).expect("a refused pass");
    assert!(!pass.gone, "the undeletable tree completed: {pass:?}");
    assert!(
        !pass.refused.is_empty(),
        "the refusal is reported: {pass:?}"
    );
    assert_eq!(holding_rows(&pool).len(), 1, "the row survived the refusal");
    assert!(
        matches!(
            lease_injected(request(&pool, &fixture.dir("project-c"), &workspace, "c")),
            Err(LeaseError::Full) | Err(LeaseError::CapacityCharged { .. })
        ),
        "an undeletable held tree stopped being charged"
    );

    // Only positive removal frees the bytes.
    fs::remove_file(&fifo).expect("the refused item is removed");
    let pass = managed::drain_holding(&pool, Some(&created[0])).expect("the resumed pass");
    assert!(pass.gone, "the tree is gone once it is deletable: {pass:?}");
    assert!(
        holding_rows(&pool).is_empty(),
        "the row is dropped with the tree"
    );
    lease_injected(request(&pool, &fixture.dir("project-d"), &workspace, "d"))
        .expect("the pool admits once the held bytes are gone");
}

/// An admission reports exactly the holding entries it created, and a pass that
/// names one deletes that entry and no other: several trees can be pending at
/// once and the publishing command identifies its own.
#[test]
fn an_admission_reports_each_holding_entry_it_created() {
    let fixture = Fixture::new("reported-holding");
    let pool = fixture.dir("pool");
    create_injected(&pool, 1024 * 1024 * 1024).expect("init");
    // Fill the raised ceiling with held slots and free one, so the next
    // admission must replace it (a count decision, never a byte one).
    let (id, lease) = super::pool::fill_held_slot_table_for_test(&pool).expect("a full table");
    release_injected(&pool, &lease).expect("release the only idle slot");
    let (admitted, created) = super::pool::lease_holding_injected(request(
        &pool,
        &fixture.dir("project-a"),
        &fixture.dir("ws"),
        "a",
    ))
    .expect("the ceiling replacement admits");
    assert_eq!(
        created.len(),
        1,
        "the admission reported no created entry: {created:?}"
    );
    let first = created[0].clone();
    assert!(
        holding_rows(&pool).iter().any(|row| row.name == first),
        "the reported entry is not the recorded one"
    );
    let rows = holding_rows(&pool);
    assert_eq!(rows.len(), 1, "one tree is held: {rows:?}");
    assert_eq!(
        rows[0].slot_id,
        id.to_string(),
        "the reported entry is not the evicted idle slot"
    );
    assert_eq!(rows[0].name, first);

    // A second, independent pending entry: the pass that names the first
    // deletes it and leaves the other.
    release_injected(&pool, &admitted.lease_id).expect("release the admitted slot");
    let other_slot = admitted.slot_id.clone();
    let second = evict_idle_slot_injected(&pool, &other_slot).expect("evict it by id");
    assert_eq!(second.len(), 1);
    assert_eq!(holding_rows(&pool).len(), 2, "two trees are pending");
    let pass = managed::drain_holding(&pool, Some(&first)).expect("the named pass");
    assert_eq!(pass.claimed.as_deref(), Some(first.as_str()));
    assert!(pass.gone, "{pass:?}");
    let left = holding_rows(&pool);
    assert_eq!(left.len(), 1, "the unnamed entry was deleted: {left:?}");
    assert_eq!(
        left[0].name, second[0],
        "the named entry was not the one deleted"
    );
}

/// An undeletable row never starves a later deletable one, and the takeover
/// rotation needs no admission to advance: with every admission capacity-blocked
/// (no grant, no release, so the rank-derived rotation stands still) the pass
/// that refuses the first row marks it attempted, and the next pass takes the
/// other row.
#[test]
fn an_undeletable_row_cannot_starve_a_later_deletable_one() {
    let fixture = Fixture::new("holding-starvation");
    let pool = fixture.dir("pool");
    create_injected(&pool, 64 * 1024 * 1024).expect("init");
    // Two idle slots, each evicted explicitly, so both rows exist at once.
    let first = lease_once(&fixture, &pool, "starve-a");
    let first_slot = first.slot_id.parse::<u64>().expect("slot id");
    release_injected(&pool, &first.lease_id).expect("release");
    let second = lease_once(&fixture, &pool, "starve-b");
    let second_slot = second.slot_id.parse::<u64>().expect("slot id");
    release_injected(&pool, &second.lease_id).expect("release");
    let a = evict_idle_slot_injected(&pool, &first_slot.to_string()).expect("evict a");
    let b = evict_idle_slot_injected(&pool, &second_slot.to_string()).expect("evict b");
    assert_eq!(holding_rows(&pool).len(), 2, "two trees are pending");
    // Make the first row undeletable: it is the oldest, so a rotation that
    // depended on admission would keep reaching it.
    let a_held = holding_area(&pool).join(&a[0]);
    let fifo = a_held.join("pipe");
    let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).expect("fifo");
    // SAFETY: a fresh path inside the fixture, mode 0600.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    // No admission happens between these passes: an unchanged grant/release
    // sequence must not stop the second row from being reached.
    let first_pass = managed::drain_holding(&pool, None).expect("the first pass");
    assert_eq!(first_pass.claimed.as_deref(), Some(a[0].as_str()));
    assert!(!first_pass.gone, "the undeletable row completed");
    let second_pass = managed::drain_holding(&pool, None).expect("the next pass");
    assert_eq!(
        second_pass.claimed.as_deref(),
        Some(b[0].as_str()),
        "the undeletable row starved the deletable one: {second_pass:?}"
    );
    assert!(
        second_pass.gone,
        "the deletable row completed: {second_pass:?}"
    );
    let rows = holding_rows(&pool);
    assert_eq!(rows.len(), 1, "only the undeletable row is left: {rows:?}");
    assert_eq!(rows[0].name, a[0]);
}

/// A private capture name inside a held tree is #155 journal state, and with the
/// journal retired no record names it: the outside-lock deletion keeps it as
/// uncertain instead of unlinking it as ordinary content, and the tree stays
/// held, charged and reported.
#[test]
fn an_unrecorded_private_capture_in_a_held_tree_is_quarantined() {
    let fixture = Fixture::new("capture-quarantine");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "capture-quarantine");
    let capture = slot_path.join("build/.oxide-capture-fedcba9876543210");
    fs::write(&capture, b"uncertain journal state").expect("a private capture name");
    evict_idle_slot_injected(&pool, &slot_id.to_string()).expect("evict");
    let row = holding_rows(&pool).remove(0);
    let held = holding_area(&pool).join(&row.name);
    let pass = managed::drain_holding(&pool, None).expect("the pass");
    assert!(!pass.gone, "a private capture name was deleted: {pass:?}");
    assert!(
        pass.refused
            .iter()
            .any(|item| item.contains("unrecorded private capture")),
        "the quarantine is not reported: {pass:?}"
    );
    assert!(
        held.join("build/.oxide-capture-fedcba9876543210").exists(),
        "the uncertain capture name was removed"
    );
    assert_eq!(
        holding_rows(&pool).len(),
        1,
        "the tree stays held and charged"
    );
}

/// A legacy pending deletion is migrated with an O(1) root verification, not a
/// recursive preflight under the pool lock: a tree whose inventory would have
/// stopped the old completeness walk no longer blocks every admission. The tree
/// is still moved whole and its per-object refusal belongs to the outside-lock
/// deletion.
#[test]
fn a_legacy_pending_tree_migrates_without_a_locked_walk() {
    let fixture = Fixture::new("legacy-no-walk");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "legacy-no-walk");
    // The old preflight listed every directory recursively, so this unreadable
    // nested directory would have blocked the whole pool.
    let locked = slot_path.join("build/locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("unreadable");
    mark_evicting_for_test(&pool, slot_id).expect("a legacy pending deletion");
    let verified = {
        let metadata = fs::symlink_metadata(&slot_path).expect("root metadata");
        (metadata.dev(), metadata.ino())
    };
    super::pool::write_hold_intent_injected(&pool, slot_id, verified.0, verified.1)
        .expect("durable intent");

    // The next locked open moves it whole and stays unblocked.
    let view = managed::ledger_view(&pool).expect("the open does not refuse the tree");
    assert!(
        !view.reconciliation_blocked,
        "the migration left the pool blocked"
    );
    assert!(view.slots.is_empty(), "the migrated record is not a slot");
    assert_eq!(view.holding.len(), 1, "the tree is held whole");
    assert!(!slot_path.exists(), "the root left the slots root");

    // The outside-lock deletion owns the per-object outcome: an item it cannot
    // enter is kept, charged and reported — it never blocks the pool the way the
    // old locked preflight did.
    let held = holding_area(&pool).join(&view.holding[0].name);
    let pass = managed::drain_holding(&pool, None).expect("the pass");
    assert!(!pass.gone, "an unenterable item completed: {pass:?}");
    assert!(
        pass.refused.iter().any(|item| item.contains("locked")),
        "the refusal names the item: {pass:?}"
    );
    assert_eq!(holding_rows(&pool).len(), 1, "the tree is still charged");
    assert!(held.is_dir(), "the held tree is kept whole");

    // Once the owner restores its own access, the same tree finishes.
    fs::set_permissions(held.join("build/locked"), fs::Permissions::from_mode(0o700))
        .expect("owner access restored");
    let pass = managed::drain_holding(&pool, None).expect("the resumed pass");
    assert!(
        pass.gone,
        "the tree is deleted once it is enterable: {pass:?}"
    );
    assert!(holding_rows(&pool).is_empty());
}
