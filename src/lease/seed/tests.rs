//! Tiny owned fixtures for the seed custody calls (`oxide#137` §15 slice 2).
//!
//! No Cargo, no image and no volume: a budget-only directory pool, hand-written
//! catalogs and a hand-written front-door control state stand in for the front
//! door. Each test builds only its own unique directory below the recipe's test
//! temp and removes it.

use super::super::pool::{
    create_injected, lease_injected, mark_evicting_for_test, release_injected,
    rewrite_removable_backend_injected, slot_state_injected, tamper_idle_generations_injected,
};
use super::super::{
    LeaseError, LeaseGrant, LeaseRequest, Profile, SlotLayout, BUDGET_BACKEND_DIRECTORY_V1,
    VOLUME_BACKEND_APFS_V1,
};
use super::*;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, OsString};
use std::fs::{self, OpenOptions};
use std::os::fd::BorrowedFd;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const FINGERPRINT: &str = "debug/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa";
const DEP: &str = "debug/deps/cfg_if-aaaaaaaaaaaaaaaa.d";
const ARTIFACT: &str = "debug/deps/libcfg_if-aaaaaaaaaaaaaaaa.rmeta";
const SOURCE_A: &str = "/fixture/src/lib.rs";
const SOURCE_B: &str = "/fixture/src/other.rs";

thread_local! {
    static WALK_LIMITS: Cell<Option<SeedWalkLimits>> = const { Cell::new(None) };
    static FAMILY_CAP: Cell<Option<u64>> = const { Cell::new(None) };
    static TAMPER_CLONE: Cell<Option<usize>> = const { Cell::new(None) };
    static BREAK_DONOR: Cell<Option<usize>> = const { Cell::new(None) };
    static PLANT_SYMLINK: Cell<bool> = const { Cell::new(false) };
    static TAMPER_LEDGER: Cell<bool> = const { Cell::new(false) };
    static VERIFY_DELAY_MS: Cell<Option<u64>> = const { Cell::new(None) };
    static CLONE_DELAY_MS: Cell<Option<u64>> = const { Cell::new(None) };
    static REMOVE_CREATED_DIRECTORY: Cell<bool> = const { Cell::new(false) };
    static UNREADABLE_CLONE: Cell<Option<usize>> = const { Cell::new(None) };
    static HARDLINK_CLONE: Cell<bool> = const { Cell::new(false) };
    static RACE_DESTINATION: Cell<bool> = const { Cell::new(false) };
    static CLONE_FAULT: Cell<Option<(usize, bool)>> = const { Cell::new(None) };
    static CATALOG_FAULT: Cell<Option<bool>> = const { Cell::new(None) };
    static VANISH_ENTRY: RefCell<Option<OsString>> = const { RefCell::new(None) };
    static AFTER_VISIT_DELAY_MS: Cell<Option<u64>> = const { Cell::new(None) };
}

/// The injected clone-operation interruption, if this thread set one: `true`
/// means after the syscall created its entry, `false` before it.
pub(super) fn clone_fault(index: usize, after_call: bool) -> Option<LeaseError> {
    (CLONE_FAULT.with(Cell::get) == Some((index, after_call)))
        .then(|| LeaseError::Io("injected clone interruption".to_owned()))
}

/// The injected sidecar-write interruption: `true` after the atomic rename,
/// `false` with the temp written and the rename not yet done.
pub(super) fn catalog_fault(after_rename: bool) -> Option<LeaseError> {
    (CATALOG_FAULT.with(Cell::get) == Some(after_rename))
        .then(|| LeaseError::Io("injected catalog-write interruption".to_owned()))
}

/// Remove a listed entry between the directory read and its classification,
/// so the walk meets a vanished entry exactly as a concurrent writer can.
pub(super) fn walk_before_stat(directory: BorrowedFd<'_>, name: &CStr) {
    let armed = VANISH_ENTRY.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|armed| armed.as_bytes() == name.to_bytes())
    });
    if armed {
        // SAFETY: the descriptor and the name are live for the call.
        unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    }
}

/// A deterministic elapsed-time delay after the walk's visit, so the
/// completion-path deadline check can be exercised.
pub(super) fn after_visit_delay() {
    sleep_opt(AFTER_VISIT_DELAY_MS.with(Cell::get));
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
    CLONE_DELAY_MS.with(|cell| cell.set(None));
    REMOVE_CREATED_DIRECTORY.with(|cell| cell.set(false));
    UNREADABLE_CLONE.with(|cell| cell.set(None));
    HARDLINK_CLONE.with(|cell| cell.set(false));
    RACE_DESTINATION.with(|cell| cell.set(false));
    CLONE_FAULT.with(|cell| cell.set(None));
    CATALOG_FAULT.with(|cell| cell.set(None));
    VANISH_ENTRY.with(|cell| *cell.borrow_mut() = None);
    AFTER_VISIT_DELAY_MS.with(|cell| cell.set(None));
}

pub(super) fn inject(point: TestPoint<'_>) {
    match point {
        TestPoint::BeforeVerify { .. } => {
            sleep_opt(VERIFY_DELAY_MS.with(Cell::get));
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
                use std::io::Write as _;
                file.write_all(b"tampered").expect("tamper clone");
            }
        }
        TestPoint::DuringClone { index, path } => {
            if index == 0 {
                sleep_opt(CLONE_DELAY_MS.with(Cell::get));
            }
            if UNREADABLE_CLONE.with(Cell::get) == Some(index) {
                fs::set_permissions(path, fs::Permissions::from_mode(0o000))
                    .expect("unreadable clone");
            }
        }
        TestPoint::BeforeDestinationMkdir { path } => {
            if RACE_DESTINATION.with(Cell::get) {
                // An unknown writer creates the directory first, so the seed's
                // `mkdirat` fails with `EEXIST`.
                let _ = fs::create_dir(path);
            }
        }
        TestPoint::AfterDestinationMkdir { path } => {
            if REMOVE_CREATED_DIRECTORY.with(Cell::get) {
                fs::remove_dir(path).expect("remove created destination");
            }
        }
        TestPoint::BeforeWalk { slot } => {
            if PLANT_SYMLINK.with(Cell::get) {
                symlink("missing-target", slot.join("foreign-link")).expect("plant symlink");
            }
            if HARDLINK_CLONE.with(Cell::get) {
                // A hard-linked pair inside the recipient slot: one inode, two
                // directory entries.
                let build = slot.join("build");
                fs::hard_link(build.join(ARTIFACT), build.join(format!("{ARTIFACT}.link")))
                    .expect("hard link the clone");
            }
        }
        TestPoint::BeforeFinalDonorCheck { pool } => {
            if TAMPER_LEDGER.with(Cell::get) {
                tamper_idle_generations_injected(pool).expect("tamper ledger");
            }
        }
    }
}

fn sleep_opt(ms: Option<u64>) {
    if let Some(ms) = ms {
        std::thread::sleep(Duration::from_millis(ms));
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

/// One record of the front door's control state, as the donor predicate sees
/// it. Only the fields this layer reads are written.
struct StateRecord {
    id: u64,
    phase: &'static str,
    checkout: Option<PathBuf>,
    held: bool,
    pending_recovery: bool,
    retired: bool,
    lost: bool,
}

impl StateRecord {
    fn idle(id: u64) -> Self {
        Self {
            id,
            phase: "idle",
            checkout: Some(PathBuf::from("/fixture/checkout")),
            held: false,
            pending_recovery: false,
            retired: false,
            lost: false,
        }
    }

    fn json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "id": self.id,
            "phase": self.phase,
            "checkout": self.checkout,
        });
        if self.held {
            value["holder"] = serde_json::json!({
                "pid": 1,
                "started_ms": 0,
                "lease_holder": "fixture",
            });
            value["lease_id"] = serde_json::json!("fixture-lease");
        }
        if self.pending_recovery {
            value["pending_recovery"] = serde_json::json!({
                "slot_id": self.id.to_string(),
                "lease_id": "fixture-lease",
            });
        }
        if self.retired {
            value["retired"] = serde_json::json!("flush failed");
        }
        if self.lost {
            value["lost"] = serde_json::json!({
                "slot_id": self.id.to_string(),
                "at_ms": 0,
            });
        }
        value
    }
}

struct Rig {
    fx: Fixture,
    pool: PathBuf,
    records: RefCell<Vec<StateRecord>>,
    cleans: RefCell<Vec<PathBuf>>,
    next_record: Cell<u64>,
}

impl Rig {
    fn new(label: &str) -> Self {
        let fx = Fixture::new(label);
        let pool = fx.dir("pool");
        create_injected(&pool, 4 * 1024 * 1024).expect("init pool");
        Self {
            fx,
            pool,
            records: RefCell::new(Vec::new()),
            cleans: RefCell::new(Vec::new()),
            next_record: Cell::new(1),
        }
    }

    fn next_record_id(&self) -> u64 {
        let id = self.next_record.get();
        self.next_record.set(id + 1);
        id
    }

    /// The front door's affinity directory for one record: the project a
    /// front-door lease records, so a slot's ledger project names its record.
    fn affinity_dir(&self, record_id: u64) -> PathBuf {
        let managed = self.pool.join(MANAGED_NAME);
        let affinity = managed.join("affinity");
        let path = affinity.join(record_id.to_string());
        for directory in [&managed, &affinity, &path] {
            fs::create_dir_all(directory).expect("affinity dir");
            // The control directory is owner-only; the front door's
            // `ensure_private_dir` refuses anything looser.
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .expect("affinity mode");
        }
        path
    }

    fn control(&self) -> PathBuf {
        self.pool.join(MANAGED_NAME)
    }

    fn lease(&self, label: &str, holder: &str) -> LeaseGrant {
        let project = self.fx.dir(&format!("project-{label}"));
        self.lease_at(label, holder, project)
    }

    fn lease_at(&self, label: &str, holder: &str, project: PathBuf) -> LeaseGrant {
        self.lease_full(label, holder, project, None)
    }

    /// A lease whose slot goes into its own project-named folder, so the
    /// `slots/<folder>/<leaf>` intermediate components are exercised.
    fn lease_layout(&self, label: &str, holder: &str, folder: &str) -> LeaseGrant {
        let project = self.fx.dir(&format!("project-{label}"));
        self.lease_full(
            label,
            holder,
            project,
            Some(SlotLayout {
                folders: vec![folder.to_owned()],
                same_repository: Vec::new(),
                profile: "quick".to_owned(),
            }),
        )
    }

    fn lease_full(
        &self,
        label: &str,
        holder: &str,
        project: PathBuf,
        layout: Option<SlotLayout>,
    ) -> LeaseGrant {
        let workspace = self.fx.dir(&format!("workspace-{label}"));
        lease_injected(LeaseRequest {
            pool: self.pool.clone(),
            project,
            workspace,
            profile: Profile::Dev,
            holder: holder.to_owned(),
            budget_bytes: None,
            reserve_bytes: None,
            layout,
            replace_slot: None,
        })
        .expect("lease")
    }

    fn add_record(&self, record: StateRecord) {
        {
            let mut records = self.records.borrow_mut();
            records.retain(|existing| existing.id != record.id);
            records.push(record);
        }
        self.save_control();
    }

    fn add_clean(&self, checkout: &str) {
        self.cleans.borrow_mut().push(PathBuf::from(checkout));
        self.save_control();
    }

    fn state_version(&self, version: u32) {
        self.save_control_version(version);
    }

    fn save_control(&self) {
        let current = {
            let path = self.control().join("state.json");
            let bytes = fs::read(&path).ok();
            bytes
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["version"].as_u64())
                .and_then(|version| u32::try_from(version).ok())
                .unwrap_or(5)
        };
        self.save_control_version(current);
    }

    fn save_control_version(&self, version: u32) {
        let control = self.control();
        fs::create_dir_all(control.join("locks")).expect("control dirs");
        fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("control mode");
        fs::set_permissions(control.join("locks"), fs::Permissions::from_mode(0o700))
            .expect("locks mode");
        let records: Vec<serde_json::Value> = self
            .records
            .borrow()
            .iter()
            .map(StateRecord::json)
            .collect();
        let cleans: Vec<serde_json::Value> = self
            .cleans
            .borrow()
            .iter()
            .map(|checkout| serde_json::json!({"id": 1, "checkout": checkout}))
            .collect();
        let state = serde_json::json!({
            "version": version,
            "records": records,
            "cleans": cleans,
        });
        let path = control.join("state.json");
        fs::write(&path, serde_json::to_vec(&state).expect("control json")).expect("control state");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("state mode");
    }

    /// The front-door record lock for exactly this record id.
    fn donor_lock(&self, record_id: u64) -> PathBuf {
        let locks = self.control().join("locks");
        fs::create_dir_all(&locks).expect("locks dir");
        fs::set_permissions(&locks, fs::Permissions::from_mode(0o700)).expect("locks mode");
        let path = locks.join(format!("{record_id}.lock"));
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
    record_id: u64,
    catalog: SeedCatalog,
    digest: String,
    files: Vec<(String, String)>,
}

impl Donor {
    fn request(&self, recipient: &SeedRecipient) -> SeedRequest {
        SeedRequest {
            recipient: recipient.clone(),
            donor: SeedDonor {
                slot_id: self.grant.slot_id.clone(),
                incarnation: self.grant.incarnation.clone(),
                lease_id: self.grant.lease_id.clone(),
                record_id: self.record_id,
                catalog_digest: self.digest.clone(),
            },
            units: vec![SeedUnitRef {
                fingerprint_dir: FINGERPRINT.to_owned(),
                role: SeedRole::Lib,
            }],
            work_budget_ms: None,
        }
    }
}

fn sha(text: &str) -> String {
    catalog_digest(text.as_bytes())
}

fn dep_info_text() -> String {
    // The ordinary rustc shape: the `.d`'s own rule first, then the role's
    // artifact, then one empty phony rule per source input.
    format!(
        "{DEP}: {SOURCE_A} {SOURCE_B}\n{ARTIFACT}: {SOURCE_A} {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n"
    )
}

fn dep_targets() -> Vec<String> {
    vec![
        DEP.to_owned(),
        ARTIFACT.to_owned(),
        SOURCE_A.to_owned(),
        SOURCE_B.to_owned(),
    ]
}

fn dep_inputs() -> Vec<String> {
    vec![SOURCE_A.to_owned(), SOURCE_B.to_owned()]
}

fn file_kind(path: &str) -> SeedFileKind {
    match path {
        DEP => SeedFileKind::DepInfo,
        ARTIFACT => SeedFileKind::Artifact,
        _ => SeedFileKind::Fingerprint,
    }
}

fn files_with_dep(dep: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("{FINGERPRINT}/lib-cfg_if"),
            "fingerprint".to_owned(),
        ),
        (format!("{FINGERPRINT}/lib-cfg_if.json"), "{}".to_owned()),
        (format!("{FINGERPRINT}/dep-lib-cfg_if"), String::new()),
        (ARTIFACT.to_owned(), "rmeta-fixture".to_owned()),
        (DEP.to_owned(), dep.to_owned()),
    ]
}

fn default_files() -> Vec<(String, String)> {
    files_with_dep(&dep_info_text())
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

fn catalog_for(grant: &LeaseGrant, record_id: u64, files: &[(String, String)]) -> SeedCatalog {
    let dep = files
        .iter()
        .find(|(path, _)| path == DEP)
        .map(|(_, content)| content.as_str())
        .unwrap_or_default();
    catalog_with_join(grant, record_id, files, &dep_targets(), &dep_inputs(), dep)
}

fn catalog_with_join(
    grant: &LeaseGrant,
    record_id: u64,
    files: &[(String, String)],
    targets: &[String],
    inputs: &[String],
    dep: &str,
) -> SeedCatalog {
    let entries: Vec<SeedFile> = files
        .iter()
        .map(|(path, content)| SeedFile {
            path: path.clone(),
            size: content.len() as u64,
            mtime_ms: 0,
            digest: sha(content),
            kind: file_kind(path),
        })
        .collect();
    SeedCatalog {
        schema: SEED_CATALOG_SCHEMA.to_owned(),
        store_id: grant.store_id.clone(),
        slot_id: grant.slot_id.clone(),
        incarnation: grant.incarnation.clone(),
        lease_id: grant.lease_id.clone(),
        record_id,
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
                    targets: targets.to_vec(),
                    inputs: inputs.to_vec(),
                    digest: sha(dep),
                },
                files: entries,
            }],
        }],
        rejected: Vec::new(),
    }
}

/// One released donor slot with a published catalog, its own record lock and
/// an idle control record.
fn make_donor(rig: &Rig, label: &str) -> Donor {
    make_donor_with(rig, label, default_files(), &dep_targets(), &dep_inputs())
}

fn make_donor_with(
    rig: &Rig,
    label: &str,
    files: Vec<(String, String)>,
    targets: &[String],
    inputs: &[String],
) -> Donor {
    make_donor_in(rig, label, None, files, targets, inputs)
}

fn make_donor_in_folder(rig: &Rig, label: &str, folder: &str) -> Donor {
    make_donor_in(
        rig,
        label,
        Some(folder),
        default_files(),
        &dep_targets(),
        &dep_inputs(),
    )
}

fn make_donor_in(
    rig: &Rig,
    label: &str,
    folder: Option<&str>,
    files: Vec<(String, String)>,
    targets: &[String],
    inputs: &[String],
) -> Donor {
    // A front-door lease records its record's affinity directory as the
    // project, which is the binding a catalog's record id must match.
    let record_id = rig.next_record_id();
    let project = rig.affinity_dir(record_id);
    let holder = "donor";
    let name = format!("donor-{label}");
    let grant = match folder {
        Some(folder) => rig.lease_full(
            &name,
            holder,
            project,
            Some(SlotLayout {
                folders: vec![folder.to_owned()],
                same_repository: Vec::new(),
                profile: "quick".to_owned(),
            }),
        ),
        None => rig.lease_at(&name, holder, project),
    };
    write_files(&grant.env.cargo_build_build_dir, &files);
    let dep = files
        .iter()
        .find(|(path, _)| path == DEP)
        .map(|(_, content)| content.as_str())
        .unwrap_or_default()
        .to_owned();
    let catalog = catalog_with_join(&grant, record_id, &files, targets, inputs, &dep);
    publish_seed_catalog(&rig.pool, &catalog).expect("publish catalog");
    let bytes = catalog.to_bytes().expect("catalog bytes");
    let digest = catalog_digest(&bytes);
    release_injected(&rig.pool, &grant.lease_id).expect("release donor");
    rig.donor_lock(record_id);
    rig.add_record(StateRecord::idle(record_id));
    Donor {
        grant,
        record_id,
        catalog,
        digest,
        files,
    }
}

/// A held producing lease and the catalog it would publish, before release:
/// the exact state a `publish_seed_catalog` call starts from.
fn held_catalog(rig: &Rig, label: &str) -> (LeaseGrant, SeedCatalog) {
    let record_id = rig.next_record_id();
    let grant = rig.lease_at(
        &format!("donor-{label}"),
        "donor",
        rig.affinity_dir(record_id),
    );
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let catalog = catalog_for(&grant, record_id, &files);
    (grant, catalog)
}

fn lease_recipient(rig: &Rig, label: &str) -> (LeaseGrant, SeedRecipient) {
    // One holder holds one lease at a time, so each recipient fixture has its
    // own holder identity.
    let grant = rig.lease(&format!("recipient-{label}"), &format!("recipient-{label}"));
    let recipient = seed_recipient(&grant);
    (grant, recipient)
}

fn lease_recipient_in_folder(rig: &Rig, label: &str, folder: &str) -> (LeaseGrant, SeedRecipient) {
    let grant = rig.lease_layout(
        &format!("recipient-{label}"),
        &format!("recipient-{label}"),
        folder,
    );
    let recipient = seed_recipient(&grant);
    (grant, recipient)
}

fn seed_recipient(grant: &LeaseGrant) -> SeedRecipient {
    SeedRecipient {
        slot_id: grant.slot_id.clone(),
        incarnation: grant.incarnation.clone(),
        lease_id: grant.lease_id.clone(),
    }
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

/// Rewrite a published catalog sidecar with `mutate` applied, returning the new
/// catalog bytes' digest for a request.
fn rewrite_catalog(rig: &Rig, donor: &Donor, mutate: impl FnOnce(&mut SeedCatalog)) -> String {
    let mut catalog = donor.catalog.clone();
    mutate(&mut catalog);
    let bytes = catalog.to_bytes().expect("catalog bytes");
    let path = rig
        .pool
        .join(MANAGED_NAME)
        .join("seeds")
        .join(format!("{}.catalog.json", donor.grant.slot_id));
    fs::write(&path, &bytes).expect("rewrite catalog");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("catalog mode");
    catalog_digest(&bytes)
}

#[test]
fn candidates_require_the_held_recipient_and_offer_catalogs_by_rank() {
    reset_hooks();
    let rig = Rig::new("candidates");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let found = candidates(&rig, &recipient);
    assert_eq!(found.candidates.len(), 1);
    let candidate = &found.candidates[0];
    assert_eq!(candidate.slot_id, donor.grant.slot_id);
    assert_eq!(candidate.catalog_digest, donor.digest);
    assert_eq!(candidate.catalog, donor.catalog);
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
    // the catalog atomically (§4.6). Its recorded project is its affinity.
    let again = rig.lease_at("donor-a", "donor", rig.affinity_dir(donor.record_id));
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
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
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
    let path = rig
        .pool
        .join(MANAGED_NAME)
        .join("seeds")
        .join(format!("{}.catalog.json", donor.grant.slot_id));
    let mut changed = donor.catalog.clone();
    changed.mark = 2;
    fs::write(&path, changed.to_bytes().expect("changed bytes")).expect("rewrite catalog");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("catalog mode");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
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
    let request = donor.request(&recipient);
    // Take the donor slot again: its ledger state is no longer `Idle`.
    let busy = rig.lease_at("donor-a", "donor", rig.affinity_dir(donor.record_id));
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
    let request = donor.request(&recipient);
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
fn seed_skips_a_join_that_does_not_prove_each_artifact() {
    reset_hooks();
    let rig = Rig::new("join-inconsistent");

    // The positive control: the ordinary rustc shape proves the artifact.
    let positive = make_donor(&rig, "control");
    let (grant, recipient) = lease_recipient(&rig, "control");
    let outcome = seed(&rig.pool, &positive.request(&recipient)).expect("seed");
    assert!(matches!(outcome, SeedOutcome::Seeded(_)), "{outcome:?}");
    assert!(grant.env.cargo_build_build_dir.join(ARTIFACT).exists());

    let refused = |donor: &Donor, label: &str| {
        let (grant, recipient) = lease_recipient(&rig, label);
        let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
        assert_eq!(
            outcome,
            SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::JoinInconsistent
            }),
            "{label}"
        );
        assert!(tree(&grant.env.cargo_build_build_dir).is_empty(), "{label}");
    };

    // A flattened rule names the artifact among several targets: the artifact
    // has no rule of its own, so nothing establishes its prerequisites.
    let flattened = format!("{DEP} {ARTIFACT}: {SOURCE_A} {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n");
    let donor = make_donor_with(
        &rig,
        "flat",
        files_with_dep(&flattened),
        &dep_targets(),
        &dep_inputs(),
    );
    refused(&donor, "flat");

    // An artifact rule with no source prerequisites proves nothing.
    let empty = format!("{DEP}: {SOURCE_A} {SOURCE_B}\n{ARTIFACT}:\n{SOURCE_A}:\n{SOURCE_B}:\n");
    let donor = make_donor_with(
        &rig,
        "empty",
        files_with_dep(&empty),
        &dep_targets(),
        &dep_inputs(),
    );
    refused(&donor, "empty");

    // An artifact the `.d` never names. The recorded flat sets are the ones
    // this `.d` really has, so only the missing artifact rule is at fault.
    let missing = format!("{DEP}: {SOURCE_A} {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n");
    let missing_targets = vec![DEP.to_owned(), SOURCE_A.to_owned(), SOURCE_B.to_owned()];
    let donor = make_donor_with(
        &rig,
        "missing",
        files_with_dep(&missing),
        &missing_targets,
        &dep_inputs(),
    );
    refused(&donor, "missing");

    // Two rules name the same artifact: duplicate evidence.
    let duplicate = format!(
        "{DEP}: {SOURCE_A} {SOURCE_B}\n{ARTIFACT}: {SOURCE_A}\n{ARTIFACT}: {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n"
    );
    let donor = make_donor_with(
        &rig,
        "duplicate",
        files_with_dep(&duplicate),
        &dep_targets(),
        &dep_inputs(),
    );
    refused(&donor, "duplicate");

    // Two rules name it with different prerequisite sets: conflicting.
    let conflicting = format!(
        "{DEP}: {SOURCE_A} {SOURCE_B}\n{ARTIFACT}: {SOURCE_A}\n{ARTIFACT}: {SOURCE_A} {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n"
    );
    let donor = make_donor_with(
        &rig,
        "conflicting",
        files_with_dep(&conflicting),
        &dep_targets(),
        &dep_inputs(),
    );
    refused(&donor, "conflicting");

    // An unparseable `.d`.
    let donor = make_donor_with(
        &rig,
        "unparseable",
        files_with_dep("not a rule\n"),
        &dep_targets(),
        &dep_inputs(),
    );
    refused(&donor, "unparseable");

    // The recorded flat rule sets disagree with the donated `.d`: the digest
    // still matches the file, so only the re-parse can catch it.
    let donor = make_donor(&rig, "flat-sets");
    let (_, recipient) = lease_recipient(&rig, "flat-sets");
    let digest = rewrite_catalog(&rig, &donor, |catalog| {
        catalog.packages[0].units[0].join.targets = vec![DEP.to_owned()];
    });
    let mut request = donor.request(&recipient);
    request.donor.catalog_digest = digest;
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::JoinInconsistent
        })
    );
}

#[test]
fn seed_skips_a_catalog_whose_record_is_not_the_donor_slots_affinity() {
    reset_hooks();
    let rig = Rig::new("record-affinity");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // Another record exists, is idle and has its own free lock, but the donor
    // slot's recorded project is the donor record's affinity: a catalog
    // naming the other record cannot select that record's custody.
    let other = rig.next_record_id();
    rig.donor_lock(other);
    rig.add_record(StateRecord::idle(other));
    let digest = rewrite_catalog(&rig, &donor, |catalog| catalog.record_id = other);
    let mut request = donor.request(&recipient);
    request.donor.record_id = other;
    request.donor.catalog_digest = digest;
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorCustodyUnknown
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn this_slice_never_writes_a_removable_payload() {
    reset_hooks();
    assert!(internal_backend(BUDGET_BACKEND_DIRECTORY_V1));
    assert!(!internal_backend(VOLUME_BACKEND_APFS_V1));
    assert_eq!(
        SeedSkipReason::RemovablePoolUnsupported.as_str(),
        "seeding not yet supported on a removable pool"
    );
}

#[test]
fn a_removable_pool_is_skipped_before_any_payload_access() {
    reset_hooks();
    let rig = Rig::new("removable-gate");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // A second producing lease, still held, so the internal sidecar write can
    // also be exercised on the same pool.
    let record_id = rig.next_record_id();
    let held = rig.lease_at("donor-b", "donor", rig.affinity_dir(record_id));
    let files = default_files();
    write_files(&held.env.cargo_build_build_dir, &files);
    let held_catalog = catalog_for(&held, record_id, &files);

    // Record the pool as a removable volume whose mount point does not exist:
    // any attempt to open or probe its payload would fail here.
    let absent_mount = rig.fx.path.join("absent-volume");
    rewrite_removable_backend_injected(&rig.pool, &absent_mount).expect("rewrite backend");

    // Call 1 answers from the ledger alone: no candidate, no volume access.
    assert!(candidates(&rig, &recipient).candidates.is_empty());
    // Call 2 is the fail-closed skip with the reported reason; the normal cold
    // build follows.
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::RemovablePoolUnsupported
        })
    );
    // The catalog sidecar is internal, so publishing still works with no
    // volume present.
    publish_seed_catalog(&rig.pool, &held_catalog).expect("publish catalog");
    assert!(!absent_mount.exists(), "no mount point was created");
    // Nothing was cloned anywhere.
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn a_removable_pool_reports_the_unsupported_skip_before_donor_lock_outcomes() {
    reset_hooks();
    let rig = Rig::new("removable-before-lock");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    let absent_mount = rig.fx.path.join("absent-volume");
    rewrite_removable_backend_injected(&rig.pool, &absent_mount).expect("rewrite backend");

    // A missing donor lock would be a donor-lock skip on an internal pool; the
    // removable reason must win.
    fs::remove_file(rig.donor_lock(donor.record_id)).expect("remove donor lock");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::RemovablePoolUnsupported
        })
    );

    // A held donor lock would be a donor-busy skip; the removable reason still
    // wins.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.donor_lock(donor.record_id))
        .expect("open lock");
    // SAFETY: the descriptor is live for the call.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::RemovablePoolUnsupported
        })
    );
}

#[test]
fn plan_bounds_the_total_join_inputs_across_units() {
    reset_hooks();
    let rig = Rig::new("plan-input-cap");
    let (grant, _) = lease_recipient(&rig, "a");
    let record_id = grant.slot_id.parse::<u64>().expect("record id");
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let mut catalog = catalog_for(&grant, record_id, &files);
    let inputs: Vec<String> = (0..SEED_MAX_UNIT_INPUTS)
        .map(|index| format!("/fixture/src/{index}.rs"))
        .collect();
    catalog.packages[0].units = (0..9)
        .map(|index| SeedUnit {
            fingerprint_dir: format!("debug/.fingerprint/cfg-if-{index:016x}"),
            role: SeedRole::Lib,
            target: "cfg_if".to_owned(),
            join: SeedJoin {
                path: format!("debug/deps/cfg_if-{index:016x}.d"),
                targets: vec![format!("debug/deps/cfg_if-{index:016x}.d")],
                inputs: inputs.clone(),
                digest: "a".repeat(64),
            },
            files: vec![SeedFile {
                path: format!("debug/deps/lib{index}.rmeta"),
                size: 1,
                mtime_ms: 0,
                digest: "b".repeat(64),
                kind: SeedFileKind::Artifact,
            }],
        })
        .collect();
    let selectors: Vec<SeedUnitRef> = catalog.packages[0]
        .units
        .iter()
        .map(|unit| SeedUnitRef {
            fingerprint_dir: unit.fingerprint_dir.clone(),
            role: SeedRole::Lib,
        })
        .collect();
    // Eight units hold exactly the 32,768 plan-wide bound.
    assert!(plan_units(&catalog, &selectors[..8]).is_ok());
    // A ninth unit takes the plan past it.
    assert!(matches!(
        plan_units(&catalog, &selectors),
        Err(SeedSkipReason::PlanCap)
    ));
}

#[test]
fn seed_refuses_an_unestablishable_donor_path_instead_of_skipping() {
    reset_hooks();
    let rig = Rig::new("donor-path-symlink");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // Replace the donor artifact with a symlink: the path no longer names an
    // owned regular file, which is a refusal, never a cold skip.
    let artifact = donor.grant.env.cargo_build_build_dir.join(ARTIFACT);
    fs::remove_file(&artifact).expect("remove donor artifact");
    symlink(donor.grant.env.cargo_build_build_dir.join(DEP), &artifact).expect("symlink artifact");
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());

    // A missing donor file stays the payload-change skip.
    let donor = make_donor(&rig, "b");
    let (grant, recipient) = lease_recipient(&rig, "b");
    fs::remove_file(donor.grant.env.cargo_build_build_dir.join(ARTIFACT))
        .expect("remove donor artifact");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
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
    // A missing lock for the record the plan names is a skip, not a wait.
    fs::remove_file(rig.donor_lock(donor.record_id)).expect("remove donor lock");
    let missing = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        missing,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorLockMissing
        })
    );
    // A held record lock means the donor is skipped, never waited for.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.donor_lock(donor.record_id))
        .expect("open record lock");
    // SAFETY: the descriptor is live for the call.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let held = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        held,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorBusy
        })
    );
}

#[test]
fn seed_does_not_accept_a_lock_for_another_record() {
    reset_hooks();
    let rig = Rig::new("donor-lock-unrelated");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    // A second, unrelated record's lock is held and its control record is
    // idle: neither stands in for the donor's own closure.
    let other = donor.record_id + 1000;
    rig.donor_lock(other);
    rig.add_record(StateRecord::idle(other));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(rig.donor_lock(other))
        .expect("open unrelated lock");
    // SAFETY: the descriptor is live for the call.
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert!(matches!(outcome, SeedOutcome::Seeded(_)));
}

#[test]
fn seed_skips_a_donor_plan_whose_record_id_is_not_the_catalog_record() {
    reset_hooks();
    let rig = Rig::new("record-mismatch");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let other = donor.record_id + 1000;
    rig.donor_lock(other);
    rig.add_record(StateRecord::idle(other));
    let mut request = donor.request(&recipient);
    request.donor.record_id = other;
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::CatalogChanged
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_donor_control_record_that_is_not_idle() {
    reset_hooks();
    let rig = Rig::new("control-busy");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut record = StateRecord::idle(donor.record_id);
    record.phase = "running";
    record.held = true;
    rig.add_record(record);
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorBusy
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_donor_with_pending_recovery_retirement_or_loss() {
    reset_hooks();
    let rig = Rig::new("control-custody");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    for mutate in [
        (|record: &mut StateRecord| record.pending_recovery = true) as fn(&mut StateRecord),
        |record: &mut StateRecord| record.retired = true,
        |record: &mut StateRecord| record.lost = true,
    ] {
        let mut record = StateRecord::idle(donor.record_id);
        mutate(&mut record);
        rig.add_record(record);
        let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
        assert_eq!(
            outcome,
            SeedOutcome::Skipped(SeedSkip {
                reason: SeedSkipReason::DonorCustodyUnknown
            })
        );
    }
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_donor_with_a_recorded_clean_of_its_checkout() {
    reset_hooks();
    let rig = Rig::new("control-clean");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    rig.add_clean("/fixture/checkout");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorBusy
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_an_unknown_or_missing_control_state() {
    reset_hooks();
    let rig = Rig::new("control-unknown");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    rig.state_version(4);
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorCustodyUnknown
        })
    );
    fs::remove_file(rig.control().join("state.json")).expect("remove state");
    let missing = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        missing,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::DonorCustodyUnknown
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn seed_skips_a_plan_naming_a_unit_the_catalog_lacks() {
    reset_hooks();
    let rig = Rig::new("unit-missing");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&recipient);
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
    let mut request = donor.request(&recipient);
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
    // The byte cap is checked on catalog data alone.
    let mut oversized = donor.catalog.clone();
    oversized.packages[0].units[0].files[0].size = SEED_MAX_PLAN_BYTES + 1;
    let request = donor.request(&recipient);
    assert!(matches!(
        plan_units(&oversized, &request.units),
        Err(SeedSkipReason::PlanCap)
    ));
}

#[test]
fn the_work_budget_is_capped_at_five_seconds_and_checked_within_reads() {
    reset_hooks();
    assert_eq!(
        work_budget(None),
        Duration::from_millis(SEED_WORK_BUDGET_MS)
    );
    assert_eq!(
        work_budget(Some(60_000)),
        Duration::from_millis(SEED_WORK_BUDGET_MS)
    );
    assert_eq!(work_budget(Some(1)), Duration::from_millis(1));
    assert_eq!(work_budget(Some(0)), Duration::from_millis(1));
    // A streamed read past its deadline is a deadline, not a digest.
    let rig = Rig::new("hash-deadline");
    let (grant, _) = lease_recipient(&rig, "a");
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let path = grant.env.cargo_build_build_dir.join(ARTIFACT);
    let file = std::fs::File::open(&path).expect("open fixture file");
    let past = Instant::now() - Duration::from_millis(1);
    assert!(matches!(
        hash_fd(file.as_fd(), past),
        Err(StreamError::Deadline)
    ));
    let future = Instant::now() + Duration::from_secs(60);
    assert!(matches!(hash_fd(file.as_fd(), future), Ok(digest) if digest == sha("rmeta-fixture")));
}

#[test]
fn seed_skips_on_the_work_deadline_before_the_first_write() {
    reset_hooks();
    let rig = Rig::new("deadline");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&recipient);
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
fn seed_refuses_after_the_clone_when_the_work_deadline_expires() {
    reset_hooks();
    let rig = Rig::new("deadline-clone");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let mut request = donor.request(&recipient);
    // A generous-enough budget that the pre-clone revalidation cannot
    // plausibly exhaust it, then a delay longer than it after the first clone.
    request.work_budget_ms = Some(1_000);
    CLONE_DELAY_MS.with(|cell| cell.set(Some(1_200)));
    let refused = seed(&rig.pool, &request);
    assert!(matches!(refused, Err(LeaseError::Io(_))));
    // The clone is retained untrusted; nothing is published.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
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
    let refused = seed(&rig.pool, &donor.request(&recipient));
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
    let refused = seed(&rig.pool, &donor.request(&recipient));
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
fn seed_skips_when_the_build_directory_cannot_be_created_before_any_write() {
    reset_hooks();
    let rig = Rig::new("clone-failed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let build = grant.env.cargo_build_build_dir.clone();
    assert!(
        build.exists(),
        "the pool creates the slot's build directory"
    );
    // No recipient write has happened yet: the (empty) `build/` refuses the
    // first destination-directory creation, which is a pre-write fault and
    // therefore a skip that leaves the same slot to build cold.
    fs::set_permissions(&build, fs::Permissions::from_mode(0o500)).expect("read-only build");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    fs::set_permissions(&build, fs::Permissions::from_mode(0o700)).expect("writable build");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::CloneFailed
        })
    );
    assert!(tree(&build).is_empty());
}

#[test]
fn seed_refuses_a_fault_after_the_destination_directory_is_created() {
    reset_hooks();
    let rig = Rig::new("destination-entry");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_ledger = rig.ledger_bytes();
    let before_sidecars = rig.sidecars();
    // The destination directory was created (a recipient write), and the
    // destination itself already exists, so `clonefile` fails with `EEXIST`:
    // an unknown writer's entry, which refuses and retains (a safety refusal,
    // not a cold skip) and has no byte-copy or hard-link fallback.
    BREAK_DONOR.with(|cell| cell.set(Some(0)));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    let destination = grant.env.cargo_build_build_dir.join(&donor.files[0].0);
    assert!(fs::symlink_metadata(&destination)
        .expect("planted entry")
        .file_type()
        .is_dir());
    assert!(!grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[1].0)
        .exists());
    // Retained: nothing is published and the ledger still records the held
    // recipient and the idle donor for the ordinary journaled eviction.
    assert_eq!(rig.ledger_bytes(), before_ledger);
    assert_eq!(rig.sidecars(), before_sidecars);
}

#[test]
fn seed_refuses_when_the_first_clone_is_created_but_untrusted() {
    reset_hooks();
    let rig = Rig::new("first-clone-untrusted");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    // The first clone is created and then fails its digest: the point of no
    // return has been crossed, so the fault refuses and retains.
    TAMPER_CLONE.with(|cell| cell.set(Some(0)));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(refused, Err(LeaseError::UnsafePath(_))));
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn seed_abandons_on_a_failed_recipient_walk_and_publishes_nothing() {
    reset_hooks();
    let rig = Rig::new("walk-failed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    PLANT_SYMLINK.with(|cell| cell.set(true));
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
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
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::EntryLimit));
    assert_eq!(abandon.observation.limits.max_entries, 1);
}

#[test]
fn seed_abandons_on_an_unreadable_recipient_entry() {
    reset_hooks();
    let rig = Rig::new("walk-unreadable");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    // One cloned file cannot be read by its owner: the walk is incomplete, so
    // seeding abandons (nothing published) instead of refusing.
    UNREADABLE_CLONE.with(|cell| cell.set(Some(0)));
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::Unreadable));
    // The clone is retained, untrusted, and nothing was published.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
    assert_eq!(rig.sidecars(), before_sidecars);
}

#[test]
fn seed_abandons_when_a_listed_recipient_entry_vanishes() {
    reset_hooks();
    let rig = Rig::new("walk-vanished");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    // The artifact disappears between the directory read and its stat, exactly
    // as a concurrent writer can make it: the walk cannot establish coverage,
    // so seeding abandons instead of publishing.
    let name = ARTIFACT.rsplit('/').next().expect("artifact name");
    VANISH_ENTRY.with(|cell| *cell.borrow_mut() = Some(OsString::from(name)));
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::Vanished));
    assert!(!abandon.observation.complete());
    assert_eq!(rig.sidecars(), before_sidecars);
    // The recipient is retained untrusted, not published.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn seed_abandons_when_the_walk_deadline_expires_at_completion() {
    reset_hooks();
    let rig = Rig::new("walk-deadline");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    // The walk finishes its visit inside a 10 ms bound and the budget elapses
    // before the completion check: the observation is not complete.
    WALK_LIMITS.with(|cell| {
        cell.set(Some(SeedWalkLimits {
            max_entries: SEED_WALK_MAX_ENTRIES,
            max_depth: SEED_WALK_MAX_DEPTH,
            max_elapsed_ms: 10,
        }))
    });
    AFTER_VISIT_DELAY_MS.with(|cell| cell.set(Some(30)));
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    let SeedOutcome::Abandoned(abandon) = outcome else {
        panic!("expected an abandoned seed, got {outcome:?}");
    };
    assert_eq!(abandon.observation.gap, Some(SeedWalkGap::Deadline));
    assert_eq!(abandon.observation.limits.max_elapsed_ms, 10);
}

#[test]
fn a_missing_walk_root_is_an_unreadable_gap() {
    reset_hooks();
    let rig = Rig::new("walk-missing-root");
    let missing = rig.pool.join("payload").join("slots").join("no-such-slot");
    let outcome = walk::walk_slot(
        &missing,
        0,
        SeedWalkLimits {
            max_entries: 10,
            max_depth: 4,
            max_elapsed_ms: 1_000,
        },
        Instant::now(),
    );
    assert_eq!(outcome.gap, Some(SeedWalkGap::Unreadable));
    assert_eq!(outcome.entries, 0);
}

#[test]
fn seed_refuses_a_slot_folder_symlink_on_either_side() {
    reset_hooks();
    let rig = Rig::new("slot-folder-symlink");
    let donor = make_donor_in_folder(&rig, "a", "donor-folder");
    let (grant, recipient) = lease_recipient_in_folder(&rig, "a", "recipient-folder");
    let slots = rig.pool.join("payload").join("slots");
    let outside = rig.fx.dir("outside");
    let donor_folder = slots.join("donor-folder");
    let recipient_folder = slots.join("recipient-folder");
    let recipient_leaf = grant
        .env
        .cargo_build_build_dir
        .parent()
        .expect("slot dir")
        .file_name()
        .expect("slot leaf")
        .to_owned();

    // Donor side: the project folder becomes a link to a directory outside the
    // payload. Custody refuses it instead of following the redirect.
    fs::rename(&donor_folder, outside.join("donor-folder")).expect("move donor folder");
    symlink(outside.join("donor-folder"), &donor_folder).expect("donor symlink");
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
    fs::remove_file(&donor_folder).expect("remove donor symlink");
    fs::rename(outside.join("donor-folder"), &donor_folder).expect("restore donor folder");

    // Recipient side: the same redirect on the recipient's own folder.
    fs::rename(&recipient_folder, outside.join("recipient-folder")).expect("move recipient folder");
    symlink(outside.join("recipient-folder"), &recipient_folder).expect("recipient symlink");
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    let redirected = outside
        .join("recipient-folder")
        .join(&recipient_leaf)
        .join("build");
    assert!(tree(&redirected).is_empty());
}

#[test]
fn seed_refuses_a_destination_entry_that_races_the_first_mkdir() {
    reset_hooks();
    let rig = Rig::new("mkdir-race");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_ledger = rig.ledger_bytes();
    // No recipient write has happened yet, but an entry appears where the seed
    // would create its first destination directory: that is an unknown
    // writer's entry, so it refuses instead of cold-skipping.
    RACE_DESTINATION.with(|cell| cell.set(true));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    assert_eq!(rig.ledger_bytes(), before_ledger);
    assert!(!grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn the_recipient_walk_unions_hard_linked_inodes() {
    reset_hooks();
    let rig = Rig::new("walk-hardlink");
    let donor = make_donor(&rig, "a");
    let (_, recipient) = lease_recipient(&rig, "a");
    // A hard-linked pair inside the recipient slot is two directory entries of
    // one inode: the within-slot union counts its bytes once.
    HARDLINK_CLONE.with(|cell| cell.set(true));
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    let SeedOutcome::Seeded(receipt) = outcome else {
        panic!("expected a published seed, got {outcome:?}");
    };
    let cloned: u64 = donor
        .files
        .iter()
        .map(|(_, content)| content.len() as u64)
        .sum();
    assert_eq!(receipt.observation.logical_bytes, cloned);
    assert!(receipt.observation.complete());
    // The extra directory entry is counted as an entry, not as extra bytes.
    assert!(receipt.observation.entries > receipt.files);
}

#[test]
fn seed_refuses_a_fault_between_the_destination_mkdir_and_its_reopen() {
    reset_hooks();
    let rig = Rig::new("mkdir-reopen");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_ledger = rig.ledger_bytes();
    let before_sidecars = rig.sidecars();
    // The first destination directory is created and then disappears before
    // its reopen. The write already happened, so this refuses and retains
    // rather than cold-skipping in the same slot.
    REMOVE_CREATED_DIRECTORY.with(|cell| cell.set(true));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(refused, Err(LeaseError::Io(_))), "{refused:?}");
    assert_eq!(rig.ledger_bytes(), before_ledger);
    assert_eq!(rig.sidecars(), before_sidecars);
    assert!(!grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn crash_row_partial_clone_is_retained_unpublished_and_never_reused() {
    reset_hooks();
    let rig = Rig::new("crash-partial-clone");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_ledger = rig.ledger_bytes();
    // §6.6's "during cloning" row: a fault after the first clone and before
    // the last leaves a partial `build/` and no published fact.
    TAMPER_CLONE.with(|cell| cell.set(Some(1)));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(
        matches!(refused, Err(LeaseError::UnsafePath(_))),
        "{refused:?}"
    );
    // Durable state: the cloned prefix, no sidecar for the recipient, and the
    // ledger still recording the held generation.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[1].0)
        .exists());
    assert!(rig
        .sidecars()
        .iter()
        .all(|name| !name.starts_with(&format!("{}.", grant.slot_id))));
    assert_eq!(rig.ledger_bytes(), before_ledger);
    // The next command cannot reuse or complete that generation through the
    // library: its `build/` is not empty.
    TAMPER_CLONE.with(|cell| cell.set(None));
    let again = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(again, Err(LeaseError::UnsafePath(_))), "{again:?}");
}

#[test]
fn crash_row_complete_seed_before_spawning_leaves_no_library_trust() {
    reset_hooks();
    let rig = Rig::new("crash-complete-seed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let before_sidecars = rig.sidecars();
    let before_ledger = rig.ledger_bytes();
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert!(matches!(outcome, SeedOutcome::Seeded(_)), "{outcome:?}");
    // §6.6's "after a complete seed, before mark_spawning" row: the cloned
    // tree is complete, and the library published no fact and wrote no trust:
    // the front door's attempt record is the only place a seed is claimed.
    assert_eq!(rig.sidecars(), before_sidecars);
    assert_eq!(rig.ledger_bytes(), before_ledger);
    // The completed-but-unclaimed generation is not offered as a donor while
    // the ledger still records it held.
    let (_, other_recipient) = lease_recipient(&rig, "b");
    let listed = candidates(&rig, &other_recipient);
    assert!(listed
        .candidates
        .iter()
        .all(|candidate| candidate.slot_id != grant.slot_id));
}

#[test]
fn crash_row_interrupted_catalog_write_is_inert_and_publish_is_atomic() {
    reset_hooks();
    let rig = Rig::new("crash-catalog-write");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let seeds = rig.pool.join(MANAGED_NAME).join("seeds");
    // A completed publish leaves exactly one whole catalog and no temp file.
    assert_eq!(
        rig.sidecars(),
        vec![format!("{}.catalog.json", donor.grant.slot_id)]
    );
    assert!(SeedCatalog::parse(
        &fs::read(seeds.join(format!("{}.catalog.json", donor.grant.slot_id)))
            .expect("catalog bytes")
    )
    .is_ok());

    // A half-written temp file left by an interrupted publish is inert: it is
    // not a catalog, so nothing offers the donor and nothing is touched.
    let temp = seeds.join("catalog.tmp-deadbeef");
    fs::write(&temp, b"{\"schema\":").expect("torn temp");
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).expect("temp mode");
    assert_eq!(candidates(&rig, &recipient).candidates.len(), 1);

    // A torn catalog at the real name (a crash cannot produce it through the
    // atomic rename, but the reader must still ignore it) offers nothing and
    // the seed skips without touching the recipient.
    let path = seeds.join(format!("{}.catalog.json", donor.grant.slot_id));
    let complete = fs::read(&path).expect("complete catalog");
    fs::write(&path, &complete[..complete.len() / 2]).expect("torn catalog");
    assert!(candidates(&rig, &recipient).candidates.is_empty());
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::CatalogChanged
        })
    );
    assert!(tree(&grant.env.cargo_build_build_dir).is_empty());
}

#[test]
fn crash_row_clone_interruption_before_the_call_retains_the_prefix() {
    reset_hooks();
    let rig = Rig::new("clone-interrupt-before");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let donor_tree = tree(&donor.grant.env.cargo_build_build_dir);
    let donor_catalog = fs::read(
        rig.pool
            .join(MANAGED_NAME)
            .join("seeds")
            .join(format!("{}.catalog.json", donor.grant.slot_id)),
    )
    .expect("donor catalog");
    let ledger = rig.ledger_bytes();
    let control = fs::read(rig.control().join("state.json")).expect("control state");
    // The clone operation itself is interrupted at the third file, before its
    // syscall creates an entry.
    CLONE_FAULT.with(|cell| cell.set(Some((2, false))));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(refused, Err(LeaseError::Io(_))), "{refused:?}");
    let build = &grant.env.cargo_build_build_dir;
    // The persisted state of an interruption mid-clone: the earlier clones only.
    assert!(build.join(&donor.files[0].0).exists());
    assert!(build.join(&donor.files[1].0).exists());
    assert!(!build.join(&donor.files[2].0).exists());
    // Unrelated state is untouched: the donor's own tree, its catalog, the
    // ledger and the front-door control state.
    assert_eq!(tree(&donor.grant.env.cargo_build_build_dir), donor_tree);
    assert_eq!(
        fs::read(
            rig.pool
                .join(MANAGED_NAME)
                .join("seeds")
                .join(format!("{}.catalog.json", donor.grant.slot_id))
        )
        .expect("donor catalog"),
        donor_catalog
    );
    assert_eq!(rig.ledger_bytes(), ledger);
    assert_eq!(
        fs::read(rig.control().join("state.json")).expect("control state"),
        control
    );
    // The persisted partial recipient is not offered as a donor and cannot be
    // seeded into again through the library.
    CLONE_FAULT.with(|cell| cell.set(None));
    let (_, other_recipient) = lease_recipient(&rig, "b");
    assert!(candidates(&rig, &other_recipient)
        .candidates
        .iter()
        .all(|candidate| candidate.slot_id != grant.slot_id));
    let again = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(again, Err(LeaseError::UnsafePath(_))), "{again:?}");
}

#[test]
fn crash_row_clone_interruption_after_the_call_leaves_an_untrusted_entry() {
    reset_hooks();
    let rig = Rig::new("clone-interrupt-after");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let ledger = rig.ledger_bytes();
    // The clone operation creates its entry and is then interrupted before the
    // clone is verified: an untrusted entry that is never published.
    CLONE_FAULT.with(|cell| cell.set(Some((1, true))));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(refused, Err(LeaseError::Io(_))), "{refused:?}");
    let build = &grant.env.cargo_build_build_dir;
    assert!(build.join(&donor.files[0].0).exists());
    assert!(build.join(&donor.files[1].0).exists());
    assert!(!build.join(&donor.files[2].0).exists());
    // No sidecar, no ledger fact, and the partial recipient is neither offered
    // as a donor nor seedable again through the library.
    assert!(rig
        .sidecars()
        .iter()
        .all(|name| !name.starts_with(&format!("{}.", grant.slot_id))));
    assert_eq!(rig.ledger_bytes(), ledger);
    CLONE_FAULT.with(|cell| cell.set(None));
    let (_, other_recipient) = lease_recipient(&rig, "b");
    assert!(candidates(&rig, &other_recipient)
        .candidates
        .iter()
        .all(|candidate| candidate.slot_id != grant.slot_id));
    let again = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(again, Err(LeaseError::UnsafePath(_))), "{again:?}");
}

#[test]
fn crash_row_catalog_interruption_before_the_rename_keeps_only_an_inert_temp() {
    reset_hooks();
    let rig = Rig::new("catalog-before-rename");
    let (grant, catalog) = held_catalog(&rig, "a");
    let seeds = rig.pool.join(MANAGED_NAME).join("seeds");
    let real = seeds.join(format!("{}.catalog.json", grant.slot_id));
    let ledger = rig.ledger_bytes();
    let control = fs::read(rig.control().join("state.json")).ok();
    // The temp is written and the rename has not happened: the real name stays
    // absent and the leftover temp is inert.
    CATALOG_FAULT.with(|cell| cell.set(Some(false)));
    let refused = publish_seed_catalog(&rig.pool, &catalog);
    assert!(matches!(refused, Err(LeaseError::Io(_))), "{refused:?}");
    assert!(!real.exists());
    assert_eq!(
        rig.sidecars()
            .iter()
            .filter(|name| name.starts_with("catalog.tmp-"))
            .count(),
        1
    );
    // Pre-rename state is not trusted: once the producing generation is idle,
    // the temp offers no candidate and the seed skips.
    assert_eq!(rig.ledger_bytes(), ledger);
    assert_eq!(fs::read(rig.control().join("state.json")).ok(), control);
    release_injected(&rig.pool, &grant.lease_id).expect("release donor");
    rig.donor_lock(catalog.record_id);
    let (_, recipient) = lease_recipient(&rig, "b");
    assert!(candidates(&rig, &recipient).candidates.is_empty());
    let request = SeedRequest {
        recipient: recipient.clone(),
        donor: SeedDonor {
            slot_id: grant.slot_id.clone(),
            incarnation: grant.incarnation.clone(),
            lease_id: grant.lease_id.clone(),
            record_id: catalog.record_id,
            catalog_digest: catalog_digest(&catalog.to_bytes().expect("catalog bytes")),
        },
        units: vec![SeedUnitRef {
            fingerprint_dir: FINGERPRINT.to_owned(),
            role: SeedRole::Lib,
        }],
        work_budget_ms: None,
    };
    let outcome = seed(&rig.pool, &request).expect("seed");
    assert_eq!(
        outcome,
        SeedOutcome::Skipped(SeedSkip {
            reason: SeedSkipReason::CatalogChanged
        })
    );
}

#[test]
fn crash_row_catalog_interruption_after_the_rename_leaves_the_whole_catalog() {
    reset_hooks();
    let rig = Rig::new("catalog-after-rename");
    let (grant, catalog) = held_catalog(&rig, "a");
    let seeds = rig.pool.join(MANAGED_NAME).join("seeds");
    let real = seeds.join(format!("{}.catalog.json", grant.slot_id));
    let ledger = rig.ledger_bytes();
    // The rename happened and the directory fsync was interrupted: the catalog
    // is whole at its real name and no temp file remains.
    CATALOG_FAULT.with(|cell| cell.set(Some(true)));
    let refused = publish_seed_catalog(&rig.pool, &catalog);
    assert!(matches!(refused, Err(LeaseError::Io(_))), "{refused:?}");
    assert!(SeedCatalog::parse(&fs::read(&real).expect("catalog bytes")).is_ok());
    assert!(!rig
        .sidecars()
        .iter()
        .any(|name| name.starts_with("catalog.tmp-")));
    // The write touched no ledger or control fact, and the post-rename catalog
    // is a candidate once the generation is idle.
    assert_eq!(rig.ledger_bytes(), ledger);
    release_injected(&rig.pool, &grant.lease_id).expect("release donor");
    let (_, recipient) = lease_recipient(&rig, "b");
    let listed = candidates(&rig, &recipient);
    assert_eq!(listed.candidates.len(), 1);
    assert_eq!(listed.candidates[0].slot_id, grant.slot_id);
}

#[test]
fn seed_refuses_a_donor_generation_that_changed_after_the_clone() {
    reset_hooks();
    let rig = Rig::new("generation-changed");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    TAMPER_LEDGER.with(|cell| cell.set(true));
    let refused = seed(&rig.pool, &donor.request(&recipient));
    assert!(matches!(refused, Err(LeaseError::UnknownAccounting)));
    // The clone is retained untrusted, and the slot is still held.
    assert!(grant
        .env
        .cargo_build_build_dir
        .join(&donor.files[0].0)
        .exists());
}

#[test]
fn seeding_never_reconciles_a_recorded_deletion() {
    reset_hooks();
    let rig = Rig::new("no-reconcile");
    // A donor with a published catalog, a second producing lease whose catalog
    // is published later, a recipient lease, and one unrelated idle slot.
    let donor = make_donor(&rig, "a");
    let (recipient_grant, recipient) = lease_recipient(&rig, "a");
    let second_record = rig.next_record_id();
    let second = rig.lease_at("donor-b", "donor", rig.affinity_dir(second_record));
    let second_files = default_files();
    write_files(&second.env.cargo_build_build_dir, &second_files);
    let second_catalog = catalog_for(&second, second_record, &second_files);
    let third = rig.lease("third", "third");
    release_injected(&rig.pool, &third.lease_id).expect("release third");
    let third_payload = third
        .env
        .cargo_build_build_dir
        .parent()
        .expect("slot dir")
        .to_path_buf();
    let third_id: u64 = third.slot_id.parse().expect("slot id");
    mark_evicting_for_test(&rig.pool, third_id).expect("mark evicting");
    let before_ledger = rig.ledger_bytes();
    assert!(third_payload.exists());

    // Neither a catalog write nor a seed may touch the queued deletion, the
    // ledger or the unrelated slot's payload.
    publish_seed_catalog(&rig.pool, &second_catalog).expect("publish second catalog");
    let outcome = seed(&rig.pool, &donor.request(&recipient)).expect("seed");
    assert!(matches!(outcome, SeedOutcome::Seeded(_)));
    assert_eq!(
        slot_state_injected(&rig.pool, &third.slot_id).as_deref(),
        Some("evicting")
    );
    assert!(third_payload.exists());
    assert_eq!(rig.ledger_bytes(), before_ledger);
    let _ = (&recipient_grant, second_record);
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
    // A ceiling the new catalog cannot meet refuses the write, and no catalog
    // is destroyed by the refusal.
    FAMILY_CAP.with(|cell| cell.set(Some(1)));
    let fifth_record = rig.next_record_id();
    let fifth = rig.lease_at("donor-e", "donor", rig.affinity_dir(fifth_record));
    let files = default_files();
    write_files(&fifth.env.cargo_build_build_dir, &files);
    let refused = publish_seed_catalog(&rig.pool, &catalog_for(&fifth, fifth_record, &files));
    assert!(matches!(refused, Err(LeaseError::LedgerExhausted)));
    assert!(!seeds
        .join(format!("{}.catalog.json", fifth.slot_id))
        .exists());
    assert!(seeds
        .join(format!("{}.catalog.json", fourth.grant.slot_id))
        .exists());
    release_injected(&rig.pool, &fifth.lease_id).expect("release fifth");
    let _ = (&first, &second);
}

#[test]
fn join_parser_accepts_rustc_shapes_and_refuses_others() {
    reset_hooks();
    let parsed = join::parse(dep_info_text().as_bytes()).expect("parse the fixture");
    assert!(parsed.sets_match(&dep_targets(), &dep_inputs()));
    // The artifact has its own single-target rule with nonempty, recorded
    // prerequisites.
    assert!(parsed.artifact_rule_proven(ARTIFACT, &dep_inputs()));
    assert_eq!(parsed.rules.len(), 4);
    // A flattened rule names several targets: it proves no artifact.
    let flattened = join::parse(
        format!("{DEP} {ARTIFACT}: {SOURCE_A} {SOURCE_B}\n{SOURCE_A}:\n{SOURCE_B}:\n").as_bytes(),
    )
    .expect("flattened");
    assert!(!flattened.artifact_rule_proven(ARTIFACT, &dep_inputs()));
    // A rule with no prerequisites proves no artifact either.
    let empty = join::parse(format!("{ARTIFACT}:\n").as_bytes()).expect("empty rule");
    assert!(!empty.artifact_rule_proven(ARTIFACT, &dep_inputs()));
    // An artifact with no rule at all.
    assert!(
        !parsed.artifact_rule_proven("debug/deps/libcfg_if-bbbbbbbbbbbbbbbb.rmeta", &dep_inputs())
    );
    // Duplicate and conflicting artifact rules never prove the artifact.
    let duplicate =
        join::parse(format!("{ARTIFACT}: {SOURCE_A}\n{ARTIFACT}: {SOURCE_B}\n").as_bytes())
            .expect("duplicate rules");
    assert!(duplicate.sole_rule_for(ARTIFACT).is_none());
    assert!(!duplicate.artifact_rule_proven(ARTIFACT, &dep_inputs()));
    let conflicting = join::parse(
        format!("{ARTIFACT}: {SOURCE_A}\n{ARTIFACT}: {SOURCE_A} {SOURCE_B}\n").as_bytes(),
    )
    .expect("conflicting rules");
    assert!(conflicting.sole_rule_for(ARTIFACT).is_none());
    assert!(!conflicting.artifact_rule_proven(ARTIFACT, &dep_inputs()));
    let sole =
        join::parse(format!("{ARTIFACT}: {SOURCE_A} {SOURCE_B}\n").as_bytes()).expect("sole rule");
    assert!(sole.sole_rule_for(ARTIFACT).is_some());
    assert!(sole.artifact_rule_proven(ARTIFACT, &dep_inputs()));

    // Escaped space, escaped colon, literal `$` and a continued line.
    let text = "out/one\\ two: /src/a\\ b.rs /src/c.rs \\\n /src/d.rs\n/srv/\\$x: /src/e.rs\n";
    let parsed = join::parse(text.as_bytes()).expect("escapes");
    assert!(parsed.targets.contains(&"out/one two".to_owned()));
    assert!(parsed.targets.contains(&"/srv/$x".to_owned()));
    assert!(parsed.inputs.contains(&"/src/a b.rs".to_owned()));
    assert!(parsed.inputs.contains(&"/src/d.rs".to_owned()));

    assert_eq!(join::parse(b"no rule here\n"), Err(JoinRefusal::Malformed));
    assert_eq!(join::parse(b"out: in\n: in\n"), Err(JoinRefusal::Malformed));
    assert_eq!(join::parse(b"out: $x\n"), Err(JoinRefusal::Malformed));
    assert_eq!(
        join::parse(b"out: /src/a.rs\nbad rule\n"),
        Err(JoinRefusal::Malformed)
    );
    assert_eq!(join::parse(&[0xff]), Err(JoinRefusal::NotUtf8));
    assert!(matches!(
        join::parse(&vec![b' '; join::MAX_JOIN_BYTES + 1]),
        Err(JoinRefusal::TooLarge)
    ));

    // Rule and input counts are bounded.
    let many_rules = "out: in\n".repeat(join::MAX_JOIN_TARGETS + 1);
    assert_eq!(join::parse(many_rules.as_bytes()), Err(JoinRefusal::Budget));
    // The per-unit input bound is exactly 4,096: one more is a budget refusal
    // and exactly the bound parses.
    let many_inputs = format!("out: {}\n", "a ".repeat(join::MAX_JOIN_INPUTS + 1));
    assert_eq!(
        join::parse(many_inputs.as_bytes()),
        Err(JoinRefusal::Budget)
    );
    let at_input_cap = format!("out: {}\n", "a ".repeat(join::MAX_JOIN_INPUTS));
    let parsed = join::parse(at_input_cap.as_bytes()).expect("input cap");
    assert_eq!(parsed.inputs.len(), join::MAX_JOIN_INPUTS);
}

#[test]
fn publish_refuses_when_a_cap_drop_cannot_be_removed() {
    reset_hooks();
    let rig = Rig::new("cap-delete-failure");
    let first = make_donor(&rig, "a");
    let seeds = rig.pool.join(MANAGED_NAME).join("seeds");
    let first_catalog = fs::metadata(seeds.join(format!("{}.catalog.json", first.grant.slot_id)))
        .expect("first catalog")
        .len();
    // A second producing lease, still held, whose catalog would need the first
    // one dropped to fit the family ceiling.
    let record_id = rig.next_record_id();
    let second = rig.lease_at("donor-b", "donor", rig.affinity_dir(record_id));
    let files = default_files();
    write_files(&second.env.cargo_build_build_dir, &files);
    let second_catalog = catalog_for(&second, record_id, &files);
    FAMILY_CAP.with(|cell| cell.set(Some(first_catalog + 8)));
    // The drop cannot be performed: the write must refuse rather than credit
    // space the failed deletion did not free.
    fs::set_permissions(&seeds, fs::Permissions::from_mode(0o500)).expect("read-only seeds");
    let refused = publish_seed_catalog(&rig.pool, &second_catalog);
    fs::set_permissions(&seeds, fs::Permissions::from_mode(0o700)).expect("writable seeds");
    assert!(
        matches!(refused, Err(LeaseError::LedgerExhausted)),
        "{refused:?}"
    );
    assert!(seeds
        .join(format!("{}.catalog.json", first.grant.slot_id))
        .exists());
    assert!(!seeds
        .join(format!("{}.catalog.json", second.slot_id))
        .exists());
    release_injected(&rig.pool, &second.lease_id).expect("release second");
}

#[test]
fn catalog_schema_rejects_unknown_fields_and_unusable_paths() {
    reset_hooks();
    let rig = Rig::new("schema");
    let (grant, _) = lease_recipient(&rig, "a");
    let files = default_files();
    write_files(&grant.env.cargo_build_build_dir, &files);
    let record_id = grant.slot_id.parse::<u64>().expect("record id");
    let catalog = catalog_for(&grant, record_id, &files);
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
    duplicate.packages.push(second);
    assert!(duplicate.validate().is_err());
    let mut wrong_profile = catalog.clone();
    wrong_profile.packages[0].units[0].files[0].path =
        "release/.fingerprint/cfg-if-aaaaaaaaaaaaaaaa/lib-cfg_if".to_owned();
    assert!(wrong_profile.validate().is_err());
    // The join's own digest is its `.d` file record's digest.
    let mut join_digest = catalog.clone();
    join_digest.packages[0].units[0].join.digest = "f".repeat(64);
    assert!(join_digest.validate().is_err());
    // The unit's one dep-info file is its join.
    let mut join_kind = catalog.clone();
    for file in &mut join_kind.packages[0].units[0].files {
        if file.path == DEP {
            file.kind = SeedFileKind::Artifact;
        }
    }
    assert!(join_kind.validate().is_err());
    // A producer cannot opt an artifact out by calling it a fingerprint file,
    // and a fingerprint file cannot pose as an artifact.
    let mut artifact_as_fingerprint = catalog.clone();
    for file in &mut artifact_as_fingerprint.packages[0].units[0].files {
        if file.path == ARTIFACT {
            file.kind = SeedFileKind::Fingerprint;
        }
    }
    assert!(artifact_as_fingerprint.validate().is_err());
    let mut fingerprint_as_artifact = catalog.clone();
    for file in &mut fingerprint_as_artifact.packages[0].units[0].files {
        if file.path.ends_with("/dep-lib-cfg_if") {
            file.kind = SeedFileKind::Artifact;
        }
    }
    assert!(fingerprint_as_artifact.validate().is_err());
    // A unit with no rustc artifact at all is not offered.
    let mut no_artifact = catalog.clone();
    let unit = &mut no_artifact.packages[0].units[0];
    unit.files.retain(|file| file.path != ARTIFACT);
    unit.join.targets.retain(|target| target != ARTIFACT);
    assert!(no_artifact.validate().is_err());
    // A unit's `.d` may record at most 4,096 inputs: one more is invalid and
    // exactly the bound is accepted.
    let mut too_many_inputs = catalog.clone();
    too_many_inputs.packages[0].units[0].join.inputs = (0..=SEED_MAX_UNIT_INPUTS)
        .map(|i| format!("/fixture/src/{i}.rs"))
        .collect();
    assert!(too_many_inputs.validate().is_err());
    let mut at_input_cap = catalog.clone();
    at_input_cap.packages[0].units[0].join.inputs = (0..SEED_MAX_UNIT_INPUTS)
        .map(|i| format!("/fixture/src/{i}.rs"))
        .collect();
    assert!(at_input_cap.validate().is_ok());
    // The catalog's own byte ceiling is enforced before parsing.
    assert!(SeedCatalog::parse(&vec![b' '; SEED_CATALOG_MAX_BYTES + 1]).is_err());
}

#[test]
fn join_accepts_absolute_donor_outputs_and_rejects_ambiguous_aliases() {
    let root = Path::new("/fixture/pool/slot/build");
    let source = "/fixture/source/lib.rs";
    let dep = "debug/deps/cfg_if-aaaaaaaaaaaaaaaa.d";
    let artifact = "debug/deps/libcfg_if-aaaaaaaaaaaaaaaa.rmeta";
    let contents = format!(
        "{}/{dep}: {source}\n{}/{artifact}: {source}\n{source}:\n# env-dep:EXAMPLE=value\n",
        root.display(),
        root.display(),
    );
    let parsed = join::parse(contents.as_bytes()).expect("absolute rustc dep-info");
    let inputs = vec![source.to_owned()];
    assert!(join::output_rule_proven(&parsed, root, dep, &inputs));
    assert!(join::output_rule_proven(&parsed, root, artifact, &inputs));
    let duplicate = format!("{contents}{artifact}: {source}\n");
    let parsed = join::parse(duplicate.as_bytes()).expect("duplicate rules parse");
    assert!(!join::output_rule_proven(&parsed, root, artifact, &inputs));
    assert!(!join::output_rule_proven(
        &parsed,
        Path::new("/other"),
        dep,
        &inputs
    ));
}

#[test]
fn monitor_stop_after_a_recipient_write_refuses_and_keeps_the_write() {
    reset_hooks();
    let rig = Rig::new("monitor-stop");
    let donor = make_donor(&rig, "a");
    let (grant, recipient) = lease_recipient(&rig, "a");
    let fingerprint = grant.env.cargo_build_build_dir.join(FINGERPRINT);
    let mut monitor = || fingerprint.exists();
    let result = seed_monitored(&rig.pool, &donor.request(&recipient), &mut monitor);
    assert!(
        matches!(result, Err(LeaseError::Io(ref reason)) if reason.contains("monitor stopped"))
    );
    assert!(fingerprint.exists());
}

#[test]
fn catalog_publication_requires_matching_unseeded_attempt() {
    reset_hooks();
    let rig = Rig::new("attempt-catalog");
    let (grant, mut catalog) = held_catalog(&rig, "a");
    let mut attempt = SeedAttempt {
        schema: ATTEMPT_SCHEMA.into(),
        store_id: grant.store_id.clone(),
        slot_id: grant.slot_id.clone(),
        incarnation: grant.incarnation.clone(),
        lease_id: grant.lease_id.clone(),
        state: SeedAttemptState::Intent,
        donor_slot: None,
        units: None,
        files: None,
        logical_bytes: None,
        catalog_digest: None,
        observation: None,
    };
    write_seed_attempt(&rig.pool, &attempt).expect("intent");
    assert!(publish_seed_catalog(&rig.pool, &catalog).is_err());
    attempt.state = SeedAttemptState::Skipped;
    write_seed_attempt(&rig.pool, &attempt).expect("terminal skip");
    assert!(publish_seed_catalog(&rig.pool, &catalog).is_err());
    catalog.attempt = SeedAttemptState::Skipped;
    publish_seed_catalog(&rig.pool, &catalog).expect("unseeded terminal generation");
    assert!(write_seed_attempt(&rig.pool, &attempt).is_err());
}
