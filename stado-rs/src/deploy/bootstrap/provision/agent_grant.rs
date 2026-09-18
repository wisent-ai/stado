//! The dedicated workload-agent grant a remote host receives at bootstrap,
//! for every platform.
//!
//! Until 2026-09-18 only a Darwin host got one. The Linux branch wrote its
//! systemd units with no `WC_AGENT_SKARBIEC_*` declaration at all, so a
//! Linux agent had no consumer, no bearer and no item list, and declined
//! every job that declared a secret with "workload secrets require a
//! dedicated agent Skarbiec grant; leaving it queued for a host that can
//! resolve it". The fleet's only Linux builder declined every Skarbiec and
//! Brama `linux-amd64` release build for weeks that way — twelve pinned jobs,
//! the oldest twenty-five days in the queue — while `host gates` said it was
//! claiming. Same defect, second platform: the grant is now one function,
//! and the Linux unit carries the same declaration the Darwin installer
//! receives on its command line.

use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};

use super::super::install::ssh_argv;

/// Where bootstrap puts the agent's bearer on a fleet host, relative to the
/// remote account's home; the same file `service grant-sync` and the agent's
/// own renewal read.
pub(crate) const REMOTE_AGENT_TOKEN_LEAF: &str = ".stado/local-agent-skarbiec-token";

/// The declaration the remote agent runs with, once its bearer is on disk.
pub(crate) struct AgentGrant {
    pub(crate) url: String,
    pub(crate) consumer: String,
    /// The bearer's path on the remote host, spelled for the consumer: the
    /// Darwin installer expands `$HOME`, a systemd unit does not.
    pub(crate) token_file: String,
    pub(crate) items: String,
    pub(crate) secret_fields: String,
}

impl AgentGrant {
    /// `NAME=VALUE` pairs, shell-quoted, for the process that runs the agent.
    pub(crate) fn assignments(&self) -> Vec<(&'static str, String)> {
        vec![
            ("WC_AGENT_SKARBIEC_URL", self.url.clone()),
            ("WC_AGENT_SKARBIEC_CONSUMER", self.consumer.clone()),
            ("WC_AGENT_SKARBIEC_TOKEN_FILE", self.token_file.clone()),
            ("WC_AGENT_SKARBIEC_ITEMS", self.items.clone()),
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
            items: crate::config::agent_skarbiec_items().join(","),
            secret_fields: crate::config::agent_skarbiec_secret_fields().join(","),
        }
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
    let same_path = std::fs::canonicalize(grant_path)
        .ok()
        .zip(std::fs::canonicalize(crate::config::skarbiec_token_file()).ok())
        .is_some_and(|(agent, control)| agent == control);
    if grant_path.is_empty()
        || same_path
        || agent_consumer != "stado-local-agent"
        || agent_consumer == crate::config::skarbiec_consumer()
    {
        return Err(DeployError(
            "remote bootstrap requires consumer stado-local-agent and a distinct agent token_file"
                .to_string(),
        ));
    }
    if !agent_url.starts_with("https://") {
        return Err(DeployError(
            "remote bootstrap requires agent.skarbiec.url on authenticated HTTPS".to_string(),
        ));
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
        DeployError(format!(
            "cannot configure dedicated remote agent grant: {error}"
        ))
    })?;
    let mut visible = agent_vault
        .list_items()
        .await
        .map_err(|error| DeployError(format!("cannot authorize remote agent grant: {error}")))?
        .into_iter()
        .map(|item| item.id)
        .collect::<Vec<_>>();
    visible.sort();
    let mut expected = crate::config::agent_skarbiec_items().to_vec();
    expected.sort();
    expected.dedup();
    if visible != expected {
        return Err(DeployError(format!(
            "stado-local-agent grant exposes {visible:?}; expected exactly {expected:?}"
        )));
    }
    let prepare = runner(CommandSpec::new(ssh_argv(
        ssh_target,
        "umask u=rwx,go=; mkdir -p \"$HOME/.stado\"",
    )))
    .await
    .map_err(DeployError)?;
    if !prepare.ok() {
        return Err(DeployError(format!(
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
    .map_err(DeployError)?;
    if !copy.ok() {
        return Err(DeployError(format!(
            "cannot provision dedicated remote agent grant: {}",
            copy.detail()
        )));
    }
    let secure = runner(CommandSpec::new(ssh_argv(
        ssh_target,
        &format!("chmod u=rw,go= \"$HOME/{REMOTE_AGENT_TOKEN_LEAF}\""),
    )))
    .await
    .map_err(DeployError)?;
    if !secure.ok() {
        return Err(DeployError(format!(
            "cannot secure dedicated remote agent grant: {}",
            secure.detail()
        )));
    }
    Ok(AgentGrant {
        url: agent_url.to_string(),
        consumer: agent_consumer.to_string(),
        token_file: format!("{remote_home}/{REMOTE_AGENT_TOKEN_LEAF}"),
        items: crate::config::agent_skarbiec_items().join(","),
        secret_fields: crate::config::agent_skarbiec_secret_fields().join(","),
    })
}
