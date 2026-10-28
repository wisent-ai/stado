//! Fleet ingress acts only on the machine its published object names: a
//! published entrance that states no `pid_hint.machine` is refused as
//! unreadable by `status` and `down`, and one naming another machine is
//! refused by `down` naming it, its object left in place, while `status`
//! reports the processes as unknown here. Each drives the built `stado`
//! binary against a local store of its own, so no real entrance, process
//! group or registry is touched.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{json, Value};

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // The empty shared config: the developer's own config is never read.
        .env(
            "STADO_CONFIG",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
        )
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("the stado binary runs")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The entrance object the store publishes, path as `stado fleet ingress`
/// reads it (`INGRESS_PATH`).
fn ingress_path(storage: &Path) -> std::path::PathBuf {
    storage.join("enrollments").join("ingress.json")
}

/// A local store holding one published entrance whose pid hint is `hint`.
/// The process groups are this test process's own id, which is no group `up`
/// started and which nothing here may signal.
fn store_publishing(hint: Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temporary storage root");
    let own = std::process::id();
    let mut pid_hint = json!({
        "listener_pgid": own,
        "tunnel_pgid": own,
        "listener_log": "listener.log",
        "tunnel_log": "tunnel.log",
    });
    for (key, value) in hint.as_object().expect("the hint is an object") {
        pid_hint[key] = value.clone();
    }
    let document = json!({
        "base_url": "https://ingress-test.invalid",
        "mode": "quick",
        "host": "ingress-test.invalid",
        "started_at": "2026-10-26T00:00:00+00:00",
        "verified_at": "2026-10-26T00:00:00+00:00",
        "listener_port": own,
        "pid_hint": pid_hint,
    });
    let path = ingress_path(dir.path());
    std::fs::create_dir_all(path.parent().expect("the object has a folder"))
        .expect("the enrollments folder");
    std::fs::write(&path, document.to_string()).expect("the ingress object is written");
    dir
}

/// `stado fleet ingress <verb>` on an entrance naming no machine is refused
/// naming the missing field.
fn refuses_a_machineless_entrance(verb: &str) {
    let storage = store_publishing(json!({}));
    let out = stado(storage.path(), &["fleet", "ingress", verb]);
    assert!(
        !out.status.success(),
        "ingress {verb} accepted an entrance naming no machine"
    );
    assert!(
        stderr(&out).contains("ingress pid_hint has no 'machine'"),
        "ingress {verb} did not name the missing machine: {}",
        stderr(&out)
    );
    assert!(
        ingress_path(storage.path()).exists(),
        "a refused {verb} removed the published object"
    );
}

#[test]
fn status_refuses_an_entrance_naming_no_machine() {
    refuses_a_machineless_entrance("status");
}

#[test]
fn down_refuses_an_entrance_naming_no_machine() {
    refuses_a_machineless_entrance("down");
}

#[test]
fn down_on_another_machine_is_refused_naming_it_and_status_reads_unknown() {
    let elsewhere = format!("not-this-machine-{}", std::process::id());
    let storage = store_publishing(json!({ "machine": elsewhere }));

    let down = stado(storage.path(), &["fleet", "ingress", "down"]);
    assert!(
        !down.status.success(),
        "ingress down acted on another machine's entrance"
    );
    assert!(
        stderr(&down).contains(&format!("runs on '{elsewhere}', not on this machine")),
        "ingress down did not name the machine the entrance runs on: {}",
        stderr(&down)
    );
    assert!(
        ingress_path(storage.path()).exists(),
        "a refused down removed the published object"
    );

    let status = stado(storage.path(), &["fleet", "ingress", "status", "--json"]);
    let answer: Value = serde_json::from_slice(&status.stdout).expect("status --json answers JSON");
    assert_eq!(answer["processes_on_this_machine"], json!(false));
    assert_eq!(
        answer["listener_alive"],
        Value::Null,
        "another machine's listener was read as running or stopped here"
    );
    assert_eq!(
        answer["tunnel_alive"],
        Value::Null,
        "another machine's tunnel was read as running or stopped here"
    );
}
