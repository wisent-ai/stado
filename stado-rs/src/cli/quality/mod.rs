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
//! would otherwise answer only after it had started. After the gates it asks
//! the store whether every release input the manifest pins is there, the
//! question the build's input staging would otherwise answer after the batch
//! carrying the commit had queued.

mod gates;
mod inputs;
mod lockfile;
mod web;

use std::path::{Path, PathBuf};
use std::process::Command;

use self::gates::format_gates;
pub(crate) use self::gates::Selection;
use crate::cli::CmdError;

/// The argument that makes a formatter report instead of write.
const CHECK_FLAG: &str = "--check";

pub async fn format(root: Option<&str>) -> Result<(), CmdError> {
    let declared = format_gates(root, Selection::Formatting)?;
    if declared.web_version.is_some() {
        return Err(CmdError::refused(format!(
            "{} is a web product: its quality gate is `stado web quality`, which runs the \
             product's own typecheck and lint scripts and names no formatter for Stado to run; \
             format it with the product's own script",
            declared.product
        )));
    }
    for gate in &declared.gates {
        converge(gate, &declared.root, &declared.product)?;
    }
    println!(
        "stado quality format: {} formatted in {}",
        declared.product,
        declared.root.display()
    );
    Ok(())
}

/// Write with the gate's formatter until the gate's own check passes.
///
/// One rustfmt pass is not always a fixed point: a match arm whose body is a
/// literal too long for the line is rewritten into a block only on the next
/// pass, so a single write left the tree the gate still refused and the
/// install that followed failed on `fmt` again. The check is the judge, so it
/// runs after every write; the loop ends when it passes, and refuses when a
/// write left the check's report exactly as it was, because another pass
/// would change nothing either.
fn converge(
    gate: &crate::release_pipeline::QualityGate,
    root: &Path,
    product: &str,
) -> Result<(), CmdError> {
    let writing = writing_argv(&gate.argv);
    let mut previous: Option<Vec<u8>> = None;
    loop {
        println!("stado quality format: {}", writing.join(" "));
        run(&writing, root, &[], Report::Stdout)?;
        let (program, args) = gate.argv.split_first().ok_or_else(|| {
            CmdError::click("a quality gate declares an empty command")
                .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        let checked = crate::wait::output(
            Command::new(installed(program)?)
                .args(args)
                .current_dir(root),
        )
        .map_err(|error| {
            CmdError::click(format!("cannot run {program}: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        if checked.status.success() {
            return Ok(());
        }
        let report = [checked.stdout, checked.stderr].concat();
        if previous.as_deref() == Some(report.as_slice()) {
            return Err(CmdError::refused(format!(
                "gate {:?} of {product} still refuses {} after `{}` left its report unchanged, \
                 so another pass would not satisfy it either; the gate reports:\n{}",
                gate.name,
                root.display(),
                writing.join(" "),
                String::from_utf8_lossy(&report).trim_end()
            )));
        }
        println!(
            "stado quality format: gate {:?} still refuses after that pass; writing again",
            gate.name
        );
        previous = Some(report);
    }
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
    check_revision(&checkout, &revision, Report::Stdout, Selection::Formatting).await
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
/// beside it, writing nothing to the checkout, with every release input the
/// manifest pins staged beside the export as the release worker stages it.
pub(crate) async fn check_revision(
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
) -> Result<(), CmdError> {
    // The gates run in the exported tree with that tree as their working
    // directory, and a gate such as `stado web quality` reads the tree from
    // WISENT_SOURCE_DIR. A checkout named relatively (`--source .`) made that
    // path relative to the caller's directory, so the gate looked for
    // `./.wisent-output/quality/...` inside the tree itself and refused with
    // "not a directory". The scratch is named from the absolute checkout.
    let checkout = &std::path::absolute(checkout).map_err(|error| {
        CmdError::click(format!(
            "cannot resolve the checkout {}: {error}",
            checkout.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    // Each run owns one directory holding the exported tree and its staged
    // inputs, as a release job owns its work area. A `--link-input` lands
    // beside the tree (the tree's parent is the work area it may write), so
    // with the tree exported straight under `.wisent-output/quality/` every
    // run's link landed in that shared directory, outlived the run, and
    // refused every later run with "already exists".
    let run_area = checkout
        .join(".wisent-output")
        .join("quality")
        .join(format!(
            "{}-{}",
            std::process::id(),
            &revision[..12.min(revision.len())]
        ));
    let scratch = run_area.join("source");
    stado_product::export_committed_source(checkout, revision, &scratch)
        .map_err(|error| CmdError::unreachable(format!("cannot export {revision}: {error:#}")))?;
    let inputs_area = run_area.join("inputs");
    let verdict = check_tree(
        &scratch,
        &inputs_area,
        checkout,
        revision,
        report,
        selection,
    )
    .await;
    if run_area.exists() {
        std::fs::remove_dir_all(&run_area).map_err(|error| {
            CmdError::click(format!("cannot remove {}: {error}", run_area.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let product = verdict?;
    report.say(&format!(
        "stado quality check: {product} resolves its locks, passes its quality gates and finds its \
         release inputs stored at {revision} of {}",
        checkout.display()
    ));
    Ok(())
}

/// The gates of the tree at `tree`, with its pinned inputs staged under
/// `inputs_area` first; answers the product.
async fn check_tree(
    tree: &Path,
    inputs_area: &Path,
    checkout: &Path,
    revision: &str,
    report: Report,
    selection: Selection,
) -> Result<String, CmdError> {
    lockfile::check(tree, checkout, revision, report)?;
    let declared = format_gates(Some(&tree.to_string_lossy()), selection)?;
    // A pin whose object was never stored, or was stored for a lock the
    // product has since moved past, is refused here, while the session that
    // pushed it can still repair it, instead of by the build that fetches it;
    // a gate that reads an input finds it where the worker would put it.
    let staged = inputs::stage(&declared, inputs_area, report).await?;
    let contract = match &declared.contract_version {
        Some(version) => web::worker_contract(tree, version, &declared.platform)?,
        None => Vec::new(),
    };
    let contract: Vec<(&str, String)> = contract
        .iter()
        .map(|(name, value)| (&**name, value.clone()))
        .chain(
            staged
                .iter()
                .map(|(name, value)| (name.as_str(), value.clone())),
        )
        .collect();
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
    Ok(declared.product)
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
    let contained = crate::wait::status(
        Command::new("git")
            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
            .current_dir(checkout),
    )
    .map_err(|error| {
        CmdError::click(format!("cannot run git merge-base: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?
    .success();
    Ok(if contained { main } else { head })
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = crate::wait::output(Command::new("git").args(args).current_dir(checkout))
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

/// The path this machine runs a gate's program from, resolved through the
/// fleet's declared install paths rather than this process's `PATH`.
pub(super) fn installed(program: &str) -> Result<std::path::PathBuf, CmdError> {
    crate::deploy::host_exec::installed_program(program).map_err(|missing| {
        CmdError::click(format!("cannot run {program}: {missing}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })
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
    let mut command = Command::new(installed(program)?);
    command.args(args).current_dir(root);
    command.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
    // A gate's own report (a formatter's diff) follows the check's lines.
    if let Report::Stderr = report {
        command.stdout(std::process::Stdio::from(std::io::stderr()));
    }
    let status = crate::wait::status(&mut command).map_err(|error| {
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
