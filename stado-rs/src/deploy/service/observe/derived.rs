//! The units one catalog product's process replaced on one host, found from
//! what each unit on that host runs.
//!
//! Every unit the host loads or holds a file for, and every unit the registry
//! declares there, is matched against the product by its program
//! ([`service_catalog::runs_program_of`]). A match that is not the product's
//! own unit is work the product's one process does now. For any product but
//! the host Stado process the whole unit is replaced. For the host Stado
//! process the unit does one or more roles, read from the unit's own command
//! line by Stado's own command definitions, or from the live process's role
//! options that name the program, root or destination the unit runs; a unit
//! running Stado's program with no role it can be matched to is reported and
//! left alone. Nothing here names a unit.

use crate::deploy::service::*;
use crate::deploy::service_catalog::{self, CatalogService, RoleUnit};

/// What one host holds that one product's process replaced.
#[derive(Debug, Clone, Default)]
pub struct Predecessors {
    /// Units the product's process replaces whole, wherever that process runs.
    pub replaced: Vec<String>,
    /// Units whose work is a role of the host Stado process: retired where
    /// that process is proven to run the role.
    pub roles: Vec<RoleUnit>,
    /// Units that run the host Stado program and do work no role of the one
    /// process does: never retired, each reported `kept` with the reason.
    pub uncovered: Vec<PredecessorRetirement>,
}

impl Predecessors {
    /// Every unit that is retired once its work is proven to run elsewhere.
    pub fn units(&self) -> impl Iterator<Item = &str> {
        self.replaced
            .iter()
            .map(String::as_str)
            .chain(self.roles.iter().map(|role| role.unit.as_str()))
    }
}

/// One unit as its host and the registry describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUnit {
    pub label: String,
    /// The program and arguments its unit file or declaration names.
    pub declared: String,
    /// The live process's command line, empty when nothing runs.
    pub running: String,
}

/// The units of one host: every row the host reported that has a tie to this
/// fleet, then every registry declaration the host did not report.
pub fn host_units(loaded: &[UndeclaredUnit], declared: &[ManagedService]) -> Vec<HostUnit> {
    let mut units: Vec<HostUnit> = loaded
        .iter()
        .filter(|unit| unit.classification() != "unaffiliated")
        .map(|unit| HostUnit {
            label: unit.label.clone(),
            declared: unit.declared_program(),
            running: unit.running_program.clone(),
        })
        .collect();
    for service in declared {
        if units.iter().any(|unit| unit.label == service.unit_id()) {
            continue;
        }
        units.push(HostUnit {
            label: service.unit_id().to_string(),
            declared: declared_line(service),
            running: String::new(),
        });
    }
    units
}

/// A registry declaration's program and arguments as one command line.
pub fn declared_line(service: &ManagedService) -> String {
    std::iter::once(service.program.as_str())
        .chain(service.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The role option of the live host process's path-carrying role `unit`
/// runs from: the role's program or file, a path under the role's root, or a
/// forward to the role's destination.
fn path_role(unit: &HostUnit, role_paths: &[(String, String)]) -> Option<&'static str> {
    let words: Vec<&str> = unit
        .declared
        .split_whitespace()
        .chain(unit.running.split_whitespace())
        .collect();
    let (flag, _) = role_paths.iter().find(|(_, value)| {
        let value = value.trim_end_matches('/');
        !value.is_empty()
            && words
                .iter()
                .any(|word| *word == value || word.starts_with(&format!("{value}/")))
    })?;
    [
        "--precheck-runner",
        "--edge-caddyfile",
        "--forward-destination",
    ]
    .into_iter()
    .find(|role| *role == flag.as_str())
}

/// The predecessors of `entry` among `units` on the host whose home, release
/// platform and registry name are given; `role_paths` are the live host
/// Stado process's path-carrying roles there (empty for any other product).
pub fn derive_predecessors(
    entry: &CatalogService,
    units: &[HostUnit],
    home: &str,
    platform: &str,
    host: &str,
    role_paths: &[(String, String)],
) -> Result<Predecessors, String> {
    let host_product = service_catalog::host_process()?.name == entry.name;
    let mut found = Predecessors::default();
    for unit in units {
        if service_catalog::is_catalog_unit(&unit.label)? {
            continue;
        }
        let runs = service_catalog::runs_program_of(
            entry,
            &unit.declared,
            &unit.running,
            home,
            platform,
            host,
        );
        if !host_product {
            if runs {
                found.replaced.push(unit.label.clone());
            }
            continue;
        }
        let line = if unit.declared.is_empty() {
            unit.running.as_str()
        } else {
            unit.declared.as_str()
        };
        let mut roles = if runs {
            service_catalog::host_roles(entry, line)
        } else {
            Vec::new()
        };
        roles.extend(path_role(unit, role_paths));
        match service_catalog::role_unit(&unit.label, &roles) {
            Some(role) => found.roles.push(role),
            None if runs => found.uncovered.push(PredecessorRetirement {
                unit: unit.label.clone(),
                state: "kept".to_string(),
                detail: format!(
                    "it runs {line}, the {} program, and no role of {} does that work",
                    entry.name,
                    service_catalog::unit_of(entry)
                ),
            }),
            None => {}
        }
    }
    Ok(found)
}

/// The predecessors of `entry` on `target`, from `loaded` (the host's
/// [`loaded_units`] answer) and the registry's declarations there. `running`
/// is `entry`'s unit on that host when it is the host Stado process, whose
/// live role options are read to match units that run a role's program.
pub async fn predecessors_on_with(
    target: &ComputeTarget,
    entry: &CatalogService,
    running: Option<&ManagedService>,
    loaded: &[UndeclaredUnit],
    runner: &Runner,
) -> Result<Predecessors, DeployError> {
    let host_product = service_catalog::host_process().map_err(DeployError)?.name == entry.name;
    let role_paths = match running {
        Some(running) if host_product => inspect_process(target, running, runner)
            .await
            .map(|process| process.serve_role_paths)
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let units = host_units(loaded, &declared_services(target));
    derive_predecessors(
        entry,
        &units,
        &service_catalog::home_for(target),
        &target.release_platform,
        &target.name,
        &role_paths,
    )
    .map_err(DeployError)
}

/// [`predecessors_on_with`], reading the host's units first.
pub async fn predecessors_on(
    target: &ComputeTarget,
    entry: &CatalogService,
    running: Option<&ManagedService>,
    runner: &Runner,
) -> Result<Predecessors, DeployError> {
    let loaded = loaded_units(target, runner).await?;
    predecessors_on_with(target, entry, running, &loaded, runner).await
}

/// The units on `target` whose work is the host Stado process's API
/// listener, which only that process's own takeover at API start retires.
pub async fn api_predecessors_on(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    let entry = service_catalog::host_process().map_err(DeployError)?;
    Ok(predecessors_on(target, &entry, None, runner)
        .await?
        .roles
        .into_iter()
        .filter(|role| service_catalog::api_role(role))
        .map(|role| role.unit)
        .collect())
}

/// The role of the host Stado process whose work the registry declaration
/// `declared` on `target` does, when `entry` is that process; `None` for any
/// other product, a declaration that runs another program, and one whose
/// work no role covers. `running` is `entry`'s unit on that host, whose live
/// role options match a declaration that runs a role's program or root.
pub async fn declared_role(
    target: &ComputeTarget,
    entry: &CatalogService,
    running: Option<&ManagedService>,
    declared: &ManagedService,
    runner: &Runner,
) -> Option<RoleUnit> {
    if service_catalog::host_process().ok()?.name != entry.name {
        return None;
    }
    let role_paths = match running {
        Some(running) => inspect_process(target, running, runner)
            .await
            .map(|process| process.serve_role_paths)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let unit = HostUnit {
        label: declared.unit_id().to_string(),
        declared: declared_line(declared),
        running: String::new(),
    };
    derive_predecessors(
        entry,
        &[unit],
        &service_catalog::home_for(target),
        &target.release_platform,
        &target.name,
        &role_paths,
    )
    .ok()?
    .roles
    .into_iter()
    .next()
}

/// The catalog product the registry declaration `service` belongs to
/// although it is not that product's unit, judged from the program it
/// declares on `target`. `None` for a catalog unit, a declaration that names
/// no program, and one whose program no product runs.
pub fn declared_owner(
    target: &ComputeTarget,
    service: &ManagedService,
) -> Result<Option<CatalogService>, String> {
    if service.program.is_empty() {
        return Ok(None);
    }
    service_catalog::owner_of(
        service.unit_id(),
        &declared_line(service),
        "",
        &service_catalog::home_for(target),
        &target.release_platform,
        &target.name,
    )
}
