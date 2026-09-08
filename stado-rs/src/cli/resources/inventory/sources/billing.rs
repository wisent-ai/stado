//! Billing and balance health for the configured, billed providers.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::cli::resources::inventory::model::SourceReport;
use crate::monitor::billing;
use crate::queue::JobStorage;

pub(in crate::cli::resources::inventory) async fn inspect_billing(
    configured: &BTreeSet<String>,
) -> SourceReport {
    let billed = billing::providers()
        .into_iter()
        .filter(|provider| configured.contains(*provider))
        .collect::<Vec<_>>();
    if billed.is_empty() {
        return SourceReport {
            name: "billing",
            state: "skipped".to_string(),
            data: Value::Null,
            error: None,
        };
    }
    let store = match JobStorage::new().await {
        Ok(store) => store,
        Err(error) => {
            return SourceReport {
                name: "billing",
                state: "blocked".to_string(),
                data: Value::Null,
                error: Some(error.to_string()),
            }
        }
    };
    match tokio::time::timeout(crate::doctor::PROBE_TIMEOUT, billing::live_snapshot(&store)).await {
        Ok(snapshot) => {
            let failures: Vec<String> = billed
                .iter()
                .filter_map(|provider| {
                    let status = snapshot
                        .get(*provider)
                        .and_then(|section| section.get("status"))
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    (status != "ok").then(|| format!("{provider}: {status}"))
                })
                .collect();
            SourceReport {
                name: "billing",
                state: if failures.is_empty() {
                    "ok".to_string()
                } else {
                    "degraded".to_string()
                },
                data: snapshot,
                error: if failures.is_empty() {
                    None
                } else {
                    Some(failures.join("; "))
                },
            }
        }
        Err(_) => SourceReport {
            name: "billing",
            state: "blocked".to_string(),
            data: Value::Null,
            error: Some(format!(
                "billing inventory exceeded {:?}",
                crate::doctor::PROBE_TIMEOUT
            )),
        },
    }
}
