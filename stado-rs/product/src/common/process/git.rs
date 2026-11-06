//! The one way this crate starts git, and the one way it hands a child the
//! environment git would otherwise read as naming another repository.
//!
//! A caller's environment can carry `GIT_DIR`, `GIT_INDEX_FILE`,
//! `GIT_WORK_TREE` and the other variables git reads as naming one
//! repository: a git hook sets them for its children, and a session that
//! builds a commit in an index of its own exports `GIT_INDEX_FILE`. A git
//! started with them reads that repository or index instead of the checkout
//! the call names, so every git this crate runs is started without them. A
//! call that means a different index sets it explicitly after the runner, and
//! that setting wins.

use super::checked;
use anyhow::{anyhow, Context, Result};
use std::{process::Command, sync::LazyLock};

/// The variables git reads as naming one repository, as the installed git
/// lists them (`git rev-parse --local-env-vars`), or why they could not be
/// listed.
static LOCAL_VARIABLES: LazyLock<std::result::Result<Vec<String>, String>> =
    LazyLock::new(|| {
        let mut listing = Command::new("git");
        listing.args(["rev-parse", "--local-env-vars"]);
        checked(&mut listing)
            .and_then(|output| {
                String::from_utf8(output.stdout)
                    .context("git rev-parse --local-env-vars printed no UTF-8")
            })
            .map(|listing| listing.lines().map(str::to_owned).collect())
            .map_err(|error| format!("{error:#}"))
    });

/// Clear from `command` every variable git reads as naming one repository,
/// so it reads, and a Cargo it starts fetches into, the checkout the command
/// names whatever repository or index the caller's environment names.
pub fn unbound(command: &mut Command) -> Result<&mut Command> {
    let names = LOCAL_VARIABLES.as_ref().map_err(|cause| {
        anyhow!("the variables git reads as naming a repository could not be listed: {cause}")
    })?;
    for name in names {
        command.env_remove(name);
    }
    Ok(command)
}

/// `git`, started without the caller's repository-naming variables.
pub fn command() -> Result<Command> {
    let mut git = Command::new("git");
    unbound(&mut git)?;
    Ok(git)
}
