//! Classifying idle and orphaned resources against the policy TTLs.
//!
//! [`plan`] folds every classification this module returns into a finding and
//! keeps the bounded, authorized ones as actions.

mod plan;

// `policy` is rebound here for the moved lines that name
// `super::policy::ActionRisk::<variant>` verbatim in `plan`.
use super::policy;

use crate::cli::resources::model::{ActionKind, FindingDisposition};

use crate::autonomy::model::{Ownership, ResourceKind, ResourceRecord};
use crate::providers::LifecyclePhase;

use super::policy::{AutonomyMode, AutonomyPolicy};

pub use plan::build_plan;

struct Classification {
    severity: &'static str,
    confidence: &'static str,
    recommendation: &'static str,
    reason: &'static str,
    disposition: FindingDisposition,
    action: Option<ActionKind>,
}

fn classify(
    resource: &ResourceRecord,
    age_seconds: u64,
    policy: &AutonomyPolicy,
) -> Option<Classification> {
    if resource.phase == LifecyclePhase::Ended {
        return None;
    }
    let unowned = matches!(resource.ownership, Ownership::Observed | Ownership::Unknown);
    if resource.resource_type == ResourceKind::Instance
        && resource.workload.is_none()
        && age_seconds >= policy.idle.vm_seconds
        && low_utilization(resource)
    {
        if unowned {
            return Some(Classification {
                severity: "medium",
                confidence: "high",
                recommendation: "adopt explicitly or terminate manually",
                reason: "idle instance is not owned by Stado",
                disposition: FindingDisposition::Blocked,
                action: None,
            });
        }
        let scale_to_zero = policy
            .matching_rule(resource)
            .is_some_and(|rule| rule.scale_to_zero);
        if scale_to_zero {
            if resource.phase != LifecyclePhase::Running {
                return None;
            }
            return Some(Classification {
                severity: "medium",
                confidence: "high",
                recommendation: "stop idle instance according to scale-to-zero policy",
                reason: "owned/adopted instance is idle and exceeded the scale-to-zero TTL",
                disposition: if policy.mode == AutonomyMode::Report {
                    FindingDisposition::ReviewRequired
                } else {
                    FindingDisposition::Automatic
                },
                action: Some(ActionKind::StopInstance),
            });
        }
        return Some(Classification {
            severity: "high",
            confidence: "high",
            recommendation: "terminate idle Stado instance",
            reason: "owned/adopted instance has no workload and exceeded the idle TTL",
            disposition: if policy.mode == AutonomyMode::EnforceOwned {
                FindingDisposition::Automatic
            } else {
                FindingDisposition::ReviewRequired
            },
            action: Some(ActionKind::DeleteInstance),
        });
    }
    let kind = resource.resource_type;
    let (threshold, recommendation, reason) = if kind.disk() {
        (
            policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY,
            "snapshot if needed, then delete orphaned disk",
            "unattached storage exceeded the idle TTL",
        )
    } else if kind == ResourceKind::Snapshot {
        (
            policy.idle.snapshot_days * crate::monitor::billing::SECONDS_PER_DAY,
            "expire snapshot according to retention policy",
            "snapshot exceeded the retention TTL",
        )
    } else if kind == ResourceKind::PublicIp {
        (
            policy.idle.disk_days * crate::monitor::billing::SECONDS_PER_DAY,
            "release unused address",
            "unattached address exceeded the idle TTL",
        )
    } else if kind.artifact() {
        (
            policy.idle.artifact_days * crate::monitor::billing::SECONDS_PER_DAY,
            "apply artifact lifecycle policy after dependency review",
            "artifact exceeded the retention TTL",
        )
    } else {
        return None;
    };
    if resource.workload.is_some() || age_seconds < threshold {
        return None;
    }
    Some(Classification {
        severity: "medium",
        confidence: if resource.dependencies.is_empty() {
            "medium"
        } else {
            "low"
        },
        recommendation,
        reason,
        disposition: if unowned {
            FindingDisposition::Blocked
        } else {
            FindingDisposition::ReviewRequired
        },
        action: None,
    })
}

fn low_utilization(resource: &ResourceRecord) -> bool {
    resource.phase == LifecyclePhase::Stopped
        || (!resource.utilization.is_empty()
            && resource
                .utilization
                .values()
                .all(|value| *value <= f64::EPSILON))
}
