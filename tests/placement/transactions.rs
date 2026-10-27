//! `stado placement transactions` and `stado placement release` against an
//! isolated deployment's own registry store.
//!
//! The deployment declares a profile for `elsewhere-api`, placed on
//! `other-host`, and then holds what a move leaves when its process ends
//! before committing or rolling back: its claim in the registry. Expected:
//! with no move open, `transactions` says so (`[]` with `--json`), and
//! `placement move --check` to a host the profile does not name is refused
//! as the move would be, claiming nothing, while the move itself is refused
//! and kept in `placement history` (the check is not); an evacuation is
//! refused whole for the same reason, and with `--standby --check` by the
//! refusal of `placement standby --check` for that profile; `profile
//! sync` leaves a profile whose services run no catalog product as it
//! is and refuses an unknown one, `placement holds` names the profile on its
//! host and nothing on the destination, the registry unchanged by all of it; with the
//! claim, it lists the move's id, profile, hosts, start and how long it has
//! been open, and resolving the service for a declared consumer is refused
//! naming that move and the commands that list and release it. `release` of
//! an id no move holds is refused, leaving the registry as it was. `release`
//! of the open move undoes what it can and releases the claim even when a
//! rollback step fails: here the profile declares no units on the move's
//! destination, so the rollback names that and nothing runs on any host; the
//! command exits non-zero naming the failed step, the registry no longer
//! holds the claim, and the service resolves again.
use serde_json::{json, Value};

use crate::deployment::{profile, Deployment, PLACED_ON, PROFILE, SERVICE};

/// The consumer the directory template admits to `elsewhere-api`.
const CONSUMER: &str = "marker-consumer";
/// The template's other registered host, which the profile does not name.
const DESTINATION: &str = "marker-host";

fn open(deployment: &Deployment) -> Value {
    serde_json::from_str(&deployment.cli(&["placement", "transactions", "--json"])).unwrap()
}

#[test]
fn an_open_placement_move_is_listed_named_in_refusals_and_released() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let before = deployment.registry();
    let declared = deployment.write_profile("profile.json", &profile(&before, PROFILE, &[SERVICE]));
    deployment.cli(&["placement", "profile", "declare", "--file", &declared]);

    assert_eq!(open(&deployment), json!([]));
    let said = deployment.cli(&["placement", "transactions"]);
    assert!(said.contains("no placement transaction is open"), "{said}");

    // A check meets the refusal the move would, and claims nothing: the
    // registry holds no transaction and its generation stays.
    let unchecked = deployment.registry();
    deployment.refused(
        &["placement", "move", SERVICE, "--to-host", DESTINATION, "--check"],
        &format!("placement profile {PROFILE:?} does not support destination {DESTINATION:?}"),
    );
    // The move itself meets the same refusal, and the placement history
    // keeps it; the check is not a change and is not kept.
    let empty: Value =
        serde_json::from_str(&deployment.cli(&["placement", "history", "--json"])).unwrap();
    assert_eq!(empty["entries"], json!([]), "{empty}");
    deployment.refused(
        &["placement", "move", SERVICE, "--to-host", DESTINATION],
        "does not support destination",
    );
    let kept: Value =
        serde_json::from_str(&deployment.cli(&["placement", "history", "--json"])).unwrap();
    let entries = kept["entries"].as_array().expect("the history lists its entries");
    let [entry] = entries.as_slice() else {
        panic!("the history keeps exactly the refused move: {kept}");
    };
    assert_eq!(entry["kind"], "move", "{kept}");
    assert!(
        entry["refused"].as_str().is_some_and(|refused| refused.contains("does not support destination")),
        "{kept}"
    );
    // An evacuation checks every profile first: the same refusal refuses it
    // whole, with and without --check, before anything is claimed.
    for check in [true, false] {
        let mut evacuate = vec![
            "placement", "evacuate", "--from-host", PLACED_ON, "--to-host", DESTINATION,
            "--reason", "journey",
        ];
        if check {
            evacuate.push("--check");
        }
        deployment.refused(
            &evacuate,
            &format!(
                "evacuating {PLACED_ON} to {DESTINATION} would be refused, so nothing was moved"
            ),
        );
    }
    // With --standby that profile is one the run would stand the destination
    // by for first, so standby's own check runs, and its refusal (the test
    // service is no catalog product, so nothing could deliver it) refuses
    // the evacuation whole, as `placement standby --check` refuses itself.
    let uncatalogued = format!(
        "the Wisent service catalog this build ships declares no service named {SERVICE:?}"
    );
    deployment.refused(
        &["placement", "evacuate", "--from-host", PLACED_ON, "--to-host", DESTINATION,
          "--reason", "journey", "--standby", "--check"],
        &format!("{PROFILE}: standing {DESTINATION} by: {uncatalogued}"),
    );
    deployment.refused(
        &["placement", "standby", PROFILE, "--host", DESTINATION, "--reason", "journey",
          "--check"],
        &uncatalogued,
    );
    deployment.refused(
        &["placement", "evacuate", "--from-host", DESTINATION, "--to-host", PLACED_ON,
          "--reason", "journey"],
        &format!(
            "no placement profile and no role of the host Stado process is placed on {DESTINATION}"
        ),
    );
    // Handing over an emptied host's schedules is still an evacuation: the
    // check is not refused as having nothing to move, and its report says
    // which schedules it would repin (or the store's refusal names itself).
    let handover = deployment.run(&[
        "placement", "evacuate", "--from-host", DESTINATION, "--to-host", PLACED_ON,
        "--reason", "journey", "--repin-schedules", "--check", "--json",
    ]);
    let said = format!("{}{}", String::from_utf8_lossy(&handover.stdout), String::from_utf8_lossy(&handover.stderr));
    assert!(!said.contains("no placement profile and no role"), "{said}");
    match handover.status.success() {
        true => {
            let report: Value = serde_json::from_slice(&handover.stdout).unwrap();
            assert_eq!(report["status"], "would_evacuate", "{report}");
            assert!(report["repinned_schedules"].is_array(), "{report}");
        }
        false => assert!(said.contains("--repin-schedules: the schedules pinned to"), "{said}"),
    }
    deployment.refused(
        &["placement", "evacuate", "--from-host", PLACED_ON, "--to-host", PLACED_ON,
          "--reason", "journey"],
        &format!("--from-host and --to-host both name {PLACED_ON}"),
    );
    // The profile's services run no catalog product, so the catalog declares
    // no store for them: sync leaves it as it is and writes nothing; an
    // unknown profile is refused by name.
    let synced: Value = serde_json::from_str(&deployment.cli(&[
        "placement", "profile", "sync", PROFILE, "--json",
    ]))
    .unwrap();
    assert_eq!(synced["state"], "unchanged", "{synced}");
    deployment.refused(
        &["placement", "profile", "sync", "no-such-profile"],
        "the registry declares no placement profile named \"no-such-profile\"",
    );
    // --all syncs every declared profile, the same profile included, and
    // writes nothing when none lacks a store; a name beside --all and neither
    // are refused by clap.
    let before = deployment.registry()["service_directory"]["generation"].clone();
    let every: Value = serde_json::from_str(&deployment.cli(&[
        "placement", "profile", "sync", "--all", "--json",
    ]))
    .unwrap();
    let row = every["profiles"]
        .as_array()
        .expect("--all lists every profile")
        .iter()
        .find(|row| row["profile"] == PROFILE)
        .expect("the declared profile is listed");
    assert_eq!(row["state"], "unchanged", "{every}");
    assert_eq!(deployment.registry()["service_directory"]["generation"], before);
    deployment.refused(&["placement", "profile", "sync", PROFILE, "--all"], "--all");
    deployment.refused(&["placement", "profile", "sync"], "<NAME>");
    // The profile is placed on its host, which is therefore not emptied; the
    // destination holds nothing, and an undeclared host is refused.
    let held: Value = serde_json::from_str(&deployment.cli(&[
        "placement", "holds", PLACED_ON, "--json",
    ]))
    .unwrap();
    assert_eq!(held["emptied"], false, "{held}");
    let placed: Vec<&str> = held["profiles"]
        .as_array()
        .expect("holds lists its profiles")
        .iter()
        .filter_map(|profile| profile["profile"].as_str())
        .collect();
    assert_eq!(placed, [PROFILE], "{held}");
    let empty: Value = serde_json::from_str(&deployment.cli(&[
        "placement", "holds", DESTINATION, "--json",
    ]))
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
    let rows = everywhere["hosts"].as_array().expect("holds lists every host");
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
        assert!(row["pinned_schedules"].is_array() != row["pinned_schedules_unread"].is_string(), "{everywhere}");
        // No template host answers a fence read; each says why, never "not fenced".
        assert!(row["store_fence_unread"].is_string() && row["store_fence"].is_null(), "{everywhere}");
    }
    assert_eq!(open(&deployment), json!([]));
    assert_eq!(
        deployment.registry()["service_directory"]["generation"],
        unchecked["service_directory"]["generation"]
    );

    let id = uuid::Uuid::new_v4().to_string();
    let started_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut held = deployment.registry();
    held["placement_transactions"] = json!([{
        "id": id, "profile": PROFILE, "from_host": PLACED_ON,
        "to_host": DESTINATION, "started_at": started_at,
    }]);
    deployment.import(&held);

    let listed = open(&deployment);
    let entry = listed
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["id"] == id.as_str()))
        .expect("the open move is listed")
        .clone();
    assert_eq!(entry["profile"], PROFILE, "{entry}");
    assert_eq!(entry["from_host"], PLACED_ON, "{entry}");
    assert_eq!(entry["to_host"], DESTINATION, "{entry}");
    assert_eq!(entry["started_at"], started_at.as_str(), "{entry}");
    assert!(entry["open_seconds"].is_i64(), "{entry}");

    let connect = [
        "service",
        "directory",
        "connect",
        SERVICE,
        "--consumer",
        CONSUMER,
        "--no-verify",
    ];
    deployment.refused(
        &connect,
        &format!("placement transaction {id} for \"{PROFILE}\""),
    );
    deployment.refused(&connect, &format!("stado placement release {id}"));

    let unknown = uuid::Uuid::new_v4().to_string();
    deployment.refused(
        &["placement", "release", &unknown],
        &format!("no placement transaction {unknown} is open"),
    );

    let released = deployment.run(&["placement", "release", &id, "--json"]);
    let stderr = String::from_utf8_lossy(&released.stderr);
    assert!(
        !released.status.success(),
        "a failed rollback step exits non-zero"
    );
    assert!(
        stderr.contains(&format!("placement transaction {id} is released")),
        "{stderr}"
    );
    let report: Value = serde_json::from_slice(&released.stdout).unwrap();
    assert_eq!(report["status"], "released", "{report}");
    let failures = report["rollback_failures"]
        .as_array()
        .expect("the report lists the rollback steps that failed");
    assert!(
        failures.iter().any(|failure| failure
            .as_str()
            .is_some_and(|text| text.contains(DESTINATION))),
        "{report}"
    );
    assert!(deployment
        .registry()
        .get("placement_transactions")
        .is_none());
    assert_eq!(open(&deployment), json!([]));
    let resolved = deployment.run(&connect);
    let stderr = String::from_utf8_lossy(&resolved.stderr);
    assert!(!stderr.contains("during placement transaction"), "{stderr}");
}
