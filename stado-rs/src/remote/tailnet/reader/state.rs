//! What the reader beside this process knows of the Tailscale client, and the
//! beacon's `tailscale_client` block that says it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::super::Tailnet;
use crate::primitives::lock::held;

/// What the reader beside this process last knew of the client.
pub(super) enum Reading {
    /// The first read has not been asked for.
    NotAsked,
    /// No declared path holds a Tailscale client: there is no map to read.
    NoClient { at: String },
    /// The client runs and has not answered.
    Running {
        since: String,
        pid: u32,
        client: String,
    },
    /// The client answered; `map` is what it said.
    Answered { at: String, client: String },
    /// The client could not be started, or answered no readable status.
    /// `pid` is the client's when one ran; none when none was started.
    Failed {
        at: String,
        client: String,
        pid: Option<u32>,
        cause: String,
    },
}

pub(super) struct State {
    pub(super) reading: Reading,
    /// The map the last answer published; none until a client answers.
    pub(super) map: Option<Arc<Tailnet>>,
    /// The thread of the read started last, which `end` waits for.
    pub(super) reader: Option<std::thread::JoinHandle<()>>,
}

pub(super) static STATE: Mutex<State> = Mutex::new(State {
    reading: Reading::NotAsked,
    map: None,
    reader: None,
});

/// Whether this process reads the map beside its roles.
pub(super) static BESIDE: AtomicBool = AtomicBool::new(false);

pub(super) fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The beacon's `tailscale_client` block: what the reader beside this process
/// knows of the client now. A client that has not answered is named with its
/// pid, since when it runs and the waits the register holds on it.
pub(crate) fn client_state() -> Value {
    if !BESIDE.load(Ordering::Acquire) {
        return json!({
            "state": "not_read_beside",
            "detail": "this process reads the tailnet map once, when a tailnet name is first asked",
        });
    }
    let state = held(&STATE);
    let nodes = state.map.as_ref().map(|map| map.members.len());
    match &state.reading {
        Reading::NotAsked => json!({"state": "not_asked"}),
        Reading::NoClient { at } => json!({"state": "absent", "at": at,
            "detail": "no declared path holds a Tailscale client; tailnet names go to the system resolver"}),
        Reading::Answered { at, client } => json!({"state": "answered", "at": at, "client": client,
            "addresses": nodes}),
        Reading::Failed {
            at,
            client,
            pid,
            cause,
        } => {
            json!({"state": "failed", "at": at, "client": client, "pid": pid,
            "cause": cause, "addresses_kept": nodes})
        }
        Reading::Running { since, pid, client } => {
            let (pid, since, client) = (*pid, since.clone(), client.clone());
            drop(state);
            let place = format!("pid {pid} in ");
            let waits = match crate::wait::open::listed() {
                Ok(listing) => json!(listing
                    .open
                    .into_iter()
                    .filter(|open| open.line.contains(&place))
                    .map(|open| open.line)
                    .collect::<Vec<_>>()),
                Err(error) => json!({ "unread": error.to_string() }),
            };
            json!({
                "state": "no_answer",
                "detail": format!("{client} running since {since}, no answer"),
                "client": client, "pid": pid, "since": since, "waits": waits,
                "addresses_kept": nodes,
            })
        }
    }
}
