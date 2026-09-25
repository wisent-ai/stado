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
                "{path} is retired; remove it and configure Stado's secrets.skarbiec identity"
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
        let accepted = consumer.as_str().is_some_and(|name| {
            name == "stado"
                || (name.ends_with("-agent") && !RETIRED_AGENT_CONSUMERS.contains(&name))
        });
        if !accepted {
            problems.push(format!(
                "agent.skarbiec.consumer is {consumer}; use \"stado\" locally or a scoped *-agent grant on rented machines"
            ));
        }
    }
}
