//! Instance-scoped GCE reads: single-instance status and the paginated
//! aggregated list. Both are consumed by the provider component, so they
//! carry the narrowest visibility that reaches it.

use serde_json::Value;

use super::{GceClient, GceError};

impl GceClient {
    /// `GET .../zones/{zone}/instances/{name}` status, or None when the
    /// instance does not exist.
    pub(in crate::providers::gcp) async fn instance_status(
        &self,
        zone: &str,
        name: &str,
    ) -> Result<Option<String>, GceError> {
        let path = format!("/projects/{}/zones/{zone}/instances/{name}", self.project());
        let Some(instance) = self
            .get_allow_404(&path, &format!("get instance {name}@{zone}"))
            .await?
        else {
            return Ok(None);
        };
        Ok(instance
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// `GET .../aggregated/instances?filter=...`, flattened to
    /// `(zone, instance-json)` pairs across all pages.
    pub(in crate::providers::gcp) async fn aggregated_instances(
        &self,
        filter: &str,
    ) -> Result<Vec<(String, Value)>, GceError> {
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut path = format!(
                "/projects/{}/aggregated/instances?filter={}",
                self.project(),
                crate::queue::gcs::percent_encode(filter)
            );
            if let Some(token) = &page_token {
                path.push_str(&format!(
                    "&pageToken={}",
                    crate::queue::gcs::percent_encode(token)
                ));
            }
            let page = self.get(&path, "aggregatedList instances").await?;
            if let Some(items) = page.get("items").and_then(Value::as_object) {
                for (scope, scoped) in items {
                    let zone = scope.rsplit('/').next().unwrap_or("").to_string();
                    // Zones with no matching instances carry a `warning`
                    // entry instead of an `instances` list — skip those.
                    if let Some(instances) = scoped.get("instances").and_then(Value::as_array) {
                        for instance in instances {
                            out.push((zone.clone(), instance.clone()));
                        }
                    }
                }
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
    }
}
