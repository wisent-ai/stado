//! The AWS reader: the EC2 and S3 reads that make up one account snapshot.
//!
//! [`collect_aws`] opens one SDK config and runs each read in turn, marking
//! coverage before it tries and routing a failure through `record_aws_error`
//! so a refusal degrades the account instead of losing it. The paged reads
//! live next door in [`instances`], [`volumes`] and [`snapshots`]; the
//! addresses, reservations, images and buckets are single calls and stay
//! here. `aws_tags` is the tag-list read all of them share.

mod instances;
mod snapshots;
mod volumes;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::autonomy::inventory::values::permission_error;
use crate::autonomy::model::{InventorySource, ResourceRecord, SourceState};
use crate::capabilities::ProviderId;

use instances::aws_instances;
use snapshots::aws_snapshots;
use volumes::aws_volumes;

pub(in crate::autonomy::inventory) async fn collect_aws(
    observed_at: DateTime<Utc>,
) -> InventorySource {
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

fn aws_tags(tags: &[aws_sdk_ec2::types::Tag]) -> BTreeMap<String, String> {
    tags.iter()
        .filter_map(|tag| Some((tag.key()?.to_string(), tag.value()?.to_string())))
        .collect()
}
