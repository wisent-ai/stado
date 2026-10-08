//! Dependencies a web product's `package.json` reaches outside its own
//! checkout, answered from the release inputs its manifest declares instead
//! of from GitHub or a sibling checkout the builder does not have.
//!
//! Two shapes exist. A private Git dependency (`github:owner/repo` reached
//! over SSH) is answered from an input that is a Git bundle file, through
//! a Git config only this build reads (`--git-input`). A `file:` dependency on
//! a sibling repository (`file:../echo-web/packages/onboarding-web`) is
//! answered by a link at that path to a directory of an input
//! (`--link-input`).

use std::path::{Component, Path, PathBuf};

use super::command::run_with_path;
use crate::cli::web::builds::contract::worker::Worker;
use crate::cli::CmdError;

/// Where a release input was staged — the directory it was extracted into,
/// or the file it was mounted as — refused by name when the manifest declared
/// no such input for this platform. The variable is the one the release
/// worker (and `stado quality check`) publishes for every declared input.
fn input_directory(input: &str) -> Result<PathBuf, CmdError> {
    let variable = crate::cli::release_submit::input_variable(input);
    match std::env::var(&variable) {
        Ok(value) if !value.trim().is_empty() => Ok(PathBuf::from(value.trim())),
        Ok(_) | Err(std::env::VarError::NotPresent) => Err(CmdError::click(format!(
            "{variable} is not set: the manifest declares no input {input:?} for this platform, so nothing answers it"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)),
        Err(error) => Err(CmdError::click(format!("{variable} cannot be read: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)),
    }
}

/// `INPUT=OWNER/REPOSITORY.git` for each private Git dependency: the input is
/// a Git bundle mounted as a file, and a bundle is a repository Git clones
/// and lists by its path, so both spellings GitHub is reached by are
/// rewritten to that path. Answers the environment entry that points the
/// install and the build at that config; nothing when no input is declared.
pub(in crate::cli::web::builds) fn git_redirects(
    worker: &Worker,
    declared: &[String],
) -> Result<Vec<(String, String)>, CmdError> {
    if declared.is_empty() {
        return Ok(Vec::new());
    }
    let work = worker.output.join("work");
    std::fs::create_dir_all(&work).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", work.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let config = work.join("gitconfig");
    // Written afresh by each step: quality and build run in one work area,
    // and a second `git config` over a key that already holds two values
    // refuses rather than replacing them.
    if config.exists() {
        std::fs::remove_file(&config).map_err(|error| {
            CmdError::click(format!("cannot replace {}: {error}", config.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let entry = vec![(
        "GIT_CONFIG_GLOBAL".to_string(),
        config.display().to_string(),
    )];
    for declaration in declared {
        let (input, repository) = declaration.split_once('=').ok_or_else(|| {
            CmdError::usage(format!(
                "--git-input takes INPUT=OWNER/REPOSITORY.git, not {declaration:?}"
            ))
        })?;
        let bundle = input_directory(input)?;
        // A path, not a `file://` URL: Git reads a bundle only through a
        // local path, and a `file://` URL asks for a repository directory.
        let target = format!("url.{}.insteadOf", bundle.display());
        let origins = [
            format!("ssh://git@github.com/{repository}"),
            format!("git@github.com:{repository}"),
        ];
        for (index, origin) in origins.iter().enumerate() {
            let mut arguments = vec!["config", "--global"];
            if index > 0 {
                arguments.push("--add");
            }
            arguments.push(&target);
            arguments.push(origin);
            run_with_path(&worker.source, "git", &arguments, "Git", None, &entry)?;
        }
    }
    Ok(entry)
}

/// `PATH=INPUT[/SUBPATH]` for each `file:` dependency outside the package:
/// a link at PATH (relative to the package) to that directory of the input.
/// The link must land inside the release worker's work area — the checkout's
/// parent — and must not replace anything already there.
pub(in crate::cli::web::builds) fn link_inputs(
    worker: &Worker,
    project: &Path,
    declared: &[String],
) -> Result<(), CmdError> {
    let area = worker.source.parent().ok_or_else(|| {
        CmdError::click(format!(
            "{} has no parent directory, so no input can be linked beside it",
            worker.source.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    for declaration in declared {
        let (path, source) = declaration.split_once('=').ok_or_else(|| {
            CmdError::usage(format!(
                "--link-input takes PATH=INPUT[/SUBPATH], not {declaration:?}"
            ))
        })?;
        let (input, inside) = source.split_once('/').unwrap_or((source, ""));
        let target = input_directory(input)?.join(inside);
        if !target.is_dir() {
            return Err(CmdError::usage(format!(
                "--link-input {declaration:?}: {} is not a directory of input {input:?}",
                target.display()
            )));
        }
        let link = normalized(&project.join(path));
        if !link.starts_with(area) || link == area || link == worker.source {
            return Err(CmdError::usage(format!(
                "--link-input {declaration:?}: {} is outside the release work area {}",
                link.display(),
                area.display()
            )));
        }
        if let Ok(existing) = std::fs::read_link(&link) {
            // The link the quality step made, seen again by the build.
            if existing == target {
                continue;
            }
        }
        if link.symlink_metadata().is_ok() {
            return Err(CmdError::refused(format!(
                "--link-input {declaration:?}: {} already exists, and a link would replace it",
                link.display()
            )));
        }
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                CmdError::click(format!("cannot create {}: {error}", parent.display()))
                    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        }
        std::os::unix::fs::symlink(&target, &link).map_err(|error| {
            CmdError::click(format!(
                "cannot link {} to {}: {error}",
                link.display(),
                target.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        println!(
            "stado web: linked {} to input {input} ({})",
            link.display(),
            target.display()
        );
    }
    Ok(())
}

/// `path` with `.` and `..` resolved lexically, so a link's place is judged
/// by where it lands, not by how it was spelled.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
