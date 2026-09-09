//! The dispatcher-facing headroom count: one provider's composed quota
//! dict minus its reservations minus the instances it is already running.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::providers::Provider;
use crate::queue::JobStorage;
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

use super::overlay::{load_quotas, quota_provider_key};

/// Count additional GPU instances allowed by provider quota:
/// total - reserved - running.
pub async fn get_available_instances(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let quotas = load_quotas(store, provider_name).await?;
    available_instances_from_quotas(provider, provider_name, &quotas).await
}

async fn available_instances_from_quotas(
    provider: &dyn Provider,
    provider_name: &str,
    quotas: &Value,
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let provider_quotas = quotas
        .get(quota_provider_key(provider_name))
        .cloned()
        .unwrap_or(json!({}));
    let running_counts = provider.list_running_instances().await?;

    let mut available = BTreeMap::new();
    let Some(rows) = provider_quotas.as_object() else {
        return Ok(available);
    };
    for (accel_type, cfg) in rows {
        let total = py_int(cfg.get("total"));
        let reserved = py_int(cfg.get("reserved"));
        let used = running_counts.get(accel_type).copied().unwrap_or_default();
        available.insert(
            accel_type.clone(),
            (total - reserved - used).max(i64::default()),
        );
    }
    Ok(available)
}
