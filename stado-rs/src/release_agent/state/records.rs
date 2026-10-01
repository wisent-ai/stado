//! The rollout state document, the process records it names, and the digests
//! this host refuses to roll out again.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::release_cause::{self, QuarantineCause};

/// The rollout state document's schema version.
///
/// Public because a document is only this agent's if it carries this exact
/// number: `parse_state_document` refuses anything else, and a caller
/// building one — a test, a recovery tool — has to write the version the
/// parser will accept rather than a copy of it that drifts.
pub const STATE_SCHEMA: u32 = 1;

/// The exact words the agent appends to `detail` when a foreign program holds
/// the stable bind, so nothing was spawned. `release doctor` matches on this
/// to turn the phase into a blocker with a remedy: the two are one sentence
/// written in one place, not a string one command hopes the other still uses.
pub const NO_CANDIDATE_SPAWNED: &str = "no candidate was spawned";

pub(crate) const STATUS_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloutPhase {
    Idle,
    Downloaded,
    Verified,
    Staged,
    CandidateRunning,
    Ready,
    Routed,
    Monitoring,
    Committed,
    RolledBack,
    Failed,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRecord {
    pub version: String,
    pub artifact_sha256: String,
    pub manifest_sha256: String,
    pub port: u16,
    pub pid: i32,
    pub release_dir: String,
    pub started_at: DateTime<Utc>,
}

/// One digest this host refuses to roll out again, why, and what that reason
/// actually means.
///
/// `reason` is the sentence the agent composed at the moment it gave up, and it
/// leads with what the agent saw from outside the candidate. `cause` is the
/// name derived from it and from the candidate's own log, and `evidence` is the
/// one line that name was read from. All three are kept: a record that stored
/// only the cause could not be re-read when the vocabulary grows, and a record
/// that stored only the reason is what left twenty rows of truncated stderr
/// unreadable for a month.
///
/// `cause` and `evidence` default, because every record already on the fleet
/// was written without them and this struct refuses unknown fields — a missing
/// name has to read as [`QuarantineCause::Unclassified`], not as a parse
/// failure that would strand the rollout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineRecord {
    pub reason: String,
    pub quarantined_at: DateTime<Utc>,
    #[serde(default)]
    pub cause: QuarantineCause,
    #[serde(default)]
    pub evidence: String,
}

impl QuarantineRecord {
    /// Record one refusal from its sentence alone: the cause is whatever a
    /// product failure envelope inside it names.
    ///
    /// For refusals before a process starts, such as a fetch that failed, the
    /// reason is all available evidence.
    pub(crate) fn new(reason: String) -> Self {
        let classified = release_cause::classify(&reason);
        Self::classified(reason, classified)
    }

    /// Record one refusal whose cause the agent observed itself.
    pub(crate) fn observed(refusal: release_cause::Refusal) -> Self {
        let classified = release_cause::Classification::observed(refusal.cause, &refusal.sentence);
        Self::classified(refusal.sentence, classified)
    }

    /// Record one refusal whose cause was read from more of the candidate's
    /// own output than the reason could carry.
    pub(crate) fn classified(reason: String, classified: release_cause::Classification) -> Self {
        Self {
            reason,
            quarantined_at: Utc::now(),
            cause: classified.cause,
            evidence: classified.evidence,
        }
    }

    /// The named cause this record carries, derived from a failure envelope
    /// in its reason when the record has none of its own.
    ///
    /// Every record written before the agent classified anything carries no
    /// name. Re-deriving reads only the envelope a product wrote into the
    /// reason, so a record whose reason holds none stays unclassified.
    ///
    /// Stored first, and that order is load-bearing: the agent classifies the
    /// whole log, while the reason kept here is a bounded tail of it, and the
    /// tail can stop before the line the stored evidence quotes.
    ///
    /// It lives on the record because three readers need one answer: the
    /// agent deciding whether it may retire this record by itself,
    /// `release doctor` and `quarantine list`.
    pub fn classification(&self) -> release_cause::Classification {
        if self.cause.is_classified() {
            return release_cause::Classification {
                cause: self.cause,
                evidence: self.evidence.clone(),
            };
        }
        release_cause::classify(&self.reason)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostReleaseState {
    pub schema_version: u32,
    pub product: String,
    pub target: String,
    pub rollout_generation: u64,
    pub phase: RolloutPhase,
    #[serde(default)]
    pub active: Option<ProcessRecord>,
    #[serde(default)]
    pub previous: Option<ProcessRecord>,
    #[serde(default)]
    pub candidate: Option<ProcessRecord>,
    #[serde(default)]
    pub proxy_pid: Option<i32>,
    #[serde(default)]
    pub cutover_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub quarantined: BTreeMap<String, QuarantineRecord>,
    #[serde(default)]
    pub detail: String,
    pub updated_at: DateTime<Utc>,
}

impl HostReleaseState {
    pub(crate) fn new(product: &str, target: &str) -> Self {
        Self {
            schema_version: STATE_SCHEMA,
            product: product.to_string(),
            target: target.to_string(),
            rollout_generation: 0,
            phase: RolloutPhase::Idle,
            active: None,
            previous: None,
            candidate: None,
            proxy_pid: None,
            cutover_at: None,
            quarantined: BTreeMap::new(),
            detail: String::new(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveBinary {
    pub path: PathBuf,
    pub version: String,
    pub platform: String,
    pub artifact_sha256: String,
    pub manifest_sha256: String,
}
