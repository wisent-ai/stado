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
    /// Parsed beacon body; `None` when the object is absent, unparsable or
    /// is not a JSON object, with `unreadable` saying which.
    pub(super) body: Option<Map<String, Value>>,
    /// Why `body` is `None` when the object was there: the parse error, or
    /// the JSON type it held instead of an object. `None` for a body that
    /// was read, and for an object the store answered no text for.
    pub(in crate::cli::registry) unreadable: Option<String>,
}

impl Beacon {
    /// `reported_at` as the host stamped it.
    pub(super) fn reported_at(&self) -> Option<&str> {
        self.body.as_ref()?.get("reported_at")?.as_str()
    }

    /// When this beacon was last known good: the object mtime, or for a
    /// backend that carries no mtime the host's own `reported_at`. `None`
    /// when it states neither; an error naming the text when `reported_at`
    /// is there but is not a time, so a corrupt stamp is never read as a
    /// beacon that was never observed.
    pub(in crate::cli::registry) fn observed_at(&self) -> Result<Option<DateTime<Utc>>, String> {
        if let Some(updated) = self.updated {
            return Ok(Some(updated));
        }
        let Some(text) = self.reported_at() else {
            return Ok(None);
        };
        DateTime::parse_from_rfc3339(text)
            .map(|stamp| Some(stamp.with_timezone(&Utc)))
            .map_err(|error| format!("reported_at {text:?} is not an RFC 3339 time: {error}"))
    }

    /// When the reporting host promised its next beacon: the `next_by` it
    /// wrote, or for a beacon from before that field, the time it was last
    /// observed plus the `stale_after_seconds` it stated. `None` when it
    /// states neither; an error naming the text when a stamp is not a time.
    pub(in crate::cli::registry) fn next_by(&self) -> Result<Option<DateTime<Utc>>, String> {
        let Some(body) = self.body.as_ref() else {
            return Ok(None);
        };
        if let Some(text) = body.get("next_by").and_then(Value::as_str) {
            return DateTime::parse_from_rfc3339(text)
                .map(|next| Some(next.with_timezone(&Utc)))
                .map_err(|error| format!("next_by {text:?} is not an RFC 3339 time: {error}"));
        }
        let Some(window) = body.get("stale_after_seconds").and_then(Value::as_i64) else {
            return Ok(None);
        };
        Ok(self
            .observed_at()?
            .map(|observed| observed + chrono::Duration::seconds(window)))
    }

    /// State of one unit in the beacon's `units` map. Values are either
    /// `{"state": ...}` objects or a bare string, exactly as
    /// `monitor::host_health::format_host_health` reads them.
    pub(in crate::cli::registry) fn unit_state(&self, unit: &str) -> Option<String> {
        let value = self.body.as_ref()?.get("units")?.as_object()?.get(unit)?;
        match value {
            Value::Object(state) => Some(match state.get("state").and_then(Value::as_str) {
                Some(word) => word.to_string(),
                // The unit is in the beacon but its state is not a word:
                // what it carried is shown, never a blank that reads as a
                // state of its own.
                None => format!("unreadable state {value}"),
            }),
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
                .is_some_and(|word| crate::deploy::service::SystemdManager::parse(word).is_ok())
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
                .is_some_and(|word| {
                    crate::deploy::service::SystemdActiveState::parse(word)
                        .is_ok_and(crate::deploy::service::SystemdActiveState::up_or_starting)
                });
        if !common_evidence {
            return false;
        }
        match fields.get("run_state").and_then(Value::as_str) {
            Some("running") => fields
                .get("native_state")
                .and_then(Value::as_str)
                .is_some_and(|word| {
                    crate::deploy::service::SystemdActiveState::parse(word)
                        == Ok(crate::deploy::service::SystemdActiveState::Activating)
                }),
            Some("succeeded") => {
                fields
                    .get("native_state")
                    .and_then(Value::as_str)
                    .is_some_and(|word| {
                        crate::deploy::service::SystemdActiveState::parse(word)
                            .is_ok_and(crate::deploy::service::SystemdActiveState::settled)
                    })
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
