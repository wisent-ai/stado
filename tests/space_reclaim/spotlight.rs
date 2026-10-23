//! `stado host spotlight` over the isolated native home: a directory its
//! build tool tagged as regenerable is reported unmarked, `--apply` writes
//! the `.metadata_never_index` that keeps Spotlight out of it, and the next
//! report reads it excluded. A directory no CACHEDIR.TAG marks is neither
//! reported nor written to, whatever its name. A tagged cache whose folder
//! cannot be written fails the command, reported `mark-failed` with the
//! shell's own sentence.

use std::fs;

use serde_json::Value;

#[path = "native.rs"]
mod native;

use crate::native::Native;

const HOST: &str = "example-cleanup-host";

/// The state the report gives `path`, if it names it.
fn state_of(report: &Value, path: &std::path::Path) -> Option<String> {
    let path = path.to_str().expect("UTF-8 path");
    report["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["path"] == path)
        .map(|entry| entry["state"].as_str().expect("a state").to_owned())
}

#[test]
fn a_tagged_build_cache_is_marked_for_spotlight_and_an_untagged_directory_is_left_alone() {
    let native = Native::new("spotlight");
    let cache = native.home.join("Projects/sums/target");
    native.cache(&cache);
    let untagged = native.home.join("Projects/sums/target-copy");
    fs::create_dir_all(&untagged).expect("an untagged directory");

    let before = native.json(&["host", "spotlight", HOST, "--json"]);
    native.observe("before", before.clone());
    assert_eq!(
        state_of(&before, &cache).as_deref(),
        Some("unmarked"),
        "{before}"
    );
    assert_eq!(state_of(&before, &untagged), None, "{before}");
    assert!(!cache.join(".metadata_never_index").exists());

    let applied = native.json(&["host", "spotlight", HOST, "--apply", "--json"]);
    native.observe("applied", applied.clone());
    assert_eq!(
        state_of(&applied, &cache).as_deref(),
        Some("marked"),
        "{applied}"
    );
    assert!(cache.join(".metadata_never_index").is_file());
    assert!(!untagged.join(".metadata_never_index").exists());

    let after = native.json(&["host", "spotlight", HOST, "--json"]);
    native.observe("after", after.clone());
    assert_eq!(
        state_of(&after, &cache).as_deref(),
        Some("excluded"),
        "{after}"
    );

    let locked = native.home.join("Projects/rates/target");
    native.cache(&locked);
    let mut permissions = fs::metadata(&locked)
        .expect("the locked cache")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&locked, permissions.clone()).expect("the cache is made unwritable");
    let refused = native.run(&["host", "spotlight", HOST, "--apply", "--json"]);
    permissions.set_readonly(false);
    fs::set_permissions(&locked, permissions).expect("the cache is writable again");
    assert!(
        !refused.status.success(),
        "an unwritable cache fails the command"
    );
    let report: Value =
        serde_json::from_slice(&refused.stdout).expect("the report is still printed");
    native.observe("unwritable", report.clone());
    let path = locked.to_str().expect("UTF-8 path");
    let entries = report["entries"].as_array().expect("entries");
    let Some(failed) = entries.iter().find(|entry| entry["path"] == path) else {
        panic!("the unwritable cache is reported: {report}");
    };
    assert_eq!(failed["state"], "mark-failed", "{report}");
    let cause = failed["cause"].as_str().expect("the shell's sentence");
    assert!(cause.contains(".metadata_never_index"), "{report}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains(cause),
        "the refusal names why"
    );
}
