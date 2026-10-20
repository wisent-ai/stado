//! This machine's own Stado unit, read when a command failed because a
//! service did not answer.
//!
//! The object API, the resolver and the local API all run in the one host
//! Stado process. When its supervisor starts it and it exits at once, as a
//! release whose argument parser refuses the unit's own argv does, the
//! supervisor restarts it without end and tells nothing to the command that
//! met the closed port, and the process that would record its own failure is
//! the one that cannot start. So a failure stated as an outage is followed by
//! the unit's state when its supervisor holds it loaded with no live process
//! and a nonzero last exit: the supervisor's exit code and run count
//! ([`LabelState::failing`]) and the last line of the unit's log, read by the
//! same reader and window `stado service status` uses.

use crate::deploy::service_label_print::{inspect_label_state, LabelState};

/// The sentence to print beside an outage; `None` when this machine's unit
/// runs, last exited cleanly, or is not loaded here. A read that could not
/// decide is said, never taken for a running unit.
pub(super) async fn failing() -> Option<String> {
    match read().await {
        Ok(failing) => failing,
        Err(cause) => Some(format!(
            "whether this machine's Stado unit is failing could not be read: {cause}"
        )),
    }
}

async fn read() -> Result<Option<String>, String> {
    let label = crate::deploy::service_catalog::host_unit()?;
    let target = crate::deploy::service::local_target().map_err(|error| error.to_string())?;
    let scope =
        crate::deploy::service::BootoutScope::parse(None).map_err(|error| error.to_string())?;
    let runner = crate::deploy::production_runner();
    let state: LabelState = inspect_label_state(&target, &label, scope, &runner)
        .await
        .map_err(|error| error.to_string())?;
    let Some(failing) = state.failing() else {
        return Ok(None);
    };
    // One line of each stream: the window `stado service status` reads.
    let window = std::num::NonZeroUsize::MIN.get();
    let log =
        match crate::deploy::service::tail_unit_logs(&target, &label, "", Some(window), &runner)
            .await
        {
            Ok(log) => match last_line(&log.error_body).or_else(|| last_line(&log.body)) {
                Some(line) => format!("its last log line: {line}"),
                None => format!("its log ({}) holds no line", log.origin),
            },
            Err(error) => format!("its log could not be read: {error}"),
        };
    Ok(Some(format!(
        "this machine's {failing}; {log} (`stado service unit logs {label} --host {}` reads more)",
        target.name
    )))
}

fn last_line(body: &str) -> Option<String> {
    body.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::to_owned)
}
