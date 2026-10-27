//! Library custody for cold-slot seeding (`oxide#137` §15 slice 2).
//!
//! The front door keeps Cargo semantics; this module keeps custody. It exposes
//! the two calls of §6.3 and one sidecar writer:
//!
//! - [`seed_candidates`] is call 1: it takes the pool lock **alone**, validates
//!   that the recipient is still held by the caller's lease, and returns the
//!   bounded catalogs of at most eight donor slots by rank. It reads control
//!   state only: no donor payload is opened and nothing is mutated.
//! - [`seed`] is call 2: it takes the donor's record lock non-blocking and
//!   then the pool lock, both held to the end, re-reads and re-checks the
//!   selected catalog (generation, lease id and bytes), revalidates every
//!   planned donor file against the catalog's recorded digest, clones with
//!   `clonefile` only, hashes each clone against the same digest, re-reads the
//!   donor's ledger generation, and then walks the recipient as an
//!   `observed-while-held` fact before publishing anything.
//! - [`publish_seed_catalog`] writes one generation-bound catalog sidecar
//!   under `<pool>/oxide-managed/seeds/` after bounded orphan and family-size
//!   checks. It is called by the producing lease, not by [`seed`].
//!
//! Failure follows §6.5 exactly:
//!
//! - **Before the first recipient write** a missing donor, no overlap, a cap,
//!   a changed catalog or file or the cooperative deadline is a **skip**:
//!   nothing is written and the caller builds cold in the same empty slot.
//!   (`SeedSkip`.)
//! - **After it, a failed recipient walk** abandons seeding only: nothing is
//!   published, the clone is retained untrusted, and the caller releases the
//!   slot and cold-builds under ordinary admission. (`SeedAbandon`.)
//! - **After it, every other fault refuses and retains**: no deletion, no
//!   retry, no second lease. (`Err`.)
//!
//! The design names the first `clonefile` as the trust boundary. The first
//! *recipient write* is the parent-directory creation that must precede it, so
//! the library takes that write as the boundary: a clone that fails on a
//! still-untouched recipient is conservatively retained and refused rather
//! than reused. This is the strict reading, never a laxer one.
//!
//! The donor predicate of §7 has two halves. The ledger half (state `Idle`, no
//! lease, the catalog's lease as `last_lease_id`, same payload root and device,
//! matching volume binding) is checked here. The control-record half (no
//! holder, no live `oxide clean`) belongs to the front door's record, which is
//! exactly what the donor's record lock this call holds proves: a holder or a
//! clean would hold it and make the donor a skip.
//!
//! Nothing here reads the registry adapter, a source tree, an archive or a
//! Cargo home: slice 1's verification evidence reaches this layer only as the
//! catalog's recorded source and file digests, and donation re-hashes the
//! donor's own bytes against them. Source re-verification is the front door's
//! (§6.3 step 4).

pub mod catalog;
mod walk;

pub use catalog::{
    digest as catalog_digest, SeedAttemptState, SeedCatalog, SeedFile, SeedJoin, SeedPackage,
    SeedProducer, SeedRejectReason, SeedRejection, SeedRole, SeedSourceKind, SeedSourceRoot,
    SeedUnit, SEED_CATALOG_MAX_BYTES, SEED_CATALOG_SCHEMA, SEED_MAX_FILES, SEED_MAX_PACKAGES,
    SEED_MAX_REJECTED_UNITS, SEED_MAX_REJECTIONS, SEED_MAX_UNITS_PER_PACKAGE,
};
pub use walk::{WalkGap as SeedWalkGap, WalkLimits as SeedWalkLimits};

use super::pool::{Pool, SeedPoolView, SeedSlotView};
use super::{
    current_uid, ensure_private_dir, io_error, random_hex, LeaseError, MANAGED_NAME,
    MAX_LEASE_ID_BYTES, MAX_PATH_BYTES,
};
use crate::owned_delete::open_directory_at;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Donor slots examined per command: chosen by most recent rank, then slot id.
pub const SEED_MAX_CANDIDATES: usize = 8;
/// Compile units per plan.
pub const SEED_MAX_PLAN_UNITS: usize = 256;
/// Files per plan.
pub const SEED_MAX_PLAN_FILES: usize = 4096;
/// Logical bytes per plan.
pub const SEED_MAX_PLAN_BYTES: u64 = 384 * 1024 * 1024;
/// The seed's own cooperative work deadline.
pub const SEED_WORK_BUDGET_MS: u64 = 5_000;
/// The post-clone recipient walk's bounds: the capacity contract's slot walk.
pub const SEED_WALK_MAX_ENTRIES: u64 = 2_000_000;
pub const SEED_WALK_MAX_DEPTH: usize = 64;
pub const SEED_WALK_MAX_ELAPSED_MS: u64 = 60_000;
/// All seed sidecars together (catalog, attempt, last, sources).
pub const SEED_SIDECAR_FAMILY_MAX_BYTES: u64 = 24 * 1024 * 1024;
/// Sidecars one write may remove because their slot left the ledger.
pub const SEED_ORPHAN_CLEANUP_MAX: usize = 8;
/// Catalogs one write may drop to stay inside the family byte ceiling.
pub const SEED_FAMILY_DROP_MAX: usize = 8;

const SEEDS_DIR: &str = "seeds";
const CATALOG_SUFFIX: &str = ".catalog.json";

/// One pool slot generation a caller holds. All three fields are required:
/// a lease id alone cannot name a generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedRecipient {
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
}

/// The donor the caller selected from call 1's candidates, named by the exact
/// catalog bytes it planned from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedDonor {
    pub slot_id: String,
    pub incarnation: String,
    /// The producing lease, which is the ledger's `last_lease_id` while the
    /// catalog is eligible.
    pub lease_id: String,
    /// Lowercase SHA-256 of the catalog bytes call 1 returned.
    pub catalog_digest: String,
}

/// One unit the caller selected from the catalog's recorded data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedUnitRef {
    /// The unit's `fingerprint_dir`, relative to the slot's `build/`.
    pub fingerprint_dir: String,
    pub role: SeedRole,
}

/// One candidate donor from call 1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedCandidate {
    pub slot_id: String,
    pub incarnation: String,
    pub rank: u64,
    pub catalog_digest: String,
    pub catalog: SeedCatalog,
}

/// Call 1's bounded answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedCandidates {
    pub recipient: SeedRecipient,
    pub candidates: Vec<SeedCandidate>,
}

/// Call 2's input. The plan names catalog units, not caller paths: the library
/// derives every donor and recipient path from the re-read catalog itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedRequest {
    pub recipient: SeedRecipient,
    pub donor: SeedDonor,
    pub units: Vec<SeedUnitRef>,
    /// The donor's front-door record lock, which must already exist. The
    /// library takes it non-blocking and holds it to the end.
    pub donor_record_lock: PathBuf,
    /// The cooperative work deadline; `None` uses [`SEED_WORK_BUDGET_MS`].
    pub work_budget_ms: Option<u64>,
}

/// Call 2's outcome: published, skipped cold, or abandoned after the walk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SeedOutcome {
    Seeded(SeedReceipt),
    Skipped(SeedSkip),
    Abandoned(SeedAbandon),
}

/// The published seed, returned while the pool lock and the donor pin were
/// still held through the final donor check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedReceipt {
    pub donor_slot: String,
    pub donor_incarnation: String,
    pub units: u64,
    pub files: u64,
    pub logical_bytes: u64,
    pub catalog_digest: String,
    /// The `observed-while-held` fact the caller records with the attempt.
    pub observation: SeedObservation,
    pub elapsed_ms: u64,
}

/// A cold skip: nothing was written to the recipient.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedSkip {
    pub reason: SeedSkipReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedSkipReason {
    /// No donor slot had an eligible catalog.
    NoCandidate,
    DonorBusy,
    DonorLockMissing,
    /// The donor's ledger generation no longer matches the planned catalog.
    DonorGenerationChanged,
    /// The catalog bytes call 2 re-read differ from the plan's.
    CatalogChanged,
    /// The donor's `build/` tree is gone.
    DonorPayloadMissing,
    /// A planned unit is not in the re-read catalog.
    UnitMissing,
    /// A plan cap is exceeded.
    PlanCap,
    /// The seed's own cooperative deadline expired before the first write.
    WorkDeadline,
    /// A planned donor file is missing, unreadable or no longer matches its
    /// recorded digest.
    FileChanged,
    /// This platform has no `clonefile`.
    Unsupported,
}

impl SeedSkipReason {
    /// The bounded reason token a caller prints after `seeding skipped: `.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoCandidate => "no donor",
            Self::DonorBusy => "donor busy",
            Self::DonorLockMissing => "donor lock missing",
            Self::DonorGenerationChanged => "donor generation changed",
            Self::CatalogChanged => "catalog changed",
            Self::DonorPayloadMissing => "donor payload missing",
            Self::UnitMissing => "unit missing",
            Self::PlanCap => "cap",
            Self::WorkDeadline => "deadline",
            Self::FileChanged => "file changed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// A seeding attempt abandoned because the post-clone recipient walk was not
/// complete and ownership-clean. Nothing was published; the caller releases
/// the untrusted recipient and cold-builds under ordinary admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedAbandon {
    pub observation: SeedObservation,
}

/// The generation-bound `observed-while-held` walk fact. Never a closed
/// record, never an admission figure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedObservation {
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    pub lease_id: String,
    pub entries: u64,
    pub logical_bytes: u64,
    pub allocated_bytes: u64,
    /// `None` when the walk was complete and ownership-clean.
    pub gap: Option<SeedWalkGap>,
    pub limits: SeedWalkLimits,
    pub elapsed_ms: u64,
}

impl SeedObservation {
    pub fn complete(&self) -> bool {
        self.gap.is_none()
    }
}

/// Call 1: read-only candidate selection under the pool lock alone.
pub fn seed_candidates(
    pool: &Path,
    recipient: &SeedRecipient,
    max_candidates: usize,
) -> Result<SeedCandidates, LeaseError> {
    validate_recipient(recipient)?;
    let opened = Pool::open_seed_read_only(pool)?;
    let view = opened.seed_view();
    let held = view
        .slot(&recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if !held.held
        || held.incarnation.to_string() != recipient.incarnation
        || held.lease_id.as_deref() != Some(recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let mut donors: Vec<&SeedSlotView> = view
        .slots
        .iter()
        .filter(|slot| slot.idle && slot.lease_id.is_none())
        .collect();
    donors.sort_by(|left, right| right.rank.cmp(&left.rank).then(left.id.cmp(&right.id)));
    donors.truncate(max_candidates.clamp(1, SEED_MAX_CANDIDATES));
    let mut candidates = Vec::new();
    for slot in donors {
        let Some((catalog, bytes, catalog_digest)) = read_catalog(&view.pool_root, slot.id) else {
            continue;
        };
        // A catalog is eligible only while the ledger still names its
        // producing generation (§4.6), and a generation that seeded anything
        // never publishes one (§4.1).
        if catalog.store_id != view.store_id
            || catalog.slot_id != slot.id.to_string()
            || catalog.incarnation != slot.incarnation.to_string()
            || catalog.lease_id != slot.last_lease_id
            || catalog.attempt != SeedAttemptState::None
            || bytes.is_empty()
        {
            continue;
        }
        candidates.push(SeedCandidate {
            slot_id: slot.id.to_string(),
            incarnation: slot.incarnation.to_string(),
            rank: slot.rank,
            catalog_digest,
            catalog,
        });
    }
    Ok(SeedCandidates {
        recipient: recipient.clone(),
        candidates,
    })
}

/// Call 2: donor record lock, then pool lock, clone and revalidate.
pub fn seed(pool: &Path, request: &SeedRequest) -> Result<SeedOutcome, LeaseError> {
    let started = Instant::now();
    let budget = Duration::from_millis(
        request
            .work_budget_ms
            .unwrap_or(SEED_WORK_BUDGET_MS)
            .clamp(1, 60_000),
    );
    validate_request(request)?;
    if !cfg!(target_os = "macos") {
        return Ok(SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::Unsupported,
        }));
    }
    // Lock order (§7): the donor's record lock first, non-blocking and
    // `create: false`, then the pool lock, both held to the end.
    let _donor_lock = match probe_donor_lock(&request.donor_record_lock)? {
        DonorLock::Taken(file) => file,
        DonorLock::Missing => {
            return Ok(SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::DonorLockMissing,
            }))
        }
        DonorLock::Held => {
            return Ok(SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::DonorBusy,
            }))
        }
    };
    let opened = Pool::open(pool)?;
    let view = opened.seed_view();
    let recipient = view
        .slot(&request.recipient.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if !recipient.held
        || recipient.incarnation.to_string() != request.recipient.incarnation
        || recipient.lease_id.as_deref() != Some(request.recipient.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    let Some(donor) = view.slot(&request.donor.slot_id) else {
        return Ok(skip(SeedSkipReason::DonorGenerationChanged));
    };
    if !donor.idle || donor.lease_id.is_some() {
        return Ok(skip(SeedSkipReason::DonorBusy));
    }
    if donor.incarnation.to_string() != request.donor.incarnation
        || donor.last_lease_id != request.donor.lease_id
    {
        return Ok(skip(SeedSkipReason::DonorGenerationChanged));
    }
    let Some((catalog, _bytes, catalog_digest)) = read_catalog(&view.pool_root, donor.id) else {
        return Ok(skip(SeedSkipReason::CatalogChanged));
    };
    if catalog_digest != request.donor.catalog_digest
        || catalog.store_id != view.store_id
        || catalog.slot_id != request.donor.slot_id
        || catalog.incarnation != request.donor.incarnation
        || catalog.lease_id != request.donor.lease_id
        || catalog.attempt != SeedAttemptState::None
    {
        return Ok(skip(SeedSkipReason::CatalogChanged));
    }
    // The volume binding is re-verified before any payload read and again
    // before the final publication decision: a lost or re-mounted volume is
    // unknown custody, which refuses rather than cold-builds.
    opened.seed_verify_backing()?;
    let payload_device = fs::symlink_metadata(&view.payload_root)
        .map_err(io_error)?
        .dev();
    let donor_slot = opened.seed_slot_path(donor)?;
    let recipient_slot = opened.seed_slot_path(recipient)?;
    if !donor_slot.starts_with(&view.payload_root)
        || !recipient_slot.starts_with(&view.payload_root)
    {
        return Err(LeaseError::UnsafePath(
            "a slot resolves outside the pool's payload root",
        ));
    }
    let donor_fd = open_owned_directory(&donor_slot, payload_device)?;
    let recipient_fd = open_owned_directory(&recipient_slot, payload_device)?;
    // The recipient is this command's fresh slot: its `build/` is absent or
    // an empty owned directory. Anything else is refused, never cleaned.
    let recipient_build = open_recipient_build(recipient_fd.as_fd(), payload_device)?;
    let Some(donor_build) = open_optional_directory(donor_fd.as_fd(), c"build", payload_device)?
    else {
        return Ok(skip(SeedSkipReason::DonorPayloadMissing));
    };
    // Plan from the re-read catalog's own recorded file lists.
    let plan = match plan_units(&catalog, &request.units) {
        Ok(plan) => plan,
        Err(reason) => return Ok(skip(reason)),
    };
    if started.elapsed() > budget {
        return Ok(skip(SeedSkipReason::WorkDeadline));
    }
    // Every donor payload read happens here, under the pin: each planned file
    // is opened without following a link, checked against the catalog's
    // recorded size and hashed against its recorded digest before anything is
    // copied. The `.d`'s digest is its join re-check: the recorded rules were
    // derived from exactly those bytes.
    test_point(TestPoint::BeforeVerify { pool });
    let mut sources = Vec::with_capacity(plan.files.len());
    for file in &plan.files {
        if started.elapsed() > budget {
            return Ok(skip(SeedSkipReason::WorkDeadline));
        }
        let opened_file = match open_relative_file(donor_build.as_fd(), &file.path, payload_device)
        {
            Ok(opened_file) => opened_file,
            Err(_) => return Ok(skip(SeedSkipReason::FileChanged)),
        };
        let metadata = opened_file.metadata;
        let digest = match hash_fd(opened_file.fd.as_fd()) {
            Ok(digest) => digest,
            Err(_) => return Ok(skip(SeedSkipReason::FileChanged)),
        };
        if metadata.st_size.max(0) as u64 != file.size || digest != file.digest {
            return Ok(skip(SeedSkipReason::FileChanged));
        }
        sources.push(opened_file.fd);
    }
    if started.elapsed() > budget {
        return Ok(skip(SeedSkipReason::WorkDeadline));
    }
    // The first recipient write is the trust boundary (§6.5).
    let recipient_build = match recipient_build {
        Some(build) => build,
        None => create_owned_directory(recipient_fd.as_fd(), c"build", payload_device)?,
    };
    for (index, file) in plan.files.iter().enumerate() {
        let (parents, name) = split_relative(&file.path);
        let mut deepest: Option<OwnedFd> = None;
        for component in parents {
            let component = cstring(component)?;
            let parent_fd = match deepest.as_ref() {
                Some(fd) => fd.as_fd(),
                None => recipient_build.as_fd(),
            };
            deepest = Some(open_or_create_directory(
                parent_fd,
                &component,
                payload_device,
            )?);
        }
        let directory = match deepest.as_ref() {
            Some(fd) => fd.as_fd(),
            None => recipient_build.as_fd(),
        };
        let name = cstring(name)?;
        let recipient_path = recipient_slot.join("build").join(&file.path);
        test_point(TestPoint::BeforeClone {
            index,
            recipient_slot: &recipient_slot,
            relative: &file.path,
        });
        clone_file(sources[index].as_fd(), directory, &name)?;
        test_point(TestPoint::AfterClone {
            index,
            path: &recipient_path,
        });
        // The clone is hashed against the same recorded digest; a clone that
        // does not reproduce it is never published.
        let cloned = walk::open_regular(&directory, &name).map_err(io_error)?;
        let metadata = fstat(cloned.as_fd()).map_err(io_error)?;
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
            || metadata.st_uid != current_uid()
            || metadata.st_dev as u64 != payload_device
            || metadata.st_size.max(0) as u64 != file.size
            || hash_fd(cloned.as_fd())? != file.digest
        {
            return Err(LeaseError::UnsafePath(
                "a seeded clone does not match its recorded digest",
            ));
        }
        if started.elapsed() > budget {
            return Err(LeaseError::Io(
                "cold-seed work deadline expired after the first clone".to_owned(),
            ));
        }
    }
    // Final donor recheck: the ledger still names the planned generation for
    // both the donor and the recipient.
    test_point(TestPoint::BeforeFinalDonorCheck { pool });
    opened.seed_verify_backing()?;
    if !opened.seed_donor_unchanged(donor)? {
        return Err(LeaseError::UnknownAccounting);
    }
    if !opened.seed_recipient_held(recipient)? {
        return Err(LeaseError::UnknownAccounting);
    }
    // The post-clone recipient walk, before any caller proceeds (§6.3 step 7).
    test_point(TestPoint::BeforeWalk {
        slot: &recipient_slot,
    });
    let limits = walk_limits();
    // The walk's own cooperative deadline starts here; the seed's 5 s work
    // budget above is a separate bound and is not charged to the walk.
    let walked = walk::walk_slot(&recipient_slot, payload_device, limits, Instant::now())
        .map_err(io_error)?;
    let observation = SeedObservation {
        store_id: view.store_id.clone(),
        slot_id: recipient.id.to_string(),
        incarnation: recipient.incarnation.to_string(),
        lease_id: request.recipient.lease_id.clone(),
        entries: walked.entries,
        logical_bytes: walked.logical_bytes,
        allocated_bytes: walked.allocated_bytes,
        gap: walked.gap,
        limits,
        elapsed_ms: walked.elapsed_ms,
    };
    if walked.gap.is_some() {
        return Ok(SeedOutcome::Abandoned(SeedAbandon { observation }));
    }
    Ok(SeedOutcome::Seeded(SeedReceipt {
        donor_slot: request.donor.slot_id.clone(),
        donor_incarnation: request.donor.incarnation.clone(),
        units: plan.units as u64,
        files: plan.files.len() as u64,
        logical_bytes: plan.logical_bytes,
        catalog_digest,
        observation,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }))
}

/// Publish one generation-bound donor catalog sidecar.
///
/// The producing lease must still hold the slot: a catalog is written before
/// release, never after. Nothing in the payload, ledger or control state is
/// touched, and an over-cap catalog is not written.
pub fn publish_seed_catalog(pool: &Path, catalog: &SeedCatalog) -> Result<(), LeaseError> {
    let bytes = catalog.to_bytes().map_err(LeaseError::InvalidInput)?;
    let opened = Pool::open(pool)?;
    let view = opened.seed_view();
    let slot = view
        .slot(&catalog.slot_id)
        .ok_or(LeaseError::UnknownAccounting)?;
    if catalog.store_id != view.store_id
        || !slot.held
        || slot.incarnation.to_string() != catalog.incarnation
        || slot.lease_id.as_deref() != Some(catalog.lease_id.as_str())
    {
        return Err(LeaseError::UnknownAccounting);
    }
    if catalog.attempt != SeedAttemptState::None {
        return Err(LeaseError::InvalidInput(
            "a generation that seeded anything publishes no catalog",
        ));
    }
    let managed = view.pool_root.join(MANAGED_NAME);
    ensure_private_dir(&managed)?;
    let dir = managed.join(SEEDS_DIR);
    ensure_private_dir(&dir)?;
    remove_orphan_sidecars(&dir, &view)?;
    enforce_family_bytes(&dir, bytes.len() as u64, &catalog.slot_id)?;
    write_sidecar(
        &dir.join(format!("{}{CATALOG_SUFFIX}", catalog.slot_id)),
        &bytes,
    )
}

fn skip(reason: SeedSkipReason) -> SeedOutcome {
    SeedOutcome::Skipped(SeedSkip { reason })
}

fn walk_limits() -> SeedWalkLimits {
    #[cfg(test)]
    if let Some(limits) = tests::walk_limits_override() {
        return limits;
    }
    SeedWalkLimits {
        max_entries: SEED_WALK_MAX_ENTRIES,
        max_depth: SEED_WALK_MAX_DEPTH,
        max_elapsed_ms: SEED_WALK_MAX_ELAPSED_MS,
    }
}

/// The plan the library derived from the catalog: the exact file list and the
/// bytes it must still find. Nothing here came from the caller except the
/// unit identities.
struct Plan {
    units: usize,
    files: Vec<SeedFile>,
    logical_bytes: u64,
}

fn plan_units(catalog: &SeedCatalog, units: &[SeedUnitRef]) -> Result<Plan, SeedSkipReason> {
    if units.len() > SEED_MAX_PLAN_UNITS {
        return Err(SeedSkipReason::PlanCap);
    }
    let mut files: Vec<SeedFile> = Vec::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut seen_units = std::collections::BTreeSet::new();
    for wanted in units {
        if !seen_units.insert((wanted.fingerprint_dir.as_str(), wanted.role)) {
            return Err(SeedSkipReason::PlanCap);
        }
        let unit = catalog
            .packages
            .iter()
            .flat_map(|package| package.units.iter())
            .find(|unit| unit.fingerprint_dir == wanted.fingerprint_dir && unit.role == wanted.role)
            .ok_or(SeedSkipReason::UnitMissing)?;
        for file in &unit.files {
            if !seen_paths.insert(file.path.as_str()) {
                return Err(SeedSkipReason::PlanCap);
            }
            files.push(file.clone());
            if files.len() > SEED_MAX_PLAN_FILES {
                return Err(SeedSkipReason::PlanCap);
            }
        }
    }
    let logical_bytes = files
        .iter()
        .try_fold(0u64, |total, file| total.checked_add(file.size))
        .ok_or(SeedSkipReason::PlanCap)?;
    if logical_bytes > SEED_MAX_PLAN_BYTES {
        return Err(SeedSkipReason::PlanCap);
    }
    Ok(Plan {
        units: units.len(),
        files,
        logical_bytes,
    })
}

fn validate_recipient(recipient: &SeedRecipient) -> Result<(), LeaseError> {
    validate_slot_id(&recipient.slot_id)?;
    validate_generation(&recipient.incarnation, "incarnation")?;
    validate_generation(&recipient.lease_id, "lease id")?;
    Ok(())
}

fn validate_request(request: &SeedRequest) -> Result<(), LeaseError> {
    validate_recipient(&request.recipient)?;
    validate_slot_id(&request.donor.slot_id)?;
    validate_generation(&request.donor.incarnation, "incarnation")?;
    validate_generation(&request.donor.lease_id, "lease id")?;
    if request.donor.catalog_digest.len() != 64
        || !request
            .donor
            .catalog_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LeaseError::InvalidInput(
            "seed catalog digest is not a lowercase SHA-256",
        ));
    }
    for unit in &request.units {
        let valid = !unit.fingerprint_dir.is_empty()
            && unit.fingerprint_dir.len() <= MAX_PATH_BYTES
            && !unit.fingerprint_dir.starts_with('/')
            && unit
                .fingerprint_dir
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..");
        if !valid {
            return Err(LeaseError::InvalidInput(
                "seed plan names an unusable unit directory",
            ));
        }
    }
    if !request.donor_record_lock.is_absolute() {
        return Err(LeaseError::InvalidInput(
            "the donor record lock path must be absolute",
        ));
    }
    Ok(())
}

fn validate_slot_id(text: &str) -> Result<(), LeaseError> {
    match text.parse::<u64>() {
        Ok(id) if id != 0 && text.bytes().all(|byte| byte.is_ascii_digit()) => Ok(()),
        _ => Err(LeaseError::InvalidInput(
            "slot ID must be a positive integer",
        )),
    }
}

fn validate_generation(text: &str, what: &'static str) -> Result<(), LeaseError> {
    if text.is_empty() || text.len() > MAX_LEASE_ID_BYTES || !text.is_ascii() {
        return Err(LeaseError::InvalidInput(match what {
            "lease id" => "lease id is empty, over-long or not ASCII",
            _ => "incarnation is empty, over-long or not ASCII",
        }));
    }
    Ok(())
}

enum DonorLock {
    Taken(File),
    Missing,
    Held,
}

/// The donor's front-door record lock, taken non-blocking with
/// `create: false`. A missing or held lock is a skip, never a wait.
fn probe_donor_lock(path: &Path) -> Result<DonorLock, LeaseError> {
    if path.as_os_str().as_bytes().len() > MAX_PATH_BYTES {
        return Err(LeaseError::InvalidInput(
            "the donor record lock path is too long",
        ));
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DonorLock::Missing)
        }
        Err(error) => return Err(io_error(error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
    {
        return Err(LeaseError::UnsafePath(
            "the donor record lock is not an owned regular file",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    // SAFETY: the descriptor is live for the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(DonorLock::Taken(file));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(DonorLock::Held);
    }
    Err(io_error(error))
}

fn catalog_path(pool_root: &Path, slot_id: u64) -> PathBuf {
    pool_root
        .join(MANAGED_NAME)
        .join(SEEDS_DIR)
        .join(format!("{slot_id}{CATALOG_SUFFIX}"))
}

/// Read one catalog sidecar. A missing, oversized, malformed, symlinked,
/// foreign-owned, unknown-version or generation-mismatched file is ignored:
/// the caller treats the donor as offering nothing.
fn read_catalog(pool_root: &Path, slot_id: u64) -> Option<(SeedCatalog, Vec<u8>, String)> {
    let path = catalog_path(pool_root, slot_id);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > SEED_CATALOG_MAX_BYTES as u64
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(&path)
        .ok()?
        .take(SEED_CATALOG_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > SEED_CATALOG_MAX_BYTES {
        return None;
    }
    let catalog = SeedCatalog::parse(&bytes).ok()?;
    let digest = catalog_digest(&bytes);
    Some((catalog, bytes, digest))
}

/// Remove at most eight sidecars whose slot is no longer in the ledger.
fn remove_orphan_sidecars(dir: &Path, view: &SeedPoolView) -> Result<(), LeaseError> {
    let mut removed = 0usize;
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Err(io_error(error)),
    };
    for entry in entries {
        if removed >= SEED_ORPHAN_CLEANUP_MAX {
            break;
        }
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        let Some(id) = sidecar_slot_id(text) else {
            continue;
        };
        if view.slot(&id.to_string()).is_some() {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_file()
            && metadata.uid() == current_uid()
            && fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    Ok(())
}

const ATTEMPT_SUFFIX: &str = ".attempt.json";
const OBSERVED_SUFFIX: &str = ".observed.json";

fn sidecar_slot_id(name: &str) -> Option<u64> {
    for suffix in [CATALOG_SUFFIX, ATTEMPT_SUFFIX, OBSERVED_SUFFIX] {
        if let Some(id) = name.strip_suffix(suffix) {
            if id.bytes().all(|byte| byte.is_ascii_digit()) {
                return id.parse().ok().filter(|id| *id != 0);
            }
        }
    }
    None
}

/// Keep the whole sidecar family inside its byte ceiling: drop up to eight of
/// the least recently produced catalogs (never this slot's) and refuse the
/// write if it still would not fit.
fn enforce_family_bytes(dir: &Path, adding: u64, keep_slot: &str) -> Result<(), LeaseError> {
    let cap = family_cap();
    let mut total = sidecar_bytes(dir)?;
    if total.saturating_add(adding) <= cap {
        return Ok(());
    }
    let mut catalogs: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(text) = name.to_str() else { continue };
        let Some(id) = sidecar_slot_id(text) else {
            continue;
        };
        if id.to_string() == keep_slot || !text.ends_with(CATALOG_SUFFIX) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.file_type().is_file() || metadata.uid() != current_uid() {
            continue;
        }
        let produced = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        catalogs.push((produced, metadata.len(), entry.path()));
    }
    catalogs.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(&right.2)));
    // Plan the drops first: a write that cannot fit even after dropping the
    // oldest catalogs must not destroy any of them.
    let mut planned = Vec::new();
    for (_, len, path) in catalogs {
        if total.saturating_add(adding) <= cap || planned.len() >= SEED_FAMILY_DROP_MAX {
            break;
        }
        total = total.saturating_sub(len);
        planned.push(path);
    }
    if total.saturating_add(adding) > cap {
        return Err(LeaseError::LedgerExhausted);
    }
    for path in planned {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn family_cap() -> u64 {
    #[cfg(test)]
    if let Some(cap) = tests::family_cap_override() {
        return cap;
    }
    SEED_SIDECAR_FAMILY_MAX_BYTES
}

fn sidecar_bytes(dir: &Path) -> Result<u64, LeaseError> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

/// Atomic owner-only sidecar write: temp, fsync, rename, fsync the directory.
fn write_sidecar(path: &Path, bytes: &[u8]) -> Result<(), LeaseError> {
    if bytes.len() > SEED_CATALOG_MAX_BYTES {
        return Err(LeaseError::LedgerExhausted);
    }
    let parent = path
        .parent()
        .ok_or(LeaseError::InvalidInput("sidecar has no parent"))?;
    let temp = parent.join(format!("catalog.tmp-{}", random_hex(8)?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp).map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

fn open_owned_directory(path: &Path, device: u64) -> Result<OwnedFd, LeaseError> {
    let fd = open_path_directory(path).map_err(io_error)?;
    let metadata = fstat(fd.as_fd()).map_err(io_error)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_uid != current_uid()
        || metadata.st_dev as u64 != device
    {
        return Err(LeaseError::UnsafePath(
            "a slot is not an owned directory on the pool's device",
        ));
    }
    Ok(fd)
}

fn open_path_directory(path: &Path) -> std::io::Result<OwnedFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("slot path holds a NUL"))?;
    // SAFETY: the path is a live NUL-terminated string.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_optional_directory(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<Option<OwnedFd>, LeaseError> {
    match open_directory_at(parent, name) {
        Ok(fd) => Ok(Some(require_owned_directory(fd, device)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn require_owned_directory(fd: OwnedFd, device: u64) -> Result<OwnedFd, LeaseError> {
    let metadata = fstat(fd.as_fd()).map_err(io_error)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_uid != current_uid()
        || metadata.st_dev as u64 != device
    {
        return Err(LeaseError::UnsafePath(
            "a slot directory is not an owned directory on the pool's device",
        ));
    }
    Ok(fd)
}

/// The recipient's `build/`: absent, or a real empty owned directory. A
/// non-empty or foreign one refuses; it is never cleaned or reused.
fn open_recipient_build(slot: BorrowedFd<'_>, device: u64) -> Result<Option<OwnedFd>, LeaseError> {
    let Some(build) = open_optional_directory(slot, c"build", device)? else {
        return Ok(None);
    };
    if !walk::directory_is_empty(build.as_fd()).map_err(io_error)? {
        return Err(LeaseError::UnsafePath(
            "the new slot's build directory is not empty",
        ));
    }
    Ok(Some(build))
}

fn create_owned_directory(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<OwnedFd, LeaseError> {
    match open_optional_directory(parent, name, device)? {
        Some(fd) => Ok(fd),
        None => match mkdir_owned(parent, name) {
            Ok(()) => match open_optional_directory(parent, name, device)? {
                Some(fd) => Ok(fd),
                None => Err(LeaseError::Io(
                    "a created seed directory is missing".to_owned(),
                )),
            },
            Err(error) => Err(error),
        },
    }
}

fn open_or_create_directory(
    parent: BorrowedFd<'_>,
    name: &std::ffi::CStr,
    device: u64,
) -> Result<OwnedFd, LeaseError> {
    match open_optional_directory(parent, name, device) {
        Ok(Some(fd)) => Ok(fd),
        Ok(None) => {
            mkdir_owned(parent, name)?;
            match open_optional_directory(parent, name, device)? {
                Some(fd) => Ok(fd),
                None => Err(LeaseError::Io(
                    "a created seed directory is missing".to_owned(),
                )),
            }
        }
        Err(error) => Err(error),
    }
}

fn mkdir_owned(parent: BorrowedFd<'_>, name: &std::ffi::CStr) -> Result<(), LeaseError> {
    // SAFETY: the descriptor and the name are live for the call.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0 {
        Ok(())
    } else {
        Err(io_error(std::io::Error::last_os_error()))
    }
}

struct OpenedFile {
    fd: OwnedFd,
    metadata: libc::stat,
}

/// Open `relative` below `root` without following any link, requiring every
/// intermediate component to be a directory and the leaf a regular file on
/// `device`.
fn open_relative_file(
    root: BorrowedFd<'_>,
    relative: &str,
    device: u64,
) -> Result<OpenedFile, LeaseError> {
    let (parents, name) = split_relative(relative);
    let mut deepest: Option<OwnedFd> = None;
    for component in parents {
        let parent_fd = match deepest.as_ref() {
            Some(fd) => fd.as_fd(),
            None => root,
        };
        let next = open_directory_at(parent_fd, &cstring(component)?).map_err(io_error)?;
        deepest = Some(require_owned_directory(next, device)?);
    }
    let directory = match deepest.as_ref() {
        Some(fd) => fd.as_fd(),
        None => root,
    };
    let name = cstring(name)?;
    let fd = walk::open_regular(&directory, &name).map_err(io_error)?;
    let metadata = fstat(fd.as_fd()).map_err(io_error)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_uid != current_uid()
        || metadata.st_dev as u64 != device
    {
        return Err(LeaseError::UnsafePath(
            "a donor file is not an owned regular file on the pool's device",
        ));
    }
    Ok(OpenedFile { fd, metadata })
}

fn split_relative(relative: &str) -> (Vec<&str>, &str) {
    let mut parts: Vec<&str> = relative.split('/').collect();
    let name = parts.pop().unwrap_or(relative);
    (parts, name)
}

fn cstring(text: &str) -> Result<CString, LeaseError> {
    CString::new(text.as_bytes())
        .map_err(|_| LeaseError::InvalidInput("a catalog path holds a NUL"))
}

fn fstat(fd: BorrowedFd<'_>) -> std::io::Result<libc::stat> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is live and `stat` is valid for writes.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stat)
}

fn hash_fd(fd: BorrowedFd<'_>) -> Result<String, LeaseError> {
    let mut file = File::from(fd.try_clone_to_owned().map_err(io_error)?);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(catalog::hex(&hasher.finalize()))
}

/// `clonefile` only: there is no byte-copy, hard-link or shell fallback. A
/// platform without it never seeds (the caller skips before any effect).
fn clone_file(
    source: BorrowedFd<'_>,
    destination: BorrowedFd<'_>,
    name: &std::ffi::CStr,
) -> Result<(), LeaseError> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: both descriptors and the name are live for the call, and
        // `fclonefileat` creates a new entry rather than following one.
        let result = unsafe {
            libc::fclonefileat(
                source.as_raw_fd(),
                destination.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io_error(std::io::Error::last_os_error()))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (source, destination, name);
        Err(LeaseError::Unsupported(
            "clonefile is unavailable on this platform",
        ))
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

/// Test-only fault-injection points. A production build calls this with the
/// same shape and does nothing: no environment variable, flag or hook reaches
/// a real pool.
fn test_point(_point: TestPoint<'_>) {
    #[cfg(test)]
    tests::inject(_point);
}

#[allow(dead_code)]
enum TestPoint<'a> {
    /// Before any donor file is verified, after the plan exists.
    BeforeVerify { pool: &'a Path },
    /// Before cloning one planned file, by plan index.
    BeforeClone {
        index: usize,
        recipient_slot: &'a Path,
        relative: &'a str,
    },
    /// After one clone was created, before it is verified.
    AfterClone { index: usize, path: &'a Path },
    /// After every clone, before the recipient walk.
    BeforeWalk { slot: &'a Path },
    /// After the walk, before the final ledger recheck.
    BeforeFinalDonorCheck { pool: &'a Path },
}
