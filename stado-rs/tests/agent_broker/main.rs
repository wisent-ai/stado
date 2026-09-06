//! `stado host reconcile-agent-skarbiec` points a host's queue agent at the
//! credential broker the service directory declares for it.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> holding a real
//! registry document, and HOME on a temp dir carrying the two things the host
//! side of this command reads: `~/.stado/bin/stado` (the same built binary,
//! installed where the host script looks for it) and
//! `~/.config/stado/config.json`, the configuration the host's own services
//! consume. The command resolves the target through the ordinary host channel,
//! runs that binary, and the assertions read the config file it wrote.
//!
//! # The incident
//!
//! On 2026-09-05 `lukasz-macbook` carried `agent.skarbiec.url =
//! http://127.0.0.1:19096`, a port nothing on that machine has ever bound,
//! while the service directory declared `http://127.0.0.1:8787` for it. Three
//! brokers were listening and none was the one named. Nothing compared the
//! two, so the symptom was a `preferences` release job dying after it had been
//! claimed — `cannot resolve job … secret GITHUB_TOKEN: error sending request
//! for url (http://127.0.0.1:19096/v1/items/read)` — a build failure whose
//! cause sat in another product's configuration file.
//!
//! What is defended: the value is taken from the directory and written to the
//! host's own config; a second run reports the idempotent outcome and writes
//! nothing; and a host the directory gives no endpoint is refused by name
//! rather than pointed at a guess.

use std::path::Path;
use std::process::{Command, Output};

fn stado(home: &Path, storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("HOME", home)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .output()
        .expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn combined(out: &Output) -> String {
    format!("{}{}", stdout(out), String::from_utf8_lossy(&out.stderr))
}

/// The name this machine answers to, normalized the way registry-v2 requires,
/// so the command resolves this host to the fixture's target.
fn this_host() -> String {
    let named = Command::new("hostname").output().expect("hostname runs");
    String::from_utf8_lossy(&named.stdout)
        .trim()
        .to_ascii_lowercase()
}

/// A two-host registry whose `skarbiec` service is placed on `active` and
/// carries one endpoint per host in `endpoints`.
fn registry_document(active: &str, endpoints: &[(&str, u16)]) -> String {
    let endpoints: serde_json::Map<String, serde_json::Value> = endpoints
        .iter()
        .map(|(host, port)| {
            (
                (*host).to_string(),
                serde_json::json!({"url": format!("http://127.0.0.1:{port}")}),
            )
        })
        .collect();
    serde_json::json!({
        "schema_version": 2,
        "targets": [
            {
                "name": "w1",
                "kind": "local",
                "ssh": "u@10.0.0.1",
                "release_platform": "darwin-arm64",
                "hostnames": ["w1.local", this_host()],
                "services": [],
            },
            {
                "name": "w2",
                "kind": "local",
                "ssh": "u@10.0.0.2",
                "release_platform": "darwin-arm64",
                "hostnames": ["w2.local"],
                "services": [],
            },
        ],
        "coordinators": [],
        "service_directory": {
            "authority": {"target": "w1", "command": "/opt/stado/bin/stado"},
            "generation": 1,
            "services": {
                "skarbiec": {
                    "managed_service": "skarbiec",
                    "active_host": active,
                    "endpoints": endpoints,
                    "consumers": {"operator": {"capabilities": ["secret-acquisition"]}},
                },
            },
        },
    })
    .to_string()
}

/// A home the host side of this command can run in: the built binary where
/// the host script looks for it, and a config file for it to read and write.
fn fixture(document: &str, agent_url: Option<&str>) -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(storage.path().join("registry.json"), document).unwrap();

    let bin = home.path().join(".stado/bin");
    std::fs::create_dir_all(&bin).unwrap();
    // A hard link, not a copy: the same executable, at the path the host
    // script resolves.
    std::fs::hard_link(env!("CARGO_BIN_EXE_stado"), bin.join("stado"))
        .or_else(|_| std::fs::copy(env!("CARGO_BIN_EXE_stado"), bin.join("stado")).map(|_| ()))
        .unwrap();

    let config_dir = home.path().join(".config/stado");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut config = serde_json::json!({"schema_version": 1});
    if let Some(url) = agent_url {
        config["agent"] = serde_json::json!({"skarbiec": {"url": url}});
    }
    std::fs::write(config_dir.join("config.json"), config.to_string()).unwrap();
    (home, storage)
}

fn agent_url(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(".config/stado/config.json")).ok()?;
    let document: serde_json::Value = serde_json::from_str(&text).ok()?;
    document
        .pointer("/agent/skarbiec/url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

#[test]
fn the_declared_endpoint_replaces_a_port_nothing_serves() {
    let (home, storage) = fixture(
        &registry_document("w1", &[("w1", 8787), ("w2", 8895)]),
        Some("http://127.0.0.1:19096"),
    );
    let out = stado(
        home.path(),
        storage.path(),
        &["host", "reconcile-agent-skarbiec", "w1", "--json"],
    );
    assert!(out.status.success(), "{}", combined(&out));
    // One document on the stream: a receipt with the host's whole
    // configuration printed beside it is not parseable.
    let report: serde_json::Value =
        serde_json::from_str(stdout(&out).trim()).expect("json receipt");
    assert_eq!(report["target"], serde_json::json!("w1"));
    assert_eq!(
        report["declared"],
        serde_json::json!("http://127.0.0.1:8787")
    );
    assert_eq!(
        report["previous"],
        serde_json::json!("http://127.0.0.1:19096")
    );
    assert_eq!(report["changed"], serde_json::json!(true));
    assert_eq!(
        agent_url(home.path()).as_deref(),
        Some("http://127.0.0.1:8787"),
        "the host's own configuration carries the declared endpoint"
    );
}

#[test]
fn a_second_run_reports_the_idempotent_outcome() {
    let (home, storage) = fixture(
        &registry_document("w1", &[("w1", 8787)]),
        Some("http://127.0.0.1:8787"),
    );
    let out = stado(
        home.path(),
        storage.path(),
        &["host", "reconcile-agent-skarbiec", "w1"],
    );
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout(&out).contains("agent.skarbiec.url already http://127.0.0.1:8787"),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        agent_url(home.path()).as_deref(),
        Some("http://127.0.0.1:8787")
    );
}

#[test]
fn an_unset_agent_url_is_declared_from_the_directory() {
    let (home, storage) = fixture(&registry_document("w1", &[("w1", 8787)]), None);
    let out = stado(
        home.path(),
        storage.path(),
        &["host", "reconcile-agent-skarbiec", "w1"],
    );
    assert!(out.status.success(), "{}", combined(&out));
    assert!(
        stdout(&out).contains("(was unset)"),
        "an absent value is stated as absent: {}",
        stdout(&out)
    );
    assert_eq!(
        agent_url(home.path()).as_deref(),
        Some("http://127.0.0.1:8787")
    );
}

#[test]
fn a_host_the_directory_gives_no_endpoint_is_refused() {
    // The service is placed on the other machine and the directory names an
    // endpoint only there, so this host has no broker of its own to point at.
    let (home, storage) = fixture(
        &registry_document("w2", &[("w2", 8895)]),
        Some("http://127.0.0.1:19096"),
    );
    let out = stado(
        home.path(),
        storage.path(),
        &["host", "reconcile-agent-skarbiec", "w1"],
    );
    assert!(!out.status.success(), "a guess is not an answer");
    let text = combined(&out);
    assert!(
        text.contains("the service directory declares no skarbiec endpoint for w1"),
        "{text}"
    );
    assert_eq!(
        agent_url(home.path()).as_deref(),
        Some("http://127.0.0.1:19096"),
        "a refusal leaves the host's configuration untouched"
    );
}
