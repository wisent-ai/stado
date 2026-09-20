//! `stado service grants <SERVICE>` — the grants a service's consumers
//! declare, read from the registry rather than typed as flags.
//!
//! A consumer's grant existed only as `grant-sync` flags somebody remembered,
//! and 26 grants in the week of 2026-09-12 were issued from the shell instead,
//! one of them into the vault replica its owner overwrote within the hour.
//! Oko's judge named the missing product side on 2026-09-20: "deklaracja
//! konsumentów mintująca granty automatycznie z rejestru".
//!
//! Every case drives the built `stado` against a local storage backend under a
//! tempdir, with HOME and STADO_CONFIG isolated, so the operator's registry,
//! vault and cache are never read or written. Nothing here mints: minting
//! reaches a managed host, and what is checked here is the declaration the
//! minting reads and the refusals that keep it honest.

use std::path::Path;
use std::process::{Command, Output};

/// A directory whose consumer declares one grant, and one that declares none.
const REGISTRY: &str = r#"{
    "schema_version": 2,
    "coordinators": [],
    "public_origins": [],
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "hostnames": ["w1.local"],
            "services": [
                {"name": "brama", "kind": "launchd", "label": "brama", "path": "/tmp/brama.plist", "unit": ""},
                {"name": "kronika", "kind": "launchd", "label": "kronika", "path": "/tmp/kronika.plist", "unit": ""}
            ],
            "release_platform": "linux-amd64"
        }
    ],
    "service_directory": {
        "authority": {"target": "w1", "command": "/usr/local/bin/stado"},
        "generation": 1,
        "services": {
            "brama": {
                "active_host": "w1",
                "managed_service": "brama",
                "endpoints": {"w1": {"url": "http://127.0.0.1:17651"}},
                "consumers": {
                    "oko": {
                        "capabilities": ["model-routing"],
                        "grants": [
                            {
                                "consumer": "oko-model-router-client",
                                "capabilities": ["read:oko-model-router#token"],
                                "token_file": "oko-model-router-skarbiec-token"
                            }
                        ]
                    },
                    "operator": {"capabilities": ["model-routing"]}
                }
            },
            "kronika": {
                "active_host": "w1",
                "managed_service": "kronika",
                "endpoints": {"w1": {"url": "http://127.0.0.1:18080"}},
                "consumers": {"operator": {"capabilities": ["read"]}}
            }
        }
    }
}"#;

struct Store {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Store {
    fn new() -> Self {
        let store = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(store.storage.path().join("registry.json"), REGISTRY).unwrap();
        store
    }

    /// The same fleet, with its host declaring a Stado that knows `grants`.
    /// Writing a declaration is refused until every host does, which is the
    /// case above; this is the fleet that has been brought forward.
    fn ready() -> Self {
        let store = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        let ready = REGISTRY.replace(
            r#""hostnames": ["w1.local"]"#,
            r#""hostnames": ["w1.local"], "managed_versions": {"stado": "0.21.36"}"#,
        );
        std::fs::write(store.storage.path().join("registry.json"), ready).unwrap();
        store
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("stado binary runs")
    }

    /// The command under test, with whatever this case asks it.
    fn grants(&self, rest: &[&str]) -> Output {
        let mut argv = vec!["service", "grants"];
        argv.extend_from_slice(rest);
        self.stado(&argv)
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn untouched(path: &Path) {
    assert!(
        !path.join(".stado").join("skarbiec.vault.json").exists(),
        "a read wrote a vault under {}",
        path.display()
    );
}

#[test]
fn a_declared_grant_is_printed_with_what_minting_would_use() {
    let store = Store::new();
    let out = store.grants(&["brama"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let said = stdout(&out);
    assert!(said.contains("brama on w1:"), "{said}");
    assert!(
        said.contains("declared grant(s); nothing was minted"),
        "{said}"
    );
    assert!(said.contains("oko-model-router-client"), "{said}");
    assert!(said.contains("read:oko-model-router#token"), "{said}");
    assert!(said.contains("oko-model-router-skarbiec-token"), "{said}");
    assert!(
        said.contains("stado service grants brama --apply"),
        "the plan does not name what mints it: {said}"
    );
    untouched(store.home.path());
}

#[test]
fn the_declaration_is_readable_as_json_and_says_it_was_not_applied() {
    let store = Store::new();
    let out = store.grants(&["brama", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&stdout(&out)).expect("the plan prints JSON");
    assert_eq!(rows.len(), REGISTRY.matches("\"token_file\"").count());
    assert_eq!(rows[0]["authorized"], "oko");
    assert_eq!(rows[0]["consumer"], "oko-model-router-client");
    assert_eq!(
        rows[0]["audience"], "oko-model-router-client",
        "the audience defaults to the consumer"
    );
    assert_eq!(rows[0]["host"], "w1");
    assert_eq!(rows[0]["applied"], false);
}

#[test]
fn a_service_with_no_declaration_is_refused_with_where_one_goes() {
    let store = Store::new();
    let out = store.grants(&["kronika"]);
    assert!(!out.status.success());
    let said = stderr(&out);
    assert!(
        said.contains("no consumer of kronika declares a grant"),
        "{said}"
    );
    assert!(
        said.contains("service_directory.services.kronika.consumers.<consumer>.grants"),
        "the refusal does not say where a declaration goes: {said}"
    );
    assert!(said.contains("stado registry set --path"), "{said}");
}

#[test]
fn an_unknown_service_and_an_unauthorized_consumer_are_refused_with_the_names() {
    let store = Store::new();
    let out = store.grants(&["nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("carries no service \"nope\"; services there: brama, kronika"),
        "{}",
        stderr(&out)
    );

    let out = store.grants(&["brama", "--consumer", "weles"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "brama does not authorize consumer \"weles\"; consumers there: oko, operator"
        ),
        "{}",
        stderr(&out)
    );
}

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
