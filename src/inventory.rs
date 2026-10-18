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

/// Walk one canonical root with finite bounds and return a report.
///
/// The root is canonicalized once so the operator's explicit path resolves to a
/// real directory. Traversal below the root never follows symlinks and never
/// crosses a device boundary.
pub fn inventory_directory(
    root: &Path,
    limits: InventoryLimits,
) -> Result<InventoryReport, InventoryError> {
    limits.validate()?;
    let canonical = fs::canonicalize(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => InventoryError::RootMissing(root.to_path_buf()),
        _ => InventoryError::RootUnreadable {
            root: root.to_path_buf(),
            reason: error.to_string(),
        },
    })?;
    let root_metadata =
        fs::metadata(&canonical).map_err(|error| InventoryError::RootUnreadable {
            root: canonical.clone(),
            reason: error.to_string(),
        })?;
    if !root_metadata.is_dir() {
        return Err(InventoryError::RootNotDirectory(canonical));
    }
    let root_device = root_metadata.dev();
    let started = Instant::now();
    let mut walker = Walker::new(&canonical, root_device, limits, started);
    walker.run();
    let filesystem_free = probe_filesystem_free(&canonical, root_device);
    Ok(walker.finish(filesystem_free))
}

struct InodeRecord {
    allocated: u64,
    logical: u64,
    categories: u8,
}

struct Walker<'a> {
    root: &'a Path,
    limits: InventoryLimits,
    started: Instant,
    root_device: u64,
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
}

impl<'a> Walker<'a> {
    fn new(root: &'a Path, root_device: u64, limits: InventoryLimits, started: Instant) -> Self {
        Self {
            root,
            limits,
            started,
            root_device,
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

    fn run(&mut self) {
        let mut stack: Vec<(PathBuf, usize)> = vec![(self.root.to_path_buf(), 0)];
        while let Some((directory, depth)) = stack.pop() {
            if self.check_deadline() {
                return;
            }
            if self.entries_visited >= self.limits.max_entries {
                self.gap(GapKind::EntryLimit);
                return;
            }
            let Ok(entries) = fs::read_dir(&directory) else {
                self.gap(GapKind::UnreadableEntry);
                continue;
            };
            self.directories_visited += 1;
            if depth > self.max_depth_reached {
                self.max_depth_reached = depth;
            }
            for entry in entries {
                if self.entries_visited >= self.limits.max_entries {
                    self.gap(GapKind::EntryLimit);
                    return;
                }
                if self.check_deadline() {
                    return;
                }
                let Ok(entry) = entry else {
                    self.gap(GapKind::UnreadableEntry);
                    continue;
                };
                self.entries_visited += 1;
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    self.gap(GapKind::MissingMetadata);
                    continue;
                };
                let relative = path.strip_prefix(self.root).unwrap_or(&path);
                let class_path = if file_type.is_dir() {
                    relative
                } else {
                    relative.parent().unwrap_or(Path::new(""))
                };
                let index = category_index(classify(class_path));
                if file_type.is_symlink() {
                    self.symlinks[index] += 1;
                } else if file_type.is_dir() {
                    match fs::symlink_metadata(&path) {
                        Ok(metadata) if metadata.dev() != self.root_device => {
                            self.gap(GapKind::MountBoundary);
                            continue;
                        }
                        Ok(_) => {}
                        Err(_) => {
                            self.gap(GapKind::MissingMetadata);
                            continue;
                        }
                    }
                    self.directories[index] += 1;
                    if depth + 1 > self.limits.max_depth {
                        self.gap(GapKind::DepthLimit);
                        continue;
                    }
                    stack.push((path, depth + 1));
                } else if file_type.is_file() {
                    self.record_file(&path, index);
                } else {
                    self.other[index] += 1;
                }
            }
        }
    }

    fn record_file(&mut self, path: &Path, index: usize) {
        let Ok(metadata) = fs::symlink_metadata(path) else {
            self.gap(GapKind::MissingMetadata);
            return;
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
        let notes = build_notes(&coverage, &total, &hardlink_overlap);
        let policy = build_policy(dominant_category);

        InventoryReport {
            root: self.root.to_path_buf(),
            coverage,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
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
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let still_readable = fs::read_dir(&locked).is_ok();
        let report = inventory_directory(fixture.path(), InventoryLimits::default()).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();

        if still_readable {
            // Privileged environments can read a 0o000 directory.
            return;
        }
        assert!(!report.coverage.complete);
        assert!(gap(&report, GapKind::UnreadableEntry) >= 1);
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
}
