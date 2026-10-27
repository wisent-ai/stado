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
//! ([`LabelState::failing`]) and the last line of each of the unit's log
//! streams, read by the reader `stado service status` shows by default.

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
    let target = crate::deploy::service::local_target()
        .map_err(|error| format!("this host's target cannot be read: {error}"))?;
    let scope = crate::deploy::service::BootoutScope::parse(None)
        .map_err(|error| format!("the service scope of this host cannot be read: {error}"))?;
    let runner = crate::deploy::production_runner();
    let state: LabelState = inspect_label_state(&target, &label, scope, &runner)
        .await
        .map_err(|error| format!("the host unit {label} on {} cannot be inspected: {error}", target.name))?;
    let Some(failing) = state.failing() else {
        return Ok(None);
    };
    // The reader `stado service status` shows by default: the last line of
    // each stream. Both are said, each by the file it came from, since the
    // two files carry no common clock to tell which was written last.
    let log = match crate::deploy::service::last_unit_lines(&target, &label, "", &runner).await {
        Ok(log) => {
            let mut said = Vec::new();
            if !log.body.is_empty() {
                said.push(format!("{} last said: {}", log.origin, log.body));
            }
            if let (Some(origin), false) = (&log.error_origin, log.error_body.is_empty()) {
                said.push(format!("{origin} last said: {}", log.error_body));
            }
            match said.is_empty() {
                true => format!("its log ({}) holds no line", log.origin),
                false => said.join("; "),
            }
        }
        Err(error) => format!("its log could not be read: {error}"),
    };
    Ok(Some(format!(
        "this machine's {failing}; {log} (`stado service unit logs {label} --host {}` reads more)",
        target.name
    )))
}
