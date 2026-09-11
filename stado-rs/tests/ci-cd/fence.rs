//! A release run that has published nothing does not supersede the
//! deliveries of an older run that has.
//!
//! The delivery fence used to be the newest run of any state. On 2026-09-11
//! two Stado 0.20.10 runs failed before a builder was found, thirty seconds
//! after the 0.20.11 run was created, and every 0.20.11 delivery refused
//! itself on every host as superseded by runs that would never deliver a
//! byte. This drives the real product through that order: a release whose
//! build is queued, a later submission that fails before anything is built,
//! and the earlier release still delivering and installing its binary.

use super::*;

/// The runner platform this machine does not build, so a manifest naming it
/// finds no live builder in the isolated fleet and fails before enqueueing.
fn platform_nobody_builds() -> &'static str {
    match release_platform() {
        "darwin-arm64" => "linux-amd64",
        _ => "darwin-arm64",
    }
}

/// Commit a second version of the fixture product whose only platform is one
/// the isolated fleet cannot build, and return that commit.
fn commit_unbuildable_version(source: &Path) -> String {
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"ci-release-probe\"\nversion = \"2.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(source.join(".wisent-release.json")).unwrap()).unwrap();
    let recipe = manifest["platforms"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .cloned()
        .unwrap();
    let elsewhere = platform_nobody_builds();
    let mut recipe = recipe;
    recipe["runner_platform"] = json!(elsewhere);
    manifest["platforms"] = json!({ elsewhere: recipe });
    manifest["deliveries"] = json!([]);
    fs::write(
        source.join(".wisent-release.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    git(source, &["add", "Cargo.toml", ".wisent-release.json"]);
    git(
        source,
        &["commit", "-qm", "a version nobody here can build"],
    );
    String::from_utf8(
        run(Command::new("git")
            .current_dir(source)
            .args(["rev-parse", "HEAD"]))
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

/// Wait until the run the coordinator records for `version` reaches `state`.
fn wait_for_run_state(submit: &mut Child, home: &Path, storage: &Path, version: &str, state: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let runs = storage.join("runs/release-pipeline");
        if let Ok(entries) = fs::read_dir(&runs) {
            for entry in entries.flatten() {
                let Ok(bytes) = fs::read(entry.path().join("run.json")) else {
                    continue;
                };
                let Ok(run) = serde_json::from_slice::<Value>(&bytes) else {
                    continue;
                };
                if run["version"] == version && run["state"] == state {
                    return;
                }
            }
        }
        if let Some(status) = submit.try_wait().unwrap() {
            panic!(
                "release submit exited before its run reached {state}: {status}\n{}\n{}",
                fs::read_to_string(home.join("submit.out")).unwrap_or_default(),
                fs::read_to_string(home.join("submit.err")).unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "the {version} run did not reach {state} within 120 seconds\nstore:{}",
            store_snapshot(storage)
        );
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_run_that_published_nothing_does_not_fence_an_older_release_delivery() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-fence-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");
    let released = String::from_utf8(
        run(Command::new("git")
            .current_dir(&source)
            .args(["rev-parse", "HEAD"]))
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let unbuildable = commit_unbuildable_version(&source);

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

    // The release: its build is queued to the live worker and its coordinator
    // waits on it.
    let mut submit_command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit_command, home.path(), &storage, &vault);
    let mut submit = Running(
        submit_command
            .args([
                "release",
                "submit",
                "--source",
                source.to_str().unwrap(),
                "--commit",
                &released,
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

    // The release is on record and waiting on its build before the later
    // submission is made, so the two never race for the product's catalog
    // entry, which the first submission of a product creates.
    wait_for_run_state(&mut submit.0, home.path(), &storage, "1.0.0", "publishing");

    // A later submission that fails before anything is built: no live host
    // builds its platform. It is recorded, and it is newer.
    let mut later = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut later, home.path(), &storage, &vault);
    let later = later
        .args([
            "release",
            "submit",
            "--source",
            source.to_str().unwrap(),
            "--commit",
            &unbuildable,
            "--version",
            "2.0.0",
            "--channel",
            "candidate",
            "--json",
        ])
        .output()
        .unwrap();
    let later_stderr = String::from_utf8_lossy(&later.stderr).into_owned();
    assert!(
        !later.status.success() && later_stderr.contains("no live fleet builder can CLAIM"),
        "the later submission did not fail for want of a builder: {later_stderr}"
    );

    let status = wait_for_submit(&mut submit.0, &mut agent.0, home.path(), &storage);
    drop(submit);
    drop(agent);
    let stdout = fs::read(home.path().join("submit.out")).unwrap();
    let stderr = fs::read_to_string(home.path().join("submit.err")).unwrap_or_default();
    assert!(
        status.success(),
        "the release did not survive a newer run that built nothing: {}\n{stderr}",
        String::from_utf8_lossy(&stdout)
    );
    let release: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(release["state"], "completed", "{release}");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"], "passed",
        "{release}"
    );
    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert_eq!(
        run(&mut Command::new(installed)).stdout,
        b"ci-release-probe 1.0.0\n"
    );

    // The later run is on record, failed, newer, and published nothing.
    let failed = fs::read_dir(storage.join("runs/release-pipeline"))
        .unwrap()
        .flatten()
        .filter_map(|entry| fs::read(entry.path().join("run.json")).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .find(|run| run["version"] == "2.0.0")
        .expect("the later submission left a run record");
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(
        failed["platforms"]
            .as_object()
            .unwrap()
            .values()
            .all(|platform| platform["state"] != "published"),
        "the later run published something: {failed}"
    );
    assert!(
        failed["created_at"].as_str().unwrap() > release["created_at"].as_str().unwrap(),
        "the later run is not newer: {failed} vs {release}"
    );
    println!(
        "verified a published release delivered past a newer empty run on {platform}; evidence \
         retained at {}",
        home.keep().display()
    );
}
