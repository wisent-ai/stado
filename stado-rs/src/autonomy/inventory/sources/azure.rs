//! The Azure reader: the Resource Graph query, paged to the end.
//!
//! [`collect_azure`] posts one projection over every subscription resource
//! and follows the skip token until it runs out, turning each row into a
//! record with [`azure_resource`]; [`azure_resource_type`] maps the ARM type
//! string onto the resource kinds the model speaks.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::inventory::values::{
    canonical_revision, collect_resource_references, object_strings, permission_error, value_text,
};
use crate::autonomy::model::{InventorySource, ResourceRecord, SourceState};
use crate::capabilities::ProviderId;

pub(in crate::autonomy::inventory) async fn collect_azure(
    observed_at: DateTime<Utc>,
) -> InventorySource {
    let subscription = crate::config::azure_subscription_id().to_string();
    let mut source = InventorySource {
        provider: ProviderId::Azure,
        account: subscription.clone(),
        state: SourceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        coverage: ["azure.resource_graph".to_string()].into_iter().collect(),
        missing_permissions: Vec::new(),
        upstream_error: None,
        resources: Vec::new(),
    };
    if subscription.trim().is_empty() {
        source.state = SourceState::Blocked;
        source.upstream_error = Some("AZURE_SUBSCRIPTION_ID is required".to_string());
        return source;
    }
    let client = crate::providers::azure::ArmClient::new(&subscription);
    let mut skip_token: Option<String> = None;
    loop {
        let mut options = serde_json::Map::new();
        options.insert(
            "resultFormat".to_string(),
            Value::String("objectArray".to_string()),
        );
        if let Some(token) = skip_token.as_ref() {
            options.insert("$skipToken".to_string(), Value::String(token.clone()));
        }
        let body = json!({
            "subscriptions": [&subscription],
            "query": "Resources | project id, name, type, location, resourceGroup, subscriptionId, tags, properties, kind, managedBy, sku, identity, etag",
            "options": options,
        });
        let response = match client
            .post_json(
                "/providers/Microsoft.ResourceGraph/resources?api-version=2022-10-01",
                &body,
                "query Resource Graph",
            )
            .await
        {
            Ok(response) => response,
            Err(error) => {
                let detail = error.to_string();
                source.state = SourceState::Blocked;
                if permission_error(&detail) {
                    source
                        .missing_permissions
                        .push("Microsoft.ResourceGraph/resources/read".to_string());
                }
                source.upstream_error = Some(detail);
                return source;
            }
        };
        if let Some(items) = response.get("data").and_then(Value::as_array) {
            for item in items {
                source
                    .resources
                    .push(azure_resource(&subscription, item, observed_at));
            }
        }
        skip_token = response
            .get("$skipToken")
            .or_else(|| response.get("skipToken"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if skip_token.is_none() {
            break;
        }
    }
    source
}

fn azure_resource(subscription: &str, item: &Value, observed_at: DateTime<Utc>) -> ResourceRecord {
    let native = value_text(item, &["id"]).unwrap_or_else(|| "unknown".to_string());
    let name = value_text(item, &["name"]).unwrap_or_else(|| native.clone());
    let native_type = value_text(item, &["type"]).unwrap_or_else(|| "resource".to_string());
    let kind = azure_resource_type(&native_type);
    let mut resource = ResourceRecord::new(
        ProviderId::Azure,
        subscription,
        kind,
        &native,
        name.clone(),
        observed_at,
    );
    resource.region = value_text(item, &["location"]);
    resource.state = item
        .pointer("/properties/extended/instanceView/powerState/code")
        .and_then(Value::as_str)
        .and_then(|state| state.rsplit('/').next())
        .or_else(|| {
            item.pointer("/properties/provisioningState")
                .and_then(Value::as_str)
        })
        .or_else(|| item.pointer("/properties/status").and_then(Value::as_str))
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    resource.created_at = item
        .pointer("/properties/timeCreated")
        .or_else(|| item.pointer("/properties/creationTime"))
        .and_then(Value::as_str)
        .map(str::to_string);
    resource.labels = object_strings(item.get("tags"));
    if item
        .get("managedBy")
        .and_then(Value::as_str)
        .is_some_and(|managed| managed.contains("stado") || managed.contains("wisent"))
        || name.starts_with("stado-")
        || name.starts_with("wisent-")
    {
        resource
            .labels
            .insert("managed-by".to_string(), "stado".to_string());
    }
    collect_resource_references(item, &mut resource.dependencies);
    resource.source_revision = item
        .get("etag")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| canonical_revision(item));
    resource.evidence = item.clone();
    resource.apply_identity_labels();
    resource
}

fn azure_resource_type(native: &str) -> &str {
    let lowered = native.to_ascii_lowercase();
    if lowered.ends_with("/virtualmachines") {
        "instance"
    } else if lowered.ends_with("/disks") {
        "managed_disk"
    } else if lowered.ends_with("/snapshots") {
        "snapshot"
    } else if lowered.ends_with("/publicipaddresses") {
        "public_ip"
    } else if lowered.ends_with("/virtualmachinescalesets") {
        "scale_set"
    } else if lowered.ends_with("/storageaccounts") {
        "object_storage"
    } else if lowered.ends_with("/registries") {
        "container_registry"
    } else if lowered.ends_with("/servers") || lowered.ends_with("/databases") {
        "database"
    } else if lowered.ends_with("/loadbalancers") {
        "load_balancer"
    } else if lowered.ends_with("/networkinterfaces") {
        "network_interface"
    } else {
        "resource"
    }
}
