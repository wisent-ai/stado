//! Bootstrap stage three: provision one registry target end to end — pick
//! the SSH channel, install or re-qualify the release binaries, hand a Darwin
//! host to its own per-user installer, then write and enable the Linux units.

mod agent_grant;
mod unit_install;

use crate::deploy::{host_channel, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

use self::agent_grant::{provision_agent_grant, AgentGrant};
use self::unit_install::run_unit_install;
use super::install::{
    install_spec, installed_spec, parse_remote_install, retire_superseded_agent_units_spec,
    ssh_argv, WC_BIN_DEFAULT, WC_PYTHON_DEFAULT,
};
use super::units::{remote_home, unit_installs};

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

    let (platform, wc_python, stado_bin) = if dry_run {
        // The registry's declaration of the host, not a fixed platform: a
        // dry run for a Darwin host used to preview systemd units.
        (
            target.release_platform.clone(),
            WC_PYTHON_DEFAULT.to_string(),
            WC_BIN_DEFAULT.to_string(),
        )
    } else {
        let output = if let Some(expected_version) = target.declared_version("stado") {
            echo(&format!(
                "[probe] {}: verify installed stado on {ssh_target}; registry stable is {expected_version}",
                target.name
            ));
            let installed = runner(installed_spec(&ssh_target, expected_version))
                .await
                .map_err(DeployError)?;
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
                    .map_err(DeployError)?
            }
        } else {
            echo(&format!(
                "[install] {}: no stado version is declared; download release binaries",
                target.name
            ));
            runner(install_spec(&ssh_target))
                .await
                .map_err(DeployError)?
        };
        if !output.ok() {
            return Err(DeployError(format!("install failed: {}", output.detail())));
        }
        let (platform, python, bin) = parse_remote_install(&output.stdout);
        let python = if python.is_empty() {
            WC_PYTHON_DEFAULT.to_string()
        } else {
            python
        };
        let bin = if bin.is_empty() {
            WC_BIN_DEFAULT.to_string()
        } else {
            bin
        };
        (platform, python, bin)
    };

    // Both platforms receive the dedicated workload-agent grant; a host
    // without one declines every job that declares a secret.
    let remote_home = remote_home(&ssh_target);
    let grant = if dry_run {
        AgentGrant::declared(&remote_home)
    } else {
        provision_agent_grant(&ssh_target, &remote_home, runner).await?
    };

    if platform == "darwin-arm64" {
        if dry_run {
            echo(&format!(
                "--- {} launchd install (would run): {}stado bootstrap --local ---",
                target.name,
                grant.shell_prefix()
            ));
            return Ok(());
        }
        let command = format!(
            "{}{} bootstrap --local --target {}",
            grant.shell_prefix(),
            shlex_quote(&stado_bin),
            shlex_quote(&target.name)
        );
        echo(&format!(
            "[launchd] {}: installing per-user Rust agent",
            target.name
        ));
        let output = runner(CommandSpec::new(ssh_argv(&ssh_target, &command)))
            .await
            .map_err(DeployError)?;
        if !output.ok() {
            return Err(DeployError(format!(
                "launchd install failed: {}",
                output.detail()
            )));
        }
        echo(&format!("[ok]   {}: launchd agent installed", target.name));
        return Ok(());
    }

    let environment = grant.assignments();
    let installs = unit_installs(target, &ssh_target, &stado_bin, &wc_python, &environment);

    if dry_run {
        let [(agent_name, agent_text, _), (watchdog_name, watchdog_text, _)] = &installs[..] else {
            unreachable!("unit_installs always returns two units");
        };
        echo(&format!("--- {} systemd unit ---", target.name));
        for line in agent_text.lines() {
            echo(&format!("  {line}"));
        }
        let _ = agent_name;
        echo(&format!("--- {} watchdog systemd unit ---", target.name));
        for line in watchdog_text.lines() {
            echo(&format!("  {line}"));
        }
        let _ = watchdog_name;
        echo(&format!(
            "--- ssh command (would run): ssh {} 'install + enable' ---",
            shlex_quote(&ssh_target)
        ));
        return Ok(());
    }
    echo(&format!(
        "[retire] {}: disabling superseded system and per-user queue agents",
        target.name
    ));
    let retired = runner(retire_superseded_agent_units_spec(&ssh_target))
        .await
        .map_err(DeployError)?;
    if !retired.ok() {
        return Err(DeployError(format!(
            "superseded agent retirement failed: {}",
            retired.detail()
        )));
    }

    echo(&format!(
        "[unit] {}: writing /etc/systemd/system/wisent-compute-agent.service",
        target.name
    ));
    run_unit_install(&installs[0].2, runner).await?;
    echo(&format!(
        "[unit] {}: writing /etc/systemd/system/wisent-compute-watchdog.service",
        target.name
    ));
    run_unit_install(&installs[1].2, runner).await?;
    echo(&format!(
        "[ok]   {}: enabled, agent running with live resource admission",
        target.name
    ));
    Ok(())
}
