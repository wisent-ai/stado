//! One `host_health/<slug>.json` object, and every question the two
//! liveness commands ask of it.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::cli::registry::beacons::SCHEDULED_STATE;

/// One `host_health/<slug>.json` object.
pub(in crate::cli::registry) struct Beacon {
    /// Store-relative object name.
    pub(in crate::cli::registry) path: String,
    /// Object mtime: the authority for age, since the body's `reported_at`
    /// is stamped by the reporting host's own clock.
    pub(super) updated: Option<DateTime<Utc>>,
    /// Parsed beacon body; `None` when the object is unparsable or is not
    /// a JSON object.
    pub(super) body: Option<Map<String, Value>>,
}

impl Beacon {
    /// `reported_at` as the host stamped it.
    pub(super) fn reported_at(&self) -> Option<&str> {
        self.body.as_ref()?.get("reported_at")?.as_str()
    }

    /// When this beacon was last known good: the object mtime, falling
    /// back to the host's own `reported_at` for backends that carry no
    /// mtime.
    pub(in crate::cli::registry) fn observed_at(&self) -> Option<DateTime<Utc>> {
        self.updated.or_else(|| {
            DateTime::parse_from_rfc3339(self.reported_at()?)
                .ok()
                .map(|ts| ts.with_timezone(&Utc))
        })
    }

    /// State of one unit in the beacon's `units` map. Values are either
    /// `{"state": ...}` objects or a bare string, exactly as
    /// `monitor::host_health::format_host_health` reads them.
    pub(in crate::cli::registry) fn unit_state(&self, unit: &str) -> Option<String> {
        let value = self.body.as_ref()?.get("units")?.as_object()?.get(unit)?;
        match value {
            Value::Object(state) => Some(
                state
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ),
            Value::String(state) => Some(state.clone()),
            other => Some(other.to_string()),
        }
    }

    /// Whether a `scheduled` unit carries the complete native evidence the
    /// publisher requires before assigning that state.
    pub(in crate::cli::registry) fn scheduled_unit_is_healthy(&self, unit: &str) -> bool {
        let Some(fields) = self
            .body
            .as_ref()
            .and_then(|body| body.get("units"))
            .and_then(Value::as_object)
            .and_then(|units| units.get(unit))
            .and_then(Value::as_object)
        else {
            return false;
        };
        let common_evidence = fields.get("state").and_then(Value::as_str) == Some(SCHEDULED_STATE)
            && fields.get("service_type").and_then(Value::as_str) == Some("oneshot")
            && fields
                .get("manager")
                .and_then(Value::as_str)
                .is_some_and(|manager| matches!(manager, "system" | "user"))
            && fields
                .get("triggered_by")
                .and_then(Value::as_array)
                .is_some_and(|triggers| {
                    fields
                        .get("active_trigger")
                        .and_then(Value::as_str)
                        .is_some_and(|active| {
                            triggers
                                .iter()
                                .any(|trigger| trigger.as_str() == Some(active))
                        })
                })
            && fields
                .get("trigger_state")
                .and_then(Value::as_str)
                .is_some_and(|state| matches!(state, "active" | "activating" | "reloading"));
        if !common_evidence {
            return false;
        }
        match fields.get("run_state").and_then(Value::as_str) {
            Some("running") => {
                fields.get("native_state").and_then(Value::as_str) == Some("activating")
            }
            Some("succeeded") => {
                fields
                    .get("native_state")
                    .and_then(Value::as_str)
                    .is_some_and(|state| matches!(state, "inactive" | "active" | "reloading"))
                    && fields.get("result").and_then(Value::as_str) == Some("success")
                    && fields.get("exec_main_status").and_then(Value::as_str) == Some("0")
                    && fields
                        .get("last_started_at")
                        .and_then(Value::as_str)
                        .is_some_and(|stamp| !stamp.is_empty() && stamp != "n/a")
            }
            _ => false,
        }
    }
}
