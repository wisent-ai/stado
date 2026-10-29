//! The delivery routine: Stado installs the declared plan's products, in
//! order, each at the newest commit of its checkout's `main` the main check
//! found green, never `main` HEAD, then pushes every repository when the
//! plan declares it, so delivering a repair waits on no agent's turn.
//!
//! `stado ci run` is one pass, run by Stado's scheduler on the operator's
//! cadence (`stado schedule create --cron … 'stado ci run'`). It first runs
//! the main check ([`crate::cli::ci::main`]) over every new commit of each
//! step's `main`, then installs every step whose last green commit it has
//! not installed (`stado product install … --source-commit <green> --wait`,
//! every quality gate run to its end). A step with no green commit waits
//! and the pass stops there. A refused install stops the pass, is kept with
//! every refusal and recorded in Oko as one defect per session whose repair
//! the revision brought, and waits until a newer commit is green: the next
//! pass resumes there, notes a refusal that remains on the same defects and
//! records the revision that installs as their repair. Then, with `--push`
//! declared, the pass pushes `main` of every checkout through the gates just
//! installed and keeps each outcome. One pass runs at a time.

pub(in crate::cli::ci) mod oko;
pub mod plan;
pub mod state;
mod status;

pub use status::{delivery, print_status, status};

use std::path::Path;

use chrono::Utc;

use crate::cli::ci::main::record::Record;
use crate::cli::release_loop::process::{run as process, succeeds};
use crate::cli::CmdError;
use plan::{Plan, Step};
use state::{Pass, Refused, Running, State};

/// The branch the routine checks, installs and pushes. A git ref name: the
/// protocol between the checkout, its origin and the install.
const MAIN: &str = "main";

/// The commit the checkout's `main` stands on: the work committed there,
/// pushed or not.
pub(in crate::cli::ci) async fn main_tip(checkout: &Path) -> Result<String, CmdError> {
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
    /// The recorded refusal still holds: no commit of its step newer than the
    /// refused one is green.
    Waiting(Refused),
    Refused(Refused),
    /// A step none of whose commits of `main` the main check found green:
    /// what the pass said up to it.
    NoGreen(Vec<String>),
}

/// The index the pass starts at: the refused step once a newer commit than
/// the refused one is green, the first step otherwise; the recorded refusal
/// itself while it still holds.
fn start(plan: &Plan, current: &mut State, checked: &Record) -> Result<usize, Box<Refused>> {
    let Some(refused) = current.refused.as_ref() else {
        return Ok(Default::default());
    };
    let Some(index) = plan
        .steps
        .iter()
        .position(|step| step.key() == refused.step)
    else {
        // The plan no longer holds the refused step: its refusal stands in
        // Oko, and the pass starts over the plan as declared.
        current.refused = None;
        return Ok(Default::default());
    };
    match checked.last_green(&refused.step) {
        Some(green) if green.revision != refused.revision => Ok(index),
        _ => Err(Box::new(refused.clone())),
    }
}

async fn pass(plan: &Plan, current: &mut State, checked: &Record) -> Result<Outcome, CmdError> {
    let first = match start(plan, current, checked) {
        Ok(first) => first,
        Err(refused) => return Ok(Outcome::Waiting(*refused)),
    };
    let mut said = Vec::new();
    for step in &plan.steps[first..] {
        let key = step.key();
        let Some(green) = checked.last_green(&key) else {
            said.push(format!(
                "{key} has no commit of {MAIN} the main check found green; it and every later \
                 step wait (`stado ci delivery` names {}'s last checked commit and why)",
                step.product
            ));
            return Ok(Outcome::NoGreen(said));
        };
        let revision = green.revision.clone();
        let repository = step.repository()?;
        if let Some(delivered) = current.delivered.get_mut(&key) {
            oko::again(delivered, &key, repository).await;
        }
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
        let earlier = current.refused.take().filter(|refused| refused.step == key);
        let doing = format!("The delivery routine's install of {key}");
        let mut unrecorded = Vec::new();
        if let Err(error) = installed {
            let mut refused = Refused {
                step: key,
                revision,
                refusals: vec![error.to_string()],
                defects: Vec::new(),
                at: Utc::now(),
                unrecorded: Vec::new(),
            };
            let before = earlier.as_ref().map(|earlier| earlier.defects.as_slice());
            let at = oko::Step {
                checkout: &step.checkout,
                product: &step.product,
                repository,
                since: since.as_deref(),
            };
            let finding = || oko::install_refused(repository, &refused);
            refused.defects =
                oko::settle(&at, &refused, finding, before, &doing, &mut unrecorded).await;
            let (key, revision) = (&refused.step, &refused.revision);
            for line in &unrecorded {
                eprintln!("stado ci run: {key} is refused at {revision}, and {line}");
            }
            refused.unrecorded = unrecorded;
            current.refused = Some(refused.clone());
            state::write(current)?;
            return Ok(Outcome::Refused(refused));
        }
        let delivered = oko::installed(&key, revision, earlier, repository, &mut said).await;
        current.delivered.insert(key, delivered);
        state::write(current)?;
    }
    if !plan.push {
        said.push("the plan declares no push".to_string());
        return Ok(Outcome::Delivered(said, Vec::new()));
    }
    let refused_pushes = plan.push(current).await?;
    said.push("pushed main of every checkout its push gates admitted".to_string());
    Ok(Outcome::Delivered(said, refused_pushes))
}

/// `stado ci run [--json]`: one pass of the routine: the main check over
/// every new commit of each step's `main`, then the installs. A refused step
/// exits non-zero after the state and the defects are written. The attempt
/// is kept first ([`state::attempted`]), so the plan's cadence counts it
/// whatever happens next. A pass the operator's rulings forbid
/// ([`plan::admitted`]) is refused before it takes the routine's lock,
/// checks or installs anything or writes the state.
pub async fn run(json: bool) -> Result<(), CmdError> {
    let plan = plan::declared()?;
    state::attempted(Utc::now())?;
    plan::admitted(&plan.steps, &plan.purpose, "stado ci run").await?;
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
    let outcome = match crate::cli::ci::main::check(&plan).await {
        Ok((checked, checks)) => pass(&plan, &mut current, &checked)
            .await
            .map(|outcome| (checks.join("; "), outcome)),
        Err(error) => Err(error),
    };
    let (said, refused) = match &outcome {
        Ok((checks, Outcome::Delivered(said, pushes))) => match pushes.is_empty() {
            true => (format!("{checks}; {}", said.join("; ")), None),
            false => (
                format!(
                    "{checks}; {}; {} push(es) refused: {}",
                    said.join("; "),
                    pushes.len(),
                    pushes.join("; ")
                ),
                None,
            ),
        },
        Ok((checks, Outcome::NoGreen(said))) => (format!("{checks}; {}", said.join("; ")), None),
        Ok((checks, Outcome::Waiting(refused))) => (
            format!(
                "{checks}; {} refused at {} and no newer commit of its {MAIN} is green; waiting for a commit that repairs: {}",
                refused.step,
                refused.revision,
                refused.defects.join(", ")
            ),
            None,
        ),
        Ok((checks, Outcome::Refused(refused))) => (
            format!(
                "{checks}; {} refused at {}; recorded on {}",
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
        failed: outcome.as_ref().err().map(ToString::to_string),
    });
    state::write(&current)?;
    let (_, outcome) = outcome?;
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
