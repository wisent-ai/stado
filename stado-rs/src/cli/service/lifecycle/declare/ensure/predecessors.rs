//! After a catalog product's unit is ensured, the units it replaced are
//! retired on the same host, so asserting the product is also what takes its
//! predecessors down.

use super::*;

/// Retire every unit `entry` lists in `retired_units` on `target`, and each
/// of its `role_units` whose role the just-ensured `running` unit's live
/// process is proven to run, and say what happened to each on stderr,
/// leaving the command's JSON contract unchanged. A unit that could not be
/// retired is the command's failure: the replacement runs, but its
/// predecessor may run beside it. A kept role unit is not a failure. A role
/// that shares its unit's listener is not touched here: the reconciler hands
/// it over under the unit's lease and can repair it if the role does not
/// take, so this command only says so. The API listener's units are kept
/// until the host Stado process records its takeover, and retired after.
pub(super) async fn retire_after_ensure(
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    running: &crate::deploy::service::ManagedService,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let Some(entry) = entry else { return Ok(()) };
    let mut failed = Vec::new();
    for retirement in service::retire_catalog_predecessors(target, entry, running, runner).await {
        eprintln!(
            "{}: {} replaced {}: {} ({})",
            target.name, entry.name, retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    for role in entry
        .role_units
        .iter()
        .filter(|role| service::listener_role(role))
    {
        eprintln!(
            "{}: {} replaces {}: left to the autonomy reconciler, which hands its listener over",
            target.name, entry.name, role.unit
        );
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}: {} is running, but the units it replaced could not all be retired: {}",
        target.name,
        entry.name,
        failed.join("; ")
    )))
}

/// Retire the units `entry` lists in `retired_units` on `target` before its
/// one unit is started, so a product renamed to `com.wisent.<product>` can
/// bind the listener its old label held. A unit that could not be retired
/// refuses the ensure: starting the replacement beside it is the second
/// process this whole path exists to prevent. Role units are untouched here;
/// they keep their work until the running replacement proves it runs it.
pub(super) async fn retire_before_ensure(
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let Some(entry) = entry else { return Ok(()) };
    let mut failed = Vec::new();
    for retirement in service::retire_units(target, entry, runner).await {
        if retirement.state == "absent" {
            continue;
        }
        eprintln!(
            "{}: {} retired {} before starting: {} ({})",
            target.name, entry.name, retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}: {} was not started, because the units it replaced could not all be retired and \
         would run beside it: {}",
        target.name,
        entry.name,
        failed.join("; ")
    )))
}
