//! The real convergence case: this machine, through Stado's same-host
//! channel, behind the authenticated Services API.
use super::*;

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
    grant_refusals(&fixture, &protected_baseline);
    let selected = format!("/api/service/converge?target={HOST}&binary=stado");
    let host_wide = format!("/api/service/converge?target={HOST}");

    let no_bearer = fixture.request("GET", &selected, None, "");
    assert_eq!(no_bearer.status, 401, "missing bearer: {}", no_bearer.body);
    assert_eq!(no_bearer.body, json!({"error": "unauthorized"}));
    let wrong_bearer = fixture.request("GET", &selected, Some("not-a-real-grant"), "");
    assert_eq!(
        wrong_bearer.status, 401,
        "wrong bearer: {}",
        wrong_bearer.body
    );
    assert_eq!(wrong_bearer.body, json!({"error": "unauthorized"}));

    let apply_cannot_read = fixture.request("GET", &selected, Some(&apply_bearer), "");
    assert_eq!(
        apply_cannot_read.status, 401,
        "apply-only grant read the route: {}",
        apply_cannot_read.body
    );
    let read_cannot_apply = fixture.request("POST", &selected, Some(&read_bearer), "");
    assert_eq!(
        read_cannot_apply.status, 401,
        "read-only grant applied convergence: {}",
        read_cannot_apply.body
    );

    for malformed in [
        "/api/service/converge",
        "/api/service/converge?binary=stado",
        "/api/service/converge?target=",
        "/api/service/converge?target=one&binary=",
        "/api/service/converge?target=one&target=two",
        "/api/service/converge?target=one&binary=stado&binary=skarbiec",
        "/api/service/converge?target=one&unexpected=value",
        "/api/service/converge?target=%GG",
        "/api/service/converge?target=%FF",
    ] {
        assert_error(
            &fixture.request("GET", malformed, Some(&read_bearer), ""),
            400,
            "INVALID_REQUEST",
        );
    }
    assert_error(
        &fixture.request("POST", &selected, Some(&apply_bearer), "{}"),
        400,
        "INVALID_REQUEST",
    );
    assert_error(
        &fixture.request(
            "GET",
            "/api/service/converge?target=no-such-declared-host",
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_error(
        &fixture.request(
            "GET",
            &format!("/api/service/converge?target={HOST}&binary=no-such-binary"),
            Some(&read_bearer),
            "",
        ),
        503,
        "SERVICE_CONVERGE_FAILED",
    );
    assert_eq!(digest(&fixture.protected), protected_baseline);
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);

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

#[test]
#[ignore = "Probierz Desktop CUA owns this long-lived real fixture"]
fn service_convergence_cua_fixture() {
    // This fixture performs no Wisent account operation. The desktop receives
    // only an owner-readable local token file for a dedicated registry API
    // client whose bearer is resolved through the real isolated Skarbiec.
    let ready = PathBuf::from(
        std::env::var_os("STADO_SERVICE_CONVERGENCE_READY")
            .expect("STADO_SERVICE_CONVERGENCE_READY is required"),
    );
    let stop = PathBuf::from(
        std::env::var_os("STADO_SERVICE_CONVERGENCE_STOP")
            .expect("STADO_SERVICE_CONVERGENCE_STOP is required"),
    );
    let bearer = skarbiec_generated_bearer();
    let clients = [ClientGrant::new(
        "desktop-local",
        vec!["policy-read", "converge-read", "converge-apply"],
        bearer.clone(),
    )];
    let mut fixture = DashboardFixture::start(&clients);
    let token_file = fixture.home.join(".stado/desktop-registry-api-token");
    fs::write(&token_file, bearer).expect("write desktop registry API bearer");
    fs::set_permissions(&token_file, fs::Permissions::from_mode(0o600))
        .expect("desktop registry API bearer is owner-readable only");
    let stado_baseline = digest(&fixture.stado);
    let skarbiec_baseline = digest(&fixture.skarbiec);
    let registry_baseline = digest(&fixture.storage.join("registry.json"));
    let readiness = json!({
        "endpoint": fixture.endpoint(),
        "home": fixture.home,
        "storage": fixture.storage,
        "config": fixture.config,
        "binary": env!("CARGO_BIN_EXE_stado"),
        "target": HOST,
        "token_file": token_file
    });
    fs::write(
        &ready,
        serde_json::to_vec_pretty(&readiness).expect("readiness JSON"),
    )
    .expect("write CUA fixture readiness");
    println!(
        "service-convergence CUA fixture ready at {}",
        fixture.endpoint()
    );

    while !stop.exists() {
        if let Some(status) = fixture.dashboard.try_wait().expect("read dashboard status") {
            panic!("dashboard exited during CUA journey with {status}");
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(digest(&fixture.stado), stado_baseline);
    assert_eq!(digest(&fixture.skarbiec), skarbiec_baseline);
    assert_eq!(
        digest(&fixture.storage.join("registry.json")),
        registry_baseline
    );
}
