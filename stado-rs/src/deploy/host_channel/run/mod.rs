//! Running something on a resolved target. One fixed program per call, the
//! same words whichever transport carries them, and the connection that was
//! actually used handed back to the caller.
//!
//! `remote` holds the one-line primitives a caller composes instead of
//! shipping a shell program; `script` holds the stdin-fed scripts.

use std::time::Duration;

use super::{
    remote_timeout, select_connection_with_key, ssh_program_argv, target_is_this_host,
    UsedConnection,
};
use crate::deploy::{host_access::ssh_key, CommandOutput, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

mod remote;
mod script;

pub use remote::{
    extract_semver, remote_home, remote_json_member, remote_program_version, remote_read_file,
    remote_test, run_command,
};
pub use script::{run_script, run_script_with_timeout, run_script_with_timeout_and_connection};

/// Run one fixed program on a resolved target.
///
/// A target that IS this machine runs the program directly. The words are
/// the same compile-time constants the ssh path sends, so the two transports
/// cannot answer different questions; only the hop disappears.
pub async fn run_program(
    target: &ComputeTarget,
    program: &[&str],
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_program_with_timeout(target, program, remote_timeout(), runner).await
}

/// Run one fixed program with an operation-specific wall-clock bound.
///
/// The ordinary channel timeout is deliberately large enough for recovery.
/// Small read-only probes use this form so a program that does not implement
/// the requested CLI verb cannot occupy that entire recovery budget.
pub async fn run_program_with_timeout(
    target: &ComputeTarget,
    program: &[&str],
    timeout: Duration,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_program_with_timeout_and_connection(target, program, timeout, runner)
        .await
        .map(|(output, _)| output)
}

/// Run one fixed program and return which route carried it. The route is
/// selected before the real command, so failover never repeats a side effect.
pub async fn run_program_with_connection<'a>(
    target: &'a ComputeTarget,
    program: &[&str],
    runner: &Runner,
) -> Result<(CommandOutput, UsedConnection<'a>), DeployError> {
    run_program_with_timeout_and_connection(target, program, remote_timeout(), runner).await
}

async fn run_program_with_timeout_and_connection<'a>(
    target: &'a ComputeTarget,
    program: &[&str],
    timeout: Duration,
    runner: &Runner,
) -> Result<(CommandOutput, UsedConnection<'a>), DeployError> {
    if target_is_this_host(target) {
        let output = runner(CommandSpec {
            argv: program.iter().map(|word| word.to_string()).collect(),
            stdin: None,
            timeout: Some(timeout),
        })
        .await
        .map_err(DeployError)?;
        return Ok((output, UsedConnection::Local));
    }

    let key = ssh_key::materialize(target.channel_key()).await?;
    let connection = select_connection_with_key(target, &key, runner).await?;
    let argv = ssh_key::add_identity(ssh_program_argv(connection.destination, program), &key)?;
    let output = runner(CommandSpec {
        argv,
        stdin: None,
        timeout: Some(timeout),
    })
    .await
    .map_err(DeployError)?;
    Ok((output, UsedConnection::Ssh(connection)))
}

/// Run one fixed program while feeding an opaque value on stdin.
///
/// Used for host-account authentication: the password is never placed in SSH
/// argv, the remote command, registry data, stdout, or stderr.
pub async fn run_program_with_stdin(
    target: &ComputeTarget,
    program: &[&str],
    stdin: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_program_with_stdin_and_connection(target, program, stdin, runner)
        .await
        .map(|(output, _)| output)
}

pub async fn run_program_with_stdin_and_connection<'a>(
    target: &'a ComputeTarget,
    program: &[&str],
    stdin: &str,
    runner: &Runner,
) -> Result<(CommandOutput, UsedConnection<'a>), DeployError> {
    if target_is_this_host(target) {
        let output = runner(CommandSpec {
            argv: program.iter().map(|word| word.to_string()).collect(),
            stdin: Some(stdin.to_string()),
            timeout: Some(remote_timeout()),
        })
        .await
        .map_err(DeployError)?;
        return Ok((output, UsedConnection::Local));
    }

    let key = ssh_key::materialize(target.channel_key()).await?;
    let connection = select_connection_with_key(target, &key, runner).await?;
    let argv = ssh_key::add_identity(ssh_program_argv(connection.destination, program), &key)?;
    let output = runner(CommandSpec {
        argv,
        stdin: Some(stdin.to_string()),
        timeout: Some(remote_timeout()),
    })
    .await
    .map_err(DeployError)?;
    Ok((output, UsedConnection::Ssh(connection)))
}
