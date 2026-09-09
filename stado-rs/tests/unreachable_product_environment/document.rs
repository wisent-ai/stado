//! What the isolated registry declares: the product policy, the service
//! records and the names the cases address them by.
//!
//! Split out of `fixture.rs` so each file in this folder stays inside the
//! three hundred line limit this repository enforces on itself.

use std::path::Path;

use serde_json::{json, Value};

/// The product whose release policy declares an environment, and the two
/// variables it declares. `AUDIT` is the one the incident lost.
///
/// A product the compiled catalog really carries, so `managed_versions` and
/// the recommended commands in the sentences below are ones this build would
/// accept rather than names invented for the fixture.
pub const PRODUCT: &str = "skarbiec";
pub const AUDIT: &str = "SKARBIEC_AUDIT_FILE";
pub const VAULT: &str = "SKARBIEC_VAULT_FILE";

pub const UNRECORDED: &str = "unrecorded-service-environment";
pub const UNTARGETED: &str = "untargeted-product-host";

/// The adopted stub: a record that names a path and declares nothing about
/// what runs there. This is the incident's control-plane shape exactly.
pub const ADOPTED: &str = "com.wisent.compute.service.skarbiec-control-plane";

/// A unit whose declaration IS recorded — program and args both — on the same
/// host and for the same product, so nothing is missing from the document
/// about it.
pub const RECORDED: &str = "com.wisent.skarbiec-recorded";

/// A unit that mentions the product nowhere in its identifier. The product a
/// unit serves is a delimited segment of its label, never a substring of it.
pub const UNRELATED: &str = "com.wisent.transcript-lake-stream";

/// The program the incident's hand-authored launcher stood for: a real file
/// on every machine that can run this suite, so a recorded declaration names
/// something that exists.
pub const LAUNCHER: &str = "/bin/sh";

/// The release platform this machine really is, in the product's own spelling.
pub fn platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// A `services[]` element as `stado service adopt` writes one.
///
/// An empty `program` is the adopted-stub shape: the record names a path and
/// declares nothing about what runs there.
pub fn unit(label: &str, program: &str, path: &Path) -> Value {
    json!({
        "name": label,
        "unit": "",
        "label": label,
        "path": path.display().to_string(),
        "kind": "launchd",
        "program": program,
        "args": [],
        "managed_since": "2026-09-01T23:00:10.518396+00:00",
    })
}

/// A `release_control` block carrying one product that declares an
/// environment, targeting the hosts named in `targets`.
pub fn release_control(home: &Path, targets: &[&str]) -> Value {
    let home = home.display().to_string();
    let policy_target = json!({
        "platform": platform(),
        "run_as_user": "stado",
        "home": home,
        "state_dir": format!("{home}/.stado/release-state"),
        "runtime_root": format!("{home}/.stado/run"),
        "logs_root": format!("{home}/.stado/logs"),
        "stable_bind": "127.0.0.1:8799",
        "candidate_ports": [18799, 18800],
        "readiness_path": "/health",
    });
    let targets: serde_json::Map<String, Value> = targets
        .iter()
        .map(|host| ((*host).to_string(), policy_target.clone()))
        .collect();
    json!({
        "schema_version": 1,
        "generation": 4,
        "trusted_keys": {},
        "products": {
            PRODUCT: {
                "service": PRODUCT,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": format!("{home}/.stado/services/{PRODUCT}"),
                "binary": "bin/skarbiec",
                "launcher": "bin/start",
                "binary_env": "SKARBIEC_BIN",
                "port_env": "SKARBIEC_PORT_OVERRIDE",
                "runtime_env": "SKARBIEC_RUNTIME_DIR",
                "environment": {
                    AUDIT: "{home}/.stado/skarbiec.audit.jsonl",
                    VAULT: "{home}/.stado/skarbiec.vault.json",
                },
                "strategy": {
                    "kind": "blue-green",
                    "readiness_timeout_seconds": 90,
                    "drain_timeout_seconds": 60,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": true,
                },
                "targets": targets,
            }
        },
    })
}
