//! Bounded retries for transient descriptor inheritance during process spawn.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

// Match Barge's spawn-window retry bound; genuine holders remain unavailable.
const ATTEMPTS: usize = 100;
const RETRY_DELAY: Duration = Duration::from_millis(2);

/// Try an exclusive flock, allowing a bounded spawn window to close.
#[doc(hidden)]
pub fn exclusive(file: &File) -> io::Result<bool> {
    retry(file, || std::thread::sleep(RETRY_DELAY))
}

fn retry(file: &File, mut wait: impl FnMut()) -> io::Result<bool> {
    for attempt in 0..ATTEMPTS {
        // SAFETY: the file keeps the descriptor open throughout the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        if attempt + 1 < ATTEMPTS {
            wait();
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    fn pipe() -> (File, File) {
        let mut fds = [0; 2];
        // SAFETY: fds points to two writable descriptor slots.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: pipe returned two newly owned descriptors.
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    #[test]
    fn retries_until_spawned_child_exec_closes_inherited_lock() {
        let path = std::env::temp_dir().join(format!("oxide-spawn-lock-{}", std::process::id()));
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(exclusive(&holder).unwrap());
        let (mut ready_read, ready_write) = pipe();
        let (release_read, mut release_write) = pipe();
        let ready_fd = ready_write.as_raw_fd();
        let release_fd = release_read.as_raw_fd();
        let spawn = std::thread::spawn(move || {
            let mut command = Command::new("/usr/bin/true");
            // SAFETY: the child only invokes async-signal-safe syscalls before exec.
            unsafe {
                command.pre_exec(move || {
                    let byte = [1_u8];
                    if libc::write(ready_fd, byte.as_ptr().cast(), 1) != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    let mut byte = [0_u8];
                    if libc::read(release_fd, byte.as_mut_ptr().cast(), 1) != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command.spawn().unwrap().wait().unwrap()
        });
        let mut byte = [0];
        ready_read.read_exact(&mut byte).unwrap();
        drop(holder);
        let mut spawn = Some(spawn);
        let mut waits = 0;
        assert!(retry(&contender, || {
            waits += 1;
            release_write.write_all(&[1]).unwrap();
            assert!(spawn.take().unwrap().join().unwrap().success());
        })
        .unwrap());
        assert_eq!(
            waits, 1,
            "the inherited descriptor must hold the first attempt"
        );
        drop(contender);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn genuine_contention_stays_bounded() {
        let path = std::env::temp_dir().join(format!("oxide-busy-lock-{}", std::process::id()));
        let holder = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(exclusive(&holder).unwrap());
        let mut waits = 0;
        assert!(!retry(&contender, || waits += 1).unwrap());
        assert_eq!(waits, ATTEMPTS - 1);
        drop((holder, contender));
        std::fs::remove_file(path).unwrap();
    }
}
