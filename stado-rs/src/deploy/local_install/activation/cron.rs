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
    write_if_changed(&wrapper, &content).map_err(DeployError::from)?;

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
    .map_err(DeployError::unreachable)?;
    if !cron.succeeded() {
        return Err(DeployError::unreachable(format!(
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
    .map_err(DeployError::unreachable)?;
    if !start.succeeded() {
        return Err(DeployError::unreachable(format!(
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

/// Take back the cron fallback an earlier install of this unit wrote, now
/// that a launchd domain runs it. The line and its wrapper started a second
/// copy of the unit at every boot beside launchd's own, and the copy that lost
/// the race for the unit's port restarted for ever. A table with no such line,
/// or an account with no crontab, is the state asked for and says nothing.
///
/// The process cron already started is not stopped here: it is not this
/// install's to end, and from the next boot on only launchd's copy starts.
///
/// `pub(super)` for [`super::execute_plan`], which calls it after every
/// launchd branch that loaded the job.
pub(super) async fn retire_cron_fallback(
    plan: &InstallPlan,
    home: &Path,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let wrapper = home
        .join(".stado")
        .join("bin")
        .join(format!("run-{}.sh", plan.label));
    let wrapper_arg = shlex_quote(&wrapper.to_string_lossy());
    // The table is filtered only when it holds the line, so an account with
    // no crontab is not given an empty one.
    let retire_script = format!(
        // stderr: crontab -l refuses an account that has no crontab; it then
        // holds no fallback line and nothing is written. grep -v exits 1 when
        // the fallback was the only line; the table it leaves is then empty,
        // which crontab installs as an empty table.
        "table=$(crontab -l 2>/dev/null) || exit 0; \
         printf '%s\\n' \"$table\" | grep -Fq -- {wrapper_arg} || exit 0; \
         {{ printf '%s\\n' \"$table\" | grep -Fv -- {wrapper_arg} || true; }} | crontab - \
         && echo retired"
    );
    let retired = runner(CommandSpec::new(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        retire_script,
    ]))
    .await
    .map_err(DeployError::unreachable)?;
    if !retired.succeeded() {
        return Err(DeployError::unreachable(format!(
            "launchd now runs {} but its cron fallback line naming {} could not be removed: {}; \
             every boot still starts a second copy until `crontab -l` no longer lists it",
            plan.label,
            wrapper.display(),
            retired.detail()
        )));
    }
    if retired.stdout.trim() == "retired" {
        echo(&format!(
            "[ok]   removed the @reboot cron fallback of {}; launchd alone starts it",
            plan.label
        ));
    }
    match std::fs::remove_file(&wrapper) {
        Ok(()) => echo(&format!("[ok]   removed {}", wrapper.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DeployError(format!(
                "launchd now runs {} but its cron fallback wrapper {} could not be removed: \
                 {error}",
                plan.label,
                wrapper.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind())))
        }
    }
    Ok(())
}
