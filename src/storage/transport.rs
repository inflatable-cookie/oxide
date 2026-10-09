use super::codec::{Decode, Encode, Reader, Writer};
use super::store::Store;
use super::types::*;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[cfg(test)]
pub(super) fn send_acquire_and_forget(
    socket_path: &std::path::Path,
    request: AcquireRequest,
) -> Result<(), StorageError> {
    let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
    let mut writer = Writer::new();
    Request::Acquire(request).encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())?;
    stream.shutdown(std::net::Shutdown::Both).map_err(io_error)
}

/// Client for the private local daemon socket.
#[derive(Clone, Debug)]
pub struct Client {
    socket_path: PathBuf,
}

impl Client {
    /// Connects to an existing endpoint for each operation.
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Returns a read-only snapshot. Retries may observe newer state.
    pub fn inventory(&self, request_id: &str) -> Result<Inventory, StorageError> {
        match self.call(Request::Inventory(request_id.to_owned()))? {
            Response::Inventory(result) => result,
            _ => Err(StorageError::Protocol("unexpected inventory response")),
        }
    }

    /// Reads the current configured budget and accounting snapshot.
    pub fn budget(&self, request_id: &str) -> Result<Inventory, StorageError> {
        match self.call(Request::Budget(request_id.to_owned()))? {
            Response::Inventory(result) => result,
            _ => Err(StorageError::Protocol("unexpected budget response")),
        }
    }

    /// Acquires or replays the same durable run-attempt lease.
    pub fn acquire(&self, request: AcquireRequest) -> Result<AcquireResult, StorageError> {
        match self.call(Request::Acquire(request))? {
            Response::Acquire(result) => result,
            _ => Err(StorageError::Protocol("unexpected acquire response")),
        }
    }

    /// Finalizes a lease after authenticated terminal, capture, and containment evidence.
    pub fn finalize(
        &self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<FinalizationReceipt, StorageError> {
        match self.call(Request::Finalize {
            lease_id: lease_id.to_owned(),
            token: token.cloned(),
            operation_id: finalize_id.to_owned(),
            evidence,
        })? {
            Response::Finalize(result) => result,
            _ => Err(StorageError::Protocol("unexpected finalize response")),
        }
    }

    /// Reconciles an uncertain lease using authenticated evidence.
    pub fn reconcile(
        &self,
        lease_id: &str,
        token: Option<&LeaseToken>,
        reconcile_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<ReconcileReceipt, StorageError> {
        match self.call(Request::Reconcile {
            lease_id: lease_id.to_owned(),
            token: token.cloned(),
            operation_id: reconcile_id.to_owned(),
            evidence,
        })? {
            Response::Reconcile(result) => result,
            _ => Err(StorageError::Protocol("unexpected reconcile response")),
        }
    }

    /// Evicts up to the requested bytes from reconciled idle slots only.
    pub fn evict(&self, request_id: &str, target_bytes: u64) -> Result<u64, StorageError> {
        match self.call(Request::Evict {
            request_id: request_id.to_owned(),
            target_bytes,
        })? {
            Response::Evict(result) => result,
            _ => Err(StorageError::Protocol("unexpected eviction response")),
        }
    }

    fn call(&self, request: Request) -> Result<Response, StorageError> {
        let mut stream = UnixStream::connect(&self.socket_path).map_err(io_error)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(io_error)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .map_err(io_error)?;
        let mut writer = Writer::new();
        request.encode(&mut writer)?;
        write_frame(&mut stream, writer.finish())?;
        let bytes = read_frame(&mut stream)?;
        let mut reader = Reader::new(&bytes);
        let response = Response::decode(&mut reader)?;
        if !reader.done() {
            return Err(StorageError::Protocol("trailing response data"));
        }
        match response {
            Response::Error(error) => Err(error),
            response => Ok(response),
        }
    }
}

enum Request {
    Inventory(String),
    Budget(String),
    Acquire(AcquireRequest),
    Finalize {
        lease_id: String,
        token: Option<LeaseToken>,
        operation_id: String,
        evidence: SettlementEvidence,
    },
    Reconcile {
        lease_id: String,
        token: Option<LeaseToken>,
        operation_id: String,
        evidence: SettlementEvidence,
    },
    Evict {
        request_id: String,
        target_bytes: u64,
    },
}

impl Encode for Request {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        match self {
            Self::Inventory(request_id) => {
                writer.u8(1);
                request_id.encode(writer)?;
            }
            Self::Budget(request_id) => {
                writer.u8(2);
                request_id.encode(writer)?;
            }
            Self::Acquire(request) => {
                writer.u8(3);
                request.encode(writer)?;
            }
            Self::Finalize {
                lease_id,
                token,
                operation_id,
                evidence,
            } => {
                writer.u8(4);
                lease_id.encode(writer)?;
                encode_token(writer, token.as_ref())?;
                operation_id.encode(writer)?;
                evidence.encode(writer)?;
            }
            Self::Reconcile {
                lease_id,
                token,
                operation_id,
                evidence,
            } => {
                writer.u8(5);
                lease_id.encode(writer)?;
                encode_token(writer, token.as_ref())?;
                operation_id.encode(writer)?;
                evidence.encode(writer)?;
            }
            Self::Evict {
                request_id,
                target_bytes,
            } => {
                writer.u8(6);
                request_id.encode(writer)?;
                target_bytes.encode(writer)?;
            }
        }
        Ok(())
    }
}

impl Decode for Request {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Inventory(String::decode(reader)?)),
            2 => Ok(Self::Budget(String::decode(reader)?)),
            3 => Ok(Self::Acquire(AcquireRequest::decode(reader)?)),
            4 => Ok(Self::Finalize {
                lease_id: String::decode(reader)?,
                token: decode_token(reader)?,
                operation_id: String::decode(reader)?,
                evidence: SettlementEvidence::decode(reader)?,
            }),
            5 => Ok(Self::Reconcile {
                lease_id: String::decode(reader)?,
                token: decode_token(reader)?,
                operation_id: String::decode(reader)?,
                evidence: SettlementEvidence::decode(reader)?,
            }),
            6 => Ok(Self::Evict {
                request_id: String::decode(reader)?,
                target_bytes: u64::decode(reader)?,
            }),
            _ => Err(StorageError::Protocol("unknown operation")),
        }
    }
}

enum Response {
    Inventory(Result<Inventory, StorageError>),
    Acquire(Result<AcquireResult, StorageError>),
    Finalize(Result<FinalizationReceipt, StorageError>),
    Reconcile(Result<ReconcileReceipt, StorageError>),
    Evict(Result<u64, StorageError>),
    Error(StorageError),
}

impl Encode for Response {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        match self {
            Self::Inventory(result) => {
                writer.u8(1);
                encode_result(writer, result, encode_inventory)?;
            }
            Self::Acquire(result) => {
                writer.u8(2);
                encode_result(writer, result, encode_acquire_result)?;
            }
            Self::Finalize(result) => {
                writer.u8(3);
                encode_result(writer, result, encode_finalization)?;
            }
            Self::Reconcile(result) => {
                writer.u8(4);
                encode_result(writer, result, encode_reconcile)?;
            }
            Self::Evict(result) => {
                writer.u8(5);
                encode_result(writer, result, |writer, value| value.encode(writer))?;
            }
            Self::Error(error) => {
                writer.u8(6);
                encode_error(writer, error)?;
            }
        }
        Ok(())
    }
}

impl Decode for Response {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Inventory(decode_result(reader, decode_inventory))),
            2 => Ok(Self::Acquire(decode_result(reader, decode_acquire_result))),
            3 => Ok(Self::Finalize(decode_result(reader, decode_finalization))),
            4 => Ok(Self::Reconcile(decode_result(reader, decode_reconcile))),
            5 => Ok(Self::Evict(decode_result(reader, u64::decode))),
            6 => Ok(Self::Error(decode_error(reader)?)),
            _ => Err(StorageError::Protocol("unknown response")),
        }
    }
}

pub(crate) fn serve_one(mut stream: UnixStream, store: &mut Store) -> Result<(), StorageError> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(io_error)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(io_error)?;
    let request_bytes = match read_frame(&mut stream) {
        Ok(bytes) => bytes,
        Err(error) => {
            let mut writer = Writer::new();
            Response::Error(error).encode(&mut writer)?;
            return write_frame(&mut stream, writer.finish());
        }
    };
    let request = {
        let mut reader = Reader::new(&request_bytes);
        let request = Request::decode(&mut reader);
        match request {
            Ok(request) if reader.done() => request,
            Ok(_) => {
                let mut writer = Writer::new();
                Response::Error(StorageError::Protocol("trailing request data"))
                    .encode(&mut writer)?;
                return write_frame(&mut stream, writer.finish());
            }
            Err(error) => {
                let mut writer = Writer::new();
                Response::Error(error).encode(&mut writer)?;
                return write_frame(&mut stream, writer.finish());
            }
        }
    };
    let response = match request {
        Request::Inventory(request_id) => {
            if request_id.is_empty() {
                Response::Inventory(Err(StorageError::InvalidInput("request ID is required")))
            } else {
                Response::Inventory(store.inventory())
            }
        }
        Request::Budget(request_id) => {
            if request_id.is_empty() {
                Response::Inventory(Err(StorageError::InvalidInput("request ID is required")))
            } else {
                Response::Inventory(store.inventory())
            }
        }
        Request::Acquire(request) => Response::Acquire(store.acquire(request)),
        Request::Finalize {
            lease_id,
            token,
            operation_id,
            evidence,
        } => Response::Finalize(store.finalize(&lease_id, token.as_ref(), &operation_id, evidence)),
        Request::Reconcile {
            lease_id,
            token,
            operation_id,
            evidence,
        } => {
            Response::Reconcile(store.reconcile(&lease_id, token.as_ref(), &operation_id, evidence))
        }
        Request::Evict {
            request_id,
            target_bytes,
        } => {
            if request_id.is_empty() {
                Response::Evict(Err(StorageError::InvalidInput("request ID is required")))
            } else {
                Response::Evict(store.evict(target_bytes))
            }
        }
    };
    let mut writer = Writer::new();
    response.encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())
}

fn encode_result<T>(
    writer: &mut Writer,
    result: &Result<T, StorageError>,
    encode_value: impl FnOnce(&mut Writer, &T) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    match result {
        Ok(value) => {
            writer.u8(0);
            encode_value(writer, value)
        }
        Err(error) => {
            writer.u8(1);
            encode_error(writer, error)
        }
    }
}

fn decode_result<T>(
    reader: &mut Reader<'_>,
    decode_value: impl FnOnce(&mut Reader<'_>) -> Result<T, StorageError>,
) -> Result<T, StorageError> {
    match reader.u8()? {
        0 => decode_value(reader),
        1 => Err(decode_error(reader)?),
        _ => Err(StorageError::Protocol("invalid result tag")),
    }
}

fn encode_error(writer: &mut Writer, error: &StorageError) -> Result<(), StorageError> {
    match error {
        StorageError::Io(message) => {
            writer.u8(1);
            writer.string(message)?;
        }
        StorageError::InvalidInput(message) => {
            writer.u8(2);
            writer.string(message)?;
        }
        StorageError::Conflict => writer.u8(3),
        StorageError::Unauthorized => writer.u8(4),
        StorageError::Protocol(message) => {
            writer.u8(5);
            writer.string(message)?;
        }
        StorageError::Refused(reason) => {
            writer.u8(6);
            writer.u8(match reason {
                RefusalReason::AlreadyRunning => 1,
                RefusalReason::UnknownAccounting => 2,
                RefusalReason::FreeSpaceUnavailable => 3,
                RefusalReason::FreeSpaceReserve => 4,
                RefusalReason::BudgetExceeded => 5,
                RefusalReason::NoIdleStorage => 6,
                RefusalReason::LeaseUnavailable => 7,
            });
        }
    }
    Ok(())
}

fn decode_error(reader: &mut Reader<'_>) -> Result<StorageError, StorageError> {
    match reader.u8()? {
        1 => Ok(StorageError::Io(reader.string()?)),
        2 => Ok(StorageError::InvalidInput("remote request was invalid")),
        3 => Ok(StorageError::Conflict),
        4 => Ok(StorageError::Unauthorized),
        5 => Ok(StorageError::Protocol(
            "remote protocol rejected the request",
        )),
        6 => match reader.u8()? {
            1 => Ok(StorageError::Refused(RefusalReason::AlreadyRunning)),
            2 => Ok(StorageError::Refused(RefusalReason::UnknownAccounting)),
            3 => Ok(StorageError::Refused(RefusalReason::FreeSpaceUnavailable)),
            4 => Ok(StorageError::Refused(RefusalReason::FreeSpaceReserve)),
            5 => Ok(StorageError::Refused(RefusalReason::BudgetExceeded)),
            6 => Ok(StorageError::Refused(RefusalReason::NoIdleStorage)),
            7 => Ok(StorageError::Refused(RefusalReason::LeaseUnavailable)),
            _ => Err(StorageError::Protocol("unknown refusal reason")),
        },
        _ => Err(StorageError::Protocol("unknown error response")),
    }
}

fn encode_inventory(writer: &mut Writer, inventory: &Inventory) -> Result<(), StorageError> {
    inventory.budget.encode(writer)?;
    encode_optional_u64(writer, inventory.accounted_bytes);
    encode_optional_u64(writer, inventory.free_bytes);
    writer.u32(inventory.slots.len() as u32);
    for slot in &inventory.slots {
        slot.slot_id.encode(writer)?;
        slot.partition.encode(writer)?;
        slot.state.encode(writer)?;
        encode_optional_u64(writer, slot.measured_bytes);
        writer.u64(slot.reserved_bytes);
        match slot.owner.as_ref() {
            Some(owner) => {
                writer.u8(1);
                owner.encode(writer)?;
            }
            None => writer.u8(0),
        }
    }
    Ok(())
}

fn decode_inventory(reader: &mut Reader<'_>) -> Result<Inventory, StorageError> {
    let budget = Budget::decode(reader)?;
    let accounted_bytes = decode_optional_u64(reader)?;
    let free_bytes = decode_optional_u64(reader)?;
    let count = reader.count()?;
    let mut slots = Vec::with_capacity(count);
    for _ in 0..count {
        let slot_id = String::decode(reader)?;
        let partition = PartitionKey::decode(reader)?;
        let state = SlotState::decode(reader)?;
        let measured_bytes = decode_optional_u64(reader)?;
        let reserved_bytes = reader.u64()?;
        let owner = match reader.u8()? {
            0 => None,
            1 => Some(RunBinding::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid owner field")),
        };
        slots.push(InventorySlot {
            slot_id,
            partition,
            state,
            measured_bytes,
            reserved_bytes,
            owner,
        });
    }
    Ok(Inventory {
        budget,
        accounted_bytes,
        free_bytes,
        slots,
    })
}

fn encode_acquire_result(writer: &mut Writer, result: &AcquireResult) -> Result<(), StorageError> {
    result.lease_id.encode(writer)?;
    result.state.encode(writer)?;
    match result.slot_path.as_ref() {
        Some(path) => {
            writer.u8(1);
            writer.bytes(path.as_os_str().as_bytes())?;
        }
        None => writer.u8(0),
    }
    match result.token.as_ref() {
        Some(token) => {
            writer.u8(1);
            writer.string(token.value())?;
        }
        None => writer.u8(0),
    }
    writer.u8(u8::from(result.reused));
    Ok(())
}

fn decode_acquire_result(reader: &mut Reader<'_>) -> Result<AcquireResult, StorageError> {
    let lease_id = String::decode(reader)?;
    let state = SlotState::decode(reader)?;
    let slot_path = match reader.u8()? {
        0 => None,
        1 => Some(PathBuf::from(OsString::from_vec(reader.bytes()?))),
        _ => return Err(StorageError::Protocol("invalid path field")),
    };
    let token = match reader.u8()? {
        0 => None,
        1 => Some(LeaseToken::new(reader.string()?)),
        _ => return Err(StorageError::Protocol("invalid token field")),
    };
    let reused = match reader.u8()? {
        0 => false,
        1 => true,
        _ => return Err(StorageError::Protocol("invalid reused flag")),
    };
    Ok(AcquireResult {
        lease_id,
        state,
        slot_path,
        token,
        reused,
    })
}

fn encode_finalization(
    writer: &mut Writer,
    receipt: &FinalizationReceipt,
) -> Result<(), StorageError> {
    receipt.lease_id.encode(writer)?;
    receipt.finalize_id.encode(writer)?;
    receipt.state.encode(writer)?;
    encode_optional_string(writer, receipt.capture_digest.as_ref())
}

fn decode_finalization(reader: &mut Reader<'_>) -> Result<FinalizationReceipt, StorageError> {
    Ok(FinalizationReceipt {
        lease_id: String::decode(reader)?,
        finalize_id: String::decode(reader)?,
        state: SlotState::decode(reader)?,
        capture_digest: decode_optional_string(reader)?,
    })
}

fn encode_reconcile(writer: &mut Writer, receipt: &ReconcileReceipt) -> Result<(), StorageError> {
    receipt.lease_id.encode(writer)?;
    receipt.reconcile_id.encode(writer)?;
    receipt.state.encode(writer)
}

fn decode_reconcile(reader: &mut Reader<'_>) -> Result<ReconcileReceipt, StorageError> {
    Ok(ReconcileReceipt {
        lease_id: String::decode(reader)?,
        reconcile_id: String::decode(reader)?,
        state: SlotState::decode(reader)?,
    })
}

fn encode_optional_u64(writer: &mut Writer, value: Option<u64>) {
    match value {
        Some(value) => {
            writer.u8(1);
            writer.u64(value);
        }
        None => writer.u8(0),
    }
}

fn encode_token(writer: &mut Writer, token: Option<&LeaseToken>) -> Result<(), StorageError> {
    match token {
        Some(token) => {
            writer.u8(1);
            writer.string(token.value())
        }
        None => {
            writer.u8(0);
            Ok(())
        }
    }
}

fn decode_token(reader: &mut Reader<'_>) -> Result<Option<LeaseToken>, StorageError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(LeaseToken::new(reader.string()?))),
        _ => Err(StorageError::Protocol("invalid token field")),
    }
}

fn decode_optional_u64(reader: &mut Reader<'_>) -> Result<Option<u64>, StorageError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(reader.u64()?)),
        _ => Err(StorageError::Protocol("invalid optional byte field")),
    }
}

fn encode_optional_string(writer: &mut Writer, value: Option<&String>) -> Result<(), StorageError> {
    match value {
        Some(value) => {
            writer.u8(1);
            value.encode(writer)
        }
        None => {
            writer.u8(0);
            Ok(())
        }
    }
}

fn decode_optional_string(reader: &mut Reader<'_>) -> Result<Option<String>, StorageError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(String::decode(reader)?)),
        _ => Err(StorageError::Protocol("invalid optional string field")),
    }
}

fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, StorageError> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).map_err(io_error)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(StorageError::Protocol("frame exceeds the protocol limit"));
    }
    let mut bytes = vec![0_u8; length];
    stream.read_exact(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn write_frame(stream: &mut UnixStream, bytes: Vec<u8>) -> Result<(), StorageError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(StorageError::Protocol("frame exceeds the protocol limit"));
    }
    let length =
        u32::try_from(bytes.len()).map_err(|_| StorageError::Protocol("frame length overflow"))?;
    stream.write_all(&length.to_le_bytes()).map_err(io_error)?;
    stream.write_all(&bytes).map_err(io_error)
}

fn io_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}
