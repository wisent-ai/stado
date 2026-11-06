//! What each source earned a host per board-hour, from what Stado keeps.
//!
//! A source a node earns from (a proving network, a mining pool, any network
//! that runs its own node) is valued at the payouts kept for it on the host
//! (`stado market payout record`, priced at each payout's own rate) divided
//! by the hours the host's nodes for that source ran, both over the same
//! window. Vast.ai is valued at the gross of the days `stado market earnings`
//! kept, divided by the hours those days cover. A source with no hours run,
//! or a payout without a rate, has no value: it is said, never zero.
//!
//! The window is the autonomy policy's `freshness.pricing_max_age_seconds`,
//! the age the operator lets a price be, as the board allocator reads it.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cli::CmdError;
use crate::providers::vast::SECONDS_PER_DAY;

/// One source's value on one host.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceValue {
    pub source: String,
    /// US dollars per board-hour over the window, when it can be read.
    pub usd_per_hour: Option<f64>,
    /// The hours the value is read over.
    pub hours: Option<f64>,
    /// Why there is no value, when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread: Option<String>,
}

/// The window the values are read over: the policy's
/// `freshness.pricing_max_age_seconds` back from now.
pub(crate) async fn window(
    store: &crate::queue::JobStorage,
) -> Result<(chrono::DateTime<chrono::Utc>, u64), String> {
    let policy = match crate::autonomy::storage::load_policy(store).await {
        Ok(Some(policy)) => policy,
        Ok(None) => {
            return Err("no autonomy policy is written, so no pricing window is declared".into())
        }
        Err(error) => return Err(format!("the autonomy policy cannot be read: {error}")),
    };
    let seconds = policy.freshness.pricing_max_age_seconds;
    let since = i64::try_from(seconds)
        .ok()
        .and_then(chrono::Duration::try_seconds)
        .and_then(|window| chrono::Utc::now().checked_sub_signed(window))
        .ok_or_else(|| {
            format!("freshness.pricing_max_age_seconds {seconds} reaches before any time")
        })?;
    Ok((since, seconds))
}

/// Every source's value on `host` (a registry target name) since `since`.
pub(crate) async fn values(
    store: &crate::queue::JobStorage,
    host: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<SourceValue>, CmdError> {
    use crate::providers::node;
    let read = |error: node::NodeError| CmdError::declaration(error.to_string());
    let mut segments: Vec<node::NodeSegment> = node::segments(store)
        .await
        .map_err(read)?
        .into_iter()
        .filter(|segment| segment.target == host)
        .collect();
    // A run still going counts up to now, as the segment it will be kept as.
    for declared in node::list(store).await.map_err(read)? {
        if declared.target != host {
            continue;
        }
        if let Some(from) = node::load_run(store, &declared.name)
            .await
            .map_err(read)?
            .filter(|run| run.running)
            .and_then(|run| run.running_since)
        {
            segments.push(node::NodeSegment {
                node: declared.name.clone(),
                source: declared.source.clone(),
                target: declared.target.clone(),
                from,
                to: chrono::Utc::now().to_rfc3339(),
            });
        }
    }
    let mut hours: BTreeMap<String, f64> = BTreeMap::new();
    for segment in &segments {
        let ran = segment.hours_since(since).map_err(CmdError::declaration)?;
        *hours.entry(segment.source.clone()).or_default() += ran;
    }
    let mut earned: BTreeMap<String, Result<f64, String>> = BTreeMap::new();
    for payout in super::payout::kept(store, Some(host)).await? {
        let paid = chrono::DateTime::parse_from_rfc3339(&payout.paid_at).map_err(|error| {
            CmdError::declaration(format!(
                "payout of {} at {:?} has an unreadable time: {error}",
                payout.source, payout.paid_at
            ))
        })?;
        if paid < since {
            continue;
        }
        let entry = earned.entry(payout.source.clone()).or_insert(Ok(0.0));
        match (entry.as_mut(), payout.usd()) {
            (Ok(sum), Ok(usd)) => *sum += usd,
            (Ok(_), Err(cause)) => {
                *entry = Err(format!(
                    "a payout of {} at {} is unpriced: {cause}",
                    payout.source, payout.paid_at
                ))
            }
            (Err(_), _) => {}
        }
    }
    let mut found = Vec::new();
    for source in hours
        .keys()
        .chain(earned.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let ran = hours.get(source).copied().unwrap_or(0.0);
        let value = match earned.get(source) {
            None => SourceValue {
                source: source.clone(),
                usd_per_hour: None,
                hours: Some(ran),
                unread: Some(format!(
                    "no payout of {source} is kept for {host} in the window"
                )),
            },
            Some(Err(cause)) => SourceValue {
                source: source.clone(),
                usd_per_hour: None,
                hours: Some(ran),
                unread: Some(cause.clone()),
            },
            Some(Ok(_)) if ran <= 0.0 => SourceValue {
                source: source.clone(),
                usd_per_hour: None,
                hours: Some(0.0),
                unread: Some(format!(
                    "no node of {source} ran on {host} in the window, so its payouts have no \
                     hours to divide by"
                )),
            },
            Some(Ok(usd)) => SourceValue {
                source: source.clone(),
                usd_per_hour: Some(usd / ran),
                hours: Some(ran),
                unread: None,
            },
        };
        found.push(value);
    }
    found.push(vast_value(store, host, since).await?);
    Ok(found)
}

/// Vast.ai's value: the gross of the kept days in the window over their hours.
async fn vast_value(
    store: &crate::queue::JobStorage,
    host: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<SourceValue, CmdError> {
    let first = since.timestamp().div_euclid(SECONDS_PER_DAY);
    let today = chrono::Utc::now().timestamp().div_euclid(SECONDS_PER_DAY);
    let mut gross = 0.0;
    let mut days = 0u32;
    for day in first..=today {
        let Some(text) = store
            .download_text(&super::earnings::ledger_key(host, day))
            .await?
        else {
            continue;
        };
        let record: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            CmdError::declaration(format!(
                "{} is not a kept earnings day: {error}",
                super::earnings::ledger_key(host, day)
            ))
        })?;
        if let Some(day_gross) = record["gross"].as_f64() {
            gross += day_gross;
            days += 1;
        }
    }
    Ok(match days {
        0 => SourceValue {
            source: "vast".to_string(),
            usd_per_hour: None,
            hours: None,
            unread: Some(format!(
                "no Vast.ai earnings day of {host} is kept in the window; `stado market earnings \
                 --provider vast --target {host}` keeps them"
            )),
        },
        days => SourceValue {
            source: "vast".to_string(),
            usd_per_hour: Some(gross / (f64::from(days) * 24.0)),
            hours: Some(f64::from(days) * 24.0),
            unread: None,
        },
    })
}

/// Whether `source` may keep its node running on `host`: its value is read
/// and no other source's read value is higher. `Err` names why not.
pub(crate) fn may_run(values: &[SourceValue], source: &str) -> Result<(), String> {
    let own = values
        .iter()
        .find(|value| value.source == source)
        .and_then(|value| value.usd_per_hour);
    let best_other = values
        .iter()
        .filter(|value| value.source != source)
        .filter_map(|value| value.usd_per_hour.map(|usd| (value, usd)))
        .max_by(|left, right| left.1.total_cmp(&right.1));
    match (own, best_other) {
        (Some(own), Some((other, best))) if best > own => Err(format!(
            "{source} earns {own:.4} USD per board-hour and {} earns {best:.4}",
            other.source
        )),
        _ => Ok(()),
    }
}
