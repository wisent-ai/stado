//! Collecting one host's inventory over the shared channel.

use serde_json::{json, Value};

use super::super::*;
use crate::deploy::host_channel;
use crate::deploy::{DeployError, Runner};
use crate::targets::{ComputeTarget, ServiceDirectory};

/// Collect the inventory of one already-resolved registry target, against
/// the service directory of the registry it came from.
///
/// Split out from [`inventory_host`] so the whole command can be exercised
/// through the [`Runner`] seam without a registry or a remote host.
pub async fn inventory_target(
    target: &ComputeTarget,
    directory: Option<&ServiceDirectory>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_script(target, &remote_inventory_script()?, runner).await?;
    let parsed = parse_inventory(&output.stdout);
    let mut report = match &parsed {
        Ok(inventory) => to_report(target, directory, inventory),
        Err(_) => host_channel::base_report(target),
    };
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    // A clean exit with an unreadable payload is its own failure, and a
    // different one from a broken channel: the host answered, and what it
    // answered was not this command's report. A non-zero exit keeps the
    // remote's own last stderr line, which explains more.
    if let (Err(error), true) = (&parsed, output.ok()) {
        report.insert(
            "status".to_string(),
            json!(host_channel::FAILED_STATUS.to_string()),
        );
        report.insert("error".to_string(), json!(error.0));
    }
    Ok(Value::Object(report))
}

/// Collect the inventory of one canonical registry host.
///
/// The whole registry is loaded rather than just the target, because the
/// declaration this command reconciles against lives in two places in the
/// same document: `targets[].managed_versions` and `service_directory`.
/// Comparing a host against a directory from a different read is comparing
/// it against a state that may never have existed.
pub async fn inventory_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let target = host_channel::resolve_target(&registry, target_name)?.clone();
    inventory_target(&target, registry.service_directory.as_ref(), runner).await
}
