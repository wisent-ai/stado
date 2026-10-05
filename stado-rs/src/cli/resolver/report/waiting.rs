//! Channel opens the resolver is still waiting on, published so `resolver
//! status` can name a route that accepts connections and answers none.
//!
//! An adapter accepts its client at once and then asks the destination host
//! to open a channel to the service. When the service end never answers, the
//! client holds an accepted connection that receives nothing, and the only
//! trace used to be a running count in the serve log. Every open is now
//! recorded here from the moment it is sent until it is answered, with the
//! service, consumer, destination and start time, in a file beside the
//! resolver's state so the answer survives with the resolver stopped.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::probe::age_seconds;
use super::published::{now_iso, state_path};

const WAITING_FILE: &str = "resolver-waiting.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WaitingOpen {
    pub(crate) service: String,
    pub(crate) consumer: String,
    /// The host asked to open the channel.
    pub(crate) active_host: String,
    /// The `host:port` the channel was asked to reach on that host.
    pub(crate) endpoint: String,
    /// When the open was sent.
    pub(crate) since: String,
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

/// Record an open as sent; returns its key and how many are now waiting.
pub(crate) fn begin(
    service: &str,
    consumer: &str,
    active_host: &str,
    endpoint: &str,
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
            active_host: active_host.to_string(),
            endpoint: endpoint.to_string(),
            since: now_iso(),
        },
    );
    publish(&opens);
    (key, opens.len())
}

/// Record an open as answered, open or refused; returns how many still wait.
pub(crate) fn end(key: u64) -> usize {
    let mut opens = WAITING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    opens.remove(&key);
    publish(&opens);
    opens.len()
}

/// The published opens as JSON, and a blocker for each one that has waited
/// longer than the resolver's own declared refresh interval: by then the
/// resolver has re-read the directory at least once while that client still
/// held an accepted connection with nothing on it.
pub(super) fn report(local: bool, refresh_seconds: u64) -> (Vec<Value>, Vec<String>) {
    let published: Vec<WaitingOpen> = local
        .then(waiting_path)
        .flatten()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default();
    let mut listed = Vec::with_capacity(published.len());
    let mut blockers = Vec::new();
    for open in published {
        let waited = age_seconds(&open.since);
        if waited.is_some_and(|seconds| seconds > refresh_seconds as i64) {
            blockers.push(format!(
                "service {} for consumer {} has waited {}s for {} to open a channel to {}: \
                 the client holds an accepted connection that receives nothing; ask that host \
                 what listens on {}",
                open.service,
                open.consumer,
                waited.unwrap_or_default(),
                open.active_host,
                open.endpoint,
                open.endpoint
            ));
        }
        listed.push(json!({
            "service": open.service,
            "consumer": open.consumer,
            "active_host": open.active_host,
            "endpoint": open.endpoint,
            "since": open.since,
            "waited_seconds": waited,
        }));
    }
    (listed, blockers)
}
