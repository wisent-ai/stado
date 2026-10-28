//! `stado placement profile draft`: the profile that would move a fixed
//! route, read from an isolated deployment's own registry.
//!
//! The shared directory template places `elsewhere-api` on `other-host` as a
//! fixed route. Expected: the draft writes nothing (the directory generation
//! stays), places the service on its active host with the unit the host
//! declares under the route's `managed_service`, carries the stated probe and
//! state, and `declare --file` accepts it as printed, linking the route.
//! Refusals: a service with no probe (no `--probe` and no GET answering
//! request in the catalog for its product), a service the directory does not
//! declare, and a service a profile already moves. `placement role move` of
//! that fixed route, and of a service the directory does not declare, is
//! refused before anything changes. A second journey gives the template's
//! authority, `marker-host`, the fleet object API as a role of its host Stado
//! process: its store holds the registry the authority writes, so the move
//! without `--with-authority` is refused naming the flag, and
//! `--with-authority` for the role placed on a host that is not the
//! authority names the authority the directory declares.

use serde_json::{json, Value};

use crate::deployment::{generation, Deployment, PLACED_ON, PROFILE, SERVICE};

const ROLE: &str = "stado-object-api";
const AUTHORITY: &str = "marker-host";

/// A fresh deployment whose registry is the template with the fleet object
/// API a role of the host Stado process on `host` (every target declares the
/// host process; the route takes the template's own endpoint). Each case has
/// its own, since an import never replaces what an earlier one declared.
fn deployment_with_role_on(host: &str) -> Deployment {
    let mut document: Value =
        serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
    for target in document["targets"]
        .as_array_mut()
        .expect("the template lists its targets")
    {
        let services = target["services"]
            .as_array_mut()
            .expect("each target lists its services");
        services.push(json!({ "name": "stado", "unit": "com.wisent.stado" }));
    }
    let endpoint =
        document["service_directory"]["services"]["marker-api"]["endpoints"][AUTHORITY].clone();
    document["service_directory"]["services"][ROLE] =
        json!({ "managed_service": "stado", "active_host": host, "endpoints": { host: endpoint } });
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import(&document);
    deployment
}

#[test]
fn a_role_on_the_authority_moves_only_carrying_it() {
    deployment_with_role_on(AUTHORITY).refused(
        &["placement", "role", "move", ROLE, "--to-host", PLACED_ON, "--reason", "journey", "--check"],
        &format!("{AUTHORITY} holds the registry authority, and {ROLE}'s store holds the registry it writes"),
    );
    deployment_with_role_on(PLACED_ON).refused(
        &[
            "placement",
            "role",
            "move",
            ROLE,
            "--to-host",
            AUTHORITY,
            "--reason",
            "journey",
            "--with-authority",
            "--check",
        ],
        &format!("but the directory names {AUTHORITY} the authority"),
    );
}

/// The service record `host` declares for `managed`, by its logical name or
/// its unit name.
fn declared_service(document: &Value, host: &str, managed: &str) -> Value {
    document["targets"]
        .as_array()
        .and_then(|targets| targets.iter().find(|target| target["name"] == host))
        .and_then(|target| target["services"].as_array())
        .and_then(|services| {
            services.iter().find(|service| {
                service["name"] == managed
                    || service["unit"] == managed
                    || service["label"] == managed
            })
        })
        .cloned()
        .expect("the template declares the route's managed service on its active host")
}

#[test]
fn a_fixed_route_is_drafted_into_a_profile_that_declares_as_printed() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let template = deployment.registry();
    let route = &template["service_directory"]["services"][SERVICE];
    let managed = route["managed_service"]
        .as_str()
        .expect("the template's route is a fixed route naming its unit")
        .to_string();
    let endpoint = route["endpoints"][PLACED_ON]["url"]
        .as_str()
        .expect("the template names the placed host's endpoint");
    let health = format!("{endpoint}/health");
    let probe = format!("{SERVICE}={health}");

    deployment.refused(
        &[
            "placement",
            "profile",
            "draft",
            PROFILE,
            "--service",
            SERVICE,
            "--probe",
            &probe,
        ],
        &format!(
            "registry target {PLACED_ON} declares managed service {managed} with no unit file path"
        ),
    );
    let mut recorded = template.clone();
    let unit_file = format!("/home/example/.config/systemd/user/{managed}.service");
    for target in recorded["targets"]
        .as_array_mut()
        .expect("the template lists targets")
    {
        if target["name"] != PLACED_ON {
            continue;
        }
        for service in target["services"]
            .as_array_mut()
            .expect("the host lists services")
        {
            if service["name"] == managed.as_str() {
                service["path"] = json!(unit_file);
                service["kind"] = json!("systemd");
            }
        }
    }
    deployment.import(&recorded);
    let before = deployment.registry();
    let managed = managed.as_str();

    deployment.refused(
        &[
            "placement",
            "profile",
            "draft",
            PROFILE,
            "--service",
            SERVICE,
        ],
        &format!(
            "nor a GET answering request the catalog declares for its product names {SERVICE}"
        ),
    );
    let absent = format!("absent-api={health}");
    deployment.refused(
        &[
            "placement",
            "profile",
            "draft",
            PROFILE,
            "--service",
            "absent-api",
            "--probe",
            &absent,
        ],
        "the service directory declares no service absent-api",
    );
    // A fixed route served by its own unit is no role of the host Stado
    // process: role move refuses it, pointing at a profile, before any host
    // is fenced, and the registry stays as it was.
    deployment.refused(
        &[
            "placement",
            "role",
            "move",
            SERVICE,
            "--to-host",
            PLACED_ON,
            "--reason",
            "journey",
        ],
        &format!("is served by {managed}, not by the host Stado process"),
    );
    deployment.refused(
        &[
            "placement",
            "role",
            "move",
            "absent-api",
            "--to-host",
            PLACED_ON,
            "--reason",
            "journey",
        ],
        "the service directory declares no absent-api",
    );
    assert_eq!(generation(&deployment.registry()), generation(&before));

    let printed = deployment.cli(&[
        "placement",
        "profile",
        "draft",
        PROFILE,
        "--service",
        SERVICE,
        "--probe",
        &probe,
        "--state",
        ".stado/elsewhere.json",
    ]);
    let drafted: Value = serde_json::from_str(&printed).unwrap();
    assert_eq!(generation(&deployment.registry()), generation(&before));
    assert_eq!(drafted["services"], json!([SERVICE]), "{drafted}");
    assert_eq!(drafted["start_order"], json!([SERVICE]), "{drafted}");
    assert_eq!(
        drafted["state"],
        json!([{ "path": ".stado/elsewhere.json", "required": true }]),
        "{drafted}"
    );
    let hosts = drafted["hosts"]
        .as_object()
        .expect("the draft names its hosts");
    assert_eq!(
        hosts.keys().collect::<Vec<_>>(),
        vec![PLACED_ON],
        "{drafted}"
    );
    let unit = &drafted["hosts"][PLACED_ON]["units"][SERVICE];
    let declared = declared_service(&before, PLACED_ON, managed);
    assert_eq!(unit["name"], managed, "{unit}");
    assert_eq!(unit["path"], declared["path"], "{unit} against {declared}");
    assert_eq!(unit["kind"], declared["kind"], "{unit} against {declared}");
    assert_eq!(
        drafted["hosts"][PLACED_ON]["probes"],
        json!([{ "service": SERVICE, "url": health }]),
        "{drafted}"
    );

    let file = deployment.write_profile("drafted.json", &drafted);
    let receipt: Value = serde_json::from_str(&deployment.cli(&[
        "placement",
        "profile",
        "declare",
        "--file",
        &file,
        "--json",
    ]))
    .unwrap();
    assert_eq!(receipt["state"], "declared", "{receipt}");
    let after = deployment.registry();
    let linked = &after["service_directory"]["services"][SERVICE];
    assert_eq!(linked["placement_profile"], PROFILE, "{linked}");

    deployment.refused(
        &[
            "placement",
            "profile",
            "draft",
            "second-group",
            "--service",
            SERVICE,
            "--probe",
            &probe,
        ],
        &format!("directory service {SERVICE} already moves with placement profile {PROFILE}"),
    );
    deployment.cli(&["placement", "profile", "remove", PROFILE]);
}
