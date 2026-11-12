//! Covered-root coordination: Barge alone journals progress and owed hooks.
//! Oxide keeps guarded eligibility and an atomic domain settlement fact.
use super::{Candidate, GroupRow, Registry, RootIdentity, RootKind, Store};
use barge::delete::{self, DeleteBounds, OperationReaffirmation};
use barge::operation::{
    self, BeginRequest, DeclarationAuthority, FinalizationContext, FinalizeHook, FinalizeSpec,
    OperationPhase, OperationRecord, OperationUnit, SettlementConsult,
};
use barge::os::{AbsenceVolume, AncestorExpected};
use barge::oxide_outputs;
use barge::state::{LockSet, StatePaths};
use barge::unit::{MarkerFamily, UnitGrant, UnitKind, UnitMarker};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CALLER: &str = "oxide.outputs";
const HOOK: &str = "oxide.outputs.settle";
const HOOK_VERSION: &str = "1";
const PAYLOAD_LIMIT: usize = 2560;
const GROUP_LIMIT: usize = 6;

pub(super) fn bounded_reason(reason: &str) -> String {
    let mut reason = reason.to_owned();
    while reason.len() > 1024 {
        reason.pop();
    }
    if reason.is_empty() {
        reason.push_str("Barge authority is unknown");
    }
    reason
}

/// Historical route recognition exists only to refuse unsafe predecessor
/// recovery/adoption, never to select an old adapter or local fallback.
pub(super) fn route<'a>(registry: &'a Registry, id: &str) -> Option<&'a str> {
    registry
        .deletion_routes
        .as_ref()?
        .iter()
        .find_map(|route| match route {
            super::DeletionRoute::Barge {
                intent_id,
                request_sha256,
            } if intent_id == id => Some(request_sha256.as_str()),
            _ => None,
        })
}
pub(super) fn covered(registry: &Registry, candidate: &Candidate) -> Result<bool, String> {
    if !matches!(
        registry.version.as_str(),
        super::COORDINATED_REGISTRY_VERSION | super::DELEGATED_REGISTRY_VERSION
    ) || candidate.receiptless
        || candidate.root_kind != RootKind::External
        || candidate.nonce.is_none()
    {
        return Ok(false);
    }
    let _authority = super::AuthorityScope::join_or_begin()?;
    let group = super::verified_group(registry, candidate)?;
    if group.iter().any(|row| row.selected) {
        return Ok(false);
    }
    let (marker, digest) = super::outputs::read_marker_image_for_prune(&candidate.root)?;
    if digest != candidate.marker_digest {
        return Err("the marker bytes changed while choosing the deleter".to_owned());
    }
    if marker.is_legacy() {
        return Ok(false);
    }
    if marker.kind() != RootKind::External
        || !marker.is_owned_by(&candidate.checkout, &candidate.checkout_id)
        || marker.nonce() != candidate.nonce.as_deref()
        || marker.volume_uuid() != candidate.volume_uuid.as_deref()
    {
        return Err("the exact /2 marker identity does not match the candidate".to_owned());
    }
    // Historical superseded-generation rows keep their existing local path;
    // they are not the adapter's exact marker-generation class.
    Ok(marker.generation() == candidate.generation)
}

pub(super) fn state_paths() -> Result<StatePaths, String> {
    #[cfg(test)]
    if let Some(paths) = TEST_PATHS.with(|value| value.borrow().clone()) {
        return Ok(paths);
    }
    let os = barge::os::platform();
    // Reuse the existing debug proof guard. Ordinary builds ignore fixture
    // path overrides; the linked Barge proof-account feature has its own
    // independent account binding and remains off in default builds.
    #[cfg(debug_assertions)]
    if let (Some(evidence), Some(path)) = (
        super::proof_evidence(),
        std::env::var_os(barge::state::STATE_DIR_ENV),
    ) {
        let path = std::fs::canonicalize(path)
            .map_err(|error| format!("Barge fixture state cannot be resolved: {error}"))?;
        let root = evidence
            .parent()
            .ok_or("the proof evidence has no guarded parent")?;
        if !path.starts_with(root) || path == root {
            return Err("Barge fixture state escapes the existing proof root".to_owned());
        }
        return StatePaths::resolve(&os, Some(&path)).map_err(|error| error.to_string());
    }
    StatePaths::resolve(&os, None).map_err(|error| error.to_string())
}

/// Read once before publication custody. Re-declaration after this observation
/// is Barge's deciding check; recovery never replaces this generation.
pub(super) fn grant(candidate: &Candidate) -> Result<u64, String> {
    let paths = state_paths()?;
    let uuid = barge::os::VolumeUuid::parse(
        candidate
            .volume_uuid
            .as_deref()
            .ok_or("the root lacks a volume UUID")?,
    )
    .map_err(|error| error.to_string())?;
    let volume =
        barge::area::registered_volume(&paths, &uuid).map_err(|error| error.to_string())?;
    if volume.durability != barge::registry::Durability::Expendable {
        return Err("Barge volume is not registered as expendable".to_owned());
    }
    let area = barge::area::show(
        &paths,
        oxide_outputs::AREA_NAMESPACE,
        oxide_outputs::AREA_NAME,
    )
    .map_err(|error| error.to_string())?
    .ok_or("Barge oxide/outputs is undeclared")?;
    if area.prune != barge::area::PrunePolicy::ReleaseOnReference {
        return Err("Barge oxide/outputs is not release_on_reference".to_owned());
    }
    let folder = barge::area::effective_folder(&volume, &area.declaration())
        .map_err(|error| error.to_string())?;
    if !volume.root_dir.is_empty() || folder != super::outputs::EXTERNAL_DIR {
        return Err("Barge declaration does not cover this direct oxide-outputs area".to_owned());
    }
    let grant = area
        .outputs_grant
        .as_ref()
        .ok_or("Barge oxide/outputs grant is absent or withdrawn")?;
    grant.validate().map_err(|error| error.to_string())?;
    if area.generation == 0 {
        return Err("Barge grant generation is invalid".to_owned());
    }
    Ok(area.generation)
}

/// Verify the caller's registry snapshot and re-read the actual receipt.
/// `marked_ms` distinguishes the two existing domain settlement revisions.
/// Recorded device numbers are excluded: reconnects may change that observation.
/// This acquires no lock and grants no filesystem-effect authority: callers
/// retain their synchronization, clean barrier/binding and runtime checks.
/// A guarded publication must supply its freshly read revision; this result
/// cannot be cached across that guard's release or a registry revision.
pub(super) fn verify_domain_group<'a>(
    control: &Path,
    registry: &'a Registry,
    identity: &super::RootIdentity,
    group: &[super::GroupRow],
    marked_ms: Option<u64>,
    prior_receipt_sha: &str,
) -> Result<&'a super::Row, String> {
    if !registry.complete {
        return Err("the registry is incomplete".to_owned());
    }
    let rows: Vec<_> = registry
        .rows
        .iter()
        .filter(|row| row.checkout_id == identity.checkout_id && row.root == identity.root)
        .collect();
    if rows.len() != group.len() || rows.is_empty() {
        return Err("the blocking registry group changed".to_owned());
    }
    let mut members: Vec<_> = rows.iter().map(|row| row.member.as_str()).collect();
    members.sort_unstable();
    let mut expected: Vec<_> = group.iter().map(|row| row.member.as_str()).collect();
    expected.sort_unstable();
    if members != expected || expected.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("the blocking registry members changed or repeat".to_owned());
    }
    for old in group {
        let row = rows
            .iter()
            .find(|row| row.member == old.member)
            .ok_or("a group member disappeared")?;
        if row.selected
            || row.pending
            || row.reservation.is_some()
            || row.cancelling.is_some()
            || row.pruning.is_some()
            || row.pruned_ms != marked_ms
            || row.nonce != old.nonce
            || row.generation != old.generation
            || row.root_ino != old.root_ino
            || row.marker_digest != old.marker_digest
            || row.checkout != identity.checkout
            || row.receipt != identity.receipt
            || row.root_kind != identity.root_kind
            || row.volume_uuid != identity.volume_uuid
        {
            return Err("a blocking group member changed or became selected".to_owned());
        }
    }
    if registry.rows.iter().any(|row| {
        row.root == identity.root && row.checkout_id != identity.checkout_id && row.live()
    }) {
        return Err("another checkout references this root".to_owned());
    }
    if super::receipt_preimage(control, &identity.checkout_id)? != prior_receipt_sha {
        return Err("the original receipt changed before Barge settled".to_owned());
    }
    let receipt =
        super::outputs::read_receipt_for_prune(control, &identity.checkout, &identity.checkout_id)?
            .ok_or("the checkout receipt disappeared")?;
    if super::outputs::receipt_is_pruning(&receipt)
        || super::current_selection_references(&receipt, &identity.root)
    {
        return Err("the receipt is pruning or currently places this root".to_owned());
    }
    super::verify_group_receipt(
        control,
        &identity.checkout,
        &identity.checkout_id,
        &identity.root,
        group,
    )?;
    Ok(rows[0])
}

/// Original, versioned domain facts. No progress or replaceable authority is
/// stored here; this payload lives only in Barge's bounded operation record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    identity: RootIdentity,
    group: Vec<GroupRow>,
    checkout_volume_uuid: Option<String>,
    preview_sha256: String,
    prior_receipt_sha: String,
    registry_revision: u64,
    grant_generation: u64,
    key: String,
    terminal_ms: u64,
}

impl Payload {
    fn encode(&self) -> Result<String, String> {
        self.validate()?;
        let bytes = serde_json::to_string(self).map_err(|e| e.to_string())?;
        if bytes.len() > PAYLOAD_LIMIT {
            return Err("the complete original group exceeds Barge's 2.5KiB hook payload; kept before begin".to_owned());
        }
        Ok(bytes)
    }

    fn validate(&self) -> Result<(), String> {
        if self.group.is_empty()
            || self.group.len() > GROUP_LIMIT
            || !super::hex32(&self.key)
            || !super::hex64(&self.preview_sha256)
            || !super::hex64(&self.prior_receipt_sha)
            || self.registry_revision == 0
            || self.grant_generation == 0
            || self.terminal_ms == 0
            || self.identity.root_kind != RootKind::External
            || !super::hex64(&self.identity.checkout_id)
            || !self.identity.nonce.as_deref().is_some_and(super::hex32)
            || !super::hex64(&self.identity.marker_digest)
            || self.identity.ino == 0
            || self.identity.generation == 0
            || !self.identity.checkout.is_absolute()
            || !self.identity.root.is_absolute()
            || !self.identity.receipt.is_absolute()
        {
            return Err("the original operation payload is incomplete or unsupported".to_owned());
        }
        let mut members = std::collections::BTreeSet::new();
        for member in &self.group {
            if !super::text(&member.member)
                || !members.insert(&member.member)
                || member.selected
                || member.generation == 0
                || member.root_ino.is_none()
                || member
                    .marker_digest
                    .as_deref()
                    .is_none_or(|sha| !super::hex64(sha))
            {
                return Err(
                    "the original operation group is partial, selected or repeated".to_owned(),
                );
            }
        }
        Ok(())
    }

    fn from_record(record: &OperationRecord) -> Result<Self, String> {
        let decoded: Result<Self, String> = (|| {
            record.validate().map_err(|e| e.to_string())?;
            let spec = record
                .finalize
                .as_ref()
                .ok_or("the operation has no hook")?;
            if record.caller.as_deref() != Some(CALLER)
                || spec.kind != HOOK
                || spec.version != HOOK_VERSION
                || spec.payload.len() > PAYLOAD_LIMIT
                || record.owned_plan.is_some()
            {
                return Err(
                    "the delegated operation needs an unsupported caller/hook/version".to_owned(),
                );
            }
            let payload: Self = serde_json::from_str(&spec.payload).map_err(|e| e.to_string())?;
            payload.validate()?;
            if payload.encode()? != spec.payload {
                return Err("the original hook payload is not canonical".to_owned());
            }
            let identity = &payload.identity;
            let absence = record
                .absence
                .as_ref()
                .ok_or("the original checkout absence is missing")?;
            absence.validate().map_err(|e| e.to_string())?;
            if absence.verdict != "absent" || absence.path != identity.checkout {
                return Err("the original checkout absence does not bind this payload".to_owned());
            }
            if absence.ancestor.as_ref().is_some_and(|ancestor| {
                ancestor.volume.kind == "registered"
                    && payload.checkout_volume_uuid.as_deref()
                        != Some(ancestor.volume.volume_uuid.as_str())
            }) {
                return Err("the original registered checkout volume differs".to_owned());
            }
            let uuid = identity
                .volume_uuid
                .as_deref()
                .ok_or("the original UUID is missing")?;
            let declaration = record
                .declaration
                .as_ref()
                .ok_or("original declaration authority is missing")?;
            if record.key.as_deref() != Some(payload.key.as_str())
                || record.request_digest.as_deref()
                    != Some(super::sha256_hex(spec.payload.as_bytes()).as_str())
                || record.unit.volume_uuid.as_str() != uuid
                || record.unit.inode != identity.ino
                || record.unit.kind != UnitKind::Directory
                || record.unit.root_id.is_some()
                || !record.unit.root_dir.is_empty()
                || record.unit.folder != super::outputs::EXTERNAL_DIR
                || !record.unit.parent.is_empty()
                || identity.root
                    != record
                        .unit
                        .mount_hint
                        .join(&record.unit.folder)
                        .join(&record.unit.name)
                || record.unit.marker.as_ref()
                    != Some(&UnitMarker::new(
                        MarkerFamily::OxideOutputs,
                        &identity.marker_digest,
                    ))
                || record.unit.grant != UnitGrant::delegation(MarkerFamily::OxideOutputs)
                || declaration.namespace != oxide_outputs::AREA_NAMESPACE
                || declaration.area != oxide_outputs::AREA_NAME
                || declaration.generation != payload.grant_generation
            {
                return Err(
                    "the operation's original domain/unit/authority binding differs".to_owned(),
                );
            }
            Ok(payload)
        })();
        decoded.map_err(|reason| {
            format!(
                "operation {} (sequence {}) for recorded root {}: {reason}",
                record.key.as_deref().unwrap_or("<unbound>"),
                record.sequence,
                record
                    .unit
                    .mount_hint
                    .join(&record.unit.root_dir)
                    .join(&record.unit.folder)
                    .join(&record.unit.parent)
                    .join(&record.unit.name)
                    .display()
            )
        })
    }

    fn settlement(&self) -> Result<super::Settlement, String> {
        Ok(super::Settlement {
            caller: CALLER.to_owned(),
            key: self.key.clone(),
            checkout_id: self.identity.checkout_id.clone(),
            root: self.identity.root.clone(),
            nonce: self.identity.nonce.clone(),
            outcome: "deleted".to_owned(),
            terminal_ms: self.terminal_ms,
            request_sha256: super::sha256_hex(self.encode()?.as_bytes()),
        })
    }
}

fn request_from_record(record: &OperationRecord) -> Result<BeginRequest, String> {
    Payload::from_record(record)?;
    let mut unit = record.unit.clone();
    unit.capture_name = None;
    let mut request = BeginRequest::delegated(
        CALLER,
        record.key.as_ref().ok_or("operation key missing")?,
        record
            .request_digest
            .as_ref()
            .ok_or("operation digest missing")?,
        record.finalize.clone().ok_or("operation hook missing")?,
        unit,
    );
    request.absence = record.absence.clone();
    request.declaration = record.declaration.clone();
    request.validate().map_err(|e| e.to_string())?;
    Ok(request)
}

fn view() -> Result<operation::JournalView, String> {
    let paths = state_paths()?;
    let os = barge::os::platform();
    let mut locks = LockSet::new(paths.clone());
    operation::consult_operations(&os, &os, &paths, &mut locks, barge::contract::INTENT_ROWS)
        .map_err(|e| format!("delegated operation consult is unknown: {e}"))?
        .ok_or("the Barge operation journal is uninitialized; publication refuses".to_owned())
}

fn conflicts(
    records: &[OperationRecord],
    pairs: &[(String, PathBuf, bool)],
    compacting: bool,
) -> Result<(), String> {
    for record in records.iter().filter(|record| record.is_pending()) {
        if compacting {
            return Err(format!(
                "compaction refuses while delegated operation {} is nonterminal",
                record.sequence
            ));
        }
        // A caller we cannot project cannot be treated as an empty conflict set.
        let payload = Payload::from_record(record)?;
        if pairs.iter().any(|(checkout, root, live)| {
            (checkout == &payload.identity.checkout_id && root == &payload.identity.root)
                || (*live && root == &payload.identity.root)
        }) {
            return Err(format!(
                "operation {} for {} excludes this publication",
                payload.key,
                payload.identity.root.display()
            ));
        }
    }
    Ok(())
}

/// The common registry-publication gate covers adopt, bind, reserve, local
/// intent, both settlement revisions and compaction inside their own guards.
pub(super) fn consult_publication(
    control: &Path,
    previous: &Registry,
    next: &Registry,
    context: Option<(&StatePaths, &FinalizationContext)>,
    receipt_checkout: Option<&str>,
) -> Result<(), String> {
    if previous.version != super::COORDINATED_REGISTRY_VERSION {
        return Ok(());
    }
    if next.version != super::COORDINATED_REGISTRY_VERSION {
        return Err("an adopted registry cannot downgrade".to_owned());
    }
    let old = previous
        .rows
        .iter()
        .map(|row| {
            (
                (
                    row.checkout_id.as_str(),
                    row.root.as_path(),
                    row.member.as_str(),
                ),
                row,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let new = next
        .rows
        .iter()
        .map(|row| {
            (
                (
                    row.checkout_id.as_str(),
                    row.root.as_path(),
                    row.member.as_str(),
                ),
                row,
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    if old.len() != previous.rows.len() || new.len() != next.rows.len() {
        return Err("publication refuses duplicate registry members".to_owned());
    }
    let retained = previous
        .settled
        .as_ref()
        .ok_or("prior settlement set missing")?;
    let proposed = next
        .settled
        .as_ref()
        .ok_or("proposed settlement set missing")?;
    if retained.iter().any(|entry| !proposed.contains(entry)) {
        return Err("retained settlement facts are never evicted or rewritten".to_owned());
    }
    if context.is_none() && proposed != retained {
        return Err("only a checked finalization context may append a settlement fact".to_owned());
    }
    let mut pairs = Vec::new();
    for row in previous.rows.iter().chain(&next.rows) {
        let key = (
            row.checkout_id.as_str(),
            row.root.as_path(),
            row.member.as_str(),
        );
        if old.get(&key).copied() != new.get(&key).copied() {
            pairs.push((
                row.checkout_id.clone(),
                row.root.clone(),
                context.is_some() || row.live(),
            ));
        }
    }
    let compacting = next.intents.len() < previous.intents.len();
    if let Some((paths, context)) = context {
        let os = barge::os::platform();
        match operation::consult_settlement(&os, &os, paths, context).map_err(|e| e.to_string())? {
            SettlementConsult::Owed(others) => {
                publication_conflicts(&others, &pairs, compacting, receipt_checkout)
            }
            SettlementConsult::AlreadyComplete => Err(
                "the operation was already acknowledged; no publication is authorized".to_owned(),
            ),
        }
    } else {
        let _ = control;
        publication_conflicts(&view()?.records, &pairs, compacting, receipt_checkout)
    }
}

pub(super) fn unsettled(checkout_id: &str) -> Result<Option<String>, String> {
    for record in view()?.records.iter().filter(|record| record.is_pending()) {
        let payload = Payload::from_record(record)?;
        if payload.identity.checkout_id == checkout_id {
            return Ok(Some(payload.key));
        }
    }
    Ok(None)
}

fn verify_payload_domain(
    control: &Path,
    registry: &Registry,
    payload: &Payload,
    marked_ms: Option<u64>,
) -> Result<(), String> {
    verify_domain_group(
        control,
        registry,
        &payload.identity,
        &payload.group,
        marked_ms,
        &payload.prior_receipt_sha,
    )?;
    if registry
        .rows
        .iter()
        .filter(|row| {
            row.checkout_id == payload.identity.checkout_id && row.root == payload.identity.root
        })
        .any(|row| row.checkout_volume_uuid != payload.checkout_volume_uuid)
    {
        return Err("the group's original checkout volume changed".to_owned());
    }
    Ok(())
}

struct Reaffirm<'a> {
    store: &'a Store,
    control: &'a Path,
}
impl OperationReaffirmation for Reaffirm<'_> {
    fn reaffirm(&self, record: &OperationRecord) -> barge::error::Result<()> {
        self.reaffirm_with_root(record, |payload| {
            let session = super::RootSession::open_recorded(&payload.identity)?;
            session.verify_marker(&candidate(payload))?;
            Ok(session)
        })
    }
}

impl Reaffirm<'_> {
    /// Only the volume/root observation is an external seam. Admission,
    /// domain, custody, absence and budget checks compose identically.
    fn reaffirm_with_root<T>(
        &self,
        record: &OperationRecord,
        verify_root: impl FnOnce(&Payload) -> Result<T, String>,
    ) -> barge::error::Result<()> {
        let result = (|| {
            let payload = Payload::from_record(record)?;
            let guard = crate::store::publication_guard(self.control)?;
            let registry = super::ready_registry(self.control)?;
            verify_payload_domain(self.control, &registry, &payload, None)?;
            // Keep the verified root descriptors alive across custody and
            // absence, exactly as in the production callback before this seam.
            let _root = verify_root(&payload)?;
            let candidate = candidate(&payload);
            let mut authority = super::AuthorityScope::begin()?;
            super::custody_clear_under_barrier(
                &mut authority,
                self.control,
                &payload.identity.checkout,
                Some(&guard),
            )?;
            match super::checkout_absence(&super::candidate_row(&candidate), false)? {
                super::Absence::Absent(_) if authority.complete() => Ok(()),
                _ => Err(
                    "the original checkout is present or its writer/absence proof is incomplete"
                        .to_owned(),
                ),
            }
        })();
        let _ = self.store; // Caller holds this store's real clean barrier/binding.
        result.map_err(domain_error)
    }
}

fn domain_error(reason: String) -> barge::error::Error {
    barge::error::Error::new(barge::error::ErrorKind::UnknownState, reason)
}

fn candidate(payload: &Payload) -> Candidate {
    let identity = &payload.identity;
    Candidate {
        checkout_id: identity.checkout_id.clone(),
        checkout: identity.checkout.clone(),
        receipt: identity.receipt.clone(),
        member: identity.member.clone(),
        root: identity.root.clone(),
        root_kind: identity.root_kind,
        volume_uuid: identity.volume_uuid.clone(),
        checkout_volume_uuid: payload.checkout_volume_uuid.clone(),
        nonce: identity.nonce.clone(),
        generation: identity.generation,
        dev: identity.dev,
        ino: identity.ino,
        marker_digest: identity.marker_digest.clone(),
        logical_bytes: 0,
        allocated_bytes: 0,
        accounting_complete: true,
        receiptless: false,
        deleter: Some("barge".to_owned()),
        barge_unavailable: None,
    }
}

/// Called with the actual clean barrier/binding held, never admission across
/// begin, execution, cancellation or a Barge lock wait.
pub(super) fn fresh(
    store: &Store,
    control: &Path,
    candidate: &Candidate,
    preview_sha256: &str,
    list: Option<&super::OperatorList>,
    evidence: &super::SnapshotEvidence,
) -> Result<bool, String> {
    let generation = grant(candidate)?;
    let payload = {
        let guard = crate::store::publication_guard(control)?;
        let revision = super::recheck_candidate(
            store,
            control,
            candidate,
            list,
            evidence,
            Some(&guard),
            super::observe_row_root,
        )?;
        super::confirm_absence(candidate, evidence)?;
        let registry = super::ready_registry(control)?;
        if registry.version != super::COORDINATED_REGISTRY_VERSION
            || registry.revision != revision
            || !covered(&registry, candidate)?
        {
            return Err("the complete guarded verdict changed before begin".to_owned());
        }
        if registry
            .settled
            .as_ref()
            .is_none_or(|entries| entries.len() >= super::MAX_INTENTS)
        {
            return Err(
                "the retained settlement set is full or unavailable; refuse before begin"
                    .to_owned(),
            );
        }
        let group = super::verified_group(&registry, candidate)?
            .into_iter()
            .map(|mut member| {
                member.root_dev = None;
                member
            })
            .collect();
        let identity = RootIdentity {
            checkout_id: candidate.checkout_id.clone(),
            checkout: candidate.checkout.clone(),
            receipt: candidate.receipt.clone(),
            member: candidate.member.clone(),
            root: candidate.root.clone(),
            root_kind: candidate.root_kind,
            volume_uuid: candidate.volume_uuid.clone(),
            nonce: candidate.nonce.clone(),
            generation: candidate.generation,
            dev: candidate.dev,
            ino: candidate.ino,
            marker_digest: candidate.marker_digest.clone(),
        };
        Payload {
            identity,
            group,
            checkout_volume_uuid: candidate.checkout_volume_uuid.clone(),
            preview_sha256: preview_sha256.to_owned(),
            prior_receipt_sha: super::receipt_preimage(control, &candidate.checkout_id)?,
            registry_revision: revision,
            grant_generation: generation,
            key: super::fresh_nonce()?,
            terminal_ms: crate::process::now_ms(),
        }
    };
    operation_point(control, &payload, "verdict")?;
    let encoded = payload.encode()?;
    let os = barge::os::platform();
    let uuid = barge::os::VolumeUuid::parse(
        candidate
            .volume_uuid
            .as_deref()
            .ok_or("the volume UUID is missing")?,
    )
    .map_err(|e| e.to_string())?;
    let mount = candidate
        .root
        .parent()
        .and_then(Path::parent)
        .ok_or("the root has no area mount")?;
    let unit = OperationUnit {
        volume_uuid: uuid,
        mount_hint: mount.to_path_buf(),
        root_dir: String::new(),
        folder: super::outputs::EXTERNAL_DIR.to_owned(),
        parent: String::new(),
        name: candidate
            .root
            .file_name()
            .ok_or("the root has no leaf")?
            .to_str()
            .ok_or("non-UTF8 root leaf")?
            .to_owned(),
        kind: UnitKind::Directory,
        device: candidate.dev,
        inode: candidate.ino,
        marker: Some(UnitMarker::new(
            MarkerFamily::OxideOutputs,
            &candidate.marker_digest,
        )),
        root_id: None,
        capture_name: None,
        grant: UnitGrant::delegation(MarkerFamily::OxideOutputs),
    };
    let mut request = BeginRequest::delegated(
        CALLER,
        &payload.key,
        super::sha256_hex(encoded.as_bytes()),
        FinalizeSpec {
            kind: HOOK.to_owned(),
            version: HOOK_VERSION.to_owned(),
            payload: encoded,
        },
        unit,
    );
    let prefix = os.volume_prefix(&candidate.checkout);
    let checkout_uuid = candidate
        .checkout_volume_uuid
        .as_deref()
        .map(barge::os::VolumeUuid::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    let expected = match prefix.as_ref() {
        Some(prefix) => AncestorExpected::Registered {
            uuid: checkout_uuid
                .as_ref()
                .ok_or("the registered checkout UUID is missing")?,
            recorded_mount: prefix,
        },
        None => AncestorExpected::BootData,
    };
    let proof = barge::os::prove_absence(&os, &os, &candidate.checkout, expected)
        .map_err(|e| e.to_string())?;
    if proof.verdict() != "absent" {
        return Err("the checkout is not positively absent".to_owned());
    }
    request.absence = Some(
        serde_json::from_value(serde_json::to_value(proof).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?,
    );
    request.declaration = Some(DeclarationAuthority {
        namespace: oxide_outputs::AREA_NAMESPACE.to_owned(),
        area: oxide_outputs::AREA_NAME.to_owned(),
        generation,
        grant: request.unit.grant.clone(),
    });
    let paths = state_paths()?;
    delete::begin_checked_delegated(&paths, &os, &os, &os, request.clone(), DeleteBounds::default())
        .map_err(|e| format!("operation {} for {} has an uncertain/refused begin: {e}; inspect the Barge operation journal, never locally delete", payload.key, candidate.root.display()))?;
    operation_point(control, &payload, "intent")?;
    arm_operation_fault(&payload);
    match delete::execute_checked_delegated(
        &paths,
        &os,
        &os,
        &os,
        &request,
        &Reaffirm { store, control },
        DeleteBounds::default(),
    ) {
        Ok(report) if report.is_deleted() => {
            operation_point(control, &payload, "deleted")?;
            Ok(true)
        }
        result => {
            // Cancellation is retained only if Barge positively proves no
            // capture/effect under the original request; never infer it from Locked.
            let cancelled = delete::cancel_delegated_operation(&paths, &os, &os, &os, &request);
            Err(format!("operation {} for {} remains pending/refused: {result:?}; cancellation: {cancelled:?}", payload.key, candidate.root.display()))
        }
    }
}

struct Hook<'a> {
    store: &'a Store,
    control: &'a Path,
}
impl FinalizeHook for Hook<'_> {
    fn kind(&self) -> &str {
        HOOK
    }
    fn version(&self) -> &str {
        HOOK_VERSION
    }
    fn finalize(
        &self,
        context: &FinalizationContext,
        record: &OperationRecord,
    ) -> barge::error::Result<()> {
        let result = (|| {
            let payload = Payload::from_record(record)?;
            let paths = state_paths()?;
            super::under_checkout_barrier(self.store, &payload.identity.checkout, || {
                settle_domain(self.control, &paths, context, &payload)
            })?;
            if operation_fault(&payload, "lost-ack") {
                return Err("injected lost acknowledgement after atomic settlement".to_owned());
            }
            Ok(())
        })();
        result.map_err(domain_error)
    }
}

fn settle_domain(
    control: &Path,
    paths: &StatePaths,
    context: &FinalizationContext,
    payload: &Payload,
) -> Result<(), String> {
    let expected = payload.settlement()?;
    for marking in [true, false] {
        let _guard = crate::store::publication_guard(control)?;
        let mut registry = super::ready_registry(control)?;
        if !registry.complete {
            return Err("an incomplete registry cannot acknowledge finalization".to_owned());
        }
        let settled = registry
            .settled
            .as_ref()
            .ok_or("the settlement format is not adopted")?;
        let present = registry.rows.iter().any(|row| {
            row.checkout_id == payload.identity.checkout_id && row.root == payload.identity.root
        });
        if settled
            .iter()
            .any(|entry| entry.caller == expected.caller && entry.key == expected.key)
        {
            if settled.iter().any(|entry| entry == &expected) && !present {
                if registry
                    .rows
                    .iter()
                    .any(|row| row.root == payload.identity.root && row.live())
                {
                    return Err("a live reference appeared after domain settlement".to_owned());
                }
                let os = barge::os::platform();
                match operation::consult_settlement(&os, &os, paths, context)
                    .map_err(|e| e.to_string())?
                {
                    SettlementConsult::Owed(others) => conflicts(
                        &others,
                        &[(
                            payload.identity.checkout_id.clone(),
                            payload.identity.root.clone(),
                            true,
                        )],
                        false,
                    )?,
                    SettlementConsult::AlreadyComplete => {}
                }
                return Ok(());
            }
            return Err("the settlement fact or recreated group differs; keep owed".to_owned());
        }
        if settled.len() >= super::MAX_INTENTS {
            return Err("the retained settlement set is full; never evicted".to_owned());
        }
        let stamp = if marking {
            None
        } else {
            Some(payload.terminal_ms)
        };
        // A concurrent/restarted handler may encounter the original marking.
        let already_marked = marking
            && registry
                .rows
                .iter()
                .filter(|row| {
                    row.checkout_id == payload.identity.checkout_id
                        && row.root == payload.identity.root
                })
                .all(|row| row.pruned_ms == Some(payload.terminal_ms));
        verify_payload_domain(
            control,
            &registry,
            payload,
            if already_marked {
                Some(payload.terminal_ms)
            } else {
                stamp
            },
        )?;
        if marking && already_marked {
            continue;
        }
        if marking {
            for row in registry.rows.iter_mut().filter(|row| {
                row.checkout_id == payload.identity.checkout_id && row.root == payload.identity.root
            }) {
                row.pruned_ms = Some(payload.terminal_ms);
            }
        } else {
            registry.rows.retain(|row| {
                !(row.checkout_id == payload.identity.checkout_id
                    && row.root == payload.identity.root)
            });
            registry
                .settled
                .as_mut()
                .ok_or("the settlement set disappeared")?
                .push(expected.clone());
        }
        registry.bump()?;
        super::write_registry_with_context(control, &registry, Some((paths, context)))?;
        #[cfg(test)]
        if marking && STOP_AFTER_DOMAIN_MARK.with(|stop| stop.replace(false)) {
            return Err("injected interruption after actual guarded domain marking".to_owned());
        }
        operation_point(control, payload, if marking { "marked" } else { "settled" })?;
    }
    Ok(())
}

/// Drain one complete checked observation. Each effect gets its own real
/// checkout barrier/binding; hook dispatch independently reacquires it.
pub(super) fn drain(
    store: &Store,
    control: &Path,
    only_key: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let records = view()?.records;
    let mut completed = Vec::new();
    for record in records
        .iter()
        .filter(|record| record.is_pending() && record.caller.as_deref() == Some(CALLER))
    {
        let payload = Payload::from_record(record)?;
        if only_key.is_some_and(|key| key != payload.key) {
            continue;
        }
        let request = request_from_record(record)?;
        let paths = state_paths()?;
        let os = barge::os::platform();
        let deleted = match record.phase {
            OperationPhase::Intent => {
                super::under_checkout_barrier(store, &payload.identity.checkout, || {
                    match delete::execute_checked_delegated(&paths, &os, &os, &os, &request, &Reaffirm { store, control }, DeleteBounds::default()) {
                    Ok(report) if report.is_deleted() => Ok(true),
                    result => match delete::cancel_delegated_operation(&paths, &os, &os, &os, &request) {
                        Ok(cancelled) if cancelled.phase == OperationPhase::Cancelled => Ok(false),
                        cancellation => Err(format!("operation {} for {} remains pending: {result:?}; cancellation: {cancellation:?}", payload.key, payload.identity.root.display())),
                    },
                }
                })?
            }
            OperationPhase::Deleted => true,
            _ => super::under_checkout_barrier(store, &payload.identity.checkout, || {
                match delete::replay_checked_operation(
                    &paths,
                    &os,
                    &os,
                    &os,
                    &request,
                    DeleteBounds::default(),
                )
                .map_err(|e| e.to_string())?
                {
                    delete::OperationReplay::Deleted => Ok(true),
                    delete::OperationReplay::Walked(report) if report.is_deleted() => Ok(true),
                    other => Err(format!(
                        "operation {} for {} remains pending: {other:?}",
                        payload.key,
                        payload.identity.root.display()
                    )),
                }
            })?,
        };
        if !deleted {
            continue;
        }
        operation::finalize_operation(
            &os,
            &os,
            &paths,
            CALLER,
            &payload.key,
            &[&Hook { store, control }],
        )
        .map_err(|e| e.to_string())?;
        completed.push(payload.identity.root);
    }
    Ok(completed)
}

pub(super) fn initialize_fixture_journal() -> Result<(), String> {
    #[cfg(debug_assertions)]
    if super::proof_evidence().is_some() {
        let paths = state_paths()?;
        let os = barge::os::platform();
        let mut locks = LockSet::new(paths.clone());
        operation::init_operation_journal(&os, &os, &paths, &mut locks)
            .map_err(|e| e.to_string())?;
        if view()?.records.iter().any(|record| record.is_pending()) {
            return Err("drain all existing operations before one-time adoption".to_owned());
        }
        return Ok(());
    }
    Err(
        "single-authority adoption is activated only on current guarded disposable fixtures"
            .to_owned(),
    )
}

/// Receipt bytes can change without a row difference, so their complete
/// checkout scope participates in this same single fail-fast observation.
fn publication_conflicts(
    records: &[OperationRecord],
    pairs: &[(String, PathBuf, bool)],
    compacting: bool,
    receipt_checkout: Option<&str>,
) -> Result<(), String> {
    if let Some(checkout) = receipt_checkout {
        for record in records.iter().filter(|record| record.is_pending()) {
            let payload = Payload::from_record(record)?;
            if payload.identity.checkout_id == checkout {
                return Err(format!(
                    "operation {} for {} excludes receipt publication",
                    payload.key,
                    payload.identity.root.display()
                ));
            }
        }
    }
    conflicts(records, pairs, compacting)
}

#[cfg(test)]
thread_local! {
    static TEST_PATHS: std::cell::RefCell<Option<StatePaths>> = const { std::cell::RefCell::new(None) };
    static STOP_AFTER_DOMAIN_MARK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Only the external state-path input is substituted. Real private journal
/// ownership, locking, publication and dispatch still decide every result.
#[cfg(test)]
pub(super) fn with_test_journal<T>(root: &Path, work: impl FnOnce(&StatePaths) -> T) -> T {
    let os = barge::os::platform();
    let paths = StatePaths::resolve(&os, Some(&root.join("barge-state"))).unwrap();
    paths.initialize().unwrap();
    let mut locks = LockSet::new(paths.clone());
    operation::init_operation_journal(&os, &os, &paths, &mut locks).unwrap();
    struct Restore(Option<StatePaths>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_PATHS.with(|value| *value.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(TEST_PATHS.with(|value| value.replace(Some(paths.clone()))));
    work(&paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Domain tests provide a labelled terminal-record premise, not a
    /// successful effect/custody observation. The real journal reader,
    /// finalization dispatcher/context and registry transaction all run.
    struct DomainHook<'a> {
        control: &'a Path,
        paths: &'a StatePaths,
        lost_ack: std::cell::Cell<bool>,
    }
    impl FinalizeHook for DomainHook<'_> {
        fn kind(&self) -> &str {
            HOOK
        }
        fn version(&self) -> &str {
            HOOK_VERSION
        }
        fn finalize(
            &self,
            context: &FinalizationContext,
            record: &OperationRecord,
        ) -> barge::error::Result<()> {
            let payload = Payload::from_record(record).map_err(domain_error)?;
            settle_domain(self.control, self.paths, context, &payload).map_err(domain_error)?;
            if self.lost_ack.replace(false) {
                return Err(domain_error("injected lost acknowledgement".to_owned()));
            }
            Ok(())
        }
    }

    fn payload(control: &Path, candidate: &Candidate) -> Payload {
        let registry = super::super::ready_registry(control).unwrap();
        Payload {
            identity: RootIdentity {
                checkout_id: candidate.checkout_id.clone(),
                checkout: candidate.checkout.clone(),
                receipt: candidate.receipt.clone(),
                member: candidate.member.clone(),
                root: candidate.root.clone(),
                root_kind: candidate.root_kind,
                volume_uuid: candidate.volume_uuid.clone(),
                nonce: candidate.nonce.clone(),
                generation: candidate.generation,
                dev: candidate.dev,
                ino: candidate.ino,
                marker_digest: candidate.marker_digest.clone(),
            },
            group: super::super::verified_group(&registry, candidate)
                .unwrap()
                .into_iter()
                .map(|mut row| {
                    row.root_dev = None;
                    row
                })
                .collect(),
            checkout_volume_uuid: candidate.checkout_volume_uuid.clone(),
            key: "d".repeat(32),
            preview_sha256: "e".repeat(64),
            prior_receipt_sha: super::super::receipt_preimage(control, &candidate.checkout_id)
                .unwrap(),
            registry_revision: registry.revision,
            grant_generation: 7,
            terminal_ms: 123,
        }
    }

    fn begin(paths: &StatePaths, payload: &Payload) -> OperationRecord {
        let bytes = payload.encode().unwrap();
        let identity = &payload.identity;
        let unit = OperationUnit {
            volume_uuid: barge::os::VolumeUuid::parse(identity.volume_uuid.as_deref().unwrap())
                .unwrap(),
            mount_hint: identity
                .root
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_path_buf(),
            root_dir: String::new(),
            folder: super::super::outputs::EXTERNAL_DIR.to_owned(),
            parent: String::new(),
            name: identity
                .root
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            kind: UnitKind::Directory,
            device: identity.dev,
            inode: identity.ino,
            marker: Some(UnitMarker::new(
                MarkerFamily::OxideOutputs,
                &identity.marker_digest,
            )),
            root_id: None,
            capture_name: None,
            grant: UnitGrant::delegation(MarkerFamily::OxideOutputs),
        };
        let mut request = BeginRequest::delegated(
            CALLER,
            &payload.key,
            super::super::sha256_hex(bytes.as_bytes()),
            FinalizeSpec {
                kind: HOOK.to_owned(),
                version: HOOK_VERSION.to_owned(),
                payload: bytes,
            },
            unit,
        );
        request.declaration = Some(DeclarationAuthority {
            namespace: "oxide".to_owned(),
            area: "outputs".to_owned(),
            generation: 7,
            grant: request.unit.grant.clone(),
        });
        let os = barge::os::platform();
        let prefix = os.volume_prefix(&identity.checkout).unwrap();
        let uuid =
            barge::os::VolumeUuid::parse(payload.checkout_volume_uuid.as_deref().unwrap()).unwrap();
        let proof = barge::os::prove_absence(
            &os,
            &os,
            &identity.checkout,
            AncestorExpected::Registered {
                uuid: &uuid,
                recorded_mount: &prefix,
            },
        )
        .unwrap();
        assert_eq!(proof.verdict(), "absent", "{:?}", proof.reason());
        request.absence =
            Some(serde_json::from_value(serde_json::to_value(proof).unwrap()).unwrap());
        let mut locks = LockSet::new(paths.clone());
        operation::begin_operation(&os, &os, paths, &mut locks, request)
            .unwrap()
            .record()
            .clone()
    }

    fn terminal_premise(paths: &StatePaths, mut record: OperationRecord) -> OperationRecord {
        record.phase = OperationPhase::Deleted;
        record.generation += 3;
        record.validate().unwrap();
        // This is an explicitly crafted journal fixture, never evidence of
        // unlink, custody or positive removed publication.
        let file = paths
            .operations_dir()
            .join(format!("{}.json", record.sequence));
        fs::write(&file, serde_json::to_vec(&record).unwrap()).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        record
    }

    fn dispatch(
        paths: &StatePaths,
        hook: &dyn FinalizeHook,
        key: &str,
    ) -> barge::error::Result<OperationRecord> {
        let os = barge::os::platform();
        operation::finalize_operation(&os, &os, paths, CALLER, key, &[hook])
    }

    /// The real fresh recheck and reaffirm callback body compose publication
    /// admission with the real custody witness. Only APFS binding is modelled:
    /// ordinary tests use a tiny private directory, never an image or effect.
    #[test]
    fn guarded_fresh_and_reaffirm_compose_custody_and_refuse_foreign_admission() {
        if crate::store::tests::fixture_child("prune::delegation::tests::guarded_fresh_and_reaffirm_compose_custody_and_refuse_foreign_admission") {
            return;
        }
        let (fixture, control, store, candidate) = super::super::tests::delegated_group_fixture();
        // Labelled domain premise: the caller's own clean barrier is recorded
        // in this very pool. Custody itself is never substituted.
        let mut state = crate::store::State::default();
        state.cleans.push(crate::store::Clean {
            id: 1,
            checkout: candidate.checkout.clone(),
            process_group: None,
        });
        let write_state = |control: &Path, state: &crate::store::State| {
            let temp = control.join("state.guard-test");
            fs::write(&temp, serde_json::to_vec(state).unwrap()).unwrap();
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).unwrap();
            fs::rename(temp, control.join("state.json")).unwrap();
        };
        write_state(&control, &state);
        let evidence =
            match super::super::checkout_absence(&super::super::candidate_row(&candidate), false)
                .unwrap()
            {
                super::super::Absence::Absent(witness) => super::super::SnapshotEvidence {
                    witness: *witness,
                    legacy_exception: false,
                },
                _ => panic!("fixture checkout must be positively absent"),
            };
        fn directory_root(row: &super::super::Row) -> Result<super::super::RootState, String> {
            use std::os::unix::fs::MetadataExt;
            let metadata = fs::symlink_metadata(&row.root).map_err(|e| e.to_string())?;
            assert!(metadata.is_dir());
            Ok(super::super::RootState::Present(super::super::RootFacts {
                dev: metadata.dev(),
                ino: metadata.ino(),
                marker_digest: super::super::outputs::marker_digest(&row.root)?,
                legacy: false,
            }))
        }
        with_test_journal(&fixture.root, |paths| {
            let payload = payload(&control, &candidate);
            let record = begin(paths, &payload);
            let fresh_recheck = || {
                let guard = crate::store::publication_guard(&control)?;
                super::super::recheck_candidate(
                    &store,
                    &control,
                    &candidate,
                    None,
                    &evidence,
                    Some(&guard),
                    directory_root,
                )
            };
            let reaffirm = || {
                Reaffirm {
                    store: &store,
                    control: &control,
                }
                .reaffirm_with_root(&record, |payload| {
                    // Model only the mounted-volume premise. Marker bytes and
                    // inode checks still read this fixture's actual root.
                    let root = fs::File::open(&payload.identity.root).map_err(|e| e.to_string())?;
                    use std::os::fd::AsFd;
                    let root = root
                        .as_fd()
                        .try_clone_to_owned()
                        .map_err(|e| e.to_string())?;
                    super::super::verify_root_marker(&root, &candidate)?;
                    Ok(root)
                })
            };
            assert_eq!(fresh_recheck().unwrap(), payload.registry_revision);
            reaffirm().unwrap();

            // The borrowed internal lock must not suppress external admission.
            let registration = crate::removable::Registration {
                version: crate::removable::REGISTRATION_VERSION,
                mount_point: fixture.root.clone(),
                volume_uuid: candidate.volume_uuid.clone().unwrap(),
                container_uuid: candidate.volume_uuid.clone().unwrap(),
                reserve_bytes: 1 << 20,
                cap_bytes: 1 << 20,
                quota_bytes: None,
                capacity_bytes: None,
                registered_ms: 17,
                mode: crate::removable::DriveMode::Ephemeral,
            };
            let registration_path = control.join(crate::removable::REGISTRATION);
            fs::write(
                &registration_path,
                serde_json::to_vec(&registration).unwrap(),
            )
            .unwrap();
            fs::set_permissions(registration_path, fs::Permissions::from_mode(0o600)).unwrap();
            let sibling = crate::removable::removable_control(&control);
            fs::create_dir_all(&sibling).unwrap();
            fs::set_permissions(&sibling, fs::Permissions::from_mode(0o700)).unwrap();
            write_state(&sibling, &crate::store::State::default());
            let foreign = crate::store::lock_admission(&sibling).unwrap();
            let reason = fresh_recheck().unwrap_err();
            assert!(
                reason.contains("removable pool's admission lock is busy"),
                "{reason}"
            );
            let reason = reaffirm().unwrap_err().to_string();
            assert!(
                reason.contains("removable pool's admission lock is busy"),
                "{reason}"
            );
            drop(foreign);
            assert_eq!(fresh_recheck().unwrap(), payload.registry_revision);
            reaffirm().unwrap();

            // Both guarded paths must freshly read the next durable writer
            // state, rather than carrying a clear verdict across revisions.
            let mut writing = state.clone();
            writing.records.push(crate::store::Record {
                id: 2,
                identity: "identity".to_owned(),
                compatibility: "compatibility".to_owned(),
                profile: "quick".to_owned(),
                checkout: Some(candidate.checkout.clone()),
                phase: crate::store::Phase::Running,
                process_group: None,
                last_used: 1,
                holder: None,
                lease_id: None,
                pending_recovery: None,
                retired: None,
                lost: None,
                last_used_ms: Some(1),
                class: crate::store::WorkspaceClass::Normal,
                class_reason: None,
            });
            write_state(&control, &writing);
            assert!(fresh_recheck().unwrap_err().contains("active writer"));
            assert!(reaffirm()
                .unwrap_err()
                .to_string()
                .contains("active writer"));
            assert_eq!(
                view().unwrap().records[0],
                record,
                "no journal progression or effect"
            );
        });
    }

    #[test]
    fn real_dispatch_settles_exact_group_atomically_and_retries_a_lost_ack() {
        let (fixture, control, _store, candidate) = super::super::tests::delegated_group_fixture();
        with_test_journal(&fixture.root, |paths| {
            let payload = payload(&control, &candidate);
            let record = terminal_premise(paths, begin(paths, &payload));
            let hook = DomainHook {
                control: &control,
                paths,
                lost_ack: std::cell::Cell::new(true),
            };
            let receipt = fs::read(super::super::outputs::receipt_path_public(
                &control,
                &candidate.checkout_id,
            ))
            .unwrap();
            assert!(dispatch(paths, &hook, &payload.key).is_err());
            let registry = super::super::ready_registry(&control).unwrap();
            assert!(!registry.rows.iter().any(|row| row.root == candidate.root));
            assert_eq!(registry.rows.len(), 2, "selected successor remains");
            assert_eq!(
                registry.settled.as_ref().unwrap(),
                &vec![payload.settlement().unwrap()]
            );
            assert!(registry.intents.is_empty(), "no second progress journal");
            assert_eq!(view().unwrap().records[0], record, "ack loss remains owed");
            let revision = registry.revision;
            let before = fs::read(super::super::registry_path(&control)).unwrap();
            let mut incomplete = registry.clone();
            incomplete.complete = false;
            incomplete.incomplete = Some(super::super::Incomplete {
                reason: "labelled fixture incompleteness".to_owned(),
                since_ms: 1,
                transaction: "a".repeat(32),
            });
            fs::write(
                super::super::registry_path(&control),
                serde_json::to_vec(&incomplete).unwrap(),
            )
            .unwrap();
            assert!(
                dispatch(paths, &hook, &payload.key).is_err(),
                "a settlement fact does not waive complete-registry reading"
            );
            assert_eq!(view().unwrap().records[0].phase, OperationPhase::Deleted);
            fs::write(super::super::registry_path(&control), before).unwrap();
            assert_eq!(
                dispatch(paths, &hook, &payload.key).unwrap().phase,
                OperationPhase::Complete
            );
            assert_eq!(
                super::super::ready_registry(&control).unwrap().revision,
                revision,
                "re-dispatch is read-only"
            );
            assert_eq!(
                fs::read(super::super::outputs::receipt_path_public(
                    &control,
                    &candidate.checkout_id
                ))
                .unwrap(),
                receipt
            );
        });
    }

    #[test]
    fn interrupted_real_marking_rechecks_every_member_before_atomic_removal() {
        let (fixture, control, _store, candidate) = super::super::tests::delegated_group_fixture();
        with_test_journal(&fixture.root, |paths| {
            let payload = payload(&control, &candidate);
            terminal_premise(paths, begin(paths, &payload));
            let initial = super::super::ready_registry(&control).unwrap().revision;
            let hook = DomainHook {
                control: &control,
                paths,
                lost_ack: std::cell::Cell::new(false),
            };
            STOP_AFTER_DOMAIN_MARK.with(|stop| stop.set(true));
            assert!(dispatch(paths, &hook, &payload.key).is_err());
            let marked = super::super::ready_registry(&control).unwrap();
            assert_eq!(marked.revision, initial + 1);
            assert!(marked
                .rows
                .iter()
                .filter(|row| row.root == candidate.root)
                .all(|row| row.pruned_ms == Some(payload.terminal_ms)));
            assert!(marked.settled.as_ref().unwrap().is_empty());
            let mut changed = marked.clone();
            changed
                .rows
                .iter_mut()
                .find(|row| row.root == candidate.root)
                .unwrap()
                .nonce = Some("f".repeat(32));
            fs::write(
                super::super::registry_path(&control),
                serde_json::to_vec(&changed).unwrap(),
            )
            .unwrap();
            let before = fs::read(super::super::registry_path(&control)).unwrap();
            assert!(dispatch(paths, &hook, &payload.key).is_err());
            assert_eq!(
                fs::read(super::super::registry_path(&control)).unwrap(),
                before
            );
            // Restore only this unit fixture's controlled interleave. The
            // retained original marking must not be published a second time.
            fs::write(
                super::super::registry_path(&control),
                serde_json::to_vec(&marked).unwrap(),
            )
            .unwrap();
            assert_eq!(
                dispatch(paths, &hook, &payload.key).unwrap().phase,
                OperationPhase::Complete
            );
            let done = super::super::ready_registry(&control).unwrap();
            assert_eq!(done.revision, initial + 2);
            assert_eq!(
                done.settled.as_ref().unwrap(),
                &vec![payload.settlement().unwrap()]
            );
        });
    }

    #[test]
    fn partial_absent_recreated_receipt_and_foreign_reference_states_remain_owed() {
        for mutation in [
            "partial",
            "absent",
            "extra",
            "recreated",
            "selected",
            "receipt",
            "foreign",
            "full",
            "incomplete",
        ] {
            let (fixture, control, _store, candidate) =
                super::super::tests::delegated_group_fixture();
            with_test_journal(&fixture.root, |paths| {
                let payload = payload(&control, &candidate);
                terminal_premise(paths, begin(paths, &payload));
                let mut registry = super::super::ready_registry(&control).unwrap();
                match mutation {
                    "incomplete" => {
                        registry.complete = false;
                        registry.incomplete = Some(super::super::Incomplete {
                            reason: "external fixture incompleteness".to_owned(),
                            since_ms: 1,
                            transaction: "a".repeat(32),
                        });
                    }
                    "partial" => {
                        registry.rows.remove(0);
                    }
                    "absent" => registry.rows.retain(|row| row.root != candidate.root),
                    "extra" => {
                        let mut row = registry.rows[0].clone();
                        row.member = "extra".to_owned();
                        registry.rows.push(row);
                    }
                    "recreated" => registry.rows[0].nonce = Some("f".repeat(32)),
                    "selected" => registry.rows[0].selected = true,
                    "receipt" => {
                        let path = super::super::outputs::receipt_path_public(
                            &control,
                            &candidate.checkout_id,
                        );
                        fs::write(path, b"changed receipt preimage").unwrap();
                    }
                    "foreign" => {
                        let mut row = registry.rows[0].clone();
                        row.checkout_id = "f".repeat(64);
                        row.selected = true;
                        registry.rows.push(row);
                    }
                    "full" => {
                        registry.settled = Some(
                            (0..super::super::MAX_INTENTS)
                                .map(|index| {
                                    let mut entry = payload.settlement().unwrap();
                                    entry.key = format!("{index:032x}");
                                    entry
                                })
                                .collect(),
                        );
                    }
                    _ => unreachable!(),
                }
                // External interleave fixture: write the changed revision
                // directly, never ask production to bypass its consult.
                registry.bump().unwrap();
                fs::write(
                    super::super::registry_path(&control),
                    serde_json::to_vec(&registry).unwrap(),
                )
                .unwrap();
                let before = fs::read(super::super::registry_path(&control)).unwrap();
                let hook = DomainHook {
                    control: &control,
                    paths,
                    lost_ack: std::cell::Cell::new(false),
                };
                assert!(dispatch(paths, &hook, &payload.key).is_err(), "{mutation}");
                assert_eq!(
                    fs::read(super::super::registry_path(&control)).unwrap(),
                    before,
                    "{mutation}: no subset marking"
                );
                assert_eq!(view().unwrap().records[0].phase, OperationPhase::Deleted);
            });
        }
    }

    #[test]
    fn actual_publication_consults_refuse_busy_missing_gap_and_conflicting_receipts() {
        let (fixture, control, _store, candidate) = super::super::tests::delegated_group_fixture();
        with_test_journal(&fixture.root, |paths| {
            let payload = payload(&control, &candidate);
            let record = begin(paths, &payload);
            let registry = super::super::ready_registry(&control).unwrap();
            let mut changed = registry.clone();
            changed.rows[0].selected = true;
            changed.bump().unwrap();
            let before = fs::read(super::super::registry_path(&control)).unwrap();
            assert!(super::super::write_registry(&control, &changed).is_err());
            let receipt = super::super::outputs::read_receipt_for_prune(
                &control,
                &candidate.checkout,
                &candidate.checkout_id,
            )
            .unwrap()
            .unwrap();
            assert!(
                super::super::note_receipt(&control, &receipt).is_err(),
                "equal rows cannot replace receipt bytes"
            );
            assert_eq!(
                fs::read(super::super::registry_path(&control)).unwrap(),
                before
            );
            let mut locks = LockSet::new(paths.clone());
            let held = locks.lock_operations().unwrap();
            assert!(view().unwrap_err().contains("lock"));
            drop(held);
            let file = paths
                .operations_dir()
                .join(format!("{}.json", record.sequence));
            let retained = file.with_extension("retained");
            fs::rename(&file, &retained).unwrap();
            assert!(
                super::super::write_registry(&control, &changed).is_err(),
                "gap never means empty"
            );
            fs::rename(&retained, &file).unwrap();
            fs::rename(
                paths.operations_dir(),
                fixture.root.join("retained-journal"),
            )
            .unwrap();
            assert!(
                super::super::write_registry(&control, &changed).is_err(),
                "missing bound journal refuses"
            );
        });
    }

    #[test]
    fn own_dispatch_context_never_exempts_another_conflicting_operation() {
        for conflicting in [true, false] {
            let (fixture, control, _store, candidate) =
                super::super::tests::delegated_group_fixture();
            with_test_journal(&fixture.root, |paths| {
                let original = payload(&control, &candidate);
                terminal_premise(paths, begin(paths, &original));
                let mut other = original.clone();
                other.key = "f".repeat(32);
                other.identity.ino += 1; // A distinct unit, not an idempotent begin.
                if conflicting {
                    other.identity.checkout_id = "f".repeat(64);
                } else {
                    other.identity.root = other.identity.root.with_file_name("other");
                }
                begin(paths, &other);
                let before = fs::read(super::super::registry_path(&control)).unwrap();
                let hook = DomainHook {
                    control: &control,
                    paths,
                    lost_ack: std::cell::Cell::new(false),
                };
                let result = dispatch(paths, &hook, &original.key);
                assert_eq!(result.is_err(), conflicting);
                if conflicting {
                    assert_eq!(
                        fs::read(super::super::registry_path(&control)).unwrap(),
                        before
                    );
                    assert_eq!(view().unwrap().records[0].phase, OperationPhase::Deleted);
                } else {
                    assert_eq!(result.unwrap().phase, OperationPhase::Complete);
                    assert_eq!(
                        view().unwrap().records[1].phase,
                        OperationPhase::Intent,
                        "unrelated operation preserved"
                    );
                }
            });
        }
    }

    #[test]
    fn concurrent_actual_dispatchers_publish_one_settlement_and_acknowledge_once() {
        let (fixture, control, _store, candidate) = super::super::tests::delegated_group_fixture();
        with_test_journal(&fixture.root, |paths| {
            let original = payload(&control, &candidate);
            let record = terminal_premise(paths, begin(paths, &original));
            let revision = super::super::ready_registry(&control).unwrap().revision;
            let rendezvous = (std::sync::Mutex::new(0u32), std::sync::Condvar::new());
            struct Concurrent<'a> {
                control: &'a Path,
                paths: &'a StatePaths,
                gate: &'a (std::sync::Mutex<u32>, std::sync::Condvar),
            }
            impl FinalizeHook for Concurrent<'_> {
                fn kind(&self) -> &str {
                    HOOK
                }
                fn version(&self) -> &str {
                    HOOK_VERSION
                }
                fn finalize(
                    &self,
                    context: &FinalizationContext,
                    record: &OperationRecord,
                ) -> barge::error::Result<()> {
                    let payload = Payload::from_record(record).map_err(domain_error)?;
                    let mut count = self.gate.0.lock().unwrap();
                    *count += 1;
                    self.gate.1.notify_all();
                    let (count, _) = self
                        .gate
                        .1
                        .wait_timeout_while(count, std::time::Duration::from_secs(2), |count| {
                            *count < 2
                        })
                        .unwrap();
                    if *count < 2 {
                        return Err(domain_error(
                            "test dispatcher rendezvous timed out".to_owned(),
                        ));
                    }
                    drop(count);
                    settle_domain(self.control, self.paths, context, &payload).map_err(domain_error)
                }
            }
            std::thread::scope(|scope| {
                let work = || {
                    let hook = Concurrent {
                        control: &control,
                        paths,
                        gate: &rendezvous,
                    };
                    dispatch(paths, &hook, &original.key)
                };
                let first = scope.spawn(work);
                let second = scope.spawn(work);
                let results = [first.join().unwrap(), second.join().unwrap()];
                assert!(
                    results.iter().any(Result::is_ok),
                    "at least one dispatcher acknowledges"
                );
                for result in results {
                    match result {
                        Ok(record) => assert_eq!(record.phase, OperationPhase::Complete),
                        Err(error) => {
                            assert!(error.to_string().to_lowercase().contains("lock"), "{error}")
                        }
                    }
                }
            });
            let registry = super::super::ready_registry(&control).unwrap();
            assert_eq!(registry.revision, revision + 2);
            assert_eq!(
                registry.settled.as_ref().unwrap(),
                &vec![original.settlement().unwrap()]
            );
            assert_eq!(view().unwrap().records[0].generation, record.generation + 1);
        });
    }

    #[test]
    fn payload_limits_and_original_unit_binding_are_checked_before_begin() {
        let (fixture, control, _store, candidate) = super::super::tests::delegated_group_fixture();
        with_test_journal(&fixture.root, |paths| {
            let mut payload = payload(&control, &candidate);
            let record = begin(paths, &payload);
            assert!(Payload::from_record(&record).is_ok());
            for mutate in [
                |record: &mut OperationRecord| record.unit.inode += 1,
                |record: &mut OperationRecord| record.declaration.as_mut().unwrap().generation += 1,
                |record: &mut OperationRecord| {
                    record.finalize.as_mut().unwrap().version = "2".to_owned()
                },
                |record: &mut OperationRecord| record.request_digest = Some("f".repeat(64)),
            ] {
                let mut changed = record.clone();
                mutate(&mut changed);
                assert!(Payload::from_record(&changed).is_err());
            }
            payload.group = vec![payload.group[0].clone(); GROUP_LIMIT + 1];
            assert!(payload.encode().is_err());
            payload.group.truncate(1);
            payload.identity.checkout = PathBuf::from(format!("/{}", "x".repeat(PAYLOAD_LIMIT)));
            assert!(payload.encode().is_err());
            assert_eq!(
                view().unwrap().records.len(),
                1,
                "unsupported payload cannot allocate"
            );
        });
    }
}

fn operation_fault(payload: &Payload, fault: &str) -> bool {
    #[cfg(all(debug_assertions, feature = "outputs-operation-proof"))]
    if super::proof_evidence().is_some()
        && std::env::var_os("OXIDE_TEST_OPERATION_ROOT").as_deref()
            == Some(payload.identity.root.as_os_str())
    {
        return std::env::var("OXIDE_TEST_OPERATION_FAULT").is_ok_and(|value| value == fault);
    }
    let _ = (payload, fault);
    false
}

fn arm_operation_fault(payload: &Payload) {
    #[cfg(all(debug_assertions, feature = "outputs-operation-proof"))]
    {
        if operation_fault(payload, "renamed") {
            delete::stop_after_operation_renamed();
        }
        if operation_fault(payload, "removed") {
            delete::stop_after_operation_removed();
        }
        if operation_fault(payload, "capture-gap") {
            delete::fail_after_operation_capture();
        }
        if operation_fault(payload, "unlink-gap") {
            delete::fail_after_operation_unlink();
        }
    }
    let _ = payload;
}

/// A bounded observation/pause of this new operation's actual durable point.
/// It is unreachable without both the default-off feature and current guard.
fn operation_point(control: &Path, payload: &Payload, phase: &str) -> Result<(), String> {
    #[cfg(all(debug_assertions, feature = "outputs-operation-proof"))]
    if super::prune_fixture_fault(
        control,
        "OXIDE_TEST_OPERATION_ROOT",
        &payload.identity.root,
        &format!("operation-{phase}-{}", payload.key),
    ) && std::env::var("OXIDE_TEST_OPERATION_PAUSE").is_ok_and(|value| value == phase)
    {
        let evidence = super::proof_evidence().ok_or("current guard evidence is missing")?;
        let release = evidence.join(format!("operation-release-{}", payload.key));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !release.exists() {
            if std::time::Instant::now() >= end {
                return Err(format!(
                    "operation {phase} pause expired; original record remains authoritative"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let _ = (control, payload, phase);
    Ok(())
}
