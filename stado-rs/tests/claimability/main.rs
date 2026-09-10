//! A queue with no claimant, and how the product says so.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! in, and the registry document, the queued jobs, the capacity publications
//! and the health beacons all live and die inside the temp dir. Nothing here
//! reads the fleet's store or touches a host.
//!
//! What is under test is `stado status` and `stado overview` as reports: a
//! stuck queue is stated, a moving one is not mentioned, and neither says
//! anything about the exit status — both stay 0, because this is a report and
//! not a gate.
//!
//! Every fixture is copied from the live incident it was written for, not
//! invented. Job `2c4a47aa` is `bash inputs/run.sh`, submitted
//! 2026-08-14T19:11:37Z, `provider: local`, pinned to
//! `local-control-host.local`, and queued for 121 hours. The vocabulary
//! is the vocabulary `stado host gates control-host --json` printed on
//! that day: `blockers: ["no_capacity_publication", "pinned_only"]` with
//! `capacity.published_at: null`. The mini's queue-agent declaration is its
//! real one, `com.wisent.compute.service.stado-agent-mini` at
//! `/Users/charles/Library/LaunchAgents/...`, a unit its own health beacon
//! does not report.

mod overview;
mod support;

use serde_json::Value;

use support::{
    beacon, claimability, fleet, publish, queue_job, stado, stdout, AGENT_LABEL,
    AGENT_PLIST, JOB_ID, WAITED_SECONDS,
};

/// A queue nobody publishes capacity for is named as such, host by host, in
/// each host's own words — and the report is still a report: exit 0.
#[test]
fn status_names_every_host_that_cannot_claim_the_queue() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    // The mini's beacon is alive and does not carry the declared agent: the
    // unit is bootstrapped into a domain that host cannot have, so nothing
    // ever loaded it.
    beacon(
        storage,
        "mini",
        serde_json::json!({"com.wisent.host-health-beacon": {"state": "active"}}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success(), "a report never fails the command");
    let text = stdout(&out);

    assert!(
        text.contains(&format!(
            "nothing can claim the queue: 1 queued, oldest {JOB_ID} waiting 121h "
        )),
        "the headline sizes the stall with the oldest wait: {text}"
    );
    assert!(
        text.contains("0 of 3 local hosts publish capacity newer than 180s"),
        "the headline counts publishers, not declarations: {text}"
    );
    assert!(
        text.contains(&format!(
            "  cannot claim: mini no_capacity_publication, agent_declared_not_loaded ({AGENT_LABEL} is declared at {AGENT_PLIST}; the latest beacon does not report it)"
        )),
        "the mini's silence is explained by its own declaration: {text}"
    );
    // Pinned with nothing pinned to it: the pin IS the reason, so it is said.
    assert!(
        text.contains(
            "  cannot claim: rtx no_capacity_publication, pinned_only (no queued job names this host)"
        ),
        "{text}"
    );
    assert!(
        text.contains("  cannot claim: laptop no_capacity_publication"),
        "{text}"
    );
    // The mini IS named by the queued job, so `pinned_only` is not one of its
    // reasons: sending an operator to unpin a host that would have claimed is
    // sending them to the wrong place.
    assert!(
        !text.contains("cannot claim: mini no_capacity_publication, pinned_only"),
        "a pinned host with a matching queued job is not blocked by its pin: {text}"
    );
}

/// "That host went quiet seventeen hours ago" and "that host never said
/// anything" are different findings, and the reader must not collect the
/// evidence for the first one on its way past.
#[test]
fn status_reports_an_aged_publication_as_stale_and_keeps_it() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    publish(
        storage,
        "local-laptop.local",
        17 * 3600 + 5 * 60,
        serde_json::json!({}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("  cannot claim: laptop capacity_publication_stale (last published 17h "),
        "a row past the horizon is stale, with its age: {text}"
    );
    assert!(
        !text.contains("cannot claim: laptop no_capacity_publication"),
        "a stale row is not silence: {text}"
    );
    // A report deletes nothing. The scheduler's reader garbage-collects rows
    // past an hour; this one must leave the evidence where it found it.
    assert!(
        storage.join("capacity/local-laptop.local.json").exists(),
        "the publication the verdict reported still exists"
    );
}

/// A pinned host publishing capacity with work addressed to it claims, and a
/// claiming fleet is not commented on at all.
#[test]
fn status_says_nothing_while_a_pinned_host_holds_a_matching_job() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    publish(
        storage,
        "local-mini.local",
        20,
        serde_json::json!({"pinned_only": true}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        !text.contains("nothing can claim the queue"),
        "one host that can claim is a moving queue: {text}"
    );
    assert!(!text.contains("cannot claim:"), "{text}");
    assert_eq!(claimability(storage)["claimable"], Value::Bool(true));
}

/// The same pinned host, with the queued job addressed elsewhere: now the pin
/// is why nothing moves, and the words say so.
#[test]
fn status_blames_the_pin_when_no_queued_job_names_the_pinned_host() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(
        storage,
        JOB_ID,
        WAITED_SECONDS,
        "local-somewhere-else.local",
    );
    publish(
        storage,
        "local-mini.local",
        20,
        serde_json::json!({"pinned_only": true}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("1 of 3 local hosts publish capacity newer than 180s"),
        "the mini is publishing; that is not the problem: {text}"
    );
    assert!(
        text.contains("  cannot claim: mini pinned_only (no queued job names this host)"),
        "{text}"
    );
    assert!(
        !text.contains("cannot claim: mini no_capacity_publication"),
        "a publishing host is not silent: {text}"
    );
}

