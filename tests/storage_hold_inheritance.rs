//! The cooperative hold crosses a real `fork`/`exec`: a child that keeps the
//! inherited descriptor keeps the root held after the launcher drops its own
//! copy, an eviction refuses until the child exits, and the root is eligible
//! afterwards. It lives in its own process because `fork` copies every open
//! descriptor, so running it beside other fixtures would make their lock
//! probes observe this child's inherited copies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use barge::testing::TempDir;
use barge::{FakeOs, ManualClock, Mode};
use oxide_storage::storage::{AreaConfig, Storage, POOL_AREA};

const UUID: &str = "8E4A5C1D-0000-4000-8000-00000000BEEF";

fn area(storage: &Storage, mount: &Path) -> barge::Area {
    storage
        .declare(
            &AreaConfig::new("Builds", mount, POOL_AREA)
                .folder("oxide-pool/slots")
                .soft_cap_bytes(Some(10 << 30))
                .reserve_bytes(20 << 30)
                .floor_bytes(10 << 30),
        )
        .expect("area")
}

/// The forked child, held open until the test releases it. The test owns the
/// pipe's write end, so the child lives exactly as long as the test needs and no
/// timing decides the outcome. Dropping it releases the child even when an
/// assertion panics, so a failure cannot leave it blocked.
struct Child {
    pid: Option<libc::pid_t>,
    write: Option<libc::c_int>,
}

impl Child {
    /// Closes the write end so `cat` sees end of input, then reaps the child and
    /// returns its wait status. `None` once the child is already reaped.
    fn release(&mut self) -> Option<libc::c_int> {
        if let Some(fd) = self.write.take() {
            // SAFETY: `fd` is this test's own pipe descriptor, closed once.
            unsafe { libc::close(fd) };
        }
        let pid = self.pid.take()?;
        let mut status = 0;
        // SAFETY: `pid` is this test's own child and `status` is a valid out
        // pointer for one `c_int`.
        let reaped = unsafe { libc::waitpid(pid, &mut status, 0) };
        (reaped == pid).then_some(status)
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[test]
fn a_child_keeps_the_inherited_hold_across_exec_and_eviction_waits_for_it() {
    let temp = TempDir::new("hold-inheritance").expect("temp dir");
    let mount = temp.path().join("Builds");
    std::fs::create_dir_all(&mount).expect("mount");
    let os = FakeOs::with_volume(mount.clone(), UUID, 100 << 30);
    let storage = Storage::open_with(
        temp.path().join("state"),
        Arc::new(os),
        Arc::new(ManualClock::new(1_000)),
    )
    .expect("storage");
    let area = area(&storage, &mount);
    let root: barge::Root = area.create_root("quick").expect("root");
    let hold = area.hold(&root.id, Mode::Exclusive).expect("hold");
    hold.set_inheritable(true).expect("inheritable");

    // Everything the child needs is built before `fork`, so the child allocates
    // nothing between `fork` and `exec`.
    let program = std::ffi::CString::new("/bin/cat").expect("path");
    let argv = [program.as_ptr(), std::ptr::null()];
    let mut ends = [0 as libc::c_int; 2];
    // SAFETY: `ends` has room for the two descriptors `pipe` writes.
    assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0, "pipe");
    let (read, write) = (ends[0], ends[1]);

    // SAFETY: `fork` in this single-threaded test process. The child closes the
    // test's write end, makes the read end its stdin and execs `cat`, which
    // blocks until that write end closes. The hold descriptor is meant to be
    // inherited.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork: {}", std::io::Error::last_os_error());
    if pid == 0 {
        // SAFETY: the child owns these descriptors, and only async-signal-safe
        // calls run before `exec`. `_exit` runs only if `exec` fails.
        unsafe {
            libc::close(write);
            libc::dup2(read, 0);
            libc::execv(program.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    // SAFETY: the parent never reads from the pipe.
    unsafe { libc::close(read) };
    let mut child = Child {
        pid: Some(pid),
        write: Some(write),
    };
    drop(hold);
    assert!(
        area.try_hold(&root.id, Mode::Exclusive)
            .expect("try")
            .is_none(),
        "the child keeps the inherited hold"
    );
    assert!(
        area.evict(&root.id).is_err(),
        "an eviction refuses while the child holds the root"
    );
    let status = child.release().expect("the child was reaped");
    assert_eq!(status, 0, "cat exits cleanly once its input ends");
    assert!(
        area.try_hold(&root.id, Mode::Exclusive)
            .expect("try")
            .is_some(),
        "the root is eligible once the child exits"
    );
    area.evict(&root.id).expect("evicted");
    assert!(!root.path.exists());
    let _ = PathBuf::new();
}
