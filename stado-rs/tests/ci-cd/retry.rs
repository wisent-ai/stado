use super::*;
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_cancelled_release_build_is_retried_under_a_new_job() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-retry-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");

    let private = home.path().join("release-private");
    let public = home.path().join("release-public");
    let worker_bin = home.path().join(".stado/bin/stado");
    fs::create_dir_all(worker_bin.parent().unwrap()).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_stado"), &worker_bin).unwrap();
    fs::set_permissions(&worker_bin, fs::Permissions::from_mode(0o700)).unwrap();
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
    let public_key = fs::read_to_string(&public).unwrap();
    let vault = SkarbiecFixture::start_release(home.path(), &private);
    registry(home.path(), &storage, &public_key, platform, None);

    let agent_out = File::create(home.path().join("agent.out")).unwrap();
    let agent_err = File::create(home.path().join("agent.err")).unwrap();
    let mut initial_agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut initial_agent_command, home.path(), &storage, &vault);
    let mut initial_agent = initial_agent_command
        .args(["agent", "--target", "ci-runner"])
        .stdout(Stdio::from(agent_out))
        .stderr(Stdio::from(agent_err))
        .spawn()
        .unwrap();
    wait_for_claimable_capacity(&storage, home.path(), &mut initial_agent);
    initial_agent.kill().unwrap();
    initial_agent.wait().unwrap();

    let submit_out = File::create(home.path().join("submit-first.out")).unwrap();
    let submit_err = File::create(home.path().join("submit-first.err")).unwrap();
    let mut first_submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut first_submit_command, home.path(), &storage, &vault);
    let mut first_submit = first_submit_command
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--channel",
            "candidate",
            "--json",
        ])
        .stdout(Stdio::from(submit_out))
        .stderr(Stdio::from(submit_err))
        .spawn()
        .unwrap();
    let first_job = wait_for_queued_release_build(&mut first_submit, home.path(), &storage);
    let first_job_id = first_job["job_id"].as_str().unwrap().to_string();
    let mut cancel = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut cancel, home.path(), &storage, &vault);
    run(cancel.args(["cancel", &first_job_id]));

    let deadline = Instant::now() + Duration::from_secs(30);
    let first_status = loop {
        if let Some(status) = first_submit.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "cancelled release submit did not exit\nstore:{}",
            store_snapshot(&storage)
        );
        thread::sleep(Duration::from_millis(100));
    };
    assert!(!first_status.success(), "cancelled release submit passed");
    let first_error = fs::read_to_string(home.path().join("submit-first.err")).unwrap();
    assert!(
        first_error.contains(&format!("release job {first_job_id} ({platform} on "))
            && first_error.contains("failed: cancelled"),
        "cancelled release reported the wrong failure:\n{first_error}"
    );

    let agent_out = File::create(home.path().join("agent.out")).unwrap();
    let agent_err = File::create(home.path().join("agent.err")).unwrap();
    let mut agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut agent_command, home.path(), &storage, &vault);
    let mut agent = agent_command
        .args(["agent", "--target", "ci-runner"])
        .stdout(Stdio::from(agent_out))
        .stderr(Stdio::from(agent_err))
        .spawn()
        .unwrap();
    wait_for_claimable_capacity(&storage, home.path(), &mut agent);

    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut retry_submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut retry_submit_command, home.path(), &storage, &vault);
    let mut retry_submit = retry_submit_command
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--channel",
            "candidate",
            "--json",
        ])
        .stdout(Stdio::from(submit_out))
        .stderr(Stdio::from(submit_err))
        .spawn()
        .unwrap();
    let status = wait_for_submit(&mut retry_submit, &mut agent, home.path(), &storage);
    let result = Output {
        status,
        stdout: fs::read(home.path().join("submit.out")).unwrap(),
        stderr: fs::read(home.path().join("submit.err")).unwrap(),
    };
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(
        result.status.success(),
        "retried release submit failed:\nstdout:\n{}\nstderr:\n{}\nagent stdout:\n{}\nagent stderr:\n{}\nstore:{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
        fs::read_to_string(home.path().join("agent.out")).unwrap_or_default(),
        fs::read_to_string(home.path().join("agent.err")).unwrap_or_default(),
        store_snapshot(&storage)
    );
    let release: Value = serde_json::from_slice(&result.stdout).unwrap();
    let retry_job_id = release["platforms"][platform]["job_id"].as_str().unwrap();
    assert_ne!(retry_job_id, first_job_id);
    assert_eq!(release["state"], "completed");
    assert_eq!(release["platforms"][platform]["state"], "published");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    assert!(
        storage
            .join("cancelled")
            .join(format!("{first_job_id}.json"))
            .is_file(),
        "first job did not remain durably cancelled"
    );

    let installed = home.path().join(".stado/bin/ci-release-probe");
    let output = run(&mut Command::new(&installed));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ci-release-probe 1.0.0"
    );
    println!(
        "verified cancelled release retry platform={platform}; first_job={first_job_id}; retry_job={retry_job_id}; installed=ci-release-probe 1.0.0"
    );
}
