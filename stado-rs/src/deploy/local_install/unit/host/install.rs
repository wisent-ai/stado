//! Install the one host unit from the Stado units it replaces, then retire them.
//!
//! Every unit on this machine whose executable is a Stado binary is read and
//! merged into one `stado serve` declaration before anything is changed; a
//! unit the merge cannot represent refuses the whole installation. Only after
//! the host unit is loaded are the replaced units booted out and their files
//! renamed with a `.retired-<date>` suffix, which launchd and systemd ignore
//! and which leaves the exact previous definition on disk for rollback.

use std::path::{Path, PathBuf};

use crate::deploy::local_install::activation::commands::current_uid;
use crate::deploy::local_install::activation::execute_plan;
use crate::deploy::local_install::{systemd_unit, LocalOs};
use crate::deploy::service::{parse_local_unit_file, UnitFile, KIND_LAUNCHD, KIND_SYSTEMD};
use crate::deploy::{CommandSpec, DeployError, Runner};

use super::{merge, Component, InstallPlan};

/// Which planned component a native executable belongs to. Only the program
/// decides; the merge then parses the actual arguments.
fn component_kind(program: &str, arguments: &[String]) -> Option<&'static str> {
    match Path::new(program).file_name()?.to_str()? {
        "stado" => Some("agent"),
        "stado-watchdog" => Some("watchdog"),
        "bash"
            if arguments
                .iter()
                .any(|argument| argument.contains("scan-dispatch")) =>
        {
            Some("failure-fixer")
        }
        _ => None,
    }
}

fn native_kind(os: LocalOs) -> &'static str {
    match os {
        LocalOs::Darwin => KIND_LAUNCHD,
        LocalOs::Linux => KIND_SYSTEMD,
    }
}

/// The label a native file declares, from its file name.
fn label_of(path: &Path, os: LocalOs) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let suffix = match os {
        LocalOs::Darwin => ".plist",
        LocalOs::Linux => crate::deploy::local_install::SYSTEMD_SUFFIX,
    };
    name.strip_suffix(suffix).map(str::to_string)
}

/// Every Stado component unit in the host plan's own execution domain.
fn discover(
    host: &InstallPlan,
    home: &Path,
    component_plan: &dyn Fn(&str, &str) -> Result<InstallPlan, DeployError>,
) -> Result<Vec<Component>, DeployError> {
    let host_path = host.unit_path(home);
    let own = host_path
        .parent()
        .ok_or_else(|| DeployError(format!("{} has no unit directory", host_path.display())))?
        .to_path_buf();
    // A Mac keeps Stado units in both launchd domains when the registry's
    // domain for it changed after they were installed: the mini is declared
    // graphical (it hosts Weles) while its Stado units sit in the system
    // domain. The one host unit replaces them wherever they are.
    let mut directories = vec![own.clone()];
    if host.os == LocalOs::Darwin {
        directories.push(if host.daemon.is_some() {
            home.join("Library/LaunchAgents")
        } else {
            PathBuf::from(SYSTEM_DAEMON_DIRECTORY)
        });
    }
    let mut found = Vec::new();
    for directory in &directories {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(DeployError(format!(
                    "reading {}: {error}",
                    directory.display()
                )))
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| DeployError(format!("reading {}: {error}", directory.display())))?
                .path();
            found.push((path, *directory == own));
        }
    }
    let kind = native_kind(host.os);
    let mut components = Vec::new();
    for (path, in_own_domain) in found {
        let Some(label) = label_of(&path, host.os) else {
            continue;
        };
        if label == host.label && in_own_domain {
            continue;
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| DeployError(format!("reading {}: {error}", path.display())))?;
        // Other vendors keep binary plists beside ours in the system domain;
        // every unit Stado writes is text, so a file that is not cannot be one.
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        let Ok(parsed) = parse_local_unit_file(&content, kind) else {
            continue;
        };
        let Some(component) = component_kind(&parsed.program, &parsed.arguments) else {
            continue;
        };
        // A unit that runs the bare binary serves no role: launchd starts it,
        // it prints its help and exits. There is nothing to merge, and it must
        // not refuse the one unit that replaces it.
        if parsed.arguments.len() <= 1 {
            eprintln!(
                "[host] {} runs {:?} with no command; left out of the host unit",
                path.display(),
                parsed.arguments
            );
            continue;
        }
        let mut plan = component_plan(component, &label)?;
        plan.label = label;
        let unit = match host.os {
            LocalOs::Darwin => plan.label.clone(),
            LocalOs::Linux => systemd_unit(&plan.label),
        };
        let definition = UnitFile {
            host: host.name.clone(),
            unit,
            path: path.to_string_lossy().into_owned(),
            kind,
            content,
        };
        let component = Component::from_definition(plan, definition)?;
        if !super::resident_role(&component.plan) {
            eprintln!(
                "[host] {} runs {:?}, which is not a resident Stado role; left as it is",
                component.plan.label, component.plan.exec_args
            );
            continue;
        }
        components.push(component);
    }
    components.sort_by(|left, right| left.plan.label.cmp(&right.plan.label));
    Ok(components)
}

fn retired_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".retired-{}", chrono::Utc::now().format("%Y%m%d")));
    PathBuf::from(name)
}

/// Where launchd keeps system-domain daemons.
const SYSTEM_DAEMON_DIRECTORY: &str = "/Library/LaunchDaemons";

fn sudo(arguments: &[&str]) -> CommandSpec {
    let mut argv = vec!["/usr/bin/sudo".to_string(), "-n".to_string()];
    argv.extend(arguments.iter().map(|argument| argument.to_string()));
    CommandSpec::new(argv)
}

/// Stop one replaced unit and move its file out of the init system's view.
async fn retire(
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

/// The roles an installed host unit already runs. Its argv is the only
/// record of them once the units it replaced are retired, so a later
/// installation starts from it rather than from a bare `stado serve`, and
/// takes its environment from the current configuration.
fn adopt_installed(mut host: InstallPlan, home: &Path) -> InstallPlan {
    let path = host.unit_path(home);
    let Some(content) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return host;
    };
    let Ok(parsed) = parse_local_unit_file(&content, native_kind(host.os)) else {
        return host;
    };
    if parsed.arguments.get(1).map(String::as_str) != Some("serve") {
        return host;
    }
    let mut arguments = parsed.arguments;
    arguments[0] = host.exec_args.first().cloned().unwrap_or(parsed.program);
    host.exec_args = arguments;
    host
}

/// Merge, install and retire, in that order. Nothing is written before the
/// merge has accepted every discovered unit.
pub(crate) async fn install(
    host: InstallPlan,
    home: &Path,
    component_plan: &dyn Fn(&str, &str) -> Result<InstallPlan, DeployError>,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let host = adopt_installed(host, home);
    let components = discover(&host, home, component_plan)?;
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| DeployError(format!("reading the registry: {error}")))?;
    let mut merged = merge(host, &components, &registry)?;
    // The first continuous unit supplies the native lifetime settings the
    // default rendering does not know, such as a raised descriptor limit.
    merged.startup = components
        .iter()
        .find_map(|component| component.render_startup(&merged).ok());
    for component in &components {
        echo(&format!(
            "[host] {} joins {}",
            component.plan.label, merged.label
        ));
    }
    execute_plan(&merged, home, current_uid(), runner, echo).await?;
    let mut failures = Vec::new();
    for component in &components {
        match retire(component, &merged, runner).await {
            Ok(()) => echo(&format!("[host] retired {}", component.plan.label)),
            Err(error) => failures.push(error.0),
        }
    }
    if !failures.is_empty() {
        return Err(DeployError(format!(
            "{} runs, but replaced units are still installed: {}",
            merged.label,
            failures.join("; ")
        )));
    }
    Ok(())
}
