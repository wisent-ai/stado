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
//!
//! A read is abandoned only once it has stood the whole interval, and a
//! one-shot command is often ended by its caller at about that moment (an
//! agent's hook, a shell's limit), before it could keep the record; every
//! next command then waited the whole interval again. So each read also
//! says when it was sent, in `~/.stado/authority-asking.json`: a reader that
//! finds a read of the same authority sent one interval or more ago, neither
//! answered nor given up, keeps it as given up and is refused at once.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const FILE: &str = "authority-unanswered.json";
const ASKING: &str = "authority-asking.json";

/// A read of `host` sent at `asked_at` that has not yet answered or been
/// given up; `seconds` is the interval it is allowed to stand.
#[derive(Serialize, Deserialize)]
struct Asking {
    host: String,
    asked_at: DateTime<Utc>,
    seconds: u64,
}

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
    in_stado(FILE)
}

fn in_stado(name: &str) -> Option<PathBuf> {
    match std::env::var_os("HOME") {
        Some(home) => Some(PathBuf::from(home).join(".stado").join(name)),
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
    if let Err(error) = crate::primitives::document::write(&path, &record, "unanswered read record")
    {
        eprintln!(
            "stado: the unanswered read of the registry authority {host} could not be kept at {}: {error}",
            path.display()
        );
    }
    answered(host);
}

/// Say that a read of `host`, allowed to stand `seconds`, is being sent now.
/// Best effort like [`remember`]: a record that cannot be written is said.
pub(in crate::cli::resolver) fn asking(host: &str, seconds: u64) {
    let Some(path) = in_stado(ASKING) else {
        return;
    };
    let record = Asking {
        host: host.to_string(),
        asked_at: Utc::now(),
        seconds,
    };
    if let Err(error) = crate::primitives::document::write(&path, &record, "asking read record") {
        eprintln!(
            "stado: the read of the registry authority {host} being sent could not be kept at {}: {error}",
            path.display()
        );
    }
}

/// The read of `host` ended, answered or refused, or was given up: it is no
/// longer in flight.
pub(in crate::cli::resolver) fn answered(host: &str) {
    let Some(path) = in_stado(ASKING) else {
        return;
    };
    if read_record::<Asking>(&path).is_some_and(|record| record.host == host) {
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!(
                "stado: the record of a read of {host} in flight at {} could not be cleared: {error}",
                path.display()
            );
        }
    }
}

/// A read of `host` sent one interval or more ago that neither answered nor
/// was given up: its reader was ended at the interval, or still waits. It is
/// kept as given up, so this and every next reader go on to their copy.
fn stale_asking(host: &str) {
    let Some(path) = in_stado(ASKING) else {
        return;
    };
    let Some(record) = read_record::<Asking>(&path).filter(|record| record.host == host) else {
        return;
    };
    let Some(interval) = interval_of(host, record.seconds) else {
        return;
    };
    if Utc::now() < record.asked_at + interval {
        return;
    }
    remember(
        host,
        record.seconds,
        &format!(
            "a read of it sent at {} was neither answered nor given up within {}s; the command \
             that sent it was ended at that limit or still waits",
            record.asked_at.to_rfc3339(),
            record.seconds
        ),
    );
}

fn interval_of(host: &str, seconds: u64) -> Option<chrono::Duration> {
    match i64::try_from(seconds) {
        Ok(seconds) => Some(chrono::Duration::seconds(seconds)),
        Err(error) => {
            eprintln!(
                "stado: the read of {host} records {seconds}s, which is no interval: {error}"
            );
            None
        }
    }
}
/// The authority answered: no reader is turned away on its account.
pub(in crate::cli::resolver) fn forget(host: &str) {
    let Some(path) = path() else {
        return;
    };
    answered(host);
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
    read_record(path)
}

/// The record at `path`, `None` when there is none; one that cannot be read
/// or parsed is said and read as none, the best effort every record here is.
fn read_record<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    match crate::primitives::document::read(path, "registry authority read record") {
        Ok(record) => record,
        Err(error) => {
            eprintln!("stado: {error}");
            None
        }
    }
}

/// Why `host` is not asked now, when a read of it was given up less than
/// one interval ago: that read's sentence and when it was given up.
pub(in crate::cli::resolver) fn standing(host: &str) -> Option<String> {
    stale_asking(host);
    let record = standing_record(&path()?).filter(|record| record.host == host)?;
    let interval = interval_of(host, record.seconds)?;
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
