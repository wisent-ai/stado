//! The declared delivery plan: the products the delivery routine installs,
//! in order, each with the surface it installs and the canonical checkout
//! whose `main` it installs from. The operator declares it with `stado ci
//! plan set`; nothing is guessed in its place.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::state::{home_file, write_json};
use crate::cli::CmdError;

const PLAN_ENV: &str = "STADO_DELIVERY_PLAN";
const PLAN_FILE: &str = "delivery-plan.json";

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
    pub declared_at: DateTime<Utc>,
}

/// The plan's path: `STADO_DELIVERY_PLAN`, else `~/.stado/delivery-plan.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    home_file(PLAN_ENV, PLAN_FILE)
}

/// The declared plan, or `None` when none is declared; one that cannot be
/// read or parsed is a refusal naming it.
pub fn read() -> Result<Option<Plan>, CmdError> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CmdError::click(format!(
                "reading the delivery plan {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        CmdError::click(format!(
            "the delivery plan {} is not a delivery plan: {error}",
            path.display()
        ))
    })
}

/// The declared plan, refused naming how to declare one when there is none.
pub fn declared() -> Result<Plan, CmdError> {
    let path = path()?;
    read()?.ok_or_else(|| {
        CmdError::refused(format!(
            "no delivery plan is declared at {}: `stado ci plan set --step PRODUCT:SURFACE:CHECKOUT …` \
             declares the products the routine installs, in order",
            path.display()
        ))
    })
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
    if !checkout.join(".git").exists() {
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
    println!("delivery plan declared at {}, in order:", plan.declared_at);
    for step in &plan.steps {
        println!("  - {} from {}", step.key(), step.checkout.display());
    }
    Ok(())
}

/// `stado ci plan set --step PRODUCT:SURFACE:CHECKOUT … [--json]`.
pub fn set(steps: &[String], json: bool) -> Result<(), CmdError> {
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
    let plan = Plan {
        steps,
        declared_at: Utc::now(),
    };
    write_json(&path()?, &plan)?;
    print(&plan, json)
}

/// `stado ci plan show [--json]`.
pub fn show(json: bool) -> Result<(), CmdError> {
    print(&declared()?, json)
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
