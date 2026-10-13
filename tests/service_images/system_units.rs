//! The launchd units a Mac ships are binary property lists, and the
//! post-install reconcile reads the unit of every job loaded in the domain.
//! These read the system's own units off this machine, as the reconcile does.
#![cfg(target_os = "macos")]

use std::path::PathBuf;

use stado::deploy::service::{local_unit_file, KIND_LAUNCHD};

/// Every binary unit the system ships under its launch daemon directory.
fn system_binary_units() -> Vec<PathBuf> {
    let directory = PathBuf::from("/System/Library/LaunchDaemons");
    std::fs::read_dir(&directory)
        .expect("listing the system's launch daemons")
        .map(|entry| entry.expect("reading a launch daemon entry").path())
        .filter(|path| std::fs::read(path).is_ok_and(|bytes| bytes.starts_with(b"bplist")))
        .collect()
}

#[test]
fn every_binary_system_unit_is_read_with_its_program() {
    let units = system_binary_units();
    assert!(
        !units.is_empty(),
        "this Mac ships no binary launch daemon, so the binary read is not exercised"
    );
    for path in units {
        let shown = path.display().to_string();
        let unit = match local_unit_file(&shown, KIND_LAUNCHD) {
            Ok(Some(unit)) => unit,
            Ok(None) => panic!("{shown} exists and was reported absent"),
            Err(error) => panic!("{shown}: {}", error.message),
        };
        assert!(
            !unit.program.is_empty() || unit.arguments.is_empty(),
            "{shown}: arguments {:?} were read without the program they start with",
            unit.arguments
        );
    }
}

#[test]
fn a_unit_in_neither_encoding_is_refused_naming_it() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("not-a-plist.plist");
    std::fs::write(&scratch, b"\xff\xfe not a property list").expect("writing the scratch unit");
    let shown = scratch.display().to_string();
    let refusal = match local_unit_file(&shown, KIND_LAUNCHD) {
        Err(error) => error.message,
        Ok(unit) => panic!("{shown} was read as {unit:?}"),
    };
    std::fs::remove_file(&scratch).expect("removing the scratch unit");
    assert!(
        refusal.contains(&shown) && refusal.contains("neither a binary property list nor UTF-8"),
        "the refusal does not name the file and its cause: {refusal}"
    );
}
