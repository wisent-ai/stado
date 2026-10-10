//! Handing work to the client's runtime and waiting for its answer, each
//! statement's wait said on stderr through `stado::wait`, and ended early
//! by the caller's [`Cancellation`] when it holds one.

use std::future::Future;

use stado_wait::Cancellation;
use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

use super::Error;

/// How a hand-off ended.
pub(super) enum Answer<T> {
    Answered(T),
    /// The caller's cancellation fired: the work was dropped where it stood.
    Cancelled,
    /// The task stopped without an answer (it panicked).
    Stopped,
}

/// Run `work` on `runtime` and wait for its answer. With a cancellation the
/// work is raced against it on the runtime, so cancelling drops the work
/// and wakes this wait at once; the task is never left running behind a
/// receiver that has given up. Without one the work runs to its end.
pub(super) fn answer<T: Send + 'static>(
    runtime: &Runtime,
    cancellation: Option<&Cancellation>,
    work: impl Future<Output = T> + Send + 'static,
) -> Answer<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let cancellation = cancellation.cloned();
    runtime.spawn(async move {
        let outcome = match &cancellation {
            Some(cancellation) => match cancellation.until(work).await {
                Some(answered) => Answer::Answered(answered),
                None => Answer::Cancelled,
            },
            None => Answer::Answered(work.await),
        };
        let _ = sender.send(outcome);
    });
    // A closed channel means the task ended without sending: it stopped.
    let receive = move || match receiver.recv() {
        Ok(outcome) => outcome,
        Err(_) => Answer::Stopped,
    };
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
/// ended: the result, the cancellation, or the stopped task.
pub(super) fn wait<U, E>(
    runtime: &Runtime,
    cancellation: Option<&Cancellation>,
    what: impl std::fmt::Display,
    place: &str,
    work: impl Future<Output = Result<U, E>> + Send + 'static,
) -> Result<Result<U, E>, Error>
where
    U: Send + 'static,
    E: std::error::Error + Send + 'static,
{
    let waiting = stado_wait::begin(stado_wait::Kind::Database, what, place);
    match answer(runtime, cancellation, work) {
        Answer::Answered(result) => Ok(waiting.settle(result)),
        Answer::Cancelled => {
            waiting.failed(stado_wait::CANCELLED);
            Err(cancelled())
        }
        Answer::Stopped => {
            waiting.failed(STOPPED);
            Err(stopped())
        }
    }
}

pub(super) fn stopped() -> Error {
    Error::Conversion(STOPPED.to_owned())
}

pub(super) fn cancelled() -> Error {
    Error::Conversion(stado_wait::CANCELLED.to_owned())
}
