//! The system-domain spelling, for a Mac whose recorded console is headless:
//! macOS asks a launchd daemon nothing, and it loads with nobody logged in.
//! Root places the plist and loads it; the account it runs as is read out of
//! the home the plan already renders under.

use std::fs;
use std::path::Path;

use crate::deploy::local_install::unit::InstallPlan;
use crate::deploy::{write_if_changed, CommandSpec, DeployError, Runner};

/// The account a daemon spelling of a unit must name, read from the home
/// directory the plan already renders its logs and binaries under. macOS puts
/// an account's home at `/Users/<account>`.
///
/// `$USER` is deliberately not consulted: under `sudo` it names root, and a
/// daemon that ran the fleet's control binary as uid 0 against an
/// account-owned `~/.stado` is exactly the trade
/// [`crate::deploy::local_install::daemon_plist_text`] exists to refuse.
///
/// Visible to the whole module for
/// [`crate::deploy::local_install::install_local`], which resolves the account
/// before it builds a plan.
pub(in crate::deploy::local_install) fn account_of(home: &Path) -> Result<String, DeployError> {
    home.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "cannot read the account name out of home {}",
                home.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })
}

/// Install and load the daemon spelling: stage the rendered plist in the
/// account's own Stado directory, then let root place it in
/// `/Library/LaunchDaemons` and load it in the one domain this host has.
///
/// `sudo -n`, never a prompt: this runs unattended from `stado bootstrap` and
/// from the coordinator migration, so a step that blocks on a tty is a step
/// that hangs. A host without the grant is told which command was refused —
/// the same contract [`crate::deploy::service`]'s remote `ENSURE_BODY` holds
/// over the host channel.
///
/// `bootout` before `bootstrap` because launchd holds the definition it loaded
/// and a rewritten file under a live job changes nothing; an absent label
/// makes it fail, which is not an error here. This login's LaunchAgent of the
/// same label is stopped before the daemon loads and its file removed after;
/// a daemon that does not load is booted out and the agent loaded again.
///
/// `pub(super)` for [`super::execute_plan`], which routes a plan with a
/// daemon account here instead of down the per-login ladder.
pub(super) async fn install_darwin_daemon(
    plan: &InstallPlan,
    home: &Path,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let path = plan.unit_path(home);
    let staged = home
        .join(".stado")
        .join(format!("{}.plist.staged", plan.label));
    if let Some(directory) = staged.parent() {
        fs::create_dir_all(directory).map_err(DeployError::from)?;
    }
    write_if_changed(&staged, &plan.content(home)).map_err(DeployError::from)?;
    let sudo = |argv: Vec<&str>| {
        let mut spec = vec!["/usr/bin/sudo".to_string(), "-n".to_string()];
        spec.extend(argv.into_iter().map(str::to_string));
        CommandSpec::new(spec)
    };
    let unit_path = path.to_string_lossy().into_owned();
    let install = runner(sudo(vec![
        "/usr/bin/install",
        "-m",
        "644",
        "-o",
        "root",
        "-g",
        "wheel",
        &staged.to_string_lossy(),
        &unit_path,
    ]))
    .await
    .map_err(DeployError::unreachable)?;
    if let Some(left) = crate::primitives::staging::remove(&staged) {
        echo(&format!("[plist] {left}"));
    }
    if !install.succeeded() {
        return Err(DeployError::unreachable(format!(
            "sudo -n install {unit_path} was refused: {}",
            install.detail()
        )));
    }
    echo(&format!("[plist] installed {unit_path}"));
    // The same label loaded as this login's LaunchAgent would run the same
    // program beside the daemon; it is stopped first and its file kept until
    // the daemon is loaded, so a daemon that does not load leaves the agent
    // running again.
    let agent = home
        .join(crate::deploy::service::USER_LAUNCHD_AGENTS)
        .join(format!("{}.plist", plan.label));
    let agent_domains = stop_agent(&plan.label, runner, echo).await?;
    let loaded = load_daemon(plan, &unit_path, runner, &sudo, echo).await;
    if let Err(failure) = loaded {
        let service = format!("system/{}", plan.label);
        let unloaded = runner(sudo(vec!["/bin/launchctl", "bootout", &service])).await;
        let said = match &unloaded {
            Ok(ran) if ran.succeeded() => None,
            Ok(ran) => Some(ran.detail().trim().to_string()),
            Err(error) => Some(error.to_string()),
        };
        if let Some(said) = said {
            echo(&format!(
                "[note] sudo -n launchctl bootout {service}: {said}"
            ));
        }
        let given_back = restart_agent(&plan.label, &agent, &agent_domains, runner).await;
        return Err(DeployError::unreachable(format!(
            "{}{given_back}",
            failure.message
        )));
    }
    if agent.exists() {
        fs::remove_file(&agent).map_err(|error| {
            DeployError(format!(
                "{} runs as a system LaunchDaemon, and its LaunchAgent {} could not be removed: {error}",
                plan.label,
                agent.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        echo(&format!("[plist] removed LaunchAgent {}", agent.display()));
    }
    echo(&format!(
        "[ok]   loaded system LaunchDaemon {} (logs: {})",
        plan.label,
        home.join(".stado")
            .join("logs")
            .join(format!("{}.log", plan.label))
            .display()
    ));
    Ok(())
}

/// Boot this login's LaunchAgent of `label` out of every per-login domain
/// that holds it, returning those domains.
async fn stop_agent(
    label: &str,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<Vec<String>, DeployError> {
    let uid = super::commands::current_uid();
    let mut stopped = Vec::new();
    for domain in [format!("gui/{uid}"), format!("user/{uid}")] {
        let job = format!("{domain}/{label}");
        let held = runner(CommandSpec::new(vec![
            "/bin/launchctl".to_string(),
            "print".to_string(),
            job.clone(),
        ]))
        .await
        .map_err(DeployError::unreachable)?;
        if !held.succeeded() {
            continue;
        }
        let out = runner(CommandSpec::new(vec![
            "/bin/launchctl".to_string(),
            "bootout".to_string(),
            job.clone(),
        ]))
        .await
        .map_err(DeployError::unreachable)?;
        if !out.succeeded() {
            return Err(DeployError::unreachable(format!(
                "the LaunchAgent {job} could not be stopped before its daemon starts: {}",
                out.detail().trim()
            )));
        }
        echo(&format!("[agent] stopped {job}"));
        stopped.push(domain);
    }
    Ok(stopped)
}

/// Load the agent again from its kept file in the domains it was stopped in,
/// as the sentence a failed daemon install appends.
async fn restart_agent(label: &str, agent: &Path, domains: &[String], runner: &Runner) -> String {
    let mut said = Vec::new();
    for domain in domains {
        let bootstrap = runner(CommandSpec::new(vec![
            "/bin/launchctl".to_string(),
            "bootstrap".to_string(),
            domain.clone(),
            agent.to_string_lossy().into_owned(),
        ]))
        .await;
        said.push(match bootstrap {
            Ok(ran) if ran.succeeded() => format!("{domain}/{label} loaded again"),
            Ok(ran) => format!(
                "{domain}/{label} could not be loaded again: {}",
                ran.detail().trim()
            ),
            Err(error) => format!("{domain}/{label} could not be loaded again: {error}"),
        });
    }
    if said.is_empty() {
        return String::new();
    }
    format!("; the LaunchAgent was put back: {}", said.join("; "))
}

/// Bootstrap, enable and kickstart the daemon whose plist root now holds.
async fn load_daemon(
    plan: &InstallPlan,
    unit_path: &str,
    runner: &Runner,
    sudo: &dyn Fn(Vec<&str>) -> CommandSpec,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let service = format!("system/{}", plan.label);
    // A daemon that was not loaded refuses bootout, which is the state
    // bootstrap needs; any refusal is said, and bootstrap decides.
    let unloaded = runner(sudo(vec!["/bin/launchctl", "bootout", &service]))
        .await
        .map_err(DeployError::unreachable)?;
    if !unloaded.succeeded() {
        echo(&format!(
            "[note] sudo -n launchctl bootout {service}: {}",
            unloaded.detail().trim()
        ));
    }
    let bootstrap = runner(sudo(vec![
        "/bin/launchctl",
        "bootstrap",
        "system",
        unit_path,
    ]))
    .await
    .map_err(DeployError::unreachable)?;
    if !bootstrap.succeeded() {
        return Err(DeployError::unreachable(format!(
            "sudo -n launchctl bootstrap system {unit_path} failed: {}",
            bootstrap.detail()
        )));
    }
    // The daemon is reported loaded below, so an enable or kickstart that
    // launchctl refused is the install's failure, not an [ok] line.
    for step in [
        vec!["/bin/launchctl", "enable", &service],
        vec!["/bin/launchctl", "kickstart", "-k", &service],
    ] {
        let line = step.join(" ");
        let ran = runner(sudo(step)).await.map_err(DeployError::unreachable)?;
        if !ran.succeeded() {
            return Err(DeployError::unreachable(format!(
                "sudo -n {line} failed: {}",
                ran.detail()
            )));
        }
    }
    Ok(())
}
