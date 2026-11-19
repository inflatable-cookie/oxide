//! Opt-in stable-Cargo profile probe (oxide#068).
//!
//! This focused probe runs under `cargo test` in whichever profile compiled it. It asserts, in the profile that actually
//! compiled this test target:
//!
//! 1. a panic inside a workspace helper still unwinds and is delivered to
//!    `catch_unwind` with its original payload, and the thread keeps running;
//! 2. the panic's own backtrace resolves worksheet frames to `file:line`, and
//!    more than one workspace frame keeps a line number -- the panic's initial
//!    location alone is not accepted as proof;
//! 3. `debug_assertions` and `overflow_checks` are still enabled.
//!
//! It does not assert debugger-variable, type or source-retention behavior;
//! `oxide-full-debug` is the explicit control that restores dependency debug
//! information.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};

/// Marker carried by the probe panic; changes here are a contract change.
const PANIC_MARKER: &str = "oxide-profile-probe-panic";

/// File name fragment matched against resolved backtrace frames. Keeping it
/// relative avoids depending on the checkout path.
const PROBE_FILE: &str = "tests/profile_backtrace.rs";

thread_local! {
    static PANIC_BACKTRACE: RefCell<Option<Backtrace>> = const { RefCell::new(None) };
}

#[inline(never)]
fn panic_in_workspace_helper() -> ! {
    panic!("{PANIC_MARKER}");
}

#[inline(never)]
fn call_workspace_helper() -> ! {
    panic_in_workspace_helper();
}

#[inline(never)]
fn unchecked_add(value: u8) -> u8 {
    value + 1
}

#[test]
fn profile_probe_keeps_workspace_frames_and_unwind() {
    // The opt-in profiles keep the dev assertions/overflow behavior; verify
    // the compiled behavior rather than trusting the profile name.
    assert!(cfg!(debug_assertions), "debug_assertions is disabled");
    let overflow = panic::catch_unwind(|| unchecked_add(std::hint::black_box(u8::MAX)));
    assert!(
        overflow.is_err(),
        "u8::MAX + 1 did not panic, so overflow checks are disabled"
    );

    PANIC_BACKTRACE.with(|slot| *slot.borrow_mut() = None);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {
        // Capturing from the hook records the panic's own stack, not only the
        // location the panic macro was written at.
        let captured = Backtrace::force_capture();
        PANIC_BACKTRACE.with(|slot| *slot.borrow_mut() = Some(captured));
    }));
    let caught = panic::catch_unwind(AssertUnwindSafe(call_workspace_helper));
    panic::set_hook(previous);

    // Normal catch_unwind semantics: the panic arrives as an Err payload
    // instead of aborting the process, and this thread continues.
    let payload = caught.expect_err("workspace helper panic did not unwind to catch_unwind");
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("<unexpected payload type>");
    assert_eq!(message, PANIC_MARKER, "panic payload changed");
    assert!(
        panic::catch_unwind(|| 1_u8).is_ok(),
        "the thread did not continue after catching the probe panic"
    );

    let backtrace = PANIC_BACKTRACE
        .with(|slot| slot.borrow_mut().take())
        .expect("the panic hook did not capture a backtrace");
    assert_eq!(
        backtrace.status(),
        BacktraceStatus::Captured,
        "the panic backtrace was not captured"
    );
    let rendered = backtrace.to_string();
    let workspace_frames = rendered
        .lines()
        .filter(|line| {
            line.split(PROBE_FILE).nth(1).is_some_and(|rest| {
                rest.strip_prefix(':')
                    .is_some_and(|digits| digits.chars().next().is_some_and(|c| c.is_ascii_digit()))
            })
        })
        .count();
    assert!(
        workspace_frames >= 2,
        "expected at least two {PROBE_FILE}:line frames, resolved {workspace_frames}:\n{rendered}"
    );
    for frame in [
        "panic_in_workspace_helper",
        "call_workspace_helper",
        "profile_probe_keeps_workspace_frames_and_unwind",
    ] {
        assert!(
            rendered.contains(frame),
            "backtrace is missing the {frame} workspace frame:\n{rendered}"
        );
    }
}
