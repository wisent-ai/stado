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
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"]
        }
    ],
    "service_directory": {
        "authority": {"target": "w1", "command": "/usr/local/bin/stado"},
        "generation": 1,
        "services": {
            "brama": {
                "active_host": "w1",
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
