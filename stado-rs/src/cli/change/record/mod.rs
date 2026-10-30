//! A candidate change of one repository, as the submit queue keeps it: one
//! JSON document per candidate under the queue's directory
//! (`STADO_CHANGE_QUEUE`, else `~/.stado/change-queue`), at
//! `<owner>/<name>/<id>.json`. The id begins with the submission time, so
//! the names of one repository sort oldest first, which is the queue's
//! order, save that a candidate `stado change prioritize` moved ahead goes
//! before every other, in the order they were moved ahead; why is kept
//! beside it, `<owner>/<name>/<id>.priority`, and read into its `priority`
//! whenever it is read. A repository's pass holds `<owner>/<name>.lock` for
//! its whole run, so one repository's candidates are rebased, checked and
//! landed one at a time; the kernel releases that lock when the pass's
//! process ends however it ends, so a running candidate whose repository's
//! lock is free is one a pass that is gone left behind, and the next pass
//! resumes it. `stado change cancel` asks for a candidate's withdrawal with
//! a file of its own, `<owner>/<name>/<id>.cancel`, which the pass reads
//! before each step: a running candidate's record has one writer, its pass.

mod model;

pub use model::*;

use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::cli::CmdError;
use crate::primitives::document;

const DIRECTORY_ENV: &str = "STADO_CHANGE_QUEUE";
const DIRECTORY: &str = "change-queue";
const WHAT: &str = "submit queue candidate";

/// The queue's directory.
pub fn root() -> Result<PathBuf, CmdError> {
    document::located(DIRECTORY_ENV, DIRECTORY, "submit queue").map_err(CmdError::declaration)
}

/// `<root>/<owner>/<name>/<id>.json`.
fn path_of(candidate: &Candidate) -> Result<PathBuf, CmdError> {
    Ok(root()?
        .join(&candidate.repository)
        .join(format!("{}.json", candidate.id)))
}

/// `<root>/<owner>/<name>/<id>.cancel`, beside the candidate's record.
fn cancel_path(candidate: &Candidate) -> Result<PathBuf, CmdError> {
    Ok(root()?
        .join(&candidate.repository)
        .join(format!("{}.cancel", candidate.id)))
}

/// Ask the pass carrying `candidate` to withdraw it before its next step.
pub fn request_cancel(candidate: &Candidate) -> Result<(), CmdError> {
    let path = cancel_path(candidate)?;
    std::fs::write(&path, b"")
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", path.display())))
}

/// Whether `stado change cancel` asked for `candidate`'s withdrawal.
pub fn cancel_requested(candidate: &Candidate) -> Result<bool, CmdError> {
    let path = cancel_path(candidate)?;
    path.try_exists()
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", path.display())))
}

/// `<root>/<owner>/<name>/<id>.priority`, beside the candidate's record.
fn priority_path(candidate: &Candidate) -> Result<PathBuf, CmdError> {
    Ok(root()?
        .join(&candidate.repository)
        .join(format!("{}.priority", candidate.id)))
}

/// Move `candidate` ahead of the queue's order for `priority`'s reason. Its
/// own file, never the record, so a pass writing the record meanwhile does
/// not write over it.
pub fn save_priority(candidate: &Candidate, priority: &Priority) -> Result<(), CmdError> {
    document::write(
        &priority_path(candidate)?,
        priority,
        "submit queue priority",
    )
    .map_err(CmdError::click)
}

/// `<root>/<owner>/<name>/<id>.rounds/`, beside the candidate's record: the
/// review rounds recorded over it, one document each, written by `stado
/// change review` and never by the pass, so a round can be recorded while a
/// pass carries the candidate.
fn rounds_path(candidate: &Candidate) -> Result<PathBuf, CmdError> {
    Ok(root()?
        .join(&candidate.repository)
        .join(format!("{}.rounds", candidate.id)))
}

/// Keep `round` beside `candidate`; its path. A reviewer session that is
/// not a plain id (letters, digits, `.`, `_`, `-`, not starting with `.`)
/// is refused, so no round is written outside `<id>.rounds/`.
pub fn save_round(candidate: &Candidate, round: &Round) -> Result<PathBuf, CmdError> {
    let session = &round.reviewer_session;
    if session.is_empty()
        || session.starts_with('.')
        || !session
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(CmdError::refused(format!(
            "reviewer session {session:?} is not a session id (letters, digits, '.', '_' and \
             '-', not starting with '.'), so no round is kept under it"
        )));
    }
    let directory = rounds_path(candidate)?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        CmdError::click(format!("cannot create {}: {error}", directory.display()))
    })?;
    let path = directory.join(format!(
        "{}-{session}.json",
        round.at.format("%Y%m%dT%H%M%S%6f"),
    ));
    document::write(&path, round, "review round").map_err(CmdError::click)?;
    Ok(path)
}

/// Every round recorded over `candidate`, oldest first.
pub fn rounds(candidate: &Candidate) -> Result<Vec<Round>, CmdError> {
    let mut rounds = Vec::new();
    for path in entries(&rounds_path(candidate)?)? {
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            rounds.push(
                document::read(&path, "review round")
                    .map_err(CmdError::click)?
                    .ok_or_else(|| CmdError::missing(format!("{} is gone", path.display())))?,
            );
        }
    }
    Ok(rounds)
}

/// Write `candidate` whole, stamping when.
pub fn save(candidate: &mut Candidate) -> Result<(), CmdError> {
    candidate.updated_at = Utc::now();
    let path = path_of(candidate)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    document::write(&path, candidate, WHAT).map_err(CmdError::click)
}

fn read(path: &Path) -> Result<Candidate, CmdError> {
    let mut candidate: Candidate = document::read(path, WHAT)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::missing(format!("{} is gone", path.display())))?;
    candidate.priority = document::read(&priority_path(&candidate)?, "submit queue priority")
        .map_err(CmdError::click)?;
    Ok(candidate)
}

/// Every candidate of `repository` in the queue's order: those moved ahead
/// first, in the order they were moved, then the rest oldest first; every
/// repository's when none is named.
pub fn list(repository: Option<&str>) -> Result<Vec<Candidate>, CmdError> {
    let root = root()?;
    let mut directories = Vec::new();
    match repository {
        Some(repository) => directories.push(root.join(repository)),
        None => {
            for owner in entries(&root)? {
                if owner.is_dir() {
                    directories.extend(entries(&owner)?.into_iter().filter(|path| path.is_dir()));
                }
            }
        }
    }
    let mut candidates = Vec::new();
    for directory in directories {
        for path in entries(&directory)? {
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                candidates.push(read(&path)?);
            }
        }
    }
    candidates.sort_by(|left, right| {
        let ahead = |candidate: &Candidate| {
            candidate
                .priority
                .as_ref()
                .map_or((1, None), |priority| (0, Some(priority.at)))
        };
        ahead(left)
            .cmp(&ahead(right))
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(candidates)
}

/// The names in `directory`, sorted; none when it does not exist yet.
fn entries(directory: &Path) -> Result<Vec<PathBuf>, CmdError> {
    let refused = |error: std::io::Error| {
        CmdError::click(format!("cannot list {}: {error}", directory.display()))
    };
    let read = match std::fs::read_dir(directory) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(refused(error)),
    };
    let mut paths = Vec::new();
    for entry in read {
        paths.push(entry.map_err(refused)?.path());
    }
    paths.sort();
    Ok(paths)
}

/// The candidate `id` names, whole or by its unique prefix.
pub fn find(id: &str) -> Result<Candidate, CmdError> {
    let all = list(None)?;
    let matching: Vec<&Candidate> = all
        .iter()
        .filter(|candidate| candidate.id.starts_with(id))
        .collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CmdError::missing(format!(
            "no candidate of the submit queue at {} has the id {id}; `stado change list` names \
             every candidate",
            root()?.display()
        ))),
        many => Err(CmdError::usage(format!(
            "{id} names {} candidates ({}); give more of the id",
            many.len(),
            many.iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The lock one pass of `repository` holds for its whole run: `None` while
/// another pass holds it.
pub fn lock(repository: &str) -> Result<Option<std::fs::File>, CmdError> {
    let path = root()?.join(format!("{repository}.lock"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| CmdError::click(format!("cannot open {}: {error}", path.display())))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "cannot lock {}: {error}",
            path.display()
        ))),
    }
}

/// This process, as the pass that carries a candidate.
pub fn this_pass() -> Pass {
    Pass {
        pid: std::process::id(),
        started_at: Utc::now(),
    }
}

/// Whether a pass of `repository` runs now on this machine: its lock is
/// held. The kernel releases the lock of a process that ends however it
/// ends, so a free lock is a pass that is gone, killed or not.
pub fn pass_running(repository: &str) -> Result<bool, CmdError> {
    Ok(lock(repository)?.is_none())
}

/// Whether `candidate` waits for a pass: queued, or running with no pass
/// holding its repository's lock (one that was killed, or whose machine
/// restarted). A lock that cannot be read is taken as held, so no second
/// pass is started over it.
pub fn waits_for_pass(candidate: &Candidate) -> bool {
    match candidate.state {
        State::Queued => true,
        State::Running => matches!(pass_running(&candidate.repository), Ok(false)),
        State::Landed | State::Refused | State::Repaired | State::Cancelled => false,
    }
}
