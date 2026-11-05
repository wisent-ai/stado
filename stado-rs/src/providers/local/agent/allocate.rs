//! Which use a board is given: the one whose measured value per board-hour
//! is largest, never a rule that ignores money.
//!
//! The candidates for one board are each priced in US dollars per
//! board-hour from what Stado measured or was told by whoever pays:
//!
//! - a queued GPU job: the income its submitter stated for it (`stado
//!   submit --income-usd-per-hour X --income-source REF`), else the
//!   operator's declared value of internal work per GPU-hour
//!   (`budgets.internal_usd_per_gpu_hour` in the autonomy policy); a job
//!   with neither is refused as a candidate, by name, never valued at zero;
//! - a Yieldable inference deployment serving here: what outside callers
//!   paid for it over the measured hours (the gateway's priced usage and
//!   paid calls), per hour; a deployment no caller paid for in that window
//!   is valued at zero, since that is what it earned;
//! - a market offer for the board, at the hourly price it is offered at.
//!
//! The board's own cost per hour (energy above idle at the tariff) is the
//! same for every use, so the margins order as the values do; it is kept on
//! the decision for the reader. The board goes to the largest value; a tie
//! keeps the use it has, so equal offers do not flap it. Exclusive
//! deployments are the operator's own choice and are never a candidate.
//! Every decision is kept with its inputs under `market/decisions/<host>/`,
//! and `stado market decisions` reads them.

use serde::{Deserialize, Serialize};

/// What a candidate is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Use {
    Job { job_id: String },
    Deployment { name: String },
    Offer { provider: String },
}

impl Use {
    pub fn said(&self) -> String {
        match self {
            Use::Job { job_id } => format!("job {job_id}"),
            Use::Deployment { name } => format!("deployment {name}"),
            Use::Offer { provider } => format!("{provider} offer"),
        }
    }
}

/// One candidate for the board, valued or refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    #[serde(flatten)]
    pub what: Use,
    /// US dollars per board-hour, when measured or stated.
    pub usd_per_hour: Option<f64>,
    /// Where the value was read.
    pub source: String,
    /// Why it has no value, when it has none: such a candidate is never
    /// chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

impl Candidate {
    pub fn valued(what: Use, usd_per_hour: f64, source: impl Into<String>) -> Self {
        Self {
            what,
            usd_per_hour: Some(usd_per_hour),
            source: source.into(),
            refused: None,
        }
    }

    pub fn refused(what: Use, why: impl Into<String>) -> Self {
        Self {
            what,
            usd_per_hour: None,
            source: String::new(),
            refused: Some(why.into()),
        }
    }
}

/// The board's use after one decision, with every input it weighed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub host: String,
    pub board: String,
    pub at: String,
    pub candidates: Vec<Candidate>,
    /// The use the board had before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<Use>,
    /// The use it is given; none when no candidate has a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Use>,
    /// The board's cost per hour, when it is measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd_per_hour: Option<f64>,
    pub reason: String,
}

/// Decide the board's use among `candidates`. `holding` keeps the board on
/// a tie, so two equal values never move it back and forth.
pub fn decide(
    host: &str,
    board: &str,
    holding: Option<Use>,
    candidates: Vec<Candidate>,
    cost_usd_per_hour: Option<f64>,
) -> Decision {
    // The highest value wins; on a tie the use the board already has stays.
    let mut best: Option<(&Candidate, f64)> = None;
    for candidate in candidates
        .iter()
        .filter(|candidate| candidate.refused.is_none())
    {
        let Some(value) = candidate.usd_per_hour else {
            continue;
        };
        let better = match best {
            None => true,
            Some((_, kept)) if value > kept => true,
            Some((_, kept)) => value == kept && holding.as_ref() == Some(&candidate.what),
        };
        if better {
            best = Some((candidate, value));
        }
    }
    let (chosen, reason) = match best {
        Some((candidate, value)) => (
            Some(candidate.what.clone()),
            format!(
                "{} is worth {value:.4} USD per board-hour ({}), the most of {} valued candidate(s)",
                candidate.what.said(),
                candidate.source,
                candidates.iter().filter(|candidate| candidate.usd_per_hour.is_some()).count()
            ),
        ),
        None => (
            None,
            "no candidate has a measured or stated value, so the board's use is not changed"
                .to_string(),
        ),
    };
    Decision {
        host: host.to_string(),
        board: board.to_string(),
        at: chrono::Utc::now().to_rfc3339(),
        candidates,
        holding,
        chosen,
        cost_usd_per_hour,
        reason,
    }
}

/// Where decisions of `host` are kept.
pub fn prefix(host: &str) -> String {
    format!("market/decisions/{host}/")
}

/// Keep `decision`, one object per decision.
pub async fn keep(
    store: &crate::queue::JobStorage,
    decision: &Decision,
) -> Result<(), crate::queue::StorageError> {
    let key = format!(
        "{}{}-{}.json",
        prefix(&decision.host),
        decision.at.replace(':', ""),
        decision.board
    );
    let body = serde_json::to_string(decision).map_err(|error| {
        crate::queue::StorageError::Other(format!("a decision cannot be encoded: {error}"))
    })?;
    store.upload_text(&key, &body).await
}

/// Every kept decision of `host` at or after `since` (RFC 3339), oldest
/// first.
pub async fn kept(
    store: &crate::queue::JobStorage,
    host: &str,
    since: &str,
) -> Result<Vec<Decision>, crate::queue::StorageError> {
    let mut found = Vec::new();
    for key in store
        .list_paths(&prefix(host), crate::queue::Listing::Every)
        .await?
    {
        let Some(text) = store.download_text(&key).await? else {
            continue;
        };
        let decision: Decision = serde_json::from_str(&text).map_err(|error| {
            crate::queue::StorageError::Other(format!("{key} is not a kept decision: {error}"))
        })?;
        if decision.at.as_str() >= since {
            found.push(decision);
        }
    }
    found.sort_by(|left, right| left.at.cmp(&right.at));
    Ok(found)
}

/// What a deployment earned per hour from outside callers over the kept
/// hours since `since`: priced caller usage plus paid calls, divided by the
/// hours the window covers. A caller without a price makes the value
/// unread, named, since its tokens were sold at a price nobody stated.
pub async fn deployment_value(
    store: &crate::queue::JobStorage,
    deployment: &str,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<Result<f64, String>, crate::queue::StorageError> {
    use crate::inference::{callers, pay_per_call};
    let hours = (chrono::Utc::now() - since).num_seconds() as f64 / 3600.0;
    if hours <= 0.0 {
        return Ok(Err("the window to value it over is empty".to_string()));
    }
    let declared = callers::list(store, deployment).await?;
    let mut usd = 0.0;
    for hour in callers::usage(store, deployment, None, &callers::hour_of(since)).await? {
        if hour.tokens() == 0 {
            continue;
        }
        match declared
            .iter()
            .find(|caller| caller.caller == hour.caller)
            .and_then(|caller| caller.price)
        {
            Some(price) => usd += hour.charge(&price),
            None => {
                return Ok(Err(format!(
                    "caller {} of {deployment} used {} tokens with no price declared",
                    hour.caller,
                    hour.tokens()
                )))
            }
        }
    }
    for settled in pay_per_call::settlements(store, deployment, &since.to_rfc3339()).await? {
        match settled.earned_usd() {
            Some(earned) => usd += earned,
            None => {
                return Ok(Err(format!(
                    "paid call {} of {deployment} stated no usage, so what it earned is unread",
                    settled.quote
                )))
            }
        }
    }
    Ok(Ok(usd / hours))
}
