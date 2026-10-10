use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);
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
    fn audit(&self, registration: &Registration, _: &SourceEvidence) -> Result<()> {
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
        let mut units = vec![
            UnitEvidence {
                id: "source-unit".into(),
                anchors: set(&["dep-info", "consumer-two"]),
                locals: vec![LocalInput::DepInfo {
                    anchor: "dep-info".into(),
                    files: registration
                        .source_files
                        .iter()
                        .map(|path| source(path))
                        .collect(),
                    environment: BTreeMap::from([(
                        "KNOWN".into(),
                        Some("value with spaces".into()),
                    )]),
                }],
                associations: vec![
                    Association {
                        input: source("src.rs"),
                        anchors: set(&["consumer-two"]),
                        producer: None,
                        native_receipt: None,
                    },
                    Association {
                        input: InputPath {
                            root: InputRoot::Generated,
                            path: "generated.rs".into(),
                        },
                        anchors: set(&["dep-info"]),
                        producer: Some("script-unit".into()),
                        native_receipt: None,
                    },
                ],
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
