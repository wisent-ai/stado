//! Signed release recovery through the real CLI, public canonical channel, and
//! a dedicated physical fixture host. Probierz supplies the immutable release
//! coordinates and retains the command evidence; plain `cargo test` never
//! contacts or mutates a host.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

const MUTATION_ACK: &str = "dedicated-recovery-fixture";

struct Journey {
    work: tempfile::TempDir,
    registry: Value,
    target: String,
    version: String,
    key: PathBuf,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required by the recovery journey"))
}


impl Journey {
    fn load(version_variable: &str) -> Self {
        let work = tempfile::tempdir().expect("temporary recovery root");
        let registry_path = PathBuf::from(required("STADO_RECOVERY_REGISTRY"));
        let registry: Value = serde_json::from_slice(
            &std::fs::read(&registry_path).expect("recovery registry is readable"),
        )
        .expect("recovery registry is JSON");
        let target = required("STADO_RECOVERY_TARGET");
        let version = required(version_variable);
        assert!(
            registry["release_control"]["products"].get("stado").is_none(),
            "the recovery journey covers an operator-pinned Stado release, not rollout desired state"
        );
        let entry = registry["targets"]
            .as_array()
            .and_then(|targets| targets.iter().find(|entry| entry["name"] == target))
            .unwrap_or_else(|| panic!("dedicated target {target:?} is present in recovery registry"));
        assert!(
            entry["ssh"].as_str().is_some_and(|ssh| !ssh.is_empty()),
            "dedicated recovery target declares ssh"
        );
        let key = PathBuf::from(required("STADO_RECOVERY_SSH_KEY_FILE"));
        assert!(key.is_file(), "dedicated recovery SSH key exists");
        Self {
            work,
            registry,
            target,
            version,
            key,
        }
    }

    fn registry_path(&self, document: &Value) -> PathBuf {
        let storage = self.work.path().join("storage");
        std::fs::create_dir_all(&storage).expect("temporary local storage exists");
        std::fs::write(
            storage.join("registry.json"),
            serde_json::to_vec_pretty(document).expect("registry encodes"),
        )
        .expect("temporary registry is written");
        storage
    }

    fn run_recovery(&self, document: &Value, release: bool) -> Output {
        let storage = self.registry_path(document);
        let mut args = vec!["host", "recover", self.target.as_str()];
        if release {
            args.extend(["--release", self.version.as_str()]);
        }
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.work.path().join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("STADO_CONFIG", self.work.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", storage)
            .env("STADO_HOST_SSH_KEY_FILE", &self.key)
            .output()
            .expect("built Stado CLI starts")
    }

    fn stado(&self, document: &Value) -> Output {
        self.run_recovery(document, true)
    }

    fn recover_current(&self) -> Output {
        self.run_recovery(&self.registry, false)
    }

}

fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "recovery stdout is one JSON report ({error}): {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn step_status<'a>(report: &'a Value, name: &str) -> Option<&'a str> {
    report["release"]["steps"]
        .as_array()?
        .iter()
        .rev()
        .find(|entry| entry["step"] == name)?["status"]
        .as_str()
}

#[test]
#[ignore = "Probierz supplies a registry key and a release from the real signed channel"]
fn cli_refuses_a_manifest_whose_key_is_not_trusted_by_the_loaded_registry() {
    let journey = Journey::load("STADO_RECOVERY_VERSION");
    let mut wrong_key = journey.registry.clone();
    let keys = wrong_key["release_control"]["trusted_keys"]
        .as_object_mut()
        .expect("trusted_keys is an object");
    let old_keys = std::mem::take(keys);
    let mut renamed = serde_json::Map::new();
    for (index, (old_id, public_key)) in old_keys.into_iter().enumerate() {
        let new_id = format!("unrelated-recovery-key-{index}");
        keys.insert(new_id.clone(), public_key);
        renamed.insert(old_id, json!(new_id));
    }
    for product in wrong_key["release_control"]["products"]
        .as_object_mut()
        .expect("products is an object")
        .values_mut()
    {
        for slot in ["desired", "previous"] {
            let Some(artifacts) = product[slot]["artifacts"].as_object_mut() else {
                continue;
            };
            for artifact in artifacts.values_mut() {
                let Some(old_id) = artifact["key_id"].as_str() else {
                    continue;
                };
                if let Some(new_id) = renamed.get(old_id) {
                    artifact["key_id"] = new_id.clone();
                }
            }
        }
    }
    let rejected = journey.stado(&wrong_key);
    assert!(!rejected.status.success(), "untrusted signing key must be refused");
    let rejected = report(&rejected);
    assert_eq!(step_status(&rejected, "download"), Some("ok"));
    assert_eq!(step_status(&rejected, "verify"), Some("failed"));
    assert!(
        rejected["error"]
            .as_str()
            .is_some_and(|error| error.contains("not trusted by registry")),
        "the refusal names the release trust failure: {rejected}"
    );
    assert_eq!(step_status(&rejected, "install"), None);
}

#[test]
#[ignore = "Probierz supplies a signed canonical-channel fixture with mismatched release identity"]
fn cli_refuses_a_signed_manifest_whose_identity_differs_from_its_coordinate() {
    let journey = Journey::load("STADO_RECOVERY_IDENTITY_VERSION");
    let rejected = journey.stado(&journey.registry);
    assert!(!rejected.status.success(), "identity mismatch must be refused");
    let rejected = report(&rejected);
    assert_eq!(step_status(&rejected, "download"), Some("ok"));
    assert_eq!(step_status(&rejected, "verify"), Some("failed"));
    assert!(rejected["error"]
        .as_str()
        .is_some_and(|error| error.contains("identity does not match its object coordinate")));
    assert_eq!(step_status(&rejected, "install"), None);
}

#[test]
#[ignore = "Probierz supplies a signed canonical-channel fixture whose archive mismatches its manifest"]
fn cli_refuses_an_archive_that_differs_from_its_signed_digest_or_size() {
    let journey = Journey::load("STADO_RECOVERY_ARCHIVE_MISMATCH_VERSION");
    let rejected = journey.stado(&journey.registry);
    assert!(!rejected.status.success(), "archive mismatch must be refused");
    let rejected = report(&rejected);
    assert_eq!(step_status(&rejected, "download"), Some("ok"));
    assert_eq!(step_status(&rejected, "verify"), Some("failed"));
    assert!(rejected["error"].as_str().is_some_and(|error| {
        error.contains("differs from its signed manifest")
            || error.contains("exceeds its signed artifact_bytes")
    }));
    assert_eq!(step_status(&rejected, "install"), None);
}

#[test]
#[ignore = "Probierz supplies and records the dedicated physical recovery fixture"]
fn cli_preserves_the_old_binary_and_installs_without_a_local_or_old_remote_resolver() {
    assert_eq!(required("STADO_RECOVERY_ALLOW_MUTATION"), MUTATION_ACK);
    let journey = Journey::load("STADO_RECOVERY_VERSION");

    let output = journey.stado(&journey.registry);
    let document = report(&output);
    assert!(
        output.status.success(),
        "signed recovery failed: {document}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["download", "verify", "backup", "install", "resolver", "recovery"] {
        assert_eq!(step_status(&document, name), Some("ok"), "{name}: {document}");
    }
    assert_eq!(step_status(&document, "rollback"), None);
}

#[test]
#[ignore = "Probierz supplies a signed negative fixture whose binary refuses resolver --help"]
fn failed_resolver_probe_atomically_restores_the_previous_binary() {
    assert_eq!(required("STADO_RECOVERY_ALLOW_MUTATION"), MUTATION_ACK);
    let journey = Journey::load("STADO_RECOVERY_ROLLBACK_VERSION");

    let output = journey.stado(&journey.registry);
    assert!(!output.status.success(), "negative resolver fixture must be refused");
    let document = report(&output);
    assert_eq!(step_status(&document, "install"), Some("ok"));
    assert_eq!(step_status(&document, "resolver"), Some("failed"));
    assert_eq!(step_status(&document, "rollback"), Some("restored"));
    assert_eq!(step_status(&document, "recovery"), None);
    let restored = journey.recover_current();
    assert!(
        restored.status.success(),
        "the restored prior Stado binary completes ordinary recovery: {}{}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
}
