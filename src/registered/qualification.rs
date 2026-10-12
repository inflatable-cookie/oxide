//! Private, explicitly installed qualification authentication and process bootstrap.
//! No key is discovered from task data or the environment. Same-user bypasses
//! remain outside the cooperative writer model.

use super::runner_host::{
    canonical_json_bytes, HostError, OwnerReceiptVerifier, RunnerHost, Timing,
    MAX_QUALIFIED_SESSION,
};

pub use super::audit_owner::{
    audit_record_receipt, describe_audit_record, verify_audit_record, AuditOwnerAuthority,
    AuditOwnerRuntime, AuditRecord,
};
pub use super::graph_owner::{
    accept_graph_provision, issue_graph_provision, GraphOwnerAuthority, GraphProvision,
    GRAPH_OWNER_ISSUER, GRAPH_OWNER_PROVISION_DOMAIN, GRAPH_OWNER_SCHEMA, GRAPH_OWNER_VERSION,
};
use crate::storage::{
    AcquireResult, ContainmentEvidence, EvidenceAuthority, OrdinaryCargoRequest, RunEnvelope,
    SettlementEvidence, TerminalOutcome,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BOOTSTRAP_BYTES: u64 = 64 * 1024;
const MAX_MAC_BYTES: usize = 1024 * 1024;
const PIN: &str = "d9ff17988c7eaa5cf57747bc55bfe4f138692f15";
/// Explicit, supported finite-timing policy. Any other identifier refuses.
pub const TIMING_POLICY_V1: &str = "oxide-qualification-timing-v1";
const TIMING_RECEIPT_DOMAIN: &str = "oxide-qualification-timing-receipt-v1";

/// Explicitly supplied 256-bit qualification key. Debug never exposes it.
#[derive(Clone)]
pub struct MacKey([u8; 32]);
impl fmt::Debug for MacKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MacKey([redacted])")
    }
}
impl MacKey {
    pub fn from_hex(value: &str) -> Result<Self, HostError> {
        Ok(Self(decode_hex(value).ok_or(HostError::Protocol)?))
    }
    /// HMAC-SHA256 over u32 big-endian length-prefixed UTF-8 fields.
    /// The first field is the versioned domain. The remaining fields bind the
    /// qualification context and the exact subject. All fields are bounded.
    pub fn sign(&self, domain: &str, fields: &[&str]) -> Result<String, HostError> {
        let bytes = mac_frame(domain, fields)?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).map_err(|_| HostError::Protocol)?;
        mac.update(&bytes);
        Ok(format!(
            "hmac-sha256-v1:{}",
            hex(&mac.finalize().into_bytes())
        ))
    }
    pub fn verify(&self, domain: &str, fields: &[&str], receipt: &str) -> bool {
        let Some(tag) = receipt.strip_prefix("hmac-sha256-v1:").and_then(decode_hex) else {
            return false;
        };
        let Ok(bytes) = mac_frame(domain, fields) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.0) else {
            return false;
        };
        mac.update(&bytes);
        mac.verify_slice(&tag).is_ok()
    }
}

/// Cross-language receipt input encoding. No concatenation ambiguity or JSON
/// number conversion is permitted in this layer.
pub fn mac_frame(domain: &str, fields: &[&str]) -> Result<Vec<u8>, HostError> {
    if domain.is_empty() || domain.len() > 128 || fields.len() > 16 {
        return Err(HostError::Protocol);
    }
    let mut out = Vec::new();
    for field in std::iter::once(domain).chain(fields.iter().copied()) {
        if out.len().saturating_add(field.len()).saturating_add(4) > MAX_MAC_BYTES {
            return Err(HostError::Protocol);
        }
        let length = u32::try_from(field.len()).map_err(|_| HostError::Protocol)?;
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    Ok(out)
}

/// Existing host canonical JSON, shared with qualification subjects and vectors.
pub fn canonical_digest(value: &Value) -> Result<String, HostError> {
    let bytes = canonical_json_bytes(value).map_err(|_| HostError::Protocol)?;
    Ok(format!("sha256:{:x}", Sha256::digest(&bytes)))
}

/// Callback authentication is independent of daemon admission/settlement.
#[derive(Debug)]
pub struct CallbackAuthenticator {
    key: MacKey,
    context: String,
}
impl CallbackAuthenticator {
    pub fn new(key: MacKey, context: String) -> Result<Self, HostError> {
        if context.is_empty() || context.len() > 4096 {
            return Err(HostError::Protocol);
        }
        Ok(Self { key, context })
    }
    pub fn receipt(
        &self,
        owner: &str,
        id: &str,
        subject: &str,
        policy: &str,
        result: &str,
    ) -> Result<String, HostError> {
        self.key.sign(
            "oxide-owner-callback-v1",
            &[&self.context, owner, id, subject, policy, result],
        )
    }
}
impl OwnerReceiptVerifier for CallbackAuthenticator {
    fn verify(
        &self,
        owner: &str,
        id: &str,
        subject: &str,
        policy: &str,
        result: &str,
        receipt: &str,
    ) -> bool {
        self.key.verify(
            "oxide-owner-callback-v1",
            &[&self.context, owner, id, subject, policy, result],
            receipt,
        )
    }
}

/// Authenticates an exact set of embedder-approved admission envelopes. Install
/// the daemon's durable instance once, before starting its serve loop. Keys and
/// approved envelopes remain owner-side across sequential attempts.
pub struct QualificationAuthority {
    key: MacKey,
    scope: String,
    allowed: Vec<RunEnvelope>,
    ordinary_request_digests: BTreeSet<String>,
    instance: Mutex<Option<String>>,
}
impl fmt::Debug for QualificationAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("QualificationAuthority([installed])")
    }
}
impl QualificationAuthority {
    pub fn new(key: MacKey, scope: String, allowed: Vec<RunEnvelope>) -> Result<Self, HostError> {
        if scope.is_empty() || scope.len() > 4096 || allowed.is_empty() || allowed.len() > 4 {
            return Err(HostError::Protocol);
        }
        Ok(Self {
            key,
            scope,
            allowed,
            ordinary_request_digests: BTreeSet::new(),
            instance: Mutex::new(None),
        })
    }
    /// Installs exact ordinary-Cargo request digests from protected owner policy.
    /// An empty set keeps ordinary-Cargo issuance disabled.
    pub fn with_ordinary_cargo_request_digests(
        mut self,
        digests: Vec<String>,
    ) -> Result<Self, HostError> {
        if digests.len() > 4
            || digests.iter().any(|digest| !valid_digest(digest))
            || digests.iter().collect::<BTreeSet<_>>().len() != digests.len()
        {
            return Err(HostError::Protocol);
        }
        self.ordinary_request_digests = digests.into_iter().collect();
        Ok(self)
    }
    pub fn install_instance(&self, instance: &str) -> Result<(), HostError> {
        if decode_hex(instance).is_none() {
            return Err(HostError::Protocol);
        }
        let mut installed = self.instance.lock().map_err(|_| HostError::Protocol)?;
        if installed.as_ref().is_some_and(|old| old != instance) {
            return Err(HostError::OwnerRejected);
        }
        *installed = Some(instance.to_owned());
        Ok(())
    }
    pub fn receipt(&self, domain: &str, subject: &Value) -> Result<String, HostError> {
        let instance = self.instance.lock().map_err(|_| HostError::Protocol)?;
        let instance = instance.as_ref().ok_or(HostError::OwnerRejected)?;
        self.key.sign(
            domain,
            &[&self.scope, instance, &canonical_digest(subject)?],
        )
    }
    fn verifies(&self, domain: &str, subject: &Value, receipt: &str) -> bool {
        let Ok(instance) = self.instance.lock() else {
            return false;
        };
        let Some(instance) = instance.as_ref() else {
            return false;
        };
        let Ok(digest) = canonical_digest(subject) else {
            return false;
        };
        self.key
            .verify(domain, &[&self.scope, instance, &digest], receipt)
    }

    fn verify_ordinary_receipt(&self, domain: &str, subject: &Value, receipt: &str) -> bool {
        let Ok(instance) = self.instance.lock() else {
            return false;
        };
        let Some(instance) = instance.as_ref() else {
            return false;
        };
        let Ok(digest) = canonical_digest(subject) else {
            return false;
        };
        self.key
            .verify(domain, &[&self.scope, instance, &digest], receipt)
    }

    fn verifies_admitted_ordinary_request(&self, request: &OrdinaryCargoRequest) -> Option<String> {
        let digest = ordinary_cargo_request_digest(request).ok()?;
        self.ordinary_request_digests
            .contains(&digest)
            .then_some(digest)
    }
}
impl EvidenceAuthority for QualificationAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        let subject = admission_subject(envelope);
        self.allowed
            .iter()
            .any(|allowed| admission_subject(allowed) == subject)
            && self.verifies(
                "oxide-admission-v1",
                &subject,
                envelope.admission_credential.expose(),
            )
    }
    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        if !self
            .allowed
            .iter()
            .any(|allowed| allowed.binding == evidence.binding)
        {
            return false;
        }
        let Some(subject) = settlement_subject(evidence) else {
            return false;
        };
        self.verifies(
            "oxide-settlement-v1",
            &subject,
            &generic_receipt(evidence.consumer_credential.expose()),
        ) && evidence.terminal.as_ref().is_none_or(|terminal| {
            self.verifies(
                "oxide-terminal-v1",
                &subject,
                &generic_receipt(terminal.credential.expose()),
            )
        }) && evidence.capture.as_ref().is_none_or(|capture| {
            self.verifies(
                "oxide-capture-v1",
                &subject,
                &generic_receipt(capture.credential.expose()),
            )
        })
    }

    fn verify_ordinary_cargo_admission(&self, request: &OrdinaryCargoRequest) -> bool {
        let Some(digest) = self.verifies_admitted_ordinary_request(request) else {
            return false;
        };
        let Ok(instance) = self.instance.lock() else {
            return false;
        };
        let Some(instance) = instance.as_ref() else {
            return false;
        };
        self.key.verify(
            "oxide-ordinary-cargo-admission-v1",
            &[&self.scope, instance, &digest],
            request.acceptance.credential.expose(),
        )
    }

    fn verify_ordinary_cargo_settlement(
        &self,
        request: &OrdinaryCargoRequest,
        evidence: &SettlementEvidence,
    ) -> bool {
        let Some(request_digest) = self.verifies_admitted_ordinary_request(request) else {
            return false;
        };
        if evidence.binding != request.acquire.envelope.binding {
            return false;
        }
        let Some(subject) = ordinary_cargo_settlement_subject(&request_digest, evidence) else {
            return false;
        };
        if !self.verify_settlement(evidence) {
            return false;
        }
        let verifies = |credential: &str| {
            self.verify_ordinary_receipt(
                "oxide-ordinary-cargo-settlement-v1",
                &subject,
                &ordinary_receipt(credential),
            )
        };
        verifies(evidence.consumer_credential.expose())
            && evidence
                .terminal
                .as_ref()
                .is_none_or(|terminal| verifies(terminal.credential.expose()))
            && evidence
                .capture
                .as_ref()
                .is_none_or(|capture| verifies(capture.credential.expose()))
    }
}

/// Canonical digest of the complete ordinary request, excluding only its
/// admission and acceptance credentials. Every u64 is represented as decimal
/// text for lossless cross-language identity.
pub fn ordinary_cargo_request_digest(request: &OrdinaryCargoRequest) -> Result<String, HostError> {
    let source_root = request
        .root
        .source_root
        .to_str()
        .ok_or(HostError::Protocol)?;
    canonical_digest(&json!({
        "acquire": {
            "envelope": admission_subject(&request.acquire.envelope),
            "acquire_id": request.acquire.acquire_id,
            "required_bytes": request.acquire.required_bytes.to_string(),
        },
        "root": {
            "source_root": source_root,
            "consumer_root_id": request.root.consumer_root_id,
            "allocation_epoch": request.root.allocation_epoch,
        },
        "partition": request.partition.label(),
        "acceptance": {
            "policy_version": request.acceptance.policy_version,
            "installation_epoch": request.acceptance.installation_epoch,
            "acceptance_reference": request.acceptance.acceptance_reference,
            "input_boundary_digest": request.acceptance.input_boundary_digest,
        }
    }))
}

/// The consumer receipt subject binds terminal, capture and containment
/// evidence to the original ordinary-Cargo request.
pub fn ordinary_cargo_settlement_subject(
    request_digest: &str,
    evidence: &SettlementEvidence,
) -> Option<Value> {
    Some(json!({
        "ordinary_request_digest": request_digest,
        "settlement": settlement_subject(evidence)?,
    }))
}

/// Combine existing settlement authentication with its separate ordinary
/// request-bound receipt while keeping the credential opaque in storage.
pub fn bundle_ordinary_credentials(generic: &str, ordinary: &str) -> Option<String> {
    let generic = generic
        .strip_prefix("hmac-sha256-v1:")
        .and_then(decode_hex)?;
    let ordinary = ordinary
        .strip_prefix("hmac-sha256-v1:")
        .and_then(decode_hex)?;
    Some(format!(
        "hmac-bundle-v1:{}:{}",
        hex(&generic),
        hex(&ordinary)
    ))
}

fn generic_receipt(value: &str) -> String {
    value
        .strip_prefix("hmac-bundle-v1:")
        .and_then(|rest| rest.split_once(':').map(|(tag, _)| tag.to_owned()))
        .map_or_else(|| value.to_owned(), |tag| format!("hmac-sha256-v1:{tag}"))
}

fn ordinary_receipt(value: &str) -> String {
    value
        .strip_prefix("hmac-bundle-v1:")
        .and_then(|rest| rest.split_once(':').map(|(_, tag)| tag.to_owned()))
        .map_or_else(String::new, |tag| format!("hmac-sha256-v1:{tag}"))
}

pub fn admission_subject(envelope: &RunEnvelope) -> Value {
    json!({"binding": envelope.binding, "partition": envelope.partition, "admission_reference": envelope.admission_reference})
}
/// The complete accepted subject, excluding only authentication credentials and
/// Rust's private token MAC. Unknown/running/absence evidence has no authority.
pub fn settlement_subject(e: &SettlementEvidence) -> Option<Value> {
    let containment = match &e.containment {
        ContainmentEvidence::CooperativeWorkflow(c) => json!({
            "kind":"cooperative", "lease_id":c.lease_id,"slot_id":c.slot_id,"slot_path":c.slot_path,
            "temp_dir":c.temp_dir,"capture_dir":c.capture_dir,"workflow":c.workflow,
            "workflow_definition_digest":c.workflow_definition_digest,"repository_head":c.repository_head,
            "fixture_source_digest":c.fixture_source_digest,"host_run_id":c.host_run_id,
            "run_id":c.run_id,"attempt_id":c.attempt_id,"processes":c.processes,
            "free_space_samples":c.free_space_samples.iter().map(|v|super::runner_host::WireU64(*v)).collect::<Vec<_>>(),
            "exit_code":c.exit_code,"stdout_digest":c.stdout_digest,"stderr_digest":c.stderr_digest,
            "capture_reference":c.capture_reference,"capture_digest":c.capture_digest}),
        ContainmentEvidence::NotSpawned(p) => json!({"kind":"not_spawned","lease_id":p.lease_id,
            "slot_id":p.slot_id,"slot_path":p.slot_path,"run_id":p.run_id,"attempt_id":p.attempt_id}),
        _ => return None,
    };
    let terminal = e.terminal.as_ref().map(|t| json!({"run_id":t.run_id,"outcome":match t.outcome {
        TerminalOutcome::Passed => "passed", TerminalOutcome::Failed => "failed", TerminalOutcome::Cancelled => "cancelled" }}));
    let capture = e.capture.as_ref().map(|c| {
        json!({"reference":c.reference,"digest":c.digest,
        "host_run_id":c.host_run_id,"run_id":c.run_id,"attempt_id":c.attempt_id})
    });
    Some(
        json!({"binding":e.binding,"terminal":terminal,"capture":capture,"containment":containment}),
    )
}

/// Public endpoint identity is pinned by the trusted embedder after binding.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}
impl Endpoint {
    pub fn capture(path: PathBuf) -> Result<Self, HostError> {
        let m = fs::symlink_metadata(&path).map_err(|_| HostError::Io)?;
        if !m.file_type().is_socket() || m.mode() & 0o7777 != 0o600 || m.uid() != owner_uid() {
            return Err(HostError::Protocol);
        }
        private_directory(path.parent().ok_or(HostError::Protocol)?)?;
        Ok(Self {
            path,
            device: m.dev(),
            inode: m.ino(),
        })
    }
    fn check(&self) -> Result<(), HostError> {
        let current = Self::capture(self.path.clone())?;
        if current.device != self.device || current.inode != self.inode {
            return Err(HostError::Protocol);
        }
        Ok(())
    }
}

/// One-use file schema. Only the trusted launcher creates this file; task data
/// cannot choose endpoints, roots, keys, or an approved start digest.
///
/// `expires_unix_seconds` is the short load freshness of this file. It is not
/// the execution lifetime. An absent `timing` keeps the unchanged version1
/// host defaults; a present one is an explicit, receipt-bound finite window.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bootstrap {
    pub version: u32,
    pub host_protocol: u32,
    pub storage_protocol: u32,
    pub map_pin: String,
    pub expires_unix_seconds: u64,
    pub scope: String,
    pub storage_instance_id: String,
    pub start_digest: String,
    pub callback_key_hex: String,
    pub parent: Endpoint,
    pub storage: Endpoint,
    pub roots: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<TimingAuthority>,
    /// Absent keeps parent `input.units` in-memory fixture mode. Present installs
    /// the synthetic Cargo reading owner; missing context then refuses rather
    /// than falling back to a parent UnitEvidence array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_owner: Option<GraphOwnerAuthority>,
    /// Accepted audit owner for this bootstrap. Required with `graph_owner`; each
    /// attempt's audit record is verified before graph acceptance or execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_owner: Option<AuditOwnerAuthority>,
}

/// Trusted, receipt-bound finite window for exactly one approved start digest.
/// Units are decimal milliseconds in JSON. `remaining_ms` is the owner's
/// remaining window at issuance; it is not a renewable lifetime.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimingAuthority {
    pub policy: String,
    pub callback_ms: u64,
    pub remaining_ms: u64,
    pub receipt: String,
}

impl TimingAuthority {
    /// Constructor-owned issuance for one bootstrap. Bounds are checked before
    /// signing with the bootstrap's trusted callback key.
    pub fn issue(
        bootstrap: &Bootstrap,
        callback: Duration,
        remaining: Duration,
    ) -> Result<Self, HostError> {
        let (callback_ms, remaining_ms) = (
            u64::try_from(callback.as_millis()).map_err(|_| HostError::Protocol)?,
            u64::try_from(remaining.as_millis()).map_err(|_| HostError::Protocol)?,
        );
        let policy = TIMING_POLICY_V1.to_owned();
        let receipt = Self::receipt(bootstrap, &policy, callback_ms, remaining_ms)?;
        let authority = Self {
            policy,
            callback_ms,
            remaining_ms,
            receipt,
        };
        authority.bounds()?;
        Ok(authority)
    }

    fn receipt(
        bootstrap: &Bootstrap,
        policy: &str,
        callback_ms: u64,
        remaining_ms: u64,
    ) -> Result<String, HostError> {
        let callback = callback_ms.to_string();
        let remaining = remaining_ms.to_string();
        MacKey::from_hex(&bootstrap.callback_key_hex)?.sign(
            TIMING_RECEIPT_DOMAIN,
            &[
                &bootstrap.scope,
                &bootstrap.storage_instance_id,
                &bootstrap.start_digest,
                policy,
                &callback,
                &remaining,
            ],
        )
    }

    fn bounds(&self) -> Result<(Duration, Duration), HostError> {
        if self.policy != TIMING_POLICY_V1
            || self.callback_ms == 0
            || self.remaining_ms == 0
            || self.callback_ms > self.remaining_ms
            || self.remaining_ms > MAX_QUALIFIED_SESSION.as_millis() as u64
        {
            return Err(HostError::Protocol);
        }
        Ok((
            Duration::from_millis(self.callback_ms),
            Duration::from_millis(self.remaining_ms),
        ))
    }

    /// Unsupported or contradictory bounds are Protocol. A receipt that does not
    /// verify for this exact bootstrap (forged, reminted or moved) is OwnerRejected.
    fn verify(&self, bootstrap: &Bootstrap, entry: Instant) -> Result<Timing, HostError> {
        let (callback, remaining) = self.bounds()?;
        let expected = Self::receipt(bootstrap, &self.policy, self.callback_ms, self.remaining_ms)?;
        if expected != self.receipt {
            return Err(HostError::OwnerRejected);
        }
        Timing::qualified(entry, callback, remaining)
    }
}
impl fmt::Debug for Bootstrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bootstrap([private])")
    }
}

#[derive(Debug)]
pub(super) struct AttemptGrant {
    bootstrap: Bootstrap,
    pub(super) timing: Timing,
}
impl AttemptGrant {
    pub(super) fn graph_owner_runtime(
        &self,
    ) -> Result<Option<(MacKey, String, String)>, HostError> {
        if self.bootstrap.graph_owner.is_none() {
            return Ok(None);
        }
        Ok(Some((
            MacKey::from_hex(&self.bootstrap.callback_key_hex)?,
            self.bootstrap.scope.clone(),
            self.bootstrap.start_digest.clone(),
        )))
    }

    pub(super) fn audit_owner_runtime(&self) -> Result<Option<AuditOwnerRuntime>, HostError> {
        let Some(authority) = &self.bootstrap.audit_owner else {
            return Ok(None);
        };
        Ok(Some(AuditOwnerRuntime {
            key: MacKey::from_hex(&self.bootstrap.callback_key_hex)?,
            scope: self.bootstrap.scope.clone(),
            start_digest: self.bootstrap.start_digest.clone(),
            authority: authority.clone(),
        }))
    }

    pub(super) fn check_start(&self, start: &Value) -> Result<(), HostError> {
        let b = &self.bootstrap;
        b.validate()?;
        if canonical_digest(start)? != b.start_digest {
            return Err(HostError::OwnerRejected);
        }
        for version in [
            "version",
            "source_version",
            "dependency_version",
            "configuration_version",
        ] {
            if start["payload"]["registration"][version] != 1 {
                return Err(HostError::Protocol);
            }
        }
        for name in ["source_root", "target", "temp", "capture"] {
            let path = PathBuf::from(
                start["payload"]["attempt"][name]
                    .as_str()
                    .ok_or(HostError::Protocol)?,
            );
            if !b.roots.contains(&path) {
                return Err(HostError::OwnerRejected);
            }
            owned_directory(&path)?;
        }
        // Durable one-use intent prevents a fresh process from blindly replaying
        // an active or ended session. It records no credentials. Ambiguity holds.
        let root = b.parent.path.parent().ok_or(HostError::Protocol)?;
        let marker = root.join(format!("attempt-{}", &b.start_digest[7..]));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(marker)
            .map_err(|_| HostError::OwnerRejected)?;
        file.write_all(b"oxide-qualified-attempt-v1\n")
            .map_err(|_| HostError::Io)?;
        file.sync_all().map_err(|_| HostError::Io)?;
        fs::File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| HostError::Io)?;
        Ok(())
    }
    pub(super) fn check_acquired(&self, acquired: &AcquireResult) -> Result<(), HostError> {
        if acquired.storage_instance_id != self.bootstrap.storage_instance_id {
            return Err(HostError::OwnerRejected);
        }
        let path = acquired.slot_path.as_ref().ok_or(HostError::Storage)?;
        let storage_root = self
            .bootstrap
            .storage
            .path
            .parent()
            .ok_or(HostError::Protocol)?;
        if path != &storage_root.join("slots").join(&acquired.slot_id) {
            return Err(HostError::OwnerRejected);
        }
        Ok(())
    }
}
impl Bootstrap {
    pub fn callback_context(&self) -> Result<String, HostError> {
        canonical_digest(
            &json!({"scope":self.scope,"storage_instance_id":self.storage_instance_id,"start_digest":self.start_digest}),
        )
    }
    fn validate(&self) -> Result<(), HostError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| HostError::Protocol)?
            .as_secs();
        if self.version != 1
            || self.host_protocol != 1
            || self.storage_protocol != crate::storage::STORAGE_PROTOCOL_VERSION
            || self.map_pin != PIN
            || self.expires_unix_seconds < now
            || self.expires_unix_seconds > now.saturating_add(30)
            || self.scope.is_empty()
            || self.scope.len() > 4096
            || self.roots.len() != 4
            || decode_hex(&self.storage_instance_id).is_none()
            || !valid_digest(&self.start_digest)
        {
            return Err(HostError::Protocol);
        }
        self.parent.check()?;
        self.storage.check()?;
        for root in &self.roots {
            owned_directory(root)?;
        }
        Ok(())
    }
}

/// Consumes and unlinks one private bootstrap, then connects to exactly one
/// pinned parent stream. The daemon is owner-launched, using the real API and
/// independently installed QualificationAuthority. No service is installed.
pub fn serve_bootstrap(path: &Path) -> Result<(), HostError> {
    // Entry freezes the finite window: bootstrap read, validation, connect and
    // handshake all draw from the owner's remaining allowance.
    let entry = Instant::now();
    let mut bytes = read_protected(path)?;
    let bootstrap: Bootstrap = serde_json::from_slice(&bytes).map_err(|_| HostError::Protocol)?;
    bootstrap.validate()?;
    // Absent timing is the unchanged version1 default; present timing must verify
    // before the one-use file is consumed.
    let timing = match &bootstrap.timing {
        None => Timing::Version1,
        Some(authority) => authority.verify(&bootstrap, entry)?,
    };
    // A graph owner without accepted audit provenance would reach execution unaudited.
    if bootstrap.graph_owner.is_some() && bootstrap.audit_owner.is_none() {
        return Err(HostError::Protocol);
    }
    if let Some(owner) = &bootstrap.audit_owner {
        owner.verify(&bootstrap)?;
    }
    if let Some(owner) = &bootstrap.graph_owner {
        owner.verify(&bootstrap)?;
    }
    let verifier = CallbackAuthenticator::new(
        MacKey::from_hex(&bootstrap.callback_key_hex)?,
        bootstrap.callback_context()?,
    )?;
    fs::remove_file(path).map_err(|_| HostError::Io)?;
    // Disposal removes the owner-side file before any attempt effects. Memory
    // is process-lifetime; no persistence, logs or secret recovery are provided.
    bytes.fill(0);
    let stream = UnixStream::connect(&bootstrap.parent.path).map_err(|_| HostError::Io)?;
    bootstrap.parent.check()?;
    RunnerHost::new(bootstrap.storage.path.clone(), Arc::new(verifier))
        .qualified(AttemptGrant { bootstrap, timing })
        .serve(stream)
        .map(|_| ())
}
/// Reads one owner-only, single-link, bounded file without following links.
/// The named and opened identities must agree before and after the read.
/// The caller owns consumption and unlinking after its own validation.
pub(crate) fn read_protected(path: &Path) -> Result<Vec<u8>, HostError> {
    private_directory(path.parent().ok_or(HostError::Protocol)?)?;
    if fs::canonicalize(path).map_err(|_| HostError::Io)? != path {
        return Err(HostError::Protocol);
    }
    let before = fs::symlink_metadata(path).map_err(|_| HostError::Io)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| HostError::Io)?;
    let opened = file.metadata().map_err(|_| HostError::Io)?;
    if !opened.is_file()
        || opened.mode() & 0o7777 != 0o600
        || opened.uid() != owner_uid()
        || opened.nlink() != 1
        || opened.len() == 0
        || opened.len() > MAX_BOOTSTRAP_BYTES
        || !same_file(&before, &opened)
    {
        return Err(HostError::Protocol);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_BOOTSTRAP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| HostError::Io)?;
    let after = file.metadata().map_err(|_| HostError::Io)?;
    let named = fs::symlink_metadata(path).map_err(|_| HostError::Io)?;
    if !same_file(&opened, &after)
        || !same_file(&after, &named)
        || bytes.len() as u64 != opened.len()
    {
        return Err(HostError::Protocol);
    }
    Ok(bytes)
}
pub(crate) fn owner_uid() -> u32 {
    // SAFETY: geteuid takes no pointers or arguments and has no preconditions.
    unsafe { libc::geteuid() }
}
pub(crate) fn private_directory(path: &Path) -> Result<(), HostError> {
    owned_directory(path)?;
    let m = fs::symlink_metadata(path).map_err(|_| HostError::Io)?;
    if m.mode() & 0o7777 != 0o700 {
        return Err(HostError::Protocol);
    }
    Ok(())
}
pub(super) fn owned_directory(path: &Path) -> Result<(), HostError> {
    let m = fs::symlink_metadata(path).map_err(|_| HostError::Io)?;
    if !m.is_dir()
        || m.uid() != owner_uid()
        || m.mode() & 0o022 != 0
        || fs::canonicalize(path).map_err(|_| HostError::Io)? != path
    {
        return Err(HostError::Protocol);
    }
    Ok(())
}
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mode() == b.mode()
        && a.uid() == b.uid()
        && a.nlink() == b.nlink()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
pub(super) fn decode_hex(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut key = [0; 32];
    for (byte, pair) in key.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let pair = std::str::from_utf8(pair).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(key)
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").and_then(decode_hex).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;

    #[test]
    fn private_qualification_durable_marker_refuses_fresh_bootstrap_replay() {
        let scratch = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("canonical task scratch");
        let root = PathBuf::from(scratch).join("pure-replay-marker");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let mut roots = Vec::new();
        for name in ["source", "target", "temp", "capture"] {
            let path = root.join(name);
            fs::create_dir(&path).unwrap();
            roots.push(path);
        }
        let parent = UnixListener::bind(root.join("parent.sock")).unwrap();
        let storage = UnixListener::bind(root.join("storage.sock")).unwrap();
        for name in ["parent.sock", "storage.sock"] {
            fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600)).unwrap();
        }
        let start = json!({"payload":{"registration":{"version":1,"source_version":1,"dependency_version":1,"configuration_version":1},
            "attempt":{"source_root":roots[0],"target":roots[1],"temp":roots[2],"capture":roots[3]}}});
        let b = Bootstrap {
            version: 1,
            host_protocol: 1,
            storage_protocol: crate::storage::STORAGE_PROTOCOL_VERSION,
            map_pin: PIN.into(),
            expires_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 30,
            scope: "pure-marker".into(),
            storage_instance_id: "a".repeat(64),
            start_digest: canonical_digest(&start).unwrap(),
            callback_key_hex: "b".repeat(64),
            parent: Endpoint::capture(root.join("parent.sock")).unwrap(),
            storage: Endpoint::capture(root.join("storage.sock")).unwrap(),
            roots,
            timing: None,
            graph_owner: None,
            audit_owner: None,
        };
        let grant = AttemptGrant {
            bootstrap: b,
            timing: Timing::Version1,
        };
        grant.check_start(&start).unwrap();
        assert_eq!(grant.check_start(&start), Err(HostError::OwnerRejected));
        let mut changed = start.clone();
        changed["payload"]["attempt"]["source_root"] = json!("changed");
        assert_eq!(grant.check_start(&changed), Err(HostError::OwnerRejected));
        let marker = root.join(format!("attempt-{}", &grant.bootstrap.start_digest[7..]));
        assert_eq!(
            fs::read_to_string(marker).unwrap(),
            "oxide-qualified-attempt-v1\n"
        );
        drop(parent);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    fn timing_bootstrap(start_digest: &str) -> Bootstrap {
        let endpoint = |name: &str| Endpoint {
            path: PathBuf::from(name),
            device: 0,
            inode: 0,
        };
        Bootstrap {
            version: 1,
            host_protocol: 1,
            storage_protocol: crate::storage::STORAGE_PROTOCOL_VERSION,
            map_pin: PIN.into(),
            expires_unix_seconds: 0,
            scope: "timing-scope".into(),
            storage_instance_id: "a".repeat(64),
            start_digest: start_digest.into(),
            callback_key_hex: "c".repeat(64),
            parent: endpoint("parent"),
            storage: endpoint("storage"),
            roots: Vec::new(),
            timing: None,
            graph_owner: None,
            audit_owner: None,
        }
    }

    #[test]
    fn private_qualification_timing_authority_is_receipt_bound_and_bounded() {
        let digest_one = format!("sha256:{}", "b".repeat(64));
        let digest_two = format!("sha256:{}", "d".repeat(64));
        let mut b = timing_bootstrap(&digest_one);
        let other = timing_bootstrap(&digest_two);
        let second = Duration::from_secs(1);
        let issued = TimingAuthority::issue(&b, second, 4 * second).unwrap();
        let other_issued = TimingAuthority::issue(&other, second, 4 * second).unwrap();
        let entry = Instant::now();
        assert_eq!(
            issued.verify(&b, entry),
            Ok(Timing::Qualified {
                callback: second,
                session_deadline: entry + 4 * second,
            })
        );
        assert!(TimingAuthority::issue(&b, Duration::ZERO, second).is_err());
        assert!(TimingAuthority::issue(&b, second, MAX_QUALIFIED_SESSION + second).is_err());

        let mut unsupported = issued.clone();
        unsupported.policy = "oxide-qualification-timing-v0".into();
        assert_eq!(
            unsupported.verify(&b, entry),
            Err(HostError::Protocol),
            "unknown policy identifiers refuse"
        );
        let mut zero = issued.clone();
        zero.callback_ms = 0;
        assert_eq!(zero.verify(&b, entry), Err(HostError::Protocol));
        let mut contradictory = issued.clone();
        contradictory.callback_ms = 5_000;
        assert_eq!(contradictory.verify(&b, entry), Err(HostError::Protocol));
        let mut over_ceiling = issued.clone();
        over_ceiling.remaining_ms = MAX_QUALIFIED_SESSION.as_millis() as u64 + 1;
        assert_eq!(over_ceiling.verify(&b, entry), Err(HostError::Protocol));
        let mut widened = issued.clone();
        widened.remaining_ms = 3_000;
        assert_eq!(
            widened.verify(&b, entry),
            Err(HostError::OwnerRejected),
            "changed bounds keep the old receipt and refuse"
        );
        assert_eq!(
            other_issued.verify(&b, entry),
            Err(HostError::OwnerRejected),
            "a receipt minted for another start digest refuses"
        );
        b.start_digest = digest_two;
        assert_eq!(
            issued.verify(&b, entry),
            Err(HostError::OwnerRejected),
            "a moved bootstrap digest refuses"
        );
    }
}
