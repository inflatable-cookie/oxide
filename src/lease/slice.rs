//! Bounded one-use slice membership for library admission
//! (`one-use-workspaces` §6.1, §6.2, §6.5).
//!
//! The front door owns the classification and every control-record fact; the
//! library owns the ledger. These types are the whole interface between them:
//!
//! - [`LeaseRequest::class`](super::LeaseRequest) says which class the
//!   admission belongs to. It is `Normal` for every production command while
//!   the feature is dormant.
//! - [`OneUseSlice`] carries the bounded pair set the front door derived, the
//!   two thresholds of §6.1 and the count of class-unknown ledger rows.
//! - [`VictimPolicy`] says which victims a pressure path may select. `Any` is
//!   today's behaviour and the default for a caller outside this feature;
//!   `Prefer` is a normal admission (an evictable one-use member first, then
//!   the ordinary fallback with every member removed); `Only` is a one-use
//!   admission, which may never choose a normal slot.
//!
//! A pair is only a *candidate* here. The library re-verifies every pair
//! against the ledger under the pool lock (§6.2) and drops one that does not
//! verify; it is never replaced by another slot and never becomes a victim.
//! This module carries no custody claim: which members are `evictable` is a
//! front-door decision from control state it holds under its admission lock
//! (§5.5.2), and the library trusts only that subset for a one-use deletion.

use super::MAX_SLOTS;

/// The hard ceiling on one-use members per pool (§6.5), independent of
/// [`MAX_SLOTS`] and of the normal eight-slot project cap.
pub const ONE_USE_MAX_SLOTS: usize = 16;

/// The byte cap on the one-use member union (§6.5): a tenth of this
/// admission's effective soft budget, capped at 32 GiB.
pub const ONE_USE_MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;

/// The bounded slice byte bound for one admission's effective soft budget.
/// Never zero: a slice is a bound, not an OS ceiling.
pub fn slice_byte_limit(admission_budget: u64) -> u64 {
    (admission_budget / 10).min(ONE_USE_MAX_BYTES).max(1)
}

/// The admission class of one lease request (§6.1). `Normal` is the default
/// and the only class a dormant build produces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AdmissionClass {
    #[default]
    Normal,
    OneUse,
}

/// One `(slot_id, incarnation)` binding (§6.1, §6.2). Both parts are compared
/// literally against a ledger row; neither is interpreted as an identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MemberPair {
    pub slot_id: String,
    pub incarnation: String,
}

impl MemberPair {
    pub fn new(slot_id: impl Into<String>, incarnation: impl Into<String>) -> Self {
        Self {
            slot_id: slot_id.into(),
            incarnation: incarnation.into(),
        }
    }
}

/// A bounded set of `(slot_id, incarnation)` pairs (§6.1, §6.5).
///
/// At most [`MAX_SLOTS`] pairs are kept; a caller supplying more produces a
/// set that [`MemberSet::is_bounded`] refuses, rather than a silently
/// truncated one. Exact duplicate pairs are removed so one pair cannot be
/// counted twice by a member count or walked twice by the union.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MemberSet {
    pairs: Vec<MemberPair>,
}

impl MemberSet {
    pub fn new(pairs: impl IntoIterator<Item = MemberPair>) -> Self {
        let mut set = Self::empty();
        for pair in pairs {
            if !set.pairs.contains(&pair) {
                set.pairs.push(pair);
            }
        }
        set
    }

    pub fn empty() -> Self {
        Self { pairs: Vec::new() }
    }

    /// Whether the set fits the ledger's own slot bound. An over-bound set is
    /// refused before any use; it is never truncated.
    pub fn is_bounded(&self) -> bool {
        self.pairs.len() <= MAX_SLOTS
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    pub fn contains(&self, pair: &MemberPair) -> bool {
        self.pairs.contains(pair)
    }

    pub fn iter(&self) -> impl Iterator<Item = &MemberPair> {
        self.pairs.iter()
    }
}

impl FromIterator<MemberPair> for MemberSet {
    fn from_iter<I: IntoIterator<Item = MemberPair>>(iter: I) -> Self {
        Self::new(iter)
    }
}

/// Which victims a pressure path may select (§6.7).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum VictimPolicy {
    /// Today's behaviour: the least-recently-used eligible idle whole slot.
    /// The default for a library caller that passes no policy.
    #[default]
    Any,
    /// A normal admission: an eligible one-use member first, then the ordinary
    /// order with every one-use pair (`OneUseSlice::members`) removed.
    Prefer { preferred: MemberSet },
    /// A one-use admission: eligible one-use members only. A normal slot, a
    /// class-unknown orphan, a protected member and an unverified pair are
    /// all outside this set.
    Only { preferred: MemberSet },
}

impl VictimPolicy {
    /// The pairs this policy prefers, which is empty for [`VictimPolicy::Any`].
    pub fn preferred(&self) -> &MemberSet {
        match self {
            Self::Any => empty_members(),
            Self::Prefer { preferred } | Self::Only { preferred } => preferred,
        }
    }

    pub fn is_class_scoped(&self) -> bool {
        !matches!(self, Self::Any)
    }
}

fn empty_members() -> &'static MemberSet {
    static EMPTY: std::sync::OnceLock<MemberSet> = std::sync::OnceLock::new();
    EMPTY.get_or_init(MemberSet::empty)
}

/// Which kind of one-use admission restore is deciding (§6.1, §6.3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SliceAdmission {
    /// A new one-use slot: one free place is required after restoration
    /// (`member_count <= max_slots - 1`).
    NewSlot,
    /// A warm reuse of an existing member: no new slot, so `member_count ==
    /// max_slots` is admissible while the bytes fit.
    WarmReuse,
}

/// What restore measured and whether the slice admits (§6.1, §6.3, §6.9).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreOutcome {
    /// No verified members and no class-unknown rows: no walk, no write.
    NotNeeded,
    /// Every member is within the byte bound and every count threshold met.
    Within { bytes: u64 },
    /// A normal admission continues with its measured overage (§6.8).
    Over { bytes: u64 },
    /// A threshold is still reached and no eligible victim remains: a one-use
    /// admission refuses (§6.9). `held` counts the protected members.
    Full { bytes: u64, held: usize },
    /// Class-unknown ledger rows are present, so the figure is a lower bound
    /// (§5.6). A one-use admission refuses; a normal one continues.
    LowerBound { bytes: u64, unknown: usize },
    /// The bounded union walk did not complete. Never read as zero and never
    /// as an exact overage (§6.4, §6.8).
    Unmeasured { reason: String },
}

/// One restore observation, as the library reports it (§6.1 layer split: the
/// library reports ledger facts; the front door decides what a held member
/// means and what to print).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SliceReport {
    pub outcome: RestoreOutcome,
    /// Verified accounting members after restoration.
    pub members: usize,
    /// Verified members that were not evictable, so never a victim here.
    pub protected: usize,
    /// Accounting pairs that did not verify against the ledger and were
    /// dropped from every use.
    pub unverified: usize,
    pub max_slots: usize,
    pub max_bytes: u64,
}

impl SliceReport {
    /// The measured union bytes, when this observation measured one.
    pub fn measured(&self) -> Option<u64> {
        match self.outcome {
            RestoreOutcome::NotNeeded
            | RestoreOutcome::LowerBound { .. }
            | RestoreOutcome::Unmeasured { .. } => None,
            RestoreOutcome::Within { bytes }
            | RestoreOutcome::Over { bytes }
            | RestoreOutcome::Full { bytes, .. } => Some(bytes),
        }
    }
}

/// The bounded slice request carried by a new-slot or one-use admission
/// (§6.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OneUseSlice {
    /// Every member pair the front door derived, i.e. the accounting set.
    /// `VictimPolicy::preferred` is the evictable subset of this set.
    pub members: MemberSet,
    pub max_slots: usize,
    pub max_bytes: u64,
    pub kind: SliceAdmission,
    /// Class-unknown ledger rows (§5.6). Never zero free quota.
    pub unknown: usize,
}

impl OneUseSlice {
    /// The production bounds of §6.5 for one admission's effective soft
    /// budget.
    pub fn bounded(
        members: MemberSet,
        kind: SliceAdmission,
        unknown: usize,
        admission_budget: u64,
    ) -> Self {
        Self {
            members,
            max_slots: ONE_USE_MAX_SLOTS,
            max_bytes: slice_byte_limit(admission_budget),
            kind,
            unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_set_is_bounded_and_never_truncated() {
        let pairs = (1..=MAX_SLOTS as u64).map(|id| MemberPair::new(id.to_string(), "1"));
        assert_eq!(MemberSet::new(pairs).len(), MAX_SLOTS);
        let over = (1..=MAX_SLOTS as u64 + 1).map(|id| MemberPair::new(id.to_string(), "1"));
        let set = MemberSet::new(over);
        assert_eq!(set.len(), MAX_SLOTS + 1);
        assert!(
            !set.is_bounded(),
            "an over-bound set must be refused, not truncated"
        );
    }

    #[test]
    fn exact_duplicate_pairs_are_counted_once_and_stale_ones_are_kept() {
        let pair = MemberPair::new("7", "1");
        let stale = MemberPair::new("7", "2");
        let set = MemberSet::new([pair.clone(), pair.clone(), stale.clone()]);
        assert_eq!(set.len(), 2, "the exact duplicate was counted twice");
        assert!(set.contains(&pair) && set.contains(&stale));
    }

    #[test]
    fn the_slice_byte_bound_is_a_tenth_capped_and_never_zero() {
        assert_eq!(slice_byte_limit(0), 1);
        assert_eq!(slice_byte_limit(9), 1);
        assert_eq!(slice_byte_limit(1_000), 100);
        assert_eq!(
            slice_byte_limit(1000 * 1024 * 1024 * 1024),
            ONE_USE_MAX_BYTES
        );
        let bounded = OneUseSlice::bounded(MemberSet::empty(), SliceAdmission::NewSlot, 0, 1000);
        assert_eq!(bounded.max_slots, ONE_USE_MAX_SLOTS);
        assert_eq!(bounded.max_bytes, 100);
    }

    #[test]
    fn any_is_the_default_policy_and_names_no_member() {
        let policy = VictimPolicy::default();
        assert_eq!(policy, VictimPolicy::Any);
        assert!(!policy.is_class_scoped());
        assert!(policy.preferred().is_empty());
    }
}
