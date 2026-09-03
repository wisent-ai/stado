//! A long disk-cleanup pass must never delay a capacity publication.
//!
//! # The invariant
//!
//! `constants::CAPACITY_STALE_SECONDS` is 180 and
//! `constants::CAPACITY_HEARTBEAT_INTERVAL_S` is one third of it, carrying the
//! comment "always fresh before the stale threshold". That pair IS the
//! contract: published at least every heartbeat, valid for three heartbeats.
//! `cli::release_submit::builder` refuses a submission outright when no fresh
//! publication names the platform, so a host that stops publishing stops being
//! a release builder anywhere in the fleet — while remaining healthy, running
//! and correctly declared.
//!
//! The agent tick used to `await run_cleanup_once` before reaching its
//! publication, on the same task. Measured on charless-mac-mini on 2026-09-03:
//! `duration_ms: 818021` for a `healthy_noop` pass, against a policy interval
//! of 300s, so passes ran back to back and the builder was selectable about
//! three minutes in every fourteen. Two weles-worker releases were refused for
//! it.
//!
//! # What is defended here
//!
//! These tests defend the invariant rather than the incident: a pass many times
//! longer than the heartbeat must not stretch the interval between
//! publications, `latest()` must never wait for a pass in flight, and a tick
//! must be able to publish before any pass has ever completed. Nothing here
//! asserts 818 seconds or 180 seconds; it asserts the RATIO the constants
//! declare, at a scale a test can run.
//!
//! Time is scaled, not mocked: the heartbeat is milliseconds and the pass is
//! many heartbeats long, so the same arithmetic the constants express is
//! exercised without a 180-second test. Nothing here touches a real vault, a
//! real store, or the cleanup engine — the pass is a stand-in that only sleeps,
//! which is precisely the behaviour that broke the fleet.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use stado::constants::{CAPACITY_HEARTBEAT_INTERVAL_S, CAPACITY_STALE_SECONDS};
use stado::providers::local::agent_janitor::JanitorReports;

/// The test's stand-in for the heartbeat. Small enough to run, and every other
/// duration below is expressed as a multiple of it, so the ratios under test
/// are the ratios the constants declare.
const HEARTBEAT: Duration = Duration::from_millis(20);

/// How much longer than a heartbeat one pass takes. charless-mac-mini's real
/// ratio was 818s against a 60s heartbeat — about 13. Ten is the same shape.
const PASS_HEARTBEATS: u32 = 10;

/// The scaled staleness cutoff, in the same relation to the heartbeat that the
/// constants define.
fn scaled_stale() -> Duration {
    HEARTBEAT * (CAPACITY_STALE_SECONDS / CAPACITY_HEARTBEAT_INTERVAL_S) as u32
}

/// The constants must keep declaring the relation the fix relies on.
///
/// Asserted in a `const` block, so breaking it fails the BUILD rather than a
/// test run: if the heartbeat ever stops being strictly shorter than the
/// staleness cutoff, a perfectly punctual publisher goes stale and everything
/// below is defending the wrong thing. Clippy asked for the const block and
/// was right — a constant relation should be judged at compile time.
#[test]
fn the_heartbeat_is_strictly_fresher_than_the_staleness_cutoff() {
    const {
        assert!(
            CAPACITY_HEARTBEAT_INTERVAL_S < CAPACITY_STALE_SECONDS,
            "the capacity heartbeat must be shorter than the staleness cutoff"
        );
        assert!(
            CAPACITY_STALE_SECONDS / CAPACITY_HEARTBEAT_INTERVAL_S >= 2,
            "the staleness cutoff must allow at least one missed heartbeat"
        );
    }
}

/// The defect, as an invariant: a pass ten heartbeats long must not stretch the
/// gap between publications. Before the fix the tick awaited the pass, so the
/// gap became the pass duration and every publication past the cutoff was
/// invisible to `release submit`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_long_cleanup_pass_does_not_delay_capacity_publication() {
    let reports = JanitorReports::new();
    let passes = Arc::new(AtomicI64::new(0));
    let counted = Arc::clone(&passes);
    let pass_duration = HEARTBEAT * PASS_HEARTBEATS;

    let janitor = reports.spawn_janitor(HEARTBEAT, move |active_slots| {
        let counted = Arc::clone(&counted);
        async move {
            // Exactly what the real pass did to the fleet: take far longer than
            // the heartbeat while producing a verdict that freed nothing.
            tokio::time::sleep(pass_duration).await;
            counted.fetch_add(1, Ordering::Relaxed);
            json!({"outcome": "healthy_noop", "active_slots": active_slots})
        }
    });

    // Drive ticks for two full pass durations, recording when each one would
    // have published. A tick does what the agent's tick now does: read the
    // latest COMPLETED report and move on.
    let mut publications: Vec<Instant> = Vec::new();
    let started = Instant::now();
    let window = pass_duration * 2;
    while started.elapsed() < window {
        reports.set_active_slots(3);
        let _diag = reports.latest();
        publications.push(Instant::now());
        tokio::time::sleep(HEARTBEAT).await;
    }
    janitor.stop();

    assert!(
        publications.len() >= 2,
        "the tick must publish repeatedly; got {}",
        publications.len()
    );
    let worst = publications
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .max()
        .expect("at least two publications");
    // The real assertion: the worst gap stays inside the staleness cutoff.
    // Blocking on the pass makes this gap the pass duration, which is ten
    // heartbeats and past the cutoff.
    assert!(
        worst < scaled_stale(),
        "worst publication gap {worst:?} reached the staleness cutoff {:?}; a cleanup pass is \
         delaying capacity publication, which is what makes a healthy builder unselectable",
        scaled_stale()
    );
    assert!(
        worst < pass_duration,
        "worst publication gap {worst:?} is as long as one cleanup pass {pass_duration:?}, so the \
         pass is still on the publication's critical path"
    );
    // And the janitor really was running the whole time, so the test is not
    // passing because nothing happened.
    assert!(
        passes.load(Ordering::Relaxed) >= 1,
        "no cleanup pass completed, so this proves nothing about a pass that blocks"
    );
}

/// `latest()` is the line the tick calls, and it must never wait. This pins it
/// directly rather than only through timing: while a pass is in flight, the
/// call returns immediately — with `None` before the first pass completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_the_latest_report_never_waits_for_a_pass_in_flight() {
    let reports = JanitorReports::new();
    let pass_duration = HEARTBEAT * PASS_HEARTBEATS;
    let janitor = reports.spawn_janitor(HEARTBEAT, move |_| async move {
        tokio::time::sleep(pass_duration).await;
        json!({"outcome": "healthy_noop"})
    });

    // Let a pass get under way, then read while it is definitely still running.
    tokio::time::sleep(HEARTBEAT).await;
    let before = Instant::now();
    let report = reports.latest();
    let waited = before.elapsed();
    janitor.stop();

    assert!(
        waited < HEARTBEAT,
        "reading the latest report waited {waited:?}, so the tick can be blocked by a pass"
    );
    assert!(
        report.is_none(),
        "no pass has completed yet, so there is no report to hand out: {report:?}"
    );
    assert_eq!(reports.completed_passes(), 0);
}

/// A completed pass is what the tick then publishes, and the slot count the
/// tick set must reach it. This is the half that must keep working: decoupling
/// the pass must not stop its report from ever arriving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completed_pass_becomes_the_report_the_tick_publishes() {
    let reports = JanitorReports::new();
    reports.set_active_slots(7);
    let janitor = reports.spawn_janitor(HEARTBEAT, move |active_slots| async move {
        json!({"outcome": "healthy_noop", "active_slots": active_slots})
    });

    let deadline = Instant::now() + scaled_stale() * 4;
    let mut seen = None;
    while Instant::now() < deadline {
        if let Some(report) = reports.latest() {
            seen = Some(report);
            break;
        }
        tokio::time::sleep(HEARTBEAT / 4).await;
    }
    janitor.stop();

    let report = seen.expect("a fast pass must produce a report the tick can read");
    assert_eq!(report["outcome"], json!("healthy_noop"));
    assert_eq!(
        report["active_slots"],
        json!(7),
        "the pass must be told the slot count the tick recorded"
    );
    assert!(reports.completed_passes() >= 1);
}
