//! The reads a placement decision is made from: the registry document itself,
//! the hosts a profile declares a complete managed copy on, and the concrete
//! unit behind one logical service name.
//!
//! Nothing here reaches a host. These are the questions the registry answers
//! before any remote command runs, together with the refusal a
//! release-controlled member produces — a forbidden lifecycle mutation is
//! answered here rather than discovered halfway through a move.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::deploy::{host_channel, DeployError};
use crate::placement::{PlacementHost, PlacementProfile, PlacementUnit};
use crate::targets::{self, ComputeTarget, Registry};

pub(in crate::cli::placement) fn deploy_error(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

fn release_controlled_refusal(unit: &PlacementUnit) -> CmdError {
    let owner = unit
        .release_controlled()
        .expect("release-controlled refusal requires release-controlled unit");
    CmdError::click(format!(
        "release-controlled placement member {:?} for product {:?} is owned by controller \
         \"release-control\"; placement lifecycle mutation is forbidden",
        unit.name, owner.product
    ))
}

pub(in crate::cli::placement) fn managed_unit(
    unit: &PlacementUnit,
) -> Result<&crate::placement::ManagedPlacementUnit, CmdError> {
    unit.managed()
        .ok_or_else(|| release_controlled_refusal(unit))
}

pub(in crate::cli::placement) fn ensure_profile_lifecycle_mutable(
    profile: &PlacementProfile,
) -> Result<(), CmdError> {
    for logical in &profile.services {
        for host in profile.hosts.values() {
            if let Some(unit) = host.units.get(logical) {
                if unit.release_controlled().is_some() {
                    return Err(release_controlled_refusal(unit));
                }
            }
        }
    }
    Ok(())
}

pub(in crate::cli::placement) fn parse_registry(document: &Value) -> Result<Registry, CmdError> {
    let text = serde_json::to_string(document)?;
    targets::load_registry_from_str(&text).map_err(|error| CmdError::click(error.to_string()))
}

pub(in crate::cli::placement) fn target<'a>(
    registry: &'a Registry,
    name: &str,
) -> Result<&'a ComputeTarget, CmdError> {
    host_channel::resolve_target(registry, name).map_err(deploy_error)
}

pub(in crate::cli::placement) fn declared_profile_hosts(
    registry: &Registry,
    profile: &PlacementProfile,
) -> Result<Vec<String>, CmdError> {
    let mut complete = Vec::new();
    let mut partial = Vec::new();
    for (host, host_profile) in &profile.hosts {
        let target = target(registry, host)?;
        let declared: Vec<ManagedService> = service::declared_services(target)
            .into_iter()
            .filter(|managed| managed.source == SOURCE_REGISTRY)
            .collect();
        let matched = host_profile
            .units
            .values()
            .filter(|spec| {
                spec.managed().is_some_and(|unit| {
                    declared
                        .iter()
                        .any(|managed| managed.matches(&unit.unit) || managed.matches(&spec.name))
                })
            })
            .count();
        if matched == profile.services.len() {
            complete.push(host.clone());
        } else if matched != 0 {
            partial.push(format!("{host} ({matched}/{})", profile.services.len()));
        }
    }
    if !partial.is_empty() {
        return Err(CmdError::click(format!(
            "placement profile {:?} is split or incomplete: {}",
            profile.name,
            partial.join(", ")
        )));
    }
    Ok(complete)
}

pub(in crate::cli::placement) fn profile_host<'a>(
    profile: &'a PlacementProfile,
    host: &str,
) -> Result<&'a PlacementHost, CmdError> {
    profile.hosts.get(host).ok_or_else(|| {
        CmdError::click(format!(
            "placement profile {:?} does not support destination {:?}",
            profile.name, host
        ))
    })
}

pub(in crate::cli::placement) fn unit<'a>(
    host: &'a PlacementHost,
    logical: &str,
) -> Result<&'a PlacementUnit, CmdError> {
    host.units.get(logical).ok_or_else(|| {
        CmdError::click(format!(
            "placement profile has no concrete unit for service {logical:?}"
        ))
    })
}
