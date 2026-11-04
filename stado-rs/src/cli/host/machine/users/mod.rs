//! Accounts, credentials and runner limits on one host.

pub(in crate::cli::host) mod accounts;
pub(in crate::cli::host) mod credentials;
pub(in crate::cli::host) mod runners;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::probes::{print_json, report_outcome};

/// `stado host reboot TARGET` — request a graceful reboot through the
/// registered host channel and report whether the request was accepted.
///
/// A host listed on Vast is announced first: Vast tells every renter with
/// an active contract that the machine goes down, so they save their work
/// and the host's reliability score is spared. A host this Vast account
/// does not list is rebooted without a notice; a notice Vast refuses stops
/// the reboot, naming the refusal, because rebooting under a contract
/// unannounced is what the notice exists to prevent.
pub async fn reboot(target: &str, maintenance_hours: f64) -> Result<(), CmdError> {
    announce_reboot(target, maintenance_hours).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_state::reboot::reboot_host(target, &runner)
        .await
        .map_err(CmdError::from)?;
    print_json(&report);
    report_outcome(&report, "reboot_requested")
}

/// Tell Vast that `target`'s machine goes down now for `hours`, when this
/// fleet holds a Vast key and the account lists a machine with one of the
/// target's hostnames.
async fn announce_reboot(target: &str, hours: f64) -> Result<(), CmdError> {
    if !crate::providers::vast::vast_api_key_available().await {
        return Ok(());
    }
    let host = registry_target(target).await?;
    let mut hostnames = host.hostnames.clone();
    hostnames.push(host.name.clone());
    let client = crate::providers::vast::VastClient::from_env()
        .await
        .map_err(|error| CmdError::refused(format!("the Vast key cannot be used: {error}")))?;
    let machine = client.listed_machine(&hostnames).await.map_err(|error| {
        CmdError::refused(format!(
            "whether Vast lists {target} cannot be read, so its renters cannot be told before the \
             reboot: {error}"
        ))
    })?;
    let Some(machine) = machine else {
        return Ok(());
    };
    let answer = client
        .schedule_maintenance_on_machine(machine, chrono::Utc::now().timestamp(), hours, "software")
        .await
        .map_err(|error| {
            CmdError::refused(format!(
                "Vast refused the maintenance notice for machine {machine} of {target}, so the \
                 reboot is not requested: {error}"
            ))
        })?;
    eprintln!("told Vast machine {machine} goes down for {hours} hour(s): {answer}");
    Ok(())
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
