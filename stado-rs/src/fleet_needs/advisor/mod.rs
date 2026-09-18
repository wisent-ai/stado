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
use crate::models::Job;
use crate::primitives::constants;
use crate::queue::capacity::read_publications;
use crate::queue::{JobStorage, StorageError};
use crate::targets::{ComputeTarget, Registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedKind {
    Ram,
    Storage,
    Gpu,
    Cpu,
    Host,
}

impl NeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ram => "ram",
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
    pub window_days: i64,
    pub needs: Vec<Need>,
}

/// Queued jobs older than this are demand the fleet is failing to serve.
pub(super) const STALE_QUEUE_SECONDS: i64 = constants::NEEDS_STALE_QUEUE_SECONDS;

pub async fn advise(
    store: &JobStorage,
    registry: &Registry,
    window_days: i64,
    now: DateTime<Utc>,
) -> Result<NeedsReport, StorageError> {
    let since = now - chrono::Duration::days(window_days);
    let publications = read_publications(store).await?;
    let unmet = read_unmet(store, since, now).await?;
    let queued = store
        .list_jobs("queue", constants::NEEDS_QUEUE_WINDOW)
        .await?;
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
    needs.extend(fleet::platform_needs(registry, &unmet, &queued, now));
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

pub(super) fn is_stale(job: &Job, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(&job.created_at)
        .ok()
        .is_some_and(|created| {
            (now - created.with_timezone(&Utc)).num_seconds() >= STALE_QUEUE_SECONDS
        })
}

pub(super) fn fmt(value: Option<f64>) -> String {
    match value {
        Some(value) => format!("{value:.1}"),
        None => "?".to_string(),
    }
}
