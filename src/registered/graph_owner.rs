//! Qualification-only Cargo reading-owner provisioning.
//!
//! Untrusted parent JSON is verified and converted by this owner. The resulting
//! `TrustedCargoGraphContext` is constructor-owned and is never deserialized.
//! Synthetic installation stays labelled synthetic; it is not Nucleus graph proof.

use super::cargo_freshness::{
    CargoDependency, CargoUnitContext, TrustedCargoGraphContext, CARGO_SOURCE_COMMIT, CARGO_VERSION,
};
use super::dep_info::{FileReference, PathTag, RegisteredFile};
use super::qualification::{canonical_digest, Bootstrap, MacKey};
use super::runner_host::{canonical_json_bytes, HostError, StorageIdentity};
use super::{
    canonical_dir, relative, Association, Attempt, EvidenceReadBudget, ImmutableManifest,
    InputPath, InputRoot, Refusal, Registration,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;
use std::path::PathBuf;

/// Versioned callable schema for the private qualification graph owner.
pub const GRAPH_OWNER_SCHEMA: &str = "oxide-qualification-cargo-graph-v1";
/// Explicit synthetic issuer label. Unknown issuers refuse.
pub const GRAPH_OWNER_ISSUER: &str = "synthetic-qualification";
/// Provisioning version. An incompatible schema or version refuses; map pin is separate.
pub const GRAPH_OWNER_VERSION: u32 = 1;
const GRAPH_OWNER_INSTALL_DOMAIN: &str = "oxide-qualification-graph-owner-install-v1";
/// Receipt domain for an accepted graph body bound to the acquired attempt.
pub const GRAPH_OWNER_PROVISION_DOMAIN: &str = "oxide-qualification-graph-owner-v1";

/// Bootstrap installation of the qualification Cargo reading owner.
/// Absent `graph_owner` keeps parent `input.units` in-memory fixture mode.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphOwnerAuthority {
    pub version: u32,
    pub schema: String,
    pub issuer: String,
    pub receipt: String,
}

impl GraphOwnerAuthority {
    /// Constructor-owned issuance for one bootstrap. Binds schema, issuer and start.
    pub fn issue(bootstrap: &Bootstrap) -> std::result::Result<Self, HostError> {
        let authority = Self {
            version: GRAPH_OWNER_VERSION,
            schema: GRAPH_OWNER_SCHEMA.to_owned(),
            issuer: GRAPH_OWNER_ISSUER.to_owned(),
            receipt: Self::receipt(bootstrap)?,
        };
        authority.bounds()?;
        Ok(authority)
    }

    fn receipt(bootstrap: &Bootstrap) -> std::result::Result<String, HostError> {
        MacKey::from_hex(&bootstrap.callback_key_hex)?.sign(
            GRAPH_OWNER_INSTALL_DOMAIN,
            &[
                &bootstrap.scope,
                &bootstrap.storage_instance_id,
                &bootstrap.start_digest,
                GRAPH_OWNER_SCHEMA,
                GRAPH_OWNER_ISSUER,
                &GRAPH_OWNER_VERSION.to_string(),
            ],
        )
    }

    fn bounds(&self) -> std::result::Result<(), HostError> {
        if self.version != GRAPH_OWNER_VERSION
            || self.schema != GRAPH_OWNER_SCHEMA
            || self.issuer != GRAPH_OWNER_ISSUER
            || self.receipt.is_empty()
            || self.receipt.len() > 1024
        {
            return Err(HostError::Protocol);
        }
        Ok(())
    }

    /// Unsupported schema/issuer/version is Protocol. Forged, moved or reminted
    /// installation receipts are OwnerRejected.
    pub fn verify(&self, bootstrap: &Bootstrap) -> std::result::Result<(), HostError> {
        self.bounds()?;
        let expected = Self::receipt(bootstrap)?;
        if expected != self.receipt {
            return Err(HostError::OwnerRejected);
        }
        Ok(())
    }
}

/// Untrusted parent graph body. Converted only after receipt verification.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphProvision {
    pub version: u32,
    pub schema: String,
    pub issuer: String,
    pub cargo_version: String,
    pub cargo_source_commit: String,
    pub graph_digest: String,
    pub units: Vec<GraphUnitProvision>,
    pub receipt: String,
}

/// One untrusted unit record. IDs are unique; the vector form detects duplicates.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphUnitProvision {
    pub id: String,
    pub package_root: PathBuf,
    pub package_origin: InputRoot,
    pub fingerprint_directory: String,
    pub fingerprint_file: String,
    pub directory_entries: BTreeSet<String>,
    pub dep_info: Vec<DepInfoMapping>,
    pub rerun_files: Vec<RerunMapping>,
    pub associations: Vec<Association>,
    pub dependencies: Vec<GraphEdgeProvision>,
    pub effective_environment: BTreeMap<String, Option<String>>,
    pub audited_precalculated_registry_receipt: Option<String>,
    pub additional_anchors: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DepInfoMapping {
    pub path: String,
    pub files: Vec<RegisteredFileProvision>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredFileProvision {
    pub tag: WirePathTag,
    pub path: String,
    pub input: InputPath,
    pub association: Association,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePathTag {
    PackageRoot,
    BuildRoot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RerunMapping {
    pub path: String,
    pub association: Association,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEdgeProvision {
    pub package_id: u64,
    pub name: String,
    pub public: bool,
    pub unit: String,
}

/// Canonical subject the parent must sign. `build_root` is the acquired slot.
pub fn graph_provision_subject(
    provision: &GraphProvision,
    attempt: &Attempt,
    storage: &StorageIdentity,
    scope: &str,
    start_digest: &str,
) -> std::result::Result<Value, HostError> {
    let mut body = provision.clone();
    body.receipt.clear();
    serde_json::to_value(&body)
        .map_err(|_| HostError::Protocol)
        .map(|body| {
            json!({
                "body": body,
                "source_root": attempt.source_root,
                "root_id": attempt.root_id,
                "run_id": attempt.binding.run_id,
                "attempt_id": attempt.binding.attempt_id,
                "build_root": attempt.slot,
                "storage_instance_id": storage.storage_instance_id,
                "slot_id": storage.slot_id,
                "generation": storage.generation,
                "scope": scope,
                "start_digest": start_digest,
            })
        })
}

/// External-parent issuance: sign the canonical subject with the bootstrap key.
pub fn issue_graph_provision(
    key: &MacKey,
    scope: &str,
    storage_instance_id: &str,
    start_digest: &str,
    attempt: &Attempt,
    storage: &StorageIdentity,
    mut provision: GraphProvision,
) -> std::result::Result<GraphProvision, HostError> {
    provision.receipt.clear();
    let digest = canonical_digest(&graph_provision_subject(
        &provision,
        attempt,
        storage,
        scope,
        start_digest,
    )?)?;
    provision.receipt = key.sign(
        GRAPH_OWNER_PROVISION_DOMAIN,
        &[scope, storage_instance_id, start_digest, &digest],
    )?;
    Ok(provision)
}

/// Verify issuer provenance and convert to a constructor-owned trusted context.
/// Charges decode and copies to the runner's operation budget. Never reads
/// `build_root` from the parent; the acquired `Attempt.slot` is bound here.
pub fn accept_graph_provision(
    provision: GraphProvision,
    registration: &Registration,
    attempt: &Attempt,
    manifests: &BTreeMap<String, ImmutableManifest>,
    budget: &mut EvidenceReadBudget,
    key: &MacKey,
    scope: &str,
    start_digest: &str,
    storage: &StorageIdentity,
) -> super::Result<TrustedCargoGraphContext> {
    let encoded =
        canonical_json_bytes(&serde_json::to_value(&provision).map_err(|_| Refusal::Unsupported)?)
            .map_err(|_| Refusal::Unsupported)?;
    budget.decoded(encoded.len())?;
    if provision.version != GRAPH_OWNER_VERSION
        || provision.schema != GRAPH_OWNER_SCHEMA
        || provision.issuer != GRAPH_OWNER_ISSUER
        || provision.cargo_version != CARGO_VERSION
        || provision.cargo_source_commit != CARGO_SOURCE_COMMIT
        || provision.graph_digest != registration.graph_digest
        || provision.units.len() != registration.units.len()
        || provision.units.len() > registration.limits.units
        || storage.storage_instance_id.is_empty()
        || storage.slot_id.is_empty()
        || storage.generation.is_empty()
    {
        return Err(Refusal::Identity);
    }
    let expected = issue_graph_provision(
        key,
        scope,
        &storage.storage_instance_id,
        start_digest,
        attempt,
        storage,
        provision.clone(),
    )
    .map_err(|_| Refusal::Authentication)?;
    if expected.receipt != provision.receipt {
        return Err(Refusal::Authentication);
    }

    let limits = registration.limits;
    let mut units = BTreeMap::new();
    for unit in provision.units {
        budget.decoded(
            unit.id
                .len()
                .checked_add(size_of::<CargoUnitContext>() + 64)
                .ok_or(Refusal::Budget)?,
        )?;
        if unit.id.is_empty()
            || unit.id.len() > limits.path_bytes
            || unit.id.as_bytes().contains(&0)
            || !registration.units.contains(&unit.id)
            || units.contains_key(&unit.id)
        {
            return Err(Refusal::Identity);
        }
        relative(&unit.fingerprint_directory, limits)?;
        relative(&unit.fingerprint_file, limits)?;
        for entry in &unit.directory_entries {
            relative(entry, limits)?;
        }
        for mapping in &unit.dep_info {
            relative(&mapping.path, limits)?;
            if mapping.path.is_empty() {
                return Err(Refusal::Unsupported);
            }
        }
        for mapping in &unit.rerun_files {
            relative(&mapping.path, limits)?;
        }
        for anchor in &unit.additional_anchors {
            relative(anchor, limits)?;
        }
        if unit.effective_environment
            != *registration
                .unit_environment
                .get(&unit.id)
                .ok_or(Refusal::Identity)?
        {
            return Err(Refusal::Identity);
        }
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
        if let Some(receipt) = &unit.audited_precalculated_registry_receipt {
            if !matches!(
                manifests.get(receipt),
                Some(manifest) if manifest.class == super::ImmutableClass::Registry
            ) {
                return Err(Refusal::MissingAuthority);
            }
        }
        let mut dep_info = BTreeMap::new();
        for mapping in unit.dep_info {
            if dep_info.contains_key(&mapping.path) {
                return Err(Refusal::MissingAuthority);
            }
            let mut files = Vec::with_capacity(mapping.files.len());
            for file in mapping.files {
                relative(&file.path, limits)?;
                relative(&file.association.input.path, limits)?;
                for anchor in &file.association.anchors {
                    relative(anchor, limits)?;
                }
                budget.decoded(
                    file.path
                        .len()
                        .checked_add(
                            file.association.input.path.len() + size_of::<RegisteredFile>(),
                        )
                        .ok_or(Refusal::Budget)?,
                )?;
                files.push(RegisteredFile {
                    reference: FileReference {
                        tag: match file.tag {
                            WirePathTag::PackageRoot => PathTag::PackageRootRelative,
                            WirePathTag::BuildRoot => PathTag::BuildRootRelative,
                        },
                        path: file.path,
                    },
                    input: file.input,
                    association: file.association,
                });
            }
            dep_info.insert(mapping.path, files);
        }
        let mut rerun_files = BTreeMap::new();
        for mapping in unit.rerun_files {
            if rerun_files.contains_key(&mapping.path) {
                return Err(Refusal::MissingAuthority);
            }
            relative(&mapping.association.input.path, limits)?;
            for anchor in &mapping.association.anchors {
                relative(anchor, limits)?;
            }
            if let InputRoot::Immutable(receipt) = &mapping.association.input.root {
                if !manifests.contains_key(receipt) {
                    return Err(Refusal::MissingAuthority);
                }
            }
            rerun_files.insert(mapping.path, mapping.association);
        }
        for association in &unit.associations {
            relative(&association.input.path, limits)?;
            for anchor in &association.anchors {
                relative(anchor, limits)?;
            }
            if let InputRoot::Immutable(receipt) = &association.input.root {
                if !manifests.contains_key(receipt) {
                    return Err(Refusal::MissingAuthority);
                }
            }
            if let Some(receipt) = &association.native_receipt {
                if !manifests.contains_key(receipt) {
                    return Err(Refusal::MissingAuthority);
                }
            }
        }
        let mut dependencies = Vec::with_capacity(unit.dependencies.len());
        for edge in unit.dependencies {
            budget.decoded(edge.name.len() + edge.unit.len() + size_of::<CargoDependency>())?;
            if !registration.units.contains(&edge.unit)
                || edge.name.is_empty()
                || edge.unit.is_empty()
            {
                return Err(Refusal::MissingAuthority);
            }
            dependencies.push(CargoDependency {
                package_id: edge.package_id,
                name: edge.name,
                public: edge.public,
                unit: edge.unit,
            });
        }
        units.insert(
            unit.id,
            CargoUnitContext {
                package_root: unit.package_root,
                package_origin: unit.package_origin,
                fingerprint_directory: unit.fingerprint_directory,
                fingerprint_file: unit.fingerprint_file,
                directory_entries: unit.directory_entries,
                dep_info,
                rerun_files,
                associations: unit.associations,
                dependencies,
                effective_environment: unit.effective_environment,
                audited_precalculated_registry_receipt: unit.audited_precalculated_registry_receipt,
                additional_anchors: unit.additional_anchors,
            },
        );
    }
    if units.keys().any(|id| !registration.units.contains(id))
        || units.len() != registration.units.len()
    {
        return Err(Refusal::Identity);
    }
    Ok(TrustedCargoGraphContext {
        cargo_version: provision.cargo_version,
        cargo_source_commit: provision.cargo_source_commit,
        graph_digest: provision.graph_digest,
        build_root: attempt.slot.clone(),
        units,
    })
}
