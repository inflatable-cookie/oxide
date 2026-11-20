//! Per-command output binding, root verification and future-root reservations.

use super::exec::{refuses_committed_external, refuses_dangling_member, Plan};
use super::receipt::{
    link_path, member_destination, read_marker, read_receipt, write_receipt, LayoutKind, Receipt,
    ReceiptState, Reservation, RootKind, Selected,
};
use super::root::{
    binding_for_selected, ensure_member_directory, external_choice, is_real_dir, link_state,
    recreate_target_directory, spec_for, validate_selected,
};
use super::transition::{
    desired_selection, replay, transition, transition_at, NonceAuthority, Start,
};
use super::{barrier, output_store, release_barrier, Context, OutputsError, TargetState, MARKER};
use crate::identity;
use crate::process;
use crate::removable::{self, DriveMode};
use crate::store::{CheckoutCustody, Store, WaitAccounting};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What one command did to its checkout's owned outputs.
#[derive(Default)]
pub struct Outcome {
    /// Bounded, single-line warnings, printed before Cargo starts.
    pub warnings: Vec<String>,
    /// One clause for the command's summary line.
    pub note: Option<String>,
    /// The real owned root the checkout's owned link selects for this
    /// command; `None` for a real `target` directory.
    pub root: Option<PathBuf>,
}

impl Outcome {
    pub(in crate::outputs) fn with_note(mut self, note: String) -> Self {
        self.note = Some(note);
        self
    }
}

/// A command's bound output destination for the life of its start-up. The
/// lock is the checkout binding mutex, held from the committed link check (or
/// the transition) until Cargo has started, so a peer's transition cannot
/// repoint the link in between.
pub struct OutputBinding {
    pub outcome: Outcome,
    _lock: Option<File>,
    pub(in crate::outputs) _holds: Vec<barge::Hold>,
    ephemeral: bool,
}

impl OutputBinding {
    fn unchecked(outcome: Outcome) -> Self {
        Self {
            outcome,
            _lock: None,
            _holds: Vec::new(),
            ephemeral: false,
        }
    }
}

/// How long a command waits before re-checking a checkout whose other command
/// must finish before an output relink.
const BINDING_POLL: Duration = Duration::from_millis(200);

/// The one notice a binding prints while another command in this checkout may
/// still be writing, so a relink waits instead of racing it.
const WAITING_NOTICE: &str = "oxide: waiting for another oxide command in this checkout (a build, an exec tool or a program `oxide run` started) to finish before relinking outputs";

/// A build binds its checkout's output root once, before Cargo starts. The
/// check and any transition run under the checkout's binding mutex, a plain
/// per-checkout lock that no holder can wait on: second-slot commands still
/// start concurrently when their destination is unchanged, and two commands
/// can never block each other through it. A relink waits (without the mutex)
/// until no other command in the checkout may be writing Cargo, and refuses
/// outright on unknown writer custody.
///
/// `expected` is the destination the slot key was computed from, including the
/// root generation. The command binds exactly that root and generation or
/// refuses, so a slot and its uplifted artifacts never belong to different
/// roots.
///
/// `repair_absent_members` is set by a derived `oxide exec` only (oxide#160's
/// member inference, oxide#162): a receipt-recorded member link whose one
/// defect is an absent destination directory beneath the committed, verified
/// root has that directory created under this checkout's custody. Every other
/// command keeps the existing refusal, so a dangling member link never
/// transitions automatically.
pub fn bind_for_command(
    store: &Store,
    checkout: &Path,
    own_record: u64,
    expected: Option<&Plan>,
    pool_fallback: Option<&str>,
    repair_absent_members: bool,
) -> Result<OutputBinding, String> {
    let (mut binding, receipt) = bind_for_command_inner(
        store,
        checkout,
        own_record,
        expected,
        pool_fallback,
        repair_absent_members,
    )?;
    // The running holds come from the very receipt the binding decision used,
    // never from a second read that a peer transition could have replaced.
    if let Some(receipt) = receipt {
        let id = identity::digest(&checkout.to_string_lossy());
        binding._holds = crate::prune::hold_outputs(store.owner_control(), &receipt, |member| {
            // Keep the receipt's durable pin. A member that the command can
            // only reach locally needs no running hold on its former root.
            !binding.ephemeral
                || receipt.layout() != LayoutKind::Profile
                || !ephemeral_member_state(checkout, &id, member)
                    .is_ok_and(|state| is_local_member(&state))
        })?;
    }
    Ok(binding)
}

/// The binding decision and the receipt it was taken on. `None` is returned
/// only where no receipt took part (an unconverted checkout), so no hold is
/// taken either.
fn bind_for_command_inner(
    store: &Store,
    checkout: &Path,
    own_record: u64,
    expected: Option<&Plan>,
    pool_fallback: Option<&str>,
    repair_absent_members: bool,
) -> Result<(OutputBinding, Option<Receipt>), String> {
    let context = Context::new(
        Some(store.owner_control().to_path_buf()),
        checkout.to_path_buf(),
    )
    .with_pool_fallback(pool_fallback);
    let mode = context.mode()?;
    if mode != Some(DriveMode::Permanent) {
        // A converted checkout's command holds the binding mutex until it is
        // recorded as running, in either mode, so a permanent-mode relink
        // never repoints the link between this check and Cargo's start.
        let lock = store.lock_binding(checkout)?;
        let Some(receipt) = context.receipt()? else {
            keyed_state_matches(None, expected)?;
            return Ok((OutputBinding::unchecked(Outcome::default()), None));
        };
        let (outcome, receipt) = bind_ephemeral(&context, receipt, expected)?;
        return Ok((
            OutputBinding {
                outcome,
                _lock: Some(lock),
                _holds: Vec::new(),
                ephemeral: mode == Some(DriveMode::Ephemeral),
            },
            Some(receipt),
        ));
    }
    if context.receipt()?.is_none() {
        keyed_state_matches(None, expected)?;
        refuse_unowned_link(&context, "outputs")?;
        return Ok((OutputBinding::unchecked(unconverted_outcome()), None));
    }
    let mut announced = false;
    loop {
        if let Some(signal) = process::received_signal() {
            return Err(format!(
                "interrupted by signal {signal} while waiting for this checkout's other commands; nothing was changed"
            ));
        }
        let lock = store.lock_binding(checkout)?;
        let Some(receipt) = context.receipt()? else {
            keyed_state_matches(None, expected)?;
            refuse_unowned_link(&context, "outputs")?;
            return Ok((
                OutputBinding {
                    outcome: unconverted_outcome(),
                    _lock: Some(lock),
                    _holds: Vec::new(),
                    ephemeral: false,
                },
                None,
            ));
        };
        keyed_state_matches(Some(&receipt), expected)?;
        let receipt = match gated_replay(&context, store, own_record, receipt)? {
            Some(receipt) => receipt,
            None => {
                drop(lock);
                if !announced {
                    eprintln!("{WAITING_NOTICE}");
                    announced = true;
                }
                std::thread::sleep(BINDING_POLL);
                continue;
            }
        };
        match bind_under_binding(
            &context,
            store,
            own_record,
            &receipt,
            expected,
            repair_absent_members,
        ) {
            Bind::Bound { outcome, receipt } => {
                return Ok((
                    OutputBinding {
                        outcome,
                        _lock: Some(lock),
                        _holds: Vec::new(),
                        ephemeral: false,
                    },
                    Some(receipt),
                ))
            }
            Bind::Refuse(error) => return Err(error.into()),
            Bind::Wait => {
                drop(lock);
                if !announced {
                    eprintln!("{WAITING_NOTICE}");
                    announced = true;
                }
                std::thread::sleep(BINDING_POLL);
            }
        }
    }
}

/// Replay a prepared receipt under the binding mutex only after the same
/// cross-pool custody rule as a fresh transition: unknown writer custody
/// refuses with the link, sibling, receipt and roots unchanged, and a
/// command that may be writing must close first (`None`: wait). A committed
/// or detached receipt passes through untouched.
pub(in crate::outputs) fn gated_replay(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: Receipt,
) -> Result<Option<Receipt>, String> {
    if receipt.state != ReceiptState::Prepared {
        return Ok(Some(receipt));
    }
    match store.checkout_custody(&context.workspace, own_record)? {
        CheckoutCustody::Unknown(reason) => Err(format!(
            "outputs: refusing to replay the interrupted transition for {}: {reason}. The link, the prepared receipt and both roots are kept until that custody is resolved",
            context.link().display()
        )),
        CheckoutCustody::Writers => Ok(None),
        CheckoutCustody::Clear => replay(context, receipt).map(Some),
    }
}

/// A slot is keyed with or without an owned destination when its command
/// starts. If the checkout was converted (or detached) while the command
/// waited for admission, the slot's key no longer describes where Cargo
/// would write: refuse before Cargo so the next attempt keys afresh.
pub(in crate::outputs) fn keyed_state_matches(
    receipt: Option<&Receipt>,
    expected: Option<&Plan>,
) -> Result<(), String> {
    let converted = receipt.is_some_and(|receipt| !receipt.selects_nothing());
    keyed_state_refusal(converted, expected)
}

/// The shipped key mismatch refusal for one already-decided `converted` fact.
fn keyed_state_refusal(converted: bool, expected: Option<&Plan>) -> Result<(), String> {
    if converted == expected.is_some() {
        return Ok(());
    }
    Err(format!(
        "outputs: this checkout was {} while this command waited, so its slot was keyed for {}; nothing was built. Run the command again",
        if converted { "converted to owned outputs" } else { "detached from owned outputs" },
        if converted { "an unconverted target" } else { "an owned root" }
    ))
}

/// In permanent mode a checkout without a committed owned link keeps a real
/// `target`: a link there was never established as owned (a foreign link, or
/// a marked link whose receipt is missing), so Cargo is never handed it and
/// clean never follows it. Ephemeral mode keeps today's behaviour.
pub(in crate::outputs) fn refuse_unowned_link(
    context: &Context,
    action: &str,
) -> Result<(), String> {
    let link = context.link();
    match fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "{action} refused: {} is a link that no committed owned-output receipt established; permanent drive mode never follows it. Nothing was changed. Replace it with a real directory, or remove it and run `oxide outputs adopt`",
            link.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{action} refused: cannot inspect {}: {error}", link.display())),
    }
}

fn unconverted_outcome() -> Outcome {
    Outcome {
        warnings: vec![
            "permanent drive mode: this checkout is not converted to owned outputs; `oxide outputs plan` shows the conversion plan and `oxide outputs adopt` performs it. Nothing was moved"
                .to_owned(),
        ],
        note: Some("outputs not converted".to_owned()),
        root: None,
    }
}

/// Ephemeral mode keeps today's real checkout/target. A checkout that was
/// converted keeps its owned link, is never relinked automatically, and
/// refuses when the link no longer matches the receipt or the root's full
/// identity.
pub(in crate::outputs) fn bind_ephemeral(
    context: &Context,
    receipt: Receipt,
    expected: Option<&Plan>,
) -> Result<(Outcome, Receipt), String> {
    let expected_selected = expected.and_then(Plan::selected);
    if receipt.state != ReceiptState::Prepared {
        keyed_state_matches(Some(&receipt), expected)?;
    }
    match receipt.state {
        // A receipt that selects nothing owns no destination in either mode.
        ReceiptState::Detached | ReceiptState::Pruned => {
            Ok((Outcome::default(), receipt))
        }
        ReceiptState::Prepared => Err(
            "outputs: an interrupted owned-output transition is pending; restore or set a permanent drive registration, then run `oxide outputs adopt` to replay it before building"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            if receipt.layout() == LayoutKind::Profile {
                // Ephemeral mode never relinks a converted checkout. The
                // profile layout is validated exactly as keyed: each owned
                // member link selects the receipt's root and real or missing
                // members are used locally.
                return Ok((
                    bind_profile_sealed(context, &receipt, expected)?,
                    receipt,
                ));
            }
            let wanted = match expected_selected {
                Some(expected) => expected.clone(),
                None => desired_selection(context, &receipt)?
                    .ok_or("outputs: the receipt selects no root")?,
            };
            if wanted.kind == RootKind::External {
                if let Some(reason) = context.pool_fallback.as_deref() {
                    return Err(format!(
                        "outputs: {} selects the external root {}, but this command's intermediate slot fell back to the internal pool ({reason}) and the drive's removal would not be supervised; the drive mode is ephemeral, so Oxide will not relink it. Restore the drive or run `oxide outputs detach`",
                        context.link().display(),
                        wanted.root.display()
                    ));
                }
            }
            let target = context.target()?;
            if target.root() != Some(wanted.root.as_path()) {
                return Err(format!(
                    "outputs: {} is {}, not the owned link selecting {}; the drive mode is ephemeral, so Oxide will not relink it. Run `oxide outputs detach` for a real target directory, or set the drive mode to permanent and run `oxide outputs adopt`",
                    context.link().display(),
                    target.label(),
                    wanted.root.display()
                ));
            }
            check_bound_root(context, receipt.generation, &wanted)?;
            let outcome = Outcome {
                root: Some(wanted.root.clone()),
                ..Outcome::default()
            };
            Ok((
                outcome.with_note(format!(
                    "owned outputs {} through an owned link",
                    wanted.root.display()
                )),
                receipt,
            ))
        }
    }
}

/// Ephemeral-mode binding for a converted profile checkout. Cargo can create
/// a missing local directory after binding. Existing links must still be live
/// and match their keyed, receipt-recorded roots.
pub(in crate::outputs) fn bind_profile_sealed(
    context: &Context,
    receipt: &Receipt,
    expected: Option<&Plan>,
) -> Result<Outcome, String> {
    let Some(plan) = expected else {
        return Err(
            "outputs: this checkout places outputs at profile members but its slot was keyed with no owned destination; run the command again"
                .to_owned(),
        );
    };
    if plan.whole_target {
        return Err(
            "outputs: this checkout's layout changed to profile members while its slot was keyed for the whole target; nothing was built. Run the command again"
                .to_owned(),
        );
    }
    let ephemeral = context.mode()? == Some(DriveMode::Ephemeral);
    let mut outcome = Outcome::default();
    let mut real = Vec::new();
    for planned in &plan.members {
        let link = link_path(&context.workspace, &planned.member);
        let state = if ephemeral {
            ephemeral_member_state(&context.workspace, &context.checkout_id, &planned.member)?
        } else {
            link_state(&link, &context.workspace, &context.checkout_id)?
        };
        match (&planned.selected, &state) {
            (Some(wanted), TargetState::OwnedLink(root))
                if (!ephemeral || receipt.selected_for(&planned.member) == Some(wanted))
                    && root == &member_destination(&wanted.root, &planned.member) =>
            {
                if let Some(reason) = context.pool_fallback.as_deref().filter(|_| wanted.kind == RootKind::External) {
                    return Err(format!(
                        "outputs: {} selects the external root {}, but this command's intermediate slot fell back to the internal pool ({reason}) and the drive's removal would not be supervised. Restore the drive or run `oxide outputs detach`",
                        link.display(),
                        wanted.root.display()
                    ));
                }
                check_bound_root(context, receipt.generation, wanted).map_err(|reason| format!(
                    "outputs: {} selects an unverified root: {reason}. Run `oxide outputs adopt` or `oxide outputs detach`",
                    link.display()
                ))?;
                if ephemeral && (!profile_member_parent_exists(&wanted.root, &planned.member)?
                    || !profile_directory_exists(root)?)
                {
                    return Err(format!(
                        "outputs: {} selects {}, which is unavailable. Run `oxide outputs adopt` or `oxide outputs detach`",
                        link.display(), root.display()
                    ));
                }
                outcome.root.get_or_insert_with(|| wanted.root.clone());
            }
            (None, TargetState::RealEmpty | TargetState::RealNonEmpty) => {
                real.push(planned.member.clone());
            }
            (None, TargetState::Missing) if ephemeral => {
                real.push(planned.member.clone());
            }
            _ => {
                return Err(format!(
                    "outputs: {} is {}, not the keyed placement; the drive mode is ephemeral, so Oxide will not relink it. Run `oxide outputs adopt` or `oxide outputs detach`",
                    link.display(),
                    state.label()
                ))
            }
        }
    }
    if let Some(root) = outcome.root.clone() {
        outcome.note = Some(format!(
            "owned outputs {} through profile member links",
            root.display()
        ));
    }
    if !real.is_empty() {
        let list = real.join(", ");
        outcome.warnings.push(format!(
            "profile outputs: {list} use local directories; only an explicit `oxide outputs adopt` converts an empty one"
        ));
        if outcome.note.is_none() {
            outcome.note = Some(format!("local profile directories ({list})"));
        }
    }
    Ok(outcome)
}

pub(in crate::outputs) fn is_local_member(state: &TargetState) -> bool {
    matches!(
        state,
        TargetState::Missing | TargetState::RealEmpty | TargetState::RealNonEmpty
    )
}

/// Check ancestors before inspecting a member. A missing ancestor means the
/// member is absent. A link or file there cannot make a safe local placement.
pub(in crate::outputs) fn ephemeral_member_state(
    checkout: &Path,
    checkout_id: &str,
    member: &str,
) -> Result<TargetState, String> {
    let target = checkout.join("target");
    if !profile_member_parent_exists(&target, member)? {
        return Ok(TargetState::Missing);
    }
    link_state(&target.join(member), checkout, checkout_id)
}

fn profile_member_parent_exists(base: &Path, member: &str) -> Result<bool, String> {
    if !profile_directory_exists(base)? {
        return Ok(false);
    }
    match member.split_once('/') {
        Some((triple, _)) => profile_directory_exists(&base.join(triple)),
        None => Ok(true),
    }
}

fn profile_directory_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(format!(
            "outputs: {} is not a real profile directory; Oxide never follows or replaces it. Run `oxide outputs adopt` or `oxide outputs detach`",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("outputs: cannot inspect {}: {error}", path.display())),
    }
}

/// The result of one binding attempt under the mutex.
pub(in crate::outputs) enum Bind {
    /// The command is bound. The receipt is the one this decision was taken
    /// on (after any transition it committed), so the caller takes its holds
    /// from that same read instead of re-reading a peer-mutable document.
    Bound {
        outcome: Outcome,
        receipt: Receipt,
    },
    Refuse(OutputsError),
    /// Another command may be writing in this checkout; release the mutex,
    /// wait, and try again.
    Wait,
}

impl Bind {
    fn refused(reason: impl Into<String>) -> Self {
        Self::Refuse(OutputsError::refused(reason))
    }
}

pub(in crate::outputs) fn bind_under_binding(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: &Receipt,
    expected: Option<&Plan>,
    repair_absent_members: bool,
) -> Bind {
    if receipt.layout() == LayoutKind::Profile {
        return bind_profile(
            context,
            store,
            own_record,
            receipt,
            expected,
            repair_absent_members,
        );
    }
    let expected = expected.and_then(Plan::selected);
    let generation = receipt.generation;
    // A receipt that selects nothing owns no destination in either mode: the
    // command runs locally through the real directory the detach intends.
    if receipt.selects_nothing() {
        if expected.is_some() {
            return Bind::refused(
                "outputs: the receipt says this checkout selects nothing but its slot was keyed for an owned root; run the command again",
            );
        }
        return bind_real_directory(context, receipt);
    }
    let wanted = match receipt.state {
        ReceiptState::Prepared => {
            return Bind::refused(
                "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building",
            )
        }
        ReceiptState::Committed => match expected {
            Some(expected) => expected.clone(),
            None => match desired_selection(context, receipt) {
                Ok(Some(wanted)) => wanted,
                Ok(None) => return Bind::refused("outputs: the receipt selects no root"),
                Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
            },
        },
        // `selects_nothing` answered both of these above.
        ReceiptState::Detached | ReceiptState::Pruned => {
            return bind_real_directory(context, receipt)
        }
    };
    let usable = match external_choice(context) {
        Ok(usable) => usable,
        Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
    };
    if wanted.kind == RootKind::External {
        if let Err(reason) = &usable {
            return Bind::refused(format!(
                "outputs: this command's slot is keyed for the external root {} but the registered volume is no longer usable ({reason}); nothing was changed. Run the command again to select the checkout-local root",
                wanted.root.display()
            ));
        }
    }
    let target = match context.target() {
        Ok(target) => target,
        Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
    };
    // The live link must agree with the committed selection: a link to
    // another owned root (an older selection restored by hand) is foreign,
    // even when that root's marker is valid.
    let committed_root = receipt.selected().map(|selected| selected.root.as_path());
    let link_selects_wanted = target.link_root() == Some(wanted.root.as_path());
    if link_selects_wanted && committed_root != Some(wanted.root.as_path()) {
        return Bind::refused(format!(
            "outputs: {} selects {}, but the committed receipt selects {}; Oxide treats the link as foreign and changes nothing. Run `oxide outputs status` and restore the link",
            context.link().display(),
            wanted.root.display(),
            committed_root.map_or_else(|| "no root".to_owned(), |root| root.display().to_string())
        ));
    }
    match check_root(context, generation, &wanted) {
        RootCheck::Valid if link_selects_wanted => {
            return Bind::Bound {
                outcome: bound_outcome(&usable, &wanted, false),
                receipt: receipt.clone(),
            };
        }
        RootCheck::Valid => {}
        RootCheck::Missing => {
            // A future root binds only the exact surviving reservation its
            // slot was keyed with: a cancelled, replaced or absent one
            // refuses before Cargo, with no automatic retry.
            if let Err(reason) = reservation_matches(context, receipt, &wanted) {
                return Bind::refused(reason);
            }
        }
        RootCheck::Contradiction(reason) => {
            return Bind::Refuse(OutputsError::corrupt(format!(
                "outputs: {reason}; nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
            )))
        }
    }
    // A transition is needed: only this checkout's own link may be replaced.
    let old_link = match (&target, receipt.current()) {
        (TargetState::Missing, _) => true,
        (_, Some(selected)) => target.link_root() == Some(selected.root.as_path()),
        _ => false,
    };
    if !old_link {
        return Bind::refused(format!(
            "outputs: {} is {}, not this checkout's owned link; Oxide will not replace it. Run `oxide outputs detach` or restore the link yourself",
            context.link().display(),
            target.label()
        ));
    }
    match store.checkout_custody(&context.workspace, own_record) {
        Err(reason) => Bind::Refuse(OutputsError::io(reason)),
        Ok(CheckoutCustody::Unknown(reason)) => Bind::refused(format!(
            "outputs: refusing to relink {}: {reason}. The owned link and both roots are kept until that custody is resolved",
            context.link().display()
        )),
        Ok(CheckoutCustody::Writers) => Bind::Wait,
        Ok(CheckoutCustody::Clear) => {
            // The slot's own keyed nonce is the only future-root authority a
            // leased command may bind (oxide#152).
            let committed = match transition(
                context,
                receipt,
                &spec_for(&wanted),
                Start::Any,
                NonceAuthority::Keyed(wanted.nonce()),
            ) {
                Ok(committed) => committed,
                Err(reason) => return Bind::refused(reason),
            };
            let bound = match committed.selected() {
                Some(bound) => bound.clone(),
                None => return Bind::refused("outputs: the transition committed no root"),
            };
            if !bound.same_place(&wanted) {
                return Bind::refused(format!(
                    "outputs: the transition bound {} but this command's slot was keyed for {}; nothing is built into the wrong root. Run the command again",
                    bound.root.display(),
                    wanted.root.display()
                ));
            }
            // The root must carry exactly the generation this command's slot
            // was keyed with, whichever generation the transition committed.
            match check_root(context, committed.generation, &wanted) {
                RootCheck::Valid => Bind::Bound {
                    outcome: bound_outcome(&usable, &bound, true),
                    receipt: committed,
                },
                RootCheck::Missing => Bind::refused(format!(
                    "outputs: the transition bound {} but its root is missing; run the command again",
                    bound.root.display()
                )),
                RootCheck::Contradiction(reason) => Bind::refused(format!(
                    "outputs: the transition bound {} but {reason}; run the command again",
                    bound.root.display()
                )),
            }
        }
    }
}

/// The refusal for a link that a receipt selecting nothing still leaves in
/// place: a detach writes its receipt before it removes the link, so this is
/// the crash window between the two (or a pruned root's leftover link). The
/// link is never followed, and the detach is the verb that restores the real
/// directory.
fn leftover_link_refusal(link: &Path) -> String {
    format!(
        "outputs: {} is a link, but this checkout's receipt selects no root; it is never followed. Run `oxide outputs detach` to restore a real directory. Nothing was built",
        link.display()
    )
}

/// Bind a receipt that selects nothing (`Detached` or `Pruned`): the command
/// runs locally through real directories. The target a completed detach
/// restores is created here when it is absent (a detach crash window between
/// its durable receipt and the directory), and an owned link an interrupted
/// detach left behind is never followed: it names the verb that finishes the
/// detach.
fn bind_real_directory(context: &Context, receipt: &Receipt) -> Bind {
    match context.target() {
        Ok(TargetState::RealEmpty | TargetState::RealNonEmpty) => {}
        Ok(TargetState::Missing) => {
            if let Err(reason) = recreate_target_directory(&context.workspace) {
                return Bind::Refuse(OutputsError::io(reason));
            }
        }
        Ok(TargetState::OwnedLink(_) | TargetState::Dangling(_)) => {
            return Bind::refused(leftover_link_refusal(&context.link()))
        }
        Ok(state) => {
            return Bind::refused(format!(
                "outputs: {} is {}; a detached checkout keeps a real target directory and the link is never followed. Nothing was changed",
                context.link().display(),
                state.label()
            ))
        }
        Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
    }
    Bind::Bound {
        outcome: Outcome {
            warnings: vec![
                "permanent drive mode: this checkout is detached from owned outputs; `oxide outputs adopt` converts it again. Nothing was moved"
                    .to_owned(),
            ],
            note: Some("outputs detached".to_owned()),
            root: None,
        },
        receipt: receipt.clone(),
    }
}

/// Bind a profile-layout checkout's declared members under its binding
/// mutex. Every member either keeps the owned link its slot was keyed with, is
/// linked to the keyed root now (under positive custody closure), or is a real
/// profile directory this command keeps local. Anything else changed since the
/// key was built and refuses before Cargo.
pub(in crate::outputs) fn bind_profile(
    context: &Context,
    store: &Store,
    own_record: u64,
    receipt: &Receipt,
    expected: Option<&Plan>,
    repair_absent_members: bool,
) -> Bind {
    // A profile receipt that selects nothing owns no destination: the command
    // runs locally through real directories, exactly as the legacy detached
    // state does (oxide#160, S1). Nothing is planned, so nothing may be keyed
    // as owned, and a link below `target` is still never followed.
    if receipt.selects_nothing() {
        if expected.is_some_and(|plan| !plan.members.is_empty()) {
            return Bind::refused(
                "outputs: the receipt says this checkout selects nothing but its slot was keyed for an owned root; nothing was built. Run the command again",
            );
        }
        // A detach writes its receipt before it converts the member links, so
        // a crash can leave one behind; it is never treated as a real
        // directory, and the detach that produced it is the verb that finishes
        // it.
        for placement in receipt.links() {
            let link = link_path(&context.workspace, placement.member());
            match link_state(&link, &context.workspace, &context.checkout_id) {
                Ok(TargetState::OwnedLink(_) | TargetState::Dangling(_)) => {
                    return Bind::refused(leftover_link_refusal(&link))
                }
                Ok(_) => {}
                Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
            }
        }
        return bind_real_directory(context, receipt);
    }
    let Some(plan) = expected else {
        return Bind::refused(
            "outputs: this checkout places outputs at profile members but this command's slot was keyed with no owned destination; nothing was built. Run the command again",
        );
    };
    if plan.whole_target {
        return Bind::refused(
            "outputs: this checkout changed to profile members while this command waited, so its slot was keyed for the whole target; nothing was built. Run the command again",
        );
    }
    if receipt.state != ReceiptState::Committed {
        return Bind::refused(
            "outputs: this checkout's profile receipt selects no root (it is pending); run `oxide outputs adopt` or the command again",
        );
    }
    let mut needed: Vec<(String, Selected)> = Vec::new();
    let mut absent: Vec<(String, Selected)> = Vec::new();
    let mut real: Vec<String> = Vec::new();
    let mut owned_root: Option<PathBuf> = None;
    // oxide#165: the profile layout keeps `<checkout>/target` itself a real
    // directory, and its owned member links are published inside it. A
    // committed profile receipt whose target directory was deleted (a manual
    // `rm -rf target`, a task-local build-output cleanup) must never be a
    // checkout whose every command fails with ENOENT from the transition link:
    // the binding recreates that one real directory exclusively before the
    // first missing-member transition publishes into it. Anything else at that
    // path — a link of any kind, a file or another type — still refuses and is
    // never replaced or followed.
    let recreate_target = match context.target() {
        Ok(TargetState::RealEmpty | TargetState::RealNonEmpty) => false,
        Ok(TargetState::Missing) => true,
        Ok(state) => {
            return Bind::refused(format!(
                "outputs: this checkout places outputs at profile members inside a real target directory, but {} is {}; it is never replaced or followed. Nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                context.link().display(),
                state.label()
            ))
        }
        Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
    };
    for planned in &plan.members {
        let link = link_path(&context.workspace, &planned.member);
        let state = match link_state(&link, &context.workspace, &context.checkout_id) {
            Ok(state) => state,
            Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
        };
        // P4: this checkout's committed receipt still places the member on an
        // EXTERNAL root, but this command's fresh plan does not (a still-mounted
        // volume became unusable after the slot was keyed). Relinking,
        // recreating or linking an absent member under an internal root would be
        // an automatic external -> local transition behind the owner's back, so
        // it refuses before Cargo whatever the live link looks like and whether
        // or not the intermediate slot fell back. Only the explicit
        // `oxide outputs adopt` may change the recorded place.
        if let Some(committed) = receipt
            .links
            .iter()
            .find(|placement| placement.member == planned.member && placement.selected.is_some())
            .and_then(|placement| placement.selected.as_ref())
        {
            let planned_external = planned
                .selected
                .as_ref()
                .is_some_and(|wanted| wanted.kind == RootKind::External);
            if committed.kind == RootKind::External && !planned_external {
                let now = planned.selected.as_ref().map_or_else(
                    || "a real profile directory".to_owned(),
                    |wanted| {
                        member_destination(&wanted.root, &planned.member)
                            .display()
                            .to_string()
                    },
                );
                // Name the fallback reason too when this command's own
                // intermediate slot came from the internal pool, so the
                // refusal states both why the slot is internal and why the
                // member is not moved there.
                let fallback = context
                    .pool_fallback
                    .as_deref()
                    .map(|reason| {
                        format!(
                            "; this command's intermediate slot fell back to the internal pool ({reason})"
                        )
                    })
                    .unwrap_or_default();
                return Bind::refused(format!(
                    "outputs: {} keeps the external member root {} from this checkout's committed receipt, but this command would place it at {now}{fallback}; Oxide never moves an owned external output to another pool automatically, so nothing was built. Restore the drive, or run `oxide outputs adopt` to recheck this checkout's owned outputs",
                    link.display(),
                    member_destination(&committed.root, &planned.member).display()
                ));
            }
        }
        match (&planned.selected, &state) {
            (Some(wanted), TargetState::OwnedLink(root))
                if root == &member_destination(&wanted.root, &planned.member) =>
            {
                // The legacy #117 rule: an internal-pool fallback slot must
                // never write through an unsupervised external owned link.
                if wanted.kind == RootKind::External {
                    if let Some(reason) = context.pool_fallback.as_deref() {
                        return Bind::refused(format!(
                            "outputs: this command's intermediate slot fell back to the internal pool ({reason}) while {} selects the external member root {}; the drive's removal is not supervised, so nothing was built. Restore the drive or run `oxide outputs adopt`",
                            link_path(&context.workspace, &planned.member).display(),
                            wanted.root.display()
                        ));
                    }
                }
                if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                    return Bind::refused(format!(
                        "outputs: {reason}; nothing was changed. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                    ));
                }
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Missing) => {
                if let Err(reason) = member_root_authority(context, receipt, wanted) {
                    return Bind::refused(reason);
                }
                needed.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::OwnedLink(_)) => {
                needed.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Dangling(destination))
                if repair_absent_members
                    && destination == &member_destination(&wanted.root, &planned.member) =>
            {
                // A derived `oxide exec` only (oxide#162): the link selects
                // exactly this member's destination under the root this
                // command's slot was keyed for, and the only defect is that the
                // member directory inside that verified root is absent (the
                // shape an older migration left: the link was published before
                // its member directory existed). The directory is created under
                // the root's own verified identity and this checkout's writer
                // custody; nothing is relinked, replaced or followed. A
                // missing, unverified or foreign root fails `check_bound_root`
                // here and refuses below, and every other command keeps the
                // existing never-transition-automatically refusal.
                if wanted.kind == RootKind::External {
                    if let Some(reason) = context.pool_fallback.as_deref() {
                        return Bind::refused(format!(
                            "outputs: this command's intermediate slot fell back to the internal pool ({reason}) while {} selects the external member root {}; the drive's removal is not supervised, so nothing was built. Restore the drive or run `oxide outputs adopt`",
                            link.display(),
                            wanted.root.display()
                        ));
                    }
                }
                if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                    return Bind::refused(format!(
                        "outputs: {} links to {}, whose owned root {} is not verified ({reason}); an absent member directory is created only under this checkout's verified, committed root. Nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                        link.display(),
                        destination.display(),
                        wanted.root.display()
                    ));
                }
                absent.push((planned.member.clone(), wanted.clone()));
                owned_root.get_or_insert_with(|| wanted.root.clone());
            }
            (Some(wanted), TargetState::Dangling(_)) => {
                // Never transition automatically through a dangling link.
                // With an internal-pool fallback slot this is the #117 refusal;
                // otherwise the owner must run the explicit adopt.
                let fallback = context
                    .pool_fallback
                    .as_deref()
                    .map(|reason| format!("; this command's intermediate slot fell back to the internal pool ({reason})"))
                    .unwrap_or_default();
                return Bind::refused(format!(
                    "outputs: {} selects the member root {} which is not available now{fallback}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                    link.display(),
                    wanted.root.display()
                ));
            }
            (Some(_), _) => {
                return Bind::refused(format!(
                    "outputs: {} is now {}, but this command's slot was keyed for an owned member link; nothing was built. Run the command again",
                    link.display(),
                    state.label()
                ))
            }
            (None, TargetState::RealEmpty | TargetState::RealNonEmpty) => {
                real.push(planned.member.clone());
            }
            (None, _) => {
                return Bind::refused(format!(
                    "outputs: {} is now {}, but this command's slot was keyed for a real profile directory; nothing was built. Run the command again",
                    link.display(),
                    state.label()
                ))
            }
        }
    }
    if !absent.is_empty() {
        // Directory creation inside the owned root is a write, so it takes the
        // same writer custody as a relink: an unknown writer or a command that
        // may still be writing refuses or waits before the first creation.
        match store.checkout_custody(&context.workspace, own_record) {
            Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
            Ok(CheckoutCustody::Unknown(reason)) => {
                return Bind::refused(format!(
                    "outputs: refusing to create the absent profile member directory: {reason}. Nothing was changed"
                ))
            }
            Ok(CheckoutCustody::Writers) => return Bind::Wait,
            Ok(CheckoutCustody::Clear) => {}
        }
        for (member, wanted) in &absent {
            // Re-verify immediately before the write: the root's marker,
            // kind, volume, nonce, generation and device are all this
            // command's, not merely a directory this path names.
            if let Err(reason) = check_bound_root(context, receipt.generation, wanted) {
                return Bind::refused(format!(
                    "outputs: the absent profile member {member} is not created because {reason}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs"
                ));
            }
            if let Err(reason) = ensure_member_directory(&wanted.root, member) {
                return Bind::refused(format!(
                    "outputs: cannot create the absent profile member directory for {member}: {reason}; nothing was built"
                ));
            }
            // The link must still select exactly the directory just created:
            // a link repointed in between is never written through.
            let destination = member_destination(&wanted.root, member);
            match link_state(
                &link_path(&context.workspace, member),
                &context.workspace,
                &context.checkout_id,
            ) {
                Ok(TargetState::OwnedLink(live)) if live == destination => {}
                Ok(state) => {
                    return Bind::refused(format!(
                        "outputs: the absent profile member directory {} was created, but {} is now {}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                        destination.display(),
                        link_path(&context.workspace, member).display(),
                        state.label()
                    ))
                }
                Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
            }
        }
    }
    // The receipt every member transition advances; without a transition it
    // stays the one this command was keyed on.
    let mut current = receipt.clone();
    if !needed.is_empty() {
        match store.checkout_custody(&context.workspace, own_record) {
            Err(reason) => return Bind::Refuse(OutputsError::io(reason)),
            Ok(CheckoutCustody::Unknown(reason)) => {
                return Bind::refused(format!(
                    "outputs: refusing to link profile members: {reason}. Nothing was changed"
                ))
            }
            Ok(CheckoutCustody::Writers) => return Bind::Wait,
            Ok(CheckoutCustody::Clear) => {}
        }
        // The missing member links live inside the target directory this layout
        // keeps real, so a positively absent one is recreated before the first
        // transition publishes inside it (oxide#165). The create is exclusive
        // and never replaces or follows anything that appeared meanwhile.
        if recreate_target {
            if let Err(reason) = recreate_target_directory(&context.workspace) {
                return Bind::refused(format!(
                    "outputs: this checkout's profile members are placed inside the real target directory {}, which is absent and was not recreated: {reason}. Nothing was built",
                    context.link().display()
                ));
            }
        }
        for (member, wanted) in &needed {
            let spec = spec_for(wanted);
            // The member's keyed nonce is the only future-root authority this
            // command may bind; a cancelled, replaced or absent reservation
            // refuses inside the transition before any write (oxide#152).
            match transition_at(
                context,
                &current,
                member,
                &spec,
                Start::Any,
                NonceAuthority::Keyed(wanted.nonce()),
            ) {
                Ok(committed) => current = committed,
                Err(reason) => return Bind::refused(reason),
            }
            let bound = match current.selected_for(member) {
                Some(bound) => bound.clone(),
                None => {
                    return Bind::refused(format!(
                        "outputs: the transition for {member} committed no root; nothing was built"
                    ))
                }
            };
            if bound.root != wanted.root
                || bound.generation != wanted.generation
                || bound.nonce() != wanted.nonce()
            {
                return Bind::refused(format!(
                    "outputs: the transition bound {} for {member} but this command's slot was keyed for {}; nothing is built into the wrong root. Run the command again",
                    bound.root.display(),
                    wanted.root.display()
                ));
            }
            if let Err(reason) = check_bound_root(context, current.generation, &bound) {
                return Bind::refused(format!(
                    "outputs: the transition bound {} for {member} but {reason}; run the command again",
                    bound.root.display()
                ));
            }
        }
    }
    let mut outcome = Outcome::default();
    outcome.root = owned_root;
    if let Some(root) = outcome.root.clone() {
        let linked = plan
            .members
            .iter()
            .filter(|member| member.selected.is_some())
            .map(|member| member.member.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        outcome.note = Some(format!(
            "owned outputs {} through profile member links ({linked})",
            root.display()
        ));
    }
    if !real.is_empty() {
        let list = real.join(", ");
        outcome.warnings.push(format!(
            "profile outputs: {list} stay real directories in {} and are used locally; an empty one is converted only by `oxide outputs adopt`",
            context.link().display()
        ));
        if outcome.note.is_none() {
            outcome.note = Some(format!("real profile directories kept ({list})"));
        } else if let Some(note) = outcome.note.as_mut() {
            note.push_str(&format!("; real directories kept for {list}"));
        }
    }
    Bind::Bound {
        outcome,
        // The members a transition committed advanced the receipt; every
        // remaining member kept the one this command was keyed on.
        receipt: current,
    }
}

/// The authority a MISSING member link is created under. A member that joins a
/// root this checkout already owns is placed by the receipt itself: the commit
/// of that root's first member consumed the reservation, so adding a later
/// member (a host `--target` triple after `debug`, say) must not demand one.
/// The keyed root must still be exactly a root the receipt currently selects
/// and pass the full marker/kind/volume/generation/device check. Only a root
/// that does not exist yet binds the durable reservation its slot was keyed
/// with.
fn member_root_authority(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> Result<(), String> {
    let committed = receipt
        .links
        .iter()
        .filter_map(|placement| placement.selected.as_ref())
        .any(|selected| {
            selected.root == wanted.root
                && selected.kind == wanted.kind
                && selected.volume_uuid == wanted.volume_uuid
                && selected.nonce() == wanted.nonce()
        });
    if committed {
        return check_bound_root(context, receipt.generation, wanted).map_err(|reason| {
            format!(
                "outputs: this command's new profile member joins the committed root {} but {reason}; nothing was built. Run `oxide outputs adopt` to recheck this checkout's owned outputs",
                wanted.root.display()
            )
        });
    }
    reservation_matches(context, receipt, wanted)
}

/// The keyed future root's reservation must survive exactly: the receipt's
/// reservation is for this root and generation with the keyed nonce, and the
/// registry row still backs it with no cancellation recorded.
pub(in crate::outputs) fn reservation_matches(
    context: &Context,
    receipt: &Receipt,
    wanted: &Selected,
) -> Result<(), String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    crate::prune::require_registry(control)?;
    let stale = || {
        format!(
            "outputs: this command's slot was keyed for root nonce {} of {}, but that reservation was cancelled or replaced; nothing was built. Run the command again",
            wanted.nonce().unwrap_or("(none)"),
            wanted.root.display()
        )
    };
    let Some(nonce) = wanted.nonce() else {
        return Err(stale());
    };
    let Some(reserved) = receipt
        .reserved()
        .filter(|reserved| reserved.is_for(wanted) && reserved.nonce == nonce)
    else {
        return Err(stale());
    };
    crate::prune::validate_reservation(control, reserved)
        .map_err(|reason| format!("{} ({reason})", stale()))
}

/// Whether a root can be used as the keyed destination right now.
#[derive(Debug)]
pub(in crate::outputs) enum RootCheck {
    /// A real, fully owned root with the expected generation and device.
    Valid,
    /// The root does not exist yet, or is an empty directory with no marker; a
    /// transition may create it.
    Missing,
    /// Ownership, kind, volume, generation or device contradicts the receipt.
    Contradiction(String),
}

/// The complete check a reused-root command performs: marker identity, kind,
/// volume, generation and current device/ancestry. Any contradiction refuses
/// without mutation; only a wholly absent or empty unmarked root may be
/// created by a transition.
pub(in crate::outputs) fn check_root(
    context: &Context,
    receipt_generation: u64,
    wanted: &Selected,
) -> RootCheck {
    let root = &wanted.root;
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RootCheck::Missing,
        Err(error) => {
            return RootCheck::Contradiction(format!("cannot inspect {}: {error}", root.display()))
        }
        Ok(metadata) if !is_real_dir(&metadata) => {
            return RootCheck::Contradiction(format!("{} is not a real directory", root.display()))
        }
        Ok(_) => {}
    }
    if fs::symlink_metadata(root.join(MARKER)).is_err() {
        let empty = fs::read_dir(root)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        return if empty {
            RootCheck::Missing
        } else {
            RootCheck::Contradiction(format!(
                "{} holds content and carries no Oxide owner marker",
                root.display()
            ))
        };
    }
    match check_bound_root(context, receipt_generation, wanted) {
        Ok(()) => RootCheck::Valid,
        Err(reason) => RootCheck::Contradiction(reason),
    }
}

/// Validate a selected real root in full: this checkout's marker, the root
/// kind and volume, the generation it was created at, its device and that it
/// is not a mount root. Refuses any contradiction.
pub(in crate::outputs) fn check_bound_root(
    context: &Context,
    receipt_generation: u64,
    wanted: &Selected,
) -> Result<(), String> {
    let root = &wanted.root;
    let binding = binding_for_selected(context, wanted)?;
    validate_selected(
        &context.workspace,
        &context.checkout_id,
        wanted,
        binding.as_ref(),
        receipt_generation,
    )?;
    let marker = read_marker(root)?;
    if wanted.nonce.is_some() && marker.nonce != wanted.nonce {
        return Err(format!(
            "{} carries another root nonce than the keyed one; run the command again",
            root.display()
        ));
    }
    if marker.generation != wanted.generation {
        return Err(format!(
            "{} was created at generation {}, not the keyed {}; run the command again",
            root.display(),
            marker.generation,
            wanted.generation
        ));
    }
    Ok(())
}

fn bound_outcome(
    usable: &Result<removable::Registration, String>,
    bound: &Selected,
    relinked: bool,
) -> Outcome {
    let mut outcome = Outcome::default();
    // One notice per leased command on a legacy whole-target checkout: switching
    // to profile member links is explicit via `detach` then `adopt`.
    outcome.warnings.push(
        "outputs: this checkout uses the legacy whole-target layout; run `oxide outputs detach` then `oxide outputs adopt` to switch to profile member links"
            .to_owned(),
    );
    if let Err(reason) = usable {
        if bound.kind == RootKind::Local {
            outcome.warnings.push(format!(
                "permanent drive mode: outputs use the checkout-local fallback root {} ({reason}){}",
                bound.root.display(),
                if relinked { "; the external root is kept" } else { "" }
            ));
        }
    }
    outcome.root = Some(bound.root.clone());
    outcome.with_note(format!(
        "{} outputs {}{}",
        if bound.kind == RootKind::Local {
            "fallback"
        } else {
            "owned"
        },
        bound.root.display(),
        if relinked { " (relinked)" } else { "" }
    ))
}

/// Reserve the nonce of this command's future root before its slot is keyed
/// (oxide#129). Runs after automatic adoption and before
/// `planned_destination`/`SlotKey::resolve_in`, under the checkout's binding
/// lock (held only here, never across the lease wait) and, through
/// `write_receipt`, the internal publication lock. An existing root, an
/// identical uncancelled reservation, a pool without a ready registry (whose
/// transition refuses later and names the upgrade) or a checkout with no
/// committed receipt reserves nothing.
pub fn reserve(
    store: &Store,
    control: Option<&Path>,
    checkout: &Path,
    declaration: &crate::layout::Declaration,
    pool_fallback: Option<&str>,
) -> Result<(), String> {
    let Some(control) = control else {
        return Ok(());
    };
    let context = Context::new(Some(control.to_path_buf()), checkout.to_path_buf())
        .with_pool_fallback(pool_fallback);
    if context.receipt()?.is_none() || crate::prune::require_registry(control).is_err() {
        return Ok(());
    }
    let _binding = store.lock_binding(checkout)?;
    let Some(mut receipt) = context.receipt()? else {
        return Ok(());
    };
    // Finish any exact cancellation a crash interrupted after its
    // receipt committed.
    crate::prune::finish_dropped_cancels(
        control,
        &context.checkout_id,
        receipt.reserved().map(|reserved| reserved.id.as_str()),
    )?;
    if receipt.state != ReceiptState::Committed
        || (receipt.layout() == LayoutKind::Profile
            && context.mode()? == Some(DriveMode::Ephemeral))
    {
        return Ok(());
    }
    let Some(wanted) = desired_selection(&context, &receipt)? else {
        return Ok(());
    };
    // oxide#133 case14: a command that binding will refuse because of a
    // committed external member must not reserve (or cancel) anything
    // first. Reserving a root here used to rewrite the receipt — a real
    // authority effect on a command that builds nothing. The condition is
    // binding's own P4 precondition, per requested member and profile
    // layout only, so every command binding accepts still reserves.
    if refuses_committed_external(&receipt, &wanted, declaration) {
        return Ok(());
    }
    // oxide#152: a requested member whose owned link dangles is a
    // destination binding refuses, so this command publishes no
    // reservation at all. Its receipt and registry stay byte-identical and
    // the explicit `oxide outputs adopt` the refusal names can recover.
    if refuses_dangling_member(&context, &receipt, declaration)? {
        return Ok(());
    }
    if read_marker(&wanted.root).is_ok() {
        return Ok(());
    }
    if let Some(reserved) = receipt.reserved().cloned() {
        if reserved.is_for(&wanted)
            && crate::prune::validate_reservation(control, &reserved).is_ok()
        {
            return Ok(());
        }
        // A different or invalidated reservation is cancelled through
        // its journal before a new one is made.
        cancel_reservation(store, control, &context, &mut receipt)?;
    }
    let id = crate::prune::fresh_nonce()?;
    let nonce = crate::prune::fresh_nonce()?;
    *receipt.reserved_mut() = Some(Reservation {
        id,
        nonce,
        kind: wanted.kind,
        root: wanted.root.clone(),
        volume_uuid: wanted.volume_uuid.clone(),
        generation: wanted.generation,
        created_ms: process::now_ms(),
    });
    receipt.updated_ms = process::now_ms();
    write_receipt(control, &receipt)
}

/// Cancel this checkout's unused reservation (oxide#129). Only under the
/// binding lock with every writer positively clear and the reserved root absent
/// (no marker, no entry). The durable cancellation record invalidates its
/// authority first, then the receipt commits without it, then the abandoned
/// registry row is removed. A stale starter holding the old nonce refuses at
/// binding; the nonce is never reused.
pub(in crate::outputs) fn cancel_reservation(
    store: &Store,
    control: &Path,
    context: &Context,
    receipt: &mut Receipt,
) -> Result<(), String> {
    let Some(reserved) = receipt.reserved().cloned() else {
        return Ok(());
    };
    match fs::symlink_metadata(&reserved.root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => {
            return Err(format!(
                "outputs: the reserved root {} exists or cannot be inspected, so its reservation is kept; nothing was built",
                reserved.root.display()
            ))
        }
    }
    match store.checkout_custody(&context.workspace, 0)? {
        CheckoutCustody::Clear => {}
        _ => {
            return Err("outputs: this checkout's root reservation changed while another command may hold it; nothing was built. Run the command again once other commands in this checkout finish".to_owned())
        }
    }
    crate::prune::begin_cancel(control, &reserved)?;
    *receipt.reserved_mut() = None;
    receipt.updated_ms = process::now_ms();
    write_receipt(control, receipt)?;
    crate::prune::finish_cancel(control, &reserved)
}

/// `oxide outputs cancel-reservation`: the explicit cancellation of this
/// checkout's unused reservation, under its clean barrier and binding lock.
pub(in crate::outputs) fn cancel_command() -> Result<u8, String> {
    let context = Context::open()?;
    cancel_at(&context, output_store)
}

/// `oxide outputs cancel-reservation` against one already-opened context and a
/// **lazy** pool provider, so the verb decides before the pool is opened.
pub(in crate::outputs) fn cancel_at(
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
    let outcome = (|| -> Result<Option<Reservation>, String> {
        let _binding = internal.lock_binding(&context.workspace)?;
        let mut receipt = read_receipt(&control, &context.workspace, &context.checkout_id)?
            .ok_or("this checkout has no owned-output receipt; nothing to cancel")?;
        let Some(reserved) = receipt.reserved().cloned() else {
            return Ok(None);
        };
        cancel_reservation(&internal, &control, &context, &mut receipt)?;
        Ok(Some(reserved))
    })();
    release_barrier(&internal, guard)?;
    match outcome? {
        Some(reserved) => println!(
            "outputs: cancelled the unused reservation {} for {}; its nonce is never reused",
            reserved.id,
            reserved.root.display()
        ),
        None => println!("outputs: this checkout holds no root reservation"),
    }
    Ok(0)
}
