//! Private, bounded acquisition of pinned Cargo registry correspondence.
//!
//! This entry only locates exact lock-derived paths below the observed Cargo
//! registry roots and delegates all archive/extraction checks to `archive`.
//! It does not install evidence authority or establish later immutability.

use super::archive::{self, Counters, Request};
use super::Limits;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const CARGO_HOME: &str = "/Users/tom/.cargo";
const SOURCE_COMMIT: &str = "576ba7904d58e4bf829ddddda89a06833b8c5087";
const SOURCE_TREE: &str = "0aa379b840c2f8990adeecc1fc49d9e57ab5869b";
const LOCK_SHA256: &str = "ab2ed097a6d9cc740cc738b32d1c361d99214b16f0dbdeacfeb061bc10d1d57d";
const LOCKED_INVENTORY_SHA256: &str =
    "d34be11369c2dc97d43afca0a4211a287f5e841048187c4a386d0f19f2a29cc6";
const SOURCE_LEAVES: [&str; 5] = [
    "Cargo.lock",
    "Cargo.toml",
    "crates/nucleus-core/Cargo.toml",
    "crates/nucleus-observer/Cargo.toml",
    "effigy.toml",
];
const MAX_PACKAGES: usize = 128;
const MAX_NAMESPACES: usize = 8;
const MAX_CANDIDATES: usize = 512;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

/// Pinned per-leaf SHA-256 values for the five allowlisted source leaves. The
/// streaming caller binds a request to these without re-reading Git; the legacy
/// caller still hashes them from the pinned object.
const PINNED_SOURCE_LEAVES: [(&str, &str); 5] = [
    ("Cargo.lock", LOCK_SHA256),
    (
        "Cargo.toml",
        "81f90a4e389cef850a26446900173d3bbe70ae6a31371abe5aae650a99ecca5a",
    ),
    (
        "crates/nucleus-core/Cargo.toml",
        "3d0e9fd5ead4e189ccb8d9dfd3e87254b46a84f8ca3e741d38e045a0da6c2e48",
    ),
    (
        "crates/nucleus-observer/Cargo.toml",
        "3bc428a6b5c2ede9f526e178f90d71da4f7a38222160514e7f6509ff8fa552a9",
    ),
    (
        "effigy.toml",
        "f666615645cb59f24fea78fbc7c1627e7d76cb647355dd791f1c57a2207ab8c2",
    ),
];

/// The exact two registry tuples the streaming caller is allowed to observe.
const STREAMING_PACKAGES: [(&str, &str, &str); 2] = [
    (
        "libsqlite3-sys",
        "0.38.2",
        "f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8",
    ),
    (
        "sqlite-wasm-rs",
        "0.5.5",
        "dc3efc0da82635d7e1ced0053bbbfa8c7ab9645d0bf36ceb4f7127bb85315d75",
    ),
];

/// Selects the legacy capped verifier or the private streaming verifier. The
/// route is fixed by the request schema and can never be raised by package or
/// counter JSON.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Route {
    Legacy,
    Streaming,
}
impl Route {
    fn schema(self) -> &'static str {
        match self {
            Route::Legacy => "oxide.registry-acquisition.v2",
            Route::Streaming => "oxide.registry-acquisition.v3",
        }
    }
    fn mode(self) -> Option<&'static str> {
        match self {
            Route::Legacy => None,
            Route::Streaming => Some("streaming"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceIdentity {
    pub commit: String,
    pub tree: String,
    pub lock_sha256: String,
    pub leaves: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseStart {
    pub counters: CounterSnapshot,
    pub candidate_observations: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AcquisitionRequest {
    pub schema: String,
    pub source: SourceIdentity,
    pub phase_start: PhaseStart,
    pub packages: Vec<LockedPackage>,
}

/// Schema-v3 streaming request. The `mode` field and the exact two-package set
/// distinguish it from the unchanged schema-v2 legacy request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamingAcquisitionRequest {
    pub schema: String,
    pub mode: String,
    pub source: SourceIdentity,
    pub phase_start: PhaseStart,
    pub packages: Vec<LockedPackage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Metadata {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
    pub changed_seconds: i64,
    pub changed_nanoseconds: i64,
    pub links: u64,
}

impl Metadata {
    fn read(path: &Path) -> io::Result<Self> {
        let m = fs::symlink_metadata(path)?;
        Ok(Self {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
            size: m.size(),
            modified_seconds: m.mtime(),
            modified_nanoseconds: m.mtime_nsec(),
            changed_seconds: m.ctime(),
            changed_nanoseconds: m.ctime_nsec(),
            links: m.nlink(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Namespace {
    pub name: String,
    pub path: String,
    pub state: String,
    pub metadata_before: Option<Metadata>,
    pub metadata_after: Option<Metadata>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RootSnapshot {
    pub path: String,
    pub metadata_before: Metadata,
    pub metadata_after: Option<Metadata>,
    pub stable: bool,
    pub namespace_limit_reached: bool,
    pub namespaces: Vec<Namespace>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Candidate {
    pub kind: String,
    pub namespace: String,
    pub path: String,
    pub state: String,
    pub metadata: Option<Metadata>,
    pub canonical: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CounterSnapshot {
    pub archive_bytes: usize,
    pub decompressed_bytes: usize,
    pub extracted_bytes: usize,
    pub entries: usize,
}

impl From<Counters> for CounterSnapshot {
    fn from(c: Counters) -> Self {
        Self {
            archive_bytes: c.archive_bytes,
            decompressed_bytes: c.decompressed_bytes,
            extracted_bytes: c.extracted_bytes,
            entries: c.entries,
        }
    }
}

impl From<CounterSnapshot> for Counters {
    fn from(c: CounterSnapshot) -> Self {
        Self {
            archive_bytes: c.archive_bytes,
            decompressed_bytes: c.decompressed_bytes,
            extracted_bytes: c.extracted_bytes,
            entries: c.entries,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackageOutcome {
    pub package: LockedPackage,
    pub candidates: Vec<Candidate>,
    pub archive: Option<String>,
    pub extracted_root: Option<String>,
    pub metadata_before: Option<(Metadata, Metadata)>,
    pub metadata_after: Option<(Metadata, Metadata)>,
    pub status: String,
    pub refusal: Option<String>,
    pub files: BTreeMap<String, String>,
    pub directories: BTreeSet<String>,
    pub counters_after: CounterSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AcquisitionReport {
    pub schema: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    pub source: SourceIdentity,
    pub phase_start: PhaseStart,
    pub cache: RootSnapshot,
    pub extracted: RootSnapshot,
    pub candidate_observations: usize,
    pub acquisition_candidate_observations: usize,
    pub packages: Vec<PackageOutcome>,
    pub counters: CounterSnapshot,
}

struct Root {
    canonical: PathBuf,
    before: Metadata,
    namespace_limit_reached: bool,
    namespaces: Vec<(String, PathBuf, String, Option<Metadata>)>,
}

/// Reads one schema-v2 legacy request and emits a capped JSON report.
pub fn run<R: Read, W: Write>(input: R, output: W) -> Result<(), String> {
    let bytes = read_request(input)?;
    let request: AcquisitionRequest =
        serde_json::from_slice(&bytes).map_err(|e| format!("request JSON: {e}"))?;
    emit(acquire(request)?, "oxide.registry-acquisition.v2", output)
}

/// Reads one schema-v3 streaming request and emits a capped JSON report. The
/// route is opt-in and cannot be selected by a legacy request.
pub fn run_streaming<R: Read, W: Write>(input: R, output: W) -> Result<(), String> {
    let bytes = read_request(input)?;
    let request: StreamingAcquisitionRequest =
        serde_json::from_slice(&bytes).map_err(|e| format!("request JSON: {e}"))?;
    emit(
        acquire_streaming(request)?,
        "oxide.registry-acquisition.v3",
        output,
    )
}

fn read_request<R: Read>(input: R) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    input
        .take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("request read: {e}"))?;
    if bytes.len() > MAX_OUTPUT {
        return Err("request exceeds the 8 MiB output/input ceiling".into());
    }
    Ok(bytes)
}

fn emit<W: Write>(report: AcquisitionReport, schema: &str, mut output: W) -> Result<(), String> {
    let mut capped = Capped(Vec::new());
    if serde_json::to_writer(&mut capped, &report).is_err() {
        let small = serde_json::json!({
            "schema": schema,
            "status": "refused",
            "refusal": "retained_output_exceeds_8_mib"
        });
        serde_json::to_writer(&mut output, &small).map_err(|e| e.to_string())?;
        output.write_all(b"\n").map_err(|e| e.to_string())?;
        return Ok(());
    }
    output.write_all(&capped.0).map_err(|e| e.to_string())?;
    output.write_all(b"\n").map_err(|e| e.to_string())?;
    Ok(())
}

struct Capped(Vec<u8>);
impl Write for Capped {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_OUTPUT {
            return Err(io::Error::other("output ceiling"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn acquire(request: AcquisitionRequest) -> Result<AcquisitionReport, String> {
    if request.schema != "oxide.registry-acquisition.request.v2"
        || request.source.commit != SOURCE_COMMIT
        || request.source.tree != SOURCE_TREE
        || request.source.lock_sha256 != LOCK_SHA256
        || request.packages.is_empty()
        || request.packages.len() > MAX_PACKAGES
    {
        return Err("pinned request identity or package bound refused".into());
    }
    let starting = request.phase_start.counters;
    let starting_total = starting
        .archive_bytes
        .saturating_add(starting.decompressed_bytes)
        .saturating_add(starting.extracted_bytes);
    if starting_total > Limits::NUCLEUS.collection_bytes
        || starting.entries > Limits::NUCLEUS.associations
        || request.phase_start.candidate_observations > MAX_CANDIDATES
    {
        return Err("phase starting counters exceed the acquisition bounds".into());
    }
    if request.source.leaves.len() != SOURCE_LEAVES.len()
        || SOURCE_LEAVES.iter().any(|leaf| {
            request
                .source
                .leaves
                .get(*leaf)
                .is_none_or(|digest| !valid_hex_digest(digest))
        })
    {
        return Err("pinned source leaf set refused".into());
    }
    if request.source.leaves.get("Cargo.lock").map(String::as_str) != Some(LOCK_SHA256) {
        return Err("pinned Cargo.lock leaf digest refused".into());
    }
    let mut package_ids = BTreeSet::new();
    if request.packages.iter().any(|package| {
        !safe_package(package)
            || !package_ids.insert((package.name.clone(), package.version.clone()))
    }) {
        return Err("locked registry package set refused".into());
    }
    let mut inventory = request.packages.clone();
    inventory
        .sort_by(|left, right| (&left.name, &left.version).cmp(&(&right.name, &right.version)));
    let encoded = inventory
        .iter()
        .map(|package| {
            format!(
                "{}\t{}\t{}\t{}\n",
                package.name, package.version, package.source, package.checksum
            )
        })
        .collect::<String>();
    if format!(
        "{:x}",
        <sha2::Sha256 as sha2::Digest>::digest(encoded.as_bytes())
    ) != LOCKED_INVENTORY_SHA256
    {
        return Err("locked package tuple inventory digest refused".into());
    }
    if std::env::var_os("CARGO_HOME").as_deref() != Some(std::ffi::OsStr::new(CARGO_HOME)) {
        return Err("explicit CARGO_HOME does not match the pinned observation".into());
    }
    let home = fs::canonicalize(CARGO_HOME).map_err(|e| format!("CARGO_HOME: {e}"))?;
    if home != Path::new(CARGO_HOME) {
        return Err("canonical CARGO_HOME differs from the pinned root".into());
    }
    let cache = root(&home, &home.join("registry/cache"))?;
    let extracted = root(&home, &home.join("registry/src"))?;
    acquire_roots(request, cache, extracted, Limits::NUCLEUS)
}

/// Legacy-route wrapper kept for the unchanged schema-v2 entry and its tests.
fn acquire_roots(
    request: AcquisitionRequest,
    cache: Root,
    extracted: Root,
    limits: Limits,
) -> Result<AcquisitionReport, String> {
    acquire_roots_route(
        request.source,
        request.phase_start,
        request.packages,
        cache,
        extracted,
        limits,
        Route::Legacy,
    )
}

/// Schema-v3 entry. It binds the pinned source leaves without re-reading Git and
/// accepts only the exact two SQLite registry tuples before using the shared
/// candidate discovery and the streaming verifier.
fn acquire_streaming(request: StreamingAcquisitionRequest) -> Result<AcquisitionReport, String> {
    if request.schema != "oxide.registry-acquisition.request.v3"
        || request.mode != "streaming"
        || request.source.commit != SOURCE_COMMIT
        || request.source.tree != SOURCE_TREE
        || request.source.lock_sha256 != LOCK_SHA256
    {
        return Err("streaming request identity or mode refused".into());
    }
    if request.source.leaves.len() != PINNED_SOURCE_LEAVES.len()
        || PINNED_SOURCE_LEAVES.iter().any(|(leaf, digest)| {
            request.source.leaves.get(*leaf).map(String::as_str) != Some(*digest)
        })
    {
        return Err("streaming request pinned source leaf set refused".into());
    }
    let starting_total = request
        .phase_start
        .counters
        .archive_bytes
        .saturating_add(request.phase_start.counters.decompressed_bytes)
        .saturating_add(request.phase_start.counters.extracted_bytes);
    if starting_total > Limits::NUCLEUS.collection_bytes
        || request.phase_start.counters.entries > Limits::NUCLEUS.associations
        || request.phase_start.candidate_observations > MAX_CANDIDATES
    {
        return Err("phase starting counters exceed the acquisition bounds".into());
    }
    if request.packages.len() != STREAMING_PACKAGES.len()
        || STREAMING_PACKAGES.iter().any(|(name, version, checksum)| {
            !request.packages.iter().any(|package| {
                package.name == *name
                    && package.version == *version
                    && package.checksum == *checksum
                    && package.source == "registry+https://github.com/rust-lang/crates.io-index"
            })
        })
    {
        return Err("streaming request must carry exactly the two pinned SQLite tuples".into());
    }
    if std::env::var_os("CARGO_HOME").as_deref() != Some(std::ffi::OsStr::new(CARGO_HOME)) {
        return Err("explicit CARGO_HOME does not match the pinned observation".into());
    }
    let home = fs::canonicalize(CARGO_HOME).map_err(|e| format!("CARGO_HOME: {e}"))?;
    if home != Path::new(CARGO_HOME) {
        return Err("canonical CARGO_HOME differs from the pinned root".into());
    }
    let cache = root(&home, &home.join("registry/cache"))?;
    let extracted = root(&home, &home.join("registry/src"))?;
    acquire_roots_route(
        request.source,
        request.phase_start,
        request.packages,
        cache,
        extracted,
        Limits::NUCLEUS,
        Route::Streaming,
    )
}

fn root(home: &Path, path: &Path) -> Result<Root, String> {
    let before =
        Metadata::read(path).map_err(|e| format!("root metadata {}: {e}", path.display()))?;
    if before.mode & 0o170000 != 0o040000 {
        return Err(format!(
            "registry root is not a real directory: {}",
            path.display()
        ));
    }
    let canonical = fs::canonicalize(path).map_err(|e| format!("root canonicalize: {e}"))?;
    if !canonical.starts_with(home) {
        return Err(format!(
            "registry root escaped CARGO_HOME: {}",
            canonical.display()
        ));
    }
    let mut namespaces = Vec::new();
    let entries = fs::read_dir(&canonical).map_err(|e| format!("registry namespaces: {e}"))?;
    for entry in entries.take(MAX_NAMESPACES) {
        let entry = entry.map_err(|e| format!("namespace entry: {e}"))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF-8 registry namespace".to_owned())?;
        let child = entry.path();
        let metadata = Metadata::read(&child).ok();
        let state = match metadata.as_ref() {
            Some(m) if m.mode & 0o170000 == 0o040000 => "directory",
            Some(m) if m.mode & 0o170000 == 0o120000 => "symlink",
            Some(_) => "other",
            None => "unreadable",
        }
        .to_owned();
        namespaces.push((name, child, state, metadata));
    }
    let after = Metadata::read(path).map_err(|e| format!("root post metadata: {e}"))?;
    if before != after {
        return Err(format!(
            "registry root changed while enumerating: {}",
            path.display()
        ));
    }
    Ok(Root {
        canonical,
        before,
        namespace_limit_reached: namespaces.len() == MAX_NAMESPACES,
        namespaces,
    })
}

fn root_snapshot(root: &Root, after: Option<Metadata>) -> RootSnapshot {
    let namespaces = root
        .namespaces
        .iter()
        .map(|(name, path, state, before)| {
            let after = Metadata::read(path).ok();
            (
                Namespace {
                    name: name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    state: state.clone(),
                    metadata_before: before.clone(),
                    metadata_after: after.clone(),
                },
                before == &after,
            )
        })
        .collect::<Vec<_>>();
    let namespaces_stable = namespaces.iter().all(|(_, stable)| *stable);
    RootSnapshot {
        path: root.canonical.to_string_lossy().into_owned(),
        metadata_before: root.before.clone(),
        stable: after.as_ref() == Some(&root.before) && namespaces_stable,
        namespace_limit_reached: root.namespace_limit_reached,
        metadata_after: after,
        namespaces: namespaces
            .into_iter()
            .map(|(namespace, _)| namespace)
            .collect(),
    }
}

fn acquire_roots_route(
    source: SourceIdentity,
    phase_start: PhaseStart,
    packages: Vec<LockedPackage>,
    cache: Root,
    extracted: Root,
    limits: Limits,
    route: Route,
) -> Result<AcquisitionReport, String> {
    let mut report = AcquisitionReport {
        schema: route.schema().into(),
        mode: route.mode().map(str::to_owned),
        source,
        phase_start,
        cache: root_snapshot(&cache, Some(cache.before.clone())),
        extracted: root_snapshot(&extracted, Some(extracted.before.clone())),
        candidate_observations: phase_start.candidate_observations,
        acquisition_candidate_observations: 0,
        packages: Vec::with_capacity(packages.len()),
        counters: phase_start.counters,
    };
    let mut counters: Counters = phase_start.counters.into();
    let unsafe_namespace = cache
        .namespaces
        .iter()
        .chain(&extracted.namespaces)
        .any(|(_, _, state, _)| state == "symlink" || state == "unreadable");
    if cache.namespace_limit_reached || extracted.namespace_limit_reached || unsafe_namespace {
        let refusal = if unsafe_namespace {
            "namespace_link_or_unreadable"
        } else {
            "namespace_observation_limit_reached"
        };
        for package in packages {
            report.packages.push(empty_outcome(
                package,
                "not_observed",
                refusal,
                counters.into(),
            ));
        }
        report.counters = counters.into();
        return Ok(report);
    }
    let mut capped = false;
    for package in packages {
        if capped {
            report.packages.push(empty_outcome(
                package,
                "not_observed",
                "candidate_observation_budget",
                counters.into(),
            ));
            continue;
        }
        if !safe_package(&package) {
            report.packages.push(empty_outcome(
                package,
                "refused",
                "unsupported_lock_tuple",
                counters.into(),
            ));
            continue;
        }
        let mut candidates = Vec::new();
        let mut archives = Vec::new();
        let mut sources = Vec::new();
        let archive_name = format!("{}-{}.crate", package.name, package.version);
        let root_name = format!("{}-{}", package.name, package.version);
        let mut candidate_error = None;
        for (kind, root, expected_name, target) in [
            ("archive", &cache, archive_name.as_str(), &mut archives),
            ("extracted", &extracted, root_name.as_str(), &mut sources),
        ] {
            for (namespace, namespace_path, namespace_state, _) in &root.namespaces {
                if namespace_state != "directory" {
                    continue;
                }
                if report.candidate_observations == MAX_CANDIDATES {
                    capped = true;
                    candidate_error = Some("candidate_observation_budget".to_owned());
                    break;
                }
                report.candidate_observations += 1;
                report.acquisition_candidate_observations += 1;
                let path = namespace_path.join(expected_name);
                let observed = observe_candidate(kind, namespace, &root.canonical, &path);
                match observed.state.as_str() {
                    "present" => target.push((
                        namespace.clone(),
                        path.clone(),
                        observed.metadata.clone().unwrap(),
                    )),
                    "missing" => {}
                    _ => candidate_error = Some("candidate_link_or_type".to_owned()),
                }
                candidates.push(observed);
            }
            if capped {
                break;
            }
        }
        if let Some(reason) = candidate_error {
            report.packages.push(PackageOutcome {
                package,
                candidates,
                archive: None,
                extracted_root: None,
                metadata_before: None,
                metadata_after: None,
                status: "refused".into(),
                refusal: Some(reason),
                files: BTreeMap::new(),
                directories: BTreeSet::new(),
                counters_after: counters.into(),
            });
            continue;
        }
        let complete_pairs = archives
            .iter()
            .flat_map(|archive| {
                sources
                    .iter()
                    .filter(move |source| archive.0 == source.0)
                    .map(move |source| (archive.clone(), source.clone()))
            })
            .collect::<Vec<_>>();
        if complete_pairs.len() != 1 {
            let reason = if archives.is_empty() && sources.is_empty() {
                "missing_archive_and_extraction"
            } else if archives.is_empty() {
                "missing_archive"
            } else if sources.is_empty() {
                "missing_extraction"
            } else if complete_pairs.is_empty() {
                "no_complete_namespace_pair"
            } else {
                "ambiguous_complete_pairs"
            };
            report.packages.push(PackageOutcome {
                package,
                candidates,
                archive: None,
                extracted_root: None,
                metadata_before: None,
                metadata_after: None,
                status: "refused".into(),
                refusal: Some(reason.into()),
                files: BTreeMap::new(),
                directories: BTreeSet::new(),
                counters_after: counters.into(),
            });
            continue;
        }
        let ((_, archive_path, archive_meta), (_, source_path, source_meta)) =
            complete_pairs.into_iter().next().unwrap();
        let request = Request {
            archive: &archive_path,
            extracted_root: &source_path,
            locked_checksum: &format!("sha256:{}", package.checksum),
        };
        let outcome = match route {
            Route::Legacy => archive::verify(&request, limits, counters),
            Route::Streaming => archive::stream_verify(&request, limits, counters),
        };
        counters = outcome.counters;
        let archive_after = Metadata::read(&archive_path).ok();
        let source_after = Metadata::read(&source_path).ok();
        let stable = archive_after.as_ref() == Some(&archive_meta)
            && source_after.as_ref() == Some(&source_meta);
        let (status, refusal, files, directories) = if !stable {
            (
                "refused".to_owned(),
                Some("unstable_metadata".to_owned()),
                BTreeMap::new(),
                BTreeSet::new(),
            )
        } else {
            match outcome.value {
                Ok(verified) => (
                    "supported".to_owned(),
                    None,
                    verified.files().clone(),
                    verified.directories().clone(),
                ),
                Err(error) => (
                    "refused".to_owned(),
                    Some(format!("verifier_{error:?}")),
                    BTreeMap::new(),
                    BTreeSet::new(),
                ),
            }
        };
        report.packages.push(PackageOutcome {
            package,
            candidates,
            archive: Some(archive_path.to_string_lossy().into_owned()),
            extracted_root: Some(source_path.to_string_lossy().into_owned()),
            metadata_before: Some((archive_meta, source_meta)),
            metadata_after: archive_after.zip(source_after),
            status,
            refusal,
            files,
            directories,
            counters_after: counters.into(),
        });
    }
    let cache_after = Metadata::read(&cache.canonical).ok();
    let extracted_after = Metadata::read(&extracted.canonical).ok();
    report.cache = root_snapshot(&cache, cache_after);
    report.extracted = root_snapshot(&extracted, extracted_after);
    if !report.cache.stable || !report.extracted.stable {
        for package in &mut report.packages {
            if package.status == "supported" {
                package.status = "refused".into();
                package.refusal = Some("unstable_registry_root_metadata".into());
                package.files.clear();
                package.directories.clear();
            }
        }
    }
    report.counters = counters.into();
    Ok(report)
}

fn empty_outcome(
    package: LockedPackage,
    status: &str,
    refusal: &str,
    counters: CounterSnapshot,
) -> PackageOutcome {
    PackageOutcome {
        package,
        candidates: Vec::new(),
        archive: None,
        extracted_root: None,
        metadata_before: None,
        metadata_after: None,
        status: status.into(),
        refusal: Some(refusal.into()),
        files: BTreeMap::new(),
        directories: BTreeSet::new(),
        counters_after: counters,
    }
}

fn observe_candidate(kind: &str, namespace: &str, namespace_root: &Path, path: &Path) -> Candidate {
    let path_text = path.to_string_lossy().into_owned();
    let result = Metadata::read(path);
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Candidate {
            kind: kind.into(),
            namespace: namespace.into(),
            path: path_text,
            state: "missing".into(),
            metadata: None,
            canonical: None,
        },
        Err(_) => Candidate {
            kind: kind.into(),
            namespace: namespace.into(),
            path: path_text,
            state: "unreadable".into(),
            metadata: None,
            canonical: None,
        },
        Ok(metadata) => {
            let expected_type = if kind == "archive" {
                0o100000
            } else {
                0o040000
            };
            if metadata.mode & 0o170000 == 0o120000 {
                return Candidate {
                    kind: kind.into(),
                    namespace: namespace.into(),
                    path: path_text,
                    state: "symlink".into(),
                    metadata: Some(metadata),
                    canonical: None,
                };
            }
            if metadata.mode & 0o170000 != expected_type
                || (kind == "archive" && metadata.links != 1)
            {
                return Candidate {
                    kind: kind.into(),
                    namespace: namespace.into(),
                    path: path_text,
                    state: "wrong_type".into(),
                    metadata: Some(metadata),
                    canonical: None,
                };
            }
            match fs::canonicalize(path) {
                Ok(canonical) if canonical.starts_with(namespace_root) && canonical == path => {
                    Candidate {
                        kind: kind.into(),
                        namespace: namespace.into(),
                        path: path_text,
                        state: "present".into(),
                        metadata: Some(metadata),
                        canonical: Some(canonical.to_string_lossy().into_owned()),
                    }
                }
                _ => Candidate {
                    kind: kind.into(),
                    namespace: namespace.into(),
                    path: path_text,
                    state: "escape".into(),
                    metadata: Some(metadata),
                    canonical: None,
                },
            }
        }
    }
}

fn safe_package(package: &LockedPackage) -> bool {
    package.source == "registry+https://github.com/rust-lang/crates.io-index"
        && safe_component(&package.name)
        && safe_component(&package.version)
        && package.checksum.len() == 64
        && package
            .checksum
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+'))
}

#[cfg(test)]
mod private_qualification_tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use sha2::Digest;
    use std::fs::File;
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tar::{Builder, Header};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn fixture_root(label: &str) -> PathBuf {
        let base =
            PathBuf::from(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("task scratch"));
        let path = base.join(format!(
            "registry-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn package(name: &str, version: &str, checksum: &str) -> LockedPackage {
        LockedPackage {
            name: name.into(),
            version: version.into(),
            source: "registry+https://github.com/rust-lang/crates.io-index".into(),
            checksum: checksum.into(),
        }
    }

    fn fixture_package(
        root: &Path,
        name: &str,
        version: &str,
    ) -> (LockedPackage, PathBuf, PathBuf) {
        let cache = root.join("cache/ns");
        let extracted = root.join("src/ns");
        fs::create_dir_all(&cache).unwrap();
        fs::create_dir_all(&extracted).unwrap();
        let archive = cache.join(format!("{name}-{version}.crate"));
        let extracted_root = extracted.join(format!("{name}-{version}"));
        fs::create_dir(&extracted_root).unwrap();
        fs::create_dir(extracted_root.join("src")).unwrap();
        let files = [
            ("Cargo.toml", b"[package]\nname='fixture'\n".as_slice()),
            ("src/lib.rs", b"pub fn f() {}\n".as_slice()),
        ];
        for (rel, bytes) in files {
            fs::write(extracted_root.join(rel), bytes).unwrap();
        }
        let encoder = GzEncoder::new(File::create(&archive).unwrap(), Compression::default());
        let mut tar = Builder::new(encoder);
        append_dir(&mut tar, &format!("{name}-{version}/"));
        append_file(
            &mut tar,
            &format!("{name}-{version}/Cargo.toml"),
            b"[package]\nname='fixture'\n",
        );
        append_dir(&mut tar, &format!("{name}-{version}/src/"));
        append_file(
            &mut tar,
            &format!("{name}-{version}/src/lib.rs"),
            b"pub fn f() {}\n",
        );
        let encoder = tar.into_inner().unwrap();
        encoder.finish().unwrap();
        let bytes = fs::read(&archive).unwrap();
        let checksum = format!("{:x}", sha2::Sha256::digest(&bytes));
        (package(name, version, &checksum), archive, extracted_root)
    }

    fn append_dir(tar: &mut Builder<GzEncoder<File>>, path: &str) {
        let mut header = Header::new_ustar();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, std::io::empty())
            .unwrap();
    }

    fn append_file(tar: &mut Builder<GzEncoder<File>>, path: &str, bytes: &[u8]) {
        let mut header = Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, path, bytes).unwrap();
    }

    fn roots(root_dir: &Path) -> (Root, Root) {
        let home = root_dir.canonicalize().unwrap();
        let cache = root_dir.join("cache");
        let src = root_dir.join("src");
        let cache_root = root(&home, &cache).unwrap();
        let src_root = root(&home, &src).unwrap();
        (cache_root, src_root)
    }

    #[test]
    fn private_qualification_registry_pair_uses_the_archive_verifier() {
        let root = fixture_root("pair");
        let (locked, _, _) = fixture_package(&root, "fixture", "1.0.0");
        let (cache, src) = roots(&root);
        let mut request = request(vec![locked]);
        request.phase_start = PhaseStart {
            counters: CounterSnapshot {
                archive_bytes: 7,
                decompressed_bytes: 11,
                extracted_bytes: 13,
                entries: 2,
            },
            candidate_observations: 5,
        };
        let report = super::acquire_roots(request, cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "supported");
        assert_eq!(report.packages[0].files.len(), 2);
        assert!(report.counters.archive_bytes > 7);
        assert!(report.counters.decompressed_bytes > 11);
        assert!(report.counters.extracted_bytes > 13);
        assert_eq!(report.candidate_observations, 7);
        assert_eq!(report.acquisition_candidate_observations, 2);
        assert_eq!(report.phase_start.candidate_observations, 5);
    }

    #[test]
    fn private_qualification_registry_pair_ignores_unmatched_spare_archive() {
        let root = fixture_root("spare-archive");
        let (locked, archive, _) = fixture_package(&root, "fixture", "1.0.0");
        let spare_namespace = root.join("cache/spare");
        fs::create_dir_all(&spare_namespace).unwrap();
        fs::copy(&archive, spare_namespace.join("fixture-1.0.0.crate")).unwrap();
        let (cache, src) = roots(&root);
        let report =
            super::acquire_roots(request(vec![locked]), cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "supported");
        assert_eq!(report.packages[0].candidates.len(), 3);
        assert_eq!(
            report.packages[0]
                .candidates
                .iter()
                .filter(|candidate| candidate.kind == "archive" && candidate.state == "present")
                .count(),
            2
        );
        assert_eq!(
            report.packages[0].archive.as_deref(),
            Some(archive.to_str().unwrap())
        );
    }

    #[test]
    fn private_qualification_registry_multiple_complete_pairs_refuse() {
        let root = fixture_root("multiple-pairs");
        let (locked, archive, extracted_root) = fixture_package(&root, "fixture", "1.0.0");
        let namespace = "second";
        let second_archive = root.join(format!("cache/{namespace}/fixture-1.0.0.crate"));
        let second_root = root.join(format!("src/{namespace}/fixture-1.0.0"));
        fs::create_dir_all(second_archive.parent().unwrap()).unwrap();
        fs::create_dir_all(&second_root.join("src")).unwrap();
        fs::copy(&archive, &second_archive).unwrap();
        fs::copy(
            extracted_root.join("Cargo.toml"),
            second_root.join("Cargo.toml"),
        )
        .unwrap();
        fs::copy(
            extracted_root.join("src/lib.rs"),
            second_root.join("src/lib.rs"),
        )
        .unwrap();
        let (cache, src) = roots(&root);
        let report =
            super::acquire_roots(request(vec![locked]), cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "refused");
        assert_eq!(
            report.packages[0].refusal.as_deref(),
            Some("ambiguous_complete_pairs")
        );
        assert_eq!(report.packages[0].candidates.len(), 4);
        assert!(report.packages[0].archive.is_none());
        assert!(report.packages[0].extracted_root.is_none());
    }

    #[test]
    fn private_qualification_registry_cross_namespace_candidates_do_not_pair() {
        let root = fixture_root("cross-namespace");
        let (locked, archive, extracted_root) = fixture_package(&root, "fixture", "1.0.0");
        let archive_namespace = root.join("cache/archive-only");
        let extraction_namespace = root.join("src/extraction-only");
        fs::create_dir_all(&archive_namespace).unwrap();
        fs::create_dir_all(&extraction_namespace).unwrap();
        fs::rename(archive, archive_namespace.join("fixture-1.0.0.crate")).unwrap();
        fs::rename(extracted_root, extraction_namespace.join("fixture-1.0.0")).unwrap();
        let (cache, src) = roots(&root);
        let report =
            super::acquire_roots(request(vec![locked]), cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "refused");
        assert_eq!(
            report.packages[0].refusal.as_deref(),
            Some("no_complete_namespace_pair")
        );
        assert!(report.packages[0].archive.is_none());
        assert!(report.packages[0].extracted_root.is_none());
    }

    #[test]
    fn private_qualification_registry_checksum_mismatch_is_retained_as_refusal() {
        let root = fixture_root("mismatch");
        let (mut locked, _, _) = fixture_package(&root, "fixture", "1.0.0");
        locked.checksum = "0".repeat(64);
        let (cache, src) = roots(&root);
        let report =
            super::acquire_roots(request(vec![locked]), cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "refused");
        assert_eq!(
            report.packages[0].refusal.as_deref(),
            Some("verifier_Integrity")
        );
        assert!(report.counters.archive_bytes > 0);
    }

    #[test]
    fn private_qualification_registry_link_and_missing_paths_refuse_before_reads() {
        let root = fixture_root("link");
        let (locked, _, extracted_root) = fixture_package(&root, "fixture", "1.0.0");
        let moved = root.join("outside");
        fs::rename(&extracted_root, &moved).unwrap();
        symlink(&moved, extracted_root).unwrap();
        let (cache, src) = roots(&root);
        let report =
            super::acquire_roots(request(vec![locked]), cache, src, Limits::NUCLEUS).unwrap();
        assert_eq!(report.packages[0].status, "refused");
        assert_eq!(
            report.packages[0].refusal.as_deref(),
            Some("candidate_link_or_type")
        );
        assert_eq!(report.counters, CounterSnapshot::default());
    }

    #[test]
    fn private_qualification_registry_missing_pair_is_reported_without_guessing() {
        let root = fixture_root("missing");
        fs::create_dir_all(root.join("cache/ns")).unwrap();
        fs::create_dir_all(root.join("src/ns")).unwrap();
        let (cache, src) = roots(&root);
        let report = super::acquire_roots(
            request(vec![package("missing", "1.0.0", &"0".repeat(64))]),
            cache,
            src,
            Limits::NUCLEUS,
        )
        .unwrap();
        assert_eq!(report.packages[0].status, "refused");
        assert_eq!(
            report.packages[0].refusal.as_deref(),
            Some("missing_archive_and_extraction")
        );
        assert_eq!(report.counters, CounterSnapshot::default());
    }

    #[test]
    fn private_qualification_registry_collection_budget_is_shared_across_packages() {
        let root = fixture_root("shared");
        let (first, _, _) = fixture_package(&root, "first", "1.0.0");
        let (cache, src) = roots(&root);
        let full = super::acquire_roots(request(vec![first.clone()]), cache, src, Limits::NUCLEUS)
            .unwrap();
        let used = full.counters.archive_bytes
            + full.counters.decompressed_bytes
            + full.counters.extracted_bytes;
        let shared_root = fixture_root("budget-shared");
        let (one, _, _) = fixture_package(&shared_root, "first", "1.0.0");
        let (two, _, _) = fixture_package(&shared_root, "second", "1.0.0");
        let (cache, src) = roots(&shared_root);
        let mut limits = Limits::NUCLEUS;
        limits.collection_bytes = used + 1;
        let report = super::acquire_roots(request(vec![one, two]), cache, src, limits).unwrap();
        assert_eq!(report.packages[0].status, "supported");
        assert_eq!(report.packages[1].status, "refused");
        assert_eq!(
            report.packages[1].refusal.as_deref(),
            Some("verifier_Budget")
        );
        assert!(
            report.packages[1].counters_after.archive_bytes
                >= report.packages[0].counters_after.archive_bytes
        );
    }

    fn request(packages: Vec<LockedPackage>) -> AcquisitionRequest {
        AcquisitionRequest {
            schema: "oxide.registry-acquisition.request.v2".into(),
            source: SourceIdentity {
                commit: SOURCE_COMMIT.into(),
                tree: SOURCE_TREE.into(),
                lock_sha256: LOCK_SHA256.into(),
                leaves: BTreeMap::new(),
            },
            phase_start: PhaseStart::default(),
            packages,
        }
    }

    fn streaming_request(mode: &str, packages: Vec<LockedPackage>) -> StreamingAcquisitionRequest {
        let mut leaves = BTreeMap::new();
        for (leaf, digest) in super::PINNED_SOURCE_LEAVES {
            leaves.insert(leaf.to_owned(), digest.to_owned());
        }
        StreamingAcquisitionRequest {
            schema: "oxide.registry-acquisition.request.v3".into(),
            mode: mode.into(),
            source: SourceIdentity {
                commit: SOURCE_COMMIT.into(),
                tree: SOURCE_TREE.into(),
                lock_sha256: LOCK_SHA256.into(),
                leaves,
            },
            phase_start: PhaseStart::default(),
            packages,
        }
    }

    fn streaming_packages() -> Vec<LockedPackage> {
        super::STREAMING_PACKAGES
            .iter()
            .map(|(name, version, checksum)| package(name, version, checksum))
            .collect()
    }

    fn run_streaming_error(request: &StreamingAcquisitionRequest) -> String {
        let input = serde_json::to_vec(request).unwrap();
        let mut output = Vec::new();
        run_streaming(std::io::Cursor::new(input), &mut output)
            .expect_err("invalid streaming requests must refuse")
    }

    #[test]
    fn private_qualification_streaming_refuses_wrong_mode_and_package_set() {
        assert!(
            run_streaming_error(&streaming_request("legacy", streaming_packages()))
                .contains("identity or mode")
        );
        let mut one = streaming_packages();
        one.truncate(1);
        assert!(run_streaming_error(&streaming_request("streaming", one))
            .contains("two pinned SQLite tuples"));
        let mut extra = streaming_packages();
        extra.push(package("serde", "1.0.229", &"00".repeat(32)));
        assert!(run_streaming_error(&streaming_request("streaming", extra))
            .contains("two pinned SQLite tuples"));
        // A schema-v2 legacy request cannot select the streaming route.
        let legacy = serde_json::to_vec(&request(Vec::new())).unwrap();
        assert!(run_streaming(std::io::Cursor::new(legacy), &mut Vec::new()).is_err());
    }

    #[test]
    fn private_qualification_streaming_route_admits_what_legacy_caps() {
        let root = fixture_root("streaming-route");
        let (locked, _, _) = fixture_package(&root, "fixture", "1.0.0");
        let mut limits = Limits::NUCLEUS;
        limits.file_bytes = 1024;
        limits.collection_bytes = 64 * 1024;
        let (cache, src) = roots(&root);
        let legacy_report =
            super::acquire_roots(request(vec![locked.clone()]), cache, src, limits).unwrap();
        assert_eq!(legacy_report.schema, "oxide.registry-acquisition.v2");
        assert!(legacy_report.mode.is_none());
        assert_eq!(
            legacy_report.packages[0].refusal.as_deref(),
            Some("verifier_Budget")
        );

        let (cache, src) = roots(&root);
        let request_v3 = super::StreamingAcquisitionRequest {
            schema: "oxide.registry-acquisition.request.v3".into(),
            mode: "streaming".into(),
            source: SourceIdentity {
                commit: SOURCE_COMMIT.into(),
                tree: SOURCE_TREE.into(),
                lock_sha256: LOCK_SHA256.into(),
                leaves: BTreeMap::new(),
            },
            phase_start: PhaseStart::default(),
            packages: vec![locked],
        };
        let streamed = super::acquire_roots_route(
            request_v3.source,
            request_v3.phase_start,
            request_v3.packages,
            cache,
            src,
            limits,
            super::Route::Streaming,
        )
        .unwrap();
        assert_eq!(streamed.schema, "oxide.registry-acquisition.v3");
        assert_eq!(streamed.mode.as_deref(), Some("streaming"));
        assert_eq!(streamed.packages[0].status, "supported");
        assert_eq!(streamed.packages[0].files.len(), 2);
        assert!(streamed.counters.decompressed_bytes > limits.file_bytes);
    }
}
