//! `stado service label-print` — ask the host init system what it holds under
//! one named unit identity.
//!
//! Enumeration cannot find a loaded unit whose file was deleted, and a unit
//! file cannot say which environment or executable image launchd already
//! loaded. On launchd this reports fixed state fields, the five non-secret
//! storage-routing variables required by recovery, and the running process
//! start, executable path, and digest. It never emits the rest of launchd's
//! environment. A bounded exact-label event tail supplies recent spawn/exit
//! context. Systemd reports the matching fixed properties.
//!
//! It signals nothing, loads nothing and stops nothing. `service bootout` or
//! `service remove` are the commands that act; this one only answers.

use super::service::{validate_unit_id, BootoutScope};
use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

mod parse;
#[cfg(test)]
mod reports_and_read_only;
mod script;
mod state;

use script::LABEL_PRINT_SCRIPT;

pub use parse::parse_label_print;
pub use state::{LabelReadFailure, LabelState};

/// Ask one host what it holds under one label, refusing an inconclusive
/// negative observation so internal lifecycle callers cannot treat a failed
/// domain read as proof that the unit is absent.
pub async fn print_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    let state = inspect_label(target, label, scope, runner).await?;
    if !state.loaded() {
        if let Some(detail) = state.read_failure_detail() {
            return Err(DeployError(format!(
                "{}: could not determine whether {label} is loaded: {detail}",
                state.host
            )));
        }
    }
    Ok(state)
}

/// Ask one host what it holds under one label while retaining unavailable
/// domain evidence for diagnostic callers.
///
/// Signals nothing. This reads only the host's init-system state.
pub async fn inspect_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    validate_unit_id(label)?;
    let predicate = format!(
        "process == \"launchd\" AND eventMessage CONTAINS \"{}\"",
        label.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let script = LABEL_PRINT_SCRIPT
        .replace("@LABEL@", &shlex_quote(label))
        .replace("@PREDICATE@", &shlex_quote(&predicate))
        .replace("@SCOPE@", scope.word());
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the label print did not complete",
        )));
    }
    Ok(parse_label_print(&target.name, label, &output.stdout))
}
