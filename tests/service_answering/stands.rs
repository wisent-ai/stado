//! A declared service that accepts connections and answers none is read as
//! standing, and one that answers is read as answered, through the host's
//! own probe against real listeners on this machine: one that accepts the
//! connection and never writes a byte, one that answers every request. The
//! bound is the one the fixture registry declares for its resolver, the same
//! measure the resolver applies to a held adapter, and the listeners bind
//! where that registry's resolver binds. The recovery register is written
//! and read back under an isolated home. The recovery itself — the declared
//! repair and `stado service restart` of a real unit — acts on the machine's
//! init system and is not run here; what this proves is the judgement that
//! triggers it and the record it leaves.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;

use serde_json::{json, Value};
use stado::deploy::service_answering::register::{self, Recovery};
use stado::deploy::service_answering::{ask_and_read, forget, read, Finished, Reading};
use stado::deploy::service_catalog::CatalogService;

fn fixture_registry() -> Value {
    serde_json::from_str(include_str!("../service_directory/registry.json"))
        .expect("the fixture registry is JSON")
}

/// The fixture resolver's declaration: where it binds, and the refresh
/// interval every held wait is judged by.
fn declared_resolver() -> Value {
    fixture_registry()["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|target| target.get("service_resolver").cloned())
        .expect("the fixture registry declares a resolver")
}

/// A catalog entry declaring the question `POST /ask`.
fn entry(name: &str) -> CatalogService {
    serde_json::from_value(json!({
        "name": name,
        "summary": "a service of this test",
        "program": "/usr/bin/true",
        "answering": { "method": "POST", "path": "/ask", "body": {} }
    }))
    .expect("a catalog service with an answering declaration")
}

/// A listener where the fixture's resolver binds: its declared address, a
/// port the host hands out.
fn listener(resolver: &Value) -> (TcpListener, u16) {
    let bind = resolver["api_bind"]
        .as_str()
        .expect("the resolver declares api_bind");
    let listener = TcpListener::bind(bind).expect("the declared bind is handed out");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();
    (listener, port)
}

/// Accept every connection and never answer: the stand-in for a service
/// that holds requests. Connections are kept so the client's request is
/// never refused, only unanswered.
fn stands_forever(listener: TcpListener) {
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for connection in listener.incoming().flatten() {
            held.push(connection);
        }
    });
}

/// Answer every request with an empty body once its request line is read.
fn answers(listener: TcpListener) {
    std::thread::spawn(move || {
        for connection in listener.incoming().flatten() {
            let mut reader = BufReader::new(connection);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut connection = reader.into_inner();
            let status = reqwest::StatusCode::NO_CONTENT;
            let response = format!(
                "HTTP/1.1 {} {}\r\n\r\n",
                status.as_u16(),
                status
                    .canonical_reason()
                    .expect("standard HTTP status has a reason")
            );
            let _ = connection.write_all(response.as_bytes());
        }
    });
}

/// An actual HTTP failure carries the service's diagnostic body, not a
/// healthy answer merely because it completed instead of standing.
fn answers_unready(listener: TcpListener) {
    std::thread::spawn(move || {
        for connection in listener.incoming().flatten() {
            let mut reader = BufReader::new(connection);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut connection = reader.into_inner();
            let status = reqwest::StatusCode::SERVICE_UNAVAILABLE;
            let body = r#"{"detail":"canary decryption refused"}"#;
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                status.as_u16(),
                status.canonical_reason().expect("standard HTTP status has a reason"),
                body.len(),
            );
            let _ = connection.write_all(response.as_bytes());
        }
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_that_answers_nothing_stands_once_the_bound_has_passed_and_an_answering_one_is_answered(
) {
    let resolver = declared_resolver();
    let bound = resolver["refresh_seconds"]
        .as_u64()
        .expect("the resolver declares refresh_seconds");

    let (silent, silent_port) = listener(&resolver);
    stands_forever(silent);
    let standing = entry("standing-service");
    let first = ask_and_read(&standing, &standing.name, silent_port, bound)
        .expect("a declared question is asked");
    assert!(matches!(first, Reading::Asked { .. }), "{first:?}");
    // The pass after the bound: the question is still in flight, so the
    // service stands. The wait is the declared bound itself, measured, not
    // a guess of how long a service takes.
    let waited = std::time::Instant::now();
    while waited.elapsed().as_secs() <= bound {
        std::thread::yield_now();
    }
    let later = read("standing-service", bound);
    let Reading::Stood {
        waited_seconds,
        bound_seconds,
        ..
    } = &later
    else {
        panic!("a question unanswered past the bound reads as standing: {later:?}");
    };
    assert_eq!(*bound_seconds, bound);
    assert!(*waited_seconds > bound, "{later:?}");
    forget("standing-service");
    assert!(
        matches!(read("standing-service", bound), Reading::Asked { .. }),
        "after the restart the next pass asks afresh"
    );

    let (live, live_port) = listener(&resolver);
    answers(live);
    let answering = entry("answering-service");
    let asked = ask_and_read(&answering, &answering.name, live_port, bound)
        .expect("a declared question is asked");
    assert!(matches!(asked, Reading::Asked { .. }), "{asked:?}");
    let answered = loop {
        match read("answering-service", bound) {
            Reading::Finished { result: found } => break found,
            Reading::Stood { .. } => panic!("the answering service was read as standing"),
            _ => std::thread::yield_now(),
        }
    };
    let Finished::Answered {
        status,
        successful,
        detail,
        ..
    } = answered
    else {
        panic!("the answering service's answer was refused: {answered:?}");
    };
    assert_eq!(status, reqwest::StatusCode::NO_CONTENT.as_u16());
    assert!(
        successful,
        "a successful HTTP answer was classified as failed"
    );
    assert!(
        detail.is_none(),
        "a successful HTTP answer retained an error body"
    );

    let (unready, unready_port) = listener(&resolver);
    answers_unready(unready);
    let unavailable = entry("unready-service");
    ask_and_read(&unavailable, &unavailable.name, unready_port, bound)
        .expect("the readiness question is asked");
    let failed = loop {
        match read("unready-service", bound) {
            Reading::Finished { result: found } => break found,
            Reading::Stood { .. } => {
                panic!("a finished HTTP refusal was read as a standing request")
            }
            _ => std::thread::yield_now(),
        }
    };
    let Finished::Answered {
        status,
        successful,
        detail,
        ..
    } = failed
    else {
        panic!("the readiness HTTP failure was lost: {failed:?}");
    };
    assert_eq!(status, reqwest::StatusCode::SERVICE_UNAVAILABLE.as_u16());
    assert!(
        !successful,
        "a failed readiness HTTP response was classified as healthy"
    );
    assert_eq!(
        detail.as_deref(),
        Some(r#"{"detail":"canary decryption refused"}"#)
    );

    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("service-answering")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&home).expect("the isolated home is made");
    assert!(register::read(&home)
        .expect("an absent register reads as no recovery")
        .is_empty());
    let recovery = Recovery {
        at: "recorded".into(),
        host: "this-host".into(),
        service: "standing-service".into(),
        unit: "com.wisent.standing-service".into(),
        question: "POST /ask".into(),
        answering: None,
        stood_since: "earlier".into(),
        stood_seconds: *waited_seconds,
        bound_seconds: bound,
        held_requests: Vec::new(),
        repair: Value::Null,
        restart: json!({ "refusal": "no unit of that name on this machine" }),
        restarted: false,
        alert: json!([]),
        observed_by: None,
    };
    register::append(&home, &recovery).expect("the recovery is recorded");
    let listed = register::read(&home).expect("the register reads back");
    assert_eq!(listed, vec![recovery]);
    std::fs::write(register::path(&home), "not a recovery\n").expect("the register is overwritten");
    let refused =
        register::read(&home).expect_err("a register line that is not a recovery is refused");
    assert!(
        refused.to_string().contains("is not a recovery"),
        "{refused}"
    );
    std::fs::remove_dir_all(&home).expect("the isolated home is removed");
}

/// The real `stado product catalog` accepts the shipped catalog and refuses
/// the same structured catalog when one service's repair steps become a
/// scalar. The refusal names the field rather than silently running one step.
#[test]
fn the_catalog_refuses_scalar_repair_steps() {
    let shipped = include_str!("../../catalog/products.yml");
    let mut invalid = stado_product::catalog::embedded().expect("the shipped catalog is valid");
    let products = invalid["products"]
        .as_array_mut()
        .expect("the catalog holds products");
    let vault = products
        .iter_mut()
        .find(|product| product["id"] == "skarbiec")
        .expect("the vault is catalogued");
    let repair = &mut vault["service"]["answering"]["repair"];
    assert!(repair.is_array(), "the vault declares repair steps");
    *repair = json!("audit-lock");
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("service-answering-catalog")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&directory).expect("the catalog directory is made");
    let read = |name: &str, body: &str| {
        let path = directory.join(name);
        std::fs::write(&path, body).expect("the catalog copy is written");
        std::process::Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["product", "catalog", "--catalog"])
            .arg(&path)
            .output()
            .expect("stado runs")
    };
    let listed = read("listed.yml", shipped);
    assert!(
        listed.status.success(),
        "the shipped catalog is read: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let malformed = serde_json::to_string(&invalid)
        .expect("the altered catalog is JSON, also accepted as YAML");
    let single = read("single.yml", &malformed);
    let stderr = String::from_utf8_lossy(&single.stderr);
    assert!(
        !single.status.success()
            && stderr.contains("skarbiec: service.answering.repair must be a list of repair steps"),
        "one string where the steps are a list is refused naming the field: {stderr}"
    );
    std::fs::remove_dir_all(&directory).expect("the catalog directory is removed");
}
