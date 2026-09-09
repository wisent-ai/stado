//! The plan document: refused whole, before any of it is enqueued.
//!
//! A workload plan is one JSON object declaring the schema
//! `stado-rs/data/workloads.json` names for the kind. Every refusal here
//! arrives before a host is contacted, and the exit code is clap's usage code
//! rather than the click code a placed run fails with — the operator is being
//! told to fix the document, not the fleet.

use serde_json::json;

use crate::fixture::{refusal, said, Fleet, DEFAULT_ACTION, PLAN_SCHEMA, TARGET};

fn declared_fleet() -> Fleet {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    fleet.allowlist(&format!("{DEFAULT_ACTION}\n"));
    fleet
}

#[test]
fn a_plan_declaring_another_schema_is_refused_whole() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({ "schema": "wisent.weles-capture-plan.v1" }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "weles-browser-task plan declares schema wisent.weles-capture-plan.v1, not \
             {PLAN_SCHEMA}; fix the whole plan before any work is enqueued"
        )
    );
}

#[test]
fn the_command_will_not_run_without_the_plan_it_declares() {
    let fleet = declared_fleet();

    let out = fleet.stado(&["workload", "run", "weles-browser-task", "--target", TARGET]);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "weles-browser-task requires --plan FILE with schema {PLAN_SCHEMA}; add the plan \
             declared by stado-rs/data/workloads.json"
        )
    );
}

/// A URL carrying userinfo is refused, and the refusal must not hand the
/// password back to whatever is reading the operator's terminal.
#[test]
fn a_url_carrying_credentials_is_refused_without_echoing_them() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({ "url": "https://user:hunter2@example.com/" }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan url must be HTTP or HTTPS without embedded credentials"
    );
    assert!(
        !said(&out).contains("hunter2"),
        "the refusal echoed the credential:\n{}",
        said(&out)
    );
}

/// An all-whitespace objective is not a task. The runner trims before it
/// looks, so this also pins that a plan cannot smuggle an empty objective past
/// the check by padding it.
#[test]
fn a_blank_objective_is_no_task() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({ "objective": "   " }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "workload plan declares no objective; add it to the plan"
    );
}

/// Deferring the fills and prefilling all of them are opposite orders, and a
/// plan that gives both would be resolved by whichever branch happened to run
/// last. It is refused instead.
#[test]
fn deferring_and_prefilling_at_once_is_refused() {
    let fleet = declared_fleet();
    let plan = fleet.plan(json!({
        "allow_login": true,
        "sign_in_origin": "https://accounts.google.com",
        "sign_in_item": "weles-google-sso-login",
        "defer_fills": true,
        "prefill_all": true,
    }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(2), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        "weles-browser-task plan cannot enable both defer_fills and prefill_all; choose one"
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "a refused plan recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}
