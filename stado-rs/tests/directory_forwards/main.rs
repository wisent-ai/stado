//! `stado service directory publish` writes the address THIS host dials.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>, a STADO_CONFIG
//! pointing at a nonexistent path so the operator's real config cannot leak
//! in, and HOME on a temp dir because the markers under
//! `~/.stado/forwards/` are exactly what is under test.
//!
//! # The incident
//!
//! `publish` wrote `endpoints[<this host>]` and skipped every service the
//! directory places elsewhere. A skipped service leaves the marker file as
//! whatever last wrote it, and consumers keep reading it: on `lukasz-macbook`
//! `~/.stado/forwards/brama.local` named `http://127.0.0.1:8080`, which is
//! Brama's port **on the Mac mini** and on the laptop belongs to an unrelated
//! service, and `weles-admission.local` named `8788` while the documented
//! answer for a non-serving host is that host's own resolver adapter at
//! `17614`. Kronika's documentation gate dialled the marker's address and got
//! `fetch failed`; nothing in the fleet compared the two.
//!
//! What is defended: a serving host publishes the directory endpoint; a
//! non-serving host publishes its own adapter; a service with one adapter per
//! consumer is refused by name rather than resolved to one consumer's socket;
//! and an adapter-published marker is never swept as a fossil.

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

/// The name this machine answers to, normalized the way registry-v2 requires,
/// so `publish` resolves this host to the fixture's target.
fn this_host() -> String {
    let named = Command::new("hostname").output().expect("hostname runs");
    String::from_utf8_lossy(&named.stdout)
        .trim()
        .to_ascii_lowercase()
}

/// A two-host registry: `w1` is this machine, `w2` is the other one, and the
/// service is active on whichever `active` names. `adapters` are the resolver
/// adapters `w1` declares for that service.
fn registry_document(active: &str, adapters: &[(&str, u16)]) -> String {
    let service = serde_json::json!([{
        "name": "weles-admission",
        "unit": "",
        "label": "com.wisent.compute.service.weles-admission",
        "path": "/Users/u/Library/LaunchAgents/com.wisent.compute.service.weles-admission.plist",
        "kind": "launchd",
        "managed_since": "2026-08-01T00:00:00+00:00",
    }]);
    let declared: Vec<serde_json::Value> = adapters
        .iter()
        .map(|(consumer, port)| {
            serde_json::json!({
                "service": "weles-admission",
                "consumer": consumer,
                "bind": format!("127.0.0.1:{port}"),
            })
        })
        .collect();
    let mut w1 = serde_json::json!({
        "name": "w1",
        "kind": "local",
        "ssh": "u@10.0.0.1",
        "release_platform": "darwin-arm64",
        "hostnames": ["w1.local", this_host()],
        "services": serde_json::json!([]),
        "service_resolver": {
            "api_bind": "127.0.0.1:17600",
            "refresh_seconds": 5,
            "max_stale_seconds": 15,
            "adapters": declared,
        },
    });
    let mut w2 = serde_json::json!({
        "name": "w2",
        "kind": "local",
        "ssh": "u@10.0.0.2",
        "release_platform": "darwin-arm64",
        "hostnames": ["w2.local"],
        "services": serde_json::json!([]),
    });
    if active == "w1" {
        w1["services"] = service;
    } else {
        w2["services"] = service;
    }
    serde_json::json!({
        "schema_version": 2,
        "targets": [w1, w2],
        "coordinators": [],
        "service_directory": {
            "authority": {"target": "w1", "command": "/opt/stado/bin/stado"},
            "generation": 1,
            "services": {
                "weles-admission": {
                    "managed_service": "weles-admission",
                    "active_host": active,
                    "endpoints": {active: {"url": "http://127.0.0.1:8766"}},
                    "consumers": {"skarbiec": {"capabilities": ["credential-admission"]}},
                },
            },
        },
    })
    .to_string()
}

fn fixture(document: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(storage.path().join("registry.json"), document).unwrap();
    (home, storage)
}

fn marker(home: &Path, service: &str) -> Option<String> {
    let path = home
        .join(".stado")
        .join("forwards")
        .join(format!("{service}.local"));
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

fn published(report: &serde_json::Value, service: &str) -> serde_json::Value {
    report["published"]
        .as_array()
        .expect("published array")
        .iter()
        .find(|row| row["service"] == serde_json::json!(service))
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

fn skipped_reason(report: &serde_json::Value, service: &str) -> String {
    report["skipped"]
        .as_array()
        .expect("skipped array")
        .iter()
        .find(|row| row["service"] == serde_json::json!(service))
        .and_then(|row| row["reason"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn the_serving_host_publishes_the_directory_endpoint() {
    let (home, storage) = fixture(&registry_document("w1", &[]));
    let out = stado(
        home.path(),
        storage.path(),
        &["service", "directory", "publish", "--json"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json report");
    let row = published(&report, "weles-admission");
    assert_eq!(row["url"], serde_json::json!("http://127.0.0.1:8766"));
    assert_eq!(row["source"], serde_json::json!("directory-endpoint"));
    assert_eq!(
        marker(home.path(), "weles-admission").as_deref(),
        Some("http://127.0.0.1:8766"),
        "the serving host dials the port it serves on"
    );
}

#[test]
fn a_non_serving_host_publishes_its_own_resolver_adapter() {
    let (home, storage) = fixture(&registry_document("w2", &[("skarbiec", 17614)]));
    let out = stado(
        home.path(),
        storage.path(),
        &["service", "directory", "publish", "--json"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json report");
    let row = published(&report, "weles-admission");
    assert_eq!(
        row["url"],
        serde_json::json!("http://127.0.0.1:17614"),
        "a host that does not serve the service dials its own adapter, never the serving host's \
         loopback port: {report}"
    );
    assert_eq!(row["source"], serde_json::json!("resolver-adapter"));
    assert_eq!(
        marker(home.path(), "weles-admission").as_deref(),
        Some("http://127.0.0.1:17614")
    );
}

#[test]
fn several_consumer_adapters_are_refused_by_name_and_write_nothing() {
    let (home, storage) = fixture(&registry_document(
        "w2",
        &[("skarbiec", 17614), ("operator", 17615)],
    ));
    let out = stado(
        home.path(),
        storage.path(),
        &["service", "directory", "publish", "--json"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json report");
    let reason = skipped_reason(&report, "weles-admission");
    assert!(
        reason.contains("2 weles-admission adapters") && reason.contains("skarbiec, operator"),
        "the refusal names every consumer adapter: {reason}"
    );
    assert_eq!(
        marker(home.path(), "weles-admission"),
        None,
        "an ambiguous service leaves no marker for a consumer to dial"
    );
}

#[test]
fn an_adapter_published_marker_is_not_swept_as_a_fossil() {
    let (home, storage) = fixture(&registry_document("w2", &[("skarbiec", 17614)]));
    // A marker for a service no declaration accounts for: the fossil shape.
    let forwards = home.path().join(".stado").join("forwards");
    std::fs::create_dir_all(&forwards).unwrap();
    std::fs::write(
        forwards.join("stado-weles-api.local"),
        "http://127.0.0.1:8766\n",
    )
    .unwrap();

    let out = stado(
        home.path(),
        storage.path(),
        &["service", "directory", "publish", "--prune", "--json"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("json report");
    let pruned: Vec<&str> = report["pruned"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["service"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        pruned,
        vec!["stado-weles-api"],
        "only the undeclared marker is swept: {report}"
    );
    assert_eq!(
        marker(home.path(), "weles-admission").as_deref(),
        Some("http://127.0.0.1:17614"),
        "the adapter address survives the sweep that removed the fossil"
    );
    assert_eq!(marker(home.path(), "stado-weles-api"), None);
}
