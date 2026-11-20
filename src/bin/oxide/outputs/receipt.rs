//! Owned-output receipts, root markers, serialization and file digests.

use super::{MARKER, MEMBER};
use crate::process;
use crate::store;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

pub(in crate::outputs) const RECEIPT_VERSION: &str = "oxide.outputs.transition/2";
/// The bound successor receipt (oxide#133): `target` stays a real directory
/// and each positively identified profile member is an owned link. A legacy
/// receipt is never written in this form without an explicit migration.
pub(in crate::outputs) const PROFILE_RECEIPT_VERSION: &str = "oxide.outputs.transition/3";
/// The whole-target receipt oxide#117 published. Prune reads a valid one as
/// its current in-memory whole-target form (oxide#152) so a legacy root is
/// not globally unprunable; every other reader still refuses it, and reading
/// or a dry run never rewrites it.
pub(in crate::outputs) const LEGACY_RECEIPT_VERSION: &str = "oxide.outputs.transition/1";
pub(in crate::outputs) const MARKER_VERSION: &str = "oxide.outputs.root/2";
pub(in crate::outputs) const LEGACY_MARKER_VERSION: &str = "oxide.outputs.root/1";

pub(in crate::outputs) const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
pub(in crate::outputs) const MAX_MARKER_BYTES: u64 = 64 * 1024;

/// Known roots kept per receipt; the oldest is dropped first.
const MAX_ROOTS: usize = 8;

/// Which real root a link selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    External,
    Local,
}

impl RootKind {
    pub(in crate::outputs) fn label(self) -> &'static str {
        match self {
            Self::External => "external",
            Self::Local => "local fallback",
        }
    }
}

/// One selected real output root and the volume it is bound to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub(crate) kind: RootKind,
    pub(crate) root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) volume_uuid: Option<String>,
    /// The root's immutable 128-bit nonce. Absent only for a root created
    /// before oxide#129; a recreated root always gets a fresh one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) nonce: Option<String>,
    /// The generation this selection was committed at.
    pub(crate) generation: u64,
}

impl Selected {
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn kind(&self) -> RootKind {
        self.kind
    }

    pub(crate) fn volume_uuid(&self) -> Option<&str> {
        self.volume_uuid.as_deref()
    }

    pub(crate) fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(in crate::outputs) fn same_place(&self, other: &Self) -> bool {
        self.kind == other.kind && self.root == other.root && self.volume_uuid == other.volume_uuid
    }

    pub(in crate::outputs) fn view(&self) -> SelectedView {
        SelectedView {
            kind: self.kind,
            root: self.root.clone(),
            volume_uuid: self.volume_uuid.clone(),
            generation: self.generation,
        }
    }

    /// Bind a fresh nonce to a newly created root, or keep the root's own.
    pub(in crate::outputs) fn with_nonce(mut self, nonce: Option<String>) -> Self {
        self.nonce = nonce;
        self
    }

    pub(in crate::outputs) fn at(&self, generation: u64) -> Self {
        Self {
            generation,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(in crate::outputs) struct SelectedView {
    pub(in crate::outputs) kind: RootKind,
    pub(in crate::outputs) root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) volume_uuid: Option<String>,
    pub(in crate::outputs) generation: u64,
}

/// A transition written ahead but not committed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prepared {
    pub(crate) generation: u64,
    pub(crate) to: Selected,
    pub(in crate::outputs) link: PathBuf,
}

/// Receipt state. `Prepared` is the only state a crash can leave behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::outputs) enum ReceiptState {
    Committed,
    Prepared,
    Detached,
    /// The selected root was deleted or resolved by an earlier explicit
    /// operator prune.
    Pruned,
}

/// How a receipt places this checkout's final outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LayoutKind {
    /// oxide#117/#129: `<checkout>/target` is one owned link to the root.
    /// Legacy receipts keep this format until the user switches via `detach` +
    /// `adopt`.
    WholeTarget,
    /// The bound profile layout: `<checkout>/target` stays a real directory
    /// and each profile member is an owned link into the root.
    Profile,
}

impl LayoutKind {
    /// The receipt format this layout is published in: a legacy receipt is
    /// never silently rewritten as a successor.
    fn receipt_version(self) -> &'static str {
        match self {
            Self::WholeTarget => RECEIPT_VERSION,
            Self::Profile => PROFILE_RECEIPT_VERSION,
        }
    }
}

/// The checkout's owned-output receipt, keyed by canonical checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Receipt {
    pub(in crate::outputs) version: String,
    pub(in crate::outputs) checkout: PathBuf,
    pub(in crate::outputs) checkout_id: String,
    /// Last committed generation; zero before the first conversion.
    pub(in crate::outputs) generation: u64,
    pub(in crate::outputs) state: ReceiptState,
    pub(in crate::outputs) layout: LayoutKind,
    /// The owned placements of this layout: exactly one `target` placement
    /// for a legacy receipt, one per positively identified profile member for
    /// the bound profile layout. Roots hold immutable nonces; placements only
    /// reference them.
    pub(in crate::outputs) links: Vec<LinkPlacement>,
    /// A durable random nonce reserved for a future root before a command's
    /// slot is keyed (oxide#129). Reservations are root-level: every member
    /// of this checkout shares one future root and its nonce. Commands that
    /// want the identical future root share it; it is consumed by the commit
    /// that creates the root or removed only by the cancellation journal.
    pub(in crate::outputs) reserved: Option<Reservation>,
    /// Bounded history of roots this checkout has selected, newest last.
    pub(in crate::outputs) roots: Vec<Selected>,
    pub(in crate::outputs) updated_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LinkPlacement {
    pub(in crate::outputs) member: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) selected: Option<Selected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) prepared: Option<Prepared>,
}

impl LinkPlacement {
    pub(crate) fn member(&self) -> &str {
        &self.member
    }

    pub(crate) fn selected_ref(&self) -> Option<&Selected> {
        self.selected.as_ref()
    }

    pub(crate) fn prepared_ref(&self) -> Option<&Prepared> {
        self.prepared.as_ref()
    }
}

/// A reserved future root: its transaction ID, root nonce and the exact
/// destination it was reserved for.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Reservation {
    pub(crate) id: String,
    pub(crate) nonce: String,
    pub(crate) kind: RootKind,
    pub(crate) root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) volume_uuid: Option<String>,
    pub(crate) generation: u64,
    pub(crate) created_ms: u64,
}

impl Reservation {
    /// Whether this reservation is for exactly `wanted`'s future root.
    pub(in crate::outputs) fn is_for(&self, wanted: &Selected) -> bool {
        self.kind == wanted.kind
            && self.root == wanted.root
            && self.volume_uuid == wanted.volume_uuid
            && self.generation == wanted.generation
    }
}

impl Receipt {
    /// A receipt for a checkout that has never converted: the legacy
    /// whole-target layout, selected by nothing.
    #[cfg(test)]
    pub(in crate::outputs) fn first(checkout: &Path, checkout_id: &str) -> Self {
        Self::first_with(checkout, checkout_id, LayoutKind::WholeTarget)
    }

    pub(in crate::outputs) fn first_with(
        checkout: &Path,
        checkout_id: &str,
        layout: LayoutKind,
    ) -> Self {
        let links = match layout {
            LayoutKind::WholeTarget => vec![LinkPlacement {
                member: MEMBER.to_owned(),
                selected: None,
                prepared: None,
            }],
            LayoutKind::Profile => Vec::new(),
        };
        Self {
            version: layout.receipt_version().to_owned(),
            checkout: checkout.to_path_buf(),
            checkout_id: checkout_id.to_owned(),
            generation: 0,
            state: ReceiptState::Detached,
            layout,
            links,
            reserved: None,
            roots: Vec::new(),
            updated_ms: process::now_ms(),
        }
    }

    /// The first placement's mutable handle. Every production writer addresses
    /// placements by member (`place_member`, `detached_receipt`); the tests
    /// still build single-placement fixtures through it.
    #[cfg(test)]
    pub(in crate::outputs) fn link_mut(&mut self) -> &mut LinkPlacement {
        &mut self.links[0]
    }

    pub(crate) fn layout(&self) -> LayoutKind {
        self.layout
    }

    pub(crate) fn links(&self) -> &[LinkPlacement] {
        &self.links
    }

    pub(crate) fn placement(&self, member: &str) -> Option<&LinkPlacement> {
        self.links
            .iter()
            .find(|placement| placement.member == member)
    }

    /// The placement a prepared transition belongs to, by its recorded link.
    /// The link path is authoritative, so a replay never depends on a file's
    /// contents to find its own transition.
    pub(in crate::outputs) fn placement_for_link(&self, link: &Path) -> Option<&LinkPlacement> {
        self.links
            .iter()
            .find(|placement| self.link_path(&placement.member).as_path() == link)
    }

    pub(in crate::outputs) fn placement_for_link_mut(
        &mut self,
        link: &Path,
    ) -> Option<&mut LinkPlacement> {
        let checkout = self.checkout.clone();
        self.links
            .iter_mut()
            .find(|placement| link_path(&checkout, placement.member.as_str()).as_path() == link)
    }

    /// The live link path of one placement member.
    pub(in crate::outputs) fn link_path(&self, member: &str) -> PathBuf {
        link_path(&self.checkout, member)
    }

    pub(crate) fn checkout(&self) -> &Path {
        &self.checkout
    }

    pub(crate) fn checkout_id(&self) -> &str {
        &self.checkout_id
    }

    /// Whether the selected placement is still live: a committed receipt and
    /// a prepared transition both keep their committed root selected.
    pub(crate) fn holds_selection(&self) -> bool {
        matches!(self.state, ReceiptState::Committed | ReceiptState::Prepared)
    }

    /// The layout's representative selected root, whether or not the state is
    /// committed. A profile layout may let two members select different
    /// roots (per-member fallback), so member-specific paths use
    /// [`Receipt::selected_for`].
    pub(crate) fn current(&self) -> Option<&Selected> {
        self.links
            .iter()
            .find_map(|placement| placement.selected.as_ref())
    }

    /// The root one member's placement selects.
    pub(crate) fn selected_for(&self, member: &str) -> Option<&Selected> {
        self.placement(member)
            .and_then(|placement| placement.selected.as_ref())
    }

    pub(crate) fn prepared_ref(&self) -> Option<&Prepared> {
        self.links
            .iter()
            .find_map(|placement| placement.prepared.as_ref())
    }

    pub(crate) fn reserved(&self) -> Option<&Reservation> {
        self.reserved.as_ref()
    }

    pub(in crate::outputs) fn reserved_mut(&mut self) -> &mut Option<Reservation> {
        &mut self.reserved
    }

    /// Add a placement for one profile member, keeping members sorted.
    pub(in crate::outputs) fn place_member(&mut self, member: &str) -> &mut LinkPlacement {
        let position = self
            .links
            .iter()
            .position(|placement| placement.member == member)
            .unwrap_or_else(|| {
                self.links.push(LinkPlacement {
                    member: member.to_owned(),
                    selected: None,
                    prepared: None,
                });
                self.links
                    .sort_by(|left, right| left.member.cmp(&right.member));
                self.links
                    .iter()
                    .position(|placement| placement.member == member)
                    .unwrap_or(0)
            });
        &mut self.links[position]
    }

    pub(in crate::outputs) fn validate(
        &self,
        checkout: &Path,
        checkout_id: &str,
    ) -> Result<(), String> {
        let format_ok = match (self.version.as_str(), self.layout) {
            (RECEIPT_VERSION, LayoutKind::WholeTarget) => true,
            (PROFILE_RECEIPT_VERSION, LayoutKind::Profile) => true,
            _ => false,
        };
        if !format_ok {
            return Err(format!(
                "the owned-output receipt has an unsupported version {:?}",
                self.version
            ));
        }
        if self.checkout != checkout || self.checkout_id != checkout_id {
            return Err(format!(
                "the owned-output receipt names another checkout ({}); it is not followed",
                self.checkout.display()
            ));
        }
        if self.roots.len() > MAX_ROOTS {
            return Err("the owned-output receipt holds too many roots".to_owned());
        }
        match self.layout {
            LayoutKind::WholeTarget => {
                if self.links.len() != 1 || self.links[0].member != MEMBER {
                    return Err(
                        "a whole-target owned-output receipt must hold exactly one `target` placement"
                            .to_owned(),
                    );
                }
            }
            LayoutKind::Profile => {
                if self.links.is_empty() || self.links.len() > crate::layout::MAX_RECEIPT_MEMBERS {
                    return Err(
                        "a profile owned-output receipt holds an unsupported number of members"
                            .to_owned(),
                    );
                }
                let mut names = Vec::with_capacity(self.links.len());
                for placement in &self.links {
                    crate::layout::Member::parse(&placement.member)
                        .map_err(|reason| format!("a profile placement is unusable: {reason}"))?;
                    names.push(placement.member.clone());
                }
                let mut sorted = names.clone();
                sorted.sort();
                sorted.dedup();
                if sorted != names {
                    return Err(
                        "the profile placements are not unique and sorted by member".to_owned()
                    );
                }
                crate::layout::validate_set(&sorted, crate::layout::MAX_RECEIPT_MEMBERS)
                    .map_err(|reason| format!("the profile placements are unusable: {reason}"))?;
            }
        }
        for placement in &self.links {
            if let Some(selected) = placement.selected.as_ref() {
                if !safe_absolute(&selected.root) || selected.generation > self.generation {
                    return Err(
                        "the owned-output receipt selects an unusable root or generation"
                            .to_owned(),
                    );
                }
                if let Some(nonce) = selected.nonce() {
                    if nonce.len() != 32 {
                        return Err(
                            "the owned-output receipt has an unusable root nonce".to_owned()
                        );
                    }
                }
            }
        }
        if let Some(reserved) = self.reserved() {
            if !crate::prune::is_nonce(&reserved.id)
                || !crate::prune::is_nonce(&reserved.nonce)
                || !safe_absolute(&reserved.root)
            {
                return Err("the owned-output receipt has an unusable reservation".to_owned());
            }
        }
        for root in &self.roots {
            // A rolled-back preparation may have created a root at the next
            // generation without advancing the committed one.
            if !safe_absolute(&root.root) || root.generation > self.generation.saturating_add(1) {
                return Err(
                    "the owned-output receipt names an unusable known root or generation"
                        .to_owned(),
                );
            }
        }
        let prepared = self.prepared_ref();
        if self
            .links
            .iter()
            .filter(|placement| placement.prepared.is_some())
            .count()
            > 1
        {
            return Err(
                "the owned-output receipt prepares more than one placement; it is not followed"
                    .to_owned(),
            );
        }
        if let Some(prepared) = prepared {
            if prepared.generation != self.generation.saturating_add(1)
                || prepared.generation == 0
                || !safe_absolute(&prepared.to.root)
                || self.placement_for_link(&prepared.link).is_none()
            {
                return Err(
                    "the owned-output receipt prepares an unusable transition; it is not followed"
                        .to_owned(),
                );
            }
        }
        let consistent = match self.state {
            // A whole-target receipt names its one committed link. A profile
            // receipt places each member independently: a member with no
            // selection is a real local directory, or one left unselected by
            // a detach that a later adoption converts again, so only the
            // receipt as a whole must still select one place.
            ReceiptState::Committed => {
                prepared.is_none()
                    && !self.links.is_empty()
                    && match self.layout {
                        LayoutKind::Profile => self
                            .links
                            .iter()
                            .any(|placement| placement.selected.is_some()),
                        LayoutKind::WholeTarget => self
                            .links
                            .iter()
                            .all(|placement| placement.selected.is_some()),
                    }
            }
            ReceiptState::Detached => {
                prepared.is_none()
                    && self
                        .links
                        .iter()
                        .all(|placement| placement.selected.is_none())
            }
            ReceiptState::Pruned => prepared.is_none(),
            ReceiptState::Prepared => {
                // The write-ahead transition names exactly one placement;
                // a profile receipt's other members may be unselected
                // (detached, awaiting their own re-adoption) without
                // contradicting the pending transition.
                prepared.is_some()
                    && (self.layout == LayoutKind::Profile
                        || self.links.iter().all(|placement| {
                            placement.selected.is_some() || placement.prepared.is_some()
                        }))
            }
        };
        if !consistent {
            return Err(
                "the owned-output receipt is internally inconsistent; it is not followed"
                    .to_owned(),
            );
        }
        Ok(())
    }

    /// The root the current owned link must select, when committed.
    pub(in crate::outputs) fn selected(&self) -> Option<&Selected> {
        match self.state {
            ReceiptState::Committed => self.current(),
            _ => None,
        }
    }

    /// Whether this receipt selects no owned destination at all: `Detached`
    /// (the per-checkout opt-out) or `Pruned` (its root was deleted or settled
    /// by an explicit prune). Every reader that asks "does this checkout own an
    /// output root now" answers through this one method, so the two states are
    /// never classified differently by two call sites.
    pub(in crate::outputs) fn selects_nothing(&self) -> bool {
        matches!(self.state, ReceiptState::Detached | ReceiptState::Pruned)
    }
}

/// The real destination one member's link selects: the approved distinct
/// subpath beneath the shared root (`R/debug`, `R/release`,
/// `R/<triple>/debug`, ...). Only the legacy whole-target placement, member
/// `target`, selects the root directory itself.
pub(crate) fn member_destination(root: &Path, member: &str) -> PathBuf {
    if member == MEMBER {
        root.to_path_buf()
    } else {
        root.join(member)
    }
}

/// The live link path of one placement member under one checkout.
pub(in crate::outputs) fn link_path(checkout: &Path, member: &str) -> PathBuf {
    if member == MEMBER {
        checkout.join("target")
    } else {
        checkout.join("target").join(member)
    }
}

impl Serialize for Receipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Doc<'a> {
            version: &'a str,
            checkout: &'a Path,
            checkout_id: &'a str,
            pub(in crate::outputs) generation: u64,
            pub(in crate::outputs) state: ReceiptState,
            #[serde(skip_serializing_if = "Option::is_none")]
            layout: Option<LayoutKind>,
            links: Vec<PlacementDoc<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            reserved: Option<&'a Reservation>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            roots: &'a Vec<Selected>,
            pub(in crate::outputs) updated_ms: u64,
        }
        #[derive(Serialize)]
        struct PlacementDoc<'a> {
            member: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            selected: Option<&'a Selected>,
            #[serde(skip_serializing_if = "Option::is_none")]
            prepared: Option<&'a Prepared>,
            /// A legacy receipt keeps its reservation inside its one
            /// placement, byte for byte as published.
            #[serde(skip_serializing_if = "Option::is_none")]
            reserved: Option<&'a Reservation>,
        }
        let legacy = self.version == RECEIPT_VERSION;
        let links = self
            .links
            .iter()
            .map(|placement| PlacementDoc {
                member: &placement.member,
                selected: placement.selected.as_ref(),
                prepared: placement.prepared.as_ref(),
                reserved: legacy.then_some(self.reserved.as_ref()).flatten(),
            })
            .collect();
        Doc {
            version: &self.version,
            checkout: &self.checkout,
            checkout_id: &self.checkout_id,
            generation: self.generation,
            state: self.state,
            layout: (!legacy).then_some(self.layout),
            links,
            reserved: (!legacy).then_some(self.reserved.as_ref()).flatten(),
            roots: &self.roots,
            updated_ms: self.updated_ms,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Receipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PlacementDoc {
            pub(in crate::outputs) member: String,
            #[serde(default)]
            pub(in crate::outputs) selected: Option<Selected>,
            #[serde(default)]
            pub(in crate::outputs) prepared: Option<Prepared>,
            #[serde(default)]
            pub(in crate::outputs) reserved: Option<Reservation>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Doc {
            version: String,
            checkout: PathBuf,
            pub(in crate::outputs) checkout_id: String,
            pub(in crate::outputs) generation: u64,
            pub(in crate::outputs) state: ReceiptState,
            #[serde(default)]
            layout: Option<LayoutKind>,
            #[serde(default)]
            links: Vec<PlacementDoc>,
            #[serde(default)]
            pub(in crate::outputs) reserved: Option<Reservation>,
            #[serde(default)]
            pub(in crate::outputs) selected: Option<Selected>,
            #[serde(default)]
            pub(in crate::outputs) prepared: Option<Prepared>,
            #[serde(default)]
            pub(in crate::outputs) roots: Vec<Selected>,
            #[serde(default)]
            pruned: Option<serde_json::Value>,
            pub(in crate::outputs) updated_ms: u64,
        }
        let doc = Doc::deserialize(deserializer)?;
        let legacy_reservations: Vec<Reservation> = doc
            .links
            .iter()
            .filter_map(|placement| placement.reserved.clone())
            .collect();
        let (version, layout, links) = match doc.version.as_str() {
            RECEIPT_VERSION => {
                if doc.layout.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-2 receipt carries no successor layout",
                    ));
                }
                if doc.selected.is_some() || doc.prepared.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-2 receipt carries placements, not top-level references",
                    ));
                }
                let links = doc
                    .links
                    .into_iter()
                    .map(|placement| LinkPlacement {
                        member: placement.member,
                        selected: placement.selected,
                        prepared: placement.prepared,
                    })
                    .collect();
                (RECEIPT_VERSION.to_owned(), LayoutKind::WholeTarget, links)
            }
            LEGACY_RECEIPT_VERSION => {
                // oxide#152: the whole-target shape oxide#117 published. It
                // predates layouts, placements, root-level reservations,
                // prune states and root nonces, so a document that carries
                // one of those is contradictory rather than merely old. It
                // is decoded into the current in-memory whole-target form;
                // reading writes nothing.
                if doc.layout.is_some()
                    || !doc.links.is_empty()
                    || !legacy_reservations.is_empty()
                    || doc.reserved.is_some()
                    || doc.pruned.is_some()
                    || doc.state == ReceiptState::Pruned
                    || doc
                        .selected
                        .as_ref()
                        .is_some_and(|selected| selected.nonce.is_some())
                    || doc
                        .prepared
                        .as_ref()
                        .is_some_and(|prepared| prepared.to.nonce.is_some())
                    || doc.roots.iter().any(|root| root.nonce.is_some())
                {
                    return Err(serde::de::Error::custom(
                        "a version-1 receipt carries only its whole-target selection",
                    ));
                }
                (
                    RECEIPT_VERSION.to_owned(),
                    LayoutKind::WholeTarget,
                    vec![LinkPlacement {
                        member: MEMBER.to_owned(),
                        selected: doc.selected,
                        prepared: doc.prepared,
                    }],
                )
            }
            PROFILE_RECEIPT_VERSION => {
                if doc.selected.is_some() || doc.prepared.is_some() {
                    return Err(serde::de::Error::custom(
                        "a version-3 receipt carries placements, not top-level references",
                    ));
                }
                let layout = match doc.layout {
                    Some(LayoutKind::Profile) => LayoutKind::Profile,
                    _ => {
                        return Err(serde::de::Error::custom(
                            "a version-3 receipt must declare its layout",
                        ))
                    }
                };
                if doc
                    .links
                    .iter()
                    .any(|placement| placement.reserved.is_some())
                {
                    return Err(serde::de::Error::custom(
                        "a version-3 reservation is root-level, not per placement",
                    ));
                }
                let links = doc
                    .links
                    .into_iter()
                    .map(|placement| LinkPlacement {
                        member: placement.member,
                        selected: placement.selected,
                        prepared: placement.prepared,
                    })
                    .collect();
                (PROFILE_RECEIPT_VERSION.to_owned(), layout, links)
            }
            other => {
                return Err(serde::de::Error::custom(format!(
                    "unsupported version {other:?} of the owned-output receipt"
                )))
            }
        };
        // A legacy receipt keeps a reservation inside its placement; the
        // in-memory form holds one root-level reservation.
        let mut reserved = doc.reserved;
        if legacy_reservations.len() > 1 || (reserved.is_some() && !legacy_reservations.is_empty())
        {
            return Err(serde::de::Error::custom(
                "an owned-output receipt carries more than one reservation",
            ));
        }
        if let Some(placement) = legacy_reservations.into_iter().next() {
            reserved = Some(placement);
        }
        Ok(Receipt {
            version,
            checkout: doc.checkout,
            checkout_id: doc.checkout_id,
            generation: doc.generation,
            state: doc.state,
            layout,
            links,
            reserved,
            roots: doc.roots,
            updated_ms: doc.updated_ms,
        })
    }
}

/// Ownership marker inside an output root. Written once, at root creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Marker {
    pub(in crate::outputs) version: String,
    pub(in crate::outputs) checkout: PathBuf,
    pub(in crate::outputs) checkout_id: String,
    pub(in crate::outputs) label: String,
    pub(in crate::outputs) recipe: String,
    pub(in crate::outputs) kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) volume_uuid: Option<String>,
    /// The root's immutable nonce. Version-1 markers predate it; a version-2
    /// marker always carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) nonce: Option<String>,
    /// The APFS volume UUID of the checkout when the root was created
    /// (shared `oxide.outputs.root/2`); absent in version-1 markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) checkout_volume_uuid: Option<String>,
    /// The generation the root was created at; never rewritten.
    pub(in crate::outputs) generation: u64,
    pub(in crate::outputs) created_ms: u64,
}

impl Marker {
    pub(crate) fn is_owned_by(&self, checkout: &Path, checkout_id: &str) -> bool {
        self.checkout == checkout && self.checkout_id == checkout_id
    }

    pub(crate) fn kind(&self) -> RootKind {
        self.kind
    }

    pub(crate) fn volume_uuid(&self) -> Option<&str> {
        self.volume_uuid.as_deref()
    }

    pub(crate) fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(in crate::outputs) fn validate(&self) -> Result<(), String> {
        if self.version != MARKER_VERSION && self.version != LEGACY_MARKER_VERSION {
            return Err(format!(
                "owner marker version {:?} is not supported",
                self.version
            ));
        }
        if self.version == MARKER_VERSION
            && self.nonce.as_deref().is_none_or(|nonce| {
                nonce.len() != 32
                    || !nonce
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err("a version-2 owner marker needs a 128-bit lowercase hex nonce".to_owned());
        }
        Ok(())
    }
}

pub(in crate::outputs) fn remember_root(receipt: &mut Receipt, root: Selected) {
    receipt.roots.retain(|known| known.root != root.root);
    receipt.roots.push(root);
    while receipt.roots.len() > MAX_ROOTS {
        receipt.roots.remove(0);
    }
}

/// Whether a receipt reader accepts the version-1 whole-target document.
/// Read-only status, plan, detach and prune readers accept a valid document
/// without rewriting it; ordinary execution readers still refuse it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::outputs) enum LegacyReceiptReading {
    /// Ordinary execution readers refuse a version-1 receipt.
    Refuse,
    /// Read-only and switch readers decode a valid version-1 receipt into the
    /// current in-memory whole-target form. Reading writes nothing.
    Read,
}

/// The lowercase state name a receipt document carries.
pub(in crate::outputs) fn receipt_state_label(state: ReceiptState) -> &'static str {
    match state {
        ReceiptState::Committed => "committed",
        ReceiptState::Prepared => "prepared",
        ReceiptState::Detached => "detached",
        ReceiptState::Pruned => "pruned",
    }
}

/// Only the version field, so a reader can decide about a version-1 document
/// before decoding the rest of it.
#[derive(Deserialize)]
struct ReceiptVersion {
    version: String,
}

pub(in crate::outputs) fn read_receipt(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt_with(LegacyReceiptReading::Refuse, control, checkout, checkout_id)
}

/// Read one checkout's receipt without creating or repairing anything.
/// `legacy` decides whether a valid version-1 whole-target document is read or
/// refused; every reader keeps the byte limit, the unknown-field strictness and
/// the checkout identity check unchanged.
pub(in crate::outputs) fn read_receipt_with(
    legacy: LegacyReceiptReading,
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    let path = receipt_path(control, checkout_id);
    let Some(file) = store::private_file(&path, false)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(format!(
            "{} exceeds its {MAX_RECEIPT_BYTES}-byte limit",
            path.display()
        ));
    }
    let version: ReceiptVersion = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    let legacy_document = version.version == LEGACY_RECEIPT_VERSION;
    if legacy_document && legacy == LegacyReceiptReading::Refuse {
        return Err(format!(
            "{} is invalid: unsupported version {LEGACY_RECEIPT_VERSION:?} of the owned-output receipt; run `oxide outputs detach` then `oxide outputs adopt` to switch a legacy checkout",
            path.display()
        ));
    }
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    receipt.validate(checkout, checkout_id)?;
    Ok(Some(receipt))
}

pub(in crate::outputs) fn receipt_path(control: &Path, checkout_id: &str) -> PathBuf {
    control.join("outputs").join(format!("{checkout_id}.json"))
}

pub(in crate::outputs) fn write_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    receipt.validate(&receipt.checkout, &receipt.checkout_id)?;
    // One critical section for the registry and the receipt. The registry's
    // revision advances before the changed receipt becomes visible.
    let _guard = store::publication_guard(control)?;
    crate::prune::note_receipt(control, receipt)?;
    let directory = control.join("outputs");
    super::root::create_owned_directory(&directory)?;
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| format!("cannot encode the output receipt: {error}"))?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err("the output receipt exceeds its byte limit".to_owned());
    }
    let path = receipt_path(control, &receipt.checkout_id);
    // Only the admission-lock holder writes, so one temp name suffices.
    let temp = directory.join(format!(
        ".{}.tmp-{}",
        receipt.checkout_id,
        std::process::id()
    ));
    write_private(&temp, &bytes)?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output receipt: {error}")
    })?;
    sync_directory(&directory)?;
    crate::prune::finish_receipt(control, receipt)
}

pub(crate) fn receipt_path_public(control: &Path, checkout_id: &str) -> PathBuf {
    receipt_path(control, checkout_id)
}

/// Read one checkout's receipt for the prune snapshot. Never creates or
/// repairs anything. A valid version-1 whole-target receipt is read as its
/// current whole-target form (oxide#152); its bytes stay exactly as they are.
pub(crate) fn read_receipt_for_prune(
    control: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<Option<Receipt>, String> {
    read_receipt_with(LegacyReceiptReading::Read, control, checkout, checkout_id)
}

/// Open one root marker for reading without ever following a symlink, and
/// return the descriptor whose own metadata every check applies to. The
/// bytes and the bytes' identity therefore belong to the same inode, so a
/// path that is swapped for a symlink (or for a different file) between an
/// inspection and the read is refused instead of read through.
pub(in crate::outputs) fn open_marker_file(path: &Path) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not an owner marker file", path.display()));
    }
    if metadata.len() > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(file)
}

/// The SHA-256 of the exact marker bytes, for the recorded root identity.
pub(crate) fn marker_digest(root: &Path) -> Result<String, String> {
    let path = root.join(MARKER);
    let file = open_marker_file(&path)?;
    let mut bytes = Vec::new();
    file.take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(sha256_hex(&bytes))
}

/// A marker read for prune discovery of a listed receiptless root.
pub(crate) fn read_marker_for_prune(root: &Path) -> Result<Marker, String> {
    read_marker(root)
}

pub(in crate::outputs) fn read_marker(root: &Path) -> Result<Marker, String> {
    let path = root.join(MARKER);
    // O_NOFOLLOW, and the regular-file and byte-limit checks, all apply to the
    // opened descriptor: a symlink that appears after any earlier inspection
    // is never followed.
    let file = open_marker_file(&path)
        .map_err(|error| format!("{} has no readable owner marker ({error})", root.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    let marker: Marker = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    marker
        .validate()
        .map_err(|reason| format!("{} is invalid ({reason})", path.display()))?;
    Ok(marker)
}

pub(in crate::outputs) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    };
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(path);
            format!("cannot write {}: {error}", path.display())
        })
}

pub(in crate::outputs) fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

/// A path must be absolute and free of `..` for use in a receipt.
pub(in crate::outputs) fn safe_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

/// SHA-256 hex digest of a byte slice.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// SHA-256 of one file's exact bytes.
#[cfg(test)]
pub(in crate::outputs) fn file_digest(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(sha256_hex(&bytes))
}
