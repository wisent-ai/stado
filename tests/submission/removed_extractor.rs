//! A job that would run a removed wisent-tools activation extractor is
//! refused by `stado submit` of an isolated real deployment — its own HOME,
//! config and local store under the repository's ignored `.build/` — with
//! the reason and what replaced it, and nothing reaches its queue; an
//! ordinary command submitted beside it is queued as before. Every command,
//! its exit status and output go to the run's report.json.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn stado(root: &Path, args: &[&str], report: &mut Vec<Value>) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_stado"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root)
        .env("STADO_CONFIG", root.join(".stado").join("config.json"))
        .env("TMPDIR", root.join("tmp"))
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap();
    report.push(json!({
        "args": args,
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }));
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "journey": "removed-extractor",
            "source_revision": std::env::var("STADO_SOURCE_REVISION").ok(),
            "commands": report,
        }))
        .unwrap(),
    )
    .unwrap();
    output
}

/// Every queued job document in the deployment's local store.
fn queued(root: &Path) -> Vec<Value> {
    let queue = root.join(".stado").join("local-storage").join("queue");
    let Ok(entries) = fs::read_dir(&queue) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

#[test]
fn a_removed_activation_extractor_is_refused_at_submit() {
    // A Unix socket path is at most 104 bytes on macOS, so HOME is the run
    // directory itself under a short name.
    let root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".build/z")
        .join(&uuid::Uuid::new_v4().simple().to_string()[..6]);
    fs::create_dir_all(root.join("tmp")).unwrap();
    let mut report = Vec::new();
    assert!(stado(&root, &["config", "init"], &mut report)
        .status
        .success());

    let removed = "python3 -m wisent.scripts.activations.raw.extract_and_upload --task aexams";
    let refused = stado(
        &root,
        &["submit", "--run-id", "removed-extractor", removed],
        &mut report,
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "the submit was accepted: {stderr}"
    );
    assert!(
        stderr.contains("wisent-tools no longer ships its Python activation extractors")
            && stderr.contains("ster extract"),
        "{stderr}"
    );
    assert!(queued(&root).is_empty(), "a refused job reached the queue");

    let ordinary = stado(
        &root,
        &["submit", "--run-id", "ordinary-job", "true"],
        &mut report,
    );
    assert!(
        ordinary.status.success(),
        "{}",
        String::from_utf8_lossy(&ordinary.stderr)
    );
    let jobs = queued(&root);
    assert!(!jobs.is_empty(), "the ordinary job was not queued");
    assert!(jobs.iter().all(|job| job["command"] == "true"), "{jobs:?}");
}
