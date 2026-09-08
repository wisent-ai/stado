//! Promotion: drive every rollout target of the promoted release onto the
//! exact published coordinate, and record what converged.

mod receipt;
mod replace;

use std::time::Duration;

use chrono::Utc;
use serde_json::json;

use crate::cli::release_submit::publish::promotion::receipt::queue_deployment_receipt;
use crate::cli::release_submit::publish::promotion::replace::{
    replace_service, replace_status_exact,
};
use crate::cli::release_submit::run::source::run_path;
use crate::cli::CmdError;
use crate::release_control::{self, StrategyKind};
use crate::release_pipeline::ReleaseRun;

pub(crate) async fn reconcile(run: &ReleaseRun) -> Result<(), CmdError> {
    // Every poll runs the target's own release agent once, and that run is
    // what advances the rollout state machine. The wait therefore has to
    // cover the phases the agent must pass through, and the last of them is
    // the product's own declared rollback window: the agent leaves
    // Monitoring for Committed only once `rollback_window_seconds` have
    // elapsed since cutover. A fixed ceiling cannot express that. Twenty-four
    // polls five seconds apart gave 120s, while brama, image-video-router and
    // weles-worker each declare a 300s window in the same document read
    // below, so a healthy rollout of any of them reported "did not converge"
    // every single time.
    const POLL_INTERVAL: Duration = Duration::from_secs(5);
    // Headroom for the agent runs themselves and for the poll that observes
    // the commit, which can only be the first one after the window closes.
    const CONVERGENCE_SLACK: Duration = Duration::from_secs(60);

    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("release control disappeared"))?;
    let policy = control
        .products
        .get(&run.product)
        .ok_or_else(|| CmdError::click("release product has no rollout policy"))?;
    let desired = policy
        .desired
        .as_ref()
        .ok_or_else(|| CmdError::click("promoted release has no desired coordinate"))?;
    if desired.version != run.version {
        return Err(CmdError::click(format!(
            "promoted release is {}, not {}",
            desired.version, run.version
        )));
    }
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|e| CmdError::click(e.to_string()))?;
    let runner = crate::deploy::production_runner();
    let mut observed = Vec::new();
    for name in policy.targets.keys() {
        let target = registry
            .targets
            .iter()
            .find(|t| &t.name == name)
            .ok_or_else(|| CmdError::click(format!("rollout target {name} is absent")))?;
        let expected = run
            .platforms
            .get(&policy.targets[name].platform)
            .ok_or_else(|| CmdError::click("rollout platform was not built"))?;
        if policy.strategy.kind == StrategyKind::Replace {
            let artifact_sha256 = expected
                .artifact_sha256
                .as_deref()
                .ok_or_else(|| CmdError::click("rollout artifact digest was not recorded"))?;
            let manifest_sha256 = expected
                .release_manifest_sha256
                .as_deref()
                .ok_or_else(|| CmdError::click("rollout manifest digest was not recorded"))?;
            if !replace_status_exact(
                &run.product,
                name,
                desired.rollout_generation,
                &run.version,
                artifact_sha256,
            )
            .await
            {
                // Same default the validator applies, read through the same
                // constant: a replace target may omit the key, and a submit
                // that refused what validation accepts would be the second
                // reader of one contract disagreeing with the first.
                let readiness_path = policy.targets[name]
                    .readiness_path
                    .as_deref()
                    .unwrap_or(crate::release_control::DEFAULT_REPLACE_READINESS_PATH);
                let (service, readiness_url) =
                    replace_service(&document, &policy.service, name, readiness_path)?;
                crate::cli::service::release_pipeline_product(
                    &service,
                    name,
                    &run.product,
                    &run.version,
                    &readiness_url,
                    policy.strategy.readiness_timeout_seconds,
                )
                .await?;
            }
            if !replace_status_exact(
                &run.product,
                name,
                desired.rollout_generation,
                &run.version,
                artifact_sha256,
            )
            .await
            {
                return Err(CmdError::click(format!(
                    "target {name} did not publish committed replace status for {} generation {}",
                    run.version, desired.rollout_generation
                )));
            }
            observed.push(json!({
                "target": name,
                "version": run.version,
                "artifact_sha256": artifact_sha256,
                "manifest_sha256": manifest_sha256
            }));
            continue;
        }
        let script = format!(
            "set -eu\n\
             if [ -x /bin/systemctl ] && /bin/systemctl is-active --quiet wisent-agent.service; then\n\
               environment=$(/bin/systemctl show wisent-agent.service --property=Environment --value)\n\
               /usr/bin/env -S \"$environment\" \"$HOME/.stado/bin/stado\" release agent --target {} --product {} --once --json\n\
             else\n\
               \"$HOME/.stado/bin/stado\" release agent --target {} --product {} --once --json\n\
             fi\n",
            crate::deploy::shlex_quote(name),
            crate::deploy::shlex_quote(&run.product),
            crate::deploy::shlex_quote(name),
            crate::deploy::shlex_quote(&run.product)
        );
        let mut last_observation = "product state was not returned".to_string();
        let mut converged = false;
        let budget = Duration::from_secs(
            policy
                .strategy
                .readiness_timeout_seconds
                .saturating_add(policy.strategy.drain_timeout_seconds)
                .saturating_add(policy.strategy.rollback_window_seconds),
        ) + CONVERGENCE_SLACK;
        let deadline = std::time::Instant::now() + budget;
        loop {
            let output = crate::deploy::host_channel::run_script(target, &script, &runner)
                .await
                .map_err(|e| CmdError::click(e.to_string()))?;
            if !output.ok() {
                return Err(CmdError::click(format!(
                    "reconciliation failed on {name}: {}",
                    output.detail()
                )));
            }
            let states: Vec<crate::release_agent::HostReleaseState> =
                serde_json::from_str(output.stdout.trim())?;
            if let Some(state) = states
                .into_iter()
                .find(|state| state.product == run.product)
            {
                if state.rollout_generation > desired.rollout_generation {
                    return Err(CmdError::click(format!(
                        "target {name} advanced to rollout generation {}, beyond {}",
                        state.rollout_generation, desired.rollout_generation
                    )));
                }
                let exact = state.rollout_generation == desired.rollout_generation
                    && state.active.as_ref().is_some_and(|active| {
                        active.version == run.version
                            && Some(active.artifact_sha256.as_str())
                                == expected.artifact_sha256.as_deref()
                            && Some(active.manifest_sha256.as_str())
                                == expected.release_manifest_sha256.as_deref()
                    });
                // An exact active process is still reversible during Monitoring.
                // Record deployment only after the rollout window commits.
                if exact && matches!(state.phase, crate::release_agent::RolloutPhase::Committed) {
                    let active = state.active.as_ref().expect("checked above");
                    observed.push(json!({
                        "target": name,
                        "version": active.version,
                        "artifact_sha256": active.artifact_sha256,
                        "manifest_sha256": active.manifest_sha256
                    }));
                    converged = true;
                    break;
                }
                if state.rollout_generation == desired.rollout_generation
                    && matches!(
                        state.phase,
                        crate::release_agent::RolloutPhase::RolledBack
                            | crate::release_agent::RolloutPhase::Failed
                            | crate::release_agent::RolloutPhase::Quarantined
                    )
                {
                    return Err(CmdError::click(format!(
                        "target {name} refused rollout generation {} in phase {:?}: {}",
                        desired.rollout_generation, state.phase, state.detail
                    )));
                }
                last_observation = format!(
                    "generation={} phase={:?} active={} detail={}",
                    state.rollout_generation,
                    state.phase,
                    state
                        .active
                        .as_ref()
                        .map(|active| active.version.as_str())
                        .unwrap_or("-"),
                    state.detail
                );
            }
            if std::time::Instant::now() + POLL_INTERVAL >= deadline {
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        if !converged {
            return Err(CmdError::click(format!(
                "target {name} did not converge to {} generation {} within {}s \
                 (readiness {}s + drain {}s + declared rollback window {}s): \
                 {last_observation}",
                run.version,
                desired.rollout_generation,
                budget.as_secs(),
                policy.strategy.readiness_timeout_seconds,
                policy.strategy.drain_timeout_seconds,
                policy.strategy.rollback_window_seconds
            )));
        }
    }
    let receipt = serde_json::to_vec(
        &json!({"schema_version":1,"run_id":run.run_id,"product":run.product,"version":run.version,"targets":observed,"completed_at":Utc::now().to_rfc3339()}),
    )?;
    queue_deployment_receipt(
        &run_path(&run.product, &run.run_id, "deployment.json"),
        &receipt,
    )
    .await
}
