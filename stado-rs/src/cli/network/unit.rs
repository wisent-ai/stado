//! The node's own unit: a launchd job (the system domain on an always-on Mac
//! with no graphical session) or a systemd user unit, running
//! `stado network serve` from this binary. It is separate from the agent's
//! unit on purpose: a host whose agent is broken stays reachable over the
//! network that is how the agent gets repaired.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;
use crate::deploy::local_install::activation::{commands::current_uid, execute_plan};
use crate::deploy::local_install::{label, systemd_unit, InstallPlan, LocalOs};
use crate::deploy::{production_runner, CommandSpec};
use crate::targets::ComputeTarget;

/// The unit's name under Stado's label prefix.
const UNIT: &str = "stado-network";

fn home() -> Result<PathBuf, CmdError> {
    crate::config_file::home().map_err(|cause| {
        CmdError::declaration(format!("the network node's unit has no home: {cause}"))
    })
}

/// The account a daemon runs as: the home's own directory name, the
/// convention `local_install` reads it by.
fn account(home: &Path) -> Result<String, CmdError> {
    home.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "cannot read the account name out of home {}",
                home.display()
            ))
        })
}

fn plan(target: &ComputeTarget) -> Result<(InstallPlan, PathBuf), CmdError> {
    let home = home()?;
    let os = LocalOs::detect().map_err(|error| CmdError::refused(error.to_string()))?;
    let program = std::env::current_exe().map_err(|error| {
        CmdError::declaration(format!(
            "this binary cannot name its own path for the unit to run: {error}"
        ))
    })?;
    let mut env = crate::deploy::local_install::unit::env::install_env(&home, "network", "")
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    env.push(("HOME".to_string(), home.to_string_lossy().into_owned()));
    let daemon = if crate::deploy::service::requires_daemon_domain(target) {
        Some(account(&home)?)
    } else {
        None
    };
    let plan = InstallPlan {
        name: target.name.clone(),
        kind: "network".to_string(),
        os,
        label: label(UNIT),
        exec_args: vec![
            program.to_string_lossy().into_owned(),
            "network".to_string(),
            "serve".to_string(),
        ],
        env,
        daemon,
        startup: None,
    };
    Ok((plan, home))
}

/// Write and load the unit, restarting a node already running.
pub(super) async fn install(target: &ComputeTarget) -> Result<String, CmdError> {
    let (plan, home) = plan(target)?;
    execute_plan(
        &plan,
        &home,
        current_uid(),
        &production_runner(),
        &mut |line| eprintln!("{line}"),
    )
    .await
    .map_err(|error| {
        CmdError::unreachable(format!(
            "the network node's unit {} was not loaded: {error}",
            plan.label
        ))
    })?;
    Ok(plan.unit_path(&home).display().to_string())
}

/// Stop the node and remove its unit; a unit that is not there is not an
/// error, since that is the state asked for.
pub(super) async fn remove(target: &ComputeTarget) -> Result<String, CmdError> {
    let (plan, home) = plan(target)?;
    let path = plan.unit_path(&home);
    let path_text = path.to_string_lossy().into_owned();
    let steps: Vec<Vec<String>> = match (plan.os, plan.daemon.is_some()) {
        (LocalOs::Darwin, true) => vec![
            vec![
                "/usr/bin/sudo".into(),
                "-n".into(),
                "/bin/launchctl".into(),
                "bootout".into(),
                format!("system/{}", plan.label),
            ],
            vec![
                "/usr/bin/sudo".into(),
                "-n".into(),
                "/bin/rm".into(),
                "-f".into(),
                path_text.clone(),
            ],
        ],
        (LocalOs::Darwin, false) => vec![
            vec![
                "launchctl".into(),
                "bootout".into(),
                format!("gui/{}/{}", current_uid(), plan.label),
            ],
            vec!["/bin/rm".into(), "-f".into(), path_text.clone()],
        ],
        (LocalOs::Linux, _) => vec![
            vec![
                "systemctl".into(),
                "--user".into(),
                "disable".into(),
                "--now".into(),
                systemd_unit(&plan.label),
            ],
            vec!["/bin/rm".into(), "-f".into(), path_text.clone()],
            vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
        ],
    };
    let runner = production_runner();
    for argv in steps {
        let line = argv.join(" ");
        let output = runner(CommandSpec::new(argv))
            .await
            .map_err(CmdError::unreachable)?;
        // Stopping a job that is not loaded is refused by both init systems;
        // the file check below is what decides.
        if !output.succeeded() {
            eprintln!("[note] {line}: {}", output.detail().trim());
        }
    }
    if path.exists() {
        return Err(CmdError::unreachable(format!(
            "the network node's unit {path_text} is still there after removal"
        )));
    }
    Ok(path_text)
}
