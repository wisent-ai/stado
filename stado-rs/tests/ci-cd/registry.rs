//! The registry document the release journeys read.
//!
//! One local target that is this machine, the service it hosts, the trusted
//! release key and the product's install roots. Every name here is copied from
//! a registry `stado release submit` really parsed on this host; a field the
//! control plane renames makes the journey refuse rather than pass.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::fixture::run;

/// The disk-cleanup policy every target in this document declares. Cleanup is
/// off for a release journey: the case owns a tempdir, not the host's disk.
fn disk_cleanup() -> Value {
    json!({
        "mode": "off",
        "check_interval_seconds": 300,
        "low_free_gb": 1,
        "target_free_gb": 2,
        "max_bytes_per_pass": 53687091200_u64,
        "max_items_per_pass": 50,
        "max_scan_items": 10000,
        "cleaners": {}
    })
}

/// This machine's fully qualified name, which is how the local provider
/// recognises itself in the registry.
fn this_host() -> String {
    String::from_utf8(run(Command::new("hostname").arg("-f")).stdout)
        .unwrap()
        .trim()
        .to_ascii_lowercase()
}

pub fn write(
    home: &Path,
    storage: &Path,
    public_key: &str,
    platform: &str,
    recovery_target: Option<(&str, &str)>,
) {
    let mut document = json!({
        "schema_version": 2,
        "targets": [{
            "name": "ci-runner",
            "kind": "local",
            "ssh": "nobody@127.0.0.1",
            "release_platform": platform,
            "hostnames": [this_host()],
            "disk_cleanup": disk_cleanup(),
            "services": [{
                "kind": "launchd",
                "name": "ci-release-probe",
                "label": "ci-release-probe",
                "path": home.join("Library/LaunchAgents/ci-release-probe.plist"),
                "unit": ""
            }]
        }],
        "service_directory": {
            "authority": {
                "target": "ci-runner",
                "command": env!("CARGO_BIN_EXE_stado")
            },
            "generation": 1,
            "services": {
                "ci-release-probe": {
                    "active_host": "ci-runner",
                    "managed_service": "ci-release-probe",
                    "endpoints": {
                        "ci-runner": {"url": "http://127.0.0.1:1"}
                    },
                    "consumers": {
                        "ci-release": {"capabilities": ["release"]}
                    }
                }
            }
        },
        "release_control": {
            "schema_version": 1,
            "generation": 1,
            "trusted_keys": {"ci-release-key": public_key.trim()},
            "products": {
                "ci-release-probe": {
                    "service": "ci-release-probe",
                    "config_schema": 1,
                    "state_schema": 1,
                    "install_root": home.join(".stado/services/ci-release-probe"),
                    "binary": "bin/ci-release-probe",
                    "launcher": "bin/ci-release-probe",
                    "binary_env": "CI_RELEASE_PROBE_BIN",
                    "port_env": "CI_RELEASE_PROBE_PORT",
                    "runtime_env": "CI_RELEASE_PROBE_RUNTIME",
                    "environment": {},
                    "signing_key_item": "ci-release-signing",
                    "signing_key_id": "ci-release-key",
                    "strategy": {
                        "kind": "replace",
                        "readiness_timeout_seconds": 30,
                        "drain_timeout_seconds": 30,
                        "rollback_window_seconds": 300,
                        "automatic_rollback": false
                    },
                    "targets": {
                        "ci-runner": {
                            "platform": platform,
                            "run_as_user": "ci-release",
                            "home": home,
                            "state_dir": home.join(".stado/release-state"),
                            "runtime_root": home.join(".stado/run"),
                            "logs_root": home.join(".stado/logs"),
                            "readiness_path": "/healthz"
                        }
                    }
                }
            }
        }
    });
    if let Some((name, hostname)) = recovery_target {
        document["targets"]
            .as_array_mut()
            .expect("registry targets are an array")
            .push(json!({
                "name": name,
                "kind": "local",
                "ssh": format!("nobody@{hostname}"),
                "release_platform": platform,
                "hostnames": [hostname],
                "disk_cleanup": disk_cleanup(),
                "slots": 1
            }));
    }
    std::fs::write(
        storage.join("registry.json"),
        serde_json::to_string_pretty(&document).unwrap(),
    )
    .unwrap();
}
