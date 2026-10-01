//! A unit that already runs `stado serve` under another label — the
//! first-generation `com.wisent.always-on.stado-object-api` ran the worker,
//! the resolver, disk cleanup and the release agent that way — is the
//! product's own process under the wrong name. Its roles fold into the one
//! host unit: every role it runs the host unit runs too, and an option both
//! declare with different values is refused, because one process cannot run
//! both settings.

use std::fmt::Debug;

use crate::cli::integrations::runtime::ServeArgs;
use crate::deploy::DeployError;

use super::super::InstallPlan;

/// Take `theirs` for an option the host unit does not declare; refuse two
/// different declarations of it.
fn adopt<T: PartialEq + Debug>(
    ours: &mut Option<T>,
    theirs: Option<T>,
    option: &str,
    label: &str,
) -> Result<(), DeployError> {
    match (ours.as_ref(), theirs) {
        (_, None) => Ok(()),
        (None, Some(value)) => {
            *ours = Some(value);
            Ok(())
        }
        (Some(mine), Some(value)) if *mine == value => Ok(()),
        (Some(mine), Some(value)) => Err(DeployError(format!(
            "{label} declares {option} {value:?} but the host unit declares {mine:?}; one process cannot run both"
        ))),
    }
}

/// Fold `theirs`, a predecessor's `stado serve` arguments, into `runtime`.
/// Returns whether the predecessor ran the worker, which decides whose
/// environment variables are the worker's.
pub(super) fn merge(
    runtime: &mut ServeArgs,
    theirs: ServeArgs,
    component: &InstallPlan,
    host_name: &str,
) -> Result<bool, DeployError> {
    let label = &component.label;
    if theirs.run_worker {
        if let Some(target) = &theirs.worker.target {
            super::check_target(host_name, target, label)?;
        }
        if theirs.worker.idle_shutdown
            || !crate::capabilities::ProviderId::Local.matches(&theirs.worker.kind)
        {
            return Err(DeployError(format!(
                "{label} runs an ephemeral worker, not a resident host component"
            )));
        }
        if theirs.worker.poll_seconds.is_none() {
            return Err(DeployError(format!(
                "{label} runs the worker without --poll-seconds, which the worker requires; \
                 declare the host's one unit with `stado service ensure stado --host {host_name} \
                 --from <stado> --arg=serve --arg=--worker --arg=--poll-seconds=<N> …` instead"
            )));
        }
        let mut worker = theirs.worker;
        worker.target = Some(host_name.to_string());
        if runtime.run_worker && runtime.worker != worker {
            return Err(DeployError(format!(
                "{label} and the host unit disagree on worker options"
            )));
        }
        runtime.worker = worker;
        runtime.run_worker = true;
    }
    runtime.disk_cleanup |= theirs.disk_cleanup;
    runtime.resolver |= theirs.resolver;
    runtime.api |= theirs.api;
    runtime.watchdog |= theirs.watchdog;
    adopt(
        &mut runtime.failure_fixer_interval_seconds,
        theirs.failure_fixer_interval_seconds,
        "--failure-fixer-interval-seconds",
        label,
    )?;
    adopt(
        &mut runtime.failure_fixer_command_pattern,
        theirs.failure_fixer_command_pattern,
        "--failure-fixer-command-pattern",
        label,
    )?;
    adopt(
        &mut runtime.coordinator,
        theirs.coordinator,
        "--coordinator",
        label,
    )?;
    adopt(
        &mut runtime.control_plane,
        theirs.control_plane,
        "--control-plane",
        label,
    )?;
    adopt(
        &mut runtime.control_plane_interval_seconds,
        theirs.control_plane_interval_seconds,
        "--control-plane-interval-seconds",
        label,
    )?;
    adopt(&mut runtime.bind, theirs.bind, "--bind", label)?;
    adopt(&mut runtime.port, theirs.port, "--port", label)?;
    adopt(
        &mut runtime.release_interval_seconds,
        theirs.release_interval_seconds,
        "--release-interval-seconds",
        label,
    )?;
    adopt(
        &mut runtime.health_interval_seconds,
        theirs.health_interval_seconds,
        "--health-interval-seconds",
        label,
    )?;
    adopt(
        &mut runtime.product_sync_interval_seconds,
        theirs.product_sync_interval_seconds,
        "--product-sync-interval-seconds",
        label,
    )?;
    for surface in theirs.product_sync_surface {
        if !runtime.product_sync_surface.contains(&surface) {
            runtime.product_sync_surface.push(surface);
        }
    }
    adopt(
        &mut runtime.forward_destination,
        theirs.forward_destination,
        "--forward-destination",
        label,
    )?;
    adopt(
        &mut runtime.forward_remote_port,
        theirs.forward_remote_port,
        "--forward-remote-port",
        label,
    )?;
    adopt(
        &mut runtime.forward_local_port,
        theirs.forward_local_port,
        "--forward-local-port",
        label,
    )?;
    adopt(
        &mut runtime.forward_interval_seconds,
        theirs.forward_interval_seconds,
        "--forward-interval-seconds",
        label,
    )?;
    adopt(
        &mut runtime.edge_caddy,
        theirs.edge_caddy,
        "--edge-caddy",
        label,
    )?;
    adopt(
        &mut runtime.edge_caddyfile,
        theirs.edge_caddyfile,
        "--edge-caddyfile",
        label,
    )?;
    adopt(
        &mut runtime.precheck_runner,
        theirs.precheck_runner,
        "--precheck-runner",
        label,
    )?;
    adopt(
        &mut runtime.watchdog_bucket,
        theirs.watchdog_bucket,
        "--watchdog-bucket",
        label,
    )?;
    adopt(
        &mut runtime.watchdog_interval_seconds,
        theirs.watchdog_interval_seconds,
        "--watchdog-interval-seconds",
        label,
    )?;
    if theirs.api_storage.is_some() {
        return Err(DeployError(format!(
            "{label} declares --api-storage; the host unit's API serves the host's own store"
        )));
    }
    Ok(runtime.run_worker)
}
