//! The two planes that hand work to a host: machine enrollment and service
//! deployment. Both carry a verifier grant that has to stay distinct from
//! every grant declared before it, and the service plane is where a local
//! workload's own agent grant is judged.

use serde_json::{Map, Value};

use crate::config_file::readers::{field_in, py_truthy};

/// The machine plane: clients that parse, and a verifier grant distinct from
/// the coordinator, workload-agent, object and release grants.
pub(in crate::config_file::validation) fn machine_api(
    root: &Map<String, Value>,
    control_token_file: &str,
    object_token_file: &str,
    release_token_file: &str,
    machine_token_file: &str,
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
        let machine_skarbiec = machine_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "machine_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::MACHINE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "machine_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::MACHINE_API_VERIFIER_CONSUMER
            ));
        }
        if machine_token_file.is_empty() {
            problems.push(
            "machine_api.skarbiec.token_file must name the owner-only machine verifier grant file"
                .to_string(),
        );
        }
        if !machine_token_file.is_empty()
            && (machine_token_file == control_token_file
                || machine_token_file == object_token_file
                || machine_token_file == release_token_file
                || machine_token_file
                    == field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file)
                        .and_then(Value::as_str)
                        .unwrap_or_default())
        {
            problems.push(
            "machine_api.skarbiec.token_file must be distinct from coordinator, workload-agent, object, and release verifier grants"
                .to_string(),
        );
        }
        if machine_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "machine_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
        }
    }
}

/// The service plane: deployers that parse, a verifier grant distinct from the
/// four declared before it, and — when a local provider is enabled and the
/// document exposes workload fields — an agent grant that is not the control
/// plane's.
pub(in crate::config_file::validation) fn service_api(
    root: &Map<String, Value>,
    active_providers: &[crate::capabilities::ProviderId],
    control_token_file: &str,
    object_token_file: &str,
    release_token_file: &str,
    machine_token_file: &str,
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
        let service_skarbiec = service_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "service_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::SERVICE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "service_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::SERVICE_API_VERIFIER_CONSUMER
            ));
        }
        let service_token_file =
            field_in(root, &crate::capabilities::SERVICE_API_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
        if service_token_file.is_empty() {
            problems.push(
            "service_api.skarbiec.token_file must name the owner-only service verifier grant file"
                .to_string(),
        );
        }
        if !service_token_file.is_empty()
            && (service_token_file == control_token_file
                || service_token_file == object_token_file
                || service_token_file == release_token_file
                || service_token_file == machine_token_file)
        {
            problems.push(
            "service_api.skarbiec.token_file must be distinct from coordinator, product-object, release, and machine verifier grants"
                .to_string(),
        );
        }
        if service_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "service_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
        }
        let local_provider = active_providers.contains(&crate::capabilities::ProviderId::Local);
        let has_workload_fields = field_in(
            root,
            &crate::capabilities::AGENT_SKARBIEC_SECRET_FIELDS_CONFIG,
        )
        .and_then(Value::as_array)
        .is_some_and(|fields| !fields.is_empty());
        if local_provider && has_workload_fields {
            if field_in(root, &crate::capabilities::AGENT_SKARBIEC.consumer).and_then(Value::as_str)
                != Some("stado-local-agent")
            {
                problems.push(
                    "local workload secrets require agent.skarbiec.consumer stado-local-agent"
                        .to_string(),
                );
            }
            let agent_token = field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
            let control_token = field_in(root, &crate::capabilities::SECRETS_SKARBIEC.token_file)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if agent_token.is_empty() || agent_token == control_token {
                problems.push(
                "local workload secrets require an agent.skarbiec.token_file distinct from the control-plane grant"
                    .to_string(),
            );
            }
        }
    }
}
