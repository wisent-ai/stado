//! The runtime invocation: where one host's Weles admission API listens, and
//! the held-open path this command reaches it through.

mod actions;
mod forward;
mod requests;
mod token;

use super::{ADMISSION_SERVICE, REQUEST_DEADLINE};
use crate::deploy::{host_channel, DeployError};
use crate::targets::ComputeTarget;
use forward::{await_forward, free_loopback_port};
use token::{read_token, Token};

pub use actions::{checked_account_id, latest_action_log, observe_action_payload, run_action};

/// Where one host's Weles admission API listens, as the service directory
/// declares it.
#[derive(Debug, Clone)]
pub struct Admission {
    pub target: ComputeTarget,
    /// The loopback port the API binds ON THE TARGET.
    pub port: u16,
    /// The directory's own address string, for the report.
    pub declared_url: String,
}

/// Resolve TARGET and the admission endpoint it serves.
///
/// Both come from the canonical registry: [`host_channel::resolve_target`]
/// makes the same refusals every other host command makes, and the service
/// directory supplies the port. `active_host` is checked because these
/// endpoints are keyed by the machine ASKING — the entry for the serving host
/// is the loopback address it serves on, and no other host's entry is.
pub async fn resolve_admission(target: &str) -> Result<Admission, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let resolved = host_channel::resolve_target(&registry, target)?.clone();
    let service = registry.service(ADMISSION_SERVICE).ok_or_else(|| {
        DeployError(format!(
            "the service directory carries no {ADMISSION_SERVICE} entry, so nothing declares where the Weles admission API listens"
        ))
    })?;
    if service.active_host != resolved.name {
        return Err(DeployError(format!(
            "the service directory says {ADMISSION_SERVICE} runs on {}, not on {}",
            service.active_host, resolved.name
        )));
    }
    let endpoint = service.address_for(&resolved.name).ok_or_else(|| {
        DeployError(format!(
            "the service directory declares no {ADMISSION_SERVICE} address for {}",
            resolved.name
        ))
    })?;
    let url = url::Url::parse(&endpoint.url).map_err(|error| {
        DeployError(format!(
            "the {ADMISSION_SERVICE} address {} is not a URL: {error}",
            endpoint.url
        ))
    })?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"));
    let port = url.port_or_known_default();
    match (url.scheme(), loopback, port) {
        ("http", true, Some(port)) => Ok(Admission {
            target: resolved,
            port,
            declared_url: endpoint.url.clone(),
        }),
        _ => Err(DeployError(format!(
            "the {ADMISSION_SERVICE} address {} is not a loopback http listener, and this command forwards a loopback port rather than dialling anything else",
            endpoint.url
        ))),
    }
}

/// An open path to one host's loopback admission API, alive for exactly as
/// long as this value is.
pub struct Channel {
    /// The ssh process holding the forward, or `None` when the target is this
    /// machine and there is no hop to make. `kill_on_drop` is what makes the
    /// forward end with the command: nothing is daemonised and no marker is
    /// written, which is the whole difference from `host forward-remote`.
    forward: Option<tokio::process::Child>,
    base_url: String,
    client: reqwest::Client,
    token: Token,
}

/// Open the channel to a resolved admission endpoint.
pub async fn open_channel(admission: &Admission) -> Result<Channel, DeployError> {
    let token = read_token().await;
    let client = reqwest::Client::builder()
        .timeout(REQUEST_DEADLINE)
        .build()
        .map_err(|error| {
            DeployError(format!(
                "cannot build the Weles admission API client: {error}"
            ))
        })?;
    if host_channel::target_is_this_host(&admission.target) {
        return Ok(Channel {
            forward: None,
            base_url: format!("http://127.0.0.1:{}", admission.port),
            client,
            token,
        });
    }
    let runner = crate::deploy::production_runner();
    let connection = host_channel::select_ssh_connection(&admission.target, &runner).await?;
    let local_port = free_loopback_port()?;
    let mut argv = host_channel::ssh_options(connection.destination);
    let destination = argv
        .pop()
        .ok_or_else(|| DeployError("SSH channel has no destination".to_string()))?;
    argv.extend([
        "-N".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        "-L".to_string(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{}", admission.port),
        destination,
    ]);
    let key =
        crate::deploy::host_access::ssh_key::materialize(admission.target.channel_key()).await?;
    let argv = crate::deploy::host_access::ssh_key::add_identity(argv, &key)?;
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| DeployError("SSH channel is empty".to_string()))?;
    let mut child = tokio::process::Command::new(program)
        .args(arguments)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            DeployError(format!(
                "cannot start SSH forwarding to the Weles admission API: {error}"
            ))
        })?;
    await_forward(&mut child, local_port).await?;
    drop(key);
    Ok(Channel {
        forward: Some(child),
        base_url: format!("http://127.0.0.1:{local_port}"),
        client,
        token,
    })
}
