//! Refusals, published without replacing the caller's error: the
//! per-refusal dedupe (marked only once a record is written), the store
//! opened once per process for callers that hold none, and the three entry
//! points a refusing component calls. A record that is lost is said on
//! stderr with its cause.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use tokio::sync::OnceCell;

use crate::monitor::host_silence::paths::refusal_object_path;
use crate::monitor::host_silence::records::RefusalRecord;
use crate::queue::JobStorage;

type RefusalKey = (String, String, String);

/// The last sentence this process recorded for each (host, reader, reason).
///
/// Repeated requests produce the same refusal; a record is written when the
/// refusal is new or its sentence changed, so the store holds each state a
/// reader was refused in, not one copy per request. The keys are bounded by
/// the hosts, readers and reasons that exist.
static LAST_SENTENCE: LazyLock<Mutex<HashMap<RefusalKey, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether this process already recorded this (host, reader, reason)
/// refusal with this sentence.
///
/// A poisoned lock means another thread panicked mid-update; the refusal is
/// then written rather than dropped, because losing the evidence is worse
/// than writing one extra blob.
fn already_recorded(key: &RefusalKey, detail: &str) -> bool {
    match LAST_SENTENCE.lock() {
        Ok(table) => table.get(key).is_some_and(|last| last == detail),
        Err(_) => false,
    }
}

/// Mark a refusal recorded once its record is in the store. A refusal whose
/// write failed is not marked, so the next identical refusal writes it.
fn remember(key: RefusalKey, detail: &str) {
    if let Ok(mut table) = LAST_SENTENCE.lock() {
        table.insert(key, detail.to_string());
    }
}

/// Say on stderr that a refusal record was lost, and why. The caller's own
/// error stays its answer; the lost record is no longer silent.
fn unrecorded(host: &str, reader: &str, reason: &str, cause: impl std::fmt::Display) {
    eprintln!(
        "stado: the {reader} refusal about {host} ({reason}) was not recorded in the fleet store: {cause}"
    );
}

/// The store path and body of one refusal record.
fn refusal_blob(
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) -> Result<(String, String), serde_json::Error> {
    let at = Utc::now();
    let record = RefusalRecord {
        host: host.to_string(),
        at,
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    };
    let body = serde_json::to_string_pretty(&record)?;
    Ok((refusal_object_path(host, at), body))
}

/// Publish one reader refusal about `host`.
///
/// Called from inside a caller's own error path, so no failure replaces the
/// caller's error: a record that cannot be serialized or written is said on
/// stderr with its cause and left unmarked, so the next identical refusal
/// writes it. Written once per distinct sentence per (host, reader, reason).
///
/// `detail` is the component's own sentence and is stored verbatim.
pub async fn record_refusal(
    store: &JobStorage,
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) {
    let key = (host.to_string(), reader.to_string(), reason.to_string());
    if already_recorded(&key, detail) {
        return;
    }
    let (path, body) = match refusal_blob(host, reader, reason, detail) {
        Ok(blob) => blob,
        Err(cause) => return unrecorded(host, reader, reason, cause),
    };
    match store.upload_text(&path, &body).await {
        Ok(()) => remember(key, detail),
        Err(cause) => unrecorded(host, reader, reason, cause),
    }
}

/// Storage opened once per process for refusal publication.
///
/// The refusing components (the resolver's serve loop, one-shot CLI reads)
/// hold no `JobStorage`, and opening one per refusal would put a backend
/// handshake on an error path that is already the slow path of a sick
/// fleet. Only success is cached: a store that was unreachable during the
/// outage must be retried once it is back, which is the whole point.
static SHARED_STORE: OnceCell<JobStorage> = OnceCell::const_new();

async fn shared_store() -> Result<JobStorage, crate::queue::StorageError> {
    if let Some(store) = SHARED_STORE.get() {
        return Ok(store.clone());
    }
    let store = JobStorage::new().await?;
    let _ = SHARED_STORE.set(store.clone());
    Ok(store)
}

/// [`record_refusal`] for a caller that holds no [`JobStorage`].
///
/// Opens the fleet store itself; a store that cannot be opened is said on
/// stderr with its cause like any other lost record, because a refusal
/// nobody recorded is an outage nobody can read afterwards.
pub async fn report_refusal(host: &str, reader: &str, reason: &str, detail: &str) {
    let key = (host.to_string(), reader.to_string(), reason.to_string());
    if already_recorded(&key, detail) {
        return;
    }
    let (path, body) = match refusal_blob(host, reader, reason, detail) {
        Ok(blob) => blob,
        Err(cause) => return unrecorded(host, reader, reason, cause),
    };
    let store = match shared_store().await {
        Ok(store) => store,
        Err(cause) => return unrecorded(host, reader, reason, cause),
    };
    match store.upload_text(&path, &body).await {
        Ok(()) => remember(key, detail),
        Err(cause) => unrecorded(host, reader, reason, cause),
    }
}

/// [`report_refusal`] for a caller on a request path.
///
/// The resolver refuses EVERY resolution while its cache is stale, and a
/// resolution is something a workload is blocking on. Making each of those
/// refusals wait for a blob write would convert a fast, correct refusal into
/// a slow one — the diagnostic changing the behaviour it was added to
/// explain. The write is detached instead, which is right for a long-lived
/// service and wrong for a one-shot command: a command that exits
/// immediately after must `await` [`report_refusal`], or the process is gone
/// before the task runs.
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
