//! Unpublished private entry point for the bounded read-only storage inventory.
//!
//! The command requires one explicit root; it never defaults to HOME, a
//! workspace, Cargo home or the host. It is a read-only report and grants no
//! ownership, eviction or cleanup authority. See
//! `docs/knowledge/contracts/storage-inventory.md` for the owning contract.
//!
//! Exit 0 reports a complete inventory. Exit 1 refuses without a report
//! (usage, missing root or unusable root). Exit 2 reports an honestly partial
//! inventory because a finite bound, an unreadable entry or the output cap was
//! reached.

use oxide_storage::inventory::{inventory_directory, InventoryLimits};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
oxide-storage-inventory: bounded read-only inventory of one Rust target directory

USAGE:
    oxide-storage-inventory --root <path> [--json] [--max-entries <n>]
        [--max-depth <n>] [--max-output-bytes <n>] [--max-elapsed-ms <n>]

REQUIRED:
    --root <path>            Explicit directory to inventory. There is no default.

OPTIONS:
    --json                   Emit bounded JSON instead of human text.
    --max-entries <n>        Maximum directory entries visited (default 200000).
    --max-depth <n>          Maximum directory depth read (default 64).
    --max-output-bytes <n>   Output cap in bytes (default 65536).
    --max-elapsed-ms <n>     Cooperative traversal deadline in ms (default 30000).
    --help                   Print this message.

EXIT:
    0  complete inventory
    1  refused: usage, missing root or unusable root; no inventory
    2  partial inventory: a bound, unreadable entry or output cap was reached
";

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("oxide-storage-inventory: {message}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<OsString>) -> Result<ExitCode, String> {
    let mut root: Option<PathBuf> = None;
    let mut json = false;
    let mut limits = InventoryLimits::default();
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.to_str() {
            Some("--help") => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            Some("--json") => json = true,
            Some("--root") => {
                if root.is_some() {
                    return Err("--root may be given only once".to_owned());
                }
                root = Some(PathBuf::from(next_value(&mut iter, "--root")?));
            }
            Some("--max-entries") => {
                limits.max_entries = parse_u64(&mut iter, "--max-entries")?;
            }
            Some("--max-depth") => {
                limits.max_depth = parse_usize(&mut iter, "--max-depth")?;
            }
            Some("--max-output-bytes") => {
                limits.max_output_bytes = parse_usize(&mut iter, "--max-output-bytes")?;
            }
            Some("--max-elapsed-ms") => {
                limits.max_elapsed_ms = parse_u64(&mut iter, "--max-elapsed-ms")?;
            }
            _ => {
                return Err(format!(
                    "unrecognized argument: {}",
                    argument.to_string_lossy()
                ));
            }
        }
    }

    let root =
        root.ok_or_else(|| "--root <path> is required; no default root is used".to_owned())?;
    limits.validate().map_err(|error| error.to_string())?;
    let report = inventory_directory(&root, limits).map_err(|error| error.to_string())?;
    let rendered = if json {
        report.render_json().map_err(|error| error.to_string())?
    } else {
        report.render_human()
    };

    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    output
        .write_all(&rendered.bytes)
        .map_err(|error| error.to_string())?;
    if !rendered.bytes.ends_with(b"\n") {
        output.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    output.flush().map_err(|error| error.to_string())?;

    if rendered.truncated || !report.coverage.complete {
        Ok(ExitCode::from(2))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn next_value(iter: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_u64(iter: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<u64, String> {
    let value = next_value(iter, flag)?;
    value
        .to_str()
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or_else(|| format!("{flag} requires a non-negative integer"))
}

fn parse_usize(iter: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<usize, String> {
    let value = next_value(iter, flag)?;
    value
        .to_str()
        .and_then(|text| text.parse::<usize>().ok())
        .ok_or_else(|| format!("{flag} requires a non-negative integer"))
}
