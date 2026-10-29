//! Where the resolver's published state lives, and how it is written and
//! read back.

use super::{lifetime, PublishedState, STATE_FILE, STATE_FILE_ENV};

/// The published state's file: the one the environment names, else
/// `~/.stado/<state file>` under the home every Stado file is kept under
/// ([`crate::config_file::home`]: HOME, else the user database), so a
/// resolver unit started without HOME publishes where `stado resolver
/// status` reads. A home neither names is said.
pub(in crate::cli::resolver) fn state_path() -> Option<std::path::PathBuf> {
    if let Some(explicit) = std::env::var_os(STATE_FILE_ENV) {
        let path = std::path::PathBuf::from(explicit);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    match crate::config_file::home() {
        Ok(home) => Some(home.join(".stado").join(STATE_FILE)),
        Err(cause) => {
            eprintln!("stado: the resolver's published state has no file: {cause}");
            None
        }
    }
}

/// Publish the state, atomically, best effort.
///
/// Best effort on purpose: a resolver that cannot write its own diagnostic
/// file is still one that can serve traffic, and refusing to start over it
/// would make this file the outage. Written to a sibling and renamed rather
/// than in place, because `status` reads it while `serve` writes it and a
/// half-written document reads as a resolver that has never run.
pub(crate) fn publish(state: &PublishedState) {
    let Some(path) = state_path() else { return };
    let Some(parent) = path.parent() else { return };
    // The lifetime lock is held from the first publication on, so a reader
    // can tell a running publisher from a pid that outlived it.
    if let Err(error) = std::fs::create_dir_all(parent) {
        eprintln!(
            "stado resolver could not create {}: {error}",
            parent.display()
        );
    }
    lifetime::hold(&path);
    let body = match serde_json::to_vec_pretty(state) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("stado resolver could not serialize its state: {error}");
            return;
        }
    };
    if let Err(error) = crate::primitives::document::persist(&path, &body) {
        eprintln!(
            "stado resolver could not publish its state to {}: {error}",
            path.display()
        );
    }
}

/// The state the last `serve` process published, when there is one. A state
/// file that exists but cannot be read or parsed is named on stderr with its
/// cause: answered as none, status reported that no resolver had published
/// while one had, and every adapter lookup through it looked unserved.
pub(in crate::cli::resolver) fn published_state() -> Option<PublishedState> {
    let path = state_path()?;
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!(
                "stado resolver state {} cannot be read: {error}",
                path.display()
            );
            return None;
        }
    };
    match serde_json::from_str(&body) {
        Ok(state) => Some(state),
        Err(error) => {
            eprintln!(
                "stado resolver state {} is not a published state this build reads: {error}",
                path.display()
            );
            None
        }
    }
}

/// Whether the process that published `state` is still running: a state
/// file outlives the resolver that wrote it, and a reader that trusts a
/// dead resolver's `listening: true` sends traffic to ports nobody holds.
/// Read from the lifetime lock the publisher holds ([`lifetime`]), never
/// from its pid alone, which the system hands on once it exits. A lock that
/// cannot be read is said on stderr and answers that no publisher runs.
pub(crate) fn publisher_running(state: &PublishedState) -> bool {
    publisher_stale(state).is_none()
}

/// Why the state's publisher is not known to run, naming the pid it checked
/// and the lock it checked it by; `None` while that pid holds the lock.
pub(crate) fn publisher_stale(state: &PublishedState) -> Option<String> {
    let Some(path) = state_path() else {
        return Some("HOME is unset, so the resolver's lifetime lock cannot be found".to_string());
    };
    match lifetime::held_by(&path, state.pid) {
        Ok(stale) => stale,
        Err(cause) => {
            eprintln!("stado resolver state: whether its publisher runs cannot be read: {cause}");
            Some(format!(
                "whether pid {} still runs cannot be read: {cause}",
                state.pid
            ))
        }
    }
}
