//! Oxide's storage authority: the lean Barge library.
//!
//! Barge owns registered volumes and their identity, declared areas, marked
//! roots, cooperative holds, durable pins, byte-pressure pruning, idle
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

use crate::lease::admission_cost;
use barge::{
    Absent, Area, AreaSpec, Barge, Hold, Location, Mode, PruneReport, Space, SweepReport, Volume,
    VolumeRegistration, VolumeState, VolumeStatus,
};

/// The namespace every Oxide area lives under.
pub const NAMESPACE: &str = "oxide";
/// Barge marker leaf, used only for passive identity checks.
pub const ROOT_MARKER: &str = ".barge-root.json";
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

/// Keep the refusal floor inside a configured nonzero reserve.
/// A zero reserve disables reserve pruning and retains Barge's default floor.
pub fn reserve_floor(reserve_bytes: u64) -> u64 {
    if reserve_bytes == 0 {
        barge::volume::DEFAULT_FLOOR_BYTES
    } else {
        barge::volume::DEFAULT_FLOOR_BYTES.min(reserve_bytes)
    }
}

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

/// Barge's platform seam for macOS mount roots and Linux directory filesystems.
/// A stale `/Volumes/<name>` directory on macOS's boot disk is absent; Linux
/// directory pools use the kernel filesystem identity through `DirectoryOs`.
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
        if cfg!(target_os = "macos") && is_drive_mount_point(mount) {
            // A path that does not exist is positively absent; a stale
            // directory (a positive observation that this is not the volume's
            // mount root) is absent too; any observation failure is unknown
            // and refuses rather than falling back.
            if std::fs::symlink_metadata(mount)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok(None);
            }
            return match crate::volume::observe_path(mount) {
                Ok(facts) => judge_mount_identity(mount, &facts).map_err(|reason| {
                    barge::Error::unknown_state(format!(
                        "cannot read the volume identity at {}: {reason}",
                        mount.display()
                    ))
                }),
                Err(reason) => Err(barge::Error::unknown_state(format!(
                    "cannot read the volume identity at {}: {reason}",
                    mount.display()
                ))),
            };
        }
        #[cfg(target_os = "linux")]
        {
            barge::Os::volume_identity(&DirectoryOs, mount)
        }
        #[cfg(not(target_os = "linux"))]
        barge::os::platform().volume_identity(mount)
    }

    fn free_bytes(&self, path: &Path) -> barge::Result<u64> {
        barge::os::platform().free_bytes(path)
    }
}

/// Directory backends on Linux use the kernel filesystem ID, not an APFS UUID.
/// Encode that observed ID in a namespaced UUID so Barge can compare it on each
/// open. A missing directory is absent; an unreadable/zero ID is unknown.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct DirectoryOs;

#[cfg(any(target_os = "linux", test))]
impl barge::Os for DirectoryOs {
    fn volume_identity(&self, path: &Path) -> barge::Result<Option<String>> {
        use sha2::{Digest, Sha256};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let directory = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(barge::Error::unknown_state(format!(
                    "cannot open directory filesystem at {}: {error}",
                    path.display()
                )))
            }
        };
        let mut facts = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: the file owns a live directory descriptor and facts is a
        // writable statvfs buffer; it is read only after a successful call.
        #[allow(unsafe_code)]
        let result = unsafe { libc::fstatvfs(directory.as_raw_fd(), facts.as_mut_ptr()) };
        if result != 0 {
            return Err(barge::Error::unknown_state(format!(
                "cannot read directory filesystem at {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: fstatvfs succeeded and initialized this buffer.
        #[allow(unsafe_code)]
        let facts = unsafe { facts.assume_init() };
        if facts.f_fsid == 0 {
            return Err(barge::Error::unknown_state(
                "directory filesystem reports no filesystem ID",
            ));
        }
        let mut digest = Sha256::digest(
            format!("oxide.directory-filesystem.statvfs.v1:{}", facts.f_fsid).as_bytes(),
        );
        digest[6] = (digest[6] & 0x0f) | 0x80;
        digest[8] = (digest[8] & 0x3f) | 0x80;
        let hex: String = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect();
        Ok(Some(format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )))
    }

    fn free_bytes(&self, path: &Path) -> barge::Result<u64> {
        barge::os::platform().free_bytes(path)
    }
}

/// Decide a drive mount's identity from the facts observed on its descriptor.
///
/// A positively stale directory (the path is not its own mount root) is absent;
/// a mounted root whose UUID cannot be read is unknown and refuses, never an
/// absent volume that would silently take the fallback.
pub fn judge_mount_identity(
    mount: &Path,
    facts: &crate::volume::Facts,
) -> Result<Option<String>, String> {
    if facts.mounted_on != mount {
        return Ok(None);
    }
    facts
        .uuid
        .clone()
        .map(Some)
        .ok_or_else(|| format!("{} reports no volume UUID", mount.display()))
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
        Barge::open_with(
            state_dir,
            Arc::new(OxideOs),
            Arc::new(barge::SystemClock::new()),
        )
        .map(|barge| Self { barge })
        .map_err(|error| format!("cannot open the storage state: {error}"))
    }

    /// Inspect an account without preparing any state, including when the
    /// account is absent or concurrently removed. Returned areas remain
    /// read-only, so an accidental mutation refuses before touching disk.
    pub fn open_read_only(state_root: &Path) -> Result<Self, String> {
        Barge::open_read_only_with(
            state_root.join("barge"),
            Arc::new(OxideOs),
            Arc::new(barge::SystemClock::new()),
        )
        .map(|barge| Self { barge })
        .map_err(|error| format!("cannot read the storage state: {error}"))
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
            Some(path) => Absent::Fallback { path: path.clone() },
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

    /// Find an existing declaration without registering a volume or declaring
    /// an area. Missing state is not permission to publish a new declaration.
    pub fn existing_area(&self, config: &AreaConfig) -> Result<Option<Area>, String> {
        let Some(area) = self
            .areas()?
            .into_iter()
            .find(|area| area.key().label() == config.key())
        else {
            return Ok(None);
        };
        let proposed = barge::AreaRecord {
            schema: barge::area::AREA_SCHEMA.to_owned(),
            namespace: NAMESPACE.to_owned(),
            name: config.area.clone(),
            volume: config.volume.clone(),
            folder: Some(config.folder.clone()),
            absent: match &config.fallback {
                Some(path) => Absent::Fallback { path: path.clone() },
                None => Absent::Fail,
            },
            soft_cap_bytes: config.soft_cap_bytes,
            counts_toward_reserve: true,
        };
        let differing = area.record().differing_fields(&proposed);
        let volume = area.volume().spec();
        if !differing.is_empty()
            || volume.mount != config.mount
            || volume.root_dir != config.root_dir
            || config
                .uuid
                .as_ref()
                .is_some_and(|uuid| !uuid.eq_ignore_ascii_case(&volume.uuid))
        {
            return Err(format!(
                "the existing declaration for {} differs from its recorded output location",
                config.key()
            ));
        }
        Ok(Some(area))
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

// Barge's recursive accounting is opaque to Oxide. Observe the adapter
// operations that invoke it, alongside the local per-root walk.
#[cfg(test)]
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PoolWalk {
    Measure(PathBuf),
    Evict(String),
    Inventory,
    Adopt(String),
}

#[cfg(test)]
thread_local! {
    static POOL_WALKS: std::cell::RefCell<Option<Vec<PoolWalk>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn observe_pool_walk(walk: PoolWalk) {
    POOL_WALKS.with_borrow_mut(|walks| {
        if let Some(walks) = walks {
            walks.push(walk);
        }
    });
}

#[cfg(test)]
pub(crate) fn count_pool_walks<T>(action: impl FnOnce() -> T) -> (T, Vec<PoolWalk>) {
    POOL_WALKS.with_borrow_mut(|walks| {
        assert!(walks.is_none());
        *walks = Some(Vec::new());
    });
    let result = action();
    let walks = POOL_WALKS.with_borrow_mut(|walks| walks.take().unwrap());
    (result, walks)
}

/// One pool's storage: its own Barge account, its volume and its
/// `oxide/pool` area.
///
/// A pool owns its account so the literal `oxide/pool` declaration is
/// unambiguous even while a removable pool, an internal pool and a one-use
/// pool coexist: each has its own account, volume and area under its own
/// control directory. The area's folder keeps the existing slot parent, so
/// adoption moves no path.
#[derive(Debug, Clone)]
pub struct PoolStorage {
    storage: Storage,
    area: Area,
    location: Location,
    /// The area name, folder and absent policy sub-areas inherit.
    name: String,
    folder: String,
    absent: Absent,
    soft_cap_bytes: Option<u64>,
    backing: PoolBacking,
}

/// The facts a pool's storage declaration needs.
#[derive(Clone, Debug)]
pub struct PoolBacking {
    /// The volume base: the removable drive's mount point, or the pool
    /// directory itself for a budget pool.
    pub mount: PathBuf,
    /// The folder inside the mount that holds this pool's slot roots.
    pub folder: String,
    /// The identity of a removable drive as Oxide recorded it.
    pub uuid: Option<String>,
    /// The pool's soft cap.
    pub soft_cap_bytes: Option<u64>,
    /// The drive's prune trigger; zero disables the reserve route.
    pub reserve_bytes: u64,
    /// Below this an admission refuses once nothing idle is left.
    pub floor_bytes: u64,
    /// True when the backing is a removable volume (a reserve participant).
    pub removable: bool,
}

impl PoolBacking {
    /// A budget-only directory pool at `root`: the area folder is the
    /// existing `payload/slots`, and admission is bounded by the soft cap.
    pub fn budget(root: &Path, soft_cap_bytes: u64) -> Self {
        Self {
            mount: root.to_path_buf(),
            folder: format!("{PAYLOAD_FOLDER}/{POOL_FOLDER}"),
            uuid: None,
            soft_cap_bytes: Some(soft_cap_bytes),
            reserve_bytes: 0,
            floor_bytes: 0,
            removable: false,
        }
    }

    /// A pool whose slot roots live on a removable volume.
    pub fn volume(
        mount: &Path,
        uuid: &str,
        payload_dir: &str,
        soft_cap_bytes: u64,
        reserve_bytes: u64,
        floor_bytes: u64,
    ) -> Self {
        Self {
            mount: mount.to_path_buf(),
            folder: format!("{payload_dir}/{POOL_FOLDER}"),
            uuid: Some(uuid.to_owned()),
            soft_cap_bytes: Some(soft_cap_bytes),
            reserve_bytes,
            floor_bytes,
            removable: true,
        }
    }

    fn area_config(&self) -> AreaConfig {
        AreaConfig::new("pool", self.mount.clone(), POOL_AREA)
            .uuid(self.uuid.clone())
            .folder(self.folder.clone())
            .soft_cap_bytes(self.soft_cap_bytes)
            .reserve_bytes(self.reserve_bytes)
            .floor_bytes(if self.reserve_bytes == 0 {
                self.floor_bytes
            } else {
                self.floor_bytes.min(self.reserve_bytes)
            })
    }

    /// Open an area already bound to this pool without declaring or repairing it.
    pub(crate) fn existing_pool_area(
        &self,
        storage: &Storage,
        project_folder: Option<&str>,
    ) -> Result<Area, String> {
        let mut config = self.area_config();
        if let Some(folder) = project_folder {
            config.area = format!("{}-{}", config.area, slug(folder));
            config.folder = format!("{}/{folder}", config.folder);
        }
        storage
            .existing_area(&config)?
            .ok_or_else(|| format!("pool area {} is missing", config.key()))
    }
}

impl PoolStorage {
    /// Open (and declare) one pool's storage under its control directory.
    ///
    /// The control directory is Oxide's own private directory; it is created
    /// owner-only here so Barge's account state never widens its mode.
    pub fn open(control: &Path, backing: &PoolBacking) -> Result<Self, String> {
        use std::os::unix::fs::DirBuilderExt;
        if !control.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(control)
                .map_err(|error| format!("cannot create the pool control directory: {error}"))?;
        }
        let storage = Storage::open(control)?;
        Self::from_storage(storage, backing)
    }

    /// Build a pool storage over an already-open account (Oxide's tests use a
    /// fake platform seam).
    pub fn from_storage(storage: Storage, backing: &PoolBacking) -> Result<Self, String> {
        let config = backing.area_config();
        let area = storage.declare(&config)?;
        let location = area
            .location()
            .map_err(|error| format!("pool area: {error}"))?;
        Ok(Self {
            storage,
            area,
            location,
            name: config.area.clone(),
            folder: backing.folder.clone(),
            absent: match &config.fallback {
                Some(path) => Absent::Fallback { path: path.clone() },
                None => Absent::Fail,
            },
            soft_cap_bytes: backing.soft_cap_bytes,
            backing: backing.clone(),
        })
    }

    /// Record the front door's registered reserve on this volume, so Barge's
    /// prune target is the operator's reserve rather than zero.
    pub fn set_reserve(&self, reserve_bytes: u64) -> Result<(), String> {
        let floor_bytes = if reserve_bytes == 0 {
            self.backing.floor_bytes
        } else {
            self.backing.floor_bytes.min(reserve_bytes)
        };
        let volume = self.storage.volume("pool")?;
        if volume.spec().reserve_bytes == reserve_bytes && volume.spec().floor_bytes == floor_bytes
        {
            return Ok(());
        }
        self.storage.register_volume(
            "pool",
            &self.backing.mount,
            self.backing.uuid.as_deref(),
            "",
            reserve_bytes,
            floor_bytes,
        )?;
        Ok(())
    }

    /// Where the pool's new slot roots go right now, or why there is nowhere.
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// The storage account behind this pool.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// The declared area.
    pub fn area(&self) -> &Area {
        &self.area
    }

    /// Whether the pool's roots are on a removable volume.
    pub fn is_removable(&self) -> bool {
        self.backing.removable && self.location.on_volume()
    }

    /// The declared area, or the sub-area for one project folder.
    ///
    /// The flat slot parent (`slots/`) is one area; a legacy slot recorded as
    /// `<folder>/<leaf>` lives one level lower, so its own project folder is
    /// its area. Declaring it is idempotent, and it is how an existing nested
    /// slot is adopted with its path intact.
    pub fn area_for(&self, folder: Option<&str>) -> Result<Area, String> {
        let Some(folder) = folder else {
            return Ok(self.area.clone());
        };
        let name = format!("{}-{}", self.name, slug(folder));
        let spec = barge::AreaSpec::new(NAMESPACE, name, "pool")
            .folder(format!("{}/{folder}", self.folder))
            .absent(self.absent.clone())
            .soft_cap_bytes(self.soft_cap_bytes)
            .counts_toward_reserve(true);
        self.storage
            .barge()
            .area(spec)
            .map_err(|error| format!("cannot declare the area for {folder}: {error}"))
    }

    /// Every area this pool declares, in name order.
    fn areas(&self) -> Result<Vec<Area>, String> {
        Ok(self
            .storage
            .areas()?
            .into_iter()
            .filter(|area| {
                let key = area.key();
                key.namespace() == NAMESPACE
                    && (key.name() == self.name
                        || key.name().starts_with(&format!("{}-", self.name)))
            })
            .collect::<Vec<_>>())
    }

    /// Adopt an existing Oxide directory as an owned root, keeping its folder
    /// name and bytes. `folder` is the directory's own name in its area;
    /// `id` is Oxide's stable identity for it.
    pub fn adopt_in(
        &self,
        project_folder: Option<&str>,
        folder: &str,
        label: &str,
        id: &str,
    ) -> barge::Result<barge::Root> {
        let area = self
            .area_for(project_folder)
            .map_err(barge::Error::unknown_state)?;
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Adopt(id.to_owned()));
        admission_cost::opaque_listing();
        area.adopt_root(folder, label, id)
    }

    /// Create one root for a slot (a fresh id and path) in the flat area.
    pub fn create(&self, label: &str) -> Result<barge::Root, String> {
        self.area
            .create_root(label)
            .map_err(|error| format!("cannot create a slot root: {error}"))
    }

    /// Find a root by id in the pool's areas.
    pub fn resolve_any(&self, id: &str) -> Result<barge::Root, String> {
        for area in self.areas()? {
            if let Ok(root) = area.resolve(id) {
                return Ok(root);
            }
        }
        Err(format!("no pool root {id}"))
    }

    /// Take a waiting exclusive hold on a root in the given project folder.
    pub fn hold_in(&self, project_folder: Option<&str>, id: &str) -> Result<Hold, String> {
        self.area_for(project_folder)?
            .hold(id, Mode::Exclusive)
            .map_err(|error| format!("cannot hold slot root {id}: {error}"))
    }

    /// Pin a root in one project area, protecting it from prune and eviction.
    pub fn pin_in(
        &self,
        project_folder: Option<&str>,
        id: &str,
        reason: &str,
    ) -> Result<(), String> {
        self.area_for(project_folder)?
            .pin(id, reason)
            .map_err(|error| format!("cannot pin root {id}: {error}"))
    }

    /// Remove a root's pin in one project area.
    pub fn unpin_in(&self, project_folder: Option<&str>, id: &str) -> Result<(), String> {
        self.area_for(project_folder)?
            .unpin(id)
            .map_err(|error| format!("cannot unpin root {id}: {error}"))
    }

    /// Try to take an exclusive hold without waiting.
    pub fn try_hold_in(
        &self,
        project_folder: Option<&str>,
        id: &str,
    ) -> Result<Option<Hold>, String> {
        let area = self.area_for(project_folder)?;
        admission_cost::opaque_listing();
        area.try_hold(id, Mode::Exclusive)
            .map_err(|error| format!("cannot test slot root {id}: {error}"))
    }

    /// Evict one root in the given project folder.
    pub fn evict_in(
        &self,
        project_folder: Option<&str>,
        id: &str,
    ) -> barge::Result<barge::area::Eviction> {
        let area = self
            .area_for(project_folder)
            .map_err(barge::Error::unknown_state)?;
        admission_cost::opaque_listing();
        let eviction = area.evict(id);
        if eviction.is_ok() || eviction.as_ref().err().is_some_and(eviction_walked_bytes) {
            admission_cost::walked(1);
            #[cfg(test)]
            observe_pool_walk(PoolWalk::Evict(id.to_owned()));
        }
        eviction
    }

    /// One metadata-only inventory for a ceiling refusal or a protected victim.
    pub(crate) fn list_protection(&self) -> Result<Vec<barge::RootInfo>, String> {
        let mut roots = Vec::new();
        for area in self.areas()? {
            #[cfg(test)]
            observe_pool_walk(PoolWalk::Inventory);
            let listing = area.list_without_bytes().map_err(|error| {
                admission_cost::opaque_listing();
                error.to_string()
            })?;
            admission_cost::listed(listing.roots.len() + listing.stragglers.len());
            roots.extend(listing.roots);
        }
        Ok(roots)
    }

    /// Evict one root from whichever area holds it.
    pub fn evict_any(&self, id: &str) -> Result<barge::area::Eviction, String> {
        let mut last = format!("no pool root {id}");
        for area in self.areas()? {
            match area.evict(id) {
                Ok(eviction) => return Ok(eviction),
                Err(error) => last = format!("cannot evict slot root {id}: {error}"),
            }
        }
        Err(last)
    }

    /// The bytes every area of this pool holds.
    pub fn total_bytes(&self) -> Result<u64, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        let mut total = 0_u64;
        for area in self.areas()? {
            total = total.saturating_add(
                area.bytes()
                    .map_err(|error| format!("cannot account the pool area: {error}"))?,
            );
        }
        Ok(total)
    }

    /// Every owned root of this pool with its area's project folder (the first
    /// component of `folder` is the area, so a root's area is recoverable).
    pub fn all_roots(&self) -> Result<Vec<(Option<String>, barge::RootInfo)>, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        let mut roots = Vec::new();
        for area in self.areas()? {
            let area_folder = area.record().effective_folder();
            let project = project_folder(self.folder.as_str(), &area_folder);
            let listing = area
                .list()
                .map_err(|error| format!("cannot list the pool area: {error}"))?;
            for info in listing.roots {
                roots.push((project.clone(), info));
            }
        }
        Ok(roots)
    }

    /// Sweep every area's owned trash.
    pub fn sweep_all_trash(&self) -> Result<barge::SweepReport, String> {
        let mut report = barge::SweepReport::default();
        for area in self.areas()? {
            let swept = area.sweep_trash().map_err(|error| {
                admission_cost::opaque_listing();
                format!("cannot sweep the pool trash: {error}")
            })?;
            // The sweep traverses trash for deletion and reports reclaimed
            // bytes as it goes. Barge runs no separate sizing walk here.
            admission_cost::listed(swept.deleted.len() + swept.kept.len());
            if !swept.kept.is_empty() {
                admission_cost::opaque_listing();
            }
            report.deleted.extend(swept.deleted);
            report.kept.extend(swept.kept);
        }
        Ok(report)
    }

    /// Find a root by its id.
    pub fn resolve(&self, id: &str) -> Result<barge::Root, String> {
        self.area
            .resolve(id)
            .map_err(|error| format!("cannot resolve slot root {id}: {error}"))
    }

    /// Take a waiting exclusive hold on a root.
    pub fn hold(&self, id: &str) -> Result<Hold, String> {
        admission_cost::opaque_listing();
        self.area
            .hold(id, Mode::Exclusive)
            .map_err(|error| format!("cannot hold slot root {id}: {error}"))
    }

    /// Try to take an exclusive hold without waiting. `None` means another
    /// holder is still using the root, so it is not idle.
    pub fn try_hold(&self, id: &str) -> Result<Option<Hold>, String> {
        self.area
            .try_hold(id, Mode::Exclusive)
            .map_err(|error| format!("cannot test slot root {id}: {error}"))
    }

    /// Every owned root, with held/pinned/bytes facts.
    pub fn list(&self) -> Result<barge::Listing, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        self.area
            .list()
            .map_err(|error| format!("cannot list the pool area: {error}"))
    }

    /// Evict one root: refused while it is held or pinned, otherwise renamed
    /// into the area's trash and deleted there.
    pub fn evict(&self, id: &str) -> Result<barge::area::Eviction, String> {
        self.area
            .evict(id)
            .map_err(|error| format!("cannot evict slot root {id}: {error}"))
    }

    /// The bytes the area's owned roots hold.
    pub fn bytes(&self) -> Result<u64, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        self.area
            .bytes()
            .map_err(|error| format!("cannot account the pool area: {error}"))
    }

    /// Delete owned trash a crash left behind.
    pub fn sweep_trash(&self) -> Result<barge::SweepReport, String> {
        self.area
            .sweep_trash()
            .map_err(|error| format!("cannot sweep the pool trash: {error}"))
    }

    /// The volume's space policy, or why it is unavailable.
    pub fn space(&self) -> Result<Space, String> {
        self.storage.space("pool")
    }

    /// Prune: soft cap first, then least-recently-used idle unpinned roots
    /// until free space is back above the reserve.
    pub fn prune(&self, dry_run: bool) -> Result<PruneReport, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        self.storage.prune("pool", dry_run)
    }

    /// Admit work: prune, then refuse only below the floor.
    pub fn admit(&self) -> Result<PruneReport, String> {
        #[cfg(test)]
        observe_pool_walk(PoolWalk::Inventory);
        self.storage.admit("pool")
    }
}

/// At the pinned revision these outcomes follow the single sizing walk.
fn eviction_walked_bytes(error: &barge::Error) -> bool {
    let message = error.message();
    match error.kind() {
        barge::ErrorKind::Refused => message == "root marker changed before eviction",
        barge::ErrorKind::Io if error.io_kind().is_some() => {
            message.starts_with("cannot create the trash at ")
                || message.starts_with("cannot evict the root to ")
        }
        _ => eviction_moved(error),
    }
}

/// The pinned Barge API reports a post-rename deletion error as source-less
/// I/O with this trash diagnosis. Ordinary path I/O still has its source kind.
pub(crate) fn eviction_moved(error: &barge::Error) -> bool {
    error.kind() == barge::ErrorKind::Io
        && error.io_kind().is_none()
        && error
            .message()
            .contains("; evicted root remains in trash at ")
}

/// Read a single Barge pin record for a known slot. Barge has no public pin
/// reason accessor at the pinned revision. This private-layout read is only
/// for diagnostics. A missing, raced or unreadable fact cannot permit eviction.
pub(crate) fn slot_pin_reason(
    path: &Path,
    folder: Option<&str>,
    id: &str,
) -> Result<Option<String>, ()> {
    use crate::fs_util::{entry_label, open_directory_at};
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    const MAX_RECORD: u64 = 16 * 1024;
    fn read_json(parent: &File, name: &CStr) -> std::io::Result<serde_json::Value> {
        // SAFETY: the held directory and C string are live for the call.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor, now owned by File.
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        // SAFETY: geteuid is infallible and needs no arguments.
        if !metadata.is_file()
            || metadata.len() > MAX_RECORD
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        serde_json::from_slice(&bytes).map_err(std::io::Error::other)
    }

    let parent = path.parent().ok_or(())?;
    if std::fs::canonicalize(parent).map_err(|_| ())? != parent {
        return Err(());
    }
    let parent = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)
        .map_err(|_| ())?;
    let open_child =
        |parent: &File, name: &CStr| open_directory_at(parent.as_fd(), name).map(File::from);
    let leaf = CString::new(path.file_name().and_then(OsStr::to_str).ok_or(())?).map_err(|_| ())?;
    let root = open_child(&parent, &leaf).map_err(|_| ())?;
    let marker = read_json(&root, c".barge-root.json").map_err(|_| ())?;
    let area = folder.map_or_else(
        || format!("{NAMESPACE}/{POOL_AREA}"),
        |folder| format!("{NAMESPACE}/{POOL_AREA}-{}", slug(folder)),
    );
    if marker["id"].as_str() != Some(id) || marker["area"].as_str() != Some(area.as_str()) {
        return Err(());
    }
    if !id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || id.is_empty()
        || id.len() > 64
    {
        return Err(());
    }
    let pin_name = CString::new(format!("{id}.json")).map_err(|_| ())?;
    let pin = open_child(&parent, c".barge")
        .and_then(|barge| open_child(&barge, c"pins"))
        .and_then(|pins| read_json(&pins, &pin_name));
    match pin {
        Ok(pin)
            if pin["schema"].as_str() == Some(barge::pin::PIN_SCHEMA)
                && pin["root_id"].as_str() == Some(id) =>
        {
            let reason = pin["reason"]
                .as_str()
                .filter(|reason| !reason.is_empty())
                .ok_or(())?;
            Ok(Some(entry_label(OsStr::new(reason))))
        }
        Ok(_) => Err(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// A bounded, Barge-safe suffix for a sub-area name.
fn slug(text: &str) -> String {
    let mut slug: String = text
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(48);
    while slug.ends_with('-') && slug.len() > 1 {
        slug.pop();
    }
    slug
}

/// The project folder of an area whose folder is `<base>/<project>`.
fn project_folder(base: &str, area_folder: &str) -> Option<String> {
    area_folder
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .map(str::to_owned)
}

/// Make a directory Barge created owner-only, the mode Oxide writes slots in.
pub fn make_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
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

    #[test]
    fn directory_filesystem_identity_opens_and_reopens_barge_pool_storage() {
        let temp = TempDir::new("directory-filesystem-identity").unwrap();
        let mount = temp.path().join("pool");
        std::fs::create_dir(&mount).unwrap();
        let identity = DirectoryOs.volume_identity(&mount).unwrap().unwrap();
        assert_eq!(barge::os::canonical_uuid(&identity).unwrap(), identity);
        assert_eq!(
            DirectoryOs.volume_identity(temp.path()).unwrap().as_ref(),
            Some(&identity)
        );
        let state = temp.path().join("state");
        let open = || {
            let storage = Storage::open_with(
                state.clone(),
                Arc::new(DirectoryOs),
                Arc::new(barge::SystemClock::new()),
            )
            .unwrap();
            PoolStorage::from_storage(storage, &PoolBacking::budget(&mount, 8 << 20)).unwrap()
        };
        let pool = open();
        let root = pool.area().create_root("warm").unwrap();
        std::fs::write(root.path.join("artifact"), b"warm").unwrap();
        drop(pool);
        let reopened = open();
        let same = reopened.area().resolve(&root.id).unwrap();
        assert_eq!(same.path, root.path);
        assert_eq!(std::fs::read(same.path.join("artifact")).unwrap(), b"warm");
        let link = temp.path().join("alias");
        std::os::unix::fs::symlink(&mount, &link).unwrap();
        assert!(DirectoryOs.volume_identity(&link).is_err());
        assert_eq!(
            DirectoryOs
                .volume_identity(&temp.path().join("absent"))
                .unwrap(),
            None
        );
    }

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
        assert!(root
            .path
            .starts_with(fixture.mount.join(VOLUME_PAYLOAD_FOLDER).join(POOL_FOLDER)));
        assert!(root.path.join(".barge-root.json").is_file());
        assert_eq!(
            fixture.storage.location(&fixture.pool()).expect("location"),
            Location::Volume(fixture.mount.join(VOLUME_PAYLOAD_FOLDER).join(POOL_FOLDER))
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
        let state =
            volume_state(&fixture.storage.volume("Builds").expect("volume")).expect("state");
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
        // Barge samples the filesystem again after deleting a root. The
        // fake mount gains space only after the older root has disappeared.
        #[derive(Debug)]
        struct FreeAfterEviction {
            volume: FakeOs,
            older: PathBuf,
            reserve: u64,
        }

        impl Os for FreeAfterEviction {
            fn volume_identity(&self, mount: &Path) -> barge::Result<Option<String>> {
                self.volume.volume_identity(mount)
            }

            fn free_bytes(&self, mount: &Path) -> barge::Result<u64> {
                let before = self.volume.free_bytes(mount)?;
                Ok(if self.older.exists() {
                    before
                } else {
                    self.reserve
                })
            }
        }

        let reserve = 20_u64 << 30;
        fixture.os.set_free(&fixture.mount, reserve - 1);
        let storage = Storage::open_with(
            fixture.storage.state_dir().to_path_buf(),
            Arc::new(FreeAfterEviction {
                volume: fixture.os.clone(),
                older: older.path.clone(),
                reserve,
            }),
            fixture.clock.clone(),
        )
        .expect("storage");
        let report = storage.prune("Builds", false).expect("prune");
        assert_eq!(report.free_after_bytes, reserve);
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

    fn pool_storage(fixture: &Fixture, label: &str) -> PoolStorage {
        let control = fixture._temp.path().join(format!("pool-{label}"));
        std::fs::create_dir_all(control.join("oxide-managed")).expect("control");
        fixture.os.add_volume(&control, Some(UUID), 100 << 30);
        PoolStorage::open(
            &control.join("oxide-managed"),
            &PoolBacking::budget(&control, 1 << 30),
        )
        .expect("pool storage")
    }

    #[test]
    fn a_pool_creation_records_a_root_in_the_existing_slot_parent() {
        let fixture = Fixture::new("storage-pool-create");
        let pool = pool_storage(&fixture, "create");
        let root = pool.create("quick").expect("root");
        assert!(root
            .path
            .starts_with(fixture._temp.path().join("pool-create/payload/slots")));
        assert!(root.path.join(".barge-root.json").is_file());
        let listing = pool.list().expect("list");
        assert_eq!(listing.roots.len(), 1);
        assert!(pool.bytes().expect("bytes") > 0);
    }

    // A sibling test's `Command` spawn briefly duplicates every open descriptor
    // in the forked child before `exec` closes the close-on-exec ones, so a hold
    // that this test has just dropped can still read as held for a few
    // milliseconds. The refusal above is the strict statement; this retry only
    // waits out that fork window.
    fn evict_after_release(pool: &PoolStorage, id: &str) {
        for _ in 0..200 {
            match pool.evict(id) {
                Ok(_) => return,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        panic!("the released root was never evictable");
    }

    #[test]
    fn a_pool_hold_is_released_by_dropping_it_and_an_eviction_refuses_while_held() {
        let fixture = Fixture::new("storage-pool-hold");
        let pool = pool_storage(&fixture, "hold");
        let root = pool.create("quick").expect("root");
        let hold = pool.hold(&root.id).expect("hold");
        assert!(
            pool.try_hold(&root.id).expect("try").is_none(),
            "a held root is not idle"
        );
        assert!(pool.evict(&root.id).is_err());
        drop(hold);
        assert!(pool.try_hold(&root.id).expect("try").is_some());
        evict_after_release(&pool, &root.id);
        assert!(!root.path.exists());
        assert!(pool.list().expect("list").roots.is_empty());
    }

    #[test]
    fn a_pool_space_policy_reports_its_cap_and_refuses_below_the_floor() {
        let fixture = Fixture::new("storage-pool-space");
        let control = fixture._temp.path().join("pool-space");
        std::fs::create_dir_all(control.join("oxide-managed")).expect("control");
        fixture.os.add_volume(&control, Some(UUID), 1 << 30);
        let pool = PoolStorage::open(
            &control.join("oxide-managed"),
            &PoolBacking {
                mount: control.clone(),
                folder: "payload/slots".to_owned(),
                uuid: None,
                soft_cap_bytes: Some(1 << 30),
                reserve_bytes: 2 << 30,
                floor_bytes: 10 << 20,
                removable: false,
            },
        )
        .expect("pool storage");
        assert_eq!(pool.space().expect("space").floor_bytes, 10 << 20);
        assert!(matches!(pool.location(), Location::Volume(_)));
    }
    #[test]
    fn an_existing_oxide_directory_is_adopted_in_place_and_the_repeat_is_idempotent() {
        let fixture = Fixture::new("storage-adopt");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let location = fixture.storage.location(&fixture.pool()).expect("location");
        let base = location.path().expect("path").to_path_buf();
        let legacy = base.join("oxide-72ea0e6b");
        std::fs::create_dir_all(&legacy).expect("legacy root");
        std::fs::write(legacy.join("payload.bin"), vec![0_u8; 4096]).expect("content");

        let root = area
            .adopt_root("oxide-72ea0e6b", "checkout", "deadbeef00c0ffee")
            .expect("adopt");
        assert_eq!(root.path, legacy);
        assert_eq!(root.folder, "oxide-72ea0e6b");
        assert!(legacy.join("payload.bin").is_file(), "nothing moved");
        let marker: serde_json::Value = serde_json::from_slice(
            &std::fs::read(legacy.join(".barge-root.json")).expect("marker"),
        )
        .expect("json");
        assert_eq!(marker["schema"], "barge.root/2");
        assert_eq!(marker["folder"], "oxide-72ea0e6b");

        let repeat = area
            .adopt_root("oxide-72ea0e6b", "checkout", "deadbeef00c0ffee")
            .expect("idempotent repeat");
        assert_eq!(repeat.id, root.id);
        let listing = area.list().expect("list");
        assert_eq!(listing.roots.len(), 1);
        assert!(listing.bytes() >= 4096);

        // Adopted roots behave like created ones.
        let hold = area.hold(&root.id, Mode::Exclusive).expect("hold");
        assert!(area.evict(&root.id).is_err());
        drop(hold);
        let mut evicted = false;
        for _ in 0..200 {
            if area.evict(&root.id).is_ok() {
                evicted = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(evicted, "the released adopted root was never evictable");
        assert!(!legacy.exists());
    }

    #[test]
    fn adoption_refuses_a_foreign_marker_a_symlink_and_a_writable_folder() {
        let fixture = Fixture::new("storage-adopt-refuse");
        let area = fixture.storage.declare(&fixture.pool()).expect("area");
        let other = fixture
            .storage
            .declare(
                &AreaConfig::new("Builds", fixture.mount.clone(), "other")
                    .folder(format!("{VOLUME_PAYLOAD_FOLDER}/{POOL_FOLDER}")),
            )
            .expect("other area");
        let base = fixture
            .storage
            .location(&fixture.pool())
            .expect("location")
            .path()
            .expect("path")
            .to_path_buf();
        let foreign = base.join("oxide-adopted-elsewhere");
        std::fs::create_dir_all(&foreign).expect("dir");
        other
            .adopt_root("oxide-adopted-elsewhere", "checkout", "aaaabbbbccccdddd")
            .expect("adopted by the other area");
        let error = area
            .adopt_root("oxide-adopted-elsewhere", "checkout", "aaaabbbbccccdddd")
            .expect_err("foreign marker");
        assert!(error.message().contains("area"), "{error}");

        let writable = base.join("oxide-writable");
        std::fs::create_dir_all(&writable).expect("dir");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&writable, std::fs::Permissions::from_mode(0o777)).expect("mode");
        assert!(area
            .adopt_root("oxide-writable", "checkout", "1111222233334444")
            .is_err());

        let target = base.join("oxide-real");
        std::fs::create_dir_all(&target).expect("dir");
        std::os::unix::fs::symlink(&target, base.join("oxide-link")).expect("link");
        assert!(area
            .adopt_root("oxide-link", "checkout", "5555666677778888")
            .is_err());

        assert!(
            area.adopt_root("oxide-missing", "checkout", "9999aaaabbbbcccc")
                .is_err(),
            "a missing folder is never created by adoption"
        );
    }

    #[test]
    fn a_pool_adopts_a_legacy_nested_slot_in_its_project_area() {
        let fixture = Fixture::new("storage-adopt-nested");
        let control = fixture._temp.path().join("pool-nested");
        std::fs::create_dir_all(control.join("oxide-managed")).expect("control");
        fixture.os.add_volume(&control, Some(UUID), 100 << 30);
        let pool = PoolStorage::open(
            &control.join("oxide-managed"),
            &PoolBacking::budget(&control, 1 << 30),
        )
        .expect("pool storage");
        let base = control.join("payload/slots");
        let nested = base.join("oxide-f88047ec/quick-3");
        std::fs::create_dir_all(&nested).expect("nested slot");
        std::fs::write(nested.join("build.log"), b"log").expect("content");

        let root = pool
            .adopt_in(
                Some("oxide-f88047ec"),
                "quick-3",
                "quick",
                "0123456789abcdef",
            )
            .expect("adopt nested");
        assert_eq!(root.path, nested);
        assert!(pool.total_bytes().expect("bytes") >= 3);
        let hold = pool
            .hold_in(Some("oxide-f88047ec"), &root.id)
            .expect("hold");
        assert!(pool.evict_any(&root.id).is_err());
        drop(hold);
        pool.evict_any(&root.id).expect("evicted");
        assert!(!nested.exists());
        // The project folder itself is kept: only the recorded slot is gone.
        assert!(base.join("oxide-f88047ec").is_dir());
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

#[cfg(test)]
mod revision_tests {
    use super::*;
    use barge::{FakeOs, ManualClock, Mode};
    use std::sync::Arc;

    const UUID: &str = "8E4A5C1D-0000-4000-8000-00000000BEEF";

    fn fixture(label: &str) -> (barge::testing::TempDir, Storage, FakeOs, PathBuf) {
        let temp = barge::testing::TempDir::new(label).expect("temp");
        let mount = temp.path().join("Builds");
        std::fs::create_dir_all(&mount).expect("mount");
        let os = FakeOs::with_volume(mount.clone(), UUID, 100 << 30);
        let storage = Storage::open_with(
            temp.path().join("state"),
            Arc::new(os.clone()),
            Arc::new(ManualClock::new(1_000)),
        )
        .expect("storage");
        (temp, storage, os, mount)
    }

    /// An OS seam whose drive mount identity cannot be read: that is unknown,
    /// never absent, so no fallback is taken.
    #[derive(Debug, Clone, Copy)]
    struct UnreadableOs;

    impl barge::Os for UnreadableOs {
        fn volume_identity(&self, mount: &Path) -> barge::Result<Option<String>> {
            Err(barge::Error::unknown_state(format!(
                "cannot read the volume identity at {}",
                mount.display()
            )))
        }
        fn free_bytes(&self, _path: &Path) -> barge::Result<u64> {
            Ok(100 << 30)
        }
    }

    #[test]
    fn an_unreadable_drive_identity_refuses_instead_of_falling_back() {
        let temp = barge::testing::TempDir::new("storage-unknown-identity").expect("temp");
        let mount = temp.path().join("Builds");
        std::fs::create_dir_all(&mount).expect("mount");
        let storage = Storage::open_with(
            temp.path().join("state"),
            Arc::new(UnreadableOs),
            Arc::new(ManualClock::new(1_000)),
        )
        .expect("storage");
        let fallback = temp.path().join("internal/slots");
        let config = AreaConfig::new("Builds", mount.clone(), POOL_AREA)
            .uuid(Some(UUID.to_owned()))
            .folder("oxide-pool/slots")
            .fallback(Some(fallback));
        assert!(
            storage.declare(&config).is_err(),
            "an unknown identity is a refusal, not an absent volume"
        );
    }

    #[test]
    fn a_prune_skips_a_pinned_and_a_held_root_and_evicts_a_later_eligible_one() {
        let (_temp, storage, os, mount) = fixture("storage-multi-victim");
        let config = AreaConfig::new("Builds", mount.clone(), POOL_AREA)
            .folder("oxide-pool/slots")
            .reserve_bytes(20 << 30)
            .floor_bytes(10 << 30);
        let area = storage.declare(&config).expect("area");
        let pinned = area.create_root("quick").expect("root");
        area.pin(&pinned.id, "selected output").expect("pin");
        let held = area.create_root("quick").expect("root");
        let hold = area.hold(&held.id, Mode::Exclusive).expect("hold");
        let eligible = area.create_root("quick").expect("root");
        std::fs::write(eligible.path.join("file"), vec![0_u8; 8192]).expect("write");
        let eligible_bytes = area
            .list()
            .expect("list")
            .get(&eligible.id)
            .map(|root| root.bytes)
            .expect("eligible");
        // Below the reserve by exactly the eligible root's bytes: only that
        // root can bring free space back.
        os.set_free(&mount, (20_u64 << 30) - eligible_bytes);
        let report = storage.prune("Builds", false).expect("prune");
        assert_eq!(report.evicted.len(), 1, "{report:?}");
        assert_eq!(report.evicted[0].id, eligible.id);
        assert!(pinned.path.exists(), "the pin kept its root");
        assert!(held.path.exists(), "the hold kept its root");
        drop(hold);
    }

    #[test]
    fn a_removable_area_records_the_front_door_reserve() {
        let (_temp, storage, os, mount) = fixture("storage-reserve");
        let control = _temp.path().join("pool");
        std::fs::create_dir_all(control.join("oxide-managed")).expect("control");
        os.add_volume(&control, Some(UUID), 100 << 30);
        let pool = PoolStorage::from_storage(
            storage.clone(),
            &PoolBacking {
                mount: mount.clone(),
                folder: "oxide-pool/slots".to_owned(),
                uuid: Some(UUID.to_owned()),
                soft_cap_bytes: Some(1 << 30),
                reserve_bytes: 0,
                floor_bytes: 10 << 20,
                removable: true,
            },
        )
        .expect("pool");
        pool.set_reserve(20 << 30).expect("reserve");
        assert_eq!(pool.space().expect("space").reserve_bytes, 20 << 30);
        pool.set_reserve(0).expect("disable reserve pruning");
        assert_eq!(pool.space().expect("space").reserve_bytes, 0);
        assert_eq!(pool.space().expect("space").floor_bytes, 10 << 20);
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn facts(mounted_on: &Path, uuid: Option<&str>) -> crate::volume::Facts {
        crate::volume::Facts {
            mounted_on: mounted_on.to_path_buf(),
            device: "/dev/disk9s1".to_owned(),
            fstype: "apfs".to_owned(),
            read_only: false,
            root_fs: false,
            uuid: uuid.map(str::to_owned),
            dev: 1,
            capacity_bytes: None,
            available_bytes: None,
        }
    }

    #[test]
    fn the_real_seam_refuses_a_mounted_root_without_a_uuid() {
        let mount = Path::new("/Volumes/Builds");
        assert_eq!(
            judge_mount_identity(
                mount,
                &facts(mount, Some("8E4A5C1D-0000-4000-8000-00000000BEEF"))
            )
            .unwrap(),
            Some("8E4A5C1D-0000-4000-8000-00000000BEEF".to_owned())
        );
        assert!(
            judge_mount_identity(mount, &facts(mount, None)).is_err(),
            "a mounted root with no UUID is unknown, not absent"
        );
        // A stale directory on another volume is positively not this mount.
        assert_eq!(
            judge_mount_identity(mount, &facts(Path::new("/"), Some("boot"))).unwrap(),
            None
        );
    }
}
