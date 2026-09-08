use crate::deploy::service::*;

/// Stop an explicitly declared per-login recovery label before taking over its
/// listener. Recovery labels are not derived from the primary unit: older
/// deployments used service names while the managed unit used launchd labels.
pub async fn stop_recovery_unit(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_unit_id(unit)?;
    let uid_output = host_channel::run_program(target, &["/usr/bin/id", "-u"], runner).await?;
    if !uid_output.ok() {
        return Err(DeployError(format!(
            "{}: cannot resolve the GUI user's uid: {}",
            target.name,
            host_channel::last_error_line(&uid_output, "id returned no detail")
        )));
    }
    let uid = uid_output.stdout.trim();
    if uid.is_empty() || !uid.chars().all(|character| character.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{}: id returned an invalid uid: {}",
            target.name, uid
        )));
    }
    for domain in ["gui", "user"] {
        let qualified = format!("{domain}/{uid}/{unit}");
        let output =
            host_channel::run_program(target, &["/bin/launchctl", "bootout", &qualified], runner)
                .await?;
        if !output.ok() {
            let detail = host_channel::last_error_line(&output, "launchctl returned no detail");
            if !detail.contains("Could not find specified service")
                && !detail.contains("No such process")
            {
                return Err(DeployError(format!(
                    "{}: cannot stop recovery label {qualified}: {detail}",
                    target.name
                )));
            }
        }
    }
    Ok(RemoteReport {
        os: "Darwin".to_string(),
        unit: unit.to_string(),
        status: "stopped".to_string(),
        detail: "recovery label removed from gui and user launchd domains".to_string(),
        postcondition: STOPPED_DESCRIBE.to_string(),
        postcondition_state: host_channel::POSTCONDITION_MET.to_string(),
        postcondition_detail: "launchctl bootout completed for both per-login domains".to_string(),
        ..RemoteReport::default()
    })
}

pub async fn reset_service_listener(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    let port = url::Url::parse(probe_url)
        .map_err(|error| DeployError(format!("invalid service probe URL: {error}")))?
        .port()
        .ok_or_else(|| DeployError("service probe URL has no explicit port".to_string()))?;
    let body = LISTENER_RESET_BODY.replace("@PORT@", &shlex_quote(&port.to_string()));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Stop one managed service for a fenced recovery cutover. Unlike
/// [`retire_service`], this leaves the unit enabled and registered.
pub async fn stop_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    stop_service_with_password(target, service, None, runner).await
}

/// Stop a managed service, using the host account credential when the unit is
/// a system LaunchDaemon. The credential travels only on stdin to `sudo -S`.
pub async fn stop_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        let password = sudo_password.ok_or_else(|| {
            DeployError(format!(
                "{} on {} is a system LaunchDaemon and {} has no readable host-account password",
                service.unit_id(),
                service.host,
                target.name
            ))
        })?;
        validate_unit_id(service.unit_id())?;
        let qualified = format!("system/{}", service.unit_id());
        let recovery = format!("system/{}-recovery", service.unit_id());
        for job in [&qualified, &recovery] {
            let output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "bootout",
                    job,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !output.ok() {
                let detail =
                    host_channel::last_error_line(&output, "sudo or launchctl returned no detail");
                if !detail.contains("Could not find specified service")
                    && !detail.contains("No such process")
                {
                    return Err(DeployError(format!(
                        "privileged launchd stop failed on {} for {} with exit {}: {}",
                        target.name, job, output.code, detail
                    )));
                }
            }
        }
        let body = STOP_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP);
        let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
        return run_remote_checked(
            target,
            &prelude,
            &body,
            &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
            runner,
        )
        .await;
    }
    let body = STOP_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP);
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
        runner,
    )
    .await
}

/// `service retire` on one host: bootout / disable, files kept.
///
/// Unlike a command that is merely tested to be working, `retire` must verify
/// a postcondition: the unit is actually unloaded. If bootout/disable fails
/// or is ineffective, the report reflects the host's state, not the script's
/// exit status, and the caller correctly refuses to forget the declaration.
pub async fn retire_service(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        let stopped = stop_service_with_password(target, service, sudo_password, runner).await?;
        if !stopped.succeeded("stopped") {
            return Ok(stopped);
        }
        let password = sudo_password.ok_or_else(|| {
            DeployError(format!(
                "{} on {} is a system LaunchDaemon and {} has no readable host-account password",
                service.unit_id(),
                service.host,
                target.name
            ))
        })?;
        validate_unit_id(service.unit_id())?;
        let qualified = format!("system/{}", service.unit_id());
        let recovery = format!("system/{}-recovery", service.unit_id());
        for job in [&qualified, &recovery] {
            let output = host_channel::run_program_with_stdin(
                target,
                &[
                    "/usr/bin/sudo",
                    "-S",
                    "-p",
                    "",
                    "/bin/launchctl",
                    "disable",
                    job,
                ],
                &format!("{password}\n"),
                runner,
            )
            .await?;
            if !output.ok() {
                let detail =
                    host_channel::last_error_line(&output, "sudo or launchctl returned no detail");
                return Err(DeployError(format!(
                    "privileged launchd disable failed on {} for {} with exit {}: {}",
                    target.name, job, output.code, detail
                )));
            }
        }
    }
    let body = RETIRE_BODY.to_string();
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(STOPPED_DESCRIBE, STOPPED_PROBE),
        runner,
    )
    .await
}

/// `service adopt`'s probe: does this unit actually exist on this host, and
/// what does the host call its file?
pub async fn probe_service(
    target: &ComputeTarget,
    unit: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let script = remote_script(unit, "", "", PROBE_BODY)?;
    run_remote(target, script, runner).await
}
