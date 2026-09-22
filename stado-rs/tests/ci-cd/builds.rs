//! A build is not a release, proved through the real binary.
//!
//! `stado build submit` compiles one committed source on a real worker and
//! keeps the result under its own id. A release made from that build is
//! refused while the build is still waiting, consumes the same job once the
//! build has passed — no second compile — and then publishes, delivers and
//! installs exactly as a source submission does. Submitting the same source
//! again resumes the same build and queues nothing.
use super::*;

/// One `stado` command under the release environment, its output retained
/// beside the journey's other evidence.
fn stado(
    home: &Path,
    storage: &Path,
    vault: &SkarbiecFixture,
    name: &str,
    args: &[&str],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut command, home, storage, vault);
    let output = command.args(args).output().unwrap();
    fs::write(home.join(format!("{name}.out")), &output.stdout).unwrap();
    fs::write(home.join(format!("{name}.err")), &output.stderr).unwrap();
    output
}

fn document(output: &Output, what: &str) -> Value {
    assert!(
        output.status.success(),
        "{what} failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_build_is_kept_on_its_own_and_a_release_consumes_it_once_it_passed() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("build-release-")
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
    registry(
        home.path(),
        &storage,
        &public_key,
        platform,
        None,
        &vault.url(),
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

    // The build: queued, recorded under its own id, nothing published.
    let queued = document(
        &stado(
            home.path(),
            &storage,
            &vault,
            "build-submit",
            &[
                "build",
                "submit",
                "--source",
                source.to_str().unwrap(),
                "--version",
                "1.0.0",
                "--json",
            ],
        ),
        "build submit",
    );
    let build_id = queued["build_id"].as_str().unwrap().to_owned();
    assert_eq!(queued["state"], "waiting", "{queued}");
    assert_eq!(queued["product"], "ci-release-probe");
    assert_eq!(queued["platforms"][platform]["builder"], "ci-runner");
    let job_id = queued["platforms"][platform]["job_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        !storage.join("releases").exists(),
        "a build published something: {}",
        store_snapshot(&storage)
    );

    // A release of a build that has not passed is refused, and leaves no run.
    let refused = stado(
        home.path(),
        &storage,
        &vault,
        "release-early",
        &[
            "release",
            "submit",
            "--build",
            &build_id,
            "--channel",
            "candidate",
        ],
    );
    assert!(!refused.status.success(), "a waiting build was released");
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(
        complaint.contains(&format!("build {build_id} is waiting, not passed")),
        "the refusal names the build and its state: {complaint}"
    );
    assert!(
        !storage.join("runs/release-pipeline").exists(),
        "a refused release recorded a run: {}",
        store_snapshot(&storage)
    );

    // The worker builds it; `build status --wait` follows the job to its end
    // and the record says what happened from the job's own receipt.
    let status_out = File::create(home.path().join("build-status.out")).unwrap();
    let status_err = File::create(home.path().join("build-status.err")).unwrap();
    let mut following = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut following, home.path(), &storage, &vault);
    let mut following = following
        .args(["build", "status", &build_id, "--wait", "--json"])
        .stdout(Stdio::from(status_out))
        .stderr(Stdio::from(status_err))
        .spawn()
        .unwrap();
    let followed = wait_for_submit(&mut following, &mut agent, home.path(), &storage, &vault);
    let passed = document(
        &Output {
            status: followed,
            stdout: fs::read(home.path().join("build-status.out")).unwrap(),
            stderr: fs::read(home.path().join("build-status.err")).unwrap(),
        },
        "build status --wait",
    );
    assert_eq!(passed["state"], "passed", "{passed}");
    assert_eq!(passed["platforms"][platform]["state"], "qualified");
    assert_eq!(passed["platforms"][platform]["job_id"], job_id.as_str());
    let receipt: Value = serde_json::from_slice(
        &fs::read(storage.join(format!("status/{job_id}/output/receipt.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        receipt["run_id"],
        build_id.as_str(),
        "the receipt names the build"
    );
    assert_eq!(
        receipt["artifact"]["sha256"],
        passed["platforms"][platform]["artifact_sha256"]
    );

    let listed = document(
        &stado(
            home.path(),
            &storage,
            &vault,
            "build-list",
            &["build", "list", "--json"],
        ),
        "build list",
    );
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|build| build["build_id"] == build_id.as_str() && build["state"] == "passed"),
        "the listing does not carry the build: {listed}"
    );

    // The same source again is the same build, and queues nothing.
    let resubmitted = document(
        &stado(
            home.path(),
            &storage,
            &vault,
            "build-resubmit",
            &[
                "build",
                "submit",
                "--source",
                source.to_str().unwrap(),
                "--version",
                "1.0.0",
                "--json",
            ],
        ),
        "build submit again",
    );
    assert_eq!(resubmitted["build_id"], build_id.as_str());
    assert_eq!(resubmitted["state"], "passed");
    assert_eq!(
        resubmitted["platforms"][platform]["job_id"],
        job_id.as_str()
    );

    // The release consumes the passed build: the run names it, reads the same
    // job, and publishes, delivers and installs without a second compile.
    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut release = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut release, home.path(), &storage, &vault);
    let mut release = release
        .args([
            "release",
            "submit",
            "--build",
            &build_id,
            "--channel",
            "candidate",
            "--json",
        ])
        .stdout(Stdio::from(submit_out))
        .stderr(Stdio::from(submit_err))
        .spawn()
        .unwrap();
    let status = wait_for_submit(&mut release, &mut agent, home.path(), &storage, &vault);
    let result = Output {
        status,
        stdout: fs::read(home.path().join("submit.out")).unwrap(),
        stderr: fs::read(home.path().join("submit.err")).unwrap(),
    };
    let _ = agent.kill();
    let _ = agent.wait();
    if !result.status.success() {
        let retained = home.keep();
        panic!(
            "release submit --build failed; evidence retained at {}\nstdout:\n{}\nstderr:\n{}\nstore:\n{}",
            retained.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
            store_snapshot(&storage),
        );
    }
    let released: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(released["state"], "completed", "{released}");
    assert_eq!(released["build_id"], build_id.as_str());
    assert_eq!(released["platforms"][platform]["state"], "published");
    assert_eq!(
        released["platforms"][platform]["job_id"],
        job_id.as_str(),
        "the release compiled again instead of consuming the build"
    );
    assert_eq!(
        released["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert!(installed.exists(), "delivery did not install {installed:?}");
    let output = run(&mut Command::new(&installed));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ci-release-probe 1.0.0"
    );
    println!("verified build separation platform={platform}; build={build_id}");
    println!("build evidence retained at {}", home.keep().display());
}
