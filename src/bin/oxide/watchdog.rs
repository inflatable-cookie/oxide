//! Best-effort host-reserve watchdog for builds on a budget-only pool.
//!
//! A directory pool has no OS ceiling. While a supervised build runs, the
//! watchdog samples the caller-available bytes of the pool filesystem with
//! `statfs` about once a second. Below the trigger (20 GiB protected reserve
//! plus a 2 GiB margin) it sends TERM to Cargo's process group, then KILL
//! after a bounded grace. It never walks the slot. Sampling cannot stop a
//! faster writer, or another process, from going past the trigger or even
//! the reserve; the report states what was observed.
//!
//! On a removable pool the same stop runs when the volume disappears: about
//! once a second it checks that the mount root still opens as the uuid the
//! build started on. A build script or other program whose executable lived
//! on a forcibly removed volume can spin instead of exiting, so without the
//! stop the wait for Cargo's group would never end (oxide#098).

use crate::store::{GIB, HOST_RESERVE_BYTES};
use barge::Os;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Safety margin above the protected reserve.
pub const MARGIN_BYTES: u64 = 2 * GIB;
pub(crate) const INTERVAL: Duration = Duration::from_secs(1);
/// TERM to KILL.
const GRACE: Duration = Duration::from_secs(2);
/// KILL to giving up on positive closure: the slot then stays held.
const CLOSE_BOUND: Duration = Duration::from_secs(10);

enum Source {
    Statfs(PathBuf),
    /// A removable pool's mount root and the uuid bound when the build
    /// started (`None`: it did not bind, so the first poll stops).
    Volume {
        mount: PathBuf,
        uuid: Option<String>,
    },
    /// Debug builds only: `OXIDE_TEST_WATCHDOG_READINGS` names a file holding
    /// the available-bytes reading, so the fixture proof never consumes host
    /// space to reach a trigger.
    #[cfg(debug_assertions)]
    Injected(PathBuf),
}

struct Stop {
    at: Instant,
    /// The sample below the trigger; `None` when sampling failed.
    reading: Option<u64>,
    killed: Option<Instant>,
}

pub struct Watchdog {
    source: Source,
    trigger: u64,
    grace: Duration,
    close_bound: Duration,
    next: Instant,
    minimum: Option<u64>,
    stop: Option<Stop>,
}

impl Watchdog {
    pub fn for_pool(payload: &Path) -> Self {
        #[cfg(debug_assertions)]
        if let Some(readings) = std::env::var_os("OXIDE_TEST_WATCHDOG_READINGS") {
            return Self::new(
                Source::Injected(PathBuf::from(readings)),
                GRACE,
                CLOSE_BOUND,
            );
        }
        Self::new(Source::Statfs(payload.to_path_buf()), GRACE, CLOSE_BOUND)
    }

    /// The removal stop for a removable pool mounted at `mount`.
    pub fn for_volume(mount: &Path) -> Self {
        let uuid = oxide_storage::storage::OxideOs
            .volume_identity(mount)
            .ok()
            .flatten();
        let mut watchdog = Self::new(
            Source::Volume {
                mount: mount.to_path_buf(),
                uuid,
            },
            GRACE,
            CLOSE_BOUND,
        );
        // Only a failed sample stops: there is no space trigger.
        watchdog.trigger = 0;
        watchdog
    }

    /// Whether this watchdog stops on removal of a removable volume rather
    /// than on host free space.
    pub fn removal(&self) -> bool {
        matches!(self.source, Source::Volume { .. })
    }

    fn new(source: Source, grace: Duration, close_bound: Duration) -> Self {
        Self {
            source,
            trigger: HOST_RESERVE_BYTES + MARGIN_BYTES,
            grace,
            close_bound,
            next: Instant::now(),
            minimum: None,
            stop: None,
        }
    }

    /// Call on every wait poll for the live group. Samples once per interval
    /// until a stop, then on every poll so the minimum is recorded.
    pub fn poll(&mut self, group: i32) {
        let now = Instant::now();
        if self.stop.is_none() && now < self.next {
            return;
        }
        self.next = now + INTERVAL;
        let reading = self.sample();
        if let Some(bytes) = reading {
            self.minimum = Some(self.minimum.map_or(bytes, |minimum| minimum.min(bytes)));
        }
        match &mut self.stop {
            None => {
                if reading.is_none_or(|bytes| bytes < self.trigger) {
                    signal_group(group, libc::SIGTERM);
                    self.stop = Some(Stop {
                        at: now,
                        reading,
                        killed: None,
                    });
                }
            }
            Some(stop) => {
                if stop.killed.is_none() && now.duration_since(stop.at) >= self.grace {
                    signal_group(group, libc::SIGKILL);
                    stop.killed = Some(now);
                }
            }
        }
    }

    pub fn stopped(&self) -> bool {
        self.stop.is_some()
    }

    /// KILL was sent and the closure bound has passed.
    pub fn gave_up(&self) -> bool {
        self.stop
            .as_ref()
            .and_then(|stop| stop.killed)
            .is_some_and(|killed| killed.elapsed() >= self.close_bound)
    }

    /// One line for a stop: the trigger, the signals sent, the sampled
    /// minimum, overshoot and latency to `closed` (`None`: not confirmed).
    pub fn report(&self, group: i32, closed: Option<Instant>) -> String {
        let Some(stop) = &self.stop else {
            return String::new();
        };
        let signals = match stop.killed {
            Some(killed) => format!(
                "TERM, then KILL after {} ms",
                killed.duration_since(stop.at).as_millis()
            ),
            None => "TERM".to_owned(),
        };
        let latency = closed.map_or_else(
            || "closure not confirmed".to_owned(),
            |closed| format!("{} ms", closed.duration_since(stop.at).as_millis()),
        );
        if let Source::Volume { mount, .. } = &self.source {
            return format!(
                "oxide: removable volume {} is gone; stopped Cargo group {group}: {signals}, stop latency {latency}",
                mount.display()
            );
        }
        let cause = match stop.reading {
            Some(bytes) => format!(
                "pool filesystem available {bytes} bytes fell below the {} byte trigger",
                self.trigger
            ),
            None => format!(
                "pool filesystem space could not be sampled against the {} byte trigger",
                self.trigger
            ),
        };
        let minimum = self
            .minimum
            .map_or_else(|| "unknown".to_owned(), |bytes| bytes.to_string());
        let overshoot = self.minimum.map_or_else(
            || "unknown".to_owned(),
            |bytes| self.trigger.saturating_sub(bytes).to_string(),
        );
        let source = match self.source {
            Source::Statfs(_) | Source::Volume { .. } => "",
            #[cfg(debug_assertions)]
            Source::Injected(_) => " (injected test readings)",
        };
        format!(
            "oxide: reserve stop{source}: {cause} ({} GiB reserve + {} GiB margin); Cargo group {group}: {signals}; sampled minimum {minimum} bytes, overshoot past trigger {overshoot} bytes, stop latency {latency}; budget-only pool: best-effort stop, no guarantee against faster writers or other processes",
            HOST_RESERVE_BYTES / GIB,
            MARGIN_BYTES / GIB
        )
    }

    /// The same reading used by the running watchdog, including its scoped
    /// debug fixture seam, for a seed's pre-child reserve gate.
    pub(crate) fn seed_sample(&self) -> Option<u64> {
        self.sample()
    }

    fn sample(&self) -> Option<u64> {
        match &self.source {
            Source::Statfs(path) => available_bytes(path),
            Source::Volume { mount, uuid } => oxide_storage::storage::OxideOs
                .volume_identity(mount)
                .ok()
                .flatten()
                .filter(|observed| Some(observed.as_str()) == uuid.as_deref())
                .map(|_| u64::MAX),
            #[cfg(debug_assertions)]
            Source::Injected(path) => std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse().ok()),
        }
    }
}

fn signal_group(group: i32, signal: libc::c_int) {
    if group > 1 {
        // SAFETY: a negative PID addresses the process group this command
        // created; TERM and KILL go only to that group.
        unsafe {
            libc::kill(-group, signal);
        }
    }
}

/// Caller-available bytes (`f_bavail`) of the filesystem holding `path`.
pub fn available_bytes(path: &Path) -> Option<u64> {
    let sample = crate::store::statfs(path)?;
    #[cfg(target_os = "macos")]
    let block = u64::from(sample.f_bsize);
    #[cfg(not(target_os = "macos"))]
    let block = u64::try_from(sample.f_frsize)
        .ok()
        .filter(|block| *block > 0)?;
    sample.f_bavail.checked_mul(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process;
    use std::process::Command;

    fn scratch(label: &str) -> PathBuf {
        let base = std::env::var_os("OXIDE_TASK_SCRATCH_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!("watchdog-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("watchdog scratch");
        path
    }

    /// Owned, so a failed assertion below cannot leave `sleep` in the group.
    fn group(script: &str) -> process::OwnedGroup {
        process::OwnedGroup::spawn(Command::new("/bin/sh").args(["-c", script]))
    }

    fn settle(watchdog: &mut Watchdog, group: &mut process::Group) -> Option<Instant> {
        loop {
            watchdog.poll(group.id);
            let _ = group.child.try_wait();
            if process::group_empty(group.id) {
                return Some(Instant::now());
            }
            if watchdog.gave_up() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn statfs_samples_the_scratch_filesystem() {
        let path = scratch("statfs");
        assert!(available_bytes(&path).is_some_and(|bytes| bytes > 0));
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn an_absent_removable_volume_stops_at_the_first_poll() {
        let mount = PathBuf::from(format!("/Volumes/oxide-absent-{}", std::process::id()));
        let mut watchdog = Watchdog::for_volume(&mount);
        assert!(watchdog.removal() && !watchdog.stopped());
        // Group 0 is never signalled; the stop is recorded.
        watchdog.poll(0);
        assert!(watchdog.stopped());
        let report = watchdog.report(0, None);
        assert!(
            report.contains("is gone") && report.contains("closure not confirmed"),
            "{report}"
        );
        assert!(!Watchdog::for_pool(Path::new("/")).removal());
    }

    #[test]
    fn trigger_is_reserve_plus_margin() {
        let watchdog = Watchdog::for_pool(Path::new("/"));
        assert_eq!(watchdog.trigger, 22 * GIB);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn readings_above_the_trigger_never_stop() {
        let path = scratch("above");
        let readings = path.join("readings");
        std::fs::write(&readings, (23 * GIB).to_string()).expect("reading");
        let mut watchdog = Watchdog::new(
            Source::Injected(readings),
            Duration::from_millis(100),
            Duration::from_millis(500),
        );
        watchdog.poll(-1);
        assert!(!watchdog.stopped());
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn a_failed_sample_stops_the_build() {
        let path = scratch("failed");
        let mut child = group("sleep 30");
        let mut watchdog = Watchdog::new(
            Source::Injected(path.join("missing")),
            Duration::from_millis(100),
            Duration::from_secs(5),
        );
        let closed = settle(&mut watchdog, &mut child);
        assert!(closed.is_some(), "TERM closes a plain sleep");
        assert!(watchdog
            .report(child.id, closed)
            .contains("could not be sampled"));
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    #[cfg(debug_assertions)]
    fn term_then_kill_closes_a_group_that_ignores_term() {
        let path = scratch("kill");
        let readings = path.join("readings");
        std::fs::write(&readings, (22 * GIB - 4096).to_string()).expect("reading");
        let mut child = group("trap '' TERM; sleep 30");
        let mut watchdog = Watchdog::new(
            Source::Injected(readings),
            Duration::from_millis(200),
            Duration::from_secs(5),
        );
        std::thread::sleep(Duration::from_millis(100));
        let closed = settle(&mut watchdog, &mut child);
        assert!(closed.is_some(), "KILL closes the group");
        let report = watchdog.report(child.id, closed);
        assert!(report.contains("TERM, then KILL after"), "{report}");
        assert!(
            report.contains("overshoot past trigger 4096 bytes"),
            "{report}"
        );
        assert!(report.contains("(injected test readings)"), "{report}");
        std::fs::remove_dir_all(path).expect("cleanup");
    }
}
