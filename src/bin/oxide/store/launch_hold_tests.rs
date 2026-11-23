//! The real child launch keeps the lease's root hold. The test runs in its own
//! test process: a spawned child inherits every inheritable descriptor of the
//! process, so running it beside other fixtures would make their lock probes
//! observe this child's copies.

use super::*;

use crate::cli::Profile;
use crate::identity::SlotKey;

const NAME: &str =
    "store::launch_hold_tests::the_real_launch_keeps_the_lease_hold_until_the_child_exits";

#[test]
fn the_real_launch_keeps_the_lease_hold_until_the_child_exits() {
    if std::env::var_os("OXIDE_LAUNCH_HOLD_CHILD").is_none() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!("oxide-launch-hold-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("OXIDE_LAUNCH_HOLD_CHILD", "1")
            .env("OXIDE_LAUNCH_ROOT", &root)
            .env("TMPDIR", &root)
            .env("TMP", &root)
            .env("TEMP", &root)
            .status()
            .expect("isolated child");
        let _ = fs::remove_dir_all(&root);
        assert!(status.success(), "isolated launch-hold child failed");
        return;
    }
    let root = PathBuf::from(std::env::var_os("OXIDE_LAUNCH_ROOT").unwrap());
    let pool = root.join("pool");
    oxide_storage::lease::managed::init_budget_pool(&pool, 8 << 20).unwrap();
    let store = Store {
        pool: pool.clone(),
        control: pool.join(oxide_storage::lease::MANAGED_NAME),
        payload: pool.join("payload"),
        backend: Backend::DirectoryBudget,
        budget: Some(8 << 20),
        recorded_budget: 8 << 20,
        max_active: 16,
        sibling: None,
    };
    store.ensure_control().unwrap();
    let checkout = root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    let key = SlotKey {
        checkout,
        identity: "identity".to_owned(),
        compatibility: "quick-key".to_owned(),
    };
    let (ticket, _file) = store.register_ticket().unwrap();
    let outcome = store
        .attempt(ticket, &key, Profile::Quick, Some(16), false, None)
        .unwrap();
    store.remove_ticket(ticket).unwrap();
    let lease = match outcome {
        Ok(leased) => leased,
        Err(wait) => panic!("the first command leased: {wait:?}"),
    };
    // The real launch route: the store made the hold inheritable in
    // `attempt`, so this child keeps the root held across exec.
    let mut command = std::process::Command::new("/bin/sleep");
    command.arg("0.6");
    let mut group = crate::process::spawn_group(&mut command).expect("launch");
    // The launcher drops its own copy; only the child's inherited
    // descriptor holds the root now.
    drop(lease);
    let backing = oxide_storage::storage::PoolBacking::budget(&pool, 8 << 20);
    let area = oxide_storage::storage::PoolStorage::open(
        &pool.join(oxide_storage::lease::MANAGED_NAME),
        &backing,
    )
    .unwrap();
    let roots = area.all_roots().unwrap();
    let (_, info) = roots.first().expect("one slot root").clone();
    assert!(
        area.try_hold_in(None, &info.id).unwrap().is_none(),
        "the child keeps the inherited hold"
    );
    assert!(
        area.evict_any(&info.id).is_err(),
        "a competing eviction refuses while the child holds the root"
    );
    // A separately pinned fixture is protected independently of the test.
    let pinned = area.create("pinned").unwrap();
    let pinned_area = area.area();
    pinned_area.pin(&pinned.id, "selected output").unwrap();
    assert!(
        area.evict_any(&pinned.id).is_err(),
        "the pin keeps the root"
    );
    assert!(group.child.wait().unwrap().success(), "the child exited");
    assert!(
        area.try_hold_in(None, &info.id).unwrap().is_some(),
        "the root is eligible once the child exits"
    );
    area.evict_any(&info.id)
        .expect("evicted after the child exits");
    assert!(area.evict_any(&pinned.id).is_err(), "still pinned");
    pinned_area.unpin(&pinned.id).unwrap();
    area.evict_any(&pinned.id).expect("evicted after unpin");
}
