//! `oxide status --launch-check` lifecycle (oxide#093), on the real binary.
//!
//! Each case runs the CLI to exit, then checks at once, with the CLI gone,
//! that its temporary directory is empty and no probe it started survives.
//! Probes are shell scripts named by `OXIDE_TEST_LAUNCH_SOURCE`; a freshly
//! written script pays the same first-launch cost as a binary. No penalty
//! size is asserted.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const OXIDE: &str = env!("CARGO_BIN_EXE_oxide");
/// Scheduling slack above a limit on a loaded host.
const SLACK: Duration = Duration::from_millis(400);
const QUICK: &str = "#!/bin/sh\nexit 0\n";
/// Records its PID, then never exits on its own.
const SLOW: &str = "#!/bin/sh\necho $$ >> PIDS\nexec sleep 30\n";

struct Run {
    stdout: String,
    wall: Duration,
    tmp: PathBuf,
    pids: PathBuf,
}

fn scratch(label: &str) -> PathBuf {
    let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = base.join(format!("launch-check-{label}-{}", std::process::id()));
    std::fs::create_dir_all(root.join("tmp")).expect("launch scratch");
    root
}

fn check(label: &str, script: &str, environment: &[(&str, &str)]) -> Run {
    let root = scratch(label);
    let tmp = root.join("tmp");
    let pids = root.join("pids");
    let source = root.join("probe");
    let script = script.replace("PIDS", &pids.display().to_string());
    std::fs::write(&source, script).expect("probe script");
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o700)).expect("mode");
    // Absorb this freshly built binary's own first-launch cost.
    let warm = Command::new(OXIDE)
        .arg("__oxide-launch-probe")
        .status()
        .expect("probe");
    assert!(warm.success());
    let started = Instant::now();
    let output = Command::new(OXIDE)
        .args(["status", "--launch-check"])
        .env("TMPDIR", &tmp)
        .env("PATH", root.join("no-bin"))
        .env("OXIDE_TEST_LAUNCH_SOURCE", &source)
        .envs(environment.iter().copied())
        .output()
        .expect("oxide status --launch-check");
    let wall = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{label}: {output:?}");
    assert!(output.stderr.is_empty(), "{label}: {output:?}");
    Run {
        stdout,
        wall,
        tmp,
        pids,
    }
}

/// Nothing left in the temporary directory once the CLI has exited.
fn assert_clean(run: &Run, label: &str) {
    let left: Vec<_> = std::fs::read_dir(&run.tmp)
        .expect("tmp")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert!(left.is_empty(), "{label}: left behind {left:?}");
    assert!(
        run.stdout.contains("; temporary copies removed\n"),
        "{label}: {}",
        run.stdout
    );
}

fn logged_pids(path: &Path) -> Vec<i32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[test]
fn an_ordinary_check_completes_and_cleans_up() {
    let run = check("ordinary", QUICK, &[]);
    assert!(
        run.wall < Duration::from_secs(3) + SLACK,
        "took {:?}",
        run.wall
    );
    assert!(run.stdout.contains("  medians: same file "), "{}", run.stdout);
    assert_clean(&run, "ordinary");
}

#[test]
fn a_probe_that_never_exits_is_killed_before_the_cli_exits() {
    let run = check("slow", SLOW, &[("OXIDE_TEST_LAUNCH_LIMIT_MS", "600")]);
    assert!(
        run.wall < Duration::from_millis(600) + SLACK,
        "took {:?}",
        run.wall
    );
    assert!(
        run.stdout
            .contains("result: inconclusive (the 600 ms limit was reached); no median reported"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("medians:"));
    assert_clean(&run, "slow");
    let pids = logged_pids(&run.pids);
    assert!(!pids.is_empty(), "the slow probe never ran");
    for pid in pids {
        // SAFETY: signal 0 only checks existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "probe {pid} outlived the check");
    }
}

#[test]
fn a_blocked_copy_start_or_removal_cannot_outlive_the_cli() {
    for delay in ["copy:2000", "start:2000", "remove:2000"] {
        // The start delay holds a running slow probe; the others hold a
        // created copy.
        let script = if delay.starts_with("start") { SLOW } else { QUICK };
        let run = check(
            delay.split(':').next().unwrap_or("delay"),
            script,
            &[
                ("OXIDE_TEST_LAUNCH_LIMIT_MS", "600"),
                ("OXIDE_TEST_LAUNCH_DELAY", delay),
            ],
        );
        assert!(
            run.wall < Duration::from_millis(600) + SLACK,
            "{delay}: took {:?}",
            run.wall
        );
        assert!(
            run.stdout.contains("result: inconclusive (the 600 ms limit was reached)"),
            "{delay}: {}",
            run.stdout
        );
        assert_clean(&run, delay);
        for pid in logged_pids(&run.pids) {
            // SAFETY: signal 0 only checks existence.
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            assert!(!alive, "{delay}: probe {pid} outlived the check");
        }
    }
}
