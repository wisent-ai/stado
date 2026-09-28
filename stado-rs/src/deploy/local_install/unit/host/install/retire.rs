//! Retiring one Stado unit the host unit replaced: stop it, then move its
//! file to a `.retired-<date>` name the init system ignores, which leaves the
//! exact previous definition on disk for rollback.

use std::path::{Path, PathBuf};

use crate::deploy::local_install::activation::commands::current_uid;
use crate::deploy::local_install::{systemd_unit, LocalOs};
use crate::deploy::{CommandSpec, DeployError, Runner};

use super::{Component, InstallPlan, SYSTEM_DAEMON_DIRECTORY};

fn retired_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".retired-{}", chrono::Utc::now().format("%Y%m%d")));
    PathBuf::from(name)
}

fn sudo(arguments: &[&str]) -> CommandSpec {
    let mut argv = vec!["/usr/bin/sudo".to_string(), "-n".to_string()];
    argv.extend(arguments.iter().map(|argument| argument.to_string()));
    CommandSpec::new(argv)
}

/// Stop one replaced unit and move its file out of the init system's view.
pub(super) async fn retire(
    component: &Component,
    host: &InstallPlan,
    runner: &Runner,
) -> Result<(), DeployError> {
    let path = PathBuf::from(&component.native_definition().path);
    let target = retired_path(&path);
    let label = &component.plan.label;
    let (stop, rename) = match (host.os, path.starts_with(SYSTEM_DAEMON_DIRECTORY)) {
        (LocalOs::Darwin, true) => (
            sudo(&["/bin/launchctl", "bootout", &format!("system/{label}")]),
            Some(sudo(&[
                "/bin/mv",
                &path.to_string_lossy(),
                &target.to_string_lossy(),
            ])),
        ),
        (LocalOs::Darwin, false) => (
            CommandSpec::new(vec![
                "launchctl".to_string(),
                "bootout".to_string(),
                format!("gui/{}/{label}", current_uid()),
            ]),
            None,
        ),
        (LocalOs::Linux, _) => (
            CommandSpec::new(vec![
                "systemctl".to_string(),
                "--user".to_string(),
                "disable".to_string(),
                "--now".to_string(),
                systemd_unit(label),
            ]),
            None,
        ),
    };
    // An unloaded job answers bootout with an error; the file move below is
    // what keeps it from returning, so only that step decides success.
    let _ = runner(stop).await.map_err(DeployError)?;
    match rename {
        Some(rename) => {
            let output = runner(rename).await.map_err(DeployError)?;
            if !output.ok() {
                return Err(DeployError(format!(
                    "retiring {label}: moving {} was refused: {}",
                    path.display(),
                    output.detail()
                )));
            }
        }
        None => std::fs::rename(&path, &target).map_err(|error| {
            DeployError(format!(
                "retiring {label}: moving {}: {error}",
                path.display()
            ))
        })?,
    }
    Ok(())
}
