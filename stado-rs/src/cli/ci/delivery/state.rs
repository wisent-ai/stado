//! Where the delivery routine stands: the step it is installing and since
//! when, the step that refused with every refusal of that run and the defects
//! recorded for it, and the revision each step last delivered. One file, read
//! by `stado ci status` and Stado Desktop, rewritten whole by each pass.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;
use crate::primitives::document;

const STATE_ENV: &str = "STADO_DELIVERY_STATE";
const STATE_FILE: &str = "delivery-state.json";
const STATE: &str = "delivery state";

/// The step a pass is installing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Running {
    pub step: String,
    pub revision: String,
    pub started_at: DateTime<Utc>,
}

/// The step whose installation refused, and every refusal of that run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Refused {
    pub step: String,
    pub revision: String,
    pub refusals: Vec<String>,
    /// The defects recorded in Oko for the refusal, one per authoring session.
    pub defects: Vec<String>,
    pub at: DateTime<Utc>,
    /// Each write about this refusal the register refused, with its words.
    #[serde(default)]
    pub unrecorded: Vec<String>,
}

/// The revision a step last installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivered {
    pub revision: String,
    pub at: DateTime<Utc>,
    /// Each repair record of the install the register refused, with its
    /// words; the step is installed whether or not the register took them.
    #[serde(default)]
    pub unrecorded: Vec<String>,
    /// The defects whose repair record by this install the register refused,
    /// recorded again on every later pass until it takes them.
    #[serde(default)]
    pub unrepaired: Vec<String>,
}

/// A checkout's push of `main` after every step installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pushed {
    pub revision: String,
    pub at: DateTime<Utc>,
    /// What `git push` said when the push gates refused it.
    #[serde(default)]
    pub refusal: Option<String>,
}

/// What the last pass did, in its own words.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pass {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub said: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub running: Option<Running>,
    #[serde(default)]
    pub refused: Option<Refused>,
    /// By step (`PRODUCT:SURFACE`).
    #[serde(default)]
    pub delivered: BTreeMap<String, Delivered>,
    /// By checkout.
    #[serde(default)]
    pub pushed: BTreeMap<String, Pushed>,
    #[serde(default)]
    pub last_pass: Option<Pass>,
}

/// The state's path: `STADO_DELIVERY_STATE`, else
/// `~/.stado/delivery-state.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    document::located(STATE_ENV, STATE_FILE, STATE).map_err(CmdError::click)
}

/// The state as the last pass left it; none written yet is the empty state,
/// one that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<State, CmdError> {
    match document::read(&path()?, STATE).map_err(CmdError::click)? {
        Some(state) => Ok(state),
        None => Ok(State::default()),
    }
}

pub fn write(state: &State) -> Result<(), CmdError> {
    document::write(&path()?, state, STATE).map_err(CmdError::click)
}

/// The lock one pass holds for its whole run, beside the state: `None` when
/// another pass holds it. The holder writes its pid and start beside the
/// lock, so a reader that finds it held can name who holds it.
pub fn lock() -> Result<Option<std::fs::File>, CmdError> {
    let (path, file) = lock_file()?;
    match file.try_lock() {
        Ok(()) => {
            let holder = format!(
                "pid {} since {}",
                std::process::id(),
                Utc::now().to_rfc3339()
            );
            let named = path.with_extension("holder");
            std::fs::write(&named, holder).map_err(|error| {
                CmdError::click(format!(
                    "recording the holder in {}: {error}",
                    named.display()
                ))
            })?;
            Ok(Some(file))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "locking {}: {error}",
            path.display()
        ))),
    }
}

/// Who holds the pass lock now, as the holder recorded itself; `None` when
/// no pass holds it.
pub fn lock_holder() -> Result<Option<String>, CmdError> {
    let (path, file) = lock_file()?;
    match file.try_lock_shared() {
        Ok(()) => Ok(None),
        Err(std::fs::TryLockError::WouldBlock) => {
            let named = path.with_extension("holder");
            let recorded = std::fs::read_to_string(&named).map_err(|error| {
                CmdError::click(format!(
                    "reading the holder in {}: {error}",
                    named.display()
                ))
            })?;
            Ok(Some(recorded.trim().to_string()))
        }
        Err(error) => Err(CmdError::click(format!(
            "locking {}: {error}",
            path.display()
        ))),
    }
}

fn lock_file() -> Result<(PathBuf, std::fs::File), CmdError> {
    let path = path()?.with_extension("lock");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CmdError::click(format!("creating {}: {error}", parent.display())))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| CmdError::click(format!("opening {}: {error}", path.display())))?;
    Ok((path, file))
}
