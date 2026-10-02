use super::Isolated;

#[test]
fn an_authorized_unplaced_database_resolves_its_credential_coordinate() {
    let isolated = Isolated::new("unplaced-resolution");
    isolated.succeed(&[
        "database",
        "declare",
        "ledger",
        "--engine",
        "mysql",
        "--consumer",
        "probe",
        "--json",
    ]);
    let report = isolated.succeed(&[
        "database",
        "resolve",
        "ledger",
        "--consumer",
        "probe",
        "--json",
    ]);
    assert_eq!(report["engine"], "mysql");
    assert_eq!(report["placed"], false);
    assert_eq!(report["credential_item"], "ledger-database");
    assert!(report.get("endpoint").is_none());
}

#[test]
fn a_malformed_present_directory_is_not_reported_as_an_unplaced_database() {
    let isolated = Isolated::new("malformed-directory");
    isolated.succeed(&[
        "database",
        "declare",
        "ledger",
        "--engine",
        "mysql",
        "--consumer",
        "probe",
        "--json",
    ]);
    let path = isolated
        .directory
        .join(".stado/local-storage/registry.json");
    let mut registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).expect("read the isolated initialized registry"),
    )
    .expect("parse the isolated registry");
    registry["service_directory"] = serde_json::Value::Null;
    let corrupt = serde_json::to_vec(&registry).expect("encode the malformed directory fixture");
    std::fs::write(&path, &corrupt).expect("damage only the isolated test registry");
    isolated.refuse(&["database", "list", "--json"], None, "service_directory");
    assert_eq!(
        std::fs::read(&path).expect("read the refused registry"),
        corrupt
    );
}

#[test]
fn a_present_null_database_plane_is_not_reported_as_an_empty_list() {
    let isolated = Isolated::new("null-database-plane");
    let mut document: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&isolated.config).expect("read the isolated configuration"),
    )
    .expect("parse the isolated configuration");
    document["database_api"] = serde_json::Value::Null;
    let corrupt = serde_json::to_vec(&document).expect("encode the malformed database plane");
    std::fs::write(&isolated.config, &corrupt).expect("damage only the isolated configuration");
    isolated.refuse(
        &["database", "list", "--json"],
        None,
        "database_api.databases",
    );
}
