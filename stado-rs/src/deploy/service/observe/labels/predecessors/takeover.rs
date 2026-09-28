//! A catalog product's one process takes its place on the host when it
//! starts under its own unit.
//!
//! A unit the catalog retired can hold the very listeners its replacement
//! binds: a product whose unit was renamed runs the same program under the
//! old label. The replacement could then never come up beside it, and nothing
//! else would retire it, because the reconciler that retires predecessors runs
//! inside that old process. So before the process binds anything, every unit
//! the catalog retired for it that this host still loads is booted out and its
//! autostart withdrawn, the promise `retired_sentence` makes.

use crate::deploy::service::*;

use super::{retirement, PredecessorRetirement};

/// The unit the init system started this process under: launchd names the
/// job in `XPC_SERVICE_NAME`, systemd in the process's own cgroup path.
/// `None` for a process nothing started as a unit, such as a shell command.
fn own_unit() -> Option<String> {
    if let Ok(label) = std::env::var("XPC_SERVICE_NAME") {
        return Some(label);
    }
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    cgroup.lines().find_map(|line| {
        let unit = line.rsplit('/').next()?;
        unit.strip_suffix(".service").map(str::to_string)
    })
}

/// Retire, on this host, every unit the catalog lists as retired by the
/// product this process runs as, when the init system started it under that
/// product's unit. A process started any other way, or under a label the
/// catalog does not own, retires nothing: an old unit restarting must not boot
/// anything out, itself included.
async fn take_over_retired(runner: &Runner) -> Result<Vec<PredecessorRetirement>, DeployError> {
    let Some(unit) = own_unit() else {
        return Ok(Vec::new());
    };
    let Some(entry) = crate::deploy::service_catalog::lookup(&unit).map_err(DeployError)? else {
        return Ok(Vec::new());
    };
    if entry.unit.as_deref() != Some(unit.as_str()) || entry.retired_units.is_empty() {
        return Ok(Vec::new());
    }
    let target = this_host()?;
    let mut retirements = Vec::with_capacity(entry.retired_units.len());
    for retired in &entry.retired_units {
        retirements.push(retirement(&target, retired, runner).await);
    }
    Ok(retirements)
}

/// [`take_over_retired`] with the production runner, as every API listener
/// runs it before binding: each outcome goes to stderr, where the unit's log
/// keeps it, and a unit that stays loaded is the error the process exits
/// with, because it holds what the process is about to bind.
pub async fn take_over_on_start() -> Result<(), String> {
    let retirements = take_over_retired(&crate::deploy::production_runner())
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
fn this_host() -> Result<ComputeTarget, DeployError> {
    let hostname = crate::providers::vast::system_hostname();
    if hostname.is_empty() {
        return Err(DeployError(
            "this host's name could not be read, so its retired units cannot be addressed"
                .to_string(),
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
