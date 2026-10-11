//! Bounded collector for the pinned Cargo serialized fingerprint format.
//!
//! The collector reads only paths named by a constructor-owned audited graph
//! context. It does not infer units or graph edges from fingerprint hashes.

use super::dep_info::{self, PathTag, RegisteredFile, TrustedContext};
use super::{
    canonical_dir, owned, relative, Association, Attempt, EvidenceReadBudget, ImmutableClass,
    ImmutableManifest, InputPath, InputRoot, LocalInput, Refusal, Registration, Result,
    UnitEvidence,
};
use serde::de::{self, SeqAccess, Visitor};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::marker::PhantomData;
use std::mem::size_of;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Cargo source revision whose fingerprint and dep-info formats are decoded.
pub const CARGO_SOURCE_COMMIT: &str = "c980f4866141969fab6254a680546a277789d6f0";
/// Cargo version used by the accepted Oxide toolchain pin.
pub const CARGO_VERSION: &str = "1.97.1";

const MAX_DEPENDENCIES_PER_UNIT: usize = 1024;
const MAX_PATHS_PER_RERUN_RECORD: usize = 64;
const MAX_RUSTFLAGS_PER_UNIT: usize = 256;
const FINGERPRINT_DECODE_OVERHEAD: usize = 512 * 1024;

/// Accepted graph and path context, built by the installed `InputEvidenceOwner`
/// after it authenticates graph provenance and audits complete input behavior.
/// Do not deserialize this value from task configuration or caller JSON.
pub struct TrustedCargoGraphContext {
    /// Exact Cargo release identifier accepted by this module.
    pub cargo_version: String,
    /// Full Cargo source commit accepted by this module.
    pub cargo_source_commit: String,
    /// Digest of the graph independently authenticated by the owner.
    pub graph_digest: String,
    /// Cargo `build.build-dir`, which the runner binds to `Attempt.slot`.
    pub build_root: PathBuf,
    /// Keys are semantic unit IDs from the audited graph, never Cargo hash
    /// directory names.
    pub units: BTreeMap<String, CargoUnitContext>,
}
// Keep the consumer's physical build root out of diagnostic formatting.
impl std::fmt::Debug for TrustedCargoGraphContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedCargoGraphContext")
            .field("cargo_version", &self.cargo_version)
            .field("cargo_source_commit", &self.cargo_source_commit)
            .field("graph_digest", &self.graph_digest)
            .field("unit_count", &self.units.len())
            .finish_non_exhaustive()
    }
}

/// Explicit evidence paths and accepted associations for one audited unit.
pub struct CargoUnitContext {
    /// Canonical package root for resolving package-relative Cargo paths.
    pub package_root: PathBuf,
    /// Registered root class backing `package_root` and package references.
    pub package_origin: InputRoot,
    /// Exact directory under the Cargo build root whose inventory is checked.
    pub fingerprint_directory: String,
    /// Exact serialized Cargo fingerprint filename within that directory.
    pub fingerprint_file: String,
    /// Complete accepted inventory for `fingerprint_directory`.
    pub directory_entries: BTreeSet<String>,
    /// Exact mappings keyed by Cargo's build-root-relative `dep_info` value.
    pub dep_info: BTreeMap<String, Vec<RegisteredFile>>,
    /// Exact package-root-relative mappings for `RerunIfChanged.paths`.
    pub rerun_files: BTreeMap<String, Association>,
    /// Other explicitly audited consuming/native/immutable associations.
    pub associations: Vec<Association>,
    /// Semantic graph edges corresponding to Cargo's serialized dependency
    /// summaries. The summary hash is parsed but never treated as graph proof.
    pub dependencies: Vec<CargoDependency>,
    /// Complete effective environment map, including deliberate absence.
    pub effective_environment: BTreeMap<String, Option<String>>,
    /// Trusted-owner permission for this exact immutable-registry unit to use
    /// `Precalculated`; mutable, build-script and rustdoc policies leave it unset.
    pub audited_precalculated_registry_receipt: Option<String>,
    /// Additional named build-root evidence anchors for this unit.
    pub additional_anchors: BTreeSet<String>,
}
// Report counts without exposing roots, input paths or effective environment values.
impl std::fmt::Debug for CargoUnitContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CargoUnitContext")
            .field("fingerprint_file_count", &self.directory_entries.len())
            .field("dep_info_record_count", &self.dep_info.len())
            .field("rerun_input_count", &self.rerun_files.len())
            .field("association_count", &self.associations.len())
            .field("dependency_count", &self.dependencies.len())
            .field("environment_entry_count", &self.effective_environment.len())
            .field(
                "audited_precalculated",
                &self.audited_precalculated_registry_receipt.is_some(),
            )
            .finish_non_exhaustive()
    }
}

/// One audited semantic edge matching Cargo's package ID/name/public tuple.
#[derive(Clone, Debug)]
pub struct CargoDependency {
    /// Cargo's package identity number from the dependency fingerprint tuple.
    pub package_id: u64,
    /// Cargo dependency name from the fingerprint tuple.
    pub name: String,
    /// Public-dependency bit from the fingerprint tuple.
    pub public: bool,
    /// Semantic graph unit resolved by the accepted owner for this edge.
    pub unit: String,
}

/// Collect complete `UnitEvidence` for every unit in the accepted graph.
///
/// The graph context must come from trusted embedding code, not task data. The
/// runner passes its per-operation byte budget so evidence reads, decoded
/// strings and bounded vector allocations share one ceiling with input reads.
pub fn collect(
    registration: &Registration,
    attempt: &Attempt,
    manifests: &BTreeMap<String, ImmutableManifest>,
    budget: &mut EvidenceReadBudget,
    context: &TrustedCargoGraphContext,
) -> Result<Vec<UnitEvidence>> {
    let limits = registration.limits;
    limits.validate()?;
    if context.cargo_version != CARGO_VERSION
        || context.cargo_source_commit != CARGO_SOURCE_COMMIT
        || context.graph_digest != registration.graph_digest
        || context.build_root != attempt.slot
        || context.units.len() != registration.units.len()
        || context
            .units
            .keys()
            .any(|unit_id| !registration.units.contains(unit_id))
        || context.units.len() > limits.units
    {
        return Err(Refusal::Identity);
    }
    canonical_dir(&context.build_root)?;

    budget.decoded(
        context
            .units
            .len()
            .checked_mul(size_of::<UnitEvidence>() + size_of::<DirectoryInventory>())
            .ok_or(Refusal::Budget)?,
    )?;
    let mut result = Vec::with_capacity(context.units.len());
    let mut association_count = 0usize;
    let mut inventories = Vec::with_capacity(context.units.len());
    let mut fingerprint_directories = BTreeSet::<&str>::new();
    for (unit_id, unit) in &context.units {
        if unit_id.is_empty()
            || unit_id.len() > limits.path_bytes
            || unit_id.as_bytes().contains(&0)
            || unit.effective_environment
                != *registration
                    .unit_environment
                    .get(unit_id)
                    .ok_or(Refusal::Identity)?
        {
            return Err(Refusal::Identity);
        }
        if unit.dependencies.len() > MAX_DEPENDENCIES_PER_UNIT
            || unit.associations.len() > limits.associations
            || unit.additional_anchors.len() > limits.entries_per_unit
            || unit.effective_environment.len() > limits.associations
            || unit.dep_info.len() > limits.associations
            || unit.rerun_files.len() > limits.associations
        {
            return Err(Refusal::Budget);
        }
        relative(&unit.fingerprint_directory, limits)?;
        if !fingerprint_directories.insert(unit.fingerprint_directory.as_str()) {
            return Err(Refusal::MissingAuthority);
        }
        for path in unit.dep_info.keys().chain(unit.rerun_files.keys()) {
            relative(path, limits)?;
        }
        budget.decoded(
            unit_id
                .len()
                .checked_add(size_of::<String>() + 64)
                .ok_or(Refusal::Budget)?,
        )?;
        validate_package_root(unit, attempt, manifests)?;
        charge(
            &mut association_count,
            unit.dependencies.len(),
            limits.associations,
        )?;
        charge(
            &mut association_count,
            unit.associations.len(),
            limits.associations,
        )?;
        let inventory = read_inventory(&context.build_root, unit, budget, limits)?;
        inventories.push(inventory);

        let fingerprint_path = format!("{}/{}", unit.fingerprint_directory, unit.fingerprint_file);
        relative(&fingerprint_path, limits)?;
        let fingerprint_path = owned(&context.build_root, &fingerprint_path, limits)?;
        let fingerprint_bytes = budget.evidence_bytes(&fingerprint_path, limits.file_bytes)?;
        let fingerprint = decode_fingerprint(&fingerprint_bytes, budget)?;
        if fingerprint.local.is_empty() {
            return Err(Refusal::Unsupported);
        }
        check_dependencies(
            &fingerprint.dependencies,
            &unit.dependencies,
            registration,
            budget,
        )?;

        let mut anchors = BTreeSet::new();
        for entry in &unit.directory_entries {
            let length = unit
                .fingerprint_directory
                .len()
                .checked_add(entry.len() + 1 + size_of::<String>() + 64)
                .ok_or(Refusal::Budget)?;
            budget.decoded(length)?;
            anchors.insert(format!("{}/{}", unit.fingerprint_directory, entry));
        }
        for anchor in &unit.additional_anchors {
            relative(anchor, limits)?;
            budget.decoded(anchor.len() + size_of::<String>() + 64)?;
            anchors.insert(anchor.clone());
        }
        budget.decoded(
            fingerprint
                .local
                .len()
                .checked_mul(size_of::<LocalInput>())
                .ok_or(Refusal::Budget)?,
        )?;
        let mut locals = Vec::with_capacity(fingerprint.local.len());
        let mut associations = Vec::new();
        let mut used_dep_info = BTreeSet::new();
        let mut used_rerun_files = BTreeSet::new();
        let mut observed_environment = BTreeMap::<String, Option<String>>::new();
        let mut saw_precalculated = false;

        for local in fingerprint.local {
            match local {
                LocalFingerprint::CheckDepInfo(CheckDepInfo { dep_info, checksum }) => {
                    relative(&dep_info, limits)?;
                    if checksum || !used_dep_info.insert(dep_info.clone()) {
                        return Err(Refusal::Unsupported);
                    }
                    let mappings = unit
                        .dep_info
                        .get(&dep_info)
                        .ok_or(Refusal::MissingAuthority)?;
                    let path = owned(&context.build_root, &dep_info, limits)?;
                    let bytes = budget.evidence_bytes(&path, limits.file_bytes)?;
                    let decoded = dep_info::decode_with_budget(&bytes, budget)?;
                    if mappings.len() != decoded.files.len() || mappings.len() > limits.associations
                    {
                        return Err(Refusal::MissingAuthority);
                    }
                    charge(
                        &mut association_count,
                        decoded.files.len() + decoded.environment.len(),
                        limits.associations,
                    )?;
                    let conversion_allocation = mappings.iter().try_fold(
                        decoded
                            .files
                            .len()
                            .checked_mul(size_of::<InputPath>() + size_of::<Association>())
                            .and_then(|size| {
                                size.checked_add(
                                    unit_id.len() + dep_info.len() + size_of::<LocalInput>(),
                                )
                            })
                            .ok_or(Refusal::Budget)?,
                        |size, mapping| {
                            let duplicated_path = mapping
                                .input
                                .path
                                .len()
                                .checked_add(match &mapping.input.root {
                                    InputRoot::Immutable(receipt) => receipt.len(),
                                    _ => 0,
                                })
                                .and_then(|bytes| bytes.checked_add(size_of::<InputPath>()))
                                .ok_or(Refusal::Budget)?;
                            size.checked_add(association_allocation(&mapping.association)?)
                                .and_then(|size| size.checked_add(duplicated_path))
                                .ok_or(Refusal::Budget)
                        },
                    )?;
                    budget.decoded(conversion_allocation)?;
                    for file in &mappings[..] {
                        match file.reference.tag {
                            PathTag::PackageRootRelative
                                if file.input.root == unit.package_origin => {}
                            PathTag::BuildRootRelative
                                if file.input.root == InputRoot::Generated => {}
                            _ => return Err(Refusal::MissingAuthority),
                        }
                    }
                    for (name, value) in &decoded.environment {
                        check_environment(
                            registration,
                            unit_id,
                            &unit.effective_environment,
                            name,
                            value.as_deref(),
                        )?;
                        if observed_environment
                            .insert(name.clone(), value.clone())
                            .is_some_and(|old| old != *value)
                        {
                            return Err(Refusal::Identity);
                        }
                    }
                    let converted = decoded.into_registered(TrustedContext {
                        unit: unit_id,
                        anchor: &dep_info,
                        registered_units: &registration.units,
                        files: mappings,
                    })?;
                    anchors.insert(dep_info.clone());
                    locals.push(converted.local);
                    extend_associations(&mut associations, converted.associations, budget)?;
                }
                LocalFingerprint::RerunIfChanged(RerunIfChanged { output, paths }) => {
                    relative(&output, limits)?;
                    let output_path = owned(&context.build_root, &output, limits)?;
                    if !fs::symlink_metadata(&output_path)
                        .map_err(|_| Refusal::Io)?
                        .is_file()
                    {
                        return Err(Refusal::Unsupported);
                    }
                    anchors.insert(output.clone());
                    let mut inputs = Vec::with_capacity(paths.len());
                    let mut seen_paths = BTreeSet::new();
                    for path in paths {
                        relative(&path, limits)?;
                        if !seen_paths.insert(path.clone())
                            || !used_rerun_files.insert(path.clone())
                        {
                            return Err(Refusal::Unsupported);
                        }
                        charge(&mut association_count, 1, limits.associations)?;
                        let association = unit
                            .rerun_files
                            .get(&path)
                            .ok_or(Refusal::MissingAuthority)?;
                        if association.input.root != unit.package_origin
                            || !association.anchors.contains(&output)
                        {
                            return Err(Refusal::MissingAuthority);
                        }
                        validate_association(association, registration, manifests, limits)?;
                        budget.decoded(association_allocation(association)?)?;
                        inputs.push(association.input.clone());
                        push_association(&mut associations, association.clone(), budget)?;
                    }
                    locals.push(LocalInput::RerunFiles {
                        output,
                        paths: inputs,
                    });
                }
                LocalFingerprint::RerunIfEnvChanged(RerunIfEnvChanged { var, val }) => {
                    check_environment(
                        registration,
                        unit_id,
                        &unit.effective_environment,
                        &var,
                        val.as_deref(),
                    )?;
                    if observed_environment
                        .insert(var.clone(), val.clone())
                        .is_some_and(|old| old != val)
                    {
                        return Err(Refusal::Identity);
                    }
                    charge(&mut association_count, 1, limits.associations)?;
                    locals.push(LocalInput::RerunEnvironment {
                        name: var,
                        value: val,
                    });
                }
                LocalFingerprint::Precalculated(value) => {
                    if saw_precalculated
                        || value.is_empty()
                        || value.len() > limits.path_bytes
                        || value.as_bytes().contains(&0)
                    {
                        return Err(Refusal::Unsupported);
                    }
                    let receipt = unit
                        .audited_precalculated_registry_receipt
                        .as_ref()
                        .ok_or(Refusal::MissingAuthority)?;
                    if receipt.is_empty()
                        || receipt.len() > limits.path_bytes
                        || receipt.as_bytes().contains(&0)
                    {
                        return Err(Refusal::MissingAuthority);
                    }
                    budget.decoded(
                        receipt
                            .len()
                            .checked_add(size_of::<String>() + 64)
                            .and_then(|size| size.checked_mul(2))
                            .ok_or(Refusal::Budget)?,
                    )?;
                    if unit.package_origin != InputRoot::Immutable(receipt.clone())
                        || !matches!(manifests.get(receipt), Some(m) if m.class == ImmutableClass::Registry)
                    {
                        return Err(Refusal::Unsupported);
                    }
                    saw_precalculated = true;
                    locals.push(LocalInput::Precalculated {
                        value,
                        registry_receipt: receipt.clone(),
                    });
                }
            }
        }
        if used_dep_info.len() != unit.dep_info.len()
            || unit
                .dep_info
                .keys()
                .any(|path| !used_dep_info.contains(path))
            || used_rerun_files.len() != unit.rerun_files.len()
            || unit
                .rerun_files
                .keys()
                .any(|path| !used_rerun_files.contains(path))
            || saw_precalculated != unit.audited_precalculated_registry_receipt.is_some()
        {
            return Err(Refusal::MissingAuthority);
        }

        for association in &unit.associations {
            validate_association(association, registration, manifests, limits)?;
        }
        extend_associations_from_slice(&mut associations, &unit.associations, budget)?;
        ensure_unique_associations(&associations, budget)?;
        for association in &associations {
            if association.anchors.is_empty() {
                return Err(Refusal::MissingAuthority);
            }
            for anchor in &association.anchors {
                relative(anchor, limits)?;
                anchors.insert(anchor.clone());
            }
        }
        for anchor in &anchors {
            owned(&context.build_root, anchor, limits)?;
        }
        if anchors.is_empty()
            || anchors.len() > limits.entries_per_unit
            || locals.len() > limits.entries_per_unit
            || associations.len() > limits.associations
        {
            return Err(Refusal::Budget);
        }
        charge(
            &mut association_count,
            anchors.len() + locals.len() + associations.len(),
            limits.associations,
        )?;
        result.push(UnitEvidence {
            id: unit_id.clone(),
            locals,
            associations,
            anchors,
        });
    }

    for inventory in &inventories {
        verify_inventory(inventory, budget, limits)?;
    }
    Ok(result)
}

fn validate_package_root(
    unit: &CargoUnitContext,
    attempt: &Attempt,
    manifests: &BTreeMap<String, ImmutableManifest>,
) -> Result<()> {
    canonical_dir(&unit.package_root)?;
    let accepted_root = match &unit.package_origin {
        InputRoot::Source => &attempt.source_root,
        InputRoot::Generated => &attempt.slot,
        InputRoot::Immutable(receipt) => {
            &manifests
                .get(receipt)
                .ok_or(Refusal::MissingAuthority)?
                .root
        }
    };
    if !unit.package_root.starts_with(accepted_root) {
        return Err(Refusal::Identity);
    }
    Ok(())
}

#[derive(Debug)]
struct DirectoryInventory {
    path: PathBuf,
    expected: BTreeSet<String>,
    entries: BTreeSet<String>,
    stamp: (u64, u64, u64),
    ctime: i64,
    ctime_nsec: i64,
}

fn read_inventory(
    build_root: &Path,
    unit: &CargoUnitContext,
    budget: &mut EvidenceReadBudget,
    limits: super::Limits,
) -> Result<DirectoryInventory> {
    relative(&unit.fingerprint_directory, limits)?;
    relative(&unit.fingerprint_file, limits)?;
    if Path::new(&unit.fingerprint_file).components().count() != 1
        || unit.directory_entries.is_empty()
        || unit.directory_entries.len() > limits.entries_per_unit
        || !unit.directory_entries.contains(&unit.fingerprint_file)
    {
        return Err(Refusal::MissingAuthority);
    }
    for entry in &unit.directory_entries {
        relative(entry, limits)?;
        if Path::new(entry).components().count() != 1 {
            return Err(Refusal::Unsupported);
        }
    }
    let inventory_allocation =
        unit.directory_entries
            .iter()
            .try_fold(0usize, |allocation, entry| {
                allocation
                    .checked_add(entry.len() + size_of::<String>() + 64)
                    .ok_or(Refusal::Budget)
            })?;
    budget.decoded(inventory_allocation.checked_mul(2).ok_or(Refusal::Budget)?)?;
    let path = owned(build_root, &unit.fingerprint_directory, limits)?;
    let before = fs::symlink_metadata(&path).map_err(|_| Refusal::Io)?;
    if !before.is_dir() || before.file_type().is_symlink() {
        return Err(Refusal::Unsupported);
    }
    let mut entries = BTreeSet::new();
    for entry in fs::read_dir(&path).map_err(|_| Refusal::Io)? {
        budget.visits += 1;
        if budget.visits > limits.associations || entries.len() >= limits.entries_per_unit {
            return Err(Refusal::Budget);
        }
        let entry = entry.map_err(|_| Refusal::Io)?;
        let file_type = entry.file_type().map_err(|_| Refusal::Io)?;
        let file_name = entry.file_name();
        budget.decoded(file_name.len() + size_of::<String>() + 64)?;
        let name = file_name.into_string().map_err(|_| Refusal::Unsupported)?;
        relative(&name, limits)?;
        if !file_type.is_file() || file_type.is_symlink() || !entries.insert(name) {
            return Err(Refusal::Unsupported);
        }
    }
    let after = fs::symlink_metadata(&path).map_err(|_| Refusal::Io)?;
    if super::stamp(&before)? != super::stamp(&after)?
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
        || entries != unit.directory_entries
    {
        return Err(Refusal::Membership);
    }
    Ok(DirectoryInventory {
        path,
        expected: unit.directory_entries.clone(),
        entries,
        stamp: super::stamp(&after)?,
        ctime: after.ctime(),
        ctime_nsec: after.ctime_nsec(),
    })
}

fn verify_inventory(
    inventory: &DirectoryInventory,
    budget: &mut EvidenceReadBudget,
    limits: super::Limits,
) -> Result<()> {
    let before = fs::symlink_metadata(&inventory.path).map_err(|_| Refusal::Io)?;
    if !before.is_dir()
        || before.file_type().is_symlink()
        || super::stamp(&before)? != inventory.stamp
        || before.ctime() != inventory.ctime
        || before.ctime_nsec() != inventory.ctime_nsec
    {
        return Err(Refusal::Unstable);
    }
    let mut entries = BTreeSet::new();
    for entry in fs::read_dir(&inventory.path).map_err(|_| Refusal::Io)? {
        budget.visits += 1;
        if budget.visits > limits.associations || entries.len() >= limits.entries_per_unit {
            return Err(Refusal::Budget);
        }
        let entry = entry.map_err(|_| Refusal::Io)?;
        let file_type = entry.file_type().map_err(|_| Refusal::Io)?;
        let file_name = entry.file_name();
        budget.decoded(file_name.len() + size_of::<String>() + 64)?;
        let name = file_name.into_string().map_err(|_| Refusal::Unsupported)?;
        if !file_type.is_file() || file_type.is_symlink() || !entries.insert(name) {
            return Err(Refusal::Unsupported);
        }
    }
    let after = fs::symlink_metadata(&inventory.path).map_err(|_| Refusal::Io)?;
    if super::stamp(&after)? != inventory.stamp
        || after.ctime() != inventory.ctime
        || after.ctime_nsec() != inventory.ctime_nsec
        || entries != inventory.entries
        || entries != inventory.expected
    {
        return Err(Refusal::Membership);
    }
    Ok(())
}

fn check_dependencies(
    observed: &[DependencySummary],
    expected: &[CargoDependency],
    registration: &Registration,
    budget: &mut EvidenceReadBudget,
) -> Result<()> {
    if observed.len() != expected.len() || observed.len() > MAX_DEPENDENCIES_PER_UNIT {
        return Err(Refusal::MissingAuthority);
    }
    let comparison_allocation = observed
        .iter()
        .map(|dependency| dependency.1.len() + size_of::<(u64, String, bool)>() + 64)
        .chain(
            expected
                .iter()
                .map(|dependency| dependency.name.len() + size_of::<(u64, String, bool)>() + 64),
        )
        .try_fold(0usize, |total, size| {
            total.checked_add(size).ok_or(Refusal::Budget)
        })?;
    budget.decoded(comparison_allocation)?;
    let mut observed_keys = BTreeSet::new();
    for dependency in observed {
        if dependency.1.is_empty()
            || dependency.1.len() > registration.limits.path_bytes
            || dependency.1.as_bytes().contains(&0)
        {
            return Err(Refusal::MissingAuthority);
        }
        let key = (dependency.0, dependency.1.clone(), dependency.2);
        if !observed_keys.insert(key) {
            return Err(Refusal::MissingAuthority);
        }
    }
    let mut expected_keys = BTreeSet::new();
    for dependency in expected {
        if !registration.units.contains(&dependency.unit)
            || dependency.unit.is_empty()
            || dependency.name.is_empty()
            || dependency.name.len() > registration.limits.path_bytes
            || dependency.name.as_bytes().contains(&0)
            || !expected_keys.insert((
                dependency.package_id,
                dependency.name.clone(),
                dependency.public,
            ))
        {
            return Err(Refusal::MissingAuthority);
        }
    }
    if observed_keys != expected_keys {
        return Err(Refusal::MissingAuthority);
    }
    Ok(())
}

fn check_environment(
    registration: &Registration,
    unit: &str,
    environment: &BTreeMap<String, Option<String>>,
    name: &str,
    value: Option<&str>,
) -> Result<()> {
    if name.is_empty()
        || name.len() > registration.limits.path_bytes
        || name.contains(['=', '\0'])
        || value.is_some_and(|value| {
            value.len() > registration.limits.path_bytes || value.as_bytes().contains(&0)
        })
        || environment.get(name).map(Option::as_deref) != Some(value)
        || registration
            .unit_environment
            .get(unit)
            .and_then(|values| values.get(name))
            .map(Option::as_deref)
            != Some(value)
    {
        return Err(Refusal::Identity);
    }
    Ok(())
}

fn validate_association(
    association: &Association,
    registration: &Registration,
    manifests: &BTreeMap<String, ImmutableManifest>,
    limits: super::Limits,
) -> Result<()> {
    relative(&association.input.path, limits)?;
    if association.anchors.is_empty() || association.anchors.len() > limits.entries_per_unit {
        return Err(Refusal::MissingAuthority);
    }
    for anchor in &association.anchors {
        relative(anchor, limits)?;
    }
    match (&association.input.root, &association.producer) {
        (InputRoot::Generated, Some(producer)) if registration.units.contains(producer) => {}
        (InputRoot::Generated, _) => return Err(Refusal::MissingAuthority),
        (_, Some(_)) => return Err(Refusal::MissingAuthority),
        _ => {}
    }
    if let Some(receipt) = &association.native_receipt {
        if !matches!(manifests.get(receipt), Some(m) if m.class == ImmutableClass::ToolSdk) {
            return Err(Refusal::MissingAuthority);
        }
    }
    if let InputRoot::Immutable(receipt) = &association.input.root {
        if !manifests.contains_key(receipt) {
            return Err(Refusal::MissingAuthority);
        }
    }
    Ok(())
}

fn extend_associations(
    target: &mut Vec<Association>,
    additions: Vec<Association>,
    budget: &mut EvidenceReadBudget,
) -> Result<()> {
    for association in additions {
        push_association(target, association, budget)?;
    }
    Ok(())
}

fn extend_associations_from_slice(
    target: &mut Vec<Association>,
    additions: &[Association],
    budget: &mut EvidenceReadBudget,
) -> Result<()> {
    for association in additions {
        budget.decoded(association_allocation(association)?)?;
        push_association(target, association.clone(), budget)?;
    }
    Ok(())
}

fn push_association(
    target: &mut Vec<Association>,
    association: Association,
    budget: &mut EvidenceReadBudget,
) -> Result<()> {
    budget.decoded(size_of::<Association>())?;
    target.push(association);
    Ok(())
}

fn ensure_unique_associations(
    associations: &[Association],
    budget: &mut EvidenceReadBudget,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for association in associations {
        budget.decoded(size_of::<(&InputPath, &BTreeSet<String>)>() + 64)?;
        if !seen.insert((&association.input, &association.anchors)) {
            return Err(Refusal::MissingAuthority);
        }
    }
    Ok(())
}

fn association_allocation(association: &Association) -> Result<usize> {
    let anchors = association
        .anchors
        .iter()
        .try_fold(0usize, |size, anchor| {
            size.checked_add(anchor.len() + size_of::<String>() + 64)
                .ok_or(Refusal::Budget)
        })?;
    size_of::<Association>()
        .checked_add(size_of::<InputPath>())
        .and_then(|size| size.checked_add(association.input.path.len()))
        .and_then(|size| size.checked_add(anchors))
        .and_then(|size| {
            size.checked_add(
                association
                    .producer
                    .as_ref()
                    .map_or(0, |producer| producer.len()),
            )
        })
        .and_then(|size| {
            size.checked_add(
                association
                    .native_receipt
                    .as_ref()
                    .map_or(0, |receipt| receipt.len()),
            )
        })
        .ok_or(Refusal::Budget)
}

fn charge(count: &mut usize, value: usize, maximum: usize) -> Result<()> {
    *count = count
        .checked_add(value)
        .filter(|count| *count <= maximum)
        .ok_or(Refusal::Budget)?;
    Ok(())
}

fn decode_fingerprint(bytes: &[u8], budget: &mut EvidenceReadBudget) -> Result<Fingerprint> {
    if bytes.len() > budget.limits.file_bytes {
        return Err(Refusal::Budget);
    }
    budget.decoded(
        bytes
            .len()
            .checked_mul(3)
            .and_then(|length| length.checked_add(FINGERPRINT_DECODE_OVERHEAD))
            .ok_or(Refusal::Budget)?,
    )?;
    serde_json::from_slice(bytes).map_err(|_| Refusal::Unsupported)
}

#[allow(dead_code)] // Persisted fields outside freshness are parsed to validate the pinned schema.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fingerprint {
    rustc: u64,
    features: String,
    declared_features: String,
    target: u64,
    profile: u64,
    path: u64,
    #[serde(rename = "deps", deserialize_with = "deserialize_dependencies")]
    dependencies: Vec<DependencySummary>,
    #[serde(deserialize_with = "deserialize_locals")]
    local: Vec<LocalFingerprint>,
    #[serde(deserialize_with = "deserialize_rustflags")]
    rustflags: Vec<String>,
    config: u64,
    compile_kind: u64,
}

#[allow(dead_code)] // Cargo's dependency fingerprint hash is parsed but is not graph authority.
#[derive(Debug)]
struct DependencySummary(u64, String, bool, u64);

impl<'de> Deserialize<'de> for DependencySummary {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DependencyVisitor;
        impl<'de> Visitor<'de> for DependencyVisitor {
            type Value = DependencySummary;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a four-field Cargo dependency fingerprint tuple")
            }

            fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let package_id = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(0, &self))?;
                let name = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(1, &self))?;
                let public = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(2, &self))?;
                let fingerprint = sequence
                    .next_element()?
                    .ok_or_else(|| de::Error::invalid_length(3, &self))?;
                if sequence.next_element::<de::IgnoredAny>()?.is_some() {
                    return Err(de::Error::custom("dependency tuple has extra fields"));
                }
                Ok(DependencySummary(package_id, name, public, fingerprint))
            }
        }
        deserializer.deserialize_tuple(4, DependencyVisitor)
    }
}

#[derive(Debug, Deserialize)]
enum LocalFingerprint {
    Precalculated(String),
    CheckDepInfo(CheckDepInfo),
    RerunIfChanged(RerunIfChanged),
    RerunIfEnvChanged(RerunIfEnvChanged),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckDepInfo {
    dep_info: String,
    checksum: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RerunIfChanged {
    output: String,
    #[serde(deserialize_with = "deserialize_paths")]
    paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RerunIfEnvChanged {
    var: String,
    val: Option<String>,
}

fn deserialize_dependencies<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<DependencySummary>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded(deserializer, MAX_DEPENDENCIES_PER_UNIT)
}

fn deserialize_locals<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<LocalFingerprint>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded(deserializer, super::Limits::NUCLEUS.entries_per_unit)
}

fn deserialize_paths<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded(deserializer, MAX_PATHS_PER_RERUN_RECORD)
}

fn deserialize_rustflags<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_bounded(deserializer, MAX_RUSTFLAGS_PER_UNIT)
}

fn deserialize_bounded<'de, D, T>(
    deserializer: D,
    maximum: usize,
) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct BoundedVisitor<T> {
        maximum: usize,
        marker: PhantomData<T>,
    }
    impl<'de, T> Visitor<'de> for BoundedVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(formatter, "a sequence of at most {} items", self.maximum)
        }
        fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let capacity = sequence.size_hint().unwrap_or(0).min(self.maximum);
            let mut values = Vec::with_capacity(capacity);
            while values.len() < self.maximum {
                match sequence.next_element()? {
                    Some(value) => values.push(value),
                    None => return Ok(values),
                }
            }
            if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(de::Error::custom("sequence exceeds its evidence limit"));
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(BoundedVisitor {
        maximum,
        marker: PhantomData,
    })
}

#[cfg(test)]
mod tests;
