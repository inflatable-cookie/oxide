use super::adopt::*;
use super::bind::*;
use super::exec::*;
use super::git::*;
use super::receipt::*;
use super::root::*;
use super::transition::*;
use super::*;
use std::fs;
use std::os::unix::fs::{symlink, DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

mod adopt_tests;
mod auto_adopt_tests;
mod bind_tests;
mod exec_tests;
mod git_tests;
mod receipt_tests;
mod transition_tests;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    checkout: PathBuf,
    control: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::in_tree("", "/target\n/.oxide-outputs/\n")
    }

    /// A checkout `relative` inside its own Git work tree whose
    /// `.gitignore` holds exactly `ignore`. A first conversion needs Git to
    /// ignore the exact non-directory link path (oxide#125).
    fn in_tree(relative: &str, ignore: &str) -> Self {
        let mut fixture = Self::without_git();
        let repo = fixture.root.join("repo");
        fs::create_dir(&repo).unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg("-q")
            .current_dir(&repo)
            .status()
            .expect("git init in the fixture");
        assert!(status.success(), "git init failed: {status}");
        fs::write(repo.join(".gitignore"), ignore).unwrap();
        let checkout = if relative.is_empty() {
            repo
        } else {
            let checkout = repo.join(relative);
            fs::create_dir_all(&checkout).unwrap();
            checkout
        };
        fixture.checkout = checkout;
        fixture
    }

    /// A checkout that is not inside any Git work tree this fixture made.
    fn without_git() -> Self {
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
        // Receipt writes publish through the registry authority, so every
        // fixture starts with a ready, empty registry.
        crate::prune::seed_test_registry(&control).unwrap();
        Self {
            root,
            checkout,
            control,
        }
    }

    fn context(&self) -> Context {
        Context::new(Some(self.control.clone()), self.checkout.clone())
    }

    /// The Git work tree `in_tree` made for this fixture.
    fn repo(&self) -> PathBuf {
        self.root.join("repo")
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

/// A legacy whole-target plan around one selected root, for the binding
/// tests that predate profile members.
fn whole_target_plan(selected: Option<Selected>) -> Plan {
    Plan {
        whole_target: true,
        members: vec![PlannedMember {
            member: MEMBER.to_owned(),
            selected,
        }],
    }
}

/// A store whose control directory also carries a ready empty registry, so
/// tests can exercise the same receipt write-through as production.
fn pool_store(root: &Path) -> Store {
    let store = Store::for_tests(root);
    crate::prune::seed_test_registry(store.control()).unwrap();
    store
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn context_id(fixture: &Fixture) -> String {
    identity::digest(&fixture.checkout.to_string_lossy())
}

/// The whole-target receipt oxide#117 published, byte for byte: no
/// layout, placements, reservation, prune state or root nonce.
fn version_one_receipt(
    fixture: &Fixture,
    state: &str,
    selected: Option<&Path>,
) -> serde_json::Value {
    let mut document = serde_json::json!({
        "version": LEGACY_RECEIPT_VERSION,
        "checkout": fixture.checkout.to_string_lossy(),
        "checkout_id": context_id(fixture),
        "generation": 1,
        "state": state,
        "updated_ms": 1700000000000u64,
    });
    if let Some(root) = selected {
        let selection = serde_json::json!({
            "kind": "local",
            "root": root,
            "generation": 1,
        });
        document["selected"] = selection.clone();
        document["roots"] = serde_json::json!([selection]);
    }
    document
}

/// Publish a raw receipt document into a control directory.
fn write_receipt_document_at(fixture: &Fixture, control: &Path, document: &serde_json::Value) {
    let directory = control.join("outputs");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let path = receipt_path(control, &context_id(fixture));
    fs::write(&path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// Publish a raw receipt document into the fixture's own control directory.
fn write_receipt_document(fixture: &Fixture, document: &serde_json::Value) {
    write_receipt_document_at(fixture, &fixture.control, document);
}

/// A receipt's exact bytes and the metadata a read must never touch.
fn receipt_proof_at(control: &Path, checkout_id: &str) -> (Vec<u8>, u64, u64, u32, i64) {
    let path = receipt_path(control, checkout_id);
    let bytes = fs::read(&path).unwrap();
    let metadata = fs::symlink_metadata(&path).unwrap();
    (
        bytes,
        metadata.ino(),
        metadata.len(),
        metadata.mode(),
        metadata.mtime(),
    )
}

/// A receipt's exact bytes and metadata in the fixture's own control
/// directory.
fn receipt_proof(fixture: &Fixture) -> (Vec<u8>, u64, u64, u32, i64) {
    receipt_proof_at(&fixture.control, &context_id(fixture))
}

/// oxide#152: prune reads a valid version-1 whole-target receipt as the
/// current in-memory whole-target form, every other reader keeps refusing
/// it, and a read never rewrites a byte.

/// A committed profile receipt over the fixture's own local root: every
/// `placed` member is a live owned link into one root and every
/// `unselected` member is recorded without a selection (the state a
/// per-member history keeps after a partial detach).
fn placed_profile(
    fixture: &Fixture,
    placed: &[&str],
    unselected: &[&str],
) -> (Context, Receipt, PathBuf) {
    let context = fixture.context();
    let spec = fixture.local();
    ensure_root(&context, &spec, 1).unwrap();
    fs::create_dir_all(fixture.checkout.join("target")).unwrap();
    let selected = spec.selected(1);
    let mut receipt =
        Receipt::first_with(&fixture.checkout, &context.checkout_id, LayoutKind::Profile);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    for member in placed {
        receipt.place_member(member).selected = Some(selected.clone());
        let destination = member_destination(spec.root(), member);
        fs::create_dir_all(&destination).unwrap();
        let link = fixture.checkout.join("target").join(member);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&destination, &link).unwrap();
    }
    for member in unselected {
        receipt.place_member(member).selected = None;
    }
    write_receipt(&fixture.control, &receipt).unwrap();
    (context, receipt, spec.root().to_path_buf())
}

/// The derived `exec` member names of one fixture, in declaration order.
fn derived_exec(fixture: &Fixture) -> Vec<String> {
    exec_declaration(Some(&fixture.control), &fixture.checkout, None)
        .unwrap()
        .members()
        .iter()
        .map(|member| member.name().to_owned())
        .collect()
}

fn declaration_of(names: &[String]) -> crate::layout::Declaration {
    crate::layout::Declaration::new(
        names
            .iter()
            .map(|name| crate::layout::Member::parse(name).unwrap()),
    )
    .unwrap()
}

/// S1 counterfactual 1 + 10: the derived set is the receipt's placement,
/// rendered exactly as the equivalent complete declaration is, so the two
/// share one slot key.

fn profile_receipt(fixture: &Fixture) -> Receipt {
    let mut receipt = Receipt::first_with(
        &fixture.checkout,
        &identity::digest(&fixture.checkout.to_string_lossy()),
        LayoutKind::Profile,
    );
    let selected = fixture
        .local()
        .selected(1)
        .with_nonce(Some("22222222222222222222222222222222".to_owned()));
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.place_member("debug").selected = Some(selected.clone());
    receipt.place_member("release").selected = Some(selected);
    receipt
}

/// A profile receipt places members independently: a detach leaves every
/// member unselected, and the per-member transitions that re-adopt them
/// must be able to write a receipt where one member is committed while the
/// others are still unselected. The whole-target receipt keeps the strict
/// rule, and no profile receipt may select nothing at all.

/// A committed whole-target checkout in a Git work tree that ignores
/// `target/`, with a root that carries its marker.
fn legacy_checkout() -> (Fixture, Receipt) {
    let fixture = Fixture::in_tree("", "/target\n/.oxide-outputs/\n");
    let context = fixture.context();
    let receipt = transition(
        &context,
        &fixture.receipt(),
        &fixture.local(),
        Start::Any,
        NonceAuthority::Unkeyed,
    )
    .unwrap();
    (fixture, receipt)
}

/// Rewrite the root marker as a source-shaped root/1 document: no nonce,
/// no checkout volume, same owner, recipe, kind and generation.
fn make_root_one_marker(fixture: &Fixture, receipt: &Receipt) {
    let root = receipt.selected().unwrap().root();
    let marker = read_marker(root).unwrap();
    let legacy = Marker {
        version: LEGACY_MARKER_VERSION.to_owned(),
        checkout: marker.checkout,
        checkout_id: marker.checkout_id,
        label: marker.label,
        recipe: marker.recipe,
        kind: marker.kind,
        volume_uuid: marker.volume_uuid,
        nonce: None,
        checkout_volume_uuid: None,
        generation: marker.generation,
        created_ms: marker.created_ms,
    };
    fs::write(
        root.join(MARKER),
        serde_json::to_vec_pretty(&legacy).unwrap(),
    )
    .unwrap();
    let _ = fixture;
}

/// A fresh internal pool that has never adopted owned outputs: no registry
/// transaction is recorded and no registry document exists (oxide#217). It
/// is registered as the permanent drive so `adopt` proceeds, with an absent
/// mount point so the conversion selects the checkout-local root.
fn fresh_pool(fixture: &Fixture) -> (Store, Context) {
    let store = Store::for_tests(&fixture.root.join("pool"));
    register_at(fixture, store.control(), DriveMode::Permanent);
    let context = Context::new(
        Some(store.control().to_path_buf()),
        fixture.checkout.clone(),
    );
    assert!(!registry_document(&store).exists());
    assert!(store.outputs_initialization().unwrap().is_none());
    (store, context)
}

fn registry_document(store: &Store) -> PathBuf {
    store.control().join("outputs").join("registry.json")
}

fn committed_receipt(fixture: &Fixture) -> Receipt {
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(fixture.local().selected(1));
    receipt.roots = vec![fixture.local().selected(1)];
    receipt
}

/// Publish a receipt document as bytes: a pool whose registry is gone cannot
/// write one through the registry authority.
fn write_receipt_bytes(control: &Path, checkout_id: &str, receipt: &Receipt) {
    let directory = control.join("outputs");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let path = receipt_path(control, checkout_id);
    fs::write(&path, serde_json::to_vec_pretty(receipt).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// oxide#217: on a fresh pool the outputs registry does not exist yet, and
/// the first explicit adoption of a real nonempty target whose profile
/// members are both absent used to reach `transition_at` with no registry
/// and refuse. Every conversion path that can publish a transition has it
/// ready first, so the documented first adoption succeeds.

fn committed_whole_target_external(fixture: &Fixture) -> (Context, Selected) {
    let context = fixture.context();
    register(fixture, DriveMode::Permanent);
    let external = Selected {
        kind: RootKind::External,
        root: fixture.root.join("volume/oxide-outputs/checkout-0000"),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: Some("99999999999999999999999999999999".to_owned()),
        generation: 1,
    };
    let mut receipt = Receipt::first(&fixture.checkout, &context.checkout_id);
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(external.clone());
    write_receipt(&fixture.control, &receipt).unwrap();
    fs::create_dir_all(&external.root).unwrap();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::External,
        volume_uuid: external.volume_uuid.clone(),
        nonce: external.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::write(
        external.root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    let legacy_link = fixture.checkout.join("target");
    symlink(&external.root, &legacy_link).unwrap();
    (context, external)
}

/// oxide#164 review finding: an external subcommand derives an EMPTY set in
/// a whole-target checkout, and the shipped whole-target plan there can
/// name a FUTURE root (the first local fallback to an absent root,
/// oxide#117). The reservation must therefore still happen, or binding

fn os_symlink_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Exact `git status` entries naming `relative`, read NUL delimited with
/// every untracked file listed: a wholly untracked workspace would
/// otherwise collapse to one entry such as `apps/` and hide an exposed
/// `apps/farmyard/target`. Paths are compared exactly, never by suffix.
fn status_entries_naming(checkout: &Path, relative: &str) -> Vec<String> {
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .current_dir(checkout)
        .output()
        .expect("git status in the fixture");
    assert!(output.status.success(), "git status: {output:?}");
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .filter(|entry| {
            entry.get(2..3) == Some(" ")
                && (entry.get(3..) == Some(relative) || entry.get(3..) == Some("target"))
        })
        .collect()
}

/// A prepared receipt replacing a committed external root rolls back when
/// its target is still the exact old link (the ordinary crash-before-work
/// case), and never touches a different link.

fn register(fixture: &Fixture, mode: DriveMode) {
    register_at(fixture, &fixture.control, mode);
}

fn register_at(fixture: &Fixture, control: &Path, mode: DriveMode) {
    let registration = removable::Registration {
        payload_dir: "oxide-pool".to_owned(),
        outputs_dir: "oxide-outputs".to_owned(),
        version: removable::REGISTRATION_VERSION,
        mount_point: fixture.root.join("absent-volume"),
        volume_uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
        container_uuid: "66666666-7777-8888-9999-000000000000".to_owned(),
        reserve_bytes: 1,
        cap_bytes: 1 << 30,
        quota_bytes: None,
        capacity_bytes: None,
        registered_ms: 1,
        mode,
    };
    let path = control.join("removable.json");
    fs::write(&path, serde_json::to_vec(&registration).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn committed_external(fixture: &Fixture) -> (Receipt, PathBuf) {
    let root = fixture.root.join("volume/oxide-outputs/checkout-0000");
    let mut receipt = fixture.receipt();
    receipt.state = ReceiptState::Committed;
    receipt.generation = 1;
    receipt.link_mut().selected = Some(Selected {
        kind: RootKind::External,
        root: root.clone(),
        volume_uuid: Some("11111111-2222-3333-4444-555555555555".to_owned()),
        nonce: None,
        generation: 1,
    });
    write_receipt(&fixture.control, &receipt).unwrap();
    (receipt, root)
}

/// Ephemeral mode never relinks, so a converted checkout plans the root
/// its committed link selects (here external), not the local fallback.

fn reservation_for(root: &Path, generation: u64) -> Reservation {
    Reservation {
        id: crate::prune::fresh_nonce().unwrap(),
        nonce: crate::prune::fresh_nonce().unwrap(),
        kind: RootKind::Local,
        root: root.to_path_buf(),
        volume_uuid: None,
        generation,
        created_ms: 1,
    }
}

/// A future root's nonce is reserved durably (receipt plus pending
/// registry row), the transition that creates the root uses exactly that
/// nonce and consumes the reservation, and cancellation runs through its
/// journal so a stale starter holding the old nonce refuses.

fn keyed_fixture(fixture: &Fixture) -> (Receipt, RootSpec) {
    let spec = fixture.local();
    let future = spec.root().to_path_buf();
    let mut receipt = fixture.receipt();
    publish_reservation(fixture, &mut receipt, &future);
    (receipt, spec)
}

/// Every regular file's relative path, bytes and mode under `root`, plus
/// every symlink's target: the authority oracle for a refused transition.
/// Bounded by the fixture's own tiny control tree.
fn authority_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>, u32)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let facts = fs::symlink_metadata(&path).unwrap();
            if facts.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if facts.file_type().is_symlink() {
                let target = fs::read_link(&path).unwrap();
                found.push((relative, target.as_os_str().as_encoded_bytes().to_vec(), 0));
            } else {
                let mode = facts.permissions().mode();
                found.push((relative, fs::read(&path).unwrap(), mode));
            }
        }
    }
    found.sort();
    found
}

/// Publish the fixture's reservation for `root` in its receipt.
fn publish_reservation(fixture: &Fixture, receipt: &mut Receipt, root: &Path) -> Reservation {
    let reservation = reservation_for(root, receipt.generation + 1);
    *receipt.reserved_mut() = Some(reservation.clone());
    write_receipt(&fixture.control, receipt).unwrap();
    crate::prune::validate_reservation(&fixture.control, &reservation).unwrap();
    reservation
}

/// A keyed future-root slot binds only the exact uncancelled reservation it
/// was keyed with (oxide#152). The positive arm: the surviving reservation's
/// nonce creates the root and the owned link, and the commit consumes it.
/// No Cargo process exists below this seam, so the whole authority surface a
/// lease would expose is the root, the receipt and the registry.

fn committed_local_profile(fixture: &Fixture, context: &Context) -> Receipt {
    let mut receipt = profile_receipt(fixture);
    receipt.reserved = None;
    let selected = receipt.selected().cloned().unwrap();
    let root = selected.root.clone();
    let marker = Marker {
        version: MARKER_VERSION.to_owned(),
        checkout: fixture.checkout.clone(),
        checkout_id: context.checkout_id.clone(),
        label: "checkout".to_owned(),
        recipe: RECIPE.to_owned(),
        kind: RootKind::Local,
        volume_uuid: None,
        nonce: selected.nonce.clone(),
        checkout_volume_uuid: None,
        generation: 1,
        created_ms: 1,
    };
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join(MARKER),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(context.link()).unwrap();
    for member in ["debug", "release"] {
        fs::create_dir_all(member_destination(&root, member)).unwrap();
        symlink(
            member_destination(&root, member),
            context.link().join(member),
        )
        .unwrap();
    }
    // Publish only after the owned root exists, as the real transition
    // does. This fixture then carries its Barge root ID and durable pin.
    write_receipt(&fixture.control, &receipt).unwrap();
    receipt
}

fn declared(members: &[&str]) -> crate::layout::Declaration {
    crate::layout::Declaration::new(
        members
            .iter()
            .map(|name| crate::layout::Member::parse(name).unwrap()),
    )
    .unwrap()
}

/// oxide#152 publication seam: a requested member whose owned link dangles
/// is a destination binding refuses, so the pre-lease reservation publishes
/// NOTHING — every control byte, the receipt bytes and the registry revision
/// stay identical, no root appears and no Cargo runs. The explicit
/// `oxide outputs adopt` the refusal names then recovers with a FRESH nonce
/// (superseding the old committed row through the ordinary rules) and the
/// next ordinary build binds the recovered root.

fn all_facts() -> AutoFacts {
    AutoFacts {
        selected_permanent: true,
        same_selection: true,
        external_usable: true,
        permanent: true,
        receipt_absent: true,
        target_absent: true,
    }
}

/// A fixture for the output verbs: a checkout and a pool whose control
/// directory is the one its receipts live in, so `bind_for_command`'s own
/// context finds them. The registry is ready, as every receipt write
/// requires.
struct OutputFixture {
    root: PathBuf,
    checkout_root: PathBuf,
    checkout: PathBuf,
    store: Store,
}

impl OutputFixture {
    /// Separate checkout and payload paths under this run's TMPDIR.
    fn new(label: &str) -> Self {
        let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("oxide-guard-{label}-{}-{id}", std::process::id());
        let root = std::env::temp_dir().canonicalize().unwrap().join(name);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let checkout_root = root.join("temporary");
        fs::create_dir(&checkout_root).unwrap();
        fs::set_permissions(&checkout_root, fs::Permissions::from_mode(0o700)).unwrap();
        let checkout = checkout_root.join("checkout");
        fs::create_dir(&checkout).unwrap();
        let store = pool_store(&root);
        Self {
            root,
            checkout_root,
            checkout,
            store,
        }
    }

    fn control(&self) -> PathBuf {
        self.store.owner_control().to_path_buf()
    }

    fn context(&self) -> Context {
        Context::new(Some(self.control()), self.checkout.clone())
    }

    fn checkout_id(&self) -> String {
        identity::digest(&self.checkout.to_string_lossy())
    }

    fn write(&self, receipt: &Receipt) {
        write_receipt(&self.control(), receipt).unwrap();
    }

    /// A committed profile receipt whose members the live links would have
    /// to select. The guard refuses before it reads them.
    fn committed(&self) -> Receipt {
        let mut receipt =
            Receipt::first_with(&self.checkout, &self.checkout_id(), LayoutKind::Profile);
        let selected = local_spec(&self.checkout).selected(1);
        receipt.generation = 1;
        receipt.state = ReceiptState::Committed;
        receipt.links = vec![LinkPlacement {
            member: "debug".to_owned(),
            selected: Some(selected.clone()),
            prepared: None,
        }];
        receipt.roots = vec![selected];
        receipt
    }

    /// A `Prepared` receipt in the shipped write-ahead shape: exactly one
    /// placement carries a prepared transition, at the next generation.
    fn prepared(&self) -> Receipt {
        let mut receipt = self.committed();
        let to = local_spec(&self.checkout).selected(2);
        let link = link_path(&self.checkout, "debug");
        receipt.state = ReceiptState::Prepared;
        receipt.links[0].prepared = Some(Prepared {
            generation: receipt.generation + 1,
            to,
            link,
        });
        receipt
    }

    fn state_bytes(&self) -> Option<Vec<u8>> {
        fs::read(self.control().join("state.json")).ok()
    }
}

impl Drop for OutputFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.checkout_root);
    }
}

/// A bounded durable digest of one fixture's authority state: the control
/// state bytes, every receipt file, and the checkout's `target` tree with
/// each entry's kind and (for a link) its exact text. Small trees only;
/// never a payload walk.
fn durable_digest(fixture: &OutputFixture) -> Vec<String> {
    let mut entries = Vec::new();
    let state = fixture.control().join("state.json");
    if let Ok(bytes) = fs::read(&state) {
        entries.push(format!("state:{}", file_digest(&state).unwrap()));
        assert!(!String::from_utf8_lossy(&bytes).contains("\"class\""));
    }
    let outputs = fixture.control().join("outputs");
    if let Ok(listing) = fs::read_dir(&outputs) {
        let mut names: Vec<String> = listing
            .filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned()))
            .collect();
        names.sort();
        for name in names {
            entries.push(format!(
                "receipt {name}:{}",
                file_digest(&outputs.join(&name)).unwrap()
            ));
        }
    }
    let mut pending = vec![fixture.checkout.join("target")];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let shown = path
            .strip_prefix(&fixture.checkout)
            .unwrap_or(&path)
            .display()
            .to_string();
        if metadata.file_type().is_symlink() {
            let text = fs::read_link(&path).unwrap();
            entries.push(format!("link {shown} -> {}", text.display()));
        } else if metadata.is_dir() {
            entries.push(format!("dir {shown}"));
            if let Ok(listing) = fs::read_dir(&path) {
                for entry in listing.flatten() {
                    pending.push(entry.path());
                }
            }
        } else {
            entries.push(format!("file {shown}"));
        }
    }
    entries.sort();
    entries
}

/// The pool state's `next_id`, read from its own document (a durable fact
/// the barriered verbs advance one clean mark at a time).
fn next_id(fixture: &OutputFixture) -> Option<u64> {
    let bytes = fixture.state_bytes()?;
    let text = String::from_utf8(bytes).ok()?;
    text.split("\"next_id\":")
        .nth(1)?
        .split([',', '}'])
        .next()?
        .parse()
        .ok()
}

/// Every path and metadata a tree holds, for the read-only assertions.
fn tree_snapshot(root: &Path) -> Vec<(PathBuf, bool, u64, i64, i64)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let is_directory = metadata.is_dir();
            entries.push((
                path.clone(),
                is_directory,
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
            ));
            if is_directory {
                pending.push(path);
            }
        }
    }
    entries.sort();
    entries
}

/// Register a permanent drive at a real, present fixture directory and
/// declare the `oxide/pool` area in its Barge account, so the read-only
/// plan reaches the account branch instead of the identity seam.
fn register_live_volume(fixture: &Fixture) -> (PathBuf, PathBuf) {
    let mount = fixture.root.join("live-volume");
    fs::create_dir(&mount).unwrap();
    let uuid = {
        use barge::Os;
        oxide_storage::storage::OxideOs
            .volume_identity(&mount)
            .unwrap()
            .expect("the fixture directory is on a volume with an identity")
    };
    let registration = removable::Registration {
        payload_dir: "oxide-pool".to_owned(),
        outputs_dir: "oxide-outputs".to_owned(),
        version: removable::REGISTRATION_VERSION,
        mount_point: mount.clone(),
        volume_uuid: uuid.clone(),
        container_uuid: "66666666-7777-8888-9999-000000000000".to_owned(),
        reserve_bytes: 1,
        cap_bytes: 1 << 30,
        quota_bytes: None,
        capacity_bytes: None,
        registered_ms: 1,
        mode: DriveMode::Permanent,
    };
    let path = fixture.control.join("removable.json");
    fs::write(&path, serde_json::to_vec(&registration).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let state = removable::removable_control(&fixture.control);
    // Barge creates missing state directories under the umask. Oxide's control
    // checks need them owner-only whatever the host umask is.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&state)
        .unwrap();
    let storage = oxide_storage::storage::Storage::open(&state).unwrap();
    storage
        .declare(
            &oxide_storage::storage::AreaConfig::new("pool", &mount, "pool")
                .uuid(Some(uuid))
                .folder("oxide-pool/slots")
                .reserve_bytes(1)
                .floor_bytes(oxide_storage::storage::reserve_floor(1)),
        )
        .unwrap();
    (mount, state)
}
