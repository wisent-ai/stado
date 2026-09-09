//! The documents this area declares: the fleet registry of exactly this
//! machine, the release-control policy `stado release submit` requires before
//! it will look for a builder, and the capacity publication a worker writes
//! about itself.
//!
//! Every field here is copied from a live run of the product against this
//! machine — the registry the product validates, the `release_control` block
//! its rollback and signing steps read, and the `capacity/<consumer>.json`
//! document `queue::capacity::publish_capacity` writes. None of it tunes any
//! product behaviour; the numbers are the schema versions and time bounds the
//! product's own validator demands of a declaration.

use std::path::Path;

use serde_json::{json, Value};

/// The product this area submits. Its own name, so a run of this area cannot
/// be confused with a real product's run in a shared store.
pub const PRODUCT: &str = "builder-claim-probe";

/// The version the source tree declares and `--version` must agree with.
pub const VERSION: &str = "9.9.9";

/// The registry name of the one declared host: this machine.
pub const TARGET: &str = "builder-claim-host";

/// The logical service the release-control product owns. A product whose
/// service is not in the service directory is refused by registry validation.
const SERVICE: &str = "builder-claim-probe";

/// The release platform of the machine the area runs on, spelled the way the
/// registry and the platform recipe spell it.
pub fn platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("no release platform is declared for {os}-{arch}"),
    }
}

/// This machine's own kernel host name, normalized the way the product
/// requires a declared host name to be spelled.
pub fn hostname() -> String {
    let out = std::process::Command::new("/bin/hostname")
        .output()
        .expect("the kernel answers its host name");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .to_ascii_lowercase()
}

/// The consumer id a local queue agent publishes under, which is also the
/// consumer a claimed build is pinned to.
pub fn consumer() -> String {
    format!("local-{}", hostname())
}

/// The product manifest `stado release submit` reads out of the source tree.
///
/// One platform, built by `true`: this area never lets a build run, because
/// what it is about is which host is allowed to receive the build at all.
pub fn manifest() -> Value {
    json!({
        "schema_version": 1,
        "product": PRODUCT,
        "releases": true,
        "version_source": {
            "kind": "regex",
            "path": "VERSION",
            "pattern": "(?m)^(?P<version>[^\\s]+)\\s*$"
        },
        "platforms": {
            platform(): {
                "runner_platform": platform(),
                "quality": [],
                "build": {"argv": ["true"]},
                "stage": {"out": "builder-claim-probe"}
            }
        },
        "promotion": {"channels": ["candidate"], "reconcile": false},
        "deliveries": []
    })
}

/// The fleet document: this machine, declared for its own release platform,
/// with the release-control policy the submit pipeline requires.
///
/// `command` is the built binary under test, because the service directory's
/// authority names the executable an operator would run.
pub fn registry(home: &Path, binary: &Path) -> Value {
    json!({
        "schema_version": 2,
        "targets": [{
            "name": TARGET,
            "kind": "local",
            "ssh": "nobody@127.0.0.1",
            "release_platform": platform(),
            "hostnames": [hostname()],
            "services": [{
                "kind": "launchd",
                "name": SERVICE,
                "label": SERVICE,
                "path": home.join("Library/LaunchAgents").join(format!("{SERVICE}.plist")),
                "unit": ""
            }]
        }],
        "service_directory": {
            "authority": {"target": TARGET, "command": binary},
            "generation": 1,
            "services": {
                SERVICE: {
                    "active_host": TARGET,
                    "managed_service": SERVICE,
                    "endpoints": {TARGET: {"url": "http://127.0.0.1:1"}},
                    "consumers": {"builder-claim": {"capabilities": ["release"]}}
                }
            }
        },
        "release_control": release_control(home),
    })
}

/// The release-control block for this product on this machine. Its own
/// function so the registry above stays readable.
fn release_control(home: &Path) -> Value {
    json!({
        "schema_version": 1,
        "generation": 1,
        "trusted_keys": {},
        "products": {
            PRODUCT: {
                "service": SERVICE,
                "config_schema": 1,
                "state_schema": 1,
                "install_root": home.join(".stado/services").join(SERVICE),
                "binary": "bin/builder-claim-probe",
                "launcher": "bin/builder-claim-probe",
                "binary_env": "BUILDER_CLAIM_PROBE_BIN",
                "port_env": "BUILDER_CLAIM_PROBE_PORT",
                "runtime_env": "BUILDER_CLAIM_PROBE_RUNTIME",
                "environment": {},
                "signing_key_item": "builder-claim-signing",
                "signing_key_id": "builder-claim-key",
                "strategy": {
                    "kind": "replace",
                    "readiness_timeout_seconds": 30,
                    "drain_timeout_seconds": 30,
                    "rollback_window_seconds": 300,
                    "automatic_rollback": false
                },
                "targets": {
                    TARGET: {
                        "platform": platform(),
                        "run_as_user": "builder-claim",
                        "home": home,
                        "state_dir": home.join(".stado/release-state"),
                        "runtime_root": home.join(".stado/run"),
                        "logs_root": home.join(".stado/logs"),
                        "readiness_path": "/healthz"
                    }
                }
            }
        }
    })
}

/// One capacity publication, in the shape `publish_capacity` writes it: the
/// measured resources, and the worker's own admission decision when it made
/// one. `accepting_jobs` is left out entirely when `accepting` is `None`,
/// which is what a worker running an older build publishes during a rolling
/// upgrade.
pub fn publication(accepting: Option<Value>, diag: Value) -> Value {
    let mut payload = json!({
        "consumer_id": consumer(),
        "kind": "local",
        "running_jobs": 0,
        "total_cpu_cores": 12,
        "available_cpu_cores": 7,
        "free_ram_gb": 32,
        "total_ram_gb": 64,
        "free_vram_gb": 0,
        "total_vram_gb": 0,
        "available_accelerators": {},
        "published_at": chrono::Utc::now().to_rfc3339(),
        "diag": diag,
    });
    if let Some(accepting) = accepting {
        payload["accepting_jobs"] = accepting;
    }
    payload
}
