//! An owner that is a process: the host and pid that hold something, and
//! what this process can observe about it.
//!
//! Every lease Stado takes for the length of one invocation (a provider
//! lease for one Box pass, a run-manifest entry for one submission) is held
//! exactly as long as the process holding it runs — and, inside the holding
//! process, as long as the invocation that took it. Nothing measures a time
//! for it: a holder on this host is gone when its pid is, and a holder on
//! another host cannot be observed from here, so it is held and named.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// Owner ids whose invocation is running in this process.
static ACTIVE_OWNERS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// Marks one owner id as running in this process until dropped. Every
/// entry point that mints an owner id holds one for as long as it works, so
/// a lease its invocation left behind (an error path that skipped the
/// release) is seen as gone by the next invocation of the same process.
pub struct OwnerInvocation {
    owner_id: String,
}

impl OwnerInvocation {
    pub fn begin(owner_id: &str) -> Self {
        ACTIVE_OWNERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(owner_id.to_string());
        Self {
            owner_id: owner_id.to_string(),
        }
    }

    /// Whether `owner_id`'s invocation is running in this process now.
    pub fn running(owner_id: &str) -> bool {
        ACTIVE_OWNERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(owner_id)
    }
}

impl Drop for OwnerInvocation {
    fn drop(&mut self) {
        ACTIVE_OWNERS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.owner_id);
    }
}

/// The process that holds something.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOwner {
    pub host: String,
    pub pid: u32,
}

/// What can be said about an owner from this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerState {
    /// The owner is this process.
    This,
    /// The owner is another process on this host that still exists.
    Running,
    /// The owner was a process on this host that no longer exists.
    Gone,
    /// The owner is on another host; this process cannot see it.
    Elsewhere,
}

/// This machine's kernel hostname, the same spelling every Stado command
/// uses for "this host" (a process does not change hosts while it runs).
fn this_host() -> String {
    static HOST: LazyLock<String> = LazyLock::new(crate::providers::vast::system_hostname);
    HOST.clone()
}

/// Whether a pid on this host is a process now: `kill(pid, 0)` succeeds, or
/// is refused only because the process belongs to somebody else.
fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) => true,
        Err(errno) => errno == nix::errno::Errno::EPERM,
    }
}

impl ProcessOwner {
    /// This process.
    pub fn current() -> Self {
        Self {
            host: this_host(),
            pid: std::process::id(),
        }
    }

    /// What this process can observe about the owner now.
    pub fn state(&self) -> OwnerState {
        if self.host != this_host() {
            return OwnerState::Elsewhere;
        }
        if self.pid == std::process::id() {
            return OwnerState::This;
        }
        if pid_alive(self.pid) {
            OwnerState::Running
        } else {
            OwnerState::Gone
        }
    }

    /// "pid N on HOST", for refusals.
    pub fn describe(&self) -> String {
        format!("pid {} on {}", self.pid, self.host)
    }
}
