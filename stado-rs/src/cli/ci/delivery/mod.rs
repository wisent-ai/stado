//! The delivery routine: Stado installs the declared plan's products from the
//! `main` HEAD of their checkouts, in order, then pushes every repository when
//! the plan declares it, so delivering a repair waits on no agent's turn.
//!
//! `stado ci run` is one pass, run by Stado's scheduler on the operator's
//! cadence (`stado schedule create --cron … 'stado ci run'`), never per
//! commit. `main` HEAD holds the commits the compile-free push checks passed,
//! pushed or not: the repairs of the push gates wait there until the products
//! running those gates are installed. A pass installs every step whose `main`
//! it has not installed (`stado product install … --source-commit <main>
//! --wait`, every quality gate run to its end). A refused step stops the pass,
//! is kept with every refusal and recorded in Oko as one defect per session
//! whose repair the revision brought, and waits until its `main` moves: the
//! next pass resumes there, notes a refusal that remains on the same defects
//! and records the revision that installs as their repair. Then, with
//! `--push` declared, the pass pushes `main` of every checkout through the
//! gates just installed and keeps each outcome. One pass runs at a time.

mod oko;
pub mod plan;
pub mod state;
mod status;

pub use status::{delivery, print_status, status};

use std::path::Path;

use chrono::Utc;

use crate::cli::release_loop::process::{run as process, succeeds};
use crate::cli::CmdError;
use plan::{Plan, Step};
use state::{Delivered, Pass, Pushed, Refused, Running, State};

/// The branch the routine installs and pushes. A git ref name: the protocol
/// between the checkout, its origin and the install.
const MAIN: &str = "main";

/// The commit the checkout's `main` stands on: the work committed there,
/// pushed or not.
async fn main_tip(checkout: &Path) -> Result<String, CmdError> {
    let head = format!("refs/heads/{MAIN}");
    let tip = process("git", &["rev-parse", "--verify", &head], Some(checkout))
        .await
        .map_err(|error| {
            error.within(format!(
                "{} holds no {MAIN} branch to install from",
                checkout.display()
            ))
        })?;
    Ok(tip.trim().to_string())
}

/// Push `main` of every checkout of the plan, each once, through the push
/// gates the pass installed; every push's outcome, refused ones named.
async fn push(plan: &Plan, current: &mut State) -> Result<Vec<String>, CmdError> {
    let mut refusals = Vec::new();
    let mut pushed: Vec<&Path> = Vec::new();
    for step in &plan.steps {
        if pushed.contains(&step.checkout.as_path()) {
            continue;
        }
        pushed.push(&step.checkout);
        let revision = main_tip(&step.checkout).await?;
        let refspec = format!("refs/heads/{MAIN}:refs/heads/{MAIN}");
        let outcome = process("git", &["push", "origin", &refspec], Some(&step.checkout)).await;
        let refusal = outcome.err().map(|error| error.to_string());
        if let Some(refusal) = &refusal {
            refusals.push(format!("{}: {refusal}", step.checkout.display()));
        }
        current.pushed.insert(
            step.checkout.display().to_string(),
            Pushed {
                revision,
                at: Utc::now(),
                refusal,
            },
        );
        state::write(current)?;
    }
    Ok(refusals)
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
    /// Every step installed; what each step said, and every refused push.
    Delivered(Vec<String>, Vec<String>),
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
    if !plan.push {
        said.push("the plan declares no push".to_string());
        return Ok(Outcome::Delivered(said, Vec::new()));
    }
    let refused_pushes = push(plan, current).await?;
    said.push("pushed main of every checkout its push gates admitted".to_string());
    Ok(Outcome::Delivered(said, refused_pushes))
}

/// Every step's install asked of Oko's admission decision (`oko admit
/// install --product P --purpose DEFECT`) before anything is planned or run.
/// The routine installs and builds: the operator forbade both until every
/// task and defect is closed, and consented to them only to repair CI/CD.
/// The plan's purpose names the open defect it repairs; a consent he limited
/// to that purpose admits it while the defect is open, and every other
/// purpose, or the same one once it closed, meets the withdrawal quoted.
/// Stado keeps no copy of the rule; the answer is Oko's.
pub(crate) async fn admitted(steps: &[Step], purpose: &str, doing: &str) -> Result<(), CmdError> {
    let mut refusals = Vec::new();
    for step in steps {
        if let Err(refused) = crate::cli::release_loop::allowed::refusal(
            crate::cli::release_loop::allowed::INSTALL,
            Some(&step.product),
            &[purpose],
        )
        .await
        {
            refusals.push(refused);
        }
    }
    if refusals.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{doing} is refused: {}",
        refusals.join("; ")
    )))
}

/// `stado ci run [--json]`: one pass of the routine. A refused step exits
/// non-zero after the state and the defects are written. A pass the
/// operator's rulings forbid ([`admitted`]) is refused before it takes the
/// routine's lock, installs anything or writes the state.
pub async fn run(json: bool) -> Result<(), CmdError> {
    let plan = plan::declared()?;
    admitted(&plan.steps, &plan.purpose, "stado ci run").await?;
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
        Ok(Outcome::Delivered(said, pushes)) => match pushes.is_empty() {
            true => (said.join("; "), None),
            false => (
                format!(
                    "{}; {} push(es) refused: {}",
                    said.join("; "),
                    pushes.len(),
                    pushes.join("; ")
                ),
                None,
            ),
        },
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
    let outcome = outcome?;
    if json {
        println!("{}", serde_json::to_string_pretty(&current)?);
    } else {
        println!("stado ci run: {said}");
    }
    if let Some(refused) = refused {
        return Err(CmdError::refused(format!(
            "{} refused at {}, every refusal of the run:\n{}",
            refused.step,
            refused.revision,
            refused.refusals.join("\n")
        )));
    }
    match outcome {
        Outcome::Delivered(_, pushes) if !pushes.is_empty() => Err(CmdError::refused(format!(
            "every step is installed, and these pushes were refused:\n{}",
            pushes.join("\n")
        ))),
        _ => Ok(()),
    }
}
