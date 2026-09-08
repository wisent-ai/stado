//! Fencing: the writer side of it.
//!
//! Resolution is the strict part. A named service must resolve to exactly one
//! registry-managed unit, that unit must carry an absolute `STADO_CONFIG`, and
//! it must carry no `EnvironmentFile` and no storage-routing variable — because
//! either of those could override the config this command is about to rewrite,
//! which would make the cutover unprovable. Every such case is a refusal, not
//! a warning.

use std::collections::BTreeMap;
use std::path::Path;

use crate::cli::recovery::deploy_error;
use crate::cli::recovery::request::{RecoveryMigrateArgs, ResolvedService, ServiceRef};
use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, service};

pub(in crate::cli::recovery) async fn resolve_services(
    args: &RecoveryMigrateArgs,
) -> Result<Vec<ResolvedService>, CmdError> {
    let mut requested: BTreeMap<ServiceRef, bool> = BTreeMap::new();
    for reference in &args.writers {
        requested.entry(reference.clone()).or_insert(false);
    }
    for reference in &args.activate {
        requested.insert(reference.clone(), true);
    }
    let runner = production_runner();
    let mut resolved = Vec::new();
    for (reference, activate) in requested {
        let target = host_channel::canonical_target(&reference.host)
            .await
            .map_err(deploy_error)?;
        let matches: Vec<service::ManagedService> = service::declared_services(&target)
            .into_iter()
            .filter(|candidate| candidate.matches(&reference.service))
            .collect();
        let managed = match matches.as_slice() {
            [managed] => managed.clone(),
            [] => {
                return Err(CmdError::click(format!(
                    "{} has no registry-managed service named {:?}",
                    reference.host, reference.service
                )))
            }
            _ => {
                return Err(CmdError::click(format!(
                    "{} resolves ambiguously on {}",
                    reference.service, reference.host
                )))
            }
        };
        let unit = service::fetch_unit_file(&target, &managed, &runner)
            .await
            .map_err(deploy_error)?;
        let environment = service::unit_environment(&unit).map_err(deploy_error)?;
        if !environment.environment_files.is_empty() {
            return Err(CmdError::click(format!("{} uses EnvironmentFile entries; Stado cannot prove they do not override storage routing", reference)));
        }
        let routing_overrides = [
            crate::capabilities::PROVIDERS_CONFIG.env,
            crate::capabilities::DISABLED_PROVIDERS_CONFIG.env,
            crate::capabilities::STORAGE_BACKEND_CONFIG.env,
        ]
        .into_iter()
        .chain(crate::capabilities::STORAGE_BACKEND_CONFIG.backup_env)
        .chain(crate::capabilities::config_envs(
            crate::capabilities::RuntimeFacet::Storage,
        ));
        for override_name in routing_overrides {
            if environment
                .env
                .iter()
                .any(|(name, _)| name == override_name)
            {
                return Err(CmdError::click(format!("{} hard-codes {override_name}; remove the routing override so STADO_CONFIG is authoritative", reference)));
            }
        }
        let config_path = environment
            .env
            .iter()
            .find_map(|(name, value)| (name == "STADO_CONFIG").then(|| value.clone()))
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} has no STADO_CONFIG in its unit; refusing an unverifiable cutover",
                    reference
                ))
            })?;
        if !Path::new(&config_path).is_absolute() {
            return Err(CmdError::click(format!(
                "{} has non-absolute STADO_CONFIG={config_path:?}",
                reference
            )));
        }
        resolved.push(ResolvedService {
            reference,
            target,
            service: managed,
            config_path,
            activate,
        });
    }
    Ok(resolved)
}

pub(in crate::cli::recovery) async fn stop_services(
    services: &[ResolvedService],
) -> Result<(), CmdError> {
    let runner = production_runner();
    for resolved in services {
        let report = service::stop_service(&resolved.target, &resolved.service, &runner)
            .await
            .map_err(deploy_error)?;
        if !report.succeeded("stopped") {
            return Err(CmdError::click(format!(
                "could not fence {}: {}",
                resolved.reference,
                report.failure()
            )));
        }
        println!("  stopped {}", resolved.reference);
    }
    Ok(())
}

pub(in crate::cli::recovery) async fn restart_activated(
    services: &[ResolvedService],
) -> Result<(), CmdError> {
    let runner = production_runner();
    for resolved in services.iter().filter(|service| service.activate) {
        let report = service::restart_service(&resolved.target, &resolved.service, &runner)
            .await
            .map_err(deploy_error)?;
        if !report.succeeded("restarted") {
            return Err(CmdError::click(format!(
                "could not activate {}: {}; destination remains PAUSED",
                resolved.reference,
                report.failure()
            )));
        }
        println!("  restarted {}", resolved.reference);
    }
    Ok(())
}
