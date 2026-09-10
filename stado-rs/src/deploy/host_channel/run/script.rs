//! Scripts fed to the far side on stdin. The local branch runs the same
//! `/bin/bash -s` the ssh branch asks the login shell for, so the marker
//! protocol is byte-identical whichever transport carried it.

use std::time::Duration;

use super::super::{
    remote_timeout, select_connection_with_key, ssh_script_argv, target_is_this_host,
    UsedConnection,
};
use crate::deploy::{ssh_key, CommandOutput, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Run one fixed script (fed on stdin) on a resolved target.
///
/// The local branch runs the same `/bin/bash -s` the ssh branch asks the
/// login shell for, so the marker protocol on the far side is byte-identical
/// whichever transport carried it.
pub async fn run_script(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_script_with_timeout(target, script, remote_timeout(), runner).await
}

/// Run a fixed remote script with an operation-specific wall-clock bound.
/// Connection setup remains bounded by the shared SSH options.
pub async fn run_script_with_timeout(
    target: &ComputeTarget,
    script: &str,
    timeout: Duration,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    Ok(
        run_script_with_timeout_and_connection(target, script, timeout, runner)
            .await?
            .0,
    )
}

/// Run a fixed script and report which declared connection carried it.
pub async fn run_script_with_timeout_and_connection<'a>(
    target: &'a ComputeTarget,
    script: &str,
    timeout: Duration,
    runner: &Runner,
) -> Result<(CommandOutput, UsedConnection<'a>), DeployError> {
    let (argv, _key, used_connection) = if target_is_this_host(target) {
        (
            vec!["/bin/bash".to_string(), "-s".to_string()],
            None,
            UsedConnection::Local,
        )
    } else {
        let key = ssh_key::materialize(target.channel_key()).await?;
        let connection = select_connection_with_key(target, &key, runner).await?;
        let argv = ssh_key::add_identity(ssh_script_argv(connection.destination), &key)?;
        (argv, Some(key), UsedConnection::Ssh(connection))
    };
    let output = runner(CommandSpec {
        argv,
        stdin: Some(script.to_string()),
        timeout: Some(timeout),
    })
    .await
    .map_err(DeployError)?;
    Ok((output, used_connection))
}
