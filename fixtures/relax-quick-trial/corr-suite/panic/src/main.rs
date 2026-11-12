//! Panic correspondence: `catch_unwind` must catch a panic payload, and an
//! uncaught panic must exit 101 naming this file and the exact line:column
//! of the panic call. The driver passes the mode as argv[1]. The uncaught
//! probe's call below is pinned by scripts/relax-quick-trial.py, whose
//! self-check re-derives that location from these committed bytes.

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
