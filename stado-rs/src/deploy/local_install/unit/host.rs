//! Combine existing Stado component declarations into one native host unit.
//! Conflicting options or credentials are refused before any unit is changed.

mod definition;
mod failure_fixer;
mod inputs;
mod install;
mod merge;
mod serve;

use merge::{merge_control_plane, merge_environment, merge_watchdog};

pub(crate) use install::install;

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use clap::Parser;

use crate::cli::entry::spec::fleet::host::{state::HostStateCommands, HostCommands};
use crate::cli::entry::spec::root::installation::InstallationCommands;
use crate::cli::entry::spec::root::{
    planes::PlaneCommands, platform::PlatformCommands, work::WorkCommands,
};
use crate::cli::entry::spec::{Cli, Commands};
use crate::cli::integrations::runtime::ServeArgs;
use crate::cli::release_cmd::ReleaseCommands;
use crate::cli::resolver::ResolverCommands;
use crate::deploy::DeployError;
use crate::targets::Registry;

use super::InstallPlan;

pub(crate) struct Component {
    pub plan: InstallPlan,
    definition: crate::deploy::service::UnitFile,
    /// Native scheduler cadence, not an installer process default.
    pub periodic: Option<NonZeroU64>,
}

pub(crate) fn canonical_label() -> Result<String, DeployError> {
    crate::deploy::local_install::stado_unit()
}

fn command(plan: &InstallPlan) -> Result<Commands, DeployError> {
    Cli::try_parse_from(&plan.exec_args)
        .map_err(|error| {
            DeployError(format!(
                "{}: invalid component command: {error}",
                plan.label
            ))
        })?
        .command
        .ok_or_else(|| {
            DeployError(format!(
                "{} has no component command: its unit runs {:?}",
                plan.label, plan.exec_args
            ))
        })
}

/// The environment a Stado process reads its Skarbiec identity from.
pub(super) fn skarbiec_identity(name: &str) -> bool {
    name.starts_with("WC_SKARBIEC_") || name.starts_with("WC_AGENT_SKARBIEC_")
}

/// Whether a captured unit runs one of the resident roles the host process
/// carries. A Stado unit that runs anything else — a periodic
/// `stado product sync`, say — is not a part of the host and is left alone.
/// An argv this build cannot parse counts as resident, so the merge names it.
pub(super) fn resident_role(plan: &InstallPlan) -> bool {
    if plan.kind == "watchdog" || plan.kind == "failure-fixer" {
        return true;
    }
    let Ok(parsed) = command(plan) else {
        return true;
    };
    matches!(
        parsed,
        Commands::Installation(InstallationCommands::DiskCleanup { .. })
            | Commands::Platform(PlatformCommands::Host(HostCommands::State(
                HostStateCommands::CollectBeacon { .. }
            )))
            | Commands::Work(WorkCommands::Agent(_))
            | Commands::Platform(PlatformCommands::Resolver(ResolverCommands::Serve { .. }))
            | Commands::Platform(PlatformCommands::Release(ReleaseCommands::Agent(_)))
            | Commands::Planes(
                PlaneCommands::Serve(_)
                    | PlaneCommands::Coordinator { .. }
                    | PlaneCommands::LocalControlPlane { .. }
                    | PlaneCommands::CloudControlPlane { .. }
                    | PlaneCommands::Dashboard { .. }
            )
    )
}

fn check_target(expected: &str, actual: &str, label: &str) -> Result<(), DeployError> {
    if expected != actual {
        return Err(DeployError(format!(
            "{label} targets {actual}, not host {expected}"
        )));
    }
    Ok(())
}

/// Merge only component plans whose native definitions have already been read.
/// The caller retains those definitions for transactional retirement and rollback.
pub(crate) fn merge(
    mut host: InstallPlan,
    components: &[Component],
    registry: &Registry,
) -> Result<InstallPlan, DeployError> {
    let runtime: ServeArgs = match command(&host)? {
        Commands::Planes(PlaneCommands::Serve(runtime)) => *runtime,
        _ => {
            return Err(DeployError(
                "host installation must execute stado serve".to_string(),
            ))
        }
    };
    let inputs::Inputs {
        mut runtime,
        components,
    } = inputs::prepare(runtime, &host, components)?;
    let mut worker_seen = runtime.run_worker;
    let mut environment = BTreeMap::new();
    for (source, parsed_command) in components {
        let component = &source.plan;
        if component.kind == "watchdog" {
            merge_watchdog(&mut runtime, component)?;
            merge_environment(&mut environment, component, false)?;
            continue;
        }
        if component.kind == "failure-fixer" {
            failure_fixer::merge(&mut runtime, component)?;
            merge_environment(&mut environment, component, false)?;
            continue;
        }
        let Some(parsed_command) = parsed_command else {
            merge_environment(&mut environment, component, false)?;
            continue;
        };
        let worker = match parsed_command {
            Commands::Installation(InstallationCommands::DiskCleanup {
                once,
                watch,
                to_target,
                dry_run,
            }) => {
                if once || !watch || to_target || dry_run {
                    return Err(DeployError(format!(
                        "{} is a finite or preview cleanup, not the resident policy watch",
                        component.label
                    )));
                }
                runtime.disk_cleanup = true;
                false
            }
            Commands::Platform(PlatformCommands::Host(HostCommands::State(
                HostStateCommands::CollectBeacon { publish },
            ))) => {
                merge::merge_health(&mut runtime, component, publish, source.periodic)?;
                false
            }
            Commands::Work(WorkCommands::Agent(mut worker)) => {
                if let Some(target) = &worker.target {
                    check_target(&host.name, target, &component.label)?;
                }
                if worker.idle_shutdown
                    || !crate::capabilities::ProviderId::Local.matches(&worker.kind)
                {
                    return Err(DeployError(format!(
                        "{} is an ephemeral worker, not a resident host component",
                        component.label
                    )));
                }
                worker.target = Some(host.name.clone());
                // Keep --auto: it applies the target's current environment
                // overrides at each startup, not just its inferred GPU type.
                if worker_seen && runtime.worker != worker {
                    return Err(DeployError(
                        "existing worker units disagree on worker options".to_string(),
                    ));
                }
                runtime.worker = worker;
                runtime.run_worker = true;
                worker_seen = true;
                true
            }
            Commands::Platform(PlatformCommands::Resolver(ResolverCommands::Serve { target })) => {
                check_target(&host.name, &target, &component.label)?;
                runtime.resolver = true;
                false
            }
            Commands::Platform(PlatformCommands::Release(ReleaseCommands::Agent(release))) => {
                check_target(&host.name, &release.target, &component.label)?;
                let resident = !release.once && release.product.is_none();
                let interval_seconds = release.interval_seconds.ok_or_else(|| {
                    DeployError(format!(
                        "{} declares a resident release agent without --interval-seconds",
                        component.label
                    ))
                })?;
                merge::merge_release_agent(&mut runtime, component, resident, interval_seconds)?;
                false
            }
            Commands::Planes(PlaneCommands::Coordinator { target, once }) => {
                merge::merge_coordinator(&mut runtime, component, once, registry, target)?;
                false
            }
            Commands::Planes(PlaneCommands::LocalControlPlane {
                bind,
                port,
                interval,
            }) => {
                merge_control_plane(
                    &mut runtime,
                    component,
                    crate::remote::control_plane::CoordinatorMode::Local,
                    bind,
                    port,
                    interval,
                )?;
                worker_seen = runtime.run_worker;
                false
            }
            Commands::Planes(PlaneCommands::CloudControlPlane {
                bind,
                port,
                interval,
            }) => {
                merge_control_plane(
                    &mut runtime,
                    component,
                    crate::remote::control_plane::CoordinatorMode::Cloud,
                    bind,
                    port,
                    interval,
                )?;
                false
            }
            Commands::Planes(PlaneCommands::Dashboard {
                inherited_listener: true,
                ..
            }) => {
                return Err(DeployError(format!(
                    "{} declares --inherited-listener; a resident listener binds its own port",
                    component.label
                )))
            }
            Commands::Planes(PlaneCommands::Dashboard {
                bind,
                port,
                enrollment_only,
                inherited_listener: false,
            }) => {
                merge::merge_dashboard(&mut runtime, component, bind, port, enrollment_only)?;
                false
            }
            Commands::Planes(PlaneCommands::Serve(theirs)) => {
                let worker = serve::merge(&mut runtime, *theirs, component, &host.name)?;
                worker_seen = runtime.run_worker;
                worker
            }
            _ => {
                return Err(DeployError(format!(
                    "{} does not declare a supported resident Stado component",
                    component.label
                )))
            }
        };
        merge_environment(&mut environment, component, worker)?;
    }
    host.env = merge::host_environment(std::mem::take(&mut host.env), environment);
    let binary = host
        .exec_args
        .first()
        .cloned()
        .ok_or_else(|| DeployError("host unit has no executable".to_string()))?;
    host.exec_args = std::iter::once(binary).chain(runtime.arguments()).collect();
    // Parse the emitted invocation with the real CLI declaration before installation.
    command(&host)?;
    Ok(host)
}
