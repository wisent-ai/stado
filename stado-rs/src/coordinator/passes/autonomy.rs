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
        // Each part the control record lacks is said as missing, so the line
        // never reads as a breaker open until an instant called "unknown".
        let until = match control.circuit_open_until.as_deref() {
            Some(until) => format!("until {until}"),
            None => "with no recorded end".to_string(),
        };
        let cause = match control.last_mutation_error.as_deref() {
            Some(error) => error.to_string(),
            None => "the control record names no last error".to_string(),
        };
        log(&format!(
            "autonomy circuit breaker open {until} after {} consecutive mutation failures: {cause}",
            control.mutation_failures.len(),
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
    // A monthly budget is judged against the month's billed spend; while the
    // billing snapshot does not give it, the month-end projection is unknown
    // and the budget cannot be shown to hold, so no new cloud is placed.
    let monthly_unverified = match (
        policy.budgets.monthly_usd,
        &budget_forecast.month_spent_unknown,
    ) {
        (Some(_declared), Some(cause)) => Some(cause.clone()),
        _ => None,
    };
    let new_cloud_allowed = !budget_forecast.budget_exceeded && monthly_unverified.is_none();
    let hours_per_day = (crate::monitor::billing::SECONDS_PER_DAY
        / crate::monitor::billing::SECONDS_PER_HOUR) as f64;
    let daily_hourly_limit = policy.budgets.daily_usd.map(|limit| limit / hours_per_day);
    let hourly_limit = match (policy.budgets.hourly_usd, daily_hourly_limit) {
        (Some(hourly), Some(daily)) => Some(hourly.min(daily)),
        (Some(hourly), None) => Some(hourly),
        (None, Some(daily)) => Some(daily),
        (None, None) => None,
    };
    // While placement is allowed no budget is exceeded, so each remainder
    // below is what is left of its limit.
    let mut new_cloud_hourly_budget_usd =
        hourly_limit.map(|limit| limit - budget_forecast.current_hourly_usd);
    let mut new_cloud_cost_budget_usd = policy
        .budgets
        .monthly_usd
        .zip(budget_forecast.end_of_month_usd)
        .map(|(limit, projected)| limit - projected);
    if !new_cloud_allowed || policy.mode != crate::autonomy::AutonomyMode::EnforceOwned {
        // No remainder admits a new cloud offer at any price.
        new_cloud_hourly_budget_usd = Some(f64::NEG_INFINITY);
        new_cloud_cost_budget_usd = Some(f64::NEG_INFINITY);
    }
    let overrun = |figure: Option<f64>| match figure {
        Some(overrun) => format!("${overrun:.2}"),
        None => "none".to_string(),
    };
    if budget_forecast.budget_exceeded {
        log(&format!(
            "autonomy budget guard: hourly overrun {}, daily overrun {}, monthly overrun {}; new cloud placement blocked",
            overrun(budget_forecast.hourly_overrun_usd),
            overrun(budget_forecast.daily_overrun_usd),
            overrun(budget_forecast.projected_overrun_usd),
        ));
    }
    if let Some(cause) = monthly_unverified {
        log(&format!(
            "autonomy budget guard: budgets.monthly_usd is declared but this month's billed spend is unknown ({cause}); new cloud placement blocked"
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
            placement.considered_jobs.len(),
            placement.decided_jobs.len(),
            placement.changed_jobs.len(),
            placement.no_eligible_target.len()
        ));
    } else if policy.mode != crate::autonomy::AutonomyMode::Report {
        log("autonomy placement blocked: inventory is incomplete");
    }

    let fingerprint = crate::cli::resources::planner::configuration_fingerprint()
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let reconciliation =
        crate::autonomy::reconciler::reconcile(store, &inventory, &policy, &fingerprint, log)
            .await?;
    if reconciliation.operation_id.is_some() || reconciliation.executed {
        log(&format!(
            "autonomy reconciliation: findings={} actions={} executed={}",
            reconciliation.findings, reconciliation.automatic_actions, reconciliation.executed
        ));
    }
    crate::autonomy::service_reconciler::reconcile(store, &policy, log).await?;
    let advice =
        crate::autonomy::advisor::publish_recommendations(store, &inventory, &policy, now).await?;
    if !advice.is_empty() {
        log(&format!(
            "autonomy advice: rightsizing={:?} schedules={:?} storage={:?} network={:?} commitments={:?}",
            advice.rightsizing,
            advice.schedules,
            advice.storage_lifecycle,
            advice.network,
            advice.commitments
        ));
    }
    for why in &advice.unjudged {
        log(&format!("autonomy advice unjudged: {why}"));
    }

    let allocation = crate::autonomy::cost::build_allocation(store, &inventory).await?;
    let billing = crate::autonomy::cost::load_billing_snapshot(store).await?;
    let forecast = crate::autonomy::cost::forecast(&allocation, &policy, billing.as_ref(), now);
    let anomalies = crate::autonomy::cost::detect_anomalies(&allocation, &inventory, &forecast);
    crate::autonomy::cost::persist_reports(store, &prices, &allocation, &forecast, &anomalies)
        .await?;
    let outcomes = crate::autonomy::cost::measure_outcomes(store, &policy, now).await?;
    if !outcomes.is_empty() {
        log(&format!(
            "autonomy outcomes: feedback={:?} savings-measured={:?} feedback-withheld-no-target={:?}",
            outcomes.feedback_written, outcomes.savings_measured, outcomes.feedback_unattributed
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
    let lifecycle = crate::autonomy::lifecycle::enforce(store, &policy, now, log).await?;
    if lifecycle != crate::autonomy::lifecycle::LifecycleSummary::default() {
        log(&format!("autonomy lifecycle: {lifecycle:?}"));
    }
    Ok(())
}

fn timestamp_fresh(raw: &str, max_age_seconds: u64, now: chrono::DateTime<Utc>) -> bool {
    let Ok(stamp) = DateTime::parse_from_rfc3339(raw) else {
        return false;
    };
    // A stamp from the future has no age to call fresh.
    u64::try_from(
        now.signed_duration_since(stamp.with_timezone(&Utc))
            .num_seconds(),
    )
    .is_ok_and(|age| age <= max_age_seconds)
}
