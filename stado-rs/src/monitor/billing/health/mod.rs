//! Account health: the record folded forward across ticks, and the alerting
//! it feeds.
//!
//! The time-unit ladder, the health record types and the two entry points
//! that read and advance the record — [`apply_health`] and [`commit_firing`]
//! — are here. [`fold`] carries one provider's history forward and formats
//! the elapsed figures, [`signals`] turns a document plus that history into
//! the conditions that are true right now, and [`alerting`] logs and
//! dispatches them.

mod alerting;
mod fold;
mod signals;

use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::config;

use fold::{fold_provider, health_value};
use signals::signals;

pub use alerting::dispatch_signals;
pub(super) use alerting::emit_alerts;
pub use fold::humanize;

// ---------------------------------------------------------------------------
// Account health
// ---------------------------------------------------------------------------

/// Time-unit ladder. The base unit is explicit; larger units are derived
/// from standard-library integer constants and prior entries in the ladder.
pub const SECONDS_PER_SECOND: u64 = true as u64;
/// `64 - 32/8 == 60`.
pub const SECONDS_PER_MINUTE: u64 = (u64::BITS - u32::BITS / u8::BITS) as u64;
/// `60 * 60 == 3600`.
pub const SECONDS_PER_HOUR: u64 = SECONDS_PER_MINUTE * SECONDS_PER_MINUTE;
/// `3600 * (32 - 8) == 86400`.
pub const SECONDS_PER_DAY: u64 = SECONDS_PER_HOUR * (u32::BITS - u8::BITS) as u64;

/// How long a provider section may report a non-`ok` status before it is
/// alerted on. One hour: long enough to ride out one failed collector tick
/// or a transient ARM/BigQuery 5xx, short enough that a closed account or a
/// disabled service principal is reported within the hour it breaks.
pub const HEALTH_GRACE_SECONDS: i64 = SECONDS_PER_HOUR as i64;

/// Key of the health record inside the billing document. It lives in the
/// same blob as the sections it describes, so the last-good timestamps
/// travel with the snapshot — and with `queue::copy`, whose
/// `CANONICAL_PREFIXES` already carries `billing_health/`.
pub const HEALTH_KEY: &str = "account_health";

/// Provider sections carried by the billing document, in catalog order.
pub fn providers() -> Vec<&'static str> {
    let enabled = config::billing_providers();
    crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Billing)
        .into_iter()
        .filter(|provider| {
            enabled
                .iter()
                .any(|configured| configured == provider.as_str())
        })
        .map(|provider| provider.as_str())
        .collect()
}

/// The one section status that means "this query actually succeeded".
const OK_STATUS: &str = "ok";
/// Status recorded for a provider key the document does not carry at all —
/// itself a defect worth alerting on, never a silent skip.
const MISSING_STATUS: &str = "missing";

/// Per-provider account health, folded forward across ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub provider: String,
    /// The section's own `status` (`ok`, `no_credentials`, `error`, ...).
    pub status: String,
    /// The section's own `detail`: the exact upstream cause, verbatim.
    pub detail: String,
    /// Last tick at which this section reported `ok`, RFC-3339.
    pub last_ok: Option<String>,
    /// First tick of the current non-`ok` run, RFC-3339.
    pub failing_since: Option<String>,
    /// Length of the current non-`ok` run, in seconds.
    pub failing_seconds: i64,
    /// Non-`ok` for longer than [`HEALTH_GRACE_SECONDS`].
    pub degraded: bool,
}

impl ProviderHealth {
    /// Whether the section reported a successful query this tick.
    pub fn healthy(&self) -> bool {
        self.status == OK_STATUS
    }
}

/// One alert condition. `key` is stable across ticks, so a condition that
/// stays true alerts on the transition into it rather than once per poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub key: String,
    pub subject: String,
    pub message: String,
}

/// Result of evaluating one billing document against the previous one.
#[derive(Debug, Clone, Default)]
pub struct HealthEvaluation {
    /// Health of every billing provider in catalog order.
    pub providers: Vec<ProviderHealth>,
    /// Every condition true right now.
    pub firing: Vec<Signal>,
    /// Conditions that were NOT firing at the previous tick.
    pub new_signals: Vec<Signal>,
    /// Keys that were firing at the previous tick and are not any more.
    pub cleared: Vec<String>,
}

/// Fold the previous snapshot's health record into `document`, write the
/// updated record under [`HEALTH_KEY`], and report what is firing.
///
/// The firing set carried into `document` is the PREVIOUS one, untouched:
/// only [`commit_firing`] advances it, and only alert-dispatching callers
/// may call that. A read-only republish (`stado billing refresh`) therefore
/// cannot swallow a transition the collector or `billing watch` still owes.
pub fn apply_health(
    previous: Option<&Value>,
    document: &mut Value,
    now: DateTime<Utc>,
) -> HealthEvaluation {
    let stamp = now.to_rfc3339_opts(SecondsFormat::Micros, false);
    let history = previous.map(|doc| &doc[HEALTH_KEY]);
    let previously_firing: BTreeSet<String> = history
        .and_then(|health| health.get("firing"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();

    let billing_providers = providers();
    let mut providers = Vec::with_capacity(billing_providers.len());
    let mut record = serde_json::Map::new();
    for provider in billing_providers {
        let prior = history
            .and_then(|health| health.get("providers"))
            .and_then(|map| map.get(provider));
        let health = fold_provider(provider, document.get(provider), prior, &stamp, now);
        record.insert(provider.to_string(), health_value(&health));
        providers.push(health);
    }
    document[HEALTH_KEY] = json!({
        "grace_seconds": HEALTH_GRACE_SECONDS,
        "providers": Value::Object(record),
        "firing": previously_firing.iter().collect::<Vec<_>>(),
    });

    let firing = signals(document, &providers);
    let live: BTreeSet<&str> = firing.iter().map(|signal| signal.key.as_str()).collect();
    let new_signals = firing
        .iter()
        .filter(|signal| !previously_firing.contains(&signal.key))
        .cloned()
        .collect();
    let cleared = previously_firing
        .iter()
        .filter(|key| !live.contains(key.as_str()))
        .cloned()
        .collect();
    HealthEvaluation {
        providers,
        firing,
        new_signals,
        cleared,
    }
}

/// Replace the document's firing-signal set with what is firing NOW. Call
/// this only from a path that also dispatches, never from a read-only one —
/// see [`apply_health`].
pub fn commit_firing(document: &mut Value, evaluation: &HealthEvaluation) {
    let keys: Vec<Value> = evaluation
        .firing
        .iter()
        .map(|signal| json!(signal.key))
        .collect();
    document[HEALTH_KEY]["firing"] = Value::Array(keys);
}
