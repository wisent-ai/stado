//! The refusals a delivery answers on this machine, and the proof that each
//! one refused before a byte moved.

use std::fs;
use std::os::unix::fs::symlink;

use crate::fleet::{said, stderr, Fleet, TARGET};

/// A destination the fixture keeps outside the managed area, inside its own
/// tempdir, so a delivery that ignored the policy would be visible on disk.
const OUTSIDE_NAME: &str = "outside-the-managed-area";

#[test]
fn a_host_outside_the_registry_is_named_before_any_transfer() {
    let fleet = Fleet::new();
    let payload = fleet.source.join("payload");
    fs::write(&payload, b"never copied").unwrap();

    let destination = Fleet::destination("payload");
    let output = fleet.run(&[
        "host",
        "deliver",
        "not-in-registry",
        payload.to_str().unwrap(),
        &destination,
    ]);
    assert!(!output.status.success(), "{}", said(&output));
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: target 'not-in-registry' is not in the canonical registry")
    );
    assert!(
        !fleet.run_area().exists(),
        "an unknown target must not create a managed run area"
    );
}

#[test]
fn a_source_path_that_does_not_exist_is_named_with_its_own_error() {
    let fleet = Fleet::new();
    let absent = fleet.source.join("never-written.tar");
    let destination = Fleet::destination("never-written.tar");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        absent.to_str().unwrap(),
        &destination,
    ]);
    assert!(!output.status.success(), "{}", said(&output));
    assert_eq!(
        stderr(&output).lines().next(),
        Some(
            format!(
                "Error: cannot read delivery source {:?}: No such file or directory (os error 2)",
                absent.to_str().unwrap()
            )
            .as_str()
        )
    );
    assert!(
        !fleet.run_area().exists(),
        "a missing source must not create a managed run area"
    );
}

#[test]
fn a_destination_outside_the_managed_area_is_refused() {
    let fleet = Fleet::new();
    let payload = fleet.source.join("payload");
    fs::write(&payload, b"never copied").unwrap();
    let outside = fleet.root.path().join(OUTSIDE_NAME);

    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        payload.to_str().unwrap(),
        outside.to_str().unwrap(),
    ]);
    assert!(!output.status.success(), "{}", said(&output));
    assert!(
        stderr(&output).contains(&format!(
            "destination {:?} is outside the managed area",
            outside.to_str().unwrap()
        )),
        "{}",
        stderr(&output)
    );
    assert!(!outside.exists(), "the refused destination was created");
}

#[test]
fn a_destination_that_traverses_a_symlink_is_refused_before_transfer() {
    let fleet = Fleet::new();
    let payload = fleet.source.join("payload");
    fs::write(&payload, b"never copied").unwrap();

    // A real symlink in the managed path, pointing at a directory the
    // delivery must never reach through it.
    let outside = fleet.root.path().join(OUTSIDE_NAME);
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(fleet.home.join(".stado")).unwrap();
    symlink(&outside, fleet.home.join(".stado/work")).unwrap();

    let destination = Fleet::destination("payload");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        payload.to_str().unwrap(),
        &destination,
    ]);
    assert!(!output.status.success(), "{}", said(&output));
    assert!(
        stderr(&output)
            .contains("delivery refused before transfer: destination traverses a symlink"),
        "{}",
        stderr(&output)
    );
    assert!(
        !outside.join("runs").exists(),
        "the symlink target was written through"
    );
}

#[test]
fn a_delivery_without_its_required_arguments_refuses_at_the_command_line() {
    let fleet = Fleet::new();
    let output = fleet.run(&["host", "deliver"]);
    assert_eq!(output.status.code(), Some(2), "{}", said(&output));
    assert!(
        stderr(&output).contains("the following required arguments were not provided"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("<TARGET>"), "{}", stderr(&output));
}
