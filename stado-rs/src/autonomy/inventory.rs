//! Direct provider-API inventory normalized into the autonomy resource model.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::capabilities::ProviderId;
use crate::cli::resources::model::canonical_json_bytes;
use crate::queue::{copy::Endpoint, JobStorage, StorageError};

use super::model::{
    InventorySnapshot, InventorySource, Ownership, ResourceGraph, ResourceRecord, SourceState,
    SCHEMA_VERSION,
};

pub async fn collect(store: &JobStorage) -> Result<InventorySnapshot, StorageError> {
    let observed_at = Utc::now();
    let configured: BTreeSet<ProviderId> = crate::config::wc_providers()
        .iter()
        .filter_map(|name| crate::capabilities::provider(name))
        .collect();
    let (gcp, aws, azure) = tokio::join!(
        async {
            if configured.contains(&ProviderId::Gcp) {
                Some(collect_gcp(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Aws) {
                Some(collect_aws(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Azure) {
                Some(collect_azure(observed_at).await)
            } else {
                None
            }
        },
    );
    let mut sources: Vec<InventorySource> = [gcp, aws, azure].into_iter().flatten().collect();
    sources.push(collect_local(store, observed_at).await?);
    let adoptions = super::storage::list_adoptions(store).await?;
    let adopted: BTreeMap<&str, _> = adoptions
        .iter()
        .map(|record| (record.resource_id.as_str(), record))
        .collect();
    for source in &mut sources {
        for resource in &mut source.resources {
            if let Some(record) = adopted.get(resource.resource_id.as_str()) {
                resource.ownership = Ownership::Adopted;
                resource.owner = Some(record.owner.clone());
                resource.policy_ref = Some(record.policy_ref.clone());
            }
        }
    }
    let mut resources: Vec<ResourceRecord> = sources
        .iter()
        .flat_map(|source| source.resources.iter().cloned())
        .collect();
    resolve_dependencies(&mut resources);
    resources.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    let graph = ResourceGraph::from_resources(&resources);
    let complete = sources
        .iter()
        .all(|source| source.state == SourceState::Complete);
    let mut snapshot = InventorySnapshot {
        schema_version: SCHEMA_VERSION,
        snapshot_id: String::new(),
        created_at: observed_at.to_rfc3339(),
        complete,
        sources,
        resources,
        graph,
    };
    reseal(&mut snapshot)?;
    Ok(snapshot)
}

pub fn reseal(snapshot: &mut InventorySnapshot) -> Result<(), StorageError> {
    snapshot.snapshot_id.clear();
    let digest = sha256_hex(&canonical_json_bytes(snapshot).map_err(|error| {
        StorageError::Other(format!("inventory canonicalization failed: {error}"))
    })?);
    snapshot.snapshot_id = digest;
    Ok(())
}

pub async fn collect_and_publish(store: &JobStorage) -> Result<InventorySnapshot, StorageError> {
    let snapshot = collect(store).await?;
    super::storage::publish_inventory(store, &snapshot).await?;
    Ok(snapshot)
}

async fn collect_local(
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

async fn collect_gcp(observed_at: DateTime<Utc>) -> InventorySource {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let options = crate::cli::blast_radius::gcp_inventory_options(&primary, backup.as_ref());
    let project = options.project.clone();
    let report = crate::providers::gcp::inventory::inspect(options).await;
    let mut resources = Vec::new();
    let mut coverage = BTreeSet::new();
    let mut missing_permissions = Vec::new();
    let mut errors = Vec::new();
    for probe in &report.probes {
        coverage.insert(probe.service.clone());
        if let Some(error) = &probe.error {
            errors.push(format!("{}: {error}", probe.name));
            if permission_error(error) {
                missing_permissions.push(probe.name.clone());
            }
        }
        let Some((kind, key)) = gcp_probe_shape(&probe.name) else {
            continue;
        };
        let Some(items) = probe.detail.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            resources.push(gcp_resource(&project, kind, item, observed_at, &probe.name));
        }
    }
    let state = source_state(&report.summary.state, &errors);
    InventorySource {
        provider: ProviderId::Gcp,
        account: project,
        state,
        observed_at: observed_at.to_rfc3339(),
        coverage,
        missing_permissions,
        upstream_error: (!errors.is_empty()).then(|| errors.join("; ")),
        resources,
    }
}

fn gcp_probe_shape(name: &str) -> Option<(&'static str, &'static str)> {
    match name {
        "compute_instances" => Some(("instance", "instances")),
        "compute_disks" => Some(("persistent_disk", "disks")),
        "managed_instance_groups" => Some(("managed_instance_group", "managed_instance_groups")),
        "compute_reservations" => Some(("reservation", "reservations")),
        "static_addresses" => Some(("public_ip", "addresses")),
        "service_accounts" => Some(("service_account", "service_accounts")),
        "cloud_builds" => Some(("build", "builds")),
        "cloud_scheduler" => Some(("schedule", "jobs")),
        "cloud_functions" => Some(("function", "functions")),
        "cloud_run_services" => Some(("service", "services")),
        _ => None,
    }
}

fn gcp_resource(
    project: &str,
    kind: &str,
    item: &Value,
    observed_at: DateTime<Utc>,
    probe: &str,
) -> ResourceRecord {
    let name = value_text(item, &["name", "id", "self_link", "selfLink"])
        .unwrap_or_else(|| "unknown".to_string());
    let native = value_text(item, &["self_link", "selfLink", "id"]).unwrap_or_else(|| name.clone());
    let mut resource = ResourceRecord::new(
        ProviderId::Gcp,
        project,
        kind,
        native,
        name.clone(),
        observed_at,
    );
    resource.zone = value_text(item, &["zone"]);
    resource.region = value_text(item, &["region"]).or_else(|| {
        resource
            .zone
            .as_deref()
            .and_then(region_from_zone)
            .map(str::to_string)
    });
    resource.state = value_text(item, &["status", "state", "terminal_state"])
        .unwrap_or_else(|| "unknown".to_string())
        .to_ascii_lowercase();
    resource.created_at = value_text(item, &["creation_timestamp", "create_time", "created_at"]);
    resource.labels = object_strings(item.get("labels"));
    if item
        .get("stado_managed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || name.starts_with("wisent-")
        || name.starts_with("stado-")
    {
        resource
            .labels
            .insert("managed-by".to_string(), "stado".to_string());
    }
    collect_resource_references(item, &mut resource.dependencies);
    resource.source_revision =
        value_text(item, &["fingerprint", "etag"]).or_else(|| canonical_revision(item));
    resource.evidence = json!({"probe": probe, "item": item});
    resource.apply_identity_labels();
    resource
}

async fn collect_aws(observed_at: DateTime<Utc>) -> InventorySource {
    let region = crate::config::aws_region().to_string();
    let account = std::env::var("AWS_ACCOUNT_ID").unwrap_or_else(|_| format!("region:{region}"));
    let mut source = InventorySource {
        provider: ProviderId::Aws,
        account: account.clone(),
        state: SourceState::Complete,
        observed_at: observed_at.to_rfc3339(),
        coverage: BTreeSet::new(),
        missing_permissions: Vec::new(),
        upstream_error: None,
        resources: Vec::new(),
    };
    let sdk = match crate::providers::aws::sdk_config(&region).await {
        Ok(sdk) => sdk,
        Err(error) => {
            source.state = SourceState::Blocked;
            source.upstream_error = Some(error.to_string());
            return source;
        }
    };
    let ec2 = aws_sdk_ec2::Client::new(&sdk);
    let s3 = aws_sdk_s3::Client::new(&sdk);
    let mut errors = Vec::new();

    source.coverage.insert("ec2.instances".to_string());
    match aws_instances(&ec2, &account, &region, observed_at).await {
        Ok(items) => source.resources.extend(items),
        Err(error) => record_aws_error("ec2.instances", error, &mut source, &mut errors),
    }
    source.coverage.insert("ec2.volumes".to_string());
    match aws_volumes(&ec2, &account, &region, observed_at).await {
        Ok(items) => source.resources.extend(items),
        Err(error) => record_aws_error("ec2.volumes", error, &mut source, &mut errors),
    }
    source.coverage.insert("ec2.snapshots".to_string());
    match aws_snapshots(&ec2, &account, &region, observed_at).await {
        Ok(items) => source.resources.extend(items),
        Err(error) => record_aws_error("ec2.snapshots", error, &mut source, &mut errors),
    }
    source.coverage.insert("ec2.addresses".to_string());
    match ec2.describe_addresses().send().await {
        Ok(output) => {
            for address in output.addresses() {
                let native = address
                    .allocation_id()
                    .or_else(|| address.public_ip())
                    .unwrap_or("unknown");
                let mut resource = ResourceRecord::new(
                    ProviderId::Aws,
                    &account,
                    "public_ip",
                    native,
                    native,
                    observed_at,
                );
                resource.region = Some(region.clone());
                resource.state = if address.association_id().is_some() {
                    "in_use".to_string()
                } else {
                    "available".to_string()
                };
                resource.labels = aws_tags(address.tags());
                if let Some(instance) = address.instance_id() {
                    resource.dependencies.insert(instance.to_string());
                }
                resource.evidence = json!({
                    "allocation_id": address.allocation_id(),
                    "association_id": address.association_id(),
                    "public_ip": address.public_ip(),
                    "private_ip": address.private_ip_address(),
                    "instance_id": address.instance_id(),
                    "domain": address.domain().map(|domain| domain.as_str()),
                });
                resource.apply_identity_labels();
                source.resources.push(resource);
            }
        }
        Err(error) => {
            record_aws_error("ec2.addresses", error.to_string(), &mut source, &mut errors)
        }
    }
    source.coverage.insert("ec2.reservations".to_string());
    match ec2.describe_reserved_instances().send().await {
        Ok(output) => {
            for reservation in output.reserved_instances() {
                let native = reservation.reserved_instances_id().unwrap_or("unknown");
                let mut resource = ResourceRecord::new(
                    ProviderId::Aws,
                    &account,
                    "reservation",
                    native,
                    native,
                    observed_at,
                );
                resource.region = Some(region.clone());
                resource.state = reservation
                    .state()
                    .map(|state| state.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                resource.evidence = json!({
                    "id": reservation.reserved_instances_id(),
                    "instance_type": reservation.instance_type().map(|kind| kind.as_str()),
                    "instance_count": reservation.instance_count(),
                    "offering_type": reservation.offering_type().map(|kind| kind.as_str()),
                    "availability_zone": reservation.availability_zone(),
                    "duration_seconds": reservation.duration(),
                    "fixed_price": reservation.fixed_price(),
                    "usage_price": reservation.usage_price(),
                });
                source.resources.push(resource);
            }
        }
        Err(error) => record_aws_error(
            "ec2.reservations",
            error.to_string(),
            &mut source,
            &mut errors,
        ),
    }
    source.coverage.insert("ec2.images".to_string());
    match ec2.describe_images().owners("self").send().await {
        Ok(output) => {
            for image in output.images() {
                let native = image.image_id().unwrap_or("unknown");
                let name = image.name().unwrap_or(native);
                let mut resource = ResourceRecord::new(
                    ProviderId::Aws,
                    &account,
                    "machine_image",
                    native,
                    name,
                    observed_at,
                );
                resource.region = Some(region.clone());
                resource.state = image
                    .state()
                    .map(|state| state.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                resource.created_at = image.creation_date().map(str::to_string);
                resource.labels = aws_tags(image.tags());
                resource.evidence = json!({
                    "image_id": image.image_id(),
                    "name": image.name(),
                    "architecture": image.architecture().map(|kind| kind.as_str()),
                    "root_device_type": image.root_device_type().map(|kind| kind.as_str()),
                    "root_device_name": image.root_device_name(),
                    "description": image.description(),
                });
                resource.apply_identity_labels();
                source.resources.push(resource);
            }
        }
        Err(error) => record_aws_error("ec2.images", error.to_string(), &mut source, &mut errors),
    }
    source.coverage.insert("s3.buckets".to_string());
    match s3.list_buckets().send().await {
        Ok(output) => {
            for bucket in output.buckets() {
                let name = bucket.name().unwrap_or("unknown");
                let mut resource = ResourceRecord::new(
                    ProviderId::Aws,
                    &account,
                    "object_bucket",
                    name,
                    name,
                    observed_at,
                );
                resource.state = "active".to_string();
                resource.created_at = bucket.creation_date().map(|date| date.to_string());
                resource.evidence = json!({"name": name});
                source.resources.push(resource);
            }
        }
        Err(error) => record_aws_error("s3.buckets", error.to_string(), &mut source, &mut errors),
    }
    if !errors.is_empty() {
        source.state = SourceState::Degraded;
        source.upstream_error = Some(errors.join("; "));
    }
    source
}

async fn aws_instances(
    client: &aws_sdk_ec2::Client,
    account: &str,
    region: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<ResourceRecord>, String> {
    let mut resources = Vec::new();
    let mut token = None;
    loop {
        let output = client
            .describe_instances()
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        for reservation in output.reservations() {
            for instance in reservation.instances() {
                let native = instance.instance_id().unwrap_or("unknown");
                let labels = aws_tags(instance.tags());
                let name = labels.get("Name").map(String::as_str).unwrap_or(native);
                let mut resource = ResourceRecord::new(
                    ProviderId::Aws,
                    account,
                    "instance",
                    native,
                    name,
                    observed_at,
                );
                resource.region = Some(region.to_string());
                resource.zone = instance
                    .placement()
                    .and_then(|placement| placement.availability_zone())
                    .map(str::to_string);
                resource.state = instance
                    .state()
                    .and_then(|state| state.name())
                    .map(|state| state.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                resource.created_at = instance.launch_time().map(|date| date.to_string());
                resource.labels = labels;
                for mapping in instance.block_device_mappings() {
                    if let Some(volume) = mapping.ebs().and_then(|ebs| ebs.volume_id()) {
                        resource.dependencies.insert(volume.to_string());
                    }
                }
                if let Some(image) = instance.image_id() {
                    resource.dependencies.insert(image.to_string());
                }
                resource.evidence = json!({
                    "instance_id": instance.instance_id(),
                    "instance_type": instance.instance_type().map(|kind| kind.as_str()),
                    "image_id": instance.image_id(),
                    "private_ip": instance.private_ip_address(),
                    "public_ip": instance.public_ip_address(),
                    "subnet_id": instance.subnet_id(),
                    "vpc_id": instance.vpc_id(),
                    "root_device_name": instance.root_device_name(),
                    "block_devices": instance.block_device_mappings().iter().map(|mapping| json!({
                        "device_name": mapping.device_name(),
                        "volume_id": mapping.ebs().and_then(|ebs| ebs.volume_id()),
                        "delete_on_termination": mapping.ebs().and_then(|ebs| ebs.delete_on_termination()),
                    })).collect::<Vec<Value>>(),
                });
                resource.source_revision = canonical_revision(&json!({
                    "state": &resource.state,
                    "labels": &resource.labels,
                    "evidence": &resource.evidence,
                }));
                resource.apply_identity_labels();
                resources.push(resource);
            }
        }
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    Ok(resources)
}

async fn aws_volumes(
    client: &aws_sdk_ec2::Client,
    account: &str,
    region: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<ResourceRecord>, String> {
    let mut resources = Vec::new();
    let mut token = None;
    loop {
        let output = client
            .describe_volumes()
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        for volume in output.volumes() {
            let native = volume.volume_id().unwrap_or("unknown");
            let labels = aws_tags(volume.tags());
            let name = labels.get("Name").map(String::as_str).unwrap_or(native);
            let mut resource = ResourceRecord::new(
                ProviderId::Aws,
                account,
                "volume",
                native,
                name,
                observed_at,
            );
            resource.region = Some(region.to_string());
            resource.zone = volume.availability_zone().map(str::to_string);
            resource.state = volume
                .state()
                .map(|state| state.as_str())
                .unwrap_or("unknown")
                .to_string();
            resource.created_at = volume.create_time().map(|date| date.to_string());
            resource.labels = labels;
            for attachment in volume.attachments() {
                if let Some(instance) = attachment.instance_id() {
                    resource.dependencies.insert(instance.to_string());
                }
            }
            resource.evidence = json!({
                "volume_id": volume.volume_id(),
                "size_gb": volume.size(),
                "volume_type": volume.volume_type().map(|kind| kind.as_str()),
                "encrypted": volume.encrypted(),
                "iops": volume.iops(),
                "throughput": volume.throughput(),
            });
            resource.apply_identity_labels();
            resources.push(resource);
        }
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    Ok(resources)
}

async fn aws_snapshots(
    client: &aws_sdk_ec2::Client,
    account: &str,
    region: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<ResourceRecord>, String> {
    let mut resources = Vec::new();
    let mut token = None;
    loop {
        let output = client
            .describe_snapshots()
            .owner_ids("self")
            .set_next_token(token)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        for snapshot in output.snapshots() {
            let native = snapshot.snapshot_id().unwrap_or("unknown");
            let labels = aws_tags(snapshot.tags());
            let name = labels.get("Name").map(String::as_str).unwrap_or(native);
            let mut resource = ResourceRecord::new(
                ProviderId::Aws,
                account,
                "snapshot",
                native,
                name,
                observed_at,
            );
            resource.region = Some(region.to_string());
            resource.state = snapshot
                .state()
                .map(|state| state.as_str())
                .unwrap_or("unknown")
                .to_string();
            resource.created_at = snapshot.start_time().map(|date| date.to_string());
            resource.labels = labels;
            if let Some(volume) = snapshot.volume_id() {
                resource.dependencies.insert(volume.to_string());
            }
            resource.evidence = json!({
                "snapshot_id": snapshot.snapshot_id(),
                "volume_id": snapshot.volume_id(),
                "volume_size_gb": snapshot.volume_size(),
                "encrypted": snapshot.encrypted(),
                "progress": snapshot.progress(),
                "description": snapshot.description(),
            });
            resource.apply_identity_labels();
            resources.push(resource);
        }
        token = output.next_token().map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    Ok(resources)
}

fn record_aws_error(
    operation: &str,
    error: String,
    source: &mut InventorySource,
    errors: &mut Vec<String>,
) {
    if permission_error(&error) {
        source.missing_permissions.push(operation.to_string());
    }
    errors.push(format!("{operation}: {error}"));
}

async fn collect_azure(observed_at: DateTime<Utc>) -> InventorySource {
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

fn resolve_dependencies(resources: &mut [ResourceRecord]) {
    let mut aliases = BTreeMap::new();
    for resource in resources.iter() {
        aliases.insert(
            resource.native_reference.clone(),
            resource.resource_id.clone(),
        );
        aliases.insert(resource.name.clone(), resource.resource_id.clone());
        if let Some(tail) = resource.native_reference.rsplit('/').next() {
            aliases
                .entry(tail.to_string())
                .or_insert_with(|| resource.resource_id.clone());
        }
    }
    for resource in resources {
        resource.dependencies = resource
            .dependencies
            .iter()
            .filter_map(|reference| aliases.get(reference).cloned())
            .filter(|dependency| dependency != &resource.resource_id)
            .collect();
    }
}

fn collect_resource_references(value: &Value, output: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            if text.starts_with("/subscriptions/")
                || text.starts_with("https://www.googleapis.com/compute/")
                || text.starts_with("projects/")
                || text.starts_with("i-")
                || text.starts_with("vol-")
                || text.starts_with("ami-")
            {
                output.insert(text.to_string());
                if let Some(tail) = text.rsplit('/').next() {
                    output.insert(tail.to_string());
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_resource_references(item, output);
            }
        }
        Value::Object(object) => {
            for nested in object.values() {
                collect_resource_references(nested, output);
            }
        }
        _ => {}
    }
}

fn aws_tags(tags: &[aws_sdk_ec2::types::Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|tag| Some((tag.key()?.to_string(), tag.value()?.to_string())))
        .collect()
}

fn object_strings(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string())))
        .collect()
}

fn value_text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn source_state(summary: &str, errors: &[String]) -> SourceState {
    if summary == "ok" && errors.is_empty() {
        SourceState::Complete
    } else if summary == "blocked" || summary == "failed" {
        SourceState::Blocked
    } else {
        SourceState::Degraded
    }
}

fn permission_error(error: &str) -> bool {
    let lowered = error.to_ascii_lowercase();
    lowered.contains("permission")
        || lowered.contains("forbidden")
        || lowered.contains("unauthorized")
        || lowered.contains("accessdenied")
}

fn region_from_zone(zone: &str) -> Option<&str> {
    zone.rsplit_once('-').map(|(region, _)| region)
}

fn canonical_revision(value: &Value) -> Option<String> {
    canonical_json_bytes(value)
        .ok()
        .map(|bytes| sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
