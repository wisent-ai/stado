//! Refusals, published best effort: the per-refusal dedupe, the store
//! opened once per process for callers that hold none, and the three entry
//! points a refusing component calls.

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

/// Whether this (host, reader, reason) refusal says something not yet
/// recorded by this process, marking it recorded.
///
/// A poisoned lock means another thread panicked mid-update; the refusal is
/// then written rather than dropped, because losing the evidence is worse
/// than writing one extra blob.
fn sentence_is_new(host: &str, reader: &str, reason: &str, detail: &str) -> bool {
    let key = (host.to_string(), reader.to_string(), reason.to_string());
    let mut table = match LAST_SENTENCE.lock() {
        Ok(table) => table,
        Err(_) => return true,
    };
    if table.get(&key).is_some_and(|last| last == detail) {
        return false;
    }
    table.insert(key, detail.to_string());
    true
}

/// Publish one reader refusal about `host`.
///
/// Best effort by contract: every failure — already recorded, storage down,
/// serialization — is swallowed, because this is called from inside a
/// caller's own error path and must never replace the caller's error with
/// its own. Written once per distinct sentence per (host, reader, reason).
///
/// `detail` is the component's own sentence and is stored verbatim.
pub async fn record_refusal(
    store: &JobStorage,
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) {
    if !sentence_is_new(host, reader, reason, detail) {
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
    let _ = store.upload_text(&path, &body).await;
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
/// Opens the fleet store itself and swallows every failure including the
/// open. What it no longer does is abandon the write partway: a refusal
/// nobody recorded is an outage nobody can read afterwards.
pub async fn report_refusal(host: &str, reader: &str, reason: &str, detail: &str) {
    if !sentence_is_new(host, reader, reason, detail) {
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
    let _ = async move {
        let store = shared_store().await?;
        store.upload_text(&path, &body).await.ok()
    }
    .await;
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
