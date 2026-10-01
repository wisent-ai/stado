//! How one existing component's declaration folds into the single host
//! unit: its environment, its watchdog schedule and its control-plane
//! listener. A disagreement between two replaced units is refused, because
//! the host cannot run both settings at once.

use std::collections::BTreeMap;

use crate::cli::integrations::runtime::ServeArgs;
use crate::deploy::DeployError;

use super::super::InstallPlan;

pub(super) fn merge_environment(
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
        if let Some((previous, owner)) = values.get_mut(name) {
            if previous != value {
                // A search path is a list, not a setting: the host process
                // needs every directory any replaced unit searched, in the
                // order they were first named.
                if name == "PATH" {
                    let mut entries: Vec<&str> = previous.split(':').collect();
                    for entry in value.split(':') {
                        if !entries.contains(&entry) {
                            entries.push(entry);
                        }
                    }
                    *previous = entries.join(":");
                    continue;
                }
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

pub(super) fn merge_watchdog(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
) -> Result<(), DeployError> {
    let (_, arguments) = component
        .exec_args
        .split_first()
        .ok_or_else(|| DeployError(format!("{} has no watchdog executable", component.label)))?;
    let bucket_env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::Gcs.id(),
        "bucket",
    )
    .ok_or_else(|| {
        DeployError("GCS bucket binding is missing from the capability catalog".to_string())
    })?;
    let bucket = component
        .env
        .iter()
        .find(|(name, _)| name == bucket_env)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| crate::watchdog::DEFAULT_BUCKET.to_string());
    let diagnostics =
        crate::watchdog::parse_args_with_bucket(arguments, bucket).map_err(|error| {
            DeployError(format!(
                "{}: invalid resident watchdog arguments: {error:?}",
                component.label
            ))
        })?;
    if diagnostics.once {
        return Err(DeployError(format!(
            "{} is a finite watchdog invocation",
            component.label
        )));
    }
    let interval = diagnostics.interval_s;
    if runtime.watchdog
        && (runtime.watchdog_bucket.as_ref() != Some(&diagnostics.bucket)
            || runtime.watchdog_interval_seconds != interval)
    {
        return Err(DeployError(
            "existing watchdog units disagree on their destination or interval".to_string(),
        ));
    }
    runtime.watchdog = true;
    runtime.watchdog_bucket = Some(diagnostics.bucket);
    runtime.watchdog_interval_seconds = interval;
    Ok(())
}

pub(super) fn merge_control_plane(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    mode: crate::remote::control_plane::CoordinatorMode,
    bind: String,
    port: i64,
    interval: i64,
) -> Result<(), DeployError> {
    if runtime.coordinator.is_some()
        || runtime
            .control_plane
            .is_some_and(|previous| previous != mode)
        || runtime
            .control_plane_interval_seconds
            .is_some_and(|previous| previous != interval)
    {
        return Err(DeployError(
            "existing units declare different coordinator schedules".to_string(),
        ));
    }
    let port = u16::try_from(port).map_err(|_| {
        DeployError(format!(
            "{}: API port is outside the supported range",
            component.label
        ))
    })?;
    if runtime.api && (runtime.bind.as_ref() != Some(&bind) || runtime.port != Some(port)) {
        return Err(DeployError(
            "existing API units declare different listeners".to_string(),
        ));
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

pub(super) fn merge_health(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    publish: bool,
    periodic: Option<std::num::NonZeroU64>,
) -> Result<(), DeployError> {
    if !publish {
        return Err(DeployError(format!(
            "{} collects a finite report, not published host health",
            component.label
        )));
    }
    let interval = periodic.ok_or_else(|| {
        DeployError(format!(
            "{} has no native health publication cadence",
            component.label
        ))
    })?;
    if runtime
        .health_interval_seconds
        .is_some_and(|previous| previous != interval)
    {
        return Err(DeployError(
            "existing health publishers disagree on their cadence".to_string(),
        ));
    }
    runtime.health_interval_seconds = Some(interval);
    Ok(())
}

pub(super) fn merge_coordinator(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    once: bool,
    registry: &crate::targets::Registry,
    target: Option<String>,
) -> Result<(), DeployError> {
    if runtime.control_plane.is_some() {
        return Err(DeployError(
            "existing units declare both a registry coordinator and a bundled scheduler"
                .to_string(),
        ));
    }
    if once {
        return Err(DeployError(format!(
            "{} is a finite coordinator invocation",
            component.label
        )));
    }
    let name = super::inputs::coordinator_name(registry, target.as_deref())?;
    if runtime
        .coordinator
        .as_ref()
        .is_some_and(|previous| previous != &name)
    {
        return Err(DeployError(
            "existing coordinator units select different coordinators".to_string(),
        ));
    }
    runtime.coordinator = Some(name);
    Ok(())
}

pub(super) fn merge_dashboard(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    bind: Option<String>,
    port: Option<i64>,
    enrollment_only: bool,
) -> Result<(), DeployError> {
    if enrollment_only {
        return Err(DeployError(format!(
            "{} is enrollment-only; it must not become a full API",
            component.label
        )));
    }
    // Leave omitted settings omitted: the installed process reads its own
    // configuration, not the installer's cached defaults.
    let port = port.map(u16::try_from).transpose().map_err(|_| {
        DeployError(format!(
            "{}: API port is outside the supported range",
            component.label
        ))
    })?;
    if runtime.api && (runtime.bind != bind || runtime.port != port) {
        return Err(DeployError(
            "existing API units declare different listeners".to_string(),
        ));
    }
    runtime.api = true;
    runtime.bind = bind;
    runtime.port = port;
    Ok(())
}

pub(super) fn merge_release_agent(
    runtime: &mut ServeArgs,
    component: &InstallPlan,
    resident: bool,
    interval_seconds: u64,
) -> Result<(), DeployError> {
    if !resident {
        return Err(DeployError(format!(
            "{} is not a host-wide resident release agent",
            component.label
        )));
    }
    let interval = std::num::NonZeroU64::new(interval_seconds)
        .ok_or_else(|| DeployError(format!("{} has a zero release interval", component.label)))?;
    if runtime
        .release_interval_seconds
        .is_some_and(|previous| previous != interval)
    {
        return Err(DeployError(
            "existing release agents disagree on their interval".to_string(),
        ));
    }
    runtime.release_interval_seconds = Some(interval);
    Ok(())
}

/// The host unit's environment: the plan's defaults, overridden by every
/// replaced unit's variables, with the plan's Skarbiec identity kept.
///
/// A replaced unit's environment is what its role needs (its storage
/// backend, its interpreter), so it is kept over the plan's defaults. Its
/// Skarbiec identity is not: that is the current configuration's, or the one
/// process keeps the retired identities (`stado-control-plane`,
/// `stado-local-agent`) the units were installed with.
pub(super) fn host_environment(
    planned: Vec<(String, String)>,
    replaced: BTreeMap<String, (String, String)>,
) -> Vec<(String, String)> {
    let identity: Vec<(String, String)> = planned
        .iter()
        .filter(|(name, _)| super::skarbiec_identity(name))
        .cloned()
        .collect();
    let mut merged: BTreeMap<String, String> = planned.into_iter().collect();
    merged.extend(replaced.into_iter().map(|(name, (value, _))| (name, value)));
    merged.extend(identity);
    merged.into_iter().collect()
}
