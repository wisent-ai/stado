//! Which declared SSH connection a target is actually reached through:
//! every path a registry declares is probed with the same fixed program, and
//! the first one that answers is the one every later command rides.

use std::future::Future;

use super::{
    last_error_line, ssh_program_argv, target_is_this_host, SshConnection, SshConnectionProbe,
    CONNECTION_PROBE_PROGRAM, HOST_SESSION,
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
    let result = runner(CommandSpec { argv, stdin: None }).await;
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

tokio::task_local! {
    /// The declared service whose repair this task applies.
    static REPAIRING: String;
}

/// Run `operation` as the declared repair of `service` (`stado repair
/// <service> --apply`). A repair is the one command a host whose services
/// stand must still be sent: the vault's daemon repair exists for the wedge
/// that holds the resolver's requests to that host, and refusing it there
/// would leave the wedge with no product path out of it. Under this scope
/// the held reading is said on stderr and the command goes.
pub(crate) async fn repairing<T>(service: &str, operation: impl Future<Output = T>) -> T {
    REPAIRING.scope(service.to_string(), operation).await
}

/// What this host's resolver publishes about `target`: the held reading when
/// it holds connections to that host unanswered longer than its refresh
/// interval (`resolver::held_on_host`), `None` when it holds none or the
/// target is this host, and a refusal when the published list is there and
/// cannot be read, since nobody can then say whether the host answers.
fn held_reading(target: &ComputeTarget) -> Result<Option<String>, DeployError> {
    if target_is_this_host(target) {
        return Ok(None);
    }
    crate::cli::resolver::held_on_host(&target.name).map_err(|unreadable| {
        DeployError::unreachable(format!(
            "whether {} answers cannot be told before sending to it: {unreadable}",
            target.name
        ))
    })
}

/// The key the target's command channel authenticates with. A host the
/// resolver holds unanswered is not refused here: the resolver's waits ride
/// its own SSH session, and the command channel is a new connection that
/// [`select_connection_with_key`] asks first. When the key cannot be read
/// for a held host, the refusal names the held wait beside the key failure,
/// so the vault's error does not stand in for the host's state.
pub(in crate::deploy::host_channel) async fn channel_key(
    target: &ComputeTarget,
) -> Result<ssh_key::KeyFile, DeployError> {
    let held = held_reading(target)?;
    if let (Some(held), Ok(service)) = (&held, REPAIRING.try_with(String::clone)) {
        eprintln!(
            "stado: sending the declared repair of {service} to {} although it stands: {held}",
            target.name
        );
    }
    match (ssh_key::materialize(target.channel_key()).await, held) {
        (Ok(key), _) => Ok(key),
        (Err(error), None) => Err(error),
        (Err(error), Some(held)) => Err(DeployError::unreachable(format!(
            "{held}; and the key {}'s command channel authenticates with cannot be read: {error}",
            target.name
        ))),
    }
}

/// The first declared path the target is reached through, authenticating
/// with `key` from [`channel_key`]. A host this resolver holds unanswered
/// is asked over every declared path even when only one is declared: a path
/// that answers proves the host's command channel works whatever the
/// resolver's session stands on, and the command goes with the held reading
/// said on stderr; when no path answers, the refusal names the held wait and
/// every path's failure. A declared repair ([`repairing`]) skips the probe.
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
    let held = match REPAIRING.try_with(|_| ()) {
        Ok(()) => None,
        Err(_) => held_reading(target)?,
    };
    let declared: Vec<SshConnection<'a>> = declared_connections(target).collect();
    match (&held, declared.as_slice()) {
        (_, []) => {
            return Err(DeployError(format!(
                "target {} has no registry-managed SSH connection path",
                py_str_repr(&target.name)
            ))
            .stating(crate::primitives::failure::FailureCode::Config))
        }
        (None, [only]) => return Ok(*only),
        _ => {}
    }

    let mut failures = Vec::new();
    for connection in declared {
        let probe = probe_connection(connection, key, runner).await;
        if probe.reachable {
            if let Some(held) = &held {
                eprintln!(
                    "stado: {} answers over {} ({}) although this host's resolver holds it: \
                     {held}; the command goes over that channel",
                    target.name, connection.name, connection.destination
                );
            }
            return Ok(connection);
        }
        failures.push(match probe.error {
            Some(error) => format!("{}: {error}", connection.name),
            None => format!("{}: the probe failed without saying why", connection.name),
        });
    }
    let unreachable = format!(
        "target {} has no reachable SSH connection path ({})",
        py_str_repr(&target.name),
        failures.join("; ")
    );
    Err(DeployError::unreachable(match held {
        Some(held) => format!("{held}; {unreachable}"),
        None => unreachable,
    }))
}

/// Select the first declared path that authenticates without sending the real
/// operation. A single-path target keeps the old one-connection behavior.
pub async fn select_ssh_connection<'a>(
    target: &'a ComputeTarget,
    runner: &Runner,
) -> Result<SshConnection<'a>, DeployError> {
    let key = channel_key(target).await?;
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
