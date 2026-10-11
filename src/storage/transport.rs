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
const PROTOCOL_MAGIC: &[u8; 8] = b"OXSTOR01";
pub const STORAGE_PROTOCOL_VERSION: u32 = 2;
const PROTOCOL_VERSION: u32 = STORAGE_PROTOCOL_VERSION;

#[cfg(test)]
pub(super) fn send_acquire_and_forget(
    socket_path: &std::path::Path,
    request: AcquireRequest,
) -> Result<(), StorageError> {
    let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
    let mut writer = Writer::new();
    encode_protocol_header(&mut writer)?;
    Request::Acquire(request).encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())?;
    stream.shutdown(std::net::Shutdown::Both).map_err(io_error)
}

#[cfg(test)]
pub(super) fn send_ordinary_acquire_and_forget(
    socket_path: &std::path::Path,
    request: OrdinaryCargoRequest,
) -> Result<(), StorageError> {
    let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
    let mut writer = Writer::new();
    encode_protocol_header(&mut writer)?;
    Request::AcquireOrdinaryCargo(request).encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())?;
    let _discarded_response = read_frame(&mut stream)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn send_ordinary_finalize_and_forget(
    socket_path: &std::path::Path,
    request: OrdinaryCargoFinalizeRequest,
) -> Result<(), StorageError> {
    let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
    let mut writer = Writer::new();
    encode_protocol_header(&mut writer)?;
    Request::FinalizeOrdinaryCargo(request).encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())?;
    let _discarded_response = read_frame(&mut stream)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn send_legacy_acquire_and_forget(
    socket_path: &std::path::Path,
    request: AcquireRequest,
) -> Result<(), StorageError> {
    let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
    let mut writer = Writer::new();
    Request::Acquire(request).encode(&mut writer)?;
    write_frame(&mut stream, writer.finish())?;
    let _ = read_frame(&mut stream)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn send_unsupported_version_acquire_and_forget(
    socket_path: &std::path::Path,
    request: AcquireRequest,
) -> Result<(), StorageError> {
    for version in [PROTOCOL_VERSION - 1, PROTOCOL_VERSION + 1] {
        let mut stream = UnixStream::connect(socket_path).map_err(io_error)?;
        let mut writer = Writer::new();
        writer.bytes(PROTOCOL_MAGIC)?;
        writer.u32(version);
        Request::Acquire(request.clone()).encode(&mut writer)?;
        write_frame(&mut stream, writer.finish())?;
        let _ = read_frame(&mut stream)?;
    }
    Ok(())
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

    /// Looks up the exact admitted acquire request without granting storage access.
    pub fn lookup_acquire(
        &self,
        request: AcquireRequest,
    ) -> Result<LookupResult<LeaseIdentity>, StorageError> {
        match self.call(Request::LookupAcquire(request))? {
            Response::LookupAcquire(result) => result,
            _ => Err(StorageError::Protocol("unexpected acquire lookup response")),
        }
    }

    /// Acquires or exactly replays one accepted ordinary-Cargo generation.
    pub fn acquire_ordinary_cargo(
        &self,
        request: OrdinaryCargoRequest,
    ) -> Result<OrdinaryCargoAcquireResult, StorageError> {
        match self.call(Request::AcquireOrdinaryCargo(request))? {
            Response::AcquireOrdinaryCargo(result) => result,
            _ => Err(StorageError::Protocol(
                "unexpected ordinary Cargo acquire response",
            )),
        }
    }

    /// Looks up the exact ordinary acquire without allocating or granting access.
    pub fn lookup_ordinary_cargo(
        &self,
        request: OrdinaryCargoRequest,
    ) -> Result<LookupResult<OrdinaryCargoLeaseIdentity>, StorageError> {
        match self.call(Request::LookupOrdinaryCargo(request))? {
            Response::LookupOrdinaryCargo(result) => result,
            _ => Err(StorageError::Protocol(
                "unexpected ordinary Cargo acquire lookup response",
            )),
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

    /// Looks up an authenticated finalize intent without repeating settlement.
    pub fn lookup_finalize(
        &self,
        lease_id: &str,
        finalize_id: &str,
        evidence: SettlementEvidence,
    ) -> Result<LookupResult<FinalizationReceipt>, StorageError> {
        match self.call(Request::LookupFinalize {
            lease_id: lease_id.to_owned(),
            operation_id: finalize_id.to_owned(),
            evidence,
        })? {
            Response::LookupFinalize(result) => result,
            _ => Err(StorageError::Protocol(
                "unexpected finalize lookup response",
            )),
        }
    }

    /// Finalizes the exact ordinary request and records its reuse transition.
    pub fn finalize_ordinary_cargo(
        &self,
        request: OrdinaryCargoFinalizeRequest,
    ) -> Result<OrdinaryCargoFinalizationReceipt, StorageError> {
        match self.call(Request::FinalizeOrdinaryCargo(request))? {
            Response::FinalizeOrdinaryCargo(result) => result,
            _ => Err(StorageError::Protocol(
                "unexpected ordinary Cargo finalize response",
            )),
        }
    }

    /// Reads an exact ordinary finalize result without changing state or access.
    pub fn lookup_finalize_ordinary_cargo(
        &self,
        mut request: OrdinaryCargoFinalizeRequest,
    ) -> Result<LookupResult<OrdinaryCargoFinalizationReceipt>, StorageError> {
        request.token = None;
        match self.call(Request::LookupFinalizeOrdinaryCargo(request))? {
            Response::LookupFinalizeOrdinaryCargo(result) => result,
            _ => Err(StorageError::Protocol(
                "unexpected ordinary Cargo finalize lookup response",
            )),
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
        encode_protocol_header(&mut writer)?;
        request.encode(&mut writer)?;
        write_frame(&mut stream, writer.finish())?;
        let bytes = read_frame(&mut stream)?;
        let mut reader = Reader::new(&bytes);
        decode_protocol_header(&mut reader)?;
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
    LookupAcquire(AcquireRequest),
    AcquireOrdinaryCargo(OrdinaryCargoRequest),
    LookupOrdinaryCargo(OrdinaryCargoRequest),
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
    LookupFinalize {
        lease_id: String,
        operation_id: String,
        evidence: SettlementEvidence,
    },
    FinalizeOrdinaryCargo(OrdinaryCargoFinalizeRequest),
    LookupFinalizeOrdinaryCargo(OrdinaryCargoFinalizeRequest),
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
            Self::LookupAcquire(request) => {
                writer.u8(7);
                request.encode(writer)?;
            }
            Self::AcquireOrdinaryCargo(request) => {
                writer.u8(9);
                request.encode(writer)?;
            }
            Self::LookupOrdinaryCargo(request) => {
                writer.u8(10);
                request.encode(writer)?;
            }
            Self::FinalizeOrdinaryCargo(request) => {
                writer.u8(11);
                request.encode(writer)?;
            }
            Self::LookupFinalizeOrdinaryCargo(request) => {
                writer.u8(12);
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
            Self::LookupFinalize {
                lease_id,
                operation_id,
                evidence,
            } => {
                writer.u8(8);
                lease_id.encode(writer)?;
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
            7 => Ok(Self::LookupAcquire(AcquireRequest::decode(reader)?)),
            9 => Ok(Self::AcquireOrdinaryCargo(OrdinaryCargoRequest::decode(
                reader,
            )?)),
            10 => Ok(Self::LookupOrdinaryCargo(OrdinaryCargoRequest::decode(
                reader,
            )?)),
            11 => Ok(Self::FinalizeOrdinaryCargo(
                OrdinaryCargoFinalizeRequest::decode(reader)?,
            )),
            12 => Ok(Self::LookupFinalizeOrdinaryCargo(
                OrdinaryCargoFinalizeRequest::decode(reader)?,
            )),
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
            8 => Ok(Self::LookupFinalize {
                lease_id: String::decode(reader)?,
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
    LookupAcquire(Result<LookupResult<LeaseIdentity>, StorageError>),
    AcquireOrdinaryCargo(Result<OrdinaryCargoAcquireResult, StorageError>),
    LookupOrdinaryCargo(Result<LookupResult<OrdinaryCargoLeaseIdentity>, StorageError>),
    Finalize(Result<FinalizationReceipt, StorageError>),
    Reconcile(Result<ReconcileReceipt, StorageError>),
    LookupFinalize(Result<LookupResult<FinalizationReceipt>, StorageError>),
    FinalizeOrdinaryCargo(Result<OrdinaryCargoFinalizationReceipt, StorageError>),
    LookupFinalizeOrdinaryCargo(
        Result<LookupResult<OrdinaryCargoFinalizationReceipt>, StorageError>,
    ),
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
            Self::LookupAcquire(result) => {
                writer.u8(6);
                encode_result(writer, result, encode_lease_lookup)?;
            }
            Self::AcquireOrdinaryCargo(result) => {
                writer.u8(9);
                encode_result(writer, result, encode_ordinary_acquire_result)?;
            }
            Self::LookupOrdinaryCargo(result) => {
                writer.u8(10);
                encode_result(writer, result, encode_ordinary_lease_lookup)?;
            }
            Self::Finalize(result) => {
                writer.u8(3);
                encode_result(writer, result, encode_finalization)?;
            }
            Self::Reconcile(result) => {
                writer.u8(4);
                encode_result(writer, result, encode_reconcile)?;
            }
            Self::LookupFinalize(result) => {
                writer.u8(7);
                encode_result(writer, result, encode_finalization_lookup)?;
            }
            Self::FinalizeOrdinaryCargo(result) => {
                writer.u8(11);
                encode_result(writer, result, encode_ordinary_finalization)?;
            }
            Self::LookupFinalizeOrdinaryCargo(result) => {
                writer.u8(12);
                encode_result(writer, result, encode_ordinary_finalization_lookup)?;
            }
            Self::Evict(result) => {
                writer.u8(5);
                encode_result(writer, result, |writer, value| value.encode(writer))?;
            }
            Self::Error(error) => {
                writer.u8(8);
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
            6 => Ok(Self::LookupAcquire(decode_result(
                reader,
                decode_lease_lookup,
            ))),
            9 => Ok(Self::AcquireOrdinaryCargo(decode_result(
                reader,
                decode_ordinary_acquire_result,
            ))),
            10 => Ok(Self::LookupOrdinaryCargo(decode_result(
                reader,
                decode_ordinary_lease_lookup,
            ))),
            7 => Ok(Self::LookupFinalize(decode_result(
                reader,
                decode_finalization_lookup,
            ))),
            8 => Ok(Self::Error(decode_error(reader)?)),
            11 => Ok(Self::FinalizeOrdinaryCargo(decode_result(
                reader,
                decode_ordinary_finalization,
            ))),
            12 => Ok(Self::LookupFinalizeOrdinaryCargo(decode_result(
                reader,
                decode_ordinary_finalization_lookup,
            ))),
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
        Err(error) => return write_response(&mut stream, Response::Error(error)),
    };
    let request = (|| {
        let mut reader = Reader::new(&request_bytes);
        decode_protocol_header(&mut reader)?;
        let request = Request::decode(&mut reader)?;
        if !reader.done() {
            return Err(StorageError::Protocol("trailing request data"));
        }
        Ok(request)
    })();
    let request = match request {
        Ok(request) => request,
        Err(error) => return write_response(&mut stream, Response::Error(error)),
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
        Request::LookupAcquire(request) => Response::LookupAcquire(store.lookup_acquire(request)),
        Request::AcquireOrdinaryCargo(request) => {
            Response::AcquireOrdinaryCargo(store.acquire_ordinary_cargo(request))
        }
        Request::LookupOrdinaryCargo(request) => {
            Response::LookupOrdinaryCargo(store.lookup_ordinary_cargo(request))
        }
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
        Request::LookupFinalize {
            lease_id,
            operation_id,
            evidence,
        } => Response::LookupFinalize(store.lookup_finalize(&lease_id, &operation_id, evidence)),
        Request::FinalizeOrdinaryCargo(request) => {
            Response::FinalizeOrdinaryCargo(store.finalize_ordinary_cargo(request))
        }
        Request::LookupFinalizeOrdinaryCargo(request) => {
            Response::LookupFinalizeOrdinaryCargo(store.lookup_finalize_ordinary_cargo(request))
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
    write_response(&mut stream, response)
}

fn encode_protocol_header(writer: &mut Writer) -> Result<(), StorageError> {
    writer.bytes(PROTOCOL_MAGIC)?;
    writer.u32(PROTOCOL_VERSION);
    Ok(())
}

fn decode_protocol_header(reader: &mut Reader<'_>) -> Result<(), StorageError> {
    if reader.bytes()?.as_slice() != &PROTOCOL_MAGIC[..] || reader.u32()? != PROTOCOL_VERSION {
        return Err(StorageError::Protocol(
            "unsupported storage protocol version",
        ));
    }
    Ok(())
}

fn write_response(stream: &mut UnixStream, response: Response) -> Result<(), StorageError> {
    let mut writer = Writer::new();
    encode_protocol_header(&mut writer)?;
    response.encode(&mut writer)?;
    write_frame(stream, writer.finish())
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
                RefusalReason::OrdinaryCargoPending => 8,
                RefusalReason::OrdinaryCargoColdOnly => 9,
                RefusalReason::OrdinaryCargoPartitionLimit => 10,
                RefusalReason::OrdinaryCargoIncompatible => 11,
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
            8 => Ok(StorageError::Refused(RefusalReason::OrdinaryCargoPending)),
            9 => Ok(StorageError::Refused(RefusalReason::OrdinaryCargoColdOnly)),
            10 => Ok(StorageError::Refused(
                RefusalReason::OrdinaryCargoPartitionLimit,
            )),
            11 => Ok(StorageError::Refused(
                RefusalReason::OrdinaryCargoIncompatible,
            )),
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
        match slot.cargo_reuse_state {
            Some(state) => {
                writer.u8(1);
                state.encode(writer)?;
            }
            None => writer.u8(0),
        }
        encode_optional_ordinary_root(writer, slot.ordinary_root.as_ref())?;
        match slot.ordinary_partition.as_ref() {
            Some(partition) => {
                writer.u8(1);
                partition.encode(writer)?;
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
        let cargo_reuse_state = match reader.u8()? {
            0 => None,
            1 => Some(CargoReuseState::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid Cargo reuse state")),
        };
        let ordinary_root = decode_optional_ordinary_root(reader)?;
        let ordinary_partition = match reader.u8()? {
            0 => None,
            1 => Some(OrdinaryCargoPartition::decode(reader)?),
            _ => return Err(StorageError::Protocol("invalid ordinary Cargo partition")),
        };
        slots.push(InventorySlot {
            slot_id,
            partition,
            state,
            measured_bytes,
            reserved_bytes,
            owner,
            cargo_reuse_state,
            ordinary_root,
            ordinary_partition,
        });
    }
    Ok(Inventory {
        budget,
        accounted_bytes,
        free_bytes,
        slots,
    })
}

fn encode_optional_ordinary_root(
    writer: &mut Writer,
    root: Option<&StrictPhysicalCargoRoot>,
) -> Result<(), StorageError> {
    match root {
        Some(root) => {
            writer.u8(1);
            root.encode(writer)?;
        }
        None => writer.u8(0),
    }
    Ok(())
}

fn decode_optional_ordinary_root(
    reader: &mut Reader<'_>,
) -> Result<Option<StrictPhysicalCargoRoot>, StorageError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(StrictPhysicalCargoRoot::decode(reader)?)),
        _ => Err(StorageError::Protocol("invalid ordinary Cargo root")),
    }
}

fn encode_acquire_result(writer: &mut Writer, result: &AcquireResult) -> Result<(), StorageError> {
    validate_storage_identity(
        &result.slot_id,
        &result.storage_instance_id,
        &result.generation,
    )?;
    if (result.state == SlotState::Leased) != (result.slot_path.is_some() && result.token.is_some())
        || (result.state != SlotState::Leased
            && (result.slot_path.is_some() || result.token.is_some()))
    {
        return Err(StorageError::Protocol(
            "acquire result access does not match lease state",
        ));
    }
    result.lease_id.encode(writer)?;
    result.slot_id.encode(writer)?;
    result.storage_instance_id.encode(writer)?;
    result.generation.encode(writer)?;
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
    let slot_id = String::decode(reader)?;
    let storage_instance_id = String::decode(reader)?;
    let generation = String::decode(reader)?;
    validate_storage_identity(&slot_id, &storage_instance_id, &generation)?;
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
    if (state == SlotState::Leased) != (slot_path.is_some() && token.is_some())
        || (state != SlotState::Leased && (slot_path.is_some() || token.is_some()))
    {
        return Err(StorageError::Protocol(
            "acquire result access does not match lease state",
        ));
    }
    Ok(AcquireResult {
        lease_id,
        slot_id,
        storage_instance_id,
        generation,
        state,
        slot_path,
        token,
        reused,
    })
}

fn encode_ordinary_acquire_result(
    writer: &mut Writer,
    result: &OrdinaryCargoAcquireResult,
) -> Result<(), StorageError> {
    writer.u32(result.policy_version);
    result.root.encode(writer)?;
    result.partition.encode(writer)?;
    result.reuse_state.encode(writer)?;
    encode_acquire_result(writer, &result.lease)
}

fn decode_ordinary_acquire_result(
    reader: &mut Reader<'_>,
) -> Result<OrdinaryCargoAcquireResult, StorageError> {
    Ok(OrdinaryCargoAcquireResult {
        policy_version: reader.u32()?,
        root: StrictPhysicalCargoRoot::decode(reader)?,
        partition: OrdinaryCargoPartition::decode(reader)?,
        reuse_state: CargoReuseState::decode(reader)?,
        lease: decode_acquire_result(reader)?,
    })
}

fn encode_ordinary_lease_lookup(
    writer: &mut Writer,
    result: &LookupResult<OrdinaryCargoLeaseIdentity>,
) -> Result<(), StorageError> {
    validate_ordinary_lease_lookup_state(result)?;
    encode_lookup_result(writer, result, |writer, identity| {
        writer.u32(identity.policy_version);
        identity.root.encode(writer)?;
        identity.partition.encode(writer)?;
        identity.reuse_state.encode(writer)?;
        encode_lease_identity(writer, &identity.lease)
    })
}

fn decode_ordinary_lease_lookup(
    reader: &mut Reader<'_>,
) -> Result<LookupResult<OrdinaryCargoLeaseIdentity>, StorageError> {
    let result = decode_lookup_result(reader, |reader| {
        Ok(OrdinaryCargoLeaseIdentity {
            policy_version: reader.u32()?,
            root: StrictPhysicalCargoRoot::decode(reader)?,
            partition: OrdinaryCargoPartition::decode(reader)?,
            reuse_state: CargoReuseState::decode(reader)?,
            lease: decode_lease_identity(reader)?,
        })
    })?;
    validate_ordinary_lease_lookup_state(&result)?;
    Ok(result)
}

fn encode_lease_identity(
    writer: &mut Writer,
    identity: &LeaseIdentity,
) -> Result<(), StorageError> {
    validate_storage_identity(
        &identity.slot_id,
        &identity.storage_instance_id,
        &identity.generation,
    )?;
    identity.lease_id.encode(writer)?;
    identity.slot_id.encode(writer)?;
    identity.storage_instance_id.encode(writer)?;
    identity.generation.encode(writer)?;
    identity.state.encode(writer)
}

fn decode_lease_identity(reader: &mut Reader<'_>) -> Result<LeaseIdentity, StorageError> {
    let lease_id = String::decode(reader)?;
    let slot_id = String::decode(reader)?;
    let storage_instance_id = String::decode(reader)?;
    let generation = String::decode(reader)?;
    validate_storage_identity(&slot_id, &storage_instance_id, &generation)?;
    Ok(LeaseIdentity {
        lease_id,
        slot_id,
        storage_instance_id,
        generation,
        state: SlotState::decode(reader)?,
    })
}

fn validate_ordinary_lease_lookup_state(
    result: &LookupResult<OrdinaryCargoLeaseIdentity>,
) -> Result<(), StorageError> {
    match result {
        LookupResult::Absent => Ok(()),
        LookupResult::Present(identity) if identity.lease.state != SlotState::Uncertain => Ok(()),
        LookupResult::Uncertain(Some(identity))
            if identity.lease.state == SlotState::Uncertain
                && identity.reuse_state == CargoReuseState::Pending =>
        {
            Ok(())
        }
        _ => Err(StorageError::Protocol(
            "ordinary Cargo lookup result does not match lease state",
        )),
    }
}

fn encode_lookup_result<T>(
    writer: &mut Writer,
    result: &LookupResult<T>,
    encode_value: impl Fn(&mut Writer, &T) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    match result {
        LookupResult::Absent => writer.u8(0),
        LookupResult::Present(value) => {
            writer.u8(1);
            encode_value(writer, value)?;
        }
        LookupResult::Uncertain(None) => writer.u8(2),
        LookupResult::Uncertain(Some(value)) => {
            writer.u8(3);
            encode_value(writer, value)?;
        }
    }
    Ok(())
}

fn decode_lookup_result<T>(
    reader: &mut Reader<'_>,
    decode_value: impl Fn(&mut Reader<'_>) -> Result<T, StorageError>,
) -> Result<LookupResult<T>, StorageError> {
    match reader.u8()? {
        0 => Ok(LookupResult::Absent),
        1 => Ok(LookupResult::Present(decode_value(reader)?)),
        2 => Ok(LookupResult::Uncertain(None)),
        3 => Ok(LookupResult::Uncertain(Some(decode_value(reader)?))),
        _ => Err(StorageError::Protocol("unknown lookup result")),
    }
}

fn encode_lease_lookup(
    writer: &mut Writer,
    result: &LookupResult<LeaseIdentity>,
) -> Result<(), StorageError> {
    validate_lookup_identity_state(result)?;
    encode_lookup_result(writer, result, |writer, identity| {
        validate_storage_identity(
            &identity.slot_id,
            &identity.storage_instance_id,
            &identity.generation,
        )?;
        identity.lease_id.encode(writer)?;
        identity.slot_id.encode(writer)?;
        identity.storage_instance_id.encode(writer)?;
        identity.generation.encode(writer)?;
        identity.state.encode(writer)
    })
}

fn decode_lease_lookup(
    reader: &mut Reader<'_>,
) -> Result<LookupResult<LeaseIdentity>, StorageError> {
    let result = decode_lookup_result(reader, |reader| {
        let lease_id = String::decode(reader)?;
        let slot_id = String::decode(reader)?;
        let storage_instance_id = String::decode(reader)?;
        let generation = String::decode(reader)?;
        validate_storage_identity(&slot_id, &storage_instance_id, &generation)?;
        Ok(LeaseIdentity {
            lease_id,
            slot_id,
            storage_instance_id,
            generation,
            state: SlotState::decode(reader)?,
        })
    })?;
    validate_lookup_identity_state(&result)?;
    Ok(result)
}

fn validate_lookup_identity_state(
    result: &LookupResult<LeaseIdentity>,
) -> Result<(), StorageError> {
    match result {
        LookupResult::Absent => Ok(()),
        LookupResult::Present(identity) if identity.state != SlotState::Uncertain => Ok(()),
        LookupResult::Uncertain(Some(identity)) if identity.state == SlotState::Uncertain => Ok(()),
        _ => Err(StorageError::Protocol(
            "lookup result does not match lease state",
        )),
    }
}

fn validate_storage_identity(
    slot_id: &str,
    storage_instance_id: &str,
    generation: &str,
) -> Result<(), StorageError> {
    if !valid_storage_instance_id(storage_instance_id) {
        return Err(StorageError::Protocol("invalid storage instance identity"));
    }
    let ordinal = slot_ordinal(slot_id).ok_or(StorageError::Protocol("invalid slot identity"))?;
    if parse_generation(generation)? != ordinal {
        return Err(StorageError::Protocol(
            "generation does not match slot identity",
        ));
    }
    Ok(())
}

fn encode_finalization_lookup(
    writer: &mut Writer,
    result: &LookupResult<FinalizationReceipt>,
) -> Result<(), StorageError> {
    validate_finalize_lookup_state(result)?;
    encode_lookup_result(writer, result, encode_finalization)
}

fn decode_finalization_lookup(
    reader: &mut Reader<'_>,
) -> Result<LookupResult<FinalizationReceipt>, StorageError> {
    let result = decode_lookup_result(reader, decode_finalization)?;
    validate_finalize_lookup_state(&result)?;
    Ok(result)
}

fn validate_finalize_lookup_state(
    result: &LookupResult<FinalizationReceipt>,
) -> Result<(), StorageError> {
    match result {
        LookupResult::Absent | LookupResult::Uncertain(None) => Ok(()),
        LookupResult::Present(receipt) if receipt.state == SlotState::Idle => Ok(()),
        LookupResult::Uncertain(Some(receipt)) if receipt.state == SlotState::Uncertain => Ok(()),
        _ => Err(StorageError::Protocol(
            "finalize lookup result does not match operation state",
        )),
    }
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

fn encode_ordinary_finalization(
    writer: &mut Writer,
    receipt: &OrdinaryCargoFinalizationReceipt,
) -> Result<(), StorageError> {
    writer.u32(receipt.policy_version);
    receipt.root.encode(writer)?;
    receipt.partition.encode(writer)?;
    receipt.reuse_state.encode(writer)?;
    encode_finalization(writer, &receipt.finalization)
}

fn decode_ordinary_finalization(
    reader: &mut Reader<'_>,
) -> Result<OrdinaryCargoFinalizationReceipt, StorageError> {
    Ok(OrdinaryCargoFinalizationReceipt {
        policy_version: reader.u32()?,
        root: StrictPhysicalCargoRoot::decode(reader)?,
        partition: OrdinaryCargoPartition::decode(reader)?,
        reuse_state: CargoReuseState::decode(reader)?,
        finalization: decode_finalization(reader)?,
    })
}

fn encode_ordinary_finalization_lookup(
    writer: &mut Writer,
    result: &LookupResult<OrdinaryCargoFinalizationReceipt>,
) -> Result<(), StorageError> {
    validate_ordinary_finalization_lookup_state(result)?;
    encode_lookup_result(writer, result, encode_ordinary_finalization)
}

fn decode_ordinary_finalization_lookup(
    reader: &mut Reader<'_>,
) -> Result<LookupResult<OrdinaryCargoFinalizationReceipt>, StorageError> {
    let result = decode_lookup_result(reader, decode_ordinary_finalization)?;
    validate_ordinary_finalization_lookup_state(&result)?;
    Ok(result)
}

fn validate_ordinary_finalization_lookup_state(
    result: &LookupResult<OrdinaryCargoFinalizationReceipt>,
) -> Result<(), StorageError> {
    match result {
        LookupResult::Absent | LookupResult::Uncertain(None) => Ok(()),
        LookupResult::Present(receipt)
            if receipt.finalization.state == SlotState::Idle
                && receipt.reuse_state != CargoReuseState::Pending =>
        {
            Ok(())
        }
        LookupResult::Uncertain(Some(receipt))
            if receipt.finalization.state == SlotState::Uncertain
                && receipt.reuse_state == CargoReuseState::Pending =>
        {
            Ok(())
        }
        _ => Err(StorageError::Protocol(
            "ordinary Cargo finalize lookup result does not match operation state",
        )),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_acquire_result() -> AcquireResult {
        AcquireResult {
            lease_id: "lease-1".to_owned(),
            slot_id: "slot-ffffffffffffffff".to_owned(),
            storage_instance_id: "a".repeat(64),
            generation: u64::MAX.to_string(),
            state: SlotState::Uncertain,
            slot_path: None,
            token: None,
            reused: true,
        }
    }

    fn raw_acquire_result(slot_id: &str, generation: &str) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.string("lease-1").expect("encode lease");
        writer.string(slot_id).expect("encode slot");
        writer.string(&"a".repeat(64)).expect("encode instance");
        writer.string(generation).expect("encode generation");
        SlotState::Uncertain
            .encode(&mut writer)
            .expect("encode state");
        writer.u8(0);
        writer.u8(0);
        writer.u8(1);
        writer.finish()
    }

    #[test]
    fn acquire_result_roundtrips_max_generation_and_rejects_noncanonical_values() {
        let expected = valid_acquire_result();
        let mut writer = Writer::new();
        encode_acquire_result(&mut writer, &expected).expect("encode result");
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            decode_acquire_result(&mut reader).expect("decode result"),
            expected
        );
        assert!(reader.done());

        for generation in ["0", "01", "+1", "18446744073709551616", "١"] {
            let bytes = raw_acquire_result("slot-0000000000000001", generation);
            assert!(decode_acquire_result(&mut Reader::new(&bytes)).is_err());
        }
        let mismatched = raw_acquire_result("slot-0000000000000001", "2");
        assert!(decode_acquire_result(&mut Reader::new(&mismatched)).is_err());
    }

    #[test]
    fn lookup_result_roundtrips_without_writer_access_fields() {
        let expected = LookupResult::Uncertain(Some(LeaseIdentity {
            lease_id: "lease-1".to_owned(),
            slot_id: "slot-0000000000000001".to_owned(),
            storage_instance_id: "b".repeat(64),
            generation: "1".to_owned(),
            state: SlotState::Uncertain,
        }));
        let mut writer = Writer::new();
        encode_lease_lookup(&mut writer, &expected).expect("encode lookup");
        let bytes = writer.finish();
        let mut reader = Reader::new(&bytes);
        assert_eq!(
            decode_lease_lookup(&mut reader).expect("decode lookup"),
            expected
        );
        assert!(reader.done());
    }

    #[test]
    fn protocol_header_rejects_legacy_and_unsupported_peers() {
        let mut writer = Writer::new();
        encode_protocol_header(&mut writer).expect("encode current header");
        let bytes = writer.finish();
        decode_protocol_header(&mut Reader::new(&bytes)).expect("decode current header");

        let mut legacy = Writer::new();
        legacy.u8(3);
        legacy.u32(0);
        assert!(decode_protocol_header(&mut Reader::new(&legacy.finish())).is_err());

        let mut future = Writer::new();
        future.bytes(PROTOCOL_MAGIC).expect("encode protocol magic");
        future.u32(PROTOCOL_VERSION + 1);
        assert!(decode_protocol_header(&mut Reader::new(&future.finish())).is_err());
    }
}
