//! What `stado placement holds` says while no move is open: the declared
//! profile's host holds it and is not emptied, the destination holds
//! nothing, an undeclared host is refused, and every host's row names the
//! release-control products rolled out there and why its fence is unread.

use serde_json::{json, Value};

use super::DESTINATION;
use crate::deployment::{Deployment, PLACED_ON, PROFILE};

pub(super) fn name_what_each_host_holds(deployment: &Deployment) {
    let held: Value =
        serde_json::from_str(&deployment.cli(&["placement", "holds", PLACED_ON, "--json"]))
            .unwrap();
    assert_eq!(held["emptied"], false, "{held}");
    let placed: Vec<&str> = held["profiles"]
        .as_array()
        .expect("holds lists its profiles")
        .iter()
        .filter_map(|profile| profile["profile"].as_str())
        .collect();
    assert_eq!(placed, [PROFILE], "{held}");
    let empty: Value =
        serde_json::from_str(&deployment.cli(&["placement", "holds", DESTINATION, "--json"]))
            .unwrap();
    assert_eq!(empty["profiles"], json!([]), "{empty}");
    // The schedules pinned to a host are either listed or said unread with
    // the store's cause, never both and never neither.
    assert!(
        empty["pinned_schedules"].is_array() != empty["pinned_schedules_unread"].is_string(),
        "{empty}"
    );
    deployment.refused(&["placement", "holds", "no-such-host"], "no-such-host");
    let everywhere: Value =
        serde_json::from_str(&deployment.cli(&["placement", "holds", "--json"])).unwrap();
    let rows = everywhere["hosts"]
        .as_array()
        .expect("holds lists every host");
    let placed_row = rows
        .iter()
        .find(|row| row["host"] == PLACED_ON)
        .expect("the placed host is listed");
    assert_eq!(placed_row["profiles"], json!([PROFILE]), "{everywhere}");
    assert_eq!(placed_row["emptied"], false, "{everywhere}");
    // Each row names exactly the release-control products whose targets
    // name that host, as the registry holds them.
    let registry = deployment.registry();
    for row in rows {
        let host = row["host"].as_str().expect("a row names its host");
        let expected: Vec<&String> = registry["release_control"]["products"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, product)| product["targets"].get(host).is_some())
            .map(|(name, _)| name)
            .collect();
        assert_eq!(row["rolled_out_by"], json!(expected), "{everywhere}");
        assert!(
            row["pinned_schedules"].is_array() != row["pinned_schedules_unread"].is_string(),
            "{everywhere}"
        );
        // No template host answers a fence read; each says why, never "not fenced".
        assert!(
            row["store_fence_unread"].is_string() && row["store_fence"].is_null(),
            "{everywhere}"
        );
    }
}
