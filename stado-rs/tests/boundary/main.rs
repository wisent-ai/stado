//! Authorization-boundary recovery on the object plane.
//!
//! On 2026-08-19 `com.wisent.always-on.stado-object-api` — which is
//! `stado dashboard --bind 127.0.0.1 --port 8765` — answered
//! `503 {"error":"object authorization unavailable"}` to the whole fleet
//! because one slow vault read at startup shut the `object` boundary and
//! nothing ever revalidated it. Clearing it needed a privileged LaunchDaemon
//! restart, which is exactly what the product's own recovery path cannot do.
//!
//! This test drives the product's own dashboard entry point
//! (`stado::dashboard::serve`, what `stado dashboard` calls) on loopback,
//! against a tempdir local storage backend and a stand-in Skarbiec broker
//! whose item listing is refused exactly once. It defends: the exact 503 body,
//! recovery without any restart once the cooldown elapses, the verifier's own
//! sentence in `last_error`, and the cooldown itself — repeated requests
//! inside it must not turn into a vault sweep per request.
//!
//! Isolation is environmental, as everywhere else in this suite:
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>, a set-but-
//! missing STADO_CONFIG, every Skarbiec URL pointed at loopback (the object
//! verifier at the stand-in broker, every other verifier at a dead port), and
//! owner-only grant files inside the temp dir. Nothing here can reach the
//! operator's real vault, registry or fleet.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The body every object route answers while its boundary is shut. Copied from
/// the wire, not from the source: the fleet's clients and the incident
/// vocabulary both match on this exact string.
const OBJECT_UNAVAILABLE: &str = r#"{"error":"object authorization unavailable"}"#;

/// The refusal the stand-in broker answers the first item listing with. It
/// travels all the way into `last_error`, which is the point: an operator
/// reading the boundary state must see the verifier's own words.
const BROKER_REFUSAL: &str = r#"{"error":"skarbiec vault broker reset the connection"}"#;

/// What `SkarbiecError::Response` makes of [`BROKER_REFUSAL`], and therefore
/// what `/api/state.json` must publish as the `object` boundary's reason.
const OBJECT_LAST_ERROR: &str =
    r#"Skarbiec returned HTTP 503: {"error":"skarbiec vault broker reset the connection"}"#;

/// The namespace and key this test reads. `probierz` is one of the active
/// object namespaces, so the gateway's own configuration accepts it.
const NAMESPACE: &str = "probierz";
const KEY: &str = "data/probe.json";

/// Seconds a shut boundary waits before it may be revalidated again. Long
/// enough that the storm probe below is decided by the cooldown and not by
/// scheduling luck, short enough to keep this test a few seconds.
const COOLDOWN_SECONDS: u64 = 3;

/// The Skarbiec item holding one namespace's bearer, as
/// `config::parse_object_api_namespaces` requires it to be named.
fn verifier_item(namespace: &str) -> String {
    if namespace == "wisent-backend" {
        "wisent-backend-object-client".to_string()
    } else {
        format!("{namespace}-object-api")
    }
}

/// The bearer the stand-in broker holds for one namespace.
fn namespace_token(namespace: &str) -> String {
    format!("{}-token", verifier_item(namespace))
}

/// The `WC_OBJECT_API_NAMESPACES` document: every active namespace, each with
/// its own item and one `data/` subtree. A missing active namespace is a
/// configuration problem the verifier reports instead of reaching the vault.
fn namespaces_document() -> String {
    let entries = stado::config::ACTIVE_OBJECT_NAMESPACES
        .iter()
        .map(|namespace| {
            format!(
                r#""{namespace}": {{"item": "{}", "prefixes": ["data/"]}}"#,
                verifier_item(namespace)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{entries}}}")
}

/// An owner-only grant file. `skarbiec::read_grant` refuses anything a group
/// or other user can read, so the mode is part of the fixture.
fn write_grant(path: &Path, token: &str) {
    std::fs::write(path, token).expect("grant file is writable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("grant file takes owner-only mode");
}

/// One HTTP answer, reduced to what a contract is written against.
struct Answer {
    status: u16,
    body: String,
}

/// Read one HTTP message off `stream`: the head, then `Content-Length` bytes.
fn read_message(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => return None,
            Ok(_) => raw.push(byte[0]),
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    if length > 0 && stream.read_exact(&mut body).is_err() {
        return None;
    }
    Some((head, String::from_utf8_lossy(&body).into_owned()))
}

fn write_response(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// A stand-in Skarbiec broker.
///
/// `POST /v1/items/list` is refused exactly once and answers the full item set
/// afterwards — a transient reset, which is what shut the real boundary.
/// `POST /v1/items/read` always answers the item's bearer, so the difference
/// between the closed and the recovered listener is only the boundary verdict.
struct FakeVault {
    addr: SocketAddr,
    /// How many item listings the broker has been asked for. Only boundary
    /// validation lists items, so this counts vault sweeps.
    listings: Arc<AtomicUsize>,
}

impl FakeVault {
    fn spawn() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("stand-in broker binds loopback");
        let addr = listener
            .local_addr()
            .expect("stand-in broker has an address");
        let listings = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&listings);
        std::thread::Builder::new()
            .name("stand-in-skarbiec".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let counter = Arc::clone(&counter);
                    std::thread::spawn(move || {
                        let Some((head, body)) = read_message(&mut stream) else {
                            return;
                        };
                        let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                        match target.as_str() {
                            "/v1/items/list" => {
                                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                                    write_response(
                                        &mut stream,
                                        503,
                                        "Service Unavailable",
                                        BROKER_REFUSAL,
                                    );
                                    return;
                                }
                                let items = stado::config::ACTIVE_OBJECT_NAMESPACES
                                    .iter()
                                    .map(|namespace| {
                                        format!(
                                            r#"{{"id": "{}", "deleted": false}}"#,
                                            verifier_item(namespace)
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                write_response(&mut stream, 200, "OK", &format!("[{items}]"));
                            }
                            "/v1/items/read" => {
                                let request: Value =
                                    serde_json::from_str(&body).unwrap_or(Value::Null);
                                let id = request
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                write_response(
                                    &mut stream,
                                    200,
                                    "OK",
                                    &format!(r#"{{"value": "{id}-token"}}"#),
                                );
                            }
                            _ => write_response(&mut stream, 404, "Not Found", "{}"),
                        }
                    });
                }
            })
            .expect("stand-in broker thread starts");
        Self { addr, listings }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn listings(&self) -> usize {
        self.listings.load(Ordering::SeqCst)
    }
}

/// One GET against the dashboard, with the loopback `Host` its guard requires.
fn get(addr: SocketAddr, target: &str, bearer: Option<&str>) -> Answer {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        match TcpStream::connect(addr) {
            Ok(stream) => break stream,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "dashboard never accepted a loopback connection: {error}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .expect("read timeout is settable");
    let mut request = format!("GET {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(bearer) = bearer {
        request.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("dashboard accepts the request");
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .expect("dashboard answers and closes");
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .expect("the answer has a head and a body");
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .expect("the status line carries a code");
    Answer {
        status,
        body: body.to_string(),
    }
}

/// The `object` boundary as `/api/state.json` publishes it.
fn object_boundary_state(addr: SocketAddr) -> Value {
    let answer = get(addr, "/api/state.json", None);
    assert_eq!(answer.status, 200, "state document: {}", answer.body);
    let document: Value =
        serde_json::from_str(&answer.body).expect("the state document is JSON: {answer.body}");
    document["boundaries"]["object"].clone()
}

/// A shut object boundary revalidates itself, once per cooldown, and the fleet
/// gets its object plane back without a privileged unit restart.
#[test]
fn a_shut_object_boundary_recovers_without_a_restart() {
    let storage = tempfile::TempDir::new().expect("temp storage root");
    let vault = FakeVault::spawn();
    let coordinator_grant = storage.path().join("coordinator-grant");
    let object_grant = storage.path().join("object-verifier-grant");
    write_grant(&coordinator_grant, "coordinator-grant-value");
    write_grant(&object_grant, "object-verifier-grant-value");

    std::env::set_var("WC_STORAGE_BACKEND", "local");
    std::env::set_var("WC_LOCAL_STORAGE_PATH", storage.path());
    // A set-but-missing STADO_CONFIG disables config-file discovery.
    std::env::set_var("STADO_CONFIG", storage.path().join("no-such-config.json"));
    std::env::remove_var("COMPUTE_API_KEY");
    std::env::remove_var("COMPUTE_API_URL");
    std::env::remove_var("WC_PROFILES_DIR");
    // Every boundary but `object` is pointed at a dead loopback port: they must
    // fail, they must fail instantly, and they must never reach a real vault.
    for variable in [
        "WC_SKARBIEC_URL",
        "WC_RELEASE_SKARBIEC_URL",
        "WC_MACHINE_SKARBIEC_URL",
        "WC_SERVICE_SKARBIEC_URL",
        "WC_RATE_LIMIT_SKARBIEC_URL",
        "WC_INTEGRATION_SKARBIEC_URL",
        "WC_INTEGRATION_PROVIDER_SKARBIEC_URL",
    ] {
        std::env::set_var(variable, "http://127.0.0.1:1");
    }
    std::env::set_var("WC_SKARBIEC_TOKEN_FILE", &coordinator_grant);
    std::env::set_var("WC_OBJECT_SKARBIEC_URL", vault.url());
    std::env::set_var("WC_OBJECT_SKARBIEC_TOKEN_FILE", &object_grant);
    std::env::set_var("WC_OBJECT_API_NAMESPACES", namespaces_document());
    // One attempt, so the single refusal below is the startup verdict rather
    // than the first of three retries.
    std::env::set_var("WC_DASHBOARD_BOUNDARY_ATTEMPTS", "1");
    std::env::set_var(
        "WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS",
        COOLDOWN_SECONDS.to_string(),
    );
    std::env::set_var("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS", "20");

    // The dashboard binds a fixed port, so the port is chosen here and
    // released; the listener below claims it immediately.
    let addr = {
        let probe = TcpListener::bind(("127.0.0.1", 0)).expect("a free loopback port exists");
        probe.local_addr().expect("the probe has an address")
    };
    // The listener runs on its own OS thread, for the reason the production
    // refresh loop does: `serve`'s future takes `Option<&str>` and spawning it
    // trips the `&str` lifetime-generalization issue (Send "not general
    // enough"). `block_on` never asks the future to be Send.
    let port = i64::from(addr.port());
    std::thread::Builder::new()
        .name("boundary-dashboard".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("dashboard runtime");
            // Exactly what `stado dashboard --bind 127.0.0.1 --port <port>` runs.
            let outcome = runtime.block_on(stado::dashboard::serve(
                Some("127.0.0.1"),
                Some(port),
                false,
            ));
            if let Err(error) = outcome {
                eprintln!("[test] dashboard exited: {error}");
            }
        })
        .expect("dashboard thread starts");

    let uri = format!("stado://{NAMESPACE}/{KEY}");
    let target = format!("/api/object?uri={uri}");
    let bearer = namespace_token(NAMESPACE);

    // The listener accepts only after startup validation has recorded every
    // verdict, so the first answer is decided by the refused sweep above.
    let refused = get(addr, &target, Some(&bearer));
    assert_eq!(refused.status, 503, "body: {}", refused.body);
    assert_eq!(refused.body, OBJECT_UNAVAILABLE);
    assert_eq!(
        vault.listings(),
        1,
        "startup validation swept the vault exactly once"
    );

    // The reason is the verifier's own sentence, published where an operator
    // already has the `view` permission.
    let closed = object_boundary_state(addr);
    assert_eq!(closed["ready"], Value::Bool(false));
    assert_eq!(
        closed["last_error"],
        Value::String(OBJECT_LAST_ERROR.into())
    );
    assert!(
        closed["checked_at"].is_string(),
        "a closed verdict is timestamped: {closed}"
    );

    // Liveness answers before authorization, so it stays flat booleans: no
    // vault item, grant or endpoint leaks to an unauthenticated prober.
    let health = get(addr, "/healthz", None);
    assert_eq!(health.status, 200, "body: {}", health.body);
    let health: Value = serde_json::from_str(&health.body).expect("liveness answers JSON");
    assert_eq!(health["ok"], Value::Bool(true));
    assert_eq!(health["degraded"], Value::Bool(true));
    assert_eq!(
        health["boundaries"]["object"],
        Value::Bool(false),
        "liveness publishes the verdict as a bare boolean: {health}"
    );
    assert!(
        !health.to_string().contains("last_error"),
        "no boundary reason on the unauthenticated liveness route: {health}"
    );

    // A storm inside the cooldown is answered from the recorded verdict: same
    // refusal, and not one additional vault sweep.
    for _ in 0..5 {
        let repeated = get(addr, &target, Some(&bearer));
        assert_eq!(repeated.status, 503, "body: {}", repeated.body);
        assert_eq!(repeated.body, OBJECT_UNAVAILABLE);
    }
    assert_eq!(
        vault.listings(),
        1,
        "the cooldown held: repeated requests revalidated nothing"
    );

    // Past the cooldown the next request revalidates inline. Nothing restarted
    // the unit, nothing reloaded the daemon, no operator was paged.
    std::thread::sleep(Duration::from_secs(COOLDOWN_SECONDS) + Duration::from_millis(400));
    let recovered = get(addr, &target, Some(&bearer));
    assert_eq!(
        recovered.status, 404,
        "the request passed the boundary and read the store: {}",
        recovered.body
    );
    assert_eq!(
        recovered.body,
        format!(r#"{{"state":"absent","uri":"{uri}"}}"#)
    );
    assert_eq!(
        vault.listings(),
        2,
        "recovery cost exactly one more vault sweep"
    );

    let open = object_boundary_state(addr);
    assert_eq!(open["ready"], Value::Bool(true));
    assert_eq!(open["last_error"], Value::Null);
    assert_ne!(
        open["checked_at"], closed["checked_at"],
        "the recovered verdict carries its own timestamp"
    );
}
