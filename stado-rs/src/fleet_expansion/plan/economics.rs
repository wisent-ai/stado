//! Expenditure starts now; benefit starts when the option becomes usable.
use super::validate::timestamp;
use crate::fleet_expansion::constants::{CENTS_PER_USD, DAYS_PER_MONTH, PER_CENT};
use crate::fleet_expansion::model::{Candidate, CandidateStatus, ExpansionOption};
use chrono::{DateTime, Utc};
use std::collections::BTreeSet;

/// Whether an amount is above zero: a sum gained, a net that pays back.
pub(crate) fn positive(amount: f64) -> bool {
    amount > -amount
}

/// A dollar amount as the whole cents money is kept and compared in, so two
/// amounts are equal exactly when they are the same number of cents.
pub(crate) fn cents(usd: f64) -> i64 {
    (usd * CENTS_PER_USD).round() as i64
}

/// One option's stated assumptions and what they come to over the horizon.
/// It exists only for an option that states all four assumptions, so no
/// figure here stands in for one the option left out.
pub(crate) struct Figures {
    pub(crate) upfront: f64,
    pub(crate) monthly_cost: f64,
    /// Savings plus margin, per month, once the option is usable.
    pub(crate) benefit: f64,
    /// Months from now until the option is usable.
    pub(crate) lead: f64,
    pub(crate) committed: f64,
    pub(crate) net: f64,
    pub(crate) gain: f64,
}

impl Figures {
    /// The option priced over `months`, or `None` when it leaves an
    /// assumption out.
    fn of(option: &ExpansionOption, months: u32) -> Option<Self> {
        let upfront = option.upfront_usd?;
        let monthly_cost = option.monthly_cost_usd?;
        let benefit = option.monthly_savings_usd? + option.monthly_margin_usd?;
        let lead = option.lead_time_days as f64 / DAYS_PER_MONTH;
        let committed = upfront + monthly_cost * months as f64;
        let usable_months = months as f64 - lead;
        // An option usable only after the horizon earns nothing within it.
        let gain = if positive(usable_months) {
            benefit * usable_months - committed
        } else {
            -committed
        };
        Some(Self {
            upfront,
            monthly_cost,
            benefit,
            lead,
            committed,
            net: benefit - monthly_cost,
            gain,
        })
    }
}

/// An eligible row and its figures.
pub(crate) struct Priced<'a> {
    pub(crate) row: &'a Candidate,
    pub(crate) figures: Figures,
}

/// A row priced from its option; `None` for a row whose option leaves an
/// assumption out, which [`candidate`] never marks eligible.
pub(crate) fn priced(row: &Candidate, months: u32) -> Option<Priced<'_>> {
    Some(Priced {
        row,
        figures: Figures::of(&row.option, months)?,
    })
}

pub(crate) fn candidate(
    option: ExpansionOption,
    keys: &BTreeSet<String>,
    budget: f64,
    months: u32,
    now: DateTime<Utc>,
) -> Result<Candidate, String> {
    let mut reasons = Vec::new();
    let future = timestamp(&option.observed_at, "observed_at")? > now;
    if future {
        reasons.push("evidence observation is in the future".into());
    }
    let expired = timestamp(&option.valid_until, "valid_until")? <= now;
    if expired {
        reasons.push("evidence has expired".into());
    }
    let unevidenced = future || expired;
    for key in &option.need_keys {
        if !keys.contains(key) {
            reasons.push(format!(
                "need is not present in current fleet evidence: {key}"
            ));
        }
    }
    for (name, value) in [
        ("upfront_usd", option.upfront_usd),
        ("monthly_cost_usd", option.monthly_cost_usd),
        ("monthly_savings_usd", option.monthly_savings_usd),
        ("monthly_margin_usd", option.monthly_margin_usd),
    ] {
        if value.is_none() {
            reasons.push(format!("missing economic assumption: {name}"));
        }
    }
    let figures = Figures::of(&option, months);
    let mut row = Candidate {
        option,
        status: if unevidenced {
            CandidateStatus::Unevidenced
        } else {
            CandidateStatus::Unknown
        },
        reasons,
        committed_cost_usd: None,
        monthly_net_usd: None,
        payback_months: None,
        horizon_net_usd: None,
        roi_pct: None,
    };
    let Some(figures) = figures else {
        return Ok(row);
    };
    let Figures { upfront, monthly_cost, lead, committed, net, gain, .. } = figures;
    row.committed_cost_usd = Some(committed);
    row.monthly_net_usd = Some(net);
    row.horizon_net_usd = Some(gain);
    row.payback_months = positive(net).then(|| lead + (upfront + monthly_cost * lead) / net);
    row.roi_pct = positive(committed).then(|| gain / committed * PER_CENT);
    if cents(committed) > cents(budget) {
        row.reasons.push("total expenditure exceeds budget".into());
    }
    if !positive(net) {
        row.reasons
            .push("no finite payback: monthly benefit does not exceed operating cost".into());
    }
    if !positive(gain) {
        row.reasons
            .push("no positive cash gain within the selected horizon".into());
    }
    row.status = if unevidenced {
        CandidateStatus::Unevidenced
    } else if row.reasons.is_empty() {
        CandidateStatus::Eligible
    } else {
        CandidateStatus::Excluded
    };
    Ok(row)
}

/// Sum the time-ordered cashflows instead of averaging individual paybacks.
/// Time is months from now; `None` for no selection or none that pays back.
pub(crate) fn portfolio_payback(rows: &[&Priced<'_>]) -> Option<f64> {
    if rows.is_empty() {
        return None;
    }
    let upfront: f64 = rows.iter().map(|r| r.figures.upfront).sum();
    let monthly_cost: f64 = rows.iter().map(|r| r.figures.monthly_cost).sum();
    let mut starts: Vec<(f64, f64)> = rows
        .iter()
        .map(|r| (r.figures.lead, r.figures.benefit))
        .collect();
    starts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut balance = -upfront;
    let mut slope = -monthly_cost;
    // The months elapsed when the last benefit began; none has begun yet.
    let mut time: Option<f64> = None;
    for (start, benefit) in starts {
        if positive(slope) && !positive(balance) {
            let crossing = match time {
                Some(time) => time - balance / slope,
                None => -balance / slope,
            };
            if crossing <= start {
                return Some(crossing);
            }
        }
        let elapsed = match time {
            Some(time) => start - time,
            None => start,
        };
        balance += slope * elapsed;
        time = Some(start);
        slope += benefit;
    }
    if !positive(slope) {
        return None;
    }
    // A balance already above water owes nothing more after the last start.
    let owed = if positive(balance) { None } else { Some(-balance / slope) };
    match (time, owed) {
        (Some(time), Some(owed)) => Some(time + owed),
        (Some(time), None) => Some(time),
        (None, owed) => owed,
    }
}
