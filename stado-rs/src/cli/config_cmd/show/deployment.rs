//! The stretch of `config show` that names this deployment: the Azure
//! account it bills to and the cloud it rents from, the API and enrollment
//! endpoints it talks to, the release it expects to be running, and its own
//! identifier.

use serde_json::{Map, Value};

use crate::config;

pub(super) fn insert(resolved: &mut Map<String, Value>) {
    resolved.insert(
        "azure_subscription_id".into(),
        Value::from(config::azure_subscription_id()),
    );
    resolved.insert(
        "azure_vm_identity_id".into(),
        Value::from(config::azure_vm_identity_id()),
    );
    // The endpoints every Azure call reaches, as resolved: each declared
    // one, the public cloud's for the rest, or why the declaration is refused.
    resolved.insert(
        "azure_cloud".into(),
        match crate::remote::azure_cloud::declared() {
            Ok(cloud) => serde_json::json!({
                "resource_manager": cloud.resource_manager,
                "authority": cloud.authority,
                "identity_endpoint": cloud.identity_endpoint,
                "retail_prices": cloud.retail_prices,
            }),
            Err(cause) => serde_json::json!({ "refused": cause }),
        },
    );
    resolved.insert("stado_api_url".into(), Value::from(config::stado_api_url()));
    resolved.insert(
        "enrollment_url".into(),
        Value::from(config::enrollment_url()),
    );
    resolved.insert(
        "stado_release_version".into(),
        Value::from(config::stado_release_version()),
    );
    resolved.insert(
        "stado_release_platform".into(),
        Value::from(config::stado_release_platform()),
    );
    resolved.insert(
        "dashboard_trust_https_proxy".into(),
        Value::from(config::dashboard_trust_https_proxy()),
    );
    resolved.insert(
        "stado_deployment_id".into(),
        Value::from(config::stado_deployment_id()),
    );
}
