//! What the main check found, commit by commit: for every step of the
//! delivery plan, each commit of its checkout's `main` it checked, oldest
//! first, green or red with every refusal and the Oko defects recorded for
//! it. One file, read by `stado ci status`, `stado ci delivery`, the product installer's verb prerequisites and Stado Desktop,
//! rewritten whole after each commit is checked.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;
use crate::primitives::document;

const RECORD_ENV: &str = "STADO_CI_MAIN_RECORD";
const RECORD_FILE: &str = "ci-main.json";
const RECORD: &str = "main check record";

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
    document::located(RECORD_ENV, RECORD_FILE, RECORD).map_err(CmdError::click)
}

/// The record as the last check left it; none written yet is the empty
/// record, one that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<Record, CmdError> {
    match document::read(&path()?, RECORD).map_err(CmdError::click)? {
        Some(record) => Ok(record),
        None => Ok(Record::default()),
    }
}

pub fn write(record: &Record) -> Result<(), CmdError> {
    document::write(&path()?, record, RECORD).map_err(CmdError::click)
}
