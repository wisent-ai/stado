//! Detached Jeden sessions: the fleet keeps working after the process that
//! asked for the work is gone.
//!
//! Every case drives the built Stado CLI against the isolated store of
//! [`super::harness`], with the real Jeden binary installed as the managed
//! runtime of that journey's home. The evidence is the persisted queue
//! record and the persisted job document, never stdout alone.

use serde_json::Value;
use stado::queue::runs::TERMINAL_PREFIXES;

use super::attach::install_runtime;
use super::harness::{said, Area, TARGET};

const TASK: &str = "Report the workspace name and stop";

/// The consumer id the queue stores for this machine: the registry
/// normalizes the hostname, so the test reads it the same way rather than
/// assuming the case the kernel answered with.
fn consumer_id(area: &Area) -> String {
    format!("local-{}", area.hostname.to_lowercase())
}

/// A host that reads its own Skarbiec credentials holds this grant file,
/// which is how `stado secrets get` works on the operator's machines. Its
/// presence is what the session placement probes for; the bytes are never
/// read by the placement.
fn hold_operator_grant(area: &Area) {
    let path = area.home.join(".stado/local-operator-skarbiec-token");
    std::fs::create_dir_all(path.parent().expect("the grant file has a parent")).unwrap();
    std::fs::write(&path, "journey-grant\n").unwrap();
}

fn start(area: &Area, arguments: &[&str]) -> std::process::Output {
    let mut args = vec!["workload", "start", "jeden-session", "--target", TARGET];
    args.extend_from_slice(arguments);
    area.stado(&args)
}

fn started_record(area: &Area, arguments: &[&str]) -> Value {
    let output = start(area, arguments);
    assert!(
        output.status.success(),
        "starting a detached session failed: {}",
        said(&output.stderr)
    );
    let stdout = said(&output.stdout);
    serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("the start record is JSON ({error}):\n{stdout}"))
}

#[test]
fn a_detached_session_is_queued_as_a_real_job_sized_from_its_declaration() {
    let area = Area::new();
    install_runtime(&area);
    hold_operator_grant(&area);
    let record = started_record(
        &area,
        &["--workspace", "__home__", "--task", TASK, "--json"],
    );
    let job_id = record["job_id"].as_str().expect("the record names the job");
    let job = area.record("queue", job_id);
    assert_eq!(job["batch_id"], "workload-jeden-session", "{job}");
    assert_eq!(job["cpu_cores"], 2, "{job}");
    assert_eq!(job["memory_gb"], 4, "{job}");
    assert_eq!(
        job["priority"],
        stado::primitives::constants::DETACHED_SESSION_JOB_PRIORITY,
        "a session a person is waiting on outranks routine batch work: {job}"
    );
    assert_eq!(
        job["pinned_host"].as_str().unwrap_or_default(),
        consumer_id(&area),
        "the session is pinned to the host the placement chose: {job}"
    );
    let command = job["command"].as_str().expect("the job carries a command");
    assert!(
        command.contains("/.stado/bin/jeden run"),
        "the queued command runs the managed Jeden: {command}"
    );
    assert!(
        command.contains("'Report the workspace name and stop'"),
        "the queued command carries the task: {command}"
    );
    assert!(
        !command.contains("--allow-write") && !command.contains("--allow-command"),
        "an ungranted session may neither write nor run commands: {command}"
    );
}

#[test]
fn the_start_record_says_where_the_session_runs_and_what_it_holds() {
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
            "gpt-6-astra",
            "--allow-write",
            "--json",
        ],
    );
    assert_eq!(record["kind"], "jeden-session", "{record}");
    assert_eq!(record["target"], TARGET, "{record}");
    assert_eq!(record["workspace"], "__home__", "{record}");
    assert_eq!(record["state"], "queued", "{record}");
    assert_eq!(record["model"], "gpt-6-astra", "{record}");
    assert_eq!(record["grants"]["write"], true, "{record}");
    assert_eq!(record["grants"]["command"], false, "{record}");
    assert_eq!(record["reservation"]["cpu_cores"], 2, "{record}");
    assert_eq!(record["reservation"]["ram_gb"], 4.0, "{record}");
    assert!(
        record["ledger"]
            .as_str()
            .is_some_and(|ledger| ledger.ends_with(".jeden/sessions")),
        "the record names the durable session ledger: {record}"
    );
    let command = area.record("queue", record["job_id"].as_str().unwrap())["command"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        command.contains("--model 'gpt-6-astra'") && command.contains("--allow-write"),
        "the granted model and write grant reach the host: {command}"
    );
}

#[test]
fn a_detached_session_with_no_task_is_refused_before_anything_is_queued() {
    let area = Area::new();
    install_runtime(&area);
    hold_operator_grant(&area);
    let output = start(&area, &["--workspace", "__home__"]);
    assert!(!output.status.success(), "{}", said(&output.stdout));
    assert!(
        said(&output.stderr).contains("jeden-session requires --task TEXT for a detached session"),
        "the refusal is the documented sentence: {}",
        said(&output.stderr)
    );
    assert!(
        !area.holds("queue"),
        "a refused start queues nothing at all"
    );
}

#[test]
fn a_kind_that_is_not_detachable_is_refused_with_the_file_to_change() {
    let area = Area::new();
    let output = area.stado(&[
        "workload",
        "start",
        "weles-activity",
        "--target",
        TARGET,
        "--task",
        TASK,
    ]);
    assert!(!output.status.success(), "{}", said(&output.stdout));
    assert!(
        said(&output.stderr).contains(
            "workload kind 'weles-activity' is not detachable; add \"detachable\": true to stado-rs/data/work/workloads.json"
        ),
        "the refusal names the declaration to change: {}",
        said(&output.stderr)
    );
}

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
