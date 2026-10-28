//! How far a placement move got, kept on the host running it, so a move whose
//! process ended without committing or rolling back is undone exactly as far
//! as it went.
//!
//! [`Progress`] lived only in the move's process. When that process ended —
//! killed by its caller, its terminal closed, the host restarted —
//! `stado placement release` could not know which steps ran and counted every
//! one as taken: it retired destination units that never started, restored
//! state paths that were never written and started source units that were
//! never fenced. Each undo ends harmlessly on a step that never ran, but every
//! one of them is a remote command against hosts that may be the very ones
//! not answering, and a release that stands on them stands for nothing.
//!
//! The move now writes its progress to
//! `~/.stado/placement/<transaction>.json` before each step it takes, the
//! same moment the in-memory progress changes, and removes it when the move
//! committed or rolled back. A release on the same host reads it and undoes
//! only those steps; a release on another host, or one that finds no record,
//! still counts every step as taken and says why.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Progress;
use crate::placement::PlacementProfile;

/// The record of one move's progress, as the move's process last kept it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    transaction_id: String,
    /// The process running the move, so a reader can tell a move still at
    /// work from one whose process ended without committing.
    pid: u32,
    source_stopped: bool,
    /// The state paths the move began writing on the destination, in the
    /// order it began them.
    destination_written: Vec<String>,
    route_applied: bool,
    destination_started: bool,
    source_retired: bool,
    kept_at: DateTime<Utc>,
}

/// The directory this host keeps move records in, under the home every Stado
/// file is kept under ([`crate::config_file::home`]: HOME, else the user
/// database), or why there is no such place.
fn directory() -> Result<PathBuf, String> {
    let home = crate::config_file::home()
        .map_err(|cause| format!("this host has no place for placement move records: {cause}"))?;
    Ok(home.join(".stado").join("placement"))
}

/// Where the record of `transaction_id` lives on this host.
fn path(transaction_id: &str) -> Result<PathBuf, String> {
    Ok(directory()?.join(format!("{transaction_id}.json")))
}

/// Keep how far the move `transaction_id` got. A record that cannot be
/// written is said: a release of this move would then count every step as
/// taken, which is what it did before records were kept.
pub(in crate::cli::placement) fn keep(transaction_id: &str, progress: &Progress) {
    let journal = Journal {
        transaction_id: transaction_id.to_string(),
        pid: std::process::id(),
        source_stopped: progress.source_stopped,
        destination_written: progress
            .destination_written
            .iter()
            .map(|state| state.path.clone())
            .collect(),
        route_applied: progress.route_applied,
        destination_started: progress.destination_started,
        source_retired: progress.source_retired,
        kept_at: Utc::now(),
    };
    let written = path(transaction_id).and_then(|path| {
        crate::primitives::document::write(&path, &journal, "placement move progress")
            .map_err(|error| format!("{}: {error}", path.display()))
    });
    if let Err(cause) = written {
        eprintln!(
            "stado: the progress of placement move {transaction_id} is not kept, so a release of \
             it would undo every step: {cause}"
        );
    }
}

/// The progress the move `transaction_id` kept on this host, read back
/// against `profile`; `Ok(None)` when this host keeps no record of it. A
/// record that names a state path the profile no longer declares is refused:
/// its backup could not be told from a path the move never touched.
pub(in crate::cli::placement) fn recorded(
    transaction_id: &str,
    profile: &PlacementProfile,
) -> Result<Option<Progress>, String> {
    let path = path(transaction_id)?;
    let Some(journal) =
        crate::primitives::document::read::<Journal>(&path, "placement move progress")?
    else {
        return Ok(None);
    };
    if journal.transaction_id != transaction_id {
        return Err(format!(
            "{} records placement move {}, not {transaction_id}",
            path.display(),
            journal.transaction_id
        ));
    }
    let mut destination_written = Vec::with_capacity(journal.destination_written.len());
    for written in &journal.destination_written {
        let state = profile
            .state
            .iter()
            .find(|state| &state.path == written)
            .ok_or_else(|| {
                format!(
                    "{} records state path {written} written on the destination, which placement \
                     profile {} no longer declares",
                    path.display(),
                    profile.name
                )
            })?;
        destination_written.push(state.clone());
    }
    Ok(Some(Progress {
        source_stopped: journal.source_stopped,
        destination_written,
        route_applied: journal.route_applied,
        destination_started: journal.destination_started,
        source_retired: journal.source_retired,
        steps: Vec::new(),
    }))
}

/// The move `transaction_id` committed or rolled back: its record is spent.
/// A record that cannot be removed is said, since a later release would read
/// steps of a move that already ended.
pub(in crate::cli::placement) fn forget(transaction_id: &str) {
    let removed = path(transaction_id).and_then(|path| match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    });
    if let Err(cause) = removed {
        eprintln!(
            "stado: the progress record of placement move {transaction_id}, which ended, could not \
             be removed: {cause}"
        );
    }
}

/// What this host recorded of the move `transaction_id`, for a reader that
/// decides whether to release it: the last step the move began, when it was
/// kept, and whether the process that kept it still runs. `Ok(None)` when this
/// host keeps no record of it.
pub(in crate::cli::placement) fn summary(
    transaction_id: &str,
) -> Result<Option<serde_json::Value>, String> {
    let path = path(transaction_id)?;
    let Some(journal) =
        crate::primitives::document::read::<Journal>(&path, "placement move progress")?
    else {
        return Ok(None);
    };
    let last_step = if journal.source_retired {
        "retire the source units"
    } else if journal.destination_started {
        "start the destination units"
    } else if journal.route_applied {
        "apply routes"
    } else if !journal.destination_written.is_empty() {
        "write state on the destination"
    } else if journal.source_stopped {
        "fence the source units"
    } else {
        "preflight"
    };
    let running = i32::try_from(journal.pid).is_ok_and(crate::primitives::procutil::pid_alive);
    Ok(Some(serde_json::json!({
        "last_step": last_step,
        "state_written": journal.destination_written,
        "kept_at": journal.kept_at,
        "pid": journal.pid,
        "process_running": running,
    })))
}

/// The process on this host still running the move `transaction_id`, when
/// its record names one that runs; `Ok(None)` when this host keeps no record
/// or the recorded process has ended.
pub(in crate::cli::placement) fn running_pid(transaction_id: &str) -> Result<Option<u32>, String> {
    let path = path(transaction_id)?;
    let Some(journal) =
        crate::primitives::document::read::<Journal>(&path, "placement move progress")?
    else {
        return Ok(None);
    };
    let running = journal.pid != std::process::id()
        && i32::try_from(journal.pid).is_ok_and(crate::primitives::procutil::pid_alive);
    Ok(running.then_some(journal.pid))
}

/// Remove what this host keeps of moves the registry no longer holds open:
/// each progress record (`<id>.json`) and each tree staged for the move
/// (the `<id>` directory a directory state is copied through), answering
/// what was removed. A move that ended while they could not be removed left
/// them behind, and a later move with no open transaction would never read
/// them; kept, they only grow the directory, a staged tree by its whole
/// size. One that cannot be removed is named in the answer's error.
pub(in crate::cli::placement) fn sweep(open: &[String]) -> Result<Vec<String>, String> {
    let directory = directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", directory.display())),
    };
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let file = entry.path();
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let (id, staged) = match name.strip_suffix(".json") {
            Some(id) => (id, false),
            None if file.is_dir() => (name, true),
            None => continue,
        };
        if open.iter().any(|transaction| transaction == id) {
            continue;
        }
        // The registry read the open set came from may be one refresh
        // interval old, behind a move claimed since; a record whose move
        // process still runs belongs to such a move and is kept.
        match running_pid(id) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(cause) => {
                failed.push(format!(
                    "{}: whether its move still runs: {cause}",
                    file.display()
                ));
                continue;
            }
        }
        let (gone, what) = if staged {
            (
                std::fs::remove_dir_all(&file),
                format!("the trees staged for {id}"),
            )
        } else {
            (
                std::fs::remove_file(&file),
                format!("the progress record of {id}"),
            )
        };
        match gone {
            Ok(()) => removed.push(what),
            Err(error) => failed.push(format!("{}: {error}", file.display())),
        }
    }
    if failed.is_empty() {
        Ok(removed)
    } else {
        Err(format!(
            "progress records of ended moves could not be removed: {}",
            failed.join("; ")
        ))
    }
}
