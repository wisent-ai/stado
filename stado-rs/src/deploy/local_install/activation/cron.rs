//! The last rung of the Darwin ladder: no launchd domain answered, so the
//! same rendered environment is started by hand and again at every boot.

use std::path::Path;

use crate::deploy::local_install::unit::InstallPlan;
use crate::deploy::{shlex_quote, write_if_changed, CommandSpec, DeployError, Runner};

use super::owner_log::prepare_owner_log;

/// Persistent headless-mac last resort when launchctl refuses bootstrap from
/// an SSH audit session. Cron starts the same generated environment on boot,
/// and nohup starts it immediately.
///
/// `pub(super)` for [`super::execute_plan`], the one caller: it is reached
/// only after every launchd domain has refused.
pub(super) async fn install_cron_job(
    plan: &InstallPlan,
    home: &Path,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let wrapper = home
        .join(".stado")
        .join("bin")
        .join(format!("run-{}.sh", plan.label));
    let log = prepare_owner_log(home, &plan.label)?;
    let mut content = String::from("#!/bin/sh\n");
    for (key, value) in &plan.env {
        if !value.is_empty() {
            content.push_str(&format!("export {key}={}\n", shlex_quote(value)));
        }
    }
    content.push_str("exec");
    for arg in &plan.exec_args {
        content.push(' ');
        content.push_str(&shlex_quote(arg));
    }
    content.push('\n');
    write_if_changed(&wrapper, &content).map_err(|exc| DeployError(exc.to_string()))?;

    let wrapper_arg = shlex_quote(&wrapper.to_string_lossy());
    let cron_line = format!(
        "@reboot /bin/sh {wrapper_arg} >> {} 2>&1",
        shlex_quote(&log.to_string_lossy())
    );
    let cron_script = format!(
        "{{ crontab -l 2>/dev/null | grep -Fv -- {wrapper_arg} || true; printf '%s\\n' {}; }} | crontab -",
        shlex_quote(&cron_line)
    );
    let cron = runner(CommandSpec::new(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        cron_script,
    ]))
    .await
    .map_err(DeployError)?;
    if !cron.ok() {
        return Err(DeployError(format!(
            "crontab install failed: {}",
            cron.detail()
        )));
    }
    let start = runner(CommandSpec::new(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "nohup /bin/sh {wrapper_arg} >> {} 2>&1 </dev/null &",
            shlex_quote(&log.to_string_lossy())
        ),
    ]))
    .await
    .map_err(DeployError)?;
    if !start.ok() {
        return Err(DeployError(format!(
            "agent start failed: {}",
            start.detail()
        )));
    }
    echo(&format!(
        "[ok]   installed headless cron job {} (logs: {})",
        plan.label,
        log.display()
    ));
    Ok(())
}
