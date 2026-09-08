//! Autonomy pass — inventory, economic placement, reconciliation, advice,
//! cost reports and lifecycle enforcement, run once per tick.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::providers::Provider;
use crate::queue::{JobStorage, StorageError};

use super::providers::ResolvedProvider;

pub(crate) async fn run_autonomy_once(
    store: &JobStorage,
    providers: &[ResolvedProvider],
    mut policy: crate::autonomy::AutonomyPolicy,
    log: &dyn Fn(&str),
) -> Result<(), StorageError> {
    let now = Utc::now();
    let control = crate::autonomy::storage::load_control(store).await?;
    let circuit_open = control.circuit_open_at(now);
    if circuit_open {
        log(&format!(
            "autonomy circuit breaker open until {} after {} consecutive mutation failures: {}",
            control.circuit_open_until.as_deref().unwrap_or("unknown"),
            control.consecutive_mutation_failures,
            control.last_mutation_error.as_deref().unwrap_or("unknown"),
        ));
    }
    policy.emergency_paused |= control.emergency_paused || circuit_open;
    let prices_path = "state/autonomy/cost/prices.json";
    let prices: crate::autonomy::cost::PriceBook = match crate::autonomy::storage::read_json::<
        crate::autonomy::cost::PriceBook,
    >(store, prices_path)
    .await?
    {
        Some(book)
            if timestamp_fresh(
                &book.created_at,
                policy.freshness.pricing_max_age_seconds,
                now,
            ) =>
        {
            book
        }
        _ => crate::autonomy::cost::refresh_prices(&policy).await,
    };
    let cached = crate::autonomy::storage::load_latest_inventory(store).await?;
    let mut inventory = match cached {
        Some(snapshot)
            if policy.mode == crate::autonomy::AutonomyMode::Report
                && timestamp_fresh(
                    &snapshot.created_at,
                    policy.freshness.inventory_max_age_seconds,
                    now,
                ) =>
        {
            snapshot
        }
        _ => crate::autonomy::inventory::collect(store).await?,
    };
    let prior_snapshot_id = inventory.snapshot_id.clone();
    crate::autonomy::cost::enrich_inventory(&mut inventory, &prices);
    crate::autonomy::inventory::reseal(&mut inventory)?;
    if inventory.snapshot_id != prior_snapshot_id
        || crate::autonomy::storage::load_latest_inventory(store)
            .await?
            .is_none_or(|latest| latest.snapshot_id != inventory.snapshot_id)
    {
        crate::autonomy::storage::publish_inventory(store, &inventory).await?;
    }
    let budget_allocation = crate::autonomy::cost::build_allocation(store, &inventory).await?;
    let budget_billing = crate::autonomy::cost::load_billing_snapshot(store).await?;
    let budget_forecast =
        crate::autonomy::cost::forecast(&budget_allocation, &policy, budget_billing.as_ref(), now);
    let new_cloud_allowed = !budget_forecast.budget_exceeded;
    let hours_per_day = (crate::monitor::billing::SECONDS_PER_DAY
        / crate::monitor::billing::SECONDS_PER_HOUR) as f64;
    let daily_hourly_limit = policy.budgets.daily_usd.map(|limit| limit / hours_per_day);
    let hourly_limit = match (policy.budgets.hourly_usd, daily_hourly_limit) {
        (Some(hourly), Some(daily)) => Some(hourly.min(daily)),
        (Some(hourly), None) => Some(hourly),
        (None, Some(daily)) => Some(daily),
        (None, None) => None,
    };
    let mut new_cloud_hourly_budget_usd =
        hourly_limit.map(|limit| (limit - budget_forecast.current_hourly_usd).max(f64::default()));
    let mut new_cloud_cost_budget_usd = policy
        .budgets
        .monthly_usd
        .map(|limit| (limit - budget_forecast.end_of_month_usd).max(f64::default()));
    if !new_cloud_allowed || policy.mode != crate::autonomy::AutonomyMode::EnforceOwned {
        new_cloud_hourly_budget_usd = Some(f64::default());
        new_cloud_cost_budget_usd = Some(f64::default());
    }
    if !new_cloud_allowed {
        log(&format!(
            "autonomy budget guard: hourly overrun ${:.2}, daily overrun ${:.2}, monthly overrun ${:.2}; new cloud placement blocked",
            budget_forecast.hourly_overrun_usd,
            budget_forecast.daily_overrun_usd,
            budget_forecast.projected_overrun_usd,
        ));
    }

    if policy.mode != crate::autonomy::AutonomyMode::Report && inventory.complete {
        let cloud: Vec<(String, Arc<dyn Provider>)> = providers
            .iter()
            .filter_map(|provider| match provider {
                ResolvedProvider::Cloud { name, provider } => {
                    Some((name.clone(), Arc::clone(provider)))
                }
                ResolvedProvider::Box { .. } => None,
            })
            .collect();
        let placement = crate::autonomy::optimizer::plan_queued(
            store,
            &cloud,
            &policy,
            &prices,
            &inventory.snapshot_id,
            log,
            new_cloud_hourly_budget_usd,
            new_cloud_cost_budget_usd,
        )
        .await?;
        log(&format!(
            "autonomy placement: considered={} decided={} changed={} blocked={}",
            placement.considered_jobs,
            placement.decided_jobs,
            placement.changed_jobs,
            placement.no_eligible_target
        ));
    } else if policy.mode != crate::autonomy::AutonomyMode::Report {
        log("autonomy placement blocked: inventory is incomplete");
    }

    let fingerprint = crate::cli::resources::planner::configuration_fingerprint()
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let reconciliation =
        crate::autonomy::reconciler::reconcile(store, &inventory, &policy, &fingerprint, log)
            .await?;
    if reconciliation.findings > usize::default() {
        log(&format!(
            "autonomy reconciliation: findings={} actions={} executed={}",
            reconciliation.findings, reconciliation.automatic_actions, reconciliation.executed
        ));
    }
    crate::autonomy::service_reconciler::reconcile(store, &policy, log).await?;
    let advice =
        crate::autonomy::advisor::publish_recommendations(store, &inventory, &policy, now).await?;
    if advice.rightsizing > usize::default()
        || advice.schedules > usize::default()
        || advice.storage_lifecycle > usize::default()
        || advice.network > usize::default()
        || advice.commitments > usize::default()
    {
        log(&format!(
            "autonomy advice: rightsizing={} schedules={} storage={} network={} commitments={}",
            advice.rightsizing,
            advice.schedules,
            advice.storage_lifecycle,
            advice.network,
            advice.commitments
        ));
    }

    let allocation = crate::autonomy::cost::build_allocation(store, &inventory).await?;
    let billing = crate::autonomy::cost::load_billing_snapshot(store).await?;
    let forecast = crate::autonomy::cost::forecast(&allocation, &policy, billing.as_ref(), now);
    let anomalies = crate::autonomy::cost::detect_anomalies(&allocation, &inventory, &forecast);
    crate::autonomy::cost::persist_reports(store, &prices, &allocation, &forecast, &anomalies)
        .await?;
    let outcomes = crate::autonomy::cost::measure_outcomes(store, &policy, now).await?;
    if outcomes.feedback_written > usize::default() || outcomes.savings_measured > usize::default()
    {
        log(&format!(
            "autonomy outcomes: feedback={} savings-measured={}",
            outcomes.feedback_written, outcomes.savings_measured
        ));
    }
    let savings = crate::autonomy::storage::list_savings(store).await?;
    let measurements = crate::autonomy::storage::list_savings_measurements(store).await?;
    let savings_summary =
        crate::autonomy::cost::summarize_savings_with_measurements(&savings, &measurements);
    crate::autonomy::storage::write_json(
        store,
        "state/autonomy/cost/savings.json",
        &savings_summary,
        false,
    )
    .await?;
    let lifecycle = crate::autonomy::lifecycle::enforce(store, &policy, now).await?;
    if lifecycle.deleted > usize::default() {
        log(&format!(
            "autonomy lifecycle: deleted={} bytes={} capped={}",
            lifecycle.deleted, lifecycle.deleted_bytes, lifecycle.capped
        ));
    }
    Ok(())
}

fn timestamp_fresh(raw: &str, max_age_seconds: u64, now: chrono::DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(raw)
        .map(|stamp| {
            now.signed_duration_since(stamp.with_timezone(&Utc))
                .num_seconds()
        })
        .is_ok_and(|age| {
            age >= i64::default() && age <= i64::try_from(max_age_seconds).unwrap_or(i64::MAX)
        })
}
