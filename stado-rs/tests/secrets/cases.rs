//! The secret-sync refusals.
use super::*;

#[test]
fn secrets_put_writes_a_typed_item_to_real_skarbiec() {
    let fixture = SkarbiecFixture::new();
    let put = fixture.stado(
        &["credentials", "put", "stado-cli-login", "--type", "login"],
        Some(r#"{"username":"alice","password":"not-returned"}"#),
    );
    assert_success(&put, "stado credentials put");

    let stored = fixture.skarbiec(&["get", "stado-cli-login"]);
    assert_success(&stored, "read fixture state with Skarbiec");
    let document: Value = serde_json::from_slice(&stored.stdout).expect("stored item is JSON");
    assert_eq!(document["kind"], "login");
    assert_eq!(document["fields"]["username"], "alice");
    assert_eq!(document["fields"]["password"], "not-returned");
}

#[test]
fn secrets_get_reads_only_the_granted_field_from_real_skarbiec() {
    let mut fixture = SkarbiecFixture::new();
    fixture.seed_login();
    fixture.grant_username();
    fixture.start_server();
    let before = fs::read(&fixture.vault).expect("read the declared credential state");

    let get = fixture.stado(
        &[
            "credentials",
            "get",
            "stado-cli-login",
            "--field",
            "username",
        ],
        None,
    );
    assert_success(&get, "stado credentials get");
    assert_eq!(String::from_utf8_lossy(&get.stdout), "alice\n");

    let refused = fixture.stado(
        &[
            "credentials",
            "get",
            "stado-cli-login",
            "--field",
            "password",
        ],
        None,
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("consumer not authorized to read item field"),
        "unexpected refusal: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(fs::read(&fixture.vault).unwrap(), before);
}
