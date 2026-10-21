use super::managed;
use super::pool::{
    create_injected, evict_idle_slot_injected, lease_injected, mark_evicting_for_test,
    recover_injected, release_injected, status_injected, with_accounting_limits,
};
use super::{
    AccountingCoverage, AccountingGapKind, AccountingScan, LeaseError, LeaseRequest, Profile,
    SlotLayout, SlotOccupancy, BUDGET_BACKEND_DIRECTORY_V1, MANAGED_NAME,
};
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
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
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
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

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
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
        layout: None,
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
fn recycle_threshold_is_the_overflow_safe_ceiling_of_half_the_cap() {
    assert_eq!(super::pool::recycle_threshold(1), 1);
    assert_eq!(super::pool::recycle_threshold(2), 1);
    assert_eq!(super::pool::recycle_threshold(3), 2);
    assert_eq!(super::pool::recycle_threshold(4), 2);
    assert!(!super::pool::should_recycle_slot(0, 1));
    assert!(!super::pool::should_recycle_slot(2, 5));
    assert!(super::pool::should_recycle_slot(3, 5));
    assert!(super::pool::should_recycle_slot(4, 5));
    assert_eq!(super::pool::recycle_threshold(u64::MAX), u64::MAX / 2 + 1);
}

#[test]
fn oversized_compatible_idle_slot_is_replaced_then_fresh_slot_is_warm() {
    let fx = Fixture::new("recycle-compatible");
    let pool = fx.dir("pool");
    let cap = 8 * 1024 * 1024;
    create_injected(&pool, cap).expect("init");
    let project = fx.dir("project");
    let first_workspace = fx.dir("first-workspace");
    let replacement_workspace = fx.dir("replacement-workspace");
    let first = lease_injected(request(&pool, &project, &first_workspace, "holder-a"))
        .expect("first lease");
    assert!(!first.reused_slot);
    let old_slot_root = pool.join("payload").join("slots").join(&first.slot_id);
    let old_payload = first.env.cargo_build_build_dir.join("generated.bin");
    fs::write(&old_payload, vec![b'x'; 5 * 1024 * 1024]).expect("allocated payload");
    release_injected(&pool, &first.lease_id).expect("release first slot");

    let idle_status = status_injected(&pool).expect("inventory idle slot");
    let allocated = idle_status
        .slots
        .iter()
        .find(|slot| slot.slot_id == first.slot_id)
        .and_then(|slot| slot.unique_allocated_bytes)
        .expect("complete allocated-byte inventory");
    assert!(allocated >= cap.div_ceil(2));

    let replacement = lease_injected(request(&pool, &project, &replacement_workspace, "holder-b"))
        .expect("replace oversized compatible slot");
    assert!(!replacement.reused_slot);
    assert_ne!(replacement.slot_id, first.slot_id);
    assert!(replacement.slot_id.parse::<u64>().unwrap() > first.slot_id.parse::<u64>().unwrap());
    assert!(lease_sequence(&replacement.lease_id) > lease_sequence(&first.lease_id));
    assert!(!old_slot_root.exists());
    assert!(!old_payload.exists());
    assert_ne!(
        replacement.env.cargo_build_build_dir,
        first.env.cargo_build_build_dir
    );
    assert!(matches!(
        release_injected(&pool, &first.lease_id),
        Err(LeaseError::NotFound)
    ));
    let held_status = status_injected(&pool).expect("replacement remains held");
    let held = held_status
        .slots
        .iter()
        .find(|slot| slot.slot_id == replacement.slot_id)
        .expect("replacement record");
    assert_eq!(held.state, SlotOccupancy::Held);
    assert_eq!(
        held.lease_id.as_deref(),
        Some(replacement.lease_id.as_str())
    );

    release_injected(&pool, &replacement.lease_id).expect("release replacement");
    let warm = lease_injected(request(&pool, &project, &replacement_workspace, "holder-c"))
        .expect("reuse fresh slot below threshold");
    assert_eq!(warm.slot_id, replacement.slot_id);
    assert!(warm.reused_slot);
    assert!(lease_sequence(&warm.lease_id) > lease_sequence(&replacement.lease_id));
}

#[test]
fn oversized_recycling_targets_only_the_selected_project_and_profile() {
    let fx = Fixture::new("recycle-scope");
    let pool = fx.dir("pool");
    create_injected(&pool, 8 * 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let other_project = fx.dir("other-project");
    let workspace = fx.dir("workspace");

    let other = lease_injected(request(&pool, &other_project, &workspace, "other-project"))
        .expect("unrelated project slot");
    let other_marker = other.env.cargo_build_build_dir.join("keep-project");
    fs::write(&other_marker, vec![b'p'; 64 * 1024]).expect("unrelated project payload");
    release_injected(&pool, &other.lease_id).expect("release unrelated project");

    let other_profile = lease_injected(request_with_profile(
        &pool,
        &project,
        &workspace,
        "other-profile",
        Profile::FullDebug,
    ))
    .expect("unrelated profile slot");
    let profile_marker = other_profile.env.cargo_build_build_dir.join("keep-profile");
    fs::write(&profile_marker, vec![b'f'; 64 * 1024]).expect("unrelated profile payload");
    release_injected(&pool, &other_profile.lease_id).expect("release unrelated profile");

    let selected = lease_injected(request(&pool, &project, &workspace, "selected"))
        .expect("selected compatible slot");
    let selected_root = pool.join("payload").join("slots").join(&selected.slot_id);
    let selected_payload = selected.env.cargo_build_build_dir.join("oversized");
    fs::write(&selected_payload, vec![b's'; 5 * 1024 * 1024]).expect("selected payload");
    release_injected(&pool, &selected.lease_id).expect("release selected slot");

    let replacement = lease_injected(request(&pool, &project, &workspace, "replacement"))
        .expect("replace only selected compatible slot");
    assert!(!replacement.reused_slot);
    assert_ne!(replacement.slot_id, selected.slot_id);
    assert!(!selected_root.exists());
    assert!(!selected_payload.exists());
    assert_eq!(
        fs::read(&other_marker).expect("other project survives"),
        vec![b'p'; 64 * 1024]
    );
    assert_eq!(
        fs::read(&profile_marker).expect("other profile survives"),
        vec![b'f'; 64 * 1024]
    );
    let status = status_injected(&pool).expect("status after selected replacement");
    assert!(status
        .slots
        .iter()
        .any(|slot| slot.slot_id == other.slot_id));
    assert!(status
        .slots
        .iter()
        .any(|slot| slot.slot_id == other_profile.slot_id));
    assert!(status
        .slots
        .iter()
        .all(|slot| slot.slot_id != selected.slot_id));
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
    assert!(matches!(
        lease_injected(request(
            &pool,
            &fx.dir("blocked-project"),
            &workspace,
            "blocked-holder"
        )),
        Err(LeaseError::UnknownAccounting)
    ));
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
        let scan = incomplete(managed::lease(request(&pool, &fresh, &workspace, "new")));
        assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::EntryLimit]);
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
        assert_eq!(status.slots.len(), before.slots.len());
    });
    // The pool bounds cover the same tree: the big slot is reused as the
    // same generation and a new slot is admitted.
    let reused = managed::lease(request(&pool, &big, &workspace, "reuse")).expect("reuse");
    assert!(reused.reused_slot);
    assert_eq!(
        (reused.slot_id.as_str(), reused.incarnation.as_str()),
        (slots[0].slot_id.as_str(), slots[0].incarnation.as_str())
    );
    let admitted = managed::lease(request(&pool, &fresh, &workspace, "new")).expect("new slot");
    assert!(!admitted.reused_slot);
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
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    managed::release(&pool, &grant.lease_id).expect("release");
    let reuse = managed::lease(request(&pool, &project, &workspace, "b"));
    // Over the tiny budget with the only idle slot unmeasurable: no eviction.
    let admit = managed::lease(request(&pool, &fx.dir("other"), &workspace, "c"));
    let status = managed::status(&pool);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("restore");
    for scan in [incomplete(reuse), incomplete(admit)] {
        assert_eq!(scan.coverage, AccountingCoverage::Partial);
        assert_eq!(gap_kinds(&scan), vec![AccountingGapKind::UnreadableEntry]);
    }
    let status = status.expect("status");
    assert_eq!(status.slots.len(), 1);
    assert_eq!(status.slots[0].state, SlotOccupancy::Idle);
    assert_eq!(status.full, None);
    assert_eq!(
        gap_kinds(&status.slots[0].accounting),
        vec![AccountingGapKind::UnreadableEntry]
    );
    assert!(grant.env.cargo_build_build_dir.join("blob").exists());
    // Measurable again, the idle slot is evicted whole at admission.
    let admitted =
        managed::lease(request(&pool, &fx.dir("third"), &workspace, "d")).expect("admitted");
    assert_ne!(admitted.slot_id, grant.slot_id);
    assert!(!grant.env.cargo_build_build_dir.exists());
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
    let members: Vec<super::LedgerSlot> = status.slots.iter().map(Into::into).collect();
    assert_eq!(view.slots, members);
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
    assert_eq!(first_walks, vec!["payload".to_owned()]);
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
    lease_injected(request(&denied, &denied_project, &workspace, "d")).expect("lease");
    let slots = denied.join("payload").join("slots");
    fs::set_permissions(&slots, fs::Permissions::from_mode(0o000)).expect("deny");
    let from_status = status_injected(&denied).map(|_| ());
    let from_view = ledger_injected(&denied).map(|_| ());
    fs::set_permissions(&slots, fs::Permissions::from_mode(0o700)).expect("restore");
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
    let members: Vec<super::LedgerSlot> = status.slots.iter().map(Into::into).collect();
    assert_eq!(view.slots, members);
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
