//! The GCP half of the live read: the regional-quota metric names the
//! scheduler's accel_type strings are mapped from, the regions.get fan-out
//! that sums them, and the project the read targets.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::providers::gcp::GceClient;
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
pub const GCP_METRIC_TO_ACCEL: [(&str, &str); 4] = [
    ("NVIDIA_T4_GPUS", "nvidia-tesla-t4"),
    ("NVIDIA_L4_GPUS", "nvidia-l4"),
    ("NVIDIA_A100_GPUS", "nvidia-tesla-a100"),
    ("NVIDIA_A100_80GB_GPUS", "nvidia-a100-80gb"),
];

/// Live regional quota limits from GCP, summed across all dispatch
/// regions, keyed by internal accel_type names (Python
/// `_fetch_quotas_gcp`).
///
/// Python's docstring claims "{} on any error in the FIRST region; partial
/// coverage across regions is preserved", but the code has no try/except —
/// a regions.get failure raises out of `load_quotas`. This port keeps the
/// code-as-written behavior: the first failing region propagates.
pub async fn fetch_quotas_gcp(
    client: &GceClient,
    regions: &[String],
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let mut out: BTreeMap<String, i64> = BTreeMap::new();
    for region in regions {
        let path = format!("/projects/{}/regions/{region}", client.project());
        let region_obj = client.get(&path, &format!("get region {region}")).await?;
        if let Some(quotas) = region_obj.get("quotas").and_then(Value::as_array) {
            for quota in quotas {
                let metric = quota.get("metric").and_then(Value::as_str).unwrap_or("");
                let Some((_, accel)) = GCP_METRIC_TO_ACCEL.iter().find(|(m, _)| *m == metric)
                else {
                    continue;
                };
                // Python int(q.limit): float limits truncate.
                let limit = py_int(quota.get("limit"));
                *out.entry(accel.to_string()).or_insert(0) += limit;
            }
        }
    }
    Ok(out)
}

/// The GCP project the quota read targets. Python quota.py resolves
/// `os.environ.get("GCP_PROJECT", "wisent-480400")` — env only, NOT
/// config.PROJECT (which also reads the config file). Kept env-only for
/// parity.
pub(in crate::scheduler::quota) fn gcp_project_env() -> String {
    let env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
    )
    .expect("GCP project binding is missing from the capability catalog");
    std::env::var(env).unwrap_or_else(|_| "wisent-480400".to_string())
}
