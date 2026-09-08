//! The paged EBS snapshot read, owned snapshots only.

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::autonomy::model::ResourceRecord;
use crate::capabilities::ProviderId;

use super::aws_tags;

pub(super) async fn aws_snapshots(
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
