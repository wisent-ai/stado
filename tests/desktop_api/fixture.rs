use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

pub struct Service {
    pub root: PathBuf,
    pub config: PathBuf,
    pub origin: String,
    report: Value,
    child: Option<Child>,
    log: Option<JoinHandle<()>>,
    client: reqwest::Client,
}

impl Service {
    pub fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/desktop-api")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        let config = root.join("config.json");
        let mut service = Self {
            root,
            config,
            origin: String::new(),
            report: json!({"commands": [], "outcome": "failed"}),
            child: None,
            log: None,
            client: reqwest::Client::new(),
        };
        let revision = Command::new("git")
            .current_dir(&repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(revision.status.success());
        let patch = Command::new("git")
            .current_dir(&repository)
            .args(["diff", "--binary", "HEAD"])
            .output()
            .unwrap();
        assert!(patch.status.success());
        fs::write(service.root.join("source.patch"), &patch.stdout).unwrap();
        service.report["source_revision"] =
            json!(String::from_utf8(revision.stdout).unwrap().trim());
        service.report["source_patch_sha256"] =
            json!(format!("{:x}", Sha256::digest(&patch.stdout)));
        let mut binary = File::open(env!("CARGO_BIN_EXE_stado")).unwrap();
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = binary.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        service.report["binary_sha256"] = json!(format!("{:x}", digest.finalize()));
        let version = service.cli(&["--version"]);
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .any(|part| Some(part) == service.report["source_revision"].as_str()),
            "the tested executable must identify the exact source revision"
        );
        service.report["binary_version"] = json!(version);
        service.cli(&["config", "init"]);
        let mut child = service
            .command()
            .args(["dashboard", "--bind", "127.0.0.1", "--port", "0"])
            .stdout(Stdio::from(
                File::create(service.root.join("service.stdout")).unwrap(),
            ))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        service.report["service"] = json!({"arguments": ["dashboard", "--bind", "127.0.0.1", "--port", "0"], "pid": child.id()});
        service.child = Some(child);
        service.save();
        let (sender, ready) = mpsc::channel();
        let path = service.root.join("service.stderr");
        service.log = Some(std::thread::spawn(move || {
            let mut file = File::create(path).unwrap();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                file.flush().unwrap();
                if let Some(origin) = line.strip_prefix("[dashboard] listening on ") {
                    let _ = sender.send(origin.to_owned());
                }
            }
        }));
        service.origin = ready
            .recv()
            .expect("the real service must announce a bound listener; inspect service.stderr");
        service.report["origin"] = json!(service.origin);
        service.save();
        service
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("STADO_CONFIG", &self.config)
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    pub fn cli(&mut self, args: &[&str]) -> String {
        let output = self.command().args(args).output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "interface": "cli", "args": args, "exit_status": output.status.code(), "stdout": stdout, "stderr": stderr
        }));
        self.save();
        assert!(output.status.success(), "stado {args:?}: {stderr}");
        stdout
    }

    pub async fn call(&mut self, body: Value, expected_status: u16) -> Value {
        let endpoint = format!("{}/api/operator/run", self.origin);
        let response = self
            .client
            .post(&endpoint)
            .header("X-Stado-Action", "operator-command")
            .json(&body)
            .send()
            .await;
        if let Err(error) = &response {
            self.report["commands"].as_array_mut().unwrap().push(json!({"endpoint": endpoint, "request": body, "transport_error": error.to_string()}));
            self.save();
        }
        let response = response.expect("real API request");
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        self.report["commands"].as_array_mut().unwrap().push(
            json!({"endpoint": endpoint, "request": body, "http_status": status, "response": text}),
        );
        self.save();
        assert_eq!(status, expected_status, "{text}");
        serde_json::from_str(&text).unwrap()
    }

    pub fn persisted(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.config).unwrap()).unwrap()
    }

    pub fn pass(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            match child.wait() {
                Ok(status) => self.report["service"]["exit_status"] = json!(status.to_string()),
                Err(error) => self.report["service"]["wait_error"] = json!(error.to_string()),
            }
        }
        if let Some(log) = self.log.take() {
            let _ = log.join();
        }
        self.save();
        eprintln!("native API evidence: {}", self.root.display());
    }
}
