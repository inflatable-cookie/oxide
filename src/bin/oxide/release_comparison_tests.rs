// Only included inside prune's cfg(test) module. No production reader seam.
use barge::os::{AccountFacts, ProcessObserver, VolumeFacts};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

struct ComparisonAccount(PathBuf);

impl AccountFacts for ComparisonAccount {
    fn account_home(&self) -> barge::error::Result<PathBuf> {
        Ok(self.0.clone())
    }
    fn state_root(&self) -> barge::error::Result<PathBuf> {
        Ok(self.0.join("barge-state"))
    }
    fn tool_state_dir(&self, tool: &str) -> barge::error::Result<PathBuf> {
        assert_eq!(tool, "oxide");
        Ok(self.0.join("oxide-state"))
    }
    fn tool_cache_dir(&self, tool: &str) -> barge::error::Result<PathBuf> {
        assert_eq!(tool, "oxide");
        Ok(self.0.join("cache"))
    }
}

// Declared test-platform facts: this observer reports no open descriptors,
// holds or platform binaries. This is synthetic custody, not a host census.
struct ComparisonObserver;
impl ProcessObserver for ComparisonObserver {
    fn effective_uid(&self) -> barge::error::Result<u32> {
        barge::os::platform().effective_uid()
    }
    fn boot_identity(&self) -> barge::error::Result<String> {
        Ok("comparison-fixture-boot".to_owned())
    }
    fn process_start(&self, pid: u32) -> barge::error::Result<Option<u64>> {
        Ok((pid == std::process::id()).then_some(7_000_000))
    }
    fn verify_platform_binary(
        &self,
        _identity: barge::os::ProcessIdentity,
    ) -> barge::error::Result<barge::os::PlatformStatus> {
        Ok(barge::os::PlatformStatus::NotVerified {
            reason: "synthetic observer; no opaque descriptors".to_owned(),
        })
    }
    fn scan_open_descriptors(
        &self,
        _request: &barge::os::DescriptorScanRequest<'_>,
    ) -> barge::error::Result<barge::os::DescriptorScan> {
        Ok(barge::os::DescriptorScan::Complete {
            processes: 0,
            descriptors: 0,
            foreign: 0,
            retry: Default::default(),
            os_blind_spot: Default::default(),
            dead_handles: Default::default(),
            open: Vec::new(),
        })
    }
}

#[derive(Clone, Copy)]
struct ComparisonCase {
    name: &'static str,
    selected: bool,
    state_known: bool,
}

fn comparison_private_dir(path: &Path) {
    fs::create_dir(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn comparison_records(account: &ComparisonAccount, row: &mut Row, spec: ComparisonCase) -> Store {
    comparison_private_dir(&account.0);
    comparison_private_dir(&account.0.join("cache"));
    comparison_private_dir(&account.0.join("oxide-state"));
    let store = Store::for_tests(&account.0.join("cache/build-pool"));
    store.prime_control_for_tests();
    let control = store.owner_control();
    ensure_private_dir(&registry_dir(control)).unwrap();
    row.receipt = outputs::receipt_path_public(control, &row.checkout_id);
    row.selected = spec.selected;
    let mut receipt = snapshot_receipt(
        row,
        "oxide.outputs.transition/2",
        if spec.selected {
            "committed"
        } else {
            "detached"
        },
        spec.selected.then_some(row.root.as_path()),
    );
    if spec.selected {
        receipt["roots"][0]["nonce"] = serde_json::json!(row.nonce);
        receipt["links"][0]["selected"]["nonce"] = serde_json::json!(row.nonce);
    }
    publish_snapshot_receipt(control, row, &receipt);
    let mut registry = empty_registry();
    registry.rows = vec![row.clone()];
    write_registry(control, &registry).unwrap();
    // Barge reads the real fixture control plus a synthetic empty lease ledger.
    // The empty ledger is explicit, never inferred from a missing file.
    fs::write(
        store.owner_control().parent().unwrap().join("ledger.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "store_id": "0123456789abcdef0123456789abcdef",
            "cap_bytes": 1048576, "next_slot": 1, "next_lease": 1, "slots": []
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(
        store.owner_control().parent().unwrap().join("ledger.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    if !spec.state_known {
        // A present malformed required state is unknown to BOTH real readers:
        // Oxide PoolWitness::read_state -> parse_state and Barge
        // PoolCustody::read_pool -> parse_state reject invalid JSON. Missing
        // state has different meanings and is retained only as earlier evidence.
        fs::write(control.join("state.json"), b"{").unwrap();
    }
    store
}

fn comparison_marker(row: &Row) -> serde_json::Value {
    let mut marker = root_marker(row);
    // Match the production root/2 writer before recording the marker digest.
    marker["checkout_volume_uuid"] = serde_json::json!(row.checkout_volume_uuid);
    marker
}

fn release_comparison_case(spec: ComparisonCase) {
    use barge::area::{self, AreaDeclaration, RootSpec};
    let mount =
        PathBuf::from(std::env::var_os("OXIDE_COMPARISON_MOUNT").expect("owned image required"));
    let uuid = std::env::var("OXIDE_COMPARISON_UUID").unwrap();
    let binding = Binding::open(&mount, Some(&uuid)).unwrap();
    drop(binding);
    let platform = barge::os::platform();
    let facts = platform.identify(&mount).unwrap();
    assert_eq!(facts.volume_uuid.as_ref().unwrap().as_str(), uuid);
    assert_eq!(facts.mount_point, mount);
    assert_eq!(facts.filesystem, barge::os::FileSystem::Apfs);
    assert!(!facts.boot_volume);
    let base = mount.join(spec.name);
    comparison_private_dir(&base);
    let home = base.join("home");
    comparison_private_dir(&home);
    // Filtered libtest runs serially; no fixture Cargo/rustup child is launched.
    // All engine account inputs below are explicit, with no account fallback.
    std::env::set_var("HOME", &home);
    std::env::set_var("OXIDE_TEST_HOME_FROM_ENV", "1");
    let checkout = base.join("checkout");
    // Selection is the sole changed premise of the selected control. Its
    // checkout stays positively absent, just like the candidate's checkout.
    assert!(matches!(fs::symlink_metadata(&checkout), Err(error)
        if error.kind() == std::io::ErrorKind::NotFound));
    let account = ComparisonAccount(base.join("barge-account"));
    comparison_private_dir(&account.0);
    let state =
        barge::state::StatePaths::resolve(&account, Some(&account.0.join("barge-state"))).unwrap();
    state.initialize().unwrap();
    let record = barge::registry::VolumeRecord {
        volume_uuid: facts.volume_uuid.unwrap(),
        mount_hint: Some(mount.clone()),
        availability: barge::registry::Availability::Permanent,
        durability: barge::registry::Durability::Expendable,
        root_dir: spec.name.to_owned(),
        folders: barge::registry::FolderOverrides::none(),
        origin: barge::registry::RecordOrigin::Declared,
        scratch_grants: barge::registry::ScratchGrants::none(),
        space: None,
        generation: 1,
        registered_ms: 1000,
    };
    let mut locks = barge::state::LockSet::new(state.clone());
    let lock = locks.lock_registry().unwrap();
    barge::registry::RegistryWriter::new(&state)
        .publish(
            &lock,
            &barge::registry::RegistryDocument::new(vec![record.clone()]),
        )
        .unwrap();
    drop(lock);
    let declaration = AreaDeclaration {
        namespace: "oxide".to_owned(),
        area: "outputs".to_owned(),
        folder: "barge-outputs".to_owned(),
        overrides: Default::default(),
        absent: area::AbsentPolicy::Fail {
            attach: "owned comparison image".to_owned(),
        },
        prune: area::PrunePolicy::ReleaseOnReference,
        reserve: area::ReserveParticipation::Excluded,
    };
    area::declare(&state, &declaration, 1000).unwrap();
    let resolved = area::resolve(
        &platform,
        &platform,
        &platform,
        &platform,
        &record,
        &declaration,
        None,
    )
    .unwrap();
    let barge_root = area::create_root(
        &state,
        &platform,
        &ComparisonObserver,
        &resolved,
        RootSpec {
            kind: area::RootKind::AreaRoot,
        },
    )
    .unwrap();
    drop(resolved);
    let oxide_root = mount.join("oxide-outputs").join(spec.name);
    comparison_private_dir(&oxide_root);
    let mut row = row_for(&base);
    row.checkout = checkout;
    row.checkout_volume_uuid = Some(uuid.clone());
    row.volume_uuid = Some(uuid);
    row.root = oxide_root.clone();
    write_root_marker(&oxide_root, &comparison_marker(&row));
    let identity = observe_fixture_root(&row);
    row.root_dev = Some(identity.dev);
    row.root_ino = Some(identity.ino);
    row.marker_digest = Some(identity.marker_digest);
    let oxide_account = ComparisonAccount(base.join("oxide-account"));
    let oxide_store = comparison_records(&oxide_account, &mut row, spec);
    // Explicit one-to-one mapping: logical spec.name -> these two physical roots.
    // All other synthetic checkout/reference/custody facts come from spec.
    let mut mapped = row.clone();
    mapped.root = barge_root.path.clone();
    write_root_marker(&mapped.root, &comparison_marker(&mapped));
    // Account directory exists for Barge StatePaths; records owns its own child.
    let reference_account = ComparisonAccount(account.0.join("references"));
    let _barge_store = comparison_records(&reference_account, &mut mapped, spec);
    let registry = read_registry(oxide_store.owner_control()).unwrap().unwrap();
    let inputs = [&row, &mapped]
        .into_iter()
        .flat_map(|input| {
            let control = input.receipt.parent().unwrap().parent().unwrap();
            [
                input.receipt.clone(),
                input.root.join(outputs::MARKER),
                control.join("registry.json"),
                control.join("outputs/registry.json"),
                control.join("state.json"),
                control.parent().unwrap().join("ledger.json"),
            ]
        })
        .map(|path| {
            let bytes = match fs::read(&path) {
                Ok(bytes) => Some(String::from_utf8(bytes).unwrap()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("fixture input {}: {error}", path.display()),
            };
            (path, bytes)
        })
        .collect::<Vec<_>>();
    // Setup is complete before either actual engine call.
    // ReleaseOnReference has no age/cooldown waiver: the ordinary policy still
    // requires known activity at or before this actual post-setup clock.
    let now_ms = barge::state::now_ms();
    let oxide = evaluate_test(&oxide_store, oxide_store.owner_control(), &registry, None);
    let barge = barge::release::plan(
        &state,
        &platform,
        &platform,
        &platform,
        &ComparisonObserver,
        &reference_account,
        &barge::release::ReleaseRequest {
            namespace: "oxide",
            area: "outputs",
            volume_uuid: &record.volume_uuid,
            now_ms,
            bounds: barge::release::ReleaseBounds::design(),
            named_roots: &[],
        },
    );
    let oxide_value = match &oxide {
        Ok(result) => {
            serde_json::json!({"complete": result.complete, "rows": result.iter().map(|entry|
            serde_json::json!({"root": entry.row.root, "candidate": entry.candidate.is_some(), "kept": entry.kept})
        ).collect::<Vec<_>>() })
        }
        Err(reason) => serde_json::json!({"global_refusal": reason}),
    };
    let barge_value = match &barge {
        Ok(result) => serde_json::to_value(result).unwrap(),
        Err(reason) => serde_json::json!({"global_refusal": reason.to_string()}),
    };
    let report = serde_json::json!({"case": spec.name, "logical_root": spec.name, "now_ms": now_ms,
        "mapping": {"oxide": oxide_root, "barge": barge_root.path},
        "fixture_inputs": inputs,
        "synthetic": {"selected": spec.selected, "checkout_present": false,
            "state_known": spec.state_known, "barge_open_descriptors": [], "ledger_slots": []},
        "oxide": oxide_value, "barge": barge_value});
    let evidence = PathBuf::from(std::env::var_os("OXIDE_PROOF_EVIDENCE").unwrap());
    fs::write(
        evidence.join(format!("comparison-{}.json", spec.name)),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{report}");
    let oxide = oxide.unwrap();
    let barge = barge.unwrap();
    assert_eq!(oxide.len(), 1, "union must include the Oxide root");
    assert_eq!(
        barge.plan.rows.rows.len(),
        1,
        "union must include the Barge root"
    );
    let expected = !spec.selected && spec.state_known;
    let barge_row = &barge.plan.rows.rows[0];
    assert!(
        barge_row.idle.age_seconds.is_some(),
        "clock/policy masked the case: {report}"
    );
    assert_ne!(barge_row.blocked_by.as_deref(), Some("policy"), "{report}");
    if spec.selected {
        assert_eq!(
            barge_row.blocked_by.as_deref(),
            Some("references"),
            "{report}"
        );
        assert!(
            barge_row
                .references
                .iter()
                .any(|reference| reference.current),
            "{report}"
        );
    } else if !spec.state_known {
        assert!(
            !oxide.complete,
            "unknown custody must refuse the Oxide batch: {report}"
        );
        assert!(
            !barge_row.references_complete,
            "unknown custody gate was not reached: {report}"
        );
        assert_eq!(
            barge_row.blocked_by.as_deref(),
            Some("references"),
            "{report}"
        );
    }
    assert_eq!(oxide[0].candidate.is_some(), expected, "{report}");
    assert_eq!(barge.plan.rows.rows[0].eligible, expected, "{report}");
    if expected {
        assert!(
            oxide.complete && barge.plan.rows.rows[0].references_complete,
            "{report}"
        );
    }
}

#[test]
#[ignore = "requires the guarded owned image selector"]
fn release_comparison_candidate() {
    release_comparison_case(ComparisonCase {
        name: "candidate",
        selected: false,
        state_known: true,
    });
}
#[test]
#[ignore = "requires the guarded owned image selector"]
fn release_comparison_selected() {
    release_comparison_case(ComparisonCase {
        name: "selected",
        selected: true,
        state_known: true,
    });
}
#[test]
#[ignore = "requires the guarded owned image selector"]
fn release_comparison_unknown() {
    release_comparison_case(ComparisonCase {
        name: "unknown",
        selected: false,
        state_known: false,
    });
}
