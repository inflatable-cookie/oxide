//! Reserve admission tests: the pool-walk budget and eviction ordering.

use super::*;
use crate::lease::tests::{request, Fixture};
use crate::storage::{count_pool_walks, PoolBacking, PoolStorage, PoolWalk, Storage};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct ScriptedSpace {
    volume: barge::FakeOs,
    reads: Mutex<VecDeque<u64>>,
}

impl barge::Os for ScriptedSpace {
    fn volume_identity(&self, mount: &Path) -> barge::Result<Option<String>> {
        barge::Os::volume_identity(&self.volume, mount)
    }

    fn free_bytes(&self, _mount: &Path) -> barge::Result<u64> {
        self.reads
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| barge::Error::unknown_state("unexpected free-space read"))
    }
}

#[test]
fn removable_admission_reads_space_and_evicts_only_needed_ledger_roots() {
    const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const RESERVE: u64 = 4 << 20;
    let fixture = Fixture::new("removable-reserve");
    let pool = fixture.pool();
    let workspace = fixture.dir("workspace");
    let mut opened = Pool::open(&pool).unwrap();
    // Only mount identity and free-space readings are synthetic. The
    // leases, records, locks and evictions use the real fixture tree.
    let os = Arc::new(ScriptedSpace {
        volume: barge::FakeOs::with_volume(pool.clone(), UUID, RESERVE),
        reads: Mutex::new(VecDeque::new()),
    });
    let account = Storage::open_with(
        fixture.dir("account"),
        os.clone(),
        Arc::new(barge::ManualClock::new(1_000)),
    )
    .unwrap();
    let storage = PoolStorage::from_storage(
        account,
        &PoolBacking::volume(&pool, UUID, "payload", 8 << 20, RESERVE, RESERVE / 2),
    )
    .unwrap();
    opened.storage = Some(storage.clone());

    *os.reads.lock().unwrap() = [RESERVE / 2 - 1].into();
    let (refused, walks) = count_pool_walks(|| {
        opened.lease(&request(&pool, &fixture.dir("floor"), &workspace, "floor"))
    });
    assert!(matches!(refused, Err(LeaseError::Full)));
    assert!(walks.is_empty());

    let mut projects = Vec::new();
    let mut roots = Vec::new();
    for i in 0..24 {
        let project = fixture.dir(&format!("project-{i}"));
        *os.reads.lock().unwrap() = [RESERVE + 1].into();
        let leased = opened
            .lease(&request(&pool, &project, &workspace, "first"))
            .unwrap();
        let slot = opened.ledger.slots.last().unwrap();
        roots.push(slot.root.clone().unwrap());
        opened.release_or_recover(&leased.grant.lease_id).unwrap();
        projects.push(project);
    }

    let new_project = fixture.dir("new");
    for (project, reused) in [(&projects[0], true), (&new_project, false)] {
        *os.reads.lock().unwrap() = [RESERVE + 1].into();
        let (leased, walks) =
            count_pool_walks(|| opened.lease(&request(&pool, project, &workspace, "above")));
        let leased = leased.unwrap();
        assert_eq!(leased.grant.reused_slot, reused);
        assert!(walks.is_empty());
        assert!(os.reads.lock().unwrap().is_empty());
        opened.release_or_recover(&leased.grant.lease_id).unwrap();
    }

    storage.pin_in(None, &roots[1], "protected").unwrap();
    let hold = storage.hold(&roots[2]).unwrap();
    // The first free-space reread is still short. The second reaches
    // reserve, so the next eligible root must remain untouched.
    *os.reads.lock().unwrap() = [RESERVE - 2, RESERVE - 1, RESERVE].into();
    let (warm, walks) =
        count_pool_walks(|| opened.lease(&request(&pool, &projects[3], &workspace, "below")));
    let warm = warm.unwrap();
    assert!(warm.grant.reused_slot);
    assert_eq!(
        walks,
        vec![
            PoolWalk::Evict(roots[4].clone()),
            PoolWalk::Evict(roots[5].clone())
        ]
    );
    assert!(os.reads.lock().unwrap().is_empty());
    opened.release_or_recover(&warm.grant.lease_id).unwrap();
    drop(warm);

    *os.reads.lock().unwrap() = [RESERVE - 1, RESERVE].into();
    let (fresh, walks) = count_pool_walks(|| {
        opened.lease(&request(
            &pool,
            &fixture.dir("new-below"),
            &workspace,
            "new-below",
        ))
    });
    assert!(!fresh.unwrap().grant.reused_slot);
    assert_eq!(walks, vec![PoolWalk::Evict(roots[6].clone())]);
    assert!(os.reads.lock().unwrap().is_empty());
    for index in [1, 2, 3, 7] {
        assert!(storage.resolve(&roots[index]).is_ok());
    }
    drop(hold);
}

/// A committed rename with a trash copy Barge could not delete is progress:
/// the row is gone, so the reserve loop re-reads free space and reports the
/// kept tree instead of failing the admission after draining the slot.
#[test]
fn a_committed_rename_with_kept_trash_is_progress_for_the_reserve_loop() {
    const UUID: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const RESERVE: u64 = 4 << 20;
    let fixture = Fixture::new("reserve-kept-trash");
    let pool = fixture.pool();
    let workspace = fixture.dir("workspace");
    let mut opened = Pool::open(&pool).unwrap();
    let os = Arc::new(ScriptedSpace {
        volume: barge::FakeOs::with_volume(pool.clone(), UUID, RESERVE),
        reads: Mutex::new(VecDeque::new()),
    });
    let account = Storage::open_with(
        fixture.dir("account"),
        os.clone(),
        Arc::new(barge::ManualClock::new(1_000)),
    )
    .unwrap();
    let storage = PoolStorage::from_storage(
        account,
        &PoolBacking::volume(&pool, UUID, "payload", 8 << 20, RESERVE, RESERVE / 2),
    )
    .unwrap();
    opened.storage = Some(storage);

    // The victim's tree is deeper than Barge's deletion walk, so its
    // committed rename leaves the trash copy in place.
    let victim_project = fixture.dir("victim");
    *os.reads.lock().unwrap() = [RESERVE + 1].into();
    let victim = opened
        .lease(&request(&pool, &victim_project, &workspace, "victim"))
        .unwrap();
    let victim_path = victim
        .grant
        .env
        .cargo_build_build_dir
        .parent()
        .unwrap()
        .to_path_buf();
    let mut deep = victim.grant.env.cargo_build_build_dir.clone();
    for _ in 0..140 {
        deep = deep.join("d");
        fs::create_dir(&deep).unwrap();
    }
    opened.release_or_recover(&victim.grant.lease_id).unwrap();
    drop(victim);

    // Short until the rename moves the tree out of the way: the first read
    // is the loop's entry, the second its re-read after the eviction.
    *os.reads.lock().unwrap() = [RESERVE - 1, RESERVE].into();
    let fresh_project = fixture.dir("fresh");
    let fresh = opened
        .lease(&request(&pool, &fresh_project, &workspace, "fresh"))
        .unwrap();
    let note = fresh.admission_note.clone().unwrap_or_default();
    assert!(
        note.contains("moved to trash and could not be deleted"),
        "{note}"
    );
    assert!(os.reads.lock().unwrap().is_empty());
    assert!(!victim_path.exists(), "the rename committed");
    assert!(
        !opened
            .ledger
            .slots
            .iter()
            .any(|slot| opened.slot_path(slot.id).unwrap() == victim_path),
        "the retired row is gone"
    );
    // Put the deep trash back in reach so the fixture can remove it.
    if let Ok(trash) = fs::read_dir(victim_path.parent().unwrap().join(".trash")) {
        for entry in trash.flatten() {
            let mut current = entry.path();
            for _ in 0..140 {
                current = current.join("d");
            }
            let _ = fs::set_permissions(current, fs::Permissions::from_mode(0o700));
        }
    }
}
