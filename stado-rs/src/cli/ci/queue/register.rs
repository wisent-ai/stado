//! What the delivery queue decided for every candidate it gated: the
//! register `stado ci status` prints and the pre-push check reads before it
//! lets a commit onto `main`.
//!
//! One entry per checkout and candidate commit, rewritten as the candidate
//! moves from `running` to `admitted` or `refused`. A refused entry keeps
//! every refusal of its run and the defect the queue recorded for it; an
//! admitted entry is the only thing that lets a push name `main`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

/// Operator override for the register's location, absolute.
const REGISTER_ENV: &str = "STADO_DELIVERY_QUEUE_REGISTER";
const REGISTER_FILE: &str = "delivery-queue.json";

/// Where one candidate stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The builder is running the installation gates over it.
    Running,
    /// Every gate passed and `main` was moved to it.
    Admitted,
    /// A gate refused, or the candidate does not descend from `main`;
    /// `main` stands where it was.
    Refused,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Admitted => "admitted",
            Self::Refused => "refused",
        }
    }
}

/// One candidate of one checkout, as the queue last left it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub product: String,
    /// The checkout the builder gated it in.
    pub checkout: PathBuf,
    /// The pushed tip of the delivery branch.
    pub candidate: String,
    /// Where `main` stood when the candidate was taken.
    pub main: String,
    pub state: State,
    /// Every refusal of the run, each as its gate said it.
    #[serde(default)]
    pub refusals: Vec<String>,
    /// The defect recorded in Oko for a refused candidate.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub defect: Option<String>,
    /// The sessions whose recorded repairs ride on the candidate.
    #[serde(default)]
    pub authors: Vec<String>,
    /// For a refused candidate: the admitted candidate whose run passed
    /// after it, recorded on its defect as the repair.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repaired_by: Option<String>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

/// The register's path: `STADO_DELIVERY_QUEUE_REGISTER`, else
/// `~/.stado/delivery-queue.json`.
pub fn path() -> Result<PathBuf, CmdError> {
    if let Some(explicit) = std::env::var_os(REGISTER_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "the delivery queue register has no place: neither {REGISTER_ENV} nor HOME is set"
            ))
        })?;
    Ok(PathBuf::from(home).join(".stado").join(REGISTER_FILE))
}

/// The lock the one serialized builder holds for a whole pass, beside the
/// register: two passes never gate or move `main` together.
pub fn lock_path() -> Result<PathBuf, CmdError> {
    Ok(path()?.with_extension("lock"))
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
                "reading the delivery queue register {}: {error}",
                path.display()
            )))
        }
    };
    let mut document: Document = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::click(format!(
            "the delivery queue register {} is not a register: {error}",
            path.display()
        ))
    })?;
    document
        .entries
        .sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
    Ok(document)
}

/// Write `entry` in place of the one for the same checkout and candidate.
pub fn keep(entry: &Entry) -> Result<(), CmdError> {
    let path = path()?;
    let mut document = read()?;
    document
        .entries
        .retain(|kept| !(kept.checkout == entry.checkout && kept.candidate == entry.candidate));
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

/// The entry the queue kept for `candidate` of `checkout`.
pub fn kept(checkout: &Path, candidate: &str) -> Result<Option<Entry>, CmdError> {
    Ok(read()?
        .entries
        .into_iter()
        .find(|entry| entry.checkout == checkout && entry.candidate == candidate))
}

/// Whether the queue admitted `commit`: its run passed every installation
/// gate, so a push may move `main` to it.
pub fn admitted(commit: &str) -> Result<bool, CmdError> {
    Ok(read()?
        .entries
        .iter()
        .any(|entry| entry.candidate == commit && entry.state == State::Admitted))
}

/// The entries of `product`, newest first.
pub fn of_product(product: &str) -> Result<Vec<Entry>, CmdError> {
    Ok(read()?
        .entries
        .into_iter()
        .filter(|entry| entry.product == product)
        .collect())
}

/// One line per entry, as `stado ci status` and `stado ci deliver` print it.
pub fn line(entry: &Entry) -> String {
    let mut line = format!(
        "{} {} over main {} {}",
        entry.product,
        entry.candidate,
        entry.main,
        entry.state.name()
    );
    if let Some(defect) = &entry.defect {
        line.push_str(&format!(" defect {defect}"));
    }
    if !entry.authors.is_empty() {
        line.push_str(&format!(" authors {}", entry.authors.join(",")));
    }
    if let Some(repaired) = &entry.repaired_by {
        line.push_str(&format!(" repaired by {repaired}"));
    }
    for refusal in &entry.refusals {
        line.push_str(&format!("\n    refused: {refusal}"));
    }
    line
}
