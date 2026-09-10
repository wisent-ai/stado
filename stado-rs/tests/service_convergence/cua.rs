//! The long-lived fixture Probierz Desktop drives through the real dashboard.

use serde_json::json;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use crate::fixture::{ClientGrant, DashboardFixture, HOST};
use crate::helpers::{digest, skarbiec_generated_bearer};

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
