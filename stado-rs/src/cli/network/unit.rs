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

/// The node's own copy of Stado, which its unit runs. A Stado install
/// replaces `~/.stado/bin/stado`; the node keeps running this copy, so macOS
/// keeps judging the node by the code identity it already decided Local
/// Network for, and only `network up` places a new one.
fn pinned_program(home: &Path) -> PathBuf {
    home.join(".stado").join("libexec").join("stado-network")
}

/// What placing this binary as the node's program does to the code identity
/// macOS decides Local Network by (its designated requirement).
pub(super) enum Identity {
    /// No node program is placed yet: macOS has never judged one here.
    Fresh,
    /// The placed program and this binary carry the same identity.
    Same,
    /// The identity changes from the first to the second.
    Changed(String, String),
}

/// The designated requirement macOS keeps a decision under, or the
/// signature's state when it has none worth keeping one under.
fn requirement(program: &Path) -> Result<String, CmdError> {
    let report = stado_product::inspect_signature(program).map_err(|error| {
        CmdError::declaration(format!(
            "the signature of {} cannot be read: {error:#}",
            program.display()
        ))
    })?;
    Ok(match report["requirement"].as_str() {
        Some(requirement) => requirement.to_string(),
        None => format!("a signature in state {}", report["state"]),
    })
}

/// What placing this binary as the node's program changes.
pub(super) fn identity_change() -> Result<Identity, CmdError> {
    let placed = pinned_program(&home()?);
    if !crate::primitives::presence::present(&placed)
        .map_err(|error| CmdError::declaration(format!("{}: {error}", placed.display())))?
    {
        return Ok(Identity::Fresh);
    }
    let incoming = std::env::current_exe().map_err(|error| {
        CmdError::declaration(format!(
            "this binary cannot name its own path to compare with the node's: {error}"
        ))
    })?;
    let (before, after) = (requirement(&placed)?, requirement(&incoming)?);
    Ok(if before == after {
        Identity::Same
    } else {
        Identity::Changed(before, after)
    })
}

/// Place this binary as the node's program: staged beside it, then renamed
/// over it, so a running node keeps the image it started from.
fn place(home: &Path) -> Result<PathBuf, CmdError> {
    let placed = pinned_program(home);
    let incoming = std::env::current_exe().map_err(|error| {
        CmdError::declaration(format!(
            "this binary cannot name its own path for the node to run: {error}"
        ))
    })?;
    let failed = |error: std::io::Error| {
        CmdError::click(format!(
            "the node's program {} could not be placed: {error}",
            placed.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    };
    if let Some(directory) = placed.parent() {
        std::fs::create_dir_all(directory).map_err(failed)?;
    }
    let staged = placed.with_extension("incoming");
    std::fs::copy(&incoming, &staged).map_err(failed)?;
    #[allow(
        clippy::disallowed_methods,
        reason = "places a verified program over the node's, not a document"
    )]
    std::fs::rename(&staged, &placed).map_err(failed)?;
    Ok(placed)
}

fn plan(target: &ComputeTarget) -> Result<(InstallPlan, PathBuf), CmdError> {
    let home = home()?;
    let os = LocalOs::detect().map_err(|error| CmdError::refused(error.to_string()))?;
    let program = pinned_program(&home);
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

/// Place this binary as the node's program, then write and load the unit,
/// restarting a node already running.
pub(super) async fn install(target: &ComputeTarget) -> Result<String, CmdError> {
    let (plan, home) = plan(target)?;
    place(&home)?;
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
    let placed = pinned_program(&home);
    if let Err(error) = std::fs::remove_file(&placed) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(CmdError::click(format!(
                "the node's program {} could not be removed: {error}",
                placed.display()
            )));
        }
    }
    Ok(path_text)
}
