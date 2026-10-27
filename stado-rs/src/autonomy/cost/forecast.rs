//! The burn projection and the anomalies it exposes.
//!
//! [`forecast`] projects the allocated run-rate to the end of the day and of
//! the billing month against the policy budgets, and [`detect_anomalies`]
//! reports the overruns, the idle paid resources and the spend nobody owns.
//! The month's billed spend the projection starts from, and the credit the
//! burn runs down, come from [`super::billed`].

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::InventorySnapshot;
use crate::autonomy::policy::AutonomyPolicy;

use super::allocation::AllocationReport;
use super::billed::{credit_balance, month_spent};
use super::HOURS_PER_DAY;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostForecast {
    pub created_at: String,
    pub current_hourly_usd: f64,
    pub end_of_day_usd: f64,
    /// The spend the billing providers have billed this calendar month.
    pub month_spent_usd: Option<f64>,
    /// Why the month's billed spend is not known, when it is not.
    pub month_spent_unknown: Option<String>,
    /// The billed spend plus the current burn to the month's end; unknown
    /// while the billed spend is, never projected from zero spent.
    pub end_of_month_usd: Option<f64>,
    pub hourly_budget_usd: Option<f64>,
    pub daily_budget_usd: Option<f64>,
    pub monthly_budget_usd: Option<f64>,
    /// How far each figure exceeds the budget the policy declares for it;
    /// absent while it does not, or while no budget or figure is known.
    pub hourly_overrun_usd: Option<f64>,
    pub daily_overrun_usd: Option<f64>,
    pub budget_exceeded: bool,
    pub projected_overrun_usd: Option<f64>,
    pub credit_runway_days: Option<f64>,
}

/// An amount of money there is some of, as the price readers judge one.
fn positive(amount: f64) -> bool {
    amount.is_normal() && amount.is_sign_positive()
}

/// How far `figure` exceeds `limit`, when it does.
fn exceeding(limit: f64, figure: f64) -> Option<f64> {
    (figure > limit).then(|| figure - limit)
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
    let remaining_month_hours = month_hours - elapsed_month_hours;
    let (month_spent_usd, month_spent_unknown) = match month_spent(billing_snapshot, now) {
        Ok(spent) => (Some(spent), None),
        Err(cause) => (None, Some(cause)),
    };
    if let Some(spent) = month_spent_usd {
        if now > month_start {
            current_hourly = current_hourly.max(spent / elapsed_month_hours);
        }
    }
    let end_of_month = month_spent_usd.map(|spent| spent + current_hourly * remaining_month_hours);
    let end_of_day = current_hourly * HOURS_PER_DAY;
    let hourly_overrun = policy
        .budgets
        .hourly_usd
        .and_then(|limit| exceeding(limit, current_hourly));
    let daily_overrun = policy
        .budgets
        .daily_usd
        .and_then(|limit| exceeding(limit, end_of_day));
    let budget = policy.budgets.monthly_usd;
    let projected_overrun = budget
        .zip(end_of_month)
        .and_then(|(limit, projected)| exceeding(limit, projected));
    CostForecast {
        created_at: now.to_rfc3339(),
        current_hourly_usd: current_hourly,
        end_of_day_usd: end_of_day,
        month_spent_usd,
        month_spent_unknown,
        end_of_month_usd: end_of_month,
        hourly_budget_usd: policy.budgets.hourly_usd,
        daily_budget_usd: policy.budgets.daily_usd,
        monthly_budget_usd: budget,
        projected_overrun_usd: projected_overrun,
        hourly_overrun_usd: hourly_overrun,
        daily_overrun_usd: daily_overrun,
        budget_exceeded: hourly_overrun.is_some()
            || daily_overrun.is_some()
            || projected_overrun.is_some(),
        credit_runway_days: credit_balance(billing_snapshot).and_then(|balance| {
            let daily = current_hourly * HOURS_PER_DAY;
            positive(daily).then(|| balance / daily)
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
    /// The budget the observed figure exceeds; absent for spend that should
    /// not exist at all (unowned, unattributed or idle).
    pub expected_value: Option<f64>,
}

pub fn detect_anomalies(
    allocation: &AllocationReport,
    inventory: &InventorySnapshot,
    forecast: &CostForecast,
) -> Vec<CostAnomaly> {
    let mut anomalies = Vec::new();
    let overruns = [
        (
            "hourly-budget-overrun",
            "hourly-budget",
            "current hourly burn exceeds the configured budget",
            Some(forecast.current_hourly_usd),
            forecast.hourly_budget_usd,
            forecast.hourly_overrun_usd,
        ),
        (
            "daily-budget-overrun",
            "daily-budget",
            "projected daily run-rate exceeds the configured budget",
            Some(forecast.end_of_day_usd),
            forecast.daily_budget_usd,
            forecast.daily_overrun_usd,
        ),
        (
            "budget-overrun",
            "monthly-budget",
            "projected month-end cost exceeds the configured budget",
            forecast.end_of_month_usd,
            forecast.monthly_budget_usd,
            forecast.projected_overrun_usd,
        ),
    ];
    for (id, subject, reason, observed, budget, overrun) in overruns {
        if let (Some(observed), Some(budget), Some(_overrun)) = (observed, budget, overrun) {
            anomalies.push(anomaly(
                id,
                "critical",
                "budget_forecast",
                subject,
                reason,
                observed,
                Some(budget),
            ));
        }
    }
    if positive(allocation.unallocated.net_cost_usd) {
        anomalies.push(anomaly(
            "unallocated-spend",
            "high",
            "unallocated_cost",
            "cost-ledger",
            "cost exists without an owner or workload attribution",
            allocation.unallocated.net_cost_usd,
            None,
        ));
    }
    for resource in &inventory.resources {
        // A resource with no live price has no burn to call idle.
        let Some(hourly) = resource.current_hourly_cost_usd.filter(|hourly| positive(*hourly))
        else {
            continue;
        };
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
                None,
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
                None,
            ));
        }
    }
    for (provider, bucket) in &allocation.by_provider {
        let attributed = allocation.entries.iter().any(|entry| {
            entry.provider.as_str() == provider
                && (entry.workload.is_some() || entry.job_id.is_some())
        });
        if positive(bucket.net_cost_usd) && !attributed {
            anomalies.push(anomaly(
                &format!("provider-without-workload:{provider}"),
                "medium",
                "provider_spend_without_workload",
                provider,
                "provider cost exists without workload attribution",
                bucket.net_cost_usd,
                None,
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
    expected: Option<f64>,
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
