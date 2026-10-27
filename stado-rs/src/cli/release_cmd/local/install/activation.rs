//! Activate the placed image, verify its resident unit serves, and put the
//! retained predecessor back if the new unit exits before serving.

use std::path::Path;

use crate::cli::service::lifecycle::release::awaited::{until_serving, Served};
use crate::cli::CmdError;
use crate::deploy::{production_runner, service};

use super::VERB;

pub(super) async fn place(
    directory: &Path,
    name: &str,
    staged: &Path,
    staged_version: Option<&Path>,
    already_current: bool,
    resident_host: Option<&str>,
) -> Result<(), CmdError> {
    let destination = directory.join(name);
    let coordinate = directory.join("stado.release-version");
    let previous_coordinate = if staged_version.is_some() {
        match std::fs::read(&coordinate) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(CmdError::unreachable(format!(
                    "cannot retain {} before activating {name}: {error}",
                    coordinate.display()
                )))
            }
        }
    } else {
        None
    };
    let mut log = |message: &str| println!("{VERB}: {message}");
    let in_flight =
        crate::self_update::ReplacementInFlight::begin(directory, &[name.to_string()], &mut log);
    if !already_current {
        std::fs::rename(staged, &destination).map_err(|error| {
            CmdError::unreachable(format!("cannot install {}: {error}", destination.display()))
        })?;
    }
    let activated = async {
        if let Some(stage) = staged_version {
            std::fs::rename(stage, &coordinate).map_err(|error| {
                CmdError::unreachable(format!("cannot activate {}: {error}", coordinate.display()))
            })?;
        }
        crate::self_update::recycle_replaced_units(VERB, directory, &[name.to_string()], &mut log)
            .await
            .map_err(CmdError::unreachable)?;
        if let Some(host) = resident_host {
            serving(host, name).await?;
        }
        Ok::<(), CmdError>(())
    }
    .await;
    if let Err(cause) = activated {
        let predecessor = directory.join(format!("{name}.previous"));
        let restored = restore(
            directory,
            name,
            &predecessor,
            &destination,
            staged_version.is_some(),
            previous_coordinate.as_deref(),
            resident_host,
        )
        .await;
        drop(in_flight);
        return match restored {
            Ok(()) => Err(CmdError::unreachable(format!("{name} activation failed: {cause}; restored the retained predecessor and verified its resident unit"))),
            Err(rollback) => Err(CmdError::unreachable(format!("{name} activation failed: {cause}; predecessor rollback failed: {rollback}"))),
        };
    }
    drop(in_flight);
    Ok(())
}

async fn serving(host: &str, name: &str) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let label = crate::deploy::service_catalog::host_unit().map_err(CmdError::declaration)?;
    let declared = service::declared_services(&target)
        .into_iter()
        .find(|unit| {
            unit.unit_id() == label
                && crate::deploy::service_catalog::executable_name(&unit.program) == Some(name)
        })
        .ok_or_else(|| {
            CmdError::refused(format!(
                "{host}: no managed resident unit {label} invokes {name}"
            ))
        })?;
    match until_serving(&target, &declared, &production_runner()).await? {
        Served::Serving => Ok(()),
        Served::NoDeclaredPort => Err(CmdError::refused(format!(
            "{host}: {label} has no declared serving port; cannot verify replacement"
        ))),
        Served::Failed(why) => {
            let evidence = crate::cli::service::last_failure(&declared).await;
            Err(CmdError::unreachable(format!("{why}; {evidence}")))
        }
    }
}

async fn restore(
    directory: &Path,
    name: &str,
    predecessor: &Path,
    destination: &Path,
    had_coordinate: bool,
    previous_coordinate: Option<&[u8]>,
    resident_host: Option<&str>,
) -> Result<(), CmdError> {
    // Put back whole and durably, so a host that loses power during the
    // rollback starts the predecessor, not a missing or partial binary.
    crate::primitives::document::persist_copy(
        destination,
        predecessor,
        crate::primitives::file_mode::program(),
    )
    .map_err(|error| {
        CmdError::unreachable(format!(
            "cannot restore {} from {}: {error}",
            destination.display(),
            predecessor.display()
        ))
    })?;
    if had_coordinate {
        let destination = directory.join("stado.release-version");
        if let Some(bytes) = previous_coordinate {
            crate::primitives::document::persist_with_mode(
                &destination,
                bytes,
                crate::primitives::file_mode::shared_read(),
            )
            .map_err(|error| {
                CmdError::unreachable(format!("cannot restore {}: {error}", destination.display()))
            })?;
        } else {
            std::fs::remove_file(&destination).map_err(|error| {
                CmdError::unreachable(format!(
                    "cannot remove newly installed {}: {error}",
                    destination.display()
                ))
            })?;
        }
    }
    // The unit was migrated to the incoming release's command line; the
    // predecessor renders it again from its own catalog before it restarts,
    // or it would restart on an option only the incoming release has.
    if resident_host.is_some() {
        let home = crate::config_file::home().map_err(|cause| {
            CmdError::unreachable(format!("the unit of {name} cannot be rendered again: {cause}"))
        })?;
        super::unit_migration::restore(destination, name, &home)?;
    }
    let mut log = |message: &str| println!("{VERB}: {message}");
    crate::self_update::recycle_replaced_units(VERB, directory, &[name.to_string()], &mut log)
        .await
        .map_err(CmdError::unreachable)?;
    if let Some(host) = resident_host {
        serving(host, name).await?;
    }
    Ok(())
}
