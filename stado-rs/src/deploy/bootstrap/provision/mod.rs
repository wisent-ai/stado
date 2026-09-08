//! Bootstrap stage three: provision one registry target end to end — pick
//! the SSH channel, install or re-qualify the release binaries, hand a Darwin
//! host to its own per-user installer, then write and enable the Linux units.

mod unit_install;

use crate::deploy::{host_channel, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

use self::unit_install::run_unit_install;
use super::install::{
    install_spec, installed_spec, parse_remote_install, retire_superseded_agent_units_spec,
    ssh_argv, WC_BIN_DEFAULT, WC_PYTHON_DEFAULT,
};
use super::units::unit_installs;

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
        (
            "linux-amd64".to_string(),
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

    if platform == "darwin-arm64" {
        // A remote workstation receives only its dedicated workload-agent
        // consumer. Reusing either the control-plane consumer or its token
        // path is a closed failure before SCP runs.
        let grant_path = crate::config::agent_skarbiec_token_file();
        let agent_consumer = crate::config::agent_skarbiec_consumer();
        let agent_url = crate::config::agent_skarbiec_url();
        let same_path = std::fs::canonicalize(grant_path)
            .ok()
            .zip(std::fs::canonicalize(crate::config::skarbiec_token_file()).ok())
            .is_some_and(|(agent, control)| agent == control);
        if grant_path.is_empty()
            || same_path
            || agent_consumer != "stado-local-agent"
            || agent_consumer == crate::config::skarbiec_consumer()
        {
            return Err(DeployError(
                "remote Darwin bootstrap requires consumer stado-local-agent and a distinct agent token_file"
                    .to_string(),
            ));
        }
        if !agent_url.starts_with("https://") {
            return Err(DeployError(
                "remote Darwin bootstrap requires agent.skarbiec.url on authenticated HTTPS"
                    .to_string(),
            ));
        }
        // This validates the grant from wherever bootstrap runs, so the grant
        // file's placement is the fact available: an owner-only provisioned file
        // on the control plane, the platform's handoff on an agent host.
        let agent_vault = crate::skarbiec::Client::new(
            agent_url,
            agent_consumer,
            grant_path,
            crate::skarbiec::GrantMode::for_grant_file(grant_path),
        )
        .map_err(|error| {
            DeployError(format!(
                "cannot configure dedicated remote agent grant: {error}"
            ))
        })?;
        let mut visible = agent_vault
            .list_items()
            .await
            .map_err(|error| DeployError(format!("cannot authorize remote agent grant: {error}")))?
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        visible.sort();
        let mut expected = crate::config::agent_skarbiec_items().to_vec();
        expected.sort();
        expected.dedup();
        if visible != expected {
            return Err(DeployError(format!(
                "stado-local-agent grant exposes {visible:?}; expected exactly {expected:?}"
            )));
        }
        let remote_grant = "$HOME/.stado/local-agent-skarbiec-token";
        let prepare = runner(CommandSpec::new(ssh_argv(
            &ssh_target,
            "umask u=rwx,go=; mkdir -p \"$HOME/.stado\"",
        )))
        .await
        .map_err(DeployError)?;
        if !prepare.ok() {
            return Err(DeployError(format!(
                "cannot prepare remote agent grant directory: {}",
                prepare.detail()
            )));
        }
        let copy = runner(CommandSpec::new(vec![
            "scp".to_string(),
            "-q".to_string(),
            grant_path.to_string(),
            format!("{ssh_target}:.stado/local-agent-skarbiec-token"),
        ]))
        .await
        .map_err(DeployError)?;
        if !copy.ok() {
            return Err(DeployError(format!(
                "cannot provision dedicated remote agent grant: {}",
                copy.detail()
            )));
        }
        let secure = runner(CommandSpec::new(ssh_argv(
            &ssh_target,
            &format!("chmod u=rw,go= \"{remote_grant}\""),
        )))
        .await
        .map_err(DeployError)?;
        if !secure.ok() {
            return Err(DeployError(format!(
                "cannot secure dedicated remote agent grant: {}",
                secure.detail()
            )));
        }
        let items = crate::config::agent_skarbiec_items().join(",");
        let secret_fields = crate::config::agent_skarbiec_secret_fields().join(",");
        let skarbiec_prefix = format!(
            "WC_AGENT_SKARBIEC_URL={} WC_AGENT_SKARBIEC_CONSUMER={} \
             WC_AGENT_SKARBIEC_TOKEN_FILE=\"{remote_grant}\" \
             WC_AGENT_SKARBIEC_ITEMS={} WC_AGENT_SKARBIEC_SECRET_FIELDS={} \
             WC_SKARBIEC_URL={} WC_SKARBIEC_CONSUMER={} \
             WC_SKARBIEC_TOKEN_FILE=\"{remote_grant}\" ",
            shlex_quote(agent_url),
            shlex_quote(agent_consumer),
            shlex_quote(&items),
            shlex_quote(&secret_fields),
            shlex_quote(agent_url),
            shlex_quote(agent_consumer),
        );
        let command = format!(
            "{skarbiec_prefix}{} bootstrap --local --target {}",
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

    let installs = unit_installs(target, &ssh_target, &stado_bin, &wc_python);

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
