//! The runner role on a host's one Stado unit.
//!
//! A runner is not a unit of its own: the host's `com.wisent.stado` runs its
//! launcher as the `--precheck-runner <ROOT>` role of `stado serve`. Installing
//! a runner therefore ends by asserting that unit with the role switched on
//! for the runner's root, through the same `service ensure` pass an operator
//! runs by hand, so the declaration, the audit record and the restart are
//! the ones every other role gets. Removing a runner takes the role off the
//! same way.

use crate::cli::service::{declared_matching, ensure_unit, EnsureOptions};
use crate::deploy::DeployError;

/// The `stado serve` option that switches the role on, as the live-process
/// role read names it.
pub const RUNNER_ROLE: &str = "--precheck-runner";

/// The launchd system domain's unit directory: a declaration there is a
/// daemon and is re-asserted as one.
const DAEMON_DIRECTORY: &str = "/Library/LaunchDaemons/";

/// Assert the host's Stado unit with the runner role for `runner_root`
/// switched on; `off` takes it off instead. Returns the ensure action the
/// pass reported.
pub async fn declare_runner_role(
    host: &str,
    runner_root: &str,
    off: bool,
    reason: &str,
) -> Result<String, DeployError> {
    let unit = crate::deploy::local_install::stado_unit()?;
    let declared = declared_matching(&unit, Some(host))
        .await
        .map_err(|error| DeployError(format!("{host}: {error}")))?;
    let existing = declared
        .into_iter()
        .next()
        .ok_or_else(|| DeployError(format!("{host}: no declaration of {unit} on this host")))?;
    if existing.program.is_empty() {
        return Err(DeployError(format!(
            "{host}: {unit} is declared without its program, so no role can be added to it; \
             declare it with `stado service ensure stado --host {host}` first"
        )));
    }
    let option = format!("{RUNNER_ROLE}={runner_root}");
    let declared_on = existing.args.contains(&option);
    if off && !declared_on {
        return Ok("absent".to_string());
    }
    let mut args: Vec<String> = existing
        .args
        .iter()
        .filter(|argument| **argument != option)
        .cloned()
        .collect();
    if !off {
        args.push(option);
    }
    let env: Vec<String> = existing
        .env
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let receipt = ensure_unit(EnsureOptions {
        name: &unit,
        host,
        from: Some(&existing.program),
        args: &args,
        env: &env,
        unset_env: &[],
        reason,
        as_daemon: existing.path.starts_with(DAEMON_DIRECTORY),
        as_launch_agent: false,
        as_json: true,
    })
    .await
    .map_err(|error| DeployError(format!("{host}: {unit}: {error}")))?;
    Ok(receipt.action)
}
