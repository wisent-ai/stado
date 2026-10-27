//! Turn what the fleet publishes into what it lacks.
//!
//! Every need here is a sentence with numbers in it, and every number comes
//! from something the fleet already wrote: a capacity publication, a
//! declared watermark, a queued job's constraints, a refused placement. The
//! advisor never measures a host itself and never invents a demand: no
//! record asking for Windows means no Windows need, however plausible one
//! sounds.
//!
//! `host` reads one host's publication against its declarations; `fleet`
//! reads the demand nothing in the fleet can serve.

mod fleet;
mod host;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::unmet::read_unmet;
use crate::primitives::constants;
use crate::queue::capacity::read_publications;
use crate::queue::{JobStorage, StorageError};
use crate::targets::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedKind {
    Storage,
    Gpu,
    Cpu,
    Host,
}

impl NeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
            Self::Host => "host",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub source: String,
    pub detail: String,
}

impl Evidence {
    pub(super) fn new(source: &str, detail: String) -> Self {
        Self {
            source: source.to_string(),
            detail,
        }
    }
}

/// One thing the fleet lacks. Its JSON carries every field below and
/// `subject`, the word the text report prints in parentheses, so a reader
/// (Stado Desktop, a terminal) shows the same subject without deriving it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Need {
    pub need: NeedKind,
    pub target: Option<String>,
    pub platform: Option<String>,
    pub severity: Severity,
    pub summary: String,
    pub evidence: Vec<Evidence>,
    pub suggestion: String,
}

impl Need {
    /// What the need is about: the host when it names one, else the
    /// platform, else the fleet as a whole.
    pub fn subject(&self) -> &str {
        match (&self.target, &self.platform) {
            (Some(target), _) => target,
            (None, Some(platform)) => platform,
            (None, None) => "fleet",
        }
    }
}

impl Serialize for Need {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut row = serializer.serialize_map(None)?;
        row.serialize_entry("need", &self.need)?;
        row.serialize_entry("target", &self.target)?;
        row.serialize_entry("platform", &self.platform)?;
        row.serialize_entry("subject", self.subject())?;
        row.serialize_entry("severity", &self.severity)?;
        row.serialize_entry("summary", &self.summary)?;
        row.serialize_entry("evidence", &self.evidence)?;
        row.serialize_entry("suggestion", &self.suggestion)?;
        row.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NeedsReport {
    pub schema_version: u64,
    pub generated_at: String,
    /// The days of placement history read; None when every retained record
    /// was read.
    pub window_days: Option<i64>,
    pub needs: Vec<Need>,
}

/// Every need the evidence shows. A queued job counts as demand as soon as
/// no host the fleet declares (or no live GPU) could run it: its age adds
/// nothing to that fact.
pub async fn advise(
    store: &JobStorage,
    registry: &Registry,
    window_days: Option<i64>,
    now: DateTime<Utc>,
) -> Result<NeedsReport, StorageError> {
    let since = window_days.map(|days| now - chrono::Duration::days(days));
    let publications = read_publications(store).await?;
    let unmet = read_unmet(store, since, now).await?;
    // Every queued job is demand; no window is chosen here.
    let queued = store.list_jobs("queue").await?;
    let mut needs = Vec::new();
    for target in registry
        .targets
        .iter()
        .filter(|target| target.kind == "local")
    {
        // An identity several targets declare leaves this host's publication
        // unknown; read as none, its needs would be advised as a silent host's.
        let publication = crate::queue::capacity::publication_of(registry, target, &publications)
            .map_err(|error| StorageError::Other(error.to_string()))?
            .map(|(_, publication)| publication);
        needs.extend(host::host_needs(target, publication, &unmet, now));
    }
    needs.extend(fleet::platform_needs(registry, &unmet, &queued));
    needs.extend(fleet::gpu_needs(
        registry,
        &publications,
        &unmet,
        &queued,
        now,
    ));
    needs.sort_by_key(|need| std::cmp::Reverse(need.severity));
    Ok(NeedsReport {
        schema_version: constants::NEEDS_SCHEMA_VERSION,
        generated_at: now.to_rfc3339(),
        window_days,
        needs,
    })
}

pub(super) fn diag_number(payload: &Value, key: &str) -> Option<f64> {
    payload
        .get("diag")
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_f64)
}
