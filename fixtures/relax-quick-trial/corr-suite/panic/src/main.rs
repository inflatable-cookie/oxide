//! Panic correspondence: `catch_unwind` must catch a panic payload, and an
//! uncaught panic must exit 101 naming this file and the exact line:column of
//! the panic call in its message. The driver passes the mode as argv[1].
//!
//! The uncaught probe's panic call sits at file main.rs line 11, column 5.
//! The oracle requires `main.rs:11:5` so the file-name and exact position
//! are pinned without betting on the path prefix spelling rustc prints for
//! a workspace member; the captured stderr prefix keeps the full form.

use std::panic;

fn uncaught_probe() {
    panic!("corr-uncaught-boom");
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "caught" => {
            let result = panic::catch_unwind(|| {
                panic!("corr-caught-boom");
            });
            let payload = result
                .err()
                .and_then(|p| {
                    p.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                })
                .unwrap_or_else(|| "<no payload>".to_string());
            println!("CAUGHT:{payload}");
        }
        "uncaught" => uncaught_probe(),
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(2);
        }
    }
}
