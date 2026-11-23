//! Cold-slot seeding: the pre-child seed attempt and the catalog a successful
//! producing generation publishes.
//!
//! One admission lifetime spans the catalog read, the effect-free plan, the
//! reserve gate, the durable intent and the pinned library transaction. The
//! seed's own start gate samples the reserve the pool will later stop a build
//! for; a removable pool re-verifies its mount and registration instead.

use super::*;
use crate::freshness::{Built, Prior};
use crate::process;
use crate::watchdog::Watchdog;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oxide_storage::lease::LeaseError;

use super::control_io::lease_error;

/// The post-Cargo source may stand in for a pre-Cargo entry only when every
/// recorded identity fact is unchanged. A newly appearing source has no
/// pre-Cargo entry and is accepted after the release-time unchanged-since check.
pub(super) fn source_identity_matches(
    previous: &crate::seed_plan::VerifiedSource,
    post: &crate::seed_plan::VerifiedSource,
) -> bool {
    previous.package == post.package
        && previous.kind == post.kind
        && previous.root == post.root
        && previous.verification_root == post.verification_root
        && previous.tree_digest == post.tree_digest
        && previous.proc_macro == post.proc_macro
}

/// Outcome of one pre-child seed attempt. An abandoned recipient must be
/// released untrusted and the command admitted normally again.
pub(crate) enum SeedRun {
    Skipped(&'static str),
    Seeded(
        /// The transaction's own receipt. The command prints its truncation
        /// note, if any, and logs the `last` document; the seed-outcome
        /// regression reads the receipt.
        oxide_storage::lease::managed::SeedReceipt,
        oxide_storage::lease::managed::SeedLast,
    ),
    /// Seeding only was abandoned after its first recipient write: the caller
    /// releases the recipient untrusted and cold-builds in a new slot. The
    /// reason is the bounded token after `seeding skipped: `.
    Abandoned(oxide_storage::lease::managed::SeedAbandonReason),
}

impl Store {
    /// Verify the locked sources inside the admission lifetime before making
    /// a plan. Source verification reads Cargo home, never the donor payload.
    #[allow(dead_code)]
    pub(crate) fn seed_verified(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        cargo_home: &Path,
        cache: &mut oxide_storage::seed_source::SourceCache,
    ) -> Result<SeedRun, String> {
        self.seed(held, producer, lock, |candidates| {
            let mut pass = crate::seed_sources::Pass::new(Duration::from_secs(3));
            let offered: Vec<_> = lock
                .eligible
                .iter()
                .filter(|entry| {
                    candidates.iter().any(|candidate| {
                        candidate.catalog.packages.iter().any(|package| {
                            package.name == entry.name
                                && package.version == entry.version
                                && Some(package.source.as_str()) == entry.source.as_deref()
                                && package.checksum == entry.checksum
                        })
                    })
                })
                .cloned()
                .collect();
            pass.locked(cargo_home, &offered, cache, Self::unix_seconds()?)
        })
    }

    /// The cache's `verified_at` is Unix **seconds**, as `seed_sources::verify_pass`
    /// and the cache record document it. A millisecond reading would be stored
    /// as a verification far in the future.
    fn unix_seconds() -> Result<u64, &'static str> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .map_err(|_| "the clock is before the Unix epoch")
    }

    /// Build and publish a catalog for a successful, still-held producing
    /// generation. The caller verifies source trees on both sides of Cargo and
    /// supplies the producing start; all catalog work is bounded and the
    /// library rechecks the generation before its atomic sidecar write.
    #[allow(clippy::too_many_arguments, dead_code)]
    pub(crate) fn publish_seed_catalog(
        &self,
        held: &Lease,
        producer: oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        before: &[crate::seed_plan::VerifiedSource],
        after: &[crate::seed_plan::VerifiedSource],
        started: SystemTime,
        started_ms: u64,
        successful: bool,
        origin: Option<&Built>,
        profile: &str,
        source_cache: &oxide_storage::seed_source::SourceCache,
    ) -> Result<(), String> {
        use oxide_storage::lease::managed::{
            held_seed_attempt, publish_seed_catalog, SeedAttemptState, SeedCatalog, SeedPackage,
            SeedRecipient, SeedSourceRoot,
        };
        if !successful || origin.is_none() || held.reused || !matches!(held.prior, Prior::Fresh) {
            return Ok(());
        }
        let recipient = SeedRecipient {
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
        };
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let state = self.load()?;
        let record = state
            .records
            .iter()
            .find(|record| record.id == held.record)
            .filter(|record| record.lease_id.as_deref() == Some(held.lease_id.as_str()))
            .ok_or("the producing record is no longer held")?;
        let attempt = held_seed_attempt(&self.pool, &recipient)
            .map_err(|error| lease_error("cannot read seed attempt", &error))?;
        if !matches!(attempt, SeedAttemptState::None | SeedAttemptState::Skipped) {
            return Ok(());
        }
        let Some(mut classifier) = crate::seed_catalog::Producer::new(&held.build_dir) else {
            return Ok(());
        };
        // The producer's adapter names the unit shapes this catalog records.
        let layout = oxide_storage::lease::seed::layout_of_adapter(&producer.adapter);
        let rejected = classifier.rejections(profile, &lock.rejected, layout);
        let mut pass = crate::seed_sources::Pass::new(Duration::from_secs(4));
        let mut packages = Vec::new();
        // The catalog is built from the POST-Cargo verified sources. A first
        // build extracts a registry or Git source only while Cargo runs, so a
        // pre-Cargo pass alone can never catalogue it (§4.1/§4.4: the closed
        // producing epoch is the release-time evidence). An entry that also
        // appeared before Cargo must still have the same kind, roots, digest
        // and macro flag; a changed source is skipped, never catalogued. A
        // newly appearing source is accepted only after the same
        // unchanged-since check below.
        for post in after {
            if !lock.eligible.iter().any(|entry| entry == &post.package) {
                continue;
            }
            if let Some(previous) = before
                .iter()
                .find(|previous| previous.package == post.package)
            {
                if !source_identity_matches(previous, post) {
                    continue;
                }
            }
            if !pass.unchanged_since(&post.verification_root, started) {
                continue;
            }
            let Some(units) = classifier.units(profile, post, layout) else {
                return Ok(());
            };
            if units.is_empty() {
                continue;
            }
            let Some(source_name) = post.package.source.clone() else {
                continue;
            };
            packages.push(SeedPackage {
                name: post.package.name.clone(),
                target: post.package.name.replace('-', "_"),
                version: post.package.version.clone(),
                source: source_name,
                checksum: post.package.checksum.clone(),
                unique: true,
                leaf: true,
                source_root: SeedSourceRoot {
                    kind: post.kind,
                    digest: post.tree_digest.clone(),
                },
                units,
            });
        }
        if packages.is_empty() {
            return Ok(());
        }
        let catalog = SeedCatalog {
            schema: oxide_storage::lease::seed::SEED_CATALOG_SCHEMA.into(),
            store_id: held.store_id.clone(),
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
            record_id: held.record,
            mark: record.last_used,
            started_at_ms: started_ms,
            finished_at_ms: process::now_ms(),
            attempt,
            producer,
            packages,
            rejected,
        };
        publish_seed_catalog(&self.pool, &catalog)
            .map_err(|error| lease_error("cannot publish seed catalog", &error))?;
        // A cache miss is safe: this sidecar is only a performance hint and
        // never a trust root. Keep a malformed or over-cap cache untouched.
        let _ =
            oxide_storage::lease::managed::write_seed_sources(&self.pool, &recipient, source_cache);
        Ok(())
    }

    /// One admission lifetime spans the catalog read, effect-free plan,
    /// reserve gate, durable intent and pinned library transaction. `held`
    /// already owns the outermost recipient record lock.
    pub(crate) fn seed(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        verify_sources: impl FnOnce(
            &[oxide_storage::lease::managed::SeedCandidate],
        )
            -> Result<Vec<crate::seed_plan::VerifiedSource>, &'static str>,
    ) -> Result<SeedRun, String> {
        self.seed_with(
            held,
            producer,
            lock,
            verify_sources,
            |pool, request, monitor| {
                oxide_storage::lease::managed::seed_monitored(pool, request, monitor)
            },
        )
    }

    pub(super) fn seed_with(
        &self,
        held: &Lease,
        producer: &oxide_storage::lease::managed::SeedProducer,
        lock: &crate::seed_lock::Analysis,
        verify_sources: impl FnOnce(
            &[oxide_storage::lease::managed::SeedCandidate],
        )
            -> Result<Vec<crate::seed_plan::VerifiedSource>, &'static str>,
        transaction: impl FnOnce(
            &Path,
            &oxide_storage::lease::managed::SeedRequest,
            &mut dyn FnMut() -> bool,
        )
            -> Result<oxide_storage::lease::managed::SeedOutcome, LeaseError>,
    ) -> Result<SeedRun, String> {
        use oxide_storage::lease::managed::{
            seed_candidates, write_seed_attempt, write_seed_last, SeedAttempt, SeedAttemptState,
            SeedBucket, SeedLast, SeedOutcome, SeedRecipient, ATTEMPT_SCHEMA,
        };
        if held.reused || !matches!(held.prior, Prior::Fresh) {
            return Ok(SeedRun::Skipped("recipient not fresh"));
        }
        if lock.eligible.is_empty() {
            return Ok(SeedRun::Skipped("no eligible locked packages"));
        }
        let recipient = SeedRecipient {
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
        };
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let state = self.load()?;
        let record = state
            .records
            .iter()
            .find(|record| record.id == held.record)
            .filter(|record| record.lease_id.as_deref() == Some(held.lease_id.as_str()))
            .ok_or("the recipient record is no longer held")?;
        let candidates = seed_candidates(&self.pool, &recipient, 8)
            .map_err(|error| lease_error("cannot inspect seed candidates", &error))?;
        if candidates.candidates.is_empty() {
            return Ok(SeedRun::Skipped("no eligible donor"));
        }
        let sources = match verify_sources(&candidates.candidates) {
            Ok(sources) => sources,
            Err(reason) => return Ok(SeedRun::Skipped(reason)),
        };
        // Selection exposes its own miss reason; the front door's user-facing
        // mapping is a separate change, so this call still reports the one
        // existing token.
        let request = match crate::seed_plan::select(
            &recipient,
            producer,
            &lock.eligible,
            &sources,
            &candidates.candidates,
        ) {
            Ok(request) => request,
            Err(_miss) => return Ok(SeedRun::Skipped("no eligible donor")),
        };
        let rejected = candidates
            .candidates
            .iter()
            .find(|candidate| candidate.slot_id == request.donor.slot_id)
            .ok_or("the selected donor disappeared")?
            .catalog
            .rejected
            .clone();
        // The seed point's start gate (§8.3). The internal pool samples the
        // host reserve that will later stop its Cargo; a removable pool
        // re-verifies its mount/volume-UID binding and the registration's
        // reserve (the existing admission rule) and then monitors the binding.
        // A command with nothing to seed never reaches here.
        let (watchdog, trigger) = match self.backend {
            Backend::DirectoryBudget => {
                let watchdog = Watchdog::for_pool(&self.payload);
                let trigger = HOST_RESERVE_BYTES + crate::watchdog::MARGIN_BYTES;
                let first = watchdog.seed_sample();
                if first.is_none_or(|bytes| bytes < trigger) {
                    return Err(format!(
                        "seed reserve stop: available space {first:?}, trigger {trigger} bytes"
                    ));
                }
                (watchdog, Some(trigger))
            }
            Backend::ApfsVolume => {
                let mount = self.seed_removable_gate()?;
                (Watchdog::for_volume(&mount), None)
            }
        };
        let mut minimum: Option<u64> = None;
        let mut attempt = SeedAttempt {
            schema: ATTEMPT_SCHEMA.to_owned(),
            store_id: held.store_id.clone(),
            slot_id: held.slot_id.clone(),
            incarnation: held.incarnation.clone(),
            lease_id: held.lease_id.clone(),
            state: SeedAttemptState::Intent,
            donor_slot: None,
            units: None,
            files: None,
            logical_bytes: None,
            catalog_digest: None,
            observation: None,
        };
        write_seed_attempt(&self.pool, &attempt)
            .map_err(|error| lease_error("cannot record seed intent", &error))?;
        self.write_origin(held.record, record.last_used, None)?;
        let mut next_sample = Instant::now() + crate::watchdog::INTERVAL;
        let mut steps = 0usize;
        let mut stopped = false;
        let mut admission_stopped = false;
        let mut monitor = || {
            if crate::budget::admission_expired() {
                admission_stopped = true;
                return true;
            }
            steps += 1;
            if steps < 64 && Instant::now() < next_sample {
                return false;
            }
            steps = 0;
            next_sample = Instant::now() + crate::watchdog::INTERVAL;
            match (trigger, watchdog.seed_sample()) {
                (Some(trigger), Some(bytes)) if bytes >= trigger => {
                    minimum = Some(minimum.map_or(bytes, |low| low.min(bytes)));
                    false
                }
                (Some(_), Some(bytes)) => {
                    minimum = Some(minimum.unwrap_or(bytes).min(bytes));
                    stopped = true;
                    true
                }
                // A removable pool has no space trigger: only a failed
                // binding (a removal or remount) stops the seed.
                (None, Some(_)) => false,
                (Some(_), None) | (None, None) => {
                    stopped = true;
                    true
                }
            }
        };
        let outcome = transaction(&self.pool, &request, &mut monitor);
        drop(monitor);
        match outcome {
            Ok(SeedOutcome::Skipped(skip)) => {
                attempt.state = SeedAttemptState::Skipped;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record seed skip", &error))?;
                Ok(SeedRun::Skipped(skip.reason.as_str()))
            }
            Ok(SeedOutcome::Abandoned(abandon)) => {
                attempt.state = SeedAttemptState::Reset;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record abandoned seed", &error))?;
                Ok(SeedRun::Abandoned(abandon.reason))
            }
            Ok(SeedOutcome::Seeded(receipt)) => {
                attempt.state = SeedAttemptState::Seeded;
                attempt.donor_slot = Some(receipt.donor_slot.clone());
                attempt.units = Some(receipt.units);
                attempt.files = Some(receipt.files);
                attempt.logical_bytes = Some(receipt.logical_bytes);
                attempt.catalog_digest = Some(receipt.catalog_digest.clone());
                attempt.observation = Some(receipt.observation.clone());
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record completed seed", &error))?;
                let mut buckets = std::collections::BTreeMap::new();
                let mut unknown = 0u32;
                for rejection in rejected {
                    if let Some(count) = rejection.units {
                        let current: &mut u32 = buckets.entry(rejection.reason).or_default();
                        *current = current
                            .checked_add(count)
                            .ok_or("seed rejection count overflow")?;
                    } else {
                        unknown = unknown
                            .checked_add(1)
                            .ok_or("seed rejection count overflow")?;
                    }
                }
                let last = SeedLast {
                    schema: "oxide.cold-seed/1".into(),
                    store_id: held.store_id.clone(),
                    slot_id: held.slot_id.clone(),
                    incarnation: held.incarnation.clone(),
                    lease_id: held.lease_id.clone(),
                    donor_slot: receipt.donor_slot.clone(),
                    units: receipt.units,
                    files: receipt.files,
                    logical_bytes: receipt.logical_bytes,
                    elapsed_ms: receipt.elapsed_ms,
                    when_ms: process::now_ms(),
                    buckets: buckets
                        .into_iter()
                        .filter(|(_, units)| *units > 0)
                        .map(|(reason, units)| SeedBucket { reason, units })
                        .collect(),
                    unknown,
                };
                write_seed_last(&self.pool, &last)
                    .map_err(|error| lease_error("cannot record last seed", &error))?;
                Ok(SeedRun::Seeded(receipt, last))
            }
            Err(error) => {
                attempt.state = SeedAttemptState::Reset;
                write_seed_attempt(&self.pool, &attempt)
                    .map_err(|error| lease_error("cannot record stopped seed", &error))?;
                if stopped {
                    let minimum =
                        minimum.map_or_else(|| "unknown".to_owned(), |bytes| bytes.to_string());
                    return match trigger {
                        Some(trigger) => Err(format!(
                            "seed reserve stop: minimum {minimum} bytes, trigger {trigger} bytes"
                        )),
                        None => Err(format!(
                            "seed volume stop: the removable volume under {} is gone or was remounted",
                            self.payload.display()
                        )),
                    };
                }
                if admission_stopped {
                    return Err("seed admission deadline expired".to_owned());
                }
                Err(lease_error("cannot seed slot", &error))
            }
        }
    }

    /// The seed's removable start gate: the registration read from this
    /// removable pool's paired internal control, re-verified for its
    /// mount/volume-UID binding and its free-space reserve (§8.3).
    pub(super) fn seed_removable_gate(&self) -> Result<PathBuf, String> {
        let sibling = self
            .sibling
            .as_ref()
            .ok_or("the removable pool has no paired internal control")?;
        let registration = crate::removable::read_registration(&sibling.control)?
            .ok_or("the removable pool has no registration")?;
        crate::removable::verified_registration(&self.control, &registration)?;
        Ok(registration.mount_point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the seed cache recorded `process::now_ms()` where the cache
    /// record and `seed_sources::verify_pass` both mean Unix **seconds**. A
    /// millisecond reading lands about a thousand times in the future.
    #[test]
    fn source_verification_time_is_unix_seconds_not_milliseconds() {
        let now = Store::unix_seconds().expect("the clock is after the epoch");
        assert!(
            (1_700_000_000..4_000_000_000).contains(&now),
            "{now} is not a plausible Unix-second reading"
        );
    }
}
