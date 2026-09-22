//! The park time an attach hands to its runtime.
//!
//! An attach holds the kind's reservation for exactly as long as `jeden rpc`
//! lives, and on 2026-09-22 eight idle Jeden Desktop tabs held 16 cores and
//! 32 GiB of a 12-core laptop for 26 hours. The runtime parks itself when
//! nothing runs, but only when the attach tells it how long nothing may run.
//! The value is read back through `stado workload list --json`, never
//! restated here, so the declaration and the hand-off cannot drift apart.

use std::process::Output;

use serde_json::Value;

use super::harness::{said, Area};

pub fn assert_declared_park_time_was_handed_over(area: &Area, attached: &Output) {
    let listed = area.stado(&["workload", "list", "--json"]);
    assert!(listed.status.success(), "{}", said(&listed.stderr));
    let catalog: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let declared = catalog["workloads"]
        .as_array()
        .and_then(|kinds| kinds.iter().find(|kind| kind["kind"] == "jeden-session"))
        .map(|kind| kind["park_after_seconds"].clone())
        .expect("the catalog lists jeden-session");
    assert!(
        declared.as_u64().is_some_and(|seconds| seconds > 0),
        "jeden-session declares no park time: {catalog}"
    );
    let placement: Value = said(&attached.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("STADO_JEDEN_PLACEMENT "))
        .map(|line| serde_json::from_str(line).expect("the placement line is JSON"))
        .unwrap_or_else(|| panic!("no placement line: {}", said(&attached.stderr)));
    assert_eq!(
        placement["park_after_seconds"], declared,
        "the attach did not hand the declared park time to the runtime: {placement}"
    );
}
