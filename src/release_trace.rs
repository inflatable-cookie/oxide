//! Opt-in phase diagnostics for the lease closeout path.

use std::time::{Duration, Instant};

fn line(phase: &str, elapsed: Option<Duration>) -> String {
    match elapsed {
        None => format!("oxide: release trace: {phase} started"),
        Some(elapsed) => format!(
            "oxide: release trace: {phase} finished in {:.3}s",
            elapsed.as_secs_f64()
        ),
    }
}

/// Trace the beginning and end separately so an unfinished operation is visible.
/// The switch is deliberately opt-in and does not change custody or deadlines.
pub fn phase<T>(name: &'static str, action: impl FnOnce() -> T) -> T {
    let enabled =
        std::env::var_os("OXIDE_TRACE_RELEASE").as_deref() == Some(std::ffi::OsStr::new("1"));
    phase_with(name, enabled, |message| eprintln!("{message}"), action)
}

fn phase_with<T>(
    name: &'static str,
    enabled: bool,
    mut emit: impl FnMut(&str),
    action: impl FnOnce() -> T,
) -> T {
    if !enabled {
        return action();
    }
    emit(&line(name, None));
    let started = Instant::now();
    let result = action();
    emit(&line(name, Some(started.elapsed())));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_lines_name_the_started_and_finished_operation() {
        for name in [
            "cargo group closure",
            "run group closure",
            "tool directory notice",
            "program target resolution",
            "freshness finish",
            "seed publication",
            "stale unit pruning",
            "admission lock",
            "temporary directory removal",
            "removable flush",
            "pool open",
            "pool lock",
            "pool reconciliation",
            "release walk",
            "ledger write",
            "origin and control write",
        ] {
            assert_eq!(
                line(name, None),
                format!("oxide: release trace: {name} started")
            );
            assert_eq!(
                line(name, Some(Duration::from_millis(1250))),
                format!("oxide: release trace: {name} finished in 1.250s")
            );
        }
    }
    #[test]
    fn tracing_emits_before_and_after_each_action_and_preserves_its_result() {
        use std::cell::RefCell;
        let events = RefCell::new(Vec::new());
        let result: Result<(), &str> = phase_with(
            "pool lock",
            true,
            |line| events.borrow_mut().push(line.to_owned()),
            || {
                assert_eq!(
                    events.borrow().as_slice(),
                    ["oxide: release trace: pool lock started"]
                );
                Err("unchanged error")
            },
        );
        assert_eq!(result, Err("unchanged error"));
        assert!(events.borrow()[1].starts_with("oxide: release trace: pool lock finished in "));
        events.borrow_mut().clear();
        assert_eq!(
            phase_with(
                "pool lock",
                false,
                |line| events.borrow_mut().push(line.to_owned()),
                || 42
            ),
            42
        );
        assert!(events.borrow().is_empty());
    }
}
