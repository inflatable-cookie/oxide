mod codec;
mod store;
mod transport;
mod types;

pub use store::{Daemon, DaemonConfig, EvidenceAuthority, FreeSpaceProbe};
pub use transport::Client;
pub use types::{
    AcquireRequest, AcquireResult, Budget, CaptureReceipt, ContainmentEvidence, Credential,
    FinalizationReceipt, Inventory, InventorySlot, LeaseToken, PartitionKey, ReconcileReceipt,
    RefusalReason, RunBinding, RunEnvelope, SettlementEvidence, SlotState, StorageError,
    TerminalOutcome, TerminalRecord,
};

#[cfg(test)]
mod tests;
