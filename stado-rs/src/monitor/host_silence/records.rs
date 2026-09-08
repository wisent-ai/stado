//! The three stored documents: one gap in a host's beacon publication, one
//! reader declining to answer, and a window of refusals counted.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One gap in a host's beacon publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SilenceRecord {
    /// Registry name of the host that went quiet.
    pub host: String,
    /// Last moment the host was heard from — the newest beacon that existed
    /// when the gap opened, or the moment of observation when the host has
    /// never published one.
    pub started_at: DateTime<Utc>,
    /// The fresher beacon that ended the gap; `None` while it is open.
    pub ended_at: Option<DateTime<Utc>>,
    /// `ended_at - started_at`; `None` while the gap is open.
    pub duration_seconds: Option<i64>,
    /// The first refusal sentence any reader produced during this gap,
    /// verbatim. Kept on the silence itself because it is the one line that
    /// tells an operator which subsystem noticed first.
    pub first_reader_error: Option<String>,
    /// Every component that observed this gap, in first-observation order.
    pub observed_by: Vec<String>,
}

/// One reader declining to answer, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalRecord {
    /// The host the refusal is ABOUT, which need not be the host that
    /// refused.
    pub host: String,
    /// When the reader refused.
    pub at: DateTime<Utc>,
    /// `resolver` / `cli` / `dashboard`.
    pub reader: String,
    /// Short stable token: [`REASON_DIRECTORY_CACHE_STALE`](super::REASON_DIRECTORY_CACHE_STALE),
    /// [`REASON_AUTHORITY_UNREACHABLE`](super::REASON_AUTHORITY_UNREACHABLE),
    /// [`REASON_BEACON_STALE`](super::REASON_BEACON_STALE).
    pub reason: String,
    /// The refusing component's own sentence, verbatim.
    pub detail: String,
}

/// Refusals about one host over one window, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalSummary {
    /// The window these counts cover, in seconds.
    pub window_seconds: i64,
    /// Refusals in the window.
    pub count: usize,
    /// Refusals in the window per `reason` token.
    pub reasons: BTreeMap<String, usize>,
}

impl RefusalSummary {
    /// An empty window — no refusals, no reasons.
    pub fn empty(window_seconds: i64) -> Self {
        Self {
            window_seconds,
            count: 0,
            reasons: BTreeMap::new(),
        }
    }
}
