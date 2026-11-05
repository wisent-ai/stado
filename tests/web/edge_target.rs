//! A public edge declared on a host the canonical registry does not hold is
//! refused where it is declared and read, not only when a hostname is
//! converged: through the real `stado` binary against one isolated
//! deployment, a local store holding a registry and a config declaring the
//! edge.
//!
//! An edge on `absent-edge-host`, which the registry does not hold, is refused
//! by `stado config validate` and `stado web edge status`, each naming the
//! host and the two commands that fix it; the same edge on `marker-host`,
//! which the registry holds, passes `config validate`.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

const ABSENT: &str = "absent-edge-host";
const REGISTERED: &str = "marker-host";
const REFUSAL: &str = "which the canonical registry does not hold";

struct Deployment {
    root: PathBuf,
}

impl Deployment {
    fn start(case: &str, edge_target: &str) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/web-edge-target")
            .join(format!("{case}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join(".stado")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        let registry: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        fs::write(
            root.join("store/ecosystem/probierz/registry.json"),
            serde_json::to_vec_pretty(&registry).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(".stado/config.json"),
            serde_json::to_vec_pretty(&json!({
                "web_api": {
                    "products": {},
                    "edge": {
                        "target": edge_target,
                        "address": "192.0.2.1",
                        "contact": "edge-test@example.com",
                        "probe_seconds": 2,
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        Self { root }
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado/config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove the deployment directory");
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn refused_naming_the_fix(output: &Output, command: &str) {
    let answer = said(output);
    assert!(
        !output.status.success(),
        "{command} accepted an edge on {ABSENT}: {answer}"
    );
    for word in [
        ABSENT,
        REFUSAL,
        "stado web edge provision",
        "stado web edge declare",
    ] {
        assert!(
            answer.contains(word),
            "{command} does not name {word}: {answer}"
        );
    }
}

#[test]
fn config_validate_refuses_an_edge_on_an_unregistered_host() {
    let deployment = Deployment::start("validate-absent", ABSENT);
    refused_naming_the_fix(
        &deployment.stado(&["config", "validate"]),
        "config validate",
    );
}

#[test]
fn edge_status_refuses_an_edge_on_an_unregistered_host() {
    let deployment = Deployment::start("status-absent", ABSENT);
    refused_naming_the_fix(&deployment.stado(&["web", "edge", "status"]), "edge status");
}

#[test]
fn config_validate_accepts_the_edge_once_its_host_is_registered() {
    let deployment = Deployment::start("validate-registered", REGISTERED);
    let validated = deployment.stado(&["config", "validate"]);
    assert!(
        validated.status.success(),
        "config validate refused an edge on {REGISTERED}: {}",
        said(&validated)
    );
    assert!(
        !said(&validated).contains(REFUSAL),
        "a registered edge host was refused: {}",
        said(&validated)
    );
}
