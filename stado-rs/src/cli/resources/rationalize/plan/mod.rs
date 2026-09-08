//! Compiling audited findings into an immutable plan: the selected findings,
//! the actions that would carry them out, and the inventory snapshot the plan
//! is bound to. Nothing here mutates a resource; the plan is the only output.

mod actions;
mod conditions;

use std::collections::BTreeSet;

use sha2::Digest;

use crate::cli::resources::model::{
    Finding as PlanFinding, FindingDisposition, Intent, InventorySnapshot, OperationScope, Plan,
    ProviderKind, SourceSnapshot,
};
use crate::cli::resources::planner;
use crate::cli::CmdError;
use crate::config;
use crate::queue::copy::Endpoint;

use super::{Finding, RationalizationReport};
use actions::actions_for;
use conditions::locator;

pub(super) fn compile_plan(
    report: &RationalizationReport,
    selected_provider: Option<&str>,
) -> Result<Plan, CmdError> {
    let selected_provider = selected_provider.map(str::trim);
    let selected: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|finding| selected_provider.is_none_or(|provider| finding.provider == provider))
        .collect();
    if let Some(provider) = selected_provider {
        let configured = config::wc_providers()
            .iter()
            .chain(config::wc_disabled_providers())
            .any(|name| name == provider);
        let reported = selected.iter().any(|finding| finding.provider == provider);
        if !configured && !reported {
            return Err(CmdError::usage(format!(
                "provider {provider:?} is neither configured nor present in the audit"
            )));
        }
    }

    let findings: Vec<PlanFinding> = selected
        .iter()
        .map(|finding| PlanFinding {
            id: finding.id.clone(),
            severity: finding.severity.to_string(),
            confidence: finding.confidence.to_string(),
            recommendation: finding.action.to_string(),
            reason: finding.reason.clone(),
            evidence: finding.evidence.clone(),
            disposition: finding_disposition(finding),
            resource: locator(finding),
        })
        .collect();
    let mut actions = Vec::new();
    for finding in &selected {
        actions.extend(actions_for(finding));
    }
    let providers = findings
        .iter()
        .map(|finding| finding.resource.provider)
        .collect();
    let mut projects = BTreeSet::new();
    if findings
        .iter()
        .any(|finding| finding.resource.provider == ProviderKind::Gcp)
        && !config::project().is_empty()
    {
        projects.insert(config::project().to_string());
    }
    let report_value = serde_json::to_value(report)?;
    let snapshot_id = hex::encode(sha2::Sha256::digest(serde_json::to_vec(&report_value)?));
    let inventory = InventorySnapshot {
        snapshot_id,
        complete: report.summary.incomplete_sources == usize::default(),
        sources: report
            .sources
            .iter()
            .map(|source| SourceSnapshot {
                name: source.name.clone(),
                state: source.state.to_string(),
                detail: source.detail.clone(),
            })
            .collect(),
    };
    planner::new_plan(
        Intent::RationalizationCleanup,
        OperationScope {
            providers,
            projects,
            storage: Endpoint::configured_primary().describe(),
        },
        inventory,
        findings,
        actions,
    )
}

fn finding_disposition(finding: &Finding) -> FindingDisposition {
    if finding.automatic {
        FindingDisposition::Automatic
    } else if matches!(finding.action, "disable-or-migrate" | "review-deprovision") {
        FindingDisposition::Blocked
    } else {
        FindingDisposition::ReviewRequired
    }
}
