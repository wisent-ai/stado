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

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

/// The environment every command in this suite runs with. Shared, because the
/// transport tests leave two of these commands running instead of waiting on
/// them, and a second copy of this environment is a second fixture.
fn configured(home: &Path, storage: &Path, args: &[&str]) -> Command {
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
    cmd
}

fn stado(home: &Path, storage: &Path, args: &[&str]) -> Output {
    configured(home, storage, args)
        .output()
        .expect("stado binary runs")
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
    // 18765 is a port nothing in this suite serves: the status tests ask what
    // the resolver reports, never what the declared service answers.
    proxy_registry_document(generation, api_port, adapter_port, 18765, None)
}

/// The name this machine answers to, normalized the way registry-v2 requires.
///
/// `resolver serve` identifies its own host by hostname and refuses a registry
/// that does not name it -- `resolver host "..." has no registry target
/// identity`, which is exactly what a fixture naming only `w1.local` earns.
/// Nothing overrides this from the environment, so the fixture names the
/// machine the test runs on.
///
/// Lowercase because the document contract refuses anything else:
/// `registry.targets[0].hostnames[1]: must be normalized as '...'`. macOS
/// answers `hostname` with capitals, so an unnormalized alias trades one
/// refusal for another.
fn this_host() -> String {
    let named = Command::new("hostname").output().expect("hostname runs");
    String::from_utf8_lossy(&named.stdout)
        .trim()
        .to_ascii_lowercase()
}

/// The same document with the two things the transport tests vary: the port the
/// declared service really listens on, and whether the active host is this
/// machine (`None`) or a host reached over SSH (`Some(destination)`).
///
/// The active host declares the service either way, because the route names a
/// `managed_service` and the contract is that its host declares it.
fn proxy_registry_document(
    generation: u64,
    api_port: u16,
    adapter_port: u16,
    upstream_port: u16,
    remote: Option<&str>,
) -> String {
    let service = serde_json::json!([{
        "name": "stado-object-api",
        "unit": "",
        "label": "com.wisent.compute.service.stado-object-api",
        "path": "/Users/u/Library/LaunchAgents/com.wisent.compute.service.stado-object-api.plist",
        "kind": "launchd",
        "managed_since": "2026-08-01T00:00:00+00:00",
    }]);
    let mut targets = vec![serde_json::json!({
        "name": "w1",
        "kind": "local",
        "ssh": "u@10.0.0.1",
        "release_platform": "darwin-arm64",
        "hostnames": ["w1.local", this_host()],
        "slots": 1,
        "services": service.clone(),
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
    })];
    let active = match remote {
        None => "w1",
        Some(destination) => {
            targets[0]["services"] = serde_json::json!([]);
            // Exactly one SSH path, so path selection returns it without
            // probing and the transport under test is the next thing to run.
            targets.push(serde_json::json!({
                "name": "w2",
                "kind": "local",
                "ssh": destination,
                "release_platform": "darwin-arm64",
                "hostnames": ["w2.local"],
                "slots": 1,
                "services": service,
            }));
            "w2"
        }
    };
    let mut endpoints = serde_json::Map::new();
    endpoints.insert(
        active.to_string(),
        serde_json::json!({"url": format!("http://127.0.0.1:{upstream_port}")}),
    );
    serde_json::json!({
        "schema_version": 2,
        "targets": targets,
        "coordinators": [],
        "service_directory": {
            "authority": {"target": "w1", "command": "/opt/stado/bin/stado"},
            "generation": generation,
            "services": {
                "stado-object-api": {
                    "managed_service": "stado-object-api",
                    "active_host": active,
                    "endpoints": endpoints,
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

/// What the repository ships as a registry, and what it must not ship.
///
/// This test used to assert that the shipped `data/registry.json` declared a
/// target called `operator-host` carrying the resolver and dashboard units,
/// down to absolute paths under `/Users/lukaszbartoszcze`. It had been failing
/// since the file was created: `d3dbabdf fix: ship an empty public registry
/// seed` (2026-08-20) shipped it with `"targets": []`, because a public
/// repository has no business carrying one operator's hosts, their launchd
/// paths, or the ports they serve. A test demanding that data back is a test
/// asking for the leak to return.
///
/// The concern behind it survives and is worth keeping: before 2026-08-19 the
/// resolver ran from a hand-installed
/// `~/Library/LaunchAgents/com.wisent.stado-resolver.plist` and the dashboard
/// from a plist renamed `...plist.retired-20260818` while something kept
/// respawning it, so `stado service list` showed neither and nobody could say
/// what their restart policy was. What fixed that is a declaration carrying its
/// program -- and that declaration lives in the canonical registry in Stado
/// storage, which is where `stado service ensure` reads it and where the fleet
/// tests reach it. It is not in this file and cannot be read from here.
///
/// So what is checkable from a public checkout is what this now checks: the
/// seed is a valid registry-v2 document by the product's own validator, and it
/// names no host, so a fresh install cannot silently adopt somebody else's
/// declaration.
#[test]
fn the_shipped_registry_seed_names_no_host_and_still_validates() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("registry.json");
    let seed = std::fs::read_to_string(&path).unwrap();
    let document: serde_json::Value = serde_json::from_str(&seed).unwrap();
    assert_eq!(document["schema_version"], 2);
    assert_eq!(
        document["targets"],
        serde_json::json!([]),
        "the public seed carries a host: {seed}"
    );
    assert_eq!(
        document["coordinators"],
        serde_json::json!([]),
        "the public seed carries a coordinator: {seed}"
    );

    let (home, storage) = fixture(&seed);
    let (home, storage) = (home.path(), storage.path());

    // The product's own validator, on the file the repository ships.
    let out = stado(
        home,
        storage,
        &["registry", "validate", path.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "the shipped seed does not validate: {}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("valid registry: "),
        "got: {}",
        stdout(&out)
    );

    // And a host that installs it holds no declaration to serve.
    let out = stado(
        home,
        storage,
        &["resolver", "status", "--target", "operator-host"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("Error: resolver target \"operator-host\" is not registered"),
        "got: {}",
        stderr(&out)
    );
}

/// A product process this test leaves running, killed when the test ends
/// however it ends -- a panic must not leak a listener into the next test.
///
/// Its output is kept, not discarded: a command that refuses to start says why
/// on stderr, and a test that throws that away can only report the symptom it
/// happened to be waiting on.
struct Serving {
    child: std::process::Child,
    output: std::path::PathBuf,
}

impl Serving {
    fn start(home: &Path, storage: &Path, args: &[&str]) -> Self {
        Self::spawn(home, args, configured(home, storage, args))
    }

    /// The same, with `ahead` in front of `PATH`, which is what lets a test
    /// answer the resolver's `ssh` with one of its own.
    fn start_behind(home: &Path, storage: &Path, args: &[&str], ahead: &Path) -> Self {
        let mut command = configured(home, storage, args);
        let inherited = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{inherited}", ahead.display()));
        Self::spawn(home, args, command)
    }

    fn spawn(home: &Path, args: &[&str], mut command: Command) -> Self {
        let output = home.join(format!("serving-{}.log", args.join("-")));
        let log = std::fs::File::create(&output).expect("output file for a served command");
        let errors = log.try_clone().expect("one file for both streams");
        let child = command
            .stdout(std::process::Stdio::from(log))
            .stderr(std::process::Stdio::from(errors))
            .spawn()
            .expect("stado binary spawns");
        Self { child, output }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Everything the process has written so far, for a failure message.
    fn said(&self) -> String {
        std::fs::read_to_string(&self.output).unwrap_or_default()
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait, bounded, until something accepts on a loopback port.
fn wait_until_listening(port: u16, budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// One HTTP/1.1 GET on a fresh connection, and whatever comes back. The test
/// holds the client end, so the answer is the declared service's own.
fn http_get(port: u16, path: &str, budget: Duration) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_secs(5),
    )
    .map_err(|error| format!("connect: {error}"))?;
    stream.set_read_timeout(Some(budget)).unwrap();
    stream.set_write_timeout(Some(budget)).unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .map_err(|error| format!("write: {error}"))?;
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .map_err(|error| format!("read: {error}"))?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Direct children of a process, by program name -- the count that used to grow
/// with traffic.
fn children_named(pid: u32, program: &str) -> usize {
    let listed = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("pgrep runs");
    String::from_utf8_lossy(&listed.stdout)
        .split_whitespace()
        .filter(|child| {
            let named = Command::new("ps")
                .args(["-p", child, "-o", "comm="])
                .output()
                .expect("ps runs");
            String::from_utf8_lossy(&named.stdout).contains(program)
        })
        .count()
}

/// The highest count of `program` children `pid` held while `work` ran.
fn peak_children(pid: u32, program: &'static str, window: Duration, work: impl FnOnce()) -> usize {
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let sampler = {
        let (peak, stop) = (peak.clone(), stop.clone());
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + window;
            while !stop.load(std::sync::atomic::Ordering::Relaxed)
                && std::time::Instant::now() < deadline
            {
                peak.fetch_max(
                    children_named(pid, program),
                    std::sync::atomic::Ordering::Relaxed,
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        })
    };
    work();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    sampler.join().expect("sampler thread joins");
    peak.load(std::sync::atomic::Ordering::Relaxed)
}

/// A resolver in front of the real object API, both from the fixture's store.
fn proxied_object_api(
    home: &Path,
    storage: &Path,
    upstream_port: u16,
    adapter_port: u16,
) -> (Serving, Serving) {
    let upstream = Serving::start(
        home,
        storage,
        &[
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &upstream_port.to_string(),
        ],
    );
    assert!(
        wait_until_listening(upstream_port, Duration::from_secs(60)),
        "the object API never bound 127.0.0.1:{upstream_port}: {}",
        upstream.said()
    );
    let resolver = Serving::start(home, storage, &["resolver", "serve", "--target", "w1"]);
    assert!(
        wait_until_listening(adapter_port, Duration::from_secs(60)),
        "the resolver never bound its declared adapter 127.0.0.1:{adapter_port}: {}",
        resolver.said()
    );
    (upstream, resolver)
}

/// What the whole mechanism is for: a client that speaks to its own loopback
/// port reaches the object API, and the API's own answer comes back.
///
/// Both answers are the ones a hand run of this fixture produced: `/healthz`
/// is served with no credential, and an object read from a store with no
/// Skarbiec configuration is refused by the API itself. Carrying a refusal
/// verbatim is as much the contract as carrying a body.
#[test]
fn the_resolver_carries_the_object_api_answer_end_to_end() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document(
        7,
        api_port,
        adapter_port,
        upstream_port,
        None,
    ));
    let (home, storage) = (home.path(), storage.path());
    let (_upstream, mut resolver) = proxied_object_api(home, storage, upstream_port, adapter_port);

    let health = http_get(adapter_port, "/healthz", Duration::from_secs(30))
        .expect("the adapter answers a health read");
    assert!(
        health.starts_with("HTTP/1.1 200 OK"),
        "the object API's status line did not come back: {health}"
    );
    assert!(
        health.contains("\"ok\":true"),
        "the object API's body did not come back: {health}"
    );

    let object = http_get(
        adapter_port,
        "/api/object?uri=stado%3A%2F%2Fprobierz%2Fcapacity%2Flocal-probe.json",
        Duration::from_secs(30),
    )
    .expect("the adapter answers an object read");
    assert!(
        object.starts_with("HTTP/1.1 503"),
        "an object read was not carried to the API: {object}"
    );
    assert!(
        object.contains("object authorization unavailable"),
        "the API's own refusal was not carried back: {object}"
    );
    assert!(
        resolver.running(),
        "the resolver died while carrying two reads"
    );
}

/// Twenty-four reads at once cost the resolver no child process, and every one
/// of them is answered.
///
/// This is the shape that killed the fleet on 2026-09-02: the transport forked
/// a client process per request, so a release's fan-out walked the resolver
/// into its own descriptor budget -- `accept failed: Too many open files`,
/// launchd counted 166 restarts, and every restart dropped the connections in
/// flight.
#[test]
fn concurrent_reads_cost_the_resolver_no_process_per_request() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document(
        7,
        api_port,
        adapter_port,
        upstream_port,
        None,
    ));
    let (home, storage) = (home.path(), storage.path());
    let (_upstream, mut resolver) = proxied_object_api(home, storage, upstream_port, adapter_port);

    let mut answers = Vec::new();
    let peak = peak_children(resolver.pid(), "stado", Duration::from_secs(30), || {
        let readers: Vec<_> = (0..24)
            .map(|_| {
                std::thread::spawn(move || {
                    http_get(adapter_port, "/healthz", Duration::from_secs(30))
                })
            })
            .collect();
        answers = readers
            .into_iter()
            .map(|reader| reader.join().expect("reader thread joins"))
            .collect();
    });

    for (index, answer) in answers.iter().enumerate() {
        let answer = answer
            .as_ref()
            .unwrap_or_else(|error| panic!("read {index} of 24 was not answered: {error}"));
        assert!(
            answer.starts_with("HTTP/1.1 200 OK"),
            "read {index} of 24 got: {answer}"
        );
    }
    assert_eq!(
        peak, 0,
        "the resolver spawned {peak} child process(es) for 24 reads to a service on this host"
    );
    assert!(resolver.running(), "the resolver died under 24 reads");
}

/// One forward carries every request to a remote active host, however many
/// requests there are.
///
/// The forward cannot open here: 192.0.2.1 is reserved for documentation and
/// answers nothing, so each attempt ends when `ssh` gives up. That is what
/// makes the count decisive rather than incidental -- while six reads are in
/// flight against a destination that never answers, the transport that shipped
/// before this held six `ssh` processes, one per read. This one holds one,
/// because a forward is per destination and the pool opens it under a lock.
#[test]
fn one_ssh_forward_serves_every_request_to_a_remote_active_host() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document(
        7,
        api_port,
        adapter_port,
        upstream_port,
        Some("u@192.0.2.1"),
    ));
    let (home, storage) = (home.path(), storage.path());
    let mut resolver = Serving::start(home, storage, &["resolver", "serve", "--target", "w1"]);
    assert!(
        wait_until_listening(adapter_port, Duration::from_secs(60)),
        "the resolver never bound its declared adapter 127.0.0.1:{adapter_port}: {}",
        resolver.said()
    );

    // The clients give up before the forward does: this test asks how many
    // processes the resolver holds while they wait, not what they are told.
    let peak = peak_children(resolver.pid(), "ssh", Duration::from_secs(12), || {
        let readers: Vec<_> = (0..6)
            .map(|_| {
                std::thread::spawn(move || {
                    let _ = http_get(adapter_port, "/healthz", Duration::from_secs(5));
                })
            })
            .collect();
        for reader in readers {
            reader.join().expect("reader thread joins");
        }
        std::thread::sleep(Duration::from_secs(6));
    });

    assert!(
        peak >= 1,
        "no forward was ever opened, so this test measured nothing: {}",
        resolver.said()
    );
    assert_eq!(
        peak, 1,
        "the resolver held {peak} ssh processes for 6 reads to one destination"
    );
    assert!(
        resolver.running(),
        "the resolver died while a forward could not open"
    );
    assert!(
        wait_until_listening(api_port, Duration::from_secs(5)),
        "the resolver stopped answering on its own API port"
    );
}

/// A forward that cannot open refuses the connection with the gateway sentence
/// and the resolver keeps serving.
///
/// 127.0.0.1:22 refuses immediately on this fleet's Macs, so the refusal is
/// reached in milliseconds rather than at a connect timeout.
#[test]
fn a_forward_that_cannot_open_refuses_the_read_instead_of_hanging() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document(
        7,
        api_port,
        adapter_port,
        upstream_port,
        Some("u@127.0.0.1"),
    ));
    let (home, storage) = (home.path(), storage.path());
    let mut resolver = Serving::start(home, storage, &["resolver", "serve", "--target", "w1"]);
    assert!(
        wait_until_listening(adapter_port, Duration::from_secs(60)),
        "the resolver never bound its declared adapter 127.0.0.1:{adapter_port}: {}",
        resolver.said()
    );

    let refusal = http_get(adapter_port, "/healthz", Duration::from_secs(60))
        .expect("a refusal is an answer, so the read completes");
    assert!(
        refusal.starts_with("HTTP/1.1 502 Bad Gateway"),
        "an unopenable forward answered: {refusal}"
    );
    assert!(
        refusal.contains("upstream unavailable"),
        "the refusal carried no reason: {refusal}"
    );
    assert!(
        resolver.running(),
        "the resolver died on a forward it could not open"
    );
    assert!(
        wait_until_listening(api_port, Duration::from_secs(5)),
        "the resolver stopped answering on its own API port"
    );
}

/// The same document with a second SSH path on the active host.
///
/// Every host in this fleet carries one -- `charless-mac-mini` declares `lan`
/// beside its tailnet primary -- and no transport test had one, because the
/// fixture above states in a comment that it declares exactly one path so that
/// "path selection returns it without probing". Path selection is the thing
/// that probes, so the branch every real host takes was the branch nothing
/// ran.
fn proxy_registry_document_with_fallback(
    generation: u64,
    api_port: u16,
    adapter_port: u16,
    upstream_port: u16,
    remote: &str,
    fallback: &str,
) -> String {
    let mut document: serde_json::Value = serde_json::from_str(&proxy_registry_document(
        generation,
        api_port,
        adapter_port,
        upstream_port,
        Some(remote),
    ))
    .expect("the fixture is a document");
    document["targets"][1]["ssh_fallbacks"] = serde_json::json!([
        {"name": "lan", "destination": fallback},
    ]);
    document.to_string()
}

/// An `ssh` of this test's own, ahead of the resolver's `PATH`, that answers
/// the way OpenSSH answers behind a live control master.
///
/// It records what it was asked for and always exits 0:
///
/// * `<destination> true` -- one line in `probes`. That is the path probe, and
///   counting it is how this suite can say whether it runs once per forward or
///   once per request.
/// * `-N -L 127.0.0.1:<local>:<host>:<port> <destination>` -- one line in
///   `handoffs`, then exit 0 without binding anything. This is the multiplex
///   hand-off verbatim: the slave gives the listener to the master and is gone
///   before the master has bound it, so the exit says nothing about the
///   forward. [`bind_handed_off_forwards`] plays the master.
fn fake_ssh(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let probes = dir.join("probes");
    let handoffs = dir.join("handoffs");
    let script = dir.join("ssh");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
forward=""
prev=""
for arg in "$@"; do
  case "$prev" in -L) forward="$arg";; esac
  prev="$arg"
done
if [ -n "$forward" ]; then
  echo "$forward" >> "{handoffs}"
else
  echo probe >> "{probes}"
fi
exit 0
"#,
            handoffs = handoffs.display(),
            probes = probes.display(),
        ),
    )
    .expect("the fake ssh is written");
    let mut mode = std::fs::metadata(&script)
        .expect("the fake ssh exists")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&script, mode).expect("the fake ssh is executable");
    (probes, handoffs)
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Play the control master: bind every `-L` listener the slave handed over and
/// relay it to the address that `-L` named, for as long as the returned flag
/// says to.
///
/// The bind happens after the slave has already exited, which is the ordering
/// this whole test exists for.
fn bind_handed_off_forwards(
    handoffs: std::path::PathBuf,
) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let mut bound: Vec<String> = Vec::new();
        while !flag.load(std::sync::atomic::Ordering::Relaxed) {
            for spec in lines(&handoffs) {
                if bound.contains(&spec) {
                    continue;
                }
                // `127.0.0.1:<local>:<host>:<port>`
                let field: Vec<&str> = spec.split(':').collect();
                if field.len() != 4 {
                    continue;
                }
                let (local, upstream) = (
                    format!("{}:{}", field[0], field[1]),
                    format!("{}:{}", field[2], field[3]),
                );
                let Ok(listener) = TcpListener::bind(&local) else {
                    continue;
                };
                bound.push(spec);
                let flag = flag.clone();
                std::thread::spawn(move || {
                    for accepted in listener.incoming() {
                        if flag.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        let Ok(mut client) = accepted else { return };
                        let Ok(mut server) = TcpStream::connect(&upstream) else {
                            continue;
                        };
                        let (mut client_up, mut server_up) = (
                            client.try_clone().expect("client half"),
                            server.try_clone().expect("server half"),
                        );
                        std::thread::spawn(move || {
                            let _ = std::io::copy(&mut client_up, &mut server_up);
                            let _ = server_up.shutdown(std::net::Shutdown::Write);
                        });
                        std::thread::spawn(move || {
                            let _ = std::io::copy(&mut server, &mut client);
                            let _ = client.shutdown(std::net::Shutdown::Write);
                        });
                    }
                });
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    stop
}

/// A forward handed to a live control master carries the read; it is not a
/// refusal.
///
/// `ssh -N -L` behind a master is a multiplex slave: it asks the master for the
/// listener and exits 0 immediately, before the master has bound it. Judging
/// the forward by that exit answered the client `HTTP 502 upstream
/// unavailable` -- 56 of them in one resolver lifetime on this workstation, 41
/// on `stado-object-api`, each one a release-pipeline object read that died
/// while the transport underneath it was healthy. The port is what proves a
/// forward, so the read must come back.
#[test]
fn a_forward_handed_to_a_control_master_carries_the_read() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document_with_fallback(
        7,
        api_port,
        adapter_port,
        upstream_port,
        "u@192.0.2.1",
        "u@192.0.2.2",
    ));
    let (home, storage) = (home.path(), storage.path());
    let (_probes, handoffs) = fake_ssh(home);

    let upstream = Serving::start(
        home,
        storage,
        &[
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &upstream_port.to_string(),
        ],
    );
    assert!(
        wait_until_listening(upstream_port, Duration::from_secs(60)),
        "the object API never bound 127.0.0.1:{upstream_port}: {}",
        upstream.said()
    );
    let stop = bind_handed_off_forwards(handoffs);
    let mut resolver = Serving::start_behind(
        home,
        storage,
        &["resolver", "serve", "--target", "w1"],
        home,
    );
    assert!(
        wait_until_listening(adapter_port, Duration::from_secs(60)),
        "the resolver never bound its declared adapter 127.0.0.1:{adapter_port}: {}",
        resolver.said()
    );

    let health = http_get(adapter_port, "/healthz", Duration::from_secs(60))
        .expect("the adapter answers a health read");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        !health.starts_with("HTTP/1.1 502"),
        "a forward the control master accepted was refused as dead: {health}\n{}",
        resolver.said()
    );
    assert!(
        health.starts_with("HTTP/1.1 200 OK") && health.contains("\"ok\":true"),
        "the object API's answer did not come back over the handed-off forward: {health}\n{}",
        resolver.said()
    );
    assert!(
        resolver.running(),
        "the resolver died carrying a handed-off forward"
    );
}

/// A host that declares a fallback SSH path is probed once per forward, not
/// once per request.
///
/// The probe is an `ssh <destination> true` process bounded at twenty seconds.
/// Running it in `proxy_connection` put one of them in front of every accepted
/// connection to every host in this fleet, because every host in this fleet
/// declares a fallback -- the process per request this transport was rewritten
/// to remove, and the thing that leaves a control master up for the forward to
/// be handed to.
#[test]
fn a_fallback_path_is_probed_once_per_forward_not_once_per_request() {
    let upstream_port = free_port();
    let api_port = free_port();
    let adapter_port = free_port();
    let (home, storage) = fixture(&proxy_registry_document_with_fallback(
        7,
        api_port,
        adapter_port,
        upstream_port,
        "u@192.0.2.1",
        "u@192.0.2.2",
    ));
    let (home, storage) = (home.path(), storage.path());
    let (probes, handoffs) = fake_ssh(home);

    let upstream = Serving::start(
        home,
        storage,
        &[
            "dashboard",
            "--bind",
            "127.0.0.1",
            "--port",
            &upstream_port.to_string(),
        ],
    );
    assert!(
        wait_until_listening(upstream_port, Duration::from_secs(60)),
        "the object API never bound 127.0.0.1:{upstream_port}: {}",
        upstream.said()
    );
    let stop = bind_handed_off_forwards(handoffs);
    let mut resolver = Serving::start_behind(
        home,
        storage,
        &["resolver", "serve", "--target", "w1"],
        home,
    );
    assert!(
        wait_until_listening(adapter_port, Duration::from_secs(60)),
        "the resolver never bound its declared adapter 127.0.0.1:{adapter_port}: {}",
        resolver.said()
    );

    let readers: Vec<_> = (0..8)
        .map(|_| {
            std::thread::spawn(move || http_get(adapter_port, "/healthz", Duration::from_secs(60)))
        })
        .collect();
    let answers: Vec<_> = readers
        .into_iter()
        .map(|reader| reader.join().expect("reader thread joins"))
        .collect();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);

    for (index, answer) in answers.iter().enumerate() {
        let answer = answer
            .as_ref()
            .unwrap_or_else(|error| panic!("read {index} of 8 was not answered: {error}"));
        assert!(
            answer.starts_with("HTTP/1.1 200 OK"),
            "read {index} of 8 got: {answer}\n{}",
            resolver.said()
        );
    }
    let probed = lines(&probes).len();
    assert!(
        probed >= 1,
        "no path was ever probed, so this test measured nothing: {}",
        resolver.said()
    );
    assert_eq!(
        probed, 1,
        "the resolver probed the active host's SSH paths {probed} times for 8 reads \
         over one forward"
    );
    assert!(resolver.running(), "the resolver died under 8 reads");
}
