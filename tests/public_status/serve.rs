//! `stado availability serve` is the status page: every request reads the
//! published document again.
//!
//! The built stado serves the page in the isolated deployment, on a port the
//! operating system handed out to a listener this test opened and closed:
//!
//! - before any check, `/` answers 503 with the store's reason (no status is
//!   published) and `/status.json` answers 503 with `{"error": …}` naming it;
//! - after `stado availability check` against the real declared endpoints,
//!   `/` answers 200 naming every declared component, and `/status.json`
//!   answers exactly the document `show --json` reads;
//! - a path that is not a page answers 404, a POST answers 405;
//! - a head past the declared bound answers 431 naming the bound, and a head
//!   that is not HTTP/1.1 answers 400 (or 501 for a transfer coding) quoting
//!   what was wrong;
//! - the page's own first line says where it listens.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::{Child, Stdio};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::deployment::{declared, Deployment};

/// The page process, ended when the journey ends.
struct Page(Child);

impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Start the page and wait for its own line saying where it listens.
fn start(deployment: &mut Deployment, head_limit: Option<usize>) -> (Page, String) {
    let free = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, Default::default())))
        .expect("the operating system hands out a port");
    let bind = free
        .local_addr()
        .expect("the handed-out address")
        .to_string();
    drop(free);
    let mut command = deployment.command();
    if let Some(limit) = head_limit {
        command.env(
            "WC_DASHBOARD_REQUEST_LIMITS",
            json!({
                "head_bytes": limit,
                "body_bytes": limit,
                "registry_import_bytes": limit,
                "operator_console": {
                    "argument_count": limit,
                    "argument_bytes": limit,
                    "input_bytes": limit,
                    "request_bytes": limit
                },
                "fleet_join": { "request_bytes": limit, "field_bytes": limit }
            })
            .to_string(),
        );
    }
    let mut child = command
        .args(["availability", "serve", "--bind", &bind])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the page starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("the page's stdout"))
        .read_line(&mut line)
        .expect("the page says where it listens");
    deployment.report["page"] = json!({ "bind": bind, "said": line });
    deployment.save();
    assert!(line.contains(&format!("listens on {bind}")), "{line}");
    (Page(child), format!("http://{bind}"))
}

async fn get(origin: &str, path: &str) -> (StatusCode, String) {
    let response = reqwest::get(format!("{origin}{path}"))
        .await
        .expect("the page answers");
    let status = response.status();
    (status, response.text().await.expect("the page's body"))
}

#[tokio::test]
async fn the_page_reads_the_published_status_on_every_request() {
    let mut deployment = Deployment::start("serve");
    let (_page, origin) = start(&mut deployment, None);

    let (status, body) = get(&origin, "/").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body.contains("no status is published"), "{body}");
    let (status, body) = get(&origin, "/status.json").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let refusal: Value = serde_json::from_str(&body).expect("a JSON refusal");
    assert!(
        refusal["error"]
            .as_str()
            .is_some_and(|error| error.contains("no status is published")),
        "{body}"
    );

    deployment.json(&["availability", "check", "--json"]);
    let shown = deployment.json(&["availability", "show", "--json"]);

    let (status, body) = get(&origin, "/").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (_, _, title) in declared() {
        assert!(body.contains(&title), "{title} is on the page: {body}");
    }
    let (status, body) = get(&origin, "/status.json").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let served: Value = serde_json::from_str(&body).expect("the published document");
    assert_eq!(served, shown);

    let (status, _) = get(&origin, "/elsewhere").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let posted = reqwest::Client::new()
        .post(format!("{origin}/"))
        .send()
        .await
        .expect("the page answers a POST");
    assert_eq!(posted.status(), StatusCode::METHOD_NOT_ALLOWED);
    deployment.passed();
}

/// A real socket request ending exactly at the declared bound is accepted;
/// a larger head is refused before the store is read.
#[tokio::test]
async fn the_status_page_bounds_request_heads() {
    let mut deployment = Deployment::start("bounded-head");
    let valid = b"GET /status.json HTTP/1.1\r\n\r\n";
    let (_page, origin) = start(&mut deployment, Some(valid.len()));
    let address = origin.strip_prefix("http://").expect("the page origin");

    let mut accepted = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    accepted.write_all(valid).await.expect("send valid head");
    let mut accepted_reply = String::new();
    accepted
        .read_to_string(&mut accepted_reply)
        .await
        .expect("read status");
    assert!(
        accepted_reply
            .lines()
            .next()
            .expect("status line")
            .contains(&StatusCode::SERVICE_UNAVAILABLE.to_string()),
        "{accepted_reply}"
    );
    assert!(
        accepted_reply.contains("no status is published"),
        "{accepted_reply}"
    );
    let mut with_body = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    with_body
        .write_all(&[valid.as_slice(), &[u8::MAX]].concat())
        .await
        .expect("send head and body");
    let mut body_reply = String::new();
    with_body
        .read_to_string(&mut body_reply)
        .await
        .expect("read status");
    assert!(
        body_reply
            .lines()
            .next()
            .expect("status line")
            .contains(&StatusCode::SERVICE_UNAVAILABLE.to_string()),
        "{body_reply}"
    );

    let oversized = format!(
        "GET /status.json HTTP/1.1\r\nX-Fill: {}\r\n\r\n",
        "x".repeat(valid.len())
    );
    let refused_reply = exchange(address, oversized.as_bytes()).await;
    assert!(
        refused_reply
            .lines()
            .next()
            .expect("status line")
            .contains(&StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE.to_string()),
        "{refused_reply}"
    );
    // How many bytes the page had read when it stopped depends on how the
    // socket delivered them; the bound it names does not.
    assert!(
        refused_reply.contains("HTTP request head too large"),
        "{refused_reply}"
    );
    assert!(
        refused_reply.contains(&format!("the {}-byte head bound", valid.len())),
        "{refused_reply}"
    );
    deployment.report["bounded_head"] = json!({
        "limit": valid.len(),
        "accepted": accepted_reply,
        "accepted_with_binary_body": body_reply,
        "refused": refused_reply
    });
    deployment.passed();
}

/// Send `bytes` on a fresh connection and read the whole answer.
async fn exchange(address: &str, bytes: &[u8]) -> String {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("connect");
    stream.write_all(bytes).await.expect("send the request");
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .await
        .expect("read the answer");
    reply
}

/// A head that is not HTTP/1.1 is refused with the status that names the
/// fault and the sentence that quotes it, before the store is read: a short
/// request line, a header line without a colon and a head that is not UTF-8
/// are Bad Request, a transfer coding the page does not frame is Not
/// Implemented.
#[tokio::test]
async fn the_status_page_refuses_malformed_heads_by_name() {
    let mut deployment = Deployment::start("malformed-head");
    let (_page, origin) = start(&mut deployment, None);
    let address = origin.strip_prefix("http://").expect("the page origin");
    let not_utf8 = [b"GET /".as_slice(), &[u8::MAX], b" HTTP/1.1\r\n\r\n"].concat();
    let cases: Vec<(&str, &[u8], StatusCode, &str)> = vec![
        (
            "short request line",
            b"GET /status.json\r\n\r\n",
            StatusCode::BAD_REQUEST,
            "is not <method> <target> HTTP/<version>",
        ),
        (
            "header without a colon",
            b"GET /status.json HTTP/1.1\r\nno-colon-here\r\n\r\n",
            StatusCode::BAD_REQUEST,
            "HTTP header line \"no-colon-here\" has no colon",
        ),
        (
            "head not UTF-8",
            &not_utf8,
            StatusCode::BAD_REQUEST,
            "HTTP request head is not UTF-8",
        ),
        (
            "transfer coding",
            b"GET /status.json HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
            StatusCode::NOT_IMPLEMENTED,
            "Transfer-Encoding is unsupported",
        ),
    ];
    for (case, bytes, status, sentence) in cases {
        let reply = exchange(address, bytes).await;
        let status_line = reply.lines().next().expect("status line");
        assert!(status_line.contains(&status.to_string()), "{case}: {reply}");
        assert!(reply.contains(sentence), "{case}: {reply}");
        deployment.report["refusals"][case] = json!(reply);
    }
    deployment.passed();
}
