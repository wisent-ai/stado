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

#[test]
fn identity_migration_refuses_missing_bearer_then_persists_valid_cutover() {
    let fixture = SkarbiecFixture::new();
    let path = fixture.root.join(".stado/config.json");
    let command = |args: &[&str]| {
        fixture.command(std::path::Path::new(env!("CARGO_BIN_EXE_stado")))
            .env("STADO_CONFIG", &path).args(args).output().expect("run Stado config CLI")
    };
    assert_success(&command(&["config", "init"]), "initialize isolated config");
    let mut document: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    document["credentials"]["admin"] = serde_json::json!({"consumer":"old-admin", "token_file":"old-token"});
    document["object_api"]["skarbiec"] = serde_json::json!({"consumer":"old-object", "token_file":"old-object-token"});
    document["secrets"]["skarbiec"]["token_file"] = Value::from(fixture.token.to_string_lossy().as_ref());
    fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&document).unwrap())).unwrap();
    let before = fs::read(&path).unwrap();

    let refused = command(&["config", "migrate-identities"]);
    assert!(!refused.status.success(), "missing bearer was accepted");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no Stado bearer file"));
    assert_eq!(fs::read(&path).unwrap(), before, "refusal changed the config");

    fixture.grant_username();
    assert_success(&command(&["config", "migrate-identities"]), "migrate identity config");
    let actual: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(actual["secrets"]["skarbiec"]["consumer"], "stado");
    assert!(actual["credentials"].get("admin").is_none());
    assert!(actual["object_api"].get("skarbiec").is_none());
    assert_eq!(fs::read(format!("{}.before-identity-migration", path.display())).unwrap(), before);
    assert_success(&command(&["config", "validate"]), "validate migrated config");
}
