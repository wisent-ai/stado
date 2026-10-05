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
use crate::targets::{ComputeTarget, Registry};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Need {
    pub need: NeedKind,
    pub target: Option<String>,
    pub platform: Option<String>,
    pub severity: Severity,
    pub summary: String,
    pub evidence: Vec<Evidence>,
    pub suggestion: String,
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
    let queued = store.list_jobs("queue", 0).await?;
    let mut needs = Vec::new();
    for target in registry
        .targets
        .iter()
        .filter(|target| target.kind == "local")
    {
        let publication = publications
            .iter()
            .find(|(consumer, _)| consumer_names(registry, target, consumer))
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

/// Whether a `<kind>-<hostname>` publication key names `target`, by the
/// fleet's one hostname-to-target rule.
fn consumer_names(registry: &Registry, target: &ComputeTarget, consumer: &str) -> bool {
    crate::queue::capacity::consumer_names_target(registry, target, consumer)
}

pub(super) fn diag_number(payload: &Value, key: &str) -> Option<f64> {
    payload
        .get("diag")
        .and_then(|diag| diag.get(key))
        .and_then(Value::as_f64)
}
