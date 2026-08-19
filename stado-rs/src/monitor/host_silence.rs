//! Durable record of a host going quiet, and of the readers that refused
//! while it was quiet.
//!
//! NO Python original. The incident it exists for: on 2026-08-19 between
//! 18:29 and 18:35 UTC `charless-mac-mini` dropped off the tailnet — 100%
//! ping loss, ssh timing out, then `direct 10.0.0.253:41641` again with
//! 13-215 ms. Six minutes of a production host being unreachable, and
//! afterwards the product could not say it had happened. The beacon prefix
//! only ever holds the LATEST document per host, so the gap closed over
//! itself the moment the host came back: `host_health/<host>.json` was
//! fresh again and nothing anywhere remembered that it had been stale. The
//! only evidence that survived was an operator's two ping packets in a
//! terminal.
//!
//! The readers knew. The resolver refused resolutions with "service
//! directory cache is stale (store generation ...)" and its registry read
//! failed with "registry authority exited with ...: ssh: connect to host
//! ... Operation timed out" — both true, both timestamped, both written to
//! `~/.stado/logs/stado-resolver.err` and read by nobody. A refusal that
//! only a log file knows about is a refusal the product did not make.
//!
//! So two blob families, both append-only, both keyed by host:
//!
//! - `host_silence/<host>/<started_at>.json` — one record per gap, opened
//!   when the newest beacon crosses [`silence_threshold_seconds`] and
//!   closed by the first fresher beacon. `started_at` is the last moment
//!   the host was heard from, not the moment somebody noticed, so the
//!   duration is the outage rather than the polling interval.
//! - `reader_refusals/<host>/<at>.json` — one record per refusal, carrying
//!   the refusing component's own sentence VERBATIM in `detail`. A reader
//!   that rephrases the sentence it logged has invented a second vocabulary
//!   for one condition, and the operator then greps for a string that
//!   exists in no source file.
//!
//! `<host>` is the subject of the refusal, not the machine that refused:
//! the resolver on the laptop failing to reach the authority on the Mac
//! mini is evidence about the Mac mini, and it has to land where
//! `stado host link charless-mac-mini` will look for it.
//!
//! The joins and transitions are pure functions over already-loaded
//! documents ([`beacon_is_silent`], [`open_record`], [`merge_observation`],
//! [`close_record`], [`summarize_refusals`]) so the truth table is
//! exercisable without a store, a network, or a sick host.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::queue::{JobStorage, StorageError};

/// Blob prefix holding one record per silence, per host.
pub const SILENCE_PREFIX: &str = "host_silence";

/// Blob prefix holding one record per reader refusal, per host.
pub const REFUSAL_PREFIX: &str = "reader_refusals";

/// Seconds of beacon age that open a silence when no operator overrides
/// `STADO_SILENCE_THRESHOLD_SECONDS`.
///
/// Five minutes, because the fleet's beacons are published on a one-minute
/// timer: three consecutive misses is a host that has stopped talking, one
/// miss is a slow `pmset` call.
pub const DEFAULT_SILENCE_THRESHOLD_SECONDS: i64 = 300;

/// Environment override for [`silence_threshold_seconds`].
pub const SILENCE_THRESHOLD_ENV: &str = "STADO_SILENCE_THRESHOLD_SECONDS";

/// The resolver's cached service directory aged past `max_stale` and it
/// stopped answering resolutions. Its own sentence: "service directory
/// cache is stale (store generation ...)".
pub const REASON_DIRECTORY_CACHE_STALE: &str = "directory_cache_stale";

/// A registry read through the service-directory authority failed at the
/// transport. Its own sentence: "registry authority exited with ...".
pub const REASON_AUTHORITY_UNREACHABLE: &str = "authority_unreachable";

/// A reader found the newest beacon for a host older than the silence
/// threshold and refused to answer from it.
pub const REASON_BEACON_STALE: &str = "beacon_stale";

/// `reader` values, the three components that read fleet state.
pub const READER_RESOLVER: &str = "resolver";
/// See [`READER_RESOLVER`].
pub const READER_CLI: &str = "cli";
/// See [`READER_RESOLVER`].
pub const READER_DASHBOARD: &str = "dashboard";

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
const REFUSAL_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// One gap in a host's beacon publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SilenceRecord {
    /// Registry name of the host that went quiet.
    pub host: String,
    /// Last moment the host was heard from — the newest beacon that existed
    /// when the gap opened, or the moment of observation when the host has
    /// never published one.
    pub started_at: DateTime<Utc>,
    /// The fresher beacon that ended the gap; `None` while it is open.
    pub ended_at: Option<DateTime<Utc>>,
    /// `ended_at - started_at`; `None` while the gap is open.
    pub duration_seconds: Option<i64>,
    /// The first refusal sentence any reader produced during this gap,
    /// verbatim. Kept on the silence itself because it is the one line that
    /// tells an operator which subsystem noticed first.
    pub first_reader_error: Option<String>,
    /// Every component that observed this gap, in first-observation order.
    pub observed_by: Vec<String>,
}

/// One reader declining to answer, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalRecord {
    /// The host the refusal is ABOUT, which need not be the host that
    /// refused.
    pub host: String,
    /// When the reader refused.
    pub at: DateTime<Utc>,
    /// `resolver` / `cli` / `dashboard`.
    pub reader: String,
    /// Short stable token: [`REASON_DIRECTORY_CACHE_STALE`],
    /// [`REASON_AUTHORITY_UNREACHABLE`], [`REASON_BEACON_STALE`].
    pub reason: String,
    /// The refusing component's own sentence, verbatim.
    pub detail: String,
}

/// Refusals about one host over one window, counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalSummary {
    /// The window these counts cover, in seconds.
    pub window_seconds: i64,
    /// Refusals in the window.
    pub count: usize,
    /// Refusals in the window per `reason` token.
    pub reasons: BTreeMap<String, usize>,
}

impl RefusalSummary {
    /// An empty window — no refusals, no reasons.
    pub fn empty(window_seconds: i64) -> Self {
        Self {
            window_seconds,
            count: 0,
            reasons: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------

/// One path segment from a host name.
///
/// Registry names are already `[a-z0-9-]`, so for every real host this is
/// the identity. It exists for the one that is not: a name carrying `/`
/// would address a different directory, and the local backend would reject
/// the write as a path escape at the moment an operator most needs the
/// record. Everything outside `[A-Za-z0-9._-]` collapses to `-`; the
/// document keeps the host name verbatim regardless.
fn path_segment(host: &str) -> String {
    let mapped: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if mapped.is_empty() {
        "unknown".to_string()
    } else {
        mapped
    }
}

/// Compact UTC stamp used as a blob key.
///
/// Compact rather than RFC-3339 because the key is also a file name on the
/// local-file backend (same rule as `cli/service.rs`'s ensure audit), and
/// microsecond precision because two readers can open the same record in
/// the same second. Lexicographic order over these keys IS chronological
/// order, which is what lets the listing sort without downloading.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Directory holding every silence record for one host.
pub fn silence_prefix(host: &str) -> String {
    format!("{SILENCE_PREFIX}/{}/", path_segment(host))
}

/// Blob path of one silence record.
pub fn silence_object_path(host: &str, started_at: DateTime<Utc>) -> String {
    format!("{}{}.json", silence_prefix(host), stamp(started_at))
}

/// Directory holding every refusal record about one host.
pub fn refusal_prefix(host: &str) -> String {
    format!("{REFUSAL_PREFIX}/{}/", path_segment(host))
}

/// Blob path of one refusal record.
pub fn refusal_object_path(host: &str, at: DateTime<Utc>) -> String {
    format!("{}{}.json", refusal_prefix(host), stamp(at))
}

// ---------------------------------------------------------------------------
// pure transitions
// ---------------------------------------------------------------------------

/// Beacon age past which a host counts as silent.
///
/// The single reader of `STADO_SILENCE_THRESHOLD_SECONDS` in the crate. A
/// second parse elsewhere is how two commands come to disagree about
/// whether a host is down. Values that are not a positive integer fall back
/// to [`DEFAULT_SILENCE_THRESHOLD_SECONDS`] rather than disabling the
/// detector: a typo in a launchd plist must not silently switch off the
/// thing that notices outages.
pub fn silence_threshold_seconds() -> i64 {
    std::env::var(SILENCE_THRESHOLD_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_SILENCE_THRESHOLD_SECONDS)
}

/// Whether a host with this newest beacon counts as silent at `now`.
///
/// No beacon at all is silent: a host that has never published is not a
/// host that is fine. A beacon stamped in the future is NOT silent — clock
/// skew on the publisher is not an outage, and reporting it as one sends an
/// operator to the wrong machine.
pub fn beacon_is_silent(
    newest_beacon_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_seconds: i64,
) -> bool {
    match newest_beacon_at {
        None => true,
        Some(at) => (now - at).num_seconds() > threshold_seconds,
    }
}

/// Open a silence that began at `started_at`.
pub fn open_record(
    host: &str,
    started_at: DateTime<Utc>,
    observer: &str,
    first_reader_error: Option<&str>,
) -> SilenceRecord {
    SilenceRecord {
        host: host.to_string(),
        started_at,
        ended_at: None,
        duration_seconds: None,
        first_reader_error: first_reader_error.map(str::to_string),
        observed_by: vec![observer.to_string()],
    }
}

/// Fold one more observation into an open record; `true` when it changed.
///
/// `first_reader_error` is written once and never overwritten — the point
/// of the field is which subsystem noticed FIRST, and a later reader
/// clobbering it turns the record into a report of whoever ran most
/// recently.
pub fn merge_observation(
    record: &mut SilenceRecord,
    observer: &str,
    first_reader_error: Option<&str>,
) -> bool {
    let mut changed = false;
    if !record.observed_by.iter().any(|seen| seen == observer) {
        record.observed_by.push(observer.to_string());
        changed = true;
    }
    if record.first_reader_error.is_none() {
        if let Some(error) = first_reader_error {
            record.first_reader_error = Some(error.to_string());
            changed = true;
        }
    }
    changed
}

/// Close an open record at `ended_at`; `false` when it was already closed.
///
/// A close stamped before the open is clamped to a zero duration rather
/// than reported as negative time: the fresher beacon proves the host is
/// back, and the only thing a negative number would document is the skew
/// between two clocks.
pub fn close_record(record: &mut SilenceRecord, ended_at: DateTime<Utc>) -> bool {
    if record.ended_at.is_some() {
        return false;
    }
    record.ended_at = Some(ended_at);
    record.duration_seconds = Some((ended_at - record.started_at).num_seconds().max(0));
    true
}

/// Count refusals inside `window_seconds` back from `now`, per reason.
///
/// Records stamped in the future are counted: they are refusals that
/// happened, and dropping them because a publisher's clock runs fast would
/// hide exactly the fleet-wide condition this is for.
pub fn summarize_refusals(
    records: &[RefusalRecord],
    now: DateTime<Utc>,
    window_seconds: i64,
) -> RefusalSummary {
    let mut summary = RefusalSummary::empty(window_seconds);
    for record in records {
        if (now - record.at).num_seconds() > window_seconds {
            continue;
        }
        summary.count += 1;
        *summary.reasons.entry(record.reason.clone()).or_insert(0) += 1;
    }
    summary
}

// ---------------------------------------------------------------------------
// store reads
// ---------------------------------------------------------------------------

/// Blob paths under `prefix`, newest key first.
///
/// The key is a compact UTC stamp, so a reverse lexicographic sort is a
/// reverse chronological sort and the caller downloads only the documents
/// it is going to keep.
async fn newest_first(store: &JobStorage, prefix: &str) -> Result<Vec<String>, StorageError> {
    let mut paths = store.list_paths(prefix, 0).await?;
    paths.retain(|path| path.ends_with(".json"));
    paths.sort_unstable_by(|left, right| right.cmp(left));
    Ok(paths)
}

/// Parse one stored document, or `None` when it is absent or unreadable.
///
/// A corrupt record is skipped rather than propagated. These blobs are read
/// while something is already broken; refusing to report five silences
/// because one of them was truncated by a host that lost power mid-write is
/// the diagnostic failing for the same reason as its subject.
async fn read_document<T: serde::de::DeserializeOwned>(
    store: &JobStorage,
    path: &str,
) -> Result<Option<T>, StorageError> {
    let Some(body) = store.download_text(path).await? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&body).ok())
}

/// The newest `limit` silence records for `host`, newest first.
pub async fn recent_silences(
    store: &JobStorage,
    host: &str,
    limit: usize,
) -> Result<Vec<SilenceRecord>, StorageError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(limit);
    for path in newest_first(store, &silence_prefix(host)).await? {
        if let Some(record) = read_document::<SilenceRecord>(store, &path).await? {
            out.push(record);
            if out.len() == limit {
                break;
            }
        }
    }
    Ok(out)
}

/// The currently open silence for `host`, if the host is inside one.
///
/// Only the newest record can be open: a silence opens only when none is
/// open, and closing writes back to the same key. An older record still
/// carrying a null `ended_at` is a crashed writer, not a second live gap,
/// and is deliberately left alone rather than retro-closed with a time
/// nobody observed.
pub async fn open_silence(
    store: &JobStorage,
    host: &str,
) -> Result<Option<(String, SilenceRecord)>, StorageError> {
    let Some(path) = newest_first(store, &silence_prefix(host)).await?.into_iter().next() else {
        return Ok(None);
    };
    let Some(record) = read_document::<SilenceRecord>(store, &path).await? else {
        return Ok(None);
    };
    if record.ended_at.is_some() {
        return Ok(None);
    }
    Ok(Some((path, record)))
}

/// Every refusal about `host` inside `window_seconds` back from `now`,
/// newest first.
pub async fn recent_refusals_at(
    store: &JobStorage,
    host: &str,
    window_seconds: i64,
    now: DateTime<Utc>,
) -> Result<Vec<RefusalRecord>, StorageError> {
    let mut out = Vec::new();
    for path in newest_first(store, &refusal_prefix(host)).await? {
        let Some(record) = read_document::<RefusalRecord>(store, &path).await? else {
            continue;
        };
        // Keys sort chronologically, so the first record older than the
        // window ends the walk: nothing behind it can be newer.
        if (now - record.at).num_seconds() > window_seconds {
            break;
        }
        out.push(record);
    }
    Ok(out)
}

/// [`recent_refusals_at`] at the current instant.
pub async fn recent_refusals(
    store: &JobStorage,
    host: &str,
    window_seconds: i64,
) -> Result<Vec<RefusalRecord>, StorageError> {
    recent_refusals_at(store, host, window_seconds, Utc::now()).await
}

/// Refusal count and per-reason counts about `host` over a window.
pub async fn refusal_summary_at(
    store: &JobStorage,
    host: &str,
    window_seconds: i64,
    now: DateTime<Utc>,
) -> Result<RefusalSummary, StorageError> {
    let records = recent_refusals_at(store, host, window_seconds, now).await?;
    Ok(summarize_refusals(&records, now, window_seconds))
}

/// [`refusal_summary_at`] at the current instant.
pub async fn refusal_summary(
    store: &JobStorage,
    host: &str,
    window_seconds: i64,
) -> Result<RefusalSummary, StorageError> {
    refusal_summary_at(store, host, window_seconds, Utc::now()).await
}

// ---------------------------------------------------------------------------
// the transition, written by whoever notices
// ---------------------------------------------------------------------------

/// Fold one observation of a host's newest beacon into its silence record.
///
/// The single entry point for the open/close transition, called by whatever
/// component happens to look at a beacon: `stado host link`, the resolver,
/// the dashboard. Whichever one notices the threshold crossing writes it,
/// which is why the record is keyed by `started_at` and created
/// conditionally — three readers noticing the same outage produce one
/// record with three names in `observed_by`, not three records.
///
/// Returns the record this call wrote, or `None` when there was nothing to
/// write (host healthy and no gap open, or an open gap this observer had
/// already been recorded in).
pub async fn observe_beacon_age_at(
    store: &JobStorage,
    host: &str,
    newest_beacon_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    threshold_seconds: i64,
    observer: &str,
    first_reader_error: Option<&str>,
) -> Result<Option<SilenceRecord>, StorageError> {
    let existing = open_silence(store, host).await?;
    if beacon_is_silent(newest_beacon_at, now, threshold_seconds) {
        let Some((path, mut record)) = existing else {
            // A host that has never published has no last-heard-from
            // instant, so the gap starts now: claiming it started at the
            // epoch would report an outage nobody lived through.
            let started_at = newest_beacon_at.unwrap_or(now);
            let record = open_record(host, started_at, observer, first_reader_error);
            let path = silence_object_path(host, started_at);
            let body = serde_json::to_string_pretty(&record)?;
            if store.create_text_if_absent(&path, &body).await? {
                return Ok(Some(record));
            }
            // Lost the create race with another observer of the same
            // crossing. Their record is the record; merge into it rather
            // than overwrite, so neither name is lost.
            let Some(mut theirs) = read_document::<SilenceRecord>(store, &path).await? else {
                return Ok(None);
            };
            if !merge_observation(&mut theirs, observer, first_reader_error) {
                return Ok(None);
            }
            store
                .upload_text(&path, &serde_json::to_string_pretty(&theirs)?)
                .await?;
            return Ok(Some(theirs));
        };
        if !merge_observation(&mut record, observer, first_reader_error) {
            return Ok(None);
        }
        store
            .upload_text(&path, &serde_json::to_string_pretty(&record)?)
            .await?;
        return Ok(Some(record));
    }
    let Some((path, mut record)) = existing else {
        return Ok(None);
    };
    // The gap ended when the host published again, not when somebody got
    // around to looking: `ended_at` is the fresher beacon's own instant, so
    // `duration_seconds` is the outage and not the polling interval.
    let ended_at = newest_beacon_at.unwrap_or(now);
    merge_observation(&mut record, observer, first_reader_error);
    if !close_record(&mut record, ended_at) {
        return Ok(None);
    }
    store
        .upload_text(&path, &serde_json::to_string_pretty(&record)?)
        .await?;
    Ok(Some(record))
}

/// [`observe_beacon_age_at`] at the current instant, with the fleet-wide
/// threshold from [`silence_threshold_seconds`].
pub async fn observe_beacon_age(
    store: &JobStorage,
    host: &str,
    newest_beacon_at: Option<DateTime<Utc>>,
    observer: &str,
    first_reader_error: Option<&str>,
) -> Result<Option<SilenceRecord>, StorageError> {
    observe_beacon_age_at(
        store,
        host,
        newest_beacon_at,
        Utc::now(),
        silence_threshold_seconds(),
        observer,
        first_reader_error,
    )
    .await
}

// ---------------------------------------------------------------------------
// refusals, published best effort
// ---------------------------------------------------------------------------

static REFUSAL_THROTTLE: LazyLock<Mutex<HashMap<(String, String, String), Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
/// [`REFUSAL_WRITE_TIMEOUT`] per write.
///
/// `detail` is the component's own sentence and is stored verbatim.
pub async fn record_refusal(store: &JobStorage, host: &str, reader: &str, reason: &str, detail: &str) {
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
    let _ = tokio::time::timeout(REFUSAL_WRITE_TIMEOUT, store.upload_text(&path, &body)).await;
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
    let _ = tokio::time::timeout(REFUSAL_WRITE_TIMEOUT, async move {
        let store = shared_store().await?;
        store.upload_text(&path, &body).await.ok()
    })
    .await;
}

/// [`report_refusal`] for a caller on a request path.
///
/// The resolver refuses EVERY resolution while its cache is stale, and a
/// resolution is something a workload is blocking on. Making each of those
/// refusals wait up to [`REFUSAL_WRITE_TIMEOUT`] for a blob write would
/// convert a fast, correct refusal into a client timeout — the diagnostic
/// changing the behaviour it was added to explain. The write is detached
/// instead, which is right for a long-lived service and wrong for a
/// one-shot command: a command that exits immediately after must
/// `await` [`report_refusal`], or the process is gone before the task runs.
///
/// Requires a Tokio runtime, which every caller of this already has.
pub fn report_refusal_detached(host: String, reader: &'static str, reason: &'static str, detail: String) {
    tokio::spawn(async move {
        report_refusal(&host, reader, reason, &detail).await;
    });
}
