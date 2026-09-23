//! Combine existing Stado component declarations into one native host unit.
//! Conflicting options or credentials are refused before any unit is changed.

mod failure_fixer;
mod definition;
mod inputs;

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use clap::Parser;

use crate::cli::entry::spec::{Cli, Commands};
use crate::cli::entry::spec::root::installation::InstallationCommands;
use crate::cli::entry::spec::root::{planes::PlaneCommands, platform::PlatformCommands, work::WorkCommands};
use crate::cli::entry::spec::fleet::host::{HostCommands, state::HostStateCommands};
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
    let product = crate::deploy::service_catalog::lookup("stado")
        .map_err(DeployError)?
        .ok_or_else(|| DeployError("the service catalog does not declare Stado".to_string()))?;
    Ok(product.unit.unwrap_or(product.name))
}

fn command(plan: &InstallPlan) -> Result<Commands, DeployError> {
    Cli::try_parse_from(&plan.exec_args)
        .map_err(|error| DeployError(format!("{}: invalid component command: {error}", plan.label)))?
        .command.ok_or_else(|| DeployError(format!("{} has no component command", plan.label)))
}

fn check_target(expected: &str, actual: &str, label: &str) -> Result<(), DeployError> {
    if expected != actual {
        return Err(DeployError(format!("{label} targets {actual}, not host {expected}")));
    }
    Ok(())
}

fn coordinator_name(registry: &Registry, selector: Option<&str>) -> Result<String, DeployError> {
    if let Some(selector) = selector {
        return registry.lookup_coordinator_selector(selector).map(|entry| entry.name.clone())
            .ok_or_else(|| DeployError(format!("coordinator {selector} is not declared")));
    }
    let mut active = registry.coordinators.iter().filter(|entry| entry.active);
    let entry = active.next().ok_or_else(|| DeployError("no active coordinator is declared".to_string()))?;
    if active.next().is_some() {
        return Err(DeployError("multiple active coordinators require an explicit selection".to_string()));
    }
    Ok(entry.name.clone())
}

fn merge_environment(
    values: &mut BTreeMap<String, (String, String)>,
    component: &InstallPlan,
    worker: bool,
) -> Result<(), DeployError> {
    for (name, value) in &component.env {
        // Old worker units put their limited grant in the default namespace.
        // The host retains it in the worker namespace, never as its operator grant.
        let name = if worker {
            crate::config::resident_worker_environment_key(name)
        } else {
            name
        };
        if let Some((previous, owner)) = values.get(name) {
            if previous != value {
                return Err(DeployError(format!(
                    "host consolidation cannot merge variable {name}: units {owner} and {} disagree",
                    component.label
                )));
            }
        } else {
            values.insert(name.to_string(), (value.clone(), component.label.clone()));
        }
    }
    Ok(())
}

fn merge_watchdog(runtime: &mut ServeArgs, component: &InstallPlan) -> Result<(), DeployError> {
    let (_, arguments) = component.exec_args.split_first()
        .ok_or_else(|| DeployError(format!("{} has no watchdog executable", component.label)))?;
    let bucket_env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::Gcs.id(),
        "bucket",
    ).ok_or_else(|| DeployError("GCS bucket binding is missing from the capability catalog".to_string()))?;
    let bucket = component.env.iter().find(|(name, _)| name == bucket_env)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| crate::watchdog::DEFAULT_BUCKET.to_string());
    let diagnostics = crate::watchdog::parse_args_with_bucket(arguments, bucket)
        .map_err(|error| DeployError(format!("{}: invalid resident watchdog arguments: {error:?}", component.label)))?;
    if diagnostics.once {
        return Err(DeployError(format!("{} is a finite watchdog invocation", component.label)));
    }
    let interval = diagnostics.interval_s.max(crate::watchdog::MIN_INTERVAL_S);
    if runtime.watchdog && (
        runtime.watchdog_bucket.as_ref() != Some(&diagnostics.bucket)
        || runtime.watchdog_interval_seconds != interval
    ) {
        return Err(DeployError("existing watchdog units disagree on their destination or interval".to_string()));
    }
    runtime.watchdog = true;
    runtime.watchdog_bucket = Some(diagnostics.bucket);
    runtime.watchdog_interval_seconds = interval;
    Ok(())
}

fn merge_control_plane(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    mode: crate::remote::control_plane::CoordinatorMode,
    bind: String,
    port: i64,
    interval: i64,
) -> Result<(), DeployError> {
    if runtime.coordinator.is_some() || runtime.control_plane.is_some_and(|previous| previous != mode)
        || runtime.control_plane_interval_seconds.is_some_and(|previous| previous != interval)
    {
        return Err(DeployError("existing units declare different coordinator schedules".to_string()));
    }
    let port = u16::try_from(port)
        .map_err(|_| DeployError(format!("{}: API port is outside the supported range", component.label)))?;
    if runtime.api && (runtime.bind.as_ref() != Some(&bind) || runtime.port != Some(port)) {
        return Err(DeployError("existing API units declare different listeners".to_string()));
    }
    runtime.control_plane = Some(mode);
    runtime.control_plane_interval_seconds = Some(interval);
    runtime.api = true;
    runtime.bind = Some(bind);
    runtime.port = Some(port);
    if mode == crate::remote::control_plane::CoordinatorMode::Local {
        runtime.run_worker = true;
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
        Commands::Planes(PlaneCommands::Serve(runtime)) => runtime,
        _ => return Err(DeployError("host installation must execute stado serve".to_string())),
    };
    let inputs::Inputs { mut runtime, components } = inputs::prepare(runtime, &host, components)?;
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
                once, watch, to_target, dry_run,
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
                if !publish {
                    return Err(DeployError(format!("{} collects a finite report, not published host health", component.label)));
                }
                let interval = source.periodic.ok_or_else(|| {
                    DeployError(format!("{} has no native health publication cadence", component.label))
                })?;
                if runtime.health_interval_seconds.is_some_and(|previous| previous != interval) {
                    return Err(DeployError("existing health publishers disagree on their cadence".to_string()));
                }
                runtime.health_interval_seconds = Some(interval);
                false
            }
            Commands::Work(WorkCommands::Agent(mut worker)) => {
                if let Some(target) = &worker.target {
                    check_target(&host.name, target, &component.label)?;
                }
                if worker.idle_shutdown || !crate::capabilities::ProviderId::Local.matches(&worker.kind) {
                    return Err(DeployError(format!("{} is an ephemeral worker, not a resident host component", component.label)));
                }
                worker.target = Some(host.name.clone());
                // Keep --auto: it applies the target's current environment
                // overrides at each startup, not just its inferred GPU type.
                if worker_seen && runtime.worker != worker {
                    return Err(DeployError("existing worker units disagree on worker options".to_string()));
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
                if release.once || release.product.is_some() {
                    return Err(DeployError(format!("{} is not a host-wide resident release agent", component.label)));
                }
                let interval = NonZeroU64::new(release.interval_seconds)
                    .ok_or_else(|| DeployError(format!("{} has a zero release interval", component.label)))?;
                if runtime.release_interval_seconds.is_some_and(|previous| previous != interval) {
                    return Err(DeployError("existing release agents disagree on their interval".to_string()));
                }
                runtime.release_interval_seconds = Some(interval);
                false
            }
            Commands::Planes(PlaneCommands::Coordinator { target, once }) => {
                if runtime.control_plane.is_some() {
                    return Err(DeployError("existing units declare both a registry coordinator and a bundled scheduler".to_string()));
                }
                if once {
                    return Err(DeployError(format!("{} is a finite coordinator invocation", component.label)));
                }
                let name = coordinator_name(registry, target.as_deref())?;
                if runtime.coordinator.as_ref().is_some_and(|previous| previous != &name) {
                    return Err(DeployError("existing coordinator units select different coordinators".to_string()));
                }
                runtime.coordinator = Some(name);
                false
            }
            Commands::Planes(PlaneCommands::LocalControlPlane { bind, port, interval }) => {
                merge_control_plane(&mut runtime, component, crate::remote::control_plane::CoordinatorMode::Local, bind, port, interval)?;
                worker_seen = runtime.run_worker;
                false
            }
            Commands::Planes(PlaneCommands::CloudControlPlane { bind, port, interval }) => {
                merge_control_plane(&mut runtime, component, crate::remote::control_plane::CoordinatorMode::Cloud, bind, port, interval)?;
                false
            }
            Commands::Planes(PlaneCommands::Dashboard { bind, port, enrollment_only }) => {
                if enrollment_only {
                    return Err(DeployError(format!("{} is enrollment-only; it must not become a full API", component.label)));
                }
                // Leave omitted settings omitted: the installed process reads
                // its own configuration, not the installer's cached defaults.
                let port = port.map(u16::try_from).transpose()
                    .map_err(|_| DeployError(format!("{}: API port is outside the supported range", component.label)))?;
                if runtime.api && (runtime.bind != bind || runtime.port != port) {
                    return Err(DeployError("existing API units declare different listeners".to_string()));
                }
                runtime.api = true;
                runtime.bind = bind;
                runtime.port = port;
                false
            }
            _ => return Err(DeployError(format!("{} does not declare a supported resident Stado component", component.label))),
        };
        merge_environment(&mut environment, component, worker)?;
    }
    let mut merged: BTreeMap<String, String> = host.env.into_iter().collect();
    merged.extend(environment.into_iter().map(|(name, (value, _))| (name, value)));
    host.env = merged.into_iter().collect();
    let binary = host.exec_args.first().cloned().ok_or_else(|| DeployError("host unit has no executable".to_string()))?;
    host.exec_args = std::iter::once(binary).chain(runtime.arguments()).collect();
    // Parse the emitted invocation with the real CLI declaration before installation.
    command(&host)?;
    Ok(host)
}
