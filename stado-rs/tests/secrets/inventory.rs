use super::{assert_success, fs, SkarbiecFixture, Value};
use std::os::unix::fs::PermissionsExt;

#[test]
fn local_inventory_filters_real_items_and_refuses_an_unprotected_vault() {
    let fixture = SkarbiecFixture::new();
    fixture.seed_login();
    fixture.grant_username();
    let seeded = fixture.skarbiec(&[
        "set",
        "unrelated-note",
        "--type",
        "note",
        "value=not-a-credential",
    ]);
    assert_success(&seeded, "seed unrelated item");
    let before = fs::read(&fixture.vault).unwrap();
    let path = fixture.vault.to_str().unwrap();
    let matched = fixture.stado(
        &[
            "credentials",
            "inspect-vault",
            path,
            "--match",
            "CLI-LOGIN",
            "--json",
        ],
        None,
    );
    assert_success(&matched, "read matching metadata");
    let report: Value = serde_json::from_slice(&matched.stdout).unwrap();
    let items = report["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["stado-cli-login"]
    );
    assert!(report["grants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|grant| grant["consumer"] == "stado-control-plane"));
    let absent = fixture.stado(
        &[
            "credentials",
            "inspect-vault",
            path,
            "--match",
            "no-such-item",
            "--json",
        ],
        None,
    );
    assert_success(&absent, "empty match is an inventory result");
    let report: Value = serde_json::from_slice(&absent.stdout).unwrap();
    assert_eq!(report["items"], serde_json::json!([]));
    assert_eq!(report["count"], 0);
    assert_eq!(fs::read(&fixture.vault).unwrap(), before);

    fs::set_permissions(&fixture.vault, fs::Permissions::from_mode(0o644)).unwrap();
    let refused = fixture.stado(&["credentials", "inspect-vault", path, "--json"], None);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("vault must be an owner-only regular local file"));
    assert_eq!(fs::read(&fixture.vault).unwrap(), before);
}
