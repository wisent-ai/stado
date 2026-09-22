//! Writing one field by the path that reads it, and refusing a write to a
//! path that does not exist without touching the document.

use serde_json::Value;

use crate::fixture::{stderr, stdout, untouched, Store};

#[test]
fn one_field_is_written_by_the_path_that_reads_it() {
    let store = Store::new();
    let written = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w2.ssh",
        "--value",
        "u@10.0.0.9",
    ]);
    assert!(written.status.success(), "{}", stderr(&written));
    assert!(
        stdout(&written).contains("was u@10.0.0.2"),
        "the sentence names what it replaced: {}",
        stdout(&written)
    );

    let read = store.stado(&["registry", "pull", "--path", "targets.w2.ssh"]);
    assert_eq!(stdout(&read).trim(), "u@10.0.0.9", "{}", stderr(&read));

    // A second identical write changes nothing and says so.
    let again = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w2.ssh",
        "--value",
        "u@10.0.0.9",
        "--json",
    ]);
    assert!(again.status.success(), "{}", stderr(&again));
    let receipt: Value = serde_json::from_str(&stdout(&again)).expect("a receipt");
    assert_eq!(receipt["state"], "unchanged");
    assert_eq!(receipt["schema"], "stado.registry-set-receipt.v1");
    untouched(store.home.path());
}

#[test]
fn a_write_to_a_path_that_does_not_exist_changes_nothing() {
    let store = Store::new();
    let before = std::fs::read_to_string(store.storage.path().join("registry.json"))
        .expect("read the canonical document");
    let refused = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w9.release_platform",
        "--value",
        "linux-amd64",
    ]);
    assert!(!refused.status.success());
    assert!(
        stderr(&refused)
            .contains("registry array `targets` has no element named `w9`; names there: w1, w2"),
        "{}",
        stderr(&refused)
    );
    let after = std::fs::read_to_string(store.storage.path().join("registry.json"))
        .expect("read the canonical document");
    assert_eq!(before, after, "a refused write left the registry alone");
}
