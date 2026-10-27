//! Schedule declarations the fleet store could not take, kept on this host
//! under `~/.stado/schedules/held` until a schedule command reaches the
//! store, so a host declares its schedules while another host's store does
//! not answer.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;
use crate::models::isoformat_utc;
use crate::queue::{JobStorage, StorageError};
use crate::schedules::Schedule;

/// One declaration kept on this host: the schedule as `schedule create`
/// built it, how it was asked to be declared, and why the store did not
/// take it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Held {
    pub(super) schedule: Schedule,
    /// Declared under a caller-retained id (`--id`).
    pub(super) id_retained: bool,
    /// Declared with `--converge`.
    pub(super) converge: bool,
    /// The store's own refusal when the declaration was kept.
    pub(super) refusal: String,
    pub(super) held_at: String,
}

/// Whether the store did not answer, as against refusing: a request it
/// answered with a server error, one not sent because the resolver holds
/// its route unanswered, or one whose transport failed.
pub(super) fn unanswered(error: &StorageError) -> bool {
    match error {
        StorageError::Held(_) | StorageError::Http(_) => true,
        StorageError::Stado { status, .. } | StorageError::Gcs { status, .. } => {
            reqwest::StatusCode::from_u16(*status).is_ok_and(|status| status.is_server_error())
        }
        _ => false,
    }
}

fn directory() -> Result<PathBuf, CmdError> {
    let home = crate::config_file::home().map_err(|cause| {
        CmdError::refused(format!(
            "no schedule declaration can be held on this host: {cause}"
        ))
    })?;
    Ok(home.join(".stado").join("schedules").join("held"))
}

fn failed(what: &str, path: &Path, error: impl std::fmt::Display) -> CmdError {
    CmdError::refused(format!("{what} {}: {error}", path.display()))
}

/// Keep `schedule` on this host because the store did not answer with
/// `refusal`; answers the file it is kept in. Written whole and flushed with
/// its directory ([`crate::primitives::document::persist`]), so a reader never
/// meets half a declaration and a held one survives a power loss.
pub(super) fn hold(
    schedule: &Schedule,
    id_retained: bool,
    converge: bool,
    refusal: &StorageError,
) -> Result<PathBuf, CmdError> {
    let directory = directory()?;
    let held = Held {
        schedule: schedule.clone(),
        id_retained,
        converge,
        refusal: refusal.to_string(),
        held_at: isoformat_utc(Utc::now()),
    };
    let path = directory.join(format!("{}.json", schedule.schedule_id));
    crate::primitives::document::persist(&path, &serde_json::to_vec_pretty(&held)?)
        .map_err(|error| failed("cannot keep the held schedule as", &path, error))?;
    Ok(path)
}

/// Every declaration held on this host, by schedule id; none when nothing
/// was ever held. A file that cannot be read or is not a held declaration is
/// refused naming it, never skipped.
pub(super) fn all() -> Result<Vec<(PathBuf, Held)>, CmdError> {
    let directory = directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(failed("cannot list held schedules in", &directory, error)),
    };
    let mut held = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| failed("cannot list held schedules in", &directory, error))?
            .path();
        let visible = path
            .file_name()
            .is_some_and(|name| !name.to_string_lossy().starts_with('.'));
        if !visible {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| failed("cannot read the held schedule", &path, error))?;
        let declaration: Held = serde_json::from_str(&text)
            .map_err(|error| failed("is not a held schedule declaration:", &path, error))?;
        held.push((path, declaration));
    }
    held.sort_by(|(_, a), (_, b)| a.schedule.schedule_id.cmp(&b.schedule.schedule_id));
    Ok(held)
}

/// Write every held declaration into `store` before a schedule command
/// works on it, removing each file the store took and saying so on stderr.
/// A declaration the store refuses (another schedule under its id) stays
/// held and is said. A store error is the outer error, so a caller can tell
/// a store that does not answer from a held file it cannot read or remove,
/// which is the inner one.
pub(super) async fn deliver(store: &JobStorage) -> Result<Result<(), CmdError>, StorageError> {
    let held = match all() {
        Ok(held) => held,
        Err(error) => return Ok(Err(error)),
    };
    for (path, held) in held {
        let schedule_id = held.schedule.schedule_id.clone();
        let declared =
            super::declare::declare(store, &held.schedule, held.id_retained, held.converge).await?;
        match declared {
            Err(refusal) => eprintln!(
                "stado: held schedule {schedule_id} stays at {}: the fleet store refused it: \
                 {refusal}",
                path.display()
            ),
            Ok(outcome) => {
                if let Err(error) = std::fs::remove_file(&path) {
                    return Ok(Err(failed(
                        "cannot remove the delivered held schedule",
                        &path,
                        error,
                    )));
                }
                eprintln!(
                    "stado: held schedule {schedule_id} {}; {} removed",
                    outcome.said(),
                    path.display()
                );
            }
        }
    }
    Ok(Ok(()))
}

/// The fleet store, with every held declaration written into it first.
pub(super) async fn store() -> Result<JobStorage, CmdError> {
    let store = JobStorage::new().await?;
    deliver(&store).await??;
    Ok(store)
}
