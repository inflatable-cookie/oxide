//! The owned-outputs initialization protocol (oxide#129) and its fences.
//!
//! The internal control is durable first, then the paired removable control,
//! then the registry itself, then both controls record `ready`. Writer closure
//! across both pools is checked before anything is published, and a crash
//! resumes only the recorded transaction.

use super::*;
use crate::process;
use std::path::Path;

use super::recovery::{boot_session, closed_by_boot};

/// Record `transaction` as the paired control's ready registry and publish the
/// successor fence, after that control's writers are positively closed. The
/// caller holds the paired pool's admission lock and has already checked any
/// predecessor fence it accepted.
pub(super) fn fence_paired(
    sibling: &Store,
    state: &mut State,
    transaction: &str,
    own_checkout: Option<&Path>,
) -> Result<(), String> {
    writers_closed(state, "the paired", own_checkout)?;
    state.outputs_init = Some(OutputsInit::Ready {
        transaction: transaction.to_owned(),
        ready_ms: process::now_ms(),
    });
    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
    sibling.save_for_writer(state)
}

/// Refuse an owned-outputs fence that is neither the successor nor an accepted
/// predecessor. An absent fence is not refused here; each caller decides what
/// absence means for it.
fn check_predecessor_fence(owned_outputs: Option<&str>, holder: &str) -> Result<(), String> {
    if owned_outputs.is_some_and(|fence| !predecessor_owned_outputs_fence(fence)) {
        return Err(format!(
            "{holder} carries an unknown owned-outputs fence; refusing"
        ));
    }
    Ok(())
}

/// Positive writer closure in one pool's state for owned-outputs
/// initialization (see [`Store::initialize_outputs`]). Anything that is not
/// positively idle, closed or the caller's own barrier refuses.
pub(super) fn writers_closed(
    state: &State,
    pool: &str,
    own_checkout: Option<&Path>,
) -> Result<(), String> {
    let boot = boot_session();
    for record in &state.records {
        if record.active() {
            return Err(format!(
                "{pool} pool holds an active command record (record {}); the shared output format is initialized only after every command in both pools closes",
                record.id
            ));
        }
        if closed_by_boot(record, boot) {
            // A restart ended the quarantined writers; the slot is released
            // and deleted whole at the next admission.
            continue;
        }
        if let Some(why) = record
            .lost
            .as_ref()
            .and_then(|lost| lost.unclosed.as_deref())
        {
            return Err(format!(
                "{pool} pool has a lost slot whose writers were never confirmed closed ({why}); the shared output format is not initialized until that is resolved"
            ));
        }
    }
    if let Some(clean) = state
        .cleans
        .iter()
        .find(|clean| Some(clean.checkout.as_path()) != own_checkout)
    {
        return Err(format!(
            "{pool} pool holds a clean barrier for {}; the shared output format is initialized only after it closes",
            clean.checkout.display()
        ));
    }
    Ok(())
}

impl Store {
    /// Publish the owned-outputs downgrade fence in this pool's state and in
    /// its paired pool's (internal first), creating either state when absent
    /// so a pre-feature Oxide cannot start from a fresh unfenced one. Test
    /// fixtures only: production initialization is [`Store::initialize_outputs`].
    #[cfg(test)]
    pub fn fence_owned_outputs(&self) -> Result<(), String> {
        let mut controls = vec![self.owner_control().to_path_buf()];
        controls.extend(self.sibling.as_ref().map(|sibling| sibling.control.clone()));
        for control in controls {
            let store = Store {
                pool: self.pool.clone(),
                backend: self.backend,
                payload: self.payload.clone(),
                control,
                budget: self.budget,
                recorded_budget: self.recorded_budget,
                max_active: self.max_active,
                sibling: None,
            };
            store.ensure_control()?;
            let _admission = store.admission()?;
            let mut state = store.load()?;
            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                check_predecessor_fence(
                    state.owned_outputs.as_deref(),
                    &format!("the Oxide state in {}", store.control.display()),
                )?;
                if state.outputs_init.is_some() {
                    return Err(format!(
                        "Oxide state in {} already records an output-registry transaction but not the successor fence; refusing",
                        store.control.display()
                    ));
                }
                state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                store.save(&state)?;
            }
        }
        Ok(())
    }

    /// The recorded owned-outputs registry transaction, when initialization
    /// has begun. Read without a lock; callers that mutate recheck it under
    /// admission.
    pub fn outputs_initialization(&self) -> Result<Option<OutputsInit>, String> {
        Ok(self.load()?.outputs_init)
    }

    /// Run the owned-outputs initialization protocol exactly once
    /// (oxide#129). The internal control is durable first
    /// (`initializing` + the successor `oxide.outputs/3` fence), then the
    /// paired removable control, then `create(transaction)` creates and syncs
    /// the registry,
    /// then both controls record `ready`. A crash resumes only the recorded
    /// transaction; an unknown or contradicting record refuses before any
    /// registry effect.
    ///
    /// Initialization (and resuming an `initializing` record) first requires
    /// positive pool-wide writer closure under both admission locks, held
    /// together: no active or held command record and no lost slot with
    /// unclosed writers in either pool, and no clean barrier except the
    /// caller's own `own_checkout`. An older binary's in-flight command is a
    /// record in the same state, so it must close before the fence is
    /// published.
    pub fn initialize_outputs(
        &self,
        transaction: &str,
        own_checkout: Option<&Path>,
        create: &dyn Fn(&str, bool) -> Result<(), String>,
    ) -> Result<bool, String> {
        if !bounded_text(transaction, MAX_INIT_TRANSACTION_BYTES) {
            return Err("the output-registry transaction identity is unusable".to_owned());
        }
        let internal_control = self.owner_control().to_path_buf();
        let internal = self.view(&internal_control);
        internal.ensure_control()?;
        let _internal_admission = internal.control_admission()?;
        let internal_state = internal.load()?;
        // A recorded `ready` registry is validated, never rebuilt; an
        // `initializing` one resumes its exact transaction; a fresh control
        // records the caller's transaction before any effect.
        let (transaction, fresh) = match internal_state.outputs_init.clone() {
            Some(OutputsInit::Ready {
                transaction: recorded,
                ..
            }) => {
                self.resume_ready_outputs(&internal, &internal_control, &recorded, own_checkout)?;
                // The registry must already exist: this path is validation
                // only and must never create or rebuild it.
                create(&recorded, false)?;
                return Ok(true);
            }
            Some(OutputsInit::Initializing { transaction, .. }) => (transaction, false),
            None => (transaction.to_owned(), true),
        };
        self.publish_initializing(
            &internal,
            &internal_control,
            &internal_state,
            &transaction,
            fresh,
            own_checkout,
        )?;
        create(&transaction, true)?;
        self.record_ready_outputs(&internal, &internal_control, &transaction)?;
        Ok(true)
    }

    /// The paired control's own store view, when this pool is registered with
    /// a different control directory. `barrier` marks the removable store,
    /// whose attempts hold the internal admission first.
    pub(super) fn paired_view(&self, internal_control: &Path) -> Option<(Store, bool)> {
        self.sibling
            .as_ref()
            .filter(|sibling| sibling.control != internal_control)
            .map(|sibling| (self.view(&sibling.control), sibling.barrier))
    }

    /// Validate an already recorded ready registry: the internal control is
    /// upgraded to the successor fence under positive writer closure, and the
    /// paired control must record the same transaction and carry the fence
    /// before the registry is re-verified. The caller holds the internal
    /// admission.
    pub(super) fn resume_ready_outputs(
        &self,
        internal: &Store,
        internal_control: &Path,
        recorded: &str,
        own_checkout: Option<&Path>,
    ) -> Result<(), String> {
        let mut internal_state = internal.load()?;
        if internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
            // The published whole-target fence is upgraded to the
            // successor only with positive writer closure across both
            // pools, internal fence first (oxide#133).
            self.upgrade_outputs_fence(own_checkout, recorded)?;
            internal_state = internal.load()?;
            if internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                return Err(
                    "the internal control records a ready output registry without the successor fence; refusing"
                        .to_owned(),
                );
            }
        }
        // A new or reappearing paired control is fenced and marked ready
        // under the exact recorded transaction before any effect; a
        // different or unknown record refuses.
        let Some((sibling, _barrier)) = self.paired_view(internal_control) else {
            return Ok(());
        };
        sibling.ensure_control()?;
        let _admission = sibling.control_admission()?;
        let mut state = sibling.load()?;
        match &state.outputs_init {
            Some(OutputsInit::Ready { transaction, .. }) if transaction == recorded => {
                if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                    check_predecessor_fence(state.owned_outputs.as_deref(), "the paired pool")?;
                    // The internal control may already carry the successor
                    // fence (a crash between the two saves), so this paired
                    // upgrade is the only remaining closure check: it must
                    // hold here too, not only in the first upgrade.
                    writers_closed(&state, "the paired", own_checkout)?;
                    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                    sibling.save_for_writer(&state)?;
                }
            }
            Some(OutputsInit::Ready { .. }) => {
                return Err(
                    "the paired pool records a different ready output registry; refusing"
                        .to_owned(),
                )
            }
            Some(OutputsInit::Initializing { transaction, .. }) => {
                if transaction.as_str() != recorded {
                    return Err(format!(
                        "the paired pool records initializer transaction {transaction}, not the recorded {recorded}; refusing"
                    ));
                }
                fence_paired(&sibling, &mut state, recorded, own_checkout)?;
            }
            None => {
                // Parity with the `Ready` arm above: the released `/1` fence
                // is a legitimate predecessor here too, not an unknown value
                // (oxide#159).
                check_predecessor_fence(state.owned_outputs.as_deref(), "the paired pool")?;
                // A new or reappearing control is fenced only with its own
                // writers positively closed.
                fence_paired(&sibling, &mut state, recorded, own_checkout)?;
            }
        }
        if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
            return Err(
                "the paired pool does not carry the successor output fence; refusing".to_owned(),
            );
        }
        Ok(())
    }

    /// Record the transaction as `initializing` in the internal control, then
    /// in the paired one, both under their admission locks and after positive
    /// writer closure in both pools.
    pub(super) fn publish_initializing(
        &self,
        internal: &Store,
        internal_control: &Path,
        internal_state: &State,
        transaction: &str,
        fresh: bool,
        own_checkout: Option<&Path>,
    ) -> Result<(), String> {
        // Both admission locks are held together (internal first) from the
        // closure check through both `initializing` records.
        let sibling = self.paired_view(internal_control);
        let mut sibling_admission = None;
        if let Some((sibling, _barrier)) = &sibling {
            sibling.ensure_control()?;
            sibling_admission = Some(sibling.control_admission()?);
            writers_closed(&sibling.load()?, "the paired", own_checkout)?;
        }
        writers_closed(internal_state, "the internal", own_checkout)?;
        let mut internal_state = internal_state.clone();
        if !fresh && internal_state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save_for_writer(&internal_state)?;
        }
        if fresh {
            internal_state.outputs_init = Some(OutputsInit::Initializing {
                transaction: transaction.to_owned(),
                started_ms: process::now_ms(),
            });
            internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            internal.save_for_writer(&internal_state)?;
        }
        internal.fence_admission_state()?;
        // Internal-first: the paired removable control is fenced only after
        // the internal record is durable.
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling_admission.take();
            let mut state = sibling.load()?;
            match &state.outputs_init {
                Some(OutputsInit::Ready {
                    transaction: ready,
                    ..
                }) if ready.as_str() == transaction => {}
                Some(OutputsInit::Ready { .. }) => {
                    return Err(
                        "the paired pool records a different ready output registry; refusing"
                            .to_owned(),
                    )
                }
                Some(OutputsInit::Initializing { transaction: other, .. })
                    if other.as_str() != transaction =>
                {
                    return Err(format!(
                        "the paired pool records initializer transaction {other}, not {transaction}; refusing"
                    ))
                }
                Some(OutputsInit::Initializing { .. }) => {}
                None => {
                    state.outputs_init = Some(OutputsInit::Initializing {
                        transaction: transaction.to_owned(),
                        started_ms: process::now_ms(),
                    });
                    state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
                    sibling.save_for_writer(&state)?;
                }
            }
            if state.owned_outputs.as_deref() != Some(OWNED_OUTPUTS_FENCE) {
                return Err(
                    "the paired pool does not carry the successor output fence; refusing"
                        .to_owned(),
                );
            }
            sibling.fence_admission_state()?;
        }
        Ok(())
    }

    /// Record the registry `ready` in the internal control, then in the paired
    /// one. The registry already exists and was synced before this runs.
    pub(super) fn record_ready_outputs(
        &self,
        internal: &Store,
        internal_control: &Path,
        transaction: &str,
    ) -> Result<(), String> {
        let mut internal_state = internal.load()?;
        if let Some(OutputsInit::Initializing { .. }) = internal_state.outputs_init {
            internal_state.outputs_init = Some(OutputsInit::Ready {
                transaction: transaction.to_owned(),
                ready_ms: process::now_ms(),
            });
            internal.save_for_writer(&internal_state)?;
        }
        let Some((sibling, _barrier)) = self.paired_view(internal_control) else {
            return Ok(());
        };
        let _admission = sibling.control_admission()?;
        let mut state = sibling.load()?;
        if !matches!(
            &state.outputs_init,
            Some(OutputsInit::Ready { transaction: ready, .. }) if ready.as_str() == transaction
        ) {
            state.outputs_init = Some(OutputsInit::Ready {
                transaction: transaction.to_owned(),
                ready_ms: process::now_ms(),
            });
            sibling.save_for_writer(&state)?;
        }
        Ok(())
    }

    /// Upgrade the published whole-target fence to the successor, internal
    /// control first (oxide#133). The caller holds the internal admission and
    /// is inside its writer-closure window; this takes the paired pool's
    /// admission so both are held together, and a missing, unreadable or
    /// contradicting authority refuses and changes nothing.
    pub(super) fn upgrade_outputs_fence(
        &self,
        own_checkout: Option<&Path>,
        transaction: &str,
    ) -> Result<(), String> {
        let internal_control = self.owner_control().to_path_buf();
        let internal = self.view(&internal_control);
        let mut internal_state = internal.load()?;
        if internal_state.owned_outputs.as_deref() == Some(OWNED_OUTPUTS_FENCE) {
            return Ok(());
        }
        match internal_state.owned_outputs.as_deref() {
            Some(fence) if predecessor_owned_outputs_fence(fence) => {}
            None => {
                return Err(
                    "the internal control records a ready output registry without an owned-outputs fence; refusing"
                        .to_owned(),
                )
            }
            Some(_) => {
                return Err(
                    "the internal control carries an unknown owned-outputs fence; refusing"
                        .to_owned(),
                )
            }
        }
        if !matches!(
            &internal_state.outputs_init,
            Some(OutputsInit::Ready { transaction: ready, .. }) if ready == transaction
        ) {
            return Err(
                "the recorded output-registry transaction changed while upgrading the fence; refusing"
                    .to_owned(),
            );
        }
        writers_closed(&internal_state, "the internal", own_checkout)?;
        let sibling = self.paired_view(&internal_control);
        let mut sibling_admission = None;
        if let Some((sibling, _barrier)) = &sibling {
            sibling.ensure_control()?;
            sibling_admission = Some(sibling.control_admission()?);
            let state = sibling.load()?;
            writers_closed(&state, "the paired", own_checkout)?;
            match state.outputs_init.as_ref() {
                Some(OutputsInit::Ready {
                    transaction: ready, ..
                }) if ready == transaction => {}
                Some(_) => {
                    return Err(
                        "the paired pool records a different output-registry transaction; refusing"
                            .to_owned(),
                    )
                }
                None => match state.owned_outputs.as_deref() {
                    None => {}
                    Some(fence) if predecessor_owned_outputs_fence(fence) => {}
                    Some(_) => {
                        return Err(
                            "the paired pool carries an unknown owned-outputs fence; refusing"
                                .to_owned(),
                        )
                    }
                },
            }
        }
        internal_state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
        internal.save_for_writer(&internal_state)?;
        if let Some((sibling, _barrier)) = &sibling {
            let _admission = sibling_admission.take();
            let mut state = sibling.load()?;
            state.owned_outputs = Some(OWNED_OUTPUTS_FENCE.to_owned());
            if state.outputs_init.is_none() {
                state.outputs_init = Some(OutputsInit::Ready {
                    transaction: transaction.to_owned(),
                    ready_ms: process::now_ms(),
                });
            }
            sibling.save_for_writer(&state)?;
        }
        Ok(())
    }
}
