//! Connections the resolver is still waiting on, published so `resolver
//! status` can name a route that accepts connections and answers none, and
//! so a client about to use that route can refuse instead of joining them.
//!
//! An adapter accepts its client at once and then asks the destination host
//! to open a channel to the service. Every wait is recorded here from the
//! moment it starts until it is answered, with the service, consumer,
//! adapter bind, destination and start time, beside the resolver's state so
//! the answer survives with the resolver stopped, in two phases: `open`,
//! until the host answers the channel open, and `answer`, from the client's
//! first byte going up the open channel until the service's first byte.
//!
//! [`held_at`] is the other reader: a client about to send a request to one
//! of this host's adapter binds asks whether that adapter already holds a
//! connection unanswered longer than the directory refresh interval, and
//! refuses with the sentence instead of standing behind it without end.
//! [`host::held_on_host`] asks the same of every connection to one host,
//! for a remote command about to ride that host's channel.

pub(in crate::cli::resolver) mod host;
pub(in crate::cli::resolver) mod inflight;
pub(in crate::cli::resolver) mod inspect;
mod listing;
pub(in crate::cli::resolver) mod session;

pub(crate) use host::held_at;
pub(super) use listing::report;
use listing::unanswered;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::probe::age_seconds;
use super::published::{now_iso, state_path};

const WAITING_FILE: &str = "resolver-waiting.json";

/// Which answer a recorded wait is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// The channel open was sent to the destination host and not answered.
    #[default]
    Open,
    /// The channel is open, the client's request went up it, and the service
    /// has not sent its first byte back.
    Answer,
}

impl Phase {
    fn describe(self) -> &'static str {
        match self {
            Self::Open => "to open a channel",
            Self::Answer => "for the first byte of an answer on an open channel",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WaitingOpen {
    pub(crate) service: String,
    pub(crate) consumer: String,
    /// The loopback address the adapter accepted the client on.
    #[serde(default)]
    pub(crate) bind: String,
    /// The host asked to open the channel.
    pub(crate) active_host: String,
    /// The `host:port` the channel was asked to reach on that host.
    pub(crate) endpoint: String,
    /// When the wait started.
    pub(crate) since: String,
    #[serde(default)]
    pub(crate) phase: Phase,
    /// The loopback address the client connected from: the one fact that
    /// ties the held connection to the process holding it (`lsof -i
    /// :<port>` names it), so a hook that ended hours ago is told from one
    /// still waiting. Empty in a list an older resolver published.
    #[serde(default)]
    pub(crate) client: String,
    /// When the resolver replaced the SSH session this wait rides with a new
    /// one to the same host ([`session::replaced`]); absent while it is the
    /// session new connections ride.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session_replaced_at: Option<String>,
    /// The request line the client sent first (`GET /api/object?… HTTP/1.1`),
    /// when its first bytes carried a whole line: the one fact that names
    /// which operation the holding process is waiting on. Hundreds of waits
    /// from one process stood in this list with nothing saying what they
    /// asked for. Absent for a wait to open a channel, and for a first chunk
    /// without a line end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request: Option<String>,
}

static WAITING: Mutex<BTreeMap<u64, WaitingOpen>> = Mutex::new(BTreeMap::new());
static NEXT: AtomicU64 = AtomicU64::new(0);

fn waiting_path() -> Option<std::path::PathBuf> {
    state_path().map(|path| path.with_file_name(WAITING_FILE))
}

/// Write the current set, atomically and best effort like the state file:
/// a resolver that cannot write its diagnostics still serves traffic.
fn publish(opens: &BTreeMap<u64, WaitingOpen>) {
    let Some(path) = waiting_path() else { return };
    let listed: Vec<&WaitingOpen> = opens.values().collect();
    let body = match serde_json::to_vec_pretty(&listed) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("stado resolver could not serialize its waiting channel opens: {error}");
            return;
        }
    };
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temp, &body).and_then(|()| std::fs::rename(&temp, &path)) {
        eprintln!(
            "{}",
            crate::primitives::staging::failed_with(
                format!(
                    "stado resolver could not publish its waiting channel opens to {}: {error}",
                    path.display()
                ),
                &temp,
            )
        );
    }
}

/// What a wait is for, as the relay knows it when the wait starts.
pub(crate) struct Began<'a> {
    pub(crate) service: &'a str,
    pub(crate) consumer: &'a str,
    pub(crate) bind: &'a str,
    pub(crate) active_host: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) phase: Phase,
    pub(crate) client: &'a str,
    pub(crate) request: Option<String>,
}

/// Record a wait as started; returns its key and how many are now waiting.
pub(crate) fn begin(began: Began<'_>) -> (u64, usize) {
    let Began {
        service,
        consumer,
        bind,
        active_host,
        endpoint,
        phase,
        client,
        request,
    } = began;
    let key = NEXT.fetch_add(1, Ordering::SeqCst);
    let mut opens = WAITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    opens.insert(
        key,
        WaitingOpen {
            service: service.to_string(),
            consumer: consumer.to_string(),
            bind: bind.to_string(),
            active_host: active_host.to_string(),
            endpoint: endpoint.to_string(),
            since: now_iso(),
            phase,
            client: client.to_string(),
            session_replaced_at: None,
            request,
        },
    );
    publish(&opens);
    (key, opens.len())
}

/// Publish this process's waits, none yet, as it starts serving: a list a
/// previous resolver left names connections no process holds now.
pub(crate) fn start() {
    match WAITING.lock() {
        Ok(opens) => publish(&opens),
        Err(poisoned) => publish(&poisoned.into_inner()),
    }
}

/// Record a wait as over, answered or refused; returns how many still wait.
pub(crate) fn end(key: u64) -> usize {
    let mut opens = WAITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    opens.remove(&key);
    publish(&opens);
    opens.len()
}

/// The waits this host's resolver has published: none when no resolver has
/// published any yet, every one when the file is there, and the file's own
/// failure when it is there and cannot be read. A list whose resolver has
/// exited names connections that went with it: the kernel closed them when
/// it ended, so nothing a request sends now stands behind them; a resolver
/// that starts again clears the list itself ([`start`]). With no published
/// state to name the writer, the list is read as it is.
fn published() -> Result<Vec<WaitingOpen>, String> {
    let Some(path) = waiting_path() else {
        return Ok(Vec::new());
    };
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "the resolver's waiting list at {} cannot be read: {error}",
                path.display()
            ))
        }
    };
    if let Some(state) = super::published::published_state() {
        if !super::published::publisher_running(&state) {
            return Ok(Vec::new());
        }
    }
    serde_json::from_str(&body).map_err(|error| {
        format!(
            "the resolver's waiting list at {} is not the list this build writes: {error}",
            path.display()
        )
    })
}

/// The waits this host's resolver holds to `service`, each as JSON with
/// how long it has waited, for a recovery record that names the requests
/// the service held; a list that cannot be read answers its own failure.
pub(crate) fn held_to_service(service: &str) -> Result<Vec<Value>, String> {
    Ok(published()?
        .into_iter()
        .filter(|open| open.service == service)
        .map(|open| {
            let waited = age_seconds(&open.since);
            let mut row = json!(open);
            row["waited_seconds"] = json!(waited);
            row
        })
        .collect())
}

/// The directory refresh interval this host's resolver declares: the one
/// measure of a wait that stands. `None` without a resolver configuration.
pub(crate) fn declared_bound_seconds() -> Result<Option<u64>, String> {
    declared_refresh_seconds()
}

/// The refresh interval of the resolver that published these waits, as that
/// resolver publishes it beside its state: the process that holds the waits
/// is the one that knows the interval it measures them by. When no resolver
/// on this machine has published one — none runs, or the state left behind
/// names no interval — the interval this machine's own registry record
/// declares for its resolver, read from the last-known-good copy and never
/// from another host. Without it a request to a host that accepted and
/// answers nothing, the authority's snapshot over SSH, was waited on without
/// end by every registry reader, an install's converge among them. `None`
/// only when neither names one.
fn declared_refresh_seconds() -> Result<Option<u64>, String> {
    let published = super::published::published_state();
    if let Some(seconds) = published.and_then(|state| state.refresh_seconds) {
        return Ok(Some(seconds));
    }
    let Some((registry, _)) = crate::targets::last_good_for_this_host() else {
        return Ok(None);
    };
    let hostname = crate::providers::vast::system_hostname();
    // A copy that names no record for this machine, or declares no resolver
    // for it, declares no interval: nothing here can be measured. A copy
    // that cannot be read, or a resolver declaration that does not parse,
    // is that failure: read as "no interval", every wait went unmeasured
    // and no held request was ever refused.
    let target = match registry.lookup_self(&hostname) {
        Ok(Some(target)) => target,
        Ok(None) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the refresh interval cannot be read: this host ({hostname}) cannot be looked up in its last-known-good registry: {error}"
            ))
        }
    };
    let document = crate::cli::resolver::directory::document::last_good_document().map_err(|error| {
        format!("the refresh interval cannot be read: the last-known-good registry document: {error}")
    })?;
    crate::service_resolution::declared_resolver_config(&document, &target.name)
        .map(|config| config.map(|config| config.refresh_seconds))
        .map_err(|error| {
            format!(
                "the refresh interval cannot be read: the resolver declaration of {}: {error}",
                target.name
            )
        })
}

/// The wait `key` has stood since `accepted`, when the client's connection
/// was accepted: a wait on an answer begins when the request goes up, but
/// the client has held its accepted connection from the accept on, through
/// the channel open before it, and that is what the held measure counts.
/// A wait on an answer that counted only from the request stood minutes
/// in the list after the client had held its connection far longer, most
/// of it opening the channel, and no reader abandoned it.
pub(crate) fn since_accepted(key: u64, accepted: &str) {
    let mut opens = match WAITING.lock() {
        Ok(opens) => opens,
        Err(poisoned) => poisoned.into_inner(),
    };
    let Some(open) = opens.get_mut(&key) else {
        return;
    };
    open.since = accepted.to_string();
    publish(&opens);
}
