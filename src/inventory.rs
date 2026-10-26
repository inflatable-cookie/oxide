//! Bounded, read-only inventory of one explicitly selected Rust target
//! directory.
//!
//! The inventory answers what occupies space in an operator-named directory
//! without invoking Cargo or the compiler, without following symlinks, without
//! crossing mounts and without reading file payloads, configuration or
//! secrets. It reports logical bytes and allocated bytes separately, counts the
//! global `(device, inode)` union so hardlink overlap is visible, and marks
//! partial coverage explicitly when a bound, an unreadable entry or a
//! filesystem limitation is hit. It never infers a cache hit rate and never
//! recommends deleting objects.
//!
//! The owning contract is
//! `docs/knowledge/contracts/storage-inventory.md`. This module has no daemon,
//! scheduler, dependency scanner, parser graph or cleanup authority.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Default finite maximum number of directory entries visited.
pub const DEFAULT_MAX_ENTRIES: u64 = 200_000;
/// Default finite maximum directory depth read below the selected root.
pub const DEFAULT_MAX_DEPTH: usize = 64;
/// Default finite output cap in bytes for one rendered report.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
/// Default cooperative traversal deadline in milliseconds.
pub const DEFAULT_MAX_ELAPSED_MS: u64 = 30_000;

/// Hard-limit list field for one rendered report. The report body is bounded by
/// construction; these caps are still enforced so a future field cannot make
/// output unbounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryLimits {
    /// Maximum directory entries visited before traversal stops partial.
    pub max_entries: u64,
    /// Maximum directory depth read below the root. `0` reads only root entries.
    pub max_depth: usize,
    /// Maximum rendered bytes for one report. Must be at least 64.
    pub max_output_bytes: usize,
    /// Cooperative traversal deadline in milliseconds. `0` stops immediately.
    pub max_elapsed_ms: u64,
}

impl Default for InventoryLimits {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_depth: DEFAULT_MAX_DEPTH,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            max_elapsed_ms: DEFAULT_MAX_ELAPSED_MS,
        }
    }
}

impl InventoryLimits {
    /// Reject limits that cannot produce a bounded report.
    pub fn validate(&self) -> Result<(), InventoryError> {
        if self.max_entries == 0 {
            return Err(InventoryError::InvalidLimits(
                "max_entries must be at least 1",
            ));
        }
        if self.max_output_bytes < 64 {
            return Err(InventoryError::InvalidLimits(
                "max_output_bytes must be at least 64",
            ));
        }
        Ok(())
    }

    fn elapsed_limit(&self) -> Duration {
        Duration::from_millis(self.max_elapsed_ms)
    }
}

/// Known Cargo target subdirectory categories. `Other` holds everything that
/// is not a recognized target category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Deps,
    Incremental,
    Build,
    Fingerprints,
    Other,
}

/// Fixed output order and classification priority.
const CATEGORY_ORDER: [Category; 5] = [
    Category::Deps,
    Category::Incremental,
    Category::Build,
    Category::Fingerprints,
    Category::Other,
];

fn category_index(category: Category) -> usize {
    match category {
        Category::Deps => 0,
        Category::Incremental => 1,
        Category::Build => 2,
        Category::Fingerprints => 3,
        Category::Other => 4,
    }
}

fn classify(relative: &Path) -> Category {
    let mut found: Option<Category> = None;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let Some(name) = name.to_str() else {
            continue;
        };
        let candidate = match name {
            "deps" => Category::Deps,
            "incremental" => Category::Incremental,
            "build" => Category::Build,
            ".fingerprint" => Category::Fingerprints,
            _ => continue,
        };
        if found.is_none_or(|current| category_index(candidate) < category_index(current)) {
            found = Some(candidate);
        }
    }
    found.unwrap_or(Category::Other)
}

/// Why a report is not exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKind {
    /// The finite entry maximum was reached.
    EntryLimit,
    /// A directory was deeper than the finite depth maximum.
    DepthLimit,
    /// The cooperative elapsed deadline was reached.
    ElapsedDeadline,
    /// A directory or file was on another device and was not traversed.
    MountBoundary,
    /// A directory could not be listed, or a directory entry failed to decode.
    UnreadableEntry,
    /// Metadata for an entry could not be read.
    MissingMetadata,
    /// Metadata could not be interpreted for accounting.
    UnsupportedMetadata,
    /// A rendered report exceeded the output cap.
    OutputLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageGap {
    pub kind: GapKind,
    pub occurrences: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub complete: bool,
    pub gaps: Vec<CoverageGap>,
}

/// Counts and bytes within one scope.
///
/// `logical_bytes` sums apparent sizes over every name in the scope.
/// `allocated_bytes` sums allocation over distinct `(device, inode)` values in
/// the scope, so a repeated hardlink is counted once within a category but can
/// still overlap another category.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub other: u64,
    pub logical_bytes: u64,
    pub distinct_inodes: u64,
    pub allocated_bytes: u64,
}

/// Whole-traversal accounting, which is the union over all categories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeAccounting {
    /// Regular-file names, counting every hardlink name.
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub other: u64,
    /// Apparent bytes over names (a hardlink is counted once per name).
    pub logical_bytes_by_name: u64,
    /// Apparent bytes over distinct regular-file inodes.
    pub logical_bytes_by_inode: u64,
    /// Distinct regular-file inodes.
    pub distinct_files: u64,
    /// Allocated bytes over distinct `(device, inode)` values.
    pub allocated_bytes: u64,
}

/// Cross-category hardlink sharing. None of these values are physical disk
/// usage and none are reclaimable space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardlinkOverlap {
    /// Names beyond the first for a repeated inode.
    pub extra_names: u64,
    /// Inodes present in two or more categories.
    pub shared_inodes: u64,
    /// Apparent bytes counted more than once across names.
    pub logical_bytes_shared: u64,
    /// Allocated bytes counted more than once because categories overlap.
    pub allocated_bytes_shared: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryInventory {
    pub category: Category,
    pub accounting: Accounting,
}

/// Raw filesystem capacity for the selected root's device. This is deliberately
/// separate from inventory accounting and is not reclaimable space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FilesystemFree {
    Measured {
        device: u64,
        block_size: u64,
        total_bytes: u64,
        free_bytes: u64,
        available_bytes: u64,
    },
    Unavailable {
        reason: String,
    },
}

/// A factual pointer to an existing supported policy area. It never recommends
/// deletion or predicts reuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyHint {
    pub topic: String,
    pub reference: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryReport {
    pub root: PathBuf,
    pub coverage: Coverage,
    /// Enumerated non-root paths that disappeared before a deferred read.
    /// This diagnostic does not claim their inode allocation was reclaimed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub vanished: u64,
    /// Hard-stopped subtrees, counted only by a [`scope_directory`] walk.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub stopped: u64,
    pub limits: InventoryLimits,
    pub elapsed_ms: u64,
    pub entries_visited: u64,
    pub directories_visited: u64,
    pub max_depth_reached: usize,
    pub dominant_category: Option<Category>,
    pub categories: Vec<CategoryInventory>,
    pub total: TreeAccounting,
    pub hardlink_overlap: HardlinkOverlap,
    pub filesystem_free: FilesystemFree,
    pub notes: Vec<String>,
    pub policy: Vec<PolicyHint>,
}

/// One rendered report, bounded to `InventoryLimits::max_output_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

/// Refusals that prevent any report from being produced. Partial results are
/// returned as a report with incomplete coverage, never as an error.
#[derive(Debug)]
pub enum InventoryError {
    RootMissing(PathBuf),
    RootNotDirectory(PathBuf),
    RootUnreadable { root: PathBuf, reason: String },
    InvalidLimits(&'static str),
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootMissing(root) => write!(formatter, "root does not exist: {}", root.display()),
            Self::RootNotDirectory(root) => {
                write!(formatter, "root is not a directory: {}", root.display())
            }
            Self::RootUnreadable { root, reason } => {
                write!(
                    formatter,
                    "root is unusable ({}): {}",
                    reason,
                    root.display()
                )
            }
            Self::InvalidLimits(reason) => write!(formatter, "invalid limits: {reason}"),
        }
    }
}

impl std::error::Error for InventoryError {}

/// Which read produced one enumerated entry's metadata: relative to the live
/// directory entry, or the former full-path no-follow operation for a path the
/// platform cannot name. This is test-only source accounting of the selection,
/// not a syscall count and not a performance measurement.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetadataSource {
    EntryRelative,
    FullPath,
}

/// Where a test-only directory-boundary mutation applies. These controls are
/// thread-local and exist only in test builds; no product environment hook
/// exists for them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BoundaryPoint {
    /// The enumerated regular-file or directory read.
    Entry,
    /// Before the pathname sample taken before the directory is read.
    BeforeGuard,
    /// Before the pathname sample taken after its entries were enumerated.
    BeforeConfirm,
}

fn boundary_fault(_path: &Path, _point: BoundaryPoint) -> Option<std::io::Error> {
    #[cfg(test)]
    {
        guard_faults::apply(_path, _point)
    }
    #[cfg(not(test))]
    {
        None
    }
}

/// The platform's `PATH_MAX` including its terminating NUL: a full-path
/// operation can only name a path shorter than this. It is a filesystem lookup
/// limit, never a walk bound, and it never widens completeness.
fn full_path_limit() -> usize {
    // An unavailable limit takes the conservative former full-path operation.
    usize::try_from(libc::PATH_MAX).unwrap_or(0)
}

/// Metadata for one enumerated regular file: the live directory entry when the
/// path name fits the platform limit, otherwise the former full-path no-follow
/// operation. Both keep the path-keyed test seam the file read had before.
fn enumerated_file_metadata(entry: &fs::DirEntry, path: &Path) -> std::io::Result<fs::Metadata> {
    if let Some(error) = boundary_fault(path, BoundaryPoint::Entry) {
        return Err(error);
    }
    if path.as_os_str().as_bytes().len() < full_path_limit() {
        entry_relative_metadata(entry, path, true)
    } else {
        full_path_metadata(path, true)
    }
}

/// Metadata for one enumerated directory. It reads the live directory entry
/// when the path name fits the platform limit and the former full-path no-follow
/// operation when it does not. Directory entry reads keep the debug walk hook
/// and the test-only boundary control, exactly as before.
fn enumerated_directory_metadata(
    entry: &fs::DirEntry,
    path: &Path,
) -> std::io::Result<fs::Metadata> {
    if let Some(error) = boundary_fault(path, BoundaryPoint::Entry) {
        return Err(error);
    }
    if path.as_os_str().as_bytes().len() < full_path_limit() {
        entry_relative_metadata(entry, path, false)
    } else {
        full_path_metadata(path, false)
    }
}

fn entry_relative_metadata(
    entry: &fs::DirEntry,
    path: &Path,
    injected: bool,
) -> std::io::Result<fs::Metadata> {
    #[cfg(test)]
    crate::test_fs::note_metadata_source(MetadataSource::EntryRelative);
    if injected {
        #[cfg(test)]
        return crate::test_fs::entry_metadata(entry, path);
    }
    let _ = path;
    entry.metadata()
}

fn full_path_metadata(path: &Path, injected: bool) -> std::io::Result<fs::Metadata> {
    #[cfg(test)]
    crate::test_fs::note_metadata_source(MetadataSource::FullPath);
    if injected {
        #[cfg(test)]
        return crate::test_fs::symlink_metadata(path);
    }
    fs::symlink_metadata(path)
}

/// Walk one canonical root with finite bounds and return a report.
///
/// The root is canonicalized once so the operator's explicit path resolves to a
/// real directory. Traversal below the root never follows symlinks and never
/// crosses a device boundary.
pub fn inventory_directory(
    root: &Path,
    limits: InventoryLimits,
) -> Result<InventoryReport, InventoryError> {
    #[cfg(debug_assertions)]
    walk_observer::record(root);
    inventory_with(root, limits, &[])
}

/// Walk `root` but skip the subtrees named by `stops`, which are paths
/// relative to `root`.
///
/// A stop is a **hard stop**: the named directory is neither descended into nor
/// counted, so a caller that owns those subtrees elsewhere (a pool's recorded
/// slot roots) can account for them exactly once. Stops are matched by relative
/// path name only, so they express no ownership: the caller must verify each
/// stop's own root identity before passing it. Entries that are not a stopped
/// directory are walked exactly as [`inventory_directory`] walks them, so a
/// stray region still counts and a stop that is a symlink or a file is counted
/// as the ordinary walk counts it (symlinks never contribute bytes).
///
/// This is a *different* entry point from `inventory_directory` on purpose: it
/// records on the separate [`walk_observer::SCOPE_LOG_ENV`] log, so "no payload
/// walk happened" stays a checkable claim while the scope pass is observable.
pub fn scope_directory(
    root: &Path,
    stops: &[PathBuf],
    limits: InventoryLimits,
) -> Result<InventoryReport, InventoryError> {
    #[cfg(debug_assertions)]
    walk_observer::record_scope(root);
    inventory_with(root, limits, stops)
}

fn inventory_with(
    root: &Path,
    limits: InventoryLimits,
    stops: &[PathBuf],
) -> Result<InventoryReport, InventoryError> {
    limits.validate()?;
    let canonical = fs::canonicalize(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => InventoryError::RootMissing(root.to_path_buf()),
        _ => InventoryError::RootUnreadable {
            root: root.to_path_buf(),
            reason: error.to_string(),
        },
    })?;
    let root_metadata = fs::metadata(&canonical).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            InventoryError::RootMissing(canonical.clone())
        } else {
            InventoryError::RootUnreadable {
                root: canonical.clone(),
                reason: error.to_string(),
            }
        }
    })?;
    if !root_metadata.is_dir() {
        return Err(InventoryError::RootNotDirectory(canonical));
    }
    let root_device = root_metadata.dev();
    let root_inode = root_metadata.ino();
    let started = Instant::now();
    let mut walker = Walker::new(&canonical, root_device, root_inode, limits, started, stops);
    walker.run()?;
    walker.verify_root()?;
    let filesystem_free = probe_filesystem_free(&canonical, root_device);
    Ok(walker.finish(filesystem_free))
}

struct InodeRecord {
    allocated: u64,
    logical: u64,
    categories: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkPoint {
    BeforeReadDir,
    BeforeFileType,
    BeforeSymlinkMetadata,
}

/// One directory still to read, with the identity observed when it was
/// classified. No `DirEntry`, `ReadDir`, `File` or descriptor is retained.
struct PendingDirectory {
    path: PathBuf,
    depth: usize,
    inode: u64,
}

/// Outcome of a directory identity guard. A guard samples the pathname only; it
/// never binds the opened `ReadDir` to the observed inode.
enum DirectoryGuard {
    Matched,
    Diverged,
    Refused(InventoryError),
}

struct Walker<'a> {
    root: &'a Path,
    /// Relative paths whose subtrees belong to another owner: hard stops.
    stops: &'a [PathBuf],
    stopped: u64,
    limits: InventoryLimits,
    started: Instant,
    root_device: u64,
    root_inode: u64,
    deadline_exceeded: bool,
    entries_visited: u64,
    directories_visited: u64,
    max_depth_reached: usize,
    gaps: std::collections::BTreeMap<GapKind, u64>,
    files: [u64; 5],
    directories: [u64; 5],
    symlinks: [u64; 5],
    other: [u64; 5],
    logical_by_name: [u64; 5],
    inodes: HashMap<(u64, u64), InodeRecord>,
    vanished: u64,
    #[cfg(debug_assertions)]
    test_hook: Option<walk_observer::WalkHook>,
}

impl<'a> Walker<'a> {
    fn new(
        root: &'a Path,
        root_device: u64,
        root_inode: u64,
        limits: InventoryLimits,
        started: Instant,
        stops: &'a [PathBuf],
    ) -> Self {
        Self {
            root,
            stops,
            stopped: 0,
            limits,
            started,
            root_device,
            root_inode,
            deadline_exceeded: false,
            entries_visited: 0,
            directories_visited: 0,
            max_depth_reached: 0,
            gaps: std::collections::BTreeMap::new(),
            files: [0; 5],
            directories: [0; 5],
            symlinks: [0; 5],
            other: [0; 5],
            logical_by_name: [0; 5],
            inodes: HashMap::new(),
            vanished: 0,
            #[cfg(debug_assertions)]
            test_hook: walk_observer::WalkHook::from_environment(),
        }
    }

    fn gap(&mut self, kind: GapKind) {
        *self.gaps.entry(kind).or_insert(0) += 1;
    }

    fn check_deadline(&mut self) -> bool {
        if self.deadline_exceeded {
            return true;
        }
        if self.started.elapsed() >= self.limits.elapsed_limit() {
            self.deadline_exceeded = true;
            self.gap(GapKind::ElapsedDeadline);
            return true;
        }
        false
    }

    fn run(&mut self) -> Result<(), InventoryError> {
        let mut stack: Vec<PendingDirectory> = vec![PendingDirectory {
            path: self.root.to_path_buf(),
            depth: 0,
            inode: self.root_inode,
        }];
        while let Some(pending) = stack.pop() {
            let PendingDirectory {
                path: directory,
                depth,
                inode,
            } = pending;
            let is_root = directory == self.root;
            if self.check_deadline() {
                return Ok(());
            }
            if self.entries_visited >= self.limits.max_entries {
                self.gap(GapKind::EntryLimit);
                return Ok(());
            }
            // The pathname must still name the directory observed when it was
            // classified before that directory is read. This sample detects a
            // persistent observed replacement of kind, device or inode; it
            // does not pin the directory `read_dir` opens.
            match self.guard_directory(&directory, inode, is_root, BoundaryPoint::BeforeGuard) {
                DirectoryGuard::Matched => {}
                DirectoryGuard::Diverged => continue,
                DirectoryGuard::Refused(error) => return Err(error),
            }
            if self.check_deadline() {
                return Ok(());
            }
            let read_dir = self
                .test_hook_error(&directory, WalkPoint::BeforeReadDir)
                .map_or_else(
                    || {
                        #[cfg(test)]
                        {
                            crate::test_fs::read_dir(&directory)
                        }
                        #[cfg(not(test))]
                        {
                            fs::read_dir(&directory)
                        }
                    },
                    Err,
                );
            let entries = match read_dir {
                Ok(entries) => entries,
                Err(error)
                    if directory == self.root && error.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Err(InventoryError::RootMissing(self.root.to_path_buf()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.vanished += 1;
                    continue;
                }
                Err(_) => {
                    self.gap(GapKind::UnreadableEntry);
                    continue;
                }
            };
            #[cfg(debug_assertions)]
            if is_root {
                self.test_hook_remove_after_open(&directory);
            }
            self.directories_visited += 1;
            if depth > self.max_depth_reached {
                self.max_depth_reached = depth;
            }
            // Children queued by this directory are exactly the entries above
            // this pre-enumeration depth; a failed second sample discards only
            // those and keeps every older pending directory.
            let pending_before = stack.len();
            for entry in entries {
                if self.entries_visited >= self.limits.max_entries {
                    self.gap(GapKind::EntryLimit);
                    return Ok(());
                }
                if self.check_deadline() {
                    return Ok(());
                }
                let Ok(entry) = entry else {
                    self.gap(GapKind::UnreadableEntry);
                    continue;
                };
                self.entries_visited += 1;
                let path = entry.path();
                let file_type_result = self
                    .test_hook_error(&path, WalkPoint::BeforeFileType)
                    .map_or_else(|| entry.file_type(), Err);
                let file_type = match file_type_result {
                    Ok(file_type) => file_type,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        self.vanished += 1;
                        continue;
                    }
                    Err(_) => {
                        self.gap(GapKind::MissingMetadata);
                        continue;
                    }
                };
                let relative = path.strip_prefix(self.root).unwrap_or(&path);
                // A hard stop owns its subtree elsewhere: neither descend nor
                // count it, so the caller's other accounting is the only one.
                if file_type.is_dir() && self.stops.iter().any(|stop| stop == relative) {
                    self.stopped += 1;
                    continue;
                }
                let class_path = if file_type.is_dir() {
                    relative
                } else {
                    relative.parent().unwrap_or(Path::new(""))
                };
                let index = category_index(classify(class_path));
                if file_type.is_symlink() {
                    self.symlinks[index] += 1;
                } else if file_type.is_dir() {
                    let metadata = self
                        .test_hook_error(&path, WalkPoint::BeforeSymlinkMetadata)
                        .map_or_else(|| enumerated_directory_metadata(&entry, &path), Err);
                    let metadata = match metadata {
                        // A kind that contradicts the enumerated kind is never
                        // counted, queued or unioned as its replacement.
                        Ok(metadata) if !metadata.is_dir() => {
                            self.gap(GapKind::UnsupportedMetadata);
                            continue;
                        }
                        Ok(metadata) if metadata.dev() != self.root_device => {
                            self.gap(GapKind::MountBoundary);
                            continue;
                        }
                        Ok(metadata) => metadata,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            self.vanished += 1;
                            continue;
                        }
                        Err(_) => {
                            self.gap(GapKind::MissingMetadata);
                            continue;
                        }
                    };
                    self.directories[index] += 1;
                    if depth + 1 > self.limits.max_depth {
                        self.gap(GapKind::DepthLimit);
                        continue;
                    }
                    stack.push(PendingDirectory {
                        path,
                        depth: depth + 1,
                        inode: metadata.ino(),
                    });
                } else if file_type.is_file() {
                    self.record_file(&entry, &path, index);
                } else {
                    self.other[index] += 1;
                }
            }
            // A second sample of the same pathname after enumeration. A
            // mismatch discards this directory's newly queued children only;
            // observed entries and bytes are kept and no vanished event is
            // invented for the discarded children.
            match self.guard_directory(&directory, inode, is_root, BoundaryPoint::BeforeConfirm) {
                DirectoryGuard::Matched => {}
                DirectoryGuard::Diverged => stack.truncate(pending_before),
                DirectoryGuard::Refused(error) => return Err(error),
            }
            // Both samples are charged to the same deadline: a slow final guard
            // cannot finish a complete report after the bound.
            if self.check_deadline() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Sample one directory pathname and compare it with the identity observed
    /// when the directory was classified. Root failures keep the existing
    /// refusal classes; a non-root mismatch is a partial-coverage observation.
    fn guard_directory(
        &mut self,
        path: &Path,
        expected_inode: u64,
        is_root: bool,
        point: BoundaryPoint,
    ) -> DirectoryGuard {
        if let Some(error) = boundary_fault(path, point) {
            return self.guard_error(path, error, is_root);
        }
        // The pathname sample is a plain no-follow read: directory entry faults
        // keep their existing debug walk hook and the test-only boundary
        // control rather than the file metadata seam.
        let metadata = fs::symlink_metadata(path);
        match metadata {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    if is_root {
                        return DirectoryGuard::Refused(InventoryError::RootNotDirectory(
                            path.to_path_buf(),
                        ));
                    }
                    self.gap(GapKind::UnsupportedMetadata);
                    return DirectoryGuard::Diverged;
                }
                if metadata.dev() != self.root_device {
                    if is_root {
                        return DirectoryGuard::Refused(InventoryError::RootUnreadable {
                            root: path.to_path_buf(),
                            reason: "root device changed during traversal".to_owned(),
                        });
                    }
                    self.gap(GapKind::MountBoundary);
                    return DirectoryGuard::Diverged;
                }
                if metadata.ino() != expected_inode {
                    if is_root {
                        return DirectoryGuard::Refused(InventoryError::RootUnreadable {
                            root: path.to_path_buf(),
                            reason: "root identity changed during traversal".to_owned(),
                        });
                    }
                    self.gap(GapKind::UnsupportedMetadata);
                    return DirectoryGuard::Diverged;
                }
                DirectoryGuard::Matched
            }
            Err(error) => self.guard_error(path, error, is_root),
        }
    }

    /// The unchanged error classes for one directory sample. Only a non-root
    /// `NotFound` is a vanished observation; `ENOTDIR`, `ELOOP` and every other
    /// error keep their conservative categories.
    fn guard_error(&mut self, path: &Path, error: std::io::Error, is_root: bool) -> DirectoryGuard {
        if is_root {
            return DirectoryGuard::Refused(if error.kind() == std::io::ErrorKind::NotFound {
                InventoryError::RootMissing(path.to_path_buf())
            } else {
                InventoryError::RootUnreadable {
                    root: path.to_path_buf(),
                    reason: error.to_string(),
                }
            });
        }
        if error.kind() == std::io::ErrorKind::NotFound {
            self.vanished += 1;
        } else {
            self.gap(GapKind::MissingMetadata);
        }
        DirectoryGuard::Diverged
    }

    fn record_file(&mut self, entry: &fs::DirEntry, path: &Path, index: usize) {
        let metadata = self
            .test_hook_error(path, WalkPoint::BeforeSymlinkMetadata)
            .map_or_else(|| enumerated_file_metadata(entry, path), Err);
        let metadata = match metadata {
            // A kind that contradicts the enumerated kind is never counted or
            // unioned as its replacement.
            Ok(metadata) if !metadata.is_file() => {
                self.gap(GapKind::UnsupportedMetadata);
                return;
            }
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.vanished += 1;
                return;
            }
            Err(_) => {
                self.gap(GapKind::MissingMetadata);
                return;
            }
        };
        if metadata.dev() != self.root_device {
            self.gap(GapKind::MountBoundary);
            return;
        }
        let Some(allocated) = metadata.blocks().checked_mul(512) else {
            self.gap(GapKind::UnsupportedMetadata);
            return;
        };
        let logical = metadata.size();
        self.files[index] += 1;
        self.logical_by_name[index] = self.logical_by_name[index].saturating_add(logical);
        let key = (metadata.dev(), metadata.ino());
        let bit = 1u8 << index;
        match self.inodes.get_mut(&key) {
            Some(record) => record.categories |= bit,
            None => {
                self.inodes.insert(
                    key,
                    InodeRecord {
                        allocated,
                        logical,
                        categories: bit,
                    },
                );
            }
        }
    }

    fn verify_root(&self) -> Result<(), InventoryError> {
        let metadata = fs::symlink_metadata(self.root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                InventoryError::RootMissing(self.root.to_path_buf())
            } else {
                InventoryError::RootUnreadable {
                    root: self.root.to_path_buf(),
                    reason: error.to_string(),
                }
            }
        })?;
        if !metadata.is_dir() {
            return Err(InventoryError::RootNotDirectory(self.root.to_path_buf()));
        }
        if metadata.dev() != self.root_device || metadata.ino() != self.root_inode {
            return Err(InventoryError::RootUnreadable {
                root: self.root.to_path_buf(),
                reason: "root identity changed during traversal".to_owned(),
            });
        }
        Ok(())
    }

    #[cfg(debug_assertions)]
    fn test_hook_error(&mut self, path: &Path, point: WalkPoint) -> Option<std::io::Error> {
        self.test_hook.as_mut()?.before(path, point)
    }

    #[cfg(not(debug_assertions))]
    fn test_hook_error(&mut self, _path: &Path, _point: WalkPoint) -> Option<std::io::Error> {
        None
    }

    #[cfg(debug_assertions)]
    fn test_hook_remove_after_open(&mut self, path: &Path) {
        if let Some(hook) = self.test_hook.as_mut() {
            hook.after_root_open(path);
        }
    }

    fn finish(self, filesystem_free: FilesystemFree) -> InventoryReport {
        let elapsed_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut category_allocated = [0u64; 5];
        let mut category_distinct = [0u64; 5];
        let mut tree_logical_by_inode = 0u64;
        let mut tree_allocated = 0u64;
        let mut shared_inodes = 0u64;
        for record in self.inodes.values() {
            tree_logical_by_inode = tree_logical_by_inode.saturating_add(record.logical);
            tree_allocated = tree_allocated.saturating_add(record.allocated);
            for index in 0..5 {
                if record.categories & (1 << index) != 0 {
                    category_allocated[index] =
                        category_allocated[index].saturating_add(record.allocated);
                    category_distinct[index] += 1;
                }
            }
            if record.categories.count_ones() >= 2 {
                shared_inodes += 1;
            }
        }

        let mut categories = Vec::with_capacity(5);
        let mut total = TreeAccounting::default();
        for (index, category) in CATEGORY_ORDER.iter().enumerate() {
            total.files += self.files[index];
            total.directories += self.directories[index];
            total.symlinks += self.symlinks[index];
            total.other += self.other[index];
            total.logical_bytes_by_name = total
                .logical_bytes_by_name
                .saturating_add(self.logical_by_name[index]);
            categories.push(CategoryInventory {
                category: *category,
                accounting: Accounting {
                    files: self.files[index],
                    directories: self.directories[index],
                    symlinks: self.symlinks[index],
                    other: self.other[index],
                    logical_bytes: self.logical_by_name[index],
                    distinct_inodes: category_distinct[index],
                    allocated_bytes: category_allocated[index],
                },
            });
        }
        total.distinct_files = u64::try_from(self.inodes.len()).unwrap_or(u64::MAX);
        total.logical_bytes_by_inode = tree_logical_by_inode;
        total.allocated_bytes = tree_allocated;

        let sum_category_allocated = category_allocated
            .iter()
            .fold(0u64, |sum, bytes| sum.saturating_add(*bytes));
        let hardlink_overlap = HardlinkOverlap {
            extra_names: total.files.saturating_sub(total.distinct_files),
            shared_inodes,
            logical_bytes_shared: total
                .logical_bytes_by_name
                .saturating_sub(total.logical_bytes_by_inode),
            allocated_bytes_shared: sum_category_allocated.saturating_sub(total.allocated_bytes),
        };

        let dominant_category = dominant_category(&categories);
        let gaps = self
            .gaps
            .into_iter()
            .map(|(kind, occurrences)| CoverageGap { kind, occurrences })
            .collect::<Vec<_>>();
        let coverage = Coverage {
            complete: gaps.is_empty(),
            gaps,
        };
        let notes = build_notes(&coverage, self.vanished, &total, &hardlink_overlap);
        let policy = build_policy(dominant_category);

        InventoryReport {
            root: self.root.to_path_buf(),
            coverage,
            vanished: self.vanished,
            stopped: self.stopped,
            limits: self.limits,
            elapsed_ms,
            entries_visited: self.entries_visited,
            directories_visited: self.directories_visited,
            max_depth_reached: self.max_depth_reached,
            dominant_category,
            categories,
            total,
            hardlink_overlap,
            filesystem_free,
            notes,
            policy,
        }
    }
}

fn dominant_category(categories: &[CategoryInventory]) -> Option<Category> {
    let mut best: Option<(Category, u64)> = None;
    for entry in categories {
        let bytes = entry.accounting.allocated_bytes;
        if best.is_none_or(|(_, current)| bytes > current) {
            best = Some((entry.category, bytes));
        }
    }
    match best {
        Some((category, bytes)) if bytes > 0 => Some(category),
        _ => {
            let by_logical = categories
                .iter()
                .max_by_key(|entry| entry.accounting.logical_bytes)?;
            if by_logical.accounting.logical_bytes > 0 {
                Some(by_logical.category)
            } else {
                None
            }
        }
    }
}

fn build_notes(
    coverage: &Coverage,
    vanished: u64,
    total: &TreeAccounting,
    overlap: &HardlinkOverlap,
) -> Vec<String> {
    let mut notes = Vec::new();
    if coverage.complete {
        notes.push("Coverage is complete for the visited bounds.".to_owned());
    } else {
        notes.push(
            "Coverage is partial: unvisited or unreadable entries are not represented, so the \
             reported totals are lower bounds and not an exact total."
                .to_owned(),
        );
    }
    if vanished > 0 {
        notes.push(
            "Enumerated paths vanished during this best-effort walk. The observation is not \
             atomic and does not establish that their inode allocation was freed or reclaimable."
                .to_owned(),
        );
    }
    notes.push(
        "Allocated bytes count each (device, inode) once across the whole root; per-category \
         allocated bytes overlap through hardlinks and must never be added to estimate physical \
         disk usage."
            .to_owned(),
    );
    notes.push(
        "APFS clone and snapshot sharing, Finder reclaimable space and attributable disk writes \
         are not measured here and remain unknown."
            .to_owned(),
    );
    notes.push(
        "Raw filesystem free and available bytes are a separate filesystem value; they are not \
         reclaimable space attributable to this root."
            .to_owned(),
    );
    notes.push(
        "Names, sizes and age never establish that an entry is stale, unused or safe to delete."
            .to_owned(),
    );
    if total.logical_bytes_by_inode > total.allocated_bytes {
        notes.push(
            "Apparent (logical) bytes exceed allocated bytes; sparse files, compression or clone \
             sharing can explain the difference and it does not prove reclaimable space."
                .to_owned(),
        );
    }
    if overlap.shared_inodes > 0 {
        notes.push(
            "Repeated hardlinks were observed; category totals share those inodes and only the \
             global union is a non-overlapping allocation figure."
                .to_owned(),
        );
    }
    notes.push(
        "The elapsed limit is cooperative between filesystem calls; a blocked filesystem syscall \
         cannot be preempted by this process, so the deadline is not a hard wall-clock ceiling."
            .to_owned(),
    );
    notes.push(
        "This read-only snapshot is not storage ownership, cleanup authority or a validation \
         result."
            .to_owned(),
    );
    notes
}

fn build_policy(dominant: Option<Category>) -> Vec<PolicyHint> {
    let mut policy = vec![PolicyHint {
        topic: "read-only inventory".to_owned(),
        reference: "docs/knowledge/contracts/storage-inventory.md".to_owned(),
        note: "The inventory reports occupancy only; it grants no ownership, eviction or cleanup \
               authority and infers no cache hit or miss rate."
            .to_owned(),
    }];
    let hint = match dominant {
        Some(Category::Incremental) => Some(PolicyHint {
            topic: "incremental build state".to_owned(),
            reference: "docs/knowledge/contracts/build-context-policy.md".to_owned(),
            note: "Incremental state dominates allocation here. Lifecycle-aware incremental-on \
                   versus incremental-off is a proposed context policy, not an activated default; \
                   disabling incremental does not by itself reduce cumulative writes."
                .to_owned(),
        }),
        Some(Category::Deps) => Some(PolicyHint {
            topic: "reusable intermediate slots".to_owned(),
            reference: "docs/knowledge/contracts/project-adoption.md".to_owned(),
            note: "Compiled dependencies dominate allocation here. Reusable intermediate slots \
                   require authenticated consumer integration and exclusive leases; no consumer \
                   binding is activated."
                .to_owned(),
        }),
        Some(Category::Build) => Some(PolicyHint {
            topic: "build-script output".to_owned(),
            reference: "docs/knowledge/contracts/project-adoption.md".to_owned(),
            note: "Build-script output dominates allocation here. These directories are \
                   Cargo-managed inputs to later units; editing or removing them is not an \
                   inventory action."
                .to_owned(),
        }),
        Some(Category::Fingerprints) => Some(PolicyHint {
            topic: "Cargo freshness state".to_owned(),
            reference: "docs/knowledge/qualification-readiness.md".to_owned(),
            note: "Fingerprint state dominates allocation here. It encodes Cargo freshness and \
                   must not be edited or removed to force or avoid rebuilds."
                .to_owned(),
        }),
        Some(Category::Other) | None => None,
    };
    if let Some(hint) = hint {
        policy.push(hint);
    }
    policy.push(PolicyHint {
        topic: "no automatic recommendation".to_owned(),
        reference: "docs/knowledge/contracts/optimization-adoption.md".to_owned(),
        note: "A dominating category is not a cleanup recommendation, a stale marker or a \
               predicted saving."
            .to_owned(),
    });
    policy
}

/// Probe raw filesystem capacity for the root's device. Failure is reported as
/// unavailable rather than guessed.
fn probe_filesystem_free(root: &Path, device: u64) -> FilesystemFree {
    let path = match CString::new(root.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(_) => {
            return FilesystemFree::Unavailable {
                reason: "root path contains a NUL byte".to_owned(),
            };
        }
    };
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid NUL-terminated C string and `stat` points to
    // writable storage for exactly one `statvfs` value.
    let status = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if status != 0 {
        return FilesystemFree::Unavailable {
            reason: std::io::Error::last_os_error().to_string(),
        };
    }
    // SAFETY: a zero return status means `statvfs` initialized the structure.
    let stat = unsafe { stat.assume_init() };
    let block_size = u64::from(stat.f_frsize);
    let multiply =
        |blocks: libc::fsblkcnt_t| -> Option<u64> { u64::from(blocks).checked_mul(block_size) };
    let (Some(total_bytes), Some(free_bytes), Some(available_bytes)) = (
        multiply(stat.f_blocks),
        multiply(stat.f_bfree),
        multiply(stat.f_bavail),
    ) else {
        return FilesystemFree::Unavailable {
            reason: "filesystem byte counts overflowed".to_owned(),
        };
    };
    FilesystemFree::Measured {
        device,
        block_size,
        total_bytes,
        free_bytes,
        available_bytes,
    }
}

impl InventoryReport {
    /// Render concise human output bounded to `limits.max_output_bytes`.
    pub fn render_human(&self) -> Rendered {
        let mut text = String::new();
        let _ = writeln!(text, "oxide storage inventory (read-only)");
        let _ = writeln!(text, "root: {}", self.root.display());
        if self.coverage.complete {
            let _ = writeln!(text, "coverage: complete");
        } else {
            let gaps = self
                .coverage
                .gaps
                .iter()
                .map(|gap| format!("{:?}={}", gap.kind, gap.occurrences))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(text, "coverage: partial ({gaps})");
        }
        let _ = writeln!(text, "vanished paths: {}", self.vanished);
        let _ = writeln!(
            text,
            "elapsed: {} ms, entries visited: {}, directories visited: {}, max depth: {}",
            self.elapsed_ms, self.entries_visited, self.directories_visited, self.max_depth_reached
        );
        let _ = writeln!(
            text,
            "{:<14} {:>9} {:>7} {:>7} {:>7} {:>15} {:>15}",
            "category", "files", "dirs", "links", "other", "logical bytes", "allocated bytes"
        );
        for entry in &self.categories {
            let _ = writeln!(
                text,
                "{:<14} {:>9} {:>7} {:>7} {:>7} {:>15} {:>15}",
                format!("{:?}", entry.category).to_lowercase(),
                entry.accounting.files,
                entry.accounting.directories,
                entry.accounting.symlinks,
                entry.accounting.other,
                entry.accounting.logical_bytes,
                entry.accounting.allocated_bytes
            );
        }
        let _ = writeln!(
            text,
            "total by name: files {}, logical bytes {}, symlinks {}, other {}, directories {}",
            self.total.files,
            self.total.logical_bytes_by_name,
            self.total.symlinks,
            self.total.other,
            self.total.directories
        );
        let _ = writeln!(
            text,
            "global unique inodes: distinct files {}, logical bytes {}, allocated bytes {}",
            self.total.distinct_files,
            self.total.logical_bytes_by_inode,
            self.total.allocated_bytes
        );
        let _ = writeln!(
            text,
            "hardlink overlap: extra names {}, shared inodes {}, logical bytes shared {}, \
             allocated bytes shared {}",
            self.hardlink_overlap.extra_names,
            self.hardlink_overlap.shared_inodes,
            self.hardlink_overlap.logical_bytes_shared,
            self.hardlink_overlap.allocated_bytes_shared
        );
        match &self.filesystem_free {
            FilesystemFree::Measured {
                total_bytes,
                free_bytes,
                available_bytes,
                ..
            } => {
                let _ = writeln!(
                    text,
                    "filesystem: total bytes {total_bytes}, free bytes {free_bytes}, available \
                     bytes {available_bytes} (raw filesystem values, not reclaimable space)"
                );
            }
            FilesystemFree::Unavailable { reason } => {
                let _ = writeln!(text, "filesystem: unavailable ({reason})");
            }
        }
        if let Some(category) = self.dominant_category {
            let _ = writeln!(text, "dominant category: {:?}", category);
        }
        for note in &self.notes {
            let _ = writeln!(text, "note: {note}");
        }
        for hint in &self.policy {
            let _ = writeln!(text, "policy: {} -> {}", hint.topic, hint.reference);
            let _ = writeln!(text, "  {}", hint.note);
        }
        bound_utf8(text, self.limits.max_output_bytes)
    }

    /// Render machine-readable JSON bounded to `limits.max_output_bytes`.
    ///
    /// The schema is fixed-size, so truncation is expected only for a
    /// deliberately tiny cap. The fallback is a small fixed object so a long
    /// root, note or escaped field cannot exceed the cap or produce invalid
    /// JSON.
    pub fn render_json(&self) -> Result<Rendered, serde_json::Error> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() <= self.limits.max_output_bytes {
            append_newline_within(&mut bytes, self.limits.max_output_bytes);
            return Ok(Rendered {
                bytes,
                truncated: false,
            });
        }
        Ok(truncated_json(self.limits.max_output_bytes))
    }
}

/// Compact truncated JSON, including the trailing newline, stays well under
/// the 64-byte minimum output cap and has no path or note fields.
const JSON_TRUNCATED_FALLBACK: &[u8] = b"{\"truncated\":true,\"partial\":true}\n";
const HUMAN_TRUNCATION_MARKER: &[u8] = b"\n[output truncated at max_output_bytes]\n";

fn truncated_json(limit: usize) -> Rendered {
    assert!(
        JSON_TRUNCATED_FALLBACK.len() <= limit,
        "truncated JSON fallback must fit the minimum output cap"
    );
    Rendered {
        bytes: JSON_TRUNCATED_FALLBACK.to_vec(),
        truncated: true,
    }
}

fn append_newline_within(bytes: &mut Vec<u8>, limit: usize) {
    if !bytes.ends_with(b"\n") && bytes.len() < limit {
        bytes.push(b'\n');
    }
}

fn bound_utf8(text: String, limit: usize) -> Rendered {
    let bytes = text.into_bytes();
    if bytes.len() <= limit {
        return Rendered {
            bytes,
            truncated: false,
        };
    }
    let marker = HUMAN_TRUNCATION_MARKER;
    let body_budget = limit.saturating_sub(marker.len());
    let mut cut = body_budget.min(bytes.len());
    while cut > 0 && (bytes[cut] & 0b1100_0000) == 0b1000_0000 {
        cut -= 1;
    }
    let mut bounded = Vec::with_capacity(cut + marker.len());
    bounded.extend_from_slice(&bytes[..cut]);
    bounded.extend_from_slice(marker);
    debug_assert!(bounded.len() <= limit);
    Rendered {
        bytes: bounded,
        truncated: true,
    }
}

/// Debug builds only, absent from release: walk observation and deterministic
/// mutation fixtures. Production accounting is unchanged unless a debug
/// fixture explicitly opts in through its bounded scratch environment.#[cfg(debug_assertions)]
pub mod walk_observer {
    use super::WalkPoint;
    use serde::Deserialize;
    use std::cell::RefCell;
    use std::fs::OpenOptions;
    use std::io::Write as _;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};

    /// An absolute file path: each walk appends its root as one line.
    pub const LOG_ENV: &str = "OXIDE_TEST_WALK_LOG";
    /// An absolute file path: each `scope_directory` call appends its root as
    /// one line. Separate from [`LOG_ENV`] so a caller can prove that no
    /// `inventory_directory` walk ran while its shallow scope pass still did.
    pub const SCOPE_LOG_ENV: &str = "OXIDE_TEST_SCOPE_LOG";
    #[cfg(not(test))]
    const MUTATIONS_ENV: &str = "OXIDE_TEST_WALK_MUTATIONS";

    thread_local! {
        static RECORDED: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
        static RECORDED_SCOPE: RefCell<Option<Vec<PathBuf>>> = const { RefCell::new(None) };
        #[cfg(test)]
        static TEST_MUTATIONS: RefCell<Option<String>> = const { RefCell::new(None) };
        #[cfg(test)]
        static TEST_SCRATCH_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    enum Action {
        RemoveBeforeReadDir,
        RemoveBeforeFileType,
        RemoveBeforeSymlinkMetadata,
        RemoveAfterRootOpen,
        PermissionDeniedBeforeReadDir,
        PermissionDeniedBeforeSymlinkMetadata,
        /// Test-only slowness, so a proof can drive a real walk deadline:
        /// one total delay of 1..=65,000 ms, marked fired before it sleeps
        /// and reported by a debug marker. Release builds have no hook.
        DelayBeforeReadDir,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MutationSpec {
        path: PathBuf,
        action: Action,
        #[serde(default)]
        delay_ms: Option<u64>,
    }

    struct Mutation {
        path: PathBuf,
        action: Action,
        delay_ms: Option<u64>,
        fired: bool,
    }

    pub(crate) struct WalkHook {
        mutations: Vec<Mutation>,
    }

    impl WalkHook {
        pub(crate) fn from_environment() -> Option<Self> {
            #[cfg(test)]
            let test_value = TEST_MUTATIONS.with(|cell| cell.borrow().clone());
            #[cfg(not(test))]
            let test_value: Option<String> = None;
            #[cfg(test)]
            let value = test_value.clone()?;
            #[cfg(not(test))]
            let value = {
                std::env::var("OXIDE_PROOF_DEADLINE_NS")
                    .ok()?
                    .parse::<u64>()
                    .ok()?;
                std::env::var(MUTATIONS_ENV).ok()?
            };
            if value.len() > 8192 {
                return None;
            }
            let specs: Vec<MutationSpec> = serde_json::from_str(&value).ok()?;
            if specs.is_empty() || specs.len() > 4 {
                return None;
            }
            #[cfg(test)]
            let scratch = if test_value.is_some() {
                TEST_SCRATCH_ROOT.with(|cell| cell.borrow().clone())?
            } else {
                std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").map(PathBuf::from)?
            };
            #[cfg(not(test))]
            let scratch = {
                let scratch = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").map(PathBuf::from)?;
                let evidence = std::env::var_os("OXIDE_PROOF_EVIDENCE").map(PathBuf::from)?;
                let scratch = std::fs::canonicalize(scratch).ok()?;
                let evidence = std::fs::canonicalize(evidence).ok()?;
                // Adopted runs keep evidence inside caller scratch. The
                // supervisor-owned layout keeps `scratch/` and `evidence/`
                // as siblings under its monitored root.
                let evidence_is_nested = evidence.starts_with(&scratch);
                let evidence_is_supervisor_sibling = scratch.file_name()
                    == Some(std::ffi::OsStr::new("scratch"))
                    && evidence.file_name() == Some(std::ffi::OsStr::new("evidence"))
                    && scratch.parent() == evidence.parent();
                if !scratch.starts_with("/private/tmp")
                    || !(evidence_is_nested || evidence_is_supervisor_sibling)
                {
                    return None;
                }
                scratch
            };
            let scratch = std::fs::canonicalize(scratch).ok()?;
            let mut mutations = Vec::with_capacity(specs.len());
            let mut aggregate = 0_u64;
            for spec in specs {
                // A delay belongs only to the delay action, is bounded per
                // specification and in aggregate, and anything malformed
                // refuses the whole injection before any walk runs.
                match (spec.action, spec.delay_ms) {
                    (Action::DelayBeforeReadDir, Some(delay @ 1..=65_000)) => {
                        aggregate = aggregate
                            .checked_add(delay)
                            .filter(|total| *total <= 65_000)
                            .unwrap_or(65_001);
                        if aggregate > 65_000 {
                            return None;
                        }
                    }
                    (Action::DelayBeforeReadDir, _) => return None,
                    (_, Some(_)) => return None,
                    (_, None) => {}
                }
                let candidate = if test_value.is_some() {
                    std::fs::canonicalize(&spec.path).ok()?
                } else {
                    spec.path
                };
                if !candidate.is_absolute() || candidate == scratch {
                    return None;
                }
                let canonical = std::fs::canonicalize(&candidate).ok()?;
                if canonical != candidate || !canonical.starts_with(&scratch) {
                    return None;
                }
                mutations.push(Mutation {
                    path: canonical,
                    action: spec.action,
                    delay_ms: spec.delay_ms,
                    fired: false,
                });
            }
            Some(Self { mutations })
        }

        pub(crate) fn before(&mut self, path: &Path, point: WalkPoint) -> Option<std::io::Error> {
            let mutation = self.mutations.iter_mut().find(|mutation| {
                !mutation.fired
                    && mutation.path == path
                    && matches!(
                        (mutation.action, point),
                        (Action::RemoveBeforeReadDir, WalkPoint::BeforeReadDir)
                            | (Action::RemoveBeforeFileType, WalkPoint::BeforeFileType)
                            | (
                                Action::RemoveBeforeSymlinkMetadata,
                                WalkPoint::BeforeSymlinkMetadata
                            )
                            | (
                                Action::PermissionDeniedBeforeReadDir,
                                WalkPoint::BeforeReadDir
                            )
                            | (
                                Action::PermissionDeniedBeforeSymlinkMetadata,
                                WalkPoint::BeforeSymlinkMetadata
                            )
                            | (Action::DelayBeforeReadDir, WalkPoint::BeforeReadDir)
                    )
            })?;
            mutation.fired = true;
            match mutation.action {
                Action::PermissionDeniedBeforeReadDir
                | Action::PermissionDeniedBeforeSymlinkMetadata => Some(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected permission denial",
                )),
                Action::RemoveBeforeReadDir
                | Action::RemoveBeforeFileType
                | Action::RemoveBeforeSymlinkMetadata => {
                    remove_fixture_path(path).err().or_else(|| {
                        Some(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "fixture path disappeared",
                        ))
                    })
                }
                Action::RemoveAfterRootOpen => None,
                Action::DelayBeforeReadDir => {
                    let delay = mutation.delay_ms.unwrap_or(0);
                    eprintln!("oxide: debug walk delay {delay} ms");
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    None
                }
            }
        }

        pub(super) fn after_root_open(&mut self, path: &Path) {
            let Some(mutation) = self.mutations.iter_mut().find(|mutation| {
                !mutation.fired
                    && mutation.path == path
                    && mutation.action == Action::RemoveAfterRootOpen
            }) else {
                return;
            };
            mutation.fired = true;
            if let Err(error) = remove_fixture_path(path) {
                eprintln!("oxide: debug walk fixture could not remove its scratch path: {error}");
            }
        }
    }

    fn remove_fixture_path(path: &Path) -> std::io::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
    }

    #[cfg(test)]
    pub(crate) fn with_mutations<T>(
        scratch_root: &Path,
        value: String,
        body: impl FnOnce() -> T,
    ) -> T {
        struct Restore {
            value: Option<String>,
            scratch_root: Option<PathBuf>,
        }

        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_MUTATIONS.with(|cell| cell.replace(self.value.take()));
                TEST_SCRATCH_ROOT.with(|cell| cell.replace(self.scratch_root.take()));
            }
        }

        let scratch_root =
            std::fs::canonicalize(scratch_root).expect("test mutation scratch root must exist");
        let _restore = Restore {
            value: TEST_MUTATIONS.with(|cell| cell.replace(Some(value))),
            scratch_root: TEST_SCRATCH_ROOT.with(|cell| cell.replace(Some(scratch_root))),
        };
        body()
    }

    /// Run `body` and return the roots this thread walked during it.
    pub fn observe<T>(body: impl FnOnce() -> T) -> (T, Vec<PathBuf>) {
        let outer = RECORDED.with(|cell| cell.replace(Some(Vec::new())));
        let result = body();
        let walked = RECORDED
            .with(|cell| cell.replace(outer))
            .unwrap_or_default();
        (result, walked)
    }

    /// Run `body` and return the roots this thread scope-walked during it.
    pub fn observe_scopes<T>(body: impl FnOnce() -> T) -> (T, Vec<PathBuf>) {
        let outer = RECORDED_SCOPE.with(|cell| cell.replace(Some(Vec::new())));
        let result = body();
        let walked = RECORDED_SCOPE
            .with(|cell| cell.replace(outer))
            .unwrap_or_default();
        (result, walked)
    }

    /// Test-only: whether the current thread's injection is accepted.
    #[cfg(test)]
    pub(crate) fn injection_accepted() -> bool {
        WalkHook::from_environment().is_some()
    }

    pub(super) fn record(root: &Path) {
        RECORDED.with(|cell| {
            if let Some(walked) = cell.borrow_mut().as_mut() {
                walked.push(root.to_path_buf());
            }
        });
        append_log(LOG_ENV, root);
    }

    /// Record one shallow scope pass. It is deliberately not pushed onto the
    /// walk recorder: the walk log answers "did any inventory walk run".
    pub(super) fn record_scope(root: &Path) {
        RECORDED_SCOPE.with(|cell| {
            if let Some(walked) = cell.borrow_mut().as_mut() {
                walked.push(root.to_path_buf());
            }
        });
        append_log(SCOPE_LOG_ENV, root);
    }

    /// Append one root to an absolute log file, if one is named.
    fn append_log(variable: &str, root: &Path) {
        let Some(log) = std::env::var_os(variable) else {
            return;
        };
        let log = Path::new(&log);
        let mut line = root.as_os_str().as_bytes().to_vec();
        line.push(b'\n');
        let written = log.is_absolute()
            && OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(log)
                .and_then(|mut file| file.write_all(&line))
                .is_ok();
        if !written {
            eprintln!("oxide: walk log {} not written", log.display());
        }
    }
}

/// Test-only deterministic directory-boundary mutations. They are thread-local
/// and one-shot per named point, and exist only in test builds: they let a unit
/// test replace, rename, remove or fail one observed directory between its
/// classification, its read and its second sample without a race.
#[cfg(test)]
mod guard_faults {
    use super::BoundaryPoint;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    #[derive(Clone, Debug)]
    pub(super) enum Fault {
        Remove,
        ReplaceWithSymlink(PathBuf),
        ReplaceWithFile,
        ReplaceWithDirectory,
        Fail(i32),
        Delay(u64),
    }

    struct Armed {
        path: PathBuf,
        point: BoundaryPoint,
        fault: Fault,
        fired: bool,
    }

    thread_local! {
        static ARMED: RefCell<Option<Armed>> = const { RefCell::new(None) };
    }

    /// Arm one boundary mutation for `path` and run `body`. The mutation fires
    /// at most once, at `point`, and the thread's armed state is restored even
    /// if `body` unwinds.
    pub(super) fn with_fault<T>(
        path: &Path,
        point: BoundaryPoint,
        fault: Fault,
        body: impl FnOnce() -> T,
    ) -> T {
        let path = fs::canonicalize(path).expect("a boundary fault path must exist");
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                ARMED.with(|armed| {
                    let _ = armed.borrow_mut().take();
                });
            }
        }
        ARMED.with(|armed| {
            *armed.borrow_mut() = Some(Armed {
                path,
                point,
                fault,
                fired: false,
            });
        });
        let _restore = Restore;
        body()
    }

    /// Apply the armed fault when it targets this path and point. Filesystem
    /// mutations act on the fixture and let the walk's own read observe them;
    /// only an injected error or a bounded delay is returned directly.
    pub(super) fn apply(path: &Path, point: BoundaryPoint) -> Option<std::io::Error> {
        let fault = ARMED.with(|armed| {
            let mut armed = armed.borrow_mut();
            let armed = armed.as_mut()?;
            if armed.fired || armed.point != point || armed.path != path {
                return None;
            }
            armed.fired = true;
            Some(armed.fault.clone())
        })?;
        match fault {
            Fault::Remove => {
                remove(path);
                None
            }
            Fault::ReplaceWithSymlink(target) => {
                remove(path);
                if let Err(error) = symlink(&target, path) {
                    eprintln!("oxide: test boundary symlink failed: {error}");
                }
                None
            }
            Fault::ReplaceWithFile => {
                remove(path);
                if let Err(error) = fs::write(path, b"replacement") {
                    eprintln!("oxide: test boundary file failed: {error}");
                }
                None
            }
            Fault::ReplaceWithDirectory => {
                remove(path);
                if let Err(error) = fs::create_dir(path) {
                    eprintln!("oxide: test boundary directory failed: {error}");
                }
                None
            }
            Fault::Fail(code) => Some(std::io::Error::from_raw_os_error(code)),
            Fault::Delay(ms) => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                None
            }
        }
    }

    fn remove(path: &Path) {
        let removed = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(path),
            Ok(_) => fs::remove_file(path),
            Err(_) => return,
        };
        if let Err(error) = removed {
            eprintln!("oxide: test boundary removal failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{CString, OsStr, OsString};
    use std::io::Write as _;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, MetadataExt};
    use std::path::Path as StdPath;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    static NEXT_ROOT: AtomicUsize = AtomicUsize::new(1);

    struct Fixture {
        path: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let parent = base.join("inventory");
            fs::create_dir_all(&parent).expect("create inventory fixture parent");
            loop {
                let id = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
                let path = parent.join(format!("{label}-{:x}-{id:x}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create inventory fixture: {error}"),
                }
            }
        }

        fn path(&self) -> &StdPath {
            &self.path
        }

        fn file(&self, relative: &str, bytes: usize) -> PathBuf {
            let path = self.path.join(relative);
            fs::create_dir_all(path.parent().expect("fixture file has a parent")).unwrap();
            let payload = vec![b'x'; bytes];
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(&payload).unwrap();
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn category(report: &InventoryReport, wanted: Category) -> Accounting {
        report
            .categories
            .iter()
            .find(|entry| entry.category == wanted)
            .expect("category is always present")
            .accounting
    }

    fn gap(report: &InventoryReport, wanted: GapKind) -> u64 {
        report
            .coverage
            .gaps
            .iter()
            .filter(|gap| gap.kind == wanted)
            .map(|gap| gap.occurrences)
            .sum()
    }

    fn with_walk_mutations<T>(
        scratch_root: &StdPath,
        value: serde_json::Value,
        body: impl FnOnce() -> T,
    ) -> T {
        walk_observer::with_mutations(scratch_root, value.to_string(), body)
    }

    /// The debug-only delay action is strictly bounded, scoped, one-shot and
    /// can drive a real walk deadline. It never exists outside debug builds.
    #[test]
    fn scope_directory_hard_stops_at_named_subtrees_and_counts_everything_else() {
        // A payload-shaped tree: `slots/` holds one legacy root and one
        // labelled root, plus out-of-slot metadata, a stray file and a stray
        // subtree that belongs to no slot. Logical bytes are exact file sizes,
        // so they are the deterministic oracle here.
        let fixture = Fixture::new("scope");
        fixture.file("slots/3/build/blob", 40_000);
        fixture.file("slots/oxide-ab/quick-7/build/blob", 50_000);
        fixture.file("slots/.DS_Store", 1_000);
        fixture.file("slots/stray.bin", 20_000);
        fixture.file("slots/orphan/nested/blob", 30_000);
        fixture.file(".DS_Store", 2_000);
        let outside = 20_000 + 30_000 + 1_000 + 2_000;
        let inside = 40_000 + 50_000;
        let stops = [
            PathBuf::from("slots/3"),
            PathBuf::from("slots/oxide-ab/quick-7"),
        ];

        let scoped = scope_directory(fixture.path(), &stops, InventoryLimits::default())
            .expect("the scope root is a directory");
        assert!(scoped.coverage.complete, "{:?}", scoped.coverage.gaps);
        assert_eq!(scoped.stopped, 2, "both stop predicates must be live");
        assert_eq!(
            scoped.total.logical_bytes_by_name, outside,
            "the scope pass did not count exactly the out-of-slot files"
        );
        assert_eq!(
            scoped.total.files, 4,
            "the scope pass counted stopped content"
        );

        // Without the stops the same tree counts the slot content too, so the
        // stop predicate is exactly what makes the two figures a partition.
        let whole = scope_directory(fixture.path(), &[], InventoryLimits::default())
            .expect("the scope root is a directory");
        assert!(whole.coverage.complete);
        assert_eq!(whole.stopped, 0);
        assert_eq!(whole.total.logical_bytes_by_name, outside + inside);
        assert_eq!(
            whole.total.logical_bytes_by_name - scoped.total.logical_bytes_by_name,
            inside,
            "the stopped content is not exactly the difference"
        );

        // A stop that names a path which is not a directory stops nothing and
        // changes no figure: it is walked as the ordinary pass walks it.
        let absent = [PathBuf::from("slots/absent")];
        let unchanged = scope_directory(fixture.path(), &absent, InventoryLimits::default())
            .expect("the scope root is a directory");
        assert!(unchanged.coverage.complete);
        assert_eq!(unchanged.stopped, 0);
        assert_eq!(
            unchanged.total.logical_bytes_by_name,
            whole.total.logical_bytes_by_name
        );
    }

    #[test]
    fn debug_delay_injection_is_bounded_scoped_one_shot_and_reaches_the_deadline() {
        let fixture = Fixture::new("delay");
        fixture.file("debug/deps/pkg.rlib", 10);
        let root = fixture.path().to_path_buf();
        let delayed = root.join("debug");
        let spec = |path: &StdPath, cost: u64| serde_json::json!([{ "path": path, "action": "delay_before_read_dir", "delay_ms": cost }]);
        for value in [
            serde_json::json!([{ "path": delayed, "action": "delay_before_read_dir" }]),
            spec(&delayed, 0),
            spec(&delayed, 65_001),
            serde_json::json!([{ "path": delayed, "action": "remove_before_read_dir", "delay_ms": 5 }]),
            // The injection must be strictly below the canonical scratch root.
            spec(&root, 5),
            serde_json::json!([
                { "path": delayed, "action": "delay_before_read_dir", "delay_ms": 40_000 },
                { "path": root.join("other"), "action": "delay_before_read_dir", "delay_ms": 30_000 }
            ]),
        ] {
            let accepted =
                with_walk_mutations(&root, value.clone(), || walk_observer::injection_accepted());
            assert!(!accepted, "injection was accepted: {value}");
        }
        let accepted = with_walk_mutations(&root, spec(&delayed, 60), || {
            walk_observer::injection_accepted()
        });
        assert!(accepted, "a bounded delay injection was refused");

        // One-shot: the first matching call waits, a second does not.
        let mut hook = with_walk_mutations(&root, spec(&delayed, 60), || {
            walk_observer::WalkHook::from_environment()
        })
        .expect("an accepted hook");
        let first = std::time::Instant::now();
        assert!(hook
            .before(&delayed, super::WalkPoint::BeforeReadDir)
            .is_none());
        let waited = first.elapsed();
        let second = std::time::Instant::now();
        assert!(hook
            .before(&delayed, super::WalkPoint::BeforeReadDir)
            .is_none());
        assert!(waited >= std::time::Duration::from_millis(40), "{waited:?}");
        assert!(
            second.elapsed() < std::time::Duration::from_millis(40),
            "the injection fired twice: {:?}",
            second.elapsed()
        );

        // A delay longer than the walk's own bound produces a real
        // elapsed-deadline gap, not a fabricated one.
        let limits = InventoryLimits {
            max_elapsed_ms: 40,
            ..InventoryLimits::default()
        };
        let report = with_walk_mutations(&root, spec(&delayed, 200), || {
            inventory_directory(&root, limits)
        })
        .expect("walk under the delay");
        assert!(!report.coverage.complete);
        assert!(report
            .coverage
            .gaps
            .iter()
            .any(|gap| gap.kind == GapKind::ElapsedDeadline));
    }

    #[test]
    fn categories_group_known_target_subdirectories() {
        let fixture = Fixture::new("categories");
        fixture.file("debug/deps/pkg.rlib", 100);
        fixture.file("debug/incremental/pkg/session.o", 200);
        fixture.file("debug/build/pkg/out.rs", 300);
        fixture.file("debug/.fingerprint/pkg-hash/lib-pkg", 400);
        fixture.file("debug/notes.txt", 500);

        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert!(report.coverage.complete);
        assert_eq!(category(&report, Category::Deps).files, 1);
        assert_eq!(category(&report, Category::Incremental).files, 1);
        assert_eq!(category(&report, Category::Build).files, 1);
        assert_eq!(category(&report, Category::Fingerprints).files, 1);
        assert_eq!(category(&report, Category::Other).files, 1);
        assert_eq!(report.total.files, 5);
        assert_eq!(report.total.logical_bytes_by_name, 1500);
        assert_eq!(report.total.logical_bytes_by_inode, 1500);
        assert_eq!(report.total.distinct_files, 5);
    }

    #[test]
    fn hardlink_overlap_is_counted_once_globally() {
        let fixture = Fixture::new("hardlink");
        let original = fixture.file("debug/deps/shared.rlib", 4096);
        let linked = fixture.path().join("debug/incremental/shared.rlib");
        fs::create_dir_all(linked.parent().unwrap()).unwrap();
        fs::hard_link(&original, &linked).unwrap();
        let expected_allocated = fs::symlink_metadata(&original).unwrap().blocks() * 512;

        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert!(report.coverage.complete);
        assert_eq!(report.total.files, 2);
        assert_eq!(report.total.distinct_files, 1);
        assert_eq!(report.total.allocated_bytes, expected_allocated);
        assert_eq!(report.hardlink_overlap.extra_names, 1);
        assert_eq!(report.hardlink_overlap.shared_inodes, 1);
        assert_eq!(report.hardlink_overlap.logical_bytes_shared, 4096);
        let sum: u64 = report
            .categories
            .iter()
            .map(|entry| entry.accounting.allocated_bytes)
            .sum();
        assert_eq!(sum, expected_allocated * 2);
        assert_eq!(
            report.hardlink_overlap.allocated_bytes_shared,
            expected_allocated
        );
    }

    #[test]
    fn enumerated_paths_that_vanish_are_separate_from_gaps_and_keep_seen_bytes() {
        let fixture = Fixture::new("vanished");
        let kept = fixture.file("debug/deps/kept.rlib", 37);
        let gone_dir = fixture.path().join("debug/deps/gone");
        fs::create_dir_all(&gone_dir).unwrap();
        fs::write(gone_dir.join("never-read"), b"temporary directory bytes").unwrap();
        fs::hard_link(&kept, gone_dir.join("same-inode.rlib")).unwrap();
        let gone_file = fixture.file("debug/incremental/gone.o", 211);
        let kept_allocated = fs::symlink_metadata(&kept).unwrap().blocks() * 512;

        let report = with_walk_mutations(
            fixture.path(),
            serde_json::json!([
                {"path": gone_dir, "action": "remove_before_read_dir"},
                {"path": gone_file, "action": "remove_before_symlink_metadata"}
            ]),
            || inventory_directory(fixture.path(), InventoryLimits::default()),
        )
        .unwrap();

        assert!(report.coverage.complete);
        assert_eq!(report.coverage.gaps, Vec::new());
        assert_eq!(report.vanished, 2);
        assert_eq!(report.total.files, 1);
        assert_eq!(report.total.logical_bytes_by_name, 37);
        assert_eq!(report.total.allocated_bytes, kept_allocated);
        assert!(!gone_dir.exists());
        assert!(!gone_file.exists());
        assert!(report
            .notes
            .iter()
            .any(|note| { note.contains("not atomic") && note.contains("not establish") }));
        assert_eq!(
            serde_json::to_value(&report).unwrap()["vanished"],
            serde_json::json!(2)
        );
        let mut old_report = serde_json::to_value(&report).unwrap();
        old_report.as_object_mut().unwrap().remove("vanished");
        let decoded: InventoryReport = serde_json::from_value(old_report).unwrap();
        assert_eq!(decoded.vanished, 0);
        assert!(String::from_utf8(report.render_human().bytes)
            .unwrap()
            .contains("vanished paths: 2"));
    }

    #[test]
    fn not_found_at_file_type_is_counted_after_the_entry_was_enumerated() {
        let fixture = Fixture::new("vanished-file-type");
        let gone = fixture.file("debug/deps/gone.rlib", 99);
        let report = with_walk_mutations(
            fixture.path(),
            serde_json::json!([
                {"path": gone, "action": "remove_before_file_type"}
            ]),
            || inventory_directory(fixture.path(), InventoryLimits::default()),
        )
        .unwrap();

        assert!(report.coverage.complete);
        assert_eq!(report.vanished, 1);
        assert_eq!(report.total.files, 0);
        assert_eq!(report.total.allocated_bytes, 0);
    }

    #[test]
    fn not_found_at_directory_metadata_is_counted_as_vanished() {
        let fixture = Fixture::new("vanished-directory-metadata");
        let gone = fixture.path().join("debug/build/gone");
        fs::create_dir_all(&gone).unwrap();
        fs::write(gone.join("payload"), b"not reached").unwrap();

        let report = with_walk_mutations(
            fixture.path(),
            serde_json::json!([
                {"path": gone, "action": "remove_before_symlink_metadata"}
            ]),
            || inventory_directory(fixture.path(), InventoryLimits::default()),
        )
        .unwrap();

        assert!(report.coverage.complete);
        assert_eq!(report.vanished, 1);
        assert_eq!(report.total.directories, 2);
        assert!(!gone.exists());
    }

    #[test]
    fn permission_errors_keep_their_existing_gap_categories() {
        let fixture = Fixture::new("vanished-errors");
        let blocked_dir = fixture.path().join("debug/build/blocked");
        fs::create_dir_all(&blocked_dir).unwrap();
        let blocked_file = fixture.file("debug/deps/blocked.rlib", 99);
        let report = with_walk_mutations(
            fixture.path(),
            serde_json::json!([
                {"path": blocked_dir, "action": "permission_denied_before_read_dir"},
                {"path": blocked_file, "action": "permission_denied_before_symlink_metadata"}
            ]),
            || inventory_directory(fixture.path(), InventoryLimits::default()),
        )
        .unwrap();

        assert!(!report.coverage.complete);
        assert_eq!(gap(&report, GapKind::UnreadableEntry), 1);
        assert_eq!(gap(&report, GapKind::MissingMetadata), 1);
        assert_eq!(report.vanished, 0);
    }

    #[test]
    fn a_root_removed_after_open_is_a_root_missing_refusal() {
        let fixture = Fixture::new("root-vanished-after-open");
        fixture.file("kept-until-open", 11);
        let scratch_root = fixture.path().parent().unwrap();
        let result = with_walk_mutations(
            scratch_root,
            serde_json::json!([
                {"path": fixture.path(), "action": "remove_after_root_open"}
            ]),
            || inventory_directory(fixture.path(), InventoryLimits::default()),
        );

        assert!(matches!(
            result,
            Err(InventoryError::RootMissing(ref root)) if root == fixture.path()
        ));
        assert!(!fixture.path().exists());
    }

    #[test]
    fn sparse_files_report_logical_and_allocated_separately() {
        let fixture = Fixture::new("sparse");
        let path = fixture.path().join("debug/deps/sparse.o");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(&path).unwrap();
        file.set_len(1024 * 1024).unwrap();
        drop(file);

        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert!(report.coverage.complete);
        assert_eq!(report.total.logical_bytes_by_name, 1024 * 1024);
        assert!(
            report.total.allocated_bytes < report.total.logical_bytes_by_name,
            "expected a sparse file to allocate fewer bytes than its apparent size"
        );
    }

    #[test]
    fn symlinks_are_counted_but_never_followed() {
        let outside = Fixture::new("symlink-outside");
        let target = outside.file("secret.rs", 1234);
        let fixture = Fixture::new("symlink");
        fixture.file("debug/deps/real.rlib", 10);
        fs::create_dir_all(fixture.path().join("debug/incremental")).unwrap();
        symlink(
            &target,
            fixture.path().join("debug/incremental/escape.rlib"),
        )
        .unwrap();
        symlink(
            fixture.path().join("debug/deps"),
            fixture.path().join("debug/linkdir"),
        )
        .unwrap();

        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert!(report.coverage.complete);
        assert_eq!(report.total.files, 1);
        assert_eq!(report.total.logical_bytes_by_name, 10);
        assert_eq!(category(&report, Category::Incremental).symlinks, 1);
        assert_eq!(category(&report, Category::Incremental).files, 0);
        assert_eq!(category(&report, Category::Other).symlinks, 1);
    }

    #[test]
    fn unreadable_directory_yields_partial_coverage() {
        let fixture = Fixture::new("unreadable");
        fixture.file("debug/deps/ok.rlib", 10);
        let locked = fixture.path().join("debug/incremental");
        fixture.file("debug/incremental/hidden.o", 10);
        let denied = crate::test_fs::PermissionDeniedGuard::read_dir(&locked);
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert_eq!(denied.hits(), 1);
        assert!(!report.coverage.complete);
        assert_eq!(gap(&report, GapKind::UnreadableEntry), 1);
        assert_eq!(report.total.files, 1);
        assert_eq!(report.total.logical_bytes_by_name, 10);
    }

    #[test]
    fn entry_limit_truncates_with_partial_coverage() {
        let fixture = Fixture::new("entry-limit");
        for index in 0..20 {
            fixture.file(&format!("debug/deps/f{index}.rlib"), 1);
        }
        let limits = InventoryLimits {
            max_entries: 5,
            ..InventoryLimits::default()
        };
        let report = inventory_directory(fixture.path(), limits).unwrap();
        assert!(!report.coverage.complete);
        assert!(gap(&report, GapKind::EntryLimit) >= 1);
        assert_eq!(report.entries_visited, 5);
        assert!(report.total.files < 20);
    }

    #[test]
    fn depth_limit_truncates_with_partial_coverage() {
        let fixture = Fixture::new("depth-limit");
        fixture.file("a/b/c/d/leaf.o", 10);
        let limits = InventoryLimits {
            max_depth: 1,
            ..InventoryLimits::default()
        };
        let report = inventory_directory(fixture.path(), limits).unwrap();
        assert!(!report.coverage.complete);
        assert!(gap(&report, GapKind::DepthLimit) >= 1);
        assert_eq!(report.total.files, 0);
        assert_eq!(report.max_depth_reached, 1);
    }

    #[test]
    fn cooperative_deadline_truncates_with_partial_coverage() {
        let fixture = Fixture::new("deadline");
        fixture.file("debug/deps/f.rlib", 1);
        let limits = InventoryLimits {
            max_elapsed_ms: 0,
            ..InventoryLimits::default()
        };
        let report = inventory_directory(fixture.path(), limits).unwrap();
        assert!(!report.coverage.complete);
        assert!(gap(&report, GapKind::ElapsedDeadline) >= 1);
    }

    #[test]
    fn rendering_is_bounded_and_reports_truncation() {
        let fixture = Fixture::new("output");
        fixture.file("debug/deps/f.rlib", 10);
        let limits = InventoryLimits {
            max_output_bytes: 64,
            ..InventoryLimits::default()
        };
        let report = inventory_directory(fixture.path(), limits).unwrap();

        let human = report.render_human();
        assert!(human.truncated);
        assert!(human.bytes.len() <= 64);
        let human_text = String::from_utf8(human.bytes.clone()).unwrap();
        assert!(human_text.contains("truncated"));

        let json = report.render_json().unwrap();
        assert!(json.truncated);
        assert!(json.bytes.len() <= 64);
        let value: serde_json::Value = serde_json::from_slice(&json.bytes).unwrap();
        assert_eq!(value["truncated"], serde_json::Value::Bool(true));
        assert_eq!(value["partial"], serde_json::Value::Bool(true));
        assert!(value.get("root").is_none());
    }

    #[test]
    fn tiny_output_cap_holds_with_long_multibyte_root_and_notes() {
        let fixture = Fixture::new("output-multibyte");
        fixture.file("debug/deps/f.rlib", 10);
        let limits = InventoryLimits {
            max_output_bytes: 64,
            ..InventoryLimits::default()
        };
        let mut report = inventory_directory(fixture.path(), limits).unwrap();
        report.root = PathBuf::from(format!("{}{}", "é".repeat(80), "/debug"));
        report.notes = vec![format!("{}{}", "α".repeat(40), " remaining note")];

        let human = report.render_human();
        assert!(human.truncated);
        assert!(human.bytes.len() <= 64);
        let human_text = String::from_utf8(human.bytes).unwrap();
        assert!(human_text.contains("truncated"));

        let json = report.render_json().unwrap();
        assert!(json.truncated);
        assert!(json.bytes.len() <= 64);
        let json_text = String::from_utf8(json.bytes.clone()).unwrap();
        serde_json::from_str::<serde_json::Value>(&json_text).unwrap();
        assert!(!json_text.contains("é"));
        assert!(!json_text.contains("α"));
    }

    #[test]
    fn default_output_cap_is_honored_for_complete_reports() {
        let fixture = Fixture::new("output-default");
        fixture.file("debug/deps/f.rlib", 10);
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        let human = report.render_human();
        assert!(!human.truncated);
        assert!(human.bytes.len() <= DEFAULT_MAX_OUTPUT_BYTES);
        assert!(String::from_utf8(human.bytes.clone()).is_ok());
        let json = report.render_json().unwrap();
        assert!(!json.truncated);
        assert!(json.bytes.len() <= DEFAULT_MAX_OUTPUT_BYTES);
        serde_json::from_slice::<serde_json::Value>(&json.bytes).unwrap();
    }

    #[test]
    fn missing_and_nondirectory_roots_are_refused() {
        let fixture = Fixture::new("refused");
        let missing = fixture.path().join("does-not-exist");
        assert!(matches!(
            inventory_directory(&missing, InventoryLimits::default()),
            Err(InventoryError::RootMissing(_))
        ));
        let file = fixture.file("plain.txt", 3);
        assert!(matches!(
            inventory_directory(&file, InventoryLimits::default()),
            Err(InventoryError::RootNotDirectory(_))
        ));
    }

    #[test]
    fn invalid_limits_are_refused() {
        let fixture = Fixture::new("invalid-limits");
        fixture.file("debug/deps/f.rlib", 1);
        let limits = InventoryLimits {
            max_entries: 0,
            ..InventoryLimits::default()
        };
        assert!(matches!(
            inventory_directory(fixture.path(), limits),
            Err(InventoryError::InvalidLimits(_))
        ));
    }

    #[test]
    fn concurrent_mutation_never_panics() {
        let fixture = Fixture::new("concurrent");
        for index in 0..200 {
            fixture.file(&format!("debug/deps/keep{index}.rlib"), 8);
        }
        let churn = fixture.path().join("debug/deps");
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let churn = churn.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut round = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let path = churn.join(format!("churn{round}.rlib"));
                    let _ = fs::write(&path, [0u8; 4]);
                    let _ = fs::remove_file(&path);
                    round += 1;
                }
            })
        };
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();

        assert!(report.entries_visited >= 200);
        if !report.coverage.complete {
            assert!(report.coverage.gaps.iter().all(|gap| matches!(
                gap.kind,
                GapKind::MissingMetadata | GapKind::UnreadableEntry | GapKind::EntryLimit
            )));
        }
    }

    #[test]
    fn filesystem_capacity_is_a_distinct_measured_value() {
        let fixture = Fixture::new("filesystem");
        fixture.file("debug/deps/f.rlib", 10);
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        match &report.filesystem_free {
            FilesystemFree::Measured {
                total_bytes,
                free_bytes,
                available_bytes,
                ..
            } => {
                assert!(available_bytes <= free_bytes);
                assert!(free_bytes <= total_bytes);
            }
            FilesystemFree::Unavailable { reason } => {
                panic!("expected a measured filesystem reading: {reason}")
            }
        }
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("reclaimable space")));
    }

    #[test]
    fn notes_avoid_stale_and_savings_claims() {
        let fixture = Fixture::new("notes");
        fixture.file("debug/deps/f.rlib", 10);
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("safe to delete")));
        assert!(report
            .notes
            .iter()
            .any(|note| note.contains("not storage ownership")));
        assert!(report
            .policy
            .iter()
            .all(|hint| !hint.note.to_lowercase().contains("delet")));
    }

    fn path_bytes(path: &StdPath) -> usize {
        path.as_os_str().as_bytes().len()
    }

    fn component(name: &OsStr) -> CString {
        CString::new(name.as_bytes()).expect("a fixture component has no NUL byte")
    }

    fn open_directory(path: &StdPath) -> OwnedFd {
        let name = CString::new(path.as_os_str().as_bytes()).expect("a fixture path has no NUL");
        // SAFETY: `name` is a valid NUL-terminated path and the flags need no
        // further precondition; the descriptor is owned here.
        let raw = unsafe {
            libc::open(
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            raw >= 0,
            "open fixture directory {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        );
        // SAFETY: `raw` is a fresh descriptor this call owns.
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    fn open_at(parent: &OwnedFd, name: &OsStr) -> OwnedFd {
        let name = component(name);
        // SAFETY: a valid NUL-terminated single component with plain flags.
        let raw = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(
            raw >= 0,
            "openat fixture level: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `raw` is a fresh descriptor this call owns.
        unsafe { OwnedFd::from_raw_fd(raw) }
    }

    fn mkdir_at(parent: &OwnedFd, name: &OsStr) {
        let name = component(name);
        // SAFETY: a valid NUL-terminated single component; the mode is only a
        // permission mask.
        let status = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        assert_eq!(
            status,
            0,
            "mkdirat fixture level: {}",
            std::io::Error::last_os_error()
        );
    }

    fn unlink_at(parent: &OwnedFd, name: &OsStr, flags: i32) {
        let name = component(name);
        // SAFETY: a valid NUL-terminated single component below an open parent.
        let status = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) };
        assert_eq!(
            status,
            0,
            "unlinkat fixture level: {}",
            std::io::Error::last_os_error()
        );
    }

    fn create_file_at(parent: &OwnedFd, name: &OsStr, bytes: usize) {
        let name = component(name);
        // SAFETY: a valid NUL-terminated single component with plain flags.
        let raw = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_WRONLY | libc::O_CLOEXEC,
                0o600,
            )
        };
        assert!(
            raw >= 0,
            "openat fixture file: {}",
            std::io::Error::last_os_error()
        );
        let payload = vec![b'x'; bytes];
        // SAFETY: `raw` is writable and `payload` is a live buffer of that
        // length.
        let written = unsafe { libc::write(raw, payload.as_ptr().cast(), payload.len()) };
        assert_eq!(
            written as usize,
            bytes,
            "write fixture file: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: `raw` is owned here and closed exactly once.
        unsafe { libc::close(raw) };
    }

    struct OracleMetadata {
        allocated: u64,
        size: u64,
        inode: u64,
    }

    fn oracle_metadata(parent: &OwnedFd, name: &OsStr) -> OracleMetadata {
        let name = component(name);
        // SAFETY: zeroed storage for exactly one `stat`, read no-follow.
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: `name` is a valid single component below `parent` and `stat`
        // points to writable storage for one value.
        let status = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        assert_eq!(
            status,
            0,
            "fstatat fixture file: {}",
            std::io::Error::last_os_error()
        );
        OracleMetadata {
            allocated: u64::try_from(stat.st_blocks)
                .unwrap_or_default()
                .saturating_mul(512),
            size: u64::try_from(stat.st_size).unwrap_or_default(),
            inode: stat.st_ino,
        }
    }

    /// A chain built only with `openat`/`mkdirat` so a leaf path longer than
    /// the platform limit can be created and removed without a process-wide
    /// working directory change. Every level stays nameable by path.
    struct DeepChain {
        levels: Vec<OwnedFd>,
        names: Vec<OsString>,
        leaf: OsString,
        leaf_path: PathBuf,
    }

    impl DeepChain {
        /// `index` gives each chain its own base directory so two chains never
        /// share a component name, and the deep levels are unique per chain.
        fn new(fixture: &Fixture, index: usize, target_bytes: usize) -> Self {
            let base = fixture.path().join(format!("chain{index}"));
            fs::create_dir_all(&base).expect("create the deep-chain base");
            let base_bytes = path_bytes(&base);
            let mut depth = target_bytes.saturating_sub(base_bytes + 41) / 41;
            let leaf_bytes = loop {
                let rest = target_bytes
                    .checked_sub(base_bytes + 41 * depth + 1)
                    .expect("the target path is longer than its base");
                if (1..=200).contains(&rest) {
                    break rest;
                }
                depth += 1;
            };
            let mut levels = vec![open_directory(&base)];
            let mut names = Vec::with_capacity(depth);
            for _ in 0..depth {
                let name = OsString::from("\u{e9}".repeat(20));
                let parent = levels.last().expect("a current level");
                mkdir_at(parent, &name);
                let child = open_at(parent, &name);
                names.push(name);
                levels.push(child);
            }
            let leaf = OsString::from(format!(
                "{}{}",
                "\u{e9}".repeat(leaf_bytes / 2),
                "f".repeat(leaf_bytes % 2)
            ));
            create_file_at(levels.last().expect("a deepest level"), &leaf, 4096);
            let mut leaf_path = base.to_path_buf();
            for name in &names {
                leaf_path.push(name);
            }
            leaf_path.push(&leaf);
            assert_eq!(path_bytes(&leaf_path), target_bytes);
            Self {
                levels,
                names,
                leaf,
                leaf_path,
            }
        }

        fn oracle(&self) -> OracleMetadata {
            oracle_metadata(self.levels.last().expect("a deepest level"), &self.leaf)
        }
    }

    impl Drop for DeepChain {
        fn drop(&mut self) {
            let deepest = self.levels.last().expect("a deepest level");
            unlink_at(deepest, &self.leaf, 0);
            for index in (0..self.names.len()).rev() {
                unlink_at(&self.levels[index], &self.names[index], libc::AT_REMOVEDIR);
            }
        }
    }

    fn walk_with_fault(
        fixture: &Fixture,
        path: &StdPath,
        point: BoundaryPoint,
        fault: guard_faults::Fault,
    ) -> InventoryReport {
        guard_faults::with_fault(path, point, fault, || {
            inventory_directory(fixture.path(), InventoryLimits::default())
        })
        .expect("the fixture root is unchanged")
    }

    #[test]
    fn enumerated_entry_metadata_uses_the_live_directory_entry() {
        let fixture = Fixture::new("metadata-source");
        fixture.file("debug/deps/pkg.rlib", 4096);
        fixture.file("debug/incremental/session.o", 128);
        fs::create_dir_all(fixture.path().join("debug/build/out")).unwrap();
        symlink(
            fixture.path().join("debug/deps"),
            fixture.path().join("debug/linkdir"),
        )
        .unwrap();
        let (report, sources) = crate::test_fs::observe_metadata_sources(|| {
            inventory_directory(fixture.path(), InventoryLimits::default())
        });
        let report = report.expect("the fixture root is unchanged");
        assert!(report.coverage.complete);
        assert_eq!(report.total.files, 2);
        assert_eq!(report.total.directories, 5);
        assert_eq!(report.total.symlinks, 1);
        assert_eq!(report.total.logical_bytes_by_name, 4224);
        // A symlink is never read for C1, so only the file and directory
        // entries were read and each came from its live directory entry.
        assert!(
            sources
                .iter()
                .all(|source| *source == MetadataSource::EntryRelative),
            "unexpected metadata source: {sources:?}"
        );
        assert_eq!(
            u64::try_from(sources.len()).unwrap(),
            report.total.files + report.total.directories
        );
    }

    #[test]
    fn a_path_at_or_past_the_platform_limit_keeps_the_full_path_operation() {
        let fixture = Fixture::new("path-max");
        let limit = full_path_limit();
        let inside = DeepChain::new(&fixture, 0, limit - 1);
        let oracle = inside.oracle();
        let at = DeepChain::new(&fixture, 1, limit);
        let above = DeepChain::new(&fixture, 2, limit + 1);
        // The fallback performs exactly the former full-path operation: a path
        // at or past the limit cannot be named by path at all.
        for chain in [&at, &above] {
            let error = fs::symlink_metadata(&chain.leaf_path)
                .expect_err("a path at or past the limit must stay unnameable");
            assert_eq!(
                error.raw_os_error(),
                Some(libc::ENAMETOOLONG),
                "{}",
                chain.leaf_path.display()
            );
        }
        let (report, sources) = crate::test_fs::observe_metadata_sources(|| {
            inventory_directory(fixture.path(), InventoryLimits::default())
        });
        let report = report.expect("the fixture root is unchanged");
        assert!(!report.coverage.complete);
        assert_eq!(gap(&report, GapKind::MissingMetadata), 2);
        assert_eq!(report.vanished, 0);
        assert_eq!(report.total.files, 1);
        assert_eq!(report.total.distinct_files, 1);
        assert_eq!(report.total.allocated_bytes, oracle.allocated);
        assert_eq!(report.total.logical_bytes_by_name, oracle.size);
        assert!(
            oracle.inode != 0,
            "the descriptor oracle did not observe the leaf"
        );
        assert_eq!(
            sources
                .iter()
                .filter(|source| **source == MetadataSource::FullPath)
                .count(),
            2,
            "the fallback selection was not used exactly twice: {sources:?}"
        );
        assert!(sources
            .iter()
            .any(|source| *source == MetadataSource::EntryRelative));
    }

    #[test]
    fn replaced_entry_kinds_are_partial_and_never_counted() {
        let fixture = Fixture::new("entry-kind");
        let replaced_file = fixture.file("debug/deps/replaced.rlib", 64);
        let replaced_dir = fixture.path().join("debug/build/replaced");
        fs::create_dir_all(&replaced_dir).unwrap();
        fixture.file("debug/incremental/kept.o", 32);

        // A file that is a directory when read is never counted as one.
        let report = walk_with_fault(
            &fixture,
            &replaced_file,
            BoundaryPoint::Entry,
            guard_faults::Fault::ReplaceWithDirectory,
        );
        assert!(!report.coverage.complete);
        assert_eq!(gap(&report, GapKind::UnsupportedMetadata), 1);
        assert_eq!(report.vanished, 0);
        assert_eq!(report.total.files, 1);

        // A directory that is a symlink when read is never followed or read.
        let target = fixture.path().join("debug/deps");
        let report = walk_with_fault(
            &fixture,
            &replaced_dir,
            BoundaryPoint::Entry,
            guard_faults::Fault::ReplaceWithSymlink(target),
        );
        assert_eq!(gap(&report, GapKind::UnsupportedMetadata), 1);
        assert_eq!(report.vanished, 0);
        assert_eq!(report.total.files, 1);
    }

    #[test]
    fn a_directory_that_changes_at_a_boundary_sample_is_never_read() {
        // Each case builds its own fixture: the fault mutates the victim.
        let case =
            |label: &str, fault: guard_faults::Fault, expected: &dyn Fn(&InventoryReport)| {
                let fixture = Fixture::new(label);
                let victim = fixture.path().join("debug/build/victim");
                fs::create_dir_all(&victim).unwrap();
                fs::write(victim.join("payload"), vec![b'p'; 512]).unwrap();
                let kept = fixture.file("debug/deps/kept.rlib", 64);
                fs::create_dir_all(fixture.path().join("debug/incremental")).unwrap();
                let kept_allocated = fs::symlink_metadata(&kept).unwrap().blocks() * 512;
                let report = walk_with_fault(&fixture, &victim, BoundaryPoint::BeforeGuard, fault);
                assert_eq!(report.total.files, 1, "{label}: only kept was observed");
                assert_eq!(
                    report.total.allocated_bytes, kept_allocated,
                    "{label}: observed bytes were not retained exactly"
                );
                expected(&report);
            };

        // A persistent replacement with another directory is an identity
        // contradiction, never a read of the replacement.
        case(
            "guard-before-directory",
            guard_faults::Fault::ReplaceWithDirectory,
            &|report| {
                assert_eq!(gap(report, GapKind::UnsupportedMetadata), 1);
                assert_eq!(report.vanished, 0);
            },
        );
        case(
            "guard-before-file",
            guard_faults::Fault::ReplaceWithFile,
            &|report| {
                assert_eq!(gap(report, GapKind::UnsupportedMetadata), 1);
                assert_eq!(report.vanished, 0);
            },
        );
        case(
            "guard-before-symlink",
            guard_faults::Fault::ReplaceWithSymlink(PathBuf::from("/nonexistent-boundary-target")),
            &|report| {
                assert_eq!(gap(report, GapKind::UnsupportedMetadata), 1);
                assert_eq!(report.vanished, 0);
            },
        );
        // One vanished event for the observed directory, and none for its
        // never-observed children. A vanished path is not a coverage gap, as
        // everywhere else in this walk.
        case(
            "guard-before-remove",
            guard_faults::Fault::Remove,
            &|report| {
                assert!(report.coverage.complete);
                assert_eq!(report.vanished, 1);
            },
        );
        // ELOOP and ENOTDIR are never turned into vanished observations.
        for (label, code) in [
            ("guard-before-loop", libc::ELOOP),
            ("guard-before-notdir", libc::ENOTDIR),
        ] {
            case(label, guard_faults::Fault::Fail(code), &|report| {
                assert_eq!(gap(report, GapKind::MissingMetadata), 1);
                assert_eq!(report.vanished, 0);
            });
        }
    }

    #[test]
    fn a_directory_replaced_after_enumeration_discards_only_its_new_children() {
        let fixture = Fixture::new("guard-after");
        let victim = fixture.path().join("a");
        fs::create_dir_all(victim.join("sub")).unwrap();
        fs::write(victim.join("observed"), vec![b'o'; 256]).unwrap();
        fs::write(victim.join("sub/deep"), vec![b'd'; 256]).unwrap();
        let sibling = fixture.file("b/kept", 256);
        let observed_allocated = fs::symlink_metadata(&victim.join("observed"))
            .unwrap()
            .blocks()
            * 512;
        let sibling_allocated = fs::symlink_metadata(&sibling).unwrap().blocks() * 512;

        let report = walk_with_fault(
            &fixture,
            &victim,
            BoundaryPoint::BeforeConfirm,
            guard_faults::Fault::ReplaceWithDirectory,
        );
        assert!(!report.coverage.complete);
        assert_eq!(gap(&report, GapKind::UnsupportedMetadata), 1);
        // Discarded children are never invented as vanished observations.
        assert_eq!(report.vanished, 0);
        // The already observed file inside the replaced directory is kept, the
        // sibling directory queued before it is still read, and the deeper
        // child queued by the replaced directory is never read.
        assert_eq!(report.total.files, 2);
        assert_eq!(
            report.total.allocated_bytes,
            observed_allocated + sibling_allocated
        );
        assert_eq!(report.total.directories, 3);
    }

    #[test]
    fn a_slow_final_directory_sample_cannot_finish_after_the_deadline() {
        let fixture = Fixture::new("guard-deadline");
        let directory = fixture.path().join("debug");
        fixture.file("debug/f.rlib", 1);
        let limits = InventoryLimits {
            max_elapsed_ms: 500,
            ..InventoryLimits::default()
        };
        let report = guard_faults::with_fault(
            &directory,
            BoundaryPoint::BeforeConfirm,
            guard_faults::Fault::Delay(600),
            || inventory_directory(fixture.path(), limits),
        )
        .expect("the fixture root is unchanged");
        // The enumerated file was observed before the second sample, so the
        // walk reached that sample; the slow sample may not still produce
        // complete coverage.
        assert_eq!(report.total.files, 1);
        assert!(!report.coverage.complete);
        assert!(gap(&report, GapKind::ElapsedDeadline) >= 1);
    }

    #[test]
    fn root_boundary_samples_keep_the_existing_refusals() {
        let case = |label: &str, fault: guard_faults::Fault| -> InventoryError {
            let fixture = Fixture::new(label);
            fixture.file("kept/file", 8);
            let root = fixture.path().to_path_buf();
            let result = guard_faults::with_fault(&root, BoundaryPoint::BeforeGuard, fault, || {
                inventory_directory(&root, InventoryLimits::default())
            });
            // Restore a plain directory so the fixture's own drop can clean up
            // whatever the fault left at the root path.
            let _ = fs::remove_file(&root);
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("restore the fixture root");
            result.expect_err("the root boundary refusal is an error")
        };

        assert!(matches!(
            case("root-guard-removed", guard_faults::Fault::Remove),
            InventoryError::RootMissing(_)
        ));
        assert!(matches!(
            case(
                "root-guard-directory",
                guard_faults::Fault::ReplaceWithDirectory
            ),
            InventoryError::RootUnreadable { .. }
        ));
        assert!(matches!(
            case("root-guard-file", guard_faults::Fault::ReplaceWithFile),
            InventoryError::RootNotDirectory(_)
        ));
        assert!(matches!(
            case(
                "root-guard-symlink",
                guard_faults::Fault::ReplaceWithSymlink(PathBuf::from("/nonexistent-root-target"))
            ),
            InventoryError::RootNotDirectory(_)
        ));
        assert!(matches!(
            case("root-guard-denied", guard_faults::Fault::Fail(libc::EACCES)),
            InventoryError::RootUnreadable { .. }
        ));
    }
}
