//! The system-domain spelling: the only domain an always-on mac with no
//! graphical session has. Root places the plist and loads it; the account it
//! runs as is read out of the home the plan already renders under.

use std::fs;
use std::path::Path;

use crate::deploy::local_install::unit::InstallPlan;
use crate::deploy::{write_if_changed, CommandSpec, DeployError, Runner};

/// The account a daemon spelling of a unit must name, read from the home
/// directory the plan already renders its logs and binaries under. macOS puts
/// an account's home at `/Users/<account>`, the same derivation
/// [`crate::deploy::service::MisdeclaredDomain`] reads out of a declared
/// LaunchAgent path — one convention, not two.
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
/// makes it fail, which is not an error here. This is the one place that
/// sequence is safe: a unit in a domain that never existed has no live job to
/// take down.
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
        fs::create_dir_all(directory).map_err(|exc| DeployError(exc.to_string()))?;
    }
    write_if_changed(&staged, &plan.content(home)).map_err(|exc| DeployError(exc.to_string()))?;
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
    .map_err(DeployError)?;
    let _ = fs::remove_file(&staged);
    if !install.ok() {
        return Err(DeployError(format!(
            "sudo -n install {unit_path} was refused: {}",
            install.detail()
        )));
    }
    echo(&format!("[plist] installed {unit_path}"));
    let service = format!("system/{}", plan.label);
    let _ = runner(sudo(vec!["/bin/launchctl", "bootout", &service]))
        .await
        .map_err(DeployError)?;
    let bootstrap = runner(sudo(vec![
        "/bin/launchctl",
        "bootstrap",
        "system",
        &unit_path,
    ]))
    .await
    .map_err(DeployError)?;
    if !bootstrap.ok() {
        return Err(DeployError(format!(
            "sudo -n launchctl bootstrap system {unit_path} failed: {}",
            bootstrap.detail()
        )));
    }
    let _ = runner(sudo(vec!["/bin/launchctl", "enable", &service]))
        .await
        .map_err(DeployError)?;
    let _ = runner(sudo(vec!["/bin/launchctl", "kickstart", "-k", &service]))
        .await
        .map_err(DeployError)?;
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
