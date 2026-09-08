use super::privileged::privileged_restart_system_daemon;

use crate::deploy::service::*;

/// `service restart` on one host, with the end state it intends checked on
/// the host before the connection closes. A restart whose own steps report
/// success while the unit ends up unloaded is reported as the failure it
/// is: see [`RemoteReport::succeeded`].
///
/// A unit in the system domain takes a different route, because the approved
/// channel is unprivileged and `launchctl bootstrap system` is not available
/// to it. It is not, however, unrecoverable: every daemon this fleet installs
/// carries `UserName`, so the process runs as the approved user even though
/// the job is root's, and it carries `KeepAlive` `<true/>`, so launchd puts a
/// new process in place of one that ends. Ending the process from the account
/// that owns it is therefore the same sequence `launchctl kickstart -k`
/// performs — the job is never unloaded, and there is no window in which it
/// does not exist.
///
/// Both gates are read from the host first ([`inspect_system_daemon`]) and
/// neither is assumed. Without them the command refuses and names the one
/// privileged command that works, because ending a process nothing will
/// respawn is how a degraded control plane becomes a dead one. That refusal
/// used to be the only answer here, and it sent the operator to a host repair
/// that does not re-bootstrap a system daemon either:
/// on 2026-08-19 the object API answered 503 to the whole fleet for an
/// afternoon with no product path back.
pub async fn restart_service(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    restart_service_with_password(target, service, None, runner).await
}

pub async fn restart_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if UnitDomain::from_path(&service.path).requires_privileged_bootstrap() {
        return restart_system_daemon(target, service, sudo_password, runner).await;
    }
    restart_non_system_service(target, service, None, false, runner).await
}

pub(crate) async fn restart_non_system_service(
    target: &ComputeTarget,
    service: &ManagedService,
    observed_domain: Option<&str>,
    reload_unit: bool,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let body = format!(
        "stado_reload_unit={}\n{}",
        u8::from(reload_unit),
        RESTART_BODY.replace("@DISOWNED_SWEEP@", DISOWNED_SWEEP)
    );
    let prelude = prelude_with(
        service.unit_id(),
        "",
        &service.path,
        NO_DOMAIN_REFUSE,
        observed_domain,
    )?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(service.unit_id(), "restart");
    Ok(report)
}
/// Reload one system LaunchDaemon definition and wait for its owned process.
///
/// Unlike `kickstart`, this performs `bootout` and `bootstrap`, so launchd
/// reads changed ProgramArguments from the plist before the service is checked.
pub async fn reload_service_with_password(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    if !matches!(UnitDomain::from_path(&service.path), UnitDomain::System) {
        return Err(DeployError(
            "unit reload is supported only for a system LaunchDaemon".to_string(),
        ));
    }
    privileged_restart_system_daemon(
        target,
        service,
        sudo_password.unwrap_or_default(),
        true,
        runner,
    )
    .await
}

/// The system-domain half of [`restart_service`]: probe, then either end the
/// owned process and let launchd recreate it, or refuse with the privileged
/// command named.
async fn restart_system_daemon(
    target: &ComputeTarget,
    service: &ManagedService,
    sudo_password: Option<&str>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    let (probe, daemon) = inspect_system_daemon(target, service, runner).await?;
    let Some(daemon) = daemon else {
        // No marker: the probe never got as far as reading the unit. Its own
        // report already carries why (a missing unit file, a refused key),
        // and that is a better answer than a refusal composed here.
        return Ok(probe);
    };
    let cached = crate::deploy::service_label_print::print_label(
        target,
        service.unit_id(),
        BootoutScope::System,
        runner,
    )
    .await?;
    if cached.loaded() {
        let cached_argv = cached.runs().ok_or_else(|| {
            DeployError(format!(
                "{} has no readable cached launchd argument vector",
                service.unit_id()
            ))
        })?;
        if cached_argv != daemon.argv {
            return privileged_restart_system_daemon(
                target,
                service,
                sudo_password.unwrap_or_default(),
                true,
                runner,
            )
            .await;
        }
    }
    if !daemon.restartable_unprivileged() {
        if let Some(password) = sudo_password {
            return privileged_restart_system_daemon(target, service, password, false, runner)
                .await;
        }
        return privileged_restart_system_daemon(target, service, "", false, runner)
            .await
            .map_err(|error| {
                DeployError(format!(
                    "{}; passwordless privileged restart also failed: {error}",
                    daemon.refusal(service)
                ))
            });
    }
    let body = DAEMON_TERM_BODY
        .replace("@ARGV@", &shlex_quote(&daemon.argv))
        .replace(
            "@PIDS@",
            &shlex_quote(&validate_pid_list(&daemon.owned_pids)?),
        );
    let prelude = remote_prelude(service.unit_id(), "", &service.path)?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RESPAWNED_DESCRIBE, RESPAWNED_PROBE),
        runner,
    )
    .await?;
    if report.succeeded("restarted") {
        // The host's own detail says what happened, in the 160 characters one
        // marker field allows. Why that counts as a restart is a fixed
        // sentence about launchd, not a fact about this host, so it is stated
        // here instead of eating the framing budget on every pass. Without it
        // an operator reading `restarted` beside a `kill` has to take the
        // equivalence on trust.
        report.detail = format!(
            "{} — that is what `launchctl kickstart -k` does to a KeepAlive job, minus the \
             privilege it needs: the process is replaced and the job is never unloaded",
            report.detail
        );
        return Ok(report);
    }
    if let Some(password) = sudo_password {
        return privileged_restart_system_daemon(target, service, password, false, runner).await;
    }
    Ok(report)
}
