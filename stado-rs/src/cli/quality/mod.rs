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

mod check;
mod gates;
mod inputs;
mod lockfile;
mod web;

use std::path::Path;
use std::process::Command;

pub use self::check::check;
pub(crate) use self::check::check_revision;

pub(crate) use self::gates::Selection;
use self::gates::{declared_gates, format_gates, Declaration};
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
