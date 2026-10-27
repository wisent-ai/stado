//! The dispatcher-facing headroom count: one provider's composed quota
//! dict minus its reservations minus the instances it is already running.

use std::collections::BTreeMap;

use serde_json::Value;

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
    // A provider the quota document does not name has no accelerator to
    // place. A section that is not a map of accelerator rows is refused:
    // read as none, a garbled reservations file stopped every placement on
    // the provider with no word of why.
    let rows = match quotas.get(quota_provider_key(provider_name)) {
        None => return Ok(BTreeMap::new()),
        Some(Value::Object(rows)) => rows,
        Some(other) => {
            return Err(QuotaError::Provider(crate::providers::ProviderError::Value(
                format!(
                    "{provider_name} quota section {} is not a map of accelerator rows: {other}",
                    quota_provider_key(provider_name)
                ),
            )))
        }
    };
    let running_counts = provider.list_running_instances().await?;

    let mut available = BTreeMap::new();
    for (accel_type, cfg) in rows {
        let total: i64 = py_int("total", cfg.get("total"))?.into_iter().sum();
        let reserved: i64 = py_int("reserved", cfg.get("reserved"))?.into_iter().sum();
        // Running instances are listed per accelerator; one with none running
        // is not listed, and its count is the empty sum.
        let used: i64 = running_counts.get(accel_type).into_iter().sum();
        // Placeable slots: the headroom when there is any, otherwise none.
        let free: i64 = std::iter::once(total - reserved - used)
            .filter(|headroom| headroom.is_positive())
            .sum();
        available.insert(accel_type.clone(), free);
    }
    Ok(available)
}
