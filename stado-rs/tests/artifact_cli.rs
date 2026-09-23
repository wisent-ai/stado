//! End-to-end CLI tests for the `artifact` command group and submit-side
//! `--input-artifact` resolution, against the local storage backend.
//!
//! Same harness as `tests/cli_local.rs`: the built `stado` binary
//! (`CARGO_BIN_EXE_stado`) with WC_STORAGE_BACKEND=local +
//! WC_LOCAL_STORAGE_PATH=<TempDir>.

use std::path::Path;
use std::process::{Command, Output};

use stado::models::Job;

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("HF_TOKEN");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Write a minimal valid manifest JSON (Python field names) and return
/// its path.
fn write_manifest(dir: &Path, file: &str, version: &str, description: &str) -> std::path::PathBuf {
    let path = dir.join(file);
    let body = format!(
        r#"{{
            "ref": "dataset/wisent/cli-demo@{version}",
            "title": "CLI demo",
            "description": "{description}",
            "producer": {{"run_id": "run-cli", "job_ids": ["j1"]}},
            "locations": [
                {{"role": "primary", "uri": "gs://stado/artifacts/cli-demo/{version}", "storage": "gcs"}}
            ],
            "labels": {{"tier": "gold"}}
        }}"#
    );
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn artifact_publish_list_show_resolve_alias_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let storage = storage.as_path();
    let manifest_v1 = write_manifest(dir.path(), "manifest-v1.json", "v1", "first");
    let ref_v1 = "dataset/wisent/cli-demo@v1";

    // Publish (default --verify: the generic adapter passes offline).
    let out = stado(
        storage,
        &["artifact", "publish", manifest_v1.to_str().unwrap()],
    );
    assert!(out.status.success(), "publish failed: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), ref_v1);

    // Re-publish with changed content conflicts (immutable versions).
    let manifest_v1_changed = write_manifest(dir.path(), "manifest-v1b.json", "v1", "CHANGED");
    let out = stado(
        storage,
        &["artifact", "publish", manifest_v1_changed.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    assert!(
        stderr(&out).contains(
            "Error: ARTIFACT_VERSION_CONFLICT: immutable artifact version already exists with different content"
        ),
        "stderr: {}",
        stderr(&out)
    );

    // A missing manifest file is a click-style usage error (exit 2).
    let out = stado(storage, &["artifact", "publish", "no-such-manifest.json"]);
    assert_eq!(out.status.code(), Some(2), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("does not exist"),
        "stderr: {}",
        stderr(&out)
    );

    // List: table header + the published row.
    let out = stado(storage, &["artifact", "list"]);
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("REF"), "{text}");
    assert!(text.contains(ref_v1), "{text}");
    assert!(text.contains("passed"), "{text}");

    // List filters: label match / mismatch, JSON shape.
    let out = stado(storage, &["artifact", "list", "--label", "tier=gold"]);
    assert!(stdout(&out).contains(ref_v1), "{}", stdout(&out));
    let out = stado(storage, &["artifact", "list", "--label", "tier=silver"]);
    assert_eq!(stdout(&out).trim(), "(no artifacts found)");
    let out = stado(
        storage,
        &["artifact", "list", "--type", "dataset", "--json"],
    );
    assert!(out.status.success());
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 1);
    assert_eq!(parsed[0]["ref"], ref_v1);
    // 64-hex manifest digest stamped at publish.
    let digest = parsed[0]["verification"]["manifest_sha256"]
        .as_str()
        .unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    let out = stado(storage, &["artifact", "list", "--type", "model", "--json"]);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&out))
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // Show: text layout + the summary-free body.
    let out = stado(storage, &["artifact", "show", ref_v1]);
    assert!(out.status.success(), "show failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Artifact:     dataset/wisent/cli-demo"),
        "{text}"
    );
    assert!(text.contains("Version:      v1"), "{text}");
    assert!(text.contains("Title:        CLI demo"), "{text}");
    assert!(text.contains("Verification: passed"), "{text}");
    assert!(
        text.contains("Location:     [primary] gs://stado/artifacts/cli-demo/v1"),
        "{text}"
    );
    assert!(text.contains("Run:          run-cli"), "{text}");

    // Resolve a version ref: identity. JSON uses Python's default
    // separators (", " / ": "), not the compact form.
    let out = stado(storage, &["artifact", "resolve", ref_v1]);
    assert_eq!(stdout(&out).trim(), ref_v1);
    let out = stado(storage, &["artifact", "resolve", ref_v1, "--json"]);
    assert_eq!(
        stdout(&out).trim(),
        format!(r#"{{"requested_ref": "{ref_v1}", "resolved_ref": "{ref_v1}"}}"#)
    );

    // Alias: set, resolve through it, then retarget with the CAS
    // precondition.
    let out = stado(storage, &["artifact", "alias", "set", ref_v1, "latest"]);
    assert!(out.status.success(), "alias set failed: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        format!("dataset/wisent/cli-demo@latest -> {ref_v1}")
    );
    let out = stado(
        storage,
        &["artifact", "resolve", "dataset/wisent/cli-demo@latest"],
    );
    assert_eq!(stdout(&out).trim(), ref_v1);
    // Show through the alias resolves to the immutable manifest.
    let out = stado(
        storage,
        &["artifact", "show", "dataset/wisent/cli-demo@latest"],
    );
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("Aliases:      latest"),
        "{}",
        stdout(&out)
    );

    // Publish v2 and retarget the alias.
    let manifest_v2 = write_manifest(dir.path(), "manifest-v2.json", "v2", "second");
    let out = stado(
        storage,
        &[
            "artifact",
            "publish",
            manifest_v2.to_str().unwrap(),
            "--no-verify",
        ],
    );
    assert!(out.status.success(), "publish v2 failed: {}", stderr(&out));
    // No precondition → conflict.
    let out = stado(
        storage,
        &[
            "artifact",
            "alias",
            "set",
            "dataset/wisent/cli-demo@v2",
            "latest",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "ARTIFACT_ALIAS_CONFLICT: alias dataset/wisent/cli-demo@latest currently targets v1; pass expected_previous"
        ),
        "stderr: {}",
        stderr(&out)
    );
    // Wrong precondition → conflict.
    let out = stado(
        storage,
        &[
            "artifact",
            "alias",
            "set",
            "dataset/wisent/cli-demo@v2",
            "latest",
            "--expected-previous",
            "v9",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("targets v1, not expected v9"),
        "stderr: {}",
        stderr(&out)
    );
    // Correct precondition commits.
    let out = stado(
        storage,
        &[
            "artifact",
            "alias",
            "set",
            "dataset/wisent/cli-demo@v2",
            "latest",
            "--expected-previous",
            "v1",
            "--json",
        ],
    );
    assert!(out.status.success(), "alias set failed: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        r#"{"alias_ref": "dataset/wisent/cli-demo@latest", "resolved_ref": "dataset/wisent/cli-demo@v2"}"#
    );
    let out = stado(
        storage,
        &["artifact", "resolve", "dataset/wisent/cli-demo@latest"],
    );
    assert_eq!(stdout(&out).trim(), "dataset/wisent/cli-demo@v2");

    // List shows the alias on the v2 row.
    let out = stado(storage, &["artifact", "list", "--name", "cli-demo"]);
    let text = stdout(&out);
    assert!(text.contains("latest"), "{text}");

    // Verify (generic adapter) passes; lineage walks producer + deps.
    let out = stado(storage, &["artifact", "verify", ref_v1]);
    assert!(out.status.success(), "verify failed: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "PASSED (generic-v1)");
    let out = stado(
        storage,
        &["artifact", "lineage", "dataset/wisent/cli-demo@latest"],
    );
    assert!(out.status.success(), "lineage failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Artifact: dataset/wisent/cli-demo@v2"),
        "{text}"
    );
    assert!(text.contains("Run:      run-cli"), "{text}");
    assert!(text.contains("Jobs:     j1"), "{text}");
    assert!(text.contains("Source:   -@-"), "{text}");
    assert!(text.contains("Inputs:   -"), "{text}");

    // Unknown refs exit 1 with the machine-readable code.
    let out = stado(storage, &["artifact", "show", "dataset/wisent/ghost@v9"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "Error: ARTIFACT_NOT_FOUND: artifact or alias not found: dataset/wisent/ghost@v9"
        ),
        "stderr: {}",
        stderr(&out)
    );
    // Malformed refs are rejected as ARTIFACT_INVALID_REF.
    let out = stado(storage, &["artifact", "resolve", "not-a-ref"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("ARTIFACT_INVALID_REF"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn submit_input_artifact_resolves_into_job_blob() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("storage");
    let storage = storage.as_path();
    let manifest_v1 = write_manifest(dir.path(), "manifest.json", "v1", "first");
    let out = stado(
        storage,
        &["artifact", "publish", manifest_v1.to_str().unwrap()],
    );
    assert!(out.status.success(), "publish failed: {}", stderr(&out));
    let out = stado(
        storage,
        &[
            "artifact",
            "alias",
            "set",
            "dataset/wisent/cli-demo@v1",
            "latest",
        ],
    );
    assert!(out.status.success());

    // Submit through the ALIAS: the job records the requested ref and the
    // resolved immutable ref + primary URI + manifest digest.
    let out = stado(
        storage,
        &[
            "submit",
            "echo artifact-consumer",
            "--input-artifact",
            "DATA=dataset/wisent/cli-demo@latest",
        ],
    );
    assert!(out.status.success(), "submit failed: {}", stderr(&out));
    let job_id = stdout(&out)
        .lines()
        .find_map(|line| line.strip_prefix("Job ID: "))
        .expect("submit echoed a Job ID")
        .trim()
        .to_string();
    let raw =
        std::fs::read_to_string(storage.join("queue").join(format!("{job_id}.json"))).unwrap();
    let job = Job::from_json(&raw).unwrap();
    assert_eq!(
        job.input_artifacts.get("DATA").and_then(|v| v.as_str()),
        Some("dataset/wisent/cli-demo@latest")
    );
    let resolved = &job.resolved_input_artifacts["DATA"];
    assert_eq!(resolved["ref"], "dataset/wisent/cli-demo@v1");
    assert_eq!(resolved["uri"], "gs://stado/artifacts/cli-demo/v1");
    let digest = resolved["manifest_sha256"].as_str().unwrap();
    assert_eq!(digest.len(), 64);

    // Unknown artifact refs fail the submit before anything is written.
    let out = stado(
        storage,
        &[
            "submit",
            "echo nope",
            "--input-artifact",
            "DATA=dataset/wisent/ghost@v9",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("ARTIFACT_NOT_FOUND"),
        "stderr: {}",
        stderr(&out)
    );
    // Unsafe input names and duplicate names are usage errors (exit 1,
    // click parity).
    let out = stado(
        storage,
        &[
            "submit",
            "echo nope",
            "--input-artifact",
            "1bad=dataset/wisent/cli-demo@v1",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("artifact input name is unsafe"),
        "stderr: {}",
        stderr(&out)
    );
    let out = stado(
        storage,
        &[
            "submit",
            "echo nope",
            "--input-artifact",
            "missing-equals-sign",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("--input-artifact must be NAME=REF"),
        "stderr: {}",
        stderr(&out)
    );
}
