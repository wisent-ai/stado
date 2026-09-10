//! A delivery job run again in a work tree its previous attempt already
//! extracted into leaves that tree alone and delivers from one of its own.

use super::*;

/// Plant a `delivery-source` directory in the queued delivery job's work
/// tree before the agent claims it: the state a lease loss leaves behind when
/// the queue runs the same job a second time in the same directory.
fn plant_leftover_source_tree(submit: &mut Child, home: &Path, storage: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(entries) = fs::read_dir(storage.join("queue")) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path()) else {
                    continue;
                };
                let Ok(job) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                let delivery = job["command"]
                    .as_str()
                    .is_some_and(|command| command.contains("release delivery-worker"));
                if job["state"] == "queued" && delivery {
                    let job_id = job["job_id"].as_str().unwrap().to_owned();
                    let leftover = home
                        .join(".stado/work/jobs")
                        .join(format!("wc-{job_id}"))
                        .join("delivery-source");
                    fs::create_dir_all(&leftover).unwrap();
                    fs::write(leftover.join("stale"), b"a previous attempt's file\n").unwrap();
                    return job_id;
                }
            }
        }
        if let Some(status) = submit.try_wait().unwrap() {
            panic!(
                "release submit exited before queuing its delivery: {status}\n\
                 submit stdout:\n{}\nsubmit stderr:\n{}\nstore:{}",
                fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
                fs::read_to_string(home.join("submit.err")).unwrap_or_default(),
                store_snapshot(storage)
            );
        }
        assert!(
            Instant::now() < deadline,
            "release submit queued no delivery within 180 seconds\nstore:{}",
            store_snapshot(storage)
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_delivery_rerun_in_its_own_work_tree_delivers_from_a_fresh_source_tree() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-leftover-")
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
            .stdout(File::create(home.path().join("submit.out")).unwrap())
            .stderr(File::create(home.path().join("submit.err")).unwrap())
            .spawn()
            .unwrap(),
    );
    let job_id = plant_leftover_source_tree(&mut submit.0, home.path(), &storage);
    let status = wait_for_submit(&mut submit.0, &mut agent.0, home.path(), &storage);
    drop(submit);
    drop(agent);
    let stdout = fs::read(home.path().join("submit.out")).unwrap();
    assert!(
        status.success(),
        "a delivery with a leftover source tree failed: {}\n{}",
        String::from_utf8_lossy(&stdout),
        fs::read_to_string(home.path().join("submit.err")).unwrap_or_default()
    );
    let release: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(release["state"], "completed");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["job_id"],
        job_id
    );
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    let work = home
        .path()
        .join(".stado/work/jobs")
        .join(format!("wc-{job_id}"));
    // The attempt extracted the verified source into a tree of its own and
    // never opened the leftover one.
    let own_trees: Vec<PathBuf> = fs::read_dir(&work)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("delivery-source-"))
        })
        .collect();
    assert_eq!(
        own_trees.len(),
        1,
        "the attempt did not extract into exactly one tree of its own: {own_trees:?}"
    );
    assert!(
        own_trees[0].join("Cargo.toml").is_file(),
        "the attempt's own tree carries no source: {:?}",
        own_trees[0]
    );
    assert!(
        work.join("delivery-source/stale").is_file(),
        "the leftover tree was touched"
    );
    println!(
        "verified leftover delivery tree platform={platform}; evidence retained at {}",
        home.keep().display()
    );
}
