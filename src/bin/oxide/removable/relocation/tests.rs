use super::*;
use barge::{Absent, AreaSpec, Class, Mode, Placement, VolumeRegistration};
use oxide_storage::lease::managed::init_budget_pool;
use oxide_storage::lease::{BUDGET_BACKEND_DIRECTORY_V1, VOLUME_BACKEND_APFS_V1};
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, DirBuilderExt};
use std::sync::Arc;

const PAYLOAD: &str = "builds/oxide/pool";
const OUTPUTS: &str = "builds/oxide/outputs";
const CAP: u64 = 128 << 20;

thread_local! {
    static RACING_OUTPUT_HOLD: std::cell::RefCell<Option<Box<dyn FnOnce() -> barge::Hold>>> =
        std::cell::RefCell::new(None);
}

pub(super) fn hold_before_outputs_move() -> Option<barge::Hold> {
    RACING_OUTPUT_HOLD.with(|hook| hook.borrow_mut().take().map(|hold| hold()))
}

struct Fixture {
    _temp: barge::testing::TempDir,
    internal: Store,
    removable: Store,
    mount: PathBuf,
    uuid: String,
    os: Arc<barge::FakeOs>,
    checkout: PathBuf,
    checkout_id: String,
    slots: Area,
    outputs: Area,
    slot: barge::Root,
    output: barge::Root,
    unused_output: barge::Root,
    host_layout: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let temp = barge::testing::TempDir::new("oxide-relocate").unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mount = root.join("drive");
        for dir in [
            "oxide-pool/slots",
            "oxide-pool/.oxide-holding",
            "oxide-outputs",
        ] {
            private_tree(&mount.join(dir));
        }
        // Feed the fake the fixture filesystem's UUID so the ordinary
        // directory-backed Barge reader can also preview the relocated area.
        let uuid = OxideOs.volume_identity(&mount).unwrap().unwrap();
        let os = Arc::new(barge::FakeOs::with_volume(&mount, &uuid, 100 << 30));
        let internal_root = root.join("internal");
        init_budget_pool(&internal_root, CAP).unwrap();
        let internal = Store::for_tests(&internal_root);
        let removable_root = internal.control().join(POOL_DIR);
        init_budget_pool(&removable_root, CAP).unwrap();
        fs::remove_dir(removable_root.join("payload/slots")).unwrap();
        fs::remove_dir(removable_root.join("payload")).unwrap();
        let mut config = read_json(&removable_root.join("config.json"));
        assert_eq!(config["backend"], BUDGET_BACKEND_DIRECTORY_V1);
        config["backend"] = json!(VOLUME_BACKEND_APFS_V1);
        config["volume"] = json!({
            "mount_point": mount, "volume_uuid": uuid, "payload_dir": "oxide-pool"
        });
        write_json(&removable_root, "config.json", &config);
        Store::for_tests(&removable_root);
        let removable = Store::open_recorded_removable(&removable_root, 2).unwrap();
        write_registration(
            internal.control(),
            &Registration {
                version: 3,
                mount_point: mount.clone(),
                volume_uuid: uuid.clone(),
                container_uuid: uuid.clone(),
                reserve_bytes: 0,
                cap_bytes: CAP,
                quota_bytes: None,
                capacity_bytes: None,
                registered_ms: 17,
                mode: super::super::DriveMode::Permanent,
                payload_dir: "oxide-pool".to_owned(),
                outputs_dir: "oxide-outputs".to_owned(),
            },
        )
        .unwrap();

        let host = Barge::open_with(
            root.join("host-account"),
            os.clone(),
            Arc::new(barge::ManualClock::new(1)),
        )
        .unwrap();
        register(&host, "Stash", &mount, &uuid);
        host.set_layout(
            Class::Builds,
            Placement {
                volume: "Stash".to_owned(),
                path: "builds".to_owned(),
                absent: Absent::Fail,
            },
        )
        .unwrap();
        let host_layout = fs::read(mount.join(".barge/layout.json")).unwrap();

        let slot_storage = Barge::open_with(
            removable.control().join("barge"),
            os.clone(),
            Arc::new(barge::ManualClock::new(1)),
        )
        .unwrap();
        register(&slot_storage, "pool", &mount, &uuid);
        let slots = slot_storage
            .area(
                AreaSpec::new(NAMESPACE, POOL_AREA, "pool")
                    .folder("oxide-pool/slots")
                    .soft_cap_bytes(Some(CAP))
                    .counts_toward_reserve(true),
            )
            .unwrap();
        let slot = slots.create_root("slot").unwrap();
        fs::write(slot.path.join("slot-sentinel"), "slot data").unwrap();

        let account = prune::output_account(internal.control(), &mount.join("oxide-outputs"));
        private_tree(&account);
        let output_storage = Barge::open_with(
            account.join("barge"),
            os.clone(),
            Arc::new(barge::ManualClock::new(1)),
        )
        .unwrap();
        register(&output_storage, "outputs", &mount, &uuid);
        let outputs = output_storage
            .area(
                AreaSpec::new(NAMESPACE, OUTPUTS_AREA, "outputs")
                    .folder("oxide-outputs")
                    .counts_toward_reserve(true),
            )
            .unwrap();
        let output = outputs.create_root("selected").unwrap();
        let unused_output = outputs.create_root("unused").unwrap();
        outputs.pin(&output.id, "fixture selected").unwrap();
        private_tree(&output.path.join("debug"));
        fs::write(output.path.join("debug/output-sentinel"), "output data").unwrap();
        let checkout = root.join("checkout");
        private_tree(&checkout.join("target"));
        symlink(output.path.join("debug"), checkout.join("target/debug")).unwrap();
        let mut ledger = read_json(&removable_root.join("ledger.json"));
        ledger["next_slot"] = json!(2);
        ledger["next_lease"] = json!(2);
        ledger["slots"] = json!([{
            "id": 1, "incarnation": 1, "state": "idle", "project": checkout,
            "workspace": checkout, "profile": "dev", "holder": "previous",
            "lease_id": null, "last_lease_id": "1", "dir": slot.path.file_name().unwrap().to_str().unwrap(),
            "root": slot.id, "owned": true,
        }]);
        write_json(&removable_root, "ledger.json", &ledger);
        let checkout_id = crate::identity::digest(checkout.to_str().unwrap());
        let receipt_name = format!("{checkout_id}.json");
        let receipt_dir = internal.control().join("outputs");
        private_tree(&receipt_dir);
        write_json(
            &output.path,
            crate::outputs::MARKER,
            &json!({
                "version": "oxide.outputs.root/2", "checkout": checkout, "checkout_id": checkout_id,
                "label": "checkout", "recipe": "target-debug-v1", "kind": "external",
                "volume_uuid": uuid, "nonce": "1".repeat(32), "generation": 1, "created_ms": 1
            }),
        );
        let selected = json!({
            "kind": "external", "root": output.path, "volume_uuid": uuid,
            "nonce": "1".repeat(32), "generation": 1
        });
        write_json(
            &receipt_dir,
            &receipt_name,
            &json!({
                "version": "oxide.outputs.transition/3", "checkout": checkout, "checkout_id": checkout_id,
                "generation": 1, "state": "committed", "layout": "profile",
                "links": [{"member": "debug", "selected": selected}], "roots": [selected], "updated_ms": 1
            }),
        );
        let row = |root: &barge::Root, selected: bool| {
            json!({
                "checkout_id": checkout_id, "checkout": checkout, "receipt": receipt_dir.join(&receipt_name),
                "member": "debug", "root": root.path, "barge_id": root.id, "root_kind": "external",
                "volume_uuid": uuid, "recipe": "target-debug-v1", "generation": 1,
                "selected": selected, "pending": false
            })
        };
        write_json(
            &receipt_dir,
            "registry.json",
            &json!({
                "version": "oxide.outputs.registry/2", "schema": 2, "revision": 1, "complete": true,
                "created_by": "fixture", "rows": [row(&output, true), row(&unused_output, false)]
            }),
        );
        write_json(
            removable.control(),
            "state.json",
            &json!({
                "version": 2, "next_id": 2, "next_use": 2, "tickets": [],
                "records": [{"id": 1, "identity": "fixture", "compatibility": "fixture", "profile": "debug",
                    "checkout": checkout, "phase": "idle", "process_group": null, "last_used": 1}]
            }),
        );
        let origins = removable.control().join("origins");
        private_tree(&origins);
        write_json(
            &origins,
            "1.json",
            &json!({"version": 1, "mark": 1, "origin": {"affine": checkout}}),
        );

        Self {
            _temp: temp,
            internal,
            removable,
            mount,
            uuid,
            os,
            checkout,
            checkout_id,
            slots,
            outputs,
            slot,
            output,
            unused_output,
            host_layout,
        }
    }

    fn media(&self) -> Media {
        Media::Fake {
            directory: File::open(&self.mount).unwrap(),
            mount: self.mount.clone(),
            os: self.os.clone(),
            uuid: self.uuid.clone(),
        }
    }

    fn run(&self, dry_run: bool) -> Result<Report, String> {
        relocate(
            &self.internal,
            &Options {
                payload: PAYLOAD,
                outputs: OUTPUTS,
                dry_run,
                json: false,
            },
            |_| Ok(self.media()),
        )
    }

    fn read_receipt(&self) -> Value {
        read_json(
            &self
                .internal
                .control()
                .join("outputs")
                .join(format!("{}.json", self.checkout_id)),
        )
    }

    fn assert_original(&self) {
        assert_eq!(
            fs::read(self.mount.join(".barge/layout.json")).unwrap(),
            self.host_layout
        );
        assert_eq!(
            read_registration(self.internal.control())
                .unwrap()
                .unwrap()
                .payload_dir(),
            "oxide-pool"
        );
        assert_eq!(
            fs::read_link(self.checkout.join("target/debug")).unwrap(),
            self.output.path.join("debug")
        );
        assert!(self.slot.path.exists());
        assert!(self.output.path.exists());
        assert!(!self.mount.join("builds").exists());
        assert!(!self.internal.control().join(PENDING).exists());
        assert_eq!(
            self.media()
                .account(self.removable.control(), true)
                .unwrap()
                .find_area("oxide/pool")
                .unwrap()
                .record()
                .effective_folder(),
            "oxide-pool/slots"
        );
    }
}

fn register(barge: &Barge, alias: &str, mount: &Path, uuid: &str) {
    barge
        .register_volume(
            VolumeRegistration::new(alias, mount)
                .uuid(Some(uuid))
                .root_dir(""),
        )
        .unwrap();
}

fn private_tree(path: &Path) {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .unwrap();
}

fn write_json(parent: &Path, name: &str, value: &Value) {
    store::atomic_write_private(parent, name, &serde_json::to_vec(value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn shared_readable_builds_parent_allows_nested_relocation() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let builds = fixture.mount.join("builds");
    fs::create_dir(&builds).unwrap();
    fs::set_permissions(&builds, PermissionsExt::from_mode(0o755)).unwrap();
    let report = fixture.run(false).unwrap();
    assert!(report.moved_outputs);
    assert!(report.cleanup.is_empty());
    assert!(fixture.mount.join(PAYLOAD).join(HOLDING).is_dir());
    assert_eq!(
        std::fs::metadata(fixture.mount.join(PAYLOAD))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let registration = read_registration(fixture.internal.control())
        .unwrap()
        .unwrap();
    assert_eq!(
        (registration.payload_dir(), registration.outputs_dir()),
        (PAYLOAD, OUTPUTS)
    );
}

#[test]
fn group_writable_builds_parent_refuses_nested_relocation() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let builds = fixture.mount.join("builds");
    fs::create_dir(&builds).unwrap();
    fs::set_permissions(&builds, PermissionsExt::from_mode(0o775)).unwrap();
    let error = fixture.run(false).unwrap_err();
    assert!(error.contains("not group- or world-writable"), "{error}");
    // Nothing moved: the original layout, links and registration are intact,
    // and no nested payload was created under the refused parent.
    assert_eq!(
        read_registration(fixture.internal.control())
            .unwrap()
            .unwrap()
            .payload_dir(),
        "oxide-pool"
    );
    assert!(fixture.slot.path.exists());
    assert!(fixture.output.path.exists());
    assert!(!fixture.mount.join(PAYLOAD).exists());
    assert!(!fixture.internal.control().join(PENDING).exists());
}

#[test]
fn legacy_layout_moves_both_areas_with_links_receipts_and_origins() {
    let fixture = Fixture::new();
    let preview = fixture.run(true).unwrap();
    assert!(preview.dry_run && preview.moved_outputs);
    assert_eq!(preview.output_links, 1);
    fixture.assert_original();

    let report = fixture.run(false).unwrap();
    assert!(report.moved_outputs);
    assert_eq!(report.invalidated_origins, 1);
    assert!(report.cleanup.is_empty());
    assert_eq!(
        fs::read(fixture.mount.join(".barge/layout.json")).unwrap(),
        fixture.host_layout
    );
    assert!(!fixture.mount.join("oxide-pool").exists());
    assert!(!fixture.mount.join("oxide-outputs").exists());
    assert!(fixture.mount.join(PAYLOAD).join(HOLDING).is_dir());

    let registration = read_registration(fixture.internal.control())
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            registration.version,
            registration.payload_dir(),
            registration.outputs_dir()
        ),
        (3, PAYLOAD, OUTPUTS)
    );
    let media = fixture.media();
    let slots = media
        .account(fixture.removable.control(), true)
        .unwrap()
        .find_area("oxide/pool")
        .unwrap();
    let output_account =
        prune::output_account(fixture.internal.control(), &fixture.mount.join(OUTPUTS));
    let outputs = media
        .account(&output_account, true)
        .unwrap()
        .find_area("oxide/outputs")
        .unwrap();
    assert_eq!(
        slots.record().effective_folder(),
        format!("{PAYLOAD}/slots")
    );
    assert_eq!(outputs.record().effective_folder(), OUTPUTS);
    let moved_slot = slots.resolve(&fixture.slot.id).unwrap().path;
    let moved_output = outputs.resolve(&fixture.output.id).unwrap().path;
    assert_eq!(
        fs::read(moved_slot.join("slot-sentinel")).unwrap(),
        b"slot data"
    );
    assert_eq!(
        fs::read(moved_output.join("debug/output-sentinel")).unwrap(),
        b"output data"
    );
    let output_link = fixture.checkout.join("target/debug");
    assert_eq!(
        fs::read_link(&output_link).unwrap(),
        moved_output.join("debug")
    );
    assert_eq!(
        fixture.read_receipt()["links"][0]["selected"]["root"],
        json!(moved_output)
    );
    assert_eq!(
        fixture.read_receipt()["roots"][0]["root"],
        json!(moved_output)
    );
    let registry = read_json(&fixture.internal.control().join("outputs/registry.json"));
    assert_eq!(registry["revision"], 2);
    assert_eq!(registry["rows"][0]["root"], json!(moved_output));
    assert_eq!(registry["rows"][0]["barge_id"], fixture.output.id);
    assert!(read_json(&fixture.removable.control().join("origins/1.json"))["origin"].is_null());

    let status = super::super::status(&fixture.internal, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        status.recorded.unwrap().payload,
        fixture.mount.join(PAYLOAD)
    );
    let described = crate::describe::describe_fake_volume(&fixture.internal.pool, |registration| {
        media.directory(registration.payload_dir()).map(|_| ())
    })
    .unwrap();
    let ledger = oxide_storage::lease::managed::snapshot(&fixture.removable.pool).unwrap();
    assert_eq!(
        ledger.slots[0].dir.as_deref(),
        moved_slot.file_name().and_then(|name| name.to_str())
    );
    assert_eq!(described["selected"], "removable");
    assert_eq!(
        described["removable"]["outputs"],
        json!(fixture.mount.join(OUTPUTS))
    );
    let planned = prune::planned_paths_for_tests(fixture.internal.control()).unwrap();
    assert_eq!(
        planned,
        [outputs.resolve(&fixture.unused_output.id).unwrap().path]
    );

    // A subsequent Cargo build uses the relocated slot and profile link.
    let slots = media
        .account(fixture.removable.control(), false)
        .unwrap()
        .find_area("oxide/pool")
        .unwrap();
    let _hold = slots.hold(&fixture.slot.id, Mode::Exclusive).unwrap();
    private_tree(&fixture.checkout.join("src"));
    fs::write(
        fixture.checkout.join("Cargo.toml"),
        "[package]\nname = \"relocated-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(fixture.checkout.join("src/main.rs"), "fn main() {}\n").unwrap();
    let built = std::process::Command::new(crate::CARGO)
        .args(["build", "--offline", "--quiet"])
        .current_dir(&fixture.checkout)
        .env("CARGO_BUILD_BUILD_DIR", &moved_slot)
        .env("CARGO_TARGET_DIR", fixture.checkout.join("target"))
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(moved_output.join("debug/relocated-fixture").is_file());
}

#[test]
fn invalid_and_overlapping_targets_refuse() {
    for (payload, outputs) in [("../pool", OUTPUTS), ("builds", OUTPUTS), ("a//b", OUTPUTS)] {
        assert!(Options::parse(&["--payload-dir", payload, "--outputs-dir", outputs]).is_err());
    }
    let fixture = Fixture::new();
    let options = Options {
        payload: "oxide-pool",
        outputs: OUTPUTS,
        dry_run: false,
        json: false,
    };
    assert!(
        relocate(&fixture.internal, &options, |_| Ok(fixture.media()))
            .unwrap_err()
            .contains("overlaps source")
    );
    fixture.assert_original();
}

#[test]
fn busy_or_unsafe_preconditions_leave_the_layout_unchanged() {
    let fixture = Fixture::new();
    fixture.os.set_uuid(&fixture.mount, None);
    assert!(fixture.run(false).unwrap_err().contains("absent"));
    fixture.os.set_uuid(&fixture.mount, Some(&fixture.uuid));

    let held = fixture
        .slots
        .hold(&fixture.slot.id, Mode::Exclusive)
        .unwrap();
    assert!(fixture.run(false).unwrap_err().contains("is held"));
    drop(held);
    let held = fixture
        .outputs
        .hold(&fixture.output.id, Mode::Shared)
        .unwrap();
    assert!(fixture.run(false).unwrap_err().contains("is held"));
    drop(held);

    let state_path = fixture.removable.control().join("state.json");
    let original = fs::read(&state_path).unwrap();
    let mut state = read_json(&state_path);
    state["records"][0]["phase"] = json!("acquiring");
    state["records"][0]["holder"] =
        json!({"pid": std::process::id(), "started_ms": 1, "lease_holder": "test"});
    write_json(fixture.removable.control(), "state.json", &state);
    assert!(fixture.run(false).unwrap_err().contains("active"));
    fs::write(&state_path, original).unwrap();

    let holding = fixture
        .mount
        .join("oxide-pool")
        .join(HOLDING)
        .join("pending");
    fs::write(&holding, b"keep").unwrap();
    assert!(fixture.run(false).unwrap_err().contains("non-empty"));
    assert_eq!(fs::read(holding).unwrap(), b"keep");
    fixture.assert_original();

    private_tree(&fixture.mount.join(OUTPUTS));
    assert!(fixture.run(false).unwrap_err().contains("already exists"));
    assert!(fixture.slot.path.exists());
    assert!(fixture.output.path.exists());
    assert!(!fixture.mount.join(PAYLOAD).exists());
}

#[test]
fn a_hold_appearing_on_outputs_rolls_back_the_first_move() {
    let fixture = Fixture::new();
    let (area, id) = (fixture.outputs.clone(), fixture.output.id.clone());
    RACING_OUTPUT_HOLD.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || area.hold(&id, Mode::Shared).unwrap()));
    });
    let error = fixture.run(false).unwrap_err();
    assert!(
        error.contains("is held") && error.contains("relocation rolled back"),
        "{error}"
    );
    fixture.assert_original();
    assert_eq!(
        fixture.read_receipt()["roots"][0]["root"],
        json!(fixture.output.path)
    );
    assert_eq!(
        read_json(&fixture.removable.control().join("origins/1.json"))["origin"],
        json!({"affine": fixture.checkout})
    );
}

#[test]
fn held_waiters_cleans_and_output_bindings_refuse_without_a_move() {
    let fixture = Fixture::new();
    let control = fixture.removable.control();
    let original = fs::read(control.join("state.json")).unwrap();
    let mut state = read_json(&control.join("state.json"));
    state["next_id"] = json!(3);
    let id = 2;
    let lock = store::private_file(&control.join(format!("waiters/{id}.lock")), true)
        .unwrap()
        .unwrap();
    assert!(store::flock_nonblocking(&lock).unwrap());

    state["tickets"] = json!([{"id": id, "pid": std::process::id()}]);
    write_json(control, "state.json", &state);
    assert!(fixture.run(false).unwrap_err().contains("waiter ticket"));
    state["tickets"] = json!([]);
    state["cleans"] = json!([{"id": id, "checkout": fixture.checkout}]);
    write_json(control, "state.json", &state);
    assert!(fixture.run(false).unwrap_err().contains("checkout clean"));
    drop(lock);
    fs::write(control.join("state.json"), original).unwrap();

    let binding = fixture.internal.lock_binding(&fixture.checkout).unwrap();
    assert!(fixture
        .run(false)
        .unwrap_err()
        .contains("checkout binding is held"));
    drop(binding);
    fixture.assert_original();
}

#[test]
fn a_foreign_or_missing_checkout_link_stays_untouched() {
    for missing in [false, true] {
        let fixture = Fixture::new();
        let link = fixture.checkout.join("target/debug");
        let foreign = fixture.checkout.join("foreign");
        if missing {
            fs::rename(
                &fixture.checkout,
                fixture.checkout.with_file_name("moved-checkout"),
            )
            .unwrap();
        } else {
            fs::remove_file(&link).unwrap();
            symlink(&foreign, &link).unwrap();
        }
        let report = fixture.run(false).unwrap();
        assert_eq!(report.untouched_links, [link.clone()]);
        assert_eq!(report.output_links, 0);
        if missing {
            let original_link = fixture
                .checkout
                .with_file_name("moved-checkout")
                .join("target/debug");
            assert_eq!(
                fs::read_link(original_link).unwrap(),
                fixture.output.path.join("debug")
            );
            assert!(!fixture.checkout.exists());
        } else {
            assert_eq!(fs::read_link(&link).unwrap(), foreign);
        }
        let selected = fixture
            .mount
            .join(OUTPUTS)
            .join(fixture.output.path.file_name().unwrap());
        assert_eq!(
            fixture.read_receipt()["links"][0]["selected"]["root"],
            json!(selected)
        );
    }
}

fn upgrade_fixture(
    fixture: &Fixture,
    dry_run: bool,
) -> Result<super::super::upgrade_legacy::Report, String> {
    super::super::upgrade_legacy::upgrade(&fixture.internal, dry_run, |_| Ok(fixture.media()))
}

fn legacy_output_fixture(fixture: &Fixture) {
    fixture.outputs.unpin(&fixture.output.id).unwrap();
    for root in [&fixture.output, &fixture.unused_output] {
        fs::remove_file(root.path.join(oxide_storage::storage::ROOT_MARKER)).unwrap();
    }
    fs::write(fixture.unused_output.path.join("stale-artifact"), b"dead").unwrap();
    let directory = fixture.internal.control().join("outputs");
    let mut registry = read_json(&directory.join("registry.json"));
    registry["rows"][1]["checkout"] = json!(fixture.checkout.with_file_name("deleted-checkout"));
    // One dead reference to a live root must not make the root deletable.
    let mut history = registry["rows"][0].clone();
    history["checkout"] = registry["rows"][1]["checkout"].clone();
    history["selected"] = json!(false);
    registry["rows"].as_array_mut().unwrap().push(history);
    for row in registry["rows"].as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("barge_id");
    }
    write_json(&directory, "registry.json", &registry);
}

fn upgrade_ready_fixture() -> Fixture {
    let fixture = Fixture::new();
    for pool in [&fixture.internal, &fixture.removable] {
        store::lock_admission(pool.control()).unwrap();
    }
    fixture
}

fn legacy_project(fixture: &Fixture, name: &str) -> Area {
    let folder = format!("oxide-pool/slots/{name}");
    private_tree(&fixture.mount.join(&folder).join(".barge/locks"));
    fixture
        .media()
        .account(fixture.removable.control(), false)
        .unwrap()
        .area(
            AreaSpec::new(NAMESPACE, format!("pool-{name}"), "pool")
                .folder(folder)
                .soft_cap_bytes(Some(CAP))
                .counts_toward_reserve(true),
        )
        .unwrap()
}

#[test]
fn upgrade_legacy_adopts_and_cleans_then_relocation_succeeds() {
    let fixture = upgrade_ready_fixture();
    legacy_output_fixture(&fixture);
    let slots = fixture.mount.join("oxide-pool/slots");
    let empty = slots.join("empty-project");
    private_tree(&empty);
    legacy_project(&fixture, "empty-area");
    private_tree(&slots.join("empty-area/.trash"));
    let legacy = legacy_project(&fixture, "legacy-project");
    let dead_slot = slots.join("legacy-project/quick-2");
    private_tree(&dead_slot);
    let adopted = legacy
        .adopt_root("quick-2", "quick", "legacy-slot-id")
        .unwrap();
    fs::write(dead_slot.join("stale-intermediate"), "dead slot").unwrap();
    let markerless_slot = slots.join("legacy-project/quick-3");
    private_tree(&markerless_slot);
    fs::write(
        markerless_slot.join("stale-intermediate"),
        "dead pre-Barge slot",
    )
    .unwrap();
    let trash = slots.join(".trash/.barge-claim-oxide-quick-0123456789abcdef.1");
    private_tree(&trash);
    symlink(&fixture.checkout, trash.join("never-follow")).unwrap();
    let outside = fixture.checkout.join("outside-sentinel");
    fs::write(&outside, "keep me").unwrap();

    let affinity = fixture.removable.control().join("affinity/17");
    private_tree(&affinity);
    let missing_workspace = fixture.checkout.with_file_name("deleted-slot-workspace");
    let mut ledger = read_json(&fixture.removable.pool.join("ledger.json"));
    ledger["next_slot"] = json!(4);
    let rows = ledger["slots"].as_array_mut().unwrap();
    let mut row = rows[0].clone();
    row["id"] = json!(2);
    row["project"] = json!(affinity);
    row["workspace"] = json!(missing_workspace);
    row["dir"] = json!("legacy-project/quick-2");
    row["root"] = json!(adopted.id);
    rows.push(row.clone());
    row["id"] = json!(3);
    row["dir"] = json!("legacy-project/quick-3");
    row.as_object_mut().unwrap().remove("root");
    row["owned"] = json!(false);
    rows.push(row);
    write_json(&fixture.removable.pool, "ledger.json", &ledger);
    let state_path = fixture.removable.control().join("state.json");
    let mut state = read_json(&state_path);
    let mut record = state["records"][0].clone();
    record["id"] = json!(17);
    record["checkout"] = json!(missing_workspace);
    state["next_id"] = json!(18);
    state["records"].as_array_mut().unwrap().push(record);
    write_json(fixture.removable.control(), "state.json", &state);

    let registry_path = fixture.internal.control().join("outputs/registry.json");
    let registry_before = fs::read(&registry_path).unwrap();
    let state_before = fs::read(&state_path).unwrap();
    let ledger_before = fs::read(fixture.removable.pool.join("ledger.json")).unwrap();
    let preview = upgrade_fixture(&fixture, true).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.adopt_outputs.paths, [fixture.output.path.clone()]);
    assert_eq!(
        preview.remove_dead_outputs.paths,
        [fixture.unused_output.path.clone()]
    );
    assert_eq!(preview.remove_empty_projects.paths, [empty.clone()]);
    assert_eq!(
        preview.remove_legacy_areas.paths,
        [slots.join("empty-area"), slots.join("legacy-project")]
    );
    assert_eq!(
        preview.retire_dead_slots.paths,
        [dead_slot.clone(), markerless_slot.clone()]
    );
    assert_eq!(preview.remove_markerless_trash.paths, [trash.clone()]);
    assert_eq!(fs::read(&registry_path).unwrap(), registry_before);
    assert_eq!(fs::read(&state_path).unwrap(), state_before);
    assert_eq!(
        fs::read(fixture.removable.pool.join("ledger.json")).unwrap(),
        ledger_before
    );
    assert!(!fixture
        .output
        .path
        .join(oxide_storage::storage::ROOT_MARKER)
        .exists());
    assert!(trash.is_dir() && dead_slot.is_dir() && empty.is_dir());
    assert!(fixture
        .run(true)
        .unwrap_err()
        .contains("cannot establish custody"));

    let report = upgrade_fixture(&fixture, false).unwrap();
    assert!(!report.dry_run);
    let mut preview_json = serde_json::to_value(&preview).unwrap();
    preview_json["dry_run"] = json!(false);
    assert_eq!(serde_json::to_value(&report).unwrap(), preview_json);
    assert!(fixture
        .output
        .path
        .join(oxide_storage::storage::ROOT_MARKER)
        .is_file());
    assert_eq!(
        fs::read(fixture.output.path.join("debug/output-sentinel")).unwrap(),
        b"output data"
    );
    let registry = read_json(&registry_path);
    let remaining = registry["rows"].as_array().unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(remaining
        .iter()
        .all(|row| row["root"] == json!(fixture.output.path)));
    let imported = fixture.outputs.list_without_bytes().unwrap().roots;
    assert_eq!(imported.len(), 1);
    assert!(imported[0].pinned);
    assert_eq!(remaining[0]["barge_id"], imported[0].id);
    assert_eq!(
        read_json(&state_path)["records"].as_array().unwrap().len(),
        1
    );
    let ledger = read_json(&fixture.removable.pool.join("ledger.json"));
    assert_eq!(ledger["slots"].as_array().unwrap().len(), 1);
    assert_eq!(ledger["slots"][0]["id"], 1);
    assert_eq!(fs::read(outside).unwrap(), b"keep me");
    for removed in [
        empty,
        slots.join("empty-area"),
        slots.join("legacy-project"),
        dead_slot,
        markerless_slot,
        trash,
        fixture.unused_output.path.clone(),
        affinity,
    ] {
        assert!(!removed.exists(), "{} still exists", removed.display());
    }
    let declarations = fixture.removable.control().join("barge/areas");
    for name in [
        "oxide.pool-empty-area.json",
        "oxide.pool-legacy-project.json",
    ] {
        assert!(!declarations.join(name).exists());
    }
    assert!(fixture
        .slots
        .list_without_bytes()
        .unwrap()
        .stragglers
        .is_empty());
    assert!(fixture
        .outputs
        .list_without_bytes()
        .unwrap()
        .stragglers
        .is_empty());
    let retry = upgrade_fixture(&fixture, false).unwrap();
    assert_eq!(
        retry.adopt_outputs.count + retry.remove_dead_outputs.count + retry.retire_dead_slots.count,
        0
    );

    assert!(fixture.run(false).unwrap().moved_outputs);
    let moved = fixture.mount.join(OUTPUTS).join(&fixture.output.folder);
    assert_eq!(
        fs::read(moved.join("debug/output-sentinel")).unwrap(),
        b"output data"
    );
    assert_eq!(
        fs::read_link(fixture.checkout.join("target/debug")).unwrap(),
        moved.join("debug")
    );
}

#[test]
fn upgrade_legacy_refuses_unknown_entries_before_any_payload_mutation() {
    let fixture = upgrade_ready_fixture();
    legacy_output_fixture(&fixture);
    let empty = fixture.mount.join("oxide-pool/slots/empty");
    private_tree(&empty);
    let unknown = fixture.mount.join("oxide-outputs/unregistered");
    private_tree(&unknown);
    assert!(upgrade_fixture(&fixture, false)
        .unwrap_err()
        .contains("unregistered"));
    assert!(unknown.is_dir() && empty.is_dir());

    fs::remove_dir(unknown).unwrap();
    let foreign = fixture.mount.join("oxide-pool/slots/foreign");
    private_tree(&foreign);
    fs::write(foreign.join("sentinel"), b"unowned").unwrap();
    assert!(upgrade_fixture(&fixture, false)
        .unwrap_err()
        .contains("foreign/sentinel"));
    assert!(empty.is_dir() && fixture.unused_output.path.is_dir());
    assert!(!fixture
        .output
        .path
        .join(oxide_storage::storage::ROOT_MARKER)
        .exists());
}

#[test]
fn upgrade_legacy_refuses_oxides_and_barges_active_writers() {
    let fixture = upgrade_ready_fixture();
    let control = fixture.removable.control();
    let mut state = read_json(&control.join("state.json"));
    state["records"][0]["phase"] = json!("running");
    state["records"][0]["process_group"] = json!(std::process::id());
    write_json(control, "state.json", &state);
    assert!(upgrade_fixture(&fixture, false)
        .unwrap_err()
        .contains("build record 1"));
    state["records"][0]["phase"] = json!("idle");
    state["records"][0]["process_group"] = json!(null);
    write_json(control, "state.json", &state);

    let hold = fixture
        .outputs
        .hold(&fixture.output.id, Mode::Shared)
        .unwrap();
    assert!(upgrade_fixture(&fixture, true)
        .unwrap_err()
        .contains("Barge root"));
    drop(hold);

    let hold = fixture
        .slots
        .hold(&fixture.slot.id, Mode::Exclusive)
        .unwrap();
    assert!(upgrade_fixture(&fixture, false)
        .unwrap_err()
        .contains("Barge root"));
    drop(hold);

    legacy_project(&fixture, "locked-area");
    let lock_path = fixture
        .mount
        .join("oxide-pool/slots/locked-area/.barge/locks/old.lock");
    let file = store::private_file(&lock_path, true).unwrap().unwrap();
    assert!(store::flock_nonblocking(&file).unwrap());
    assert!(upgrade_fixture(&fixture, false)
        .unwrap_err()
        .contains("old.lock"));
    assert!(lock_path.is_file());
}
