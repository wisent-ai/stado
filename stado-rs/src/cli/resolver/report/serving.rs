//! Waiting for the resolver process `pid` to publish `serving`, for a command
//! that just restarted the unit carrying this host's own registry route and
//! has one more registry write to make. Nothing is polled on an interval: the
//! kernel's change notification on the state file's directory wakes the
//! reader each time the resolver publishes, and the resolver's own verdict
//! ends the wait — `serving` with its listeners bound, or `failed` with its
//! reason.

use super::published::{published_state, state_path, RESOLVER_SERVING};

const RESOLVER_FAILED: &str = "failed";

/// Block until the resolver process `pid` publishes that it serves, or
/// return its own reason when it publishes that it failed.
pub(crate) fn await_serving(pid: u32) -> Result<(), String> {
    let path = state_path().ok_or_else(|| {
        "the resolver's state file has no location: HOME is unset and STADO_RESOLVER_STATE_FILE is empty"
            .to_string()
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?
        .to_path_buf();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    loop {
        // Armed before the read, so a publication that lands between the
        // read and the wait still wakes it.
        let mut watch =
            crate::queue::local_file::watch_directories(std::slice::from_ref(&directory))
                .map_err(|error| error.to_string())?;
        if let Some(state) = published_state().filter(|state| state.pid == pid) {
            if state.state == RESOLVER_SERVING && state.listening {
                return Ok(());
            }
            if state.state == RESOLVER_FAILED {
                return Err(format!(
                    "the resolver (pid {pid}) failed: {}",
                    state.reason.as_deref().unwrap_or("no reason published")
                ));
            }
        }
        watch.next().map_err(|error| error.to_string())?;
    }
}
