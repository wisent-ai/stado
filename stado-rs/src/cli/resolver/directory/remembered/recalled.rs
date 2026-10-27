//! The registry authority's snapshot this machine already read, kept so the
//! registry is read from a remote authority once per refresh interval, not
//! once per registry read.
//!
//! On a host whose registry authority is another host and whose own resolver
//! holds no generation it may answer from, every registry read is a `stado
//! resolver snapshot` over SSH on the authority. One command that read the
//! registry three times stood three whole snapshots in a row for the same
//! document, and every next command of a script stood its own, so the elapsed
//! time of a migration step was registry reads, not the step.
//!
//! An answered snapshot is kept for the refresh interval that measures reads
//! of that authority ([`crate::cli::resolver::bound_seconds`]): this host's
//! resolver would not refresh its own generation sooner either, so a reader
//! asking again inside it cannot be promised a newer document. It is kept in
//! this process and in `~/.stado/authority-snapshot.json`
//! (`STADO_AUTHORITY_SNAPSHOT_FILE` moves it), so the next command inside the
//! interval reads it too. An interval nobody declares keeps nothing, since
//! there is then no bound to keep it for. A registry write through this
//! process forgets both, so the write's own command and the next one read
//! what it wrote. The checks whether the authority answers now (`placement
//! move --authority`, `registry push --last-known-good`) call
//! [`super::super::document::authority_snapshot`] and never read this record.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::VersionedText;

const FILE: &str = "authority-snapshot.json";
const FILE_VARIABLE: &str = "STADO_AUTHORITY_SNAPSHOT_FILE";
const WHAT: &str = "kept registry authority snapshot";

/// One answered snapshot: whose it is, what it said, and until when it may
/// stand for the authority.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kept {
    authority: String,
    content: String,
    version: String,
    until: DateTime<Utc>,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

/// The kept snapshot of `authority` that still stands, from this process or
/// else from the record an earlier command left, with which of the two it
/// came from.
fn standing(authority: &str) -> Option<(Kept, &'static str)> {
    let in_process = match KEPT.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let (kept, from) = match in_process {
        Some(kept) => (kept, "this command already read"),
        None => (kept_on_disk()?, "an earlier command read"),
    };
    (kept.authority == authority && Utc::now() < kept.until).then_some((kept, from))
}

/// The kept snapshot of `authority` that still stands, said on stderr with
/// how long it still stands, or `None`.
pub(in crate::cli::resolver) fn recall(authority: &str) -> Option<VersionedText> {
    let (kept, from) = standing(authority)?;
    eprintln!(
        "stado: registry read from the snapshot of the registry authority {authority} {from}; \
         it stands {}s more, the refresh interval reads of that authority are measured by",
        kept.until.signed_duration_since(Utc::now()).num_seconds()
    );
    Some(VersionedText {
        content: kept.content,
        version: kept.version,
    })
}

/// Which version of `authority`'s snapshot this machine keeps and until
/// when, for `stado resolver status`; nothing is said on stderr, the report
/// is the answer.
pub(in crate::cli::resolver) fn kept(authority: &str) -> Option<(String, DateTime<Utc>)> {
    standing(authority).map(|(kept, _)| (kept.version, kept.until))
}

/// The record an earlier command left, or `None` when there is none; one
/// that cannot be read is said, and the authority is asked.
fn kept_on_disk() -> Option<Kept> {
    let read = crate::primitives::document::located(FILE_VARIABLE, FILE, WHAT)
        .and_then(|path| crate::primitives::document::read::<Kept>(&path, WHAT));
    match read {
        Ok(kept) => kept,
        Err(cause) => {
            eprintln!("stado: {cause}; the registry authority is asked");
            None
        }
    }
}

/// Keep the snapshot `authority` just answered for the refresh interval that
/// measures reads of it, in this process and on disk. Nothing is kept when no
/// interval is declared, and an interval or a record that cannot be had is
/// said, because every next read then asks the authority again.
pub(in crate::cli::resolver) fn keep(authority: &str, snapshot: &VersionedText) {
    let route = crate::cli::resolver::Route::Host(authority);
    let seconds = match crate::cli::resolver::bound_seconds(&route) {
        Ok(Some(bound)) => bound.seconds,
        Ok(None) => return,
        Err(unreadable) => {
            eprintln!(
                "stado: the snapshot of the registry authority {authority} is not kept for the \
                 next registry read, so that read asks the authority again: {unreadable}"
            );
            return;
        }
    };
    let Ok(seconds) = i64::try_from(seconds) else {
        eprintln!(
            "stado: the refresh interval of {authority}, {seconds}s, is beyond any time, so its \
             snapshot is not kept"
        );
        return;
    };
    let kept = Kept {
        authority: authority.to_string(),
        content: snapshot.content.clone(),
        version: snapshot.version.clone(),
        until: Utc::now() + chrono::Duration::seconds(seconds),
    };
    let written = crate::primitives::document::located(FILE_VARIABLE, FILE, WHAT)
        .and_then(|path| crate::primitives::document::write(&path, &kept, WHAT));
    if let Err(cause) = written {
        eprintln!("stado: {cause}; the next command asks the registry authority again");
    }
    match KEPT.lock() {
        Ok(mut guard) => *guard = Some(kept),
        Err(poisoned) => *poisoned.into_inner() = Some(kept),
    }
}

/// A registry write went through this process: no kept snapshot may answer
/// for the document it replaced, here or in the next command.
pub(crate) fn forget() {
    match KEPT.lock() {
        Ok(mut guard) => *guard = None,
        Err(poisoned) => *poisoned.into_inner() = None,
    }
    let removed = crate::primitives::document::located(FILE_VARIABLE, FILE, WHAT).and_then(
        |path| match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        },
    );
    if let Err(cause) = removed {
        eprintln!(
            "stado: the kept registry authority snapshot could not be removed after a registry \
             write, so the next command may read the document the write replaced: {cause}"
        );
    }
}
