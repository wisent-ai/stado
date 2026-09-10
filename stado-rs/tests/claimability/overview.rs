//! When the queue is claimable, when it is empty, and what `stado overview`
//! counts and prints either way. A report, never a gate: exit stays 0.

use serde_json::Value;

use crate::support::{
    beacon, claimability, fleet, publish, queue_job, stado, stdout, AGENT_LABEL, AGENT_PLIST,
    JOB_ID, WAITED_SECONDS,
};

/// A host publishing fresh capacity with nothing in its way ends the verdict:
/// the queue is claimable, and neither surface says a word about it.
#[test]
fn a_claimable_queue_gets_no_verdict_on_either_surface() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "");
    publish(storage, "local-laptop.local", 20, serde_json::json!({}));

    let status = stado(storage, &["status"]);
    assert!(status.status.success());
    let listing = stdout(&status);
    assert!(listing.contains("1 queued"), "the job is still listed");
    assert!(
        !listing.contains("nothing can claim the queue"),
        "{listing}"
    );

    let overview = stado(storage, &["overview"]);
    assert!(overview.status.success());
    let snapshot = stdout(&overview);
    assert!(
        snapshot.contains("fleet: 1 of 3 local hosts publishing capacity | 3 registered targets"),
        "the fleet line counts publications: {snapshot}"
    );
    assert!(
        !snapshot.contains("nothing can claim the queue"),
        "{snapshot}"
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(true));
    assert_eq!(verdict["stuck"], Value::Bool(false));
    assert_eq!(verdict["publishing"], serde_json::json!(["laptop"]));
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
