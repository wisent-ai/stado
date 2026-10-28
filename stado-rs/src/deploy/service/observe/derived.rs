//! The units one catalog product's process replaced on one host, found from
//! what each unit on that host runs.
//!
//! Every host unit and registry declaration is matched by the program it
//! runs. Other products replace matching units whole. The host Stado process
//! replaces proven role units and units invoking a removed root verb; valid
//! one-pass commands with no resident role are reported and kept.

use crate::deploy::service::*;
use crate::deploy::service_catalog::{self, CatalogService, RoleUnit};

/// What one host holds that one product's process replaced.
#[derive(Debug, Clone, Default)]
pub struct Predecessors {
    /// Units the product's process replaces whole, wherever that process runs.
    pub replaced: Vec<String>,
    /// Host Stado units with a removed root verb: withdrawn before ensure.
    pub obsolete: Vec<String>,
    /// Host Stado roles, retired only when the new process runs them.
    pub roles: Vec<RoleUnit>,
    /// Valid one-pass units with no resident role: reported and kept.
    pub uncovered: Vec<PredecessorRetirement>,
}

impl Predecessors {
    /// Units eligible for withdrawal during ensure or a live reconcile pass.
    pub fn units(&self) -> impl Iterator<Item = &str> {
        self.replaced
            .iter()
            .map(String::as_str)
            .chain(self.roles.iter().map(|role| role.unit.as_str()))
            .chain(self.obsolete.iter().map(String::as_str))
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
                .map_err(|cause| format!("{}: {cause}", unit.label))?
        } else {
            Vec::new()
        };
        roles.extend(path_role(unit, role_paths));
        match service_catalog::role_unit(&unit.label, &roles) {
            Some(role) => found.roles.push(role),
            None if runs && !service_catalog::host_command_known(entry, line) => {
                found.obsolete.push(unit.label.clone())
            }
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
    let host_product = service_catalog::host_process()
        .map_err(compiled_catalog)?
        .name
        == entry.name;
    // The live process's role options decide which units are its roles; a
    // process that cannot be inspected is that failure, not a process with
    // no path-carrying roles.
    let role_paths = match running {
        Some(running) if host_product => {
            inspect_process(target, running, runner)
                .await?
                .serve_role_paths
        }
        _ => Vec::new(),
    };
    let units = host_units(loaded, &declared_services(target));
    // Units resolve `$HOME` under the home the host itself states, never one
    // derived from the login user's name.
    let home = crate::deploy::host_channel::remote_home(target, runner).await?;
    derive_predecessors(
        entry,
        &units,
        &home,
        &target.release_platform,
        &target.name,
        &role_paths,
    )
    .map_err(compiled_catalog)
}

/// The compiled product catalog is a declaration this build carries; a
/// lookup in it that fails is that declaration's failure.
fn compiled_catalog(message: String) -> DeployError {
    DeployError(message).stating(crate::primitives::failure::FailureCode::Config)
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
    let entry = service_catalog::host_process().map_err(compiled_catalog)?;
    Ok(predecessors_on(target, &entry, None, runner)
        .await?
        .roles
        .into_iter()
        .filter(service_catalog::api_role)
        .map(|role| role.unit)
        .collect())
}
