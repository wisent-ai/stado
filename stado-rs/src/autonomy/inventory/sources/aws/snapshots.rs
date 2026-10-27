//! The paged EBS snapshot read, owned snapshots only.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::autonomy::model::{ResourceKind, ResourceRecord};
use crate::capabilities::ProviderId;

use super::{aws_tags, console_name};

pub(super) async fn aws_snapshots(
    client: &aws_sdk_ec2::Client,
    account: &str,
    region: &str,
    observed_at: DateTime<Utc>,
) -> Result<Vec<ResourceRecord>, String> {
    let mut resources = Vec::new();
    let mut token = None;
    loop {
        let output = crate::wait::sdk(
            client
                .describe_snapshots()
                .owner_ids("self")
                .set_next_token(token)
                .send(),
        )
        .await
        .map_err(|error| {
            format!(
                "EC2 DescribeSnapshots in {region}: {}",
                aws_sdk_ec2::error::DisplayErrorContext(&error)
            )
        })?;
        for snapshot in output.snapshots() {
            let Some(native) = snapshot.snapshot_id() else {
                return Err(format!(
                    "a snapshot names no snapshot id (volume {:?}), so this account's snapshots \
                     cannot be listed by id",
                    snapshot.volume_id()
                ));
            };
            let labels = aws_tags(snapshot.tags());
            let name = console_name(&labels, native);
            let mut resource = ResourceRecord::new(
                ProviderId::Aws,
                account,
                ResourceKind::Snapshot,
                native,
                name,
                observed_at,
            );
            resource.region = Some(region.to_string());
            resource.state = snapshot.state().map(|state| state.as_str().to_string());
            resource.phase = super::phase::snapshot(snapshot.state());
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
