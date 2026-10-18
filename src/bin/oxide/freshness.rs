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
//! not covered.

use crate::cli::Resolution;
use crate::MIB;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Dirty files in one checkout and uncertain paths per slot. Beyond this the
/// gate refuses and the slot stays checkout-affine.
pub const MAX_PATHS: usize = 4096;
/// Files listed for the path-change step.
const MAX_LISTED: usize = 200_000;
/// `cargo metadata` output and lifetime for the path-change step.
const MAX_METADATA_BYTES: u64 = 64 * MIB;
const MAX_METADATA_ERROR_BYTES: u64 = 64 * 1024;
const METADATA_TIME: Duration = Duration::from_secs(60);
const DELETED: &str = "deleted";
pub const LIMIT_NOTE: &str = "inputs outside Git or ignored by it are not checked";
/// Cargo's wording when `--locked` or `--frozen` stops it creating or
/// updating the lock file.
const LOCK_REFUSAL: &str = "was passed to prevent this";

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

/// One checkout's content: HEAD's tree plus the Git blob digest of every
/// file that differs from it (tracked changes and untracked files Git does
/// not ignore). Paths are relative to the repository top level.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Content {
    pub tree: String,
    pub dirty: BTreeMap<String, String>,
}

/// What a slot last compiled.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    /// Canonical workspace root of the compiling checkout.
    pub checkout: PathBuf,
    pub content: Content,
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
    /// `None`: a checkout without Git content; the slot stays affine.
    before: Option<(PathBuf, Content)>,
    /// Paths that differed from the prior origin.
    changed: BTreeSet<String>,
    carried: BTreeSet<String>,
    /// The slot was last compiled by another checkout.
    pub cross: bool,
    pub refreshed: usize,
    pub packages: usize,
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
        }
    }

    /// Text for the summary line when the gate did visible work.
    pub fn describe(&self) -> Option<String> {
        if self.cross {
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
        let (_, after) = snapshot(&top).ok()?;
        let mut uncertain = self.carried;
        uncertain.extend(self.changed);
        uncertain.extend(differing(&top, &before, &after).ok()?);
        if uncertain.len() > MAX_PATHS {
            return None;
        }
        Some(Built::Content(Origin {
            checkout: checkout.to_path_buf(),
            content: after,
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
            "the slot holds another checkout's build without Git content".to_owned(),
        )),
        // Nothing compiled yet: a checkout without Git content stays affine.
        Prior::Fresh => Ok(match snapshot(checkout) {
            Ok(before) => Gate {
                before: Some(before),
                ..Gate::affine()
            },
            Err(_) => Gate::affine(),
        }),
        Prior::Known(Built::Content(origin)) => {
            let (top, content) = snapshot(checkout)?;
            let changed = differing(&top, &origin.content, &content)?;
            let cross = origin.checkout != checkout;
            let mut stale: BTreeSet<&String> = changed.iter().collect();
            if cross {
                stale.extend(&origin.uncertain);
            }
            let mut refreshed = refresh(&top, stale, origin.anchor_ns)?;
            let mut packages = 0;
            if cross {
                let metadata = cargo_metadata(checkout, request)?;
                let (count, files) = local_package_files(&top, &metadata)?;
                packages = count;
                refreshed += refresh(&top, files.iter(), origin.anchor_ns)?;
            }
            Ok(Gate {
                before: Some((top, content)),
                changed,
                carried: origin.uncertain.clone(),
                cross,
                refreshed,
                packages,
            })
        }
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
        })
}

fn git(top: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(top)
        // Status must not rewrite the index while reading it.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not start git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
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

/// Repository top level and content for the checkout containing `path`.
fn snapshot(path: &Path) -> Result<(PathBuf, Content), String> {
    let top = utf8(git(path, &["rev-parse", "--show-toplevel"])?)?;
    let top = fs::canonicalize(top.trim())
        .map_err(|error| format!("cannot resolve the repository: {error}"))?;
    let tree = utf8(git(&top, &["rev-parse", "--verify", "HEAD^{tree}"])?)?
        .trim()
        .to_owned();
    let status = git(
        &top,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=none",
        ],
    )?;
    let paths = status_paths(&nul_separated(status)?)?;
    let mut dirty = BTreeMap::new();
    let mut present = Vec::new();
    for path in paths {
        match fs::symlink_metadata(top.join(&path)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                dirty.insert(path, DELETED.to_owned());
            }
            Err(error) => return Err(format!("cannot read {path}: {error}")),
            Ok(metadata) if metadata.is_dir() => {
                return Err(format!(
                    "{path} is a submodule or directory the gate does not cover"
                ));
            }
            Ok(_) => present.push(path),
        }
    }
    for (path, digest) in present.iter().zip(hash_objects(&top, &present)?) {
        dirty.insert(path.clone(), digest);
    }
    Ok((top, Content { tree, dirty }))
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
fn hash_objects(top: &Path, paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if paths.iter().any(|path| path.contains('\n')) {
        return Err("a changed path contains a newline".to_owned());
    }
    let mut child = Command::new("git")
        .args(["hash-object", "--no-filters", "--stdin-paths"])
        .current_dir(top)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start git: {error}"))?;
    let mut input = child.stdin.take().ok_or("git stdin was not captured")?;
    let list = paths.join("\n") + "\n";
    let writer = std::thread::spawn(move || input.write_all(list.as_bytes()));
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not wait for git: {error}"))?;
    let written = writer.join().map_err(|_| "git input writer panicked")?;
    if !output.status.success() || written.is_err() {
        return Err(format!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let digests: Vec<String> = utf8(output.stdout)?.lines().map(str::to_owned).collect();
    if digests.len() == paths.len() {
        Ok(digests)
    } else {
        Err("git hash-object returned the wrong number of digests".to_owned())
    }
}

/// Every path whose content may differ between two snapshots of the same
/// repository. Run in the checkout whose object store must hold both trees.
fn differing(top: &Path, a: &Content, b: &Content) -> Result<BTreeSet<String>, String> {
    let mut paths = BTreeSet::new();
    if a.tree != b.tree {
        let diff = git(
            top,
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                &a.tree,
                &b.tree,
            ],
        )?;
        paths.extend(nul_separated(diff)?);
    }
    paths.extend(dirty_differences(a, b));
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

/// Set mtime to now on each listed checkout file that is not newer than the
/// anchor. Missing files are skipped: Cargo rebuilds a unit whose input is
/// gone. A symlink is followed only inside the checkout.
fn refresh<'a>(
    top: &Path,
    paths: impl IntoIterator<Item = &'a String>,
    anchor_ns: u64,
) -> Result<usize, String> {
    let mut refreshed = 0;
    for path in paths {
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
/// a registry or Git package (vendored sources). So a local unit rebuilds
/// whatever it reads, inside or outside its own directory. A local target or
/// a followed symlink inside a vendored package is ambiguous and refused.
/// Returns the local package count and the files.
fn local_package_files(
    top: &Path,
    metadata: &serde_json::Value,
) -> Result<(usize, Vec<String>), String> {
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
    let listed = nul_separated(git(
        top,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?)?;
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

struct CargoOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    overflow: bool,
    stderr: String,
}

/// Run one Cargo call, bounded in output and lifetime.
fn run_cargo(mut command: Command, stdout_cap: u64) -> Result<CargoOutput, String> {
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
        assert_eq!(command.get_current_dir(), None);
        assert_eq!(command.get_envs().count(), 0);
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
    fn origin_round_trips_and_refuses_unknown_fields() {
        let origin = Origin {
            checkout: PathBuf::from("/a"),
            content: content("t", &[("x", "1")]),
            anchor_ns: 7,
            uncertain: BTreeSet::from(["x".to_owned()]),
        };
        let bytes = serde_json::to_vec(&origin).expect("serialise");
        let back: Origin = serde_json::from_slice(&bytes).expect("parse");
        assert_eq!(back.content, origin.content);
        assert!(serde_json::from_str::<Origin>(
            r#"{"checkout":"/a","content":{"tree":"t","dirty":{}},"anchor_ns":1,"uncertain":[],"extra":1}"#
        )
        .is_err());
    }
}
