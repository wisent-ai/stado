//! A wait that can be told to stop: the caller's own decision, delivered as
//! an event, not a clock. A tool call a session cancels, a request the
//! operator abandons, a program ending — each one holds a [`Cancellation`]
//! and calls [`Cancellation::cancel`]; a wait made with it ends at once with
//! `blad czekania … przyczyna: czekanie odwolane` and no child or task is
//! left running unowned. Nothing here guesses a duration: a wait without a
//! cancellation runs to its end as every other wait does.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;

/// The cause a cancelled wait writes.
pub const CANCELLED: &str = "czekanie odwolane";

/// One cancellation, shared by cloning: every holder sees the same state,
/// and `cancel` wakes every wait made with it.
#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    inner: Arc<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    cancelled: AtomicBool,
    notify: Notify,
}

impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tell every wait made with this cancellation to stop. Calling it again
    /// changes nothing.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// Resolve once cancelled; at once when already cancelled. A `select!`
    /// over this and the work is how an async wait stops on the event.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            let notified = self.inner.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    /// Run `work` until it answers or this cancellation fires, whichever is
    /// first: `Some` with the answer, `None` when cancelled — and then `work`
    /// is dropped where it stood, so nothing of it keeps running.
    pub async fn until<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            answered = work => Some(answered),
            _ = self.cancelled() => None,
        }
    }
}
