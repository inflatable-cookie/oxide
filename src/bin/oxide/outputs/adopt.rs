//! Explicit `adopt`, `detach`, and automatic first-conversion logic.

use super::bind::cancel_reservation;
use super::git::{first_link_git_answer, refuse_link_git_exposure_at, LinkGitVerdict};
use super::receipt::{
    link_path, member_destination, read_receipt_for_prune, remember_root, sync_directory,
    write_receipt, LayoutKind, Receipt, ReceiptState, Selected, PROFILE_RECEIPT_VERSION,
};
use super::root::{
    create_owned_directory, current_or_successor_spec, ensure_member_directory, ensure_root,
    external_choice, external_spec, link_state, local_spec, recreate_target_directory, spec_for,
    RootSpec,
};
use super::transition::{desired_selection, replay, transition_at, NonceAuthority, Start};
use super::{
    barrier, mode_label, output_store, refuse_unknown_custody, release_barrier, Context,
    TargetState, MEMBER,
};
use crate::process;
use crate::removable::{self, DriveMode};
use crate::store::{self, Store, WaitAccounting};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// `oxide outputs adopt`: the explicit first conversion and later relinks.
pub(in crate::outputs) fn adopt_command() -> Result<u8, String> {
    let context = Context::open()?;
    adopt_at(&context, output_store)
}

/// Whether this explicit adoption can publish an output transition, and so must
/// have the shared forward format and the registry established first
/// (oxide#217).
///
/// `prune::ensure_ready` takes the same admission lock the caller's publication
/// guard holds, so it can only run **before** that guard and, to keep a refusal
/// that publishes nothing effect-free, must not run on such a refusal. This is
/// the effect-free classification the adoption itself dispatches on, in the
/// creator's own order: a committed whole-target receipt refuses without writing,
/// every other receipt may publish a replay, relink or profile switch, a
/// positively missing or empty real target always converts the host profile
/// pair, and a real nonempty target converts only the members that are not
/// already real nonempty directories.
pub(in crate::outputs) fn adoption_publishes_a_transition(
    context: &Context,
    control: &Path,
) -> Result<bool, String> {
    if let Some(receipt) =
        read_receipt_for_prune(control, &context.workspace, &context.checkout_id)?
    {
        return Ok(!(receipt.layout() == LayoutKind::WholeTarget
            && receipt.state == ReceiptState::Committed));
    }
    match context.target()? {
        TargetState::Missing | TargetState::RealEmpty => Ok(true),
        TargetState::RealNonEmpty => {
            // A receiptless checkout's first conversion places exactly the host
            // profile pair here (a receipt may add more members, and any
            // receipt already answered above). Every state except a real
            // nonempty directory publishes a transition: a missing member or an
            // empty real one is created, and an owned or dangling member link is
            // relinked, because a first conversion has no committed selection.
            let mut publishes = false;
            for member in ["debug", "release"] {
                let state = link_state(
                    &link_path(&context.workspace, member),
                    &context.workspace,
                    &context.checkout_id,
                )?;
                match state {
                    TargetState::RealNonEmpty => {}
                    // The profile creator refuses this member; whatever an
                    // earlier member already published bounds the answer.
                    TargetState::Foreign => return Ok(publishes),
                    TargetState::Missing
                    | TargetState::RealEmpty
                    | TargetState::OwnedLink(_)
                    | TargetState::Dangling(_) => publishes = true,
                }
            }
            Ok(publishes)
        }
        _ => Ok(false),
    }
}

/// `oxide outputs adopt` against one already-opened context and a **lazy** pool
/// provider, so the verb decides before the pool is opened or coordinated and
/// the verb-level regressions can hand in a provider that fails.
///
/// The shipped order is kept **exactly**, including its error precedence: drive
/// mode, then the control directory, then the pool. The pool provider is
/// therefore called last, so the verb reports the same refusal a pre-feature
/// build reported when more than one precondition is unmet.
pub(in crate::outputs) fn adopt_at(
    context: &Context,
    pool: impl FnOnce() -> Result<Option<Store>, String>,
) -> Result<u8, String> {
    let mode = context.mode()?;
    if mode != Some(DriveMode::Permanent) {
        return Err(format!(
            "outputs adopt needs permanent drive mode (mode is {}): register the drive with `oxide pool register-removable <mount> --reserve <size> --cap <size> --mode permanent` or `oxide pool set-mode permanent`",
            mode_label(mode)
        ));
    }
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err(
            "no internal pool exists; run a building oxide command once so the control state exists"
                .to_owned(),
        );
    };
    let internal = pool()?;
    let Some(internal) = internal else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let internal = &internal;
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(internal, &context.workspace, &mut waits)?;
    let outcome = (|| {
        refuse_unknown_custody(&internal, &context.workspace, "relink")?;
        // Fence a pre-feature Oxide out of both pools before any link, receipt
        // or registry row can be published: an existing receipt may need a
        // replay write, and every conversion arm that publishes a transition
        // needs the registry `transition_at` requires (oxide#217). The
        // decision is made from effect-free facts here because initialization
        // takes the same admission lock the publication guard below holds.
        if adoption_publishes_a_transition(&context, &control)? {
            crate::prune::ensure_ready(&internal, &context.workspace)?;
        }
        let _admission = store::publication_guard(&control)?;
        let stored = read_receipt_for_prune(&control, &context.workspace, &context.checkout_id)?;
        let receipt = match stored {
            None => Receipt::first_with(
                &context.workspace,
                &context.checkout_id,
                LayoutKind::Profile,
            ),
            Some(receipt) => replay(&context, receipt)?,
        };
        if receipt.layout() == LayoutKind::WholeTarget {
            if !receipt.selects_nothing() {
                return Err(
                    "outputs adopt refused: this checkout uses the legacy whole-target layout; run `oxide outputs detach` first to restore a real target directory, then run `oxide outputs adopt`"
                        .to_owned(),
                );
            }
            let mut profile = receipt;
            profile.version = PROFILE_RECEIPT_VERSION.to_owned();
            profile.layout = LayoutKind::Profile;
            profile.links = Vec::new();
            return adopt_profile(&context, profile);
        }
        adopt_profile(&context, receipt)
    })();
    release_barrier(&internal, guard)?;
    let (receipt, selected, linked) = outcome?;
    if linked {
        println!(
            "outputs: {} now selects the owned {} root {} (generation {})",
            context.link().display(),
            selected.kind.label(),
            selected.root.display(),
            selected.generation
        );
        println!(
            "outputs: no existing output was moved, copied or deleted; CARGO_TARGET_DIR stays {}",
            context.link().display()
        );
    } else {
        println!(
            "outputs: {} already selects the owned {} root {} (generation {})",
            context.link().display(),
            selected.kind.label(),
            selected.root.display(),
            selected.generation
        );
    }
    println!(
        "outputs: receipt generation {}, {} known root(s)",
        receipt.generation,
        receipt.roots.len()
    );
    Ok(0)
}

/// The profile members under `root`: the host debug/release pair, plus every
/// other v1-form directory it already holds. Nothing is invented and no other
/// directory is classified.
pub(in crate::outputs) fn profile_members(root: &Path) -> Result<Vec<String>, String> {
    let mut names = vec!["debug".to_owned(), "release".to_owned()];
    for entry in fs::read_dir(root)
        .map_err(|error| format!("cannot read the output root {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // `file_type` does not follow a symlink, so a linked entry is never
        // classified as a directory here.
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        if crate::layout::Member::parse(&name).is_ok() {
            names.push(name);
            continue;
        }
        // One bounded second component: a positively identified plain triple
        // directory contributes only its own `debug` and `release` profile
        // directories. No third level and no other profile name is walked, so
        // a custom profile, a JSON target specification and anything else the
        // old root holds stay exactly as they are.
        if !crate::layout::plain_triple(&name) {
            continue;
        }
        for child in fs::read_dir(entry.path())
            .map_err(|error| format!("cannot read {}: {error}", entry.path().display()))?
        {
            let child = child.map_err(|error| error.to_string())?;
            let Some(leaf) = child.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if leaf != "debug" && leaf != "release" {
                continue;
            }
            if !child
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                continue;
            }
            let member = format!("{name}/{leaf}");
            if crate::layout::Member::parse(&member).is_ok() {
                names.push(member);
            }
        }
    }
    names.sort();
    names.dedup();
    crate::layout::validate_set(&names, crate::layout::MAX_RECEIPT_MEMBERS)?;
    Ok(names)
}

/// `oxide outputs adopt` for the bound profile layout: `target` stays a real
/// directory and every positively identified member becomes an owned link into
/// one root. A real nonempty member directory is never moved or converted; an
/// empty one converts only here.
pub(in crate::outputs) fn adopt_profile(
    context: &Context,
    receipt: Receipt,
) -> Result<(Receipt, Selected, bool), String> {
    adopt_profile_from(context, receipt, Start::Adopt, None)
}

/// The ONE profile-layout creator, shared by the explicit `oxide outputs adopt`
/// and by the automatic first conversion of a fresh checkout on the permanent
/// drive (oxide#160, S4b). `start` is the only difference: explicit adoption
/// may convert an empty real directory (and relink a stale one), while the
/// automatic conversion creates its members only where nothing exists.
pub(in crate::outputs) fn adopt_profile_from(
    context: &Context,
    receipt: Receipt,
    start: Start,
    captured: Option<&RootSpec>,
) -> Result<(Receipt, Selected, bool), String> {
    // The members this conversion establishes: the host debug/release pair
    // plus every member the receipt already places, sorted and prefix-free.
    let mut members = vec!["debug".to_owned(), "release".to_owned()];
    for placement in receipt.links() {
        members.push(placement.member().to_owned());
    }
    let parsed = members
        .iter()
        .map(|name| crate::layout::Member::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    let declaration = crate::layout::Declaration::new(parsed)?;
    let target = context.target()?;
    let action = if start == Start::Adopt {
        "outputs adopt"
    } else {
        "auto-adopt"
    };
    match &target {
        TargetState::Missing => {
            // The first conversion's ignore prerequisite (oxide#125) is checked
            // for EVERY member link it is about to publish before the real
            // target directory exists, so a refused conversion changes nothing
            // at all (oxide#152's rule). `transition_at` rechecks each one
            // under its own prepared receipt.
            for member in declaration.members() {
                refuse_link_git_exposure_at(
                    context,
                    &link_path(&context.workspace, member.name()),
                    Some(member.name()),
                )?;
            }
            // The lexical CARGO_TARGET_DIR stays real. Adoption creates it so
            // Cargo's own top-level files land in a real directory.
            create_owned_directory(&context.link())?;
            fs::set_permissions(&context.link(), fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("cannot set permissions on the target: {error}"))?;
            sync_directory(&context.workspace)?;
        }
        TargetState::RealEmpty | TargetState::RealNonEmpty if start == Start::Adopt => {}
        TargetState::OwnedLink(root) => {
            return Err(format!(
                "{action} refused: {} is an owned whole-target link to {}, but no committed whole-target receipt places one there; it is kept exactly as it is. Run `oxide outputs detach` then `oxide outputs adopt` for a committed whole-target checkout; an unrecorded link is never adopted",
                context.link().display(),
                root.display()
            ))
        }
        TargetState::Dangling(root) => {
            return Err(format!(
                "{action} refused: {} links to {}, which is not available; it is left untouched",
                context.link().display(),
                root.display()
            ))
        }
        TargetState::Foreign => {
            return Err(format!(
                "{action} refused: {} is {}; Oxide never replaces it",
                context.link().display(),
                target.label()
            ))
        }
        // The automatic first conversion creates its members only where
        // nothing exists: an empty or populated real directory appearing where
        // `target` was positively absent is not adopted, converted or kept.
        state => {
            return Err(format!(
                "{action} refused: {} is {}; an automatic first conversion creates no member where anything exists. Nothing was changed. Run `oxide outputs adopt` yourself if you accept converting an empty directory",
                context.link().display(),
                state.label()
            ))
        }
    }
    // The explicit adoption decides its own place from the live registration.
    // The automatic conversion uses exactly the root it captured and verified
    // twice before this point, so it can never be redirected to the
    // checkout-local fallback behind the owner's back.
    let desired = match start {
        Start::MissingOnly => None,
        Start::Any | Start::Adopt => desired_selection(context, &receipt)?,
    };
    let spec = match (start, captured) {
        (Start::MissingOnly, Some(spec)) => spec.clone(),
        (Start::MissingOnly, None) => {
            return Err(
                "outputs: auto-adopt has no captured root selection; nothing was changed"
                    .to_owned(),
            )
        }
        (_, _) => {
            let desired = match &desired {
                Some(desired) => desired.clone(),
                None => {
                    let usable = external_choice(context)?;
                    let spec = match &usable {
                        Ok(registration) => external_spec(registration, &context.workspace),
                        Err(_) => local_spec(&context.workspace),
                    };
                    spec.selected(receipt.generation.saturating_add(1))
                }
            };
            spec_for(&desired)
        }
    };
    let spec = current_or_successor_spec(context, &receipt, spec)?;
    let wanted = match &desired {
        Some(desired) => Some(desired.clone()),
        None => Some(spec.selected(receipt.generation.saturating_add(1))),
    };
    let mut committed = receipt;
    let mut converted = false;
    let mut kept = Vec::new();
    for member in declaration.members() {
        let link = link_path(&context.workspace, member.name());
        let state = link_state(&link, &context.workspace, &context.checkout_id)?;
        match state {
            // A real nonempty member is kept local, but only an explicit
            // adoption may do that; the automatic conversion never leaves a
            // half-converted member behind.
            TargetState::RealNonEmpty if start == Start::Adopt => {
                kept.push(member.name().to_owned());
                continue;
            }
            TargetState::RealEmpty if start == Start::Adopt => {}
            TargetState::Missing => {}
            TargetState::OwnedLink(root)
                if start == Start::Adopt
                    && wanted.as_ref().is_some_and(|desired| {
                        root == member_destination(&desired.root, member.name())
                            && committed
                                .selected_for(member.name())
                                .is_some_and(|selected| selected.same_place(desired))
                    }) =>
            {
                // Already the desired destination this adoption would select:
                // idempotent, nothing is written. A link that still matches an
                // older receipt but not the desired place is relinked below.
                continue;
            }
            TargetState::OwnedLink(_) | TargetState::Dangling(_) if start == Start::Adopt => {}
            TargetState::Foreign => {
                return Err(format!(
                    "{action} refused: {} is {}; it is never followed or replaced",
                    link.display(),
                    state.label()
                ))
            }
            state => {
                return Err(format!(
                    "{action} refused: {} is {}; an automatic first conversion creates no member where anything exists. Nothing was changed. Run `oxide outputs adopt` yourself if you accept converting an empty directory",
                    link.display(),
                    state.label()
                ))
            }
        }
        committed = transition_at(
            &context,
            &committed,
            member.name(),
            &spec,
            start,
            NonceAuthority::Unkeyed,
        )?;
        converted = true;
    }
    let selected = match committed.selected().cloned().or(desired) {
        Some(selected) => selected,
        None => return Err("the adoption committed no root".to_owned()),
    };
    if converted {
        ensure_root(context, &spec, committed.generation)?;
    }
    if !kept.is_empty() {
        eprintln!(
            "oxide: profile outputs: {} stay real directories under {} (an existing nonempty directory is never moved or converted)",
            kept.join(", "),
            context.link().display()
        );
    }
    Ok((committed, selected, converted))
}

pub(in crate::outputs) fn refuse_existing_target_at(target: &TargetState, link: &Path) -> String {
    format!(
        "outputs adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed. {}",
        link.display(),
        target.label(),
        match target {
            TargetState::RealNonEmpty =>
                "Move it aside yourself if you accept losing its warm build, then re-run `oxide outputs adopt`.",
            TargetState::OwnedLink(_) | TargetState::Dangling(_) =>
                "Its receipt does not select this root, so Oxide will not touch it.",
            _ =>
                "Remove or restore it yourself, then re-run `oxide outputs adopt`.",
        }
    )
}

/// The facts the automatic first conversion decides on. Every field must hold;
/// any other state keeps today's behaviour.
#[derive(Debug, Eq, PartialEq)]
pub(in crate::outputs) struct AutoFacts {
    /// This command's intermediate slot came from the removable pool under a
    /// registration in permanent mode, as selected at its start.
    pub(in crate::outputs) selected_permanent: bool,
    /// The external root the registration in force now selects is the one the
    /// command's selection captured: same volume UUID and root.
    pub(in crate::outputs) same_selection: bool,
    /// The registered permanent volume is usable right now.
    pub(in crate::outputs) external_usable: bool,
    /// The registration in force now is in permanent mode.
    pub(in crate::outputs) permanent: bool,
    /// No owned-output receipt of any state, including `detached`.
    pub(in crate::outputs) receipt_absent: bool,
    /// `<checkout>/target` is positively absent: no file, directory or link.
    pub(in crate::outputs) target_absent: bool,
}

pub(in crate::outputs) fn auto_adopts(facts: &AutoFacts) -> bool {
    facts.selected_permanent
        && facts.same_selection
        && facts.external_usable
        && facts.permanent
        && facts.receipt_absent
        && facts.target_absent
}

/// Whether the external root a registration selects is the one a command
/// captured at its selection (same volume, same root).
pub(in crate::outputs) fn selection_matches(captured: &Selected, current: &Selected) -> bool {
    captured.same_place(current)
}

/// The member links an automatic first conversion publishes: `debug` and
/// `release` (`adopt_profile_from`'s declaration for a fresh receipt).
pub(in crate::outputs) const AUTO_ADOPT_MEMBERS: [&str; 2] = ["debug", "release"];

/// Automatic first conversion (oxide#121), run once per leased command before
/// its destination is planned, its key is built or its slot is taken. The
/// command's own selection must be a removable permanent one, and the volume
/// in force now must be the one that selection captured. A checkout with no
/// receipt of any state and a positively absent target then converts under the
/// same barrier, fences, admission lock and receipt as `oxide outputs adopt`,
/// with the missing-target-only transition. Any other state is left to the
/// existing paths untouched. A metadata error refuses; it is never read as
/// absence.
///
/// The first conversion's ignore prerequisite decides *eligibility*, not the
/// command (oxide#154): when Git cannot confirm a link path this conversion
/// would publish is ignored by a rule that covers it, the command is an
/// ordinary unconverted checkout. Nothing is written — no receipt, root,
/// link, notice or downgrade fence — so the build keeps its real local
/// target and the binding prints its one unconverted warning. An explicit
/// `oxide outputs adopt` still refuses with the exact fix, and the transition
/// rechecks the same prerequisite before its first effect, so a concurrent
/// change refuses rather than publishing a link Git does not cover.
///
/// Since oxide#160 the first conversion creates the profile layout, so
/// eligibility is asked for every member link it would publish
/// (`adopt_profile_from`'s declaration for a fresh receipt), exactly as that
/// creator checks them before it creates the real `target` directory. Asking
/// the member paths also keeps a directory-only rule such as `target/`
/// eligible, because it genuinely covers `target/<member>`.
pub fn auto_adopt(
    store: &Store,
    checkout: &Path,
    selection: &removable::Selected,
) -> Result<(), String> {
    let selected_permanent = selection.removable && selection.mode == Some(DriveMode::Permanent);
    let Some(captured_registration) = selection
        .registration
        .as_ref()
        .filter(|_| selected_permanent)
    else {
        return Ok(());
    };
    let captured = external_spec(captured_registration, checkout).selected(0);
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        checkout.to_path_buf(),
    );
    let current = external_choice(&context)?;
    let facts = AutoFacts {
        selected_permanent,
        same_selection: current.as_ref().is_ok_and(|registration| {
            selection_matches(
                &captured,
                &external_spec(registration, checkout).selected(0),
            )
        }),
        external_usable: current.is_ok(),
        permanent: context.mode()? == Some(DriveMode::Permanent),
        receipt_absent: context.receipt()?.is_none(),
        target_absent: context.target()? == TargetState::Missing,
    };
    if !auto_adopts(&facts) {
        return Ok(());
    }
    auto_adopt_fresh(store, &context, &captured, || {
        Ok(external_choice(&context)?
            .ok()
            .map(|registration| external_spec(&registration, checkout)))
    })
}

/// Convert a fresh checkout, consulting the current placement again while
/// admission is held. The placement lookup is separate from publication so
/// the same transition can run on a private directory fixture.
pub(in crate::outputs) fn auto_adopt_fresh(
    store: &Store,
    context: &Context,
    captured: &Selected,
    current: impl FnOnce() -> Result<Option<RootSpec>, String>,
) -> Result<(), String> {
    // Expected automatic-adoption ineligibility must leave the build usable
    // (oxide#154). The prerequisite is asked before the barrier, the
    // downgrade fence or the admission lock, so an ineligible checkout gets
    // no side effect at all: it is exactly the checkout this command found.
    // It asks every member link the profile-layout first conversion would
    // publish (oxide#160), the same set `adopt_profile_from` checks before it
    // creates the real target directory.
    for member in AUTO_ADOPT_MEMBERS {
        if first_link_git_answer(context, Some(member)).verdict != LinkGitVerdict::Eligible {
            return Ok(());
        }
    }
    let checkout = &context.workspace;
    let mut waits = WaitAccounting::default();
    let guard = barrier(store, checkout, &mut waits)?;
    let outcome: Result<Option<Selected>, String> = (|| {
        refuse_unknown_custody(store, checkout, "auto-adopt")?;
        // Fence a pre-feature Oxide out of both pools before any link can be
        // published, as an explicit adopt does for a missing target.
        crate::prune::ensure_ready(store, checkout)?;
        let control = context
            .control()
            .ok_or("no internal pool exists, so no removable drive is registered")?;
        let _admission = store::publication_guard(control)?;
        // Every fact is rechecked under the admission lock: a concurrent
        // creator, receipt, mode or volume switch makes this a no-op.
        if context.receipt()?.is_some() || context.target()? != TargetState::Missing {
            return Ok(None);
        }
        let Some(spec) = current()? else {
            return Ok(None);
        };
        if !selection_matches(captured, &spec.selected(0)) {
            return Ok(None);
        }
        // The first conversion creates the profile layout through the SAME
        // creator an explicit `oxide outputs adopt` uses, with the
        // missing-target-only start this path has always required (oxide#160,
        // S4b): a real `target` directory holding an owned `debug`/`release`
        // pair into one new owned root, and a receipt and root marker
        // identical in form to a fresh explicit adoption.
        let receipt = Receipt::first_with(checkout, &context.checkout_id, LayoutKind::Profile);
        let (_committed, selected, _converted) =
            adopt_profile_from(context, receipt, Start::MissingOnly, Some(&spec))?;
        Ok(Some(selected))
    })();
    release_barrier(store, guard)?;
    if let Some(selected) = outcome? {
        eprintln!(
            "oxide: auto-adopted {} -> {} for permanent-drive outputs; no existing outputs moved",
            context.link().display(),
            selected.root.display()
        );
    }
    Ok(())
}

/// `oxide outputs detach`: back to a real target directory, keeping roots.
/// It is a per-checkout opt-out in either drive mode (oxide#124): the
/// detached receipt stays, so automatic first conversion never follows it.
pub(in crate::outputs) fn detach_command() -> Result<u8, String> {
    let context = Context::open()?;
    detach_at(&context, output_store)
}

/// `oxide outputs detach` against one already-opened context and a **lazy**
/// pool provider, so the verb decides before the pool is opened.
pub(in crate::outputs) fn detach_at(
    context: &Context,
    pool: impl FnOnce() -> Result<Option<Store>, String>,
) -> Result<u8, String> {
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = pool()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let internal = &internal;
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(internal, &context.workspace, &mut waits)?;
    let outcome = (|| {
        refuse_unknown_custody(&internal, &context.workspace, "detach")?;
        // A pool adopted before the shared output format upgrades behind
        // the forward fence before its first receipt write (a no-op once
        // ready); a missing receipt changes nothing.
        if read_receipt_for_prune(&control, &context.workspace, &context.checkout_id)?.is_some() {
            crate::prune::ensure_ready(&internal, &context.workspace)?;
        }
        // Detach selects nothing, so an unused reservation is cancelled
        // through its journal first, before the admission lock.
        if let Some(mut reserved) =
            read_receipt_for_prune(&control, &context.workspace, &context.checkout_id)?
                .filter(|receipt| receipt.reserved().is_some())
        {
            let _binding = internal.lock_binding(&context.workspace)?;
            cancel_reservation(&internal, &control, &context, &mut reserved)?;
        }
        let _admission = store::publication_guard(&control)?;
        let receipt = read_receipt_for_prune(&control, &context.workspace, &context.checkout_id)?
            .ok_or("this checkout has no owned-output receipt; nothing to detach")?;
        let receipt = replay(&context, receipt)?;
        if receipt.state == ReceiptState::Detached {
            // The opt-out is already durable. Finish any effect the crash
            // between that receipt and its real directories may have skipped.
            let changed = settle_detached(&context, &receipt)?;
            return Ok((receipt, changed));
        }
        if receipt.layout() == LayoutKind::Profile {
            return detach_profile(&context, &control, receipt);
        }
        let selected = receipt
            .selected()
            .cloned()
            .ok_or("the receipt selects no root; nothing to detach")?;
        let target = context.target()?;
        if target.link_root() != Some(selected.root.as_path()) {
            return Err(format!(
                "outputs detach refused: {} is {}, not the owned link selecting {}; it is left untouched",
                context.link().display(),
                target.label(),
                selected.root.display()
            ));
        }
        // Write-ahead: the opt-out is durable before the link goes, so a crash
        // in the window leaves a Detached receipt the next detach (or binding)
        // finishes, never a Committed receipt with a missing target that the
        // next bind would silently re-link.
        let detached = detached_receipt(&receipt);
        write_receipt(&control, &detached)?;
        context.crash_point("detach-receipt")?;
        // The same effects the crash-window recovery applies: one function
        // describes what a `Detached` receipt means on disk.
        settle_detached(&context, &detached)?;
        Ok((detached, true))
    })();
    release_barrier(&internal, guard)?;
    let (receipt, detached) = outcome?;
    if detached {
        println!(
            "outputs: {} is a real empty directory again; no output root was moved or deleted",
            context.link().display()
        );
    } else {
        println!("outputs: this checkout is already detached from owned outputs");
    }
    for root in &receipt.roots {
        println!(
            "outputs: preserved {} root {} ({})",
            root.kind.label(),
            root.root.display(),
            if root.root.is_dir() {
                "present"
            } else {
                "absent"
            }
        );
    }
    Ok(0)
}

/// `oxide outputs detach` for the profile layout: every owned member link
/// becomes a real empty directory again and `target` stays real. No root is
/// moved or deleted; the receipt keeps its identity and known roots.
///
/// The `Detached` receipt is durable before the first link is converted, so a
/// crash leaves a checkout whose recorded opt-out the next detach finishes;
/// the link-removal ordering can never leave a Committed receipt over real
/// directories that a later bind would silently re-link.
pub(in crate::outputs) fn detach_profile(
    context: &Context,
    control: &Path,
    receipt: Receipt,
) -> Result<(Receipt, bool), String> {
    if receipt.state != ReceiptState::Committed {
        return Err(
            "outputs detach: an interrupted transition is pending; nothing was changed".to_owned(),
        );
    }
    // Verify every recorded link before the receipt is written: only this
    // checkout's own link to the root its placement names, or an absent one, is
    // a detach entry. Anything else refuses with nothing changed.
    for placement in receipt.links() {
        let Some(selected) = placement.selected_ref() else {
            continue;
        };
        let link = link_path(&context.workspace, placement.member());
        match link_state(&link, &context.workspace, &context.checkout_id)? {
            TargetState::Missing => {}
            TargetState::OwnedLink(root)
                if root == member_destination(selected.root(), placement.member()) => {}
            state => {
                return Err(format!(
                    "outputs detach refused: {} is {}, not this checkout's owned link to {}; it is left untouched",
                    link.display(),
                    state.label(),
                    selected.root.display()
                ))
            }
        }
    }
    match context.target()? {
        TargetState::Missing | TargetState::RealEmpty | TargetState::RealNonEmpty => {}
        state => {
            return Err(format!(
                "outputs detach refused: {} is {}; it is left untouched",
                context.link().display(),
                state.label()
            ))
        }
    }
    let detached = detached_receipt(&receipt);
    write_receipt(control, &detached)?;
    context.crash_point("detach-receipt")?;
    settle_detached(context, &detached)?;
    Ok((detached, true))
}

/// The receipt of a checkout that selects nothing: every placement is
/// cleared, each former root joins the known-root history, and the state
/// records the opt-out a later automatic conversion never follows.
fn detached_receipt(receipt: &Receipt) -> Receipt {
    let mut detached = receipt.clone();
    let mut previous = Vec::new();
    for placement in &mut detached.links {
        if let Some(selected) = placement.selected.take() {
            previous.push(selected);
        }
        placement.prepared = None;
    }
    for root in previous {
        remember_root(&mut detached, root);
    }
    detached.state = ReceiptState::Detached;
    detached.updated_ms = process::now_ms();
    detached
}

/// Turn one owned link into the real empty directory a detach restores. The
/// caller has already written the `Detached` receipt, so a failure here is
/// recoverable by re-running `oxide outputs detach`.
fn replace_link_with_directory(link: &Path) -> Result<(), String> {
    fs::remove_file(link).map_err(|error| {
        format!(
            "cannot remove the owned link {}: {error}; re-run `oxide outputs detach` to finish",
            link.display()
        )
    })?;
    if let Err(error) = fs::create_dir(link) {
        return Err(format!(
            "the owned link {} was removed, but creating the real directory failed: {error}; re-run `oxide outputs detach` to finish",
            link.display()
        ));
    }
    fs::set_permissions(link, fs::Permissions::from_mode(0o755)).map_err(|error| {
        format!(
            "cannot set permissions on {}: {error}; re-run `oxide outputs detach` to finish",
            link.display()
        )
    })
}

/// Finish a detach whose `Detached` receipt is durable but whose real
/// directories were not all created: the crash window between the receipt and
/// the link removals. Only an entry this receipt's known roots still name is
/// converted; every other entry is left untouched and refused.
pub(in crate::outputs) fn settle_detached(
    context: &Context,
    receipt: &Receipt,
) -> Result<bool, String> {
    let mut changed = settle_detached_entry(context, receipt, MEMBER)?;
    if receipt.layout() == LayoutKind::Profile {
        for placement in receipt.links() {
            if placement.member() == MEMBER {
                continue;
            }
            changed |= settle_detached_entry(context, receipt, placement.member())?;
        }
    }
    if changed {
        // `target` itself is an entry of the checkout, and every profile member
        // it holds is an entry of `target`.
        sync_directory(&context.workspace)?;
        if receipt.layout() == LayoutKind::Profile && context.link().is_dir() {
            sync_directory(&context.link())?;
        }
    }
    Ok(changed)
}

/// One entry of [`settle_detached`]: the whole-target link or one profile
/// member link.
fn settle_detached_entry(
    context: &Context,
    receipt: &Receipt,
    member: &str,
) -> Result<bool, String> {
    let entry = link_path(&context.workspace, member);
    match link_state(&entry, &context.workspace, &context.checkout_id)? {
        TargetState::Missing => {
            if member == MEMBER {
                recreate_target_directory(&context.workspace)?;
            } else {
                ensure_member_directory(&context.link(), member)?;
            }
            Ok(true)
        }
        TargetState::RealEmpty | TargetState::RealNonEmpty => Ok(false),
        TargetState::OwnedLink(root)
            if receipt
                .roots
                .iter()
                .any(|known| member_destination(&known.root, member) == root) =>
        {
            replace_link_with_directory(&entry)?;
            Ok(true)
        }
        state => Err(format!(
            "outputs detach refused: {} is {} and is not this checkout's owned link to a preserved root; it is left untouched",
            entry.display(),
            state.label()
        )),
    }
}
