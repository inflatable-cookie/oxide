//! `oxide init` / `oxide new`: Cargo's command when it applies, then one
//! marked guidance block in the workspace-root `AGENTS.md` and a readiness
//! report. No Oxide config file; `Cargo.toml` changes only as Cargo writes it.

use crate::identity::{self, Identity};
use crate::store::{self, Store};
use crate::{CARGO, MIB};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BEGIN: &str = "<!-- oxide:begin -->";
pub const END: &str = "<!-- oxide:end -->";

/// The managed block, markers included. Refreshing replaces it whole.
pub const BLOCK: &str = "\
<!-- oxide:begin -->
## Rust commands: use `oxide`

- Write `oxide` wherever you would write `cargo`: `oxide check`, `oxide test`, `oxide run -- <args>`, `oxide add <crate>`.
- Profiles: `quick` (default), `--profile debug` for a debugger, `--release`. Oxide places build output, so it refuses `--target-dir`.
- A `waiting for …` line means another build holds a slot; let it wait. `oxide status` shows the pool, slots and waits.
- Use `oxide clean`, not `cargo clean`.

Managed by `oxide init`; edits between these markers are replaced.
<!-- oxide:end -->
";

/// `cargo init`/`cargo new` options that take a value.
const VALUE_OPTIONS: [&str; 7] = [
    "--vcs",
    "--edition",
    "--name",
    "--registry",
    "--color",
    "--config",
    "-Z",
];
/// Bounded read of an `effigy.toml`; larger files are reported, not scanned.
const EFFIGY_LIMIT: u64 = 1024 * 1024;
const AGENTS: &str = "AGENTS.md";

/// The directory argument and every other argument of `cargo init`/`new`.
struct Parsed {
    path: Option<OsString>,
    options: Vec<OsString>,
    help: bool,
}

fn parse(args: &[OsString]) -> Parsed {
    let mut parsed = Parsed {
        path: None,
        options: Vec::new(),
        help: false,
    };
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        let text = argument.to_string_lossy();
        if text == "--" {
            if parsed.path.is_none() {
                parsed.path = iter.next().cloned();
            }
            parsed.options.extend(iter.by_ref().cloned());
            break;
        }
        if text == "-h" || text == "--help" {
            parsed.help = true;
        }
        if text.starts_with('-') {
            parsed.options.push(argument.clone());
            if VALUE_OPTIONS.contains(&text.as_ref()) {
                parsed.options.extend(iter.next().cloned());
            }
        } else if parsed.path.is_none() {
            parsed.path = Some(argument.clone());
        } else {
            parsed.options.push(argument.clone());
        }
    }
    parsed
}

/// `oxide init [cargo init args] [path]`.
pub fn run_init(args: &[OsString]) -> Result<u8, String> {
    let parsed = parse(args);
    if parsed.help {
        return Err(exec_cargo("init", args));
    }
    let directory = PathBuf::from(parsed.path.unwrap_or_else(|| OsString::from(".")));
    let manifest = directory.join("Cargo.toml");
    if fs::symlink_metadata(&manifest).is_ok() {
        if !parsed.options.is_empty() {
            return Err(format!(
                "{} exists, so oxide init skips `cargo init` and adopts the project as it is. \
                 Refused Cargo-only arguments: {}. Re-run without them.",
                manifest.display(),
                join_lossy(&parsed.options)
            ));
        }
    } else {
        let status = Command::new(CARGO)
            .arg("init")
            .args(args)
            .status()
            .map_err(|error| format!("could not start Cargo: {error}"))?;
        if !status.success() {
            return Ok(crate::process::exit_code(status));
        }
    }
    adopt(&manifest)
}

/// `oxide new <path> [cargo new args]`.
pub fn run_new(args: &[OsString]) -> Result<u8, String> {
    let parsed = parse(args);
    if parsed.help {
        return Err(exec_cargo("new", args));
    }
    let status = Command::new(CARGO)
        .arg("new")
        .args(args)
        .status()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    if !status.success() {
        return Ok(crate::process::exit_code(status));
    }
    let directory = parsed
        .path
        .ok_or("cargo new succeeded without a path argument")?;
    adopt(&Path::new(&directory).join("Cargo.toml"))
}

fn exec_cargo(command: &str, args: &[OsString]) -> String {
    let error = Command::new(CARGO).arg(command).args(args).exec();
    format!("could not start Cargo: {error}")
}

fn join_lossy(values: &[OsString]) -> String {
    values
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write or refresh the block at the workspace root Cargo resolves for this
/// manifest, then print readiness.
fn adopt(manifest: &Path) -> Result<u8, String> {
    let workspace = identity::locate_workspace(CARGO, Some(manifest)).map_err(|error| {
        format!("{error}\nNo AGENTS.md block was written: Oxide adopts at the workspace root Cargo resolves.")
    })?;
    let agents = workspace.join(AGENTS);
    let outcome = write_block(&agents)?;
    println!("oxide: {outcome} {}", agents.display());
    print_readiness(&workspace);
    Ok(0)
}

/// The workspace root's `AGENTS.md` holds one ordered Oxide marker pair.
/// A link or unreadable file is not adoption.
pub fn is_adopted(workspace: &Path) -> bool {
    let agents = workspace.join(AGENTS);
    std::fs::symlink_metadata(&agents).is_ok_and(|metadata| metadata.is_file())
        && std::fs::read(&agents)
            .ok()
            .and_then(|text| find_block(&text).ok().flatten())
            .is_some()
}

/// What [`write_block`] did.
#[derive(Debug, Eq, PartialEq)]
enum Written {
    Created,
    Added,
    Refreshed,
    Unchanged,
}

impl std::fmt::Display for Written {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Created => "created the Oxide block in",
            Self::Added => "added the Oxide block to",
            Self::Refreshed => "refreshed the Oxide block in",
            Self::Unchanged => "Oxide block already current in",
        })
    }
}

fn write_block(agents: &Path) -> Result<Written, String> {
    let existing = match fs::symlink_metadata(agents) {
        Ok(metadata) if metadata.file_type().is_file() => Some(
            fs::read(agents)
                .map_err(|error| format!("cannot read {}: {error}", agents.display()))?,
        ),
        Ok(_) => {
            return Err(format!(
                "{} is not a regular file (a link or directory); Oxide does not write through it. Add this block by hand:\n{BLOCK}",
                agents.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("cannot inspect {}: {error}", agents.display())),
    };
    let updated = apply_block(existing.as_deref())
        .map_err(|problem| format!("{}: {problem}; nothing was written", agents.display()))?;
    let outcome = match &existing {
        None => Written::Created,
        Some(before) if *before == updated => return Ok(Written::Unchanged),
        Some(before) if find_block(before).ok().flatten().is_some() => Written::Refreshed,
        Some(_) => Written::Added,
    };
    replace_file(agents, &updated)?;
    Ok(outcome)
}

/// The new file contents: the block replaces the one marked span, or is
/// appended after a blank line. Bytes outside the markers never change.
fn apply_block(existing: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let Some(existing) = existing else {
        return Ok(BLOCK.as_bytes().to_vec());
    };
    let mut updated = Vec::with_capacity(existing.len() + BLOCK.len() + 2);
    match find_block(existing)? {
        Some((start, end)) => {
            updated.extend_from_slice(&existing[..start]);
            updated.extend_from_slice(BLOCK.as_bytes());
            updated.extend_from_slice(&existing[end..]);
        }
        None => {
            updated.extend_from_slice(existing);
            if !existing.is_empty() {
                if !existing.ends_with(b"\n") {
                    updated.push(b'\n');
                }
                if !existing.ends_with(b"\n\n") {
                    updated.push(b'\n');
                }
            }
            updated.extend_from_slice(BLOCK.as_bytes());
        }
    }
    Ok(updated)
}

/// Byte span of the marked block, from the start of the begin line through
/// the end line's newline. Markers count only as whole lines; anything but
/// one ordered pair or none is refused rather than guessed.
fn find_block(text: &[u8]) -> Result<Option<(usize, usize)>, String> {
    let mut begins = Vec::new();
    let mut ends = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive(|byte| *byte == b'\n') {
        let content = line.strip_suffix(b"\n").unwrap_or(line);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if content == BEGIN.as_bytes() {
            begins.push(offset);
        } else if content == END.as_bytes() {
            ends.push(offset + line.len());
        }
        offset += line.len();
    }
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => Ok(None),
        ([begin], [end]) if begin < end => Ok(Some((*begin, *end))),
        _ => Err(format!(
            "found {} `{BEGIN}` and {} `{END}` lines; Oxide needs one ordered pair or none, so fix the markers by hand",
            begins.len(),
            ends.len()
        )),
    }
}

/// Replace through a sibling temporary file and a rename, keeping the
/// original permissions, so a crash never leaves a truncated file.
fn replace_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("AGENTS.md has no parent directory")?;
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let temporary = directory.join(format!(".{AGENTS}.oxide-{}.tmp", std::process::id()));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(contents)?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions)?;
            }
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temporary, path));
    written.map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("cannot write {}: {error}", path.display())
    })
}

/// Toolchain, identity, pool and Effigy selectors. Reports only: nothing here
/// creates the pool or edits a file.
fn print_readiness(workspace: &Path) {
    println!("readiness:");
    match identity::rustc_version(Some(workspace)) {
        Ok(version) => {
            let release = version.lines().next().unwrap_or("rustc").trim();
            let host = version
                .lines()
                .find_map(|line| line.strip_prefix("host: "))
                .unwrap_or("unknown host");
            println!("  toolchain: {release} for {host}, as rustup resolves it here");
        }
        Err(error) => println!("  toolchain: unavailable ({error})"),
    }
    println!("  identity: {}", Identity::infer(workspace).describe());
    match store::pool_path() {
        Err(error) => println!("  pool: unknown ({error})"),
        Ok(path) => match Store::open_existing() {
            Ok(None) => println!(
                "  pool: {} not created; the first building oxide command creates it",
                path.display()
            ),
            Ok(Some(store)) => match store.status() {
                Ok(status) => {
                    // This full status's own whole-payload walk.
                    let used = status
                        .payload_unique_allocated_bytes
                        .map(|used| {
                            let total = store.budget(status.cap_bytes);
                            format!("{} of {} MiB used", used / MIB, total / MIB)
                        })
                        .unwrap_or_else(|| "use unknown".to_owned());
                    println!(
                        "  pool: {}, cap {} MiB, {used}",
                        path.display(),
                        status.cap_bytes / MIB
                    );
                }
                Err(error) => println!("  pool: {} unreadable ({error})", path.display()),
            },
            Err(error) => println!("  pool: {} unreadable ({error})", path.display()),
        },
    }
    report_effigy(workspace);
}

/// Every `effigy.toml` from the workspace root up to the Git top level, with
/// the lines that still call plain `cargo`. Effigy files are never edited.
fn report_effigy(workspace: &Path) {
    let top = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| fs::canonicalize(String::from_utf8_lossy(&output.stdout).trim()).ok())
        .filter(|top| workspace.starts_with(top));
    let mut found = false;
    for directory in workspace.ancestors() {
        let file = directory.join("effigy.toml");
        if fs::symlink_metadata(&file).is_ok_and(|metadata| metadata.is_file()) {
            found = true;
            report_effigy_file(&file);
        }
        if top.as_deref().is_none_or(|top| directory == top) {
            break;
        }
    }
    if !found {
        println!("  effigy: no effigy.toml");
    }
}

fn report_effigy_file(file: &Path) {
    let text = match fs::metadata(file) {
        Ok(metadata) if metadata.len() > EFFIGY_LIMIT => {
            println!(
                "  effigy: {} is over {} KiB; not scanned",
                file.display(),
                EFFIGY_LIMIT / 1024
            );
            return;
        }
        Ok(_) => match fs::read(file) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => {
                println!("  effigy: {} unreadable ({error})", file.display());
                return;
            }
        },
        Err(error) => {
            println!("  effigy: {} unreadable ({error})", file.display());
            return;
        }
    };
    let lines = plain_cargo_lines(&text);
    if lines.is_empty() {
        println!(
            "  effigy: {} has no selectors calling plain cargo",
            file.display()
        );
        return;
    }
    println!(
        "  effigy: {} still calls plain cargo (reported, not edited; use `oxide`):",
        file.display()
    );
    for (number, line) in lines {
        let shown: String = line.chars().take(120).collect();
        println!("    line {number}: {shown}");
    }
}

/// Lines (1-based) that invoke `cargo` as a command word. A line scan, not a
/// TOML parse: comments are skipped and `cargo-*` binaries or `$CARGO` do not
/// count.
fn plain_cargo_lines(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with('#'))
        .filter(|(_, line)| calls_cargo(line))
        .map(|(index, line)| (index + 1, line.trim()))
        .collect()
}

fn calls_cargo(line: &str) -> bool {
    let bytes = line.as_bytes();
    line.match_indices("cargo").any(|(start, _)| {
        let before = start.checked_sub(1).map(|index| bytes[index]);
        let after = bytes.get(start + "cargo".len()).copied();
        let word_start =
            before.is_none_or(|byte| !(byte.is_ascii_alphanumeric() || b"_-$".contains(&byte)));
        let word_end =
            after.is_none_or(|byte| byte.is_ascii_whitespace() || b"\"'".contains(&byte));
        word_start && word_end
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parse_separates_the_directory_from_cargo_options() {
        let parsed = parse(&args(&[
            "--lib",
            "--name",
            "demo",
            "crates/demo",
            "--vcs=none",
        ]));
        assert_eq!(parsed.path, Some(OsString::from("crates/demo")));
        assert_eq!(
            parsed.options,
            args(&["--lib", "--name", "demo", "--vcs=none"])
        );
        assert!(!parsed.help);
        let bare = parse(&[]);
        assert_eq!(bare.path, None);
        assert!(bare.options.is_empty());
        let separated = parse(&args(&["--edition", "2021", "--", "-odd"]));
        assert_eq!(separated.path, Some(OsString::from("-odd")));
        assert!(parse(&args(&["--help"])).help);
    }

    #[test]
    fn absent_or_empty_files_get_only_the_block() {
        assert_eq!(apply_block(None).unwrap(), BLOCK.as_bytes());
        assert_eq!(apply_block(Some(b"")).unwrap(), BLOCK.as_bytes());
    }

    #[test]
    fn appending_separates_with_one_blank_line_and_keeps_existing_bytes() {
        for (before, separator) in [
            ("# Notes", "\n\n"),
            ("# Notes\n", "\n"),
            ("# Notes\n\n", ""),
        ] {
            let updated = apply_block(Some(before.as_bytes())).unwrap();
            assert_eq!(updated, format!("{before}{separator}{BLOCK}").into_bytes());
        }
    }

    #[test]
    fn refresh_replaces_only_the_marked_span_and_is_idempotent() {
        let before =
            format!("# Project\r\nkeep me\n{BEGIN}\nstale guidance\n{END}\ntrailing text\n");
        let updated = apply_block(Some(before.as_bytes())).unwrap();
        assert_eq!(
            updated,
            format!("# Project\r\nkeep me\n{BLOCK}trailing text\n").into_bytes()
        );
        assert_eq!(apply_block(Some(&updated)).unwrap(), updated);
        let first = apply_block(Some(b"intro\n")).unwrap();
        assert_eq!(apply_block(Some(&first)).unwrap(), first);
        // A final end marker without a newline gains the block's own newline.
        let unterminated = format!("x\n{BEGIN}\nold\n{END}");
        assert_eq!(
            apply_block(Some(unterminated.as_bytes())).unwrap(),
            format!("x\n{BLOCK}").into_bytes()
        );
    }

    #[test]
    fn malformed_markers_refuse_instead_of_guessing() {
        for text in [
            format!("{BEGIN}\nno end\n"),
            format!("no begin\n{END}\n"),
            format!("{END}\n{BEGIN}\n"),
            format!("{BLOCK}{BLOCK}"),
            format!("{BEGIN}\n{BEGIN}\n{END}\n"),
        ] {
            assert!(apply_block(Some(text.as_bytes())).is_err(), "{text:?}");
        }
        // Markers inside a line are prose, not markers.
        let inline = format!("see `{BEGIN}` in docs\n");
        assert_eq!(
            apply_block(Some(inline.as_bytes())).unwrap(),
            format!("{inline}\n{BLOCK}").into_bytes()
        );
    }

    #[test]
    fn effigy_scan_reports_command_words_only() {
        let text = r#"[tasks]
"rust:compile" = "cargo check --workspace"
"rust:oxide" = "oxide test"
# "old" = "cargo build"
"nextest" = "cargo-nextest run"
"env" = "$CARGO build"
[test.suites.rust]
run = "python3 scripts/bounded-test.py cargo test --workspace"
"#;
        let lines = plain_cargo_lines(text);
        assert_eq!(
            lines,
            vec![
                (2, r#""rust:compile" = "cargo check --workspace""#),
                (
                    8,
                    r#"run = "python3 scripts/bounded-test.py cargo test --workspace""#
                ),
            ]
        );
    }
}
