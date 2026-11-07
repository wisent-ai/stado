//! `stado host processes` names a child holding memory as the process that
//! leaves the host none before the next reading.
//!
//! One isolated deployment: the real `stado host processes` keeps its first
//! reading, which judges no growth. A child takes half a GiB and says so
//! before the second reading; with the measured interval as the horizon it
//! is named only if that interval's growth really uses up what is
//! available. With the next reading said to be far enough away that the
//! growth of one interval uses up the available memory, a second such child
//! is named first, its growth at least what it took and its time to
//! exhaustion the one the report's own figures give. `--within 0` is
//! refused naming the flag, and the text report carries the `memory:` line.
//! No other program reads the processes: the report comes from the kernel
//! through Stado alone.
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::deployment::Deployment;
use crate::host_processes::{reading, Busy};
use serde_json::{json, Value};

/// What the memory child takes and holds: large enough that no other
/// process on a test host grows as much within one interval.
const HELD_BYTES: u64 = 512 * 1024 * 1024;

/// A child that takes [`HELD_BYTES`] and holds it, started once it says it
/// holds them, ended however the journey ends.
fn hold(deployment: &mut Deployment, key: &str) -> (Busy, u32) {
    let script = format!("$x = 'x' x {HELD_BYTES}; $| = 1; print qq(held\\n); sleep 600");
    let mut child = Command::new("perl")
        .args(["-e", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("perl starts the memory child");
    let stdout = child.stdout.take().expect("the memory child's stdout");
    let holder = Busy(child);
    let pid = holder.0.id();
    deployment.report[key] = json!(pid);
    let mut said = String::new();
    BufReader::new(stdout)
        .read_line(&mut said)
        .expect("the memory child says it holds the memory");
    assert_eq!(said.trim(), "held", "the memory child's first line");
    (holder, pid)
}

/// One journey, so no other memory child of this binary grows in the same
/// interval as the one each reading judges.
#[test]
fn memory_growth_is_judged_against_what_is_left_before_the_next_reading() {
    let mut deployment = Deployment::start("host-processes-memory");
    let host = deployment
        .cli(&["registry", "self", "--name-only"])
        .trim()
        .to_string();

    let first = reading(&mut deployment, &host);
    deployment.report["first_reading"] = first.clone();
    assert_eq!(first["state"], json!("first_reading"), "{first}");
    assert_eq!(
        first["memory"],
        Value::Null,
        "a first reading judges no growth: {first}"
    );

    // With the measured interval as the horizon, half a GiB grown in it
    // leaves this host memory past the next reading, so it is not named
    // unless all growth together uses the memory up.
    let (held, pid) = hold(&mut deployment, "measured_pid");
    let measured = reading(&mut deployment, &host);
    deployment.report["measured_reading"] = measured.clone();
    assert_eq!(measured["state"], json!("compared"), "{measured}");
    let memory = &measured["memory"];
    assert_eq!(
        memory["horizon_source"],
        json!("measured_interval"),
        "{memory}"
    );
    assert_eq!(
        memory["horizon_seconds"], measured["interval_seconds"],
        "{memory}"
    );
    let available = memory["available_bytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("the available memory was read: {memory}"));
    let interval = measured["interval_seconds"].as_f64().expect("an interval");
    let child = measured["processes"]
        .as_array()
        .and_then(|processes| {
            processes
                .iter()
                .find(|process| process["pid"] == json!(pid))
        })
        .unwrap_or_else(|| panic!("the memory child {pid} is not in the reading: {measured}"));
    let grown = child["resident_change_bytes"]
        .as_i64()
        .expect("a resident change");
    assert!(
        u64::try_from(grown).is_ok_and(|grown| grown >= HELD_BYTES),
        "the memory child grew by what it holds: {child}"
    );
    let alone = available.max(0) as f64 * interval / grown as f64;
    assert!(
        alone > interval,
        "half a GiB in one interval leaves this test host memory past the next reading \
         ({available} byte(s) available): {memory}"
    );
    let total = memory["exhausts_available_in_seconds"]
        .as_f64()
        .unwrap_or_else(|| {
            panic!("the child grew, so all growth has a time to exhaustion: {memory}")
        });
    if total > interval {
        assert_eq!(
            memory["processes"],
            json!([]),
            "all growth together leaves memory past the next reading, so nobody is named: {memory}"
        );
    } else {
        let named = memory["processes"].as_array().expect("named processes");
        assert!(
            !named.is_empty(),
            "all growth together uses up the memory before the next reading, so the largest \
             growers are named: {memory}"
        );
    }
    drop(held);

    // With the next reading said to be ten years away, one interval's growth
    // of the largest grower uses up what is available: a second child, the
    // only one growing in this interval, is named first, with the time to
    // exhaustion the report's own figures give.
    let (held, pid) = hold(&mut deployment, "within_pid");
    let within = (10 * 365 * 24 * 3600).to_string();
    let judged: Value = serde_json::from_str(&deployment.cli(&[
        "host",
        "processes",
        &host,
        "--within",
        &within,
        "--json",
    ]))
    .expect("host processes --within answers its JSON report");
    deployment.report["within_reading"] = judged.clone();
    let memory = &judged["memory"];
    assert_eq!(memory["horizon_source"], json!("within"), "{memory}");
    assert_eq!(
        memory["horizon_seconds"].as_f64(),
        within.parse::<f64>().ok(),
        "{memory}"
    );
    let available = memory["available_bytes"]
        .as_i64()
        .unwrap_or_else(|| panic!("the available memory was read: {memory}"));
    let interval = judged["interval_seconds"].as_f64().expect("an interval");
    let first_named = memory["processes"]
        .as_array()
        .and_then(|processes| processes.first())
        .unwrap_or_else(|| panic!("ten years of this interval's growth names nobody: {memory}"));
    assert_eq!(
        first_named["pid"],
        json!(pid),
        "the child grew the most: {memory}"
    );
    let grown = first_named["grown_bytes"].as_u64().expect("grown bytes");
    assert!(
        grown >= HELD_BYTES,
        "it grew by what it holds: {first_named}"
    );
    let expected = available.max(0) as f64 * interval / grown as f64;
    let stated = first_named["exhausts_available_in_seconds"]
        .as_f64()
        .expect("a time to exhaustion");
    assert!(
        (stated - expected).abs() <= expected * 1e-9 + 1e-9,
        "its time to exhaustion is available / (grown / interval): stated {stated}, \
         expected {expected}: {memory}"
    );
    drop(held);

    // --within takes a whole number of seconds above zero: 0 is refused,
    // naming the flag, and nothing is read.
    let (accepted, _, refused) = deployment.run(&["host", "processes", &host, "--within", "0"], "");
    deployment.report["within_zero"] = json!(refused);
    assert!(!accepted, "--within 0 was accepted: {refused}");
    assert!(
        refused.contains("--within"),
        "the refusal names --within: {refused}"
    );

    // The text report carries the memory verdict as its own line.
    let text = deployment.cli(&["host", "processes", &host]);
    deployment.report["text_reading"] = json!(text);
    assert!(
        text.lines().any(|line| line.starts_with("memory: ")),
        "the text report carries the memory line: {text}"
    );
    deployment.pass();
}
