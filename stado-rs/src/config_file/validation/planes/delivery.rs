//! The two planes that hand work to a host: machine enrollment and service
//! deployment. Both are verified through Stado's Skarbiec identity.

use serde_json::{Map, Value};

use crate::config_file::readers::field_in;

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
