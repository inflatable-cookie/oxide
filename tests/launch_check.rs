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
/// Scheduling slack above a limit on a loaded host; below the 2 s delays,
/// so an unbounded check still fails.
const SLACK: Duration = Duration::from_millis(1_000);
const QUICK: &str = "#!/bin/sh\nexit 0\n";
/// Records its PID, then never exits on its own.
const SLOW: &str = "#!/bin/sh\necho $$ >> PIDS\nexec sleep 30\n";

struct Run {
    stdout: String,
    wall: Duration,
    tmp: PathBuf,
    pids: PathBuf,
    /// The measuring helper's process group, recorded by the debug hook.
    group: PathBuf,
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
    let group = root.join("group");
    assert!(!group.exists(), "{label}: stale group record");
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
        .env("OXIDE_TEST_LAUNCH_GROUP_LOG", &group)
        .envs(environment.iter().copied())
        .output()
        .expect("oxide status --launch-check");
    let wall = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{label}: {output:?}");
    assert!(output.stderr.is_empty(), "{label}: {output:?}");
    let run = Run {
        stdout,
        wall,
        tmp,
        pids,
        group,
    };
    assert_group_closed(&run, label);
    run
}

/// The helper's recorded process group, this case's own, is gone right
/// after the CLI exits: `kill(-group, 0)` fails with exactly `ESRCH`. A
/// missing or invalid record, or any other result, fails.
fn assert_group_closed(run: &Run, label: &str) {
    let text = std::fs::read_to_string(&run.group)
        .unwrap_or_else(|error| panic!("{label}: no recorded helper group: {error}"));
    let group: i32 = text
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{label}: invalid group record {text:?}"));
    assert!(group > 1, "{label}: invalid group {group}");
    // SAFETY: signal 0 only checks existence.
    let status = unsafe { libc::kill(-group, 0) };
    let error = std::io::Error::last_os_error().raw_os_error();
    assert!(
        status == -1 && error == Some(libc::ESRCH),
        "{label}: helper group {group} not closed: kill returned {status}, errno {error:?}"
    );
}

/// Nothing left in the temporary directory once the CLI has exited.
fn assert_clean(run: &Run, label: &str) {
    let left: Vec<_> = std::fs::read_dir(&run.tmp)
        .expect("tmp")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert!(left.is_empty(), "{label}: left behind {left:?}");
    // Only the settled report: "… ms limit; temporary copies removed". An
    // Unsettled cleanup ("process group … not confirmed closed …; temporary
    // copies removed") fails even if the group is gone by the time of the
    // post-exit ESRCH check.
    assert!(
        run.stdout.contains(" ms limit; temporary copies removed\n")
            && !run.stdout.contains("not confirmed closed"),
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
    // A generous limit. On a loaded host the fresh copies' first launches
    // can still exhaust it; the command's contract then reports exactly that
    // deadline as inconclusive. Those are the only two accepted outcomes;
    // the median and report arithmetic are unit-tested in `launch.rs`.
    let run = check(
        "ordinary",
        QUICK,
        &[("OXIDE_TEST_LAUNCH_LIMIT_MS", "20000")],
    );
    assert!(
        run.wall < Duration::from_secs(20) + SLACK,
        "took {:?}",
        run.wall
    );
    let complete = run.stdout.contains("  medians: same file ")
        && run.stdout.contains(", fresh copy ")
        && (run
            .stdout
            .contains("  result: no material penalty (difference at most 30 ms); nothing to do\n")
            || run
                .stdout
                .contains(" per newly written executable, above the 30 ms threshold\n"))
        && !run.stdout.contains("inconclusive");
    let deadline = run.stdout.contains(
        "  result: inconclusive (the 20000 ms limit was reached); no median reported; run it again",
    ) && !run.stdout.contains("medians:");
    assert!(
        complete != deadline,
        "neither complete medians nor the 20000 ms deadline: {}",
        run.stdout
    );
    assert_clean(&run, "ordinary");
}

#[test]
fn a_slow_probe_group_is_closed_before_the_cli_exits() {
    // `check` already requires the helper's group (probes included, even one
    // still starting) to be gone. Whether the slow script reached its PID
    // marker under host load is separate: every PID it did log must be dead.
    let run = check("slow", SLOW, &[("OXIDE_TEST_LAUNCH_LIMIT_MS", "5000")]);
    assert!(
        run.wall < Duration::from_millis(5_000) + SLACK,
        "took {:?}",
        run.wall
    );
    assert!(
        run.stdout
            .contains("result: inconclusive (the 5000 ms limit was reached); no median reported"),
        "{}",
        run.stdout
    );
    assert!(!run.stdout.contains("medians:"));
    assert_clean(&run, "slow");
    for pid in logged_pids(&run.pids) {
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
        let script = if delay.starts_with("start") {
            SLOW
        } else {
            QUICK
        };
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
            run.stdout
                .contains("result: inconclusive (the 600 ms limit was reached)"),
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
