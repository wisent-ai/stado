//! Rate-limit and integration verifiers use Stado's Skarbiec identity.
//! Neither verifier item may reach a job.

use serde_json::{Map, Value};

use crate::config_file::readers::field_in;

/// The rate-limit plane: declared clients parse, and no verifier item is
/// exposed to workloads.
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

/// The integration plane: its clients and providers parse, and no client's
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
    }
}
