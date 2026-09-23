//! `release_unit_image_revisit` names the launchd units the release agent may
//! put back on a replaced image. A unit a product retired is never one of
//! them: its work runs inside that product's one unit, and restarting it would
//! start the predecessor again beside it.
//!
//! On 2026-09-23 lukasz-macbook authorised com.wisent.transcript-lake-stream,
//! the streamer Transcript Lake retires, and not the declared
//! com.wisent.compute.service.transcript-lake. A new build would have been put
//! back into the retired unit, which retires nothing, and never into the one
//! that retires it, so two streamers of 680 MB each kept running.
//!
//! Every case drives the built `stado` against a local storage backend under a
//! tempdir, with HOME and STADO_CONFIG isolated, so the operator's registry is
//! never read or written.

use std::process::{Command, Output};

const PATH: &str = "release_unit_image_revisit.targets.m1.products.transcript-lake";
const DECLARED: &str = "com.wisent.compute.service.transcript-lake";
const RETIRED: &str = "com.wisent.transcript-lake-stream";

const REGISTRY: &str = r#"{
    "schema_version": 2,
    "coordinators": [],
    "public_origins": [],
    "targets": [
        {
            "name": "m1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "hostnames": ["m1.local"],
            "services": [],
            "release_platform": "darwin-arm64"
        }
    ],
    "release_unit_image_revisit": {
        "schema_version": 1,
        "targets": {
            "m1": {
                "state_dir": "/Users/m1/.stado/release-state",
                "products": {
                    "transcript-lake": ["com.wisent.compute.service.transcript-lake"]
                }
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

    fn authorised(&self) -> String {
        let out = self.stado(&["registry", "pull", "--path", PATH]);
        assert!(out.status.success(), "{}", stderr(&out));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_retired_unit_is_refused_and_the_authorisation_it_would_replace_stays() {
    let store = Store::new();
    let value = format!(r#"["{RETIRED}"]"#);
    let out = store.stado(&["registry", "set", "--path", PATH, "--value", &value]);
    assert!(!out.status.success(), "a retired unit was authorised");
    let said = stderr(&out);
    assert!(said.contains(&format!("{RETIRED} is retired")), "{said}");
    assert!(said.contains(DECLARED), "{said}");
    let kept = store.authorised();
    assert!(kept.contains(DECLARED), "{kept}");
    assert!(!kept.contains(RETIRED), "{kept}");
}

#[test]
fn the_declared_unit_that_retires_it_is_authorised() {
    let store = Store::new();
    let value = format!(r#"["{DECLARED}"]"#);
    let out = store.stado(&["registry", "set", "--path", PATH, "--value", &value]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(store.authorised().contains(DECLARED));
}
