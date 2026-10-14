//! Who holds the cleanup lock in shared mode, and the janitor's request for
//! its turn.
//!
//! A shared `flock` names no owner, so a janitor that found the lock held by
//! running workloads could only say "held with no holder record" and count no
//! job, and while workloads kept arriving it never got the exclusive hold at
//! all. Each shared hold therefore writes one record beside the lock, removed
//! when the hold ends, and a janitor below its low watermark writes a turn
//! request that stops new workloads from taking a hold until it has run. The
//! request names the janitor's process and stands exactly while that process
//! does: a janitor that exits without getting its turn leaves no request
//! behind to keep the host from work.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::providers::local::disk_cleanup::janitor::pass::lock::ownership::pid_alive;
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::report::build::epoch_now;

/// Directory, beside the lock, holding one record per live shared hold.
pub(crate) const HOLDS_DIR: &str = "disk-cleanup-holds";
/// The janitor's request for its turn.
pub(crate) const TURN_NAME: &str = "disk-cleanup-turn.json";

/// One shared hold: the process, the work it stands for, and since when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadHold {
    pub pid: i32,
    pub holder: String,
    pub acquired_at: f64,
}

/// Write the record of a hold this process just took; the path is removed
/// when the hold ends.
pub(crate) fn record(state_dir: &Path, holder: &str) -> Result<PathBuf, JanitorError> {
    let dir = state_dir.join(HOLDS_DIR);
    std::fs::create_dir_all(&dir)?;
    let pid = std::process::id() as i32;
    let path = dir.join(format!("{pid}-{}.json", uuid::Uuid::new_v4()));
    let hold = WorkloadHold {
        pid,
        holder: holder.to_string(),
        acquired_at: epoch_now(),
    };
    let body = serde_json::to_vec(&hold).map_err(|exc| JanitorError::os(&exc.to_string()))?;
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Every recorded hold whose process is still alive, and what could not be
/// read. A record whose process is gone is removed: its flock went with the
/// process. No holds directory is no holds; a directory that cannot be listed
/// or a record that cannot be read or parsed is named, so a lock held by
/// workloads is not reported as held by nobody.
pub(crate) fn live(state_dir: &Path) -> (Vec<WorkloadHold>, Vec<String>) {
    let dir = state_dir.join(HOLDS_DIR);
    let mut holds = Vec::new();
    let mut unreadable = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (holds, unreadable),
        Err(error) => {
            unreadable.push(format!("{}: {error}", dir.display()));
            return (holds, unreadable);
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                unreadable.push(format!(
                    "cannot enumerate a hold in {}: {error}",
                    dir.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let hold = match std::fs::read(&path) {
            Ok(body) => match serde_json::from_slice::<WorkloadHold>(&body) {
                Ok(hold) => hold,
                Err(error) => {
                    unreadable.push(format!("{}: not a hold record: {error}", path.display()));
                    continue;
                }
            },
            Err(error) => {
                unreadable.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if pid_alive(hold.pid) {
            holds.push(hold);
        } else if let Some(left) = crate::primitives::staging::remove(&path) {
            unreadable.push(format!("the hold of a workload that is gone stays: {left}"));
        }
    }
    holds.sort_by(|left, right| left.acquired_at.total_cmp(&right.acquired_at));
    (holds, unreadable)
}

/// Ask running workloads to drain so the next pass gets the exclusive hold.
pub(crate) fn request_turn(state_dir: &Path, reason: &str) -> Result<(), JanitorError> {
    let body = serde_json::json!({
        "requested_at": epoch_now(),
        "reason": reason,
        "pid": std::process::id(),
    });
    std::fs::write(state_dir.join(TURN_NAME), body.to_string())?;
    Ok(())
}

/// The janitor's request for its turn as it was written: when, why, and by
/// which process (none when an older Stado wrote it without one).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRequest {
    pub requested_at: f64,
    pub reason: String,
    #[serde(default)]
    pub pid: Option<i32>,
}

/// The turn request that stands: one whose janitor process is still alive.
/// A request without a process (written by an older Stado) or from a process
/// that is gone stands for nothing and is removed. A request that exists and
/// cannot be read is an error naming it: read as none, workloads kept taking
/// holds while the janitor waited for its turn.
pub(crate) fn turn_request(state_dir: &Path) -> Result<Option<TurnRequest>, JanitorError> {
    let path = state_dir.join(TURN_NAME);
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(JanitorError::os(&format!(
                "cannot read the janitor turn request {}: {error}",
                path.display()
            )))
        }
    };
    let request = serde_json::from_slice::<TurnRequest>(&body)
        .ok()
        .filter(|request| request.pid.is_some_and(pid_alive));
    if request.is_none() {
        if let Some(left) = crate::primitives::staging::remove(&path) {
            eprintln!("janitor: a turn request nobody stands behind stays: {left}");
        }
    }
    Ok(request)
}

/// Whether a turn request stands.
pub(crate) fn turn_requested(state_dir: &Path) -> Result<bool, JanitorError> {
    Ok(turn_request(state_dir)?.is_some())
}

/// The janitor has its exclusive hold: the request is answered. A request
/// that stays keeps workloads yielding to a janitor already done, so that is
/// said with its path and cause.
pub(crate) fn clear_turn(state_dir: &Path) {
    if let Some(left) = crate::primitives::staging::remove(&state_dir.join(TURN_NAME)) {
        eprintln!("janitor: the answered turn request stays: {left}");
    }
}
