//! Pure request-shape helpers: zone-to-region folding and the GCE instance
//! insert body. No I/O, so they stay directly unit-testable.

use serde_json::{json, Value};

use crate::config;
use crate::providers::gcp::client::CLOUD_PLATFORM_SCOPE;

/// Python `"-".join(zone.split("-")[:2])`: "us-central1-b" -> "us-central1".
pub fn region_of_zone(zone: &str) -> String {
    zone.split('-').take(2).collect::<Vec<_>>().join("-")
}

/// Build the REST insert body (Python's `compute_v1.Instance(...)`).
/// Split out pure for tests.
#[allow(clippy::too_many_arguments)]
pub fn instance_body(
    name: &str,
    zone: &str,
    machine_type: &str,
    accel_type: &str,
    boot_disk_gb: i64,
    image: &str,
    image_project: &str,
    startup_script: &str,
    preemptible: bool,
) -> Value {
    let scheduling = if preemptible {
        // Use Spot (the modern provisioning model). The legacy
        // `preemptible` flag is a separate Bool that GCP keeps for
        // back-compat; setting both is redundant but explicit.
        // instanceTerminationAction="DELETE" so a preempted VM is fully
        // removed (disk + instance), not just STOPped. With STOP, every
        // preemption left a zombie TERMINATED instance holding 200GB of
        // regional disk quota — empirically we accumulated 546 of them in
        // 4 days, eating ~109TB and bottlenecking dispatch with
        // DISKS_TOTAL_GB QUOTA_EXCEEDED.
        json!({
            "preemptible": true,
            "provisioningModel": "SPOT",
            "onHostMaintenance": "TERMINATE",
            "instanceTerminationAction": "DELETE",
        })
    } else {
        json!({ "preemptible": false, "onHostMaintenance": "TERMINATE" })
    };
    let guest_accelerators = if accel_type.is_empty() {
        json!([])
    } else {
        json!([{
            "acceleratorType": format!("zones/{zone}/acceleratorTypes/{accel_type}"),
            "acceleratorCount":
                1,
        }])
    };
    json!({
        "name": name,
        "machineType": format!("zones/{zone}/machineTypes/{machine_type}"),
        "disks": [{
            "autoDelete": true,
            "boot": true,
            "initializeParams": {
                "diskSizeGb": boot_disk_gb,
                "sourceImage": format!("projects/{image_project}/global/images/{image}"),
            },
        }],
        "networkInterfaces": [{ "accessConfigs": [{ "name": "External NAT" }] }],
        "metadata": { "items": [{ "key": "startup-script", "value": startup_script }] },
        "scheduling": scheduling,
        "guestAccelerators": guest_accelerators,
        // Attach wisent-compute-sa so the instance can write status +
        // heartbeat to GCS, pull HF models with the in-startup token, and
        // fetch from gcloud APIs. Without an SA attached, the metadata
        // service returns 404 for default tokens and the whole startup
        // script crashes before extraction begins.
        "serviceAccounts": [{
            "email": format!("wisent-compute-sa@{}.iam.gserviceaccount.com", config::project()),
            "scopes": [CLOUD_PLATFORM_SCOPE],
        }],
    })
}
