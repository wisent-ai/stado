//! Real declared publisher-profile and Developer ID journeys.
//!
//! These tests require a dedicated macOS target and disposable repository;
//! no part of the release path is substituted with a local fixture.
use std::process::{Command, Output};

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must name the dedicated publisher fixture"))
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .output()
        .expect("stado starts")
}

fn success(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stado failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("stado returns JSON")
}

#[test]
#[ignore = "Probierz supplies a dedicated macOS host and disposable GitHub repository"]
fn publisher_runner_install_reconciles_an_existing_runner_without_org_admin_access() {
    let target = required("STADO_PUBLISHER_TEST_TARGET");
    let repository = required("STADO_PUBLISHER_TEST_REPOSITORY");

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    assert_eq!(installed["status"], "completed");
    assert_eq!(installed["runner_kind"], "publisher");
    assert_eq!(installed["runner_group"], "Default");
    assert!(installed["stdout"]
        .as_str()
        .unwrap_or_default()
        .contains("runner service: running"));

    let status = success(&run(&[
        "runner",
        "status",
        &target,
        "--profile",
        "publisher",
        "--json",
    ]));
    assert_eq!(status["status"], "completed");
    assert_eq!(status["runner_kind"], "publisher");
}

#[test]
#[ignore = "Probierz supplies Account Holder 2FA consent on the dedicated macOS host"]
fn publisher_install_issues_once_reuses_the_bundle_and_grants_repository_signing() {
    let target = required("STADO_PUBLISHER_TEST_TARGET");
    let repository = required("STADO_PUBLISHER_TEST_REPOSITORY");

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    let issued = &installed["repository_bootstrap"]["developer_id"];
    assert!(matches!(
        issued["status"].as_str(),
        Some("issued" | "reused")
    ));
    assert!(issued["identity"]
        .as_str()
        .unwrap_or_default()
        .starts_with("Developer ID Application:"));
    assert_eq!(issued["repositories"][0], repository);

    let installed = success(&run(&[
        "runner",
        "install",
        &target,
        "--profile",
        "publisher",
        "--repository",
        &repository,
        "--json",
    ]));
    let reused = &installed["repository_bootstrap"]["developer_id"];
    assert_eq!(reused["status"], "reused");
    assert_eq!(reused["identity"], issued["identity"]);

    let secret_names = Command::new("gh")
        .args([
            "secret",
            "list",
            "--repo",
            &format!("wisent-ai/{repository}"),
            "--json",
            "name",
        ])
        .output()
        .expect("gh starts");
    assert!(
        secret_names.status.success(),
        "gh secret list failed: {}",
        String::from_utf8_lossy(&secret_names.stderr)
    );
    let names: serde_json::Value =
        serde_json::from_slice(&secret_names.stdout).expect("gh returns JSON");
    for required in [
        "MACOS_CERT_P12",
        "MACOS_CERT_PASSWORD",
        "MACOS_SIGN_IDENTITY",
    ] {
        assert!(
            names
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == required),
            "missing {required}"
        );
    }
}

// The declared GitHub identity every runner profile registers with.
//
// `GITHUB_CREDENTIAL_ITEM` was the literal `"GITHUB_TOKEN"` compiled into the
// runner lifecycle. On 2026-09-07 that identity was refused by
// `GET /orgs/wisent-ai/actions/runner-groups` with HTTP 403 "You must be an
// org admin or have the runners and runner groups fine-grained permission" --
// an OAuth token carrying `read:org` where the endpoint answers only
// `admin:org` -- and nothing could point Stado at another credential without
// editing Rust. The checks below need no host and no network: their subject is
// Stado's own resolution of the route declared in
// `stado-rs/data/github-identity.json`, and Skarbiec's answer is its input, so
// a loopback listener states that answer exactly.

/// The coordinate the fixture route names. Deliberately nothing like
/// `GITHUB_TOKEN`: a refusal naming it can only have come from the route.
const FIXTURE_ITEM: &str = "publisher-fixture-github-identity";
const FIXTURE_FIELD: &str = "api_key";

/// A loopback stand-in for Skarbiec's operator route. Stado is the product
/// under test and Skarbiec's answer is its input, so the answer is stated here
/// exactly. Each canned reply serves one request, in order.
fn skarbiec_answering(replies: Vec<(u16, String)>) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a loopback port");
    let port = listener
        .local_addr()
        .expect("read the bound address")
        .port();
    std::thread::spawn(move || {
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = std::io::Read::read(&mut stream, &mut [0u8; 4096]);
            let line = if status == 200 {
                "200 OK"
            } else {
                "404 Not Found"
            };
            let _ = std::io::Write::write_all(
                &mut stream,
                format!(
                    "HTTP/1.1 {line}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    port
}

/// One owner-only grant file, because Stado refuses to send a credential read
/// without one. It is never a real bearer: the stand-in does not check it.
fn grant_file() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("stado-github-identity-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the fixture root");
    let path = root.join("admin-token");
    std::fs::write(&path, "fixture-grant").expect("write the fixture grant");
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .expect("protect the fixture grant");
    path
}

fn credential_report(port: u16) -> Output {
    let grant = grant_file();
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["runner", "credential", "--json"])
        // A configuration that does not exist, so the answer can only come from
        // this test's declaration and never from the operator's machine.
        .env("STADO_CONFIG", grant.with_file_name("no-such-config.json"))
        .env(
            "STADO_CREDENTIALS_ADMIN_URL",
            format!("http://127.0.0.1:{port}"),
        )
        .env("STADO_CREDENTIALS_ADMIN_CONSUMER", "local-operator")
        .env("STADO_CREDENTIALS_ADMIN_TOKEN_FILE", &grant)
        .output()
        .expect("stado starts")
}

fn streams(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The shipped declaration is readable and states a route Skarbiec's own
/// `route declare` accepts. A coordinate such as `GITHUB_TOKEN#value` would put
/// the hardcoded id back under a new name, so the validator refuses one.
#[test]
fn the_shipped_github_identity_declaration_states_a_declarable_route() {
    let identity = stado::github_identity::declared().expect("the shipped declaration parses");
    assert!(
        identity.credential_route.contains(':') && !identity.credential_route.contains('#'),
        "credential_route is not a declarable route: {}",
        identity.credential_route
    );
    for prefix in ["provider:", "agent:", "login:"] {
        assert!(
            !identity.credential_route.starts_with(prefix),
            "credential_route claims a namespace items declare for themselves: {}",
            identity.credential_route
        );
    }
    assert!(
        identity.reality_check.starts_with('/') && !identity.required_permission.is_empty(),
        "the declaration carries no usable reality check"
    );
}

/// A route nothing answers is refused by name, with the declaration that asked
/// for it and the command that declares it. The predecessor could only fail as
/// `credential GITHUB_TOKEN.value is required`, naming no route and no way to
/// point Stado at another credential.
#[test]
fn a_github_route_nothing_declares_is_refused_by_name() {
    let port = skarbiec_answering(vec![(200, serde_json::json!({"routes": []}).to_string())]);
    let refused = credential_report(port);
    assert!(!refused.status.success(), "an unanswered route must refuse");
    let text = streams(&refused);
    for needle in [
        "github:org-runner-admin",
        "stado-rs/data/github-identity.json",
        "skarbiec route declare",
    ] {
        assert!(text.contains(needle), "refusal omits {needle:?}: {text}");
    }
}

/// Which coordinate Stado reads comes from the route, not from the binary.
/// Skarbiec names a fixture item and field; the read is then answered 404, so
/// the refusal has to say which coordinate it tried -- and it must never name
/// the id this declaration replaced.
#[test]
fn the_declared_github_route_decides_which_coordinate_stado_reads() {
    let resolved = serde_json::json!({
        "routes": [{
            "resource": "github:org-runner-admin",
            "item": FIXTURE_ITEM,
            "field": FIXTURE_FIELD,
            "declared_by": "table",
            "item_present": true,
            "field_present": true,
        }]
    });
    let port = skarbiec_answering(vec![
        (200, resolved.to_string()),
        (404, serde_json::json!({"error": "no item"}).to_string()),
    ]);
    let refused = credential_report(port);
    assert!(!refused.status.success(), "an empty credential must refuse");
    let text = streams(&refused);
    assert!(
        text.contains(&format!("{FIXTURE_ITEM}.{FIXTURE_FIELD}")),
        "refusal does not name the coordinate the route gave it: {text}"
    );
    assert!(
        !text.contains("GITHUB_TOKEN"),
        "a route-resolved read named the replaced item id: {text}"
    );
}
