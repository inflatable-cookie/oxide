//! Owned-output placement validation for `oxide clean`.

use super::bind::refuse_unowned_link;
use super::receipt::{
    link_path, member_destination, LayoutKind, Receipt, ReceiptState, RootKind, Selected,
};
use super::root::{binding_for, is_real_dir_path, link_state, validate_selected};
use super::{Context, TargetState};
use crate::removable::DriveMode;
use crate::store::{self, Store};
use std::path::{Path, PathBuf};

/// Where `oxide clean` enumerates. Permanent owned outputs resolve through
/// the receipt to the selected real root; a clean never follows a link by
/// path text and never removes the link or the root's ownership marker.
pub struct CleanPlacement {
    target: PathBuf,
    keep_root: bool,
    link: PathBuf,
    inactive: Vec<PathBuf>,
    /// The bound profile layout: every owned member link must survive a
    /// whole clean, and its real root's payload is what gets cleaned.
    members: Vec<(PathBuf, PathBuf)>,
}

impl CleanPlacement {
    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn keep_root(&self) -> bool {
        self.keep_root
    }

    /// Owned member links and their real roots, for the profile layout.
    pub fn members(&self) -> &[(PathBuf, PathBuf)] {
        &self.members
    }

    /// Every path a whole clean must keep, or nothing when this is not a
    /// profile placement. In the profile layout `target` is a REAL directory,
    /// so it is kept as well as each owned member link and each real ancestor
    /// directory between the target and a nested link (a triple directory).
    /// Keeping a directory never stops the walk inside it.
    pub fn preserved(&self) -> Vec<PathBuf> {
        if self.members.is_empty() {
            return Vec::new();
        }
        let mut paths = vec![self.target.clone()];
        for (link, _) in &self.members {
            if !paths.iter().any(|known| known == link) {
                paths.push(link.clone());
            }
            let mut ancestor = link.parent();
            while let Some(directory) = ancestor {
                if directory == self.target.as_path() || !directory.starts_with(&self.target) {
                    break;
                }
                if !paths.iter().any(|known| known == directory) {
                    paths.push(directory.to_path_buf());
                }
                ancestor = directory.parent();
            }
        }
        paths
    }

    /// What the clean keeps, for its report; empty for an ephemeral checkout.
    pub fn preserved_note(&self) -> Option<String> {
        if !self.members.is_empty() {
            return Some(format!(
                "{} profile member link(s) and their ownership marker are kept; each owned member root's payload is cleaned instead: {}",
                self.members.len(),
                self.members
                    .iter()
                    .map(|(link, root)| format!("{} -> {}", link.display(), root.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.keep_root {
            return None;
        }
        let inactive = if self.inactive.is_empty() {
            String::new()
        } else {
            format!(
                "; inactive roots kept: {}",
                self.inactive
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        Some(format!(
            "{} and its ownership marker are kept, the owned link {} is kept{inactive}",
            self.target.display(),
            self.link.display()
        ))
    }
}

/// Validate a selected owned root for `oxide clean` by reusing
/// [`validate_selected`] with clean-specific refusal wording.
fn validate_clean_selected(
    context: &Context,
    workspace: &Path,
    selected: &Selected,
    receipt_generation: u64,
) -> Result<(), String> {
    let binding = match selected.kind {
        RootKind::External => Some(
            binding_for(context, selected.volume_uuid.as_deref().unwrap_or_default()).map_err(
                |reason| {
                    format!(
                        "clean refused: the registered volume for {} is not usable ({reason})",
                        selected.root().display()
                    )
                },
            )?,
        ),
        RootKind::Local => None,
    };
    validate_selected(
        workspace,
        &context.checkout_id,
        selected,
        binding.as_ref(),
        receipt_generation,
    )
    .map(|_| ())
    .map_err(|reason| format!("clean refused: {reason}"))
}

/// Resolve this checkout's clean target. `None` keeps today's ephemeral
/// behaviour; an owned checkout resolves to its selected real root and
/// refuses when the link and receipt disagree.
pub fn clean_placement(workspace: &Path) -> Result<Option<CleanPlacement>, String> {
    let settings = store::settings()?;
    let control = Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
    let context = Context::new(control, workspace.to_path_buf());
    let receipt = context.receipt()?;
    let unconverted = receipt.as_ref().is_none_or(Receipt::selects_nothing);
    if unconverted && context.mode()? == Some(DriveMode::Permanent) {
        refuse_unowned_link(&context, "clean")?;
    }
    let Some(receipt) = receipt else {
        return Ok(None);
    };
    match receipt.state {
        ReceiptState::Detached | ReceiptState::Pruned => Ok(None),
        ReceiptState::Prepared => Err(
            "clean refused: an interrupted owned-output transition is pending; ensure a permanent drive registration, then run `oxide outputs adopt` to replay it before cleaning"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            // The profile layout is validated BEFORE the whole-target link
            // pre-gate: `target` is a real directory there, never the link the
            // legacy path requires.
            if receipt.layout() == LayoutKind::Profile {
                let target = context.target()?;
                if !matches!(target, TargetState::RealEmpty | TargetState::RealNonEmpty) {
                    return Err(format!(
                        "clean refused: {} is {}, not a real profile target directory",
                        context.link().display(),
                        target.label()
                    ));
                }
                let mut members = Vec::new();
                let mut validated: Vec<PathBuf> = Vec::new();
                for placement in receipt.links() {
                    let Some(selected) = placement.selected_ref() else {
                        continue;
                    };
                    let link = link_path(workspace, placement.member());
                    let destination = member_destination(selected.root(), placement.member());
                    match link_state(&link, workspace, &context.checkout_id)? {
                        TargetState::OwnedLink(root) if root == destination => {
                            if !validated.iter().any(|known| known == selected.root()) {
                                validate_clean_selected(
                                    &context,
                                    workspace,
                                    selected,
                                    receipt.generation,
                                )?;
                                validated.push(selected.root().to_path_buf());
                            }
                            if !members.iter().any(|(_, known): &(PathBuf, PathBuf)| known == &destination) {
                                members.push((link, destination));
                            }
                        }
                        TargetState::Missing => {}
                        state => {
                            return Err(format!(
                                "clean refused: {} is {}, not this checkout's owned member link; Oxide never cleans through a foreign link",
                                link.display(),
                                state.label()
                            ))
                        }
                    }
                }
                let inactive = receipt
                    .roots
                    .iter()
                    .filter(|root| {
                        !validated.iter().any(|known| known == &root.root) && is_real_dir_path(&root.root)
                    })
                    .map(|root| root.root.clone())
                    .collect();
                return Ok(Some(CleanPlacement {
                    target: context.link(),
                    keep_root: false,
                    link: context.link(),
                    inactive,
                    members,
                }));
            }
            let selected = receipt
                .selected()
                .cloned()
                .ok_or("clean refused: the owned-output receipt selects no root")?;
            let target = context.target()?;
            if target.root() != Some(selected.root.as_path()) {
                return Err(format!(
                    "clean refused: {} is {}, not the owned link selecting {}; Oxide never cleans through a link or a foreign target. Run `oxide outputs adopt` or restore the link",
                    context.link().display(),
                    target.label(),
                    selected.root.display()
                ));
            }
            // Validate marker identity, generation and live device before
            // exposing the root for cleaning.
            validate_clean_selected(&context, workspace, &selected, receipt.generation)?;
            let inactive = receipt
                .roots
                .iter()
                .filter(|root| root.root != selected.root && is_real_dir_path(&root.root))
                .map(|root| root.root.clone())
                .collect();
            Ok(Some(CleanPlacement {
                target: selected.root,
                keep_root: true,
                link: context.link(),
                inactive,
                members: Vec::new(),
            }))
        }
    }
}
