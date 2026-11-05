//! Build source snapshots in the local store: at the disk-full rule a pass
//! removes the ones nothing names, and keeps each product's newest, the one a
//! release catalog entry names, the one a queued job names, the one an
//! unfinished release run names, and every pinned dependency input. Run on a
//! real APFS volume under the repository's build directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::json;

use crate::native::Native;

/// A snapshot of `product` with a 64-hex digest made from `seed`, aged by
/// `age` so the newest is known.
fn snapshot(store: &Path, product: &str, seed: char, age: u64) -> (PathBuf, String) {
    let digest: String = std::iter::repeat(seed).take(64).collect();
    let path = store
        .join("ecosystem/sources")
        .join(product)
        .join(&digest)
        .join("source.tar.gz");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, vec![b's'; 64 * 1024]).unwrap();
    let when = SystemTime::now() - Duration::from_secs(age);
    fs::File::open(&path).unwrap().set_modified(when).unwrap();
    let uri = format!("stado://sources/{product}/{digest}/source.tar.gz");
    (path, uri)
}

fn write_json(path: &Path, value: serde_json::Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
fn at_the_threshold_unnamed_source_snapshots_go_and_named_ones_stay() {
    let native = Native::new("source-snapshots");
    let store = native.home.join(".stado/local-storage");
    let (newest, _) = snapshot(&store, "widget", 'a', 10);
    let (catalogued, catalogued_uri) = snapshot(&store, "widget", 'b', 100);
    let (queued, queued_uri) = snapshot(&store, "widget", 'c', 200);
    let (running_release, running_uri) = snapshot(&store, "widget", 'd', 300);
    let (forgotten, _) = snapshot(&store, "widget", 'e', 400);
    let (finished_release, finished_uri) = snapshot(&store, "widget", 'f', 500);
    let dependency = store
        .join("ecosystem/sources/widget/dependencies/lib/sha256")
        .join("0".repeat(64))
        .join("lib.tar.gz");
    fs::create_dir_all(dependency.parent().unwrap()).unwrap();
    fs::write(&dependency, b"pinned input").unwrap();

    write_json(
        &store.join("ecosystem/system/release-catalog/widget.json"),
        json!({"product": "widget", "source": {"source_uri": catalogued_uri}}),
    );
    write_json(
        &store.join("ecosystem/stado/queue/job-1.json"),
        json!({"job_id": "job-1", "inputs": {"source": {"uri": queued_uri}}}),
    );
    write_json(
        &store.join("ecosystem/probierz/runs/release-pipeline/widget/run-1/run.json"),
        json!({"product": "widget", "state": "building", "source_uri": running_uri}),
    );
    write_json(
        &store.join("ecosystem/probierz/runs/release-pipeline/widget/run-2/run.json"),
        json!({"product": "widget", "state": "completed", "source_uri": finished_uri}),
    );
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    let cleaner = &report["cleaners"]["source_snapshots"];
    assert_eq!(cleaner["scanned_items"], 6, "{report}");
    assert_eq!(cleaner["deleted_items"], 2, "{report}");
    for kept in [&newest, &catalogued, &queued, &running_release] {
        assert!(kept.is_file(), "{} was taken: {report}", kept.display());
    }
    for taken in [&forgotten, &finished_release] {
        assert!(
            !taken.exists(),
            "{} survived the rule: {report}",
            taken.display()
        );
    }
    assert!(dependency.is_file(), "a pinned dependency input was taken");
    assert_eq!(cleaner["skipped"]["newest_of_product"], 1, "{report}");
    assert_eq!(cleaner["skipped"]["named_by_record"], 3, "{report}");
    assert!(user_data.is_file(), "the user's data was deleted");
}

#[test]
fn an_unreadable_record_keeps_every_snapshot() {
    let native = Native::new("source-snapshots-unreadable");
    let store = native.home.join(".stado/local-storage");
    snapshot(&store, "widget", 'a', 10);
    let (older, _) = snapshot(&store, "widget", 'b', 100);
    let broken = store.join("ecosystem/stado/queue/job-2.json");
    fs::create_dir_all(broken.parent().unwrap()).unwrap();
    fs::write(&broken, b"{not json").unwrap();
    native.fill_with_user_data();

    let report = native.cleanup();
    let cleaner = &report["cleaners"]["source_snapshots"];
    assert!(
        older.is_file(),
        "a snapshot went while a record was unreadable: {report}"
    );
    assert_eq!(cleaner["skipped"]["records_unreadable"], 1, "{report}");
    assert!(
        report["errors"]
            .as_array()
            .is_some_and(|errors| errors.iter().any(|error| error
                .as_str()
                .is_some_and(|text| text.contains("job-2.json")))),
        "the unreadable record is not named: {report}"
    );
}
