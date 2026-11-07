//! `stado host processes` names a known busy child with the CPU it spent,
//! and measures a process that lives through two readings from the reading
//! before, not from its start.
//!
//! One isolated deployment: the real `stado host processes` on this host
//! keeps its first reading and says so; a child that does nothing but spend
//! CPU is started; the second reading must list that child by pid, with its
//! program and a CPU figure above zero, as a process started in the interval.
//! A third reading, the child still running, must list it as the same
//! process: not started in the interval, not ended, and with no more CPU than
//! one thread can spend in the time between the readings. A second child
//! that spent CPU and then sleeps through both readings is neither listed
//! nor ended in the third: a conversion of its counters that moved between
//! readings would make it one or the other. This test's own process, alive
//! through every reading, is never counted as ended or new. A reading kept
//! by a build that did not record how it converted CPU is not compared: the
//! next run is a first reading naming why, and the one after compares.
//! No other program reads the processes: the report comes from the kernel
//! through Stado alone.
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use crate::deployment::Deployment;
use serde_json::{json, Value};

/// The busy child, ended however the test ends.
struct Busy(Child);

impl Drop for Busy {
    fn drop(&mut self) {
        if let Err(error) = self.0.kill() {
            eprintln!("the busy child {} could not be ended: {error}", self.0.id());
        }
        if let Err(error) = self.0.wait() {
            eprintln!(
                "the busy child {} could not be reaped: {error}",
                self.0.id()
            );
        }
    }
}

fn reading(deployment: &mut Deployment, host: &str) -> Value {
    serde_json::from_str(&deployment.cli(&["host", "processes", host, "--json"]))
        .expect("host processes answers its JSON report")
}

#[test]
fn a_busy_child_is_named_with_the_cpu_it_spent() {
    let mut deployment = Deployment::start("host-processes");
    let host = deployment
        .cli(&["registry", "self", "--name-only"])
        .trim()
        .to_string();

    let first = reading(&mut deployment, &host);
    deployment.report["first_reading"] = first.clone();
    assert_eq!(first["state"], json!("first_reading"), "{first}");

    let busy = Busy(
        Command::new("perl")
            .args(["-e", "1 while 1"])
            .stdin(Stdio::null())
            .spawn()
            .expect("perl starts the busy child"),
    );
    let pid = busy.0.id();
    deployment.report["busy_pid"] = json!(pid);
    // A child that spends CPU, says so, then sleeps through the second and
    // third readings.
    let mut idle = Command::new("perl")
        .args([
            "-e",
            "$| = 1; $i++ while $i < 5e7; print qq(idle\\n); sleep 1e6",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("perl starts the idle child");
    let stdout = idle.stdout.take().expect("the idle child's stdout");
    let idle = Busy(idle);
    let idle_pid = idle.0.id();
    deployment.report["idle_pid"] = json!(idle_pid);
    let mut said = String::new();
    BufReader::new(stdout)
        .read_line(&mut said)
        .expect("the idle child says it went idle");
    assert_eq!(said.trim(), "idle", "the idle child's first line");

    let second = reading(&mut deployment, &host);
    deployment.report["second_reading"] = second.clone();
    assert_eq!(second["state"], json!("compared"), "{second}");
    let Some(child) = second["processes"].as_array().and_then(|processes| {
        processes
            .iter()
            .find(|process| process["pid"] == json!(pid))
    }) else {
        panic!("the busy child {pid} is not in the reading: {second}");
    };
    assert_eq!(child["started_in_interval"], json!(true), "{child}");
    let spent = child["cpu_seconds"].as_f64().expect("a CPU figure");
    assert!(
        spent > f64::default(),
        "the busy child spent no CPU: {child}"
    );
    let program = child["program"].as_str().expect("a program");
    assert!(
        program.contains("perl"),
        "the child is named by its program: {child}"
    );

    // The child lives through the second and third readings: on a Mac its
    // CPU counters are converted by one fixed timebase, so its converted
    // total never goes down and it is the same process in both. The reading
    // spends part of the command's own wall time listing every process, so
    // the child's interval CPU is bounded by the interval plus that time.
    let asked = Instant::now();
    let third = reading(&mut deployment, &host);
    let command_seconds = asked.elapsed().as_secs_f64();
    deployment.report["third_reading"] = third.clone();
    assert_eq!(third["state"], json!("compared"), "{third}");
    let interval = third["interval_seconds"].as_f64().expect("an interval");
    let ended = |pid: u32| {
        third["ended"]
            .as_array()
            .is_some_and(|ended| ended.iter().any(|process| process["pid"] == json!(pid)))
    };
    let listed = |pid: u32| {
        third["processes"].as_array().and_then(|processes| {
            processes
                .iter()
                .find(|process| process["pid"] == json!(pid))
                .cloned()
        })
    };
    assert!(
        !ended(pid),
        "the busy child is still running, not ended: {third}"
    );
    let Some(child) = listed(pid) else {
        panic!("the busy child {pid} spent CPU but is not in the third reading: {third}");
    };
    assert_eq!(
        child["started_in_interval"],
        json!(false),
        "the busy child was read before, so it is the same process: {child}"
    );
    let spent = child["cpu_seconds"].as_f64().expect("a CPU figure");
    assert!(
        spent <= interval + command_seconds,
        "one busy thread spent {spent} s in an interval of {interval} s (the reading took up to \
         {command_seconds} s): its lifetime was counted again: {child}"
    );
    let own = std::process::id();
    assert!(
        !ended(own),
        "this test's own process is alive, not ended: {third}"
    );
    if let Some(own) = listed(own) {
        assert_eq!(
            own["started_in_interval"],
            json!(false),
            "this test's own process was read before: {own}"
        );
    }
    assert!(
        !ended(idle_pid),
        "the idle child is asleep, not ended: {third}"
    );
    assert!(
        listed(idle_pid).is_none(),
        "the idle child spent nothing between the second and third readings, so it is not \
         listed: {third}"
    );
    drop(idle);
    drop(busy);
    deployment.pass();
}

#[test]
fn a_reading_kept_by_an_older_build_is_not_compared() {
    let mut deployment = Deployment::start("host-processes-kept");
    let host = deployment
        .cli(&["registry", "self", "--name-only"])
        .trim()
        .to_string();
    // The shape an earlier build kept: no record of how it converted CPU.
    let state = deployment.root.join(".stado").join("state");
    std::fs::create_dir_all(&state).expect("the deployment's state directory");
    let kept = state.join("host-processes.json");
    std::fs::write(&kept, r#"{"taken_ns":1,"processes":{},"unreadable":{}}"#)
        .expect("an older build's kept reading");

    let first = reading(&mut deployment, &host);
    deployment.report["first_reading"] = first.clone();
    assert_eq!(first["state"], json!("first_reading"), "{first}");
    let cause = first["not_compared"]
        .as_str()
        .unwrap_or_else(|| panic!("a kept reading that was not compared names why: {first}"));
    assert!(
        cause.contains("did not record how"),
        "the cause names the older build's reading: {first}"
    );
    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(&kept).expect("the reading kept in its place"),
    )
    .expect("the kept reading parses");
    assert!(
        written["conversion"].as_str().is_some(),
        "the kept reading now records how it converted CPU: {written}"
    );

    let second = reading(&mut deployment, &host);
    deployment.report["second_reading"] = second.clone();
    assert_eq!(second["state"], json!("compared"), "{second}");
    assert_eq!(second["not_compared"], Value::Null, "{second}");
    deployment.pass();
}
