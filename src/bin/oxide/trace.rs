//! Opt-in phase timing. `OXIDE_TRACE_TIMING=1` prints one stderr line per
//! phase: its fixed name, its start on the monotonic clock since Oxide
//! started and its duration, both in microseconds. Off by default. Lines
//! carry no arguments, paths or environment values, and at most
//! `MAX_LINES` are printed per command, so one command's trace stays far
//! below 1 MiB. Tracing only observes; it never skips a check or a sync.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Each line is under 96 bytes, so this bounds a trace to about 96 KiB.
const MAX_LINES: usize = 1024;

static ORIGIN: OnceLock<Option<Instant>> = OnceLock::new();
static LINES: AtomicUsize = AtomicUsize::new(0);

/// Read `OXIDE_TRACE_TIMING` once, at the start of `main`.
pub fn init() {
    ORIGIN.get_or_init(|| {
        std::env::var_os("OXIDE_TRACE_TIMING")
            .is_some_and(|value| value == "1")
            .then(Instant::now)
    });
}

fn origin() -> Option<Instant> {
    *ORIGIN.get_or_init(|| None)
}

/// A phase that prints its timing when dropped.
pub struct Span {
    name: &'static str,
    started: Option<Instant>,
}

/// Start timing `name`. Names are `category.phase`; every Cargo driver
/// invocation is one `cargo.*` span.
pub fn span(name: &'static str) -> Span {
    Span {
        name,
        started: origin().map(|_| Instant::now()),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let (Some(origin), Some(started)) = (origin(), self.started) {
            emit(
                self.name,
                started.duration_since(origin).as_micros(),
                started.elapsed().as_micros(),
            );
        }
    }
}

/// A zero-length mark, such as the moment Oxide replaces itself.
pub fn mark(name: &'static str) {
    if let Some(origin) = origin() {
        emit(name, origin.elapsed().as_micros(), 0);
    }
}

fn emit(name: &str, at: u128, took: u128) {
    if LINES.fetch_add(1, Ordering::Relaxed) < MAX_LINES {
        eprintln!("oxide-trace: {name} at={at}us took={took}us");
    }
}
