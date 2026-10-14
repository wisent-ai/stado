//! Waiting for the resolver process `pid` to publish `serving`, for a command
//! that just restarted the unit carrying this host's own registry route and
//! has one more registry write to make. Nothing is polled on an interval: the
//! kernel's change notification on the state file's directory wakes the
//! reader each time the resolver publishes, and the resolver's own verdict
//! ends the wait — `serving` with its listeners bound, or `failed` with its
//! reason. A resolver that exits without a verdict is restarted by its unit,
//! and the process that replaces it publishes; that publication ends the
//! wait for the one that exited. The wait stands in this host's register of
//! open waits while it lasts, so a resolver that publishes nothing (one
//! standing on its own registry read) is seen waited on, not a silent hang.

use super::published::{published_state, publisher_running, state_path, RESOLVER_SERVING};

const RESOLVER_FAILED: &str = "failed";

/// Block until the resolver process `pid` publishes that it serves, or
/// return its own reason when it publishes that it failed, or say that it
/// exited and which process published since.
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
    crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("the resolver (pid {pid}) to publish that it serves"),
        path.display(),
        || loop {
            // Armed before the read, so a publication that lands between
            // the read and the wait still wakes it.
            let mut watch =
                crate::queue::local_file::watch_directories(std::slice::from_ref(&directory))
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
            if let Some(state) = published_state() {
                if state.pid == pid && state.state == RESOLVER_SERVING && state.listening {
                    return Ok(());
                }
                if state.pid == pid && state.state == RESOLVER_FAILED {
                    return Err(std::io::Error::other(format!(
                        "the resolver (pid {pid}) failed: {}",
                        match state.reason.as_deref() {
                            Some(reason) => reason,
                            None => "it published no reason",
                        }
                    )));
                }
                if state.pid != pid && publisher_running(&state) {
                    return Err(std::io::Error::other(format!(
                        "the resolver (pid {pid}) exited without publishing that it serves; \
                         pid {} published {} since, at {}",
                        state.pid, state.state, state.updated_at
                    )));
                }
            }
            watch
                .next()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        },
    )
    .map_err(|error| error.to_string())
}
