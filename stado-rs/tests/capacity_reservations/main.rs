//! Capacity reservations, end to end through the real binary: the agent
//! publishes into an isolated store, a real `stado capacity hold` takes a
//! declared workload's reservation on the host, and the store and the
//! publication are read back at every step.
//!
//! Before this primitive a Jeden session or a browser task placed by Stado
//! held nothing, so a host kept publishing itself as free while it ran any
//! number of them. These stories are the proof that a hold is written, that
//! the agent subtracts it, that the CLI lists it, that it is released, that
//! a full host refuses with the documented sentence and records the refusal,
//! and that a hold whose holder stopped answering is retired.

mod support;

use std::fs;
use std::time::Duration;

use serde_json::Value;

use support::{Journey, TARGET};

/// The smallest declared kind: one core, one GiB, no VRAM.
const SMALL_KIND: &str = "weles-activity";
/// How long the story's hold lasts; long enough for two agent polls.
const HOLD_SECONDS: u64 = 30;
/// How long to wait for the agent to publish after a change.
const PUBLISH_WAIT: Duration = Duration::from_secs(45);

fn json_stdout(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|error| {
        panic!(
            "expected one JSON document: {error}\n{}",
            String::from_utf8_lossy(bytes)
        )
    })
}

#[test]
fn a_hold_is_written_subtracted_listed_and_released() {
    let mut journey = Journey::new();
    journey.start_agent();
    // The agent's very first broadcast is a keep-alive before it has
    // measured anything; the story needs the measured one with room in it.
    journey.wait_for("a capacity publication with room", PUBLISH_WAIT, |j| {
        j.newest_capacity().is_some_and(|capacity| {
            capacity["accepting_jobs"] == true
                && capacity["available_cpu_cores"].as_i64().unwrap_or(0) >= 1
        })
    });

    let mut hold = journey.start_hold(SMALL_KIND, HOLD_SECONDS);
    journey.wait_for("the reservation document", PUBLISH_WAIT, |j| {
        !j.reservation_files().is_empty()
    });
    let file = &journey.reservation_files()[0];
    let written: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
    assert_eq!(written["kind"], SMALL_KIND, "{written}");
    assert_eq!(written["target"], TARGET, "{written}");
    let held_cores = written["cpu_cores"].as_i64().unwrap();
    assert!(held_cores >= 1, "the declaration reserves at least one core: {written}");

    journey.wait_for("a publication net of the hold", PUBLISH_WAIT, |j| {
        j.newest_capacity()
            .is_some_and(|capacity| capacity["running_workloads"] == 1)
    });
    let capacity = journey.newest_capacity().unwrap();
    assert_eq!(capacity["reserved"]["cpu_cores"], written["cpu_cores"], "{capacity}");
    assert_eq!(capacity["reserved"]["ram_gb"], written["ram_gb"], "{capacity}");
    let measured = capacity["diag"]["measured_available_cpu_cores"]
        .as_i64()
        .unwrap_or_else(|| panic!("the publication keeps the measured cores: {capacity}"));
    let net = capacity["available_cpu_cores"].as_i64().unwrap();
    assert_eq!(
        net,
        (measured - held_cores).max(0),
        "available cores are the measured cores less the hold: {capacity}"
    );
    assert_eq!(
        capacity["reservations"][0]["reservation_id"], written["reservation_id"],
        "{capacity}"
    );

    let listed = json_stdout(
        &journey
            .invoke(&["capacity", "reservations", TARGET, "--json"])
            .stdout,
    );
    let rows = listed["reservations"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["reservation_id"], written["reservation_id"]);
    assert_eq!(rows[0]["live"], true, "{listed}");

    let table = json_stdout(&journey.invoke(&["capacity", "list", "--json"]).stdout);
    let host = table["hosts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|host| host["target"] == TARGET)
        .unwrap_or_else(|| panic!("the fleet table names the target: {table}"));
    assert_eq!(host["running_workloads"], 1, "{host}");
    assert_eq!(host["reserved"]["cpu_cores"], written["cpu_cores"], "{host}");

    let status = hold.wait().unwrap();
    assert!(
        status.success(),
        "the hold ended with {status}: {}",
        fs::read_to_string(journey.home.join("hold.err")).unwrap_or_default()
    );
    assert!(
        journey.reservation_files().is_empty(),
        "the released reservation is still in the store"
    );
    journey.wait_for("a publication with nothing held", PUBLISH_WAIT, |j| {
        j.newest_capacity()
            .is_some_and(|capacity| capacity["running_workloads"] == 0)
    });
}

#[test]
fn a_full_host_refuses_with_its_sentence_and_records_the_refusal() {
    let journey = Journey::new();
    journey.publish_full_host();

    let output = journey.invoke(&[
        "capacity",
        "hold",
        "--kind",
        "jeden-session",
        "--target",
        TARGET,
        "--seconds",
        "1",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "a full host accepted a hold: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{TARGET} has no room for jeden-session: needs 2 cores and 4 GiB, host publishes 0 cores and 20.0 GiB net of 0 reservation(s); pick another host with --target or wait for a reservation to end"
        )),
        "the refusal sentence is the documented one: {stderr}"
    );
    assert!(
        journey.reservation_files().is_empty(),
        "a refused hold wrote a reservation"
    );
    let unmet = journey.unmet_files();
    assert_eq!(unmet.len(), 1, "the refusal is recorded once: {unmet:?}");
    let record: Value = serde_json::from_slice(&fs::read(&unmet[0]).unwrap()).unwrap();
    assert_eq!(record["kind"], "jeden-session", "{record}");
    assert_eq!(record["reason"], "capacity_exhausted", "{record}");
    assert_eq!(record["candidates"][0]["target"], TARGET, "{record}");
}

#[test]
fn a_reservation_whose_holder_stopped_answering_is_retired() {
    let mut journey = Journey::new();
    let path = journey.seed_dead_reservation(SMALL_KIND);

    journey.start_agent();
    journey.wait_for("the agent's publication", PUBLISH_WAIT, |j| {
        j.newest_capacity().is_some()
    });
    let capacity = journey.newest_capacity().unwrap();
    assert_eq!(
        capacity["running_workloads"], 0,
        "an expired reservation was counted: {capacity}"
    );
    assert!(
        !path.exists(),
        "the expired reservation was not retired by the agent"
    );
}

/// The publication says who holds the accelerator, and `space report`
/// repeats it from the same publication. On Apple silicon that is the
/// unified-memory answer; on a discrete-GPU host it is the driver's
/// per-process list with the memory nobody listed accounts for.
#[test]
fn the_publication_names_who_holds_the_accelerator() {
    let mut journey = Journey::new();
    journey.start_agent();
    journey.wait_for("a measured publication", PUBLISH_WAIT, |j| {
        j.newest_capacity()
            .is_some_and(|capacity| capacity["diag"]["accelerator_memory_model"].is_string())
    });
    let capacity = journey.newest_capacity().unwrap();
    let model = capacity["diag"]["accelerator_memory_model"].as_str().unwrap();
    let expected_model = if cfg!(target_os = "macos") {
        "unified"
    } else {
        "discrete"
    };
    assert_eq!(model, expected_model, "{capacity}");
    assert!(
        capacity["diag"]["accelerator_holders"].is_array(),
        "the holders list is published even when empty: {capacity}"
    );

    let output = journey.invoke(&["space", "report", TARGET, "--json"]);
    let report = json_stdout(&output.stdout);
    assert_eq!(report["accelerators"]["memory_model"], expected_model, "{}", report["accelerators"]);
    let line = report["accelerators"]["line"]
        .as_str()
        .unwrap_or_else(|| panic!("the report carries the holders sentence: {}", report["accelerators"]));
    if cfg!(target_os = "macos") {
        assert_eq!(line, "accelerator shares the host's memory; no per-process VRAM");
    } else {
        assert!(line.starts_with("held by") || line.starts_with("no process holds"), "{line}");
    }
    let text = String::from_utf8_lossy(&journey.invoke(&["space", "report", TARGET]).stdout).into_owned();
    assert!(text.contains(&format!("accelerators: {line}")), "{text}");
}
