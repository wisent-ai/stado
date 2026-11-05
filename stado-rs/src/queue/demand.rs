//! Grid demand-response events: a window in which declared hosts shed their
//! interruptible load, and what the program paid for it.
//!
//! The operator enrols hosts in a program of their choosing; Stado assumes
//! no program and reads no utility's API. An event is recorded with `stado
//! demand event add` (from the program's own notice) and kept at
//! `state/demand/events/<id>.json`. While an event is active, each named
//! host's agent claims nothing and yields its running yieldable jobs back to
//! the queue; work that is not yieldable keeps running, since cutting it would
//! lose it. When the window ends the host claims again and the yielded jobs
//! are claimed like any queued work. The load shed is read afterwards from
//! the host's beacon hours (the boards' energy in the window against the same
//! hours before it), and the payment is recorded with `stado demand event
//! paid`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, Listing, StorageError};

pub const EVENTS_PREFIX: &str = "state/demand/events/";

/// One demand-response event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    /// The program, as the operator names it.
    pub program: String,
    /// Registry targets that shed load.
    pub hosts: Vec<String>,
    pub starts_at: String,
    pub ends_at: String,
    /// Where the event was read (the program's notice).
    pub source: String,
    pub recorded_at: String,
    /// What the program paid for this event, once stated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payment: Option<Payment>,
    /// Withdrawn before or during its window; no host sheds load for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    pub amount: f64,
    pub currency: String,
    pub source: String,
}

impl Event {
    fn window(&self) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        let starts = DateTime::parse_from_rfc3339(&self.starts_at).ok()?;
        let ends = DateTime::parse_from_rfc3339(&self.ends_at).ok()?;
        Some((starts.with_timezone(&Utc), ends.with_timezone(&Utc)))
    }

    /// Whether `host` sheds load for this event at `now`.
    pub fn sheds(&self, host: &str, now: DateTime<Utc>) -> bool {
        self.withdrawn_at.is_none()
            && self.hosts.iter().any(|named| named == host)
            && self
                .window()
                .is_some_and(|(starts, ends)| starts <= now && now < ends)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.id.is_empty()
        {
            return Err(format!(
                "event id {:?} must be lowercase letters, digits and hyphens",
                self.id
            ));
        }
        for (field, value) in [("--program", &self.program), ("--source", &self.source)] {
            if value.trim().is_empty() {
                return Err(format!("{field} cannot be empty"));
            }
        }
        if self.hosts.is_empty() {
            return Err("--host names no host; an event sheds load on named hosts".into());
        }
        let (starts, ends) = self
            .window()
            .ok_or("--starts-at and --ends-at must be RFC 3339 times".to_string())?;
        if ends <= starts {
            return Err("--ends-at must be after --starts-at".into());
        }
        Ok(())
    }
}

fn path(id: &str) -> String {
    format!("{EVENTS_PREFIX}{id}.json")
}

pub async fn load(store: &JobStorage, id: &str) -> Result<Option<Event>, StorageError> {
    let key = path(id);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StorageError::Other(format!("{key} is not a demand event: {error}")))
}

pub async fn save(store: &JobStorage, event: &Event) -> Result<(), StorageError> {
    let body = serde_json::to_string_pretty(event).map_err(|error| {
        StorageError::Other(format!("a demand event cannot be encoded: {error}"))
    })?;
    store.upload_text(&path(&event.id), &body).await
}

/// Every recorded event, newest start first.
pub async fn list(store: &JobStorage) -> Result<Vec<Event>, StorageError> {
    let mut events = Vec::new();
    for key in store.list_paths(EVENTS_PREFIX, Listing::Every).await? {
        let Some(text) = store.download_text(&key).await? else {
            continue;
        };
        events.push(serde_json::from_str::<Event>(&text).map_err(|error| {
            StorageError::Other(format!("{key} is not a demand event: {error}"))
        })?);
    }
    events.sort_by(|left, right| right.starts_at.cmp(&left.starts_at));
    Ok(events)
}

/// The event `host` sheds load for now, if any. An event list that cannot
/// be read is the error: an agent that cannot tell whether it is in a
/// window keeps claiming, and says why, rather than shedding on a guess.
pub async fn active_for(store: &JobStorage, host: &str) -> Result<Option<Event>, StorageError> {
    let now = Utc::now();
    Ok(list(store)
        .await?
        .into_iter()
        .find(|event| event.sheds(host, now)))
}
