//! The tailnet map read beside a long-running process, never on its path.
//!
//! A short-lived command reads `tailscale status --json` once, when a tailnet
//! name is first asked ([`super::MAP`]); a client that does not answer holds
//! that one command. `stado serve` cannot be held that way: its start step
//! `host identity` reaches the map through the store client, and a Tailscale
//! client that never exits (the macOS app binary started from a launchd agent
//! sits in its event loop) kept the API, the resolver and every role after
//! them from starting. So serve reads the map beside its roles: it enters
//! this mode before any role ([`read_beside`]) and from then on every reader
//! answers from the map the last client answer published, or as with no map
//! (the system resolver, the routes that need no tailnet) until one answers.
//!
//! One read runs at a time. It starts the client once, its wait is in the
//! register of open waits (`stado waits`) for as long as it runs, and nothing
//! ends it on a timer: a client that never answers is a fact the beacon
//! reports ([`client_state`], `tailscale_client`). The health role asks for a
//! read on every pass, and a pass while one still runs starts none.
//!
//! The client is this process's own and ends with it: it leads a process
//! group of its own, and [`end`], which `serve` runs however it ends, ends
//! that group, reaps the client and closes its wait before the process exits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use super::Tailnet;
use crate::primitives::lock::held;
use crate::wait::{Cancellation, Kind};

mod state;
pub(crate) use state::client_state;
use state::{now, Reading, BESIDE, STATE};

/// Fired by [`end`]: the running read ends its client's group, and no read
/// starts after it.
static ENDING: LazyLock<Cancellation> = LazyLock::new(Cancellation::new);

/// Whether a read runs now: a second is not started beside it.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// `inspect` over the map this process reads: in a process that reads it
/// beside its roles, the last published one or an empty map; in any other,
/// the one read once on first use.
pub(super) fn with_map<T>(inspect: impl FnOnce(&Tailnet) -> T) -> T {
    if !BESIDE.load(Ordering::Acquire) {
        return inspect(&*super::MAP);
    }
    let map = held(&STATE).map.clone();
    match map {
        Some(map) => inspect(&map),
        None => inspect(&Tailnet::default()),
    }
}

/// Read the map beside this process from now on, and start a read unless one
/// runs. Never waits for the client.
pub(crate) fn read_beside() {
    BESIDE.store(true, Ordering::Release);
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let binary = match super::binary() {
        Ok(Some(binary)) => binary,
        Ok(None) => return settle(Reading::NoClient { at: now() }, None),
        Err(cause) => {
            let client = "no client path".to_string();
            return settle(
                Reading::Failed {
                    at: now(),
                    client,
                    pid: None,
                    cause,
                },
                None,
            );
        }
    };
    let client = binary.display().to_string();
    // The client is started by the reader thread itself, so a child exists
    // only where something waits on it: a thread that cannot start leaves no
    // client unreaped beside it, and no later pass meets one it cannot see.
    // The thread starts under the state's lock, so [`end`] either finds it
    // to wait for or has already ended reading and none starts.
    let named = client.clone();
    let mut state = held(&STATE);
    if ENDING.is_cancelled() {
        drop(state);
        return RUNNING.store(false, Ordering::Release);
    }
    let started = std::thread::Builder::new()
        .name("tailnet-reader".to_string())
        .spawn(move || read(&binary, client));
    match started {
        Ok(reader) => state.reader = Some(reader),
        Err(error) => {
            drop(state);
            let cause =
                format!("the reader thread did not start, so no client was started: {error}");
            settle(
                Reading::Failed {
                    at: now(),
                    client: named,
                    pid: None,
                    cause,
                },
                None,
            );
        }
    }
}

/// Start `client` (the program at `binary`) with `status --json` as the
/// leader of a process group of its own, keep its pid while it runs, and
/// settle what it answered, or that [`end`] ended it.
fn read(binary: &std::path::Path, client: String) {
    let failed = |pid: Option<u32>, cause: String| Reading::Failed {
        at: now(),
        client: client.clone(),
        pid,
        cause,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let cause =
                format!("the reader's runtime did not start, so no client was started: {error}");
            return settle(failed(None, cause), None);
        }
    };
    let mut command = tokio::process::Command::from(super::command(binary));
    let mut pid = None;
    let output = runtime.block_on(crate::wait::output_cancellable(
        &mut command,
        &ENDING,
        |started| {
            pid = Some(started);
            held(&STATE).reading = Reading::Running {
                since: now(),
                pid: started,
                client: client.clone(),
            };
        },
    ));
    let answered = output
        .map_err(|error| match (pid, ENDING.is_cancelled()) {
            (None, _) => format!("`status --json` did not start: {error}"),
            (Some(_), true) => {
                format!("this process ended, so it ended its client's group: {error}")
            }
            (Some(_), false) => format!("`status --json` did not end readably: {error}"),
        })
        .and_then(super::decode);
    match answered {
        Ok(map) => settle(Reading::Answered { at: now(), client }, Some(map)),
        Err(cause) => settle(failed(pid, cause), None),
    }
}

/// End the client the reader beside this process started, and return once its
/// group is gone, it is reaped and its wait is closed; no read starts after
/// this. A process that started no read returns at once.
pub(crate) async fn end() -> Result<(), String> {
    let reader = {
        let mut state = held(&STATE);
        ENDING.cancel();
        state.reader.take()
    };
    let Some(reader) = reader else {
        return Ok(());
    };
    let joined = async move {
        let joined = tokio::task::spawn_blocking(move || reader.join())
            .await
            .map_err(|error| {
                std::io::Error::other(format!(
                    "the wait on the reader thread did not run: {error}"
                ))
            })?;
        joined.map_err(|_| std::io::Error::other("the tailnet reader thread panicked"))
    };
    crate::wait::until(
        Kind::Process,
        "the tailnet reader to end its client",
        "this process",
        joined,
    )
    .await
    .map_err(|error| format!("the tailnet reader did not end its client: {error}"))
}

/// Keep what a read ended with, and let the next one start. A failed read
/// keeps the map an earlier answer published: a name the tailnet answered
/// for is still its own.
fn settle(reading: Reading, map: Option<Tailnet>) {
    let mut state = held(&STATE);
    if let Reading::Failed { client, cause, .. } = &reading {
        eprintln!("[tailnet] {client} could not be read, so MagicDNS names keep the last map or the system resolver: {cause}");
    }
    state.reading = reading;
    if let Some(map) = map {
        state.map = Some(Arc::new(map));
    }
    drop(state);
    RUNNING.store(false, Ordering::Release);
}
