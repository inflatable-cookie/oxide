use super::managed;
use super::pool::{
    create_injected, evict_idle_slot_injected, lease_injected, mark_evicting_for_test,
    recover_injected, release_injected, status_injected,
};
use super::{
    AccountingCoverage, LeaseError, LeaseRequest, Profile, SlotOccupancy,
    BUDGET_BACKEND_DIRECTORY_V1, MANAGED_NAME,
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
    assert!(matches!(
        lease_injected(request(
            &pool,
            &partial_project,
            &workspace,
            "partial-retry"
        )),
        Err(LeaseError::UnknownAccounting)
    ));
    assert!(pool
        .join("payload")
        .join("slots")
        .join(&partial.slot_id)
        .exists());

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
    assert!(matches!(result, Err(LeaseError::UnknownAccounting)));
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
        assert!(matches!(
            status_injected(&pool),
            Err(LeaseError::UnknownAccounting)
        ));
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
    assert!(matches!(
        managed::status(&pool),
        Err(LeaseError::UnknownAccounting)
    ));
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
    // The pool lock may be created; every recorded byte is unchanged.
    let after: Vec<_> = snapshot_tree(&pool)
        .into_iter()
        .filter(|(path, _)| path != Path::new("lock"))
        .collect();
    assert_eq!(before, after);
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
    let mut file = File::create(first.env.cargo_build_build_dir.join("payload.bin"))
        .expect("payload file");
    file.write_all(&vec![7_u8; 256 * 1024]).expect("grow past budget");
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
    assert!(status.slots.iter().all(|slot| slot.slot_id == third.slot_id
        || slot.state == SlotOccupancy::Idle));
    assert!(!status.slots.iter().any(|slot| slot.slot_id == first.slot_id));
    // The recorded budget is unchanged.
    assert_eq!(status.cap_bytes, 64 * 1024 * 1024);
    let mut zero = request(&pool, &project_a, &workspace, "holder-z");
    zero.budget_bytes = Some(0);
    assert!(matches!(
        managed::lease(zero),
        Err(LeaseError::InvalidInput(_))
    ));
}
