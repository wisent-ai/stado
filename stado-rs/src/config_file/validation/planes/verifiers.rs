//! The two verifier planes whose grants are least-privilege by construction:
//! rate limiting and third-party integration. Each has to name its own
//! consumer and its own token file, and neither item may reach a job.

use serde_json::{Map, Value};

use crate::config_file::readers::{field_in, py_truthy};

/// The rate-limit verifier: declared clients, dedicated consumer, an
/// owner-only grant file shared with nothing, and no verifier item exposed to
/// workloads.
pub(in crate::config_file::validation) fn rate_limit(
    root: &Map<String, Value>,
    configured_items: &[Value],
    problems: &mut Vec<String>,
) {
    let rate_limit = root.get("rate_limit").and_then(Value::as_object);
    if rate_limit.is_some() {
        if let Err(problem) = crate::rate_limit::parse_clients(
            field_in(root, &crate::capabilities::RATE_LIMIT_CLIENTS_CONFIG).cloned(),
        ) {
            problems.push(problem);
        }
        let rate_skarbiec = rate_limit
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "rate_limit.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "rate_limit.skarbiec.consumer must be the dedicated verifier {:?}",
                crate::config::RATE_LIMIT_API_VERIFIER_CONSUMER
            ));
        }
        let rate_token_file = field_in(root, &crate::capabilities::RATE_LIMIT_SKARBIEC.token_file)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if rate_token_file.is_empty() {
            problems.push(
            "rate_limit.skarbiec.token_file must name the owner-only rate-limit verifier grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::BACKEND_MESSAGING_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::MACHINE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !rate_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(rate_token_file)
            {
                problems.push(format!(
                    "rate_limit.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
                ));
            }
        }
        if rate_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "rate_limit.skarbiec.token is forbidden; store the grant only in its owner-only token_file"
                .to_string(),
        );
        }
        if configured_items
            .iter()
            .any(|configured| configured.as_str() == Some("trading-autonomy-rate-limit-api"))
        {
            problems.push(
                "agent.skarbiec.items must not expose rate-limit verifier items to jobs"
                    .to_string(),
            );
        }
    }
}

/// The integration plane: its clients and providers parse, its verifier
/// consumer is the dedicated one, its grant file is its own, and no client's
/// item is readable by a job.
pub(in crate::config_file::validation) fn integration(
    root: &Map<String, Value>,
    configured_items: &[Value],
    problems: &mut Vec<String>,
) {
    let integration = root.get("integration").and_then(Value::as_object);
    if integration.is_some() {
        let integration_clients = crate::config::parse_integration_clients(field_in(
            root,
            &crate::capabilities::INTEGRATION_CLIENTS_CONFIG,
        ));
        match &integration_clients {
            Ok(clients) => {
                for item in clients.values().map(|client| client.item()) {
                    if configured_items
                        .iter()
                        .any(|configured| configured.as_str() == Some(item))
                    {
                        problems.push(format!(
                        "agent.skarbiec.items must not expose integration verifier item {item:?} to jobs"
                    ));
                    }
                }
            }
            Err(integration_problems) => problems.extend(integration_problems.iter().cloned()),
        }
        if integration.is_some_and(|section| section.contains_key("providers")) {
            if let Err(provider_problems) = crate::config::parse_integration_providers(field_in(
                root,
                &crate::capabilities::INTEGRATION_PROVIDERS_CONFIG,
            )) {
                problems.extend(provider_problems);
            }
        }
        let integration_skarbiec = integration
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "integration.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::INTEGRATION_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "integration.skarbiec.consumer must be the dedicated verifier {:?}",
                crate::config::INTEGRATION_API_VERIFIER_CONSUMER
            ));
        }
        let integration_token_file =
            field_in(root, &crate::capabilities::INTEGRATION_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
        if integration_token_file.is_empty() {
            problems.push(
            "integration.skarbiec.token_file must name the owner-only integration verifier grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::BACKEND_MESSAGING_SKARBIEC,
            crate::capabilities::RATE_LIMIT_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::MACHINE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !integration_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(integration_token_file)
            {
                problems.push(format!(
                    "integration.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
                ));
            }
        }
        if integration_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "integration.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
        }
    }
}
