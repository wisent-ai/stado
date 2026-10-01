//! Parse native declarations once and retain an existing host's role settings.

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::entry::spec::Commands;
use crate::cli::integrations::runtime::ServeArgs;
use crate::deploy::DeployError;

use super::{check_target, command, Component, InstallPlan};

pub(super) struct Inputs<'a> {
    pub runtime: ServeArgs,
    pub components: Vec<(&'a Component, Option<Commands>)>,
}

pub(super) fn prepare<'a>(
    mut runtime: ServeArgs,
    host: &InstallPlan,
    components: &'a [Component],
) -> Result<Inputs<'a>, DeployError> {
    let mut root_label: Option<&str> = None;
    let mut parsed = Vec::with_capacity(components.len());
    for source in components {
        let component = &source.plan;
        if component.os != host.os || component.daemon != host.daemon {
            return Err(DeployError(format!(
                "{} has a different native execution domain from {}",
                component.label, host.label
            )));
        }
        let command = if matches!(component.kind.as_str(), "watchdog" | "failure-fixer") {
            None
        } else {
            match command(component)? {
                Commands::Planes(PlaneCommands::Serve(existing)) => {
                    if let Some(previous) = root_label {
                        return Err(DeployError(format!(
                            "host consolidation found multiple resident owners: {previous} and {}",
                            component.label
                        )));
                    }
                    if let Some(target) = existing.worker.target.as_deref() {
                        check_target(&host.name, target, &component.label)?;
                    }
                    root_label = Some(&component.label);
                    runtime = *existing;
                    None
                }
                command => Some(command),
            }
        };
        parsed.push((source, command));
    }
    Ok(Inputs {
        runtime,
        components: parsed,
    })
}

/// The coordinator a replaced coordinator unit named, or the one active one.
pub(super) fn coordinator_name(
    registry: &crate::targets::Registry,
    selector: Option<&str>,
) -> Result<String, DeployError> {
    if let Some(selector) = selector {
        return registry
            .lookup_coordinator_selector(selector)
            .map(|entry| entry.name.clone())
            .ok_or_else(|| DeployError(format!("coordinator {selector} is not declared")));
    }
    let mut active = registry.coordinators.iter().filter(|entry| entry.active);
    let entry = active
        .next()
        .ok_or_else(|| DeployError("no active coordinator is declared".to_string()))?;
    if active.next().is_some() {
        return Err(DeployError(
            "multiple active coordinators require an explicit selection".to_string(),
        ));
    }
    Ok(entry.name.clone())
}
