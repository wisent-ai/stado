//! Real build → release → install proof.
//!
//! The test drives the compiled `stado` binary end to end. It creates a clean
//! committed Rust product, starts a real Stado worker against an isolated
//! local store, runs `stado release submit`, requires the worker to execute
//! `cargo check` and `cargo build --release`, signs and publishes the archive,
//! delivers it through `stado release install-local`, then executes the
//! installed binary and checks its version output. No fleet host or operator
//! registry is read or changed.
use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

#[path = "../support/skarbiec.rs"]
mod skarbiec_support;
use skarbiec_support::{SkarbiecFixture, SkarbiecItem};

mod commit;
mod fixture;
mod leftover;
mod preflight;
mod resume;
mod retry;
mod scratch;
use fixture::*;

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_real_release_builds_publishes_and_installs_its_binary() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-")
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
    let mut agent = agent_command
        .args(["agent", "--target", "ci-runner"])
        .stdout(Stdio::from(agent_out))
        .stderr(Stdio::from(agent_err))
        .spawn()
        .unwrap();
    wait_for_claimable_capacity(&storage, home.path(), &mut agent);
    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut submit = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit, home.path(), &storage, &vault);
    let mut submit = submit
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
    let status = wait_for_submit(&mut submit, &mut agent, home.path(), &storage);
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
            "release submit failed; evidence retained at {}\nstdout:\n{}\nstderr:\n{}\nstore:\n{}",
            retained.display(),
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
            store_snapshot(&storage),
        );
    }
    let release: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(release["state"], "completed");
    assert_eq!(release["platforms"][platform]["state"], "published");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );

    let installed = home.path().join(".stado/bin/ci-release-probe");
    assert!(installed.exists(), "delivery did not install {installed:?}");
    let output = run(&mut Command::new(&installed));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ci-release-probe 1.0.0"
    );
    #[cfg(target_os = "macos")]
    run(Command::new("/usr/bin/codesign").args([
        "--verify",
        "--strict",
        "-R",
        "=anchor apple generic",
        installed.to_str().unwrap(),
    ]));
    // The builder measured its own scratch and the bootstrap stored it
    // beside the receipt; the next placement of this product reads it.
    let job_id = release["platforms"][platform]["job_id"].as_str().unwrap();
    let scratch: Value = serde_json::from_slice(
        &fs::read(storage.join(format!("status/{job_id}/output/scratch.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(scratch["product"], "ci-release-probe");
    assert_eq!(scratch["platform"], platform);
    assert_eq!(scratch["builder"], "ci-runner");
    assert_eq!(scratch["build"], "passed");
    assert!(
        scratch["bytes"].as_u64().unwrap() > 0 && scratch["free_bytes"].as_u64().unwrap() > 0,
        "the build measured nothing: {scratch}"
    );
    println!("verified release platform={platform}; installed=ci-release-probe 1.0.0");
    println!("release evidence retained at {}", home.keep().display());
}

#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn stale_target_capacity_still_enqueues_its_exact_release_delivery() {
    let platform = release_platform();
    let run_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ci-cd-runs");
    fs::create_dir_all(&run_root).unwrap();
    let home = tempfile::Builder::new()
        .prefix("release-recovery-")
        .tempdir_in(run_root)
        .unwrap();
    let storage = home.path().join("store");
    let operator_home = PathBuf::from(std::env::var_os("HOME").unwrap());
    std::os::unix::fs::symlink(operator_home.join(".cargo"), home.path().join(".cargo")).unwrap();
    std::os::unix::fs::symlink(operator_home.join(".rustup"), home.path().join(".rustup")).unwrap();
    fs::create_dir_all(&storage).unwrap();
    let target = "offline-recovery";
    let consumer = "local-offline-recovery.invalid";
    let source = fixture_source(home.path(), platform, target);

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
        Some((target, "offline-recovery.invalid")),
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
    seed_stale_capacity(&storage, consumer);

    let submit_out = File::create(home.path().join("submit.out")).unwrap();
    let submit_err = File::create(home.path().join("submit.err")).unwrap();
    let mut submit = Command::new(env!("CARGO_BIN_EXE_stado"));
    release_env(&mut submit, home.path(), &storage, &vault);
    let mut submit = submit
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
    let delivery =
        wait_for_recovery_delivery(&mut submit, &mut agent, home.path(), &storage, consumer);
    let _ = submit.kill();
    let _ = submit.wait();
    let _ = agent.kill();
    let _ = agent.wait();

    assert_eq!(delivery["pinned_host"], consumer);
    assert_eq!(delivery["priority"], stado::primitives::constants::RELEASE_JOB_PRIORITY);
    assert_eq!(
        delivery["command"],
        stado::primitives::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
    );
    assert!(
        delivery["output_uri"]
            .as_str()
            .is_some_and(|uri| uri.contains("/deliveries/install-on-builder/output")),
        "delivery keeps its durable release output coordinate: {delivery}"
    );
}
