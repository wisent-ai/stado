//! GCP arm: the accel -> gpu_family table, the QuotaPreference resource
//! id/body, and the CreateQuotaPreference submission (with the
//! ALREADY_EXISTS -> UpdateQuotaPreference conversion).

use serde_json::{json, Value};

use super::error::QuotaRequestError;
use super::quota_skus::{CatalogError, CloudQuotasClient};

/// Cloud Quotas API dimensions[gpu_family] values for each accel we
/// dispatch. Keep in sync with GCP_METRIC_TO_ACCEL in scheduler.quota.
pub const GCP_ACCEL_TO_GPU_FAMILY: [(&str, &str); 4] = [
    ("nvidia-tesla-t4", "NVIDIA_T4"),
    ("nvidia-l4", "NVIDIA_L4"),
    ("nvidia-tesla-a100", "NVIDIA_A100"),
    ("nvidia-a100-80gb", "NVIDIA_A100_80GB"),
];

/// The unified GPU quota id (dimensioned by region + gpu_family).
pub const GCP_GPU_FAMILY_QUOTA_ID: &str = "GPUS-PER-GPU-FAMILY-per-project-region";

/// Python's quotaPreferenceId:
/// `f"compute-gpus-{region}-{gpu_family}".lower().replace("_", "-")`.
pub fn gcp_preference_id(region: &str, gpu_family: &str) -> String {
    format!("compute-gpus-{region}-{gpu_family}")
        .to_lowercase()
        .replace('_', "-")
}

/// The QuotaPreference resource body (REST field names; int64 values are
/// string-encoded per the Google JSON convention).
pub fn gcp_preference_body(
    region: &str,
    gpu_family: &str,
    new_limit: i64,
    justification: &str,
    contact_email: &str,
) -> Value {
    json!({
        "service": "compute.googleapis.com",
        "quotaId": GCP_GPU_FAMILY_QUOTA_ID,
        "quotaConfig": {"preferredValue": new_limit.to_string()},
        "dimensions": {"region": region, "gpu_family": gpu_family},
        "justification": justification,
        "contactEmail": contact_email,
    })
}

/// Submit a Cloud Quotas QuotaPreference for a (region, gpu_family).
/// Python `_gcp_request_for_family`.
///
/// This is the family-based primitive — no accel translation, no
/// hardcoded table. Callers iterating live cloudquotas data should use
/// this directly. Returns {"name": <resource>, "created": bool}.
/// ALREADY_EXISTS converts to UpdateQuotaPreference so re-running bumps a
/// prior pending request's preferred_value.
pub async fn gcp_request_for_family(
    client: &CloudQuotasClient,
    region: &str,
    gpu_family: &str,
    new_limit: i64,
    justification: &str,
    contact_email: &str,
) -> Result<Value, CatalogError> {
    let pref_id = gcp_preference_id(region, gpu_family);
    let body = gcp_preference_body(region, gpu_family, new_limit, justification, contact_email);
    match client.create_quota_preference(&pref_id, &body).await {
        Ok(resp) => Ok(json!({
            "name": resp.get("name").and_then(Value::as_str).unwrap_or(""),
            "created": true,
        })),
        Err(err) => {
            let msg = err.to_string();
            if !msg.contains("ALREADY_EXISTS") && !msg.to_lowercase().contains("already exists") {
                return Err(err);
            }
            let mut update = body;
            update["name"] = json!(format!(
                "projects/{}/locations/global/quotaPreferences/{pref_id}",
                client.project()
            ));
            let resp = client.update_quota_preference(&pref_id, &update).await?;
            Ok(json!({
                "name": resp.get("name").and_then(Value::as_str).unwrap_or(""),
                "created": false,
            }))
        }
    }
}

/// Accel-label entrypoint (used by `stado quota request <accel>`).
/// Python `_gcp_request_increase`.
///
/// Thin wrapper: translates the wisent-compute accel label to its Cloud
/// Quotas gpu_family via the small GCP_ACCEL_TO_GPU_FAMILY map, then
/// defers to gcp_request_for_family. Bulk submission paths that already
/// have the gpu_family in hand (e.g. from gcp_catalog) should skip this
/// and call the family-based primitive directly.
pub async fn gcp_request_increase(
    client: &CloudQuotasClient,
    region: &str,
    accel: &str,
    new_limit: i64,
    justification: &str,
    contact_email: &str,
) -> Result<Value, QuotaRequestError> {
    let Some((_, family)) = GCP_ACCEL_TO_GPU_FAMILY.iter().find(|(a, _)| *a == accel) else {
        let mut known: Vec<&str> = GCP_ACCEL_TO_GPU_FAMILY.iter().map(|(_, f)| *f).collect();
        known.sort_unstable();
        let known: Vec<String> = known.iter().map(|f| format!("'{f}'")).collect();
        return Err(QuotaRequestError::Value(format!(
            "no GCP gpu_family mapping for accel '{accel}'; known: [{}]",
            known.join(", ")
        )));
    };
    Ok(gcp_request_for_family(
        client,
        region,
        family,
        new_limit,
        justification,
        contact_email,
    )
    .await?)
}
