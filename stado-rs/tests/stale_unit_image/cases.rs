//! What `registry doctor` reports about the image a real loaded unit is
//! executing on this machine.

use std::path::Path;

use stado::deploy::service::{installed_image, IMAGE_SETTLE_SECONDS};

use crate::host::{adopted, digest, Host, STALE, UNREAD};
use crate::unit::{label, write_plist, LoadedUnit};

/// The detail sentence of a row, or everything the row carried.
fn detail(row: &serde_json::Value) -> String {
    row["detail"]
        .as_str()
        .unwrap_or_else(|| panic!("a doctor row carries a detail: {row:#?}"))
        .to_string()
}

/// The whole lifecycle on the shape the incident had: a fleet-installed unit
/// the registry does not declare, loaded and running, then replaced under
/// itself.
///
/// Three phases in one case because they need the same live process, and the
/// process is the point: a fixture that never executed anything cannot show
/// that the image a running pid holds and the file at its path have come
/// apart. The verdict is checked against a sha256 this test computed, and the
/// only difference between phase two and phase three is the declared file's
/// real modification time.
#[test]
fn a_replaced_image_is_reported_against_the_digest_this_test_computed() {
    let host = Host::new();
    let unit = LoadedUnit::start(&host, "lifecycle");

    // Phase one: the process is executing the file its unit names, and the
    // two are one file by content as well as by identity. Reporting anything
    // here would make the check unusable.
    let started_on = unit.image();
    assert_eq!(
        digest(Path::new(&started_on.path)),
        unit.digest,
        "the image the kernel reports for pid {} is not the file this test placed",
        unit.pid
    );
    assert_eq!(
        digest(&unit.program),
        unit.digest,
        "the declared file must still hold the bytes the process started on"
    );
    let clean = host.about(STALE, &unit.label);
    assert!(
        clean.is_empty(),
        "a unit running the file it declares must produce no row: {clean:#?}"
    );

    // Phase two: replaced this instant. The installer writes the bytes and
    // only afterwards cycles the units, so every managed process is
    // legitimately still on the old image for that window.
    let replacement = unit.replace_image();
    assert_ne!(
        replacement, unit.digest,
        "the replacement must be different bytes, or this proves nothing"
    );
    let in_flight = host.about(STALE, &unit.label);
    assert!(
        in_flight.is_empty(),
        "a replacement younger than {IMAGE_SETTLE_SECONDS}s is an installer mid-flight, not a \
         fault: {in_flight:#?}"
    );

    // Phase three: the same bytes, the same inodes, and the only thing that
    // changed is how long the declared file has been in place.
    unit.backdate(IMAGE_SETTLE_SECONDS + 60);
    let row = host.only_row(STALE, &unit.label);
    let detail = detail(&row);

    let running = unit.image();
    let (installed, written) =
        installed_image(&unit.program).expect("the replacement is readable on disk");
    assert_eq!(
        running.links, 0,
        "the running image must have been unlinked for this to be the incident's shape"
    );
    assert!(
        chrono::Utc::now().timestamp() - written >= IMAGE_SETTLE_SECONDS,
        "the declared file's real mtime is what carries this row"
    );
    for fact in [
        unit.label.clone(),
        format!("pid {}", unit.pid),
        running.describe(),
        installed.describe(),
        unit.program.display().to_string(),
    ] {
        assert!(
            detail.contains(&fact),
            "the row must name {fact} so nobody re-derives it: {detail}"
        );
    }
    assert!(
        detail.contains("has been unlinked"),
        "a deleted image must be named as deleted, not merely as different: {detail}"
    );
    assert_eq!(
        row["subject"].as_str(),
        Some(host.target.as_str()),
        "the row must be about this machine: {row:#?}"
    );
    // The independent measurement: the product called the running image stale,
    // and the digests say the bytes at that path are not the bytes that
    // process is executing.
    assert_eq!(digest(&unit.program), replacement);
    assert_ne!(replacement, unit.digest);
}

/// A running image that still has a name is a different operator problem from
/// one that has none, and the row says which.
#[test]
fn an_image_that_still_has_a_name_is_replaced_and_not_unlinked() {
    let host = Host::new();
    let unit = LoadedUnit::start(&host, "kept");
    // A second name for the very bytes the process is executing, so unlinking
    // the path it was started from leaves the image itself on disk.
    let kept = host.root.join("bin/kept-link");
    std::fs::hard_link(&unit.program, &kept).expect("second link to the running image");
    // The declared half of the enumeration: this unit comes off the document
    // as well as out of the agent directory.
    host.declare(&[adopted(&unit.label, &unit.plist)]);

    unit.replace_image();
    unit.backdate(IMAGE_SETTLE_SECONDS + 60);

    let detail = detail(&host.only_row(STALE, &unit.label));
    let running = unit.image();
    assert_eq!(
        running.links, 1,
        "the surviving hard link is what makes this the replaced case"
    );
    assert!(
        !detail.contains("has been unlinked"),
        "an image with a name left is not the unlinked case: {detail}"
    );
    assert!(
        detail.contains("is not the file its unit declares"),
        "the row must still say the process is on the wrong file: {detail}"
    );
    assert!(
        detail.contains(&running.describe()),
        "the row must name the running identity: {detail}"
    );
    assert_eq!(
        digest(&kept),
        unit.digest,
        "the surviving link holds the bytes the process is executing"
    );
    assert_ne!(
        digest(&unit.program),
        unit.digest,
        "the declared path holds different bytes now"
    );
}

/// A unit whose declaration cannot be read is reported as unknown.
///
/// "Nothing was read" and "nothing is wrong" are different facts, and
/// rendering the first as the second is the defect this whole check exists to
/// remove. The plist here is a real file this test wrote and the product
/// really parses; no launchd job is involved, because a declaration that
/// names no program is refused before any process is looked for.
#[test]
fn a_declaration_that_names_no_program_is_unknown_and_never_clean() {
    let host = Host::new();
    let label = label("programless");
    let plist = write_plist(&host, &label, &[], false);

    let detail = detail(&host.only_row(UNREAD, &label));
    assert!(
        detail.contains("is unknown here and is NOT reported as agreement"),
        "the row must refuse to read as a pass: {detail}"
    );
    assert!(
        detail.contains("carries neither ProgramArguments nor Program"),
        "the row must say what could not be read: {detail}"
    );
    assert!(
        detail.contains(&plist.display().to_string()),
        "the row must name the file it read: {detail}"
    );
    assert!(
        host.about(STALE, &label).is_empty(),
        "an unread unit is never also reported as stale"
    );
}

/// The unit this area loads is removed again, and the removal is read off
/// launchd rather than assumed.
///
/// The second half is the one that matters for the check: with the job gone
/// and the plist still on disk, the doctor reports nothing about the label —
/// a job that is not running holds no image, and that is not a fault.
#[test]
fn the_loaded_unit_is_removed_and_then_reported_on_no_further() {
    let host = Host::new();
    let mut unit = LoadedUnit::start(&host, "removal");
    unit.replace_image();
    unit.backdate(IMAGE_SETTLE_SECONDS + 60);
    assert_eq!(
        host.about(STALE, &unit.label).len(),
        1,
        "the loaded unit is stale before it is removed"
    );

    let removed = unit.remove();
    assert!(
        removed.contains("Could not find service"),
        "launchd must report the label gone: {removed}"
    );
    assert!(
        removed.contains(&unit.label),
        "the removal must name the label this test loaded: {removed}"
    );
    assert_eq!(
        unit.live_pid(),
        None,
        "no process may be left holding the label"
    );
    assert!(
        unit.plist.exists(),
        "the unit file is still on disk, so this is the not-running case and not a missing one"
    );
    assert!(
        host.about(STALE, &unit.label).is_empty(),
        "a job that is not running holds no image to be judged stale"
    );
    assert!(
        host.about(UNREAD, &unit.label).is_empty(),
        "and it is not reported as unread either: the declaration reads fine"
    );
}
