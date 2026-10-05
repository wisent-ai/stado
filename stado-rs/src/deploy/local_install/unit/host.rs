//! Combine existing Stado component declarations into one native host unit.
//! Conflicting options or credentials are refused before any unit is changed.
//!
//! Every resident Stado role is a role of `stado serve`, so the only unit a
//! host installation folds in is one that already runs `stado serve` (under
//! the canonical label or another). A unit whose command this build does not
//! have — the removed standalone `agent`, `coordinator`, `dashboard`,
//! `resolver serve` or `release agent` loop — is refused by name, so the
//! operator removes it rather than the installer guessing its role.

mod definition;
mod inputs;
mod install;
mod merge;

use merge::merge_environment;

pub(crate) use install::install;

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use clap::Parser;

use crate::cli::entry::spec::root::planes::PlaneCommands;
use crate::cli::entry::spec::{Cli, Commands};
use crate::deploy::DeployError;

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

/// Whether a captured unit is the host process: one that runs `stado serve`.
/// A Stado unit that runs anything else — a periodic `stado product sync`,
/// say — is not a part of the host and is left alone. An argv this build
/// cannot parse counts as resident, so the merge names it.
pub(super) fn resident_role(plan: &InstallPlan) -> bool {
    match command(plan) {
        Ok(parsed) => matches!(parsed, Commands::Planes(PlaneCommands::Serve(_))),
        Err(_) => true,
    }
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
) -> Result<InstallPlan, DeployError> {
    let runtime = match command(&host)? {
        Commands::Planes(PlaneCommands::Serve(runtime)) => *runtime,
        _ => {
            return Err(DeployError(
                "host installation must execute stado serve".to_string(),
            ))
        }
    };
    let inputs::Inputs {
        runtime,
        components,
    } = inputs::prepare(runtime, &host, components)?;
    let mut environment = BTreeMap::new();
    for (source, parsed_command) in components {
        let component = &source.plan;
        if parsed_command.is_some() {
            return Err(DeployError(format!(
                "{} does not run stado serve; every resident Stado role is a role of the host's \
                 one serve process, so remove this unit",
                component.label
            )));
        }
        merge_environment(&mut environment, component)?;
    }
    if runtime.disk_cleanup && runtime.health_interval_seconds.is_none() {
        return Err(DeployError(
            "the host's disk-cleanup watch reads the volume at its --health-interval-seconds, \
             and no component of this host declares that cadence"
                .to_string(),
        ));
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
