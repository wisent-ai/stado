//! Who holds the cleanup lock in shared mode, and the janitor's request for
//! its turn.
//!
//! A shared `flock` names no owner, so a janitor that found the lock held by
//! running workloads could only say "held with no holder record" and count no
//! job, and while workloads kept arriving it never got the exclusive hold at
//! all. Each shared hold therefore writes one record beside the lock, removed
//! when the hold ends, and a janitor below its low watermark writes a turn
//! request that stops new workloads from taking a hold until it has run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::primitives::constants::CLEANUP_TURN_TTL_S;
use crate::providers::local::disk_cleanup::janitor::pass::lock::takeover::pid_alive;
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

/// Every recorded hold whose process is still alive. A record whose process
/// is gone is removed: its flock went with the process.
pub(crate) fn live(state_dir: &Path) -> Vec<WorkloadHold> {
    let Ok(entries) = std::fs::read_dir(state_dir.join(HOLDS_DIR)) else {
        return Vec::new();
    };
    let mut holds = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(hold) = std::fs::read(&path)
            .ok()
            .and_then(|body| serde_json::from_slice::<WorkloadHold>(&body).ok())
        else {
            continue;
        };
        if pid_alive(hold.pid) {
            holds.push(hold);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    holds.sort_by(|left, right| left.acquired_at.total_cmp(&right.acquired_at));
    holds
}

/// Ask running workloads to drain so the next pass gets the exclusive hold.
pub(crate) fn request_turn(state_dir: &Path, reason: &str) -> Result<(), JanitorError> {
    let body = serde_json::json!({ "requested_at": epoch_now(), "reason": reason });
    std::fs::write(state_dir.join(TURN_NAME), body.to_string())?;
    Ok(())
}

/// Whether a turn request younger than [`CLEANUP_TURN_TTL_S`] stands.
pub(crate) fn turn_requested(state_dir: &Path) -> bool {
    std::fs::read(state_dir.join(TURN_NAME))
        .ok()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .and_then(|turn| turn.get("requested_at").and_then(serde_json::Value::as_f64))
        .is_some_and(|at| epoch_now() - at < CLEANUP_TURN_TTL_S as f64)
}

/// The janitor has its exclusive hold: the request is answered.
pub(crate) fn clear_turn(state_dir: &Path) {
    let _ = std::fs::remove_file(state_dir.join(TURN_NAME));
}
