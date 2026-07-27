//! Cloud Quotas API quota-increase orchestrator.
//!
//! Port of `stado/scheduler/dispatch/quota_request.py`. Wraps the GCP
//! Cloud Quotas CreateQuotaPreference/UpdateQuotaPreference REST API and
//! Azure Microsoft.Quota create_or_update (ARM REST PUT) so a single
//! `stado quota request <accel> --to N` invocation fans out one
//! quota-increase request per (provider, region) across every provider in
//! WC_PROVIDERS. Co-located in scheduler/dispatch/ because submitting a
//! quota preference is the write-side mirror of dispatch's read-side
//! get_available_slots: both treat per-(provider, region, accel) GPU
//! ceilings as the unit of work.
//!
//! GCP: the newer Cloud Quotas API expresses GPU quotas as a single
//! quota_id `GPUS-PER-GPU-FAMILY-per-project-region` parameterized by a
//! dimensions={"region": ..., "gpu_family": ...} map. Submission is
//! non-blocking: the QuotaPreference is created or updated and Google's
//! reviewer approves/declines asynchronously. ALREADY_EXISTS is converted
//! to UpdateQuotaPreference so re-running the command bumps an existing
//! pending request to the new preferred_value rather than erroring.
//!
//! Azure: Microsoft.Quota provider, create_or_update against
//! `subscriptions/{sub}/providers/Microsoft.Compute/locations/{loc}`
//! with resource_name = the SKU family name.
//!
//! Deviation: Python's Azure path treats a missing azure-mgmt-quota SDK as
//! an informational {"available": false, "reason": "azure-mgmt-quota not
//! installed"} row. The Rust port calls ARM REST directly (no optional
//! SDK), so only the `AZURE_SUBSCRIPTION_ID unset` unavailable case
//! remains.

use serde_json::{json, Value};

use crate::catalog::AZURE_QUOTA_FAMILY_TO_ACCEL;
use crate::config;
use crate::providers::azure::{ArmClient, AzureError};

use super::quota_skus::{gcp_project_env, CatalogError, CloudQuotasClient};

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

/// Microsoft.Quota API version used by the Python azure-mgmt-quota SDK.
pub const AZURE_QUOTA_API_VERSION: &str = "2023-02-01";

/// Quota-write error.
#[derive(Debug, thiserror::Error)]
pub enum QuotaRequestError {
    /// GCP Cloud Quotas REST failures.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// Azure ARM failures.
    #[error(transparent)]
    Azure(#[from] AzureError),
    /// Python `ValueError` (unknown accel label).
    #[error("{0}")]
    Value(String),
}

/// Python's quotaPreferenceId:
/// `f"compute-gpus-{region}-{gpu_family}".lower().replace("_", "-")`.
pub fn gcp_preference_id(region: &str, gpu_family: &str) -> String {
    format!("compute-gpus-{region}-{gpu_family}").to_lowercase().replace('_', "-")
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
        let mut known: Vec<&str> =
            GCP_ACCEL_TO_GPU_FAMILY.iter().map(|(_, f)| *f).collect();
        known.sort_unstable();
        let known: Vec<String> = known.iter().map(|f| format!("'{f}'")).collect();
        return Err(QuotaRequestError::Value(format!(
            "no GCP gpu_family mapping for accel '{accel}'; known: [{}]",
            known.join(", ")
        )));
    };
    Ok(gcp_request_for_family(client, region, family, new_limit, justification, contact_email).await?)
}

/// The Microsoft.Quota create_or_update body (Python
/// `create_quota_request`).
pub fn azure_quota_body(family_name: &str, new_limit: i64) -> Value {
    json!({
        "properties": {
            "limit": {"limitObjectType": "LimitValue", "value": new_limit},
            "name": {"value": family_name},
            "resourceType": "dedicated",
        }
    })
}

/// The Microsoft.Compute scope the quota resource hangs under.
pub fn azure_quota_scope(subscription: &str, location: &str) -> String {
    format!("subscriptions/{subscription}/providers/Microsoft.Compute/locations/{location}")
}

/// Submit an Azure Microsoft.Quota create_or_update for a compute family
/// against an injectable ARM client (Python
/// `client.quota.begin_create_or_update(...).result()` — an LRO wait).
pub async fn azure_request_increase_with_client(
    client: &ArmClient,
    location: &str,
    family_name: &str,
    new_limit: i64,
) -> Result<Value, AzureError> {
    let scope = azure_quota_scope(client.subscription(), location);
    let path = format!(
        "/{scope}/providers/Microsoft.Quota/quotas/{family_name}?api-version={AZURE_QUOTA_API_VERSION}"
    );
    let resp = client
        .put_lro(
            &path,
            &azure_quota_body(family_name, new_limit),
            &format!("quota create_or_update {family_name}"),
        )
        .await?;
    let name = resp.get("id").and_then(Value::as_str).unwrap_or(family_name);
    Ok(json!({"name": name, "available": true}))
}

/// Submit an Azure Microsoft.Quota create_or_update for a compute family.
/// Python `_azure_request_increase`.
///
/// Returns {"available": True, "name": ...} on success or
/// {"available": False, "reason": ...} when AZURE_SUBSCRIPTION_ID is
/// empty; the latter surfaces as an informational result-list entry
/// instead of aborting a multi-provider fan-out.
pub async fn azure_request_increase(
    subscription: &str,
    location: &str,
    family_name: &str,
    new_limit: i64,
) -> Result<Value, AzureError> {
    if subscription.is_empty() {
        return Ok(json!({"available": false, "reason": "AZURE_SUBSCRIPTION_ID unset"}));
    }
    let client = ArmClient::new(subscription);
    azure_request_increase_with_client(&client, location, family_name, new_limit).await
}

/// Python `_gcp_fanout`. Per-target failures are captured as result rows
/// rather than aborting the rest of the fan-out. `gcp_client` is
/// injectable for tests; `None` resolves a live client, and a
/// client-construction (auth) failure is reported per region exactly like
/// Python constructing the SDK client inside the per-region try.
pub async fn gcp_fanout(
    gcp_client: Option<&CloudQuotasClient>,
    accel: &str,
    new_limit: i64,
    regions: Option<&[String]>,
    justification: &str,
    contact_email: &str,
) -> Vec<Value> {
    let targets: Vec<String> =
        regions.map(<[String]>::to_vec).unwrap_or_else(|| config::regions().to_vec());
    let owned;
    let client = match gcp_client {
        Some(client) => Some(client),
        None => match CloudQuotasClient::new(&gcp_project_env()).await {
            Ok(c) => {
                owned = c;
                Some(&owned)
            }
            Err(_) => None,
        },
    };
    let mut out = Vec::new();
    for region in &targets {
        let row = match client {
            Some(client) => {
                match gcp_request_increase(
                    client,
                    region,
                    accel,
                    new_limit,
                    justification,
                    contact_email,
                )
                .await
                {
                    Ok(r) => {
                        let mut row = json!({"provider": "gcp", "region": region, "ok": true});
                        merge_object(&mut row, r);
                        row
                    }
                    Err(err) => json!({
                        "provider": "gcp", "region": region, "ok": false,
                        "error": format!("{}: {err}", py_type_name(&err)),
                    }),
                }
            }
            None => json!({
                "provider": "gcp", "region": region, "ok": false,
                "error": "DefaultCredentialsError: Cloud Quotas client construction failed",
            }),
        };
        out.push(row);
    }
    out
}

/// Python `_azure_fanout`.
pub async fn azure_fanout(accel: &str, new_limit: i64, regions: Option<&[String]>) -> Vec<Value> {
    let mut families: Vec<&str> = AZURE_QUOTA_FAMILY_TO_ACCEL
        .iter()
        .filter(|(_, a)| **a == accel)
        .map(|(f, _)| *f)
        .collect();
    // Deviation: Python iterates the literal dict in insertion order; the
    // Rust catalog table is a HashMap, so rows are sorted for determinism.
    families.sort_unstable();
    if families.is_empty() {
        return vec![json!({
            "provider": "azure", "ok": false,
            "error": format!("no Azure compute family matches accel '{accel}'"),
        })];
    }
    let targets: Vec<String> =
        regions.map(<[String]>::to_vec).unwrap_or_else(|| config::azure_locations().to_vec());
    let subscription = config::azure_subscription_id();
    let mut out = Vec::new();
    for loc in &targets {
        for fam in &families {
            let row = match azure_request_increase(subscription, loc, fam, new_limit).await {
                Ok(r) if r.get("available").and_then(Value::as_bool) == Some(false) => json!({
                    "provider": "azure", "location": loc, "family": fam, "ok": false,
                    "error": r.get("reason").and_then(Value::as_str).unwrap_or("not available"),
                }),
                Ok(r) => {
                    let mut row =
                        json!({"provider": "azure", "location": loc, "family": fam, "ok": true});
                    merge_object(&mut row, r);
                    row
                }
                Err(err) => json!({
                    "provider": "azure", "location": loc, "family": fam, "ok": false,
                    "error": format!("AzureError: {err}"),
                }),
            };
            out.push(row);
        }
    }
    out
}

/// Fan out quota-increase requests across providers and regions. Python
/// `request_quota_increases`.
///
/// For each provider in `providers`, iterate `regions` (or the provider's
/// configured region/location list when None) and submit one
/// quota-increase request per (provider, region). Per-target failures are
/// captured in the result list rather than aborting the rest of the
/// fan-out: each entry carries `provider`, a region/location key, `ok`
/// (bool), and either `name` (success) or `error`.
pub async fn request_quota_increases(
    gcp_client: Option<&CloudQuotasClient>,
    accel: &str,
    new_limit: i64,
    providers: &[String],
    regions: Option<&[String]>,
    justification: &str,
    contact_email: &str,
) -> Vec<Value> {
    let mut out = Vec::new();
    for provider in providers {
        match provider.as_str() {
            "gcp" => out.extend(
                gcp_fanout(gcp_client, accel, new_limit, regions, justification, contact_email)
                    .await,
            ),
            "azure" => out.extend(azure_fanout(accel, new_limit, regions).await),
            other => out.push(json!({
                "provider": other, "ok": false,
                "error": "no quota-increase impl for this provider",
            })),
        }
    }
    out
}

/// Python `{**base, **r}` row merge.
pub(crate) fn merge_object(row: &mut Value, extra: Value) {
    if let (Value::Object(base), Value::Object(extra)) = (row, extra) {
        base.extend(extra);
    }
}

/// Python `type(exc).__name__` for per-target error rows.
fn py_type_name(err: &QuotaRequestError) -> &'static str {
    match err {
        QuotaRequestError::Catalog(_) => "GoogleAPICallError",
        QuotaRequestError::Azure(_) => "AzureError",
        QuotaRequestError::Value(_) => "ValueError",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http};

    #[test]
    fn gcp_preference_id_and_body_match_python() {
        assert_eq!(
            gcp_preference_id("us-central1", "NVIDIA_A100_80GB"),
            "compute-gpus-us-central1-nvidia-a100-80gb"
        );
        let body = gcp_preference_body("europe-west4", "NVIDIA_T4", 16, "need it", "a@b.c");
        assert_eq!(
            body,
            json!({
                "service": "compute.googleapis.com",
                "quotaId": "GPUS-PER-GPU-FAMILY-per-project-region",
                "quotaConfig": {"preferredValue": "16"},
                "dimensions": {"region": "europe-west4", "gpu_family": "NVIDIA_T4"},
                "justification": "need it",
                "contactEmail": "a@b.c",
            })
        );
    }

    #[test]
    fn azure_quota_body_and_scope_match_python() {
        assert_eq!(
            azure_quota_body("standardNCADSA100v4Family", 192),
            json!({
                "properties": {
                    "limit": {"limitObjectType": "LimitValue", "value": 192},
                    "name": {"value": "standardNCADSA100v4Family"},
                    "resourceType": "dedicated",
                }
            })
        );
        assert_eq!(
            azure_quota_scope("sub-1", "eastus"),
            "subscriptions/sub-1/providers/Microsoft.Compute/locations/eastus"
        );
    }

    #[tokio::test]
    async fn gcp_request_for_family_creates_then_updates_on_already_exists() {
        let client = CloudQuotasClient::for_test(
            &mock_http(vec![http_response(
                200,
                "OK",
                r#"{"name": "projects/p/locations/global/quotaPreferences/compute-gpus-us-central1-nvidia-t4"}"#,
            )])
            .await
            .base_url,
            "p",
        );
        let r = gcp_request_for_family(&client, "us-central1", "NVIDIA_T4", 16, "j", "e@x")
            .await
            .unwrap();
        assert_eq!(
            r,
            json!({
                "name": "projects/p/locations/global/quotaPreferences/compute-gpus-us-central1-nvidia-t4",
                "created": true,
            })
        );

        // ALREADY_EXISTS (HTTP 409) converts to an update.
        let server = mock_http(vec![
            http_response(
                409,
                "Conflict",
                r#"{"error": {"code": 409, "status": "ALREADY_EXISTS", "message": "already exists"}}"#,
            ),
            http_response(
                200,
                "OK",
                r#"{"name": "projects/p/locations/global/quotaPreferences/compute-gpus-us-east1-nvidia-l4"}"#,
            ),
        ])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "p");
        let r = gcp_request_for_family(&client, "us-east1", "NVIDIA_L4", 8, "j", "e@x").await.unwrap();
        assert_eq!(r["created"], json!(false));
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            requests[0].starts_with(
                "POST /projects/p/locations/global/quotaPreferences?quotaPreferenceId=compute-gpus-us-east1-nvidia-l4 "
            ),
            "{}",
            requests[0]
        );
        assert!(
            requests[1]
                .starts_with("PATCH /projects/p/locations/global/quotaPreferences/compute-gpus-us-east1-nvidia-l4 "),
            "{}",
            requests[1]
        );
        // The update body carries the resource name (Python qp.name).
        assert!(requests[1].contains(r#"\"name\":\"projects/p/locations/global/quotaPreferences/compute-gpus-us-east1-nvidia-l4\""#)
            || requests[1].contains(r#""name":"projects/p/locations/global/quotaPreferences/compute-gpus-us-east1-nvidia-l4""#),
            "{}", requests[1]);
        server.stop();
    }

    #[tokio::test]
    async fn gcp_request_increase_rejects_unknown_accel_like_python() {
        let client = CloudQuotasClient::for_test("http://127.0.0.1:1", "p");
        let err = gcp_request_increase(&client, "us-central1", "nvidia-h100-80gb", 16, "j", "e@x")
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "no GCP gpu_family mapping for accel 'nvidia-h100-80gb'; \
             known: ['NVIDIA_A100', 'NVIDIA_A100_80GB', 'NVIDIA_L4', 'NVIDIA_T4']"
        );
    }

    #[tokio::test]
    async fn azure_request_increase_puts_quota_resource_and_waits_lro() {
        let server = mock_http(vec![
            // PUT returns 201 + Azure-AsyncOperation header.
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nAzure-AsyncOperation: /op/1\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"id\": \"x\"}".to_string(),
            http_response(200, "OK", r#"{"status": "Succeeded"}"#),
        ])
        .await;
        let client = ArmClient::for_test(&server.base_url, "sub-9");
        let r = azure_request_increase_with_client(&client, "eastus", "fam", 192).await.unwrap();
        assert_eq!(r, json!({"name": "x", "available": true}));
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with(
                "PUT /subscriptions/sub-9/providers/Microsoft.Compute/locations/eastus/providers/Microsoft.Quota/quotas/fam?api-version=2023-02-01 "
            ),
            "{}",
            requests[0]
        );
        assert!(requests[1].starts_with("GET /op/1 "), "{}", requests[1]);
        server.stop();
    }

    #[tokio::test]
    async fn azure_request_increase_unset_subscription_is_informational() {
        let r = azure_request_increase("", "eastus", "fam", 16).await.unwrap();
        assert_eq!(r, json!({"available": false, "reason": "AZURE_SUBSCRIPTION_ID unset"}));
    }

    #[tokio::test]
    async fn fanouts_capture_per_target_failures_and_unknown_providers() {
        // Azure fanout for an accel with no family mapping.
        let rows = azure_fanout("no-such-accel", 16, Some(&["eastus".to_string()])).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ok"], json!(false));
        assert_eq!(
            rows[0]["error"],
            json!("no Azure compute family matches accel 'no-such-accel'")
        );

        // request_quota_increases: unknown provider gets a static row.
        let rows = request_quota_increases(
            None,
            "nvidia-tesla-t4",
            16,
            &["dcloud".to_string()],
            None,
            "j",
            "e@x",
        )
        .await;
        assert_eq!(
            rows,
            vec![json!({
                "provider": "dcloud", "ok": false,
                "error": "no quota-increase impl for this provider",
            })]
        );
    }

    #[tokio::test]
    async fn gcp_fanout_with_mock_client_rows() {
        let server = mock_http(vec![http_response(
            200,
            "OK",
            r#"{"name": "projects/p/locations/global/quotaPreferences/x"}"#,
        )])
        .await;
        let client = CloudQuotasClient::for_test(&server.base_url, "p");
        let rows = gcp_fanout(
            Some(&client),
            "nvidia-tesla-t4",
            16,
            Some(&["us-central1".to_string()]),
            "j",
            "e@x",
        )
        .await;
        server.stop();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["ok"], json!(true));
        assert_eq!(rows[0]["provider"], json!("gcp"));
        assert_eq!(rows[0]["region"], json!("us-central1"));
        assert_eq!(
            rows[0]["name"],
            json!("projects/p/locations/global/quotaPreferences/x")
        );
    }
}
