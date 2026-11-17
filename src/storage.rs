//! Oxide's storage authority: the lean Barge library.
//!
//! Barge owns registered volumes and their identity, declared areas, marked
//! roots, coopertive holds, durable pins, byte-pressure pruning, idle
//! least-recently-used eviction, marker-checked trash and scratch. Oxide keeps
//! its Cargo front door, slot compatibility and freshness, lease workflow
//! state, recipes, seeding policy, output member links and `exec`.
//!
//! This module is the one place Oxide turns its own facts (a pool's path, a
//! registered removable drive, the outputs location, a budget) into Barge
//! declarations, and the one place it supplies Barge's platform seam. Barge
//! reads no Oxide format and Oxide forges no Barge marker.
//!
//! Owning contract: `docs/knowledge/contracts/barge-adoption.md`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use barge::{
    Absent, Area, AreaSpec, Barge, Hold, Location, Mode, PruneReport, Space, SweepReport, Volume,
    VolumeRegistration, VolumeState, VolumeStatus,
};

/// The namespace every Oxide area lives under.
pub const NAMESPACE: &str = "oxide";
/// The pool area's name.
pub const POOL_AREA: &str = "pool";
/// The output-roots area's name.
pub const OUTPUTS_AREA: &str = "outputs";

/// The folder, inside a volume's base, that holds the pool's slot roots. It is
/// the existing layout (`<mount>/oxide-pool/slots`, `<pool>/payload/slots`) so
/// adoption moves nothing.
pub const POOL_FOLDER: &str = "slots";
/// The folder that holds the per-checkout output roots (`oxide-outputs`).
pub const OUTPUTS_FOLDER: &str = "oxide-outputs";
/// The folder that holds the pool payload inside a pool directory.
pub const PAYLOAD_FOLDER: &str = "payload";
/// The registered volume's payload folder on a removable drive.
pub const VOLUME_PAYLOAD_FOLDER: &str = "oxide-pool";

/// One area Oxide declares: the volume it belongs to, where its roots go, the
/// absent-volume policy and the space policy.
#[derive(Clone, Debug)]
pub struct AreaConfig {
    /// The Barge volume name.
    pub volume: String,
    /// The mount the volume's identity is read at: a removable drive's mount
    /// point, or the pool/outputs directory itself for an internal area.
    pub mount: PathBuf,
    /// The identity a removable drive is registered under, when Oxide already
    /// recorded one. Barge reads the mount when this is `None`.
    pub uuid: Option<String>,
    /// Folder inside the mount that holds Barge's areas (`""` is the mount).
    pub root_dir: String,
    /// The area's name inside the `oxide` namespace.
    pub area: String,
    /// The area's folder inside the volume base.
    pub folder: String,
    /// Where new roots go while the volume is absent, and where roots may
    /// always be found. Never a silent move: the caller reports it.
    pub fallback: Option<PathBuf>,
    /// The area's soft cap in bytes.
    pub soft_cap_bytes: Option<u64>,
    /// Below this, a prune evicts least-recently-used idle unpinned roots.
    pub reserve_bytes: u64,
    /// Below this, an admission refuses once nothing idle is left.
    pub floor_bytes: u64,
}

impl AreaConfig {
    /// A declaration that fails when its volume is absent.
    pub fn new(volume: impl Into<String>, mount: impl Into<PathBuf>, area: &str) -> Self {
        Self {
            volume: volume.into(),
            mount: mount.into(),
            uuid: None,
            root_dir: String::new(),
            area: area.to_owned(),
            folder: area.to_owned(),
            fallback: None,
            soft_cap_bytes: None,
            reserve_bytes: 0,
            floor_bytes: barge::volume::DEFAULT_FLOOR_BYTES,
        }
    }

    pub fn uuid(mut self, uuid: Option<String>) -> Self {
        self.uuid = uuid;
        self
    }

    pub fn root_dir(mut self, root_dir: impl Into<String>) -> Self {
        self.root_dir = root_dir.into();
        self
    }

    pub fn folder(mut self, folder: impl Into<String>) -> Self {
        self.folder = folder.into();
        self
    }

    pub fn fallback(mut self, fallback: Option<PathBuf>) -> Self {
        self.fallback = fallback;
        self
    }

    pub fn soft_cap_bytes(mut self, soft_cap_bytes: Option<u64>) -> Self {
        self.soft_cap_bytes = soft_cap_bytes;
        self
    }

    pub fn reserve_bytes(mut self, reserve_bytes: u64) -> Self {
        self.reserve_bytes = reserve_bytes;
        self
    }

    pub fn floor_bytes(mut self, floor_bytes: u64) -> Self {
        self.floor_bytes = floor_bytes;
        self
    }

    fn key(&self) -> String {
        format!("{NAMESPACE}/{}", self.area)
    }
}

/// Barge's platform seam with Oxide's one extra fact: a stale `/Volumes/<name>`
/// directory left on the boot disk is not a volume.
///
/// Barge's own `volume_identity` answers the UUID of the filesystem that holds
/// any path, so on a directory left behind by an unclean detach it would report
/// the boot volume's UUID and Barge would call that a different volume at the
/// registered mount (a refusal). Oxide's absence protocol already distinguishes
/// the two: a registered drive is present only when the path is that volume's
/// mounted root. This seam applies exactly that rule to drive mount points and
/// leaves every other path to Barge: an internal pool, one-use pool or outputs
/// directory is a plain directory whose containing filesystem identity is the
/// wanted fact.
#[derive(Debug, Clone, Copy, Default)]
pub struct OxideOs;

impl barge::Os for OxideOs {
    fn volume_identity(&self, mount: &Path) -> barge::Result<Option<String>> {
        if is_drive_mount_point(mount) {
            return Ok(match crate::volume::observe_path(mount) {
                Ok(facts) if facts.mounted_on == mount => facts.uuid,
                // A stale mount point, or one that cannot be read: absent, so
                // the area's declared fallback is used and reported.
                _ => None,
            });
        }
        barge::os::platform().volume_identity(mount)
    }

    fn free_bytes(&self, path: &Path) -> barge::Result<u64> {
        barge::os::platform().free_bytes(path)
    }
}

/// Whether `path` is a removable drive's mount point shape, `/Volumes/<name>`.
///
/// A stale directory left there by an unclean detach is not a mounted volume,
/// which is the one fact Barge's platform seam cannot see for itself.
pub fn is_drive_mount_point(path: &Path) -> bool {
    let mut components = path.components();
    matches!(
        (
            components.next(),
            components.next(),
            components.next(),
            components.next()
        ),
        (
            Some(std::path::Component::RootDir),
            Some(std::path::Component::Normal(parent)),
            Some(std::path::Component::Normal(_)),
            None
        ) if parent == "Volumes"
    )
}

/// Barge's account state plus Oxide's seam.
#[derive(Debug, Clone)]
pub struct Storage {
    barge: Barge,
}

impl Storage {
    /// Open the account's Barge state stored under Oxide's own state root.
    pub fn open(state_root: &Path) -> Result<Self, String> {
        let state_dir = state_root.join("barge");
        Barge::open_with(state_dir, Arc::new(OxideOs), Arc::new(barge::SystemClock::new()))
            .map(|barge| Self { barge })
            .map_err(|error| format!("cannot open the storage state: {error}"))
    }

    /// Open with explicit seams, for Oxide's own tests.
    pub fn open_with(
        state_dir: PathBuf,
        os: Arc<dyn barge::Os>,
        clock: Arc<dyn barge::Clock>,
    ) -> Result<Self, String> {
        Barge::open_with(state_dir, os, clock)
            .map(|barge| Self { barge })
            .map_err(|error| format!("cannot open the storage state: {error}"))
    }

    /// The Barge library behind this authority, for callers that use its
    /// types directly.
    pub fn barge(&self) -> &Barge {
        &self.barge
    }

    pub fn state_dir(&self) -> &Path {
        self.barge.state_dir()
    }

    /// Register (or re-register) a volume and return it.
    pub fn register_volume(
        &self,
        name: &str,
        mount: &Path,
        uuid: Option<&str>,
        root_dir: &str,
        reserve_bytes: u64,
        floor_bytes: u64,
    ) -> Result<Volume, String> {
        let registration = VolumeRegistration::new(name, mount)
            .uuid(uuid)
            .root_dir(root_dir)
            .reserve_bytes(reserve_bytes)
            .floor_bytes(floor_bytes);
        self.barge
            .register_volume(registration)
            .map_err(|error| format!("cannot register volume {name}: {error}"))
    }

    /// One registered volume by name.
    pub fn volume(&self, name: &str) -> Result<Volume, String> {
        self.barge
            .volume(name)
            .map_err(|error| format!("volume {name}: {error}"))
    }

    /// Declare one area (recording it the first time, and refusing a
    /// declaration that differs from the stored one).
    ///
    /// The volume is registered only when the account does not know it yet: a
    /// drive that is absent at mount time keeps its recorded registration, so
    /// its area still declares and its fallback still works while the drive is
    /// away. A drive that comes back under a different identity is refused
    /// when Barge checks its state, never silently re-registered.
    pub fn declare(&self, config: &AreaConfig) -> Result<Area, String> {
        if self.volume(&config.volume).is_err() {
            self.register_volume(
                &config.volume,
                &config.mount,
                config.uuid.as_deref(),
                &config.root_dir,
                config.reserve_bytes,
                config.floor_bytes,
            )?;
        }
        let absent = match &config.fallback {
            Some(path) => Absent::Fallback {
                path: path.clone(),
            },
            None => Absent::Fail,
        };
        let spec = AreaSpec::new(NAMESPACE, config.area.clone(), config.volume.clone())
            .folder(config.folder.clone())
            .absent(absent)
            .soft_cap_bytes(config.soft_cap_bytes)
            .counts_toward_reserve(true);
        self.barge.area(spec).map_err(|error| {
            format!(
                "cannot declare area {} on volume {}: {error}",
                config.key(),
                config.volume
            )
        })
    }

    /// Where an area's new roots go right now.
    pub fn location(&self, config: &AreaConfig) -> Result<Location, String> {
        self.declare(config)?
            .location()
            .map_err(|error| format!("area {}: {error}", config.key()))
    }

    /// The registered volume if it is known, without reading or changing it.
    pub fn known_volume(&self, name: &str) -> Result<Volume, String> {
        self.volume(name)
    }

    /// Every area Oxide has declared, for status and reconciliation.
    pub fn areas(&self) -> Result<Vec<Area>, String> {
        self.barge
            .areas()
            .map_err(|error| format!("cannot read the declared areas: {error}"))
    }

    /// Free space, reserve and floor for a volume.
    pub fn space(&self, volume: &str) -> Result<Space, String> {
        self.barge
            .space(volume)
            .map_err(|error| format!("cannot read the space policy of {volume}: {error}"))
    }

    /// Prune a volume: each area's soft cap, then least-recently-used idle
    /// unpinned roots until free space is back above the reserve.
    pub fn prune(&self, volume: &str, dry_run: bool) -> Result<PruneReport, String> {
        self.barge
            .prune(volume, dry_run)
            .map_err(|error| format!("cannot prune {volume}: {error}"))
    }

    /// Admit work on a volume: prune, then refuse only below the floor.
    pub fn admit(&self, volume: &str) -> Result<PruneReport, String> {
        self.barge
            .admit(volume)
            .map_err(|error| format!("cannot admit work on {volume}: {error}"))
    }

    /// Delete every owned trash entry of every area on a volume. A crash
    /// between a rename and its deletion leaves trash, never ambiguity.
    pub fn sweep_trash(&self, volume: &str) -> Result<SweepReport, String> {
        self.barge
            .sweep_trash(volume)
            .map_err(|error| format!("cannot sweep the trash on {volume}: {error}"))
    }

    /// Barge's status document for every registered volume.
    pub fn status(&self) -> Result<Vec<VolumeStatus>, String> {
        self.barge
            .status()
            .map_err(|error| format!("cannot read the storage status: {error}"))
    }
}

/// Read a root's hold without waiting. `None` means something else holds it.
pub fn try_hold(area: &Area, id: &str, mode: Mode) -> Result<Option<Hold>, String> {
    area.try_hold(id, mode)
        .map_err(|error| format!("cannot hold root {id}: {error}"))
}

/// The volume state a caller reports: present, absent, or a different drive.
pub fn volume_state(volume: &Volume) -> Result<VolumeState, String> {
    volume
        .state()
        .map_err(|error| format!("cannot read the state of volume {}: {error}", volume.name()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use barge::testing::TempDir;
    use barge::{FakeOs, ManualClock, Mode, Os};

    const UUID: &str = "8E4A5C1D-0000-4000-8000-00000000BEEF";

    struct Fixture {
        _temp: TempDir,
        storage: Storage,
        os: FakeOs,
        clock: Arc<ManualClock>,
        mount: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let temp = TempDir::new(label).expect("temp dir");
            let mount = temp.path().join("Builds");
            std::fs::create_dir_all(&mount).expect("mount");
            let os = FakeOs::with_volume(mount.clone(), UUID, 100 << 30);
            let clock = Arc::new(ManualClock::new(1_000));
            let storage = Storage::open_with(
                temp.path().join("state"),
                Arc::new(os.clone()),
                clock.clone(),
            )
            .expect("storage");
            Self {
                _temp: temp,
                storage,
                os,
                clock,
                mount,
            }
        }

        fn pool(&self) -> AreaConfig {
            AreaConfig::new("Builds", self.mount.clone(), POOL_AREA)
                .folder(VOLUME_PAYLOAD_FOLDER.to_owned() + "/" + POOL_FOLDER)
                .soft_cap_bytes(Some(10 << 30))
                .reserve_bytes(20 << 30)
                .floor_bytes(10 << 30)
        }
    }

    #[test]
    fn a_declared_area_puts_roots_on_the_volume_with_its_marker() {
        let fixture = Fixture::new("storage-declare");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let root = area.create_root("quick").expect("root");
        assert!(root.path.starts_with(
            fixture
                .mount
                .join(VOLUME_PAYLOAD_FOLDER)
                .join(POOL_FOLDER)
        ));
        assert!(root.path.join(".barge-root.json").is_file());
        assert_eq!(
            fixture.storage.location(&fixture.pool()).expect("location"),
            Location::Volume(
                fixture
                    .mount
                    .join(VOLUME_PAYLOAD_FOLDER)
                    .join(POOL_FOLDER)
            )
        );
        let listing = area.list().expect("list");
        assert_eq!(listing.roots.len(), 1);
        assert!(listing.stragglers.is_empty());
    }

    #[test]
    fn a_second_declaration_that_differs_is_refused_by_name() {
        let fixture = Fixture::new("storage-differ");
        fixture.storage.declare(&fixture.pool()).expect("area");
        let changed = fixture.pool().soft_cap_bytes(Some(1 << 30));
        let error = fixture.storage.declare(&changed).expect_err("refused");
        assert!(error.contains("soft_cap_bytes"), "{error}");
    }

    #[test]
    fn a_hold_blocks_eviction_and_a_released_hold_allows_it() {
        let fixture = Fixture::new("storage-hold");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let root = area.create_root("quick").expect("root");
        let hold = area.hold(&root.id, Mode::Exclusive).expect("hold");
        let error = area.evict(&root.id).expect_err("held");
        assert!(error.message().contains("held"), "{error}");
        assert!(
            area.try_hold(&root.id, Mode::Exclusive)
                .expect("try")
                .is_none(),
            "a second exclusive hold is refused"
        );
        drop(hold);
        let eviction = area.evict(&root.id).expect("evicted");
        assert!(!root.path.exists());
        assert!(!eviction.trash_path.exists() || eviction.deleted);
        assert!(area.list().expect("list").roots.is_empty());
    }

    #[test]
    fn a_pin_blocks_eviction_until_it_is_removed() {
        let fixture = Fixture::new("storage-pin");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let root = area.create_root("quick").expect("root");
        area.pin(&root.id, "selected output").expect("pin");
        let error = area.evict(&root.id).expect_err("pinned");
        assert!(error.message().contains("pinned"), "{error}");
        let listing = area.list().expect("list");
        assert!(listing.roots[0].pinned);
        area.unpin(&root.id).expect("unpin");
        area.evict(&root.id).expect("evicted");
    }

    #[test]
    fn an_absent_volume_uses_the_declared_fallback_and_reports_it() {
        let fixture = Fixture::new("storage-fallback");
        let fallback = fixture
            ._temp
            .path()
            .join("Library/Caches/oxide-pool")
            .join(PAYLOAD_FOLDER)
            .join(POOL_FOLDER);
        let pool = fixture.pool().fallback(Some(fallback.clone()));
        fixture.storage.declare(&pool).expect("area");
        let present = fixture.storage.location(&pool).expect("location");
        assert!(present.on_volume());
        // The drive goes away, exactly as an unclean detach leaves it: nothing
        // is mounted at the registered mount point.
        fixture.os.set_uuid(&fixture.mount, None);
        assert_eq!(
            volume_state(&fixture.storage.volume("Builds").expect("volume")).expect("state"),
            VolumeState::Absent
        );
        assert_eq!(
            fixture.storage.location(&pool).expect("location"),
            Location::Fallback(fallback.clone())
        );
        let area = fixture.storage.declare(&pool).expect("area");
        let root = area.create_root("quick").expect("root");
        assert!(root.path.starts_with(&fallback));
        // The fallback root is still listed after the volume returns.
        fixture.os.set_uuid(&fixture.mount, Some(UUID));
        let listing = area.list().expect("list");
        assert_eq!(listing.roots.len(), 1);
        assert!(!listing.roots[0].on_volume);
    }

    #[test]
    fn a_different_volume_at_the_registered_mount_is_refused_not_absent() {
        let fixture = Fixture::new("storage-mismatch");
        let pool = fixture.pool();
        fixture.storage.declare(&pool).expect("area");
        fixture
            .os
            .set_uuid(&fixture.mount, Some("11111111-2222-4333-8444-555555555555"));
        let state = volume_state(&fixture.storage.volume("Builds").expect("volume")).expect("state");
        assert!(matches!(state, VolumeState::Mismatch { .. }), "{state:?}");
        assert!(matches!(
            fixture.storage.location(&pool).expect("location"),
            Location::Unavailable(reason) if reason.contains("not the registered")
        ));
    }

    #[test]
    fn a_reserve_prunes_the_least_recently_used_idle_unpinned_root() {
        let fixture = Fixture::new("storage-prune");
        let pool = fixture.pool();
        let area = fixture.storage.declare(&pool).expect("area");
        let older = area.create_root("quick").expect("root");
        std::fs::write(older.path.join("file"), vec![0_u8; 8192]).expect("write");
        fixture.clock.advance(1_000);
        let newer = area.create_root("quick").expect("root");
        fixture.clock.advance(1_000);
        let reserve = 20_u64 << 30;
        let older_bytes = area
            .list()
            .expect("list")
            .get(&older.id)
            .map(|root| root.bytes)
            .expect("older");
        assert!(older_bytes > 0);
        // Just below the reserve: evicting the older root alone returns above it.
        fixture.os.set_free(&fixture.mount, reserve - older_bytes);
        let report = fixture.storage.prune("Builds", false).expect("prune");
        assert_eq!(report.evicted.len(), 1, "{report:?}");
        assert_eq!(report.evicted[0].id, older.id);
        assert!(!older.path.exists());
        assert!(newer.path.exists());
    }

    #[test]
    fn an_admission_refuses_only_below_the_floor_with_nothing_left_to_evict() {
        let fixture = Fixture::new("storage-floor");
        let pool = fixture.pool();
        let area = fixture.storage.declare(&pool).expect("area");
        let root = area.create_root("quick").expect("root");
        let _hold = area.hold(&root.id, Mode::Exclusive).expect("hold");
        fixture.os.set_free(&fixture.mount, 1 << 30);
        let error = fixture.storage.admit("Builds").expect_err("floor");
        assert!(error.contains("floor"), "{error}");
        // Free space above the reserve admits without eviction.
        fixture.os.set_free(&fixture.mount, 40 << 30);
        let report = fixture.storage.admit("Builds").expect("admit");
        assert!(report.evicted.is_empty(), "{report:?}");
        assert!(root.path.exists());
    }

    #[test]
    fn an_area_soft_cap_evicts_its_own_idle_roots_only() {
        let fixture = Fixture::new("storage-cap");
        let pool = fixture.pool().soft_cap_bytes(Some(1 << 20));
        let area = fixture.storage.declare(&pool).expect("area");
        let root = area.create_root("quick").expect("root");
        std::fs::write(root.path.join("file"), vec![0_u8; 2 << 20]).expect("write");
        fixture.clock.advance(1_000);
        let report = fixture.storage.prune("Builds", true).expect("prune");
        assert_eq!(report.evicted.len(), 1, "{report:?}");
        assert_eq!(report.evicted[0].reason, barge::PruneReason::SoftCap);
        // A dry run changes nothing.
        assert!(root.path.exists());
    }

    #[test]
    fn the_trash_sweep_deletes_owned_entries_and_keeps_foreign_ones() {
        let fixture = Fixture::new("storage-trash");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let root = area.create_root("quick").expect("root");
        let parent = root.path.parent().expect("parent").to_path_buf();
        area.evict(&root.id).expect("evicted");
        // A foreign folder in the trash (no marker) is reported, never deleted.
        let foreign = parent.join(".trash/foreign");
        std::fs::create_dir_all(&foreign).expect("foreign");
        std::fs::write(foreign.join("keep"), b"keep").expect("write");
        let report = fixture.storage.sweep_trash("Builds").expect("sweep");
        assert!(report.deleted.is_empty());
        assert_eq!(report.kept.len(), 1);
        assert!(foreign.join("keep").exists());
    }

    #[test]
    fn the_platform_seam_treats_a_stale_drive_directory_as_absent() {
        // A `/Volumes/<name>` path that is not a mounted root is the stale
        // directory an unclean detach leaves behind. Nothing is created here:
        // the path is a probe, and the seam answers absent without touching it.
        let stale = Path::new("/Volumes/oxide-storage-seam-probe-not-mounted");
        assert!(is_drive_mount_point(stale));
        assert_eq!(
            OxideOs.volume_identity(stale).expect("identity"),
            None,
            "a stale mount point is absent, never a different volume"
        );
        assert!(!is_drive_mount_point(Path::new(
            "/Volumes/Builds/scratch/oxide/run"
        )));
        assert!(!is_drive_mount_point(Path::new("/private/tmp/run")));
    }

    #[test]
    fn an_internal_pool_is_a_volume_at_its_own_directory() {
        let fixture = Fixture::new("storage-internal");
        let pool_dir = fixture._temp.path().join("pool");
        std::fs::create_dir_all(&pool_dir).expect("pool");
        // The fake seam answers for the containing filesystem, so the pool's
        // own directory is a present volume and its area is a plain folder.
        fixture
            .os
            .add_volume(pool_dir.clone(), Some(UUID), 100 << 30);
        let config = AreaConfig::new("internal", pool_dir.clone(), POOL_AREA)
            .folder(PAYLOAD_FOLDER.to_owned() + "/" + POOL_FOLDER)
            .soft_cap_bytes(Some(1 << 30));
        let area = fixture.storage.declare(&config).expect("area");
        let root = area.create_root("quick").expect("root");
        assert!(root
            .path
            .starts_with(pool_dir.join(PAYLOAD_FOLDER).join(POOL_FOLDER)));
        // The root's marker is its own bytes; what matters is that Barge lists
        // and accounts for it.
        assert!(area.bytes().expect("bytes") > 0);
    }
}
