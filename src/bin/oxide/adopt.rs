//! `oxide init` / `oxide new`: Cargo's command when it applies, then one
//! marked guidance block (in the Git work-tree root's `AGENTS.md`, or the
//! workspace's own outside Git; see [`crate::guidance`]) and a readiness
//! report. No Oxide config file; `Cargo.toml` changes only as Cargo writes it.

use crate::config::{self, Found};
use crate::guidance::{self, Placement};
use crate::identity::{self, Identity};
use crate::store;
use crate::CARGO;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BEGIN: &str = "<!-- oxide:begin -->";
pub const END: &str = "<!-- oxide:end -->";
/// Opens the covered-workspace list inside a Git-root block.
pub const COVERS_OPEN: &str = "<!-- oxide:workspaces:";
pub const COVERS_CLOSE: &str = "-->";

/// The managed block, markers included. Outside Git it carries no list; a
/// Git-root block carries one (see [`block`]). Refreshing replaces it whole.
/// Outside Git, the text is the block Oxide has always written.
pub fn block(covers: Option<&[String]>) -> String {
    let mut text = format!("{BEGIN}\n");
    if let Some(covers) = covers {
        text.push_str(&format!(
            "{COVERS_OPEN} {} {COVERS_CLOSE}\n",
            covers.join(" ")
        ));
    }
    text.push_str("## Rust commands: use `oxide`\n");
    if let Some(covers) = covers {
        let listed: Vec<String> = covers.iter().map(|cover| format!("`{cover}`")).collect();
        text.push_str(&format!(
            "\nCovers Cargo workspaces {}, relative to this file's folder.\n",
            listed.join(", ")
        ));
    }
    text.push_str(
        "
- Write `oxide` wherever you would write `cargo`: `oxide check`, `oxide test`, `oxide run -- <args>`, `oxide add <crate>`.
- Profiles: `quick` (default), `--profile debug` for a debugger, `--release`. Oxide places build output, so it refuses `--target-dir`.
- A tool that runs Cargo itself (Tauri CLI, npm or bun scripts): `oxide exec -- <command>`.
- A `waiting for …` line means another build holds a slot; let it wait. `oxide status` shows the pool, slots and waits.
- Use `oxide clean`, not `cargo clean`.

Managed by `oxide init`; edits between these markers are replaced.
",
    );
    text.push_str(END);
    text.push('\n');
    text
}

/// The guidance file name, in the workspace or at the Git root.
pub const AGENTS: &str = "AGENTS.md";
/// Bounded read of an `AGENTS.md`; larger files are refused, not rewritten.
const AGENTS_LIMIT: u64 = 1024 * 1024;

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
const GITIGNORE: &str = ".gitignore";
/// Bounded read of a workspace `.gitignore`.
const GITIGNORE_LIMIT: u64 = 1024 * 1024;

/// The directory argument and every other argument of `cargo init`/`new`.
/// `--help` is not read here: `main` forwards a help request to Cargo before
/// this module runs, so there is one place that decides it.
struct Parsed {
    path: Option<OsString>,
    options: Vec<OsString>,
}

fn parse(args: &[OsString]) -> Parsed {
    let mut parsed = Parsed {
        path: None,
        options: Vec::new(),
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

fn join_lossy(values: &[OsString]) -> String {
    values
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write or refresh the guidance block for the workspace Cargo resolves for
/// this manifest, then print readiness. The placement is classified after
/// Cargo ran, so a repository Cargo just created counts as Git.
fn adopt(manifest: &Path) -> Result<u8, String> {
    let workspace = identity::locate_workspace(CARGO, Some(manifest)).map_err(|error| {
        format!("{error}\nNo AGENTS.md block was written: Oxide adopts at the workspace root Cargo resolves.")
    })?;
    let placement = guidance::placement(&workspace).map_err(|error| {
        format!(
            "{error}\nNo AGENTS.md block was written: Oxide could not confirm the Git work tree."
        )
    })?;
    let guidance = write_guidance(&workspace, &placement)?;
    println!("oxide: {} {}", guidance.written, guidance.path.display());
    let ignored = ensure_local_outputs_ignored(&workspace)?;
    println!(
        "oxide: {} {} (the checkout-local fallback output root of permanent drive mode)",
        ignored.describe(),
        workspace.join(GITIGNORE).display()
    );
    // A first permanent-drive conversion publishes owned `target/<member>`
    // links only when Git ignores each exact path; a rule that covers the
    // member links (including a directory-only parent rule) is silent. When it
    // does not, name the rule that covers them instead of rewriting a user's
    // `.gitignore` (oxide#154). Outside Git nothing is ignored and nothing is
    // converted, so there is nothing to report.
    if matches!(placement, Placement::Worktree(_)) {
        if let Some(warning) = crate::outputs::gitignore_diagnostic(&workspace) {
            eprintln!("oxide: WARNING: {warning}");
        }
    }
    print_readiness(&workspace, &placement, &guidance);
    Ok(0)
}

/// Where the guidance landed and what it covers.
struct Guidance {
    path: PathBuf,
    written: Written,
    /// Covered workspace paths relative to the Git root; empty outside Git.
    covers: Vec<String>,
}

fn write_guidance(workspace: &Path, placement: &Placement) -> Result<Guidance, String> {
    match placement {
        Placement::Outside => {
            let path = workspace.join(AGENTS);
            let existing = read_for_write(&path, &block(None))?;
            let written = write_block(&path, existing, &block(None))?;
            Ok(Guidance {
                path,
                written,
                covers: Vec::new(),
            })
        }
        Placement::Worktree(root) => {
            let current = guidance::cover_path(root, workspace)?;
            let path = root.join(AGENTS);
            // Read, merge and write under one lock on the Git root.
            let _lock = guidance::lock_root(root)?;
            let existing = read_for_write(&path, &block(Some(std::slice::from_ref(&current))))?;
            let text = match &existing {
                Some(bytes) => block_text(bytes).map_err(|problem| {
                    format!("{}: {problem}; nothing was written", path.display())
                })?,
                None => None,
            };
            let manifest = fs::symlink_metadata(root.join("Cargo.toml"))
                .is_ok_and(|metadata| metadata.file_type().is_file());
            let covers = guidance::merged_covers(text, manifest, &current)
                .map_err(|problem| format!("{}: {problem}; nothing was written", path.display()))?;
            let written = write_block(&path, existing, &block(Some(&covers)))?;
            Ok(Guidance {
                path,
                written,
                covers,
            })
        }
    }
}

/// What [`ensure_local_outputs_ignored`] did.
#[derive(Debug, Eq, PartialEq)]
enum Ignored {
    Created,
    Added,
    Present,
}

impl Ignored {
    fn describe(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Added => "added the local-output ignore entry to",
            Self::Present => "local-output ignore entry already present in",
        }
    }
}

/// Idempotently add [`crate::outputs::GITIGNORE_LINE`] to the workspace
/// `.gitignore`, so a checkout-local fallback output root is never tracked.
/// A symlink or oversized file is refused rather than rewritten.
fn ensure_local_outputs_ignored(workspace: &Path) -> Result<Ignored, String> {
    let path = workspace.join(GITIGNORE);
    let existing = match read_regular(&path, GITIGNORE_LIMIT) {
        Ok(existing) => existing.unwrap_or_default(),
        Err(Unreadable::Oversize) => {
            return Err(format!(
                "{} is larger than {GITIGNORE_LIMIT} bytes; not edited",
                path.display()
            ))
        }
        Err(Unreadable::NotRegular) => {
            return Err(format!(
                "{} is not a regular file; left alone",
                path.display()
            ))
        }
        Err(Unreadable::Io(error)) => {
            return Err(format!("cannot read {}: {error}", path.display()))
        }
    };
    let text = String::from_utf8_lossy(&existing);
    if text.lines().any(|line| ignores_local_outputs(line.trim())) {
        return Ok(Ignored::Present);
    }
    let mut bytes = existing.clone();
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        bytes.push(b'\n');
    }
    bytes.extend_from_slice(crate::outputs::GITIGNORE_LINE.as_bytes());
    bytes.push(b'\n');
    replace_file(&path, &bytes, GITIGNORE)?;
    Ok(if existing.is_empty() {
        Ignored::Created
    } else {
        Ignored::Added
    })
}

fn ignores_local_outputs(line: &str) -> bool {
    matches!(
        line,
        "/.oxide-outputs/" | ".oxide-outputs/" | "/.oxide-outputs" | ".oxide-outputs"
    )
}

/// A regular `AGENTS.md` read within [`AGENTS_LIMIT`]: `None` when absent.
/// A link, directory or oversize file is an error, never followed.
pub fn read_bounded(agents: &Path) -> Result<Option<Vec<u8>>, String> {
    read_regular(agents, AGENTS_LIMIT).map_err(|problem| match problem {
        Unreadable::Oversize => format!("is larger than {AGENTS_LIMIT} bytes"),
        Unreadable::NotRegular => "is not a regular file (a link or directory)".to_owned(),
        Unreadable::Io(error) => format!("cannot read: {error}"),
    })
}

/// Why a file could not be read whole as a regular file.
#[derive(Debug)]
enum Unreadable {
    Oversize,
    NotRegular,
    Io(std::io::Error),
}

/// Read a regular file of at most `limit` bytes; `Ok(None)` when it is absent.
///
/// The open refuses a final link (`O_NOFOLLOW`) and never blocks on a FIFO, and
/// the type and size are read from that same descriptor. A name swapped for a
/// link after a separate check therefore cannot redirect the read.
fn read_regular(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, Unreadable> {
    use std::io::Read;
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
            return Err(Unreadable::NotRegular)
        }
        Err(error) => return Err(Unreadable::Io(error)),
    };
    let metadata = file.metadata().map_err(Unreadable::Io)?;
    if !metadata.file_type().is_file() {
        return Err(Unreadable::NotRegular);
    }
    if metadata.len() > limit {
        return Err(Unreadable::Oversize);
    }
    let mut bytes = Vec::new();
    // One byte past the limit, so a file that grew since its size was read is refused.
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(Unreadable::Io)?;
    if bytes.len() as u64 > limit {
        return Err(Unreadable::Oversize);
    }
    Ok(Some(bytes))
}

/// [`read_bounded`] for a write. A refusal names the file and prints the
/// block to add by hand, since Oxide will not write through it.
fn read_for_write(agents: &Path, block: &str) -> Result<Option<Vec<u8>>, String> {
    read_bounded(agents).map_err(|problem| {
        format!(
            "{} {problem}; nothing was written. Add this block by hand:\n{block}",
            agents.display()
        )
    })
}

/// The text of the marked block in `bytes`, or `None` without markers.
pub fn block_text(bytes: &[u8]) -> Result<Option<&str>, String> {
    let Some((start, end)) = find_block(bytes)? else {
        return Ok(None);
    };
    std::str::from_utf8(&bytes[start..end])
        .map(Some)
        .map_err(|_| "the Oxide block is not UTF-8".to_owned())
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

/// Write `block` into `agents`, whose current bytes the caller has read.
fn write_block(agents: &Path, existing: Option<Vec<u8>>, block: &str) -> Result<Written, String> {
    let updated = apply_block(existing.as_deref(), block)
        .map_err(|problem| format!("{}: {problem}; nothing was written", agents.display()))?;
    let outcome = match &existing {
        None => Written::Created,
        Some(before) if *before == updated => return Ok(Written::Unchanged),
        Some(before) if find_block(before).ok().flatten().is_some() => Written::Refreshed,
        Some(_) => Written::Added,
    };
    replace_file(agents, &updated, AGENTS)?;
    Ok(outcome)
}

/// The new file contents: the block replaces the one marked span, or is
/// appended after a blank line. Bytes outside the markers never change.
fn apply_block(existing: Option<&[u8]>, block: &str) -> Result<Vec<u8>, String> {
    let Some(existing) = existing else {
        return Ok(block.as_bytes().to_vec());
    };
    let mut updated = Vec::with_capacity(existing.len() + block.len() + 2);
    match find_block(existing)? {
        Some((start, end)) => {
            updated.extend_from_slice(&existing[..start]);
            updated.extend_from_slice(block.as_bytes());
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
            updated.extend_from_slice(block.as_bytes());
        }
    }
    Ok(updated)
}

/// Byte span of the marked block, from the start of the begin line through
/// the end line's newline. Markers count only as whole lines; anything but
/// one ordered pair or none is refused rather than guessed.
pub fn find_block(text: &[u8]) -> Result<Option<(usize, usize)>, String> {
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

/// Replace `path` through a sibling temporary file and a rename, keeping the
/// original permissions, so a crash never leaves a truncated file. `name` is
/// the replaced file's own name, used to build the temporary's name in the
/// same directory, so two files in one directory never share a temporary.
fn replace_file(path: &Path, contents: &[u8], name: &str) -> Result<(), String> {
    let directory = path.parent().ok_or("the file has no parent directory")?;
    // The mode is read from the file itself without following a link, and the
    // rename below replaces the name rather than writing through it.
    let permissions = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| file.metadata())
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .map(|metadata| metadata.permissions());
    let temporary = directory.join(format!(".{name}.oxide-{}.tmp", std::process::id()));
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

/// Guidance, toolchain, identity, pool and Effigy selectors. Reports only:
/// nothing here creates the pool, opens its ledger or edits a file.
fn print_readiness(workspace: &Path, placement: &Placement, guidance: &Guidance) {
    println!("readiness:");
    println!("  workspace: {}", workspace.display());
    match placement {
        Placement::Outside => println!(
            "  guidance: {} (no Git work tree; this workspace's own AGENTS.md)",
            guidance.path.display()
        ),
        Placement::Worktree(root) => {
            println!(
                "  guidance root: {} (Git work tree); guidance: {}",
                root.display(),
                guidance.path.display()
            );
            println!("  covered paths: {}", guidance.covers.join(", "));
            if let Some(line) = guidance::retained_report(workspace, root) {
                println!("  {line}");
            }
        }
    }
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
    match store::settings() {
        Err(error) => {
            println!("  selected pool: unknown ({error})");
            println!("  configured internal pool: unknown ({error})");
        }
        Ok(settings) => {
            let (selected, configured) = readiness_pool_lines(&settings);
            println!("  selected pool: {selected}");
            println!("  configured internal pool: {configured}");
        }
    }
    report_effigy(workspace, placement);
}

/// The pool a build would select, and the configured internal pool, as
/// readiness can know them without opening either (oxide#182). The selection
/// is the same recorded fact `oxide status` prints: the registration's
/// mount point is present or absent right now, and the next build is what
/// verifies identity, reserve and write access. Reading the registration
/// takes no pool lock and reconciles nothing.
fn readiness_pool_lines(settings: &config::Settings) -> (String, String) {
    let configured = pool_state(settings);
    let selected = match config::inspect(&settings.pool) {
        // A retired image pool is not a pool a build can select; say so
        // instead of claiming the configured path is the internal pool.
        Found::LegacyImage => format!("unknown ({configured})"),
        Found::Absent => selected_pool_line(&settings.pool, &Ok(None)),
        Found::Directory | Found::Unknown => selected_pool_line(
            &settings.pool,
            &crate::removable::registration_at(&settings.pool),
        ),
    };
    (selected, configured)
}

/// One line naming the pool the next build would use, from the recorded
/// registration alone.
fn selected_pool_line(
    path: &Path,
    registration: &Result<Option<crate::removable::Registration>, String>,
) -> String {
    match registration {
        Err(error) => format!("unknown (cannot read the removable registration: {error})"),
        Ok(None) => format!(
            "internal {} (no removable volume registered)",
            path.display()
        ),
        Ok(Some(registration)) => {
            let mount = registration.mount_point.display();
            if fs::symlink_metadata(&registration.mount_point).is_ok() {
                format!(
                    "removable {mount} (registered, mode: {}; the next build verifies the volume)",
                    registration.mode
                )
            } else {
                format!(
                    "internal {} (the registered removable pool {mount} is absent; the next build uses the internal pool until the volume returns)",
                    path.display()
                )
            }
        }
    }
}

/// Pool facts from the path and the resolved settings. Opening a pool takes
/// its lock and reconciles its ledger, so readiness never does; `oxide status`
/// measures. Creation policy is the effect-free rule a build applies
/// (oxide#182), so readiness never promises a pool an explicit `OXIDE_POOL`
/// would refuse to create.
fn pool_state(settings: &config::Settings) -> String {
    let path = settings.pool.as_path();
    match config::inspect(path) {
        Found::Absent => {
            let creates = config::pool_creation_allowed(
                path,
                settings.pool_source,
                std::env::var_os("OXIDE_POOL_CREATE").as_deref(),
                std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").as_deref(),
            );
            match creates {
                Ok(()) => format!(
                    "{} not created; the first building oxide command creates it",
                    path.display()
                ),
                Err(reason) => format!(
                    "{} not created; the first building oxide command refuses: {reason}",
                    path.display()
                ),
            }
        }
        Found::LegacyImage => format!("{} is a retired image pool; not read", path.display()),
        Found::Directory | Found::Unknown => {
            format!(
                "{}, use not measured (readiness reads no pool state)",
                path.display()
            )
        }
    }
}

/// Every `effigy.toml` from the workspace root up to the Git top level, with
/// the selectors that still call plain `cargo`. Effigy files are never
/// edited.
fn report_effigy(workspace: &Path, placement: &Placement) {
    // The work-tree root is already resolved for this readiness report, so it
    // is reused rather than asking Git again without a bound.
    let top = match placement {
        Placement::Worktree(root) => Some(root.clone()),
        Placement::Outside => None,
    }
    .filter(|top| workspace.starts_with(top));
    let mut found = false;
    let mut acknowledged: Vec<String> = Vec::new();
    for directory in workspace.ancestors() {
        let file = directory.join("effigy.toml");
        if fs::symlink_metadata(&file).is_ok_and(|metadata| metadata.is_file()) {
            found = true;
            acknowledged.extend(report_effigy_file(&file));
        }
        if top.as_deref().is_none_or(|top| directory == top) {
            break;
        }
    }
    if !found {
        println!("  effigy: no effigy.toml");
        return;
    }
    // Intentional escapes are acknowledged once for this run, never stored
    // and never silenced selector by selector.
    if !acknowledged.is_empty() {
        acknowledged.sort();
        let shown: Vec<&str> = acknowledged.iter().take(8).map(String::as_str).collect();
        let rest = acknowledged.len().saturating_sub(shown.len());
        let more = if rest > 0 {
            format!(" and {rest} more")
        } else {
            String::new()
        };
        println!(
            "  effigy: acknowledged once for this run: {} intentional plain-cargo escape selector(s) (`:cargo` tasks, `-cargo` suites): {}{more}",
            acknowledged.len(),
            shown.join(", ")
        );
    }
}

/// One file's selectors, read from parsed TOML so only executable routes
/// count: a `:cargo` task or `-cargo` suite that runs plain `cargo` is an
/// acknowledged escape, any other executable selector that does is an
/// actionable warning. Comments and dependency references are not routes.
/// A file that is not parseable TOML stays honest: its calling lines are
/// reported unclassified rather than guessed into a group.
fn report_effigy_file(file: &Path) -> Vec<String> {
    let bytes = match read_regular(file, EFFIGY_LIMIT) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            println!("  effigy: {} is gone (not scanned)", file.display());
            return Vec::new();
        }
        Err(Unreadable::Oversize) => {
            println!(
                "  effigy: {} is over {} KiB; not scanned",
                file.display(),
                EFFIGY_LIMIT / 1024
            );
            return Vec::new();
        }
        Err(Unreadable::NotRegular) => {
            println!(
                "  effigy: {} is not a regular file; not scanned",
                file.display()
            );
            return Vec::new();
        }
        Err(Unreadable::Io(error)) => {
            println!("  effigy: {} unreadable ({error})", file.display());
            return Vec::new();
        }
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match classify_effigy(&text) {
        Ok(scan) => {
            if scan.warnings.is_empty() {
                println!(
                    "  effigy: {} has no actionable selectors calling plain cargo",
                    file.display()
                );
            } else {
                println!(
                    "  effigy: {} still calls plain cargo (reported, not edited; use `oxide`):",
                    file.display()
                );
                for warning in &scan.warnings {
                    let shown: String = warning.command.chars().take(120).collect();
                    println!("    {}: {shown}", warning.selector);
                }
            }
            scan.acknowledged
        }
        Err(reason) => {
            println!(
                "  effigy: {} is not parseable as TOML ({reason}); plain cargo calls are reported unclassified:",
                file.display()
            );
            for (number, line) in plain_cargo_lines(&text) {
                let shown: String = line.chars().take(120).collect();
                println!("    line {number}: {shown}");
            }
            Vec::new()
        }
    }
}

/// Plain-Cargo selectors of one parsed `effigy.toml`.
#[derive(Debug, Default, Eq, PartialEq)]
struct EffigyScan {
    /// Intentional escape selector names (`:cargo` tasks, `-cargo` suites).
    acknowledged: Vec<String>,
    /// Actionable selectors that still call plain `cargo`.
    warnings: Vec<Warning>,
}

#[derive(Debug, Eq, PartialEq)]
struct Warning {
    selector: String,
    command: String,
}

/// Parse the file and classify its executable plain-Cargo routes. Only the
/// command a selector runs counts: a task's string value, a task's or suite's
/// `run` string or list. Comments, `depends`-style references and any shape
/// this reader does not understand contribute nothing.
fn classify_effigy(text: &str) -> Result<EffigyScan, String> {
    let value: serde_json::Value = basic_toml::from_str(text).map_err(|error| error.to_string())?;
    let Some(root) = value.as_object() else {
        return Err("not a TOML table".to_owned());
    };
    let mut scan = EffigyScan::default();
    if let Some(tasks) = root.get("tasks") {
        let tasks = tasks
            .as_object()
            .ok_or("`tasks` is not a table of selectors")?;
        for (name, entry) in tasks {
            for command in executable_commands(entry) {
                if !calls_cargo(&command) {
                    continue;
                }
                if name.ends_with(":cargo") {
                    scan.acknowledged.push(format!("tasks.{name}"));
                } else {
                    scan.warnings.push(Warning {
                        selector: format!("tasks.{name}"),
                        command,
                    });
                }
            }
        }
    }
    if let Some(suites) = root
        .get("test")
        .and_then(|test| test.get("suites"))
        .filter(|suites| !suites.is_null())
    {
        let suites = suites
            .as_object()
            .ok_or("`test.suites` is not a table of suites")?;
        for (name, entry) in suites {
            for command in executable_commands(entry) {
                if !calls_cargo(&command) {
                    continue;
                }
                if name.ends_with("-cargo") || name.ends_with(":cargo") {
                    scan.acknowledged.push(format!("test.suites.{name}"));
                } else {
                    scan.warnings.push(Warning {
                        selector: format!("test.suites.{name}"),
                        command,
                    });
                }
            }
        }
    }
    Ok(scan)
}

/// The command strings one selector's value runs.
fn executable_commands(entry: &serde_json::Value) -> Vec<String> {
    match entry {
        serde_json::Value::String(command) => vec![command.clone()],
        serde_json::Value::Array(commands) => commands
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect(),
        serde_json::Value::Object(object) => match object.get("run") {
            Some(run) => executable_commands(run),
            None => Vec::new(),
        },
        _ => Vec::new(),
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

    /// Regression: the reads checked the name with `symlink_metadata`, then read
    /// it by name again, so a link swapped in between was followed. The open
    /// itself now refuses the link, and the size is checked on the descriptor.
    #[test]
    fn a_guidance_read_refuses_a_link_and_bounds_the_size() {
        let dir = std::env::temp_dir().join(format!("oxide-adopt-read-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("fixture");
        let outside = dir.join("outside");
        fs::write(&outside, b"secret").expect("outside");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&outside, &link).expect("link");
        assert!(matches!(
            read_regular(&link, 64),
            Err(Unreadable::NotRegular)
        ));
        let regular = dir.join("regular");
        fs::write(&regular, b"hello").expect("regular");
        assert_eq!(
            read_regular(&regular, 64).expect("a read").as_deref(),
            Some(b"hello".as_slice())
        );
        assert!(matches!(
            read_regular(&regular, 4),
            Err(Unreadable::Oversize)
        ));
        assert_eq!(read_regular(&dir.join("absent"), 64).expect("absent"), None);
        assert!(matches!(
            read_regular(&dir, 64),
            Err(Unreadable::NotRegular)
        ));
        fs::remove_dir_all(&dir).expect("remove own fixture");
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
        let bare = parse(&[]);
        assert_eq!(bare.path, None);
        assert!(bare.options.is_empty());
        let separated = parse(&args(&["--edition", "2021", "--", "-odd"]));
        assert_eq!(separated.path, Some(OsString::from("-odd")));
        // A help request is Cargo's, forwarded before this parse ever runs, so
        // it is just another Cargo option here.
        let help = parse(&args(&["--help"]));
        assert_eq!(help.path, None);
        assert_eq!(help.options, args(&["--help"]));
    }

    /// The outside-Git block, byte for byte as Oxide has always written it.
    const OUTSIDE: &str = "\
<!-- oxide:begin -->
## Rust commands: use `oxide`

- Write `oxide` wherever you would write `cargo`: `oxide check`, `oxide test`, `oxide run -- <args>`, `oxide add <crate>`.
- Profiles: `quick` (default), `--profile debug` for a debugger, `--release`. Oxide places build output, so it refuses `--target-dir`.
- A tool that runs Cargo itself (Tauri CLI, npm or bun scripts): `oxide exec -- <command>`.
- A `waiting for …` line means another build holds a slot; let it wait. `oxide status` shows the pool, slots and waits.
- Use `oxide clean`, not `cargo clean`.

Managed by `oxide init`; edits between these markers are replaced.
<!-- oxide:end -->
";

    #[test]
    fn outside_git_block_is_unchanged() {
        assert_eq!(block(None), OUTSIDE);
    }

    #[test]
    fn git_root_block_lists_its_covers_and_round_trips() {
        let covers = vec![".".to_owned(), "crates/app".to_owned()];
        let text = block(Some(&covers));
        assert!(text.starts_with(
            "<!-- oxide:begin -->\n<!-- oxide:workspaces: . crates/app -->\n## Rust commands"
        ));
        assert!(text.contains(
            "Covers Cargo workspaces `.`, `crates/app`, relative to this file's folder."
        ));
        assert_eq!(guidance::parse_covers(&text).unwrap(), Some(covers));
    }

    #[test]
    fn absent_or_empty_files_get_only_the_block() {
        assert_eq!(apply_block(None, &block(None)).unwrap(), OUTSIDE.as_bytes());
        assert_eq!(
            apply_block(Some(b""), &block(None)).unwrap(),
            OUTSIDE.as_bytes()
        );
    }

    #[test]
    fn appending_separates_with_one_blank_line_and_keeps_existing_bytes() {
        for (before, separator) in [
            ("# Notes", "\n\n"),
            ("# Notes\n", "\n"),
            ("# Notes\n\n", ""),
        ] {
            let updated = apply_block(Some(before.as_bytes()), OUTSIDE).unwrap();
            assert_eq!(
                updated,
                format!("{before}{separator}{OUTSIDE}").into_bytes()
            );
        }
    }

    #[test]
    fn refresh_replaces_only_the_marked_span_and_is_idempotent() {
        let before =
            format!("# Project\r\nkeep me\n{BEGIN}\nstale guidance\n{END}\ntrailing text\n");
        let updated = apply_block(Some(before.as_bytes()), OUTSIDE).unwrap();
        assert_eq!(
            updated,
            format!("# Project\r\nkeep me\n{OUTSIDE}trailing text\n").into_bytes()
        );
        assert_eq!(apply_block(Some(&updated), OUTSIDE).unwrap(), updated);
        let first = apply_block(Some(b"intro\n"), OUTSIDE).unwrap();
        assert_eq!(apply_block(Some(&first), OUTSIDE).unwrap(), first);
        // A final end marker without a newline gains the block's own newline.
        let unterminated = format!("x\n{BEGIN}\nold\n{END}");
        assert_eq!(
            apply_block(Some(unterminated.as_bytes()), OUTSIDE).unwrap(),
            format!("x\n{OUTSIDE}").into_bytes()
        );
    }

    #[test]
    fn malformed_markers_refuse_instead_of_guessing() {
        for text in [
            format!("{BEGIN}\nno end\n"),
            format!("no begin\n{END}\n"),
            format!("{END}\n{BEGIN}\n"),
            format!("{OUTSIDE}{OUTSIDE}"),
            format!("{BEGIN}\n{BEGIN}\n{END}\n"),
        ] {
            assert!(
                apply_block(Some(text.as_bytes()), OUTSIDE).is_err(),
                "{text:?}"
            );
        }
        // Markers inside a line are prose, not markers.
        let inline = format!("see `{BEGIN}` in docs\n");
        assert_eq!(
            apply_block(Some(inline.as_bytes()), OUTSIDE).unwrap(),
            format!("{inline}\n{OUTSIDE}").into_bytes()
        );
    }

    #[test]
    fn rewriting_gitignore_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let workspace = base.join(format!("gitignore-mode-{}", std::process::id()));
        fs::create_dir_all(&workspace).unwrap();
        let path = workspace.join(GITIGNORE);
        fs::write(&path, "target/\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            ensure_local_outputs_ignored(&workspace).unwrap(),
            Ignored::Added
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the rewrite must keep the file's mode");
        fs::remove_dir_all(&workspace).unwrap();
    }

    #[test]
    fn readiness_names_the_pool_a_build_would_use() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let present = base.join(format!("readiness-mount-{}", std::process::id()));
        fs::create_dir_all(&present).unwrap();
        let absent = base.join(format!("readiness-absent-{}", std::process::id()));
        let pool = base.join("readiness-pool");
        let registration = |mount: PathBuf| crate::removable::Registration {
            payload_dir: "oxide-pool".to_owned(),
            outputs_dir: "oxide-outputs".to_owned(),
            version: crate::removable::REGISTRATION_VERSION,
            mount_point: mount,
            volume_uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
            container_uuid: "66666666-7777-8888-9999-000000000000".to_owned(),
            reserve_bytes: 1,
            cap_bytes: 1 << 30,
            quota_bytes: None,
            capacity_bytes: None,
            registered_ms: 1,
            mode: crate::removable::DriveMode::Permanent,
        };
        let unregistered = selected_pool_line(&pool, &Ok(None));
        assert!(unregistered.contains("internal"), "{unregistered}");
        assert!(
            unregistered.contains(&pool.display().to_string()),
            "{unregistered}"
        );
        assert!(
            unregistered.contains("no removable volume registered"),
            "{unregistered}"
        );
        // The same recorded fact `status` prints: present means the removable
        // pool is the recorded selection, absent means the internal pool.
        let selected = selected_pool_line(&pool, &Ok(Some(registration(present.clone()))));
        assert!(selected.starts_with("removable "), "{selected}");
        assert!(selected.contains("registered"), "{selected}");
        assert!(
            selected.contains(&present.display().to_string()),
            "{selected}"
        );
        let fallback = selected_pool_line(&pool, &Ok(Some(registration(absent.clone()))));
        assert!(fallback.contains("internal"), "{fallback}");
        assert!(fallback.contains("absent"), "{fallback}");
        assert!(
            fallback.contains(&absent.display().to_string()),
            "{fallback}"
        );
        let unreadable = selected_pool_line(&pool, &Err("broken".to_owned()));
        assert!(unreadable.contains("unknown") && unreadable.contains("broken"));
        fs::remove_dir_all(&present).unwrap();
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
run = "python3 scripts/wrapper.py cargo test --workspace"
"#;
        let lines = plain_cargo_lines(text);
        assert_eq!(
            lines,
            vec![
                (2, r#""rust:compile" = "cargo check --workspace""#),
                (
                    8,
                    r#"run = "python3 scripts/wrapper.py cargo test --workspace""#
                ),
            ]
        );
    }

    /// Intentional escapes are grouped once and kept apart from the
    /// actionable selectors. Only executable routes count: comments and
    /// dependency references are not plain-Cargo calls.
    #[test]
    fn effigy_classification_separates_acknowledged_escapes() {
        let text = r#"[tasks]
"rust:compile:cargo" = "cargo check --workspace --locked"
"rust:compile" = "oxide check --workspace --locked"
"legacy" = "cargo build"
"nextest" = "cargo-nextest run"
"env" = "$CARGO build"
# "commented" = "cargo test"
"batch" = { run = ["cargo fmt --all", "oxide test"], admission = "heavy" }
"with-dep" = { run = "oxide test", depends = ["rust:compile:cargo"] }
[test.suites.rust-cargo]
run = "cargo test --workspace"
[test.suites."rust:cargo"]
run = "cargo metadata"
[test.suites.rust]
run = "python3 scripts/wrapper.py cargo test --workspace"
"#;
        let scan = classify_effigy(text).unwrap();
        assert_eq!(
            scan.acknowledged,
            vec![
                "tasks.rust:compile:cargo".to_owned(),
                "test.suites.rust-cargo".to_owned(),
                "test.suites.rust:cargo".to_owned()
            ]
        );
        let mut warnings: Vec<(String, String)> = scan
            .warnings
            .into_iter()
            .map(|warning| (warning.selector, warning.command))
            .collect();
        warnings.sort();
        assert_eq!(
            warnings,
            vec![
                ("tasks.batch".to_owned(), "cargo fmt --all".to_owned()),
                ("tasks.legacy".to_owned(), "cargo build".to_owned()),
                (
                    "test.suites.rust".to_owned(),
                    "python3 scripts/wrapper.py cargo test --workspace".to_owned()
                ),
            ]
        );
        // A file with no plain-Cargo route reports nothing at all.
        let scan = classify_effigy("[tasks]\n\"a:b\" = \"oxide test\"\n").unwrap();
        assert_eq!(scan, EffigyScan::default());
        // Malformed or wrongly shaped files stay unclassified instead of
        // being guessed into either group.
        assert!(classify_effigy("[tasks\n").is_err());
        assert!(classify_effigy("not a table").is_err());
        assert!(classify_effigy("[tasks]\nx = 3\n").is_ok());
        assert!(classify_effigy("[tasks]\nx = [1, 2]\n").is_ok());
        assert!(classify_effigy("[test]\nsuites = 3\n").is_err());
    }
}
