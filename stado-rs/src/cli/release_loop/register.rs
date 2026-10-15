//! What the delivery queue holds for each commit it read: the register
//! `stado release newest` prints beside its plan and its outcome.
//!
//! One entry per product and commit, rewritten as the commit moves through
//! the queue, so the newest entry of a product is where its delivery stands
//! and the earlier ones say what became of the commits before it. A failed
//! step keeps its exact error on the commit that failed it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;
use crate::primitives::document;

/// Operator override for the register's location, absolute.
const REGISTER_ENV: &str = "STADO_RELEASE_LOOP_REGISTER";
const REGISTER_FILE: &str = "release-loop.json";
const REGISTER: &str = "delivery loop register";

/// Where one commit stands in the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Being delivered; the step it is on is named.
    Running,
    /// Carries repairs and waits for the build gate: `step` is the first
    /// step `stado release newest` runs for it once the gate admits it.
    Queued,
    /// Built, tested, published and installed; the carried defects are
    /// recorded as released in Oko.
    Delivered,
    /// A step refused; `error` is what it said.
    Failed,
    /// No open defect records a repair this commit carries, and no release
    /// is owed for it.
    NothingCarried,
}

/// The steps a commit goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Reading Oko for the open defects whose repair the commit carries.
    Carried,
    /// The product's format gates and the lock check over the committed
    /// tree: no compilation, so they run before the build gate opens.
    Gates,
    /// Committing the least next version the versioning rule allows.
    Version,
    /// `stado release submit`: build, the product's quality steps, signing,
    /// publication and delivery to the hosts that run the product.
    Release,
    /// Recording the release on every carried defect in Oko.
    Record,
}

impl Step {
    /// The step's name as the register spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Carried => "carried",
            Self::Gates => "gates",
            Self::Version => "version",
            Self::Release => "release",
            Self::Record => "record",
        }
    }
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::NothingCarried => "nothing_carried",
        }
    }
}

/// One commit of one product, as the loop last left it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub product: String,
    pub commit: String,
    pub state: State,
    pub step: Step,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<String>,
    /// The version commit the loop wrote, when the declared one was taken.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version_commit: Option<String>,
    /// The open defects whose recorded repair this commit carries.
    #[serde(default)]
    pub carried: Vec<String>,
    /// The defects Oko closed against the published release.
    #[serde(default)]
    pub recorded: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error: Option<String>,
    /// When this pass's release step published the release; none while it
    /// has not, and on an entry the release step of an earlier pass took.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub released_at: Option<DateTime<Utc>>,
    /// When Oko recorded the published release on the carried defects.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub recorded_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

/// The register's path: `STADO_RELEASE_LOOP_REGISTER`, else
/// `~/.stado/release-loop.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    document::located(REGISTER_ENV, REGISTER_FILE, REGISTER).map_err(CmdError::click)
}

/// Every entry, newest first. A register not written yet holds none; one
/// that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<Document, CmdError> {
    let mut register = match document::read(&path()?, REGISTER).map_err(CmdError::click)? {
        Some(register) => register,
        None => Document::default(),
    };
    register
        .entries
        .sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    Ok(register)
}

/// Write `entry` in place of the one for the same product and commit.
pub fn keep(entry: &Entry) -> Result<(), CmdError> {
    let mut register = read()?;
    register
        .entries
        .retain(|held| !(held.product == entry.product && held.commit == entry.commit));
    register.entries.push(entry.clone());
    document::write(&path()?, &register, REGISTER).map_err(CmdError::click)
}

/// The entry this loop already kept for `product` at `commit`.
pub fn kept(product: &str, commit: &str) -> Result<Option<Entry>, CmdError> {
    Ok(read()?
        .entries
        .into_iter()
        .find(|entry| entry.product == product && entry.commit == commit))
}

/// One line per entry as `stado release newest` prints the queue: product,
/// commit, state, step, version, the defects carried and closed, and a
/// failure's error.
pub fn line(entry: &Entry) -> String {
    let mut line = format!(
        "{}  {}  {} at {}",
        entry.product,
        entry.commit,
        entry.state.name(),
        entry.step.name()
    );
    if let Some(version) = &entry.version {
        line.push_str(&format!("  version {version}"));
    }
    if let Some(written) = &entry.version_commit {
        line.push_str(&format!(" (committed {written})"));
    }
    if !entry.carried.is_empty() {
        line.push_str(&format!("  carries {}", entry.carried.join(", ")));
    }
    if !entry.recorded.is_empty() {
        line.push_str(&format!("  closed {}", entry.recorded.join(", ")));
    }
    if let Some(error) = &entry.error {
        line.push_str(&format!("  error: {error}"));
    }
    line
}
