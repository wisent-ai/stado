//! `sync_path`, the copy every placement move carries its states through,
//! run with real rsync against a target that is this machine, so the copy is
//! the one a move between two hosts makes minus the SSH channel. Covered:
//! a file pulled into a staging path whose directory does not exist yet; a
//! file rewritten at the same size and modification time, which the quick
//! check leaves stale and the content comparison carries, the case the read
//! after the fence depends on; a file pushed to a stage; a tree pulled with
//! a file the source no longer holds removed; and a relative remote path
//! refused before rsync runs. Everything is kept under the checkout's own
//! `.build/placement-copy`.
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

use stado::deploy::host_delivery::{sync_path, Compare, Copied, Direction};
use stado::deploy::production_runner;
use stado::targets::ComputeTarget;

/// A registry target the copy treats as this machine: named by its own
/// hostname, so `sync_path` runs rsync locally.
fn this_host() -> ComputeTarget {
    let output = Command::new("hostname").output().expect("hostname runs");
    let name = String::from_utf8(output.stdout)
        .expect("the hostname is text")
        .trim()
        .to_lowercase();
    serde_json::from_value(serde_json::json!({ "name": name, "kind": "local" }))
        .expect("a minimal local target parses")
}

/// One test's directory under the checkout's build directory.
fn scratch(test: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the package sits inside the checkout")
        .join(".build/placement-copy")
        .join(format!("{test}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).expect("the scratch directory is created");
    directory
}

fn text(path: &PathBuf) -> String {
    std::fs::read_to_string(path).expect("the copy is readable")
}

fn modified(path: &PathBuf) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .expect("the file has a modification time")
}

#[tokio::test]
async fn a_file_rewritten_within_the_second_is_carried_only_by_content() {
    let target = this_host();
    let runner = production_runner();
    let directory = scratch("file");
    let source = directory.join("source/vault.json");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "first-state").unwrap();
    let staged = directory.join("staging/txn/vault.json");

    sync_path(
        &target,
        &source.to_string_lossy(),
        &staged,
        Copied::File,
        Direction::Pull,
        Compare::Quick,
        &runner,
    )
    .await
    .expect("a file is pulled into a staging directory that did not exist");
    assert_eq!(text(&staged), "first-state");

    // Same size, same modification time: what a write within the second of
    // the first copy leaves on a filesystem rsync reads in whole seconds.
    let stamp = modified(&source);
    std::fs::write(&source, "other-state").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&source)
        .and_then(|file| file.set_modified(stamp))
        .unwrap();
    sync_path(
        &target,
        &source.to_string_lossy(),
        &staged,
        Copied::File,
        Direction::Pull,
        Compare::Quick,
        &runner,
    )
    .await
    .unwrap();
    assert_eq!(
        text(&staged),
        "first-state",
        "the quick check leaves a same-size, same-time rewrite stale"
    );
    sync_path(
        &target,
        &source.to_string_lossy(),
        &staged,
        Copied::File,
        Direction::Pull,
        Compare::Content,
        &runner,
    )
    .await
    .unwrap();
    assert_eq!(
        text(&staged),
        "other-state",
        "the content comparison carries the rewrite"
    );

    let stage = directory.join("destination/vault.json.placement-txn.stage");
    std::fs::create_dir_all(stage.parent().unwrap()).unwrap();
    sync_path(
        &target,
        &stage.to_string_lossy(),
        &staged,
        Copied::File,
        Direction::Push,
        Compare::Quick,
        &runner,
    )
    .await
    .expect("the staged copy is pushed to the destination's stage");
    assert_eq!(text(&stage), "other-state");
    std::fs::remove_dir_all(&directory).unwrap();
}

#[tokio::test]
async fn a_tree_is_pulled_whole_and_a_file_the_source_dropped_goes() {
    let target = this_host();
    let runner = production_runner();
    let directory = scratch("tree");
    let source = directory.join("source/keeper");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("nested/kept.txt"), "kept").unwrap();
    let staged = directory.join("staging/txn/keeper");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("dropped.txt"), "the source no longer holds this").unwrap();

    sync_path(
        &target,
        &source.to_string_lossy(),
        &staged,
        Copied::Tree,
        Direction::Pull,
        Compare::Quick,
        &runner,
    )
    .await
    .expect("the tree is pulled");
    assert_eq!(text(&staged.join("nested/kept.txt")), "kept");
    assert!(
        !staged.join("dropped.txt").exists(),
        "the copy holds exactly the source's tree"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[tokio::test]
async fn a_relative_remote_path_is_refused_before_rsync_runs() {
    let target = this_host();
    let runner = production_runner();
    let directory = scratch("refused");
    let refused = sync_path(
        &target,
        "relative/vault.json",
        &directory.join("copy"),
        Copied::File,
        Direction::Pull,
        Compare::Quick,
        &runner,
    )
    .await
    .expect_err("a relative remote path is refused");
    assert!(
        refused.to_string().contains("is not a clean absolute path"),
        "{refused}"
    );
    assert!(!directory.join("copy").exists(), "nothing was copied");
    std::fs::remove_dir_all(&directory).unwrap();
}
