//! One isolated deployment: a real Skarbiec (`SKARBIEC` names the binary
//! under test) serving a vault of its own on a keyring of its own, and the
//! real `stado serve --api` verifying one object namespace against that
//! vault as consumer `stado`, granted `read` of the namespace item's
//! `token`. Every command, request and the services' output stay beside
//! report.json under the checkout's `.build`.
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

pub const NAMESPACE: &str = "consultation-probe";
pub const ITEM: &str = "consultation-probe-object-api";
/// Stado's one vault identity; `Client::stado()` refuses every other name.
pub const CONSUMER: &str = "stado";
pub const URI: &str = "stado://consultation-probe/probe/absent.txt";

/// Take every permission from group and others, the way the keyring and a
/// bearer file have to be held.
pub fn owner_only(path: &Path) {
    let status = Command::new("chmod")
        .arg("go-rwx")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success(), "chmod go-rwx {}", path.display());
}

pub struct Deployment {
    pub root: PathBuf,
    pub keyring: PathBuf,
    pub report: Value,
    children: Vec<Child>,
}

impl Deployment {
    pub fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/vault-consultation")
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

    pub fn skarbiec(&self) -> Command {
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

    pub fn stado(&self) -> Command {
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
    pub fn must(&mut self, label: &str, mut command: Command) -> String {
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
    pub fn spawn(&mut self, label: &str, mut command: Command, marker: &'static str) -> String {
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

    pub fn set_token(&mut self, value: &str) {
        let mut command = self.skarbiec();
        command.args(["set", ITEM, "--type", "token", &format!("token={value}")]);
        self.must("skarbiec set (token)", command);
    }

    /// Issue the verifier's grant and write its bearer to `token_file`.
    pub fn issue_grant(&mut self, token_file: &Path) {
        let mut grant = self.skarbiec();
        grant.args([
            "grant",
            "issue",
            CONSUMER,
            "--capabilities",
            &format!("read:{ITEM}#token"),
            "--replace-capabilities",
        ]);
        let issued: Value =
            serde_json::from_str(&self.must("skarbiec grant issue", grant)).unwrap();
        fs::write(
            token_file,
            issued["token"]
                .as_str()
                .expect("grant issue answers the bearer"),
        )
        .unwrap();
        owner_only(token_file);
    }

    pub fn record(&mut self, request: &str, status: reqwest::StatusCode, body: &Value) {
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "request": request,
            "status": status.as_u16(),
            "body": body,
        }));
        self.save();
    }

    pub async fn get(&mut self, origin: &str, bearer: &str) -> (reqwest::StatusCode, Value) {
        let (status, body) = get_object(origin.to_string(), bearer.to_string()).await;
        self.record(&format!("GET /api/object?uri={URI}"), status, &body);
        (status, body)
    }

    pub async fn state(&mut self, origin: &str) -> Value {
        let response = reqwest::Client::new()
            .get(format!("{origin}/api/state.json"))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        self.record("GET /api/state.json", status, &body);
        assert_eq!(status, reqwest::StatusCode::OK);
        body
    }

    pub fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

/// One object read with `bearer`: the status and the body as JSON (a body
/// that is not JSON reads as null).
pub async fn get_object(origin: String, bearer: String) -> (reqwest::StatusCode, Value) {
    let response = reqwest::Client::new()
        .get(format!("{origin}/api/object"))
        .query(&[("uri", URI)])
        .bearer_auth(bearer)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    let body = match serde_json::from_str(&text) {
        Ok(body) => body,
        Err(_) => Value::Null,
    };
    (status, body)
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
        eprintln!("vault consultation evidence: {}", self.root.display());
    }
}

/// The keyboxd database's lock file in `keyring`, as GnuPG's dotlock names
/// it. Its first line is the holder's pid, right-aligned in a field of ten,
/// its second the host name the lock was taken on; a holder that is alive on
/// this host keeps every gpg of the keyring waiting, a dead one is removed by
/// the next gpg that finds it.
pub fn keyboxd_lock(keyring: &Path) -> PathBuf {
    keyring.join("public-keys.d/pubring.db.lock")
}

pub fn hold_keyring_lock(keyring: &Path, holder: &Child) {
    let node = Command::new("uname").arg("-n").output().unwrap();
    let node = String::from_utf8(node.stdout).unwrap();
    let lock = keyboxd_lock(keyring);
    fs::create_dir_all(lock.parent().unwrap()).unwrap();
    fs::write(&lock, format!("{:>10}\n{}\n", holder.id(), node.trim())).unwrap();
}
