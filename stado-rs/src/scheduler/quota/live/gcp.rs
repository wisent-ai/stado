//! The GCP half of the live read: the regional-quota metric names the
//! scheduler's accel_type strings are mapped from, the regions.get fan-out
//! that sums them, and the project the read targets.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::providers::gcp::GceClient;
use crate::providers::ProviderError;
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

/// Map GCP regional-quota metric names to the accel_type strings the
/// scheduler uses internally. Tracks the ON-DEMAND quotas now that the
/// dispatcher forces preemptible=False everywhere (per 0.4.56's
/// no-preemptible policy). Earlier versions tracked
/// PREEMPTIBLE_NVIDIA_*_GPUS, which became wrong as soon as the dispatcher
/// stopped creating Spot VMs:
/// 20 STANDARD T4s were running while the
/// scheduler still believed it had 20 free PREEMPTIBLE T4 slots, so it
/// would have dispatched into a saturated NVIDIA_T4_GPUS quota anyway and
/// 504'd on the QUOTA_EXCEEDED retry path.
pub const GCP_METRIC_TO_ACCEL: &[(&str, &str)] = &[
    ("NVIDIA_T4_GPUS", "nvidia-tesla-t4"),
    ("NVIDIA_L4_GPUS", "nvidia-l4"),
    ("NVIDIA_A100_GPUS", "nvidia-tesla-a100"),
    ("NVIDIA_A100_80GB_GPUS", "nvidia-a100-80gb"),
];

/// Live regional quota limits from GCP, summed across all dispatch
/// regions, keyed by internal accel_type names (Python
/// `_fetch_quotas_gcp`).
///
/// The first failing region propagates. A region answer without a `quotas`
/// list, or a quota row naming no `metric` or no `limit`, is refused with
/// the row: the Compute Engine Region resource states both for every quota
/// (<https://cloud.google.com/compute/docs/reference/rest/v1/regions>), and
/// an unstated limit read as zero made a granted accelerator look exhausted.
pub async fn fetch_quotas_gcp(
    client: &GceClient,
    regions: &[String],
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let unstated = |field: &str, row: &Value| {
        QuotaError::Provider(ProviderError::Value(format!(
            "the GCP quota row {row} states no {field}"
        )))
    };
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for region in regions {
        let path = format!("/projects/{}/regions/{region}", client.project());
        let region_obj = client.get(&path, &format!("get region {region}")).await?;
        let Some(quotas) = region_obj.get("quotas").and_then(Value::as_array) else {
            return Err(unstated("quotas list", &region_obj));
        };
        for quota in quotas {
            let Some(metric) = quota.get("metric").and_then(Value::as_str) else {
                return Err(unstated("metric", quota));
            };
            let Some((_, accel)) = GCP_METRIC_TO_ACCEL.iter().find(|(m, _)| *m == metric)
            else {
                continue;
            };
            // Python int(q.limit): float limits truncate.
            let Some(limit) = py_int("limit", quota.get("limit"))? else {
                return Err(unstated("limit", quota));
            };
            *out.entry(accel.to_string()).or_default() += limit;
        }
    }
    Ok(out)
}

/// The GCP project the quota read targets: the deployment's declared project
/// (`crate::config::project`). No project is built in; an undeclared one is
/// refused by `GceClient::new` naming `GCP_PROJECT`.
pub(in crate::scheduler::quota) fn gcp_project_env() -> String {
    crate::config::project().to_string()
}
