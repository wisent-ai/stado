//! A release build cancelled mid-flight is retried under a new job, so a
//! builder lost to a cancellation is not a release lost with it.
//!
//! The two journeys that must not spend a builder at all live in refusals.rs,
//! and the world each of them runs in is world.rs. Every case here drives the
//! real binary, a real vault and a real worker.

use super::*;

mod refusals;
mod world;

use world::{claiming_agent, said_by, signed_fleet, submit, workspace};


#[test]
#[ignore = "runs the real Skarbiec-backed release journey"]
fn a_cancelled_release_build_is_retried_under_a_new_job() {
    let platform = release_platform();
    let (home, storage, source) = workspace("release-retry-", platform);
    let vault = signed_fleet(home.path(), &storage, platform, true);
    drop(claiming_agent(home.path(), &storage, &vault));

    let first_submit = submit(home.path(), &storage, &vault, &source, "submit-first");
    // `submit` returns once the build is queued; the run is then finished
    // by `resume`, and that is the process the cancellation is reported by.
    let mut first_submit = Running(follow_submission(
        first_submit,
        home.path(),
        &storage,
        &vault,
        "submit-first",
    ));
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
    let mut retry_submit = Running(submit(home.path(), &storage, &vault, &source, "submit"));
    let status = wait_for_submit(
        &mut retry_submit.0,
        &mut agent.0,
        home.path(),
        &storage,
        &vault,
    );
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
