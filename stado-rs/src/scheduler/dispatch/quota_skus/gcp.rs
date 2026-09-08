//! GCP arm: the GPU-quota filter, the project binding, the Cloud Quotas
//! catalog read, the QuotaPreference status rows, and the
//! CreateQuotaPreference fan-out across every discovered gpu_family.

use std::sync::LazyLock;

use serde_json::{json, Value};

use super::client::{CatalogError, CloudQuotasClient};

/// Python's GPU-quota filter: `re.search(r"NVIDIA-[A-Z0-9_-]+-GPUS", qid)`.
fn legacy_gpu_quota_re() -> &'static regex::Regex {
    static RE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"NVIDIA-[A-Z0-9_-]+-GPUS").expect("static regex compiles")
    });
    &RE
}

/// The GCP project the catalog read targets. Python quota_skus.py resolves
/// `os.environ.get("GCP_PROJECT", "wisent-480400")` — env only, NOT
/// config.PROJECT. Kept env-only for parity.
pub(crate) fn gcp_project_env() -> String {
    let env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
    )
    .expect("GCP project binding is missing from the capability catalog");
    std::env::var(env).unwrap_or_else(|_| "wisent-480400".to_string())
}

/// Enumerate every GPU-related compute.googleapis.com QuotaInfo in the
/// project, one entry per (quota_id, region) with its current limit
/// (Python `_gcp_catalog`). The limit comes from
/// QuotaInfo.dimensionsInfos: each DimensionsInfo carries an
/// applicableLocations list + a details.value field that is the current
/// per-region cap.
///
/// Note: no hardcoded family list. Rows whose quota_id is the unified
/// GPUS-PER-GPU-FAMILY-per-project-region quota carry a populated
/// `gpu_family` dimension that is the ground truth for what families
/// Google currently models in this project. Anything else would
/// reintroduce the "hardcoded list drifts from reality" problem.
pub async fn gcp_catalog(client: &CloudQuotasClient) -> Result<Vec<Value>, CatalogError> {
    let mut out = Vec::new();
    for info in client.list_quota_infos().await? {
        let qid = info.get("quotaId").and_then(Value::as_str).unwrap_or("");
        let is_gpu_family = qid.contains("GPUS-PER-GPU-FAMILY");
        let is_legacy_gpu = legacy_gpu_quota_re().is_match(qid);
        if !(is_gpu_family || is_legacy_gpu) {
            continue;
        }
        // metric_display_name or metric (empty display name falls back).
        let metric = info
            .get("metricDisplayName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| info.get("metric").and_then(Value::as_str))
            .unwrap_or("");
        let empty_dims = Vec::new();
        let dims_infos = info
            .get("dimensionsInfos")
            .and_then(Value::as_array)
            .unwrap_or(&empty_dims);
        for di in dims_infos {
            let locs: Vec<&str> = di
                .get("applicableLocations")
                .and_then(Value::as_array)
                .map(|locs| locs.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            // details.value is an int64 (string-encoded per the Google
            // JSON convention); int(value) in Python.
            let limit = di
                .get("details")
                .and_then(|d| d.get("value"))
                .and_then(|v| match v {
                    Value::String(s) => s.parse::<i64>().ok(),
                    Value::Number(n) => n.as_i64(),
                    _ => None,
                });
            let gpu_family = di
                .get("dimensions")
                .and_then(|d| d.get("gpu_family"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // Python `for loc in locs or ["global"]`.
            let locations: Vec<&str> = if locs.is_empty() {
                vec!["global"]
            } else {
                locs
            };
            for loc in locations {
                out.push(json!({
                    "provider": crate::capabilities::ProviderId::Gcp.as_str(),
                    "quota_id": qid,
                    "metric": metric,
                    "gpu_family": gpu_family,
                    "region": loc,
                    "limit": limit,
                }));
            }
        }
    }
    Ok(out)
}

/// Google int64 JSON convention: string-encoded, but tolerate numbers.
fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// One row per QuotaPreference (Python `gcp_request_status`). Buckets
/// stateDetail into a state field (approved/partially_approved/denied/
/// reconciling/unknown).
pub async fn gcp_request_status(client: &CloudQuotasClient) -> Result<Vec<Value>, CatalogError> {
    let mut out = Vec::new();
    for pref in client.list_quota_preferences().await? {
        let config = pref.get("quotaConfig").cloned().unwrap_or(Value::Null);
        let sd = config
            .get("stateDetail")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let reconciling = pref
            .get("reconciling")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let state = if reconciling {
            "reconciling"
        } else if sd.contains("partially approved") {
            "partially_approved"
        } else if sd.contains("approved") {
            "approved"
        } else if sd.contains("denied") {
            "denied"
        } else {
            "unknown"
        };
        let dims = pref.get("dimensions").cloned().unwrap_or(json!({}));
        // Python `qc.granted_value if qc and qc.granted_value else None`:
        // a missing/zero granted value is null in the row.
        let granted = json_i64(config.get("grantedValue")).filter(|v| *v != 0);
        out.push(json!({
            "name": pref.get("name").and_then(Value::as_str).unwrap_or(""),
            "quota_id": pref.get("quotaId").and_then(Value::as_str).unwrap_or(""),
            "gpu_family": dims.get("gpu_family").and_then(Value::as_str).unwrap_or(""),
            "region": dims.get("region").and_then(Value::as_str).unwrap_or(""),
            "preferred_value": json_i64(config.get("preferredValue")).unwrap_or(0),
            "granted_value": granted,
            "state": state,
            "state_detail": config.get("stateDetail").and_then(Value::as_str).unwrap_or(""),
            "create_time": pref.get("createTime").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    Ok(out)
}

/// Fan out CreateQuotaPreference for every gpu_family the live
/// cloudquotas API reports under compute.googleapis.com, in every region
/// passed (Python `gcp_request_all_families`). Uses the unified
/// GPUS-PER-GPU-FAMILY-per-project-region quota (the one that takes a
/// gpu_family dimension); the set of families is discovered, not
/// hardcoded — anything Google drops or adds tomorrow is picked up on the
/// next call without a package release.
pub async fn gcp_request_all_families(
    client: &CloudQuotasClient,
    new_limit: i64,
    regions: &[String],
    contact_email: &str,
    justification: &str,
) -> Result<Vec<Value>, CatalogError> {
    // Discover (a) the set of gpu_family values Google models for this
    // project, and (b) the UNION of every region any family is available
    // in. Per-family applicable_regions is conservative (it only lists
    // regions where the project has a non-default quota); the union gives
    // us the full lattice of regions Google serves any GPU SKU in.
    // Default behavior submits each family in every region in that union
    // — over-coverage; per-target "family not available in this region"
    // failures are captured as result-list entries, not exceptions.
    let mut families: std::collections::BTreeSet<String> = Default::default();
    let mut all_regions: std::collections::BTreeSet<String> = Default::default();
    for row in gcp_catalog(client).await? {
        if row.get("quota_id").and_then(Value::as_str)
            != Some("GPUS-PER-GPU-FAMILY-per-project-region")
        {
            continue;
        }
        let fam = row.get("gpu_family").and_then(Value::as_str).unwrap_or("");
        let region = row.get("region").and_then(Value::as_str).unwrap_or("");
        if !fam.is_empty() {
            families.insert(fam.to_string());
        }
        if !region.is_empty() {
            all_regions.insert(region.to_string());
        }
    }
    let requested: std::collections::BTreeSet<&str> = regions.iter().map(String::as_str).collect();
    let mut out = Vec::new();
    for fam in &families {
        let targets: Vec<&String> = if requested.is_empty() {
            all_regions.iter().collect()
        } else {
            all_regions
                .iter()
                .filter(|r| requested.contains(r.as_str()))
                .collect()
        };
        for region in targets {
            match super::quota_request::gcp_request_for_family(
                client,
                region,
                fam,
                new_limit,
                justification,
                contact_email,
            )
            .await
            {
                Ok(r) => {
                    let mut row = json!({
                        "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region,
                        "gpu_family": fam, "ok": true,
                    });
                    super::quota_request::merge_object(&mut row, r);
                    out.push(row);
                }
                Err(err) => out.push(json!({
                    "provider": crate::capabilities::ProviderId::Gcp.as_str(), "region": region,
                    "gpu_family": fam, "ok": false,
                    "error": format!("GoogleAPICallError: {err}"),
                })),
            }
        }
    }
    Ok(out)
}
