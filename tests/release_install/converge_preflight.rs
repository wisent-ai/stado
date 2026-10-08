//! A Stado install does not recycle the host's serving units onto an image
//! the host's configuration cannot run. One isolated home with a fresh local
//! configuration that declares no `dashboard.request_limits`: the real
//! `stado release converge-local-readers`, the step every Stado install runs
//! after placing its binary, must refuse naming that declaration, and must
//! leave no release-version marker behind (the marker is what makes queue
//! agents recycle themselves onto the new image). Before, it recycled the
//! units and the vault owner's object API crash-looped on the missing
//! declaration.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[test]
fn units_are_not_recycled_onto_an_image_this_host_cannot_serve() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let root = repository
        .join(".build/release-install-converge-preflight")
        .join(uuid::Uuid::new_v4().to_string());
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(root.join("tmp")).unwrap();
    let source_revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&repository)
        .output()
        .unwrap();
    let mut report = json!({
        "source_revision": String::from_utf8_lossy(&source_revision.stdout).trim(),
        "commands": [],
        "outcome": "failed",
    });
    let mut stado = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &home)
            .env("STADO_CONFIG", home.join(".stado/config.json"))
            .env("TMPDIR", root.join("tmp"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        fs::write(
            root.join("report.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        output
    };

    let init = stado(&["config", "init"]);
    assert!(
        init.status.success(),
        "config init: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let refused = stado(&["release", "converge-local-readers", "--name", "stado"]);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "converge recycled units on a host without request limits: {}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert!(
        stderr.contains("no unit is recycled onto it")
            && stderr.contains("dashboard.request_limits"),
        "the refusal does not name the missing declaration: {stderr}"
    );
    assert!(
        !home.join(".stado/bin/stado.release-version").exists(),
        "a refused converge left the marker that recycles queue agents"
    );

    let saved: Value =
        serde_json::from_slice(&fs::read(root.join("report.json")).unwrap()).unwrap();
    let mut saved = saved;
    saved["outcome"] = json!("passed");
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&saved).unwrap(),
    )
    .unwrap();
    eprintln!("report: {}", root.join("report.json").display());
}
