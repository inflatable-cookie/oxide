use std::fmt;
use std::path::PathBuf;

/// Capacity limits for the Oxide-owned intermediate slots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    /// Maximum measured or reserved bytes across owned slots.
    pub cap_bytes: u64,
    /// Minimum host free bytes that must remain after a new reservation.
    pub free_reserve_bytes: u64,
}

/// Inputs that must match before a slot can be reused.
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
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
    pub state: SlotState,
    /// Present only while the current daemon can safely grant the active lease.
    pub slot_path: Option<PathBuf>,
    /// Required for later settlement operations; its debug output is redacted.
    pub token: Option<LeaseToken>,
    pub reused: bool,
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
    pub credential: Credential,
}

/// Process-containment evidence. PID absence is deliberately not release proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainmentEvidence {
    Unknown,
    Running {
        pid: u32,
        start_identity: String,
    },
    ProcessAbsent {
        pid: u32,
        start_identity: String,
    },
    DescendantsStopped {
        guardian_reference: String,
        credential: Credential,
    },
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
