//! The three answers the metadata write plane owes its caller.

use crate::dashboard::{harness, Unreadable, NAMESPACE};
use crate::vault::namespace_token;

/// The object the outage case asks about, inside its own directory so the
/// directory can be closed without touching anything else in the store.
const CLOSED_KEY: &str = "data/outage/probe.json";
/// An object nothing ever wrote, which is what a genuine absence looks like.
const MISSING_KEY: &str = "data/absent/nothing.json";
/// The object the successful metadata write attaches to.
const WRITTEN_KEY: &str = "data/attached/probe.json";

/// The bytes the cases put in the store, so a case can prove the route left
/// the object alone.
const BODY: &str = r#"{"probe":"object write outage"}"#;

fn metadata_only_target(key: &str) -> String {
    format!("/api/object?uri=stado://{NAMESPACE}/{key}&metadata_only=true")
}

/// A store that could not answer must not be reported as an object that is
/// not there.
///
/// The object is on disk; its directory is closed to this process, so the
/// store can neither confirm nor deny it. `Path::is_file` answered `false` for
/// that refusal and the route turned it into `404 {"state":"absent"}`: a write
/// caller was told the object was gone while it sat there, which is the answer
/// `stado storage stat` refuses to give on the read plane.
#[test]
fn a_store_that_could_not_answer_is_not_reported_absent() {
    let harness = harness();
    let object = harness.write_object(CLOSED_KEY, BODY);
    let directory = object.parent().expect("the object has a directory");

    let closed = Unreadable::close(directory);
    let answer = harness.put(
        &metadata_only_target(CLOSED_KEY),
        &namespace_token(NAMESPACE),
        r#"{"reviewed":"yes"}"#,
    );
    drop(closed);

    assert!(
        !answer.body.contains("absent"),
        "a store that could not answer was reported as an absence: {} {}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.status,
        500,
        "the refusal did not reach the caller as a failure: {}",
        answer.body
    );
    assert!(
        answer.body.contains("Permission denied"),
        "the answer did not name what the store said: {}",
        answer.body
    );
    assert!(
        answer.body.contains("exists"),
        "the answer did not name the read that failed: {}",
        answer.body
    );

    assert_eq!(
        std::fs::read_to_string(&object).expect("the object is still on disk"),
        BODY,
        "the object was changed by a request the store could not answer"
    );
    assert!(
        !harness.metadata_file(CLOSED_KEY).exists(),
        "metadata was attached to an object the store could not even find"
    );
}

/// A genuine absence stays an ANSWER, with the state the caller branches on.
///
/// This is the other half of the same contract: separating the outage out must
/// not turn every missing object into a failure, or a caller creating an
/// object cannot tell "not there yet" from "the store is down".
#[test]
fn an_object_nothing_wrote_is_still_answered_absent() {
    let harness = harness();

    let answer = harness.put(
        &metadata_only_target(MISSING_KEY),
        &namespace_token(NAMESPACE),
        r#"{"reviewed":"yes"}"#,
    );

    assert_eq!(answer.status, 404, "body: {}", answer.body);
    assert_eq!(
        answer.body,
        format!(r#"{{"state":"absent","uri":"stado://{NAMESPACE}/{MISSING_KEY}"}}"#)
    );
    assert!(
        !harness.object_file(MISSING_KEY).exists(),
        "a metadata write created the object it reported absent"
    );
    assert!(
        !harness.metadata_file(MISSING_KEY).exists(),
        "metadata was attached to an object that is not there"
    );
}

/// And a metadata write the store can answer leaves the metadata where the
/// store keeps it, so the two refusals above are measured against a route that
/// can also succeed.
#[test]
fn a_metadata_write_the_store_answers_lands_on_disk() {
    let harness = harness();
    let object = harness.write_object(WRITTEN_KEY, BODY);

    let answer = harness.put(
        &metadata_only_target(WRITTEN_KEY),
        &namespace_token(NAMESPACE),
        r#"{"reviewed":"yes"}"#,
    );

    assert_eq!(answer.status, 200, "body: {}", answer.body);
    assert_eq!(
        answer.body,
        format!(r#"{{"state":"metadata-updated","uri":"stado://{NAMESPACE}/{WRITTEN_KEY}"}}"#)
    );

    let sidecar = std::fs::read_to_string(harness.metadata_file(WRITTEN_KEY))
        .expect("the store wrote the metadata sidecar");
    let recorded: serde_json::Value =
        serde_json::from_str(&sidecar).expect("the sidecar is valid JSON");
    assert_eq!(
        recorded["reviewed"], "yes",
        "the metadata the caller sent is not what the store kept: {recorded}"
    );
    assert_eq!(
        std::fs::read_to_string(&object).expect("the object is still on disk"),
        BODY,
        "a metadata write rewrote the object's bytes"
    );
}
