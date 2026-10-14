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

/// Operator override for the register's location, absolute.
const REGISTER_ENV: &str = "STADO_RELEASE_LOOP_REGISTER";
const REGISTER_FILE: &str = "release-loop.json";

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
    if let Some(explicit) = std::env::var_os(REGISTER_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "the delivery loop register has no place: neither {REGISTER_ENV} nor HOME is set"
            ))
        })?;
    Ok(PathBuf::from(home).join(".stado").join(REGISTER_FILE))
}

/// Every entry, newest first. A register not written yet holds none; one
/// that cannot be read or parsed is a refusal naming it.
pub fn read() -> Result<Document, CmdError> {
    let path = path()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Document::default())
        }
        Err(error) => {
            return Err(CmdError::click(format!(
                "reading the delivery loop register {}: {error}",
                path.display()
            )))
        }
    };
    let mut document: Document = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!(
            "the delivery loop register {} is not a register: {error}",
            path.display()
        ))
    })?;
    document
        .entries
        .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(document)
}

/// Write `entry` in place of the one for the same product and commit.
pub fn keep(entry: &Entry) -> Result<(), CmdError> {
    let path = path()?;
    let mut document = read()?;
    document
        .entries
        .retain(|kept| !(kept.product == entry.product && kept.commit == entry.commit));
    document.entries.push(entry.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CmdError::click(format!("creating {}: {error}", parent.display())))?;
    }
    let staged = path.with_extension("json.writing");
    std::fs::write(&staged, serde_json::to_vec_pretty(&document)?)
        .map_err(|error| CmdError::click(format!("writing {}: {error}", staged.display())))?;
    std::fs::rename(&staged, &path)
        .map_err(|error| CmdError::click(format!("writing {}: {error}", path.display())))
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
