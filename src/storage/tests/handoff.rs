use super::*;
use sha2::{Digest, Sha256};

const HANDOFF_INVALIDATION_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn fixture_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    format!("sha256:{hex}")
}

fn handoff_policy_request(
    source: &Path,
    attempt: &str,
    allocation_epoch: &str,
    acquire_id: &str,
) -> OrdinaryCargoRequest {
    let mut request = ordinary_request(
        source,
        attempt,
        acquire_id,
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    request.acquire.envelope.binding.task_id = format!("task-{attempt}");
    request.acquire.envelope.binding.workspace_id = format!("workspace-{attempt}");
    request.acquire.envelope.binding.run_id = format!("run-{attempt}");
    request.acquire.envelope.binding.attempt_id = attempt.to_owned();
    request.root.consumer_root_id = "consumer-root".to_owned();
    request.root.allocation_epoch = allocation_epoch.to_owned();
    request.acceptance.policy_version = ORDINARY_CARGO_HANDOFF_ELIGIBLE_POLICY_VERSION;
    request.acceptance.installation_epoch = "install-1".to_owned();
    request.acceptance.credential = Credential::new("ordinary-proof");
    request.acceptance.input_boundary_digest =
        fixture_hex(format!("handoff boundary {attempt} {allocation_epoch}").as_bytes());
    let reference = ordinary_cargo_stable_reuse_reference(&request, HANDOFF_INVALIDATION_DIGEST)
        .expect("typed v3 stable reference");
    request.acceptance.acceptance_reference = reference;
    request
}

fn handoff_transition(
    prior_attempt: &str,
    successor_attempt: &str,
    mtime_qualified: bool,
) -> OrdinaryCargoSourceTransition {
    OrdinaryCargoSourceTransition {
        prior_commit: format!("prior-commit-{prior_attempt}"),
        prior_tree_digest: fixture_hex(format!("prior tree {prior_attempt}").as_bytes()),
        prior_content_digest: fixture_hex(format!("prior content {prior_attempt}").as_bytes()),
        successor_commit: format!("successor-commit-{successor_attempt}"),
        successor_tree_digest: fixture_hex(
            format!("successor tree {successor_attempt}").as_bytes(),
        ),
        successor_content_digest: fixture_hex(
            format!("successor content {successor_attempt}").as_bytes(),
        ),
        transition_reference: format!("transition-{prior_attempt}-to-{successor_attempt}"),
        invalidation_declaration_digest: HANDOFF_INVALIDATION_DIGEST.to_owned(),
        mtime_qualified,
        qualified_input_classes: vec![
            "manifest".to_owned(),
            "lock".to_owned(),
            "config".to_owned(),
            "generated".to_owned(),
            "build-script".to_owned(),
            "proc-macro".to_owned(),
            "native".to_owned(),
            "dependency".to_owned(),
        ],
    }
}

fn handoff_request(
    prior: OrdinaryCargoRequest,
    successor: OrdinaryCargoRequest,
    slot_id: &str,
    storage_instance_id: &str,
    generation: &str,
    handoff_id: &str,
) -> OrdinaryCargoHandoffRequest {
    let prior_epoch = prior.root.allocation_epoch.clone();
    let successor_epoch = successor.root.allocation_epoch.clone();
    OrdinaryCargoHandoffRequest {
        prior_request: prior,
        successor_request: successor,
        slot_id: slot_id.to_owned(),
        storage_instance_id: storage_instance_id.to_owned(),
        generation: generation.to_owned(),
        handoff_id: handoff_id.to_owned(),
        handoff_version: ORDINARY_CARGO_HANDOFF_VERSION,
        retention: retention_grant(),
        source_transition: handoff_transition("a", "b", true),
        prior_closure_reference: format!("closed-{prior_epoch}"),
        successor_assignment_reference: format!("assigned-{successor_epoch}"),
        credential: Credential::new("handoff-proof"),
    }
}

struct OpenHandoffAuthority;

impl EvidenceAuthority for OpenHandoffAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference == "admission-ref"
            && envelope.admission_credential.expose() == "admission-proof"
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        TestAuthority.verify_settlement(evidence)
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        request.acceptance.installation_epoch == "install-1"
            && request.acceptance.credential.expose() == "ordinary-proof"
            && request.root.consumer_root_id == "consumer-root"
            && valid_handoff_policy(request)
    }

    fn verify_ordinary_cargo_stable_reuse_policy(&self, request: &OrdinaryCargoRequest) -> bool {
        if !valid_handoff_policy(request) {
            return false;
        }
        let Some(invalidation) = invalidation_for(&request.acceptance.acceptance_reference) else {
            return false;
        };
        ordinary_cargo_stable_reuse_reference(request, invalidation)
            .is_ok_and(|expected| expected == request.acceptance.acceptance_reference)
    }

    fn verify_ordinary_cargo_settlement(
        &self,
        request: &OrdinaryCargoRequest,
        evidence: &SettlementEvidence,
    ) -> bool {
        self.verify_ordinary_cargo_admission(request)
            && self.verify_ordinary_cargo_stable_reuse_policy(request)
            && self.verify_settlement(evidence)
            && evidence.binding == request.acquire.envelope.binding
    }

    fn verify_ordinary_cargo_retirement(&self, request: &OrdinaryCargoRetireRequest) -> bool {
        self.verify_ordinary_cargo_admission(&request.request)
            && self.verify_ordinary_cargo_stable_reuse_policy(&request.request)
            && request.retention.source_evidence_reference == "retained-source-v1"
            && request.retention.artifact_evidence_reference == "retained-artifact-v1"
            && request.retention.debug_evidence_reference == "retained-debug-v1"
            && request.retention.credential.expose() == "retention-proof"
    }

    fn verify_ordinary_cargo_handoff(&self, request: &OrdinaryCargoHandoffRequest) -> bool {
        if request.handoff_version != ORDINARY_CARGO_HANDOFF_VERSION
            || request.credential.expose() != "handoff-proof"
        {
            return false;
        }
        if !self.verify_ordinary_cargo_admission(&request.prior_request)
            || !self.verify_ordinary_cargo_admission(&request.successor_request)
            || !self.verify_ordinary_cargo_stable_reuse_policy(&request.prior_request)
            || !self.verify_ordinary_cargo_stable_reuse_policy(&request.successor_request)
        {
            return false;
        }
        if request.retention.source_evidence_reference != "retained-source-v1"
            || request.retention.artifact_evidence_reference != "retained-artifact-v1"
            || request.retention.debug_evidence_reference != "retained-debug-v1"
            || request.retention.credential.expose() != "retention-proof"
        {
            return false;
        }
        let transition = &request.source_transition;
        if transition.invalidation_declaration_digest != HANDOFF_INVALIDATION_DIGEST
            || !transition.mtime_qualified
            || transition.transition_reference.is_empty()
        {
            return false;
        }
        for required in [
            "manifest",
            "lock",
            "config",
            "generated",
            "build-script",
            "proc-macro",
            "native",
            "dependency",
        ] {
            if !transition
                .qualified_input_classes
                .contains(&required.to_owned())
            {
                return false;
            }
        }
        request.prior_closure_reference
            == format!("closed-{}", request.prior_request.root.allocation_epoch)
            && request.successor_assignment_reference
                == format!(
                    "assigned-{}",
                    request.successor_request.root.allocation_epoch
                )
    }
}

fn valid_handoff_policy(request: &OrdinaryCargoRequest) -> bool {
    request.acceptance.policy_version == ORDINARY_CARGO_HANDOFF_ELIGIBLE_POLICY_VERSION
        && valid_digest_shape(&request.acceptance.input_boundary_digest)
}

fn valid_digest_shape(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn invalidation_for(reference: &str) -> Option<&str> {
    let identity = reference
        .strip_prefix("oxide-ordinary-cargo-stable-eligibility-v2/")
        .or_else(|| reference.strip_prefix("oxide-ordinary-cargo-stable-eligibility-v3/"))?;
    let (invalidation, compatibility) = identity.split_once('/')?;
    if valid_digest_shape(invalidation) && valid_digest_shape(compatibility) {
        Some(invalidation)
    } else {
        None
    }
}

fn seal_reusable(
    client: &Client,
    request: &OrdinaryCargoRequest,
    acquired: &OrdinaryCargoAcquireResult,
    finalize_id: &str,
) {
    let token = acquired.lease.token.as_ref().expect("active lease token");
    client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: request.clone(),
            lease_id: acquired.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: finalize_id.to_owned(),
            evidence: passing_evidence(
                &request.acquire.envelope.binding,
                &acquired.lease.lease_id,
                &acquired.lease.slot_id,
                acquired.lease.slot_path.as_ref().expect("slot path"),
                token,
            ),
        })
        .expect("publish reusable generation");
}

#[test]
fn handoff_preserves_generation_across_allocations() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(256), Some(1024), authority);

    let prior = handoff_policy_request(source.path(), "prior-a", "allocation-a", "acquire-a");
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(prior.clone())
        .expect("cold prior allocation acquires");
    assert!(!acquired.lease.reused);
    let slot_path = acquired.lease.slot_path.clone().expect("slot path");
    std::fs::write(slot_path.join("payload.bin"), b"ordinary-bytes-v1")
        .expect("write synthetic payload under owned slot");
    seal_reusable(&daemon.client, &prior, &acquired, "finalize-prior-a");
    let instance = acquired.lease.storage_instance_id.clone();
    let slot_id = acquired.lease.slot_id.clone();
    let generation = acquired.lease.generation.clone();

    let successor =
        handoff_policy_request(source.path(), "successor-b", "allocation-b", "acquire-b");
    let request = handoff_request(
        prior.clone(),
        successor.clone(),
        &slot_id,
        &instance,
        &generation,
        "handoff-a-b",
    );
    let receipt = daemon
        .client
        .handoff_ordinary_cargo(request.clone())
        .expect("owner-authenticated handoff commits");
    assert_eq!(receipt.slot_id, slot_id);
    assert_eq!(receipt.generation, generation);
    assert_eq!(receipt.storage_instance_id, instance);
    assert_eq!(receipt.prior_root.allocation_epoch, "allocation-a");
    assert_eq!(receipt.successor_root.allocation_epoch, "allocation-b");
    assert_eq!(
        std::fs::read(slot_path.join("payload.bin")).expect("read preserved payload"),
        b"ordinary-bytes-v1"
    );

    // Lookup replays the same receipt without allocation or access.
    let lookup = daemon
        .client
        .lookup_handoff_ordinary_cargo(request.clone())
        .expect("handoff lookup");
    assert!(matches!(lookup, LookupResult::Present(found) if found == receipt));
    // Exact replay returns the stored receipt.
    assert_eq!(
        daemon
            .client
            .handoff_ordinary_cargo(request.clone())
            .expect("exact handoff replay"),
        receipt
    );

    // Successor acquires a fresh exclusive lease against the new allocation.
    let next = daemon
        .client
        .acquire_ordinary_cargo(successor.clone())
        .expect("successor acquires handed-off generation");
    assert!(next.lease.reused);
    assert_eq!(next.lease.slot_id, slot_id);
    assert_eq!(next.lease.generation, generation);
    assert_eq!(
        std::fs::read(
            next.lease
                .slot_path
                .as_ref()
                .expect("successor slot path")
                .join("payload.bin")
        )
        .expect("successor sees preserved bytes"),
        b"ordinary-bytes-v1"
    );
    let next_token = next.lease.token.clone().expect("successor token");
    seal_reusable(&daemon.client, &successor, &next, "finalize-successor-b");
    drop(next_token);

    // Old exact prior replay stays access-free after ownership changes.
    let replay = daemon
        .client
        .acquire_ordinary_cargo(prior.clone())
        .expect("old exact replay");
    assert_eq!(replay.lease.slot_id, slot_id);
    assert!(replay.lease.slot_path.is_none());
    assert!(replay.lease.token.is_none());

    daemon.stop();
    // Restart converges to the successor allocation with old history intact.
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(256), Some(1024), authority);
    assert!(matches!(
        daemon
            .client
            .lookup_handoff_ordinary_cargo(request)
            .expect("handoff survives restart"),
        LookupResult::Present(_)
    ));
    let after = daemon
        .client
        .acquire_ordinary_cargo(successor.clone())
        .expect("successor replay after restart");
    assert_eq!(after.lease.slot_id, slot_id);
    // Fresh successor attempt with a new run still reuses the handed-off bytes.
    let mut fresh = successor.clone();
    fresh.acquire.envelope.binding.task_id = "task-fresh".to_owned();
    fresh.acquire.envelope.binding.run_id = "run-fresh".to_owned();
    fresh.acquire.envelope.binding.attempt_id = "fresh".to_owned();
    fresh.acquire.acquire_id = "acquire-fresh".to_owned();
    fresh.acquire.envelope.admission_credential = Credential::new("admission-proof");
    fresh.acceptance.input_boundary_digest = fixture_hex(b"handoff boundary fresh allocation-b");
    let fresh_reference =
        ordinary_cargo_stable_reuse_reference(&fresh, HANDOFF_INVALIDATION_DIGEST)
            .expect("fresh stable reference");
    fresh.acceptance.acceptance_reference = fresh_reference;
    // The open test authority allowlists exact admission shape only through
    // recomputation, so this fresh exact attempt is accepted once its
    // reference is installed correctly.
    let fresh_acquired = daemon
        .client
        .acquire_ordinary_cargo(fresh.clone())
        .expect("fresh successor attempt reuses handoff");
    assert!(fresh_acquired.lease.reused);
    let fresh_token = fresh_acquired.lease.token.clone().expect("fresh token");
    daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: fresh.clone(),
            lease_id: fresh_acquired.lease.lease_id.clone(),
            token: Some(fresh_token.clone()),
            finalize_id: "finalize-fresh".to_owned(),
            evidence: passing_evidence(
                &fresh.acquire.envelope.binding,
                &fresh_acquired.lease.lease_id,
                &fresh_acquired.lease.slot_id,
                fresh_acquired.lease.slot_path.as_ref().expect("fresh path"),
                &fresh_token,
            ),
        })
        .expect("fresh successor finalizes");
    // Retirement works against the current allocation with old history intact.
    let retire = OrdinaryCargoRetireRequest {
        request: successor.clone(),
        slot_id: slot_id.clone(),
        storage_instance_id: instance.clone(),
        generation: generation.clone(),
        retire_id: "retire-after-handoff".to_owned(),
        retention: retention_grant(),
    };
    daemon
        .client
        .retire_ordinary_cargo(retire)
        .expect("retire handed-off generation");
    // A replacement for the same root/partition receives a new ordinal.
    let mut cold = handoff_policy_request(source.path(), "cold-c", "allocation-b", "acquire-c");
    cold.acquire.envelope.binding.task_id = "task-cold".to_owned();
    let cold_acquired = daemon
        .client
        .acquire_ordinary_cargo(cold.clone())
        .expect("fresh ordinal after retirement");
    assert_ne!(cold_acquired.lease.slot_id, slot_id);
    daemon.stop();
}

#[test]
fn handoff_refuses_without_authority_compat_or_qualified_transition() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let other_source = TaskRoot::new();
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(512), Some(2048), authority);

    let prior = handoff_policy_request(source.path(), "prior-a", "allocation-a", "acquire-a");
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(prior.clone())
        .expect("cold prior acquires");
    seal_reusable(&daemon.client, &prior, &acquired, "finalize-prior-a");
    let good_successor =
        handoff_policy_request(source.path(), "successor-b", "allocation-b", "acquire-b");
    let good = handoff_request(
        prior.clone(),
        good_successor.clone(),
        &acquired.lease.slot_id,
        &acquired.lease.storage_instance_id,
        &acquired.lease.generation,
        "handoff-good",
    );

    // Default-deny: unknown credentials hold.
    let mut denied = good.clone();
    denied.credential = Credential::new("forged-proof");
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(denied),
        Err(StorageError::Unauthorized)
    );
    // No-policy installation: an uninstalled stable eligibility reference holds.
    let mut no_policy = good.clone();
    no_policy.handoff_id = "handoff-no-policy".to_owned();
    no_policy.successor_request.acceptance.acceptance_reference =
        ordinary_cargo_stable_reuse_reference(
            &no_policy.successor_request,
            "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        )
        .expect("uninstalled reference");
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(no_policy),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
    // Old versions refuse explicitly rather than migrating.
    for old in [
        ordinary_request(
            source.path(),
            "old-v1",
            "acquire-old-v1",
            64,
            OrdinaryCargoPartition::OrdinaryTest,
        ),
        {
            let mut v2 =
                handoff_policy_request(source.path(), "old-v2", "allocation-old", "acquire-old-v2");
            v2.acceptance.policy_version = ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION;
            let reference = ordinary_cargo_stable_reuse_reference(&v2, HANDOFF_INVALIDATION_DIGEST)
                .expect("v2 reference");
            v2.acceptance.acceptance_reference = reference;
            v2
        },
    ] {
        let mut old_handoff = good.clone();
        old_handoff.prior_request = old;
        // Old generations refuse as incompatible rather than migrating to v3.
        assert_eq!(
            daemon.client.handoff_ordinary_cargo(old_handoff),
            Err(StorageError::Refused(
                RefusalReason::OrdinaryCargoIncompatible
            ))
        );
    }
    // A legacy v1 generation itself is incompatible even with valid handoff auth shape.
    let legacy_storage = TaskRoot::new();
    let legacy_source = TaskRoot::new();
    let mut legacy_daemon = RunningDaemon::start(legacy_storage.path(), budget(256), Some(1024));
    let legacy_prior = ordinary_request(
        legacy_source.path(),
        "legacy-prior",
        "acquire-legacy",
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    let legacy_acquired = legacy_daemon
        .client
        .acquire_ordinary_cargo(legacy_prior.clone())
        .expect("legacy v1 acquires");
    let legacy_token = legacy_acquired.lease.token.clone().expect("legacy token");
    legacy_daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: legacy_prior.clone(),
            lease_id: legacy_acquired.lease.lease_id.clone(),
            token: Some(legacy_token.clone()),
            finalize_id: "finalize-legacy".to_owned(),
            evidence: passing_evidence(
                &legacy_prior.acquire.envelope.binding,
                &legacy_acquired.lease.lease_id,
                &legacy_acquired.lease.slot_id,
                legacy_acquired
                    .lease
                    .slot_path
                    .as_ref()
                    .expect("legacy path"),
                &legacy_token,
            ),
        })
        .expect("legacy v1 finalizes");
    let mut legacy_successor = legacy_prior.clone();
    legacy_successor.root.allocation_epoch = "allocation-b".to_owned();
    let legacy_handoff = OrdinaryCargoHandoffRequest {
        prior_request: legacy_prior.clone(),
        successor_request: legacy_successor,
        slot_id: legacy_acquired.lease.slot_id.clone(),
        storage_instance_id: legacy_acquired.lease.storage_instance_id.clone(),
        generation: legacy_acquired.lease.generation.clone(),
        handoff_id: "handoff-legacy".to_owned(),
        handoff_version: ORDINARY_CARGO_HANDOFF_VERSION,
        retention: retention_grant(),
        source_transition: handoff_transition("a", "b", true),
        prior_closure_reference: "closed-allocation-1".to_owned(),
        successor_assignment_reference: "assigned-allocation-b".to_owned(),
        credential: Credential::new("handoff-proof"),
    };
    assert_eq!(
        legacy_daemon.client.handoff_ordinary_cargo(legacy_handoff),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
    legacy_daemon.stop();

    // Source transition: unqualified mtime refuses (equal/older-mtime changed
    // inputs are metadata only here, explicitly not an actual Cargo test).
    let mut unqualified = good.clone();
    unqualified.handoff_id = "handoff-unqualified-mtime".to_owned();
    unqualified.source_transition.mtime_qualified = false;
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(unqualified),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
    // Missing transition classes refuse.
    let mut missing_class = good.clone();
    missing_class.handoff_id = "handoff-missing-class".to_owned();
    missing_class
        .source_transition
        .qualified_input_classes
        .retain(|class| class != "proc-macro");
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(missing_class),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
    // Retention without independent evidence refuses.
    let mut no_retention = good.clone();
    no_retention.handoff_id = "handoff-no-retention".to_owned();
    no_retention.retention.source_evidence_reference = "unknown-source".to_owned();
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(no_retention),
        Err(StorageError::Unauthorized)
    );
    // Different physical path refuses.
    let mut moved = good.clone();
    moved.handoff_id = "handoff-moved".to_owned();
    moved.successor_request.root.source_root = other_source.path().to_path_buf();
    assert!(daemon.client.handoff_ordinary_cargo(moved).is_err());
    // Changed installation refuses.
    let mut reinstall = good.clone();
    reinstall.handoff_id = "handoff-reinstall".to_owned();
    reinstall.successor_request.acceptance.installation_epoch = "install-2".to_owned();
    assert!(daemon.client.handoff_ordinary_cargo(reinstall).is_err());
    // Changed configuration refuses.
    let mut reconfig = good.clone();
    reconfig.handoff_id = "handoff-reconfig".to_owned();
    reconfig
        .successor_request
        .acquire
        .envelope
        .partition
        .config_digest = "sha256:other-config".to_owned();
    assert!(daemon.client.handoff_ordinary_cargo(reconfig).is_err());
    // A leased generation refuses: prior writers must be positively closed.
    let leased_storage = TaskRoot::new();
    let leased_source = TaskRoot::new();
    let leased_authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut leased_daemon = RunningDaemon::start_with_authority(
        leased_storage.path(),
        budget(256),
        Some(1024),
        leased_authority.clone(),
    );
    let leased_prior =
        handoff_policy_request(leased_source.path(), "leased", "allocation-a", "acquire-a");
    let leased_acquired = leased_daemon
        .client
        .acquire_ordinary_cargo(leased_prior.clone())
        .expect("leased prior acquires");
    let leased_successor =
        handoff_policy_request(leased_source.path(), "next", "allocation-b", "acquire-b");
    let leased_handoff = handoff_request(
        leased_prior.clone(),
        leased_successor,
        &leased_acquired.lease.slot_id,
        &leased_acquired.lease.storage_instance_id,
        &leased_acquired.lease.generation,
        "handoff-leased",
    );
    assert_eq!(
        leased_daemon.client.handoff_ordinary_cargo(leased_handoff),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );
    // After restart the active lease is uncertain and still holds handoff.
    leased_daemon.stop();
    let mut uncertain_daemon = RunningDaemon::start_with_authority(
        leased_storage.path(),
        budget(256),
        Some(1024),
        leased_authority,
    );
    let uncertain_successor =
        handoff_policy_request(leased_source.path(), "next", "allocation-b", "acquire-b");
    let uncertain_handoff = handoff_request(
        leased_prior,
        uncertain_successor,
        &leased_acquired.lease.slot_id,
        &leased_acquired.lease.storage_instance_id,
        &leased_acquired.lease.generation,
        "handoff-uncertain",
    );
    assert_eq!(
        uncertain_daemon
            .client
            .handoff_ordinary_cargo(uncertain_handoff),
        Err(StorageError::Refused(RefusalReason::LeaseUnavailable))
    );
    uncertain_daemon.stop();
    leased_daemon.stop();
    // A foreign generation (different slot identity) refuses.
    let mut foreign = good.clone();
    foreign.handoff_id = "handoff-foreign".to_owned();
    foreign.slot_id = "slot-ffffffffffffffff".to_owned();
    foreign.generation = u64::from_str_radix("ffffffffffffffff", 16)
        .unwrap()
        .to_string();
    assert!(daemon.client.handoff_ordinary_cargo(foreign).is_err());
    // Cold-only generation refuses warm handoff.
    let cold_storage = TaskRoot::new();
    let cold_source = TaskRoot::new();
    let cold_authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut cold_daemon = RunningDaemon::start_with_authority(
        cold_storage.path(),
        budget(256),
        Some(1024),
        cold_authority,
    );
    let cold_prior = handoff_policy_request(
        cold_source.path(),
        "cold-prior",
        "allocation-a",
        "acquire-a",
    );
    let cold_acquired = cold_daemon
        .client
        .acquire_ordinary_cargo(cold_prior.clone())
        .expect("cold prior acquires");
    let cold_token = cold_acquired.lease.token.clone().expect("cold token");
    cold_daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: cold_prior.clone(),
            lease_id: cold_acquired.lease.lease_id.clone(),
            token: Some(cold_token.clone()),
            finalize_id: "finalize-cold".to_owned(),
            evidence: complete_evidence(
                &cold_prior.acquire.envelope.binding,
                &cold_acquired.lease.lease_id,
                &cold_acquired.lease.slot_id,
                cold_acquired.lease.slot_path.as_ref().expect("cold path"),
                &cold_token,
            ),
        })
        .expect("settled failure becomes cold-only");
    let cold_successor =
        handoff_policy_request(cold_source.path(), "cold-next", "allocation-b", "acquire-b");
    let cold_handoff = handoff_request(
        cold_prior,
        cold_successor,
        &cold_acquired.lease.slot_id,
        &cold_acquired.lease.storage_instance_id,
        &cold_acquired.lease.generation,
        "handoff-cold",
    );
    assert_eq!(
        cold_daemon.client.handoff_ordinary_cargo(cold_handoff),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoColdOnly))
    );
    cold_daemon.stop();
    daemon.stop();
}

#[test]
fn handoff_replay_conflict_concurrency_and_crash_converge() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(256), Some(1024), authority);
    let prior = handoff_policy_request(source.path(), "prior-a", "allocation-a", "acquire-a");
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(prior.clone())
        .expect("cold prior acquires");
    seal_reusable(&daemon.client, &prior, &acquired, "finalize-prior-a");
    let successor =
        handoff_policy_request(source.path(), "successor-b", "allocation-b", "acquire-b");
    let base = handoff_request(
        prior.clone(),
        successor.clone(),
        &acquired.lease.slot_id,
        &acquired.lease.storage_instance_id,
        &acquired.lease.generation,
        "handoff-a-b",
    );
    let receipt = daemon
        .client
        .handoff_ordinary_cargo(base.clone())
        .expect("handoff commits");

    // Same operation id with changed body conflicts (the changed transition
    // still passes owner authentication, so exactness—not denial—decides).
    let mut conflict = base.clone();
    conflict.source_transition.transition_reference = "transition-other".to_owned();
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(conflict.clone()),
        Err(StorageError::Conflict)
    );
    assert_eq!(
        daemon.client.lookup_handoff_ordinary_cargo(conflict),
        Err(StorageError::Conflict)
    );
    // Unknown handoff lookup is absent and grants nothing.
    let mut absent = base.clone();
    absent.handoff_id = "handoff-never-recorded".to_owned();
    assert!(matches!(
        daemon
            .client
            .lookup_handoff_ordinary_cargo(absent)
            .expect("absent lookup"),
        LookupResult::Absent
    ));
    // Competing successor claims cannot both win: the recorded handoff replays,
    // a different successor for the same prior conflicts on stale ownership.
    let mut rival_successor =
        handoff_policy_request(source.path(), "rival-c", "allocation-c", "acquire-c");
    rival_successor.acquire.envelope.binding.task_id = "task-rival".to_owned();
    let mut rival = base.clone();
    rival.successor_request = rival_successor;
    rival.handoff_id = "handoff-a-c".to_owned();
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(rival),
        Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoIncompatible
        ))
    );
    assert_eq!(
        daemon
            .client
            .handoff_ordinary_cargo(base.clone())
            .expect("winner replays"),
        receipt
    );
    // Lost-reply retry around commit converges: a forgotten send followed by
    // exact replay still returns the single receipt.
    super::transport::send_ordinary_handoff_and_forget(&daemon.socket_path, base.clone())
        .expect("lost-reply send");
    assert_eq!(
        daemon
            .client
            .handoff_ordinary_cargo(base.clone())
            .expect("converges after lost reply"),
        receipt
    );
    daemon.stop();
}

#[test]
fn handoff_history_is_finite() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(OpenHandoffAuthority);
    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(1024), Some(4096), authority);
    let mut prior = handoff_policy_request(source.path(), "gen-0", "allocation-0", "acquire-0");
    let acquired = daemon
        .client
        .acquire_ordinary_cargo(prior.clone())
        .expect("initial generation acquires");
    seal_reusable(&daemon.client, &prior, &acquired, "finalize-0");
    let slot_id = acquired.lease.slot_id.clone();
    let instance = acquired.lease.storage_instance_id.clone();
    let generation = acquired.lease.generation.clone();
    // Chain one handoff per allocation epoch on the same slot/generation.
    for index in 1..=64u32 {
        let successor = handoff_policy_request(
            source.path(),
            &format!("gen-{index}"),
            &format!("allocation-{index}"),
            &format!("acquire-{index}"),
        );
        let request = OrdinaryCargoHandoffRequest {
            prior_request: prior.clone(),
            successor_request: successor.clone(),
            slot_id: slot_id.clone(),
            storage_instance_id: instance.clone(),
            generation: generation.clone(),
            handoff_id: format!("handoff-{index}"),
            handoff_version: ORDINARY_CARGO_HANDOFF_VERSION,
            retention: retention_grant(),
            source_transition: handoff_transition("a", "b", true),
            prior_closure_reference: format!("closed-{}", prior.root.allocation_epoch),
            successor_assignment_reference: format!("assigned-{}", successor.root.allocation_epoch),
            credential: Credential::new("handoff-proof"),
        };
        daemon
            .client
            .handoff_ordinary_cargo(request)
            .unwrap_or_else(|error| panic!("handoff {index} commits: {error:?}"));
        prior = successor;
    }
    // The 65th distinct handoff refuses on finite history without trimming.
    let overflow_successor = handoff_policy_request(
        source.path(),
        "gen-overflow",
        "allocation-overflow",
        "acquire-x",
    );
    let overflow = OrdinaryCargoHandoffRequest {
        prior_request: prior.clone(),
        successor_request: overflow_successor,
        slot_id: slot_id.clone(),
        storage_instance_id: instance.clone(),
        generation: generation.clone(),
        handoff_id: "handoff-overflow".to_owned(),
        handoff_version: ORDINARY_CARGO_HANDOFF_VERSION,
        retention: retention_grant(),
        source_transition: handoff_transition("a", "b", true),
        prior_closure_reference: format!("closed-{}", prior.root.allocation_epoch),
        successor_assignment_reference: "assigned-allocation-overflow".to_owned(),
        credential: Credential::new("handoff-proof"),
    };
    assert_eq!(
        daemon.client.handoff_ordinary_cargo(overflow),
        Err(StorageError::Refused(RefusalReason::UnknownAccounting))
    );
    // Old receipts still replay exactly after exhaustion.
    let mut replay_prior =
        handoff_policy_request(source.path(), "gen-0", "allocation-0", "acquire-0");
    // Rebuild the exact first handoff body: prior gen-0 to successor gen-1.
    let replay_successor =
        handoff_policy_request(source.path(), "gen-1", "allocation-1", "acquire-1");
    replay_prior.acquire.envelope.binding.task_id = "task-gen-0".to_owned();
    replay_prior.acquire.envelope.binding.workspace_id = "workspace-gen-0".to_owned();
    replay_prior.acquire.envelope.binding.run_id = "run-gen-0".to_owned();
    replay_prior.acquire.envelope.binding.attempt_id = "gen-0".to_owned();
    let mut expected_successor = replay_successor.clone();
    expected_successor.acquire.envelope.binding.task_id = "task-gen-1".to_owned();
    expected_successor.acquire.envelope.binding.workspace_id = "workspace-gen-1".to_owned();
    expected_successor.acquire.envelope.binding.run_id = "run-gen-1".to_owned();
    expected_successor.acquire.envelope.binding.attempt_id = "gen-1".to_owned();
    let replay = OrdinaryCargoHandoffRequest {
        prior_request: replay_prior,
        successor_request: expected_successor,
        slot_id: slot_id.clone(),
        storage_instance_id: instance.clone(),
        generation: generation.clone(),
        handoff_id: "handoff-1".to_owned(),
        handoff_version: ORDINARY_CARGO_HANDOFF_VERSION,
        retention: retention_grant(),
        source_transition: handoff_transition("a", "b", true),
        prior_closure_reference: "closed-allocation-0".to_owned(),
        successor_assignment_reference: "assigned-allocation-1".to_owned(),
        credential: Credential::new("handoff-proof"),
    };
    assert!(matches!(
        daemon
            .client
            .lookup_handoff_ordinary_cargo(replay)
            .expect("first receipt replays"),
        LookupResult::Present(_)
    ));
    daemon.stop();
}
