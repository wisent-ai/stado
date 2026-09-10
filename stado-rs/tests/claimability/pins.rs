//! A host pinned to a job, and what the surfaces say when the pin is the
//! reason nothing moves.
use super::*;

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
