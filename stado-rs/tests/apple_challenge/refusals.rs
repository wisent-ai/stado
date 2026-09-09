//! What the product does before it prepares anything: which host it will place
//! the preparation on, which plan it accepts, and what it reports for a
//! declared Darwin ARM64 host it cannot reach.
//!
//! None of these cases can start an Apple authentication, a two-factor prompt
//! or a notification, because none of them reaches a machine: every declared
//! destination is a name RFC 2606 reserves, and the refusals below all arrive
//! before the first byte leaves this process.

use serde_json::json;

use crate::fixture::{
    registry, report, said, stderr, target, Fixture, APPLE_HOST, APPLE_PLATFORM, DECLARATION, KIND,
    OTHER_HOST, OTHER_PLATFORM, PLAN_SCHEMA, UNDECLARED_HOST,
};

/// No Darwin ARM64 host is registered, so there is nowhere to prepare. The
/// refusal names the workload and the declaration that would add it, and the
/// registry it read is untouched.
#[test]
fn a_fleet_with_no_darwin_arm64_host_refuses_the_preparation_and_names_the_declaration() {
    let fixture = Fixture::new(&registry(vec![target(OTHER_HOST, OTHER_PLATFORM)]));
    let refused = fixture.stado(&["workload", "status", KIND, "--json"]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "the fleet declares no {KIND}; add it to {DECLARATION}"
        )),
        "the refusal must name the workload and where it is declared: {}",
        said(&refused),
    );
    assert!(
        fixture.registry_unchanged(),
        "a refused read rewrote the registry it read",
    );
    assert!(
        !fixture.gui_cache().exists(),
        "a refused read left GUI-automation state behind at {}",
        fixture.gui_cache().display(),
    );
}

/// A host is named, and it is the wrong platform. The refusal names that host
/// rather than the fleet, because the operator asked for one machine.
#[test]
fn a_named_host_of_another_platform_is_refused_by_its_own_name() {
    let fixture = Fixture::new(&registry(vec![
        target(OTHER_HOST, OTHER_PLATFORM),
        target(APPLE_HOST, APPLE_PLATFORM),
    ]));
    let refused = fixture.stado(&["workload", "status", KIND, "--target", OTHER_HOST, "--json"]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "{OTHER_HOST} declares no {KIND}; add it to {DECLARATION}"
        )),
        "the refusal must name the host that cannot carry the work: {}",
        said(&refused),
    );
    assert!(fixture.registry_unchanged());
}

/// A name the registry does not hold is refused as an undeclared target, not
/// as an unreachable host: the two send an operator to different places.
#[test]
fn a_target_the_registry_does_not_declare_is_refused_as_undeclared() {
    let fixture = Fixture::new(&registry(vec![target(APPLE_HOST, APPLE_PLATFORM)]));
    let refused = fixture.stado(&[
        "workload",
        "status",
        KIND,
        "--target",
        UNDECLARED_HOST,
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(1), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "target '{UNDECLARED_HOST}' is not declared; add it to the canonical registry"
        )),
        "the refusal must say the name is not declared: {}",
        said(&refused),
    );
    assert!(fixture.registry_unchanged());
}

/// A plan that does not declare the schema the workload publishes is refused
/// whole, before placement: the wrong-platform host below would otherwise have
/// produced a different complaint, and it does not.
#[test]
fn a_plan_without_the_declared_schema_is_refused_before_any_work_is_enqueued() {
    let fixture = Fixture::new(&registry(vec![target(OTHER_HOST, OTHER_PLATFORM)]));
    let plan = fixture.plan(&json!({"operation": "grant-accessibility", "apple_only": true}));
    let refused = fixture.stado(&[
        "workload",
        "run",
        KIND,
        "--target",
        OTHER_HOST,
        "--plan",
        plan.to_str().expect("a UTF-8 plan path"),
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(2), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(&format!(
            "{KIND} plan declares schema missing, not {PLAN_SCHEMA}; \
             fix the whole plan before any work is enqueued"
        )),
        "the refusal must name the schema it wanted: {}",
        said(&refused),
    );
    assert!(fixture.registry_unchanged());
}

/// The operation is the one field that decides whether a preparation mutates a
/// machine. An operation the workload does not declare is refused with the
/// three it does, and nothing on the host is asked for.
#[test]
fn an_operation_the_workload_does_not_declare_is_refused_with_the_ones_it_does() {
    let fixture = Fixture::new(&registry(vec![target(APPLE_HOST, APPLE_PLATFORM)]));
    let plan = fixture.plan(&json!({
        "schema": PLAN_SCHEMA,
        "operation": "not-a-declared-gui-automation-operation",
    }));
    let refused = fixture.stado(&[
        "workload",
        "run",
        KIND,
        "--target",
        APPLE_HOST,
        "--plan",
        plan.to_str().expect("a UTF-8 plan path"),
        "--json",
    ]);

    assert_eq!(refused.status.code(), Some(2), "{}", said(&refused));
    assert!(
        stderr(&refused).contains(
            "gui-automation plan operation 'not-a-declared-gui-automation-operation' is not \
             enable, disable, or grant-accessibility; fix the plan"
        ),
        "the refusal must offer the operations that exist: {}",
        said(&refused),
    );
    assert!(fixture.registry_unchanged());
    assert!(
        !fixture.gui_cache().exists(),
        "a refused plan left GUI-automation state behind",
    );
}

/// A declared Darwin ARM64 host the product cannot reach: the report is still a
/// complete document naming the target and the destination it would have used,
/// it carries no state items because none could be read, and its error is the
/// brokered key the product could not open. The exit is a failure, so no caller
/// can mistake this for a prepared host.
#[test]
fn a_declared_apple_host_it_cannot_reach_reports_the_destination_and_prepares_nothing() {
    let fixture = Fixture::new(&registry(vec![target(APPLE_HOST, APPLE_PLATFORM)]));
    let observed = fixture.stado(&["workload", "status", KIND, "--target", APPLE_HOST, "--json"]);

    assert_eq!(observed.status.code(), Some(1), "{}", said(&observed));
    let document = report(&observed);
    assert_eq!(document["target"], APPLE_HOST);
    assert_eq!(
        document["ssh_target"],
        format!("nobody@{APPLE_HOST}.invalid"),
        "the report must name the destination it would have used: {document}",
    );
    assert_eq!(
        document["items"],
        json!([]),
        "a host that answered nothing must publish no state: {document}",
    );
    let error = document["error"]
        .as_str()
        .expect("an unreachable host reports why");
    assert!(
        error.contains(&format!("stado-ssh-{APPLE_HOST}")) && error.contains("private_key"),
        "the error must name the brokered host key it could not read: {document}",
    );
    assert!(fixture.registry_unchanged());
    assert!(!fixture.gui_cache().exists());
}

/// The same host, asked for the Apple-only preparation itself rather than a
/// read. It stops at exactly the same wall, with the same document, and leaves
/// this machine's home directory holding nothing but what the fixture wrote —
/// which is what makes this case safe to run by default: it cannot reach a
/// machine, so it cannot raise an Apple prompt.
#[test]
fn the_apple_only_preparation_of_an_unreachable_host_changes_nothing_anywhere() {
    let fixture = Fixture::new(&registry(vec![target(APPLE_HOST, APPLE_PLATFORM)]));
    let plan = fixture.plan(&json!({
        "schema": PLAN_SCHEMA,
        "operation": "grant-accessibility",
        "apple_only": true,
    }));
    let attempted = fixture.stado(&[
        "workload",
        "run",
        KIND,
        "--target",
        APPLE_HOST,
        "--plan",
        plan.to_str().expect("a UTF-8 plan path"),
        "--json",
    ]);

    assert_eq!(attempted.status.code(), Some(1), "{}", said(&attempted));
    let document = report(&attempted);
    assert_eq!(document["target"], APPLE_HOST);
    assert_eq!(document["items"], json!([]), "{document}");
    assert!(
        document["error"]
            .as_str()
            .expect("an unreachable preparation reports why")
            .contains("private_key"),
        "{document}",
    );
    assert!(fixture.registry_unchanged());
    assert!(!fixture.gui_cache().exists());
    assert!(
        !fixture.home().join("Library").exists(),
        "a preparation that reached no machine touched this machine's own GUI state",
    );
}
