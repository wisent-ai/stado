//! Byte-parity tests against Python-generated `Job.to_json()` fixtures.
//! Fixtures are produced by the Python package (see repo history / phase-0
//! plan) and must round-trip through the Rust model byte-identically.

use sha2::{Digest, Sha256};
use stado::models::Job;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const RELEASE_BINARIES: &[&str] = &[
    "stado",
    "wc",
    "stado-coverage",
    "stado-fix",
    "stado-watchdog",
    "stado-mcp",
];

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture readable")
}

#[test]
fn python_minimal_job_round_trips_byte_identically() {
    let original = fixture("job_minimal.json");
    let job = Job::from_json(&original).unwrap();
    assert_eq!(job.to_json(), original);
}

#[test]
fn python_full_job_round_trips_byte_identically() {
    let original = fixture("job_full.json");
    let job = Job::from_json(&original).unwrap();
    assert_eq!(job.to_json(), original);
}

#[test]
fn legacy_partial_record_gets_python_defaults() {
    let job = Job::from_json(&fixture("job_legacy_partial.json")).unwrap();
    assert_eq!(job.job_id, "legacy");
    assert_eq!(job.state, "completed");
    // Python dataclass defaults for missing keys:
    assert_eq!(job.provider, "gcp");
    assert_eq!(job.max_restarts, 20);
    assert_eq!(job.boot_disk_gb, 500);
    assert_eq!(job.repo_extras, "train");
    assert_eq!(job.executor, "stado-agent");
    assert_eq!(job.yield_grace_seconds, 120);
    assert!(!job.created_at.is_empty(), "existing created_at preserved");
    assert_eq!(job.created_at, "2025-05-01T00:00:00+00:00");
}

#[test]
fn runtime_version_is_the_cargo_release_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .arg("--version")
        .output()
        .expect("stado version command runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        format!("stado {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn release_manifest_covers_every_binary_and_schema_contract() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("stado-rs lives inside the repository");
    let release_dir = tempfile::tempdir().unwrap();
    for name in RELEASE_BINARIES {
        std::fs::write(release_dir.path().join(name), format!("{name}-artifact")).unwrap();
    }

    let output = Command::new("/bin/sh")
        .arg(repository.join("scripts/release_manifest.sh"))
        .env("PATH", "/usr/bin:/bin:/opt/homebrew/bin")
        .env("STADO_RELEASE_DIR", release_dir.path())
        .env("STADO_RELEASE_VERSION", env!("CARGO_PKG_VERSION"))
        .env("STADO_RELEASE_CHANNEL", "candidate")
        .env("STADO_RELEASE_PLATFORM", "test-platform")
        .env("STADO_RELEASE_SOURCE_COMMIT", "abcdef")
        .env(
            "STADO_RELEASE_SOURCE_REPOSITORY",
            "https://example.invalid/repository",
        )
        .env("STADO_RELEASE_BUILT_AT", "test-built-at")
        .env(
            "STADO_MACHINE_SCHEMA_VERSION",
            stado::machine::SCHEMA_VERSION.to_string(),
        )
        .env(
            "STADO_CONFIG_SCHEMA_VERSION",
            stado::config_file::SCHEMA_VERSION.to_string(),
        )
        .env(
            "STADO_STORAGE_SCHEMA_VERSION",
            stado::queue::STORAGE_LAYOUT_VERSION.to_string(),
        )
        .env("STADO_LICENSE_FILE", repository.join("LICENSE"))
        .env(
            "STADO_RELEASE_STABLE_INTEGRATIONS",
            "local-compute,local-filesystem-storage,skarbiec",
        )
        .env(
            "STADO_RELEASE_PREVIEW_INTEGRATIONS",
            "gcs-storage,s3-storage,azure-blob-storage,gcp-compute,aws-ec2,azure-vm,box,vast",
        )
        .output()
        .expect("release manifest generator runs");
    assert!(
        output.status.success(),
        "manifest generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest_path = release_dir.path().join("release-manifest.json");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        manifest_path.to_string_lossy()
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["contract"], "stado-release-manifest");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["channel"], "candidate");
    assert_eq!(manifest["platform"], "test-platform");
    assert_eq!(
        manifest["stable_integrations"],
        serde_json::json!([
            "local-compute",
            "local-filesystem-storage",
            "skarbiec"
        ])
    );
    assert_eq!(
        manifest["preview_integrations"],
        serde_json::json!([
            "gcs-storage",
            "s3-storage",
            "azure-blob-storage",
            "gcp-compute",
            "aws-ec2",
            "azure-vm",
            "box",
            "vast"
        ])
    );
    assert_eq!(
        manifest["schema_versions"]["machine_api"],
        stado::machine::SCHEMA_VERSION
    );
    assert_eq!(
        manifest["schema_versions"]["configuration"],
        stado::config_file::SCHEMA_VERSION
    );
    assert_eq!(
        manifest["schema_versions"]["storage_layout"],
        stado::queue::STORAGE_LAYOUT_VERSION
    );

    let artifacts = manifest["artifacts"].as_array().unwrap();
    let names: BTreeSet<&str> = artifacts
        .iter()
        .map(|artifact| artifact["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, RELEASE_BINARIES.iter().copied().collect());
    for artifact in artifacts {
        let name = artifact["name"].as_str().unwrap();
        let bytes = std::fs::read(release_dir.path().join(name)).unwrap();
        assert_eq!(artifact["size_bytes"], bytes.len());
        assert_eq!(artifact["sha256"], hex::encode(Sha256::digest(&bytes)));
    }
}
