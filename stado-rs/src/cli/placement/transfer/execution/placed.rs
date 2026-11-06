//! The registry change that places a profile on another host: each
//! service's managed record moved from the source to the destination, the
//! destination's standby addresses promoted, and the profile's routes
//! retargeted. A move commits it with its transaction released; a take-over
//! from a source that does not answer commits it as it is.

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use super::super::cutover::promote_standby;
use crate::cli::placement::candidates::{deploy_error, managed_unit, profile_host, unit};
use crate::cli::CmdError;
use crate::deploy::service::{self, ManagedService, SOURCE_REGISTRY};
use crate::placement::{PlacementProfile, PlacementUnit};
use crate::targets::ComputeTarget;

fn destination_record(
    destination: &ComputeTarget,
    spec: &PlacementUnit,
    managed_since: &str,
) -> Result<ManagedService, CmdError> {
    let unit = managed_unit(spec)?;
    let mut managed = match unit.kind {
        crate::deploy::service::UnitKind::Launchd => service::launchd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        ),
        crate::deploy::service::UnitKind::Systemd => service::systemd_service(
            &destination.name,
            &unit.unit,
            &unit.path,
            SOURCE_REGISTRY,
            managed_since,
        ),
    };
    managed.name = spec.name.clone();
    managed.host_heuristic = destination.host_heuristic.clone();
    Ok(managed)
}

/// `document` with `profile` placed on `destination` instead of `source`.
pub(in crate::cli::placement::transfer) fn placed_on(
    mut document: Value,
    profile: &PlacementProfile,
    source: &ComputeTarget,
    destination: &ComputeTarget,
) -> Result<Value, CmdError> {
    let source_profile = profile_host(profile, &source.name)?;
    let destination_profile = profile_host(profile, &destination.name)?;
    let managed_since = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    for logical in &profile.services {
        let source_unit = managed_unit(unit(source_profile, logical)?)?;
        service::remove_service(&mut document, &source.name, &source_unit.unit)
            .map_err(deploy_error)?;
        let managed = destination_record(
            destination,
            unit(destination_profile, logical)?,
            &managed_since,
        )?;
        service::add_service(&mut document, &managed).map_err(deploy_error)?;
    }
    promote_standby(&mut document, &profile.name, &destination.name)?;
    crate::service_resolution::retarget_profile(&mut document, &profile.name, &destination.name)
        .map_err(CmdError::declaration)?;
    Ok(document)
}
