pub(crate) mod bounded;
pub(crate) mod codec;
mod store;
mod transport;
mod types;

pub use bounded::{BoundedBackendDescriptor, BoundedBacking, BOUNDED_BACKEND_APFS_SPARSE_IMAGE_V1};
pub use store::{
    ordinary_cargo_stable_reuse_reference, Daemon, DaemonConfig, EvidenceAuthority, FreeSpaceProbe,
};
pub use transport::{Client, STORAGE_PROTOCOL_VERSION};
pub use types::{
    AcquireRequest, AcquireResult, Budget, CaptureReceipt, CargoReuseState, ContainmentEvidence,
    CooperativeCompletionEvidence, Credential, FinalizationReceipt, Inventory, InventorySlot,
    LeaseIdentity, LeaseToken, LookupResult, NoSpawnEvidence, OrdinaryCargoAcceptance,
    OrdinaryCargoAcquireResult, OrdinaryCargoFinalizationReceipt, OrdinaryCargoFinalizeRequest,
    OrdinaryCargoLeaseIdentity, OrdinaryCargoPartition, OrdinaryCargoRequest, PartitionKey,
    ProcessIdentityEvidence, ReconcileReceipt, RefusalReason, RunBinding, RunEnvelope,
    SettlementEvidence, SlotState, StorageError, StrictPhysicalCargoRoot, TerminalOutcome,
    TerminalRecord, ORDINARY_CARGO_POLICY_VERSION, ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION,
};

#[cfg(test)]
mod tests;
