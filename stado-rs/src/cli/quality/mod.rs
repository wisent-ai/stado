//! `stado quality format` — apply the formatting the product's own quality
//! gate checks.
//!
//! Every product declares its gates in `.wisent-release.json`, and each gate
//! states its `kind`. The `format` gates read the tree without changing it:
//! `cargo fmt … -- --check`. When such a gate refuses, something has to do the
//! writing, and that something used to be a person typing `cargo fmt` — a
//! command whose effect is a tree-wide rewrite nobody reviewed, which sweeps
//! other sessions' unformatted files into one commit. Tama refuses it for that
//! reason.
//!
//! So the writing belongs here, to the product, driven by the same declaration
//! the gate reads: the checking argv with its `--check` removed. A product
//! that declares no `format` gate is told so rather than guessed at, and the
//! command never invents a formatter the manifest does not name. The web
//! platform's `format` gate is `stado web quality`, which the check runs
//! ([`web`]) and `format` cannot write.
//!
//! `stado quality check` runs the same gate exactly as declared over the
//! committed tree an install would build, exported beside the checkout, so the
//! verdict a source install will reach can be read before an install is
//! handed to anyone: the checkout is not written, and a refusal names the gate.
//! Before the gates it asks cargo whether each committed `Cargo.lock` resolves
//! its manifest ([`lockfile`]), the question the install's `--locked` build
//! would otherwise answer only after it had started. It then stages pinned
//! inputs and runs the declared gates. For a catalogued CLI, it builds the
//! exact candidate executable in the disposable export, reads the executable's
//! complete help tree, and asks Brama to judge one-off commands, aliases and
//! incomplete object operations. A review no model answered is reported as
//! not run, with its cause; the gates decide the check. With `--place` the
//! check runs as placed work on a fleet host ([`check::place`]). `stado
//! quality hooks` puts the pre-push check in every checkout of a product
//! as its Git `pre-push` hook ([`hooks`]).

mod check;
mod gates;
mod hooks;
mod inputs;
mod lockfile;
mod units;
mod web;

use std::path::Path;
use std::process::Command;

pub use self::check::{check, command_tree};
pub(crate) use self::check::{
    check_bundle, check_revision, check_revision_for, check_source, place,
};
pub use self::hooks::{hooks, Verb as HooksVerb};
pub(crate) use units::check as units;

pub(crate) use self::gates::Selection;
use self::gates::{declared_gates, selected_gates, Declaration};
use crate::cli::CmdError;

/// Validate an arbitrary Stado argv with this binary's root parser without
/// dispatching it. A release checks the unit it would restart this way while
/// the previous executable still serves the host.
pub fn parse(argv: &[String]) -> Result<(), CmdError> {
    use clap::{CommandFactory, FromArgMatches};

    let matches = crate::cli::Cli::command()
        .try_get_matches_from(std::iter::once("stado").chain(argv.iter().map(String::as_str)))
        .map_err(|error| CmdError::refused(format!("unit argv rejected: {error}")))?;
    crate::cli::Cli::from_arg_matches(&matches)
        .map_err(|error| CmdError::refused(format!("unit argv rejected: {error}")))?;
    println!("accepted: stado {}", argv.join(" "));
    Ok(())
}

/// The argument that makes a formatter report instead of write.
const CHECK_FLAG: &str = "--check";

pub async fn format(root: Option<&str>) -> Result<(), CmdError> {
    let declared = selected_gates(root, Selection::Formatting)?;
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
        run(&writing, root, &[], Report::Stdout, Diagnostics::Text)?;
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
    pub(crate) fn say(self, line: &str) {
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

/// How a gate's diagnostics are read: from the records Cargo prints for a
/// gate that compiles with Cargo, or not at all, the gate's own report then
/// standing for them.
#[derive(Clone, Copy)]
pub(crate) enum Diagnostics {
    /// A Cargo operation that compiles: run with Cargo's records, each
    /// diagnostic read from them. Another program runs as declared.
    Compiled,
    /// Read no diagnostics; a refusal carries the gate's own report.
    Text,
}

/// Run one gate's argv in `root`. Its report is captured and then printed
/// where the check's lines go, so a refusal names what the gate found —
/// each compiler finding's file:line, lint and message from Cargo's records
/// ([`stado_product::findings`]), or, for a gate that reads none, the gate's
/// own report — instead of only its argv and exit status.
fn run(
    argv: &[String],
    root: &Path,
    env: &[(&str, String)],
    report: Report,
    diagnostics: Diagnostics,
) -> Result<(), CmdError> {
    let (program, args) = argv.split_first().ok_or_else(|| {
        CmdError::click("a quality gate declares an empty command")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let recorded =
        matches!(diagnostics, Diagnostics::Compiled) && program == stado_product::findings::CARGO;
    let unreadable = |error: anyhow::Error| {
        CmdError::click(format!("{}: {error:#}", argv.join(" ")))
            .stating(crate::primitives::failure::FailureCode::Config)
    };
    let args = if recorded {
        stado_product::findings::with_messages(args).map_err(unreadable)?
    } else {
        args.to_vec()
    };
    let mut command = Command::new(installed(program)?);
    command.args(&args).current_dir(root);
    command.envs(env.iter().map(|(name, value)| (*name, value.as_str())));
    let output = crate::wait::output(&mut command).map_err(|error| {
        CmdError::click(format!("cannot run {program}: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let messages = if recorded {
        stado_product::findings::Messages::read(&output.stdout).map_err(unreadable)?
    } else {
        stado_product::findings::Messages {
            rest: output.stdout,
            ..Default::default()
        }
    };
    let stdout = String::from_utf8_lossy(&messages.rest);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // A gate's own report (a formatter's diff on stdout, a compiler's
    // diagnostics as it renders them), in the order the streams are read.
    let said: Vec<&str> = [
        stdout.trim_end(),
        messages.rendered.trim_end(),
        stderr.trim_end(),
    ]
    .into_iter()
    .filter(|text| !text.is_empty())
    .collect();
    let gate_report = said.join("\n");
    // The report follows the check's lines.
    if !gate_report.is_empty() {
        report.say(&gate_report);
    }
    if output.status.success() {
        return Ok(());
    }
    let exited = match output.status.code() {
        Some(code) => code.to_string(),
        None => "on a signal".to_string(),
    };
    let found = messages.findings;
    let named = match found.as_slice() {
        [] => format!("the gate's own report:\n{gate_report}"),
        found => {
            let named: Vec<String> = found.iter().map(ToString::to_string).collect();
            format!("{} finding(s):\n{}", found.len(), named.join("\n"))
        }
    };
    Err(
        CmdError::refused(format!("{} exited {exited}; {named}", argv.join(" "))).detailing(
            serde_json::json!({
                "argv": argv,
                "exit": exited,
                "findings": found,
                "report": gate_report,
            }),
        ),
    )
}
