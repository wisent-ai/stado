//! The reconcilers' own supervision: a declared coordinator or release-agent
//! unit is ensured by the queue agent on the host that declares it.
//!
//! Service reconciliation is what loads a declared unit that is not running,
//! and it runs inside the coordinator's tick. When the unit that runs the
//! coordinator is itself unloaded nothing restores it: with it and the
//! release agent's unit both unloaded, no schedule fires, no queued job is
//! dispatched or reaped and no release is applied on that host, while the
//! object API and its queue agent run on.
//!
//! The queue agent is the process launchd keeps alive on every host that
//! claims work, so it asserts these two units through the same `ensure`
//! repair the reconciler uses for any other declared unit. `ensure` is
//! idempotent: a unit already running its declared program is left alone.

use crate::deploy::service::{self, ManagedService, ServiceStatus};

/// A declared unit that runs a reconciler: `stado coordinator …` or
/// `stado release agent …`. Named by its argument vector, which the registry
/// declares, rather than by label, which differs per host.
fn runs_reconciler(declared: &ManagedService) -> bool {
    let args: Vec<&str> = declared.args.iter().map(String::as_str).collect();
    matches!(
        args.as_slice(),
        ["coordinator", ..] | ["release", "agent", ..]
    )
}

/// Ensure every reconciler unit this host declares, logging each outcome.
///
/// Refusals are logged and never returned: this pass shares the janitor's
/// thread, and a registry or host-channel failure here must not stop disk
/// cleanup or the memory pass.
pub async fn restore_reconcilers(log: &mut dyn FnMut(&str)) {
    let here = match crate::cli::release_catalog::this_host().await {
        Ok(here) => here,
        Err(error) => {
            log(&format!(
                "reconcilers: this host has no registry target, nothing ensured: {error}"
            ));
            return;
        }
    };
    let registry = match crate::cli::registry::read_registry().await {
        Ok(registry) => registry,
        Err(error) => {
            log(&format!(
                "reconcilers: the registry could not be read, nothing ensured: {error}"
            ));
            return;
        }
    };
    let Some(target) = registry
        .local_targets()
        .into_iter()
        .find(|target| target.name == here)
    else {
        return;
    };
    let runner = crate::deploy::production_runner();
    for declared in service::declared_services(target)
        .into_iter()
        .filter(runs_reconciler)
    {
        let unit = declared.unit_id().to_string();
        let status = ServiceStatus {
            service: declared,
            state: service::STATE_UNKNOWN.to_string(),
            reported_at: String::new(),
            detail: String::new(),
            misdeclared_domain: None,
        };
        match super::repair::reconcile_unreachable(&status, target, &runner).await {
            Ok((action, true, detail)) => {
                log(&format!(
                    "reconcilers: {unit} on {here}: {action}: {detail}"
                ));
            }
            Ok(_) => {}
            Err(error) => log(&format!(
                "reconcilers: {unit} on {here} is not running and ensure did not start it: {error}"
            )),
        }
    }
}
