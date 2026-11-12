//! Fixture construction and typed observations only. Eligibility, deletion and
//! recovery always run through the actual commands and default Barge adapter.
//! No entry point is available in release builds or outside the shared guard.
use super::*;
use barge::os::AccountFacts;
use std::ffi::OsString;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};

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

/// One physical image has one library payload. Each isolated account gets
/// private wrapper controls; copy the last closed library metadata unchanged
/// under its real library lock instead of reinitializing a nonempty payload.
/// This is fixture input construction, never an alternate product evaluator.
fn register_fixture_volume(
    internal: &Store,
    mount: &Path,
    uuid: &str,
) -> Result<serde_json::Value, String> {
    let root =
        fs::canonicalize(std::env::var_os("OXIDE_TASK_SCRATCH_ROOT").ok_or("scratch missing")?)
            .map_err(|e| e.to_string())?;
    let pointer = root.join("delegation-volume-source.json");
    let control = internal.control();
    let mut source_bytes = Vec::new();
    let previous = if let Some(file) = store::private_file(&pointer, false)? {
        file.take(4097)
            .read_to_end(&mut source_bytes)
            .map_err(|e| e.to_string())?;
        if source_bytes.len() > 4096 {
            return Err("fixture source pointer exceeds bound".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Source {
            control: PathBuf,
        }
        let source: Source = serde_json::from_slice(&source_bytes).map_err(|e| e.to_string())?;
        let previous = fs::canonicalize(&source.control).map_err(|e| e.to_string())?;
        if previous != source.control || !previous.starts_with(&root) || previous == control {
            return Err("fixture library source escapes owned scratch".into());
        }
        Some(previous)
    } else {
        None
    };
    let observation = if let Some(previous) = previous {
        let registration = removable::read_registration(&previous)?
            .ok_or("fixture registration source missing")?;
        if registration.mount_point != mount || !registration.volume_uuid.eq_ignore_ascii_case(uuid)
        {
            return Err("fixture registration differs from held image binding".into());
        }
        let library = previous.join(removable::POOL_DIR);
        let lock = store::private_file(&library.join("lock"), false)?
            .ok_or("fixture library lock missing")?;
        // SAFETY: lock is our held file descriptor; this never waits for a writer.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("prior fixture library still has a writer; retain and abort".into());
        }
        let target = control.join(removable::POOL_DIR);
        ensure_private_dir(&target)?;
        let mut hashes = serde_json::Map::new();
        for name in ["config.json", "ledger.json"] {
            let file = store::private_file(&library.join(name), false)?
                .ok_or("fixture library metadata missing")?;
            let mut bytes = Vec::new();
            file.take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("fixture library metadata exceeds bound".into());
            }
            write_fixture_file(&target.join(name), &bytes)?;
            hashes.insert(name.into(), serde_json::json!(sha256_hex(&bytes)));
        }
        write_fixture_file(&target.join("lock"), b"")?;
        write_fixture_file(
            &control.join(removable::REGISTRATION),
            &serde_json::to_vec(&registration).map_err(|e| e.to_string())?,
        )?;
        oxide_storage::lease::managed::ledger_view(&target).map_err(|e| e.to_string())?;
        serde_json::json!({"source_control":previous,"library_bytes_sha256":hashes,"physical_payload_reused":true})
    } else {
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
        serde_json::json!({"actual_product_registration":true,"physical_payload_reused":false})
    };
    let bytes =
        serde_json::to_vec(&serde_json::json!({"control":control})).map_err(|e| e.to_string())?;
    // A fixture pointer, not a safety journal. Old per-case source observations
    // and actual config/ledger bytes remain retained in their original worlds.
    let temp = root.join(".delegation-volume-source.new");
    write_fixture_file(&temp, &bytes)?;
    fs::rename(&temp, &pointer).map_err(|e| e.to_string())?;
    File::open(&root)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(observation)
}

fn write_fixture_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    File::open(path.parent().ok_or("fixture parent missing")?)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

/// The image container may be reused by later isolated worlds, but each root
/// must be freshly created. Use the same descriptor-based permission gate as
/// preview; never adopt or chmod an unexpected existing root or container.
fn create_fixture_root(mount: &Path, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err("fixture root must be one directory name".into());
    }
    let mount_fd = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(mount)
        .map_err(|e| e.to_string())?;
    let parent_fd = oxide_storage::volume::dir_at(mount_fd.as_fd(), outputs::EXTERNAL_DIR, true)?;
    let root = mount.join(outputs::EXTERNAL_DIR).join(name);
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .map_err(|e| format!("cannot create fresh fixture root: {e}"))?;
    let opened = oxide_storage::volume::dir_at(parent_fd.as_fd(), name, false)?;
    let held = fs::File::from(opened)
        .metadata()
        .map_err(|e| e.to_string())?;
    let named = fs::symlink_metadata(&root).map_err(|e| e.to_string())?;
    if named.dev() != held.dev() || named.ino() != held.ino() {
        return Err("fixture root was replaced during creation".into());
    }
    Ok(root)
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
    let removable_seed = register_fixture_volume(&internal, mount, uuid)?;
    let store = paired_store()?;
    let control = store.owner_control();
    let tx = fresh_nonce()?;
    store.initialize_outputs(&tx, None, &|tx, create| {
        create_or_validate_registry(control, tx, create)
    })?;
    let checkout =
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME missing")?).join("checkouts/anchor/c");
    let checkout_parent = checkout.parent().ok_or("checkout parent missing")?;
    ensure_private_dir(
        checkout_parent
            .parent()
            .ok_or("checkout container missing")?,
    )?;
    ensure_private_dir(checkout_parent)?;
    let checkout_uuid = checkout_volume_uuid(checkout.parent().ok_or("checkout parent missing")?)
        .ok_or("checkout volume unknown")?;
    let id = sha256_hex(checkout.as_os_str().as_bytes());
    let mut rows = Vec::new();
    let mut roots = Vec::new();
    for (suffix, generation, nonce, selected) in [
        ("old", 1, "b".repeat(32), false),
        ("successor", 2, "c".repeat(32), true),
    ] {
        let root = create_fixture_root(mount, &format!("{label}-{suffix}"))?;
        write_fixture_file(&root.join("payload-a"), b"disposable payload one")?;
        write_fixture_file(&root.join("payload-b"), b"disposable payload two")?;
        let marker = serde_json::to_vec(&serde_json::json!({"version":"oxide.outputs.root/2", "checkout":checkout, "checkout_id":id, "checkout_volume_uuid":checkout_uuid, "label":label, "recipe":outputs::RECIPE_PUBLIC, "kind":"external", "volume_uuid":uuid, "nonce":nonce, "generation":generation, "created_ms":1})).map_err(|e| e.to_string())?;
        write_fixture_file(&root.join(outputs::MARKER), &marker)?;
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
    write_fixture_file(
        &receipt_path,
        &serde_json::to_vec(&receipt).map_err(|e| e.to_string())?,
    )?;
    let mut registry = ready_registry(control)?;
    registry.rows = rows;
    registry.revision += 1;
    write_registry(control, &registry)?;
    drop(_publication);
    volume(uuid, mount, barge::registry::Durability::Expendable)?;
    grant(true)?;
    println!(
        "{}",
        serde_json::json!({"root":roots[0]["root"],"successor":roots[1]["root"],"checkout":checkout,"control":control,"receipt":outputs::receipt_path_public(control,&id),"uuid":uuid,"mount":mount,"peer_control":removable::removable_control(control),"checkout_volume_uuid":checkout_uuid,"checkout_ancestor_inode":fs::metadata(checkout.parent().ok_or("checkout parent missing")?).map_err(|e|e.to_string())?.ino(),"removable_seed":removable_seed})
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
            let os = barge::os::platform();
            let paths = paths()?;
            let mut locks = barge::state::LockSet::new(paths.clone());
            let view = barge::operation::consult_operations(
                &os,
                &os,
                &paths,
                &mut locks,
                barge::contract::INTENT_ROWS,
            )
            .map_err(|e| e.to_string())?;
            let records = view
                .as_ref()
                .map(|view| view.records.as_slice())
                .unwrap_or(&[]);
            println!(
                "{}",
                serde_json::json!({"oxide":registry,"barge":records,"journal_initialized":view.is_some()})
            );
            Ok(())
        }
        ["replay"] => {
            let store = paired_store()?;
            let roots = delegation::drain(&store, store.owner_control(), None)?;
            println!("{}", serde_json::json!({"completed_roots": roots}));
            Ok(())
        }
        ["journal-fault", kind @ ("missing" | "gap")] => {
            let state = paths()?;
            let os = barge::os::platform();
            let view =
                barge::operation::read_operations(&os, &os, &state, barge::contract::INTENT_ROWS)
                    .map_err(|e| e.to_string())?
                    .ok_or("journal missing before fault")?;
            if *kind == "missing" {
                fs::rename(
                    state.operations_dir(),
                    state.root().join("retained-operations"),
                )
                .map_err(|e| e.to_string())?;
            } else {
                let record = view
                    .records
                    .first()
                    .ok_or("no record for the gap fixture")?;
                let file = state
                    .operations_dir()
                    .join(format!("{}.json", record.sequence));
                fs::rename(file, state.root().join("retained-gap-record.json"))
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        ["domain-fault", root, kind] => {
            let store = paired_store()?;
            let control = store.owner_control();
            let _guard = store::publication_guard(control)?;
            let mut registry = ready_registry(control)?;
            let target = Path::new(root);
            let evidence = proof_evidence().ok_or("current guard missing")?;
            let image: serde_json::Value = serde_json::from_slice(
                &fs::read(evidence.join("delegation-image.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let mount = Path::new(image["mount"].as_str().ok_or("image mount missing")?);
            let _binding = Binding::open(mount, image["volume_uuid"].as_str())?;
            if target.parent() != Some(mount.join(outputs::EXTERNAL_DIR).as_path()) {
                return Err("fault root escapes current image".to_owned());
            }
            let first = registry
                .rows
                .iter()
                .position(|row| row.root == target)
                .ok_or("fault group missing")?;
            match *kind {
                "partial" => {
                    registry.rows.remove(first);
                }
                "absent" => registry.rows.retain(|row| row.root != target),
                "extra" => {
                    let mut row = registry.rows[first].clone();
                    row.member = "extra".to_owned();
                    registry.rows.push(row);
                }
                "foreign" => {
                    let mut row = registry.rows[first].clone();
                    row.checkout_id = "f".repeat(64);
                    row.selected = true;
                    registry.rows.push(row);
                }
                "selected" => registry.rows[first].selected = true,
                "nonce" => registry.rows[first].nonce = Some("f".repeat(32)),
                "receipt" => {
                    fs::write(
                        &registry.rows[first].receipt,
                        b"labelled private receipt fault",
                    )
                    .map_err(|e| e.to_string())?;
                    return Ok(());
                }
                _ => return Err("unknown domain fault".to_owned()),
            }
            registry.bump()?;
            registry.validate()?;
            let temp = registry_dir(control).join(format!(".domain-fault-{}", fresh_nonce()?));
            write_fixture_file(
                &temp,
                &serde_json::to_vec(&registry).map_err(|e| e.to_string())?,
            )?;
            fs::rename(temp, registry_path(control)).map_err(|e| e.to_string())?;
            sync_directory(&registry_dir(control))
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

#[cfg(test)]
mod tests {
    #[test]
    fn fixture_construction_passes_real_private_gates_and_preserves_refusals() {
        use super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};

        // Tiny owner-private ordinary fixture, no image, engine or process survey.
        // Retain it under the test runner's selected TMPDIR; never touch old roots.
        let output = std::process::Command::new("/usr/bin/mktemp")
            .arg("-d")
            .arg(std::env::temp_dir().join("delegation-permissions.XXXXXXXX"))
            .output()
            .unwrap();
        assert!(output.status.success());
        let base = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        let root = create_fixture_root(&base, "valid-old").unwrap();
        let parent = base.join(outputs::EXTERNAL_DIR);
        for path in [&parent, &root] {
            let file = store::open_control_directory(path).unwrap().unwrap();
            assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o700);
        }
        let marker = root.join(outputs::MARKER);
        write_fixture_file(&marker, b"bounded fixture marker").unwrap();
        let file = store::private_file(&marker, false).unwrap().unwrap();
        assert_eq!(file.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(write_fixture_file(&marker, b"replacement").is_err());
        assert_eq!(fs::read(&marker).unwrap(), b"bounded fixture marker");

        // A second world can use the valid container, but never reuse a root.
        create_fixture_root(&base, "valid-successor").unwrap();
        assert!(create_fixture_root(&base, "valid-old").is_err());
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(create_fixture_root(&base, "wrong-mode")
            .unwrap_err()
            .contains("owner-only"));
        assert_eq!(fs::metadata(&parent).unwrap().mode() & 0o777, 0o755);
        assert!(!parent.join("wrong-mode").exists());
        // Restore only this test's deliberate corruption to exercise later inputs.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let held_parent = File::open(&parent).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(oxide_storage::volume::dir_at(held_parent.as_fd(), "valid-old", false).is_err());
        assert!(create_fixture_root(&base, "valid-old").is_err());
        assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o755);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let held_root =
            oxide_storage::volume::dir_at(held_parent.as_fd(), "valid-old", false).unwrap();
        fs::rename(&root, parent.join("retained-old")).unwrap();
        symlink(parent.join("retained-old"), &root).unwrap();
        assert!(oxide_storage::volume::dir_at(held_parent.as_fd(), "valid-old", false).is_err());
        assert!(create_fixture_root(&base, "valid-old").is_err());
        assert_eq!(
            fs::File::from(held_root).metadata().unwrap().ino(),
            fs::metadata(parent.join("retained-old")).unwrap().ino()
        );
        fs::write(parent.join("foreign-file"), b"keep").unwrap();
        assert!(create_fixture_root(&base, "foreign-file").is_err());
        assert!(create_fixture_root(&base, "../escape").is_err());
        let linked_mount = base.join("linked-mount");
        symlink(&base, &linked_mount).unwrap();
        assert!(create_fixture_root(&linked_mount, "linked").is_err());
        fs::rename(&parent, base.join("retained-container")).unwrap();
        symlink(base.join("retained-container"), &parent).unwrap();
        assert!(create_fixture_root(&base, "linked-container").is_err());
        assert_eq!(
            fs::read(base.join("retained-container/foreign-file")).unwrap(),
            b"keep"
        );
    }
}
