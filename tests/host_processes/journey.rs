//! `stado host processes` names a known busy child with the CPU it spent.
//!
//! One isolated deployment: the real `stado host processes` on this host
//! keeps its first reading and says so; a child that does nothing but spend
//! CPU is started; the second reading must list that child by pid, with its
//! program and a CPU figure above zero, as a process started in the interval.
//! No other program reads the processes: the report comes from the kernel
//! through Stado alone.
use std::process::{Child, Command, Stdio};

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
    drop(busy);
    deployment.pass();
}
