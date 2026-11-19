//! The pinned Barge sweep must not hold the area lock during tree deletion.

use std::fs;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use barge::testing::TempDir;
use barge::{FakeOs, ManualClock, Mode};
use oxide_storage::storage::{AreaConfig, Storage, POOL_AREA};

#[test]
fn hold_completes_while_trash_tree_is_being_deleted() {
    let temp = TempDir::new("sweep-hold").unwrap();
    let mount = temp.path().join("mount");
    fs::create_dir(&mount).unwrap();
    let storage = Storage::open_with(
        temp.path().join("state"),
        Arc::new(FakeOs::with_volume(
            &mount,
            "8E4A5C1D-0000-4000-8000-00000000BEEF",
            100 << 30,
        )),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let area = storage
        .declare(
            &AreaConfig::new("Builds", &mount, POOL_AREA)
                .folder("oxide-pool/slots")
                .floor_bytes(0),
        )
        .unwrap();
    let doomed = area.create_root("doomed").unwrap();
    let survivor = area.create_root("survivor").unwrap();
    // Count entries while deletion runs, so the hold is attempted during the
    // walk rather than merely racing the start of the sweep.
    for i in 0..20_000 {
        fs::write(doomed.path.join(format!("{i:05}")), b"x").unwrap();
    }
    let trash = doomed.path.parent().unwrap().join(".trash");
    fs::create_dir_all(&trash).unwrap();
    let entry = trash.join("doomed");
    fs::rename(&doomed.path, &entry).unwrap();
    let claimed = trash.join(".barge-claim-doomed");
    let sweeping = area.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let sweep = std::thread::spawn(move || {
        let result = sweeping.sweep_trash();
        done_tx.send(()).unwrap();
        result
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(entries) = fs::read_dir(&claimed) {
            let count = entries.count();
            if count < 20_000 && count > 100 {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "did not observe deletion in progress"
        );
        assert!(
            done_rx.try_recv().is_err(),
            "sweep ended before deletion was observed"
        );
        std::thread::yield_now();
    }
    let holding = area.clone();
    let (held_tx, held_rx) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let hold = holding.hold(&survivor.id, Mode::Exclusive).unwrap();
        held_tx.send(()).unwrap();
        hold
    });
    held_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("hold waited for deletion");
    assert!(
        done_rx.try_recv().is_err(),
        "sweep finished before the hold completed"
    );
    drop(holder.join().unwrap());
    let report = sweep.join().unwrap().unwrap();
    assert_eq!(report.deleted.len(), 1);
}
