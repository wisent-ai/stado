//! Reading a product's CLI command tree and having a model judge it, shared
//! by the quality check, which refuses a tree with one-off commands, and by
//! `stado quality tree`, which lists them.

use std::collections::BTreeMap;
use std::path::Path;

use super::super::super::{gates::DeclaredGates, run, Diagnostics, Report};
use super::judged::{self, Reading};
use super::{cli_product, discover, staged_binaries};
use crate::cli::CmdError;

/// What reading a product's CLI command tree came to.
pub(super) enum Audit {
    /// The catalog declares no CLI surface for the product: no tree to read.
    NotCli,
    /// A model judged the tree; each command it refuses, one sentence each.
    Judged(Vec<String>),
    /// No model judgment was obtained, for this cause.
    Unjudged(String),
}

/// Build the CLI `declared` releases from the exported `tree` into `area`,
/// read its whole command tree, qualify its managed units when `qualify`,
/// and have a model judge the tree against the CLI rules.
pub(super) fn audit(
    declared: &DeclaredGates,
    tree: &Path,
    area: &Path,
    contract: &[(&str, String)],
    report: Report,
    qualify: bool,
) -> Result<Audit, CmdError> {
    if super::super::super::web::is_web_platform(&declared.platform)
        || !cli_product(&declared.product)?
    {
        return Ok(Audit::NotCli);
    }
    let output = area.join("cli-surface");
    std::fs::create_dir_all(&output).map_err(|cause| {
        CmdError::click(format!(
            "cannot create CLI artifact area {}: {cause}",
            output.display()
        ))
    })?;
    let mut env = contract.to_vec();
    env.push(("WISENT_OUTPUT_DIR", output.to_string_lossy().into_owned()));
    for (name, value) in &declared.env {
        env.push((name, value.clone()));
    }
    report.say(&format!(
        "stado quality check: building candidate CLI: {}",
        declared.build.join(" ")
    ));
    run(&declared.build, tree, &env, report, Diagnostics::Compiled).map_err(|cause| {
        cause.within(format!("building CLI command tree of {}", declared.product))
    })?;
    let mut command_tree = BTreeMap::new();
    let mut unjudged = None;
    for binary in staged_binaries(&output, &declared.stage)? {
        let mut path = Vec::new();
        if let Reading::Unanswered(cause) = discover(&binary, &mut path, None, &mut command_tree)? {
            if unjudged.is_none() {
                unjudged = Some(cause);
            }
        }
        if qualify {
            super::super::super::units::qualify(&binary, declared, report)?;
        }
    }
    let reading = match unjudged {
        Some(cause) => Reading::Unanswered(cause),
        None => judged::violations(&declared.product, &command_tree)?,
    };
    let issues = match reading {
        Reading::Answered(issues) => issues,
        Reading::Unanswered(cause) => return Ok(Audit::Unjudged(cause)),
    };
    let mut refused = hyphenated(&command_tree);
    for issue in issues {
        if !command_tree.contains_key(&issue.command)
            || issue.primitive.trim().is_empty()
            || issue.reason.trim().is_empty()
        {
            return Err(CmdError::refused(format!(
                "{}: CLI audit named an unknown command or omitted its primitive/reason: {} -> {}: {}",
                declared.product, issue.command, issue.primitive, issue.reason
            )));
        }
        refused.push(format!(
            "command `{}` duplicates or fragments primitive `{}`: {}",
            issue.command, issue.primitive, issue.reason
        ));
    }
    Ok(Audit::Judged(refused))
}

/// Every command of `tree` whose own name is several words joined by hyphens.
/// A command is named noun then verb, one word each (`token expiry` is the
/// group `token` with its verb, never the command `token-expiry`), so such a
/// name is refused from the tree's structure alone, without a model; the
/// binary's own name is the product's and is not judged.
fn hyphenated(tree: &BTreeMap<String, String>) -> Vec<String> {
    tree.keys()
        .filter_map(|command| {
            let mut words = command.split_whitespace();
            words.next()?;
            let own = words.last()?;
            own.contains('-').then(|| {
                format!(
                    "command `{command}` names `{own}` as one hyphenated word: name it noun then verb, one word each (`{}`)",
                    own.replace('-', " ")
                )
            })
        })
        .collect()
}

/// `stado quality tree`: every command of `revision`'s CLI the
/// model refuses, as the answer rather than a refusal, so what is left to
/// fold into a general primitive can be read at any revision. With
/// [`Report::Stderr`] stdout carries one document `{"product", "revision",
/// "cli", "remaining"}`; otherwise each remaining command is said as a line.
/// A tree no model judged is the refusal naming why: an inventory nobody
/// judged lists nothing.
pub(in crate::cli::quality::check) fn inventory(
    declared: &DeclaredGates,
    tree: &Path,
    area: &Path,
    contract: &[(&str, String)],
    revision: &str,
    report: Report,
) -> Result<(), CmdError> {
    let (cli, remaining) = match audit(declared, tree, area, contract, report, false)? {
        Audit::NotCli => (false, Vec::new()),
        Audit::Judged(remaining) => (true, remaining),
        Audit::Unjudged(cause) => {
            return Err(CmdError::refused(format!(
                "the CLI command tree of {} at {revision} was not judged ({cause}), so no \
                 inventory of its one-off commands can be given",
                declared.product
            )))
        }
    };
    match report {
        Report::Stderr => println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "product": declared.product,
                "revision": revision,
                "cli": cli,
                "remaining": remaining,
            }))?
        ),
        Report::Stdout if !cli => report.say(&format!(
            "{} declares no CLI surface, so it has no command tree to inventory",
            declared.product
        )),
        Report::Stdout => {
            report.say(&format!(
                "{} at {revision}: {} command(s) to fold into a general primitive",
                declared.product,
                remaining.len()
            ));
            for line in &remaining {
                report.say(&format!("  {line}"));
            }
        }
    }
    Ok(())
}
