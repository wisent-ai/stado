//! What an enforcing pass answers while a real workload holds the cleanup
//! lock on this machine, and after that workload is gone.

use crate::fixture::Journey;

/// The wedge and its release, in one journey because both halves need the same
/// live agent and the same live workload.
///
/// While the workload runs the pass is refused — `lock_busy_unattributed`,
/// because a workload hold leaves no holder record — and the backdated cache
/// directory an enforcing pass exists to remove is still on disk afterwards.
/// Once the workload has a terminal record the same command reclaims it. The
/// evidence on both sides is the directory, not the report.
#[test]
fn a_live_workload_refuses_the_pass_and_its_exit_gives_it_back() {
    let mut journey = Journey::new();
    let candidate = journey.eligible_cache("held-candidate");
    let started = journey.home().join("workload.started");
    let release = journey.home().join("workload.release");
    let job = journey.submit(
        "hold-release",
        &format!(
            ": > '{}'; while [ ! -f '{}' ]; do /bin/sleep 0.1; done",
            started.display(),
            release.display()
        ),
    );
    journey.start_agent();
    journey.wait_for("the workload to start", |_| started.is_file());

    let refused = journey.reclaim();

    assert_eq!(
        refused["outcome"], "lock_busy_unattributed",
        "pass under a live workload: {refused:#}"
    );
    assert_eq!(refused["lock_busy"], true);
    assert_eq!(refused["errors"], serde_json::json!(["lock_busy:OSError"]));
    assert!(
        candidate.join("payload.bin").is_file(),
        "a refused pass must delete nothing"
    );
    assert!(journey
        .reported_janitor_line()
        .starts_with("janitor: lock_busy_unattributed"));

    std::fs::write(&release, b"go\n").expect("release the workload");
    journey.wait_for("the workload's terminal record", |state| {
        state.recorded("completed", &job)
    });

    let reclaimed = journey.reclaim();

    assert_eq!(
        reclaimed["outcome"], "reclaimed_progress",
        "pass after the workload exited: {reclaimed:#}"
    );
    assert_eq!(reclaimed["cleaners"]["build_caches"]["deleted_items"], 1);
    assert!(
        !candidate.exists(),
        "a workload that has exited must leave the run lock takeable"
    );
    assert_eq!(
        journey.persisted()["report"]["outcome"],
        "reclaimed_progress"
    );
}

/// The failure mode, not the instance: the workload is gone, and the slot is
/// NOT.
///
/// With every terminal prefix unwritable the agent cannot finalize, so
/// `advance_slot` keeps the slot and retries on every tick for as long as the
/// agent lives — the unbounded retry that turned a shared hold into a
/// permanent one for 11.5 hours. The lock must come back anyway, and it must
/// come back while the job still has no terminal record at all.
#[test]
fn a_retained_slot_still_gives_the_run_lock_back() {
    let mut journey = Journey::new();
    let candidate = journey.eligible_cache("retained-candidate");
    let started = journey.home().join("workload.started");
    let release = journey.home().join("workload.release");
    let finished = journey.home().join("workload.finished");
    let job = journey.submit(
        "hold-retained",
        &format!(
            ": > '{}'; while [ ! -f '{}' ]; do /bin/sleep 0.1; done; : > '{}'",
            started.display(),
            release.display(),
            finished.display()
        ),
    );
    journey.start_agent();
    journey.wait_for("the workload to start", |_| started.is_file());
    journey.refuse_terminal_records(true);
    std::fs::write(&release, b"go\n").expect("release the workload");
    journey.wait_for("the workload's own process to finish", |_| {
        finished.is_file()
    });

    let (waited, report) = journey.wait_for_the_lock();

    assert_eq!(
        report["outcome"], "reclaimed_progress",
        "pass after {waited:?} of retried finalization: {report:#}"
    );
    assert!(
        !candidate.exists(),
        "a retained slot must not keep the janitor's run lock"
    );
    assert!(
        !journey.terminal(&job),
        "this case only means anything while the store still refuses the terminal record"
    );
    assert!(
        journey.recorded("running", &job),
        "the slot must still be retained, with the job's record where it was"
    );
}

/// A workload that leaves by an unhappy path. A non-zero exit is finalized
/// down a different branch from a clean one, and it must return the hold just
/// the same — the product records the job as failed, and the pass runs.
#[test]
fn a_workload_that_fails_gives_the_run_lock_back() {
    let mut journey = Journey::new();
    let candidate = journey.eligible_cache("failed-candidate");
    let started = journey.home().join("workload.started");
    let job = journey.submit(
        "hold-failed",
        &format!(": > '{}'; /bin/sleep 1; exit 7", started.display()),
    );
    journey.start_agent();
    journey.wait_for("the workload to start", |_| started.is_file());
    journey.wait_for("the failed job's record", |state| {
        state.recorded("failed", &job)
    });

    let report = journey.reclaim();

    assert_eq!(
        report["outcome"], "reclaimed_progress",
        "pass after a failed workload: {report:#}"
    );
    assert!(
        !candidate.exists(),
        "a workload that exited non-zero must not leave the run lock held"
    );
}
