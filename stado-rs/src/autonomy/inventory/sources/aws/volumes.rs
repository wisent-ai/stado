//! The paged EBS volume read.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::autonomy::model::{ResourceKind, ResourceRecord};
use crate::capabilities::ProviderId;

use super::{aws_tags, console_name};

pub(super) async fn aws_volumes(
    client: &aws_sdk_ec2::Client,
    account: &str,
    region: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<ResourceRecord>, String> {
    let mut resources = Vec::new();
    let mut token = None;
    loop {
        let output = crate::wait::sdk(client.describe_volumes().set_next_token(token).send())
            .await
            .map_err(|error| {
                // SdkError's Display says only "service error"; the context
                // carries the AWS code and message beneath it.
                format!(
                    "EC2 DescribeVolumes in {region}: {}",
                    aws_sdk_ec2::error::DisplayErrorContext(&error)
                )
            })?;
        for volume in output.volumes() {
            let Some(native) = volume.volume_id() else {
                return Err(format!(
                    "a volume names no volume id (zone {:?}), so this account's volumes cannot \
                     be listed by id",
                    volume.availability_zone()
                ));
            };
            let labels = aws_tags(volume.tags());
            let name = console_name(&labels, native);
            let mut resource = ResourceRecord::new(
                ProviderId::Aws,
                account,
                ResourceKind::Volume,
                native,
                name,
                observed_at,
            );
            resource.region = Some(region.to_string());
            resource.zone = volume.availability_zone().map(str::to_string);
            resource.state = volume.state().map(|state| state.as_str().to_string());
            resource.phase = super::phase::volume(volume.state());
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
