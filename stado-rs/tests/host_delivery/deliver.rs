//! Deliveries that really move bytes into a managed run directory on this
//! machine, and the state they leave behind.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

use serde_json::Value;

use crate::fleet::{delivered_mode, said, stdout, Fleet, TARGET};

#[test]
fn a_selected_tree_lands_byte_for_byte_and_a_second_delivery_replaces_it() {
    let fleet = Fleet::new();
    fs::create_dir_all(fleet.source.join("nested")).unwrap();
    fs::write(fleet.source.join("run.sh"), b"#!/bin/sh\necho first\n").unwrap();
    fs::set_permissions(
        fleet.source.join("run.sh"),
        fs::Permissions::from_mode(0o751),
    )
    .unwrap();
    fs::write(fleet.source.join("nested/data.txt"), b"selected\n").unwrap();
    fs::write(fleet.source.join("ignored.txt"), b"not selected\n").unwrap();
    symlink("nested/data.txt", fleet.source.join("current")).unwrap();

    let destination = Fleet::destination("probierz");
    let first = fleet.deliver_selection(&destination, b"run.sh\0nested/data.txt\0current\0");
    assert!(first.status.success(), "{}", said(&first));
    let receipt: Value = serde_json::from_slice(&first.stdout).expect("the receipt is JSON");
    assert_eq!(receipt["status"], "delivered");
    assert_eq!(receipt["selection"], "nul-file-list");

    // The state the transfer left: the selected bytes, the mode, the symlink
    // kept as a symlink, and nothing the file list did not name.
    let landed = fleet.delivered("probierz");
    assert_eq!(
        fs::read(landed.join("nested/data.txt")).unwrap(),
        b"selected\n"
    );
    assert_eq!(
        fs::read(landed.join("run.sh")).unwrap(),
        b"#!/bin/sh\necho first\n"
    );
    assert!(!landed.join("ignored.txt").exists());
    assert!(fs::symlink_metadata(landed.join("current"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(delivered_mode(&landed.join("run.sh")), 0o751);

    // A second delivery of the same destination replaces the tree and leaves
    // no staging directory behind.
    fs::write(fleet.source.join("run.sh"), b"#!/bin/sh\necho second\n").unwrap();
    let second = fleet.deliver_selection(&destination, b"run.sh\0current\0nested/data.txt\0");
    assert!(second.status.success(), "{}", said(&second));
    assert_eq!(
        fs::read(landed.join("run.sh")).unwrap(),
        b"#!/bin/sh\necho second\n"
    );
    assert!(!fleet.delivered(".probierz.stado-previous").exists());
}

#[test]
fn an_application_bundle_arrives_as_a_complete_mode_preserving_tree() {
    let fleet = Fleet::new();
    let bundle = fleet.source.join("Byk Preview.app");
    let executable = bundle.join("Contents/MacOS/Byk");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        bundle.join("Contents/Info.plist"),
        b"<plist version=\"1.0\"><dict/></plist>\n",
    )
    .unwrap();

    let destination = Fleet::destination("Byk.app");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        bundle.to_str().unwrap(),
        &destination,
        "--json",
    ]);
    assert!(output.status.success(), "{}", said(&output));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).expect("the receipt is JSON")["kind"],
        "directory"
    );

    let landed = fleet.delivered("Byk.app");
    assert_eq!(
        fs::read(landed.join("Contents/MacOS/Byk")).unwrap(),
        b"#!/bin/sh\nexit 0\n"
    );
    assert_eq!(delivered_mode(&landed.join("Contents/MacOS/Byk")), 0o755);
    assert_eq!(
        fs::read(landed.join("Contents/Info.plist")).unwrap(),
        b"<plist version=\"1.0\"><dict/></plist>\n"
    );
}

#[test]
fn one_file_is_delivered_and_reported_where_it_landed() {
    let fleet = Fleet::new();
    let payload = fleet.source.join("bridge.json");
    fs::write(&payload, b"{\"bridge\":\"real\"}\n").unwrap();
    fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();

    let destination = Fleet::destination("bridge.json");
    let output = fleet.run(&[
        "host",
        "deliver",
        TARGET,
        payload.to_str().unwrap(),
        &destination,
    ]);
    assert!(output.status.success(), "{}", said(&output));
    // The human line names the target, the kind and where the bytes went.
    assert!(
        stdout(&output).contains(&format!("{TARGET}: delivered file")),
        "{}",
        stdout(&output)
    );

    let landed = fleet.delivered("bridge.json");
    assert_eq!(fs::read(&landed).unwrap(), b"{\"bridge\":\"real\"}\n");
    assert_eq!(delivered_mode(&landed), 0o600);
}

#[test]
fn the_managed_run_root_is_prepared_owner_only_on_this_machine() {
    let fleet = Fleet::new();
    let output = fleet.run(&[
        "host",
        "exec",
        TARGET,
        "--",
        "mkdir",
        "-p",
        ".stado/work/runs",
    ]);
    assert!(output.status.success(), "{}", said(&output));

    // The directory the product's own command created, read off disk.
    let root = fleet.run_area();
    assert!(root.is_dir(), "the managed run root was not created");
    assert_eq!(delivered_mode(&root), 0o700);
}
