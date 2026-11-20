//! Machine configuration: pool location and budget, plus the host build limit.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (User
//! configuration). Precedence is environment, then the user config file,
//! then built-in defaults.
//! The file sits at the platform-native location: on macOS
//! `~/Library/Application Support/oxide/config/config.toml`, elsewhere
//! `$XDG_CONFIG_HOME/oxide/config.toml` (XDG's `~/.config` when unset or
//! relative). `OXIDE_CONFIG` selects another file. On macOS
//! `~/.config/oxide/config.toml` is never read; `status` says it is ignored.
//!
//! Config, cache and state roots come from `longhorn-storage-layout`
//! (`platform-native-v1`, leaf `oxide`), one root at a time when it is
//! needed, so an explicit `OXIDE_CONFIG` or `OXIDE_POOL` reads no platform
//! directory variable.

use crate::cargo_home;
use longhorn_storage_layout::{
    native_platform_directory_fact_from, resolve_storage_root, PlatformDirectoryFact,
    PlatformEnvironment, RootKind, StorageIdentity, StorageLayoutError, StorageLayoutRequest,
    TargetPlatform,
};
use serde::Deserialize;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

/// 100 GB, decimal: the built-in soft budget of the internal pool.
pub const DEFAULT_BUDGET_BYTES: u64 = 100_000_000_000;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_ACTIVE_LIMIT: usize = 32;
const FALLBACK_MAX_ACTIVE: usize = 2;
/// A larger result is not credible for a local macOS host and is not used
/// for automatic admission policy.
const MAX_PHYSICAL_CORES: usize = 1024;
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
    /// The home every Oxide storage root, `~` in config paths and the ignored
    /// macOS legacy config derive from: the account's home directory, never
    /// `$HOME` (a harness that overrides HOME still gets the machine pool).
    pub home: Option<OsString>,
    /// `$HOME` as the process saw it, only for the status notice.
    pub env_home: Option<OsString>,
    /// Why `home` holds what it does.
    pub home_source: HomeSource,
    /// A malformed or unsupported proof-control request; it refuses every
    /// configuration access, explicit pool and config included.
    pub control_error: Option<String>,
    pub directories: Directories,
    pub oxide_config: Option<OsString>,
    pub oxide_pool: Option<OsString>,
    pub oxide_pool_cap: Option<OsString>,
    pub oxide_max_active: Option<OsString>,
    pub physical_cores: PhysicalCoreCount,
}

/// Where the home fact came from.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum HomeSource {
    /// Injected by a unit test.
    #[default]
    Injected,
    /// The account's passwd entry (`getpwuid_r(geteuid())`).
    Account,
    /// `$HOME`, through the debug-build-only proof control.
    TestEnvironment,
    /// The account lookup failed; nothing derives from a home.
    Unavailable(String),
}

/// The debug-build-only proof control that restores `$HOME` as the home
/// input. Release builds never read it.
pub const TEST_HOME_FROM_ENV: &str = "OXIDE_TEST_HOME_FROM_ENV";

/// The account's real home directory from its passwd entry, looked up once
/// per process so every resolution in one invocation sees the same fact.
fn passwd_home() -> Result<OsString, String> {
    static HOME: std::sync::OnceLock<Result<OsString, String>> = std::sync::OnceLock::new();
    HOME.get_or_init(lookup_passwd_home).clone()
}

fn lookup_passwd_home() -> Result<OsString, String> {
    use std::ffi::CStr;
    let uid = unsafe { libc::geteuid() };
    let mut size = 1024usize;
    loop {
        let mut buffer = vec![0 as libc::c_char; size];
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer refers to live, correctly sized storage owned here.
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE && size < 1 << 20 {
            size *= 4;
            continue;
        }
        if code != 0 {
            return Err(format!(
                "the account home directory is unavailable (getpwuid_r: {})",
                std::io::Error::from_raw_os_error(code)
            ));
        }
        if result.is_null() || entry.pw_dir.is_null() {
            return Err(format!(
                "the account home directory is unavailable (no passwd entry for uid {uid})"
            ));
        }
        // SAFETY: getpwuid_r succeeded, so pw_dir is a NUL-terminated string in `buffer`.
        let bytes = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes();
        return passwd_dir(bytes);
    }
}

/// The passwd directory bytes as a home: non-empty and absolute, or refused.
fn passwd_dir(bytes: &[u8]) -> Result<OsString, String> {
    use std::os::unix::ffi::OsStrExt;
    let home = std::ffi::OsStr::from_bytes(bytes).to_owned();
    if bytes.is_empty() {
        return Err("the account home directory is unavailable (empty passwd home)".to_owned());
    }
    if !Path::new(&home).is_absolute() {
        return Err("the account home directory is unavailable (relative passwd home)".to_owned());
    }
    Ok(home)
}

/// What the proof controls ask for, validated before any configuration is
/// read so a malformed request can never fall through to the real account.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlRequest {
    None,
    HomeFromEnv,
}

fn proof_controls(
    debug: bool,
    var: &dyn Fn(&str) -> Option<OsString>,
) -> Result<ControlRequest, String> {
    let Some(value) = var(TEST_HOME_FROM_ENV) else {
        return Ok(ControlRequest::None);
    };
    if !debug {
        return Err(format!(
            "{TEST_HOME_FROM_ENV} is set, but this build has no proof controls"
        ));
    }
    if value == "1" {
        Ok(ControlRequest::HomeFromEnv)
    } else {
        Err(format!("{TEST_HOME_FROM_ENV} must be exactly 1"))
    }
}

/// The home input and its source, from an already validated control request.
fn resolve_home(
    request: &ControlRequest,
    env_home: Option<OsString>,
) -> (Option<OsString>, HomeSource) {
    let account = match request {
        ControlRequest::HomeFromEnv => return (env_home, HomeSource::TestEnvironment),
        ControlRequest::None => passwd_home(),
    };
    match account {
        Ok(home) => (Some(home), HomeSource::Account),
        Err(error) => (None, HomeSource::Unavailable(error)),
    }
}

/// Where platform directory facts come from.
#[derive(Clone, Debug)]
pub enum Directories {
    /// Longhorn reads only the variables of the one fact a root needs, when
    /// that root is resolved.
    Process,
    /// An injected snapshot for tests; the platform follows `Facts::macos`.
    Injected(PlatformEnvironment),
}

impl Default for Directories {
    /// Nothing ambient: an empty snapshot reports every fact missing.
    fn default() -> Self {
        Self::Injected(PlatformEnvironment::default())
    }
}

/// The physical-core fact used by the automatic active-build default.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum PhysicalCoreCount {
    #[default]
    NotQueried,
    Available(usize),
    Unavailable(String),
}

/// The resolved host-wide command limit and the basis status reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveBuildLimit {
    pub value: usize,
    pub source: Source,
    detail: Option<String>,
}

impl ActiveBuildLimit {
    pub fn provenance(&self, config_path: &Path) -> String {
        let source = match self.source {
            Source::Environment(name) => name.to_owned(),
            Source::File => format!("config file {} [pool].max_active", config_path.display()),
            Source::Default => "built-in default".to_owned(),
        };
        match &self.detail {
            Some(detail) => format!("{source}; {detail}"),
            None => source,
        }
    }
}

impl Facts {
    /// The account's absolute home, the one `~/.local/state/oxide/install/` and
    /// `~/.local/bin/oxide` derive from. `$HOME` is never consulted: a harness
    /// that overrides it must not move where the install receipt is read.
    pub fn account_home(&self) -> Option<PathBuf> {
        self.home
            .as_ref()
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
    }

    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        let env_home = var("HOME");
        let (control_error, (home, home_source)) =
            match proof_controls(cfg!(debug_assertions), &|name| std::env::var_os(name)) {
                Ok(request) => (None, resolve_home(&request, env_home.clone())),
                Err(error) => (Some(error.clone()), (None, HomeSource::Unavailable(error))),
            };
        Self {
            macos: cfg!(target_os = "macos"),
            home,
            env_home,
            home_source,
            control_error,
            directories: Directories::Process,
            oxide_config: var("OXIDE_CONFIG"),
            oxide_pool: var("OXIDE_POOL"),
            oxide_pool_cap: std::env::var_os("OXIDE_POOL_CAP"),
            oxide_max_active: std::env::var_os("OXIDE_MAX_ACTIVE"),
            physical_cores: PhysicalCoreCount::NotQueried,
        }
    }

    /// A malformed proof-control request refuses before anything is read.
    fn check_controls(&self) -> Result<(), String> {
        self.control_error.clone().map_or(Ok(()), Err)
    }

    fn home(&self) -> Result<PathBuf, String> {
        self.check_controls()?;
        let home = self
            .home
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| self.missing_home())?;
        if home.is_absolute() {
            Ok(home)
        } else {
            Err("the home directory is not an absolute path".to_owned())
        }
    }

    fn missing_home(&self) -> String {
        match &self.home_source {
            HomeSource::Unavailable(reason) => {
                format!("{reason}; set OXIDE_CONFIG and OXIDE_POOL explicitly")
            }
            _ => "HOME is unset; set OXIDE_CONFIG and OXIDE_POOL explicitly".to_owned(),
        }
    }

    /// Whether `$HOME` differs from an account home Oxide resolved from.
    fn home_differs(&self) -> bool {
        matches!(self.home_source, HomeSource::Account)
            && self.home.is_some()
            && self.env_home != self.home
    }

    /// The status `home:` line: which home Oxide used and why.
    pub fn home_line(&self) -> String {
        let home = || {
            self.home
                .as_deref()
                .map_or_else(|| "none".to_owned(), shown)
        };
        let line = match &self.home_source {
            HomeSource::Account => format!("account home {} (passwd entry)", home()),
            HomeSource::TestEnvironment => {
                format!("HOME {} (proof control {TEST_HOME_FROM_ENV})", home())
            }
            HomeSource::Injected => format!("{} (injected)", home()),
            HomeSource::Unavailable(reason) => format!("unavailable ({})", bounded(reason)),
        };
        if self.home_differs() {
            format!("{line}; HOME is {}", self.env_home_shown())
        } else {
            line
        }
    }

    fn env_home_shown(&self) -> String {
        self.env_home
            .as_deref()
            .map_or_else(|| "unset".to_owned(), shown)
    }

    /// One stderr line when `$HOME` differs from the account home and the
    /// pool is the built-in default under that home; `None` when an explicit
    /// override (`OXIDE_POOL`, a config-file path or an honoured XDG cache
    /// root) chose the pool.
    fn home_notice(&self, pool: &Path, pool_source: Source) -> Option<String> {
        if !self.home_differs() || pool_source != Source::Default || !self.cache_from_home() {
            return None;
        }
        Some(format!(
            "HOME is {}; using the account pool at {} (account home {}); set OXIDE_POOL to isolate",
            self.env_home_shown(),
            shown(pool.as_os_str()),
            self.home
                .as_deref()
                .map_or_else(|| "none".to_owned(), shown),
        ))
    }

    /// One platform-native root under the `oxide` leaf. Only this root's
    /// platform fact is read; Oxide's file names stay beneath it.
    fn storage_root(&self, kind: RootKind) -> Result<PathBuf, String> {
        self.root_from(kind, None)
    }

    /// Whether the home chose the cache root, and so the default pool: the
    /// root moves when only the home input changes. An honoured XDG cache
    /// root does not, even when it names the home's default path.
    fn cache_from_home(&self) -> bool {
        // Two fixed alternatives, so the probe always differs from the home.
        let probe = [
            "/oxide-home-provenance-probe-a",
            "/oxide-home-provenance-probe-b",
        ]
        .into_iter()
        .map(Path::new)
        .find(|probe| self.home.as_deref().map(Path::new) != Some(*probe))
        .unwrap_or(Path::new("/"));
        matches!(
            (self.root_from(RootKind::Cache, None), self.root_from(RootKind::Cache, Some(probe))),
            (Ok(actual), Ok(moved)) if actual != moved
        )
    }

    /// One root from the process or injected facts, with the home input
    /// replaced by `home` when given. The resolver reads no files.
    fn root_from(&self, kind: RootKind, home: Option<&Path>) -> Result<PathBuf, String> {
        self.check_controls()?;
        let fact = match kind {
            RootKind::Config => PlatformDirectoryFact::Config,
            RootKind::Cache => PlatformDirectoryFact::Cache,
            RootKind::State => PlatformDirectoryFact::State,
            _ => return Err(format!("{kind:?} is not an Oxide storage root")),
        };
        let macos = match &self.directories {
            // Longhorn also has a Windows policy; Oxide keeps to the two
            // platforms it resolved before.
            Directories::Process if !cfg!(any(target_os = "macos", target_os = "linux")) => {
                return Err("no Oxide storage-root policy for this platform".to_owned());
            }
            Directories::Process => cfg!(target_os = "macos"),
            Directories::Injected(_) => self.macos,
        };
        let platform = if macos {
            TargetPlatform::MacOs
        } else {
            TargetPlatform::Linux
        };
        let environment = match &self.directories {
            Directories::Process => self.process_environment(fact),
            Directories::Injected(environment) => environment.clone(),
        };
        let environment = match home {
            Some(home) => environment.with_home_dir(Some(home.to_path_buf())),
            None => environment,
        };
        let facts = native_platform_directory_fact_from(platform, fact, &environment);
        let identity = StorageIdentity::new("oxide").map_err(|error| error.to_string())?;
        match resolve_storage_root(&StorageLayoutRequest::new(identity, facts), kind) {
            Ok(Some(root)) => Ok(root.path().to_path_buf()),
            Ok(None) => Err(format!("no {kind:?} root resolved")),
            Err(StorageLayoutError::MissingPlatformFact { .. }) => Err(match &self.home_source {
                HomeSource::Unavailable(reason) => {
                    format!("{reason}; set OXIDE_CONFIG and OXIDE_POOL explicitly")
                }
                _ => "HOME is unset or not an absolute path; set OXIDE_CONFIG and OXIDE_POOL explicitly"
                    .to_owned(),
            }),
            Err(error) => Err(error.to_string()),
        }
    }

    /// The one fact's inputs: the resolved home in place of `$HOME`, and on
    /// Linux the XDG variable for that fact, read from the process as Longhorn
    /// would.
    fn process_environment(&self, fact: PlatformDirectoryFact) -> PlatformEnvironment {
        let home = self.home.as_ref().map(PathBuf::from);
        let variable = |name: &str| std::env::var_os(name).map(PathBuf::from);
        let environment = PlatformEnvironment::default().with_home_dir(home);
        if cfg!(target_os = "macos") {
            return environment;
        }
        match fact {
            PlatformDirectoryFact::Config => {
                environment.with_xdg_config_home(variable("XDG_CONFIG_HOME"))
            }
            PlatformDirectoryFact::Cache => {
                environment.with_xdg_cache_home(variable("XDG_CACHE_HOME"))
            }
            PlatformDirectoryFact::State => {
                environment.with_xdg_state_home(variable("XDG_STATE_HOME"))
            }
            _ => environment,
        }
    }

    fn cache_dir(&self) -> Result<PathBuf, String> {
        self.storage_root(RootKind::Cache)
    }

    pub(crate) fn state_dir(&self) -> Result<PathBuf, String> {
        self.storage_root(RootKind::State)
    }
}

/// A path for a one-line message: control characters escaped, bounded.
fn shown(value: &std::ffi::OsStr) -> String {
    bounded(&Path::new(value).display().to_string())
}

/// Our longest one-line mention of a path or other text.
const BOUNDED_CHARS: usize = 240;

pub(crate) fn bounded(text: &str) -> String {
    escape_bounded(text, BOUNDED_CHARS)
}

/// One line, bounded, in one allocation: control characters escaped and the
/// ESCAPED text cut at `limit` characters with a trailing ellipsis. Every
/// one-line mention of caller-supplied text goes through this.
pub(crate) fn escape_bounded(text: &str, limit: usize) -> String {
    let mut escaped = String::new();
    let mut written = 0_usize;
    for character in text.chars() {
        if written == limit {
            escaped.push('…');
            return escaped;
        }
        if !character.is_control() {
            escaped.push(character);
            written += 1;
            continue;
        }
        for piece in character.escape_default() {
            if written == limit {
                escaped.push('…');
                return escaped;
            }
            escaped.push(piece);
            written += 1;
        }
    }
    escaped
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
    max_active: Option<i64>,
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
    pub active_build_limit: ActiveBuildLimit,
    pub ignored_legacy_config: Option<PathBuf>,
    pub file: ConfigFile,
    /// Which home Oxide used and why, for `status`.
    pub home_line: String,
    /// Set when `$HOME` differs from the account home and the pool is the
    /// built-in default under that home.
    pub home_notice: Option<String>,
}

/// The config file the platform selects, or `OXIDE_CONFIG`.
pub fn config_file(facts: &Facts) -> Result<ConfigFile, String> {
    facts.check_controls()?;
    let (path, explicit) = match &facts.oxide_config {
        Some(path) => {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("OXIDE_CONFIG must be an absolute path".to_owned());
            }
            (path, true)
        }
        None => (
            facts.storage_root(RootKind::Config)?.join("config.toml"),
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
    facts.check_controls()?;
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
    let active_build_limit = resolve_active_build_limit(facts, section.max_active, &file.path)?;
    let ignored_legacy_config = ignored_legacy_config(facts);
    let home_notice = facts.home_notice(&pool, pool_source);
    Ok(Settings {
        pool,
        pool_source,
        budget,
        budget_source,
        active_build_limit,
        ignored_legacy_config,
        home_line: facts.home_line(),
        home_notice,
        file,
    })
}

/// Why an explicit `OXIDE_POOL` value that ends in an `oxide status` display
/// annotation is unusable, whatever the selected path holds (oxide#182). The
/// suffix is not part of a path, so an absent pool and an existing ordinary
/// directory both refuse with the same hint. Another source, or a value
/// without that suffix, is `Ok(())` here; the ordinary rules still apply.
pub fn pool_annotation_refused(pool: &Path, source: Source) -> Result<(), String> {
    if !matches!(source, Source::Environment("OXIDE_POOL")) {
        return Ok(());
    }
    let Some(annotation) = pasted_annotation(pool) else {
        return Ok(());
    };
    let target = pool.display().to_string();
    let bare = target
        .strip_suffix(&annotation)
        .unwrap_or(&target)
        .trim_end();
    Err(format!(
        "OXIDE_POOL is {:?}, which looks pasted from `oxide status`; the trailing {annotation:?} is a display annotation, not part of the path. Set OXIDE_POOL to {bare:?}",
        target
    ))
}

/// Whether an absent pool may be created (oxide#182).
///
/// The built-in default and a config-file pool path create on first use as
/// before. A pool named by `OXIDE_POOL` is an explicit path, so with no pool
/// there Oxide refuses instead of creating one silently, naming the path; two
/// opt-ins allow creation: `OXIDE_POOL_CREATE=1`, and a path strictly inside
/// the caller's canonical `OXIDE_TASK_SCRATCH_ROOT` (so proof fixtures keep
/// their implicit creation). A value that ends in a `status` display
/// annotation (`<path> (from …)`) always refuses with a hint, because that
/// annotation is not part of a path. Existing pools never reach this rule.
pub fn pool_creation_allowed(
    pool: &Path,
    source: Source,
    opt_in: Option<&std::ffi::OsStr>,
    scratch: Option<&std::ffi::OsStr>,
) -> Result<(), String> {
    if !matches!(source, Source::Environment("OXIDE_POOL")) {
        return Ok(());
    }
    pool_annotation_refused(pool, source)?;
    if opt_in == Some(std::ffi::OsStr::new("1")) {
        return Ok(());
    }
    if scratch.is_some_and(|root| strictly_inside_scratch(pool, Path::new(root))) {
        return Ok(());
    }
    Err(format!(
        "OXIDE_POOL names {}, which holds no Oxide pool; Oxide does not create a pool at an explicit path implicitly. Set OXIDE_POOL_CREATE=1 to create it there, or point OXIDE_POOL at an existing pool. The config file and the built-in default pool are unchanged",
        pool.display()
    ))
}

/// The ` (from …)` suffix `oxide status` prints after a pool path, if the
/// value carries it. The pool path itself never ends with this shape.
fn pasted_annotation(pool: &Path) -> Option<String> {
    let text = pool.to_string_lossy().into_owned();
    if !text.ends_with(')') {
        return None;
    }
    let at = text.rfind(" (from ")?;
    let inner = &text[at + " (from ".len()..text.len() - 1];
    if inner.is_empty() || inner.contains(['(', ')']) {
        return None;
    }
    Some(text[at..].to_owned())
}

/// Whether `pool` is strictly inside the canonical `scratch` directory. The
/// pool usually does not exist yet, so its deepest existing ancestor is
/// canonicalized and the missing tail is appended unchanged; no directory is
/// created and no link is followed beyond what the filesystem already has.
fn strictly_inside_scratch(pool: &Path, scratch: &Path) -> bool {
    let Ok(scratch) = fs::canonicalize(scratch) else {
        return false;
    };
    let Some(pool) = resolve_through_existing_ancestor(pool) else {
        return false;
    };
    pool != scratch && pool.starts_with(&scratch)
}

fn resolve_through_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs::canonicalize(&existing) {
            Ok(mut canonical) => {
                for part in tail.iter().rev() {
                    canonical.push(part);
                }
                return Some(canonical);
            }
            Err(_) => {
                let name = existing.file_name()?.to_os_string();
                tail.push(name);
                if !existing.pop() {
                    return None;
                }
            }
        }
    }
}

fn resolve_active_build_limit(
    facts: &Facts,
    config_value: Option<i64>,
    config_path: &Path,
) -> Result<ActiveBuildLimit, String> {
    if let Some(value) = &facts.oxide_max_active {
        let value = value
            .to_str()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| (1..=MAX_ACTIVE_LIMIT).contains(value))
            .ok_or_else(|| {
                format!("OXIDE_MAX_ACTIVE must be an integer from 1 to {MAX_ACTIVE_LIMIT}")
            })?;
        return Ok(ActiveBuildLimit {
            value,
            source: Source::Environment("OXIDE_MAX_ACTIVE"),
            detail: None,
        });
    }
    if let Some(value) = config_value {
        let value = usize::try_from(value)
            .ok()
            .filter(|value| (1..=MAX_ACTIVE_LIMIT).contains(value))
            .ok_or_else(|| {
                format!(
                    "{}: [pool] max_active must be an integer from 1 to {MAX_ACTIVE_LIMIT}",
                    config_path.display()
                )
            })?;
        return Ok(ActiveBuildLimit {
            value,
            source: Source::File,
            detail: None,
        });
    }

    let fallback = |reason: String| ActiveBuildLimit {
        value: FALLBACK_MAX_ACTIVE,
        source: Source::Default,
        detail: Some(format!(
            "physical-core fallback: {reason}; using {FALLBACK_MAX_ACTIVE}"
        )),
    };
    if !facts.macos {
        return Ok(fallback(
            "physical-core query unsupported on this platform".to_owned(),
        ));
    }
    let queried;
    let physical_cores = match &facts.physical_cores {
        PhysicalCoreCount::NotQueried => {
            queried = physical_core_count();
            &queried
        }
        fact => fact,
    };
    match physical_cores {
        PhysicalCoreCount::Available(cores) if (1..=MAX_PHYSICAL_CORES).contains(cores) => {
            let value = (*cores / 4).clamp(FALLBACK_MAX_ACTIVE, 4);
            Ok(ActiveBuildLimit {
                value,
                source: Source::Default,
                detail: Some(format!(
                    "physical cores from hw.physicalcpu={cores}; formula min(4, max(2, floor({cores}/4)))"
                )),
            })
        }
        PhysicalCoreCount::Available(cores) => Ok(fallback(format!(
            "hw.physicalcpu returned invalid count {cores} (expected 1..={MAX_PHYSICAL_CORES})"
        ))),
        PhysicalCoreCount::Unavailable(reason) => {
            Ok(fallback(format!("hw.physicalcpu unavailable ({reason})")))
        }
        PhysicalCoreCount::NotQueried => unreachable!("physical-core query was attempted"),
    }
}

#[cfg(target_os = "macos")]
fn physical_core_count() -> PhysicalCoreCount {
    let name = b"hw.physicalcpu\0";
    let mut count: libc::c_int = 0;
    let mut length = std::mem::size_of_val(&count);
    // SAFETY: `name` is NUL-terminated, `count` and `length` are writable,
    // and no new value is supplied to sysctl.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut count as *mut libc::c_int).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return PhysicalCoreCount::Unavailable(std::io::Error::last_os_error().to_string());
    }
    if length != std::mem::size_of_val(&count) {
        return PhysicalCoreCount::Unavailable(format!(
            "hw.physicalcpu returned {length} bytes, expected {}",
            std::mem::size_of_val(&count)
        ));
    }
    if count <= 0 {
        return PhysicalCoreCount::Unavailable(format!(
            "hw.physicalcpu returned nonpositive count {count}"
        ));
    }
    match usize::try_from(count) {
        Ok(count) => PhysicalCoreCount::Available(count),
        Err(_) => PhysicalCoreCount::Unavailable(format!(
            "hw.physicalcpu count {count} does not fit usize"
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn physical_core_count() -> PhysicalCoreCount {
    PhysicalCoreCount::Unavailable("unsupported platform".to_owned())
}

fn read_config(path: &Path) -> Result<FileConfig, String> {
    let named = |error: String| format!("{}: {error}", path.display());
    let metadata = fs::metadata(path).map_err(|error| named(error.to_string()))?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(named("not a small regular file".to_owned()));
    }
    let source = fs::File::open(path).map_err(|error| named(error.to_string()))?;
    let text = read_bounded_config(source, named)?;
    parse_config(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Read a config of at most `MAX_CONFIG_BYTES`. A source that holds more, for
/// example a file that grew after its size was read, is refused: a truncated
/// read would parse as a different config without any error.
fn read_bounded_config(
    source: impl Read,
    named: impl Fn(String) -> String,
) -> Result<String, String> {
    let mut text = String::new();
    source
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| named(error.to_string()))?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(named(format!(
            "grew past {MAX_CONFIG_BYTES} bytes while it was read"
        )));
    }
    Ok(text)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PathRefusal {
    Traversal(String),
    Ancestor(String),
    ExternalVolume { message: String, canonical: PathBuf },
    GitWorkTree(String),
}

impl PathRefusal {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Traversal(message)
            | Self::Ancestor(message)
            | Self::GitWorkTree(message)
            | Self::ExternalVolume { message, .. } => message,
        }
    }
}

/// Refuse unsafe paths, preserving the ordinary admission messages and order.
pub fn refuse_unsafe_pool_path(path: &Path) -> Result<(), String> {
    judge_pool_path(path).map_err(PathRefusal::message)
}

pub(crate) fn judge_pool_path(path: &Path) -> Result<(), PathRefusal> {
    // `..` after a missing component would resolve only once the missing
    // parents exist, past this check: refuse it outright.
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(PathRefusal::Traversal(format!(
            "the pool path {} must not contain . or .. components",
            path.display()
        )));
    }
    let mut existing = path;
    // Only a missing component moves the check up; any other failure to
    // inspect one (such as a denied search) refuses rather than being read as
    // absence.
    while let Err(error) = fs::symlink_metadata(existing) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(PathRefusal::Ancestor(format!(
                "cannot inspect the pool path {}: {error}",
                path.display()
            )));
        }
        existing = existing.parent().ok_or_else(|| {
            PathRefusal::Ancestor(format!(
                "the pool path {} has no existing ancestor",
                path.display()
            ))
        })?;
    }
    let canonical = fs::canonicalize(existing).map_err(|error| {
        PathRefusal::Ancestor(format!(
            "cannot resolve the pool path {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(test)]
    if internal_fixture() {
        return judge_git_work_tree(path, &canonical);
    }
    refuse_external_prefix(path, &canonical)?;
    if let Some(mount) = mount_point(&canonical).map_err(PathRefusal::Ancestor)? {
        if mount.starts_with("/Volumes") {
            return Err(PathRefusal::ExternalVolume {
                message: format!(
                    "the pool path {} is on the external volume mounted at {}",
                    path.display(),
                    mount.display()
                ),
                canonical,
            });
        }
    }
    judge_git_work_tree(path, &canonical)
}

fn refuse_external_prefix(path: &Path, canonical: &Path) -> Result<(), PathRefusal> {
    if canonical.starts_with("/Volumes") {
        return Err(PathRefusal::ExternalVolume { message: format!(
            "the pool path {} is on an external volume ({}); a removable volume is registered with `oxide pool register-removable`, not used as the internal pool",
            path.display(), canonical.display()
        ), canonical: canonical.to_path_buf() });
    }
    Ok(())
}

pub(crate) fn judge_git_work_tree(path: &Path, canonical: &Path) -> Result<(), PathRefusal> {
    for ancestor in canonical.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => {
                return Err(PathRefusal::GitWorkTree(format!(
                    "the pool path {} is inside the Git work tree {}; choose a path outside any repository",
                    path.display(),
                    ancestor.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PathRefusal::GitWorkTree(format!(
                    "cannot check {} for a Git work tree: {error}",
                    ancestor.display()
                )))
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
    // Opened once, without following a final link: the type, the size and the
    // bytes all come from the same descriptor, so a name swapped after a
    // separate check cannot redirect the read.
    let config = path.join("config.json");
    let Ok(Some(bytes)) = cargo_home::read_regular(&config, MAX_CONFIG_BYTES, false) else {
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
    cargo_home::read_regular(&path, MAX_KNOWN_POOLS_BYTES, false)
        .ok()
        .flatten()
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
    // A leftover from an earlier run with this PID is ours and stale: clear it
    // so the exclusive create below names only this write.
    let _ = fs::remove_file(&temp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, &bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&temp, &path));
    written.map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("{}: {error}", path.display())
    })
}

/// The Time Machine exclusion of a pool root Oxide just created, or must
/// confirm before a build writes into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Backup {
    Excluded,
    NotExcluded(String),
    NotApplicable,
}

/// The `tmutil` a host runs. The program is a field so a test can replace it
/// with a script that records its own invocations; production always names
/// the platform binary.
#[derive(Clone, Debug)]
struct Tmutil {
    program: std::path::PathBuf,
}

impl Tmutil {
    fn host() -> Self {
        Self {
            program: std::path::PathBuf::from("/usr/bin/tmutil"),
        }
    }

    /// `tmutil isexcluded` for the pool root: the effective exclusion state,
    /// including one inherited from an excluded parent.
    fn state(&self, pool: &Path) -> Backup {
        let mut command = std::process::Command::new(&self.program);
        command
            .arg("isexcluded")
            .arg(pool)
            .stdin(std::process::Stdio::null());
        let checked = { crate::budget::output(&mut command) };
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

    /// The sticky, unprivileged exclusion itself.
    fn add(&self, pool: &Path) -> Backup {
        let mut command = std::process::Command::new(&self.program);
        command
            .arg("addexclusion")
            .arg(pool)
            .stdin(std::process::Stdio::null());
        let added = { crate::budget::output(&mut command) };
        match added {
            Ok(output) if output.status.success() => Backup::Excluded,
            Ok(output) => Backup::NotExcluded(format!(
                "tmutil addexclusion failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => Backup::NotExcluded(format!("cannot run tmutil: {error}")),
        }
    }
}

/// Exclude an owned pool root from Time Machine and verify the result.
///
/// The cheap `tmutil isexcluded` runs first: on this host `tmutil
/// addexclusion` takes about eleven seconds, and a pool inside an already
/// excluded parent (a scratch volume, for example) is effectively excluded
/// without it. Only a genuinely included root pays for `addexclusion`, and
/// the result is verified either way. macOS only; elsewhere not applicable.
pub fn exclude_from_backup(pool: &Path) -> Backup {
    exclude_with(&Tmutil::host(), pool)
}

fn exclude_with(tmutil: &Tmutil, pool: &Path) -> Backup {
    if !cfg!(target_os = "macos") {
        return Backup::NotApplicable;
    }
    match tmutil.state(pool) {
        Backup::Excluded => return Backup::Excluded,
        Backup::NotApplicable => return Backup::NotApplicable,
        // Not excluded yet: add the sticky attribute and verify it.
        Backup::NotExcluded(_) => {}
    }
    match tmutil.add(pool) {
        Backup::NotApplicable => Backup::NotApplicable,
        Backup::NotExcluded(reason) => Backup::NotExcluded(reason),
        Backup::Excluded => tmutil.state(pool),
    }
}

/// `tmutil isexcluded` for the pool root.
pub fn backup_state(pool: &Path) -> Backup {
    if !cfg!(target_os = "macos") {
        return Backup::NotApplicable;
    }
    Tmutil::host().state(pool)
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
        at_home(macos, Some("/home/user"))
    }

    #[test]
    fn the_account_home_is_absolute_and_never_read_from_home() {
        let home = passwd_home().expect("the test account has a passwd home");
        assert!(Path::new(&home).is_absolute(), "{home:?}");
        assert_eq!(passwd_home(), Ok(home), "one fact per process");
    }

    /// Regression: the install receipt and provenance paths read `$HOME`. The
    /// account home is the only home they may use, even when `$HOME` differs.
    #[test]
    fn install_paths_derive_from_the_account_home_not_home() {
        let facts = Facts {
            home: Some(OsString::from("/Users/account")),
            env_home: Some(OsString::from("/tmp/harness-home")),
            ..Facts::default()
        };
        assert_eq!(facts.account_home(), Some(PathBuf::from("/Users/account")));
        let relative = Facts {
            home: Some(OsString::from("Users/account")),
            ..Facts::default()
        };
        assert_eq!(relative.account_home(), None);
        assert_eq!(Facts::default().account_home(), None);
    }

    #[test]
    fn a_passwd_home_must_be_present_and_absolute() {
        assert_eq!(
            passwd_dir(b"/Users/someone"),
            Ok(OsString::from("/Users/someone"))
        );
        let empty = passwd_dir(b"").expect_err("empty");
        assert!(empty.contains("empty passwd home"), "{empty}");
        let relative = passwd_dir(b"Users/someone").expect_err("relative");
        assert!(relative.contains("relative passwd home"), "{relative}");
    }

    /// A fresh scratch directory under the test scratch root, beside an
    /// `evidence` sibling that the pool fixtures use as an outside path.
    fn scratch_fixture() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .canonicalize()
            .expect("canonical test scratch");
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = base.join(format!("config-{}-{id}", std::process::id()));
        let scratch = root.join("scratch");
        fs::create_dir_all(&scratch).expect("scratch");
        fs::create_dir_all(root.join("evidence")).expect("evidence");
        scratch
    }

    fn controls(debug: bool, variables: &[(&str, OsString)]) -> Result<ControlRequest, String> {
        proof_controls(debug, &|name| {
            variables
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        })
    }

    fn with(
        variables: &[(&'static str, OsString)],
        extra: &[(&'static str, &str)],
    ) -> Vec<(&'static str, OsString)> {
        let mut all = variables.to_vec();
        for (name, value) in extra {
            all.retain(|(key, _)| key != name);
            all.push((name, OsString::from(value)));
        }
        all
    }

    /// oxide#182: an absent pool named by `OXIDE_POOL` refuses instead of
    /// being created silently, names the path, and honours exactly two
    /// opt-ins; a config-file or default pool path is unchanged. The
    /// counterfactual is the old unconditional `create_pool` call: it returns
    /// `Ok(())` for every case below.
    #[test]
    fn an_explicit_pool_is_not_created_silently() {
        let scratch = scratch_fixture();
        let pool = scratch.join("run.fixture/pool");
        let explicit = Source::Environment("OXIDE_POOL");
        let refuse = pool_creation_allowed(&pool, explicit, None, None);
        let error = refuse.expect_err("an outside explicit pool was accepted");
        assert!(
            error.contains(&pool.display().to_string()),
            "the refusal did not name the path: {error}"
        );
        assert!(error.contains("OXIDE_POOL_CREATE=1"), "{error}");
        assert!(error.contains("existing pool"), "{error}");

        // The config file and the built-in default are unchanged.
        assert!(pool_creation_allowed(&pool, Source::File, None, None).is_ok());
        assert!(pool_creation_allowed(&pool, Source::Default, None, None).is_ok());

        // A path strictly inside the task scratch root (whose tail need not
        // exist yet) is the fixture opt-in; the root itself and a path
        // outside it are not.
        let scratch_text = scratch.as_os_str();
        let inside = pool_creation_allowed(&pool, explicit, None, Some(scratch_text));
        assert!(inside.is_ok(), "{inside:?}");
        assert!(pool_creation_allowed(&scratch, explicit, None, Some(scratch_text)).is_err());
        let outside = scratch
            .parent()
            .expect("fixture root")
            .join("evidence/pool");
        let outside_pool = pool_creation_allowed(&outside, explicit, None, Some(scratch_text));
        assert!(outside_pool.is_err(), "a sibling path was accepted");

        // `OXIDE_POOL_CREATE=1` is the explicit opt-in; another value is not.
        let opted_in =
            pool_creation_allowed(&pool, explicit, Some(std::ffi::OsStr::new("1")), None);
        assert!(opted_in.is_ok(), "{opted_in:?}");
        assert!(
            pool_creation_allowed(&pool, explicit, Some(std::ffi::OsStr::new("true")), None)
                .is_err()
        );

        // A value pasted from `oxide status` refuses with a hint, even with
        // the opt-in set, and names the bare path it should be.
        let pasted = Path::new("/tmp/build-pool (from OXIDE_POOL)");
        let error = pool_creation_allowed(
            pasted,
            explicit,
            Some(std::ffi::OsStr::new("1")),
            Some(scratch_text),
        )
        .expect_err("a pasted status line was accepted");
        assert!(error.contains("pasted from `oxide status`"), "{error}");
        assert!(error.contains("/tmp/build-pool\""), "{error}");
        // A path that merely ends in a parenthesis is not an annotation.
        let ordinary = Path::new("/tmp/build-pool (fast)");
        assert!(
            pool_creation_allowed(ordinary, explicit, Some(std::ffi::OsStr::new("1")), None)
                .is_ok()
        );
    }

    /// oxide#182: a pasted `oxide status` suffix is not a path, so the hint
    /// applies whatever the path holds. The store calls this before its
    /// generic open error, which covers an existing ordinary directory
    /// (`Found::Unknown`) as well as an absent one (`Found::Absent`).
    #[test]
    fn a_pasted_status_annotation_refuses_for_every_pool_state() {
        let pasted = Path::new("/tmp/existing (from OXIDE_POOL)");
        let explicit = Source::Environment("OXIDE_POOL");
        let error = pool_annotation_refused(pasted, explicit)
            .expect_err("a pasted annotation was accepted");
        assert!(error.contains("pasted from `oxide status`"), "{error}");
        assert!(
            error.contains("Set OXIDE_POOL to \"/tmp/existing\""),
            "{error}"
        );
        assert!(pool_annotation_refused(pasted, Source::File).is_ok());
        assert!(pool_annotation_refused(pasted, Source::Default).is_ok());
        // Only the exact status shape is an annotation.
        assert!(pool_annotation_refused(Path::new("/tmp/existing"), explicit).is_ok());
        assert!(pool_annotation_refused(Path::new("/tmp/existing (fast)"), explicit).is_ok());
        assert!(pool_annotation_refused(Path::new("/tmp/x (from )"), explicit).is_ok());
        // The absent-pool rule keeps the same hint through its own entry.
        let absent = Path::new("/tmp/absent (from OXIDE_POOL)");
        let error = pool_creation_allowed(absent, explicit, None, None)
            .expect_err("the absent-pool rule lost the hint");
        assert!(error.contains("pasted from `oxide status`"), "{error}");
    }

    #[test]
    fn proof_controls_refuse_in_release_or_when_malformed() {
        assert_eq!(controls(false, &[]), Ok(ControlRequest::None));
        assert_eq!(controls(true, &[]), Ok(ControlRequest::None));
        let error = controls(false, &with(&[], &[(TEST_HOME_FROM_ENV, "1")])).expect_err("release");
        assert!(
            error.contains(TEST_HOME_FROM_ENV) && error.contains("no proof controls"),
            "{error}"
        );
        for value in ["true", "0", "", "1 "] {
            let error =
                controls(true, &with(&[], &[(TEST_HOME_FROM_ENV, value)])).expect_err(value);
            assert!(error.contains("exactly 1"), "{error}");
        }
        assert_eq!(
            controls(true, &with(&[], &[(TEST_HOME_FROM_ENV, "1")])),
            Ok(ControlRequest::HomeFromEnv)
        );
    }

    #[test]
    fn a_control_error_refuses_before_any_config_even_when_explicit() {
        let mut facts = facts(true);
        facts.oxide_pool = Some(OsString::from("/private/tmp/explicit-pool"));
        facts.oxide_config = Some(OsString::from("/private/tmp/explicit-config.toml"));
        facts.control_error = Some(format!("{TEST_HOME_FROM_ENV} must be exactly 1"));
        for error in [
            settings(&facts).expect_err("settings"),
            config_file(&facts).expect_err("config file"),
            facts.cache_dir().expect_err("cache root"),
            facts.home().expect_err("home"),
        ] {
            assert_eq!(error, format!("{TEST_HOME_FROM_ENV} must be exactly 1"));
        }
        assert_eq!(known_pools(&facts), Vec::<PathBuf>::new());
    }

    #[test]
    fn a_differing_home_is_noticed_only_for_the_default_pool_under_it() {
        let mut facts = facts(true);
        facts.home_source = HomeSource::Account;
        facts.env_home = Some(OsString::from("/private/tmp/harness-home"));
        let default = settings(&facts).expect("settings resolve");
        assert_eq!(
            default.home_notice.as_deref(),
            Some("HOME is /private/tmp/harness-home; using the account pool at /home/user/Library/Caches/oxide/build-pool (account home /home/user); set OXIDE_POOL to isolate")
        );
        assert_eq!(
            default.home_line,
            "account home /home/user (passwd entry); HOME is /private/tmp/harness-home"
        );

        facts.oxide_config = Some(OsString::from("/private/tmp/absent-config.toml"));
        assert!(
            settings(&facts).expect("settings").home_notice.is_some(),
            "an explicit config without a pool path leaves the default pool"
        );
        facts.oxide_config = None;

        facts.oxide_pool = Some(OsString::from("/private/tmp/explicit-pool"));
        let explicit = settings(&facts).expect("settings resolve");
        assert_eq!(explicit.home_notice, None, "OXIDE_POOL chose the pool");
        assert!(explicit
            .home_line
            .ends_with("; HOME is /private/tmp/harness-home"));
        facts.oxide_pool = None;

        facts.env_home = Some(OsString::from("/home/user"));
        let matching = settings(&facts).expect("settings resolve");
        assert_eq!(matching.home_notice, None, "a matching HOME is quiet");
        assert_eq!(matching.home_line, "account home /home/user (passwd entry)");

        facts.env_home = None;
        let unset = settings(&facts)
            .expect("settings")
            .home_notice
            .expect("unset HOME differs");
        assert!(unset.starts_with("HOME is unset;"), "{unset}");

        facts.home_source = HomeSource::TestEnvironment;
        let environment = settings(&facts).expect("settings");
        assert_eq!(
            environment.home_notice, None,
            "HOME is the home under this control"
        );
        assert_eq!(
            environment.home_line,
            format!("HOME /home/user (proof control {TEST_HOME_FROM_ENV})")
        );
    }

    #[test]
    fn a_config_file_pool_or_honoured_xdg_cache_root_is_not_noticed() {
        let dir = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("config-notice-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("scratch");
        let config = dir.join("config.toml");
        fs::write(&config, "[pool]\npath = \"~/file-pool\"\n").expect("config");
        let mut mac = facts(true);
        mac.home_source = HomeSource::Account;
        mac.env_home = Some(OsString::from("/private/tmp/harness-home"));
        mac.oxide_config = Some(config.into_os_string());
        let file = settings(&mac).expect("settings");
        assert_eq!(file.pool, Path::new("/home/user/file-pool"));
        assert_eq!(file.home_notice, None, "the config file chose the pool");

        let mut linux = facts(false);
        linux.home_source = HomeSource::Account;
        linux.env_home = Some(OsString::from("/private/tmp/harness-home"));
        assert!(settings(&linux).expect("settings").home_notice.is_some());
        environment(&mut linux, |environment| {
            environment.with_xdg_cache_home(Some(PathBuf::from("/srv/cache")))
        });
        let xdg = settings(&linux).expect("settings");
        assert_eq!(xdg.pool, Path::new("/srv/cache/oxide/build-pool"));
        assert_eq!(
            xdg.home_notice, None,
            "an honoured XDG cache root chose the pool"
        );
        environment(&mut linux, |environment| {
            environment.with_xdg_cache_home(Some(PathBuf::from("/home/user/.cache")))
        });
        let same = settings(&linux).expect("settings");
        for captured in [
            "/oxide-home-provenance-probe-a",
            "/oxide-home-provenance-probe-a/",
        ] {
            let mut probe_home = at_home(false, Some(captured));
            probe_home.home_source = HomeSource::Account;
            probe_home.env_home = Some(OsString::from("/private/tmp/harness-home"));
            assert!(
                settings(&probe_home)
                    .expect("settings")
                    .home_notice
                    .is_some(),
                "a home equivalent to the first probe ({captured}) still moves the root"
            );
        }
        assert_eq!(same.pool, Path::new("/home/user/.cache/oxide/build-pool"));
        assert_eq!(
            same.home_notice, None,
            "an XDG cache root naming the home default still chose the pool"
        );
    }

    #[test]
    fn home_messages_escape_control_characters_and_stay_bounded() {
        let mut facts = facts(true);
        facts.home_source = HomeSource::Account;
        facts.env_home = Some(OsString::from(format!(
            "/tmp/a\nb\u{1b}[31m{}",
            "x".repeat(1000)
        )));
        let line = facts.home_line();
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("a\\nb\\u{1b}[31m"), "{line}");
        assert!(
            line.ends_with('…') && line.chars().count() < 400,
            "{}",
            line.len()
        );
        let notice = settings(&facts)
            .expect("settings")
            .home_notice
            .expect("notice");
        assert!(!notice.chars().any(char::is_control) && notice.chars().count() < 1200);
    }

    #[test]
    fn an_unavailable_account_home_refuses_and_names_the_lookup() {
        let mut facts = at_home(true, None);
        facts.home_source = HomeSource::Unavailable(
            "the account home directory is unavailable (no passwd entry for uid 1)".to_owned(),
        );
        let error = facts.home().expect_err("no home");
        assert!(error.contains("no passwd entry"), "{error}");
        assert!(error.contains("OXIDE_POOL"), "{error}");
        assert_eq!(
            facts.home_line(),
            "unavailable (the account home directory is unavailable (no passwd entry for uid 1))"
        );
    }

    /// `home` feeds both `~` expansion and the injected directory snapshot.
    fn at_home(macos: bool, home: Option<&str>) -> Facts {
        Facts {
            macos,
            home: home.map(OsString::from),
            directories: Directories::Injected(
                PlatformEnvironment::default().with_home_dir(home.map(PathBuf::from)),
            ),
            physical_cores: PhysicalCoreCount::Unavailable("injected unsupported query".to_owned()),
            ..Facts::default()
        }
    }

    fn environment(
        facts: &mut Facts,
        change: impl FnOnce(PlatformEnvironment) -> PlatformEnvironment,
    ) {
        let Directories::Injected(environment) = &mut facts.directories else {
            panic!("tests inject their directory facts");
        };
        *environment = change(std::mem::take(environment));
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
        environment(&mut linux, |env| {
            env.with_xdg_config_home(Some("/xdg/config".into()))
        });
        assert_eq!(
            config_file(&linux).expect("XDG").path,
            Path::new("/xdg/config/oxide/config.toml")
        );
        // XDG: a relative value is ignored in favour of the default.
        environment(&mut linux, |env| {
            env.with_xdg_config_home(Some("relative".into()))
        });
        assert_eq!(
            config_file(&linux).expect("relative XDG").path,
            Path::new("/home/user/.config/oxide/config.toml")
        );
        let mut explicit = facts(true);
        explicit.oxide_config = Some("/elsewhere/oxide.toml".into());
        let chosen = config_file(&explicit).expect("explicit");
        assert_eq!(chosen.path, Path::new("/elsewhere/oxide.toml"));
        assert!(chosen.explicit);
        explicit.oxide_config = Some("oxide.toml".into());
        assert!(config_file(&explicit).is_err());
        let mut homeless = at_home(false, None);
        assert!(config_file(&homeless).is_err());
        homeless.oxide_config = Some("/explicit.toml".into());
        assert!(config_file(&homeless).is_ok());
    }

    /// Golden parity with the paths Oxide resolved before adopting
    /// `longhorn-storage-layout` (oxide#103). Literal on purpose.
    #[test]
    fn storage_roots_match_the_former_layout() {
        let home = "/Users/Zoë Ünïcode/with space";
        let mac = at_home(true, Some(home));
        assert_eq!(
            config_file(&mac).expect("config").path,
            Path::new(
                "/Users/Zoë Ünïcode/with space/Library/Application Support/oxide/config/config.toml"
            )
        );
        assert_eq!(
            default_pool(&mac).expect("pool"),
            Path::new("/Users/Zoë Ünïcode/with space/Library/Caches/oxide/build-pool")
        );
        assert_eq!(
            legacy_default_pool(&mac).expect("legacy"),
            Path::new("/Users/Zoë Ünïcode/with space/Library/Caches/oxide/pool")
        );
        assert_eq!(
            known_pools_file(&mac).expect("state"),
            Path::new(
                "/Users/Zoë Ünïcode/with space/Library/Application Support/oxide/state/pools.json"
            )
        );
        // XDG variables do not apply on macOS.
        let mut mac_xdg = mac.clone();
        environment(&mut mac_xdg, |env| {
            env.with_xdg_cache_home(Some("/xdg/cache".into()))
                .with_xdg_state_home(Some("/xdg/state".into()))
                .with_xdg_config_home(Some("/xdg/config".into()))
        });
        assert_eq!(default_pool(&mac_xdg), default_pool(&mac));
        assert_eq!(known_pools_file(&mac_xdg), known_pools_file(&mac));
        assert_eq!(
            config_file(&mac_xdg).expect("config"),
            config_file(&mac).expect("config")
        );

        // Linux defaults with no XDG_RUNTIME_DIR (never needed).
        let linux = at_home(false, Some(home));
        assert_eq!(
            config_file(&linux).expect("config").path,
            Path::new("/Users/Zoë Ünïcode/with space/.config/oxide/config.toml")
        );
        assert_eq!(
            default_pool(&linux).expect("pool"),
            Path::new("/Users/Zoë Ünïcode/with space/.cache/oxide/build-pool")
        );
        assert_eq!(
            known_pools_file(&linux).expect("state"),
            Path::new("/Users/Zoë Ünïcode/with space/.local/state/oxide/pools.json")
        );
        let mut xdg = linux.clone();
        environment(&mut xdg, |env| {
            env.with_xdg_cache_home(Some("/x dg/cåche".into()))
                .with_xdg_state_home(Some("/x dg/state".into()))
        });
        assert_eq!(
            default_pool(&xdg).expect("pool"),
            Path::new("/x dg/cåche/oxide/build-pool")
        );
        assert_eq!(
            legacy_default_pool(&xdg).expect("legacy"),
            Path::new("/x dg/cåche/oxide/pool")
        );
        assert_eq!(
            known_pools_file(&xdg).expect("state"),
            Path::new("/x dg/state/oxide/pools.json")
        );
        // A demanded relative XDG value falls back to the default.
        let mut relative = linux.clone();
        environment(&mut relative, |env| {
            env.with_xdg_cache_home(Some("cache".into()))
                .with_xdg_state_home(Some("state".into()))
        });
        assert_eq!(default_pool(&relative), default_pool(&linux));
        assert_eq!(known_pools_file(&relative), known_pools_file(&linux));
    }

    #[test]
    fn missing_facts_refuse_only_when_demanded() {
        // Absolute XDG values supply their own root without HOME.
        let mut linux = at_home(false, None);
        assert!(default_pool(&linux).is_none());
        assert!(known_pools_file(&linux).is_err());
        assert!(settings(&linux).is_err());
        environment(&mut linux, |env| {
            env.with_xdg_cache_home(Some("/xdg/cache".into()))
                .with_xdg_config_home(Some("/xdg/config".into()))
        });
        assert_eq!(
            default_pool(&linux).expect("pool"),
            Path::new("/xdg/cache/oxide/build-pool")
        );
        assert_eq!(
            settings(&linux).expect("XDG only").pool,
            Path::new("/xdg/cache/oxide/build-pool")
        );
        // Relative XDG and no HOME: the fact is missing.
        environment(&mut linux, |env| {
            env.with_xdg_cache_home(Some("cache".into()))
        });
        let refused = default_pool(&linux);
        assert!(refused.is_none());
        let error = settings(&linux).expect_err("no cache root");
        assert!(error.contains("HOME is unset"), "{error}");

        // Overrides without HOME: unused facts, relative or missing, never gate.
        for macos in [true, false] {
            let mut overridden = at_home(macos, None);
            environment(&mut overridden, |env| {
                env.with_xdg_config_home(Some("relative".into()))
                    .with_xdg_cache_home(Some("relative".into()))
                    .with_xdg_state_home(Some("relative".into()))
            });
            overridden.oxide_config = Some("/explicit/oxide.toml".into());
            overridden.oxide_pool = Some("/explicit/pool".into());
            let resolved = settings(&overridden).expect("overrides suffice");
            assert_eq!(resolved.pool, Path::new("/explicit/pool"));
            assert_eq!(resolved.pool_source, Source::Environment("OXIDE_POOL"));
            assert_eq!(resolved.file.path, Path::new("/explicit/oxide.toml"));
            assert!(known_pools(&overridden).is_empty());
        }

        // A relative HOME is not a home.
        let relative_home = at_home(true, Some("Users/someone"));
        assert!(config_file(&relative_home).is_err());
        assert!(default_pool(&relative_home).is_none());
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
        assert_eq!(settings.active_build_limit.value, 2);
        assert!(settings
            .active_build_limit
            .provenance(&settings.file.path)
            .contains("physical-core fallback"));
    }

    #[test]
    fn config_keys_types_and_units() {
        let parsed =
            parse_config("[pool]\npath = \"~/Dev/oxide\"\nbudget = \"100GB\"\nmax_active = 3\n")
                .expect("valid");
        let section = parsed.pool.expect("pool");
        assert_eq!(section.budget, Some(Budget::Text("100GB".to_owned())));
        assert_eq!(section.max_active, Some(3));
        assert_eq!(
            expand_path(section.path.as_deref().expect("path"), &facts(true)),
            Ok(PathBuf::from("/home/user/Dev/oxide"))
        );
        assert!(parse_config("[pool]\nbudget = 2000000\n").is_ok());
        assert!(parse_config("[pool]\nsize = \"1GB\"\n").is_err());
        assert!(parse_config("[other]\n").is_err());
        assert!(parse_config("[pool\n").is_err());
        assert!(parse_config("[pool]\npath = 3\n").is_err());
        assert!(parse_config("[pool]\nmax_active = \"3\"\n").is_err());
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
        fs::write(
            &file,
            "[pool]\npath = \"/from/file\"\nbudget = \"2GB\"\nmax_active = 3\n",
        )
        .expect("config");
        let mut chosen = facts(true);
        chosen.oxide_config = Some(file.clone().into());
        let from_file = settings(&chosen).expect("file");
        assert_eq!(from_file.pool, Path::new("/from/file"));
        assert_eq!(from_file.pool_source, Source::File);
        assert_eq!(from_file.budget, 2_000_000_000);
        assert_eq!(from_file.budget_source, Source::File);
        assert_eq!(from_file.active_build_limit.value, 3);
        assert_eq!(from_file.active_build_limit.source, Source::File);
        assert!(from_file
            .active_build_limit
            .provenance(&file)
            .contains("[pool].max_active"));
        chosen.oxide_pool = Some("/from/env".into());
        chosen.oxide_pool_cap = Some("3GB".into());
        chosen.oxide_max_active = Some("4".into());
        let from_env = settings(&chosen).expect("env");
        assert_eq!(from_env.pool, Path::new("/from/env"));
        assert_eq!(from_env.pool_source, Source::Environment("OXIDE_POOL"));
        assert_eq!(from_env.budget, 3_000_000_000);
        assert_eq!(from_env.active_build_limit.value, 4);
        assert_eq!(
            from_env.active_build_limit.source,
            Source::Environment("OXIDE_MAX_ACTIVE")
        );
        chosen.oxide_pool_cap = Some("lots".into());
        assert!(settings(&chosen).is_err());
        chosen.oxide_pool_cap = None;
        fs::write(&file, "[pool]\nmax_active = 0\n").expect("invalid lower-priority value");
        assert_eq!(
            settings(&chosen)
                .expect("environment wins")
                .active_build_limit
                .value,
            4
        );
        chosen.oxide_max_active = None;
        let refused = settings(&chosen).expect_err("invalid selected file value");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
        assert!(refused.contains("[pool] max_active"), "{refused}");
        for valid in [1, 32] {
            fs::write(&file, format!("[pool]\nmax_active = {valid}\n")).expect("config bound");
            assert_eq!(
                settings(&chosen)
                    .expect("config bound")
                    .active_build_limit
                    .value,
                valid
            );
        }
        for invalid in ["-1", "0", "33", "9223372036854775807"] {
            fs::write(&file, format!("[pool]\nmax_active = {invalid}\n"))
                .expect("bad config bound");
            let refused = settings(&chosen).expect_err(invalid);
            assert!(refused.contains(&file.display().to_string()), "{refused}");
            assert!(refused.contains("[pool] max_active"), "{refused}");
        }
        fs::write(&file, "[pool]\nmax_active = 9223372036854775808\n").expect("overflow config");
        let refused = settings(&chosen).expect_err("overflow config value");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
        fs::write(&file, "[pool]\nmax_active = 3\n").expect("valid file before environment checks");
        for invalid in ["0", "33", "-1", "1.5", "18446744073709551616"] {
            chosen.oxide_max_active = Some(invalid.into());
            let refused = settings(&chosen).expect_err(invalid);
            assert!(refused.contains("OXIDE_MAX_ACTIVE"), "{refused}");
        }
        for valid in ["1", "32"] {
            chosen.oxide_max_active = Some(valid.into());
            assert_eq!(
                settings(&chosen).expect(valid).active_build_limit.value,
                valid.parse::<usize>().unwrap()
            );
        }
        chosen.oxide_max_active = None;
        fs::write(&file, "[pool]\nmax_active = \"three\"\n").expect("wrong type");
        chosen.oxide_max_active = Some("4".into());
        let refused = settings(&chosen).expect_err("wrong-type lower-priority value");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
        chosen.oxide_max_active = None;
        fs::write(&file, "[pool]\nbudgte = \"2GB\"\n").expect("typo");
        chosen.oxide_pool_cap = None;
        let refused = settings(&chosen).expect_err("unknown key");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
        fs::write(&file, "not toml at all [").expect("malformed");
        let refused = settings(&chosen).expect_err("malformed");
        assert!(refused.contains(&file.display().to_string()), "{refused}");
    }

    #[test]
    fn physical_core_formula_and_fallback_are_explicit() {
        for (cores, expected) in [
            (1, 2),
            (7, 2),
            (8, 2),
            (11, 2),
            (12, 3),
            (15, 3),
            (16, 4),
            (1024, 4),
        ] {
            let mut input = facts(true);
            input.physical_cores = PhysicalCoreCount::Available(cores);
            let active = settings(&input)
                .expect("derived setting")
                .active_build_limit;
            assert_eq!(active.value, expected, "{cores} cores");
            assert_eq!(active.source, Source::Default);
            let provenance = active.provenance(Path::new("/config"));
            assert!(
                provenance.contains(&format!("hw.physicalcpu={cores}")),
                "{provenance}"
            );
            assert!(provenance.contains("floor("), "{provenance}");
        }

        for fact in [
            PhysicalCoreCount::Available(0),
            PhysicalCoreCount::Available(1025),
            PhysicalCoreCount::Available(usize::MAX),
            PhysicalCoreCount::Unavailable("sysctl name is missing".to_owned()),
            PhysicalCoreCount::Unavailable("injected query failure".to_owned()),
        ] {
            let mut input = facts(true);
            input.physical_cores = fact;
            let active = settings(&input)
                .expect("fallback setting")
                .active_build_limit;
            assert_eq!(active.value, FALLBACK_MAX_ACTIVE);
            assert!(active
                .provenance(Path::new("/config"))
                .contains("physical-core fallback"));
        }

        let mut other_platform = facts(false);
        other_platform.physical_cores = PhysicalCoreCount::Available(64);
        let active = settings(&other_platform)
            .expect("other platform fallback")
            .active_build_limit;
        assert_eq!(active.value, FALLBACK_MAX_ACTIVE);
        assert!(active
            .provenance(Path::new("/config"))
            .contains("unsupported on this platform"));
    }

    #[test]
    fn macos_ignores_the_dot_config_location() {
        let scratch = Scratch::new("ignored");
        let legacy = scratch.0.join(".config/oxide");
        fs::create_dir_all(&legacy).expect("legacy dir");
        fs::write(legacy.join("config.toml"), "[pool]\npath = \"/never\"\n").expect("legacy");
        let mac = at_home(true, scratch.0.to_str());
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
    fn a_denied_ancestor_refuses_instead_of_reading_as_missing() {
        use std::os::unix::fs::PermissionsExt;
        let _internal = internal_fixture_scope();
        let scratch = Scratch::new("denied");
        let locked = scratch.0.join("locked");
        fs::create_dir_all(&locked).expect("locked");
        // Without search permission the child cannot be inspected, which is not
        // the same as the child being absent.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("deny");
        let result = judge_pool_path(&locked.join("pool"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("restore");
        assert!(
            matches!(&result, Err(PathRefusal::Ancestor(reason)) if reason.contains("cannot inspect")),
            "{result:?}"
        );
    }

    #[test]
    fn git_work_trees_and_aliases_refuse() {
        // Model an ordinary filesystem while all real paths stay in task TMPDIR.
        let internal = internal_fixture_scope();
        let scratch = Scratch::new("git");
        let repo = scratch.0.join("repo");
        fs::create_dir_all(repo.join(".git")).expect("repo");
        let pool = repo.join("pool");
        let git_message = format!(
            "the pool path {} is inside the Git work tree {}; choose a path outside any repository",
            pool.display(),
            fs::canonicalize(&repo).unwrap().display()
        );
        assert_eq!(refuse_unsafe_pool_path(&pool), Err(git_message.clone()));
        assert_eq!(
            judge_pool_path(&pool),
            Err(PathRefusal::GitWorkTree(git_message))
        );
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
        drop(internal);
        // Pure volume-prefix facts exercise the external refusal without
        // touching a live mount or requiring an internal-volume fixture.
        assert!(refuse_external_prefix(
            Path::new("/Volumes/unit/pool"),
            Path::new("/Volumes/unit")
        )
        .is_err());
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
    fn an_already_excluded_pool_never_pays_for_addexclusion() {
        if !cfg!(target_os = "macos") {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("backup-skip");
        let invocations = scratch.0.join("invocations");
        let excluded_marker = scratch.0.join("excluded");
        let program = scratch.0.join("tmutil");
        // A fake tmutil: `addexclusion` creates the marker, and `isexcluded`
        // reports the marker's presence. Every invocation is recorded, so the
        // test can assert `addexclusion` was never run.
        fs::write(
            &program,
            format!(
                "#!/bin/sh\necho \"$1\" >> '{log}'\n\
                 case \"$1\" in\n\
                 addexclusion) : > '{marker}'; exit 0;;\n\
                 isexcluded) if [ -e '{marker}' ]; then echo \"[Excluded]  $2\"; else echo \"[Included]  $2\"; fi; exit 0;;\n\
                 *) exit 2;;\n\
                 esac\n",
                log = invocations.display(),
                marker = excluded_marker.display()
            ),
        )
        .expect("fake tmutil");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("mode");
        let tmutil = Tmutil { program };
        let pool = scratch.0.join("pool");
        fs::create_dir(&pool).expect("pool");

        // Inherited exclusion already holds: one cheap check, no add.
        fs::write(&excluded_marker, []).expect("marker");
        assert_eq!(exclude_with(&tmutil, &pool), Backup::Excluded);
        assert_eq!(
            fs::read_to_string(&invocations).expect("log"),
            "isexcluded\n"
        );

        // A genuinely included root pays for the add, then verifies it.
        fs::remove_file(&excluded_marker).expect("marker");
        fs::write(&invocations, []).expect("log");
        assert_eq!(exclude_with(&tmutil, &pool), Backup::Excluded);
        assert_eq!(
            fs::read_to_string(&invocations).expect("log"),
            "isexcluded\naddexclusion\nisexcluded\n"
        );
    }

    #[test]
    fn known_pools_are_bounded_and_deduplicated() {
        let scratch = Scratch::new("known");
        let mut linux = facts(false);
        environment(&mut linux, |env| {
            env.with_xdg_state_home(Some(scratch.0.clone()))
        });
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

    /// Regression: the record was written with `fs::write`, which follows a
    /// planted link and uses the umask mode. It is now created exclusively,
    /// owner-only, and leaves no temporary behind.
    #[test]
    fn remember_pool_writes_an_owner_only_record_and_no_temporary() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("remember");
        let mut linux = facts(false);
        environment(&mut linux, |env| {
            env.with_xdg_state_home(Some(scratch.0.clone()))
        });
        remember_pool(&linux, Path::new("/pools/owner")).expect("remember");
        let record = known_pools_file(&linux).expect("record path");
        let mode = fs::metadata(&record).expect("record").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let leftovers: Vec<_> = fs::read_dir(record.parent().expect("parent"))
            .expect("state dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "a temporary was left behind");
    }

    /// Regression: a config that grew past the cap after its size was read was
    /// truncated and parsed silently. It is now refused, and a config exactly at
    /// the cap is still read.
    #[test]
    fn a_config_that_grows_past_the_cap_is_refused_not_truncated() {
        let named = |error: String| format!("config: {error}");
        let at_cap = vec![b'#'; MAX_CONFIG_BYTES as usize];
        assert_eq!(
            read_bounded_config(&at_cap[..], named)
                .expect("a config at the cap")
                .len(),
            at_cap.len()
        );
        let grown = vec![b'#'; MAX_CONFIG_BYTES as usize + 1];
        let error = read_bounded_config(&grown[..], named).expect_err("refused");
        assert!(error.contains("grew past"), "{error}");
    }
}

#[cfg(test)]
thread_local! {
    static INTERNAL_FIXTURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(test)]
pub(crate) fn internal_fixture() -> bool {
    INTERNAL_FIXTURE.with(std::cell::Cell::get)
}
#[cfg(test)]
pub(crate) struct InternalFixture(bool);
#[cfg(test)]
impl Drop for InternalFixture {
    fn drop(&mut self) {
        INTERNAL_FIXTURE.with(|slot| slot.set(self.0));
    }
}
/// Model only boot-volume classification; file operations stay in task TMPDIR.
#[cfg(test)]
pub(crate) fn internal_fixture_scope() -> InternalFixture {
    InternalFixture(INTERNAL_FIXTURE.with(|slot| slot.replace(true)))
}
