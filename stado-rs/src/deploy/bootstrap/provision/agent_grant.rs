//! The dedicated workload-agent grant a remote host receives at bootstrap,
//! for every platform.
//!
//! A Linux agent whose systemd unit carries no `WC_AGENT_SKARBIEC_*`
//! declaration has no consumer, no bearer and no item list, and declines
//! every job that declares a secret with "workload secrets require a
//! dedicated agent Skarbiec grant; leaving it queued for a host that can
//! resolve it" while `host gates` says it is claiming. The grant is one
//! function, and the Linux unit carries the same declaration the Darwin
//! installer receives on its command line.

use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};

use super::super::install::ssh_argv;

/// Where bootstrap puts the agent's bearer on a fleet host, relative to the
/// remote account's home; the same file `service grant sync` and the agent's
/// own renewal read.
pub(crate) const REMOTE_AGENT_TOKEN_LEAF: &str = ".stado/local-agent-skarbiec-token";

/// The declaration the remote agent runs with, once its bearer is on disk.
pub(crate) struct AgentGrant {
    pub(crate) url: String,
    pub(crate) consumer: String,
    /// The bearer's path on the remote host, spelled for the consumer: the
    /// Darwin installer expands `$HOME`, a systemd unit does not.
    pub(crate) token_file: String,
    pub(crate) roles: String,
    pub(crate) secret_fields: String,
}

impl AgentGrant {
    /// `NAME=VALUE` pairs, shell-quoted, for the process that runs the agent.
    pub(crate) fn assignments(&self) -> Vec<(&'static str, String)> {
        vec![
            ("WC_AGENT_SKARBIEC_URL", self.url.clone()),
            ("WC_AGENT_SKARBIEC_CONSUMER", self.consumer.clone()),
            ("WC_AGENT_SKARBIEC_TOKEN_FILE", self.token_file.clone()),
            ("WC_AGENT_SKARBIEC_ROLES", self.roles.clone()),
            (
                "WC_AGENT_SKARBIEC_SECRET_FIELDS",
                self.secret_fields.clone(),
            ),
            ("WC_SKARBIEC_URL", self.url.clone()),
            ("WC_SKARBIEC_CONSUMER", self.consumer.clone()),
            ("WC_SKARBIEC_TOKEN_FILE", self.token_file.clone()),
        ]
    }

    /// The assignments as one shell prefix for a command line.
    pub(crate) fn shell_prefix(&self) -> String {
        self.assignments()
            .into_iter()
            .map(|(name, value)| format!("{name}={} ", shlex_quote(&value)))
            .collect()
    }

    /// The declaration as this control plane's configuration states it,
    /// without touching the vault or the host: what a dry run previews.
    pub(crate) fn declared(remote_home: &str) -> Self {
        Self {
            url: crate::config::agent_skarbiec_url().to_string(),
            consumer: crate::config::agent_skarbiec_consumer().to_string(),
            token_file: format!("{remote_home}/{REMOTE_AGENT_TOKEN_LEAF}"),
            roles: crate::config::agent_skarbiec_roles().join(","),
            secret_fields: crate::config::agent_skarbiec_secret_fields().join(","),
        }
    }
}

/// Why this host declares no dedicated workload-agent grant to hand a
/// remote host, or `None` when it declares one: `agent.skarbiec.consumer`
/// and `agent.skarbiec.token_file`, distinct from the control plane's own
/// consumer and bearer file. Read from this host's configuration rather than
/// held to one consumer name, which refused every bootstrap from a host that
/// had not declared that exact name.
pub(super) fn undeclared() -> Option<String> {
    let grant_path = crate::config::agent_skarbiec_token_file();
    let agent_consumer = crate::config::agent_skarbiec_consumer();
    let control_path = crate::config::skarbiec_token_file();
    // Two declarations name the same file when both resolve to one path. A
    // path that does not resolve on this host (an agent host receives its
    // grant by platform handoff) names no file the other could be, so the
    // declared spellings are compared instead.
    let same_path = match (
        std::fs::canonicalize(grant_path),
        std::fs::canonicalize(control_path),
    ) {
        (Ok(agent), Ok(control)) => agent == control,
        (_agent_unresolved, _control_unresolved) => grant_path == control_path,
    };
    if grant_path.is_empty() {
        Some("agent.skarbiec.token_file is empty".to_string())
    } else if agent_consumer == crate::config::skarbiec_consumer() {
        Some(format!(
            "agent.skarbiec.consumer is {agent_consumer:?}, the control plane's own consumer"
        ))
    } else if same_path {
        Some(format!(
            "agent.skarbiec.token_file {grant_path} is the control plane's own bearer file"
        ))
    } else {
        None
    }
}

/// Validate this control plane's dedicated agent grant against the vault,
/// copy the bearer to the remote host under `remote_home`, and return the
/// declaration the remote agent must run with.
pub(super) async fn provision_agent_grant(
    ssh_target: &str,
    remote_home: &str,
    runner: &Runner,
) -> Result<AgentGrant, DeployError> {
    // A remote workstation receives only its dedicated workload-agent
    // consumer. Reusing either the control-plane consumer or its token
    // path is a closed failure before SCP runs.
    let grant_path = crate::config::agent_skarbiec_token_file();
    let agent_consumer = crate::config::agent_skarbiec_consumer();
    let agent_url = crate::config::agent_skarbiec_url();
    if let Some(reason) = undeclared() {
        return Err(DeployError(format!(
            "remote bootstrap hands over only a dedicated agent grant: {reason}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    if let Some(cause) = crate::config::agent_skarbiec_url_absent() {
        return Err(DeployError(format!(
            "remote bootstrap needs agent.skarbiec.url, which names a service this host cannot \
             resolve: {cause}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    if !agent_url.starts_with("https://") {
        return Err(DeployError(
            "remote bootstrap requires agent.skarbiec.url on authenticated HTTPS".to_string(),
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    // This validates the grant from wherever bootstrap runs, so the grant
    // file's placement is the fact available: an owner-only provisioned file
    // on the control plane, the platform's handoff on an agent host.
    let agent_vault = crate::skarbiec::Client::new(
        agent_url,
        agent_consumer,
        grant_path,
        crate::skarbiec::GrantMode::for_grant_file(grant_path),
    )
    .map_err(|error| {
        DeployError::from(error).within("cannot configure dedicated remote agent grant")
    })?;
    let visible =
        crate::skarbiec::roles::roles_played(&agent_vault.list_items().await.map_err(|error| {
            DeployError::from(error).within("cannot authorize remote agent grant")
        })?);
    let mut expected = crate::config::agent_skarbiec_roles().to_vec();
    expected.sort();
    expected.dedup();
    if visible != expected {
        return Err(DeployError(format!(
            "stado-local-agent grant exposes items playing {visible:?}; expected exactly one item \
             for each of {expected:?}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    let prepare = runner(CommandSpec::new(ssh_argv(
        ssh_target,
        "umask u=rwx,go=; mkdir -p \"$HOME/.stado\"",
    )))
    .await
    .map_err(DeployError::unreachable)?;
    if !prepare.succeeded() {
        return Err(DeployError::unreachable(format!(
            "cannot prepare remote agent grant directory: {}",
            prepare.detail()
        )));
    }
    let copy = runner(CommandSpec::new(vec![
        "scp".to_string(),
        "-q".to_string(),
        grant_path.to_string(),
        format!("{ssh_target}:{REMOTE_AGENT_TOKEN_LEAF}"),
    ]))
    .await
    .map_err(DeployError::unreachable)?;
    if !copy.succeeded() {
        return Err(DeployError::unreachable(format!(
            "cannot provision dedicated remote agent grant: {}",
            copy.detail()
        )));
    }
    let secure = runner(CommandSpec::new(ssh_argv(
        ssh_target,
        &format!("chmod u=rw,go= \"$HOME/{REMOTE_AGENT_TOKEN_LEAF}\""),
    )))
    .await
    .map_err(DeployError::unreachable)?;
    if !secure.succeeded() {
        return Err(DeployError::unreachable(format!(
            "cannot secure dedicated remote agent grant: {}",
            secure.detail()
        )));
    }
    Ok(AgentGrant {
        url: agent_url.to_string(),
        consumer: agent_consumer.to_string(),
        token_file: format!("{remote_home}/{REMOTE_AGENT_TOKEN_LEAF}"),
        roles: crate::config::agent_skarbiec_roles().join(","),
        secret_fields: crate::config::agent_skarbiec_secret_fields().join(","),
    })
}
