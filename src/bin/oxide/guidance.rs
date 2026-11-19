//! Where `oxide init` writes its guidance block, and which Cargo workspaces a
//! block covers. Inside a Git work tree the block lives once, in the work-tree
//! root's `AGENTS.md`, and lists the workspaces it covers. Outside Git the
//! workspace's own `AGENTS.md` stands. Covering the workspace is adoption.

use crate::adopt::{self, AGENTS, COVERS_CLOSE, COVERS_OPEN};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Most paths one block may list.
const MAX_COVERS: usize = 64;
/// Longest one listed path may be, in bytes.
const MAX_COVER_BYTES: usize = 1024;
/// Time limit for the placement probe.
const GIT_TIME: Duration = Duration::from_secs(5);
/// Byte cap for the probe's stdout; an answer over it refuses.
const MAX_GIT_BYTES: usize = 64 * 1024;
/// Stderr kept for classification; the rest is not read.
const MAX_GIT_STDERR_BYTES: u64 = 4 * 1024;
/// Stderr prefix of Git's own "no repository here" answer.
const NOT_A_REPOSITORY: &[u8] = b"fatal: not a git repository";
/// Time the guidance lock is waited for before the init refuses.
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// Where the guidance block belongs for one workspace.
#[derive(Debug, Eq, PartialEq)]
pub enum Placement {
    /// No Git work tree contains the workspace.
    Outside,
    /// The work tree root that holds the one block.
    Worktree(PathBuf),
}

/// Whether one workspace is adopted, for `oxide clean --cargo-cache` and any
/// other caller that needs the answer.
#[derive(Debug, Eq, PartialEq)]
pub enum Adoption {
    Adopted,
    NotAdopted,
    /// The workspace's own `AGENTS.md` holds a block that the Git root does
    /// not list it under. That block is not adoption.
    LegacyNested(PathBuf),
    /// Git or the guidance file could not be inspected safely.
    Uncertain(String),
}

/// The placement of `workspace`, from a bounded `git rev-parse
/// --show-toplevel` run in its directory. Only a missing `git` and Git's own
/// "not a git repository" answer count as positive absence. Any other failure
/// refuses, so a repository error never writes nested guidance. Call it after
/// `cargo init`/`new`, so a repository Cargo just created counts.
pub fn placement(workspace: &Path) -> Result<Placement, String> {
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace);
    remove_git_environment(&mut command);
    command
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Its own group, so a timeout stops anything Git started as well.
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(Placement::Outside);
        }
        Err(error) => return Err(format!("cannot run git: {error}")),
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        crate::describe::stop_group_and_reap(&mut child);
        return Err("git kept no output pipes".to_owned());
    };
    let (out_sender, out_receiver) = mpsc::channel();
    let (err_sender, err_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = out_sender.send(read_capped(stdout));
    });
    thread::spawn(move || {
        let _ = err_sender.send(read_prefix(stderr));
    });
    let deadline = Instant::now() + GIT_TIME;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                crate::describe::stop_group_and_reap(&mut child);
                return Err("git rev-parse did not finish in 5 s; nothing was written".to_owned());
            }
            Err(error) => {
                crate::describe::stop_group_and_reap(&mut child);
                return Err(format!("cannot wait for git: {error}"));
            }
        }
    };
    let settle = Duration::from_secs(1);
    let stdout = out_receiver
        .recv_timeout(settle)
        .map_err(|_| "git rev-parse did not close its output in time".to_owned())??;
    let stderr = err_receiver
        .recv_timeout(settle)
        .map_err(|_| "git rev-parse did not close its error output in time".to_owned())??;
    match code {
        Some(0) => toplevel(workspace, &stdout),
        Some(128) if stderr.starts_with(NOT_A_REPOSITORY) => Ok(Placement::Outside),
        _ => {
            let first = String::from_utf8_lossy(&stderr);
            let first = first.lines().next().unwrap_or("").trim();
            Err(format!(
                "git rev-parse did not confirm a work tree (exit {}: {first}); nothing was written",
                code.map_or_else(|| "by signal".to_owned(), |code| code.to_string())
            ))
        }
    }
}

/// Remove every inherited `GIT_*` variable from `command`, so Git addresses
/// only the directory it runs in. Inherited repository and config addressing
/// (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_CONFIG_*`) could point
/// elsewhere.
pub(crate) fn remove_git_environment(command: &mut Command) {
    remove_git_environment_named(command, std::env::vars_os().map(|(name, _)| name));
}

pub(crate) fn remove_git_environment_named(
    command: &mut Command,
    names: impl IntoIterator<Item = OsString>,
) {
    for name in names {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
}

/// Stderr's first [`MAX_GIT_STDERR_BYTES`], enough to classify Git's answer.
fn read_prefix(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_GIT_STDERR_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read git error output: {error}"))?;
    Ok(bytes)
}

fn read_capped(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_GIT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read git output: {error}"))?;
    if bytes.len() > MAX_GIT_BYTES {
        return Err(format!("git output is over {MAX_GIT_BYTES} bytes"));
    }
    Ok(bytes)
}

/// Git's top-level answer for `workspace`: one absolute directory that
/// contains it.
fn toplevel(workspace: &Path, stdout: &[u8]) -> Result<Placement, String> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| "Git reported a work tree root that is not UTF-8".to_owned())?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() || text.contains('\n') || !Path::new(text).is_absolute() {
        return Err(
            "Git reported no single absolute work tree root; nothing was written".to_owned(),
        );
    }
    let root = fs::canonicalize(text)
        .map_err(|error| format!("cannot resolve the Git work tree root {text}: {error}"))?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(format!(
            "the Git work tree root {} is not a directory",
            root.display()
        ));
    }
    if !workspace.starts_with(&root) {
        return Err(format!(
            "the workspace is outside the Git work tree {}; nothing was written",
            root.display()
        ));
    }
    Ok(Placement::Worktree(root))
}

/// `workspace` relative to the guidance root: `.` for the root itself, else
/// slash-separated components that pass [`valid_cover`].
pub fn cover_path(root: &Path, workspace: &Path) -> Result<String, String> {
    let relative = workspace
        .strip_prefix(root)
        .map_err(|_| "the workspace is outside the guidance root".to_owned())?;
    if relative.as_os_str().is_empty() {
        return Ok(".".to_owned());
    }
    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("the workspace path is not a plain relative path".to_owned());
        };
        parts.push(
            name.to_str()
                .ok_or("the workspace path is not UTF-8")?
                .to_owned(),
        );
    }
    let text = parts.join("/");
    valid_cover(&text)?;
    Ok(text)
}

/// Accepts `.` or slash-separated components of ASCII letters, digits and
/// `- _ . + @`, none empty or leading with `.`. Whitespace, controls,
/// backticks, Unicode and other punctuation are refused, so a listed path
/// can never be misread inside the block.
fn valid_cover(text: &str) -> Result<(), String> {
    if text == "." {
        return Ok(());
    }
    if text.is_empty() || text.len() > MAX_COVER_BYTES {
        return Err(format!(
            "a covered path must be 1 to {MAX_COVER_BYTES} bytes"
        ));
    }
    let safe = text.split('/').all(|component| {
        !component.is_empty()
            && !component.starts_with('.')
            && component
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '@'))
    });
    if safe {
        Ok(())
    } else {
        Err(format!(
            "covered path {text:?} is unsafe; use ASCII letters, digits and - _ . + @ components"
        ))
    }
}

/// The paths a block's list marker covers. `None` when the block has no
/// marker (a legacy block). A duplicate, malformed, empty, oversize or
/// repeated-path list refuses.
pub fn parse_covers(block: &str) -> Result<Option<Vec<String>>, String> {
    let mut markers = block
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("<!-- oxide:workspaces"));
    let Some(line) = markers.next() else {
        return Ok(None);
    };
    if markers.next().is_some() {
        return Err("the Oxide block has more than one covered-workspace list".to_owned());
    }
    let inner = line
        .strip_prefix(COVERS_OPEN)
        .and_then(|rest| rest.strip_suffix(COVERS_CLOSE))
        .ok_or("the covered-workspace list marker is malformed")?;
    let covers: Vec<String> = inner.split_whitespace().map(str::to_owned).collect();
    if covers.is_empty() {
        return Err("the covered-workspace list is empty".to_owned());
    }
    if covers.len() > MAX_COVERS {
        return Err(format!(
            "the covered-workspace list has more than {MAX_COVERS} paths"
        ));
    }
    let mut unique = covers.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != covers.len() {
        return Err("the covered-workspace list repeats a path".to_owned());
    }
    for cover in &covers {
        valid_cover(cover)?;
    }
    Ok(Some(covers))
}

/// Whether the block covers `cover`. A legacy block without a list covers
/// `.` only when the root holds a regular Cargo manifest. Nothing else is
/// inferred.
fn covers_in(bytes: &[u8], cover: &str, root_manifest: bool) -> Result<bool, String> {
    let Some(text) = adopt::block_text(bytes)? else {
        return Ok(false);
    };
    Ok(match parse_covers(text)? {
        Some(list) => list.iter().any(|path| path == cover),
        None => cover == "." && root_manifest,
    })
}

/// The list after one init run: the existing block's paths, or a legacy
/// block's implied `.` when the root holds a Cargo manifest, plus the current
/// workspace. Sorted and unique.
pub fn merged_covers(
    block: Option<&str>,
    root_manifest: bool,
    current: &str,
) -> Result<Vec<String>, String> {
    let mut covers = match block {
        None => Vec::new(),
        Some(block) => match parse_covers(block)? {
            Some(covers) => covers,
            None if root_manifest => vec![".".to_owned()],
            None => Vec::new(),
        },
    };
    covers.push(current.to_owned());
    covers.sort();
    covers.dedup();
    if covers.len() > MAX_COVERS {
        return Err(format!(
            "the covered-workspace list would have more than {MAX_COVERS} paths"
        ));
    }
    Ok(covers)
}

/// Whether the root's guidance covers `workspace`, and the workspace's own
/// `AGENTS.md` in the nested case. Uncertain reads are never adoption.
pub fn adoption(workspace: &Path) -> Adoption {
    let root = match placement(workspace) {
        Ok(Placement::Outside) => {
            return match block_in(&workspace.join(AGENTS)) {
                Ok(true) => Adoption::Adopted,
                Ok(false) => Adoption::NotAdopted,
                Err(reason) => Adoption::Uncertain(reason),
            };
        }
        Ok(Placement::Worktree(root)) => root,
        Err(reason) => return Adoption::Uncertain(reason),
    };
    let cover = match cover_path(&root, workspace) {
        Ok(cover) => cover,
        Err(reason) => return Adoption::Uncertain(reason),
    };
    let manifest = is_regular(&root.join("Cargo.toml"));
    let root_covers = match adopt::read_bounded(&root.join(AGENTS)) {
        Ok(Some(bytes)) => covers_in(&bytes, &cover, manifest),
        Ok(None) => Ok(false),
        Err(reason) => Err(reason),
    };
    match root_covers {
        Ok(true) => Adoption::Adopted,
        Err(reason) => Adoption::Uncertain(reason),
        Ok(false) if root == workspace => Adoption::NotAdopted,
        Ok(false) => match block_in(&workspace.join(AGENTS)) {
            Ok(true) => Adoption::LegacyNested(workspace.join(AGENTS)),
            Ok(false) => Adoption::NotAdopted,
            Err(reason) => Adoption::Uncertain(reason),
        },
    }
}

/// Whether the file at `path` holds an Oxide block. A missing file has none; a
/// file that cannot be read, or whose markers are damaged, is an error. Damage
/// is neither adoption nor absence, so `init` must not treat it as absence.
fn block_in(path: &Path) -> Result<bool, String> {
    match adopt::read_bounded(path)? {
        None => Ok(false),
        Some(bytes) => Ok(adopt::find_block(&bytes)?.is_some()),
    }
}

/// A line for readiness when the workspace's own `AGENTS.md` still holds an
/// Oxide block or markers that the Git root now owns. Oxide keeps it and
/// never rewrites it.
pub fn retained_report(workspace: &Path, root: &Path) -> Option<String> {
    if workspace == root {
        return None;
    }
    let path = workspace.join(AGENTS);
    match adopt::read_bounded(&path) {
        Ok(None) => None,
        Ok(Some(bytes)) => match adopt::find_block(&bytes) {
            Ok(None) => None,
            Ok(Some(_)) => Some(format!(
                "retained: {} holds an older Oxide block; oxide init left it unchanged",
                path.display()
            )),
            Err(_) => Some(format!(
                "retained: {} has malformed Oxide markers; oxide init left it unchanged",
                path.display()
            )),
        },
        Err(reason) => Some(format!("{}: {reason}; left unchanged", path.display())),
    }
}

fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

/// An exclusive advisory lock on the guidance root, held until drop. It
/// serialises cooperating `oxide init`/`new` runs. It does not stop unrelated
/// edits to the file.
pub struct RootLock {
    _held: File,
}

/// Lock the root directory, waiting up to [`LOCK_WAIT`] without blocking.
pub fn lock_root(root: &Path) -> Result<RootLock, String> {
    let held = File::open(root)
        .map_err(|error| format!("cannot open {} for its lock: {error}", root.display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: `held` is an open descriptor this call owns. LOCK_NB never
        // blocks, and the lock ends when `held` closes.
        if unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(RootLock { _held: held });
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(format!("cannot lock {}: {error}", root.display()));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "another oxide init holds the guidance lock for {} (waited 10 s); nothing was written",
                root.display()
            ));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("guidance-{label}-{:x}", std::process::id()));
        fs::create_dir(&dir).expect("scratch dir");
        fs::canonicalize(dir).expect("canonical scratch")
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }

    fn list(text: &str) -> Vec<&str> {
        text.split(' ').collect()
    }

    #[test]
    fn covered_paths_are_safe_slash_separated_components() {
        for good in [".", "crates/app", "a-b_c.d+e@f/x"] {
            assert!(valid_cover(good).is_ok(), "{good}");
        }
        for bad in [
            "", "a//b", "a/", "/a", "a b", "a`b", "a\nb", "a\tb", "a\\b", "../x", ".hidden",
            "a/.b", "x-->", "café", "a:b",
        ] {
            assert!(valid_cover(bad).is_err(), "{bad:?}");
        }
        let long = "a".repeat(MAX_COVER_BYTES + 1);
        assert!(valid_cover(&long).is_err());
    }

    #[test]
    fn list_marker_parses_once_and_refuses_damage() {
        let block = "<!-- oxide:begin -->\n<!-- oxide:workspaces: . crates/app -->\nx\n<!-- oxide:end -->\n";
        assert_eq!(
            parse_covers(block).unwrap(),
            Some(vec![".".to_owned(), "crates/app".to_owned()])
        );
        let legacy = "<!-- oxide:begin -->\nx\n<!-- oxide:end -->\n";
        assert_eq!(parse_covers(legacy).unwrap(), None);
        for damaged in [
            "<!-- oxide:workspaces: a -->\n<!-- oxide:workspaces: b -->\n",
            "<!-- oxide:workspaces: a\n",
            "<!-- oxide:workspaces: -->\n",
            "<!-- oxide:workspaces: a a -->\n",
            "<!-- oxide:workspaces: a b`c -->\n",
            "<!-- oxide:workspaces: .hidden -->\n",
        ] {
            assert!(parse_covers(damaged).is_err(), "{damaged:?}");
        }
        let many: Vec<String> = (0..=MAX_COVERS).map(|n| format!("p{n}")).collect();
        assert!(parse_covers(&format!("<!-- oxide:workspaces: {} -->", many.join(" "))).is_err());
    }

    #[test]
    fn merging_adds_the_current_workspace_and_upgrades_legacy_blocks() {
        let listed = "<!-- oxide:workspaces: c/d -->";
        assert_eq!(
            merged_covers(Some(listed), false, "a").unwrap(),
            list("a c/d")
        );
        // A legacy root block keeps `.` only with a regular root manifest.
        let legacy = "<!-- oxide:begin -->\nx\n<!-- oxide:end -->";
        assert_eq!(merged_covers(Some(legacy), true, "a").unwrap(), list(". a"));
        assert_eq!(merged_covers(Some(legacy), false, "a").unwrap(), list("a"));
        // Repeating the same workspace adds nothing.
        assert_eq!(
            merged_covers(Some(listed), false, "c/d").unwrap(),
            list("c/d")
        );
        assert_eq!(merged_covers(None, false, ".").unwrap(), list("."));
    }

    #[test]
    fn nested_layouts_resolve_to_the_git_root_and_adopt_by_list() {
        let dir = scratch("nested");
        let repo = dir.join("repo");
        let member = repo.join("crates/app");
        fs::create_dir_all(&member).expect("member");
        git(&repo, &["init", "-q"]);
        fs::write(
            repo.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n",
        )
        .expect("root");
        fs::write(member.join("Cargo.toml"), "[package]\nname = \"app\"\n").expect("member");
        let root = fs::canonicalize(&repo).expect("root");
        let workspace = fs::canonicalize(&member).expect("member");
        assert_eq!(
            placement(&workspace).unwrap(),
            Placement::Worktree(root.clone())
        );
        assert_eq!(cover_path(&root, &workspace).unwrap(), "crates/app");
        assert_eq!(adoption(&workspace), Adoption::NotAdopted);
        fs::write(
            root.join(AGENTS),
            "<!-- oxide:begin -->\n<!-- oxide:workspaces: crates/app -->\nx\n<!-- oxide:end -->\n",
        )
        .expect("guidance");
        assert_eq!(adoption(&workspace), Adoption::Adopted);
        // A block that lists only the root does not cover the member.
        fs::write(
            root.join(AGENTS),
            "<!-- oxide:begin -->\n<!-- oxide:workspaces: . -->\nx\n<!-- oxide:end -->\n",
        )
        .expect("guidance");
        assert_eq!(adoption(&workspace), Adoption::NotAdopted);
        assert_eq!(adoption(&root), Adoption::Adopted);
        // A legacy block in the member's own AGENTS.md is not adoption.
        fs::write(
            root.join(AGENTS),
            "<!-- oxide:begin -->\nx\n<!-- oxide:end -->\n",
        )
        .expect("guidance");
        fs::write(
            member.join(AGENTS),
            "<!-- oxide:begin -->\nx\n<!-- oxide:end -->\n",
        )
        .expect("nested");
        assert_eq!(
            adoption(&workspace),
            Adoption::LegacyNested(member.join(AGENTS))
        );
        assert!(retained_report(&member, &root).is_some());
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_non_repository_is_outside_git_and_a_bad_probe_refuses() {
        let dir = scratch("outside");
        assert_eq!(placement(&dir).unwrap(), Placement::Outside);
        assert_eq!(adoption(&dir), Adoption::NotAdopted);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn damaged_guidance_is_uncertain_not_absent() {
        let dir = scratch("damaged");
        // A begin marker with no end is damage: `init` must refuse, not write
        // a second block as if none existed.
        fs::write(
            dir.join(AGENTS),
            "<!-- oxide:begin -->\n<!-- oxide:workspaces: . -->\n",
        )
        .expect("damaged guidance");
        assert!(matches!(adoption(&dir), Adoption::Uncertain(_)));
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn inherited_git_addressing_is_removed_before_git_runs() {
        let mut command = Command::new("git");
        let inherited = ["GIT_DIR", "GIT_CONFIG_COUNT", "GIT_INDEX_FILE", "PATH"];
        remove_git_environment_named(&mut command, inherited.map(OsString::from));
        let removed = |name: &str| {
            command
                .get_envs()
                .any(|(key, value)| key == name && value.is_none())
        };
        assert!(removed("GIT_DIR"));
        assert!(removed("GIT_CONFIG_COUNT"));
        assert!(removed("GIT_INDEX_FILE"));
        assert!(!command.get_envs().any(|(key, _)| key == "PATH"));
    }

    #[test]
    fn the_root_lock_is_taken_and_released() {
        let dir = scratch("lock");
        let first = lock_root(&dir).expect("first lock");
        drop(first);
        assert!(lock_root(&dir).is_ok());
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
