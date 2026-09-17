//! `stado host gates` against a queue prefix that is mostly history, and the
//! reaper pass that retires that history.
//!
//! On 2026-09-17 every host's gates read timed out at ten seconds on
//! `stado:queue/`: the prefix held 779 objects for 15 queued jobs, because a
//! job that leaves the queue leaves a `transition-cleaned:` sentinel behind
//! and nothing ever removed one. The gates read downloaded all of them to
//! find the two that were pinned. Two things changed, and this journey drives
//! both through the real binary against an isolated local store:
//!
//! - the read walks the priority index, which names queued jobs and nothing
//!   else, so the sentinels cost it nothing and the verdict is decided;
//! - the coordinator's reaper pass deletes the sentinel of a job that has
//!   settled in a terminal prefix, once it is a day old, bounded per tick.

mod support;

use std::fs;
use std::time::{Duration, SystemTime};

use support::{observation, Journey, ELSEWHERE, HOST};

/// A day and a minute: past the sweep's 24-hour age floor with margin.
const PAST_THE_AGE_FLOOR: Duration = Duration::from_secs(24 * 3600 + 60);

#[test]
fn gates_decide_from_the_index_while_the_queue_prefix_is_mostly_history() {
    let journey = Journey::new();
    journey.publish_capacity();
    let mine: Vec<String> = (0..2)
        .map(|n| journey.submit_pinned(HOST, &format!("printf mine-{n}")))
        .collect();
    let gone: Vec<String> = (0..12)
        .map(|n| journey.submit_pinned(ELSEWHERE, &format!("printf gone-{n}")))
        .collect();
    for id in &gone {
        journey.invoke_ok(&["cancel", id]);
        assert!(
            journey
                .queue_state(id)
                .is_some_and(|state| state.starts_with("transition-cleaned:")),
            "a cancelled job leaves a cleaned sentinel in queue/, which is the history this read must not pay for: {id}"
        );
    }
    let objects = journey.queue_objects().len();
    assert!(
        objects >= mine.len() + gone.len(),
        "queue/ holds the live jobs and one sentinel per cancelled job: {objects}"
    );

    let (report, output) = journey.gates();
    let queue = observation(&report, "queue");
    assert_eq!(
        queue["state"], "complete",
        "the queue read must be decided, not timed out: {queue}"
    );
    assert!(
        report["claiming"].is_boolean(),
        "the verdict is decided from what was read: {report}"
    );
    let waiting: Vec<&str> = report["waiting_jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("the report lists pinned work: {report}"))
        .iter()
        .filter_map(|job| job["job_id"].as_str())
        .collect();
    for id in &mine {
        assert!(
            waiting.contains(&id.as_str()),
            "a job pinned here is missing from the verdict: {id} not in {waiting:?}; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for id in &gone {
        assert!(
            !waiting.contains(&id.as_str()),
            "a cancelled job was reported as waiting: {id}"
        );
    }
}

#[test]
fn the_reaper_retires_day_old_sentinels_of_terminal_jobs_and_keeps_live_ones() {
    let journey = Journey::new();
    let live = journey.submit_pinned(HOST, "printf live");
    let gone: Vec<String> = (0..3)
        .map(|n| journey.submit_pinned(ELSEWHERE, &format!("printf gone-{n}")))
        .collect();
    for id in &gone {
        journey.invoke_ok(&["cancel", id]);
    }
    // Age every queue object past the floor: the sweep never reads a
    // younger one, so a sentinel of a transition still finishing is safe.
    let old = SystemTime::now() - PAST_THE_AGE_FLOOR;
    for path in journey.queue_objects() {
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    // One more is still young. The sweep's own count below proves it was
    // never inspected: four objects are older than the floor, and four is
    // what the sweep reads. (The run reaper that follows in the same tick
    // may still remove a cancelled run's blobs; that is its own contract.)
    let young = journey.submit_pinned(ELSEWHERE, "printf young");
    journey.invoke_ok(&["cancel", &young]);

    let tick = journey.invoke_ok(&["coordinator", "--once"]);
    let tick_text = format!(
        "{}{}",
        String::from_utf8_lossy(&tick.stdout),
        String::from_utf8_lossy(&tick.stderr)
    );
    assert!(
        tick_text.contains("reaper: queue/ settled sentinels retired=3 kept=1 inspected=4"),
        "the tick did not report the sweep it ran: {tick_text}"
    );

    for id in &gone {
        assert!(
            journey.queue_state(id).is_none(),
            "the settled sentinel of a cancelled job survived the sweep: {id}"
        );
    }
    assert_eq!(
        journey.queue_state(&live).as_deref(),
        Some("queued"),
        "a day-old live job was deleted by the sweep"
    );
}
