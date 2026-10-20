//! Waits on a resource of the wait-for graph: recorded as an edge while
//! they stand, and refused before they start when they would close a cycle.

use std::fmt::Display;
use std::future::Future;

use super::{closed_by_waiting_on, Cycle, Resource};
use crate::{line, start, Kind, Waiting};

/// Whether a wait of this process on `resource` would close a cycle: the
/// refusal line written and the cycle answered when it would. A graph that
/// cannot be read does not stop the wait; it is said beside its line.
fn refused(what: &str, resource: &Resource) -> Option<Cycle> {
    match closed_by_waiting_on(resource) {
        Ok(Some(cycle)) => {
            line::write(&format!(
                "czekanie odmowione: {}; gdzie: {}; przyczyna: {}",
                line::field(what),
                line::field(&resource.to_string()),
                line::field(&cycle.to_string())
            ));
            Some(cycle)
        }
        Ok(None) => None,
        Err(unreadable) => {
            line::write(&format!(
                "graf czekan nieodczytany: {}; przyczyna: {}",
                line::field(&resource.to_string()),
                line::field(&unreadable)
            ));
            None
        }
    }
}

/// [`crate::begin`] for a wait on `resource`, recorded in the wait-for graph
/// while it stands. One that would close a cycle — the resource held,
/// directly or through other waits, by this process or a process whose
/// completion needs it — never starts, and the cycle is returned with every
/// link named.
pub fn begin_on(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    resource: &Resource,
) -> Result<Waiting, Cycle> {
    let what = what.to_string();
    if let Some(cycle) = refused(&what, resource) {
        return Err(cycle);
    }
    Ok(start(kind, what, place, Some(resource)))
}

/// [`crate::blocking`] for a wait on `resource`, refused as [`begin_on`]
/// says.
pub fn blocking_on<T, E: std::error::Error>(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    resource: &Resource,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<Result<T, E>, Cycle> {
    let waiting = begin_on(kind, what, place, resource)?;
    Ok(waiting.settle(work()))
}

/// [`crate::until`] for a wait on `resource`, refused as [`begin_on`] says.
pub async fn until_on<T, E, F>(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    resource: &Resource,
    work: F,
) -> Result<Result<T, E>, Cycle>
where
    E: std::error::Error,
    F: Future<Output = Result<T, E>>,
{
    let waiting = begin_on(kind, what, place, resource)?;
    Ok(waiting.settle(work.await))
}

/// [`blocking_on`] for a wait on the lock file `lock`, whose file names its
/// holder (`"pid": <n>`), as every product installation's surface lock does.
pub fn blocking_on_lock<T, E: std::error::Error>(
    what: impl Display,
    lock: &std::path::Path,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<Result<T, E>, Cycle> {
    blocking_on(
        Kind::Lock,
        what,
        lock.display(),
        &Resource::lock(lock),
        work,
    )
}
