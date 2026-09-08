//! The refusals a run answers, each proved to have refused before the host
//! was ever asked to do anything.

use crate::fixture::{executable, said, stderr, Fixture, RUN_AREA, TARGET};

/// The sentence every path outside a managed run tree is refused with.
fn path_sentence(path: &str) -> String {
    format!(
        "path '{path}' must be an absolute path below the target account's \
         $HOME/{RUN_AREA}, with no '.' or '..' component"
    )
}

/// The same sentence as the first stderr line of a non-JSON invocation.
fn path_refusal(path: &str) -> String {
    format!("Error: {}", path_sentence(path))
}

#[test]
fn an_executable_outside_a_managed_run_tree_is_refused() {
    let fixture = Fixture::new();

    // A real executable, really on disk, but in the account's home rather
    // than inside a run: the only thing wrong with it is where it lives.
    let stray = fixture.home().join("worker");
    executable(
        &stray,
        "#!/bin/sh\nprintf 'must not run\\n' > \"$HOME/ran\"\n",
    );

    let output = fixture.run_stado(&[
        "host",
        "run-attached",
        TARGET,
        "--program",
        stray.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    assert_eq!(
        stderr(&output).lines().next(),
        Some(path_refusal(stray.to_str().unwrap()).as_str())
    );
    assert!(
        !fixture.home().join("ran").exists(),
        "the refused program was executed anyway"
    );
}

#[test]
fn a_manifest_outside_a_managed_run_tree_is_refused() {
    let fixture = Fixture::new();
    let stray = fixture.home().join("Cargo.toml");
    std::fs::write(&stray, "[package]\nname = \"never-built\"\n").unwrap();

    let output = fixture.run_stado(&[
        "host",
        "build",
        TARGET,
        "--manifest-path",
        stray.to_str().unwrap(),
        "--bin",
        "never-built",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    assert_eq!(
        stderr(&output).lines().next(),
        Some(path_refusal(stray.to_str().unwrap()).as_str())
    );
    assert!(
        !fixture.home().join("target").exists(),
        "the refused build produced output"
    );
}

#[test]
fn a_host_outside_the_registry_is_named_without_contacting_anything() {
    let fixture = Fixture::new();
    let run = fixture.run("unknown-target");
    let program = run.join("worker");
    executable(&program, "#!/bin/sh\nprintf 'ran\\n' > \"$HOME/ran\"\n");

    let output = fixture.run_stado(&[
        "host",
        "run-attached",
        "not-in-registry",
        "--program",
        program.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "{}", said(&output));
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: target 'not-in-registry' is not in the canonical registry")
    );
    assert!(
        !fixture.home().join("ran").exists(),
        "an unknown target still executed the program"
    );
}

#[test]
fn a_removal_that_names_no_run_is_refused_and_leaves_the_root_alone() {
    let fixture = Fixture::new();
    let kept = fixture.run("kept");
    std::fs::write(kept.join("artifact"), b"bytes").unwrap();
    let root = fixture.run_root();

    // The shared run root names no run of its own.
    let output = fixture.run_stado(&[
        "host",
        "remove-run-directory",
        TARGET,
        root.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    // `--json` reports the refusal as an attributed failure record, so the
    // sentence is looked for inside it rather than as the whole first line.
    let sentence = path_sentence(root.to_str().unwrap());
    assert!(
        stderr(&output).contains(&sentence),
        "expected {sentence}\ngot {}",
        stderr(&output)
    );
    assert!(root.is_dir(), "the shared run root was removed");
    assert_eq!(std::fs::read(kept.join("artifact")).unwrap(), b"bytes");
}

#[test]
fn a_removal_that_names_a_nested_subtree_is_refused() {
    let fixture = Fixture::new();
    let run = fixture.run("cleanup");
    std::fs::create_dir_all(run.join("source/nested")).unwrap();
    std::fs::write(run.join("source/nested/artifact"), b"bytes").unwrap();
    let nested = run.join("source");

    let output = fixture.run_stado(&[
        "host",
        "remove-run-directory",
        TARGET,
        nested.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    let sentence = format!(
        "run directory '{}' must be one direct, safely named child of the target account's \
         $HOME/{RUN_AREA}",
        nested.to_str().unwrap()
    );
    assert!(
        stderr(&output).contains(&sentence),
        "expected {sentence}\ngot {}",
        stderr(&output)
    );
    assert!(
        nested.join("nested/artifact").is_file(),
        "the refused removal deleted the subtree anyway"
    );
}
