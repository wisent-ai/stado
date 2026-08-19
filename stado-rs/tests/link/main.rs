//! `stado host link` against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test. The registry, the beacon, the silence records and the refusal
//! records all live and die inside the temp dir; nothing here touches the
//! operator's real registry, vault, or hosts.
//!
//! What is being defended: a connectivity gap used to leave no trace in this
//! product. control-host answered no ping and no ssh from 18:29 to 18:35
//! UTC on 2026-08-19 and came back on `direct 10.0.0.253:41641`; the only
//! evidence was two ping packets an operator sent by hand, and the refusals it
//! caused went to a log file nobody reads. These tests pin the three verdicts
//! this command answers with, the sentences it names them in, and the silence
//! record it leaves behind on disk.

use std::path::Path;
use std::process::{Command, Output};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde_json::Value;

/// The silence threshold is pinned rather than defaulted: the fleet default is
/// 300 seconds, and a test that read the operator's environment for it would
/// change verdict on the machine it ran on.
const THRESHOLD_SECONDS: &str = "300";

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("STADO_SILENCE_THRESHOLD_SECONDS", THRESHOLD_SECONDS)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `--json` document, parsed.
fn document(out: &Output) -> Value {
    serde_json::from_str(&stdout(out)).expect("host link --json prints one JSON document")
}

const REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "h1",
            "kind": "local",
            "ssh": "u@127.0.0.1",
            "release_platform": "darwin-arm64",
            "hostnames": ["h1.local"],
            "slots": 1
        }
    ],
    "coordinators": []
}"#;

/// A temp storage root carrying exactly this registry document.
fn storage_with_registry() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("registry.json"), REGISTRY).unwrap();
    dir
}

/// Write one blob at the exact path the contract gives it.
///
/// The paths are spelled out here rather than taken from the product's own
/// helpers: `host_health/<slug>.json`, `host_silence/<host>/<started_at>.json`
/// and `reader_refusals/<host>/<at>.json` ARE the contract, and a test that
/// asked the code under test where to write would pin nothing.
fn write_blob(storage: &Path, path: &str, body: &str) {
    let full = storage.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

/// Blob-key spelling of an instant: compact, microsecond, UTC, so that
/// lexicographic order over the keys is chronological order.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Beacon-document spelling of an instant, to the second, as the beacon
/// writers emit it.
fn beacon_time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Every silence record on disk for `host`, oldest key first.
fn silences_on_disk(storage: &Path, host: &str) -> Vec<Value> {
    let dir = storage.join("host_silence").join(host);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{} is unreadable: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap())
                .expect("a silence record stays JSON")
        })
        .collect()
}

/// The `link` block a host publishes when its collector could read everything.
fn link_block(collected_at: &str) -> String {
    format!(
        r#"{{
            "collected_at": "{collected_at}",
            "path_kind": "direct",
            "endpoint": "10.0.0.253:41641",
            "last_sleep_at": "2026-08-19T18:28:55Z",
            "last_wake_at": "2026-08-19T18:35:02Z",
            "interface_changes": [
                {{"at": "2026-08-19T18:29:01Z", "detail": "en0 link down"}},
                {{"at": "2026-08-19T18:35:04Z", "detail": "en0 link up"}}
            ],
            "source": "pmset+tailscale"
        }}"#
    )
}

#[test]
fn host_link_reports_the_published_link_and_closes_the_open_silence() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    let reported_at = beacon_time(now - TimeDelta::seconds(30));

    write_blob(
        storage,
        "host_health/h1.json",
        &format!(
            r#"{{"host": "h1", "reported_at": "{reported_at}", "link": {}}}"#,
            link_block(&reported_at)
        ),
    );
    // A gap somebody else opened half an hour ago and nobody closed. The
    // fresher beacon is what ends it, and this command is what notices.
    let opened = now - TimeDelta::seconds(1800);
    write_blob(
        storage,
        &format!("host_silence/h1/{}.json", stamp(opened)),
        &format!(
            r#"{{"host": "h1", "started_at": "{}", "ended_at": null, "duration_seconds": null,
                  "first_reader_error": "service directory cache is stale",
                  "observed_by": ["resolver"]}}"#,
            beacon_time(opened)
        ),
    );

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a fresh beacon with nothing refused is healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["host"], "h1");
    assert_eq!(report["verdict"], "healthy");
    // The published block, field for field: this is what the operator read over
    // ssh by hand on 2026-08-19 and could not read here.
    assert_eq!(report["path_kind"], "direct");
    assert_eq!(report["endpoint"], "10.0.0.253:41641");
    assert_eq!(report["last_sleep_at"], "2026-08-19T18:28:55Z");
    assert_eq!(report["last_wake_at"], "2026-08-19T18:35:02Z");
    assert_eq!(
        report["interface_changes"],
        serde_json::json!([
            {"at": "2026-08-19T18:29:01Z", "detail": "en0 link down"},
            {"at": "2026-08-19T18:35:04Z", "detail": "en0 link up"}
        ])
    );
    assert_eq!(report["reader_refusals"]["count"], 0);
    assert_eq!(report["reader_refusals"]["window_seconds"], 3600);
    assert_eq!(report["reader_refusals"]["reasons"], serde_json::json!({}));
    assert!(
        report["beacon_age_seconds"].as_i64().unwrap() < 300,
        "the seeded beacon is inside the threshold, got: {}",
        report["beacon_age_seconds"]
    );

    // The state that matters is on disk: the gap is closed at the instant the
    // host published again, not at the instant somebody looked, so the duration
    // is the outage and not the polling interval.
    let records = silences_on_disk(storage, "h1");
    assert_eq!(records.len(), 1, "one gap, closed, not a second record");
    assert_eq!(records[0]["ended_at"], reported_at.as_str());
    assert_eq!(records[0]["duration_seconds"], 1770);
    assert_eq!(
        records[0]["observed_by"],
        serde_json::json!(["resolver", "cli"]),
        "the reader that closed the gap is named beside the one that opened it"
    );
    assert_eq!(
        records[0]["first_reader_error"],
        "service directory cache is stale",
        "closing a gap never rewrites what the first reader said about it"
    );
    // The closed record is the one the document carried.
    assert_eq!(report["silences"].as_array().unwrap().len(), 1);
    assert_eq!(report["silences"][0]["duration_seconds"], 1770);

    // The report form prints the same facts, in the shape `host gates` uses.
    let out = stado(storage, &["host", "link", "h1"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    for line in [
        "host:     h1",
        "verdict:  healthy",
        "path:     direct via 10.0.0.253:41641",
        "sleep:    last slept 2026-08-19T18:28:55Z, last woke 2026-08-19T18:35:02Z",
        "changes:  2 recorded",
        "          2026-08-19T18:29:01Z en0 link down",
        "refusals: none in the last 3600s",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

#[test]
fn host_link_calls_a_stale_host_silent_and_records_the_gap() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    // Past the 300-second threshold, and publishing the older beacon that
    // carries no link block at all.
    let reported_at = beacon_time(now - TimeDelta::seconds(900));
    write_blob(
        storage,
        "host_health/h1.json",
        &format!(r#"{{"host": "h1", "reported_at": "{reported_at}"}}"#),
    );

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a verdict that is not healthy exits 1: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["verdict"], "silent");
    assert_eq!(report["ssh_reachable"], false);
    // A missing link block is nulls and a named blocker, never a fabricated
    // path: "we do not know how this host is reachable" is the answer that
    // sends an operator to look.
    assert_eq!(report["path_kind"], "unknown");
    assert_eq!(report["endpoint"], Value::Null);
    assert_eq!(report["last_sleep_at"], Value::Null);
    assert_eq!(report["last_wake_at"], Value::Null);
    assert_eq!(report["interface_changes"], serde_json::json!([]));

    let blockers: Vec<&str> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker.as_str().unwrap())
        .collect();
    assert!(
        blockers.iter().any(|blocker| blocker
            .ends_with("s old, past the 300s silence threshold")
            && blocker.starts_with("this host's newest beacon is")),
        "the staleness blocker names the age and the threshold, got: {blockers:?}"
    );
    assert!(
        blockers.contains(
            &"this host's beacon carries no link block, so its path, its sleep and wake \
              times and its interface changes are unknown here"
        ),
        "the missing link block is named in the component's own words, got: {blockers:?}"
    );
    assert!(
        stderr(&out).contains("h1 link verdict is silent, with"),
        "the refusal names the verdict, got: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("blocker(s) named in the report above"),
        "got: {}",
        stderr(&out)
    );

    // The gap this command noticed is now on disk, keyed by the instant the
    // host was last heard from — which is the whole point: on 2026-08-19 the
    // six minutes control-host spent unreachable were recorded nowhere.
    let records = silences_on_disk(storage, "h1");
    assert_eq!(records.len(), 1, "one open record, got: {records:?}");
    assert_eq!(records[0]["host"], "h1");
    assert_eq!(records[0]["started_at"], reported_at.as_str());
    assert_eq!(records[0]["ended_at"], Value::Null);
    assert_eq!(records[0]["duration_seconds"], Value::Null);
    assert_eq!(records[0]["observed_by"], serde_json::json!(["cli"]));

    // Looking twice records one gap with one observer, not two records.
    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(silences_on_disk(storage, "h1").len(), 1);
}

#[test]
fn host_link_calls_a_host_degraded_when_readers_refused() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    let reported_at = beacon_time(now - TimeDelta::seconds(20));
    write_blob(
        storage,
        "host_health/h1.json",
        &format!(r#"{{"host": "h1", "reported_at": "{reported_at}"}}"#),
    );
    // The two refusals the 2026-08-19 gap actually produced, in the words the
    // components wrote them in.
    for (offset, reason, detail) in [
        (
            120,
            "authority_unreachable",
            "registry authority exited: ssh connect Operation timed out",
        ),
        (
            60,
            "directory_cache_stale",
            "service directory cache is stale",
        ),
    ] {
        let at = now - TimeDelta::seconds(offset);
        write_blob(
            storage,
            &format!("reader_refusals/h1/{}.json", stamp(at)),
            &format!(
                r#"{{"host": "h1", "at": "{}", "reader": "resolver",
                      "reason": "{reason}", "detail": "{detail}"}}"#,
                beacon_time(at)
            ),
        );
    }

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a host readers refused about is not healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    // The beacon is fresh, so this is not silence: it is a host nobody could
    // read while it was perfectly well.
    assert_eq!(report["verdict"], "degraded");
    assert_eq!(
        report["reader_refusals"],
        serde_json::json!({
            "window_seconds": 3600,
            "count": 2,
            "reasons": {"authority_unreachable": 1, "directory_cache_stale": 1}
        })
    );
    let blockers: Vec<&str> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker.as_str().unwrap())
        .collect();
    assert!(
        blockers.contains(
            &"readers refused 2 time(s) in the last 3600s: authority_unreachable=1, \
              directory_cache_stale=1"
        ),
        "the refusal blocker counts each reason token, got: {blockers:?}"
    );
    assert!(
        stderr(&out).contains("h1 link verdict is degraded, with"),
        "got: {}",
        stderr(&out)
    );
    // A fresh beacon leaves no silence behind, so there is nothing to record.
    assert!(!storage.join("host_silence/h1").exists());

    let out = stado(storage, &["host", "link", "h1"]);
    assert!(
        stdout(&out).contains(
            "refusals: 2 in the last 3600s: authority_unreachable=1, directory_cache_stale=1"
        ),
        "the report form counts the same refusals, got:\n{}",
        stdout(&out)
    );
}

#[test]
fn host_link_refuses_a_target_the_registry_does_not_carry() {
    let dir = storage_with_registry();
    let storage = dir.path();

    let out = stado(storage, &["host", "link", "nope", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("target 'nope' is not in the canonical registry"),
        "got: {}",
        stderr(&out)
    );
    // A refused target produces no document at all: half a report about a host
    // that does not exist is worse than none.
    assert!(stdout(&out).trim().is_empty(), "got: {}", stdout(&out));
    assert!(!storage.join("host_silence").exists());
}
