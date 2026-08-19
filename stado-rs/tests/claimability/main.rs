//! A queue with no claimant, and how the product says so.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! in, and the registry document, the queued jobs, the capacity publications
//! and the health beacons all live and die inside the temp dir. Nothing here
//! reads the fleet's store or touches a host.
//!
//! What is under test is `stado status` and `stado overview` as reports: a
//! stuck queue is stated, a moving one is not mentioned, and neither says
//! anything about the exit status — both stay 0, because this is a report and
//! not a gate.
//!
//! Every fixture is copied from the live incident it was written for, not
//! invented. Job `2c4a47aa` is `bash inputs/run.sh`, submitted
//! 2026-08-14T19:11:37Z, `provider: local`, pinned to
//! `local-control-host.local`, and queued for 121 hours. The vocabulary
//! is the vocabulary `stado host gates control-host --json` printed on
//! that day: `blockers: ["no_capacity_publication", "pinned_only"]` with
//! `capacity.published_at: null`. The mini's queue-agent declaration is its
//! real one, `com.wisent.compute.service.stado-agent-mini` at
//! `/Users/charles/Library/LaunchAgents/...`, a unit its own health beacon
//! does not report.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

/// The real job's id, command and submitter, so a sentence about `2c4a47aa`
/// in a test is a sentence about the job the operator stared at.
const JOB_ID: &str = "2c4a47aa";
/// 121 hours and 38 minutes, the wait the live store held. Kept as an offset
/// from the test's own clock so the sentence is the same on any day.
const WAITED_SECONDS: i64 = 121 * 3600 + 38 * 60;
/// The mini's declared queue agent, verbatim from the registry.
const AGENT_LABEL: &str = "com.wisent.compute.service.stado-agent-mini";
const AGENT_PLIST: &str = "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist";

/// Three `kind=local` hosts in the shape the live registry declares them: a
/// pinned Mac mini that declares its queue agent as a user LaunchAgent, a
/// pinned Linux box that declares no agent at all, and an unpinned laptop.
const REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "mini",
            "kind": "local",
            "ssh": "charles@10.0.0.253",
            "release_platform": "darwin-arm64",
            "hostnames": ["mini.local"],
            "slots": 1,
            "pinned_only": true,
            "services": [
                {
                    "kind": "launchd",
                    "name": "com.wisent.compute.service.stado-agent-mini",
                    "label": "com.wisent.compute.service.stado-agent-mini",
                    "path": "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist",
                    "unit": "",
                    "managed_since": "2026-08-19T00:46:51.797832+00:00"
                }
            ]
        },
        {
            "name": "rtx",
            "kind": "local",
            "ssh": "root@10.0.0.108",
            "release_platform": "linux-amd64",
            "hostnames": ["rtx-box"],
            "gpu_type": "nvidia-rtx-pro-6000",
            "slots": 2,
            "pinned_only": true
        },
        {
            "name": "laptop",
            "kind": "local",
            "ssh": "op@10.0.0.234",
            "release_platform": "darwin-arm64",
            "hostnames": ["laptop.local"],
            "slots": 2
        }
    ],
    "coordinators": []
}"#;

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A temp store carrying [`REGISTRY`] and nothing else.
fn fleet() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("registry.json"), REGISTRY).unwrap();
    dir
}

fn write(storage: &Path, name: &str, body: &Value) {
    let path = storage.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(body).unwrap()).unwrap();
}

/// `seconds` before the moment this test runs, RFC-3339.
fn ago(seconds: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::seconds(seconds)).to_rfc3339()
}

/// The live queued job: `provider: local`, cpu, and pinned to `pinned_host`.
fn queue_job(storage: &Path, job_id: &str, waited_seconds: i64, pinned_host: &str) {
    write(
        storage,
        &format!("queue/{job_id}.json"),
        &serde_json::json!({
            "job_id": job_id,
            "command": "bash inputs/run.sh",
            "provider": "local",
            "state": "queued",
            "created_at": ago(waited_seconds),
            "pinned_host": pinned_host,
            "assigned_to": pinned_host,
            "submitted_by": "lukaszbartoszcze",
            "submitted_from": "laptop.local",
        }),
    );
}

/// One capacity broadcast in the shape `queue::capacity::publish_capacity`
/// writes it.
fn publish(storage: &Path, consumer_id: &str, age_seconds: i64, diag: Value) {
    write(
        storage,
        &format!("capacity/{consumer_id}.json"),
        &serde_json::json!({
            "consumer_id": consumer_id,
            "kind": consumer_id.split_once('-').map(|(kind, _)| kind).unwrap_or("local"),
            "free_slots": {"cpu": 1},
            "free_vram_gb": 1,
            "total_vram_gb": 1,
            "published_at": ago(age_seconds),
            "diag": diag,
        }),
    );
}

/// A health beacon reporting exactly `units`.
fn beacon(storage: &Path, slug: &str, units: Value) {
    write(
        storage,
        &format!("host_health/{slug}.json"),
        &serde_json::json!({
            "host": slug,
            "reported_at": ago(30),
            "units": units,
        }),
    );
}

/// The `claimability` section of `stado overview --json`.
fn claimability(storage: &Path) -> Value {
    let out = stado(storage, &["overview", "--json"]);
    assert!(out.status.success(), "overview --json exits 0");
    let document: Value = serde_json::from_str(&stdout(&out)).expect("overview --json is JSON");
    document["claimability"].clone()
}

/// A queue nobody publishes capacity for is named as such, host by host, in
/// each host's own words — and the report is still a report: exit 0.
#[test]
fn status_names_every_host_that_cannot_claim_the_queue() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    // The mini's beacon is alive and does not carry the declared agent: the
    // unit is bootstrapped into a domain that host cannot have, so nothing
    // ever loaded it.
    beacon(
        storage,
        "mini",
        serde_json::json!({"com.wisent.host-health-beacon": {"state": "active"}}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success(), "a report never fails the command");
    let text = stdout(&out);

    assert!(
        text.contains(&format!(
            "nothing can claim the queue: 1 queued, oldest {JOB_ID} waiting 121h "
        )),
        "the headline sizes the stall with the oldest wait: {text}"
    );
    assert!(
        text.contains("0 of 3 local hosts publish capacity newer than 180s"),
        "the headline counts publishers, not declarations: {text}"
    );
    assert!(
        text.contains(&format!(
            "  cannot claim: mini no_capacity_publication, agent_declared_not_loaded ({AGENT_LABEL} is declared at {AGENT_PLIST}; the latest beacon does not report it)"
        )),
        "the mini's silence is explained by its own declaration: {text}"
    );
    // Pinned with nothing pinned to it: the pin IS the reason, so it is said.
    assert!(
        text.contains(
            "  cannot claim: rtx no_capacity_publication, pinned_only (no queued job names this host)"
        ),
        "{text}"
    );
    assert!(
        text.contains("  cannot claim: laptop no_capacity_publication"),
        "{text}"
    );
    // The mini IS named by the queued job, so `pinned_only` is not one of its
    // reasons: sending an operator to unpin a host that would have claimed is
    // sending them to the wrong place.
    assert!(
        !text.contains("cannot claim: mini no_capacity_publication, pinned_only"),
        "a pinned host with a matching queued job is not blocked by its pin: {text}"
    );
}

/// "That host went quiet seventeen hours ago" and "that host never said
/// anything" are different findings, and the reader must not collect the
/// evidence for the first one on its way past.
#[test]
fn status_reports_an_aged_publication_as_stale_and_keeps_it() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    publish(
        storage,
        "local-laptop.local",
        17 * 3600 + 5 * 60,
        serde_json::json!({}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("  cannot claim: laptop capacity_publication_stale (last published 17h "),
        "a row past the horizon is stale, with its age: {text}"
    );
    assert!(
        !text.contains("cannot claim: laptop no_capacity_publication"),
        "a stale row is not silence: {text}"
    );
    // A report deletes nothing. The scheduler's reader garbage-collects rows
    // past an hour; this one must leave the evidence where it found it.
    assert!(
        storage.join("capacity/local-laptop.local.json").exists(),
        "the publication the verdict reported still exists"
    );
}

/// A pinned host publishing capacity with work addressed to it claims, and a
/// claiming fleet is not commented on at all.
#[test]
fn status_says_nothing_while_a_pinned_host_holds_a_matching_job() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    publish(
        storage,
        "local-mini.local",
        20,
        serde_json::json!({"pinned_only": true}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        !text.contains("nothing can claim the queue"),
        "one host that can claim is a moving queue: {text}"
    );
    assert!(!text.contains("cannot claim:"), "{text}");
    assert_eq!(claimability(storage)["claimable"], Value::Bool(true));
}

/// The same pinned host, with the queued job addressed elsewhere: now the pin
/// is why nothing moves, and the words say so.
#[test]
fn status_blames_the_pin_when_no_queued_job_names_the_pinned_host() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-somewhere-else.local");
    publish(
        storage,
        "local-mini.local",
        20,
        serde_json::json!({"pinned_only": true}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("1 of 3 local hosts publish capacity newer than 180s"),
        "the mini is publishing; that is not the problem: {text}"
    );
    assert!(
        text.contains("  cannot claim: mini pinned_only (no queued job names this host)"),
        "{text}"
    );
    assert!(
        !text.contains("cannot claim: mini no_capacity_publication"),
        "a publishing host is not silent: {text}"
    );
}

/// A host publishing fresh capacity with nothing in its way ends the verdict:
/// the queue is claimable, and neither surface says a word about it.
#[test]
fn a_claimable_queue_gets_no_verdict_on_either_surface() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "");
    publish(storage, "local-laptop.local", 20, serde_json::json!({}));

    let status = stado(storage, &["status"]);
    assert!(status.status.success());
    let listing = stdout(&status);
    assert!(listing.contains("1 queued"), "the job is still listed");
    assert!(
        !listing.contains("nothing can claim the queue"),
        "{listing}"
    );

    let overview = stado(storage, &["overview"]);
    assert!(overview.status.success());
    let snapshot = stdout(&overview);
    assert!(
        snapshot.contains("fleet: 1 of 3 local hosts publishing capacity | 3 registered targets"),
        "the fleet line counts publications: {snapshot}"
    );
    assert!(
        !snapshot.contains("nothing can claim the queue"),
        "{snapshot}"
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(true));
    assert_eq!(verdict["stuck"], Value::Bool(false));
    assert_eq!(verdict["publishing"], serde_json::json!(["laptop"]));
}

/// An empty queue is not a stuck queue, however silent the fleet is.
#[test]
fn an_empty_queue_gets_no_verdict_however_silent_the_fleet() {
    let dir = fleet();
    let storage = dir.path();

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("0 queued"), "{text}");
    assert!(!text.contains("nothing can claim the queue"), "{text}");

    let verdict = claimability(storage);
    assert_eq!(verdict["stuck"], Value::Bool(false));
    assert_eq!(verdict["queued"], serde_json::json!(0));
    assert_eq!(verdict["oldest_queued"], Value::Null);
}

/// A publisher no declared host claims — a cloud dispatcher, a marketplace
/// worker — is a claimant this report cannot size, so it refuses to assert a
/// stall. Saying less beats asserting a stall that is not there.
#[test]
fn a_fresh_publisher_the_registry_does_not_declare_withholds_the_verdict() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "");
    publish(storage, "gcp-dispatcher", 20, serde_json::json!({}));

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    assert!(
        !stdout(&out).contains("nothing can claim the queue"),
        "{}",
        stdout(&out)
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(true));
    assert_eq!(
        verdict["unattributed_publishers"],
        serde_json::json!(["gcp-dispatcher"])
    );
    // Not counted as a publishing local host: it is not one.
    assert_eq!(verdict["publishing"], serde_json::json!([]));
}

/// `stado overview` used to print a worker count under the words "active
/// workers" while nothing in the fleet could claim anything. It now counts
/// publications, and prints the verdict with the oldest wait.
#[test]
fn overview_counts_publications_and_prints_the_verdict() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    beacon(
        storage,
        "mini",
        serde_json::json!({"com.wisent.host-health-beacon": {"state": "active"}}),
    );

    let out = stado(storage, &["overview"]);
    assert!(out.status.success(), "a report never fails the command");
    let text = stdout(&out);
    assert!(
        text.contains("fleet: 0 of 3 local hosts publishing capacity | 3 registered targets"),
        "three declared hosts, none publishing: {text}"
    );
    assert!(
        text.contains(&format!(
            "nothing can claim the queue: 1 queued, oldest {JOB_ID} waiting 121h "
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "  cannot claim: mini no_capacity_publication, agent_declared_not_loaded ({AGENT_LABEL} is declared at {AGENT_PLIST}; the latest beacon does not report it)"
        )),
        "{text}"
    );

    let verdict = claimability(storage);
    assert_eq!(verdict["claimable"], Value::Bool(false));
    assert_eq!(verdict["stuck"], Value::Bool(true));
    assert_eq!(verdict["stale_horizon_seconds"], serde_json::json!(180));
    assert_eq!(verdict["oldest_queued"]["job_id"], JOB_ID);
    assert!(
        verdict["oldest_queued"]["age_seconds"]
            .as_i64()
            .expect("the oldest wait is dated")
            >= WAITED_SECONDS,
        "{verdict}"
    );
    assert_eq!(
        verdict["hosts"][0]["blockers"][1]["word"],
        "agent_declared_not_loaded"
    );
}

/// A beacon that reports the declared agent as loaded and active leaves the
/// declaration finding unsaid: the unit is not the reason for the silence.
#[test]
fn a_loaded_agent_is_not_reported_as_undeclared_or_unloaded() {
    let dir = fleet();
    let storage = dir.path();
    queue_job(storage, JOB_ID, WAITED_SECONDS, "local-mini.local");
    beacon(
        storage,
        "mini",
        serde_json::json!({AGENT_LABEL: {"state": "active"}}),
    );

    let out = stado(storage, &["status"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("  cannot claim: mini no_capacity_publication\n"),
        "the host is silent, and its declared agent is not why: {text}"
    );
    assert!(
        !text.contains("agent_declared_not_loaded"),
        "a loaded unit is never reported as unloaded: {text}"
    );
}
