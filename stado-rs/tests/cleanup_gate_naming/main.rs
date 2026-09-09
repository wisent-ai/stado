//! A gate that refuses work has to name the condition it refused for, and the
//! name has to reach the operator who is going to act on it.
//!
//! # What happened
//!
//! On 2026-09-03 `disk_cleanup_stalled` closed every `darwin-arm64` builder in
//! the registry at once: `charless-mac-mini` at 18.4 GiB free against a 15 GiB
//! low watermark with eight jobs pinned to it, and `lukasz-macbook` at 118.7
//! GiB free against 100 with one job that had waited a day. Neither disk was
//! under any pressure. Both janitors were stale for one reason — a shared
//! workload hold that had outlived its workload made every exclusive acquire
//! fail, so no pass ever scanned — and the word the operator was handed sent
//! them to a disk that was fine.
//!
//! # What is defended here, and through what
//!
//! `stado host gates HOST` is where that word reaches an operator: as a field
//! in the `--json` payload, as a `note:` or `blockers:` line in the console
//! output, and inside the sentence the command exits non-zero with. So every
//! case here declares this machine in an isolated registry, writes the
//! janitor's own state file into an isolated `HOME`, runs the built binary
//! against this disk, and reads the words back out of what it printed.
//!
//! Three separable properties, all of them read off the product's output:
//!
//! 1. A janitor that cannot get the run lock is reported as
//!    `disk_cleanup_lock_held` and NOT as `disk_cleanup_stalled`. The two name
//!    different remedies: one is a process holding a file, the other is a
//!    janitor that ran and got nowhere.
//! 2. Neither condition closes a host that measurably has its headroom.
//!    Refusing work there frees no byte and it took a platform offline.
//! 3. Under pressure both still refuse, and the refusal sentence names which
//!    one. That is the risk the gate exists for and it is not weakened here.
//!
//! The free space in the comparison is this machine's own `df` answer. The
//! watermarks are declared far enough either side of it that the case is about
//! the janitor and never about how full this disk happens to be today.

mod fixture;

use crate::fixture::{
    refusal, stdout, words, Fixture, Headroom, LOCK_HELD, PRESSURE, PREVENTED_SECONDS, STALLED,
};

/// A healthy janitor on a host with headroom is named by no cleanup condition
/// at all. Without this every refusal below could be the fixture reporting a
/// condition it always reports.
#[test]
fn a_healthy_janitor_with_headroom_is_named_by_no_cleanup_condition() {
    let fixture = Fixture::declaring(Headroom::Above);
    fixture.record_healthy_janitor();

    let (report, _) = fixture.gates_report();

    assert_eq!(report["disk"]["cleanup_stalled"], serde_json::json!(false));
    assert_eq!(
        report["disk"]["cleanup_lock_held"],
        serde_json::json!(false)
    );
    for key in ["blockers", "notes"] {
        let listed = words(&report, key);
        assert!(
            !listed
                .iter()
                .any(|word| word == STALLED || word == LOCK_HELD),
            "a fresh pass on a host with headroom is no cleanup condition; {key}: {listed:?}"
        );
    }
}

/// Property 1. Before the split the wedge produced either the disk-shaped word
/// — which points an operator at a disk that is fine — or, once prevented
/// passes began recording themselves, nothing at all.
#[test]
fn a_held_run_lock_is_named_as_a_held_lock_and_not_as_a_stalled_janitor() {
    let fixture = Fixture::declaring(Headroom::Above);
    fixture.record_wedged_janitor();

    let (report, _) = fixture.gates_report();

    assert_eq!(
        report["disk"]["cleanup_lock_held"],
        serde_json::json!(true),
        "a janitor refused the lock with no success inside the window is a held lock: {report:#}"
    );
    assert_eq!(
        report["disk"]["cleanup_stalled"],
        serde_json::json!(false),
        "a held lock must not also be reported as a stalled janitor: one remedy each"
    );
    let prevented = report["disk"]["cleanup_prevented_age_seconds"]
        .as_i64()
        .expect("the number behind the word travels with it");
    assert!(
        (PREVENTED_SECONDS..PREVENTED_SECONDS + 60).contains(&prevented),
        "the reported age is the age of the recorded prevented pass: {prevented}"
    );
    let notes = words(&report, "notes");
    assert!(
        notes.iter().any(|note| note == LOCK_HELD),
        "the condition is reported: {notes:?}"
    );
    assert!(
        !notes.iter().any(|note| note == STALLED),
        "the disk-shaped word must not appear for a lock-shaped fault: {notes:?}"
    );
}

/// The same wedge in the console output an operator actually reads, because a
/// field in a payload nobody prints is not a word anybody is handed.
#[test]
fn the_console_hands_the_operator_the_lock_shaped_word() {
    let fixture = Fixture::declaring(Headroom::Above);
    fixture.record_wedged_janitor();

    let output = fixture.gates_lines();

    let lines = stdout(&output);
    assert!(
        lines
            .lines()
            .any(|line| line == format!("note:     {LOCK_HELD}")),
        "the console names the condition on its own line: {lines}"
    );
    assert!(!lines.contains(STALLED), "and never the other one: {lines}");
}

/// A silent janitor keeps the word it already had. The split must not have
/// renamed the condition it was right about.
#[test]
fn a_silent_janitor_is_still_named_as_stalled() {
    let fixture = Fixture::declaring(Headroom::Above);
    fixture.record_silent_janitor();

    let (report, _) = fixture.gates_report();

    assert_eq!(report["disk"]["cleanup_stalled"], serde_json::json!(true));
    assert_eq!(
        report["disk"]["cleanup_lock_held"],
        serde_json::json!(false),
        "nothing recorded a prevented pass, so nothing is holding the lock"
    );
    assert!(
        words(&report, "notes").iter().any(|note| note == STALLED),
        "the condition is reported: {report:#}"
    );
}

/// Property 2: measured headroom decides. Both hosts that went offline had
/// theirs, and a gate that refuses work above its own watermark frees no byte
/// — it only removes the platform's last builder.
#[test]
fn neither_condition_closes_a_host_that_has_its_headroom() {
    for label in ["wedged", "silent"] {
        let fixture = Fixture::declaring(Headroom::Above);
        if label == "wedged" {
            fixture.record_wedged_janitor();
        } else {
            fixture.record_silent_janitor();
        }

        let (report, output) = fixture.gates_report();

        let blockers = words(&report, "blockers");
        assert!(
            !blockers.iter().any(|word| word == PRESSURE),
            "{label}: this disk is above the declared watermark: {report:#}"
        );
        assert!(
            !blockers
                .iter()
                .any(|word| word == STALLED || word == LOCK_HELD),
            "{label}: cleanup must not block a host with headroom: {blockers:?}"
        );
        let sentence = refusal(&output);
        assert!(
            !sentence.contains(STALLED) && !sentence.contains(LOCK_HELD),
            "{label}: and the sentence the operator is handed says so too: {sentence}"
        );
    }
}

/// Property 3: the safety property is untouched. Below the watermark a janitor
/// that cannot run must still refuse work — nothing is bringing the space
/// back, and admitting a job onto an unmanaged disk is the incident this gate
/// was written for. The word in the refusal is the one that names the remedy.
#[test]
fn under_pressure_a_janitor_that_cannot_run_still_refuses_work() {
    for (label, expected) in [("wedged", LOCK_HELD), ("silent", STALLED)] {
        let fixture = Fixture::declaring(Headroom::Below);
        if label == "wedged" {
            fixture.record_wedged_janitor();
        } else {
            fixture.record_silent_janitor();
        }

        let (report, output) = fixture.gates_report();

        assert_eq!(
            report["claiming"],
            serde_json::json!(false),
            "{label}: a host under pressure with a janitor that cannot run must not claim"
        );
        let blockers = words(&report, "blockers");
        assert!(
            blockers.iter().any(|word| word == PRESSURE),
            "{label}: this disk is below the declared watermark: {report:#}"
        );
        assert!(
            blockers.iter().any(|word| word == expected),
            "{label}: expected {expected} in {blockers:?}"
        );
        assert!(
            !output.status.success(),
            "{label}: a host that is claiming nothing is a failed verdict"
        );
        assert!(
            refusal(&output).contains(expected),
            "{label}: the refusal names the condition: {}",
            refusal(&output)
        );
    }
}
