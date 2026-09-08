//! The broker this host's queue agent reads workload secrets through.

use crate::config;
use crate::doctor::Check;

pub(in crate::doctor) const AGENT_SKARBIEC_ID: &str = "agent-skarbiec";
pub(in crate::doctor) const AGENT_SKARBIEC_TITLE: &str = "The agent's own Skarbiec broker answers";
pub(in crate::doctor) const AGENT_SKARBIEC_REMEDY: &str =
    "run `stado repair stado-control-plane --step agent-skarbiec --target <target> --apply`, \
     which sets agent.skarbiec.url to the credential endpoint the service directory declares \
     for that host; a queue agent whose broker is unreachable can claim no job that declares \
     secret_env. A broker that answers slowly or stops answering mid-request is usually a \
     wedged GnuPG daemon: run `skarbiec recover-daemons` on that host";

/// Whether this host's queue agent can reach the broker it is configured to
/// read workload secrets through.
///
/// Nothing reported this. On 2026-09-05 this laptop's `agent.skarbiec.url`
/// was `http://127.0.0.1:19096` with nothing listening — three brokers were
/// running, on 9877, 8799 and 8787, none of them that one — and the only
/// symptom was a `preferences` release job dying after it had been claimed:
/// `cannot resolve job … secret GITHUB_TOKEN: error sending request for url
/// (http://127.0.0.1:19096/v1/items/read)`. A misconfiguration that only
/// surfaces as another product's failed build is one an operator cannot find.
///
/// Metadata only: `list_items` returns ids, never values.
pub(in crate::doctor) async fn check_agent_skarbiec() -> Check {
    let url = config::agent_skarbiec_url();
    if url.trim().is_empty() {
        return Check::pass(
            AGENT_SKARBIEC_ID,
            AGENT_SKARBIEC_TITLE,
            "agent.skarbiec.url is unset, so the agent reads workload secrets through the \
             configured store client and has no separate broker to reach"
                .to_string(),
            AGENT_SKARBIEC_REMEDY,
        );
    }
    let token_file = config::agent_skarbiec_token_file();
    let consumer = config::agent_skarbiec_consumer();
    let client = match crate::skarbiec::Client::direct(
        url,
        consumer,
        token_file,
        crate::skarbiec::GrantMode::for_grant_file(token_file),
    ) {
        Ok(client) => client,
        Err(error) => {
            return Check::fail(
                AGENT_SKARBIEC_ID,
                AGENT_SKARBIEC_TITLE,
                format!("agent consumer {consumer} cannot be configured against {url}: {error}"),
                AGENT_SKARBIEC_REMEDY,
            )
        }
    };
    match client.list_items().await {
        Ok(items) => Check::pass(
            AGENT_SKARBIEC_ID,
            AGENT_SKARBIEC_TITLE,
            format!(
                "agent consumer {consumer} reaches {url} and its grant exposes {} item(s)",
                items.len()
            ),
            AGENT_SKARBIEC_REMEDY,
        ),
        Err(error) => Check::fail(
            AGENT_SKARBIEC_ID,
            AGENT_SKARBIEC_TITLE,
            format!(
                "agent consumer {consumer} cannot read through {url}: {error}. This host can \
                 claim no job that declares secret_env"
            ),
            AGENT_SKARBIEC_REMEDY,
        ),
    }
}
