//! `oxide clean`: the checkout's private outputs and slot affinity.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (`clean`). A
//! whole-target clean is planned and removed by Oxide itself: the plan
//! enumerates every path under `<workspace>/target`, refuses top-level
//! entries Cargo does not create unless `--include-foreign` is given, and
//! the dry run prints that same plan. Removal deletes only planned paths and
//! never follows a symlink. A package, profile or doc selection is passed
//! to Cargo as before. Shared slots are never touched.

use crate::{apply_storage, cargo_home, cli, identity, process, store, CARGO};
use std::ffi::{OsStr, OsString};
use std::fs::Metadata;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use store::{Cleaning, Store};

/// Enumeration bounds: past either, the plan refuses instead of truncating.
const MAX_ENTRIES: usize = 1_000_000;
const MAX_DEPTH: usize = 256;
/// Foreign names shown in one refusal.
const SHOWN: usize = 20;

/// Top-level files Cargo 1.97.1 writes in a target directory (pinned commit
/// `c980f4866`): `CACHEDIR.TAG` (`cargo-util` `paths.rs`
/// `exclude_from_backups`), `.rustc_info.json` (`util/context/mod.rs`
/// L511), `.future-incompat-report.json` (`core/compiler/future_incompat.rs`
/// L98) and `.rustdoc_fingerprint.json` (`fingerprint/rustdoc.rs` L198).
const OWNED_FILES: [&str; 4] = [
    "CACHEDIR.TAG",
    ".rustc_info.json",
    ".future-incompat-report.json",
    ".rustdoc_fingerprint.json",
];
/// Top-level directories at the same commit: `doc` and `cargo-timings`
/// (`core/compiler/layout.rs` L312-L313), `tmp` (L329), `package`
/// (`ops/cargo_package/mod.rs` L166, L220) and `.metabuild`
/// (`core/manifest.rs` L740).
const OWNED_DIRS: [&str; 5] = ["doc", "cargo-timings", "tmp", "package", ".metabuild"];
/// Entries of a target-triple directory (`layout.rs` L243-L246 push the
/// triple onto both roots): its profile directories plus these.
const TRIPLE_FILES: [&str; 2] = ["CACHEDIR.TAG", ".rustdoc_fingerprint.json"];
const TRIPLE_DIRS: [&str; 3] = ["doc", "cargo-timings", "tmp"];
/// Every profile directory Cargo uses holds one of these lock files
/// (`layout.rs` L262-L291), except on NFS, where Oxide refuses instead.
const PROFILE_LOCKS: [&str; 2] = [".cargo-lock", ".cargo-build-lock"];

/// A whole-target clean or a selection Cargo performs.
#[derive(Debug, Default, Eq, PartialEq)]
struct Request {
    dry_run: bool,
    include_foreign: bool,
    /// `-p`, `--workspace`, `--doc`, `--profile` or `--release`.
    selected: bool,
    /// Arguments for Cargo, without `--include-foreign`.
    cargo_args: Vec<OsString>,
    /// An argument a whole-target clean does not accept.
    unsupported: Option<String>,
}

fn parse(args: &[OsString]) -> Request {
    let mut request = Request::default();
    let mut iter = args.iter();
    while let Some(argument) = iter.next() {
        let text = argument.to_str().unwrap_or_default();
        if text == "--include-foreign" {
            request.include_foreign = true;
            continue;
        }
        request.cargo_args.push(argument.clone());
        let takes_value = matches!(
            text,
            "-p" | "--package" | "--profile" | "--manifest-path" | "--color"
        );
        match text {
            "--dry-run" => request.dry_run = true,
            "-p" | "--package" | "--workspace" | "--doc" | "--release" | "-r" | "--profile" => {
                request.selected = true;
            }
            _ if text.starts_with("--package=")
                || text.starts_with("--profile=")
                || (text.starts_with("-p") && !text.starts_with("--")) =>
            {
                request.selected = true;
            }
            "--manifest-path" | "--color" | "-q" | "--quiet" | "-v" | "-vv" | "--verbose"
            | "--locked" | "--frozen" | "--offline" => {}
            _ if text.starts_with("--manifest-path=") || text.starts_with("--color=") => {}
            _ => {
                if request.unsupported.is_none() {
                    request.unsupported = Some(argument.to_string_lossy().into_owned());
                }
            }
        }
        if takes_value {
            if let Some(value) = iter.next() {
                request.cargo_args.push(value.clone());
            }
        }
    }
    request
}

/// `oxide clean`.
pub fn run(args: &[OsString]) -> Result<u8, String> {
    if args.first().is_some_and(|first| first == "--cargo-cache") {
        return cargo_home::run_cache_clean(&args[1..]);
    }
    if args.iter().any(|argument| argument == "--cargo-cache") {
        return Err(
            "--cargo-cache must come first and alone: `oxide clean --cargo-cache`".to_owned(),
        );
    }
    cli::refuse_storage_overrides(args)?;
    let request = parse(args);
    let manifest = cli::manifest_path(args);
    let workspace = identity::locate_workspace(CARGO, manifest.as_deref())?;
    if request.selected {
        if request.include_foreign {
            return Err("--include-foreign applies only to a whole-target clean; -p, --workspace, --doc, --profile and --release leave the target's other entries alone".to_owned());
        }
        if request.dry_run {
            return Err("clean --dry-run lists paths only for a whole-target clean: Cargo's -p, --workspace, --doc, --profile and --release dry run does not print every path it would remove".to_owned());
        }
        return run_selected(&request.cargo_args, &workspace);
    }
    if let Some(argument) = request.unsupported {
        return Err(format!(
            "clean: {argument} is not supported for a whole-target clean, which Oxide plans and removes itself (accepted: --dry-run, --include-foreign, --manifest-path, -q, -v, --color, --locked, --frozen, --offline)"
        ));
    }
    run_whole(&workspace, request.dry_run, request.include_foreign)
}

/// One path the plan removes.
#[derive(Debug, Eq, PartialEq)]
struct Planned {
    path: PathBuf,
    dir: bool,
}

/// The target and every path under it, parents before children.
#[derive(Debug, Default)]
struct Plan {
    entries: Vec<Planned>,
    foreign: Vec<OsString>,
    files: u64,
    dirs: u64,
    bytes: u64,
}

fn is_file(metadata: &Metadata) -> bool {
    metadata.file_type().is_file()
}

fn is_dir(metadata: &Metadata) -> bool {
    metadata.file_type().is_dir()
}

/// A real directory holding a regular-file Cargo lock.
fn profile_dir(path: &Path) -> bool {
    PROFILE_LOCKS.iter().any(|lock| {
        std::fs::symlink_metadata(path.join(lock)).is_ok_and(|metadata| is_file(&metadata))
    })
}

/// A real directory whose entries are profile directories (at least one)
/// and the other entries Cargo writes under a triple.
fn triple_dir(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    let mut profiles = 0;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        let name = entry.file_name();
        let name = name.to_str().unwrap_or_default();
        if is_file(&metadata) && TRIPLE_FILES.contains(&name) {
            continue;
        }
        if !is_dir(&metadata) {
            return false;
        }
        if TRIPLE_DIRS.contains(&name) {
            continue;
        }
        if !profile_dir(&entry.path()) {
            return false;
        }
        profiles += 1;
    }
    profiles > 0
}

/// Whether Cargo creates this top-level target entry. Names alone never
/// qualify a profile or triple directory: it must carry Cargo's lock file
/// structure. A symlink is never Cargo-owned.
fn cargo_owned(path: &Path, name: &OsStr, metadata: &Metadata) -> bool {
    let name = name.to_str().unwrap_or_default();
    if is_file(metadata) {
        return OWNED_FILES.contains(&name);
    }
    if !is_dir(metadata) {
        return false;
    }
    OWNED_DIRS.contains(&name) || profile_dir(path) || triple_dir(path)
}

/// The pool directory's device and inode, resolved through symlinks, or
/// `None` when no pool exists yet.
type Protected = Option<(u64, u64)>;

/// Enumerate `target` without following symlinks or crossing a mount. An
/// unreadable directory or a bound reached refuses: the plan is complete or
/// there is none. A directory with the pool's identity refuses, whatever
/// path names the pool.
fn plan(target: &Path, pool: Protected) -> Result<Option<Plan>, String> {
    let refuse_pool = |path: &Path, metadata: &Metadata| {
        if pool == Some((metadata.dev(), metadata.ino())) {
            Err(format!(
                "{} is the Oxide pool; shared slots are never cleaned",
                path.display()
            ))
        } else {
            Ok(())
        }
    };
    let root = match std::fs::symlink_metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", target.display())),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "{} is a symlink; Oxide does not clean through it",
                target.display()
            ))
        }
        Ok(metadata) if !is_dir(&metadata) => {
            return Err(format!("{} is not a directory", target.display()))
        }
        Ok(metadata) => metadata,
    };
    refuse_pool(target, &root)?;
    let mut plan = Plan {
        entries: vec![Planned {
            path: target.to_path_buf(),
            dir: true,
        }],
        ..Plan::default()
    };
    let mut stack = vec![(target.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|error| {
            format!(
                "cannot read {}: {error}; the plan would be incomplete",
                dir.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot read {}: {error}; the plan would be incomplete",
                    dir.display()
                )
            })?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            if metadata.dev() != root.dev() {
                return Err(format!(
                    "{} is on another filesystem; Oxide does not clean across a mount",
                    path.display()
                ));
            }
            if depth == 0 && !cargo_owned(&path, &entry.file_name(), &metadata) {
                plan.foreign.push(entry.file_name());
            }
            if plan.entries.len() > MAX_ENTRIES {
                return Err(format!(
                    "{} holds more than {MAX_ENTRIES} entries; Oxide refuses rather than remove an incomplete list",
                    target.display()
                ));
            }
            if is_dir(&metadata) {
                refuse_pool(&path, &metadata)?;
                if depth + 1 >= MAX_DEPTH {
                    return Err(format!(
                        "{} is nested deeper than {MAX_DEPTH} levels; Oxide refuses rather than remove an incomplete list",
                        path.display()
                    ));
                }
                plan.dirs += 1;
                plan.entries.push(Planned {
                    path: path.clone(),
                    dir: true,
                });
                stack.push((path, depth + 1));
            } else {
                plan.files += 1;
                plan.bytes += metadata.len();
                plan.entries.push(Planned { path, dir: false });
            }
        }
    }
    plan.foreign.sort();
    Ok(Some(plan))
}

fn names(names: &[OsString]) -> String {
    let mut shown: Vec<String> = names
        .iter()
        .take(SHOWN)
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if names.len() > SHOWN {
        shown.push(format!("and {} more", names.len() - SHOWN));
    }
    shown.join(", ")
}

/// The refusal both the dry run and a real clean give for foreign entries.
fn refuse_foreign(target: &Path, plan: &Plan) -> String {
    format!(
        "clean refused: {} holds {} top-level entr{} Cargo does not create: {}. Move {} out of the target, or pass --include-foreign to remove {} too (`oxide clean --dry-run --include-foreign` lists every path)",
        target.display(),
        plan.foreign.len(),
        if plan.foreign.len() == 1 { "y" } else { "ies" },
        names(&plan.foreign),
        if plan.foreign.len() == 1 { "it" } else { "them" },
        if plan.foreign.len() == 1 { "it" } else { "them" },
    )
}

fn summary(plan: &Plan) -> String {
    format!(
        "{} files, {} directories, {} apparent",
        plan.files,
        plan.dirs + 1,
        cargo_home::size(plan.bytes)
    )
}

/// Refuse a pool that lives inside the target, by path text and by real
/// path (an outside symlink can name a pool inside), and return the pool's
/// identity for the enumeration's own check.
fn protect_pool(target: &Path) -> Result<Protected, String> {
    let pool = store::pool_path()?;
    let inside = |pool: &Path| {
        Err(format!(
            "clean refused: the Oxide pool {} is inside {}; shared slots are never cleaned",
            pool.display(),
            target.display()
        ))
    };
    if pool.starts_with(target) {
        return inside(&pool);
    }
    match std::fs::metadata(&pool) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "clean refused: cannot resolve the Oxide pool {}: {error}",
            pool.display()
        )),
        Ok(metadata) => {
            let real = std::fs::canonicalize(&pool).map_err(|error| {
                format!(
                    "clean refused: cannot resolve the Oxide pool {}: {error}",
                    pool.display()
                )
            })?;
            let real_target =
                std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
            if real.starts_with(&real_target) {
                return inside(&pool);
            }
            Ok(Some((metadata.dev(), metadata.ino())))
        }
    }
}

fn checked_plan(
    target: &Path,
    pool: Protected,
    include_foreign: bool,
) -> Result<Option<Plan>, String> {
    let plan = plan(target, pool).map_err(|reason| format!("clean refused: {reason}"))?;
    match plan {
        Some(plan) if !plan.foreign.is_empty() && !include_foreign => {
            Err(refuse_foreign(target, &plan))
        }
        plan => Ok(plan),
    }
}

/// Remove planned paths only, children first. A path that changed type or
/// gained unplanned content is left and counted, never followed.
fn remove(plan: &Plan) -> Result<(), String> {
    let mut left = Vec::new();
    for planned in plan.entries.iter().rev() {
        if process::received_signal().is_some() {
            return Err("interrupted; the target is partly removed".to_owned());
        }
        let removed = if planned.dir {
            std::fs::remove_dir(&planned.path)
        } else {
            std::fs::remove_file(&planned.path)
        };
        match removed {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => left.push(format!("{} ({error})", planned.path.display())),
        }
    }
    if left.is_empty() {
        return Ok(());
    }
    let count = left.len();
    left.truncate(SHOWN);
    Err(format!(
        "{count} planned path(s) were left because they changed or gained entries that were not in the plan: {}",
        left.join(", ")
    ))
}

fn run_whole(workspace: &Path, dry_run: bool, include_foreign: bool) -> Result<u8, String> {
    let target = workspace.join("target");
    let pool = protect_pool(&target)?;
    // Refusal precedes waiting, deletion and any affinity change.
    let first = checked_plan(&target, pool, include_foreign)?;
    if dry_run {
        let Some(plan) = first else {
            eprintln!(
                "oxide: dry run: {} does not exist; nothing would be removed",
                target.display()
            );
            return Ok(0);
        };
        for planned in &plan.entries {
            println!("{}", planned.path.display());
        }
        let foreign = if plan.foreign.is_empty() {
            String::new()
        } else {
            format!(
                ", including foreign entries (--include-foreign): {}",
                names(&plan.foreign)
            )
        };
        eprintln!(
            "oxide: dry run: a clean would remove the {} paths listed ({}){foreign}; nothing was removed. A clean also drops this checkout's slot affinity; slots stay",
            plan.entries.len(),
            summary(&plan)
        );
        return Ok(0);
    }
    process::install_signal_handlers()?;
    let store = crate::removable::coordinate(Store::open_existing()?)?;
    let guard = match &store {
        Some(store) => match store.begin_clean(workspace)? {
            Cleaning::Guard(guard) => Some(guard),
            Cleaning::Interrupted(signal) => return Ok(process::signal_exit_code(signal)),
        },
        None => None,
    };
    // Plan again under the clean barrier: what is removed is what this plan
    // lists.
    let outcome = checked_plan(&target, pool, include_foreign).and_then(|plan| match plan {
        None => Ok(None),
        Some(plan) => remove(&plan).map(|()| Some(plan)),
    });
    let plan = match (outcome, &store, guard) {
        (Ok(plan), Some(store), Some(guard)) => {
            let detached = store.finish_clean(guard, workspace)?;
            if detached > 0 {
                eprintln!(
                    "oxide: dropped this checkout's slot affinity; {detached} slot(s) kept until evicted"
                );
            }
            plan
        }
        (Ok(plan), _, _) => plan,
        (Err(reason), Some(store), Some(guard)) => {
            store.abandon_clean(guard)?;
            return Err(reason);
        }
        (Err(reason), _, _) => return Err(reason),
    };
    match plan {
        None => eprintln!(
            "oxide: {} does not exist; nothing removed",
            target.display()
        ),
        Some(plan) => {
            let foreign = if plan.foreign.is_empty() {
                String::new()
            } else {
                format!(
                    ", including foreign entries (--include-foreign): {}",
                    names(&plan.foreign)
                )
            };
            eprintln!(
                "oxide: removed {} ({}){foreign}",
                target.display(),
                summary(&plan)
            );
        }
    }
    Ok(0)
}

/// `cargo clean` with a package, profile or doc selection, against the
/// private target directory and an empty private build directory, then drop
/// this checkout's slot affinity. Plain `cargo clean` would delete an
/// injected slot build directory. It waits for active commands in the
/// checkout and blocks new ones until it finishes.
fn run_selected(args: &[OsString], workspace: &Path) -> Result<u8, String> {
    protect_pool(&workspace.join("target"))?;
    process::install_signal_handlers()?;
    let store = crate::removable::coordinate(Store::open_existing()?)?;
    let guard = match &store {
        Some(store) => match store.begin_clean(workspace)? {
            Cleaning::Guard(guard) => Some(guard),
            Cleaning::Interrupted(signal) => return Ok(process::signal_exit_code(signal)),
        },
        None => None,
    };
    let scratch = private_scratch()?;
    let mut cargo = Command::new(CARGO);
    cargo.arg("clean").args(args);
    apply_storage(&mut cargo, &scratch.join("build"), workspace);
    // The clean barrier must outlive this process if Cargo does: the child
    // inherits the clean lock and runs as a recorded group whose end is
    // awaited before the barrier lifts.
    if let Some(guard) = &guard {
        guard.share_with_children()?;
    }
    let status = process::spawn_group(&mut cargo).and_then(|mut group| {
        let recorded = match (&store, &guard) {
            (Some(store), Some(guard)) => store.clean_running(guard, group.id),
            _ => Ok(()),
        };
        let status = process::wait_leader(&mut group);
        process::wait_group_empty(group.id);
        recorded.and(status)
    });
    let _ = std::fs::remove_dir(scratch.join("build"));
    let _ = std::fs::remove_dir(&scratch);
    let detached = match (&store, guard) {
        (Some(store), Some(guard)) => store.finish_clean(guard, workspace)?,
        _ => 0,
    };
    let status = status?;
    if detached > 0 {
        eprintln!(
            "oxide: dropped this checkout's slot affinity; {detached} slot(s) kept until evicted"
        );
    }
    Ok(process::exit_code(status))
}

fn private_scratch() -> Result<PathBuf, String> {
    let root = std::env::temp_dir();
    for attempt in 0..16_u32 {
        let path = root.join(format!("oxide-clean-{}-{attempt}", std::process::id()));
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot create a private clean directory: {error}")),
        }
    }
    Err("cannot allocate a private clean directory".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn whole_and_selected_cleans_are_told_apart() {
        let whole = parse(&os(&[
            "--dry-run",
            "--include-foreign",
            "--manifest-path",
            "a/Cargo.toml",
            "-v",
        ]));
        assert!(whole.dry_run && whole.include_foreign && !whole.selected);
        assert_eq!(whole.unsupported, None);
        assert_eq!(
            whole.cargo_args,
            os(&["--dry-run", "--manifest-path", "a/Cargo.toml", "-v"])
        );
        for selection in [
            &["-p", "app"][..],
            &["-papp"],
            &["--package=app"],
            &["--workspace"],
            &["--doc"],
            &["--release"],
            &["--profile", "dev"],
            &["--profile=dev"],
        ] {
            assert!(parse(&os(selection)).selected, "{selection:?}");
        }
        assert_eq!(
            parse(&os(&["--target", "x"])).unsupported.as_deref(),
            Some("--target")
        );
        // A value is never read as a flag.
        assert!(!parse(&os(&["--manifest-path", "--doc"])).selected);
    }

    /// A fresh directory under the scratch temp root.
    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("oxide-clean-{label}-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        root
    }

    fn cargo_target(target: &Path) {
        std::fs::create_dir_all(target.join("oxide-quick/deps")).unwrap();
        std::fs::write(target.join("oxide-quick/.cargo-lock"), "").unwrap();
        std::fs::write(target.join("oxide-quick/deps/app"), "bin").unwrap();
        std::fs::write(target.join("CACHEDIR.TAG"), "Signature").unwrap();
        std::fs::create_dir_all(target.join("doc/app")).unwrap();
        std::fs::create_dir_all(target.join("x86_64-unknown-linux-gnu/release")).unwrap();
        std::fs::write(
            target.join("x86_64-unknown-linux-gnu/release/.cargo-lock"),
            "",
        )
        .unwrap();
        std::fs::write(target.join("x86_64-unknown-linux-gnu/CACHEDIR.TAG"), "").unwrap();
    }

    #[test]
    fn cargo_entries_are_owned_and_others_are_foreign() {
        let root = scratch("classify");
        let target = root.join("target");
        cargo_target(&target);
        let clean = plan(&target, None).unwrap().unwrap();
        assert!(clean.foreign.is_empty(), "{:?}", clean.foreign);
        assert_eq!(clean.entries[0].path, target);
        // A plausible name without Cargo's lock is not a profile directory;
        // neither is a symlink, a stray file or a triple holding one.
        std::fs::create_dir_all(target.join("debug")).unwrap();
        std::fs::create_dir_all(target.join("handover/bin")).unwrap();
        std::fs::write(target.join("handover/bin/tool"), "keep").unwrap();
        std::fs::write(target.join("notes.txt"), "").unwrap();
        symlink(&root, target.join("link")).unwrap();
        std::fs::create_dir_all(target.join("aarch64-apple-darwin/release")).unwrap();
        std::fs::write(target.join("aarch64-apple-darwin/release/.cargo-lock"), "").unwrap();
        std::fs::write(target.join("aarch64-apple-darwin/extra"), "").unwrap();
        let mixed = plan(&target, None).unwrap().unwrap();
        assert_eq!(
            mixed.foreign,
            os(&[
                "aarch64-apple-darwin",
                "debug",
                "handover",
                "link",
                "notes.txt"
            ])
        );
        let refusal = refuse_foreign(&target, &mixed);
        assert!(
            refusal.contains("5 top-level entries") && refusal.contains("handover"),
            "{refusal}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removal_deletes_the_plan_and_never_follows_a_symlink() {
        let root = scratch("remove");
        let target = root.join("target");
        cargo_target(&target);
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), "keep").unwrap();
        std::fs::create_dir_all(target.join("handover/nested")).unwrap();
        symlink(&outside, target.join("handover/nested/escape")).unwrap();
        assert!(checked_plan(&target, None, false)
            .unwrap_err()
            .contains("handover"));
        let plan = checked_plan(&target, None, true).unwrap().unwrap();
        assert!(plan
            .entries
            .iter()
            .any(|entry| entry.path.ends_with("escape") && !entry.dir));
        remove(&plan).unwrap();
        assert!(!target.exists());
        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"keep");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn content_added_after_planning_is_left() {
        let root = scratch("late");
        let target = root.join("target");
        cargo_target(&target);
        let plan = checked_plan(&target, None, false).unwrap().unwrap();
        std::fs::write(target.join("doc/late"), "new").unwrap();
        let left = remove(&plan).unwrap_err();
        assert!(left.contains("not in the plan"), "{left}");
        assert_eq!(std::fs::read(target.join("doc/late")).unwrap(), b"new");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_pool_is_refused_by_identity_through_an_outside_symlink() {
        let root = scratch("pool");
        let target = root.join("target");
        cargo_target(&target);
        let pool = target.join("shared-pool/slots");
        std::fs::create_dir_all(&pool).unwrap();
        std::fs::write(pool.join("sentinel"), "slot").unwrap();
        symlink(target.join("shared-pool"), root.join("pool-alias")).unwrap();
        let alias = std::fs::metadata(root.join("pool-alias")).unwrap();
        let identity = Some((alias.dev(), alias.ino()));
        for include_foreign in [false, true] {
            let refused = checked_plan(&target, identity, include_foreign).unwrap_err();
            assert!(refused.contains("is the Oxide pool"), "{refused}");
        }
        let whole = std::fs::metadata(&target).unwrap();
        assert!(plan(&target, Some((whole.dev(), whole.ino())))
            .unwrap_err()
            .contains("is the Oxide pool"));
        assert_eq!(std::fs::read(pool.join("sentinel")).unwrap(), b"slot");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_symlinked_or_missing_target_is_not_cleaned_through() {
        let root = scratch("link");
        std::fs::create_dir(root.join("real")).unwrap();
        symlink(root.join("real"), root.join("target")).unwrap();
        assert!(plan(&root.join("target"), None)
            .unwrap_err()
            .contains("symlink"));
        assert!(plan(&root.join("absent"), None).unwrap().is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
