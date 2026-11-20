//! Command planning, `oxide exec` / external-subcommand member inference and tool notices.

use super::adopt::profile_members;
use super::bind::{check_bound_root, ephemeral_member_state, is_local_member};
use super::receipt::{
    link_path, member_destination, LayoutKind, Receipt, ReceiptState, RootKind, Selected,
};
use super::root::{is_real_dir, link_state, owned_root, target_state};
use super::transition::desired_selection;
use super::{Context, TargetState, MEMBER};
use crate::identity;
use crate::removable::DriveMode;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// One declared member's planned placement for this command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PlannedMember {
    pub(crate) member: String,
    /// The owned root the member will write through, or `None` for a real
    /// profile directory this command keeps local (it is never converted by a
    /// build).
    pub(crate) selected: Option<Selected>,
}

/// This command's planned destination, read-only and computed before the slot
/// is keyed. The legacy whole-target layout has exactly one `target`
/// placement; the profile layout has one entry per declared member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub(crate) whole_target: bool,
    pub(crate) members: Vec<PlannedMember>,
}

impl Plan {
    pub(in crate::outputs) fn selected(&self) -> Option<&Selected> {
        self.members
            .iter()
            .find_map(|member| member.selected.as_ref())
    }
}

/// The root the next command must select, read-only and without any lock: the
/// external root when the registered volume is usable now, else the
/// checkout-local fallback. Its `generation` is the generation the root has or
/// will be created at, so the slot key and the binding agree. `None` when this
/// checkout has no owned output link (never converted, or detached).
pub fn planned_destination(
    control: Option<&Path>,
    checkout: &Path,
    declaration: &crate::layout::Declaration,
    pool_fallback: Option<&str>,
) -> Result<Option<Plan>, String> {
    let Some(control) = control else {
        return Ok(None);
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    let Some(receipt) = context.receipt()? else {
        return Ok(None);
    };
    // A detached profile checkout owns no destination at all: its command runs
    // locally through real directories, exactly as an unconverted checkout
    // does. Planning the receipt's (unselected) members as owned placements is
    // what used to make this state refuse; oxide#160 (S1) removes that
    // over-refusal, so the key carries no destination and the binding accepts
    // no link.
    if receipt.layout() == LayoutKind::Profile && receipt.state == ReceiptState::Detached {
        return Ok(None);
    }
    match receipt.layout() {
        LayoutKind::WholeTarget => {
            let Some(selected) = desired_selection(&context, &receipt)? else {
                return Ok(None);
            };
            Ok(Some(Plan {
                whole_target: true,
                members: vec![PlannedMember {
                    member: MEMBER.to_owned(),
                    selected: Some(selected),
                }],
            }))
        }
        LayoutKind::Profile => {
            if declaration.is_empty() {
                // A committed profile receipt is always planned from a member
                // set: `oxide exec` derives one from the receipt itself
                // (oxide#160), so an empty set here is a caller bug, not a
                // checkout state.
                return Err("outputs: this checkout places outputs at profile members, but this command declares none; declare its outputs with `oxide exec --output <member>` or run `oxide outputs detach`"
                    .to_owned());
            }
            let ephemeral = context.mode()? == Some(DriveMode::Ephemeral);
            let desired = if ephemeral {
                None
            } else {
                desired_selection(&context, &receipt)?
            };
            let mut members = Vec::with_capacity(declaration.members().len());
            for member in declaration.members() {
                let link = link_path(&context.workspace, member.name());
                let state = if ephemeral {
                    ephemeral_member_state(&context.workspace, &context.checkout_id, member.name())?
                } else {
                    link_state(&link, &context.workspace, &context.checkout_id)?
                };
                let selected = match state {
                    TargetState::RealEmpty | TargetState::RealNonEmpty => None,
                    TargetState::Missing if ephemeral => None,
                    TargetState::OwnedLink(_) | TargetState::Dangling(_) if ephemeral => {
                        receipt.selected_for(member.name()).cloned()
                    }
                    TargetState::Missing | TargetState::OwnedLink(_) | TargetState::Dangling(_) => desired.clone(),
                    TargetState::Foreign => return Err(format!(
                        "outputs: {} is {}; Oxide never follows or replaces it. Remove it or restore the owned link, then run the command again",
                        link.display(), state.label()
                    )),
                };
                members.push(PlannedMember {
                    member: member.name().to_owned(),
                    selected,
                });
            }
            Ok(Some(Plan {
                whole_target: false,
                members,
            }))
        }
    }
}

/// The profile-output declaration one leased command uses (oxide#164).
///
/// An external subcommand is Cargo running `cargo-<name>`, which is exactly
/// what `oxide exec -- cargo <name>` runs, so when the caller declares no
/// `--output` both use the SAME derivation, [`exec_declaration`]: one member
/// set, one slot key, one set of bindings and refusals. Nothing here adds a
/// second path; `exec`'s explicit `--output` set and the classification of a
/// modelled command or building built-in are unchanged, and a classification
/// this layout cannot model (a JSON target specification) still refuses in a
/// profile checkout.
pub fn lease_declaration(
    invocation: &crate::cli::Invocation,
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<crate::layout::Declaration, String> {
    let command = invocation.command.as_str();
    if crate::cli::derives_from_receipt(command, &invocation.declared_outputs) {
        return exec_declaration(control, checkout, pool_fallback);
    }
    if command == "exec" {
        // The caller's `--output` declaration is the complete set of the
        // tool's profile outputs, exactly as before; it stays an optional
        // override and keeps its byte-identical slot key.
        return crate::layout::declared(&invocation.declared_outputs);
    }
    match crate::layout::classify(
        command,
        invocation.profile,
        invocation.explicit_profile,
        &invocation.targets,
    ) {
        crate::layout::Layout::Declared(declaration) => Ok(declaration),
        crate::layout::Layout::Unknown(reason) => {
            refuse_unknown_layout(control, checkout, &reason)?;
            Ok(crate::layout::Declaration::empty())
        }
    }
}

/// The profile member set `oxide exec` derives when the caller declares none
/// (oxide#160, S1; design `exec-output-inference.md`).
///
/// The keyed set is exactly what this checkout's committed receipt places,
/// plus every receipt-recorded member it keeps real; a member-shaped path the
/// receipt does not record is never planned, keyed or converted. Every
/// candidate path is classified read-only, with the shipped
/// [`migration_members`] traversal and [`link_state`] pair: neither follows a
/// link target, lists a payload or walks a third level, and the classification
/// can only refuse. Nothing here creates, links, reserves or deletes anything.
///
/// `Ok(Declaration::empty())` is the honest answer for a checkout with no
/// owned placement (never converted, legacy whole target, or detached): those
/// commands run exactly as they do today, with no owned destination in the
/// key.
pub fn exec_declaration(
    control: Option<&Path>,
    checkout: &Path,
    pool_fallback: Option<&str>,
) -> Result<crate::layout::Declaration, String> {
    let Some(control) = control else {
        return Ok(crate::layout::Declaration::empty());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    let Some(receipt) = context.receipt()? else {
        return Ok(crate::layout::Declaration::empty());
    };
    if receipt.layout() != LayoutKind::Profile {
        // A legacy whole-target checkout's declaration is never consulted.
        return Ok(crate::layout::Declaration::empty());
    }
    match receipt.state {
        ReceiptState::Committed | ReceiptState::Detached => {}
        ReceiptState::Prepared => {
            return Err("outputs: an interrupted owned-output transition is pending; ensure a permanent drive registration, then run `oxide outputs adopt` to replay it before running an exec tool".to_owned())
        }
        ReceiptState::Pruned => {
            return Err("outputs: this checkout's owned output root was pruned; run `oxide outputs adopt` to convert it again".to_owned())
        }
    }
    let members = exec_members(&context, &receipt)?;
    if receipt.state == ReceiptState::Detached {
        // A detached checkout owns no destination: its command runs locally
        // through real directories, so the derived set is empty and the key
        // carries no destination. The classification above still refused any
        // member-shaped link or file it must not write through.
        return Ok(crate::layout::Declaration::empty());
    }
    if members.len() > crate::layout::MAX_MEMBERS {
        return Err(format!(
            "outputs: this checkout places {} profile output members, more than the {} one command binds; run `oxide outputs detach` for a real target directory, or declare the members this tool writes with `oxide exec --output <member>`. Nothing was changed",
            members.len(),
            crate::layout::MAX_MEMBERS
        ));
    }
    let parsed = members
        .iter()
        .map(|name| crate::layout::Member::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    crate::layout::Declaration::new(parsed)
}

/// Classify every candidate profile member of a profile-layout checkout for a
/// derived `exec` plan. Only this checkout's own owned links at the receipt's
/// committed destinations are accepted, plus the two absent-link shapes the
/// permanent-mode binding repairs (an absent member directory beneath a
/// verified root, oxide#162, and an absent member link of a verified root whose
/// real `target` directory went with it, oxide#165). In ephemeral mode a
/// recorded member that is real or missing is local. Foreign paths refuse.
fn exec_members(context: &Context, receipt: &Receipt) -> Result<Vec<String>, String> {
    let live_target = target_state(&context.workspace, &context.checkout_id)?;
    let mut candidates: Vec<String> = Vec::new();
    match &live_target {
        TargetState::RealEmpty | TargetState::RealNonEmpty => {
            // Member names are matched first and `file_type()` never follows a
            // link, so no profile payload is listed and no linked entry is
            // descended.
            candidates = profile_members(&context.link())?;
        }
        TargetState::Missing => {}
        state => {
            return Err(format!(
                "outputs: {} is {}; a checkout that places outputs at profile members keeps a real target directory, so an exec tool never writes through it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                context.link().display(),
                state.label()
            ))
        }
    }
    // Every recorded placement is a candidate too: a member the receipt places
    // but whose path is absent still has to be checked, and it refuses below
    // rather than being guessed into the plan.
    for placement in receipt.links() {
        if !candidates.iter().any(|name| name == placement.member()) {
            candidates.push(placement.member().to_owned());
        }
    }
    candidates.sort();
    candidates.dedup();
    // Permanent-mode repair runs under writer custody. Ephemeral-mode tools
    // can use a missing member locally without touching its recorded root.
    let mode = context.mode()?;
    let permanent = mode == Some(DriveMode::Permanent);
    let ephemeral = mode == Some(DriveMode::Ephemeral);
    let desired = if ephemeral {
        None
    } else {
        desired_selection(context, receipt)?
    };
    let mut members = Vec::new();
    for name in &candidates {
        let link = link_path(&context.workspace, name);
        let state = if ephemeral {
            ephemeral_member_state(&context.workspace, &context.checkout_id, name)?
        } else {
            link_state(&link, &context.workspace, &context.checkout_id)?
        };
        if ephemeral && receipt.placement(name).is_some() && is_local_member(&state) {
            members.push(name.clone());
            continue;
        }
        match receipt.placement(name) {
            Some(placement) => match placement.selected_ref() {
                Some(committed) => {
                    let wanted = if ephemeral { Some(committed) } else { desired.as_ref() };
                    let Some(desired) = wanted else {
                        return Err(format!(
                            "outputs: this checkout's receipt places the profile member {name} at a root this command would no longer select; an exec command never relinks it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                        ));
                    };
                    let destination = member_destination(&desired.root, name);
                    match &state {
                        TargetState::OwnedLink(root) if root == &destination => {
                            members.push(name.clone());
                        }
                        // oxide#165: the member link is positively ABSENT
                        // while its committed destination's root fully
                        // verifies — the shape a deleted `<checkout>/target`
                        // directory leaves, because the member links live
                        // inside it. The permanent-mode binding recreates that
                        // real directory and publishes this member through the
                        // unchanged transition (which creates the member
                        // directory inside the root first). A missing,
                        // unverified or other root keeps the refusal below.
                        TargetState::Missing
                            if permanent
                                && missing_member_of_verified_root(context, receipt, desired) =>
                        {
                            members.push(name.clone());
                        }
                        // The only absent-directory case a derived exec may
                        // place: the link selects exactly this member's
                        // destination under the committed, verified root (same
                        // root, nonce, volume and generation) and only that
                        // member's directory is missing. The permanent-mode
                        // binding creates it under the root's custody; a link
                        // into a missing, unverified or foreign root still
                        // refuses exactly as before (its root fails the check).
                        state
                            if permanent
                                && absent_member_of_verified_root(
                                    context,
                                    receipt,
                                    desired,
                                    name,
                                    state,
                                ) =>
                        {
                            members.push(name.clone());
                        }
                        state => {
                            return Err(format!(
                                "outputs: this checkout's receipt places the profile member {name} at {}, but {} is {}; an exec command never relinks, converts or replaces it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs, then run the command again",
                                destination.display(),
                                link.display(),
                                state.label()
                            ))
                        }
                    }
                }
                None => match &state {
                    TargetState::RealEmpty | TargetState::RealNonEmpty => {
                        members.push(name.clone());
                    }
                    TargetState::Missing => {}
                    state => {
                        return Err(format!(
                            "outputs: this checkout's receipt records the profile member {name} as a real directory, but {} is {}; an exec command never relinks, replaces or writes through it. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                            link.display(),
                            state.label()
                        ))
                    }
                },
            },
            None => match &state {
                // A real member outside the receipt is local and path-stable:
                // it is not planned and not keyed, and the tool writes into it
                // as it is.
                TargetState::RealEmpty | TargetState::RealNonEmpty | TargetState::Missing => {}
                state => {
                    return Err(format!(
                        "outputs: {} is {}, and this checkout's receipt does not place it; an exec command never writes through a link or file it does not own. Remove it or run `oxide outputs adopt` to record the placement, then run the command again. Nothing was changed",
                        link.display(),
                        state.label()
                    ))
                }
            },
        }
    }
    Ok(members)
}

/// Whether a member link is positively ABSENT while the receipt's committed
/// placement for it is a root that fully verifies right now (oxide#165). This
/// is the shape a deleted `<checkout>/target` directory leaves: the member
/// links lived inside it, so they are gone with it, while the owned root, its
/// marker, its nonce and its payload are untouched. The permanent-mode binding
/// recreates the target directory and publishes such a member; nothing is
/// replaced or followed here. A missing, another-nonce or unverified root
/// fails this check and keeps the existing refusal.
fn missing_member_of_verified_root(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> bool {
    check_bound_root(context, receipt.generation, wanted).is_ok()
}

/// Whether this member link's only defect is its ABSENT destination directory
/// beneath the committed, desired owned root. The link must select exactly
/// `<root>/<member>` and the root itself must pass the full bound check: this
/// checkout's real marker, its kind and volume, the receipt's generation, the
/// desired nonce and the live device/ancestry. Nothing is created here; the
/// permanent-mode binding does that under the checkout's custody. A missing
/// root, another root, another nonce and an unverified directory all fail,
/// which keeps every existing refusal intact.
fn absent_member_of_verified_root(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
    name: &str,
    state: &TargetState,
) -> bool {
    let TargetState::Dangling(destination) = state else {
        return false;
    };
    destination == &member_destination(&wanted.root, name)
        && check_bound_root(context, receipt.generation, wanted).is_ok()
}

/// The EXACT P4 precondition binding enforces, evaluated before the lease.
///
/// `bind_profile` refuses per REQUESTED member: that member's committed
/// placement is on an EXTERNAL root while this command's fresh selection for it
/// is not external (the registered volume is away or became unusable). Only a
/// profile receipt carries that per-member rule — a whole-target receipt never
/// refuses this way, and its future local root needs its reservation so
/// `reservation_matches` can accept it — so the layout is part of the
/// condition. A member with no committed placement (a new or nested member)
/// does not trigger the refusal either, and keeps its reservation.
///
/// Returns true only when binding will refuse because of this condition, so the
/// pre-lease reservation can leave the receipt untouched.
pub(in crate::outputs) fn refuses_committed_external(
    receipt: &Receipt,
    wanted: &Selected,
    declaration: &crate::layout::Declaration,
) -> bool {
    if receipt.layout() != LayoutKind::Profile || wanted.kind == RootKind::External {
        return false;
    }
    declaration.members().iter().any(|member| {
        receipt
            .placement(member.name())
            .and_then(|placement| placement.selected.as_ref())
            .is_some_and(|committed| committed.kind == RootKind::External)
    })
}

/// The dangling member link binding refuses, evaluated before the lease.
///
/// `bind_profile` refuses a REQUESTED profile member whose owned member link
/// dangles (the committed root is missing at that member's destination). A
/// future-root reservation published for that destination is dead on arrival:
/// its registry row is the committed SELECTED row, which keeps the root's old
/// nonce, so `validate_reservation` refuses it — and it then blocks the
/// explicit `oxide outputs adopt` that the binding refusal names. Reserving
/// nothing keeps that refusal at zero authority effect, and the adopt recovers
/// with a fresh nonce through the ordinary rules (oxide#152).
///
/// Only the requested member set is classified, and only the exact
/// `TargetState::Dangling` binding refuses: a member link that still resolves
/// (`OwnedLink`, the local→external redock) and a missing link (a new member
/// joining an existing root) keep their reservation, as does any whole-target
/// layout.
pub(in crate::outputs) fn refuses_dangling_member(
    context: &Context,
    receipt: &Receipt,
    declaration: &crate::layout::Declaration,
) -> Result<bool, String> {
    if receipt.layout() != LayoutKind::Profile {
        return Ok(false);
    }
    for member in declaration.members() {
        if receipt.selected_for(member.name()).is_none() {
            continue;
        }
        let link = link_path(&context.workspace, member.name());
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        if matches!(state, TargetState::Dangling(_)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether one leased command must durably reserve a future root nonce before
/// its slot is keyed (oxide#129).
///
/// A derived plan over this checkout's committed placement is a subset of that
/// placement, so it reserves nothing: the root it binds already exists and no
/// member is new. A derived set that is EMPTY names no placement at all — a
/// whole-target checkout, a detached receipt, an unconverted checkout — and a
/// whole-target plan can still name a FUTURE root (the first local fallback to
/// an absent root, oxide#117), which binding accepts only through the exact
/// surviving reservation its slot was keyed with. Those keep the shipped
/// reservation path, exactly as every command did before oxide#164: a derived
/// `exec` in the same state now does too (oxide#164 review finding).
///
/// Callers pass whether the command derived its set
/// ([`crate::cli::derives_from_receipt`]) and the declaration it will use.
pub fn needs_reservation(derived: bool, declaration: &crate::layout::Declaration) -> bool {
    !derived || declaration.is_empty()
}

/// Upper bound on the entries one bounded target observation considers.
const TARGET_ENTRY_CAP: usize = 4096;
/// Upper bound on the directory names one notice prints.
const TOOL_DIRECTORY_CAP: usize = 8;
/// Cargo's own top-level directory inside a real target. It is never a profile
/// member and never a tool's output directory, so it is never reported.
const CARGO_TARGET_TEMP: &str = "tmp";

/// The entry names directly under one checkout's real target directory: one
/// bounded, non-recursive listing that never follows a link.
///
/// `None` when `target` is not a real directory (the whole-target layout links
/// it and an absent target holds nothing), when it cannot be read, or when it
/// holds more than [`TARGET_ENTRY_CAP`] entries. An unexpected or oversized
/// tree is left unreported rather than walked.
pub fn target_entries(checkout: &Path) -> Option<Vec<String>> {
    let target = checkout.join("target");
    let metadata = fs::symlink_metadata(&target).ok()?;
    if !is_real_dir(&metadata) {
        return None;
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&target).ok()? {
        if names.len() >= TARGET_ENTRY_CAP {
            return None;
        }
        let entry = entry.ok()?;
        names.push(entry.file_name().to_str()?.to_owned());
    }
    names.sort();
    Some(names)
}

/// Whether one command's bound layout is the receipt-derived PROFILE-MEMBER
/// layout, the only path that prints the tool-directory notice (oxide#164).
///
/// A derived command (`oxide exec` without `--output`, or an external
/// subcommand) that planned profile members is the checkout shape the brief's
/// "reported once" belongs to. Every other shape keeps its shipped stderr: a
/// never-converted or ephemeral checkout plans nothing, a legacy whole-target
/// checkout plans the whole target, a detached profile receipt plans no
/// destination, and a modelled command or an explicit `--output` declaration is
/// not a derived set. None of them gets the extra line.
pub fn tool_notice_applies(derived: bool, plan: Option<&Plan>) -> bool {
    // The checkout's layout decides, not which members this command happened to
    // place: a profile plan whose member is a receipt-recorded real directory is
    // still the profile layout, and the tool may create a non-member directory
    // beside it.
    derived && plan.is_some_and(|plan| !plan.whole_target && !plan.members.is_empty())
}

/// One bounded notice naming the directories a tool created directly under its
/// real target that are NOT profile members — the tool's own `target/nextest`
/// or `target/llvm-cov` (oxide#164). Callers print it only when
/// [`tool_notice_applies`] holds, so an unconverted, ephemeral, whole-target or
/// detached checkout keeps its previous stderr.
///
/// `before` is [`target_entries`] captured before the writer started. The
/// comparison is two listings of ONE directory: no recursion, no payload walk,
/// no link followed, and at most [`TOOL_DIRECTORY_CAP`] names are printed.
/// Cargo's own top-level `tmp` is never reported, a member-shaped path is the
/// member layout's business (never this notice's), and a `target` that is not a
/// real directory is not observed at all.
pub fn tool_directories_note(checkout: &Path, before: Option<&[String]>) -> Option<String> {
    let before = before?;
    let after = target_entries(checkout)?;
    let mut created: Vec<String> = after
        .into_iter()
        .filter(|name| !before.iter().any(|known| known == name))
        .filter(|name| name.as_str() != CARGO_TARGET_TEMP)
        .filter(|name| crate::layout::Member::parse(name).is_err())
        .filter(|name| {
            fs::symlink_metadata(checkout.join("target").join(name))
                .is_ok_and(|metadata| is_real_dir(&metadata))
        })
        .collect();
    created.sort();
    if created.is_empty() {
        return None;
    }
    let shown = created.len().min(TOOL_DIRECTORY_CAP);
    let mut list = created[..shown].join(", ");
    if created.len() > shown {
        list.push_str(&format!(" and {} more", created.len() - shown));
    }
    Some(format!(
        "tool directories kept local ({list}): a directory that is not a profile member stays a real directory of this checkout and is never placed"
    ))
}

/// Refuse a command whose profile outputs cannot be classified from its name
/// in a checkout that already places outputs at profile members. An unlinked
/// checkout runs such a command locally exactly as before. External
/// subcommands no longer reach this refusal: they derive their member set from
/// the receipt like `oxide exec` (oxide#164), so only a modelled command whose
/// target or profile the layout cannot model (a JSON target specification)
/// refuses here.
pub fn refuse_unknown_layout(
    control: Option<&Path>,
    checkout: &Path,
    reason: &str,
) -> Result<(), String> {
    let Some(control) = control else {
        return Ok(());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf());
    let profile = context.receipt()?.is_some_and(|receipt| {
        receipt.layout() == LayoutKind::Profile && !receipt.selects_nothing()
    });
    if profile {
        return Err(format!(
            "outputs: {reason}. This checkout places outputs at profile members and Oxide will not guess where this command writes; declare its outputs with `oxide exec --output <member>` or run `oxide outputs detach`"
        ));
    }
    Ok(())
}

/// The real executable for a Cargo-reported program path, when that program's
/// member or legacy whole target is an owned link into `root`:
/// `target/<member>/<rest>` maps to `root/<member>/<rest>`. A real
/// (unlinked) member keeps Cargo's own path.
pub fn program_target(checkout: &Path, root: &Path, program: &Path) -> Option<PathBuf> {
    let relative = program.strip_prefix(&checkout.join("target")).ok()?;
    let mut components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => name.to_str(),
            _ => None,
        });
    let first = components.next()?;
    let member = if first == "debug" || first == "release" {
        first.to_owned()
    } else {
        let second = components.next()?;
        if second != "debug" && second != "release" {
            return None;
        }
        format!("{first}/{second}")
    };
    crate::layout::Member::parse(&member).ok()?;
    // The caller supplies the root bound under the held lease. A legacy
    // target link must still name that exact root and carry this checkout's
    // owner marker; an unverified link keeps the existing Cargo-path fallback.
    if fs::read_link(checkout.join("target")).ok().as_deref() == Some(root) {
        let checkout_id = identity::digest(&checkout.to_string_lossy());
        owned_root(checkout, &checkout_id, root).ok()?;
        if !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        let mapped = root.join(relative);
        return mapped.is_file().then_some(mapped);
    }
    let target = fs::read_link(link_path(checkout, &member)).ok()?;
    if target != member_destination(root, &member) {
        return None;
    }
    let mapped = root.join(relative);
    mapped.is_file().then_some(mapped)
}

/// The final-output destination, as a component of a slot's compatibility
/// identity. Empty for an ephemeral or detached checkout, so those checkouts
/// keep exactly the slot keys they had before this feature.
///
/// A legacy whole-target plan renders the published `outputs-2` line
/// unchanged: its key never changes without an explicit migration. A profile
/// plan renders one sorted `outputs-3` line per owned member and `real
/// <member>` for a real directory kept local, so a linked member and a real
/// member never share a slot.
pub fn destination_facts(plan: Option<&Plan>) -> String {
    let Some(plan) = plan else {
        return String::new();
    };
    plan.members
        .iter()
        .map(|planned| match &planned.selected {
            Some(selected) if plan.whole_target => {
                format!("outputs-2 {}", selected_facts(selected))
            }
            Some(selected) => format!("outputs-3 {} {}", selected_facts(selected), planned.member),
            None => format!("real {}", planned.member),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One owned root's identity inside a slot key, without the layout's
/// namespace: a legacy key prefixes `outputs-2`, a profile key `outputs-3` and
/// the member.
fn selected_facts(selected: &Selected) -> String {
    format!(
        "{} {} {} {} g{}",
        selected.nonce().unwrap_or("-"),
        match selected.kind {
            RootKind::External => "external",
            RootKind::Local => "local",
        },
        selected.root.display(),
        selected.volume_uuid.as_deref().unwrap_or("-"),
        selected.generation
    )
}
