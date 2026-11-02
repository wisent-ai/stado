//! A directory or one file copied between this machine and a registry target
//! over the Stado SSH route the delivery transfer uses.

use std::path::Path;

use crate::deploy::{
    host_access::ssh_key, host_channel, shlex_quote, CommandSpec, DeployError, Runner,
};
use crate::targets::ComputeTarget;

/// Which way [`sync_path`] copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The target's path into the local one.
    Pull,
    /// The local path into the target's.
    Push,
}

/// What [`sync_path`] copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Copied {
    /// A directory whole (`rsync -a --delete` of its contents), so the
    /// receiving side holds exactly the sending side's tree.
    Tree,
    /// One regular file.
    File,
}

/// How [`sync_path`] tells that what the receiving side already holds is the
/// sending side's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    /// rsync's quick check, size and modification time: enough when the
    /// receiving side holds nothing yet.
    Quick,
    /// The content: for a copy taken earlier from the same path while it still
    /// changed. The rsync macOS ships compares times in whole seconds, so a
    /// file rewritten at the same size within the second it was first copied
    /// passes the quick check and stays stale.
    Content,
}

/// `copied` at `remote` (an absolute path on `target`) and `local` made the
/// same with `rsync -a` over the same Stado SSH route [`super::transfer`]
/// pushes with; rsync sends only the blocks the receiving side lacks. A
/// placement move carries every state from its source host through this
/// machine and on to its destination this way, and a remote cargo build
/// pulls its artifacts.
pub async fn sync_path(
    target: &ComputeTarget,
    remote: &str,
    local: &Path,
    copied: Copied,
    direction: Direction,
    compare: Compare,
    runner: &Runner,
) -> Result<(), DeployError> {
    if !remote.starts_with('/') || remote.split('/').any(|part| part == "..") {
        return Err(DeployError(format!(
            "{}: {remote} is not a clean absolute path",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    if direction == Direction::Pull {
        let directory = match copied {
            Copied::Tree => Some(local),
            Copied::File => local.parent(),
        };
        if let Some(directory) = directory {
            std::fs::create_dir_all(directory).map_err(DeployError::io(format!(
                "cannot create {}",
                directory.display()
            )))?;
        }
    }
    let (remote_path, local_path, what) = match copied {
        Copied::Tree => (
            format!("{}/", remote.trim_end_matches('/')),
            format!("{}/", local.display()),
            "the tree",
        ),
        Copied::File => (remote.to_string(), local.display().to_string(), "the file"),
    };
    let mut argv = vec!["rsync".to_string(), "-a".to_string()];
    if copied == Copied::Tree {
        argv.push("--delete".to_string());
    }
    if compare == Compare::Content {
        argv.push("--checksum".to_string());
    }
    let key = if host_channel::target_is_this_host(target) {
        argv.push("--".to_string());
        match direction {
            Direction::Pull => argv.extend([remote_path, local_path]),
            Direction::Push => argv.extend([local_path, remote_path]),
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
        let remote_path = format!("{}:{remote_path}", connection.destination);
        argv.extend(["-e".to_string(), remote_shell, "--".to_string()]);
        match direction {
            Direction::Pull => argv.extend([remote_path, local_path]),
            Direction::Push => argv.extend([local_path, remote_path]),
        }
        Some(key)
    };
    let output = runner(CommandSpec { argv, stdin: None })
        .await
        .map_err(DeployError::unreachable)?;
    drop(key);
    if !output.succeeded() {
        return Err(DeployError::unreachable(format!(
            "{}: copying {what} {remote} failed: {}",
            target.name,
            host_channel::last_error_line(&output, "rsync failed")
        )));
    }
    Ok(())
}
