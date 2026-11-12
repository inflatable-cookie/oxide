//! Threads and a libc FFI path of the kind Oxide itself uses (the same exact
//! libc pin as this repository). Four worker threads each push their share;
//! the main thread joins and prints the exact sum plus the nonzero pid.
//! Worker w contributes 0+1+...+w, so the joined sum is 0+1+3+6 = 10.

use std::thread;

fn main() {
    let handles: Vec<_> = (0..4)
        .map(|worker| {
            thread::spawn(move || {
                let mut share = 0u64;
                for step in 0..=(worker as u64) {
                    share += step;
                }
                share
            })
        })
        .collect();
    let sum: u64 = handles.into_iter().map(|h| h.join().expect("join")).sum();
    let pid = unsafe { libc::getpid() };
    println!("sum={sum} pid={pid}");
    if sum != 10 {
        eprintln!("wrong sum {sum}");
        std::process::exit(1);
    }
    if pid <= 0 {
        eprintln!("wrong pid {pid}");
        std::process::exit(1);
    }
}
