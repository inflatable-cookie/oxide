//! Cross-checkout freshness gate (W-8).
//!
//! Cargo 1.97.1 keys workspace units by relative path and treats an input as
//! fresh when its mtime is not newer than the unit's dep-info. A slot shared
//! by two checkouts therefore needs the second checkout's differing files to
//! look newer than the slot's last build. Each slot records its last compile
//! origin; before Cargo runs, the gate sets mtime to now on every checkout
//! file whose content may differ from what the slot compiled and that is not
//! newer than the build anchor. On a checkout-path change it does the same
//! for every file of every local package under the checkout, as Cargo's
//! effect-free metadata names them under the guarded command's own lock,
//! network and config arguments. File contents, Cargo fingerprints
//! and the slot are never touched. Inputs outside Git or ignored by it are
//! not covered. Only a [verified Cargo release](VERIFIED_CARGO) shares a
//! slot; any other keeps it with one checkout.

use crate::cli::Resolution;
use crate::MIB;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Dirty files in one checkout and uncertain paths per slot. Beyond this the
/// gate refuses and the slot stays checkout-affine.
pub const MAX_PATHS: usize = 4096;
/// Files listed for the path-change step, counted across the superproject and
/// every initialized submodule.
const MAX_LISTED: usize = 200_000;
/// Declared submodules (gitlinks) the gate classifies. Depth is one.
pub const MAX_SUBMODULES: usize = 32;
/// Each `git` call: stdout bytes and lifetime. Runaway output or a call that
/// outlives its slice refuses the slot rather than yielding a partial
/// fingerprint.
const MAX_GIT_BYTES: u64 = 16 * MIB;
const GIT_CALL_TIME: Duration = Duration::from_secs(30);
/// Time after the child is reaped for pipes still held by anything it started
/// to close. A reader that misses it is detached: the gate refuses the slot
/// rather than waiting past its own clock.
const GIT_SETTLE_TIME: Duration = Duration::from_secs(2);
/// One snapshot or prepare: cumulative `git` work. The invocation count can
/// bind before the submodule cap; exhaustion refuses the slot.
const MAX_GIT_CALLS: usize = 64;
const GIT_TOTAL_TIME: Duration = Duration::from_secs(60);
/// A submodule's `.git` file, a `gitdir:` pointer.
const MAX_DOT_GIT_BYTES: u64 = 64 * 1024;
/// The mode Git records for a submodule in the index and in a tree.
const GITLINK_MODE: &str = "160000";
/// Version written with a content record that includes submodule state.
/// Version 0 (or a missing field) predates submodule coverage.
pub const CONTENT_VERSION: u32 = 1;
/// `cargo metadata` output and lifetime for the path-change step.
pub(crate) const MAX_METADATA_BYTES: u64 = 64 * MIB;
const MAX_METADATA_ERROR_BYTES: u64 = 64 * 1024;
const METADATA_TIME: Duration = Duration::from_secs(60);
const DELETED: &str = "deleted";
pub const LIMIT_NOTE: &str = "inputs outside Git or ignored by it are not checked";
/// Cargo's wording when `--locked` or `--frozen` stops it creating or
/// updating the lock file.
const LOCK_REFUSAL: &str = "was passed to prevent this";
/// Exact Cargo releases (`cargo -V` prefixes) whose rules the gate relies
/// on were checked against pinned Cargo source and fixture runs (oxide#091):
/// the `path_mtime <= reference_mtime` freshness rule, workspace-relative
/// unit paths, `cargo metadata` format 1, the lock and unknown `-Z` refusal
/// wording, stable `build.build-dir` and the legacy `deps/` / `incremental/`
/// layout. Any other release, older, newer or unknown, never shares a slot.
pub(crate) const VERIFIED_CARGO: [&str; 5] = [
    "cargo 1.95.0 (f2d3ce0bd ",
    "cargo 1.96.1 (356927216 ",
    "cargo 1.97.1 (c980f4866 ",
    "cargo 1.98.1 (797e8a9bc ",
    "cargo 1.99.1 (5f94df478 ",
];

/// Why the gate did not vouch for a slot.
#[derive(Debug)]
pub enum Refusal {
    /// The slot's history cannot be reconciled with this checkout: the
    /// command builds cold.
    Unvouched(String),
    /// Cargo refused the command's own `--locked` or `--frozen` request.
    /// Plain Cargo fails the same way, so the command ends with Cargo's
    /// message and exit code instead of building cold.
    Lock { code: u8, message: String },
}

impl From<String> for Refusal {
    fn from(reason: String) -> Self {
        Self::Unvouched(reason)
    }
}

impl From<&str> for Refusal {
    fn from(reason: &str) -> Self {
        Self::Unvouched(reason.to_owned())
    }
}

/// One declared Git submodule's worktree state at snapshot time. Only these
/// two states are covered; anything else refuses the slot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SubmoduleState {
    /// Positively absent, or an empty real directory with no `.git` entry:
    /// the gitlink was never initialized.
    #[serde(rename = "uninit")]
    Uninitialized,
    /// Initialized at the declared commit. `head` is the child's `HEAD` and
    /// `raw` a framed digest of its ordered cached paths and raw blob IDs,
    /// so the same commit reached through different bytes still differs.
    Exact { head: String, raw: String },
}

/// One checkout's content: HEAD's tree, the Git blob digest of every file
/// that differs from it (tracked changes and untracked files Git does not
/// ignore), and the state of every declared submodule. Paths are relative to
/// the repository top level.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Content {
    pub tree: String,
    pub dirty: BTreeMap<String, String>,
    /// Every gitlink the index and `HEAD` declare, in path order. A written
    /// record always carries this field, even when it is empty. Legacy
    /// records that lack it parse as empty but never vouch for a checkout
    /// that declares submodules.
    pub submodules: BTreeMap<String, SubmoduleState>,
    /// Whether the parsed record carried submodule state at all. Every fresh
    /// snapshot sets it; only a record that predates the field leaves it
    /// false. Never serialized, because writing always includes the field.
    #[serde(skip)]
    has_submodules: bool,
}

/// A content record's on-disk shape, kept apart only to tell a missing
/// `submodules` field apart from an empty one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentRecord {
    tree: String,
    dirty: BTreeMap<String, String>,
    #[serde(default)]
    submodules: Option<BTreeMap<String, SubmoduleState>>,
}

impl<'de> Deserialize<'de> for Content {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = ContentRecord::deserialize(deserializer)?;
        Ok(Self {
            tree: record.tree,
            dirty: record.dirty,
            has_submodules: record.submodules.is_some(),
            submodules: record.submodules.unwrap_or_default(),
        })
    }
}

/// What a slot last compiled.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// Canonical workspace root of the compiling checkout.
    pub checkout: PathBuf,
    pub content: Content,
    /// The record's content schema. Version 0 (or a missing field) predates
    /// submodule state, so it never vouches for a checkout with gitlinks.
    #[serde(default)]
    pub content_version: u32,
    /// Nanoseconds since the epoch, taken after Cargo's group was empty, so
    /// no unit in the slot has a newer dep-info.
    pub anchor_ns: u64,
    /// Paths whose compiled form in the slot may not match `content`: every
    /// path that differed at an earlier command (a command need not rebuild
    /// every unit) or changed while Cargo ran. The compiling checkout's own
    /// copies are already newer than any such unit, so they are refreshed
    /// only when another checkout takes the slot.
    pub uncertain: BTreeSet<String>,
}

/// What a slot's last command recorded.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Built {
    /// Git content the gate can reconcile with another checkout.
    Content(Origin),
    /// Every build since the slot was created came from this checkout, which
    /// has no Git content to record. Only this checkout reuses it, under
    /// Cargo's own checks; it is never shared.
    Affine(PathBuf),
}

/// The slot's history as the leasing command found it.
#[derive(Clone, Debug)]
pub enum Prior {
    /// A new slot: nothing compiled yet.
    Fresh,
    /// The last command recorded this and nothing ran since.
    Known(Built),
    /// A command died after Cargo started or could not record its build, or
    /// the slot predates the gate. Nothing says what it holds, so it is never
    /// reused: the command evicts it and builds cold.
    Unknown,
}

impl Prior {
    /// The other checkout whose content this slot holds, if any.
    pub fn foreign(&self, checkout: &Path) -> Option<PathBuf> {
        match self {
            Self::Known(Built::Content(origin)) if origin.checkout != checkout => {
                Some(origin.checkout.clone())
            }
            Self::Known(Built::Affine(owner)) if owner != checkout => Some(owner.clone()),
            _ => None,
        }
    }

    /// What the slot keeps when Cargo never ran.
    pub fn unchanged(&self) -> Option<Built> {
        match self {
            Self::Known(built) => Some(built.clone()),
            Self::Fresh | Self::Unknown => None,
        }
    }
}

/// The gate's work before Cargo, kept to record the new origin afterwards.
pub struct Gate {
    /// `None`: a checkout without Git content or verified Cargo; the slot
    /// stays affine.
    before: Option<(PathBuf, Content)>,
    /// Paths that differed from the prior origin.
    changed: BTreeSet<String>,
    carried: BTreeSet<String>,
    /// The slot was last compiled by another checkout.
    pub cross: bool,
    pub refreshed: usize,
    pub packages: usize,
    /// Why a new slot stays with this checkout, when Cargo is unverified.
    note: Option<String>,
}

impl Gate {
    fn affine() -> Self {
        Self {
            before: None,
            changed: BTreeSet::new(),
            carried: BTreeSet::new(),
            cross: false,
            refreshed: 0,
            packages: 0,
            note: None,
        }
    }

    /// Text for the summary line when the gate did visible work.
    pub fn describe(&self) -> Option<String> {
        if let Some(note) = &self.note {
            Some(format!(
                "freshness gate off: {note}; the slot stays with this checkout"
            ))
        } else if self.cross {
            Some(format!(
                "freshness gate refreshed {} file(s) from another checkout's build ({} package(s) for the path change); {LIMIT_NOTE}",
                self.refreshed, self.packages
            ))
        } else if self.refreshed > 0 {
            Some(format!(
                "freshness gate refreshed {} file(s)",
                self.refreshed
            ))
        } else {
            None
        }
    }

    /// What to record once Cargo's group is empty, or `None` when nothing
    /// can be trusted (the next command then evicts the slot).
    pub fn finish(self, checkout: &Path) -> Option<Built> {
        let anchor_ns = now_ns();
        let Some((top, before)) = self.before else {
            return Some(Built::Affine(checkout.to_path_buf()));
        };
        let mut git = Git::new();
        let after = snapshot_with(&top, &mut git).ok()?.content;
        let mut uncertain = self.carried;
        uncertain.extend(self.changed);
        uncertain.extend(differing(&mut git, &top, &before, &after).ok()?);
        if uncertain.len() > MAX_PATHS {
            return None;
        }
        Some(Built::Content(Origin {
            checkout: checkout.to_path_buf(),
            content: after,
            content_version: CONTENT_VERSION,
            anchor_ns,
            uncertain,
        }))
    }
}

/// Run the gate for `checkout` before Cargo. `request` is the guarded
/// command's resolution request (`cli::resolution`). An error means the
/// caller must not run Cargo on the slot.
pub fn prepare(checkout: &Path, prior: &Prior, request: &Resolution) -> Result<Gate, Refusal> {
    match prior {
        Prior::Unknown => Err(Refusal::Unvouched(
            "the slot has no trusted build record".to_owned(),
        )),
        Prior::Known(Built::Affine(owner)) if owner == checkout => Ok(Gate::affine()),
        Prior::Known(Built::Affine(_)) => Err(Refusal::Unvouched(
            "the slot holds another checkout's unshared build".to_owned(),
        )),
        // Nothing compiled yet: a checkout without Git content, or whose
        // Cargo is unverified, stays affine.
        Prior::Fresh => Ok(match verified_cargo() {
            Err(reason) => Gate {
                note: Some(reason),
                ..Gate::affine()
            },
            Ok(_) => match snapshot(checkout) {
                Ok(snapshot) => Gate {
                    before: Some((snapshot.top, snapshot.content)),
                    ..Gate::affine()
                },
                Err(_) => Gate::affine(),
            },
        }),
        Prior::Known(Built::Content(origin)) => {
            let mut git = Git::new();
            let current = snapshot_with(checkout, &mut git)?;
            compatible(&origin, &current.content)?;
            let changed = differing(&mut git, &current.top, &origin.content, &current.content)?;
            let cross = origin.checkout != checkout;
            // Refreshing relies on Cargo's rules; an unchanged repeat does not.
            if cross || !changed.is_empty() {
                verified_cargo()?;
            }
            let mut stale: BTreeSet<&String> = changed.iter().collect();
            if cross {
                stale.extend(&origin.uncertain);
            }
            // A changed gitlink is a directory: its path is skipped below,
            // and an initialized child's own tracked files carry the change.
            let mut refresh_paths: BTreeSet<String> =
                stale.iter().map(|path| (*path).clone()).collect();
            for path in &changed {
                if let Some(SubmoduleState::Exact { .. }) = current.content.submodules.get(path) {
                    if let Some(files) = current.children.get(path) {
                        refresh_paths.extend(files.iter().cloned());
                    }
                }
            }
            let mut refreshed = refresh(
                &current.top,
                &refresh_paths,
                origin.anchor_ns,
                &current.content.submodules,
            )?;
            let mut packages = 0;
            if cross {
                let metadata = cargo_metadata(checkout, request)?;
                let (count, files) = local_package_files(&mut git, &current, &metadata)?;
                packages = count;
                refreshed += refresh(
                    &current.top,
                    &files,
                    origin.anchor_ns,
                    &current.content.submodules,
                )?;
            }
            Ok(Gate {
                before: Some((current.top, current.content)),
                changed,
                carried: origin.uncertain.clone(),
                cross,
                refreshed,
                packages,
                note: None,
            })
        }
    }
}

/// A content record vouches for a checkout only when its schema covers what
/// the checkout declares. A record without submodule state, and a checkout
/// with gitlinks, are not equivalent even when both maps are empty: version 0
/// predates the field, and a version-1 record that omitted it is treated the
/// same way.
fn compatible(origin: &Origin, current: &Content) -> Result<(), Refusal> {
    if !matches!(origin.content_version, 0 | CONTENT_VERSION) {
        return Err("the slot's content record has an unknown version".into());
    }
    if !current.submodules.is_empty()
        && !(origin.content_version == CONTENT_VERSION && origin.content.has_submodules)
    {
        return Err("the slot's origin lacks submodule state".into());
    }
    Ok(())
}

/// `cargo -V` in the inherited working directory, where the guarded Cargo
/// and the gate's metadata call resolve their toolchain (a member's
/// `rust-toolchain.toml` included), never the workspace root.
fn version_command() -> Command {
    let mut command = Command::new(crate::CARGO);
    command.arg("-V");
    command
}

/// The first `cargo -V` line of the command's own Cargo when it is a
/// [verified release](VERIFIED_CARGO); otherwise why not.
pub(crate) fn verified_cargo() -> Result<String, String> {
    let output = version_command()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    let version = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if !output.status.success() || version.is_empty() {
        return Err("Cargo did not report its version".to_owned());
    }
    if is_verified(&version) {
        Ok(version)
    } else {
        Err(format!("{version} is not a Cargo release Oxide verified"))
    }
}

fn is_verified(version: &str) -> bool {
    VERIFIED_CARGO
        .iter()
        .any(|prefix| version.starts_with(prefix))
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// Bounded access to the `git` binary for one snapshot or prepare.
///
/// Every call carries `GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`,
/// `-c submodule.recurse=0` and `-c core.fsmonitor=false`: reading the index
/// never rewrites it, no credential prompt can block, no Git command descends
/// into a submodule, and no configured fsmonitor hook or daemon runs (and so
/// none can inherit the pipes or extend the call). Each call is bound in
/// lifetime; the whole context is bound in wall wait, total stdout and
/// invocation count. Output readers are attached to channels and share one
/// settle window drawn from that budget, so a pipe held by a process Git
/// started can never hold the gate past its clock. Exhaustion is a refusal,
/// never a partial fingerprint. There is no `submodule` subcommand, network or
/// config write.
struct Git {
    calls: usize,
    waited: Duration,
    bytes: u64,
    call_time: Duration,
    total_time: Duration,
    settle_time: Duration,
}

/// One `git` invocation: no index write, no prompt, no submodule recursion and
/// no configured fsmonitor hook or daemon.
fn git_command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "submodule.recurse=0", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

impl Git {
    fn new() -> Self {
        Self {
            calls: 0,
            waited: Duration::ZERO,
            bytes: 0,
            call_time: GIT_CALL_TIME,
            total_time: GIT_TOTAL_TIME,
            settle_time: GIT_SETTLE_TIME,
        }
    }

    /// Charge one call's stdout against the cumulative budget. The design's
    /// 16 MiB is a limit for one snapshot or prepare, not only a per-call cap.
    fn charge(&mut self, bytes: u64) -> Result<(), String> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_GIT_BYTES {
            return Err(format!(
                "the gate's git output exceeded {MAX_GIT_BYTES} bytes"
            ));
        }
        Ok(())
    }

    fn run(&mut self, dir: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
        let name = args.first().copied().unwrap_or("");
        if self.calls >= MAX_GIT_CALLS {
            return Err(format!(
                "the gate needs more than {MAX_GIT_CALLS} git calls"
            ));
        }
        let remaining = self.total_time.saturating_sub(self.waited);
        if remaining.is_zero() {
            return Err("the gate's git time budget is exhausted".to_owned());
        }
        self.calls += 1;
        // One absolute deadline covers execution and settlement: the
        // per-call cap intersected with the cumulative remainder.
        let limit = self.call_time.min(remaining);
        let mut child = git_command(dir, args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not start git: {error}"))?;
        let writer = match input {
            None => None,
            Some(bytes) => {
                let mut stdin = child.stdin.take().ok_or("git stdin was not captured")?;
                let bytes = bytes.to_vec();
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(stdin.write_all(&bytes));
                });
                Some(receiver)
            }
        };
        let stdout = child.stdout.take().ok_or("git stdout was not captured")?;
        let stderr = child.stderr.take().ok_or("git stderr was not captured")?;
        let (out_sender, out_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = out_sender.send(read_capped(stdout, MAX_GIT_BYTES));
        });
        let (err_sender, err_receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = err_sender.send(read_capped(stderr, MAX_METADATA_ERROR_BYTES));
        });
        let started = Instant::now();
        let deadline = started + limit;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() >= deadline => {
                    // Our own child; reaping it closes the pipes unless
                    // something it started still holds them. Those readers
                    // are detached, so the gate keeps its own clock.
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(format!("git {name} did not finish in time"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => break Err(format!("could not wait for git: {error}")),
            }
        };
        self.waited = self.waited.saturating_add(started.elapsed());
        let status = status?;
        // One settle window for all three readers, never past the call's own
        // deadline: each wait takes only what remains of the same window, so
        // sequential waits cannot overrun it.
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
            return Err(format!("git {name} did not finish in time"));
        }
        let (stdout, overflow) = out
            .map_err(|_| format!("git {name} did not close its output in time"))?
            .map_err(|error| format!("cannot read git output: {error}"))?;
        let stderr = err
            .map_err(|_| format!("git {name} did not close its errors in time"))?
            .map(|(bytes, _)| bytes)
            .unwrap_or_default();
        let written = match written {
            None => Ok(()),
            Some(result) => {
                result.map_err(|_| format!("git {name} did not finish its input in time"))?
            }
        };
        self.charge(stdout.len() as u64)?;
        if !status.success() || written.is_err() {
            return Err(format!(
                "git {name} failed: {}",
                String::from_utf8_lossy(&stderr).trim()
            ));
        }
        if overflow {
            return Err(format!(
                "git {name} printed more than {MAX_GIT_BYTES} bytes"
            ));
        }
        Ok(stdout)
    }
}

fn utf8(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "git printed a non-UTF-8 path".to_owned())
}

fn nul_separated(bytes: Vec<u8>) -> Result<Vec<String>, String> {
    utf8(bytes).map(|text| {
        text.split('\0')
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

/// Repository top level, content and the tracked files of every initialized
/// submodule, for the checkout containing `path`.
struct Snapshot {
    top: PathBuf,
    content: Content,
    /// Each `Exact` gitlink's cached tracked files, prefixed with its path.
    children: BTreeMap<String, Vec<String>>,
}

/// The repository's top level, its common Git directory and whether this
/// checkout is a linked worktree.
struct Roots {
    top: PathBuf,
    common: PathBuf,
    linked: bool,
}

/// Content for the checkout containing `path`, with a fresh Git budget.
fn snapshot(path: &Path) -> Result<Snapshot, String> {
    snapshot_with(path, &mut Git::new())
}

/// Content for the checkout containing `path`, on a caller's bounded Git
/// context so one prepare or finish shares one cumulative budget.
fn snapshot_with(path: &Path, git: &mut Git) -> Result<Snapshot, String> {
    let roots = roots(path, git)?;
    let top = roots.top.clone();
    let tree = utf8(git.run(&top, &["rev-parse", "--verify", "HEAD^{tree}"], None)?)?;
    let tree = tree.trim().to_owned();
    if tree.is_empty() {
        return Err("git printed no tree for HEAD".to_owned());
    }
    // Reading status never rewrites the index, and `--ignore-submodules=none`
    // reports a submodule whose content, HEAD or index state differs.
    let status = git.run(
        &top,
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
    let paths = status_paths(&nul_separated(status)?)?;
    let index = nul_separated(git.run(&top, &["ls-files", "-z", "-v", "--stage"], None)?)?;
    let listing = nul_separated(git.run(&top, &["ls-tree", "-r", "-z", "HEAD"], None)?)?;
    let declared = declared_gitlinks(&index, &listing)?;
    let mut submodules = BTreeMap::new();
    let mut children = BTreeMap::new();
    let mut listed = 0usize;
    for (path, expected) in &declared {
        let (state, files) = classify_submodule(git, &roots, path, expected)?;
        if let Some(files) = files {
            listed = listed.saturating_add(files.len());
            if listed > MAX_LISTED {
                return Err(format!("more than {MAX_LISTED} tracked submodule files"));
            }
            children.insert(path.clone(), files);
        }
        submodules.insert(path.clone(), state);
    }
    let mut dirty = BTreeMap::new();
    let mut present = Vec::new();
    for path in paths {
        match fs::symlink_metadata(top.join(&path)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                dirty.insert(path, DELETED.to_owned());
            }
            Err(error) => return Err(format!("cannot read {path}: {error}")),
            Ok(metadata) if metadata.is_dir() => {
                if submodules.contains_key(&path) {
                    return Err(format!(
                        "{path} is a submodule with content the gate does not cover"
                    ));
                }
                return Err(format!(
                    "{path} is a submodule or directory the gate does not cover"
                ));
            }
            Ok(_) => present.push(path),
        }
    }
    for (path, digest) in present.iter().zip(hash_objects(git, &top, &present)?) {
        dirty.insert(path.clone(), digest);
    }
    Ok(Snapshot {
        top,
        content: Content {
            tree,
            dirty,
            submodules,
            has_submodules: true,
        },
        children,
    })
}

/// The canonical top level and Git directories of the repository containing
/// `path`. A linked worktree has a different Git directory from its common
/// directory; the common directory owns the submodule module store.
fn roots(path: &Path, git: &mut Git) -> Result<Roots, String> {
    let output = git.run(
        path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
        ],
        None,
    )?;
    let text = utf8(output)?;
    let mut lines = text.lines();
    let (Some(top), Some(git_dir), Some(common)) = (lines.next(), lines.next(), lines.next())
    else {
        return Err("git printed unexpected repository paths".to_owned());
    };
    if lines.next().is_some() {
        return Err("git printed unexpected repository paths".to_owned());
    }
    let resolve = |name: &str| {
        fs::canonicalize(name.trim())
            .map_err(|error| format!("cannot resolve the repository ({error})"))
    };
    let top = resolve(top)?;
    let git_dir = resolve(git_dir)?;
    let common = resolve(common)?;
    Ok(Roots {
        linked: git_dir != common,
        top,
        common,
    })
}

/// Every gitlink the index and `HEAD` declare, by path and committed id.
/// Each declared path must be the same regular cached entry at stage 0 in
/// both: a differing id, an extra stage, an unmerged entry, assume-unchanged,
/// skip-worktree or sparse exclusion refuses the whole checkout.
fn declared_gitlinks(
    index: &[String],
    listing: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let mut head = BTreeMap::new();
    for entry in listing {
        let (meta, path) = entry
            .split_once('\t')
            .ok_or_else(|| format!("unexpected git tree entry {entry:?}"))?;
        let fields: Vec<&str> = meta.split(' ').collect();
        let [mode, kind, oid] = fields.as_slice() else {
            return Err(format!("unexpected git tree entry {entry:?}"));
        };
        if *mode != GITLINK_MODE {
            continue;
        }
        if *kind != "commit" || !valid_repo_path(path) {
            return Err(format!("unexpected gitlink entry {entry:?}"));
        }
        if head.insert(path.to_owned(), (*oid).to_owned()).is_some() {
            return Err(format!("git listed {path} more than once"));
        }
    }
    let mut committed = BTreeMap::new();
    for entry in parse_index(index)? {
        if entry.mode != GITLINK_MODE {
            continue;
        }
        if entry.stage != "0" {
            return Err(format!("submodule {} is unmerged", entry.path));
        }
        if entry.flag != 'H' {
            return Err(format!(
                "submodule {} has index flags the gate does not cover",
                entry.path
            ));
        }
        if !valid_repo_path(&entry.path) {
            return Err(format!(
                "submodule {} is not a plain repository path",
                entry.path
            ));
        }
        if committed
            .insert(entry.path.clone(), entry.oid.clone())
            .is_some()
        {
            return Err(format!("git listed {} more than once", entry.path));
        }
    }
    if head != committed {
        return Err("the index and HEAD disagree about their submodules".to_owned());
    }
    if head.len() > MAX_SUBMODULES {
        return Err(format!("more than {MAX_SUBMODULES} submodules"));
    }
    Ok(head)
}

/// One index entry from `git ls-files -z -v --stage`.
struct IndexEntry {
    flag: char,
    mode: String,
    oid: String,
    stage: String,
    path: String,
}

/// Parse `git ls-files -z -v --stage` entries (`<flag> <mode> <oid>
/// <stage>\t<path>`). A malformed or unexpected field refuses.
fn parse_index(entries: &[String]) -> Result<Vec<IndexEntry>, String> {
    entries
        .iter()
        .map(|entry| {
            let (meta, path) = entry
                .split_once('\t')
                .ok_or_else(|| format!("unexpected git index entry {entry:?}"))?;
            let fields: Vec<&str> = meta.split(' ').collect();
            let [flag, mode, oid, stage] = fields.as_slice() else {
                return Err(format!("unexpected git index entry {entry:?}"));
            };
            let mut flags = flag.chars();
            let flag = flags
                .next()
                .ok_or_else(|| format!("unexpected git index entry {entry:?}"))?;
            let plausible = flags.next().is_none()
                && mode.len() == 6
                && mode.bytes().all(|byte| byte.is_ascii_digit())
                && !oid.is_empty()
                && oid.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !stage.is_empty()
                && stage.bytes().all(|byte| byte.is_ascii_digit())
                && !path.is_empty();
            if !plausible {
                return Err(format!("unexpected git index entry {entry:?}"));
            }
            Ok(IndexEntry {
                flag,
                mode: (*mode).to_owned(),
                oid: (*oid).to_owned(),
                stage: (*stage).to_owned(),
                path: path.to_owned(),
            })
        })
        .collect()
}

/// A repository-relative path the gate will join to a checkout: relative,
/// without `.`/`..`, and without the separators its line protocols use.
fn valid_repo_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\n')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Classify one declared gitlink: `Uninitialized`, or `Exact` with this
/// checkout's prefixed tracked files. Anything else refuses.
fn classify_submodule(
    git: &mut Git,
    roots: &Roots,
    path: &str,
    expected: &str,
) -> Result<(SubmoduleState, Option<Vec<String>>), String> {
    let full = roots.top.join(path);
    let metadata = match fs::symlink_metadata(&full) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((SubmoduleState::Uninitialized, None));
        }
        Err(error) => return Err(format!("cannot read {path}: {error}")),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("{path} is a symlink, not a submodule directory"));
    }
    if !metadata.is_dir() {
        return Err(format!("{path} is not a submodule directory"));
    }
    let mut entries =
        fs::read_dir(&full).map_err(|error| format!("cannot read {path}: {error}"))?;
    match entries.next() {
        None => return Ok((SubmoduleState::Uninitialized, None)),
        Some(Err(error)) => return Err(format!("cannot read {path}: {error}")),
        Some(Ok(_)) => {}
    }
    // A linked worktree of the superproject never owns the module store, so
    // an initialized child cannot be vouched for there. Positive absence and
    // an empty directory were handled above without a child Git call.
    if roots.linked {
        return Err(format!(
            "{path} is initialized in a linked worktree, which the gate does not cover"
        ));
    }
    check_child_gitdir(roots, &full, path)?;
    let head = utf8(git.run(&full, &["rev-parse", "--verify", "HEAD"], None)?)?;
    let head = head.trim().to_owned();
    if head != expected {
        return Err(format!("{path} is at {head}, not the committed {expected}"));
    }
    // The child's own worktree root must be this directory: a `.git` pointer
    // may name any module directory under the repository, so the boundary is
    // checked rather than inferred.
    let toplevel = utf8(git.run(
        &full,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
        None,
    )?)?;
    let toplevel = fs::canonicalize(toplevel.trim())
        .map_err(|error| format!("cannot resolve {path}'s top level: {error}"))?;
    let child =
        fs::canonicalize(&full).map_err(|error| format!("cannot resolve {path}: {error}"))?;
    if toplevel != child {
        return Err(format!("{path} is not the root of its own repository"));
    }
    let status = git.run(
        &full,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        None,
    )?;
    if !nul_separated(status)?.is_empty() {
        return Err(format!("{path} has changes the gate does not cover"));
    }
    let entries = parse_index(&nul_separated(git.run(
        &full,
        &["ls-files", "-z", "-v", "--stage"],
        None,
    )?)?)?;
    let mut files = Vec::new();
    for entry in &entries {
        if entry.flag != 'H' || entry.stage != "0" {
            return Err(format!(
                "{path}/{} has index flags the gate does not cover",
                entry.path
            ));
        }
        if entry.mode == GITLINK_MODE {
            return Err(format!(
                "{path}/{} is a nested submodule, which the gate does not cover",
                entry.path
            ));
        }
        if !is_regular_mode(&entry.mode) {
            return Err(format!(
                "{path}/{} has mode {} the gate does not cover",
                entry.path, entry.mode
            ));
        }
        if !valid_repo_path(&entry.path) {
            return Err(format!(
                "{path}/{} is not a plain repository path",
                entry.path
            ));
        }
        files.push(entry.path.clone());
    }
    if files.len() > MAX_LISTED {
        return Err(format!("{path} tracks more than {MAX_LISTED} files"));
    }
    let digests = hash_objects(git, &full, &files)?;
    let raw = raw_digest(&files, &digests);
    let prefixed = files.iter().map(|file| format!("{path}/{file}")).collect();
    Ok((SubmoduleState::Exact { head, raw }, Some(prefixed)))
}

/// The child's `.git` must point into this repository's module store under
/// the common Git directory, component-aware, with no symlink indirection.
fn check_child_gitdir(roots: &Roots, full: &Path, path: &str) -> Result<(), String> {
    let dot = full.join(".git");
    let metadata =
        fs::symlink_metadata(&dot).map_err(|error| format!("cannot read {path}/.git: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{path}/.git is a symlink the gate does not follow"));
    }
    let target = if metadata.is_dir() {
        dot
    } else if metadata.is_file() {
        let mut bytes = Vec::new();
        fs::File::open(&dot)
            .and_then(|file| file.take(MAX_DOT_GIT_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|error| format!("cannot read {path}/.git: {error}"))?;
        if bytes.len() as u64 > MAX_DOT_GIT_BYTES {
            return Err(format!("{path}/.git is larger than the gate covers"));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{path}/.git is not a UTF-8 gitdir pointer"))?;
        let line = text.trim_end_matches(['\n', '\r']);
        if line.contains(['\n', '\r']) {
            return Err(format!("{path}/.git is not a single gitdir pointer"));
        }
        let pointer = line
            .strip_prefix("gitdir: ")
            .ok_or_else(|| format!("{path}/.git is not a gitdir pointer"))?;
        let pointer = PathBuf::from(pointer.trim());
        if pointer.as_os_str().is_empty() {
            return Err(format!("{path}/.git names no gitdir"));
        }
        if pointer.is_absolute() {
            pointer
        } else {
            full.join(pointer)
        }
    } else {
        return Err(format!("{path}/.git is not a file or directory"));
    };
    let resolved = fs::canonicalize(&target)
        .map_err(|error| format!("cannot resolve {path}/.git: {error}"))?;
    let modules = roots.common.join("modules");
    let modules = fs::canonicalize(&modules)
        .map_err(|error| format!("cannot resolve {}: {error}", modules.display()))?;
    if !resolved.starts_with(&modules) || resolved == modules {
        return Err(format!(
            "{path}/.git resolves outside this repository's module store"
        ));
    }
    Ok(())
}

/// Modes a submodule's tracked file may have. A gitlink here is nested.
fn is_regular_mode(mode: &str) -> bool {
    matches!(mode, "100644" | "100755" | "120000")
}

/// A deterministic digest of ordered tracked paths and raw blob ids. Filters
/// and smudge normalization never enter it.
fn raw_digest(paths: &[String], digests: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"oxide-submodule-raw-v1\0");
    for (path, digest) in paths.iter().zip(digests) {
        hasher.update((path.len() as u64).to_be_bytes());
        hasher.update(path.as_bytes());
        hasher.update((digest.len() as u64).to_be_bytes());
        hasher.update(digest.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Paths from `git status --porcelain=v1 -z --no-renames` entries.
fn status_paths(entries: &[String]) -> Result<Vec<String>, String> {
    if entries.len() > MAX_PATHS {
        return Err(format!("more than {MAX_PATHS} changed files"));
    }
    entries
        .iter()
        .map(|entry| {
            entry
                .get(3..)
                .filter(|path| entry.as_bytes().get(2) == Some(&b' ') && !path.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("unexpected git status entry {entry:?}"))
        })
        .collect()
}

/// Git blob digests of the raw file bytes, in order.
fn hash_objects(git: &mut Git, dir: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if paths.iter().any(|path| path.contains('\n')) {
        return Err("a listed path contains a newline".to_owned());
    }
    let list = paths.join("\n") + "\n";
    let output = git.run(
        dir,
        &["hash-object", "--no-filters", "--stdin-paths"],
        Some(list.as_bytes()),
    )?;
    let digests: Vec<String> = utf8(output)?.lines().map(str::to_owned).collect();
    if digests.len() == paths.len() {
        Ok(digests)
    } else {
        Err("git hash-object returned the wrong number of digests".to_owned())
    }
}

/// Every path whose content may differ between two snapshots of the same
/// repository: tree entries, dirty digests and declared submodule states. Run
/// in the checkout whose object store must hold both trees.
fn differing(
    git: &mut Git,
    top: &Path,
    a: &Content,
    b: &Content,
) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    if a.tree != b.tree {
        let diff = git.run(
            top,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &a.tree,
                &b.tree,
            ],
            None,
        )?;
        paths.extend(nul_separated(diff)?);
    }
    paths.extend(dirty_differences(a, b));
    paths.extend(submodule_differences(a, b));
    Ok(paths)
}

/// A path dirty on either side differs unless both carry the same digest.
fn dirty_differences(a: &Content, b: &Content) -> BTreeSet<String> {
    a.dirty
        .keys()
        .chain(b.dirty.keys())
        .filter(|path| a.dirty.get(*path) != b.dirty.get(*path))
        .cloned()
        .collect()
}

/// A declared submodule differs unless both sides classify it identically: an
/// uninitialized and an initialized worktree at the same gitlink still differ.
fn submodule_differences(a: &Content, b: &Content) -> BTreeSet<String> {
    a.submodules
        .keys()
        .chain(b.submodules.keys())
        .filter(|path| a.submodules.get(*path) != b.submodules.get(*path))
        .cloned()
        .collect()
}

/// Set mtime to now on each listed checkout file that is not newer than the
/// anchor. Missing files are skipped: Cargo rebuilds a unit whose input is
/// gone. A symlink is followed only inside the checkout. A directory is
/// skipped only when this snapshot classified it as a declared submodule; any
/// other directory still refuses, and no gitlink directory is ever touched.
fn refresh<'a>(
    top: &Path,
    paths: impl IntoIterator<Item = &'a String>,
    anchor_ns: u64,
    submodules: &BTreeMap<String, SubmoduleState>,
) -> Result<usize, String> {
    let mut refreshed = 0;
    for path in paths {
        if submodules.contains_key(path) {
            continue;
        }
        let full = top.join(path);
        let target = match fs::symlink_metadata(&full) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read {path}: {error}")),
            Ok(metadata) if metadata.file_type().is_symlink() => match fs::canonicalize(&full) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("cannot resolve {path}: {error}")),
                Ok(target) if target.starts_with(top) => target,
                Ok(_) => return Err(format!("{path} links outside the checkout")),
            },
            Ok(_) => full,
        };
        let metadata =
            fs::metadata(&target).map_err(|error| format!("cannot read {path}: {error}"))?;
        if metadata.is_dir() {
            return Err(format!(
                "{path} is a submodule or directory the gate does not cover"
            ));
        }
        if mtime_ns(&metadata) <= anchor_ns {
            touch(&target)?;
            refreshed += 1;
        }
    }
    Ok(refreshed)
}

fn mtime_ns(metadata: &fs::Metadata) -> u64 {
    let seconds = u64::try_from(metadata.mtime()).unwrap_or(0);
    let nanos = u64::try_from(metadata.mtime_nsec()).unwrap_or(0);
    seconds.saturating_mul(1_000_000_000).saturating_add(nanos)
}

/// mtime to now, access time unchanged, contents untouched.
fn touch(path: &Path) -> Result<(), String> {
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{} contains a NUL byte", path.display()))?;
    let times = [
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT,
        },
        libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW,
        },
    ];
    // SAFETY: `name` is NUL-terminated and `times` holds two valid entries.
    if unsafe { libc::utimensat(libc::AT_FDCWD, name.as_ptr(), times.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(format!(
            "cannot refresh {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ))
    }
}

/// Files to refresh for a checkout-path change. Cargo's resolved graph
/// names the packages: local ones have no registry or Git source, workspace
/// members and in-repo path dependencies alike. Every Git-listed file under
/// the checkout is refreshed unless its deepest package directory belongs to
/// a registry or Git package (vendored sources), including the tracked files
/// of an initialized submodule. So a local unit rebuilds whatever it reads,
/// inside or outside its own directory. A local target or a followed symlink
/// inside a vendored package is ambiguous and refused. Returns the local
/// package count and the files.
fn local_package_files(
    git: &mut Git,
    snapshot: &Snapshot,
    metadata: &serde_json::Value,
) -> Result<(usize, Vec<String>), String> {
    let top = &snapshot.top;
    let graph = resolve_graph(metadata)?;
    let canonical = |paths: &[PathBuf]| {
        paths
            .iter()
            .map(|path| {
                fs::canonicalize(path)
                    .map_err(|error| format!("cannot resolve {}: {error}", path.display()))
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let local = package_dirs(top, &canonical(&graph.local)?)?;
    let foreign = package_dirs(top, &canonical(&graph.foreign)?)?;
    if let Some(dir) = local.intersection(&foreign).next() {
        return Err(format!("two packages claim {dir:?}"));
    }
    let targets = contained(top, &canonical(&graph.targets)?)?;
    let mut listed = nul_separated(git.run(
        top,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
        None,
    )?)?;
    // An initialized submodule's tracked sources are local inputs too. Its
    // ignored files stay out, as before.
    for files in snapshot.children.values() {
        listed.extend(files.iter().cloned());
    }
    if listed.len() > MAX_LISTED {
        return Err(format!("more than {MAX_LISTED} repository files"));
    }
    let files = owned_files(&local, &foreign, &targets, &listed)?;
    // A followed symlink must not reach a registry or Git package's file.
    for path in &files {
        let full = top.join(path);
        let is_link = fs::symlink_metadata(&full)
            .map_err(|error| format!("cannot read {path}: {error}"))?
            .file_type()
            .is_symlink();
        if !is_link {
            continue;
        }
        let target =
            fs::canonicalize(&full).map_err(|error| format!("cannot resolve {path}: {error}"))?;
        for relative in contained(top, &[target])? {
            if deepest_owner(&local, &foreign, &relative).is_some_and(|dir| foreign.contains(dir)) {
                return Err(format!("{path} links into a registry or Git package"));
            }
        }
    }
    Ok((local.len(), files))
}

/// A Cargo call in the guarded command's context: the same Cargo, working
/// directory (so the same config files) and inherited environment, plus
/// `args` and the workspace manifest.
fn cargo_command(checkout: &Path, leading: &[&str], args: &[OsString]) -> Command {
    let mut command = Command::new(crate::CARGO);
    command
        .args(leading)
        .args(args)
        .arg("--manifest-path")
        .arg(checkout.join("Cargo.toml"));
    crate::wrapper::apply(&mut command);
    command
}

/// The gate's metadata arguments: the guarded command's own, plus
/// `--offline` and `--locked` where it lacks them. Metadata then never
/// downloads a package or writes the lock file, whatever the request.
fn probe_args(request: &Resolution) -> Vec<OsString> {
    let mut args = request.args.clone();
    if !request.offline() {
        args.push(OsString::from("--offline"));
    }
    if !request.locked() {
        args.push(OsString::from("--locked"));
    }
    args
}

/// The workspace graph from `cargo metadata`. The call itself has no
/// effects. When Cargo refuses because the lock file is missing or stale:
/// - a help request resolves nothing, so the slot is only refused;
/// - a `--locked` or `--frozen` command fails under plain Cargo when the
///   lock is missing, or when it is offline in any case: the refusal is the
///   command's own and ends it;
/// - a command allowed to write a missing lock gets it from `cargo
///   generate-lockfile` with its own arguments: Cargo's resolution with no
///   previous lock, as its build would do; no package is downloaded;
/// - anything else refuses the slot, and the cold build does Cargo's own
///   lock work.
///
/// Ending the command or writing its lock acts for Cargo, so both need a
/// built-in Cargo build command whose arguments Cargo accepts. External and
/// unknown commands, and arguments Cargo rejects, never resolve under plain
/// Cargo: they only refuse the slot, and Cargo then runs them as usual.
fn cargo_metadata(checkout: &Path, request: &Resolution) -> Result<serde_json::Value, Refusal> {
    let refused = match probe(checkout, request)? {
        Ok(metadata) => return Ok(metadata),
        Err(refused) => refused,
    };
    let missing = !request.moves_lockfile() && !checkout.join("Cargo.lock").exists();
    let reason =
        |message: &str| -> Refusal { format!("cargo metadata failed: {}", message.trim()).into() };
    let (terminal, check) = match lock_step(request, missing) {
        LockStep::Refuse(None) => return Err(reason(&refused.message)),
        LockStep::Refuse(Some(why)) => return Err(Refusal::Unvouched(why.to_owned())),
        LockStep::End(check) => (true, check),
        LockStep::Generate(check) => (false, check),
    };
    let mut parse = Command::new(crate::CARGO);
    parse.args(check);
    crate::wrapper::apply(&mut parse);
    if !cargo_accepts(parse)? {
        return Err(Refusal::Unvouched(
            "Cargo rejects this command's arguments, so it never resolves the lock file".to_owned(),
        ));
    }
    if terminal {
        return Err(Refusal::Lock {
            code: refused.code,
            message: refused.message,
        });
    }
    let generated = run_cargo(
        cargo_command(checkout, &["generate-lockfile"], &request.args),
        MAX_METADATA_ERROR_BYTES,
    )?;
    if !generated.status.success() {
        return Err(format!(
            "cargo generate-lockfile failed: {}",
            generated.stderr.trim()
        )
        .into());
    }
    probe(checkout, request)?.map_err(|refused| reason(&refused.message))
}

/// What the gate does about Cargo's lock refusal, before the parse check.
#[derive(Debug, Eq, PartialEq)]
enum LockStep<'a> {
    /// Refuse the slot, with this reason or Cargo's own message.
    Refuse(Option<&'static str>),
    /// End the command with Cargo's refusal if Cargo accepts these arguments.
    End(&'a [OsString]),
    /// Write the missing lock if Cargo accepts these arguments.
    Generate(&'a [OsString]),
}

fn lock_step(request: &Resolution, missing: bool) -> LockStep<'_> {
    if request.help {
        return LockStep::Refuse(None);
    }
    let terminal = request.locked() && (missing || request.offline());
    if request.locked() && !terminal {
        return LockStep::Refuse(None);
    }
    if !request.locked() && !missing {
        return LockStep::Refuse(Some(
            "the lock file needs updating; this command's Cargo updates it",
        ));
    }
    match &request.parse_check {
        None => LockStep::Refuse(Some(
            "only a built-in Cargo build command resolves the lock file",
        )),
        Some(check) if terminal => LockStep::End(check),
        Some(check) => LockStep::Generate(check),
    }
}

/// Run the parse check (`Resolution::parse_check`): Cargo accepts the
/// arguments when its first error is the refusal of the unknown `-Z`.
fn cargo_accepts(command: Command) -> Result<bool, String> {
    let output = run_cargo(command, MAX_METADATA_ERROR_BYTES)?;
    let first_error = output
        .stderr
        .lines()
        .find(|line| line.starts_with("error:"));
    Ok(output.status.code() == Some(101)
        && first_error.is_some_and(|line| line.contains("`-Z` flag")))
}

/// Cargo's refusal to create or update the lock file.
struct LockRefused {
    code: u8,
    message: String,
}

/// One effect-free metadata call. `Ok(Err(_))` is Cargo's lock refusal.
fn probe(
    checkout: &Path,
    request: &Resolution,
) -> Result<Result<serde_json::Value, LockRefused>, String> {
    let output = run_cargo(
        cargo_command(
            checkout,
            &["metadata", "--format-version", "1"],
            &probe_args(request),
        ),
        MAX_METADATA_BYTES,
    )?;
    if !output.status.success() {
        if output.status.code().is_some() && output.stderr.contains(LOCK_REFUSAL) {
            return Ok(Err(LockRefused {
                code: crate::process::exit_code(output.status),
                message: output.stderr,
            }));
        }
        return Err(format!("cargo metadata failed: {}", output.stderr.trim()));
    }
    if output.overflow {
        return Err("cargo metadata output exceeds the gate's limit".to_owned());
    }
    serde_json::from_slice(&output.stdout)
        .map(Ok)
        .map_err(|error| format!("Cargo metadata is not JSON: {error}"))
}

pub(crate) struct CargoOutput {
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) overflow: bool,
    pub(crate) stderr: String,
}

/// Run one Cargo call, bounded in output and lifetime.
pub(crate) fn run_cargo(mut command: Command, stdout_cap: u64) -> Result<CargoOutput, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start Cargo: {error}"))?;
    let stdout = child.stdout.take().ok_or("Cargo stdout was not captured")?;
    let stderr = child.stderr.take().ok_or("Cargo stderr was not captured")?;
    let out = std::thread::spawn(move || read_capped(stdout, stdout_cap));
    let err = std::thread::spawn(move || read_capped(stderr, MAX_METADATA_ERROR_BYTES));
    let deadline = Instant::now() + METADATA_TIME;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                // Our own child; reaping it closes the pipes.
                let _ = child.kill();
                let _ = child.wait();
                break Err("Cargo did not finish in time".to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => break Err(format!("could not wait for Cargo: {error}")),
        }
    };
    let out = out.join().map_err(|_| "Cargo output reader panicked")?;
    let err = err.join().map_err(|_| "Cargo error reader panicked")?;
    let status = status?;
    let (stdout, overflow) = out.map_err(|error| format!("cannot read Cargo output: {error}"))?;
    let stderr = err.map(|(bytes, _)| bytes).unwrap_or_default();
    Ok(CargoOutput {
        status,
        stdout,
        overflow,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// Up to `cap` bytes, then drain the rest so the writer never blocks.
/// `true` when the stream was longer.
fn read_capped(reader: impl Read, cap: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut reader = reader;
    let mut bytes = Vec::new();
    (&mut reader).take(cap).read_to_end(&mut bytes)?;
    let rest = std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok((bytes, rest > 0))
}

/// Package manifests and local target sources from `cargo metadata`.
#[derive(Debug, Default)]
struct Graph {
    /// Packages with no registry or Git source.
    local: Vec<PathBuf>,
    /// Registry and Git packages.
    foreign: Vec<PathBuf>,
    /// Every target source of a local package, build scripts included.
    targets: Vec<PathBuf>,
}

/// Split Cargo's resolved packages by source. A package whose source,
/// manifest or targets cannot be read is refused.
fn resolve_graph(metadata: &serde_json::Value) -> Result<Graph, String> {
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo metadata lists no packages")?;
    let mut graph = Graph::default();
    for package in packages {
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .ok_or("Cargo metadata has a package without a manifest path")?;
        match package.get("source") {
            Some(serde_json::Value::String(_)) => {
                graph.foreign.push(manifest);
                continue;
            }
            Some(serde_json::Value::Null) => graph.local.push(manifest),
            _ => return Err("Cargo metadata has a package with an unreadable source".to_owned()),
        }
        let targets = package
            .get("targets")
            .and_then(serde_json::Value::as_array)
            .ok_or("Cargo metadata has a local package without targets")?;
        for target in targets {
            let source = target
                .get("src_path")
                .and_then(serde_json::Value::as_str)
                .ok_or("Cargo metadata has a local target without a source path")?;
            graph.targets.push(PathBuf::from(source));
        }
    }
    Ok(graph)
}

/// Directories, relative to `top`, of the canonical manifests under it.
/// Component-aware: `/repo-other` is not under `/repo`. Two packages in one
/// directory are refused as ambiguous.
fn package_dirs(top: &Path, manifests: &[PathBuf]) -> Result<BTreeSet<String>, String> {
    let mut dirs = BTreeSet::new();
    for manifest in manifests {
        let dir = manifest
            .parent()
            .ok_or_else(|| format!("{} has no directory", manifest.display()))?;
        for relative in contained(top, &[dir.to_path_buf()])? {
            if !dirs.insert(relative) {
                return Err(format!("two packages claim {}", dir.display()));
            }
        }
    }
    Ok(dirs)
}

/// The canonical paths under `top`, relative to it; others are dropped.
fn contained(top: &Path, paths: &[PathBuf]) -> Result<BTreeSet<String>, String> {
    let mut inside = BTreeSet::new();
    for path in paths {
        let Ok(relative) = path.strip_prefix(top) else {
            continue;
        };
        let relative = relative
            .to_str()
            .ok_or_else(|| format!("{} is not UTF-8", path.display()))?;
        inside.insert(relative.to_owned());
    }
    Ok(inside)
}

/// Listed files whose deepest package directory is not a registry or Git
/// package's. A local target inside such a package is ambiguous and refused. Each local manifest and target
/// source must be listed: a file Git cannot see would be silently skipped.
fn owned_files(
    local: &BTreeSet<String>,
    foreign: &BTreeSet<String>,
    targets: &BTreeSet<String>,
    listed: &[String],
) -> Result<Vec<String>, String> {
    let visible: BTreeSet<&str> = listed.iter().map(String::as_str).collect();
    let manifests = local.iter().map(|dir| {
        Path::new(dir)
            .join("Cargo.toml")
            .to_str()
            .map(str::to_owned)
    });
    for required in manifests.chain(targets.iter().cloned().map(Some)) {
        match required {
            Some(path) if visible.contains(path.as_str()) => {}
            Some(path) => {
                return Err(format!(
                    "{path} is not visible to Git; the gate cannot cover its package"
                ))
            }
            None => return Err("a package path is not UTF-8".to_owned()),
        }
    }
    let foreign_owned =
        |path: &str| deepest_owner(local, foreign, path).is_some_and(|dir| foreign.contains(dir));
    if let Some(target) = targets.iter().find(|target| foreign_owned(target)) {
        return Err(format!(
            "local target {target} lies in a registry or Git package"
        ));
    }
    Ok(listed
        .iter()
        .filter(|path| !foreign_owned(path))
        .cloned()
        .collect())
}

/// The deepest local or foreign package directory containing `path`.
fn deepest_owner<'a>(
    local: &'a BTreeSet<String>,
    foreign: &'a BTreeSet<String>,
    path: &str,
) -> Option<&'a String> {
    local
        .iter()
        .chain(foreign)
        .filter(|dir| Path::new(path).starts_with(dir))
        .max_by_key(|dir| Path::new(dir).components().count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(tree: &str, dirty: &[(&str, &str)]) -> Content {
        Content {
            tree: tree.to_owned(),
            dirty: dirty
                .iter()
                .map(|(path, digest)| ((*path).to_owned(), (*digest).to_owned()))
                .collect(),
            submodules: BTreeMap::new(),
            has_submodules: true,
        }
    }

    fn origin_with(version: u32, content: Content) -> Origin {
        Origin {
            checkout: PathBuf::from("/a"),
            content,
            content_version: version,
            anchor_ns: 0,
            uncertain: BTreeSet::new(),
        }
    }

    #[test]
    fn dirty_paths_differ_unless_both_sides_match() {
        let a = content(
            "t",
            &[("src/a.rs", "1"), ("same.rs", "9"), ("gone.rs", DELETED)],
        );
        let b = content("t", &[("src/a.rs", "2"), ("same.rs", "9"), ("new.rs", "3")]);
        let paths: Vec<String> = dirty_differences(&a, &b).into_iter().collect();
        assert_eq!(paths, ["gone.rs", "new.rs", "src/a.rs"]);
        assert!(dirty_differences(&a, &a).is_empty());
    }

    #[test]
    fn status_entries_yield_repository_paths() {
        let entries = [" M src/lib.rs", "?? data/new file.txt", "D  old.rs"].map(str::to_owned);
        assert_eq!(
            status_paths(&entries).expect("parses"),
            ["src/lib.rs", "data/new file.txt", "old.rs"]
        );
        assert!(status_paths(&["XYZ".to_owned()]).is_err());
        assert!(status_paths(&vec![" M a".to_owned(); MAX_PATHS + 1]).is_err());
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn metadata_runs_in_the_guarded_context_and_never_writes_or_downloads() {
        let request = |values: &[&str]| Resolution {
            args: values.iter().map(OsString::from).collect(),
            help: false,
            parse_check: None,
        };
        let probe = |values: &[&str]| -> Vec<String> {
            probe_args(&request(values))
                .into_iter()
                .map(|value| value.into_string().expect("UTF-8"))
                .collect()
        };
        assert_eq!(probe(&[]), ["--offline", "--locked"]);
        assert_eq!(probe(&["--locked"]), ["--locked", "--offline"]);
        assert_eq!(probe(&["--offline"]), ["--offline", "--locked"]);
        assert_eq!(
            probe(&["--frozen", "--config", "net.offline=false"]),
            ["--frozen", "--config", "net.offline=false"]
        );
        assert_eq!(
            probe(&["--config=net.offline=true", "-Zx"]),
            ["--config=net.offline=true", "-Zx", "--offline", "--locked"]
        );
        let command = cargo_command(
            Path::new("/w"),
            &["metadata", "--format-version", "1"],
            &probe_args(&request(&["--config", "net.offline=false"])),
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "metadata",
                "--format-version",
                "1",
                "--config",
                "net.offline=false",
                "--offline",
                "--locked",
                "--manifest-path",
                "/w/Cargo.toml",
            ]
        );
        // Same working directory and environment as the guarded command:
        // Cargo resolves config files, `CARGO_NET_OFFLINE` and precedence.
        // The only addition is the guarded command's own wrapper policy.
        assert_eq!(command.get_current_dir(), None);
        let envs: Vec<_> = command.get_envs().collect();
        if crate::wrapper::Policy::from_env() == crate::wrapper::Policy::Keep {
            assert!(envs.is_empty(), "{envs:?}");
        } else {
            assert_eq!(
                envs,
                [(
                    std::ffi::OsStr::new("RUSTC_WRAPPER"),
                    Some(std::ffi::OsStr::new(""))
                )]
            );
        }
        // The lock is generated with the command's own arguments only.
        let generate = cargo_command(
            Path::new("/w"),
            &["generate-lockfile"],
            &request(&["--offline"]).args,
        );
        assert_eq!(
            generate.get_args().collect::<Vec<_>>(),
            [
                "generate-lockfile",
                "--offline",
                "--manifest-path",
                "/w/Cargo.toml"
            ]
        );
    }

    #[test]
    fn only_accepted_built_in_commands_end_or_write_the_lock() {
        let request = |values: &[&str]| {
            let values: Vec<OsString> = values.iter().map(OsString::from).collect();
            let invocation =
                crate::cli::parse_leased(values[0].to_str().expect("UTF-8"), &values[1..])
                    .expect("parses");
            crate::cli::resolution(&invocation.cargo_args)
        };
        let step = |values: &[&str], missing: bool| match lock_step(&request(values), missing) {
            LockStep::Refuse(_) => "refuse",
            LockStep::End(_) => "end",
            LockStep::Generate(_) => "generate",
        };
        // Built-in build commands: Cargo's own outcome.
        assert_eq!(step(&["test", "--locked"], true), "end");
        assert_eq!(step(&["test", "--frozen"], false), "end");
        assert_eq!(step(&["build", "--locked", "--offline"], false), "end");
        assert_eq!(step(&["test"], true), "generate");
        assert_eq!(step(&["test", "--offline"], true), "generate");
        // Uncertain or stale: the cold build's Cargo decides.
        assert_eq!(step(&["test", "--locked"], false), "refuse");
        assert_eq!(step(&["test"], false), "refuse");
        // Help, external and unknown commands, and own `-Z`: never ended
        // or given a lock by the gate.
        assert_eq!(step(&["test", "--locked", "--help"], true), "refuse");
        assert_eq!(step(&["test", "--help"], true), "refuse");
        assert_eq!(step(&["foo", "--locked"], true), "refuse");
        assert_eq!(step(&["nextest", "run", "--frozen"], false), "refuse");
        assert_eq!(step(&["clippy", "--locked"], true), "refuse");
        assert_eq!(step(&["foo"], true), "refuse");
        assert_eq!(
            step(&["test", "--locked", "-Zunstable-options"], true),
            "refuse"
        );
        assert_eq!(step(&["test", "-Zunstable-options"], true), "refuse");
    }

    /// Real Cargo in an empty scratch directory: accepted arguments reach the
    /// `-Z` refusal; anything else, including a check placed after `--`,
    /// would hit Cargo's argument error or its missing manifest instead.
    #[test]
    fn parse_check_asks_cargo_without_effects() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("parse-check-{:x}", std::process::id()));
        fs::create_dir(&dir).expect("scratch dir");
        let accepts = |values: &[&str]| -> Option<bool> {
            let values: Vec<OsString> = values.iter().map(OsString::from).collect();
            let invocation =
                crate::cli::parse_leased(values[0].to_str().expect("UTF-8"), &values[1..])
                    .expect("parses");
            let check = crate::cli::resolution(&invocation.cargo_args).parse_check?;
            let mut command = Command::new(crate::CARGO);
            command.args(check).current_dir(&dir);
            Some(cargo_accepts(command).expect("Cargo ran"))
        };
        assert_eq!(accepts(&["test", "--locked"]), Some(true));
        assert_eq!(
            accepts(&["build", "--offline", "--config", "net.offline=false"]),
            Some(true)
        );
        assert_eq!(
            accepts(&["test", "--message-format=json", "--", "--bogus"]),
            Some(true)
        );
        assert_eq!(accepts(&["test", "--bogus"]), Some(false));
        assert_eq!(accepts(&["check", "--message-format=bogus"]), Some(false));
        assert_eq!(accepts(&["build", "--release", "--features"]), Some(false));
        assert_eq!(accepts(&["nextest", "run"]), None);
        assert_eq!(accepts(&["test", "-Zunstable-options"]), None);
        let left: Vec<_> = fs::read_dir(&dir).expect("scratch").collect();
        fs::remove_dir(&dir).expect("parse check left no files");
        assert!(left.is_empty());
    }

    #[test]
    fn metadata_splits_packages_by_source() {
        let metadata = serde_json::json!({"packages": [
            {"name": "app", "source": null, "manifest_path": "/repo/pkg/Cargo.toml",
             "targets": [{"src_path": "/repo/shared/lib.rs"}, {"src_path": "/repo/pkg/build.rs"}]},
            {"name": "outside", "source": null, "manifest_path": "/outside/Cargo.toml", "targets": []},
            {"name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
             "manifest_path": "/repo/vendor/serde/Cargo.toml"},
            {"name": "dep", "source": "git+https://example.invalid/dep#abc",
             "manifest_path": "/repo/vendor/dep/Cargo.toml"},
        ]});
        let graph = resolve_graph(&metadata).expect("reads");
        let paths = |items: &[&str]| items.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(
            graph.local,
            paths(&["/repo/pkg/Cargo.toml", "/outside/Cargo.toml"])
        );
        assert_eq!(
            graph.foreign,
            paths(&[
                "/repo/vendor/serde/Cargo.toml",
                "/repo/vendor/dep/Cargo.toml"
            ])
        );
        assert_eq!(
            graph.targets,
            paths(&["/repo/shared/lib.rs", "/repo/pkg/build.rs"])
        );
        for broken in [
            serde_json::json!({}),
            serde_json::json!({"packages": [{"name": "x", "manifest_path": "/r/Cargo.toml", "targets": []}]}),
            serde_json::json!({"packages": [{"name": "x", "source": 1, "manifest_path": "/r/Cargo.toml"}]}),
            serde_json::json!({"packages": [{"name": "x", "source": null, "targets": []}]}),
            serde_json::json!({"packages": [{"name": "x", "source": null, "manifest_path": "/r/Cargo.toml"}]}),
            serde_json::json!({"packages": [{"name": "x", "source": null, "manifest_path": "/r/Cargo.toml",
                                             "targets": [{"name": "x"}]}]}),
        ] {
            assert!(resolve_graph(&broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn only_paths_under_the_checkout_count() {
        let top = Path::new("/repo");
        let manifests = [
            "/repo/Cargo.toml",
            "/repo/codegen/Cargo.toml",
            "/repo/vendor/pathmac/Cargo.toml",
            "/repo-other/Cargo.toml",
            "/outside/Cargo.toml",
        ]
        .map(PathBuf::from);
        let dirs = package_dirs(top, &manifests).expect("dirs");
        assert_eq!(dirs, set(&["", "codegen", "vendor/pathmac"]));
        let twice = ["/repo/a/Cargo.toml", "/repo/a/Cargo.toml"].map(PathBuf::from);
        assert!(package_dirs(top, &twice).is_err());
        let sources = ["/repo/shared/lib.rs", "/repo-other/lib.rs"].map(PathBuf::from);
        assert_eq!(
            contained(top, &sources).expect("contained"),
            set(&["shared/lib.rs"])
        );
    }

    #[test]
    fn every_local_input_refreshes_and_vendored_sources_do_not() {
        // Classification never reads manifests or sources: an escaped build
        // key, a default build.rs, a current_dir() script and a proc-macro
        // caller refresh like any other local package's files.
        let listed = [
            "Cargo.toml",
            "Cargo.lock",
            "src/lib.rs",
            "codegen/Cargo.toml",
            "codegen/gen.rs",
            "docs/notes.md",
            "vendor/serde/Cargo.toml",
            "vendor/serde/src/lib.rs",
            "vendor/serde/local/Cargo.toml",
            "vendor/serde/local/src/lib.rs",
        ]
        .map(str::to_owned);
        let local = set(&["", "codegen", "vendor/serde/local"]);
        let foreign = set(&["vendor/serde"]);
        assert_eq!(
            owned_files(&local, &foreign, &set(&[]), &listed).expect("covered"),
            [
                "Cargo.toml",
                "Cargo.lock",
                "src/lib.rs",
                "codegen/Cargo.toml",
                "codegen/gen.rs",
                "docs/notes.md",
                "vendor/serde/local/Cargo.toml",
                "vendor/serde/local/src/lib.rs"
            ]
        );
        // A virtual workspace: the member's target lies outside its
        // directory, and unowned files refresh too.
        let virtual_listed = [
            "Cargo.toml",
            "pkg/Cargo.toml",
            "pkg/src/main.rs",
            "shared/lib.rs",
            "vendor/dep/Cargo.toml",
            "vendor/dep/lib.rs",
        ]
        .map(str::to_owned);
        assert_eq!(
            owned_files(
                &set(&["pkg"]),
                &set(&["vendor/dep"]),
                &set(&["shared/lib.rs"]),
                &virtual_listed
            )
            .expect("covered"),
            [
                "Cargo.toml",
                "pkg/Cargo.toml",
                "pkg/src/main.rs",
                "shared/lib.rs"
            ]
        );
        // A prefix is not a parent directory.
        let prefix = ["pla/Cargo.toml", "plain/lib.rs"].map(str::to_owned);
        assert_eq!(
            owned_files(&set(&["pla"]), &set(&["plain"]), &set(&[]), &prefix).expect("covered"),
            ["pla/Cargo.toml"]
        );
        // A manifest or target source Git cannot see refuses.
        assert!(owned_files(&set(&["ignored"]), &set(&[]), &set(&[]), &listed).is_err());
        assert!(owned_files(&set(&[""]), &set(&[]), &set(&["gen/out.rs"]), &listed).is_err());
        // A local target inside a registry or Git package is ambiguous.
        let vendored_target = set(&["vendor/serde/src/lib.rs"]);
        assert!(owned_files(&local, &foreign, &vendored_target, &listed).is_err());
    }

    #[test]
    fn untrusted_or_foreign_affine_slots_refuse_the_gate() {
        assert!(prepare(Path::new("/b"), &Prior::Unknown, &Resolution::default()).is_err());
        let affine = Prior::Known(Built::Affine(PathBuf::from("/a")));
        assert!(prepare(Path::new("/b"), &affine, &Resolution::default()).is_err());
        assert_eq!(affine.foreign(Path::new("/b")), Some(PathBuf::from("/a")));
        let own =
            prepare(Path::new("/a"), &affine, &Resolution::default()).expect("own affine slot");
        assert!(!own.cross && own.describe().is_none());
        assert!(
            matches!(own.finish(Path::new("/a")), Some(Built::Affine(path)) if path == Path::new("/a"))
        );
        assert!(Prior::Unknown.unchanged().is_none() && Prior::Fresh.unchanged().is_none());
    }

    #[test]
    fn version_check_runs_where_the_command_runs() {
        let command = version_command();
        assert_eq!(command.get_current_dir(), None);
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["-V"]);
    }

    #[test]
    fn only_exact_verified_cargo_releases_share() {
        for version in [
            "cargo 1.95.0 (f2d3ce0bd 2026-03-21)",
            "cargo 1.96.1 (356927216 2026-06-26)",
            "cargo 1.97.1 (c980f4866 2026-06-30)",
            "cargo 1.98.1 (797e8a9bc 2026-08-05)",
            "cargo 1.99.1 (5f94df478 2026-08-27)",
        ] {
            assert!(is_verified(version), "{version}");
        }
        for version in [
            "cargo 1.94.1 (29ea6fb6a 2026-03-24)",
            "cargo 1.96.0 (356927216 2026-06-26)",
            "cargo 1.97.0 (c980f4866 2026-06-30)",
            "cargo 1.99.1 (0123456789 2026-08-27)",
            "cargo 1.100.0 (5f94df478 2026-08-27)",
            "cargo 1.97.1",
            "",
        ] {
            assert!(!is_verified(version), "{version}");
        }
        let fresh = Gate {
            note: Some(
                "cargo 1.94.1 (29ea6fb6a 2026-03-24) is not a Cargo release Oxide verified"
                    .to_owned(),
            ),
            ..Gate::affine()
        };
        assert!(fresh
            .describe()
            .is_some_and(|line| line.starts_with("freshness gate off: cargo 1.94.1")));
        assert!(fresh
            .finish(Path::new("/checkout"))
            .is_some_and(|built| matches!(built, Built::Affine(_))));
    }

    #[test]
    fn origin_round_trips_and_refuses_unknown_fields() {
        let origin = Origin {
            checkout: PathBuf::from("/a"),
            content: content("t", &[("x", "1")]),
            content_version: CONTENT_VERSION,
            anchor_ns: 7,
            uncertain: BTreeSet::from(["x".to_owned()]),
        };
        let bytes = serde_json::to_vec(&origin).expect("serialise");
        let back: Origin = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(back.content, origin.content);
        assert_eq!(back.content_version, CONTENT_VERSION);
        // A legacy record parses with an empty schema and version 0.
        let legacy: Origin = serde_json::from_str(
            r#"{"checkout":"/a","content":{"tree":"t","dirty":{}},"anchor_ns":1,"uncertain":[]}"#,
        )
        .expect("legacy");
        assert_eq!(legacy.content_version, 0);
        assert!(legacy.content.submodules.is_empty());
        assert!(!legacy.content.has_submodules);
        assert!(serde_json::from_str::<Origin>(
            r#"{"checkout":"/a","content":{"tree":"t","dirty":{}},"anchor_ns":1,"uncertain":[],"extra":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Origin>(
            r#"{"checkout":"/a","content":{"tree":"t","dirty":{},"extra":1},"anchor_ns":1,"uncertain":[]}"#
        )
        .is_err());
    }

    #[test]
    fn submodule_states_serialize_both_supported_shapes() {
        let states = BTreeMap::from([
            ("absent".to_owned(), SubmoduleState::Uninitialized),
            (
                "pinned".to_owned(),
                SubmoduleState::Exact {
                    head: "abc".to_owned(),
                    raw: "def".to_owned(),
                },
            ),
        ]);
        let json = serde_json::to_value(&states).expect("serialise");
        assert_eq!(json["absent"], serde_json::json!("uninit"));
        assert_eq!(
            json["pinned"],
            serde_json::json!({"exact": {"head": "abc", "raw": "def"}})
        );
        let back: BTreeMap<String, SubmoduleState> = serde_json::from_value(json).expect("parse");
        assert_eq!(back, states);
        assert!(serde_json::from_str::<SubmoduleState>(r#"{"exact":{"head":"h"}}"#).is_err());
        assert!(serde_json::from_str::<SubmoduleState>(r#"{"dirty":true}"#).is_err());
    }

    #[test]
    fn a_legacy_record_never_vouches_for_a_checkout_with_gitlinks() {
        let mut current = content("t", &[]);
        assert!(compatible(&origin_with(0, current.clone()), &current).is_ok());
        assert!(compatible(&origin_with(CONTENT_VERSION, current.clone()), &current).is_ok());
        assert!(compatible(&origin_with(CONTENT_VERSION + 1, current.clone()), &current).is_err());
        current
            .submodules
            .insert("sub".to_owned(), SubmoduleState::Uninitialized);
        assert!(compatible(&origin_with(0, current.clone()), &current).is_err());
        assert!(compatible(&origin_with(CONTENT_VERSION, current.clone()), &current).is_ok());
        assert!(compatible(&origin_with(2, current.clone()), &current).is_err());
    }

    #[test]
    fn a_version_one_record_without_submodule_state_refuses_gitlinks() {
        // The field is absent from this record even though it claims version
        // 1; it must not be treated as an empty, equivalent state.
        let legacy: Origin = serde_json::from_str(
            r#"{"checkout":"/a","content":{"tree":"t","dirty":{}},"content_version":1,"anchor_ns":1,"uncertain":[]}"#,
        )
        .expect("parses");
        assert!(!legacy.content.has_submodules);
        let mut current = content("t", &[]);
        assert!(compatible(&legacy, &current).is_ok());
        current
            .submodules
            .insert("sub".to_owned(), SubmoduleState::Uninitialized);
        assert!(compatible(&legacy, &current).is_err());
        // A written record always carries the field, even when empty.
        let written = serde_json::to_value(origin_with(CONTENT_VERSION, content("t", &[])))
            .expect("serialise");
        assert!(written["content"].get("submodules").is_some());
    }

    #[test]
    fn git_calls_disable_submodule_recursion_and_fsmonitor() {
        let command = git_command(Path::new("/w"), &["status"]);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                "-c",
                "submodule.recurse=0",
                "-c",
                "core.fsmonitor=false",
                "status"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/w")));
        let envs: Vec<_> = command.get_envs().collect();
        for (name, value) in [("GIT_OPTIONAL_LOCKS", "0"), ("GIT_TERMINAL_PROMPT", "0")] {
            assert!(
                envs.iter()
                    .any(|(key, env)| *key == std::ffi::OsStr::new(name)
                        && *env == Some(std::ffi::OsStr::new(value))),
                "{name} missing from {envs:?}"
            );
        }
    }

    #[test]
    fn git_stdout_is_capped_cumulatively() {
        let mut git = Git::new();
        assert!(git.charge(MAX_GIT_BYTES / 2).is_ok());
        assert!(git.charge(MAX_GIT_BYTES / 2).is_ok());
        assert!(git.charge(1).is_err());
        let mut exhausted = Git::new();
        assert!(exhausted.charge(MAX_GIT_BYTES).is_ok());
        assert!(exhausted.charge(0).is_ok());
        assert!(exhausted.charge(1).is_err());
    }

    #[test]
    fn retained_git_pipe_refuses_within_the_call_deadline() {
        // An owned helper Git started keeps stdout and stderr open after Git
        // exits; settlement must stop at the call's deadline, not after it.
        let mut git = Git::new();
        git.call_time = Duration::from_millis(500);
        git.settle_time = Duration::from_secs(5);
        let started = Instant::now();
        let result = git.run(
            Path::new("/"),
            &["-c", "alias.hold=!sleep 2 &", "hold"],
            None,
        );
        let elapsed = started.elapsed();
        let error = result.expect_err("a retained pipe must refuse");
        assert!(error.contains("in time"), "{error}");
        assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
        assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
        assert!(git.waited < Duration::from_millis(1500));
    }

    #[test]
    fn submodule_state_changes_differ_without_a_tree_change() {
        let mut a = content("t", &[("same", "1")]);
        let mut b = content("t", &[("same", "1")]);
        assert!(submodule_differences(&a, &b).is_empty());
        let uninit = SubmoduleState::Uninitialized;
        let exact = SubmoduleState::Exact {
            head: "h".to_owned(),
            raw: "r".to_owned(),
        };
        a.submodules.insert("sub".to_owned(), uninit.clone());
        assert_eq!(
            submodule_differences(&a, &b)
                .into_iter()
                .collect::<Vec<_>>(),
            ["sub"]
        );
        b.submodules.insert("sub".to_owned(), exact.clone());
        assert_eq!(
            submodule_differences(&a, &b)
                .into_iter()
                .collect::<Vec<_>>(),
            ["sub"]
        );
        a.submodules.insert("sub".to_owned(), exact);
        assert!(submodule_differences(&a, &b).is_empty());
        let other = SubmoduleState::Exact {
            head: "h".to_owned(),
            raw: "other".to_owned(),
        };
        b.submodules.insert("sub".to_owned(), other);
        assert_eq!(
            submodule_differences(&a, &b)
                .into_iter()
                .collect::<Vec<_>>(),
            ["sub"]
        );
    }

    #[test]
    fn declared_gitlinks_come_from_a_matching_index_and_head() {
        let oid = "1111111111111111111111111111111111111111";
        let file = "2222222222222222222222222222222222222222";
        let index = [
            format!("H 160000 {oid} 0\tsub"),
            format!("H 100644 {file} 0\tCargo.toml"),
        ];
        let tree = [
            format!("160000 commit {oid}\tsub"),
            format!("100644 blob {file}\tCargo.toml"),
        ];
        assert_eq!(
            declared_gitlinks(&index, &tree).expect("declares"),
            BTreeMap::from([("sub".to_owned(), oid.to_owned())])
        );
        // Any disagreement, index flag or stage refuses.
        let changed = [format!("H 160000 {file} 0\tsub")];
        assert!(declared_gitlinks(&changed, &tree).is_err());
        let assume = [format!("h 160000 {oid} 0\tsub")];
        assert!(declared_gitlinks(&assume, &tree).is_err());
        let sparse = [format!("S 160000 {oid} 0\tsub")];
        assert!(declared_gitlinks(&sparse, &tree).is_err());
        let unmerged = [format!("M 160000 {oid} 1\tsub")];
        assert!(declared_gitlinks(&unmerged, &tree).is_err());
        assert!(declared_gitlinks(&[], &tree).is_err());
        assert!(declared_gitlinks(&index, &tree[1..]).is_err());
        let escaped = [format!("H 160000 {oid} 0\t../sub")];
        let escaped_tree = [format!("160000 commit {oid}\t../sub")];
        assert!(declared_gitlinks(&escaped, &escaped_tree).is_err());
        let many: Vec<String> = (0..=MAX_SUBMODULES)
            .map(|index| format!("H 160000 {oid} 0\tsub{index}"))
            .collect();
        let many_tree: Vec<String> = (0..=MAX_SUBMODULES)
            .map(|index| format!("160000 commit {oid}\tsub{index}"))
            .collect();
        assert!(declared_gitlinks(&many, &many_tree).is_err());
    }

    #[test]
    fn only_plain_repository_paths_are_covered() {
        for path in ["sub", "a/b", "a b", "ü"] {
            assert!(valid_repo_path(path), "{path}");
        }
        for path in ["", "/abs", "../x", "a/../b", "a/./b", "a\nb", "."] {
            assert!(!valid_repo_path(path), "{path}");
        }
    }

    #[test]
    fn raw_digest_is_ordered_and_deterministic() {
        let paths = ["a".to_owned(), "b".to_owned()];
        let digests = ["1".to_owned(), "2".to_owned()];
        assert_eq!(raw_digest(&paths, &digests), raw_digest(&paths, &digests));
        assert_ne!(
            raw_digest(&paths, &digests),
            raw_digest(
                &["b".to_owned(), "a".to_owned()],
                &["2".to_owned(), "1".to_owned()]
            )
        );
        assert_eq!(raw_digest(&[], &[]).len(), 64);
    }

    #[test]
    fn refresh_skips_only_declared_gitlink_directories() {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("refresh-{:x}", std::process::id()));
        fs::create_dir(&dir).expect("scratch dir");
        let declared = dir.join("sub");
        let plain = dir.join("plain");
        fs::create_dir(&declared).expect("declared dir");
        fs::create_dir(&plain).expect("plain dir");
        fs::write(declared.join("inner.rs"), b"fn main() {}").expect("inner file");
        let anchor = now_ns();
        let mut states = BTreeMap::new();
        states.insert("sub".to_owned(), SubmoduleState::Uninitialized);
        let before = mtime_ns(&fs::symlink_metadata(&declared).expect("stat"));
        assert_eq!(
            refresh(&dir, &["sub".to_owned()], anchor, &states).expect("skips"),
            0
        );
        assert_eq!(
            mtime_ns(&fs::symlink_metadata(&declared).expect("stat")),
            before
        );
        // A declared child's own tracked file is an ordinary file input.
        assert_eq!(
            refresh(&dir, &["sub/inner.rs".to_owned()], anchor, &states).expect("touches"),
            1
        );
        // Any other directory keeps the old refusal.
        assert!(refresh(&dir, &["plain".to_owned()], anchor, &states).is_err());
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
