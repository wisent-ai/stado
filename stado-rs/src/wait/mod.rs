//! The one way Stado waits.
//!
//! A command that waits on the network, an object API, a channel to a host,
//! a lock, a child process or a database used to wait without a word, and a
//! stall could not be told from slow work, nor the thing it was stuck on.
//! Every such wait goes through this module instead. Before it starts
//! waiting it writes on stderr what it waits for, where and since when; when
//! it ends it writes how long it took; when it fails it names the operation
//! and the cause. Nothing here limits how long a wait may take or repeats
//! it: the module only makes the wait visible. Another Wisent program writes
//! the same three lines from its own code, so one reader (Oko's session
//! view) reads every program's waits.
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

mod line;

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

/// Send one HTTP request and wait for its answer to begin, saying so at
/// both ends. What is waited for is the request's method, path and decoded
/// query (an object reads as its `stado://` URI, not its percent-encoding);
/// where is `service` at the request's origin.
pub async fn send(
    kind: Kind,
    service: &str,
    builder: reqwest::RequestBuilder,
) -> reqwest::Result<reqwest::Response> {
    let (client, request) = builder.build_split();
    send_built(kind, service, client, request?).await
}

/// [`send`] for a request already built: a caller that must read where the
/// request goes before sending it — to refuse a route this host's resolver
/// reports as held — builds it, reads its URL, and hands it here.
pub async fn send_built(
    kind: Kind,
    service: &str,
    client: reqwest::Client,
    request: reqwest::Request,
) -> reqwest::Result<reqwest::Response> {
    let url = request.url();
    let query: Vec<String> = url
        .query_pairs()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let what = format!("{} {} {}", request.method(), url.path(), query.join(" "));
    let place = format!("{service} {}", url.origin().ascii_serialization());
    until(kind, what, place, client.execute(request)).await
}

/// Send one HTTP request to any other service and wait for its answer to
/// begin, saying so at both ends. What is waited for is the method and the
/// path — never the query, which may carry a credential a service takes
/// there; where is the request's origin.
pub async fn request(builder: reqwest::RequestBuilder) -> reqwest::Result<reqwest::Response> {
    let (client, request) = builder.build_split();
    let request = request?;
    let what = format!("{} {}", request.method(), request.url().path());
    let place = request.url().origin().ascii_serialization();
    until(Kind::Network, what, place, client.execute(request)).await
}

/// Wait for one AWS SDK call — the future its fluent builder's `send`
/// returns — saying so at both ends. What is waited for is the operation's
/// builder as the SDK names it (`aws_sdk_s3::operation::get_object::…`).
pub async fn sdk<T, E, F>(call: F) -> Result<T, E>
where
    E: std::error::Error,
    F: Future<Output = Result<T, E>>,
{
    let what = std::any::type_name::<F>().trim_end_matches("::send::{{closure}}");
    until(Kind::Network, what, "the AWS API", call).await
}

/// The program and arguments `command` runs, as a person would type them.
fn command_line(command: &std::process::Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `command` to its end and collect what it printed, saying so at both
/// ends: what is waited for is its command line, where is the directory it
/// runs in (this process's own when none is set).
pub fn output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    let place = match command.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    };
    blocking(Kind::Process, command_line(command), place, || {
        command.output()
    })
}

/// The same for a `tokio` command: run it to its end and collect what it
/// printed, saying so at both ends.
pub async fn output_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::Output> {
    let standard = command.as_std();
    let place = match standard.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    };
    let what = command_line(standard);
    until(Kind::Process, what, place, command.output()).await
}

/// Run `command` to its end with its output going where it was told to,
/// saying so at both ends.
pub fn status(command: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    let place = match command.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    };
    blocking(Kind::Process, command_line(command), place, || {
        command.status()
    })
}

/// The same for a `tokio` command.
pub async fn status_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    let standard = command.as_std();
    let place = match standard.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    };
    let what = command_line(standard);
    until(Kind::Process, what, place, command.status()).await
}

/// Wait for a child already started — `what` names it, as its caller knows
/// it — and collect what it printed, saying so at both ends.
pub fn child_output(
    child: std::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = format!("pid {}", child.id());
    blocking(Kind::Process, what, place, || child.wait_with_output())
}

/// The same for a `tokio` child already started: `what` names it, as its
/// caller knows it.
pub async fn child_output_async(
    child: tokio::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "a child that has already ended".to_string(),
    };
    until(Kind::Process, what, place, child.wait_with_output()).await
}
