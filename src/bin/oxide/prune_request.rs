//! Oxide's immutable original-request image. This validates persistence only;
//! it grants neither deletion nor absence and never consults a replacement grant.
use super::{hex32, hex64, text, Intent, IntentPhase, Registry, RootIdentity};
use barge::oxide_outputs::{DelegatedRequest, LiveIntent};
use barge::reference::OxideIntentIdentity;
use oxide_storage::output_request_store::RequestStore;
use serde::{Deserialize, Serialize};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

const VERSION: &str = "oxide.outputs.prune-delegation/1";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Metadata {
    Ready {
        version: String,
        caller_intent_id: String,
        identity: RootIdentity,
        preview_sha256: String,
        grant_generation: u64,
        caller_request_digest: String,
    },
    Unavailable {
        version: String,
        caller_intent_id: String,
        identity: RootIdentity,
        preview_sha256: String,
        reason: String,
    },
}

impl Metadata {
    fn validate(&self, intent: &Intent) -> Result<(), String> {
        let (version, id, identity, preview) = match self {
            Self::Ready {
                version,
                caller_intent_id,
                identity,
                preview_sha256,
                ..
            }
            | Self::Unavailable {
                version,
                caller_intent_id,
                identity,
                preview_sha256,
                ..
            } => (version, caller_intent_id, identity, preview_sha256),
        };
        if version != VERSION
            || id != &intent.id
            || identity != &intent.identity
            || preview != &intent.preview_sha256
        {
            return Err("the immutable request does not match its original intent".to_owned());
        }
        if !matches!(
            intent.phase,
            IntentPhase::Intent | IntentPhase::Deleted | IntentPhase::Resolved
        ) || identity.root_kind != super::RootKind::External
            || identity.ino == 0
            || identity.generation == 0
            || !identity.nonce.as_deref().is_some_and(hex32)
            || !hex64(&identity.marker_digest)
            || !hex64(preview)
        {
            return Err("the delegated original identity or phase is invalid".to_owned());
        }
        let uuid = identity
            .volume_uuid
            .as_deref()
            .ok_or_else(|| "the delegated identity lacks its original volume UUID".to_owned())?;
        barge::os::VolumeUuid::parse(uuid).map_err(|error| error.to_string())?;
        match self {
            Self::Ready {
                grant_generation,
                caller_request_digest,
                ..
            } => {
                if *grant_generation == 0
                    || !hex64(caller_request_digest)
                    || (DelegatedRequest {
                        request_sha256: None,
                        grant_generation: *grant_generation,
                    })
                    .digest(&original_claim(intent))
                    .map_err(|error| error.to_string())?
                        != *caller_request_digest
                {
                    return Err(
                        "the original grant generation/request digest is invalid".to_owned()
                    );
                }
            }
            Self::Unavailable { reason, .. } if !text(reason) => {
                return Err("the original unavailable reason is invalid".to_owned())
            }
            Self::Unavailable { .. } => {}
        }
        Ok(())
    }
}

pub(super) fn bounded_reason(reason: &str) -> String {
    let mut reason = reason.to_owned();
    while reason.len() > 1024 {
        reason.pop();
    }
    if reason.is_empty() {
        reason.push_str("Barge authority unavailable");
    }
    reason
}

pub(super) fn encode(intent: &Intent, authority: &Result<u64, String>) -> Result<Vec<u8>, String> {
    let metadata = match authority {
        Ok(generation) => Metadata::Ready {
            version: VERSION.to_owned(),
            caller_intent_id: intent.id.clone(),
            identity: intent.identity.clone(),
            preview_sha256: intent.preview_sha256.clone(),
            grant_generation: *generation,
            caller_request_digest: DelegatedRequest {
                grant_generation: *generation,
                request_sha256: None,
            }
            .digest(&original_claim(intent))
            .map_err(|error| error.to_string())?,
        },
        Err(reason) => Metadata::Unavailable {
            version: VERSION.to_owned(),
            caller_intent_id: intent.id.clone(),
            identity: intent.identity.clone(),
            preview_sha256: intent.preview_sha256.clone(),
            reason: bounded_reason(reason),
        },
    };
    metadata.validate(intent)?;
    serde_json::to_vec(&metadata).map_err(|error| error.to_string())
}

pub(super) fn load(control: &Path, intent: &Intent, sha: &str) -> Result<Metadata, String> {
    let _authority = super::AuthorityScope::join_or_begin()?;
    let fd = crate::store::open_control_directory(control)?
        .ok_or("the delegated control is absent; keep pending")?;
    let outputs = super::open_directory_at(fd.as_fd(), c"outputs")
        .map_err(|error| format!("cannot open delegated outputs: {error}"))?;
    read(outputs.as_fd(), intent, sha)
}

pub(super) fn original_claim(intent: &Intent) -> LiveIntent {
    let identity = &intent.identity;
    LiveIntent {
        id: intent.id.clone(),
        phase: "intent".to_owned(),
        preview_sha256: intent.preview_sha256.clone(),
        identity: OxideIntentIdentity {
            checkout_id: identity.checkout_id.clone(),
            checkout: identity.checkout.clone(),
            receipt: identity.receipt.clone(),
            member: identity.member.clone(),
            root: identity.root.clone(),
            root_kind: "external".to_owned(),
            volume_uuid: identity.volume_uuid.clone(),
            nonce: identity.nonce.clone(),
            generation: identity.generation,
            dev: identity.dev,
            ino: identity.ino,
            marker_digest: identity.marker_digest.clone(),
        },
    }
}

fn decode(bytes: &[u8], intent: &Intent) -> Result<Metadata, String> {
    let metadata: Metadata = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid immutable request: {error}"))?;
    metadata.validate(intent)?;
    Ok(metadata)
}

pub(super) fn read(
    outputs: BorrowedFd<'_>,
    intent: &Intent,
    sha: &str,
) -> Result<Metadata, String> {
    let store = RequestStore::open(outputs, false).map_err(|error| error.to_string())?;
    let bytes = store
        .read_with(&intent.id, sha, |reader, bytes| {
            super::authority_read(reader, bytes).map(|_| ())
        })
        .map_err(|error| error.to_string())?;
    decode(&bytes, intent)
}

pub(super) fn validate_registry_requests(
    control: &Path,
    registry: &Registry,
) -> Result<(), String> {
    let _authority = super::AuthorityScope::join_or_begin()?;
    let routes = registry
        .deletion_routes
        .as_ref()
        .ok_or_else(|| "the delegated registry lacks explicit routes".to_owned())?;
    if !routes
        .iter()
        .any(|route| matches!(route, super::DeletionRoute::Barge { .. }))
    {
        return Ok(());
    }
    let control_fd = crate::store::open_control_directory(control)?
        .ok_or_else(|| "the delegated control directory is absent; keep pending".to_owned())?;
    let outputs = super::open_directory_at(control_fd.as_fd(), c"outputs")
        .map_err(|error| format!("cannot open delegated outputs: {error}; keep pending"))?;
    for route in routes {
        if let super::DeletionRoute::Barge {
            intent_id,
            request_sha256,
        } = route
        {
            let intent = registry
                .intents
                .iter()
                .find(|intent| &intent.id == intent_id)
                .ok_or_else(|| "the delegated route has no intent".to_owned())?;
            read(outputs.as_fd(), intent, request_sha256).map_err(|error| {
                format!(
                    "intent {intent_id}: {}: {error}; keep pending",
                    control
                        .join("outputs/prune-delegations")
                        .join(format!("{intent_id}.json"))
                        .display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn ready_fixture(intent: &Intent, grant_generation: u64) -> Vec<u8> {
    serde_json::to_vec(&Metadata::Ready {
        version: VERSION.to_owned(),
        caller_intent_id: intent.id.clone(),
        identity: intent.identity.clone(),
        preview_sha256: intent.preview_sha256.clone(),
        grant_generation,
        caller_request_digest: DelegatedRequest {
            request_sha256: None,
            grant_generation,
        }
        .digest(&original_claim(intent))
        .unwrap(),
    })
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_binds_every_original_field_and_survives_only_terminal_phases() {
        let (_fixture, registry) = crate::prune::tests::routed_registry();
        let mut intent = registry.intents[0].clone();
        let bytes = ready_fixture(&intent, 7);
        decode(&bytes, &intent).unwrap();
        for phase in [IntentPhase::Deleted, IntentPhase::Resolved] {
            intent.phase = phase;
            decode(&bytes, &intent).unwrap();
        }
        for phase in [
            IntentPhase::Renamed,
            IntentPhase::Deleting,
            IntentPhase::MarkerLast,
        ] {
            intent.phase = phase;
            assert!(decode(&bytes, &intent).is_err());
        }
        intent.phase = IntentPhase::Intent;
        let wire = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap();
        for (key, value) in [
            ("caller_intent_id", serde_json::json!("e".repeat(32))),
            ("preview_sha256", serde_json::json!("e".repeat(64))),
            ("grant_generation", serde_json::json!(8)),
            ("caller_request_digest", serde_json::json!("e".repeat(64))),
            ("version", serde_json::json!("unknown")),
        ] {
            let mut changed = wire.clone();
            changed[key] = value;
            assert!(
                decode(&serde_json::to_vec(&changed).unwrap(), &intent).is_err(),
                "{key}"
            );
        }
        let mut changed = wire.clone();
        changed["identity"]["ino"] = serde_json::json!(intent.identity.ino + 1);
        assert!(decode(&serde_json::to_vec(&changed).unwrap(), &intent).is_err());
        let duplicate =
            String::from_utf8(bytes)
                .unwrap()
                .replacen("{", "{\"grant_generation\":7,", 1);
        assert!(decode(duplicate.as_bytes(), &intent).is_err());
    }

    #[test]
    fn unavailable_has_no_invented_request_or_generation_and_strict_tags() {
        let (_fixture, registry) = crate::prune::tests::routed_registry();
        let intent = &registry.intents[0];
        let metadata = Metadata::Unavailable {
            version: VERSION.to_owned(),
            caller_intent_id: intent.id.clone(),
            identity: intent.identity.clone(),
            preview_sha256: intent.preview_sha256.clone(),
            reason: "grant unavailable".to_owned(),
        };
        let original = serde_json::to_value(metadata).unwrap();
        decode(&serde_json::to_vec(&original).unwrap(), intent).unwrap();
        for (key, value) in [
            ("grant_generation", serde_json::json!(7)),
            ("caller_request_digest", serde_json::json!("d".repeat(64))),
            ("state", serde_json::json!("unknown")),
            ("reason", serde_json::json!("")),
            ("reason", serde_json::json!(null)),
        ] {
            let mut changed = original.clone();
            changed[key] = value;
            assert!(
                decode(&serde_json::to_vec(&changed).unwrap(), intent).is_err(),
                "{key}"
            );
        }
    }
}
