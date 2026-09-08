use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::super::{
    declared_units, is_sha256, marker, marker_values, markers, plan, probe_script, ReleaseRequest,
    PLANNED_STATUS, RELEASED_STATUS,
};
use super::phases::{activate_phase, prove_active_image, stage_phase, unit_state_before};
use super::restart::restart_units;
use super::verify::{planned_steps, verify_stable_binds, STABLE_BIND_BUDGET_SECONDS};
use super::{base_release_report, fail, step_entry, step_failure};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Deliver one declared product to one already-resolved registry target.
///
/// Split out from [`release_host`] so the whole command — every refusal,
/// every phase, and the order the phases run in — is exercisable through the
/// [`Runner`] seam with no registry and no host.
pub async fn release_target(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    self_store: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    release_target_inner(target, request, self_store, None, runner).await
}

/// Activate bytes staged by this same release protocol before an authority
/// outage. The caller supplies the extracted program digest reported by the
/// staging invocation; activation and every restarted unit must map that exact
/// digest. Tree products are intentionally excluded from this recovery seam.
pub async fn activate_staged_target(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    self_store: bool,
    staged_sha256: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if !is_sha256(staged_sha256) {
        return Err(DeployError(
            "pre-staged release digest is not a SHA-256".to_string(),
        ));
    }
    release_target_inner(target, request, self_store, Some(staged_sha256), runner).await
}

async fn release_target_inner(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    self_store: bool,
    pre_staged_sha256: Option<&str>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let plan = plan(target, request, self_store)?;

    let units = declared_units(target, plan.product);
    let mut report = base_release_report(target, &plan, &units);
    let mut steps: Vec<Value> = Vec::new();

    // Phase one: read the host. Read-only, and the only phase a dry run runs.
    let probe = host_channel::run_script(target, &probe_script(&plan), runner).await?;
    let probe_markers = markers(&probe.stdout);
    if !probe.ok() || marker(&probe_markers, "step") != "probe" {
        steps.push(step_entry(
            "probe",
            host_channel::FAILED_STATUS,
            Some(step_failure(&probe_markers, &probe)),
        ));
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            step_failure(&probe_markers, &probe),
        ));
    }
    let active_version = marker(&probe_markers, "active_version").to_string();
    let active_state = marker(&probe_markers, "active_state").to_string();
    let host_platform = marker(&probe_markers, "platform").to_string();
    report.insert("host_platform".to_string(), json!(host_platform));
    report.insert("active_version".to_string(), json!(active_version));
    report.insert("active_state".to_string(), json!(active_state));
    report.insert(
        "staged_state".to_string(),
        json!(marker(&probe_markers, "staged_state")),
    );
    // What the install root holds now, for a tree: the code a delivery
    // replaces and the host-local state it does not. Empty for a program,
    // which has neither.
    let code_paths: Vec<String> = marker_values(&probe_markers, "code_path")
        .into_iter()
        .map(str::to_string)
        .collect();
    if plan.product.install.is_tree() {
        report.insert(
            "root_state".to_string(),
            json!(marker(&probe_markers, "root_state")),
        );
        report.insert("code_paths".to_string(), json!(code_paths));
        report.insert(
            "preserved_paths_present".to_string(),
            json!(marker_values(&probe_markers, "preserved_path")),
        );
    }
    steps.push(step_entry("probe", "ok", None));

    // A sanitizer that failed its own probe means every string above is
    // suspect, including the active version this command decides on.
    if marker(&probe_markers, "sanitizer") != "ok" {
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            "the host's field sanitizer failed its own probe, so the version it reported \
             cannot be trusted to decide a delivery"
                .to_string(),
        ));
    }
    // A plan built for one platform must not be applied on another, and the
    // digest is per platform, so this is a wrong-artifact check as much as a
    // wrong-machine one.
    if host_platform != plan.platform {
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            format!(
                "target runs {host_platform} and this delivery is built for {}",
                plan.platform
            ),
        ));
    }

    // A matching version banner is not enough: the active file must still be
    // the retained staged image and every declared program unit must map it.
    // A proven match returns without restarting; stale or unobservable units
    // fall through to the ordinary stage/activate/restart path.
    let mut already_proven_units = BTreeMap::<String, Value>::new();
    if let Some(finished) = prove_active_image(
        target,
        &plan,
        &units,
        &probe_markers,
        probe.code,
        &active_version,
        &steps,
        &mut already_proven_units,
        &mut report,
        runner,
    )
    .await?
    {
        return Ok(finished);
    }

    if plan.dry_run {
        report.insert(
            "planned_steps".to_string(),
            json!(planned_steps(&plan, &units, &code_paths)),
        );
        report.insert("steps".to_string(), json!(steps));
        report.insert("exit_code".to_string(), json!(probe.code));
        report.insert("status".to_string(), json!(PLANNED_STATUS));
        return Ok(Value::Object(report));
    }

    // Phase two: fetch/verify/stage for ordinary delivery, or re-verify the
    // exact extracted digest persisted before an authority outage.
    let staged_sha256 = match stage_phase(
        target,
        &plan,
        pre_staged_sha256,
        &mut report,
        &mut steps,
        runner,
    )
    .await?
    {
        Ok(staged) => staged,
        Err(failure) => return Ok(failure),
    };
    let prior_unit_state = unit_state_before(target, &units, &mut report, runner).await?;

    // Phase three: activate. Reached only because phase two verified.
    let active_sha256 = match activate_phase(
        target,
        &plan,
        &staged_sha256,
        &mut report,
        &mut steps,
        runner,
    )
    .await?
    {
        Ok(active) => active,
        Err(failure) => return Ok(failure),
    };

    // Phase four: restart every declared unit, then join the new init-system
    // pid to the image digest activation just reported. The shared CLI's
    // installed version is not evidence about a service process.
    let (unit_processes, failures) = restart_units(
        target,
        &plan,
        &units,
        &already_proven_units,
        &prior_unit_state,
        &active_sha256,
        &mut steps,
        runner,
    )
    .await;
    if !failures.is_empty() {
        let exit_code = failures[0].0;
        let detail = failures
            .into_iter()
            .map(|(_, detail)| detail)
            .collect::<Vec<_>>()
            .join("; ");
        report.insert("steps".to_string(), json!(steps));
        report.insert("unit_processes".to_string(), json!(unit_processes));
        report.insert("activated".to_string(), json!(true));
        return Ok(fail(&mut report, exit_code, detail));
    }
    report.insert("unit_processes".to_string(), json!(unit_processes));

    // Every stable bind this host declares, proven listening before this
    // reports `ok`.
    //
    // A roll restarts the release agent, and the agent is the only thing that
    // publishes a stable bind. On 2026-09-03 two rolls reported `ok` with
    // every step `ok`, and Skarbiec's 127.0.0.1:8895 and Brama's
    // 127.0.0.1:8080 were both unbound behind them: the agent could not read
    // `release_control` through a closed object boundary, so it published
    // nothing, and `brama.wisent.com/health` answered 502 for hours while two
    // release reports said the roll had succeeded. A roll that restarts the
    // publisher of a serving port and does not look at the port is a roll
    // that cannot tell success from an outage.
    let (verdicts, missing) = verify_stable_binds(target, runner).await;
    if !verdicts.is_empty() {
        report.insert("stable_binds".to_string(), json!(verdicts));
    }
    report.insert("steps".to_string(), json!(steps));
    if !missing.is_empty() {
        report.insert("activated".to_string(), json!(true));
        return Ok(fail(
            &mut report,
            1,
            format!(
                "the release is active and the units restarted, but {} declared stable bind(s) are                  not listening after {STABLE_BIND_BUDGET_SECONDS}s: {}. The release agent                  publishes these ports; read its log with `stado host unit-log {}                  com.wisent.stado.release-agent` before rolling anything else",
                missing.len(),
                missing.join(", "),
                target.name
            ),
        ));
    }
    report.insert("exit_code".to_string(), json!(0));
    report.insert("status".to_string(), json!(RELEASED_STATUS));
    Ok(Value::Object(report))
}
