//! The donor catalog: strict schema, bounded caps and its owner-only sidecar.
//!
//! A catalog is written by the producing lease while it is still held
//! (`oxide#137` §4.4, §9.1) and read by a later command's read-only candidate
//! call. Every fact here is recorded data: the library never derives a unit
//! identity, a join or a digest of its own, it only checks that the bytes it
//! reads are the bytes those recorded digests describe.
//!
//! The catalog is a sidecar, never control state: nothing in the pool config,
//! ledger, registration, receipt, origin or marker format changes with it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Sidecar schema. An unknown value is ignored by a reader, never repaired.
pub const SEED_CATALOG_SCHEMA: &str = "oxide.cold-seed.catalog/1";
/// One catalog's own byte ceiling. A catalog that would exceed it is not
/// written, and a larger file on disk is ignored.
pub const SEED_CATALOG_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Packages per catalog.
pub const SEED_MAX_PACKAGES: usize = 2048;
/// File entries per catalog (all packets, all units together).
pub const SEED_MAX_FILES: usize = 8192;
/// Units per package: library, proc macro and build-script compile.
pub const SEED_MAX_UNITS_PER_PACKAGE: usize = 3;
/// Rejected-candidate entries per catalog.
pub const SEED_MAX_REJECTIONS: usize = 256;
/// Units the rejection record may count.
pub const SEED_MAX_REJECTED_UNITS: u64 = 4096;
/// A rejected name's byte ceiling.
pub const SEED_MAX_REJECTED_NAME_BYTES: usize = 64;
/// Producer identity, context and name strings.
pub const SEED_MAX_TEXT_BYTES: usize = 512;
/// One relative or absolute path string.
pub const SEED_MAX_PATH_BYTES: usize = 1024;
/// Config files in the producer context, in precedence order.
pub const SEED_MAX_CONFIG_FILES: usize = 16;
/// Rule targets (rules) recorded for one unit's `.d` (§8.1).
pub const SEED_MAX_RULES: usize = 4096;
/// Rule inputs recorded for one unit's `.d` (§8.1: 4,096 per unit).
pub const SEED_MAX_UNIT_INPUTS: usize = 4096;
/// Rule inputs across one plan (§8.1: 32,768 total).
pub const SEED_MAX_PLAN_INPUTS: usize = 32768;
/// Files recorded for one unit: the fingerprint triple, the artifacts and
/// their `.d`, with bounded headroom for role variants.
pub const SEED_MAX_UNIT_FILES: usize = 16;
/// One relative path or name component.
pub const SEED_MAX_NAME_BYTES: usize = 255;

/// A donor catalog for one pool slot generation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedCatalog {
    pub schema: String,
    pub store_id: String,
    pub slot_id: String,
    pub incarnation: String,
    /// The producing lease. Eligibility ends the moment the ledger records a
    /// different `last_lease_id` for the slot.
    pub lease_id: String,
    pub record_id: u64,
    pub mark: u64,
    pub started_at_ms: u64,
    pub finished_at_ms: u64,
    /// The producing window's terminal seed-attempt state. A generation that
    /// seeded anything never publishes a catalog (§4.1).
    pub attempt: SeedAttemptState,
    pub producer: SeedProducer,
    pub packages: Vec<SeedPackage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<SeedRejection>,
}

/// How a producing window's own seed attempt ended. Only `None` may publish.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedAttemptState {
    None,
    Intent,
    Seeded,
    Skipped,
    Reset,
}

/// Identity and context of the command that filled this slot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedProducer {
    pub cargo_version: String,
    pub rustc_digest: String,
    pub rustc: String,
    pub target: String,
    pub recipe: String,
    pub backend: String,
    pub adapter: String,
    /// The §3.1 context digest and the config files it covered.
    pub context_digest: String,
    pub config_files: Vec<String>,
    pub flags_digest: String,
    pub cargo_home: String,
    pub command_family: String,
    pub command_class: String,
}

/// One offered package, with the evidence selection needs and no donor path
/// outside the slot's `build/` tree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedPackage {
    /// The lock's literal package name: the fingerprint directory prefix.
    pub name: String,
    /// The normalized target name: unit suffix and artifact stem.
    pub target: String,
    pub version: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    /// The donor lock's uniqueness after normalization (§4.2).
    pub unique: bool,
    /// The §3.2 leaf boundary: the lock entry has no dependency edge.
    pub leaf: bool,
    pub source_root: SeedSourceRoot,
    pub units: Vec<SeedUnit>,
}

/// What the producing epoch verified about the package's source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedSourceRoot {
    pub kind: SeedSourceKind,
    /// The canonical source tree digest; opaque recorded evidence here.
    pub digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSourceKind {
    Registry,
    Git,
}

/// One complete dependency unit, joined at producing time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedUnit {
    /// Relative to the slot's `build/`, e.g.
    /// `debug/.fingerprint/cfg-if-<hex>`.
    pub fingerprint_dir: String,
    pub role: SeedRole,
    pub target: String,
    pub join: SeedJoin,
    /// The exact file list: the fingerprint triple, the role's artifacts and
    /// the joining `.d`.
    pub files: Vec<SeedFile>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeedRole {
    Lib,
    ProcMacro,
    BuildScriptCompile,
}

impl SeedRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::ProcMacro => "proc-macro",
            Self::BuildScriptCompile => "build-script-compile",
        }
    }
}

/// The recorded join of a unit's `.d`. Its own SHA-256 binds these rules to
/// exactly the bytes the producing epoch hashed, so a later digest match is
/// the join re-check: no second makefile grammar exists in the library.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedJoin {
    /// Relative to the slot's `build/`.
    pub path: String,
    /// Output rules (the `.d` itself and the role artifacts).
    pub targets: Vec<String>,
    /// Prerequisite paths of the accepted rules, as rustc recorded them.
    pub inputs: Vec<String>,
    /// SHA-256 of the `.d` file itself.
    pub digest: String,
}

/// One file offered from the donor's closed producing epoch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedFile {
    /// Relative to the slot's `build/`.
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// SHA-256 of the file's content.
    pub digest: String,
    /// What the producing epoch classified this file as. An **artifact** is a
    /// rustc output the unit's `.d` must join with its own single-target rule
    /// carrying nonempty source prerequisites; a fingerprint file, the
    /// unhashed build-script uplift and the `.d` itself are not artifacts.
    pub kind: SeedFileKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedFileKind {
    /// One of Cargo's fingerprint triple: unit state, never a rustc output.
    Fingerprint,
    /// A rustc output the unit's `.d` must join.
    Artifact,
    /// Cargo's unhashed build-script uplift: a copy of the hashed output, not
    /// a `.d` output rule.
    Uplift,
    /// The unit's own dep-info file.
    DepInfo,
}

/// Why a lock entry was not offered. The counts feed the bounded skip
/// buckets of §9.2; they are donor-side unit counts, never package counts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeedRejection {
    /// The normalized lock name, bounded and escaped.
    pub name: String,
    pub reason: SeedRejectReason,
    /// `None` means the producing epoch could not classify the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedRejectReason {
    AmbiguousName,
    NonLeaf,
    JoinUnprovable,
    UnverifiedSource,
    UnsupportedSource,
    RunOnly,
    Cap,
}

impl SeedCatalog {
    /// Parse one bounded catalog. Any schema, cap or grammar violation makes
    /// the whole file ineligible; a reader never repairs one.
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > SEED_CATALOG_MAX_BYTES {
            return Err("cold-seed catalog exceeds its byte ceiling");
        }
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(|_| "cold-seed catalog is malformed")?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Serialize and check the result still fits its ceiling.
    pub fn to_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.validate()?;
        let bytes =
            serde_json::to_vec(self).map_err(|_| "cold-seed catalog is not serializable")?;
        if bytes.len() > SEED_CATALOG_MAX_BYTES {
            return Err("cold-seed catalog exceeds its byte ceiling");
        }
        Ok(bytes)
    }

    /// Structural and cap validation. It checks the recorded shape only; it
    /// establishes nothing about the donor's files, which donation re-hashes.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema != SEED_CATALOG_SCHEMA {
            return Err("cold-seed catalog schema is unknown");
        }
        require_bounded(&self.store_id, 64, "store id")?;
        if !self.slot_id.bytes().all(|byte| byte.is_ascii_digit())
            || !matches!(self.slot_id.parse::<u64>(), Ok(id) if id != 0)
        {
            return Err("cold-seed catalog slot id is not a ledger slot id");
        }
        require_bounded(&self.incarnation, 64, "incarnation")?;
        require_bounded(&self.lease_id, 160, "lease id")?;
        if self.finished_at_ms < self.started_at_ms {
            return Err("cold-seed catalog finishes before it started");
        }
        self.producer.validate()?;
        if self.packages.len() > SEED_MAX_PACKAGES {
            return Err("cold-seed catalog has too many packages");
        }
        let mut files = 0usize;
        let mut targets = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        for package in &self.packages {
            package.validate()?;
            if !targets.insert(package.target.as_str()) {
                return Err("cold-seed catalog repeats a normalized package name");
            }
            for unit in &package.units {
                for file in &unit.files {
                    if !paths.insert(file.path.as_str()) {
                        return Err("cold-seed catalog repeats a file path");
                    }
                    files += 1;
                }
            }
        }
        if files > SEED_MAX_FILES {
            return Err("cold-seed catalog has too many file entries");
        }
        if self.rejected.len() > SEED_MAX_REJECTIONS {
            return Err("cold-seed catalog rejection record is too long");
        }
        let mut units = 0u64;
        for rejection in &self.rejected {
            require_bounded(
                &rejection.name,
                SEED_MAX_REJECTED_NAME_BYTES,
                "rejected name",
            )?;
            if let Some(count) = rejection.units {
                units = units.saturating_add(u64::from(count));
            }
        }
        if units > SEED_MAX_REJECTED_UNITS {
            return Err("cold-seed catalog counts too many rejected units");
        }
        Ok(())
    }

    /// Bytes one unit of this catalog occupies, for the plan's byte cap.
    pub fn unit_bytes(unit: &SeedUnit) -> u64 {
        unit.files.iter().map(|file| file.size).sum()
    }
}

impl SeedProducer {
    fn validate(&self) -> Result<(), &'static str> {
        for (value, field) in [
            (&self.cargo_version, "cargo version"),
            (&self.rustc, "rustc"),
            (&self.target, "target"),
            (&self.recipe, "recipe"),
            (&self.backend, "backend"),
            (&self.adapter, "adapter"),
            (&self.cargo_home, "cargo home"),
            (&self.command_family, "command family"),
            (&self.command_class, "command class"),
        ] {
            require_bounded(value, SEED_MAX_TEXT_BYTES, field)?;
        }
        require_digest(&self.rustc_digest)?;
        require_digest(&self.context_digest)?;
        require_digest(&self.flags_digest)?;
        if self.config_files.len() > SEED_MAX_CONFIG_FILES {
            return Err("cold-seed producer context has too many config files");
        }
        for path in &self.config_files {
            require_path(path, true)?;
        }
        Ok(())
    }
}

impl SeedPackage {
    fn validate(&self) -> Result<(), &'static str> {
        require_bounded(&self.name, SEED_MAX_REJECTED_NAME_BYTES, "package name")?;
        require_bounded(&self.target, SEED_MAX_REJECTED_NAME_BYTES, "target name")?;
        require_bounded(&self.version, SEED_MAX_TEXT_BYTES, "package version")?;
        require_bounded(&self.source, SEED_MAX_TEXT_BYTES, "package source")?;
        if let Some(checksum) = &self.checksum {
            require_bounded(checksum, SEED_MAX_TEXT_BYTES, "package checksum")?;
        }
        require_digest(&self.source_root.digest)?;
        if self.units.len() > SEED_MAX_UNITS_PER_PACKAGE {
            return Err("cold-seed catalog package has too many units");
        }
        for unit in &self.units {
            unit.validate()?;
        }
        Ok(())
    }
}

impl SeedUnit {
    fn validate(&self) -> Result<(), &'static str> {
        require_path(&self.fingerprint_dir, false)?;
        require_bounded(&self.target, SEED_MAX_REJECTED_NAME_BYTES, "unit target")?;
        self.join.validate()?;
        if self.files.is_empty() || self.files.len() > SEED_MAX_UNIT_FILES {
            return Err("cold-seed unit has an unusable file list");
        }
        // The profile component of the fingerprint directory is the unit's
        // output subtree: every offered file lives below it, so one unit can
        // never name a file of another profile or of the slot root.
        let profile = self
            .fingerprint_dir
            .split_once('/')
            .map(|(profile, _)| profile)
            .ok_or("cold-seed unit has no profile directory")?;
        // The joining `.d` is part of the unit's own file list and is the
        // unit's one dep-info record: a catalog that names a join it would not
        // clone is not a complete unit, and the join's own digest is that file
        // record's digest so the recorded rules and the bytes they describe
        // cannot be published apart.
        let dep_info: Vec<&SeedFile> = self
            .files
            .iter()
            .filter(|file| file.kind == SeedFileKind::DepInfo)
            .collect();
        if dep_info.len() != 1 || dep_info[0].path != self.join.path {
            return Err("cold-seed unit's join is not its one dep-info file");
        }
        if dep_info[0].digest != self.join.digest {
            return Err("cold-seed unit's join digest is not its .d file digest");
        }
        // A unit without a rustc artifact is not a unit this layer offers, and
        // the classification must match the layout so a producer cannot opt an
        // artifact out by calling it a fingerprint file or an uplift: an
        // artifact is never a fingerprint-directory entry, and a fingerprint
        // file always is one.
        let fingerprint_prefix = format!("{}/", self.fingerprint_dir);
        if !self
            .files
            .iter()
            .any(|file| file.kind == SeedFileKind::Artifact)
        {
            return Err("cold-seed unit offers no artifact");
        }
        for file in &self.files {
            match file.kind {
                SeedFileKind::Fingerprint => {
                    if !file.path.starts_with(&fingerprint_prefix) {
                        return Err("cold-seed fingerprint file is outside its unit directory");
                    }
                }
                SeedFileKind::Artifact | SeedFileKind::DepInfo => {
                    if file.path.starts_with(&fingerprint_prefix) {
                        return Err("cold-seed artifact is a fingerprint-directory entry");
                    }
                }
                SeedFileKind::Uplift => {
                    let build_prefix = format!("{}/build/", profile);
                    if self.role != SeedRole::BuildScriptCompile
                        || !file.path.starts_with(&build_prefix)
                        || file.path.starts_with(&fingerprint_prefix)
                    {
                        return Err("cold-seed uplift is not a build-script uplift");
                    }
                }
            }
        }
        for file in &self.files {
            file.validate()?;
            let file_profile = file
                .path
                .split_once('/')
                .map(|(profile, _)| profile)
                .ok_or("cold-seed file is outside the unit's profile directory")?;
            if file_profile != profile {
                return Err("cold-seed file is outside the unit's profile directory");
            }
        }
        Ok(())
    }
}

impl SeedJoin {
    fn validate(&self) -> Result<(), &'static str> {
        require_path(&self.path, false)?;
        require_digest(&self.digest)?;
        if self.targets.is_empty() || self.targets.len() > SEED_MAX_RULES {
            return Err("cold-seed join has an unusable rule set");
        }
        // Rule targets are the `.d`'s output rules and rustc's phony source
        // rules, so each is either slot-relative or an absolute source path;
        // prerequisites are the verified source paths.
        for target in &self.targets {
            require_recorded_path(target)?;
        }
        if self.inputs.len() > SEED_MAX_UNIT_INPUTS {
            return Err("cold-seed join has too many inputs for one unit");
        }
        for input in &self.inputs {
            require_recorded_path(input)?;
        }
        Ok(())
    }
}

impl SeedFile {
    fn validate(&self) -> Result<(), &'static str> {
        require_path(&self.path, false)?;
        require_digest(&self.digest)?;
        Ok(())
    }
}

/// One path string: bounded, no NUL, and either a strict relative name (no
/// empty, `.` or `..` component, no symlink-shaped Finder metadata) or a
/// strict absolute path when `absolute` is set.
fn require_path(text: &str, absolute: bool) -> Result<(), &'static str> {
    if text.is_empty() || text.len() > SEED_MAX_PATH_BYTES || text.bytes().any(|byte| byte == 0) {
        return Err("cold-seed path is empty, over-long or holds a NUL");
    }
    if absolute {
        return if text.starts_with('/') {
            Ok(())
        } else {
            Err("cold-seed path is not absolute")
        };
    }
    if text.starts_with('/') {
        return Err("cold-seed relative path is absolute");
    }
    for component in text.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > SEED_MAX_NAME_BYTES
        {
            return Err("cold-seed relative path has an unusable component");
        }
        if component == ".DS_Store" || component.starts_with("._") {
            return Err("cold-seed relative path names Finder metadata");
        }
    }
    Ok(())
}

/// A recorded join path: an absolute path or a strict relative one.
fn require_recorded_path(text: &str) -> Result<(), &'static str> {
    require_path(text, text.starts_with('/'))
}

fn require_bounded(text: &str, max: usize, field: &'static str) -> Result<(), &'static str> {
    if text.is_empty() || text.len() > max || !text.is_ascii() {
        let _ = field;
        return Err("cold-seed catalog field is empty, over-long or not ASCII");
    }
    Ok(())
}

fn require_digest(text: &str) -> Result<(), &'static str> {
    if text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err("cold-seed digest is not a lowercase SHA-256")
}

/// The SHA-256 of a catalog's exact bytes, in lowercase hex. Call 1 returns
/// it and call 2 requires the same bytes back.
pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}
