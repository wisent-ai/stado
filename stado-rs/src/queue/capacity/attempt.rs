//! This host's own record of its worker's last attempt to speak to the fleet
//! store, kept on the host itself (`~/.stado/logs/capacity-publication.json`)
//! because the store is exactly what may not answer.
//!
//! A worker whose store refuses it publishes no capacity, and a host with no
//! publication reads to every placement as one that never spoke: the cause
//! (an object API answering 503, a LAN peer macOS's Local Network decision
//! keeps it from, a vault that hangs) stayed in the unit's log on that host.
//! Every capacity write, every agent start and every tick that met a store
//! that did not answer now leaves its outcome here, with when the refusals
//! began, and `stado host gates` reads it over the host channel, which needs
//! no store.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where the record is kept, under `~/.stado` of the home every Stado file
/// is kept under, which is where `stado host gates` reads it on the host.
pub const RECORD: &str = "logs/capacity-publication.json";

/// The outcome of the last attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The capacity publication reached the store.
    Published,
    /// The store refused or did not answer.
    Refused,
}

/// The record itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub at: DateTime<Utc>,
    pub outcome: Outcome,
    /// What tried: `capacity publication`, `agent start` or `tick`.
    pub step: String,
    /// The store's own words when it refused.
    #[serde(default)]
    pub error: Option<String>,
    /// When the unbroken run of refusals this one belongs to began; none
    /// after a publication.
    #[serde(default)]
    pub refused_since: Option<DateTime<Utc>>,
    pub stado_version: String,
}

fn path() -> Result<PathBuf, String> {
    let home = crate::config_file::home()
        .map_err(|cause| format!("the capacity publication record has no place: {cause}"))?;
    Ok(home.join(".stado").join(RECORD))
}

/// Record that `step` reached the store.
pub fn published(step: &str) {
    keep(step, Outcome::Published, None);
}

/// Record that `step` met a store that refused it with `error`.
pub fn refused(step: &str, error: &str) {
    keep(step, Outcome::Refused, Some(error.to_string()));
}

/// How old a record that says the same thing may grow before it is written
/// again: a worker polling every second rewrites it once a minute, not once a
/// poll, and its time stays within a minute of the attempt it reports.
const REFRESH_SECONDS: i64 = 60;

/// Write the record. A record that cannot be written is said on this
/// process's error output, which the resident unit's log keeps; the
/// publication's own outcome stands either way.
fn keep(step: &str, outcome: Outcome, error: Option<String>) {
    let written = path().and_then(|path| {
        let now = Utc::now();
        let before =
            crate::primitives::document::read::<Attempt>(&path, "capacity publication record")
                .ok()
                .flatten();
        if before.as_ref().is_some_and(|before| {
            before.outcome == outcome
                && before.step == step
                && before.error == error
                && (now - before.at).num_seconds() < REFRESH_SECONDS
        }) {
            return Ok(());
        }
        let refused_since = match outcome {
            Outcome::Published => None,
            Outcome::Refused => Some(
                before
                    .and_then(|before| before.refused_since)
                    .unwrap_or(now),
            ),
        };
        let attempt = Attempt {
            at: now,
            outcome,
            step: step.to_string(),
            error,
            refused_since,
            stado_version: env!("CARGO_PKG_VERSION").to_string(),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|cause| format!("cannot create {}: {cause}", parent.display()))?;
        }
        crate::primitives::document::write(&path, &attempt, "capacity publication record")
    });
    if let Err(cause) = written {
        eprintln!(
            "stado: the outcome of this host's {step} was not recorded for `stado host gates` \
             ({cause}); repair the Stado directory's logs folder so the next attempt is kept"
        );
    }
}
