use reqwest::Client;
use serde_json::json;

const ZONES: &[&str] = &["us-central1-b", "us-central1-a", "us-central1-c", "us-central1-f"];
const API: &str = "https://compute.googleapis.com/compute/v1";

pub async fn get_access_token() -> Result<String, String> {
    let client = Client::new();
    let resp = client.get("http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token")
        .header("Metadata-Flavor", "Google")
        .send().await.map_err(|e| e.to_string())?;
    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    body.get("access_token").and_then(|v| v.as_str()).map(|s| s.to_string())
        .ok_or_else(|| "no token".into())
}

pub async fn create_vm(
    project: &str, machine_type: &str, accel_type: &str,
    startup_script: &str, name: &str,
) -> Result<(String, String), String> {
    let token = get_access_token().await?;
    let client = Client::new();

    for zone in ZONES {
        let url = format!("{API}/projects/{project}/zones/{zone}/instances");

        let mut guest_accelerators = vec![];
        if !accel_type.is_empty() {
            guest_accelerators.push(json!({
                "acceleratorType": format!("zones/{zone}/acceleratorTypes/{accel_type}"),
                "acceleratorCount": 1
            }));
        }

        let body = json!({
            "name": name,
            "machineType": format!("zones/{zone}/machineTypes/{machine_type}"),
            "disks": [{
                "boot": true,
                "autoDelete": true,
                "initializeParams": {
                    "diskSizeGb": "200",
                    "sourceImage": "projects/deeplearning-platform-release/global/images/pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408"
                }
            }],
            "networkInterfaces": [{"accessConfigs": [{"name": "External NAT", "type": "ONE_TO_ONE_NAT"}]}],
            "scheduling": {"preemptible": false, "onHostMaintenance": "TERMINATE"},
            "guestAccelerators": guest_accelerators,
            "metadata": {"items": [{"key": "startup-script", "value": startup_script}]}
        });

        let resp = client.post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send().await.map_err(|e| e.to_string())?;

        if resp.status().is_success() {
            tracing::info!("Created VM {name} in {zone}");
            return Ok((name.to_string(), zone.to_string()));
        }

        let err = resp.text().await.unwrap_or_default();
        if err.contains("ZONE_RESOURCE_POOL_EXHAUSTED") || err.contains("QUOTA") {
            tracing::warn!("No capacity in {zone}, trying next...");
            continue;
        }
        tracing::error!("GCE create failed in {zone}: {err}");
    }
    Err("Failed to create VM in any zone".into())
}

pub async fn delete_vm(project: &str, name: &str, zone: &str) -> Result<(), String> {
    let token = get_access_token().await?;
    let url = format!("{API}/projects/{project}/zones/{zone}/instances/{name}");
    let client = Client::new();
    client.delete(&url).bearer_auth(&token).send().await.map_err(|e| e.to_string())?;
    tracing::info!("Deleted VM {name} in {zone}");
    Ok(())
}
