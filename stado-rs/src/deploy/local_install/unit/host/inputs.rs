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
