use super::*;

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn committed_submission_preserves_active_work_and_installs_the_selected_source() {
    let platform = release_platform();
    let runs = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&runs).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-commit-")
        .tempdir_in(&runs)
        .unwrap();
    let storage = home.path().join("store");
    fs::create_dir_all(&storage).unwrap();
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for directory in [".cargo", ".rustup"] {
        std::os::unix::fs::symlink(operator_home.join(directory), home.path().join(directory))
            .unwrap();
    }
    let source = fixture_source(home.path(), platform, "");
    let selected = String::from_utf8(
        run(Command::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"]))
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let tree = String::from_utf8(
        run(Command::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD^{tree}"]))
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let later_source = b"fn main() { panic!(\"the selected commit was not built\"); }\n";
    fs::write(source.join("src/main.rs"), later_source).unwrap();
    git(&source, &["add", "src/main.rs"]);
    git(&source, &["commit", "-qm", "later source, not the release"]);
    let head = run(Command::new("git")
        .current_dir(&source)
        .args(["rev-parse", "HEAD"]))
    .stdout;
    let working_version =
        b"[package]\nname = \"unfinished\"\nversion = \"9.0.0\"\nedition = \"2021\"\n";
    fs::write(source.join("Cargo.toml"), working_version).unwrap();
    git(&source, &["add", "Cargo.toml"]);
    let working_manifest = b"unfinished manifest\n";
    fs::write(source.join(".wisent-release.json"), working_manifest).unwrap();
    let untracked = b"work not part of the release\n";
    fs::write(source.join("untracked.txt"), untracked).unwrap();
    let index = fs::read(source.join(".git/index")).unwrap();

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
    let arguments = [
        "release",
        "submit",
        "--source",
        source.to_str().unwrap(),
        "--version",
        "1.0.0",
        "--json",
    ];
    let mut implicit = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut implicit, home.path(), &storage, &vault);
    let refused = implicit.args(arguments).output().unwrap();
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr)
        .contains("release source must be a clean committed Git tree"));
    let mut refusals = vec![
        json!({"commit": Value::Null, "exit_code": refused.status.code(),
        "stderr": String::from_utf8_lossy(&refused.stderr)}),
    ];
    for (commit, sentence) in [
        (
            "HEAD",
            "--commit must be 40 lowercase hexadecimal characters",
        ),
        (tree.as_str(), "--commit must name a Git commit object"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        release_env(&mut command, home.path(), &storage, &vault);
        let refused = command
            .args(arguments)
            .args(["--commit", commit])
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&refused.stderr).contains(sentence));
        refusals.push(json!({"commit": commit, "exit_code": refused.status.code(),
            "stderr": String::from_utf8_lossy(&refused.stderr)}));
    }
    assert!(
        !storage.join("runs/release-pipeline").exists(),
        "a refused source created a release run"
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let mut agent = Running(
        command
            .args(["agent", "--target", "ci-runner"])
            .stdout(File::create(home.path().join("agent.out")).unwrap())
            .stderr(File::create(home.path().join("agent.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut agent.0);
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let mut submit = Running(
        command
            .args(arguments)
            .args(["--commit", &selected])
            .stdout(File::create(home.path().join("submit.out")).unwrap())
            .stderr(File::create(home.path().join("submit.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    let status = wait_for_submit(&mut submit.0, &mut agent.0, home.path(), &storage);
    let stdout = fs::read(home.path().join("submit.out")).unwrap();
    let stderr = fs::read(home.path().join("submit.err")).unwrap();
    assert!(
        status.success(),
        "committed release failed: {}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    let release: Value = serde_json::from_slice(&stdout).unwrap();
    let id = release["run_id"].as_str().unwrap();
    let stored: Value = serde_json::from_slice(
        &fs::read(storage.join(format!("runs/release-pipeline/{id}/run.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["state"], "completed");
    assert_eq!(stored["source_commit"], selected);
    assert_eq!(stored["version"], "1.0.0");
    assert_eq!(
        stored["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert_eq!(
        run(&mut Command::new(installed)).stdout,
        b"ci-release-probe 1.0.0\n"
    );
    assert_eq!(
        fs::read(source.join("Cargo.toml")).unwrap(),
        working_version
    );
    assert_eq!(
        fs::read(source.join(".wisent-release.json")).unwrap(),
        working_manifest
    );
    assert_eq!(fs::read(source.join("src/main.rs")).unwrap(), later_source);
    assert_eq!(fs::read(source.join("untracked.txt")).unwrap(), untracked);
    assert_eq!(fs::read(source.join(".git/index")).unwrap(), index);
    assert_eq!(
        run(Command::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"]))
        .stdout,
        head
    );
    let mut repeated = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut repeated, home.path(), &storage, &vault);
    let repeated: Value =
        serde_json::from_slice(&run(repeated.args(arguments).args(["--commit", &selected])).stdout)
            .unwrap();
    assert_eq!(repeated["platforms"], stored["platforms"]);
    assert_eq!(repeated["deliveries"], stored["deliveries"]);
    let binary = run(Command::new(env!("CARGO_BIN_EXE_stado")).arg("--version"));
    let evidence = runs.join(format!("commit-{id}.json"));
    fs::write(
        &evidence,
        serde_json::to_vec_pretty(&json!({
            "binary": String::from_utf8_lossy(&binary.stdout), "selected_commit": selected,
            "working_head": String::from_utf8_lossy(&head), "refusals": refusals,
            "run": stored, "repeated": repeated, "exit_code": status.code(),
            "stdout": String::from_utf8_lossy(&stdout), "stderr": String::from_utf8_lossy(&stderr),
            "installed_output": "ci-release-probe 1.0.0", "working_tree_preserved": true
        }))
        .unwrap(),
    )
    .unwrap();
    println!("committed source release evidence: {}", evidence.display());
}
