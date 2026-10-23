//! What the delivery routine and the main check write in Oko's defect
//! register, through Oko's own CLI: one defect per authoring session for a
//! refused step or a red commit of main, carrying every refusal, its
//! revision recorded as found-in so Oko attributes it to the commit; a note
//! on those defects when the next revision still refuses; and the repair on
//! them once a revision installs ([`repairs`]).
//!
//! Every write goes through [`write`]: what the routine found (a red commit,
//! a refused install, a green commit or an install after one) is a fact
//! whether or not the register takes the record of it, so a write the
//! register refuses (a judge it cannot reach, a step it asks for first)
//! never ends the pass. Each refusal is one sentence naming the write and the
//! register's own words, kept in the record of what it was about, where
//! `stado ci delivery` lists it. A defect the register says an open one
//! already restates is that one: its evidence is noted there and its id kept.

mod repairs;

use std::path::Path;

use serde::Deserialize;

use super::state::Refused;
use super::{process, succeeds};
use crate::cli::CmdError;
pub use repairs::{again, installed, repaired};

/// Oko's program; the routine records in Oko through its CLI only.
const OKO: &str = "oko";

/// What a refused record names each defect it restates with, as the flag
/// that would record it as distinct from them.
const RESTATES: &str = "--distinct-from ";

#[derive(Deserialize)]
struct Recorded {
    id: String,
}

/// One write in the register: Oko's answer, or the register's refusal as
/// one sentence naming the write (`what`).
async fn write(what: &str, args: &[&str]) -> Result<String, String> {
    process(OKO, args, None)
        .await
        .map_err(|error| format!("{what} was not recorded: {error}"))
}

/// The defects a refused record restates, as the register names them for
/// `--distinct-from`; none when it refused for another reason.
fn restated(refusal: &str) -> Vec<String> {
    let mut named = refusal.split(RESTATES);
    named.next();
    named
        .filter_map(|rest| rest.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Where a refusal is settled: the step's checkout, product and repository,
/// and the revision it last delivered or found green, when there is one.
pub struct Step<'a> {
    pub checkout: &'a Path,
    pub product: &'a str,
    pub repository: &'a str,
    pub since: Option<&'a str>,
}

/// Settle `refused` in the register after `earlier`, the defects the step's
/// refusal before it stands on, when there was one: a note on them when there
/// are any (`doing` names what refused), else `finding` recorded once per
/// author. The defects it stands on; a refusal before it whose defect the
/// register never took has nothing to note on, so it is recorded anew. When
/// the authors cannot be read, the defect is recorded with no owner; every
/// read or write that failed is kept in `unrecorded`.
pub async fn settle(
    step: &Step<'_>,
    refused: &Refused,
    finding: impl FnOnce() -> Finding,
    earlier: Option<&[String]>,
    doing: &str,
    unrecorded: &mut Vec<String>,
) -> Vec<String> {
    if let Some(earlier) = earlier.filter(|defects| !defects.is_empty()) {
        refused_again(earlier, doing, refused, unrecorded).await;
        return earlier.to_vec();
    }
    let revision = &refused.revision;
    let authors = match owners(step, revision).await {
        Ok(authors) => authors,
        Err(error) => {
            unrecorded.push(format!(
                "the sessions whose repairs {}@{revision} brought were not read, so its \
                 defect is recorded with no owner: {error}",
                step.repository
            ));
            Vec::new()
        }
    };
    record(step, &finding(), refused, &authors, unrecorded).await
}

/// The sessions owning the open defects whose recorded repair `revision` of
/// the step's checkout carries and its `since` does not: the authors of the
/// work the refused revision brought. The repairs are read as the release
/// loop reads them.
async fn owners(step: &Step<'_>, revision: &str) -> Result<Vec<String>, CmdError> {
    let (checkout, repository, since) = (step.checkout, step.repository, step.since);
    let repairs = crate::cli::release_loop::open_repairs().await?;
    let mut authors: Vec<String> = Vec::new();
    for repair in repairs {
        let Some(owner) = repair.owner else {
            continue;
        };
        if repair.repository != repository || authors.contains(&owner) {
            continue;
        }
        let carried = succeeds(
            "git",
            &["merge-base", "--is-ancestor", &repair.commit, revision],
            checkout,
        )
        .await?;
        let landed = match since {
            Some(since) => {
                succeeds(
                    "git",
                    &["merge-base", "--is-ancestor", &repair.commit, since],
                    checkout,
                )
                .await?
            }
            None => false,
        };
        if carried && !landed {
            authors.push(owner);
        }
    }
    Ok(authors)
}

/// The words of one refused finding as Oko records it, with the observable
/// result that completes its repair (`--criterion`: Oko records no defect
/// without one).
pub struct Finding {
    pub title: String,
    pub surface: String,
    pub impact: &'static str,
    pub repair: &'static str,
    pub criterion: String,
}

/// The finding a refused install of the delivery routine is.
pub fn install_refused(repository: &str, refused: &Refused) -> Finding {
    Finding {
        title: format!(
            "The delivery routine's install of {} at {}@{} refused, with {} refusal(s) in one run",
            refused.step,
            repository,
            refused.revision,
            refused.refusals.len()
        ),
        surface: format!(
            "stado ci run, step {} (stado product install at {repository}@{})",
            refused.step, refused.revision
        ),
        impact: "The delivery routine stops at this step: it and every later step of the plan stay uninstalled until a commit on main repairs every refusal",
        repair: "Repair every refusal named in the evidence in the source it names and commit it to main; the next pass of stado ci run resumes at this step",
        criterion: format!(
            "a later pass of stado ci run installs step {} from a commit of {repository} main that \
             carries the repair, and `stado ci delivery` shows no refused step",
            refused.step
        ),
    }
}

/// Record `refused` as `finding` once per author (once with no owner when
/// the revision carries no recorded repair), each defect carrying every
/// refusal and found in the step's repository @ the refused revision; the
/// ids the register recorded, each write it refused kept in `unrecorded`.
/// A record the register refuses as restating open defects stands on them:
/// the evidence is noted on each and their ids are the answer.
async fn record(
    step: &Step<'_>,
    finding: &Finding,
    refused: &Refused,
    authors: &[String],
    unrecorded: &mut Vec<String>,
) -> Vec<String> {
    let (product, repository) = (step.product, step.repository);
    let evidence = format!(
        "Every refusal of the run, each gate run to its end:\n{}",
        refused.refusals.join("\n")
    );
    let found = format!("{repository}@{}", refused.revision);
    let owners: Vec<Option<&String>> = match authors.is_empty() {
        true => vec![None],
        false => authors.iter().map(Some).collect(),
    };
    let what = format!("oko defects record of {found}");
    let mut ids = Vec::with_capacity(owners.len());
    for owner in owners {
        let args: &[&str] = &[
            "defects",
            "record",
            "--product",
            product,
            "--title",
            &finding.title,
            "--surface",
            &finding.surface,
            "--evidence",
            &evidence,
            "--impact",
            finding.impact,
            "--repair",
            finding.repair,
            "--found-in",
            &found,
            "--criterion",
            &finding.criterion,
            "--json",
        ];
        let answer = match write(&what, args).await {
            Ok(answer) => answer,
            Err(line) => {
                let restates = restated(&line);
                if restates.is_empty() {
                    unrecorded.push(line);
                    continue;
                }
                let text = format!("{} at {found}, restated. {evidence}", finding.title);
                note(&restates, &text, unrecorded).await;
                ids.extend(restates);
                break;
            }
        };
        let id = match serde_json::from_str::<Recorded>(&answer) {
            Ok(recorded) => recorded.id,
            Err(error) => {
                unrecorded.push(format!(
                    "{what} answered no recorded defect: {error}: {answer}"
                ));
                continue;
            }
        };
        if let Some(owner) = owner {
            let assigned = format!("oko defects assign {id} --owner {owner}");
            let args: &[&str] = &[
                "defects",
                "assign",
                &id,
                "--owner",
                owner.as_str(),
                "--priority",
                "high",
            ];
            if let Err(line) = write(&assigned, args).await {
                unrecorded.push(line);
            }
        }
        ids.push(id);
    }
    ids
}

/// Note on `defects` that what refused (`doing`, as "The delivery routine's
/// install of P:S") still refused at a later revision.
async fn refused_again(
    defects: &[String],
    doing: &str,
    refused: &Refused,
    unrecorded: &mut Vec<String>,
) {
    let text = format!(
        "{doing} still refused at {}, every gate run to its end:\n{}",
        refused.revision,
        refused.refusals.join("\n")
    );
    note(defects, &text, unrecorded).await;
}

/// Note `text` on each of `defects`; each note the register refused is kept
/// in `unrecorded`.
async fn note(defects: &[String], text: &str, unrecorded: &mut Vec<String>) {
    for defect in defects {
        let what = format!("oko defects note {defect}");
        let args: &[&str] = &["defects", "note", defect, "--text", text];
        if let Err(line) = write(&what, args).await {
            unrecorded.push(line);
        }
    }
}
