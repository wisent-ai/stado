//! The burn projection and the anomalies it exposes.
//!
//! [`forecast`] projects the allocated run-rate to the end of the day and of
//! the billing month against the policy budgets, and [`detect_anomalies`]
//! reports the overruns, the idle paid resources and the spend nobody owns.
//! The billing-snapshot readers at the bottom are what the projection starts
//! from when the provider has already invoiced part of the month.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::{InventorySnapshot, SCHEMA_VERSION};
use crate::autonomy::policy::AutonomyPolicy;

use super::allocation::AllocationReport;
use super::{BILLING_MONTH_DAYS, HOURS_PER_DAY};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostForecast {
    pub schema_version: u16,
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
    let elapsed_hours = now.hour() as f64;
    let month_days = days_in_month(now) as f64;
    let elapsed_month_hours = ((now.day() - true as u32) as f64 * HOURS_PER_DAY) + elapsed_hours;
    let remaining_month_hours =
        (month_days * HOURS_PER_DAY - elapsed_month_hours).max(f64::default());
    let spent = billing_net_cost(billing_snapshot).unwrap_or_default();
    if elapsed_month_hours > f64::default() {
        current_hourly = current_hourly.max(spent / elapsed_month_hours);
    }
    let end_of_month = spent + current_hourly * remaining_month_hours;
    let end_of_day = current_hourly * HOURS_PER_DAY;
    let hourly_overrun = policy
        .budgets
        .hourly_usd
        .map(|limit| (current_hourly - limit).max(f64::default()))
        .unwrap_or_default();
    let daily_overrun = policy
        .budgets
        .daily_usd
        .map(|limit| (end_of_day - limit).max(f64::default()))
        .unwrap_or_default();
    let budget = policy.budgets.monthly_usd;
    CostForecast {
        schema_version: SCHEMA_VERSION,
        created_at: now.to_rfc3339(),
        current_hourly_usd: current_hourly,
        end_of_day_usd: end_of_day,
        end_of_month_usd: end_of_month,
        hourly_budget_usd: policy.budgets.hourly_usd,
        daily_budget_usd: policy.budgets.daily_usd,
        monthly_budget_usd: budget,
        projected_overrun_usd: budget
            .map(|limit| (end_of_month - limit).max(f64::default()))
            .unwrap_or_default(),
        hourly_overrun_usd: hourly_overrun,
        daily_overrun_usd: daily_overrun,
        budget_exceeded: hourly_overrun > f64::default()
            || daily_overrun > f64::default()
            || budget.is_some_and(|limit| end_of_month > limit),
        credit_runway_days: credit_balance(billing_snapshot).and_then(|balance| {
            let daily = current_hourly * HOURS_PER_DAY;
            (daily > f64::default()).then_some(balance / daily)
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
    if forecast.hourly_overrun_usd > f64::default() {
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
    if forecast.daily_overrun_usd > f64::default() {
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
    if forecast.projected_overrun_usd > f64::default() {
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
    if allocation.unallocated.net_cost_usd > f64::default() {
        anomalies.push(anomaly(
            "unallocated-spend",
            "high",
            "unallocated_cost",
            "cost-ledger",
            "cost exists without an owner or workload attribution",
            allocation.unallocated.net_cost_usd,
            f64::default(),
        ));
    }
    for resource in &inventory.resources {
        let hourly = resource.current_hourly_cost_usd.unwrap_or_default();
        if hourly <= f64::default() {
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
                f64::default(),
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
                f64::default(),
            ));
        }
    }
    for (provider, bucket) in &allocation.by_provider {
        let attributed_resources = allocation
            .entries
            .iter()
            .filter(|entry| {
                entry.provider.as_str() == provider
                    && (entry.workload.is_some() || entry.job_id.is_some())
            })
            .count();
        if bucket.net_cost_usd > f64::default() && attributed_resources == usize::default() {
            anomalies.push(anomaly(
                &format!("provider-without-workload:{provider}"),
                "medium",
                "provider_spend_without_workload",
                provider,
                "provider cost exists without workload attribution",
                bucket.net_cost_usd,
                f64::default(),
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

fn days_in_month(now: DateTime<Utc>) -> u32 {
    let next_month = if now.month() == u8::BITS + (u16::BITS / u8::BITS) {
        chrono::NaiveDate::from_ymd_opt(now.year() + true as i32, true as u32, true as u32)
    } else {
        chrono::NaiveDate::from_ymd_opt(now.year(), now.month() + true as u32, true as u32)
    };
    next_month
        .and_then(|next| next.pred_opt())
        .map(|last| last.day())
        .unwrap_or(BILLING_MONTH_DAYS as u32)
}
