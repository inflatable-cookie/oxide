use super::codec::{Decode, Encode, Reader, Writer, decode_vec, encode_vec};
use super::transport;
use super::types::*;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

const LEDGER_MAGIC: &[u8; 8] = b"OXIDEST1";
const LEDGER_VERSION: u32 = 2;
const MAX_LEDGER_BYTES: u64 = 64 * 1024 * 1024;
const SOCKET_NAME: &str = "daemon.sock";
const SLOTS_NAME: &str = "slots";

/// Verifies the consumer's admission envelope and settlement evidence.
///
/// The prototype ships no Queue or Nucleus authenticator. A production adapter
/// must supply one; a rejecting implementation is the safe default.
pub trait EvidenceAuthority: Send + Sync + 'static {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool;
    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool;
}

/// Supplies a host free-space observation. `None` means capacity is unknown.
pub trait FreeSpaceProbe: Send + Sync + 'static {
    fn available_bytes(&self, root: &Path) -> Result<Option<u64>, StorageError>;
}

/// Filesystem and budget configuration for the local prototype daemon.
#[derive(Clone, Debug)]
pub struct DaemonConfig {
    /// Existing, canonical, user-private directory reserved for this prototype.
    pub root: PathBuf,
    pub budget: Budget,
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

        let store = match Store::open(root, config.budget, authority, free_space) {
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
            .spawn(move || {
                loop {
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Lease {
    id: String,
    token_hash: String,
    slot_id: String,
    request: AcquireRequest,
    status: LeaseStatus,
    reused: bool,
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
    state: SlotState,
    capture_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Ledger {
    budget: Budget,
    next_slot: u64,
    next_sequence: u64,
    slots: Vec<Slot>,
    leases: Vec<Lease>,
    operations: Vec<SettlementOperation>,
}

impl Ledger {
    fn empty(budget: Budget) -> Self {
        Self {
            budget,
            next_slot: 1,
            next_sequence: 1,
            slots: Vec::new(),
            leases: Vec::new(),
            operations: Vec::new(),
        }
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
        authority: Arc<dyn EvidenceAuthority>,
        free_space: Arc<dyn FreeSpaceProbe>,
    ) -> Result<Self, StorageError> {
        if budget.cap_bytes == 0 {
            return Err(StorageError::InvalidInput("budget cap must be positive"));
        }
        validate_root_entries(&root)?;
        remove_stale_ledger_temps(&root)?;
        validate_root_entries(&root)?;
        let slots_root = root.join(SLOTS_NAME);
        ensure_private_directory(&slots_root)?;
        let ledger_path = root.join("ledger.bin");
        let ledger = load_ledger(&ledger_path, budget)?;
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
        let mut known = std::collections::BTreeSet::new();
        for slot in &self.ledger.slots {
            if !valid_slot_id(&slot.id) || !known.insert(slot.id.as_str()) {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
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
        }
        let mut lease_ids = std::collections::BTreeSet::new();
        for lease in &self.ledger.leases {
            if lease.id.len() != 32
                || !lease.id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || lease.token_hash.len() != 64
                || !lease
                    .token_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || !lease_ids.insert(lease.id.as_str())
            {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
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
        }
        let mut operation_ids = std::collections::BTreeSet::new();
        for operation in &self.ledger.operations {
            if !lease_ids.contains(operation.lease_id.as_str())
                || !operation_ids.insert((
                    operation.lease_id.as_str(),
                    operation.kind,
                    operation.operation_id.as_str(),
                ))
            {
                return Err(StorageError::Refused(RefusalReason::UnknownAccounting));
            }
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
        validate_root_entries(&self.root)
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
            });
        }
        slots.sort_by(|left, right| left.slot_id.cmp(&right.slot_id));
        let free_bytes = self.free_space.available_bytes(&self.root)?;
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
        request.envelope.partition.features.sort();
        request.envelope.partition.features.dedup();

        if !self.authority.verify_admission(&request.envelope) {
            return Err(StorageError::Unauthorized);
        }
        if let Some(existing) = self
            .ledger
            .leases
            .iter()
            .find(|lease| same_attempt(&lease.request, &request))
        {
            if existing.request != persisted_request(&request) {
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
        self.make_room(growth, matching.as_deref())?;
        self.require_free_space(growth, matching.as_deref())?;

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
                };
                self.ledger.slots.push(slot);
                self.persist()?;
                let path = self.slot_path(&id)?;
                fs::create_dir(&path).map_err(|error| {
                    self.poisoned = true;
                    io_error(error)
                })?;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|error| {
                    self.poisoned = true;
                    io_error(error)
                })?;
                File::open(&self.slots_root)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|error| {
                        self.poisoned = true;
                        io_error(error)
                    })?;
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
            request: persisted_request,
            status: LeaseStatus::Leased,
            reused,
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
        )?;
        Ok(FinalizationReceipt {
            lease_id: operation.lease_id,
            finalize_id: operation.operation_id,
            state: operation.state,
            capture_digest: operation.capture_digest,
        })
    }

    pub(super) fn reconcile(
        &mut self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        reconcile_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<ReconcileReceipt, StorageError> {
        let operation = self.settle(
            lease_id,
            token,
            reconcile_id,
            evidence,
            SettlementKind::Reconcile,
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
        let persisted_evidence = persisted_evidence(&evidence);
        if let Some(previous) = previous {
            if previous.evidence != persisted_evidence {
                return Err(StorageError::Conflict);
            }
            return Ok(previous.clone());
        }
        if lease.status == LeaseStatus::Finalized {
            return Err(StorageError::Refused(RefusalReason::LeaseUnavailable));
        }
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
            state: new_state,
            capture_digest,
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
        if slot.state != InternalSlotState::Idle || slot.lease_id.is_some() {
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
        while accounted.saturating_add(growth) > self.ledger.budget.cap_bytes {
            let candidate = self
                .ledger
                .slots
                .iter()
                .filter(|slot| {
                    slot.state == InternalSlotState::Idle
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

    fn ensure_healthy(&self) -> Result<(), StorageError> {
        if self.poisoned {
            Err(StorageError::Refused(RefusalReason::UnknownAccounting))
        } else {
            Ok(())
        }
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
                && valid_digest(&completion.repository_head)
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

fn validate_root_entries(root: &Path) -> Result<(), StorageError> {
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == SLOTS_NAME || name == "ledger.bin" || name == SOCKET_NAME {
            continue;
        }
        if is_ledger_temp_name(&name) {
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
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(&mut value, "{:02x}", byte);
    }
    value
}

fn valid_slot_id(id: &str) -> bool {
    id.len() == 21
        && id.starts_with("slot-")
        && id[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn io_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}

fn load_ledger(path: &Path, budget: Budget) -> Result<Ledger, StorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Ledger::empty(budget));
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
    if !reader.done() || ledger.budget != budget {
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
        Ok(Self {
            id,
            partition,
            state,
            measured_bytes,
            reserved_bytes,
            lease_id,
            last_used: reader.u64()?,
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
        self.request.encode(writer)?;
        self.status.encode(writer)?;
        writer.u8(u8::from(self.reused));
        Ok(())
    }
}

impl Decode for Lease {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let id = String::decode(reader)?;
        let token_hash = String::decode(reader)?;
        let slot_id = String::decode(reader)?;
        let request = AcquireRequest::decode(reader)?;
        let status = LeaseStatus::decode(reader)?;
        let reused = match reader.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StorageError::Protocol("invalid reused flag")),
        };
        Ok(Self {
            id,
            token_hash,
            slot_id,
            request,
            status,
            reused,
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
        self.state.encode(writer)?;
        match self.capture_digest.as_ref() {
            Some(digest) => {
                writer.u8(1);
                digest.encode(writer)?;
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
        let state = SlotState::decode(reader)?;
        let capture_digest = match reader.u8()? {
            0 => None,
            1 => Some(String::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid capture digest")),
        };
        Ok(Self {
            lease_id,
            operation_id,
            kind,
            evidence,
            state,
            capture_digest,
        })
    }
}

impl Encode for Ledger {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.budget.encode(writer)?;
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
            next_slot: reader.u64()?,
            next_sequence: reader.u64()?,
            slots: decode_vec(reader)?,
            leases: decode_vec(reader)?,
            operations: decode_vec(reader)?,
        })
    }
}
