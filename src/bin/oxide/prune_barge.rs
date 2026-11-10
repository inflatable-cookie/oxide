//! Covered-root handoff. Oxide owns eligibility and settlement; only Barge
//! executes or replays deletion. No branch here opens a local quarantine.
#[cfg(debug_assertions)]
#[path = "prune_barge_observed.rs"]
mod observed;
use super::{
    request, Candidate, DeletionRoute, Intent, IntentPhase, Registry, Resumed, RootKind,
    RootSession, Store,
};
use barge::oxide_outputs::{
    self, AdapterCall, BargeLookup, DelegatedRequest, Outcome, OutcomeKind,
};
use barge::state::StatePaths;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(super) fn route<'a>(registry: &'a Registry, id: &str) -> Option<&'a str> {
    registry
        .deletion_routes
        .as_ref()?
        .iter()
        .find_map(|route| match route {
            DeletionRoute::Barge {
                intent_id,
                request_sha256,
            } if intent_id == id => Some(request_sha256.as_str()),
            _ => None,
        })
}

/// Called only after the complete batch's existing receipt/history, identity,
/// absence and custody checks. Authority availability never changes this route.
pub(super) fn covered(registry: &Registry, candidate: &Candidate) -> Result<bool, String> {
    if registry.version != super::DELEGATED_REGISTRY_VERSION
        || candidate.receiptless
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

fn metadata(
    control: &Path,
    registry: &Registry,
    intent: &Intent,
) -> Result<(request::Metadata, String), String> {
    let sha = route(registry, &intent.id).ok_or("the operation has no durable Barge route")?;
    Ok((request::load(control, intent, sha)?, sha.to_owned()))
}

fn bound_request(metadata: &request::Metadata, sha: &str) -> Result<DelegatedRequest, String> {
    match metadata {
        request::Metadata::Ready {
            grant_generation, ..
        } => Ok(DelegatedRequest {
            grant_generation: *grant_generation,
            request_sha256: Some(sha.to_owned()),
        }),
        request::Metadata::Unavailable { reason, .. } => Err(format!(
            "original Barge authority was unavailable: {reason}; no replacement grant is allowed"
        )),
    }
}

fn lookup(intent: &Intent, request: &DelegatedRequest, actual: Option<&str>) -> BargeLookup {
    let paths = match state_paths() {
        Ok(paths) => paths,
        Err(reason) => return BargeLookup::Unknown { reason },
    };
    let os = barge::os::platform();
    oxide_outputs::lookup(
        &paths,
        &os,
        &os,
        &os,
        &os,
        &request::original_claim(intent),
        request,
        actual,
    )
}

fn absence_only(intent: &Intent) -> Result<(), String> {
    let paths = state_paths()?;
    let os = barge::os::platform();
    match oxide_outputs::lookup_absence(
        &paths,
        &os,
        &os,
        &os,
        &os,
        &request::original_claim(intent),
    ) {
        oxide_outputs::AbsenceLookup::PositivelyAbsent => Ok(()),
        oxide_outputs::AbsenceLookup::Unknown { reason } => Err(reason),
    }
}

fn execute(_control: &Path, intent: &Intent, request: &DelegatedRequest) -> Outcome {
    let paths = match state_paths() {
        Ok(paths) => paths,
        Err(reason) => {
            return Outcome {
                kind: OutcomeKind::Unknown,
                caller_intent_id: intent.id.clone(),
                intent_id: None,
                reason: Some(reason),
            }
        }
    };
    let os = barge::os::platform();
    let claim = request::original_claim(intent);
    #[cfg(debug_assertions)]
    let observed = observed::ObservedDirs {
        inner: &os,
        control: _control,
        intent,
        renamed: std::cell::Cell::new(false),
    };
    #[cfg(debug_assertions)]
    let dirs: &dyn barge::os::DirOps = &observed;
    #[cfg(not(debug_assertions))]
    let dirs: &dyn barge::os::DirOps = &os;
    let outcome = oxide_outputs::execute(&AdapterCall {
        state_paths: &paths,
        dirs,
        facts: &os,
        mounts: &os,
        observer: &os,
        account: &os,
        intent: &claim,
        request,
        bounds: oxide_outputs::AdapterBounds::design(),
    });
    #[cfg(debug_assertions)]
    if super::prune_fixture_fault(
        _control,
        "OXIDE_TEST_BARGE_RECORD_REPLY",
        &intent.identity.root,
        &format!(
            "barge-actual-reply-{}-{}.armed",
            intent.id,
            std::process::id()
        ),
    ) {
        if let Some(evidence) = super::proof_evidence() {
            let value = serde_json::json!({
                "caller_intent_id": outcome.caller_intent_id,
                "intent_id": outcome.intent_id,
                "kind": format!("{:?}", outcome.kind),
                "reason": outcome.reason.as_deref().map(|s| s.chars().take(2048).collect::<String>()),
                "root": intent.identity.root,
            });
            let path = evidence.join(format!(
                "barge-actual-reply-{}-{}.json",
                intent.id,
                std::process::id()
            ));
            // Observation only; the proof refuses a missing/malformed record.
            let _ = std::fs::write(path, value.to_string());
        }
    }
    #[cfg(debug_assertions)]
    if super::prune_fixture_fault(
        _control,
        "OXIDE_TEST_BARGE_LOST_REPLY",
        &intent.identity.root,
        &format!("barge-reply-lost-{}", intent.id),
    ) {
        return Outcome {
            kind: OutcomeKind::Unknown,
            caller_intent_id: intent.id.clone(),
            intent_id: None,
            reason: Some("the adapter reply was lost".to_owned()),
        };
    }
    #[cfg(debug_assertions)]
    if super::prune_fixture_fault(
        _control,
        "OXIDE_TEST_BARGE_MALFORMED_REPLY",
        &intent.identity.root,
        &format!("barge-reply-malformed-{}", intent.id),
    ) {
        return Outcome {
            caller_intent_id: "malformed".to_owned(),
            ..outcome
        };
    }
    outcome
}

/// The first-call privilege is consumed before invoking the adapter, even if
/// its reply is lost. Every replay entry constructs a resumed attempt.
#[derive(Debug)]
struct Attempt {
    fresh: bool,
}
impl Attempt {
    fn take_first(&mut self) -> bool {
        std::mem::replace(&mut self.fresh, false)
    }
}

fn correlated(intent: &Intent, outcome: &Outcome) -> Result<(), String> {
    if outcome.caller_intent_id != intent.id
        || outcome
            .intent_id
            .as_deref()
            .is_some_and(|id| !super::hex32(id))
        || (matches!(outcome.kind, OutcomeKind::Deleted | OutcomeKind::Pending)
            && outcome.intent_id.is_none())
        || (outcome.kind == OutcomeKind::KnownNoEffectLocked && outcome.intent_id.is_some())
    {
        return Err("Barge returned a malformed or uncorrelated reply".to_owned());
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
enum ReplyAction {
    Deleted(String),
    KnownNoEffect,
    ProveAbsence,
    Pending,
}

fn reply_action(intent: &Intent, outcome: &Outcome, first: bool) -> Result<ReplyAction, String> {
    correlated(intent, outcome)?;
    Ok(match outcome.kind {
        OutcomeKind::Deleted => ReplyAction::Deleted(
            outcome
                .intent_id
                .clone()
                .ok_or("Barge omitted its terminal identity")?,
        ),
        OutcomeKind::KnownNoEffectLocked if first => ReplyAction::KnownNoEffect,
        OutcomeKind::RolledBack | OutcomeKind::Refused => ReplyAction::ProveAbsence,
        _ => ReplyAction::Pending,
    })
}

fn pending(intent: &Intent, reason: impl std::fmt::Display) -> String {
    format!("operation {} for {} remains pending: {reason}; retry `oxide outputs prune --plan FILE --plan-sha256 HEX` with the original plan, or inspect `oxide outputs resolve-intent {}` after Barge safely settles; never locally delete this root", intent.id, intent.identity.root.display(), intent.id)
}

/// Existing Oxide authority remains a gate before every resumed adapter effect.
fn recheck(
    store: &Store,
    control: &Path,
    registry: &Registry,
    intent: &Intent,
) -> Result<(), String> {
    if !registry.complete {
        return Err("the registry is incomplete".to_owned());
    }
    let live = registry
        .intent(&intent.id)
        .ok_or("the intent disappeared")?;
    if live.identity != intent.identity
        || live.phase != intent.phase
        || live.preview_sha256 != intent.preview_sha256
    {
        return Err("the durable intent changed".to_owned());
    }
    let rows: Vec<_> = registry
        .rows
        .iter()
        .filter(|row| {
            row.checkout_id == intent.identity.checkout_id && row.root == intent.identity.root
        })
        .collect();
    if rows.len() != intent.group.len() || rows.is_empty() {
        return Err("the blocking registry group changed".to_owned());
    }
    for old in &intent.group {
        let row = rows
            .iter()
            .find(|row| row.member == old.member)
            .ok_or("a group member disappeared")?;
        if row.selected
            || row.pending
            || row.reservation.is_some()
            || row.cancelling.is_some()
            || row.pruning.as_deref() != Some(intent.id.as_str())
            || row.nonce != old.nonce
            || row.generation != old.generation
            || row.root_dev != old.root_dev
            || row.root_ino != old.root_ino
            || row.marker_digest != old.marker_digest
            || row.checkout != intent.identity.checkout
            || row.receipt != intent.identity.receipt
            || row.root_kind != intent.identity.root_kind
            || row.volume_uuid != intent.identity.volume_uuid
        {
            return Err("a blocking group member changed or became selected".to_owned());
        }
    }
    if registry.rows.iter().any(|row| {
        row.root == intent.identity.root
            && row.checkout_id != intent.identity.checkout_id
            && row.live()
    }) {
        return Err("another checkout references this root".to_owned());
    }
    if !intent.prior_receipt_sha.is_empty()
        && super::receipt_preimage(control, &intent.identity.checkout_id)?
            != intent.prior_receipt_sha
    {
        return Err("the original receipt changed before Barge settled".to_owned());
    }
    super::verify_group_receipt(
        control,
        &intent.identity.checkout,
        &intent.identity.checkout_id,
        &intent.identity.root,
        &intent.group,
    )?;
    let mut authority = super::AuthorityScope::begin()?;
    super::custody_clear_under_barrier(&mut authority, control, &intent.identity.checkout)?;
    match super::checkout_absence(rows[0], false)? {
        super::Absence::Absent(_) => {}
        super::Absence::Present => return Err("the checkout reappeared".to_owned()),
        super::Absence::Unknown(reason) => return Err(reason),
    }
    if !authority.complete() {
        return Err("the recovery authority budget is incomplete".to_owned());
    }
    let _ = store; // Caller retains the store's clean barrier and binding.
    Ok(())
}

fn original_matches(intent: &Intent) -> Result<(), String> {
    let session = RootSession::open_recorded(&intent.identity)?;
    session.verify_marker(&super::journal_candidate(intent))
}

/// Positively observe the exact Barge quarantine. The actual Barge id comes
/// from its checked API; it is never replaced with the caller's id.
fn leaves_absent(intent: &Intent, actual: &str, original: bool) -> Result<(), String> {
    if !super::hex32(actual) {
        return Err("Barge intent identity is malformed".to_owned());
    }
    let root = &intent.identity.root;
    let mount = root
        .parent()
        .and_then(Path::parent)
        .ok_or("the root has no mount")?;
    let binding =
        oxide_storage::volume::Binding::open(mount, intent.identity.volume_uuid.as_deref())
            .map_err(|error| error.to_string())?;
    let outputs = binding
        .dir_at(binding.fd(), super::outputs::EXTERNAL_DIR, false)
        .map_err(|error| error.to_string())?;
    let quarantine = format!(".barge-prune-{actual}");
    let mut leaves = vec![std::ffi::OsStr::new(&quarantine)];
    if original {
        leaves.push(root.file_name().ok_or("the root has no leaf")?);
        leaves.push(
            intent
                .quarantine
                .file_name()
                .ok_or("the local quarantine has no leaf")?,
        );
    }
    for leaf in leaves {
        let name = std::ffi::CString::new(leaf.as_bytes()).map_err(|error| error.to_string())?;
        // SAFETY: stat is an output buffer; the held fd and terminated leaf
        // remain live throughout fstatat. No link is followed or changed.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::fstatat(
                outputs.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status == 0 {
            return Err(format!("{} is still present", leaf.to_string_lossy()));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("leaf absence is unknown: {error}"));
        }
    }
    binding.verify().map_err(|error| error.to_string())?;
    Ok(())
}

fn settle_deleted(
    store: &Store,
    control: &Path,
    intent: &Intent,
    request: &DelegatedRequest,
    actual: &str,
) -> Result<Resumed, String> {
    match lookup(intent, request, Some(actual)) {
        BargeLookup::FoundTerminal { intent_id } if intent_id == actual => {}
        other => {
            return Err(format!(
                "Barge terminal confirmation is not exact: {other:?}"
            ))
        }
    }
    leaves_absent(intent, actual, true)?;
    if intent.phase == IntentPhase::Intent {
        let registry = super::ready_registry(control)?;
        recheck(store, control, &registry, intent)?;
    }
    super::complete_intent(store, control, &intent.id, "deleted").map_err(super::stop_text)?;
    Ok(Resumed::Completed)
}

fn call(
    store: &Store,
    control: &Path,
    intent: &Intent,
    request: &DelegatedRequest,
    attempt: &mut Attempt,
    expected_barge_id: Option<&str>,
) -> Result<Resumed, String> {
    let first = attempt.take_first();
    #[cfg(debug_assertions)]
    if super::prune_fixture_fault(
        control,
        "OXIDE_TEST_BARGE_BEFORE_CALL",
        &intent.identity.root,
        &format!("barge-before-call-{}", intent.id),
    ) {
        let evidence = super::proof_evidence().ok_or("proof evidence unavailable")?;
        let stop = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !evidence
            .join(format!("barge-call-release-{}", intent.id))
            .exists()
        {
            if std::time::Instant::now() >= stop {
                return Err("bounded adapter-call fixture pause expired".to_owned());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    let outcome = execute(control, intent, request);
    if let (Some(expected), Some(actual)) = (expected_barge_id, outcome.intent_id.as_deref()) {
        if actual != expected {
            return Err("Barge changed the checked operation identity; keep pending".to_owned());
        }
    }
    match reply_action(intent, &outcome, first)? {
        ReplyAction::Deleted(actual) => settle_deleted(store, control, intent, request, &actual),
        ReplyAction::KnownNoEffect => {
            original_matches(intent)?;
            super::rollback_intent_checked(control, intent, true)?;
            Ok(Resumed::RolledBack)
        }
        ReplyAction::ProveAbsence => match lookup(
            intent,
            request,
            outcome.intent_id.as_deref().or(expected_barge_id),
        ) {
            BargeLookup::PositivelyAbsent => {
                original_matches(intent)?;
                super::rollback_intent_checked(control, intent, true)?;
                Ok(Resumed::RolledBack)
            }
            other => Err(format!(
                "Barge refusal/rollback is not proven absent: {other:?}"
            )),
        },
        ReplyAction::Pending => Err(format!(
            "Barge {} (Barge operation {}): {}",
            outcome.kind.as_str(),
            outcome.intent_id.as_deref().unwrap_or("unknown"),
            outcome.reason.as_deref().unwrap_or("no terminal evidence")
        )),
    }
}

pub(super) fn fresh(store: &Store, control: &Path, intent: &Intent) -> Result<Resumed, String> {
    let result = (|| {
        let registry = super::ready_registry(control)?;
        let (metadata, sha) = metadata(control, &registry, intent)?;
        let request = bound_request(&metadata, &sha)?;
        super::crash_point("delegation-intent")?;
        recheck(store, control, &registry, intent)?;
        call(
            store,
            control,
            intent,
            &request,
            &mut Attempt { fresh: true },
            None,
        )
    })();
    result.map_err(|reason| pending(intent, reason))
}

#[derive(Debug, Eq, PartialEq)]
enum RecoveryAction {
    Terminal(String),
    Rollback,
    Resume(String),
}

fn recovery_action(intent: &Intent, lookup: BargeLookup) -> Result<RecoveryAction, String> {
    if intent.phase != IntentPhase::Intent {
        return Err("a delegated operation cannot use a local rename/delete phase".to_owned());
    }
    match lookup {
        BargeLookup::FoundTerminal { intent_id } if super::hex32(&intent_id) => {
            Ok(RecoveryAction::Terminal(intent_id))
        }
        BargeLookup::PositivelyAbsent => Ok(RecoveryAction::Rollback),
        BargeLookup::FoundPending {
            intent_id,
            phase,
            quarantine_name,
        } if super::hex32(&intent_id)
            && quarantine_name == format!(".barge-prune-{intent_id}")
            && matches!(
                phase.as_str(),
                "intent" | "renamed" | "marker_last" | "removed"
            ) =>
        {
            Ok(RecoveryAction::Resume(intent_id))
        }
        BargeLookup::Unknown { reason } => Err(reason),
        other => Err(format!(
            "Barge recovery binding/phase is unknown: {other:?}"
        )),
    }
}

pub(super) fn resume(store: &Store, control: &Path, intent: &Intent) -> Result<Resumed, String> {
    let result = (|| {
        let registry = super::ready_registry(control)?;
        let (metadata, sha) = metadata(control, &registry, intent)?;
        if intent.phase == IntentPhase::Deleted {
            super::complete_intent(store, control, &intent.id, "deleted")
                .map_err(super::stop_text)?;
            return Ok(Resumed::Completed);
        }
        if intent.phase == IntentPhase::Resolved {
            super::settle_receipt(control, &intent.id, super::RESOLVED_OUTCOME)?;
            return Ok(Resumed::Settled);
        }
        if let request::Metadata::Unavailable { .. } = metadata {
            absence_only(intent)?;
            recheck(store, control, &registry, intent)?;
            original_matches(intent)?;
            super::rollback_intent_checked(control, intent, true)?;
            return Ok(Resumed::RolledBack);
        }
        let request = bound_request(&metadata, &sha)?;
        match recovery_action(intent, lookup(intent, &request, None))? {
            RecoveryAction::Terminal(actual) => {
                settle_deleted(store, control, intent, &request, &actual)
            }
            RecoveryAction::Rollback => {
                recheck(store, control, &registry, intent)?;
                original_matches(intent)?;
                super::rollback_intent_checked(control, intent, true)?;
                Ok(Resumed::RolledBack)
            }
            RecoveryAction::Resume(actual) => {
                recheck(store, control, &registry, intent)?;
                call(
                    store,
                    control,
                    intent,
                    &request,
                    &mut Attempt { fresh: false },
                    Some(&actual),
                )
            }
        }
    })();
    result.map_err(|reason| pending(intent, reason))
}

pub(super) fn resolve_check(
    control: &Path,
    registry: &Registry,
    intent: &Intent,
) -> Result<(), String> {
    if route(registry, &intent.id).is_none() {
        return Ok(());
    }
    let (metadata, sha) = metadata(control, registry, intent)?;
    match metadata {
        request::Metadata::Unavailable { .. } => absence_only(intent),
        _ => match lookup(intent, &bound_request(&metadata, &sha)?, None) {
            BargeLookup::PositivelyAbsent => Ok(()),
            BargeLookup::FoundTerminal { intent_id } => leaves_absent(intent, &intent_id, false),
            other => Err(format!(
                "Barge must be terminal or positively absent before resolving: {other:?}"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent() -> Intent {
        let (_fixture, registry) = crate::prune::tests::routed_registry();
        registry.intents[0].clone()
    }
    fn reply(intent: &Intent, kind: OutcomeKind, actual: Option<String>) -> Outcome {
        Outcome {
            kind,
            caller_intent_id: intent.id.clone(),
            intent_id: actual,
            reason: None,
        }
    }

    #[test]
    fn only_the_first_call_can_use_the_narrow_locked_fact_even_in_one_process() {
        let intent = intent();
        let locked = reply(&intent, OutcomeKind::KnownNoEffectLocked, None);
        let mut attempt = Attempt { fresh: true };
        assert_eq!(
            reply_action(&intent, &locked, attempt.take_first()).unwrap(),
            ReplyAction::KnownNoEffect
        );
        assert_eq!(
            reply_action(&intent, &locked, attempt.take_first()).unwrap(),
            ReplyAction::Pending
        );
        let mut resumed = Attempt { fresh: false };
        assert_eq!(
            reply_action(&intent, &locked, resumed.take_first()).unwrap(),
            ReplyAction::Pending
        );
        let mut lost = Attempt { fresh: true };
        assert_eq!(
            reply_action(
                &intent,
                &reply(&intent, OutcomeKind::Unknown, None),
                lost.take_first()
            )
            .unwrap(),
            ReplyAction::Pending
        );
        assert_eq!(
            reply_action(&intent, &locked, lost.take_first()).unwrap(),
            ReplyAction::Pending
        );
    }

    #[test]
    fn replies_require_correlation_and_refusal_never_directly_rolls_back() {
        let intent = intent();
        let actual = "b".repeat(32);
        let deleted = reply(&intent, OutcomeKind::Deleted, Some(actual.clone()));
        assert_eq!(
            reply_action(&intent, &deleted, false).unwrap(),
            ReplyAction::Deleted(actual)
        );
        for kind in [OutcomeKind::Refused, OutcomeKind::RolledBack] {
            assert_eq!(
                reply_action(&intent, &reply(&intent, kind, None), true).unwrap(),
                ReplyAction::ProveAbsence
            );
        }
        for kind in [OutcomeKind::Pending, OutcomeKind::Unknown] {
            assert_eq!(
                reply_action(&intent, &reply(&intent, kind, Some("e".repeat(32))), true).unwrap(),
                ReplyAction::Pending
            );
        }
        let mut foreign = deleted.clone();
        foreign.caller_intent_id = "c".repeat(32);
        assert!(reply_action(&intent, &foreign, true).is_err());
        let mut missing = deleted.clone();
        missing.intent_id = None;
        assert!(reply_action(&intent, &missing, true).is_err());
        let mut malformed = deleted.clone();
        malformed.intent_id = Some("not-an-id".to_owned());
        assert!(reply_action(&intent, &malformed, true).is_err());
        let locked_with_record = reply(
            &intent,
            OutcomeKind::KnownNoEffectLocked,
            Some("e".repeat(32)),
        );
        assert!(reply_action(&intent, &locked_with_record, true).is_err());
    }

    #[test]
    fn all_four_lookup_states_preserve_the_actual_operation_identity() {
        let mut intent = intent();
        let actual = "c".repeat(32);
        assert_eq!(
            recovery_action(&intent, BargeLookup::PositivelyAbsent).unwrap(),
            RecoveryAction::Rollback
        );
        assert_eq!(
            recovery_action(
                &intent,
                BargeLookup::FoundTerminal {
                    intent_id: actual.clone()
                }
            )
            .unwrap(),
            RecoveryAction::Terminal(actual.clone())
        );
        for phase in ["intent", "renamed", "marker_last", "removed"] {
            assert_eq!(
                recovery_action(
                    &intent,
                    BargeLookup::FoundPending {
                        intent_id: actual.clone(),
                        phase: phase.to_owned(),
                        quarantine_name: format!(".barge-prune-{actual}"),
                    }
                )
                .unwrap(),
                RecoveryAction::Resume(actual.clone())
            );
        }
        assert!(recovery_action(
            &intent,
            BargeLookup::Unknown {
                reason: "lost state or locked observation".to_owned()
            }
        )
        .is_err());
        assert!(
            recovery_action(
                &intent,
                BargeLookup::FoundPending {
                    intent_id: actual,
                    phase: "renamed".to_owned(),
                    quarantine_name: format!(".barge-prune-{}", intent.id),
                }
            )
            .is_err(),
            "caller and Barge ids are not interchangeable"
        );
        intent.phase = IntentPhase::Renamed;
        assert!(
            recovery_action(&intent, BargeLookup::PositivelyAbsent).is_err(),
            "no local phase can become a rollback"
        );
    }

    #[test]
    fn unavailable_metadata_cannot_create_a_request_with_a_replacement_generation() {
        let intent = intent();
        let bytes = request::encode(&intent, &Err("grant withdrawn".to_owned())).unwrap();
        let metadata: request::Metadata = serde_json::from_slice(&bytes).unwrap();
        assert!(bound_request(&metadata, &"e".repeat(64)).is_err());
        let bytes = request::encode(&intent, &Ok(7)).unwrap();
        let metadata: request::Metadata = serde_json::from_slice(&bytes).unwrap();
        let request = bound_request(&metadata, &"e".repeat(64)).unwrap();
        assert_eq!(request.grant_generation, 7);
        assert_eq!(
            request.request_sha256.as_deref(),
            Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")
        );
    }
}
