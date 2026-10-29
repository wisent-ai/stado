//! A role the registry declares for the host's serve unit and the host
//! process does not run is said in the beacon, through the real product.
//! One isolated deployment runs the host process with `--api` and the health
//! role while its registry declares no serve unit: the beacon must say no
//! declared unit takes the process's role lock, never that every role runs.
//! Then a Stado serve unit on this machine is declared with the resolver role
//! besides those two; the next pass must publish `roles.declared_not_running`
//! naming `--resolver` and that unit, and none of the roles it does run, and
//! `stado host health` must print each answer. The journey's process is no
//! resident unit of this machine (no unit file under its home starts its
//! line), so the converge that renders a resident unit missing a role again
//! answers `none` and records nothing. The unit's name and label are the
//! journey's own: no catalog entry asks it a question and no unit of the
//! machine's launchd carries it, so the pass reads it and acts on nothing the
//! operator runs.
use crate::deployment::Deployment;
use serde_json::{json, Value};

const NAME: &str = "stado-declared-roles-journey";
const UNIT: &str = "com.wisent.stado-declared-roles-journey";

/// The stored beacon of `slug`, as `stado host beacon show` reads it.
fn beacon(deployment: &mut Deployment, slug: &str) -> Value {
    serde_json::from_str(&deployment.cli(&["host", "beacon", "show", slug, "--json"]))
        .expect("the beacon is JSON")
}

#[test]
fn a_declared_role_the_host_process_does_not_run_is_published() {
    let mut deployment = Deployment::start();
    let slug = deployment.serve();
    let undeclared = beacon(&mut deployment, &slug)["roles"]["declared_not_running"].clone();
    deployment.report["undeclared"] = undeclared.clone();
    assert_eq!(
        undeclared,
        json!({ "no_declared_unit": "serve" }),
        "a registry declaring no serve unit is said as such"
    );
    let health = deployment.cli(&["host", "health", &slug]);
    assert!(
        health.contains("roles: no declared serve unit takes role lock serve"),
        "host health says nothing was compared: {health}"
    );

    let store = deployment.store().to_string_lossy().into_owned();
    let program = env!("CARGO_BIN_EXE_stado");
    let declared = json!([{
        "name": NAME,
        "unit": "",
        "label": UNIT,
        "path": deployment.home.join(format!("Library/LaunchAgents/{UNIT}.plist")),
        "kind": "launchd",
        "program": program,
        "args": [
            "serve", "--api", "--api-local-store", store,
            "--health-interval-seconds", "1", "--resolver",
        ],
    }]);
    deployment.cli(&[
        "registry",
        "set",
        "--path",
        "targets.0.services",
        "--value",
        &declared.to_string(),
        "--json",
    ]);
    deployment.report["declared"] = declared;

    // A pass already running may have read the registry before the
    // declaration; the one after it read it.
    deployment.next_publication();
    deployment.next_publication();
    let published = beacon(&mut deployment, &slug);
    let comparison = published["roles"]["declared_not_running"].clone();
    deployment.report["declared_not_running"] = comparison.clone();
    let roles: Vec<&str> = comparison["roles"]
        .as_array()
        .unwrap_or_else(|| panic!("the beacon compares declared roles: {published}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        roles,
        ["--resolver"],
        "only the resolver is declared and not run: {comparison}"
    );
    assert_eq!(comparison["declared_by"], json!([UNIT]), "{comparison}");
    // The journey's process is no resident unit of this machine: no unit
    // file under its home starts its line, so nothing is rendered again and
    // no converge is recorded.
    assert_eq!(
        comparison["converge"],
        json!({ "none": "this process is no resident unit of this host" }),
        "{comparison}"
    );
    let record = deployment.home.join(".stado/logs/resident-converge.json");
    assert!(
        !record.exists(),
        "no converge was recorded: {}",
        record.display()
    );

    let health = deployment.cli(&["host", "health", &slug]);
    deployment.report["health"] = json!(health);
    assert!(
        health.contains(&format!(
            "roles: declared but not running: --resolver ({UNIT}); rendered again: not \
             started: this process is no resident unit of this host"
        )),
        "host health prints the missing role and the converge's answer: {health}"
    );
    deployment.pass();
}
