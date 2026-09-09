//! What the agent published while its own janitor was busy, measured on this
//! machine.

use std::time::Duration;

use chrono::{DateTime, Utc};
use stado::constants::{CAPACITY_HEARTBEAT_INTERVAL_S, CAPACITY_STALE_SECONDS};

use crate::fixture::{Journey, Shape};
use crate::fleet::worst_gap;

/// Cache directories the declared cleaner root holds. Sized from live runs on
/// this machine: the walk opens and reads a tag in each one, and crossing this
/// many took between 18 and 33 seconds depending on what else the machine was
/// doing, which is the point — the pass has to outlast the tick's publication
/// cadence even on an idle box.
const DIRECTORIES: usize = 200_000;

/// The floor the measured pass must clear to mean anything: longer than the
/// agent's own poll interval, so a pass on the publication's critical path
/// could not possibly have gone unnoticed. Below the fastest pass measured
/// here, so this bound fails on a regression and not on a fast disk.
const LONG_ENOUGH: Duration = Duration::from_secs(12);

/// The defect, as an invariant, against the engine that caused it.
///
/// The agent is given a cleaner root it cannot cross quickly, and its own
/// janitor spends tens of seconds on it. Every assertion below is read off
/// documents the product wrote: the pass out of the janitor's state file, the
/// publications out of the capacity document the fleet reads. A pass on the
/// publication's critical path would show as a gap at least as long as the
/// pass; what must be true instead is that publications kept landing at the
/// tick's own cadence, including inside the pass's own window.
#[test]
fn a_long_cleanup_pass_does_not_delay_the_capacity_publication() {
    let mut journey = Journey::new(Shape::UnderPressure);
    journey.plant_tree(DIRECTORIES);
    journey.start_agent();

    let publications = journey
        .watch_publications("the agent's own janitor to finish a pass", |state| {
            state.agent_pass().is_some()
        });

    let pass = journey.agent_pass().expect("a completed agent-tick pass");
    let spent = Duration::from_millis(
        u64::try_from(pass["duration_ms"].as_i64().expect("a measured duration"))
            .expect("a positive duration"),
    );
    assert!(
        spent >= LONG_ENOUGH,
        "the real pass took {spent:?}, under the {LONG_ENOUGH:?} that makes this measurement \
         worth making; raise DIRECTORIES. Pass: {pass:#}"
    );
    assert!(
        publications.len() >= 2,
        "only {} publication(s) landed while a {spent:?} pass ran",
        publications.len()
    );
    let worst = worst_gap(&publications);
    assert!(
        worst < Duration::from_secs(CAPACITY_STALE_SECONDS),
        "worst publication gap {worst:?} reached the staleness cutoff, which makes a healthy \
         builder unselectable fleet-wide"
    );
    assert!(
        worst < spent,
        "worst publication gap {worst:?} is as long as the pass {spent:?}, so the pass is back on \
         the publication's critical path"
    );
    let window = pass_window(&pass, spent);
    assert!(
        publications
            .iter()
            .any(|publication| window.contains(&stamp(&publication.published_at))),
        "no publication landed between {} and {}, so nothing here shows a publication surviving a \
         pass in flight",
        window.start,
        window.end
    );
    // The tick publishes on its own cadence, so the completed pass reaches
    // the broadcast on the tick after the janitor persisted it.
    journey.wait_for("the broadcast to carry that pass", |state| {
        state
            .capacity()
            .is_some_and(|document| document["diag"]["disk_cleanup"]["writer"] == "agent-tick")
    });
    let capacity = journey.capacity().expect("a published capacity document");
    assert_eq!(
        capacity["diag"]["disk_cleanup"]["duration_ms"], pass["duration_ms"],
        "the broadcast must carry the pass the agent's own janitor ran: {capacity:#}"
    );
}

/// The count the tick measured has to reach the pass, and the pass has to
/// reach the fleet.
///
/// A janitor told nothing about running work cannot hold back from it, and the
/// scheduler reading `diag.disk_cleanup` would be reading a pass belonging to
/// no host state. With one real claimed workload on this host, the persisted
/// pass and the published document must both say one job was running.
#[test]
fn the_running_job_count_reaches_the_pass_the_agent_publishes() {
    let mut journey = Journey::new(Shape::Claiming);
    let started = journey.home().join("workload.started");
    let release = journey.home().join("workload.release");
    journey.submit(
        "agent-janitor-busy",
        &format!(
            ": > '{}'; while [ ! -f '{}' ]; do /bin/sleep 0.1; done",
            started.display(),
            release.display()
        ),
    );
    journey.start_agent();
    journey.wait_for("the workload to start", |_| started.is_file());

    journey.wait_for("a pass taken while the workload runs", |state| {
        state
            .agent_pass()
            .is_some_and(|pass| pass["active_job_count"].as_i64() == Some(1))
    });
    // The tick publishes on its own cadence, so the broadcast carries that
    // pass one tick after the janitor persisted it, not the same instant.
    journey.wait_for("the broadcast to carry that pass", |state| {
        state.capacity().is_some_and(|document| {
            document["diag"]["disk_cleanup"]["active_job_count"].as_i64() == Some(1)
        })
    });

    let pass = journey.agent_pass().expect("a completed agent-tick pass");
    assert_eq!(pass["active_job_count"], 1, "persisted pass: {pass:#}");
    let capacity = journey.capacity().expect("a published capacity document");
    assert_eq!(capacity["running_jobs"], 1, "broadcast: {capacity:#}");
    assert_eq!(capacity["diag"]["disk_cleanup"]["active_job_count"], 1);
    assert_eq!(capacity["diag"]["disk_cleanup"]["writer"], "agent-tick");
    // The heartbeat's own promise, against the document the fleet reads: this
    // publication is younger than the cutoff a reader drops it at.
    let age = Utc::now() - stamp(capacity["published_at"].as_str().expect("a stamp"));
    assert!(
        age.num_seconds() < i64::try_from(CAPACITY_STALE_SECONDS).expect("a positive cutoff"),
        "the last publication is {age} old, past the {CAPACITY_STALE_SECONDS}s cutoff, with the \
         heartbeat declared at {CAPACITY_HEARTBEAT_INTERVAL_S}s"
    );
    std::fs::write(&release, b"go\n").expect("release the workload");
}

/// The window one pass occupied, from the stamp it recorded and the duration
/// it measured.
fn pass_window(pass: &serde_json::Value, spent: Duration) -> std::ops::Range<DateTime<Utc>> {
    let start = stamp(
        pass["started_at"]
            .as_str()
            .expect("the pass stamped itself"),
    );
    start..start + spent
}

fn stamp(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|error| panic!("the product wrote {text}, which is not RFC 3339: {error}"))
        .with_timezone(&Utc)
}
