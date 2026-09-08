//! Structural judgement of the document an operator is about to deploy.
//!
//! [`helpers`] holds the checks more than one section shares, [`document`] the
//! document's own contract and its storage backends, [`providers`] the enabled
//! provider list and the gates a cloud provider opens, and [`planes`] one
//! component per declared plane. The order of the pushes below is the order
//! `stado config validate` prints, so it is the order this function calls them
//! in.

use serde_json::{Map, Value};

use crate::config_file::readers::field_in;

mod document;
mod helpers;
mod planes;
mod providers;

/// Structural validation of a config dict; returns a list of problems.
pub fn validate(data: &Value) -> Vec<String> {
    let mut problems = crate::capabilities::validate_catalog();
    let empty = Map::new();
    let root = data.as_object().unwrap_or(&empty);
    document::schema_version(root, &mut problems);
    helpers::unresolved_placeholders(data, "", &mut problems);
    helpers::unread_storage_keys(root, &mut problems);
    document::credentials_and_alerts(root, &mut problems);
    document::storage_backends(root, &mut problems);
    let active_providers = providers::declared(root, &mut problems);
    providers::cloud_release_coordinates(root, &active_providers, &mut problems);
    providers::azure_control_plane(root, &active_providers, &mut problems);
    let configured_items = planes::workload_secret_fields(root, &mut problems);
    let control_token_file = field_in(root, &crate::capabilities::SECRETS_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let object_token_file = field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let release_token_file = field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    let machine_token_file = field_in(root, &crate::capabilities::MACHINE_API_SKARBIEC.token_file)
        .and_then(Value::as_str)
        .unwrap_or_default();
    planes::messaging(root, &mut problems);
    planes::rate_limit(root, &configured_items, &mut problems);
    planes::integration(root, &configured_items, &mut problems);
    planes::object_api(root, control_token_file, object_token_file, &mut problems);
    planes::database_api(root, &mut problems);
    planes::web_api(root, &mut problems);
    planes::release_api(
        root,
        control_token_file,
        object_token_file,
        release_token_file,
        &mut problems,
    );
    planes::machine_api(
        root,
        control_token_file,
        object_token_file,
        release_token_file,
        machine_token_file,
        &mut problems,
    );
    planes::service_api(
        root,
        &active_providers,
        control_token_file,
        object_token_file,
        release_token_file,
        machine_token_file,
        &mut problems,
    );
    let port = field_in(root, &crate::capabilities::DASHBOARD_PORT_CONFIG);
    if let Some(port) = port.filter(|p| !p.is_null()) {
        let ok = port.as_i64().is_some_and(|p| p > 0 && p < 65536);
        if !ok {
            problems.push("dashboard.port must be an int between 1 and 65535".to_string());
        }
    }
    problems
}
