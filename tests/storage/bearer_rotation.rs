//! A rotated object bearer stops working on the next request, with no window.
//!
//! One isolated deployment: a real Skarbiec (`SKARBIEC` names the binary under
//! test, built with item revisions) serving a vault of its own, and the real
//! `stado serve --api` verifying one object namespace against that vault
//! through a consumer granted `read` of the namespace item's `token`. The
//! verifier holds the bearer with the version it read it under and asks the
//! vault only for the item's current version on each request
//! (`/v1/items/revision`). The journey: the first bearer is accepted (a GET of
//! an absent object answers Not Found, which only an authorized request
//! reaches); the item is given a new value with `skarbiec set`; at once the
//! old bearer is refused Unauthorized and the new one accepted. Before item
//! versions the old bearer kept working for up to 60 seconds. Every command,
//! request and the services' output stay beside report.json.
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

const NAMESPACE: &str = "rotation-probe";
const ITEM: &str = "rotation-probe-object-api";
const CONSUMER: &str = "rotation-probe-verifier";
const URI: &str = "stado://rotation-probe/probe/absent.txt";

/// Take every permission from group and others, the way the keyring and a
/// bearer file have to be held.
fn owner_only(path: &Path) {
    let status = Command::new("chmod")
        .arg("go-rwx")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "chmod go-rwx {}", path.display());
}

struct Deployment {
    root: PathBuf,
    keyring: PathBuf,
    report: Value,
    children: Vec<Child>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/bearer-rotation")
            .join(uuid::Uuid::new_v4().to_string());
        // gpg-agent's socket lives in the keyring and a Unix socket path is
        // short on macOS, so the keyring sits at a short path of its own.
        let keyring = repository
            .join(".build/g")
            .join(std::process::id().to_string());
        for directory in [
            root.join("home"),
            root.join("tmp"),
            root.join("store"),
            keyring.clone(),
        ] {
            fs::create_dir_all(&directory).unwrap();
        }
        owner_only(&keyring);
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        Self {
            root,
            keyring,
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "commands": [],
                "outcome": "failed",
            }),
            children: Vec::new(),
        }
    }

    fn skarbiec(&self) -> Command {
        let binary = std::env::var("SKARBIEC")
            .expect("SKARBIEC must name the skarbiec binary under test (with item revisions)");
        let mut command = Command::new(binary);
        command
            .env("GNUPGHOME", &self.keyring)
            .env("SKARBIEC_VAULT_FILE", self.root.join("vault.json"))
            .env("SKARBIEC_AUDIT_FILE", self.root.join("audit.jsonl"))
            .stdin(Stdio::null());
        command
    }

    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    /// Run one command that must succeed and answer its stdout. Only the
    /// label, the exit status and stderr reach the report, never a value.
    fn must(&mut self, label: &str, mut command: Command) -> String {
        let output = command.output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "command": label,
            "exit_status": output.status.code(),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        assert!(
            output.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Start a service and answer the address it printed after `marker`.
    fn spawn(&mut self, label: &str, mut command: Command, marker: &'static str) -> String {
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let log = self.root.join(format!("{label}.stderr"));
        let (sender, announced) = mpsc::channel();
        std::thread::spawn(move || {
            let mut file = File::create(log).unwrap();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                if let Some((_, address)) = line.split_once(marker) {
                    let _ = sender.send(address.split_whitespace().next().unwrap().to_string());
                }
            }
        });
        self.children.push(child);
        match announced.recv() {
            Ok(address) => address,
            Err(_) => panic!("{label} ended before announcing its address; see {label}.stderr"),
        }
    }

    fn set_token(&mut self, value: &str) {
        let mut command = self.skarbiec();
        command.args(["set", ITEM, "--type", "token", &format!("token={value}")]);
        self.must("skarbiec set (token)", command);
    }

    async fn get(&mut self, origin: &str, bearer: &str) -> reqwest::StatusCode {
        let status = reqwest::Client::new()
            .get(format!("{origin}/api/object"))
            .query(&[("uri", URI)])
            .bearer_auth(bearer)
            .send()
            .await
            .unwrap()
            .status();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "request": format!("GET /api/object?uri={URI}"),
            "status": status.as_u16(),
        }));
        self.save();
        status
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = Command::new("gpgconf")
            .arg("--homedir")
            .arg(&self.keyring)
            .args(["--kill", "all"])
            .status();
        let _ = fs::remove_dir_all(&self.keyring);
        self.save();
        eprintln!("bearer rotation evidence: {}", self.root.display());
    }
}

#[tokio::test]
async fn a_rotated_object_bearer_is_refused_on_the_next_request() {
    let mut deployment = Deployment::start();
    let first = format!("first-{}", uuid::Uuid::new_v4().simple());
    let second = format!("second-{}", uuid::Uuid::new_v4().simple());

    let mut init = deployment.skarbiec();
    init.args(["init", "rotation-probe-owner"]);
    deployment.must("skarbiec init", init);
    deployment.set_token(&first);
    let mut grant = deployment.skarbiec();
    grant.args([
        "grant",
        "issue",
        CONSUMER,
        "--capabilities",
        &format!("read:{ITEM}#token"),
    ]);
    let issued: Value =
        serde_json::from_str(&deployment.must("skarbiec grant issue", grant)).unwrap();
    let token_file = deployment.root.join("verifier.token");
    fs::write(
        &token_file,
        issued["token"]
            .as_str()
            .expect("grant issue answers the bearer"),
    )
    .unwrap();
    owner_only(&token_file);

    let mut vault = deployment.skarbiec();
    vault.args(["serve", "--port", "0"]);
    let vault_address = deployment.spawn("skarbiec", vault, "listening on http://");

    let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
        .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
    let mut init = deployment.stado();
    init.args(["config", "init"]);
    deployment.must("stado config init", init);
    let mut set = deployment.stado();
    set.args(["config", "set", "dashboard.request_limits", &limits]);
    deployment.must("stado config set dashboard.request_limits", set);

    let namespaces = json!({
        NAMESPACE: {"item": ITEM, "prefix_policies": [{"prefix": "probe/", "actions": ["get"]}]}
    });
    let loopback = std::net::Ipv4Addr::LOCALHOST.to_string();
    let mut api = deployment.stado();
    api.env("WC_SKARBIEC_URL", format!("http://{vault_address}"))
        .env("WC_SKARBIEC_CONSUMER", CONSUMER)
        .env("WC_SKARBIEC_TOKEN_FILE", &token_file)
        .env("WC_OBJECT_API_NAMESPACES", namespaces.to_string())
        .args([
            "serve",
            "--api",
            "--bind",
            &loopback,
            "--port",
            "0",
            "--api-local-store",
        ])
        .arg(deployment.root.join("store"));
    let api_address = deployment.spawn("stado", api, "[dashboard] listening on http://");
    let origin = format!("http://{api_address}");

    assert_eq!(
        deployment.get(&origin, &first).await,
        reqwest::StatusCode::NOT_FOUND,
        "the first bearer must be accepted (an absent object answers Not Found only to an authorized request)"
    );
    deployment.set_token(&second);
    assert_eq!(
        deployment.get(&origin, &first).await,
        reqwest::StatusCode::UNAUTHORIZED,
        "the rotated bearer must be refused on the very next request"
    );
    assert_eq!(
        deployment.get(&origin, &second).await,
        reqwest::StatusCode::NOT_FOUND,
        "the new bearer must be accepted at once"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
