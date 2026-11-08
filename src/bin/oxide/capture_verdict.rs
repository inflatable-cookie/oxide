//! The limited (O2-CAL) comparison verdict policy: test-only.
//!
//! Owning contract: `docs/knowledge/contracts/barge-adoption.md`, the
//! "Limited verdict policy" section of the oxide#285 design. This module is
//! reachable only through `src/bin/oxide/tape.rs`, which the binary compiles
//! under `cfg(test)`; a production build compiles none of it and no decision,
//! parser, cap or wire byte is touched.
//!
//! The policy adds one *separate* result beside the strict `/2` result. The
//! strict judgement is `capture_wire::assess_completeness` and it is retained
//! verbatim in every verdict. The limited result may set aside exactly two
//! reviewed obligations:
//!
//! 1. a null transcript window origin and end (the `/2` wire requires null),
//! 2. `parent_required` on an *enumerated* path-only direct-read site whose
//!    leaf identity the transcript binds.
//!
//! Nothing else is excludable. A handle-class (K1) parent, a missing or
//! non-matching leaf identity, a missing mandatory row, a cap or truncation
//! flag, an unobserved clock, an unproven holder, an extra or unbound
//! observation, an attempted/partial/unpathed effect, source, case, digest or
//! topology drift and an unlisted site or an unlisted engine note all remain
//! blocking. An unknown site or an unknown note is blocking, so nothing is
//! silently ignored.
//!
//! The exclusion set is a fixed, source-owned enumeration in this module. It
//! is not a caller-supplied waiver list and it is never derived from the text
//! of a strict issue: strict issues are matched against the recorder's own two
//! fixed templates only, and any strict issue those templates do not explain
//! is recorded as its own blocking reason. A strict insufficiency can
//! therefore never be turned into a limited pass by a matching decision.
//!
//! Two boundaries are deliberate and narrow. First, the manifest's
//! `mandatory_observations` list *is* the declaration of which reads the
//! decision consumes: the policy checks every declared obligation and every
//! row the recorder reports as unbound, but it cannot prove that a manifest
//! declared every input, so obligation completeness stays a review obligation.
//! Second, the recorder's per-row sentences "object or parent identity
//! unproven" and "component parent identity unproven" block only where that
//! row kind is a declared decision input. On any other row the recorder's own
//! bind verdict (`extra or unbound observation`) is what decides whether the
//! row binds the fixture, and duplicating that rule here would be a second,
//! divergent implementation.

use serde_json::{json, Value};

use super::capture_wire;

/// The limited verdict policy version. It is reported in every verdict and in
/// the effect-free plan.
pub(crate) const POLICY_VERSION: &str = "oxide.capture.limited-verdict/1";
/// The effect-free first-batch plan version.
pub(crate) const PLAN_VERSION: &str = "oxide.capture.limited-plan/1";
/// The one claim this policy can support. It is L2 only.
pub(crate) const CLAIM: &str = "L2 owned-fixture decision agreement: window-incomplete, \
parent-unproven at enumerated path-only direct-read sites, holder-lifetime-limited";
/// Why no first-batch case is runnable today, beyond the export gate. Source
/// facts only; no fixture, engine run or measurement is involved.
pub(crate) const BINDING_GAP: &str = "the /2 recorder binds a read row to a fixture entry only \
when the row carries parent device and inode (tape.rs check_manifest_obligations: bound_file \
requires parent_device and parent_inode). No file read records a parent: read_row and \
read_row_owned set none, and authority_read_observed passes a held descriptor's own device and \
inode only for prune::read_bounded. Every path-only file row therefore reports `extra or unbound \
observation at <kind>`, and every parent_required obligation over such a row reports `mandatory \
observation missing or identity unproven at <site>`. The reviewed \
path-only-direct-read-parent exclusion covers the parent obligation only and cannot manufacture a \
fixture binding, so no first-batch case is runnable. Closing the gap is a separately reviewed \
test-only recorder change (bind a leaf identity without a parent at the enumerated path-only \
sites, and give the parent-required obligation note its own declared rule); this preparation does \
not make it";

/// The exact smallest Barge source requirement for the first batch, read from the
/// pinned sibling `f9f81f72`. The harness, argv contract, `release::plan`
/// call and export binding already exist; what is missing is fixture breadth.
pub(crate) const BARGE_SUPPORT_REQUIREMENT: &str = "Barge at f9f81f72 already provides the \
in-crate harness: crates/barge/tests/tape_eval.rs (harness = false) implements the fixed argv \
contract, calls the unchanged release::plan through tape_harness::run_bound, requires the export \
binding (BARGE_TAPE_EXPORT_RECEIPT, _SHA256, _CLOSURE_SHA256) and builds one BASE-shaped fixture. \
The smallest remaining source requirement is a per-case fixture builder inside \
crates/barge/src/tape_harness.rs: build_fixture(label) materialises one fixed fixture (one volume \
record, one owned root, one settled Oxide row, an absent checkout) and its manifest carries empty \
fixture.entries, fixture.roots and dynamic.writers, so a first-batch case's own state (registry \
rows and intents, receipt and marker variants, volume records, declaration and fence, operator \
list, legacy rows) and the manifest's description of it cannot be produced. No new schema, argv \
contract, target or release::plan path is needed";

/// The `/2` manifest schema's fixed Barge pin. The comparison commit is carried
/// by `source.barge_slice_commit`; the pin field itself is not a drift knob.
pub(crate) const SCHEMA_BARGE_PIN: &str = "4af9360bd09629e970c143080ca159de8eef2562";
/// The qualified shared supervisor's digest, quoted by the plan only.
pub(crate) const QUALIFIED_RUNNER_SHA256: &str =
    "fcfda2b0a43afd192a4f75e0d575b9aa94f0fd79d195bd75b1c66305f37f046b";

// ---------------------------------------------------------------------------
// Reviewed exclusions
// ---------------------------------------------------------------------------

/// The two reviewed exclusion rules. Both are obligations the `/2` wire and
/// the unqualified K1 premise cannot discharge; neither is a waiver of a
/// decision input's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ExclusionRule {
    /// `/2` requires a null window origin and end, so no `/2` transcript can
    /// prove a window boundary or any absolute-time ordering.
    WindowOriginEndNull,
    /// A bounded direct read whose production reader never opens the parent.
    /// The row still carries the leaf's own `(device, inode)`.
    PathOnlyDirectReadParent,
}

impl ExclusionRule {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::WindowOriginEndNull => "window-origin-end-null",
            Self::PathOnlyDirectReadParent => "path-only-direct-read-parent",
        }
    }

    /// The reviewed reason, recorded on every limitation the verdict names.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::WindowOriginEndNull => {
                "the /2 wire requires a null window origin and end; no cross-engine \
ordering, absolute-time or window-span claim follows"
            }
            Self::PathOnlyDirectReadParent => {
                "the production reader opens no parent for this bounded direct read, so \
parent_required cannot be discharged; the leaf's own identity is bound and named"
            }
        }
    }
}

/// One named limitation: rule, exact site, and the leaf identity that binds it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Limitation {
    pub(crate) rule: ExclusionRule,
    pub(crate) site: String,
    pub(crate) leaf_identity: String,
    pub(crate) obligation: String,
}

// ---------------------------------------------------------------------------
// Site registry: which read classes this policy knows
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SiteClass {
    /// A bounded direct read that opens no parent; the row can carry the
    /// leaf's own device and inode. Only this class is ever eligible for the
    /// reviewed `path-only-direct-read-parent` exclusion.
    PathOnlyDirectRead,
    /// A read or walk component whose parent is a live held descriptor (K1
    /// class). Never eligible for an exclusion.
    HandleParent,
    /// A fixture-declared dynamic observation (mount census, fixture diff).
    DeclaredObservation,
    /// A pool/ledger read behind the custody and admission chain. These rows
    /// belong to the deferred second batch and are never excludable.
    Custody,
}

impl SiteClass {
    fn name(self) -> &'static str {
        match self {
            Self::PathOnlyDirectRead => "path-only-direct-read",
            Self::HandleParent => "handle-parent",
            Self::DeclaredObservation => "declared-observation",
            Self::Custody => "custody",
        }
    }
}

/// One known site. `kind` is the transcript row's `kind` (which is also the
/// manifest obligation's `site`); `source` names the production statement.
struct KnownSite {
    kind: &'static str,
    class: SiteClass,
    source: &'static str,
}

/// The finite site registry, keyed by the row kind the recorder actually
/// emits (`src/bin/oxide/tape.rs` and the instrumented leaves). A site absent
/// from this table is unlisted and blocking: the policy never guesses a site's
/// class. The class is about the row's parent provenance, which is what the
/// one reviewed parent exclusion turns on.
const KNOWN_SITES: &[KnownSite] = &[
    KnownSite {
        kind: "authority-read@prune::read_registry",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/prune.rs read_registry (authority_read_observed)",
    },
    KnownSite {
        kind: "authority-read@prune::read_bounded",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/prune.rs read_bounded (authority_bind_descriptor)",
    },
    KnownSite {
        kind: "read-bounded:lstat",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/prune.rs read_bounded",
    },
    KnownSite {
        kind: "read-bounded:open",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/prune.rs read_bounded, and authority_read_observed for \
that site (read_row_handle)",
    },
    // The prune marker read opens the marker with `openat(root.as_raw_fd())`
    // from a live held root descriptor, so its parent is handle-class.
    KnownSite {
        kind: "authority-read@prune::observe_root_in:marker",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs observe_root_in (openat from the held root)",
    },
    KnownSite {
        kind: "root-observe:marker-open",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs observe_row_root (openat from the held root)",
    },
    KnownSite {
        kind: "authority-read@outputs::read_receipt_with",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/outputs.rs read_receipt_with (authority_read_observed)",
    },
    KnownSite {
        kind: "authority-read@outputs::read_marker",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/outputs.rs read_marker (File::open, then authority_read_observed)",
    },
    KnownSite {
        kind: "outputs:read-marker:open",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/outputs.rs observed_marker_open (read_row_dev)",
    },
    KnownSite {
        kind: "authority-read@outputs::marker_digest",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/outputs.rs marker_digest (authority_read_observed)",
    },
    KnownSite {
        kind: "authority-read@removable::read_registration",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/removable.rs read_registration (authority_read_observed)",
    },
    // The absence walk: its start object is a path-only observation, and every
    // component row below it comes from a held parent descriptor.
    KnownSite {
        kind: "absence-walk:start",
        class: SiteClass::PathOnlyDirectRead,
        source: "src/bin/oxide/prune.rs walk_checkout (walk_start_row)",
    },
    KnownSite {
        kind: "absence-walk:fstatat",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs walk_checkout (component_row)",
    },
    KnownSite {
        kind: "absence-walk:openat",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs walk_checkout (component_row)",
    },
    KnownSite {
        kind: "absence-walk:boot-data",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs walk_checkout (component_row)",
    },
    KnownSite {
        kind: "absence-walk:revalidate-descriptor",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs revalidate_chain (component_without_parent)",
    },
    KnownSite {
        kind: "absence-walk:revalidate-name",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs revalidate_chain (component_without_parent)",
    },
    // The volume-binding observations. Both the absence walk and the root
    // observation record them through `volume_row`, whose identity is the
    // volume form (mount, UUID, device), so their parent provenance is a
    // declared dynamic observation rather than a path or a handle.
    KnownSite {
        kind: "absence-walk:binding-open",
        class: SiteClass::DeclaredObservation,
        source: "src/bin/oxide/prune.rs checkout_absence -> volume::Binding::open (volume_row)",
    },
    KnownSite {
        kind: "absence-walk:binding-verify",
        class: SiteClass::DeclaredObservation,
        source: "src/bin/oxide/prune.rs checkout_absence volume verification (volume_row)",
    },
    KnownSite {
        kind: "root-observe:binding-open",
        class: SiteClass::DeclaredObservation,
        source: "src/bin/oxide/prune.rs observe_row_root -> volume::Binding::open (volume_row)",
    },
    KnownSite {
        kind: "root-observe:entry",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs observe_row_root listing (component_row)",
    },
    KnownSite {
        kind: "root-observe:root-dir",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs observe_row_root (held root descriptor)",
    },
    KnownSite {
        kind: "root-observe:outputs-dir",
        class: SiteClass::HandleParent,
        source: "src/bin/oxide/prune.rs observe_row_root (held outputs descriptor)",
    },
    KnownSite {
        kind: "volume-site",
        class: SiteClass::DeclaredObservation,
        source: "src/volume.rs mount census",
    },
    KnownSite {
        kind: "fixture-diff",
        class: SiteClass::DeclaredObservation,
        source: "src/bin/oxide/tape.rs fixture diff",
    },
    KnownSite {
        kind: "authority-read@store::load",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs Store::load",
    },
    KnownSite {
        kind: "authority-read@store::active_holders",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs active_holders",
    },
    KnownSite {
        kind: "authority-read@store::recorded_lost",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs recorded_lost",
    },
    KnownSite {
        kind: "authority-read@store::recorded_last_use",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs recorded_last_use",
    },
    KnownSite {
        kind: "authority-read@store::sibling_state",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs sibling_state",
    },
    KnownSite {
        kind: "authority-read@store::sibling_state_checked",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs sibling_state_checked",
    },
    KnownSite {
        kind: "authority-read@store::trusted_origin",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs trusted_origin",
    },
    KnownSite {
        kind: "authority-read@store::read_observations",
        class: SiteClass::Custody,
        source: "src/bin/oxide/store.rs read_observations",
    },
];

fn known_site(kind: &str) -> Option<&'static KnownSite> {
    KNOWN_SITES.iter().find(|site| site.kind == kind)
}

// ---------------------------------------------------------------------------
// Blocking reasons
// ---------------------------------------------------------------------------

/// Every blocking reason is structured: rule, site (or empty), detail. The
/// limited verdict never reports a bare string, so no caller can filter a
/// blocking reason by matching warning text.
/// One macro, one source of truth: the enum and its full `ALL_BLOCKING_RULES`
/// list are generated together, so the vocabulary the plan prints cannot omit a
/// variant the verdict can return. `name()` stays an exhaustive match, so a new
/// variant cannot compile without its printed name.
macro_rules! blocking_rules {
    ($($variant:ident),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
        pub(crate) enum BlockingRule {
            $($variant,)*
        }

        /// Every blocking rule, in the enum's own order.
        pub(crate) const ALL_BLOCKING_RULES: &[BlockingRule] = &[$(BlockingRule::$variant,)*];
    };
}

blocking_rules! {
    // Caps, structure and window.
    Truncation,
    FixtureCaptureTruncated,
    WindowValueNotNull,
    MissingMandatoryKey,
    UnknownNote,
    // Observations that bind nothing or only partly.
    ExtraUnboundObservation,
    UnboundObjectIdentity,
    UnboundComponentParent,
    UnboundAuthorityRead,
    UnboundDescriptorIdentity,
    UnboundDescriptorProcess,
    AuthorityBudgetFailed,
    EngineCounterfactual,
    AuthorityDeadlineUnproven,
    StaleObservation,
    // Obligations.
    UnlistedSite,
    MissingMandatoryObservation,
    WrongLeafIdentity,
    HandleParentUnqualified,
    ParentRequiredUnproven,
    // The recorder's own obligation sentences: `mandatory observation missing or
    // identity unproven at <site>` and `obligation missing parent_required at
    // <site>`.
    RecorderObligationUnmet,
    RecorderObligationMalformed,
    // Clocks and holders.
    UnobservedDecisionClock,
    UnprovenHolder,
    // Effects.
    AttemptedEffect,
    UncompletedEffect,
    UnpathedEffect,
    UnknownEffectKind,
    DeclaredAttempt,
    OmittedEffect,
    ExtraEffect,
    SubstitutedEffect,
    FixtureChangeUnmatched,
    // Drift.
    CaseDrift,
    DigestDrift,
    SourceDrift,
    TopologyDrift,
    // Strict preservation.
    StrictIssueUnexplained,
}

impl BlockingRule {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Truncation => "truncation",
            Self::FixtureCaptureTruncated => "fixture-capture-truncated",
            Self::WindowValueNotNull => "window-value-not-null",
            Self::MissingMandatoryKey => "missing-mandatory-key",
            Self::UnknownNote => "unknown-engine-note",
            Self::ExtraUnboundObservation => "extra-unbound-observation",
            Self::UnboundObjectIdentity => "unbound-object-identity",
            Self::UnboundComponentParent => "unbound-component-parent",
            Self::UnboundAuthorityRead => "unbound-authority-read",
            Self::UnboundDescriptorIdentity => "unbound-descriptor-identity",
            Self::UnboundDescriptorProcess => "unbound-descriptor-process",
            Self::AuthorityBudgetFailed => "authority-budget-failed",
            Self::EngineCounterfactual => "engine-counterfactual",
            Self::AuthorityDeadlineUnproven => "authority-deadline-unproven",
            Self::StaleObservation => "stale-observation",
            Self::UnlistedSite => "unlisted-site",
            Self::MissingMandatoryObservation => "missing-mandatory-observation",
            Self::WrongLeafIdentity => "wrong-leaf-identity",
            Self::HandleParentUnqualified => "handle-parent-k1-unqualified",
            Self::ParentRequiredUnproven => "parent-required-unproven",
            Self::RecorderObligationUnmet => "recorder-obligation-unmet",
            Self::RecorderObligationMalformed => "recorder-obligation-malformed",
            Self::UnobservedDecisionClock => "unobserved-decision-clock",
            Self::UnprovenHolder => "unproven-recorded-holder",
            Self::AttemptedEffect => "attempted-effect",
            Self::UncompletedEffect => "uncompleted-effect",
            Self::UnpathedEffect => "unpathed-effect",
            Self::UnknownEffectKind => "unknown-effect-kind",
            Self::DeclaredAttempt => "declared-attempt",
            Self::OmittedEffect => "omitted-effect",
            Self::ExtraEffect => "extra-effect",
            Self::SubstitutedEffect => "substituted-effect",
            Self::FixtureChangeUnmatched => "fixture-change-unmatched",
            Self::CaseDrift => "case-drift",
            Self::DigestDrift => "digest-drift",
            Self::SourceDrift => "source-drift",
            Self::TopologyDrift => "topology-drift",
            Self::StrictIssueUnexplained => "strict-issue-unexplained",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Blocking {
    pub(crate) rule: BlockingRule,
    pub(crate) site: String,
    pub(crate) detail: String,
}

impl Blocking {
    fn new(rule: BlockingRule, site: &str, detail: impl Into<String>) -> Self {
        Self {
            rule,
            site: site.to_owned(),
            detail: detail.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Engine notes: a closed vocabulary
// ---------------------------------------------------------------------------

/// How one of the recorder's own fixed notes is treated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteRule {
    /// Blocking, with the note's own trailing detail.
    Blocking(BlockingRule),
    /// Explained only by the null-window limitation.
    WindowNullExplained,
    /// Explained only by a path-only parent limitation on the note's site.
    PathOnlyParentExplained,
}

/// The recorder's fixed note vocabulary (`src/bin/oxide/tape.rs`). Any note
/// that is not a prefix match here is `UnknownNote` and blocking.
const NOTE_VOCABULARY: &[(&str, NoteRule)] = &[
    (
        "absolute monotonic window origin and end unproven",
        NoteRule::WindowNullExplained,
    ),
    (
        "parent identity unproven at ",
        NoteRule::PathOnlyParentExplained,
    ),
    (
        "mandatory observation missing or identity unproven at ",
        NoteRule::Blocking(BlockingRule::RecorderObligationUnmet),
    ),
    (
        "obligation missing parent_required at ",
        NoteRule::Blocking(BlockingRule::RecorderObligationMalformed),
    ),
    (
        "extra or unbound observation at ",
        NoteRule::Blocking(BlockingRule::ExtraUnboundObservation),
    ),
    (
        "object or parent identity unproven at ",
        NoteRule::Blocking(BlockingRule::UnboundObjectIdentity),
    ),
    (
        "component parent identity unproven at ",
        NoteRule::Blocking(BlockingRule::UnboundComponentParent),
    ),
    (
        "descriptor-only effect at ",
        NoteRule::Blocking(BlockingRule::UnpathedEffect),
    ),
    (
        "unknown effect kind at ",
        NoteRule::Blocking(BlockingRule::UnknownEffectKind),
    ),
    (
        "effect path at ",
        NoteRule::Blocking(BlockingRule::UnpathedEffect),
    ),
    (
        "attempted effect at ",
        NoteRule::Blocking(BlockingRule::AttemptedEffect),
    ),
    (
        "monotonic reading before window at ",
        NoteRule::Blocking(BlockingRule::StaleObservation),
    ),
    (
        "recorded holder at ",
        NoteRule::Blocking(BlockingRule::UnprovenHolder),
    ),
    (
        "the authority budget failed",
        NoteRule::Blocking(BlockingRule::AuthorityBudgetFailed),
    ),
    (
        "authority-bind at ",
        NoteRule::Blocking(BlockingRule::UnboundAuthorityRead),
    ),
    (
        "descriptor and parent identity unproven at ",
        NoteRule::Blocking(BlockingRule::UnboundDescriptorIdentity),
    ),
    (
        "descriptor identity unavailable at ",
        NoteRule::Blocking(BlockingRule::UnboundDescriptorIdentity),
    ),
    (
        "descriptor path, parent and process-start unproven at ",
        NoteRule::Blocking(BlockingRule::UnboundDescriptorProcess),
    ),
    (
        "counterfactual: pool lock routed away",
        NoteRule::Blocking(BlockingRule::EngineCounterfactual),
    ),
    (
        "counterfactual diverted pool-lock observations",
        NoteRule::Blocking(BlockingRule::EngineCounterfactual),
    ),
    (
        "authority monotonic deadline identity unproven",
        NoteRule::Blocking(BlockingRule::AuthorityDeadlineUnproven),
    ),
    (
        "dup of held descriptor failed",
        NoteRule::Blocking(BlockingRule::UnboundDescriptorIdentity),
    ),
    (
        "descriptor identity unproven at observation site",
        NoteRule::Blocking(BlockingRule::UnboundDescriptorIdentity),
    ),
    (
        "fixture capture truncated",
        NoteRule::Blocking(BlockingRule::FixtureCaptureTruncated),
    ),
    (
        "fixture change lacks a matching observed effect",
        NoteRule::Blocking(BlockingRule::FixtureChangeUnmatched),
    ),
    (
        "an authority read ended its window without its caller binding its identity",
        NoteRule::Blocking(BlockingRule::UnboundAuthorityRead),
    ),
    (
        "an authority read was replaced before identity binding",
        NoteRule::Blocking(BlockingRule::UnboundAuthorityRead),
    ),
    (
        "effect not completed at ",
        NoteRule::Blocking(BlockingRule::UncompletedEffect),
    ),
    (
        "effect identity unproven at ",
        NoteRule::Blocking(BlockingRule::UnpathedEffect),
    ),
    (
        "declared attempt is not completed parity at ",
        NoteRule::Blocking(BlockingRule::DeclaredAttempt),
    ),
    (
        "declared effect lacks attempted at ",
        NoteRule::Blocking(BlockingRule::DeclaredAttempt),
    ),
    (
        "omitted or extra effects",
        NoteRule::Blocking(BlockingRule::OmittedEffect),
    ),
    (
        "substituted effects",
        NoteRule::Blocking(BlockingRule::SubstitutedEffect),
    ),
];

/// The strict issues the review permits the limited verdict to explain. Both
/// templates are the shared completeness module's own fixed strings; anything
/// else is `StrictIssueUnexplained`.
const STRICT_WINDOW_ISSUE: &str = "window origin and end unproven";
const STRICT_PARENT_ISSUE_PREFIX: &str = "parent_required unproven at ";
const STRICT_PARENT_ISSUE_SUFFIX: &str = ": handle parent still needs qualified K1";

// ---------------------------------------------------------------------------
// Verdict
// ---------------------------------------------------------------------------

/// The strict `/2` result, retained verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StrictResult {
    pub(crate) complete: bool,
    pub(crate) issues: Vec<String>,
}

/// Per-site evidence. Every obligation the manifest names gets one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SiteEvidence {
    pub(crate) site: String,
    pub(crate) class: String,
    pub(crate) parent_required: bool,
    pub(crate) bound: bool,
    pub(crate) leaf_identity: String,
    pub(crate) disposition: &'static str,
}

/// One limited verdict for one engine's transcript against one manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LimitedVerdict {
    pub(crate) policy: &'static str,
    pub(crate) claim: &'static str,
    pub(crate) case: String,
    pub(crate) engine: String,
    pub(crate) strict: StrictResult,
    pub(crate) blocking: Vec<Blocking>,
    pub(crate) limitations: Vec<Limitation>,
    pub(crate) per_site: Vec<SiteEvidence>,
}

impl LimitedVerdict {
    /// The separate limited result. Never a substitute for the strict result.
    pub(crate) fn sufficient(&self) -> bool {
        self.blocking.is_empty()
    }

    pub(crate) fn blocking_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .blocking
            .iter()
            .map(|blocking| blocking.rule.name())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// The verdict as one bounded document. Used by the plan and by callers
    /// that retain a per-case record; it is never an acceptance gate.
    pub(crate) fn to_document(&self) -> Value {
        json!({
            "policy": self.policy,
            "claim": self.claim,
            "case": self.case,
            "engine": self.engine,
            "sufficient": self.sufficient(),
            "strict": {
                "complete": self.strict.complete,
                "issues": self.strict.issues,
            },
            "blocking": self
                .blocking
                .iter()
                .map(|blocking| json!({
                    "rule": blocking.rule.name(),
                    "site": blocking.site,
                    "detail": blocking.detail,
                }))
                .collect::<Vec<_>>(),
            "limitations": self
                .limitations
                .iter()
                .map(|limitation| json!({
                    "rule": limitation.rule.name(),
                    "reason": limitation.rule.reason(),
                    "site": limitation.site,
                    "leaf_identity": limitation.leaf_identity,
                    "obligation": limitation.obligation,
                }))
                .collect::<Vec<_>>(),
            "sites": self
                .per_site
                .iter()
                .map(|site| json!({
                    "site": site.site,
                    "class": site.class,
                    "parent_required": site.parent_required,
                    "bound": site.bound,
                    "leaf_identity": site.leaf_identity,
                    "disposition": site.disposition,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// A local SHA-256, so this module needs no new visibility on `capture_wire`'s
/// own helpers.
fn digest_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parent_source_of(row: &Value) -> String {
    row.pointer("/identity/parent_source")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_owned()
}

/// The leaf identity a row actually binds, as one bounded string. Own device
/// and inode come from the leaf; a digest outcome also binds the bytes.
fn leaf_identity(row: &Value) -> String {
    let identity = row.get("identity");
    let part = |key: &str| -> String {
        match identity.and_then(|identity| identity.get(key)) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            _ => "null".to_owned(),
        }
    };
    let path = match identity.and_then(|identity| identity.get("path")) {
        Some(Value::String(text)) => format!("path={text}"),
        _ => match identity.and_then(|identity| identity.get("component")) {
            Some(Value::String(text)) => format!("component={text}"),
            _ => "path=null".to_owned(),
        },
    };
    format!(
        "{path} device={} inode={} outcome={}",
        part("device"),
        part("inode"),
        row_outcome(row)
    )
}

fn row_outcome(row: &Value) -> String {
    row.get("outcome")
        .map(ToString::to_string)
        .unwrap_or_else(|| "null".to_owned())
}

/// Whether a row binds its own object: both device and inode are present.
fn own_identity_bound(row: &Value) -> bool {
    let identity = match row.get("identity") {
        Some(identity) => identity,
        None => return false,
    };
    identity.get("device").is_some_and(|value| !value.is_null())
        && identity.get("inode").is_some_and(|value| !value.is_null())
}

/// Evaluate one capture pair under the limited policy.
///
/// This is the entry point a case body uses. Both documents go through the
/// canonical `/2` parsers first (`capture_wire::parse_transcript` and
/// `capture_wire::parse_manifest`), so schema, version, cap and envelope
/// refusals are the wire's own and never the policy's guess. The manifest
/// digest is computed from the exact bytes that were parsed, so it always
/// matches what the transcript recorded.
///
/// A parser refusal is returned as the error string; it is never a verdict, so
/// a malformed input can never read as a limited result.
pub(crate) fn evaluate_limited_capture(
    transcript_bytes: &[u8],
    manifest_bytes: &[u8],
) -> Result<LimitedVerdict, String> {
    let transcript = capture_wire::parse_transcript(transcript_bytes)
        .map_err(|error| format!("transcript: {}", error.message))?;
    let manifest = capture_wire::parse_manifest(manifest_bytes)
        .map_err(|error| format!("manifest: {}", error.message))?;
    let digest = digest_hex(manifest_bytes);
    Ok(evaluate_limited(&transcript, &manifest, &digest))
}

/// Evaluate one transcript against its manifest under the limited policy.
///
/// `transcript` and `manifest` must already be parser-validated `/2`
/// documents; [`evaluate_limited_capture`] is the entry point that guarantees
/// it. `manifest_sha256` is the digest of the exact manifest bytes the caller
/// read, and must equal the digest the transcript recorded, or the verdict
/// records `digest-drift`.
pub(crate) fn evaluate_limited(
    transcript: &Value,
    manifest: &Value,
    manifest_sha256: &str,
) -> LimitedVerdict {
    let strict = capture_wire::assess_completeness(transcript, Some(manifest));
    let strict_result = StrictResult {
        complete: strict.issues.is_empty(),
        issues: strict.issues.clone(),
    };
    let mut blocking: Vec<Blocking> = Vec::new();
    let mut limitations: Vec<Limitation> = Vec::new();
    let mut per_site: Vec<SiteEvidence> = Vec::new();

    let engine = transcript
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let case = transcript
        .get("case")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if engine.is_empty() {
        blocking.push(Blocking::new(
            BlockingRule::MissingMandatoryKey,
            "",
            "transcript engine",
        ));
    }
    if case.is_empty() {
        blocking.push(Blocking::new(
            BlockingRule::MissingMandatoryKey,
            "",
            "transcript case",
        ));
    }
    if transcript.get("outcome").is_none() {
        blocking.push(Blocking::new(
            BlockingRule::MissingMandatoryKey,
            "",
            "transcript outcome",
        ));
    }

    // -- caps and truncation ------------------------------------------------
    for flag in [
        "read_rows_truncated",
        "occurrences_truncated",
        "document_truncated",
        "process_rows_truncated",
        "classifications_truncated",
    ] {
        if transcript
            .pointer(&format!("/truncation/{flag}"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            blocking.push(Blocking::new(BlockingRule::Truncation, "", flag));
        }
    }
    if transcript
        .pointer("/fixture_capture/truncated")
        .and_then(Value::as_bool)
        == Some(true)
    {
        blocking.push(Blocking::new(
            BlockingRule::FixtureCaptureTruncated,
            "fixture_capture",
            "fixture capture exceeded its entry or byte bound",
        ));
    }

    // -- the one clock-adjacent exclusion -----------------------------------
    let start = transcript.get("window_monotonic_start_ns");
    let end = transcript.get("window_monotonic_end_ns");
    if start.is_none() || end.is_none() {
        blocking.push(Blocking::new(
            BlockingRule::MissingMandatoryKey,
            "window",
            "window origin or end field",
        ));
    }
    let window_null = start.is_some_and(Value::is_null) && end.is_some_and(Value::is_null);
    if window_null {
        limitations.push(Limitation {
            rule: ExclusionRule::WindowOriginEndNull,
            site: "transcript.window".to_owned(),
            leaf_identity: "window_monotonic_start_ns=null window_monotonic_end_ns=null".to_owned(),
            obligation: STRICT_WINDOW_ISSUE.to_owned(),
        });
    } else {
        blocking.push(Blocking::new(
            BlockingRule::WindowValueNotNull,
            "window",
            "the /2 wire requires a null window origin and end",
        ));
    }

    // -- clocks the decision could read -------------------------------------
    for clock in transcript
        .get("clocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if clock.get("value").is_some_and(Value::is_null) {
            let kind = clock.get("kind").and_then(Value::as_str).unwrap_or("");
            blocking.push(Blocking::new(
                BlockingRule::UnobservedDecisionClock,
                kind,
                "an unobserved clock is never decision evidence",
            ));
        }
    }

    // -- recorded holders ---------------------------------------------------
    for holder in transcript
        .get("recorded_holders")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let site = holder.get("site").and_then(Value::as_str).unwrap_or("");
        let start_null = holder
            .get("kernel_start")
            .map(Value::is_null)
            .unwrap_or(true);
        let uid_null = holder.get("uid").map(Value::is_null).unwrap_or(true);
        if start_null || uid_null {
            blocking.push(Blocking::new(
                BlockingRule::UnprovenHolder,
                site,
                "a recorded holder without an observed kernel start and uid cannot feed a \
decision",
            ));
        }
    }

    // -- effects ------------------------------------------------------------
    let observed: Vec<&Value> = transcript
        .get("effects")
        .and_then(Value::as_array)
        .map(|effects| effects.iter().collect())
        .unwrap_or_default();
    let mut completed: Vec<(String, String, String)> = Vec::new();
    for effect in &observed {
        let site = effect.get("site").and_then(Value::as_str).unwrap_or("");
        let result = effect.get("result").and_then(Value::as_str).unwrap_or("");
        let attempted = effect
            .get("attempted")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let path = effect.get("path").and_then(Value::as_str);
        if attempted {
            blocking.push(Blocking::new(
                BlockingRule::AttemptedEffect,
                site,
                format!("attempted effect with result {result}"),
            ));
        } else if result != "succeeded" {
            blocking.push(Blocking::new(
                BlockingRule::UncompletedEffect,
                site,
                format!("observed result {result}"),
            ));
        } else if path.is_none() {
            blocking.push(Blocking::new(
                BlockingRule::UnpathedEffect,
                site,
                "a descriptor-only effect has no path identity",
            ));
        }
        if capture_wire::completed_effect(attempted, result, path) {
            completed.push((
                effect
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                site.to_owned(),
                path.unwrap_or("").to_owned(),
            ));
        }
    }
    let declared: Vec<&Value> = manifest
        .get("expected_effects")
        .and_then(Value::as_array)
        .map(|effects| effects.iter().collect())
        .unwrap_or_default();
    let mut wanted: Vec<(String, String, String)> = Vec::new();
    for effect in &declared {
        let site = effect.get("site").and_then(Value::as_str).unwrap_or("");
        match effect.get("attempted").and_then(Value::as_bool) {
            Some(false) => wanted.push((
                effect
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                site.to_owned(),
                effect
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            )),
            Some(true) => blocking.push(Blocking::new(
                BlockingRule::DeclaredAttempt,
                site,
                "a declared attempt is never completed parity",
            )),
            None => blocking.push(Blocking::new(
                BlockingRule::MissingMandatoryKey,
                site,
                "declared effect lacks attempted",
            )),
        }
    }
    // Effect multiplicity is preserved: every completed observation consumes
    // at most one declaration, and every surplus or shortfall is named.
    let mut remaining = completed.clone();
    for want in &wanted {
        match remaining.iter().position(|got| got == want) {
            Some(index) => {
                remaining.remove(index);
            }
            None if remaining
                .iter()
                .any(|got| got.0 == want.0 && got.1 == want.1) =>
            {
                blocking.push(Blocking::new(
                    BlockingRule::SubstitutedEffect,
                    &want.1,
                    format!(
                        "declared {} at {} has no completed observation with that path",
                        want.0, want.1
                    ),
                ));
            }
            None => blocking.push(Blocking::new(
                BlockingRule::OmittedEffect,
                &want.1,
                format!("declared {} at {} was not completed", want.0, want.1),
            )),
        }
    }
    for extra in &remaining {
        blocking.push(Blocking::new(
            BlockingRule::ExtraEffect,
            &extra.1,
            format!("completed {} at {} is not declared", extra.0, extra.1),
        ));
    }

    // -- obligations --------------------------------------------------------
    let reads: Vec<&Value> = transcript
        .get("reads")
        .and_then(Value::as_array)
        .map(|reads| reads.iter().collect())
        .unwrap_or_default();
    let window = manifest.pointer("/window/identity").and_then(Value::as_str);
    let obligations: Vec<&Value> = manifest
        .get("mandatory_observations")
        .and_then(Value::as_array)
        .map(|obligations| obligations.iter().collect())
        .unwrap_or_default();
    // The recorder's own bind verdict, read structurally from its fixed note
    // vocabulary. A row it reports unbound is never granted an exclusion, so
    // the policy cannot outrun the engine's own binding.
    const UNBOUND_NOTE: &str = "extra or unbound observation at ";
    let mut unbound_sites: Vec<String> = Vec::new();
    for note in transcript
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(text) = note.as_str() else {
            continue;
        };
        if let Some(site) = text.strip_prefix(UNBOUND_NOTE) {
            unbound_sites.push(site.to_owned());
        }
    }
    let mut obligation_sites: Vec<(String, bool)> = Vec::new();
    for obligation in &obligations {
        if obligation.get("engine").and_then(Value::as_str) != Some(engine.as_str()) {
            continue;
        }
        let site = obligation.get("site").and_then(Value::as_str).unwrap_or("");
        obligation_sites.push((
            site.to_owned(),
            obligation
                .get("parent_required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ));
        if site.is_empty() {
            blocking.push(Blocking::new(
                BlockingRule::MissingMandatoryKey,
                "",
                "obligation site",
            ));
            continue;
        }
        let Some(known) = known_site(site) else {
            blocking.push(Blocking::new(
                BlockingRule::UnlistedSite,
                site,
                "the policy knows no read class for this site",
            ));
            per_site.push(SiteEvidence {
                site: site.to_owned(),
                class: "unlisted".to_owned(),
                parent_required: obligation
                    .get("parent_required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                bound: false,
                leaf_identity: String::new(),
                disposition: "unlisted-site",
            });
            continue;
        };
        let parent_required = match obligation.get("parent_required") {
            Some(Value::Bool(value)) => *value,
            _ => {
                blocking.push(Blocking::new(
                    BlockingRule::MissingMandatoryKey,
                    site,
                    "obligation missing parent_required",
                ));
                false
            }
        };
        let identity = match obligation.get("identity") {
            Some(identity) if identity.is_object() => identity,
            _ => {
                blocking.push(Blocking::new(
                    BlockingRule::MissingMandatoryKey,
                    site,
                    "obligation missing identity",
                ));
                continue;
            }
        };
        let at_site: Vec<&&Value> = reads
            .iter()
            .filter(|row| row.get("site").and_then(Value::as_str) == Some(site))
            .collect();
        if at_site.is_empty() {
            blocking.push(Blocking::new(
                BlockingRule::MissingMandatoryObservation,
                site,
                "no read row at this site",
            ));
            per_site.push(SiteEvidence {
                site: site.to_owned(),
                class: known.class.name().to_owned(),
                parent_required,
                bound: false,
                leaf_identity: String::new(),
                disposition: "missing-row",
            });
            continue;
        }
        let matching: Vec<&&Value> = at_site
            .iter()
            .copied()
            .filter(|row| capture_wire::identity_fields_match(identity, &row["identity"], window))
            .collect();
        if matching.is_empty() {
            blocking.push(Blocking::new(
                BlockingRule::WrongLeafIdentity,
                site,
                "no row at this site matches every named identity key",
            ));
            per_site.push(SiteEvidence {
                site: site.to_owned(),
                class: known.class.name().to_owned(),
                parent_required,
                bound: false,
                leaf_identity: leaf_identity(at_site[0]),
                disposition: "wrong-identity",
            });
            continue;
        }
        let row = matching[0];
        let parent_source = parent_source_of(row);
        let bound_own = own_identity_bound(row);
        let mut disposition = "bound";
        if parent_required {
            if parent_source == "handle" {
                // The strict rule discharges a handle parent exactly when the
                // K1 premise is qualified; the policy mirrors that rule rather
                // than blocking unconditionally.
                if capture_wire::K1_PREMISE_QUALIFIED {
                    disposition = "handle-parent-qualified";
                } else {
                    blocking.push(Blocking::new(
                        BlockingRule::HandleParentUnqualified,
                        site,
                        "a handle-class parent is never excludable while K1 is unqualified",
                    ));
                    disposition = "handle-parent-k1-unqualified";
                }
            } else if known.class == SiteClass::PathOnlyDirectRead
                && bound_own
                && !unbound_sites.iter().any(|unbound| unbound == site)
            {
                let leaf = leaf_identity(row);
                limitations.push(Limitation {
                    rule: ExclusionRule::PathOnlyDirectReadParent,
                    site: site.to_owned(),
                    leaf_identity: leaf.clone(),
                    obligation: format!(
                        "{STRICT_PARENT_ISSUE_PREFIX}{site}{STRICT_PARENT_ISSUE_SUFFIX}"
                    ),
                });
                disposition = "path-only-parent-limitation";
            } else if unbound_sites.iter().any(|unbound| unbound == site) {
                blocking.push(Blocking::new(
                    BlockingRule::ParentRequiredUnproven,
                    site,
                    "the recorder reports this row unbound, so no reviewed limitation applies",
                ));
                disposition = "row-unbound-by-recorder";
            } else {
                blocking.push(Blocking::new(
                    BlockingRule::ParentRequiredUnproven,
                    site,
                    format!(
                        "class {} with own identity bound={bound_own} cannot discharge \
parent_required",
                        known.class.name()
                    ),
                ));
                disposition = "parent-required-unproven";
            }
        }
        // Only a file-shaped obligation names the leaf's own object. A
        // component obligation names a name plus its parent, and a volume
        // obligation names the volume facts; neither requires an own inode.
        let volume_form = identity
            .get("volume_uuid")
            .is_some_and(|value| !value.is_null());
        let file_form = !volume_form && identity.get("inode").is_some();
        if file_form && !bound_own {
            blocking.push(Blocking::new(
                BlockingRule::UnboundObjectIdentity,
                site,
                "the row binds no own device and inode",
            ));
            disposition = "own-identity-unbound";
        }
        per_site.push(SiteEvidence {
            site: site.to_owned(),
            class: known.class.name().to_owned(),
            parent_required,
            bound: bound_own,
            leaf_identity: leaf_identity(row),
            disposition,
        });
    }

    // -- engine notes: a closed vocabulary -----------------------------------
    let limited_sites: Vec<String> = limitations
        .iter()
        .filter(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent)
        .map(|limitation| limitation.site.clone())
        .collect();
    for note in transcript
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(text) = note.as_str() else {
            blocking.push(Blocking::new(
                BlockingRule::UnknownNote,
                "",
                "a non-string engine note cannot be classified",
            ));
            continue;
        };
        let Some((prefix, rule)) = NOTE_VOCABULARY
            .iter()
            .find(|(prefix, _)| text.starts_with(prefix))
        else {
            blocking.push(Blocking::new(
                BlockingRule::UnknownNote,
                "",
                text.to_owned(),
            ));
            continue;
        };
        let detail = &text[prefix.len()..];
        match rule {
            NoteRule::WindowNullExplained => {
                if !window_null {
                    blocking.push(Blocking::new(
                        BlockingRule::WindowValueNotNull,
                        "window",
                        text.to_owned(),
                    ));
                }
            }
            NoteRule::PathOnlyParentExplained => {
                // The sentence names the row kind, which is the site. It is
                // explained by a granted limitation; when no declared decision
                // input requires that parent there is no obligation to exclude.
                let asks_parent = obligation_sites
                    .iter()
                    .any(|(site, required)| site == detail && *required);
                if asks_parent && !limited_sites.iter().any(|site| site == detail) {
                    blocking.push(Blocking::new(
                        BlockingRule::ParentRequiredUnproven,
                        detail,
                        "the parent stays unproven and no reviewed limitation covers this site",
                    ));
                }
            }
            NoteRule::Blocking(rule) => {
                // The two identity sentences say that a leaf's own object or a
                // component parent was not observed. The recorder's own bind
                // verdict (`extra or unbound observation`) decides whether the
                // row binds the fixture at all, so these block only where the
                // row is a declared decision input, or where the sentence is
                // about a descriptor or authority read that no reviewed
                // exclusion can ever cover.
                let declares_input = obligation_sites.iter().any(|(site, _)| site == detail);
                let leaf_identity_note = matches!(
                    rule,
                    BlockingRule::UnboundObjectIdentity | BlockingRule::UnboundComponentParent
                );
                if leaf_identity_note && !declares_input {
                    continue;
                }
                blocking.push(Blocking::new(*rule, detail, text.to_owned()));
            }
        }
    }

    // -- drift --------------------------------------------------------------
    if transcript.get("case").and_then(Value::as_str)
        != manifest.get("case").and_then(Value::as_str)
    {
        blocking.push(Blocking::new(
            BlockingRule::CaseDrift,
            "",
            "transcript and manifest name different cases",
        ));
    }
    let recorded_digest = transcript
        .get("manifest_sha256")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !is_lower_hex(manifest_sha256, 64) {
        blocking.push(Blocking::new(
            BlockingRule::DigestDrift,
            "",
            "the supplied manifest digest is not a SHA-256",
        ));
    } else if !is_lower_hex(recorded_digest, 64) || recorded_digest != manifest_sha256 {
        blocking.push(Blocking::new(
            BlockingRule::DigestDrift,
            "",
            "the transcript's manifest digest is not the supplied manifest's digest",
        ));
    }
    let source = manifest.get("source");
    match source {
        Some(source) if source.is_object() => {
            if source.get("barge_pin").and_then(Value::as_str) != Some(SCHEMA_BARGE_PIN) {
                blocking.push(Blocking::new(
                    BlockingRule::SourceDrift,
                    "",
                    "the /2 manifest schema fixes the recorded Barge pin",
                ));
            }
            if !source
                .get("oxide_tree")
                .and_then(Value::as_str)
                .is_some_and(|tree| is_lower_hex(tree, 40))
            {
                blocking.push(Blocking::new(
                    BlockingRule::SourceDrift,
                    "",
                    "source.oxide_tree is not an exact 40-hex tree identity",
                ));
            }
            if !source
                .get("barge_slice_commit")
                .and_then(Value::as_str)
                .is_some_and(|commit| !commit.is_empty())
            {
                blocking.push(Blocking::new(
                    BlockingRule::SourceDrift,
                    "",
                    "source.barge_slice_commit is empty",
                ));
            }
        }
        _ => blocking.push(Blocking::new(
            BlockingRule::SourceDrift,
            "",
            "the manifest carries no source object",
        )),
    }
    let fixture_root = manifest
        .pointer("/fixture/root")
        .and_then(Value::as_str)
        .unwrap_or("");
    if fixture_root.is_empty() {
        blocking.push(Blocking::new(
            BlockingRule::TopologyDrift,
            "fixture",
            "fixture root is missing",
        ));
    }
    let fixture_roots: Vec<&str> = manifest
        .pointer("/fixture/roots")
        .and_then(Value::as_array)
        .map(|roots| roots.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let expected_roots: Vec<&str> = manifest
        .pointer("/expected/roots")
        .and_then(Value::as_array)
        .map(|roots| {
            roots
                .iter()
                .filter_map(|root| root.get("root").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let mut declared_roots = fixture_roots.clone();
    declared_roots.sort_unstable();
    declared_roots.dedup();
    if declared_roots.len() != fixture_roots.len() {
        blocking.push(Blocking::new(
            BlockingRule::TopologyDrift,
            "fixture",
            "the fixture declares a root twice",
        ));
    }
    let mut expected_sorted = expected_roots.clone();
    expected_sorted.sort_unstable();
    expected_sorted.dedup();
    if expected_sorted != declared_roots {
        blocking.push(Blocking::new(
            BlockingRule::TopologyDrift,
            "expected",
            "declared fixture roots and expected roots differ",
        ));
    }
    for obligation in &obligations {
        let site = obligation.get("site").and_then(Value::as_str).unwrap_or("");
        let Some(path) = obligation.pointer("/identity/path").and_then(Value::as_str) else {
            continue;
        };
        if !fixture_root.is_empty()
            && !(path == fixture_root || path.starts_with(&format!("{fixture_root}/")))
        {
            blocking.push(Blocking::new(
                BlockingRule::TopologyDrift,
                site,
                "the obligation names a path outside the fixture root",
            ));
        }
    }

    // -- strict preservation -------------------------------------------------
    for issue in &strict.issues {
        if issue == STRICT_WINDOW_ISSUE && window_null {
            continue;
        }
        if let Some(site) = issue
            .strip_prefix(STRICT_PARENT_ISSUE_PREFIX)
            .and_then(|rest| rest.strip_suffix(STRICT_PARENT_ISSUE_SUFFIX))
        {
            if limited_sites.iter().any(|limited| limited == site) {
                continue;
            }
        }
        blocking.push(Blocking::new(
            BlockingRule::StrictIssueUnexplained,
            "",
            issue.clone(),
        ));
    }

    blocking.sort();
    blocking.dedup();
    limitations.sort();
    limitations.dedup();
    per_site.sort_by(|left, right| left.site.cmp(&right.site));

    LimitedVerdict {
        policy: POLICY_VERSION,
        claim: CLAIM,
        case,
        engine,
        strict: strict_result,
        blocking,
        limitations,
        per_site,
    }
}

// ---------------------------------------------------------------------------
// Comparison: decisions cannot override insufficient inputs
// ---------------------------------------------------------------------------

/// One engine's observed decision, reduced by its own adapter. `NotRun` is a
/// case that produced no decision at all; it is never agreement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Evaluated {
        batch_complete: bool,
        roots: Vec<(String, String)>,
    },
    Refused {
        reason: String,
    },
    NotRun {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaseOutcome {
    pub(crate) decision: Decision,
    /// Roots whose fate the fixture requires, from `expected.roots`.
    pub(crate) expected: Vec<(String, String)>,
}

/// The comparison result. `Insufficient` is never `Agreed`, even when both
/// engines' decisions are identical.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ComparisonState {
    /// One or both sides are insufficient, did not run, refused the case or
    /// reported an incomplete batch: no agreement, whatever the fates say.
    Insufficient,
    /// Both sides sufficient, both batches complete and the observed fates
    /// equal the declared fates.
    Agreed,
    /// Both sides sufficient and complete and they agree with each other, but
    /// the observed fates are not the fates the manifest declares. That is a
    /// fixture or expectation defect, not a cross-engine divergence.
    DeclaredMismatch,
    /// Both sides sufficient and complete, and the observed fates differ
    /// between the engines: a real finding, no credit.
    Divergence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComparisonVerdict {
    pub(crate) state: ComparisonState,
    pub(crate) reasons: Vec<String>,
    pub(crate) oxide_sufficient: bool,
    pub(crate) barge_sufficient: bool,
}

/// Compare two limited verdicts with their reduced decisions.
pub(crate) fn limited_comparison(
    oxide: &LimitedVerdict,
    oxide_outcome: &CaseOutcome,
    barge: &LimitedVerdict,
    barge_outcome: &CaseOutcome,
) -> ComparisonVerdict {
    let mut reasons = Vec::new();
    let oxide_sufficient = oxide.sufficient();
    let barge_sufficient = barge.sufficient();
    if !oxide_sufficient {
        reasons.push(format!(
            "oxide inputs are insufficient: {}",
            oxide.blocking_names().join(", ")
        ));
    }
    if !barge_sufficient {
        reasons.push(format!(
            "barge inputs are insufficient: {}",
            barge.blocking_names().join(", ")
        ));
    }
    for (engine, decision) in [
        ("oxide", &oxide_outcome.decision),
        ("barge", &barge_outcome.decision),
    ] {
        match decision {
            Decision::Evaluated { .. } => {}
            Decision::Refused { reason } => reasons.push(format!(
                "{engine} refused the case ({reason}); a refusal is that engine's outcome and \
never an absent root"
            )),
            Decision::NotRun { reason } => {
                reasons.push(format!("{engine} did not run the case ({reason})"))
            }
        }
    }
    if !reasons.is_empty() {
        return ComparisonVerdict {
            state: ComparisonState::Insufficient,
            reasons,
            oxide_sufficient,
            barge_sufficient,
        };
    }
    let fates = |decision: &Decision| -> Vec<(String, String)> {
        match decision {
            Decision::Evaluated { roots, .. } => {
                let mut roots = roots.clone();
                roots.sort();
                roots
            }
            _ => Vec::new(),
        }
    };
    let mut expected = oxide_outcome.expected.clone();
    expected.sort();
    let oxide_fates = fates(&oxide_outcome.decision);
    let barge_fates = fates(&barge_outcome.decision);
    // An incomplete batch on EITHER side is insufficient, never agreement:
    // a partial batch is not a decision over the whole root set.
    for (engine, decision) in [
        ("oxide", &oxide_outcome.decision),
        ("barge", &barge_outcome.decision),
    ] {
        if let Decision::Evaluated {
            batch_complete: false,
            ..
        } = decision
        {
            reasons.push(format!(
                "{engine} reported an incomplete batch; an incomplete batch is not a decision \
over the declared root set"
            ));
        }
    }
    if !reasons.is_empty() {
        return ComparisonVerdict {
            state: ComparisonState::Insufficient,
            reasons,
            oxide_sufficient,
            barge_sufficient,
        };
    }
    if oxide_fates != barge_fates {
        reasons.push(format!(
            "observed divergence: oxide {oxide_fates:?} versus barge {barge_fates:?}"
        ));
        return ComparisonVerdict {
            state: ComparisonState::Divergence,
            reasons,
            oxide_sufficient,
            barge_sufficient,
        };
    }
    if oxide_fates != expected {
        reasons.push(format!(
            "both engines observed {oxide_fates:?} but the manifest declares {expected:?}; the \
fixture or its expectation is unproven"
        ));
        return ComparisonVerdict {
            state: ComparisonState::DeclaredMismatch,
            reasons,
            oxide_sufficient,
            barge_sufficient,
        };
    }
    ComparisonVerdict {
        state: ComparisonState::Agreed,
        reasons,
        oxide_sufficient,
        barge_sufficient,
    }
}

// ---------------------------------------------------------------------------
// The effect-free first-batch plan
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaseStatus {
    /// The fixture can bind every non-excluded obligation of this case.
    Runnable,
    /// A named non-excluded premise cannot be met; no agreement is available.
    Insufficient,
    /// Deliberately held for the second batch; never passed or dropped.
    Deferred,
}

impl CaseStatus {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Runnable => "runnable",
            Self::Insufficient => "insufficient",
            Self::Deferred => "deferred",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cf {
    Cf1,
    Cf2,
    Cf3,
    Cf4,
    Cf5,
    Cf6,
    Cf7,
}

impl Cf {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Cf1 => "CF-1",
            Self::Cf2 => "CF-2",
            Self::Cf3 => "CF-3",
            Self::Cf4 => "CF-4",
            Self::Cf5 => "CF-5",
            Self::Cf6 => "CF-6",
            Self::Cf7 => "CF-7",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prerequisite {
    /// An independently supplied export-closure digest and receipt. None is
    /// qualified, so no case is executable today.
    ExportClosure,
    /// A bound manifest whose digest the transcript recorded.
    BoundManifest,
    /// Barge-side case support. The pinned harness exists; the smallest
    /// remaining source requirement is a per-case fixture builder, named in the
    /// plan's `barge_support` and owned by a linked Barge task (S3).
    BargeCaseSupport,
    /// A fixture that writes its own state, receipts and markers.
    FixtureWrite,
    /// One owned image with real custody facts.
    OwnedImage,
    /// Internal boot Data placement.
    InternalPlacement,
}

impl Prerequisite {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ExportClosure => "export-closure",
            Self::BoundManifest => "bound-manifest",
            Self::BargeCaseSupport => "barge-case-support",
            Self::FixtureWrite => "fixture-write",
            Self::OwnedImage => "owned-image",
            Self::InternalPlacement => "internal-placement",
        }
    }
}

struct CaseOverride {
    id: &'static str,
    status: CaseStatus,
    reason: &'static str,
    cf_extra: &'static [Cf],
}

struct Family {
    name: &'static str,
    cases: &'static [&'static str],
    status: CaseStatus,
    reason: &'static str,
    overrides: &'static [CaseOverride],
    cf: &'static [Cf],
    cf_absent: &'static [(&'static str, &'static str)],
    /// Directories and files the proposed fixture actually creates.
    directories: &'static [&'static str],
    /// Engine enumerations the fixture state triggers, with the caller's site.
    enumerations: &'static [&'static str],
    /// Actual in-crate entrypoints each side runs.
    entrypoints: &'static [&'static str],
    /// Per-case actions, in order.
    actions: &'static [&'static str],
    prerequisites: &'static [Prerequisite],
}

const FIRST_BATCH_PREREQUISITES: &[Prerequisite] = &[
    Prerequisite::ExportClosure,
    Prerequisite::BoundManifest,
    Prerequisite::BargeCaseSupport,
    Prerequisite::FixtureWrite,
];

const FAMILIES: &[Family] = &[
    Family {
        name: "control and sticky batch",
        cases: &["CTL-0", "CTL-1"],
        status: CaseStatus::Insufficient,
        reason: "every file input this decision reads is a path-only or handle-class read: the \
registry and receipts are opened by path and the owner marker with openat from a held root \
descriptor, so the recorder's in-window bind check reports `extra or unbound observation at \
<kind>` and its obligation check reports `mandatory observation missing or identity unproven at \
<site>`. The reviewed path-only exclusion covers a parent obligation only and cannot bind the \
row",
        overrides: &[],
        cf: &[Cf::Cf1, Cf::Cf4],
        cf_absent: &[
            ("CF-2", "the fixture starts no writer; custody is the deferred second batch"),
            ("CF-3", "the marker is not replaced in this family"),
            ("CF-5", "no identity epoch changes inside the window"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists, one row)",
            "<pool>/oxide-managed/outputs/ (exists, enumerated)",
            "<mount>/oxide-outputs/<root>/ (exists under the image)",
            "<checkout> (absent, positively)",
            "<pool>/admission.lock (absent)",
        ],
        enumerations: &[
            "barge: area::list_names (plan roots)",
            "barge: cache::read_placements (placements)",
            "barge: area::read_declarations (declarations)",
            "barge: reference::read_receipts over read_dir(outputs) (OxideSnapshot::read_within)",
        ],
        entrypoints: &[
            "oxide: prune::tape_entry::run_chain -> require_ready -> evaluate",
            "barge: release::plan",
        ],
        actions: &[
            "create the pool control, registry row and root marker",
            "attach the owned image and register it",
            "capture the manifest, run each engine once, retain both raw transcripts",
            "apply the limited verdict to each side, then the comparison",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "registry shape and caps",
        cases: &[
            "REG-MISSING",
            "REG-MALFORMED",
            "REG-OVERSIZE",
            "REG-CONTRADICTORY",
            "REG-OVERFLOW",
            "CAP-1",
        ],
        status: CaseStatus::Insufficient,
        reason: "the registry is a path-only direct read (`authority-read@prune::read_registry`): \
the recorder records no parent for it, so the in-window bind check reports `extra or unbound \
observation` and the parent-required obligation note is unmet. No fixture can bind that row \
under the current recorder",
        overrides: &[CaseOverride {
            id: "CAP-1",
            status: CaseStatus::Insufficient,
            reason: "CAP-1 exceeds the shared caps (8192 read rows, 64 occurrences per key). An \
overflowed or truncated document is insufficient by rule, no cap change is allowed, and no \
partial parity is claimed",
            cf_extra: &[],
        }],
        cf: &[Cf::Cf4],
        cf_absent: &[
            ("CF-1", "the fixture creates no enumerated set; the case decides on the registry file"),
            ("CF-2", "no writer participates"),
            ("CF-3", "no root marker exists in this family"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists or is positively absent per case)",
            "<pool>/oxide-managed/outputs/ (absent: no receipts are enumerated)",
            "<mount>/oxide-outputs/<root>/ (absent)",
        ],
        enumerations: &[],
        entrypoints: &[
            "oxide: prune::read_registry",
            "barge: census::read_registry, OxideSnapshot::read_within",
        ],
        actions: &[
            "write only the registry bytes this case names",
            "run each engine once and retain its raw result and reason",
            "for CAP-1 stop: the caps make the transcript insufficient",
        ],
        prerequisites: &[],
    },
    Family {
        name: "references and intents",
        cases: &[
            "REF-PENDING",
            "REF-RESERVATION",
            "REF-CANCELLING",
            "REF-PRUNING",
            "REF-SELECTED-NOT-CURRENT",
            "REF-ORPHAN-MARKED",
        ],
        status: CaseStatus::Insufficient,
        reason: "the registry row and the intent files are path-only reads with no recorded \
parent, so their rows are unbound in the in-window check and their obligations report unmet; \
the CF-1 enumeration of the intents directory is a second, separate requirement",
        overrides: &[CaseOverride {
            id: "REF-ORPHAN-MARKED",
            status: CaseStatus::Insufficient,
            reason: "EXPLAIN-OR-BLOCK: an observed difference is reportable but no current source \
can produce an agreement for an orphaned marked row",
            cf_extra: &[],
        }],
        cf: &[Cf::Cf1, Cf::Cf4],
        cf_absent: &[
            ("CF-2", "no writer participates"),
            ("CF-3", "no root marker is replaced"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists, the row under test)",
            "<pool>/oxide-managed/intents/ (exists and is enumerated)",
            "<mount>/oxide-outputs/<root>/ (exists, unread by this family)",
        ],
        enumerations: &[
            "barge: state::read_intents_with_deadline over the intents directory \
(release::pending_intents)",
        ],
        entrypoints: &[
            "oxide: Row::live, dry_run, eligibility, unfinished_intents (a registry-document \
projection, not a directory read)",
            "barge: OxideRow::live, references_for, release::pending_intents",
        ],
        actions: &[
            "write the registry row and one intent document",
            "run each engine once and retain its own scope record",
            "compare fates only where the declared classification is an agreement",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "receipts and markers",
        cases: &[
            "RCP-MISSING",
            "RCP-MALFORMED",
            "RCP-NONCOMMITTED",
            "RCP-CONTRADICTORY",
            "RCP-V1",
            "MRK-MISSING",
            "MRK-MALFORMED",
            "MRK-REPLACED",
            "ID-NONCE",
            "ID-GENERATION",
        ],
        status: CaseStatus::Insufficient,
        reason: "receipts are path-only reads (unbound in the in-window check) and the owner \
marker is opened with openat from a held root descriptor, so its parent is handle-class: the \
path-only exclusion can cover neither the unbound row nor the K1 parent",
        overrides: &[CaseOverride {
            id: "RCP-V1",
            status: CaseStatus::Insufficient,
            reason: "EXPLAIN-OR-BLOCK: Barge reads a /1 receipt as unknown, so an observed \
difference is reportable and a match is not available from current source",
            cf_extra: &[],
        }],
        cf: &[Cf::Cf1, Cf::Cf3, Cf::Cf4],
        cf_absent: &[
            ("CF-2", "no writer participates"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch inside this fixture"),
        ],
        directories: &[
            "<pool>/registry.json (exists)",
            "<pool>/oxide-managed/outputs/ (exists and is enumerated)",
            "<mount>/oxide-outputs/<root>/ (exists with the receipt and owner marker)",
        ],
        enumerations: &[
            "barge: reference::read_receipts over read_dir(outputs) (OxideSnapshot::read_within)",
        ],
        entrypoints: &[
            "oxide: read_receipt_for_prune (one receipt per registry row, no directory \
enumeration), outputs::read_marker and prune::observe_root_in's owner-marker parser",
            "barge: reference::read_within, reference::read_oxide_marker",
        ],
        actions: &[
            "write the receipt and marker bytes for the case",
            "for MRK-REPLACED rewrite identical bytes into a new inode after capture (CF-3)",
            "run each engine once and retain both raw results",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "identity and volume",
        cases: &[
            "ID-INODE",
            "ID-DEVICE-STALE",
            "ID-DEVICE-LIVE",
            "VOL-ABSENT",
            "VOL-REMOVED-DURING",
            "VOL-DUP-REGISTRY",
            "VOL-DUP-LIVE",
            "ABS-UUID-CONFLICT",
        ],
        status: CaseStatus::Insufficient,
        reason: "the owner marker is handle-class and the registry row is path-only and \
unbound; the volume-census rows do bind their own volume identity, but the decision's file \
inputs do not",
        overrides: &[
            CaseOverride {
                id: "ID-INODE",
                status: CaseStatus::Insufficient,
                reason: "D-2 is a declared batch-versus-root scope difference and needs a second \
eligible root; the fates differ by scope, so no agreement is available",
                cf_extra: &[],
            },
            CaseOverride {
                id: "ID-DEVICE-LIVE",
                status: CaseStatus::Insufficient,
                reason: "a mid-window remount is a declared dynamic outcome: it needs CF-2/CF-5 \
writers and a second mount epoch, and no cross-window parity is claimed",
                cf_extra: &[Cf::Cf2, Cf::Cf5],
            },
            CaseOverride {
                id: "VOL-REMOVED-DURING",
                status: CaseStatus::Insufficient,
                reason: "detach between windows is a declared dynamic outcome with its own mount \
census per window; parity holds only on declared outcomes and the case drives an image effect",
                cf_extra: &[Cf::Cf2, Cf::Cf5],
            },
            CaseOverride {
                id: "VOL-DUP-REGISTRY",
                status: CaseStatus::Insufficient,
                reason: "a declared per-engine record with no cross-engine parity claim",
                cf_extra: &[],
            },
            CaseOverride {
                id: "VOL-DUP-LIVE",
                status: CaseStatus::Insufficient,
                reason: "a real duplicate-UUID mount census cannot be produced from synthetic \
fixtures without kernel forgery; declared coverage limit, never parity",
                cf_extra: &[],
            },
        ],
        cf: &[Cf::Cf3, Cf::Cf4, Cf::Cf7],
        cf_absent: &[
            ("CF-1", "the fixture enumerates no receipt, intent, declaration or placement set"),
            ("CF-2", "only the dynamic rows (ID-DEVICE-LIVE, VOL-REMOVED-DURING) start a writer, and those cases name it"),
            ("CF-5", "only the dynamic rows change an identity epoch, and those cases name it"),
            ("CF-6", "no admission input is missing"),
        ],
        directories: &[
            "<pool>/registry.json (exists with the recorded volume identity)",
            "<pool>/oxide-managed/outputs/ (absent: no enumeration runs)",
            "<mount>/oxide-outputs/<root>/ (exists, or the volume is positively absent)",
        ],
        enumerations: &[],
        entrypoints: &[
            "oxide: judge_recorded_root, volume::Binding",
            "barge: identity::read, identity::compare, VolumeRoot::open_with",
        ],
        actions: &[
            "build the root and its marker with the case's recorded identity",
            "for VOL-ABSENT attach nothing and record the volume as absent",
            "run each engine once and compare only declared outcomes",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "checkout absence",
        cases: &[
            "ABS-ANCESTOR-MISSING",
            "ABS-ANCESTOR-REPLACED",
            "ABS-BOOT-DATA",
        ],
        status: CaseStatus::Insufficient,
        reason: "the absence decision reads component rows of a no-follow walk whose parents are \
live held descriptors: that is handle-class (K1) and no reviewed exclusion covers it, so the \
inputs stay insufficient",
        overrides: &[],
        cf: &[Cf::Cf4, Cf::Cf7],
        cf_absent: &[
            ("CF-1", "no receipt, intent, declaration or placement set is enumerated"),
            ("CF-2", "no writer participates"),
            ("CF-3", "no root marker is replaced"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
        ],
        directories: &[
            "<pool>/registry.json (exists with the root row)",
            "<mount>/oxide-outputs/<root>/ (exists)",
            "<checkout> (absent, or an ancestor replaced)",
        ],
        enumerations: &[],
        entrypoints: &[
            "oxide: checkout_absence, walk_checkout, revalidate_chain",
            "barge: os::prove_absence",
        ],
        actions: &[
            "build the absent or replaced ancestor chain",
            "run each engine once; the verdict stays insufficient at the component parents",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "fence, declaration and policy",
        cases: &["FENCE-MISSING", "FENCE-UNKNOWN", "DECL-MISSING", "DECL-POLICY-NONE"],
        status: CaseStatus::Insufficient,
        reason: "the fence lives in the pool control read (`authority-read@store::load`), a \
custody-class read no reviewed exclusion covers, and the registry row that precedes it is \
path-only and unbound",
        overrides: &[
            CaseOverride {
                id: "DECL-MISSING",
                status: CaseStatus::Insufficient,
                reason: "SCOPE: Barge refuses the command with no release policy while Oxide is \
unaffected; a whole-command refusal is not per-root parity",
                cf_extra: &[],
            },
            CaseOverride {
                id: "DECL-POLICY-NONE",
                status: CaseStatus::Insufficient,
                reason: "CONFIG: the two configurations differ by declaration policy, so no parity \
claim exists",
                cf_extra: &[],
            },
        ],
        cf: &[Cf::Cf1, Cf::Cf4],
        cf_absent: &[
            ("CF-2", "no writer participates"),
            ("CF-3", "no root marker is replaced"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists)",
            "<pool>/oxide-managed/control.json (absent, or carrying the fence)",
            "<pool>/oxide-managed/outputs/ (exists and is enumerated)",
            "<mount>/oxide-outputs/<root>/ (exists)",
        ],
        enumerations: &["barge: area::read_declarations"],
        entrypoints: &[
            "oxide: outputs_initialization, the state parser",
            "barge: area::show, pool_custody::parse_state, decide_root",
        ],
        actions: &[
            "write the fence or declaration document for the case",
            "run each engine once and retain its batch scope",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "legacy and operator list",
        cases: &["LEG-UNLISTED", "LEG-LISTED", "LEG-LISTED-MARKED", "LEG-RECEIPTLESS"],
        status: CaseStatus::Insufficient,
        reason: "the operator list and the receipts are path-only reads with no recorded parent, \
so their rows are unbound in the in-window check and their obligations report unmet; the receipt \
enumeration is a second, separate requirement",
        overrides: &[CaseOverride {
            id: "LEG-LISTED",
            status: CaseStatus::Insufficient,
            reason: "D-3: the legacy partition differs by engine (Oxide admits an absent checkout, \
Barge keeps at the pin); a declared difference is not agreement",
            cf_extra: &[],
        }],
        cf: &[Cf::Cf1, Cf::Cf4],
        cf_absent: &[
            ("CF-2", "no writer participates"),
            ("CF-3", "no root marker is replaced"),
            ("CF-5", "no identity epoch changes"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists with the legacy row)",
            "<pool>/operator-list.json (present or absent per case)",
            "<pool>/oxide-managed/outputs/ (exists and is enumerated)",
            "<mount>/oxide-outputs/<root>/ (exists or is positively missing)",
        ],
        enumerations: &[
            "barge: reference::read_receipts over read_dir(outputs) (OxideSnapshot::read_within)",
        ],
        entrypoints: &[
            "oxide: read_operator_list, evaluate_receiptless, read_receipt_for_prune",
            "barge: identity and reference conditions (reference::read_within)",
        ],
        actions: &[
            "write the legacy row and the operator list document",
            "run each engine once and retain the D-3 partition result",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "writers and custody (DEFERRED, second batch)",
        cases: &[
            "WRT-LIVE-THIS",
            "WRT-LIVE-OTHER",
            "WRT-UNKNOWN",
            "WRT-ACQUIRING",
            "WRT-RETIRED-CLEAN",
        ],
        status: CaseStatus::Deferred,
        reason: "the second batch is deliberately held: recorded holders carry a null kernel \
start, so the custody decision would rest on an unproven holder and the collapsed process key \
is cap-bound",
        overrides: &[],
        cf: &[Cf::Cf2, Cf::Cf4, Cf::Cf5],
        cf_absent: &[
            ("CF-1", "no receipt, intent, declaration or placement set is enumerated"),
            ("CF-3", "no root marker is replaced"),
            ("CF-6", "no admission input is missing"),
            ("CF-7", "no second read under a changed epoch inside this fixture"),
        ],
        directories: &[
            "<pool>/registry.json (exists with the holder records)",
            "<pool>/admission.lock (exists and is held)",
            "<mount>/oxide-outputs/<root>/ (exists)",
        ],
        enumerations: &[],
        entrypoints: &[
            "oxide: checkout_custody, active_holders",
            "barge: PoolCustody::read, pool_custody::classify, custody::take_custody_by, \
custody::decide",
        ],
        actions: &[
            "start the declared fixture holder for window A only",
            "run each engine in its own window and bind its own custody facts",
            "record the keep fate only; never cross-window parity",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
    Family {
        name: "admission effects (DEFERRED, second batch)",
        cases: &["EFFECT-1", "EFFECT-2"],
        status: CaseStatus::Deferred,
        reason: "the second batch is deliberately held: it drives an admission lock create and a \
fence save, so an attempted or partial effect is in scope and the tier is ELEVATED",
        overrides: &[],
        cf: &[Cf::Cf2, Cf::Cf4, Cf::Cf6],
        cf_absent: &[
            ("CF-1", "no receipt, intent, declaration or placement set is enumerated"),
            ("CF-3", "no root marker is replaced"),
            ("CF-5", "no identity epoch changes"),
            ("CF-7", "no second read under a changed epoch"),
        ],
        directories: &[
            "<pool>/registry.json (exists)",
            "<pool>/admission.lock (absent for EFFECT-1)",
            "<pool>/admission-state.json (present, without the accounting fence, for EFFECT-2)",
        ],
        enumerations: &[],
        entrypoints: &[
            "oxide: checkout_custody lock open, fence_admission_state, ledger_view",
            "barge: StateLock::shared_if_present",
        ],
        actions: &[
            "plant the missing admission input",
            "run each engine once and record the completed-effect set, not attempts",
            "stop on any pending holding or eviction reconciliation",
        ],
        prerequisites: &[Prerequisite::OwnedImage],
    },
];

/// One expanded plan row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlannedCase {
    pub(crate) id: &'static str,
    pub(crate) family: &'static str,
    pub(crate) status: CaseStatus,
    pub(crate) reason: &'static str,
    pub(crate) cf: Vec<Cf>,
    pub(crate) cf_absent: Vec<(&'static str, &'static str)>,
    pub(crate) directories: &'static [&'static str],
    pub(crate) enumerations: &'static [&'static str],
    pub(crate) entrypoints: &'static [&'static str],
    pub(crate) actions: &'static [&'static str],
    pub(crate) prerequisites: Vec<Prerequisite>,
}

/// The nine native-proof premises, each with the disposition this preparation
/// proposes. Readiness obligations only: nothing here authorises execution.
const PREMISE_DISPOSITIONS: &[(&str, &str)] = &[
    (
        "private HOME and rustup",
        "each engine child gets a private HOME, CARGO_HOME and rustup home; the invoking \
RUSTUP_HOME is resolved before the child HOME exists",
    ),
    (
        "Oxide fixture home",
        "OXIDE_TEST_HOME_FROM_ENV=1 on every Oxide child; Barge facts come from explicit \
StatePaths/AccountFacts only",
    ),
    (
        "inherited environment",
        "the case body scrubs OXIDE_*, BARGE_*, XDG_*, CARGO_*, SCCACHE_* and RUSTC* before \
setting fixture facts, restoring only the guard controls the supervisor needs",
    ),
    (
        "adoption premises",
        "fixture code creates every directory and receipt; no build runs inside the window",
    ),
    (
        "external subcommand",
        "no Cargo external subcommand is invoked; not applicable to this comparison",
    ),
    (
        "volume and image",
        "one owned 128 MiB image per occurrence in both batches, hdiutil attach -owners on, \
ordinary post-census detach, real custody facts; the first batch attaches one only to give the \
fixture a real /Volumes root binding",
    ),
    (
        "Rust flags",
        "one frozen RUSTFLAGS with CARGO_ENCODED_RUSTFLAGS removed for staging and every child",
    ),
    (
        "scratch routing",
        "fixture, transcripts and build scratch under the qualified supervisor's selected-volume \
routing, with a same-volume TMPDIR",
    ),
    (
        "temporary roots",
        "mktemp templates under the owned scratch parent; no fixed /tmp path and no internal \
drive",
    ),
];

/// The bounded next occurrence this preparation proposes. Nothing here is a
/// run, a reservation or a credit.
struct NextOccurrence {
    first_batch_tier: &'static str,
    first_batch_rationale: &'static str,
    second_batch_tier: &'static str,
    second_batch_rationale: &'static str,
    wall_clock_seconds: u32,
    scratch_mib: u32,
    images: &'static str,
    runner: &'static str,
    command: &'static str,
    scratch_interface: &'static str,
    measurement: &'static str,
}

const NEXT_OCCURRENCE: NextOccurrence = NextOccurrence {
    first_batch_tier: "ELEVATED",
    first_batch_rationale:
        "the first batch attaches one owned 128 MiB image at a /Volumes mount so the fixture \
gets a real external root binding, and an attached image is removable media: that is the \
ELEVATED trigger even though nothing is deleted, recycled or recovered",
    second_batch_tier: "ELEVATED",
    second_batch_rationale: "the second batch decides writer custody and drives an admission lock \
create and a fence save on the same attached removable media, which is the ELEVATED trigger",
    wall_clock_seconds: 600,
    scratch_mib: 512,
    images: "one owned 128 MiB image per occurrence in both batches, attached with hdiutil \
-owners on and detached ordinarily after the supervisor's post-leader census",
    runner: "the qualified shared supervisor scripts/bounded-test.py",
    command: "python3 scripts/bounded-test.py --limit 600 --scratch-budget-mib 512 -- \
python3 scripts/<case>.py",
    scratch_interface: "owned Stash scratch, volume UUID 8A660440-C919-45C6-A3F1-3365B9BD2006: \
the supervisor selects its own <mount>/scratch/oxide run root and evidence directory there, the \
case exports OXIDE_TASK_SCRATCH_ROOT inside it and passes no --root-parent and no --evidence",
    measurement: "the 600 s and 512 MiB figures are unmeasured proposals; no live-host size or \
timing is inferred here",
};

/// Expand the family table into one row per original case.
pub(crate) fn planned_cases() -> Vec<PlannedCase> {
    let mut rows = Vec::new();
    for family in FAMILIES {
        for id in family.cases {
            let mut status = family.status;
            let mut reason = family.reason;
            let mut cf: Vec<Cf> = family.cf.to_vec();
            let mut cf_absent: Vec<(&'static str, &'static str)> = family.cf_absent.to_vec();
            for case_override in family.overrides {
                if case_override.id != *id {
                    continue;
                }
                status = case_override.status;
                reason = case_override.reason;
                for extra in case_override.cf_extra {
                    if !cf.contains(extra) {
                        cf.push(*extra);
                    }
                    cf_absent.retain(|(name, _)| *name != extra.name());
                }
            }
            cf.sort_by_key(|cf| cf.name());
            cf_absent.sort_by(|left, right| left.0.cmp(right.0));
            let mut prerequisites: Vec<Prerequisite> = family.prerequisites.to_vec();
            for shared in FIRST_BATCH_PREREQUISITES {
                if !prerequisites.contains(shared) && status != CaseStatus::Deferred {
                    prerequisites.push(*shared);
                }
            }
            if status == CaseStatus::Deferred {
                for shared in FIRST_BATCH_PREREQUISITES {
                    if !prerequisites.contains(shared) {
                        prerequisites.push(*shared);
                    }
                }
            }
            rows.push(PlannedCase {
                id,
                family: family.name,
                status,
                reason,
                cf,
                cf_absent,
                directories: family.directories,
                enumerations: family.enumerations,
                entrypoints: family.entrypoints,
                actions: family.actions,
                prerequisites,
            });
        }
    }
    rows
}

/// The effect-free plan document. It prints; it writes and runs nothing.
pub(crate) fn plan_document() -> Value {
    let rows = planned_cases();
    json!({
        "schema": PLAN_VERSION,
        "policy": POLICY_VERSION,
        "claim": CLAIM,
        "executable_today": false,
        "execution_gate": "two independent gates: no export-closure digest and receipt is \
    qualified, and no first-batch input binds under the current recorder (see binding_gap); this plan \
    is readiness only and gives no comparison, parity or native credit",
        "barge_support": {
            "summary": BARGE_SUPPORT_REQUIREMENT,
            "harness": "crates/barge/tests/tape_eval.rs (harness = false) at f9f81f72",
            "smallest_requirement": "a per-case fixture builder in \
    crates/barge/src/tape_harness.rs; a linked Barge task, not this PR",
        },
        "binding_gap": {
            "summary": BINDING_GAP,
            "site_classes": KNOWN_SITES
                .iter()
                .map(|site| json!({"site": site.kind, "class": site.class.name(), "source": site.source}))
                .collect::<Vec<_>>(),
            "closure": "a separately reviewed test-only recorder change; not authorised, started or \
    assumed by this preparation",
        },
        "exclusions": [
            {
                "rule": ExclusionRule::WindowOriginEndNull.name(),
                "reason": ExclusionRule::WindowOriginEndNull.reason(),
            },
            {
                "rule": ExclusionRule::PathOnlyDirectReadParent.name(),
                "reason": ExclusionRule::PathOnlyDirectReadParent.reason(),
                "sites": KNOWN_SITES
                    .iter()
                    .filter(|site| site.class == SiteClass::PathOnlyDirectRead)
                    .map(|site| json!({"site": site.kind, "source": site.source}))
                    .collect::<Vec<_>>(),
            },
        ],
        "blocking_rules": ALL_BLOCKING_RULES
            .iter()
            .map(|rule| rule.name())
            .collect::<Vec<_>>(),
        "families": FAMILIES
            .iter()
            .map(|family| json!({
                "family": family.name,
                "status": family.status.name(),
                "reason": family.reason,
                "cf": family.cf.iter().map(|cf| cf.name()).collect::<Vec<_>>(),
                "cf_not_applicable": family
                    .cf_absent
                    .iter()
                    .map(|(cf, reason)| json!({"cf": cf, "reason": reason}))
                    .collect::<Vec<_>>(),
                "directories": family.directories,
                "enumerations": family.enumerations,
                "entrypoints": family.entrypoints,
                "actions": family.actions,
                "prerequisites": family
                    .prerequisites
                    .iter()
                    .map(|prerequisite| prerequisite.name())
                    .collect::<Vec<_>>(),
                "cases": family
                    .cases
                    .iter()
                    .map(|id| {
                        let row = rows
                            .iter()
                            .find(|row| row.id == *id)
                            .expect("every case expands");
                        json!({
                            "case": row.id,
                            "status": row.status.name(),
                            "reason": row.reason,
                            "cf": row.cf.iter().map(|cf| cf.name()).collect::<Vec<_>>(),
                            "cf_not_applicable": row
                                .cf_absent
                                .iter()
                                .map(|(cf, reason)| json!({"cf": cf, "reason": reason}))
                                .collect::<Vec<_>>(),
                            "prerequisites": row
                                .prerequisites
                                .iter()
                                .map(|prerequisite| prerequisite.name())
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>(),
        "next_occurrence": {
            "first_batch": {
                "tier": NEXT_OCCURRENCE.first_batch_tier,
                "rationale": NEXT_OCCURRENCE.first_batch_rationale,
            },
            "second_batch": {
                "tier": NEXT_OCCURRENCE.second_batch_tier,
                "rationale": NEXT_OCCURRENCE.second_batch_rationale,
            },
            "wall_clock_seconds": NEXT_OCCURRENCE.wall_clock_seconds,
            "scratch_mib": NEXT_OCCURRENCE.scratch_mib,
            "images": NEXT_OCCURRENCE.images,
            "runner": NEXT_OCCURRENCE.runner,
            "runner_sha256": QUALIFIED_RUNNER_SHA256,
            "command": NEXT_OCCURRENCE.command,
            "scratch_interface": NEXT_OCCURRENCE.scratch_interface,
            "measurement": NEXT_OCCURRENCE.measurement,
            "premises": PREMISE_DISPOSITIONS
                .iter()
                .map(|(premise, disposition)| {
                    json!({"premise": premise, "disposition": disposition})
                })
                .collect::<Vec<_>>(),
            "execution_authority": "none: a diagnostic, counted author and independent reviewer \
    native each need separate activation at the reviewed head",
        },
    })
}

// ---------------------------------------------------------------------------
// Pure policy controls
// ---------------------------------------------------------------------------

const CONTROL_DIGEST: &str = "1111111111111111111111111111111111111111111111111111111111111111";

fn hex(ch: char, count: usize) -> String {
    std::iter::repeat(ch).take(count).collect()
}

/// A synthetic transcript that is sufficient under the limited policy: one
/// path-only direct read with a bound leaf, a null window, and the encoder's
/// own window note.
fn synthetic_transcript() -> Value {
    json!({
        "schema": "oxide.capture.transcript/2",
        "version": "oxide.capture.transcript/2",
        "engine": "oxide",
        "case": "CTL-0",
        "manifest_sha256": CONTROL_DIGEST,
        "entrypoint": "oxide::prune::tape_entry::run_chain",
        "window_monotonic_start_ns": Value::Null,
        "window_monotonic_end_ns": Value::Null,
        "window_elapsed_ns": Value::Null,
        "outcome": {"type": "completed", "result": {"complete": true, "entries": []}},
        "truncation": {
            "read_rows_truncated": false,
            "occurrences_truncated": false,
            "document_truncated": false,
            "process_rows_truncated": false,
            "classifications_truncated": false,
        },
        "effects": [],
        "reads": [],
        "clocks": [],
        "classifications": [],
        "recorded_holders": [],
        "fixture_capture": {
            "root": "/fixture",
            "truncated": false,
            "pre": [],
            "post": [],
            "diff_effects": [],
        },
        "notes": ["absolute monotonic window origin and end unproven"],
    })
}

const CONTROL_SITE: &str = "authority-read@prune::read_registry";
const CONTROL_PATH: &str = "/fixture/registry.json";

fn synthetic_manifest() -> Value {
    json!({
        "schema": "oxide.capture.manifest/2",
        "case": "CTL-0",
        "window": {"identity": "window-1", "start_monotonic_ns": 1, "end_monotonic_ns": 2},
        "source": {
            "oxide_commit": "923d01f803cc7708569985cd5217edc294a1189c",
            "oxide_files": {},
            "cargo_lock_sha256": hex('a', 64),
            "rust_toolchain_sha256": hex('b', 64),
            "barge_pin": SCHEMA_BARGE_PIN,
            "barge_slice_commit": "f9f81f72a20e0f4d694b8046acfbe3a0558f8c9b",
            "barge_files": {},
            "staged_lock_sha256": hex('c', 64),
            "oxide_toolchain": "1.97.1",
            "barge_toolchain": "1.97.1",
            "oxide_executable_sha256": hex('d', 64),
            "barge_executable_sha256": hex('e', 64),
            "oxide_tree": hex('f', 40),
        },
        "fixture": {
            "root": "/fixture",
            "device": 1,
            "inode": 2,
            "mode": 448,
            "entries": [],
            "roots": ["root-a"],
            "volume": {"uuid": "00000000-0000-0000-0000-000000000000", "mount": "/fixture"},
        },
        "dynamic": {"volumes": [], "writers": [], "external_allowances": []},
        "mandatory_observations": [{
            "engine": "oxide",
            "site": CONTROL_SITE,
            "parent_required": true,
            "identity": {
                "path": CONTROL_PATH,
                "device": 3,
                "inode": 4,
            },
        }],
        "expected_effects": [],
        "expected": {"classification": "MATCH", "roots": [{"root": "root-a", "fate": "keep"}]},
        "barge": {
            "volume_uuid": "00000000-0000-0000-0000-000000000000",
            "namespace": "oxide",
            "area": "outputs",
            "named_roots": [],
            "volume_mount": "/fixture",
        },
        "operator_list": Value::Null,
    })
}

fn synthetic_row(site: &str, parent_source: &str, dev: Option<u64>, ino: Option<u64>) -> Value {
    json!({
        "kind": site,
        "site": site,
        "identity": {
            "path": CONTROL_PATH,
            "component": Value::Null,
            "parent_device": Value::Null,
            "parent_inode": Value::Null,
            "device": dev,
            "inode": ino,
            "pid": Value::Null,
            "start": Value::Null,
            "uid": Value::Null,
            "fd": Value::Null,
            "volume_uuid": Value::Null,
            "mount": Value::Null,
            "volume_device": Value::Null,
            "parent_source": parent_source,
            "own_source": if dev.is_some() { "extracted" } else { "none" },
        },
        "outcome": {"type": "digest", "digest": hex('9', 64)},
        "cap": 64,
        "epoch": 1,
        "occurrence": 1,
    })
}

fn blocked_by(verdict: &LimitedVerdict, rule: BlockingRule) -> bool {
    verdict
        .blocking
        .iter()
        .any(|blocking| blocking.rule == rule)
}

fn blocked_at(verdict: &LimitedVerdict, rule: BlockingRule, site: &str) -> bool {
    verdict
        .blocking
        .iter()
        .any(|blocking| blocking.rule == rule && blocking.site == site)
}

struct ControlFailure {
    what: String,
}

type Control = Result<(), ControlFailure>;

fn control(condition: bool, what: impl Into<String>) -> Control {
    if condition {
        Ok(())
    } else {
        Err(ControlFailure { what: what.into() })
    }
}

/// The discriminating pure controls. Each one asserts an exact rule and site,
/// never just "the verdict failed".
pub(crate) fn run_checks() -> Result<usize, String> {
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (name, case) in controls() {
        match case() {
            Ok(()) => checked += 1,
            Err(failure) => failures.push(format!("{name}: {}", failure.what)),
        }
    }
    for (name, case) in plan_controls() {
        match case() {
            Ok(()) => checked += 1,
            Err(failure) => failures.push(format!("{name}: {}", failure.what)),
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    println!("oxide: capture-verdict: {checked} limited-policy checks passed");
    Ok(checked)
}

fn controls() -> Vec<(&'static str, fn() -> Control)> {
    vec![
        ("allowed path-only limitation", allowed_path_only_limitation),
        ("handle parent blocks", handle_parent_blocks),
        ("missing row blocks", missing_row_blocks),
        ("wrong leaf identity blocks", wrong_leaf_identity_blocks),
        ("unlisted site blocks", unlisted_site_blocks),
        ("extra unbound observation blocks", extra_unbound_blocks),
        ("unknown note blocks", unknown_note_blocks),
        ("unobserved clock blocks", unobserved_clock_blocks),
        ("unproven holder blocks", unproven_holder_blocks),
        ("truncation blocks", truncation_blocks),
        ("collision blocks", collision_blocks),
        ("attempted effect blocks", attempted_effect_blocks),
        ("partial effect blocks", partial_effect_blocks),
        ("unpathed effect blocks", unpathed_effect_blocks),
        ("omitted effect blocks", omitted_effect_blocks),
        ("extra effect blocks", extra_effect_blocks),
        ("substituted effect blocks", substituted_effect_blocks),
        (
            "effect multiplicity is preserved",
            effect_multiplicity_is_preserved,
        ),
        ("numeric window origin blocks", numeric_window_blocks),
        ("case drift blocks", case_drift_blocks),
        ("digest drift blocks", digest_drift_blocks),
        ("source drift blocks", source_drift_blocks),
        ("topology drift blocks", topology_drift_blocks),
        ("unbound leaf blocks the exclusion", unbound_leaf_blocks),
        (
            "each exclusion binds its own site and leaf",
            second_path_only_site_blocks,
        ),
        (
            "an informative identity note is not a defect",
            informative_identity_note,
        ),
        (
            "a declared input's identity note blocks",
            declared_input_identity_note,
        ),
        (
            "an unrequired parent is not an exclusion",
            unrequired_parent_is_not_an_exclusion,
        ),
        (
            "a real path-only row stays insufficient",
            recorder_path_only_row_is_insufficient,
        ),
        (
            "the malformed-obligation recorder note blocks",
            recorder_malformed_obligation_note_blocks,
        ),
        (
            "the printed blocking vocabulary is the enum",
            plan_blocking_vocabulary,
        ),
        (
            "a bound leaf without a fixture binding stays insufficient",
            recorder_bound_leaf_without_binding_is_insufficient,
        ),
        (
            "an emitted absence kind is handle-class",
            emitted_absence_kind_blocks,
        ),
        (
            "the parser entry point refuses a non-/2 document",
            parser_entry_point_refuses_non_v2,
        ),
        (
            "a Barge-only incomplete batch is insufficient",
            barge_only_incomplete_is_insufficient,
        ),
        (
            "a declared mismatch is not a divergence",
            declared_mismatch_is_not_divergence,
        ),
        ("strict insufficiency is preserved", strict_preservation),
        ("identical decisions cannot override", identical_decisions),
        ("deferred case is not agreement", deferred_is_not_agreement),
        ("divergence is not agreement", divergence_is_not_agreement),
        ("agreement needs two sufficient sides", agreement_needs_both),
    ]
}

fn all_pass(verdict: &LimitedVerdict) -> Control {
    if verdict.sufficient() {
        Ok(())
    } else {
        Err(ControlFailure {
            what: format!("expected sufficient, got {:?}", verdict.blocking),
        })
    }
}

fn allowed_path_only_limitation() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    all_pass(&verdict)?;
    control(
        verdict.limitations.len() == 2
            && verdict.limitations.iter().any(|limitation| {
                limitation.rule == ExclusionRule::PathOnlyDirectReadParent
                    && limitation.site == CONTROL_SITE
                    && limitation.leaf_identity.contains("device=3")
            }),
        "the path-only limitation names the exact site and leaf identity",
    )?;
    control(
        !verdict.strict.complete
            && verdict
                .strict
                .issues
                .iter()
                .any(|issue| issue.starts_with("parent_required unproven at")),
        "the strict result stays insufficient and is retained",
    )
}

fn handle_parent_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "handle", Some(3), Some(4))]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::HandleParentUnqualified,
            CONTROL_SITE,
        ),
        "a handle-class parent is blocking",
    )?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent),
        "no path-only limitation is granted to a handle-class row",
    )
}

fn missing_row_blocks() -> Control {
    let transcript = synthetic_transcript();
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::MissingMandatoryObservation,
            CONTROL_SITE,
        ),
        "a missing mandatory row is blocking",
    )
}

fn wrong_leaf_identity_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(5), Some(6))]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::WrongLeafIdentity, CONTROL_SITE),
        "a non-matching leaf identity is blocking",
    )
}

fn unlisted_site_blocks() -> Control {
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"][0]["site"] = json!("authority-read@unknown::site");
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(
        "authority-read@unknown::site",
        "none",
        Some(3),
        Some(4)
    )]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::UnlistedSite,
            "authority-read@unknown::site",
        ),
        "an unlisted site is blocking, never guessed",
    )
}

fn extra_unbound_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "extra or unbound observation at read-bounded:lstat",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::ExtraUnboundObservation,
            "read-bounded:lstat",
        ),
        "an extra or unbound observation is blocking",
    )
}

fn unknown_note_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "something the recorder never emits",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_by(&verdict, BlockingRule::UnknownNote),
        "an unrecognised engine note is blocking",
    )
}

fn unobserved_clock_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    transcript["clocks"] = json!([{"kind": "boot-session-ms", "value": Value::Null, "epoch": 2}]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::UnobservedDecisionClock,
            "boot-session-ms",
        ),
        "an unobserved clock a decision could read is blocking",
    )
}

fn unproven_holder_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    transcript["recorded_holders"] = json!([{
        "site": "store::active_holders",
        "record_id": 7,
        "pid": 4242,
        "recorded_start_ms": 77,
        "kernel_start": Value::Null,
        "uid": 501,
        "epoch": 3,
    }]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::UnprovenHolder,
            "store::active_holders",
        ),
        "a recorded holder with a null kernel start is blocking",
    )
}

fn truncation_blocks() -> Control {
    for flag in [
        "read_rows_truncated",
        "occurrences_truncated",
        "document_truncated",
        "process_rows_truncated",
        "classifications_truncated",
    ] {
        let mut transcript = synthetic_transcript();
        transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
        transcript["notes"] = json!([
            "absolute monotonic window origin and end unproven",
            "parent identity unproven at authority-read@prune::read_registry",
        ]);
        transcript["truncation"][flag] = json!(true);
        let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
        control(
            blocked_at(&verdict, BlockingRule::Truncation, ""),
            format!("{flag} is blocking"),
        )?;
    }
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    transcript["fixture_capture"]["truncated"] = json!(true);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::FixtureCaptureTruncated,
            "fixture_capture",
        ),
        "a truncated fixture capture is blocking",
    )
}

fn collision_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([
        synthetic_row(CONTROL_SITE, "none", Some(3), Some(4)),
        synthetic_row(CONTROL_SITE, "none", Some(3), Some(4)),
    ]);
    transcript["reads"][1]["outcome"] = json!({"type": "absent"});
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_by(&verdict, BlockingRule::StrictIssueUnexplained)
            && verdict
                .strict
                .issues
                .iter()
                .any(|issue| issue.starts_with("collision at")),
        "a conflicting read under one key is preserved as blocking",
    )
}

fn effect_control(effects: Value, expected: Value) -> LimitedVerdict {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    transcript["effects"] = effects;
    let mut manifest = synthetic_manifest();
    manifest["expected_effects"] = expected;
    evaluate_limited(&transcript, &manifest, CONTROL_DIGEST)
}

fn declared_effect() -> Value {
    json!([{
        "kind": "fence",
        "site": "fence-save",
        "path": "/fixture/oxide-managed/control.json",
        "attempted": false,
    }])
}

fn attempted_effect_blocks() -> Control {
    let verdict = effect_control(
        json!([{
            "kind": "fence",
            "site": "fence-save",
            "attempted": true,
            "path": Value::Null,
            "epoch": 1,
            "result": "succeeded",
        }]),
        declared_effect(),
    );
    control(
        blocked_at(&verdict, BlockingRule::AttemptedEffect, "fence-save"),
        "an attempted effect is blocking",
    )
}

fn partial_effect_blocks() -> Control {
    let verdict = effect_control(
        json!([{
            "kind": "fence",
            "site": "fence-save",
            "attempted": false,
            "path": "/fixture/oxide-managed/control.json",
            "epoch": 1,
            "result": "partial",
        }]),
        declared_effect(),
    );
    control(
        blocked_at(&verdict, BlockingRule::UncompletedEffect, "fence-save"),
        "a partial effect is blocking",
    )
}

fn unpathed_effect_blocks() -> Control {
    let verdict = effect_control(
        json!([{
            "kind": "descriptor-effect",
            "site": "fence-save",
            "attempted": false,
            "path": Value::Null,
            "epoch": 1,
            "result": "succeeded",
        }]),
        json!([]),
    );
    control(
        blocked_at(&verdict, BlockingRule::UnpathedEffect, "fence-save"),
        "an unpathed effect is blocking",
    )
}

fn omitted_effect_blocks() -> Control {
    let verdict = effect_control(json!([]), declared_effect());
    control(
        blocked_at(&verdict, BlockingRule::OmittedEffect, "fence-save"),
        "an omitted declared effect is blocking",
    )
}

fn extra_effect_blocks() -> Control {
    let verdict = effect_control(
        json!([{
            "kind": "fence",
            "site": "fence-save",
            "attempted": false,
            "path": "/fixture/oxide-managed/control.json",
            "epoch": 1,
            "result": "succeeded",
        }]),
        json!([]),
    );
    control(
        blocked_at(&verdict, BlockingRule::ExtraEffect, "fence-save"),
        "an undeclared completed effect is blocking",
    )
}

fn substituted_effect_blocks() -> Control {
    let verdict = effect_control(
        json!([{
            "kind": "fence",
            "site": "fence-save",
            "attempted": false,
            "path": "/fixture/oxide-managed/other.json",
            "epoch": 1,
            "result": "succeeded",
        }]),
        declared_effect(),
    );
    control(
        blocked_at(&verdict, BlockingRule::SubstitutedEffect, "fence-save"),
        "a completed effect on another path is blocking",
    )
}

/// Two identical completed observations against one declaration are an extra
/// effect, and the same declaration twice against one observation is omitted:
/// multiplicity is preserved, never collapsed.
fn effect_multiplicity_is_preserved() -> Control {
    let one_effect = json!({
        "kind": "fence",
        "site": "fence-save",
        "attempted": false,
        "path": "/fixture/oxide-managed/control.json",
        "epoch": 1,
        "result": "succeeded",
    });
    let mut second = one_effect.clone();
    second["epoch"] = json!(2);
    let doubled = effect_control(json!([one_effect.clone(), second]), declared_effect());
    control(
        blocked_at(&doubled, BlockingRule::ExtraEffect, "fence-save")
            && !blocked_by(&doubled, BlockingRule::OmittedEffect),
        "a repeated completed observation is an extra effect",
    )?;
    let mut declaration = declared_effect();
    let repeat = declaration[0].clone();
    declaration.as_array_mut().expect("array").push(repeat);
    let short = effect_control(json!([one_effect]), declaration);
    control(
        blocked_at(&short, BlockingRule::OmittedEffect, "fence-save")
            && !blocked_by(&short, BlockingRule::ExtraEffect),
        "a repeated declaration is an omitted effect",
    )
}

fn numeric_window_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["window_monotonic_start_ns"] = json!(1234);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::WindowValueNotNull, "window"),
        "a numeric /2 window origin is blocking",
    )?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::WindowOriginEndNull),
        "no window limitation is granted to a numeric window",
    )
}

fn case_drift_blocks() -> Control {
    let mut manifest = synthetic_manifest();
    manifest["case"] = json!("REG-MISSING");
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_by(&verdict, BlockingRule::CaseDrift),
        "a case mismatch is blocking",
    )
}

fn digest_drift_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), &hex('2', 64));
    control(
        blocked_by(&verdict, BlockingRule::DigestDrift),
        "a manifest digest mismatch is blocking",
    )
}

fn source_drift_blocks() -> Control {
    let mut manifest = synthetic_manifest();
    manifest["source"]["oxide_tree"] = json!("not-a-tree");
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_by(&verdict, BlockingRule::SourceDrift),
        "a non-exact source identity is blocking",
    )
}

fn topology_drift_blocks() -> Control {
    let mut manifest = synthetic_manifest();
    manifest["expected"]["roots"] = json!([{"root": "root-b", "fate": "keep"}]);
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::TopologyDrift, "expected"),
        "a fixture/expected root mismatch is blocking",
    )
}

fn unbound_leaf_blocks() -> Control {
    // A file-form obligation names the leaf's own device and inode. When the
    // row carries none, the obligation's leaf identity is unproven and the
    // exclusion cannot be granted.
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", None, None)]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::WrongLeafIdentity, CONTROL_SITE),
        "a file obligation whose row binds no own identity is unproven",
    )?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent),
        "no path-only limitation without a bound leaf",
    )
}

/// The recorder's per-row identity sentence is not a defect by itself on a row
/// the manifest does not declare as a decision input.
fn informative_identity_note() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "object or parent identity unproven at read-bounded:open",
        "component parent identity unproven at absence-component",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        !blocked_by(&verdict, BlockingRule::UnboundObjectIdentity)
            && !blocked_by(&verdict, BlockingRule::UnboundComponentParent),
        "an undeclared row's identity note does not become a policy defect",
    )
}

/// The same sentence on a declared decision input blocks.
fn declared_input_identity_note() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "object or parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::UnboundObjectIdentity, CONTROL_SITE),
        "a declared input's identity note blocks",
    )
}

/// An obligation that does not ask for the parent leaves nothing to exclude, so
/// no limitation is reported and the parent sentence is not a defect.
fn unrequired_parent_is_not_an_exclusion() -> Control {
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"][0]["parent_required"] = json!(false);
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    all_pass(&verdict)?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent),
        "an unrequired parent is not reported as an exclusion",
    )?;
    control(
        verdict.strict.issues == vec![STRICT_WINDOW_ISSUE.to_owned()],
        format!(
            "the strict judgement reports only the window issue: {:?}",
            verdict.strict.issues
        ),
    )
}

/// Two path-only obligations get two limitations, each bound to its own site and
/// its own leaf identity: an exclusion is never a generic warning-text waiver.
fn second_path_only_site_blocks() -> Control {
    const SECOND_SITE: &str = "read-bounded:lstat";
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"] = json!([
        {
            "engine": "oxide",
            "site": CONTROL_SITE,
            "parent_required": true,
            "identity": {"path": CONTROL_PATH, "device": 3, "inode": 4},
        },
        {
            "engine": "oxide",
            "site": SECOND_SITE,
            "parent_required": true,
            "identity": {"path": CONTROL_PATH, "device": 7, "inode": 8},
        },
    ]);
    let mut second_row = synthetic_row(SECOND_SITE, "none", Some(7), Some(8));
    second_row["occurrence"] = json!(1);
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([
        synthetic_row(CONTROL_SITE, "none", Some(3), Some(4)),
        second_row,
    ]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "parent identity unproven at read-bounded:lstat",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    all_pass(&verdict)?;
    let named: Vec<(&str, &str)> = verdict
        .limitations
        .iter()
        .filter(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent)
        .map(|limitation| (limitation.site.as_str(), limitation.leaf_identity.as_str()))
        .collect();
    control(
        named.len() == 2,
        format!("expected two site-bound limitations, got {named:?}"),
    )?;
    control(
        named
            .iter()
            .any(|(site, leaf)| *site == CONTROL_SITE && leaf.contains("device=3")),
        "the first limitation names its own site and leaf",
    )?;
    control(
        named
            .iter()
            .any(|(site, leaf)| *site == SECOND_SITE && leaf.contains("device=7")),
        "the second limitation names its own site and leaf",
    )?;
    control(
        !named
            .iter()
            .any(|(site, leaf)| site.is_empty() || leaf.is_empty()),
        "no limitation is attributable to generic text",
    )
}

/// The exact shape the recorder produces for a non-`read_bounded` authority
/// read: no own identity and no parent, plus the recorder's own four notes. The
/// reviewed path-only exclusion cannot rescue it, so the case is insufficient.
fn recorder_path_only_row_is_insufficient() -> Control {
    let mut row = synthetic_row(CONTROL_SITE, "none", None, None);
    row["outcome"] = json!({"type": "digest", "digest": hex('9', 64)});
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([row]);
    // Exactly the sentences the recorder emits for this site: `read_registry`
    // goes through `read_row` (object-or-parent unproven), not
    // `read_row_owned` (parent unproven).
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "descriptor identity unavailable at authority-read@prune::read_registry: no authorized \
descriptor observation",
        "object or parent identity unproven at authority-read@prune::read_registry",
        "extra or unbound observation at authority-read@prune::read_registry",
        "mandatory observation missing or identity unproven at \
authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::ExtraUnboundObservation,
            CONTROL_SITE,
        ),
        "the recorder's own unbound verdict blocks",
    )?;
    control(
        blocked_at(
            &verdict,
            BlockingRule::RecorderObligationUnmet,
            CONTROL_SITE,
        ),
        "the recorder's unmet-obligation note blocks",
    )?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent),
        "the path-only exclusion is not granted to a row the recorder does not bind",
    )
}

/// The recorder's malformed-obligation sentence has its own blocking rule, so it
/// is attributed rather than reading as an unknown note.
fn recorder_malformed_obligation_note_blocks() -> Control {
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
        "obligation missing parent_required at authority-read@prune::read_registry",
    ]);
    let verdict = evaluate_limited(&transcript, &synthetic_manifest(), CONTROL_DIGEST);
    control(
        blocked_at(
            &verdict,
            BlockingRule::RecorderObligationMalformed,
            CONTROL_SITE,
        ),
        "the malformed-obligation note blocks under its own rule",
    )?;
    control(
        !blocked_by(&verdict, BlockingRule::UnknownNote),
        "the malformed-obligation note is not an unknown note",
    )
}

/// The plan's printed blocking vocabulary is generated from the enum's own list,
/// so it cannot omit a rule the verdict can return.
fn plan_blocking_vocabulary() -> Control {
    let printed: Vec<&str> = ALL_BLOCKING_RULES.iter().map(|rule| rule.name()).collect();
    let mut unique = printed.clone();
    unique.sort_unstable();
    unique.dedup();
    control(
        unique.len() == printed.len(),
        "two blocking rules print the same name",
    )?;
    let plan = plan_document();
    let documented: Vec<&str> = plan["blocking_rules"]
        .as_array()
        .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
        .unwrap_or_default();
    control(
        documented == printed,
        format!("the plan prints {documented:?}, the enum declares {printed:?}"),
    )?;
    for rule in [
        BlockingRule::RecorderObligationUnmet,
        BlockingRule::RecorderObligationMalformed,
        BlockingRule::StrictIssueUnexplained,
    ] {
        control(
            documented.contains(&rule.name()),
            format!("the plan omits {}", rule.name()),
        )?;
    }
    Ok(())
}

/// `prune::read_bounded` does record the leaf's own device and inode, and the
/// exclusion's leaf requirement is met there, but the fixture binding still
/// fails because the row carries no parent.
fn recorder_bound_leaf_without_binding_is_insufficient() -> Control {
    const SITE: &str = "authority-read@prune::read_bounded";
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"][0]["site"] = json!(SITE);
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_bounded",
        "extra or unbound observation at authority-read@prune::read_bounded",
        "mandatory observation missing or identity unproven at \
authority-read@prune::read_bounded",
    ]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::ExtraUnboundObservation, SITE),
        "a path-only row the recorder never binds stays insufficient",
    )?;
    control(
        blocked_at(&verdict, BlockingRule::RecorderObligationUnmet, SITE),
        "the unmet-obligation note blocks",
    )?;
    control(
        !verdict
            .limitations
            .iter()
            .any(|limitation| limitation.rule == ExclusionRule::PathOnlyDirectReadParent),
        "no limitation is granted while the recorder reports the row unbound",
    )?;
    control(!verdict.sufficient(), "the case is insufficient")
}

/// An obligation at a kind the recorder really emits for the absence walk, with
/// a handle-class parent: the K1 rule must be the named rule.
fn emitted_absence_kind_blocks() -> Control {
    const SITE: &str = "absence-walk:fstatat";
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"] = json!([{
        "engine": "oxide",
        "site": SITE,
        "parent_required": true,
        "identity": {"component": "checkout", "parent_device": 1, "parent_inode": 2},
    }]);
    let mut row = synthetic_row(SITE, "handle", None, None);
    row["identity"]["path"] = Value::Null;
    row["identity"]["component"] = json!("checkout");
    row["identity"]["parent_device"] = json!(1);
    row["identity"]["parent_inode"] = json!(2);
    row["outcome"] = json!({"type": "absent"});
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([row]);
    transcript["notes"] = json!(["absolute monotonic window origin and end unproven"]);
    let verdict = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    control(
        blocked_at(&verdict, BlockingRule::HandleParentUnqualified, SITE),
        "an emitted absence-walk kind is classified handle-class",
    )?;
    control(
        !blocked_at(&verdict, BlockingRule::UnlistedSite, SITE),
        "the emitted kind is listed, not unlisted",
    )
}

/// The entry point parses with the canonical /2 parsers first.
fn parser_entry_point_refuses_non_v2() -> Control {
    let good = evaluate_limited_capture(
        include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/v2/valid/handle-parent.json"
        ),
        include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/v2/manifest/valid-parent-required.json"
        ),
    );
    control(
        good.is_ok(),
        format!("a canonical /2 pair must evaluate: {:?}", good.err()),
    )?;
    let historical = evaluate_limited_capture(
        include_bytes!("../../../docs/knowledge/contracts/capture-vectors/v2/invalid/historical-v1.json"),
        include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/v2/manifest/valid-parent-required.json"
        ),
    );
    control(
        historical.is_err()
            && historical
                .err()
                .is_some_and(|error| error.starts_with("transcript:")),
        "a /1 transcript is refused by the parser before any verdict",
    )?;
    let bad_manifest = evaluate_limited_capture(
        include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/v2/valid/handle-parent.json"
        ),
        include_bytes!(
            "../../../docs/knowledge/contracts/capture-vectors/v2/manifest/invalid-extra-field.json"
        ),
    );
    control(
        bad_manifest.is_err()
            && bad_manifest
                .err()
                .is_some_and(|error| error.starts_with("manifest:")),
        "an invalid manifest is refused by the parser before any verdict",
    )
}

fn strict_preservation() -> Control {
    // The window issue is explained only by a null window, so a strict issue
    // the two fixed templates cannot explain always blocks.
    let mut transcript = synthetic_transcript();
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let manifest = synthetic_manifest();
    let sufficient = evaluate_limited(&transcript, &manifest, CONTROL_DIGEST);
    all_pass(&sufficient)?;
    // Drop the read row that binds the obligation's identity: the strict
    // judgement and the limited verdict must both fail.
    let mut broken = transcript.clone();
    broken["reads"] = json!([]);
    let verdict = evaluate_limited(&broken, &manifest, CONTROL_DIGEST);
    control(
        !verdict.sufficient()
            && blocked_at(
                &verdict,
                BlockingRule::MissingMandatoryObservation,
                CONTROL_SITE,
            )
            && blocked_by(&verdict, BlockingRule::StrictIssueUnexplained),
        "a strict insufficiency is retained as blocking",
    )
}

fn outcome(fates: &[(&str, &str)], complete: bool) -> CaseOutcome {
    CaseOutcome {
        decision: Decision::Evaluated {
            batch_complete: complete,
            roots: fates
                .iter()
                .map(|(root, fate)| ((*root).to_owned(), (*fate).to_owned()))
                .collect(),
        },
        expected: vec![("root-a".to_owned(), "keep".to_owned())],
    }
}

fn sufficient_verdict(engine: &str) -> LimitedVerdict {
    let mut transcript = synthetic_transcript();
    transcript["engine"] = json!(engine);
    transcript["reads"] = json!([synthetic_row(CONTROL_SITE, "none", Some(3), Some(4))]);
    transcript["notes"] = json!([
        "absolute monotonic window origin and end unproven",
        "parent identity unproven at authority-read@prune::read_registry",
    ]);
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"][0]["engine"] = json!(engine);
    evaluate_limited(&transcript, &manifest, CONTROL_DIGEST)
}

fn insufficient_verdict(engine: &str) -> LimitedVerdict {
    let mut transcript = synthetic_transcript();
    transcript["engine"] = json!(engine);
    let mut manifest = synthetic_manifest();
    manifest["mandatory_observations"][0]["engine"] = json!(engine);
    evaluate_limited(&transcript, &manifest, CONTROL_DIGEST)
}

fn identical_decisions() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = insufficient_verdict("barge");
    let subject = outcome(&[("root-a", "keep")], true);
    let verdict = limited_comparison(&oxide, &subject, &barge, &subject);
    control(
        verdict.state == ComparisonState::Insufficient,
        "identical decisions over insufficient inputs are not agreement",
    )?;
    control(
        verdict.barge_sufficient == false && verdict.oxide_sufficient,
        "the insufficient side is named",
    )
}

fn deferred_is_not_agreement() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = sufficient_verdict("barge");
    let mut deferred = outcome(&[("root-a", "keep")], true);
    deferred.decision = Decision::NotRun {
        reason: "case deferred to the second batch".to_owned(),
    };
    let verdict = limited_comparison(&oxide, &deferred, &barge, &deferred);
    control(
        verdict.state == ComparisonState::Insufficient,
        "a deferred case is never counted as agreement",
    )
}

fn divergence_is_not_agreement() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = sufficient_verdict("barge");
    let subject = outcome(&[("root-a", "keep")], true);
    let divergent = outcome(&[("root-a", "release")], true);
    let verdict = limited_comparison(&oxide, &subject, &barge, &divergent);
    control(
        verdict.state == ComparisonState::Divergence,
        "an observed divergence is reported, not folded into agreement",
    )
}

fn agreement_needs_both() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = sufficient_verdict("barge");
    let subject = outcome(&[("root-a", "keep")], true);
    let verdict = limited_comparison(&oxide, &subject, &barge, &subject);
    control(
        verdict.state == ComparisonState::Agreed,
        "two sufficient sides with the declared fates agree",
    )?;
    let incomplete = outcome(&[("root-a", "keep")], false);
    let verdict = limited_comparison(&oxide, &incomplete, &barge, &incomplete);
    control(
        verdict.state == ComparisonState::Insufficient,
        "an incomplete batch is insufficient, not agreement and not divergence",
    )
}

/// A Barge-only incomplete batch with fates identical to Oxide's is never
/// agreement: the incomplete side alone decides the result.
fn barge_only_incomplete_is_insufficient() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = sufficient_verdict("barge");
    let complete = outcome(&[("root-a", "keep")], true);
    let barge_incomplete = outcome(&[("root-a", "keep")], false);
    let verdict = limited_comparison(&oxide, &complete, &barge, &barge_incomplete);
    control(
        verdict.state == ComparisonState::Insufficient,
        format!(
            "a Barge-only incomplete batch is insufficient, got {:?}",
            verdict.state
        ),
    )?;
    control(
        verdict
            .reasons
            .iter()
            .any(|reason| reason.contains("barge reported an incomplete")),
        "the incomplete side is named",
    )
}

/// Agreement between the engines on fates the manifest does not declare is a
/// declared mismatch, never a cross-engine divergence.
fn declared_mismatch_is_not_divergence() -> Control {
    let oxide = sufficient_verdict("oxide");
    let barge = sufficient_verdict("barge");
    let observed = outcome(&[("root-a", "release")], true);
    let verdict = limited_comparison(&oxide, &observed, &barge, &observed);
    control(
        verdict.state == ComparisonState::DeclaredMismatch,
        format!(
            "an agreeing pair that contradicts the declaration is a declared mismatch, got {:?}",
            verdict.state
        ),
    )
}

fn plan_controls() -> Vec<(&'static str, fn() -> Control)> {
    vec![
        ("every original case appears once", plan_case_inventory),
        ("deferred set is exact", plan_deferred_set),
        ("every case names all seven CFs", plan_cf_complete),
        ("enumerations imply CF-1", plan_cf1_consistency),
        (
            "no case is runnable and both gates are named",
            plan_export_gate,
        ),
        ("no case claims execution", plan_no_execution_claim),
        (
            "the next occurrence names the qualified runner",
            plan_next_occurrence,
        ),
        ("the nine premises are all disposed", plan_premises),
    ]
}

fn plan_case_inventory() -> Control {
    let rows = planned_cases();
    let mut ids: Vec<&str> = rows.iter().map(|row| row.id).collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    control(
        ids.len() == total,
        format!("{total} rows expand with duplicate case ids"),
    )?;
    for family in FAMILIES {
        control(!family.cases.is_empty(), "a family declares no cases")?;
    }
    control(
        total == 50,
        format!("the finite inventory has {total} rows, expected 50"),
    )
}

const DEFERRED_CASES: &[&str] = &[
    "WRT-LIVE-THIS",
    "WRT-LIVE-OTHER",
    "WRT-UNKNOWN",
    "WRT-ACQUIRING",
    "WRT-RETIRED-CLEAN",
    "EFFECT-1",
    "EFFECT-2",
];

fn plan_deferred_set() -> Control {
    let rows = planned_cases();
    let mut deferred: Vec<&str> = rows
        .iter()
        .filter(|row| row.status == CaseStatus::Deferred)
        .map(|row| row.id)
        .collect();
    deferred.sort_unstable();
    let mut expected = DEFERRED_CASES.to_vec();
    expected.sort_unstable();
    control(
        deferred == expected,
        format!("deferred cases are {deferred:?}, not {expected:?}"),
    )?;
    // A deferred or insufficient case is never counted as agreement.
    for row in &rows {
        if row.status != CaseStatus::Runnable {
            control(!row.reason.is_empty(), format!("{} has no reason", row.id))?;
        }
    }
    control(
        rows.iter()
            .any(|row| row.id == "CAP-1" && row.status == CaseStatus::Insufficient),
        "CAP-1 stays insufficient",
    )
}

fn plan_cf_complete() -> Control {
    for row in planned_cases() {
        let mut named: Vec<&str> = row.cf.iter().map(|cf| cf.name()).collect();
        named.extend(row.cf_absent.iter().map(|(cf, _)| *cf));
        named.sort_unstable();
        let mut expected: Vec<&str> = vec!["CF-1", "CF-2", "CF-3", "CF-4", "CF-5", "CF-6", "CF-7"];
        expected.sort_unstable();
        control(
            named == expected,
            format!("{} names CFs {named:?}, expected all seven", row.id),
        )?;
        for (cf, reason) in &row.cf_absent {
            control(
                !reason.is_empty(),
                format!("{} leaves {cf} without a reason", row.id),
            )?;
        }
        control(
            !row.reason.is_empty() && !row.entrypoints.is_empty(),
            format!("{} has no reason or entrypoint", row.id),
        )?;
        control(
            !row.enumerations.is_empty() || row.cf_absent.iter().any(|(cf, _)| *cf == "CF-1"),
            format!("{} enumerates nothing without saying so", row.id),
        )?;
        control(row.cf.contains(&Cf::Cf4), format!("{} drops CF-4", row.id))?;
    }
    Ok(())
}

fn plan_cf1_consistency() -> Control {
    for family in FAMILIES {
        let enumerates = !family.enumerations.is_empty();
        let names_cf1 = family.cf.contains(&Cf::Cf1);
        let denies_cf1 = family.cf_absent.iter().any(|(cf, _)| *cf == "CF-1");
        control(
            (enumerates && names_cf1) || (!enumerates && denies_cf1),
            format!(
                "family {} enumerates={enumerates} but cf-1 named={names_cf1} denied={denies_cf1}",
                family.name
            ),
        )?;
    }
    // The registry-shape family decides on the registry file and creates no
    // outputs directory, so its CF-1 non-applicability is a fixture fact.
    let registry = FAMILIES
        .iter()
        .find(|family| family.name == "registry shape and caps")
        .expect("registry family");
    control(
        registry.enumerations.is_empty() && registry.cf_absent.iter().any(|(cf, _)| *cf == "CF-1"),
        "registry-shape applicability follows the fixture's absence of an outputs dir",
    )
}

fn plan_export_gate() -> Control {
    let plan = plan_document();
    control(
        plan["executable_today"] == json!(false),
        "the plan never claims a case is executable",
    )?;
    let rows = planned_cases();
    let runnable: Vec<&str> = rows
        .iter()
        .filter(|row| row.status == CaseStatus::Runnable)
        .map(|row| row.id)
        .collect();
    control(
        runnable.is_empty(),
        format!("no case is runnable under the current recorder, but got {runnable:?}"),
    )?;
    let mut counts = (0usize, 0usize, 0usize);
    for row in &rows {
        match row.status {
            CaseStatus::Runnable => counts.0 += 1,
            CaseStatus::Insufficient => counts.1 += 1,
            CaseStatus::Deferred => counts.2 += 1,
        }
        control(
            row.prerequisites.contains(&Prerequisite::ExportClosure),
            format!("{} does not name the export gate", row.id),
        )?;
        control(
            row.prerequisites.contains(&Prerequisite::BargeCaseSupport),
            format!("{} does not name Barge case support", row.id),
        )?;
    }
    control(
        counts == (0, 43, 7),
        format!("plan status counts are {counts:?}, expected (0, 43, 7)"),
    )?;
    control(
        plan["binding_gap"]["summary"].as_str() == Some(BINDING_GAP),
        "the plan names the recorder binding gap",
    )?;
    control(
        plan["barge_support"]["summary"].as_str() == Some(BARGE_SUPPORT_REQUIREMENT)
            && plan["barge_support"]["smallest_requirement"]
                .as_str()
                .is_some_and(|text| text.contains("per-case fixture builder")),
        "the plan names the exact smallest Barge source requirement",
    )?;
    control(
        plan["binding_gap"]["site_classes"]
            .as_array()
            .is_some_and(|sites| sites.len() == KNOWN_SITES.len()),
        "the plan prints every registered site and class",
    )
}

fn plan_no_execution_claim() -> Control {
    let plan = plan_document();
    let text = serde_json::to_string(&plan).expect("plan serialises");
    for forbidden in ["qualified: true", "passed", "parity achieved"] {
        control(
            !text.contains(forbidden),
            format!("the plan must not claim {forbidden}"),
        )?;
    }
    control(
        plan["execution_gate"]
            .as_str()
            .is_some_and(|gate| gate.contains("no export-closure")),
        "the plan names the unmet export gate",
    )?;
    control(
        plan["claim"].as_str() == Some(CLAIM),
        "the plan carries the single L2 claim",
    )
}

fn plan_next_occurrence() -> Control {
    let plan = plan_document();
    let next = &plan["next_occurrence"];
    control(
        next["runner_sha256"].as_str() == Some(QUALIFIED_RUNNER_SHA256),
        "the next occurrence names the qualified runner digest",
    )?;
    control(
        next["wall_clock_seconds"].as_u64() == Some(600)
            && next["scratch_mib"].as_u64() == Some(512),
        "the proposed bounds are 600 s and 512 MiB",
    )?;
    control(
        next["first_batch"]["tier"].as_str() == Some("ELEVATED")
            && next["second_batch"]["tier"].as_str() == Some("ELEVATED"),
        "both batches are ELEVATED because both attach one owned image at a /Volumes mount",
    )?;
    control(
        next["images"]
            .as_str()
            .is_some_and(|images| !images.contains("second batch only")),
        "the image is not described as second-batch only",
    )?;
    control(
        next["scratch_interface"]
            .as_str()
            .is_some_and(|text| text.contains("8A660440-C919-45C6-A3F1-3365B9BD2006")),
        "the scratch placement names the lane's Stash volume",
    )?;
    control(
        next["command"]
            .as_str()
            .is_some_and(|command| command.contains("bounded-test.py --limit 600")),
        "the proposed command uses the qualified supervisor",
    )?;
    control(
        next["execution_authority"]
            .as_str()
            .is_some_and(|text| text.starts_with("none")),
        "the plan grants no execution authority",
    )
}

fn plan_premises() -> Control {
    control(
        PREMISE_DISPOSITIONS.len() == 9,
        "the nine native-proof premises are all disposed",
    )?;
    for (premise, disposition) in PREMISE_DISPOSITIONS {
        control(
            !premise.is_empty() && !disposition.is_empty(),
            format!("premise {premise} has no disposition"),
        )?;
    }
    let plan = plan_document();
    control(
        plan["next_occurrence"]["premises"]
            .as_array()
            .is_some_and(|premises| premises.len() == 9),
        "the plan prints all nine premises",
    )
}
