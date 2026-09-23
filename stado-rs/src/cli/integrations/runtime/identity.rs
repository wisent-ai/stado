//! Only roles that address a registry target require one before they start.
//! A standalone object API must be able to start before its own registry can
//! be read through that API.

use super::ServeArgs;
use crate::cli::hosts::agent;
use crate::cli::CmdError;
use crate::targets::ComputeTarget;

pub(super) fn validate(args: &ServeArgs) -> Result<(), CmdError> {
    if args.worker.idle_shutdown {
        return Err(CmdError::usage(
            "serve owns persistent host services; --idle-shutdown belongs to an ephemeral worker",
        ));
    }
    if !crate::capabilities::ProviderId::Local.matches(&args.worker.kind) {
        return Err(CmdError::usage(
            "serve requires --kind local; ephemeral cloud workers use agent",
        ));
    }
    Ok(())
}

pub(super) async fn resolve(args: &mut ServeArgs) -> Result<Option<ComputeTarget>, CmdError> {
    let needs_target = args.run_worker
        || args.resolver
        || args.release_interval_seconds.is_some()
        || args.worker.target.is_some()
        || args.worker.auto;
    if !needs_target {
        return Ok(None);
    }
    let auto = args.worker.auto || args.worker.target.is_none();
    let environment = if args.run_worker {
        agent::RegistryEnvironment::ResidentHost
    } else {
        agent::RegistryEnvironment::HostIdentity
    };
    let (gpu_type, target) = agent::apply_registry_target(
        std::mem::take(&mut args.worker.gpu_type),
        args.worker.target.as_deref(),
        auto,
        environment,
    )
    .await?;
    let target =
        target.ok_or_else(|| CmdError::click("serve resolved no required registry target"))?;
    if auto {
        if let Some(expected) = args.worker.target.as_deref() {
            if target.name != expected {
                return Err(CmdError::usage(format!(
                    "serve --auto resolved host {}, but this resident declaration targets {expected}",
                    target.name
                )));
            }
        }
    }
    if !crate::capabilities::ProviderId::Local.matches(&target.kind) {
        return Err(CmdError::usage(format!(
            "serve target {} has kind {}; expected local",
            target.name, target.kind
        )));
    }
    args.worker.gpu_type = gpu_type;
    args.worker.target = None;
    args.worker.auto = false;
    Ok(Some(target))
}

pub(super) fn required_name(target: &Option<ComputeTarget>) -> Result<String, CmdError> {
    target
        .as_ref()
        .map(|target| target.name.clone())
        .ok_or_else(|| CmdError::click("resident role requires a resolved registry target"))
}
