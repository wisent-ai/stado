//! `stado errors`: every way an operation can fail, as the operation declares
//! it ([`wisent_errors::Operation`]).
//!
//! `list` names the operations that declare a failure catalogue, `show` prints
//! one operation's causes, and `check` is the lint Stado's quality gate runs:
//! it refuses a catalogue that is not one (`Operation::problems`) and an error
//! message in the source that says a failure happened without saying which,
//! behind a baseline that may only shrink.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{json, Value};
use wisent_errors::Operation;

use crate::cli::{CmdError, CLICK_ERROR_CODE};

/// Every operation that declares how it fails.
fn operations() -> Vec<&'static Operation> {
    crate::cli::fleet::enroll::failures::OPERATIONS.to_vec()
}

#[derive(Subcommand)]
pub(crate) enum ErrorsCommands {
    /// The operations that declare a failure catalogue, with how many causes
    /// each declares; with --markdown, every operation's documentation section.
    List {
        /// Emit the catalogues as JSON.
        #[arg(long, conflicts_with = "markdown")]
        json: bool,
        /// Emit the documentation sections, one per operation.
        #[arg(long)]
        markdown: bool,
    },
    /// One operation's causes: what failed, why, the state the refusal shows,
    /// what fixes it, its code, its exit status and the test that provokes it.
    Show {
        /// The operation, as `list` names it (`fleet.enroll`).
        operation: String,
        /// Emit the catalogue as JSON.
        #[arg(long, conflicts_with = "markdown")]
        json: bool,
        /// Emit the operation's documentation section.
        #[arg(long)]
        markdown: bool,
    },
    /// Refuse a catalogue that is not one, and an error message in the
    /// source under each --source directory that holds a generic sentence
    /// ("did not succeed", "something went wrong") beyond what --baseline
    /// allows. Exits non-zero naming each finding.
    Check {
        /// A source directory to scan; repeat for several.
        #[arg(long, required = true)]
        source: Vec<PathBuf>,
        /// `{"<path>": {"<sentence>": <count>}}`: the generic sentences each
        /// file may still hold until it is rewritten.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Emit the findings as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn dispatch(command: ErrorsCommands) -> Result<(), CmdError> {
    match command {
        ErrorsCommands::List { json, markdown } => list(json, markdown),
        ErrorsCommands::Show {
            operation,
            json,
            markdown,
        } => show(&operation, json, markdown),
        ErrorsCommands::Check {
            source,
            baseline,
            json,
        } => check(&source, baseline.as_deref(), json),
    }
}

fn catalogue(operation: &Operation) -> Result<Value, CmdError> {
    Ok(serde_json::from_str(&operation.to_json(CLICK_ERROR_CODE))?)
}

fn list(json: bool, markdown: bool) -> Result<(), CmdError> {
    let operations = operations();
    if json {
        let catalogues = operations
            .iter()
            .copied()
            .map(catalogue)
            .collect::<Result<Vec<_>, _>>()?;
        return crate::cli::print_answer(&json!({ "operations": catalogues }), true);
    }
    if markdown {
        let sections: Vec<String> = operations
            .iter()
            .map(|operation| operation.markdown(CLICK_ERROR_CODE))
            .collect();
        println!("{}", sections.join("\n"));
        return Ok(());
    }
    for operation in operations {
        println!(
            "{:<16} {:>3} causes  {}  {}",
            operation.id,
            operation.causes.len(),
            operation.command,
            operation.docs
        );
    }
    Ok(())
}

fn show(id: &str, json: bool, markdown: bool) -> Result<(), CmdError> {
    let operations = operations();
    let Some(operation) = operations.iter().find(|operation| operation.id == id) else {
        let declared: Vec<&str> = operations.iter().map(|operation| operation.id).collect();
        return Err(CmdError::usage(format!(
            "no operation '{id}' declares a failure catalogue; the operations that do: {}",
            declared.join(", ")
        )));
    };
    if json {
        return crate::cli::print_answer(&catalogue(operation)?, true);
    }
    if markdown {
        println!("{}", operation.markdown(CLICK_ERROR_CODE));
        return Ok(());
    }
    println!("{}  ({})", operation.command, operation.id);
    println!("{}", operation.summary);
    println!("documented at {}", operation.docs);
    for cause in operation.causes {
        println!();
        println!(
            "{}  [{}] exit {}",
            cause.id,
            cause.code,
            cause.exit_code(CLICK_ERROR_CODE)
        );
        println!("  failed:   {}", cause.failed);
        println!("  cause:    {}", cause.cause);
        println!("  observed: {}", cause.observed);
        println!("  fix:      {}", cause.remedy);
        println!("  test:     {}", cause.test);
    }
    Ok(())
}

/// One generic sentence found in the source.
struct Finding {
    path: String,
    line: usize,
    sentence: &'static str,
    reason: &'static str,
    text: String,
}

fn scan(directory: &Path, findings: &mut Vec<Finding>) -> Result<(), CmdError> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        CmdError::usage(format!(
            "the source directory {} cannot be read: {error}",
            directory.display()
        ))
    })?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            scan(&path, findings)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).map_err(|error| {
                CmdError::declaration(format!("{} cannot be read: {error}", path.display()))
            })?;
            for (index, line) in text.lines().enumerate() {
                for sentence in wisent_errors::operation::generic_sentences_in(line, false) {
                    findings.push(Finding {
                        path: path.display().to_string(),
                        line: index + 1,
                        sentence: sentence.text,
                        reason: sentence.reason,
                        text: line.trim().to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

type Baseline = BTreeMap<String, BTreeMap<String, usize>>;

fn check(sources: &[PathBuf], baseline: Option<&Path>, json: bool) -> Result<(), CmdError> {
    let mut problems: Vec<String> = Vec::new();
    for operation in operations() {
        problems.extend(operation.problems());
    }
    let mut findings = Vec::new();
    for source in sources {
        scan(source, &mut findings)?;
    }
    let allowed: Baseline = match baseline {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|error| {
                CmdError::usage(format!(
                    "the baseline {} cannot be read: {error}",
                    path.display()
                ))
            })?;
            serde_json::from_str(&text).map_err(|error| {
                CmdError::usage(format!(
                    "the baseline {} is not {{\"<path>\": {{\"<sentence>\": <count>}}}}: {error}",
                    path.display()
                ))
            })?
        }
        None => Baseline::new(),
    };
    let mut held: Baseline = Baseline::new();
    for finding in &findings {
        *held
            .entry(finding.path.clone())
            .or_default()
            .entry(finding.sentence.to_string())
            .or_default() += 1;
    }
    let allowance = |path: &str, sentence: &str| {
        allowed
            .get(path)
            .and_then(|sentences| sentences.get(sentence))
            .copied()
            .unwrap_or(0)
    };
    for finding in &findings {
        let count = held[&finding.path][finding.sentence];
        let allows = allowance(&finding.path, finding.sentence);
        if count > allows {
            problems.push(format!(
                "{}:{}: \"{}\" {} ({count} in this file, the baseline allows {allows}); declare the cause and say what failed, the state observed and what fixes it: {}",
                finding.path, finding.line, finding.sentence, finding.reason, finding.text
            ));
        }
    }
    for (path, sentences) in &allowed {
        for (sentence, allows) in sentences {
            let count = held
                .get(path)
                .and_then(|held| held.get(sentence))
                .copied()
                .unwrap_or(0);
            if count < *allows {
                problems.push(format!(
                    "the baseline allows {allows} \"{sentence}\" in {path}, which holds {count}; lower it to {count}"
                ));
            }
        }
    }
    let operations = operations();
    let causes: usize = operations
        .iter()
        .map(|operation| operation.causes.len())
        .sum();
    if json {
        crate::cli::print_answer(
            &json!({
                "operations": operations.len(),
                "causes": causes,
                "generic_sentences": findings.len(),
                "problems": problems,
            }),
            true,
        )?;
    } else {
        println!(
            "{} operation(s) declare {causes} cause(s); {} generic sentence(s) in the source; {} problem(s)",
            operations.len(),
            findings.len(),
            problems.len()
        );
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "the error standard is not met: {}",
        problems.join("; ")
    ))
    .detailing(json!({ "problems": problems }))
    .machine_readable(json))
}
