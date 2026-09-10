//! Whether one fleet host may claim a release build job, judged from the
//! capacity publication it wrote itself.

use serde_json::Value;

/// Whether a fresh worker publication says it can accept another job.
///
/// The worker's explicit admission decision is authoritative. Missing data is
/// kept eligible during rolling upgrades because silence is not a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claimability {
    Claimable {
        available_cpu_cores: i64,
    },
    Refusing {
        blockers: Vec<String>,
    },
    /// Accepting work, and short of the disk the last build of this product
    /// and platform wrote. The host is not refusing anything; the coordinator
    /// is, because it read the evidence the host cannot have.
    Unfit {
        reason: String,
    },
    Unstated,
}

impl Claimability {
    /// Whether [`builder`] may pin a job here.
    pub fn eligible(&self) -> bool {
        !matches!(self, Self::Refusing { .. } | Self::Unfit { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Claimable {
                available_cpu_cores,
            } => format!("accepting jobs ({available_cpu_cores} CPU core(s) available)"),
            Self::Refusing { blockers } if blockers.is_empty() => {
                "not accepting jobs; no reason published".to_string()
            }
            Self::Refusing { blockers } => {
                format!("not accepting jobs; reasons: {}", blockers.join(", "))
            }
            Self::Unfit { reason } => {
                format!("accepting jobs but cannot hold this build: {reason}")
            }
            Self::Unstated => "published no admission decision".to_string(),
        }
    }
}

/// Judge one publication without reaching the host.
pub fn claimability(publication: &Value) -> Claimability {
    match publication.get("accepting_jobs").and_then(Value::as_bool) {
        Some(true) => Claimability::Claimable {
            available_cpu_cores: publication
                .get("available_cpu_cores")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        },
        Some(false) => Claimability::Refusing {
            blockers: publication_blockers(publication),
        },
        None => Claimability::Unstated,
    }
}
fn publication_flag(publication: &Value, name: &str) -> Option<bool> {
    publication
        .get("diag")
        .and_then(|diag| diag.get(name))
        .and_then(Value::as_bool)
}

/// The blockers a publication itself declares, named from the shared host-gate
/// vocabulary so the selector and diagnostics cannot drift apart.
fn publication_blockers(publication: &Value) -> Vec<String> {
    use crate::deploy::host_gates::{
        DISK_CLEANUP_POLICY_UNKNOWN, DISK_CLEANUP_STALLED, DISK_PRESSURE_ACTIVE,
        DISK_PRESSURE_UNRESOLVED, QUEUE_PAUSED,
    };
    let flag = |name: &str| publication_flag(publication, name);
    let mut blockers = Vec::new();
    if flag(DISK_PRESSURE_ACTIVE) == Some(true) {
        blockers.push(format!("{DISK_PRESSURE_ACTIVE} (release deliveries only)"));
    }
    if flag(DISK_PRESSURE_UNRESOLVED) == Some(true) {
        blockers.push(DISK_PRESSURE_UNRESOLVED.to_string());
    }
    if flag("disk_cleanup_policy_known") == Some(false) {
        blockers.push(DISK_CLEANUP_POLICY_UNKNOWN.to_string());
    }
    if flag("queue_paused") == Some(true) {
        blockers.push(QUEUE_PAUSED.to_string());
    }
    // A janitor whose pass cannot start is the condition that closed both
    // darwin-arm64 builders on 2026-09-03 with ample free disk on each. The
    // publication cannot compute the gate's staleness arithmetic -- that reads
    // the janitor state file on the host -- but it does carry the outcome, and
    // `lock_busy` is the outcome that never advances `last_success_at`.
    if publication
        .get("diag")
        .and_then(|diag| diag.get("disk_cleanup"))
        .and_then(|cleanup| cleanup.get("outcome"))
        .and_then(Value::as_str)
        == Some("lock_busy")
    {
        blockers.push(format!("{DISK_CLEANUP_STALLED} (janitor pass lock_busy)"));
    }
    if let Some(reason) = publication
        .get("diag")
        .and_then(|diag| diag.get("admission_reason"))
        .and_then(Value::as_str)
    {
        if !blockers.iter().any(|blocker| blocker == reason) {
            blockers.push(reason.to_string());
        }
    }
    blockers
}
