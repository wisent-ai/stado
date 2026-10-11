//! A directory copied whole between this machine and a registry target over
//! the Stado SSH route the delivery transfer uses.

use std::path::Path;

use crate::deploy::{
    host_access::ssh_key, host_channel, shlex_quote, CommandSpec, DeployError, Runner,
};
use crate::targets::ComputeTarget;

/// Which way [`sync_directory`] copies a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The target's directory into the local one.
    Pull,
    /// The local directory into the target's.
    Push,
}

/// A directory copied whole between this machine and `target` with `rsync
/// -a --delete` over the same Stado SSH route [`super::transfer`] pushes with, so
/// the receiving side holds exactly the sending side's tree. `remote` is an
/// absolute path on the target. A placement move carries a state tree from
/// its source host through the controller and on to its destination this
/// way.
pub async fn sync_directory(
    target: &ComputeTarget,
    remote: &str,
    local: &Path,
    direction: Direction,
    runner: &Runner,
) -> Result<(), DeployError> {
    if !remote.starts_with('/') || remote.split('/').any(|part| part == "..") {
        return Err(DeployError(format!(
            "{}: {remote} is not a clean absolute directory",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    if direction == Direction::Pull {
        std::fs::create_dir_all(local).map_err(DeployError::io(format!(
            "cannot create {}",
            local.display()
        )))?;
    }
    let remote_tree = format!("{}/", remote.trim_end_matches('/'));
    let local_tree = format!("{}/", local.display());
    let mut argv = vec![
        "rsync".to_string(),
        "-a".to_string(),
        "--delete".to_string(),
    ];
    let key = if host_channel::target_is_this_host(target) {
        argv.push("--".to_string());
        match direction {
            Direction::Pull => argv.extend([remote_tree, local_tree]),
            Direction::Push => argv.extend([local_tree, remote_tree]),
        }
        None
    } else {
        let connection = host_channel::select_ssh_connection(target, runner).await?;
        let key = ssh_key::materialize(target.channel_key()).await?;
        let mut ssh = host_channel::ssh_options(connection.destination);
        ssh.pop();
        let ssh = ssh_key::add_identity(ssh, &key)?;
        let remote_shell = ssh
            .iter()
            .map(|word| shlex_quote(word))
            .collect::<Vec<_>>()
            .join(" ");
        let remote_tree = format!("{}:{remote_tree}", connection.destination);
        argv.extend(["-e".to_string(), remote_shell, "--".to_string()]);
        match direction {
            Direction::Pull => argv.extend([remote_tree, local_tree]),
            Direction::Push => argv.extend([local_tree, remote_tree]),
        }
        Some(key)
    };
    let output = runner(CommandSpec { argv, stdin: None })
        .await
        .map_err(DeployError::unreachable)?;
    drop(key);
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: copying the tree {remote} failed: {}",
            target.name,
            host_channel::last_error_line(&output, "rsync failed")
        )));
    }
    Ok(())
}
