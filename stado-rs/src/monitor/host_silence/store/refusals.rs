//! Refusals, published best effort: the per-refusal throttle, the store
//! opened once per process for callers that hold none, and the three entry
//! points a refusing component calls.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::sync::OnceCell;

use crate::monitor::host_silence::paths::refusal_object_path;
use crate::monitor::host_silence::records::RefusalRecord;
use crate::queue::JobStorage;

/// At most one refusal blob per (host, reader, reason) per this interval.
///
/// The bound that matters: a resolver whose cache has gone stale refuses
/// EVERY request, and the outage that motivated this module lasted six
/// minutes. Without a throttle the diagnostic writes thousands of near
/// identical blobs into the store it is trying to keep readable, and the
/// count an operator reads becomes a measure of request volume rather than
/// of the fault. One record a minute per distinct refusal preserves the
/// shape of the incident and nothing else.
const REFUSAL_MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Hard ceiling on the throttle table, so a pathological caller cycling
/// host names cannot grow it without bound. Reached only by a bug; the
/// whole table is dropped rather than evicted cleverly, which costs one
/// extra blob per live key and no bookkeeping.
const REFUSAL_THROTTLE_CAPACITY: usize = 512;

/// Wall-clock ceiling on one best-effort refusal write, storage open
/// included. A diagnostic that blocks the error path it is annotating has
/// made the outage worse.
const REFUSAL_WRITE_BUDGET: Duration = Duration::from_secs(5);

type RefusalKey = (String, String, String);
type RefusalThrottle = Mutex<HashMap<RefusalKey, Instant>>;

static REFUSAL_THROTTLE: LazyLock<RefusalThrottle> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether this (host, reader, reason) may write again, marking it written.
///
/// A poisoned lock means another thread panicked mid-update; the refusal is
/// then written unthrottled rather than dropped, because losing the
/// evidence is worse than writing one extra blob.
fn throttle_admits(host: &str, reader: &str, reason: &str) -> bool {
    let key = (host.to_string(), reader.to_string(), reason.to_string());
    let mut table = match REFUSAL_THROTTLE.lock() {
        Ok(table) => table,
        Err(_) => return true,
    };
    let now = Instant::now();
    if let Some(last) = table.get(&key) {
        if now.duration_since(*last) < REFUSAL_MIN_INTERVAL {
            return false;
        }
    }
    if table.len() >= REFUSAL_THROTTLE_CAPACITY {
        table.clear();
    }
    table.insert(key, now);
    true
}

/// Publish one reader refusal about `host`.
///
/// Best effort by contract: every failure — throttled, storage down,
/// serialization — is swallowed, because this is called from inside a
/// caller's own error path and must never replace the caller's error with
/// its own. Bounded by [`REFUSAL_MIN_INTERVAL`] per distinct refusal and by
/// [`REFUSAL_WRITE_BUDGET`] per write.
///
/// `detail` is the component's own sentence and is stored verbatim.
pub async fn record_refusal(
    store: &JobStorage,
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) {
    if !throttle_admits(host, reader, reason) {
        return;
    }
    let at = Utc::now();
    let record = RefusalRecord {
        host: host.to_string(),
        at,
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    };
    let Ok(body) = serde_json::to_string_pretty(&record) else {
        return;
    };
    let path = refusal_object_path(host, at);
    let _ = tokio::time::timeout(REFUSAL_WRITE_BUDGET, store.upload_text(&path, &body)).await;
}

/// Storage opened once per process for refusal publication.
///
/// The refusing components (the resolver's serve loop, one-shot CLI reads)
/// hold no `JobStorage`, and opening one per refusal would put a backend
/// handshake on an error path that is already the slow path of a sick
/// fleet. Only success is cached: a store that was unreachable during the
/// outage must be retried once it is back, which is the whole point.
static SHARED_STORE: OnceCell<JobStorage> = OnceCell::const_new();

async fn shared_store() -> Option<JobStorage> {
    if let Some(store) = SHARED_STORE.get() {
        return Some(store.clone());
    }
    let store = JobStorage::new().await.ok()?;
    let _ = SHARED_STORE.set(store.clone());
    Some(store)
}

/// [`record_refusal`] for a caller that holds no [`JobStorage`].
///
/// Opens the fleet store itself, inside the same bounded budget, and
/// swallows every failure including the open.
pub async fn report_refusal(host: &str, reader: &str, reason: &str, detail: &str) {
    if !throttle_admits(host, reader, reason) {
        return;
    }
    let at = Utc::now();
    let record = RefusalRecord {
        host: host.to_string(),
        at,
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    };
    let Ok(body) = serde_json::to_string_pretty(&record) else {
        return;
    };
    let path = refusal_object_path(host, at);
    let _ = tokio::time::timeout(REFUSAL_WRITE_BUDGET, async move {
        let store = shared_store().await?;
        store.upload_text(&path, &body).await.ok()
    })
    .await;
}

/// [`report_refusal`] for a caller on a request path.
///
/// The resolver refuses EVERY resolution while its cache is stale, and a
/// resolution is something a workload is blocking on. Making each of those
/// refusals wait up to [`REFUSAL_WRITE_BUDGET`] for a blob write would
/// convert a fast, correct refusal into a client timeout — the diagnostic
/// changing the behaviour it was added to explain. The write is detached
/// instead, which is right for a long-lived service and wrong for a
/// one-shot command: a command that exits immediately after must
/// `await` [`report_refusal`], or the process is gone before the task runs.
///
/// Requires a Tokio runtime, which every caller of this already has.
pub fn report_refusal_detached(
    host: String,
    reader: &'static str,
    reason: &'static str,
    detail: String,
) {
    tokio::spawn(async move {
        report_refusal(&host, reader, reason, &detail).await;
    });
}
