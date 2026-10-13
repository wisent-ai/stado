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

pub(super) use listing::report;

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

/// Record a wait as started; returns its key and how many are now waiting.
pub(crate) fn begin(
    service: &str,
    consumer: &str,
    bind: &str,
    active_host: &str,
    endpoint: &str,
    phase: Phase,
    client: &str,
) -> (u64, usize) {
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
/// failure when it is there and cannot be read.
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
    serde_json::from_str(&body).map_err(|error| {
        format!(
            "the resolver's waiting list at {} is not the list this build writes: {error}",
            path.display()
        )
    })
}

/// The sentence for a wait that has gone on longer than the resolver's own
/// refresh interval: by then the resolver has re-read the directory at least
/// once while that client still held an accepted connection with nothing on
/// it.
fn unanswered(open: &WaitingOpen, waited: i64) -> String {
    let holder = match open.client.rsplit_once(':') {
        Some((_, port)) => format!(
            " held by the client at {}: `lsof -nP -i :{port}` names the process still holding \
             it",
            open.client
        ),
        None => String::new(),
    };
    format!(
        "service {} for consumer {} at {} has waited {waited}s {} to {} on {}: the client holds \
         an accepted connection that receives nothing; ask that host what answers on {}{holder}",
        open.service,
        open.consumer,
        open.bind,
        open.phase.describe(),
        open.endpoint,
        open.active_host,
        open.endpoint
    )
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
pub(crate) fn declared_bound_seconds() -> Option<u64> {
    declared_refresh_seconds()
}

/// The refresh interval this host's resolver declares, from the
/// last-known-good registry copy: the one document on disk that says it,
/// read without the store, since the store is what a held adapter keeps a
/// caller from reaching. `None` when this host keeps no copy or declares no
/// resolver: then it holds no adapter and nothing here can be held.
fn declared_refresh_seconds() -> Option<u64> {
    let document = crate::cli::resolver::last_good_document().ok()?;
    let target = crate::cli::resolver::current_target(&document).ok()?;
    crate::service_resolution::resolver_config(&document, &target)
        .ok()
        .map(|config| config.refresh_seconds)
}

/// Why a request to `url` would wait without an answer, when this host's
/// resolver already holds a connection on that adapter unanswered longer
/// than its refresh interval: the held wait's own sentence, with how many
/// stand there. `Ok(None)` when `url` is not one of this host's adapter
/// binds, or nothing on that bind has waited that long; `Err` when the
/// published list is there and cannot be read, because then nobody can say.
/// The registry copy, which says the interval, is read only once a wait on
/// that bind is found: every store request asks here, and most binds hold
/// nothing.
pub(crate) fn held_at(url: &url::Url) -> Result<Option<String>, String> {
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return Ok(None);
    };
    let bind = format!("{host}:{port}");
    let waiting: Vec<(WaitingOpen, i64)> = published()?
        .into_iter()
        .filter(|open| open.bind == bind)
        .filter_map(|open| age_seconds(&open.since).map(|waited| (open, waited)))
        .collect();
    if waiting.is_empty() {
        return Ok(None);
    }
    let Some(refresh_seconds) = declared_refresh_seconds() else {
        return Ok(None);
    };
    let held: Vec<&(WaitingOpen, i64)> = waiting
        .iter()
        .filter(|(_, waited)| *waited > refresh_seconds as i64)
        .collect();
    let Some((longest, waited)) = held.iter().max_by_key(|(_, waited)| *waited) else {
        return Ok(None);
    };
    Ok(Some(format!(
        "this host's resolver holds {} connection(s) on that adapter unanswered longer than the \
         {refresh_seconds}s refresh interval it declares, so a request sent now would stand behind \
         them: {}; `stado resolver status` lists every one",
        held.len(),
        unanswered(longest, *waited)
    )))
}
