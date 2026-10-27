//! The operator-facing cross-provider picture: the per-accel row shape, the
//! production sweep over the configured providers, and the injectable
//! implementation both it and the fixtures drive.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config;
use crate::providers::{get_provider, ProviderError};
use crate::queue::JobStorage;
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

use super::overlay::{load_quotas, quota_provider_key};

/// One accel row of the Python `summarize_quotas` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QuotaRow {
    pub total: i64,
    pub reserved: i64,
    pub used: i64,
    /// Slots still placeable: a provider at or over its limit has none, so the
    /// signed headroom is counted only when it converts to a slot count.
    pub available: u64,
}

/// Cross-provider quota summary keyed by provider, then by accel (Python
/// `summarize_quotas`).
///
/// Each accel entry carries `total` (live cloud limit summed across the
/// provider's configured regions/locations), `reserved` (the storage
/// overlay's hold), `used` (live running-instance count from the
/// provider's own API), and `available` (the slots still placeable; none when
/// `total - reserved - used` is not above zero).
/// Provider iteration follows `WC_PROVIDERS` so the picture matches what
/// `schedule_queued_jobs` actually considers each tick. A provider whose
/// quota fetch returns nothing (credentials absent, SDK not installed)
/// appears in the output as an empty dict so the caller can distinguish
/// "configured but unreachable" from "not configured at all".
///
/// Deviation: the Rust map is BTreeMap-ordered (alphabetical), where the
/// Python dict preserves WC_PROVIDERS insertion order. The CLI's --json
/// output sorts keys anyway; the table order differs only with multiple
/// providers configured.
pub async fn summarize_quotas(
    store: &JobStorage,
) -> Result<BTreeMap<String, BTreeMap<String, QuotaRow>>, QuotaError> {
    let mut out: BTreeMap<String, BTreeMap<String, QuotaRow>> = BTreeMap::new();
    for provider_name in config::wc_providers() {
        let quotas = load_quotas(store, provider_name).await?;
        // A provider the quota document does not name declares no quota. A
        // section that is not a map of accelerator rows is refused: read as
        // none, a garbled reservations file showed the provider as having no
        // quota at all.
        let rows = match quotas.get(quota_provider_key(provider_name)) {
            None => {
                out.insert(provider_name.clone(), BTreeMap::new());
                continue;
            }
            Some(Value::Object(rows)) if rows.is_empty() => {
                out.insert(provider_name.clone(), BTreeMap::new());
                continue;
            }
            Some(Value::Object(rows)) => rows,
            Some(other) => {
                return Err(QuotaError::Provider(ProviderError::Value(format!(
                    "{provider_name} quota section {} is not a map of accelerator rows: {other}",
                    quota_provider_key(provider_name)
                ))))
            }
        };
        // A running count that cannot be read is refused: read as none, every
        // accelerator showed its whole quota as available. That includes a
        // provider that could not be constructed to be asked.
        let running = match get_provider(provider_name) {
            Ok(provider) => provider.list_running_instances().await?,
            Err(error) => {
                return Err(QuotaError::Provider(ProviderError::Value(format!(
                    "{provider_name} declares quota but its running instances cannot be read: \
                     {error}"
                ))))
            }
        };
        let mut summary_rows = BTreeMap::new();
        for (accel, cfg) in rows {
            let total: i64 = py_int("total", cfg.get("total"))?.into_iter().sum();
            let reserved: i64 = py_int("reserved", cfg.get("reserved"))?.into_iter().sum();
            // Running instances are listed per accelerator; one with none
            // running is not listed, and its count is the empty sum.
            let used: i64 = running.get(accel).into_iter().sum();
            summary_rows.insert(
                accel.clone(),
                QuotaRow {
                    total,
                    reserved,
                    used,
                    available: u64::try_from(total - reserved - used).into_iter().sum(),
                },
            );
        }
        out.insert(provider_name.clone(), summary_rows);
    }
    Ok(out)
}
