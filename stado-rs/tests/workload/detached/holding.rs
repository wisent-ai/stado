//! What the fleet does with a session once it holds one: lists it, runs it
//! after the starting process is gone, claims it on a host with no spare video
//! memory when the session names a model, and hands the credentials to the host
//! by coordinate rather than on the command line.

use serde_json::Value;
use stado::queue::runs::TERMINAL_PREFIXES;

use super::super::attach::install_runtime;
use super::super::harness::{said, Area, TARGET};

use super::{consumer_id, hold_operator_grant, started_record, TASK};

#[test]
fn the_fleet_lists_the_detached_sessions_it_is_holding() {
    let area = Area::new();
    install_runtime(&area);
    hold_operator_grant(&area);
    let record = started_record(
        &area,
        &["--workspace", "__home__", "--task", TASK, "--json"],
    );
    let output = area.stado(&["workload", "sessions", "--json"]);
    assert!(
        output.status.success(),
        "listing detached sessions failed: {}",
        said(&output.stderr)
    );
    let listed: Value = serde_json::from_str(&said(&output.stdout)).expect("the listing is JSON");
    let sessions = listed["sessions"].as_array().expect("sessions is an array");
    let session = sessions
        .iter()
        .find(|session| session["job_id"] == record["job_id"])
        .unwrap_or_else(|| panic!("the started session is listed:\n{listed}"));
    assert_eq!(session["kind"], "jeden-session", "{session}");
    assert_eq!(session["state"], "queued", "{session}");
    assert_eq!(session["workspace"], "__home__", "{session}");
    assert_eq!(
        session["host"].as_str().unwrap_or_default(),
        consumer_id(&area),
        "{session}"
    );
}

#[test]
fn the_fleet_runs_a_detached_session_after_the_starting_process_has_exited() {
    let area = Area::new();
    install_runtime(&area);
    hold_operator_grant(&area);
    let record = started_record(
        &area,
        &["--workspace", "__home__", "--task", TASK, "--json"],
    );
    let job_id = record["job_id"].as_str().expect("the record names the job");
    // The starting command has already exited: everything below is the
    // fleet's own worker executing work nobody is attached to.
    let log = area.drain();
    let terminal = TERMINAL_PREFIXES
        .into_iter()
        .find(|prefix| area.holds(&format!("{prefix}/{job_id}.json")))
        .unwrap_or_else(|| panic!("the detached session reached no terminal state:\n{log}"));
    let job = area.record(terminal, job_id);
    assert!(
        job["command"]
            .as_str()
            .is_some_and(|command| command.contains("/.stado/bin/jeden run")),
        "the worker ran the managed Jeden for this session: {job}"
    );
    assert!(
        job["started_at"].as_str().is_some_and(|at| !at.is_empty()),
        "the persisted record says the session really started: {job}"
    );
}

/// A session carries its model route in the command the worker runs, and the
/// queue's model-name sizing scan used to read that as a GPU workload: on a
/// laptop with one GiB of VRAM every session naming a model was refused by
/// the claim, silently, on every poll. The submission resolves to the CPU
/// marker now and the claim honours it, so the session runs here.
#[test]
fn a_session_that_names_a_model_is_claimed_on_a_host_with_no_spare_vram() {
    let area = Area::new();
    install_runtime(&area);
    hold_operator_grant(&area);
    let record = started_record(
        &area,
        &[
            "--workspace",
            "__home__",
            "--task",
            TASK,
            "--model",
            "openrouter/openrouter/free",
            "--json",
        ],
    );
    let job_id = record["job_id"].as_str().expect("the record names the job");
    let job = area.record("queue", job_id);
    assert_eq!(
        job["gpu_mem_gb"], 0,
        "a session declares no accelerator: {job}"
    );
    let log = area.drain();
    let terminal = TERMINAL_PREFIXES
        .into_iter()
        .find(|prefix| area.holds(&format!("{prefix}/{job_id}.json")))
        .unwrap_or_else(|| panic!("the session naming a model was never claimed:\n{log}"));
    let job = area.record(terminal, job_id);
    assert!(
        job["started_at"].as_str().is_some_and(|at| !at.is_empty()),
        "the worker really started the session: {job}"
    );
}

/// A host that cannot read the vault itself — the Linux builder holds no
/// operator grant file, which is why a session there failed on
/// 2026-09-20 with `cannot read Skarbiec grant file
/// /root/.stado/local-operator-skarbiec-token` — is handed the same two
/// credentials by its own agent, as declared job secrets. Nothing puts a
/// value in the command.
#[test]
fn a_host_without_its_own_grant_receives_the_session_credentials_from_the_fleet() {
    let area = Area::new();
    install_runtime(&area);
    let record = started_record(
        &area,
        &["--workspace", "__home__", "--task", TASK, "--json"],
    );
    let job = area.record("queue", record["job_id"].as_str().unwrap());
    assert_eq!(
        job["secret_env"]["WISENT_APP_AGENT_AUTH_SECRET"]["item"], "agent:wisent-app",
        "the signing secret is declared by coordinate: {job}"
    );
    assert_eq!(
        job["secret_env"]["BRAMA_TOKEN"]["item"], "jeden-model-router",
        "the gateway bearer is declared by coordinate: {job}"
    );
    let command = job["command"].as_str().unwrap_or_default();
    assert!(
        !command.contains("BRAMA_TOKEN") && !command.contains("AUTH_SECRET"),
        "no credential name or value reaches the command line: {command}"
    );
}
