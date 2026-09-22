//! Real native ownership checks. The dedicated-host case is read-only and
//! requires the actual ReleaseTargetPolicy in STADO_TEST_LEGACY_TARGET.

use crate::release_agent::rollout::serving::{discover, legacy};
use crate::release_control::ReleaseTargetPolicy;

fn actual_target() -> ReleaseTargetPolicy {
    let path = std::env::var("STADO_TEST_LEGACY_TARGET")
        .expect("STADO_TEST_LEGACY_TARGET must name the real dedicated-host release target JSON");
    let bytes = std::fs::read(&path).expect("the real target policy is readable");
    let target: ReleaseTargetPolicy = serde_json::from_slice(&bytes).expect("valid target policy");
    assert_eq!(
        std::fs::canonicalize(&target.home).expect("target home exists"),
        std::fs::canonicalize(std::env::var("HOME").expect("host HOME")).expect("host home exists"),
        "execute on the selected target, not against another host's coordinates"
    );
    target
}

#[tokio::test]
#[ignore = "requires a real system legacy service on the dedicated host"]
async fn declared_predecessor_stays_serving_during_candidate_admission() {
    let target = actual_target();
    let serving = target.blue_green_serving().expect("real blue-green target");
    let address: std::net::SocketAddr = serving.stable_bind.parse().expect("stable address");
    assert!(!serving.candidate_ports.contains(&address.port()));
    assert!(legacy::owns_stable_bind(&target, address.port()).expect("native owner read"));
    assert!(
        discover::foreign_stable_bind_holder(&target, &serving, "ownership-qualification")
            .await
            .expect("actual admission guard")
            .is_none(),
        "the declared owner must not block an independent candidate port"
    );
    assert!(
        legacy::owns_stable_bind(&target, address.port())
            .expect("native owner read after admission"),
        "admission must leave the predecessor serving"
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a real unrelated listener");
    let mut foreign = target;
    foreign.stable_bind = Some(listener.local_addr().expect("bound address").to_string());
    let foreign_serving = foreign
        .blue_green_serving()
        .expect("same target, unrelated port");
    let refusal =
        discover::foreign_stable_bind_holder(&foreign, &foreign_serving, "ownership-qualification")
            .await
            .expect("actual admission guard")
            .expect("a declared label cannot authorize an unrelated process");
    assert!(
        refusal.contains(&std::process::id().to_string()),
        "{refusal}"
    );
}

#[tokio::test]
async fn absent_legacy_declaration_cannot_authorize_a_real_listener() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a real unrelated listener");
    let candidates = [
        std::net::TcpListener::bind("127.0.0.1:0").expect("first independent port"),
        std::net::TcpListener::bind("127.0.0.1:0").expect("second independent port"),
    ];
    let target = ReleaseTargetPolicy {
        platform: std::env::consts::OS.to_string(),
        run_as_user: std::env::var("USER").expect("test account"),
        home: std::env::var("HOME").expect("test home"),
        state_dir: String::new(),
        runtime_root: String::new(),
        logs_root: String::new(),
        stable_bind: Some(listener.local_addr().expect("bound address").to_string()),
        candidate_ports: Some(
            candidates
                .each_ref()
                .map(|candidate| candidate.local_addr().expect("bound address").port()),
        ),
        readiness_path: Some("/readyz".to_string()),
        legacy_launchd_label: None,
        legacy_launchd_plist: None,
    };
    let serving = target.blue_green_serving().expect("blue-green coordinates");
    let refusal =
        discover::foreign_stable_bind_holder(&target, &serving, "ownership-qualification")
            .await
            .expect("actual native admission guard")
            .expect("a real listener without a declared predecessor is refused");
    assert!(
        refusal.contains(&std::process::id().to_string()),
        "{refusal}"
    );
}
