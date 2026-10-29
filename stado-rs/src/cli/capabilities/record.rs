//! What `stado capabilities resolve` keeps between calls: when each
//! alternative was last proven and what it answered, and which alternative
//! served which consumer.
//!
//! The proof is kept so a family whose first alternative is down costs one
//! proof per declared interval, not one per request: within the interval
//! the kept answer stands, and a caller that was refused by the alternative
//! it was handed says so (`--failed`) to have it proven again at once. The
//! served record is kept so a fallback in use, and the outage of the
//! alternative it stands in for, is read in `stado capabilities` instead of
//! being silent.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

/// The variable that moves the record, else `~/.stado/capabilities.json`.
const RECORD_VARIABLE: &str = "STADO_CAPABILITIES_RECORD";
const RECORD_FILE: &str = "capabilities.json";
const WHAT: &str = "capability resolution record";

/// One alternative the caller could not use: its product, the route it was
/// tried at and why it did not answer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Unreachable {
    pub(crate) product: String,
    pub(crate) route: String,
    pub(crate) cause: String,
}

/// The last proof of one alternative.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub(crate) answered: bool,
    /// RFC 3339 UTC time the proof was made.
    pub(crate) at: String,
    pub(crate) route: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cause: Option<String>,
    /// The consumer that reported this alternative refused it, when the
    /// proof was asked for by `--failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reported_by: Option<String>,
}

/// What one consumer was last handed for one family.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Served {
    /// The alternative that served it; `None` when none answered.
    pub(crate) provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) route: Option<String>,
    pub(crate) at: String,
    /// Served by an alternative after the first in the selected order.
    pub(crate) fallback: bool,
    /// Every alternative before it that did not answer.
    #[serde(default)]
    pub(crate) skipped: Vec<Unreachable>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct FamilyRecord {
    #[serde(default)]
    pub(crate) proven: BTreeMap<String, Proof>,
    #[serde(default)]
    pub(crate) served: BTreeMap<String, Served>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Record {
    #[serde(default)]
    pub(crate) families: BTreeMap<String, FamilyRecord>,
}

fn path() -> Result<PathBuf, CmdError> {
    crate::primitives::document::located(RECORD_VARIABLE, RECORD_FILE, WHAT)
        .map_err(CmdError::declaration)
}

/// The record as it stands; an empty one when none was written.
pub(crate) fn read() -> Result<Record, CmdError> {
    let path = path()?;
    crate::primitives::document::read::<Record>(&path, WHAT)
        .map(Option::unwrap_or_default)
        .map_err(CmdError::declaration)
}

/// Change the record under its lock: read what stands now, apply `change`,
/// write it whole. Two resolves writing at once both land, each on the
/// other's result.
pub(crate) fn update(change: impl FnOnce(&mut Record)) -> Result<(), CmdError> {
    let path = path()?;
    let lock_path = path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::declaration(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            CmdError::declaration(format!("cannot open {}: {error}", lock_path.display()))
        })?;
    crate::wait::blocking_on_lock(format!("the {WHAT}'s lock"), &lock_path, || lock.lock())
        .map_err(|cycle| CmdError::refused(cycle.to_string()))?
        .map_err(|error| {
            CmdError::unreachable(format!("cannot lock {}: {error}", lock_path.display()))
        })?;
    let mut record = crate::primitives::document::read::<Record>(&path, WHAT)
        .map(Option::unwrap_or_default)
        .map_err(CmdError::declaration)?;
    change(&mut record);
    crate::primitives::document::write(&path, &record, WHAT).map_err(CmdError::declaration)
}
