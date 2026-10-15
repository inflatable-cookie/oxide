use super::bounded::{validate_payload_entries, BoundedBackendDescriptor, BoundedBacking};
use super::codec::{decode_vec, encode_vec, Decode, Encode, Reader, Writer};
use super::transport;
use super::types::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

const LEDGER_MAGIC: &[u8; 8] = b"OXIDEST1";
/// Version 5 adds the durable backend record: version-4 ledgers predate the
/// hard-bounded payload store and refuse without migration or reinterpretation.
const LEDGER_VERSION: u32 = 5;
const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;
const SOCKET_NAME: &str = "daemon.sock";
pub(crate) const SLOTS_NAME: &str = "slots";
const ORDINARY_CARGO_PATH_IDENTITY: &str = "ordinary-cargo-storage-v1";

/// Finite control-history limits. Exhaustion refuses new control records
/// instead of pruning required evidence or growing the ledger indefinitely.
const MAX_LEDGER_SLOTS: usize = 1024;
const MAX_LEDGER_LEASES: usize = 4096;
const MAX_LEDGER_OPERATIONS: usize = 16384;

/// Durable backend mode of one store root. It is bound at daemon startup and
/// recorded in the ledger; a restart with a different mode refuses.
#[derive(Clone, Debug, Eq, PartialEq)]
enum LedgerBackend {
    /// Explicit budget-only accounting on a plain directory. This mode makes
    /// no hard-bound claim: nothing constrains a writer after acquire.
    Directory,
    /// Slots live on the verified hard-bounded payload filesystem. The
    /// aggregate payload ceiling is the ledger budget cap, and the operating
    /// system enforces the volume's finite size against active writers.
    Bounded(BoundedBackendDescriptor),
}

/// Verifies the consumer's admission envelope and settlement evidence.
///
/// The prototype ships no Queue or Nucleus authenticator. A production adapter
/// must supply one; a rejecting implementation is the safe default.
pub trait EvidenceAuthority: Send + Sync + 'static {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool;
    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool;

    /// Verifies that this exact ordinary-Cargo policy and input boundary were
    /// installed and accepted by the owning consumer. Safe default: refuse.
    fn verify_ordinary_cargo_admission(&self, _request: &OrdinaryCargoRequest) -> bool {
        false
    }

    /// Verifies that policy v2's stable compatibility and invalidation
    /// declaration were installed by the accepting consumer. Safe default:
    /// refuse. Exact per-attempt admission remains independently required.
    fn verify_ordinary_cargo_stable_reuse_policy(&self, _request: &OrdinaryCargoRequest) -> bool {
        false
    }

    /// Authenticates ordinary settlement against its original policy/root
    /// request. Safe default: refuse.
    fn verify_ordinary_cargo_settlement(
        &self,
        _request: &OrdinaryCargoRequest,
        _evidence: &SettlementEvidence,
    ) -> bool {
        false
    }
}

/// Supplies a host free-space observation. `None` means capacity is unknown.
pub trait FreeSpaceProbe: Send + Sync + 'static {
    fn available_bytes(&self, root: &Path) -> Result<Option<u64>, StorageError>;
}

/// Filesystem and budget configuration for the local prototype daemon.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    /// Existing, canonical, user-private directory reserved for this prototype.
    /// Holds only the ledger, socket and temporary files; in bounded mode the
    /// payload volume holds the slots so a full payload cannot wedge durable
    /// control updates.
    pub root: PathBuf,
    pub budget: Budget,
    /// Verified hard-bounded payload backing. Absent keeps explicit
    /// budget-only directory semantics; caller JSON cannot mint this value.
    pub backing: Option<BoundedBacking>,
}

/// A single-owner local daemon. Its Unix socket lives under the private root.
pub struct Daemon {
    listener: Option<UnixListener>,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
    store: Store,
}

impl fmt::Debug for Daemon {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Daemon")
            .field("socket_path", &self.socket_path)
            .finish()
    }
}

impl Daemon {
    /// Binds the private local endpoint and recovers durable lease state.
    pub fn bind(
        config: DaemonConfig,
        authority: Arc<dyn EvidenceAuthority>,
        free_space: Arc<dyn FreeSpaceProbe>,
    ) -> Result<Self, StorageError> {
        let root = validate_root(&config.root)?;
        let socket_path = root.join(SOCKET_NAME);
        if socket_path.as_os_str().as_bytes().len() > 100 {
            return Err(StorageError::InvalidInput("Unix socket path is too long"));
        }
        remove_stale_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path).map_err(io_error)?;
        let socket_meta = fs::symlink_metadata(&socket_path).map_err(io_error)?;
        if !socket_meta.file_type().is_socket() {
            let _ = fs::remove_file(&socket_path);
            return Err(StorageError::InvalidInput(
                "daemon socket path was replaced",
            ));
        }
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
        let socket_meta = fs::symlink_metadata(&socket_path).map_err(io_error)?;
        let socket_device = socket_meta.dev();
        let socket_inode = socket_meta.ino();

        let store = match Store::open(root, config.budget, config.backing, authority, free_space) {
            Ok(store) => store,
            Err(error) => {
                remove_owned_socket(&socket_path, socket_device, socket_inode);
                return Err(error);
            }
        };

        Ok(Self {
            listener: Some(listener),
            socket_path,
            socket_device,
            socket_inode,
            store,
        })
    }

    /// Returns the endpoint clients connect to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Non-secret durable instance identity for trusted qualification bootstrap.
    pub fn storage_instance_id(&self) -> &str {
        &self.store.ledger.storage_instance_id
    }

    /// Serves one request at a time until the owner signals shutdown.
    ///
    /// The serialized loop is the ledger's in-process single-writer boundary.
    /// It does not contain or monitor build descendants.
    pub fn serve(mut self, shutdown: Receiver<()>) -> Result<(), StorageError> {
        let listener = self
            .listener
            .take()
            .ok_or(StorageError::Protocol("listener was already consumed"))?;
        let socket_path = self.socket_path.clone();
        let (accepted_sender, accepted_receiver) = mpsc::sync_channel(32);
        let accept_thread = std::thread::Builder::new()
            .name("oxide-storage-accept".to_owned())
            .spawn(move || loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if accepted_sender.send(Ok(stream)).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = accepted_sender.send(Err(error));
                        break;
                    }
                }
            })
            .map_err(io_error)?;

        let mut stopping = false;
        let mut serving_error = None;
        loop {
            match shutdown.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    stopping = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            match accepted_receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(Ok(stream)) => {
                    let _ = transport::serve_one(stream, &mut self.store);
                }
                Ok(Err(error)) => {
                    serving_error = Some(io_error(error));
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    serving_error = Some(StorageError::Protocol("accept loop stopped"));
                    break;
                }
            }
        }

        drop(accepted_receiver);
        if stopping {
            // Wake the blocking accept call. The dropped receiver makes its
            // next send fail, so this connection is never served as a request.
            let _ = UnixStream::connect(socket_path);
        }
        if accept_thread.join().is_err() {
            return Err(StorageError::Protocol("accept thread panicked"));
        }
        if let Some(error) = serving_error {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.listener.take();
        remove_owned_socket(&self.socket_path, self.socket_device, self.socket_inode);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LeaseStatus {
    Leased,
    Uncertain,
    Finalized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InternalSlotState {
    Preparing,
    Idle,
    Leased,
    Uncertain,
    Evicting,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Slot {
    id: String,
    partition: PartitionKey,
    state: InternalSlotState,
    measured_bytes: Option<u64>,
    reserved_bytes: u64,
    lease_id: Option<String>,
    last_used: u64,
    ordinary_cargo: Option<OrdinaryCargoGeneration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OrdinaryCargoGeneration {
    root: StrictPhysicalCargoRoot,
    partition: OrdinaryCargoPartition,
    policy_version: u32,
    installation_epoch: String,
    acceptance_reference: String,
    // First accepted attempt boundary; v2 keeps it as evidence but compares
    // later attempts by their stable eligibility reference.
    input_boundary_digest: String,
    reuse_state: CargoReuseState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OrdinaryCargoLeaseRecord {
    request: OrdinaryCargoRequest,
    saved_reuse_state: CargoReuseState,
    finalized_reuse_state: Option<CargoReuseState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Lease {
    id: String,
    token_hash: String,
    slot_id: String,
    request_digest: String,
    request: AcquireRequest,
    status: LeaseStatus,
    reused: bool,
    ordinary_cargo: Option<OrdinaryCargoLeaseRecord>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SettlementKind {
    Finalize,
    Reconcile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SettlementOperation {
    lease_id: String,
    operation_id: String,
    kind: SettlementKind,
    evidence: SettlementEvidence,
    request_digest: String,
    state: SlotState,
    capture_digest: Option<String>,
    cargo_reuse_state: Option<CargoReuseState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Ledger {
    budget: Budget,
    backend: LedgerBackend,
    storage_instance_id: String,
    next_slot: u64,
    next_sequence: u64,
    slots: Vec<Slot>,
    leases: Vec<Lease>,
    operations: Vec<SettlementOperation>,
}

impl Ledger {
    fn empty(budget: Budget, backend: LedgerBackend) -> Result<Self, StorageError> {
        Ok(Self {
            budget,
            backend,
            storage_instance_id: random_hex(32)?,
            next_slot: 1,
            next_sequence: 1,
            slots: Vec::new(),
            leases: Vec::new(),
            operations: Vec::new(),
        })
    }
}

pub(crate) struct Store {
    root: PathBuf,
    slots_root: PathBuf,
    ledger_path: PathBuf,
    ledger: Ledger,
    authority: Arc<dyn EvidenceAuthority>,
    free_space: Arc<dyn FreeSpaceProbe>,
    live_tokens: std::collections::HashMap<String, String>,
    poisoned: bool,
}

impl Store {
    fn open(
        root: PathBuf,
        budget: Budget,
        backing: Option<BoundedBacking>,
        authority: Arc<dyn EvidenceAuthority>,
        free_space: Arc<dyn FreeSpaceProbe>,
    ) -> Result<Self, StorageError> {
        if budget.cap_bytes == 0 {
            return Err(StorageError::InvalidInput("budget cap must be positive"));
        }
        // Resolve the backend and refuse every mismatch before creating any
        // directory: a refused bind must leave the root exactly as it was.
        let (backend, slots_root) = match backing.as_ref() {
            Some(backing) => {
                // The verified capability places slots on the payload volume;
                // the daemon root keeps only control state. Unknown payload
                // entries refuse so a foreign mount can never join the store.
                validate_payload_entries(backing.payload_root())?;
                (
                    LedgerBackend::Bounded(backing.descriptor().clone()),
                    backing.payload_root().join(SLOTS_NAME),
                )
            }
            None => (LedgerBackend::Directory, root.join(SLOTS_NAME)),
        };
        validate_root_entries(&root, backend_is_bounded(&backend))?;
        let ledger_path = root.join("ledger.bin");
        let ledger = load_ledger(&ledger_path, budget, &backend)?;
        remove_stale_ledger_temps(&root)?;
        ensure_private_directory(&slots_root)?;
        let mut store = Self {
            root,
            slots_root,
            ledger_path,
            ledger,
            authority,
            free_space,
            live_tokens: std::collections::HashMap::new(),
            poisoned: false,
        };
        store.recover()?;
        Ok(store)
    }

    fn recover(&mut self) -> Result<(), StorageError> {
        self.validate_root_inventory()?;
        let mut changed = !path_exists(&self.ledger_path)?;
        let preparing: Vec<String> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.state == InternalSlotState::Preparing)
            .map(|slot| slot.id.clone())
            .collect();
        for id in preparing {
            if self.slot_by_id(&id)?.ordinary_cargo.is_some() {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            let path = self.slot_path(&id)?;
            if path_exists(&path)? {
                remove_tree_owned(&self.slots_root, &path)?;
            }
            self.ledger.slots.retain(|slot| slot.id != id);
            changed = true;
        }

        let evicting: Vec<String> = self
            .ledger
            .slots
            .iter()
            .filter(|slot| slot.state == InternalSlotState::Evicting)
            .map(|slot| slot.id.clone())
            .collect();
        for id in evicting {
            if self.slot_by_id(&id)?.ordinary_cargo.is_some() {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            let path = self.slot_path(&id)?;
            if path_exists(&path)? {
                remove_tree_owned(&self.slots_root, &path)?;
            }
            self.ledger.slots.retain(|slot| slot.id != id);
            changed = true;
        }

        let active_ids: Vec<String> = self
            .ledger
            .leases
            .iter()
            .filter(|lease| lease.status == LeaseStatus::Leased)
            .map(|lease| lease.id.clone())
            .collect();
        for lease_id in active_ids {
            let slot_id = self
                .ledger
                .leases
                .iter()
                .find(|lease| lease.id == lease_id)
                .map(|lease| lease.slot_id.clone())
                .ok_or(StorageError::Protocol("lease disappeared during recovery"))?;
            self.ledger
                .leases
                .iter_mut()
                .find(|lease| lease.id == lease_id)
                .ok_or(StorageError::Protocol("lease disappeared during recovery"))?
                .status = LeaseStatus::Uncertain;
            let slot = self
                .ledger
                .slots
                .iter_mut()
                .find(|slot| slot.id == slot_id)
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            if slot.lease_id.as_deref() != Some(&lease_id) {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            slot.state = InternalSlotState::Uncertain;
            slot.measured_bytes = None;
            changed = true;
        }

        let slot_states: Vec<(String, InternalSlotState)> = self
            .ledger
            .slots
            .iter()
            .map(|slot| (slot.id.clone(), slot.state))
            .collect();
        for (id, state) in slot_states {
            if state == InternalSlotState::Leased {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            let measured = if state == InternalSlotState::Uncertain {
                None
            } else {
                let path = self.slot_path(&id)?;
                Some(measure_tree_owned(&self.slots_root, &path)?)
            };
            let slot = self.slot_by_id_mut(&id)?;
            if slot.measured_bytes != measured {
                slot.measured_bytes = measured;
                changed = true;
            }
        }
        self.validate_slot_inventory()?;
        if changed {
            self.persist()?;
        }
        Ok(())
    }

    fn validate_slot_inventory(&self) -> Result<(), StorageError> {
        if !valid_storage_instance_id(&self.ledger.storage_instance_id)
            || self.ledger.next_slot == 0
            || self.ledger.next_sequence == 0
        {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        let mut known = std::collections::BTreeSet::new();
        let mut largest_generation = 0_u64;
        let mut ordinary_roots = Vec::<(StrictPhysicalCargoRoot, OrdinaryCargoPartition)>::new();
        for slot in &self.ledger.slots {
            if !valid_slot_id(&slot.id) || !known.insert(slot.id.as_str()) {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            largest_generation = largest_generation.max(
                parse_generation(&slot_generation(&slot.id)?)
                    .map_err(|_| StorageError::Refused(RefusalReason::UnknownAccounting))?,
            );
            match slot.state {
                InternalSlotState::Idle if slot.lease_id.is_none() => {}
                InternalSlotState::Uncertain => {
                    let lease_id = slot
                        .lease_id
                        .as_deref()
                        .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                    let lease = self
                        .ledger
                        .leases
                        .iter()
                        .find(|lease| lease.id == lease_id)
                        .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                    if lease.status != LeaseStatus::Uncertain || lease.slot_id != slot.id {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                }
                InternalSlotState::Preparing | InternalSlotState::Evicting => {
                    if slot.lease_id.is_some() {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                }
                _ => return Err(StorageError::Refused(RefusalReason::UnknownAccounting)),
            }
            if let Some(ordinary) = slot.ordinary_cargo.as_ref() {
                if !supported_ordinary_cargo_policy_version(ordinary.policy_version)
                    || ordinary.root.source_root.as_os_str().is_empty()
                    || !ordinary.root.source_root.is_absolute()
                    || ordinary.root.consumer_root_id.is_empty()
                    || ordinary.root.allocation_epoch.is_empty()
                    || ordinary.installation_epoch.is_empty()
                    || ordinary.acceptance_reference.is_empty()
                    || !valid_digest(&ordinary.input_boundary_digest)
                    || !ordinary_generation_stable_reference_is_valid(ordinary, &slot.partition)
                    || slot.partition.path_identity != ORDINARY_CARGO_PATH_IDENTITY
                    || !matches!(
                        &ordinary.partition,
                        OrdinaryCargoPartition::OrdinaryTest | OrdinaryCargoPartition::FullDebug
                    )
                    || (matches!(
                        slot.state,
                        InternalSlotState::Leased | InternalSlotState::Uncertain
                    ) != (ordinary.reuse_state == CargoReuseState::Pending))
                    || (slot.state == InternalSlotState::Preparing
                        && ordinary.reuse_state != CargoReuseState::Empty)
                    || (slot.state == InternalSlotState::Evicting)
                {
                    return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                }
                for (root, partition) in &ordinary_roots {
                    if root.source_root == ordinary.root.source_root && root != &ordinary.root {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                    if root.consumer_root_id == ordinary.root.consumer_root_id
                        && root != &ordinary.root
                    {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                    if same_ordinary_root_allocation(root, &ordinary.root)
                        && partition == &ordinary.partition
                    {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                }
                ordinary_roots.push((ordinary.root.clone(), ordinary.partition.clone()));
            }
        }
        let mut lease_ids = std::collections::BTreeSet::new();
        let mut first_attempt_slots = std::collections::BTreeSet::new();
        for lease in &self.ledger.leases {
            if lease.id.len() != 32
                || !lease.id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || lease.token_hash.len() != 64
                || !lease
                    .token_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || !valid_hash(&lease.request_digest)
                || !lease_ids.insert(lease.id.as_str())
            {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            largest_generation = largest_generation.max(
                parse_generation(&slot_generation(&lease.slot_id)?)
                    .map_err(|_| StorageError::Refused(RefusalReason::UnknownAccounting))?,
            );
            if lease.status == LeaseStatus::Uncertain {
                let slot = self
                    .ledger
                    .slots
                    .iter()
                    .find(|slot| slot.id == lease.slot_id)
                    .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                if slot.state != InternalSlotState::Uncertain
                    || slot.lease_id.as_deref() != Some(lease.id.as_str())
                {
                    return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                }
            }
            match lease.ordinary_cargo.as_ref() {
                Some(ordinary) => {
                    let slot = self
                        .ledger
                        .slots
                        .iter()
                        .find(|slot| slot.id == lease.slot_id)
                        .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                    let generation = slot
                        .ordinary_cargo
                        .as_ref()
                        .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                    let active_owner = slot.lease_id.as_deref() == Some(lease.id.as_str());
                    if validate_ordinary_request(&ordinary.request).is_err()
                        || lease.request
                            != persisted_request(&normalized_ordinary_acquire(
                                &ordinary.request.acquire,
                            ))
                        || ordinary.request.acquire.envelope.partition.path_identity
                            != ORDINARY_CARGO_PATH_IDENTITY
                        || slot.partition
                            != normalized_ordinary_acquire(&ordinary.request.acquire)
                                .envelope
                                .partition
                        || !valid_hash(&lease.request_digest)
                        || !ordinary_generation_matches_request(generation, &ordinary.request)
                        || (active_owner
                            != matches!(lease.status, LeaseStatus::Leased | LeaseStatus::Uncertain))
                        || (active_owner && generation.reuse_state != CargoReuseState::Pending)
                        || (lease.status == LeaseStatus::Finalized
                            && ordinary.finalized_reuse_state.is_none())
                        || (lease.status != LeaseStatus::Finalized
                            && ordinary.finalized_reuse_state.is_some())
                        || !matches!(
                            ordinary.saved_reuse_state,
                            CargoReuseState::Empty | CargoReuseState::Reusable
                        )
                    {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                    if generation.input_boundary_digest
                        == ordinary.request.acceptance.input_boundary_digest
                    {
                        first_attempt_slots.insert(lease.slot_id.as_str());
                    }
                }
                None => {
                    if self
                        .ledger
                        .slots
                        .iter()
                        .find(|slot| slot.id == lease.slot_id)
                        .is_some_and(|slot| slot.ordinary_cargo.is_some())
                    {
                        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
                    }
                }
            }
        }
        if self.ledger.slots.iter().any(|slot| {
            slot.ordinary_cargo.is_some()
                && slot.state != InternalSlotState::Preparing
                && !first_attempt_slots.contains(slot.id.as_str())
        }) {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        let mut operation_ids = std::collections::BTreeSet::new();
        for operation in &self.ledger.operations {
            if !lease_ids.contains(operation.lease_id.as_str())
                || !valid_hash(&operation.request_digest)
                || !operation_ids.insert((
                    operation.lease_id.as_str(),
                    operation.kind,
                    operation.operation_id.as_str(),
                ))
            {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            let lease = self
                .ledger
                .leases
                .iter()
                .find(|lease| lease.id == operation.lease_id)
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            let valid_cargo_state = match (
                lease.ordinary_cargo.is_some(),
                operation.state,
                operation.cargo_reuse_state,
            ) {
                (true, SlotState::Idle, Some(_))
                | (true, SlotState::Uncertain, None)
                | (false, _, None) => true,
                _ => false,
            };
            if !valid_cargo_state {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
        }
        if self.ledger.next_slot <= largest_generation {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        for entry in fs::read_dir(&self.slots_root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !known.contains(name.as_ref()) {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
        }
        for slot in &self.ledger.slots {
            if !path_exists(&self.slot_path(&slot.id)?)? {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
        }
        Ok(())
    }

    fn validate_root_inventory(&self) -> Result<(), StorageError> {
        validate_root_entries(&self.root, backend_is_bounded(&self.ledger.backend))?;
        if backend_is_bounded(&self.ledger.backend) {
            validate_payload_entries(
                self.slots_root
                    .parent()
                    .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?,
            )?;
        }
        Ok(())
    }

    pub(super) fn inventory(&self) -> Result<Inventory, StorageError> {
        self.ensure_healthy()?;
        let mut accounted = Some(0_u64);
        let mut slots = Vec::with_capacity(self.ledger.slots.len());
        for slot in &self.ledger.slots {
            let state = match slot.state {
                InternalSlotState::Idle => SlotState::Idle,
                InternalSlotState::Leased => SlotState::Leased,
                InternalSlotState::Uncertain
                | InternalSlotState::Preparing
                | InternalSlotState::Evicting => SlotState::Uncertain,
            };
            let owned = slot
                .measured_bytes
                .map(|measured| measured.max(slot.reserved_bytes));
            accounted = match (accounted, owned) {
                (Some(total), Some(bytes)) => total.checked_add(bytes),
                _ => None,
            };
            let owner = slot
                .lease_id
                .as_ref()
                .and_then(|id| self.ledger.leases.iter().find(|lease| &lease.id == id))
                .map(|lease| lease.request.envelope.binding.clone());
            slots.push(InventorySlot {
                slot_id: slot.id.clone(),
                partition: slot.partition.clone(),
                state,
                measured_bytes: slot.measured_bytes,
                reserved_bytes: slot.reserved_bytes,
                owner,
                cargo_reuse_state: slot
                    .ordinary_cargo
                    .as_ref()
                    .map(|record| record.reuse_state),
                ordinary_root: slot
                    .ordinary_cargo
                    .as_ref()
                    .map(|record| record.root.clone()),
                ordinary_partition: slot
                    .ordinary_cargo
                    .as_ref()
                    .map(|record| record.partition.clone()),
            });
        }
        slots.sort_by(|left, right| left.slot_id.cmp(&right.slot_id));
        // In bounded mode the actionable free reading is the payload volume's,
        // not the host control filesystem's; a full payload must be visible.
        let free_root = match &self.ledger.backend {
            LedgerBackend::Bounded(descriptor) => descriptor.payload_root.clone(),
            LedgerBackend::Directory => self.root.clone(),
        };
        let free_bytes = self.free_space.available_bytes(&free_root)?;
        Ok(Inventory {
            budget: self.ledger.budget,
            accounted_bytes: accounted,
            free_bytes,
            slots,
        })
    }

    pub(super) fn acquire(
        &mut self,
        mut request: AcquireRequest,
    ) -> Result<AcquireResult, StorageError> {
        self.ensure_healthy()?;
        validate_request(&request)?;
        reject_ordinary_path_identity(&request.envelope.partition.path_identity)?;
        request.envelope.partition.features.sort();
        request.envelope.partition.features.dedup();

        if !self.authority.verify_admission(&request.envelope) {
            return Err(StorageError::Unauthorized);
        }
        let request_digest = digest_encoded(&request)?;
        if let Some(existing) = self
            .ledger
            .leases
            .iter()
            .find(|lease| same_attempt(&lease.request, &request))
        {
            if existing.request != persisted_request(&request)
                || existing.request_digest != request_digest
            {
                return Err(StorageError::Conflict);
            }
            return self.acquire_result(existing);
        }

        if self.ledger.slots.iter().any(|slot| {
            slot.partition == request.envelope.partition && slot.state != InternalSlotState::Idle
        }) {
            return Err(StorageError::Refused(RefusalReason::LeaseUnavailable));
        }

        let matching = self
            .ledger
            .slots
            .iter()
            .filter(|slot| {
                slot.state == InternalSlotState::Idle
                    && slot.ordinary_cargo.is_none()
                    && slot.partition == request.envelope.partition
            })
            .min_by_key(|slot| slot.last_used)
            .map(|slot| slot.id.clone());

        let measured = if let Some(id) = matching.as_ref() {
            self.slot_by_id(id)?
                .measured_bytes
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?
        } else {
            0
        };
        let reservation = measured.max(request.required_bytes);
        let growth = reservation.saturating_sub(measured);
        self.require_ledger_capacity(matching.is_none())?;
        if request
            .envelope
            .partition
            .path_identity
            .starts_with("same-root-fixture-v1:")
        {
            // First slice: one compatibility partition per registered root.
            // An acquire never borrows authority for generation eviction.
            // Zero-growth reuse stays admissible over an OS-bound overhang.
            if self.ledger.slots.iter().any(|slot| {
                slot.partition
                    .path_identity
                    .starts_with("same-root-fixture-v1:")
                    && slot.partition != request.envelope.partition
            }) || (growth > 0
                && self.accounted_bytes()?.saturating_add(growth) > self.ledger.budget.cap_bytes)
            {
                return Err(StorageError::Refused(RefusalReason::BudgetExceeded));
            }
            let free = self
                .free_space
                .available_bytes(&self.root)?
                .ok_or(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))?;
            if free < self.ledger.budget.free_reserve_bytes.saturating_add(growth) {
                return Err(StorageError::Refused(RefusalReason::FreeSpaceReserve));
            }
        } else {
            self.make_room(growth, matching.as_deref())?;
            self.require_free_space(growth, matching.as_deref())?;
        }

        let lease_id = random_hex(16)?;
        let token = random_hex(32)?;
        let token_hash = hash_token(&token);
        let persisted_request = persisted_request(&request);
        let (slot_id, reused) = match matching {
            Some(id) => (id, true),
            None => {
                let id = format!("slot-{:016x}", self.ledger.next_slot);
                self.ledger.next_slot = self
                    .ledger
                    .next_slot
                    .checked_add(1)
                    .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
                let slot = Slot {
                    id: id.clone(),
                    partition: request.envelope.partition.clone(),
                    state: InternalSlotState::Preparing,
                    measured_bytes: Some(0),
                    reserved_bytes: 0,
                    lease_id: None,
                    last_used: self.take_sequence()?,
                    ordinary_cargo: None,
                };
                self.prepare_slot(slot)?;
                (id, false)
            }
        };

        let last_used = self.take_sequence()?;
        let slot = self.slot_by_id_mut(&slot_id)?;
        slot.state = InternalSlotState::Leased;
        slot.measured_bytes = Some(measured);
        slot.reserved_bytes = reservation;
        slot.lease_id = Some(lease_id.clone());
        slot.last_used = last_used;
        self.ledger.leases.push(Lease {
            id: lease_id.clone(),
            token_hash,
            slot_id,
            request_digest,
            request: persisted_request,
            status: LeaseStatus::Leased,
            reused,
            ordinary_cargo: None,
        });
        self.persist()?;
        self.live_tokens.insert(lease_id.clone(), token);
        let lease = self
            .ledger
            .leases
            .last()
            .ok_or(StorageError::Protocol("new lease was not recorded"))?;
        self.acquire_result(lease)
    }

    pub(super) fn acquire_ordinary_cargo(
        &mut self,
        request: OrdinaryCargoRequest,
    ) -> Result<OrdinaryCargoAcquireResult, StorageError> {
        self.ensure_healthy()?;
        validate_ordinary_request(&request)?;
        validate_canonical_source_root(&request.root)?;
        if !self.ordinary_request_is_authorized(&request) {
            return Err(StorageError::Unauthorized);
        }
        let request_digest = digest_encoded(&request)?;
        let persisted_ordinary = persisted_ordinary_request(&request);
        let acquire = normalized_ordinary_acquire(&request.acquire);
        let persisted_acquire = persisted_request(&acquire);

        if let Some(existing) = self.ledger.leases.iter().find(|lease| {
            same_attempt(&lease.request, &request.acquire)
                || lease.request.acquire_id == request.acquire.acquire_id
                || lease.ordinary_cargo.as_ref().is_some_and(|ordinary| {
                    ordinary.request.acquire.acquire_id == request.acquire.acquire_id
                })
        }) {
            let Some(ordinary) = existing.ordinary_cargo.as_ref() else {
                return Err(StorageError::Conflict);
            };
            if ordinary.request != persisted_ordinary || existing.request_digest != request_digest {
                return Err(StorageError::Conflict);
            }
            return self.ordinary_acquire_result(existing);
        }

        if self.ledger.leases.iter().any(|lease| {
            lease.request.acquire_id == request.acquire.acquire_id
                || lease.ordinary_cargo.as_ref().is_some_and(|ordinary| {
                    ordinary.request.acquire.acquire_id == request.acquire.acquire_id
                })
        }) {
            return Err(StorageError::Conflict);
        }

        let root_slots: Vec<String> = self
            .ledger
            .slots
            .iter()
            .filter_map(|slot| {
                slot.ordinary_cargo.as_ref().and_then(|ordinary| {
                    (same_ordinary_root_allocation(&ordinary.root, &request.root)
                        || ordinary.root.source_root == request.root.source_root
                        || ordinary.root.consumer_root_id == request.root.consumer_root_id)
                        .then(|| slot.id.clone())
                })
            })
            .collect();
        for id in &root_slots {
            let slot = self.slot_by_id(id)?;
            let ordinary = slot
                .ordinary_cargo
                .as_ref()
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            if ordinary.root != request.root {
                return Err(StorageError::Refused(
                    RefusalReason::OrdinaryCargoIncompatible,
                ));
            }
        }

        let matching = root_slots
            .iter()
            .find(|id| {
                self.slot_by_id(id)
                    .ok()
                    .and_then(|slot| slot.ordinary_cargo.as_ref())
                    .is_some_and(|ordinary| ordinary.partition == request.partition)
            })
            .cloned();

        let (slot_id, saved_reuse_state, measured, reused) = if let Some(id) = matching {
            let slot = self.slot_by_id(&id)?;
            let ordinary = slot
                .ordinary_cargo
                .as_ref()
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            if slot.partition != acquire.envelope.partition
                || ordinary.policy_version != request.acceptance.policy_version
                || ordinary.installation_epoch != request.acceptance.installation_epoch
                || ordinary.acceptance_reference != request.acceptance.acceptance_reference
                || (request.acceptance.policy_version == ORDINARY_CARGO_POLICY_VERSION
                    && ordinary.input_boundary_digest != request.acceptance.input_boundary_digest)
            {
                return Err(StorageError::Refused(
                    RefusalReason::OrdinaryCargoIncompatible,
                ));
            }
            match ordinary.reuse_state {
                CargoReuseState::Pending => {
                    return Err(StorageError::Refused(RefusalReason::OrdinaryCargoPending));
                }
                CargoReuseState::ColdOnly => {
                    return Err(StorageError::Refused(RefusalReason::OrdinaryCargoColdOnly));
                }
                CargoReuseState::Empty | CargoReuseState::Reusable => {}
            }
            if slot.state != InternalSlotState::Idle || slot.lease_id.is_some() {
                return Err(StorageError::Refused(RefusalReason::OrdinaryCargoPending));
            }
            let measured = slot
                .measured_bytes
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            let previous = ordinary.reuse_state;
            (
                id,
                previous,
                measured,
                previous == CargoReuseState::Reusable,
            )
        } else {
            if root_slots.len() >= 2 {
                return Err(StorageError::Refused(
                    RefusalReason::OrdinaryCargoPartitionLimit,
                ));
            }
            self.require_ledger_capacity(true)?;
            (String::new(), CargoReuseState::Empty, 0, false)
        };

        self.require_ledger_capacity(slot_id.is_empty())?;
        let reservation = measured.max(request.acquire.required_bytes);
        let growth = reservation
            .checked_sub(measured)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let total = self
            .accounted_bytes()?
            .checked_add(growth)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        // See make_room: zero-growth reuse of a settled generation never
        // expands the aggregate pool even when measured bytes outgrew the
        // acquire-time reservation under the OS bound.
        if growth > 0 && total > self.ledger.budget.cap_bytes {
            return Err(StorageError::Refused(RefusalReason::BudgetExceeded));
        }
        let free = self
            .free_space
            .available_bytes(&self.root)?
            .ok_or(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))?;
        let required_free = self
            .ledger
            .budget
            .free_reserve_bytes
            .checked_add(growth)
            .ok_or(StorageError::Refused(RefusalReason::FreeSpaceReserve))?;
        if free < required_free {
            return Err(StorageError::Refused(RefusalReason::FreeSpaceReserve));
        }

        let slot_id = if slot_id.is_empty() {
            let id = format!("slot-{:016x}", self.ledger.next_slot);
            self.ledger.next_slot = self
                .ledger
                .next_slot
                .checked_add(1)
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            let ordinary = OrdinaryCargoGeneration {
                root: request.root.clone(),
                partition: request.partition.clone(),
                policy_version: request.acceptance.policy_version,
                installation_epoch: request.acceptance.installation_epoch.clone(),
                acceptance_reference: request.acceptance.acceptance_reference.clone(),
                input_boundary_digest: request.acceptance.input_boundary_digest.clone(),
                reuse_state: CargoReuseState::Empty,
            };
            let last_used = self.take_sequence()?;
            self.prepare_slot(Slot {
                id: id.clone(),
                partition: acquire.envelope.partition.clone(),
                state: InternalSlotState::Preparing,
                measured_bytes: Some(0),
                reserved_bytes: 0,
                lease_id: None,
                last_used,
                ordinary_cargo: Some(ordinary),
            })?;
            id
        } else {
            slot_id
        };

        let lease_id = random_hex(16)?;
        let token = random_hex(32)?;
        let token_hash = hash_token(&token);
        let last_used = self.take_sequence()?;
        let slot = self.slot_by_id_mut(&slot_id)?;
        slot.state = InternalSlotState::Leased;
        slot.measured_bytes = Some(measured);
        slot.reserved_bytes = reservation;
        slot.lease_id = Some(lease_id.clone());
        slot.last_used = last_used;
        let ordinary = slot
            .ordinary_cargo
            .as_mut()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        ordinary.reuse_state = CargoReuseState::Pending;
        self.ledger.leases.push(Lease {
            id: lease_id.clone(),
            token_hash,
            slot_id,
            request_digest,
            request: persisted_acquire,
            status: LeaseStatus::Leased,
            reused,
            ordinary_cargo: Some(OrdinaryCargoLeaseRecord {
                request: persisted_ordinary,
                saved_reuse_state,
                finalized_reuse_state: None,
            }),
        });
        self.persist()?;
        self.live_tokens.insert(lease_id.clone(), token);
        let lease = self
            .ledger
            .leases
            .last()
            .ok_or(StorageError::Protocol("ordinary lease was not recorded"))?;
        self.ordinary_acquire_result(lease)
    }

    pub(super) fn lookup_ordinary_cargo(
        &self,
        request: OrdinaryCargoRequest,
    ) -> Result<LookupResult<OrdinaryCargoLeaseIdentity>, StorageError> {
        self.ensure_healthy()?;
        validate_ordinary_request(&request)?;
        validate_canonical_source_root(&request.root)?;
        if !self.ordinary_request_is_authorized(&request) {
            return Err(StorageError::Unauthorized);
        }
        let request_digest = digest_encoded(&request)?;
        let persisted = persisted_ordinary_request(&request);
        let lease = match self.ledger.leases.iter().find(|lease| {
            same_attempt(&lease.request, &request.acquire)
                || lease.ordinary_cargo.as_ref().is_some_and(|ordinary| {
                    ordinary.request.acquire.acquire_id == request.acquire.acquire_id
                })
        }) {
            Some(lease) => lease,
            None => {
                if self
                    .ledger
                    .leases
                    .iter()
                    .any(|lease| lease.request.acquire_id == request.acquire.acquire_id)
                {
                    return Err(StorageError::Conflict);
                }
                return Ok(LookupResult::Absent);
            }
        };
        let Some(ordinary) = lease.ordinary_cargo.as_ref() else {
            return Err(StorageError::Conflict);
        };
        if ordinary.request != persisted || lease.request_digest != request_digest {
            return Err(StorageError::Conflict);
        }
        let identity = self.ordinary_lease_identity(lease)?;
        match lease.status {
            LeaseStatus::Uncertain => Ok(LookupResult::Uncertain(Some(identity))),
            LeaseStatus::Leased | LeaseStatus::Finalized => Ok(LookupResult::Present(identity)),
        }
    }

    fn ordinary_acquire_result(
        &self,
        lease: &Lease,
    ) -> Result<OrdinaryCargoAcquireResult, StorageError> {
        let record = lease
            .ordinary_cargo
            .as_ref()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let slot = self.slot_by_id(&lease.slot_id)?;
        let generation = slot
            .ordinary_cargo
            .as_ref()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let reuse_state = if lease.status == LeaseStatus::Finalized {
            record
                .finalized_reuse_state
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?
        } else {
            generation.reuse_state
        };
        Ok(OrdinaryCargoAcquireResult {
            policy_version: record.request.acceptance.policy_version,
            root: record.request.root.clone(),
            partition: record.request.partition.clone(),
            reuse_state,
            lease: self.acquire_result(lease)?,
        })
    }

    fn ordinary_lease_identity(
        &self,
        lease: &Lease,
    ) -> Result<OrdinaryCargoLeaseIdentity, StorageError> {
        let record = lease
            .ordinary_cargo
            .as_ref()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let slot = self.slot_by_id(&lease.slot_id)?;
        let generation = slot
            .ordinary_cargo
            .as_ref()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let reuse_state = if lease.status == LeaseStatus::Finalized {
            record
                .finalized_reuse_state
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?
        } else {
            generation.reuse_state
        };
        Ok(OrdinaryCargoLeaseIdentity {
            policy_version: record.request.acceptance.policy_version,
            root: record.request.root.clone(),
            partition: record.request.partition.clone(),
            reuse_state,
            lease: self.lease_identity(lease)?,
        })
    }

    pub(super) fn lookup_acquire(
        &self,
        mut request: AcquireRequest,
    ) -> Result<LookupResult<LeaseIdentity>, StorageError> {
        self.ensure_healthy()?;
        validate_request(&request)?;
        reject_ordinary_path_identity(&request.envelope.partition.path_identity)?;
        request.envelope.partition.features.sort();
        request.envelope.partition.features.dedup();
        if !self.authority.verify_admission(&request.envelope) {
            return Err(StorageError::Unauthorized);
        }
        let request_digest = digest_encoded(&request)?;
        let same_attempt = self
            .ledger
            .leases
            .iter()
            .find(|lease| same_attempt(&lease.request, &request));
        let lease = match same_attempt {
            Some(lease) => lease,
            None => {
                if self
                    .ledger
                    .leases
                    .iter()
                    .any(|lease| lease.request.acquire_id == request.acquire_id)
                {
                    return Err(StorageError::Conflict);
                }
                return Ok(LookupResult::Absent);
            }
        };
        if lease.request != persisted_request(&request) || lease.request_digest != request_digest {
            return Err(StorageError::Conflict);
        }
        let identity = self.lease_identity(lease)?;
        match lease.status {
            LeaseStatus::Uncertain => Ok(LookupResult::Uncertain(Some(identity))),
            LeaseStatus::Leased | LeaseStatus::Finalized => Ok(LookupResult::Present(identity)),
        }
    }

    fn acquire_result(&self, lease: &Lease) -> Result<AcquireResult, StorageError> {
        let (state, access) = match lease.status {
            LeaseStatus::Leased => (SlotState::Leased, true),
            LeaseStatus::Uncertain => (SlotState::Uncertain, false),
            LeaseStatus::Finalized => (SlotState::Idle, false),
        };
        let slot_path = if access {
            Some(self.slot_path(&lease.slot_id)?)
        } else {
            None
        };
        Ok(AcquireResult {
            lease_id: lease.id.clone(),
            slot_id: lease.slot_id.clone(),
            storage_instance_id: self.ledger.storage_instance_id.clone(),
            generation: slot_generation(&lease.slot_id)?,
            state,
            slot_path,
            token: self
                .live_tokens
                .get(&lease.id)
                .cloned()
                .map(LeaseToken::new),
            reused: lease.reused,
        })
    }

    fn lease_identity(&self, lease: &Lease) -> Result<LeaseIdentity, StorageError> {
        let state = match lease.status {
            LeaseStatus::Leased => SlotState::Leased,
            LeaseStatus::Uncertain => SlotState::Uncertain,
            LeaseStatus::Finalized => SlotState::Idle,
        };
        Ok(LeaseIdentity {
            lease_id: lease.id.clone(),
            slot_id: lease.slot_id.clone(),
            storage_instance_id: self.ledger.storage_instance_id.clone(),
            generation: slot_generation(&lease.slot_id)?,
            state,
        })
    }

    pub(super) fn finalize(
        &mut self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError> {
        let operation = self.settle(
            lease_id,
            token,
            finalize_id,
            evidence,
            SettlementKind::Finalize,
            None,
        )?;
        Ok(FinalizationReceipt {
            lease_id: operation.lease_id,
            finalize_id: operation.operation_id,
            state: operation.state,
            capture_digest: operation.capture_digest,
        })
    }

    pub(super) fn lookup_finalize(
        &self,
        lease_id: &str,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<LookupResult<FinalizationReceipt>, StorageError> {
        self.ensure_healthy()?;
        validate_text(finalize_id)?;
        if !self.authority.verify_settlement(&evidence) {
            return Err(StorageError::Unauthorized);
        }
        let Some(lease) = self.ledger.leases.iter().find(|lease| lease.id == lease_id) else {
            return Ok(LookupResult::Absent);
        };
        if lease.ordinary_cargo.is_some() {
            return Err(StorageError::Conflict);
        }
        if evidence.binding != lease.request.envelope.binding {
            return Err(StorageError::Conflict);
        }
        let request_digest = digest_encoded(&evidence)?;
        let operation = self.ledger.operations.iter().find(|operation| {
            operation.lease_id == lease_id
                && operation.operation_id == finalize_id
                && operation.kind == SettlementKind::Finalize
        });
        if let Some(operation) = operation {
            if operation.request_digest != request_digest
                || operation.evidence != persisted_evidence(&evidence)
            {
                return Err(StorageError::Conflict);
            }
            let receipt = FinalizationReceipt {
                lease_id: operation.lease_id.clone(),
                finalize_id: operation.operation_id.clone(),
                state: operation.state,
                capture_digest: operation.capture_digest.clone(),
            };
            return if operation.state == SlotState::Uncertain {
                Ok(LookupResult::Uncertain(Some(receipt)))
            } else {
                Ok(LookupResult::Present(receipt))
            };
        }
        if lease.status == LeaseStatus::Uncertain {
            Ok(LookupResult::Uncertain(None))
        } else {
            Ok(LookupResult::Absent)
        }
    }

    pub(super) fn finalize_ordinary_cargo(
        &mut self,
        request: OrdinaryCargoFinalizeRequest,
    ) -> Result<OrdinaryCargoFinalizationReceipt, StorageError> {
        self.ensure_healthy()?;
        validate_ordinary_finalize_request(&request)?;
        validate_canonical_source_root(&request.request.root)?;
        let operation = self.settle(
            &request.lease_id,
            request.token.as_ref(),
            &request.finalize_id,
            request.evidence,
            SettlementKind::Finalize,
            Some(&request.request),
        )?;
        let lease = self
            .ledger
            .leases
            .iter()
            .find(|lease| lease.id == operation.lease_id)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let ordinary = lease
            .ordinary_cargo
            .as_ref()
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        let reuse_state = operation_cargo_reuse_state(&operation)?;
        Ok(OrdinaryCargoFinalizationReceipt {
            policy_version: ordinary.request.acceptance.policy_version,
            root: ordinary.request.root.clone(),
            partition: ordinary.request.partition.clone(),
            reuse_state,
            finalization: FinalizationReceipt {
                lease_id: operation.lease_id,
                finalize_id: operation.operation_id,
                state: operation.state,
                capture_digest: operation.capture_digest,
            },
        })
    }

    pub(super) fn lookup_finalize_ordinary_cargo(
        &self,
        request: OrdinaryCargoFinalizeRequest,
    ) -> Result<LookupResult<OrdinaryCargoFinalizationReceipt>, StorageError> {
        self.ensure_healthy()?;
        validate_ordinary_finalize_request(&request)?;
        validate_canonical_source_root(&request.request.root)?;
        if !self.ordinary_request_is_authorized(&request.request)
            || !self.authority.verify_settlement(&request.evidence)
            || !self
                .authority
                .verify_ordinary_cargo_settlement(&request.request, &request.evidence)
        {
            return Err(StorageError::Unauthorized);
        }
        let Some(lease) = self
            .ledger
            .leases
            .iter()
            .find(|lease| lease.id == request.lease_id)
        else {
            return Ok(LookupResult::Absent);
        };
        let Some(ordinary) = lease.ordinary_cargo.as_ref() else {
            return Err(StorageError::Conflict);
        };
        if ordinary.request != persisted_ordinary_request(&request.request)
            || lease.request_digest != digest_encoded(&request.request)?
            || request.evidence.binding != lease.request.envelope.binding
        {
            return Err(StorageError::Conflict);
        }
        let request_digest =
            ordinary_finalize_digest(&request.request, &request.finalize_id, &request.evidence)?;
        let operation = self.ledger.operations.iter().find(|operation| {
            operation.lease_id == request.lease_id
                && operation.operation_id == request.finalize_id
                && operation.kind == SettlementKind::Finalize
        });
        if let Some(operation) = operation {
            if operation.request_digest != request_digest
                || operation.evidence != persisted_evidence(&request.evidence)
            {
                return Err(StorageError::Conflict);
            }
            let reuse_state = operation_cargo_reuse_state(operation)?;
            let receipt = OrdinaryCargoFinalizationReceipt {
                policy_version: ordinary.request.acceptance.policy_version,
                root: ordinary.request.root.clone(),
                partition: ordinary.request.partition.clone(),
                reuse_state,
                finalization: FinalizationReceipt {
                    lease_id: operation.lease_id.clone(),
                    finalize_id: operation.operation_id.clone(),
                    state: operation.state,
                    capture_digest: operation.capture_digest.clone(),
                },
            };
            return if operation.state == SlotState::Uncertain {
                Ok(LookupResult::Uncertain(Some(receipt)))
            } else {
                Ok(LookupResult::Present(receipt))
            };
        }
        if lease.status == LeaseStatus::Uncertain {
            Ok(LookupResult::Uncertain(None))
        } else {
            Ok(LookupResult::Absent)
        }
    }

    pub(super) fn reconcile(
        &mut self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        reconcile_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<ReconcileReceipt, StorageError> {
        if self
            .ledger
            .leases
            .iter()
            .any(|lease| lease.id == lease_id && lease.ordinary_cargo.is_some())
        {
            return Err(StorageError::Conflict);
        }
        let operation = self.settle(
            lease_id,
            token,
            reconcile_id,
            evidence,
            SettlementKind::Reconcile,
            None,
        )?;
        Ok(ReconcileReceipt {
            lease_id: operation.lease_id,
            reconcile_id: operation.operation_id,
            state: operation.state,
        })
    }

    fn settle(
        &mut self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        operation_id: &str,
        evidence: SettlementEvidence,
        kind: SettlementKind,
        ordinary_request: Option<&OrdinaryCargoRequest>,
    ) -> Result<SettlementOperation, StorageError> {
        self.ensure_healthy()?;
        validate_text(operation_id)?;
        let lease = self
            .ledger
            .leases
            .iter()
            .find(|lease| lease.id == lease_id)
            .cloned()
            .ok_or(StorageError::Refused(RefusalReason::LeaseUnavailable))?;
        match (&lease.ordinary_cargo, ordinary_request) {
            (None, None) => {}
            (Some(ordinary), Some(request))
                if ordinary.request == persisted_ordinary_request(request)
                    && lease.request
                        == persisted_request(&normalized_ordinary_acquire(&request.acquire))
                    && kind == SettlementKind::Finalize =>
            {
                validate_ordinary_request(request)?;
                if !self.ordinary_request_is_authorized(request)
                    || !self
                        .authority
                        .verify_ordinary_cargo_settlement(request, &evidence)
                {
                    return Err(StorageError::Unauthorized);
                }
            }
            _ => return Err(StorageError::Conflict),
        }
        if evidence.binding != lease.request.envelope.binding {
            return Err(StorageError::Conflict);
        }
        let previous = self
            .ledger
            .operations
            .iter()
            .find(|previous| {
                previous.lease_id == lease_id
                    && previous.operation_id == operation_id
                    && previous.kind == kind
            })
            .cloned();
        match token {
            Some(token) if hash_token(token.value()) == lease.token_hash => {}
            None if lease.status == LeaseStatus::Uncertain || previous.is_some() => {}
            _ => return Err(StorageError::Refused(RefusalReason::LeaseUnavailable)),
        }
        if !self.authority.verify_settlement(&evidence) {
            return Err(StorageError::Unauthorized);
        }
        let request_digest = match ordinary_request {
            Some(request) => ordinary_finalize_digest(request, operation_id, &evidence)?,
            None => digest_encoded(&evidence)?,
        };
        let persisted_evidence = persisted_evidence(&evidence);
        if let Some(previous) = previous {
            if previous.evidence != persisted_evidence || previous.request_digest != request_digest
            {
                return Err(StorageError::Conflict);
            }
            return Ok(previous.clone());
        }
        if lease.status == LeaseStatus::Finalized {
            return Err(StorageError::Refused(RefusalReason::LeaseUnavailable));
        }
        // A settlement that cannot be recorded durably must not half-apply.
        // Refusal keeps the lease exactly as it was, so ownership stays held.
        self.require_operation_capacity()?;
        let slot_id = lease.slot_id.clone();
        let capture_digest = evidence
            .capture
            .as_ref()
            .map(|capture| capture.digest.clone());
        let expected_slot_path = self.slot_path(&slot_id)?;
        let proof_complete = release_proven(
            &evidence,
            &lease,
            token,
            &expected_slot_path.to_string_lossy(),
        );
        let mut new_state = SlotState::Uncertain;
        let mut cargo_reuse_state = None;
        let measured = if proof_complete {
            let slot_path = self.slot_path(&slot_id)?;
            measure_tree_owned(&self.slots_root, &slot_path).ok()
        } else {
            None
        };
        if let Some(bytes) = measured {
            let last_used = self.take_sequence()?;
            {
                let slot = self.slot_by_id_mut(&slot_id)?;
                slot.state = InternalSlotState::Idle;
                slot.lease_id = None;
                slot.measured_bytes = Some(bytes);
                slot.reserved_bytes = bytes;
                slot.last_used = last_used;
                if ordinary_request.is_some() {
                    let reuse_state =
                        if matches!(&evidence.containment, ContainmentEvidence::NotSpawned(_)) {
                            lease
                                .ordinary_cargo
                                .as_ref()
                                .map(|ordinary| ordinary.saved_reuse_state)
                                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?
                        } else {
                            match evidence.terminal.as_ref().map(|terminal| terminal.outcome) {
                                Some(TerminalOutcome::Passed) => CargoReuseState::Reusable,
                                Some(TerminalOutcome::Failed) => CargoReuseState::ColdOnly,
                                _ => {
                                    return Err(StorageError::Refused(
                                        RefusalReason::UnknownAccounting,
                                    ));
                                }
                            }
                        };
                    slot.ordinary_cargo
                        .as_mut()
                        .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?
                        .reuse_state = reuse_state;
                    cargo_reuse_state = Some(reuse_state);
                }
            }
            new_state = SlotState::Idle;
            let stored = self
                .ledger
                .leases
                .iter_mut()
                .find(|stored| stored.id == lease_id)
                .ok_or(StorageError::Protocol(
                    "lease disappeared during settlement",
                ))?;
            stored.status = LeaseStatus::Finalized;
            if let (Some(reuse_state), Some(ordinary)) =
                (cargo_reuse_state, stored.ordinary_cargo.as_mut())
            {
                ordinary.finalized_reuse_state = Some(reuse_state);
            }
        } else {
            {
                let slot = self.slot_by_id_mut(&slot_id)?;
                slot.state = InternalSlotState::Uncertain;
                slot.measured_bytes = None;
                slot.lease_id = Some(lease_id.to_owned());
            }
            let stored = self
                .ledger
                .leases
                .iter_mut()
                .find(|stored| stored.id == lease_id)
                .ok_or(StorageError::Protocol(
                    "lease disappeared during settlement",
                ))?;
            stored.status = LeaseStatus::Uncertain;
        }

        let operation = SettlementOperation {
            lease_id: lease_id.to_owned(),
            operation_id: operation_id.to_owned(),
            kind,
            evidence: persisted_evidence,
            request_digest,
            state: new_state,
            capture_digest,
            cargo_reuse_state,
        };
        self.ledger.operations.push(operation.clone());
        self.persist()?;
        if new_state == SlotState::Idle {
            self.live_tokens.remove(lease_id);
        }
        Ok(operation)
    }

    pub(super) fn evict(&mut self, target_bytes: u64) -> Result<u64, StorageError> {
        self.ensure_healthy()?;
        if target_bytes == 0 {
            return Ok(0);
        }
        let mut freed = 0_u64;
        while freed < target_bytes {
            let candidate = self
                .ledger
                .slots
                .iter()
                .filter(|slot| {
                    slot.state == InternalSlotState::Idle
                        && slot.ordinary_cargo.is_none()
                        && !slot
                            .partition
                            .path_identity
                            .starts_with("same-root-fixture-v1:")
                        && slot.lease_id.is_none()
                        && slot.measured_bytes.is_some()
                })
                .min_by_key(|slot| slot.last_used)
                .map(|slot| slot.id.clone());
            let id = match candidate {
                Some(id) => id,
                None => break,
            };
            let bytes = self.evict_slot(&id)?;
            freed = freed.saturating_add(bytes);
        }
        Ok(freed)
    }

    fn evict_slot(&mut self, id: &str) -> Result<u64, StorageError> {
        let slot = self.slot_by_id(id)?;
        if slot
            .partition
            .path_identity
            .starts_with("same-root-fixture-v1:")
            || slot.ordinary_cargo.is_some()
            || slot.state != InternalSlotState::Idle
            || slot.lease_id.is_some()
        {
            return Err(StorageError::Refused(RefusalReason::NoIdleStorage));
        }
        let bytes = slot
            .measured_bytes
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        self.slot_by_id_mut(id)?.state = InternalSlotState::Evicting;
        self.persist()?;
        let path = self.slot_path(id)?;
        remove_tree_owned(&self.slots_root, &path).map_err(|error| {
            self.poisoned = true;
            error
        })?;
        self.ledger.slots.retain(|slot| slot.id != id);
        self.persist()?;
        Ok(bytes)
    }

    fn make_room(&mut self, growth: u64, protected: Option<&str>) -> Result<(), StorageError> {
        let mut accounted = self.accounted_bytes()?;
        // Measured payload may legitimately exceed acquire-time reservations:
        // the OS bound sits at the verified volume capacity, above the
        // reservation ceiling. Zero-growth reuse never expands the pool, so
        // only real growth evicts or refuses.
        while growth > 0 && accounted.saturating_add(growth) > self.ledger.budget.cap_bytes {
            let candidate = self
                .ledger
                .slots
                .iter()
                .filter(|slot| {
                    slot.state == InternalSlotState::Idle
                        && slot.ordinary_cargo.is_none()
                        && !slot
                            .partition
                            .path_identity
                            .starts_with("same-root-fixture-v1:")
                        && slot.lease_id.is_none()
                        && slot.measured_bytes.is_some()
                        && Some(slot.id.as_str()) != protected
                })
                .min_by_key(|slot| slot.last_used)
                .map(|slot| slot.id.clone());
            let id = match candidate {
                Some(id) => id,
                None => return Err(StorageError::Refused(RefusalReason::BudgetExceeded)),
            };
            self.evict_slot(&id)?;
            accounted = self.accounted_bytes()?;
        }
        Ok(())
    }

    fn require_free_space(
        &mut self,
        growth: u64,
        protected: Option<&str>,
    ) -> Result<(), StorageError> {
        let free = self
            .free_space
            .available_bytes(&self.root)?
            .ok_or(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))?;
        let required = self
            .ledger
            .budget
            .free_reserve_bytes
            .checked_add(growth)
            .ok_or(StorageError::Refused(RefusalReason::FreeSpaceReserve))?;
        let mut available = free;
        while available < required {
            let candidate = self
                .ledger
                .slots
                .iter()
                .filter(|slot| {
                    slot.state == InternalSlotState::Idle
                        && slot.ordinary_cargo.is_none()
                        && !slot
                            .partition
                            .path_identity
                            .starts_with("same-root-fixture-v1:")
                        && slot.lease_id.is_none()
                        && slot.measured_bytes.is_some()
                        && Some(slot.id.as_str()) != protected
                })
                .min_by_key(|slot| slot.last_used)
                .map(|slot| slot.id.clone());
            let id = match candidate {
                Some(id) => id,
                None => return Err(StorageError::Refused(RefusalReason::FreeSpaceReserve)),
            };
            self.evict_slot(&id)?;
            available = self
                .free_space
                .available_bytes(&self.root)?
                .ok_or(StorageError::Refused(RefusalReason::FreeSpaceUnavailable))?;
        }
        Ok(())
    }

    fn accounted_bytes(&self) -> Result<u64, StorageError> {
        self.ledger.slots.iter().try_fold(0_u64, |sum, slot| {
            let measured = slot
                .measured_bytes
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
            sum.checked_add(measured.max(slot.reserved_bytes))
                .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))
        })
    }

    /// Refuses when a finite control-history limit is reached. Exhaustion keeps
    /// every recorded lease and settlement intact and refuses new control
    /// records instead of pruning required evidence or growing indefinitely.
    fn require_ledger_capacity(&self, new_slot: bool) -> Result<(), StorageError> {
        if (new_slot && self.ledger.slots.len() >= MAX_LEDGER_SLOTS)
            || self.ledger.leases.len() >= MAX_LEDGER_LEASES
        {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        Ok(())
    }

    fn require_operation_capacity(&self) -> Result<(), StorageError> {
        if self.ledger.operations.len() >= MAX_LEDGER_OPERATIONS {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        Ok(())
    }

    /// Records one preparing slot and creates its directory. On a bounded
    /// payload that is already full at the OS level, the record rolls back so
    /// the store stays healthy and the caller receives the actionable
    /// admission refusal instead of a poisoned daemon.
    fn prepare_slot(&mut self, slot: Slot) -> Result<String, StorageError> {
        let id = slot.id.clone();
        let bounded = backend_is_bounded(&self.ledger.backend);
        self.ledger.slots.push(slot);
        self.persist()?;
        let path = self.slot_path(&id)?;
        if let Err(error) = fs::create_dir(&path) {
            if bounded && error.kind() == std::io::ErrorKind::StorageFull {
                self.ledger.slots.retain(|record| record.id != id);
                self.persist()?;
                return Err(StorageError::Refused(RefusalReason::BudgetExceeded));
            }
            self.poisoned = true;
            return Err(io_error(error));
        }
        if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o700)) {
            self.poisoned = true;
            return Err(io_error(error));
        }
        if let Err(error) = File::open(&self.slots_root).and_then(|d| d.sync_all()) {
            self.poisoned = true;
            return Err(io_error(error));
        }
        Ok(id)
    }

    fn ensure_healthy(&self) -> Result<(), StorageError> {
        if self.poisoned {
            Err(StorageError::Refused(RefusalReason::UnknownAccounting))
        } else {
            Ok(())
        }
    }

    fn ordinary_request_is_authorized(&self, request: &OrdinaryCargoRequest) -> bool {
        self.authority.verify_admission(&request.acquire.envelope)
            && self.authority.verify_ordinary_cargo_admission(request)
            && (request.acceptance.policy_version != ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
                || self
                    .authority
                    .verify_ordinary_cargo_stable_reuse_policy(request))
    }

    fn slot_by_id(&self, id: &str) -> Result<&Slot, StorageError> {
        self.ledger
            .slots
            .iter()
            .find(|slot| slot.id == id)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))
    }

    fn slot_by_id_mut(&mut self, id: &str) -> Result<&mut Slot, StorageError> {
        self.ledger
            .slots
            .iter_mut()
            .find(|slot| slot.id == id)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))
    }

    fn slot_path(&self, id: &str) -> Result<PathBuf, StorageError> {
        if !valid_slot_id(id) {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        let path = self.slots_root.join(id);
        if path_exists(&path)? {
            let canonical = fs::canonicalize(&path).map_err(io_error)?;
            if canonical.parent() != Some(self.slots_root.as_path()) {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
            let meta = fs::symlink_metadata(&path).map_err(io_error)?;
            if !meta.file_type().is_dir() {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
        }
        Ok(path)
    }

    fn take_sequence(&mut self) -> Result<u64, StorageError> {
        let sequence = self.ledger.next_sequence;
        self.ledger.next_sequence = sequence
            .checked_add(1)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
        Ok(sequence)
    }

    fn persist(&mut self) -> Result<(), StorageError> {
        match persist_ledger(&self.root, &self.ledger_path, &self.ledger) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }
}

fn validate_request(request: &AcquireRequest) -> Result<(), StorageError> {
    let binding = &request.envelope.binding;
    let partition = &request.envelope.partition;
    for value in [
        binding.consumer.as_str(),
        binding.project.as_str(),
        binding.component.as_str(),
        binding.workflow.as_str(),
        binding.task_id.as_str(),
        binding.workspace_id.as_str(),
        binding.run_id.as_str(),
        binding.attempt_id.as_str(),
        binding.commit.as_str(),
        binding.source_tree_digest.as_str(),
        partition.project_component.as_str(),
        partition.workflow.as_str(),
        partition.cargo_provenance.as_str(),
        partition.rustc_provenance.as_str(),
        partition.host.as_str(),
        partition.target.as_str(),
        partition.profile.as_str(),
        partition.config_digest.as_str(),
        partition.environment_digest.as_str(),
        partition.path_identity.as_str(),
        request.envelope.admission_reference.as_str(),
        request.acquire_id.as_str(),
    ] {
        validate_text(value)?;
    }
    validate_text(request.envelope.admission_credential.expose())?;
    if request.required_bytes == 0 {
        return Err(StorageError::InvalidInput(
            "slot reservation must be positive",
        ));
    }
    if partition.project_component != format!("{}:{}", binding.project, binding.component) {
        return Err(StorageError::InvalidInput(
            "project/component key does not match run binding",
        ));
    }
    if partition.workflow != binding.workflow {
        return Err(StorageError::InvalidInput(
            "workflow key does not match run binding",
        ));
    }
    for pair in partition.features.windows(2) {
        if pair[0] == pair[1] {
            return Err(StorageError::InvalidInput(
                "partition contains duplicate features",
            ));
        }
    }
    for feature in &partition.features {
        validate_text(feature)?;
    }
    Ok(())
}

fn validate_ordinary_request(request: &OrdinaryCargoRequest) -> Result<(), StorageError> {
    validate_request(&request.acquire)?;
    if !matches!(
        &request.partition,
        OrdinaryCargoPartition::OrdinaryTest | OrdinaryCargoPartition::FullDebug
    ) {
        return Err(StorageError::Refused(
            RefusalReason::OrdinaryCargoPartitionLimit,
        ));
    }
    if !supported_ordinary_cargo_policy_version(request.acceptance.policy_version) {
        return Err(StorageError::InvalidInput(
            "unsupported ordinary Cargo policy version",
        ));
    }
    if request.acquire.envelope.partition.path_identity != ORDINARY_CARGO_PATH_IDENTITY {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo path identity must use the versioned storage route",
        ));
    }
    validate_text(&request.root.consumer_root_id)?;
    validate_text(&request.root.allocation_epoch)?;
    validate_text(&request.acceptance.installation_epoch)?;
    validate_text(&request.acceptance.acceptance_reference)?;
    validate_text(request.acceptance.credential.expose())?;
    let mut features = std::collections::BTreeSet::new();
    if request
        .acquire
        .envelope
        .partition
        .features
        .iter()
        .any(|feature| !features.insert(feature.as_str()))
    {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo partition contains duplicate features",
        ));
    }
    if !valid_digest(&request.acceptance.input_boundary_digest) {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo input boundary must be a SHA-256 digest",
        ));
    }
    if !request.root.source_root.is_absolute() {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo source root must be absolute",
        ));
    }
    if request.acceptance.policy_version == ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
        && !ordinary_request_stable_reference_is_valid(request)
    {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo stable reuse reference does not match its compatibility identity",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct StableReuseReferenceMaterial<'a> {
    schema: &'static str,
    policy_version: u32,
    source_root_path_hex: String,
    consumer_root_id: &'a str,
    allocation_epoch: &'a str,
    ordinary_partition: &'a str,
    project_component: &'a str,
    workflow: &'a str,
    cargo_provenance: &'a str,
    rustc_provenance: &'a str,
    host: &'a str,
    target: &'a str,
    profile: &'a str,
    features: Vec<&'a str>,
    config_digest: &'a str,
    environment_digest: &'a str,
    path_identity: &'a str,
    installation_epoch: &'a str,
    qualified_invalidation_declaration_digest: &'a str,
}

/// Builds the typed v2 acceptance reference for an installed compatibility
/// and invalidation policy. Storage recomputes this value and still requires
/// the accepting authority to approve it.
pub fn ordinary_cargo_stable_reuse_reference(
    request: &OrdinaryCargoRequest,
    qualified_invalidation_declaration_digest: &str,
) -> Result<String, StorageError> {
    if request.acceptance.policy_version != ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION {
        return Err(StorageError::InvalidInput(
            "stable reuse references require ordinary Cargo policy version 2",
        ));
    }
    stable_reuse_reference_for_parts(
        &request.root,
        &request.acquire.envelope.partition,
        &request.partition,
        request.acceptance.policy_version,
        &request.acceptance.installation_epoch,
        qualified_invalidation_declaration_digest,
    )
}

fn stable_reuse_reference_for_parts(
    root: &StrictPhysicalCargoRoot,
    cargo_partition: &PartitionKey,
    ordinary_partition: &OrdinaryCargoPartition,
    policy_version: u32,
    installation_epoch: &str,
    qualified_invalidation_declaration_digest: &str,
) -> Result<String, StorageError> {
    if policy_version != ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
        || !canonical_sha256_digest(qualified_invalidation_declaration_digest)
    {
        return Err(StorageError::InvalidInput(
            "stable reuse reference requires policy v2 and a canonical invalidation digest",
        ));
    }
    let mut features: Vec<_> = cargo_partition
        .features
        .iter()
        .map(String::as_str)
        .collect();
    features.sort_unstable();
    let material = StableReuseReferenceMaterial {
        schema: "oxide.ordinary-cargo.stable-reuse.v2",
        policy_version,
        source_root_path_hex: digest_hex(root.source_root.as_os_str().as_bytes()),
        consumer_root_id: &root.consumer_root_id,
        allocation_epoch: &root.allocation_epoch,
        ordinary_partition: ordinary_partition.label(),
        project_component: &cargo_partition.project_component,
        workflow: &cargo_partition.workflow,
        cargo_provenance: &cargo_partition.cargo_provenance,
        rustc_provenance: &cargo_partition.rustc_provenance,
        host: &cargo_partition.host,
        target: &cargo_partition.target,
        profile: &cargo_partition.profile,
        features,
        config_digest: &cargo_partition.config_digest,
        environment_digest: &cargo_partition.environment_digest,
        path_identity: &cargo_partition.path_identity,
        installation_epoch,
        qualified_invalidation_declaration_digest,
    };
    let encoded = serde_json::to_vec(&material)
        .map_err(|_| StorageError::Protocol("stable reuse identity encoding failed"))?;
    let compatibility_digest = format!("sha256:{}", digest_hex(&Sha256::digest(encoded)));
    Ok(format!(
        "oxide-ordinary-cargo-stable-eligibility-v2/{qualified_invalidation_declaration_digest}/{compatibility_digest}"
    ))
}

fn ordinary_request_stable_reference_is_valid(request: &OrdinaryCargoRequest) -> bool {
    let Some(invalidation_digest) =
        invalidation_digest_from_stable_reference(&request.acceptance.acceptance_reference)
    else {
        return false;
    };
    ordinary_cargo_stable_reuse_reference(request, invalidation_digest)
        .is_ok_and(|expected| expected == request.acceptance.acceptance_reference)
}

fn ordinary_generation_stable_reference_is_valid(
    generation: &OrdinaryCargoGeneration,
    cargo_partition: &PartitionKey,
) -> bool {
    match generation.policy_version {
        ORDINARY_CARGO_POLICY_VERSION => true,
        ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION => {
            let Some(invalidation_digest) =
                invalidation_digest_from_stable_reference(&generation.acceptance_reference)
            else {
                return false;
            };
            stable_reuse_reference_for_parts(
                &generation.root,
                cargo_partition,
                &generation.partition,
                generation.policy_version,
                &generation.installation_epoch,
                invalidation_digest,
            )
            .is_ok_and(|expected| expected == generation.acceptance_reference)
        }
        _ => false,
    }
}

fn ordinary_generation_matches_request(
    generation: &OrdinaryCargoGeneration,
    request: &OrdinaryCargoRequest,
) -> bool {
    generation.root == request.root
        && generation.partition == request.partition
        && generation.policy_version == request.acceptance.policy_version
        && generation.installation_epoch == request.acceptance.installation_epoch
        && generation.acceptance_reference == request.acceptance.acceptance_reference
        && (request.acceptance.policy_version == ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
            || generation.input_boundary_digest == request.acceptance.input_boundary_digest)
}

fn supported_ordinary_cargo_policy_version(policy_version: u32) -> bool {
    matches!(
        policy_version,
        ORDINARY_CARGO_POLICY_VERSION | ORDINARY_CARGO_STABLE_REUSE_POLICY_VERSION
    )
}

fn invalidation_digest_from_stable_reference(reference: &str) -> Option<&str> {
    let identity = reference.strip_prefix("oxide-ordinary-cargo-stable-eligibility-v2/")?;
    let (invalidation_digest, compatibility_digest) = identity.split_once('/')?;
    if canonical_sha256_digest(invalidation_digest) && canonical_sha256_digest(compatibility_digest)
    {
        Some(invalidation_digest)
    } else {
        None
    }
}

fn canonical_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn validate_ordinary_finalize_request(
    request: &OrdinaryCargoFinalizeRequest,
) -> Result<(), StorageError> {
    validate_ordinary_request(&request.request)?;
    validate_text(&request.lease_id)?;
    validate_text(&request.finalize_id)?;
    if request.evidence.binding != request.request.acquire.envelope.binding {
        return Err(StorageError::Conflict);
    }
    if let Some(token) = request.token.as_ref() {
        validate_text(token.value())?;
    }
    Ok(())
}

fn validate_canonical_source_root(root: &StrictPhysicalCargoRoot) -> Result<(), StorageError> {
    let canonical = fs::canonicalize(&root.source_root).map_err(io_error)?;
    if canonical != root.source_root {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo source root must be the canonical physical path",
        ));
    }
    let metadata = fs::symlink_metadata(&root.source_root).map_err(io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(StorageError::InvalidInput(
            "ordinary Cargo source root must be a directory",
        ));
    }
    Ok(())
}

fn reject_ordinary_path_identity(path_identity: &str) -> Result<(), StorageError> {
    if path_identity == ORDINARY_CARGO_PATH_IDENTITY
        || path_identity.starts_with("ordinary-cargo-storage-")
    {
        return Err(StorageError::Conflict);
    }
    Ok(())
}

fn same_ordinary_root_allocation(
    left: &StrictPhysicalCargoRoot,
    right: &StrictPhysicalCargoRoot,
) -> bool {
    left.source_root == right.source_root
        && left.consumer_root_id == right.consumer_root_id
        && left.allocation_epoch == right.allocation_epoch
}

fn persisted_ordinary_request(request: &OrdinaryCargoRequest) -> OrdinaryCargoRequest {
    let mut request = request.clone();
    request.acquire = persisted_request(&request.acquire);
    request.acceptance.credential = Credential::new("verified");
    request
}

fn normalized_ordinary_acquire(request: &AcquireRequest) -> AcquireRequest {
    let mut request = request.clone();
    request.envelope.partition.features.sort();
    request.envelope.partition.features.dedup();
    request.envelope.partition.path_identity = ORDINARY_CARGO_PATH_IDENTITY.to_owned();
    request
}

fn ordinary_finalize_digest(
    request: &OrdinaryCargoRequest,
    finalize_id: &str,
    evidence: &SettlementEvidence,
) -> Result<String, StorageError> {
    let mut writer = Writer::new();
    request.encode(&mut writer)?;
    finalize_id.to_owned().encode(&mut writer)?;
    evidence.encode(&mut writer)?;
    Ok(format!("{:x}", Sha256::digest(writer.finish())))
}

fn operation_cargo_reuse_state(
    operation: &SettlementOperation,
) -> Result<CargoReuseState, StorageError> {
    match (operation.state, operation.cargo_reuse_state) {
        (SlotState::Idle, Some(state)) => Ok(state),
        (SlotState::Uncertain, None) => Ok(CargoReuseState::Pending),
        _ => Err(StorageError::Refused(RefusalReason::UnknownAccounting)),
    }
}

fn validate_text(value: &str) -> Result<(), StorageError> {
    if value.is_empty() || value.len() > 64 * 1024 || value.as_bytes().contains(&0) {
        return Err(StorageError::InvalidInput(
            "text fields must be non-empty and bounded",
        ));
    }
    Ok(())
}

fn same_attempt(left: &AcquireRequest, right: &AcquireRequest) -> bool {
    let a = &left.envelope.binding;
    let b = &right.envelope.binding;
    a.consumer == b.consumer && a.run_id == b.run_id && a.attempt_id == b.attempt_id
}

fn persisted_request(request: &AcquireRequest) -> AcquireRequest {
    let mut request = request.clone();
    request.envelope.admission_credential = Credential::new("verified");
    request
}

fn persisted_evidence(evidence: &SettlementEvidence) -> SettlementEvidence {
    let mut evidence = evidence.clone();
    evidence.consumer_credential = Credential::new("verified");
    if let Some(terminal) = evidence.terminal.as_mut() {
        terminal.credential = Credential::new("verified");
    }
    if let Some(capture) = evidence.capture.as_mut() {
        capture.credential = Credential::new("verified");
    }
    if let ContainmentEvidence::CooperativeWorkflow(completion) = &mut evidence.containment {
        completion.signature = "verified".to_owned();
    }
    evidence
}

fn release_proven(
    evidence: &SettlementEvidence,
    lease: &Lease,
    token: Option<&LeaseToken>,
    expected_slot_path: &str,
) -> bool {
    if let (ContainmentEvidence::NotSpawned(proof), Some(token)) = (&evidence.containment, token) {
        return lease.status == LeaseStatus::Leased
            && evidence.capture.is_none()
            && evidence.terminal.is_none()
            && proof.lease_id == lease.id
            && proof.slot_id == lease.slot_id
            && proof.slot_path == expected_slot_path
            && proof.run_id == evidence.binding.run_id
            && proof.attempt_id == evidence.binding.attempt_id
            && proof.verify(token);
    }
    let terminal_matches = evidence
        .terminal
        .as_ref()
        .map(|terminal| {
            terminal.run_id == evidence.binding.run_id
                && terminal.outcome != TerminalOutcome::Cancelled
        })
        .unwrap_or(false);
    let capture_complete = evidence
        .capture
        .as_ref()
        .map(|capture| {
            valid_id(&capture.reference)
                && valid_digest(&capture.digest)
                && valid_id(&capture.host_run_id)
                && capture.run_id == evidence.binding.run_id
                && capture.attempt_id == evidence.binding.attempt_id
        })
        .unwrap_or(false);
    let containment_complete = match (&evidence.containment, token) {
        (ContainmentEvidence::CooperativeWorkflow(completion), Some(token)) => {
            let process_ids: std::collections::BTreeSet<_> = completion
                .processes
                .iter()
                .map(|process| process.pid)
                .collect();
            completion.lease_id == lease.id
                && completion.slot_id == lease.slot_id
                && completion.slot_path == expected_slot_path
                && !completion.temp_dir.is_empty()
                && !completion.capture_dir.is_empty()
                && Path::new(&completion.temp_dir).is_absolute()
                && Path::new(&completion.capture_dir).is_absolute()
                && completion.workflow == evidence.binding.workflow
                && completion.host_run_id
                    == evidence
                        .capture
                        .as_ref()
                        .map_or("", |capture| capture.host_run_id.as_str())
                && completion.run_id == evidence.binding.run_id
                && completion.attempt_id == evidence.binding.attempt_id
                && valid_id(&completion.host_run_id)
                && !completion.workflow_definition_digest.is_empty()
                && !completion.repository_head.is_empty()
                && !completion.fixture_source_digest.is_empty()
                && !completion.stdout_digest.is_empty()
                && !completion.stderr_digest.is_empty()
                && valid_id(&completion.capture_reference)
                && completion.capture_reference
                    == evidence
                        .capture
                        .as_ref()
                        .map_or("", |capture| capture.reference.as_str())
                && completion.capture_digest
                    == evidence
                        .capture
                        .as_ref()
                        .map_or("", |capture| capture.digest.as_str())
                && !completion.processes.is_empty()
                && completion.processes.len() <= 8
                && process_ids.len() == completion.processes.len()
                && !completion.free_space_samples.is_empty()
                && completion.free_space_samples.len() <= 16
                && completion
                    .free_space_samples
                    .iter()
                    .all(|sample| *sample >= 20 * 1024 * 1024 * 1024)
                && completion
                    .processes
                    .iter()
                    .all(|process| process.pid > 0 && valid_digest(&process.start_identity))
                && completion.exit_code >= 0
                && evidence.terminal.as_ref().is_some_and(|terminal| {
                    (completion.exit_code == 0 && terminal.outcome == TerminalOutcome::Passed)
                        || (completion.exit_code > 0 && terminal.outcome == TerminalOutcome::Failed)
                })
                && valid_digest(&completion.workflow_definition_digest)
                && valid_repository_head(&completion.repository_head)
                && valid_digest(&completion.fixture_source_digest)
                && valid_digest(&completion.stdout_digest)
                && valid_digest(&completion.stderr_digest)
                && completion.verify(token)
        }
        _ => false,
    };
    terminal_matches && capture_complete && containment_complete
}

fn valid_digest(value: &str) -> bool {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_repository_head(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.as_bytes().contains(&0)
}

fn validate_root(path: &Path) -> Result<PathBuf, StorageError> {
    if !path.is_absolute() {
        return Err(StorageError::InvalidInput("storage root must be absolute"));
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(StorageError::InvalidInput(
            "storage root must be supplied by its canonical path",
        ));
    }
    let metadata = fs::symlink_metadata(&canonical).map_err(io_error)?;
    if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(StorageError::InvalidInput(
            "storage root must be a private directory",
        ));
    }
    Ok(canonical)
}

fn ensure_private_directory(path: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.permissions().mode() & 0o077 != 0 {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(io_error)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
            if let Some(parent) = path.parent() {
                File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)?;
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if canonical != path {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    Ok(())
}

fn backend_is_bounded(backend: &LedgerBackend) -> bool {
    matches!(backend, LedgerBackend::Bounded(_))
}

/// Directory roots may hold the slots directory; bounded roots keep slots on
/// the payload volume, so the daemon root must not contain one.
fn validate_root_entries(root: &Path, bounded: bool) -> Result<(), StorageError> {
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "ledger.bin" || name == SOCKET_NAME {
            continue;
        }
        if is_ledger_temp_name(&name) {
            continue;
        }
        if !bounded && name == SLOTS_NAME {
            continue;
        }
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    Ok(())
}

fn remove_stale_ledger_temps(root: &Path) -> Result<(), StorageError> {
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("ledger.tmp-") {
            continue;
        }
        if !is_ledger_temp_name(&name) {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(io_error)?;
        if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
        }
        fs::remove_file(entry.path()).map_err(io_error)?;
    }
    Ok(())
}

fn is_ledger_temp_name(name: &str) -> bool {
    name.strip_prefix("ledger.tmp-")
        .map(|suffix| suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn remove_stale_socket(path: &Path) -> Result<(), StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };
    if !metadata.file_type().is_socket() {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    match UnixStream::connect(path) {
        Ok(_) => return Err(StorageError::Refused(RefusalReason::AlreadyRunning)),
        Err(error)
            if error.kind() == std::io::ErrorKind::ConnectionRefused
                || error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let current = fs::symlink_metadata(path).map_err(io_error)?;
    if !current.file_type().is_socket()
        || current.dev() != metadata.dev()
        || current.ino() != metadata.ino()
    {
        return Err(StorageError::Refused(RefusalReason::AlreadyRunning));
    }
    fs::remove_file(path).map_err(io_error)
}

fn remove_owned_socket(path: &Path, device: u64, inode: u64) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_socket() && metadata.dev() == device && metadata.ino() == inode {
            let _ = fs::remove_file(path);
        }
    }
}

fn path_exists(path: &Path) -> Result<bool, StorageError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

fn measure_tree_owned(root: &Path, path: &Path) -> Result<u64, StorageError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Ok(metadata.len());
    }
    if metadata.file_type().is_file() {
        return Ok(metadata.len());
    }
    if !metadata.file_type().is_dir() {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if !canonical.starts_with(root) {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let mut bytes = 0_u64;
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        bytes = bytes
            .checked_add(measure_tree_owned(root, &entry.path())?)
            .ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
    }
    Ok(bytes)
}

fn remove_tree_owned(root: &Path, path: &Path) -> Result<(), StorageError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() || metadata.file_type().is_file() {
        return fs::remove_file(path).map_err(io_error);
    }
    if !metadata.file_type().is_dir() {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let canonical = fs::canonicalize(path).map_err(io_error)?;
    if !canonical.starts_with(root) || canonical == root {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        remove_tree_owned(root, &entry.path())?;
    }
    fs::remove_dir(path).map_err(io_error)
}

fn random_hex(bytes: usize) -> Result<String, StorageError> {
    let mut value = vec![0_u8; bytes];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut value))
        .map_err(io_error)?;
    let mut result = String::with_capacity(bytes * 2);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut result, "{:02x}", byte)
            .map_err(|_| StorageError::Protocol("token encoding failed"))?;
    }
    Ok(result)
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest_hex(&digest)
}

fn digest_encoded<T: Encode>(value: &T) -> Result<String, StorageError> {
    let mut writer = Writer::new();
    value.encode(&mut writer)?;
    Ok(digest_hex(&Sha256::digest(writer.finish())))
}

fn digest_hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{:02x}", byte);
    }
    value
}

fn valid_slot_id(id: &str) -> bool {
    slot_ordinal(id).is_some()
}

fn slot_generation(slot_id: &str) -> Result<String, StorageError> {
    let generation =
        slot_ordinal(slot_id).ok_or(StorageError::Refused(RefusalReason::UnknownAccounting))?;
    Ok(generation.to_string())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn io_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}

fn load_ledger(
    path: &Path,
    budget: Budget,
    backend: &LedgerBackend,
) -> Result<Ledger, StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ledger::empty(budget, backend.clone());
        }
        Err(error) => return Err(io_error(error)),
    };
    if !metadata.file_type().is_file()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > MAX_LEDGER_BYTES
    {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let file = File::open(path).map_err(io_error)?;
    let mut bytes = Vec::new();
    file.take(MAX_LEDGER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let mut reader = Reader::new(&bytes);
    if reader.bytes()?.as_slice() != &LEDGER_MAGIC[..] || reader.u32()? != LEDGER_VERSION {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let ledger = Ledger::decode(&mut reader)?;
    if !reader.done() || ledger.budget != budget || ledger.backend != *backend {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    Ok(ledger)
}

fn persist_ledger(root: &Path, path: &Path, ledger: &Ledger) -> Result<(), StorageError> {
    let mut writer = Writer::new();
    writer.bytes(LEDGER_MAGIC)?;
    writer.u32(LEDGER_VERSION);
    ledger.encode(&mut writer)?;
    let bytes = writer.finish();
    if bytes.len() as u64 > MAX_LEDGER_BYTES {
        return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
    }
    let temp_path = root.join(format!("ledger.tmp-{}", random_hex(8)?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(&temp_path).map_err(io_error)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(io_error(error));
    }
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(io_error)
}

impl Encode for Budget {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u64(self.cap_bytes);
        writer.u64(self.free_reserve_bytes);
        Ok(())
    }
}

impl Decode for Budget {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            cap_bytes: reader.u64()?,
            free_reserve_bytes: reader.u64()?,
        })
    }
}

impl Encode for InternalSlotState {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u8(match self {
            Self::Preparing => 0,
            Self::Idle => 1,
            Self::Leased => 2,
            Self::Uncertain => 3,
            Self::Evicting => 4,
        });
        Ok(())
    }
}

impl Decode for InternalSlotState {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            0 => Ok(Self::Preparing),
            1 => Ok(Self::Idle),
            2 => Ok(Self::Leased),
            3 => Ok(Self::Uncertain),
            4 => Ok(Self::Evicting),
            _ => Err(StorageError::Protocol("unknown slot state")),
        }
    }
}

impl Encode for OrdinaryCargoGeneration {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.root.encode(writer)?;
        self.partition.encode(writer)?;
        writer.u32(self.policy_version);
        self.installation_epoch.encode(writer)?;
        self.acceptance_reference.encode(writer)?;
        self.input_boundary_digest.encode(writer)?;
        self.reuse_state.encode(writer)
    }
}

impl Decode for OrdinaryCargoGeneration {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            root: StrictPhysicalCargoRoot::decode(reader)?,
            partition: OrdinaryCargoPartition::decode(reader)?,
            policy_version: reader.u32()?,
            installation_epoch: String::decode(reader)?,
            acceptance_reference: String::decode(reader)?,
            input_boundary_digest: String::decode(reader)?,
            reuse_state: CargoReuseState::decode(reader)?,
        })
    }
}

impl Encode for OrdinaryCargoLeaseRecord {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.request.encode(writer)?;
        self.saved_reuse_state.encode(writer)?;
        match self.finalized_reuse_state {
            Some(state) => {
                writer.u8(1);
                state.encode(writer)?;
            }
            None => writer.u8(0),
        }
        Ok(())
    }
}

impl Decode for OrdinaryCargoLeaseRecord {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let request = OrdinaryCargoRequest::decode(reader)?;
        let saved_reuse_state = CargoReuseState::decode(reader)?;
        let finalized_reuse_state = match reader.u8()? {
            0 => None,
            1 => Some(CargoReuseState::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid ordinary reuse state field")),
        };
        Ok(Self {
            request,
            saved_reuse_state,
            finalized_reuse_state,
        })
    }
}

impl Encode for Slot {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.id.encode(writer)?;
        self.partition.encode(writer)?;
        self.state.encode(writer)?;
        match self.measured_bytes {
            Some(bytes) => {
                writer.u8(1);
                writer.u64(bytes);
            }
            None => writer.u8(0),
        }
        writer.u64(self.reserved_bytes);
        match self.lease_id.as_ref() {
            Some(id) => {
                writer.u8(1);
                id.encode(writer)?;
            }
            None => writer.u8(0),
        }
        writer.u64(self.last_used);
        match self.ordinary_cargo.as_ref() {
            Some(ordinary) => {
                writer.u8(1);
                ordinary.encode(writer)?;
            }
            None => writer.u8(0),
        }
        Ok(())
    }
}

impl Decode for Slot {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let id = String::decode(reader)?;
        let partition = PartitionKey::decode(reader)?;
        let state = InternalSlotState::decode(reader)?;
        let measured_bytes = match reader.u8()? {
            0 => None,
            1 => Some(reader.u64()?),
            _ => return Err(StorageError::Protocol("invalid measured byte field")),
        };
        let reserved_bytes = reader.u64()?;
        let lease_id = match reader.u8()? {
            0 => None,
            1 => Some(String::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid lease reference")),
        };
        let last_used = reader.u64()?;
        let ordinary_cargo = match reader.u8()? {
            0 => None,
            1 => Some(OrdinaryCargoGeneration::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid ordinary Cargo slot record")),
        };
        Ok(Self {
            id,
            partition,
            state,
            measured_bytes,
            reserved_bytes,
            lease_id,
            last_used,
            ordinary_cargo,
        })
    }
}

impl Encode for LeaseStatus {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u8(match self {
            Self::Leased => 1,
            Self::Uncertain => 2,
            Self::Finalized => 3,
        });
        Ok(())
    }
}

impl Decode for LeaseStatus {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Leased),
            2 => Ok(Self::Uncertain),
            3 => Ok(Self::Finalized),
            _ => Err(StorageError::Protocol("unknown lease state")),
        }
    }
}

impl Encode for Lease {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.id.encode(writer)?;
        self.token_hash.encode(writer)?;
        self.slot_id.encode(writer)?;
        self.request_digest.encode(writer)?;
        self.request.encode(writer)?;
        self.status.encode(writer)?;
        writer.u8(u8::from(self.reused));
        match self.ordinary_cargo.as_ref() {
            Some(ordinary) => {
                writer.u8(1);
                ordinary.encode(writer)?;
            }
            None => writer.u8(0),
        }
        Ok(())
    }
}

impl Decode for Lease {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let id = String::decode(reader)?;
        let token_hash = String::decode(reader)?;
        let slot_id = String::decode(reader)?;
        let request_digest = String::decode(reader)?;
        let request = AcquireRequest::decode(reader)?;
        let status = LeaseStatus::decode(reader)?;
        let reused = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StorageError::Protocol("invalid reused flag")),
        };
        let ordinary_cargo = match reader.u8()? {
            0 => None,
            1 => Some(OrdinaryCargoLeaseRecord::decode(reader)?),
            _ => {
                return Err(StorageError::Protocol(
                    "invalid ordinary Cargo lease record",
                ))
            }
        };
        Ok(Self {
            id,
            token_hash,
            slot_id,
            request_digest,
            request,
            status,
            reused,
            ordinary_cargo,
        })
    }
}

impl Encode for SettlementKind {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u8(match self {
            Self::Finalize => 1,
            Self::Reconcile => 2,
        });
        Ok(())
    }
}

impl Decode for SettlementKind {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Finalize),
            2 => Ok(Self::Reconcile),
            _ => Err(StorageError::Protocol("unknown settlement operation")),
        }
    }
}

impl Encode for SlotState {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u8(match self {
            Self::Idle => 1,
            Self::Leased => 2,
            Self::Uncertain => 3,
        });
        Ok(())
    }
}

impl Decode for SlotState {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Idle),
            2 => Ok(Self::Leased),
            3 => Ok(Self::Uncertain),
            _ => Err(StorageError::Protocol("unknown public slot state")),
        }
    }
}

impl Encode for SettlementOperation {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.lease_id.encode(writer)?;
        self.operation_id.encode(writer)?;
        self.kind.encode(writer)?;
        self.evidence.encode(writer)?;
        self.request_digest.encode(writer)?;
        self.state.encode(writer)?;
        match self.capture_digest.as_ref() {
            Some(digest) => {
                writer.u8(1);
                digest.encode(writer)?;
            }
            None => writer.u8(0),
        }
        match self.cargo_reuse_state {
            Some(state) => {
                writer.u8(1);
                state.encode(writer)?;
            }
            None => writer.u8(0),
        }
        Ok(())
    }
}

impl Decode for SettlementOperation {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let lease_id = String::decode(reader)?;
        let operation_id = String::decode(reader)?;
        let kind = SettlementKind::decode(reader)?;
        let evidence = SettlementEvidence::decode(reader)?;
        let request_digest = String::decode(reader)?;
        let state = SlotState::decode(reader)?;
        let capture_digest = match reader.u8()? {
            0 => None,
            1 => Some(String::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid capture digest")),
        };
        let cargo_reuse_state = match reader.u8()? {
            0 => None,
            1 => Some(CargoReuseState::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid Cargo reuse state field")),
        };
        Ok(Self {
            lease_id,
            operation_id,
            kind,
            evidence,
            request_digest,
            state,
            capture_digest,
            cargo_reuse_state,
        })
    }
}

impl Encode for LedgerBackend {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        match self {
            Self::Directory => writer.u8(0),
            Self::Bounded(descriptor) => {
                writer.u8(1);
                descriptor.encode(writer)?;
            }
        }
        Ok(())
    }
}

impl Decode for LedgerBackend {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            0 => Ok(Self::Directory),
            1 => Ok(Self::Bounded(BoundedBackendDescriptor::decode(reader)?)),
            _ => Err(StorageError::Protocol("unknown durable backend")),
        }
    }
}

impl Encode for Ledger {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.budget.encode(writer)?;
        self.backend.encode(writer)?;
        self.storage_instance_id.encode(writer)?;
        writer.u64(self.next_slot);
        writer.u64(self.next_sequence);
        encode_vec(writer, &self.slots)?;
        encode_vec(writer, &self.leases)?;
        encode_vec(writer, &self.operations)
    }
}

impl Decode for Ledger {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            budget: Budget::decode(reader)?,
            backend: LedgerBackend::decode(reader)?,
            storage_instance_id: String::decode(reader)?,
            next_slot: reader.u64()?,
            next_sequence: reader.u64()?,
            slots: decode_vec(reader)?,
            leases: decode_vec(reader)?,
            operations: decode_vec(reader)?,
        })
    }
}
