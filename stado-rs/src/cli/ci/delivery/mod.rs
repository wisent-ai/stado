//! The delivery routine: Stado installs the declared plan's products from the
//! current `main` of their checkouts, in order, so delivering a repair does
//! not wait on any agent's turn.
//!
//! `stado ci run` is one pass, run by Stado's own scheduler on the cadence
//! the operator declares (`stado schedule create --cron … 'stado ci run'`),
//! never per commit. A pass reads each step's `main` on its origin and runs
//! `stado product install PRODUCT --surface SURFACE --source-commit <main>
//! --wait` for every step whose `main` it has not installed yet; the install
//! runs every quality gate to its end, so its refusal names every refusal of
//! the run. A refused step stops the pass (the later steps build on it), is
//! kept with every refusal and recorded in Oko as one defect per session whose
//! recorded repair the refused revision brought, and is not installed again
//! until its `main` moves: the next pass resumes at that step, notes a refusal
//! that remains on the same defects, and records the revision that installs
//! as their repair. One pass runs at a time; a pass that finds another
//! running says which step it is on and since when, and ends.

mod oko;
pub mod plan;
pub mod state;
mod status;

pub use status::{print_status, status};

use std::path::Path;

use chrono::Utc;

use crate::cli::release_loop::process::{run as process, succeeds};
use crate::cli::CmdError;
use plan::{Plan, Step};
use state::{Delivered, Pass, Refused, Running, State};

/// The branch the routine installs from. A git ref name: the protocol
/// between the checkout, its origin and the install.
const MAIN: &str = "main";

/// The commit `main` stands on at the checkout's origin now, fetched into
/// the checkout so the install can export it.
async fn main_tip(checkout: &Path) -> Result<String, CmdError> {
    let head = format!("refs/heads/{MAIN}");
    let listed = process("git", &["ls-remote", "origin", &head], Some(checkout)).await?;
    let tip = listed
        .lines()
        .find_map(|line| {
            let (object, reference) = line.split_once('\t')?;
            (reference == head).then(|| object.to_string())
        })
        .ok_or_else(|| {
            CmdError::refused(format!(
                "the origin of {} holds no {MAIN} branch to install from",
                checkout.display()
            ))
        })?;
    process("git", &["fetch", "--quiet", "origin", MAIN], Some(checkout)).await?;
    Ok(tip)
}

/// Install `step` at `revision` through this Stado's own product install;
/// the install's refusal is the error, every gate of it named.
async fn install(step: &Step, revision: &str) -> Result<(), CmdError> {
    let stado = std::env::current_exe().map_err(|error| {
        CmdError::click(format!("this Stado cannot name its executable: {error}"))
    })?;
    let stado = stado.to_string_lossy();
    process(
        &stado,
        &[
            "product",
            "install",
            &step.product,
            "--surface",
            &step.surface,
            "--source-commit",
            revision,
            "--wait",
        ],
        None,
    )
    .await
    .map(drop)
}

/// What one pass ended with.
enum Outcome {
    Delivered(Vec<String>),
    Waiting(Refused),
    Refused(Refused),
}

/// The index the pass starts at: the refused step once its `main` moved, the
/// first step otherwise; `Err` with the refusal while its `main` has not moved.
async fn start(plan: &Plan, current: &mut State) -> Result<Result<usize, Refused>, CmdError> {
    let Some(refused) = current.refused.clone() else {
        return Ok(Ok(Default::default()));
    };
    let Some(index) = plan
        .steps
        .iter()
        .position(|step| step.key() == refused.step)
    else {
        // The plan no longer holds the refused step: its refusal stands in
        // Oko, and the pass starts over the plan as declared.
        current.refused = None;
        return Ok(Ok(Default::default()));
    };
    if main_tip(&plan.steps[index].checkout).await? == refused.revision {
        return Ok(Err(refused));
    }
    Ok(Ok(index))
}

async fn pass(plan: &Plan, current: &mut State) -> Result<Outcome, CmdError> {
    let first = match start(plan, current).await? {
        Ok(first) => first,
        Err(refused) => return Ok(Outcome::Waiting(refused)),
    };
    let mut said = Vec::new();
    for step in &plan.steps[first..] {
        let key = step.key();
        let revision = main_tip(&step.checkout).await?;
        let since = current
            .delivered
            .get(&key)
            .map(|delivered| delivered.revision.clone());
        if since.as_deref() == Some(revision.as_str()) {
            said.push(format!("{key} already installed at {revision}"));
            continue;
        }
        current.running = Some(Running {
            step: key.clone(),
            revision: revision.clone(),
            started_at: Utc::now(),
        });
        state::write(current)?;
        let installed = install(step, &revision).await;
        current.running = None;
        let repository = step.repository()?;
        let earlier = current.refused.take().filter(|refused| refused.step == key);
        if let Err(error) = installed {
            let mut refused = Refused {
                step: key,
                revision,
                refusals: vec![error.to_string()],
                defects: Vec::new(),
                at: Utc::now(),
            };
            refused.defects = match earlier {
                Some(earlier) => {
                    oko::refused_again(&earlier.defects, &refused).await?;
                    earlier.defects
                }
                None => {
                    let authors = oko::authors(
                        &step.checkout,
                        repository,
                        since.as_deref(),
                        &refused.revision,
                    )
                    .await?;
                    oko::record(&step.product, repository, &refused, &authors).await?
                }
            };
            current.refused = Some(refused.clone());
            state::write(current)?;
            return Ok(Outcome::Refused(refused));
        }
        if let Some(earlier) = earlier {
            oko::repaired(&earlier.defects, &key, repository, &revision).await?;
        }
        said.push(format!("{key} installed at {revision}"));
        current.delivered.insert(
            key,
            Delivered {
                revision,
                at: Utc::now(),
            },
        );
        state::write(current)?;
    }
    Ok(Outcome::Delivered(said))
}

/// `stado ci run [--json]`: one pass of the routine. A refused step exits
/// non-zero after the state and the defects are written.
pub async fn run(json: bool) -> Result<(), CmdError> {
    let plan = plan::declared()?;
    let Some(_held) = state::lock()? else {
        let current = state::read()?;
        let said = match &current.running {
            Some(running) => format!(
                "another pass is installing {} at {} since {}; this pass ends",
                running.step, running.revision, running.started_at
            ),
            None => "another pass holds the routine and has not started a step yet; this pass ends"
                .to_string(),
        };
        println!("stado ci run: {said}");
        return Ok(());
    };
    let mut current = state::read()?;
    let started_at = Utc::now();
    let outcome = pass(&plan, &mut current).await;
    let (said, refused) = match &outcome {
        Ok(Outcome::Delivered(said)) => (said.join("; "), None),
        Ok(Outcome::Waiting(refused)) => (
            format!(
                "{} refused at {} and its {MAIN} has not moved; waiting for a commit that repairs: {}",
                refused.step,
                refused.revision,
                refused.defects.join(", ")
            ),
            None,
        ),
        Ok(Outcome::Refused(refused)) => (
            format!(
                "{} refused at {}; recorded on {}",
                refused.step,
                refused.revision,
                refused.defects.join(", ")
            ),
            Some(refused.clone()),
        ),
        Err(error) => (format!("the pass could not finish: {error}"), None),
    };
    current.running = None;
    current.last_pass = Some(Pass {
        started_at,
        ended_at: Utc::now(),
        said: said.clone(),
    });
    state::write(&current)?;
    outcome?;
    if json {
        println!("{}", serde_json::to_string_pretty(&current)?);
    } else {
        println!("stado ci run: {said}");
    }
    match refused {
        Some(refused) => Err(CmdError::refused(format!(
            "{} refused at {}, every refusal of the run:\n{}",
            refused.step,
            refused.revision,
            refused.refusals.join("\n")
        ))),
        None => Ok(()),
    }
}
