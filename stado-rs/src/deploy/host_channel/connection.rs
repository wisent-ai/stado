//! Which declared SSH connection a target is actually reached through:
//! every path a registry declares is probed with the same fixed program, and
//! the first one that answers is the one every later command rides.

use super::{
    last_error_line, ssh_program_argv, target_is_this_host, SshConnection, SshConnectionProbe,
    CONNECTION_PROBE_PROGRAM, CONNECTION_PROBE_TIMEOUT, HOST_SESSION,
};
use crate::deploy::{host_access::ssh_key, py_str_repr, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

fn declared_connections(target: &ComputeTarget) -> impl Iterator<Item = SshConnection<'_>> {
    target
        .ssh_connections()
        .map(|(name, destination)| SshConnection { name, destination })
}

async fn probe_connection(
    connection: SshConnection<'_>,
    key: &ssh_key::KeyFile,
    runner: &Runner,
) -> SshConnectionProbe {
    let argv = match ssh_key::add_identity(
        ssh_program_argv(connection.destination, &CONNECTION_PROBE_PROGRAM),
        key,
    ) {
        Ok(argv) => argv,
        Err(error) => {
            return SshConnectionProbe {
                name: connection.name.to_string(),
                destination: connection.destination.to_string(),
                reachable: false,
                error: Some(error.to_string()),
            };
        }
    };
    let result = runner(CommandSpec {
        argv,
        stdin: None,
        timeout: Some(CONNECTION_PROBE_TIMEOUT),
    })
    .await;
    match result {
        Ok(output) if output.ok() => SshConnectionProbe {
            name: connection.name.to_string(),
            destination: connection.destination.to_string(),
            reachable: true,
            error: None,
        },
        Ok(output) => SshConnectionProbe {
            name: connection.name.to_string(),
            destination: connection.destination.to_string(),
            reachable: false,
            error: Some(last_error_line(&output, "SSH path refused the connection")),
        },
        Err(error) => SshConnectionProbe {
            name: connection.name.to_string(),
            destination: connection.destination.to_string(),
            reachable: false,
            error: Some(error),
        },
    }
}

pub(in crate::deploy::host_channel) async fn select_connection_with_key<'a>(
    target: &'a ComputeTarget,
    key: &ssh_key::KeyFile,
    runner: &Runner,
) -> Result<SshConnection<'a>, DeployError> {
    if let Ok(Some(connection)) = HOST_SESSION.try_with(|session| {
        (session.target == target.name)
            .then(|| {
                declared_connections(target).find(|connection| {
                    connection.name == session.connection_name
                        && connection.destination == session.destination
                })
            })
            .flatten()
    }) {
        return Ok(connection);
    }
    let mut connections = declared_connections(target);
    let Some(first) = connections.next() else {
        return Err(DeployError(format!(
            "target {} has no registry-managed SSH connection path",
            py_str_repr(&target.name)
        )));
    };
    let Some(second) = connections.next() else {
        return Ok(first);
    };

    let mut failures = Vec::new();
    for connection in std::iter::once(first)
        .chain(std::iter::once(second))
        .chain(connections)
    {
        let probe = probe_connection(connection, key, runner).await;
        if probe.reachable {
            return Ok(connection);
        }
        failures.push(format!(
            "{}: {}",
            connection.name,
            probe
                .error
                .as_deref()
                .unwrap_or("connection probe returned no detail")
        ));
    }
    Err(DeployError(format!(
        "target {} has no reachable SSH connection path ({})",
        py_str_repr(&target.name),
        failures.join("; ")
    )))
}

/// Select the first declared path that authenticates without sending the real
/// operation. A single-path target keeps the old one-connection behavior.
pub async fn select_ssh_connection<'a>(
    target: &'a ComputeTarget,
    runner: &Runner,
) -> Result<SshConnection<'a>, DeployError> {
    let key = ssh_key::materialize(target.channel_key()).await?;
    select_connection_with_key(target, &key, runner).await
}

/// Probe every declared route in order. This is a diagnostic operation; normal
/// commands stop probing as soon as one route answers.
pub async fn probe_ssh_connections(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<SshConnectionProbe>, DeployError> {
    let local = target_is_this_host(target);
    let connections = declared_connections(target).collect::<Vec<_>>();
    let mut probes = Vec::with_capacity(connections.len() + usize::from(local));
    if local {
        probes.push(SshConnectionProbe {
            name: "local".to_string(),
            destination: "local process".to_string(),
            reachable: true,
            error: None,
        });
    }
    if connections.is_empty() {
        return Ok(probes);
    }
    let key = match ssh_key::materialize(target.channel_key()).await {
        Ok(key) => key,
        Err(error) if local => {
            probes.extend(
                connections
                    .into_iter()
                    .map(|connection| SshConnectionProbe {
                        name: connection.name.to_string(),
                        destination: connection.destination.to_string(),
                        reachable: false,
                        error: Some(error.to_string()),
                    }),
            );
            return Ok(probes);
        }
        Err(error) => return Err(error),
    };
    for connection in connections {
        probes.push(probe_connection(connection, &key, runner).await);
    }
    Ok(probes)
}
