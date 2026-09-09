//! The two reads of this host's disk, and the properties that made scoping
//! them a fix rather than a regression.
//!
//! `stado host gates` consumes three fields — free space, the janitor's state
//! and the local snapshot count — and `stado space report` consumes those
//! three plus the `du` inventory, the clone census and the run-lock holders.
//! The incident was that the gate read computed the second set: it died on the
//! channel's 120-second deadline having produced nothing, so
//! `disk_cleanup_stalled` and `cleanup_success_age_seconds` were unobtainable
//! on the machine the command was about.
//!
//! Both cases here run both commands against this machine. The first proves
//! the gate read answers, and that what it answers changes when the janitor's
//! state on disk changes. The second proves the cheap read cannot disagree
//! with the expensive one about a field they share, and that it really is the
//! cheaper of the two — a scope that still walked `$HOME` would fix nothing.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::fixture::{Host, TARGET};
use crate::native::{df_root, local_snapshot_count, said, KIB};

/// `deploy::host_channel::remote_timeout`, the deadline the gate read died on.
/// The bound below is a fraction of it: this machine answers the gate fields
/// in about a tenth of a second, and the point is that the answer arrives at
/// all rather than that it arrives in any particular tenth.
const CHANNEL_DEADLINE: Duration = Duration::from_secs(120);
const GATE_BOUND: Duration = Duration::from_secs(30);

/// The gate read must be materially cheaper than the full read, not merely
/// different. Measured on this host: the gate fields take about 0.1 s while
/// the full report takes about 2.6 s, so a factor of two is far inside the
/// margin and still fails a gate read that carried the inventory again.
const COST_FACTOR: u32 = 2;

/// `free_gb` is a GiB figure rounded to one decimal and free space moves while
/// the test runs, so the two reads are compared inside a band rather than
/// pinned. The band is far tighter than the difference between a real answer
/// and an invented one.
const DRIFT_GIB: f64 = 4.0;
const GIB: f64 = (1024 * 1024 * 1024) as f64;

/// Local snapshots rotate hourly, so two reads a second apart may legitimately
/// differ by one.
const SNAPSHOT_DRIFT: i64 = 1;

/// The gate read and the test read the clock a moment apart, so the age the
/// product computed and the age measured here agree to within seconds.
const AGE_TOLERANCE_SECONDS: f64 = 3.0;

/// The blocker the incident's host published every tick, and the sentence
/// `host gates` leaves with when a host is claiming nothing. Both copied from
/// live runs of the built binary against this fixture.
const STALLED_BLOCKER: &str = "disk_cleanup_stalled";
const CLAIMING_NOTHING: &str = "is claiming nothing";
/// `CmdError::click` — `host gates` reports a host that claims nothing as a
/// finding about the fleet, so a passing exit code is not the signal here.
const CLICK_EXIT: i32 = 1;

/// The gate read answers on the host it is about, and its answer follows the
/// janitor's state on disk.
///
/// Before any pass there is no state document, so the janitor is stalled and
/// the success age is unreadable — which is exactly what the incident's
/// operator could not find out. After one real pass writes that document, the
/// same command reports an age, drops the blocker, and does so well inside the
/// deadline it used to die on.
#[test]
fn the_gate_read_answers_on_the_host_it_is_about() {
    let host = Host::new();
    host.seed_tree(
        &host.cache_root,
        "target-tree",
        crate::CACHE_MIB,
        true,
        true,
    );

    let (before, silent_elapsed) = gates(&host);
    assert_eq!(before["disk"]["cleanup_stalled"], true);
    assert!(
        before["disk"]["cleanup_success_age_seconds"].is_null(),
        "a host whose janitor never completed a pass cannot report an age: {}",
        before["disk"]
    );
    assert!(
        blockers(&before).contains(&STALLED_BLOCKER.to_string()),
        "the stalled janitor was not published as a blocker: {before}"
    );
    assert!(
        silent_elapsed < GATE_BOUND,
        "the gate read took {silent_elapsed:?} of its {CHANNEL_DEADLINE:?} deadline"
    );
    assert!(
        host.janitor_state().is_none(),
        "reading the gates wrote a janitor state document"
    );

    host.cleanup_pass(&["disk-cleanup", "--once"]);
    let state = host
        .janitor_state()
        .expect("the pass wrote its state document");
    let attempted = state["last_attempt_at"]
        .as_f64()
        .expect("the state document records when the pass ran");

    let (after, answered_elapsed) = gates(&host);
    assert_eq!(
        after["disk"]["cleanup_stalled"], false,
        "the janitor completed a pass on this host and the gate still reads stalled: {}",
        after["disk"]
    );
    let age = after["disk"]["cleanup_success_age_seconds"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "the gate read no success age after a pass: {}",
                after["disk"]
            )
        });
    let measured = seconds_since(attempted);
    assert!(
        (age - measured).abs() <= AGE_TOLERANCE_SECONDS,
        "the gate reports the janitor succeeded {age}s ago; the state document \
         on disk was written {measured}s ago"
    );
    assert!(
        !blockers(&after).contains(&STALLED_BLOCKER.to_string()),
        "the blocker survived the pass that resolved it: {after}"
    );
    assert!(
        answered_elapsed < GATE_BOUND,
        "the gate read took {answered_elapsed:?} of its {CHANNEL_DEADLINE:?} deadline"
    );
}

/// The cheap read and the expensive one cannot disagree about a field they
/// share, and the cheap one really is cheaper.
///
/// Free space, the local snapshot count, the declared watermarks and the
/// janitor's own success age are read by both commands; each is compared
/// across the two and against what this machine itself says. The full report
/// additionally carries the inventory that the gate read has no field for,
/// which is the work that was being done for nobody — so the same case
/// measures both reads and requires the gate one to cost a fraction.
#[test]
fn the_two_reads_agree_on_what_they_share_and_only_one_pays_for_the_rest() {
    let host = Host::new();
    host.seed_tree(
        &host.cache_root,
        "target-tree",
        crate::CACHE_MIB,
        true,
        true,
    );
    host.cleanup_pass(&["disk-cleanup", "--once"]);

    let (gate, gate_elapsed) = gates(&host);
    let full_started = Instant::now();
    let full = host.json(&["space", "report", TARGET, "--json"]);
    let full_elapsed = full_started.elapsed();

    // Free space: both reads against the filesystem's own answer.
    let gate_free = gate["disk"]["free_gb"]
        .as_f64()
        .expect("the gate read reports free space");
    let full_free = full["free_space"]["available_bytes"]
        .as_f64()
        .expect("the full report reports free space")
        / GIB;
    let native_free = (df_root().available_kb as f64) * (KIB as f64) / GIB;
    assert!(
        (gate_free - full_free).abs() < DRIFT_GIB,
        "the gate read reports {gate_free} GiB free and the full report {full_free} GiB"
    );
    assert!(
        (gate_free - native_free).abs() < DRIFT_GIB,
        "the gate read reports {gate_free} GiB free; df says {native_free} GiB"
    );
    assert!(
        gate_free > 0.0 && gate_free < (df_root().blocks_kb as f64) * (KIB as f64) / GIB,
        "{gate_free} GiB is not a figure this filesystem could report"
    );

    // The declared policy both reads carry.
    assert_eq!(gate["disk"]["policy_mode"], full["policy"]["mode"]);
    assert_eq!(
        gate["disk"]["low_watermark_gb"],
        full["policy"]["low_free_gb"]
    );
    assert_eq!(
        gate["disk"]["target_free_gb"],
        full["policy"]["target_free_gb"]
    );

    // The snapshots whose blocks sit inside `used` and which nothing here
    // reclaims: the same count in both reads, and the machine's own count.
    let gate_snapshots = gate["disk"]["local_snapshots"]
        .as_i64()
        .expect("the gate read counts local snapshots");
    let full_snapshots = full["local_snapshots"]["count"]
        .as_i64()
        .expect("the full report counts local snapshots");
    assert_eq!(gate_snapshots, full_snapshots);
    assert!(
        (gate_snapshots - local_snapshot_count()).abs() <= SNAPSHOT_DRIFT,
        "both reads report {gate_snapshots} snapshots; tmutil holds {}",
        local_snapshot_count()
    );

    // The janitor state: an age in the gate read, the timestamp it was
    // computed from in the full one.
    assert_eq!(full["cleanup_state"]["present"], true);
    assert!(
        gate["disk"]["cleanup_success_age_seconds"]
            .as_f64()
            .is_some(),
        "the gate read lost the state field after a real pass: {}",
        gate["disk"]
    );

    // What only the full report pays for, and what that costs.
    let inventory = full["inventory"]
        .as_array()
        .expect("the full report carries an inventory");
    assert!(
        !inventory.is_empty(),
        "the full report measured no occupants at all: {full}"
    );
    assert!(
        inventory
            .iter()
            .all(|item| std::path::Path::new(item["path"].as_str().unwrap_or_default()).is_dir()),
        "the inventory names a path that is not a directory: {inventory:?}"
    );
    assert!(
        gate["disk"].get("inventory").is_none(),
        "the gate read grew a field for the work it does not consume: {}",
        gate["disk"]
    );
    assert!(
        gate_elapsed * COST_FACTOR < full_elapsed,
        "the gate read took {gate_elapsed:?} against the full read's \
         {full_elapsed:?}: the cheap scope is not dropping the expensive work"
    );
}

/// Run `host gates` and return its JSON with the wall time the command took.
///
/// The command reports a host that claims nothing through `CmdError::click`,
/// so the exit code is asserted here rather than treated as a failure: this
/// fixture declares no agent, so `no_capacity_publication` alone guarantees
/// it.
fn gates(host: &Host) -> (Value, Duration) {
    let started = Instant::now();
    let output = host.run(&["host", "gates", TARGET, "--json"]);
    let elapsed = started.elapsed();
    assert_eq!(
        output.status.code(),
        Some(CLICK_EXIT),
        "host gates exited {:?}\nstderr:\n{}",
        output.status.code(),
        said(&output.stderr)
    );
    let stderr = said(&output.stderr);
    assert!(
        stderr.contains(CLAIMING_NOTHING),
        "the gate read did not say why the host claims nothing:\n{stderr}"
    );
    let gates = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "host gates did not print one JSON document: {error}\n{}",
            said(&output.stdout)
        )
    });
    (gates, elapsed)
}

fn blockers(gates: &Value) -> Vec<String> {
    gates["blockers"]
        .as_array()
        .expect("the gate read publishes its blockers")
        .iter()
        .map(|blocker| blocker.as_str().unwrap_or_default().to_string())
        .collect()
}

fn seconds_since(unix_seconds: f64) -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("this machine's clock is after the epoch")
        .as_secs_f64()
        - unix_seconds
}
