//! Real Services API convergence against the built Stado dashboard, a real
//! isolated Skarbiec vault, and this machine through Stado's same-host channel.

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;

mod authorization;
mod bearer;
mod client;
mod cua;
mod fixture;
mod helpers;

use authorization::refuse_unauthorized_and_malformed;
use bearer::verify_persistent_verifier;
use fixture::{ClientGrant, DashboardFixture, HOST};
use helpers::{binary_rows, digest, release_platform, skarbiec_generated_bearer};

#[test]
#[ignore = "Probierz owns the real Skarbiec/dashboard/host qualification"]
fn authenticated_services_api_converges_real_same_host_state() {
    let read_bearer = skarbiec_generated_bearer();
    let apply_bearer = skarbiec_generated_bearer();
    let clients = [
        ClientGrant::new("read-only", vec!["converge-read"], read_bearer.clone()),
        ClientGrant::new("apply-only", vec!["converge-apply"], apply_bearer.clone()),
    ];
    let fixture = DashboardFixture::start(&clients);
    let protected_baseline = digest(&fixture.protected);
    let stado_baseline = digest(&fixture.stado);
    let skarbiec_baseline = digest(&fixture.skarbiec);
    let verifier_token = fixture
        .vault
        .as_ref()
        .expect("authenticated fixture has a real Skarbiec vault")
        .token
        .clone();
    let verifier_token_json = verifier_token
        .to_str()
        .expect("isolated verifier token path is UTF-8");
    let selected = format!("/api/service/converge?target={HOST}&binary=stado");
    let host_wide = format!("/api/service/converge?target={HOST}");
    verify_persistent_verifier(&fixture, &verifier_token, verifier_token_json, &protected_baseline);
    refuse_unauthorized_and_malformed(
        &fixture,
        &selected,
        &read_bearer,
        &apply_bearer,
        &protected_baseline,
        &stado_baseline,
        &skarbiec_baseline,
    );

    let current = fixture.request("GET", &selected, Some(&read_bearer), "");
    assert_eq!(
        current.status, 200,
        "persisted verifier grant could not authorize the real Skarbiec read: {}",
        current.body
    );
    assert_eq!(
        current.body["exit_code"], 0,
        "selected GET: {}",
        current.body
    );
    println!(
        "verified persistent verifier bearer through built Stado and real Skarbiec: create, byte-identical reuse, mode 0600, effective read, symlink refusal, empty-file refusal"
    );
    assert_eq!(current.body["report"]["target"], HOST);
    assert_eq!(current.body["report"]["applied"], false);
    let selected_rows = current.body["report"]["binaries"]
        .as_array()
        .expect("selected report carries rows");
    assert_eq!(selected_rows.len(), 1, "selected report: {}", current.body);
    assert_eq!(selected_rows[0]["binary"], "stado");
    assert_eq!(selected_rows[0]["verdict"], "in-sync");
    assert_eq!(
        selected_rows[0]["declared_version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        selected_rows[0]["installed_version"],
        env!("CARGO_PKG_VERSION")
    );

    let applied_current = fixture.request("POST", &selected, Some(&apply_bearer), "");
    assert_eq!(
        applied_current.status, 200,
        "selected POST: {}",
        applied_current.body
    );
    assert_eq!(
        applied_current.body["exit_code"], 0,
        "selected POST: {}",
        applied_current.body
    );
    assert_eq!(applied_current.body["report"]["applied"], true);

    let report = fixture.request("GET", &host_wide, Some(&read_bearer), "");
    assert_eq!(report.status, 200, "host-wide GET: {}", report.body);
    assert_ne!(
        report.body["exit_code"], 0,
        "host-wide GET: {}",
        report.body
    );
    let rows = binary_rows(&report.body["report"]);
    assert_eq!(rows.len(), 2, "host-wide GET: {}", report.body);
    assert_eq!(rows["stado"]["verdict"], "in-sync");
    assert_eq!(rows["skarbiec"]["verdict"], "host-behind");
    assert_eq!(
        rows["skarbiec"]["installed_version"],
        fixture.skarbiec_current
    );
    assert_eq!(
        rows["skarbiec"]["declared_version"],
        fixture.skarbiec_declared
    );

    let failed = fixture.request("POST", &host_wide, Some(&apply_bearer), "");
    assert_eq!(failed.status, 200, "failed apply envelope: {}", failed.body);
    assert_ne!(failed.body["exit_code"], 0, "failed apply: {}", failed.body);
    assert_eq!(failed.body["report"]["applied"], true);
    let releases = failed.body["report"]["releases"]
        .as_array()
        .expect("failed apply retains releases");
    let release = releases
        .iter()
        .find(|release| release["binary"] == "skarbiec")
        .expect("failed Skarbiec delivery remains in the report");
    assert_eq!(release["version"], fixture.skarbiec_declared);
    assert_eq!(release["status"], "failed");
    println!("failed convergence receipt: {}", failed.body);
    let final_rows = binary_rows(&failed.body["report"]);
    assert_eq!(final_rows["stado"]["verdict"], "in-sync");
    assert_eq!(final_rows["skarbiec"]["verdict"], "host-behind");
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);

    println!(
        "verified authenticated nonlocal Services API on {}: selected-current=stado {}; failed-delivery=skarbiec {}->{}; exit={}",
        release_platform(),
        env!("CARGO_PKG_VERSION"),
        fixture.skarbiec_current,
        fixture.skarbiec_declared,
        failed.body["exit_code"]
    );
}

