//! Qualification-only accepted audit-record provenance.
//!
//! The installed audit owner signs one bounded record per acquired attempt. The
//! record binds the registration, semantic graph, complete closure receipt set and
//! attempt identity. `ParentCallbacks::audit` verifies it inside the existing
//! `RegistrationOwner.audit` hook, which runs before any graph acceptance or
//! `ExecutionOwner.execute`. The synthetic issuer is fixture evidence only; it is
//! not the Nucleus closure audit and never proves a complete native/SDK closure.

use super::qualification::{canonical_digest, Bootstrap, MacKey};
use super::runner_host::{HostError, StorageIdentity};
use super::{Attempt, Refusal, Registration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Versioned record schema. Incompatible schema, issuer or version refuses.
pub const AUDIT_RECORD_SCHEMA: &str = "oxide-qualification-audit-record-v1";
/// Explicit synthetic issuer label. Unknown issuers refuse.
pub const AUDIT_RECORD_ISSUER: &str = "synthetic-qualification";
/// Authority and record version. Unchanged map pins do not imply compatibility.
pub const AUDIT_RECORD_VERSION: u32 = 1;
const AUDIT_OWNER_INSTALL_DOMAIN: &str = "oxide-qualification-audit-owner-install-v1";
/// Receipt domain for one audit record bound to an acquired attempt.
pub const AUDIT_RECORD_DOMAIN: &str = "oxide-qualification-audit-record-v1";

/// Bootstrap installation of the accepted qualification audit owner. The owner
/// identity and policy digest are embedder-installed; task JSON cannot name them.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuditOwnerAuthority {
    pub version: u32,
    pub schema: String,
    pub issuer: String,
    pub owner: String,
    pub policy_digest: String,
    pub receipt: String,
}

impl AuditOwnerAuthority {
    /// Constructor-owned issuance for one bootstrap, owner and policy.
    pub fn issue(
        bootstrap: &Bootstrap,
        owner: &str,
        policy_digest: &str,
    ) -> std::result::Result<Self, HostError> {
        let mut authority = Self {
            version: AUDIT_RECORD_VERSION,
            schema: AUDIT_RECORD_SCHEMA.to_owned(),
            issuer: AUDIT_RECORD_ISSUER.to_owned(),
            owner: owner.to_owned(),
            policy_digest: policy_digest.to_owned(),
            receipt: String::new(),
        };
        authority.receipt = authority.install_receipt(bootstrap)?;
        authority.bounds()?;
        Ok(authority)
    }

    fn install_receipt(&self, bootstrap: &Bootstrap) -> std::result::Result<String, HostError> {
        MacKey::from_hex(&bootstrap.callback_key_hex)?.sign(
            AUDIT_OWNER_INSTALL_DOMAIN,
            &[
                &bootstrap.scope,
                &bootstrap.storage_instance_id,
                &bootstrap.start_digest,
                &self.schema,
                &self.issuer,
                &self.owner,
                &self.policy_digest,
                &self.version.to_string(),
            ],
        )
    }

    fn bounds(&self) -> std::result::Result<(), HostError> {
        if self.version != AUDIT_RECORD_VERSION
            || self.schema != AUDIT_RECORD_SCHEMA
            || self.issuer != AUDIT_RECORD_ISSUER
            || self.owner.is_empty()
            || self.owner.len() > 128
            || !valid_digest(&self.policy_digest)
            || self.receipt.is_empty()
            || self.receipt.len() > 1024
        {
            return Err(HostError::Protocol);
        }
        Ok(())
    }

    /// Unsupported schema/issuer/version is Protocol. Forged or reminted installs
    /// are OwnerRejected, and verification runs before the one-use bootstrap is consumed.
    pub fn verify(&self, bootstrap: &Bootstrap) -> std::result::Result<(), HostError> {
        self.bounds()?;
        if self.install_receipt(bootstrap)? != self.receipt {
            return Err(HostError::OwnerRejected);
        }
        Ok(())
    }
}

/// Installed verifier material for one bootstrap. Built only by the attempt grant.
pub struct AuditOwnerRuntime {
    pub key: MacKey,
    pub scope: String,
    pub start_digest: String,
    pub authority: AuditOwnerAuthority,
}

/// Untrusted audit record returned by the parent. Unknown fields refuse.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuditRecord {
    pub version: u32,
    pub schema: String,
    pub issuer: String,
    pub owner: String,
    pub policy_digest: String,
    pub registration_id: String,
    pub graph_digest: String,
    /// Digest of the complete accepted registration, including argv, units,
    /// environment, configuration, toolchain, debug policy and limits.
    pub registration_digest: String,
    /// Digest of the acquired attempt: binding, source root, slot, generation,
    /// lease, partition and authorization. Source tree is part of the binding.
    pub attempt_digest: String,
    /// Exact closure: must equal the registration's immutable receipt set.
    pub closure_receipts: BTreeSet<String>,
    pub receipt: String,
}

/// Canonical digest of the accepted registration used by audit records.
pub fn registration_digest(registration: &Registration) -> std::result::Result<String, HostError> {
    canonical_digest(&serde_json::to_value(registration).map_err(|_| HostError::Protocol)?)
}

/// Canonical digest of the acquired attempt used by audit records.
pub fn attempt_digest(attempt: &Attempt) -> std::result::Result<String, HostError> {
    canonical_digest(&serde_json::to_value(attempt).map_err(|_| HostError::Protocol)?)
}

/// Builds the honest unsigned record for one acquired attempt. Callers sign it
/// with `audit_record_receipt`; tests may mutate fields to build refusal controls.
pub fn describe_audit_record(
    authority: &AuditOwnerAuthority,
    registration: &Registration,
    attempt: &Attempt,
) -> std::result::Result<AuditRecord, HostError> {
    Ok(AuditRecord {
        version: AUDIT_RECORD_VERSION,
        schema: AUDIT_RECORD_SCHEMA.to_owned(),
        issuer: AUDIT_RECORD_ISSUER.to_owned(),
        owner: authority.owner.clone(),
        policy_digest: authority.policy_digest.clone(),
        registration_id: registration.id.clone(),
        graph_digest: registration.graph_digest.clone(),
        registration_digest: registration_digest(registration)?,
        attempt_digest: attempt_digest(attempt)?,
        closure_receipts: registration.immutable_receipts.clone(),
        receipt: String::new(),
    })
}

/// Synthetic signing of one record under the installed key. Test-only evidence;
/// it never stands in for the actual Nucleus closure or audit receipt.
pub fn audit_record_receipt(
    key: &MacKey,
    scope: &str,
    storage_instance_id: &str,
    start_digest: &str,
    record: &AuditRecord,
) -> std::result::Result<String, HostError> {
    let mut body = record.clone();
    body.receipt.clear();
    let subject = canonical_digest(&serde_json::to_value(&body).map_err(|_| HostError::Protocol)?)?;
    key.sign(
        AUDIT_RECORD_DOMAIN,
        &[scope, storage_instance_id, start_digest, &subject],
    )
}

/// Verifies accepted provenance for this acquired attempt. Every refusal is
/// explicit: no boolean or HMAC-only assertion can prove closure completeness.
pub fn verify_audit_record(
    record: &AuditRecord,
    runtime: &AuditOwnerRuntime,
    storage: &StorageIdentity,
    registration: &Registration,
    attempt: &Attempt,
) -> super::Result<()> {
    if record.version != AUDIT_RECORD_VERSION
        || record.schema != AUDIT_RECORD_SCHEMA
        || record.issuer != AUDIT_RECORD_ISSUER
        || record.owner != runtime.authority.owner
        || record.policy_digest != runtime.authority.policy_digest
    {
        return Err(Refusal::Authentication);
    }
    if record.registration_id != registration.id
        || record.graph_digest != registration.graph_digest
        || record.registration_digest
            != registration_digest(registration).map_err(|_| Refusal::Identity)?
        || record.attempt_digest != attempt_digest(attempt).map_err(|_| Refusal::Identity)?
    {
        return Err(Refusal::Identity);
    }
    if record.closure_receipts != registration.immutable_receipts {
        return Err(Refusal::Membership);
    }
    let expected = audit_record_receipt(
        &runtime.key,
        &runtime.scope,
        &storage.storage_instance_id,
        &runtime.start_digest,
        record,
    )
    .map_err(|_| Refusal::Authentication)?;
    if expected != record.receipt {
        return Err(Refusal::Authentication);
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}
