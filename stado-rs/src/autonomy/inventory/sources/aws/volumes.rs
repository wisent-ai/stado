//! The paged EBS volume read.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::autonomy::model::ResourceRecord;
use crate::capabilities::ProviderId;

use super::aws_tags;

pub(super) async fn aws_volumes(
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
