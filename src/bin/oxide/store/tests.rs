//! Unit tests for the store submodules.

use super::admission::*;
use super::barrier::*;
use super::control_io::*;
use super::recovery::*;
use super::seeding::*;
use super::status::*;
use super::*;

use crate::cli::Profile;
use crate::freshness::Built;
use crate::identity::SlotKey;
use crate::process;
use oxide_storage::lease::managed::{init_budget_pool, prepare_pool};
use oxide_storage::lease::{
    LeaseError, LedgerSlot, Profile as LeaseProfile, SlotOccupancy, UnmeasuredSlot, MANAGED_NAME,
};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The registered removable volume's reserve belongs to that volume's pool
/// only. The registration lives in the internal control directory, which is
/// the removable store's owner control: reading the removable pool's own
/// control found nothing, and an internal fallback admission carried the
/// removable reserve (oxide#337).
#[test]
fn the_removable_reserve_applies_only_to_the_removable_store() {
    const RESERVE: u64 = 40 * GIB;

    let (root, internal, _key) = lease_fixture("admission-reserve");
    let control = internal.control().to_path_buf();
    let registration = serde_json::json!({
        "version": 2,
        "mount_point": root.join("volume"),
        "volume_uuid": "volume",
        "container_uuid": "container",
        "reserve_bytes": RESERVE,
        "cap_bytes": 64 * GIB,
        "quota_bytes": null,
        "capacity_bytes": null,
        "registered_ms": 1,
        "mode": "permanent",
    });
    let path = control.join(crate::removable::REGISTRATION);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(&serde_json::to_vec(&registration).unwrap())
        .unwrap();
    drop(file);

    assert!(
        crate::removable::registration(&internal).unwrap().is_some(),
        "the registration is recorded beside the internal pool"
    );
    assert_eq!(
        internal.admission_reserve(),
        None,
        "an internal pool never carries the volume's reserve"
    );

    let removable_pool = root.join("removable-pool");
    let removable_control = removable_pool.join(MANAGED_NAME);
    fs::create_dir_all(&removable_control).unwrap();
    fs::set_permissions(&removable_control, fs::Permissions::from_mode(0o700)).unwrap();
    let removable = Store {
        control: removable_pool.join(MANAGED_NAME),
        pool: removable_pool.clone(),
        backend: Backend::ApfsVolume,
        payload: root.join("volume").join("oxide-pool"),
        budget: None,
        recorded_budget: 64 * GIB,
        max_active: 8,
        sibling: Some(Sibling {
            control: control.clone(),
            barrier: true,
        }),
    };
    assert!(
        crate::removable::registration(&removable)
            .unwrap()
            .is_none(),
        "the removable pool's own control holds no registration"
    );
    assert_eq!(
        removable.admission_reserve(),
        Some(RESERVE),
        "the removable pool reads the paired registration"
    );
}

/// A failed attempt ends the wait: its ticket is removed instead of being
/// left for every later command to queue behind (oxide#337).
#[test]
fn a_failed_attempt_leaves_no_dead_wait_ticket() {
    let (root, store, key) = lease_fixture("dead-ticket");
    // A removable store's barrier takes the paired internal admission
    // first; a paired control with no admission lock refuses the attempt
    // before any admission work runs.
    let store = store.with_sibling(&root.join("paired-control"), true);
    let mut waits = WaitAccounting::default();
    let refused = store.acquire(&key, Profile::Quick, false, None, &mut waits);
    let error = match refused {
        Err(error) => error,
        Ok(_) => panic!("the paired control has no admission lock to take"),
    };
    assert!(
        error.contains("paired-control"),
        "the failure is the paired barrier, after the ticket existed: {error}"
    );

    let state = store.load().unwrap();
    assert!(
        state.tickets.is_empty(),
        "the failed command's ticket outlived it: {:?}",
        state.tickets
    );
    let waiters = store.control.join("waiters");
    assert_eq!(
        fs::read_dir(&waiters).unwrap().count(),
        0,
        "a dead ticket lock file stayed in {}",
        waiters.display()
    );
}

/// Pool creation tolerates exactly one failure: another command created the
/// pool first. Any other failure is reported, even when a `config.json`
/// exists (oxide#337).
#[test]
fn pool_creation_does_not_swallow_an_unexpected_failure() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("oxide-create-pool-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let pool = root.join("pool");
    fs::create_dir_all(&pool).unwrap();
    let config = pool.join("config.json");
    fs::write(&config, b"not an oxide config").unwrap();

    let refused = create_pool(&pool, 8 * GIB);
    assert!(
        refused.is_err(),
        "an occupied pool directory is not a successful creation: {refused:?}"
    );
    assert!(
        refused.unwrap_err().contains("could not create the pool"),
        "the failure names the creation"
    );
    assert_eq!(fs::read(&config).unwrap(), b"not an oxide config");
    fs::remove_dir_all(&root).unwrap();
}

/// `main` asks once, after the command ran. Consuming the flag keeps one
/// writer-update refusal from poisoning every later command on this thread
/// (oxide#337).
#[test]
fn a_writer_update_refusal_is_reported_once() {
    assert!(!writer_updated(), "no refusal has been recorded yet");
    let message = lease_error("could not lease", &LeaseError::WriterUpdated);
    assert!(message.contains("could not lease"), "{message}");
    assert!(writer_updated(), "the refusal is reported to main");
    assert!(
        !writer_updated(),
        "the refusal must not persist into the next command"
    );
}

#[test]
fn release_admission_notice_and_budget_are_bounded_and_truthful() {
    let (root, _store, _key) = lease_fixture("release-admission-notice");
    let path = root.join("notice-lock");
    let holder = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let waiter = OpenOptions::new().write(true).open(&path).unwrap();
    flock_blocking(&holder).unwrap();
    let calls = std::cell::Cell::new(0);
    wait_release_admission(
        &waiter,
        Duration::ZERO,
        Duration::from_secs(120),
        || false,
        || false,
        || {
            calls.set(calls.get() + 1);
            // SAFETY: holder owns this fixture lock.
            assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_UN) }, 0);
        },
    )
    .unwrap();
    assert_eq!(calls.get(), 1);
    // SAFETY: waiter owns this fixture lock.
    assert_eq!(unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_UN) }, 0);
    wait_release_admission(
        &waiter,
        Duration::from_secs(5),
        Duration::from_secs(120),
        || false,
        || false,
        || panic!("uncontended notice"),
    )
    .unwrap();
    // SAFETY: waiter owns this fixture lock.
    assert_eq!(unsafe { libc::flock(waiter.as_raw_fd(), libc::LOCK_UN) }, 0);
    flock_blocking(&holder).unwrap();
    assert_eq!(
        wait_release_admission(
            &waiter,
            Duration::ZERO,
            Duration::from_secs(120),
            || true,
            || true,
            || panic!("expired notice")
        )
        .unwrap_err(),
        "the admission budget expired while waiting for the admission lock"
    );
    // An unbudgeted wait on a held lock ends at its bound, naming what it waited on.
    let started = Instant::now();
    let refusal = wait_release_admission(
        &waiter,
        Duration::ZERO,
        Duration::from_millis(60),
        || false,
        || false,
        || {},
    )
    .unwrap_err();
    assert!(
        refusal.starts_with("gave up waiting for the admission lock after "),
        "{refusal}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        idle_slot_evicted_message("3", "held when the removable volume was lost (Cargo group 4 empty)"),
        "oxide: evicted idle slot 3; the slot was held when the removable volume was lost (Cargo group 4 empty)"
    );
    assert_eq!(admission_wait_message(Some("17")), "oxide: waiting for the admission lock (slot 17 is recorded as being deleted; its current writer is unknown)");
    assert_eq!(
        admission_wait_message(None),
        "oxide: waiting for the admission lock (holder unknown)"
    );
    drop(holder);
    drop(waiter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn release_admission_wait_is_bounded_when_the_holder_never_lets_go() {
    let (root, _store, _key) = lease_fixture("release-admission-bound");
    let path = root.join("bound-lock");
    let holder = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let waiter = OpenOptions::new().write(true).open(&path).unwrap();
    flock_blocking(&holder).unwrap();
    let started = Instant::now();
    let refusal = wait_release_admission(
        &waiter,
        Duration::from_millis(10),
        Duration::from_millis(80),
        || false,
        || false,
        || {},
    )
    .unwrap_err();
    assert!(refusal.contains("the admission lock"), "{refusal}");
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(holder);
    drop(waiter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn wait_release_admission_caps_to_bound_when_budget_exceeds_bound() {
    let (root, _store, _key) = lease_fixture("release-admission-long-budget");
    let path = root.join("bound-lock");
    let holder = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    let waiter = OpenOptions::new().write(true).open(&path).unwrap();
    flock_blocking(&holder).unwrap();
    let started = Instant::now();
    // Simulate an active admission budget longer than the 120s lock bound:
    // `expired` stays false, yet the wait must stop at `bound`.
    let refusal = wait_release_admission(
        &waiter,
        Duration::ZERO,
        Duration::from_millis(60),
        || true,
        || false,
        || {},
    )
    .unwrap_err();
    assert!(
        refusal.starts_with("gave up waiting for the admission lock after "),
        "{refusal}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(holder);
    drop(waiter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn post_build_release_succeeds_under_contention_after_expired_admission_budget() {
    crate::budget::clear();
    crate::budget::install(crate::budget::Options {
        admission: Some(Duration::from_millis(1)),
        execution: None,
        events: None,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    assert!(crate::budget::admission_expired());
    crate::budget::arm_execution(Instant::now(), crate::budget::Phase::PrimaryCargo);

    let (root, store, key) = seed_fixture();
    let held = seed_fixture_lease(&store, &key);
    let lock_file = open_admission_file(store.control()).unwrap().unwrap();
    flock_blocking(&lock_file).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        drop(lock_file);
    });
    let disposition = store
        .release(held, None)
        .expect("post-build release must survive contention after arm_execution");
    assert_eq!(disposition, ReleaseDisposition::Released);
    releaser.join().unwrap();

    // Even before arm_execution (e.g. a pre-child abort right as the window
    // closes), an already-expired admission deadline does not block release().
    crate::budget::clear();
    crate::budget::install(crate::budget::Options {
        admission: Some(Duration::from_millis(1)),
        execution: None,
        events: None,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    assert!(crate::budget::admission_expired());
    let held = seed_fixture_lease(&store, &key);
    let lock_file = open_admission_file(store.control()).unwrap().unwrap();
    flock_blocking(&lock_file).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(40));
        drop(lock_file);
    });
    let disposition = store
        .release(held, None)
        .expect("release with an already-expired budget must still wait on the bound");
    assert_eq!(disposition, ReleaseDisposition::Released);
    releaser.join().unwrap();

    crate::budget::clear();
    fs::remove_dir_all(root).unwrap();
}

/// A ledger as an earlier Oxide wrote it reads back, keeps its stored
/// digests and serializes to the same bytes (oxide#103 sha2 0.11.0).
#[test]
fn frozen_ledger_reads_back_unchanged() {
    let frozen = concat!(
        r#"{"version":1,"next_id":2,"next_use":4,"tickets":[],"cleans":[],"records":[{"#,
        r#""id":1,"identity":"4344f9285b31bb4b9e695c75b8af04a5f4a97a99705393e8d0126c94832eec4f","#,
        r#""compatibility":"12d9fc45c5c84a2e41a4e96d4918b08d421b41fd92114cbecd1eecb4b0a16936","#,
        r#""profile":"quick","checkout":"/Users/Zoë/work space/oxide","phase":"idle","#,
        r#""process_group":null,"last_used":3,"holder":null,"lease_id":null,"#,
        r#""pending_recovery":null,"last_used_ms":1760000000000}],"recoveries":[]}"#
    );
    let state: State = serde_json::from_str(frozen).expect("frozen ledger");
    let record = &state.records[0];
    assert_eq!(
        record.identity,
        crate::identity::digest("git:https://github.com/inflatable-cookie/oxide:")
    );
    assert_eq!(
        record.compatibility,
        "12d9fc45c5c84a2e41a4e96d4918b08d421b41fd92114cbecd1eecb4b0a16936"
    );
    assert_eq!(serde_json::to_string(&state).expect("serialize"), frozen);
}

#[test]
fn wait_accounting_partitions_observed_intervals_and_pauses_between_grants() {
    let start = Instant::now();
    let mut waits = WaitAccounting::default();
    waits.observe(&Wait::Queue(1), 2, start);
    waits.observe(&Wait::Queue(2), 2, start + Duration::from_millis(200));
    waits.stop(start + Duration::from_millis(300));

    // Freshness work between lease attempts is outside measured waiting.
    waits.observe(&Wait::HostLimit(2), 2, start + Duration::from_secs(10));
    waits.stop(start + Duration::from_millis(11_100));
    assert_eq!(
        waits.admission_queue_wait(),
        Some(Duration::from_millis(1_400))
    );

    assert_eq!(
        waits.completion_line("slot ready").as_deref(),
        Some("oxide: waited 1.4s (queue 0.3s, host limit 1.1s); slot ready")
    );
    assert_eq!(waits.summary_suffix().as_deref(), Some(", waited 1.4s"));
}

#[test]
fn wait_accounting_reports_unavailable_after_a_backward_clock() {
    let start = Instant::now();
    let mut waits = WaitAccounting::default();
    waits.observe(&Wait::Queue(1), 2, start);
    waits.stop(start + Duration::from_secs(1));
    waits.observe(&Wait::Capacity(None), 2, start + Duration::from_secs(3));
    waits.stop(start + Duration::from_secs(2));

    assert_eq!(
        waits.completion_line("slot ready").as_deref(),
        Some("oxide: wait duration unavailable; slot ready")
    );
    assert_eq!(
        waits.summary_suffix().as_deref(),
        Some(", wait duration unavailable")
    );
}

#[test]
fn wait_accounting_closes_intervals_on_cancellation_and_error() {
    let start = Instant::now();
    for ending in ["cancelled", "refused"] {
        let mut waits = WaitAccounting::default();
        waits.observe(&Wait::Clean, 2, start);
        waits.stop(start + Duration::from_millis(250));
        let expected = format!("oxide: waited 0.3s (clean 0.3s); {ending}");
        assert_eq!(
            waits.completion_line(ending).as_deref(),
            Some(expected.as_str())
        );
    }
}

#[test]
fn wait_accounting_bounds_reason_announcements_and_omits_unobserved_waits() {
    let mut waits = WaitAccounting::default();
    assert_eq!(waits.summary_suffix(), None);
    let start = Instant::now();
    let reasons = [
        Wait::Queue(1),
        Wait::HostLimit(2),
        Wait::Checkout,
        Wait::Clean,
        Wait::ProjectCap,
        Wait::Uncertain,
        Wait::Capacity(None),
    ];
    for reason in reasons {
        waits.observe(&reason, 2, start);
    }
    waits.observe(&Wait::Queue(2), 2, start + Duration::from_millis(7));
    assert_eq!(waits.reason_lines, 8);
    waits.observe(&Wait::HostLimit(1), 2, start + Duration::from_millis(8));
    assert_eq!(waits.reason_lines, 8);
    assert!(waits.suppression_announced);
    waits.observe(&Wait::Checkout, 2, start + Duration::from_millis(9));
    assert_eq!(waits.reason_lines, 8);
    assert!(waits.suppression_announced);
}

#[test]
fn wait_age_prefers_creation_and_uses_modification_only_when_unavailable() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let created = now - Duration::from_secs(10);
    let modified = now - Duration::from_secs(20);
    let age = wait_age_from_times(Some(created), Some(modified), now).unwrap();
    assert_eq!(age.seconds, 10);
    assert!(matches!(age.source, WaitAgeSource::Created));

    let age = wait_age_from_times(None, Some(modified), now).unwrap();
    assert_eq!(age.seconds, 20);
    assert!(matches!(age.source, WaitAgeSource::Modified));
    assert!(wait_age_from_times(Some(now + Duration::from_secs(1)), Some(modified), now).is_none());
    assert!(wait_age_from_times(None, None, now).is_none());
}

#[test]
fn wait_ticket_age_requires_private_regular_nonsymlink_metadata() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let (root, _, _) = paired_controls("wait-ticket-age");
    let ticket = root.join("ticket.lock");
    fs::write(&ticket, b"").unwrap();
    fs::set_permissions(&ticket, fs::Permissions::from_mode(0o600)).unwrap();
    let age = ticket_age(&ticket, SystemTime::now()).unwrap();
    assert!(age.seconds <= 1);

    fs::set_permissions(&ticket, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ticket_age(&ticket, SystemTime::now()).is_none());
    fs::set_permissions(&ticket, fs::Permissions::from_mode(0o600)).unwrap();
    let link = root.join("ticket-link.lock");
    symlink(&ticket, &link).unwrap();
    assert!(ticket_age(&link, SystemTime::now()).is_none());
    assert!(ticket_age(&root.join("missing.lock"), SystemTime::now()).is_none());
    fs::remove_dir_all(root).unwrap();
}

fn paired_controls(label: &str) -> (PathBuf, Store, Store) {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-{label}-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let store = |name: &str| Store {
        pool: root.join(name),
        backend: Backend::DirectoryBudget,
        payload: root.join(name).join("payload"),
        control: root.join(name).join(MANAGED_NAME),
        budget: None,
        recorded_budget: 1,
        max_active: 2,
        sibling: None,
    };
    let (internal, removable) = (store("internal"), store("removable"));
    for side in [&internal, &removable] {
        fs::create_dir_all(&side.pool).unwrap();
        side.ensure_control().unwrap();
    }
    let internal_control = internal.control.clone();
    let removable_control = removable.control.clone();
    (
        root,
        internal.with_sibling(&removable_control, false),
        removable.with_sibling(&internal_control, true),
    )
}

/// Output receipts are keyed in the internal pool's control directory,
/// whichever pool a command's slots come from.
#[test]
fn owned_output_receipts_live_in_the_internal_control_directory() {
    let (root, internal, removable) = paired_controls("owner-control");
    assert_eq!(internal.owner_control(), internal.control());
    assert_eq!(removable.owner_control(), internal.control());
    fs::remove_dir_all(root).unwrap();
}

fn custody_record(id: u64, checkout: &Path, phase: Phase, lost: Option<Lost>) -> Record {
    Record {
        id,
        identity: "identity".to_owned(),
        compatibility: "compatibility".to_owned(),
        profile: "quick".to_owned(),
        checkout: Some(checkout.to_path_buf()),
        phase,
        process_group: None,
        last_used: 1,
        holder: None,
        lease_id: None,
        pending_recovery: None,
        retired: lost.as_ref().map(|lost| lost.describe(false)),
        lost,
        last_used_ms: Some(1),
    }
}

fn state_with(records: Vec<Record>) -> State {
    State {
        records,
        ..State::default()
    }
}

/// Only a possibly-writing command or an unresolved lost writer blocks a
/// relink or clean; a retired slot that closed positively does not, and a
/// caller never blocks on its own record.
#[test]
fn checkout_custody_separates_writers_unknown_and_retired_storage() {
    let (root, internal, removable) = paired_controls("custody");
    let checkout = Path::new("/checkout/custody");
    internal
        .save(&state_with(vec![custody_record(
            1,
            checkout,
            Phase::Running,
            None,
        )]))
        .unwrap();
    assert_eq!(
        internal.checkout_custody(checkout, 0).unwrap(),
        CheckoutCustody::Writers
    );
    assert_eq!(
        internal.checkout_custody(checkout, 1).unwrap(),
        CheckoutCustody::Clear
    );

    let unknown = Lost {
        slot_id: "3".to_owned(),
        at_ms: 2,
        unclosed: Some("exec may have started processes outside its group".to_owned()),
    };
    removable
        .save(&state_with(vec![custody_record(
            2,
            checkout,
            Phase::Retired,
            Some(unknown),
        )]))
        .unwrap();
    internal.save(&State::default()).unwrap();
    match internal.checkout_custody(checkout, 0).unwrap() {
        CheckoutCustody::Unknown(reason) => {
            assert!(reason.contains("never confirmed closed"), "{reason}")
        }
        other => panic!("expected unknown custody, got {other:?}"),
    }

    // A retired slot whose writers closed positively is not unknown.
    removable
        .save(&state_with(vec![custody_record(
            3,
            checkout,
            Phase::Retired,
            Some(Lost {
                slot_id: "4".to_owned(),
                at_ms: 3,
                unclosed: None,
            }),
        )]))
        .unwrap();
    assert_eq!(
        internal.checkout_custody(checkout, 0).unwrap(),
        CheckoutCustody::Clear
    );
    fs::remove_dir_all(root).unwrap();
}

/// Record IDs are per pool: the caller's own ID exempts only its record in
/// its own pool, never an equal ID in the other pool, in either direction
/// and for running and quarantined records alike.
#[test]
fn checkout_custody_exempts_own_id_only_in_its_own_pool() {
    let (root, internal, removable) = paired_controls("custody-ids");
    let checkout = Path::new("/checkout/custody-ids");
    let id_seven = |phase| state_with(vec![custody_record(7, checkout, phase, None)]);

    internal.save(&id_seven(Phase::Acquiring)).unwrap();
    removable.save(&id_seven(Phase::Running)).unwrap();
    assert_eq!(
        internal.checkout_custody(checkout, 7).unwrap(),
        CheckoutCustody::Writers
    );
    assert_eq!(
        removable.checkout_custody(checkout, 7).unwrap(),
        CheckoutCustody::Clear
    );

    removable.save(&id_seven(Phase::Acquiring)).unwrap();
    internal.save(&id_seven(Phase::Spawning)).unwrap();
    assert_eq!(
        removable.checkout_custody(checkout, 7).unwrap(),
        CheckoutCustody::Writers
    );

    let quarantined = Lost {
        slot_id: "9".to_owned(),
        at_ms: 4,
        unclosed: Some("an allowed compiler wrapper may still run".to_owned()),
    };
    internal
        .save(&state_with(vec![custody_record(
            7,
            checkout,
            Phase::Retired,
            Some(quarantined),
        )]))
        .unwrap();
    match removable.checkout_custody(checkout, 7).unwrap() {
        CheckoutCustody::Unknown(reason) => {
            assert!(reason.contains("the other pool"), "{reason}")
        }
        other => panic!("expected unknown custody, got {other:?}"),
    }
    fs::remove_dir_all(root).unwrap();
}

/// The fence is written to both pools' state (created when absent), a
/// state parser without the field (as every pre-feature Oxide has)
/// refuses it, the current one keeps loading it, and an unknown fence
/// value refuses.
#[test]
fn the_owned_outputs_fence_refuses_a_pre_feature_parser() {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct PreFeatureState {
        version: u32,
        next_id: u64,
        next_use: u64,
        tickets: Vec<serde_json::Value>,
        #[serde(default)]
        cleans: Vec<serde_json::Value>,
        records: Vec<serde_json::Value>,
        #[serde(default)]
        recoveries: Vec<serde_json::Value>,
        #[serde(default)]
        flushes: Vec<serde_json::Value>,
    }
    let (root, internal, removable) = paired_controls("fence");
    let before = fs::read(internal.control.join("state.json")).ok();
    if let Some(bytes) = &before {
        assert!(serde_json::from_slice::<PreFeatureState>(bytes).is_ok());
    }
    let _ = fs::remove_file(removable.control.join("state.json"));
    internal.fence_owned_outputs().unwrap();
    internal.fence_owned_outputs().unwrap();
    for control in [&internal.control, &removable.control] {
        let text = fs::read_to_string(control.join("state.json")).unwrap();
        assert!(serde_json::from_str::<PreFeatureState>(&text).is_err());
        // Isolate the owned-outputs check from the earlier writer field.
        let writer = format!("\"minimum_writer_generation\":{WRITER_GENERATION},");
        let owned_only = text.replacen(&writer, "", 1);
        let refused = serde_json::from_str::<PreFeatureState>(&owned_only)
            .err()
            .expect("a pre-feature parser accepted the fenced state");
        assert!(refused.to_string().contains("owned_outputs"), "{refused}");
    }
    assert_eq!(
        internal.load().unwrap().owned_outputs.as_deref(),
        Some(OWNED_OUTPUTS_FENCE)
    );
    let mut state = internal.load().unwrap();
    state.owned_outputs = Some("oxide.outputs/9".to_owned());
    internal.save(&state).unwrap();
    assert!(internal.load().is_err());
    fs::remove_dir_all(root).unwrap();
}

/// The numeric state version advances only with the owned-outputs fence
/// or its initialization record (oxide#129). An ordinary mutation must
/// not refuse older binaries that have not opted into owned outputs.
#[test]
fn the_numeric_state_version_advances_only_with_the_output_fence() {
    let (root, internal, _removable) = paired_controls("version-gate");
    let state = internal.load().unwrap();
    internal.save(&state).unwrap();
    let doc: serde_json::Value =
        serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap()).unwrap();
    assert_eq!(
        doc["version"].as_u64(),
        Some(u64::from(LEGACY_FIELDS_STATE_VERSION)),
        "an ordinary unfenced save advanced the numeric state version"
    );
    // Neither released fence is consent to a successor numeric format.
    let released = State {
        owned_outputs: Some(RELEASED_OWNED_OUTPUTS_FENCE.to_owned()),
        ..State::default()
    };
    internal.save(&released).unwrap();
    let doc: serde_json::Value =
        serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap()).unwrap();
    assert_eq!(
        doc["version"].as_u64(),
        Some(u64::from(LEGACY_FIELDS_STATE_VERSION)),
        "a released /1 fence advanced the numeric state version"
    );
    // The published whole-target fence keeps version 3, so a state-3
    // binary still runs a pool it understands.
    let published = State {
        owned_outputs: Some(LEGACY_OWNED_OUTPUTS_FENCE.to_owned()),
        outputs_init: Some(OutputsInit::Ready {
            transaction: "b".repeat(32),
            ready_ms: 1,
        }),
        ..State::default()
    };
    internal.save(&published).unwrap();
    let doc: serde_json::Value =
        serde_json::from_slice(&fs::read(internal.control.join("state.json")).unwrap()).unwrap();
    assert_eq!(
        doc["version"].as_u64(),
        Some(u64::from(OLDER_STATE_VERSION)),
        "the published /2 fence left its numeric version"
    );
    // A pool that has not opted into owned outputs initializes from a
    // fresh state below.
    internal.save(&State::default()).unwrap();
    let transaction = "a".repeat(32);
    let created = std::cell::Cell::new(0);
    internal
        .initialize_outputs(&transaction, None, &|_, may_create| {
            assert!(may_create);
            created.set(created.get() + 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(created.get(), 1);
    let state = internal.load().unwrap();
    assert_eq!(state.version, WRITER_STATE_VERSION);
    assert_eq!(state.minimum_writer_generation, Some(WRITER_GENERATION));
    assert!(matches!(
        state.outputs_init,
        Some(OutputsInit::Ready { .. })
    ));
    assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
    // A ready pool validates the existing registry without creating it.
    internal
        .initialize_outputs(&transaction, None, &|_, may_create| {
            assert!(!may_create);
            Ok(())
        })
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Owned-outputs initialization requires positive writer closure in both
/// pools under both admission locks: an active record (an older binary's
/// in-flight command included), a lost slot with unclosed writers or
/// another checkout's clean barrier refuses with nothing written, while
/// the initializing checkout's own barrier is allowed.
#[test]
fn output_initialization_requires_closed_writers_in_both_pools() {
    let (root, internal, removable) = paired_controls("init-closure");
    let own = Path::new("/own");
    let transaction = "b".repeat(32);
    let refuses = |internal: &Store, label: &str| {
        let created = std::cell::Cell::new(false);
        let error = internal
            .initialize_outputs(&transaction, Some(own), &|_, _| {
                created.set(true);
                Ok(())
            })
            .unwrap_err();
        assert!(!created.get(), "{label}: the registry was created");
        assert!(internal.load().unwrap().outputs_init.is_none(), "{label}");
        assert!(internal.load().unwrap().owned_outputs.is_none(), "{label}");
        assert!(removable.load().unwrap().outputs_init.is_none(), "{label}");
        error
    };
    let mut active = State::default();
    active
        .records
        .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
    removable.save(&active).unwrap();
    assert!(refuses(&internal, "active removable record").contains("active command record"));
    removable.save(&State::default()).unwrap();
    let mut lost = State::default();
    let mut retired = record(Phase::Retired, None, None);
    // A holder of this boot session: the restart fact never closes it.
    retired.holder.as_mut().unwrap().started_ms = process::now_ms();
    retired.lost = Some(Lost {
        slot_id: "1".to_owned(),
        at_ms: 1,
        unclosed: Some("never confirmed".to_owned()),
    });
    lost.records.push(retired);
    internal.save(&lost).unwrap();
    assert!(refuses(&internal, "unclosed lost slot").contains("never confirmed closed"));
    let mut other_clean = State::default();
    other_clean.cleans.push(Clean {
        id: 1,
        checkout: PathBuf::from("/other"),
        process_group: None,
    });
    internal.save(&other_clean).unwrap();
    assert!(refuses(&internal, "other clean barrier").contains("/other"));
    let mut own_clean = State::default();
    own_clean.cleans.push(Clean {
        id: 1,
        checkout: own.to_path_buf(),
        process_group: None,
    });
    internal.save(&own_clean).unwrap();
    removable.save(&own_clean).unwrap();
    internal
        .initialize_outputs(&transaction, Some(own), &|_, may_create| {
            assert!(may_create);
            Ok(())
        })
        .unwrap();
    for side in [&internal, &removable] {
        let state = side.load().unwrap();
        assert!(matches!(
            state.outputs_init,
            Some(OutputsInit::Ready { .. })
        ));
        assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
    }
    fs::remove_dir_all(root).unwrap();
}

/// Owned output format initialization accepts a quarantined lost slot in
/// either pool when a restart ended its writers, and still refuses the
/// same record when its holder started in this boot session.
#[cfg(target_os = "macos")]
#[test]
fn output_initialization_accepts_a_lost_slot_from_an_earlier_boot() {
    let own = Path::new("/own");
    let transaction = "e".repeat(32);
    let quarantined = |started_ms: u64| {
        let mut entry = record(Phase::Retired, None, None);
        entry.holder.as_mut().unwrap().started_ms = started_ms;
        entry.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
        entry.lost = Some(Lost {
            slot_id: "4".to_owned(),
            at_ms: 1,
            unclosed: Some("exec tool may have started processes outside its group".to_owned()),
        });
        entry
    };
    for label in ["internal", "paired"] {
        let (root, internal, removable) = paired_controls(&format!("init-boot-{label}"));
        let (side, other) = if label == "internal" {
            (&internal, &removable)
        } else {
            (&removable, &internal)
        };
        // This boot session's holder: unchanged refusal, in either pool.
        other.save(&State::default()).unwrap();
        side.save(&state_with(vec![quarantined(process::now_ms())]))
            .unwrap();
        let created = std::cell::Cell::new(false);
        let error = internal
            .initialize_outputs(&transaction, Some(own), &|_, _| {
                created.set(true);
                Ok(())
            })
            .unwrap_err();
        assert!(error.contains("never confirmed closed"), "{label}: {error}");
        assert!(!created.get(), "{label}: the registry was created");
        // A holder from an earlier boot session: the restart closed it,
        // so initialization proceeds and reads both pools the same way.
        side.save(&state_with(vec![quarantined(1)])).unwrap();
        internal
            .initialize_outputs(&transaction, Some(own), &|_, may_create| {
                assert!(may_create);
                Ok(())
            })
            .unwrap();
        for pool in [&internal, &removable] {
            let state = pool.load().unwrap();
            assert!(matches!(
                state.outputs_init,
                Some(OutputsInit::Ready { .. })
            ));
            assert_eq!(state.owned_outputs.as_deref(), Some(OWNED_OUTPUTS_FENCE));
        }
        // The record is still present until the next admission deletes
        // it whole; initialization never deletes a slot itself.
        assert_eq!(side.load().unwrap().records.len(), 1, "{label}");
        fs::remove_dir_all(root).unwrap();
    }
}

/// A quarantined lost slot whose holder started before this boot session
/// is released and deleted whole through the existing path. A group
/// number that is live now does not keep it: it belongs to no writer of an
/// earlier session. The same record with a current-session holder is kept
/// (the counterfactual).
#[cfg(target_os = "macos")]
#[test]
fn a_lost_slot_from_an_earlier_boot_is_released_and_deleted_whole() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-lost-boot-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(&root).unwrap();
    let pool = root.join("pool");
    init_budget_pool(&pool, 1 << 30).unwrap();
    let store = Store {
        control: pool.join(MANAGED_NAME),
        payload: pool.join("payload"),
        pool: pool.clone(),
        backend: Backend::DirectoryBudget,
        budget: Some(1 << 30),
        recorded_budget: 1 << 30,
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
    let leased = lease(&store, &key, None).expect("a slot");
    let record_id = leased.record_id();
    let slot_id = leased.slot_id.clone();
    let slot_dir = leased.build_dir.parent().expect("slot dir").to_path_buf();
    drop(leased);
    // SAFETY: getpgrp has no preconditions and this test process is in
    // its group, so the recorded group is genuinely not empty.
    let live_group = unsafe { libc::getpgrp() };
    let quarantine = |started_ms: u64| {
        let mut state = store.load().unwrap();
        let entry = state
            .records
            .iter_mut()
            .find(|entry| entry.id == record_id)
            .unwrap();
        entry.phase = Phase::Retired;
        entry.process_group = Some(live_group);
        entry.holder.as_mut().unwrap().started_ms = started_ms;
        entry.lost = Some(Lost {
            slot_id: slot_id.clone(),
            at_ms: 1,
            unclosed: Some("exec tool may have started processes outside its group".to_owned()),
        });
        store.save(&state).unwrap();
    };
    // Counterfactual: this session's holder is never closed by the boot
    // fact, whatever its recorded group is.
    quarantine(process::now_ms());
    let mut state = store.load().unwrap();
    store.discard_lost(&mut state).unwrap();
    assert!(slot_dir.is_dir(), "a current-session lost slot was deleted");
    assert_eq!(
        store.load().unwrap().records.len(),
        1,
        "a current-session lost record was forgotten"
    );
    // A holder from an earlier boot session: deleted whole, with its
    // record, despite the live group number.
    quarantine(1);
    let mut state = store.load().unwrap();
    store.discard_lost(&mut state).unwrap();
    // The decision is committed under the pool lock; Barge's eviction and
    // deletion are the settle's work, with that lock released.
    oxide_storage::lease::managed::settle_pending_evictions(&store.pool).unwrap();
    assert!(!slot_dir.exists(), "the boot-closed slot was not deleted");
    assert!(
        store.load().unwrap().records.is_empty(),
        "the boot-closed record was not forgotten"
    );
    assert!(
        store.ledger().unwrap().slots.is_empty(),
        "the boot-closed slot is still in the ledger"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A paired removable control fenced by the released `oxide.outputs/1`
/// binary or by the published `/2` fence carries no registry transaction
/// on a host that last ran the released binary. Both predecessors migrate
/// under the internal control's exact recorded transaction; an unknown
/// fence and an open writer still refuse with both controls unchanged
/// (oxide#159).
#[test]
fn a_paired_pool_fenced_by_a_predecessor_migrates_under_the_recorded_transaction() {
    let transaction = "c".repeat(32);
    // One definition decides the accepted predecessor set, so no branch
    // can narrow it back to `/2`-only.
    assert!(predecessor_owned_outputs_fence(LEGACY_OWNED_OUTPUTS_FENCE));
    assert!(predecessor_owned_outputs_fence(
        RELEASED_OWNED_OUTPUTS_FENCE
    ));
    assert!(!predecessor_owned_outputs_fence(OWNED_OUTPUTS_FENCE));
    assert!(!predecessor_owned_outputs_fence("oxide.outputs/9"));
    // Counterfactual: this is the paired `None` arm's condition before
    // oxide#159, which accepted only an absent or `/2` fence. It refuses
    // exactly the `/1` input migrated below, so a revert fails there too.
    let refused_before =
        |fence: Option<&str>| fence.is_some_and(|fence| fence != LEGACY_OWNED_OUTPUTS_FENCE);
    assert!(refused_before(Some(RELEASED_OWNED_OUTPUTS_FENCE)));
    assert!(!refused_before(Some(LEGACY_OWNED_OUTPUTS_FENCE)));
    assert!(!refused_before(None));

    // The released host state records both fences at numeric version 5;
    // the published `/2` predecessor keeps version 3.
    for (label, fence, recorded_accounting) in [
        (
            "paired-released",
            RELEASED_OWNED_OUTPUTS_FENCE,
            Some(RECORDED_ACCOUNTING_FENCE),
        ),
        ("paired-published", LEGACY_OWNED_OUTPUTS_FENCE, None),
    ] {
        let (root, internal, removable) = paired_controls(label);
        internal
            .save(&State {
                owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
                outputs_init: Some(OutputsInit::Ready {
                    transaction: transaction.clone(),
                    ready_ms: 1,
                }),
                recorded_accounting: recorded_accounting.map(str::to_owned),
                ..State::default()
            })
            .unwrap();
        removable
            .save(&State {
                owned_outputs: Some(fence.to_owned()),
                recorded_accounting: recorded_accounting.map(str::to_owned),
                ..State::default()
            })
            .unwrap();
        let before = removable.load().unwrap();
        assert_eq!(before.owned_outputs.as_deref(), Some(fence), "{label}");
        assert!(before.outputs_init.is_none(), "{label}");
        assert_eq!(
            before.version,
            if recorded_accounting.is_some() {
                RECORDED_ACCOUNTING_STATE_VERSION
            } else {
                OLDER_STATE_VERSION
            },
            "{label}: unexpected fixture state version"
        );

        let created = std::cell::Cell::new(0);
        let outcome = internal
            .initialize_outputs(&transaction, None, &|recorded, may_create| {
                assert!(!may_create, "{label}: a ready control created a registry");
                assert_eq!(recorded, transaction, "{label}");
                created.set(created.get() + 1);
                Ok(())
            })
            .unwrap();
        assert!(outcome, "{label}");
        assert_eq!(created.get(), 1, "{label}");

        let internal_state = internal.load().unwrap();
        assert_eq!(
            internal_state.owned_outputs.as_deref(),
            Some(OWNED_OUTPUTS_FENCE),
            "{label}"
        );
        let paired_state = removable.load().unwrap();
        assert_eq!(
            paired_state.owned_outputs.as_deref(),
            Some(OWNED_OUTPUTS_FENCE),
            "{label}: the paired fence was not upgraded"
        );
        for (side, state) in [("internal", internal_state), ("paired", paired_state)] {
            match &state.outputs_init {
                Some(OutputsInit::Ready {
                    transaction: recorded,
                    ..
                }) => assert_eq!(
                    recorded, &transaction,
                    "{label}: the {side} control lost the recorded transaction"
                ),
                other => panic!("{label}: the {side} control is not ready: {other:?}"),
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}

/// An unknown paired fence refuses at the shared state parser before any
/// branch acts, and a released `/1` fence with an open writer still
/// refuses through writer closure: neither case changes either control
/// (oxide#159).
#[test]
fn an_unknown_paired_fence_and_an_open_writer_still_refuse() {
    let transaction = "d".repeat(32);
    // Both fixtures model older writable controls. A refused migration
    // leaves both checks and both documents unchanged.
    let ready_internal = |internal: &Store| {
        internal
            .save(&State {
                owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
                outputs_init: Some(OutputsInit::Ready {
                    transaction: transaction.clone(),
                    ready_ms: 1,
                }),
                recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
                ..State::default()
            })
            .unwrap();
    };
    let snapshot = |side: &Store| fs::read(side.control.join("state.json")).unwrap();
    let refuses = |internal: &Store, removable: &Store, label: &str| {
        let (internal_before, paired_before) = (snapshot(internal), snapshot(removable));
        let error = internal
            .initialize_outputs(&transaction, None, &|_, _| -> Result<(), String> {
                panic!("{label}: a refusing control created a registry")
            })
            .unwrap_err();
        assert_eq!(snapshot(internal), internal_before, "{label}");
        assert_eq!(snapshot(removable), paired_before, "{label}");
        error
    };

    let (root, internal, removable) = paired_controls("paired-unknown-fence");
    ready_internal(&internal);
    // `Store::load` is the single validation gate for every fence value
    // outside the accepted set, so this refuses before any branch can
    // read it as an empty record.
    removable
        .save(&State {
            owned_outputs: Some("oxide.outputs/9".to_owned()),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        })
        .unwrap();
    let error = refuses(&internal, &removable, "unknown paired fence");
    assert!(
        error.contains("invalid owned_outputs fence"),
        "unknown paired fence: {error}"
    );
    fs::remove_dir_all(root).unwrap();

    let (root, internal, removable) = paired_controls("paired-released-writer");
    ready_internal(&internal);
    let mut active = State {
        owned_outputs: Some(RELEASED_OWNED_OUTPUTS_FENCE.to_owned()),
        recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
        ..State::default()
    };
    active
        .records
        .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
    removable.save(&active).unwrap();
    let error = refuses(&internal, &removable, "released fence with an open writer");
    assert!(
        error.contains("the paired pool holds an active command record"),
        "released fence with an open writer: {error}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// The crash window of the paired fence upgrade: the internal control
/// already carries the successor fence while the paired control is still
/// `Ready` for the same transaction under the predecessor fence. The
/// upgrade must refuse while a paired command still holds a record.
#[test]
fn the_paired_ready_upgrade_refuses_an_open_writer() {
    let transaction = "e".repeat(32);
    let (root, internal, removable) = paired_controls("paired-ready-upgrade");
    internal
        .save(&State {
            owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
            outputs_init: Some(OutputsInit::Ready {
                transaction: transaction.clone(),
                ready_ms: 1,
            }),
            recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
            ..State::default()
        })
        .unwrap();
    let mut paired = State {
        owned_outputs: Some(LEGACY_OWNED_OUTPUTS_FENCE.to_owned()),
        outputs_init: Some(OutputsInit::Ready {
            transaction: transaction.clone(),
            ready_ms: 1,
        }),
        recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
        ..State::default()
    };
    paired
        .records
        .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
    removable.save(&paired).unwrap();
    let paired_before = fs::read(removable.control.join("state.json")).unwrap();
    let error = internal
        .initialize_outputs(&transaction, None, &|_, _| -> Result<(), String> {
            panic!("a refusing control created a registry")
        })
        .unwrap_err();
    assert!(
        error.contains("the paired pool holds an active command record"),
        "{error}"
    );
    assert_eq!(
        fs::read(removable.control.join("state.json")).unwrap(),
        paired_before,
        "the refused upgrade left the paired control unchanged"
    );
    fs::remove_dir_all(root).unwrap();
}

/// An unreadable sibling control directory is unknown custody, never
/// clear: the document looks absent through it, so the verdict must
/// refuse instead of reading that as no records.
#[test]
fn checkout_custody_refuses_an_unreadable_sibling_state() {
    // SAFETY: geteuid has no preconditions; root reads through mode 000.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let (root, internal, removable) = paired_controls("custody-sibling-unreadable");
    removable.save(&State::default()).unwrap();
    fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o000)).unwrap();
    let custody = internal.checkout_custody(Path::new("/checkout"), 0);
    fs::set_permissions(&removable.control, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        matches!(custody, Ok(CheckoutCustody::Unknown(_))),
        "{custody:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A sibling document the shared parser refuses is unknown custody even
/// when plain serde would accept it: the fence, the writer and every
/// reader must agree on what a valid control document is.
#[test]
fn checkout_custody_refuses_a_sibling_state_the_parser_rejects() {
    let (root, internal, removable) = paired_controls("custody-sibling-invalid");
    let invalid = State {
        owned_outputs: Some("oxide.outputs/9".to_owned()),
        ..State::default()
    };
    write_private(
        &removable.control.join("state.json"),
        &serde_json::to_vec(&invalid).unwrap(),
    );
    let custody = internal
        .checkout_custody(Path::new("/checkout"), 0)
        .unwrap();
    assert!(
        matches!(custody, CheckoutCustody::Unknown(_)),
        "{custody:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// The bounded reader refuses a file over its limit rather than returning
/// a truncated prefix, and an absent file is `None`, not an error.
#[test]
fn bounded_read_enforces_its_limit_and_reports_absence() {
    let (root, internal, _removable) = paired_controls("bounded-read");
    let path = internal.control.join("bounded.json");
    assert_eq!(bounded_read(&path, 4).unwrap(), None);
    write_private(&path, b"0123456789");
    let error = bounded_read(&path, 4).unwrap_err();
    assert!(error.contains("byte limit"), "{error}");
    assert_eq!(
        bounded_read(&path, 10).unwrap(),
        Some(b"0123456789".to_vec())
    );
    fs::remove_dir_all(root).unwrap();
}

/// A holder of a checkout's clean barrier excludes only barriers when it
/// rechecks writers: command records are still reported. The prune
/// custody routes take the records-only shape through the fresh witness's
/// `cleans` split; this pins the split at the shared formatter.
#[test]
fn active_records_omit_barriers_but_keep_commands() {
    let (root, internal, _removable) = paired_controls("active-records");
    let checkout = Path::new("/checkout");
    let mut state = State::default();
    state.cleans.push(Clean {
        id: 1,
        checkout: checkout.to_path_buf(),
        process_group: None,
    });
    internal.save(&state).unwrap();
    assert_eq!(
        active_holders(&internal.control, checkout).unwrap(),
        vec!["oxide clean barrier".to_owned()]
    );
    assert!(holders(&internal.control, checkout, false)
        .unwrap()
        .is_empty());
    let mut running = record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1"));
    running.checkout = Some(checkout.to_path_buf());
    state.records.push(running);
    internal.save(&state).unwrap();
    assert_eq!(
        holders(&internal.control, checkout, false).unwrap().len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

/// The `lock_admission` messages: an absent control directory names
/// itself, and an unusable lock path reports the precise error rather than
/// the absence of a lock.
#[test]
fn the_admission_messages_are_unchanged() {
    let (root, internal, _removable) = paired_controls("admission-text");
    let absent = Path::new("/nonexistent-oxide-control");
    let refused = lock_admission(absent).unwrap_err();
    assert_eq!(refused, "/nonexistent-oxide-control does not exist");
    let admission = internal.control.join("admission.lock");
    let elsewhere = root.join("elsewhere.lock");
    fs::write(&elsewhere, b"").unwrap();
    std::os::unix::fs::symlink(&elsewhere, &admission).unwrap();
    let refused = lock_admission(&internal.control).unwrap_err();
    assert!(
        refused.starts_with(&format!(
            "cannot open admission lock in {}: ",
            internal.control.display()
        )),
        "{refused}"
    );
    assert!(!refused.contains("does not succeed"), "{refused}");
    fs::remove_file(&admission).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// The checkout clean barrier spans both pools: an internal clean waits
/// for the checkout's active removable command, and a removable slot
/// attempt sees a live internal clean under the internal admission lock.
#[test]
fn clean_barrier_spans_both_pools() {
    let (root, internal, removable) = paired_controls("clean-barrier");
    let checkout = Path::new("/checkout");
    let other = Path::new("/other");
    let mut state = State::default();
    state
        .records
        .push(record(Phase::Running, Some(i32::MAX), Some("v1.a.1.1.1")));
    removable.save(&state).unwrap();
    assert!(internal.sibling_busy(checkout).unwrap());
    assert!(!internal.sibling_busy(other).unwrap());
    let Cleaning::Guard(guard) = internal
        .begin_clean(other, &mut WaitAccounting::default())
        .unwrap()
    else {
        panic!("clean of an idle checkout was interrupted");
    };
    let (lock, cleaning) = removable.sibling_barrier(other).unwrap();
    assert!(lock.is_some() && cleaning);
    drop(lock);
    assert!(!removable.sibling_barrier(checkout).unwrap().1);
    internal.abandon_clean(guard).unwrap();
    assert!(!removable.sibling_barrier(other).unwrap().1);
    // The internal store itself never takes the other pool's lock.
    assert!(internal.sibling_barrier(other).unwrap().0.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn host_limit_counts_internal_and_removable_activity_in_both_routes() {
    let (root, internal, removable) = paired_controls("host-limit");
    let key = SlotKey {
        checkout: PathBuf::from("/checkout"),
        identity: "identity".to_owned(),
        compatibility: "quick-key".to_owned(),
    };
    // SAFETY: getpgrp has no preconditions.
    let group = unsafe { libc::getpgrp() };
    let mut active = State::default();
    active.records = vec![
        record(Phase::Running, Some(group), Some("v1.a.1.1.1")),
        record(Phase::Running, Some(group), Some("v1.a.2.1.2")),
    ];

    // A removable admission sees two active internal records.
    internal.save(&active).unwrap();
    let (ticket, _file) = removable.register_ticket().unwrap();
    assert!(matches!(
        removable.attempt(ticket, &key, Profile::Quick, 2, false, None),
        Ok(Err(Wait::HostLimit(2)))
    ));
    removable.remove_ticket(ticket).unwrap();

    // An internal admission sees two active removable records.
    internal.save(&State::default()).unwrap();
    removable.save(&active).unwrap();
    let (ticket, _file) = internal.register_ticket().unwrap();
    assert!(matches!(
        internal.attempt(ticket, &key, Profile::Quick, 2, false, None),
        Ok(Err(Wait::HostLimit(2)))
    ));
    internal.remove_ticket(ticket).unwrap();
    fs::remove_dir_all(root).unwrap();
}

pub(super) fn lease(store: &Store, key: &SlotKey, label: Option<&str>) -> Result<Lease, Wait> {
    // Another test's `Command` spawn briefly duplicates every open
    // descriptor in the forked child before `exec` closes the
    // close-on-exec ones, so this fixture's admission lock can read as
    // busy for a few milliseconds. Only that transient uncertain outcome
    // is retried; every other wait is returned as before.
    for _ in 0..200 {
        let (ticket, _file) = store.register_ticket().unwrap();
        let outcome = store
            .attempt(ticket, key, Profile::Quick, 16, false, label)
            .unwrap();
        store.remove_ticket(ticket).unwrap();
        match outcome {
            Err(Wait::Uncertain) => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
    Err(Wait::Uncertain)
}

pub(crate) fn seed_fixture_lease(store: &Store, key: &SlotKey) -> Lease {
    lease(store, key, None).expect("fixture lease")
}

pub(crate) fn seed_fixture() -> (PathBuf, Store, SlotKey) {
    use std::ffi::{CStr, CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    let template = std::env::temp_dir().join("oxide-seed-front.XXXXXX");
    let mut name = CString::new(template.as_os_str().as_bytes())
        .unwrap()
        .into_bytes_with_nul();
    // SAFETY: name is writable, NUL-terminated and ends in XXXXXX.
    assert!(!unsafe { libc::mkdtemp(name.as_mut_ptr().cast()) }.is_null());
    let name = CStr::from_bytes_until_nul(&name).unwrap();
    let root = fs::canonicalize(PathBuf::from(OsStr::from_bytes(name.to_bytes()))).unwrap();
    let pool = root.join("pool");
    init_budget_pool(&pool, 1 << 30).unwrap();
    let store = Store {
        control: pool.join(MANAGED_NAME),
        payload: pool.join("payload"),
        pool: pool.clone(),
        backend: Backend::DirectoryBudget,
        budget: Some(1 << 30),
        recorded_budget: 1 << 30,
        max_active: 16,
        sibling: None,
    };
    store.ensure_control().unwrap();
    let checkout = root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    let key = SlotKey {
        checkout,
        identity: "identity".into(),
        compatibility: "quick-key".into(),
    };
    (root, store, key)
}

fn seed_test_producer() -> oxide_storage::lease::managed::SeedProducer {
    oxide_storage::lease::managed::SeedProducer {
        cargo_version: "cargo".into(),
        rustc_digest: "digest".into(),
        rustc: "rustc".into(),
        target: "target".into(),
        recipe: "quick".into(),
        backend: "backend".into(),
        adapter: "adapter".into(),
        context_digest: "context".into(),
        config_files: vec![],
        flags_digest: "flags".into(),
        cargo_home: "/cargo".into(),
        command_family: "check".into(),
        command_class: "check".into(),
    }
}

#[test]
fn seed_with_no_catalog_does_not_verify_sources_or_write_sidecars() {
    let (root, store, key) = seed_fixture();
    let held = lease(&store, &key, None).expect("fresh lease");
    let lock = format!("version = 4\n[[package]]\nname = 'leaf'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
    let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
    let producer = seed_test_producer();
    let before = fs::read(store.control.join("state.json")).unwrap();
    let result = store.seed(&held, &producer, &lock, |_| {
        panic!("source verification ran")
    });
    assert!(matches!(result, Ok(SeedRun::Skipped("no eligible donor"))));
    assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
    assert!(!store.control.join("seeds").exists());
    store.release(held, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_refused_catalog_after_a_successful_build_keeps_the_built_origin() {
    // A catalog the library refuses (here: the producing record is no
    // longer ours) must leave the successful generation's trusted origin
    // and slot intact; the caller ignores the refusal (§4.4, §9.1).
    let (root, store, donor_key) = seed_fixture();
    let mut donor = seed_fixture_lease(&store, &donor_key);
    let record_id = donor.record_id();
    let origin = Built::Affine(donor_key.checkout.clone());
    let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
    let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
    let cache = oxide_storage::seed_source::SourceCache::new(&donor.store_id).unwrap();
    let lease_id = std::mem::replace(&mut donor.lease_id, "lost-producer".into());
    let refused = store.publish_seed_catalog(
        &donor,
        seed_test_producer(),
        &lock,
        &[],
        &[],
        SystemTime::now(),
        process::now_ms(),
        true,
        Some(&origin),
        "debug",
        &cache,
    );
    assert!(refused.is_err());
    donor.lease_id = lease_id;
    assert!(!store
        .control
        .join("seeds")
        .join(format!("{}.catalog.json", donor.slot_id))
        .exists());
    store.release(donor, Some(origin)).unwrap();
    let state = store.load().unwrap();
    let record = state
        .records
        .iter()
        .find(|record| record.id == record_id)
        .unwrap();
    assert!(matches!(
        store.trusted_origin(record),
        Some(Built::Affine(path)) if path == donor_key.checkout
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn post_cargo_source_identity_rejects_a_changed_source_and_accepts_new_ones() {
    use crate::seed_plan::VerifiedSource;
    let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
    let package = crate::seed_lock::eligible(lock.as_bytes())
        .unwrap()
        .remove(0);
    let source = VerifiedSource {
        package: package.clone(),
        tree_digest: "b".repeat(64),
        kind: oxide_storage::lease::managed::SeedSourceKind::Registry,
        root: "/source".into(),
        verification_root: "/source".into(),
        proc_macro: false,
    };
    assert!(source_identity_matches(&source, &source));
    let mut changed = source.clone();
    changed.tree_digest = "c".repeat(64);
    assert!(!source_identity_matches(&source, &changed));
    let mut changed = source.clone();
    changed.root = "/other".into();
    assert!(!source_identity_matches(&source, &changed));
    let mut changed = source.clone();
    changed.proc_macro = true;
    assert!(!source_identity_matches(&source, &changed));
    let mut changed = source.clone();
    changed.kind = oxide_storage::lease::managed::SeedSourceKind::Git;
    assert!(!source_identity_matches(&source, &changed));
}

#[test]
fn seed_with_an_empty_allowlist_skips_without_side_effects() {
    let (root, store, key) = seed_fixture();
    let held = lease(&store, &key, None).expect("fresh lease");
    let lock = crate::seed_lock::analyze(b"version = 4\n").unwrap();
    let before = fs::read(store.control.join("state.json")).unwrap();
    let producer = seed_test_producer();
    let result = store.seed(&held, &producer, &lock, |_| {
        panic!("source verification ran")
    });
    assert!(matches!(
        result,
        Ok(SeedRun::Skipped("no eligible locked packages"))
    ));
    assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
    assert!(!store.control.join("seeds").exists());
    store.release(held, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_removable_store_without_its_registration_refuses_its_start_gate() {
    // §8.3: the removable start gate re-verifies the registration's
    // mount/volume-UID binding and reserve inside the admission lifetime.
    // A removable store with no paired internal control cannot prove that,
    // so its gate refuses before the attempt record or untrusted origin.
    let (root, mut store, _key) = seed_fixture();
    store.backend = Backend::ApfsVolume;
    let refused = store.seed_removable_gate();
    assert!(matches!(
        refused,
        Err(ref reason) if reason.contains("no paired internal control")
    ));
    assert!(!store.control.join("seeds").exists());
    fs::remove_dir_all(root).unwrap();
}

/// A released donor with a verified catalog that offers cfg-if.
fn front_door_donor() -> (
    PathBuf,
    Store,
    SlotKey,
    crate::seed_lock::Analysis,
    oxide_storage::lease::managed::SeedProducer,
) {
    use oxide_storage::lease::managed::{SeedCatalog, SeedSourceKind};
    let (root, store, donor_key) = seed_fixture();
    let donor = lease(&store, &donor_key, None).expect("donor lease");
    let source_root = root.join("source");
    fs::create_dir_all(source_root.join("src")).unwrap();
    let input = source_root.join("src/lib.rs");
    fs::write(&input, "source").unwrap();
    let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
    let lock = crate::seed_lock::analyze(lock.as_bytes()).unwrap();
    let source = crate::seed_plan::VerifiedSource {
        package: lock.eligible[0].clone(),
        tree_digest: "b".repeat(64),
        kind: SeedSourceKind::Registry,
        root: source_root.clone(),
        verification_root: source_root,
        proc_macro: false,
    };
    let fingerprint = donor
        .build_dir
        .join("debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa");
    fs::create_dir_all(&fingerprint).unwrap();
    for name in ["lib-cfg_if", "lib-cfg_if.json", "dep-lib-cfg_if"] {
        fs::write(fingerprint.join(name), "fingerprint").unwrap();
    }
    let deps = donor.build_dir.join("debug/deps");
    fs::create_dir_all(&deps).unwrap();
    let dep = deps.join("cfg_if-aaaaaaaaaaaaaaaa.d");
    let artifact = deps.join("libcfg_if-aaaaaaaaaaaaaaaa.rmeta");
    fs::write(&artifact, "artifact").unwrap();
    fs::write(
        &dep,
        format!(
            "{}: {}\n{}: {}\n{}:\n",
            dep.display(),
            input.display(),
            artifact.display(),
            input.display(),
            input.display()
        ),
    )
    .unwrap();
    let mut producer = seed_test_producer();
    producer.rustc_digest = "c".repeat(64);
    producer.context_digest = "d".repeat(64);
    producer.flags_digest = "e".repeat(64);
    let origin = Built::Affine(donor_key.checkout.clone());
    let cache = oxide_storage::seed_source::SourceCache::new(&donor.store_id).unwrap();
    store
        .publish_seed_catalog(
            &donor,
            producer.clone(),
            &lock,
            &[source.clone()],
            &[source.clone()],
            SystemTime::now(),
            process::now_ms(),
            true,
            Some(&origin),
            "debug",
            &cache,
        )
        .unwrap();
    let catalog_path = store
        .control
        .join("seeds")
        .join(format!("{}.catalog.json", donor.slot_id));
    let catalog = SeedCatalog::parse(&fs::read(&catalog_path).unwrap()).unwrap();
    assert_eq!(catalog.packages.len(), 1);
    assert_eq!(catalog.packages[0].units.len(), 1);
    assert_eq!(catalog.packages[0].units[0].files.len(), 5);
    assert!(store.control.join("seeds/sources.json").exists());
    store.release(donor, Some(origin)).unwrap();
    (root, store, donor_key, lock, producer)
}

#[test]
fn a_source_failure_before_the_first_clone_skips_and_changes_nothing() {
    let (root, store, _donor_key, lock, producer) = front_door_donor();
    let recipient_checkout = root.join("recipient");
    fs::create_dir(&recipient_checkout).unwrap();
    let recipient_key = SlotKey {
        checkout: recipient_checkout,
        identity: "recipient".into(),
        compatibility: "quick-key".into(),
    };
    let recipient = lease(&store, &recipient_key, None).expect("recipient lease");
    let before = fs::read(store.control.join("state.json")).unwrap();
    // A skip before the first clone writes no attempt and changes no state:
    // the command builds cold in the same empty slot.
    let skipped = store
        .seed(&recipient, &producer, &lock, |_| {
            Err("injected source failure")
        })
        .unwrap();
    assert!(matches!(
        skipped,
        SeedRun::Skipped("injected source failure")
    ));
    assert_eq!(fs::read(store.control.join("state.json")).unwrap(), before);
    assert!(!store
        .control
        .join("seeds")
        .join(format!("{}.attempt.json", recipient.slot_id))
        .exists());
    store.release(recipient, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Second-slot rule: a busy slot of this checkout's key sends a further
/// command to another idle compatible slot or a new one; a slot whose
/// holder died while its group lives is never reused; with no safely
/// evictable idle slot under the project cap, a command waits only for a
/// specific held compatible slot.
#[test]
fn a_busy_slot_sends_a_further_command_elsewhere() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-second-slot-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    let pool = fs::canonicalize(&root).unwrap().join("pool");
    init_budget_pool(&pool, 1 << 30).unwrap();
    let store = Store {
        control: pool.join(MANAGED_NAME),
        payload: pool.join("payload"),
        pool: pool.clone(),
        backend: Backend::DirectoryBudget,
        budget: Some(1 << 30),
        recorded_budget: 1 << 30,
        max_active: 16,
        sibling: None,
    };
    store.ensure_control().unwrap();
    let checkout = fs::canonicalize(&root).unwrap().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let key = SlotKey {
        checkout,
        identity: "identity".to_owned(),
        compatibility: "quick-key".to_owned(),
    };
    let first = lease(&store, &key, Some("exec tool")).expect("the first command leased");
    // The holder's group is this test's own, which is alive.
    // SAFETY: getpgrp has no preconditions.
    let group = unsafe { libc::getpgrp() };
    store.mark_spawning(&first).unwrap();
    store.mark_running(&first, group).unwrap();
    let second = lease(&store, &key, None).expect("a further command leased");
    assert_ne!(second.slot_id, first.slot_id);
    let report = store.report(Accounting::Recorded, 1).unwrap();
    let shown = |slot: &str| {
        report
            .slots
            .iter()
            .find(|view| view.slot_id == slot)
            .and_then(|view| view.holder.clone())
    };
    assert_eq!(shown(&first.slot_id).as_deref(), Some("exec tool"));
    assert_eq!(shown(&second.slot_id), None);

    // Fill the project to its cap with held records; each further lease
    // takes a new slot because the earlier ones are not idle.
    let mut held = vec![first];
    held.push(second);
    while held.len() < PROJECT_SLOT_CAP {
        held.push(lease(&store, &key, None).expect("a slot below the project cap"));
    }
    // Every project slot is held: a further command waits for a specific
    // held compatible slot, and no slot is evicted.
    assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));

    // Release the newest held slot: it is the specific slot that can
    // become reusable, and the waiting rule takes it instead of a new one.
    let released = held.pop().expect("newest lease");
    let released_slot = released.slot_id.clone();
    store.release(released, None).unwrap();
    let reused = lease(&store, &key, None).expect("the released slot is taken");
    assert_eq!(reused.slot_id, released_slot);

    // The first holder's group is still alive: its slot stays held and is
    // never reused while the cap admits nothing else.
    let view = store.ledger().unwrap();
    let held_first = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == held[0].slot_id)
        .unwrap();
    assert_eq!(held_first.state, SlotOccupancy::Held);
    assert!(matches!(lease(&store, &key, None), Err(Wait::Checkout)));
    store.release(reused, None).unwrap();
    for lease in held {
        drop(lease);
    }
    fs::remove_dir_all(root).unwrap();
}

/// A run program that uses an owned temporary directory is still that
/// directory's writer. The lease is the fence: while the holder's group
/// is alive a competing command takes another slot and never reconciles
/// the live directory, and only the holder's own release — which the
/// product performs after positive group closure — removes it.
#[test]
fn a_live_program_keeps_its_owned_temporary_directory_from_a_competing_lease() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-live-scratch-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    let pool = fs::canonicalize(&root).unwrap().join("pool");
    init_budget_pool(&pool, 1 << 30).unwrap();
    let store = Store {
        control: pool.join(MANAGED_NAME),
        payload: pool.join("payload"),
        pool: pool.clone(),
        backend: Backend::DirectoryBudget,
        budget: Some(1 << 30),
        recorded_budget: 1 << 30,
        max_active: 16,
        sibling: None,
    };
    store.ensure_control().unwrap();
    let checkout = fs::canonicalize(&root).unwrap().join("checkout");
    fs::create_dir(&checkout).unwrap();
    let key = SlotKey {
        checkout,
        identity: "identity".to_owned(),
        compatibility: "quick-key".to_owned(),
    };
    let mut live = lease(&store, &key, Some("exec run program")).expect("the run leased");
    let store_id = store.ledger().unwrap().store_id.clone();
    let slot_dir = live.build_dir.parent().expect("slot dir").to_path_buf();
    let scratch = crate::child_temp::choose_with(
        None,
        &store.payload,
        &slot_dir,
        crate::child_temp::Identity {
            store_id,
            slot_id: live.slot_id.clone(),
            incarnation: live.incarnation.clone(),
            lease_id: live.lease_id().to_owned(),
        },
        4096,
    )
    .expect("the run's owned directory");
    let owned = scratch.owned().expect("owned directory");
    assert!(owned.path().is_dir());
    fs::write(owned.path().join("program-marker"), b"live writer").unwrap();
    live.scratch = Some(owned.clone());
    // The program's own group is alive for the whole competing attempt.
    // SAFETY: getpgrp has no preconditions.
    let group = unsafe { libc::getpgrp() };
    store.mark_spawning(&live).unwrap();
    store.mark_running(&live, group).unwrap();

    // A competing command in the same checkout takes another slot and
    // leaves the live directory alone: it never reconciles, deletes or
    // re-creates it.
    let competing = lease(&store, &key, None).expect("a competing command leased");
    assert_ne!(competing.slot_id, live.slot_id);
    assert!(
        owned.path().is_dir(),
        "a competing lease removed the live program's directory"
    );
    assert_eq!(
        fs::read(owned.path().join("program-marker")).unwrap(),
        b"live writer"
    );
    // The held slot is not evictable by the project cap either.
    let view = store.ledger().unwrap();
    let held = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == live.slot_id)
        .expect("the live slot is a ledger member");
    assert_eq!(held.state, SlotOccupancy::Held);
    store.release(competing, None).unwrap();
    assert!(owned.path().is_dir(), "releasing another slot touched it");

    // Only this holder's release removes it, which the product performs
    // after its own group is observed empty.
    store.release(live, None).unwrap();
    assert!(
        !owned.path().exists(),
        "the released holder kept its owned directory"
    );
    fs::remove_dir_all(root).unwrap();
}

fn record(phase: Phase, group: Option<i32>, lease_id: Option<&str>) -> Record {
    Record {
        id: 1,
        identity: "identity".to_owned(),
        compatibility: "key".to_owned(),
        profile: "quick".to_owned(),
        checkout: Some(PathBuf::from("/checkout")),
        phase,
        process_group: group,
        last_used: 1,
        holder: Some(Holder {
            pid: 41,
            started_ms: 7,
            lease_holder: "oxide-41-3".to_owned(),
            command: None,
        }),
        lease_id: lease_id.map(str::to_owned),
        pending_recovery: None,
        retired: None,
        lost: None,
        last_used_ms: None,
    }
}

#[test]
fn lost_volume_closure_needs_the_own_closed_group() {
    let mut own = record(Phase::Running, Some(77), Some("lease"));
    own.holder.as_mut().unwrap().pid = std::process::id();
    let bypassed = crate::wrapper::Policy::from_env().bypasses()
        && [
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        ]
        .iter()
        .all(|name| std::env::var_os(name).is_none_or(|value| value.is_empty()));
    if bypassed {
        assert_eq!(unclosed_writers(&own), None);
    }
    // Another process's record, an exec tool or no holder: not closed.
    let other = record(Phase::Running, Some(77), Some("lease"));
    assert!(unclosed_writers(&other).unwrap().contains("oxide pid 41"));
    let mut exec = own.clone();
    exec.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
    assert!(unclosed_writers(&exec).unwrap().contains("exec tool"));
    // Cargo never started: no writer.
    let acquiring = record(Phase::Acquiring, None, Some("lease"));
    assert_eq!(unclosed_writers(&acquiring), None);
}

#[test]
fn a_lost_slot_is_never_active_or_reused() {
    let mut lost = record(Phase::Retired, Some(77), Some("lease"));
    lost.lost = Some(Lost {
        slot_id: "3".to_owned(),
        at_ms: 1,
        unclosed: None,
    });
    assert!(!lost.active());
    assert!(lost
        .lost
        .as_ref()
        .unwrap()
        .describe(false)
        .contains("deleted whole"));
    lost.lost.as_mut().unwrap().unclosed = Some("why".to_owned());
    let description = lost.lost.as_ref().unwrap().describe(false);
    assert!(description.starts_with("quarantined"), "{description}");
    assert!(
        !description.contains("release-quarantined"),
        "post-Barge quarantined descriptions must not recommend release-quarantined: {description}"
    );
    let encoded = serde_json::to_string(&lost).unwrap();
    let decoded: Record = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded.lost.unwrap().unclosed.as_deref(), Some("why"));
}

/// A quarantined lost record with a holder start, for the boot-session
/// closure table.
fn quarantined_at(started_ms: u64) -> Record {
    let mut entry = record(Phase::Retired, None, Some("lease"));
    entry.holder.as_mut().unwrap().started_ms = started_ms;
    entry.holder.as_mut().unwrap().command = Some("exec tool".to_owned());
    entry.lost = Some(Lost {
        slot_id: "5".to_owned(),
        at_ms: 1,
        unclosed: Some("exec tool may have started processes outside its group".to_owned()),
    });
    entry
}

/// A restart ends every process: a quarantined holder that started before
/// the current boot session is closed, one that started inside the safety
/// margin or during this session is not.
#[test]
fn only_a_holder_from_an_earlier_boot_session_is_closed_by_the_restart() {
    let boot = 1_900_000_000_000_u64;
    let earlier = quarantined_at(boot - BOOT_MARGIN_MS - 1);
    assert!(lost_writers_closed(&earlier, Some(boot)));
    assert!(closed_by_boot(&earlier, Some(boot)));
    // The same record is still quoted and kept when the boot fact is
    // absent (the counterfactual: without the check nothing closes it).
    assert!(!lost_writers_closed(&earlier, None));
    assert!(!closed_by_boot(&earlier, None));
    for started in [boot - BOOT_MARGIN_MS, boot - 1, boot, boot + 1] {
        let near = quarantined_at(started);
        assert!(!lost_writers_closed(&near, Some(boot)), "started {started}");
        assert!(!closed_by_boot(&near, Some(boot)), "started {started}");
    }
    // A record that already closed positively is never reported as
    // closed by the boot session, whatever the boot time says.
    let mut positive = quarantined_at(0);
    positive.lost.as_mut().unwrap().unclosed = None;
    assert!(lost_writers_closed(&positive, None));
    assert!(!closed_by_boot(&positive, Some(boot)));
}

/// An unknown boot time, a missing holder and a zero start are never
/// closure, and elapsed time is not a fact this reads.
#[test]
fn an_unknown_boot_time_or_a_missing_start_never_closes_a_lost_slot() {
    let boot = 1_900_000_000_000_u64;
    assert!(!lost_writers_closed(
        &quarantined_at(boot - BOOT_MARGIN_MS - 1),
        None
    ));
    assert!(!lost_writers_closed(&quarantined_at(0), Some(boot)));
    let mut missing = quarantined_at(boot - BOOT_MARGIN_MS - 1);
    missing.holder = None;
    assert!(!lost_writers_closed(&missing, Some(boot)));
    assert!(!closed_by_boot(&missing, Some(boot)));
    // A record without a lost entry is not a lost slot at all.
    assert!(!lost_writers_closed(
        &record(Phase::Idle, None, None),
        Some(boot)
    ));
}

fn held(holder: &str, lease_id: &str) -> LedgerSlot {
    LedgerSlot {
        slot_id: "1".to_owned(),
        incarnation: "1".to_owned(),
        state: SlotOccupancy::Held,
        project: PathBuf::from("/pool/oxide-managed/affinity/1"),
        workspace: PathBuf::from("/checkout"),
        profile: LeaseProfile::Dev,
        holder: Some(holder.to_owned()),
        lease_id: Some(lease_id.to_owned()),
        dir: None,
        rank: 0,
        recorded: None,
    }
}

#[test]
fn recovers_only_an_empty_recorded_group() {
    let slot = held("oxide-41-3", "v1.a.1.1.1");
    let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
    assert_eq!(settle(&running, Some(&slot), |_| true), Settle::Recover);
    // A surviving member, or a reused or unreadable group ID, reports
    // anything but ESRCH.
    assert_eq!(settle(&running, Some(&slot), |_| false), Settle::Keep);
    let unrecorded = record(Phase::Spawning, None, Some("v1.a.1.1.1"));
    assert_eq!(settle(&unrecorded, Some(&slot), |_| true), Settle::Keep);
    let no_group = record(Phase::Running, None, Some("v1.a.1.1.1"));
    assert_eq!(settle(&no_group, Some(&slot), |_| true), Settle::Keep);
}

#[test]
fn cargo_that_never_started_leaves_no_group() {
    let slot = held("oxide-41-3", "v1.a.1.1.1");
    let granted = record(Phase::Acquiring, None, Some("v1.a.1.1.1"));
    assert_eq!(settle(&granted, Some(&slot), |_| false), Settle::Recover);
    // Died between the ledger commit and saving the lease ID: the unique
    // holder string identifies the lease.
    let unsaved = record(Phase::Acquiring, None, None);
    assert_eq!(settle(&unsaved, Some(&slot), |_| false), Settle::Recover);
    assert_eq!(settle(&unsaved, None, |_| false), Settle::Reset);
}

#[test]
fn never_recovers_a_lease_the_record_did_not_take() {
    let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
    let other = held("nucleus-task", "v1.a.1.2.5");
    assert_eq!(settle(&running, Some(&other), |_| true), Settle::Keep);
    let unsaved = record(Phase::Acquiring, None, None);
    assert_eq!(settle(&unsaved, Some(&other), |_| true), Settle::Keep);
    let mut anonymous = record(Phase::Running, Some(900), None);
    anonymous.holder = None;
    let slot = held("oxide-41-3", "v1.a.1.1.1");
    assert_eq!(settle(&anonymous, Some(&slot), |_| true), Settle::Keep);
}

#[test]
fn state_without_holder_or_recoveries_still_loads() {
    let earlier = r#"{"version":1,"next_id":3,"next_use":2,"tickets":[],"records":[
        {"id":1,"identity":"i","compatibility":"k","profile":"quick",
         "checkout":"/checkout","phase":"running","process_group":900,"last_used":1}]}"#;
    let state: State = serde_json::from_str(earlier).expect("earlier state parses");
    assert!(state.recoveries.is_empty() && state.cleans.is_empty());
    let record = &state.records[0];
    assert!(record.holder.is_none() && record.lease_id.is_none());
    // Without a holder or lease ID the held lease cannot be proven its own.
    let slot = held("oxide-41-3", "v1.a.1.1.1");
    assert_eq!(settle(record, Some(&slot), |_| true), Settle::Keep);
    // Status must not promise automatic recovery for it.
    assert_eq!(return_path(record, &slot), Return::NotOwn);
}

#[test]
fn status_path_matches_the_rule() {
    let slot = held("oxide-41-3", "v1.a.1.1.1");
    let running = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
    assert_eq!(return_path(&running, &slot), Return::GroupEmpty(900));
    let spawning = record(Phase::Spawning, None, Some("v1.a.1.1.1"));
    assert_eq!(return_path(&spawning, &slot), Return::GroupUnrecorded);
    let acquiring = record(Phase::Acquiring, None, None);
    assert_eq!(return_path(&acquiring, &slot), Return::NeverStarted);
    let other = held("nucleus-task", "v1.a.1.2.5");
    assert_eq!(return_path(&running, &other), Return::NotOwn);
}

#[test]
fn pending_recovery_survives_a_crash_after_the_ledger_change() {
    let mut state = State::default();
    let mut crashed = record(Phase::Running, Some(900), Some("v1.a.1.1.1"));
    crashed.pending_recovery = Some(Recovery {
        slot_id: "1".to_owned(),
        lease_id: "v1.a.1.1.1".to_owned(),
        checkout: None,
        holder_pid: 41,
        holder_started_ms: 7,
        process_group: Some(900),
        recovered_ms: 9,
        recovered_by: 42,
    });
    let bytes = serde_json::to_vec(&crashed).expect("serialise");
    state
        .records
        .push(serde_json::from_slice(&bytes).expect("pending reloads"));
    // The ledger recovery committed, so the slot is no longer held.
    assert_eq!(settle(&state.records[0], None, |_| false), Settle::Reset);
    let pending = state.records[0].pending_recovery.take().expect("pending");
    state.log_recovery(pending);
    state.settle_idle(0);
    assert_eq!(state.recoveries.len(), 1);
    assert!(state.records[0].pending_recovery.is_none());
}

/// A crashed record whose lease is still held, with its lock free, plus one
/// unrelated record whose lock a live command holds. Returns the crashed
/// record's ID and the live command's lock.
fn crashed_record_beside_a_live_one(
    label: &str,
) -> (PathBuf, Store, u64, File, Option<barge::Hold>) {
    let (root, store, key) = lease_fixture(label);
    let leased = lease(&store, &key, None).expect("fixture lease");
    let crashed = store
        .load()
        .unwrap()
        .records
        .iter()
        .find(|record| record.active())
        .expect("an active record")
        .id;
    // The crash shape: the ledger still holds the slot, the record lock is
    // free, and Cargo never started (so its group was never recorded). The
    // slot's Barge root hold stays with the ledger, which otherwise reads a
    // held slot whose holder has gone as idle.
    let mut leased = leased;
    let root_hold = leased._root_hold.take();
    drop(leased);
    let mut state = store.load().unwrap();
    let record = state
        .records
        .iter_mut()
        .find(|record| record.id == crashed)
        .expect("the crashed record");
    record.phase = Phase::Acquiring;
    record.process_group = None;
    let lease_id = record
        .lease_id
        .clone()
        .expect("the crashed record's lease ID");
    store.save(&state).unwrap();
    // The ledger still holds the slot under that lease: the crash left it held.
    let ledger_path = store.pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let slot = ledger["slots"]
        .as_array_mut()
        .expect("slots")
        .first_mut()
        .expect("the fixture slot");
    slot["state"] = serde_json::json!("held");
    slot["lease_id"] = serde_json::json!(lease_id);
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let live = hold_record_for_tests(&store, &key.checkout, 900);
    (root, store, crashed, live, root_hold)
}

/// Regression (oxide#356): a flush whose reload drops the record it was
/// settling must not leave the recovery loop indexing past the end of the
/// reloaded records.
#[test]
fn recover_settled_survives_a_reload_that_drops_a_record() {
    let (root, store, crashed, _live, _root_hold) =
        crashed_record_beside_a_live_one("recover-drops");
    let flushes = std::cell::Cell::new(0);
    let mut state = store.load().unwrap();
    store
        .recover_settled_with(&mut state, |store, _slot, _record| {
            flushes.set(flushes.get() + 1);
            // The flush retires the record, and the reload that follows
            // no longer holds it.
            let mut reloaded = store.load()?;
            reloaded.records.retain(|record| record.id != crashed);
            store.save(&reloaded)?;
            Ok(false)
        })
        .expect("recovery settles without indexing past the reloaded records");
    assert_eq!(flushes.get(), 1, "the flush did not run");
    assert!(state.records.iter().all(|record| record.id != crashed));
    assert!(state
        .records
        .iter()
        .any(|record| record.id == 900 && record.phase == Phase::Running));
    fs::remove_dir_all(root).unwrap();
}

/// Regression (oxide#356): a flush whose reload reorders the records must
/// still recover the record it settled, found by its ID rather than position.
#[test]
fn recover_settled_finds_its_record_after_a_reload_reorders_them() {
    let (root, store, crashed, _live, _root_hold) =
        crashed_record_beside_a_live_one("recover-reorders");
    let mut state = store.load().unwrap();
    store
        .recover_settled_with(&mut state, |store, _slot, _record| {
            let mut reloaded = store.load()?;
            reloaded.records.reverse();
            store.save(&reloaded)?;
            Ok(true)
        })
        .expect("recovery settles after the reload reorders the records");
    let settled = state
        .records
        .iter()
        .find(|record| record.id == crashed)
        .expect("the crashed record is kept");
    assert_eq!(settled.phase, Phase::Idle);
    assert_eq!(state.recoveries.len(), 1);
    assert!(state
        .records
        .iter()
        .any(|record| record.id == 900 && record.phase == Phase::Running));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pool_cap_parses_decimal_and_binary_units() {
    let budget = Backend::DirectoryBudget;
    assert_eq!(parse_cap("100GB", budget), Ok(100_000_000_000));
    assert_eq!(parse_cap("1TiB", budget), Ok(1024 * GIB));
    assert_eq!(parse_cap("1024KiB", budget), Ok(1024 * 1024));
    assert_eq!(parse_cap("12MB", budget), Ok(12_000_000));
    assert_eq!(parse_cap("1048577", budget), Ok(1_048_577));
    for invalid in [
        "",
        "MiB",
        "1.5GiB",
        "-1",
        "64 MiB",
        "99999999999999999999",
        "17TiB",
        "0",
    ] {
        let error = parse_cap(invalid, budget).unwrap_err();
        assert!(error.contains("OXIDE_POOL_CAP"), "{error}");
    }
}

#[test]
fn the_retired_image_backend_refuses_by_name() {
    let error = Backend::recorded("apfs-sparse-image-v1").unwrap_err();
    assert!(error.contains("retired"), "{error}");
    assert_eq!(
        Backend::recorded("directory-budget-v1"),
        Ok(Backend::DirectoryBudget)
    );
}

#[test]
fn recovery_log_keeps_the_newest() {
    let mut state = State::default();
    for index in 0..MAX_RECOVERIES + 3 {
        state.log_recovery(Recovery {
            slot_id: index.to_string(),
            lease_id: format!("v1.a.{index}.1.1"),
            checkout: None,
            holder_pid: 41,
            holder_started_ms: 7,
            process_group: Some(900),
            recovered_ms: 9,
            recovered_by: 42,
        });
    }
    assert_eq!(state.recoveries.len(), MAX_RECOVERIES);
    assert_eq!(state.recoveries[0].slot_id, "3");
}

fn worst_record(id: u64) -> Record {
    Record {
        id,
        identity: "i".repeat(MAX_IDENTITY_BYTES),
        compatibility: "c".repeat(MAX_COMPATIBILITY_BYTES),
        profile: "p".repeat(MAX_PROFILE_BYTES),
        checkout: Some(PathBuf::from(format!(
            "/{}",
            "x".repeat(MAX_PATH_TEXT_BYTES - 1)
        ))),
        phase: Phase::Retired,
        process_group: Some(i32::MAX),
        last_used: u64::MAX,
        holder: Some(Holder {
            pid: u32::MAX,
            started_ms: u64::MAX,
            lease_holder: "h".repeat(MAX_LEASE_HOLDER_BYTES),
            command: Some("c".repeat(MAX_COMMAND_BYTES)),
        }),
        lease_id: Some("l".repeat(MAX_STATE_LEASE_ID_BYTES)),
        pending_recovery: Some(Recovery {
            slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
            lease_id: "l".repeat(MAX_STATE_LEASE_ID_BYTES),
            checkout: Some(PathBuf::from(format!(
                "/{}",
                "x".repeat(MAX_PATH_TEXT_BYTES - 1)
            ))),
            holder_pid: 1,
            holder_started_ms: 2,
            process_group: Some(3),
            recovered_ms: 4,
            recovered_by: 5,
        }),
        retired: Some("r".repeat(MAX_DIAGNOSTIC_BYTES)),
        lost: Some(Lost {
            slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
            at_ms: u64::MAX,
            unclosed: Some("u".repeat(MAX_DIAGNOSTIC_BYTES)),
        }),
        last_used_ms: Some(u64::MAX),
    }
}

fn escaped(value: usize) -> String {
    "\u{1}".repeat(value)
}

fn escaped_path(bytes: usize) -> PathBuf {
    PathBuf::from(format!("/{}", escaped(bytes - 1)))
}

fn escaped_record(id: u64) -> Record {
    Record {
        id,
        identity: escaped(MAX_IDENTITY_BYTES),
        compatibility: escaped(MAX_COMPATIBILITY_BYTES),
        profile: escaped(MAX_PROFILE_BYTES),
        checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
        phase: Phase::Retired,
        process_group: Some(i32::MAX),
        last_used: u64::MAX,
        holder: Some(Holder {
            pid: u32::MAX,
            started_ms: u64::MAX,
            lease_holder: escaped(MAX_LEASE_HOLDER_BYTES),
            command: Some(escaped(MAX_COMMAND_BYTES)),
        }),
        lease_id: Some(escaped(MAX_STATE_LEASE_ID_BYTES)),
        pending_recovery: Some(Recovery {
            slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
            lease_id: escaped(MAX_STATE_LEASE_ID_BYTES),
            checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
            holder_pid: 1,
            holder_started_ms: 2,
            process_group: Some(3),
            recovered_ms: 4,
            recovered_by: 5,
        }),
        retired: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
        lost: Some(Lost {
            slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
            at_ms: u64::MAX,
            unclosed: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
        }),
        last_used_ms: Some(u64::MAX),
    }
}

#[test]
fn worst_escaped_control_state_roundtrips_through_save_and_load() {
    let (root, store, _removable) = paired_controls("control-escaped");
    // The current representation carries both forward fences, so this is
    // the worst valid version-5 document and not an owned-outputs-only one.
    let mut state = State {
        version: RECORDED_ACCOUNTING_STATE_VERSION,
        owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
        recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
        ..State::default()
    };
    for id in 1..=MAX_RECORDS as u64 {
        state.records.push(escaped_record(id));
    }
    for id in 1..=MAX_TICKETS as u64 {
        state.tickets.push(Ticket { id, pid: u32::MAX });
    }
    for id in 1..=MAX_CLEANS as u64 {
        state.cleans.push(Clean {
            id,
            checkout: escaped_path(MAX_PATH_TEXT_BYTES),
            process_group: Some(i32::MAX),
        });
    }
    for _ in 0..MAX_RECOVERIES {
        state.recoveries.push(Recovery {
            slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
            lease_id: escaped(MAX_STATE_LEASE_ID_BYTES),
            checkout: Some(escaped_path(MAX_PATH_TEXT_BYTES)),
            holder_pid: u32::MAX,
            holder_started_ms: u64::MAX,
            process_group: Some(i32::MAX),
            recovered_ms: u64::MAX,
            recovered_by: u32::MAX,
        });
        state.flushes.push(Flush {
            slot_id: escaped(MAX_STATE_SLOT_ID_BYTES),
            at_ms: u64::MAX,
            micros: None,
            error: Some(escaped(MAX_DIAGNOSTIC_BYTES)),
            lost: true,
        });
    }
    let before = serde_json::to_vec(&state).expect("serialize escaped worst valid state");
    assert!(
        before.len() as u64 <= MAX_STATE_BYTES,
        "escaped worst valid state {} exceeds {MAX_STATE_BYTES}",
        before.len()
    );
    assert!(
        before.len() > 4_000_000,
        "escaped fixture is not substantial: {}",
        before.len()
    );
    assert!(before.windows(6).any(|window| window == b"\\u0001"));

    store
        .save(&state)
        .expect("the escaped worst valid state saves");
    let loaded = store.load().expect("the escaped worst valid state loads");
    assert_eq!(loaded.version, RECORDED_ACCOUNTING_STATE_VERSION);
    assert_eq!(
        serde_json::to_vec(&loaded).expect("reserialize loaded"),
        before,
        "the escaped worst valid state did not round-trip byte for byte"
    );
    assert_eq!(loaded.records.len(), MAX_RECORDS);
    assert_eq!(loaded.records[0].checkout, state.records[0].checkout);
    assert_eq!(loaded.flushes.len(), MAX_FLUSHES);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn worst_valid_control_state_fits_its_object_and_byte_caps() {
    // The current representation, so the bound covers the version-5 fence.
    let mut state = State {
        version: RECORDED_ACCOUNTING_STATE_VERSION,
        recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
        ..State::default()
    };
    for id in 1..=MAX_RECORDS as u64 {
        state.records.push(worst_record(id));
    }
    for id in 1..=MAX_TICKETS as u64 {
        state.tickets.push(Ticket { id, pid: u32::MAX });
    }
    for id in 1..=MAX_CLEANS as u64 {
        state.cleans.push(Clean {
            id,
            checkout: PathBuf::from(format!("/{}", "x".repeat(MAX_PATH_TEXT_BYTES - 1))),
            process_group: Some(i32::MAX),
        });
    }
    for index in 0..MAX_RECOVERIES {
        state.recoveries.push(Recovery {
            slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
            lease_id: "l".repeat(MAX_STATE_LEASE_ID_BYTES),
            checkout: None,
            holder_pid: u32::MAX,
            holder_started_ms: u64::MAX,
            process_group: Some(i32::MAX),
            recovered_ms: u64::MAX,
            recovered_by: u32::MAX,
        });
        state.flushes.push(Flush {
            slot_id: "s".repeat(MAX_STATE_SLOT_ID_BYTES),
            at_ms: u64::MAX,
            micros: Some(u64::MAX),
            error: Some("e".repeat(MAX_DIAGNOSTIC_BYTES)),
            lost: index % 2 == 0,
        });
    }
    state.next_id = u64::MAX;
    state.next_use = u64::MAX;
    validate_state(&state, false).expect("the worst valid control state is accepted");
    let bytes = serde_json::to_vec(&state).expect("serialize");
    assert!(
        bytes.len() as u64 <= MAX_STATE_BYTES,
        "worst valid control state {} exceeds {MAX_STATE_BYTES}",
        bytes.len()
    );
    assert!(
        bytes.len() > 1_000_000,
        "fixture is not substantial: {}",
        bytes.len()
    );
    let object = serde_json::to_vec(&worst_record(1)).expect("record");
    assert!(object.len() <= MAX_RECORD_OBJECT_BYTES);
    assert!(
        object.len() * MAX_RECORDS <= MAX_STATE_BYTES as usize,
        "a full record set exceeds the state bound"
    );
}

#[test]
fn legacy_control_reads_keep_their_limits_and_save_keeps_the_previous_version_until_the_fence() {
    let (root, store, _removable) = paired_controls("control-fence");
    let mut legacy = State {
        version: LEGACY_STATE_VERSION,
        ..State::default()
    };
    legacy.records.push(Record {
        checkout: Some(PathBuf::from(format!(
            "/{}",
            "x".repeat(MAX_PATH_TEXT_BYTES + 64)
        ))),
        ..worst_record(1)
    });
    let bytes = serde_json::to_vec(&legacy).expect("legacy state");
    let path = store.control.join("state.json");
    write_private(&path, &bytes);
    let loaded = store.load().expect("legacy oversized field still reads");
    assert_eq!(loaded.version, LEGACY_STATE_VERSION);
    // A revised mutation validates every legacy field against the raised
    // caps and refuses with the original file untouched.
    let error = store.save(&loaded).unwrap_err();
    assert!(error.contains("field bounds"), "{error}");
    assert_eq!(
        fs::read(&path).unwrap(),
        bytes,
        "the original state was rewritten"
    );
    // Within the caps, an ordinary save stays on the previous numeric
    // version so older binaries are not refused before the owned-outputs
    // fence opts the pool in.
    let mut bounded = legacy.clone();
    bounded.records[0].checkout = Some(PathBuf::from("/checkout"));
    store.save(&bounded).expect("bounded legacy state saves");
    let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        stored["version"],
        serde_json::json!(LEGACY_FIELDS_STATE_VERSION)
    );
    assert!(stored.get("owned_outputs").is_none());
    // The version *derivation*, proved on `save` itself so it cannot
    // depend on which caller reached it: an owned-outputs successor alone
    // is version 4, and only the recorded-accounting fence (oxide#146)
    // makes it 5.
    let owned_only = State {
        owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
        ..State::default()
    };
    store
        .save(&owned_only)
        .expect("an owned-outputs-only state saves");
    let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored["version"], serde_json::json!(PREVIOUS_STATE_VERSION));
    assert_eq!(
        stored["owned_outputs"],
        serde_json::json!(OWNED_OUTPUTS_FENCE)
    );
    assert!(stored.get("recorded_accounting").is_none());
    let both = State {
        owned_outputs: Some(OWNED_OUTPUTS_FENCE.to_owned()),
        recorded_accounting: Some(RECORDED_ACCOUNTING_FENCE.to_owned()),
        ..State::default()
    };
    store.save(&both).expect("a doubly fenced state saves");
    let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        stored["version"],
        serde_json::json!(RECORDED_ACCOUNTING_STATE_VERSION)
    );
    assert_eq!(
        stored["recorded_accounting"],
        serde_json::json!(RECORDED_ACCOUNTING_FENCE)
    );
    store
        .save(&State::default())
        .expect("an unfenced state saves");

    // The *path* truth: `fence_owned_outputs` is a mutating command, so it
    // takes the admission gate, which makes the control fence durable
    // first. That is the ordering the design requires — the control
    // hand-off must precede any incompatible ledger write or locked pool
    // open — and it is why this path ends at version 8 with both fences.
    store
        .fence_owned_outputs()
        .expect("the forward fence saves");
    let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored["version"], serde_json::json!(STATE_VERSION));
    assert_eq!(
        stored["owned_outputs"],
        serde_json::json!(OWNED_OUTPUTS_FENCE)
    );
    assert_eq!(
        stored["recorded_accounting"],
        serde_json::json!(RECORDED_ACCOUNTING_FENCE)
    );
    // A repeated admission fence is idempotent.
    let fenced = stored;
    store
        .fence_admission_state()
        .expect("a repeated fence is a no-op");
    store
        .fence_admission_state()
        .expect("a repeated fence is a no-op");
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after, fenced, "a repeated fence rewrote the state");
    // Genuine absence at the current version refuses, and an explicit JSON null is a
    // present non-string field that the shared parser refuses too.
    let mut unfenced = fenced.clone();
    unfenced
        .as_object_mut()
        .expect("object")
        .remove("recorded_accounting");
    write_private(&path, &serde_json::to_vec(&unfenced).unwrap());
    let error = store.load().unwrap_err();
    assert!(
        error.contains("lacks the recorded_accounting fence"),
        "{error}"
    );
    let mut nulled = fenced.clone();
    nulled["recorded_accounting"] = serde_json::Value::Null;
    write_private(&path, &serde_json::to_vec(&nulled).unwrap());
    let error = store.load().unwrap_err();
    assert!(error.contains("not a string"), "{error}");
    let mut unknown = fenced.clone();
    unknown["recorded_accounting"] = serde_json::json!("oxide.accounting/9");
    write_private(&path, &serde_json::to_vec(&unknown).unwrap());
    let error = store.load().unwrap_err();
    assert!(
        error.contains("invalid recorded_accounting fence"),
        "{error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_control_reads_keep_the_legacy_byte_limit() {
    let (root, store, _removable) = paired_controls("control-legacy-bytes");
    let mut legacy = State {
        version: LEGACY_STATE_VERSION,
        ..State::default()
    };
    legacy.records.push(Record {
        retired: Some("r".repeat(LEGACY_MAX_STATE_BYTES as usize)),
        ..worst_record(1)
    });
    let bytes = serde_json::to_vec(&legacy).expect("legacy state");
    assert!(bytes.len() as u64 > LEGACY_MAX_STATE_BYTES);
    write_private(&store.control.join("state.json"), &bytes);
    let error = store.load().unwrap_err();
    assert!(error.contains("byte limit"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    let mut file = options.open(path).unwrap();
    file.write_all(bytes).unwrap();
}

fn ledger_slot(id: &str, incarnation: &str, dir: Option<&str>) -> LedgerSlot {
    LedgerSlot {
        slot_id: id.to_owned(),
        incarnation: incarnation.to_owned(),
        state: SlotOccupancy::Idle,
        project: PathBuf::from("/pool/oxide-managed/affinity/1"),
        workspace: PathBuf::from("/work/oxide"),
        profile: LeaseProfile::Dev,
        holder: None,
        lease_id: None,
        dir: dir.map(str::to_owned),
        rank: 0,
        recorded: None,
    }
}

fn observation(slot: &LedgerSlot, at_ms: u64) -> Observation {
    Observation {
        store_id: "a".repeat(32),
        slot_id: slot.slot_id.clone(),
        incarnation: slot.incarnation.clone(),
        dir: slot.dir.clone(),
        measured_at_ms: at_ms,
        allocated_bytes: 3 * 1024 * 1024,
        source: ObservationSource::LeaseReuse,
        lease_id: Some("v1.a.1.1.1".to_owned()),
    }
}

/// A slot whose last walk stopped partial is reported as
/// `not measured (walk incomplete)` and never with a figure; a later
/// complete walk of the same generation replaces that fact (oxide#186).
#[test]
fn an_incomplete_admission_walk_reads_as_not_measured() {
    let slot = ledger_slot("1", "7", Some("oxide-abcd/quick-1"));
    let incomplete = Observation {
        store_id: "a".repeat(32),
        slot_id: slot.slot_id.clone(),
        incarnation: slot.incarnation.clone(),
        dir: slot.dir.clone(),
        measured_at_ms: 10_000,
        allocated_bytes: 0,
        source: ObservationSource::AdmissionWalkIncomplete,
        lease_id: None,
    };
    let document = Observations {
        schema: OBSERVATIONS_SCHEMA.to_owned(),
        observations: vec![incomplete],
    };
    let recorded = recorded_size(&Ok(Some(document.clone())), &"a".repeat(32), &slot, 15_000);
    assert_eq!(recorded, SlotSize::NotMeasured(INCOMPLETE_WALK.to_owned()));
    assert_eq!(recorded.describe(), "not measured (walk incomplete)");
    assert!(
        !recorded.describe().contains("bytes") && !recorded.describe().contains('B'),
        "an incomplete walk claimed a size"
    );
    // Another generation, directory or store is not this slot's fact.
    for other in [
        ledger_slot("1", "8", Some("oxide-abcd/quick-1")),
        ledger_slot("1", "7", Some("oxide-abcd/quick-2")),
        ledger_slot("2", "7", Some("oxide-abcd/quick-1")),
    ] {
        assert!(
            matches!(
                recorded_size(&Ok(Some(document.clone())), &"a".repeat(32), &other, 15_000),
                SlotSize::NotMeasured(ref reason)
                    if reason.starts_with("not measured (no recorded observation")
            ),
            "another generation borrowed the incomplete fact"
        );
    }
}

/// The incomplete fact is written from the admission's own report, bound to
/// the walked generation, and a later reuse measurement of the same
/// generation replaces it (oxide#186).
#[test]
fn recording_an_incomplete_walk_says_not_measured_until_a_complete_walk() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-incomplete-walk-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store {
        control: root.clone(),
        payload: root.join("payload"),
        pool: root.clone(),
        backend: Backend::DirectoryBudget,
        budget: None,
        recorded_budget: 1 << 30,
        max_active: 1,
        sibling: None,
    };
    let slot = ledger_slot("1", "1", Some("oxide-abcd/quick-1"));
    let store_id = "a".repeat(32);
    store.record_incomplete_walks(
        Some(&store_id),
        &[UnmeasuredSlot {
            slot_id: slot.slot_id.clone(),
            incarnation: slot.incarnation.clone(),
            dir: slot.dir.clone(),
        }],
    );
    let document = read_observations_for_tests(&root).unwrap().unwrap();
    assert_eq!(document.len(), 1);
    assert_eq!(
        document[0].source,
        ObservationSource::AdmissionWalkIncomplete
    );
    assert_eq!(document[0].allocated_bytes, 0, "a walk wrote a size");
    assert_eq!(document[0].slot_id, slot.slot_id);
    assert_eq!(document[0].incarnation, slot.incarnation);
    assert_eq!(document[0].dir, slot.dir);
    assert_eq!(document[0].lease_id, None);
    let recorded = recorded_size(&read_observations(&root), &store_id, &slot, 5);
    assert_eq!(recorded.describe(), "not measured (walk incomplete)");
    // A complete walk of the same generation records it normally.
    store
        .record_observation(observation(&slot, 20))
        .expect("record the complete walk");
    let recorded = recorded_size(&read_observations(&root), &store_id, &slot, 25);
    assert!(
        matches!(recorded, SlotSize::Recorded { .. }),
        "a complete walk did not supersede the incomplete fact: {recorded:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A recorded fact is bound to exactly one slot generation: a different
/// incarnation or recorded directory is not a size, and a future clock
/// is an unknown age rather than a fresh observation.
#[test]
fn recorded_sizes_bind_to_one_slot_generation() {
    let slot = ledger_slot("1", "7", Some("oxide-abcd/quick-1"));
    let document = Observations {
        schema: OBSERVATIONS_SCHEMA.to_owned(),
        observations: vec![observation(&slot, 10_000)],
    };
    let recorded = recorded_size(&Ok(Some(document)), &"a".repeat(32), &slot, 15_000);
    assert_eq!(
        recorded,
        SlotSize::Recorded {
            allocated_bytes: 3 * 1024 * 1024,
            age: RecordedAge::Ago(5),
            source: ObservationSource::LeaseReuse,
            lease_id: Some("v1.a.1.1.1".to_owned()),
        }
    );
    // A different incarnation, directory or store identity is a
    // different slot: not measured, never a stale size.
    for other in [
        ledger_slot("1", "8", Some("oxide-abcd/quick-1")),
        ledger_slot("1", "7", Some("oxide-abcd/quick-2")),
        ledger_slot("1", "7", None),
        ledger_slot("2", "7", Some("oxide-abcd/quick-1")),
    ] {
        assert!(
            matches!(
                recorded_size(
                    &Ok(Some(Observations {
                        schema: OBSERVATIONS_SCHEMA.to_owned(),
                        observations: vec![observation(&slot, 10_000)],
                    })),
                    &"a".repeat(32),
                    &other,
                    15_000
                ),
                SlotSize::NotMeasured(_)
            ),
            "a different slot generation must not inherit a recorded size"
        );
    }
    assert!(matches!(
        recorded_size(&Ok(None), &"a".repeat(32), &slot, 15_000),
        SlotSize::NotMeasured(_)
    ));
    assert!(matches!(
        recorded_size(&Err("corrupt".to_owned()), &"a".repeat(32), &slot, 15_000),
        SlotSize::NotMeasured(_)
    ));
    let skew = recorded_size(
        &Ok(Some(Observations {
            schema: OBSERVATIONS_SCHEMA.to_owned(),
            observations: vec![observation(&slot, 20_000)],
        })),
        &"a".repeat(32),
        &slot,
        15_000,
    );
    assert!(matches!(
        skew,
        SlotSize::Recorded {
            age: RecordedAge::ClockSkew,
            ..
        }
    ));
}

/// Retention keeps at most [`MAX_OBSERVATIONS`] observations,
/// deterministically choosing the most recent, so a pool at its slot
/// ceiling still fits.
#[test]
fn observation_retention_keeps_the_newest_deterministically() {
    let count = MAX_OBSERVATIONS + 12;
    let mut observations: Vec<Observation> = (0..count)
        .map(|index| {
            let slot = ledger_slot(&format!("{index}"), "1", None);
            observation(&slot, 1_000 + index as u64)
        })
        .collect();
    retain_newest(&mut observations);
    assert_eq!(observations.len(), MAX_OBSERVATIONS);
    assert_eq!(
        observations.first().map(|item| item.slot_id.as_str()),
        Some("12")
    );
    let newest = (count - 1).to_string();
    assert_eq!(
        observations.last().map(|item| item.slot_id.as_str()),
        Some(newest.as_str())
    );
    let mut again: Vec<Observation> = (0..count)
        .rev()
        .map(|index| {
            let slot = ledger_slot(&format!("{index}"), "1", None);
            observation(&slot, 1_000 + index as u64)
        })
        .collect();
    retain_newest(&mut again);
    assert_eq!(observations, again, "retention is order independent");
    assert!(MAX_OBSERVATIONS >= oxide_storage::lease::MAX_SLOTS);
}

/// The sidecar is read strictly: unknown schema, a foreign or unreadable
/// document and an over-retention file all refuse, which the view reports
/// as not measured instead of scanning.
#[test]
fn observations_read_strictly_and_never_scan_as_fallback() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-observations-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join(OBSERVATIONS_NAME);
    assert!(read_observations(&root).unwrap().is_none());

    let slot = ledger_slot("1", "1", None);
    let good = Observations {
        schema: OBSERVATIONS_SCHEMA.to_owned(),
        observations: vec![observation(&slot, 5)],
    };
    write_private(&path, &serde_json::to_vec(&good).unwrap());
    assert_eq!(
        read_observations_for_tests(&root)
            .unwrap()
            .map(|items| items.len()),
        Some(1)
    );

    write_private(
        &path,
        br#"{"schema":"oxide.status-observations/2","observations":[]}"#,
    );
    let error = read_observations(&root).unwrap_err();
    assert!(error.contains("schema"), "{error}");

    write_private(&path, b"not json");
    assert!(read_observations(&root).unwrap_err().contains("invalid"));

    let over = Observations {
        schema: OBSERVATIONS_SCHEMA.to_owned(),
        observations: (0..=MAX_OBSERVATIONS)
            .map(|index| {
                let slot = ledger_slot(&format!("{index}"), "1", None);
                observation(&slot, index as u64)
            })
            .collect(),
    };
    write_private(&path, &serde_json::to_vec(&over).unwrap());
    assert!(read_observations(&root)
        .unwrap_err()
        .contains("retention bound"));

    // A symlink or a world-readable file is foreign, never followed or
    // trusted.
    fs::remove_file(&path).unwrap();
    let target = root.join("elsewhere.json");
    write_private(&target, &serde_json::to_vec(&good).unwrap());
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(read_observations(&root).is_err());
    fs::remove_file(&path).unwrap();
    write_private(&path, &serde_json::to_vec(&good).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_observations(&root).is_err());
    fs::remove_dir_all(root).unwrap();
}

/// Recording replaces one slot generation's fact, keeps the newest
/// bounded set, and never rewrites a document it cannot read.
#[test]
fn recording_observations_is_bounded_and_never_repairs_a_foreign_file() {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-observation-write-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store {
        control: root.clone(),
        payload: root.join("payload"),
        pool: root.clone(),
        backend: Backend::DirectoryBudget,
        budget: None,
        recorded_budget: 1 << 30,
        max_active: 1,
        sibling: None,
    };
    let slot = ledger_slot("1", "1", Some("oxide-abcd/quick-1"));
    store
        .record_observation(observation(&slot, 10))
        .expect("record");
    let path = root.join(OBSERVATIONS_NAME);
    let first = fs::read(&path).unwrap();
    assert_eq!(
        read_observations_for_tests(&root)
            .unwrap()
            .map(|items| items.len()),
        Some(1)
    );
    // The same generation is replaced, not duplicated, and the write is
    // byte-stable for the same fact.
    store
        .record_observation(observation(&slot, 10))
        .expect("record");
    assert_eq!(fs::read(&path).unwrap(), first);
    let newer = observation(&slot, 20);
    store.record_observation(newer).expect("record");
    let document = read_observations_for_tests(&root).unwrap().unwrap();
    assert_eq!(document.len(), 1);
    assert_eq!(document[0].measured_at_ms, 20);

    // A file this version cannot read is left exactly as it is.
    write_private(&path, b"foreign");
    let foreign = fs::read(&path).unwrap();
    assert!(store.record_observation(observation(&slot, 30)).is_err());
    assert_eq!(fs::read(&path).unwrap(), foreign);

    // The worst case the file allows still fits its byte bound: every
    // retained observation with every bounded field at its ceiling.
    let mut document = Observations {
        schema: OBSERVATIONS_SCHEMA.to_owned(),
        observations: Vec::new(),
    };
    for index in 0..MAX_OBSERVATIONS {
        document.observations.push(Observation {
            store_id: "a".repeat(64),
            slot_id: format!("{:0>160}", index),
            incarnation: "1".repeat(64),
            dir: Some("d".repeat(200)),
            measured_at_ms: 9_999_999_999_999,
            allocated_bytes: u64::MAX,
            source: ObservationSource::LeaseReuse,
            lease_id: Some("l".repeat(160)),
        });
    }
    let bytes = serde_json::to_vec(&document).unwrap();
    assert!(
        bytes.len() as u64 <= MAX_OBSERVATIONS_BYTES,
        "{} > {}",
        bytes.len(),
        MAX_OBSERVATIONS_BYTES
    );
    fs::remove_dir_all(root).unwrap();
}

/// Write a raw control document the way the store does: owner-only, so
/// the private-file checks in `load` are exercised rather than bypassed.
fn write_raw_state(store: &Store, bytes: &[u8]) {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let path = store.control.join("state.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

fn lease_fixture(label: &str) -> (PathBuf, Store, SlotKey) {
    lease_fixture_with_budget(label, 1 << 30)
}

fn lease_fixture_with_budget(label: &str, budget: u64) -> (PathBuf, Store, SlotKey) {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!(
        "oxide-lease-{label}-{}-{}",
        std::process::id(),
        process::now_ms()
    ));
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(&root).unwrap();
    let pool = root.join("pool");
    init_budget_pool(&pool, budget).unwrap();
    let store = Store {
        control: pool.join(MANAGED_NAME),
        payload: pool.join("payload"),
        pool: pool.clone(),
        backend: Backend::DirectoryBudget,
        budget: Some(budget),
        recorded_budget: budget,
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
    (root, store, key)
}

#[test]
fn a_legacy_match_past_the_first_batch_refuses_and_the_next_command_reuses_it() {
    let (root, store, key) = lease_fixture("pending-legacy-adoption");
    let lease = lease(&store, &key, None).unwrap();
    let slot_id = lease.slot_id.clone();
    let affinity = store.affinity_dir(lease.record);
    store.release(lease, None).unwrap();

    let ledger_path = store.pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let store_id = ledger["store_id"].as_str().unwrap().to_owned();
    oxide_storage::lease::managed::evict_idle_slot(&store.pool, &slot_id).unwrap();

    let slots: Vec<_> = (1..=9)
        .map(|id| {
            let path = store.payload.join("slots").join(id.to_string());
            fs::create_dir_all(path.join("build")).unwrap();
            fs::write(path.join("build/artifact"), b"legacy").unwrap();
            let project = if id == 9 { &affinity } else { &key.checkout };
            serde_json::json!({
                "id": id, "incarnation": 1, "state": "idle",
                "project": project, "workspace": key.checkout, "profile": "dev",
                "holder": format!("oxide-{}-1", libc::pid_t::MAX),
                "lease_id": null, "last_lease_id": format!("v1.{store_id}.{id}.1.1"),
                "rank": id,
            })
        })
        .collect();
    ledger["slots"] = slots.into();
    ledger["next_slot"] = 10.into();
    ledger["next_rank"] = 10.into();
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

    prepare_pool(&store.pool).unwrap();
    let path = store.payload.join("slots/9");
    assert!(!path.join(oxide_storage::storage::ROOT_MARKER).exists());
    let (ticket, _lock) = store.register_ticket().unwrap();
    let attempt = store.attempt(ticket, &key, Profile::Quick, 16, false, None);
    let refusal = attempt
        .err()
        .expect("pending adoption finishes this attempt without entering the wait loop");
    assert!(
        refusal.contains("compatible legacy slot 9 is pending adoption"),
        "{refusal}"
    );
    assert!(refusal.contains("retry the Oxide command"), "{refusal}");
    assert!(!path.join(oxide_storage::storage::ROOT_MARKER).exists());
    store.remove_ticket(ticket).unwrap();

    prepare_pool(&store.pool).unwrap();
    let reused = match lease_as(&store, &key, false) {
        Ok(reused) => reused,
        Err(wait) => panic!("the next command reuses the adopted row: {wait:?}"),
    };
    assert!(reused.reused);
    assert_eq!(reused.slot_id, "9");
    assert_eq!(
        fs::read(reused.build_dir.join("artifact")).unwrap(),
        b"legacy"
    );
    store.release(reused, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_ceiling_wait_keeps_the_held_and_pinned_diagnosis() {
    use oxide_storage::lease::{MAX_SLOTS, RAISED_CAPACITY_POLICY_VERSION};
    use oxide_storage::storage::{PoolBacking, PoolStorage};

    let (root, store, key) = lease_fixture("pinned-ceiling-wait");
    let held = lease(&store, &key, None).unwrap();
    let storage = PoolStorage::open(
        &store.control,
        &PoolBacking::budget(&store.pool, store.recorded_budget),
    )
    .unwrap();
    let ledger_path = store.pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let store_id = ledger["store_id"].as_str().unwrap().to_owned();
    let slots = ledger["slots"].as_array_mut().unwrap();
    for id in 2..=MAX_SLOTS {
        let pinned = storage.create("pinned").unwrap();
        storage.pin_in(None, &pinned.id, "keep evidence").unwrap();
        slots.push(serde_json::json!({
            "id": id, "incarnation": 1, "state": "idle",
            "project": key.checkout, "workspace": key.checkout, "profile": "dev",
            "holder": format!("oxide-{}-{id}", std::process::id()), "lease_id": null, "last_lease_id": format!("v1.{store_id}.{id}.1.1"),
            "rank": id, "root": pinned.id, "dir": pinned.folder, "owned": true,
        }));
    }
    ledger["next_slot"] = (MAX_SLOTS + 1).into();
    ledger["next_rank"] = (MAX_SLOTS + 1).into();
    ledger["capacity_policy"] = RAISED_CAPACITY_POLICY_VERSION.into();
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

    let wait = match lease_as(&store, &key, false) {
        Err(wait @ Wait::Capacity(Some(_))) => wait,
        Err(other) => panic!("expected the detailed ceiling wait, got {other:?}"),
        Ok(_) => panic!("admitted past the ceiling"),
    };
    let message = wait.describe(16);
    assert!(message.contains("1 held, 255 pinned"), "{message}");
    assert!(message.contains(r#"255 "keep evidence""#), "{message}");
    let mut waits = WaitAccounting::default();
    waits.observe(&Wait::Capacity(None), 16, Instant::now());
    waits.suppression_announced = true;
    waits.observe(&wait, 16, Instant::now());
    assert!(
        waits.ceiling_announced,
        "the first diagnosis bypasses generic wait suppression"
    );

    let held_slot = held.slot_id.clone();
    store.release(held, None).unwrap();
    let reused = match lease_as(&store, &key, false) {
        Ok(reused) => reused,
        Err(wait) => panic!("the released slot makes progress at the ceiling: {wait:?}"),
    };
    assert!(reused.reused);
    assert_eq!(reused.slot_id, held_slot);
    store.release(reused, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn lease_as(store: &Store, key: &SlotKey, share: bool) -> Result<Lease, Wait> {
    let (ticket, _file) = store.register_ticket().unwrap();
    let outcome = store
        .attempt(ticket, key, Profile::Quick, 16, share, None)
        .unwrap();
    store.remove_ticket(ticket).unwrap();
    outcome
}

#[test]
fn warm_reuse_has_no_measurement_sidecar() {
    let (root, store, key) = lease_fixture("warm-reuse-size");
    let first = lease(&store, &key, None).unwrap();
    let slot = first.slot_id.clone();
    store.release(first, None).unwrap();
    assert!(read_observations(&store.control).unwrap().is_none());
    let warm = lease(&store, &key, None).unwrap();
    assert_eq!(warm.slot_id, slot);
    assert!(warm.reused);
    assert!(warm.measured.is_none());
    assert_eq!(
        crate::leased_size(warm.measured.as_ref(), warm.reused),
        "slot size unknown"
    );
    assert!(read_observations(&store.control).unwrap().is_none());
    let ledger = store.ledger().unwrap();
    assert!(ledger.slots.iter().any(|row| row.slot_id == slot
        && row.state == SlotOccupancy::Held
        && row.recorded.is_none()));
    store.release(warm, None).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Fence first (oxide#177; Supervisor ruling 4c9984a1): the first mutating
/// command makes version 8 durable at the admission gate, before any locked
/// pool open or ledger write, for every pool — no slot-count or ledger-size
/// condition. An already-version-8 control is not rewritten, and a later
/// ordinary save keeps the version.
#[test]
fn an_admission_fences_the_control_state_to_version_eight() {
    let (root, store, _key) = lease_fixture("fence-first-gate");
    store.admission().unwrap();
    let state = store.load().unwrap();
    assert_eq!(state.version, STATE_VERSION);
    assert_eq!(state.slot_ceiling.as_deref(), Some(SLOT_CEILING_FENCE));
    assert_eq!(
        state.recorded_accounting.as_deref(),
        Some(RECORDED_ACCOUNTING_FENCE)
    );

    // A second gate finds version 8 already durable: no write at all.
    let bytes = fs::read(store.control.join("state.json")).unwrap();
    store.admission().unwrap();
    assert_eq!(
        fs::read(store.control.join("state.json")).unwrap(),
        bytes,
        "an already-version-8 control was rewritten"
    );

    // Forward-only: a later ordinary save keeps the version.
    store.save(&state).unwrap();
    let again = store.load().unwrap();
    assert_eq!(again.version, STATE_VERSION);
    assert_eq!(again.slot_ceiling.as_deref(), Some(SLOT_CEILING_FENCE));
    fs::remove_dir_all(root).unwrap();
}

/// A pool with one idle slot made `Evicting` without a journal, its
/// recorded slot root renamed aside so the recovery preflight finds it
/// absent: the next locked open keeps the record and starts no journal,
/// and renaming it back lets the following open journal and conclude. The
/// fixture carries a version-5 control state (the recorded-accounting fence
/// without the raised fence). Nothing in it is unreadable, so a scratch
/// measurement can always walk it. Returns the fixture
/// root, the store, the key, the recorded slot root and the hidden path.
fn pending_deletion_fixture(label: &str) -> (PathBuf, Store, SlotKey, PathBuf, PathBuf) {
    let (root, store, key) = lease_fixture(label);
    let held = lease(&store, &key, None).expect("a slot");
    let build = held.build_dir.clone();
    store.release(held, None).expect("idle slot");

    // Make the recorded slot Evicting with no journal.
    let ledger_path = store.pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let slot = ledger["slots"]
        .as_array_mut()
        .expect("slots")
        .iter_mut()
        .next()
        .expect("one recorded slot");
    slot["state"] = serde_json::json!("evicting");
    slot["lease_id"] = serde_json::json!(null);
    slot.as_object_mut().expect("object").remove("recorded");
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

    // Move the recorded root aside: the preflight then finds it absent,
    // which keeps the `Evicting` record and starts no journal.
    let slot_root = build.parent().expect("the slot root").to_path_buf();
    let hidden = slot_root.with_file_name(format!(
        "{}.pending",
        slot_root
            .file_name()
            .expect("the slot leaf")
            .to_string_lossy()
    ));
    fs::rename(&slot_root, &hidden).expect("hide the pending slot root");

    downgrade_control_to_recorded_accounting(&store);
    (root, store, key, slot_root, hidden)
}

/// Rewrite a pool's control state as version 5 with only the
/// recorded-accounting fence, the shape a predecessor leaves behind.
fn downgrade_control_to_recorded_accounting(store: &Store) {
    let state_path = store.control.join("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state["version"] = serde_json::json!(RECORDED_ACCOUNTING_STATE_VERSION);
    state["recorded_accounting"] = serde_json::json!(RECORDED_ACCOUNTING_FENCE);
    for field in [
        "slot_ceiling",
        "eviction_accounting",
        "one_use_workspaces",
        "holding",
    ] {
        state.as_object_mut().expect("object").remove(field);
    }
    write_raw_state(store, &serde_json::to_vec(&state).unwrap());
}

/// A pool with one idle slot made `Evicting` (no journal) whose root is
/// ready for the recovery preflight, and a version-5 control state.
/// Returns the fixture root, the store, the key and the ledger path.
fn ready_pending_deletion_fixture(label: &str) -> (PathBuf, Store, SlotKey, PathBuf) {
    let (root, store, key) = lease_fixture(label);
    let held = lease(&store, &key, None).expect("a slot");
    store.release(held, None).expect("idle slot");
    let ledger_path = store.pool.join("ledger.json");
    let mut ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let slot = ledger["slots"]
        .as_array_mut()
        .expect("slots")
        .iter_mut()
        .next()
        .expect("one recorded slot");
    slot["state"] = serde_json::json!("evicting");
    slot["lease_id"] = serde_json::json!(null);
    fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    downgrade_control_to_recorded_accounting(&store);
    (root, store, key, ledger_path)
}
/// A fence-write failure in the real entry refuses before the recovery
/// journal or any ledger write: the bytes stay identical (oxide#177).
#[test]
fn a_failed_real_open_fence_leaves_the_ledger_byte_identical() {
    let (root, store, _key, ledger_path) = ready_pending_deletion_fixture("open-fence-fails");
    let ledger_bytes = fs::read(&ledger_path).unwrap();
    let mode = fs::metadata(&store.control).unwrap().permissions().mode() & 0o777;
    fs::set_permissions(&store.control, fs::Permissions::from_mode(0o500)).unwrap();
    let result = Store::open_removable(&store.pool, store.payload.clone(), 8);
    assert!(result.is_err(), "a failed fence did not refuse the open");
    assert_eq!(
        fs::read(&ledger_path).unwrap(),
        ledger_bytes,
        "the ledger changed although the fence failed"
    );
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    let pending = after["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["id"] == 1)
        .expect("the pending record was dropped");
    assert_eq!(pending["state"], "evicting");
    assert!(
        pending.get("capture").is_none(),
        "a journal started although the fence failed"
    );
    fs::set_permissions(&store.control, fs::Permissions::from_mode(mode)).unwrap();
    fs::remove_dir_all(root).unwrap();
}
/// A failed fence write refuses at the gate with the ledger and journals
/// byte-identical: the fence is the command's first durable effect.
#[test]
fn a_failed_fence_write_leaves_the_ledger_and_journals_byte_identical() {
    let (root, store, _key, _slot_root, hidden) = pending_deletion_fixture("fence-write-failure");
    let ledger_path = store.pool.join("ledger.json");
    let state_path = store.control.join("state.json");
    let ledger_bytes = fs::read(&ledger_path).unwrap();
    let state_bytes = fs::read(&state_path).unwrap();
    // Force the atomic state write to fail: a read-only control directory
    // refuses the exclusive temp file it must create.
    let mode = fs::metadata(&store.control).unwrap().permissions().mode() & 0o777;
    fs::set_permissions(&store.control, fs::Permissions::from_mode(0o500)).unwrap();

    let outcome = store.admission();
    fs::set_permissions(&store.control, fs::Permissions::from_mode(mode)).unwrap();
    assert!(outcome.is_err(), "a failed fence write did not refuse");
    assert_eq!(
        fs::read(&ledger_path).unwrap(),
        ledger_bytes,
        "a failed fence write changed the ledger"
    );
    assert_eq!(
        fs::read(&state_path).unwrap(),
        state_bytes,
        "a failed fence write changed the control state"
    );
    assert!(
        hidden.is_dir(),
        "a failed fence write touched the pending slot"
    );
    let ledger: serde_json::Value = serde_json::from_slice(&ledger_bytes).unwrap();
    assert!(
        ledger["slots"][0].get("capture").is_none(),
        "a failed fence write started a recovery journal"
    );
    fs::remove_dir_all(root).unwrap();
}

/// A version-8 control is newer than the one-use and eviction-accounting
/// fences a predecessor knows, so a binary that only reads up to version 7
/// refuses it before reading the ledger; this binary refuses an unknown
/// version by the same rule (oxide#122, oxide#175, oxide#177).
#[test]
fn a_version_eight_control_is_newer_than_a_predecessor_knows() {
    assert!(
        STATE_VERSION > EVICTION_ACCOUNTING_STATE_VERSION && STATE_VERSION > ONE_USE_STATE_VERSION,
        "a predecessor that knows version 7 or 6 must see the current version as newer"
    );
    let (root, store, _key) = lease_fixture("v7-predecessor");
    store.admission().unwrap();
    let bytes = fs::read(store.control.join("state.json")).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["version"], serde_json::json!(STATE_VERSION));
    assert_eq!(
        document["slot_ceiling"],
        serde_json::json!(SLOT_CEILING_FENCE)
    );
    assert_eq!(document["holding"], serde_json::json!(HOLDING_FENCE));

    // A predecessor knows at most version 7, so it refuses this real
    // version-8 document before reading the ledger. The model is local
    // because the test build has no older binary; it mirrors the one rule
    // such a reader applies (oxide#122).
    assert!(
        predecessor_accepts_control(&bytes).is_err(),
        "a predecessor accepted a current-version control"
    );
    // This binary applies the same rule to a version it does not know.
    let mut newer = document;
    newer["version"] = serde_json::json!(STATE_VERSION + 1);
    write_raw_state(&store, &serde_json::to_vec(&newer).unwrap());
    let error = store.load().unwrap_err();
    assert!(error.contains("unsupported version"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

/// Stand-in for an installed predecessor's strict control reader: it
/// accepts the versions it knows and refuses anything newer before reading
/// the ledger (oxide#122).
fn predecessor_accepts_control(bytes: &[u8]) -> Result<(), String> {
    let document: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("unparseable control state: {error}"))?;
    let version = document
        .get("version")
        .and_then(|version| version.as_u64())
        .ok_or("the control state has no numeric version")?;
    if version > u64::from(EVICTION_ACCOUNTING_STATE_VERSION) {
        return Err("Oxide state has an unsupported version".to_owned());
    }
    Ok(())
}

/// The fence's parser and the front door's `Store::load` are the same
/// shared function (`oxide_storage::lease::control::parse_state`), so they
/// must agree on every document the fence is asked to inspect.
#[test]
fn the_fence_parser_and_store_load_agree_on_control_documents() {
    use serde_json::json;
    let (root, store, key) = lease_fixture("control-parity");
    let held = lease(&store, &key, None).expect("a slot");
    store.release(held, None).expect("idle slot");
    let path = store.control.join("state.json");
    let valid: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Each invalid case is a complete document with only the named field
    // wrong, so it cannot be rejected for a missing scalar first.
    let complete_v8 = json!({
        "version": 8,
        "next_id": 1,
        "next_use": 1,
        "tickets": [],
        "records": [],
        "cleans": [],
        "recorded_accounting": "oxide.accounting/1",
        "eviction_accounting": "oxide.eviction-accounting/1",
        "slot_ceiling": "oxide.slots/256",
    });
    let mut malformed = complete_v8.clone();
    malformed["records"] = json!(42);
    let unsupported = json!({
        "version": 0,
        "next_id": 1,
        "next_use": 1,
        "tickets": [],
        "records": [],
        "cleans": [],
    });
    let missing_one_use = json!({
        "version": 6,
        "next_id": 1,
        "next_use": 1,
        "tickets": [],
        "records": [],
        "cleans": [],
        "recorded_accounting": "oxide.accounting/1",
    });
    let mut non_string = complete_v8.clone();
    non_string["slot_ceiling"] = json!(42);
    let mut null_fence = complete_v8.clone();
    null_fence["slot_ceiling"] = serde_json::Value::Null;
    for (label, document) in [
        ("valid", valid),
        ("malformed records", malformed),
        ("unsupported version", unsupported),
        ("version 6 without one use", missing_one_use),
        ("non-string ceiling", non_string),
        ("null ceiling", null_fence),
    ] {
        let bytes = serde_json::to_vec(&document).unwrap();
        fs::write(&path, &bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let shared = oxide_storage::lease::control::parse_state(&bytes);
        let front_door = store.load();
        assert_eq!(
            shared.is_ok(),
            front_door.is_ok(),
            "{label}: the fence and Store::load disagree ({shared:?} vs {front_door:?})"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

/// A version-8 document always carries the raised-ceiling fence and a
/// smaller document never does; an unknown fence value and a version-8
/// document without the fence are states no writer produces. Version 8
/// carries the eviction-accounting fence too and may carry the one-use
/// fence, so a one-use pool at the raised ceiling keeps all three.
/// Version 9 adds the whole-tree holding fence (oxide#190) on the same
/// terms: a version-9 document always carries it, and a smaller pool never
/// does, so an older binary hands off before it reads a holding row.
#[test]
fn the_raised_slot_ceiling_fence_and_version_are_equivalent() {
    let mut state = State::default();
    state.recorded_accounting = Some(RECORDED_ACCOUNTING_FENCE.to_owned());
    state.eviction_accounting = Some(EVICTION_ACCOUNTING_FENCE.to_owned());

    state.version = HOLDING_STATE_VERSION;
    assert!(
        validate_state(&state, false).is_err(),
        "the current version without the raised fence was accepted"
    );
    state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());
    assert!(
        validate_state(&state, false).is_err(),
        "the current version without the holding fence was accepted"
    );
    state.holding = Some(HOLDING_FENCE.to_owned());
    assert!(validate_state(&state, false).is_ok());

    // Version 8 is still exactly the raised-ceiling shape: it carries that
    // fence and neither the holding fence nor a holding row.
    state.version = SLOT_CEILING_STATE_VERSION;
    state.holding = None;
    assert!(validate_state(&state, false).is_ok());
    assert!(
        validate_state(
            &State {
                holding: Some(HOLDING_FENCE.to_owned()),
                ..state.clone()
            },
            false
        )
        .is_err(),
        "the holding fence at version 8 was accepted"
    );
    state.holding = Some(HOLDING_FENCE.to_owned());
    state.version = HOLDING_STATE_VERSION;
    state.slot_ceiling = None;
    assert!(
        validate_state(&state, false).is_err(),
        "the holding version without the raised fence was accepted"
    );
    state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());

    // Version 7 is the eviction-accounting shape without the raised fence.
    state.version = EVICTION_ACCOUNTING_STATE_VERSION;
    state.slot_ceiling = None;
    state.holding = None;
    assert!(validate_state(&state, false).is_ok());
    assert!(
        validate_state(
            &State {
                slot_ceiling: Some(SLOT_CEILING_FENCE.to_owned()),
                ..state.clone()
            },
            false
        )
        .is_err(),
        "the raised fence at version 7 was accepted"
    );

    // Version 6 requires the one-use fence and carries neither later one.
    state.version = ONE_USE_STATE_VERSION;
    state.eviction_accounting = None;
    assert!(
        validate_state(&state, false).is_err(),
        "version 6 without the one-use fence was accepted"
    );
    state.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
    assert!(
        validate_state(&state, false).is_ok(),
        "the one-use version must stay readable without the raised fence"
    );
    state.one_use_workspaces = None;

    // An unknown raised-ceiling fence, or a version-8 document without
    // it, is a state no writer produces.
    state.version = HOLDING_STATE_VERSION;
    state.eviction_accounting = Some(EVICTION_ACCOUNTING_FENCE.to_owned());
    state.slot_ceiling = Some("oxide.slots/128".to_owned());
    assert!(
        validate_state(&state, false).is_err(),
        "an unknown raised-ceiling fence was accepted"
    );

    state.slot_ceiling = Some(SLOT_CEILING_FENCE.to_owned());
    state.holding = Some(HOLDING_FENCE.to_owned());
    state.one_use_workspaces = Some(ONE_USE_FENCE.to_owned());
    assert!(
        validate_state(&state, false).is_ok(),
        "a one-use pool at the raised ceiling must keep every fence"
    );
}

/// A refused count replacement never becomes a thirteenth project slot.
/// At `PROJECT_SLOT_CAP` the front door asks the library to retire one
/// named idle slot; when the library refuses that candidate (here because
/// its recorded root no longer resolves) the command fails with the
/// bounded reason and the pool keeps exactly its capped slot count. The
/// library-side refusal for a slot whose own accounting walk was partial
/// (`a_named_count_replacement_refuses_rather_than_exceeding_the_project_
/// cap`) reaches the same front-door path.
#[test]
fn a_refused_count_replacement_never_adds_a_thirteenth_project_slot() {
    let (root, store, key) = lease_fixture("cap-refused-replacement");
    let mut normal = Vec::new();
    while normal.len() < PROJECT_SLOT_CAP {
        normal.push(lease_as(&store, &key, false).expect("normal"));
    }
    // Free one record and its lock so it becomes the cap's replacement
    // candidate, then make the recorded slot root unresolvable.
    let victim = normal.pop().expect("a released slot");
    let victim_slot = victim.slot_id.clone();
    store.release(victim, None).unwrap();
    let view = store.ledger().unwrap();
    let dir = view
        .slots
        .iter()
        .find(|slot| slot.slot_id == victim_slot)
        .and_then(|slot| slot.dir.clone())
        .expect("the victim's recorded directory");
    let slot_root = store.payload.join("slots").join(&dir);
    fs::remove_dir_all(&slot_root).unwrap();
    fs::write(&slot_root, b"not a directory").unwrap();

    let (ticket, _file) = store.register_ticket().unwrap();
    let outcome = store.attempt(ticket, &key, Profile::Quick, 16, false, None);
    store.remove_ticket(ticket).unwrap();
    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("a refused replacement admitted or waited instead"),
    };
    assert!(
        error.contains("cannot lease a slot"),
        "the refusal is not the bounded capacity shape: {error}"
    );
    let view = store.ledger().unwrap();
    assert_eq!(
        view.slots.len(),
        PROJECT_SLOT_CAP,
        "the refused replacement still added a slot"
    );
    assert!(view.slots.iter().any(|slot| slot.slot_id == victim_slot));
    for held in normal {
        drop(held);
    }
    fs::remove_dir_all(root).unwrap();
}
