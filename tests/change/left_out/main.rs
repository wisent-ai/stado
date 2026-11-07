//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test change-left-out -- --nocapture`.
//!
//! A gate the operator's rulings rule out is not run by the submit queue, and
//! the record says so: the gate beside the source check with the ruling that
//! left it out, printed by `stado change status` as not run, the risk
//! classified from the gates that ran, so the change waits for a review, and
//! once a reviewer's round admits it, a landing whose integration line says
//! its gates did not all run. The real `stado` over the isolated world of
//! [`journey`], with the installed Oko answering from a no-test ruling.

mod journey;

use journey::{Journey, DEFECT, GATE, PLATFORM, RULING, SAID};
use serde_json::{json, Value};
use std::fs;

#[test]
fn a_gate_the_rulings_leave_out_is_recorded_as_not_run() {
    let mut journey = Journey::start();
    journey.repository();
    let change = journey.commit("notes.txt", "one\n", "a change the queue carries");
    let checkout = journey.checkout.to_string_lossy().into_owned();
    let submit: [&str; 8] = [
        "change", "submit", &checkout, "--commit", &change, "--defect", DEFECT, "--json",
    ];
    let (submitted, stdout, said) = journey.stado(&submit);
    assert!(submitted, "submit: {said}");
    let submitted: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("submit answers JSON ({error}): {said}"));
    let id = submitted["candidates"][0]["id"]
        .as_str()
        .expect("the candidate's id")
        .to_string();

    // The first pass runs the gates and stops at the review the reclassified
    // risk asks for; the record is written once the gates ended.
    let (_, _, said) = journey.stado(&["change", "run", &checkout]);
    journey.report["first_pass"] = json!(said);
    let record = journey.candidate(&id);
    let shards = record["shards"].as_array().cloned().unwrap_or_default();
    let left: Vec<&Value> = shards
        .iter()
        .filter(|shard| shard["gate"] == GATE)
        .collect();
    assert_eq!(
        left.len(),
        1,
        "the test gate is recorded once beside the source check: {record}"
    );
    let shard = left[0];
    assert_eq!(shard["kind"], "test", "{shard}");
    assert_eq!(shard["platform"], PLATFORM, "{shard}");
    assert_ne!(
        shard["exit_code"], 0,
        "a gate that never ran does not read as passed: {shard}"
    );
    let ruling = shard["left_out"]
        .as_str()
        .unwrap_or_else(|| panic!("the record says why: {shard}"));
    assert!(
        ruling.contains(RULING) && ruling.contains(SAID),
        "the record keeps the ruling that left the gate out, by id and words: {ruling}"
    );
    assert!(
        shards
            .iter()
            .any(|shard| shard["kind"] == "source" && shard["left_out"].is_null()),
        "the source check ran and is not marked left out: {record}"
    );
    assert_eq!(
        record["risk"]["level"], "high",
        "the risk is classified from the gates that ran: {record}"
    );
    let reasons = record["risk"]["reasons"].to_string();
    assert!(
        reasons.contains("left out by the operator's rulings"),
        "the reason says why: {reasons}"
    );
    assert!(
        said.contains("risk from the gates that ran"),
        "the pass prints the new risk: {said}"
    );

    let (shown, status, said) = journey.stado(&["change", "status", &id]);
    assert!(shown, "status: {said}");
    let named = format!("{GATE} (test) for {PLATFORM}: not run, left out: ");
    let line = format!("gate {named}");
    let printed = status.lines().find(|printed| printed.contains(&line));
    assert!(
        printed.is_some_and(|printed| printed.contains(RULING) && printed.contains(SAID)),
        "status prints the gate as not run with the ruling that left it out: {status}"
    );
    assert!(
        status.contains("so the reviewer judges whether it compiles by reading it"),
        "the review wait tells the reviewer no gate compiled it: {status}"
    );

    // A reviewer that is not the author admits it, and the next pass lands it.
    let none = journey.root.join("none.json");
    fs::write(&none, "[]").unwrap();
    let none = none.to_string_lossy().into_owned();
    let review: [&str; 7] = [
        "change",
        "review",
        &id,
        "--findings",
        &none,
        "--improvements",
        &none,
    ];
    let (reviewed, _, said) = journey.stado_as(Some("left-out-reviewer"), &review);
    assert!(reviewed, "review: {said}");
    let (_, _, said) = journey.stado(&["change", "run", &checkout]);
    journey.report["second_pass"] = json!(said);
    let record = journey.candidate(&id);
    assert_eq!(
        record["state"], "landed",
        "the reviewed candidate lands: {record}"
    );
    let landed = record["landed"]["commit"]
        .as_str()
        .expect("the landed commit")
        .to_string();
    assert_eq!(
        journey.origin_main(),
        landed,
        "origin's main is the landed commit"
    );
    let integrated = &record["integrated"][0];
    assert_eq!(
        integrated["gates"], "not-run",
        "a landing with a gate left out says so: {integrated}"
    );
    let evidence = integrated["evidence"].as_str().unwrap_or_default();
    assert!(
        evidence.contains(&named) && evidence.contains(RULING) && evidence.contains(SAID),
        "the integration evidence names the gate left out and its ruling: {evidence}"
    );
    let (listed, list, said) = journey.stado(&["change", "list", &checkout]);
    assert!(listed, "list: {said}");
    let note =
        format!("; gates left out: {GATE} (test) for {PLATFORM}, see `stado change status {id}`");
    assert!(
        list.lines()
            .any(|row| row.starts_with(&id) && row.contains(&note)),
        "the landed candidate's list line names the gate left out: {list}"
    );

    // A small change submitted without --defect is low risk at submit; with
    // its gate left out it is reviewed before landing, and waits for a
    // recorded round (which needs no defect) instead of being refused.
    let small = journey.commit("notes.txt", "two\n", "a change with no defect");
    let bare: [&str; 6] = ["change", "submit", &checkout, "--commit", &small, "--json"];
    let (submitted, stdout, said) = journey.stado(&bare);
    assert!(submitted, "submit without --defect: {said}");
    let bare_id = serde_json::from_str::<Value>(&stdout).expect("submit answers JSON")
        ["candidates"][0]["id"]
        .as_str()
        .expect("the candidate's id")
        .to_string();
    let (_, _, said) = journey.stado(&["change", "run", &checkout]);
    let waiting = journey.candidate(&bare_id);
    assert_eq!(
        waiting["state"], "queued",
        "it is held, not refused: {said}"
    );
    assert_eq!(waiting["waits_on"], "review-round", "{waiting}");
    let held = waiting["refusal"].as_str().unwrap_or_default();
    assert!(
        held.contains("no lint or test gate ran on it"),
        "the hold names the gates that never ran: {held}"
    );
    let (_, list, said) = journey.stado(&["change", "list", &checkout]);
    let bare_note = format!(
        "; gates left out: {GATE} (test) for {PLATFORM}, see `stado change status {bare_id}`"
    );
    assert!(
        list.lines()
            .any(|row| row.starts_with(&bare_id) && row.ends_with(&bare_note)),
        "the held candidate's list line ends with the note: {list}{said}"
    );

    journey.report["record"] = record;
    journey.report["outcome"] = json!("passed");
    journey.save();
    // The report stays as the run's evidence; the repositories and home go.
    for made in ["checkout", "remote", "home"] {
        fs::remove_dir_all(journey.root.join(made)).unwrap();
    }
    eprintln!(
        "change-left-out evidence: {}",
        journey.root.join("report.json").display()
    );
}
