use super::*;
use crate::project::ordinary_cargo::{
    ordinary_cargo_input_boundary_digest, OrdinaryCargoInvocation, OrdinaryCargoOperationIds,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const INVALIDATION_DECLARATION_DIGEST: &str =
    "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

struct InstalledStableReuseAuthority {
    accepted_requests: Vec<OrdinaryCargoRequest>,
    accepted_reference: String,
}

impl EvidenceAuthority for InstalledStableReuseAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        self.accepted_requests
            .iter()
            .any(|request| request.acquire.envelope == *envelope)
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        TestAuthority.verify_settlement(evidence)
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        self.accepted_requests.contains(request)
    }

    fn verify_ordinary_cargo_stable_reuse_policy(&self, request: &OrdinaryCargoRequest) -> bool {
        request.acceptance.policy_version == ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
            && request.acceptance.acceptance_reference == self.accepted_reference
    }

    fn verify_ordinary_cargo_settlement(
        &self,
        request: &OrdinaryCargoRequest,
        evidence: &SettlementEvidence,
    ) -> bool {
        self.accepted_requests.contains(request)
            && self.verify_settlement(evidence)
            && evidence.binding == request.acquire.envelope.binding
    }
}

struct ExactOnlyAuthority(OrdinaryCargoRequest);

impl EvidenceAuthority for ExactOnlyAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        self.0.acquire.envelope == *envelope
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        TestAuthority.verify_settlement(evidence)
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        self.0 == *request
    }

    fn verify_ordinary_cargo_settlement(
        &self,
        request: &OrdinaryCargoRequest,
        evidence: &SettlementEvidence,
    ) -> bool {
        self.0 == *request
            && self.verify_settlement(evidence)
            && evidence.binding == request.acquire.envelope.binding
    }
}

fn fixture_digest(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    format!("sha256:{hex}")
}

fn invocation(
    request: OrdinaryCargoRequest,
    attempt: &str,
    source_content_digest: String,
) -> OrdinaryCargoInvocation {
    let source_root = request.root.source_root.clone();
    OrdinaryCargoInvocation {
        request,
        branch_id: format!("synthetic-branch-{attempt}"),
        source_content_digest,
        selector: "rust:test".into(),
        argv: vec!["test".into(), "--workspace".into()],
        cwd: source_root.clone(),
        environment: BTreeMap::from([
            ("CARGO_TARGET_DIR".into(), None),
            ("RUSTFLAGS".into(), None),
            ("TMPDIR".into(), None),
        ]),
        target_dir: source_root.join(format!("target-{attempt}")),
        temp_dir: source_root.join(format!("temp-{attempt}")),
        capture_dir: source_root.join(format!("capture-{attempt}")),
        expected_outputs: vec![
            "source-archive".into(),
            "cargo-log".into(),
            "test-result".into(),
        ],
        operation_ids: OrdinaryCargoOperationIds {
            submit: format!("submit-{attempt}"),
            capture: format!("capture-{attempt}"),
            not_submitted: format!("not-submitted-{attempt}"),
            finalize: format!("finalize-{attempt}"),
        },
    }
}

fn refresh_stable_reference_and_boundary(request: &mut OrdinaryCargoRequest, attempt: &str) {
    let reference = ordinary_cargo_stable_reuse_reference(request, INVALIDATION_DECLARATION_DIGEST)
        .expect("typed stable reuse reference");
    request.acceptance.acceptance_reference = reference;
    let invocation = invocation(
        request.clone(),
        attempt,
        fixture_digest(format!("untrusted source {attempt}").as_bytes()),
    );
    let boundary =
        ordinary_cargo_input_boundary_digest(&invocation).expect("exact attempt input boundary");
    request.acceptance.input_boundary_digest = boundary;
}

fn stable_request(source: &Path, attempt: &str, source_mark: char) -> OrdinaryCargoRequest {
    let mut request = ordinary_request(
        source,
        attempt,
        &format!("stable-acquire-{attempt}"),
        64,
        OrdinaryCargoPartition::OrdinaryTest,
    );
    request.acquire.envelope.binding.task_id = format!("task-{attempt}");
    request.acquire.envelope.binding.workspace_id = format!("workspace-{attempt}");
    request.acquire.envelope.binding.commit = source_mark.to_string().repeat(40);
    let source_bytes = format!("fn source_revision_{attempt}() {{}}\n");
    std::fs::write(
        source.join(format!("{attempt}.rs")),
        source_bytes.as_bytes(),
    )
    .expect("write synthetic source revision under owned root");
    request.acquire.envelope.binding.source_tree_digest = fixture_digest(source_bytes.as_bytes());
    request.acceptance.policy_version = ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION;
    request.acceptance.acceptance_reference.clear();
    refresh_stable_reference_and_boundary(&mut request, attempt);
    let invocation = invocation(
        request.clone(),
        attempt,
        fixture_digest(source_bytes.as_bytes()),
    );
    request.acceptance.input_boundary_digest =
        ordinary_cargo_input_boundary_digest(&invocation).expect("exact source-backed boundary");
    request
}

#[test]
fn ordinary_cargo_stable_reuse_identity_reuses_across_exact_attempts() {
    let storage = TaskRoot::new();
    let source = TaskRoot::new();
    let other_source = TaskRoot::new();
    let first_request = stable_request(source.path(), "first", 'a');
    let second_request = stable_request(source.path(), "second", 'b');
    let no_spawn_request = stable_request(source.path(), "no-spawn", 'e');
    let retry_request = stable_request(source.path(), "retry", 'f');
    let accepted_reference = first_request.acceptance.acceptance_reference.clone();
    assert_eq!(
        accepted_reference,
        second_request.acceptance.acceptance_reference
    );
    assert_ne!(
        first_request.acceptance.input_boundary_digest,
        second_request.acceptance.input_boundary_digest
    );
    let first_invocation = invocation(
        first_request.clone(),
        "first",
        fixture_digest(b"fn source_revision_first() {}\n"),
    );
    let second_invocation = invocation(
        second_request.clone(),
        "second",
        fixture_digest(b"fn source_revision_second() {}\n"),
    );
    assert_ne!(first_invocation.target_dir, second_invocation.target_dir);
    assert_ne!(first_invocation.temp_dir, second_invocation.temp_dir);
    assert_ne!(first_invocation.capture_dir, second_invocation.capture_dir);

    let accepted_requests = vec![
        first_request.clone(),
        second_request.clone(),
        no_spawn_request.clone(),
        retry_request.clone(),
    ];
    let authority: Arc<dyn EvidenceAuthority> = Arc::new(InstalledStableReuseAuthority {
        accepted_requests: accepted_requests.clone(),
        accepted_reference: accepted_reference.clone(),
    });

    let missing_policy_storage = TaskRoot::new();
    let mut missing_policy = RunningDaemon::start_with_authority(
        missing_policy_storage.path(),
        budget(256),
        Some(1024),
        Arc::new(ExactOnlyAuthority(first_request.clone())),
    );
    assert_eq!(
        missing_policy
            .client
            .acquire_ordinary_cargo(first_request.clone()),
        Err(StorageError::Unauthorized)
    );
    assert!(missing_policy
        .client
        .inventory("stable-policy-rejected")
        .unwrap()
        .slots
        .is_empty());
    missing_policy.stop();

    let mut daemon = RunningDaemon::start_with_authority(
        storage.path(),
        budget(256),
        Some(1024),
        authority.clone(),
    );

    let changed_attempts: [fn(&mut OrdinaryCargoRequest); 9] = [
        |request| request.acquire.envelope.partition.config_digest = "sha256:other-config".into(),
        |request| request.acquire.envelope.partition.profile = "full-debug".into(),
        |request| request.acquire.envelope.partition.features = vec!["changed-feature".into()],
        |request| request.acquire.envelope.partition.environment_digest = "sha256:other-env".into(),
        |request| request.acquire.envelope.partition.cargo_provenance = "cargo other".into(),
        |request| request.acquire.envelope.partition.rustc_provenance = "rustc other".into(),
        |request| request.acquire.envelope.partition.target = "other-target".into(),
        |request| request.acceptance.installation_epoch = "other-installation".into(),
        |request| request.root.allocation_epoch = "other-allocation".into(),
    ];
    for (index, change) in changed_attempts.into_iter().enumerate() {
        let mut changed = first_request.clone();
        change(&mut changed);
        refresh_stable_reference_and_boundary(&mut changed, &format!("changed-{index}"));
        assert_ne!(changed.acceptance.acceptance_reference, accepted_reference);
        assert_eq!(
            daemon.client.acquire_ordinary_cargo(changed),
            Err(StorageError::Unauthorized)
        );
    }
    let mut stale_reference = first_request.clone();
    stale_reference.acquire.envelope.partition.config_digest = "sha256:stale-config".into();
    let stale_reference_result = daemon.client.acquire_ordinary_cargo(stale_reference);
    assert!(
        stale_reference_result.is_err(),
        "stale stable reference must be refused"
    );
    let mut changed_root = first_request.clone();
    changed_root.root.source_root = other_source.path().to_path_buf();
    refresh_stable_reference_and_boundary(&mut changed_root, "changed-root");
    assert_ne!(
        changed_root.acceptance.acceptance_reference,
        accepted_reference
    );
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(changed_root),
        Err(StorageError::Unauthorized)
    );
    let mut changed_invalidation = first_request.clone();
    let changed_invalidation_reference = ordinary_cargo_stable_reuse_reference(
        &changed_invalidation,
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    )
    .unwrap();
    changed_invalidation.acceptance.acceptance_reference = changed_invalidation_reference;
    let changed_invalidation_body = invocation(
        changed_invalidation.clone(),
        "changed-invalidation",
        fixture_digest(b"changed invalidation fixture"),
    );
    changed_invalidation.acceptance.input_boundary_digest =
        ordinary_cargo_input_boundary_digest(&changed_invalidation_body).unwrap();
    assert_ne!(
        changed_invalidation.acceptance.acceptance_reference,
        accepted_reference
    );
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(changed_invalidation),
        Err(StorageError::Unauthorized)
    );
    let mut malformed_reference = first_request.clone();
    malformed_reference
        .acceptance
        .acceptance_reference
        .push_str("/unknown");
    assert!(daemon
        .client
        .acquire_ordinary_cargo(malformed_reference)
        .is_err());
    let mut unsigned_attempt = second_request.clone();
    unsigned_attempt.acquire.envelope.binding.source_tree_digest =
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into();
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(unsigned_attempt),
        Err(StorageError::Unauthorized)
    );
    let mut mismatched_boundary = second_request.clone();
    mismatched_boundary.acceptance.input_boundary_digest =
        fixture_digest(b"an unsigned second-attempt boundary");
    assert_eq!(
        daemon
            .client
            .acquire_ordinary_cargo(mismatched_boundary.clone()),
        Err(StorageError::Unauthorized)
    );
    assert_eq!(
        daemon.client.lookup_ordinary_cargo(mismatched_boundary),
        Err(StorageError::Unauthorized)
    );

    let first = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("accepted first exact attempt");
    assert!(!first.lease.reused);
    let first_token = first.lease.token.as_ref().expect("first lease token");
    let first_evidence = passing_evidence(
        &first_request.acquire.envelope.binding,
        &first.lease.lease_id,
        &first.lease.slot_id,
        first.lease.slot_path.as_ref().expect("first slot path"),
        first_token,
    );
    let first_finalize = OrdinaryCargoFinalizeRequest {
        request: first_request.clone(),
        lease_id: first.lease.lease_id.clone(),
        token: Some(first_token.clone()),
        finalize_id: "stable-first-finalize".into(),
        evidence: first_evidence.clone(),
    };
    daemon
        .client
        .finalize_ordinary_cargo(first_finalize.clone())
        .expect("first success and capture publish reusable generation");

    let second = daemon
        .client
        .acquire_ordinary_cargo(second_request.clone())
        .expect("next exact attempt reuses accepted stable generation");
    assert!(second.lease.reused);
    assert_eq!(second.lease.slot_id, first.lease.slot_id);
    assert_eq!(second.lease.generation, first.lease.generation);
    let second_token = second.lease.token.as_ref().expect("second lease token");
    let second_evidence = passing_evidence(
        &second_request.acquire.envelope.binding,
        &second.lease.lease_id,
        &second.lease.slot_id,
        second.lease.slot_path.as_ref().expect("second slot path"),
        second_token,
    );
    let second_finalize = OrdinaryCargoFinalizeRequest {
        request: second_request.clone(),
        lease_id: second.lease.lease_id.clone(),
        token: Some(second_token.clone()),
        finalize_id: "stable-second-finalize".into(),
        evidence: second_evidence.clone(),
    };
    daemon
        .client
        .finalize_ordinary_cargo(second_finalize.clone())
        .expect("second success and capture publish the same generation");
    assert_eq!(daemon.client.evict("stable-no-eviction", 64), Ok(0));
    assert_eq!(
        daemon
            .client
            .inventory("stable-one-generation")
            .unwrap()
            .slots
            .len(),
        1
    );
    let mut mismatched_finalize = second_finalize.clone();
    mismatched_finalize.token = None;
    mismatched_finalize.request.acceptance.input_boundary_digest =
        fixture_digest(b"a different finalized-attempt boundary");
    assert_eq!(
        daemon
            .client
            .lookup_finalize_ordinary_cargo(mismatched_finalize),
        Err(StorageError::Unauthorized)
    );
    daemon.stop();

    let mut daemon =
        RunningDaemon::start_with_authority(storage.path(), budget(256), Some(1024), authority);
    assert!(matches!(
        daemon
            .client
            .lookup_ordinary_cargo(first_request.clone())
            .unwrap(),
        LookupResult::Present(_)
    ));
    assert!(matches!(
        daemon
            .client
            .lookup_ordinary_cargo(second_request.clone())
            .unwrap(),
        LookupResult::Present(_)
    ));
    for finalize in [first_finalize, second_finalize] {
        let mut tokenless = finalize;
        tokenless.token = None;
        assert!(matches!(
            daemon
                .client
                .lookup_finalize_ordinary_cargo(tokenless)
                .unwrap(),
            LookupResult::Present(_)
        ));
    }
    let replay = daemon
        .client
        .acquire_ordinary_cargo(first_request.clone())
        .expect("old exact attempt replay remains bound after restart");
    assert_eq!(replay.lease.slot_id, first.lease.slot_id);
    assert_eq!(replay.lease.generation, first.lease.generation);
    assert!(replay.lease.token.is_none());
    assert!(replay.lease.slot_path.is_none());

    let no_spawn = daemon
        .client
        .acquire_ordinary_cargo(no_spawn_request.clone())
        .expect("third exact attempt reuses after restart");
    assert!(no_spawn.lease.reused);
    let no_spawn_token = no_spawn.lease.token.as_ref().expect("no-spawn lease token");
    let no_spawn_proof = NoSpawnEvidence::new(
        &no_spawn.lease,
        &no_spawn_request.acquire.envelope.binding,
        no_spawn_token,
    )
    .expect("authenticated no-spawn proof");
    daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: no_spawn_request.clone(),
            lease_id: no_spawn.lease.lease_id.clone(),
            token: Some(no_spawn_token.clone()),
            finalize_id: "stable-no-spawn-finalize".into(),
            evidence: SettlementEvidence {
                binding: no_spawn_request.acquire.envelope.binding.clone(),
                consumer_credential: Credential::new("consumer-proof"),
                terminal: None,
                capture: None,
                containment: ContainmentEvidence::NotSpawned(no_spawn_proof),
            },
        })
        .expect("no-spawn restores reusable generation");
    let retry = daemon
        .client
        .acquire_ordinary_cargo(retry_request)
        .expect("retry reuses generation after no-spawn");
    assert!(retry.lease.reused);
    assert_eq!(retry.lease.slot_id, first.lease.slot_id);
    assert_eq!(retry.lease.generation, first.lease.generation);
    daemon.stop();

    let uncertain_storage = TaskRoot::new();
    let uncertain_source = TaskRoot::new();
    let uncertain_first = stable_request(uncertain_source.path(), "uncertain-first", '1');
    let uncertain_owner = stable_request(uncertain_source.path(), "uncertain-owner", '2');
    let uncertain_next = stable_request(uncertain_source.path(), "uncertain-next", '3');
    let uncertain_authority: Arc<dyn EvidenceAuthority> = Arc::new(InstalledStableReuseAuthority {
        accepted_requests: vec![
            uncertain_first.clone(),
            uncertain_owner.clone(),
            uncertain_next.clone(),
        ],
        accepted_reference: uncertain_first.acceptance.acceptance_reference.clone(),
    });
    let mut daemon = RunningDaemon::start_with_authority(
        uncertain_storage.path(),
        budget(256),
        Some(1024),
        uncertain_authority.clone(),
    );
    let first = daemon
        .client
        .acquire_ordinary_cargo(uncertain_first.clone())
        .expect("uncertain control starts with a cold generation");
    let token = first.lease.token.as_ref().expect("cold control token");
    daemon
        .client
        .finalize_ordinary_cargo(OrdinaryCargoFinalizeRequest {
            request: uncertain_first.clone(),
            lease_id: first.lease.lease_id.clone(),
            token: Some(token.clone()),
            finalize_id: "uncertain-first-finalize".into(),
            evidence: passing_evidence(
                &uncertain_first.acquire.envelope.binding,
                &first.lease.lease_id,
                &first.lease.slot_id,
                first.lease.slot_path.as_ref().unwrap(),
                token,
            ),
        })
        .expect("publish first control generation");
    let owner = daemon
        .client
        .acquire_ordinary_cargo(uncertain_owner.clone())
        .expect("second control attempt takes the reusable generation");
    assert!(owner.lease.reused);
    daemon.stop();

    let daemon = RunningDaemon::start_with_authority(
        uncertain_storage.path(),
        budget(256),
        Some(1024),
        uncertain_authority,
    );
    assert!(matches!(
        daemon
            .client
            .lookup_ordinary_cargo(uncertain_owner)
            .unwrap(),
        LookupResult::Uncertain(Some(_))
    ));
    assert_eq!(
        daemon.client.acquire_ordinary_cargo(uncertain_next),
        Err(StorageError::Refused(RefusalReason::OrdinaryCargoPending))
    );
}
