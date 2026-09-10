use super::*;

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn failed_delivery_resumes_original_source_after_checkout_changes() {
    let platform = release_platform();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-resume-")
        .tempdir_in(&root)
        .unwrap();
    let storage = home.path().join("store");
    fs::create_dir_all(&storage).unwrap();
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for directory in [".cargo", ".rustup"] {
        std::os::unix::fs::symlink(operator_home.join(directory), home.path().join(directory))
            .unwrap();
    }
    let source = fixture_source(home.path(), platform, "");
    let private = home.path().join("release-private");
    let public = home.path().join("release-public");
    let worker = home.path().join(".stado/bin/stado");
    fs::create_dir_all(worker.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_stado"), &worker).unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700)).unwrap();
    run(Command::new(env!("CARGO_BIN_EXE_stado")).args([
        "release",
        "keygen",
        "--private-key",
        private.to_str().unwrap(),
        "--public-key",
        public.to_str().unwrap(),
        "--key-id",
        "ci-release-key",
    ]));
    let vault = SkarbiecFixture::start_release(home.path(), &private);
    registry(
        home.path(),
        &storage,
        &fs::read_to_string(public).unwrap(),
        platform,
        None,
    );
    let installed = home.path().join(".stado/bin/ci-release-probe");
    fs::create_dir(&installed).unwrap();
    let mut agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut agent_command, home.path(), &storage, &vault);
    let mut agent = Running(
        agent_command
            .args(["agent", "--target", "ci-runner"])
            .stdout(File::create(home.path().join("agent.out")).unwrap())
            .stderr(File::create(home.path().join("agent.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut agent.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let mut submission = Running(
        command
            .args([
                "release",
                "submit",
                "--source",
                source.to_str().unwrap(),
                "--version",
                "1.0.0",
                "--json",
            ])
            .stdout(File::create(home.path().join("submit.out")).unwrap())
            .stderr(File::create(home.path().join("submit.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    let status = wait_for_submit(&mut submission.0, &mut agent.0, home.path(), &storage);
    assert!(
        !status.success(),
        "an occupied installation destination passed"
    );
    assert!(installed.is_dir(), "the existing destination was replaced");
    let mut inventory = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut inventory, home.path(), &storage, &vault);
    let before: Value = serde_json::from_slice(
        &run(inventory.args(["release", "status", "ci-release-probe", "--json"])).stdout,
    )
    .unwrap();
    let before = &before["runs"][0];
    assert_eq!(before["state"], "failed");
    assert_eq!(before["platforms"][platform]["state"], "published");
    assert_eq!(
        before["deliveries"]["install-on-builder"]["state"],
        "failed"
    );
    let run_id = before["run_id"].as_str().unwrap();

    // Neither a later commit nor an unreadable current manifest may affect resume.
    fs::write(
        source.join("src/main.rs"),
        "fn main() { panic!(\"wrong source was rebuilt\"); }\n",
    )
    .unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-qm", "later source"]);
    fs::write(source.join(".wisent-release.json"), "not a manifest").unwrap();
    fs::remove_dir(&installed).unwrap();
    let mut malformed = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut malformed, home.path(), &storage, &vault);
    let refused = malformed
        .args(["release", "resume", "../invalid", "--json"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("run ID must be 32 lowercase hexadecimal characters"));
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let mut resumed = Running(
        command
            .current_dir(&source)
            .args(["release", "resume", run_id, "--json"])
            .stdout(File::create(home.path().join("submit.out")).unwrap())
            .stderr(File::create(home.path().join("submit.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    let status = wait_for_submit(&mut resumed.0, &mut agent.0, home.path(), &storage);
    let stdout = fs::read(home.path().join("submit.out")).unwrap();
    let stderr = fs::read(home.path().join("submit.err")).unwrap();
    assert!(
        status.success(),
        "resume failed: {}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    let after: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(after["state"], "completed");
    assert_eq!(after["run_id"], before["run_id"]);
    assert_eq!(after["source_commit"], before["source_commit"]);
    assert_eq!(after["source_sha256"], before["source_sha256"]);
    assert_eq!(
        after["platforms"], before["platforms"],
        "published builds must not be rebuilt"
    );
    assert_ne!(
        after["deliveries"]["install-on-builder"]["job_id"],
        before["deliveries"]["install-on-builder"]["job_id"]
    );
    assert_eq!(after["deliveries"]["install-on-builder"]["state"], "passed");
    assert_eq!(
        run(&mut Command::new(&installed)).stdout,
        b"ci-release-probe 1.0.0\n"
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    command.env(
        "WC_RELEASE_SIGNING_SKARBIEC_TOKEN_FILE",
        home.path().join("absent-signing-grant"),
    );
    let repeated: Value = serde_json::from_slice(
        &run(command
            .current_dir(&source)
            .args(["release", "resume", run_id, "--json"]))
        .stdout,
    )
    .unwrap();
    assert_eq!(repeated["deliveries"], after["deliveries"]);
    let stored: Value = serde_json::from_slice(
        &fs::read(storage.join(format!("runs/release-pipeline/{run_id}/run.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["state"], "completed");
    assert_eq!(stored["deliveries"], after["deliveries"]);
    let evidence = root.join(format!("resume-{run_id}.json"));
    let binary = run(Command::new(env!("CARGO_BIN_EXE_stado")).arg("--version"));
    fs::write(
        &evidence,
        serde_json::to_vec_pretty(&json!({
            "binary": String::from_utf8_lossy(&binary.stdout), "before": before,
            "after": after, "persisted": stored, "command": ["release", "resume", run_id, "--json"],
            "exit_code": status.code(), "stdout": String::from_utf8_lossy(&stdout),
            "stderr": String::from_utf8_lossy(&stderr), "installed_output": "ci-release-probe 1.0.0"
        }))
        .unwrap(),
    )
    .unwrap();
    println!("real resume evidence: {}", evidence.display());
}
