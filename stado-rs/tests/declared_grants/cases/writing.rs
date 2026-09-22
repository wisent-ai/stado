//! Writing a declaration: refused while a host cannot read the field, refused
//! on a path that is not in the schema, and the generation moving with it.

use crate::fixture::{stderr, stdout, Store};

/// A host whose Stado predates the field parses the directory strictly and
/// would resolve nothing at all once a consumer carries it. On 2026-09-20 the
/// host every service resolves through ran 0.21.32 while the field arrived in
/// 0.21.35, so the write is refused until the fleet can read it.
#[test]
fn declaring_a_grant_is_refused_while_a_host_cannot_read_the_field() {
    let store = Store::new();
    let declaration = r#"[{"consumer":"weles-model-router-client","capabilities":["read:weles-model-router#token"],"token_file":"weles-model-router-skarbiec-token"}]"#;
    let path = "service_directory.services.brama.consumers.operator.grants";
    let out = store.stado(&["registry", "set", "--path", path, "--value", declaration]);
    assert!(!out.status.success());
    let said = stderr(&out);
    assert!(said.contains("older than 0.21.35"), "{said}");
    assert!(said.contains("w1 (declares no stado version)"), "{said}");
    assert!(said.contains("stado release host-state --host"), "{said}");
    assert!(
        said.contains("an installed binary can lag the version its registry entry declares"),
        "the refusal does not say a declaration is not an installation: {said}"
    );
}

/// A field no document carries yet has to be writable by the command the
/// documentation names, and a service directory that changed has to carry a
/// new generation or every resolver treats it as the document it already
/// read. Both were missing on 2026-09-20: the declaration this command reads
/// could not be made at all.
#[test]
fn a_field_no_document_carries_yet_can_be_written_and_a_typo_cannot() {
    let store = Store::new();
    let path = "targets.w1.managed_versions";
    let out = store.stado(&[
        "registry",
        "set",
        "--path",
        path,
        "--value",
        r#"{"stado":"0.21.36"}"#,
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = store.stado(&[
        "registry",
        "pull",
        "--path",
        "targets.w1.managed_versions.stado",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "0.21.36");

    // Only the last segment is created: a typo in the middle still names the
    // keys that exist, because inventing a host writes one nothing reads.
    let out = store.stado(&[
        "registry",
        "set",
        "--path",
        "targets.w9.managed_versions",
        "--value",
        r#"{"stado":"0.21.36"}"#,
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("has no element named `w9`"),
        "{}",
        stderr(&out)
    );
}

/// A service directory that changed and kept its generation is the document
/// every resolver believes it has already read, and the validator refuses it.
/// The number moves with the change now, in the same command.
#[test]
fn a_directory_change_moves_its_generation() {
    let store = Store::new();
    let out = store.stado(&[
        "registry",
        "set",
        "--path",
        "service_directory.services.kronika.consumers.operator.capabilities",
        "--value",
        r#"["read","write"]"#,
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = store.stado(&["registry", "pull", "--path", "service_directory.generation"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "2",
        "the directory changed and its generation did not"
    );
}

/// The other side of that refusal: once every host declares a Stado that can
/// read the field, the same write goes through and the document carries it.
///
/// `Store::ready` was written for this case on 2026-09-20 and the case was
/// not, so the fleet gate had only its refusal proved, and the helper sat
/// unused until the release worker's clippy gate refused to build Stado at
/// all.
#[test]
fn the_same_declaration_is_written_once_every_host_can_read_the_field() {
    let store = Store::ready();
    let declaration = r#"[{"consumer":"weles-model-router-client","capabilities":["read:weles-model-router#token"],"token_file":"weles-model-router-skarbiec-token"}]"#;
    let path = "service_directory.services.brama.consumers.operator.grants";

    let out = store.stado(&["registry", "set", "--path", path, "--value", declaration]);
    assert!(out.status.success(), "{}", stderr(&out));

    let out = store.stado(&["registry", "pull", "--path", path]);
    assert!(out.status.success(), "{}", stderr(&out));
    let written: serde_json::Value =
        serde_json::from_str(stdout(&out).trim()).expect("the declaration reads back as JSON");
    assert_eq!(
        written[0]["consumer"], "weles-model-router-client",
        "the declaration the fleet accepted is not the one that was written: {written}"
    );
    assert_eq!(
        written[0]["capabilities"][0], "read:weles-model-router#token",
        "the grant lost its capability on the way into the document: {written}"
    );

    let out = store.grants(&["brama", "--consumer", "operator"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("weles-model-router-client"),
        "the accepted declaration is not what `service grants` reads back: {}",
        stdout(&out)
    );
}
