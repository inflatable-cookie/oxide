use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    base: PathBuf,
    root: PathBuf,
    slot: PathBuf,
    partition: PartitionKey,
}
impl Fixture {
    fn new() -> Self {
        let scratch = PathBuf::from(
            std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").expect("canonical task scratch"),
        );
        let base = scratch.join(format!(
            "freshness-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).expect("new task-owned regression root");
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let root = base.join("source");
        let slot = base.join("generation");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&slot).unwrap();
        for (name, bytes) in FILES {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let partition = PartitionKey {
            project_component: "fixture:local".into(),
            workflow: WORKFLOW.into(),
            cargo_provenance: "pinned".into(),
            rustc_provenance: "pinned".into(),
            host: "local".into(),
            target: "local".into(),
            profile: "test".into(),
            features: vec![],
            config_digest: "fixture".into(),
            environment_digest: "fixture".into(),
            path_identity: format!("same-root-fixture-v1:{}", root.display()),
        };
        Self {
            base,
            root,
            slot,
            partition,
        }
    }
    fn evidence(&self) {
        let units = [
            ("oxide-source-probe", "lib-oxide_source_probe", "src/lib.rs"),
            (
                "oxide-source-probe",
                "test-lib-oxide_source_probe",
                "src/lib.rs",
            ),
            (
                "oxide-source-probe",
                "test-integration-test-probe",
                "tests/probe.rs",
            ),
            (
                "oxide-source-probe",
                "build-script-build-script-build",
                "build.rs",
            ),
            (
                "oxide-source-probe",
                "run-build-script-build-script-build",
                "marker.txt",
            ),
            (
                "oxide-source-probe-macro",
                "lib-oxide_source_probe_macro",
                "src/lib.rs",
            ),
            (
                "oxide-source-probe-macro",
                "test-lib-oxide_source_probe_macro",
                "src/lib.rs",
            ),
        ];
        for (index, (package, unit, source)) in units.iter().enumerate() {
            let directory = format!("{package}-{index:016x}");
            let prefix = format!("debug/.fingerprint/{directory}");
            fs::create_dir_all(self.slot.join(&prefix)).unwrap();
            fs::write(self.slot.join(&prefix).join(unit), b"0123456789abcdef").unwrap();
            let local = if unit.starts_with("run-") {
                let output = format!("debug/build/{directory}/output");
                let out = self.slot.join(format!("debug/build/{directory}/out"));
                fs::create_dir_all(&out).unwrap();
                for name in [
                    "generated.rs",
                    "proc-macro-input.txt",
                    "producer-observation.txt",
                ] {
                    fs::write(out.join(name), b"generated fixture data").unwrap();
                }
                fs::write(
                    self.slot.join(&output),
                    b"cargo:rerun-if-changed=marker.txt\n",
                )
                .unwrap();
                format!(r#"{{"RerunIfChanged":{{"output":"{output}","paths":["marker.txt"]}}}}"#)
            } else {
                let dep = format!("{prefix}/dep-{unit}");
                fs::write(self.slot.join(&dep), encoded(&[(0, source)])).unwrap();
                format!(r#"{{"CheckDepInfo":{{"dep_info":"{dep}","checksum":false}}}}"#)
            };
            // Committed pinned capture shape; synthetic hashes are never live receipts.
            let json = format!(
                r#"{{"rustc":11722972603611519644,"features":"[]","declared_features":"[]","target":1,"profile":1,"path":1,"deps":[],"local":[{local}],"rustflags":[],"config":9396254390672932401,"compile_kind":0}}"#
            );
            fs::write(self.slot.join(prefix).join(format!("{unit}.json")), json).unwrap();
        }
    }
    fn map(&self) -> InputMap {
        collect(
            &self.root,
            &self.slot,
            &self.partition,
            "generation-1",
            requested_tree(&self.root).unwrap(),
        )
        .unwrap()
    }
    fn persist(&self) {
        fs::write(self.slot.join(MAP), self.map().encode().unwrap()).unwrap();
    }
    fn admit(&self) -> Result<()> {
        admit(
            &self.root,
            &self.slot,
            &self.partition,
            "generation-1",
            true,
            &requested_tree(&self.root)?,
        )
    }
    fn set_time(&self, path: &Path, time: SystemTime) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).expect("delete only new test-owned fixture");
    }
}
fn encoded(paths: &[(u8, &str)]) -> Vec<u8> {
    let mut bytes = vec![1, 0, 0, 0, 255, 1];
    let mut writer = Writer::new();
    writer.u32(paths.len() as u32);
    for (kind, path) in paths {
        writer.u8(*kind);
        writer.string(path).unwrap();
        writer.u8(0);
    }
    writer.u32(0);
    bytes.extend(writer.finish());
    bytes
}

#[test]
fn same_root_different_tree_preserves_dependency_evidence_and_equal_content() {
    let fixture = Fixture::new();
    fixture.evidence();
    fixture.persist();
    let old_tree = requested_tree(&fixture.root).unwrap();
    let old_dependency = fs::read(fixture.slot.join("debug/.fingerprint/oxide-source-probe-macro-0000000000000005/dep-lib-oxide_source_probe_macro")).unwrap();
    assert_eq!(fixture.admit(), Ok(()));
    let marker = fixture.root.join("probe/marker.txt");
    fs::write(&marker, b"changed-marker\n").unwrap();
    fixture.set_time(&marker, SystemTime::now() + Duration::from_secs(3));
    assert_ne!(old_tree, requested_tree(&fixture.root).unwrap());
    assert_eq!(fixture.admit(), Ok(()));
    assert_eq!(fs::read(fixture.slot.join("debug/.fingerprint/oxide-source-probe-macro-0000000000000005/dep-lib-oxide_source_probe_macro")).unwrap(), old_dependency);
}

#[test]
fn old_and_equal_mtime_changed_content_refuses_before_execution_then_restore_passes() {
    let fixture = Fixture::new();
    fixture.evidence();
    fixture.persist();
    let map = fixture.map();
    let reference = map
        .references
        .values()
        .map(|entry| entry.mtime)
        .max()
        .unwrap();
    let marker = fixture.root.join("probe/marker.txt");
    for time in [
        UNIX_EPOCH + Duration::from_nanos(reference),
        UNIX_EPOCH + Duration::from_secs(1),
    ] {
        fs::write(&marker, b"changed-marker\n").unwrap();
        fixture.set_time(&marker, time);
        assert_eq!(fixture.admit(), Err(AdapterRefusal::SourceStale));
        assert!(!fixture.slot.join(BARRIER).exists());
        fs::write(&marker, b"baseline-marker\n").unwrap();
        fixture.set_time(&marker, time);
        assert_eq!(fixture.admit(), Ok(()));
    }
}

#[test]
fn tamper_reference_identity_cross_root_and_unknown_formats_refuse() {
    let fixture = Fixture::new();
    fixture.evidence();
    fixture.persist();
    let original = fs::read(fixture.slot.join(MAP)).unwrap();
    let mut corrupt = original.clone();
    corrupt[40] ^= 1;
    fs::write(fixture.slot.join(MAP), corrupt).unwrap();
    assert_eq!(fixture.admit(), Err(AdapterRefusal::InputUnknown));
    fs::write(fixture.slot.join(MAP), &original).unwrap();
    let other = Fixture::new();
    assert_eq!(
        admit(
            &other.root,
            &fixture.slot,
            &fixture.partition,
            "generation-1",
            true,
            &requested_tree(&other.root).unwrap()
        ),
        Err(AdapterRefusal::InputUnknown)
    );
    let mut wrong_identity = fixture.map();
    wrong_identity.generation = "other-generation".into();
    fs::write(fixture.slot.join(MAP), wrong_identity.encode().unwrap()).unwrap();
    assert_eq!(fixture.admit(), Err(AdapterRefusal::InputUnknown));
    fs::write(fixture.slot.join(MAP), original).unwrap();
    let reference = fixture
        .slot
        .join("debug/.fingerprint/oxide-source-probe-0000000000000000/dep-lib-oxide_source_probe");
    fs::write(&reference, encoded(&[(0, "marker.txt")])).unwrap();
    assert_eq!(fixture.admit(), Err(AdapterRefusal::InputUnknown));
    assert!(dep_info(&[1, 0, 0, 0, 255, 2], &fixture.root, &fixture.slot).is_err());
    assert!(JsonReader::parse(br#"{"local":[],"local":[]}"#).is_err());
}

#[test]
fn partial_success_overall_failure_lost_outcome_and_reconcile_do_not_resurrect() {
    let fixture = Fixture::new();
    fixture.evidence();
    fixture.persist();
    let map = fs::read(fixture.slot.join(MAP)).unwrap();
    // Even a pre-spawn runner error after begin is possibly spawned. A success
    // in another unit can rewrite references while the overall command fails.
    begin(&fixture.slot, "lease-failed").unwrap();
    let dep = fixture.slot.join("debug/.fingerprint/oxide-source-probe-macro-0000000000000005/dep-lib-oxide_source_probe_macro");
    fs::write(dep, encoded(&[(0, "src/lib.rs")])).unwrap();
    assert_eq!(fixture.admit(), Err(AdapterRefusal::InputUnknown));
    assert_eq!(fs::read(fixture.slot.join(MAP)).unwrap(), map);
    // Ownership reconciliation cannot remove the durable eligibility barrier.
    assert_eq!(
        recovery(SlotState::Idle, true, true, false, true),
        RecoveryDecision::Refuse
    );
    assert_eq!(fixture.admit(), Err(AdapterRefusal::InputUnknown));
    assert_eq!(
        recovery(SlotState::Uncertain, true, true, true, true),
        RecoveryDecision::Refuse
    );
}

#[test]
fn cold_initialization_and_explicit_owned_test_recovery_keep_boundaries() {
    let fixture = Fixture::new();
    let tree = requested_tree(&fixture.root).unwrap();
    assert_eq!(
        admit(
            &fixture.root,
            &fixture.slot,
            &fixture.partition,
            "generation-1",
            false,
            &tree
        ),
        Ok(())
    );
    fixture.evidence();
    fixture.persist();
    begin(&fixture.slot, "failed").unwrap();
    for (state, owned, grant, capacity) in [
        (SlotState::Idle, true, false, true),
        (SlotState::Idle, true, true, false),
        (SlotState::Idle, false, true, true),
        (SlotState::Leased, true, true, true),
        (SlotState::Uncertain, true, true, true),
    ] {
        assert_eq!(
            recovery(state, owned, true, grant, capacity),
            RecoveryDecision::Refuse
        );
    }
    assert_eq!(
        recovery(SlotState::Idle, true, true, true, true),
        RecoveryDecision::ReplaceOwnedGeneration
    );
    // Explicit test-only grant covers this newly owned small generation, never
    // source/evidence. Runtime has no destructive recovery entrypoint.
    fs::remove_dir_all(&fixture.slot).unwrap();
    fs::create_dir(&fixture.slot).unwrap();
    assert_eq!(
        admit(
            &fixture.root,
            &fixture.slot,
            &fixture.partition,
            "generation-2",
            false,
            &tree
        ),
        Ok(())
    );
    fixture.evidence();
    let rebound = collect(
        &fixture.root,
        &fixture.slot,
        &fixture.partition,
        "generation-2",
        tree,
    )
    .unwrap();
    assert_eq!(
        InputMap::decode(&rebound.encode().unwrap())
            .unwrap()
            .generation,
        "generation-2"
    );
    fs::write(fixture.slot.join(MAP), b"interrupted initialization").unwrap();
    assert_eq!(
        admit(
            &fixture.root,
            &fixture.slot,
            &fixture.partition,
            "generation-2",
            false,
            &requested_tree(&fixture.root).unwrap()
        ),
        Err(AdapterRefusal::InputUnknown)
    );
}

#[test]
fn generated_proc_macro_directory_external_symlink_and_extra_source_inputs_refuse() {
    let fixture = Fixture::new();
    fixture.evidence();
    fixture.persist();
    let generated = fixture
        .slot
        .join("debug/build/oxide-source-probe-0000000000000004/out/proc-macro-input.txt");
    fs::write(&generated, b"changed generated read").unwrap();
    fixture.set_time(&generated, UNIX_EPOCH + Duration::from_secs(1));
    assert_eq!(fixture.admit(), Err(AdapterRefusal::SourceStale));
    fs::write(&generated, b"generated fixture data").unwrap();
    fs::write(fixture.root.join("extra-input"), b"unknown").unwrap();
    assert_eq!(
        requested_tree(&fixture.root),
        Err(AdapterRefusal::InputUnknown)
    );
    fs::remove_file(fixture.root.join("extra-input")).unwrap();
    let dep = fixture
        .slot
        .join("debug/.fingerprint/oxide-source-probe-0000000000000000/dep-lib-oxide_source_probe");
    fs::write(&dep, encoded(&[(0, "src")])).unwrap();
    assert!(fixture.map_result().is_err());
    fs::write(&dep, encoded(&[(1, "/unregistered/external")])).unwrap();
    assert!(fixture.map_result().is_err());
    fs::write(&dep, encoded(&[(0, "../probe-macro/src/lib.rs")])).unwrap();
    assert!(fixture.map_result().is_err());
    // Link only to another newly-created file in this test's owned scratch.
    fs::remove_file(&generated).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("probe/marker.txt"), &generated).unwrap();
    assert!(owned_file(
        &fixture.slot,
        "debug/build/oxide-source-probe-0000000000000004/out/proc-macro-input.txt"
    )
    .is_err());
}
impl Fixture {
    fn map_result(&self) -> Result<InputMap> {
        collect(
            &self.root,
            &self.slot,
            &self.partition,
            "generation-1",
            requested_tree(&self.root)?,
        )
    }
}

#[test]
fn decode_caps_and_old_style_script_refuse_without_omission() {
    assert!(InputMap::decode(&vec![0; MAX_MAP_BYTES + 1]).is_err());
    let mut dep = vec![1, 0, 0, 0, 255, 1];
    dep.extend_from_slice(&((MAX_ENTRIES + 1) as u32).to_le_bytes());
    assert!(dep_info(&dep, Path::new("/source"), Path::new("/slot")).is_err());
    let fixture = Fixture::new();
    fixture.evidence();
    let output = fixture
        .slot
        .join("debug/build/oxide-source-probe-0000000000000004/output");
    fs::write(output, b"old-style script\n").unwrap();
    assert_eq!(
        fixture.map_result().err(),
        Some(AdapterRefusal::InputUnknown)
    );
}
