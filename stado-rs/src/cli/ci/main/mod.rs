//! The main check: every new commit of each step's `main` in the delivery
//! plan, checked with the product's own declared gates (every kind, from its
//! release manifest) over that exact committed tree, exported beside the
//! checkout, on the host that runs `stado ci run` under the plan's purpose.
//! Each commit is recorded green or red with every refusal; the newest green
//! one is what the delivery routine installs, never `main` HEAD. A red
//! commit after a green one is recorded in Oko as one defect per session
//! whose repair the commit brought, found in that commit, so Oko attributes
//! the break to it; a red one after a red one notes on those defects, and
//! the next green one records itself as their repair. A write the register
//! refuses is kept on the commit's record as `unrecorded` and said on
//! stderr; the verdict stands and the check goes on.
//!
//! `stado ci status` and `stado ci delivery` carry the record under the
//! routine's `main`: each step's last checked and last green commit and every
//! commit checked; `stado ci delivery` names a step whose last checked commit
//! is red, and since when, as a way the delivery path is down (`document`).

mod document;
pub mod record;

pub(in crate::cli::ci) use document::{document, print, red, unrecorded};

use std::path::Path;

use chrono::Utc;

use crate::cli::ci::delivery::state::Refused;
use crate::cli::ci::delivery::{main_tip, oko, plan};
use crate::cli::quality::{check_revision_for, Report, Selection};
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
        criterion: format!(
            "`stado ci run` records a later commit of {repository} main green for {product}'s \
             declared gates"
        ),
    }
}

/// Check one commit, settling it in Oko against the step's commit checked
/// before it. Its compile and test gates run as work serving `purpose`, the
/// open defect the delivery plan names.
async fn check_commit(
    step: &plan::Step,
    revision: String,
    checked: &Record,
    purpose: &str,
) -> Result<Checked, CmdError> {
    let key = step.key();
    let repository = step.repository()?;
    let earlier_red = checked.last_checked(&key).filter(|done| !done.green);
    let verdict = check_revision_for(
        &step.checkout,
        &revision,
        Report::Stderr,
        Selection::Installation,
        &[purpose],
    )
    .await;
    let mut unrecorded = Vec::new();
    let Err(error) = verdict else {
        let unrepaired = match earlier_red {
            Some(red) => {
                let passed = format!("The main check found {key} green again at");
                let defects = &red.defects;
                oko::repaired(defects, &passed, repository, &revision, &mut unrecorded).await
            }
            None => Vec::new(),
        };
        for line in &unrecorded {
            eprintln!("stado ci run: {key} is green at {revision}, and {line}");
        }
        return Ok(Checked {
            revision,
            at: Utc::now(),
            green: true,
            refusals: Vec::new(),
            defects: Vec::new(),
            unrecorded,
            unrepaired,
        });
    };
    let mut refused = Refused {
        step: key.clone(),
        revision: revision.clone(),
        refusals: vec![error.to_string()],
        defects: Vec::new(),
        at: Utc::now(),
        unrecorded: Vec::new(),
    };
    let at = oko::Step {
        checkout: &step.checkout,
        product: &step.product,
        repository,
        since: checked
            .last_green(&key)
            .map(|green| green.revision.as_str()),
    };
    let before = earlier_red.map(|red| red.defects.as_slice());
    let doing = format!("The main check of {key}");
    let finding = || red_commit(repository, &step.product, &refused);
    refused.defects = oko::settle(&at, &refused, finding, before, &doing, &mut unrecorded).await;
    for line in &unrecorded {
        eprintln!("stado ci run: {key} is red at {revision}, and {line}");
    }
    Ok(Checked {
        revision,
        at: refused.at,
        green: false,
        refusals: refused.refusals,
        defects: refused.defects,
        unrecorded,
        unrepaired: Vec::new(),
    })
}

/// Record again the repairs each green commit of `step` keeps refused; each
/// keeps those the register refuses again, with its new sentences. The
/// record is written when any was asked again.
async fn repair_again(step: &plan::Step, checked: &mut Record) -> Result<(), CmdError> {
    let key = step.key();
    let repository = step.repository()?;
    let passed = format!("The main check found {key} green again at");
    let mut asked = false;
    let commits = checked.steps.get_mut(&key).into_iter().flatten();
    for done in commits.filter(|done| !done.unrepaired.is_empty()) {
        let mut unrecorded = Vec::new();
        let (defects, revision) = (&done.unrepaired, &done.revision);
        let refused = oko::repaired(defects, &passed, repository, revision, &mut unrecorded).await;
        done.unrepaired = refused;
        done.unrecorded = unrecorded;
        asked = true;
    }
    match asked {
        true => record::write(checked),
        false => Ok(()),
    }
}

/// Check every new commit of each step's `main`, recording each as soon as
/// it is checked, after recording again the repairs the register refused
/// before; the record and what the check said per step.
pub(in crate::cli::ci) async fn check(
    plan: &plan::Plan,
) -> Result<(Record, Vec<String>), CmdError> {
    let mut checked = record::read()?;
    let mut said = Vec::new();
    for step in &plan.steps {
        let key = step.key();
        repair_again(step, &mut checked).await?;
        let tip = main_tip(&step.checkout).await?;
        let last = checked.last_checked(&key).map(|done| done.revision.clone());
        let pending = new_commits(&step.checkout, last.as_deref(), &tip).await?;
        if pending.is_empty() {
            said.push(format!("{key}: main at {tip} already checked"));
            continue;
        }
        for revision in pending {
            let done = check_commit(step, revision, &checked, &plan.purpose).await?;
            let verdict = match done.green {
                true => "green",
                false => "red",
            };
            said.push(match done.unrecorded.is_empty() {
                true => format!("{key}: {} {verdict}", done.revision),
                false => format!(
                    "{key}: {} {verdict}, its register writes refused: {}",
                    done.revision,
                    done.unrecorded.join("; ")
                ),
            });
            checked.steps.entry(key.clone()).or_default().push(done);
            record::write(&checked)?;
        }
    }
    Ok((checked, said))
}
