//! What a registry declaration on one host is, judged from the program it
//! declares: the host Stado role whose work it does, or the catalog product
//! it belongs to.

use crate::deploy::service::*;
use crate::deploy::service_catalog::{self, CatalogService, RoleUnit};

use super::derived::{declared_line, derive_predecessors, HostUnit};
use super::process::inspect_process;

/// The role of the host Stado process whose work the registry declaration
/// `declared` on `target` does, when `entry` is that process; `Ok(None)` for
/// any other product, a declaration that runs another program, and one whose
/// work no role covers. `running` is `entry`'s unit on that host, whose live
/// role options match a declaration that runs a role's program or root.
/// `Err` names what could not be read: the catalog's host process, the
/// running unit's process, or the derivation itself; a caller that treated
/// those as "no role" would leave a taken-over unit running beside its
/// replacement.
pub async fn declared_role(
    target: &ComputeTarget,
    entry: &CatalogService,
    running: Option<&ManagedService>,
    declared: &ManagedService,
    runner: &Runner,
) -> Result<Option<RoleUnit>, String> {
    if service_catalog::host_process()?.name != entry.name {
        return Ok(None);
    }
    let role_paths = match running {
        Some(running) => {
            inspect_process(target, running, runner)
                .await
                .map_err(|error| {
                    format!(
                        "{} on {}: its process cannot be inspected: {error}",
                        running.unit_id(),
                        target.name
                    )
                })?
                .serve_role_paths
        }
        None => Vec::new(),
    };
    let unit = HostUnit {
        label: declared.unit_id().to_string(),
        declared: declared_line(declared),
        running: String::new(),
    };
    // The declaration's `$HOME` resolves under the home the host states.
    let home = crate::deploy::host_channel::remote_home(target, runner)
        .await
        .map_err(|error| format!("{}: its home cannot be read: {error}", target.name))?;
    Ok(derive_predecessors(
        entry,
        &[unit],
        &home,
        &target.release_platform,
        &target.name,
        &role_paths,
    )?
    .roles
    .into_iter()
    .next())
}

/// The catalog product the registry declaration `service` belongs to
/// although it is not that product's unit, judged from the program it
/// declares on `target`, whose `$HOME` resolves under `home`, the home the
/// host states ([`crate::deploy::host_channel::remote_home`]). `None` for a
/// catalog unit, a declaration that names no program, and one whose program
/// no product runs.
pub fn declared_owner(
    target: &ComputeTarget,
    service: &ManagedService,
    home: &str,
) -> Result<Option<CatalogService>, String> {
    if service.program.is_empty() {
        return Ok(None);
    }
    service_catalog::owner_of(
        service.unit_id(),
        &declared_line(service),
        "",
        home,
        &target.release_platform,
        &target.name,
    )
}
