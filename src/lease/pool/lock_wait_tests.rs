//! Pool-lock wait tests: bounds, deadlines and the release-walk limit.

use super::*;
use crate::lease::tests::Fixture;

#[test]
fn a_pool_lock_wait_ends_at_its_bound_and_names_what_it_waited_on() {
    let fixture = Fixture::new("pool-lock-bound");
    let pool = fixture.dir("pool");
    let holder = lock_pool_inner(&pool, true, false).unwrap();
    let started = Instant::now();
    let (deadline, bound) = (
        started + Duration::from_millis(60),
        "the pool-lock bound was reached",
    );
    let refusal = lock_pool_until(&pool, true, false, deadline, bound)
        .map(|_| ())
        .unwrap_err();
    let text = refusal.to_string();
    assert!(
        text.starts_with("gave up waiting for the pool lock after "),
        "{text}"
    );
    assert!(text.contains("the pool-lock bound was reached"), "{text}");
    assert!(text.contains("holder unknown"), "{text}");
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(holder);
    assert!(lock_pool_until(
        &pool,
        true,
        false,
        Instant::now() + Duration::from_secs(5),
        bound
    )
    .is_ok());
}

#[test]
fn the_release_walk_bound_stays_well_below_a_pool_lock_wait() {
    assert!(
        RELEASE_WALK_MAX_ELAPSED_MS * 10 <= POOL_LOCK_BOUND_SECONDS * 1000,
        "a release's one walk must not be able to approach the lock-wait bound"
    );
}

#[test]
fn the_admission_window_bounds_a_pool_lock_wait_when_it_is_nearer() {
    crate::lease::set_admission_deadline(Some(Instant::now() + Duration::from_secs(30)));
    let (deadline, bound) = pool_lock_deadline(Instant::now());
    assert_eq!(bound, "the admission budget expired");
    assert!(deadline <= Instant::now() + Duration::from_secs(30));
    crate::lease::set_admission_deadline(None);
    let (deadline, bound) = pool_lock_deadline(Instant::now());
    assert_eq!(bound, "the pool-lock bound was reached");
    assert!(deadline > Instant::now() + Duration::from_secs(100));
    // A window that has already closed (a release after a long build)
    // does not refuse the wait: it gets the default bound.
    crate::lease::set_admission_deadline(Some(Instant::now() - Duration::from_secs(1)));
    let (deadline, bound) = pool_lock_deadline(Instant::now());
    assert_eq!(bound, "the pool-lock bound was reached");
    assert!(deadline > Instant::now() + Duration::from_secs(100));
    crate::lease::set_admission_deadline(None);
}
