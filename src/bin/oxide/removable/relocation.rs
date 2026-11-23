//! Move the areas under both admission locks and publish the new layout last.

use super::{
    read_registration, validate_layout_paths, write_registration, Binding, Registration, POOL_DIR,
    SLOTS_FOLDER,
};
use crate::outputs::{self, FileEdit, LinkEdit};
use crate::prune;
use crate::store::{self, Store};
use barge::area::AreaKey;
use barge::{Area, Barge, ErrorKind as BargeError, Os};
use oxide_storage::lease::{managed::PayloadRelocation, RecordedVolume};
use oxide_storage::storage::{OxideOs, NAMESPACE, OUTPUTS_AREA, POOL_AREA};
use oxide_storage::volume;
use serde::Serialize;
use std::ffi::CString;
use std::fs::{self, File};
use std::io::ErrorKind;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const HOLDING: &str = ".oxide-holding";
const PENDING: &str = "pool-relocation.json";
const USAGE: &str =
    "usage: oxide pool relocate --payload-dir <rel> --outputs-dir <rel> [--dry-run] [--json]";

#[derive(Debug)]
struct Options<'a> {
    payload: &'a str,
    outputs: &'a str,
    dry_run: bool,
    json: bool,
}

impl<'a> Options<'a> {
    fn parse(args: &[&'a str]) -> Result<Self, String> {
        let mut payload = None;
        let mut outputs = None;
        let mut dry_run = false;
        let mut json = false;
        let mut args = args.iter().copied();
        while let Some(flag) = args.next() {
            match flag {
                "--payload-dir" if payload.is_none() => {
                    payload = Some(
                        args.next()
                            .filter(|value| !value.starts_with("--"))
                            .ok_or("--payload-dir needs a relative path")?,
                    );
                }
                "--outputs-dir" if outputs.is_none() => {
                    outputs = Some(
                        args.next()
                            .filter(|value| !value.starts_with("--"))
                            .ok_or("--outputs-dir needs a relative path")?,
                    );
                }
                "--dry-run" if !dry_run => dry_run = true,
                "--json" if !json => json = true,
                _ => return Err(USAGE.to_owned()),
            }
        }
        let (Some(payload), Some(outputs)) = (payload, outputs) else {
            return Err(USAGE.to_owned());
        };
        validate_layout_paths(payload, outputs)?;
        for path in [payload, outputs] {
            if path
                .split('/')
                .any(|part| matches!(part, ".barge" | ".trash" | HOLDING))
            {
                return Err(format!("{path:?} uses a reserved storage directory name"));
            }
        }
        Ok(Self {
            payload,
            outputs,
            dry_run,
            json,
        })
    }
}

#[derive(Debug, Serialize)]
struct Report {
    dry_run: bool,
    old_payload: PathBuf,
    new_payload: PathBuf,
    old_outputs: PathBuf,
    new_outputs: PathBuf,
    moved_outputs: bool,
    output_links: usize,
    untouched_links: Vec<PathBuf>,
    invalidated_origins: usize,
    cleanup: Vec<String>,
}

pub(super) fn run(args: &[&str]) -> Result<u8, String> {
    if matches!(args, ["--help" | "-h"]) {
        println!("{USAGE}");
        return Ok(0);
    }
    let options = Options::parse(args)?;
    let internal = Store::open_recorded(&store::settings()?)?.ok_or("no internal pool exists")?;
    let report = relocate(&internal, &options, |registration| {
        let binding = Binding::open(&registration.mount_point, Some(&registration.volume_uuid))?;
        if !volume::writable(binding.mount_point())? {
            return Err(format!(
                "{} is not writable",
                binding.mount_point().display()
            ));
        }
        Ok(Media::Mounted(binding))
    })?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        let action = if report.dry_run {
            "would move"
        } else {
            "moved"
        };
        println!(
            "{action} pool payload from {} to {}",
            report.old_payload.display(),
            report.new_payload.display()
        );
        if report.moved_outputs {
            println!(
                "{action} outputs from {} to {}",
                report.old_outputs.display(),
                report.new_outputs.display()
            );
        } else {
            println!(
                "no outputs area on disk; new outputs directory {}",
                report.new_outputs.display()
            );
        }
        for path in &report.untouched_links {
            println!("left checkout link untouched at {}", path.display());
        }
        for warning in &report.cleanup {
            eprintln!("oxide: {warning}");
        }
    }
    Ok(0)
}

enum Media {
    Mounted(Binding),
    #[cfg(all(test, target_os = "macos"))]
    Fake {
        directory: File,
        mount: PathBuf,
        os: Arc<barge::FakeOs>,
        uuid: String,
    },
}

impl Media {
    fn fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Mounted(binding) => binding.fd(),
            #[cfg(all(test, target_os = "macos"))]
            Self::Fake { directory, .. } => directory.as_fd(),
        }
    }

    fn verify(&self) -> Result<(), String> {
        match self {
            Self::Mounted(binding) => binding.verify().map(|_| ()),
            #[cfg(all(test, target_os = "macos"))]
            Self::Fake {
                mount, os, uuid, ..
            } => {
                let observed = os
                    .volume_identity(mount)
                    .map_err(|error| error.to_string())?;
                if observed.as_deref() == Some(uuid) && mount.is_dir() {
                    Ok(())
                } else {
                    Err(format!(
                        "registered volume {} is absent or changed",
                        mount.display()
                    ))
                }
            }
        }
    }

    fn account(&self, control: &Path, read_only: bool) -> Result<Barge, String> {
        let os: Arc<dyn Os> = match self {
            Self::Mounted(_) => Arc::new(OxideOs),
            #[cfg(all(test, target_os = "macos"))]
            Self::Fake { os, .. } => os.clone(),
        };
        let open = if read_only {
            Barge::open_read_only_with
        } else {
            Barge::open_with
        };
        open(
            control.join("barge"),
            os,
            Arc::new(barge::SystemClock::new()),
        )
        .map_err(|error| error.to_string())
    }

    fn directory(&self, relative: &str) -> Result<File, String> {
        match self {
            Self::Mounted(binding) => binding.dir_path_at(relative, false).map(File::from),
            #[cfg(all(test, target_os = "macos"))]
            Self::Fake { .. } => {
                self.verify()?;
                volume::walk_dir_path(self.fd(), relative, false).map(File::from)
            }
        }
    }
}

/// Readers stop using the registration if the writer died between the area
/// moves and publication. The record preserves both layouts for inspection.
pub(super) fn check_pending(directory: &File) -> Result<(), String> {
    let name = CString::new(PENDING).map_err(|error| error.to_string())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: the name and directory descriptor are live and stat is writable.
    let found = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if found == 0 {
        return Err(format!("a pool relocation is incomplete; inspect {PENDING} in the internal pool control directory"));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() != ErrorKind::NotFound {
        return Err(format!("cannot check pool relocation state: {error}"));
    }
    Ok(())
}

fn relocate(
    internal: &Store,
    options: &Options<'_>,
    bind: impl FnOnce(&Registration) -> Result<Media, String>,
) -> Result<Report, String> {
    let control = internal.control();
    let _internal_admission = store::lock_admission(control)?;
    let old = read_registration(control)?.ok_or("no removable pool is registered")?;
    let mount = &old.mount_point;
    let mut report = Report {
        dry_run: options.dry_run,
        old_payload: mount.join(old.payload_dir()),
        new_payload: mount.join(options.payload),
        old_outputs: mount.join(old.outputs_dir()),
        new_outputs: mount.join(options.outputs),
        moved_outputs: false,
        output_links: 0,
        untouched_links: Vec::new(),
        invalidated_origins: 0,
        cleanup: Vec::new(),
    };
    let media = bind(&old)?;
    media.verify()?;
    let removable = Store::open_recorded_removable(&control.join(POOL_DIR), internal.max_active())?;
    let _removable_admission = store::lock_admission(removable.control())?;
    for pool in [internal, &removable] {
        pool.require_quiet_for_relocation()?;
    }
    let layout = PayloadRelocation::begin(
        &removable.pool,
        &RecordedVolume {
            mount_point: mount.clone(),
            volume_uuid: old.volume_uuid.clone(),
            payload_dir: old.payload_dir.clone(),
        },
    )
    .map_err(|error| format!("cannot lock the removable layout: {error}"))?;
    for destination in [&report.new_payload, &report.new_outputs] {
        for source in [&report.old_payload, &report.old_outputs] {
            if destination.starts_with(source) || source.starts_with(destination) {
                return Err(format!(
                    "relocation destination {} overlaps source {}",
                    destination.display(),
                    source.display()
                ));
            }
        }
        require_absent(destination)?;
        check_existing_parents(&media, mount, destination)?;
    }
    let old_payload = media.directory(old.payload_dir())?;
    let slots = format!("{}/{SLOTS_FOLDER}", old.payload_dir());
    volume::dir_at(old_payload.as_fd(), SLOTS_FOLDER, false)?;
    let old_holding = inspect_holding(&old_payload, &report.old_payload)?;

    let pool_area = checked_area(
        &media.account(removable.control(), true)?,
        POOL_AREA,
        &slots,
        "pool",
        &old,
    )?
    .ok_or("the removable slot area has not been declared")?;
    require_unheld(&pool_area, true)?;
    for area in media
        .account(control, true)?
        .areas()
        .map_err(|error| error.to_string())?
    {
        require_unheld(&area, false)?;
    }
    let old_account = prune::output_account(control, &report.old_outputs);
    let new_account = prune::output_account(control, &report.new_outputs);
    require_absent(&new_account)?;
    let mut has_outputs = false;
    for account in prune::output_accounts(control)? {
        let barge = media.account(&account, true)?;
        for area in barge.areas().map_err(|error| error.to_string())? {
            require_unheld(&area, account == old_account)?;
        }
        if account == old_account {
            has_outputs =
                checked_area(&barge, OUTPUTS_AREA, old.outputs_dir(), "outputs", &old)?.is_some();
        }
    }
    if has_outputs {
        media.directory(old.outputs_dir())?;
    } else {
        require_absent(&report.old_outputs)
            .map_err(|error| format!("outputs area is undeclared: {error}"))?;
        require_absent(&old_account)?;
    }
    report.moved_outputs = has_outputs;

    let mut edits = outputs::plan_relocated_receipts(
        control,
        &report.old_outputs,
        &report.new_outputs,
        &old.volume_uuid,
    )?;
    let _bindings = internal.lock_relocation_bindings(&edits.checkouts, options.dry_run)?;
    if let Some(registry) = prune::plan_registry_relocation(
        control,
        &report.old_outputs,
        &report.new_outputs,
        &old.volume_uuid,
    )? {
        edits.files.insert(0, registry);
    }
    report.untouched_links = edits.skipped;
    report.output_links = edits.links.len();
    if options.dry_run {
        return Ok(report);
    }

    let mut new = old.clone();
    new.payload_dir = options.payload.to_owned();
    new.outputs_dir = options.outputs.to_owned();
    let pending = serde_json::to_vec_pretty(&serde_json::json!({
        "old_payload": report.old_payload, "new_payload": report.new_payload,
        "old_outputs": report.old_outputs, "new_outputs": report.new_outputs,
        "move_outputs": has_outputs,
    }))
    .map_err(|error| error.to_string())?;
    store::atomic_write_private(control, PENDING, &pending)
        .map_err(|error| format!("cannot record the pending pool relocation: {error}"))?;
    let mut undo = Vec::new();
    let mut created = Vec::new();
    let new_slots = format!("{}/{SLOTS_FOLDER}", options.payload);
    let result = (|| {
        store::publish_writer_generation(control)?;
        store::publish_writer_generation(removable.control())?;
        ensure_directory(&media, mount, options.payload, true, &mut created)?;
        ensure_directory(
            &media,
            mount,
            &format!("{}/{HOLDING}", options.payload),
            true,
            &mut created,
        )?;
        if has_outputs {
            if let Some((parent, _)) = options.outputs.rsplit_once('/') {
                ensure_directory(&media, mount, parent, false, &mut created)?;
            }
        }
        let pool = media.account(removable.control(), false)?;
        move_area(&media, &pool, &pool_area.key(), &new_slots)?;
        undo.push(Undo::Area(pool, pool_area.key(), &slots, &new_slots, mount));
        sync_move(mount, &slots, &new_slots)?;

        if has_outputs {
            let barge = media.account(&old_account, false)?;
            let key = AreaKey::new(NAMESPACE, OUTPUTS_AREA).map_err(|error| error.to_string())?;
            #[cfg(all(test, target_os = "macos"))]
            let _racing_hold = tests::hold_before_outputs_move();
            move_area(&media, &barge, &key, options.outputs)?;
            undo.push(Undo::Area(
                barge,
                key,
                old.outputs_dir(),
                options.outputs,
                mount,
            ));
            sync_move(mount, old.outputs_dir(), options.outputs)?;
            // Undo the account rename before reopening Barge at the old name.
            outputs::rename_no_replace(&old_account, &new_account)
                .map_err(|error| format!("cannot move the outputs account: {error}"))?;
            undo.push(Undo::Account(&old_account, &new_account));
            sync_parent(&new_account)?;
        }
        for link in &edits.links {
            undo.push(Undo::Link(link));
            link.write(false)?;
        }
        for file in &edits.files {
            undo.push(Undo::File(file));
            file.write(false)?;
        }
        report.invalidated_origins = removable.invalidate_relocated_origins()?;
        undo.push(Undo::Config(&layout, old.payload_dir()));
        layout
            .publish(options.payload)
            .map_err(|error| error.to_string())?;
        undo.push(Undo::Registration(control, &old));
        write_registration(control, &new)
    })();
    if let Err(error) = result {
        let verify_restored = || {
            checked_area(
                &media.account(removable.control(), true)?,
                POOL_AREA,
                &slots,
                "pool",
                &old,
            )?
            .ok_or("the original slot area is missing")?;
            media.directory(old.payload_dir())?;
            volume::dir_at(old_payload.as_fd(), SLOTS_FOLDER, false)?;
            if has_outputs {
                checked_area(
                    &media.account(&old_account, true)?,
                    OUTPUTS_AREA,
                    old.outputs_dir(),
                    "outputs",
                    &old,
                )?
                .ok_or("the original output area is missing")?;
                media.directory(old.outputs_dir())?;
            }
            require_absent(&report.new_payload.join(SLOTS_FOLDER))?;
            require_absent(&report.new_outputs)
        };
        return Err(rollback(
            error,
            control,
            &media,
            undo,
            created,
            verify_restored,
        ));
    }
    clear_pending(control).map_err(|error| format!("new layout was committed; {error}"))?;
    if let Some(holding) = old_holding {
        if let Err(error) = remove_empty(&report.old_payload.join(HOLDING), &holding, &media) {
            report.cleanup.push(error);
        }
    }
    if let Err(error) = remove_empty(&report.old_payload, &old_payload, &media) {
        report.cleanup.push(error);
    }
    Ok(report)
}

fn checked_area(
    barge: &Barge,
    name: &str,
    folder: &str,
    alias: &str,
    registration: &Registration,
) -> Result<Option<Area>, String> {
    let key = format!("{NAMESPACE}/{name}");
    let area = match barge.find_area(&key) {
        Ok(area) => area,
        Err(error) if error.kind() == BargeError::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let volume = area.volume().spec();
    if area.record().effective_folder() != folder
        || volume.name != alias
        || !volume.root_dir.is_empty()
        || volume.mount != registration.mount_point
        || !volume.uuid.eq_ignore_ascii_case(&registration.volume_uuid)
        || area.location().map_err(|error| error.to_string())?
            != barge::Location::Volume(registration.mount_point.join(folder))
    {
        return Err(format!(
            "{key} records {} on volume {}; expected {folder} on {alias}",
            area.record().effective_folder(),
            volume.name
        ));
    }
    Ok(Some(area))
}

fn require_unheld(area: &Area, moving: bool) -> Result<(), String> {
    let listing = area
        .list_without_bytes()
        .map_err(|error| error.to_string())?;
    if let Some(root) = listing.roots.iter().find(|root| root.held) {
        return Err(format!("Barge root {} is held", root.path.display()));
    }
    if moving {
        if let Some(root) = listing.stragglers.first() {
            return Err(format!(
                "Barge cannot establish custody of {}: {}",
                root.path.display(),
                root.reason
            ));
        }
    }
    Ok(())
}

fn move_area(media: &Media, barge: &Barge, key: &AreaKey, folder: &str) -> Result<(), String> {
    media.verify()?;
    barge
        .relocate_area(key, folder)
        .map(|_| ())
        .map_err(|error| format!("cannot relocate {} to {folder}: {error}", key.label()))
}

fn absent(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

fn require_absent(path: &Path) -> Result<(), String> {
    if absent(path)? {
        Ok(())
    } else {
        Err(format!("{} already exists", path.display()))
    }
}

fn check_existing_parents(media: &Media, mount: &Path, path: &Path) -> Result<(), String> {
    for parent in path
        .ancestors()
        .skip(1)
        .take_while(|parent| *parent != mount)
    {
        if !absent(parent)? {
            let relative = parent
                .strip_prefix(mount)
                .ok()
                .and_then(Path::to_str)
                .ok_or("destination parent is not under the registered volume")?;
            media.directory(relative)?;
            break;
        }
    }
    Ok(())
}

fn inspect_holding(payload_fd: &File, payload: &Path) -> Result<Option<File>, String> {
    let path = payload.join(HOLDING);
    if absent(&path)? {
        return Ok(None);
    }
    let directory = File::from(volume::dir_at(payload_fd.as_fd(), HOLDING, false)?);
    if fs::read_dir(&path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .next()
        .is_some()
    {
        return Err(format!(
            "{} is non-empty; finish its recovery before relocating",
            path.display()
        ));
    }
    Ok(Some(directory))
}

fn ensure_directory(
    media: &Media,
    mount: &Path,
    relative: &str,
    exclusive_leaf: bool,
    created: &mut Vec<(PathBuf, File)>,
) -> Result<(), String> {
    media.verify()?;
    let mut parent = media
        .fd()
        .try_clone_to_owned()
        .map_err(|error| error.to_string())?;
    let mut path = mount.to_path_buf();
    // The payload prefix is already validated. Its private holding child can
    // add one level beyond the user-facing relative-path limit.
    let mut parts = relative.split('/').peekable();
    while let Some(part) = parts.next() {
        path.push(part);
        let name = CString::new(part).map_err(|error| error.to_string())?;
        // SAFETY: the directory descriptor and NUL-terminated name are live.
        let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != ErrorKind::AlreadyExists
                || (exclusive_leaf && parts.peek().is_none())
            {
                return Err(format!("cannot create {}: {error}", path.display()));
            }
        }
        let child = volume::dir_at(parent.as_fd(), part, false)?;
        if result == 0 {
            let file = File::from(
                child
                    .as_fd()
                    .try_clone_to_owned()
                    .map_err(|error| error.to_string())?,
            );
            created.push((path.clone(), file));
            sync_parent(&path)?;
        }
        parent = child;
    }
    Ok(())
}

fn remove_empty(path: &Path, file: &File, media: &Media) -> Result<(), String> {
    media.verify()?;
    let named = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    let opened = file.metadata().map_err(|error| error.to_string())?;
    if !named.is_dir()
        || named.dev() != opened.dev()
        || named.ino() != opened.ino()
        || fs::canonicalize(path).as_deref().ok() != Some(path)
    {
        return Err(format!("directory changed at {}; retained", path.display()));
    }
    fs::remove_dir(path).map_err(|error| {
        format!(
            "could not remove empty directory {}: {error}",
            path.display()
        )
    })?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("path has no parent")?;
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", parent.display()))
}

fn clear_pending(control: &Path) -> Result<(), String> {
    let path = control.join(PENDING);
    fs::remove_file(&path)
        .map_err(|error| format!("cannot clear the relocation record: {error}"))?;
    sync_parent(&path)
}

fn sync_move(mount: &Path, old: &str, new: &str) -> Result<(), String> {
    sync_parent(&mount.join(old))?;
    sync_parent(&mount.join(new))
}

enum Undo<'a> {
    Area(Barge, AreaKey, &'a str, &'a str, &'a Path),
    Account(&'a Path, &'a Path),
    Link(&'a LinkEdit),
    File(&'a FileEdit),
    Config(&'a PayloadRelocation, &'a str),
    Registration(&'a Path, &'a Registration),
}

impl Undo<'_> {
    fn restore(self, media: &Media) -> Result<(), String> {
        match self {
            Self::Area(barge, key, old, new, mount) => {
                move_area(media, &barge, &key, old)?;
                sync_move(mount, old, new)
            }
            Self::Account(old, new) => {
                outputs::rename_no_replace(new, old).map_err(|error| {
                    format!(
                        "cannot restore the outputs account at {}: {error}",
                        old.display()
                    )
                })?;
                sync_parent(old)
            }
            Self::Link(link) => link.write(true),
            Self::File(file) => file.write(true),
            Self::Config(layout, dir) => layout
                .publish(dir)
                .map_err(|error| format!("cannot restore the removable config: {error}")),
            Self::Registration(control, old) => write_registration(control, old),
        }
    }
}

fn rollback(
    error: String,
    control: &Path,
    media: &Media,
    undo: Vec<Undo<'_>>,
    created: Vec<(PathBuf, File)>,
    verify_restored: impl FnOnce() -> Result<(), String>,
) -> String {
    let mut failures = Vec::new();
    for step in undo.into_iter().rev() {
        if let Err(failure) = step.restore(media) {
            failures.push(failure);
        }
    }
    for (path, file) in created.into_iter().rev() {
        if let Err(failure) = remove_empty(&path, &file, media) {
            failures.push(failure);
        }
    }
    // A failed Barge publication can leave its directory at the destination
    // even when the call returned no success to undo.
    if let Err(failure) = verify_restored() {
        failures.push(failure);
    }
    if failures.is_empty() {
        if let Err(failure) = clear_pending(control) {
            failures.push(failure);
        }
    }
    if failures.is_empty() {
        format!("{error}; relocation rolled back")
    } else {
        format!(
            "{error}; rollback incomplete ({}); inspect {}",
            failures.join("; "),
            control.join(PENDING).display()
        )
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests;
