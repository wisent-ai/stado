//! Whether a host answered at all, when it last spoke, and what powers it.
//!
//! A host that sleeps or leaves the network stops answering and stops
//! publishing at once, and the other readings then only say what is
//! missing. This says it in one place: the host's own state read answered
//! or did not, the last capacity publication it made and how long ago, and
//! the power source that publication names (`mains`, `battery`), so work is
//! not placed on a laptop running down its battery.

use serde_json::{json, Value};

use crate::deploy::host_gates::gates::HostGates;

/// A host's presence, as one gates verdict read it.
pub struct Presence<'a> {
    /// Whether the host answered this command's own state read.
    pub answered: bool,
    pub last_seen_at: Option<&'a str>,
    pub last_seen_age_seconds: Option<i64>,
    pub power_source: Option<&'a str>,
    pub battery_percent: Option<i64>,
}

impl<'a> Presence<'a> {
    pub fn of(gates: &'a HostGates) -> Self {
        let diagnostic = |key: &str| {
            gates
                .published_diagnostics
                .as_ref()
                .and_then(|diag| diag.get(key))
        };
        Self {
            answered: gates
                .observations
                .iter()
                .any(|read| read.operation == "host_state" && read.complete()),
            last_seen_at: gates.published_at.as_deref(),
            last_seen_age_seconds: gates.age_seconds,
            power_source: diagnostic("power_source").and_then(Value::as_str),
            battery_percent: diagnostic("battery_percent").and_then(Value::as_i64),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "answered": self.answered,
            "last_seen_at": self.last_seen_at,
            "last_seen_age_seconds": self.last_seen_age_seconds,
            "power_source": self.power_source,
            "battery_percent": self.battery_percent,
        })
    }

    /// The presence line a person reads.
    pub fn line(&self) -> String {
        let seen = match (self.last_seen_at, self.last_seen_age_seconds) {
            (Some(at), Some(age)) => format!("last published {at} ({age} s ago)"),
            (Some(at), None) => format!("last published {at}"),
            _ => "never published".to_string(),
        };
        let power = match (self.power_source, self.battery_percent) {
            (Some(source), Some(percent)) => format!("on {source}, battery {percent}%"),
            (Some(source), None) => format!("on {source}"),
            (None, _) => "power not reported".to_string(),
        };
        if self.answered {
            format!("answering; {seen}; {power}")
        } else {
            format!("not answering (asleep, off or unreachable); {seen}; {power}")
        }
    }
}
