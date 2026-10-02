mod support;
use support::Run;

#[test]
fn named_field_read_does_not_select_a_same_named_role_or_broaden_its_grant() {
    let mut run = Run::new();
    let owner = "Credential Test <credential@example.invalid>";
    run.vault(&["init", owner], None);
    let item = uuid::Uuid::new_v4().simple().to_string();
    let decoy = uuid::Uuid::new_v4().simple().to_string();
    let expected = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    run.store(&item, &expected);
    run.store(&decoy, &other);
    run.start(&item, &decoy);
    let saved = run.vault(&["get", &item], None);
    assert_eq!(saved["fields"]["token"], expected);
    let untagged = run.get(&item, "token");
    assert!(
        untagged.status.success(),
        "{}",
        String::from_utf8_lossy(&untagged.stderr)
    );
    assert_eq!(untagged.stdout, format!("{expected}\n").as_bytes());

    run.vault(
        &["retag", &decoy, "--tags", &format!("stado:role:{item}")],
        None,
    );
    let shadowed = run.get(&item, "token");
    assert!(
        shadowed.status.success(),
        "{}",
        String::from_utf8_lossy(&shadowed.stderr)
    );
    assert_eq!(shadowed.stdout, format!("{expected}\n").as_bytes());

    let denied = run.get(&item, "private");
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    let diagnostic = String::from_utf8(denied.stderr).unwrap();
    assert!(
        diagnostic.contains(&item) && diagnostic.contains("private"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("not-granted"));
    assert_eq!(run.vault(&["get", &item], None), saved);
    run.completed = true;
}
