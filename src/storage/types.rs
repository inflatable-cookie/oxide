use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Original exact-attempt ordinary-Cargo reuse policy semantics.
pub const ORDINARY_CARGO_POLICY_VERSION: u32 = 1;
/// Stable compatibility eligibility semantics for ordinary-Cargo policy v2.
pub const ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION: u32 = 2;

/// Capacity limits for the Oxide-owned intermediate slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    /// Maximum measured or reserved bytes across owned slots.
    pub cap_bytes: u64,
    /// Minimum host free bytes that must remain after a new reservation.
    pub free_reserve_bytes: u64,
}

/// Inputs that must match before a slot can be reused.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartitionKey {
    pub project_component: String,
    pub workflow: String,
    pub cargo_provenance: String,
    pub rustc_provenance: String,
    pub host: String,
    pub target: String,
    pub profile: String,
    pub features: Vec<String>,
    pub config_digest: String,
    pub environment_digest: String,
    pub path_identity: String,
}

/// Exact consumer and source identity for one admitted run attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunBinding {
    pub consumer: String,
    pub project: String,
    pub component: String,
    pub workflow: String,
    pub task_id: String,
    pub workspace_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub commit: String,
    pub source_tree_digest: String,
}

/// Opaque credential data validated by the configured consumer authority.
#[derive(Deserialize, Serialize)]
pub struct Credential(String);

impl Credential {
    /// Wrap a credential supplied by the trusted adapter boundary.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Exposes the credential to an evidence verifier.
    ///
    /// Callers should use this only to authenticate the envelope and must not
    /// log or persist the returned value.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Clone for Credential {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl PartialEq for Credential {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Credential {}

impl fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Credential([redacted])")
    }
}

/// Authenticated run envelope from Queue, Nucleus, or another consumer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunEnvelope {
    pub binding: RunBinding,
    pub partition: PartitionKey,
    pub admission_reference: String,
    pub admission_credential: Credential,
}

/// A retryable request to acquire one exclusive slot for a run attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcquireRequest {
    pub envelope: RunEnvelope,
    pub acquire_id: String,
    /// Capacity reserved for this slot while the run owns it.
    pub required_bytes: u64,
}

/// The only compatibility partitions admitted by the ordinary-Cargo policy.
/// `Other` keeps unsupported labels representable so the daemon can refuse
/// them explicitly instead of silently creating another partition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OrdinaryCargoPartition {
    OrdinaryTest,
    FullDebug,
    Other(String),
}

impl OrdinaryCargoPartition {
    pub fn label(&self) -> &str {
        match self {
            Self::OrdinaryTest => "ordinary-test",
            Self::FullDebug => "full-debug",
            Self::Other(value) => value,
        }
    }
}

/// Consumer-owned strict physical source-root allocation identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrictPhysicalCargoRoot {
    pub source_root: PathBuf,
    pub consumer_root_id: String,
    pub allocation_epoch: String,
}

/// Installed, owner-authenticated ordinary-Cargo acceptance.
///
/// The credential is verified by the configured owning boundary and is never
/// persisted. Version 1 binds generation reuse to the exact input digest;
/// version 2 uses a stable eligibility reference while each exact attempt is
/// independently authenticated. A caller-provided digest alone cannot enable
/// warm reuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoAcceptance {
    pub policy_version: u32,
    pub installation_epoch: String,
    pub acceptance_reference: String,
    pub input_boundary_digest: String,
    pub credential: Credential,
}

/// Exact authenticated request for one ordinary-Cargo generation lease.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoRequest {
    pub acquire: AcquireRequest,
    pub root: StrictPhysicalCargoRoot,
    pub partition: OrdinaryCargoPartition,
    pub acceptance: OrdinaryCargoAcceptance,
}

/// Durable ordinary reuse eligibility, independent of slot occupancy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CargoReuseState {
    Empty,
    Pending,
    Reusable,
    ColdOnly,
}

/// Ordinary acquire result. Its embedded lease has no access after settlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoAcquireResult {
    pub policy_version: u32,
    pub root: StrictPhysicalCargoRoot,
    pub partition: OrdinaryCargoPartition,
    pub reuse_state: CargoReuseState,
    pub lease: AcquireResult,
}

/// Access-free identity returned by ordinary acquire lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoLeaseIdentity {
    pub policy_version: u32,
    pub root: StrictPhysicalCargoRoot,
    pub partition: OrdinaryCargoPartition,
    pub reuse_state: CargoReuseState,
    pub lease: LeaseIdentity,
}

/// Exact ordinary finalization request. `request` repeats the original acquire
/// body so policy, root and compatibility identity are checked on settlement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoFinalizeRequest {
    pub request: OrdinaryCargoRequest,
    pub lease_id: String,
    pub token: Option<LeaseToken>,
    pub finalize_id: String,
    pub evidence: SettlementEvidence,
}

/// Ordinary finalize result including the durable reuse transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinaryCargoFinalizationReceipt {
    pub policy_version: u32,
    pub root: StrictPhysicalCargoRoot,
    pub partition: OrdinaryCargoPartition,
    pub reuse_state: CargoReuseState,
    pub finalization: FinalizationReceipt,
}

/// State of a slot in the durable ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotState {
    Idle,
    Leased,
    Uncertain,
}

/// A read-only view of one owned slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySlot {
    pub slot_id: String,
    pub partition: PartitionKey,
    pub state: SlotState,
    pub measured_bytes: Option<u64>,
    pub reserved_bytes: u64,
    pub owner: Option<RunBinding>,
    pub cargo_reuse_state: Option<CargoReuseState>,
    pub ordinary_root: Option<StrictPhysicalCargoRoot>,
    pub ordinary_partition: Option<OrdinaryCargoPartition>,
}

/// A read-only storage and capacity snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Inventory {
    pub budget: Budget,
    pub accounted_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub slots: Vec<InventorySlot>,
}

/// Result of an acquire, including the original result on an idempotent retry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcquireResult {
    pub lease_id: String,
    pub slot_id: String,
    /// Durable random identity of the ledger instance that owns this slot.
    pub storage_instance_id: String,
    /// Slot ordinal as canonical positive u64 decimal text.
    pub generation: String,
    pub state: SlotState,
    /// Present only while the current daemon can safely grant the active lease.
    pub slot_path: Option<PathBuf>,
    /// Required for later settlement operations; its debug output is redacted.
    pub token: Option<LeaseToken>,
    pub reused: bool,
}

/// Storage identity returned by a lookup-only acquire recovery request.
///
/// This deliberately contains no slot path or lease token and cannot grant
/// writer access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseIdentity {
    pub lease_id: String,
    pub slot_id: String,
    pub storage_instance_id: String,
    /// Slot ordinal as canonical positive u64 decimal text.
    pub generation: String,
    pub state: SlotState,
}

/// Truthful result of an authenticated lookup that has no mutation authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LookupResult<T> {
    Absent,
    Present(T),
    /// The current lease is uncertain; an optional value is present only when
    /// the requested operation itself was durably accepted.
    Uncertain(Option<T>),
}

/// Opaque token scoped to one persisted lease.
pub struct LeaseToken(String);

impl LeaseToken {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    pub(crate) fn value(&self) -> &str {
        &self.0
    }
}

impl Clone for LeaseToken {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl PartialEq for LeaseToken {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for LeaseToken {}

impl fmt::Debug for LeaseToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LeaseToken([redacted])")
    }
}

/// Test or production outcome recorded by the consumer, not an Oxide verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOutcome {
    Passed,
    Failed,
    Cancelled,
}

/// Authenticated terminal run record from the consumer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalRecord {
    pub run_id: String,
    pub outcome: TerminalOutcome,
    pub credential: Credential,
}

/// Consumer-owned capture receipt reference and digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureReceipt {
    pub reference: String,
    pub digest: String,
    pub host_run_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub credential: Credential,
}

/// One task-owned child process joined by the cooperative workflow boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentityEvidence {
    pub pid: u32,
    /// A per-spawn identity echoed by the child before it can write.
    pub start_identity: String,
}

/// Completion evidence for a registered cooperative foreground workflow.
///
/// The record is MAC-bound to the current lease token before it can release a
/// slot. It proves only that the trusted workflow joined the listed fixture
/// children and captured their bounded output; it is not an OS writer fence.
#[derive(Clone, Eq, PartialEq)]
pub struct CooperativeCompletionEvidence {
    pub lease_id: String,
    pub slot_id: String,
    pub slot_path: String,
    pub temp_dir: String,
    pub capture_dir: String,
    pub workflow: String,
    pub workflow_definition_digest: String,
    pub repository_head: String,
    pub fixture_source_digest: String,
    pub host_run_id: String,
    pub run_id: String,
    pub attempt_id: String,
    pub processes: Vec<ProcessIdentityEvidence>,
    pub free_space_samples: Vec<u64>,
    pub exit_code: i32,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub capture_reference: String,
    pub capture_digest: String,
    pub signature: String,
}

impl fmt::Debug for CooperativeCompletionEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CooperativeCompletionEvidence")
            .field("lease_id", &self.lease_id)
            .field("slot_id", &self.slot_id)
            .field("slot_path", &self.slot_path)
            .field("temp_dir", &self.temp_dir)
            .field("capture_dir", &self.capture_dir)
            .field("workflow", &self.workflow)
            .field(
                "workflow_definition_digest",
                &self.workflow_definition_digest,
            )
            .field("repository_head", &self.repository_head)
            .field("fixture_source_digest", &self.fixture_source_digest)
            .field("host_run_id", &self.host_run_id)
            .field("run_id", &self.run_id)
            .field("attempt_id", &self.attempt_id)
            .field("processes", &self.processes)
            .field("free_space_samples", &self.free_space_samples)
            .field("exit_code", &self.exit_code)
            .field("stdout_digest", &self.stdout_digest)
            .field("stderr_digest", &self.stderr_digest)
            .field("capture_reference", &self.capture_reference)
            .field("capture_digest", &self.capture_digest)
            .field("signature", &"[redacted]")
            .finish()
    }
}

impl CooperativeCompletionEvidence {
    pub(crate) fn sign(&mut self, token: &LeaseToken) {
        self.signature = completion_mac(token.value(), self);
    }

    pub(crate) fn verify(&self, token: &LeaseToken) -> bool {
        let expected = completion_mac(token.value(), self);
        constant_time_eq(self.signature.as_bytes(), expected.as_bytes())
    }
}

/// Proof scoped to an active lease whose command boundary was never called.
/// Construction/signing is crate-private; external caller declarations cannot
/// release a slot. Like cooperative completion, this assumes the trusted adapter.
#[derive(Clone, Eq, PartialEq)]
pub struct NoSpawnEvidence {
    pub(crate) lease_id: String,
    pub(crate) slot_id: String,
    pub(crate) slot_path: String,
    pub(crate) run_id: String,
    pub(crate) attempt_id: String,
    pub(crate) signature: String,
}
impl fmt::Debug for NoSpawnEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NoSpawnEvidence")
            .field("lease_id", &self.lease_id)
            .field("slot_id", &self.slot_id)
            .field("run_id", &self.run_id)
            .field("attempt_id", &self.attempt_id)
            .finish_non_exhaustive()
    }
}
impl NoSpawnEvidence {
    pub(crate) fn new(
        acquired: &AcquireResult,
        binding: &RunBinding,
        token: &LeaseToken,
    ) -> Option<Self> {
        let mut proof = Self {
            lease_id: acquired.lease_id.clone(),
            slot_id: acquired.slot_id.clone(),
            slot_path: acquired.slot_path.as_ref()?.to_str()?.to_owned(),
            run_id: binding.run_id.clone(),
            attempt_id: binding.attempt_id.clone(),
            signature: String::new(),
        };
        proof.signature = proof.mac(token);
        Some(proof)
    }
    fn mac(&self, token: &LeaseToken) -> String {
        use sha2::{Digest, Sha256};
        let mut key = [0u8; 64];
        let digest = Sha256::digest(token.value().as_bytes());
        key[..32].copy_from_slice(&digest);
        let mut inner_pad = [0x36u8; 64];
        let mut outer_pad = [0x5cu8; 64];
        for (index, byte) in key.iter().enumerate() {
            inner_pad[index] ^= byte;
            outer_pad[index] ^= byte;
        }
        let mut inner = Sha256::new();
        inner.update(inner_pad);
        for value in [
            "oxide-no-spawn-v1",
            &self.lease_id,
            &self.slot_id,
            &self.slot_path,
            &self.run_id,
            &self.attempt_id,
        ] {
            update_field(&mut inner, value.as_bytes());
        }
        let mut outer = Sha256::new();
        outer.update(outer_pad);
        outer.update(inner.finalize());
        format!("{:x}", outer.finalize())
    }
    pub(crate) fn verify(&self, token: &LeaseToken) -> bool {
        constant_time_eq(self.signature.as_bytes(), self.mac(token).as_bytes())
    }
}

/// Process-lifetime evidence. Observations such as PID absence are not proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainmentEvidence {
    Unknown,
    /// Token-bound proof issued by the adapter before any runner is called.
    NotSpawned(NoSpawnEvidence),
    Running {
        pid: u32,
        start_identity: String,
    },
    ProcessAbsent {
        pid: u32,
        start_identity: String,
    },
    CooperativeWorkflow(CooperativeCompletionEvidence),
}

/// Evidence presented for finalization or reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementEvidence {
    pub binding: RunBinding,
    pub consumer_credential: Credential,
    pub terminal: Option<TerminalRecord>,
    pub capture: Option<CaptureReceipt>,
    pub containment: ContainmentEvidence,
}

/// Durable result of a finalize operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizationReceipt {
    pub lease_id: String,
    pub finalize_id: String,
    pub state: SlotState,
    pub capture_digest: Option<String>,
}

/// Durable result of a reconcile operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconcileReceipt {
    pub lease_id: String,
    pub reconcile_id: String,
    pub state: SlotState,
}

/// Fail-closed storage, protocol, or authentication error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StorageError {
    Io(String),
    InvalidInput(&'static str),
    Conflict,
    Unauthorized,
    Protocol(&'static str),
    Refused(RefusalReason),
}

/// Safe refusal reasons callers can explain without deleting uncertain state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefusalReason {
    AlreadyRunning,
    UnknownAccounting,
    FreeSpaceUnavailable,
    FreeSpaceReserve,
    BudgetExceeded,
    NoIdleStorage,
    LeaseUnavailable,
    OrdinaryCargoPending,
    OrdinaryCargoColdOnly,
    OrdinaryCargoPartitionLimit,
    OrdinaryCargoIncompatible,
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "storage I/O failed: {}", message),
            Self::InvalidInput(message) => {
                write!(formatter, "invalid storage request: {}", message)
            }
            Self::Conflict => {
                formatter.write_str("operation identity was reused with different data")
            }
            Self::Unauthorized => formatter.write_str("consumer evidence was not authenticated"),
            Self::Protocol(message) => {
                write!(formatter, "local storage protocol failed: {}", message)
            }
            Self::Refused(reason) => write!(formatter, "storage operation refused: {:?}", reason),
        }
    }
}

impl std::error::Error for StorageError {}

pub(crate) fn parse_generation(value: &str) -> Result<u64, StorageError> {
    if value.is_empty()
        || value.bytes().any(|byte| !byte.is_ascii_digit())
        || value.starts_with('0')
    {
        return Err(StorageError::Protocol(
            "generation is not canonical positive u64 decimal text",
        ));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| StorageError::Protocol("generation exceeds u64"))?;
    if parsed.to_string() != value {
        return Err(StorageError::Protocol(
            "generation is not canonical positive u64 decimal text",
        ));
    }
    Ok(parsed)
}

pub(crate) fn valid_storage_instance_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn slot_ordinal(value: &str) -> Option<u64> {
    if value.len() != 21
        || !value.starts_with("slot-")
        || !value[5..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    u64::from_str_radix(&value[5..], 16)
        .ok()
        .filter(|ordinal| *ordinal > 0)
}

fn completion_mac(token: &str, evidence: &CooperativeCompletionEvidence) -> String {
    use sha2::{Digest, Sha256};

    let mut key = [0_u8; 64];
    let token_bytes = token.as_bytes();
    if token_bytes.len() > key.len() {
        let digest = Sha256::digest(token_bytes);
        key[..digest.len()].copy_from_slice(&digest);
    } else {
        key[..token_bytes.len()].copy_from_slice(token_bytes);
    }
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    update_field(&mut inner, b"oxide-cooperative-completion-v1");
    for value in [
        evidence.lease_id.as_str(),
        evidence.slot_id.as_str(),
        evidence.slot_path.as_str(),
        evidence.temp_dir.as_str(),
        evidence.capture_dir.as_str(),
        evidence.workflow.as_str(),
        evidence.workflow_definition_digest.as_str(),
        evidence.repository_head.as_str(),
        evidence.fixture_source_digest.as_str(),
        evidence.host_run_id.as_str(),
        evidence.run_id.as_str(),
        evidence.attempt_id.as_str(),
    ] {
        update_field(&mut inner, value.as_bytes());
    }
    inner.update((evidence.processes.len() as u64).to_le_bytes());
    for process in &evidence.processes {
        inner.update(process.pid.to_le_bytes());
        update_field(&mut inner, process.start_identity.as_bytes());
    }
    inner.update((evidence.free_space_samples.len() as u64).to_le_bytes());
    for sample in &evidence.free_space_samples {
        inner.update(sample.to_le_bytes());
    }
    inner.update(evidence.exit_code.to_le_bytes());
    for value in [
        evidence.stdout_digest.as_str(),
        evidence.stderr_digest.as_str(),
        evidence.capture_reference.as_str(),
        evidence.capture_digest.as_str(),
    ] {
        update_field(&mut inner, value.as_bytes());
    }
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn update_field(hasher: &mut sha2::Sha256, bytes: &[u8]) {
    use sha2::Digest;

    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (*left ^ *right)
        })
        == 0
}
