//! An ensure of this machine's own `stado serve --resolver` restarts the
//! process that serves the resolver adapter this command reads the registry
//! through, when its storage backend is the client route `stado`. The
//! record of the pass is written through that very adapter, so between the
//! restart and the resolver's `serving` publication every write answered
//! "connection refused" and the pass ended with "recording the completed
//! ensure failed" on every run. The record is made after the restarted
//! process publishes that it serves, on its own verdict and with no interval
//! of this command's choosing.

use crate::deploy::service::{DeployPlan, EnsureOutcome};
use crate::targets::ComputeTarget;

/// Whether `plan`, acted on at `target`, is this machine's own resolver and
/// this process reads the registry through a resolver adapter.
fn carries_this_process_route(target: &ComputeTarget, plan: &DeployPlan) -> bool {
    let hostname = crate::providers::vast::system_hostname();
    let this_machine = target.hostnames.contains(&hostname);
    let runs_resolver = plan
        .argv
        .split_whitespace()
        .any(|argument| argument == "--resolver");
    let through_client_route = matches!(
        crate::config::wc_storage_backend(),
        "stado" | "stado-object"
    );
    this_machine && runs_resolver && through_client_route
}

/// Wait for the route the pass's record goes through, when the pass just
/// restarted the process that carries it; a no-op otherwise.
pub(super) fn await_route(
    target: &ComputeTarget,
    plan: &DeployPlan,
    outcome: &EnsureOutcome,
    name: &str,
) -> Result<(), String> {
    if !carries_this_process_route(target, plan) {
        return Ok(());
    }
    let Ok(pid) = outcome.pid.trim().parse::<u32>() else {
        return Ok(());
    };
    crate::cli::resolver::await_serving(pid).map_err(|cause| {
        format!(
            "{name} is running (action {}, pid {pid}), but the resolver it carries, which this \
             command records through, did not come to serve: {cause}",
            outcome.action
        )
    })
}
