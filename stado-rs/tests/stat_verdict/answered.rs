//! The two verdicts that are ANSWERS, against a store on this machine's disk.
//!
//! `present` and `absent` are the only states that exit zero, and the
//! difference between them has to be the store's answer rather than the
//! command's guess — so each case here is checked against the filesystem the
//! store is rooted in: the bytes, their digest, and the absence of a file.

use crate::fixture::{code, digest_on_disk, receipt, said, state, stdout, LocalStore};

/// The object every fleet host really reads out of its store, so the case
/// names a path the product itself uses rather than an invented one.
const REGISTRY: &str = "registry.json";

/// A path under the queue's own prefix that this test never writes, which is
/// how a caller asks "is this coordinate spent?".
const UNWRITTEN: &str = "queue/nothing.json";

/// The declared registry document the case puts in the store. Its
/// constants/config are the shape the product writes — a schema version and
/// empty target lists — copied so the object is a real document rather than
/// arbitrary bytes; nothing here tunes anything.
const REGISTRY_DOCUMENT: &str = r#"{"schema_version":2,"targets":[],"coordinators":[]}"#;

#[test]
fn a_present_object_is_reported_with_the_size_and_digest_it_has_on_disk() {
    let store = LocalStore::new();
    let object = store.write_object(REGISTRY, REGISTRY_DOCUMENT);

    let output = store.stat(&[REGISTRY, "--json"]);
    let receipt = receipt(&output);

    assert_eq!(
        code(&output),
        0,
        "an answered question exits zero: {}",
        said(&output)
    );
    assert_eq!(receipt["state"], "present");
    assert_eq!(receipt["backend"], "local");

    let bytes = std::fs::metadata(&object)
        .expect("the object this test wrote is on disk")
        .len();
    assert_eq!(
        receipt["size"].as_u64(),
        Some(bytes),
        "the reported size is not the size of the bytes on disk: {receipt}"
    );
    assert_eq!(
        receipt["version"].as_str(),
        Some(digest_on_disk(&object).as_str()),
        "the reported version is not the digest of the bytes on disk: {receipt}"
    );
    assert!(
        receipt["updated_at"].is_string(),
        "a present object carries the store's own timestamp: {receipt}"
    );

    // The state the command left behind: opening this store wrote its layout
    // marker, which is what proves the receipt came from a store that was
    // really constructed here rather than from a reading of the path alone.
    let marker: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(store.object_path("system/storage-layout.json"))
            .expect("the store the command opened left its layout marker"),
    )
    .expect("the layout marker is valid JSON");
    assert_eq!(marker["product"], "stado");
}

#[test]
fn an_object_the_store_does_not_have_is_answered_absent_and_exits_zero() {
    let store = LocalStore::new();

    let output = store.stat(&[UNWRITTEN, "--json"]);

    assert_eq!(
        code(&output),
        0,
        "absence is an answer and must exit zero, or a dead store reads as a \
         drained one: {}",
        said(&output)
    );
    assert_eq!(state(&output), "absent");
    let receipt = receipt(&output);
    assert_eq!(receipt["size"], serde_json::Value::Null);
    assert_eq!(
        receipt["version"],
        serde_json::Value::Null,
        "nothing is invented for an object that is not there: {receipt}"
    );
    assert!(
        !store.object_path(UNWRITTEN).exists(),
        "the object the store reported absent is on disk after all"
    );
}

/// The table form says out loud what the exit code means, because the operator
/// reading a terminal is the caller who cannot branch on a status.
///
/// Sentence copied from a live run on 2026-09-08.
#[test]
fn the_table_form_says_the_store_answered_rather_than_stayed_silent() {
    let store = LocalStore::new();

    let output = store.stat(&[UNWRITTEN]);

    assert_eq!(code(&output), 0, "{}", said(&output));
    let printed = stdout(&output);
    assert!(
        printed.contains(
            "The store ANSWERED: \"queue/nothing.json\" is not there. This is not the same as a \
             store that refused the question, could not answer it now, or could not be reached \
             at all."
        ),
        "the absence was not distinguished from silence: {printed}"
    );
    assert!(
        printed
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["state", "absent"]),
        "the table did not carry the verdict: {printed}"
    );
}
