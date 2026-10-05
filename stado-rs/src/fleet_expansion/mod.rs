//! Budgeted fleet expansion from live unmet needs and explicitly sourced estimates.
pub mod constants;
mod model;
mod plan;
mod render;
mod storage;

use plan::{economics, select, validate};

pub use model::{need_key, Catalog, CatalogRecord, ExpansionReport};
pub use render::render_report;
pub use storage::{history, read_catalog, read_plan, replace_catalog};

use crate::queue::JobStorage;
use crate::targets::Registry;
use chrono::Utc;
use constants::*;
use std::collections::BTreeSet;

pub async fn create_plan(
    store: &JobStorage,
    registry: &Registry,
    budget_usd: f64,
    horizon_months: u32,
    window_days: Option<i64>,
) -> Result<ExpansionReport, String> {
    let budget_cents = validate::money(budget_usd, "budget_usd")?;
    if horizon_months == 0 {
        return Err("horizon_months must be at least one month".into());
    }
    if window_days.is_some_and(|days| days < 1) {
        return Err("days must be at least 1".to_string());
    }
    let now = Utc::now();
    let catalog = read_catalog(store).await?;
    let needs = crate::fleet_needs::advise(store, registry, window_days, now)
        .await
        .map_err(|e| format!("read fleet needs for expansion: {e}"))?
        .needs;
    let keys: BTreeSet<String> = needs.iter().map(need_key).collect();
    let candidates = catalog
        .catalog
        .options
        .into_iter()
        .map(|option| economics::candidate(option, &keys, budget_usd, horizon_months, now))
        .collect::<Result<Vec<_>, _>>()?;
    let portfolio = select::select(&candidates, budget_cents);
    let mut warnings = vec![
        "Optimal only among supplied option bundles and declared estimates; not a market-wide purchasing recommendation.".into(),
        "Savings and margin are incremental declared estimates, not measured earnings. Time saved has no cash value unless explicitly evidenced. Avoided cloud spend must reflect usable credits.".into(),
        "Budget covers upfront plus operating expenditure for the entire horizon; savings do not finance purchases. No hardware is bought and no host is enrolled.".into(),
        "Needs are observations and refusal heuristics, not measured throughput gains; CPU labels can represent reservation limits, and occupied swap alone does not prove more RAM is needed.".into(),
    ];
    let mut uncovered = false;
    for key in &keys {
        if !candidates
            .iter()
            .any(|c| c.status == "eligible" && c.option.need_keys.contains(key))
        {
            warnings.push(format!("no eligible evidenced option for {key}"));
            uncovered = true;
        }
    }
    if catalog.version.is_none() {
        warnings
            .push("no expansion catalog has been recorded; prices and benefits are unknown".into());
    }
    let status = if needs.is_empty() {
        "no_needs"
    } else if !portfolio.selected_ids.is_empty() {
        if uncovered {
            "partial"
        } else {
            "ready"
        }
    } else if candidates.is_empty()
        || candidates
            .iter()
            .any(|c| c.status == "unknown" || c.reasons.iter().any(|r| r.starts_with("evidence")))
    {
        "insufficient_evidence"
    } else {
        "no_viable_option"
    };
    let report = ExpansionReport {
        schema_version: SCHEMA_VERSION,
        plan_id: uuid::Uuid::new_v4().to_string(),
        generated_at: now.to_rfc3339(),
        stado_version: env!("CARGO_PKG_VERSION").into(),
        catalog_version: catalog.version,
        budget_usd,
        horizon_months,
        window_days,
        status: status.into(),
        needs,
        candidates,
        portfolio,
        warnings,
    };
    storage::save_plan(store, &report).await?;
    Ok(report)
}
