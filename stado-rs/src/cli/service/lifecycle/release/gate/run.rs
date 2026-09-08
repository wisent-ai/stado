//! `service release`: the pass itself, in the order its steps have to
//! happen.

use super::*;

pub(crate) async fn release(options: ServiceReleaseOptions<'_>) -> Result<(), CmdError> {
    let (target, services) = release_convergence(&options).await?;
    let Some(declared) = services.first() else {
        return Err(CmdError::click(format!(
            "{} lost the managed {} declaration during domain convergence",
            options.host, options.name
        )));
    };
    let automatic_supersede_unit = (options.supersede_same_label_user
        && UnitDomain::from_path(&declared.path) == UnitDomain::System)
        .then(|| declared.unit_id().to_string());
    let requested_supersede_unit = options
        .supersede_unit
        .or(automatic_supersede_unit.as_deref());
    if options.require_release_version && options.readiness_url.is_none() {
        return Err(CmdError::usage(
            "--require-release-version requires --readiness-url",
        ));
    }
    if options.reload_unit && !UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
    {
        return Err(CmdError::usage(
            "--reload-unit is only needed for a system LaunchDaemon",
        ));
    }
    let runner = production_runner();
    let supersede_unit = if let Some(label) = requested_supersede_unit {
        // One launchd label may exist in both the system and user domains.
        // A managed system daemon superseding its same-named legacy
        // LaunchAgent is safe because every operation below is explicitly
        // scoped: the legacy bootout/restore/delete uses the user domain and
        // the managed restart uses the daemon path. Keep rejecting the same
        // name everywhere else, where the two references would identify one
        // unit rather than the migration pair.
        if label == declared.unit_id()
            && !UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            return Err(CmdError::usage(
                "--supersede-unit may match the managed unit only when that unit is a system \
                 LaunchDaemon replacing its same-named legacy user LaunchAgent",
            ));
        }
        let present = if options.supersede_unit.is_some() {
            service::check_user_launchagent(&target, label, &runner)
                .await
                .map_err(click)?;
            true
        } else {
            service::restorable_user_launchagent_exists(&target, label, &runner)
                .await
                .map_err(click)?
        };
        present.then_some(label)
    } else {
        None
    };
    let shown = service::show_service(&target, declared, &runner)
        .await
        .map_err(click)?;
    let program = shown.detail.trim();
    let directory = program
        .split("/services/")
        .nth(usize::from(true))
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: {} runs {program:?}, which is not under a managed services directory",
                options.host, options.name
            ))
        })?;
    let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap() {
        host_sudo_password(&target).await?
    } else {
        None
    };
    let bundle = service_release_bundle(&options, &target, declared).await?;
    let archive_path = stage_service_release_archive(
        options.product,
        options.version,
        &target.release_platform,
        &bundle.archive,
    )?;
    let previous_directory = current_service_version(&target, directory, &runner).await?;

    let superseded_was_running = if let Some(label) = supersede_unit {
        // `--supersede-unit` names a user LaunchAgent by definition, and the
        // unscoped call would have taken out a system job of the same label
        // first, which is the opposite of superseding.
        let (state, detail) =
            service::bootout_label(&target, label, service::BootoutScope::User, &runner)
                .await
                .map_err(click)?;
        match state.as_str() {
            "booted_out" => true,
            "absent" => false,
            _ => {
                return Err(CmdError::click(format!(
                    "could not supersede user LaunchAgent {label}: {detail}"
                )))
            }
        }
    } else {
        false
    };
    if let Err(error) = update(
        options.name,
        options.host,
        None,
        archive_path.to_str(),
        None,
        false,
        false,
    )
    .await
    {
        if superseded_was_running {
            if let Some(label) = supersede_unit {
                service::restore_user_launchagent(&target, label, &runner)
                    .await
                    .map_err(click)?;
            }
        }
        return Err(error);
    }
    let installed_directory = current_service_version(&target, directory, &runner).await?;
    let restart = if options.reload_unit {
        service::reload_service_with_password(&target, declared, sudo_password.as_deref(), &runner)
            .await
    } else {
        service::restart_service_with_password(&target, declared, sudo_password.as_deref(), &runner)
            .await
    }
    .map_err(click);
    let activation = match restart {
        Ok(report) if report.succeeded("restarted") => {
            if let Some(url) = options.readiness_url {
                let expected = options.require_release_version.then_some(options.version);
                wait_for_service_readiness(
                    &target,
                    url,
                    expected,
                    options.readiness_timeout_seconds,
                    &runner,
                )
                .await
            } else {
                Ok(())
            }
        }
        Ok(report) => Err(CmdError::click(format!(
            "restart failed: {}",
            report.failure()
        ))),
        Err(error) => Err(error),
    };
    if let Err(error) = activation {
        let rollback = rollback_service_release(
            &options,
            &previous_directory,
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await;
        let legacy_restore = if superseded_was_running {
            if let Some(label) = supersede_unit {
                service::restore_user_launchagent(&target, label, &runner)
                    .await
                    .map_err(click)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        return match (rollback, legacy_restore) {
            (Ok(()), Ok(())) => {
                crate::release_agent::publish_service_release_status(
                    options.product,
                    options.host,
                    bundle.rollout_generation,
                    crate::release_agent::RolloutPhase::RolledBack,
                    bundle.previous_version.as_deref(),
                    bundle.previous_sha256.as_deref(),
                    Some(options.version),
                    "service readiness failed; previous release and legacy unit restored",
                )
                .await
                .map_err(CmdError::click)?;
                Err(CmdError::click(format!(
                    "{error}; rolled back to {previous_directory} and restored the prior unit"
                )))
            }
            (Err(rollback_error), Ok(())) => Err(CmdError::click(format!(
                "{error}; rollback to {previous_directory} also failed: {rollback_error}"
            ))),
            (Ok(()), Err(legacy_error)) => Err(CmdError::click(format!(
                "{error}; managed release rolled back, but the legacy unit could not be restored: {legacy_error}"
            ))),
            (Err(rollback_error), Err(legacy_error)) => Err(CmdError::click(format!(
                "{error}; managed rollback failed: {rollback_error}; legacy restore failed: {legacy_error}"
            ))),
        };
    }
    if let Some(label) = supersede_unit {
        service::delete_user_launchagent(&target, label, &runner)
            .await
            .map_err(click)?;
    }

    crate::release_agent::publish_service_release_status(
        options.product,
        options.host,
        bundle.rollout_generation,
        crate::release_agent::RolloutPhase::Committed,
        Some(options.version),
        Some(&bundle.artifact.artifact_sha256),
        bundle.previous_version.as_deref(),
        if supersede_unit.is_some() {
            "service readiness passed; superseded user LaunchAgent removed"
        } else if options.readiness_url.is_some() {
            "service restart and readiness passed"
        } else {
            "service restarted; no readiness endpoint was requested"
        },
    )
    .await
    .map_err(CmdError::click)?;
    record_released_service_source(&options, &bundle.artifact).await?;
    let report = json!({
        "host": options.host,
        "service": options.name,
        "product": options.product,
        "previous_version": bundle.previous_version,
        "version": options.version,
        "artifact_sha256": bundle.artifact.artifact_sha256,
        "artifact_directory": installed_directory,
        "status": "released",
        "readiness": if options.readiness_url.is_some() { "passed" } else { "unit-running" },
        "superseded_unit": supersede_unit,
    });
    if options.emit {
        if options.json {
            print_json(&report)?;
        } else {
            let readiness = if options.readiness_url.is_some() {
                "restart and readiness passed"
            } else {
                "unit restarted; readiness was not requested"
            };
            println!(
                "{}: {} released {} {} ({readiness})",
                options.host, options.name, options.product, options.version
            );
        }
    }
    Ok(())
}
