//! What the main check found, commit by commit: for every step of the
//! delivery plan, each commit of its checkout's `main` it checked, oldest
//! first, green or red with every refusal and the Oko defects recorded for
//! it. One file, read by `stado ci main status|history` and Stado Desktop,
//! rewritten whole after each commit is checked.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::ci::delivery::state::{home_file, write_json};
use crate::cli::CmdError;

const RECORD_ENV: &str = "STADO_CI_MAIN_RECORD";
const RECORD_FILE: &str = "ci-main.json";

/// One commit of `main` the check ran over.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checked {
    pub revision: String,
    pub at: DateTime<Utc>,
    /// Every declared gate passed over that commit's tree.
    pub green: bool,
    /// Every refusal of the check, each gate run to its end, for a red one.
    #[serde(default)]
    pub refusals: Vec<String>,
    /// The Oko defects recorded for a red commit, one per authoring session.
    #[serde(default)]
    pub defects: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Record {
    /// By step (`PRODUCT:SURFACE`), oldest first.
    #[serde(default)]
    pub steps: BTreeMap<String, Vec<Checked>>,
}

impl Record {
    /// The newest green commit of `step`: the one the delivery routine
    /// installs, never `main` HEAD.
    pub fn last_green(&self, step: &str) -> Option<&Checked> {
        self.steps
            .get(step)?
            .iter()
            .rev()
            .find(|checked| checked.green)
    }

    /// The newest commit of `step` the check ran over.
    pub fn last_checked(&self, step: &str) -> Option<&Checked> {
        self.steps.get(step)?.last()
    }
}

/// The record's path: `STADO_CI_MAIN_RECORD`, else `~/.stado/ci-main.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    home_file(RECORD_ENV, RECORD_FILE)
}

/// The record as the last check left it; none written yet is the empty
/// record, one that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<Record, CmdError> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Record::default()),
        Err(error) => {
            return Err(CmdError::click(format!(
                "reading the main check record {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!(
            "the main check record {} is not a main check record: {error}",
            path.display()
        ))
    })
}

pub fn write(record: &Record) -> Result<(), CmdError> {
    write_json(&path()?, record)
}
