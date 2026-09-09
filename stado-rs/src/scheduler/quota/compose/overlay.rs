//! The reservation overlay: the storage-backed file itself, the key a
//! configured provider variant is filed under, and the composition of that
//! overlay with a live limit map — either an already-fetched one or the one
//! the provider's own quota adapter is dispatched to for.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::config;
use crate::providers::azure::{ArmClient, AzureError};
use crate::providers::gcp::GceClient;
use crate::queue::JobStorage;
use crate::scheduler::quota::live::azure::fetch_quotas_azure;
use crate::scheduler::quota::live::gcp::{fetch_quotas_gcp, gcp_project_env};
use crate::scheduler::quota::scalar::py_int;
use crate::scheduler::quota::QuotaError;

/// Python `_load_overlay`: read the optional reservations file from the
/// queue's storage backend. Format:
/// `{"gcp": {"nvidia-tesla-a100": {"reserved":
/// 4}, ...},
///   "azure": {"nvidia-a100-80gb": {"reserved":
/// 1}, ...}}`.
/// Reservations subtract from the live cloud limit so non-wisent workloads
/// can keep some headroom without lowering the actual cloud quota. A
/// missing file is `{}`; a corrupt file raises (Python parity).
pub async fn load_overlay(store: &JobStorage) -> Result<Value, QuotaError> {
    let Some(raw) = store.download_text("config/quotas.json").await? else {
        return Ok(json!({}));
    };
    Ok(serde_json::from_str(&raw)?)
}

/// The canonical overlay/live-quota key for a configured provider variant.
pub(super) fn quota_provider_key(provider_name: &str) -> &str {
    crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, provider_name)
        .map_or(provider_name, |variant| variant.id)
}

/// Compose an already-fetched live limit map with the storage-backed
/// reservation overlay. An empty live map deliberately passes the complete
/// overlay through unchanged for offline/dev operation.
pub(super) async fn load_quotas_from_live(
    store: &JobStorage,
    provider_name: &str,
    live: BTreeMap<String, i64>,
) -> Result<Value, QuotaError> {
    let overlay = load_overlay(store).await?;
    if live.is_empty() {
        return Ok(overlay);
    }
    let provider_key = quota_provider_key(provider_name);
    let overlay_p = overlay.get(provider_key).cloned().unwrap_or(json!({}));
    let mut provider_rows = serde_json::Map::new();
    for (accel, total) in live {
        let reserved = py_int(overlay_p.get(&accel).and_then(|cfg| cfg.get("reserved")));
        provider_rows.insert(accel, json!({"total": total, "reserved": reserved}));
    }
    Ok(json!({ provider_key: Value::Object(provider_rows) }))
}

/// Compose live cloud quota limits with the storage-backed reservation
/// overlay (Python `load_quotas`).
///
/// Source of truth for `total` is the live cloud API — never the storage
/// file. The storage file only contributes `reserved` slots per accel.
/// Falls through to the storage file's `total` if the live API returns
/// nothing (offline / dev).
pub async fn load_quotas(store: &JobStorage, provider_name: &str) -> Result<Value, QuotaError> {
    let variant =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, provider_name);
    let live = match variant.map(|variant| variant.adapter) {
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Gcp,
        )) => {
            let client = GceClient::new(&gcp_project_env()).await?;
            fetch_quotas_gcp(&client, config::regions()).await?
        }
        Some(crate::capabilities::RuntimeAdapter::Quota(
            crate::capabilities::QuotaAdapter::Azure,
        )) => {
            let subscription = config::azure_subscription_id();
            if subscription.is_empty() {
                return Err(AzureError::Auth("AZURE_SUBSCRIPTION_ID is required".into()).into());
            }
            let client = ArmClient::new(subscription);
            fetch_quotas_azure(&client, config::azure_locations()).await?
        }
        _ => BTreeMap::new(),
    };
    load_quotas_from_live(store, provider_name, live).await
}
