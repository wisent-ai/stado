//! An ensure that installs a unit as a launchd daemon while the login still
//! runs it as a LaunchAgent moves it: the agent is stopped before the daemon
//! starts, the daemon is read until it answers, and only then is the agent's
//! file removed. A daemon that does not answer is taken down and the agent
//! loaded again from the file that was kept.

use super::*;

use crate::cli::service::lifecycle::release::awaited::{until_serving, Served};

/// Stop the LaunchAgent `plan` would start beside, when `plan` installs a
/// daemon and the login holds one. A stop that fails gives back the units
/// [`predecessors::retire_before_ensure`] retired, and nothing was started.
pub(super) async fn stop_agent(
    target: &crate::targets::ComputeTarget,
    plan: &service::DeployPlan,
    retired: &[service::Reversible],
    runner: &crate::deploy::Runner,
) -> Result<Option<service::AgentTwin>, CmdError> {
    if !plan.force_daemon || !target.release_platform.starts_with("darwin") {
        return Ok(None);
    }
    let Some(twin) = service::agent_twin(target, &plan.label, runner)
        .await
        .map_err(click)?
    else {
        return Ok(None);
    };
    match service::stop_agent_twin(target, &plan.label, runner).await {
        Ok(said) => {
            eprintln!(
                "{}: {} stopped before its daemon starts: {said}",
                target.name,
                twin.describe(&plan.label)
            );
            Ok(Some(twin))
        }
        Err(error) => {
            let given_back = service::reinstate_units(target, None, retired, runner).await;
            let mut refusal = click(error);
            if !given_back.is_empty() {
                refusal = refusal.also(format!(
                    "the units retired for it were given back: {}",
                    given_back.join("; ")
                ));
            }
            Err(refusal)
        }
    }
}

/// `refusal` from an ensure that stopped `twin` first, with the agent put
/// back.
pub(super) async fn restored(
    target: &crate::targets::ComputeTarget,
    plan: &service::DeployPlan,
    twin: Option<&service::AgentTwin>,
    refusal: CmdError,
    runner: &crate::deploy::Runner,
) -> CmdError {
    let Some(twin) = twin else {
        return refusal;
    };
    let said = service::restore_agent_twin(target, &plan.label, twin, runner).await;
    refusal.also(format!("the LaunchAgent was put back: {said}"))
}

/// Read the daemon `record` names until it answers, then remove the agent's
/// file. A daemon that does not answer is taken down, the units retired for
/// it are given back and the agent is loaded again, and the ensure fails
/// naming why.
pub(super) async fn settle_agent(
    target: &crate::targets::ComputeTarget,
    plan: &service::DeployPlan,
    twin: &service::AgentTwin,
    record: &service::ManagedService,
    retired: &[service::Reversible],
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let answered = match until_serving(target, record, runner).await {
        Ok(Served::Serving) => Ok("it serves its declared port".to_string()),
        Ok(Served::NoDeclaredPort) => Ok(format!(
            "its service directory entry declares no port on {}, so the live process launchd \
             holds for it is the answer",
            target.name
        )),
        Ok(Served::Failed(why)) => Err(why),
        Err(error) => Err(error.to_string()),
    };
    match answered {
        Ok(said) => {
            let removed = service::remove_agent_twin(target, twin, runner)
                .await
                .map_err(|error| {
                    click(error).also(format!(
                        "{} answers as a launchd daemon at {}, and {} is still on disk",
                        plan.label,
                        record.path,
                        twin.describe(&plan.label)
                    ))
                })?;
            eprintln!(
                "{}: {} moved into the system domain at {}: {said}; {removed}",
                target.name, plan.label, record.path
            );
            Ok(())
        }
        Err(why) => {
            let given_back =
                predecessors::reinstate_after_failed_ensure(target, &plan.label, retired, runner)
                    .await;
            let refusal = CmdError::unreachable(format!(
                "{}: {} did not answer as a launchd daemon at {}: {why}{given_back}",
                target.name, plan.label, record.path
            ));
            Err(restored(target, plan, Some(twin), refusal, runner).await)
        }
    }
}
