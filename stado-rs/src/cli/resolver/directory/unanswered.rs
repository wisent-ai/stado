//! An authority snapshot this machine already left unanswered, kept so the
//! next reader is told at once instead of waiting the same interval again.
//!
//! A registry read abandons the authority's `stado resolver snapshot` over
//! SSH once it stands longer than the refresh interval this machine declares,
//! and goes on to its last-known-good copy. Every command reads the
//! registry, so on a host whose authority accepts and answers nothing each of
//! them waited that whole interval in turn: `stado host ping` printed its
//! dependency's refusal and then stood, the install's converge stood, and
//! so did every agent's call. The abandoned read is written to
//! `~/.stado/authority-unanswered.json`; for one interval after it, a read
//! of that authority is refused at once with the earlier read's sentence
//! and the time it was given up, and the reader goes on to its copy. After
//! the interval the authority is asked again, and an answer clears the
//! record.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const FILE: &str = "authority-unanswered.json";

#[derive(Serialize, Deserialize)]
struct Unanswered {
    host: String,
    abandoned_at: DateTime<Utc>,
    seconds: u64,
    cause: String,
}

/// Where the record lives; a HOME that is not set is said, since without the
/// record every reader waits the whole interval again.
fn path() -> Option<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => Some(PathBuf::from(home).join(".stado").join(FILE)),
        None => {
            eprintln!(
                "stado: HOME is not set, so an unanswered read of the registry authority is not \
                 kept for the next reader"
            );
            None
        }
    }
}

/// Keep `cause`, the sentence a read of `host` was abandoned with after
/// `seconds`. Best effort: a record that cannot be written leaves the next
/// reader to wait the interval itself, which is what it did before, and the
/// failure is said.
pub(in crate::cli::resolver) fn remember(host: &str, seconds: u64, cause: &str) {
    let Some(path) = path() else {
        return;
    };
    let record = Unanswered {
        host: host.to_string(),
        abandoned_at: Utc::now(),
        seconds,
        cause: cause.to_string(),
    };
    let written = serde_json::to_vec(&record)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            let staged = path.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&staged, bytes)
                .and_then(|()| std::fs::rename(&staged, &path))
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!(
            "stado: the unanswered read of the registry authority {host} could not be kept at {}: {error}",
            path.display()
        );
    }
}

/// The authority answered: no reader is turned away on its account.
pub(in crate::cli::resolver) fn forget(host: &str) {
    let Some(path) = path() else {
        return;
    };
    if standing_record(&path).is_some_and(|record| record.host == host) {
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!(
                "stado: the record of an unanswered read of {host} at {} could not be cleared: {error}",
                path.display()
            );
        }
    }
}

fn standing_record(path: &std::path::Path) -> Option<Unanswered> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            eprintln!("stado: {} cannot be read: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(record) => Some(record),
        Err(error) => {
            eprintln!(
                "stado: {} is not a record of an unanswered read: {error}",
                path.display()
            );
            None
        }
    }
}

/// Why `host` is not asked now, when a read of it was given up less than
/// one interval ago: that read's sentence and when it was given up.
pub(in crate::cli::resolver) fn standing(host: &str) -> Option<String> {
    let record = standing_record(&path()?).filter(|record| record.host == host)?;
    let interval = match i64::try_from(record.seconds) {
        Ok(seconds) => chrono::Duration::seconds(seconds),
        Err(error) => {
            eprintln!(
                "stado: the unanswered read of {host} records {}s, which is no interval: {error}",
                record.seconds
            );
            return None;
        }
    };
    let retry_at = record.abandoned_at + interval;
    (Utc::now() < retry_at).then(|| {
        format!(
            "the registry authority {host} is not asked again before {}: a read of it was given up \
             at {} after {}s unanswered ({})",
            retry_at.to_rfc3339(),
            record.abandoned_at.to_rfc3339(),
            record.seconds,
            record.cause
        )
    })
}
