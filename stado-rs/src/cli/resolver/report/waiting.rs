//! Connections the resolver is still waiting on, published so `resolver
//! status` can name a route that accepts connections and answers none, and
//! so a client about to use that route can refuse instead of joining them.
//!
//! An adapter accepts its client at once and then asks the destination host
//! to open a channel to the service. When the service end never answers,
//! the client holds an accepted connection that receives nothing, and the
//! only trace used to be a running count in the serve log. Every wait is
//! recorded here from the moment it starts until it is answered, with the
//! service, consumer, adapter bind, destination and start time, in a file
//! beside the resolver's state so the answer survives with the resolver
//! stopped. Two phases are recorded: `open`, from the channel open being
//! sent until the host answers it, and `answer`, from the client's first
//! byte going up the open channel until the service's first byte comes
//! back. A channel that opens and then answers nothing to the request on it
//! is the second phase; it showed in no list at all while a client waited on
//! it for hours.
//!
//! [`held_at`] is the other reader: a client about to send a request to one
//! of this host's adapter binds asks whether that adapter already holds a
//! connection unanswered longer than the directory refresh interval, and
//! refuses with the sentence instead of standing behind it without end.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::probe::age_seconds;
use super::published::{now_iso, state_path};

const WAITING_FILE: &str = "resolver-waiting.json";

/// Which answer a recorded wait is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    let Ok(body) = serde_json::to_vec_pretty(&listed) else {
        return;
    };
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    if let Err(error) = std::fs::write(&temp, &body).and_then(|()| std::fs::rename(&temp, &path)) {
        let _ = std::fs::remove_file(&temp);
        eprintln!(
            "stado resolver could not publish its waiting channel opens to {}: {error}",
            path.display()
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
        },
    );
    publish(&opens);
    (key, opens.len())
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
    format!(
        "service {} for consumer {} at {} has waited {waited}s {} to {} on {}: the client holds \
         an accepted connection that receives nothing; ask that host what answers on {}",
        open.service,
        open.consumer,
        open.bind,
        open.phase.describe(),
        open.endpoint,
        open.active_host,
        open.endpoint
    )
}

/// The published waits as JSON, and a blocker for each one that has waited
/// longer than `refresh_seconds`; a list that cannot be read is a blocker
/// of its own, since nothing can then say whether a route is held.
pub(super) fn report(local: bool, refresh_seconds: u64) -> (Vec<Value>, Vec<String>) {
    let mut blockers = Vec::new();
    let published = match (local, published()) {
        (false, _) => Vec::new(),
        (true, Ok(published)) => published,
        (true, Err(unreadable)) => {
            blockers.push(unreadable);
            Vec::new()
        }
    };
    let mut listed = Vec::with_capacity(published.len());
    for open in published {
        let waited = age_seconds(&open.since);
        if let Some(seconds) = waited.filter(|seconds| *seconds > refresh_seconds as i64) {
            blockers.push(unanswered(&open, seconds));
        }
        listed.push(json!({
            "service": open.service,
            "consumer": open.consumer,
            "bind": open.bind,
            "active_host": open.active_host,
            "endpoint": open.endpoint,
            "since": open.since,
            "phase": open.phase,
            "waited_seconds": waited,
        }));
    }
    (listed, blockers)
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
