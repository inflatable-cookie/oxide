//! Writes Oxide's own output without changing the process's SIGPIPE behavior.

#[cfg(not(test))]
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static BROKEN_PIPE: AtomicBool = AtomicBool::new(false);

/// Writes one formatted stdout fragment.
#[cfg(not(test))]
pub(crate) fn print(args: std::fmt::Arguments<'_>) {
    write(args, false);
}

/// Writes one formatted stdout line.
#[cfg(not(test))]
pub(crate) fn println(args: std::fmt::Arguments<'_>) {
    write(args, true);
}

/// Returns whether an Oxide stdout write encountered a closed pipe.
pub(crate) fn broken_pipe() -> bool {
    BROKEN_PIPE.load(Ordering::Relaxed)
}

#[cfg(not(test))]
fn write(args: std::fmt::Arguments<'_>, newline: bool) {
    if broken_pipe() {
        return;
    }

    let result = (|| {
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        stdout.write_fmt(args)?;
        if newline {
            stdout.write_all(b"\n")?;
        }
        Ok::<(), io::Error>(())
    })();

    if let Err(error) = result {
        if error.kind() == io::ErrorKind::BrokenPipe {
            BROKEN_PIPE.store(true, Ordering::Relaxed);
        } else {
            panic!("failed printing to stdout: {error}");
        }
    }
}
