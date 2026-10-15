use super::*;
use crate::registered::runner_host::{
    AttemptInput, HostError, HostResult, OwnerReceiptVerifier, RunnerHost,
};
use crate::storage::{
    AcquireRequest, Budget, Client, ContainmentEvidence, Credential, Daemon, DaemonConfig,
    EvidenceAuthority, FreeSpaceProbe, PartitionKey, RunEnvelope, SettlementEvidence, StorageError,
};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);
#[cfg(feature = "private-qualification")]
mod private_qualification;
#[cfg(feature = "private-qualification")]
mod private_qualification_runtime;

#[cfg(feature = "private-qualification")]
fn evidence_budget(maximum: usize) -> EvidenceReadBudget {
    EvidenceReadBudget::new(maximum, Limits::NUCLEUS)
}

#[cfg(feature = "private-qualification")]
fn budget_used(budget: &EvidenceReadBudget) -> usize {
    budget.used
}

#[cfg(feature = "private-qualification")]
fn charge_budget(budget: &mut EvidenceReadBudget, bytes: usize) -> Result<()> {
    budget.decoded(bytes)
}
struct Fixture {
    base: PathBuf,
    registration: Registration,
    attempt: Attempt,
    owners: Owners,
}
#[derive(Clone, Copy)]
enum Mode {
    Pass,
    Fail,
    Partial,
    CaptureFailure,
    LostReply,
    NoSpawn,
    ForgedNoSpawn,
    MutateSource,
    MutateTool,
    IdleFailure,
}
struct Owners {
    manifests: BTreeMap<String, ImmutableManifest>,
    seals: Mutex<BTreeSet<String>>,
    mode: Mutex<Mode>,
    calls: AtomicUsize,
    unknown_unit: bool,
    opaque: bool,
    missing_issuer: bool,
    mutable_precalculated: bool,
    extra_env: bool,
    cargo_context: Mutex<Option<cargo_freshness::TrustedCargoGraphContext>>,
}
fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|s| (*s).to_owned()).collect()
}
fn source_tree(root: &Path, files: &BTreeSet<String>) -> (String, BTreeMap<String, String>) {
    let mut hash = Sha256::new();
    let mut digests = BTreeMap::new();
    for name in files {
        let value = digest(&fs::read(root.join(name)).unwrap());
        frame(&mut hash, name.as_bytes());
        frame(&mut hash, value.as_bytes());
        digests.insert(name.clone(), value);
    }
    (format!("sha256:{:x}", hash.finalize()), digests)
}
impl Fixture {
    fn new() -> Self {
        let scratch = PathBuf::from(
            std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("canonical task-owned scratch"),
        );
        assert_eq!(fs::canonicalize(&scratch).unwrap(), scratch);
        let base = scratch.join(format!(
            "registered-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        for name in [
            "source",
            "generation",
            "target",
            "temp",
            "capture",
            "registry",
            "tools",
        ] {
            fs::create_dir(base.join(name)).unwrap();
        }
        fs::create_dir(base.join("source/data")).unwrap();
        for (name, bytes) in [
            ("src.rs", "source"),
            ("Cargo.toml", "manifest"),
            ("build-input", "build"),
            ("data/member", "data"),
        ] {
            fs::write(base.join("source").join(name), bytes).unwrap();
        }
        fs::write(base.join("registry/sqlite.c"), "native-source").unwrap();
        fs::write(base.join("registry/build.rs"), "audited-script").unwrap();
        fs::write(
            base.join("archive"),
            "synthetic archive, not a Cargo observation",
        )
        .unwrap();
        fs::write(
            base.join("tools/launcher with spaces"),
            "synthetic executable identity",
        )
        .unwrap();
        fs::write(base.join("tools/header.h"), "SDK header").unwrap();
        let files = set(&["Cargo.toml", "src.rs", "build-input", "data/member"]);
        let (tree, _) = source_tree(&base.join("source"), &files);
        let registration = Registration {
            version: 1,
            source_version: 1,
            dependency_version: 1,
            configuration_version: 1,
            id: "synthetic-audited-contract".into(),
            graph_digest: digest(b"known graph"),
            configuration_digest: digest(b"config"),
            toolchain_digest: digest(b"synthetic tool identity"),
            debug_policy_digest: digest(b"full independently owned capture"),
            argv: Argv {
                executable: base.join("tools/launcher with spaces"),
                arguments: vec![
                    "check:core:test".into(),
                    "--".into(),
                    "one argument with spaces".into(),
                    "".into(),
                    "literal;$()".into(),
                ],
                cwd: base.join("source"),
                environment: BTreeMap::from([("KNOWN".into(), "value with spaces".into())]),
            },
            source_files: files,
            immutable_receipts: set(&["registry-receipt", "tool-receipt"]),
            units: set(&["source-unit", "script-unit", "registry-unit"]),
            unit_environment: BTreeMap::from([
                (
                    "source-unit".into(),
                    BTreeMap::from([
                        ("ABSENT".into(), None),
                        ("EMPTY".into(), Some(String::new())),
                        ("SET".into(), Some("yes".into())),
                        ("KNOWN".into(), Some("value with spaces".into())),
                        ("CARGO_PKG_NAME".into(), Some("synthetic-core".into())),
                    ]),
                ),
                (
                    "script-unit".into(),
                    BTreeMap::from([
                        ("KNOWN".into(), Some("value with spaces".into())),
                        ("ABSENT".into(), None),
                    ]),
                ),
                ("registry-unit".into(), BTreeMap::new()),
            ]),
            tests: set(&["planning", "build_diagnostics"]),
            limits: Limits::NUCLEUS,
        };
        let attempt = Attempt {
            binding: RunBinding {
                consumer: "synthetic".into(),
                project: "owned".into(),
                component: "core".into(),
                workflow: registration.id.clone(),
                task_id: "task-1".into(),
                workspace_id: "workspace-1".into(),
                run_id: "run-1".into(),
                attempt_id: "attempt-1".into(),
                commit: "committed-1".into(),
                source_tree_digest: tree,
            },
            root_id: "root-1".into(),
            allocation_epoch: "epoch-1".into(),
            branch: "task-branch-1".into(),
            source_root: base.join("source"),
            slot: base.join("generation"),
            generation: "generation-1".into(),
            partition: digest(b"partition"),
            lease_id: "lease-1".into(),
            target: base.join("target"),
            temp: base.join("temp"),
            capture: base.join("capture"),
            authorization: "authenticated-attempt".into(),
            reused: false,
        };
        let mut manifests = BTreeMap::new();
        for (receipt, directory, class) in [
            ("registry-receipt", "registry", ImmutableClass::Registry),
            ("tool-receipt", "tools", ImmutableClass::ToolSdk),
        ] {
            let root = base.join(directory);
            let files = inventory(&root, Limits::NUCLEUS)
                .unwrap()
                .into_iter()
                .map(|name| {
                    let hash = digest(&fs::read(root.join(&name)).unwrap());
                    (name, hash)
                })
                .collect();
            manifests.insert(
                receipt.into(),
                ImmutableManifest {
                    version: 1,
                    issuer: "synthetic-owned-issuer".into(),
                    source_identity: format!("audited-{directory}"),
                    epoch: "immutable-epoch-1".into(),
                    class,
                    directories: BTreeSet::new(),
                    root,
                    files,
                    archive: if class == ImmutableClass::Registry {
                        Some(ArchiveEvidence {
                            path: base.join("archive"),
                            locked_checksum: digest(&fs::read(base.join("archive")).unwrap()),
                        })
                    } else {
                        None
                    },
                },
            );
        }
        let owners = Owners {
            manifests,
            seals: Mutex::new(BTreeSet::new()),
            mode: Mutex::new(Mode::Pass),
            calls: AtomicUsize::new(0),
            unknown_unit: false,
            opaque: false,
            missing_issuer: false,
            mutable_precalculated: false,
            extra_env: false,
            cargo_context: Mutex::new(None),
        };
        Self {
            base,
            registration,
            attempt,
            owners,
        }
    }
    fn run(&self) -> Result<Outcome> {
        Runner {
            registration_owner: &self.owners,
            input_owner: &self.owners,
            execution_owner: &self.owners,
        }
        .run(&self.registration, &self.attempt)
    }
    fn warm(&mut self) {
        assert_eq!(self.run(), Ok(Outcome::Published));
        self.attempt.reused = true;
        self.attempt.lease_id = "lease-2".into();
    }
    fn mode(&self, mode: Mode) {
        *self.owners.mode.lock().unwrap() = mode;
    }
    fn changed_source(&mut self, timestamp: SystemTime) {
        let path = self.attempt.source_root.join("src.rs");
        fs::write(&path, "changed-source").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        self.attempt.binding.source_tree_digest =
            source_tree(&self.attempt.source_root, &self.registration.source_files).0;
        self.attempt.binding.task_id = "task-2".into();
        self.attempt.binding.run_id = "run-2".into();
        self.attempt.binding.commit = "committed-2".into();
        self.attempt.allocation_epoch = "epoch-2".into();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}
impl RegistrationOwner for Owners {
    fn authenticate(
        &self,
        registration: &Registration,
        attempt: &Attempt,
    ) -> Result<SourceEvidence> {
        if attempt.authorization != "authenticated-attempt" {
            return Err(Refusal::Authentication);
        }
        if inventory(&attempt.source_root, registration.limits)? != registration.source_files {
            return Err(Refusal::Membership);
        }
        let (tree, files) = source_tree(&attempt.source_root, &registration.source_files);
        Ok(SourceEvidence {
            version: 1,
            commit: attempt.binding.commit.clone(),
            tree_digest: tree,
            clean_committed: true,
            submodules: false,
            lfs: false,
            files,
        })
    }
    fn audit(&self, registration: &Registration, _: &Attempt, _: &SourceEvidence) -> Result<()> {
        if registration.graph_digest != digest(b"known graph") {
            return Err(Refusal::Unsupported);
        }
        Ok(())
    }
}
fn source(path: &str) -> InputPath {
    InputPath {
        root: InputRoot::Source,
        path: path.into(),
    }
}

fn write_slot_file(slot: &Path, relative: &str, bytes: &[u8]) {
    let path = slot.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn write_cargo_slot_files(slot: &Path) {
    write_slot_file(
        slot,
        "debug/.fingerprint/source-hash/lib-source.json",
        include_bytes!("cargo_freshness/fixtures/check-dep-info-rerun-environment.json"),
    );
    write_slot_file(slot, "debug/deps/source.d", dep_info::UPSTREAM_V1);
    write_slot_file(slot, "generated.rs", b"generated source");
    write_slot_file(
        slot,
        "debug/.fingerprint/script-hash/run-build-script.json",
        include_bytes!("cargo_freshness/fixtures/rerun-files.json"),
    );
    write_slot_file(
        slot,
        "debug/.fingerprint/script-hash/output",
        b"build script output anchor",
    );
    write_slot_file(
        slot,
        "debug/.fingerprint/registry-hash/lib-registry.json",
        include_bytes!("cargo_freshness/fixtures/precalculated.json"),
    );
}

fn observed_association(
    input: InputPath,
    anchors: &[&str],
    producer: Option<&str>,
    native_receipt: Option<&str>,
) -> Association {
    Association {
        input,
        anchors: set(anchors),
        producer: producer.map(str::to_owned),
        native_receipt: native_receipt.map(str::to_owned),
    }
}

fn install_cargo_freshness(fixture: &Fixture) {
    use cargo_freshness::{CargoDependency, CargoUnitContext, TrustedCargoGraphContext};

    let slot = &fixture.attempt.slot;
    write_cargo_slot_files(slot);

    let dep_info_anchor = "debug/deps/source.d";
    let package_file = |reference_path: &str, input_path: &str| dep_info::RegisteredFile {
        reference: dep_info::FileReference {
            tag: dep_info::PathTag::PackageRootRelative,
            path: reference_path.into(),
        },
        input: source(input_path),
        association: observed_association(source(input_path), &[dep_info_anchor], None, None),
    };
    let generated_input = InputPath {
        root: InputRoot::Generated,
        path: "generated.rs".into(),
    };
    let source_unit = CargoUnitContext {
        package_root: fixture.attempt.source_root.clone(),
        package_origin: InputRoot::Source,
        fingerprint_directory: "debug/.fingerprint/source-hash".into(),
        fingerprint_file: "lib-source.json".into(),
        directory_entries: set(&["lib-source.json"]),
        dep_info: BTreeMap::from([(
            dep_info_anchor.into(),
            vec![
                package_file("src/lib.rs", "src.rs"),
                dep_info::RegisteredFile {
                    reference: dep_info::FileReference {
                        tag: dep_info::PathTag::BuildRootRelative,
                        path: "build/out.rs".into(),
                    },
                    input: generated_input.clone(),
                    association: observed_association(
                        generated_input,
                        &[dep_info_anchor],
                        Some("script-unit"),
                        None,
                    ),
                },
                package_file("Cargo.toml", "Cargo.toml"),
            ],
        )]),
        rerun_files: BTreeMap::new(),
        associations: Vec::new(),
        dependencies: vec![CargoDependency {
            package_id: 17,
            name: "script-unit".into(),
            public: false,
            unit: "script-unit".into(),
        }],
        effective_environment: fixture.registration.unit_environment["source-unit"].clone(),
        audited_precalculated_registry_receipt: None,
        additional_anchors: BTreeSet::new(),
    };

    let output_anchor = "debug/.fingerprint/script-hash/output";
    let script_unit = CargoUnitContext {
        package_root: fixture.attempt.source_root.clone(),
        package_origin: InputRoot::Source,
        fingerprint_directory: "debug/.fingerprint/script-hash".into(),
        fingerprint_file: "run-build-script.json".into(),
        directory_entries: set(&["run-build-script.json", "output"]),
        dep_info: BTreeMap::new(),
        rerun_files: BTreeMap::from([
            (
                "build-input".into(),
                observed_association(source("build-input"), &[output_anchor], None, None),
            ),
            (
                "data".into(),
                observed_association(source("data"), &[output_anchor], None, None),
            ),
        ]),
        associations: vec![
            observed_association(
                InputPath {
                    root: InputRoot::Immutable("registry-receipt".into()),
                    path: "sqlite.c".into(),
                },
                &[output_anchor],
                None,
                Some("tool-receipt"),
            ),
            observed_association(
                InputPath {
                    root: InputRoot::Immutable("tool-receipt".into()),
                    path: "header.h".into(),
                },
                &[output_anchor],
                None,
                Some("tool-receipt"),
            ),
        ],
        dependencies: Vec::new(),
        effective_environment: fixture.registration.unit_environment["script-unit"].clone(),
        audited_precalculated_registry_receipt: None,
        additional_anchors: BTreeSet::new(),
    };

    let registry = fixture.owners.manifests.get("registry-receipt").unwrap();
    let registry_unit = CargoUnitContext {
        package_root: registry.root.clone(),
        package_origin: InputRoot::Immutable("registry-receipt".into()),
        fingerprint_directory: "debug/.fingerprint/registry-hash".into(),
        fingerprint_file: "lib-registry.json".into(),
        directory_entries: set(&["lib-registry.json"]),
        dep_info: BTreeMap::new(),
        rerun_files: BTreeMap::new(),
        associations: Vec::new(),
        dependencies: Vec::new(),
        effective_environment: fixture.registration.unit_environment["registry-unit"].clone(),
        audited_precalculated_registry_receipt: Some("registry-receipt".into()),
        additional_anchors: BTreeSet::new(),
    };

    let context = TrustedCargoGraphContext {
        cargo_version: cargo_freshness::CARGO_VERSION.into(),
        cargo_source_commit: cargo_freshness::CARGO_SOURCE_COMMIT.into(),
        graph_digest: fixture.registration.graph_digest.clone(),
        build_root: slot.clone(),
        units: BTreeMap::from([
            ("source-unit".into(), source_unit),
            ("script-unit".into(), script_unit),
            ("registry-unit".into(), registry_unit),
        ]),
    };
    *fixture.owners.cargo_context.lock().unwrap() = Some(context);
}

fn collect_cargo_fixture(fixture: &Fixture) -> Result<Snapshot> {
    let runner = Runner {
        registration_owner: &fixture.owners,
        input_owner: &fixture.owners,
        execution_owner: &fixture.owners,
    };
    let inputs = runner.inputs(&fixture.registration, &fixture.attempt)?;
    runner.collect(&fixture.registration, &fixture.attempt, inputs)
}

#[test]
fn pinned_cargo_evidence_flows_through_registered_stale_gate() {
    let mut fixture = Fixture::new();
    install_cargo_freshness(&fixture);
    let runner = Runner {
        registration_owner: &fixture.owners,
        input_owner: &fixture.owners,
        execution_owner: &fixture.owners,
    };
    let mut producer_inputs = runner
        .inputs(&fixture.registration, &fixture.attempt)
        .unwrap();
    let cargo_context = fixture.owners.cargo_context.lock().unwrap();
    let unit_evidence = cargo_freshness::collect(
        &fixture.registration,
        &fixture.attempt,
        &producer_inputs.manifests,
        &mut producer_inputs.budget,
        cargo_context.as_ref().unwrap(),
    )
    .unwrap();
    let source_evidence = unit_evidence
        .iter()
        .find(|unit| unit.id == "source-unit")
        .unwrap();
    assert!(source_evidence.anchors.contains("debug/deps/source.d"));
    assert!(source_evidence
        .anchors
        .contains("debug/.fingerprint/source-hash/lib-source.json"));
    let LocalInput::DepInfo {
        unit,
        files,
        environment,
        ..
    } = &source_evidence.locals[0]
    else {
        panic!("pinned CheckDepInfo must become registered dep-info evidence")
    };
    assert_eq!(unit, "source-unit");
    assert_eq!(environment.get("ABSENT"), Some(&None));
    assert_eq!(environment.get("EMPTY"), Some(&Some(String::new())));
    assert!(files.contains(&InputPath {
        root: InputRoot::Generated,
        path: "generated.rs".into(),
    }));
    assert!(source_evidence.associations.iter().any(|association| {
        association.input.root == InputRoot::Generated
            && association.producer.as_deref() == Some("script-unit")
    }));
    assert!(source_evidence.locals.iter().any(|local| matches!(
        local,
        LocalInput::RerunEnvironment {
            name,
            value: None
        } if name == "ABSENT"
    )));
    assert!(source_evidence.locals.iter().any(|local| matches!(
        local,
        LocalInput::RerunEnvironment {
            name,
            value: Some(value)
        } if name == "EMPTY" && value.is_empty()
    )));
    let script_evidence = unit_evidence
        .iter()
        .find(|unit| unit.id == "script-unit")
        .unwrap();
    assert!(script_evidence.associations.iter().any(|association| {
        association.input.path == "header.h"
            && association.native_receipt.as_deref() == Some("tool-receipt")
    }));
    let registry_evidence = unit_evidence
        .iter()
        .find(|unit| unit.id == "registry-unit")
        .unwrap();
    assert!(matches!(
        &registry_evidence.locals[..],
        [LocalInput::Precalculated {
            registry_receipt,
            ..
        }] if registry_receipt == "registry-receipt"
    ));
    drop(cargo_context);

    let first = runner
        .collect(
            &fixture.registration,
            &fixture.attempt,
            runner
                .inputs(&fixture.registration, &fixture.attempt)
                .unwrap(),
        )
        .unwrap();

    let dep_info_anchor = "debug/deps/source.d";
    let anchor_mtime = first.anchors[dep_info_anchor].mtime;
    assert!(first.inputs.contains_key(&InputPath {
        root: InputRoot::Generated,
        path: "generated.rs".into(),
    }));
    assert!(first.inputs.contains_key(&InputPath {
        root: InputRoot::Immutable("registry-receipt".into()),
        path: "sqlite.c".into(),
    }));
    assert!(first.inputs.contains_key(&InputPath {
        root: InputRoot::Immutable("tool-receipt".into()),
        path: "header.h".into(),
    }));
    assert!(first.directories.contains_key(&source("data")));
    let semantics = &first.semantics;
    assert!(!semantics.is_empty());

    drop(runner);
    fixture.changed_source(SystemTime::UNIX_EPOCH + Duration::from_nanos(anchor_mtime));
    let runner = Runner {
        registration_owner: &fixture.owners,
        input_owner: &fixture.owners,
        execution_owner: &fixture.owners,
    };
    let current = runner
        .collect(
            &fixture.registration,
            &fixture.attempt,
            runner
                .inputs(&fixture.registration, &fixture.attempt)
                .unwrap(),
        )
        .unwrap();
    assert_eq!(admit(&first, &current), Err(Refusal::Stale));
}

#[test]
fn checksum_and_mutable_precalculated_fingerprints_refuse() {
    for (checksum, mutable_precalculated) in [(true, false), (false, true)] {
        let fixture = Fixture::new();
        install_cargo_freshness(&fixture);
        if checksum {
            let source_path = fixture
                .attempt
                .slot
                .join("debug/.fingerprint/source-hash/lib-source.json");
            let source_json = fs::read_to_string(&source_path).unwrap();
            fs::write(
                &source_path,
                source_json.replace("\"checksum\": false", "\"checksum\": true"),
            )
            .unwrap();
        }
        if mutable_precalculated {
            let mut context = fixture.owners.cargo_context.lock().unwrap();
            let registry = context
                .as_mut()
                .unwrap()
                .units
                .get_mut("registry-unit")
                .unwrap();
            registry.package_root = fixture.attempt.source_root.clone();
            registry.package_origin = InputRoot::Source;
        }
        let runner = Runner {
            registration_owner: &fixture.owners,
            input_owner: &fixture.owners,
            execution_owner: &fixture.owners,
        };
        let result = runner.collect(
            &fixture.registration,
            &fixture.attempt,
            runner
                .inputs(&fixture.registration, &fixture.attempt)
                .unwrap(),
        );
        assert_eq!(result, Err(Refusal::Unsupported));
    }
}

#[test]
fn cargo_collector_refuses_graph_mapping_and_inventory_gaps() {
    for scenario in 0..7 {
        let fixture = Fixture::new();
        install_cargo_freshness(&fixture);
        {
            let mut context = fixture.owners.cargo_context.lock().unwrap();
            let context = context.as_mut().unwrap();
            match scenario {
                0 => context.graph_digest = digest(b"different audited graph"),
                1 => context
                    .units
                    .get_mut("source-unit")
                    .unwrap()
                    .dep_info
                    .clear(),
                2 => context
                    .units
                    .get_mut("source-unit")
                    .unwrap()
                    .dependencies
                    .push(cargo_freshness::CargoDependency {
                        package_id: 18,
                        name: "extra-edge".into(),
                        public: false,
                        unit: "script-unit".into(),
                    }),
                3 => {
                    let script = context.units.get_mut("script-unit").unwrap();
                    script.associations.push(script.associations[0].clone());
                }
                4 => {
                    context
                        .units
                        .get_mut("script-unit")
                        .unwrap()
                        .directory_entries
                        .remove("output");
                }
                5 => {
                    context
                        .units
                        .get_mut("source-unit")
                        .unwrap()
                        .fingerprint_file = "../outside.json".into();
                }
                6 => {
                    let extra = context.units.remove("registry-unit").unwrap();
                    context.units.insert("extra-unit".into(), extra);
                }
                _ => unreachable!(),
            }
        }
        let expected = match scenario {
            0 => Refusal::Identity,
            1..=3 => Refusal::MissingAuthority,
            4 => Refusal::Membership,
            5 => Refusal::Unsupported,
            _ => Refusal::Identity,
        };
        assert_eq!(collect_cargo_fixture(&fixture), Err(expected));
    }
}
impl InputEvidenceOwner for Owners {
    fn immutable(&self, receipt: &str, _: &Attempt) -> Result<ImmutableManifest> {
        if self.missing_issuer {
            return Err(Refusal::MissingAuthority);
        }
        self.manifests
            .get(receipt)
            .cloned()
            .ok_or(Refusal::MissingAuthority)
    }
    fn units(&self, registration: &Registration, _: &Attempt) -> Result<Vec<UnitEvidence>> {
        // This literal fixture is the hand-annotated Cargo v1 vector in
        // dep_info.rs; the trusted synthetic owner supplies the exact mappings.
        let decoded = dep_info::decode(dep_info::UPSTREAM_V1)?;
        let package_input = source("src.rs");
        let generated_input = InputPath {
            root: InputRoot::Generated,
            path: "generated.rs".into(),
        };
        let cargo_files = [
            dep_info::RegisteredFile {
                reference: dep_info::FileReference {
                    tag: dep_info::PathTag::PackageRootRelative,
                    path: "src/lib.rs".into(),
                },
                input: package_input.clone(),
                association: Association {
                    input: package_input.clone(),
                    anchors: set(&["dep-info"]),
                    producer: None,
                    native_receipt: None,
                },
            },
            dep_info::RegisteredFile {
                reference: dep_info::FileReference {
                    tag: dep_info::PathTag::BuildRootRelative,
                    path: "build/out.rs".into(),
                },
                input: generated_input.clone(),
                association: Association {
                    input: generated_input,
                    anchors: set(&["dep-info"]),
                    producer: Some("script-unit".into()),
                    native_receipt: None,
                },
            },
            dep_info::RegisteredFile {
                reference: dep_info::FileReference {
                    tag: dep_info::PathTag::PackageRootRelative,
                    path: "Cargo.toml".into(),
                },
                input: source("Cargo.toml"),
                association: Association {
                    input: source("Cargo.toml"),
                    anchors: set(&["dep-info"]),
                    producer: None,
                    native_receipt: None,
                },
            },
        ];
        let decoded = decoded.into_registered(dep_info::TrustedContext {
            unit: "source-unit",
            anchor: "dep-info",
            registered_units: &registration.units,
            files: &cargo_files,
        })?;
        let mut source_associations = decoded.associations;
        source_associations.push(Association {
            input: source("src.rs"),
            anchors: set(&["consumer-two"]),
            producer: None,
            native_receipt: None,
        });
        let mut units = vec![
            UnitEvidence {
                id: "source-unit".into(),
                anchors: set(&["dep-info", "consumer-two"]),
                locals: vec![decoded.local],
                associations: source_associations,
            },
            UnitEvidence {
                id: "script-unit".into(),
                anchors: set(&["script-output"]),
                locals: vec![
                    LocalInput::RerunFiles {
                        output: "script-output".into(),
                        paths: vec![source("build-input"), source("data")],
                    },
                    LocalInput::RerunEnvironment {
                        name: "KNOWN".into(),
                        value: Some(
                            if self.extra_env {
                                "changed"
                            } else {
                                "value with spaces"
                            }
                            .into(),
                        ),
                    },
                    LocalInput::RerunEnvironment {
                        name: "ABSENT".into(),
                        value: None,
                    },
                ],
                associations: vec![Association {
                    input: InputPath {
                        root: InputRoot::Immutable("registry-receipt".into()),
                        path: "sqlite.c".into(),
                    },
                    anchors: set(&["script-output"]),
                    producer: None,
                    native_receipt: Some("tool-receipt".into()),
                }],
            },
            UnitEvidence {
                id: "registry-unit".into(),
                anchors: set(&["registry-output"]),
                locals: vec![LocalInput::Precalculated {
                    value: "immutable package identity".into(),
                    registry_receipt: if self.mutable_precalculated {
                        "source"
                    } else {
                        "registry-receipt"
                    }
                    .into(),
                }],
                associations: vec![],
            },
        ];
        if self.unknown_unit {
            units[0].id = "unknown-unit".into();
        }
        if self.opaque {
            units[0].locals.push(LocalInput::Unsupported);
        }
        Ok(units)
    }
    fn units_with_budget(
        &self,
        registration: &Registration,
        attempt: &Attempt,
        manifests: &BTreeMap<String, ImmutableManifest>,
        budget: &mut EvidenceReadBudget,
    ) -> Result<Vec<UnitEvidence>> {
        let context = self.cargo_context.lock().unwrap();
        if let Some(context) = context.as_ref() {
            cargo_freshness::collect(registration, attempt, manifests, budget, context)
        } else {
            self.units(registration, attempt)
        }
    }
    fn seal_map(&self, _: &Attempt, map_digest: &str) -> Result<String> {
        let seal = format!("synthetic-seal-{map_digest}");
        self.seals.lock().unwrap().insert(seal.clone());
        Ok(seal)
    }
    fn verify_map(&self, _: &Attempt, map_digest: &str, seal: &str) -> Result<()> {
        if seal != format!("synthetic-seal-{map_digest}")
            || !self.seals.lock().unwrap().contains(seal)
        {
            return Err(Refusal::Authentication);
        }
        Ok(())
    }
}
impl ExecutionOwner for Owners {
    fn execute(&self, launch: &Launch) -> Result<ExecutionReceipt> {
        assert_eq!(
            fs::read(launch.slot.join(PENDING)).unwrap(),
            launch.lease_id.as_bytes(),
            "durable barrier precedes trusted scheduler"
        );
        assert_eq!(
            launch
                .argv
                .environment
                .get("CARGO_BUILD_BUILD_DIR")
                .unwrap(),
            launch.slot.to_str().unwrap()
        );
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mode = *self.mode.lock().unwrap();
        if matches!(mode, Mode::LostReply) {
            return Err(Refusal::Completion);
        }
        if matches!(mode, Mode::NoSpawn) {
            return Ok(ExecutionReceipt::NotSpawned("authentic-no-spawn".into()));
        }
        if matches!(mode, Mode::ForgedNoSpawn) {
            return Ok(ExecutionReceipt::NotSpawned("forged".into()));
        }
        for name in [
            "dep-info",
            "consumer-two",
            "script-output",
            "registry-output",
            "generated.rs",
        ] {
            if name != "registry-output" || !launch.slot.join(name).exists() {
                fs::write(launch.slot.join(name), name).unwrap();
            }
        }
        if matches!(mode, Mode::MutateSource) {
            fs::write(launch.argv.cwd.join("src.rs"), "mutated-during-capture").unwrap();
        }
        if matches!(mode, Mode::MutateTool) {
            fs::write(&launch.argv.executable, "mutated tool").unwrap();
        }
        Ok(ExecutionReceipt::Completed(
            "synthetic-capture-receipt".into(),
        ))
    }
    fn verify_success(
        &self,
        _: &Registration,
        _: &Launch,
        receipt: &str,
        inputs: &CaptureInputs,
    ) -> Result<()> {
        assert_eq!(
            inputs.generated_files.get("generated.rs"),
            Some(&digest(b"generated.rs"))
        );
        assert!(inputs.immutable_identities.contains_key("registry-receipt"));
        if receipt != "synthetic-capture-receipt"
            || matches!(
                *self.mode.lock().unwrap(),
                Mode::Fail | Mode::Partial | Mode::CaptureFailure
            )
        {
            return Err(Refusal::Completion);
        }
        Ok(())
    }
    fn finalize_idle(&self, _: &Launch, _: &str) -> Result<()> {
        if matches!(*self.mode.lock().unwrap(), Mode::IdleFailure) {
            return Err(Refusal::Completion);
        }
        Ok(())
    }
    fn finalize_not_spawned(&self, _: &Launch, receipt: &str) -> Result<()> {
        if receipt != "authentic-no-spawn" {
            return Err(Refusal::Authentication);
        }
        Ok(())
    }
}

#[test]
fn audited_classes_and_changed_task_retain_every_dependency_reference() {
    let mut f = Fixture::new();
    f.warm();
    let old_registry = fs::read(f.attempt.slot.join("registry-output")).unwrap();
    let old_registry_time = fs::metadata(f.attempt.slot.join("registry-output"))
        .unwrap()
        .modified()
        .unwrap();
    let bytes = fs::read(f.attempt.slot.join(MAP)).unwrap();
    let (map, _) = decode(&bytes, f.registration.limits).unwrap();
    assert_eq!(
        map.inputs.get(&source("src.rs")).unwrap().anchors,
        set(&["consumer-two", "dep-info"])
    );
    assert!(map.inputs.contains_key(&InputPath {
        root: InputRoot::Generated,
        path: "generated.rs".into()
    }));
    assert!(map.inputs.contains_key(&InputPath {
        root: InputRoot::Immutable("registry-receipt".into()),
        path: "sqlite.c".into()
    }));
    f.changed_source(SystemTime::now() + Duration::from_secs(2));
    assert_eq!(f.run(), Ok(Outcome::Published));
    assert_eq!(
        fs::read(f.attempt.slot.join("registry-output")).unwrap(),
        old_registry
    );
    assert_eq!(f.owners.calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        fs::metadata(f.attempt.slot.join("registry-output"))
            .unwrap()
            .modified()
            .unwrap(),
        old_registry_time
    );
}

#[test]
fn old_and_equal_mtimes_refuse_before_spawn_and_preserve_map() {
    for equal in [false, true] {
        let mut f = Fixture::new();
        f.warm();
        let original = fs::read(f.attempt.slot.join(MAP)).unwrap();
        let anchor = fs::metadata(f.attempt.slot.join("consumer-two"))
            .unwrap()
            .modified()
            .unwrap();
        f.changed_source(if equal {
            anchor
        } else {
            UNIX_EPOCH + Duration::from_secs(1)
        });
        assert_eq!(f.run(), Err(Refusal::Stale));
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
        assert_eq!(fs::read(f.attempt.slot.join(MAP)).unwrap(), original);
        assert!(!f.attempt.slot.join(PENDING).exists());
    }
}

#[test]
fn immutable_archive_registry_and_tools_refuse_even_with_newer_times() {
    for name in [
        "archive",
        "registry/sqlite.c",
        "tools/header.h",
        "tools/launcher with spaces",
    ] {
        let mut f = Fixture::new();
        f.warm();
        let path = f.base.join(name);
        fs::write(&path, "changed").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(f.run(), Err(Refusal::ImmutableChanged));
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn directory_membership_environment_precalculated_and_unknown_inputs_refuse() {
    for case in 0..6 {
        let mut f = Fixture::new();
        f.warm();
        match case {
            0 => fs::write(f.attempt.source_root.join("data/added"), "new").unwrap(),
            1 => {
                fs::remove_file(f.attempt.source_root.join("data/member")).unwrap();
            }
            2 => f.owners.extra_env = true,
            3 => f.owners.mutable_precalculated = true,
            4 => f.owners.unknown_unit = true,
            _ => f.owners.opaque = true,
        }
        assert!(f.run().is_err(), "case {case}");
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn missing_authority_authentication_and_tampered_seals_refuse() {
    let mut f = Fixture::new();
    f.owners.missing_issuer = true;
    assert_eq!(f.run(), Err(Refusal::MissingAuthority));
    f.owners.missing_issuer = false;
    f.attempt.authorization = "forged".into();
    assert_eq!(f.run(), Err(Refusal::Authentication));
    f.attempt.authorization = "authenticated-attempt".into();
    f.warm();
    let bytes = fs::read(f.attempt.slot.join(MAP)).unwrap();
    let (mut map, seal) = decode(&bytes, f.registration.limits).unwrap();
    map.inputs.get_mut(&source("src.rs")).unwrap().file.digest = digest(b"forged input");
    fs::write(
        f.attempt.slot.join(MAP),
        encode(&map, &seal, f.registration.limits).unwrap(),
    )
    .unwrap();
    assert_eq!(f.run(), Err(Refusal::Authentication));
    assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn partial_failure_capture_loss_mutation_and_idle_failure_never_publish() {
    for mode in [
        Mode::Fail,
        Mode::Partial,
        Mode::CaptureFailure,
        Mode::LostReply,
        Mode::MutateSource,
        Mode::MutateTool,
        Mode::IdleFailure,
    ] {
        let mut f = Fixture::new();
        f.warm();
        f.mode(mode);
        assert!(f.run().is_err());
        assert!(f.attempt.slot.join(PENDING).exists());
        // Reconciliation alone supplies no publication operation.
        f.mode(Mode::Pass);
        assert!(f.run().is_err());
        assert!(f.attempt.slot.join(PENDING).exists());
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 2);
    }
}

#[test]
fn authentic_no_spawn_preserves_map_and_forgery_keeps_barrier() {
    let mut f = Fixture::new();
    f.warm();
    let old = fs::read(f.attempt.slot.join(MAP)).unwrap();
    f.mode(Mode::NoSpawn);
    assert_eq!(f.run(), Ok(Outcome::NotSpawned));
    assert_eq!(fs::read(f.attempt.slot.join(MAP)).unwrap(), old);
    assert!(!f.attempt.slot.join(PENDING).exists());
    f.mode(Mode::ForgedNoSpawn);
    assert_eq!(f.run(), Err(Refusal::Authentication));
    assert!(f.attempt.slot.join(PENDING).exists());
}

#[test]
fn literal_argv_paths_and_environment_are_not_split_or_inherited() {
    let f = Fixture::new();
    let command = f.registration.argv.command();
    assert_eq!(command.get_program(), f.registration.argv.executable);
    assert_eq!(
        command
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "check:core:test",
            "--",
            "one argument with spaces",
            "",
            "literal;$()"
        ]
    );
    assert_eq!(
        command.get_current_dir(),
        Some(f.attempt.source_root.as_path())
    );
    assert_eq!(command.get_envs().count(), 1);
}

#[test]
fn physical_paths_symlink_escape_and_graph_changes_refuse() {
    let mut f = Fixture::new();
    f.registration.graph_digest = digest(b"unknown graph");
    assert_eq!(f.run(), Err(Refusal::Unsupported));
    f.registration.graph_digest = digest(b"known graph");
    fs::remove_file(f.attempt.source_root.join("src.rs")).unwrap();
    // Both ends are exclusively owned scratch; never link a real checkout.
    symlink(
        f.base.join("tools/header.h"),
        f.attempt.source_root.join("src.rs"),
    )
    .unwrap();
    assert_eq!(f.run(), Err(Refusal::Unsupported));
    assert_eq!(f.owners.calls.load(Ordering::Relaxed), 0);
}

#[test]
fn finite_read_map_association_and_path_budgets_refuse() {
    for case in 0..5 {
        let mut f = Fixture::new();
        match case {
            0 => f.registration.limits.file_bytes = 1,
            1 => f.registration.limits.source_bytes = 1,
            2 => f.registration.limits.collection_bytes = 1,
            3 => f.registration.limits.associations = 2,
            _ => f.registration.limits.path_bytes = 1,
        }
        assert!(f.run().is_err());
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 0);
    }
    let mut f = Fixture::new();
    f.warm();
    f.registration.limits.map_bytes = 10;
    assert_eq!(f.run(), Err(Refusal::Budget));
    let mut f = Fixture::new();
    f.registration.limits.file_bytes = Limits::NUCLEUS.file_bytes + 1;
    assert_eq!(f.run(), Err(Refusal::Budget));
}

#[test]
fn read_accounting_deduplicates_without_dropping_consuming_anchors() {
    let f = Fixture::new();
    let path = f.attempt.source_root.join("src.rs");
    let mut budget = ReadBudget::new(6, Limits::NUCLEUS);
    let first = budget.file(&path).unwrap();
    assert_eq!(budget.used, 6);
    assert_eq!(budget.file(&path).unwrap(), first);
    assert_eq!(budget.used, 6, "one physical read for repeated references");
    assert_eq!(
        budget.file(&f.attempt.source_root.join("build-input")),
        Err(Refusal::Budget)
    );
    fs::write(&path, "different").unwrap();
    assert_eq!(budget.file(&path), Err(Refusal::Unstable));
}

#[test]
fn argv_boundaries_generation_and_physical_identity_cannot_replay_a_map() {
    for case in 0..4 {
        let mut f = Fixture::new();
        f.warm();
        match case {
            0 => f.registration.argv.arguments.push("extra-argument".into()),
            1 => f.attempt.generation = "generation-2".into(),
            2 => f.attempt.root_id = "root-2".into(),
            _ => f.attempt.partition = digest(b"other partition"),
        }
        assert_eq!(f.run(), Err(Refusal::Identity));
        assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
    }
    let mut f = Fixture::new();
    f.registration.argv.arguments = vec!["a".into(), "b".into()];
    f.registration.argv.environment = BTreeMap::from([("c".into(), "d".into())]);
    let first = identity(&f.registration, &f.attempt).unwrap();
    f.registration.argv.arguments.clear();
    f.registration.argv.environment =
        BTreeMap::from([("a".into(), "b".into()), ("c".into(), "d".into())]);
    assert_ne!(identity(&f.registration, &f.attempt).unwrap(), first);
}

#[test]
fn added_empty_directories_and_identical_byte_replacement_refuse() {
    let mut f = Fixture::new();
    f.warm();
    fs::create_dir(f.attempt.source_root.join("data/empty")).unwrap();
    assert_eq!(f.run(), Err(Refusal::Membership));
    let mut f = Fixture::new();
    f.warm();
    fs::create_dir(f.base.join("tools/empty")).unwrap();
    assert_eq!(f.run(), Err(Refusal::Membership));
    let mut f = Fixture::new();
    f.warm();
    let path = f.base.join("registry/sqlite.c");
    let bytes = fs::read(&path).unwrap();
    fs::write(f.base.join("replacement"), bytes).unwrap();
    fs::rename(f.base.join("replacement"), path).unwrap();
    assert_eq!(f.run(), Err(Refusal::ImmutableChanged));
}

#[test]
fn cargo_generated_unit_environment_is_distinct_and_partition_bound() {
    let mut f = Fixture::new();
    f.warm();
    assert!(!f
        .registration
        .argv
        .environment
        .contains_key("CARGO_PKG_NAME"));
    f.registration
        .unit_environment
        .get_mut("source-unit")
        .unwrap()
        .insert("CARGO_PKG_NAME".into(), Some("other-package".into()));
    assert_eq!(f.run(), Err(Refusal::Identity));
    assert_eq!(f.owners.calls.load(Ordering::Relaxed), 1);
    let mut f = Fixture::new();
    f.warm();
    f.registration
        .unit_environment
        .get_mut("script-unit")
        .unwrap()
        .remove("ABSENT");
    assert_eq!(
        f.run(),
        Err(Refusal::Identity),
        "undeclared absence is not coverage"
    );
}

struct BridgeAuthority;

impl EvidenceAuthority for BridgeAuthority {
    fn verify_admission(&self, envelope: &RunEnvelope) -> bool {
        envelope.admission_reference == "admission-ref"
            && matches!(
                envelope.admission_credential.expose(),
                "admission-proof" | "reminted-admission-proof"
            )
    }

    fn verify_settlement(&self, evidence: &SettlementEvidence) -> bool {
        evidence.consumer_credential.expose() == "consumer-proof"
            && evidence.terminal.is_none()
            && evidence.capture.is_none()
            && matches!(&evidence.containment, ContainmentEvidence::NotSpawned(_))
    }
}

struct BridgeFreeSpace;

impl FreeSpaceProbe for BridgeFreeSpace {
    fn available_bytes(&self, _: &Path) -> std::result::Result<Option<u64>, StorageError> {
        Ok(Some(64 * 1024 * 1024 * 1024))
    }
}

struct BridgeDaemon {
    shutdown: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<std::result::Result<(), StorageError>>>,
}

impl BridgeDaemon {
    fn start(daemon: Daemon) -> (PathBuf, Self) {
        let socket = daemon.socket_path().to_path_buf();
        let (shutdown, receiver) = mpsc::channel();
        let thread = thread::spawn(move || daemon.serve(receiver));
        (
            socket,
            Self {
                shutdown: Some(shutdown),
                thread: Some(thread),
            },
        )
    }

    fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            assert!(thread.join().expect("bridge daemon thread joined").is_ok());
        }
    }
}

impl Drop for BridgeDaemon {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct BridgeVerifier;

impl OwnerReceiptVerifier for BridgeVerifier {
    fn verify(
        &self,
        owner: &str,
        callback_id: &str,
        subject_digest: &str,
        policy_digest: &str,
        result_digest: &str,
        receipt: &str,
    ) -> bool {
        receipt
            == bridge_receipt(
                owner,
                callback_id,
                subject_digest,
                policy_digest,
                result_digest,
            )
    }
}

fn bridge_receipt(
    owner: &str,
    callback_id: &str,
    subject_digest: &str,
    policy_digest: &str,
    result_digest: &str,
) -> String {
    format!("fixture-v1:{owner}:{callback_id}:{subject_digest}:{policy_digest}:{result_digest}")
}

fn bridge_parent(
    mut stream: std::os::unix::net::UnixStream,
    owners: &Owners,
    expected_partition: &PartitionKey,
    start: &serde_json::Value,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    use std::io::{BufRead, BufReader, Write};

    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut callbacks = Vec::new();
    let mut line = String::new();
    serde_json::to_writer(&mut stream, start).unwrap();
    stream.write_all(b"\n").unwrap();
    loop {
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert!(!line.is_empty(), "host closed before final response");
        let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
        if frame["operation"] == "result" || frame["operation"] == "error" {
            return (frame, callbacks);
        }
        assert_eq!(frame["operation"], "callback");
        assert_eq!(
            frame["partition"],
            serde_json::to_value(expected_partition).unwrap()
        );
        let id = frame["id"].as_str().unwrap().to_owned();
        assert_eq!(id.parse::<usize>().unwrap(), callbacks.len() + 1);
        let owner = frame["owner"].as_str().unwrap().to_owned();
        let payload = frame["payload"].clone();
        let result = match owner.as_str() {
            "registration.authenticate" => {
                let registration: Registration =
                    serde_json::from_value(payload["registration"].clone()).unwrap();
                let attempt: Attempt = serde_json::from_value(payload["attempt"].clone()).unwrap();
                assert_eq!(
                    attempt.partition,
                    digest(&serde_json::to_vec(expected_partition).unwrap())
                );
                serde_json::to_value(owners.authenticate(&registration, &attempt).unwrap()).unwrap()
            }
            "registration.audit" => {
                let registration: Registration =
                    serde_json::from_value(payload["registration"].clone()).unwrap();
                let attempt: Attempt = serde_json::from_value(payload["attempt"].clone()).unwrap();
                let source: SourceEvidence =
                    serde_json::from_value(payload["source"].clone()).unwrap();
                owners.audit(&registration, &attempt, &source).unwrap();
                serde_json::Value::Null
            }
            "input.immutable" => {
                let attempt: Attempt = serde_json::from_value(payload["attempt"].clone()).unwrap();
                let receipt = payload["receipt"].as_str().unwrap();
                serde_json::to_value(owners.immutable(receipt, &attempt).unwrap()).unwrap()
            }
            "input.units" => {
                let registration: Registration =
                    serde_json::from_value(payload["registration"].clone()).unwrap();
                let attempt: Attempt = serde_json::from_value(payload["attempt"].clone()).unwrap();
                serde_json::to_value(owners.units(&registration, &attempt).unwrap()).unwrap()
            }
            "execution.execute" => {
                let launch: Launch = serde_json::from_value(payload["launch"].clone()).unwrap();
                serde_json::to_value(owners.execute(&launch).unwrap()).unwrap()
            }
            "execution.finalize_not_spawned" => {
                let launch: Launch = serde_json::from_value(payload["launch"].clone()).unwrap();
                let receipt = payload["receipt"].as_str().unwrap();
                owners.finalize_not_spawned(&launch, receipt).unwrap();
                serde_json::json!({"consumer_credential":"consumer-proof"})
            }
            other => panic!("unexpected callback {other}"),
        };
        let result_digest = digest(&serde_json::to_vec(&result).unwrap());
        let subject_digest = frame["subject_digest"].as_str().unwrap();
        let policy_digest = frame["policy_digest"].as_str().unwrap();
        let receipt = bridge_receipt(&owner, &id, subject_digest, policy_digest, &result_digest);
        let response = serde_json::json!({
            "version": 1,
            "operation": "callback_result",
            "id": id,
            "owner": owner,
            "subject_digest": subject_digest,
            "policy_digest": policy_digest,
            "result_digest": result_digest,
            "receipt": receipt,
            "result": result
        });
        serde_json::to_writer(&mut stream, &response).unwrap();
        stream.write_all(b"\n").unwrap();
        callbacks.push(frame);
    }
}

fn has_token_field(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => object
            .iter()
            .any(|(key, value)| key == "token" || has_token_field(value)),
        serde_json::Value::Array(values) => values.iter().any(has_token_field),
        _ => false,
    }
}

#[test]
fn inactive_runner_host_uses_the_durable_generation_api_and_verified_parent_callbacks() {
    let mut fixture = Fixture::new();
    fixture.mode(Mode::NoSpawn);
    let storage_root = fixture.base.join("bridge-storage");
    fs::create_dir(&storage_root).unwrap();
    fs::set_permissions(&storage_root, fs::Permissions::from_mode(0o700)).unwrap();
    let ledger_path = storage_root.join("ledger.bin");
    let daemon = Daemon::bind(
        DaemonConfig {
            root: storage_root,
            budget: Budget {
                cap_bytes: 1024 * 1024,
                free_reserve_bytes: 0,
            },
            backing: None,
        },
        Arc::new(BridgeAuthority),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let (socket, daemon_guard) = BridgeDaemon::start(daemon);

    let partition = PartitionKey {
        project_component: "owned:core".into(),
        workflow: fixture.registration.id.clone(),
        cargo_provenance: "cargo-synthetic".into(),
        rustc_provenance: "rustc-synthetic".into(),
        host: "aarch64-apple-darwin".into(),
        target: "aarch64-apple-darwin".into(),
        profile: "full-debug".into(),
        features: vec!["default".into()],
        config_digest: digest(b"partition config"),
        environment_digest: digest(b"partition env"),
        path_identity: digest(b"physical path"),
    };
    fixture.attempt.partition = digest(&serde_json::to_vec(&partition).unwrap());
    fixture.attempt.allocation_epoch = "1".into();
    let envelope = RunEnvelope {
        binding: fixture.attempt.binding.clone(),
        partition: partition.clone(),
        admission_reference: "admission-ref".into(),
        admission_credential: Credential::new("admission-proof"),
    };
    let acquire_request = AcquireRequest {
        envelope: envelope.clone(),
        acquire_id: "host-acquire-1".into(),
        required_bytes: 64,
    };
    let storage_client = Client::new(socket.clone());
    let original_grant = storage_client
        .acquire(acquire_request.clone())
        .expect("synthetic parent admitted the exact original request");
    assert_eq!(
        storage_client
            .acquire(acquire_request.clone())
            .expect("matching acquire replay returns the original lease"),
        original_grant
    );
    let mut reminted_request = acquire_request.clone();
    reminted_request.envelope.admission_credential = Credential::new("reminted-admission-proof");
    assert_eq!(
        storage_client.acquire(reminted_request),
        Err(StorageError::Conflict),
        "a valid replacement secret changes the exact persisted request body"
    );
    let mut missing_secret_request = acquire_request.clone();
    missing_secret_request.envelope.admission_credential =
        Credential::new("missing-original-secret");
    assert_eq!(
        storage_client.acquire(missing_secret_request),
        Err(StorageError::Unauthorized),
        "missing original credential refuses instead of being re-minted"
    );
    let input = AttemptInput {
        binding: fixture.attempt.binding.clone(),
        root_id: fixture.attempt.root_id.clone(),
        allocation_epoch: fixture.attempt.allocation_epoch.clone(),
        branch: fixture.attempt.branch.clone(),
        source_root: fixture.attempt.source_root.clone(),
        partition: fixture.attempt.partition.clone(),
        target: fixture.attempt.target.clone(),
        temp: fixture.attempt.temp.clone(),
        capture: fixture.attempt.capture.clone(),
        authorization: fixture.attempt.authorization.clone(),
    };
    let request = serde_json::json!({
        "version": 1,
        "operation": "run",
        "id": "host-run-1",
        "payload": {
            "registration": &fixture.registration,
            "attempt": input,
            "acquire": {
                "envelope": &acquire_request.envelope,
                "acquire_id": &acquire_request.acquire_id,
                "required_bytes": acquire_request.required_bytes
            },
            "finalize_id": "host-finalize-1"
        }
    });
    for replacement in ["reminted-admission-proof", "missing-original-secret"] {
        let mut refused_request = request.clone();
        refused_request["payload"]["acquire"]["envelope"]["admission_credential"] =
            serde_json::Value::String(replacement.to_owned());
        let (host_stream, parent_stream) = std::os::unix::net::UnixStream::pair().unwrap();
        let refused_host = RunnerHost::new(socket.clone(), Arc::new(BridgeVerifier));
        let refused_thread = thread::spawn(move || refused_host.serve(host_stream));
        let (response, callbacks) =
            bridge_parent(parent_stream, &fixture.owners, &partition, &refused_request);
        assert_eq!(response["operation"], "error");
        assert_eq!(response["code"], "storage_failure");
        assert!(callbacks.is_empty(), "refused request cannot reach Runner");
        assert_eq!(
            refused_thread.join().unwrap().unwrap_err(),
            HostError::Storage
        );
    }
    let (host_stream, parent_stream) = std::os::unix::net::UnixStream::pair().unwrap();
    let host = RunnerHost::new(socket, Arc::new(BridgeVerifier));
    let host_thread = thread::spawn(move || host.serve(host_stream));
    let (response, callbacks) = bridge_parent(parent_stream, &fixture.owners, &partition, &request);
    let result: HostResult = serde_json::from_value(response["result"].clone()).unwrap();
    let runner_result = host_thread.join().unwrap().unwrap();
    daemon_guard.stop();

    assert!(matches!(
        runner_result.status,
        super::runner_host::HostStatus::Finished
    ));
    assert_eq!(
        result.outcome,
        Some(super::runner_host::OutcomeName::NotSpawned)
    );
    let storage = result.storage.unwrap();
    assert_eq!(storage.generation, "1");
    assert_eq!(storage.reused, Some(false));
    assert_eq!(storage.storage_instance_id.len(), 64);
    assert!(result.token_digest.unwrap().starts_with("sha256:"));
    assert!(!has_token_field(&request));
    assert_eq!(callbacks.len(), 6);
    assert!(callbacks.iter().all(|frame| !has_token_field(frame)));
    assert!(callbacks.iter().all(|frame| {
        frame["storage"]["slot_id"] == storage.slot_id
            && frame["storage"]["storage_instance_id"] == storage.storage_instance_id
            && frame["storage"]["generation"] == storage.generation
    }));
    assert!(callbacks.iter().all(|frame| {
        let subject = serde_json::json!({
            "id": frame["id"],
            "owner": frame["owner"],
            "storage": frame["storage"],
            "partition": frame["partition"],
            "payload": frame["payload"]
        });
        frame["subject_digest"] == digest(&serde_json::to_vec(&subject).unwrap())
    }));
    assert_eq!(fixture.owners.calls.load(Ordering::Relaxed), 1);
    let ledger = fs::read(ledger_path).expect("read synthetic storage ledger");
    for secret in ["admission-proof", "reminted-admission-proof"] {
        assert!(!ledger
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
    }
    assert!(!fixture
        .base
        .join("bridge-storage")
        .join("slots/slot-0000000000000001/PENDING")
        .exists());
}

#[test]
fn capacity_pending_returns_before_registered_runner_or_parent_callbacks() {
    let mut fixture = Fixture::new();
    let storage_root = fixture.base.join("pending-storage");
    fs::create_dir(&storage_root).unwrap();
    fs::set_permissions(&storage_root, fs::Permissions::from_mode(0o700)).unwrap();
    let daemon = Daemon::bind(
        DaemonConfig {
            root: storage_root,
            budget: Budget {
                cap_bytes: 64,
                free_reserve_bytes: 0,
            },
            backing: None,
        },
        Arc::new(BridgeAuthority),
        Arc::new(BridgeFreeSpace),
    )
    .unwrap();
    let (socket, daemon_guard) = BridgeDaemon::start(daemon);
    let client = Client::new(socket.clone());
    let partition = PartitionKey {
        project_component: "owned:core".into(),
        workflow: fixture.registration.id.clone(),
        cargo_provenance: "cargo-synthetic".into(),
        rustc_provenance: "rustc-synthetic".into(),
        host: "aarch64-apple-darwin".into(),
        target: "aarch64-apple-darwin".into(),
        profile: "full-debug".into(),
        features: vec!["default".into()],
        config_digest: digest(b"partition config"),
        environment_digest: digest(b"partition env"),
        path_identity: digest(b"physical path"),
    };
    fixture.attempt.partition = digest(&serde_json::to_vec(&partition).unwrap());
    let first_request = AcquireRequest {
        envelope: RunEnvelope {
            binding: fixture.attempt.binding.clone(),
            partition: partition.clone(),
            admission_reference: "admission-ref".into(),
            admission_credential: Credential::new("admission-proof"),
        },
        acquire_id: "occupying-acquire".into(),
        required_bytes: 64,
    };
    let occupied = client.acquire(first_request).unwrap();
    assert_eq!(occupied.generation, "1");

    let mut binding = fixture.attempt.binding.clone();
    binding.task_id = "next-task".into();
    binding.run_id = "next-run".into();
    binding.attempt_id = "next-attempt".into();
    let attempt = AttemptInput {
        binding: binding.clone(),
        root_id: fixture.attempt.root_id.clone(),
        allocation_epoch: "2".into(),
        branch: "next-branch".into(),
        source_root: fixture.attempt.source_root.clone(),
        partition: fixture.attempt.partition.clone(),
        target: fixture.attempt.target.clone(),
        temp: fixture.attempt.temp.clone(),
        capture: fixture.attempt.capture.clone(),
        authorization: fixture.attempt.authorization.clone(),
    };
    let request = serde_json::json!({
        "version": 1,
        "operation": "run",
        "id": "pending-run",
        "payload": {
            "registration": &fixture.registration,
            "attempt": attempt,
            "acquire": {
                "envelope": RunEnvelope {
                    binding,
                    partition: partition.clone(),
                    admission_reference: "admission-ref".into(),
                    admission_credential: Credential::new("admission-proof")
                },
                "acquire_id": "pending-acquire",
                "required_bytes": 64
            },
            "finalize_id": "pending-finalize"
        }
    });
    let (host_stream, parent_stream) = std::os::unix::net::UnixStream::pair().unwrap();
    let host = RunnerHost::new(socket, Arc::new(BridgeVerifier));
    let host_thread = thread::spawn(move || host.serve(host_stream));
    let (response, callbacks) = bridge_parent(parent_stream, &fixture.owners, &partition, &request);
    let result: HostResult = serde_json::from_value(response["result"].clone()).unwrap();
    let served = host_thread.join().unwrap().unwrap();
    daemon_guard.stop();

    assert!(matches!(
        served.status,
        super::runner_host::HostStatus::StoragePending
    ));
    assert!(result.storage.is_none());
    assert!(result.token_digest.is_none());
    assert!(result.outcome.is_none());
    assert!(callbacks.is_empty());
    assert_eq!(fixture.owners.calls.load(Ordering::Relaxed), 0);
}
