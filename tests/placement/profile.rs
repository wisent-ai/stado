//! `stado placement profile`: declare, list, show and remove a placement
//! profile against an isolated deployment's own registry store.
//!
//! One isolated deployment (its own HOME, config and local store) imports the
//! shared directory template, where `elsewhere-api` is a fixed route placed on
//! `other-host`. Expected: a profile naming that service and only its placed
//! host is declared, the registry then holds it, the route names it, carries
//! no `managed_service`, and the directory generation advanced; `list` and
//! `show` print it. Refusals, each leaving the registry as it was: a service
//! the directory does not declare, the same service in a second profile, a
//! redeclaration dropping a held service, a profile that does not parse, and
//! removing a profile the registry does not hold. `remove` turns the route
//! back into a fixed route naming the unit on its active host and advances
//! the generation again.
//!
//! This file is the root of the placement test binary: the open-move journey
//! (`transactions`) and the fixed-route draft with the role moves (`draft`)
//! run in it too, each against its own isolated deployment.
mod deployment;
mod draft;
mod transactions;

use serde_json::{json, Value};

use crate::deployment::{generation, profile, Deployment, PLACED_ON, PROFILE, SERVICE};

#[test]
fn a_placement_profile_is_declared_linked_listed_refused_and_removed() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let before = deployment.registry();

    let declared = profile(&before, PROFILE, &[SERVICE]);
    let declared = deployment.write_profile("profile.json", &declared);
    let receipt: Value = serde_json::from_str(&deployment.cli(&[
        "placement",
        "profile",
        "declare",
        "--file",
        &declared,
        "--json",
    ]))
    .unwrap();
    assert_eq!(receipt["state"], "declared", "{receipt}");
    assert_eq!(receipt["profile"], PROFILE, "{receipt}");
    let after = deployment.registry();
    let held = after["placement_profiles"]
        .as_array()
        .and_then(|profiles| profiles.iter().find(|entry| entry["name"] == PROFILE))
        .expect("the registry holds the declared profile")
        .clone();
    assert_eq!(held["services"], json!([SERVICE]), "{held}");
    let route = &after["service_directory"]["services"][SERVICE];
    assert_eq!(route["placement_profile"], PROFILE, "{route}");
    assert!(route.get("managed_service").is_none(), "{route}");
    assert_eq!(route["active_host"], PLACED_ON, "{route}");
    assert!(generation(&after) > generation(&before));

    let listed = deployment.cli(&["placement", "profile", "list", "--json"]);
    let listed: Value = serde_json::from_str(&listed).unwrap();
    let expected = json!([{ "name": PROFILE, "services": [SERVICE], "hosts": [PLACED_ON] }]);
    assert_eq!(listed, expected);
    let shown = deployment.cli(&["placement", "profile", "show", PROFILE]);
    let shown: Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(shown, held);

    let unknown = profile(&before, "unknown-group", &["absent-api"]);
    let unknown = deployment.write_profile("unknown.json", &unknown);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &unknown],
        "service absent-api is not in the service directory; declare it with stado service declare",
    );
    let twice = profile(&before, "second-group", &[SERVICE]);
    let twice = deployment.write_profile("twice.json", &twice);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &twice],
        &format!("service {SERVICE} already moves with placement profile {PROFILE}"),
    );
    let dropping = profile(&before, PROFILE, &["marker-api"]);
    let dropping = deployment.write_profile("dropping.json", &dropping);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &dropping],
        &format!(
            "placement profile {PROFILE} holds service {SERVICE}, which this declaration drops"
        ),
    );
    let mut malformed = profile(&before, "malformed-group", &["marker-api"]);
    malformed["unexpected"] = json!(true);
    let malformed = deployment.write_profile("malformed.json", &malformed);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &malformed],
        "the placement profile does not parse as the registry reads one",
    );
    deployment.refused(
        &["placement", "profile", "remove", "unknown-group"],
        "the registry declares no placement profile named unknown-group",
    );

    let removed = deployment.cli(&["placement", "profile", "remove", PROFILE, "--json"]);
    let removed: Value = serde_json::from_str(&removed).unwrap();
    assert_eq!(removed["state"], "removed", "{removed}");
    let restored = deployment.registry();
    let route = &restored["service_directory"]["services"][SERVICE];
    assert!(route.get("placement_profile").is_none(), "{route}");
    assert_eq!(route["managed_service"], SERVICE, "{route}");
    let remaining = restored["placement_profiles"]
        .as_array()
        .expect("the profiles array stays");
    assert!(remaining.iter().all(|entry| entry["name"] != PROFILE));
    assert!(generation(&restored) > generation(&after));
}
