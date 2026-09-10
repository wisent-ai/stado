//! Release submissions that must not spend a builder twice, and the two that
//! must not spend one at all.
//!
//! The three journeys here shared forty lines of setup each, which is what
//! pushed this file past the 300-line limit. That setup is now three helpers
//! and every case still drives the real binary, a real Skarbiec and a real
//! worker.
use super::*;

/// One isolated release world: an ignored run root under this package's own
/// `target/`, the operator's toolchain symlinked in so a build can compile,
/// an empty store and the fixture product's committed source.
fn workspace(prefix: &str, platform: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(run_root)
        .unwrap();
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    for directory in [".cargo", ".rustup"] {
        std::os::unix::fs::symlink(operator_home.join(directory), home.path().join(directory))
            .unwrap();
    }
    let storage = home.path().join("store");
    fs::create_dir_all(&storage).unwrap();
    let source = fixture_source(home.path(), platform, "");
    (home, storage, source)
}

/// The signing key, the vault that holds it and the registry that trusts it.
/// `with_worker` also stages the built binary where a claimed job runs it,
/// which a journey that never reaches a build does not need.
fn signed_fleet(home: &Path, storage: &Path, platform: &str, with_worker: bool) -> SkarbiecFixture {
    let private = home.join("release-private");
    let public = home.join("release-public");
    if with_worker {
        let worker_bin = home.join(".stado/bin/stado");
        fs::create_dir_all(worker_bin.parent().unwrap()).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_stado"), &worker_bin).unwrap();
        fs::set_permissions(&worker_bin, fs::Permissions::from_mode(0o700)).unwrap();
    }
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
    let vault = SkarbiecFixture::start_release(home, &private);
    registry(home, storage, &public_key, platform, None);
    vault
}

/// A real worker, running until its capacity is claimable.
fn claiming_agent(home: &Path, storage: &Path, vault: &SkarbiecFixture) -> Running {
    let out = File::create(home.join("agent.out")).unwrap();
    let err = File::create(home.join("agent.err")).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    let mut agent = Running(
        command
            .args(["agent", "--target", "ci-runner"])
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap(),
    );
    wait_for_claimable_capacity(storage, home, &mut agent.0);
    agent
}

/// One `release submit`, spawned with its output retained beside the run.
fn submit(
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
    source: &Path,
    name: &str,
) -> Running {
    let out = File::create(home.join(format!("{name}.out"))).unwrap();
    let err = File::create(home.join(format!("{name}.err"))).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    Running(
        command
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
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .unwrap(),
    )
}

fn said_by(home: &Path, name: &str) -> String {
    format!(
        "{}{}",
        fs::read_to_string(home.join(format!("{name}.out"))).unwrap_or_default(),
        fs::read_to_string(home.join(format!("{name}.err"))).unwrap_or_default()
    )
}

/// Declare one more key on this platform's recipe and commit it, the way a
/// product declares a requirement its release worker is meant to read.
fn declare(source: &Path, platform: &str, key: &str, value: Value) {
    let manifest_path = source.join(".wisent-release.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["platforms"][platform][key] = value;
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    run(Command::new("git")
        .current_dir(source)
        .args(["commit", "-qam", "declare a recipe key"]));
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_cancelled_release_build_is_retried_under_a_new_job() {
    let platform = release_platform();
    let (home, storage, source) = workspace("release-retry-", platform);
    let vault = signed_fleet(home.path(), &storage, platform, true);
    drop(claiming_agent(home.path(), &storage, &vault));

    let mut first_submit = submit(home.path(), &storage, &vault, &source, "submit-first");
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
    let first_error = said_by(home.path(), "submit-first");
    assert!(
        first_error.contains(&format!("release job {first_job_id} ({platform} on "))
            && first_error.contains("failed: cancelled"),
        "cancelled release reported the wrong failure:\n{first_error}"
    );

    let mut agent = claiming_agent(home.path(), &storage, &vault);
    let mut retry_submit = submit(home.path(), &storage, &vault, &source, "submit");
    let status = wait_for_submit(&mut retry_submit.0, &mut agent.0, home.path(), &storage);
    let result = Output {
        status,
        stdout: fs::read(home.path().join("submit.out")).unwrap(),
        stderr: fs::read(home.path().join("submit.err")).unwrap(),
    };
    drop(agent);
    assert!(
        result.status.success(),
        "retried release submit failed:\n{}\nagent:\n{}\nstore:{}",
        said_by(home.path(), "submit"),
        said_by(home.path(), "agent"),
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
        "verified cancelled release retry platform={platform}; first_job={first_job_id}; retry_job={retry_job_id}"
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
    let (home, storage, source) = workspace("release-no-room-", platform);
    // The one difference from every other journey here: this product declares
    // more free space than any volume has.
    declare(&source, platform, "min_free_gb", json!(99_999_999_u64));
    let vault = signed_fleet(home.path(), &storage, platform, true);
    let mut agent = claiming_agent(home.path(), &storage, &vault);

    let mut running = submit(home.path(), &storage, &vault, &source, "submit");
    let status = wait_for_submit(&mut running.0, &mut agent.0, home.path(), &storage);
    drop(agent);
    let reported = said_by(home.path(), "submit");
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
/// Declaring `min_free_gb` in the same commit as its reader failed stado
/// 0.20.4 on both platforms with serde's "unknown field", because a release is
/// built by the binary a host already has and that binary denied the key
/// before compiling a single crate. Unknown keys are therefore tolerated by
/// the contract — and named by the submitting binary, so a typo never reaches
/// the queue.
#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn an_unknown_recipe_key_is_refused_by_name_before_a_job_is_queued() {
    let platform = release_platform();
    let (home, storage, source) = workspace("release-unknown-key-", platform);
    declare(&source, platform, "min_fee_gb", json!(20));
    let vault = signed_fleet(home.path(), &storage, platform, false);

    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home.path(), &storage, &vault);
    let refused = command
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
