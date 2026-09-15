//! Install a signed published release into a blank disposable account, then
//! prove a missing release cannot replace those installed bytes. The product
//! owns first installation, verification and teardown; no bootstrap script or
//! operator installation stands in for any part of this story.

mod fleet;
mod lease;

use crate::fixture::{stderr, BINARY};
use fleet::{channel_state, deliverable_version, host_turn, leasable_host};
use lease::Lease;

#[test]
fn a_signed_release_is_delivered_and_an_absent_version_cannot_replace_it() {
    let _turn = host_turn();
    let identity = fleet::fleet(&["--version"]);
    assert!(
        identity.status.success(),
        "the product identity must be recorded before a real release test"
    );
    let (target, profile, platform) = leasable_host();
    let version = deliverable_version(&platform);
    let mut lease = Lease::take(&target, &profile);
    lease.declare(&version);

    let (before, state, row) = lease.host_state(&[]);
    assert!(
        !before.status.success(),
        "a declared missing binary must not read as installed: {state}"
    );
    assert_eq!(row["verdict"], "host-missing", "{row}");
    let (installed, applied, _) = lease.host_state(&["--apply"]);
    assert!(
        installed.status.success(),
        "{}\n{applied}",
        stderr(&installed)
    );
    let delivery = Lease::released(&applied);
    assert_eq!(delivery["version"], version.as_str(), "{delivery}");
    assert_eq!(delivery["status"], "completed", "{delivery}");

    let inventory = lease.installed();
    assert_eq!(inventory["state"], "present", "{inventory}");
    assert_eq!(inventory["executable"], true, "{inventory}");
    assert_eq!(inventory["regular_file"], true, "{inventory}");
    assert!(
        inventory["version"]
            .as_str()
            .unwrap_or_default()
            .starts_with(&format!("{BINARY} {version} (")),
        "{inventory}"
    );
    let (read, state, row) = lease.host_state(&[]);
    assert!(read.status.success(), "{}\n{state}", stderr(&read));
    assert_eq!(row["verdict"], "in-sync", "{row}");
    assert_eq!(row["installed_version"], version.as_str(), "{row}");
    assert_eq!(row["attestation"], "staged-match", "{row}");
    let installed_receipt = row["receipt"].clone();

    let major = version.split('.').next().unwrap().parse::<u64>().unwrap();
    let absent = format!("{}.0.0", major.checked_add(1).unwrap());
    let manifest = format!("stado://releases/{BINARY}/{absent}/{platform}/release.json");
    assert_eq!(
        channel_state(&manifest),
        "absent",
        "the refusal needs an authoritatively absent coordinate"
    );
    lease.declare(&absent);
    let (refused, refusal, _) = lease.host_state(&["--apply"]);
    assert!(
        !refused.status.success(),
        "the unavailable release was accepted: {refusal}"
    );
    assert!(
        format!("{} {refusal}", stderr(&refused)).contains(&absent),
        "the refusal must identify the requested version: {refusal}"
    );
    let unchanged = lease.installed();
    assert_eq!(unchanged["version"], inventory["version"], "{unchanged}");
    let (_, _, row) = lease.host_state(&[]);
    assert_eq!(row["installed_version"], version.as_str(), "{row}");
    assert_eq!(row["attestation"], "staged-match", "{row}");
    assert_eq!(
        row["receipt"], installed_receipt,
        "a refused release changed delivery evidence"
    );

    lease.destroy();
}
