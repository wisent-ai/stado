//! Host silence records and reader refusals against the local backend.
//!
//! The incident under test, replayed with its own numbers: on 2026-08-19
//! `control-host` stopped answering at 18:29 UTC and came back at
//! 18:35. Six minutes during which the beacon prefix — which holds only the
//! LATEST document per host — quietly closed over the gap, and the two
//! readers that did notice wrote their refusals to
//! `~/.stado/logs/stado-resolver.err`. Afterwards nothing in the product
//! could say the outage had happened.
//!
//! Storage is a `tempfile::TempDir` behind `stado::queue::LocalBackend`, so
//! the assertions are against real blobs on a real disk and the operator's
//! registry, vault and running services are never touched. Blob paths are
//! asserted as literal file names because those paths are the contract that
//! `stado host link`, the resolver and the desktop console all read — and
//! because the first cut of them was unwritable: the object API rejected
//! every `host_silence/...` key with `401 unauthorized or non-immutable
//! release write`, so the records live under the authorized, canonical
//! `state/` root and a literal assertion is what keeps them there.
//!
//! The last test drives the built binary (`CARGO_BIN_EXE_stado`) end to end
//! with WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> and a
//! STADO_CONFIG pointing at a nonexistent file: it makes the real
//! `stado resolver resolve` fail against an authority whose name cannot
//! resolve, and proves the refusal it publishes carries the same sentence
//! the command printed. The `.invalid` TLD is reserved by RFC 2606 and
//! never resolves, so no packet leaves the machine.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use stado::monitor::host_silence::{
    beacon_is_silent, close_record, merge_observation, observe_beacon_age_at, open_record,
    recent_refusals_at, recent_silences, record_refusal, refusal_object_path, refusal_summary_at,
    silence_object_path, silence_threshold_seconds, summarize_refusals, RefusalRecord,
    DEFAULT_SILENCE_THRESHOLD_SECONDS, READER_CLI, READER_DASHBOARD, READER_RESOLVER,
    REASON_AUTHORITY_UNREACHABLE, REASON_BEACON_STALE, REASON_DIRECTORY_CACHE_STALE,
    SILENCE_THRESHOLD_ENV,
};
use stado::queue::{JobStorage, LocalBackend};

const HOST: &str = "control-host";

/// The resolver's own sentence from the incident, verbatim.
const AUTHORITY_SENTENCE: &str = "registry authority exited with exit status: 255: ssh: connect to host 10.0.0.253 port 22: Operation timed out";

/// The other reader's own sentence from the incident, verbatim.
const STALE_SENTENCE: &str = "service directory cache is stale (store generation 7)";

fn store(root: &Path) -> JobStorage {
    let backend = LocalBackend::new(root.to_str().expect("tempdir path is utf-8"))
        .expect("local backend roots at the tempdir");
    JobStorage::with_backend(Arc::new(backend), "local")
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("fixture timestamp is RFC 3339")
        .with_timezone(&Utc)
}

/// The document actually on disk at `blob`, parsed.
fn on_disk(root: &Path, blob: &str) -> Value {
    let body = std::fs::read_to_string(root.join(blob))
        .unwrap_or_else(|error| panic!("{blob} is not on disk: {error}"));
    serde_json::from_str(&body).expect("the record stays JSON")
}

/// Blob names directly under `dir`, sorted.
fn blob_names(root: &Path, dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Seed one refusal at an exact instant, the way a reader publishes it.
///
/// `record_refusal` stamps `Utc::now()` and throttles, which is right for a
/// live reader and useless for building a window with known ages; the path
/// and body here are the ones it would have written.
async fn seed_refusal(store: &JobStorage, record: &RefusalRecord) {
    store
        .upload_text(
            &refusal_object_path(&record.host, record.at),
            &serde_json::to_string_pretty(record).expect("refusal serializes"),
        )
        .await
        .expect("seeding a refusal writes");
}

fn refusal(at_text: &str, reader: &str, reason: &str, detail: &str) -> RefusalRecord {
    RefusalRecord {
        host: HOST.to_string(),
        at: at(at_text),
        reader: reader.to_string(),
        reason: reason.to_string(),
        detail: detail.to_string(),
    }
}

// ---------------------------------------------------------------------------
// the transition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_silence_opens_on_the_crossing_and_closes_on_the_fresher_beacon() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    // 18:29:00 is the last beacon the Mac mini published before it went.
    let last_beacon = at("2026-08-19T18:29:00Z");
    let blob = "state/host_silence/control-host/20260819T182900.000000Z.json";

    // 18:31 — two minutes of quiet. Inside the 300s threshold, so nothing is
    // recorded: one missed publication is not an outage.
    let quiet = observe_beacon_age_at(
        &store,
        HOST,
        Some(last_beacon),
        at("2026-08-19T18:31:00Z"),
        300,
        READER_CLI,
        None,
    )
    .await
    .unwrap();
    assert!(quiet.is_none(), "a two-minute gap wrote {quiet:?}");
    assert!(
        blob_names(dir.path(), "state/host_silence/control-host").is_empty(),
        "a two-minute gap left a blob behind"
    );

    // 18:34:30 — 330 seconds. The resolver crosses the threshold first, and
    // it arrives carrying the sentence it was already logging.
    let opened = observe_beacon_age_at(
        &store,
        HOST,
        Some(last_beacon),
        at("2026-08-19T18:34:30Z"),
        300,
        READER_RESOLVER,
        Some(AUTHORITY_SENTENCE),
    )
    .await
    .unwrap()
    .expect("the crossing opens a silence");
    assert_eq!(opened.started_at, last_beacon);

    // The record is keyed by when the host was last heard from, not by when
    // somebody noticed, and it says so on disk.
    assert_eq!(silence_object_path(HOST, last_beacon), blob);
    assert_eq!(
        on_disk(dir.path(), blob),
        json!({
            "host": "control-host",
            "started_at": "2026-08-19T18:29:00Z",
            "ended_at": null,
            "duration_seconds": null,
            "first_reader_error": AUTHORITY_SENTENCE,
            "observed_by": ["resolver"],
        })
    );

    // A second reader of the same gap joins the record instead of opening a
    // rival one, and does NOT overwrite whose error came first.
    let merged = observe_beacon_age_at(
        &store,
        HOST,
        Some(last_beacon),
        at("2026-08-19T18:34:50Z"),
        300,
        READER_CLI,
        Some("beacon for control-host is 350s old"),
    )
    .await
    .unwrap()
    .expect("a new observer updates the open record");
    assert_eq!(merged.observed_by, vec!["resolver", "cli"]);
    assert_eq!(
        merged.first_reader_error.as_deref(),
        Some(AUTHORITY_SENTENCE)
    );
    assert_eq!(
        blob_names(dir.path(), "state/host_silence/control-host"),
        vec!["20260819T182900.000000Z.json"],
        "the second observer opened a second record"
    );

    // The same reader looking again changes nothing and writes nothing.
    let repeat = observe_beacon_age_at(
        &store,
        HOST,
        Some(last_beacon),
        at("2026-08-19T18:35:00Z"),
        300,
        READER_CLI,
        None,
    )
    .await
    .unwrap();
    assert!(repeat.is_none(), "a repeat observation wrote {repeat:?}");

    // 18:35:12 — the host publishes again. That beacon, not the moment
    // anybody looked, is when the silence ended.
    let closed = observe_beacon_age_at(
        &store,
        HOST,
        Some(at("2026-08-19T18:35:12Z")),
        at("2026-08-19T18:40:00Z"),
        300,
        READER_DASHBOARD,
        None,
    )
    .await
    .unwrap()
    .expect("a fresher beacon closes the silence");
    assert_eq!(closed.duration_seconds, Some(372));

    assert_eq!(
        on_disk(dir.path(), blob),
        json!({
            "host": "control-host",
            "started_at": "2026-08-19T18:29:00Z",
            "ended_at": "2026-08-19T18:35:12Z",
            "duration_seconds": 372,
            "first_reader_error": AUTHORITY_SENTENCE,
            "observed_by": ["resolver", "cli", "dashboard"],
        })
    );
    assert_eq!(
        blob_names(dir.path(), "state/host_silence/control-host"),
        vec!["20260819T182900.000000Z.json"],
        "the whole outage is one record"
    );

    // A closed gap is not reopened by looking at it again.
    let after = observe_beacon_age_at(
        &store,
        HOST,
        Some(at("2026-08-19T18:36:12Z")),
        at("2026-08-19T18:41:00Z"),
        300,
        READER_CLI,
        None,
    )
    .await
    .unwrap();
    assert!(after.is_none(), "a healthy host wrote {after:?}");

    let recent = recent_silences(&store, HOST, 5).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].duration_seconds, Some(372));
}

#[tokio::test]
async fn a_host_that_never_published_starts_its_silence_at_the_observation() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = at("2026-08-19T18:34:30Z");

    let opened = observe_beacon_age_at(&store, "gpu-host", None, now, 300, READER_CLI, None)
        .await
        .unwrap()
        .expect("no beacon at all is a silence");

    // Not the epoch: the product may not report an outage nobody lived
    // through just because it has no earlier evidence.
    assert_eq!(opened.started_at, now);
    assert_eq!(
        on_disk(
            dir.path(),
            "state/host_silence/gpu-host/20260819T183430.000000Z.json"
        )["started_at"],
        "2026-08-19T18:34:30Z"
    );
}

/// Held across every `set_var` and every subprocess spawn in this binary.
///
/// `#[test]` functions share one process and run on parallel threads, and
/// `setenv(3)` concurrent with the `environ` walk `Command::spawn` does is a
/// data race in libc, not in Rust — it aborts rather than failing an
/// assertion, at whatever rate the scheduler feels like. These are the only
/// two tests here that touch the process environment.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

mod authority;
mod threshold;
mod truth_table;
