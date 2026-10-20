//! Machine configuration: where the internal pool lives and its soft budget.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (User
//! configuration). Precedence is environment (`OXIDE_POOL`,
//! `OXIDE_POOL_CAP`), then the user config file, then built-in defaults.
//! The file sits at the platform-native location: on macOS
//! `~/Library/Application Support/oxide/config/config.toml`, elsewhere
//! `$XDG_CONFIG_HOME/oxide/config.toml` (XDG's `~/.config` when unset).
//! `OXIDE_CONFIG` selects another file. On macOS `~/.config/oxide/config.toml`
//! is never read; `status` says it is ignored.

use serde::Deserialize;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// 100 GB, decimal: the built-in soft budget of the internal pool.
pub const DEFAULT_BUDGET_BYTES: u64 = 100_000_000_000;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
/// Leaf of the built-in pool under the platform cache. Distinct from the
/// retired image pool's `pool` leaf, so that one is retired separately.
const DEFAULT_POOL_LEAF: &str = "build-pool";
/// The retired `apfs-sparse-image-v1` default pool's leaf.
const LEGACY_POOL_LEAF: &str = "pool";
const MAX_KNOWN_POOLS: usize = 16;
const MAX_KNOWN_POOLS_BYTES: u64 = 64 * 1024;

/// Platform and environment facts, read once so tests can inject them.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub macos: bool,
    pub home: Option<OsString>,
    pub oxide_config: Option<OsString>,
    pub xdg_config_home: Option<OsString>,
    pub xdg_cache_home: Option<OsString>,
    pub xdg_state_home: Option<OsString>,
    pub oxide_pool: Option<OsString>,
    pub oxide_pool_cap: Option<OsString>,
}

impl Facts {
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            macos: cfg!(target_os = "macos"),
            home: var("HOME"),
            oxide_config: var("OXIDE_CONFIG"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_cache_home: var("XDG_CACHE_HOME"),
            xdg_state_home: var("XDG_STATE_HOME"),
            oxide_pool: var("OXIDE_POOL"),
            oxide_pool_cap: std::env::var_os("OXIDE_POOL_CAP"),
        }
    }

    fn home(&self) -> Result<PathBuf, String> {
        let home = self
            .home
            .as_ref()
            .map(PathBuf::from)
            .ok_or("HOME is unset; set OXIDE_CONFIG and OXIDE_POOL explicitly")?;
        if home.is_absolute() {
            Ok(home)
        } else {
            Err("HOME is not an absolute path".to_owned())
        }
    }

    /// An XDG base directory: the variable when set (it must be absolute),
    /// else XDG's documented default under HOME.
    fn xdg(&self, value: Option<&OsString>, name: &str, default: &str) -> Result<PathBuf, String> {
        match value {
            Some(value) if Path::new(value).is_absolute() => Ok(PathBuf::from(value)),
            Some(_) => Err(format!("{name} must be an absolute path")),
            None => Ok(self.home()?.join(default)),
        }
    }

    fn cache_dir(&self) -> Result<PathBuf, String> {
        if self.macos {
            Ok(self.home()?.join("Library/Caches/oxide"))
        } else {
            Ok(self
                .xdg(self.xdg_cache_home.as_ref(), "XDG_CACHE_HOME", ".cache")?
                .join("oxide"))
        }
    }

    fn state_dir(&self) -> Result<PathBuf, String> {
        if self.macos {
            Ok(self.home()?.join("Library/Application Support/oxide/state"))
        } else {
            Ok(self
                .xdg(
                    self.xdg_state_home.as_ref(),
                    "XDG_STATE_HOME",
                    ".local/state",
                )?
                .join("oxide"))
        }
    }
}

/// Where a setting came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    Environment(&'static str),
    File,
    Default,
}

impl Source {
    pub fn describe(self) -> String {
        match self {
            Self::Environment(name) => name.to_owned(),
            Self::File => "config file".to_owned(),
            Self::Default => "built-in default".to_owned(),
        }
    }
}

/// The selected config file and whether it exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigFile {
    pub path: PathBuf,
    /// Selected by `OXIDE_CONFIG` rather than the platform location.
    pub explicit: bool,
    pub present: bool,
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    pool: Option<PoolSection>,
}

#[derive(Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct PoolSection {
    path: Option<String>,
    budget: Option<Budget>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(untagged)]
enum Budget {
    Bytes(u64),
    Text(String),
}

/// The effective internal pool settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Settings {
    pub pool: PathBuf,
    pub pool_source: Source,
    pub budget: u64,
    pub budget_source: Source,
    pub file: ConfigFile,
}

/// The config file the platform selects, or `OXIDE_CONFIG`.
pub fn config_file(facts: &Facts) -> Result<ConfigFile, String> {
    let (path, explicit) = match &facts.oxide_config {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("OXIDE_CONFIG must be an absolute path".to_owned());
            }
            (path, true)
        }
        None if facts.macos => (
            facts
                .home()?
                .join("Library/Application Support/oxide/config/config.toml"),
            false,
        ),
        None => (
            facts
                .xdg(facts.xdg_config_home.as_ref(), "XDG_CONFIG_HOME", ".config")?
                .join("oxide/config.toml"),
            false,
        ),
    };
    let present = match fs::symlink_metadata(&path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    Ok(ConfigFile {
        path,
        explicit,
        present,
    })
}

/// On macOS, `~/.config/oxide/config.toml` if it exists: never read.
pub fn ignored_legacy_config(facts: &Facts) -> Option<PathBuf> {
    if !facts.macos || facts.oxide_config.is_some() {
        return None;
    }
    let path = facts.home().ok()?.join(".config/oxide/config.toml");
    fs::symlink_metadata(&path).ok().map(|_| path)
}

/// Resolve the effective settings: environment, then file, then default.
pub fn settings(facts: &Facts) -> Result<Settings, String> {
    let file = config_file(facts)?;
    let parsed = if file.present {
        read_config(&file.path)?
    } else {
        FileConfig::default()
    };
    let section = parsed.pool.unwrap_or_default();
    let named = |error: String| format!("{}: {error}", file.path.display());
    let (pool, pool_source) = match &facts.oxide_pool {
        Some(path) => {
            let path = PathBuf::from(path);
            let path = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(path))
                    .map_err(|error| format!("cannot resolve OXIDE_POOL: {error}"))?
            };
            (path, Source::Environment("OXIDE_POOL"))
        }
        None => match &section.path {
            Some(text) => (expand_path(text, facts).map_err(named)?, Source::File),
            None => (facts.cache_dir()?.join(DEFAULT_POOL_LEAF), Source::Default),
        },
    };
    let (budget, budget_source) = match &facts.oxide_pool_cap {
        Some(value) => {
            let value = value.to_str().ok_or("OXIDE_POOL_CAP must be valid UTF-8")?;
            (
                parse_size(value).map_err(|error| format!("OXIDE_POOL_CAP={value} {error}"))?,
                Source::Environment("OXIDE_POOL_CAP"),
            )
        }
        None => match &section.budget {
            Some(Budget::Bytes(bytes)) => (
                check_size(*bytes).map_err(|error| named(format!("[pool] budget {error}")))?,
                Source::File,
            ),
            Some(Budget::Text(text)) => (
                parse_size(text).map_err(|error| named(format!("[pool] budget {error}")))?,
                Source::File,
            ),
            None => (DEFAULT_BUDGET_BYTES, Source::Default),
        },
    };
    Ok(Settings {
        pool,
        pool_source,
        budget,
        budget_source,
        file,
    })
}

fn read_config(path: &Path) -> Result<FileConfig, String> {
    let named = |error: String| format!("{}: {error}", path.display());
    let metadata = fs::metadata(path).map_err(|error| named(error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(named("not a small regular file".to_owned()));
    }
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_CONFIG_BYTES).read_to_string(&mut text))
        .map_err(|error| named(error.to_string()))?;
    parse_config(&text).map_err(named)
}

fn parse_config(text: &str) -> Result<FileConfig, String> {
    basic_toml::from_str(text).map_err(|error| format!("invalid config: {error}"))
}

/// `~` or `~/...` expands to HOME; the result must be absolute and plain
/// (no `.` or `..` components).
fn expand_path(text: &str, facts: &Facts) -> Result<PathBuf, String> {
    let path = if text == "~" {
        facts.home()?
    } else if let Some(rest) = text.strip_prefix("~/") {
        facts.home()?.join(rest)
    } else if text.starts_with('~') {
        return Err(format!("[pool] path {text:?}: only ~ or ~/ expands"));
    } else {
        PathBuf::from(text)
    };
    if !path.is_absolute() {
        return Err(format!(
            "[pool] path {text:?} must be absolute after ~ expansion"
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!("[pool] path {text:?} must not contain . or .."));
    }
    Ok(path)
}

/// A byte count: whole bytes, or a whole number of decimal (`KB`, `MB`,
/// `GB`, `TB`) or binary (`KiB`, `MiB`, `GiB`, `TiB`) units. 1 MiB to
/// 16 TiB.
pub fn parse_size(value: &str) -> Result<u64, String> {
    let digits = value.bytes().take_while(u8::is_ascii_digit).count();
    let (number, unit) = value.split_at(digits);
    let scale: u64 = match unit {
        "" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => 0,
    };
    let bytes = number
        .parse::<u64>()
        .ok()
        .filter(|_| scale > 0)
        .and_then(|number| number.checked_mul(scale))
        .ok_or("is not a byte count or a whole number of KB, MB, GB, TB, KiB, MiB, GiB or TiB")?;
    check_size(bytes)
}

fn check_size(bytes: u64) -> Result<u64, String> {
    const MIN: u64 = 1 << 20;
    const MAX: u64 = 16 << 40;
    if (MIN..=MAX).contains(&bytes) {
        Ok(bytes)
    } else {
        Err("must be from 1 MiB to 16 TiB".to_owned())
    }
}

/// Refuse a pool path inside a Git work tree or on a mounted external
/// volume (`/Volumes/...`, where a removable volume is registered), after
/// resolving symlinks in its nearest existing ancestor. Unknown refuses.
pub fn refuse_unsafe_pool_path(path: &Path) -> Result<(), String> {
    // `..` after a missing component would resolve only once the missing
    // parents exist, past this check: refuse it outright.
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "the pool path {} must not contain . or .. components",
            path.display()
        ));
    }
    let mut existing = path;
    while fs::symlink_metadata(existing).is_err() {
        existing = existing
            .parent()
            .ok_or_else(|| format!("the pool path {} has no existing ancestor", path.display()))?;
    }
    let canonical = fs::canonicalize(existing)
        .map_err(|error| format!("cannot resolve the pool path {}: {error}", path.display()))?;
    if canonical.starts_with("/Volumes") {
        return Err(format!(
            "the pool path {} is on an external volume ({}); a removable volume is registered with `oxide pool register-removable`, not used as the internal pool",
            path.display(),
            canonical.display()
        ));
    }
    if let Some(mount) = mount_point(&canonical)? {
        if mount.starts_with("/Volumes") {
            return Err(format!(
                "the pool path {} is on the external volume mounted at {}",
                path.display(),
                mount.display()
            ));
        }
    }
    for ancestor in canonical.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                return Err(format!(
                    "the pool path {} is inside the Git work tree {}; choose a path outside any repository",
                    path.display(),
                    ancestor.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot check {} for a Git work tree: {error}",
                    ancestor.display()
                ))
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mount_point(path: &Path) -> Result<Option<PathBuf>, String> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "the pool path contains a NUL byte".to_owned())?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: c_path is NUL-terminated; statfs initializes the record on 0.
    if unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(format!(
            "cannot identify the volume of {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: zero return means the record is initialized.
    let stat = unsafe { stat.assume_init() };
    let bytes: Vec<u8> = stat
        .f_mntonname
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    Ok(Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes))))
}

#[cfg(not(target_os = "macos"))]
fn mount_point(_path: &Path) -> Result<Option<PathBuf>, String> {
    Ok(None)
}

/// What sits at a pool path, judged read-only: config files and names only,
/// never the lock, an attach or a write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Found {
    Absent,
    /// The retired `apfs-sparse-image-v1` backend: an image or receipt, or a
    /// config recording it.
    LegacyImage,
    /// A budget-only directory pool.
    Directory,
    /// Something Oxide does not recognise.
    Unknown,
}

pub fn inspect(path: &Path) -> Found {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Found::Absent,
        Err(_) => return Found::Unknown,
        Ok(_) => {}
    }
    // Judge what an alias names, so a symlink to a legacy pool is legacy.
    let Ok(resolved) = fs::canonicalize(path) else {
        return Found::Unknown;
    };
    if !fs::metadata(&resolved).is_ok_and(|metadata| metadata.is_dir()) {
        return Found::Unknown;
    }
    let path = resolved.as_path();
    if fs::symlink_metadata(path.join("image.sparseimage")).is_ok()
        || fs::symlink_metadata(path.join("receipt.json")).is_ok()
    {
        return Found::LegacyImage;
    }
    let config = path.join("config.json");
    match fs::symlink_metadata(&config) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES => {}
        _ => return Found::Unknown,
    }
    let Ok(bytes) = fs::read(&config) else {
        return Found::Unknown;
    };
    let backend = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("backend")?.as_str().map(str::to_owned));
    match backend.as_deref() {
        Some("apfs-sparse-image-v1") => Found::LegacyImage,
        Some("directory-budget-v1") => Found::Directory,
        _ => Found::Unknown,
    }
}

/// The retired image pool's default path.
pub fn legacy_default_pool(facts: &Facts) -> Option<PathBuf> {
    facts.cache_dir().ok().map(|dir| dir.join(LEGACY_POOL_LEAF))
}

/// The built-in default pool path, whatever the current settings say.
pub fn default_pool(facts: &Facts) -> Option<PathBuf> {
    facts
        .cache_dir()
        .ok()
        .map(|dir| dir.join(DEFAULT_POOL_LEAF))
}

fn known_pools_file(facts: &Facts) -> Result<PathBuf, String> {
    Ok(facts.state_dir()?.join("pools.json"))
}

/// Informational record of pools Oxide created from the config file or the
/// default. Never ownership or reclaim proof; `status` lists the others as
/// orphaned. Bounded to the newest few.
pub fn known_pools(facts: &Facts) -> Vec<PathBuf> {
    let Ok(path) = known_pools_file(facts) else {
        return Vec::new();
    };
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_KNOWN_POOLS_BYTES => {}
        _ => return Vec::new(),
    }
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<PathBuf>>(&bytes).ok())
        .unwrap_or_default()
}

/// Add `pool` to the record. Best effort: a failure is reported, not fatal.
pub fn remember_pool(facts: &Facts, pool: &Path) -> Result<(), String> {
    let path = known_pools_file(facts)?;
    let mut pools = known_pools(facts);
    pools.retain(|known| known != pool);
    pools.push(pool.to_path_buf());
    let excess = pools.len().saturating_sub(MAX_KNOWN_POOLS);
    pools.drain(..excess);
    let parent = path.parent().ok_or("the state path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let bytes = serde_json::to_vec(&pools).map_err(|error| error.to_string())?;
    let temp = parent.join(format!("pools.json.tmp-{}", std::process::id()));
    fs::write(&temp, bytes)
        .and_then(|()| fs::rename(&temp, &path))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// The Time Machine exclusion of a pool root Oxide just created, or must
/// confirm before a build writes into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Backup {
    Excluded,
    NotExcluded(String),
    NotApplicable,
}

/// Exclude an owned pool root from Time Machine with the sticky,
/// unprivileged `tmutil addexclusion` and verify with `tmutil isexcluded`.
/// macOS only; elsewhere not applicable.
pub fn exclude_from_backup(pool: &Path) -> Backup {
    if !cfg!(target_os = "macos") {
        return Backup::NotApplicable;
    }
    let added = std::process::Command::new("/usr/bin/tmutil")
        .arg("addexclusion")
        .arg(pool)
        .stdin(std::process::Stdio::null())
        .output();
    match added {
        Ok(output) if output.status.success() => backup_state(pool),
        Ok(output) => Backup::NotExcluded(format!(
            "tmutil addexclusion failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Backup::NotExcluded(format!("cannot run tmutil: {error}")),
    }
}

/// `tmutil isexcluded` for the pool root.
pub fn backup_state(pool: &Path) -> Backup {
    if !cfg!(target_os = "macos") {
        return Backup::NotApplicable;
    }
    let checked = std::process::Command::new("/usr/bin/tmutil")
        .arg("isexcluded")
        .arg(pool)
        .stdin(std::process::Stdio::null())
        .output();
    match checked {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.trim_start().starts_with("[Excluded]") {
                Backup::Excluded
            } else {
                Backup::NotExcluded(format!("tmutil isexcluded: {}", text.trim()))
            }
        }
        Ok(output) => Backup::NotExcluded(format!(
            "tmutil isexcluded failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Backup::NotExcluded(format!("cannot run tmutil: {error}")),
    }
}

/// Cheap check before a build: the sticky exclusion is the
/// `com.apple.metadata:com_apple_backup_excludeItem` attribute `tmutil
/// addexclusion` sets on the root. Present means excluded; absent sends the
/// caller to [`exclude_from_backup`].
#[cfg(target_os = "macos")]
pub fn has_sticky_exclusion(pool: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(pool.as_os_str().as_bytes()) else {
        return false;
    };
    let name = c"com.apple.metadata:com_apple_backup_excludeItem";
    // SAFETY: both strings are NUL-terminated; a null buffer asks for size.
    let size = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            name.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    size >= 0
}

#[cfg(not(target_os = "macos"))]
pub fn has_sticky_exclusion(_pool: &Path) -> bool {
    true
}

/// Oxide applies no Spotlight exclusion: Apple documents only the Spotlight
/// Privacy list in System Settings, which needs the user. This is the honest
/// status line.
pub fn spotlight_advice(pool: &Path) -> String {
    if cfg!(target_os = "macos") {
        format!(
            "not excluded by Oxide; to stop indexing, add {} in System Settings > Spotlight > Search Privacy",
            pool.display()
        )
    } else {
        "not applicable".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(macos: bool) -> Facts {
        Facts {
            macos,
            home: Some("/home/user".into()),
            ..Facts::default()
        }
    }

    #[test]
    fn platform_config_locations() {
        let mac = config_file(&facts(true)).expect("macOS");
        assert_eq!(
            mac.path,
            Path::new("/home/user/Library/Application Support/oxide/config/config.toml")
        );
        assert!(!mac.explicit);
        let mut linux = facts(false);
        assert_eq!(
            config_file(&linux).expect("XDG default").path,
            Path::new("/home/user/.config/oxide/config.toml")
        );
        linux.xdg_config_home = Some("/xdg/config".into());
        assert_eq!(
            config_file(&linux).expect("XDG").path,
            Path::new("/xdg/config/oxide/config.toml")
        );
        linux.xdg_config_home = Some("relative".into());
        assert!(config_file(&linux).is_err());
        let mut explicit = facts(true);
        explicit.oxide_config = Some("/elsewhere/oxide.toml".into());
        let chosen = config_file(&explicit).expect("explicit");
        assert_eq!(chosen.path, Path::new("/elsewhere/oxide.toml"));
        assert!(chosen.explicit);
        explicit.oxide_config = Some("oxide.toml".into());
        assert!(config_file(&explicit).is_err());
        let mut homeless = facts(false);
        homeless.home = None;
        assert!(config_file(&homeless).is_err());
        homeless.oxide_config = Some("/explicit.toml".into());
        assert!(config_file(&homeless).is_ok());
    }

    #[test]
    fn default_pool_is_distinct_from_the_legacy_image_pool() {
        let mac = facts(true);
        assert_eq!(
            default_pool(&mac).expect("default"),
            Path::new("/home/user/Library/Caches/oxide/build-pool")
        );
        assert_eq!(
            legacy_default_pool(&mac).expect("legacy"),
            Path::new("/home/user/Library/Caches/oxide/pool")
        );
        let settings = settings(&mac).expect("settings without a file");
        assert_eq!(settings.pool_source, Source::Default);
        assert_eq!(settings.budget, 100_000_000_000);
        assert_eq!(settings.budget_source, Source::Default);
    }

    #[test]
    fn config_keys_types_and_units() {
        let parsed =
            parse_config("[pool]\npath = \"~/Dev/oxide\"\nbudget = \"100GB\"\n").expect("valid");
        let section = parsed.pool.expect("pool");
        assert_eq!(section.budget, Some(Budget::Text("100GB".to_owned())));
        assert_eq!(
            expand_path(section.path.as_deref().expect("path"), &facts(true)),
            Ok(PathBuf::from("/home/user/Dev/oxide"))
        );
        assert!(parse_config("[pool]\nbudget = 2000000\n").is_ok());
        assert!(parse_config("[pool]\nsize = \"1GB\"\n").is_err());
        assert!(parse_config("[other]\n").is_err());
        assert!(parse_config("[pool\n").is_err());
        assert!(parse_config("[pool]\npath = 3\n").is_err());
        assert!(expand_path("~other/x", &facts(true)).is_err());
        assert!(expand_path("relative/x", &facts(true)).is_err());
        assert!(expand_path("/a/../b", &facts(true)).is_err());
        assert_eq!(parse_size("100GB"), Ok(100_000_000_000));
        assert_eq!(parse_size("750000000000"), Ok(750_000_000_000));
        assert_eq!(parse_size("2GiB"), Ok(2 << 30));
        assert_eq!(parse_size("1MiB"), Ok(1 << 20));
        for invalid in ["", "0", "1.5GB", "-1", "1gb", "1 GB", "17TiB", "1000000"] {
            assert!(parse_size(invalid).is_err(), "{invalid}");
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let path = base.join(format!("oxide-config-{label}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("scratch");
            Self(fs::canonicalize(path).expect("canonical scratch"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn precedence_is_environment_then_file_then_default() {
        let scratch = Scratch::new("precedence");
        let file = scratch.0.join("config.toml");
        fs::write(&file, "[pool]\npath = \"/from/file\"\nbudget = \"2GB\"\n").expect("config");
        let mut chosen = facts(true);
        chosen.oxide_config = Some(file.clone().into());
        let from_file = settings(&chosen).expect("file");
        assert_eq!(from_file.pool, Path::new("/from/file"));
        assert_eq!(from_file.pool_source, Source::File);
        assert_eq!(from_file.budget, 2_000_000_000);
        assert_eq!(from_file.budget_source, Source::File);
        chosen.oxide_pool = Some("/from/env".into());
        chosen.oxide_pool_cap = Some("3GB".into());
        let from_env = settings(&chosen).expect("env");
        assert_eq!(from_env.pool, Path::new("/from/env"));
        assert_eq!(from_env.pool_source, Source::Environment("OXIDE_POOL"));
        assert_eq!(from_env.budget, 3_000_000_000);
        chosen.oxide_pool_cap = Some("lots".into());
        assert!(settings(&chosen).is_err());
        fs::write(&file, "[pool]\nbudgte = \"2GB\"\n").expect("typo");
        chosen.oxide_pool_cap = None;
        let refused = settings(&chosen).expect_err("unknown key");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
        fs::write(&file, "not toml at all [").expect("malformed");
        let refused = settings(&chosen).expect_err("malformed");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
    }

    #[test]
    fn macos_ignores_the_dot_config_location() {
        let scratch = Scratch::new("ignored");
        let legacy = scratch.0.join(".config/oxide");
        fs::create_dir_all(&legacy).expect("legacy dir");
        fs::write(legacy.join("config.toml"), "[pool]\npath = \"/never\"\n").expect("legacy");
        let mut mac = facts(true);
        mac.home = Some(scratch.0.clone().into());
        assert_eq!(
            ignored_legacy_config(&mac),
            Some(legacy.join("config.toml"))
        );
        let resolved = settings(&mac).expect("settings");
        assert_eq!(resolved.pool_source, Source::Default);
        assert!(!resolved.file.present);
        let mut linux = mac.clone();
        linux.macos = false;
        assert_eq!(ignored_legacy_config(&linux), None);
        assert_eq!(settings(&linux).expect("linux").pool, Path::new("/never"));
    }

    #[test]
    fn git_work_trees_and_aliases_refuse() {
        let scratch = Scratch::new("git");
        let repo = scratch.0.join("repo");
        fs::create_dir_all(repo.join(".git")).expect("repo");
        assert!(refuse_unsafe_pool_path(&repo.join("pool")).is_err());
        assert!(refuse_unsafe_pool_path(&repo.join("deep/missing/pool")).is_err());
        let alias = scratch.0.join("alias");
        std::os::unix::fs::symlink(&repo, &alias).expect("alias");
        assert!(refuse_unsafe_pool_path(&alias.join("pool")).is_err());
        let worktree = scratch.0.join("worktree");
        fs::create_dir_all(&worktree).expect("worktree");
        fs::write(worktree.join(".git"), "gitdir: elsewhere\n").expect("worktree link");
        assert!(refuse_unsafe_pool_path(&worktree.join("pool")).is_err());
        let plain = scratch.0.join("plain");
        fs::create_dir_all(&plain).expect("plain");
        assert_eq!(refuse_unsafe_pool_path(&plain.join("pool")), Ok(()));
        assert!(refuse_unsafe_pool_path(Path::new("/Volumes/Builds/pool")).is_err());
        // A missing component followed by .. would only resolve into the
        // work tree after creation: refused before any lookup.
        let traversal = plain.join("missing/../../repo/pool");
        let refused = refuse_unsafe_pool_path(&traversal).expect_err("traversal");
        assert!(refused.contains(".."), "{refused}");
        assert!(refuse_unsafe_pool_path(&plain.join("missing/../pool")).is_err());
        assert!(!plain.join("missing").exists());
    }

    #[test]
    fn inspection_is_read_only_and_names_the_legacy_shape() {
        let scratch = Scratch::new("inspect");
        assert_eq!(inspect(&scratch.0.join("absent")), Found::Absent);
        let image = scratch.0.join("image-pool");
        fs::create_dir_all(&image).expect("image pool");
        fs::write(image.join("image.sparseimage"), b"").expect("image");
        assert_eq!(inspect(&image), Found::LegacyImage);
        let alias = scratch.0.join("alias");
        std::os::unix::fs::symlink(&image, &alias).expect("alias");
        assert_eq!(inspect(&alias), Found::LegacyImage);
        let recorded = scratch.0.join("recorded");
        fs::create_dir_all(&recorded).expect("recorded");
        fs::write(
            recorded.join("config.json"),
            br#"{"backend":"apfs-sparse-image-v1"}"#,
        )
        .expect("config");
        assert_eq!(inspect(&recorded), Found::LegacyImage);
        let budget = scratch.0.join("budget");
        fs::create_dir_all(&budget).expect("budget");
        fs::write(
            budget.join("config.json"),
            br#"{"backend":"directory-budget-v1"}"#,
        )
        .expect("config");
        assert_eq!(inspect(&budget), Found::Directory);
        assert_eq!(inspect(&scratch.0), Found::Unknown);
    }

    #[test]
    fn known_pools_are_bounded_and_deduplicated() {
        let scratch = Scratch::new("known");
        let mut linux = facts(false);
        linux.xdg_state_home = Some(scratch.0.clone().into());
        for index in 0..20 {
            remember_pool(&linux, Path::new(&format!("/pools/{index}"))).expect("remember");
        }
        remember_pool(&linux, Path::new("/pools/5")).expect("again");
        let known = known_pools(&linux);
        assert_eq!(known.len(), MAX_KNOWN_POOLS);
        assert_eq!(
            known.last().map(PathBuf::as_path),
            Some(Path::new("/pools/5"))
        );
        assert_eq!(known.iter().filter(|path| path.ends_with("5")).count(), 1);
    }
}
