//! `stado overview` — one operator snapshot for queue, fleet, quota and money.

use std::collections::HashSet;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};

use super::CmdError;
use crate::monitor::billing;
use crate::queue::{capacity, JobStorage, StorageError};
use crate::targets::{self, ComputeTarget, Registry};

const CLOUD_BILLING_BASE: &str = "https://cloudbilling.googleapis.com";
const BILLING_BUDGETS_BASE: &str = "https://billingbudgets.googleapis.com";
const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

pub async fn run(as_json: bool) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    let registry = targets::load_registry_auto()
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;

    let (jobs, consumers, billing_snapshot, budgets, quotas) = tokio::join!(
        queue_counts(&store),
        capacity::read_consumer_capacity(&store),
        read_billing(&store),
        read_gcp_budgets(),
        crate::scheduler::quota::summarize_quotas(&store),
    );

    let jobs = jobs?;
    let consumers = consumers?;
    let billing_snapshot = billing_snapshot?;
    let budgets = budgets;
    let quotas = match quotas {
        Ok(summary) => serde_json::to_value(summary)?,
        Err(err) => json!({"status": "error", "detail": err.to_string()}),
    };
    let fleet = fleet_snapshot(&registry, &consumers);
    let document = json!({
        "generated_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "jobs": jobs,
        "fleet": fleet,
        "quota": quotas,
        "billing": billing_snapshot,
        "budgets": budgets,
    });

    if as_json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        print_human(&document);
    }
    Ok(())
}

async fn queue_counts(store: &JobStorage) -> Result<Value, StorageError> {
    let mut counts = Map::new();
    for state in ["queue", "running", "completed", "uploaded", "failed"] {
        let prefix = format!("{state}/");
        let count = store
            .list_blobs_with_meta(&prefix)
            .await?
            .into_iter()
            .filter(|blob| {
                blob.name
                    .strip_prefix(&prefix)
                    .is_some_and(|name| name.ends_with(".json") && !name.contains('/'))
            })
            .count();
        counts.insert(state.to_string(), json!(count));
    }
    Ok(Value::Object(counts))
}

async fn read_billing(store: &JobStorage) -> Result<Value, StorageError> {
    let Some(text) = store.download_text(billing::BLOB).await? else {
        return Ok(json!({
            "status": "unavailable",
            "detail": format!("{} has not been published yet", billing::BLOB),
        }));
    };
    serde_json::from_str(&text).map_err(StorageError::from)
}

fn target_identities(target: &ComputeTarget) -> HashSet<String> {
    let mut identities = HashSet::from([targets::normalize_hostname(&target.name)]);
    identities.extend(
        target
            .hostnames
            .iter()
            .map(|name| targets::normalize_hostname(name)),
    );
    identities
}

fn target_for_consumer<'a>(
    registry: &'a Registry,
    consumer_id: &str,
    kind: &str,
) -> Option<&'a str> {
    let hostname = consumer_id
        .strip_prefix(&format!("{kind}-"))
        .unwrap_or(consumer_id);
    let hostname = targets::normalize_hostname(hostname);
    registry
        .targets
        .iter()
        .find(|target| target.kind == kind && target_identities(target).contains(&hostname))
        .map(|target| target.name.as_str())
}

fn fleet_snapshot(
    registry: &Registry,
    consumers: &std::collections::BTreeMap<String, Value>,
) -> Value {
    let mut active_targets = HashSet::new();
    let workers: Vec<Value> = consumers
        .iter()
        .map(|(consumer_id, payload)| {
            let kind = payload
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let target = target_for_consumer(registry, consumer_id, kind);
            if let Some(name) = target {
                active_targets.insert(name.to_string());
            }
            json!({
                "consumer_id": consumer_id,
                "target": target,
                "kind": kind,
                "published_at": payload.get("published_at").cloned().unwrap_or(Value::Null),
                "stado_version": payload.get("stado_version").cloned().unwrap_or(Value::Null),
                "free_slots": payload.get("free_slots").cloned().unwrap_or_else(|| json!({})),
                "free_vram_gb": payload.get("free_vram_gb").cloned().unwrap_or(Value::Null),
                "total_vram_gb": payload.get("total_vram_gb").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    let targets: Vec<Value> = registry
        .targets
        .iter()
        .map(|target| {
            let active_worker = target
                .is_provider(crate::capabilities::ProviderId::Local)
                .then(|| active_targets.contains(&target.name));
            json!({
                "name": target.name,
                "kind": target.kind,
                "active_worker": active_worker,
                "gpu_type": target.gpu_type,
                "slots": target.slots,
                "max_concurrent": target.max_concurrent,
                "pinned_only": target.pinned_only,
            })
        })
        .collect();
    let coordinators: Vec<Value> = registry
        .coordinators
        .iter()
        .map(|coordinator| {
            json!({
                "name": coordinator.name,
                "runtime": coordinator.runtime,
                "active": coordinator.active,
                "interval_seconds": coordinator.interval_seconds,
            })
        })
        .collect();
    let local_registered = registry
        .targets
        .iter()
        .filter(|target| target.is_provider(crate::capabilities::ProviderId::Local))
        .count();

    json!({
        "active_workers": workers.len(),
        "registered_targets": targets.len(),
        "registered_local_workers": local_registered,
        "workers": workers,
        "targets": targets,
        "coordinators": coordinators,
    })
}

async fn authorized_json(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<Value, String> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

async fn read_gcp_budgets() -> Value {
    let auth = match crate::skarbiec::gcp_provider().await {
        Ok(auth) => auth,
        Err(err) => return json!({"status": "error", "detail": err.to_string()}),
    };
    let token = match auth.token(&[CLOUD_PLATFORM_SCOPE]).await {
        Ok(token) => token,
        Err(err) => return json!({"status": "error", "detail": err.to_string()}),
    };
    let client = reqwest::Client::new();
    let billing_info_url = format!(
        "{CLOUD_BILLING_BASE}/v1/projects/{}/billingInfo",
        crate::config::project()
    );
    let billing_info = match authorized_json(&client, &billing_info_url, token.as_str()).await {
        Ok(value) => value,
        Err(err) => return json!({"status": "error", "detail": err}),
    };
    let Some(account) = billing_info
        .get("billingAccountName")
        .and_then(Value::as_str)
    else {
        return json!({"status": "unavailable", "detail": "project has no billing account"});
    };
    let budgets_url = format!("{BILLING_BUDGETS_BASE}/v1/{account}/budgets");
    match authorized_json(&client, &budgets_url, token.as_str()).await {
        Ok(value) => json!({
            "status": "ok",
            "billing_account": account,
            "budgets": value.get("budgets").cloned().unwrap_or_else(|| json!([])),
        }),
        Err(err) => json!({"status": "error", "billing_account": account, "detail": err}),
    }
}

fn number(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        })
        .unwrap_or_default()
}

fn money(value: &Value) -> String {
    let units = number(value.get("units"));
    let nanos = number(value.get("nanos"));
    let currency = value
        .get("currencyCode")
        .and_then(Value::as_str)
        .unwrap_or("USD");
    format!("{currency} {:.2}", units + nanos / 1_000_000_000.0)
}

fn print_human(document: &Value) {
    println!("STADO OVERVIEW");
    println!(
        "generated: {}",
        document
            .get("generated_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    );

    let jobs = &document["jobs"];
    println!(
        "jobs: {} running | {} queued | {} completed | {} uploaded | {} failed",
        jobs["running"], jobs["queue"], jobs["completed"], jobs["uploaded"], jobs["failed"]
    );

    let fleet = &document["fleet"];
    println!(
        "fleet: {} active workers | {} registered local | {} registered targets",
        fleet["active_workers"], fleet["registered_local_workers"], fleet["registered_targets"]
    );
    if let Some(workers) = fleet.get("workers").and_then(Value::as_array) {
        for worker in workers {
            let target = worker
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("unmapped");
            let version = worker
                .get("stado_version")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let kind = worker
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            println!("  worker: {target} [{kind}] stado={version}");
        }
    }
    if let Some(targets) = fleet.get("targets").and_then(Value::as_array) {
        let offline: Vec<&str> = targets
            .iter()
            .filter(|target| target.get("active_worker").and_then(Value::as_bool) == Some(false))
            .filter_map(|target| target.get("name").and_then(Value::as_str))
            .collect();
        if !offline.is_empty() {
            println!("  offline local: {}", offline.join(", "));
        }
    }

    println!("quota:");
    let quota = &document["quota"];
    if quota.get("status").and_then(Value::as_str) == Some("error") {
        println!(
            "  unavailable: {}",
            quota
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
    } else if let Some(providers) = quota.as_object() {
        for (provider, rows) in providers {
            let Some(rows) = rows.as_object() else {
                println!("  {provider}: unavailable");
                continue;
            };
            if rows.is_empty() {
                println!("  {provider}: no live quota visible");
            }
            for (accel, row) in rows {
                println!(
                    "  {provider}/{accel}: total={} used={} reserved={} available={}",
                    row["total"], row["used"], row["reserved"], row["available"]
                );
            }
        }
    } else {
        println!("  unavailable: malformed quota response");
    }

    println!("billing:");
    let billing = &document["billing"];
    println!(
        "  reported: {}",
        billing
            .get("reported_at")
            .and_then(Value::as_str)
            .unwrap_or("unavailable")
    );
    let gcp = &billing[crate::capabilities::ProviderId::Gcp.as_str()];
    if gcp.get("status").and_then(Value::as_str) == Some("ok") {
        if let Some(month) = gcp
            .get("monthly")
            .and_then(Value::as_array)
            .and_then(|rows| rows.last())
        {
            println!(
                "  GCP {}: gross=${:.2} credits=${:.2} net=${:.2}",
                month
                    .get("month")
                    .and_then(Value::as_str)
                    .unwrap_or("current"),
                number(month.get("gross")),
                -number(month.get("credits")),
                number(month.get("net")),
            );
        }
        println!(
            "  GCP credit burn (7d avg): ${:.2}/day",
            -number(gcp.get("avg_daily_credit_applied_7d"))
        );
        let promotion_used: f64 = gcp
            .get("credits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|credit| credit.get("type").and_then(Value::as_str) == Some("PROMOTION"))
            .map(|credit| -number(credit.get("cumulative")))
            .sum();
        println!("  GCP promotion credits applied: ${promotion_used:.2}");
        println!("  GCP promotion remaining: unavailable (grant ceiling is not exposed by GCP)");
    } else {
        println!(
            "  GCP: {}",
            gcp.get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unavailable")
        );
    }
    let azure = &billing[crate::capabilities::ProviderId::Azure.as_str()];
    if azure.get("status").and_then(Value::as_str) == Some("ok") {
        println!(
            "  Azure credits: current={} estimated={} {}",
            azure
                .get("available_balance")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("estimated_balance")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD"),
        );
        println!(
            "  Azure grant: amount={} used={}, valid {} — {}",
            azure
                .get("grant_amount")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("credit_used")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("grant_start_date")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            azure
                .get("grant_end_date")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        );
        println!(
            "  Azure pending eligible charges={} expired={}",
            azure
                .get("pending_eligible_charges")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
            azure
                .get("expired_credit")
                .map_or_else(|| "unknown".to_string(), Value::to_string),
        );
        if azure.get("overage_risk").and_then(Value::as_bool) == Some(true) {
            println!(
                "  Azure warning: spending limit is off; paid overage can continue after credits"
            );
        }
    } else {
        println!(
            "  Azure credits: {} — {}",
            azure
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unavailable"),
            azure
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail"),
        );
    }

    println!("budgets:");
    let budgets = &document["budgets"];
    if budgets.get("status").and_then(Value::as_str) == Some("ok") {
        let rows = budgets
            .get("budgets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if rows.is_empty() {
            println!("  no GCP budgets configured");
        }
        for budget in rows {
            let name = budget
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or("unnamed");
            let period = budget
                .pointer("/budgetFilter/calendarPeriod")
                .and_then(Value::as_str)
                .unwrap_or("CUSTOM");
            let amount = budget
                .pointer("/amount/specifiedAmount")
                .or_else(|| budget.pointer("/amount/lastPeriodAmount"));
            println!(
                "  {name}: {} ({period})",
                amount.map_or_else(|| "dynamic amount".to_string(), money)
            );
        }
    } else {
        println!(
            "  unavailable: {}",
            budgets
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
        );
    }
}
