//! Waits registered for a process by another program.
//!
//! A wait a program performs in Rust is registered by [`crate::begin`]. Some
//! waits are not: a grant dialog a harness shows (`zgoda`), a terminal a
//! program reads in an agent's shell (`terminal`), another session a
//! supervisor sees an executor stand on (`sesja`). The program that knows
//! about such a wait registers it for the waiting process's pid, with the
//! same line, so every reader of the register reads it like any other; it
//! ends when the reader finds that process gone, or when its registrant
//! removes it.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::open::{directory, RegisterError};
use crate::Kind;

/// Register for process `pid` a wait on `kind`, for `what`, at `place`, and
/// answer its id. The id's number is the moment of registration in
/// nanoseconds, so it never meets the numbers that process counts its own
/// waits by. A register that cannot be written is the error naming it.
pub fn register_for(
    pid: u32,
    kind: Kind,
    what: &str,
    place: &str,
) -> Result<String, RegisterError> {
    let moment = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| RegisterError {
            cause: format!("this machine's clock reads before the epoch: {error}"),
            io: None,
        })?;
    let number = u64::try_from(moment.as_nanos()).map_err(|error| RegisterError {
        cause: format!("the moment {moment:?} does not fit a wait number: {error}"),
        io: None,
    })?;
    let id = format!("{pid}-{number}");
    let line = format!(
        "czekam: {}; gdzie: {}; rodzaj: {}; od: {}; id: {id}",
        crate::line::field(what),
        crate::line::field(place),
        kind.word(),
        crate::line::now(),
    );
    let directory = directory().map_err(|cause| RegisterError { cause, io: None })?;
    std::fs::create_dir_all(&directory).map_err(|error| RegisterError {
        cause: format!("creating {}: {error}", directory.display()),
        io: Some(error.kind()),
    })?;
    let file = directory.join(&id);
    std::fs::write(&file, format!("{line}\n")).map_err(|error| RegisterError {
        cause: format!("writing {}: {error}", file.display()),
        io: Some(error.kind()),
    })?;
    Ok(id)
}

/// [`register_for`] for a wait of process `pid` that awaits `resource`: the
/// resource is recorded in the wait-for graph with the wait. A wait that
/// would close a cycle — the resource held, directly or through other
/// waits, by `pid` or a process whose completion needs it — is not
/// registered, and the cycle is returned with every link named. A graph
/// that cannot be read is the refusal naming it, since the registrant asked
/// for the check.
pub fn register_awaiting(
    pid: u32,
    kind: Kind,
    what: &str,
    place: &str,
    resource: &crate::cycle::Resource,
) -> Result<Result<String, RegisterError>, crate::cycle::Cycle> {
    match crate::cycle::closed_by(pid, resource) {
        Ok(Some(cycle)) => return Err(cycle),
        Ok(None) => {}
        Err(cause) => {
            return Ok(Err(RegisterError {
                cause: format!("the wait-for graph cannot be read: {cause}"),
                io: None,
            }))
        }
    }
    let registered = register_for(pid, kind, what, place);
    if let Ok(id) = &registered {
        crate::cycle::record(id, resource);
    }
    Ok(registered)
}

/// Remove the wait `id` from the register: it ended. Answers whether the
/// register held it; one already gone (ended, or its process gone and read)
/// is not an error.
pub fn unregister(id: &str) -> Result<bool, RegisterError> {
    let directory = directory().map_err(|cause| RegisterError { cause, io: None })?;
    if id.contains(std::path::MAIN_SEPARATOR) || id.split_once('-').is_none() {
        return Err(RegisterError {
            cause: format!("{id} is not a wait id: an id is <pid>-<n>, as `stado waits` lists it"),
            io: None,
        });
    }
    let file = directory.join(id);
    crate::cycle::forget(id);
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RegisterError {
            cause: format!("removing {}: {error}", file.display()),
            io: Some(error.kind()),
        }),
    }
}
