//! Only roles that address a registry target require one before they start.
//! A standalone object API must be able to start before its own registry can
//! be read through that API.

use super::ServeArgs;
use crate::cli::hosts::agent;
use crate::cli::CmdError;
use crate::targets::ComputeTarget;

/// The one worker shape that is not a host's: an ephemeral cloud machine's
/// `--standalone --worker --kind <provider> --idle-shutdown`, which exits when
/// no eligible work remains. A persistent host process never idles out, and a
/// provider kind other than local is only ever such a machine.
pub(super) fn validate(args: &ServeArgs) -> Result<(), CmdError> {
    if args.worker.idle_shutdown && !args.standalone {
        return Err(CmdError::usage(
            "serve --idle-shutdown ends an ephemeral cloud machine's worker and needs \
             --standalone --worker; a registry host's process does not idle out",
        ));
    }
    if !crate::capabilities::ProviderId::Local.matches(&args.worker.kind)
        && !args.worker.idle_shutdown
    {
        return Err(CmdError::usage(format!(
            "serve --kind {} is an ephemeral cloud worker and needs --standalone --worker \
             --idle-shutdown; a registry host runs --kind local",
            args.worker.kind
        )));
    }
    Ok(())
}

pub(super) async fn resolve(args: &mut ServeArgs) -> Result<Option<ComputeTarget>, CmdError> {
    if args.standalone {
        // A device outside any fleet has no registry entry to resolve; its
        // worker reads the queue the configuration names.
        return Ok(None);
    }
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
    let target = target.ok_or_else(|| {
        CmdError::click("serve resolved no required registry target")
            .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
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
        .ok_or_else(|| {
            CmdError::click("resident role requires a resolved registry target")
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })
}
