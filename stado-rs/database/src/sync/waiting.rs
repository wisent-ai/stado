//! Handing work to the client's runtime and waiting for its answer, each
//! statement's wait said on stderr through `stado::wait`.

use std::future::Future;

use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

use super::Error;

/// Run `work` on `runtime` and wait for its answer; `None` if the task
/// stopped without one.
pub(super) fn answer<T: Send + 'static>(
    runtime: &Runtime,
    work: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        let _ = sender.send(work.await);
    });
    let receive = move || receiver.recv().ok();
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(receive)
        }
        _ => receive(),
    }
}

pub(super) const STOPPED: &str = "the fleet database task stopped before it answered";

/// [`answer`] for one statement or transaction step of the database named
/// `place`, saying on stderr what it waits for before it waits and how it
/// ended.
pub(super) fn wait<U, E>(
    runtime: &Runtime,
    what: impl std::fmt::Display,
    place: &str,
    work: impl Future<Output = Result<U, E>> + Send + 'static,
) -> Option<Result<U, E>>
where
    U: Send + 'static,
    E: std::error::Error + Send + 'static,
{
    let waiting = stado_wait::begin(stado_wait::Kind::Database, what, place);
    match answer(runtime, work) {
        Some(result) => Some(waiting.settle(result)),
        None => {
            waiting.failed(STOPPED);
            None
        }
    }
}

pub(super) fn stopped() -> Error {
    Error::Conversion(STOPPED.to_owned())
}
