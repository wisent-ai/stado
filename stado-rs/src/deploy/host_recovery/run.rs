//! Carrying one pass to the host, and resolving which host it may run on.

use std::time::Duration;

use serde_json::{json, Value};

use super::plan::{plan_stable_binds, StableBindPlan};
use super::program::remote_script_with_stable_binds;
use super::report::parse_output;
use super::TIMEOUT_SECONDS;
use crate::deploy::{host_channel, py_str_repr, DeployError, Runner};
use crate::targets::{ComputeTarget, Registry};

/// Python `_target`: resolve a canonical kind=local registry host.
fn resolve_target<'a>(
    registry: &'a Registry,
    target_name: &str,
) -> Result<&'a ComputeTarget, DeployError> {
    let Some(target) = registry.lookup(target_name) else {
        return Err(DeployError(format!(
            "target {} is not in the canonical registry",
            py_str_repr(target_name)
        )));
    };
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return Err(DeployError(format!(
            "target {} is not a local host",
            py_str_repr(target_name)
        )));
    }
    if target
        .weles
        .as_ref()
        .is_some_and(|policy| policy.actions.iter().any(|action| action == "*"))
    {
        return Err(DeployError(format!(
            "target {} carries forbidden wildcard recovery state",
            py_str_repr(target_name)
        )));
    }
    if !target.has_ssh_connection() {
        return Err(DeployError(format!(
            "target {} has no registry-managed ssh destination",
            py_str_repr(target_name)
        )));
    }
    Ok(target)
}

/// [`recover_host`] against an already-loaded registry (Python's
/// `lookup(target_name, source="gcs")` is the caller's concern here).
pub async fn recover_host_with_registry(
    registry: &Registry,
    target_name: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let target = resolve_target(registry, target_name)?;
    // The stable-bind stage is the one part of this pass that needs the
    // registry document rather than the target, and it is the part that can
    // put a serving port back after a roll left it unbound.
    let stable_binds: Vec<StableBindPlan> = plan_stable_binds(&registry.to_document(), target);
    let output = host_channel::run_script_with_timeout(
        target,
        &remote_script_with_stable_binds(target, &stable_binds),
        Duration::from_secs(TIMEOUT_SECONDS),
        runner,
    )
    .await?;
    let mut report = parse_output(&output.stdout, target)?;
    report["exit_code"] = json!(output.code);
    if output.code != 0 {
        let detail = output.detail().trim();
        let error = match detail.lines().next_back() {
            Some(last) => last.chars().take(300).collect::<String>(),
            None => "remote recovery failed".to_string(),
        };
        report["error"] = json!(error);
    }
    Ok(report)
}

/// Python `recover_host`: run the fixed recovery procedure on one
/// canonical registry host, resolved through
/// [`crate::deploy::host_channel::canonical_registry`] — the canonical store
/// first, the last-known-good copy with its age announced when the store does
/// not answer, never an empty registry. Recovering a host you cannot reach the
/// registry for is the case this command exists for.
pub async fn recover_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    recover_host_with_registry(&registry, target_name, runner).await
}
