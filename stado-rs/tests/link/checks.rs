//! Reading one published link block back against the machine that produced
//! it: every field checked twice, once against the bytes the product wrote
//! and once against the running host.

use chrono::{TimeDelta, Utc};
use serde_json::{json, Value};

use crate::fixture::{instant, CHANGE_WINDOW_SECONDS, SLACK_SECONDS};
use crate::machine;

/// The route list a current-host target publishes: one local route, no ssh
/// hop, and `error` omitted because there was none.
pub(crate) fn local_route() -> Value {
    json!([{"name": "local", "destination": "local process", "reachable": true}])
}

/// Everything the collected block claims that this test can check against the
/// machine itself. `cutoff` is the instant the publish returned, which bounds
/// what the collector could possibly have seen.
///
/// This is the function a fabricated report dies in. The block the old
/// fixture wrote by hand — `direct`, `10.0.0.253:41641`, a sleep in August
/// 2026, two `en0` changes — fails every one of these checks on this machine.
pub(crate) fn check_against_this_machine(block: &Value, cutoff: chrono::DateTime<Utc>) {
    let sources = machine::allowed_sources();
    let source = block["source"].as_str().expect("source is a string");
    assert!(
        sources.contains(&source),
        "this machine is {}, so the block may only name {sources:?}, got {source:?}",
        machine::os()
    );

    let collected = instant(block, "collected_at");
    let age = (cutoff - collected).num_seconds();
    assert!(
        (0..=SLACK_SECONDS).contains(&age),
        "the block was collected during this run, got collected_at {collected} against a \
         publish that returned at {cutoff}"
    );

    // The path is the tailnet tool's answer or nothing, and this test does not
    // get to guess where that tool lives: on this machine the product finds it
    // inside the Tailscale application bundle, which a PATH search never sees.
    // What has to hold is that the two fields agree — a named path carries the
    // endpoint it was read from, and an unknown path carries none, so a report
    // can never name a reading nobody took.
    let path_kind = block["path_kind"].as_str().unwrap_or_default();
    assert!(
        ["direct", "relay", "unknown"].contains(&path_kind),
        "the block may only name a path the product declares, got {path_kind:?}"
    );
    if path_kind == "unknown" {
        assert_eq!(
            block["endpoint"],
            Value::Null,
            "an unknown path has no endpoint to report"
        );
    } else {
        assert!(
            block["endpoint"]
                .as_str()
                .is_some_and(|endpoint| endpoint.contains(':')),
            "a {path_kind} path was read from an endpoint, so the block has to carry it, got {}",
            block["endpoint"]
        );
    }

    // The sleep and wake instants are this machine's own newest transitions,
    // read out of the real power log by this test.
    if machine::os() == "Darwin" {
        assert!(
            machine::power_log_has_transitions(),
            "this machine's power log carries no sleep or wake at all, so the two assertions \
             below would check nothing"
        );
        for (field, kinds) in [
            ("last_sleep_at", &machine::SLEEP_KINDS[..]),
            ("last_wake_at", &machine::WAKE_KINDS[..]),
        ] {
            if let Some(reported) = block[field].as_str() {
                assert_eq!(
                    Some(reported),
                    machine::newest_transition(kinds, cutoff).as_deref(),
                    "{field} is not the newest {kinds:?} in this machine's power log"
                );
            }
        }
    }

    // Every interface change sits inside the window the collector read, which
    // is the beacon's own cadence back from the moment it collected.
    let changes = block["interface_changes"]
        .as_array()
        .expect("interface_changes is an array");
    assert!(
        changes.len() <= 8,
        "one beacon carries at most eight changes, got {}",
        changes.len()
    );
    for change in changes {
        let at = instant(change, "at");
        assert!(
            at <= cutoff && at >= collected - TimeDelta::seconds(CHANGE_WINDOW_SECONDS),
            "an interface change at {at} is outside the {CHANGE_WINDOW_SECONDS}s window the \
             collector read back from {collected}"
        );
        assert!(
            !change["detail"]
                .as_str()
                .expect("a change carries a sentence")
                .trim()
                .is_empty(),
            "a change with no sentence is not evidence"
        );
    }
}
