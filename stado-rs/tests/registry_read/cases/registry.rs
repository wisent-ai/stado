//! Reading one part of the registry, and being refused with what is there.

use crate::fixture::{stderr, stdout, untouched, Store};

#[test]
fn one_part_of_the_registry_is_printed_by_key_index_or_name() {
    let store = Store::new();
    let out = store.stado(&["registry", "pull", "--path", "targets.w2.release_platform"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "darwin-arm64", "a string prints bare");

    let out = store.stado(&["registry", "pull", "--path", "targets.w1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let target: Value = serde_json::from_str(&stdout(&out)).expect("a subtree prints as JSON");
    assert_eq!(target["hostnames"][0], "w1.local");

    let out = store.stado(&["registry", "pull", "--path", "targets.1.name"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "w2",
        "an index reaches an array element"
    );
    untouched(store.home.path());
}

#[test]
fn a_missing_segment_is_refused_with_what_exists_there() {
    let store = Store::new();
    let out = store.stado(&["registry", "pull", "--path", "targets.w9.ssh"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("registry array `targets` has no element named `w9`; names there: w1, w2"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "pull", "--path", "schema_version.more"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("registry value at `schema_version` is a number, which has no `more` inside"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "pull", "--path", "nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no `nope` under `<root>`; keys there: coordinators, public_origins, schema_version, targets"),
        "{}",
        stderr(&out)
    );
}

/// One host's declaration, which is the question an operator actually has.
/// Until 2026-09-20 it needed either the whole document in a file or a dotted
/// path only a reader of the schema could write.
#[test]
fn one_host_is_read_by_name_and_an_unknown_one_is_refused_with_the_names() {
    let store = Store::new();
    let out = store.stado(&["registry", "host", "show", "w2"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let host: Value = serde_json::from_str(&stdout(&out)).expect("the host prints as JSON");
    assert_eq!(host["ssh"], "u@10.0.0.2");
    assert_eq!(host["release_platform"], "darwin-arm64");

    let out = store.stado(&["registry", "host", "show", "w1", "--path", "hostnames.0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "w1.local", "a string prints bare");

    let out = store.stado(&["registry", "host", "show", "w9"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no host `w9`; hosts there: w1, w2"),
        "{}",
        stderr(&out)
    );

    let out = store.stado(&["registry", "host", "show", "w1", "--path", "space"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("registry has no `space` under `<root>`; keys there:"),
        "{}",
        stderr(&out)
    );
    untouched(store.home.path());
}

