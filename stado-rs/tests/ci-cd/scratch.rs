//! A builder that publishes less free disk than the last build of this
//! product wrote is refused before anything is queued, with both numbers.

use super::*;

/// A prior run of the same product on this platform, whose builder measured a
/// scratch tree no host in this fleet can hold.
fn seed_measured_scratch(storage: &Path, platform: &str) {
    let run_id = "5eed5eed5eed5eed5eed5eed5eed5eed";
    let job_id = "job-5eed5eed5eed5eed5eed5eed";
    let output_prefix = format!("status/{job_id}/output/");
    let run = storage.join(format!("runs/release-pipeline/{run_id}"));
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("run.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "run_id": run_id,
            "product": "ci-release-probe",
            "version": "0.9.0",
            "channel": "candidate",
            "state": "completed",
            "platforms": {
                platform: {
                    "platform": platform,
                    "builder": "ci-runner",
                    "job_id": job_id,
                    "output_prefix": output_prefix,
                    "state": "published"
                }
            },
            "deliveries": {},
            "failure": null
        }))
        .unwrap(),
    )
    .unwrap();
    let output = storage.join(&output_prefix);
    fs::create_dir_all(&output).unwrap();
    fs::write(
        output.join("scratch.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "run_id": run_id,
            "job_id": job_id,
            "product": "ci-release-probe",
            "platform": platform,
            "builder": "ci-runner",
            "bytes": 1_u64 << 50,
            "free_bytes": 1_u64 << 40,
            "build": "passed",
            "measured_at": "2026-09-10T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_builder_short_of_the_last_measured_scratch_is_refused_before_queuing() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-scratch-")
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
    seed_measured_scratch(&storage, platform);

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
            "--json",
        ])
        .output()
        .unwrap();
    drop(agent);
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success(),
        "a builder without room accepted the build: {stderr}"
    );
    for expected in [
        "release_scratch_short",
        "ci-runner accepting jobs but cannot hold this build",
        &format!("the last {platform} build of ci-release-probe wrote 1048576.0 GiB"),
        "GiB free",
        "low watermark",
        "stado space reclaim",
        "stado space watermark <host> --disk-low-free-gb",
    ] {
        assert!(
            stderr.contains(expected),
            "missing {expected:?} in: {stderr}"
        );
    }
    assert!(
        fs::read_dir(storage.join("queue")).map_or(true, |mut queue| queue.next().is_none()),
        "a build was queued despite the refusal:\n{}",
        store_snapshot(&storage)
    );
    // The refusal is in the run the CLI and Desktop both read.
    let refused_run = fs::read_dir(storage.join("runs/release-pipeline"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path().join("run.json"))
        .filter_map(|path| fs::read(path).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .find(|run| run["version"] == "1.0.0")
        .expect("the refused submission recorded its run");
    assert_eq!(refused_run["state"], "failed");
    assert!(
        refused_run["failure"]
            .as_str()
            .is_some_and(|failure| failure.contains("release_scratch_short")),
        "the run does not carry the refusal: {refused_run}"
    );
    println!(
        "verified scratch refusal platform={platform}; evidence retained at {}",
        home.keep().display()
    );
}
