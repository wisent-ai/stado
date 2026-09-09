//! Cancelling a release build, and releasing again afterwards.

use serde_json::Value;

use crate::fixture::{run, ReleaseFixture};
use crate::waits;

/// Cancelling a queued release build fails that submit with the job named and
/// the word `cancelled`, leaves the cancellation durably recorded in the
/// store, and does not poison the product: a second submit builds under a new
/// job id, publishes, and installs a binary that answers.
///
/// Gated for the same reason as the other two journeys: the signing key lives
/// in a real Skarbiec broker, and the installed 0.2.40 cannot issue the grant.
#[test]
#[ignore = "needs a skarbiec that can issue grants (the installed 0.2.40 answers `unknown command: grant`): build wisent-ai/skarbiec at origin/main with `cargo build --release --locked`, then run `SKARBIEC_TEST_BIN=<skarbiec>/target/release/skarbiec cargo test --test ci-cd -- --ignored`"]
fn a_cancelled_release_build_is_retried_under_a_new_job() {
    let fixture = ReleaseFixture::start("release-retry-", "", None);

    // The first submit only needs its build queued, so the builder is stopped
    // once it has proved it would claim work: a running worker would compile
    // the release before the cancellation could reach the job.
    let mut initial_agent = fixture.spawn_agent();
    waits::wait_for_claimable_capacity(&fixture, &mut initial_agent);
    initial_agent.kill().unwrap();
    initial_agent.wait().unwrap();

    let mut first_submit = fixture.spawn_submit("submit-first");
    let first_job = waits::wait_for_queued_release_build(&fixture, &mut first_submit);
    let first_job_id = first_job["job_id"].as_str().unwrap().to_string();
    run(fixture.stado().args(["cancel", &first_job_id]));

    let first_status = waits::wait_for_cancelled_submit(&fixture, &mut first_submit);
    assert!(!first_status.success(), "cancelled release submit passed");
    let first_error = fixture.read("submit-first.err");
    assert!(
        first_error.contains(&format!(
            "release job {first_job_id} ({} on ",
            fixture.platform
        )) && first_error.contains("failed: cancelled"),
        "cancelled release reported the wrong failure:\n{first_error}"
    );

    let mut agent = fixture.spawn_agent();
    waits::wait_for_claimable_capacity(&fixture, &mut agent);
    let mut retry_submit = fixture.spawn_submit("submit");
    let status = waits::wait_for_submit(&fixture, &mut retry_submit, &mut agent);
    let report = fixture.read("submit.out");
    let refusal = fixture.read("submit.err");
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(
        status.success(),
        "retried release submit failed:\nstdout:\n{report}\nstderr:\n{refusal}\n\
         agent stderr:\n{}\nstore:{}",
        fixture.read("agent.err"),
        waits::store_snapshot(&fixture.storage)
    );

    let release: Value = serde_json::from_str(&report).unwrap();
    let retry_job_id = release["platforms"][fixture.platform]["job_id"]
        .as_str()
        .unwrap();
    assert_ne!(retry_job_id, first_job_id);
    assert_eq!(release["state"], "completed");
    assert_eq!(release["platforms"][fixture.platform]["state"], "published");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    assert!(
        fixture
            .storage
            .join("cancelled")
            .join(format!("{first_job_id}.json"))
            .is_file(),
        "first job did not remain durably cancelled"
    );
    fixture.assert_installed_probe_answers();
}
