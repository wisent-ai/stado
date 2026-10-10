//! The priority a scan runs at: the lowest the host offers, for the thread
//! that walks in this process and for the script a host read runs.
//!
//! On macOS the calling thread is put in the background band
//! (`PRIO_DARWIN_THREAD` with `PRIO_DARWIN_BG`), which throttles its disk
//! I/O as well as its CPU share, the same policy `taskpolicy -b` gives a
//! whole process. On Linux the thread takes the lowest nice value the system
//! declares (`PRIO_MAX`), from which the default I/O scheduler derives the
//! thread's I/O priority. A script run on a host is prefixed the same way:
//! `taskpolicy -b` on Darwin, `ionice -c idle nice` elsewhere.

use nix::errno::Errno;
use nix::libc;

/// The calling thread, in `setpriority`'s own words: `who` names the caller
/// when it is left unset.
fn calling_thread() -> libc::id_t {
    Default::default()
}

/// Read a thread's priority: `getpriority` answers `-1` both as a value and
/// as a failure, so the errno it leaves is the verdict.
///
/// # Safety
/// `which` and `who` name the calling thread, as the callers here pass them.
unsafe fn read_priority(which: libc::c_int, who: libc::id_t) -> Option<libc::c_int> {
    Errno::clear();
    let read = libc::getpriority(which, who);
    (Errno::last() == Errno::UnknownErrno).then_some(read)
}

/// The thread's priority before a scan, put back when the scan ends; the
/// threads a runtime pools would otherwise carry the background band into
/// work that is not a scan.
pub struct Throttled {
    previous: Option<libc::c_int>,
}

impl Throttled {
    /// Put the calling thread in the background band for as long as the
    /// guard lives. A refusal by the system is not an error a scan should
    /// stop on: the scan runs at its previous priority and `applied` says so.
    pub fn for_this_thread() -> Self {
        #[cfg(target_os = "macos")]
        {
            let which = libc::PRIO_DARWIN_THREAD;
            let who = calling_thread();
            // SAFETY: a documented query and set of the calling thread's own
            // scheduling band; no memory is handed to the system.
            let previous = unsafe {
                read_priority(which, who).filter(|_| {
                    Errno::result(libc::setpriority(which, who, libc::PRIO_DARWIN_BG)).is_ok()
                })
            };
            return Self { previous };
        }
        #[cfg(target_os = "linux")]
        {
            let which = libc::PRIO_PROCESS;
            // SAFETY: the calling thread's own id and its own nice value.
            let previous = unsafe {
                let who = libc::gettid() as libc::id_t;
                read_priority(which, who).filter(|_| {
                    Errno::result(libc::setpriority(which, who, libc::PRIO_MAX)).is_ok()
                })
            };
            return Self { previous };
        }
        #[allow(unreachable_code)]
        Self { previous: None }
    }

    /// Whether the background band was applied.
    pub fn applied(&self) -> bool {
        self.previous.is_some()
    }
}

impl Drop for Throttled {
    fn drop(&mut self) {
        let Some(previous) = self.previous else {
            return;
        };
        #[cfg(target_os = "macos")]
        // SAFETY: restoring the value read from the same thread above.
        unsafe {
            libc::setpriority(libc::PRIO_DARWIN_THREAD, calling_thread(), previous);
        }
        #[cfg(target_os = "linux")]
        // SAFETY: restoring the value read from the same thread above.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, previous);
        }
    }
}

/// The words that put a host script in the background band, by the host's
/// kernel: `taskpolicy -b` on Darwin, the idle I/O class and the default
/// nice increment elsewhere. Prepended to the program a scan runs.
pub fn background_prefix(darwin: bool) -> &'static str {
    if darwin {
        "/usr/bin/taskpolicy -b"
    } else {
        "ionice -c idle nice"
    }
}
