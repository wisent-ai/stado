//! Whether a placement move runs on this host or on the registry authority,
//! and handing it to the authority when it is another host.

use std::process::Stdio;

use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::Registry;

/// Whether the host this binary runs on is the directory authority, the one
/// host allowed to commit a placement transaction. `Ok(None)` is a document
/// with no directory: every host may then run the transaction itself.
fn local_is_authority(document: &Value, registry: &Registry) -> Result<Option<bool>, String> {
    let Some(directory) = crate::service_resolution::directory(document)? else {
        return Ok(None);
    };
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("placement host {hostname:?} has no registry target identity"))?;
    Ok(Some(local.name == directory.authority.target))
}

/// Run the move on the registry authority over its SSH channel when that is
/// another host, and answer whether it did. The authority's own output is
/// the operator's receipt; a remote exit other than success is a refusal.
pub(super) async fn delegate_to_registry_authority(
    document: &Value,
    registry: &Registry,
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<bool, CmdError> {
    let Some(directory) =
        crate::service_resolution::directory(document).map_err(CmdError::declaration)?
    else {
        return Ok(false);
    };
    // `None` is a document with no directory, where every host runs the
    // transaction itself; only a directory naming another host delegates.
    match local_is_authority(document, registry).map_err(CmdError::declaration)? {
        Some(false) => {}
        Some(true) | None => return Ok(false),
    }
    let authority = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| {
            CmdError::click("registry authority target disappeared")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
    let runner = crate::deploy::production_runner();
    let connection = crate::deploy::host_channel::select_ssh_connection(authority, &runner)
        .await
        .map_err(CmdError::from)?;
    let mut argv = vec![
        directory.authority.command,
        "placement".to_string(),
        "move".to_string(),
        "--to-host".to_string(),
        to_host.to_string(),
    ];
    if json_output {
        argv.push("--json".to_string());
    }
    argv.extend(requested.iter().cloned());
    let remote_command = argv
        .iter()
        .map(|argument| crate::deploy::shlex_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let ssh_options = crate::deploy::host_channel::ssh_options(connection.destination);
    let (ssh_program, ssh_rest) = ssh_options.split_first().ok_or_else(|| {
        CmdError::click("registry authority SSH channel is empty")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut ssh_argv = vec![ssh_program.clone(), "-T".to_string()];
    ssh_argv.extend_from_slice(ssh_rest);
    ssh_argv.push(remote_command);
    let key = crate::deploy::host_access::ssh_key::materialize(authority.channel_key())
        .await
        .map_err(CmdError::from)?;
    let ssh_argv = crate::deploy::host_access::ssh_key::add_identity(ssh_argv, &key)
        .map_err(CmdError::from)?;
    let (program, arguments) = ssh_argv.split_first().ok_or_else(|| {
        CmdError::click("registry authority SSH channel is empty")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let status = crate::wait::status_async(
        tokio::process::Command::new(program)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
    )
    .await
    .map_err(|error| {
        CmdError::click(format!("registry authority SSH failed: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if !status.success() {
        return Err(
            CmdError::click(format!("registry authority placement exited with {status}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    Ok(true)
}
