//! Write-ahead output transitions, commit, replay and destination selection.

use super::adopt::refuse_existing_target_at;
use super::bind::reservation_matches;
use super::git::refuse_link_git_exposure_at;
use super::receipt::{
    link_path, member_destination, read_marker, remember_root, sync_directory, write_receipt,
    Prepared, Receipt, ReceiptState, Selected,
};
use super::root::{
    binding_for_selected, ensure_member_ancestors, ensure_member_directory, ensure_root_with,
    external_choice, external_spec, link_state, local_spec, owned_root, refuse_mount_root,
    spec_for, validate_selected, RootSpec,
};
use super::{Context, TargetState, MEMBER};
use crate::process;
use crate::removable::DriveMode;
use oxide_storage::volume::Binding;
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::fs;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

/// The destination rule itself: in permanent mode, external when the volume
/// is usable, else the local fallback; in any other mode, the committed
/// selection, which is never relinked automatically. Each comes with the
/// generation its root has or will be created at. `None` for a detached or
/// absent receipt.
pub(in crate::outputs) fn desired_selection(
    context: &Context,
    receipt: &Receipt,
) -> Result<Option<Selected>, String> {
    if receipt.state == ReceiptState::Detached {
        return Ok(None);
    }
    let next = receipt.generation.saturating_add(1);
    let spec = if context.mode()? == Some(DriveMode::Permanent) {
        match external_choice(context)? {
            Ok(registration) => external_spec(&registration, &context.workspace),
            Err(_) => local_spec(&context.workspace),
        }
    } else {
        match receipt.selected() {
            Some(selected) => spec_for(selected),
            None => return Ok(None),
        }
    };
    let spec = receipt
        .roots
        .iter()
        .rev()
        .chain(
            receipt
                .links()
                .iter()
                .filter_map(|placement| placement.selected_ref()),
        )
        .find(|selected| {
            selected.kind == spec.selected(0).kind
                && selected.volume_uuid == spec.selected(0).volume_uuid
                && selected.root.parent() == spec.root().parent()
                && read_marker(&selected.root).is_ok()
        })
        .map(spec_for)
        .unwrap_or(spec);
    let marker = read_marker(spec.root()).ok();
    let generation = marker.as_ref().map_or(next, |marker| marker.generation);
    let wanted = spec.selected(generation);
    // An existing root keys its own marker nonce; a future root keys the
    // nonce reserved for exactly it, never a missing root's old nonce.
    let nonce = match marker {
        Some(marker) => marker.nonce,
        None => receipt
            .reserved()
            .filter(|reserved| reserved.is_for(&wanted))
            .map(|reserved| reserved.nonce.clone()),
    };
    Ok(Some(wanted.with_nonce(nonce)))
}

/// The generation a destination root has, or will be created at: its marker's
/// creation generation, else the generation the next transition would use.
pub(in crate::outputs) fn destination_generation(root: &Path, next: u64) -> u64 {
    read_marker(root).map_or(next, |marker| marker.generation)
}

/// What a transition may do with `<checkout>/target` when it starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::outputs) enum Start {
    /// A command's own relink: a missing destination link or the receipt's
    /// own link. Only the legacy whole-target layout may still replace an
    /// empty real directory here.
    Any,
    /// `oxide outputs adopt`: like [`Start::Any`], and additionally the only
    /// way a real empty profile member directory is converted.
    Adopt,
    /// The automatic first conversion of oxide#121: a positively absent target
    /// only. An empty directory, a link and anything else refuse.
    MissingOnly,
}

/// The nonce authority one transition runs under (oxide#152).
///
/// A leased command's slot is keyed with the destination nonce it committed to
/// today, and that keyed nonce is the only authority a transition may bind: a
/// cancelled, replaced or absent reservation refuses before the prepared
/// receipt, the root, the link or any registry row exists. Only an explicit
/// adoption (or the automatic first conversion) never keyed a slot, so only it
/// may draw a fresh nonce for a root it decides to create.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::outputs) enum NonceAuthority<'a> {
    /// A keyed command's destination nonce (its reservation nonce, or `None`
    /// when no reservation existed when the slot was keyed).
    Keyed(Option<&'a str>),
    /// No slot was keyed: an explicit adoption or an automatic conversion.
    Unkeyed,
}

/// Write the committed receipt for one transition (the legacy whole-target
/// link).
pub(in crate::outputs) fn transition(
    context: &Context,
    receipt: &Receipt,
    spec: &RootSpec,
    start: Start,
    authority: NonceAuthority<'_>,
) -> Result<Receipt, String> {
    transition_at(context, receipt, MEMBER, spec, start, authority)
}

/// Write the committed receipt for one member's transition: the legacy
/// `target` link, or one profile member link below the real `target`
/// directory.
pub(in crate::outputs) fn transition_at(
    context: &Context,
    receipt: &Receipt,
    member: &str,
    spec: &RootSpec,
    start: Start,
    authority: NonceAuthority<'_>,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    // Transition publication requires the ready registry. Every adopting verb
    // establishes it first, before its publication lock — automatic adoption in
    // its own barrier window and explicit `oxide outputs adopt` through
    // `adoption_publishes_a_transition` — while a leased command's transition or
    // replay refuses here and names that upgrade, because a command holding a
    // slot is itself an open writer (oxide#217).
    crate::prune::require_registry(control)?;
    let link = link_path(&context.workspace, member);
    let profile_member = member != MEMBER;
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    // Only the receipt's own owned link, a missing destination, or (for the
    // explicit conversion) an empty real directory may be replaced. A build
    // never converts a real profile directory: only `oxide outputs adopt`
    // does, through `Start::Adopt`.
    let target = link_state(&link, &context.workspace, &context.checkout_id)?;
    let committed = receipt.selected_for(member);
    let converts_empty =
        target == TargetState::RealEmpty && (!profile_member || start == Start::Adopt);
    // A first conversion publishes an owned link, so Git must ignore that
    // exact path first — before the prepared receipt, the root or either link
    // exists. Later generations keep their committed link's eligibility and
    // are not re-guarded.
    let first_link = committed.is_none() && (target == TargetState::Missing || converts_empty);
    if first_link {
        refuse_link_git_exposure_at(context, &link, profile_member.then_some(member))?;
    }
    // The publish below replaces nothing when the link was absent (or an empty
    // directory was just removed): a creator that appears in between is kept.
    let exclusive = matches!(target, TargetState::Missing) || converts_empty;
    match (&target, committed, start) {
        (TargetState::Missing, _, _) => {}
        (TargetState::RealEmpty, _, Start::Adopt) if profile_member => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty profile directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (TargetState::RealEmpty, _, Start::Any) if !profile_member => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty target directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (_, Some(selected), Start::Any | Start::Adopt)
            if target.link_root()
                == Some(member_destination(selected.root(), member).as_path()) => {}
        (_, _, Start::MissingOnly) => {
            return Err(format!(
                "outputs: auto-adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed",
                link.display(),
                target.label()
            ))
        }
        (TargetState::RealEmpty, _, Start::Any) if profile_member => {
            return Err(format!(
                "outputs: {} is an empty real directory; only an explicit `oxide outputs adopt` converts it. It is used locally for this command",
                link.display()
            ))
        }
        _ => return Err(refuse_existing_target_at(&target, &link)),
    }
    // A newly created root gets a fresh random nonce; an existing root keeps
    // the nonce in its marker. A missing root is never re-created with the
    // receipt's old nonce (oxide#129). A selection keys the root's OWN
    // immutable creation generation, which is the placement generation only
    // for the transition that creates the root; a member joining an existing
    // root must not claim a newer one.
    let existing_marker = read_marker(spec.root()).ok();
    let existing_nonce = existing_marker
        .as_ref()
        .and_then(|marker| marker.nonce.clone());
    let root_generation = existing_marker
        .as_ref()
        .map_or(generation, |marker| marker.generation);
    // A future root takes the nonce reserved for exactly it, after the
    // reservation is validated against the registry.
    // A slot that was KEYED for a future root binds only the exact nonce it
    // named: a cancelled, replaced or absent reservation, or a root that now
    // carries another nonce, refuses here — before the prepared receipt, the
    // root, the link or any registry write — so a command that refuses leaves
    // no authority effect (oxide#152). Only an explicit adoption without a
    // keyed slot draws a fresh one.
    let reserved = receipt
        .reserved()
        .filter(|reserved| reserved.is_for(&spec.selected(generation)))
        .cloned();
    let nonce = match existing_nonce {
        Some(nonce) => {
            if let NonceAuthority::Keyed(keyed) = authority {
                if keyed != Some(nonce.as_str()) {
                    return Err(format!(
                        "outputs: this command's slot was keyed for root nonce {} of {}, but the root now carries {nonce}; nothing was built. Run the command again",
                        keyed.unwrap_or("(none)"),
                        spec.root().display()
                    ));
                }
            }
            Some(nonce)
        }
        None if read_marker(spec.root()).is_ok() => None,
        None => match authority {
            NonceAuthority::Keyed(keyed) => {
                let wanted = spec
                    .selected(root_generation)
                    .with_nonce(keyed.map(str::to_owned));
                reservation_matches(context, receipt, &wanted)?;
                wanted.nonce().map(str::to_owned)
            }
            NonceAuthority::Unkeyed => match &reserved {
                Some(reserved) => {
                    crate::prune::validate_reservation(control, reserved)?;
                    Some(reserved.nonce.clone())
                }
                None => Some(crate::prune::fresh_nonce()?),
            },
        },
    };
    let to = spec.selected(root_generation).with_nonce(nonce);
    // Write-ahead: prepared first, then the destination root, then the
    // sibling link, then the rename, then the committed receipt.
    let mut writing = receipt.clone();
    writing.state = ReceiptState::Prepared;
    writing.place_member(member).prepared = Some(Prepared {
        generation,
        to: to.clone(),
        link: link.clone(),
    });
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    context.crash_point("prepared")?;

    ensure_root_with(context, spec, generation, to.nonce())?;
    ensure_member_directory(&to.root, member)?;
    ensure_member_ancestors(context, &link)?;
    let sibling = sibling_link(&link, generation);
    match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(&member_destination(&to.root, member), &sibling).map_err(|error| {
                format!(
                    "cannot create the transition link {}: {error}",
                    sibling.display()
                )
            })?;
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing = fs::read_link(&sibling)
                .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
            if existing != member_destination(&to.root, member) {
                return Err(format!(
                    "outputs: {} exists and selects {}, not {}; both are left as they are",
                    sibling.display(),
                    existing.display(),
                    to.root.display()
                ));
            }
        }
        Ok(_) => {
            return Err(format!(
                "outputs: {} is occupied by something Oxide does not own; it is not replaced",
                sibling.display()
            ))
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", sibling.display())),
    }
    let sibling_read = fs::read_link(&sibling)
        .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
    if sibling_read != member_destination(&to.root, member) {
        return Err(format!(
            "outputs: {} does not read back as the transition root",
            sibling.display()
        ));
    }
    sync_directory(link.parent().unwrap_or(&context.workspace))?;
    context.crash_point("linked")?;

    publish_link(&sibling, &link, exclusive).map_err(|error| {
        format!(
            "cannot publish the owned link {}: {error}; the previous link is unchanged{}",
            link.display(),
            if exclusive {
                "; nothing that appeared at the target was replaced"
            } else {
                ""
            }
        )
    })?;
    sync_directory(link.parent().unwrap_or(&context.workspace))?;
    context.crash_point("renamed")?;
    let prepared = writing
        .prepared_ref()
        .clone()
        .ok_or("the prepared receipt lost its transition")?;
    // Commit from the write-ahead receipt: it is the one that carries this
    // placement, which a first member link adds here.
    finish_commit(context, &writing, &prepared)
}

/// Commit a prepared transition whose new link is live.
pub(in crate::outputs) fn finish_commit(
    context: &Context,
    receipt: &Receipt,
    prepared: &Prepared,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    let link = prepared.link.clone();
    let member = receipt
        .placement_for_link(&link)
        .map(|placement| placement.member.clone())
        .ok_or("the prepared transition names no placement of this receipt")?;
    let expected = member_destination(&prepared.to.root, &member);
    let live =
        fs::read_link(&link).map_err(|error| format!("cannot read {}: {error}", link.display()))?;
    if live != expected {
        return Err(format!(
            "outputs: {} selects {}, not the prepared {}; both roots are kept",
            link.display(),
            live.display(),
            expected.display()
        ));
    }
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    let marker = ensure_root_with(
        context,
        &spec_for(&prepared.to),
        generation,
        prepared.to.nonce(),
    )?;
    ensure_member_directory(&prepared.to.root, &member)?;
    // The committed selection carries the root's own creation generation, not
    // the placement revision that committed it.
    let selected = prepared
        .to
        .at(marker.generation)
        .with_nonce(marker.nonce.clone());
    let mut committed = receipt.clone();
    committed.state = ReceiptState::Committed;
    committed.generation = prepared.generation;
    let placement = committed
        .placement_for_link_mut(&link)
        .ok_or("the prepared transition names no placement of this receipt")?;
    placement.selected = Some(selected.clone());
    placement.prepared = None;
    // The commit that creates the reserved root consumes its reservation.
    if committed.reserved().is_some_and(|reserved| {
        reserved.root == selected.root && Some(reserved.nonce.as_str()) == selected.nonce()
    }) {
        *committed.reserved_mut() = None;
    }
    remember_root(&mut committed, selected);
    committed.updated_ms = process::now_ms();
    write_receipt(control, &committed)?;
    Ok(committed)
}

pub(in crate::outputs) fn sibling_link(link: &Path, generation: u64) -> PathBuf {
    let name = link
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_owned());
    link.with_file_name(format!(".{name}.oxide-new-{generation}"))
}

/// Publish a staged link at `link`. An `exclusive` publish is for a link that
/// was positively absent when classified: it fails rather than replacing
/// anything that has appeared since, and has no replacing fallback.
pub(in crate::outputs) fn publish_link(
    sibling: &Path,
    link: &Path,
    exclusive: bool,
) -> std::io::Result<()> {
    if exclusive {
        rename_no_replace(sibling, link)
    } else {
        fs::rename(sibling, link)
    }
}

#[cfg(target_os = "macos")]
pub(in crate::outputs) fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let from = path_c_string(from)?;
    let to = path_c_string(to)?;
    // SAFETY: both pointers come from live NUL-terminated strings that outlive
    // the call, and RENAME_EXCL never replaces an existing entry.
    let status = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
pub(in crate::outputs) fn rename_no_replace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    // The removable backend is macOS-only, so no automatic conversion reaches
    // here elsewhere; an explicit refusal, never a replacing rename.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a no-replace rename is unavailable on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn path_c_string(path: &Path) -> std::io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
}

/// Replay a prepared receipt: the exact new/old generations only.
pub(in crate::outputs) fn replay(context: &Context, receipt: Receipt) -> Result<Receipt, String> {
    if receipt.state != ReceiptState::Prepared {
        return Ok(receipt);
    }
    let prepared = receipt
        .prepared_ref()
        .clone()
        .ok_or("the prepared receipt names no transition")?;
    // A first conversion publishes its link during replay too: the same
    // prerequisite is rechecked under the caller's binding mutex, with no
    // recovery bypass. A receipt that already committed this placement keeps
    // it.
    let member = receipt
        .placement_for_link(&prepared.link)
        .map(|placement| placement.member.clone())
        .ok_or_else(|| {
            stop_unknown(
                &prepared,
                "its link is not this checkout's target or a profile member of this receipt",
            )
        })?;
    let committed_here = receipt.selected_for(&member).is_some();
    if !committed_here {
        refuse_link_git_exposure_at(
            context,
            &prepared.link,
            (member != MEMBER).then_some(member.as_str()),
        )?;
    }
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    // Every replay write records into the registry: refuse before the first
    // effect when it is not ready.
    crate::prune::require_registry(control)?;
    // The prepared link is validated to be this checkout's own link path; the
    // rename destination comes from the receipt's placement, never from a
    // file's contents.
    let link = prepared.link.clone();
    if link_path(&context.workspace, &member) != link {
        return Err(stop_unknown(
            &prepared,
            "its link is not this checkout's link for that placement",
        ));
    }
    let target = link_state(&link, &context.workspace, &context.checkout_id)?;
    let expected = member_destination(&prepared.to.root, &member);
    let sibling = sibling_link(&link, prepared.generation);
    // The sibling is classified before any replay mutation: positively
    // absent, or exactly the prepared link. Anything else (another kind, a
    // link to another root, unreadable) is unknown transition state and
    // stops with the receipt, links and roots unchanged.
    let sibling_live = match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(metadata) if metadata.file_type().is_symlink() => match fs::read_link(&sibling) {
            Ok(read) if read == expected => true,
            Ok(read) => {
                return Err(stop_unknown(
                    &prepared,
                    &format!(
                        "{} selects {}, not the prepared root {}",
                        sibling.display(),
                        read.display(),
                        prepared.to.root.display()
                    ),
                ))
            }
            Err(error) => {
                return Err(stop_unknown(
                    &prepared,
                    &format!("cannot read {}: {error}", sibling.display()),
                ))
            }
        },
        Ok(_) => {
            return Err(stop_unknown(
                &prepared,
                &format!("{} exists and is not a link", sibling.display()),
            ))
        }
        Err(error) => {
            return Err(stop_unknown(
                &prepared,
                &format!("cannot inspect {}: {error}", sibling.display()),
            ))
        }
    };
    let owned_new = owned_root(&context.workspace, &context.checkout_id, &prepared.to.root).is_ok();
    let renamed = target.link_root() == Some(expected.as_path());
    // After the rename the sibling no longer exists: a live sibling beside an
    // already-renamed target is contradictory.
    if renamed && sibling_live {
        return Err(stop_unknown(
            &prepared,
            "the target already selects the prepared root but the prepared sibling still exists",
        ));
    }
    // A first conversion has no committed selection, so its only valid old
    // state is a missing target: a link that appeared after the crash (even a
    // dangling one) is not this receipt's and is never renamed over. A
    // prepared receipt keeps the selection it is replacing in `selected`,
    // which is why the stored field is read here rather than the accessor.
    let old_live = match (&target, receipt.selected_for(&member)) {
        (TargetState::Missing, None) => true,
        (TargetState::Missing, Some(_)) => true,
        (_, Some(selected))
            if target.link_root()
                == Some(member_destination(selected.root(), &member).as_path()) =>
        {
            true
        }
        _ => false,
    };
    // An owned new root is validated in full (marker kind, volume UUID,
    // generation, live registered device and ancestry) before anything is
    // committed or renamed: a contradiction keeps the old link, the sibling,
    // both roots and the prepared receipt exactly as they are.
    let _binding = if owned_new && (renamed || (sibling_live && old_live)) {
        Some(
            validate_prepared(context, &prepared)
                .map_err(|reason| stop_unknown(&prepared, &reason))?,
        )
    } else {
        None
    };
    // 1. The rename already happened: commit.
    if owned_new && renamed {
        return finish_commit(context, &receipt, &prepared);
    }
    // 2. The sibling link is live: retry the rename under the barrier.
    if owned_new && sibling_live && old_live {
        // A link that was absent when classified is published without
        // replacement: a creator that appeared since the crash is kept.
        let exclusive = matches!(target, TargetState::Missing);
        publish_link(&sibling, &link, exclusive).map_err(|error| {
            format!(
                "cannot replay the prepared link rename {}: {error}; both roots are kept",
                link.display()
            )
        })?;
        sync_directory(link.parent().unwrap_or(&context.workspace))?;
        let live = fs::read_link(&link)
            .map_err(|error| format!("cannot read {}: {error}", link.display()))?;
        if live != expected {
            return Err(stop_unknown(
                &prepared,
                "the replayed link does not read back as the prepared member destination",
            ));
        }
        return finish_commit(context, &receipt, &prepared);
    }
    // 3. Nothing was published: roll the preparation back, keeping the old
    //    selection and any root the attempt created. Only a positively absent
    //    sibling means nothing was published.
    if sibling_live {
        return Err(stop_unknown(
            &prepared,
            "the prepared sibling exists but neither its rename nor a rollback is safe",
        ));
    }
    if !old_live {
        return Err(stop_unknown(
            &prepared,
            "neither the prepared sibling nor the receipt's old target link is live",
        ));
    }
    let mut rolled = receipt.clone();
    rolled.state = if rolled.current().is_some() {
        ReceiptState::Committed
    } else {
        ReceiptState::Detached
    };
    if let Some(placement) = rolled.placement_for_link_mut(&link) {
        placement.prepared = None;
    }
    if prepared.to.root.is_dir() {
        remember_root(&mut rolled, prepared.to.clone());
    }
    rolled.updated_ms = process::now_ms();
    write_receipt(control, &rolled)?;
    Ok(rolled)
}

/// The complete check of a prepared destination root, held (with its volume
/// binding) across the replayed rename and commit.
pub(in crate::outputs) fn validate_prepared(
    context: &Context,
    prepared: &Prepared,
) -> Result<Option<Binding>, String> {
    let to = &prepared.to;
    let binding = binding_for_selected(context, to)?;
    validate_selected(
        &context.workspace,
        &context.checkout_id,
        to,
        binding.as_ref(),
        prepared.generation,
    )?;
    Ok(binding)
}

fn stop_unknown(prepared: &Prepared, reason: &str) -> String {
    format!(
        "outputs: the prepared transition to generation {} cannot be replayed: {reason}. Both roots and {} are kept exactly as they are; inspect `oxide outputs status` and the file system before acting",
        prepared.generation,
        prepared.link.display()
    )
}
