//! Private, bounded observations for the installed-handoff proof only.

#[cfg(debug_assertions)]
mod debug {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

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

    fn open() -> Option<Mutex<Trace>> {
        open_with(&|name| std::env::var_os(name))
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
            .open(marker_path)
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
        let run = get("OXIDE_PROOF_RUN_ID")?.into_string().ok()?;
        if run.len() != 32
            || !run.bytes().all(|b| b.is_ascii_hexdigit())
            || marker["run_id"].as_str()? != run
            || marker["root"].as_str()? != root.to_str()?
            || marker["owner_uid"].as_u64()? != u64::from(info.uid())
            || marker["device"].as_u64()? != info.dev()
            || marker["inode"].as_u64()? != info.ino()
            || marker["version"].as_u64()? != 1
            || marker["outcome"].as_str()? != "running"
            || marker["closure"].as_str()? != "none"
            || marker.get("concluded_unix")? != &serde_json::Value::Null
        {
            return None;
        }
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
        #[test]
        fn private_layout_is_required_and_events_are_bounded() {
            use std::collections::HashMap;
            use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
            let base =
                std::env::temp_dir().join(format!("handoff-trace-test-{}", std::process::id()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&base)
                .unwrap();
            let scratch = base.join("scratch");
            let evidence = base.join("evidence");
            let fixture = scratch.join("fixture");
            let home = fixture.join("case/home");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir(&evidence).unwrap();
            let info = std::fs::metadata(&base).unwrap();
            let run = "a".repeat(32);
            let marker = serde_json::json!({"run_id":run, "root":base, "owner_uid":info.uid(),
                "device":info.dev(), "inode":info.ino(), "version":1, "outcome":"running", "closure":"none", "concluded_unix":null});
            let marker_path = base.join(".oxide-scratch.json");
            std::fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();
            std::fs::set_permissions(&marker_path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let path = evidence.join("handoff-trace.test");
            std::fs::write(&path, b"").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let mut env = HashMap::<&str, std::ffi::OsString>::new();
            env.insert("OXIDE_HANDOFF_TRACE", "1".into());
            env.insert("OXIDE_TEST_HOME_FROM_ENV", "1".into());
            env.insert("OXIDE_TASK_SCRATCH_ROOT", scratch.into_os_string());
            env.insert("OXIDE_PROOF_EVIDENCE", evidence.into_os_string());
            env.insert("OXIDE_SCRATCH_ROOT", fixture.into_os_string());
            env.insert(
                "OXIDE_POOL",
                home.parent().unwrap().join("pool").into_os_string(),
            );
            env.insert("HOME", home.into_os_string());
            env.insert("OXIDE_PROOF_RUN_ID", "a".repeat(32).into());
            env.insert("OXIDE_HANDOFF_TRACE_FILE", path.clone().into_os_string());
            let get = |key: &str| env.get(key).cloned();
            let trace = super::open_with(&get).expect("private layout");
            {
                let mut trace = trace.lock().unwrap();
                for _ in 0..70 {
                    super::emit(&mut trace, "store", false);
                }
            }
            drop(trace);
            let lines = std::fs::read_to_string(&path).unwrap();
            let events: Vec<_> = lines.lines().collect();
            assert_eq!(events.len(), 64);
            assert!(events.iter().all(|line| line.starts_with("store start ")));
            let times: Vec<u128> = events
                .iter()
                .map(|line| line.rsplit_once(' ').unwrap().1.parse().unwrap())
                .collect();
            assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
            assert!(super::open_with(&get).is_none(), "nonempty file is refused");
            std::fs::write(&path, b"").unwrap();
            env.remove("OXIDE_HANDOFF_TRACE");
            assert!(super::open_with(&|key| env.get(key).cloned()).is_none());
            env.insert("OXIDE_HANDOFF_TRACE", "1".into());
            env.insert("OXIDE_PROOF_RUN_ID", "wrong".into());
            assert!(super::open_with(&|key| env.get(key).cloned()).is_none());
            std::fs::remove_dir_all(base).unwrap();
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
    #[test]
    fn phase_preserves_success_and_error() {
        assert_eq!(super::phase("test", || 42), 42);
        assert_eq!(super::phase("test", || Err::<(), _>("error")), Err("error"));
    }
}
