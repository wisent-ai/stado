//! Refusals, published without replacing the caller's error: the
//! per-refusal dedupe (one write in flight per refusal, marked only once a
//! record is written), the store opened once per process for callers that
//! hold none, and the two entry points a refusing component calls. A
//! record that is lost is said on stderr with its cause.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use tokio::sync::OnceCell;

use crate::monitor::host_silence::paths::refusal_object_path;
use crate::monitor::host_silence::records::RefusalRecord;
use crate::queue::JobStorage;

type RefusalKey = (String, String, String);

/// What this process holds for one (host, reader, reason) refusal.
#[derive(Default)]
struct Recording {
    /// The sentence whose record is in the store.
    written: Option<String>,
    /// The sentence whose record is being written now.
    writing: Option<String>,
    /// The newest other sentence raised while that write stood, written
    /// once it ends.
    next: Option<String>,
}

/// Every refusal this process recorded or is recording.
///
/// Repeated requests produce the same refusal; a record is written when the
/// refusal is new or its sentence changed, so the store holds each state a
/// reader was refused in, not one copy per request. At most one write per
/// refusal stands at a time: a resolver whose directory is stale refuses
/// every resolution, and a record marked only once written let every
/// resolution open its own write of the same sentence while the store did
/// not answer, each one more connection waiting on that store. The keys are
/// bounded by the hosts, readers and reasons that exist.
static RECORDINGS: LazyLock<Mutex<HashMap<RefusalKey, Recording>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The right to write one refusal's record, held while the write stands.
///
/// Dropped before [`Claim::settle`] (a caller's future cancelled mid-write),
/// it frees the refusal so the next one raised writes it.
struct Claim {
    key: RefusalKey,
    sentence: String,
    settled: bool,
}

/// The refusal table, also after a thread panicked holding it.
///
/// Every update of a [`Recording`] is whole, so a holder that panicked leaves
/// the table between updates. Skipping the table on a poisoned lock instead
/// would turn off the dedupe for the rest of the process: every refusal would
/// open its own write, which is the burst the table exists to prevent, and a
/// write that ended unsettled would leave its refusal marked as being
/// written, so it would never be written again.
fn recordings() -> std::sync::MutexGuard<'static, HashMap<RefusalKey, Recording>> {
    match RECORDINGS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Claim {
    /// Claim the write of `detail`, unless its record is already in the
    /// store or a write of this refusal stands; a different sentence raised
    /// meanwhile is kept and written by that write's caller once it ends.
    fn take(key: RefusalKey, detail: &str) -> Option<Self> {
        let mut table = recordings();
        let recording = table.entry(key.clone()).or_default();
        if let Some(writing) = &recording.writing {
            if writing != detail {
                recording.next = Some(detail.to_string());
            }
            return None;
        }
        if recording.written.as_deref() == Some(detail) {
            return None;
        }
        recording.writing = Some(detail.to_string());
        Some(Self {
            key,
            sentence: detail.to_string(),
            settled: false,
        })
    }

    /// End this write: mark the sentence recorded when its record is in the
    /// store (a failed write is not marked, so the next identical refusal
    /// writes it), and claim the sentence raised while it stood, if that is
    /// not the one now in the store.
    fn settle(mut self, written: bool) -> Option<Self> {
        self.settled = true;
        let mut table = recordings();
        let recording = table.entry(self.key.clone()).or_default();
        if written {
            recording.written = Some(self.sentence.clone());
        }
        recording.writing = None;
        let next = recording
            .next
            .take()
            .filter(|next| recording.written.as_deref() != Some(next.as_str()))?;
        recording.writing = Some(next.clone());
        Some(Self {
            key: self.key.clone(),
            sentence: next,
            settled: false,
        })
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        if let Some(recording) = recordings().get_mut(&self.key) {
            recording.writing = None;
        }
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
) -> Result<(String, String), crate::queue::StorageError> {
    let at = Utc::now();
    let record = RefusalRecord {
        host: host.to_string(),
        at,
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    };
    let body = serde_json::to_string_pretty(&record)?;
    Ok((refusal_object_path(host, at)?, body))
}

/// Where a refusal record is written: a store the caller holds, or the one
/// this process opens for callers that hold none.
#[derive(Clone, Copy)]
enum Destination<'a> {
    Held(&'a JobStorage),
    Shared,
}

/// Write the claimed sentence, then each sentence raised while a write of
/// the same refusal stood, until none is left.
async fn publish(
    mut claim: Claim,
    destination: Destination<'_>,
    host: &str,
    reader: &str,
    reason: &str,
) {
    loop {
        let written = write_record(destination, host, reader, reason, &claim.sentence).await;
        match claim.settle(written) {
            Some(next) => claim = next,
            None => return,
        }
    }
}

/// One record in the store; whether it landed. A record that cannot be
/// serialized or written is said on stderr with its cause.
async fn write_record(
    destination: Destination<'_>,
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) -> bool {
    let (path, body) = match refusal_blob(host, reader, reason, detail) {
        Ok(blob) => blob,
        Err(cause) => {
            unrecorded(host, reader, reason, cause);
            return false;
        }
    };
    let store = match destination {
        Destination::Held(store) => store,
        Destination::Shared => match shared_store().await {
            Ok(store) => store,
            Err(cause) => {
                unrecorded(host, reader, reason, cause);
                return false;
            }
        },
    };
    match store.upload_text(&path, &body).await {
        Ok(()) => true,
        Err(cause) => {
            unrecorded(host, reader, reason, cause);
            false
        }
    }
}

fn refusal_key(host: &str, reader: &str, reason: &str) -> RefusalKey {
    (host.to_string(), reader.to_string(), reason.to_string())
}

/// Publish one reader refusal about `host`.
///
/// Called from inside a caller's own error path, so no failure replaces the
/// caller's error: a record that cannot be serialized or written is said on
/// stderr with its cause and left unmarked, so the next identical refusal
/// writes it. Written once per distinct sentence per (host, reader, reason),
/// and never while a write of the same refusal stands.
///
/// `detail` is the component's own sentence and is stored verbatim.
pub async fn record_refusal(
    store: &JobStorage,
    host: &str,
    reader: &str,
    reason: &str,
    detail: &str,
) {
    if let Some(claim) = Claim::take(refusal_key(host, reader, reason), detail) {
        publish(claim, Destination::Held(store), host, reader, reason).await;
    }
}

/// Storage opened once per process for refusal publication.
///
/// The refusing components (the resolver's serve loop, one-shot CLI reads)
/// hold no `JobStorage`, and opening one per refusal would put a backend
/// handshake on an error path that is already the slow path of a sick
/// fleet. One opening stands at a time and every caller waits on it, so a
/// store that does not answer is asked once, not once per refusal. Only
/// success is cached: a store that was unreachable during the outage must be
/// retried once it is back, which is the whole point.
static SHARED_STORE: OnceCell<JobStorage> = OnceCell::const_new();

async fn shared_store() -> Result<&'static JobStorage, crate::queue::StorageError> {
    SHARED_STORE.get_or_try_init(JobStorage::new).await
}

/// Publish one reader refusal about `host` without the caller standing
/// behind the write, for a caller that holds no [`JobStorage`].
///
/// Callers are refusing reads of a host that did not answer, and the fleet
/// store this opens can be served from that same host: a caller that
/// awaited the write would wait on that host a second time, and each one
/// added a write to the queue in front of it. The write is detached. A
/// one-shot command that exits first loses its copy, which the long-lived
/// resolver on the same machine records from its own process; a record that
/// cannot be written is said on stderr. The claim is taken before anything
/// is spawned, so a refusal already recorded or being written costs no
/// task.
///
pub fn report_refusal_detached(
    host: String,
    reader: &'static str,
    reason: &'static str,
    detail: String,
) {
    let Some(claim) = Claim::take(refusal_key(&host, reader, reason), &detail) else {
        return;
    };
    tokio::spawn(async move {
        publish(claim, Destination::Shared, &host, reader, reason).await;
    });
}
