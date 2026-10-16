use super::pool::{
    create_injected, lease_injected, mark_evicting_for_test, recover_injected, release_injected,
    status_injected,
};
use super::{
    init_pool, render_status_json, AccountingCoverage, LeaseError, LeaseRequest, Profile,
    SlotOccupancy,
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
    LeaseRequest {
        pool: pool.to_path_buf(),
        project: project.to_path_buf(),
        workspace: workspace.to_path_buf(),
        profile: Profile::Dev,
        holder: holder.to_owned(),
    }
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
    assert_eq!(report.slots[0].workspace, ws2);
    assert_eq!(report.coverage, AccountingCoverage::Complete);
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
fn known_macos_metadata_added_after_init_is_outside_slot_accounting() {
    let fx = Fixture::new("metadata-after-init");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init before OS metadata appears");
    let payload = pool.join("payload");
    for name in [
        ".fseventsd",
        ".Spotlight-V100",
        ".Trashes",
        ".TemporaryItems",
    ] {
        let metadata_dir = payload.join(name);
        fs::create_dir(&metadata_dir).expect("post-init metadata directory");
        if name == ".fseventsd" {
            fs::write(metadata_dir.join("fseventsd-uuid"), b"metadata").expect("FSEvents UUID");
        }
    }

    let status = status_injected(&pool).expect("known metadata directories are accepted");
    assert_eq!(status.coverage, AccountingCoverage::Complete);
    assert_eq!(status.payload_logical_bytes, Some(0));
    assert_eq!(status.payload_unique_allocated_bytes, Some(0));
    assert!(status.slots.is_empty());
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let grant = lease_injected(request(&pool, &project, &workspace, "holder-a"))
        .expect("lease after delayed metadata arrival");
    assert_eq!(grant.slot_id, "1");
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
fn status_json_stays_bounded() {
    let fx = Fixture::new("status");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    lease_injected(request(&pool, &project, &workspace, "holder-a")).expect("lease");
    let status = status_injected(&pool).expect("status");
    let rendered = render_status_json(&status, 64).expect("render");
    assert!(rendered.bytes.len() <= 64);
    assert!(rendered.truncated);
}

#[test]
fn failed_init_lock_only_retry_succeeds() {
    let fx = Fixture::new("init-retry");
    let pool = fx.dir("pool");
    let undersized = init_pool(&pool, 1024, 512);
    assert!(
        matches!(undersized, Err(LeaseError::InvalidInput(_))),
        "{undersized:?}"
    );
    assert!(
        !pool.join("lock").exists(),
        "cap check must run before creating lock"
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
fn product_open_refuses_directory_injection() {
    let fx = Fixture::new("product");
    let pool = fx.dir("pool");
    create_injected(&pool, 1024 * 1024).expect("init");
    let project = fx.dir("project");
    let workspace = fx.dir("workspace");
    let result = super::lease(request(&pool, &project, &workspace, "holder-a"));
    assert!(
        matches!(result, Err(LeaseError::BackingMismatch)),
        "CLI path must not treat a directory as the APFS product: {result:?}"
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
