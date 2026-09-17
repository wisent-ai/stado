//! The record of a placement the fleet could not make.
//!
//! `stado fleet needs` answers "what does the fleet lack" from evidence, and
//! a refused placement is the strongest evidence there is: someone asked for
//! a Windows host, a GPU with more memory, or a Mac with room for one more
//! session, and no host could give it. Without a record, that demand is
//! gone the moment the refusal scrolls off the terminal, and the advisor can
//! only guess. Every refusing path writes one of these; the advisor reads
//! them back over its window.
//!
//! Scheme: `state/fleet/unmet/<yyyy-mm-dd>/<uuid>.json`, an
//! [`UnmetPlacement`] serialized as written.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

pub const UNMET_PREFIX: &str = "state/fleet/unmet/";
pub const UNMET_SCHEMA_VERSION: u64 = 1;

/// What the placement asked for. Every field is optional because a request
/// names only what it needs; an unset field is "no requirement", never zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Requirement {
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub gpu_type: Option<String>,
    #[serde(default)]
    pub vram_gb: i64,
    #[serde(default)]
    pub ram_gb: f64,
    #[serde(default)]
    pub cpu_cores: i64,
    #[serde(default)]
    pub exclusive: bool,
    #[serde(default)]
    pub pinned_host: Option<String>,
}

/// Why one candidate host was refused, in the sentence it was refused with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub target: String,
    pub refusal: String,
}

/// The reason no host could take the work. One word, from a closed set, so
/// the advisor can count them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnmetReason {
    NoEligibleTarget,
    CapacityExhausted,
    ReservationsExhausted,
    MemoryPressure,
    DiskPressure,
}

impl UnmetReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoEligibleTarget => "no_eligible_target",
            Self::CapacityExhausted => "capacity_exhausted",
            Self::ReservationsExhausted => "reservations_exhausted",
            Self::MemoryPressure => "memory_pressure",
            Self::DiskPressure => "disk_pressure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnmetPlacement {
    pub schema_version: u64,
    pub recorded_at: String,
    /// The workload kind or `job`.
    pub kind: String,
    pub product: String,
    /// Who asked: the operator's `user@host`, or a coordinator.
    pub requester: String,
    pub requirement: Requirement,
    pub reason: UnmetReason,
    pub candidates: Vec<Candidate>,
}

impl UnmetPlacement {
    pub fn new(
        kind: &str,
        product: &str,
        requester: String,
        requirement: Requirement,
        reason: UnmetReason,
        candidates: Vec<Candidate>,
    ) -> Self {
        Self {
            schema_version: UNMET_SCHEMA_VERSION,
            recorded_at: Utc::now().to_rfc3339(),
            kind: kind.to_string(),
            product: product.to_string(),
            requester,
            requirement,
            reason,
            candidates,
        }
    }

    pub fn recorded(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.recorded_at)
            .ok()
            .map(|stamp| stamp.with_timezone(&Utc))
    }
}

/// The operator identity a CLI-side refusal is recorded under.
pub fn this_requester() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "operator".to_string());
    let host = crate::targets::normalize_hostname(&crate::providers::vast::system_hostname());
    format!("{user}@{host}")
}

/// Write one record. The day folder keeps a listing over a window cheap:
/// the advisor lists only the days it asks about.
pub async fn record_unmet(store: &JobStorage, unmet: &UnmetPlacement) -> Result<(), StorageError> {
    let day = unmet
        .recorded()
        .unwrap_or_else(Utc::now)
        .format("%Y-%m-%d")
        .to_string();
    let key = format!("{UNMET_PREFIX}{day}/{}.json", uuid::Uuid::new_v4());
    let body = serde_json::to_string(unmet)?;
    store.upload_text(&key, &body).await
}

/// Every record from `since` on, oldest first. Days are listed one by one
/// so a long history costs only the window asked for.
pub async fn read_unmet(
    store: &JobStorage,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<UnmetPlacement>, StorageError> {
    let mut records = Vec::new();
    let mut day = since.date_naive();
    let last = now.date_naive();
    loop {
        let directory = format!("{UNMET_PREFIX}{}/", day.format("%Y-%m-%d"));
        for blob in store.list_blobs_with_meta(&directory).await? {
            if !blob.name.ends_with(".json") {
                continue;
            }
            let Some(raw) = store.download_text(&blob.name).await? else {
                continue;
            };
            let record: UnmetPlacement = serde_json::from_str(&raw)?;
            if record.recorded().is_some_and(|stamp| stamp >= since) {
                records.push(record);
            }
        }
        if day >= last {
            break;
        }
        day = day.succ_opt().unwrap_or(last);
    }
    records.sort_by(|left, right| left.recorded_at.cmp(&right.recorded_at));
    Ok(records)
}
