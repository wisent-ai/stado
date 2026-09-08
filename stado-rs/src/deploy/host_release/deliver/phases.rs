use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::super::{
    activate_script, is_sha256, marker, marker_values, markers, recheck_staged_script,
    stage_script, ReleasePlan, ALREADY_ACTIVE_STATUS, STAGE_TIMEOUT,
};
use super::{fail, step_entry, step_failure};
use crate::deploy::{host_channel, service, service_label_print, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Whether the host already runs this exact image, proven rather than assumed.
///
/// A finished report means nothing has to be delivered. Otherwise the units
/// this did prove are recorded, and the ordinary phases run.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prove_active_image(
    target: &ComputeTarget,
    plan: &ReleasePlan,
    units: &[service::ManagedService],
    probe_markers: &[(String, String)],
    probe_code: i32,
    active_version: &str,
    steps: &[Value],
    already_proven_units: &mut BTreeMap<String, Value>,
    report: &mut Map<String, Value>,
    runner: &Runner,
) -> Result<Option<Value>, DeployError> {
    if active_version == plan.version && !plan.reinstall {
        if plan.product.install.is_tree() {
            report.insert("steps".to_string(), json!(steps));
            report.insert("exit_code".to_string(), json!(probe_code));
            report.insert("status".to_string(), json!(ALREADY_ACTIVE_STATUS));
            return Ok(Some(Value::Object(std::mem::take(report))));
        }
        let active_sha256 = marker(probe_markers, "active_sha256");
        let staged_sha256 = marker(probe_markers, "staged_sha256");
        let mut unit_processes = Vec::new();
        let mut exact_image = is_sha256(active_sha256) && active_sha256 == staged_sha256;
        if exact_image {
            for declared in units {
                let state = service_label_print::print_label(
                    target,
                    declared.unit_id(),
                    service::BootoutScope::Any,
                    runner,
                )
                .await?;
                let mapped = state.pid.is_some()
                    && state.process_started_at.is_some()
                    && state.process_executable.is_some()
                    && state.process_device.is_some()
                    && state.process_inode.is_some_and(|inode| inode != 0)
                    && state.process_sha256.as_deref() == Some(active_sha256);
                if mapped {
                    let evidence = state.to_json();
                    unit_processes.push(evidence.clone());
                    already_proven_units.insert(declared.unit_id().to_string(), evidence);
                } else {
                    exact_image = false;
                }
            }
        }
        if exact_image {
            report.insert("active_sha256".to_string(), json!(active_sha256));
            report.insert("unit_processes".to_string(), json!(unit_processes));
            report.insert("steps".to_string(), json!(steps));
            report.insert("exit_code".to_string(), json!(probe_code));
            report.insert("status".to_string(), json!(ALREADY_ACTIVE_STATUS));
            return Ok(Some(Value::Object(std::mem::take(report))));
        }
        report.insert("active_image_proved".to_string(), json!(false));
    }
    Ok(None)
}

/// Phase two for one plan: fetch, verify and stage, or re-verify the exact
/// extracted digest persisted before an authority outage.
pub(super) async fn stage_phase(
    target: &ComputeTarget,
    plan: &ReleasePlan,
    pre_staged_sha256: Option<&str>,
    report: &mut Map<String, Value>,
    steps: &mut Vec<Value>,
    runner: &Runner,
) -> Result<Result<String, Value>, DeployError> {
    let stage = if pre_staged_sha256.is_some() {
        host_channel::run_script(target, &recheck_staged_script(plan)?, runner).await?
    } else {
        host_channel::run_script_with_timeout(target, &stage_script(plan), STAGE_TIMEOUT, runner)
            .await?
    };
    let stage_markers = markers(&stage.stdout);
    report.insert(
        "fetched_sha256".to_string(),
        json!(marker(&stage_markers, "sha256")),
    );
    if !stage.ok() || marker(&stage_markers, "step") != "stage" {
        let detail = step_failure(&stage_markers, &stage);
        steps.push(step_entry(
            "stage",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        // Said outright, because it is the question an operator asks next.
        report.insert("active_version_unchanged".to_string(), json!(true));
        return Ok(Err(fail(report, stage.code, detail)));
    }
    let staged_sha256 = marker(&stage_markers, "staged_sha256").to_string();
    if pre_staged_sha256.is_some_and(|expected| expected != staged_sha256) {
        let detail = "pre-staged program digest changed before activation".to_string();
        steps.push(step_entry(
            "stage",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        report.insert("active_version_unchanged".to_string(), json!(true));
        return Ok(Err(fail(report, stage.code, detail)));
    }
    if !plan.product.install.is_tree() && !is_sha256(&staged_sha256) {
        let detail = "staging did not report the exact extracted program digest".to_string();
        steps.push(step_entry(
            "stage",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        report.insert("active_version_unchanged".to_string(), json!(true));
        return Ok(Err(fail(report, stage.code, detail)));
    }
    report.insert("staged_sha256".to_string(), json!(staged_sha256));
    steps.push(step_entry("stage", "ok", None));
    Ok(Ok(staged_sha256))
}

/// Every declared unit as it stood before activation, so a restart can be
/// proven to have produced a fresh pid rather than the one already running.
pub(super) async fn unit_state_before(
    target: &ComputeTarget,
    units: &[service::ManagedService],
    report: &mut Map<String, Value>,
    runner: &Runner,
) -> Result<BTreeMap<String, service_label_print::LabelState>, DeployError> {
    let mut prior_unit_state = BTreeMap::new();
    for declared in units {
        let state = service_label_print::print_label(
            target,
            declared.unit_id(),
            service::BootoutScope::Any,
            runner,
        )
        .await?;
        prior_unit_state.insert(declared.unit_id().to_string(), state);
    }
    report.insert(
        "unit_processes_before".to_string(),
        json!(prior_unit_state
            .values()
            .map(|state| state.to_json())
            .collect::<Vec<_>>()),
    );
    Ok(prior_unit_state)
}

/// Phase three for one plan: activate the verified staging image and read
/// back the digest the host now maps.
pub(super) async fn activate_phase(
    target: &ComputeTarget,
    plan: &ReleasePlan,
    staged_sha256: &str,
    report: &mut Map<String, Value>,
    steps: &mut Vec<Value>,
    runner: &Runner,
) -> Result<Result<String, Value>, DeployError> {
    let activate = host_channel::run_script(target, &activate_script(plan), runner).await?;
    let activate_markers = markers(&activate.stdout);
    if !activate.ok() || marker(&activate_markers, "step") != "activate" {
        let detail = step_failure(&activate_markers, &activate);
        steps.push(step_entry(
            "activate",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        return Ok(Err(fail(report, activate.code, detail)));
    }
    steps.push(step_entry("activate", "ok", None));
    let active_sha256 = marker(&activate_markers, "active_sha256").to_string();
    if !plan.product.install.is_tree()
        && (!is_sha256(&active_sha256) || active_sha256 != staged_sha256)
    {
        let detail =
            "activation did not map the exact digest verified from the release archive".to_string();
        steps.push(step_entry(
            "activate",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        return Ok(Err(fail(report, activate.code, detail)));
    }
    report.insert("active_sha256".to_string(), json!(active_sha256));
    if plan.product.install.is_tree() {
        // The paths this delivery actually replaced, as the host named them
        // one by one. An operator asking what a tree delivery touched gets
        // the answer from the program that did it.
        report.insert(
            "replaced_paths".to_string(),
            json!(marker_values(&activate_markers, "replaced")),
        );
    }
    Ok(Ok(active_sha256))
}
