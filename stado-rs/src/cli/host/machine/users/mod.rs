//! Accounts, credentials and runner limits on one host.

pub(in crate::cli::host) mod accounts;
pub(in crate::cli::host) mod credentials;
pub(in crate::cli::host) mod runners;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::probes::{print_json, report_outcome};

/// `stado host reboot TARGET` — request a graceful reboot through the
/// approved channel (`stado.wisent.com/docs/missing-commands` item one).
///
/// [`crate::deploy::host_state::reboot`] has been complete since July but was
/// never reachable: `deploy/mod.rs` did not declare the module and no CLI
/// variant dispatched to it, so the command the incident write-up records
/// as shipped did not exist. Both halves are wired now.
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
        .ok_or_else(|| CmdError::click(format!("unknown registry target: {target}")))
}
