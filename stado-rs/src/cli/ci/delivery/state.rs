//! Where the delivery routine stands: the step it is installing and since
//! when, the step that refused with every refusal of that run and the defects
//! recorded for it, and the revision each step last delivered. One file, read
//! by `stado ci status` and Stado Desktop, rewritten whole by each pass.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

const STATE_ENV: &str = "STADO_DELIVERY_STATE";
const STATE_FILE: &str = "delivery-state.json";

/// `$<env>` when set, else `~/.stado/<file>`, refused by name when neither
/// can be read.
pub(super) fn home_file(env: &str, file: &str) -> Result<PathBuf, CmdError> {
    if let Some(explicit) = std::env::var_os(env).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{file} has no place: neither {env} nor HOME is set"
            ))
        })?;
    Ok(PathBuf::from(home).join(".stado").join(file))
}

/// Write `value` to `path` whole: staged beside it, then renamed over it.
pub(super) fn write_json(path: &PathBuf, value: &impl Serialize) -> Result<(), CmdError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CmdError::click(format!("creating {}: {error}", parent.display())))?;
    }
    let staged = path.with_extension("json.writing");
    std::fs::write(&staged, serde_json::to_vec_pretty(value)?)
        .map_err(|error| CmdError::click(format!("writing {}: {error}", staged.display())))?;
    std::fs::rename(&staged, path)
        .map_err(|error| CmdError::click(format!("writing {}: {error}", path.display())))
}

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
}

/// The revision a step last installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delivered {
    pub revision: String,
    pub at: DateTime<Utc>,
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
    home_file(STATE_ENV, STATE_FILE)
}

/// The state as the last pass left it; none written yet is the empty state,
/// one that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<State, CmdError> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
        Err(error) => {
            return Err(CmdError::click(format!(
                "reading the delivery state {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!(
            "the delivery state {} is not a delivery state: {error}",
            path.display()
        ))
    })
}

pub fn write(state: &State) -> Result<(), CmdError> {
    write_json(&path()?, state)
}

/// The lock one pass holds for its whole run, beside the state: `None` when
/// another pass holds it.
pub fn lock() -> Result<Option<std::fs::File>, CmdError> {
    use fs2::FileExt;
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
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "locking {}: {error}",
            path.display()
        ))),
    }
}
