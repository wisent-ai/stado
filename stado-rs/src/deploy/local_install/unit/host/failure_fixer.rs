//! Import the generated legacy loop without executing or retaining its shell.

use std::num::NonZeroU64;
use std::path::Path;

use crate::cli::integrations::runtime::ServeArgs;
use crate::deploy::DeployError;

use super::InstallPlan;

pub(super) fn merge(runtime: &mut ServeArgs, component: &InstallPlan) -> Result<(), DeployError> {
    let invalid = || DeployError(format!(
        "{}: failure-fixer declaration is not the generated scan-dispatch loop; cannot discard custom shell behavior",
        component.label
    ));
    let [shell, flag, script] = component.exec_args.as_slice() else {
        return Err(invalid());
    };
    if shell != "/bin/bash" || flag != "-c" {
        return Err(invalid());
    }
    let body = script.strip_prefix("while true; do ")
        .and_then(|body| body.strip_suffix("; done")).ok_or_else(invalid)?;
    let (command, interval) = body.rsplit_once("; sleep ").ok_or_else(invalid)?;
    let interval = interval.parse::<NonZeroU64>().map_err(|_| invalid())?;
    let (executable, options) = command.split_once(" scan-dispatch --execute ")
        .ok_or_else(invalid)?;
    // The old generator emitted the executable unquoted. Refuse expansion,
    // redirection and extra commands instead of interpreting them as a path.
    if !Path::new(executable).is_absolute()
        || Path::new(executable).file_name().and_then(|name| name.to_str()) != Some("stado-fix")
        || !executable.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        return Err(invalid());
    }
    let pattern = if options.is_empty() {
        None
    } else {
        let value = options.strip_prefix("--command-pattern '")
            .and_then(|value| value.strip_suffix('\'')).ok_or_else(invalid)?;
        if value.contains('\'') {
            return Err(invalid());
        }
        Some(value.to_string())
    };
    if runtime.failure_fixer_interval_seconds.is_some_and(|previous| previous != interval)
        || (runtime.failure_fixer_interval_seconds.is_some()
            && runtime.failure_fixer_command_pattern != pattern)
    {
        return Err(DeployError(format!(
            "{}: existing failure fixers disagree on their cadence or command filter",
            component.label
        )));
    }
    runtime.failure_fixer_interval_seconds = Some(interval);
    runtime.failure_fixer_command_pattern = pattern;
    Ok(())
}
