//! `stado resolver status` against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config cannot leak in,
//! and HOME points at the temp dir because two things this command touches are
//! HOME-derived and not store-derived: the state file `resolver serve`
//! publishes (`~/.stado/resolver-state.json`) and the last-known-good registry
//! cache (`~/.stado/cache/`). Leaving the real HOME in place would have this
//! suite overwrite the operator's cached registry with a two-host fixture.
//!
//! What is under test is the readiness answer the resolver had none of on
//! 2026-08-19, when it sat in a launchd restart loop with `last exit code =
//! 69: EX_UNAVAILABLE` and the only trace of the reason was a sentence in
//! `~/.stado/logs/stado-resolver.err`. The refusal sentences here are copied
//! from that log and from hand runs of this command, not invented.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};

fn stado(home: &Path, storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("HOME", home)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR")
        .env_remove("STADO_RESOLVER_STATE_FILE");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A port nothing holds: bound to learn the number, then released.
fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    listener.local_addr().expect("bound address").port()
}

/// A port something holds for as long as the returned listener lives — what
/// makes `listening: true` a fact this test observed rather than one it
/// assumed.
fn held_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let port = listener.local_addr().expect("bound address").port();
    (listener, port)
}

/// The one registry shape these tests vary: the generation the authority
/// publishes, and the two loopback ports the resolver policy declares.
///
/// `service_directory.authority.target` is the same target the resolver runs
/// on, so the authority read is the local store read and no test ever opens an
/// ssh connection.
///
/// It satisfies the whole registry-v2 contract, not merely the loader: the
/// route names a `managed_service` and the target declares it. A document that
/// only parses is refused by the last-known-good cache with
/// `[registry-cache] not recording the last-known-good registry ...`, and a
/// fixture that trips a contract check is a fixture testing the wrong thing.
fn registry_document(generation: u64, api_port: u16, adapter_port: u16) -> String {
    serde_json::json!({
        "schema_version": 2,
        "targets": [{
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "darwin-arm64",
            "hostnames": ["w1.local"],
            "slots": 1,
            "services": [{
                "name": "stado-object-api",
                "unit": "",
                "label": "com.wisent.compute.service.stado-object-api",
                "path": "/Users/u/Library/LaunchAgents/com.wisent.compute.service.stado-object-api.plist",
                "kind": "launchd",
                "managed_since": "2026-08-01T00:00:00+00:00",
            }],
            "service_resolver": {
                "api_bind": format!("127.0.0.1:{api_port}"),
                "refresh_seconds": 5,
                "max_stale_seconds": 15,
                "adapters": [{
                    "service": "stado-object-api",
                    "consumer": "stado-local-agent",
                    "bind": format!("127.0.0.1:{adapter_port}"),
                }],
            },
        }],
        "coordinators": [],
        "service_directory": {
            "authority": {"target": "w1", "command": "/opt/stado/bin/stado"},
            "generation": generation,
            "services": {
                "stado-object-api": {
                    "managed_service": "stado-object-api",
                    "active_host": "w1",
                    "endpoints": {"w1": {"url": "http://127.0.0.1:18765"}},
                    "consumers": {"stado-local-agent": {"capabilities": ["object-store"]}},
                },
            },
        },
    })
    .to_string()
}

/// A temp HOME + storage root carrying exactly this registry document.
fn fixture(document: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(storage.path().join("registry.json"), document).unwrap();
    (home, storage)
}

/// Write the state file `resolver serve` publishes, as that process writes it.
fn publish_state(home: &Path, state: &str, generation: Option<u64>, loaded_seconds_ago: i64) {
    let now = chrono::Utc::now();
    let mut document = serde_json::json!({
        "updated_at": now.to_rfc3339(),
        "target": "w1",
        "pid": 4242,
        "state": state,
        "generation": generation,
        "store_version": "945077b5e1c74a0c",
        "loaded_at": (now - chrono::Duration::seconds(loaded_seconds_ago)).to_rfc3339(),
        "reason": serde_json::Value::Null,
        "attempt": 0,
        "next_attempt_at": serde_json::Value::Null,
    });
    if state == "backing_off" {
        // The sentence the authority itself produced on 2026-08-19, kept
        // verbatim: a rephrased copy would be a second vocabulary for one
        // condition.
        document["reason"] = serde_json::json!(
            "registry authority exited with exit status: 255: ssh: connect to host \
             100.120.25.24 port 22: Operation timed out"
        );
        document["attempt"] = serde_json::json!(3);
        document["next_attempt_at"] = serde_json::json!("2026-08-19T18:35:00+00:00");
    }
    let directory = home.join(".stado");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("resolver-state.json"),
        serde_json::to_string(&document).unwrap(),
    )
    .unwrap();
}

/// The `--json` report, parsed.
fn report(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out)).expect("resolver status --json prints one JSON object")
}

#[test]
fn resolver_status_is_ready_only_while_the_generation_it_holds_is_current() {
    let (api, api_port) = held_port();
    let (adapter, adapter_port) = held_port();
    let (home, storage) = fixture(&registry_document(7, api_port, adapter_port));
    let (home, storage) = (home.path(), storage.path());

    // Serving, holding the generation the authority publishes, inside its
    // max-stale window, with both declared binds held: ready, exit 0.
    publish_state(home, "serving", Some(7), 1);
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "w1", "--json"],
    );
    assert!(
        out.status.success(),
        "a ready resolver exited {:?}: {}{}",
        out.status.code(),
        stdout(&out),
        stderr(&out)
    );
    let document = report(&out);
    assert_eq!(document["verdict"], "ready");
    assert_eq!(document["state"], "serving");
    assert_eq!(document["generation"], 7);
    assert_eq!(document["stale"], false);
    assert_eq!(document["blockers"], serde_json::json!([]));
    assert_eq!(document["api"]["listening"], true);
    assert_eq!(document["adapters"][0]["listening"], true);
    assert_eq!(document["authority"]["source"], "local");
    assert_eq!(document["authority"]["reachable"], true);
    assert_eq!(document["authority"]["generation"], 7);
    assert_eq!(
        document["registry_staleness_seconds"],
        serde_json::Value::Null,
        "a fresh authority read reports no registry staleness"
    );

    // The authority advances to 9 and the resolver still holds 7: stale, and
    // the blocker names both numbers rather than the word "stale".
    std::fs::write(
        storage.join("registry.json"),
        registry_document(9, api_port, adapter_port),
    )
    .unwrap();
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "w1", "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a stale generation must exit 1: {}",
        stdout(&out)
    );
    let document = report(&out);
    assert_eq!(document["verdict"], "degraded");
    assert_eq!(document["stale"], true);
    assert_eq!(document["generation"], 7);
    assert_eq!(document["authority"]["generation"], 9);
    assert_eq!(
        document["blockers"],
        serde_json::json!([
            "the resolver holds service directory generation 7 and the authority publishes 9"
        ])
    );

    // Back on the authority's generation, but the snapshot is older than the
    // window this target declares. That is the condition the adapters refuse
    // on with "service directory cache is stale", so it cannot report ready.
    std::fs::write(
        storage.join("registry.json"),
        registry_document(7, api_port, adapter_port),
    )
    .unwrap();
    publish_state(home, "serving", Some(7), 400);
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "w1", "--json"],
    );
    assert_eq!(out.status.code(), Some(1));
    let document = report(&out);
    assert_eq!(document["stale"], true);
    assert_eq!(document["max_stale_seconds"], 15);
    assert!(
        document["blockers"][0]
            .as_str()
            .unwrap()
            .contains("past the 15s max-stale window this target declares"),
        "got: {}",
        document["blockers"]
    );

    drop((api, adapter));
}

#[test]
fn resolver_status_answers_while_the_resolver_is_stopped() {
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&registry_document(7, api_port, adapter_port));
    let (home, storage) = (home.path(), storage.path());

    // No state file and nothing listening: the whole point of the subcommand
    // is that this answers at all. On 2026-08-19 the equivalent question had
    // no answer anywhere in the product.
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "w1", "--json"],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a resolver that is not running must exit 1: {}",
        stdout(&out)
    );
    let document = report(&out);
    assert_eq!(document["verdict"], "down");
    assert_eq!(document["state"], "unpublished");
    assert_eq!(document["generation"], serde_json::Value::Null);
    assert_eq!(
        document["stale"], true,
        "holding no generation is not freshness"
    );
    assert_eq!(document["api"]["listening"], false);
    assert_eq!(document["adapters"][0]["listening"], false);
    let blockers = document["blockers"].as_array().unwrap().clone();
    let expected_state_file = home.join(".stado").join("resolver-state.json");
    assert_eq!(
        blockers[0].as_str().unwrap(),
        format!(
            "no resolver has published state at {}: nothing has served here since that file was \
             last removed",
            expected_state_file.display()
        )
    );
    assert_eq!(
        blockers[1].as_str().unwrap(),
        format!("nothing is listening on the resolution API at 127.0.0.1:{api_port}")
    );
    assert_eq!(
        blockers[2].as_str().unwrap(),
        format!(
            "nothing is listening on the stado-object-api adapter for consumer \
             stado-local-agent at 127.0.0.1:{adapter_port}"
        )
    );
    assert!(
        !expected_state_file.exists(),
        "status is a read: it must not create the file it reports missing"
    );

    // Backing off: the published reason is the authority's own sentence, and
    // it reaches the operator through this command instead of through 83 MiB
    // of stderr.
    publish_state(home, "backing_off", Some(7), 1);
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "w1", "--json"],
    );
    assert_eq!(out.status.code(), Some(1));
    let document = report(&out);
    assert_eq!(document["state"], "backing_off");
    assert_eq!(document["attempt"], 3);
    assert_eq!(document["next_attempt_at"], "2026-08-19T18:35:00+00:00");
    assert_eq!(
        document["reason"],
        "registry authority exited with exit status: 255: ssh: connect to host 100.120.25.24 \
         port 22: Operation timed out"
    );
    assert_eq!(
        document["blockers"][0],
        "the resolver reports state backing_off: registry authority exited with exit status: \
         255: ssh: connect to host 100.120.25.24 port 22: Operation timed out (failed attempt 3, \
         next read due 2026-08-19T18:35:00+00:00)"
    );

    // The non-JSON report carries the same facts, one per line.
    let out = stado(home, storage, &["resolver", "status", "--target", "w1"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.starts_with("resolver w1 state=backing_off verdict=down generation=7 stale=no\n"),
        "got: {text}"
    );
    assert!(
        text.contains(&format!("api 127.0.0.1:{api_port} not-listening\n")),
        "got: {text}"
    );
    assert!(
        text.contains("authority w1 source=local reachable generation=7\n"),
        "got: {text}"
    );
}

#[test]
fn resolver_status_refuses_a_target_it_cannot_report_on() {
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&registry_document(7, api_port, adapter_port));
    let (home, storage) = (home.path(), storage.path());

    // A registry host that declares no resolver policy is refused by that
    // fact, not answered with an empty report.
    let mut document: serde_json::Value =
        serde_json::from_str(&registry_document(7, api_port, adapter_port)).unwrap();
    document["targets"][0]
        .as_object_mut()
        .unwrap()
        .remove("service_resolver");
    std::fs::write(storage.join("registry.json"), document.to_string()).unwrap();
    let out = stado(home, storage, &["resolver", "status", "--target", "w1"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: registry target has no service_resolver configuration"),
        "got: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).is_empty(),
        "a refused status prints no report: {}",
        stdout(&out)
    );

    // A target the registry does not hold at all is named in the refusal.
    std::fs::write(
        storage.join("registry.json"),
        registry_document(7, api_port, adapter_port),
    )
    .unwrap();
    let out = stado(home, storage, &["resolver", "status", "--target", "ghost"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: resolver target \"ghost\" is not registered"),
        "got: {}",
        stderr(&out)
    );
}

/// The units the registry declares for this host, and what they run.
///
/// Neither the resolver nor the local dashboard was declared anywhere before
/// 2026-08-19: the resolver ran from a hand-installed
/// `~/Library/LaunchAgents/com.wisent.stado-resolver.plist` and the dashboard
/// serving 127.0.0.1:8765 from a plist that had been renamed
/// `...plist.retired-20260818` while something kept respawning it, so
/// `stado service list` showed neither and nobody could say what their restart
/// policy was. A declaration that carries its program is what makes the
/// document, rather than a file somebody installed by hand, the source of the
/// unit.
#[test]
fn the_shipped_registry_declares_what_the_resolver_and_dashboard_run() {
    let shipped: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("data")
                .join("registry.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let target = shipped["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|target| target["name"] == "operator-host")
        .expect("the shipped registry declares operator-host");
    let declared = |name: &str| -> serde_json::Value {
        target["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|service| service["name"] == name)
            .unwrap_or_else(|| panic!("{name} is declared"))
            .clone()
    };

    let resolver = declared("com.wisent.stado-resolver");
    assert_eq!(resolver["kind"], "launchd");
    assert_eq!(
        resolver["label"], "com.wisent.stado-resolver",
        "the declaration names the label the host already uses, so adopting it \
         cannot install a second resolver competing for one stable port"
    );
    assert_eq!(
        resolver["path"],
        "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.stado-resolver.plist"
    );
    assert_eq!(
        resolver["program"],
        "/Users/lukaszbartoszcze/.stado/bin/stado"
    );
    assert_eq!(
        resolver["args"],
        serde_json::json!(["resolver", "serve", "--target", "operator-host"])
    );

    let dashboard = declared("com.wisent.compute.service.com.wisent.stado-dashboard");
    assert_eq!(dashboard["kind"], "launchd");
    assert_eq!(
        dashboard["program"], "/Users/lukaszbartoszcze/.stado/stado-dashboard-launcher",
        "the program the live unit runs, not the one it would be tidier to declare"
    );
    assert_eq!(dashboard["args"], serde_json::json!([]));

    // Every declared program is absolute: `deploy::service::validate_program`
    // refuses anything else, so a `$HOME`-relative declaration would be a
    // declaration `service ensure` could never render.
    for service in target["services"].as_array().unwrap() {
        if let Some(program) = service["program"].as_str() {
            assert!(
                program.starts_with('/'),
                "{} declares a non-absolute program {program}",
                service["name"]
            );
        }
    }
}
