//! Bootstrap stage three: provision one registry target end to end — pick
//! the SSH channel, install or re-qualify the release binary, retire any unit
//! still running the removed standalone queue agent, and hand the host to
//! its own installer, which writes the one `com.wisent.stado` unit running
//! `stado serve`.

mod agent_grant;

use crate::deploy::{host_channel, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

use self::agent_grant::{provision_agent_grant, AgentGrant};
use super::install::{
    install_spec, installed_spec, parse_remote_install, retire_superseded_agent_units_spec,
    ssh_argv, WC_BIN_DEFAULT,
};
use super::units::remote_home;

/// Provision one registry target (Python `_provision`'s shape, Rust
/// binaries). Echoes the `[skip]`/`[install]`/`[unit]`/`[ok]` lines; `Err`
/// carries the failure message (the caller in [`run`](super::dispatch::run) prints it as
/// `[err]  {name}: {exc}`).
pub async fn provision_target(
    target: &ComputeTarget,
    dry_run: bool,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    if !target.has_ssh_connection() {
        echo(&format!(
            "[skip] {}: no SSH connection path is configured",
            target.name
        ));
        return Ok(());
    }
    let ssh_target = if dry_run {
        target
            .ssh_connections()
            .next()
            .map(|(_, destination)| destination.to_string())
            .unwrap_or_default()
    } else {
        host_channel::select_ssh_connection(target, runner)
            .await?
            .destination
            .to_string()
    };

    let (platform, stado_bin) = if dry_run {
        // The registry's declaration of the host, not a fixed platform: a
        // dry run for a Darwin host used to preview systemd units.
        (target.release_platform.clone(), WC_BIN_DEFAULT.to_string())
    } else {
        let output = if let Some(expected_version) = target.declared_version("stado") {
            echo(&format!(
                "[probe] {}: verify installed stado on {ssh_target}; registry stable is {expected_version}",
                target.name
            ));
            let installed = runner(installed_spec(&ssh_target, expected_version))
                .await
                .map_err(DeployError::unreachable)?;
            if installed.ok() {
                echo(&format!(
                    "[reuse] {}: installed stado matches the registry or its release marker",
                    target.name
                ));
                installed
            } else {
                echo(&format!(
                    "[install] {}: installed stado is missing or unmarked; download bootstrap release binaries",
                    target.name
                ));
                runner(install_spec(&ssh_target))
                    .await
                    .map_err(DeployError::unreachable)?
            }
        } else {
            echo(&format!(
                "[install] {}: no stado version is declared; download release binaries",
                target.name
            ));
            runner(install_spec(&ssh_target))
                .await
                .map_err(DeployError::unreachable)?
        };
        if !output.ok() {
            return Err(DeployError(format!("install failed: {}", output.detail()))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        let (platform, bin) = parse_remote_install(&output.stdout);
        // The install's own report names the platform and the binary it
        // placed; a report without them is a damaged answer, never read as
        // the conventional path or as a platform with no retirement step.
        if platform.is_empty() || bin.is_empty() {
            return Err(DeployError::unreachable(format!(
                "{}: the install reported no platform and binary path: {}",
                target.name,
                output.stdout.trim()
            )));
        }
        (platform, bin)
    };

    // Both platforms receive the dedicated workload-agent grant; a host
    // without one declines every job that declares a secret.
    let remote_home = remote_home(&ssh_target);
    let grant = if dry_run {
        AgentGrant::declared(&remote_home)
    } else {
        provision_agent_grant(&ssh_target, &remote_home, runner).await?
    };
    let command = format!(
        "{}{} bootstrap --local --target {}",
        grant.shell_prefix(),
        shlex_quote(&stado_bin),
        shlex_quote(&target.name)
    );
    if dry_run {
        echo(&format!(
            "--- {} ({platform}) host install (would run): {command} ---",
            target.name
        ));
        return Ok(());
    }

    if platform.starts_with("linux-") {
        // Units an earlier bootstrap wrote run `stado agent`, a command this
        // release does not have; left enabled they restart forever.
        echo(&format!(
            "[retire] {}: disabling units that run the removed stado agent",
            target.name
        ));
        let retired = runner(retire_superseded_agent_units_spec(&ssh_target, &stado_bin))
            .await
            .map_err(DeployError::unreachable)?;
        if !retired.ok() {
            return Err(DeployError::unreachable(format!(
                "retiring units that run the removed stado agent failed: {}",
                retired.detail()
            )));
        }
    }

    echo(&format!(
        "[host] {}: installing the host unit through stado bootstrap --local",
        target.name
    ));
    let output = runner(CommandSpec::new(ssh_argv(&ssh_target, &command)))
        .await
        .map_err(DeployError::unreachable)?;
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "host unit install failed: {}",
            output.detail()
        )));
    }
    echo(&format!("[ok]   {}: host unit installed", target.name));
    Ok(())
}
