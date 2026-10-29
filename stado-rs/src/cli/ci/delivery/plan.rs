//! The declared delivery plan: the products the delivery routine installs,
//! in order, each with the surface it installs and the canonical checkout
//! whose `main` it installs from. The operator declares it with `stado ci
//! plan set`; nothing is guessed in its place.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::state::{Pushed, State};
use crate::cli::CmdError;
use crate::primitives::document;

const PLAN_ENV: &str = "STADO_DELIVERY_PLAN";
const PLAN_FILE: &str = "delivery-plan.json";
const PLAN: &str = "delivery plan";

/// One product of the plan.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Step {
    pub product: String,
    pub surface: String,
    pub checkout: PathBuf,
}

impl Step {
    /// The step's name in the state and in every message: `PRODUCT:SURFACE`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.product, self.surface)
    }

    /// The repository the step's checkout is, as Oko names it in a recorded
    /// repair: the checkout's directory name.
    pub fn repository(&self) -> Result<&str, CmdError> {
        self.checkout
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} names no repository directory",
                    self.checkout.display()
                ))
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Whether a pass that installs every step then pushes `main` of every
    /// checkout: declared with `--push`, never assumed, because a push
    /// publishes the operator's commits.
    #[serde(default)]
    pub push: bool,
    /// The open Oko defect the routine's installs serve (`--purpose`), asked
    /// of the operator's rulings with every install: a consent he limited to
    /// that purpose (to repair CI/CD) admits the routine while the defect is
    /// open, and once it closes the routine is refused again. A plan declared
    /// before purposes existed has none and is refused until declared anew.
    pub purpose: String,
    pub declared_at: DateTime<Utc>,
    /// The seconds from the start of one pass to the start of the next
    /// (`--every`); a plan declared before cadences existed has none, and no
    /// pass is fired for it until it is declared again.
    #[serde(default)]
    pub every_seconds: Option<NonZeroU64>,
}

impl Plan {
    /// Push `main` of every checkout of the plan, each once, through the push
    /// gates the pass installed; every push's outcome, refused ones named.
    pub(super) async fn push(&self, current: &mut State) -> Result<Vec<String>, CmdError> {
        let mut refusals = Vec::new();
        let mut pushed: Vec<&Path> = Vec::new();
        for step in &self.steps {
            if pushed.contains(&step.checkout.as_path()) {
                continue;
            }
            pushed.push(&step.checkout);
            let revision = super::main_tip(&step.checkout).await?;
            let main = super::MAIN;
            let refspec = format!("refs/heads/{main}:refs/heads/{main}");
            let outcome =
                super::process("git", &["push", "origin", &refspec], Some(&step.checkout)).await;
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
            super::state::write(current)?;
        }
        Ok(refusals)
    }
}

/// The plan's path: `STADO_DELIVERY_PLAN`, else `~/.stado/delivery-plan.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    document::located(PLAN_ENV, PLAN_FILE, PLAN).map_err(CmdError::click)
}

/// The declared plan, or `None` when none is declared; one that cannot be
/// read or parsed is a refusal naming it.
pub fn read() -> Result<Option<Plan>, CmdError> {
    document::read(&path()?, PLAN).map_err(CmdError::click)
}

/// The declared plan, refused naming how to declare one when there is none.
pub fn declared() -> Result<Plan, CmdError> {
    let path = path()?;
    read()?.ok_or_else(|| {
        CmdError::refused(format!(
            "no delivery plan is declared at {}: `stado ci plan set --purpose DEFECT --step PRODUCT:SURFACE:CHECKOUT …` \
             declares the products the routine installs, in order, and the defect it installs them to repair",
            path.display()
        ))
    })
}

/// Every step's build and install, and the compile and test its main check's
/// gates run, asked of Oko's admission decision (`oko admit
/// build|install|compile|test --product P --purpose DEFECT`) before anything
/// is planned or run. The routine installs, and each install it starts builds
/// the product: the operator forbade both until every task and defect is
/// closed, and consented to them only to repair CI/CD. The plan's purpose
/// names the open defect it repairs; a consent he limited to that purpose
/// admits it while the defect is open, and every other purpose, or the same
/// one once it closed, meets the withdrawal quoted. A gate the rulings would
/// keep from running is refused here, before any commit is checked, so it is
/// never recorded as a red commit. Each admission is kept in this process's
/// environment for its action, so the `stado product install` a step starts
/// inherits it and does not ask again without the purpose. Stado keeps no
/// copy of the rule; the answer is Oko's.
pub(crate) async fn admitted(steps: &[Step], purpose: &str, doing: &str) -> Result<(), CmdError> {
    use crate::cli::release_loop::allowed::{refusal, BUILD, COMPILE, INSTALL, TEST};
    let mut refusals = Vec::new();
    for step in steps {
        for action in [BUILD, INSTALL, COMPILE, TEST] {
            if let Err(refused) = refusal(action, Some(&step.product), &[purpose]).await {
                refusals.push(refused);
            }
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

/// One `--step PRODUCT:SURFACE:CHECKOUT`, its checkout an existing git
/// checkout named absolutely.
fn parse(text: &str) -> Result<Step, CmdError> {
    let malformed = || CmdError::usage(format!("--step {text:?} is not PRODUCT:SURFACE:CHECKOUT"));
    let (product, rest) = text.split_once(':').ok_or_else(malformed)?;
    let (surface, checkout) = rest.split_once(':').ok_or_else(malformed)?;
    if product.is_empty() || surface.is_empty() || checkout.is_empty() {
        return Err(CmdError::usage(format!(
            "--step {text:?} leaves its product, surface or checkout empty"
        )));
    }
    let checkout = Path::new(checkout);
    if !checkout.is_absolute() {
        return Err(CmdError::usage(format!(
            "--step {text:?} names the checkout {} relatively; name it absolutely",
            checkout.display()
        )));
    }
    if !crate::primitives::presence::present(&checkout.join(".git"))? {
        return Err(CmdError::refused(format!(
            "--step {text:?}: {} holds no git checkout",
            checkout.display()
        )));
    }
    Ok(Step {
        product: product.to_string(),
        surface: surface.to_string(),
        checkout: checkout.to_path_buf(),
    })
}

fn print(plan: &Plan, json: bool) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(plan)?);
        return Ok(());
    }
    println!(
        "delivery plan declared at {}, serving defect {}, in order:",
        plan.declared_at, plan.purpose
    );
    for step in &plan.steps {
        println!("  - {} from {}", step.key(), step.checkout.display());
    }
    match plan.push {
        true => println!("  then push main of every checkout"),
        false => println!("  no push: declared without --push"),
    }
    match plan.every_seconds {
        Some(every) => println!("  a pass begins every {every} s"),
        None => println!("  no cadence declared: no pass is fired until --every is declared"),
    }
    Ok(())
}

/// `stado ci plan set --purpose DEFECT --step PRODUCT:SURFACE:CHECKOUT …
/// --every SECONDS [--push] [--json]`. A plan whose installs the operator's
/// rulings forbid now for that purpose is refused before it is written,
/// quoting the ruling ([`admitted`]): a declared plan is what the pass every
/// `stado serve` of this host fires at its cadence installs.
pub async fn set(
    purpose: &str,
    steps: &[String],
    push: bool,
    every: NonZeroU64,
    json: bool,
) -> Result<(), CmdError> {
    let purpose = purpose.trim();
    if purpose.is_empty() {
        return Err(CmdError::usage(
            "--purpose names the open Oko defect the routine installs to repair; it is empty",
        ));
    }
    if steps.is_empty() {
        return Err(CmdError::usage(
            "a delivery plan names at least one --step PRODUCT:SURFACE:CHECKOUT",
        ));
    }
    let steps = steps
        .iter()
        .map(|text| parse(text))
        .collect::<Result<Vec<_>, _>>()?;
    for (place, step) in steps.iter().enumerate() {
        if steps[..place]
            .iter()
            .any(|earlier| earlier.key() == step.key())
        {
            return Err(CmdError::usage(format!(
                "--step {} is named twice; a plan installs each step once",
                step.key()
            )));
        }
    }
    admitted(&steps, purpose, "declaring the delivery plan").await?;
    let plan = Plan {
        steps,
        push,
        purpose: purpose.to_string(),
        declared_at: Utc::now(),
        every_seconds: Some(every),
    };
    document::write(&path()?, &plan, PLAN).map_err(CmdError::click)?;
    print(&plan, json)
}

/// `stado ci plan show [--json]`: the plan, then when its next pass is due.
pub fn show(json: bool) -> Result<(), CmdError> {
    let plan = declared()?;
    print(&plan, json)?;
    let last = super::state::last_started()?;
    if !json {
        let due = crate::coordinator::delivery::next_due(&plan, last)?;
        println!("  next pass due at {due}");
    }
    Ok(())
}

/// `stado ci plan clear`: the routine installs nothing until a plan is
/// declared again.
pub fn clear() -> Result<(), CmdError> {
    let path = path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("delivery plan {} cleared", path.display());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(CmdError::refused(
            format!("no delivery plan is declared at {}", path.display()),
        )),
        Err(error) => Err(CmdError::click(format!(
            "removing {}: {error}",
            path.display()
        ))),
    }
}
