//! `stado availability check` asks every component the catalog declares and
//! publishes what each answered; `show` reads it back.
//!
//! Against the real endpoints the compiled catalog declares (`public_status`
//! of each product):
//!
//! - `show` before any check is refused, naming that nothing is published;
//! - `check --json` publishes one row per declared component, each `up` or
//!   `down` with the answer that says so, `since` the first check, and a down
//!   row's `last_failure` carrying that check's error;
//! - a second check keeps `since` for every component whose state did not
//!   change, and `show --json` answers exactly what the last check published;
//! - the plain `show` names every component on its own line.
//!
//! A component this deployment cannot reach (a vault field, a forward marker
//! this home does not hold) is reported down with that cause: the journey
//! requires each row to say why, not that the fleet is healthy.
use serde_json::Value;

use crate::deployment::{declared, row, Deployment};

/// A row of the first check: down with its failure, or up with none.
fn first_look(row: &Value, checked_at: &str) {
    assert_eq!(row["since"], checked_at, "{row}");
    assert!(
        row["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty()),
        "the answer that says so: {row}"
    );
    if row["state"] == "down" {
        assert_eq!(row["last_failure"]["at"], checked_at, "{row}");
        assert_eq!(row["last_failure"]["error"], row["detail"], "{row}");
        return;
    }
    assert_eq!(row["state"], "up", "{row}");
    assert!(row["last_failure"].is_null(), "{row}");
}

#[test]
fn every_declared_component_is_checked_published_and_read_back() {
    let mut deployment = Deployment::start("check");

    let unpublished = deployment.run(&["availability", "show", "--json"]);
    assert!(!unpublished.status.success(), "{unpublished:?}");
    assert!(
        String::from_utf8_lossy(&unpublished.stderr).contains("no status is published"),
        "{unpublished:?}"
    );

    let first = deployment.json(&["availability", "check", "--json"]);
    let checked_at = first["checked_at"]
        .as_str()
        .expect("the check is dated")
        .to_owned();
    for (product, component, _) in declared() {
        first_look(row(&first, &product, &component), &checked_at);
    }

    let second = deployment.json(&["availability", "check", "--json"]);
    for (product, component, _) in declared() {
        let before = row(&first, &product, &component);
        let after = row(&second, &product, &component);
        if before["state"] == after["state"] {
            assert_eq!(after["since"], before["since"], "{before} then {after}");
        } else {
            assert_eq!(
                after["since"], second["checked_at"],
                "{before} then {after}"
            );
        }
    }

    let shown = deployment.json(&["availability", "show", "--json"]);
    assert_eq!(shown, second);

    let plain = deployment.run(&["availability", "show"]);
    assert!(plain.status.success(), "{plain:?}");
    let plain = String::from_utf8_lossy(&plain.stdout).into_owned();
    for (product, component, _) in declared() {
        assert!(
            plain.contains(&format!("({product}/{component})")),
            "{product}/{component} is named: {plain}"
        );
    }
    deployment.passed();
}
