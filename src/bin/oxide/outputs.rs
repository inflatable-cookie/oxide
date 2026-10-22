//! Final outputs on a permanent drive: one owned checkout `target` link.
//!
//! Owning contract: `docs/knowledge/contracts/workflow.md` (Final outputs on
//! the permanent drive). Ephemeral drive mode keeps today's real
//! `<checkout>/target`. Permanent mode needs one explicit conversion
//! ([`adopt`]): `<checkout>/target` becomes an owned symlink pointing at a
//! real output root outside the pool payload, either
//! `<mount>/oxide-outputs/<label>-<checkout id>/` on the registered volume or
//! the checkout-local fallback `<checkout>/.oxide-outputs/local/` while the
//! volume is absent. `CARGO_TARGET_DIR` stays the lexical
//! `<checkout>/target`, so `target/debug` and the profiles of #106 keep
//! working.
//!
//! Nothing is moved, copied or deleted by a transition. Ownership is a marker
//! inside the root plus a committed receipt in the internal pool's control
//! directory; a link whose target does not carry this checkout's marker is
//! foreign and is never followed, adopted, swapped or cleaned through.
//! Transitions are written ahead: a `prepared` receipt, then the destination
//! root and marker, then a unique sibling link, then an atomic rename onto
//! `target`, then the committed receipt. A crash replays only the exact
//! generations the receipt and the live links name; any contradiction stops
//! with both roots retained.
//!
//! The owning lock is the internal pool's existing admission lock, together
//! with the checkout's existing clean/admission barrier: `oxide clean` and
//! `oxide outputs adopt|detach` use [`Store::begin_clean`], and a building
//! command holds its checkout's lease, which that barrier waits for. Receipt
//! writes happen under the admission lock, so one transition is published at
//! a time.

use crate::identity;
use crate::process;
use crate::removable::{self, DriveMode};
use crate::store::{self, CleanGuard, Cleaning, Store, WaitAccounting};
use crate::CARGO;
use oxide_storage::volume::{self, Binding};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{symlink, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Directory under the volume mount that holds every checkout's output root.
pub const EXTERNAL_DIR: &str = "oxide-outputs";
/// Directory inside the checkout that holds the local fallback root.
pub const LOCAL_DIR: &str = ".oxide-outputs";
/// The local fallback root's name inside [`LOCAL_DIR`].
const LOCAL_ROOT: &str = "local";
/// Ownership marker inside every output root.
pub const MARKER: &str = ".oxide-output.json";
/// The gitignore entry `oxide init` adds for the local fallback root.
pub const GITIGNORE_LINE: &str = "/.oxide-outputs/";

const RECEIPT_VERSION: &str = "oxide.outputs.transition/1";
const MARKER_VERSION: &str = "oxide.outputs.root/1";
const PLAN_VERSION: &str = "oxide.outputs.plan/1";
/// The placement recipe a marker binds: `target/` with `target/debug`.
const RECIPE: &str = "target-debug-v1";
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const MAX_MARKER_BYTES: u64 = 64 * 1024;
const MAX_PLAN_BYTES: usize = 64 * 1024;
/// Known roots kept per receipt; the oldest is dropped first.
const MAX_ROOTS: usize = 8;
/// Cooperative holders named in a plan.
const MAX_HOLDERS: usize = 16;
/// Debug-only crash injection, naming a write-ahead point.
const CRASH_ENVIRONMENT: &str = "OXIDE_TEST_OUTPUTS_CRASH";

const USAGE: &str = "\
usage: oxide outputs plan      effect-free JSON plan for this checkout (oxide.outputs.plan/1)
       oxide outputs adopt     explicitly convert <checkout>/target to an owned link
       oxide outputs detach    restore a real <checkout>/target directory; roots are kept
       oxide outputs status    read-only state of this checkout's owned outputs

Permanent drive mode owns one `<checkout>/target` symlink. Its real root is
<mount>/oxide-outputs/<project>-<checkout id>/ while the registered volume is
usable, else the checkout-local fallback <checkout>/.oxide-outputs/local/.
`plan` and `adopt` never move, copy or delete an existing target directory or
an arbitrary symlink: a nonempty real target and a foreign symlink refuse
without any change. Only an explicit `adopt` converts a missing or empty
target. `detach` refuses while the drive mode is still permanent.";

/// Which real root a link selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    External,
    Local,
}

impl RootKind {
    fn label(self) -> &'static str {
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
    kind: RootKind,
    root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    /// The generation this selection was committed at.
    generation: u64,
}

impl Selected {
    fn same_place(&self, other: &Self) -> bool {
        self.kind == other.kind && self.root == other.root && self.volume_uuid == other.volume_uuid
    }

    fn view(&self) -> SelectedView {
        SelectedView {
            kind: self.kind,
            root: self.root.clone(),
            volume_uuid: self.volume_uuid.clone(),
            generation: self.generation,
        }
    }

    fn at(&self, generation: u64) -> Self {
        Self {
            generation,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct SelectedView {
    kind: RootKind,
    root: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    generation: u64,
}

/// A transition written ahead but not committed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    generation: u64,
    to: Selected,
    link: PathBuf,
}

/// Receipt state. `Prepared` is the only state a crash can leave behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReceiptState {
    Committed,
    Prepared,
    Detached,
}

/// The checkout's owned-output receipt, keyed by canonical checkout.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: String,
    checkout: PathBuf,
    checkout_id: String,
    /// Last committed generation; zero before the first conversion.
    generation: u64,
    state: ReceiptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<Selected>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prepared: Option<Prepared>,
    /// Bounded history of roots this checkout has selected, newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    roots: Vec<Selected>,
    updated_ms: u64,
}

impl Receipt {
    fn first(checkout: &Path, checkout_id: &str) -> Self {
        Self {
            version: RECEIPT_VERSION.to_owned(),
            checkout: checkout.to_path_buf(),
            checkout_id: checkout_id.to_owned(),
            generation: 0,
            state: ReceiptState::Detached,
            selected: None,
            prepared: None,
            roots: Vec::new(),
            updated_ms: process::now_ms(),
        }
    }

    fn validate(&self, checkout: &Path, checkout_id: &str) -> Result<(), String> {
        if self.version != RECEIPT_VERSION {
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
        if let Some(selected) = &self.selected {
            if !safe_absolute(&selected.root) || selected.generation > self.generation {
                return Err(
                    "the owned-output receipt selects an unusable root or generation".to_owned(),
                );
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
        match (self.state, &self.selected, &self.prepared) {
            (ReceiptState::Committed, Some(_), None) => {}
            (ReceiptState::Detached, None, None) => {}
            (ReceiptState::Prepared, _, Some(prepared))
                if prepared.generation == self.generation.saturating_add(1)
                    && prepared.generation > 0
                    && safe_absolute(&prepared.link)
                    && safe_absolute(&prepared.to.root) => {}
            _ => {
                return Err(
                    "the owned-output receipt is internally inconsistent; it is not followed"
                        .to_owned(),
                )
            }
        }
        Ok(())
    }

    /// The root the current owned link must select, when committed.
    fn selected(&self) -> Option<&Selected> {
        match self.state {
            ReceiptState::Committed => self.selected.as_ref(),
            ReceiptState::Prepared | ReceiptState::Detached => None,
        }
    }
}

/// Ownership marker inside an output root. Written once, at root creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: String,
    checkout: PathBuf,
    checkout_id: String,
    label: String,
    recipe: String,
    kind: RootKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    volume_uuid: Option<String>,
    /// The generation the root was created at; never rewritten.
    generation: u64,
    created_ms: u64,
}

/// How an output root is found and created.
#[derive(Clone, Debug)]
enum RootSpec {
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
    fn root(&self) -> &Path {
        match self {
            Self::External { root, .. } | Self::Local { root } => root,
        }
    }

    fn selected(&self, generation: u64) -> Selected {
        match self {
            Self::External { root, uuid, .. } => Selected {
                kind: RootKind::External,
                root: root.clone(),
                volume_uuid: Some(uuid.clone()),
                generation,
            },
            Self::Local { root } => Selected {
                kind: RootKind::Local,
                root: root.clone(),
                volume_uuid: None,
                generation,
            },
        }
    }
}

/// What `<checkout>/target` is right now.
#[derive(Clone, Debug, Eq, PartialEq)]
enum TargetState {
    Missing,
    RealEmpty,
    RealNonEmpty,
    /// A symlink whose target is a real directory carrying this checkout's
    /// ownership marker.
    OwnedLink(PathBuf),
    /// Any other symlink or non-directory.
    Foreign,
}

impl TargetState {
    fn label(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::RealEmpty => "an empty directory",
            Self::RealNonEmpty => "a directory with content",
            Self::OwnedLink(_) => "an owned Oxide link",
            Self::Foreign => "a symlink or file Oxide does not own",
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::RealEmpty => "real-empty",
            Self::RealNonEmpty => "real-nonempty",
            Self::OwnedLink(_) => "owned-link",
            Self::Foreign => "foreign",
        }
    }

    fn root(&self) -> Option<&Path> {
        match self {
            Self::OwnedLink(root) => Some(root),
            _ => None,
        }
    }
}

/// What one command did to its checkout's owned outputs.
#[derive(Default)]
pub struct Outcome {
    /// Bounded, single-line warnings, printed before Cargo starts.
    pub warnings: Vec<String>,
    /// One clause for the command's summary line.
    pub note: Option<String>,
}

/// `oxide outputs ...`
pub fn run(args: &[OsString]) -> Result<u8, String> {
    let args: Vec<&str> = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be valid UTF-8"))
        .collect::<Result<_, _>>()?;
    match args.as_slice() {
        ["plan"] => plan_command(),
        ["adopt"] => adopt_command(),
        ["detach"] => detach_command(),
        ["status"] => status_command(),
        ["-h" | "--help"] | [] => {
            println!("{USAGE}");
            Ok(0)
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// One checkout's output context. Read-only: a receipt read never creates a
/// directory.
struct Context {
    control: Option<PathBuf>,
    workspace: PathBuf,
    checkout_id: String,
}

impl Context {
    fn open() -> Result<Self, String> {
        let settings = store::settings()?;
        let workspace = identity::locate_workspace(CARGO, None)?;
        let control =
            Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
        Ok(Self::new(control, workspace))
    }

    fn new(control: Option<PathBuf>, workspace: PathBuf) -> Self {
        let checkout_id = identity::digest(&workspace.to_string_lossy());
        Self {
            control,
            workspace,
            checkout_id,
        }
    }

    fn control(&self) -> Option<&Path> {
        self.control.as_deref()
    }

    fn mode(&self) -> Result<Option<DriveMode>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => Ok(removable::read_registration(control)?.map(|item| item.mode)),
        }
    }

    fn receipt(&self) -> Result<Option<Receipt>, String> {
        match self.control() {
            None => Ok(None),
            Some(control) => read_receipt(control, &self.workspace, &self.checkout_id),
        }
    }

    fn target(&self) -> Result<TargetState, String> {
        target_state(&self.workspace, &self.checkout_id)
    }

    fn link(&self) -> PathBuf {
        self.workspace.join("target")
    }
}

fn mode_label(mode: Option<DriveMode>) -> &'static str {
    match mode {
        Some(DriveMode::Permanent) => "permanent",
        Some(DriveMode::Ephemeral) => "ephemeral",
        None => "unregistered",
    }
}

fn plan_command() -> Result<u8, String> {
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
struct PlanReport {
    version: &'static str,
    checkout: PathBuf,
    mode: &'static str,
    state: &'static str,
    current_target: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<SelectedView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposed_root: Option<SelectedView>,
    proposed_fallback_root: SelectedView,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Always false: a conversion never moves existing output.
    data_move: bool,
    cooperative_holders: Vec<String>,
    next_generation: u64,
}

fn plan_report(context: &Context) -> Result<PlanReport, String> {
    let mode = context.mode()?;
    let receipt = context.receipt()?;
    let target = context.target()?;
    let state = receipt
        .as_ref()
        .map_or("none", |receipt| match receipt.state {
            ReceiptState::Committed => "committed",
            ReceiptState::Prepared => "prepared",
            ReceiptState::Detached => "detached",
        });
    let next_generation = receipt
        .as_ref()
        .map_or(1, |receipt| receipt.generation.saturating_add(1));
    let holders = match context.control() {
        None => Vec::new(),
        Some(control) => {
            let mut holders = store::active_holders(control, &context.workspace)?;
            holders.truncate(MAX_HOLDERS);
            holders
        }
    };
    let usable = usable_external(context);
    let external = usable
        .as_ref()
        .ok()
        .map(|registration| external_spec(registration, &context.workspace));
    let local = local_spec(&context.workspace);
    let mut report = PlanReport {
        version: PLAN_VERSION,
        checkout: context.workspace.clone(),
        mode: mode_label(mode),
        state,
        current_target: target.code(),
        current_root: target.root().map(Path::to_path_buf),
        selected: receipt
            .as_ref()
            .and_then(Receipt::selected)
            .map(Selected::view),
        proposed_root: external
            .as_ref()
            .map(|spec| spec.selected(next_generation).view()),
        proposed_fallback_root: local.selected(next_generation).view(),
        action: "refuse",
        reason: None,
        data_move: false,
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
                report.reason = Some(
                    "the existing target directory has content; Oxide never moves, adopts or deletes it. Move it aside yourself if you accept losing its warm build, then re-run `oxide outputs adopt`"
                        .to_owned(),
                );
            }
            TargetState::Foreign => {
                report.reason = Some(
                    "the existing target is a symlink or file Oxide does not own; it is never followed, adopted or replaced"
                        .to_owned(),
                );
            }
            TargetState::OwnedLink(_) => {
                report.reason = Some(
                    "the target is an owned link but this checkout has no receipt; remove it yourself before adopting"
                        .to_owned(),
                );
            }
        }
        return Ok(report);
    };
    if receipt.state == ReceiptState::Prepared {
        report.reason = Some(format!(
            "an interrupted transition to {} is pending; run `oxide outputs adopt` to replay it (its exact generations are rechecked)",
            receipt.prepared.as_ref().map_or_else(
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
    match (&target, target.root()) {
        (TargetState::OwnedLink(root), _) if *root == selected.root => {
            let wanted = match &usable {
                Ok(registration) => {
                    external_spec(registration, &context.workspace).selected(next_generation)
                }
                Err(_) => local.selected(next_generation),
            };
            report.action = if wanted.same_place(&selected) {
                "noop"
            } else {
                "relink"
            };
            if let Err(reason) = &usable {
                report.reason = Some(format!(
                    "the volume is not usable ({reason}); the next command would select {}",
                    local.root().display()
                ));
            }
            Ok(report)
        }
        _ => {
            report.reason = Some(format!(
                "the receipt selects {} but the target is {}; `oxide outputs adopt` rechecks and relinks it",
                selected.root.display(),
                target.label()
            ));
            Ok(report)
        }
    }
}

fn status_command() -> Result<u8, String> {
    for line in status_lines_for_current()? {
        println!("{line}");
    }
    Ok(0)
}

fn status_lines_for_current() -> Result<Vec<String>, String> {
    let context = Context::open()?;
    output_status_lines(&context)
}

/// Read-only owned-output lines for `oxide status`. Never creates a pool,
/// directory or receipt.
pub fn status_lines(workspace: &Path) -> Result<Vec<String>, String> {
    let settings = store::settings()?;
    let control = Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
    output_status_lines(&Context::new(control, workspace.to_path_buf()))
}

fn output_status_lines(context: &Context) -> Result<Vec<String>, String> {
    let mode = context.mode()?;
    let target = context.target()?;
    let receipt = context.receipt()?;
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
                ReceiptState::Prepared => format!(
                    "an interrupted transition to {} is pending",
                    receipt
                        .prepared
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
        if local.root().is_dir() {
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
                if root.root.is_dir() {
                    "present"
                } else {
                    "absent"
                }
            ));
        }
        if let Some(selected) = receipt.selected() {
            if target.root() != Some(selected.root.as_path()) {
                lines.push(format!(
                    "outputs: the target is {} rather than the owned link selecting {}; run `oxide outputs adopt`",
                    target.label(),
                    selected.root.display()
                ));
            }
        }
    }
    Ok(lines)
}

/// `oxide outputs adopt`: the explicit first conversion and later relinks.
fn adopt_command() -> Result<u8, String> {
    let context = Context::open()?;
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
    let Some(internal) = Store::open_existing()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(&internal, &context.workspace, &mut waits)?;
    let outcome = (|| {
        let _admission = store::lock_admission(&control)?;
        let receipt = match read_receipt(&control, &context.workspace, &context.checkout_id)? {
            None => Receipt::first(&context.workspace, &context.checkout_id),
            Some(receipt) => replay(&context, receipt)?,
        };
        adopt_under_barrier(&context, receipt)
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

fn adopt_under_barrier(
    context: &Context,
    receipt: Receipt,
) -> Result<(Receipt, Selected, bool), String> {
    let target = context.target()?;
    let usable = usable_external(context);
    let spec = match &usable {
        Ok(registration) => external_spec(registration, &context.workspace),
        Err(_) => local_spec(&context.workspace),
    };
    let current = receipt.selected().cloned();
    match (&target, &current) {
        // Already converted to the place this command would select.
        (TargetState::OwnedLink(root), Some(selected))
            if *root == selected.root && spec.selected(0).same_place(selected) =>
        {
            ensure_root(context, &spec, receipt.generation)?;
            let selected = selected.clone();
            Ok((receipt, selected, false))
        }
        // A committed receipt whose owned link selects another known place:
        // relink under the barrier.
        (TargetState::OwnedLink(root), Some(selected)) if *root == selected.root => {
            let committed = transition(context, &receipt, &spec)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // A committed receipt whose link is missing: recreate it.
        (TargetState::Missing, Some(_)) => {
            let committed = transition(context, &receipt, &spec)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        // Explicit first conversion, or one after `detach`.
        (TargetState::Missing | TargetState::RealEmpty, _) => {
            let committed = transition(context, &receipt, &spec)?;
            let selected = committed
                .selected()
                .cloned()
                .ok_or("the transition committed no root")?;
            Ok((committed, selected, true))
        }
        (TargetState::RealNonEmpty | TargetState::Foreign | TargetState::OwnedLink(_), _) => {
            Err(refuse_existing_target(context, &target))
        }
    }
}

fn refuse_existing_target(context: &Context, target: &TargetState) -> String {
    format!(
        "outputs adopt refused: {} is {}; Oxide never moves, adopts or deletes it. Nothing was changed. {}",
        context.link().display(),
        target.label(),
        match target {
            TargetState::RealNonEmpty =>
                "Move it aside yourself if you accept losing its warm build, then re-run `oxide outputs adopt`.",
            TargetState::OwnedLink(_) =>
                "Its receipt does not select this root, so Oxide will not touch it.",
            _ =>
                "Remove or restore it yourself, then re-run `oxide outputs adopt`.",
        }
    )
}

/// `oxide outputs detach`: back to a real target directory, keeping roots.
fn detach_command() -> Result<u8, String> {
    let context = Context::open()?;
    let mode = context.mode()?;
    if mode == Some(DriveMode::Permanent) {
        return Err(
            "outputs detach refused while the drive mode is still permanent: a permanent checkout would only relink. Set `oxide pool set-mode ephemeral` first, then detach"
                .to_owned(),
        );
    }
    let Some(control) = context.control().map(Path::to_path_buf) else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    let Some(internal) = Store::open_existing()? else {
        return Err("no internal pool exists; there is no owned output state".to_owned());
    };
    process::install_signal_handlers()?;
    let mut waits = WaitAccounting::default();
    let guard = barrier(&internal, &context.workspace, &mut waits)?;
    let outcome = (|| {
        let _admission = store::lock_admission(&control)?;
        let receipt = read_receipt(&control, &context.workspace, &context.checkout_id)?
            .ok_or("this checkout has no owned-output receipt; nothing to detach")?;
        let mut receipt = replay(&context, receipt)?;
        if receipt.state == ReceiptState::Detached {
            return Ok((receipt, false));
        }
        let selected = receipt
            .selected()
            .cloned()
            .ok_or("the receipt selects no root; nothing to detach")?;
        let target = context.target()?;
        if target.root() != Some(selected.root.as_path()) {
            return Err(format!(
                "outputs detach refused: {} is {}, not the owned link selecting {}; it is left untouched",
                context.link().display(),
                target.label(),
                selected.root.display()
            ));
        }
        let link = context.link();
        fs::remove_file(&link)
            .map_err(|error| format!("cannot remove the owned link {}: {error}", link.display()))?;
        if let Err(error) = fs::create_dir(&link) {
            return Err(format!(
                "the owned link was removed, but creating the real {} failed: {error}; re-run `oxide outputs adopt` to restore the link",
                link.display()
            ));
        }
        fs::set_permissions(&link, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("cannot set permissions on {}: {error}", link.display()))?;
        sync_directory(&context.workspace)?;
        if let Some(previous) = receipt.selected.take() {
            remember_root(&mut receipt, previous);
        }
        receipt.state = ReceiptState::Detached;
        receipt.prepared = None;
        receipt.updated_ms = process::now_ms();
        write_receipt(&control, &receipt)?;
        Ok((receipt, true))
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

/// A build binds its checkout's output root once, after taking its exclusive
/// checkout lease and before Cargo starts. A transition never redirects a
/// running command: another command in the same checkout waits for that
/// lease, and `oxide clean` and `oxide outputs adopt|detach` wait for it
/// through the clean barrier.
pub fn bind_for_command(control: Option<&Path>, checkout: &Path) -> Result<Outcome, String> {
    let context = Context::new(control.map(Path::to_path_buf), checkout.to_path_buf());
    let mode = context.mode()?;
    if mode == Some(DriveMode::Permanent) {
        let Some(control) = context.control() else {
            return Ok(Outcome::default());
        };
        let _admission = store::lock_admission(control)?;
        let Some(receipt) = context.receipt()? else {
            return Ok(Outcome {
                warnings: vec![
                    "permanent drive mode: this checkout is not converted to owned outputs; `oxide outputs plan` shows the conversion plan and `oxide outputs adopt` performs it. Nothing was moved"
                        .to_owned(),
                ],
                note: Some("outputs not converted".to_owned()),
            });
        };
        let receipt = if receipt.state == ReceiptState::Prepared {
            replay(&context, receipt)?
        } else {
            receipt
        };
        return bind_with_receipt(&context, receipt);
    }
    // Ephemeral mode keeps today's real checkout/target. A checkout that was
    // converted keeps its owned link but is never relinked automatically.
    let Some(receipt) = context.receipt()? else {
        return Ok(Outcome::default());
    };
    match receipt.state {
        ReceiptState::Detached => Ok(Outcome::default()),
        ReceiptState::Prepared => Err(
            "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            let selected = receipt
                .selected()
                .cloned()
                .ok_or("outputs: the receipt selects no root")?;
            let target = context.target()?;
            if target.root() == Some(selected.root.as_path()) {
                return Ok(Outcome::default().with_note(format!(
                    "owned outputs {} through an owned link",
                    selected.root.display()
                )));
            }
            Err(format!(
                "outputs: {} is {}, not the owned link selecting {}; the drive mode is ephemeral, so Oxide will not relink it. Run `oxide outputs adopt` (or set the drive mode to permanent) before building",
                context.link().display(),
                target.label(),
                selected.root.display()
            ))
        }
    }
}

impl Outcome {
    fn with_note(mut self, note: String) -> Self {
        self.note = Some(note);
        self
    }
}

fn bind_with_receipt(context: &Context, receipt: Receipt) -> Result<Outcome, String> {
    match receipt.state {
        ReceiptState::Detached => Ok(Outcome {
            warnings: vec![
                "permanent drive mode: this checkout is detached from owned outputs; `oxide outputs adopt` converts it again. Nothing was moved"
                    .to_owned(),
            ],
            note: Some("outputs detached".to_owned()),
        }),
        ReceiptState::Prepared => Err(
            "outputs: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before building"
                .to_owned(),
        ),
        ReceiptState::Committed => {
            let target = context.target()?;
            let usable = usable_external(context);
            let spec = match &usable {
                Ok(registration) => external_spec(registration, &context.workspace),
                Err(_) => local_spec(&context.workspace),
            };
            let selected = receipt
                .selected()
                .cloned()
                .ok_or("outputs: the receipt selects no root")?;
            let wanted = spec.selected(receipt.generation.saturating_add(1));
            if target.root() == Some(wanted.root.as_path()) {
                let mut outcome = Outcome::default();
                if let Err(reason) = &usable {
                    outcome.warnings.push(format!(
                        "permanent drive mode: outputs use the checkout-local fallback root {} ({reason})",
                        wanted.root.display()
                    ));
                }
                return Ok(outcome.with_note(format!(
                    "{} outputs {}",
                    if wanted.kind == RootKind::Local {
                        "fallback"
                    } else {
                        "owned"
                    },
                    wanted.root.display()
                )));
            }
            match &target {
                TargetState::Missing => {}
                TargetState::OwnedLink(root) if *root == selected.root => {}
                _ => {
                    return Err(format!(
                        "outputs: {} is {}, not this checkout's owned link; Oxide will not replace it. Run `oxide outputs detach` or restore the link yourself",
                        context.link().display(),
                        target.label()
                    ))
                }
            }
            let committed = transition(context, &receipt, &spec)?;
            let to = committed
                .selected()
                .cloned()
                .ok_or("outputs: the transition committed no root")?;
            let mut outcome = Outcome::default();
            if let Err(reason) = &usable {
                outcome.warnings.push(format!(
                    "permanent drive mode: outputs fell back to the checkout-local root {} ({reason}); the external root is kept",
                    to.root.display()
                ));
            }
            Ok(outcome.with_note(format!(
                "outputs relinked to the {} root {}",
                to.kind.label(),
                to.root.display()
            )))
        }
    }
}

/// Acquire the checkout's clean barrier: no other command in this checkout
/// holds or takes a slot, and no clean runs, until the guard is released.
fn barrier(
    store: &Store,
    checkout: &Path,
    waits: &mut WaitAccounting,
) -> Result<CleanGuard, String> {
    match store.begin_clean(checkout, waits)? {
        Cleaning::Guard(guard) => Ok(guard),
        Cleaning::Interrupted(signal) => Err(format!(
            "interrupted by signal {signal} while waiting for this checkout's commands to finish; nothing was changed"
        )),
    }
}

/// End the barrier without dropping slot affinity: an output transition is
/// not a clean.
fn release_barrier(store: &Store, guard: CleanGuard) -> Result<(), String> {
    store.abandon_clean(guard)
}

/// The registered volume when the permanent drive is usable right now.
fn usable_external(context: &Context) -> Result<removable::Registration, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists, so no removable drive is registered")?;
    let registration =
        removable::read_registration(control)?.ok_or("no removable drive is registered")?;
    if registration.mode != DriveMode::Permanent {
        return Err("the drive is not in permanent mode".to_owned());
    }
    if !cfg!(target_os = "macos") {
        return Err("the removable backend is macOS-only".to_owned());
    }
    let binding = Binding::open(&registration.mount_point, Some(&registration.volume_uuid))?;
    if !volume::writable(binding.mount_point())? {
        return Err("the volume is not writable".to_owned());
    }
    let facts = binding.verify()?;
    let available = facts
        .available_bytes
        .ok_or("the volume's free space is unknown")?;
    if available <= registration.reserve_bytes {
        return Err(format!(
            "{} MiB available is not above the {} MiB reserve",
            available / crate::MIB,
            registration.reserve_bytes / crate::MIB
        ));
    }
    Ok(registration)
}

fn external_spec(registration: &removable::Registration, checkout: &Path) -> RootSpec {
    let label = identity::project_label(checkout);
    let id = identity::digest(&checkout.to_string_lossy());
    let leaf = format!("{label}-{}", &id[..8]);
    RootSpec::External {
        root: registration.mount_point.join(EXTERNAL_DIR).join(&leaf),
        uuid: registration.volume_uuid.clone(),
        leaf,
    }
}

fn local_spec(checkout: &Path) -> RootSpec {
    RootSpec::Local {
        root: checkout.join(LOCAL_DIR).join(LOCAL_ROOT),
    }
}

/// Classify `<checkout>/target` without following a link away from ownership.
fn target_state(checkout: &Path, checkout_id: &str) -> Result<TargetState, String> {
    let target = checkout.join("target");
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
        return Ok(match owned_root(checkout, checkout_id, &root) {
            Ok(()) => TargetState::OwnedLink(root),
            Err(_) => TargetState::Foreign,
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

/// A root is owned by this checkout when it is a real directory carrying this
/// checkout's marker. Volume binding and generation are checked against the
/// receipt by [`validate_selected`].
fn owned_root(checkout: &Path, checkout_id: &str, root: &Path) -> Result<(), String> {
    if !safe_absolute(root) {
        return Err(format!("{} is not an absolute path", root.display()));
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("cannot inspect {}: {error}", root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not a real directory", root.display()));
    }
    let marker = read_marker(root)?;
    check_marker(&marker, checkout, checkout_id)
}

fn check_marker(marker: &Marker, checkout: &Path, checkout_id: &str) -> Result<(), String> {
    if marker.version != MARKER_VERSION {
        return Err(format!(
            "{} carries an unsupported owner marker version",
            checkout.display()
        ));
    }
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
fn validate_selected(
    checkout: &Path,
    checkout_id: &str,
    selected: &Selected,
    binding: Option<&Binding>,
    receipt_generation: u64,
) -> Result<(), String> {
    owned_root(checkout, checkout_id, &selected.root)?;
    let marker = read_marker(&selected.root)?;
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
    validate_root_device(checkout, selected, binding)?;
    Ok(())
}

/// A root must be a real directory on the intended device, and never a mount
/// root of its own.
fn validate_root_device(
    checkout: &Path,
    selected: &Selected,
    binding: Option<&Binding>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(&selected.root)
        .map_err(|error| format!("cannot inspect {}: {error}", selected.root.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!(
            "{} is not a real directory",
            selected.root.display()
        ));
    }
    refuse_mount_root(&selected.root)?;
    match (selected.kind, binding) {
        (RootKind::Local, _) => {
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

/// A root that is itself a mount point is never written through.
fn refuse_mount_root(root: &Path) -> Result<(), String> {
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
fn ensure_root(context: &Context, spec: &RootSpec, generation: u64) -> Result<Marker, String> {
    let checkout = &context.workspace;
    let root = spec.root();
    let binding = match spec {
        RootSpec::External { uuid, .. } => Some(binding_for(context, uuid)?),
        RootSpec::Local { .. } => None,
    };
    let selected = spec.selected(generation);
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
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
                    create_marker(context, spec, generation, binding.as_ref())?
                }
            };
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
            create_marker(context, spec, generation, binding.as_ref())
        }
        Err(error) => Err(format!("cannot inspect {}: {error}", root.display())),
    }
}

fn binding_for(context: &Context, uuid: &str) -> Result<Binding, String> {
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
            let checkout = &context.workspace;
            let outer = checkout.join(LOCAL_DIR);
            // Ask about a path *inside* the fallback root: Git only matches a
            // trailing-slash directory pattern when the path is a directory,
            // which `.oxide-outputs` is not yet.
            let probe = format!("{LOCAL_DIR}/{LOCAL_ROOT}");
            if !outer.exists() && git_would_track(checkout, &probe)? {
                return Err(format!(
                    "outputs: refusing to create the checkout-local root {} because Git would track it; add `{GITIGNORE_LINE}` to {} (oxide init does this) and re-run",
                    root.display(),
                    checkout.join(".gitignore").display()
                ));
            }
            create_owned_directory(&outer)?;
            create_owned_directory(root)
        }
    }
}

/// Create one owner-only real directory, refusing a symlink or another kind.
fn create_owned_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => {
            return Err(format!(
                "{} exists and is not a real directory; it is never replaced",
                path.display()
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("cannot create {}: {error}", path.display())),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("cannot set permissions on {}: {error}", path.display()))?;
    sync_directory(path.parent().unwrap_or(path))
}

fn create_marker(
    context: &Context,
    spec: &RootSpec,
    generation: u64,
    binding: Option<&Binding>,
) -> Result<Marker, String> {
    let root = spec.root();
    let selected = spec.selected(generation);
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
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("cannot publish the output root marker: {error}")
    })?;
    sync_directory(root)?;
    Ok(marker)
}

/// Write the committed receipt for one transition.
fn transition(context: &Context, receipt: &Receipt, spec: &RootSpec) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    let link = context.link();
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    // Only the receipt's own owned link, a missing target, or (for the
    // explicit conversion) an empty real directory may be replaced.
    let target = context.target()?;
    match (&target, receipt.selected()) {
        (TargetState::Missing, _) => {}
        (TargetState::RealEmpty, _) => {
            refuse_mount_root(&link)?;
            fs::remove_dir(&link).map_err(|error| {
                format!(
                    "cannot remove the empty target directory {}: {error}",
                    link.display()
                )
            })?;
        }
        (TargetState::OwnedLink(root), Some(selected)) if *root == selected.root => {}
        _ => return Err(refuse_existing_target(context, &target)),
    }
    let to = spec.selected(generation);
    // Write-ahead: prepared first, then the destination root, then the
    // sibling link, then the rename, then the committed receipt.
    let mut writing = receipt.clone();
    writing.state = ReceiptState::Prepared;
    writing.prepared = Some(Prepared {
        generation,
        to: to.clone(),
        link: link.clone(),
    });
    writing.updated_ms = process::now_ms();
    write_receipt(control, &writing)?;
    crash_point("prepared")?;

    ensure_root(context, spec, generation)?;
    let sibling = sibling_link(&link, generation);
    match fs::symlink_metadata(&sibling) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            symlink(&to.root, &sibling).map_err(|error| {
                format!(
                    "cannot create the transition link {}: {error}",
                    sibling.display()
                )
            })?;
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing = fs::read_link(&sibling)
                .map_err(|error| format!("cannot read {}: {error}", sibling.display()))?;
            if existing != to.root {
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
    if sibling_read != to.root {
        return Err(format!(
            "outputs: {} does not read back as the transition root",
            sibling.display()
        ));
    }
    sync_directory(&context.workspace)?;
    crash_point("linked")?;

    fs::rename(&sibling, &link).map_err(|error| {
        format!(
            "cannot publish the owned link {}: {error}; the previous link is unchanged",
            link.display()
        )
    })?;
    sync_directory(&context.workspace)?;
    crash_point("renamed")?;
    let prepared = writing
        .prepared
        .clone()
        .ok_or("the prepared receipt lost its transition")?;
    finish_commit(context, receipt, &prepared)
}

/// Commit a prepared transition whose new link is live.
fn finish_commit(
    context: &Context,
    receipt: &Receipt,
    prepared: &Prepared,
) -> Result<Receipt, String> {
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    let link = context.link();
    let live =
        fs::read_link(&link).map_err(|error| format!("cannot read {}: {error}", link.display()))?;
    if live != prepared.to.root {
        return Err(format!(
            "outputs: {} selects {}, not the prepared {}; both roots are kept",
            link.display(),
            live.display(),
            prepared.to.root.display()
        ));
    }
    let generation = receipt
        .generation
        .checked_add(1)
        .ok_or("the output generation sequence is exhausted")?;
    ensure_root(context, &spec_for(&prepared.to), generation)?;
    let selected = prepared.to.at(prepared.generation);
    let mut committed = receipt.clone();
    committed.state = ReceiptState::Committed;
    committed.generation = prepared.generation;
    committed.selected = Some(selected.clone());
    committed.prepared = None;
    remember_root(&mut committed, selected);
    committed.updated_ms = process::now_ms();
    write_receipt(control, &committed)?;
    Ok(committed)
}

/// The spec that verifies an already committed selection.
fn spec_for(selected: &Selected) -> RootSpec {
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

fn remember_root(receipt: &mut Receipt, root: Selected) {
    receipt.roots.retain(|known| known.root != root.root);
    receipt.roots.push(root);
    while receipt.roots.len() > MAX_ROOTS {
        receipt.roots.remove(0);
    }
}

fn sibling_link(link: &Path, generation: u64) -> PathBuf {
    let name = link
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "target".to_owned());
    link.with_file_name(format!(".{name}.oxide-new-{generation}"))
}

/// Debug-only crash injection: stop at a write-ahead point without further
/// state, exactly as a killed process would leave the fixture.
fn crash_point(point: &str) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if std::env::var(CRASH_ENVIRONMENT).is_ok_and(|value| value == point) {
        return Err(format!(
            "outputs: injected transition stop at {point} ({CRASH_ENVIRONMENT}={point}); the receipt stays prepared"
        ));
    }
    let _ = point;
    Ok(())
}

/// Replay a prepared receipt: the exact new/old generations only.
fn replay(context: &Context, receipt: Receipt) -> Result<Receipt, String> {
    if receipt.state != ReceiptState::Prepared {
        return Ok(receipt);
    }
    let prepared = receipt
        .prepared
        .clone()
        .ok_or("the prepared receipt names no transition")?;
    let control = context
        .control()
        .ok_or("no internal pool exists; there is no place for an output receipt")?;
    let link = &prepared.link;
    let target = context.target()?;
    let sibling = sibling_link(link, prepared.generation);
    let sibling_read = fs::read_link(&sibling).ok();
    let owned_new = owned_root(&context.workspace, &context.checkout_id, &prepared.to.root).is_ok();
    let old_live = match (&target, receipt.selected()) {
        (TargetState::Missing, _) => true,
        (TargetState::OwnedLink(root), Some(selected)) => *root == selected.root,
        (TargetState::OwnedLink(_), None) => true,
        _ => false,
    };
    // 1. The rename already happened: commit.
    if owned_new && target.root() == Some(prepared.to.root.as_path()) {
        return finish_commit(context, &receipt, &prepared);
    }
    // 2. The sibling link is live: retry the rename under the barrier.
    if owned_new && sibling_read.as_deref() == Some(prepared.to.root.as_path()) && old_live {
        fs::rename(&sibling, link).map_err(|error| {
            format!(
                "cannot replay the prepared link rename {}: {error}; both roots are kept",
                link.display()
            )
        })?;
        sync_directory(&context.workspace)?;
        let live = fs::read_link(link)
            .map_err(|error| format!("cannot read {}: {error}", link.display()))?;
        if live != prepared.to.root {
            return Err(stop_unknown(
                context,
                &prepared,
                "the replayed link does not read back as the prepared root",
            ));
        }
        return finish_commit(context, &receipt, &prepared);
    }
    // 3. Nothing was published: roll the preparation back, keeping the old
    //    selection and any root the attempt created.
    if !old_live {
        return Err(stop_unknown(
            context,
            &prepared,
            "neither the prepared sibling nor the receipt's old target link is live",
        ));
    }
    let mut rolled = receipt.clone();
    rolled.state = if rolled.selected.is_some() {
        ReceiptState::Committed
    } else {
        ReceiptState::Detached
    };
    rolled.prepared = None;
    if prepared.to.root.is_dir() {
        remember_root(&mut rolled, prepared.to);
    }
    rolled.updated_ms = process::now_ms();
    write_receipt(control, &rolled)?;
    Ok(rolled)
}

fn stop_unknown(context: &Context, prepared: &Prepared, reason: &str) -> String {
    format!(
        "outputs: the prepared transition to generation {} cannot be replayed: {reason}. Both roots and {} are kept exactly as they are; inspect `oxide outputs status` and the file system before acting",
        prepared.generation,
        context.link().display()
    )
}

fn read_receipt(
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
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))?;
    receipt.validate(checkout, checkout_id)?;
    Ok(Some(receipt))
}

fn receipt_path(control: &Path, checkout_id: &str) -> PathBuf {
    control.join("outputs").join(format!("{checkout_id}.json"))
}

fn write_receipt(control: &Path, receipt: &Receipt) -> Result<(), String> {
    receipt.validate(&receipt.checkout, &receipt.checkout_id)?;
    let directory = control.join("outputs");
    create_owned_directory(&directory)?;
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
    sync_directory(&directory)
}

fn read_marker(root: &Path) -> Result<Marker, String> {
    let path = root.join(MARKER);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|error| format!("{} has no readable owner marker ({error})", root.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("{} is not an owner marker file", path.display()));
    }
    if metadata.len() > MAX_MARKER_BYTES {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
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

fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("cannot sync {}: {error}", path.display()))
}

/// Whether Git would track a path inside the checkout. A checkout that is
/// not a work tree has no tracking risk.
fn git_would_track(checkout: &Path, relative: &str) -> Result<bool, String> {
    let status = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["check-ignore", "-q", "--", relative])
        .status();
    match status {
        Err(_) => Ok(false),
        Ok(status) => match status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => {
                let probe = Command::new("git")
                    .arg("-C")
                    .arg(checkout)
                    .args(["rev-parse", "--is-inside-work-tree"])
                    .output();
                match probe {
                    Ok(probe)
                        if probe.status.success()
                            && String::from_utf8_lossy(&probe.stdout).trim() == "true" =>
                    {
                        Err(
                            "git could not decide whether the local output root is ignored; refusing to create it"
                                .to_owned(),
                        )
                    }
                    Ok(_) => Ok(false),
                    Err(_) => Ok(false),
                }
            }
        },
    }
}

/// A path must be absolute and free of `..` for use in a receipt.
fn safe_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir | Component::CurDir))
}

/// Where `oxide clean` enumerates. Permanent owned outputs resolve through
/// the receipt to the selected real root; a clean never follows a link by
/// path text and never removes the link or the root's ownership marker.
pub struct CleanPlacement {
    target: PathBuf,
    keep_root: bool,
    link: PathBuf,
    inactive: Vec<PathBuf>,
}

impl CleanPlacement {
    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn keep_root(&self) -> bool {
        self.keep_root
    }

    /// What the clean keeps, for its report; empty for an ephemeral checkout.
    pub fn preserved_note(&self) -> Option<String> {
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

/// Resolve this checkout's clean target. `None` keeps today's ephemeral
/// behaviour; an owned checkout resolves to its selected real root and
/// refuses when the link and receipt disagree.
pub fn clean_placement(workspace: &Path) -> Result<Option<CleanPlacement>, String> {
    let settings = store::settings()?;
    let control = Store::open_existing_with(&settings)?.map(|store| store.control().to_path_buf());
    let context = Context::new(control, workspace.to_path_buf());
    let Some(receipt) = context.receipt()? else {
        return Ok(None);
    };
    match receipt.state {
        ReceiptState::Detached => Ok(None),
        ReceiptState::Prepared => Err(
            "clean refused: an interrupted owned-output transition is pending; run `oxide outputs adopt` to replay it before cleaning"
                .to_owned(),
        ),
        ReceiptState::Committed => {
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
            let marker = read_marker(&selected.root)?;
            check_marker(&marker, workspace, &context.checkout_id)?;
            if marker.kind != selected.kind || marker.volume_uuid != selected.volume_uuid {
                return Err(format!(
                    "clean refused: {} carries a marker for another root kind or volume",
                    selected.root.display()
                ));
            }
            if marker.generation > receipt.generation {
                return Err(format!(
                    "clean refused: {} carries generation {} ahead of the receipt's {}",
                    selected.root.display(),
                    marker.generation,
                    receipt.generation
                ));
            }
            let inactive = receipt
                .roots
                .iter()
                .filter(|root| root.root != selected.root && root.root.is_dir())
                .map(|root| root.root.clone())
                .collect();
            Ok(Some(CleanPlacement {
                target: selected.root,
                keep_root: true,
                link: context.link(),
                inactive,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        checkout: PathBuf,
        control: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("oxide-outputs-{}-{id}", std::process::id()));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let checkout = root.join("checkout");
            fs::create_dir(&checkout).unwrap();
            let control = root.join("control");
            fs::create_dir(&control).unwrap();
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                root,
                checkout,
                control,
            }
        }

        fn context(&self) -> Context {
            Context::new(Some(self.control.clone()), self.checkout.clone())
        }

        fn local(&self) -> RootSpec {
            local_spec(&self.checkout)
        }

        fn receipt(&self) -> Receipt {
            Receipt::first(
                &self.checkout,
                &identity::digest(&self.checkout.to_string_lossy()),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn receipts_reject_unknown_versions_fields_and_generations() {
        let fixture = Fixture::new();
        let mut receipt = fixture.receipt();
        receipt.state = ReceiptState::Committed;
        receipt.generation = 1;
        receipt.selected = Some(fixture.local().selected(1));
        receipt.roots = vec![fixture.local().selected(1)];
        let id = fixture.context().checkout_id.clone();
        write_receipt(&fixture.control, &receipt).unwrap();
        assert_eq!(
            read_receipt(&fixture.control, &fixture.checkout, &id)
                .unwrap()
                .unwrap(),
            receipt
        );

        let path = receipt_path(&fixture.control, &id);
        let mut unknown = serde_json::to_value(&receipt).unwrap();
        unknown["version"] = serde_json::Value::String("oxide.outputs.transition/9".to_owned());
        fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        let refused = read_receipt(&fixture.control, &fixture.checkout, &id).unwrap_err();
        assert!(refused.contains("unsupported version"), "{refused}");

        let mut extra = serde_json::to_value(&receipt).unwrap();
        extra["surprise"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&extra).unwrap()).unwrap();
        assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

        let mut ahead = receipt.clone();
        ahead.roots = vec![fixture.local().selected(3)];
        ahead.selected = None;
        ahead.state = ReceiptState::Detached;
        fs::write(&path, serde_json::to_vec(&ahead).unwrap()).unwrap();
        assert!(read_receipt(&fixture.control, &fixture.checkout, &id).is_err());

        let other = Fixture::new();
        let refused = read_receipt(&fixture.control, &other.checkout, &id).unwrap_err();
        assert!(refused.contains("another checkout"), "{refused}");
        let other_id = identity::digest(&other.checkout.to_string_lossy());
        assert!(read_receipt(&fixture.control, &other.checkout, &other_id)
            .unwrap()
            .is_none());
    }

    #[test]
    fn an_empty_root_gets_a_marker_and_a_nonempty_foreign_root_is_left_alone() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        assert!(spec.root().join(MARKER).is_file());
        let marker = read_marker(spec.root()).unwrap();
        assert_eq!(marker.kind, RootKind::Local);
        assert_eq!(marker.generation, 1);
        // Reusing the root keeps the marker and its creation generation.
        assert_eq!(ensure_root(&context, &spec, 4).unwrap().generation, 1);

        let other = Fixture::new();
        let foreign = other.local();
        fs::create_dir_all(foreign.root()).unwrap();
        fs::write(foreign.root().join("keep"), b"content").unwrap();
        let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
        assert!(refused.contains("holds content"), "{refused}");
        assert_eq!(fs::read(foreign.root().join("keep")).unwrap(), b"content");
    }

    #[test]
    fn a_marker_for_another_checkout_is_not_owned() {
        let fixture = Fixture::new();
        let context = fixture.context();
        ensure_root(&context, &fixture.local(), 1).unwrap();
        let other = Fixture::new();
        let foreign = other.local();
        fs::create_dir_all(foreign.root()).unwrap();
        fs::write(
            foreign.root().join(MARKER),
            serde_json::to_vec(&Marker {
                version: MARKER_VERSION.to_owned(),
                checkout: fixture.checkout.clone(),
                checkout_id: context.checkout_id.clone(),
                label: "x".to_owned(),
                recipe: RECIPE.to_owned(),
                kind: RootKind::Local,
                volume_uuid: None,
                generation: 1,
                created_ms: 1,
            })
            .unwrap(),
        )
        .unwrap();
        let refused = ensure_root(&other.context(), &foreign, 1).unwrap_err();
        assert!(refused.contains("not this checkout"), "{refused}");
    }

    #[test]
    fn a_transition_publishes_an_owned_link_and_keeps_the_root() {
        let fixture = Fixture::new();
        let receipt = transition(&fixture.context(), &fixture.receipt(), &fixture.local()).unwrap();
        let link = fixture.checkout.join("target");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), fixture.local().root());
        assert_eq!(
            fixture.context().target().unwrap(),
            TargetState::OwnedLink(fixture.local().root().to_path_buf())
        );
        assert_eq!(receipt.state, ReceiptState::Committed);
        assert_eq!(receipt.generation, 1);
        assert_eq!(receipt.roots.len(), 1);
        // A second transition to the same place is a new generation, not a
        // new root.
        let second = transition(&fixture.context(), &receipt, &fixture.local()).unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(second.roots.len(), 1);
        assert!(!sibling_link(&link, 2).exists());
        assert!(fixture.local().root().join(MARKER).is_file());
    }

    #[test]
    fn a_nonempty_real_target_and_a_foreign_link_are_never_replaced() {
        let fixture = Fixture::new();
        let target = fixture.checkout.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), b"content").unwrap();
        let refused =
            transition(&fixture.context(), &fixture.receipt(), &fixture.local()).unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read(target.join("keep")).unwrap(), b"content");
        assert!(fs::symlink_metadata(&target).unwrap().is_dir());
        assert!(!fixture.local().root().exists());

        let linked = Fixture::new();
        let outside = linked.root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"keep").unwrap();
        symlink(&outside, linked.checkout.join("target")).unwrap();
        let refused =
            transition(&linked.context(), &linked.receipt(), &linked.local()).unwrap_err();
        assert!(refused.contains("never moves"), "{refused}");
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"keep");
        assert!(!linked.local().root().exists());
        assert_eq!(
            fs::read_link(linked.checkout.join("target")).unwrap(),
            outside
        );
    }

    #[test]
    fn an_empty_real_target_is_converted_explicitly() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.checkout.join("target")).unwrap();
        let receipt = transition(&fixture.context(), &fixture.receipt(), &fixture.local()).unwrap();
        assert_eq!(receipt.generation, 1);
        assert!(fs::symlink_metadata(fixture.checkout.join("target"))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn a_prepared_transition_with_a_live_sibling_is_retried() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let prepared = Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.prepared = Some(prepared.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&prepared.to.root, sibling_link(&prepared.link, 1)).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
        assert_eq!(replayed.generation, 1);
        assert_eq!(fs::read_link(context.link()).unwrap(), prepared.to.root);
        assert!(!sibling_link(&prepared.link, 1).exists());
    }

    #[test]
    fn a_prepared_transition_after_the_rename_commits() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let prepared = Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        };
        receipt.state = ReceiptState::Prepared;
        receipt.prepared = Some(prepared.clone());
        write_receipt(&fixture.control, &receipt).unwrap();
        symlink(&prepared.to.root, &prepared.link).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Committed);
        assert_eq!(replayed.selected().unwrap().root, prepared.to.root);
    }

    #[test]
    fn a_prepared_transition_with_nothing_published_rolls_back() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        receipt.state = ReceiptState::Prepared;
        receipt.prepared = Some(Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let replayed = replay(&context, receipt).unwrap();
        assert_eq!(replayed.state, ReceiptState::Detached);
        assert_eq!(replayed.generation, 0);
        assert_eq!(replayed.roots.len(), 1);
        assert!(!context.link().exists());
        assert!(spec.root().is_dir());
    }

    #[test]
    fn a_contradictory_prepared_transition_stops_and_keeps_both_roots() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let mut receipt = fixture.receipt();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        receipt.state = ReceiptState::Prepared;
        receipt.prepared = Some(Prepared {
            generation: 1,
            to: spec.selected(1),
            link: context.link(),
        });
        write_receipt(&fixture.control, &receipt).unwrap();
        let outside = fixture.root.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, context.link()).unwrap();
        let refused = replay(&context, receipt).unwrap_err();
        assert!(refused.contains("cannot be replayed"), "{refused}");
        assert!(fs::symlink_metadata(context.link())
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(spec.root().is_dir());
        let loaded = read_receipt(&fixture.control, &fixture.checkout, &context.checkout_id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, ReceiptState::Prepared);
    }

    #[test]
    fn a_root_with_a_wrong_volume_or_kind_is_not_owned() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 1).unwrap();
        let mut selected = spec.selected(1);
        selected.volume_uuid = Some("00000000-0000-0000-0000-000000000000".to_owned());
        let refused =
            validate_selected(&fixture.checkout, &context.checkout_id, &selected, None, 1)
                .unwrap_err();
        assert!(refused.contains("another root kind or volume"), "{refused}");
        let mut external = spec.selected(1);
        external.kind = RootKind::External;
        let refused =
            validate_selected(&fixture.checkout, &context.checkout_id, &external, None, 1)
                .unwrap_err();
        let text = refused.to_string();
        assert!(
            text.contains("another root kind or volume") || text.contains("not usable"),
            "{text}"
        );
    }

    #[test]
    fn a_marker_generation_ahead_of_the_receipt_refuses() {
        let fixture = Fixture::new();
        let context = fixture.context();
        let spec = fixture.local();
        ensure_root(&context, &spec, 3).unwrap();
        let refused = validate_selected(
            &fixture.checkout,
            &context.checkout_id,
            &spec.selected(3),
            None,
            2,
        )
        .unwrap_err();
        assert!(refused.contains("ahead of the receipt"), "{refused}");
    }

    #[test]
    fn paths_in_receipts_are_absolute() {
        assert!(safe_absolute(Path::new("/work/app/target")));
        assert!(!safe_absolute(Path::new("target")));
        assert!(!safe_absolute(Path::new("/work/../target")));
    }
}
