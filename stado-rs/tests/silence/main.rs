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
//! `stado host link`, the resolver and the desktop console all read.
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
    let blob = "host_silence/control-host/20260819T182900.000000Z.json";

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
        blob_names(dir.path(), "host_silence/control-host").is_empty(),
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
    assert_eq!(merged.first_reader_error.as_deref(), Some(AUTHORITY_SENTENCE));
    assert_eq!(
        blob_names(dir.path(), "host_silence/control-host"),
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
        blob_names(dir.path(), "host_silence/control-host"),
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

    let opened = observe_beacon_age_at(
        &store,
        "gpu-host",
        None,
        now,
        300,
        READER_CLI,
        None,
    )
    .await
    .unwrap()
    .expect("no beacon at all is a silence");

    // Not the epoch: the product may not report an outage nobody lived
    // through just because it has no earlier evidence.
    assert_eq!(opened.started_at, now);
    assert_eq!(
        on_disk(
            dir.path(),
            "host_silence/gpu-host/20260819T183430.000000Z.json"
        )["started_at"],
        "2026-08-19T18:34:30Z"
    );
}

#[test]
fn the_transition_truth_table_needs_no_store() {
    let beacon = at("2026-08-19T18:29:00Z");

    assert!(!beacon_is_silent(Some(beacon), at("2026-08-19T18:33:59Z"), 300));
    assert!(beacon_is_silent(Some(beacon), at("2026-08-19T18:34:01Z"), 300));
    assert!(
        beacon_is_silent(None, at("2026-08-19T18:34:01Z"), 300),
        "a host that never published is not a host that is fine"
    );
    assert!(
        !beacon_is_silent(Some(at("2026-08-19T19:00:00Z")), beacon, 300),
        "a publisher with a fast clock is not an outage"
    );

    // A close stamped before the open reports zero, never negative time.
    let mut skewed = open_record(HOST, beacon, READER_CLI, None);
    assert!(close_record(&mut skewed, at("2026-08-19T18:28:00Z")));
    assert_eq!(skewed.duration_seconds, Some(0));
    assert!(
        !close_record(&mut skewed, at("2026-08-19T18:40:00Z")),
        "a closed record does not close twice"
    );

    let mut record = open_record(HOST, beacon, READER_RESOLVER, None);
    assert!(merge_observation(&mut record, READER_CLI, Some("first")));
    assert!(!merge_observation(&mut record, READER_CLI, Some("second")));
    assert_eq!(
        record.first_reader_error.as_deref(),
        Some("first"),
        "the field records who noticed first, not who ran last"
    );
    assert_eq!(record.observed_by, vec!["resolver", "cli"]);
}

/// Held across every `set_var` and every subprocess spawn in this binary.
///
/// `#[test]` functions share one process and run on parallel threads, and
/// `setenv(3)` concurrent with the `environ` walk `Command::spawn` does is a
/// data race in libc, not in Rust — it aborts rather than failing an
/// assertion, at whatever rate the scheduler feels like. These are the only
/// two tests here that touch the process environment.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn the_threshold_is_read_from_one_place() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        silence_threshold_seconds(),
        DEFAULT_SILENCE_THRESHOLD_SECONDS
    );
    std::env::set_var(SILENCE_THRESHOLD_ENV, "45");
    assert_eq!(silence_threshold_seconds(), 45);
    // A typo must not switch the detector off.
    std::env::set_var(SILENCE_THRESHOLD_ENV, "not a number");
    assert_eq!(
        silence_threshold_seconds(),
        DEFAULT_SILENCE_THRESHOLD_SECONDS
    );
    std::env::set_var(SILENCE_THRESHOLD_ENV, "0");
    assert_eq!(
        silence_threshold_seconds(),
        DEFAULT_SILENCE_THRESHOLD_SECONDS
    );
    std::env::remove_var(SILENCE_THRESHOLD_ENV);
}

// ---------------------------------------------------------------------------
// aggregation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn refusals_aggregate_per_reason_over_a_window() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = at("2026-08-19T18:35:00Z");

    // Four refusals during the outage, two readers, two reasons — plus one
    // from an hour earlier that a 15-minute window must not count.
    for record in [
        refusal(
            "2026-08-19T17:20:00Z",
            READER_RESOLVER,
            REASON_AUTHORITY_UNREACHABLE,
            "registry authority exited with exit status: 255: ssh: connect to host 10.0.0.253 port 22: Operation timed out",
        ),
        refusal(
            "2026-08-19T18:30:11Z",
            READER_RESOLVER,
            REASON_AUTHORITY_UNREACHABLE,
            AUTHORITY_SENTENCE,
        ),
        refusal(
            "2026-08-19T18:31:11Z",
            READER_RESOLVER,
            REASON_DIRECTORY_CACHE_STALE,
            STALE_SENTENCE,
        ),
        refusal(
            "2026-08-19T18:32:11Z",
            READER_RESOLVER,
            REASON_DIRECTORY_CACHE_STALE,
            STALE_SENTENCE,
        ),
        refusal(
            "2026-08-19T18:33:41Z",
            READER_CLI,
            REASON_BEACON_STALE,
            "beacon for control-host is 281s old",
        ),
    ] {
        seed_refusal(&store, &record).await;
    }

    // The path a reader writes is the path an aggregator reads.
    assert_eq!(
        blob_names(dir.path(), "reader_refusals/control-host"),
        vec![
            "20260819T172000.000000Z.json",
            "20260819T183011.000000Z.json",
            "20260819T183111.000000Z.json",
            "20260819T183211.000000Z.json",
            "20260819T183341.000000Z.json",
        ]
    );
    assert_eq!(
        on_disk(
            dir.path(),
            "reader_refusals/control-host/20260819T183011.000000Z.json"
        ),
        json!({
            "host": "control-host",
            "at": "2026-08-19T18:30:11Z",
            "reader": "resolver",
            "reason": "authority_unreachable",
            "detail": AUTHORITY_SENTENCE,
        })
    );

    let summary = refusal_summary_at(&store, HOST, 900, now).await.unwrap();
    assert_eq!(summary.window_seconds, 900);
    assert_eq!(summary.count, 4, "the 17:20 refusal is outside the window");
    assert_eq!(
        summary.reasons,
        [
            ("authority_unreachable".to_string(), 1),
            ("beacon_stale".to_string(), 1),
            ("directory_cache_stale".to_string(), 2),
        ]
        .into_iter()
        .collect()
    );

    // Widen the window and the older one joins its own reason's count.
    let wide = refusal_summary_at(&store, HOST, 7200, now).await.unwrap();
    assert_eq!(wide.count, 5);
    assert_eq!(wide.reasons["authority_unreachable"], 2);

    // Newest first, and the walk stops at the window edge.
    let listed = recent_refusals_at(&store, HOST, 900, now).await.unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0].at, at("2026-08-19T18:33:41Z"));
    assert_eq!(listed[0].detail, "beacon for control-host is 281s old");
    assert_eq!(listed[3].at, at("2026-08-19T18:30:11Z"));

    // A host nobody refused about answers zero, not an error.
    let none = refusal_summary_at(&store, "operator-host", 900, now)
        .await
        .unwrap();
    assert_eq!(none.count, 0);
    assert!(none.reasons.is_empty());

    // Same counting rule, no store involved.
    assert_eq!(summarize_refusals(&listed, now, 900), summary);
}

#[tokio::test]
async fn recent_silences_returns_the_newest_five_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    // Seven gaps across the day, written in the order they happened.
    for hour in 10..17 {
        let started = at(&format!("2026-08-19T{hour:02}:29:00Z"));
        observe_beacon_age_at(
            &store,
            HOST,
            Some(started),
            started + chrono::Duration::seconds(400),
            300,
            READER_CLI,
            None,
        )
        .await
        .unwrap()
        .expect("each gap opens");
        observe_beacon_age_at(
            &store,
            HOST,
            Some(started + chrono::Duration::seconds(420)),
            started + chrono::Duration::seconds(500),
            300,
            READER_CLI,
            None,
        )
        .await
        .unwrap()
        .expect("each gap closes");
    }
    assert_eq!(
        blob_names(dir.path(), "host_silence/control-host").len(),
        7
    );

    let newest = recent_silences(&store, HOST, 5).await.unwrap();
    assert_eq!(newest.len(), 5);
    let hours: Vec<u32> = newest
        .iter()
        .map(|record| record.started_at.format("%H").to_string().parse().unwrap())
        .collect();
    assert_eq!(hours, vec![16, 15, 14, 13, 12]);
    assert!(newest.iter().all(|r| r.duration_seconds == Some(420)));

    assert!(recent_silences(&store, HOST, 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_published_refusal_is_bounded_and_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    record_refusal(
        &store,
        "silence-bounded-host",
        READER_RESOLVER,
        REASON_DIRECTORY_CACHE_STALE,
        STALE_SENTENCE,
    )
    .await;
    // The same refusal again inside the throttle window writes nothing: a
    // resolver refuses every request while its cache is stale, and the count
    // an operator reads must measure the fault, not the request volume.
    record_refusal(
        &store,
        "silence-bounded-host",
        READER_RESOLVER,
        REASON_DIRECTORY_CACHE_STALE,
        STALE_SENTENCE,
    )
    .await;

    let names = blob_names(dir.path(), "reader_refusals/silence-bounded-host");
    assert_eq!(names.len(), 1, "the throttle let a duplicate through: {names:?}");
    let record = on_disk(
        dir.path(),
        &format!("reader_refusals/silence-bounded-host/{}", names[0]),
    );
    assert_eq!(record["reader"], "resolver");
    assert_eq!(record["reason"], "directory_cache_stale");
    assert_eq!(
        record["detail"], STALE_SENTENCE,
        "the component's own sentence is stored verbatim"
    );

    // A different reason about the same host is a different refusal.
    record_refusal(
        &store,
        "silence-bounded-host",
        READER_RESOLVER,
        REASON_AUTHORITY_UNREACHABLE,
        AUTHORITY_SENTENCE,
    )
    .await;
    assert_eq!(
        blob_names(dir.path(), "reader_refusals/silence-bounded-host").len(),
        2
    );
}

// ---------------------------------------------------------------------------
// end to end, through the binary
// ---------------------------------------------------------------------------

/// This machine's kernel hostname, normalized the way the registry
/// validator demands ("must be normalized as '<lowercase>'").
fn hostname() -> String {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let out = Command::new("hostname").output().expect("hostname(1) runs");
    String::from_utf8_lossy(&out.stdout).trim().to_lowercase()
}

fn spawn_stado(storage: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        // The resolver reads its ssh key, its control sockets and its
        // socket-reaper directory out of $HOME/.stado. Pointed at the
        // tempdir the ssh call is hermetic AND the reaper cannot reach the
        // live resolver's control sockets on the operator's machine — it
        // dropped one during the first hand probe of this very test.
        .env("HOME", storage)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("STADO_RESOLVER_SSH_KEY_FILE")
        .output()
}

fn stado(storage: &Path, args: &[&str]) -> Output {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    spawn_stado(storage, args).expect("stado binary runs")
}

#[test]
fn an_unreachable_authority_publishes_its_own_sentence_as_a_refusal() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path();
    // This machine has to be a registry target for the resolver to know who
    // it is; the authority is a name RFC 2606 reserves so it can never
    // resolve and no packet leaves the machine.
    let document = json!({
        "schema_version": 2,
        "targets": [
            {
                "name": "silence-test-local",
                "kind": "local",
                "release_platform": "darwin-arm64",
                "hostnames": [hostname()],
                "slots": 1
            },
            {
                "name": "silence-test-authority",
                "kind": "local",
                "ssh": "stado@silence-test-authority.invalid",
                "release_platform": "darwin-arm64",
                "slots": 1,
                "services": [
                    {"name": "brama", "kind": "launchd", "path": "/opt/stado/brama.plist"}
                ]
            }
        ],
        "coordinators": [],
        "service_directory": {
            "authority": {
                "target": "silence-test-authority",
                "command": "/opt/stado/bin/stado"
            },
            "generation": 7,
            "services": {
                "brama": {
                    "managed_service": "brama",
                    "active_host": "silence-test-authority",
                    "endpoints": {
                        "silence-test-authority": {"url": "http://127.0.0.1:8080"}
                    },
                    "consumers": {"lem": {"capabilities": ["model-routing"]}}
                }
            }
        }
    });
    std::fs::write(
        storage.join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();

    let out = stado(storage, &["resolver", "resolve", "brama", "--consumer", "lem"]);
    assert!(
        !out.status.success(),
        "an unreachable authority resolved: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let printed = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        printed.contains("registry authority"),
        "the command did not reach the authority read: {printed}"
    );

    let names = blob_names(storage, "reader_refusals/silence-test-authority");
    assert_eq!(
        names.len(),
        1,
        "the failed read published no refusal (stderr: {printed})"
    );
    let record = on_disk(
        storage,
        &format!("reader_refusals/silence-test-authority/{}", names[0]),
    );
    assert_eq!(
        record["host"], "silence-test-authority",
        "the refusal is filed under the host it is evidence about, not the \
         machine that noticed"
    );
    assert_eq!(record["reader"], "cli");
    assert_eq!(record["reason"], "authority_unreachable");
    let detail = record["detail"].as_str().expect("detail is a string");
    assert!(
        printed.contains(detail),
        "the stored detail is not the sentence the command printed:\n  stored: {detail}\n  printed: {printed}"
    );
}
