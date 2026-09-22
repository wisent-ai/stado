//! Expenditure starts now; benefit starts when the option becomes usable.
use super::validate::timestamp;
use crate::fleet_expansion::constants::{CENTS_PER_USD, COMPARISON_TOLERANCE, DAYS_PER_MONTH};
use crate::fleet_expansion::model::{Candidate, ExpansionOption};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;

pub(crate) fn candidate(
    option: ExpansionOption,
    keys: &BTreeSet<String>,
    budget: f64,
    months: u32,
    now: DateTime<Utc>,
) -> Result<Candidate, String> {
    let mut reasons = Vec::new();
    if timestamp(&option.observed_at, "observed_at")? > now {
        reasons.push("evidence observation is in the future".into());
    }
    if timestamp(&option.valid_until, "valid_until")? <= now {
        reasons.push("evidence has expired".into());
    }
    for key in &option.need_keys {
        if !keys.contains(key) {
            reasons.push(format!(
                "need is not present in current fleet evidence: {key}"
            ));
        }
    }
    let values = [
        option.upfront_usd,
        option.monthly_cost_usd,
        option.monthly_savings_usd,
        option.monthly_margin_usd,
    ];
    for (name, value) in [
        "upfront_usd",
        "monthly_cost_usd",
        "monthly_savings_usd",
        "monthly_margin_usd",
    ]
    .iter()
    .zip(values)
    {
        if value.is_none() {
            reasons.push(format!("missing economic assumption: {name}"));
        }
    }
    let mut row = Candidate {
        option,
        status: "unknown".into(),
        reasons,
        committed_cost_usd: None,
        monthly_net_usd: None,
        payback_months: None,
        horizon_net_usd: None,
        roi_pct: None,
    };
    let [Some(upfront), Some(cost), Some(savings), Some(margin)] = values else {
        return Ok(row);
    };
    let lead = row.option.lead_time_days as f64 / DAYS_PER_MONTH;
    let benefit = savings + margin;
    let net = benefit - cost;
    let committed = upfront + cost * months as f64;
    let gain = benefit * (months as f64 - lead).max(0.0) - committed;
    row.committed_cost_usd = Some(committed);
    row.monthly_net_usd = Some(net);
    row.horizon_net_usd = Some(gain);
    row.payback_months = (net > 0.0).then(|| lead + (upfront + cost * lead) / net);
    row.roi_pct = (committed > 0.0).then(|| gain / committed * CENTS_PER_USD);
    if committed > budget + COMPARISON_TOLERANCE {
        row.reasons.push("total expenditure exceeds budget".into());
    }
    if net <= 0.0 {
        row.reasons
            .push("no finite payback: monthly benefit does not exceed operating cost".into());
    }
    if gain <= 0.0 {
        row.reasons
            .push("no positive cash gain within the selected horizon".into());
    }
    row.status = if row.reasons.is_empty() {
        "eligible"
    } else {
        "excluded"
    }
    .into();
    Ok(row)
}

/// Sum the time-ordered cashflows instead of averaging individual paybacks.
pub(crate) fn portfolio_payback(rows: &[&Candidate]) -> Option<f64> {
    if rows.is_empty() {
        return None;
    }
    let upfront: f64 = rows
        .iter()
        .map(|r| r.option.upfront_usd.unwrap_or_default())
        .sum();
    let monthly_cost: f64 = rows
        .iter()
        .map(|r| r.option.monthly_cost_usd.unwrap_or_default())
        .sum();
    let mut starts: Vec<(f64, f64)> = rows
        .iter()
        .map(|r| {
            (
                r.option.lead_time_days as f64 / DAYS_PER_MONTH,
                r.option.monthly_savings_usd.unwrap_or_default()
                    + r.option.monthly_margin_usd.unwrap_or_default(),
            )
        })
        .collect();
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut balance = -upfront;
    let mut slope = -monthly_cost;
    let mut time = f64::default();
    for (start, benefit) in starts {
        if slope > 0.0 && balance <= 0.0 {
            let crossing = time - balance / slope;
            if crossing <= start {
                return Some(crossing);
            }
        }
        balance += slope * (start - time);
        time = start;
        slope += benefit;
    }
    if slope > 0.0 {
        Some(time + (-balance).max(0.0) / slope)
    } else {
        None
    }
}
