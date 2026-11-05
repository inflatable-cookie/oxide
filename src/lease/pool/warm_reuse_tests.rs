//! Actual lease-engine correctness on tiny private trees. The volume adapter
//! models verification; it is not a kernel mount/UUID qualification.
use super::*;
use std::cell::RefCell;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static PARENT_FACTS: RefCell<Option<(u64, u32, u32)>> = const { RefCell::new(None) };
    static BEFORE_COMMIT: RefCell<Option<Box<dyn FnOnce(&mut Pool)>>> = RefCell::new(None);
}

pub(super) fn before_commit(pool: &mut Pool) {
    let hook = BEFORE_COMMIT.with(|cell| cell.borrow_mut().take());
    if let Some(hook) = hook {
        hook(pool);
    }
}

// A single metadata-result projection tests device/UID/kind refusal without
// chown, mounting a filesystem, or claiming those kernel operations occurred.
pub(super) fn parent_facts(actual: (u64, u32, u32)) -> (u64, u32, u32) {
    PARENT_FACTS
        .with(|cell| cell.borrow_mut().take())
        .unwrap_or(actual)
}

struct Fixture {
    root: PathBuf,
    pool: Pool,
    request: LeaseRequest,
    slot: PathBuf,
    retained: bool,
}

impl Fixture {
    fn new(removable: bool) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let retained = std::env::var_os("OXIDE_WARM_REUSE_FIXTURE").is_some();
        let base = std::env::var_os("OXIDE_WARM_REUSE_FIXTURE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "warm-reuse-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("exclusive fixture");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let pool_root = root.join("pool");
        let project = root.join("project");
        let workspace = root.join("workspace");
        for path in [&pool_root, &project, &workspace] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut pool = if removable {
            let payload = root.join("payload");
            fs::create_dir(&payload).unwrap();
            fs::set_permissions(&payload, fs::Permissions::from_mode(0o700)).unwrap();
            fs::create_dir(payload.join(SLOTS_NAME)).unwrap();
            fs::set_permissions(payload.join(SLOTS_NAME), fs::Permissions::from_mode(0o700))
                .unwrap();
            let mut pool = volume_pool_for_test(&pool_root, &payload, 64 << 20, None).unwrap();
            pool._lock = Some(lock_pool(&pool_root).unwrap());
            pool
        } else {
            create_injected(&pool_root, 64 << 20).unwrap();
            Pool::open(&pool_root).unwrap()
        };
        let slot = add_idle_slot_for_test(&mut pool, 1, 1, 4096, 0).unwrap();
        pool.ledger.next_lease = 2; // the seeded idle row already used sequence 1
        pool.ledger.slots[0].project = project.clone();
        pool.ledger.slots[0].workspace = workspace.clone();
        pool.persist_revised().unwrap();
        let request = LeaseRequest {
            pool: pool_root,
            project,
            workspace,
            profile: Profile::Dev,
            holder: "warm-reuse-fixture".to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout: None,
            replace_slot: None,
            class: AdmissionClass::Normal,
            victims: VictimPolicy::Any,
            slice: None,
        };
        Self {
            root,
            pool,
            request,
            slot,
            retained,
        }
    }

    fn measured_request(&self) -> LeaseRequest {
        let mut request = self.request.clone();
        request.class = AdmissionClass::OneUse;
        request.slice = Some(OneUseSlice {
            members: MemberSet::new([MemberPair::new("1", "1")]),
            unknown: 0,
            kind: SliceAdmission::WarmReuse,
            max_slots: 16,
            max_bytes: 32 << 20,
        });
        request
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.retained {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn with_backend(removable: bool, body: impl FnOnce()) {
    if removable {
        crate::volume::verified_facts::with(
            crate::volume::verified_facts::VolumeFacts::default(),
            body,
        );
    } else {
        body();
    }
}

fn normal_case(removable: bool) {
    for missing in [false, true] {
        with_backend(removable, || {
            let mut fx = Fixture::new(removable);
            let mut deep = fx.slot.join("build");
            for _ in 0..70 {
                deep = deep.join("d");
                fs::create_dir(&deep).unwrap();
            }
            let unrelated = fx.root.join("unrelated");
            fs::create_dir(&unrelated).unwrap();
            symlink(&unrelated, fx.slot.join("link")).unwrap();
            let fault = if missing {
                crate::test_fs::InjectedErrorGuard::not_found_symlink_metadata(&deep)
            } else {
                crate::test_fs::InjectedErrorGuard::read_dir(&deep)
            };
            // A descendant really changes during admission; root/parent identity
            // remains stable. This is a fixture action, not actor attribution.
            BEFORE_COMMIT.with(|cell| {
                *cell.borrow_mut() = Some(Box::new(move |_| {
                    fs::rename(&deep, deep.with_file_name("changed-descendant")).unwrap();
                }))
            });
            let (leased, walks) = crate::inventory::walk_observer::observe(|| {
                with_accounting_limits(1, 0, 0, || fx.pool.lease(&fx.request))
            });
            let leased = leased.unwrap();
            assert!(walks.is_empty(), "{walks:?}");
            assert_eq!(fault.hits(), 0);
            assert!(leased.grant.reused_slot);
            assert_eq!(leased.measured, None);
            assert!(leased.holding.is_empty());
            let slot = &fx.pool.ledger.slots[0];
            assert_eq!(slot.state, SlotPersistState::Held);
            assert!(slot.recorded.is_none());
            let durable = read_ledger(&fx.request.pool).unwrap();
            assert_eq!(
                durable.slots[0].lease_id.as_ref(),
                Some(&leased.grant.lease_id)
            );
            let mut other = fx.request.clone();
            other.holder = "other".to_owned();
            assert!(matches!(fx.pool.lease(&other), Err(LeaseError::Busy)));
        });
    }
}
fn refusal_case(removable: bool, fault: &str) {
    with_backend(removable, || {
        let mut fx = Fixture::new(removable);
        let before = fs::read(fx.request.pool.join("ledger.json")).unwrap();
        let path = fx.slot.clone();
        let fault = fault.to_owned();
        BEFORE_COMMIT.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(move |pool| match fault.as_str() {
                "root-replaced" => {
                    fs::rename(&path, path.with_file_name("old-root")).unwrap();
                    fs::create_dir(&path).unwrap();
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
                }
                "parent-replaced" => {
                    let parent = path.parent().unwrap();
                    let saved = parent.with_file_name("old-parent");
                    fs::rename(parent, &saved).unwrap();
                    fs::create_dir(parent).unwrap();
                    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
                    fs::rename(saved.join(path.file_name().unwrap()), &path).unwrap();
                }
                "missing" => fs::rename(&path, path.with_file_name("missing-root")).unwrap(),
                "mode" => fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap(),
                "parent-mode" => {
                    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755))
                        .unwrap()
                }
                "kind" => {
                    fs::rename(&path, path.with_file_name("old-root")).unwrap();
                    fs::write(&path, b"file").unwrap();
                }
                "symlink" => {
                    fs::rename(&path, path.with_file_name("old-root")).unwrap();
                    symlink(path.with_file_name("old-root"), &path).unwrap();
                }
                "parent-device" => PARENT_FACTS
                    .with(|cell| *cell.borrow_mut() = Some((u64::MAX, current_uid(), 0o040700))),
                "parent-uid" => PARENT_FACTS.with(|cell| {
                    *cell.borrow_mut() = Some((
                        fs::metadata(&path).unwrap().dev(),
                        current_uid().wrapping_add(1),
                        0o040700,
                    ))
                }),
                "parent-kind" => PARENT_FACTS.with(|cell| {
                    *cell.borrow_mut() =
                        Some((fs::metadata(&path).unwrap().dev(), current_uid(), 0o100700))
                }),
                "generation" => pool.ledger.slots[0].incarnation += 1,
                "held" => pool.ledger.slots[0].state = SlotPersistState::Held,
                "profile" => pool.ledger.slots[0].profile = Profile::FullDebug,
                _ => panic!("unknown fixture fault"),
            }));
        });
        assert!(fx.pool.lease(&fx.request).is_err());
        assert_eq!(
            fs::read(fx.request.pool.join("ledger.json")).unwrap(),
            before
        );
        assert_eq!(fx.pool.ledger.slots.len(), 1);
        assert!(fx.pool.ledger.holding.is_empty());
    });
}

fn one_use_case(removable: bool) {
    with_backend(removable, || {
        let mut fx = Fixture::new(removable);
        let request = fx.measured_request();
        let denied = crate::test_fs::InjectedErrorGuard::read_dir(&fx.slot.join("build"));
        assert!(fx.pool.lease(&request).is_err());
        assert!(denied.hits() > 0);
        assert_eq!(fx.pool.ledger.slots[0].state, SlotPersistState::Idle);
        drop(denied);
        let leased = fx.pool.lease(&request).unwrap();
        assert!(leased.measured.is_some());
        assert!(leased.slice.is_some());
    });
}

#[test]
fn warm_reuse_normal_and_one_use() {
    for removable in [false, true] {
        normal_case(removable);
        one_use_case(removable);
    }
}

#[test]
fn warm_reuse_rechecks_before_grant() {
    for removable in [false, true] {
        for fault in [
            "parent-device",
            "parent-uid",
            "parent-kind",
            "root-replaced",
            "parent-replaced",
            "missing",
            "mode",
            "parent-mode",
            "kind",
            "symlink",
            "generation",
            "held",
            "profile",
        ] {
            refusal_case(removable, fault);
        }
    }
}

#[test]
fn warm_reuse_volume_and_headroom_refusals() {
    for removable in [false, true] {
        with_backend(removable, || {
            for fence in [false, true] {
                let mut fx = Fixture::new(removable);
                if fence {
                    fx.pool.ledger.capacity_policy = Some(u32::MAX);
                } else {
                    fx.pool.ledger.next_rank = Some(u64::MAX);
                }
                let before = fs::read(fx.request.pool.join("ledger.json")).unwrap();
                assert!(fx.pool.lease(&fx.request).is_err());
                assert_eq!(
                    fs::read(fx.request.pool.join("ledger.json")).unwrap(),
                    before
                );
            }
        });
    }
    let mut fx = Fixture::new(true);
    crate::volume::verified_facts::with(
        crate::volume::verified_facts::VolumeFacts {
            verify_error: Some("wrong volume identity".to_owned()),
            ..Default::default()
        },
        || {
            assert!(matches!(
                fx.pool.lease(&fx.request),
                Err(LeaseError::BackingMismatch)
            ))
        },
    );
}

/// Separately activated proof: real engine/filesystem effects, bounded by the
/// shared guard. Keep independent case outcomes; no claim of kernel mounts.
#[test]
#[ignore = "requires the separately activated qualified guard"]
fn warm_reuse_correctness_proof() {
    let root = PathBuf::from(std::env::var_os("OXIDE_WARM_REUSE_FIXTURE").expect("proof root"));
    let evidence = PathBuf::from(std::env::var_os("OXIDE_PROOF_EVIDENCE").expect("evidence"));
    assert!(root.starts_with(&evidence) && root != evidence && root.is_dir());
    let diagnostic = std::env::var_os("OXIDE_WARM_REUSE_DIAGNOSTIC").is_some();
    let mut rows = Vec::new();
    let mut cases: Vec<(String, Box<dyn FnOnce()>)> = Vec::new();
    for removable in [false, true] {
        cases.push((
            format!("removable={removable}:normal"),
            Box::new(move || normal_case(removable)),
        ));
        cases.push((
            format!("removable={removable}:one-use"),
            Box::new(move || one_use_case(removable)),
        ));
        for fault in [
            "parent-device",
            "parent-uid",
            "parent-kind",
            "root-replaced",
            "parent-replaced",
            "missing",
            "mode",
            "parent-mode",
            "kind",
            "symlink",
            "generation",
            "held",
            "profile",
        ] {
            cases.push((
                format!("removable={removable}:{fault}"),
                Box::new(move || refusal_case(removable, fault)),
            ));
        }
    }
    cases.push((
        "volume-and-headroom".to_owned(),
        Box::new(warm_reuse_volume_and_headroom_refusals),
    ));
    for (name, case) in cases {
        let passed = crate::volume::verified_facts::with(Default::default(), || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(case)).is_ok()
        });
        BEFORE_COMMIT.with(|cell| cell.borrow_mut().take());
        PARENT_FACTS.with(|cell| cell.borrow_mut().take());
        rows.push(serde_json::json!({"case": name, "passed": passed}));
        if !passed && !diagnostic {
            break;
        }
    }
    let passed = rows.len() == 31 && rows.iter().all(|row| row["passed"] == true);
    fs::write(
        evidence.join("warm-reuse-cases.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "passed": passed, "cases": rows, "synthetic_volume_binding": true,
            "kernel_mount_qualification": false
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(passed, "correctness cases failed");
}
