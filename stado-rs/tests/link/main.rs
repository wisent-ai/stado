//! `stado host link` against the machine running the test.
//!
//! What this replaced: the area used to stand on a script named `ssh` on
//! PATH, which received the product's own session script on stdin and ran it
//! against fake `uname`, `id`, `stat` and `launchctl` tools in a temporary
//! `host-bin`; on a fake host called `fake-mini` at `charles@10.9.9.11`; and
//! on a hand-written `link` block claiming `direct 10.0.0.253:41641`, a sleep
//! at `2026-08-19T18:28:55Z` and two `en0` transitions. Every sentence the
//! report printed came out of that fixture, so the report could not be wrong.
//!
//! What stands here instead: an isolated registry whose one target is this
//! machine's own host name, lower cased, with no ssh destination — so the
//! product takes its current-host path and runs this machine's real tools in
//! its own process. The beacon is published through the product itself
//! (`stado host publish-beacon --print`), which is where the `link` block is
//! collected from the real power log, the real unified log and the real
//! tailnet tool. `stado host link` then reads that document back, and the
//! assertions compare the report both with the bytes the product published
//! and with facts this test reads for itself in `machine.rs`: the login, the
//! console owner, the launchd domains, the newest sleep and wake in the
//! actual power log, and whether a tailnet tool exists on PATH at all.
//!
//! Isolation: one `tempfile::TempDir` per case, `WC_STORAGE_BACKEND=local`,
//! `WC_LOCAL_STORAGE_PATH` and `HOME` inside it, `STADO_CONFIG` pointed at a
//! path that does not exist, and every configuration variable this flow reads
//! removed from the child. Nothing here touches the operator's registry,
//! vault, fleet, launchd units or any remote host.

mod checks;
mod fixture;
mod machine;
mod refusals;
mod silence;

use chrono::Utc;
use serde_json::{json, Value};

use checks::{check_against_this_machine, local_route};
use fixture::{
    beacon_time, blockers, document, stderr, stdout, Fixture,
    REFUSAL_WINDOW_SECONDS, THRESHOLD_SECONDS,
};


/// The case the whole area turns on: the product collects this machine's link
/// block, the reader reads it back, and every field is checked twice — once
/// against the bytes the product published, once against the machine.
#[test]
fn the_report_reads_back_the_link_block_this_machine_published() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let reported_at = beacon_time(Utc::now());
    let published = fixture.publish_beacon(&reported_at);
    let cutoff = Utc::now();

    let block = &published["link"];
    assert!(
        block.is_object(),
        "publish-beacon collects a link block for this machine, got: {published}"
    );
    check_against_this_machine(block, cutoff);

    // The oracle above is only worth its assertions if it can refuse. This is
    // the block the deleted fixture wrote by hand: a path and endpoint nobody
    // read here, and a sleep from August. Running the same checks against it
    // has to fail, so the check is exercised rather than assumed.
    let fabricated = json!({
        "collected_at": beacon_time(Utc::now()),
        "path_kind": "direct",
        "endpoint": "10.0.0.253:41641",
        "last_sleep_at": "2026-08-19T18:28:55Z",
        "last_wake_at": "2026-08-19T18:35:02Z",
        "interface_changes": [
            {"at": "2026-08-19T18:29:01Z", "detail": "en0 link down"},
            {"at": "2026-08-19T18:35:04Z", "detail": "en0 link up"}
        ],
        "source": "pmset+tailscale"
    });
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let refused = std::panic::catch_unwind(|| check_against_this_machine(&fabricated, cutoff));
    std::panic::set_hook(previous);
    assert!(
        refused.is_err(),
        "the machine checks accepted a block nobody collected here"
    );

    fixture.seed_beacon(&published);
    let out = fixture.stado(&["host", "link", &host, "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a beacon published seconds ago with nothing refused is healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["host"], host.as_str());
    assert_eq!(report["verdict"], "healthy");

    // The published block, field for field. A reader that dropped or reworded
    // one of these is the failure the operator hit: the fact existed on the
    // host and did not reach the person reading about it.
    for field in [
        "path_kind",
        "endpoint",
        "last_sleep_at",
        "last_wake_at",
        "interface_changes",
    ] {
        assert_eq!(
            report[field], block[field],
            "{field} does not match the block this machine published"
        );
    }

    // The channel: this machine is the target, so there is one route and no
    // hop, and the report names it rather than inventing an ssh destination.
    assert_eq!(report["ssh_reachable"], true);
    assert_eq!(report["selected_connection"], "local");
    assert_eq!(report["connection_paths"], local_route());
    assert_eq!(report["connection_probe_error"], Value::Null);
    assert_eq!(
        report["beacon_publisher"],
        Value::Null,
        "a fresh beacon needs no publisher diagnosis: {}",
        stdout(&out)
    );

    // The session, against the console owner, the login and the launchd
    // domains this test read for itself.
    let (kind, console_owner, detail) = machine::expected_session();
    assert_eq!(report["session"]["kind"], kind);
    assert_eq!(report["session"]["console_owner"], json!(console_owner));
    assert_eq!(report["session"]["detail"], detail.as_str());

    let age = report["beacon_age_seconds"]
        .as_i64()
        .expect("beacon_age_seconds is a number");
    assert!(
        (0..THRESHOLD_SECONDS).contains(&age),
        "the beacon this run published is inside the {THRESHOLD_SECONDS}s threshold, got {age}"
    );
    assert_eq!(
        report["reader_refusals"],
        json!({"window_seconds": REFUSAL_WINDOW_SECONDS, "count": 0, "reasons": {}}),
        "nothing refused about this host, and the window is the product's own"
    );
    assert_eq!(report["silences"], json!([]));
    assert_eq!(report["blockers"], json!([]));
    // A host inside the threshold leaves no silence behind, so the reader
    // wrote nothing at all under the isolated root.
    assert!(!fixture.silence_dir().exists());
}

/// The report form prints the same facts the document carries, and prints the
/// operator's sentence for the session above the resolver's vocabulary for
/// it. Reversing those two is how `gui/501` becomes the answer to "is anyone
/// logged in on that host".
#[test]
fn the_report_form_prints_what_this_machine_published() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let published = fixture.publish_beacon(&beacon_time(Utc::now()));
    fixture.seed_beacon(&published);

    let out = fixture.stado(&["host", "link", &host]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);

    let block = &published["link"];
    let path = match block["endpoint"].as_str() {
        Some(endpoint) => format!(
            "path:     {} via {endpoint}",
            block["path_kind"].as_str().unwrap_or_default()
        ),
        None => format!(
            "path:     {}",
            block["path_kind"].as_str().unwrap_or_default()
        ),
    };
    let (_, _, detail) = machine::expected_session();
    let sleep = |field: &str| {
        block[field]
            .as_str()
            .map_or_else(|| "-".to_string(), str::to_string)
    };
    for line in [
        format!("host:     {host}"),
        "verdict:  healthy".to_string(),
        "blockers: none".to_string(),
        "ssh:      answered".to_string(),
        "routes:   local (local process) answered, selected".to_string(),
        format!("          {detail}"),
        path,
        format!(
            "sleep:    last slept {}, last woke {}",
            sleep("last_sleep_at"),
            sleep("last_wake_at")
        ),
        format!("refusals: none in the last {REFUSAL_WINDOW_SECONDS}s"),
        "silences: none recorded for this host".to_string(),
    ] {
        assert!(text.contains(&line), "missing {line:?} in:\n{text}");
    }
    assert!(
        blockers(&document(
            &fixture.stado(&["host", "link", &host, "--json"])
        ))
        .is_empty(),
        "the two forms disagree about whether anything is blocking this host"
    );
}
