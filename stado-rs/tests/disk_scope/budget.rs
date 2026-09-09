//! How much one pass is allowed to remove, and what it leaves for the next.
//!
//! The declared budgets are the other half of scope: a pass that ignored them
//! would empty a cache root in one sweep on a host whose disk pressure is
//! being managed deliberately. Both cases here seed three identical eligible
//! trees, declare a budget that stops the pass after the first, and then read
//! the filesystem: exactly one tree is gone, two still hold their payload, and
//! the pass reports which limit stopped it and how many directories it
//! declined for that reason.

use crate::fixture::{build_caches, payload_kept, Host, MANY_ITEMS, WHOLE_GIB};
use crate::native::allocated_bytes;
use crate::CACHE_MIB;

/// A budget smaller than one seeded tree, so the pass's first removal already
/// reaches it and every later candidate is declined. Declared registry tuning,
/// not a threshold this test invents: the janitor compares the bytes it has
/// already deleted against exactly this figure.
const HALF_A_TREE_BYTES: u64 = (CACHE_MIB as u64) << 19;
/// One item per pass: the same stop, expressed as a count.
const ONE_ITEM: u32 = 1;

/// Three identical eligible trees, in the order the cleaner walks them.
const TREES: [&str; 3] = ["a-tree", "b-tree", "c-tree"];

/// A pass stops at the declared byte budget, names the limit it hit, and
/// leaves the rest of the scope on disk.
#[test]
fn a_pass_stops_at_the_declared_byte_budget() {
    let host = Host::new();
    host.declare(HALF_A_TREE_BYTES, MANY_ITEMS);
    let trees: Vec<_> = TREES
        .iter()
        .map(|name| host.seed_tree(&host.cache_root, name, CACHE_MIB, true, true))
        .collect();
    let eligible_bytes: i64 = trees.iter().map(|tree| allocated_bytes(tree)).sum();

    let report = host.cleanup_pass(&["disk-cleanup", "--once"]);
    assert_eq!(
        report["caps"]["bytes"], true,
        "the pass did not report that the byte budget stopped it: {report}"
    );
    let cleaner = build_caches(&report);
    assert_eq!(cleaner["scanned_items"].as_i64(), Some(TREES.len() as i64));
    assert_eq!(cleaner["eligible_items"].as_i64(), Some(TREES.len() as i64));
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(1));
    assert_eq!(
        cleaner["skipped"]["byte_cap"].as_i64(),
        Some(TREES.len() as i64 - 1),
        "every candidate past the budget must be reported as declined for it: {cleaner}"
    );
    // The reported figure is what the whole eligible scope holds, so an
    // operator reading a capped pass still learns what is left to reclaim.
    assert_eq!(
        cleaner["expected_bytes"].as_i64(),
        Some(eligible_bytes),
        "the pass reported {} bytes for a scope stat says holds {eligible_bytes}",
        cleaner["expected_bytes"]
    );

    let survivors: Vec<_> = trees.iter().filter(|tree| payload_kept(tree)).collect();
    assert_eq!(
        survivors.len(),
        TREES.len() - 1,
        "the capped pass removed {} of {} trees",
        TREES.len() - survivors.len(),
        TREES.len()
    );
    assert!(
        !trees[0].exists(),
        "the pass reported one removal but the first tree is still there"
    );
    assert!(
        host.janitor_state()
            .expect("the capped pass still wrote its state document")["report"]["caps"]["bytes"]
            == true,
        "the state document must carry the limit the pass hit"
    );
}

/// The same stop expressed as an item count: one item per pass leaves the rest
/// of the scope on disk and says so.
#[test]
fn a_pass_stops_at_the_declared_item_budget() {
    let host = Host::new();
    host.declare(WHOLE_GIB, ONE_ITEM);
    let trees: Vec<_> = TREES
        .iter()
        .map(|name| host.seed_tree(&host.cache_root, name, CACHE_MIB, true, true))
        .collect();

    let report = host.cleanup_pass(&["disk-cleanup", "--once"]);
    assert_eq!(
        report["caps"]["items"], true,
        "the pass did not report that the item budget stopped it: {report}"
    );
    assert_eq!(
        report["caps"]["bytes"], false,
        "a pass under a whole-gibibyte budget must not report a byte cap: {report}"
    );
    let cleaner = build_caches(&report);
    assert_eq!(cleaner["deleted_items"].as_i64(), Some(i64::from(ONE_ITEM)));
    assert_eq!(
        cleaner["skipped"]["item_cap"].as_i64(),
        Some(TREES.len() as i64 - i64::from(ONE_ITEM))
    );

    assert!(
        !trees[0].exists(),
        "the one permitted removal did not happen"
    );
    for tree in &trees[1..] {
        assert!(
            payload_kept(tree),
            "a pass allowed one item removed {}",
            tree.display()
        );
    }
}
