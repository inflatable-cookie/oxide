//! Cross-checkout freshness gate (W-8).
//!
//! Cargo 1.97.1 keys workspace units by relative path and treats an input as
//! fresh when its mtime is not newer than the unit's dep-info. A slot shared
//! by two checkouts therefore needs the second checkout's differing files to
//! look newer than the slot's last build. Each slot records its last compile
//! origin; before Cargo runs, the gate sets mtime to now on every checkout
//! file whose content may differ from what the slot compiled and that is not
//! newer than the build anchor. On a checkout-path change it does the same
//! for packages that can embed the path. File contents, Cargo fingerprints
//! and the slot are never touched. Inputs outside Git or ignored by it are
//! not covered.

use crate::MIB;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Dirty files in one checkout and uncertain paths per slot. Beyond this the
/// gate refuses and the slot stays checkout-affine.
pub const MAX_PATHS: usize = 4096;
/// Files listed for the path-change step.
const MAX_LISTED: usize = 200_000;
/// Source bytes read looking for `CARGO_MANIFEST_DIR`.
const MAX_SCAN_BYTES: u64 = 64 * MIB;
const DELETED: &str = "deleted";
pub const LIMIT_NOTE: &str = "inputs outside Git or ignored by it are not checked";

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

/// The slot's history as the leasing command found it.
#[derive(Clone, Debug)]
pub enum Prior {
    /// A new slot: nothing compiled yet.
    Fresh,
    /// The last command recorded this origin and nothing ran since.
    Known(Origin),
    /// A command crashed, could not record its origin, or predates the gate.
    /// The slot is never shared across checkouts again.
    Unknown,
}

/// The gate's work before Cargo, kept to record the new origin afterwards.
pub struct Gate {
    before: Option<(PathBuf, Content)>,
    /// Paths that differed from the prior origin.
    changed: BTreeSet<String>,
    /// `None` when the new origin cannot be trusted.
    carried: Option<BTreeSet<String>>,
    /// The slot was last compiled by another checkout.
    pub cross: bool,
    pub refreshed: usize,
    pub packages: usize,
}

impl Gate {
    /// No usable snapshot: the command builds as a checkout-affine slot would
    /// and its slot is never shared afterwards.
    pub fn untrusted() -> Self {
        Self {
            before: None,
            changed: BTreeSet::new(),
            carried: None,
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

    /// The origin to record once Cargo's group is empty, or `None` when it
    /// cannot be trusted (the slot then stays checkout-affine).
    pub fn finish(self, checkout: &Path) -> Option<Origin> {
        let anchor_ns = now_ns();
        let (top, before) = self.before?;
        let mut uncertain = self.carried?;
        let (_, after) = snapshot(&top).ok()?;
        uncertain.extend(self.changed);
        uncertain.extend(differing(&top, &before, &after).ok()?);
        if uncertain.len() > MAX_PATHS {
            return None;
        }
        Some(Origin {
            checkout: checkout.to_path_buf(),
            content: after,
            anchor_ns,
            uncertain,
        })
    }
}

/// Run the gate for `checkout` before Cargo. An error means the slot's
/// history cannot be reconciled with this checkout.
pub fn prepare(checkout: &Path, prior: &Prior) -> Result<Gate, String> {
    match prior {
        Prior::Unknown => Ok(Gate::untrusted()),
        Prior::Fresh => Ok(match snapshot(checkout) {
            Ok(before) => Gate {
                before: Some(before),
                carried: Some(BTreeSet::new()),
                ..Gate::untrusted()
            },
            Err(_) => Gate::untrusted(),
        }),
        Prior::Known(origin) => {
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
                let (count, files) = embedding_files(&top)?;
                packages = count;
                refreshed += refresh(&top, files.iter(), origin.anchor_ns)?;
            }
            Ok(Gate {
                before: Some((top, content)),
                changed,
                carried: Some(origin.uncertain.clone()),
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

/// Files of every repository package that can embed its checkout path: one
/// with a build script, or whose sources mention `CARGO_MANIFEST_DIR`. A file
/// belongs to the package with the deepest `Cargo.toml` above it. Returns the
/// package count and their files.
fn embedding_files(top: &Path) -> Result<(usize, Vec<String>), String> {
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
    let packages = package_files(&listed);
    let mut budget = MAX_SCAN_BYTES;
    let mut count = 0;
    let mut files = Vec::new();
    for (dir, owned) in packages {
        if embeds_path(top, &dir, &owned, &mut budget)? {
            count += 1;
            files.extend(owned);
        }
    }
    Ok((count, files))
}

/// Group listed files by owning package directory (`""` is the top level).
fn package_files(listed: &[String]) -> BTreeMap<String, Vec<String>> {
    let dirs: BTreeSet<&str> = listed
        .iter()
        .filter_map(|path| match path.rsplit_once('/') {
            Some((dir, "Cargo.toml")) => Some(dir),
            None if path == "Cargo.toml" => Some(""),
            _ => None,
        })
        .collect();
    let mut packages: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in listed {
        let mut dir = path.as_str();
        let owner = loop {
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
            if dirs.contains(dir) {
                break Some(dir);
            }
            if dir.is_empty() {
                break None;
            }
        };
        if let Some(owner) = owner {
            packages
                .entry(owner.to_owned())
                .or_default()
                .push(path.clone());
        }
    }
    packages
}

fn embeds_path(top: &Path, dir: &str, owned: &[String], budget: &mut u64) -> Result<bool, String> {
    let join = |name: &str| {
        if dir.is_empty() {
            name.to_owned()
        } else {
            format!("{dir}/{name}")
        }
    };
    let build_rs = join("build.rs");
    if owned.contains(&build_rs) {
        return Ok(true);
    }
    if let Some(manifest) = read_bounded(top, &join("Cargo.toml"), budget)? {
        if declares_build(&String::from_utf8_lossy(&manifest)) {
            return Ok(true);
        }
    }
    for path in owned.iter().filter(|path| path.ends_with(".rs")) {
        if let Some(source) = read_bounded(top, path, budget)? {
            if source
                .windows(b"CARGO_MANIFEST_DIR".len())
                .any(|window| window == b"CARGO_MANIFEST_DIR")
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A manifest line `build = ...` names a build script outside `build.rs`.
fn declares_build(manifest: &str) -> bool {
    manifest.lines().any(|line| {
        line.trim_start()
            .strip_prefix("build")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

fn read_bounded(top: &Path, path: &str, budget: &mut u64) -> Result<Option<Vec<u8>>, String> {
    let file = match fs::File::open(top.join(path)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {path}: {error}")),
        Ok(file) => file,
    };
    let mut bytes = Vec::new();
    file.take(budget.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {path}: {error}"))?;
    let used = bytes.len() as u64;
    if used > *budget {
        return Err("repository sources exceed the path-change scan limit".to_owned());
    }
    *budget -= used;
    Ok(Some(bytes))
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

    #[test]
    fn files_belong_to_the_deepest_package() {
        let listed = [
            "Cargo.toml",
            "build.rs",
            "src/lib.rs",
            "helper/Cargo.toml",
            "helper/src/lib.rs",
            "helper/nested/data.txt",
            "docs/notes.md",
        ]
        .map(str::to_owned);
        let packages = package_files(&listed);
        assert_eq!(
            packages[""],
            ["Cargo.toml", "build.rs", "src/lib.rs", "docs/notes.md"]
        );
        assert_eq!(
            packages["helper"],
            [
                "helper/Cargo.toml",
                "helper/src/lib.rs",
                "helper/nested/data.txt"
            ]
        );
        let outside = package_files(&["crates/a/x.rs".to_owned()]);
        assert!(outside.is_empty());
    }

    #[test]
    fn manifest_build_key_is_detected() {
        assert!(declares_build("[package]\nbuild = \"gen.rs\"\n"));
        assert!(declares_build("  build=false"));
        assert!(!declares_build(
            "[build-dependencies]\nbuild-helper = \"1\"\n"
        ));
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
