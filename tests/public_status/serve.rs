//! `stado public-status serve` is the status page: every request reads the
//! published document again.
//!
//! The built stado serves the page in the isolated deployment, on a port the
//! operating system handed out to a listener this test opened and closed:
//!
//! - before any check, `/` answers 503 with the store's reason (no status is
//!   published) and `/status.json` answers 503 with `{"error": …}` naming it;
//! - after `stado public-status check` against the real declared endpoints,
//!   `/` answers 200 naming every declared component, and `/status.json`
//!   answers exactly the document `show --json` reads;
//! - a path that is not a page answers 404, a POST answers 405;
//! - the page's own first line says where it listens.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::{Child, Stdio};

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
fn start(deployment: &mut Deployment) -> (Page, String) {
    let free = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, Default::default())))
        .expect("the operating system hands out a port");
    let bind = free
        .local_addr()
        .expect("the handed-out address")
        .to_string();
    drop(free);
    let mut child = deployment
        .command()
        .args(["public-status", "serve", "--bind", &bind])
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
    let (_page, origin) = start(&mut deployment);

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

    deployment.json(&["public-status", "check", "--json"]);
    let shown = deployment.json(&["public-status", "show", "--json"]);

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
