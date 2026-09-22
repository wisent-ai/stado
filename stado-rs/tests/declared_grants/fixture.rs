//! The isolated store every case drives the binary against, and the directory
//! it is written for: one consumer that declares a grant, one that declares
//! none.

use std::path::Path;
use std::process::{Command, Output};

/// A directory whose consumer declares one grant, and one that declares none.
pub(crate) const REGISTRY: &str = r#"{
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

pub(crate) struct Store {
    pub(crate) home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Store {
    pub(crate) fn new() -> Self {
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
    pub(crate) fn ready() -> Self {
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

    pub(crate) fn stado(&self, args: &[&str]) -> Output {
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
    pub(crate) fn grants(&self, rest: &[&str]) -> Output {
        let mut argv = vec!["service", "grants"];
        argv.extend_from_slice(rest);
        self.stado(&argv)
    }
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn untouched(path: &Path) {
    assert!(
        !path.join(".stado").join("skarbiec.vault.json").exists(),
        "a read wrote a vault under {}",
        path.display()
    );
}
