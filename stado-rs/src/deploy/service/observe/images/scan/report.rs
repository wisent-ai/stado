use crate::deploy::service::*;

/// The stable public projection of [`observe_unit_image_scan`].
///
/// Both callers receive rows produced by the same native-owner/image pass;
/// only the internal release revisit keeps the observed argv it additionally
/// needs.
pub async fn observe_unit_images(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<UnitImageObservation> {
    observe_unit_image_scan(target, local_units, now_epoch)
        .await
        .into_iter()
        .map(|scan| scan.observation)
        .collect()
}

/// Managed units on one host whose live process is executing an image that is
/// not the file their `ProgramArguments` name.
///
/// The `registry doctor` view of [`observe_unit_images`]: the same pass, with
/// the units that are fine dropped.
pub async fn units_running_replaced_images(
    target: &ComputeTarget,
    local_units: Option<&str>,
    now_epoch: i64,
) -> Vec<StaleUnitImage> {
    observe_unit_images(target, local_units, now_epoch)
        .await
        .iter()
        .filter_map(UnitImageObservation::finding)
        .collect()
}

/// Restart one local launchd unit in its observed owner domain.
///
/// Reuse the in-place kick when launchd still holds the program on disk.
/// A changed cached definition must instead be reloaded through the existing
/// system or user service lifecycle, after the plist and executable are read.
/// Multiple owners, a domain inconsistent with the unit path, or an unreadable
/// replacement refuse before mutation. System operations remain non-interactive.
pub async fn restart_local_unit(
    target: &ComputeTarget,
    label: &str,
    unit_path: &str,
    observed_domain: Option<&str>,
) -> Result<String, DeployError> {
    use crate::primitives::failure::FailureCode;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let refused = |message: String| DeployError(message).stating(FailureCode::Refused);
    if !host_channel::target_is_this_host(target) {
        return Err(refused(
            "local unit restart requires this host's registry target".to_string(),
        ));
    }
    validate_unit_id(label)?;
    let unit_domain = UnitDomain::from_path(unit_path);
    if matches!(unit_domain, UnitDomain::Unknown) {
        return Err(DeployError(format!(
            "{unit_path} is in none of launchd's three unit directories, so no domain places it"
        ))
        .stating(FailureCode::Config));
    }
    let mut candidates: Vec<String> = if unit_domain.requires_privileged_bootstrap() {
        vec!["system".to_string()]
    } else {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            DeployError("this process has no HOME".to_string()).stating(FailureCode::Config)
        })?;
        let uid = std::fs::metadata(&home)
            .map_err(|error| {
                DeployError(format!("this account's uid is unreadable: {error}"))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?
            .uid();
        vec![format!("gui/{uid}"), format!("user/{uid}")]
    };
    if let Some(observed) = observed_domain {
        if !candidates.iter().any(|candidate| candidate == observed) {
            return Err(refused(format!(
                "{unit_path} permits {}, but launchd reports owner {observed}; refusing before restart",
                candidates.join(" or ")
            )));
        }
        candidates.retain(|candidate| candidate == observed);
    }
    let runner = crate::deploy::production_runner();
    let units = loaded_units(target, &runner).await?;
    let loaded = units
        .iter()
        .find(|unit| unit.label == label)
        .ok_or_else(|| {
            DeployError(format!("launchd holds no observed unit named {label}"))
                .stating(FailureCode::NotFound)
        })?;
    let [domain] = loaded.loaded_domains.as_slice() else {
        return Err(refused(format!(
            "{label} has {} loaded owners; refusing to choose a lifecycle domain",
            loaded.loaded_domains.len()
        )));
    };
    if !candidates.contains(domain) {
        return Err(refused(format!(
            "{unit_path} permits {}, but launchd reports owner {domain}",
            candidates.join(" or ")
        )));
    }
    let service = ManagedService {
        host: target.name.clone(),
        name: label.to_string(),
        label: label.to_string(),
        path: unit_path.to_string(),
        kind: KIND_LAUNCHD.to_string(),
        ..ManagedService::default()
    };
    let unit = fetch_unit_file(target, &service, &runner).await?;
    let program = parse_unit_program(&unit)?.ok_or_else(|| {
        DeployError(format!("{unit_path} declares no executable program"))
            .stating(FailureCode::Config)
    })?;
    let metadata = std::fs::metadata(&program).map_err(|error| {
        DeployError(format!("cannot read the replacement {program}: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(refused(format!("{program} is not an executable file")));
    }
    let scope = if unit_domain.requires_privileged_bootstrap() {
        BootoutScope::System
    } else {
        BootoutScope::User
    };
    let cached =
        crate::deploy::service_label_print::print_label(target, label, scope, &runner).await?;
    if cached.domain.as_deref() != Some(domain.as_str()) {
        return Err(refused(format!(
            "{label} changed its loaded owner before restart"
        )));
    }
    let cached_program = cached
        .program
        .as_deref()
        .or_else(|| cached.arguments.as_deref()?.split_whitespace().next())
        .ok_or_else(|| {
            DeployError(format!("{domain}/{label} has no readable cached program"))
                .stating(FailureCode::InfraDown)
        })?;
    if cached_program != program
        || cached
            .arguments
            .as_deref()
            .is_some_and(|argv| argv != loaded.program)
    {
        let report = if unit_domain.requires_privileged_bootstrap() {
            reload_service_with_password(target, &service, None, &runner).await
        } else {
            restart_non_system_service(target, &service, Some(domain), true, &runner).await
        }?;
        if !report.succeeded("restarted") {
            return Err(DeployError(report.failure()).stating(FailureCode::InfraDown));
        }
        if report.domain != *domain {
            return Err(DeployError::unreachable(format!(
                "{label} reloaded in {}, not {domain}",
                report.domain
            ))
            .stating(FailureCode::InfraDown));
        }
        return Ok(format!("{domain}/{label}"));
    }
    let qualified = format!("{domain}/{label}");
    let ran = |program: &str, error: std::io::Error| {
        DeployError(format!("{program} did not run: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    };
    let output = if unit_domain.requires_privileged_bootstrap() {
        std::process::Command::new("/usr/bin/sudo")
            .args(["-n", "/bin/launchctl", "kickstart", "-k", &qualified])
            .output()
            .map_err(|error| ran("/usr/bin/sudo", error))?
    } else {
        std::process::Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &qualified])
            .output()
            .map_err(|error| ran("/bin/launchctl", error))?
    };
    if output.status.success() {
        Ok(qualified)
    } else {
        Err(refused(format!(
            "{qualified}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}
