//! GPU quota tracking — live from each provider's quota API, the storage
//! file is reservation overlay only.
//!
//! Ports both quota reads and the reservation overlay. GCP reads regional
//! limits through the Compute REST API; Azure reads regional
//! Microsoft.Compute usages through ARM and converts family vCPU limits to
//! schedulable GPU slots using the machine catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::catalog::{AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE, AZURE_VM_TO_ACCEL};
use crate::config;
use crate::providers::azure::{ArmClient, AzureError};
use crate::providers::gcp::{GceClient, GceError};
use crate::providers::{get_provider, Provider, ProviderError};
use crate::queue::{JobStorage, StorageError};

/// Map GCP regional-quota metric names to the accel_type strings the
/// scheduler uses internally. Tracks the ON-DEMAND quotas now that the
/// dispatcher forces preemptible=False everywhere (per 0.4.56's
/// no-preemptible policy). Earlier versions tracked
/// PREEMPTIBLE_NVIDIA_*_GPUS, which became wrong as soon as the dispatcher
/// stopped creating Spot VMs: 20 STANDARD T4s were running while the
/// scheduler still believed it had 20 free PREEMPTIBLE T4 slots, so it
/// would have dispatched into a saturated NVIDIA_T4_GPUS quota anyway and
/// 504'd on the QUOTA_EXCEEDED retry path.
pub const GCP_METRIC_TO_ACCEL: [(&str, &str); 4] = [
    ("NVIDIA_T4_GPUS", "nvidia-tesla-t4"),
    ("NVIDIA_L4_GPUS", "nvidia-l4"),
    ("NVIDIA_A100_GPUS", "nvidia-tesla-a100"),
    ("NVIDIA_A100_80GB_GPUS", "nvidia-a100-80gb"),
];

/// Quota-read error.
#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    /// GCP (GCE REST) failures from the live regions.get fan-out.
    #[error(transparent)]
    Gcp(#[from] GceError),
    /// Azure ARM failures from the regional usages fan-out.
    #[error(transparent)]
    Azure(#[from] AzureError),
    /// Storage failures reading the reservation overlay.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Python `json.JSONDecodeError` on a corrupt `config/quotas.json`.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Provider failures from `list_running_instances`.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

/// Python `int(value)` for JSON scalars in the quota dicts, with Python's
/// default of 0 for missing keys. Deviation: Python's `int()` raises
/// ValueError on a non-numeric string; this port treats garbage as 0 (the
/// live API only ever emits numbers, and the overlay is operator-written).
fn py_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

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

fn azure_family_slot(family: &str) -> Option<(&'static str, i64)> {
    let machine_type = AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE.get(family)?;
    let (accel, gpu_count) = AZURE_VM_TO_ACCEL.get(machine_type)?;
    if !gpu_count.is_positive() {
        return None;
    }
    let digits: String = machine_type
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    let vcpus = digits.parse::<i64>().ok()?;
    let vcpus_per_slot = vcpus / *gpu_count;
    vcpus_per_slot
        .is_positive()
        .then_some((*accel, vcpus_per_slot))
}

/// Live Azure regional quota limits converted from vCPU-family limits to
/// schedulable GPU slots.
pub async fn fetch_quotas_azure(
    client: &ArmClient,
    locations: &[String],
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let mut out = BTreeMap::new();
    for location in locations {
        for usage in client.list_usages(location).await? {
            let family = usage
                .pointer("/name/value")
                .and_then(Value::as_str)
                .unwrap_or("");
            let Some((accel, vcpus_per_slot)) = azure_family_slot(family) else {
                continue;
            };
            let slots = py_int(usage.get("limit")) / vcpus_per_slot;
            *out.entry(accel.to_string()).or_default() += slots;
        }
    }
    Ok(out)
}

/// The GCP project the quota read targets. Python quota.py resolves
/// `os.environ.get("GCP_PROJECT", "wisent-480400")` — env only, NOT
/// config.PROJECT (which also reads the config file). Kept env-only for
/// parity.
fn gcp_project_env() -> String {
    std::env::var("GCP_PROJECT").unwrap_or_else(|_| "wisent-480400".to_string())
}

/// Python `_load_overlay`: read the optional reservations file from the
/// queue's storage backend. Format:
/// `{"gcp": {"nvidia-tesla-a100": {"reserved": 4}, ...},
///   "azure": {"nvidia-a100-80gb": {"reserved": 1}, ...}}`.
/// Reservations subtract from the live cloud limit so non-wisent workloads
/// can keep some headroom without lowering the actual cloud quota. A
/// missing file is `{}`; a corrupt file raises (Python parity).
pub async fn load_overlay(store: &JobStorage) -> Result<Value, QuotaError> {
    let Some(raw) = store.download_text("config/quotas.json").await? else {
        return Ok(json!({}));
    };
    Ok(serde_json::from_str(&raw)?)
}

/// Compose live cloud quota limits with the storage-backed reservation
/// overlay (Python `load_quotas`).
///
/// Source of truth for `total` is the live cloud API — never the storage
/// file. The storage file only contributes `reserved` slots per accel.
/// Falls through to the storage file's `total` if the live API call
/// returns nothing (offline / dev).
pub async fn load_quotas(store: &JobStorage, provider_name: &str) -> Result<Value, QuotaError> {
    load_quotas_with_client(store, provider_name, None).await
}

/// [`load_quotas`] with an injectable GCE client (loopback mocks in
/// tests). `None` resolves a live client when the gcp arm needs one.
pub async fn load_quotas_with_client(
    store: &JobStorage,
    provider_name: &str,
    client: Option<&GceClient>,
) -> Result<Value, QuotaError> {
    let overlay = load_overlay(store).await?;
    let live: BTreeMap<String, i64> = match provider_name {
        "gcp" => {
            let owned;
            let client = match client {
                Some(client) => client,
                None => {
                    owned = GceClient::new(&gcp_project_env()).await?;
                    &owned
                }
            };
            fetch_quotas_gcp(client, config::regions()).await?
        }
        "azure" => {
            let subscription = config::azure_subscription_id();
            if subscription.is_empty() {
                return Err(AzureError::Auth("AZURE_SUBSCRIPTION_ID is required".into()).into());
            }
            let client = ArmClient::new(subscription);
            fetch_quotas_azure(&client, config::azure_locations()).await?
        }
        _ => BTreeMap::new(),
    };
    if live.is_empty() {
        return Ok(overlay);
    }
    let mut provider_rows = serde_json::Map::new();
    let overlay_p = overlay.get(provider_name).cloned().unwrap_or(json!({}));
    for (accel, total) in live {
        let reserved = py_int(overlay_p.get(&accel).and_then(|cfg| cfg.get("reserved")));
        provider_rows.insert(accel, json!({"total": total, "reserved": reserved}));
    }
    Ok(json!({ provider_name: Value::Object(provider_rows) }))
}

/// Count available GPU slots: total - reserved - running (Python
/// `get_available_slots`).
pub async fn get_available_slots(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
) -> Result<BTreeMap<String, i64>, QuotaError> {
    get_available_slots_with_client(store, provider, provider_name, None).await
}

/// [`get_available_slots`] with an injectable GCE client (tests).
pub async fn get_available_slots_with_client(
    store: &JobStorage,
    provider: &dyn Provider,
    provider_name: &str,
    client: Option<&GceClient>,
) -> Result<BTreeMap<String, i64>, QuotaError> {
    let quotas = load_quotas_with_client(store, provider_name, client).await?;
    let provider_quotas = quotas.get(provider_name).cloned().unwrap_or(json!({}));
    let running_counts = provider.list_running_instances().await?;

    let mut available = BTreeMap::new();
    let Some(rows) = provider_quotas.as_object() else {
        return Ok(available);
    };
    for (accel_type, cfg) in rows {
        let total = py_int(cfg.get("total"));
        let reserved = py_int(cfg.get("reserved"));
        let used = running_counts.get(accel_type).copied().unwrap_or(0);
        available.insert(accel_type.clone(), (total - reserved - used).max(0));
    }
    Ok(available)
}

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
    let mut providers: BTreeMap<String, Arc<dyn Provider>> = BTreeMap::new();
    for name in config::wc_providers() {
        // A provider whose constructor throws (creds missing) is skipped
        // here; its running count then defaults to {} below, like Python's
        // `except Exception: running = {}`.
        if let Ok(provider) = get_provider(name) {
            providers.insert(name.clone(), provider);
        }
    }
    summarize_quotas_with(store, None, &providers).await
}

/// [`summarize_quotas`] with injectable GCE client and provider instances
/// (loopback mocks in tests).
pub async fn summarize_quotas_with(
    store: &JobStorage,
    client: Option<&GceClient>,
    providers: &BTreeMap<String, Arc<dyn Provider>>,
) -> Result<BTreeMap<String, BTreeMap<String, QuotaRow>>, QuotaError> {
    let mut out: BTreeMap<String, BTreeMap<String, QuotaRow>> = BTreeMap::new();
    for provider_name in config::wc_providers() {
        let quotas = load_quotas_with_client(store, provider_name, client).await?;
        let provider_quotas = quotas.get(provider_name).cloned().unwrap_or(json!({}));
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
            let used = running.get(accel).copied().unwrap_or(0);
            summary_rows.insert(
                accel.clone(),
                QuotaRow {
                    total,
                    reserved,
                    used,
                    available: (total - reserved - used).max(0),
                },
            );
        }
        out.insert(provider_name.clone(), summary_rows);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::gcp::GceClient;
    use crate::queue::local_file::LocalBackend;
    use crate::testutil::{http_response, mock_http, MockHttp};

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    /// A provider with a fixed running-instances count (Python duck-typed
    /// test double).
    struct FakeProvider {
        running: BTreeMap<String, i64>,
    }

    #[async_trait::async_trait]
    impl Provider for FakeProvider {
        async fn create_instance(
            &self,
            _n: &str,
            _m: &str,
            _a: &str,
            _d: i64,
            _i: &str,
            _p: &str,
            _s: &str,
            _pre: bool,
        ) -> Result<Option<String>, ProviderError> {
            Ok(None)
        }
        async fn delete_instance(&self, _r: &str) -> Result<(), ProviderError> {
            Ok(())
        }
        async fn instance_exists(&self, _r: &str) -> Result<bool, ProviderError> {
            Ok(false)
        }
        async fn list_running_instances(&self) -> Result<BTreeMap<String, i64>, ProviderError> {
            Ok(self.running.clone())
        }
    }

    /// regions.get fixture: two GPU metrics plus one unmapped, float
    /// limits like the live API emits.
    fn region_body(t4: f64, a100: f64) -> String {
        format!(
            r#"{{"name": "{region}", "quotas": [
                {{"metric": "NVIDIA_T4_GPUS", "limit": {t4}, "usage": 2.0}},
                {{"metric": "NVIDIA_A100_GPUS", "limit": {a100}, "usage": 0.0}},
                {{"metric": "CPUS", "limit": 2400.0, "usage": 10.0}},
                {{"metric": "PREEMPTIBLE_NVIDIA_T4_GPUS", "limit": 99.0, "usage": 0.0}}
            ]}}"#,
            region = "us-central1"
        )
    }

    fn gcp_mock(server: &MockHttp) -> GceClient {
        GceClient::for_test(&server.base_url, "test-project")
    }

    #[tokio::test]
    async fn fetch_quotas_gcp_maps_metrics_and_truncates_float_limits() {
        let server = mock_http(vec![
            http_response(200, "OK", &region_body(8.9, 16.0)),
            http_response(200, "OK", &region_body(8.0, 4.0)),
        ])
        .await;
        let client = gcp_mock(&server);
        let regions = vec!["us-central1".to_string(), "europe-west4".to_string()];
        let quotas = fetch_quotas_gcp(&client, &regions).await.unwrap();
        // int(8.9) truncates to 8; CPUS / PREEMPTIBLE_* are not mapped.
        assert_eq!(
            quotas,
            BTreeMap::from([
                ("nvidia-tesla-a100".to_string(), 20),
                ("nvidia-tesla-t4".to_string(), 16),
            ])
        );
        let requests = server.requests.lock().unwrap().clone();
        // The mock base URL replaces the whole API base (no /compute/v1).
        assert!(
            requests[0].starts_with("GET /projects/test-project/regions/us-central1 "),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn load_quotas_composes_live_total_with_overlay_reserved() {
        let (_dir, store) = store();
        store
            .upload_text(
                "config/quotas.json",
                r#"{"gcp": {"nvidia-tesla-t4": {"reserved": 3}}}"#,
            )
            .await
            .unwrap();
        // One mock region per configured config::regions() entry (5 by
        // default); each contributes 8 T4 / 0 A100.
        let responses = (0..config::regions().len())
            .map(|_| http_response(200, "OK", &region_body(8.0, 0.0)))
            .collect();
        let server = mock_http(responses).await;
        let client = gcp_mock(&server);
        let quotas = load_quotas_with_client(&store, "gcp", Some(&client))
            .await
            .unwrap();
        server.stop();
        let expected_total = 8 * config::regions().len() as i64;
        assert_eq!(
            quotas,
            json!({"gcp": {
                "nvidia-tesla-t4": {"total": expected_total, "reserved": 3},
                "nvidia-tesla-a100": {"total": 0, "reserved": 0},
            }})
        );
    }

    #[tokio::test]
    async fn load_quotas_falls_back_to_overlay_when_live_is_empty() {
        let (_dir, store) = store();
        let overlay = r#"{"azure": {"nvidia-a100-80gb": {"total": 4, "reserved": 1}}}"#;
        store
            .upload_text("config/quotas.json", overlay)
            .await
            .unwrap();
        // The Azure live path is a stub contributing zero, so the overlay
        // passes through unchanged (Python parity for offline/dev).
        let quotas = load_quotas(&store, "azure").await.unwrap();
        assert_eq!(quotas, serde_json::from_str::<Value>(overlay).unwrap());

        // Unknown provider: live is {} too.
        let quotas = load_quotas(&store, "dcloud").await.unwrap();
        assert_eq!(quotas, serde_json::from_str::<Value>(overlay).unwrap());
    }

    #[tokio::test]
    async fn missing_overlay_file_is_empty_and_corrupt_raises() {
        let (_dir, store) = store();
        assert_eq!(load_overlay(&store).await.unwrap(), json!({}));
        store
            .upload_text("config/quotas.json", "{not json")
            .await
            .unwrap();
        let err = load_overlay(&store).await.unwrap_err();
        assert!(matches!(err, QuotaError::Json(_)), "{err:?}");
    }

    #[tokio::test]
    async fn available_slots_is_total_minus_reserved_minus_running() {
        let (_dir, store) = store();
        store
            .upload_text(
                "config/quotas.json",
                r#"{"azure": {"nvidia-tesla-t4": {"total": 10, "reserved": 2}, "nvidia-l4": {"total": 1, "reserved": 0}}}"#,
            )
            .await
            .unwrap();
        let provider = FakeProvider {
            running: BTreeMap::from([
                ("nvidia-tesla-t4".to_string(), 5),
                ("nvidia-l4".to_string(), 3),
            ]),
        };
        let available = get_available_slots(&store, &provider, "azure")
            .await
            .unwrap();
        assert_eq!(
            available,
            BTreeMap::from([
                ("nvidia-l4".to_string(), 0),       // 1 - 0 - 3 clamps at 0
                ("nvidia-tesla-t4".to_string(), 3), // 10 - 2 - 5
            ])
        );
    }

    #[tokio::test]
    async fn summarize_quotas_reports_total_reserved_used_available() {
        let (_dir, store) = store();
        store
            .upload_text(
                "config/quotas.json",
                r#"{"gcp": {"nvidia-tesla-t4": {"total": 8, "reserved": 1}}}"#,
            )
            .await
            .unwrap();
        // Default WC_PROVIDERS is ["gcp"]. Live fetch is stubbed out by
        // pointing the overlay-only path: "gcp" with no client would try
        // real auth, so inject a mock client serving one region body per
        // configured region.
        let responses = (0..config::regions().len())
            .map(|_| http_response(200, "OK", &region_body(8.0, 0.0)))
            .collect();
        let server = mock_http(responses).await;
        let client = gcp_mock(&server);
        let fake: Arc<dyn Provider> = Arc::new(FakeProvider {
            running: BTreeMap::from([("nvidia-tesla-t4".to_string(), 2)]),
        });
        let providers = BTreeMap::from([("gcp".to_string(), fake)]);
        let summary = summarize_quotas_with(&store, Some(&client), &providers)
            .await
            .unwrap();
        server.stop();
        let expected_total = 8 * config::regions().len() as i64;
        let gcp = &summary["gcp"];
        assert_eq!(
            gcp["nvidia-tesla-t4"],
            QuotaRow {
                total: expected_total,
                reserved: 1,
                used: 2,
                available: expected_total - 3
            }
        );
        assert_eq!(
            gcp["nvidia-tesla-a100"],
            QuotaRow {
                total: 0,
                reserved: 0,
                used: 0,
                available: 0
            }
        );
    }
}
