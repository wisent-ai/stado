//! A release delivered for real, to a target it is safe to deliver to.
//!
//! The rest of this area exercises the apply path only where it must refuse,
//! because a delivery on the machine running these tests would replace the
//! operator's own Stado. `stado scratch` removes that constraint: a leased
//! target is a throwaway account on a registry host, with its own home and
//! its own emitted registry, so the delivering half runs here in full — one
//! fetch, one digest check, one staging tree, one rename, one receipt.
//!
//! Two facts about the capability were found by running it, and they shape
//! both cases below.
//!
//! `--apply` cannot bootstrap a blank machine. It delivers `host-behind` and
//! `unattested` rows, and both need a version the reporter could read, so a
//! freshly leased account reads `verdict=unknown`, `root=none`, and nothing
//! is delivered. [`Lease::bootstrap`] therefore gives the account Stado with
//! this repository's own documented installer first, which leaves exactly the
//! state the capability calls `no-delivery-history`: installed, nothing
//! staged. Everything after that is the product's own delivery.
//!
//! The declared version must be one the channel carries as a legacy
//! manifest — see [`fleet::legacy_complete`] for why a pipeline-signed
//! coordinate cannot reach a leased target — and
//! [`fleet::deliverable_versions`] asks the channel which versions qualify
//! rather than naming one here.
//!
//! Every sentence asserted below was copied from a live run on 2026-09-08.

mod fleet;
mod lease;

use fleet::{channel_state, deliverable_versions, host_turn, leasable_host};
use lease::Lease;

use crate::fixture::BINARY;

/// The delivering half, end to end: a leased account carrying an older
/// release is declared onto a newer one the channel serves, `--apply`
/// delivers it, and the machine is then read back through commands that never
/// see that apply's report.
#[test]
fn a_declared_version_the_channel_serves_is_delivered_to_a_leased_target() {
    let _turn = host_turn();
    let (target, profile, platform) = leasable_host();
    let (declared, older) = deliverable_versions(&platform);
    let mut lease = Lease::take(&target, &profile);

    lease.bootstrap(&older, &platform);
    lease.declare(&declared);

    // Installed, and provably not by the delivery path. This is the reading
    // whose own gate says "deliver with `stado release host-state --apply`".
    let (_, row) = lease.host_state(&[]);
    assert_eq!(row["installed_version"], older.as_str(), "{row}");
    assert_eq!(row["verdict"], "unattested", "{row}");
    assert_eq!(row["attestation"], "no-delivery-history", "{row}");
    assert_eq!(row["receipt"], "none", "{row}");

    let (applied, _) = lease.host_state(&["--apply"]);
    let released = Lease::released(&applied);
    assert_eq!(released["version"], declared.as_str(), "{released}");
    assert_eq!(released["status"], "completed", "{released}");
    assert_eq!(released["detail"], "released", "{released}");

    // The binary that landed under the leased account's home, and the version
    // it prints, read off the machine by `stado host inventory`.
    let installed = lease.installed();
    assert_eq!(installed["state"], "present", "{installed}");
    assert_eq!(installed["executable"], true, "{installed}");
    assert_eq!(installed["regular_file"], true, "{installed}");
    assert_eq!(installed["version_verdict"], "matched", "{installed}");
    assert!(
        installed["version"]
            .as_str()
            .unwrap_or_default()
            .starts_with(&format!("{BINARY} {declared} (")),
        "the leased machine prints the delivered version: {installed}"
    );

    // The evidence trail, read from where delivery stored it: the staged copy
    // under `$HOME/.stado/releases/<binary>/<version>/<platform>/` and the
    // `release-receipt.json` beside it. `staged-match` is the installed file
    // compared byte for byte against that staged copy, and the receipt is
    // that file's own content — neither is the apply's word for its work.
    let (state, row) = lease.host_state(&[]);
    assert_eq!(state["applied"], false, "a second read delivers nothing");
    assert_eq!(row["verdict"], "in-sync", "{row}");
    assert_eq!(row["installed_version"], declared.as_str(), "{row}");
    assert_eq!(row["declared_version"], declared.as_str(), "{row}");
    assert_eq!(row["attestation"], "staged-match", "{row}");
    let receipt = row["receipt"].as_str().unwrap_or_default();
    assert!(
        receipt.starts_with("delivered ") && receipt.contains(" by "),
        "the receipt delivery wrote says when it landed and who delivered it: {row}"
    );

    lease.destroy();
}

/// The refusal only a real delivery can produce: a declared version the
/// channel does not carry. The coordinate is resolved against the channel
/// before anything is staged, so the leased machine does not move.
#[test]
fn a_version_the_channel_does_not_carry_is_refused_and_changes_nothing() {
    let _turn = host_turn();
    let (target, profile, platform) = leasable_host();
    let (installed_version, _) = deliverable_versions(&platform);
    // The same minor line, one patch nobody has published. Derived rather
    // than invented so the case cannot be quietly outlived by a release.
    let unpublished = format!(
        "{}.999",
        installed_version
            .rsplit_once('.')
            .expect("an exact semantic version")
            .0
    );
    let manifest = format!(
        "stado://releases/{BINARY}/{unpublished}/{platform}/release-manifest-{platform}.json"
    );
    assert_eq!(
        channel_state(&manifest),
        "absent",
        "{unpublished} has to be a version the channel authoritatively lacks"
    );

    let mut lease = Lease::take(&target, &profile);
    lease.bootstrap(&installed_version, &platform);
    lease.declare(&unpublished);

    let (applied, _) = lease.host_state(&["--apply"]);
    let released = Lease::released(&applied);
    assert_eq!(released["status"], "failed", "{released}");
    let detail = released["detail"].as_str().unwrap_or_default();
    assert!(
        detail.starts_with(&format!(
            "canonical release manifests are unavailable: legacy {manifest}: "
        )),
        "the refusal names the legacy coordinate it could not read: {released}"
    );
    assert!(
        detail.contains(&format!(
            "; pipeline stado://releases/{BINARY}/{unpublished}/{platform}/release.json: "
        )),
        "and the pipeline coordinate beside it: {released}"
    );
    assert!(
        detail.contains(&format!("{{\"state\":\"absent\",\"uri\":\"{manifest}\"}}")),
        "with the channel's own authoritative absence, never a transport guess: {released}"
    );

    // Unchanged: the version the machine prints, and the absence of any
    // staged copy or receipt, are exactly what they were before the apply.
    let unchanged = lease.installed();
    assert!(
        unchanged["version"]
            .as_str()
            .unwrap_or_default()
            .starts_with(&format!("{BINARY} {installed_version} (")),
        "a refused delivery must not move the installed binary: {unchanged}"
    );
    let (_, row) = lease.host_state(&[]);
    assert_eq!(
        row["installed_version"],
        installed_version.as_str(),
        "{row}"
    );
    assert_eq!(row["attestation"], "no-delivery-history", "{row}");
    assert_eq!(row["receipt"], "none", "{row}");

    lease.destroy();
}
