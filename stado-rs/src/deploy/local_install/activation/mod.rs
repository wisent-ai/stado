//! Stage three: write the rendered file and load the job into the host's init
//! system. [`commands`] holds the argv per init system, [`owner_log`] the log
//! a booted job writes into, [`daemon`] the system-domain spelling, and
//! [`cron`](self::cron) the last rung of the Darwin ladder.

pub mod commands;
pub mod cron;
pub mod daemon;
pub mod owner_log;

use std::path::Path;
use std::time::Duration;

use crate::deploy::local_install::unit::InstallPlan;
use crate::deploy::local_install::LocalOs;
use crate::deploy::{write_if_changed, CommandOutput, CommandSpec, DeployError, Runner};

use self::commands::{darwin_commands, linux_commands};
use self::cron::install_cron_job;
use self::daemon::install_darwin_daemon;
use self::owner_log::prepare_owner_log;

/// Execute an [`InstallPlan`] (Python `_install_darwin` / `_install_linux`):
/// write the file (skipping a byte-identical rewrite), then boot the job.
pub async fn execute_plan(
    plan: &InstallPlan,
    home: &Path,
    uid: u32,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    // A host with no per-login domain gets the daemon spelling and none of the
    // ladder below: every rung of it addresses a domain that does not exist
    // there, and the last one installs a crontab entry instead of a unit.
    if plan.daemon.is_some() {
        prepare_owner_log(home, &plan.label)?;
        return install_darwin_daemon(plan, home, runner, echo).await;
    }
    let path = plan.unit_path(home);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|exc| DeployError(exc.to_string()))?;
    }
    let log = if plan.os == LocalOs::Darwin {
        Some(prepare_owner_log(home, &plan.label)?)
    } else {
        None
    };
    let written =
        write_if_changed(&path, &plan.content(home)).map_err(|exc| DeployError(exc.to_string()))?;
    let verb = if written { "wrote" } else { "unchanged" };
    match plan.os {
        LocalOs::Darwin => {
            echo(&format!("[plist] {verb} {}", path.display()));
            let [bootout, bootstrap, kickstart] = darwin_commands(&plan.label, &path, uid);
            let _ = runner(bootout).await.map_err(DeployError)?;
            // Retry bootstrap: launchd sporadically rejects a fresh domain
            // right after bootout (Python:
            // 5 attempts, 0.5s apart).
            let mut last: Option<CommandOutput> = None;
            for attempt in 0..5 {
                let output = runner(bootstrap.clone()).await.map_err(DeployError)?;
                let ok = output.ok();
                last = Some(output);
                if ok {
                    break;
                }
                if attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            let Some(output) = last else {
                return Err("launchctl did not run".into());
            };
            if output.ok() {
                let _ = runner(kickstart).await.map_err(DeployError)?;
            } else {
                // Headless SSH sessions can see the logged-in user's domain
                // while macOS rejects bootstrap actions against its gui alias.
                let domain = format!("user/{uid}");
                let service = format!("{domain}/{}", plan.label);
                let _ = runner(CommandSpec::new(vec![
                    "launchctl".to_string(),
                    "bootout".to_string(),
                    service.clone(),
                ]))
                .await
                .map_err(DeployError)?;
                let user_bootstrap = runner(CommandSpec::new(vec![
                    "launchctl".to_string(),
                    "bootstrap".to_string(),
                    domain.clone(),
                    path.to_string_lossy().into_owned(),
                ]))
                .await
                .map_err(DeployError)?;
                if user_bootstrap.ok() {
                    let _ = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "kickstart".to_string(),
                        "-k".to_string(),
                        service,
                    ]))
                    .await
                    .map_err(DeployError)?;
                } else {
                    let gui_domain = format!("gui/{uid}");
                    let gui_service = format!("{gui_domain}/{}", plan.label);
                    let asuser = uid.to_string();
                    let contextual = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "asuser".to_string(),
                        asuser.clone(),
                        "launchctl".to_string(),
                        "bootstrap".to_string(),
                        gui_domain,
                        path.to_string_lossy().into_owned(),
                    ]))
                    .await
                    .map_err(DeployError)?;
                    if !contextual.ok() {
                        echo(
                            "[warn] launchctl unavailable in this SSH session; using cron instead",
                        );
                        install_cron_job(plan, home, runner, echo).await?;
                        return Ok(());
                    }
                    let _ = runner(CommandSpec::new(vec![
                        "launchctl".to_string(),
                        "asuser".to_string(),
                        asuser,
                        "launchctl".to_string(),
                        "kickstart".to_string(),
                        "-k".to_string(),
                        gui_service,
                    ]))
                    .await
                    .map_err(DeployError)?;
                }
            }
            let Some(log) = log.as_ref() else {
                return Err(DeployError("launchd log path was not prepared".to_string()));
            };
            echo(&format!(
                "[ok]   loaded launchd job {} (logs: {})",
                plan.label,
                log.display()
            ));
        }
        LocalOs::Linux => {
            echo(&format!("[unit] {verb} {}", path.display()));
            let [daemon_reload, enable] = linux_commands(&plan.label);
            let _ = runner(daemon_reload).await.map_err(DeployError)?;
            let output = runner(enable).await.map_err(DeployError)?;
            if !output.ok() {
                return Err(DeployError(format!(
                    "systemctl enable failed: {}",
                    output.detail()
                )));
            }
            echo(&format!("[ok]   enabled systemd --user job {}", plan.label));
        }
    }
    Ok(())
}
