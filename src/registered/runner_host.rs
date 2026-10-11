//! Inactive newline-JSON bridge around the registered runner.
//!
//! The parent supplies task data and owner callbacks. It never supplies storage
//! tokens or verifier material. A trusted embedder constructs RunnerHost with
//! the storage endpoint and its verifier, then explicitly serves one private
//! Unix-stream session per attempt.

use super::{
    Attempt, CaptureInputs, ExecutionOwner, ExecutionReceipt, ImmutableManifest,
    InputEvidenceOwner, Launch, Outcome, Refusal, Registration, RegistrationOwner, Runner,
    SourceEvidence, UnitEvidence,
};
use crate::storage::{
    AcquireRequest, AcquireResult, CaptureReceipt, Client, ContainmentEvidence,
    CooperativeCompletionEvidence, Credential, FinalizationReceipt, LeaseIdentity, LeaseToken,
    LookupResult, NoSpawnEvidence, RunBinding, SettlementEvidence, SlotState, StorageError,
    TerminalOutcome, TerminalRecord,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_AGGREGATE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CALLBACKS: usize = 256;
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_ID_BYTES: usize = 128;
const MAX_RECEIPT_BYTES: usize = 4096;
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;
const IMPLEMENTATION_PIN: &str = "d9ff17988c7eaa5cf57747bc55bfe4f138692f15";

/// Verifies parent receipts using material installed by the trusted embedder.
/// The local boolean answer is not serialized or accepted as a credential.
pub trait OwnerReceiptVerifier: Send + Sync {
    fn verify(
        &self,
        owner: &str,
        callback_id: &str,
        subject_digest: &str,
        policy_digest: &str,
        result_digest: &str,
        receipt: &str,
    ) -> bool;
}

/// Parent-supplied run identity. Lease-specific fields are filled only from the
/// storage client's authenticated acquire result.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptInput {
    pub binding: RunBinding,
    pub root_id: String,
    pub allocation_epoch: String,
    pub branch: String,
    pub source_root: PathBuf,
    pub partition: String,
    pub target: PathBuf,
    pub temp: PathBuf,
    pub capture: PathBuf,
    pub authorization: String,
}

/// JSON u64: safe numbers and canonical decimal strings are accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WireU64(pub u64);

impl Serialize for WireU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.0 <= MAX_SAFE_JSON_INTEGER {
            serializer.serialize_u64(self.0)
        } else {
            serializer.serialize_str(&self.0.to_string())
        }
    }
}

impl<'de> Deserialize<'de> for WireU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = WireU64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a safe unsigned integer or canonical u64 decimal string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value > MAX_SAFE_JSON_INTEGER {
                    return Err(E::custom("unsafe JSON integer must be decimal text"));
                }
                Ok(WireU64(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let value = u64::try_from(value).map_err(|_| E::custom("negative JSON integer"))?;
                if value > MAX_SAFE_JSON_INTEGER {
                    return Err(E::custom("unsafe JSON integer must be decimal text"));
                }
                Ok(WireU64(value))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if value.is_empty()
                    || (value.len() > 1 && value.starts_with('0'))
                    || !value.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(E::custom("non-canonical u64 decimal string"));
                }
                value
                    .parse::<u64>()
                    .map(WireU64)
                    .map_err(|_| E::custom("u64 decimal string overflow"))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartFrame {
    version: u32,
    operation: String,
    id: String,
    payload: StartPayload,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StartPayload {
    registration: Registration,
    attempt: AttemptInput,
    acquire: AcquireRequestWire,
    finalize_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AcquireRequestWire {
    envelope: crate::storage::RunEnvelope,
    acquire_id: String,
    required_bytes: WireU64,
}

impl From<AcquireRequestWire> for AcquireRequest {
    fn from(value: AcquireRequestWire) -> Self {
        Self {
            envelope: value.envelope,
            acquire_id: value.acquire_id,
            required_bytes: value.required_bytes.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CallbackRequest {
    version: u32,
    operation: String,
    id: String,
    owner: String,
    subject_digest: String,
    policy_digest: String,
    storage: StorageIdentity,
    partition: crate::storage::PartitionKey,
    payload: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallbackResponse {
    version: u32,
    operation: String,
    id: String,
    owner: String,
    subject_digest: String,
    policy_digest: String,
    result_digest: String,
    receipt: String,
    result: Value,
}

#[derive(Clone, Debug, Serialize)]
struct ResultFrame {
    version: u32,
    operation: &'static str,
    id: String,
    result: HostResult,
}

#[derive(Clone, Debug, Serialize)]
struct ErrorFrame {
    version: u32,
    operation: &'static str,
    id: String,
    code: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostStatus {
    StoragePending,
    Held,
    Finished,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StorageIdentity {
    pub slot_id: String,
    pub storage_instance_id: String,
    pub generation: String,
    pub reused: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostResult {
    pub status: HostStatus,
    pub storage: Option<StorageIdentity>,
    /// SHA-256 of the lease token. Token material never crosses a frame.
    pub token_digest: Option<String>,
    pub outcome: Option<OutcomeName>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeName {
    Published,
    NotSpawned,
}

/// Bounded bridge errors contain no task, credential, or lease-token contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostError {
    Io,
    Protocol,
    Timeout,
    Storage,
    OwnerRejected,
    RunnerRefused,
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Io => "private runner host I/O failed",
            Self::Protocol => "private runner host protocol refused the frame",
            Self::Timeout => "private runner host exceeded its time bound",
            Self::Storage => "private runner host storage operation failed",
            Self::OwnerRejected => "installed owner verifier rejected a callback receipt",
            Self::RunnerRefused => "registered runner refused the attempt",
        })
    }
}

impl std::error::Error for HostError {}

/// Inactive host API. Construction does not spawn a process or register a
/// workflow; the embedder must explicitly serve a private stream.
pub struct RunnerHost {
    client: Client,
    verifier: Arc<dyn OwnerReceiptVerifier>,
    #[cfg(feature = "private-qualification")]
    qualification: Option<super::qualification::AttemptGrant>,
}

impl fmt::Debug for RunnerHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerHost")
            .field("client", &"installed")
            .field("verifier", &"installed")
            .finish()
    }
}

impl RunnerHost {
    /// Installs the host-owned daemon client and trusted verifier.
    pub fn new(
        storage_socket: impl Into<PathBuf>,
        verifier: Arc<dyn OwnerReceiptVerifier>,
    ) -> Self {
        Self {
            client: Client::new(storage_socket),
            verifier,
            #[cfg(feature = "private-qualification")]
            qualification: None,
        }
    }

    #[cfg(feature = "private-qualification")]
    pub(super) fn qualified(mut self, grant: super::qualification::AttemptGrant) -> Self {
        self.qualification = Some(grant);
        self
    }

    /// Serves one bounded request over an embedder-provided Unix stream.
    pub fn serve(&self, stream: UnixStream) -> Result<HostResult, HostError> {
        let channel = Arc::new(Mutex::new(Channel::new(stream)?));
        let frame: StartFrame = channel
            .lock()
            .map_err(|_| HostError::Protocol)?
            .read_frame()?;
        if frame.version != PROTOCOL_VERSION || frame.operation != "run" || !valid_id(&frame.id) {
            return Err(HostError::Protocol);
        }
        #[cfg(feature = "private-qualification")]
        let preflight = match &self.qualification {
            Some(grant) => {
                grant.check_start(&serde_json::to_value(&frame).map_err(|_| HostError::Protocol)?)
            }
            None => Ok(()),
        };
        #[cfg(not(feature = "private-qualification"))]
        let preflight = Ok(());
        let result = preflight.and_then(|_| self.run_request(frame.payload, channel.clone()));
        match result {
            Ok(result) => {
                channel
                    .lock()
                    .map_err(|_| HostError::Protocol)?
                    .write_frame(&ResultFrame {
                        version: PROTOCOL_VERSION,
                        operation: "result",
                        id: frame.id,
                        result: result.clone(),
                    })?;
                Ok(result)
            }
            Err(error) => {
                let code = match error {
                    HostError::OwnerRejected => "owner_rejected",
                    HostError::RunnerRefused => "runner_refused",
                    HostError::Storage => "storage_failure",
                    HostError::Timeout => "timeout",
                    HostError::Io => "io_failure",
                    HostError::Protocol => "protocol_refused",
                };
                if let Ok(mut channel) = channel.lock() {
                    let _ = channel.write_frame(&ErrorFrame {
                        version: PROTOCOL_VERSION,
                        operation: "error",
                        id: frame.id,
                        code,
                    });
                }
                Err(error)
            }
        }
    }

    fn run_request(
        &self,
        payload: StartPayload,
        channel: Arc<Mutex<Channel>>,
    ) -> Result<HostResult, HostError> {
        if !valid_id(&payload.finalize_id)
            || payload.acquire.acquire_id.is_empty()
            || payload.acquire.acquire_id.len() > MAX_ID_BYTES
            || payload.acquire.required_bytes.0 == 0
            || payload.attempt.binding != payload.acquire.envelope.binding
            || !canonical_positive_u64(&payload.attempt.allocation_epoch)
        {
            return Err(HostError::Protocol);
        }
        let request: AcquireRequest = payload.acquire.into();
        let acquired = match self.acquire(&request)? {
            AcquireDisposition::Pending => {
                return Ok(HostResult {
                    status: HostStatus::StoragePending,
                    storage: None,
                    token_digest: None,
                    outcome: None,
                });
            }
            AcquireDisposition::Held(identity) => {
                return Ok(HostResult {
                    status: HostStatus::Held,
                    storage: identity.map(StorageIdentity::from),
                    token_digest: None,
                    outcome: None,
                });
            }
            AcquireDisposition::Acquired(acquired) => acquired,
        };
        if !valid_acquired(&acquired) {
            return Err(HostError::Storage);
        }
        #[cfg(feature = "private-qualification")]
        if let Some(grant) = &self.qualification {
            grant.check_acquired(&acquired)?;
        }
        let token = acquired.token.as_ref().ok_or(HostError::Storage)?;
        let token_digest = digest(token.value().as_bytes());
        let slot = acquired.slot_path.as_ref().ok_or(HostError::Storage)?;
        let attempt = Attempt {
            binding: payload.attempt.binding,
            root_id: payload.attempt.root_id,
            allocation_epoch: payload.attempt.allocation_epoch,
            branch: payload.attempt.branch,
            source_root: payload.attempt.source_root,
            slot: slot.clone(),
            generation: acquired.generation.clone(),
            partition: payload.attempt.partition,
            lease_id: acquired.lease_id.clone(),
            target: payload.attempt.target,
            temp: payload.attempt.temp,
            capture: payload.attempt.capture,
            authorization: payload.attempt.authorization,
            reused: acquired.reused,
        };
        let parent = Arc::new(ParentCallbacks {
            channel,
            client: self.client.clone(),
            verifier: self.verifier.clone(),
            registration: payload.registration.clone(),
            partition: request.envelope.partition.clone(),
            acquired,
            finalize_id: payload.finalize_id,
            settlement: Mutex::new(None),
            owner_rejected: Mutex::new(false),
            callback_failure: Mutex::new(None),
        });
        let runner = Runner {
            registration_owner: parent.as_ref(),
            input_owner: parent.as_ref(),
            execution_owner: parent.as_ref(),
        };
        let outcome = match runner.run(&payload.registration, &attempt) {
            Ok(outcome) => outcome,
            Err(_) => {
                if parent
                    .owner_rejected
                    .lock()
                    .map_err(|_| HostError::Protocol)?
                    .to_owned()
                {
                    return Err(HostError::OwnerRejected);
                }
                if let Some(error) = parent
                    .callback_failure
                    .lock()
                    .map_err(|_| HostError::Protocol)?
                    .take()
                {
                    return Err(error);
                }
                return Err(HostError::RunnerRefused);
            }
        };
        let outcome = match outcome {
            Outcome::Published => OutcomeName::Published,
            Outcome::NotSpawned => OutcomeName::NotSpawned,
        };
        Ok(HostResult {
            status: HostStatus::Finished,
            storage: Some(storage_identity(&parent.acquired)),
            token_digest: Some(token_digest),
            outcome: Some(outcome),
        })
    }

    fn acquire(&self, request: &AcquireRequest) -> Result<AcquireDisposition, HostError> {
        acquire_with(
            request,
            |request| self.client.acquire(request),
            |request| self.client.lookup_acquire(request),
        )
    }
}

/// Keep the original credential-bearing request in attempt memory. In
/// particular, lookup is read-only and an Absent result permits only replay of
/// this exact request; this function has no authority to mint credentials.
fn acquire_with<A, L>(
    request: &AcquireRequest,
    mut acquire: A,
    mut lookup: L,
) -> Result<AcquireDisposition, HostError>
where
    A: FnMut(AcquireRequest) -> Result<AcquireResult, StorageError>,
    L: FnMut(AcquireRequest) -> Result<LookupResult<LeaseIdentity>, StorageError>,
{
    match acquire(request.clone()) {
        Ok(result) => return acquired_or_hold(result),
        Err(StorageError::Refused(
            crate::storage::RefusalReason::LeaseUnavailable
            | crate::storage::RefusalReason::BudgetExceeded
            | crate::storage::RefusalReason::FreeSpaceReserve
            | crate::storage::RefusalReason::FreeSpaceUnavailable,
        )) => return Ok(AcquireDisposition::Pending),
        Err(StorageError::Refused(crate::storage::RefusalReason::UnknownAccounting)) => {
            return Ok(AcquireDisposition::Held(None));
        }
        Err(StorageError::Unauthorized | StorageError::Conflict) => {
            return Err(HostError::Storage);
        }
        Err(_) => {}
    }

    // A lost reply is resolved by the read-only API before any exact replay.
    match lookup(request.clone()) {
        Ok(LookupResult::Present(identity)) | Ok(LookupResult::Uncertain(Some(identity))) => {
            return Ok(AcquireDisposition::Held(Some(identity)));
        }
        Ok(LookupResult::Uncertain(None)) => return Ok(AcquireDisposition::Held(None)),
        Ok(LookupResult::Absent) => {}
        Err(_) => return Ok(AcquireDisposition::Held(None)),
    }
    match acquire(request.clone()) {
        Ok(result) => acquired_or_hold(result),
        Err(StorageError::Refused(
            crate::storage::RefusalReason::LeaseUnavailable
            | crate::storage::RefusalReason::BudgetExceeded
            | crate::storage::RefusalReason::FreeSpaceReserve
            | crate::storage::RefusalReason::FreeSpaceUnavailable,
        )) => Ok(AcquireDisposition::Pending),
        Err(_) => Ok(AcquireDisposition::Held(None)),
    }
}

#[derive(Debug)]
enum AcquireDisposition {
    Pending,
    Held(Option<LeaseIdentity>),
    Acquired(AcquireResult),
}

fn acquired_or_hold(result: AcquireResult) -> Result<AcquireDisposition, HostError> {
    if result.state == SlotState::Leased {
        if valid_acquired(&result) {
            Ok(AcquireDisposition::Acquired(result))
        } else {
            Err(HostError::Storage)
        }
    } else {
        Ok(AcquireDisposition::Held(Some(LeaseIdentity {
            lease_id: result.lease_id,
            slot_id: result.slot_id,
            storage_instance_id: result.storage_instance_id,
            generation: result.generation,
            state: result.state,
        })))
    }
}

fn valid_acquired(result: &AcquireResult) -> bool {
    let Some(path) = result.slot_path.as_ref() else {
        return false;
    };
    let Some(token) = result.token.as_ref() else {
        return false;
    };
    let generation = result.generation.parse::<u64>();
    result.state == SlotState::Leased
        && !result.lease_id.is_empty()
        && !result.slot_id.is_empty()
        && result.storage_instance_id.len() == 64
        && result
            .storage_instance_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && generation.is_ok_and(|value| value > 0 && value.to_string() == result.generation)
        && !token.value().is_empty()
        && path.is_absolute()
        && path.to_str().is_some()
        && std::fs::canonicalize(path).is_ok_and(|canonical| canonical == *path)
        && std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

fn storage_identity(result: &AcquireResult) -> StorageIdentity {
    StorageIdentity {
        slot_id: result.slot_id.clone(),
        storage_instance_id: result.storage_instance_id.clone(),
        generation: result.generation.clone(),
        reused: Some(result.reused),
    }
}

impl From<LeaseIdentity> for StorageIdentity {
    fn from(identity: LeaseIdentity) -> Self {
        Self {
            slot_id: identity.slot_id,
            storage_instance_id: identity.storage_instance_id,
            generation: identity.generation,
            reused: None,
        }
    }
}

struct Channel {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
    started: Instant,
    callbacks: usize,
    aggregate_bytes: usize,
}

impl Channel {
    fn new(stream: UnixStream) -> Result<Self, HostError> {
        stream
            .set_read_timeout(Some(CALLBACK_TIMEOUT))
            .map_err(|_| HostError::Io)?;
        stream
            .set_write_timeout(Some(CALLBACK_TIMEOUT))
            .map_err(|_| HostError::Io)?;
        let writer = stream.try_clone().map_err(|_| HostError::Io)?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer: BufWriter::new(writer),
            started: Instant::now(),
            callbacks: 0,
            aggregate_bytes: 0,
        })
    }

    fn read_frame<T: DeserializeOwned>(&mut self) -> Result<T, HostError> {
        self.check_time()?;
        let mut bytes = Vec::new();
        loop {
            let available = self.reader.fill_buf().map_err(|error| {
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) {
                    HostError::Timeout
                } else {
                    HostError::Io
                }
            })?;
            if available.is_empty() {
                return Err(HostError::Protocol);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if bytes.len().saturating_add(count) > MAX_FRAME_BYTES {
                return Err(HostError::Protocol);
            }
            bytes.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                break;
            }
        }
        if bytes.len() < 2 || bytes.last() != Some(&b'\n') {
            return Err(HostError::Protocol);
        }
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            return Err(HostError::Protocol);
        }
        self.charge(bytes.len() + 1)?;
        serde_json::from_slice(&bytes).map_err(|_| HostError::Protocol)
    }

    fn write_frame<T: Serialize>(&mut self, value: &T) -> Result<(), HostError> {
        self.check_time()?;
        let mut bytes = serde_json::to_vec(value).map_err(|_| HostError::Protocol)?;
        if bytes.len().saturating_add(1) > MAX_FRAME_BYTES {
            return Err(HostError::Protocol);
        }
        self.charge(bytes.len() + 1)?;
        bytes.push(b'\n');
        self.writer.write_all(&bytes).map_err(|_| HostError::Io)?;
        self.writer.flush().map_err(|_| HostError::Io)
    }

    fn exchange<T: Serialize, O: DeserializeOwned>(
        &mut self,
        owner: &str,
        payload: &T,
        registration: &Registration,
        storage: &StorageIdentity,
        partition: &crate::storage::PartitionKey,
        verifier: &dyn OwnerReceiptVerifier,
    ) -> Result<O, CallbackError> {
        if self.callbacks >= MAX_CALLBACKS {
            return Err(CallbackError::Protocol);
        }
        self.callbacks += 1;
        let callback_id = self.callbacks.to_string();
        let payload = serde_json::to_value(payload).map_err(|_| CallbackError::Protocol)?;
        let subject = serde_json::to_value(CallbackSubject {
            id: &callback_id,
            owner,
            storage,
            partition,
            payload: &payload,
        })
        .map_err(|_| CallbackError::Protocol)?;
        let subject_digest =
            digest(&canonical_json_bytes(&subject).map_err(|_| CallbackError::Protocol)?);
        let policy_digest = callback_policy_digest(owner, registration)?;
        self.write_frame(&CallbackRequest {
            version: PROTOCOL_VERSION,
            operation: "callback".to_owned(),
            id: callback_id.clone(),
            owner: owner.to_owned(),
            subject_digest: subject_digest.clone(),
            policy_digest: policy_digest.clone(),
            storage: storage.clone(),
            partition: partition.clone(),
            payload,
        })
        .map_err(CallbackError::from)?;
        let response: CallbackResponse = self.read_frame().map_err(CallbackError::from)?;
        if response.version != PROTOCOL_VERSION
            || response.operation != "callback_result"
            || response.id != callback_id
            || response.owner != owner
            || response.subject_digest != subject_digest
            || response.policy_digest != policy_digest
            || response.receipt.is_empty()
            || response.receipt.len() > MAX_RECEIPT_BYTES
        {
            return Err(CallbackError::Protocol);
        }
        let result_digest =
            digest(&canonical_json_bytes(&response.result).map_err(|_| CallbackError::Protocol)?);
        if result_digest != response.result_digest
            || !verifier.verify(
                owner,
                &response.id,
                &subject_digest,
                &policy_digest,
                &result_digest,
                &response.receipt,
            )
        {
            return Err(CallbackError::Rejected);
        }
        serde_json::from_value(response.result).map_err(|_| CallbackError::Protocol)
    }

    fn charge(&mut self, bytes: usize) -> Result<(), HostError> {
        self.aggregate_bytes = self
            .aggregate_bytes
            .checked_add(bytes)
            .ok_or(HostError::Protocol)?;
        if self.aggregate_bytes > MAX_AGGREGATE_BYTES {
            return Err(HostError::Protocol);
        }
        self.check_time()
    }

    fn check_time(&self) -> Result<(), HostError> {
        if self.started.elapsed() > SESSION_TIMEOUT {
            Err(HostError::Timeout)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CallbackError {
    Protocol,
    Rejected,
    Host(HostError),
}

impl From<HostError> for CallbackError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

#[derive(Serialize)]
struct CallbackSubject<'a> {
    id: &'a str,
    owner: &'a str,
    storage: &'a StorageIdentity,
    partition: &'a crate::storage::PartitionKey,
    payload: &'a Value,
}

#[derive(Serialize)]
struct CallbackPolicy<'a> {
    version: &'static str,
    implementation_pin: &'static str,
    owner: &'a str,
    registration_digest: String,
    registration_version: u32,
    source_version: u32,
    dependency_version: u32,
    configuration_version: u32,
    graph_digest: &'a str,
    configuration_digest: &'a str,
    toolchain_digest: &'a str,
    debug_policy_digest: &'a str,
    limits: super::Limits,
}

fn callback_policy_digest(
    owner: &str,
    registration: &Registration,
) -> Result<String, CallbackError> {
    let registration_value =
        serde_json::to_value(registration).map_err(|_| CallbackError::Protocol)?;
    let registration_digest =
        digest(&canonical_json_bytes(&registration_value).map_err(|_| CallbackError::Protocol)?);
    let policy = CallbackPolicy {
        version: "oxide-runner-callback-policy-v1",
        implementation_pin: IMPLEMENTATION_PIN,
        owner,
        registration_digest,
        registration_version: registration.version,
        source_version: registration.source_version,
        dependency_version: registration.dependency_version,
        configuration_version: registration.configuration_version,
        graph_digest: &registration.graph_digest,
        configuration_digest: &registration.configuration_digest,
        toolchain_digest: &registration.toolchain_digest,
        debug_policy_digest: &registration.debug_policy_digest,
        limits: registration.limits,
    };
    serde_json::to_value(policy)
        .and_then(|value| canonical_json_bytes(&value))
        .map(|bytes| digest(&bytes))
        .map_err(|_| CallbackError::Protocol)
}

pub(super) fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    fn sort_objects(value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let mut keys: Vec<_> = fields.keys().collect();
                keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
                let mut sorted = serde_json::Map::new();
                for key in keys {
                    sorted.insert(key.clone(), sort_objects(&fields[key]));
                }
                Value::Object(sorted)
            }
            Value::Array(values) => Value::Array(values.iter().map(sort_objects).collect()),
            value => value.clone(),
        }
    }

    serde_json::to_vec(&sort_objects(value))
}

#[derive(Serialize)]
struct RegistrationAuthPayload<'a> {
    registration: &'a Registration,
    attempt: &'a Attempt,
}

#[derive(Serialize)]
struct AuditPayload<'a> {
    registration: &'a Registration,
    source: &'a SourceEvidence,
}

#[derive(Serialize)]
struct ImmutablePayload<'a> {
    receipt: &'a str,
    attempt: &'a Attempt,
}

#[derive(Serialize)]
struct UnitsPayload<'a> {
    registration: &'a Registration,
    attempt: &'a Attempt,
}

#[derive(Serialize)]
struct SealPayload<'a> {
    attempt: &'a Attempt,
    map_digest: &'a str,
}

#[derive(Serialize)]
struct VerifyMapPayload<'a> {
    attempt: &'a Attempt,
    map_digest: &'a str,
    seal: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SealResult {
    seal: String,
}

struct ParentCallbacks {
    channel: Arc<Mutex<Channel>>,
    client: Client,
    verifier: Arc<dyn OwnerReceiptVerifier>,
    registration: Registration,
    partition: crate::storage::PartitionKey,
    acquired: AcquireResult,
    finalize_id: String,
    settlement: Mutex<Option<SettlementClaims>>,
    owner_rejected: Mutex<bool>,
    callback_failure: Mutex<Option<HostError>>,
}

impl ParentCallbacks {
    fn callback<T: Serialize, O: DeserializeOwned>(
        &self,
        owner: &str,
        payload: &T,
    ) -> Result<O, Refusal> {
        let result = self
            .channel
            .lock()
            .map_err(|_| Refusal::Authentication)?
            .exchange(
                owner,
                payload,
                &self.registration,
                &storage_identity(&self.acquired),
                &self.partition,
                self.verifier.as_ref(),
            );
        match result {
            Ok(result) => Ok(result),
            Err(CallbackError::Rejected) => {
                if let Ok(mut rejected) = self.owner_rejected.lock() {
                    *rejected = true;
                }
                Err(Refusal::Authentication)
            }
            Err(CallbackError::Protocol) => Err(Refusal::Authentication),
            Err(CallbackError::Host(error)) => {
                if let Ok(mut failure) = self.callback_failure.lock() {
                    *failure = Some(error);
                }
                Err(Refusal::Authentication)
            }
        }
    }

    fn settlement_evidence(&self, launch: &Launch) -> Result<SettlementEvidence, Refusal> {
        let claims = self
            .settlement
            .lock()
            .map_err(|_| Refusal::Completion)?
            .clone()
            .ok_or(Refusal::Completion)?;
        if claims.terminal.run_id != launch.binding.run_id
            || claims.terminal.outcome != "passed"
            || claims.capture.run_id != launch.binding.run_id
            || claims.capture.attempt_id != launch.binding.attempt_id
            || claims.completion.repository_head != launch.binding.commit
            || claims.completion.host_run_id != claims.capture.host_run_id
            || claims.completion.capture_reference != claims.capture.reference
            || claims.completion.capture_digest != claims.capture.digest
            || claims.completion.exit_code != 0
            || !valid_digest(&claims.completion.workflow_definition_digest)
            || !valid_digest(&claims.completion.fixture_source_digest)
            || !valid_digest(&claims.completion.stdout_digest)
            || !valid_digest(&claims.completion.stderr_digest)
        {
            return Err(Refusal::Completion);
        }
        let token = self.acquired.token.as_ref().ok_or(Refusal::Completion)?;
        let mut completion = CooperativeCompletionEvidence {
            lease_id: launch.lease_id.clone(),
            slot_id: self.acquired.slot_id.clone(),
            slot_path: launch.slot.to_str().ok_or(Refusal::Identity)?.to_owned(),
            temp_dir: launch.temp.to_str().ok_or(Refusal::Identity)?.to_owned(),
            capture_dir: launch.capture.to_str().ok_or(Refusal::Identity)?.to_owned(),
            workflow: launch.binding.workflow.clone(),
            workflow_definition_digest: claims.completion.workflow_definition_digest,
            repository_head: claims.completion.repository_head,
            fixture_source_digest: claims.completion.fixture_source_digest,
            host_run_id: claims.completion.host_run_id,
            run_id: launch.binding.run_id.clone(),
            attempt_id: launch.binding.attempt_id.clone(),
            processes: claims.completion.processes,
            free_space_samples: claims
                .completion
                .free_space_samples
                .into_iter()
                .map(|sample| sample.0)
                .collect(),
            exit_code: claims.completion.exit_code,
            stdout_digest: claims.completion.stdout_digest,
            stderr_digest: claims.completion.stderr_digest,
            capture_reference: claims.completion.capture_reference,
            capture_digest: claims.completion.capture_digest,
            signature: String::new(),
        };
        completion.sign(token);
        Ok(SettlementEvidence {
            binding: launch.binding.clone(),
            consumer_credential: Credential::new(claims.consumer_credential),
            terminal: Some(TerminalRecord {
                run_id: claims.terminal.run_id,
                outcome: TerminalOutcome::Passed,
                credential: Credential::new(claims.terminal.credential),
            }),
            capture: Some(CaptureReceipt {
                reference: claims.capture.reference,
                digest: claims.capture.digest,
                host_run_id: claims.capture.host_run_id,
                run_id: claims.capture.run_id,
                attempt_id: claims.capture.attempt_id,
                credential: Credential::new(claims.capture.credential),
            }),
            containment: ContainmentEvidence::CooperativeWorkflow(completion),
        })
    }

    fn finalize(&self, evidence: SettlementEvidence) -> Result<FinalizationReceipt, Refusal> {
        let token = self.acquired.token.as_ref().ok_or(Refusal::Completion)?;
        finalize_with(
            evidence,
            |evidence| self.request_finalize(token, evidence),
            |evidence| {
                self.client
                    .lookup_finalize(&self.acquired.lease_id, &self.finalize_id, evidence)
            },
        )
    }

    fn request_finalize(
        &self,
        token: &LeaseToken,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, ()> {
        self.client
            .finalize(
                &self.acquired.lease_id,
                Some(token),
                &self.finalize_id,
                evidence,
            )
            .map_err(|_| ())
    }
}

/// Resolve ambiguous settlement only through the authenticated lookup API.
/// An Absent observation permits an exact evidence replay; it never settles by
/// itself and there is deliberately no reconcile branch here.
fn finalize_with<F, L>(
    evidence: SettlementEvidence,
    mut finalize: F,
    mut lookup: L,
) -> Result<FinalizationReceipt, Refusal>
where
    F: FnMut(SettlementEvidence) -> Result<FinalizationReceipt, ()>,
    L: FnMut(SettlementEvidence) -> Result<LookupResult<FinalizationReceipt>, StorageError>,
{
    match finalize(evidence.clone()) {
        Ok(receipt) if receipt.state == SlotState::Idle => Ok(receipt),
        Ok(_) => Err(Refusal::Completion),
        Err(()) => match lookup(evidence.clone()) {
            Ok(LookupResult::Present(receipt)) if receipt.state == SlotState::Idle => Ok(receipt),
            Ok(LookupResult::Absent) => match finalize(evidence.clone()) {
                Ok(receipt) if receipt.state == SlotState::Idle => Ok(receipt),
                Err(()) => match lookup(evidence) {
                    Ok(LookupResult::Present(receipt)) if receipt.state == SlotState::Idle => {
                        Ok(receipt)
                    }
                    _ => Err(Refusal::Completion),
                },
                _ => Err(Refusal::Completion),
            },
            Ok(LookupResult::Uncertain(_)) | Ok(LookupResult::Present(_)) | Err(_) => {
                Err(Refusal::Completion)
            }
        },
    }
}

impl RegistrationOwner for ParentCallbacks {
    fn authenticate(
        &self,
        registration: &Registration,
        attempt: &Attempt,
    ) -> Result<SourceEvidence, Refusal> {
        self.callback(
            "registration.authenticate",
            &RegistrationAuthPayload {
                registration,
                attempt,
            },
        )
    }

    fn audit(&self, registration: &Registration, source: &SourceEvidence) -> Result<(), Refusal> {
        self.callback(
            "registration.audit",
            &AuditPayload {
                registration,
                source,
            },
        )
    }
}

impl InputEvidenceOwner for ParentCallbacks {
    fn immutable(&self, receipt: &str, attempt: &Attempt) -> Result<ImmutableManifest, Refusal> {
        self.callback("input.immutable", &ImmutablePayload { receipt, attempt })
    }

    fn units(
        &self,
        registration: &Registration,
        attempt: &Attempt,
    ) -> Result<Vec<UnitEvidence>, Refusal> {
        self.callback(
            "input.units",
            &UnitsPayload {
                registration,
                attempt,
            },
        )
    }

    fn seal_map(&self, attempt: &Attempt, map_digest: &str) -> Result<String, Refusal> {
        let result: SealResult = self.callback(
            "input.seal_map",
            &SealPayload {
                attempt,
                map_digest,
            },
        )?;
        if result.seal.is_empty() || result.seal.len() > self.registration.limits.path_bytes {
            return Err(Refusal::Authentication);
        }
        Ok(result.seal)
    }

    fn verify_map(&self, attempt: &Attempt, map_digest: &str, seal: &str) -> Result<(), Refusal> {
        self.callback(
            "input.verify_map",
            &VerifyMapPayload {
                attempt,
                map_digest,
                seal,
            },
        )
    }
}

#[derive(Serialize)]
struct ExecutePayload<'a> {
    launch: &'a Launch,
}

#[derive(Serialize)]
struct VerifySuccessPayload<'a> {
    registration: &'a Registration,
    launch: &'a Launch,
    receipt: &'a str,
    inputs: &'a CaptureInputs,
}

#[derive(Serialize)]
struct FinalizePayload<'a> {
    launch: &'a Launch,
    receipt: &'a str,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TerminalClaims {
    run_id: String,
    outcome: String,
    credential: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureClaims {
    reference: String,
    digest: String,
    host_run_id: String,
    run_id: String,
    attempt_id: String,
    credential: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompletionClaims {
    workflow_definition_digest: String,
    repository_head: String,
    fixture_source_digest: String,
    host_run_id: String,
    processes: Vec<crate::storage::ProcessIdentityEvidence>,
    free_space_samples: Vec<WireU64>,
    exit_code: i32,
    stdout_digest: String,
    stderr_digest: String,
    capture_reference: String,
    capture_digest: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SettlementClaims {
    consumer_credential: String,
    terminal: TerminalClaims,
    capture: CaptureClaims,
    completion: CompletionClaims,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoSpawnClaims {
    consumer_credential: String,
}

impl ExecutionOwner for ParentCallbacks {
    fn execute(&self, launch: &Launch) -> Result<ExecutionReceipt, Refusal> {
        self.callback("execution.execute", &ExecutePayload { launch })
    }

    fn verify_success(
        &self,
        registration: &Registration,
        launch: &Launch,
        receipt: &str,
        inputs: &CaptureInputs,
    ) -> Result<(), Refusal> {
        let claims: SettlementClaims = self.callback(
            "execution.verify_success",
            &VerifySuccessPayload {
                registration,
                launch,
                receipt,
                inputs,
            },
        )?;
        *self.settlement.lock().map_err(|_| Refusal::Completion)? = Some(claims);
        Ok(())
    }

    fn finalize_idle(&self, launch: &Launch, receipt: &str) -> Result<(), Refusal> {
        self.callback::<_, ()>(
            "execution.finalize_idle",
            &FinalizePayload { launch, receipt },
        )?;
        self.finalize(self.settlement_evidence(launch)?)?;
        Ok(())
    }

    fn finalize_not_spawned(&self, launch: &Launch, receipt: &str) -> Result<(), Refusal> {
        let claims: NoSpawnClaims = self.callback(
            "execution.finalize_not_spawned",
            &FinalizePayload { launch, receipt },
        )?;
        let token = self.acquired.token.as_ref().ok_or(Refusal::Completion)?;
        let proof = NoSpawnEvidence::new(&self.acquired, &launch.binding, token)
            .ok_or(Refusal::Completion)?;
        self.finalize(SettlementEvidence {
            binding: launch.binding.clone(),
            consumer_credential: Credential::new(claims.consumer_credential),
            terminal: None,
            capture: None,
            containment: ContainmentEvidence::NotSpawned(proof),
        })?;
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_BYTES && !value.as_bytes().contains(&0)
}

fn canonical_positive_u64(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|parsed| parsed > 0 && parsed.to_string() == value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream;
    use std::thread;

    struct AcceptReceipts;

    impl OwnerReceiptVerifier for AcceptReceipts {
        fn verify(&self, _: &str, _: &str, _: &str, _: &str, _: &str, _: &str) -> bool {
            true
        }
    }

    struct RejectReceipts;

    impl OwnerReceiptVerifier for RejectReceipts {
        fn verify(&self, _: &str, _: &str, _: &str, _: &str, _: &str, _: &str) -> bool {
            false
        }
    }

    fn registration() -> Registration {
        Registration {
            version: 1,
            source_version: 1,
            dependency_version: 1,
            configuration_version: 1,
            id: "fixture".into(),
            graph_digest: digest(b"graph"),
            configuration_digest: digest(b"configuration"),
            toolchain_digest: digest(b"toolchain"),
            argv: super::super::Argv {
                executable: PathBuf::from("/private/tmp/fixture"),
                arguments: Vec::new(),
                cwd: PathBuf::from("/private/tmp"),
                environment: BTreeMap::new(),
            },
            source_files: BTreeSet::new(),
            immutable_receipts: BTreeSet::new(),
            units: BTreeSet::new(),
            unit_environment: BTreeMap::new(),
            tests: BTreeSet::new(),
            debug_policy_digest: digest(b"debug"),
            limits: super::super::Limits::NUCLEUS,
        }
    }

    fn acquire_request() -> AcquireRequest {
        start_frame(PROTOCOL_VERSION, "run").payload.acquire.into()
    }

    fn test_lease_identity() -> LeaseIdentity {
        LeaseIdentity {
            lease_id: "lease-fixture".into(),
            slot_id: "slot-0000000000000001".into(),
            storage_instance_id: "a".repeat(64),
            generation: "1".into(),
            state: SlotState::Leased,
        }
    }

    fn test_acquire_result(path: PathBuf) -> AcquireResult {
        AcquireResult {
            lease_id: "lease-fixture".into(),
            slot_id: "slot-0000000000000001".into(),
            storage_instance_id: "a".repeat(64),
            generation: "1".into(),
            state: SlotState::Leased,
            slot_path: Some(path),
            token: Some(LeaseToken::new("fixture-only-token".into())),
            reused: false,
        }
    }

    fn test_settlement_evidence() -> SettlementEvidence {
        SettlementEvidence {
            binding: acquire_request().envelope.binding,
            consumer_credential: Credential::new("fixture-original-consumer-secret"),
            terminal: None,
            capture: None,
            containment: ContainmentEvidence::Unknown,
        }
    }

    fn start_frame(version: u32, operation: &str) -> StartFrame {
        let binding = RunBinding {
            consumer: "consumer".into(),
            project: "project".into(),
            component: "component".into(),
            workflow: "workflow".into(),
            task_id: "task".into(),
            workspace_id: "workspace".into(),
            run_id: "run".into(),
            attempt_id: "attempt".into(),
            commit: "commit".into(),
            source_tree_digest: digest(b"source"),
        };
        StartFrame {
            version,
            operation: operation.into(),
            id: "request-1".into(),
            payload: StartPayload {
                registration: registration(),
                attempt: AttemptInput {
                    binding: binding.clone(),
                    root_id: "root".into(),
                    allocation_epoch: "1".into(),
                    branch: "branch".into(),
                    source_root: PathBuf::from("/private/tmp/source"),
                    partition: digest(b"partition"),
                    target: PathBuf::from("/private/tmp/target"),
                    temp: PathBuf::from("/private/tmp/temp"),
                    capture: PathBuf::from("/private/tmp/capture"),
                    authorization: "authorization-reference".into(),
                },
                acquire: AcquireRequestWire {
                    envelope: crate::storage::RunEnvelope {
                        binding,
                        partition: crate::storage::PartitionKey {
                            project_component: "project:component".into(),
                            workflow: "workflow".into(),
                            cargo_provenance: "cargo".into(),
                            rustc_provenance: "rustc".into(),
                            host: "host".into(),
                            target: "target".into(),
                            profile: "profile".into(),
                            features: Vec::new(),
                            config_digest: digest(b"config"),
                            environment_digest: digest(b"env"),
                            path_identity: "path".into(),
                        },
                        admission_reference: "admission".into(),
                        admission_credential: Credential::new("credential"),
                    },
                    acquire_id: "acquire-1".into(),
                    required_bytes: WireU64(1),
                },
                finalize_id: "finalize-1".into(),
            },
        }
    }

    #[test]
    fn wire_u64_requires_safe_numbers_or_canonical_decimal_text() {
        for accepted in [
            "0",
            "1",
            "9007199254740991",
            "\"9007199254740992\"",
            "\"18446744073709551615\"",
        ] {
            assert!(
                serde_json::from_str::<WireU64>(accepted).is_ok(),
                "{accepted}"
            );
        }
        for rejected in [
            "-1",
            "9007199254740992",
            "1.0",
            "\"\"",
            "\"01\"",
            "\"+1\"",
            "\"18446744073709551616\"",
        ] {
            assert!(
                serde_json::from_str::<WireU64>(rejected).is_err(),
                "{rejected}"
            );
        }
    }

    #[test]
    fn canonical_json_digests_sort_keys_by_utf8_bytes() {
        let value = serde_json::json!({"ä": 2, "z": 0, "a": 1});
        assert_eq!(
            canonical_json_bytes(&value).unwrap(),
            r#"{"a":1,"z":0,"ä":2}"#.as_bytes()
        );
        assert_eq!(
            digest(&canonical_json_bytes(&value).unwrap()),
            digest(r#"{"a":1,"z":0,"ä":2}"#.as_bytes())
        );
        assert_ne!(
            digest(&canonical_json_bytes(&value).unwrap()),
            digest(b"file-content-is-a-different-identity")
        );
    }

    #[test]
    fn acquired_result_requires_generation_canonical_slot_path_and_token() {
        let scratch = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .expect("Effigy supplies the canonical isolated scratch root");
        let slot =
            PathBuf::from(scratch).join(format!("runner-host-valid-slot-{}", std::process::id()));
        std::fs::create_dir(&slot).unwrap();
        let result = AcquireResult {
            lease_id: "lease-fixture".into(),
            slot_id: "slot-fixture".into(),
            storage_instance_id: "a".repeat(64),
            generation: "1".into(),
            state: SlotState::Leased,
            slot_path: Some(slot.clone()),
            token: Some(LeaseToken::new("fixture-only-token".into())),
            reused: false,
        };
        assert!(valid_acquired(&result));

        let mut missing_generation = result.clone();
        missing_generation.generation.clear();
        assert!(!valid_acquired(&missing_generation));
        let mut missing_path = result.clone();
        missing_path.slot_path = None;
        assert!(!valid_acquired(&missing_path));
        let mut missing_token = result.clone();
        missing_token.token = None;
        assert!(!valid_acquired(&missing_token));
        let mut noncanonical_generation = result;
        noncanonical_generation.generation = "01".into();
        assert!(!valid_acquired(&noncanonical_generation));
        std::fs::remove_dir(slot).unwrap();
    }

    #[test]
    fn lost_acquire_uses_lookup_then_replays_the_exact_original_credential() {
        let request = acquire_request();
        let scratch = PathBuf::from(
            std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
                .expect("Effigy supplies the canonical isolated scratch root"),
        );
        let slot = scratch.join(format!("runner-host-replay-{}", std::process::id()));
        std::fs::create_dir(&slot).unwrap();
        let mut acquire_calls = 0;
        let observed = std::cell::RefCell::new(Vec::new());

        let result = acquire_with(
            &request,
            |seen| {
                observed.borrow_mut().push(("acquire", seen.clone()));
                acquire_calls += 1;
                if acquire_calls == 1 {
                    Err(StorageError::Io("synthetic lost reply".into()))
                } else {
                    Ok(test_acquire_result(slot.clone()))
                }
            },
            |seen| {
                observed.borrow_mut().push(("lookup", seen.clone()));
                Ok(LookupResult::Absent)
            },
        )
        .unwrap();

        assert!(matches!(result, AcquireDisposition::Acquired(_)));
        assert_eq!(
            observed
                .borrow()
                .iter()
                .map(|(operation, _)| *operation)
                .collect::<Vec<_>>(),
            ["acquire", "lookup", "acquire"]
        );
        assert!(observed.borrow().iter().all(|(_, seen)| seen == &request));
        assert_eq!(request.envelope.admission_credential.expose(), "credential");
        std::fs::remove_dir(slot).unwrap();
    }

    #[test]
    fn lookup_present_leased_is_observation_only_and_unknown_accounting_holds() {
        let request = acquire_request();
        let operations = std::cell::RefCell::new(Vec::new());
        let result = acquire_with(
            &request,
            |seen| {
                assert_eq!(seen, request);
                operations.borrow_mut().push("acquire");
                Err(StorageError::Io("synthetic lost reply".into()))
            },
            |seen| {
                assert_eq!(seen, request);
                operations.borrow_mut().push("lookup");
                Ok(LookupResult::Present(test_lease_identity()))
            },
        )
        .unwrap();
        let AcquireDisposition::Held(Some(identity)) = result else {
            panic!("a lookup identity must remain held");
        };
        assert_eq!(identity, test_lease_identity());
        assert_eq!(*operations.borrow(), ["acquire", "lookup"]);

        operations.borrow_mut().clear();
        let result = acquire_with(
            &request,
            |_| {
                operations.borrow_mut().push("acquire");
                Err(StorageError::Refused(
                    crate::storage::RefusalReason::UnknownAccounting,
                ))
            },
            |_| {
                operations.borrow_mut().push("lookup");
                Ok(LookupResult::Absent)
            },
        )
        .unwrap();
        assert!(matches!(result, AcquireDisposition::Held(None)));
        assert_eq!(*operations.borrow(), ["acquire"]);
    }

    #[test]
    fn missing_or_reminted_admission_secret_refuses_without_retry() {
        let operations = std::cell::RefCell::new(Vec::new());
        let mut missing = acquire_request();
        missing.envelope.admission_credential = Credential::new("");
        assert_eq!(
            acquire_with(
                &missing,
                |_| {
                    operations.borrow_mut().push("acquire");
                    Err(StorageError::Unauthorized)
                },
                |_| {
                    operations.borrow_mut().push("lookup");
                    Ok(LookupResult::Absent)
                },
            )
            .unwrap_err(),
            HostError::Storage
        );
        assert_eq!(*operations.borrow(), ["acquire"]);

        operations.borrow_mut().clear();
        let original = acquire_request();
        let mut reminted = original.clone();
        reminted.envelope.admission_credential =
            Credential::new("fixture-reminted-admission-secret");
        assert_eq!(
            acquire_with(
                &reminted,
                |seen| {
                    operations.borrow_mut().push("acquire");
                    assert_eq!(seen, reminted);
                    assert_ne!(seen, original);
                    Err(StorageError::Conflict)
                },
                |_| {
                    operations.borrow_mut().push("lookup");
                    Ok(LookupResult::Absent)
                },
            )
            .unwrap_err(),
            HostError::Storage
        );
        assert_eq!(*operations.borrow(), ["acquire"]);
    }

    #[test]
    fn lost_finalize_uses_lookup_before_exact_evidence_replay() {
        let evidence = test_settlement_evidence();
        let expected = evidence.clone();
        let receipt = FinalizationReceipt {
            lease_id: "lease-fixture".into(),
            finalize_id: "finalize-fixture".into(),
            state: SlotState::Idle,
            capture_digest: None,
        };
        let mut finalizes = 0;
        let observed = std::cell::RefCell::new(Vec::new());
        let result = finalize_with(
            evidence,
            |seen| {
                observed.borrow_mut().push(("finalize", seen.clone()));
                finalizes += 1;
                if finalizes == 1 {
                    Err(())
                } else {
                    Ok(receipt.clone())
                }
            },
            |seen| {
                observed.borrow_mut().push(("lookup", seen.clone()));
                Ok(LookupResult::Absent)
            },
        )
        .unwrap();
        assert_eq!(result, receipt);
        assert_eq!(
            observed
                .borrow()
                .iter()
                .map(|(operation, _)| *operation)
                .collect::<Vec<_>>(),
            ["finalize", "lookup", "finalize"]
        );
        assert!(observed.borrow().iter().all(|(_, seen)| seen == &expected));
    }

    #[test]
    fn unsupported_top_level_operation_and_version_refuse_before_acquire() {
        for frame in [
            start_frame(PROTOCOL_VERSION + 1, "run"),
            start_frame(1, "spawn"),
        ] {
            let host = RunnerHost::new("/private/tmp/no-daemon", Arc::new(AcceptReceipts));
            let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
            let worker = thread::spawn(move || host.serve(host_stream));
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            parent_stream.write_all(&bytes).unwrap();
            assert_eq!(worker.join().unwrap().unwrap_err(), HostError::Protocol);
        }
    }

    #[test]
    fn truncated_frame_refuses_without_a_partial_parse() {
        let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            Channel::new(host_stream)
                .unwrap()
                .read_frame::<StartFrame>()
        });
        parent_stream
            .write_all(br#"{"version":1,"operation":"run""#)
            .unwrap();
        parent_stream.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err(), HostError::Protocol);
    }

    #[test]
    fn oversized_frame_and_unknown_fields_refuse() {
        #[derive(Serialize)]
        struct OversizedFrame {
            payload: String,
        }

        let (host_stream, _peer_stream) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(host_stream).unwrap();
        assert_eq!(
            channel.write_frame(&OversizedFrame {
                payload: "x".repeat(MAX_FRAME_BYTES),
            }),
            Err(HostError::Protocol)
        );

        let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            Channel::new(host_stream)
                .unwrap()
                .read_frame::<StartFrame>()
        });
        let mut frame = serde_json::to_value(start_frame(PROTOCOL_VERSION, "run")).unwrap();
        frame["unrecognized"] = serde_json::Value::Bool(true);
        serde_json::to_writer(&mut parent_stream, &frame).unwrap();
        parent_stream.write_all(b"\n").unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err(), HostError::Protocol);
    }

    #[test]
    fn callback_reply_must_match_the_exact_next_operation() {
        let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
        let parent = thread::spawn(move || {
            let mut reader = BufReader::new(parent_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let response = serde_json::json!({
                "version": 1,
                "operation": "callback_result",
                "id": "2",
                "owner": request["owner"],
                "subject_digest": request["subject_digest"],
                "policy_digest": request["policy_digest"],
                "result_digest": digest(b"null"),
                "receipt": "fixture-only",
                "result": null
            });
            serde_json::to_writer(&mut parent_stream, &response).unwrap();
            parent_stream.write_all(b"\n").unwrap();
        });
        let mut channel = Channel::new(host_stream).unwrap();
        let error = channel.exchange::<_, ()>(
            "test.owner",
            &serde_json::json!({"identity":"attempt"}),
            &registration(),
            &StorageIdentity {
                slot_id: "slot-0000000000000001".into(),
                storage_instance_id: "a".repeat(64),
                generation: "1".into(),
                reused: Some(false),
            },
            &crate::storage::PartitionKey {
                project_component: "project:component".into(),
                workflow: "workflow".into(),
                cargo_provenance: "cargo".into(),
                rustc_provenance: "rustc".into(),
                host: "host".into(),
                target: "target".into(),
                profile: "profile".into(),
                features: Vec::new(),
                config_digest: digest(b"config"),
                environment_digest: digest(b"env"),
                path_identity: "path".into(),
            },
            &AcceptReceipts,
        );
        parent.join().unwrap();
        assert!(matches!(error, Err(CallbackError::Protocol)));
    }

    #[test]
    fn callback_binding_refuses_old_lease_shape_and_mismatched_tuple() {
        let storage = StorageIdentity {
            slot_id: "slot-0000000000000001".into(),
            storage_instance_id: "a".repeat(64),
            generation: "1".into(),
            reused: Some(false),
        };
        let partition = crate::storage::PartitionKey {
            project_component: "project:component".into(),
            workflow: "workflow".into(),
            cargo_provenance: "cargo".into(),
            rustc_provenance: "rustc".into(),
            host: "host".into(),
            target: "target".into(),
            profile: "profile".into(),
            features: Vec::new(),
            config_digest: digest(b"config"),
            environment_digest: digest(b"env"),
            path_identity: "path".into(),
        };

        for old_lease_shape in [false, true] {
            let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
            let parent_storage = serde_json::to_value(&storage).unwrap();
            let parent_partition = serde_json::to_value(&partition).unwrap();
            let parent = thread::spawn(move || {
                let mut reader = BufReader::new(parent_stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["storage"]["slot_id"], parent_storage["slot_id"]);
                assert_eq!(
                    request["storage"]["storage_instance_id"],
                    parent_storage["storage_instance_id"]
                );
                assert_eq!(
                    request["storage"]["generation"],
                    parent_storage["generation"]
                );
                assert_eq!(request["partition"], parent_partition);

                let mut changed_storage = request["storage"].clone();
                if old_lease_shape {
                    changed_storage = serde_json::json!({
                        "id": "legacy-lease-id",
                        "generation": request["storage"]["generation"]
                    });
                } else {
                    changed_storage["storage_instance_id"] = Value::String("b".repeat(64));
                }
                let changed_subject = serde_json::json!({
                    "id": request["id"],
                    "owner": request["owner"],
                    "storage": changed_storage,
                    "partition": request["partition"],
                    "payload": request["payload"]
                });
                let result = Value::Null;
                let response = serde_json::json!({
                    "version": 1,
                    "operation": "callback_result",
                    "id": request["id"],
                    "owner": request["owner"],
                    "subject_digest": digest(&canonical_json_bytes(&changed_subject).unwrap()),
                    "policy_digest": request["policy_digest"],
                    "result_digest": digest(&canonical_json_bytes(&result).unwrap()),
                    "receipt": "fixture-only-mismatched-binding",
                    "result": result
                });
                serde_json::to_writer(&mut parent_stream, &response).unwrap();
                parent_stream.write_all(b"\n").unwrap();
            });

            let mut channel = Channel::new(host_stream).unwrap();
            let error = channel.exchange::<_, ()>(
                "test.owner",
                &serde_json::json!({"identity":"attempt"}),
                &registration(),
                &storage,
                &partition,
                &AcceptReceipts,
            );
            parent.join().unwrap();
            assert!(matches!(error, Err(CallbackError::Protocol)));
        }
    }

    #[test]
    fn owner_receipts_from_a_policy_without_the_map_pin_refuse() {
        let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
        let parent = thread::spawn(move || {
            let mut reader = BufReader::new(parent_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = Value::Null;
            let response = serde_json::json!({
                "version": 1,
                "operation": "callback_result",
                "id": request["id"],
                "owner": request["owner"],
                "subject_digest": request["subject_digest"],
                "policy_digest": digest(b"older policy without the reviewed map pin"),
                "result_digest": digest(&canonical_json_bytes(&result).unwrap()),
                "receipt": "fixture-only-old-receipt",
                "result": result
            });
            serde_json::to_writer(&mut parent_stream, &response).unwrap();
            parent_stream.write_all(b"\n").unwrap();
        });
        let mut channel = Channel::new(host_stream).unwrap();
        let error = channel.exchange::<_, ()>(
            "test.owner",
            &serde_json::json!({"map_digest":digest(b"fixture map")}),
            &registration(),
            &StorageIdentity {
                slot_id: "slot-0000000000000001".into(),
                storage_instance_id: "a".repeat(64),
                generation: "1".into(),
                reused: Some(false),
            },
            &crate::storage::PartitionKey {
                project_component: "project:component".into(),
                workflow: "workflow".into(),
                cargo_provenance: "cargo".into(),
                rustc_provenance: "rustc".into(),
                host: "host".into(),
                target: "target".into(),
                profile: "profile".into(),
                features: Vec::new(),
                config_digest: digest(b"config"),
                environment_digest: digest(b"env"),
                path_identity: "path".into(),
            },
            &AcceptReceipts,
        );
        parent.join().unwrap();
        assert!(matches!(error, Err(CallbackError::Protocol)));
        assert_eq!(
            IMPLEMENTATION_PIN,
            "d9ff17988c7eaa5cf57747bc55bfe4f138692f15"
        );
    }

    #[test]
    fn installed_verifier_rejection_refuses_a_correctly_ordered_reply() {
        let (host_stream, mut parent_stream) = UnixStream::pair().unwrap();
        let parent = thread::spawn(move || {
            let mut reader = BufReader::new(parent_stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let response = serde_json::json!({
                "version": 1,
                "operation": "callback_result",
                "id": request["id"],
                "owner": request["owner"],
                "subject_digest": request["subject_digest"],
                "policy_digest": request["policy_digest"],
                "result_digest": digest(b"null"),
                "receipt": "forged-fixture-receipt",
                "result": null
            });
            serde_json::to_writer(&mut parent_stream, &response).unwrap();
            parent_stream.write_all(b"\n").unwrap();
        });
        let mut channel = Channel::new(host_stream).unwrap();
        let error = channel.exchange::<_, ()>(
            "test.owner",
            &serde_json::json!({"identity":"attempt"}),
            &registration(),
            &StorageIdentity {
                slot_id: "slot-0000000000000001".into(),
                storage_instance_id: "a".repeat(64),
                generation: "1".into(),
                reused: Some(false),
            },
            &crate::storage::PartitionKey {
                project_component: "project:component".into(),
                workflow: "workflow".into(),
                cargo_provenance: "cargo".into(),
                rustc_provenance: "rustc".into(),
                host: "host".into(),
                target: "target".into(),
                profile: "profile".into(),
                features: Vec::new(),
                config_digest: digest(b"config"),
                environment_digest: digest(b"env"),
                path_identity: "path".into(),
            },
            &RejectReceipts,
        );
        parent.join().unwrap();
        assert!(matches!(error, Err(CallbackError::Rejected)));
    }

    #[test]
    fn explicit_session_and_callback_bounds_refuse_over_budget_work() {
        let (stream, _peer_stream) = UnixStream::pair().unwrap();
        let mut channel = Channel::new(stream).unwrap();
        channel.aggregate_bytes = MAX_AGGREGATE_BYTES;
        assert_eq!(channel.charge(1), Err(HostError::Protocol));
        channel.started = Instant::now() - SESSION_TIMEOUT - Duration::from_millis(1);
        assert_eq!(channel.check_time(), Err(HostError::Timeout));
        channel.callbacks = MAX_CALLBACKS;
        assert!(matches!(
            channel.exchange::<_, ()>(
                "test.owner",
                &serde_json::json!(null),
                &registration(),
                &StorageIdentity {
                    slot_id: "slot-0000000000000001".into(),
                    storage_instance_id: "a".repeat(64),
                    generation: "1".into(),
                    reused: Some(false),
                },
                &crate::storage::PartitionKey {
                    project_component: "project:component".into(),
                    workflow: "workflow".into(),
                    cargo_provenance: "cargo".into(),
                    rustc_provenance: "rustc".into(),
                    host: "host".into(),
                    target: "target".into(),
                    profile: "profile".into(),
                    features: Vec::new(),
                    config_digest: digest(b"config"),
                    environment_digest: digest(b"env"),
                    path_identity: "path".into(),
                },
                &AcceptReceipts,
            ),
            Err(CallbackError::Protocol)
        ));
    }
}
