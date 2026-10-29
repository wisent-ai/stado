//! A wait's deadline, measured: the longest any wait on the same operation
//! has taken to complete on this machine.
//!
//! A wait with no deadline in its line could stand for an hour on one
//! connection, or minutes on one query, and nothing reading the waits could
//! tell it from slow work. No number in this crate says how long a wait may
//! stand, and nothing here cuts one short. Every wait that completes records
//! how long it took under its operation ([`store`]), and a later wait on the
//! same operation is due once it has stood as long as the longest completed
//! one. Its `czekam` line says so:
//!
//! ```text
//! ...; od: <RFC 3339 UTC>; termin: <RFC 3339 UTC>; pomiar: <seconds>s, <operation>; id: ...
//! ...; od: <RFC 3339 UTC>; termin: brak; pomiar: <why there is none>; id: ...
//! ```
//!
//! The register of open waits keeps that line, so Oko's wait watch settles
//! every wait standing past its `termin`, naming what it waited on, where
//! and the measurement, and counts it; a wait whose operation has never
//! completed here says so in `pomiar`.
//!
//! An operation is what a wait waits on, read from the wait's structure:
//! the program file a child runs and its subcommand, the first argument
//! that is neither an option nor an assignment (`proces git status`,
//! `proces cargo check`; a program run with options only is its file
//! alone), the method and origin of an HTTP request (`siec GET https://…`),
//! and for every other wait its kind and what it waits for, exactly as the
//! caller wrote it. A caller whose child takes an option's value before its
//! subcommand (`git -C <checkout> status`) names the operation itself
//! (`output_as`, `status_as`), since no reading of the arguments can tell a
//! value from a subcommand.
//!
//! Opening a wait reads its operation's measurement and completing one may
//! rewrite it ([`store`]): one small file read on every wait and a locked
//! write when a completion is the longest, on the caller's thread, async
//! waits included.

pub mod store;

use std::time::Duration;

use crate::{line, Kind};

/// The operation of a wait of `kind` on `what`, written by a caller.
pub(crate) fn operation(kind: Kind, what: &str) -> String {
    format!("{} {}", kind.word(), line::field(what))
}

/// The operation of a child `command` runs: its program file and its
/// subcommand, the first argument that is neither an option (it starts with
/// `-`) nor an assignment (`name=value`, as `git -c key=value` gives one).
pub(crate) fn program(command: &std::process::Command) -> String {
    let program = command.get_program();
    let file = match std::path::Path::new(program).file_name() {
        Some(file) => file,
        None => program,
    };
    let subcommand = command
        .get_args()
        .map(|argument| argument.to_string_lossy())
        .find(|argument| !argument.starts_with('-') && !argument.contains('='));
    match subcommand {
        Some(subcommand) => format!(
            "{} {} {}",
            Kind::Process.word(),
            file.to_string_lossy(),
            line::field(&subcommand)
        ),
        None => format!("{} {}", Kind::Process.word(), file.to_string_lossy()),
    }
}

/// `took`, exactly as kept, written for a person: whole seconds and the
/// nanoseconds after them.
pub(crate) fn seconds(took: Duration) -> String {
    format!("{}.{:09}s", took.as_secs(), took.subsec_nanos())
}

/// The `termin:` and `pomiar:` fields a wait on `operation` opens with.
pub(crate) fn fields(operation: &str) -> String {
    let measured = store::longest(operation).and_then(|longest| match longest {
        Some(longest) => due(longest).map(|moment| Some((moment, longest))),
        None => Ok(None),
    });
    match measured {
        Ok(Some((moment, longest))) => {
            format!(
                "; termin: {moment}; pomiar: {}, {operation}",
                seconds(longest)
            )
        }
        Ok(None) => {
            format!("; termin: brak; pomiar: {operation} nie skonczylo sie tu jeszcze ani razu")
        }
        Err(cause) => format!("; termin: brak; pomiar: {}", line::field(&cause)),
    }
}

/// The moment, RFC 3339 UTC to the nanosecond, `longest` from now.
fn due(longest: Duration) -> Result<String, String> {
    let delta = chrono::TimeDelta::from_std(longest)
        .map_err(|error| format!("{} is no clock interval: {error}", seconds(longest)))?;
    match chrono::Utc::now().checked_add_signed(delta) {
        Some(moment) => Ok(moment.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)),
        None => Err(format!(
            "{} from now is past the clock's last moment",
            seconds(longest)
        )),
    }
}

/// Record that a wait on `operation` completed in `took`. A measurement
/// that cannot be kept is said on stderr beside the wait's own lines.
pub(crate) fn completed(operation: &str, took: Duration) {
    if let Err(cause) = store::record(operation, took) {
        line::write(&format!(
            "pomiar czekania niezapisany: {operation}; przyczyna: {}",
            line::field(&cause)
        ));
    }
}
