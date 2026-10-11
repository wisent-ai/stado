//! One process per role on a host, through the real product. A second
//! `stado serve` of the same role on one deployment is refused at once,
//! naming the first's pid, program and start; a role process whose program
//! is gone from disk — a copy of the binary under the deployment, started as
//! `stado serve` and then deleted — is ended by the host process's next
//! health pass, which publishes what it ended in the beacon's `roles`, so
//! `stado service list --json` carries it. The lock file is inspected on
//! disk, and the stray's absence is read from the kernel.
mod deployment;

use deployment::Deployment;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Whether `pid` still exists, as the kernel answers.
fn alive(pid: u32) -> bool {
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(pid).expect("a pid fits")),
        None,
    )
    .is_ok()
}

#[test]
fn a_second_serve_of_the_same_role_is_refused_and_a_stray_is_ended_by_the_health_pass() {
    let mut deployment = Deployment::start();
    let slug = deployment.serve();
    let first_pid = deployment.child.as_ref().expect("the host process").id();
    let lock = deployment.home.join(".stado/roles/serve.lock");
    let holder: Value = serde_json::from_slice(&fs::read(&lock).expect("the role lock is written"))
        .expect("the lock holds the holder record");
    assert_eq!(holder["pid"], first_pid, "{holder}");
    assert_eq!(holder["role"], "serve", "{holder}");

    // A second start of the same role, with the arguments the first was
    // started with: refused before any listener binds.
    let again = deployment.serve_arguments_with_store(&deployment.store());
    let again: Vec<&str> = again.iter().map(String::as_str).collect();
    let second = deployment.run(&again);
    assert!(
        !second.status.success(),
        "a second serve of the same role ran"
    );
    let said = String::from_utf8_lossy(&second.stderr);
    assert!(
        said.contains(&format!(
            "role serve already runs on this host: pid {first_pid}, program "
        )),
        "{said}"
    );
    assert!(said.contains("one process holds a role"), "{said}");
    deployment.report["second_start_refusal"] = json!(said.trim());

    // A stray: a copy of the program under the deployment, run as a role
    // with its own store, its program then removed. Its own lock lives
    // under its HOME too, so it holds no role of this host.
    let stray_home = deployment.root.join("stray");
    fs::create_dir_all(stray_home.join(".stado")).unwrap();
    fs::create_dir_all(deployment.root.join("stray-bin")).unwrap();
    let copy = deployment.root.join("stray-bin").join("stado");
    fs::copy(env!("CARGO_BIN_EXE_stado"), &copy).unwrap();
    let mut stray = Command::new(&copy)
        .args(deployment.serve_arguments_with_store(&stray_home.join("store")))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &stray_home)
        .env(
            "STADO_CONFIG",
            deployment.home.join(".stado").join("config.json"),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the copied program starts");
    let stray_pid = stray.id();
    let listening = BufReader::new(stray.stderr.take().unwrap())
        .lines()
        .map(|line| line.unwrap())
        .find(|line| line.contains("listening on"));
    assert!(listening.is_some(), "the stray ended before it listened");
    fs::remove_file(&copy).unwrap();
    assert!(
        alive(stray_pid),
        "the stray must be running when the pass finds it"
    );

    // The host process's health pass runs every second: it ends the stray
    // and publishes it. The beacon is read until a publication says so.
    let record = loop {
        let beacon: Value =
            serde_json::from_str(&deployment.cli(&["host", "beacon", "show", &slug, "--json"]))
                .expect("the beacon is JSON");
        let published = beacon["roles"]["ended_strays"]
            .as_array()
            .and_then(|list| list.iter().find(|stray| stray["pid"] == stray_pid).cloned());
        if let Some(record) = published {
            break record;
        }
    };
    assert_eq!(record["reason"], "program_gone", "{record}");
    assert!(
        record["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("no longer exists")),
        "{record}"
    );
    assert!(
        !alive(stray_pid),
        "the stray is still running after the pass ended it"
    );
    let _ = stray.wait();
    deployment.report["ended_stray"] = record;

    // `service list --json` carries the same register.
    let listed: Value =
        serde_json::from_str(&deployment.cli(&["service", "list", "--json"])).expect("JSON");
    let rows = listed.as_array().expect("rows");
    assert!(
        rows.iter().any(|row| row["roles"]["held"]
            .as_array()
            .is_some_and(|held| held.iter().any(|h| h["pid"] == first_pid))),
        "service list does not carry the role holder: {listed}"
    );
    deployment.pass();
}
