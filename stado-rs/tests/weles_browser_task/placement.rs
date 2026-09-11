//! Placement: which host the fleet will put this work on, decided from the
//! registry row alone, before the plan reaches any host.
//!
//! `stado-rs/data/work/workloads.json` declares this workload's product as
//! `weles-worker` and its registry allowance as `$plan.action`, so a row that
//! carries no `weles` key declares no browser task at all, and a row that
//! carries one declares only the actions it lists. Both refusals happen before
//! a channel is opened.

use serde_json::json;

use crate::fixture::{refusal, said, Fleet, CAPTURE_ACTION, DEFAULT_ACTION, TARGET};

#[test]
fn a_host_whose_registry_row_declares_no_weles_actions_is_refused_by_name() {
    let fleet = Fleet::without_weles("noweles");
    let plan = fleet.plan(json!({}));

    let out = fleet.run(Some("noweles"), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "noweles declares no weles-browser-task; add it to stado-rs/data/work/workloads.json"
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "a refused placement recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}

#[test]
fn a_fleet_with_no_weles_host_refuses_rather_than_picking_one_anyway() {
    let fleet = Fleet::without_weles("noweles");
    let plan = fleet.plan(json!({}));

    // No `--target`: the product picks, and there is nothing eligible to pick.
    let out = fleet.run(None, &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "the fleet declares no weles-browser-task; add it to stado-rs/data/work/workloads.json"
    );
}

/// The allowance is the plan's own action, so a host declared for browser work
/// is still not declared for an action its row does not list. This is the
/// refusal that keeps `weles-capture`'s hard-coded `generic_capture` off a
/// worker that never accepted it.
#[test]
fn an_action_the_registry_row_does_not_list_is_refused_before_the_host_is_touched() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    // A catalog that WOULD admit the action, so only the registry row can be
    // what refuses it.
    fleet.allowlist(&format!("{DEFAULT_ACTION}\n{CAPTURE_ACTION}\n"));
    let plan = fleet.plan(json!({ "action": CAPTURE_ACTION }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET} declares no weles-browser-task; add it to stado-rs/data/work/workloads.json"
        )
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "a refused placement recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}
