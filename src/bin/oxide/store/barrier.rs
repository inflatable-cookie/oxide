//! Clean and checkout barriers, checkout custody and the reentrant admission
//! publication guard.
//!
//! A clean barrier excludes every command of one checkout in both pools; a
//! record lock excludes every other command from one record. The custody
//! verdict is the conservative half: unknown writers are reported as unknown,
//! never as clear.

use super::*;
use crate::identity;
use crate::process;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::admission::{Wait, WaitAccounting, WAIT_POLL};
use super::control_io::{
    ensure_private_dir, lock_admission, private_file, probe_lock, publish_writer_generation,
    read_control_state_if_present, Probe,
};
use super::recovery::{boot_session, closed_by_boot};

/// Exclusive clean of one checkout: no command for it holds or takes a slot
/// until [`Store::finish_clean`].
pub struct CleanGuard {
    id: u64,
    lock: File,
}

impl CleanGuard {
    /// Let the next spawned child inherit the clean lock, so the barrier
    /// holds until that child and everything sharing the descriptor ends.
    pub fn share_with_children(&self) -> Result<(), String> {
        // SAFETY: the descriptor is open; F_SETFD with 0 only clears
        // FD_CLOEXEC on it.
        if unsafe { libc::fcntl(self.lock.as_raw_fd(), libc::F_SETFD, 0) } == 0 {
            Ok(())
        } else {
            Err(format!(
                "cannot share the clean lock with Cargo: {}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

pub enum Cleaning {
    Guard(CleanGuard),
    Interrupted(i32),
}

/// What a checkout's cooperative custody allows; see
/// [`Store::checkout_custody`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckoutCustody {
    /// No command may be writing and no unknown writer custody remains.
    Clear,
    /// A command in this checkout may be running Cargo now.
    Writers,
    /// A lost slot's writers were never positively closed. Nothing may be
    /// relinked or cleaned through this checkout until that is resolved.
    Unknown(String),
}

/// Cooperative custody for one checkout across parsed pool states: an active
/// record in either pool counts as a writer, and a retired slot whose lost
/// writers were never positively closed — and whose holder predates no boot
/// boundary — is unknown custody, not activity. Shared by
/// [`Store::checkout_custody`] and the fresh per-row custody witness
/// (oxide#235) so both classify records identically. The pool labels appear
/// verbatim in unknown-custody reasons.
pub(super) fn custody_verdict(
    pools: &[(Option<&State>, &str, Option<u64>)],
    checkout: &Path,
    boot: Option<u64>,
) -> CheckoutCustody {
    let mut writers = false;
    // Record IDs are per pool, so the caller's own ID is exempt only in
    // its own pool's state; an equal ID in the other pool is another command.
    for (state, pool, exempt) in pools {
        let Some(state) = state else { continue };
        for record in &state.records {
            if record.checkout.as_deref() != Some(checkout) || Some(record.id) == *exempt {
                continue;
            }
            if record.phase == Phase::Retired {
                if let Some(lost) = record.lost.as_ref() {
                    // A restart closed a quarantined record whose holder
                    // predates this boot session; its slot is released and
                    // deleted whole at the next admission.
                    let unclosed = lost
                        .unclosed
                        .as_deref()
                        .filter(|_| !closed_by_boot(record, boot));
                    if let Some(why) = unclosed {
                        return CheckoutCustody::Unknown(format!(
                            "slot {} in the {pool} pool is lost with writers that were never confirmed closed ({why})",
                            lost.slot_id
                        ));
                    }
                }
                continue;
            }
            if matches!(record.phase, Phase::Spawning | Phase::Running) {
                writers = true;
            }
        }
    }
    if writers {
        CheckoutCustody::Writers
    } else {
        CheckoutCustody::Clear
    }
}

/// The internal pool's admission lock, made reentrant for this thread so an
/// owned-output publication can update its registry and its receipt inside one
/// critical section without taking the same lock twice (oxide#129). Nested
/// acquisition only succeeds on the thread that already holds it.
pub(crate) struct PublicationGuard {
    control: PathBuf,
    nested: bool,
    _file: Option<File>,
}

thread_local! {
    pub(super) static HELD_PUBLICATION: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn publication_guard(control: &Path) -> Result<PublicationGuard, String> {
    let already = HELD_PUBLICATION.with(|held| held.borrow().iter().any(|path| path == control));
    if already {
        return Ok(PublicationGuard {
            control: control.to_path_buf(),
            nested: true,
            _file: None,
        });
    }
    let file = lock_admission(control)?;
    publish_writer_generation(control)?;
    HELD_PUBLICATION.with(|held| held.borrow_mut().push(control.to_path_buf()));
    Ok(PublicationGuard {
        control: control.to_path_buf(),
        nested: false,
        _file: Some(file),
    })
}

impl Drop for PublicationGuard {
    fn drop(&mut self) {
        if self.nested {
            return;
        }
        HELD_PUBLICATION.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(index) = held.iter().rposition(|path| path == &self.control) {
                held.remove(index);
            }
        });
    }
}

impl Store {
    /// A per-checkout binding mutex: not state-dependent, so two commands can
    /// never wait on each other through it. It serialises only the short
    /// window in which a command validates or replaces its checkout's owned
    /// output link and starts Cargo. The file lives in the internal pool's
    /// control directory, so both pools share it.
    pub fn lock_binding(&self, checkout: &Path) -> Result<File, String> {
        let directory = self.owner_control().join("bindings");
        ensure_private_dir(&directory)?;
        let id = identity::digest(&checkout.to_string_lossy());
        let file = private_file(&directory.join(format!("{id}.lock")), true)?
            .ok_or("the checkout binding lock is missing")?;
        crate::budget::lock(&file, "checkout binding")?;
        Ok(file)
    }

    /// One checkout's cooperative custody for an output transition or a clean.
    /// A command that may be writing counts while it is spawning or running; a
    /// record that has not started Cargo cannot write through the link and
    /// binds again after a transition. A retired slot whose lost writers were
    /// never positively closed is unknown custody, not activity: it blocks
    /// every relink and clean until it is resolved.
    pub fn checkout_custody(&self, checkout: &Path, own: u64) -> Result<CheckoutCustody, String> {
        let _sibling = self.sibling_admission()?;
        let _admission = self.admission()?;
        let own_state = read_control_state_if_present(&self.control)?;
        // A sibling with no state file has no records; any failure to read or
        // parse one is unknown custody, never "clear".
        let other_state = match self.checked_sibling_state() {
            Ok(state) => state,
            Err(reason) => return Ok(CheckoutCustody::Unknown(reason)),
        };
        let boot = boot_session();
        let pools = [
            (own_state.as_ref(), "this", Some(own)),
            (other_state.as_ref(), "the other", None),
        ];
        Ok(custody_verdict(&pools, checkout, boot))
    }

    /// The sibling pool's admission lock, when this store is the removable
    /// one. Always internal first, as slot attempts and cleans do.
    pub(super) fn sibling_admission(&self) -> Result<Option<File>, String> {
        let Some(sibling) = self.sibling.as_ref().filter(|sibling| sibling.barrier) else {
            return Ok(None);
        };
        let file = private_file(&sibling.control.join("admission.lock"), true)?
            .ok_or("the internal admission lock is missing")?;
        crate::budget::lock(&file, "internal admission")?;
        Ok(Some(file))
    }

    /// The internal pool's admission lock, for the removable store's slot
    /// attempts, plus whether a live internal clean covers `checkout`. A
    /// clean mark counts while its lock is held or its Cargo group lives.
    pub(super) fn sibling_barrier(&self, checkout: &Path) -> Result<(Option<File>, bool), String> {
        let Some(sibling) = self.sibling.as_ref().filter(|sibling| sibling.barrier) else {
            return Ok((None, false));
        };
        let file = private_file(&sibling.control.join("admission.lock"), true)?
            .ok_or("the internal admission lock is missing")?;
        crate::budget::lock(&file, "internal admission")?;
        let cleaning = match self.checked_sibling_state()? {
            None => false,
            Some(state) => {
                let mut live = false;
                for clean in state
                    .cleans
                    .iter()
                    .filter(|clean| clean.checkout == checkout)
                {
                    let path = sibling
                        .control
                        .join("waiters")
                        .join(format!("{}.lock", clean.id));
                    live |= match probe_lock(&path, false)? {
                        Probe::Held => true,
                        Probe::Free(_) => clean
                            .process_group
                            .is_some_and(|group| !process::group_empty(group)),
                        Probe::Missing => false,
                    };
                }
                live
            }
        };
        Ok((Some(file), cleaning))
    }

    /// Wait until no live or unsettled command for this checkout holds a
    /// record, then mark the checkout as cleaning. Ctrl-C while waiting
    /// cancels.
    pub fn begin_clean(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
    ) -> Result<Cleaning, String> {
        self.begin_clean_recovering(checkout, waits)
    }

    pub(super) fn begin_clean_recovering(
        &self,
        checkout: &Path,
        waits: &mut WaitAccounting,
    ) -> Result<Cleaning, String> {
        self.ensure_control()?;
        loop {
            if let Some(signal) = process::received_signal() {
                waits.stop(Instant::now());
                return Ok(Cleaning::Interrupted(signal));
            }
            let attempt = (|| {
                // A removable store's barrier covers both pools, so it takes
                // the internal admission lock first (the same order slot
                // attempts and cleans use) and checks the sibling's live state
                // under it.
                let _sibling = self.sibling_admission()?;
                let _admission = self.admission()?;
                let mut state = self.load()?;
                self.drop_dead_tickets(&mut state)?;
                self.recover_settled(&mut state)?;
                let busy = state.cleans.iter().any(|clean| clean.checkout == checkout)
                    || state.records.iter().any(|record| {
                        record.checkout.as_deref() == Some(checkout) && record.active()
                    })
                    || self.sibling_busy(checkout)?;
                if !busy {
                    if state.cleans.len() >= MAX_TICKETS {
                        return Err(format!("too many concurrent oxide cleans ({MAX_TICKETS})"));
                    }
                    let id = state.take_id()?;
                    let Probe::Free(lock) = probe_lock(&self.ticket_path(id), true)? else {
                        return Err("a new Oxide clean lock is unexpectedly held".to_owned());
                    };
                    state.cleans.push(Clean {
                        id,
                        checkout: checkout.to_path_buf(),
                        process_group: None,
                    });
                    self.save(&state)?;
                    return Ok(Some(CleanGuard { id, lock }));
                }
                self.save(&state)?;
                Ok(None)
            })();
            match attempt {
                Err(error) => {
                    waits.stop(Instant::now());
                    return Err(error);
                }
                Ok(Some(guard)) => {
                    waits.stop(Instant::now());
                    waits.print_completion("clean barrier acquired");
                    return Ok(Cleaning::Guard(guard));
                }
                Ok(None) => {
                    waits.observe_with_reason(
                        &Wait::Clean,
                        Instant::now(),
                        "active oxide commands in this checkout",
                    );
                    std::thread::sleep(WAIT_POLL);
                }
            }
        }
    }

    /// Record the `cargo clean` process group for crash checks.
    pub fn clean_running(&self, guard: &CleanGuard, group: i32) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        if let Some(clean) = state.cleans.iter_mut().find(|clean| clean.id == guard.id) {
            clean.process_group = Some(group);
        }
        self.save(&state)
    }

    /// Drop this checkout's slot affinity and end the clean. Slots stay intact.
    pub fn finish_clean(&self, guard: CleanGuard, checkout: &Path) -> Result<usize, String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        let mut detached = 0;
        for record in &mut state.records {
            if record.checkout.as_deref() == Some(checkout)
                && matches!(record.phase, Phase::Idle | Phase::Retired)
            {
                record.checkout = None;
                detached += 1;
            }
        }
        state.cleans.retain(|clean| clean.id != guard.id);
        self.save(&state)?;
        let _ = fs::remove_file(self.ticket_path(guard.id));
        Ok(detached)
    }

    /// End a refused or failed clean without touching affinity.
    pub fn abandon_clean(&self, guard: CleanGuard) -> Result<(), String> {
        let _admission = self.admission()?;
        let mut state = self.load()?;
        state.cleans.retain(|clean| clean.id != guard.id);
        self.save(&state)?;
        let _ = fs::remove_file(self.ticket_path(guard.id));
        Ok(())
    }
}
