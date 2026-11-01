//! Private, bounded observations for the installed-handoff proof only.

#[cfg(debug_assertions)]
mod debug {
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
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

    fn canonical(name: &str) -> Option<PathBuf> {
        let path = PathBuf::from(std::env::var_os(name)?);
        (path.is_absolute() && fs::canonicalize(&path).ok()? == path).then_some(path)
    }

    fn open() -> Option<Mutex<Trace>> {
        if std::env::var_os("OXIDE_HANDOFF_TRACE")?.as_os_str() != "1"
            || std::env::var_os("OXIDE_TEST_HOME_FROM_ENV")?.as_os_str() != "1"
        {
            return None;
        }
        let scratch = canonical("OXIDE_TASK_SCRATCH_ROOT")?;
        let evidence = canonical("OXIDE_PROOF_EVIDENCE")?;
        let home = canonical("HOME")?;
        let fixture = home.parent()?;
        let fixture_root = canonical("OXIDE_SCRATCH_ROOT")?;
        let root = scratch.parent()?;
        if scratch.file_name()? != "scratch"
            || evidence != root.join("evidence")
            || !fixture.starts_with(&fixture_root)
            || !fixture_root.starts_with(&scratch)
            || Path::new(&std::env::var_os("OXIDE_POOL")?) != fixture.join("pool")
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
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(marker_path).ok()?).ok()?;
        let run = std::env::var("OXIDE_PROOF_RUN_ID").ok()?;
        if run.len() != 32
            || !run.bytes().all(|b| b.is_ascii_hexdigit())
            || marker["run_id"].as_str()? != run
            || marker["root"].as_str()? != root.to_str()?
            || marker["owner_uid"].as_u64()? != u64::from(info.uid())
            || marker["device"].as_u64()? != info.dev()
            || marker["inode"].as_u64()? != info.ino()
            || marker["outcome"].as_str()? != "running"
            || !marker["concluded_unix"].is_null()
        {
            return None;
        }
        let path = canonical("OXIDE_HANDOFF_TRACE_FILE")?;
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
        if trace.count >= 64 {
            return;
        }
        trace.count += 1;
        let nanos = trace.clock.elapsed().as_nanos();
        let _ = trace.file.write_all(line(name, end, nanos).as_bytes());
    }
    #[cfg(test)]
    mod tests {
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
