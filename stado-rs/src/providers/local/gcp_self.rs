//! GCE self-awareness helpers for the agent.
//!
//! Port of `stado/providers/local/gcp_self.py`.
//!
//! Lets the agent detect that it is running inside a GCE VM and
//! self-terminate it through the Compute REST API with managed identity.
//! Only used by the agent's idle-shutdown branch.

use std::time::Duration;

/// Python `_METADATA_BASE`.
pub const METADATA_BASE: &str = "http://metadata.google.internal/computeMetadata/v1";

const METADATA_TIMEOUT: Duration = Duration::from_secs(2);

/// [`METADATA_TIMEOUT`], for the sibling cloud probe: the Azure IMDS
/// probe in [`super::azure_self`] fails closed for the same reason (off
/// that cloud the link-local endpoint black-holes packets instead of
/// refusing them) and must not drift to a different ceiling.
pub(crate) const fn metadata_timeout() -> Duration {
    METADATA_TIMEOUT
}

/// Python `_fetch_metadata(path, timeout=2.0)`.
pub async fn fetch_metadata(path: &str) -> Result<String, reqwest::Error> {
    fetch_metadata_at(METADATA_BASE, path, METADATA_TIMEOUT).await
}

/// [`fetch_metadata`] against an explicit base URL (tests use a loopback
/// playback server). The `Metadata-Flavor: Google` header is required by
/// the real metadata server.
pub async fn fetch_metadata_at(
    base: &str,
    path: &str,
    timeout: Duration,
) -> Result<String, reqwest::Error> {
    let text = reqwest::Client::new()
        .get(format!("{base}/{path}"))
        .header("Metadata-Flavor", "Google")
        .timeout(timeout)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(text.trim().to_string())
}

/// True iff this process is running on a GCE VM (metadata service
/// responds). Python `on_gcp`.
pub async fn on_gcp() -> bool {
    fetch_metadata("instance/id").await.is_ok()
}

/// Pure: `zone.rsplit("/", 1)[-1]` — "projects/123/zones/us-central1-a" ->
/// "us-central1-a".
pub fn zone_from_path(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).to_string()
}

/// Return (instance_name, zone) from the GCE metadata service.
/// Python `self_metadata`.
pub async fn self_metadata() -> Result<(String, String), reqwest::Error> {
    let name = fetch_metadata("instance/name").await?;
    let zone = zone_from_path(&fetch_metadata("instance/zone").await?);
    Ok((name, zone))
}

/// If on GCE, delete this VM through the Compute REST API. Best-effort;
/// failure is non-fatal. No-op outside GCE.
pub async fn self_terminate(log_fn: &mut dyn FnMut(&str)) {
    if !on_gcp().await {
        return;
    }
    let result: Result<(), String> = async {
        let (name, zone) = self_metadata().await.map_err(|err| err.to_string())?;
        let project = fetch_metadata("project/project-id")
            .await
            .map_err(|err| err.to_string())?;
        log_fn(&format!(
            "GCE self-terminate: instances delete {name} in {zone}"
        ));
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|err| err.to_string())?;
        let token = auth
            .token(&["https://www.googleapis.com/auth/cloud-platform"])
            .await
            .map_err(|err| err.to_string())?;
        let response = reqwest::Client::new()
            .delete(format!(
                "https://compute.googleapis.com/compute/v1/projects/{project}/zones/{zone}/instances/{name}"
            ))
            .bearer_auth(token.as_str())
            .send()
            .await
            .map_err(|err| err.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "Compute instances.delete returned HTTP {}",
                response.status()
            ))
        }
    }
    .await;
    if let Err(err) = result {
        log_fn(&format!("GCE self-terminate failed: {err}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn zone_path_parsing() {
        assert_eq!(
            zone_from_path("projects/123/zones/us-central1-a"),
            "us-central1-a"
        );
        assert_eq!(zone_from_path("us-east4-b"), "us-east4-b");
    }

    #[tokio::test]
    async fn fetch_metadata_sends_flavor_header_and_trims() {
        let server =
            testutil::mock_http(vec![testutil::http_response(200, "OK", "123456789\n")]).await;
        let value = fetch_metadata_at(&server.base_url, "instance/id", Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(value, "123456789");
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains("GET /instance/id HTTP/1.1"),
            "{}",
            requests[0]
        );
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("metadata-flavor: google"),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn fetch_metadata_errors_when_unreachable() {
        // A playback server with zero responses closes its listener
        // immediately, so the connection is refused — the off-GCE path.
        let server = testutil::mock_http(vec![]).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let result =
            fetch_metadata_at(&server.base_url, "instance/id", Duration::from_millis(500)).await;
        assert!(result.is_err());
        server.stop();
    }
}
