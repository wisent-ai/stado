//! `stado service watch-spawn` — sit on one host and name the parent of the
//! next process that matches a program, while that parent is still alive.
//!
//! NO Python original. This exists because of a diagnosis that no shipped
//! command could finish. On charless-mac-mini an **undeclared**
//! `stado agent --target charless-mac-mini` kept coming back within one to
//! four minutes of being reaped. Every replacement was read with `ppid 1` and
//! no launchd label holding it, which says only one thing: whatever started it
//! had already exited, so the process reparented to launchd. The question
//! "what started it" was therefore unanswerable from any snapshot taken after
//! the fact, and every snapshot this fleet can take is after the fact.
//!
//! Why the existing readers cannot do it:
//!
//! - [`super::host_exec`] is an exact allowlist of argument-free read-only
//!   programs. Its `ps ax -o pid -o ppid -o etime -o comm` entry deliberately
//!   carries no `command`, because process arguments are where the secrets
//!   are, and every stado unit executes the same binary — so `comm` cannot
//!   tell an agent from a resolver. Widening that table is the wrong repair
//!   and its own module says so.
//! - [`super::service::reap_undeclared_processes`] does read full argv, but
//!   one invocation is one snapshot, and it is a signalling command besides.
//! - Driving either from here in a loop cannot sample faster than an SSH
//!   round trip, which on this fleet is tens of seconds. A parent that
//!   backgrounds a child and exits lives for a fraction of one.
//!
//! So the loop has to run ON the host, and that is the whole design:
//!
//! - one fixed remote program, no interpolation except a vetted command
//!   substring, a sample count and a sleep, exactly the contract
//!   [`super::service::REAP_SCRIPT`] holds;
//! - it **signals nothing, starts nothing and writes nothing**. It reads
//!   `ps` on an interval and prints. A watch that could also act would be a
//!   supervisor, and this fleet already has too many of those;
//! - the ancestry of a new arrival is resolved out of the SAME `ps` snapshot
//!   that first saw it, not by asking the host again. Asking again is how the
//!   answer gets lost: by the time a second `ps` runs the parent is gone and
//!   the child reads `ppid 1`, which is the state that made the question
//!   unanswerable in the first place. Each ancestor also carries a live
//!   `alive` re-check, so a report can say whether the parent was still
//!   running at the moment its child was caught.
//!
//! The match never enters any argv. It is handed to `awk` through the
//! environment, because a `ps` sweep looking for `stado agent` would otherwise
//! find the `awk` that is looking for it and report the searcher as the
//! arrival. That is not a hypothetical: it is the first thing this script did.

mod model;
mod script;
mod watch;

#[cfg(test)]
mod script_match_privacy;

pub use self::model::{Ancestor, Arrival, Baseline, ProcessRow, WatchReport};
pub use self::watch::{parse_watch, watch_spawns};

/// Longest watch a single invocation will hold the channel open for.
///
/// An hour is far past any respawn cadence worth catching, and a bound means
/// a forgotten watch cannot pin an SSH session open forever.
pub const MAX_SECONDS: u64 = 3600;

/// Shortest gap between samples, in milliseconds.
///
/// Below this the sampler spends more time forking `ps` than waiting, and on a
/// six-hundred-process host that is a measurable load on a machine somebody
/// else is using.
pub const MIN_INTERVAL_MS: u64 = 200;

/// Longest gap between samples. Past this the watch is not catching a parent,
/// it is taking snapshots, and [`super::service`] already does that.
pub const MAX_INTERVAL_MS: u64 = 10_000;
