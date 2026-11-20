//! Read-only `oxide outputs plan` and `oxide outputs status` reporting.

use super::receipt::{
    link_path, member_destination, read_receipt_for_prune, receipt_state_label, LayoutKind,
    Receipt, ReceiptState, RootKind, Selected, SelectedView,
};
use super::root::{
    external_choice_read_only, external_spec, is_real_dir_path, link_state, local_spec,
};
use super::transition::destination_generation;
use super::{mode_label, Context, TargetState, MEMBER};
use crate::removable::{self, DriveMode};
use crate::store;
use serde::Serialize;
use std::path::{Path, PathBuf};

const PLAN_VERSION: &str = "oxide.outputs.plan/2";
const MAX_PLAN_BYTES: usize = 64 * 1024;
/// Cooperative holders named in a plan.
const MAX_HOLDERS: usize = 16;

pub(in crate::outputs) fn plan_command() -> Result<u8, String> {
    let context = Context::open()?;
    let report = plan_report(&context)?;
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("cannot encode the output plan: {error}"))?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(format!(
            "the output plan exceeds its {MAX_PLAN_BYTES}-byte limit; nothing was changed"
        ));
    }
    println!("{}", String::from_utf8_lossy(&bytes));
    Ok(0)
}

/// The effect-free conversion plan: bounded JSON, no writes, no barrier.
#[derive(Serialize)]
pub(in crate::outputs) struct PlanReport {
    version: &'static str,
    checkout: PathBuf,
    pub(in crate::outputs) mode: &'static str,
    pub(in crate::outputs) state: &'static str,
    current_target: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) selected: Option<SelectedView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) proposed_root: Option<SelectedView>,
    pub(in crate::outputs) proposed_fallback_root: SelectedView,
    pub(in crate::outputs) action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) reason: Option<String>,
    /// Always false: a conversion never moves existing output.
    pub(in crate::outputs) data_move: bool,
    /// The placement this checkout publishes: `none`, `whole-target` (a
    /// legacy receipt; switch via `detach` + `adopt`) or `profile`.
    pub(in crate::outputs) layout: &'static str,
    /// Every member this checkout's receipt places, with its live state.
    /// Reading it never grants ownership of a real or foreign directory.
    pub(in crate::outputs) members: Vec<MemberPlanView>,
    pub(in crate::outputs) cooperative_holders: Vec<String>,
    pub(in crate::outputs) next_generation: u64,
}

#[derive(Serialize)]
pub(in crate::outputs) struct MemberPlanView {
    pub(in crate::outputs) member: String,
    /// `owned-link`, `real-empty`, `real-nonempty`, `missing` or `foreign`.
    pub(in crate::outputs) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) root: Option<PathBuf>,
    /// First-adoption prediction: keep, convert, or refuse this member.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::outputs) adoption: Option<&'static str>,
}

pub(in crate::outputs) fn plan_report(context: &Context) -> Result<PlanReport, String> {
    let mode = context.mode()?;
    let receipt = match context.control() {
        None => None,
        Some(control) => read_receipt_for_prune(control, &context.workspace, &context.checkout_id)?,
    };
    let target = context.target()?;
    let state = receipt
        .as_ref()
        .map_or("none", |receipt| receipt_state_label(receipt.state));
    let next_generation = receipt
        .as_ref()
        .map_or(1, |receipt| receipt.generation.saturating_add(1));
    let holders = match context.control() {
        None => Vec::new(),
        Some(control) => {
            // Holders in either pool: a removable command's record lives in
            // the removable pool's control directory.
            let mut holders = store::active_holders(control, &context.workspace)?;
            holders.extend(store::active_holders(
                &removable::removable_control(control),
                &context.workspace,
            )?);
            holders.truncate(MAX_HOLDERS);
            holders
        }
    };
    let usable = external_choice_read_only(context)?;
    let next = next_generation;
    let external = usable
        .as_ref()
        .ok()
        .map(|registration| external_spec(registration, &context.workspace));
    let local = local_spec(&context.workspace);
    let proposed = external
        .as_ref()
        .map(|spec| spec.selected(destination_generation(spec.root(), next)));
    let fallback = local.selected(destination_generation(local.root(), next));
    let mut report = PlanReport {
        version: PLAN_VERSION,
        checkout: context.workspace.clone(),
        mode: mode_label(mode),
        state,
        current_target: target.code(),
        current_root: target.link_root().map(Path::to_path_buf),
        selected: receipt
            .as_ref()
            .and_then(Receipt::selected)
            .map(Selected::view),
        proposed_root: proposed.as_ref().map(Selected::view),
        proposed_fallback_root: fallback.view(),
        action: "refuse",
        reason: None,
        data_move: false,
        layout: match receipt.as_ref().map(Receipt::layout) {
            None => "none",
            Some(LayoutKind::WholeTarget) => "whole-target",
            Some(LayoutKind::Profile) => "profile",
        },
        members: match &receipt {
            None => Vec::new(),
            Some(receipt) => receipt
                .links()
                .iter()
                .map(|placement| {
                    let link = link_path(&context.workspace, placement.member());
                    let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                    Ok(MemberPlanView {
                        member: placement.member().to_owned(),
                        state: state.code(),
                        root: state.link_root().map(Path::to_path_buf),
                        adoption: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        },
        cooperative_holders: holders,
        next_generation,
    };
    if mode != Some(DriveMode::Permanent) {
        report.reason = Some(
            "the registered drive is not in permanent mode, so there is no owned-output policy to apply"
                .to_owned(),
        );
        return Ok(report);
    }
    let Some(receipt) = receipt else {
        match target {
            TargetState::Missing | TargetState::RealEmpty => {
                report.action = "create-owned-link";
                if let Err(reason) = &usable {
                    report.reason = Some(format!(
                        "the volume is not usable ({reason}); the link would select the checkout-local fallback root"
                    ));
                }
            }
            TargetState::RealNonEmpty => {
                report.layout = "profile";
                report.action = "create-owned-link";
                for member in ["debug", "release"] {
                    let link = link_path(&context.workspace, member);
                    let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                    let adoption = match &state {
                        TargetState::RealNonEmpty => "keep",
                        TargetState::Foreign => "refuse",
                        TargetState::Missing
                        | TargetState::RealEmpty
                        | TargetState::OwnedLink(_)
                        | TargetState::Dangling(_) => "convert",
                    };
                    if adoption == "refuse" && report.reason.is_none() {
                        report.action = "refuse";
                        report.reason = Some(format!(
                            "{} is {}; it is never followed or replaced; earlier members may already be converted",
                            link.display(), state.label()
                        ));
                    }
                    report.members.push(MemberPlanView {
                        member: member.to_owned(),
                        state: state.code(),
                        root: state.link_root().map(Path::to_path_buf),
                        adoption: Some(adoption),
                    });
                }
                if report.reason.is_none() {
                    if report
                        .members
                        .iter()
                        .all(|member| member.adoption == Some("keep"))
                    {
                        report.action = "refuse";
                        report.reason = Some("both profile members are nonempty real directories; adoption keeps them local, creates no owned link and commits no root".to_owned());
                    } else if let Err(reason) = &usable {
                        report.reason = Some(format!(
                            "the volume is not usable ({reason}); converted members would select the checkout-local fallback root"
                        ));
                    }
                }
            }
            TargetState::Foreign => {
                report.reason = Some(
                    "the existing target is a symlink or file Oxide does not own; it is never followed, adopted or replaced"
                        .to_owned(),
                );
            }
            TargetState::OwnedLink(_) | TargetState::Dangling(_) => {
                report.reason = Some(
                    "the target is a link but this checkout has no receipt; remove it yourself before adopting"
                        .to_owned(),
                );
            }
        }
        return Ok(report);
    };
    if receipt.state == ReceiptState::Prepared {
        report.reason = Some(format!(
            "an interrupted transition to {} is pending; run `oxide outputs adopt` to replay it (its exact generations are rechecked)",
            receipt.prepared_ref().map_or_else(
                || "a new root".to_owned(),
                |prepared| prepared.to.root.display().to_string()
            )
        ));
        return Ok(report);
    }
    if receipt.state == ReceiptState::Detached {
        report.reason = Some(
            "the checkout is detached from owned outputs; `oxide outputs adopt` converts its target again"
                .to_owned(),
        );
        return Ok(report);
    }
    let Some(selected) = receipt.selected().cloned() else {
        report.reason = Some(
            "the receipt selects no root; `oxide outputs adopt` starts a conversion".to_owned(),
        );
        return Ok(report);
    };
    if receipt.layout() == LayoutKind::Profile {
        if matches!(target, TargetState::RealEmpty | TargetState::RealNonEmpty) {
            let wanted = match &usable {
                Ok(registration) => {
                    external_spec(registration, &context.workspace).selected(next_generation)
                }
                Err(_) => local.selected(next_generation),
            };
            let mut members = vec!["debug".to_owned(), "release".to_owned()];
            for placement in receipt.links() {
                members.push(placement.member().to_owned());
            }
            members.sort();
            members.dedup();
            let mut needs_relink = false;
            for member in &members {
                let link = link_path(&context.workspace, member);
                let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                match state {
                    TargetState::Foreign => {
                        report.action = "refuse";
                        report.reason = Some(format!(
                            "{} is {}; it is never followed or replaced",
                            link.display(),
                            state.label()
                        ));
                        return Ok(report);
                    }
                    TargetState::RealNonEmpty => {}
                    TargetState::OwnedLink(root)
                        if root == member_destination(&wanted.root, member)
                            && receipt
                                .selected_for(member)
                                .is_some_and(|bound| bound.same_place(&wanted)) => {}
                    TargetState::RealEmpty
                    | TargetState::Missing
                    | TargetState::OwnedLink(_)
                    | TargetState::Dangling(_) => {
                        needs_relink = true;
                    }
                }
            }
            report.action = if needs_relink { "relink" } else { "noop" };
            if let Err(reason) = &usable {
                report.reason = Some(format!(
                    "the volume is not usable ({reason}); the next command would select {}",
                    local.root().display()
                ));
            }
            return Ok(report);
        }
        report.reason = Some(format!(
            "the receipt selects {} but the target is {}; `oxide outputs adopt` rechecks and relinks it",
            selected.root.display(),
            target.label()
        ));
        return Ok(report);
    }
    match (&target, target.link_root()) {
        (_, Some(root)) if root == selected.root.as_path() => {
            report.action = "refuse";
            report.reason = Some(
                "this checkout uses the legacy whole-target layout; run `oxide outputs detach` then `oxide outputs adopt` to switch to profile member links"
                    .to_owned(),
            );
            Ok(report)
        }
        _ => {
            report.reason = Some(format!(
                "the legacy whole-target receipt selects {} and the target is {}; run `oxide outputs detach` then `oxide outputs adopt` to switch to profile member links",
                selected.root.display(),
                target.label()
            ));
            Ok(report)
        }
    }
}

pub(in crate::outputs) fn status_command() -> Result<u8, String> {
    for line in status_lines_for_current()? {
        println!("{line}");
    }
    Ok(0)
}

fn status_lines_for_current() -> Result<Vec<String>, String> {
    let context = Context::open()?;
    output_status_lines(&context)
}

/// Read-only owned-output lines for `oxide status --measure`. The control
/// directory comes from the recorded pool the command already read, so this
/// opens no pool and never creates a pool, directory or receipt.
pub fn status_lines_for(control: Option<PathBuf>, workspace: &Path) -> Result<Vec<String>, String> {
    output_status_lines(&Context::new(control, workspace.to_path_buf()))
}

pub(in crate::outputs) fn output_status_lines(context: &Context) -> Result<Vec<String>, String> {
    let mode = context.mode()?;
    let target = context.target()?;
    let receipt = match context.control() {
        None => None,
        Some(control) => read_receipt_for_prune(control, &context.workspace, &context.checkout_id)?,
    };
    let clause = |selected: &Selected| match selected.kind {
        RootKind::External => format!(
            "{} (volume {})",
            selected.root.display(),
            selected.volume_uuid.as_deref().unwrap_or("unknown")
        ),
        RootKind::Local => format!("{} (checkout-local fallback)", selected.root.display()),
    };
    let mut lines = Vec::new();
    match &receipt {
        None => lines.push(format!(
            "outputs: mode {}, this checkout is not converted (target {}); `oxide outputs plan` shows the conversion plan",
            mode_label(mode),
            target.label()
        )),
        Some(receipt) => {
            let state = match receipt.state {
                ReceiptState::Committed => receipt
                    .selected()
                    .map_or_else(|| "no selected root".to_owned(), clause),
                ReceiptState::Pruned => "the recorded root was pruned".to_owned(),
                ReceiptState::Prepared => format!(
                    "an interrupted transition to {} is pending",
                    receipt
                        .prepared_ref()
                        .as_ref()
                        .map_or_else(|| "a new root".to_owned(), |prepared| clause(&prepared.to))
                ),
                ReceiptState::Detached => "detached; no root is selected".to_owned(),
            };
            lines.push(format!(
                "outputs: mode {}, generation {}, {state}, target {}",
                mode_label(mode),
                receipt.generation,
                target.label()
            ));
        }
    }
    let local = local_spec(&context.workspace);
    let selected_local = receipt
        .as_ref()
        .and_then(Receipt::selected)
        .is_some_and(|selected| selected.kind == RootKind::Local);
    lines.push(format!(
        "outputs: local fallback root {} ({}{})",
        local.root().display(),
        if is_real_dir_path(local.root()) {
            "present"
        } else {
            "absent"
        },
        if selected_local {
            ", selected"
        } else {
            ", not selected"
        }
    ));
    if let Some(receipt) = &receipt {
        for root in &receipt.roots {
            if receipt.selected() == Some(root) {
                continue;
            }
            lines.push(format!(
                "outputs: preserved {} root {} ({}, never deleted)",
                root.kind.label(),
                root.root.display(),
                if is_real_dir_path(&root.root) {
                    "present"
                } else {
                    "absent"
                }
            ));
        }
        if receipt.layout() == LayoutKind::Profile {
            lines.push(format!(
                "outputs: layout profile members; {} is a real directory",
                context.link().display()
            ));
            for placement in receipt.links() {
                let link = link_path(&context.workspace, placement.member());
                let state = link_state(&link, &context.workspace, &context.checkout_id)?;
                let text = match (&state, placement.selected_ref()) {
                    (TargetState::OwnedLink(root), Some(selected))
                        if root == &member_destination(&selected.root, placement.member()) =>
                    {
                        format!("owned link to {}", root.display())
                    }
                    (TargetState::RealEmpty, _) => "real empty directory, not owned".to_owned(),
                    (TargetState::RealNonEmpty, _) => "real directory, not owned".to_owned(),
                    (TargetState::Missing, _) => {
                        "missing; linked when a declaring command runs".to_owned()
                    }
                    (state, _) => format!("{} (not this receipt's link)", state.label()),
                };
                lines.push(format!("outputs: member {} {}", placement.member(), text));
            }
        } else if let Some(selected) = receipt.selected() {
            lines.push(
                "outputs: layout whole-target (legacy); run `oxide outputs detach` then `oxide outputs adopt` to switch to profile member links"
                    .to_owned(),
            );
            if receipt.matching_live_root(&target, MEMBER).is_none() {
                lines.push(format!(
                    "outputs: the target is {} rather than the owned link selecting {}; run `oxide outputs detach`",
                    target.label(),
                    selected.root.display()
                ));
            }
        }
    }
    Ok(lines)
}
