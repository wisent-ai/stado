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

/// Write the record of this attempt. Every attempt is written, so the
/// record's time is the time of the last attempt it reports: how often it is
/// written is how often the worker attempts, not a refresh age of this
/// module's own. The worker attempts on its declared poll period, which
/// `stado serve --worker` requires as `--poll-seconds` (an installed unit
/// passes `STADO_AGENT_POLL_SECONDS`, config `agent.poll_seconds`). The
/// agent tick writes the record at its capacity publication and again when
/// one of its steps (or, before it starts, the agent start) meets a store
/// that does not answer, and the capacity heartbeat at its republish; the
/// tick and the heartbeat each run at most once per poll period, so this one
/// small file is replaced, through a staged copy renamed over it, at most
/// three times per poll period. Skipping a write whose bytes would not
/// change saves none, since each write carries its own attempt time.
///
/// The previous record is read only to carry when the current run of
/// refusals began. A previous record that cannot be read is said on this
/// process's error output with its cause, and the run of refusals is counted
/// from this attempt. A record that cannot be written is said the same way,
/// which the resident unit's log keeps; the publication's own outcome stands
/// either way.
fn keep(step: &str, outcome: Outcome, error: Option<String>) {
    let written = path().and_then(|path| {
        let now = Utc::now();
        let before = match crate::primitives::document::read::<Attempt>(
            &path,
            "capacity publication record",
        ) {
            Ok(before) => before,
            Err(cause) => {
                eprintln!(
                    "stado: the previous record of this host's capacity publication attempts \
                         at {} cannot be read ({cause}); this attempt replaces it and counts any \
                         run of refusals from now",
                    path.display()
                );
                None
            }
        };
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
