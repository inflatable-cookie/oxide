//! Conservative stale-unit pruning for Cargo build-dir layout v2 warm slots.
//!
//! Layout v2 (`<profile>/build/<pkg>/<16-hex>/{fingerprint,out,run}`) isolates
//! each Cargo compile or build-script unit in its own directory, unlike layout
//! v1's shared `deps/` and `.fingerprint/`. Over long-lived warm slots, old
//! feature flags, dependency versions and removed targets leave unused unit
//! directories behind.
//!
//! Safety rules:
//! - Runs only while the caller holds the slot's exclusive Barge root hold and
//!   only after Cargo's process group has positively closed on a clean build.
//! - Never follows symlinks (`O_NOFOLLOW`, `AT_SYMLINK_NOFOLLOW`), never crosses
//!   mount boundaries (`dev` check), and never touches foreign-owned entries.
//! - Leaves layout v1 slots (any member holding `deps/` or `.fingerprint/`, or
//!   lacking v2 units) and unknown/unrecognized entries completely untouched.
//! - Verifies an entire unit subtree before unlinking anything from it, so an
//!   unexpected symlink or mount boundary inside `out/` leaves the unit whole.
//! - Uses `atime` observation armed without modifying `mtime` (`UTIME_OMIT`),
//!   verified at runtime with an instant `futimens`+`pread` probe on the slot's
//!   own filesystem. Filesystems that disable `atime` (`noatime` or a failed
//!   probe) skip pruning completely.
//! - Bounds both elapsed time and walked/deleted entry counts so slot release
//!   never stalls.

use crate::process;
use crate::store::Lease;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Units unused for at least seven days become eligible for pruning.
pub(crate) const STALE_UNIT_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Pre-Cargo arming deadline; keeps slot preparation bounded.
const PRE_ARM_MAX_ELAPSED: Duration = Duration::from_millis(200);
/// Post-Cargo pruning deadline; keeps slot release bounded.
const PRUNE_MAX_ELAPSED: Duration = Duration::from_millis(500);
/// Maximum profile member directories inspected per pass.
const PRUNE_MAX_PROFILES: usize = 16;
/// Maximum package directories inspected per profile member.
const PRUNE_MAX_PACKAGES: usize = 4_096;
/// Maximum unit directories inspected per slot pass.
const PRUNE_MAX_UNITS: usize = 16_384;
/// Maximum filesystem entries walked (inspection + verification) per pass.
const PRUNE_MAX_ENTRIES: usize = 50_000;
/// Maximum directory nesting depth inside a unit's `{fingerprint,out,run}` tree.
const PRUNE_MAX_DEPTH: usize = 32;
/// Maximum stale unit directories deleted in one closeout pass.
const PRUNE_MAX_DELETIONS: usize = 256;
/// Maximum direct entries in a recognized `<meta>` directory or `fingerprint/`.
const MAX_UNIT_META_ENTRIES: usize = 16;
const MAX_FINGERPRINT_ENTRIES: usize = 64;

const UNITS_DIR: &str = "units";
const UNIT_RECORD_VERSION: u32 = 1;
const MAX_UNIT_RECORD_BYTES: u64 = 4 * 1024 * 1024;
const MAX_UNIT_RECORDS_PER_SLOT: usize = 16_384;
const UNIT_TEMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);
const UNIT_ORPHAN_CLEANUP_MAX: usize = 8;

/// Pre-Cargo state carried through a leased build so closeout knows when the
/// build started and which layout-v2 units had `atime < mtime` armed.
pub(crate) struct Session {
    pub(crate) build_started: SystemTime,
    pub(crate) armed_units: BTreeSet<String>,
}

/// Summary of one post-build pruning pass.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PruneReport {
    pub(crate) pruned_units: usize,
    pub(crate) reclaimed_bytes: u64,
    pub(crate) stopped_by_bound: bool,
}

/// Explicit bounds for a pruning pass, parameterized for unit and integration
/// tests while production callers use [`Limits::production`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) max_age: Duration,
    pub(crate) max_elapsed: Duration,
    pub(crate) max_profiles: usize,
    pub(crate) max_packages: usize,
    pub(crate) max_units: usize,
    pub(crate) max_entries: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_deletions: usize,
}

impl Limits {
    fn production(max_elapsed: Duration) -> Self {
        Self {
            max_age: STALE_UNIT_MAX_AGE,
            max_elapsed,
            max_profiles: PRUNE_MAX_PROFILES,
            max_packages: PRUNE_MAX_PACKAGES,
            max_units: PRUNE_MAX_UNITS,
            max_entries: PRUNE_MAX_ENTRIES,
            max_depth: PRUNE_MAX_DEPTH,
            max_deletions: PRUNE_MAX_DELETIONS,
        }
    }
}

/// Prepare stale-unit observation before Cargo starts on a warm layout-v2 slot.
///
/// Verifies that the slot's filesystem updates `atime` on reads when
/// `atime < mtime`, records the pre-build timestamp, and ensures each existing
/// layout-v2 unit's primary fingerprint file has `atime < mtime` without
/// altering `mtime`. Returns `None` when the slot is fresh, unheld, on a
/// `noatime` filesystem, or lacks an active admission window.
pub(crate) fn prepare(lease: &Lease) -> Option<Session> {
    if !lease.reused || !lease.has_exclusive_hold() {
        return None;
    }
    let max_elapsed = crate::budget::clamp(PRE_ARM_MAX_ELAPSED);
    if max_elapsed.is_zero() {
        return None;
    }
    prepare_with(&lease.build_dir, Limits::production(max_elapsed))
}

/// Inner pre-Cargo arming entry point, shared with regression tests.
pub(crate) fn prepare_with(build_dir: &Path, limits: Limits) -> Option<Session> {
    let slot_dir = build_dir.parent()?;
    if !supports_atime(slot_dir) {
        return None;
    }
    let build_started = SystemTime::now();
    let mut armed_units = BTreeSet::new();
    let euid = effective_uid();
    let Some((build_fd, build_dev)) = open_verified_dir(build_dir, euid) else {
        return Some(Session {
            build_started,
            armed_units,
        });
    };
    let mut budget = WalkBudget::new(limits);
    for (member_rel, member_build_fd) in
        profile_v2_build_dirs(build_dir, &build_fd, build_dev, euid, &mut budget)
    {
        if budget.exhausted() {
            break;
        }
        let Ok(packages) = member_build_fd.names(limits.max_packages) else {
            continue;
        };
        for pkg_cstr in packages {
            if budget.exhausted() {
                break;
            }
            let Ok(pkg_str) = pkg_cstr.to_str() else {
                continue;
            };
            if !is_valid_package_name(pkg_str) {
                continue;
            }
            let Some(pkg_fd) =
                open_child_verified_dir(&member_build_fd, &pkg_cstr, build_dev, euid)
            else {
                continue;
            };
            let Ok(units) = pkg_fd.names(limits.max_units) else {
                continue;
            };
            for meta_cstr in units {
                if !budget.take_unit() || budget.exhausted() {
                    break;
                }
                let Ok(meta_str) = meta_cstr.to_str() else {
                    continue;
                };
                if !is_unit_hash(meta_str) {
                    continue;
                }
                let Some(inspected) = inspect_v2_unit(
                    &pkg_fd,
                    &meta_cstr,
                    build_dev,
                    euid,
                    &mut budget,
                    true,
                    SystemTime::UNIX_EPOCH,
                ) else {
                    continue;
                };
                if inspected.all_primary_armed {
                    armed_units.insert(format!("{member_rel}/{pkg_str}/{meta_str}"));
                }
            }
        }
    }
    Some(Session {
        build_started,
        armed_units,
    })
}

/// Prune stale layout-v2 units after Cargo's writer group has closed cleanly,
/// while the caller still holds the slot's exclusive Barge root hold.
///
/// On a cold or seeded slot (`!lease.reused`), nothing is deleted; observed units
/// are recorded with `last_active_ms = now` and armed for later warm builds.
pub(crate) fn finish(
    lease: &Lease,
    control_dir: &Path,
    session: Option<Session>,
    build_succeeded: bool,
) -> PruneReport {
    if !lease.has_exclusive_hold()
        || !build_succeeded
        || process::received_signal().is_some()
        || crate::budget::origin().is_some()
    {
        return PruneReport::default();
    }
    let max_elapsed = match crate::budget::execution_remaining() {
        Some(left) => PRUNE_MAX_ELAPSED.min(left),
        None => PRUNE_MAX_ELAPSED,
    };
    if max_elapsed.is_zero() {
        return PruneReport {
            stopped_by_bound: true,
            ..PruneReport::default()
        };
    }
    finish_with(lease, control_dir, session, Limits::production(max_elapsed))
}

/// Inner post-build pruning entry point, shared with regression tests.
pub(crate) fn finish_with(
    lease: &Lease,
    control_dir: &Path,
    session: Option<Session>,
    limits: Limits,
) -> PruneReport {
    if !lease.has_exclusive_hold() {
        return PruneReport::default();
    }
    // Warm builds require a pre-Cargo session proving `atime` works on this
    // mount. Cold builds never delete anything and only record/arm units when
    // `atime` is supported on the slot's filesystem.
    let (build_started, armed_units, allow_delete) = match (lease.reused, session) {
        (true, Some(session)) => (session.build_started, session.armed_units, true),
        (true, None) => return PruneReport::default(),
        (false, _) => {
            let Some(slot_dir) = lease.build_dir.parent() else {
                return PruneReport::default();
            };
            if !supports_atime(slot_dir) {
                return PruneReport::default();
            }
            (SystemTime::now(), BTreeSet::new(), false)
        }
    };
    let euid = effective_uid();
    let Some((build_fd, build_dev)) = open_verified_dir(&lease.build_dir, euid) else {
        return PruneReport::default();
    };

    let now = SystemTime::now();
    let now_ms = system_time_ms(now);
    let touch_cutoff = build_started
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(build_started);
    let max_age_ms = u64::try_from(limits.max_age.as_millis()).unwrap_or(u64::MAX);

    let units_dir = control_dir.join(UNITS_DIR);
    let prior = read_activity_file(&units_dir, lease, euid);
    let mut updated: BTreeMap<String, UnitActivityEntry> = prior
        .as_ref()
        .map(|doc| doc.units.clone())
        .unwrap_or_default();

    let mut budget = WalkBudget::new(limits);
    let mut report = PruneReport::default();
    let mut observed_keys = BTreeSet::new();

    for (member_rel, member_build_fd) in
        profile_v2_build_dirs(&lease.build_dir, &build_fd, build_dev, euid, &mut budget)
    {
        if budget.exhausted() {
            report.stopped_by_bound = true;
            break;
        }
        let Ok(packages) = member_build_fd.names(limits.max_packages) else {
            continue;
        };
        for pkg_cstr in packages {
            if budget.exhausted() {
                report.stopped_by_bound = true;
                break;
            }
            let Ok(pkg_str) = pkg_cstr.to_str() else {
                continue;
            };
            if !is_valid_package_name(pkg_str) {
                continue;
            }
            let Some(pkg_fd) =
                open_child_verified_dir(&member_build_fd, &pkg_cstr, build_dev, euid)
            else {
                continue;
            };
            let Ok(units) = pkg_fd.names(limits.max_units) else {
                continue;
            };
            let mut pruned_in_pkg = false;
            for meta_cstr in units {
                if budget.exhausted() {
                    report.stopped_by_bound = true;
                    break;
                }
                if !budget.take_unit() {
                    report.stopped_by_bound = true;
                    break;
                }
                let Ok(meta_str) = meta_cstr.to_str() else {
                    continue;
                };
                if !is_unit_hash(meta_str) {
                    continue;
                }
                let unit_key = format!("{member_rel}/{pkg_str}/{meta_str}");
                let Some(inspected) = inspect_v2_unit(
                    &pkg_fd,
                    &meta_cstr,
                    build_dev,
                    euid,
                    &mut budget,
                    false,
                    touch_cutoff,
                ) else {
                    continue;
                };

                let observed_mtime_ms = system_time_ms(inspected.max_mtime);
                let observed_atime_ms = system_time_ms(inspected.max_atime);
                let disk_latest_ms = observed_mtime_ms.max(observed_atime_ms);

                if !allow_delete || inspected.touched_since_cutoff {
                    let active_ms = if allow_delete || !lease.reused {
                        now_ms
                    } else {
                        disk_latest_ms
                    };
                    observed_keys.insert(unit_key.clone());
                    updated.insert(
                        unit_key,
                        UnitActivityEntry {
                            last_active_ms: active_ms,
                            observed_mtime_ms,
                        },
                    );
                    arm_unit_fingerprints(&inspected.fp_fd, &inspected.arm_targets);
                    continue;
                }

                let prev = prior.as_ref().and_then(|doc| doc.units.get(&unit_key));
                let effective_last_active_ms = match prev {
                    Some(entry) if observed_mtime_ms >= entry.observed_mtime_ms => {
                        entry.last_active_ms.max(disk_latest_ms)
                    }
                    _ => disk_latest_ms,
                };
                let age_ms = now_ms.saturating_sub(effective_last_active_ms);
                let was_armed = armed_units.contains(&unit_key);

                if was_armed && age_ms >= max_age_ms && report.pruned_units < limits.max_deletions {
                    match delete_verified_unit(
                        &pkg_fd,
                        &meta_cstr,
                        &inspected.meta_fd,
                        &inspected.subdirs,
                        build_dev,
                        euid,
                        &mut budget,
                    ) {
                        Ok(Some(bytes)) => {
                            report.pruned_units += 1;
                            report.reclaimed_bytes = report.reclaimed_bytes.saturating_add(bytes);
                            updated.remove(&unit_key);
                            pruned_in_pkg = true;
                            continue;
                        }
                        Ok(None) => {
                            if budget.exhausted() {
                                report.stopped_by_bound = true;
                            }
                        }
                        Err(()) => {}
                    }
                } else if was_armed && age_ms >= max_age_ms {
                    report.stopped_by_bound = true;
                }

                observed_keys.insert(unit_key.clone());
                updated.insert(
                    unit_key,
                    UnitActivityEntry {
                        last_active_ms: effective_last_active_ms,
                        observed_mtime_ms,
                    },
                );
                arm_unit_fingerprints(&inspected.fp_fd, &inspected.arm_targets);
            }
            if pruned_in_pkg {
                let _ = member_build_fd.unlink(&pkg_cstr, true);
            }
        }
    }

    if budget.exhausted() {
        report.stopped_by_bound = true;
    }

    if !report.stopped_by_bound {
        updated.retain(|key, _| observed_keys.contains(key));
    }

    if !updated.is_empty() || prior.is_some() {
        while updated.len() > MAX_UNIT_RECORDS_PER_SLOT {
            let Some(oldest_key) = updated
                .iter()
                .min_by_key(|(_, entry)| entry.last_active_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            updated.remove(&oldest_key);
        }
        let doc = UnitActivityFile {
            version: UNIT_RECORD_VERSION,
            store_id: lease.store_id.clone(),
            slot_id: lease.slot_id.clone(),
            incarnation: lease.incarnation.clone(),
            updated_ms: now_ms,
            units: updated,
        };
        let _ = write_activity_file(&units_dir, lease, &doc);
    }

    report
}

struct WalkBudget {
    started: Instant,
    max_elapsed: Duration,
    max_profiles: usize,
    max_entries: usize,
    max_units: usize,
    max_depth: usize,
    entries: usize,
    units: usize,
    exhausted: bool,
}

impl WalkBudget {
    fn new(limits: Limits) -> Self {
        Self {
            started: Instant::now(),
            max_elapsed: limits.max_elapsed,
            max_profiles: limits.max_profiles,
            max_entries: limits.max_entries,
            max_units: limits.max_units,
            max_depth: limits.max_depth,
            entries: 0,
            units: 0,
            exhausted: false,
        }
    }

    fn exhausted(&mut self) -> bool {
        if !self.exhausted && self.started.elapsed() >= self.max_elapsed {
            self.exhausted = true;
        }
        self.exhausted
    }

    fn take_entry(&mut self) -> bool {
        if self.exhausted() || self.entries >= self.max_entries {
            self.exhausted = true;
            return false;
        }
        self.entries += 1;
        true
    }

    fn take_unit(&mut self) -> bool {
        if self.exhausted() || self.units >= self.max_units {
            self.exhausted = true;
            return false;
        }
        self.units += 1;
        true
    }
}

/// Discover profile members under `build_dir` that hold layout v2 units and no
/// layout v1 directories (`deps/`, `.fingerprint/`).
fn profile_v2_build_dirs(
    build_dir: &Path,
    build_fd: &SafeDir,
    build_dev: u64,
    euid: u32,
    budget: &mut WalkBudget,
) -> Vec<(String, SafeDir)> {
    let mut found = Vec::new();
    for member in crate::cargo_layout::profile_members(build_dir)
        .into_iter()
        .take(budget.max_profiles)
    {
        if budget.exhausted() {
            break;
        }
        let Ok(rel) = member.strip_prefix(build_dir) else {
            continue;
        };
        let Some(rel_str) = rel.to_str() else {
            continue;
        };
        let Some(member_fd) = open_relative_verified_dir(build_fd, rel, build_dev, euid) else {
            continue;
        };
        if has_layout_v1_dirs(&member_fd) {
            continue;
        }
        if !crate::cargo_layout::holds_v2_build(&member) {
            continue;
        }
        let Some(member_build_fd) = open_child_verified_dir(&member_fd, c"build", build_dev, euid)
        else {
            continue;
        };
        found.push((rel_str.to_owned(), member_build_fd));
    }
    found
}

/// Whether a profile member directory holds any `deps` or `.fingerprint` entry
/// (including symlinks). Presence of either marks a layout v1 or mixed member
/// that must remain completely untouched.
fn has_layout_v1_dirs(member_fd: &SafeDir) -> bool {
    for name in [c"deps", c".fingerprint"] {
        match member_fd.entry_stat(name) {
            Ok(_) => return true,
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
            Err(_) => return true,
        }
    }
    false
}

struct UnitInspect {
    meta_fd: SafeDir,
    fp_fd: SafeDir,
    subdirs: Vec<CString>,
    arm_targets: Vec<(CString, SystemTime)>,
    all_primary_armed: bool,
    max_mtime: SystemTime,
    max_atime: SystemTime,
    touched_since_cutoff: bool,
}

/// Validate a `<meta>` directory as a recognized Cargo layout v2 unit and read
/// its `fingerprint/` timestamps. Optionally arms primary fingerprint files
/// (`atime = mtime - 1s`, `mtime` untouched) when `arm_now` is true.
fn inspect_v2_unit(
    pkg_fd: &SafeDir,
    meta_name: &CStr,
    build_dev: u64,
    euid: u32,
    budget: &mut WalkBudget,
    arm_now: bool,
    touch_cutoff: SystemTime,
) -> Option<UnitInspect> {
    if !budget.take_entry() {
        return None;
    }
    let meta_stat = pkg_fd.entry_stat(meta_name).ok()?;
    if !is_owned_dir(&meta_stat, build_dev, euid) {
        return None;
    }
    let meta_fd = pkg_fd.open_dir(meta_name).ok()?;
    let opened_stat = meta_fd.stat().ok()?;
    if opened_stat.st_dev != meta_stat.st_dev || opened_stat.st_ino != meta_stat.st_ino {
        return None;
    }

    let entries = meta_fd.names(MAX_UNIT_META_ENTRIES + 1).ok()?;
    if entries.len() > MAX_UNIT_META_ENTRIES {
        return None;
    }
    let mut has_fingerprint = false;
    let mut subdirs = Vec::with_capacity(3);
    let mut touched_since_cutoff = false;
    let mut max_mtime = SystemTime::UNIX_EPOCH;
    let mut max_atime = SystemTime::UNIX_EPOCH;

    for entry in entries {
        if !budget.take_entry() {
            return None;
        }
        let Ok(name) = entry.to_str() else {
            return None;
        };
        if !matches!(name, "fingerprint" | "out" | "run") {
            return None;
        }
        let st = meta_fd.entry_stat(&entry).ok()?;
        if !is_owned_dir(&st, build_dev, euid) {
            return None;
        }
        if let Some(mtime) = stat_mtime(&st) {
            if mtime > max_mtime {
                max_mtime = mtime;
            }
            if mtime >= touch_cutoff {
                touched_since_cutoff = true;
            }
        }
        if name == "fingerprint" {
            has_fingerprint = true;
        }
        subdirs.push(entry);
    }
    if !has_fingerprint {
        return None;
    }

    let fp_fd = open_child_verified_dir(&meta_fd, c"fingerprint", build_dev, euid)?;
    let fp_entries = fp_fd.names(MAX_FINGERPRINT_ENTRIES + 1).ok()?;
    if fp_entries.len() > MAX_FINGERPRINT_ENTRIES {
        return None;
    }

    let mut primary_targets = Vec::new();
    let mut fallback_targets = Vec::new();
    let mut all_primary_armed = true;

    for entry in fp_entries {
        if !budget.take_entry() {
            return None;
        }
        let st = fp_fd.entry_stat(&entry).ok()?;
        if !is_owned_regular(&st, build_dev, euid) {
            return None;
        }
        let mtime = stat_mtime(&st)?;
        let atime = stat_atime(&st)?;
        if mtime > max_mtime {
            max_mtime = mtime;
        }
        if atime > max_atime {
            max_atime = atime;
        }
        if mtime >= touch_cutoff || atime >= touch_cutoff {
            touched_since_cutoff = true;
        }
        let Ok(name) = entry.to_str() else {
            return None;
        };
        if is_primary_fingerprint_file(name) {
            primary_targets.push((entry, mtime, atime));
        } else if is_secondary_fingerprint_file(name) {
            fallback_targets.push((entry, mtime, atime));
        }
    }

    let targets = if primary_targets.is_empty() {
        fallback_targets
    } else {
        primary_targets
    };
    if targets.is_empty() {
        all_primary_armed = false;
    }
    let mut arm_targets = Vec::with_capacity(targets.len());
    for (name, mtime, atime) in targets {
        if arm_now {
            if atime >= mtime && !arm_one_file(&fp_fd, &name, mtime) {
                all_primary_armed = false;
            }
        } else {
            arm_targets.push((name, mtime));
        }
    }

    Some(UnitInspect {
        meta_fd,
        fp_fd,
        subdirs,
        arm_targets,
        all_primary_armed,
        max_mtime,
        max_atime,
        touched_since_cutoff,
    })
}

fn is_primary_fingerprint_file(name: &str) -> bool {
    !name.ends_with(".json")
        && name != "invoked.timestamp"
        && !name.starts_with("dep-")
        && !name.starts_with("output-")
}

fn is_secondary_fingerprint_file(name: &str) -> bool {
    !name.ends_with(".json") && name != "invoked.timestamp"
}

fn arm_unit_fingerprints(fp_fd: &SafeDir, targets: &[(CString, SystemTime)]) {
    for (name, mtime) in targets {
        let _ = arm_one_file(fp_fd, name, *mtime);
    }
}

fn arm_one_file(fp_fd: &SafeDir, name: &CStr, mtime: SystemTime) -> bool {
    let Some(target_atime) = mtime.checked_sub(Duration::from_secs(1)) else {
        return false;
    };
    if target_atime < UNIX_EPOCH {
        return false;
    }
    fp_fd.set_atime(name, target_atime).is_ok()
}

/// Two-phase verified deletion of one `<meta>` unit directory.
///
/// Phase 1 walks every entry inside `{fingerprint,out,run}` descriptor-relative
/// without following symlinks, verifying ownership, device, entry type, depth
/// and budget. Only if 100% of the subtree passes verification does Phase 2
/// unlink files and remove directories bottom-up.
fn delete_verified_unit(
    pkg_fd: &SafeDir,
    meta_name: &CStr,
    meta_fd: &SafeDir,
    subdirs: &[CString],
    build_dev: u64,
    euid: u32,
    budget: &mut WalkBudget,
) -> Result<Option<u64>, ()> {
    struct VerifiedDir {
        parent_idx: Option<usize>,
        name: CString,
        fd: SafeDir,
        files: Vec<CString>,
        child_dirs: Vec<CString>,
    }

    let mut verified_dirs: Vec<VerifiedDir> = Vec::new();
    let mut bytes = 0u64;
    let max_depth = budget.max_depth;

    // Stack of (parent_verified_idx, parent_fd_cloned_or_borrowed_via_idx, name, depth)
    let mut stack: Vec<(Option<usize>, CString, usize)> =
        subdirs.iter().map(|name| (None, name.clone(), 1)).collect();

    while let Some((parent_idx, name, depth)) = stack.pop() {
        if depth > max_depth || !budget.take_entry() {
            return Ok(None);
        }
        let parent_fd = match parent_idx {
            Some(idx) => &verified_dirs[idx].fd,
            None => meta_fd,
        };
        let Ok(dir_stat) = parent_fd.entry_stat(&name) else {
            return Ok(None);
        };
        if !is_owned_dir(&dir_stat, build_dev, euid) {
            return Ok(None);
        }
        let Ok(dir_fd) = parent_fd.open_dir(&name) else {
            return Ok(None);
        };
        let Ok(opened_stat) = dir_fd.stat() else {
            return Ok(None);
        };
        if opened_stat.st_dev != dir_stat.st_dev || opened_stat.st_ino != dir_stat.st_ino {
            return Ok(None);
        }
        let Ok(children) = dir_fd.names(PRUNE_MAX_ENTRIES) else {
            return Ok(None);
        };
        let current_idx = verified_dirs.len();
        let mut files = Vec::new();
        let mut child_dirs = Vec::new();
        for child in children {
            if !budget.take_entry() {
                return Ok(None);
            }
            let Ok(child_stat) = dir_fd.entry_stat(&child) else {
                return Ok(None);
            };
            if is_owned_regular(&child_stat, build_dev, euid) {
                bytes = bytes.saturating_add(u64::try_from(child_stat.st_size).unwrap_or(0));
                files.push(child);
            } else if is_owned_dir(&child_stat, build_dev, euid) {
                child_dirs.push(child.clone());
                stack.push((Some(current_idx), child, depth + 1));
            } else {
                // Symlink, socket, FIFO, foreign owner or cross-mount entry:
                // abort deletion and leave the entire unit untouched.
                return Ok(None);
            }
        }
        verified_dirs.push(VerifiedDir {
            parent_idx,
            name,
            fd: dir_fd,
            files,
            child_dirs,
        });
    }

    // Phase 2: all entries verified; unlink bottom-up (reverse push order removes
    // deeper directories before their parents).
    for dir in &verified_dirs {
        for file in &dir.files {
            if dir.fd.unlink(file, false).is_err() {
                return Err(());
            }
        }
    }
    for idx in (0..verified_dirs.len()).rev() {
        let dir = &verified_dirs[idx];
        for child in &dir.child_dirs {
            if dir.fd.unlink(child, true).is_err() {
                return Err(());
            }
        }
        if dir.parent_idx.is_none() && meta_fd.unlink(&dir.name, true).is_err() {
            return Err(());
        }
    }
    pkg_fd.unlink(meta_name, true).map_err(|_| ())?;
    Ok(Some(bytes))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitActivityFile {
    version: u32,
    store_id: String,
    slot_id: String,
    incarnation: String,
    updated_ms: u64,
    units: BTreeMap<String, UnitActivityEntry>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitActivityEntry {
    last_active_ms: u64,
    observed_mtime_ms: u64,
}

fn read_activity_file(units_dir: &Path, lease: &Lease, euid: u32) -> Option<UnitActivityFile> {
    let dir_meta = fs::symlink_metadata(units_dir).ok()?;
    if !dir_meta.is_dir() || dir_meta.file_type().is_symlink() || dir_meta.uid() != euid {
        return None;
    }
    let path = units_dir.join(format!("{}.json", lease.slot_id));
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file()
        || meta.uid() != euid
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_UNIT_RECORD_BYTES
    {
        return None;
    }
    let mut bytes = Vec::new();
    use std::io::Read as _;
    file.take(MAX_UNIT_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_UNIT_RECORD_BYTES {
        return None;
    }
    let parsed: UnitActivityFile = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != UNIT_RECORD_VERSION
        || parsed.store_id != lease.store_id
        || parsed.slot_id != lease.slot_id
        || parsed.incarnation != lease.incarnation
        || parsed.units.len() > MAX_UNIT_RECORDS_PER_SLOT
    {
        return None;
    }
    Some(parsed)
}

fn write_activity_file(units_dir: &Path, lease: &Lease, doc: &UnitActivityFile) -> io::Result<()> {
    let bytes = serde_json::to_vec(doc).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_UNIT_RECORD_BYTES {
        return Err(io::Error::other("unit activity sidecar exceeds byte cap"));
    }
    match fs::DirBuilder::new().mode(0o700).create(units_dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let euid = effective_uid();
    let dir_fd = SafeDir::open(units_dir)?;
    let dir_stat = dir_fd.stat()?;
    if !is_owned_dir(&dir_stat, dir_stat.st_dev as u64, euid) || dir_stat.st_mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "units sidecar directory is not owner-only",
        ));
    }
    reap_old_temps(&dir_fd);
    // Created exclusively under a name unique to this process and call, so a
    // temp can never be opened over or followed; `reap_old_temps` still matches
    // the `.json.tmp-` infix.
    static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = units_dir.join(format!(
        "{}.json.tmp-{}-{sequence}",
        lease.slot_id,
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temp)?;
    let write_res = file.write_all(&bytes).and_then(|()| file.sync_all());
    if let Err(error) = write_res {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let dest = units_dir.join(format!("{}.json", lease.slot_id));
    if let Err(error) = fs::rename(&temp, &dest) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}

fn reap_old_temps(dir_fd: &SafeDir) {
    let Ok(names) = dir_fd.names(64) else {
        return;
    };
    let now = SystemTime::now();
    let mut reaped = 0usize;
    for name in names {
        if reaped >= UNIT_ORPHAN_CLEANUP_MAX {
            break;
        }
        let Ok(text) = name.to_str() else {
            continue;
        };
        if !text.contains(".json.tmp-") {
            continue;
        }
        let Ok(st) = dir_fd.entry_stat(&name) else {
            continue;
        };
        if let Some(mtime) = stat_mtime(&st) {
            if now.duration_since(mtime).unwrap_or(Duration::ZERO) >= UNIT_TEMP_MAX_AGE
                && dir_fd.unlink(&name, false).is_ok()
            {
                reaped += 1;
            }
        }
    }
}

/// Runtime kernel proof that reading a file with `atime < mtime` on `slot_dir`'s
/// filesystem updates `atime` and that `NOATIME` is not set on the mount.
pub(crate) fn supports_atime(slot_dir: &Path) -> bool {
    if mount_has_noatime(slot_dir) {
        return false;
    }
    let Ok(dir_fd) = SafeDir::open(slot_dir) else {
        return false;
    };
    let Ok(name) = CString::new(format!(".oxide-atime-probe-{}", std::process::id())) else {
        return false;
    };
    // SAFETY: `dir_fd` and `name` are live for the call.
    let raw = unsafe {
        libc::openat(
            dir_fd.0.as_raw_fd(),
            name.as_ptr(),
            libc::O_CREAT | libc::O_EXCL | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if raw < 0 {
        return false;
    }
    // SAFETY: `raw` is a newly created descriptor we exclusively own.
    let file = unsafe { File::from_raw_fd(raw) };
    struct UnlinkGuard<'a> {
        dir: &'a SafeDir,
        name: &'a CStr,
    }
    impl Drop for UnlinkGuard<'_> {
        fn drop(&mut self) {
            let _ = self.dir.unlink(self.name, false);
        }
    }
    let _guard = UnlinkGuard {
        dir: &dir_fd,
        name: &name,
    };

    let mut file_ref = &file;
    if file_ref.write_all(b"x").is_err() {
        return false;
    }
    let now = SystemTime::now();
    let Some(past_atime) = now.checked_sub(Duration::from_secs(10)) else {
        return false;
    };
    let Some(past_mtime) = now.checked_sub(Duration::from_secs(5)) else {
        return false;
    };
    let (Some(atime_ts), Some(mtime_ts)) = (to_timespec(past_atime), to_timespec(past_mtime))
    else {
        return false;
    };
    let times = [atime_ts, mtime_ts];
    // SAFETY: `file` is open and `times` holds two valid `timespec` structs.
    if unsafe { libc::futimens(file.as_raw_fd(), times.as_ptr()) } != 0 {
        return false;
    }
    let mut byte = [0u8; 1];
    // SAFETY: `file` is open and `byte` is 1 writable byte.
    let read_n = unsafe { libc::pread(file.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) };
    if read_n != 1 {
        return false;
    }
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: `file` is open and `st` is valid for writes.
    if unsafe { libc::fstat(file.as_raw_fd(), &mut st) } != 0 {
        return false;
    }
    let Some(observed_atime) = stat_atime(&st) else {
        return false;
    };
    let threshold = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
    observed_atime >= threshold
}

#[cfg(target_os = "macos")]
fn mount_has_noatime(path: &Path) -> bool {
    crate::store::statfs(path)
        .is_some_and(|sample| (sample.f_flags as u32) & (libc::MNT_NOATIME as u32) != 0)
}

#[cfg(target_os = "linux")]
fn mount_has_noatime(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `c_path` is NUL-terminated and `stat` is valid for writes.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return true;
    }
    // SAFETY: `statvfs` succeeded and initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    (stat.f_flag & libc::ST_NOATIME) != 0
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn mount_has_noatime(_path: &Path) -> bool {
    true
}

fn open_verified_dir(path: &Path, euid: u32) -> Option<(SafeDir, u64)> {
    let fd = SafeDir::open(path).ok()?;
    let st = fd.stat().ok()?;
    if !is_dir_mode(&st) || st.st_uid != euid {
        return None;
    }
    Some((fd, st.st_dev as u64))
}

fn open_child_verified_dir(
    parent: &SafeDir,
    name: &CStr,
    expected_dev: u64,
    euid: u32,
) -> Option<SafeDir> {
    let st = parent.entry_stat(name).ok()?;
    if !is_owned_dir(&st, expected_dev, euid) {
        return None;
    }
    let child = parent.open_dir(name).ok()?;
    let opened = child.stat().ok()?;
    if opened.st_dev != st.st_dev || opened.st_ino != st.st_ino {
        return None;
    }
    Some(child)
}

fn open_relative_verified_dir(
    root: &SafeDir,
    relative: &Path,
    expected_dev: u64,
    euid: u32,
) -> Option<SafeDir> {
    let mut current: Option<SafeDir> = None;
    for component in relative.components() {
        let std::path::Component::Normal(part) = component else {
            return None;
        };
        let cstr = CString::new(part.as_bytes()).ok()?;
        let parent = current.as_ref().unwrap_or(root);
        let next = open_child_verified_dir(parent, &cstr, expected_dev, euid)?;
        current = Some(next);
    }
    current
}

fn is_valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn is_unit_hash(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_dir_mode(st: &libc::stat) -> bool {
    (st.st_mode & libc::S_IFMT) == libc::S_IFDIR
}

fn is_regular_mode(st: &libc::stat) -> bool {
    (st.st_mode & libc::S_IFMT) == libc::S_IFREG
}

fn is_owned_dir(st: &libc::stat, expected_dev: u64, euid: u32) -> bool {
    is_dir_mode(st) && st.st_dev as u64 == expected_dev && st.st_uid == euid
}

fn is_owned_regular(st: &libc::stat, expected_dev: u64, euid: u32) -> bool {
    is_regular_mode(st) && st.st_dev as u64 == expected_dev && st.st_uid == euid
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn system_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    })
}

fn stat_mtime(st: &libc::stat) -> Option<SystemTime> {
    from_sec_nsec(st.st_mtime, st.st_mtime_nsec)
}

fn stat_atime(st: &libc::stat) -> Option<SystemTime> {
    from_sec_nsec(st.st_atime, st.st_atime_nsec)
}

fn from_sec_nsec(sec: libc::time_t, nsec: libc::c_long) -> Option<SystemTime> {
    let secs = u64::try_from(sec).ok()?;
    let nanos = u32::try_from(nsec).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(secs, nanos))
}

fn to_timespec(time: SystemTime) -> Option<libc::timespec> {
    let elapsed = time.duration_since(UNIX_EPOCH).ok()?;
    let tv_sec = libc::time_t::try_from(elapsed.as_secs()).ok()?;
    let tv_nsec = libc::c_long::try_from(elapsed.subsec_nanos()).ok()?;
    Some(libc::timespec { tv_sec, tv_nsec })
}

struct SafeDir(OwnedFd);

impl SafeDir {
    fn open(path: &Path) -> io::Result<Self> {
        let name = c_string(path.as_os_str())?;
        // SAFETY: `name` is a NUL-terminated string live for the call.
        let fd = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a newly opened descriptor we exclusively own.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn open_dir(&self, name: &CStr) -> io::Result<Self> {
        // SAFETY: `self.0` and `name` are live for the call.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a newly opened descriptor we exclusively own.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    fn stat(&self) -> io::Result<libc::stat> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `self.0` is an open descriptor and `stat` is valid for writes.
        if unsafe { libc::fstat(self.0.as_raw_fd(), &mut stat) } == 0 {
            Ok(stat)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn entry_stat(&self, name: &CStr) -> io::Result<libc::stat> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `self.0` and `name` are live for the call and `stat` is valid for writes.
        if unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            Ok(stat)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn unlink(&self, name: &CStr, is_dir: bool) -> io::Result<()> {
        let flags = if is_dir { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: `self.0` and `name` are live for the call.
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), flags) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn set_atime(&self, name: &CStr, atime: SystemTime) -> io::Result<()> {
        let Some(atime_ts) = to_timespec(atime) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid access timestamp",
            ));
        };
        let times = [
            atime_ts,
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
        ];
        // SAFETY: `self.0` and `name` are live and `times` holds two valid entries.
        if unsafe {
            libc::utimensat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn names(&self, limit: usize) -> io::Result<Vec<CString>> {
        // SAFETY: `self.0` is an open directory descriptor.
        let duplicate = unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `duplicate` is a newly duplicated directory descriptor.
        let raw = unsafe { libc::fdopendir(duplicate) };
        if raw.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `fdopendir` failed, so `duplicate` is still ours to close.
            unsafe { libc::close(duplicate) };
            return Err(error);
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                // SAFETY: `self.0` is a live `DIR*` stream owned by this guard.
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(raw);
        let mut names = Vec::new();
        loop {
            clear_errno();
            // SAFETY: `stream.0` is live until `Stream` drops.
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                let errno = current_errno();
                if errno != 0 {
                    return Err(io::Error::from_raw_os_error(errno));
                }
                break;
            }
            // SAFETY: `readdir` returned a live entry with NUL-terminated `d_name`.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            let bytes = name.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            names.push(name.to_owned());
            if names.len() >= limit {
                break;
            }
        }
        Ok(names)
    }
}

fn c_string(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

fn clear_errno() {
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` returns this thread's errno pointer.
    unsafe {
        *libc::__error() = 0;
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: `__errno_location` returns this thread's errno pointer.
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn current_errno() -> i32 {
    #[cfg(target_os = "macos")]
    // SAFETY: `__error` returns this thread's errno pointer.
    unsafe {
        *libc::__error()
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: `__errno_location` returns this thread's errno pointer.
    unsafe {
        *libc::__errno_location()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_layout::support::Fixture;
    use crate::cli::Profile;
    use crate::identity::SlotKey;
    use crate::store::tests::seed_fixture;
    use crate::store::{Acquired, Store, WaitAccounting};
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn set_times(path: &Path, atime: SystemTime, mtime: SystemTime) {
        let name = c_string(path.as_os_str()).unwrap();
        let times = [to_timespec(atime).unwrap(), to_timespec(mtime).unwrap()];
        // SAFETY: `name` is NUL-terminated and `times` holds two valid entries.
        assert_eq!(
            unsafe {
                libc::utimensat(
                    libc::AT_FDCWD,
                    name.as_ptr(),
                    times.as_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            },
            0
        );
    }

    fn backdate_tree(path: &Path, atime: SystemTime, mtime: SystemTime) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            if meta.is_dir() {
                backdate_tree(&entry.path(), atime, mtime);
            } else if meta.is_file() {
                set_times(&entry.path(), atime, mtime);
            }
        }
        set_times(path, atime, mtime);
    }

    fn write_unit(build: &Path, profile: &str, pkg: &str, hash: &str, payload: &[u8]) -> PathBuf {
        let unit = build.join(profile).join("build").join(pkg).join(hash);
        fs::create_dir_all(unit.join("fingerprint")).unwrap();
        fs::create_dir_all(unit.join("out")).unwrap();
        let target = pkg.replace('-', "_");
        fs::write(
            unit.join("fingerprint").join(format!("lib-{target}")),
            b"fp",
        )
        .unwrap();
        fs::write(
            unit.join("fingerprint").join(format!("lib-{target}.json")),
            b"{}",
        )
        .unwrap();
        fs::write(
            unit.join("fingerprint").join(format!("dep-lib-{target}")),
            b"dep",
        )
        .unwrap();
        fs::write(
            unit.join("out").join(format!("lib{target}-{hash}.rmeta")),
            payload,
        )
        .unwrap();
        unit
    }

    fn test_limits() -> Limits {
        Limits {
            max_age: Duration::from_secs(7 * 86_400),
            max_elapsed: Duration::from_secs(5),
            max_profiles: PRUNE_MAX_PROFILES,
            max_packages: PRUNE_MAX_PACKAGES,
            max_units: PRUNE_MAX_UNITS,
            max_entries: PRUNE_MAX_ENTRIES,
            max_depth: PRUNE_MAX_DEPTH,
            max_deletions: PRUNE_MAX_DELETIONS,
        }
    }

    fn acquire_slot(store: &Store, key: &SlotKey) -> Lease {
        let mut waits = WaitAccounting::default();
        match store
            .acquire(key, Profile::Quick, false, None, &mut waits)
            .expect("acquire")
        {
            Acquired::Lease(lease) => lease,
            _ => panic!("expected slot lease"),
        }
    }

    fn warm_lease(store: &Store, key: &SlotKey) -> Lease {
        let first = acquire_slot(store, key);
        fs::create_dir_all(&first.build_dir).unwrap();
        store.release(first, None).unwrap();
        acquire_slot(store, key)
    }

    #[test]
    fn prunes_stale_unused_units_while_keeping_touched_and_recent_units() {
        let (root, store, key) = seed_fixture();
        let held = warm_lease(&store, &key);
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400);

        let stale = write_unit(
            &held.build_dir,
            "debug",
            "oldpkg",
            "0123456789abcdef",
            b"old",
        );
        let active = write_unit(
            &held.build_dir,
            "debug",
            "curpkg",
            "fedcba9876543210",
            b"cur",
        );
        let recent = write_unit(
            &held.build_dir,
            "debug",
            "newpkg",
            "aaaaaaaaaaaaaaaa",
            b"new",
        );
        backdate_tree(&stale, old, old);
        backdate_tree(&active, old, old);

        let session = prepare_with(&held.build_dir, test_limits()).expect("atime supported");
        assert!(session
            .armed_units
            .contains("debug/oldpkg/0123456789abcdef"));
        assert!(session
            .armed_units
            .contains("debug/curpkg/fedcba9876543210"));

        // Simulate Cargo reading the active unit's fingerprint during the build.
        let active_fp = active.join("fingerprint/lib-curpkg");
        let _ = fs::read(&active_fp).unwrap();
        // Verify mtime was never altered by arming.
        let active_mtime = fs::symlink_metadata(&active_fp)
            .unwrap()
            .modified()
            .unwrap();
        assert!(
            active_mtime <= old + Duration::from_secs(1),
            "arming must not advance mtime"
        );

        let report = finish_with(&held, store.control(), Some(session), test_limits());
        assert_eq!(report.pruned_units, 1);
        assert!(report.reclaimed_bytes >= 3);
        assert!(!stale.exists(), "stale unused unit must be pruned");
        assert!(!held.build_dir.join("debug/build/oldpkg").exists());
        assert!(active.exists(), "unit read during build must stay");
        assert!(recent.exists(), "recent unit must stay");

        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_prune_without_exclusive_hold_or_when_build_failed() {
        let (root, store, key) = seed_fixture();
        let mut held = warm_lease(&store, &key);
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400);
        let stale = write_unit(
            &held.build_dir,
            "debug",
            "oldpkg",
            "0123456789abcdef",
            b"old",
        );
        backdate_tree(&stale, old, old);

        let session = prepare(&held);
        assert!(session.is_some());

        // Failed build: never prunes.
        let failed = finish(&held, store.control(), session, false);
        assert_eq!(failed.pruned_units, 0);
        assert!(stale.exists());

        // Missing exclusive Barge hold: never prunes.
        let session = prepare_with(&held.build_dir, test_limits());
        held.drop_hold_for_tests();
        let unheld = finish_with(&held, store.control(), session, test_limits());
        assert_eq!(unheld.pruned_units, 0);
        assert!(stale.exists());

        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leaves_layout_v1_slots_and_unknown_entries_completely_untouched() {
        let (root, store, key) = seed_fixture();
        let held = warm_lease(&store, &key);
        let outside = Fixture::new("prune-outside");
        outside.file("secret.txt");
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400);

        // Profile member with v1 `deps/`: entire member must be untouched even if
        // a matching `<pkg>/<16-hex>/fingerprint` subtree exists inside `build/`.
        fs::create_dir_all(held.build_dir.join("debug/deps")).unwrap();
        let v1_unit = write_unit(&held.build_dir, "debug", "v1pkg", "0123456789abcdef", b"v1");
        backdate_tree(&v1_unit, old, old);

        // Release member is pure v2, with both valid and unrecognized entries.
        let stale_valid = write_unit(
            &held.build_dir,
            "release",
            "okpkg",
            "0123456789abcdef",
            b"ok",
        );
        let unknown_in_unit = write_unit(
            &held.build_dir,
            "release",
            "weird",
            "1111111111111111",
            b"x",
        );
        fs::write(unknown_in_unit.join("extra-note.txt"), b"keep").unwrap();
        let symlink_in_out = write_unit(
            &held.build_dir,
            "release",
            "linked",
            "2222222222222222",
            b"x",
        );
        symlink(
            outside.0.join("secret.txt"),
            symlink_in_out.join("out/secret-link"),
        )
        .unwrap();
        let no_fingerprint = held
            .build_dir
            .join("release/build/doctest/3333333333333333/out");
        fs::create_dir_all(&no_fingerprint).unwrap();
        fs::write(no_fingerprint.join("keep"), b"k").unwrap();
        let non_hex = held.build_dir.join("release/build/okpkg/not-a-hex-hash");
        fs::create_dir_all(&non_hex).unwrap();
        fs::write(non_hex.join("keep"), b"k").unwrap();

        for path in [
            &stale_valid,
            &unknown_in_unit,
            &symlink_in_out,
            no_fingerprint.parent().unwrap(),
            &non_hex,
        ] {
            backdate_tree(path, old, old);
        }

        let session = prepare_with(&held.build_dir, test_limits());
        let report = finish_with(&held, store.control(), session, test_limits());
        assert_eq!(report.pruned_units, 1);
        assert!(!stale_valid.exists());
        assert!(v1_unit.exists(), "v1 member must never be pruned");
        assert!(unknown_in_unit.join("extra-note.txt").exists());
        assert!(symlink_in_out
            .join("out/secret-link")
            .symlink_metadata()
            .is_ok());
        assert!(outside.0.join("secret.txt").exists());
        assert!(no_fingerprint.join("keep").exists());
        assert!(non_hex.join("keep").exists());

        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bounds_deletion_count_and_entry_walk() {
        let (root, store, key) = seed_fixture();
        let held = warm_lease(&store, &key);
        let old = SystemTime::now() - Duration::from_secs(30 * 86_400);

        let u1 = write_unit(&held.build_dir, "debug", "pkg1", "0000000000000001", b"1");
        let u2 = write_unit(&held.build_dir, "debug", "pkg2", "0000000000000002", b"2");
        let u3 = write_unit(&held.build_dir, "debug", "pkg3", "0000000000000003", b"3");
        for u in [&u1, &u2, &u3] {
            backdate_tree(u, old, old);
        }

        let session = prepare_with(&held.build_dir, test_limits());
        let mut bounded = test_limits();
        bounded.max_deletions = 1;
        let report = finish_with(&held, store.control(), session, bounded);
        assert_eq!(report.pruned_units, 1);
        assert!(report.stopped_by_bound);
        let remaining = [&u1, &u2, &u3].iter().filter(|u| u.exists()).count();
        assert_eq!(remaining, 2);

        // Zero time bound stops without deleting anything.
        let session2 = prepare_with(&held.build_dir, test_limits());
        let mut zero_time = test_limits();
        zero_time.max_elapsed = Duration::ZERO;
        let report2 = finish_with(&held, store.control(), session2, zero_time);
        assert_eq!(report2.pruned_units, 0);
        assert!(report2.stopped_by_bound);
        assert_eq!([&u1, &u2, &u3].iter().filter(|u| u.exists()).count(), 2);

        // Tiny entry bound stops cleanly without leaving partial deletions.
        let session3 = prepare_with(&held.build_dir, test_limits());
        let mut tiny_entries = test_limits();
        tiny_entries.max_entries = 1;
        let report3 = finish_with(&held, store.control(), session3, tiny_entries);
        assert_eq!(report3.pruned_units, 0);
        assert!(report3.stopped_by_bound);
        assert_eq!([&u1, &u2, &u3].iter().filter(|u| u.exists()).count(), 2);

        store.release(held, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
