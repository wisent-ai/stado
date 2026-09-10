//! What each surface says about a queue nobody can claim.
use super::*;

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

/// An empty queue is not a stuck queue, however silent the fleet is.
#[test]
fn an_empty_queue_gets_no_verdict_however_silent_the_fleet() {
    let dir = fleet();
    let storage = dir.path();

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("0 queued"), "{text}");
    assert!(!text.contains("nothing can claim the queue"), "{text}");

    let verdict = claimability(storage);
    assert_eq!(verdict["stuck"], Value::Bool(false));
    assert_eq!(verdict["queued"], serde_json::json!(0));
    assert_eq!(verdict["oldest_queued"], Value::Null);
}

/// A publisher no declared host claims — a cloud dispatcher, a marketplace
/// worker — is a claimant this report cannot size, so it refuses to assert a
/// stall. Saying less beats asserting a stall that is not there.
#[test]
fn a_fresh_publisher_the_registry_does_not_declare_withholds_the_verdict() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "");
    publish(storage, "gcp-dispatcher", 20, serde_json::json!({}));

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    assert!(
        !stdout(&out).contains("nothing can claim the queue"),
        "{}",
        stdout(&out)
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(true));
    assert_eq!(
        verdict["unattributed_publishers"],
        serde_json::json!(["gcp-dispatcher"])
    );
    // Not counted as a publishing local host: it is not one.
    assert_eq!(verdict["publishing"], serde_json::json!([]));
}

/// `stado overview` used to print a worker count under the words "active
/// workers" while nothing in the fleet could claim anything. It now counts
/// publications, and prints the verdict with the oldest wait.
#[test]
fn overview_counts_publications_and_prints_the_verdict() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    beacon(
        storage,
        "mini",
        serde_json::json!({"com.wisent.host-health-beacon": {"state": "active"}}),
    );

    let out = stado(storage, &["overview"]);
    assert!(out.status.success(), "a report never fails the command");
    let text = stdout(&out);
    assert!(
        text.contains("fleet: 0 of 3 local hosts publishing capacity | 3 registered targets"),
        "three declared hosts, none publishing: {text}"
    );
    assert!(
        text.contains(&format!(
            "nothing can claim the queue: 1 queued, oldest {JOB_ID} waiting 121h "
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  cannot claim: mini no_capacity_publication, agent_declared_not_loaded ({AGENT_LABEL} is declared at {AGENT_PLIST}; the latest beacon does not report it)"
        )),
        "{text}"
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(false));
    assert_eq!(verdict["stuck"], Value::Bool(true));
    assert_eq!(verdict["stale_horizon_seconds"], serde_json::json!(180));
    assert_eq!(verdict["oldest_queued"]["job_id"], JOB_ID);
    assert!(
        verdict["oldest_queued"]["age_seconds"]
            .as_i64()
            .expect("the oldest wait is dated")
            >= WAITED_SECONDS,
        "{verdict}"
    );
    assert_eq!(
        verdict["hosts"][0]["blockers"][1]["word"],
        "agent_declared_not_loaded"
    );
}

/// A beacon that reports the declared agent as loaded and active leaves the
/// declaration finding unsaid: the unit is not the reason for the silence.
#[test]
fn a_loaded_agent_is_not_reported_as_undeclared_or_unloaded() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    beacon(
        storage,
        "mini",
        serde_json::json!({AGENT_LABEL: {"state": "active"}}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("  cannot claim: mini no_capacity_publication\n"),
        "the host is silent, and its declared agent is not why: {text}"
    );
    assert!(
        !text.contains("agent_declared_not_loaded"),
        "a loaded unit is never reported as unloaded: {text}"
    );
}
