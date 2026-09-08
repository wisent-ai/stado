//! What `service file-fetch` refuses, in the words a live run of the built
//! binary printed on this machine.
//!
//! Every case checks the same two things a refusal owes the operator: the
//! sentence, and that nothing was written where the copy would have gone.

use std::path::Path;

use crate::{on_disk, report, said, stderr, stdout, Fleet};

/// The mode a private key carries, and the mode live operator tooling
/// carries.
const OWNER_ONLY: u32 = 0o600;
const EXECUTABLE_OWNER_ONLY: u32 = 0o700;

/// The product's own transfer limit, `service_file_fetch::MAX_FETCH_BYTES`.
/// The fixture below is one byte past it, because that is the boundary this
/// case exists to cross.
const MAX_FETCH_BYTES: usize = 1_048_576;

#[test]
fn a_symlink_under_the_target_home_is_refused_without_being_followed() {
    let fleet = Fleet::new();
    // The exact escape the confinement exists for: a link inside the managed
    // area whose target is the account's private key.
    let secret = fleet.file(".ssh/id_ed25519", b"PRIVATE KEY MATERIAL\n", OWNER_ONLY);
    let link = fleet.home.path().join(".stado/bin/innocent-looking");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, &link).unwrap();
    let destination = fleet.destination("leaked");

    let out = fleet.file_fetch(
        "$HOME/.stado/bin/innocent-looking",
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(
        !out.status.success(),
        "a refused fetch must exit non-zero:\n{}",
        stdout(&out)
    );
    let said = said(&out);
    assert!(said.contains("refused_symlink"), "{said}");
    assert!(said.contains("was not followed"), "{said}");
    assert!(
        !destination.exists(),
        "a refusal wrote {}",
        destination.display()
    );
    assert!(
        !said.contains("PRIVATE KEY MATERIAL"),
        "the link's target crossed the channel:\n{said}"
    );
    // The key itself is untouched on disk, still holding what the test wrote.
    assert_eq!(on_disk(&secret), b"PRIVATE KEY MATERIAL\n");
}

#[test]
fn a_path_outside_the_target_home_is_refused_before_anything_is_read() {
    let fleet = Fleet::new();
    // A real file that really exists and really is not under this home.
    let outside = fleet.destination("outside.txt");
    std::fs::write(&outside, b"not yours\n").unwrap();
    let destination = fleet.destination("copied-outside");

    let out = fleet.file_fetch(
        outside.to_str().unwrap(),
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let said = said(&out);
    assert!(said.contains("refused_outside_home"), "{said}");
    assert!(!destination.exists());
    assert!(!said.contains("not yours"), "{said}");
}

#[test]
fn a_missing_file_is_reported_as_missing_rather_than_written_as_empty() {
    let fleet = Fleet::new();
    let destination = fleet.destination("never-existed");
    let out = fleet.file_fetch(
        "$HOME/.stado/bin/never-existed",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let row = report(&out);
    assert_eq!(row["file_state"], "missing");
    assert_eq!(row["integrity"], "unverified");
    assert_eq!(row["dest_file"], "-");
    assert!(
        !destination.exists(),
        "a missing source produced {}",
        destination.display()
    );
}

#[test]
fn a_file_past_the_transfer_limit_is_refused_whole_rather_than_truncated() {
    let fleet = Fleet::new();
    // One byte over. A prefix would hash consistently at both ends, so a
    // command that truncated here would report `verified` for half a program.
    let oversized = vec![b'x'; MAX_FETCH_BYTES + 1];
    let source = fleet.file(".stado/bin/too-big", &oversized, EXECUTABLE_OWNER_ONLY);
    let destination = fleet.destination("too-big");

    let out = fleet.file_fetch(
        "$HOME/.stado/bin/too-big",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    assert!(!out.status.success(), "{}", stdout(&out));
    let row = report(&out);
    assert_eq!(row["file_state"], "refused_too_large");
    // The size reported is the file's real size, and the host says which
    // limit it broke.
    assert_eq!(
        row["bytes"],
        serde_json::json!(on_disk(&source).len()),
        "the reported size is not this file's"
    );
    assert_eq!(
        row["detail"],
        serde_json::json!(format!(
            "the file is {} bytes and the limit is {MAX_FETCH_BYTES}",
            oversized.len()
        ))
    );
    assert_eq!(row["host_digest"], "");
    assert!(!destination.exists());
}

#[test]
fn a_relative_destination_is_refused_before_the_host_is_contacted() {
    let fleet = Fleet::new();
    fleet.file(".stado/bin/thing", b"#!/bin/sh\n", EXECUTABLE_OWNER_ONLY);
    let out = fleet.file_fetch("$HOME/.stado/bin/thing", &["--dest-file", "thing"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--dest-file must be absolute"),
        "{}",
        stderr(&out)
    );
    assert!(
        !Path::new("thing").exists(),
        "a refused destination was written into the working directory"
    );
}

#[test]
fn a_host_the_registry_does_not_hold_is_refused_before_any_file_is_read() {
    let fleet = Fleet::new();
    let source = fleet.file(
        ".stado/bin/weles-release-cutover",
        b"#!/bin/sh\nexit 0\n",
        EXECUTABLE_OWNER_ONLY,
    );
    let destination = fleet.destination("from-elsewhere");

    // `elsewhere` is a name this registry never declared. A fetch that fell
    // back to the machine it is standing on would copy this file and label the
    // copy as another host's.
    let out = fleet.fetch_from(
        "elsewhere",
        "$HOME/.stado/bin/weles-release-cutover",
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(
        !out.status.success(),
        "file-fetch answered for a host outside the registry:\n{}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("target 'elsewhere' is not in the canonical registry"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        !destination.exists(),
        "a fetch for an unknown host wrote {}",
        destination.display()
    );
    // The source is still exactly what the test wrote.
    assert_eq!(on_disk(&source), b"#!/bin/sh\nexit 0\n");
}
