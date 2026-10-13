//! The host Stado process takes the API listener over from the units that
//! held it before, when it starts under its own unit to serve the API.
//!
//! The units whose work is the API listener (units that run the host Stado
//! program as `serve --api` under another label, found on the host from what
//! they run) hold the very port the replacement binds: a unit that was
//! renamed runs the same program under the old label. The replacement could
//! never come up beside it, and nothing else would retire it, because the
//! reconciler that retires predecessors runs inside that old process. So
//! once the API's store is prepared and before it binds, each such unit this
//! host still loads is booted out and its autostart withdrawn, provided it
//! serves the same root. Nothing else
//! retires these units: a flag in a live argument vector proves neither a
//! bound listener nor the same root. Each retirement is recorded on the host
//! as `taken_over`, and that record is what keeps ensure and the reconciler
//! from repairing the old unit afterwards, see [`record::taken_over`].

use crate::deploy::service::*;

use super::record::{self, taken_over, TAKEN_OVER, WITHDRAWN};
use super::{retirement, PredecessorRetirement};
mod served_root;
mod unit_state;
use unit_state::{unit_state, UnitState};

/// The unit the init system started this process under: launchd names the
/// job in `XPC_SERVICE_NAME`, systemd in the process's own cgroup path.
/// `None` for a process nothing started as a unit, such as a shell command,
/// or on a host without `/proc`. A cgroup file that exists but cannot be
/// read is refused: read as "no unit", the takeover is skipped and the old
/// unit keeps the port the replacement must bind, with nothing saying why.
fn own_unit() -> Result<Option<String>, DeployError> {
    if let Ok(label) = std::env::var("XPC_SERVICE_NAME") {
        return Ok(Some(label));
    }
    let path = "/proc/self/cgroup";
    let cgroup = match std::fs::read_to_string(path) {
        Ok(cgroup) => cgroup,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DeployError(format!("cannot read {path}: {error}"))),
    };
    Ok(cgroup.lines().find_map(|line| {
        let unit = line.rsplit('/').next()?;
        unit.strip_suffix(".service").map(str::to_string)
    }))
}

/// Retire, on this host, every API listener predecessor of the host Stado
/// process, when this process is that unit's own main process and is
/// about to serve the object API from `served_root`. Both the launchd label
/// and the cgroup are inherited by children, so the name alone proves
/// nothing: the init system must also bind the unit to this pid. A process
/// started any other way, a child of any unit, or an old unit restarting
/// retires nothing.
async fn take_over_retired(
    served_root: Option<&str>,
    runner: &Runner,
) -> Result<Vec<PredecessorRetirement>, DeployError> {
    let Some(unit) = own_unit()? else {
        return Ok(Vec::new());
    };
    let entry = crate::deploy::service_catalog::host_process().map_err(|message| {
        DeployError(message).stating(crate::primitives::failure::FailureCode::Config)
    })?;
    if entry.unit.as_deref() != Some(unit.as_str()) {
        return Ok(Vec::new());
    }
    let owner = match unit_state(&unit) {
        Ok(state) => state.as_ref().and_then(UnitState::main_pid),
        Err(error) => {
            eprintln!(
                "[stado] {unit}: its init system could not be read, so this pid is not proven \
                 its main process and its predecessors are left alone: {error}"
            );
            return Ok(Vec::new());
        }
    };
    if owner != Some(std::process::id()) {
        eprintln!(
            "[stado] {unit} names pid {owner:?} as its main process, not this pid {}; \
             its predecessors are left to that process",
            std::process::id()
        );
        return Ok(Vec::new());
    }
    let target = local_target()?;
    // A scan that fails names its failure and retires nothing: a unit still
    // holding the port then fails this process's bind with its own error,
    // and a host with no predecessor is not kept from serving by a read.
    let predecessors = match api_predecessors_on(&target, runner).await {
        Ok(predecessors) => predecessors,
        Err(error) => {
            eprintln!("[stado] {unit}: this host's units could not be read, none retired: {error}");
            Vec::new()
        }
    };
    let mut retirements = Vec::with_capacity(predecessors.len());
    for retired in &predecessors {
        let retired = retired.as_str();
        // A predecessor serving another storage root is an authority change,
        // which only `stado host storage-root-reconcile` may make.
        let refusal = match unit_state(retired) {
            Ok(state) => state.and_then(|state| state.other_root(served_root)),
            Err(error) => Some(format!(
                "its state could not be read, so it is not proven to serve this root: {error}"
            )),
        };
        if let Some(refusal) = refusal {
            retirements.push(PredecessorRetirement {
                unit: retired.to_string(),
                state: "failed".to_string(),
                detail: format!(
                    "{refusal}; moving the object store is `stado host storage-root-reconcile`, \
                     not a takeover"
                ),
            });
            continue;
        }
        // The record is written before the unit is retired, so a repair that
        // re-reads it after its own ensure sees it and retires the unit again
        // (see [`retire_if_taken_over`]); a retirement that fails withdraws it.
        let pid = std::process::id().to_string();
        if let Err(error) =
            record::write_record(&target, retired, TAKEN_OVER, &[], &pid, runner).await
        {
            retirements.push(PredecessorRetirement {
                unit: retired.to_string(),
                state: "failed".to_string(),
                detail: format!(
                    "its takeover could not be recorded, so the reconciler would repair it: {error}"
                ),
            });
            continue;
        }
        let outcome = retirement(&target, retired, runner).await;
        if outcome.state == "failed" {
            if let Err(error) =
                record::write_record(&target, retired, WITHDRAWN, &[], &pid, runner).await
            {
                eprintln!("[stado] predecessor {retired}: its takeover record stays: {error}");
            }
        }
        retirements.push(outcome);
    }
    Ok(retirements)
}

/// Retire `unit` on `target` again when its takeover is recorded: what a
/// repair runs after its own ensure, because a takeover that started while
/// the repair was under way recorded itself before retiring, and the ensure
/// may have brought the unit back after that retirement. `None` when no
/// takeover is recorded.
pub async fn retire_if_taken_over(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Option<PredecessorRetirement> {
    taken_over(target, unit, runner).await?;
    Some(retirement(target, unit, runner).await)
}

/// [`take_over_retired`] with the production runner, run by an API listener
/// once its storage is prepared and before it binds, with the local root that
/// storage serves (`None` for a store with no local root). Each outcome goes
/// to stderr, where the unit's log keeps it, and a unit that stays loaded is
/// the error the process exits with, because it holds what the process is
/// about to bind.
pub async fn take_over_on_start(served_root: Option<&str>) -> Result<(), String> {
    let retirements = take_over_retired(served_root, &crate::deploy::production_runner())
        .await
        .map_err(|error| format!("could not retire this process's predecessors: {error}"))?;
    let mut failed = Vec::new();
    for retirement in retirements {
        eprintln!(
            "[stado] predecessor {}: {} ({})",
            retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "refusing to start beside predecessors that could not be retired: {}",
        failed.join("; ")
    ))
}

/// This machine as a target of the host channel, which then runs every
/// script locally: the name and the one hostname the channel matches on.
pub fn local_target() -> Result<ComputeTarget, DeployError> {
    let hostname = crate::providers::vast::system_hostname();
    if hostname.is_empty() {
        return Err(DeployError::unreachable(
            "this host's name could not be read, so its units cannot be addressed".to_string(),
        ));
    }
    serde_json::from_value(serde_json::json!({
        "name": hostname,
        "kind": "local",
        "hostnames": [hostname],
    }))
    .map_err(|error| {
        DeployError(format!(
            "this host could not be described as a target: {error}"
        ))
    })
}
