//! The two journeys that must not spend a builder at all: a build with no room
//! for its own output, and a recipe key this Stado does not know.
//!
//! Both refuse while reading the declaration, before a job is queued, because a
//! journey that queues first pays a builder to discover what the manifest
//! already said - and the failure it pays for is readable only as a linker
//! error inside a thirty kilobyte log.

use super::super::*;
use super::world::{declare, said_by, signed_fleet, submit, workspace};

/// readable only as a linker error inside a 30 KB log.
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_build_with_no_room_is_refused_before_its_first_gate() {
    let platform = release_platform();
    let (home, storage, source) = workspace("release-no-room-", platform);
    // The one difference from every other journey here: this product declares
    // more free space than any volume has.
    declare(&source, platform, "min_free_gb", json!(99_999_999_u64));
    let vault = signed_fleet(home.path(), &storage, platform, true);
    let mut agent = claiming_agent(home.path(), &storage, &vault);

    let mut running = Running(submit(home.path(), &storage, &vault, &source, "submit"));
    let status = wait_for_submit(&mut running.0, &mut agent.0, home.path(), &storage, &vault);
    drop(agent);
    let reported = said_by(home.path(), "submit");
    assert!(
        !status.success(),
        "a build with no room reported success: {reported}"
    );
    assert!(
        reported.contains("this build needs 99999999 GiB free on"),
        "the refusal did not name the declared requirement: {reported}"
    );
    assert!(
        !reported.contains("Compiling ci-release-probe"),
        "the build ran a gate before the room was checked: {reported}"
    );
    println!("verified the no-room refusal platform={platform}");
}

/// A recipe key this Stado does not know is kept, then refused by name.
///
/// Declaring `min_free_gb` in the same commit as its reader failed stado
/// 0.20.4 on both platforms with serde's "unknown field", because a release is
/// built by the binary a host already has and that binary denied the key
/// before compiling a single crate. Unknown keys are therefore tolerated by
/// the contract — and named by the submitting binary, so a typo never reaches
/// the queue.
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn an_unknown_recipe_key_is_refused_by_name_before_a_job_is_queued() {
    let platform = release_platform();
    let (home, storage, source) = workspace("release-unknown-key-", platform);
    declare(&source, platform, "min_fee_gb", json!(20));
    let vault = signed_fleet(home.path(), &storage, platform, false);

    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let refused = command
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--channel",
            "candidate",
        ])
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "the manifest was accepted: {said}"
    );
    assert!(
        said.contains("unknown recipe keys for this Stado: min_fee_gb"),
        "the refusal did not name the key: {said}"
    );
    // Nothing was queued: the refusal happened while reading the source.
    assert!(
        !storage.join("queue").exists(),
        "a job was queued for a manifest that was refused:\n{}",
        store_snapshot(&storage)
    );
    println!("verified the unknown-key refusal platform={platform}");
}
