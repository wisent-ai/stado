//! `stado quality format` — apply the formatting the product's own quality
//! gate checks.
//!
//! Every product declares its gates in `.wisent-release.json`, and the first
//! of them reads the tree without changing it: `cargo fmt … -- --check`. When
//! that gate refuses, something has to do the writing, and that something
//! used to be a person typing `cargo fmt` — a command whose effect is a
//! tree-wide rewrite nobody reviewed, which sweeps other sessions'
//! unformatted files into one commit. Tama refuses it for that reason.
//!
//! So the writing belongs here, to the product, driven by the same declaration
//! the gate reads: the checking argv with its `--check` removed. A product
//! that declares no formatting gate is told so rather than guessed at, and the
//! command never invents a formatter the manifest does not name. The web
//! platform is the one exception to `fmt`: its gate is `stado web quality`,
//! which the check runs ([`web`]) and `format` cannot write.
//!
//! `stado quality check` runs the same gate exactly as declared over the
//! committed tree an install would build, exported beside the checkout, so the
//! verdict a source install will reach can be read before an install is
//! handed to anyone: the checkout is not written, and a refusal names the gate.
//! Before the gates it asks cargo whether each committed `Cargo.lock` resolves
//! its manifest ([`lockfile`]), the question the install's `--locked` build
//! would otherwise answer only after it had started.

mod gates;
mod lockfile;
mod web;

use std::path::{Path, PathBuf};
use std::process::Command;

use self::gates::format_gates;
use crate::cli::CmdError;

/// The argument that makes a formatter report instead of write.
const CHECK_FLAG: &str = "--check";

pub async fn format(root: Option<&str>) -> Result<(), CmdError> {
    let declared = format_gates(root)?;
    if declared.web_version.is_some() {
        return Err(CmdError::refused(format!(
            "{} is a web product: its quality gate is `stado web quality`, which runs the \
             product's own typecheck and lint scripts and names no formatter for Stado to run; \
             format it with the product's own script",
            declared.product
        )));
    }
    for gate in &declared.gates {
        let argv = writing_argv(&gate.argv);
        println!("stado quality format: {}", argv.join(" "));
        run(&argv, &declared.root, &[], Report::Stdout)?;
    }
    println!(
        "stado quality format: {} formatted in {}",
        declared.product,
        declared.root.display()
    );
    Ok(())
}

/// Run the declared formatting gates over the committed tree an install of
/// the checkout at `root` would build, writing nothing to the checkout; the
/// first refusal is returned with the gate that refused.
///
/// `stado product install|update` exports the committed revision and runs its
/// quality there, so an uncommitted edit neither fails nor rescues it. The
/// check reads that same tree: `origin/main` when the tracked tree is clean
/// and behind it (the install advances to it), `HEAD` otherwise.
pub async fn check(root: Option<&str>) -> Result<(), CmdError> {
    let checkout = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?,
    };
    let revision = built_revision(&checkout)?;
    check_revision(&checkout, &revision, Report::Stdout)
}

/// Where a check's progress and verdict lines go: stdout when they are the
/// command's answer (`stado quality check`), stderr when another command's
/// answer owns stdout (`stado release changes submit --json`).
#[derive(Clone, Copy)]
pub(crate) enum Report {
    Stdout,
    Stderr,
}

impl Report {
    fn say(self, line: &str) {
        match self {
            Self::Stdout => println!("{line}"),
            Self::Stderr => eprintln!("{line}"),
        }
    }
}

/// Run the lock and formatting gates over `revision` of `checkout`, exported
/// beside it, writing nothing to the checkout.
pub(crate) fn check_revision(
    checkout: &Path,
    revision: &str,
    report: Report,
) -> Result<(), CmdError> {
    let scratch = checkout
        .join(".wisent-output")
        .join("quality")
        .join(format!(
            "{}-{}",
            std::process::id(),
            &revision[..12.min(revision.len())]
        ));
    stado_product::export_committed_source(checkout, revision, &scratch)
        .map_err(|error| CmdError::unreachable(format!("cannot export {revision}: {error:#}")))?;
    let verdict = check_tree(&scratch, checkout, revision, report);
    std::fs::remove_dir_all(&scratch).map_err(|error| {
        CmdError::click(format!("cannot remove {}: {error}", scratch.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    verdict
}

fn check_tree(
    tree: &Path,
    checkout: &Path,
    revision: &str,
    report: Report,
) -> Result<(), CmdError> {
    lockfile::check(tree, checkout, revision)?;
    let declared = format_gates(Some(&tree.to_string_lossy()))?;
    let contract = match &declared.web_version {
        Some(version) => web::worker_contract(tree, version)?,
        None => Vec::new(),
    };
    for gate in &declared.gates {
        report.say(&format!("stado quality check: {}", gate.argv.join(" ")));
        run(&gate.argv, tree, &contract, report).map_err(|error| {
            error
                .within(format!(
                    "stado quality check: gate {:?} of {} refuses {revision} of {}",
                    gate.name,
                    declared.product,
                    checkout.display()
                ))
                .also("`stado quality format` writes what it reads")
        })?;
    }
    report.say(&format!(
        "stado quality check: {} resolves its locks and passes its quality gates at {revision} of {}",
        declared.product,
        checkout.display()
    ));
    Ok(())
}

/// The revision an install of `checkout` builds: `origin/main` after a fetch
/// when the tracked tree is clean and `HEAD` is contained in it, else `HEAD`.
fn built_revision(checkout: &Path) -> Result<String, CmdError> {
    let head = git(checkout, &["rev-parse", "HEAD"])?;
    let dirty = !git(checkout, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    if dirty {
        return Ok(head);
    }
    git(checkout, &["fetch", "--quiet", "origin", "main"])?;
    let main = git(checkout, &["rev-parse", "origin/main"])?;
    let contained = Command::new("git")
        .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
        .current_dir(checkout)
        .status()
        .map_err(|error| {
            CmdError::click(format!("cannot run git merge-base: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?
        .success();
    Ok(if contained { main } else { head })
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(checkout)
        .output()
        .map_err(|error| {
            CmdError::click(format!("cannot run git {}: {error}", args.join(" ")))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The checking argv turned into the writing one: `--check` removed, and the
/// `--` separator dropped when nothing follows it.
fn writing_argv(argv: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = argv
        .iter()
        .filter(|arg| arg.as_str() != CHECK_FLAG)
        .cloned()
        .collect();
    if kept.last().map(String::as_str) == Some("--") {
        kept.pop();
    }
    kept
}

fn run(
    argv: &[String],
    root: &Path,
    env: &[(&str, String)],
    report: Report,
) -> Result<(), CmdError> {
    let (program, args) = argv.split_first().ok_or_else(|| {
        CmdError::click("a quality gate declares an empty command")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    command.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
    // A gate's own report (a formatter's diff) follows the check's lines.
    if let Report::Stderr = report {
        command.stdout(std::process::Stdio::from(std::io::stderr()));
    }
    let status = command.status().map_err(|error| {
        CmdError::click(format!("cannot run {program}: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if status.success() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{} exited {}",
        argv.join(" "),
        status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "on a signal".to_string())
    )))
}
