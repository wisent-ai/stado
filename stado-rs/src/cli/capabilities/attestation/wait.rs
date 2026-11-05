//! Waiting for the statement that answers one kept request, for a caller of
//! `stado capabilities call --await-statement`.

use serde_json::Value;

use super::{answer_or_keep, answer_path, directory, request_digest};
use crate::capabilities::alternatives::Family;

/// The statement recorded for `request`, waiting for one when none is: the
/// request is kept first, as without waiting, then the consumer's directory
/// is watched by the kernel (kqueue, inotify) and read again on every write
/// to it, so the statement `attestation record` writes answers this same
/// call the moment it lands. The watch is armed before each read, so a
/// statement written between the read and the wait still wakes it. The wait
/// is a `sesja` wait: another session, or the operator, answers it; it is
/// listed with every other wait and ends when the caller ends it.
pub(in crate::cli::capabilities) fn await_statement(
    family: &Family,
    consumer: &str,
    request: &Value,
) -> Result<(String, Value), String> {
    let directory = directory(family, consumer)?;
    let digest = request_digest(request);
    let answer = answer_path(&directory, &digest);
    let what = format!(
        "a statement answering request {digest}: `stado capabilities attestation show {} --consumer {consumer} {digest}`, then `stado capabilities attestation record {} --consumer {consumer} {digest} --checked <what was checked>`",
        family.id, family.id
    );
    let watched = std::slice::from_ref(&directory);
    crate::wait::blocking(
        crate::wait::Kind::Session,
        what,
        answer.display(),
        || -> Result<Value, std::io::Error> {
            loop {
                let mut watch = crate::queue::local_file::watch_directories(watched)
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                if let (_, Some(statement)) =
                    answer_or_keep(family, consumer, request).map_err(std::io::Error::other)?
                {
                    return Ok(statement);
                }
                watch.next().map_err(|error| std::io::Error::other(error.to_string()))?;
            }
        },
    )
    .map(|statement| (digest, statement))
    .map_err(|error| error.to_string())
}
