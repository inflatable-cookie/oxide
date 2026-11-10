//! Fixture construction and typed observations only. Eligibility, deletion and
//! recovery always run through the actual commands and default Barge adapter.
//! No entry point is available in release builds or outside the shared guard.
use super::*;
use barge::os::AccountFacts;
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;

fn guard() -> Result<PathBuf, String> {
    let evidence = proof_evidence().ok_or("delegation fixtures require the shared proof guard")?;
    let root = evidence.parent().ok_or("guard root missing")?.to_path_buf();
    let facts = crate::config::Facts::from_env();
    let os = barge::os::platform();
    let account = os.tool_state_dir("oxide").map_err(|e| e.to_string())?;
    if account != facts.state_dir()? || !account.starts_with(&root) {
        return Err(
            "fixture account control requires the source-bound Barge proof-account feature".into(),
        );
    }
    let settings = store::settings()?;
    if !settings.pool.starts_with(&root) {
        return Err("fixture pool escapes the guard".into());
    }
    Ok(root)
}

fn paths() -> Result<barge::state::StatePaths, String> {
    delegation::state_paths()
}

fn volume(uuid: &str, mount: &Path, durability: barge::registry::Durability) -> Result<(), String> {
    use barge::registry::*;
    let paths = paths()?;
    paths.initialize().map_err(|e| e.to_string())?;
    let mut locks = barge::state::LockSet::new(paths.clone());
    let lock = locks.lock_registry().map_err(|e| e.to_string())?;
    RegistryWriter::new(&paths)
        .publish(
            &lock,
            &RegistryDocument::new(vec![VolumeRecord {
                volume_uuid: barge::os::VolumeUuid::parse(uuid).map_err(|e| e.to_string())?,
                mount_hint: Some(mount.to_path_buf()),
                availability: Availability::Permanent,
                durability,
                root_dir: String::new(),
                folders: FolderOverrides::none(),
                origin: RecordOrigin::Declared,
                scratch_grants: ScratchGrants::none(),
                space: None,
                generation: 1,
                registered_ms: barge::state::now_ms(),
            }]),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn grant(present: bool) -> Result<(), String> {
    use barge::area::*;
    declare(
        &paths()?,
        &AreaDeclaration {
            namespace: "oxide".into(),
            area: "outputs".into(),
            folder: outputs::EXTERNAL_DIR.into(),
            overrides: Default::default(),
            absent: AbsentPolicy::Fail {
                attach: "owned fixture image".into(),
            },
            prune: PrunePolicy::ReleaseOnReference,
            reserve: ReserveParticipation::Excluded,
            outputs_grant: present.then(|| OutputsGrant::new("oxide", barge::state::now_ms())),
        },
        barge::state::now_ms(),
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn setup(label: &str, mount: &Path, uuid: &str) -> Result<(), String> {
    if label.is_empty()
        || label.len() > 40
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("invalid fixture label".into());
    }
    // The image is already declared to the guard BEFORE creation. Bind this
    // fixture's mount to that exact declaration and kernel UUID before writes.
    let evidence = proof_evidence().ok_or("guard unavailable")?;
    let custody: serde_json::Value = serde_json::from_slice(
        &fs::read(evidence.join("delegation-image.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if custody["mount"].as_str() != mount.to_str() || custody["volume_uuid"].as_str() != Some(uuid)
    {
        return Err("fixture mount differs from the guarded image".into());
    }
    let name = mount
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("fixture mount name missing")?;
    let backing = PathBuf::from(custody["backing"].as_str().ok_or("backing missing")?);
    let image_declaration: serde_json::Value = serde_json::from_slice(
        &fs::read(evidence.join("image-custody.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if mount.parent() != Some(Path::new("/Volumes"))
        || !name.starts_with("Ox304")
        || name.len() != 13
        || !name[5..].bytes().all(|v| v.is_ascii_hexdigit())
        || fs::canonicalize(&backing).map_err(|e| e.to_string())? != backing
        || backing.parent() != Some(evidence.as_path())
        || image_declaration["backings"]
            .as_array()
            .is_none_or(|v| v.len() != 1 || v[0].as_str() != backing.to_str())
    {
        return Err("fixture image is not this guard's declared private backing".into());
    }
    let _binding = Binding::open(mount, Some(uuid))?;
    let internal = Store::open_or_create()?;
    crate::config::remember_pool(&crate::config::Facts::from_env(), &internal.pool)?;
    let code = removable::run(
        &[
            "register-removable",
            mount.to_str().ok_or("mount is not UTF-8")?,
            "--reserve",
            "16MiB",
            "--cap",
            "64MiB",
            "--mode",
            "permanent",
        ]
        .map(OsString::from),
    )?;
    if code != 0 {
        return Err("fixture registration refused".into());
    }
    let store = paired_store()?;
    let control = store.owner_control();
    let tx = fresh_nonce()?;
    store.initialize_outputs(&tx, None, &|tx, create| {
        create_or_validate_registry(control, tx, create)
    })?;
    let checkout = PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?).join("c");
    let checkout_uuid = checkout_volume_uuid(checkout.parent().ok_or("checkout parent missing")?)
        .ok_or("checkout volume unknown")?;
    let id = sha256_hex(checkout.as_os_str().as_bytes());
    let parent = mount.join(outputs::EXTERNAL_DIR);
    fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    let mut roots = Vec::new();
    for (suffix, generation, nonce, selected) in [
        ("old", 1, "b".repeat(32), false),
        ("successor", 2, "c".repeat(32), true),
    ] {
        let root = parent.join(format!("{label}-{suffix}"));
        fs::create_dir(&root).map_err(|e| e.to_string())?;
        fs::write(root.join("payload-a"), b"disposable payload one").map_err(|e| e.to_string())?;
        fs::write(root.join("payload-b"), b"disposable payload two").map_err(|e| e.to_string())?;
        let marker = serde_json::to_vec(&serde_json::json!({"version":"oxide.outputs.root/2", "checkout":checkout, "checkout_id":id, "checkout_volume_uuid":checkout_uuid, "label":label, "recipe":outputs::RECIPE_PUBLIC, "kind":"external", "volume_uuid":uuid, "nonce":nonce, "generation":generation, "created_ms":1})).map_err(|e| e.to_string())?;
        fs::write(root.join(outputs::MARKER), &marker).map_err(|e| e.to_string())?;
        let facts = fs::metadata(&root).map_err(|e| e.to_string())?;
        for member in ["debug", "release"] {
            rows.push(Row {
                checkout_id: id.clone(),
                checkout: checkout.clone(),
                checkout_volume_uuid: Some(checkout_uuid.clone()),
                receipt: outputs::receipt_path_public(control, &id),
                member: member.into(),
                root: root.clone(),
                root_kind: RootKind::External,
                volume_uuid: Some(uuid.into()),
                recipe: outputs::RECIPE_PUBLIC.into(),
                nonce: Some(nonce.clone()),
                generation,
                root_dev: Some(facts.dev()),
                root_ino: Some(facts.ino()),
                marker_digest: Some(sha256_hex(&marker)),
                selected,
                pending: false,
                pruning: None,
                pruned_ms: None,
                reservation: None,
                cancelling: None,
            });
        }
        roots.push(serde_json::json!({"kind":"external", "root":root, "volume_uuid":uuid, "nonce":nonce, "generation":generation}));
    }
    let receipt = serde_json::json!({"version":"oxide.outputs.transition/3", "checkout":checkout, "checkout_id":id, "generation":2, "state":"committed", "updated_ms":1, "layout":"profile", "roots":roots, "links":[{"member":"debug","selected":roots[1]},{"member":"release","selected":roots[1]}]});
    let _publication = store::publication_guard(control)?;
    let receipt_path = outputs::receipt_path_public(control, &id);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&receipt_path)
        .map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec(&receipt).map_err(|e| e.to_string())?)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    File::open(receipt_path.parent().ok_or("receipt parent missing")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    let mut registry = ready_registry(control)?;
    registry.rows = rows;
    registry.revision += 1;
    write_registry(control, &registry)?;
    drop(_publication);
    volume(uuid, mount, barge::registry::Durability::Expendable)?;
    grant(true)?;
    println!(
        "{}",
        serde_json::json!({"root":roots[0]["root"],"successor":roots[1]["root"],"checkout":checkout,"control":control,"receipt":outputs::receipt_path_public(control,&id),"uuid":uuid,"mount":mount})
    );
    Ok(())
}

pub(super) fn run(args: &[OsString]) -> Result<(), String> {
    let _root = guard()?;
    let args = args
        .iter()
        .map(|v| v.to_str().ok_or("fixture arguments must be UTF-8"))
        .collect::<Result<Vec<_>, _>>()?;
    match args.as_slice() {
        ["setup", label, mount, uuid] => setup(label, Path::new(mount), uuid),
        ["grant", "present"] => grant(true),
        ["grant", "withdraw"] => grant(false),
        ["grant", mode @ ("invalid" | "older")] => {
            let path = barge::area::declaration_path(&paths()?, "oxide", "outputs")
                .map_err(|e| e.to_string())?;
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if *mode == "older" {
                value["generation"] = serde_json::json!(1);
            } else {
                value["outputs_grant"]["family"] = serde_json::json!("invalid");
            }
            fs::write(path, serde_json::to_vec(&value).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        ["volume", uuid, mount, "kept"] => {
            volume(uuid, Path::new(mount), barge::registry::Durability::Kept)
        }
        ["inspect"] => {
            let store = paired_store()?;
            let registry = read_registry_image(store.owner_control())?.ok_or("registry missing")?;
            let intents = barge::state::read_intents(&paths()?, 256).map_err(|e| e.to_string())?;
            if intents.truncated {
                return Err("Barge observation truncated".into());
            }
            println!(
                "{}",
                serde_json::json!({"oxide":registry,"barge":intents.records})
            );
            Ok(())
        }
        ["replay"] => {
            let os = barge::os::platform();
            let report = barge::delete::replay_pending(
                &paths()?,
                &os,
                &os,
                &os,
                barge::delete::DeleteBounds::default(),
                None,
            )
            .map_err(|e| e.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        ["generic-release", uuid] => {
            let os = barge::os::platform();
            let uuid = barge::os::VolumeUuid::parse(uuid).map_err(|e| e.to_string())?;
            let request = barge::release::ReleaseRequest {
                namespace: "oxide",
                area: "outputs",
                volume_uuid: &uuid,
                now_ms: barge::state::now_ms(),
                bounds: barge::release::ReleaseBounds::design(),
                named_roots: &[],
            };
            let paths = paths()?;
            let plan = barge::release::plan(&paths, &os, &os, &os, &os, &os, &request)
                .map_err(|e| e.to_string())?;
            let bytes =
                barge::release::canonical_plan_bytes(&plan.plan).map_err(|e| e.to_string())?;
            let result = barge::release::execute(
                &paths,
                &os,
                &os,
                &os,
                &os,
                &os,
                &barge::release::ReleaseRunRequest {
                    plan_bytes: &bytes,
                    plan_digest: &plan.plan_digest,
                    release: request,
                    delete: barge::delete::DeleteBounds::default(),
                },
            )
            .map_err(|e| e.to_string())?;
            println!("{}", serde_json::json!({"plan":plan,"run":result}));
            Ok(())
        }
        ["lock", uuid, token] => {
            if !hex32(token) {
                return Err("invalid lock token".into());
            }
            let mut locks = barge::state::LockSet::new(paths()?);
            let _lock = locks
                .lock_volume(&barge::os::VolumeUuid::parse(uuid).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let evidence = proof_evidence().ok_or("guard missing")?;
            fs::write(
                evidence.join(format!("lock-{token}")),
                std::process::id().to_string(),
            )
            .map_err(|e| e.to_string())?;
            let stop = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !evidence.join(format!("release-{token}")).exists() {
                if std::time::Instant::now() >= stop {
                    return Err("fixture lock deadline".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(())
        }
        _ => Err("unknown guarded delegation fixture operation".into()),
    }
}
