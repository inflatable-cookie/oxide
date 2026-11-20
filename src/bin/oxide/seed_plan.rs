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

/// Why effect-free selection found no plan. The front door's user-facing
/// mapping is a separate change; this layer exposes the fact so a caller never
/// has to report every miss as "no eligible donor".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectMiss {
    /// Call 1 offered no donor candidate.
    NoCandidates,
    /// Every candidate's producer context differs from this command's.
    ContextMismatch,
    /// A context-matched candidate's plan exceeded a plan bound. A cap
    /// rejects the whole candidate rather than truncating it.
    PlanCap,
    /// A candidate unit's recorded join input lies outside its verified
    /// source root, so the unit is never planned.
    InputRootEscape,
    /// Context-matched candidates were considered, but no catalog unit
    /// matched the allowlist and verified sources.
    NoMatchingUnit,
}

/// Select the largest overlap, breaking ties by newer rank then slot id. Caps
/// reject an entire plan rather than silently truncating a unit or donor.
pub(crate) fn select(
    recipient: &SeedRecipient,
    producer: &SeedProducer,
    allowed: &[Package],
    sources: &[VerifiedSource],
    candidates: &[SeedCandidate],
) -> Result<SeedRequest, SelectMiss> {
    let max_units = SEED_MAX_PLAN_UNITS;
    let max_files = SEED_MAX_PLAN_FILES;
    let max_bytes = SEED_MAX_PLAN_BYTES;
    let mut best: Option<(&SeedCandidate, Vec<SeedUnitRef>)> = None;
    let mut context_matched = false;
    let mut input_escape = false;
    let mut capped_any = false;
    for candidate in candidates {
        if !same_context(producer, &candidate.catalog.producer) {
            continue;
        }
        context_matched = true;
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
            // v1 offers only plain registry library compile units. A
            // proc-macro or build-script unit in a catalog is never selected,
            // even if a producer wrote one; Git is rejected by the lock.
            for unit in &package.units {
                if package.source_root.kind != SeedSourceKind::Registry
                    || source.kind != SeedSourceKind::Registry
                    || source.proc_macro
                    || unit.role != SeedRole::Lib
                {
                    continue;
                }
                // These are recorded prerequisites, not donor payload reads.
                // Reject lexical escapes and roots other than the reverified
                // source tree. Source verification forbids links inside it.
                if unit.join.inputs.is_empty() {
                    continue;
                }
                if unit.join.inputs.iter().any(|input| {
                    let path = std::path::Path::new(input);
                    !path.is_absolute()
                        || !path.starts_with(&source.root)
                        || path
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                }) {
                    input_escape = true;
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
                if units.len() > max_units
                    || files > max_files
                    || inputs > SEED_MAX_PLAN_INPUTS
                    || bytes > max_bytes
                {
                    capped = true;
                    break;
                }
            }
            if capped {
                break;
            }
        }
        if capped {
            capped_any = true;
            continue;
        }
        if units.is_empty() {
            continue;
        }
        let better = best.as_ref().is_none_or(|(old, old_units)| {
            units.len() > old_units.len()
                || (units.len() == old_units.len()
                    && (candidate.rank > old.rank
                        || (candidate.rank == old.rank
                            && candidate.slot_id.parse::<u64>().unwrap_or(u64::MAX)
                                < old.slot_id.parse::<u64>().unwrap_or(u64::MAX))))
        });
        if better {
            best = Some((candidate, units));
        }
    }
    let Some((candidate, units)) = best else {
        return Err(if candidates.is_empty() {
            SelectMiss::NoCandidates
        } else if !context_matched {
            SelectMiss::ContextMismatch
        } else if capped_any {
            SelectMiss::PlanCap
        } else if input_escape {
            SelectMiss::InputRootEscape
        } else {
            SelectMiss::NoMatchingUnit
        });
    };
    Ok(SeedRequest {
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
        assert_eq!(
            select(&recipient, &producer(), &[], &[], &[]),
            Err(SelectMiss::NoCandidates)
        );
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
        assert_eq!(
            select(&recipient, &producer(), &[], &[source.clone()], &candidates),
            Err(SelectMiss::NoMatchingUnit)
        );
        let allowed = [source.package.clone()];
        let selected = select(&recipient, &producer(), &allowed, &[source], &candidates).unwrap();
        assert_eq!(selected.donor.slot_id, "new");
        assert_eq!(selected.units.len(), 1);
    }

    #[test]
    fn equal_rank_uses_numeric_slot_order() {
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source = verified_source(package.clone());
        let candidates = [candidate("10", 1, "tree"), candidate("2", 1, "tree")];
        let selected = select(
            &recipient(),
            &producer(),
            &[package],
            &[source],
            &candidates,
        )
        .unwrap();
        assert_eq!(selected.donor.slot_id, "2");
    }

    fn verified_source(package: crate::seed_lock::Package) -> VerifiedSource {
        VerifiedSource {
            package,
            tree_digest: "tree".into(),
            kind: SeedSourceKind::Registry,
            root: "/fixture".into(),
            verification_root: "/fixture".into(),
            proc_macro: false,
        }
    }

    /// Layout v1 and layout v2 donors never meet: the adapter is part of the
    /// producer context, so a donor of the other layout is a context mismatch.
    #[test]
    fn a_donor_of_the_other_layout_is_a_context_mismatch() {
        use oxide_storage::lease::seed::{ADAPTER_V1, ADAPTER_V2};
        let package = crate::seed_lock::eligible(cfg_if_lock().as_bytes())
            .unwrap()
            .remove(0);
        let source = verified_source(package.clone());
        let allowed = [package];
        let mut v1_donor = candidate("donor", 1, "tree");
        v1_donor.catalog.producer.adapter = ADAPTER_V1.into();
        let mut v1_recipient = producer();
        v1_recipient.adapter = ADAPTER_V1.into();
        // Control: the same donor is selected for a recipient of its own layout.
        assert!(select(
            &recipient(),
            &v1_recipient,
            &allowed,
            &[source.clone()],
            &[v1_donor.clone()]
        )
        .is_ok());
        let mut v2_recipient = producer();
        v2_recipient.adapter = ADAPTER_V2.into();
        assert_eq!(
            select(
                &recipient(),
                &v2_recipient,
                &allowed,
                &[source],
                &[v1_donor]
            ),
            Err(SelectMiss::ContextMismatch)
        );
    }

    fn recipient() -> SeedRecipient {
        SeedRecipient {
            slot_id: "recipient".into(),
            incarnation: "i".into(),
            lease_id: "l".into(),
        }
    }

    fn cfg_if_lock() -> String {
        format!(
            "version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n",
            "a".repeat(64)
        )
    }

    #[test]
    fn a_different_producer_context_is_named_as_the_miss() {
        let package = crate::seed_lock::eligible(cfg_if_lock().as_bytes())
            .unwrap()
            .remove(0);
        let source = verified_source(package.clone());
        let mut other = candidate("id", 1, "tree");
        other.catalog.producer.rustc = "/other/rustc".into();
        assert_eq!(
            select(&recipient(), &producer(), &[package], &[source], &[other],),
            Err(SelectMiss::ContextMismatch)
        );
    }

    #[test]
    fn a_join_input_outside_the_verified_root_is_named_as_the_miss() {
        let package = crate::seed_lock::eligible(cfg_if_lock().as_bytes())
            .unwrap()
            .remove(0);
        let source = verified_source(package.clone());
        for input in ["/elsewhere/src/lib.rs", "/fixture/../outside/lib.rs"] {
            let mut escaped = candidate("id", 1, "tree");
            escaped.catalog.packages[0].units[0].join.inputs = vec![input.into()];
            assert_eq!(
                select(
                    &recipient(),
                    &producer(),
                    &[package.clone()],
                    &[source.clone()],
                    &[escaped],
                ),
                Err(SelectMiss::InputRootEscape),
                "an escaping input {input} became a plan"
            );
        }
    }

    #[test]
    fn a_capped_candidate_is_discarded_while_a_valid_one_is_chosen() {
        let package = crate::seed_lock::eligible(cfg_if_lock().as_bytes())
            .unwrap()
            .remove(0);
        let source = verified_source(package.clone());
        let mut capped = candidate("capped", 3, "tree");
        capped.catalog.packages[0].units[0].join.inputs = (0..=SEED_MAX_PLAN_INPUTS)
            .map(|index| format!("/fixture/src/{index}.rs"))
            .collect();
        let valid = candidate("valid", 1, "tree");
        let selected = select(
            &recipient(),
            &producer(),
            &[package.clone()],
            &[source.clone()],
            &[capped, valid],
        )
        .expect("the valid candidate is still planned");
        assert_eq!(selected.donor.slot_id, "valid");
        // A lone over-cap candidate is a cap miss, never a truncated plan.
        let mut capped = candidate("capped", 3, "tree");
        capped.catalog.packages[0].units[0].join.inputs = (0..=SEED_MAX_PLAN_INPUTS)
            .map(|index| format!("/fixture/src/{index}.rs"))
            .collect();
        assert_eq!(
            select(&recipient(), &producer(), &[package], &[source], &[capped],),
            Err(SelectMiss::PlanCap)
        );
    }

    #[test]
    fn v1_never_selects_a_proc_macro_or_build_script_unit() {
        let lock = format!("version = 4\n[[package]]\nname = 'cfg-if'\nversion = '1'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = '{}'\n", "a".repeat(64));
        let package = crate::seed_lock::eligible(lock.as_bytes())
            .unwrap()
            .remove(0);
        let source = VerifiedSource {
            package: package.clone(),
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
        let allowed = [package];
        for role in [SeedRole::ProcMacro, SeedRole::BuildScriptCompile] {
            let mut candidate = candidate("id", 1, "tree");
            candidate.catalog.packages[0].units[0].role = role;
            assert_eq!(
                select(
                    &recipient,
                    &producer(),
                    &allowed,
                    &[source.clone()],
                    &[candidate],
                ),
                Err(SelectMiss::NoMatchingUnit),
                "v1 selected a {role:?} unit"
            );
        }
    }
}
