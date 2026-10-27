//! Tiny owned fixtures for the seed custody calls (`oxide#137` §15 slice 2).
//!
//! No Cargo, no image and no volume: a budget-only directory pool and
//! hand-written catalogs stand in for the front door. Each test builds only
//! its own unique directory below the recipe's test temp and removes it.

use super::super::pool::{
    create_injected, lease_injected, release_injected, tamper_idle_generations_injected,
};
use super::super::{LeaseError, LeaseGrant, LeaseRequest, Profile};
use super::*;
use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const FINGERPRINT: &str = "debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa";
const DEP: &str = "debug/deps/cfg_if-aaaaaaaaaaaaaaaa.d";
const ARTIFACT: &str = "debug/deps/libcfg_if-aaaaaaaaaaaaaaaa.rmeta";

thread_local! {
    static WALK_LIMITS: Cell<Option<SeedWalkLimits>> = const { Cell::new(None) };
    static FAMILY_CAP: Cell<Option<u64>> = const { Cell::new(None) };
    static TAMPER_CLONE: Cell<Option<usize>> = const { Cell::new(None) };
    static BREAK_DONOR: Cell<Option<usize>> = const { Cell::new(None) };
    static PLANT_SYMLINK: Cell<bool> = const { Cell::new(false) };
    static TAMPER_LEDGER: Cell<bool> = const { Cell::new(false) };
    static VERIFY_DELAY_MS: Cell<Option<u64>> = const { Cell::new(None) };
}

pub(super) fn walk_limits_override() -> Option<SeedWalkLimits> {
    WALK_LIMITS.with(Cell::get)
}

pub(super) fn family_cap_override() -> Option<u64> {
    FAMILY_CAP.with(Cell::get)
}

fn reset_hooks() {
    WALK_LIMITS.with(|cell| cell.set(None));
    FAMILY_CAP.with(|cell| cell.set(None));
    TAMPER_CLONE.with(|cell| cell.set(None));
    BREAK_DONOR.with(|cell| cell.set(None));
    PLANT_SYMLINK.with(|cell| cell.set(false));
    TAMPER_LEDGER.with(|cell| cell.set(false));
    VERIFY_DELAY_MS.with(|cell| cell.set(None));
}

pub(super) fn inject(point: TestPoint<'_>) {
    match point {
        TestPoint::BeforeVerify { .. } => {
            if let Some(ms) = VERIFY_DELAY_MS.with(Cell::get) {
                std::thread::sleep(Duration::from_millis(ms));
            }
        }
        TestPoint::BeforeClone {
            index,
            recipient_slot,
            relative,
        } => {
            if BREAK_DONOR.with(Cell::get) == Some(index) {
                // A pre-existing entry makes `clonefile` fail: the layer has
                // no byte-copy, hard-link or overwrite fallback.
                let _ = fs::create_dir(recipient_slot.join("build").join(relative));
            }
        }
        TestPoint::AfterClone { index, path } => {
            if TAMPER_CLONE.with(Cell::get) == Some(index) {
                let mut file = OpenOptions::new()
                    .append(true)
                    .open(path)
                    .expect("open tampered clone");
                file.write_all(b"tampered").expect("tamper clone");
            }
        }
        TestPoint::BeforeWalk { slot } => {
            if PLANT_SYMLINK.with(Cell::get) {
                symlink("missing-target", slot.join("foreign-link")).expect("plant symlink");
            }
        }
        TestPoint::BeforeFinalDonorCheck { pool } => {
            if TAMPER_LEDGER.with(Cell::get) {
                tamper_idle_generations_injected(pool).expect("tamper ledger");
            }
        }
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let parent = std::env::temp_dir().join("lease-seed");
        fs::create_dir_all(&parent).expect("fixture parent");
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{label}-{:x}-{id:x}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                        .expect("private fixture");
                    return Self { path };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create seed fixture: {error}"),
            }
        }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir_all(&path).expect("fixture dir");
        path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct Rig {
    fx: Fixture,
    pool: PathBuf,
}

impl Rig {
    fn new(label: &str) -> Self {
        let fx = Fixture::new(label);
        let pool = fx.dir("pool");
        create_injected(&pool, 4 * 1024 * 1024).expect("init pool");
        Self { fx, pool }
    }

    fn lease(&self, label: &str, holder: &str) -> LeaseGrant {
        let project = self.fx.dir(&format!("project-{label}"));
        let workspace = self.fx.dir(&format!("workspace-{label}"));
        lease_injected(LeaseRequest {
            pool: self.pool.clone(),
            project,
            workspace,
            profile: Profile::Dev,
            holder: holder.to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout: None,
            replace_slot: None,
        })
        .expect("lease")
    }

    fn record_lock(&self, slot_id: &str) -> PathBuf {
        let locks = self.fx.path.join("locks");
        if !locks.exists() {
            fs::create_dir(&locks).expect("locks dir");
            fs::set_permissions(&locks, fs::Permissions::from_mode(0o700)).expect("locks mode");
        }
        let path = locks.join(format!("{slot_id}.lock"));
        if !path.exists() {
            fs::write(&path, b"").expect("record lock");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("lock mode");
        }
        path
    }

    fn sidecars(&self) -> Vec<String> {
        let dir = self.pool.join(MANAGED_NAME).join("seeds");
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn ledger_bytes(&self) -> Vec<u8> {
        fs::read(self.pool.join("ledger.json")).expect("ledger bytes")
    }
}

struct Donor {
    grant: LeaseGrant,
    catalog: SeedCatalog,
    digest: String,
    files: Vec<(String, String)>,
}

impl Donor {
    fn lock_path(&self, rig: &Rig) -> PathBuf {
        rig.record_lock(&self.grant.slot_id)
    }

    fn request(&self, rig: &Rig, recipient: &SeedRecipient) -> SeedRequest {
        SeedRequest {
            recipient: recipient.clone(),
            donor: SeedDonor {
                slot_id: self.grant.slot_id.clone(),
                incarnation: self.grant.incarnation.clone(),
                lease_id: self.grant.lease_id.clone(),
                catalog_digest: self.digest.clone(),
            },
            units: vec![SeedUnitRef {
                fingerprint_dir: FINGERPRINT.to_owned(),
                role: SeedRole::Lib,
            }],
            donor_record_lock: self.lock_path(rig),
            work_budget_ms: None,
        }
    }
}

fn sha(text: &str) -> String {
    catalog_digest(text.as_bytes())
}

fn default_files() -> Vec<(String, String)> {
    vec![
        (
            format!("{FINGERPRINT}/lib-cfg_if"),
            "fingerprint".to_owned(),
        ),
        (format!("{FINGERPRINT}/lib-cfg_if.json"), "{}".to_owned()),
        (format!("{FINGERPRINT}/dep-lib-cfg_if"), String::new()),
        (ARTIFACT.to_owned(), "rmeta-fixture".to_owned()),
        (
            DEP.to_owned(),
            format!("{DEP}: /fixture/src/lib.rs\n/fixture/src/lib.rs:\n"),
        ),
    ]
}

fn write_files(build: &Path, files: &[(String, String)]) {
    for (relative, content) in files {
        let path = build.join(relative);
        fs::create_dir_all(path.parent().expect("file parent")).expect("file dir");
        fs::write(&path, content).expect("fixture file");
    }
}

fn producer() -> SeedProducer {
    SeedProducer {
        cargo_version: "1.97.1".to_owned(),
        rustc_digest: "c".repeat(64),
        rustc: "/fixture/rustc".to_owned(),
        target: "aarch64-apple-darwin".to_owned(),
        recipe: "quick".to_owned(),
        backend: "directory-budget-v1".to_owned(),
        adapter: "cargo-1.97.1/c980f4866/build-dir-v1".to_owned(),
        context_digest: "d".repeat(64),
        config_files: vec!["/fixture/.cargo/config.toml".to_owned()],
        flags_digest: "e".repeat(64),
        cargo_home: "/fixture/cargo-home".to_owned(),
        command_family: "build".to_owned(),
        command_class: "dependency".to_owned(),
    }
}

fn catalog_for(grant: &LeaseGrant, files: &[(String, String)]) -> SeedCatalog {
    let entries: Vec<SeedFile> = files
        .iter()
        .map(|(path, content)| SeedFile {
            path: path.clone(),
            size: content.len() as u64,
            mtime_ms: 0,
            digest: sha(content),
        })
        .collect();
    let dep = files
        .iter()
        .find(|(path, _)| path == DEP)
        .expect("dep-info fixture");
    SeedCatalog {
        schema: SEED_CATALOG_SCHEMA.to_owned(),
        store_id: grant.store_id.clone(),
        slot_id: grant.slot_id.clone(),
        incarnation: grant.incarnation.clone(),
        lease_id: grant.lease_id.clone(),
        record_id: 1,
        mark: 1,
        started_at_ms: 1,
        finished_at_ms: 2,
        attempt: SeedAttemptState::None,
        producer: producer(),
        packages: vec![SeedPackage {
            name: "cfg-if".to_owned(),
            target: "cfg_if".to_owned(),
            version: "1.0.0".to_owned(),
            source: "registry+https://github.com/rust-lang/crates.io-index".to_owned(),
            checksum: Some("a".repeat(64)),
            unique: true,
            leaf: true,
            source_root: SeedSourceRoot {
                kind: SeedSourceKind::Registry,
                digest: "b".repeat(64),
            },
            units: vec![SeedUnit {
                fingerprint_dir: FINGERPRINT.to_owned(),
                role: SeedRole::Lib,
                target: "cfg_if".to_owned(),
                join: SeedJoin {
                    path: DEP.to_owned(),
                    targets: vec![DEP.to_owned()],
                    inputs: vec!["/fixture/src/lib.rs".to_owned()],
                    digest: sha(&dep.1),
                },
                files: entries,
            }],
        }],
        rejected: Vec::new(),
    }
}

/// One released donor slot with a published catalog.
fn make_donor(rig: &Rig, label: &str) -> Donor {
    let grant = rig.lease(&format!("donor-{label}"), "donor");
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let catalog = catalog_for(&grant, &files);
    publish_seed_catalog(&rig.pool, &catalog).expect("publish catalog");
    let bytes = catalog.to_bytes().expect("catalog bytes");
    let digest = catalog_digest(&bytes);
    release_injected(&rig.pool, &grant.lease_id).expect("release donor");
    Donor {
        grant,
        catalog,
        digest,
        files,
    }
}

fn lease_recipient(rig: &Rig, label: &str) -> (LeaseGrant, SeedRecipient) {
    let grant = rig.lease(&format!("recipient-{label}"), "recipient");
    let recipient = SeedRecipient {
        slot_id: grant.slot_id.clone(),
        incarnation: grant.incarnation.clone(),
        lease_id: grant.lease_id.clone(),
    };
    (grant, recipient)
}

fn candidates(rig: &Rig, recipient: &SeedRecipient) -> SeedCandidates {
    seed_candidates(&rig.pool, recipient, 8).expect("candidates")
}

fn tree(root: &Path) -> Vec<(String, u64, Vec<u8>)> {
    let mut found = Vec::new();
    collect(root, root, &mut found);
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

fn collect(root: &Path, dir: &Path, found: &mut Vec<(String, u64, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(root)
            .expect("under root")
            .to_string_lossy()
            .into_owned();
        if metadata.file_type().is_dir() {
            found.push((format!("{relative}/"), metadata.ino(), Vec::new()));
            collect(root, &path, found);
        } else {
            found.push((
                relative,
                metadata.ino(),
                fs::read(&path).unwrap_or_default(),
            ));
        }
    }
}

#[test]
fn candidates_require_the_held_recipient_and_offer_catalogs_by_rank() {
    reset_hooks();
    let rig = Rig::new("candidates");
    let _donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let found = candidates(&rig, &recipient);
    assert_eq!(found.candidates.len(), 1);
    let candidate = &found.candidates[0];
    assert_eq!(candidate.slot_id, _donor.grant.slot_id);
    assert_eq!(candidate.catalog_digest, _donor.digest);
    assert_eq!(candidate.catalog, _donor.catalog);
    assert!(candidate.rank > 0);

    // A released recipient generation is not the lease call 1 was told about.
    release_injected(&rig.pool, &grant.lease_id).expect("release recipient");
    let stale = seed_candidates(&rig.pool, &recipient, 8);
    assert!(matches!(stale, Err(LeaseError::UnknownAccounting)));

    // A live recipient whose generation differs is refused too.
    let (_, fresh) = lease_recipient(&rig, "b");
    let mismatched = SeedRecipient {
        lease_id: "not-the-lease".to_owned(),
        ..fresh
    };
    let refused = seed_candidates(&rig.pool, &mismatched, 8);
    assert!(matches!(refused, Err(LeaseError::UnknownAccounting)));
}

#[test]
fn candidates_ignore_a_catalog_whose_generation_moved_on() {
    reset_hooks();
    let rig = Rig::new("candidates-generation");
    let donor = make_donor(&rig, "a");
    // The slot is granted again, which mints a new lease id and invalidates
    // the catalog atomically (§4.6).
    let again = rig.lease("donor-a", "donor");
    assert_eq!(again.slot_id, donor.grant.slot_id);
    release_injected(&rig.pool, &again.lease_id).expect("release again");
    let (_, recipient) = lease_recipient(&rig, "a");
    assert!(candidates(&rig, &recipient).candidates.is_empty());
}

#[test]
fn seed_clones_catalogued_units_and_records_a_complete_observation() {
    reset_hooks();
    let rig = Rig::new("seeded");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_donor = tree(&donor.grant.env.cargo_build_build_dir);
    let before_sidecars = rig.sidecars();
    let before_ledger = rig.ledger_bytes();
    let outcome = seed(&rig.pool, &donor.request(&rig, &recipient)).expect("seed");
    let SeedOutcome::Seeded(receipt) = outcome else {
        panic!("expected a published seed, got {outcome:?}");
    };
    assert_eq!(receipt.donor_slot, donor.grant.slot_id);
    assert_eq!(receipt.units, 1);
    assert_eq!(receipt.files, donor.files.len() as u64);
    assert_eq!(
        receipt.logical_bytes,
        donor
            .files
            .iter()
            .map(|(_, content)| content.len() as u64)
            .sum::<u64>()
    );
    assert_eq!(receipt.catalog_digest, donor.digest);
    assert!(receipt.observation.complete());
    assert_eq!(receipt.observation.slot_id, grant.slot_id);
    assert_eq!(receipt.observation.lease_id, recipient.lease_id);
    assert!(receipt.observation.entries >= donor.files.len() as u64);
    // Every offered file is a fresh owned clone with the donor's bytes and no
    // shared inode, and the donor's own tree is byte- and identity-identical.
    let build = &grant.env.cargo_build_build_dir;
    for (relative, content) in &donor.files {
        let cloned = fs::read(build.join(relative)).expect("cloned file");
        assert_eq!(&cloned, content.as_bytes());
        let cloned_metadata = fs::symlink_metadata(build.join(relative)).expect("clone metadata");
        let donor_metadata =
            fs::symlink_metadata(donor.grant.env.cargo_build_build_dir.join(relative))
                .expect("donor metadata");
        assert_ne!(cloned_metadata.ino(), donor_metadata.ino());
        assert_eq!(cloned_metadata.nlink(), 1);
        assert_eq!(cloned_metadata.uid(), donor_metadata.uid());
    }
    assert_eq!(tree(&donor.grant.env.cargo_build_build_dir), before_donor);
    // The library publishes nothing: catalog, attempt and observation sidecars
    // are all the producing window's, and the ledger records no seed fact.
    assert_eq!(rig.sidecars(), before_sidecars);
    assert_eq!(rig.ledger_bytes(), before_ledger);
}

#[test]
fn seed_skips_a_changed_catalog_without_touching_the_recipient() {
    reset_hooks();
    let rig = Rig::new("catalog-changed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let request = donor.request(&rig, &recipient);
    let _ = request;
    // Rewrite the sidecar with a different (still valid) catalog.
    let mut changed = donor.catalog.clone();
    changed.mark = 2;
    let path = rig
        .pool
        .join(MANAGED_NAME)
        .join("seeds")
        .join(format!("{}.catalog.json", donor.grant.slot_id));
    fs::write(&path, changed.to_bytes().expect("changed bytes")).expect("rewrite catalog");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("catalog mode");
    let outcome = seed(&rig.pool, &donor.request(&rig, &recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::CatalogChanged
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_donor_that_left_idle_before_the_first_clone() {
    reset_hooks();
    let rig = Rig::new("donor-busy");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let request = donor.request(&rig, &recipient);
    // Take the donor slot again: its ledger state is no longer `Idle`.
    let busy = rig.lease("donor-a", "donor");
    assert_eq!(busy.slot_id, donor.grant.slot_id);
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorBusy
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_changed_donor_file_without_touching_the_recipient() {
    reset_hooks();
    let rig = Rig::new("file-changed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let request = donor.request(&rig, &recipient);
    // Same size, same mtime is not required: the digest is authoritative.
    let artifact = donor.grant.env.cargo_build_build_dir.join(ARTIFACT);
    fs::write(&artifact, b"rmeta-FIXTURE").expect("rewrite donor file");
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::FileChanged
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_missing_or_held_donor_record_lock() {
    reset_hooks();
    let rig = Rig::new("donor-lock");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&rig, &recipient);
    request.donor_record_lock = rig.fx.path.join("locks").join("missing.lock");
    let missing = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        missing,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorLockMissing
        })
    );
    // A peer's record lock means the donor is skipped, never waited for.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(donor.lock_path(&rig))
        .expect("open record lock");
    // SAFETY: the descriptor is live for the call.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let held = seed(&rig.pool, &donor.request(&rig, &recipient)).expect("seed");
    assert_eq!(
        held,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorBusy
        })
    );
}

#[test]
fn seed_skips_a_plan_naming_a_unit_the_catalog_lacks() {
    reset_hooks();
    let rig = Rig::new("unit-missing");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&rig, &recipient);
    request.units = vec![SeedUnitRef {
        fingerprint_dir: "debug/.fingerprint/cfg-if-bbbbbbbbbbbbbbbb".to_owned(),
        role: SeedRole::Lib,
    }];
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::UnitMissing
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn plan_caps_before_the_first_clone() {
    reset_hooks();
    let rig = Rig::new("plan-caps");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // The unit cap: one selector more than the plan may hold.
    let mut request = donor.request(&rig, &recipient);
    request.units = (0..=SEED_MAX_PLAN_UNITS)
        .map(|index| SeedUnitRef {
            fingerprint_dir: format!("debug/.fingerprint/cfg-if-{index:016x}"),
            role: SeedRole::Lib,
        })
        .collect();
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::PlanCap
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
    // The file and byte caps are checked on catalog data alone.
    let mut oversized = donor.catalog.clone();
    oversized.packages[0].units[0].files[0].size = SEED_MAX_PLAN_BYTES + 1;
    let mut request = donor.request(&rig, &recipient);
    request.units = vec![SeedUnitRef {
        fingerprint_dir: FINGERPRINT.to_owned(),
        role: SeedRole::Lib,
    }];
    assert!(matches!(
        plan_units(&oversized, &request.units),
        Err(SeedSkipReason::PlanCap)
    ));
}

#[test]
fn seed_skips_on_the_work_deadline_before_the_first_write() {
    reset_hooks();
    let rig = Rig::new("deadline");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&rig, &recipient);
    request.work_budget_ms = Some(1);
    VERIFY_DELAY_MS.with(|cell| cell.set(Some(25)));
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::WorkDeadline
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_refuses_a_nonempty_recipient_build() {
    reset_hooks();
    let rig = Rig::new("nonempty-build");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let build = &grant.env.cargo_build_build_dir;
    fs::create_dir_all(build).expect("recipient build");
    fs::write(build.join("stray"), b"x").expect("stray entry");
    let refused = seed(&rig.pool, &donor.request(&rig, &recipient));
    assert!(matches!(refused, Err(LeaseError::UnsafePath(_))));
    assert!(build.join("stray").exists());
}

#[test]
fn seed_refuses_a_clone_whose_bytes_do_not_match_and_retains_the_partial_slot() {
    reset_hooks();
    let rig = Rig::new("clone-digest");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    let before_ledger = rig.ledger_bytes();
    TAMPER_CLONE.with(|cell| cell.set(Some(1)));
    let refused = seed(&rig.pool, &donor.request(&rig, &recipient));
    assert!(matches!(refused, Err(LeaseError::UnsafePath(_))));
    // Everything already cloned is retained: nothing is deleted or repaired,
    // and nothing is published.
    let build = &grant.env.cargo_build_build_dir;
    assert!(build.join(&donor.files[0].0).exists());
    assert!(build.join(&donor.files[1].0).exists());
    assert_eq!(rig.sidecars(), before_sidecars);
    assert_eq!(rig.ledger_bytes(), before_ledger);
    // The slot is still the command's held lease: the front door releases it
    // untrusted, and the ordinary journaled eviction removes it later.
    let released = release_injected(&rig.pool, &grant.lease_id).expect("release untrusted");
    assert_eq!(released.state, super::super::SlotOccupancy::Idle);
}

#[test]
fn seed_refuses_a_failed_clone_and_never_copies_bytes() {
    reset_hooks();
    let rig = Rig::new("clone-failed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // The donor file disappears after it was verified and before its clone, so
    // the clone alone fails. There is no byte-copy fallback.
    BREAK_DONOR.with(|cell| cell.set(Some(0)));
    let refused = seed(&rig.pool, &donor.request(&rig, &recipient));
    assert!(matches!(refused, Err(LeaseError::Io(_))));
    let destination = grant.env.cargo_build_build_dir.join(&donor.files[0].0);
    assert!(fs::symlink_metadata(&destination)
        .expect("planted entry")
        .file_type()
        .is_dir());
}

#[test]
fn seed_abandons_on_a_failed_recipient_walk_and_publishes_nothing() {
    reset_hooks();
    let rig = Rig::new("walk-failed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    PLANT_SYMLINK.with(|cell| cell.set(true));
    let outcome = seed(&rig.pool, &donor.request(&rig, &recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::UnexpectedType));
    assert!(!abandon.observation.complete());
    assert_eq!(abandon.observation.slot_id, grant.slot_id);
    // The clone is retained, untrusted, and nothing was published.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
    assert_eq!(rig.sidecars(), before_sidecars);
}

#[test]
fn seed_abandons_on_an_entry_limit_walk() {
    reset_hooks();
    let rig = Rig::new("walk-limit");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    WALK_LIMITS.with(|cell| {
        cell.set(Some(SeedWalkLimits {
            max_entries: 1,
            max_depth: SEED_WALK_MAX_DEPTH,
            max_elapsed_ms: SEED_WALK_MAX_ELAPSED_MS,
        }))
    });
    let outcome = seed(&rig.pool, &donor.request(&rig, &recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::EntryLimit));
    assert_eq!(abandon.observation.limits.max_entries, 1);
}

#[test]
fn seed_refuses_a_donor_generation_that_changed_after_the_clone() {
    reset_hooks();
    let rig = Rig::new("generation-changed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    TAMPER_LEDGER.with(|cell| cell.set(true));
    let refused = seed(&rig.pool, &donor.request(&rig, &recipient));
    assert!(matches!(refused, Err(LeaseError::UnknownAccounting)));
    // The clone is retained untrusted, and the slot is still held.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn publish_requires_the_producing_lease_and_a_bounded_catalog() {
    reset_hooks();
    let rig = Rig::new("publish");
    let donor = make_donor(&rig, "a");
    let stale = publish_seed_catalog(&rig.pool, &donor.catalog);
    assert!(matches!(stale, Err(LeaseError::UnknownAccounting)));
    let (grant, _) = lease_recipient(&rig, "a");
    let mut unknown_slot = donor.catalog.clone();
    unknown_slot.slot_id = "999".to_owned();
    unknown_slot.incarnation = grant.incarnation.clone();
    unknown_slot.lease_id = grant.lease_id.clone();
    let missing_slot = publish_seed_catalog(&rig.pool, &unknown_slot);
    assert!(matches!(missing_slot, Err(LeaseError::UnknownAccounting)));
}

#[test]
fn publish_removes_orphan_sidecars_and_respects_the_family_ceiling() {
    reset_hooks();
    let rig = Rig::new("publish-family");
    let first = make_donor(&rig, "a");
    let second = make_donor(&rig, "b");
    let seeds = rig.pool.join(MANAGED_NAME).join("seeds");
    // Sidecars of slot ids the ledger no longer holds are bounded cleanup.
    fs::write(seeds.join("999.catalog.json"), b"{}").expect("orphan");
    fs::set_permissions(
        seeds.join("999.catalog.json"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("orphan mode");
    let (_, recipient) = lease_recipient(&rig, "a");
    let third = make_donor(&rig, "c");
    assert!(!seeds.join("999.catalog.json").exists());
    assert!(seeds
        .join(format!("{}.catalog.json", third.grant.slot_id))
        .exists());

    // A family ceiling that only one catalog can meet drops the oldest
    // catalog and refuses nothing.
    let one_catalog = fs::metadata(seeds.join(format!("{}.catalog.json", third.grant.slot_id)))
        .expect("third catalog")
        .len();
    FAMILY_CAP.with(|cell| cell.set(Some(one_catalog + 8)));
    let fourth = make_donor(&rig, "d");
    assert!(seeds
        .join(format!("{}.catalog.json", fourth.grant.slot_id))
        .exists());
    assert!(!seeds
        .join(format!("{}.catalog.json", third.grant.slot_id))
        .exists());
    // A ceiling the new catalog cannot meet refuses the write, and the pool's
    // seed sidecars stay as they were.
    FAMILY_CAP.with(|cell| cell.set(Some(1)));
    let fifth = rig.lease("donor-e", "donor");
    let files = default_files();
    write_files(&fifth.env.cargo_build_build_dir, &files);
    let refused = publish_seed_catalog(&rig.pool, &catalog_for(&fifth, &files));
    assert!(matches!(refused, Err(LeaseError::LedgerExhausted)));
    assert!(!seeds
        .join(format!("{}.catalog.json", fifth.slot_id))
        .exists());
    release_injected(&rig.pool, &fifth.lease_id).expect("release fifth");
    assert!(seeds
        .join(format!("{}.catalog.json", fourth.grant.slot_id))
        .exists());
    let _ = (&first, &second, &recipient);
}

#[test]
fn catalog_schema_rejects_unknown_fields_and_unusable_paths() {
    reset_hooks();
    let rig = Rig::new("schema");
    let (grant, _) = lease_recipient(&rig, "a");
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let catalog = catalog_for(&grant, &files);
    let bytes = catalog.to_bytes().expect("valid catalog bytes");
    assert!(SeedCatalog::parse(&bytes).is_ok());

    let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    unknown
        .as_object_mut()
        .expect("object")
        .insert("extra".to_owned(), serde_json::json!(1));
    assert!(SeedCatalog::parse(&serde_json::to_vec(&unknown).expect("serialize")).is_err());

    let mut escape = catalog.clone();
    escape.packages[0].units[0].files[0].path = "debug/../../../etc/passwd".to_owned();
    assert!(escape.validate().is_err());
    let mut absolute = catalog.clone();
    absolute.packages[0].units[0].files[0].path = "/etc/passwd".to_owned();
    assert!(absolute.validate().is_err());
    let mut digest = catalog.clone();
    digest.packages[0].units[0].files[0].digest = "A".repeat(64);
    assert!(digest.validate().is_err());
    let mut units = catalog.clone();
    units.packages[0].units = vec![catalog.packages[0].units[0].clone(); 4];
    assert!(units.validate().is_err());
    let mut duplicate = catalog.clone();
    let mut second = catalog.packages[0].clone();
    second.name = "cfg_if".to_owned();
    second.target = "cfg_if".to_owned();
    duplicate.packages.push(second);
    assert!(duplicate.validate().is_err());
    let mut wrong_profile = catalog.clone();
    wrong_profile.packages[0].units[0].files[0].path =
        "release/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa/lib-cfg_if".to_owned();
    assert!(wrong_profile.validate().is_err());
    // The catalog's own byte ceiling is enforced before parsing.
    assert!(SeedCatalog::parse(&vec![b' '; SEED_CATALOG_MAX_BYTES + 1]).is_err());
}
