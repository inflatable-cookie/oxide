//! Output root discovery, validation, ownership markers and directory creation.

use super::git::git_would_track;
use super::receipt::{
    member_destination, read_marker, safe_absolute, sync_directory, write_private, Marker, Receipt,
    RootKind, Selected, MARKER_VERSION, MAX_MARKER_BYTES,
};
use super::transition::rename_no_replace;
use super::{
    Context, OutputsError, TargetState, EXTERNAL_DIR, GITIGNORE_LINE, LOCAL_DIR, LOCAL_ROOT,
    MARKER, MEMBER, RECIPE,
};
use crate::identity;
use crate::process;
use crate::removable::{self, DriveMode};
use oxide_storage::volume::{self, Binding};
use std::fs::{self, File, OpenOptions};
use std::os::fd::AsFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// A real directory and nothing else: never a symlink.
pub(in crate::outputs) fn is_real_dir(metadata: &fs::Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink()
}

/// Open one existing real directory through its own descriptor: the final
/// component is never followed, and a directory swapped for another between
/// the inspection and the open is refused.
fn open_real_directory(path: &Path) -> Result<File, OutputsError> {
    let named = fs::symlink_metadata(path)
        .map_err(|error| OutputsError::io(format!("cannot inspect {}: {error}", path.display())))?;
    if !is_real_dir(&named) {
        return Err(OutputsError::refused(format!(
            "{} is not a real directory; it is never followed",
            path.display()
        )));
    }
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| OutputsError::io(format!("cannot open {}: {error}", path.display())))?;
    let opened = directory
        .metadata()
        .map_err(|error| OutputsError::io(format!("cannot inspect {}: {error}", path.display())))?;
    if !is_real_dir(&opened) || opened.dev() != named.dev() || opened.ino() != named.ino() {
        return Err(OutputsError::corrupt(format!(
            "{} changed while it was being opened",
            path.display()
        )));
    }
    Ok(directory)
}

/// Create one directory component beneath `parent` and return its descriptor.
/// An existing entry is opened only while it really is a directory: a symlink,
/// a file or any other entry at that name refuses and is never followed.
fn create_directory_component(
    parent: &File,
    component: &str,
    shown: &Path,
) -> Result<File, OutputsError> {
    let name = std::ffi::CString::new(component)
        .map_err(|_| OutputsError::refused(format!("{component} is not a usable name")))?;
    // SAFETY: `parent` is a live descriptor and `name` is a live
    // NUL-terminated string that outlives the calls below.
    let created = unsafe {
        libc::mkdirat(
            std::os::unix::io::AsRawFd::as_raw_fd(parent),
            name.as_ptr(),
            0o755,
        )
    } == 0;
    if !created {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(OutputsError::io(format!(
                "cannot create the member directory {}: {error}",
                shown.display()
            )));
        }
    }
    // SAFETY: `parent` is a live descriptor and `name` outlives the call.
    let opened = unsafe {
        libc::openat(
            std::os::unix::io::AsRawFd::as_raw_fd(parent),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if opened < 0 {
        return Err(OutputsError::refused(format!(
            "the member entry {} is not a real directory ({}) and is never followed",
            shown.display(),
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: `opened` is a newly owned descriptor.
    let opened = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(opened) };
    if created
        && unsafe { libc::fchmod(std::os::unix::io::AsRawFd::as_raw_fd(&opened), 0o755) } != 0
    {
        return Err(OutputsError::io(format!(
            "cannot set permissions on {}: {}",
            shown.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(File::from(opened))
}

/// Ensure the member's real destination directory exists beneath `base` (an
/// output root, or a profile checkout's real `target`). `base` itself is
/// already validated as a real directory owned by this checkout. Every
/// component below it is created one at a time, relative to its parent's
/// descriptor, so a symlinked intermediate is never followed the way
/// `create_dir_all` would: a symlink, a file or any other entry at any
/// component refuses and is left untouched.
pub(in crate::outputs) fn ensure_member_directory(
    base: &Path,
    member: &str,
) -> Result<PathBuf, OutputsError> {
    let destination = member_destination(base, member);
    if member == MEMBER {
        return Ok(destination);
    }
    let mut directory = open_real_directory(base)?;
    for component in member.split('/') {
        directory = create_directory_component(&directory, component, &destination)?;
    }
    Ok(destination)
}

/// Create the real ancestor directories a nested profile member needs inside
/// the checkout's own `target` (`target/<triple>` for `<triple>/debug`). The
/// target root stays a real directory, so a member link at a second component
/// is published only after that ancestor exists as a real directory: a
/// symlink, a file or any other entry there is refused, never followed or
/// replaced, and nothing outside the checkout's target is created.
pub(in crate::outputs) fn ensure_member_ancestors(
    context: &Context,
    link: &Path,
) -> Result<(), String> {
    let target = context.link();
    // The legacy whole-target placement IS the target directory, and a
    // one-component profile member sits directly in it: neither needs an
    // ancestor, but every profile member is published inside the real target
    // directory, so that root itself is inspected first (oxide#165).
    if link == target.as_path() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(&target).map_err(|error| {
        format!(
            "cannot inspect {}: {error}; a profile member is placed only inside a real target directory",
            target.display()
        )
    })?;
    if !is_real_dir(&metadata) {
        return Err(format!(
            "{} is not a real directory; a profile member is placed only inside one",
            target.display()
        ));
    }
    let Some(parent) = link.parent() else {
        return Ok(());
    };
    if parent == target.as_path() {
        return Ok(());
    }
    let relative = parent.strip_prefix(&target).map_err(|_| {
        format!(
            "{} is not beneath the checkout's target directory {}",
            link.display(),
            target.display()
        )
    })?;
    let mut current = target.clone();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_real_dir(&metadata) => {}
            Ok(_) => {
                return Err(format!(
                    "{} is not a real directory; Oxide never follows or replaces it",
                    current.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {
                        fs::set_permissions(&current, fs::Permissions::from_mode(0o755)).map_err(
                            |error| {
                                format!(
                                    "cannot set permissions on the member directory {}: {error}",
                                    current.display()
                                )
                            },
                        )?;
                        sync_directory(&target)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&current).map_err(|error| {
                            format!("cannot inspect {}: {error}", current.display())
                        })?;
                        if !is_real_dir(&metadata) {
                            return Err(format!(
                                "{} is not a real directory; Oxide never follows or replaces it",
                                current.display()
                            ));
                        }
                    }
                    Err(error) => {
                        return Err(format!(
                            "cannot create the member directory {}: {error}",
                            current.display()
                        ))
                    }
                }
            }
            Err(error) => return Err(format!("cannot inspect {}: {error}", current.display())),
        }
    }
    Ok(())
}

/// Recreate a profile checkout's real `target` directory when it is
/// positively absent (oxide#165).
///
/// The profile layout keeps `<checkout>/target` itself a real directory and
/// publishes its owned member links inside it, so a checkout whose `target`
/// was deleted (a manual `rm -rf target`, a task-local build-output cleanup)
/// would otherwise fail every leased command with ENOENT while creating the
/// transition link, and never recreate the directory. The create is one
/// exclusive `mkdirat` beneath the checkout's own directory descriptor, so the
/// final component is never followed and an entry that appeared since the
/// caller's classification is never replaced; only a real directory counts as
/// that concurrent creator's result. The caller holds this checkout's binding
/// mutex and has cleared writer custody, so no cooperative command can be
/// racing this, and the missing-member transitions run immediately after it.
pub(in crate::outputs) fn recreate_target_directory(checkout: &Path) -> Result<(), String> {
    let target = checkout.join("target");
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(checkout)
        .map_err(|error| format!("cannot open {}: {error}", checkout.display()))?;
    let named = fs::symlink_metadata(checkout)
        .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
    let opened = directory
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
    // A checkout path swapped for another directory between the classification
    // and this open is refused: the new directory is never written into.
    if !opened.is_dir() || opened.dev() != named.dev() || opened.ino() != named.ino() {
        return Err(format!(
            "{} changed while it was being opened",
            checkout.display()
        ));
    }
    let name = std::ffi::CString::new("target").expect("the fixed name has no NUL");
    // SAFETY: `directory` is a live descriptor for the checkout directory and
    // `name` is a live NUL-terminated string that outlives the call.
    let status = unsafe {
        libc::mkdirat(
            std::os::unix::io::AsRawFd::as_raw_fd(&directory),
            name.as_ptr(),
            0o700,
        )
    };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("cannot create {}: {error}", target.display()));
        }
        // Something appeared between the classification and this exclusive
        // create. It is the same state the classification accepts when it is
        // already there — a real directory — or a refusal; a link, a file or
        // any other entry is never replaced or followed.
        return match fs::symlink_metadata(&target) {
            Ok(metadata) if is_real_dir(&metadata) => Ok(()),
            Ok(_) => Err(format!(
                "{} appeared while the missing target directory was being recreated and is not a real directory; it is never replaced",
                target.display()
            )),
            Err(error) => Err(format!("cannot inspect {}: {error}", target.display())),
        };
    }
    // Widen to the mode a conversion creates (oxide#121, oxide#133), through
    // the new directory's own descriptor so nothing else at that path is ever
    // chmod'ed.
    // SAFETY: `directory` is a live descriptor and the path is a fixed,
    // NUL-terminated single component.
    let created = unsafe {
        libc::openat(
            std::os::unix::io::AsRawFd::as_raw_fd(&directory),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if created < 0 {
        return Err(format!(
            "cannot open the recreated {}: {}",
            target.display(),
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `created` is a newly owned descriptor.
    let created = unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(created) };
    if unsafe { libc::fchmod(std::os::unix::io::AsRawFd::as_raw_fd(&created), 0o755) } != 0 {
        return Err(format!(
            "cannot set permissions on the recreated {}: {}",
            target.display(),
            std::io::Error::last_os_error()
        ));
    }
    drop(created);
    directory
        .sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", checkout.display()))?;
    Ok(())
}

/// How an output root is found and created.
#[derive(Clone, Debug)]
pub(in crate::outputs) enum RootSpec {
    External {
        root: PathBuf,
        uuid: String,
        /// One directory name under `<mount>/oxide-outputs`.
        leaf: String,
    },
    Local {
        root: PathBuf,
    },
}

impl RootSpec {
    pub(in crate::outputs) fn root(&self) -> &Path {
        match self {
            Self::External { root, .. } | Self::Local { root } => root,
        }
    }

    pub(in crate::outputs) fn selected(&self, generation: u64) -> Selected {
        match self {
            Self::External { root, uuid, .. } => Selected {
                kind: RootKind::External,
                root: root.clone(),
                volume_uuid: Some(uuid.clone()),
                nonce: None,
                generation,
            },
            Self::Local { root } => Selected {
                kind: RootKind::Local,
                root: root.clone(),
                volume_uuid: None,
                nonce: None,
                generation,
            },
        }
    }
}

/// Only positive absence selects checkout-local fallback. Identity failures,
/// a different registered volume and unwritable storage refuse the command.
/// Existing declarations route through Barge's location; a legacy read-only
/// plan observes its platform seam without importing state as a side effect.
pub(in crate::outputs) fn external_choice(
    context: &Context,
) -> Result<Result<removable::Registration, String>, String> {
    external_choice_with(context, false)
}

/// The read-only form `oxide outputs plan` uses: the recorded Barge account is
/// opened without preparing it, so a plan never creates a directory or takes
/// the account's lock on its way to an effect-free answer.
pub(in crate::outputs) fn external_choice_read_only(
    context: &Context,
) -> Result<Result<removable::Registration, String>, String> {
    external_choice_with(context, true)
}

fn external_choice_with(
    context: &Context,
    read_only: bool,
) -> Result<Result<removable::Registration, String>, String> {
    use barge::Os;
    let Some(control) = context.control() else {
        return Ok(Err("no removable drive is registered".to_owned()));
    };
    let Some(registration) = removable::read_registration(control)? else {
        return Ok(Err("no removable drive is registered".to_owned()));
    };
    if registration.mode != DriveMode::Permanent {
        return Ok(Err("the drive is not in permanent mode".to_owned()));
    }
    if let Some(reason) = context.pool_fallback.as_deref() {
        return Ok(Err(format!(
            "this command's intermediate slot fell back to the internal pool ({reason})"
        )));
    }
    let state = removable::removable_control(control);
    let present = if state.join("barge/areas").is_dir() {
        let storage = if read_only {
            oxide_storage::storage::Storage::open_read_only(&state)?
        } else {
            oxide_storage::storage::Storage::open(&state)?
        };
        let area = storage
            .areas()?
            .into_iter()
            .find(|area| area.key().label() == "oxide/pool")
            .ok_or("the registered Barge pool area is missing")?;
        if area.volume().spec().mount != registration.mount_point
            || !area
                .volume()
                .spec()
                .uuid
                .eq_ignore_ascii_case(&registration.volume_uuid)
        {
            return Err("the output registration differs from Barge's volume".to_owned());
        }
        external_location(&area)?
    } else {
        match oxide_storage::storage::OxideOs
            .volume_identity(&registration.mount_point)
            .map_err(|error| error.to_string())?
        {
            None => false,
            Some(uuid) if uuid.eq_ignore_ascii_case(&registration.volume_uuid) => true,
            Some(uuid) => {
                return Err(format!(
                    "output volume identity mismatch: expected {}, found {uuid}; refusing fallback",
                    registration.volume_uuid
                ))
            }
        }
    };
    if !present {
        return Ok(Err(format!(
            "registered volume {} is absent",
            registration.mount_point.display()
        )));
    }
    if !volume::writable(&registration.mount_point)? {
        return Err("the registered output volume is not writable".to_owned());
    }
    // Reserve is a Barge pruning trigger, never permission to change volumes.
    Ok(Ok(registration))
}

pub(in crate::outputs) fn external_location(area: &barge::Area) -> Result<bool, String> {
    match area.location().map_err(|error| error.to_string())? {
        barge::Location::Volume(_) => Ok(true),
        barge::Location::Fallback(_) => Ok(false),
        barge::Location::Unavailable(_) => {
            match area.volume().state().map_err(|error| error.to_string())? {
                barge::VolumeState::Absent => Ok(false),
                _ => Err("the registered output volume is unavailable".to_owned()),
            }
        }
    }
}

#[cfg(test)]
pub(in crate::outputs) fn usable_external(
    context: &Context,
) -> Result<removable::Registration, String> {
    external_choice(context)?
}

pub(in crate::outputs) fn external_spec(
    registration: &removable::Registration,
    checkout: &Path,
) -> RootSpec {
    let label = identity::project_label(checkout);
    let id = identity::digest(&checkout.to_string_lossy());
    let leaf = format!("{label}-{}", &id[..8]);
    RootSpec::External {
        root: registration.mount_point.join(EXTERNAL_DIR).join(&leaf),
        uuid: registration.volume_uuid.clone(),
        leaf,
    }
}

pub(in crate::outputs) fn current_or_successor_spec(
    context: &Context,
    receipt: &Receipt,
    spec: RootSpec,
) -> Result<RootSpec, String> {
    let wanted = spec.selected(0);
    for selected in receipt
        .roots
        .iter()
        .rev()
        .chain(
            receipt
                .links()
                .iter()
                .filter_map(|placement| placement.selected_ref()),
        )
        .filter(|selected| {
            selected.kind == wanted.kind
                && selected.volume_uuid == wanted.volume_uuid
                && selected.root.parent() == wanted.root.parent()
                && read_marker(&selected.root).is_ok()
        })
    {
        if let Some(control) = context.control() {
            if crate::prune::needs_successor(control, &selected.root)? {
                continue;
            }
        }
        return Ok(spec_for(selected));
    }
    let Some(control) = context.control() else {
        return Ok(spec);
    };
    if !crate::prune::needs_successor(control, spec.root())? {
        return Ok(spec);
    }
    let root = crate::prune::fresh_output(control, &wanted)?;
    let replacement = match spec {
        RootSpec::External { uuid, .. } => RootSpec::External {
            leaf: root.folder,
            root: root.path,
            uuid,
        },
        RootSpec::Local { .. } => RootSpec::Local { root: root.path },
    };
    let binding = match &replacement {
        RootSpec::External { uuid, .. } => Some(binding_for(context, uuid)?),
        RootSpec::Local { .. } => None,
    };
    create_marker(
        context,
        &replacement,
        receipt.generation.saturating_add(1),
        binding.as_ref(),
        None,
    )?;
    Ok(replacement)
}

pub(in crate::outputs) fn local_spec(checkout: &Path) -> RootSpec {
    RootSpec::Local {
        root: checkout.join(LOCAL_DIR).join(LOCAL_ROOT),
    }
}

/// Classify `<checkout>/target` without following a link away from ownership.
pub(in crate::outputs) fn target_state(
    checkout: &Path,
    checkout_id: &str,
) -> Result<TargetState, String> {
    link_state(&checkout.join("target"), checkout, checkout_id)
}

/// Classify one link path — the legacy `<checkout>/target` or one profile
/// member below it — without following a link away from ownership.
pub(in crate::outputs) fn link_state(
    link: &Path,
    checkout: &Path,
    checkout_id: &str,
) -> Result<TargetState, String> {
    let target = link.to_path_buf();
    let metadata = match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TargetState::Missing)
        }
        Err(error) => return Err(format!("cannot inspect {}: {error}", target.display())),
        Ok(metadata) => metadata,
    };
    if metadata.file_type().is_symlink() {
        let root = fs::read_link(&target)
            .map_err(|error| format!("cannot read the link {}: {error}", target.display()))?;
        if owned_root(checkout, checkout_id, &root).is_ok() {
            return Ok(TargetState::OwnedLink(root));
        }
        // A profile member link selects a distinct subpath beneath the owned
        // root, whose marker lives at the root itself. Ownership is decided by
        // that root marker, never by following an arbitrary ancestor, and the
        // destination's own shape (`<profile>` or `<triple>/<profile>`) is only
        // a candidate list: it grants nothing without the marker.
        for member_root in member_root_candidates(&root) {
            if owned_root(checkout, checkout_id, &member_root).is_ok() {
                // The root is ours; the member destination is only an owned
                // link while it actually exists.
                return Ok(if fs::symlink_metadata(&root).is_ok() {
                    TargetState::OwnedLink(root)
                } else {
                    TargetState::Dangling(root)
                });
            }
        }
        return Ok(if fs::symlink_metadata(&root).is_err() {
            TargetState::Dangling(root)
        } else {
            TargetState::Foreign
        });
    }
    if !metadata.is_dir() {
        return Ok(TargetState::Foreign);
    }
    let mut entries = fs::read_dir(&target)
        .map_err(|error| format!("cannot read {}: {error}", target.display()))?;
    Ok(if entries.next().is_none() {
        TargetState::RealEmpty
    } else {
        TargetState::RealNonEmpty
    })
}

/// The candidate owned roots one member destination can belong to: strip a
/// valid v1 member suffix and return every root that shape could name, most
/// specific first. A plain `<root>/<profile>` destination names its parent; a
/// `<root>/<triple>/<profile>` destination names its grandparent, whose child
/// directory is the triple. Both are only candidates — ownership is decided by
/// the marker at the root, and a wrong root simply fails that check.
fn member_root_candidates(target: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(name) = target.file_name().and_then(|name| name.to_str()) else {
        return roots;
    };
    if name != "debug" && name != "release" {
        return roots;
    }
    let Some(parent) = target.parent() else {
        return roots;
    };
    // <root>/<triple>/<profile>, then <root>/<profile>. The triple form is
    // tried first so a nested link is reported against the root that really
    // owns it when a checkout's root is itself named like a member component.
    if let (Some(triple), Some(grandparent)) = (
        parent.file_name().and_then(|triple| triple.to_str()),
        parent.parent(),
    ) {
        if crate::layout::Member::parse(&format!("{triple}/{name}")).is_ok() {
            roots.push(grandparent.to_path_buf());
        }
    }
    roots.push(parent.to_path_buf());
    roots
}

/// A root is owned by this checkout when it is a real directory carrying this
/// checkout's marker. Volume binding and generation are checked against the
/// receipt by [`validate_selected`].
pub(in crate::outputs) fn owned_root(
    checkout: &Path,
    checkout_id: &str,
    root: &Path,
) -> Result<Marker, String> {
    if !safe_absolute(root) {
        return Err(format!("{} is not an absolute path", root.display()));
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("cannot inspect {}: {error}", root.display()))?;
    if !is_real_dir(&metadata) {
        return Err(format!("{} is not a real directory", root.display()));
    }
    let marker = read_marker(root)?;
    check_marker(&marker, checkout, checkout_id)?;
    Ok(marker)
}

fn check_marker(marker: &Marker, checkout: &Path, checkout_id: &str) -> Result<(), String> {
    let _ = marker;
    marker.validate().map_err(|reason| {
        format!(
            "{} carries an unusable owner marker ({reason})",
            checkout.display()
        )
    })?;
    if marker.checkout != checkout || marker.checkout_id != checkout_id {
        return Err(format!(
            "{} belongs to {}, not this checkout",
            marker.checkout.display(),
            checkout.display()
        ));
    }
    if marker.recipe != RECIPE {
        return Err(format!(
            "{} carries owner recipe {:?}, not {RECIPE}",
            checkout.display(),
            marker.recipe
        ));
    }
    Ok(())
}

/// Validate a selected real root against the receipt's identity: marker,
/// kind, volume UUID, device and generation.
pub(in crate::outputs) fn validate_selected(
    checkout: &Path,
    checkout_id: &str,
    selected: &Selected,
    binding: Option<&Binding>,
    receipt_generation: u64,
) -> Result<Marker, String> {
    let marker = owned_root(checkout, checkout_id, &selected.root)?;
    if marker.kind != selected.kind || marker.volume_uuid != selected.volume_uuid {
        return Err(format!(
            "{} carries a marker for another root kind or volume",
            selected.root.display()
        ));
    }
    if marker.generation > receipt_generation {
        return Err(format!(
            "{} carries generation {} ahead of the receipt's {receipt_generation}",
            selected.root.display(),
            marker.generation
        ));
    }
    if let (Some(nonce), Some(recorded)) = (marker.nonce(), selected.nonce()) {
        if recorded != nonce {
            return Err(format!(
                "{} carries root nonce {nonce}, not the receipt's {:?}",
                selected.root.display(),
                selected.nonce()
            ));
        }
    }
    validate_root_device(checkout, selected, binding)?;
    Ok(marker)
}

/// A root must be a real directory on the intended device, and never a mount
/// root of its own.
pub(in crate::outputs) fn validate_root_device(
    checkout: &Path,
    selected: &Selected,
    binding: Option<&Binding>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(&selected.root)
        .map_err(|error| format!("cannot inspect {}: {error}", selected.root.display()))?;
    if !is_real_dir(&metadata) {
        return Err(format!(
            "{} is not a real directory",
            selected.root.display()
        ));
    }
    refuse_mount_root(&selected.root)?;
    match (selected.kind, binding) {
        (RootKind::Local, _) => {
            validate_local_ancestry(checkout, &selected.root)?;
            let checkout_metadata = fs::symlink_metadata(checkout)
                .map_err(|error| format!("cannot inspect {}: {error}", checkout.display()))?;
            if metadata.dev() != checkout_metadata.dev() {
                return Err(format!(
                    "{} is on another device than the checkout",
                    selected.root.display()
                ));
            }
        }
        (RootKind::External, Some(binding)) => {
            let facts = binding.verify()?;
            let observed = volume::observe_path(&selected.root)?;
            if observed.dev != facts.dev || observed.mounted_on != facts.mounted_on {
                return Err(format!(
                    "{} is not on the registered volume {}",
                    selected.root.display(),
                    binding.mount_point().display()
                ));
            }
            if observed.uuid.as_deref() != selected.volume_uuid.as_deref() {
                return Err(format!(
                    "{} is on volume {:?}, not the recorded {:?}",
                    selected.root.display(),
                    observed.uuid,
                    selected.volume_uuid
                ));
            }
            let canonical = fs::canonicalize(&selected.root)
                .map_err(|error| format!("cannot resolve {}: {error}", selected.root.display()))?;
            if canonical != selected.root {
                return Err(format!(
                    "{} does not resolve to itself",
                    selected.root.display()
                ));
            }
        }
        (RootKind::External, None) => {
            return Err(format!(
                "{} is an external root and the registered volume is not usable now",
                selected.root.display()
            ))
        }
    }
    Ok(())
}

/// The local root is exactly `<checkout>/.oxide-outputs/local` with a real
/// `.oxide-outputs` directory and no link anywhere between the checkout and
/// the leaf: a same-device symlinked ancestor would place outputs (and a
/// clean's deletions) outside the checkout.
fn validate_local_ancestry(checkout: &Path, root: &Path) -> Result<(), String> {
    let outer = checkout.join(LOCAL_DIR);
    if root.parent() != Some(outer.as_path()) {
        return Err(format!(
            "{} is not this checkout's local root {}",
            root.display(),
            outer.join(LOCAL_ROOT).display()
        ));
    }
    let metadata = fs::symlink_metadata(&outer)
        .map_err(|error| format!("cannot inspect {}: {error}", outer.display()))?;
    if !is_real_dir(&metadata) {
        return Err(format!("{} is not a real directory", outer.display()));
    }
    let canonical_checkout = fs::canonicalize(checkout)
        .map_err(|error| format!("cannot resolve {}: {error}", checkout.display()))?;
    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;
    if canonical
        != canonical_checkout
            .join(LOCAL_DIR)
            .join(root.file_name().ok_or("the local root has no folder")?)
    {
        return Err(format!(
            "{} resolves to {}, outside this checkout",
            root.display(),
            canonical.display()
        ));
    }
    Ok(())
}

/// A root that is itself a mount point is never written through.
pub(in crate::outputs) fn refuse_mount_root(root: &Path) -> Result<(), String> {
    let parent = root
        .parent()
        .ok_or_else(|| format!("{} has no parent", root.display()))?;
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("cannot inspect {}: {error}", root.display()))?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|error| format!("cannot inspect {}: {error}", parent.display()))?;
    if root_metadata.dev() != parent_metadata.dev() {
        return Err(format!(
            "{} is a mount point; Oxide does not write outputs on a mount root",
            root.display()
        ));
    }
    Ok(())
}

/// Create the root and its marker when absent, or verify an existing owned
/// root. Nothing is deleted; a nonempty root without a marker is left alone.
pub(in crate::outputs) fn ensure_root(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
) -> Result<Marker, String> {
    ensure_root_with(context, spec, generation, None)
}

/// Establish or validate an output root. `nonce` is the immutable root nonce
/// already recorded in the write-ahead transition; a new root without one
/// gets a fresh random value, and an existing root keeps its marker's.
pub(in crate::outputs) fn ensure_root_with(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
    nonce: Option<&str>,
) -> Result<Marker, String> {
    let checkout = &context.workspace;
    let root = spec.root();
    let binding = match spec {
        RootSpec::External { uuid, .. } => Some(binding_for(context, uuid)?),
        RootSpec::Local { .. } => None,
    };
    let marker = match fs::symlink_metadata(root) {
        Ok(metadata) if is_real_dir(&metadata) => {
            let marker = match read_marker(root) {
                Ok(marker) => marker,
                Err(reason) => {
                    let empty = fs::read_dir(root)
                        .map_err(|error| format!("cannot read {}: {error}", root.display()))?
                        .next()
                        .is_none();
                    if !empty {
                        return Err(format!(
                            "outputs: {reason}; {} holds content and is left untouched",
                            root.display()
                        ));
                    }
                    // Ownership is about to be established here, whether the
                    // empty directory was just created or already existed.
                    if matches!(spec, RootSpec::Local { .. }) {
                        refuse_tracked_local_root(context)?;
                    }
                    create_marker(context, spec, generation, binding.as_ref(), nonce)?
                }
            };
            let selected = spec.selected(generation).with_nonce(marker.nonce.clone());
            validate_selected(
                checkout,
                &context.checkout_id,
                &selected,
                binding.as_ref(),
                generation,
            )
            .map_err(|reason| {
                format!(
                    "outputs: {reason}; {root} is left untouched",
                    root = root.display()
                )
            })?;
            Ok(marker)
        }
        Ok(_) => Err(format!(
            "outputs: {} exists and is not a real directory; it is never replaced",
            root.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_root(context, spec, &binding)?;
            create_marker(context, spec, generation, binding.as_ref(), nonce)
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", root.display())),
    }?;
    if let Some(control) = context.control() {
        let selected = spec
            .selected(marker.generation)
            .with_nonce(marker.nonce.clone());
        crate::prune::pin_output(control, checkout, &context.checkout_id, &selected)?;
    }
    Ok(marker)
}

pub(in crate::outputs) fn binding_for(context: &Context, uuid: &str) -> Result<Binding, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists, so no removable drive is registered")?;
    let registration =
        removable::read_registration(control)?.ok_or("no removable drive is registered")?;
    if registration.volume_uuid != uuid {
        return Err(format!(
            "the registered volume is {}, not the recorded {uuid}",
            registration.volume_uuid
        ));
    }
    Binding::open(&registration.mount_point, Some(uuid))
}

fn create_root(
    context: &Context,
    spec: &RootSpec,
    binding: &Option<Binding>,
) -> Result<(), String> {
    match (spec, binding) {
        (RootSpec::External { leaf, .. }, Some(binding)) => {
            // Recheck identity, then create through the verified volume
            // descriptor: never through a re-resolved path, and never on
            // another device.
            binding.verify()?;
            let outer = binding
                .dir_at(binding.fd(), EXTERNAL_DIR, true)
                .map_err(|error| {
                    format!("cannot create the output directory on the volume: {error}")
                })?;
            binding
                .dir_at(outer.as_fd(), leaf, true)
                .map_err(|error| format!("cannot create the output root {leaf}: {error}"))?;
            Ok(())
        }
        (RootSpec::External { .. }, None) => Err(
            "the registered volume is not usable; refusing to create an external root".to_owned(),
        ),
        (RootSpec::Local { root }, _) => {
            refuse_tracked_local_root(context)?;
            create_owned_directory(&context.workspace.join(LOCAL_DIR))?;
            create_owned_directory(root)
        }
    }
}

/// Refuse to establish or write into a checkout-local output root that Git
/// would track. Checked whenever ownership is about to be established, so an
/// empty `.oxide-outputs/local` that already exists (and is not ignored) is
/// refused and left unchanged.
fn refuse_tracked_local_root(context: &Context) -> Result<(), String> {
    let checkout = &context.workspace;
    // Ask about a path *inside* the fallback root: Git only matches a
    // trailing-slash directory pattern when the path is a directory, which
    // `.oxide-outputs` may not be yet.
    let probe = format!("{LOCAL_DIR}/{LOCAL_ROOT}");
    if git_would_track(checkout, &probe)? {
        return Err(format!(
            "outputs: refusing to create the checkout-local root {} because Git would track it; add `{GITIGNORE_LINE}` to {} (oxide init does this) and re-run",
            checkout.join(LOCAL_DIR).join(LOCAL_ROOT).display(),
            checkout.join(".gitignore").display()
        ));
    }
    Ok(())
}

/// Create one owner-only real directory, refusing a symlink or another kind.
///
/// A creator that races this call is accepted only on exactly the terms the
/// first inspection gives: the entry that appeared must be the same real
/// directory this call would have made, never a symlink, a file or anything
/// else that a following write would then be redirected through.
pub(in crate::outputs) fn create_owned_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => return accept_owned_directory(path, &metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // The path was positively absent a moment ago, so this entry was
            // created between the inspection and the create. It is verified
            // as the same real directory the create would have produced;
            // nothing is replaced or chmod'ed through it.
            let metadata = fs::symlink_metadata(path)
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            return accept_owned_directory(path, &metadata);
        }
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))?;
    sync_directory(path.parent().unwrap_or(path))
}

/// One existing entry is accepted as an owned directory only while it really
/// is a directory, never a symlink or a file (the same rule
/// [`ensure_member_ancestors`] applies when it finds a component already
/// present).
pub(in crate::outputs) fn accept_owned_directory(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), String> {
    if is_real_dir(metadata) {
        Ok(())
    } else {
        Err(format!(
            "{} exists and is not a real directory; it is never replaced",
            path.display()
        ))
    }
}

/// Publish one prepared marker file without replacing anything.
pub(in crate::outputs) fn publish_marker_file(temp: &Path, path: &Path) -> Result<(), String> {
    rename_no_replace(temp, path).map_err(|error| {
        let _ = fs::remove_file(temp);
        format!("cannot publish the output root marker: {error}")
    })
}

pub(in crate::outputs) fn create_marker(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
    binding: Option<&Binding>,
    nonce: Option<&str>,
) -> Result<Marker, String> {
    let root = spec.root();
    let selected = spec.selected(generation).with_nonce(match nonce {
        Some(nonce) => Some(nonce.to_owned()),
        None => Some(crate::prune::fresh_nonce()?),
    });
    // The root must be on the intended device before the marker is written.
    validate_root_device(&context.workspace, &selected, binding)?;
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: context.workspace.clone(),
        checkout_id: context.checkout_id.clone(),
        label: identity::project_label(&context.workspace),
        recipe: RECIPE.to_owned(),
        kind: selected.kind,
        volume_uuid: selected.volume_uuid.clone(),
        nonce: selected.nonce.clone(),
        checkout_volume_uuid: crate::prune::checkout_volume_uuid(&context.workspace),
        generation,
        created_ms: process::now_ms(),
    };
    let path = root.join(MARKER);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            return Err(format!(
                "{} already exists; it is never replaced",
                path.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    let bytes = serde_json::to_vec_pretty(&marker)
        .map_err(|error| format!("cannot encode the output root marker: {error}"))?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err("the output root marker exceeds its byte limit".to_owned());
    }
    let temp = root.join(format!(".{MARKER}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    write_private(&temp, &bytes)?;
    publish_marker_file(&temp, &path)?;
    sync_directory(root)?;
    Ok(marker)
}

/// The spec that verifies an already committed selection.
pub(in crate::outputs) fn spec_for(selected: &Selected) -> RootSpec {
    match selected.kind {
        RootKind::External => RootSpec::External {
            leaf: selected
                .root
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            root: selected.root.clone(),
            uuid: selected.volume_uuid.clone().unwrap_or_default(),
        },
        RootKind::Local => RootSpec::Local {
            root: selected.root.clone(),
        },
    }
}

/// Resolve the volume binding for `selected` when it is external.
pub(in crate::outputs) fn binding_for_selected(
    context: &Context,
    selected: &Selected,
) -> Result<Option<Binding>, String> {
    match selected.kind {
        RootKind::External => Ok(Some(binding_for(
            context,
            selected.volume_uuid.as_deref().unwrap_or_default(),
        )?)),
        RootKind::Local => Ok(None),
    }
}
