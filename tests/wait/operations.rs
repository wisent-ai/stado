//! A child's wait is measured under its program and subcommand, and a caller
//! names the operation its arguments cannot say.
//!
//! The real `git` runs through `stado_wait` in a directory of the test's own
//! under this checkout's build directory that holds a directory named
//! `status`, with a measurement directory of the test's own. `git status`
//! beside that directory is measured as `proces git status`: the
//! same-named directory does not move the subcommand. `git -C <dir> status`
//! read from its arguments is measured under the directory, the first
//! argument that is not an option; named by its caller with `output_as` it
//! is measured as `proces git status`, the same operation as the plain
//! `git status`. Each record holds the nanoseconds of the longest wait and
//! the operation it measured.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/journeys/wait-operations")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(root.join("status")).expect("the same-named directory");
    root
}

/// The operations the measurement directory `measured` holds a record of.
fn measured_operations(measured: &Path) -> BTreeSet<String> {
    std::fs::read_dir(measured)
        .expect("the measurement directory")
        .map(|entry| entry.expect("a measurement entry").path())
        .filter(|path| path.extension().is_none())
        .map(|path| {
            let record = std::fs::read_to_string(&path).expect("a measurement record");
            let (nanos, operation) = record.split_once(' ').expect("nanoseconds, then the operation");
            nanos.parse::<u64>().expect("the record's nanoseconds");
            operation.trim_end().to_string()
        })
        .collect()
}

#[test]
fn a_subcommand_beside_a_same_named_directory_and_a_named_operation_are_measured_as_one() {
    let root = root();
    let measured = root.join("measured");
    std::env::set_var(stado_wait::measured::DIRECTORY_ENV, &measured);
    std::env::set_var("STADO_WAITS_DIR", root.join("waits"));

    stado_wait::output(Command::new("git").arg("status").current_dir(&root)).expect("git status ran");
    assert_eq!(
        measured_operations(&measured),
        BTreeSet::from(["proces git status".to_string()]),
        "a directory named like the subcommand moved the operation"
    );

    let directory = root.display().to_string();
    stado_wait::output(Command::new("git").args(["-C", &directory, "status"])).expect("git -C ran");
    let read = format!("proces git {directory}");
    assert!(
        measured_operations(&measured).contains(&read),
        "an option's value is read as the subcommand, which is why a caller names it"
    );

    let mut named = Command::new("git");
    named.args(["-C", &directory, "status"]);
    stado_wait::output_as(&mut named, "git status").expect("git -C ran under its named operation");
    assert_eq!(
        measured_operations(&measured),
        BTreeSet::from(["proces git status".to_string(), read]),
        "the named operation is kept apart from the one its arguments say"
    );
    std::fs::remove_dir_all(&root).expect("remove the journey's directory");
}
