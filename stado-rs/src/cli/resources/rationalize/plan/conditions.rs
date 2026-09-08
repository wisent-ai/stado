//! What an action asserts before and after it runs, and how a resource is
//! addressed. The stable preconditions pin the exact observed object, so a
//! resource that changed between audit and apply fails its own plan.

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::resources::model::{
    Action, ActionKind, Authorization, Condition, ProviderKind, ResourceLocator, Reversibility,
};
use crate::cli::resources::planner;
use crate::cli::resources::rationalize::Finding;
use crate::config;

pub(super) fn recovery_snapshot_name(disk_name: &str) -> String {
    let prefix = "stado-recovery-";
    let nonce: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take((u64::BITS / u8::BITS) as usize)
        .collect();
    let suffix = format!("-{}-{nonce}", Utc::now().format("%Y%m%d%H%M%S"));
    let maximum = (u64::BITS as usize).saturating_sub(true as usize);
    let available = maximum.saturating_sub(prefix.len() + suffix.len());
    let disk: String = disk_name.chars().take(available).collect();
    let disk = disk.trim_end_matches('-');
    format!(
        "{prefix}{}{suffix}",
        if disk.is_empty() { "disk" } else { disk }
    )
}

pub(super) fn disk_restore_postconditions(
    finding: &Finding,
    snapshot_name: &str,
) -> Vec<Condition> {
    let mut conditions = vec![
        planner::condition("exists", json!(true)),
        planner::condition("source_snapshot", json!(snapshot_name)),
    ];
    for field in [
        "type_url",
        "type",
        "size_gb",
        "labels",
        "description",
        "replica_zones",
        "resource_policies",
        "physical_block_size_bytes",
    ] {
        if let Some(value) = finding.evidence.get(field).filter(|value| !value.is_null()) {
            conditions.push(planner::condition(field, value.clone()));
        }
    }
    conditions
}

pub(super) fn stable_preconditions(
    finding: &Finding,
    mut conditions: Vec<Condition>,
) -> Vec<Condition> {
    if let Some(resource_id) = finding.evidence.get("id").filter(|value| !value.is_null()) {
        conditions.push(planner::condition("resource_id", resource_id.clone()));
    }
    if let Some(created) = finding
        .evidence
        .get("creation_timestamp")
        .filter(|value| !value.is_null())
    {
        conditions.push(planner::condition("creation_timestamp", created.clone()));
    }
    if let Some(fingerprint) = finding
        .evidence
        .get("fingerprint")
        .filter(|value| !value.is_null())
    {
        conditions.push(planner::condition("fingerprint", fingerprint.clone()));
    }
    conditions
}

pub(super) fn irreversible_action(
    id: String,
    finding: &Finding,
    kind: ActionKind,
    resource: ResourceLocator,
    parameters: Value,
    preconditions: Vec<Condition>,
) -> Action {
    Action {
        id,
        finding_id: Some(finding.id.clone()),
        kind,
        authorization: Authorization::Explicit,
        reversibility: Reversibility::Irreversible,
        resource,
        parameters,
        preconditions,
        postconditions: vec![planner::condition("exists", json!(false))],
        rollback: None,
        depends_on: Vec::new(),
    }
}

pub(super) fn locator(finding: &Finding) -> ResourceLocator {
    let provider = crate::capabilities::provider(&finding.provider)
        .unwrap_or(crate::capabilities::ProviderId::Stado);
    let (name, location) = finding
        .resource
        .rsplit_once('@')
        .map_or((finding.resource.as_str(), None), |(name, location)| {
            (name, Some(location.to_string()))
        });
    ResourceLocator {
        provider,
        resource_type: finding.resource_type.to_string(),
        project: (provider == ProviderKind::Gcp && !config::project().is_empty())
            .then(|| config::project().to_string()),
        location,
        name: name.to_string(),
        reference: finding.resource.clone(),
    }
}
