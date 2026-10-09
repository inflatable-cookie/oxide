use super::types::*;

const MAX_STRING_BYTES: usize = 64 * 1024;
const MAX_ITEMS: usize = 16 * 1024;

pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn new() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn string(&mut self, value: &str) -> Result<(), StorageError> {
        if value.len() > MAX_STRING_BYTES {
            return Err(StorageError::InvalidInput(
                "field exceeds the protocol limit",
            ));
        }
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> Result<(), StorageError> {
        if value.len() > MAX_STRING_BYTES {
            return Err(StorageError::InvalidInput(
                "field exceeds the protocol limit",
            ));
        }
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
        Ok(())
    }
}

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub(crate) fn done(&self) -> bool {
        self.offset == self.bytes.len()
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], StorageError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(StorageError::Protocol("length overflow"))?;
        if end > self.bytes.len() {
            return Err(StorageError::Protocol("truncated message"));
        }
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, StorageError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32, StorageError> {
        let bytes = self.take(4)?;
        let mut value = [0; 4];
        value.copy_from_slice(bytes);
        Ok(u32::from_le_bytes(value))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, StorageError> {
        let bytes = self.take(8)?;
        let mut value = [0; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    }

    pub(crate) fn string(&mut self) -> Result<String, StorageError> {
        let length = self.u32()? as usize;
        if length > MAX_STRING_BYTES {
            return Err(StorageError::Protocol("field exceeds the protocol limit"));
        }
        String::from_utf8(self.take(length)?.to_vec())
            .map_err(|_| StorageError::Protocol("field is not UTF-8"))
    }

    pub(crate) fn bytes(&mut self) -> Result<Vec<u8>, StorageError> {
        let length = self.u32()? as usize;
        if length > MAX_STRING_BYTES {
            return Err(StorageError::Protocol("field exceeds the protocol limit"));
        }
        Ok(self.take(length)?.to_vec())
    }

    pub(crate) fn count(&mut self) -> Result<usize, StorageError> {
        let count = self.u32()? as usize;
        if count > MAX_ITEMS {
            return Err(StorageError::Protocol(
                "item count exceeds the protocol limit",
            ));
        }
        Ok(count)
    }
}

pub(crate) trait Encode {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError>;
}

pub(crate) trait Decode: Sized {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError>;
}

impl Encode for String {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.string(self)
    }
}

impl Decode for String {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        reader.string()
    }
}

impl Encode for u64 {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u64(*self);
        Ok(())
    }
}

impl Decode for u64 {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        reader.u64()
    }
}

impl Encode for PartitionKey {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.project_component.encode(writer)?;
        self.workflow.encode(writer)?;
        self.cargo_provenance.encode(writer)?;
        self.rustc_provenance.encode(writer)?;
        self.host.encode(writer)?;
        self.target.encode(writer)?;
        self.profile.encode(writer)?;
        encode_vec(writer, &self.features)?;
        self.config_digest.encode(writer)?;
        self.environment_digest.encode(writer)?;
        self.path_identity.encode(writer)
    }
}

impl Decode for PartitionKey {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        let project_component = String::decode(reader)?;
        let workflow = String::decode(reader)?;
        let cargo_provenance = String::decode(reader)?;
        let rustc_provenance = String::decode(reader)?;
        let host = String::decode(reader)?;
        let target = String::decode(reader)?;
        let profile = String::decode(reader)?;
        let features = decode_vec(reader)?;
        Ok(Self {
            project_component,
            workflow,
            cargo_provenance,
            rustc_provenance,
            host,
            target,
            profile,
            features,
            config_digest: String::decode(reader)?,
            environment_digest: String::decode(reader)?,
            path_identity: String::decode(reader)?,
        })
    }
}

impl Encode for RunBinding {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.consumer.encode(writer)?;
        self.project.encode(writer)?;
        self.component.encode(writer)?;
        self.workflow.encode(writer)?;
        self.task_id.encode(writer)?;
        self.workspace_id.encode(writer)?;
        self.run_id.encode(writer)?;
        self.attempt_id.encode(writer)?;
        self.commit.encode(writer)?;
        self.source_tree_digest.encode(writer)
    }
}

impl Decode for RunBinding {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            consumer: String::decode(reader)?,
            project: String::decode(reader)?,
            component: String::decode(reader)?,
            workflow: String::decode(reader)?,
            task_id: String::decode(reader)?,
            workspace_id: String::decode(reader)?,
            run_id: String::decode(reader)?,
            attempt_id: String::decode(reader)?,
            commit: String::decode(reader)?,
            source_tree_digest: String::decode(reader)?,
        })
    }
}

impl Encode for Credential {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.string(self.expose())
    }
}

impl Decode for Credential {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self::new(reader.string()?))
    }
}

impl Encode for RunEnvelope {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.binding.encode(writer)?;
        self.partition.encode(writer)?;
        self.admission_reference.encode(writer)?;
        self.admission_credential.encode(writer)
    }
}

impl Decode for RunEnvelope {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            binding: RunBinding::decode(reader)?,
            partition: PartitionKey::decode(reader)?,
            admission_reference: String::decode(reader)?,
            admission_credential: Credential::decode(reader)?,
        })
    }
}

impl Encode for AcquireRequest {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.envelope.encode(writer)?;
        self.acquire_id.encode(writer)?;
        self.required_bytes.encode(writer)
    }
}

impl Decode for AcquireRequest {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            envelope: RunEnvelope::decode(reader)?,
            acquire_id: String::decode(reader)?,
            required_bytes: u64::decode(reader)?,
        })
    }
}

impl Encode for TerminalOutcome {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u8(match self {
            Self::Passed => 1,
            Self::Failed => 2,
            Self::Cancelled => 3,
        });
        Ok(())
    }
}

impl Decode for TerminalOutcome {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            1 => Ok(Self::Passed),
            2 => Ok(Self::Failed),
            3 => Ok(Self::Cancelled),
            _ => Err(StorageError::Protocol("unknown terminal outcome")),
        }
    }
}

impl Encode for TerminalRecord {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.run_id.encode(writer)?;
        self.outcome.encode(writer)?;
        self.credential.encode(writer)
    }
}

impl Decode for TerminalRecord {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            run_id: String::decode(reader)?,
            outcome: TerminalOutcome::decode(reader)?,
            credential: Credential::decode(reader)?,
        })
    }
}

impl Encode for CaptureReceipt {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.reference.encode(writer)?;
        self.digest.encode(writer)?;
        self.host_run_id.encode(writer)?;
        self.run_id.encode(writer)?;
        self.attempt_id.encode(writer)?;
        self.credential.encode(writer)
    }
}

impl Decode for CaptureReceipt {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            reference: String::decode(reader)?,
            digest: String::decode(reader)?,
            host_run_id: String::decode(reader)?,
            run_id: String::decode(reader)?,
            attempt_id: String::decode(reader)?,
            credential: Credential::decode(reader)?,
        })
    }
}

impl Encode for ProcessIdentityEvidence {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        writer.u32(self.pid);
        self.start_identity.encode(writer)
    }
}

impl Decode for ProcessIdentityEvidence {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            pid: reader.u32()?,
            start_identity: String::decode(reader)?,
        })
    }
}

impl Encode for CooperativeCompletionEvidence {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.lease_id.encode(writer)?;
        self.slot_id.encode(writer)?;
        self.slot_path.encode(writer)?;
        self.temp_dir.encode(writer)?;
        self.capture_dir.encode(writer)?;
        self.workflow.encode(writer)?;
        self.workflow_definition_digest.encode(writer)?;
        self.repository_head.encode(writer)?;
        self.fixture_source_digest.encode(writer)?;
        self.host_run_id.encode(writer)?;
        self.run_id.encode(writer)?;
        self.attempt_id.encode(writer)?;
        encode_vec(writer, &self.processes)?;
        encode_vec(writer, &self.free_space_samples)?;
        writer.u32(self.exit_code as u32);
        self.stdout_digest.encode(writer)?;
        self.stderr_digest.encode(writer)?;
        self.capture_reference.encode(writer)?;
        self.capture_digest.encode(writer)?;
        self.signature.encode(writer)
    }
}

impl Decode for CooperativeCompletionEvidence {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            lease_id: String::decode(reader)?,
            slot_id: String::decode(reader)?,
            slot_path: String::decode(reader)?,
            temp_dir: String::decode(reader)?,
            capture_dir: String::decode(reader)?,
            workflow: String::decode(reader)?,
            workflow_definition_digest: String::decode(reader)?,
            repository_head: String::decode(reader)?,
            fixture_source_digest: String::decode(reader)?,
            host_run_id: String::decode(reader)?,
            run_id: String::decode(reader)?,
            attempt_id: String::decode(reader)?,
            processes: decode_vec(reader)?,
            free_space_samples: decode_vec(reader)?,
            exit_code: reader.u32()? as i32,
            stdout_digest: String::decode(reader)?,
            stderr_digest: String::decode(reader)?,
            capture_reference: String::decode(reader)?,
            capture_digest: String::decode(reader)?,
            signature: String::decode(reader)?,
        })
    }
}

impl Encode for ContainmentEvidence {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        match self {
            Self::Unknown => writer.u8(0),
            Self::Running {
                pid,
                start_identity,
            } => {
                writer.u8(1);
                writer.u32(*pid);
                start_identity.encode(writer)?;
            }
            Self::ProcessAbsent {
                pid,
                start_identity,
            } => {
                writer.u8(2);
                writer.u32(*pid);
                start_identity.encode(writer)?;
            }
            Self::CooperativeWorkflow(completion) => {
                writer.u8(3);
                completion.encode(writer)?;
            }
        }
        Ok(())
    }
}

impl Decode for ContainmentEvidence {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        match reader.u8()? {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Running {
                pid: reader.u32()?,
                start_identity: String::decode(reader)?,
            }),
            2 => Ok(Self::ProcessAbsent {
                pid: reader.u32()?,
                start_identity: String::decode(reader)?,
            }),
            3 => Ok(Self::CooperativeWorkflow(
                CooperativeCompletionEvidence::decode(reader)?,
            )),
            _ => Err(StorageError::Protocol("unknown containment evidence")),
        }
    }
}

impl Encode for SettlementEvidence {
    fn encode(&self, writer: &mut Writer) -> Result<(), StorageError> {
        self.binding.encode(writer)?;
        self.consumer_credential.encode(writer)?;
        encode_option(writer, self.terminal.as_ref())?;
        encode_option(writer, self.capture.as_ref())?;
        self.containment.encode(writer)
    }
}

impl Decode for SettlementEvidence {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, StorageError> {
        Ok(Self {
            binding: RunBinding::decode(reader)?,
            consumer_credential: Credential::decode(reader)?,
            terminal: decode_option(reader)?,
            capture: decode_option(reader)?,
            containment: ContainmentEvidence::decode(reader)?,
        })
    }
}

pub(crate) fn encode_option<T: Encode>(
    writer: &mut Writer,
    value: Option<&T>,
) -> Result<(), StorageError> {
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

pub(crate) fn decode_option<T: Decode>(reader: &mut Reader<'_>) -> Result<Option<T>, StorageError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(T::decode(reader)?)),
        _ => Err(StorageError::Protocol("invalid optional field")),
    }
}

pub(crate) fn encode_vec<T: Encode>(writer: &mut Writer, values: &[T]) -> Result<(), StorageError> {
    if values.len() > MAX_ITEMS {
        return Err(StorageError::InvalidInput("too many items"));
    }
    writer.u32(values.len() as u32);
    for value in values {
        value.encode(writer)?;
    }
    Ok(())
}

pub(crate) fn decode_vec<T: Decode>(reader: &mut Reader<'_>) -> Result<Vec<T>, StorageError> {
    let mut values = Vec::new();
    for _ in 0..reader.count()? {
        values.push(T::decode(reader)?);
    }
    Ok(values)
}
