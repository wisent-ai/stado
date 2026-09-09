//! The operator-facing cross-provider picture: the per-accel row shape, the
//! production sweep over the configured providers, and the injectable
//! implementation both it and the fixtures drive.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;

use crate::config;
use crate::providers::{get_provider, Provider};
use crate::queue::JobStorage;
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

use super::overlay::{load_quotas, load_quotas_from_live, quota_provider_key};

/// One accel row of the Python `summarize_quotas` output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QuotaRow {
    pub total: i64,
    pub reserved: i64,
    pub used: i64,
    pub available: i64,
}

/// Cross-provider quota summary keyed by provider, then by accel (Python
/// `summarize_quotas`).
///
/// Each accel entry carries `total` (live cloud limit summed across the
/// provider's configured regions/locations), `reserved` (the storage
/// overlay's hold), `used` (live running-instance count from the
/// provider's own API), and `available` (= max(0, total-reserved-used)).
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
    let provider_names = config::wc_providers().to_vec();
    let mut providers: BTreeMap<String, Arc<dyn Provider>> = BTreeMap::new();
    for name in &provider_names {
        // A provider whose constructor throws (creds missing) is skipped
        // here; its running count then defaults to {} below, like Python's
        // `except Exception: running = {}`.
        if let Ok(provider) = get_provider(name) {
            providers.insert(name.clone(), provider);
        }
    }
    summarize_quotas_with(store, &provider_names, None, &providers).await
}

/// Summary implementation with an explicit provider list and optional live
/// quota fixture. Production passes `None` and retains live provider reads;
/// tests inject per-provider maps and never consult ambient provider config or
/// cloud authentication.
async fn summarize_quotas_with(
    store: &JobStorage,
    provider_names: &[String],
    live_by_provider: Option<&BTreeMap<String, BTreeMap<String, i64>>>,
    providers: &BTreeMap<String, Arc<dyn Provider>>,
) -> Result<BTreeMap<String, BTreeMap<String, QuotaRow>>, QuotaError> {
    let mut out: BTreeMap<String, BTreeMap<String, QuotaRow>> = BTreeMap::new();
    for provider_name in provider_names {
        let quotas = match live_by_provider {
            Some(live) => {
                load_quotas_from_live(
                    store,
                    provider_name,
                    live.get(provider_name).cloned().unwrap_or_default(),
                )
                .await?
            }
            None => load_quotas(store, provider_name).await?,
        };
        let provider_quotas = quotas
            .get(quota_provider_key(provider_name))
            .cloned()
            .unwrap_or(json!({}));
        let Some(rows) = provider_quotas.as_object() else {
            out.insert(provider_name.clone(), BTreeMap::new());
            continue;
        };
        if rows.is_empty() {
            out.insert(provider_name.clone(), BTreeMap::new());
            continue;
        }
        let running = match providers.get(provider_name) {
            Some(provider) => provider.list_running_instances().await.unwrap_or_default(),
            None => BTreeMap::new(),
        };
        let mut summary_rows = BTreeMap::new();
        for (accel, cfg) in rows {
            let total = py_int(cfg.get("total"));
            let reserved = py_int(cfg.get("reserved"));
            let used = running.get(accel).copied().unwrap_or_default();
            summary_rows.insert(
                accel.clone(),
                QuotaRow {
                    total,
                    reserved,
                    used,
                    available: (total - reserved - used).max(i64::default()),
                },
            );
        }
        out.insert(provider_name.clone(), summary_rows);
    }
    Ok(out)
}
