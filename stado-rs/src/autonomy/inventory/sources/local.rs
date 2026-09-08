//! The local reader: the capacity publications the agents write to the queue.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::autonomy::inventory::values::object_strings;
use crate::autonomy::model::{InventorySource, ResourceRecord, SourceState};
use crate::capabilities::ProviderId;
use crate::queue::{JobStorage, StorageError};

pub(in crate::autonomy::inventory) async fn collect_local(
    store: &JobStorage,
    observed_at: DateTime<Utc>,
) -> Result<InventorySource, StorageError> {
    let capacities = crate::queue::capacity::read_consumer_capacity(store).await?;
    let mut resources = Vec::new();
    for (consumer_id, payload) in capacities {
        if payload.get("kind").and_then(Value::as_str) != Some("local") {
            continue;
        }
        let mut resource = ResourceRecord::new(
            ProviderId::Local,
            "local",
            "local_host",
            &consumer_id,
            &consumer_id,
            observed_at,
        );
        resource.state = "online".to_string();
        resource.created_at = payload
            .get("published_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        resource.labels = object_strings(payload.get("labels"));
        resource
            .labels
            .insert("managed-by".to_string(), "stado".to_string());
        resource.evidence = payload;
        resource.apply_identity_labels();
        resources.push(resource);
    }
    Ok(InventorySource {
        provider: ProviderId::Local,
        account: "local".to_string(),
        state: SourceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        coverage: BTreeSet::from(["capacity.publications".to_string()]),
        missing_permissions: Vec::new(),
        upstream_error: None,
        resources,
    })
}
