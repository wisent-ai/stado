//! What the resolver's published state is a blocker of, read as the word
//! of the process that wrote it.

use crate::cli::resolver::report::published::{
    publisher_running, state_path, PublishedState, RESOLVER_SERVING, STATE_FILE,
};

/// The one blocker the published state is: none published, published by a
/// process that has exited, or published in a state other than serving with
/// the resolver's own reason. `None` for a running resolver that serves.
pub(super) fn state_blocker(
    published: Option<&PublishedState>,
    resolver_state: &str,
) -> Option<String> {
    let Some(state) = published else {
        return Some(format!(
            "no resolver has published state at {}: nothing has served here since that file was \
             last removed",
            match state_path() {
                Some(path) => path.display().to_string(),
                None => STATE_FILE.to_string(),
            }
        ));
    };
    if !publisher_running(state) {
        return Some(format!(
            "the resolver that published state {resolver_state} (pid {}) at {} is not running, \
             and no resolver has published since: what the file says was true of a process \
             that has exited",
            state.pid, state.updated_at
        ));
    }
    if resolver_state == RESOLVER_SERVING {
        return None;
    }
    let mut sentence = format!("the resolver reports state {resolver_state}");
    if let Some(reason) = state.reason.as_deref() {
        sentence.push_str(": ");
        sentence.push_str(reason);
    }
    if let Some(next) = state.next_attempt_at.as_deref() {
        sentence.push_str(&format!(
            " (failed attempt {}, next read due {next})",
            state.attempt
        ));
    }
    Some(sentence)
}
