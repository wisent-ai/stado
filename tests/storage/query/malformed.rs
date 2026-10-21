//! A query field the object API cannot decode is refused by name, never read
//! as other text.
//!
//! The real `stado serve --api` serves a local store of its own; the public
//! release route reads its query before any trust or vault check, so each
//! request below reaches the query reader:
//!
//! - `uri=%ZZ` answers 400 naming the field and the escape that is not two
//!   hexadecimal digits;
//! - `uri=%E` answers 400 naming the escape that is cut short;
//! - `uri=%FF` answers 400 naming bytes that are not UTF-8 once decoded;
//! - a well-formed, percent-encoded release coordinate is decoded and answered
//!   404 with the decoded `uri`, the object being absent.
//!
//! Every request, status and body stays in report.json beside the service's
//! log under the checkout's `.build/storage-query-malformed`.
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RELEASE: &str = "stado://releases/query-probe/0.0.1/darwin-arm64/release.json";

/// Port zero asks the operating system for a port no one holds (ip(7),
/// <https://man7.org/linux/man-pages/man7/ip.7.html>); the service says which.
const ANY_PORT: &str = "0";

struct Deployment {
    root: PathBuf,
    report: Value,
    service: Option<Child>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crate sits in the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/storage-query-malformed")
            .join(uuid::Uuid::new_v4().to_string());
        for directory in [root.join("home"), root.join("tmp"), root.join("store")] {
            fs::create_dir_all(&directory).expect("create the deployment directory");
        }
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the source revision");
        Self {
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "requests": {},
                "outcome": "failed",
            }),
            root,
            service: None,
        }
    }

    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    /// Start the object API on a port the operating system hands out and
    /// answer the address it says it listens on.
    fn serve(&mut self) -> String {
        let init = self
            .stado()
            .args(["config", "init"])
            .output()
            .expect("stado config init runs");
        assert!(
            init.status.success(),
            "stado config init: {}",
            String::from_utf8_lossy(&init.stderr)
        );
        let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
        let mut child = self
            .stado()
            .args([
                "serve",
                "--api",
                "--bind",
                &loopback,
                "--port",
                ANY_PORT,
                "--api-local-store",
            ])
            .arg(self.root.join("store"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("stado serve --api starts");
        let stderr = child.stderr.take().expect("the service's stderr");
        let log = self.root.join("stado.stderr");
        let (sender, announced) = mpsc::channel();
        std::thread::spawn(move || {
            let mut file = File::create(log).expect("create the service log");
            for line in BufReader::new(stderr).lines() {
                let line = line.expect("a service log line");
                writeln!(file, "{line}").expect("write the service log");
                if let Some((_, address)) = line.split_once("[dashboard] listening on http://") {
                    let address = address
                        .split_whitespace()
                        .next()
                        .expect("the address")
                        .to_string();
                    if let Err(error) = sender.send(address) {
                        panic!("the journey stopped reading the service's address: {error}");
                    }
                }
            }
        });
        self.service = Some(child);
        match announced.recv() {
            Ok(address) => address,
            Err(error) => panic!("stado serve --api ended before saying where it listens ({error}); see stado.stderr"),
        }
    }

    /// Send one GET for `target` on a fresh connection; answer its status
    /// line and JSON body, both kept in the report.
    async fn get(&mut self, address: &str, target: &str) -> (String, Value) {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("connect to the object API");
        let head = format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(head.as_bytes())
            .await
            .expect("send the request");
        let mut reply = String::new();
        stream
            .read_to_string(&mut reply)
            .await
            .expect("read the answer");
        let (head, body) = reply
            .split_once("\r\n\r\n")
            .expect("an HTTP answer has a head and a body");
        let status = head.lines().next().expect("a status line").to_string();
        let body: Value = serde_json::from_str(body).expect("the object API answers JSON");
        self.report["requests"][target] = json!({ "status": status, "body": body });
        self.save();
        (status, body)
    }

    fn save(&self) {
        let text = serde_json::to_string_pretty(&self.report).expect("the report is JSON");
        fs::write(self.root.join("report.json"), text).expect("write report.json");
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(mut child) = self.service.take() {
            if let Err(error) = child.kill() {
                eprintln!("stopping stado serve --api: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("waiting for stado serve --api: {error}");
            }
        }
    }
}

#[tokio::test]
async fn a_query_field_that_cannot_be_decoded_is_refused_by_name() {
    let mut deployment = Deployment::start();
    let address = deployment.serve();
    let bad_request = reqwest::StatusCode::BAD_REQUEST.to_string();
    let cases = [
        ("uri=%ZZ", "is not 2 hexadecimal digits"),
        ("uri=%E", "is not followed by 2 hexadecimal digits"),
        ("uri=%FF", "its decoded bytes are not UTF-8"),
    ];
    for (field, reason) in cases {
        let (status, body) = deployment
            .get(&address, &format!("/api/release/object?{field}"))
            .await;
        assert!(status.contains(&bad_request), "{field}: {status} {body}");
        let error = body["error"].as_str().expect("a refusal carries its error");
        assert!(
            error.contains(&format!("the query field {field:?} cannot be read")),
            "{field}: {error}"
        );
        assert!(error.contains(reason), "{field}: {error}");
    }
    let encoded: String = url::form_urlencoded::byte_serialize(RELEASE.as_bytes()).collect();
    let (status, body) = deployment
        .get(&address, &format!("/api/release/object?uri={encoded}"))
        .await;
    assert!(
        status.contains(&reqwest::StatusCode::NOT_FOUND.to_string()),
        "{status} {body}"
    );
    assert_eq!(
        body["uri"],
        json!(RELEASE),
        "the decoded coordinate is the one looked up: {body}"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
