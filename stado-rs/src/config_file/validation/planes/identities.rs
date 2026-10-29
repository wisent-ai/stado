//! Refuse identity settings that no longer have a reader. A stale document
//! must not silently select a different grant from the one the operator expects.

use serde_json::{Map, Value};

use crate::config_file::readers::get_in;

const RETIRED_SECTIONS: &[&str] = &[
    "credentials.admin",
    "alerts.skarbiec",
    "object_api.skarbiec",
    "release_api.skarbiec",
    "release.publisher_skarbiec",
    "machine_api.skarbiec",
    "service_api.skarbiec",
    "rate_limit.skarbiec",
    "integration.skarbiec",
    "integration.provider_skarbiec",
    "backend.push_skarbiec",
];

const RETIRED_KEYS: &[&str] = &[
    "backend.messaging.skarbiec.url",
    "backend.messaging.skarbiec.consumer",
    "backend.messaging.skarbiec.token_file",
    "backend.messaging.skarbiec.token",
    "agent.skarbiec.token",
    // Replaced by agent.skarbiec.roles: secrets are asked for by role.
    "agent.skarbiec.items",
];

const RETIRED_AGENT_CONSUMERS: &[&str] = &[
    "stado-local-agent",
    "stado-azure-agent",
    "stado-control-plane",
];

pub(in crate::config_file::validation) fn retired_identities(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    for path in RETIRED_SECTIONS.iter().chain(RETIRED_KEYS) {
        if get_in(root, path).is_some() {
            problems.push(format!(
                "{path} is retired; `stado config migrate` removes it (declare \
                 workload roles with `stado release catalog enroll`, which writes \
                 agent.skarbiec.roles)"
            ));
        }
    }
    if let Some(consumer) = get_in(root, "secrets.skarbiec.consumer") {
        if consumer.as_str() != Some("stado") {
            problems.push(format!(
                "secrets.skarbiec.consumer is {consumer}; Stado's Skarbiec identity must be \"stado\""
            ));
        }
    }
    if let Some(consumer) = get_in(root, "agent.skarbiec.consumer") {
        let refusal = match consumer.as_str() {
            Some(name) => agent_consumer_refusal(name),
            None => Some(format!("agent.skarbiec.consumer is {consumer}, not a name")),
        };
        if let Some(refusal) = refusal {
            problems.push(refusal);
        }
    }
}

/// Why `name` cannot be a workload agent's Skarbiec consumer, or `None` when
/// it can: `stado` on a local host, or a scoped `*-agent` grant that is not a
/// retired identity.
fn agent_consumer_refusal(name: &str) -> Option<String> {
    if let Some(retired) = retired_consumer_refusal(name) {
        Some(retired)
    } else if name == "stado" || name.ends_with("-agent") {
        None
    } else {
        Some(format!(
            "agent.skarbiec.consumer is \"{name}\"; use \"stado\" locally or a scoped *-agent \
             grant on rented machines"
        ))
    }
}

/// Why `name` is a retired Skarbiec identity no host accepts, or `None`.
/// `credentials token mint` asks this before minting, so a consumer no host
/// would run with is never created.
pub(crate) fn retired_consumer_refusal(name: &str) -> Option<String> {
    if RETIRED_AGENT_CONSUMERS.contains(&name) {
        Some(format!(
            "\"{name}\" is a retired agent identity that no host's configuration accepts; use \
             \"stado\" locally or a new scoped *-agent consumer"
        ))
    } else {
        None
    }
}
