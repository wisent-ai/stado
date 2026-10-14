//! The one way a Wisent program waits: Stado (as `stado::wait`) and every
//! product that depends on this crate.
//!
//! A command that waits on the network, an object API, a channel to a host,
//! a lock, a child process or a database used to wait without a word, and a
//! stall could not be told from slow work, nor the thing it was stuck on.
//! Every such wait goes through this crate instead. Before it starts
//! waiting it writes on stderr what it waits for, where and since when; when
//! it ends it writes how long it took; when it fails it names the operation
//! and the cause. Nothing here limits how long a wait may take or repeats
//! it: the crate only makes the wait visible. One reader (Oko's session
//! view) reads every program's waits from these lines, and every wait still
//! open is also a file in the register [`open`], which `stado waits` lists
//! however the waiting program's stderr was filtered or held back.
//!
//! Three lines, one per event, each a single write ending in a newline:
//!
//! ```text
//! czekam: <co>; gdzie: <gdzie>; rodzaj: <rodzaj>; od: <RFC 3339 UTC>; id: <pid>-<n>
//! koniec czekania: <co>; trwalo: <sekundy>s; id: <pid>-<n>
//! blad czekania: <co>; gdzie: <gdzie>; trwalo: <sekundy>s; przyczyna: <przyczyna>; id: <pid>-<n>
//! ```
//!
//! Fields are separated by `"; "` and every field after the first is
//! `name: value`. No value carries `;` or a line break: they are replaced by
//! `,` and a space. `id` pairs a start with its end inside one process,
//! because waits interleave. A wait whose future is dropped, or whose
//! blocking work panics, ends with a `blad czekania` line that says so.
//! Stdout is never written, so `--json` output is untouched.

// The cancellable waits and the async process waits need `tokio`; the HTTP
// waits need `reqwest`. A product that waits only on blocking work — child
// processes, locks, its own calls — takes neither (`default-features =
// false`), so the one wait mechanism reaches it without either client.
#[cfg(feature = "async")]
mod cancel;
pub mod cycle;
pub mod external;
#[cfg(feature = "http")]
mod http;
mod line;
pub mod open;
mod process;

#[cfg(feature = "async")]
pub use cancel::{Cancellation, CANCELLED};
#[cfg(feature = "http")]
pub use http::{refusal, request, request_blocking, sdk, send, send_built};
pub use process::{child_output, output, status};
#[cfg(feature = "async")]
pub use process::{child_output_async, output_async, output_cancellable, status_async};

use std::fmt::Display;
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

/// What a wait waits on, written in each line's `rodzaj` field; the kinds
/// and their words are declared in `kind/`. Three of them are waits nothing
/// answers by itself: `terminal` (a key typed on the terminal the program
/// runs on, which in an agent's shell nobody types), `zgoda` (the
/// operator's answer to a grant) and `sesja` (another agent session's
/// work). A reader of the register (Oko's wait watch) acts on each by its
/// kind rather than by how long it has stood. A wait a program does not
/// perform in Rust itself — a grant dialog in a harness, a terminal a
/// supervisor sees standing — is registered for that program's pid with
/// [`external::register_for`] or `stado waits open`, and ends when that
/// process does or with [`external::unregister`].
mod kind;
pub use kind::Kind;

/// The waits this process has started, so each gets its own id: a count
/// starting empty, and each wait's number is the count so far plus the
/// least step, so the first is the least positive number.
static STARTED: LazyLock<AtomicU64> = LazyLock::new(AtomicU64::default);

/// An error and every source under it, outermost first: the cause a
/// failed wait writes, for a caller that keeps that cause.
pub fn chain(error: &dyn std::error::Error) -> String {
    line::chain(error)
}

/// One wait in progress. Its end is written by [`Waiting::done`],
/// [`Waiting::failed`] or [`Waiting::settle`]; dropped without one of them,
/// it writes that the wait was cut short.
#[must_use = "a wait that is dropped at once says it was cut short"]
pub struct Waiting {
    id: String,
    what: String,
    place: String,
    kind: Kind,
    since: String,
    started: Instant,
    open: bool,
}

/// Say that a wait starts, before it starts.
pub fn begin(kind: Kind, what: impl Display, place: impl Display) -> Waiting {
    start(kind, what, place, None)
}

/// [`begin`], recording first the lock the wait awaits, when it awaits one,
/// in the wait-for graph.
fn start(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    awaits: Option<&std::path::Path>,
) -> Waiting {
    let id = format!(
        "{}-{}",
        std::process::id(),
        STARTED.fetch_add(NonZeroU64::MIN.get(), Ordering::Relaxed) + NonZeroU64::MIN.get()
    );
    let waiting = Waiting {
        id,
        what: line::field(&what.to_string()),
        place: line::field(&place.to_string()),
        kind,
        since: line::now(),
        started: Instant::now(),
        open: true,
    };
    let opening = waiting.open_line();
    // In the register first: whoever reads the line can find the file, and
    // the lock it awaits.
    if let Some(lock) = awaits {
        cycle::record(&waiting.id, lock);
    }
    open::opened(&waiting.id, &opening);
    line::write(&opening);
    waiting
}

impl Waiting {
    /// The `czekam` line this wait wrote when it began. A server that
    /// refuses a request because it would stand behind this wait hands the
    /// caller this line, so the refusal names what is waited on, where and
    /// since when in the same words the server's own log carries.
    pub fn open_line(&self) -> String {
        format!(
            "czekam: {}; gdzie: {}; rodzaj: {}; od: {}; id: {}",
            self.what,
            self.place,
            self.kind.word(),
            self.since,
            self.id
        )
    }

    /// When this wait began, RFC 3339 UTC: the `od:` field of
    /// [`Waiting::open_line`], for a server that keeps the line and wants
    /// to age it without parsing it back.
    pub fn since(&self) -> &str {
        &self.since
    }

    /// The `blad czekania` line this wait writes when it fails for `cause`,
    /// as it stands now. Read before [`Waiting::failed`] by a server that
    /// keeps the line to refuse later requests with.
    pub fn failure_line(&self, cause: &str) -> String {
        format!(
            "blad czekania: {}; gdzie: {}; trwalo: {}s; przyczyna: {}; id: {}",
            self.what,
            self.place,
            line::seconds(self.started),
            line::field(cause),
            self.id
        )
    }

    /// The wait ended with what it waited for.
    pub fn done(mut self) {
        self.open = false;
        line::write(&format!(
            "koniec czekania: {}; trwalo: {}s; id: {}",
            self.what,
            line::seconds(self.started),
            self.id
        ));
        open::closed(&self.id);
        cycle::forget(&self.id);
    }

    /// The wait ended in a failure, for this cause.
    pub fn failed(mut self, cause: impl Display) {
        self.open = false;
        self.write_failure(&cause.to_string());
    }

    /// End the wait the way `result` says, and hand `result` back unchanged.
    /// The cause written for an error is its whole chain of sources.
    pub fn settle<T, E: std::error::Error>(self, result: Result<T, E>) -> Result<T, E> {
        match &result {
            Ok(_) => self.done(),
            Err(error) => {
                let cause = line::chain(error);
                self.failed(cause)
            }
        }
        result
    }

    fn write_failure(&self, cause: &str) {
        line::write(&self.failure_line(cause));
        open::closed(&self.id);
        cycle::forget(&self.id);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        let cause = match std::thread::panicking() {
            true => "kod czekajacy spanikowal",
            false => "czekanie przerwane, zanim sie skonczylo",
        };
        self.write_failure(cause);
    }
}

/// Wait for `work`, saying so at both ends.
pub async fn until<T, E, F>(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    work: F,
) -> Result<T, E>
where
    E: std::error::Error,
    F: Future<Output = Result<T, E>>,
{
    let waiting = begin(kind, what, place);
    waiting.settle(work.await)
}

/// [`until`] that `cancellation` can end: `work` is raced against it, and
/// when the cancellation fires first `work` is dropped where it stood and
/// the wait ends with `przyczyna: czekanie odwolane`, answered as `None`.
#[cfg(feature = "async")]
pub async fn until_cancellable<T, E, F>(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    cancellation: &Cancellation,
    work: F,
) -> Option<Result<T, E>>
where
    E: std::error::Error,
    F: Future<Output = Result<T, E>>,
{
    let waiting = begin(kind, what, place);
    match cancellation.until(work).await {
        Some(result) => Some(waiting.settle(result)),
        None => {
            waiting.failed(CANCELLED);
            None
        }
    }
}

/// Wait for blocking `work` on this thread, saying so at both ends.
pub fn blocking<T, E: std::error::Error>(
    kind: Kind,
    what: impl Display,
    place: impl Display,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let waiting = begin(kind, what, place);
    waiting.settle(work())
}

/// [`blocking`] for a wait on the lock file `lock`, whose file names its
/// holder (`"pid": <n>`). The wait is recorded in the wait-for graph while it
/// stands; one that would close a cycle — the lock held, directly or through
/// other lock waits, by this process or a process whose completion needs it
/// — never starts, and the cycle is returned with every link named. A graph
/// that cannot be read does not stop the wait; it is said beside its line.
pub fn blocking_on_lock<T, E: std::error::Error>(
    what: impl Display,
    lock: &std::path::Path,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<Result<T, E>, cycle::Cycle> {
    match cycle::closed_by_waiting_on(lock) {
        Ok(Some(cycle)) => {
            line::write(&format!(
                "czekanie odmowione: {}; gdzie: {}; przyczyna: {}",
                line::field(&what.to_string()),
                line::field(&lock.display().to_string()),
                line::field(&cycle.to_string())
            ));
            return Err(cycle);
        }
        Ok(None) => {}
        Err(unreadable) => line::write(&format!(
            "graf czekan nieodczytany: {}; przyczyna: {}",
            line::field(&lock.display().to_string()),
            line::field(&unreadable)
        )),
    }
    let waiting = start(Kind::Lock, what, lock.display(), Some(lock));
    Ok(waiting.settle(work()))
}
