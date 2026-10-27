//! Bounded git source verification for one locked dependency checkout.
//!
//! Owning design: `docs/knowledge/contracts/cold-slot-seeding.md` §4.3. The
//! seed side runs the reviewed effect-free git protocol in
//! `src/bin/oxide/freshness.rs` (`freshness::Git`): `GIT_OPTIONAL_LOCKS=0`,
//! `GIT_TERMINAL_PROMPT=0`, `-c submodule.recurse=0`, `-c core.fsmonitor=false`,
//! bounded call count, wall time, stdout and settle window, and an unconfirmed
//! child closure that refuses. Because this adapter lives in the library crate
//! it reproduces that protocol instead of calling the front-door binary; it
//! additionally passes `-C <verified root>` only and clears `GIT_DIR`,
//! `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_CONFIG_*` from the child
//! environment, so an inherited variable cannot re-root the repository.
//!
//! Verification requires the lock's full revision, a clean checkout (modulo
//! Cargo's own untracked `.cargo-ok` marker, which is never evidence), no
//! submodule (a declared gitlink cannot present an empty status and its content
//! is not part of the tree this digest binds), and a tracked tree whose content
//! hashes equal the locked revision's tree. The digest is derived from the locked
//! tree's own object ids, so present cleanliness alone is never accepted as
//! evidence of what the checkout contains.

use super::digest::{validate_path, ContentHash, DigestScheme, EntryKind, SourceTree, TreeDigest};
use super::SourceLimits;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Cumulative git calls in one verification pass.
pub const MAX_GIT_CALLS: usize = 64;
/// Cumulative git stdout bytes in one verification pass.
pub const MAX_GIT_STDOUT_BYTES: u64 = 16 * 1024 * 1024;
/// One git call's wall limit.
pub const GIT_CALL_TIME: Duration = Duration::from_secs(30);
/// One verification pass's cumulative wall limit.
pub const GIT_TOTAL_TIME: Duration = Duration::from_secs(60);
/// Time after a child is reaped for pipes held by anything it started.
pub const GIT_SETTLE_TIME: Duration = Duration::from_secs(2);
/// Bound on the path list written to `git hash-object --stdin-paths`.
const MAX_STDIN_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 64 * 1024;
const CLOSE_BOUND: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(5);
const CHUNK: usize = 64 * 1024;
/// Cargo's own marker at the root of a git checkout.
const CARGO_OK: &str = ".cargo-ok";

/// Why a git dependency is not eligible for seeding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitRefusal {
    /// The checkout is not at the lock's full revision.
    WrongRevision,
    /// The checkout has a tracked or untracked change.
    Dirty,
    /// The checkout has an initialized or unclassifiable submodule.
    Submodule,
    /// A tracked entry is a symlink or another type the seed cannot copy.
    Unsupported,
    /// git failed, or its output could not be interpreted.
    Unreadable,
    /// A call, time, byte or entry bound was exhausted.
    Budget,
    /// A child did not close within its clock, so its custody is unknown.
    Unconfirmed,
}

impl fmt::Display for GitRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongRevision => "the checkout is not at the locked revision",
            Self::Dirty => "the checkout has changes",
            Self::Submodule => "the checkout has a submodule",
            Self::Unsupported => "the checkout has an unsupported tracked entry",
            Self::Unreadable => "the checkout cannot be read",
            Self::Budget => "the git verification budget was exhausted",
            Self::Unconfirmed => "a git child did not close within its clock",
        })
    }
}

/// One verified git source.
#[derive(Clone, Debug)]
pub struct GitSource {
    /// The locked revision, lowercase hexadecimal.
    pub revision: String,
    /// Canonical digest of the tracked tree at that revision.
    pub digest: TreeDigest,
    /// Tracked regular files.
    pub files: u64,
    /// Sum of the tracked files' sizes.
    pub content_bytes: u64,
}

/// Bounded git verification context. All budgets are fixed by the design;
/// tests may narrow them.
#[derive(Clone, Debug)]
pub struct GitVerifier {
    program: PathBuf,
    calls: usize,
    waited: Duration,
    stdout_bytes: u64,
    call_time: Duration,
    total_time: Duration,
    settle_time: Duration,
    stdout_limit: u64,
}

impl Default for GitVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl GitVerifier {
    /// A verifier using the `git` on `PATH` and the design's bounds.
    pub fn new() -> Self {
        Self {
            program: PathBuf::from("git"),
            calls: 0,
            waited: Duration::ZERO,
            stdout_bytes: 0,
            call_time: GIT_CALL_TIME,
            total_time: GIT_TOTAL_TIME,
            settle_time: GIT_SETTLE_TIME,
            stdout_limit: MAX_GIT_STDOUT_BYTES,
        }
    }

    /// Narrow every budget for a test.
    #[cfg(test)]
    pub(super) fn for_test(
        program: PathBuf,
        calls: usize,
        call_time: Duration,
        total_time: Duration,
        settle_time: Duration,
        stdout_limit: u64,
    ) -> Self {
        Self {
            program,
            calls,
            waited: Duration::ZERO,
            stdout_bytes: 0,
            call_time,
            total_time,
            settle_time,
            stdout_limit,
        }
    }

    /// Verify one checkout against its locked full revision.
    pub fn verify(
        &mut self,
        root: &Path,
        locked_revision: &str,
        limits: &SourceLimits,
    ) -> Result<GitSource, GitRefusal> {
        let locked = normalized_revision(locked_revision).ok_or(GitRefusal::WrongRevision)?;
        let output = self.run(root, &["rev-parse", "--verify", "HEAD"], None)?;
        let head = text(&output)?;
        let revision = normalized_revision(head.trim()).ok_or(GitRefusal::Unreadable)?;
        if revision != locked {
            return Err(GitRefusal::WrongRevision);
        }
        let object_bytes = revision.len() / 2;
        let status = self.run(
            root,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--no-renames",
                "--ignore-submodules=none",
            ],
            None,
        )?;
        if !clean_status(&status) {
            return Err(GitRefusal::Dirty);
        }
        let listing = self.run(root, &["ls-tree", "-r", "-l", "-z", "HEAD"], None)?;
        let listing = Listing::parse(&listing, object_bytes, limits)?;
        let paths: Vec<String> = listing.files.iter().map(|file| file.path.clone()).collect();
        let objects = self.hash_objects(root, &paths, object_bytes)?;
        let mut expected = SourceTree::new();
        let mut actual = SourceTree::new();
        let mut content_bytes: u64 = 0;
        for (file, object) in listing.files.iter().zip(objects.iter()) {
            let executable = file.executable;
            expected
                .insert_explicit(
                    &file.path,
                    EntryKind::File {
                        executable,
                        size: file.size,
                        content: ContentHash::GitObject(file.object.clone()),
                    },
                    limits.max_path_bytes,
                )
                .map_err(|_| GitRefusal::Unreadable)?;
            let metadata =
                fs::symlink_metadata(root.join(&file.path)).map_err(|_| GitRefusal::Unreadable)?;
            if !metadata.file_type().is_file() {
                return Err(GitRefusal::Unsupported);
            }
            if metadata.size() != file.size
                || (metadata.permissions().mode() & 0o111 != 0) != executable
            {
                return Err(GitRefusal::Dirty);
            }
            content_bytes = content_bytes
                .checked_add(file.size)
                .ok_or(GitRefusal::Budget)?;
            actual
                .insert_explicit(
                    &file.path,
                    EntryKind::File {
                        executable,
                        size: file.size,
                        content: ContentHash::GitObject(object.clone()),
                    },
                    limits.max_path_bytes,
                )
                .map_err(|_| GitRefusal::Unreadable)?;
        }
        if expected != actual {
            return Err(GitRefusal::Dirty);
        }
        let digest = actual
            .digest(DigestScheme::GitSource)
            .map_err(|_| GitRefusal::Unreadable)?;
        Ok(GitSource {
            revision,
            digest,
            files: u64::try_from(paths.len()).unwrap_or(u64::MAX),
            content_bytes,
        })
    }

    fn hash_objects(
        &mut self,
        root: &Path,
        paths: &[String],
        object_bytes: usize,
    ) -> Result<Vec<Vec<u8>>, GitRefusal> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        if paths.iter().any(|path| path.contains('\n')) {
            return Err(GitRefusal::Unsupported);
        }
        let list = paths.join("\n") + "\n";
        if u64::try_from(list.len()).unwrap_or(u64::MAX) > MAX_STDIN_BYTES {
            return Err(GitRefusal::Budget);
        }
        let output = self.run(
            root,
            &["hash-object", "--no-filters", "--stdin-paths"],
            Some(list.as_bytes()),
        )?;
        let text = text(&output)?;
        let objects: Vec<Vec<u8>> = text
            .lines()
            .map(|line| parse_object(line, object_bytes))
            .collect::<Result<_, _>>()?;
        if objects.len() != paths.len() {
            return Err(GitRefusal::Unreadable);
        }
        Ok(objects)
    }

    fn run(
        &mut self,
        root: &Path,
        args: &[&str],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, GitRefusal> {
        if self.calls >= MAX_GIT_CALLS {
            return Err(GitRefusal::Budget);
        }
        let remaining = self.total_time.saturating_sub(self.waited);
        if remaining.is_zero() {
            return Err(GitRefusal::Budget);
        }
        self.calls += 1;
        let limit = self.call_time.min(remaining);
        let mut command = git_command(&self.program, root, args);
        command
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|_| GitRefusal::Unreadable)?;
        let writer = match input {
            None => None,
            Some(bytes) => {
                let mut stdin = child.stdin.take().ok_or(GitRefusal::Unreadable)?;
                let bytes = bytes.to_vec();
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(stdin.write_all(&bytes));
                });
                Some(receiver)
            }
        };
        let stdout = child.stdout.take().ok_or(GitRefusal::Unreadable)?;
        let stderr = child.stderr.take().ok_or(GitRefusal::Unreadable)?;
        let (out_sender, out_receiver) = mpsc::channel();
        let stdout_limit = self.stdout_limit;
        std::thread::spawn(move || {
            let _ = out_sender.send(read_capped(stdout, stdout_limit));
        });
        let (err_sender, err_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = err_sender.send(read_capped(stderr, MAX_STDERR_BYTES));
        });
        let started = Instant::now();
        let deadline = started + limit;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() >= deadline => {
                    close(&mut child);
                    break Err(GitRefusal::Unconfirmed);
                }
                Ok(None) => std::thread::sleep(POLL),
                Err(_) => {
                    close(&mut child);
                    break Err(GitRefusal::Unreadable);
                }
            }
        };
        self.waited = self.waited.saturating_add(started.elapsed());
        let status = status?;
        // One settle window for all readers, never past the call's own
        // deadline: each wait takes only what remains of the same window.
        let settle_deadline = (Instant::now() + self.settle_time).min(deadline);
        let left = || settle_deadline.saturating_duration_since(Instant::now());
        let settling = Instant::now();
        let out = out_receiver.recv_timeout(left());
        let err = err_receiver.recv_timeout(left());
        let written = writer
            .as_ref()
            .map(|receiver| receiver.recv_timeout(left()));
        self.waited = self.waited.saturating_add(settling.elapsed());
        if Instant::now() > deadline {
            return Err(GitRefusal::Unconfirmed);
        }
        let (stdout, overflow) = out
            .map_err(|_| GitRefusal::Unconfirmed)?
            .map_err(|_| GitRefusal::Unreadable)?;
        // The stderr reader must still settle, but its text is diagnostic only
        // and no refusal carries it.
        let _stderr = err
            .map_err(|_| GitRefusal::Unconfirmed)?
            .map(|(bytes, _)| bytes)
            .unwrap_or_default();
        let written = match written {
            None => Ok(()),
            Some(result) => result.map_err(|_| GitRefusal::Unconfirmed)?,
        };
        if !status.success() || written.is_err() {
            return Err(GitRefusal::Unreadable);
        }
        if overflow {
            return Err(GitRefusal::Budget);
        }
        self.stdout_bytes = self
            .stdout_bytes
            .saturating_add(u64::try_from(stdout.len()).unwrap_or(u64::MAX));
        if self.stdout_bytes > self.stdout_limit {
            return Err(GitRefusal::Budget);
        }
        Ok(stdout)
    }
}

/// The exact command every seed-side git call uses.
pub(crate) fn git_command(program: &Path, root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .args([
            "-c",
            "submodule.recurse=0",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    for (name, _) in std::env::vars_os() {
        if clears(&name) {
            command.env_remove(&name);
        }
    }
    for name in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        command.env_remove(name);
    }
    command
}

/// Whether an inherited variable could re-root the repository or inject
/// configuration into the child.
pub(super) fn clears(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    matches!(
        bytes,
        b"GIT_DIR" | b"GIT_WORK_TREE" | b"GIT_INDEX_FILE" | b"GIT_CONFIG"
    ) || bytes.starts_with(b"GIT_CONFIG_")
}

fn close(child: &mut std::process::Child) {
    let _ = child.kill();
    let deadline = Instant::now() + CLOSE_BOUND;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(POLL),
            Err(_) => return,
        }
    }
}

fn read_capped(mut reader: impl Read, cap: u64) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; CHUNK];
    let mut total: u64 = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok((bytes, false));
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > cap {
            return Ok((bytes, true));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn text(bytes: &[u8]) -> Result<String, GitRefusal> {
    String::from_utf8(bytes.to_vec()).map_err(|_| GitRefusal::Unreadable)
}

fn normalized_revision(revision: &str) -> Option<String> {
    let bytes = revision.as_bytes();
    if !matches!(bytes.len(), 40 | 64) {
        return None;
    }
    let mut normalized = String::with_capacity(bytes.len());
    for byte in bytes {
        if !byte.is_ascii_hexdigit() {
            return None;
        }
        normalized.push(char::from(byte.to_ascii_lowercase()));
    }
    Some(normalized)
}

fn parse_object(text: &str, object_bytes: usize) -> Result<Vec<u8>, GitRefusal> {
    let bytes = text.as_bytes();
    if bytes.len() != object_bytes * 2 {
        return Err(GitRefusal::Unreadable);
    }
    let mut object = Vec::with_capacity(object_bytes);
    for pair in bytes.chunks_exact(2) {
        let high = hex_value(pair[0]).ok_or(GitRefusal::Unreadable)?;
        let low = hex_value(pair[1]).ok_or(GitRefusal::Unreadable)?;
        object.push((high << 4) | low);
    }
    Ok(object)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Cargo's own untracked marker is the only accepted status entry; anything
/// else is a change.
fn clean_status(status: &[u8]) -> bool {
    let mut fields = status
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    match (fields.next(), fields.next()) {
        (None, None) => true,
        (Some(entry), None) => entry == format!("?? {CARGO_OK}").as_bytes(),
        _ => false,
    }
}

struct ListedFile {
    path: String,
    executable: bool,
    size: u64,
    object: Vec<u8>,
}

struct Listing {
    files: Vec<ListedFile>,
}

impl Listing {
    fn parse(bytes: &[u8], object_bytes: usize, limits: &SourceLimits) -> Result<Self, GitRefusal> {
        let mut files = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut content_bytes: u64 = 0;
        let mut records = bytes.split(|byte| *byte == 0).peekable();
        while let Some(record) = records.next() {
            if record.is_empty() {
                if records.peek().is_none() {
                    break;
                }
                return Err(GitRefusal::Unreadable);
            }
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or(GitRefusal::Unreadable)?;
            let (header, path) = record.split_at(tab);
            let path = std::str::from_utf8(&path[1..]).map_err(|_| GitRefusal::Unreadable)?;
            validate_path(path, limits.max_path_bytes).map_err(|_| GitRefusal::Unreadable)?;
            if !seen.insert(path.to_owned()) {
                return Err(GitRefusal::Unreadable);
            }
            let header = std::str::from_utf8(header).map_err(|_| GitRefusal::Unreadable)?;
            let mut parts = header.split_ascii_whitespace();
            let mode = parts.next().ok_or(GitRefusal::Unreadable)?;
            let kind = parts.next().ok_or(GitRefusal::Unreadable)?;
            let object = parts.next().ok_or(GitRefusal::Unreadable)?;
            let size = parts.next().ok_or(GitRefusal::Unreadable)?;
            if parts.next().is_some() {
                return Err(GitRefusal::Unreadable);
            }
            match (mode, kind) {
                ("100644", "blob") | ("100755", "blob") => {
                    let size = size.parse::<u64>().map_err(|_| GitRefusal::Unreadable)?;
                    content_bytes = content_bytes.checked_add(size).ok_or(GitRefusal::Budget)?;
                    if content_bytes > limits.max_content_bytes {
                        return Err(GitRefusal::Budget);
                    }
                    files.push(ListedFile {
                        path: path.to_owned(),
                        executable: mode == "100755",
                        size,
                        object: parse_object(object, object_bytes)?,
                    });
                }
                // A declared submodule cannot present an empty status and its
                // content is not part of the locked tree this digest binds, so
                // a submodule-bearing checkout is ineligible.
                ("160000", "commit") => return Err(GitRefusal::Submodule),
                ("120000", "blob") => return Err(GitRefusal::Unsupported),
                _ => return Err(GitRefusal::Unsupported),
            }
        }
        if u64::try_from(files.len()).unwrap_or(u64::MAX) > limits.max_entries {
            return Err(GitRefusal::Budget);
        }
        Ok(Self { files })
    }
}
