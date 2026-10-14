//! The main check: every new commit of each step's `main` in the delivery
//! plan, checked with the product's own declared gates (every kind, from its
//! release manifest) over that exact committed tree, exported beside the
//! checkout, on the host that runs `stado ci run` under the plan's purpose.
//! Each commit is recorded green or red with every refusal; the newest green
//! one is what the delivery routine installs, never `main` HEAD. A red
//! commit after a green one is recorded in Oko as one defect per session
//! whose repair the commit brought, found in that commit, so Oko attributes
//! the break to it; a red one after a red one notes on those defects, and
//! the next green one records itself as their repair.
//!
//! `stado ci status` and `stado ci delivery` carry the record under the
//! routine's `main`: each step's last checked and last green commit and every
//! commit checked; `stado ci delivery` names a step whose last checked commit
//! is red as a way the delivery path is down.

pub mod record;

use std::path::Path;

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::ci::delivery::state::Refused;
use crate::cli::ci::delivery::{main_tip, oko, plan};
use crate::cli::quality::{check_revision, Report, Selection};
use crate::cli::release_loop::process::{run as process, succeeds};
use crate::cli::CmdError;
use record::{Checked, Record};

/// The commits of `main` after `last` up to `tip`, oldest first; `tip`
/// alone when nothing was checked yet, or when `main` was rewritten so that
/// `last` is no longer behind it.
async fn new_commits(
    checkout: &Path,
    last: Option<&str>,
    tip: &str,
) -> Result<Vec<String>, CmdError> {
    let Some(last) = last else {
        return Ok(vec![tip.to_string()]);
    };
    if last == tip {
        return Ok(Vec::new());
    }
    if !succeeds("git", &["merge-base", "--is-ancestor", last, tip], checkout).await? {
        return Ok(vec![tip.to_string()]);
    }
    let range = format!("{last}..{tip}");
    let listed = process("git", &["rev-list", "--reverse", &range], Some(checkout)).await?;
    Ok(listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The finding a red commit of `main` is.
fn red_commit(repository: &str, product: &str, refused: &Refused) -> oko::Finding {
    oko::Finding {
        title: format!(
            "{repository}@{} on main fails {product}'s declared gates, with {} refusal(s)",
            refused.revision,
            refused.refusals.len()
        ),
        surface: format!(
            "stado ci run, main check of {} ({repository}@{})",
            refused.step, refused.revision
        ),
        impact: "The delivery routine installs the last green commit of main, so this step stays at it until a later commit is green",
        repair: "Repair every refusal named in the evidence in the source it names and commit it to main; the next main check records the green commit as the repair",
    }
}

/// Check one commit, settling it in Oko against the step's commit checked
/// before it.
async fn check_commit(
    step: &plan::Step,
    revision: String,
    checked: &Record,
) -> Result<Checked, CmdError> {
    let key = step.key();
    let repository = step.repository()?;
    let earlier_red = checked.last_checked(&key).filter(|done| !done.green);
    let verdict = check_revision(
        &step.checkout,
        &revision,
        Report::Stderr,
        Selection::Installation,
    )
    .await;
    let Err(error) = verdict else {
        if let Some(red) = earlier_red {
            let passed = format!("The main check found {key} green again at");
            oko::repaired(&red.defects, &passed, repository, &revision).await?;
        }
        return Ok(Checked {
            revision,
            at: Utc::now(),
            green: true,
            refusals: Vec::new(),
            defects: Vec::new(),
        });
    };
    let mut refused = Refused {
        step: key.clone(),
        revision: revision.clone(),
        refusals: vec![error.to_string()],
        defects: Vec::new(),
        at: Utc::now(),
    };
    refused.defects = match earlier_red {
        Some(red) => {
            let doing = format!("The main check of {key}");
            oko::refused_again(&red.defects, &doing, &refused).await?;
            red.defects.clone()
        }
        None => {
            let since = checked.last_green(&key).map(|green| green.revision.clone());
            let authors =
                oko::authors(&step.checkout, repository, since.as_deref(), &revision).await?;
            let finding = red_commit(repository, &step.product, &refused);
            oko::record(&step.product, repository, &finding, &refused, &authors).await?
        }
    };
    Ok(Checked {
        revision,
        at: refused.at,
        green: false,
        refusals: refused.refusals,
        defects: refused.defects,
    })
}

/// Check every new commit of each step's `main`, recording each as soon as
/// it is checked; the record and what the check said per step.
pub(in crate::cli::ci) async fn check(
    plan: &plan::Plan,
) -> Result<(Record, Vec<String>), CmdError> {
    let mut checked = record::read()?;
    let mut said = Vec::new();
    for step in &plan.steps {
        let key = step.key();
        let tip = main_tip(&step.checkout).await?;
        let last = checked.last_checked(&key).map(|done| done.revision.clone());
        let pending = new_commits(&step.checkout, last.as_deref(), &tip).await?;
        if pending.is_empty() {
            said.push(format!("{key}: main at {tip} already checked"));
            continue;
        }
        for revision in pending {
            let done = check_commit(step, revision, &checked).await?;
            said.push(format!(
                "{key}: {} {}",
                done.revision,
                if done.green { "green" } else { "red" }
            ));
            checked.steps.entry(key.clone()).or_default().push(done);
            record::write(&checked)?;
        }
    }
    Ok((checked, said))
}

/// The record as `stado ci status` and `stado ci delivery` carry it: per
/// step its last checked and last green commit, and every commit checked.
pub(in crate::cli::ci) fn document() -> Result<Value, CmdError> {
    let checked = record::read()?;
    let steps: serde_json::Map<String, Value> = checked
        .steps
        .iter()
        .map(|(key, commits)| {
            (
                key.clone(),
                json!({
                    "last_checked": checked.last_checked(key),
                    "last_green": checked.last_green(key),
                    "checked": commits,
                }),
            )
        })
        .collect();
    Ok(json!({ "record": record::path()?, "steps": steps }))
}

/// Each step whose last checked commit is red, as one sentence.
pub(in crate::cli::ci) fn red(document: &Value) -> Vec<String> {
    document["steps"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, step)| step["last_checked"]["green"] == false)
        .map(|(key, step)| {
            let last = &step["last_checked"];
            format!(
                "main of {key} is red at {} (installs stay at {}): {}",
                last["revision"], step["last_green"]["revision"], last["refusals"]
            )
        })
        .collect()
}

/// The record's lines in `stado ci status`'s and `stado ci delivery`'s plain
/// form.
pub(in crate::cli::ci) fn print(document: &Value) {
    for (key, step) in document["steps"].as_object().into_iter().flatten() {
        let last = &step["last_checked"];
        let verdict = match last["green"].as_bool() {
            Some(true) => "green",
            Some(false) => "red",
            None => "not checked",
        };
        println!(
            "  main of {key}: last checked {} {verdict}; last green {}",
            last["revision"], step["last_green"]["revision"]
        );
    }
}
