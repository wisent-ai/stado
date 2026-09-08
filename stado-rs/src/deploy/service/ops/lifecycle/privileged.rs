use crate::deploy::service::*;

/// Restart or reload one system LaunchDaemon through the host account credential.
///
/// The password travels only on the Stado host channel's stdin to `sudo -S`;
/// neither the password nor a shell program containing it is present in argv
/// or command output.
pub(super) async fn privileged_restart_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    password: &str,
    reload_unit: bool,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_unit_id(service.unit_id())?;
    let qualified = format!("system/{}", service.unit_id());
    let recovery = format!("system/{}-recovery", service.unit_id());
    if reload_unit {
        let lint =
            host_channel::run_program(target, &["/usr/bin/plutil", "-lint", &service.path], runner)
                .await?;
        if !lint.ok() {
            return Err(DeployError(format!(
                "refusing to reload invalid LaunchDaemon plist {} on {}: {}",
                service.path,
                target.name,
                host_channel::last_error_line(&lint, "plutil returned no detail")
            )));
        }
    }
    let recovery_stop = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-S",
            "-p",
            "",
            "/bin/launchctl",
            "bootout",
            &recovery,
        ],
        &format!("{password}\n"),
        runner,
    )
    .await?;
    if !recovery_stop.ok() {
        let detail =
            host_channel::last_error_line(&recovery_stop, "sudo or launchctl returned no detail");
        if !detail.contains("Could not find specified service")
            && !detail.contains("No such process")
        {
            return Err(DeployError(format!(
                "privileged recovery stop failed on {} with exit {}: {}",
                target.name, recovery_stop.code, detail
            )));
        }
    }
    let mut output = if reload_unit {
        let bootout = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootout",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if !bootout.ok() {
            let detail =
                host_channel::last_error_line(&bootout, "sudo or launchctl returned no detail");
            if !detail.contains("Could not find specified service")
                && !detail.contains("No such process")
            {
                return Err(DeployError(format!(
                    "privileged launchd bootout failed on {} with exit {}: {}",
                    target.name, bootout.code, detail
                )));
            }
        }
        let mut unloaded = false;
        for _ in 0..15 {
            let print = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "print",
                    &qualified,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !print.ok() {
                unloaded = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if !unloaded {
            return Err(DeployError(format!(
                "privileged launchd bootout on {} returned, but {} remained loaded after 15s",
                target.name, qualified
            )));
        }
        let enable = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "enable",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if !enable.ok() {
            return Err(DeployError(format!(
                "privileged launchd enable failed on {} with exit {}: {}",
                target.name,
                enable.code,
                host_channel::last_error_line(&enable, "sudo or launchctl returned no detail")
            )));
        }
        host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootstrap",
                "system",
                &service.path,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?
    } else {
        host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "kickstart",
                "-k",
                &qualified,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?
    };
    if !output.ok() && !reload_unit {
        let bootstrap = host_channel::run_program_with_stdin(
            target,
            &[
                "/usr/bin/sudo",
                "-S",
                "-p",
                "",
                "/bin/launchctl",
                "bootstrap",
                "system",
                &service.path,
            ],
            &format!("{password}\n"),
            runner,
        )
        .await?;
        if bootstrap.ok() {
            output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "kickstart",
                    "-k",
                    &qualified,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
        }
    }
    if !output.ok() {
        return Err(DeployError(format!(
            "privileged launchd restart failed on {} with exit {}: {}",
            target.name,
            output.code,
            host_channel::last_error_line(&output, "sudo or launchctl returned no detail")
        )));
    }

    for _ in 0..15 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let (_, daemon) = inspect_system_daemon(target, service, runner).await?;
        if let Some(daemon) = daemon.filter(|daemon| !daemon.owned_pids.is_empty()) {
            return Ok(RemoteReport {
                os: "Darwin".to_string(),
                domain: "system".to_string(),
                domain_status: DOMAIN_STATUS_SYSTEM.to_string(),
                domain_reason: "the unit file is a system LaunchDaemon".to_string(),
                unit: service.unit_id().to_string(),
                path: service.path.clone(),
                status: "restarted".to_string(),
                detail: format!(
                    "launchctl {} the system daemon with pid(s) {}",
                    if reload_unit { "reloaded" } else { "restarted" },
                    daemon.owned_pids.join(" ")
                ),
                postcondition: RUNNING_DESCRIBE.to_string(),
                postcondition_state: host_channel::POSTCONDITION_MET.to_string(),
                postcondition_detail: "launchd reports a process for the unit".to_string(),
                ..RemoteReport::default()
            });
        }
    }
    Err(DeployError(format!(
        "{} accepted the privileged {} but no process appeared for {} in 15 seconds",
        target.name,
        if reload_unit { "reload" } else { "kickstart" },
        service.unit_id()
    )))
}
