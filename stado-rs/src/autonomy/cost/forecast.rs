//! The burn projection and the anomalies it exposes.
//!
//! [`forecast`] projects the allocated run-rate to the end of the day and of
//! the billing month against the policy budgets, and [`detect_anomalies`]
//! reports the overruns, the idle paid resources and the spend nobody owns.
//! The billing-snapshot readers at the bottom are what the projection starts
//! from when the provider has already invoiced part of the month.

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::InventorySnapshot;
use crate::autonomy::policy::AutonomyPolicy;

use super::allocation::AllocationReport;
use super::HOURS_PER_DAY;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostForecast {
    pub created_at: String,
    pub current_hourly_usd: f64,
    pub end_of_day_usd: f64,
    pub end_of_month_usd: f64,
    pub hourly_budget_usd: Option<f64>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    pub hourly_overrun_usd: f64,
    pub daily_overrun_usd: f64,
    pub budget_exceeded: bool,
    pub projected_overrun_usd: f64,
    pub credit_runway_days: Option<f64>,
}

pub fn forecast(
    allocation: &AllocationReport,
    policy: &AutonomyPolicy,
    billing_snapshot: Option<&Value>,
    now: DateTime<Utc>,
) -> CostForecast {
    let mut current_hourly = allocation
        .entries
        .iter()
        .filter(|entry| entry.source == "live hourly price")
        .map(|entry| entry.net_cost_usd)
        .sum::<f64>();
    let month_start = (now.date_naive() - chrono::Days::new(u64::from(now.day0())))
        .and_time(chrono::NaiveTime::MIN)
        .and_utc();
    let month_hours = f64::from(now.num_days_in_month()) * HOURS_PER_DAY;
    let elapsed_month_hours =
        (now - month_start).as_seconds_f64() / crate::monitor::billing::SECONDS_PER_HOUR as f64;
    let remaining_month_hours = (month_hours - elapsed_month_hours).max(0.0);
    let spent = billing_net_cost(billing_snapshot).unwrap_or_default();
    if elapsed_month_hours > 0.0 {
        current_hourly = current_hourly.max(spent / elapsed_month_hours);
    }
    let end_of_month = spent + current_hourly * remaining_month_hours;
    let end_of_day = current_hourly * HOURS_PER_DAY;
    let hourly_overrun = policy
        .budgets
        .hourly_usd
        .map(|limit| (current_hourly - limit).max(0.0))
        .unwrap_or_default();
    let daily_overrun = policy
        .budgets
        .daily_usd
        .map(|limit| (end_of_day - limit).max(0.0))
        .unwrap_or_default();
    let budget = policy.budgets.monthly_usd;
    CostForecast {
        created_at: now.to_rfc3339(),
        current_hourly_usd: current_hourly,
        end_of_day_usd: end_of_day,
        end_of_month_usd: end_of_month,
        hourly_budget_usd: policy.budgets.hourly_usd,
        daily_budget_usd: policy.budgets.daily_usd,
        monthly_budget_usd: budget,
        projected_overrun_usd: budget
            .map(|limit| (end_of_month - limit).max(0.0))
            .unwrap_or_default(),
        hourly_overrun_usd: hourly_overrun,
        daily_overrun_usd: daily_overrun,
        budget_exceeded: hourly_overrun > 0.0
            || daily_overrun > 0.0
            || budget.is_some_and(|limit| end_of_month > limit),
        credit_runway_days: credit_balance(billing_snapshot).and_then(|balance| {
            let daily = current_hourly * HOURS_PER_DAY;
            (daily > 0.0).then_some(balance / daily)
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostAnomaly {
    pub anomaly_id: String,
    pub severity: String,
    pub kind: String,
    pub subject: String,
    pub reason: String,
    pub observed_value: f64,
    pub expected_value: f64,
}

pub fn detect_anomalies(
    allocation: &AllocationReport,
    inventory: &InventorySnapshot,
    forecast: &CostForecast,
) -> Vec<CostAnomaly> {
    let mut anomalies = Vec::new();
    if forecast.hourly_overrun_usd > 0.0 {
        anomalies.push(anomaly(
            "hourly-budget-overrun",
            "critical",
            "budget_forecast",
            "hourly-budget",
            "current hourly burn exceeds the configured budget",
            forecast.current_hourly_usd,
            forecast.hourly_budget_usd.unwrap_or_default(),
        ));
    }
    if forecast.daily_overrun_usd > 0.0 {
        anomalies.push(anomaly(
            "daily-budget-overrun",
            "critical",
            "budget_forecast",
            "daily-budget",
            "projected daily run-rate exceeds the configured budget",
            forecast.end_of_day_usd,
            forecast.daily_budget_usd.unwrap_or_default(),
        ));
    }
    if forecast.projected_overrun_usd > 0.0 {
        anomalies.push(anomaly(
            "budget-overrun",
            "critical",
            "budget_forecast",
            "monthly-budget",
            "projected month-end cost exceeds the configured budget",
            forecast.end_of_month_usd,
            forecast.monthly_budget_usd.unwrap_or_default(),
        ));
    }
    if allocation.unallocated.net_cost_usd > 0.0 {
        anomalies.push(anomaly(
            "unallocated-spend",
            "high",
            "unallocated_cost",
            "cost-ledger",
            "cost exists without an owner or workload attribution",
            allocation.unallocated.net_cost_usd,
            0.0,
        ));
    }
    for resource in &inventory.resources {
        let hourly = resource.current_hourly_cost_usd.unwrap_or_default();
        if hourly <= 0.0 {
            continue;
        }
        let utilization = resource
            .utilization
            .get("gpu")
            .or_else(|| resource.utilization.get("cpu"))
            .copied();
        if utilization.is_some_and(|value| value <= f64::EPSILON) {
            anomalies.push(anomaly(
                &format!("idle:{}", resource.resource_id),
                "high",
                "idle_paid_resource",
                &resource.resource_id,
                "paid resource reports no utilization",
                hourly,
                0.0,
            ));
        }
        if resource.ownership == super::model::Ownership::Unknown {
            anomalies.push(anomaly(
                &format!("unknown-owner:{}", resource.resource_id),
                "medium",
                "unknown_owner_spend",
                &resource.resource_id,
                "paid resource has no Stado ownership contract",
                hourly,
                0.0,
            ));
        }
    }
    for (provider, bucket) in &allocation.by_provider {
        let attributed = allocation.entries.iter().any(|entry| {
            entry.provider.as_str() == provider
                && (entry.workload.is_some() || entry.job_id.is_some())
        });
        if bucket.net_cost_usd > 0.0 && !attributed {
            anomalies.push(anomaly(
                &format!("provider-without-workload:{provider}"),
                "medium",
                "provider_spend_without_workload",
                provider,
                "provider cost exists without workload attribution",
                bucket.net_cost_usd,
                0.0,
            ));
        }
    }
    anomalies
}

fn anomaly(
    id: &str,
    severity: &str,
    kind: &str,
    subject: &str,
    reason: &str,
    observed: f64,
    expected: f64,
) -> CostAnomaly {
    CostAnomaly {
        anomaly_id: id.to_string(),
        severity: severity.to_string(),
        kind: kind.to_string(),
        subject: subject.to_string(),
        reason: reason.to_string(),
        observed_value: observed,
        expected_value: expected,
    }
}

fn billing_net_cost(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .pointer("/gcp/latest_month/net_cost")
        .or_else(|| snapshot.pointer("/gcp/net_cost"))
        .and_then(Value::as_f64)
}

fn credit_balance(snapshot: Option<&Value>) -> Option<f64> {
    let snapshot = snapshot?;
    snapshot
        .pointer("/azure/available_balance")
        .or_else(|| snapshot.pointer("/azure/balance"))
        .and_then(Value::as_f64)
}
