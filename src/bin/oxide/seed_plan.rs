//! Effect-free selection from generation-bound catalog data.
//!
//! The caller verifies sources before supplying them here. This module never
//! opens donor payload; the library revalidates the chosen catalog and every
//! file under the donor pin.

use crate::seed_lock::Package;
use oxide_storage::lease::managed::{
    SeedCandidate, SeedDonor, SeedProducer, SeedRecipient, SeedRequest, SeedRole, SeedSourceKind,
    SeedUnitRef,
};
use oxide_storage::lease::seed::{
    SEED_MAX_PLAN_BYTES, SEED_MAX_PLAN_FILES, SEED_MAX_PLAN_INPUTS, SEED_MAX_PLAN_UNITS,
};

/// The result of verifying the recipient's locked source against the adapter.
#[derive(Clone, Debug)]
pub(crate) struct VerifiedSource {
    pub package: Package,
    pub tree_digest: String,
    pub kind: SeedSourceKind,
    pub root: std::path::PathBuf,
    /// Root whose complete content was verified. For a Git workspace this
    /// can contain the package root as a subdirectory.
    pub verification_root: std::path::PathBuf,
    pub proc_macro: bool,
}

fn same_context(a: &SeedProducer, b: &SeedProducer) -> bool {
    a.cargo_version == b.cargo_version
        && a.rustc_digest == b.rustc_digest
        && a.rustc == b.rustc
        && a.target == b.target
        && a.recipe == b.recipe
        && a.backend == b.backend
        && a.adapter == b.adapter
        && a.context_digest == b.context_digest
        && a.flags_digest == b.flags_digest
        && a.cargo_home == b.cargo_home
        && a.command_family == b.command_family
}

/// Select the largest overlap, breaking ties by newer rank then slot id. Caps
/// reject an entire plan rather than silently truncating a unit or donor.
pub(crate) fn select(
    recipient: &SeedRecipient,
    producer: &SeedProducer,
    allowed: &[Package],
    sources: &[VerifiedSource],
    candidates: &[SeedCandidate],
) -> Option<SeedRequest> {
    let mut best: Option<(&SeedCandidate, Vec<SeedUnitRef>)> = None;
    for candidate in candidates {
        if !same_context(producer, &candidate.catalog.producer) {
            continue;
        }
        let mut units = Vec::new();
        let (mut files, mut bytes, mut inputs) = (0usize, 0u64, 0usize);
        let mut capped = false;
        for package in &candidate.catalog.packages {
            let matching_source = sources.iter().find(|source| {
                let lock = &source.package;
                allowed.contains(lock)
                    && package.unique
                    && package.leaf
                    && package.name == lock.name
                    && package.target == lock.name.replace('-', "_")
                    && package.version == lock.version
                    && Some(package.source.as_str()) == lock.source.as_deref()
                    && package.checksum == lock.checksum
                    && package.source_root.kind == source.kind
                    && package.source_root.digest == source.tree_digest
            });
            let Some(source) = matching_source else {
                continue;
            };
            for unit in &package.units {
                if matches!(unit.role, SeedRole::Lib) && source.proc_macro
                    || matches!(unit.role, SeedRole::ProcMacro) && !source.proc_macro
                {
                    continue;
                }
                // These are recorded prerequisites, not donor payload reads.
                // Reject lexical escapes and roots other than the reverified
                // source tree. Source verification forbids links inside it.
                if unit.join.inputs.is_empty()
                    || unit.join.inputs.iter().any(|input| {
                        let path = std::path::Path::new(input);
                        !path.is_absolute()
                            || !path.starts_with(&source.root)
                            || path
                                .components()
                                .any(|part| matches!(part, std::path::Component::ParentDir))
                    })
                {
                    continue;
                }
                files = match files.checked_add(unit.files.len()) {
                    Some(value) => value,
                    None => {
                        capped = true;
                        break;
                    }
                };
                inputs = match inputs.checked_add(unit.join.inputs.len()) {
                    Some(value) => value,
                    None => {
                        capped = true;
                        break;
                    }
                };
                for file in &unit.files {
                    match bytes.checked_add(file.size) {
                        Some(value) => bytes = value,
                        None => {
                            capped = true;
                            break;
                        }
                    }
                }
                units.push(SeedUnitRef {
                    fingerprint_dir: unit.fingerprint_dir.clone(),
                    role: unit.role,
                });
                if units.len() > SEED_MAX_PLAN_UNITS
                    || files > SEED_MAX_PLAN_FILES
                    || inputs > SEED_MAX_PLAN_INPUTS
                    || bytes > SEED_MAX_PLAN_BYTES
                {
                    capped = true;
                    break;
                }
            }
            if capped {
                break;
            }
        }
        if capped || units.is_empty() {
            continue;
        }
        let better = best.as_ref().is_none_or(|(old, old_units)| {
            units.len() > old_units.len()
                || (units.len() == old_units.len()
                    && (candidate.rank > old.rank
                        || (candidate.rank == old.rank && candidate.slot_id < old.slot_id)))
        });
        if better {
            best = Some((candidate, units));
        }
    }
    best.map(|(candidate, units)| SeedRequest {
        recipient: recipient.clone(),
        donor: SeedDonor {
            slot_id: candidate.slot_id.clone(),
            incarnation: candidate.incarnation.clone(),
            lease_id: candidate.catalog.lease_id.clone(),
            record_id: candidate.catalog.record_id,
            catalog_digest: candidate.catalog_digest.clone(),
        },
        units,
        work_budget_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn producer() -> SeedProducer {
        SeedProducer {
            cargo_version: "1.97.1".into(),
            rustc_digest: "rustc-digest".into(),
            rustc: "/rustc".into(),
            target: "target".into(),
            recipe: "quick".into(),
            backend: "directory-budget-v1".into(),
            adapter: "adapter".into(),
            context_digest: "context".into(),
            config_files: vec!["/first".into()],
            flags_digest: "flags".into(),
            cargo_home: "/cargo".into(),
            command_family: "build".into(),
            command_class: "build".into(),
        }
    }

    #[test]
    fn context_uses_effective_digest_not_config_file_list() {
        let a = producer();
        let mut b = a.clone();
        b.config_files = vec!["/other".into()];
        assert!(same_context(&a, &b));
        b.context_digest = "different".into();
        assert!(!same_context(&a, &b));
        b = a.clone();
        b.cargo_home = "/other".into();
        assert!(!same_context(&a, &b));
        b = a.clone();
        b.command_class = "test".into();
        assert!(same_context(&a, &b));
        b.command_family = "check".into();
        assert!(!same_context(&a, &b));
    }

    #[test]
    fn no_candidate_means_no_plan() {
        let recipient = SeedRecipient {
            slot_id: "1".into(),
            incarnation: "i".into(),
            lease_id: "l".into(),
        };
        assert!(select(&recipient, &producer(), &[], &[], &[]).is_none());
    }
    fn candidate(id: &str, rank: u64, digest: &str) -> SeedCandidate {
        use oxide_storage::lease::managed::{
            SeedAttemptState, SeedCatalog, SeedJoin, SeedPackage, SeedRole, SeedSourceRoot,
            SeedUnit,
        };
        SeedCandidate {
            slot_id: id.into(),
            incarnation: "i".into(),
            rank,
            catalog_digest: "catalog".into(),
            catalog: SeedCatalog {
                schema: "oxide.cold-seed.catalog/1".into(),
                store_id: "store".into(),
                slot_id: id.into(),
                incarnation: "i".into(),
                lease_id: "lease".into(),
                record_id: 1,
                mark: 1,
                started_at_ms: 0,
                finished_at_ms: 1,
                attempt: SeedAttemptState::None,
                producer: producer(),
                packages: vec![SeedPackage {
                    name: "cfg-if".into(),
                    target: "cfg_if".into(),
                    version: "1".into(),
                    source: "registry+https://github.com/rust-lang/crates.io-index".into(),
                    checksum: Some("a".repeat(64)),
                    unique: true,
                    leaf: true,
                    source_root: SeedSourceRoot {
                        kind: SeedSourceKind::Registry,
                        digest: digest.into(),
                    },
                    units: vec![SeedUnit {
                        fingerprint_dir: "debug/.fingerprint/cfg-if-a".into(),
                        role: SeedRole::Lib,
                        target: "cfg_if".into(),
                        join: SeedJoin {
                            path: "d".into(),
                            targets: vec![],
                            inputs: vec!["/fixture/src/lib.rs".into()],
                            digest: "d".into(),
                        },
                        files: vec![],
                    }],
                }],
                rejected: vec![],
            },
        }
    }

    #[test]
    fn selection_requires_verified_matching_source_and_breaks_ties_by_rank() {
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source = VerifiedSource {
            package,
            tree_digest: "tree".into(),
            kind: SeedSourceKind::Registry,
            root: "/fixture".into(),
            verification_root: "/fixture".into(),
            proc_macro: false,
        };
        let recipient = SeedRecipient {
            slot_id: "recipient".into(),
            incarnation: "i".into(),
            lease_id: "l".into(),
        };
        let candidates = [
            candidate("old", 1, "tree"),
            candidate("new", 2, "tree"),
            candidate("wrong", 3, "other"),
        ];
        assert!(select(&recipient, &producer(), &[], &[source.clone()], &candidates).is_none());
        let allowed = [source.package.clone()];
        let selected = select(&recipient, &producer(), &allowed, &[source], &candidates).unwrap();
        assert_eq!(selected.donor.slot_id, "new");
        assert_eq!(selected.units.len(), 1);
    }
}
