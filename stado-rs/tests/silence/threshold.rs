//! The silence threshold, read from one place.
use super::*;

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
        blob_names(dir.path(), "state/reader_refusals/control-host"),
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
            "state/reader_refusals/control-host/20260819T183011.000000Z.json"
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
        blob_names(dir.path(), "state/host_silence/control-host").len(),
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

    let names = blob_names(dir.path(), "state/reader_refusals/silence-bounded-host");
    assert_eq!(
        names.len(),
        1,
        "the throttle let a duplicate through: {names:?}"
    );
    let record = on_disk(
        dir.path(),
        &format!("state/reader_refusals/silence-bounded-host/{}", names[0]),
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
        blob_names(dir.path(), "state/reader_refusals/silence-bounded-host").len(),
        2
    );
}

// ---------------------------------------------------------------------------
// end to end, through the binary
// ---------------------------------------------------------------------------
