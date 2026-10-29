use super::managed;
use super::pool::{
    create_injected, evict_idle_slot_injected, lease_injected, mark_evicting_for_test,
    recover_injected, release_injected, status_injected, with_accounting_limits,
};
use super::{
    remove_tree_owned, AccountingCoverage, AccountingGapKind, AccountingScan, AdmissionClass,
    LeaseError, LeaseRequest, Profile, SlotLayout, SlotOccupancy, VictimPolicy,
    BUDGET_BACKEND_DIRECTORY_V1, DELETION_CUSTODY_CAPACITY_POLICY_VERSION, MANAGED_NAME,
    MAX_HOLDER_BYTES, MAX_LEASE_ID_BYTES, MAX_LEDGER_BYTES, RAISED_CAPACITY_POLICY_VERSION,
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

    let a = lease_injected(request(&pool, &fx.dir("project-a"), &workspace, "a")).expect("a");
    let a_marker = a.env.cargo_build_build_dir.join("blob");
    fs::write(&a_marker, vec![b'a'; 3 * 1024 * 1024]).expect("a payload");
    release_injected(&pool, &a.lease_id).expect("release a");
    let b = lease_injected(request(&pool, &fx.dir("project-b"), &workspace, "b")).expect("b");
    let b_marker = b.env.cargo_build_build_dir.join("blob");
    fs::write(&b_marker, vec![b'b'; 1024 * 1024]).expect("b payload");
    release_injected(&pool, &b.lease_id).expect("release b");
    let c = lease_injected(request(&pool, &fx.dir("project-c"), &workspace, "c")).expect("c");
    let c_marker = c.env.cargo_build_build_dir.join("blob");
    fs::write(&c_marker, vec![b'c'; 512 * 1024]).expect("c payload");
    // c stays held: a held slot is never an eviction candidate.

    let d = lease_injected(request(&pool, &fx.dir("project-d"), &workspace, "d"))
        .expect("byte pressure replaces the oldest idle slot");
    assert!(!d.reused_slot);
    assert!(!a_marker.exists(), "the rank-LRU idle slot was not evicted");
    assert_eq!(
        fs::read(&b_marker).expect("newer idle slot survives"),
        vec![b'b'; 1024 * 1024]
    );
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
    assert!(ids.contains(&b.slot_id.as_str()));
    let held = status
        .slots
        .iter()
        .find(|slot| slot.slot_id == c.slot_id)
        .expect("held slot remains a member");
    assert_eq!(held.state, SlotOccupancy::Held);
    assert!(status.slots.iter().any(|slot| slot.slot_id == d.slot_id));
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
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION),
        "a 256-slot ledger must carry the raised-ceiling fence, and this          writer's deletion-custody fence supersedes it"
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
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION),
        "the first revised write publishes the deletion-custody fence"
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
    let second = lease_injected(request(&pool, &project_b, &workspace, "holder-b")).expect("b");
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

    fs::remove_dir_all(&managed).expect("remove control");
    symlink(&project, &managed).expect("managed symlink");
    assert!(matches!(
        status_injected(&pool),
        Err(LeaseError::UnsafePath(_))
    ));
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
    let second = managed::lease(request(&pool, &project_b, &workspace, "holder-b"))
        .expect("evict then admit");
    assert_ne!(second.slot_id, first.slot_id);
    assert!(!blob.exists());
    assert!(!pool.join("payload/slots").join(&first.slot_id).exists());
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
    let third = managed::lease(smaller).expect("admitted after idle eviction");
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
    let admit = managed::lease(request(&pool, &fx.dir("other"), &workspace, "c"));
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
        || managed::lease(request(&pool, &next_project, &workspace, "next")),
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
        || managed::lease(request(&pool, &next_project, &workspace, "next")),
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

/// The maximal ledger this writer reaches on its **ordinary eviction path**
/// (oxide#177): the raised-ceiling representation at 256 slots with every field
/// at its cap, where the one slot a pool can durably leave `Evicting` carries a
/// maximal deletion journal. `retire_idle_slot` refuses a second deletion while
/// a journal is outstanding, so an admission starts one. `MAX_LEDGER_BYTES` is
/// the *worst-valid byte bound* — a larger ledger is corrupt and refused, and
/// the reader accepts any otherwise-valid ledger at or below it. Recovery can
/// leave more journals (see
/// `recovery_creates_a_journal_for_a_second_pending_deletion`), so this figure
/// is the maximal ordinary-path state, not a bound on recovery.
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

/// Recovery is a second journal path (oxide#177): `Pool::open` reconciles
/// every `Evicting` record, and `remove_owned_slot` creates and persists a fresh
/// `SlotCaptureJournal` for one that has none (or after a recoverable journal is
/// rolled back and cleared), without `retire_idle_slot`'s single-journal guard;
/// the loop continues past a blocked record and a failed deletion keeps its
/// journal. One open can therefore leave a journal per failing pending
/// deletion. The admission gate and the library's locked open make the
/// version-8 control fence durable before the open, so this recovery path is
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
    let second_path = pool.join("payload/slots").join(&second.slot_id);

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

    // Slot B: `Evicting` with no journal, and a tree the deletion cannot
    // finish (a directory with no search bit), so recovery refuses before its
    // preflight and keeps the record without starting a journal (oxide#175).
    super::pool::mark_evicting_for_test(&pool, second_id).expect("recorded deletion");
    let blocked = second_path.join("blocked");
    fs::create_dir(&blocked).expect("blocked directory");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).expect("no search bit");

    // One open reconciles them: A stays blocked with its journal, and B stays
    // pending with no journal, so recovery never starts a second one
    // (oxide#177's single-journal writer guard, kept by the fence-first run).
    let _ = status_injected(&pool);
    assert!(
        super::pool::capture_record_injected(&pool, first_id).is_some(),
        "the blocked journal is retained"
    );
    assert!(
        super::pool::capture_record_injected(&pool, second_id).is_none(),
        "recovery started a second journal"
    );
    let snapshot = super::pool::snapshot_injected(&pool).expect("snapshot");
    assert_eq!(
        snapshot.evicting_slot_ids.len(),
        2,
        "both pending deletions are retained: {:?}",
        snapshot.evicting_slot_ids
    );
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).expect("restore for cleanup");
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

#[test]
fn owned_read_only_directories_left_by_tests_are_deleted_whole() {
    // Effigy fixtures under a slot's child scratch leave
    // `.effigy/state/objects/<hash>` read-only (2026-10-25: slot 1987's
    // recorded deletion could not complete and blocked every lease).
    let fixture = Fixture::new("readonly-tree");
    let slot = fixture.dir("slot");
    let objects = slot.join("scratch/.tmpX/.effigy/state/objects");
    let object = objects.join("4bcd");
    fs::create_dir_all(&object).expect("object dir");
    fs::write(object.join("data"), b"x").expect("object file");
    fs::set_permissions(object.join("data"), fs::Permissions::from_mode(0o444)).expect("ro file");
    fs::set_permissions(&object, fs::Permissions::from_mode(0o555)).expect("ro dir");
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o500)).expect("ro parent");

    remove_tree_owned(fixture.path(), &slot, None).expect("owned read-only tree is removed");
    assert!(!slot.exists(), "the whole slot is gone");
}

#[test]
fn a_symlink_inside_a_read_only_directory_is_unlinked_not_followed() {
    let fixture = Fixture::new("readonly-symlink");
    let slot = fixture.dir("slot");
    let outside = fixture.dir("outside");
    let inner = slot.join("ro");
    fs::create_dir(&inner).expect("inner");
    symlink(&outside, inner.join("link")).expect("symlink");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o500)).expect("ro inner");
    // The owned symlink is unlinked as a leaf; its target is never touched.
    remove_tree_owned(fixture.path(), &slot, None).expect("owned symlink leaf removed");
    assert!(outside.exists(), "symlink target untouched");
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
    let (third, walks) = walk_observer::observe(|| lease_injected(small));
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
    let second = lease_injected(second).expect("admission");
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

/// Finder metadata recreated after the walk listed a slot directory, and
/// before its removal, completes the deletion instead of leaving it pending.
#[test]
fn finder_metadata_recreated_during_a_slot_deletion_is_drained() {
    let fixture = Fixture::new("race-metadata");
    let slot = fixture.dir("slot");
    let inner = slot.join("debug");
    fs::create_dir(&inner).expect("nested directory");
    fs::write(inner.join("blob"), b"x").expect("blob");
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("slot"), "metadata", false);
    remove_tree_owned(fixture.path(), &slot, None).expect("owned raced tree is removed");
    assert!(!slot.exists(), "the whole raced slot is gone");
}

/// A recreated entry that is not tolerated Finder metadata stays a refusal and
/// is left exactly as the writer made it.
#[test]
fn a_foreign_entry_recreated_during_a_slot_deletion_refuses_and_is_kept() {
    let fixture = Fixture::new("race-foreign");
    let slot = fixture.dir("slot");
    fs::write(slot.join("blob"), b"keep").expect("blob");
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("slot"), "foreign", false);
    match remove_tree_owned(fixture.path(), &slot, None) {
        Err(LeaseError::UnexpectedEntry { place, entry }) => {
            assert_eq!(place, "slot directory");
            assert_eq!(entry, "foreign.txt");
        }
        other => panic!("a foreign entry must refuse: {other:?}"),
    }
    assert_eq!(
        fs::read(slot.join("foreign.txt")).expect("foreign entry kept"),
        vec![b'd'; 16]
    );
    assert!(slot.is_dir(), "the slot with foreign content is kept");
}

/// A metadata-named link is refused by name: it is never followed, and neither
/// the link nor the directory it names is removed.
#[test]
fn a_metadata_named_link_recreated_during_a_slot_deletion_is_not_followed() {
    let fixture = Fixture::new("race-link");
    let slot = fixture.dir("slot");
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("slot"), "link", false);
    match remove_tree_owned(fixture.path(), &slot, None) {
        Err(LeaseError::UnexpectedEntry { place, entry }) => {
            assert_eq!(place, "slot directory");
            assert_eq!(entry, ".DS_Store");
        }
        other => panic!("a metadata-named link must refuse: {other:?}"),
    }
    assert!(fs::symlink_metadata(slot.join(".DS_Store"))
        .expect("link kept")
        .file_type()
        .is_symlink());
    assert!(slot.is_dir());
}

/// A writer that recreates metadata on every attempt uses the fixed bound and
/// leaves the deletion unfinished; it never becomes a success.
#[test]
fn repeated_metadata_recreation_leaves_the_slot_deletion_unfinished() {
    let fixture = Fixture::new("race-exhaust");
    let slot = fixture.dir("slot");
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("slot"), "metadata", true);
    assert_eq!(
        remove_tree_owned(fixture.path(), &slot, None),
        Err(LeaseError::UnknownAccounting)
    );
    assert!(slot.is_dir(), "the slot that was not removed is kept");
}

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

/// Foreign content racing the same reconciliation keeps both the slot and its
/// recorded intent: status reports the slot and never claims a deletion.
#[test]
fn a_foreign_entry_racing_an_evicting_reconciliation_keeps_the_record() {
    let fx = Fixture::new("race-evicting-foreign");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    release_injected(&pool, &leased.lease_id).expect("release");
    let slot = leased.slot_id.parse::<u64>().expect("slot id");
    mark_evicting_for_test(&pool, slot).expect("mark evicting");
    let slot_path = pool.join("payload/slots").join(&leased.slot_id);

    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "foreign", false);
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the unfinished slot stays recorded");
    assert_eq!(after.slots[0].state, SlotOccupancy::Abandoned);
    assert!(slot_path.is_dir(), "the slot with foreign content is kept");
}

/// An owned directory the owner left without read permission but with its own
/// search bit (a build or test scratch directory) is still deleted whole: the
/// repair takes a search-only descriptor of that entry, revalidates its
/// identity, and changes only that entry's own mode.
#[test]
fn an_owned_directory_left_without_read_permission_is_still_deleted_whole() {
    let fixture = Fixture::new("unreadable-tree");
    let slot = fixture.dir("slot");
    let inner = slot.join("locked");
    fs::create_dir(&inner).expect("locked directory");
    fs::write(inner.join("data"), b"x").expect("data");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o100)).expect("search-only directory");
    remove_tree_owned(fixture.path(), &slot, None).expect("owned unreadable tree is removed");
    assert!(!slot.exists(), "the whole slot is gone");
}

/// A directory with no search bit cannot be reached by any descriptor as this
/// user. Repairing it would need a chmod through its name, which could follow
/// a replacement link, so the deletion refuses and keeps the tree instead of
/// chmodding through a name.
#[test]
fn an_owned_unreachable_directory_is_kept_not_chmodded() {
    let fixture = Fixture::new("unreachable-tree");
    let slot = fixture.dir("slot");
    let inner = slot.join("locked");
    fs::create_dir(&inner).expect("locked directory");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o000)).expect("unreachable directory");
    assert_eq!(
        remove_tree_owned(fixture.path(), &slot, None),
        Err(LeaseError::UnsafePath(
            "an owned directory that cannot be opened is kept, never chmodded through its name"
        ))
    );
    assert_eq!(
        fs::symlink_metadata(&inner)
            .expect("kept directory")
            .permissions()
            .mode()
            & 0o777,
        0o000,
        "the kept directory's mode is unchanged"
    );
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
}

/// The same for a directory that keeps only write and search bits.
#[test]
fn an_owned_write_only_directory_is_still_deleted_whole() {
    let fixture = Fixture::new("write-only-tree");
    let slot = fixture.dir("slot");
    let inner = slot.join("locked");
    fs::create_dir(&inner).expect("locked directory");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o300)).expect("write-only directory");
    remove_tree_owned(fixture.path(), &slot, None).expect("owned write-only tree is removed");
    assert!(!slot.exists(), "the whole slot is gone");
}

/// The no-follow permission repair, driven through the real lease deletion
/// path: an owned unreadable directory is rebound to a symlink at exactly the
/// repair boundary, and the repair refuses instead of chmodding the referent.
#[test]
fn a_rebound_repair_target_is_refused_and_its_referent_is_untouched() {
    let fixture = Fixture::new("repair-rebind");
    let outside = fixture.dir("outside");
    let referent = outside.join("referent");
    fs::create_dir(&referent).expect("referent");
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o000)).expect("referent mode");
    let slot = fixture.dir("slot");
    let locked = slot.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");

    let (fixture_path, referent_path) = (fixture.path().to_path_buf(), referent.clone());
    let _rebind = crate::test_fs::before_repair(move || {
        // The slot and the entry are both captured under private names by this
        // point, so those are the names the racer has to replace.
        let captures = |dir: &Path| -> Vec<std::path::PathBuf> {
            fs::read_dir(dir)
                .expect("directory listed")
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"))
                })
                .collect()
        };
        let slot_capture = captures(&fixture_path)
            .into_iter()
            .next()
            .expect("the slot is captured under a private name");
        let entry_capture = captures(&slot_capture)
            .into_iter()
            .next()
            .expect("the entry is captured under a private name");
        fs::rename(&entry_capture, slot_capture.join("captured-moved"))
            .expect("rename the capture away");
        symlink(&referent_path, &entry_capture).expect("replacement link at the private name");
    });
    // The deletion refuses: the repair's no-follow open sees a link, not the
    // verified directory.
    assert!(remove_tree_owned(fixture.path(), &slot, None).is_err());
    assert_eq!(
        fs::symlink_metadata(&referent)
            .expect("referent metadata")
            .permissions()
            .mode()
            & 0o777,
        0o000,
        "the referent's mode changed through the repair"
    );
    // The captured object is never removed: the slot is restored under its
    // recorded name and the racer's replacement link is still inside it.
    assert!(
        fs::read_dir(&slot)
            .expect("restored slot listed")
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_symlink())),
        "the replacement link is untouched"
    );
    fs::set_permissions(&referent, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
    // The racer moved the capture aside and put a writable link in its place, so
    // the only restrictive leftover is that moved capture.
    for entry in fs::read_dir(&slot).expect("restored slot listed") {
        let path = entry.expect("entry").path();
        if fs::symlink_metadata(&path)
            .expect("metadata")
            .file_type()
            .is_dir()
        {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
        }
    }
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

/// The 4 MiB capture-era cap is gone: the #105 predicate alone decides, so a
/// large tolerated `.DS_Store` is still removed like any other metadata.
#[test]
fn a_large_tolerated_metadata_file_is_still_removed() {
    let fixture = Fixture::new("metadata-no-size-cap");
    let slot = fixture.dir("slot");
    fs::write(slot.join(".DS_Store"), vec![b'd'; 5 * 1024 * 1024]).expect("large metadata");
    remove_tree_owned(fixture.path(), &slot, None).expect("the owned tree is removed");
    assert!(
        !slot.exists(),
        "a large tolerated metadata file does not block deletion"
    );
}

/// An owner's directory mode is preserved exactly when a refusal after the
/// owner-access repair puts the directory back.
#[test]
fn a_refused_repair_restores_the_original_directory_mode() {
    let fixture = Fixture::new("repair-mode-roundtrip");
    let slot = fixture.dir("slot");
    let inner = slot.join("locked");
    fs::create_dir(&inner).expect("locked directory");
    fs::write(inner.join("data"), b"x").expect("data");
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o100)).expect("search-only mode");
    // The finder race seam fires in the slot's own drain; a foreign entry left
    // there keeps the deletion unfinished after the repair widened the mode.
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("locked"), "foreign", true);
    assert!(remove_tree_owned(fixture.path(), &slot, None).is_err());
    assert_eq!(
        fs::symlink_metadata(&inner)
            .expect("kept directory")
            .permissions()
            .mode()
            & 0o7777,
        0o100,
        "the owner's original mode was not restored"
    );
    fs::set_permissions(&inner, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
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

/// The root capture is journaled intent-first through the real deletion: the
/// planned record exists while the slot is still at its recorded name, and the
/// captured phase is recorded only after the exclusive rename — with the
/// root's own identity and its full mode in the record.
#[test]
fn the_root_capture_record_is_durable_before_the_rename() {
    let fixture = Fixture::new("capture-order");
    let slot = fixture.dir("slot");
    let inner = slot.join("locked");
    fs::create_dir(&inner).expect("nested directory");
    fs::write(inner.join("blob"), b"x").expect("blob");
    fs::set_permissions(&slot, fs::Permissions::from_mode(0o500)).expect("owner mode");
    let slot_metadata = fs::symlink_metadata(&slot).expect("slot metadata");

    let mut journal = RecordingJournal {
        slot: &slot,
        events: Vec::new(),
        records: Vec::new(),
        seen_modes: Vec::new(),
    };
    remove_tree_owned(fixture.path(), &slot, Some(&mut journal)).expect("owned tree removed");

    assert_eq!(
        journal
            .events
            .iter()
            .map(|(stage, ..)| *stage)
            .collect::<Vec<_>>(),
        vec!["planned", "captured"],
        "the record is written once before and once after the rename"
    );
    let (_, planned, original_present, captures_at_plan) = &journal.events[0];
    assert!(
        *original_present && *captures_at_plan == 0,
        "the planned record was not written while the slot was at its own name"
    );
    assert_eq!(
        planned.phase,
        crate::owned_delete::RootCapturePhase::Planned
    );
    assert_eq!(
        (planned.device, planned.inode),
        (slot_metadata.dev(), slot_metadata.ino()),
        "the record keeps the root's own identity"
    );
    assert_eq!(
        planned.mode_bits(),
        0o500,
        "the record keeps the owner's mode"
    );
    assert_eq!(
        (planned.parent_device, planned.parent_inode),
        (
            fs::symlink_metadata(slot.parent().expect("parent"))
                .expect("parent metadata")
                .dev(),
            fs::symlink_metadata(slot.parent().expect("parent"))
                .expect("parent metadata")
                .ino()
        ),
        "the record keeps the verified parent's identity"
    );
    let (_, captured, original_present, captures_at_capture) = &journal.events[1];
    assert!(
        !*original_present && *captures_at_capture == 1,
        "the captured phase was recorded before the exclusive rename"
    );
    // The seam hands the same plan to the captured step; recording the
    // captured phase is the caller's own journal step (the pool tests assert
    // the lease's ledger then reads `Captured`).
    assert_eq!(captured, planned, "both steps describe one object");
    assert!(!slot.exists(), "the tree is gone");
    assert!(capture_names(fixture.path()).is_empty());
}

/// End the real deletion at one root-capture stage the way a crash does: no
/// put-back, no journal write and no ledger write runs after the panic.
fn crash_eviction(pool: &Path, slot_id: u64, stage: crate::test_fs::RootCaptureCrash) {
    let slot_id = slot_id.to_string();
    let armed = crate::test_fs::root_capture_crash(stage);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        evict_idle_slot_injected(pool, &slot_id)
    }));
    drop(armed);
    assert!(
        outcome.is_err(),
        "the armed crash at {stage:?} did not fire"
    );
}

/// A crash after the planned record and before the rename: the ledger names the
/// planned private name while the slot is still at its own, and the next open
/// reconciles both names and finishes the deletion.
#[test]
fn a_crash_before_the_rename_recovers_the_recorded_slot() {
    let fixture = Fixture::new("crash-planned");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "crash-planned");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();

    crash_eviction(&pool, slot_id, crate::test_fs::RootCaptureCrash::Planned);
    let journal = super::pool::capture_record_injected(&pool, slot_id).expect("planned record");
    assert_eq!(
        journal.root.phase,
        crate::owned_delete::RootCapturePhase::Planned,
        "the planned phase is what the crashed run recorded"
    );
    assert!(slot_path.is_dir(), "the slot is still at its recorded name");
    assert!(
        capture_names(&parent).is_empty(),
        "no private name exists before the rename"
    );

    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded deletion completed");
    assert!(!slot_path.exists(), "the slot is gone");
    assert!(
        capture_names(&parent).is_empty(),
        "no capture is left behind"
    );
    assert!(
        super::pool::capture_record_injected(&pool, slot_id).is_none(),
        "the reconciled record is gone"
    );
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

/// A crash mid-delete with a recorded nested capture and an unrecorded private
/// name beside it. Replay restores the recorded capture under its observed name
/// and puts the root back; the unrecorded private name is never removed, so the
/// resumed deletion refuses and the slot stays pending — never a success.
#[test]
fn a_crash_mid_delete_restores_the_captured_root_and_keeps_unknown_captures() {
    let fixture = Fixture::new("crash-mid-delete");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "crash-mid-delete");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    // One nested capture that was recorded and not yet removed…
    fs::write(captured.join("keep.txt"), b"recorded").expect("nested entry");
    let held = File::open(&captured).expect("captured root descriptor");
    let keep_name = std::ffi::CString::new("keep.txt").expect("name");
    let entry = crate::owned_delete::capture_entry(held.as_fd(), &keep_name).expect("nested");
    let facts = crate::owned_delete::observe_entry(held.as_fd(), &entry.private).expect("facts");
    let record = crate::owned_delete::CaptureRecord {
        path: Vec::new(),
        private: String::from_utf8_lossy(entry.private.as_bytes()).into_owned(),
        original: "keep.txt".to_owned(),
        device: facts.device,
        inode: facts.inode,
        mode: u32::from(facts.file_type()) | 0o600,
    };
    // …and an unrecorded private name the crash left behind.
    fs::write(
        captured.join(".oxide-capture-0123456789abcdef"),
        b"unrecorded",
    )
    .expect("unrecorded private name");
    let recorded_private = record.private.clone();
    write_journal(&pool, slot_id, plan, true, vec![record], Vec::new());

    let after = status_injected(&pool).expect("status");
    assert_eq!(
        after.slots.len(),
        1,
        "an unrecorded private name keeps the deletion pending"
    );
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(slot_path.is_dir(), "the recorded root was put back");
    // The recorded capture was put back under its observed name: its private
    // name is gone from the tree (the resumed walk may have re-deleted the file
    // itself, and its order is the filesystem's, so only the name is asserted).
    assert!(
        !slot_path.join(&recorded_private).exists(),
        "the recorded capture's private name is resolved"
    );
    assert!(
        slot_path.join(".oxide-capture-0123456789abcdef").is_file(),
        "the unrecorded private name is untouched"
    );
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
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

#[test]
fn a_crash_after_widening_the_capture_restores_the_owners_exact_mode() {
    let fixture = Fixture::new("crash-mode");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "crash-mode");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o500)).expect("owner mode");
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    assert_eq!(
        plan.mode_bits(),
        0o500,
        "the durable record keeps the owner's mode, not the widened one"
    );
    let captured = parent.join(&plan.private);
    // The product widened the captured root before it walked the tree; the
    // crash left it widened.
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o700)).expect("widened");
    assert_eq!(mode_of(&captured), 0o700);

    // The recorded recovery restores the captured tree under its recorded name
    // with exactly the recorded mode.
    let held = File::open(&parent).expect("parent descriptor");
    let name = std::ffi::CString::new(slot_path.file_name().expect("slot leaf").as_encoded_bytes())
        .expect("leaf name");
    crate::owned_delete::restore_root_capture(held.as_fd(), &name, &plan).expect("put back");
    assert!(!captured.exists(), "the private name is free again");
    assert_eq!(
        mode_of(&slot_path),
        0o500,
        "the owner's exact mode was restored through the record"
    );

    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded deletion completed");
    assert!(!slot_path.exists());
    assert!(capture_names(&parent).is_empty());
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

/// A **real** eviction refused after the root was captured and recorded, using
/// the production journal: the Finder race refuses inside the captured slot, the
/// slot is put back, and the next pool open reconciles both names and finishes
/// the deletion. This is the representation the replay must read.
#[test]
fn a_refused_eviction_after_the_root_capture_recovers_on_the_next_open() {
    let fixture = Fixture::new("real-refusal");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "real-refusal");
    let race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "foreign", false);
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "the raced eviction refuses"
    );
    drop(race);
    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the production journal is retained");
    assert_eq!(
        journal.root.phase,
        crate::owned_delete::RootCapturePhase::Captured
    );
    assert!(
        journal
            .captures
            .iter()
            .all(|record| record.private != journal.root.private),
        "the root is recorded by its plan, never as a nested capture"
    );
    assert!(
        slot_path.is_dir(),
        "the refused slot is back at its recorded name"
    );
    assert!(private_names_in(slot_path.parent().expect("parent")).is_empty());

    let after = status_injected(&pool).expect("status");
    assert!(
        after.slots.is_empty(),
        "the next open completes the deletion"
    );
    assert!(!slot_path.exists());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
}

/// A vanished capture remains uncertain when the live run restored its root.
#[test]
fn a_vanished_capture_under_a_restored_root_remains_pending() {
    let fixture = Fixture::new("live-vanish");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "live-vanish");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "metadata", false);
    // Just before the drain unlinks the private name it recorded, take that
    // name away: the live run must not treat the missing name as its removal.
    let vanish = crate::test_fs::before_unlink_at(1, move || remove_private_files(&parent));
    assert!(super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err());
    drop(vanish);
    drop(race);
    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the vanished capture's record is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == ".DS_Store"),
        "the vanished capture was recorded before it was removed"
    );

    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1);
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
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

#[test]
fn a_changed_pending_root_is_not_journaled_or_deleted() {
    let fixture = Fixture::new("changed-pending-root");
    let root = fixture.dir("root");
    let slot = root.join("slot");
    fs::create_dir(&slot).unwrap();
    let actual = fs::symlink_metadata(&slot).unwrap();
    fs::write(slot.join("keep"), b"unchanged").unwrap();
    let expected = (actual.dev(), actual.ino().wrapping_add(1));
    let result = super::remove_tree_owned_checked(&root, &slot, None, Some(expected));
    assert!(matches!(
        result,
        Err(LeaseError::UnsafePath(
            "pending root changed after its completeness preflight"
        ))
    ));
    assert_eq!(fs::read(slot.join("keep")).unwrap(), b"unchanged");
}

#[test]
fn a_private_pending_tree_over_the_entry_bound_is_not_journaled() {
    let fixture = Fixture::new("private-pending-bound");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "pending");
    mark_evicting_for_test(&pool, slot_id).unwrap();
    for index in 0..40 {
        fs::write(slot_path.join(format!("entry-{index}")), b"unchanged").unwrap();
    }
    let project = fixture.dir("other-project");
    let workspace = fixture.dir("other-workspace");
    with_accounting_limits(20, 64, 60_000, || {
        let result = lease_injected(request(&pool, &project, &workspace, "other"));
        assert!(result.is_ok(), "unrelated admission: {result:?}");
    });
    assert_eq!(fs::read(slot_path.join("entry-39")).unwrap(), b"unchanged");
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
}

#[test]
fn a_journaled_search_only_root_over_the_bound_restores_mode_and_stays_pending() {
    let fixture = Fixture::new("journaled-search-only-bound");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "pending");
    let parent = slot_path.parent().unwrap().to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    for index in 0..40 {
        fs::write(captured.join(format!("entry-{index}")), b"unchanged").unwrap();
    }
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o100)).unwrap();
    write_journal(
        &pool,
        slot_id,
        crate::owned_delete::RootCapturePlan {
            mode: u32::from(plan.mode & u32::from(libc::S_IFMT)) | 0o100,
            ..plan
        },
        true,
        Vec::new(),
        Vec::new(),
    );
    let before = fs::read(pool.join("ledger.json")).unwrap();
    let widened = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&widened);
    let hook = crate::test_fs::after_step_chmod(move || {
        observed.fetch_add(1, Ordering::Relaxed);
    });
    let status = with_accounting_limits(20, 64, 60_000, || status_injected(&pool)).unwrap();
    drop(hook);
    assert_eq!(
        widened.load(Ordering::Relaxed),
        1,
        "root was widened for preflight"
    );
    assert_eq!(status.slots.len(), 1);
    assert_eq!(mode_of(&captured), 0o100);
    assert!(!slot_path.exists());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_some());
    assert_eq!(before, fs::read(pool.join("ledger.json")).unwrap());
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(fs::read(captured.join("entry-39")).unwrap(), b"unchanged");
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

/// A competing admission waits for the pool lock while eviction is in flight,
/// then succeeds after the first holder finishes.
#[test]
fn a_competing_admission_waits_for_an_inflight_eviction() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    if let Some(pool) = std::env::var_os("OXIDE_175_CHILD_POOL") {
        let project = PathBuf::from(std::env::var_os("OXIDE_175_CHILD_PROJECT").expect("project"));
        let workspace =
            PathBuf::from(std::env::var_os("OXIDE_175_CHILD_WORKSPACE").expect("workspace"));
        let ready = PathBuf::from(std::env::var_os("OXIDE_175_CHILD_READY").expect("ready"));
        fs::write(ready, b"ready").expect("child ready");
        lease_injected(request(Path::new(&pool), &project, &workspace, "second"))
            .expect("second process admits after the eviction");
        return;
    }
    let fixture = Fixture::new("concurrent-eviction");
    let (pool, _, _) = idle_slot_for_eviction(&fixture, "concurrent-eviction");
    let first_project = fixture.dir("first-project");
    let second_project = fixture.dir("second-project");
    let workspace = fixture.dir("shared-workspace");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let mut first_request = request(&pool, &first_project, &workspace, "first");
    first_request.budget_bytes = Some(1);
    let first = thread::spawn(move || {
        let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "metadata", false);
        let _pause = crate::test_fs::before_unlink_at(1, move || {
            entered_tx.send(()).expect("pause reached");
            resume_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("pause released");
        });
        lease_injected(first_request)
    });
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("eviction paused");
    let ready = fixture.path().join("second-ready");
    let mut second = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "lease::tests::a_competing_admission_waits_for_an_inflight_eviction",
        ])
        .env("OXIDE_175_CHILD_POOL", &pool)
        .env("OXIDE_175_CHILD_PROJECT", &second_project)
        .env("OXIDE_175_CHILD_WORKSPACE", &workspace)
        .env("OXIDE_175_CHILD_READY", &ready)
        .spawn()
        .expect("start second process");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let started = ready.exists();
    thread::sleep(Duration::from_millis(250));
    let waiting = second.try_wait().expect("second process status").is_none();
    resume_tx.send(()).expect("resume eviction");
    let first_result = first.join().expect("first worker");
    let second_result = second.wait().expect("second process exit");
    assert!(
        started && waiting,
        "second process did not wait for the pool lock"
    );
    assert!(
        second_result.success(),
        "second process exit: {second_result}"
    );
    assert!(first_result.is_ok(), "first admission: {first_result:?}");
}

/// A panic after the product widened the captured root's mode restores the
/// owner's exact mode through the scope guard, and the recorded recovery then
/// puts the root back.
#[test]
fn a_panic_after_the_mode_change_restores_the_owner_mode() {
    let fixture = Fixture::new("panic-mode");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "panic-mode");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o100)).expect("owner mode");
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "metadata", false);
    let abort = crate::test_fs::before_capture(1, || panic!("modelled panic after the widening"));
    let slot_id_text = slot_id.to_string();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        super::pool::evict_idle_slot_injected(&pool, &slot_id_text)
    }));
    drop(abort);
    assert!(outcome.is_err(), "the armed panic did not fire");
    let captured = private_names_in(&parent);
    assert_eq!(
        captured.len(),
        1,
        "the root is under its recorded private name"
    );
    assert_eq!(
        mode_of(&captured[0]),
        0o100,
        "the guard restored the owner's exact mode on the panic path"
    );

    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded recovery completed");
    assert!(!slot_path.exists());
}

/// A failed restore keeps the deletion pending, and the next open restores the
/// recorded mode and finishes once the recorded object is back in place.
#[test]
fn a_failed_root_restore_stays_pending_then_replay_restores_and_finishes() {
    let fixture = Fixture::new("failed-restore");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "failed-restore");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o100)).expect("owner mode");
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::set_permissions(&captured, fs::Permissions::from_mode(0o700)).expect("widened");
    // A foreign object claims the recorded name, so no put-back is possible.
    let foreign = fixture.dir("not-the-root");
    fs::rename(&foreign, &slot_path).expect("foreign at the recorded name");
    let blocked = status_injected(&pool).expect("status");
    assert_eq!(blocked.slots.len(), 1, "the failed restore stays pending");
    assert_eq!(blocked.coverage, AccountingCoverage::Unknown);
    assert_eq!(mode_of(&captured), 0o700, "the mode was not restored yet");

    // The obstruction goes; the next open restores the recorded mode, puts the
    // root back and finishes the deletion.
    fs::remove_dir(&slot_path).expect("obstruction removed");
    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded recovery completed");
    assert!(!slot_path.exists());
    assert!(private_names_in(&parent).is_empty());
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

/// The owner-access repair widens a captured nested directory, so its durable
/// capture record must be written **before** that widening, and a later refusal
/// in the same tree must restore the owner's exact mode. Both are asserted
/// through the real deletion and a journal that reads the live mode at the
/// moment it records the capture.
#[test]
fn a_nested_repair_widening_is_recorded_first_and_restored_on_refusal() {
    let fixture = Fixture::new("repair-record");
    let slot = fixture.dir("slot");
    let locked = slot.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");

    // The race refuses inside `locked`, i.e. after the repair widened it.
    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("locked"), "foreign", false);
    let mut journal = RecordingJournal {
        slot: &slot,
        events: Vec::new(),
        records: Vec::new(),
        seen_modes: Vec::new(),
    };
    assert!(
        remove_tree_owned(fixture.path(), &slot, Some(&mut journal)).is_err(),
        "the raced nested directory refuses"
    );

    let recorded = journal
        .seen_modes
        .iter()
        .find(|(original, _)| original == "locked")
        .expect("the captured directory was recorded");
    assert_eq!(
        recorded.1, 0o100,
        "the durable record was written while the directory still had the owner's mode"
    );
    assert_eq!(
        mode_of(&locked),
        0o100,
        "the refusal restored the owner's exact mode after the repair widened it"
    );
    assert!(slot.is_dir(), "the refused slot is kept");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("fixture cleanup");
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

/// A replacement that takes the deleted private name **after** the repair
/// widened the verified directory is neither chmodded nor renamed: the verified
/// original's mode is restored through its verified descriptor, the replacement
/// stays exactly where the racer put it, the observed name stays free, and the
/// journal record stays outstanding (pending) in the production ledger.
#[test]
fn a_replacement_after_the_repair_keeps_both_names_and_stays_pending() {
    let fixture = Fixture::new("reopen-replacement");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "reopen-replacement");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let locked = slot_path.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");

    let fixture_path = fixture.path().to_path_buf();
    let _rebind = crate::test_fs::before_child_reopen(move || {
        // The captured directory is under a private name inside the captured
        // slot: move the verified object aside and put a different directory at
        // that name, so the reopen sees a replacement.
        let captured_slot = find_private_dir(&fixture_path).expect("the captured slot");
        let captured_locked = find_private_dir(&captured_slot).expect("the captured directory");
        fs::rename(&captured_locked, captured_slot.join("moved-away"))
            .expect("the verified directory is moved aside");
        fs::create_dir(&captured_locked).expect("replacement at the private name");
        fs::set_permissions(&captured_locked, fs::Permissions::from_mode(0o500))
            .expect("replacement mode");
    });
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "a replaced capture refuses"
    );

    // The slot is back at its recorded name and both objects are preserved
    // exactly where they were: the replacement under the private name, the
    // verified original where the racer moved it, and the observed name free.
    assert!(slot_path.is_dir(), "the slot is restored");
    assert!(
        !slot_path.join("locked").exists(),
        "nothing was renamed into the observed name"
    );
    let replacement = find_dir_by_mode(&slot_path, 0o500).expect("replacement kept");
    assert!(
        replacement
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-")),
        "the replacement is still at its private name"
    );
    let original = find_dir_by_mode(&slot_path, 0o100).expect("verified original kept");
    assert_eq!(
        mode_of(&original),
        0o100,
        "the verified directory's owner mode was restored through its verified descriptor"
    );

    // Journal disposition in the production ledger: the entry's capture record
    // is still outstanding, so the deletion stays pending rather than complete.
    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == "locked"),
        "the replaced entry's capture record is kept: {journal:?}"
    );
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the deletion stays pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    assert!(
        slot_path
            .join(replacement.file_name().expect("replacement name"))
            .is_dir(),
        "the replacement is still at its private name after replay"
    );
    let _ = parent;
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("cleanup");
    fs::set_permissions(&original, fs::Permissions::from_mode(0o700)).expect("cleanup");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o700)).expect("cleanup");
}

/// A reopen that fails because the private name is no longer an openable
/// directory restores the verified original's owner mode through its verified
/// descriptor, moves nothing (the link stays at the private name and the
/// observed name stays free), follows nothing, and keeps the journal pending.
#[test]
fn a_failed_reopen_keeps_both_names_and_stays_pending() {
    let fixture = Fixture::new("reopen-link");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "reopen-link");
    let locked = slot_path.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");
    let outside = fixture.dir("outside");
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).expect("outside mode");

    let (fixture_path, outside_path) = (fixture.path().to_path_buf(), outside.clone());
    let _rebind = crate::test_fs::before_child_reopen(move || {
        let captured_slot = find_private_dir(&fixture_path).expect("the captured slot");
        let captured_locked = find_private_dir(&captured_slot).expect("the captured directory");
        fs::rename(&captured_locked, captured_slot.join("moved-away"))
            .expect("the verified directory is moved aside");
        symlink(&outside_path, &captured_locked).expect("a link at the private name");
    });
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "a failed reopen refuses"
    );

    assert!(slot_path.is_dir(), "the slot is restored");
    assert!(
        !slot_path.join("locked").exists(),
        "the link was not renamed into the observed name"
    );
    let link = fs::read_dir(&slot_path)
        .expect("slot listed")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"))
                && fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
        .expect("the link is still at its private name");
    assert_eq!(
        fs::read_link(&link).expect("link target"),
        outside,
        "the link still names exactly what the racer put there"
    );
    assert_eq!(
        fs::symlink_metadata(&outside)
            .expect("outside kept")
            .permissions()
            .mode()
            & 0o7777,
        0o700,
        "nothing was chmodded through the link"
    );
    let original = find_dir_by_mode(&slot_path, 0o100).expect("verified original kept");
    assert_eq!(
        mode_of(&original),
        0o100,
        "the failed reopen restored the verified original's owner mode"
    );

    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == "locked"),
        "the unresolved entry's capture record is kept"
    );
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the deletion stays pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    for path in [&slot_path, &original, &outside] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).expect("cleanup");
    }
    let _ = link;
}

/// A replacement created at the private name **before** the repair opens it is
/// neither repaired nor renamed: the whole deletion refuses, the replacement
/// stays at its private name with its own mode, the observed name stays free,
/// and the production ledger keeps the capture record so the slot stays pending.
#[test]
fn a_replacement_before_the_repair_keeps_both_names_and_stays_pending() {
    let fixture = Fixture::new("repair-replacement");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "repair-replacement");
    let locked = slot_path.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");

    let fixture_path = fixture.path().to_path_buf();
    let _rebind = crate::test_fs::before_repair(move || {
        let captured_slot = find_private_dir(&fixture_path).expect("the captured slot");
        let captured_locked = find_private_dir(&captured_slot).expect("the captured directory");
        fs::rename(&captured_locked, captured_slot.join("moved-away"))
            .expect("the verified directory is moved aside");
        fs::create_dir(&captured_locked).expect("replacement at the private name");
        fs::set_permissions(&captured_locked, fs::Permissions::from_mode(0o500))
            .expect("replacement mode");
    });
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "a replacement before the repair refuses"
    );

    assert!(slot_path.is_dir(), "the slot is restored");
    assert!(
        !slot_path.join("locked").exists(),
        "the replacement was not renamed into the observed name"
    );
    let replacement = find_dir_by_mode(&slot_path, 0o500).expect("replacement kept");
    assert!(
        replacement
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-")),
        "the replacement is still at its private name"
    );
    assert_eq!(
        mode_of(&replacement),
        0o500,
        "the replacement keeps its own mode"
    );
    assert_eq!(
        mode_of(&find_dir_by_mode(&slot_path, 0o100).expect("verified original kept")),
        0o100,
        "the verified original keeps the owner's mode"
    );

    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == "locked"),
        "the replaced entry's capture record is kept"
    );
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the deletion stays pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("cleanup");
}

/// A symlink planted at the private name before the repair opens it is never
/// followed, never repaired and never renamed: both names and the production
/// journal stay exactly as they were, pending.
#[test]
fn a_symlink_before_the_repair_keeps_both_names_and_stays_pending() {
    let fixture = Fixture::new("repair-link");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "repair-link");
    let locked = slot_path.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o100)).expect("search-only");
    let outside = fixture.dir("outside");
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).expect("outside mode");

    let (fixture_path, outside_path) = (fixture.path().to_path_buf(), outside.clone());
    let _rebind = crate::test_fs::before_repair(move || {
        let captured_slot = find_private_dir(&fixture_path).expect("the captured slot");
        let captured_locked = find_private_dir(&captured_slot).expect("the captured directory");
        fs::rename(&captured_locked, captured_slot.join("moved-away"))
            .expect("the verified directory is moved aside");
        symlink(&outside_path, &captured_locked).expect("a link at the private name");
    });
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "a link before the repair refuses"
    );

    assert!(slot_path.is_dir(), "the slot is restored");
    assert!(
        !slot_path.join("locked").exists(),
        "the link was not renamed into the observed name"
    );
    let link = fs::read_dir(&slot_path)
        .expect("slot listed")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".oxide-capture-"))
                && fs::symlink_metadata(path)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
        .expect("the link is still at its private name");
    assert_eq!(
        fs::read_link(&link).expect("link target"),
        outside,
        "the link still names exactly what the racer put there"
    );
    assert_eq!(
        fs::symlink_metadata(&outside)
            .expect("outside kept")
            .permissions()
            .mode()
            & 0o7777,
        0o700,
        "nothing was repaired or chmodded through the link"
    );
    assert_eq!(
        mode_of(&find_dir_by_mode(&slot_path, 0o100).expect("verified original kept")),
        0o100,
        "the verified original keeps the owner's mode"
    );

    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == "locked"),
        "the unresolved entry's capture record is kept"
    );
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the deletion stays pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("cleanup");
}

/// A whole-slot capture that picks up a replacement is recorded as a foreign
/// capture before anything is renamed: the replacement is preserved, the record
/// survives reopening, and replay keeps the deletion pending instead of
/// clearing it and retrying the slot as ordinary owned content.
#[test]
fn a_replaced_whole_slot_capture_is_foreign_and_stays_pending() {
    let fixture = Fixture::new("slot-capture-foreign");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "slot-capture-foreign");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();

    // The racer takes the observed slot away and leaves its own directory at
    // the recorded name, after the walk observed it and before the capture.
    let (slot_for_race, parent_for_race) = (slot_path.clone(), parent.clone());
    let _race = crate::test_fs::before_walk_capture(move || {
        fs::rename(&slot_for_race, parent_for_race.join("moved-away"))
            .expect("the observed slot is moved aside");
        fs::create_dir(&slot_for_race).expect("replacement at the recorded name");
        fs::write(slot_for_race.join("racer-marker"), b"racer").expect("replacement marker");
    });
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "a replaced whole-slot capture refuses"
    );

    // The racer's replacement was captured under the private name and kept
    // exactly as it was; the recorded name stays free.
    assert!(
        !slot_path.exists(),
        "nothing was renamed back into the recorded name"
    );
    let captured = private_names_in(&parent)
        .into_iter()
        .find(|path| path.is_dir())
        .expect("the captured replacement");
    assert_eq!(
        fs::read(captured.join("racer-marker")).expect("replacement preserved"),
        b"racer",
        "the racer's object is preserved exactly as it was"
    );

    // The owning journal holds the unexpected object as a foreign capture, and
    // it survives reopening: the deletion stays pending and never retries.
    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert_eq!(
        journal.foreign.len(),
        1,
        "the unexpected object is recorded as a foreign capture: {journal:?}"
    );
    assert!(
        !journal.foreign[0].restored,
        "nothing was put back for the foreign capture"
    );
    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the deletion stays pending");
    assert_eq!(after.coverage, AccountingCoverage::Unknown);
    let reopened = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the record survives reopening");
    assert_eq!(reopened.foreign.len(), 1, "the foreign record is kept");
    assert_eq!(
        private_names_in(&parent).len(),
        1,
        "no second capture was made: the deletion did not retry"
    );
}

/// An entry name that cannot be journaled exactly is refused **before** it is
/// captured: nothing is renamed or deleted, the escaped bytes are reported, and
/// the deletion stays pending. APFS refuses to create a non-UTF-8 name at all,
/// so the walk's check is fed that input through the debug-only seam and the
/// production refusal path is what is asserted; the predicate itself is also
/// checked directly against real non-UTF-8 bytes.
#[test]
fn an_entry_name_that_cannot_round_trip_refuses_before_capture() {
    // The predicate: real non-UTF-8 bytes are rejected with their escaped form.
    let odd = std::ffi::CString::new(b"caf\xe9".to_vec()).expect("name");
    assert_eq!(
        crate::owned_delete::journal_name(&odd),
        Err("an entry name is not valid UTF-8 (caf\\xe9)".to_owned()),
        "a lossy conversion would silently journal a different name"
    );
    let lossy = String::from_utf8_lossy(b"caf\xe9").into_owned();
    assert_ne!(
        lossy.as_bytes(),
        b"caf\xe9",
        "a lossy conversion changes bytes, which is why the name is refused"
    );

    let fixture = Fixture::new("invalid-name");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "invalid-name");
    let locked = slot_path.join("locked");
    fs::create_dir(&locked).expect("locked directory");
    fs::write(locked.join("data"), b"x").expect("data");

    let forced = crate::test_fs::force_invalid_name();
    let outcome = super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string());
    drop(forced);
    match outcome {
        Err(LeaseError::UnexpectedEntry { place, entry }) => {
            assert_eq!(place, "slot tree entry");
            assert_eq!(
                entry,
                slot_path.file_name().expect("slot leaf").to_string_lossy(),
                "the refused entry is named"
            );
        }
        other => panic!("a name that cannot round-trip must refuse before capture: {other:?}"),
    }

    // Nothing was captured or renamed, and the deletion stays pending.
    assert!(slot_path.is_dir(), "the slot is restored");
    assert!(locked.is_dir(), "the refused entry is untouched");
    assert_eq!(
        private_names_in(&slot_path).len(),
        0,
        "nothing was captured: the name was refused before the capture"
    );
    // Nothing was journaled for the refused attempt, so the recorded deletion
    // stays pending until a retry can run; here the platform's real names are
    // UTF-8, so that retry completes instead of touching the refused entry.
    let after = status_injected(&pool).expect("status");
    assert!(
        after.slots.is_empty(),
        "the pending deletion completed on retry"
    );
    assert!(!slot_path.exists());
}

/// A captured entry whose name is valid multi-byte UTF-8 is journaled exactly
/// and put back under exactly that name when the deletion refuses.
#[test]
fn a_live_putback_restores_the_exact_non_ascii_name() {
    let fixture = Fixture::new("exact-name");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "exact-name");
    let odd = slot_path.join("café");
    fs::create_dir(&odd).expect("non-ASCII directory");
    fs::write(odd.join("data"), b"x").expect("data");

    let _race = crate::test_fs::arm_race(std::ffi::OsStr::new("*"), "foreign", false);
    assert!(
        super::pool::evict_idle_slot_injected(&pool, &slot_id.to_string()).is_err(),
        "the raced deletion refuses"
    );

    let journal = super::pool::capture_record_injected(&pool, slot_id)
        .expect("the capture journal is retained");
    assert!(
        journal
            .captures
            .iter()
            .any(|record| record.original == "café"),
        "the journal records the exact UTF-8 name: {journal:?}"
    );
    assert!(
        odd.is_dir(),
        "the live put-back restored the entry under its exact original name"
    );

    // The next open releases the recorded capture and finishes the deletion,
    // which is only possible because the recorded name round-trips exactly.
    let after = status_injected(&pool).expect("status");
    assert!(after.slots.is_empty(), "the recorded deletion completed");
    assert!(!slot_path.exists());
    assert!(super::pool::capture_record_injected(&pool, slot_id).is_none());
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

/// An interrupted capture that is still private replays to its **exact**
/// multi-byte UTF-8 original name — never a replacement-character name.
#[test]
fn a_still_private_multibyte_capture_replays_to_its_exact_name() {
    let fixture = Fixture::new("exact-replay");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "exact-replay");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    let captured = parent.join(&plan.private);
    fs::create_dir(captured.join("café")).expect("multibyte directory");
    let held = File::open(&captured).expect("captured root descriptor");
    let name = std::ffi::CString::new("café").expect("name");
    let entry = crate::owned_delete::capture_entry(held.as_fd(), &name).expect("capture");
    let facts = crate::owned_delete::observe_entry(held.as_fd(), &entry.private).expect("facts");
    let record = crate::owned_delete::CaptureRecord {
        path: Vec::new(),
        private: entry.private.to_string_lossy().into_owned(),
        original: "café".to_owned(),
        device: facts.device,
        inode: facts.inode,
        mode: facts.mode as u32,
    };
    // An unrecorded private name makes the resumed walk refuse, so the restored
    // name is observable after replay instead of being deleted again.
    fs::write(
        captured.join(".oxide-capture-0123456789abcdef"),
        b"unrecorded",
    )
    .expect("unrecorded private name");
    write_journal(&pool, slot_id, plan, true, vec![record.clone()], Vec::new());
    assert_eq!(record.original, "café");
    assert!(
        !record.original.contains('\u{fffd}'),
        "the journal never holds a replacement-character name"
    );

    let after = status_injected(&pool).expect("status");
    assert_eq!(after.slots.len(), 1, "the unrecorded name keeps it pending");
    let restored = slot_path.join("café");
    assert!(
        restored.is_dir(),
        "the captured entry replayed to its exact original name"
    );
    assert_eq!(
        fs::symlink_metadata(&restored)
            .expect("restored name")
            .permissions()
            .mode()
            & 0o7777,
        0o755,
        "and the restored directory keeps its own mode"
    );
    assert!(
        std::fs::read_dir(&slot_path)
            .expect("slot listed")
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains('\u{fffd}')),
        "no replacement-character name exists anywhere in the tree"
    );
    fs::set_permissions(&slot_path, fs::Permissions::from_mode(0o700)).expect("cleanup");
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
// Per-deletion custody (oxide#181, frozen design oxide#180 slice 1).
//
// The deletion still runs under the pool lock in this slice: these regressions
// cover the fence, the custody lock protocol, journal migration, replay from
// the per-deletion file and the final record transaction.
// ---------------------------------------------------------------------------

/// The journal file and stable lock of one published deletion, as a test reads
/// them without opening the pool.
fn custody_files(pool: &Path, nonce: &str) -> (PathBuf, PathBuf) {
    let dir = pool.join(MANAGED_NAME).join("deletions").join(nonce);
    (dir.join("lock"), dir.join("journal.json"))
}

/// The recorded custody of one slot, read straight from the ledger.
fn recorded_custody(pool: &Path, slot_id: u64) -> Option<super::custody::DeletionCustody> {
    let bytes = fs::read(pool.join("ledger.json")).ok()?;
    let ledger: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let row = ledger["slots"]
        .as_array()?
        .iter()
        .find(|slot| slot["id"] == serde_json::json!(slot_id))?;
    serde_json::from_value(row.get("custody")?.clone()).ok()
}

/// The pre-oxide#181 reader: the strict ledger and slot shape an older binary
/// parses, plus its policy check. It exists here so the fence is proved
/// against the *actual* predecessor shape rather than against a comment.
mod pre_custody_reader {
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Ledger {
        version: u32,
        store_id: String,
        cap_bytes: u64,
        next_slot: u64,
        next_lease: u64,
        #[serde(default)]
        capacity_policy: Option<u32>,
        #[serde(default)]
        next_rank: Option<u64>,
        slots: Vec<Slot>,
    }

    /// The predecessor's exact strict slot shape: every field it knows, and
    /// `deny_unknown_fields` so a `custody` field refuses the ledger.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Slot {
        id: u64,
        incarnation: u64,
        state: serde_json::Value,
        project: serde_json::Value,
        workspace: serde_json::Value,
        profile: serde_json::Value,
        holder: String,
        lease_id: Option<String>,
        last_lease_id: String,
        dir: Option<String>,
        rank: u64,
        recorded: Option<serde_json::Value>,
        capture: Option<serde_json::Value>,
    }

    /// The predecessor's read path: a strict parse, then its accepted policy
    /// set (`1..=4`). Anything else is refused before any effect.
    pub(super) fn accepts(bytes: &[u8]) -> bool {
        let Ok(ledger) = serde_json::from_slice::<Ledger>(bytes) else {
            return false;
        };
        if ledger.version != 1 || ledger.next_slot == 0 {
            return false;
        }
        match ledger.capacity_policy {
            None => true,
            Some(1..=4) => true,
            Some(_) => false,
        }
    }
}

/// The deletion-custody fence refuses every older supported reader before any
/// effect, and it does so in the intermediate state (control fenced, ledger
/// not yet) as well as the final one.
#[test]
fn the_custody_fence_refuses_the_predecessor_reader_in_both_states() {
    let fixture = Fixture::new("custody-fence");
    let pool = fixture.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fixture.dir("project");
    let workspace = fixture.dir("workspace");
    // A mutating command publishes the policy-5 ledger fence and the front
    // door's version-9 control fence.
    let leased = lease_injected(request(&pool, &project, &workspace, "holder")).expect("lease");
    release_injected(&pool, &leased.lease_id).expect("release");
    let bytes = fs::read(pool.join("ledger.json")).expect("ledger");
    let stored: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        stored["capacity_policy"],
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION)
    );
    assert!(
        !pre_custody_reader::accepts(&bytes),
        "the predecessor reader accepted the final policy-5 ledger"
    );

    // The intermediate state: the control fence is durable, the ledger is
    // still the predecessor's policy-4 shape. The predecessor's control reader
    // refuses the version-9 document (it accepts only versions at most 8), and
    // the ledger it would then read carries no custody record yet.
    let mut intermediate = stored.clone();
    intermediate["capacity_policy"] = serde_json::json!(RAISED_CAPACITY_POLICY_VERSION);
    let intermediate_bytes = serde_json::to_vec(&intermediate).expect("intermediate");
    assert!(
        pre_custody_reader::accepts(&intermediate_bytes),
        "the intermediate ledger must still be the predecessor's shape"
    );
    assert!(
        !pre_custody_reader::accepts(
            &serde_json::to_vec(&serde_json::json!({
                "version": 9,
                "next_id": 1,
                "next_use": 1,
                "tickets": [],
                "cleans": [],
                "records": [],
                "recorded_accounting": "oxide.accounting/1",
                "eviction_accounting": "oxide.eviction-accounting/1",
                "slot_ceiling": "oxide.slots/256",
                "deletion_custody": "oxide.deletion-custody/1",
            }))
            .expect("control")
        ),
        "the predecessor ledger reader accepted a version-9 control document"
    );

    // A policy-5 ledger that still carries a legacy in-ledger journal is a
    // state no writer produces, and the strict predecessor reader refuses the
    // `custody` field as an unknown field in the other direction.
    let mut with_journal = stored.clone();
    with_journal["slots"][0]["capture"] = serde_json::json!({
        "root": {
            "private": format!("{}{}", crate::owned_delete::CAPTURE_PREFIX, "a".repeat(32)),
            "phase": "planned",
            "device": 1,
            "inode": 2,
            "parent_device": 1,
            "parent_inode": 3,
            "mode": 0o40700,
        },
        "captures": [],
        "foreign": [],
    });
    assert!(
        super::pool::write_ledger_bytes_injected(
            &pool,
            &serde_json::to_vec(&with_journal).expect("journaled")
        )
        .is_err(),
        "a policy-5 ledger with an in-ledger journal was accepted"
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A real deletion publishes its private custody directory, a stable lock and
/// an initial journal **before** any payload mutation, under the policy-5
/// fence. The crash at the planned boundary leaves exactly that state, and the
/// next open replays it from the per-deletion file and retires the directory.
#[test]
fn custody_and_the_initial_journal_are_durable_before_the_payload_effect() {
    let fixture = Fixture::new("custody-before-payload");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "custody-before-payload");
    crash_eviction(&pool, slot_id, crate::test_fs::RootCaptureCrash::Planned);

    // The payload is untouched.
    assert!(slot_path.is_dir(), "the payload was deleted before custody");
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert_eq!(stored["slots"][0]["state"], "evicting");
    assert_eq!(
        stored["capacity_policy"],
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION),
        "the custody record was published without the ledger fence"
    );
    assert!(
        stored["slots"][0].get("capture").is_none(),
        "the journal is still in the ledger"
    );
    let custody = recorded_custody(&pool, slot_id).expect("custody record");
    assert!(custody.is_valid());
    let (lock, journal_path) = custody_files(&pool, &custody.nonce);
    let lock_metadata = fs::symlink_metadata(&lock).expect("stable lock");
    assert!(lock_metadata.is_file());
    assert_eq!(lock_metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(lock_metadata.ino(), custody.lock_inode);
    assert_eq!(lock_metadata.dev(), custody.lock_device);
    let journal_metadata = fs::symlink_metadata(&journal_path).expect("journal file");
    assert_eq!(journal_metadata.permissions().mode() & 0o777, 0o600);
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("journal bytes")).expect("json");
    assert_eq!(document["nonce"], serde_json::json!(custody.nonce));
    assert_eq!(document["slot"], serde_json::json!(slot_id));
    assert_eq!(document["capture"]["root"]["phase"], "planned");

    // The next open replays from that file, deletes the tree and drops the
    // record; the private directory is retired while custody is held.
    let after = status_injected(&pool).expect("replay");
    assert!(after.slots.is_empty(), "the replay did not conclude");
    assert!(!slot_path.exists(), "the tree is gone");
    assert!(
        !lock.parent().expect("custody dir").exists(),
        "the retired custody directory was kept"
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A busy custody lock is a live custodian: the record, its charge and its
/// payload are kept, and an unrelated admission is neither refused nor made to
/// wait for it. A missing, replaced or identity-mismatched lock is unknown and
/// keeps the record too.
#[test]
fn a_busy_custody_lock_is_skipped_and_an_unknown_one_keeps_the_record() {
    use std::os::fd::AsRawFd;

    let fixture = Fixture::new("custody-locks");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "custody-locks");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let held_root = File::open(&parent).expect("parent");
    let leaf = slot_path.file_name().expect("leaf");
    let leaf_name = std::ffi::CString::new(leaf.as_encoded_bytes()).expect("name");
    let observed =
        crate::owned_delete::observe_entry(held_root.as_fd(), &leaf_name).expect("observed");
    let plan = crate::owned_delete::plan_root_capture(held_root.as_fd(), &observed).expect("plan");
    mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    super::pool::write_capture_record_injected(
        &pool,
        slot_id,
        super::pool::DeletionJournal {
            root: plan.clone(),
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("planned journal");
    let custody = recorded_custody(&pool, slot_id).expect("custody record");
    let (lock_path, journal_path) = custody_files(&pool, &custody.nonce);
    assert!(journal_path.is_file());

    // A live custodian holds the exclusive lock.
    let held = File::open(&lock_path).expect("custody lock");
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);

    // The pass skips this object only: the record and payload stay, and a new
    // project still admits.
    let status = status_injected(&pool).expect("a busy custody is not a refusal");
    assert_eq!(status.slots.len(), 1, "the busy record was dropped");
    assert!(slot_path.is_dir(), "the busy payload was touched");
    assert_eq!(
        super::pool::slot_state_injected(&pool, &slot_id.to_string()).as_deref(),
        Some("evicting")
    );
    let project = fixture.dir("project-second");
    let workspace = fixture.dir("workspace-second");
    let admitted = lease_injected(request(&pool, &project, &workspace, "second"))
        .expect("a busy custody must not refuse an unrelated admission");
    assert_ne!(admitted.slot_id, slot_id.to_string());
    release_injected(&pool, &admitted.lease_id).expect("release");

    // Missing: the record is kept, never completed and never re-created.
    fs::rename(&lock_path, parent.join("moved-lock")).expect("hide the lock");
    let status = status_injected(&pool).expect("a missing lock is not a global refusal");
    assert_eq!(status.slots.len(), 2, "the record was dropped with no lock");
    assert!(slot_path.is_dir());
    assert!(
        !lock_path.exists(),
        "an unknown custody lock was re-created"
    );
    fs::rename(parent.join("moved-lock"), &lock_path).expect("restore lock");

    // Replaced: a new file at the same name has a different identity and is
    // refused as unknown, even though the name exists.
    let replaced = tempfile_path(&parent, "replacement-lock");
    fs::write(&replaced, b"").expect("replacement lock");
    fs::set_permissions(&replaced, fs::Permissions::from_mode(0o600)).expect("mode");
    fs::rename(&replaced, &lock_path).expect("replace the lock");
    let status = status_injected(&pool).expect("a replaced lock is not a global refusal");
    assert_eq!(
        status.slots.len(),
        2,
        "a replaced lock completed a deletion"
    );

    // Identity mismatch: restoring the original name is not enough; the
    // recorded (device, inode) is what authorizes custody.
    assert!(slot_path.is_dir());
    assert_eq!(
        plan.device,
        fs::symlink_metadata(&slot_path).expect("root").dev()
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// One small helper: a fresh path inside `parent`.
fn tempfile_path(parent: &Path, label: &str) -> PathBuf {
    let unique = NEXT.fetch_add(1, Ordering::SeqCst);
    parent.join(format!(".{label}-{unique}"))
}

/// A record published by a writer that predates the custody fence keeps its
/// journal in the ledger. The next open migrates it byte-exactly into a
/// private custody directory, publishes the policy-5 fence and clears the
/// ledger copy, then applies the ordinary replay outcome to the migrated
/// journal.
#[test]
fn a_legacy_in_ledger_journal_migrates_once_and_is_not_reinterpreted() {
    let fixture = Fixture::new("custody-migration");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "custody-migration");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let held = File::open(&parent).expect("parent");
    let leaf = slot_path.file_name().expect("leaf");
    let name = std::ffi::CString::new(leaf.as_encoded_bytes()).expect("name");
    let observed = crate::owned_delete::observe_entry(held.as_fd(), &name).expect("observed");
    let plan = crate::owned_delete::plan_root_capture(held.as_fd(), &observed).expect("plan");
    mark_evicting_for_test(&pool, slot_id).expect("recorded deletion");
    super::pool::write_legacy_capture_record_injected(
        &pool,
        slot_id,
        super::pool::DeletionJournal {
            root: plan.clone(),
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("legacy journal");
    // The root disappears without its durable `removed` phase, so replay keeps
    // the object pending: that is the outcome the migrated journal must get.
    fs::remove_dir_all(&slot_path).expect("hide the root");
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert!(before["slots"][0]["capture"].is_object());
    assert!(before["slots"][0].get("custody").is_none());
    assert_ne!(
        before["capacity_policy"],
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION)
    );

    let status = status_injected(&pool).expect("migration");
    assert_eq!(status.slots.len(), 1, "the pending record was dropped");
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    assert_eq!(
        after["capacity_policy"],
        serde_json::json!(DELETION_CUSTODY_CAPACITY_POLICY_VERSION),
        "the migrated record was published without its fence"
    );
    assert!(
        after["slots"][0].get("capture").is_none(),
        "the legacy journal stayed in the ledger"
    );
    let custody = recorded_custody(&pool, slot_id).expect("migrated custody");
    let (_, journal_path) = custody_files(&pool, &custody.nonce);
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(&journal_path).expect("journal")).expect("json");
    assert_eq!(document["capture"]["root"]["phase"], "planned");
    assert_eq!(
        document["capture"]["root"]["private"],
        serde_json::json!(plan.private),
        "the migrated journal was reinterpreted"
    );
    assert_eq!(
        super::pool::capture_record_injected(&pool, slot_id)
            .expect("migrated journal")
            .root
            .private,
        plan.private
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// The final transaction reloads the ledger from disk and drops only the
/// matching nonce and generation: a concurrent admission written after this
/// deletion published its record is preserved.
#[test]
fn the_final_record_transaction_preserves_a_concurrent_ledger_update() {
    let fixture = Fixture::new("custody-final-merge");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "custody-final-merge");
    let plan = capture_root_state(&slot_path, slot_id, &pool, false);
    let custody = recorded_custody(&pool, slot_id).expect("custody record");
    // Restore the modelled capture so the fixture leaves a pending record.
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    fs::rename(parent.join(&plan.private), &slot_path).expect("restore the root");

    // A concurrent admission: a second idle row appended after this deletion
    // published. It is a plain ledger update, not a custody change.
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(pool.join("ledger.json")).expect("ledger")).expect("json");
    let mut concurrent = ledger["slots"][0].clone();
    concurrent["id"] = serde_json::json!(slot_id + 1000);
    concurrent["state"] = serde_json::json!("idle");
    concurrent["lease_id"] = serde_json::json!(null);
    concurrent["rank"] = serde_json::json!(9);
    concurrent
        .as_object_mut()
        .expect("object")
        .remove("custody");
    concurrent
        .as_object_mut()
        .expect("object")
        .remove("capture");
    ledger["slots"]
        .as_array_mut()
        .expect("slots")
        .push(concurrent);
    ledger["next_rank"] = serde_json::json!(10);
    ledger["next_slot"] = serde_json::json!(slot_id + 1001);
    let bytes = super::pool::write_ledger_bytes_injected(
        &pool,
        &serde_json::to_vec(&ledger).expect("ledger"),
    )
    .expect("concurrent update");

    let merged = super::pool::drop_record_on_disk(&pool, slot_id, 1, &custody).expect("final drop");
    let merged_json: serde_json::Value =
        serde_json::from_slice(&serde_json::to_vec(&merged).expect("json")).expect("merged json");
    let rows = merged_json["slots"].as_array().expect("rows");
    assert!(
        rows.iter()
            .all(|slot| slot["id"] != serde_json::json!(slot_id)),
        "the deleted record survived"
    );
    assert!(
        rows.iter().any(|slot| {
            slot["id"] == serde_json::json!(slot_id + 1000) && slot["state"] == "idle"
        }),
        "the concurrent admission was lost"
    );
    let stored = fs::read(pool.join("ledger.json")).expect("ledger");
    assert_eq!(
        serde_json::to_vec(&merged).expect("json"),
        stored,
        "the merged ledger is not what was written"
    );
    assert!(
        bytes.starts_with(b"{"),
        "the concurrent write did not replace the ledger"
    );
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// A crash after the durable root `removed` phase but before the record drops
/// repeats the final transaction without touching payload, and the second
/// open cannot resurrect the record.
#[test]
fn a_crash_after_removed_repeats_the_final_transaction_without_payload_effect() {
    let fixture = Fixture::new("custody-removed");
    let (pool, slot_id, slot_path) = idle_slot_for_eviction(&fixture, "custody-removed");
    let parent = slot_path.parent().expect("slot parent").to_path_buf();
    let plan = capture_root_state(&slot_path, slot_id, &pool, true);
    fs::remove_dir_all(parent.join(&plan.private)).expect("the payload is gone");
    let removed = crate::owned_delete::RootCapturePlan {
        phase: crate::owned_delete::RootCapturePhase::Removed,
        ..plan
    };
    super::pool::rewrite_custody_journal_injected(
        &pool,
        slot_id,
        super::pool::DeletionJournal {
            root: removed,
            captures: Vec::new(),
            foreign: Vec::new(),
        },
    )
    .expect("removed phase");
    let custody = recorded_custody(&pool, slot_id).expect("custody record");
    let (lock_path, _) = custody_files(&pool, &custody.nonce);

    let status = status_injected(&pool).expect("final transaction");
    assert!(status.slots.is_empty(), "the record was not dropped");
    assert!(
        !lock_path.parent().expect("custody dir").exists(),
        "the custody directory was kept after completion"
    );
    // A second open has nothing to resurrect.
    let again = status_injected(&pool).expect("second open");
    assert!(again.slots.is_empty());
    assert!(root_dir_empty(&pool));
    fs::remove_dir_all(&fixture.path).expect("cleanup");
}

/// Whether the pool's custody holdings directory has no live deletion.
fn root_dir_empty(pool: &Path) -> bool {
    let holdings = pool.join(MANAGED_NAME).join("deletions");
    match fs::read_dir(holdings) {
        Ok(entries) => entries.filter_map(|entry| entry.ok()).next().is_none(),
        Err(_) => true,
    }
}
