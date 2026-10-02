//! Accounts, credentials and runner limits on one host.

pub(in crate::cli::host) mod accounts;
pub(in crate::cli::host) mod credentials;
pub(in crate::cli::host) mod runners;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::probes::{print_json, report_outcome};

/// `stado host reboot TARGET` — request a graceful reboot through the
/// registered host channel and report whether the request was accepted.
pub async fn reboot(target: &str) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_state::reboot::reboot_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    print_json(&report);
    report_outcome(&report, "reboot_requested")
}

/// Resolve TARGET in the canonical registry used by declaration-backed host
/// operations.
async fn registry_target(target: &str) -> Result<ComputeTarget, CmdError> {
    let registry = crate::cli::registry::read_registry().await?;
    registry
        .targets
        .iter()
        .find(|candidate| candidate.name == target)
        .cloned()
        .ok_or_else(|| CmdError::refused(format!("unknown registry target: {target}")))
}
