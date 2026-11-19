//! Private, bounded observations for the installed-handoff proof only.
//!
//! Activation binds two independent supervisor identities to their own real
//! sources and never equates them: the exported session token
//! (`OXIDE_PROOF_RUN_ID`) must equal the guard's published receipt `run_id`,
//! while the scratch marker's `run_id` must equal that receipt's
//! `scratch_marker.run_id` (the monitor generates the two tokens
//! independently).

#[cfg(debug_assertions)]
mod debug {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    /// The guard receipt cap. The published document is bounded by its own
    /// schema (census rows, admission records and captured output are all
    /// capped); 64 KiB leaves a wide margin over observed publications and
    /// refuses an oversized or substituted file instead of parsing a partial
    /// read.
    const RECEIPT_MAX_BYTES: u64 = 64 * 1024;

    struct Trace {
        file: File,
        clock: Instant,
        count: u8,
    }
    static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();

    fn canonical(get: &impl Fn(&str) -> Option<std::ffi::OsString>, name: &str) -> Option<PathBuf> {
        let path = PathBuf::from(get(name)?);
        (path.is_absolute() && fs::canonicalize(&path).ok()? == path).then_some(path)
    }

    /// Both the supervisor session token and the marker token are 32 hex
    /// characters drawn independently by the monitor.
    fn hex32(value: &str) -> bool {
        value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
    }

    /// One string field of a JSON document.
    fn text<'a>(document: &'a serde_json::Value, key: &str) -> Option<&'a str> {
        document.get(key)?.as_str()
    }

    fn open() -> Option<Mutex<Trace>> {
        open_with(&|name| std::env::var_os(name))
    }

    /// One bounded no-follow snapshot of the guard's published receipt at
    /// `<evidence>/receipt.json`. The monitor replaces that path atomically
    /// (`receipt.json.tmp` + rename) at each phase, so one open descriptor
    /// always reads exactly one whole publication; a snapshot whose identity
    /// changed between `lstat` and `open`, a symlink, a foreign owner, an
    /// oversized file, or a truncated or malformed document refuses. There is
    /// no retry: any refused receipt disables the trace for this process.
    fn read_receipt(evidence: &Path, owner: u32) -> Option<serde_json::Value> {
        let path = evidence.join("receipt.json");
        let before = fs::symlink_metadata(&path).ok()?;
        if !before.is_file() || before.uid() != owner || before.len() > RECEIPT_MAX_BYTES {
            return None;
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .ok()?;
        let opened = file.metadata().ok()?;
        if (before.dev(), before.ino()) != (opened.dev(), opened.ino()) {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(RECEIPT_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() as u64 > RECEIPT_MAX_BYTES {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    }

    /// Bind the two independent run identities to their own sources and the
    /// receipt to the already verified private topology. The exported session
    /// token equals the receipt's `run_id`; the marker's token equals the
    /// receipt's `scratch_marker.run_id`; the two tokens are never equated
    /// with each other.
    fn receipt_binds(
        receipt: &serde_json::Value,
        session: &str,
        root: &Path,
        scratch: &Path,
        evidence: &Path,
        marker: &serde_json::Value,
    ) -> Option<()> {
        if text(receipt, "run_id")? != session
            || text(receipt, "root")? != root.to_str()?
            || text(receipt, "scratch")? != scratch.to_str()?
            || text(receipt, "evidence")? != evidence.to_str()?
            || text(receipt, "phase")?.is_empty()
        {
            return None;
        }
        let scratch_marker = receipt.get("scratch_marker")?;
        if text(scratch_marker, "run_id")? != text(marker, "run_id")?
            || text(scratch_marker, "path")? != root.join(".oxide-scratch.json").to_str()?
        {
            return None;
        }
        Some(())
    }

    fn open_with(get: &impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<Mutex<Trace>> {
        if get("OXIDE_HANDOFF_TRACE")?.as_os_str() != "1"
            || get("OXIDE_TEST_HOME_FROM_ENV")?.as_os_str() != "1"
        {
            return None;
        }
        let scratch = canonical(get, "OXIDE_TASK_SCRATCH_ROOT")?;
        let evidence = canonical(get, "OXIDE_PROOF_EVIDENCE")?;
        let home = canonical(get, "HOME")?;
        let fixture = home.parent()?;
        let fixture_root = canonical(get, "OXIDE_SCRATCH_ROOT")?;
        let root = scratch.parent()?;
        if scratch.file_name()? != "scratch"
            || evidence != root.join("evidence")
            || !fixture.starts_with(&fixture_root)
            || !fixture_root.starts_with(&scratch)
            || Path::new(&get("OXIDE_POOL")?) != fixture.join("pool")
        {
            return None;
        }
        let info = fs::symlink_metadata(root).ok()?;
        let marker_path = root.join(".oxide-scratch.json");
        let marker_info = fs::symlink_metadata(&marker_path).ok()?;
        if !info.is_dir()
            || info.mode() & 0o077 != 0
            || !marker_info.is_file()
            || marker_info.uid() != info.uid()
            || marker_info.mode() & 0o077 != 0
            || marker_info.len() > 4096
        {
            return None;
        }
        let mut bytes = Vec::new();
        let marker_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&marker_path)
            .ok()?;
        let opened_marker = marker_file.metadata().ok()?;
        if (marker_info.dev(), marker_info.ino()) != (opened_marker.dev(), opened_marker.ino()) {
            return None;
        }
        marker_file.take(4097).read_to_end(&mut bytes).ok()?;
        if bytes.len() > 4096 {
            return None;
        }
        let marker: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let session = get("OXIDE_PROOF_RUN_ID")?.into_string().ok()?;
        if !hex32(&session)
            || marker["root"].as_str()? != root.to_str()?
            || marker["owner_uid"].as_u64()? != u64::from(info.uid())
            || marker["device"].as_u64()? != info.dev()
            || marker["inode"].as_u64()? != info.ino()
            || marker["version"].as_u64()? != 1
            || marker["outcome"].as_str()? != "running"
            || marker["closure"].as_str()? != "none"
            || marker.get("concluded_unix")? != &serde_json::Value::Null
            || !hex32(marker["run_id"].as_str()?)
        {
            return None;
        }
        receipt_binds(
            &read_receipt(&evidence, info.uid())?,
            &session,
            &root,
            &scratch,
            &evidence,
            &marker,
        )?;
        let path = canonical(get, "OXIDE_HANDOFF_TRACE_FILE")?;
        if path.parent()? != evidence || !path.file_name()?.to_str()?.starts_with("handoff-trace.")
        {
            return None;
        }
        let before = fs::symlink_metadata(&path).ok()?;
        if !before.is_file()
            || before.uid() != info.uid()
            || before.mode() & 0o077 != 0
            || before.len() != 0
        {
            return None;
        }
        let file = OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .ok()?;
        let after = file.metadata().ok()?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return None;
        }
        Some(Mutex::new(Trace {
            file,
            clock: Instant::now(),
            count: 0,
        }))
    }

    fn line(name: &str, end: bool, nanos: u128) -> String {
        format!("{} {} {}\n", name, if end { "end" } else { "start" }, nanos)
    }

    pub(super) fn event(name: &'static str, end: bool) {
        let Some(trace) = TRACE.get_or_init(open).as_ref() else {
            return;
        };
        let Ok(mut trace) = trace.lock() else { return };
        emit(&mut trace, name, end);
    }

    fn emit(trace: &mut Trace, name: &'static str, end: bool) {
        if trace.count < 64 {
            trace.count += 1;
            let nanos = trace.clock.elapsed().as_nanos();
            let _ = trace.file.write_all(line(name, end, nanos).as_bytes());
        }
    }
    #[cfg(test)]
    mod tests {
        use std::collections::HashMap;
        use std::ffi::OsString;
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        use std::path::PathBuf;

        /// Two deliberately different valid tokens: the receipt's session
        /// identity and the marker's own identity, as the qualified monitor
        /// generates them independently.
        const SESSION: &str = "9f13c2a47b8d0e5623ab4cd6ef901257";
        const MARKER_RUN: &str = "5d8a02be71c49f3ea6d80b29c5147f36";

        struct Layout {
            env: HashMap<&'static str, OsString>,
            base: PathBuf,
            evidence: PathBuf,
            trace: PathBuf,
        }

        /// One private supervisor-like layout bound to the qualified schema:
        /// the run root carries the real 12-field marker document and the
        /// evidence directory carries the guard's initial published receipt
        /// with its real initial field set, whose `run_id` (session token)
        /// and `scratch_marker.run_id` (marker token) differ deliberately.
        fn layout(tag: &str) -> Layout {
            let base =
                std::env::temp_dir().join(format!("handoff-trace-{tag}-{}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&base)
                .unwrap();
            let scratch = base.join("scratch");
            let evidence = base.join("evidence");
            let fixture_root = scratch.join("installed-handoff.fixture");
            let home = fixture_root.join("case/home");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir(&evidence).unwrap();
            let info = std::fs::metadata(&base).unwrap();
            let text = |path: &std::path::Path| path.to_string_lossy().to_string();
            let marker_path = base.join(".oxide-scratch.json");
            let marker = serde_json::json!({
                "version": 1, "run_id": MARKER_RUN, "root": text(&base),
                "owner_uid": info.uid(), "device": info.dev(), "inode": info.ino(),
                "created_unix": 1_700_000_000.0, "concluded_unix": null,
                "outcome": "running", "closure": "none", "kept_reason": "none",
                "nested_scope": "exclusive"
            });
            std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
            std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let receipt = serde_json::json!({
                "run_id": SESSION,
                "command": ["python3", "scripts/example-handoff.py", "--diagnostic"],
                "limit_s": 600.0, "root": text(&base), "scratch": text(&scratch),
                "evidence": text(&evidence), "scratch_owned": true, "start_ns": 1,
                "abort_threshold": 1, "terminal_mode": {"mode": "strict", "reason": "r", "evidence": "e"},
                "ancestors": {"state": "recorded", "count": 0, "identities": [], "stop": null},
                "ancestor_exclusions": {"rows": [], "decisions": 0, "omitted": 0},
                "scratch_default": {"source": "selected-volume", "root_parent": text(&base.parent().unwrap())},
                "scratch_sweep": null,
                "scratch_marker": {"path": text(&marker_path), "state": "written",
                                   "run_id": MARKER_RUN, "reason": null},
                "phase": "started: incomplete", "leader": null, "custodian": [],
                "outcome": null, "census": null, "census_rounds": 0,
                "settlement": {"schema": 1, "clock": "CLOCK_UPTIME_RAW"},
                "survivors": [], "published_ns": 2,
                "reserve": {"samples": 1, "minimum": 1, "max_gap_ns": 0},
                "owned_bytes": {"samples": 1, "maximum": 1, "budget": 1, "last_ns": 1},
                "output": {}, "admission": {"schema": 1}, "admission_waiting": {"schema": 1}
            });
            let receipt_path = evidence.join("receipt.json");
            std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            std::fs::set_permissions(&receipt_path, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            let trace = evidence.join("handoff-trace.test");
            std::fs::write(&trace, b"").unwrap();
            std::fs::set_permissions(&trace, std::fs::Permissions::from_mode(0o600)).unwrap();
            let mut env = HashMap::<&str, OsString>::new();
            env.insert("OXIDE_HANDOFF_TRACE", "1".into());
            env.insert("OXIDE_TEST_HOME_FROM_ENV", "1".into());
            env.insert("OXIDE_TASK_SCRATCH_ROOT", scratch.into_os_string());
            env.insert("OXIDE_PROOF_EVIDENCE", evidence.clone().into_os_string());
            env.insert("OXIDE_SCRATCH_ROOT", fixture_root.into_os_string());
            env.insert(
                "OXIDE_POOL",
                home.parent().unwrap().join("pool").into_os_string(),
            );
            env.insert("HOME", home.into_os_string());
            env.insert("OXIDE_PROOF_RUN_ID", SESSION.into());
            env.insert("OXIDE_HANDOFF_TRACE_FILE", trace.clone().into_os_string());
            Layout {
                env,
                base,
                evidence,
                trace,
            }
        }

        impl Layout {
            fn get(&self) -> impl Fn(&str) -> Option<OsString> + '_ {
                move |key: &str| self.env.get(key).cloned()
            }

            /// Replant `receipt.json` from the guard's initial publication
            /// with one mutation applied.
            fn plant_receipt(&self, original: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) {
                let mut receipt: serde_json::Value = serde_json::from_slice(original).unwrap();
                mutate(&mut receipt);
                let path = self.evidence.join("receipt.json");
                std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            }
        }

        impl Drop for Layout {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.base).unwrap();
            }
        }

        #[test]
        fn two_distinct_tokens_enable_and_events_are_bounded() {
            let layout = layout("positive");
            let trace = super::open_with(&layout.get())
                .expect("two distinct valid tokens enable the trace");
            {
                let mut trace = trace.lock().unwrap();
                for _ in 0..70 {
                    super::emit(&mut trace, "store", false);
                }
            }
            drop(trace);
            let lines = std::fs::read_to_string(&layout.trace).unwrap();
            let events: Vec<_> = lines.lines().collect();
            assert_eq!(events.len(), 64);
            assert!(events.iter().all(|line| line.starts_with("store start ")));
            let times: Vec<u128> = events
                .iter()
                .map(|line| line.rsplit_once(' ').unwrap().1.parse().unwrap())
                .collect();
            assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
            assert!(
                super::open_with(&layout.get()).is_none(),
                "nonempty file is refused"
            );
            let mut env = layout.env.clone();
            env.remove("OXIDE_HANDOFF_TRACE");
            assert!(super::open_with(&|key| env.get(key).cloned()).is_none());
            env.insert("OXIDE_HANDOFF_TRACE", "1".into());
            env.remove("OXIDE_PROOF_RUN_ID");
            assert!(
                super::open_with(&|key| env.get(key).cloned()).is_none(),
                "missing token"
            );
            env.insert("OXIDE_PROOF_RUN_ID", "zz".into());
            assert!(
                super::open_with(&|key| env.get(key).cloned()).is_none(),
                "malformed token"
            );
            env.insert("OXIDE_PROOF_RUN_ID", MARKER_RUN.into());
            assert!(
                super::open_with(&|key| env.get(key).cloned()).is_none(),
                "the marker token is never a session token"
            );
        }

        #[test]
        fn receipt_and_marker_bindings_refuse_every_mismatch() {
            let layout = layout("bindings");
            let get = layout.get();
            let receipt_path = layout.evidence.join("receipt.json");
            let original = std::fs::read(&receipt_path).unwrap();
            let restore = |original: &[u8]| {
                std::fs::write(&receipt_path, original).unwrap();
                std::fs::set_permissions(&receipt_path, std::fs::Permissions::from_mode(0o644))
                    .unwrap();
            };
            assert!(super::open_with(&get).is_some(), "the clean layout enables");
            // Swapped tokens: the receipt's session slot carries the marker token.
            layout.plant_receipt(&original, |receipt| receipt["run_id"] = MARKER_RUN.into());
            assert!(
                super::open_with(&get).is_none(),
                "swapped session token refused"
            );
            restore(&original);
            // The marker binds to its own receipt field, not to the session token.
            layout.plant_receipt(&original, |receipt| {
                receipt["scratch_marker"]["run_id"] = SESSION.into()
            });
            assert!(
                super::open_with(&get).is_none(),
                "marker bound to the wrong token refused"
            );
            restore(&original);
            // The receipt must name this verified topology.
            for key in ["root", "scratch", "evidence"] {
                layout.plant_receipt(&original, |receipt| receipt[key] = "elsewhere".into());
                assert!(super::open_with(&get).is_none(), "wrong {key} refused");
                restore(&original);
            }
            // An unpublished receipt snapshot refuses.
            layout.plant_receipt(&original, |receipt| {
                receipt["phase"] = serde_json::Value::Null
            });
            assert!(
                super::open_with(&get).is_none(),
                "unpublished phase refused"
            );
            restore(&original);
            // Missing, malformed, oversized and symlinked receipts refuse.
            std::fs::remove_file(&receipt_path).unwrap();
            assert!(super::open_with(&get).is_none(), "missing receipt refused");
            std::fs::write(&receipt_path, b"{not json").unwrap();
            assert!(
                super::open_with(&get).is_none(),
                "malformed receipt refused"
            );
            std::fs::write(&receipt_path, vec![b'x'; 65 * 1024]).unwrap();
            assert!(
                super::open_with(&get).is_none(),
                "oversized receipt refused"
            );
            restore(&original);
            let link = layout.evidence.join("receipt-link");
            std::os::unix::fs::symlink(&receipt_path, &link).unwrap();
            std::fs::rename(&link, &receipt_path).unwrap();
            assert!(
                super::open_with(&get).is_none(),
                "symlinked receipt refused"
            );
            std::fs::remove_file(&receipt_path).unwrap();
            restore(&original);
            // The scratch_marker must name this run root's marker file.
            layout.plant_receipt(&original, |receipt| {
                receipt["scratch_marker"]["path"] = "elsewhere".into()
            });
            assert!(
                super::open_with(&get).is_none(),
                "wrong marker path refused"
            );
            restore(&original);
            // A missing scratch_marker object refuses.
            layout.plant_receipt(&original, |receipt| {
                receipt["scratch_marker"] = serde_json::Value::Null
            });
            assert!(
                super::open_with(&get).is_none(),
                "missing scratch_marker refused"
            );
            restore(&original);
        }

        #[test]
        fn marker_identity_facts_must_match_the_root() {
            let layout = layout("marker-facts");
            let get = layout.get();
            assert!(super::open_with(&get).is_some(), "the clean layout enables");
            let marker_path = layout.base.join(".oxide-scratch.json");
            let original = std::fs::read(&marker_path).unwrap();
            for (key, value) in [
                ("device", serde_json::Value::from(u64::MAX)),
                ("inode", serde_json::Value::from(0)),
                ("owner_uid", serde_json::Value::from(u64::MAX)),
                ("root", serde_json::Value::from("elsewhere")),
                ("version", serde_json::Value::from(2)),
                ("outcome", serde_json::Value::from("closed")),
                ("run_id", serde_json::Value::from("short")),
            ] {
                let mut marker: serde_json::Value = serde_json::from_slice(&original).unwrap();
                marker[key] = value;
                std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
                std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
                assert!(super::open_with(&get).is_none(), "{key} mismatch refused");
                std::fs::write(&marker_path, &original).unwrap();
                std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
        }

        #[test]
        fn events_use_one_monotonic_nanosecond_scale() {
            assert_eq!(super::line("store", false, 12), "store start 12\n");
            assert_eq!(super::line("store", true, 99), "store end 99\n");
        }
    }
}

pub(crate) fn phase<T>(name: &'static str, action: impl FnOnce() -> T) -> T {
    #[cfg(debug_assertions)]
    debug::event(name, false);
    #[cfg(not(debug_assertions))]
    let _ = name;
    let result = action();
    #[cfg(debug_assertions)]
    debug::event(name, true);
    result
}

#[cfg(test)]
mod tests {
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_child_has_no_trace() {
        let Some(path) = std::env::var_os("OXIDE_HANDOFF_TRACE_FILE") else {
            return;
        };
        assert_eq!(super::phase("release-test", || 42), 42);
        assert!(std::fs::read(path).unwrap().is_empty());
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_trace_is_disabled_even_with_private_layout() {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!("release-trace-{}", std::process::id()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let scratch = root.join("scratch");
        let evidence = root.join("evidence");
        let fixture = scratch.join("fixture");
        let home = fixture.join("case/home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir(&evidence).unwrap();
        let info = std::fs::metadata(&root).unwrap();
        // The real two-token schema: the marker carries the monitor's own
        // token and the guard receipt binds it to `scratch_marker.run_id`,
        // while `OXIDE_PROOF_RUN_ID` carries the distinct session token.
        const SESSION: &str = "9f13c2a47b8d0e5623ab4cd6ef901257";
        const MARKER_RUN: &str = "5d8a02be71c49f3ea6d80b29c5147f36";
        let marker_path = root.join(".oxide-scratch.json");
        let marker = serde_json::json!({
            "version": 1, "run_id": MARKER_RUN, "root": root.to_string_lossy(),
            "owner_uid": info.uid(), "device": info.dev(), "inode": info.ino(),
            "created_unix": 1_700_000_000.0, "concluded_unix": null,
            "outcome": "running", "closure": "none", "kept_reason": "none",
            "nested_scope": "exclusive"
        });
        std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
        std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let receipt = serde_json::json!({
            "run_id": SESSION, "root": root.to_string_lossy(),
            "scratch": scratch.to_string_lossy(), "evidence": evidence.to_string_lossy(),
            "scratch_owned": true, "start_ns": 1, "phase": "started: incomplete",
            "scratch_marker": {"path": marker_path.to_string_lossy(), "state": "written",
                               "run_id": MARKER_RUN, "reason": null}
        });
        std::fs::write(
            evidence.join("receipt.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let trace = evidence.join("handoff-trace.release");
        std::fs::write(&trace, b"").unwrap();
        std::fs::set_permissions(&trace, std::fs::Permissions::from_mode(0o600)).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "handoff_trace::tests::release_child_has_no_trace",
            ])
            .env("OXIDE_HANDOFF_TRACE", "1")
            .env("OXIDE_TEST_HOME_FROM_ENV", "1")
            .env("OXIDE_TASK_SCRATCH_ROOT", &scratch)
            .env("OXIDE_PROOF_EVIDENCE", &evidence)
            .env("OXIDE_SCRATCH_ROOT", &fixture)
            .env("OXIDE_POOL", home.parent().unwrap().join("pool"))
            .env("HOME", &home)
            .env("OXIDE_PROOF_RUN_ID", SESSION)
            .env("OXIDE_HANDOFF_TRACE_FILE", &trace)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
        assert!(std::fs::read(trace).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn phase_preserves_success_and_error() {
        assert_eq!(super::phase("test", || 42), 42);
        assert_eq!(super::phase("test", || Err::<(), _>("error")), Err("error"));
    }
}
