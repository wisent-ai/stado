//! Which paths a cleanup pass may measure and remove, proved on this machine.
//!
//! # What this area used to be
//!
//! `disk_scope` existed because `stado host gates lukasz-macbook`, run on
//! `lukasz-macbook`, died with `Command '['/bin/bash', '-s']' timed out after
//! 120 seconds`: the gate read was computing a `du` inventory that no gate
//! field consumes, so `disk_cleanup_stalled` and `cleanup_success_age_seconds`
//! were unobtainable on the machine the command was about. The fix scoped the
//! remote script. The cases that guarded it compared `remote_script_for`'s
//! output strings to each other — a subsequence check, a byte-identity check,
//! a `set -u` header check — so the area could pass with a product whose
//! commands never ran at all, and it never once created a file or read one
//! back. Four of those cases are gone; the rest are rebuilt here against the
//! binary, and `reads.rs` states the same two properties as facts an operator
//! can see: the gate read answers on the host it is about, and the two scopes
//! report the same measurement.
//!
//! # What it is now
//!
//! Every case declares an isolated local registry whose one target names THIS
//! machine, seeds real directories with real payload bytes inside a tempdir it
//! owns, drives the command an operator reaches for — `stado disk-cleanup`,
//! `stado space report`, `stado host gates` — and then reads the filesystem:
//! which trees are gone, which are still there with their payload intact, what
//! the state document says, and whether the byte figures agree with `stat` and
//! `du`. The only directory any pass here is allowed to reclaim from is the
//! cleaner root the case created, so a cleaner that ever widened its scope
//! fails a case instead of deleting an operator's work.

mod budget;
mod fixture;
mod gates;
mod native;
mod reads;

use std::fs;

use fixture::{build_caches, payload_kept, Host, CACHE_TAG_NAME, PAYLOAD_NAME};
use native::{allocated_bytes, du_bytes};

/// Payload sizes for the scopes these cases create. Small enough to write
/// quickly, large enough that `stat` and `du` report whole allocation blocks
/// rather than a rounding artefact.
const CACHE_MIB: usize = 6;
const SMALL_MIB: usize = 3;

/// The outcome a pass reports when it reclaimed something, and the mode a
/// preview pins itself to. Both copied from live runs of the built binary
/// against this fixture.
const RECLAIMED: &str = "reclaimed_progress";
const ENFORCE: &str = "enforce";
const REPORT: &str = "report";

/// A pass removes the declared scope, charges exactly the bytes the operating
/// system says that tree held, and leaves an identical tree outside the
/// declared root untouched.
///
/// The neighbour is the point: it carries the same payload, the same cache tag
/// and the same age, one directory away from the cleaner root. Only the
/// declaration separates them, so a cleaner that walked `$HOME` instead of the
/// root it was given fails here.
#[test]
fn a_pass_removes_the_declared_scope_and_nothing_beside_it() {
    let host = Host::new();
    let inside = host.seed_tree(&host.cache_root, "target-tree", CACHE_MIB, true, true);
    let outside_root = host.under_home("not-declared");
    fs::create_dir_all(&outside_root).expect("create the directory outside the declared scope");
    let outside = host.seed_tree(&outside_root, "target-tree", CACHE_MIB, true, true);
    let charged = allocated_bytes(&inside);
    let neighbour_bytes = du_bytes(&outside);
    assert!(charged > 0, "the fixture wrote no payload");

    let report = host.cleanup_pass(&["disk-cleanup", "--once"]);
    assert_eq!(report["mode"], ENFORCE);
    assert_eq!(report["outcome"], RECLAIMED);
    let cleaner = build_caches(&report);
    assert_eq!(cleaner["scanned_items"].as_i64(), Some(1));
    assert_eq!(cleaner["eligible_items"].as_i64(), Some(1));
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(1));
    assert_eq!(
        cleaner["expected_bytes"].as_i64(),
        Some(charged),
        "the pass charged {} bytes; stat says the tree held {charged}",
        cleaner["expected_bytes"]
    );

    assert!(!inside.exists(), "the pass left the declared scope behind");
    assert!(
        host.cache_root.is_dir(),
        "the pass removed the declared root itself"
    );
    assert!(
        payload_kept(&outside),
        "a tree outside the declared root was reclaimed: {}",
        outside.display()
    );
    assert_eq!(
        du_bytes(&outside),
        neighbour_bytes,
        "the neighbour outside the declared root changed size"
    );
    assert!(
        outside.join(CACHE_TAG_NAME).is_file(),
        "the neighbour's cache tag was removed"
    );

    // The pass has to record what it did where the gate read looks for it.
    let state = host
        .janitor_state()
        .expect("the pass wrote its state document");
    let recorded = build_caches(&state["report"]);
    assert_eq!(recorded["expected_bytes"].as_i64(), Some(charged));
    assert_eq!(recorded["deleted_items"].as_i64(), Some(1));
    assert!(
        state["report"]["free_bytes_after"]
            .as_i64()
            .expect("the pass measured free space after itself")
            > 0
    );
}

/// A preview measures the same scope and removes nothing — not the tree, not
/// its payload, and not even a state document — and the pass that follows
/// charges the same bytes the preview reported.
#[test]
fn a_preview_measures_the_scope_and_removes_nothing() {
    let host = Host::new();
    let tree = host.seed_tree(&host.cache_root, "target-tree", CACHE_MIB, true, true);
    let charged = allocated_bytes(&tree);
    let occupied = du_bytes(&tree);
    assert!(charged > 0, "the fixture wrote no payload");

    let preview = host.cleanup_pass(&["disk-cleanup", "--dry-run"]);
    assert_eq!(preview["mode"], REPORT);
    let previewed = build_caches(&preview);
    assert_eq!(previewed["eligible_items"].as_i64(), Some(1));
    assert_eq!(previewed["deleted_items"].as_i64(), Some(0));
    assert_eq!(previewed["expected_bytes"].as_i64(), Some(charged));

    assert!(payload_kept(&tree), "the preview removed the payload");
    assert_eq!(
        du_bytes(&tree),
        occupied,
        "the preview changed what the tree occupies"
    );
    assert!(
        host.janitor_state().is_none(),
        "the preview wrote a state document, so a later pass would read a \
         success that never happened"
    );

    let report = host.cleanup_pass(&["disk-cleanup", "--once"]);
    let cleaner = build_caches(&report);
    assert_eq!(
        cleaner["expected_bytes"].as_i64(),
        Some(charged),
        "the applying pass charged {} for a tree the preview measured at {charged}",
        cleaner["expected_bytes"]
    );
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(1));
    assert!(!tree.exists(), "the applying pass left the tree behind");
}

/// Inside the declared root, only a directory its own build tool tagged and
/// old enough to pass the declared age gate is removed. The other two are
/// measured — they are counted as scanned — and kept.
///
/// The distinction between them is deliberate: the young one is a candidate
/// the age gate refused, so it appears as a `too_young` skip, while the
/// untagged one is not a candidate at all and produces no skip entry. A
/// cleaner that treated a plausible directory name as permission would remove
/// it, because nothing but the tag separates it from the tree that goes.
#[test]
fn an_untagged_or_young_directory_is_measured_and_kept() {
    let host = Host::new();
    let removable = host.seed_tree(&host.cache_root, "aged-tagged", CACHE_MIB, true, true);
    let young = host.seed_tree(&host.cache_root, "young-tagged", SMALL_MIB, true, false);
    let untagged = host.seed_tree(&host.cache_root, "aged-untagged", SMALL_MIB, false, true);
    let charged = allocated_bytes(&removable);

    let report = host.cleanup_pass(&["disk-cleanup", "--once"]);
    let cleaner = build_caches(&report);
    assert_eq!(
        cleaner["scanned_items"].as_i64(),
        Some(3),
        "the pass measured {} directories under the declared root, not the \
         three the case created: {cleaner}",
        cleaner["scanned_items"]
    );
    assert_eq!(cleaner["eligible_items"].as_i64(), Some(1));
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(1));
    assert_eq!(
        cleaner["expected_bytes"].as_i64(),
        Some(charged),
        "the pass charged bytes for a tree it was not allowed to remove: {cleaner}"
    );
    assert_eq!(cleaner["skipped"]["too_young"].as_i64(), Some(1));
    assert_eq!(
        cleaner["skipped"].as_object().map(serde_json::Map::len),
        Some(1),
        "an untagged directory must not be a candidate at all: {cleaner}"
    );

    assert!(!removable.exists(), "the eligible tree was kept");
    assert!(
        payload_kept(&young),
        "a cache younger than the declared age gate was removed"
    );
    assert!(
        payload_kept(&untagged),
        "an untagged directory was removed: a name is not permission"
    );
    assert!(
        !untagged.join(CACHE_TAG_NAME).exists(),
        "the fixture tagged the directory it meant to leave untagged"
    );
    assert_eq!(
        fs::metadata(young.join(PAYLOAD_NAME))
            .expect("the young cache's payload is still there")
            .len(),
        (SMALL_MIB as u64) << 20
    );
}
