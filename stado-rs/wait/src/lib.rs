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
//! view) reads every program's waits from these lines.
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

mod http;
mod line;
mod process;

pub use http::{request, request_blocking, sdk, send, send_built};
pub use process::{child_output, child_output_async, output, output_async, status, status_async};

use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// What a wait is waiting on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A request to a network service that is not one of the kinds below.
    Network,
    /// A read, write, listing or stat through Stado's object API or another
    /// object store.
    ObjectApi,
    /// A channel to a fleet host: a connection, a remote command, a transfer.
    Host,
    /// A file lock or another lock held by a different process.
    Lock,
    /// A child process this program started and waits for.
    Process,
    /// A database connection or query.
    Database,
}

impl Kind {
    /// The word the `rodzaj` field carries.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Network => "siec",
            Kind::ObjectApi => "object-api",
            Kind::Host => "host",
            Kind::Lock => "blokada",
            Kind::Process => "proces",
            Kind::Database => "baza",
        }
    }
}

/// The waits this process has started, so each gets its own id.
static STARTED: AtomicU64 = AtomicU64::new(0);

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
    let id = format!(
        "{}-{}",
        std::process::id(),
        STARTED.fetch_add(1, Ordering::Relaxed) + 1
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
    line::write(&waiting.open_line());
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
