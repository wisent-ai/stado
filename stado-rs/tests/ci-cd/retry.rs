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
    let mut initial_agent = Running(
        initial_agent_command
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(agent_out))
            .stderr(Stdio::from(agent_err))
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut initial_agent.0);
    drop(initial_agent);

    let submit_out = File::create(home.path().join("submit-first.out")).unwrap();
    let submit_err = File::create(home.path().join("submit-first.err")).unwrap();
    let mut first_submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut first_submit_command, home.path(), &storage, &vault);
    let mut first_submit = Running(
        first_submit_command
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
            .unwrap(),
    );
    let first_job = wait_for_queued_release_build(&mut first_submit.0, home.path(), &storage);
    let first_job_id = first_job["job_id"].as_str().unwrap().to_string();
    let mut cancel = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut cancel, home.path(), &storage, &vault);
    run(cancel.args(["cancel", &first_job_id]));

    let deadline = Instant::now() + Duration::from_secs(30);
    let first_status = loop {
        if let Some(status) = first_submit.0.try_wait().unwrap() {
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
    let mut agent = Running(
        agent_command
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(agent_out))
            .stderr(Stdio::from(agent_err))
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut agent.0);

    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut retry_submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut retry_submit_command, home.path(), &storage, &vault);
    let mut retry_submit = Running(
        retry_submit_command
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
            .unwrap(),
    );
    let status = wait_for_submit(&mut retry_submit.0, &mut agent.0, home.path(), &storage);
    let result = Output {
        status,
        stdout: fs::read(home.path().join("submit.out")).unwrap(),
        stderr: fs::read(home.path().join("submit.err")).unwrap(),
    };
    drop(agent);
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

/// A build the host has no room for is refused before its first gate.
///
/// On 2026-09-10 the stado 0.20.3 darwin build compiled 616 crates on
/// charless-mac-mini and died with `No space left on device (os error 28)`
/// while rustc wrote metadata: twenty minutes spent, and the requirement
/// readable only as a linker error inside a 30 KB log.
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_build_with_no_room_is_refused_before_its_first_gate() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-no-room-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");

    // The one difference from every other journey here: this product declares
    // more free space than any volume has.
    let manifest_path = source.join(".wisent-release.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["platforms"][platform]["min_free_gb"] = json!(99_999_999_u64);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    run(Command::new("git").current_dir(&source).args([
        "commit",
        "-qam",
        "declare an unreachable free-space requirement",
    ]));

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
    let mut agent_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut agent_command, home.path(), &storage, &vault);
    let mut agent = Running(
        agent_command
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(agent_out))
            .stderr(Stdio::from(agent_err))
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(&storage, home.path(), &mut agent.0);

    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit_command, home.path(), &storage, &vault);
    let mut submit = Running(
        submit_command
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
            .unwrap(),
    );
    let status = wait_for_submit(&mut submit.0, &mut agent.0, home.path(), &storage);
    drop(agent);
    let reported = format!(
        "{}{}",
        fs::read_to_string(home.path().join("submit.out")).unwrap_or_default(),
        fs::read_to_string(home.path().join("submit.err")).unwrap_or_default()
    );
    assert!(
        !status.success(),
        "a build with no room reported success: {reported}"
    );
    assert!(
        reported.contains("this build needs 99999999 GiB free on"),
        "the refusal did not name the declared requirement: {reported}"
    );
    assert!(
        !reported.contains("Compiling ci-release-probe"),
        "the build ran a gate before the room was checked: {reported}"
    );
    println!("verified the no-room refusal platform={platform}");
}

/// A recipe key this Stado does not know is kept, then refused by name.
///
/// Both halves matter, and they were learnt the hard way on 2026-09-10:
/// declaring `min_free_gb` in the same commit as its reader failed stado
/// 0.20.4 on both platforms with serde's "unknown field", because a release is
/// built by the binary a host already has and that binary denied the key
/// before compiling a single crate. Unknown keys are therefore tolerated by
/// the contract — and named by the submitting binary, so a typo never reaches
/// the queue.
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn an_unknown_recipe_key_is_refused_by_name_before_a_job_is_queued() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-unknown-key-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    fs::create_dir_all(&storage).unwrap();
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    let source = fixture_source(home.path(), platform, "");

    let manifest_path = source.join(".wisent-release.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["platforms"][platform]["min_fee_gb"] = json!(20);
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    run(Command::new("git")
        .current_dir(&source)
        .args(["commit", "-qam", "misspell a recipe key"]));

    let private = home.path().join("release-private");
    let public = home.path().join("release-public");
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

    let mut submit = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit, home.path(), &storage, &vault);
    let refused = submit
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--version",
            "1.0.0",
            "--channel",
            "candidate",
        ])
        .output()
        .unwrap();
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success(),
        "the manifest was accepted: {said}"
    );
    assert!(
        said.contains("unknown recipe keys for this Stado: min_fee_gb"),
        "the refusal did not name the key: {said}"
    );
    // Nothing was queued: the refusal happened while reading the source.
    assert!(
        !storage.join("queue").exists(),
        "a job was queued for a manifest that was refused:\n{}",
        store_snapshot(&storage)
    );
    println!("verified the unknown-key refusal platform={platform}");
}
