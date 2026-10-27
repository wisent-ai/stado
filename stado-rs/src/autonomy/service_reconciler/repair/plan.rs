//! The unit a repair would assert, and the registry write that records it.

use crate::deploy::service::{self, ManagedService, ServiceStatus};

/// Render the unit a repair would assert, through the one resolution chain
/// `service ensure` already uses: the host's own declaration, then the shipped
/// Wisent catalog, then the declaration bundled with this build. A second
/// resolution order here would let a repair install a different program than
/// an operator's `ensure` for the same name. The catalog service's port comes
/// from the service directory the same way `ensure` reads or assigns it.
pub(super) async fn resolved_plan(
    status: &ServiceStatus,
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Result<(service::DeployPlan, String, Vec<String>, String), super::RepairRefused> {
    let declared = &status.service;
    let catalog_entry = crate::deploy::service_catalog::lookup(&declared.name)?;
    let listen_port = match catalog_entry.as_ref() {
        Some(entry) => crate::cli::directory::listen_port_for(entry, target, runner)
            .await
            .map_err(|error| error.to_string())?,
        None => None,
    };
    // The home the host's own shell states: the unit is rendered under it.
    let home = crate::deploy::host_channel::remote_home(target, runner)
        .await
        .map_err(|error| format!("{}'s home cannot be read: {error}", target.name))?;
    // `resolved_unit` refuses (a stated `refused`) when nothing declares what
    // the unit runs: the declaration is incomplete, and no host command ran.
    let (mut unit, unit_env) = crate::cli::service::resolved_unit(
        target,
        &home,
        &declared.name,
        None,
        &[],
        Some(declared),
        catalog_entry.as_ref(),
        listen_port,
    )
    .map_err(|error| {
        let kind = match error.failure {
            Some(crate::primitives::failure::FailureCode::Refused) => {
                super::FailureKind::DeclarationIncomplete
            }
            _ => super::FailureKind::RepairFailed,
        };
        super::RepairRefused::new(kind, error.to_string())
    })?;
    let mut plan = match unit
        .unit
        .as_deref()
        .or_else(|| crate::cli::service::declared_label(declared))
    {
        Some(label) => service::plan_deploy_labelled(
            target,
            &declared.name,
            label,
            &unit.program,
            &unit.args,
            &unit_env,
        ),
        None => service::plan_deploy_labelled(
            target,
            &declared.name,
            &crate::deploy::local_install::label(&declared.name),
            &unit.program,
            &unit.args,
            &unit_env,
        ),
    }
    .map_err(|error| format!("the deploy plan of {} cannot be built: {error}", declared.name))?;
    if !unit.systemd_unit.is_empty() {
        if !target.release_platform.starts_with("linux") {
            return Err(format!(
                "{} declares a systemd unit definition on non-Linux platform {}",
                declared.name, target.release_platform
            )
            .into());
        }
        let definition = std::mem::take(&mut unit.systemd_unit);
        unit.systemd_unit = service::retain_systemd_unit(&mut plan, &definition, &unit_env, false)
            .map_err(|error| format!("the systemd unit of {} cannot be kept in its plan: {error}", declared.name))?;
    }
    if service::UnitDomain::from_path(&declared.path).is_per_login() {
        plan.force_daemon = false;
    }
    Ok((plan, unit.program, unit.args, unit.systemd_unit))
}

pub(super) async fn replace_declaration(
    existing: &ManagedService,
    mut corrected: ManagedService,
    program: String,
    args: Vec<String>,
    systemd_unit: String,
) -> Result<bool, String> {
    corrected.name = existing.name.clone();
    corrected.host_heuristic = existing.host_heuristic.clone();
    corrected.source = existing.source.clone();
    corrected.managed_since = existing.managed_since.clone();
    corrected.onboarding = existing.onboarding.clone();
    corrected.program = program;
    corrected.args = args;
    corrected.env = existing.env.clone();
    corrected.systemd_unit = systemd_unit;
    if corrected == *existing {
        return Ok(false);
    }
    // Pure: the corrected record is already decided, so replacing the entry
    // is a function of whatever document is current. A lost race is answered
    // by replacing it in the newer one, and `commit_document` does that
    // rather than overwriting the writer that got there first — this runs on
    // a loop, so it is the caller most likely to meet one.
    crate::cli::registry::commit_document(|current| {
        let mut document = current.clone();
        service::replace_service(&mut document, &corrected).map_err(crate::cli::CmdError::from)?;
        Ok(document)
    })
    .await
    .map_err(|error| format!("the corrected declaration of {} cannot be recorded in the registry: {error}", existing.name))?;
    Ok(true)
}
