//! The planes Stado's Skarbiec identity verifies: the rate-limit and
//! integration verifiers, neither of whose items may reach a job, and the two
//! planes that hand work to a host, machine enrollment and service deployment.

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
        match crate::rate_limit::parse_clients(
            field_in(root, &crate::capabilities::RATE_LIMIT_CLIENTS_CONFIG).cloned(),
        ) {
            Err(problem) => problems.push(problem),
            Ok(clients) => {
                for role in clients.values().map(|client| client.item()) {
                    if configured_items
                        .iter()
                        .any(|configured| configured.as_str() == Some(role))
                    {
                        problems.push(format!(
                            "agent.skarbiec.roles must not expose rate-limit verifier role {role:?} to jobs"
                        ));
                    }
                }
            }
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
                for role in clients.values().map(|client| client.item()) {
                    if configured_items
                        .iter()
                        .any(|configured| configured.as_str() == Some(role))
                    {
                        problems.push(format!(
                        "agent.skarbiec.roles must not expose integration verifier role {role:?} to jobs"
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

/// The machine plane: clients that parse.
pub(in crate::config_file::validation) fn machine_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    let machine_api = root.get("machine_api").and_then(Value::as_object);
    if machine_api.is_some() {
        if let Err(machine_problems) = crate::config::parse_machine_api_clients(field_in(
            root,
            &crate::capabilities::MACHINE_API_CLIENTS_CONFIG,
        )) {
            problems.extend(machine_problems);
        }
    }
}

/// The service plane: deployers that parse.
pub(in crate::config_file::validation) fn service_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    let service_api = root.get("service_api").and_then(Value::as_object);
    if service_api.is_some() {
        if let Err(service_problems) = crate::config::parse_service_deployers(field_in(
            root,
            &crate::capabilities::SERVICE_API_DEPLOYERS_CONFIG,
        )) {
            problems.extend(service_problems);
        }
    }
}
