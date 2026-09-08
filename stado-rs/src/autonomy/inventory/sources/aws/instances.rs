//! The paged EC2 instance read.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::autonomy::inventory::values::canonical_revision;
use crate::autonomy::model::ResourceRecord;
use crate::capabilities::ProviderId;

use super::aws_tags;

pub(super) async fn aws_instances(
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
