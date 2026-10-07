use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

pub struct Service {
    pub root: PathBuf,
    pub config: PathBuf,
    pub home: PathBuf,
    temporary: PathBuf,
    pub origin: String,
    report: Value,
    child: Option<Child>,
    log: Option<JoinHandle<()>>,
    client: reqwest::Client,
}

impl Service {
    /// The installed-host journey's constructor; the cancellation journey
    /// shares this fixture and starts with a configuration instead.
    #[allow(dead_code)]
    pub fn start() -> Self {
        Self::start_with_input(None)
    }

    /// The cancellation journey's constructor; see [`Service::start`].
    #[allow(dead_code)]
    pub fn start_with_configuration(variable: &str) -> Self {
        Self::start_with_input(Some(variable))
    }

    fn start_with_input(configuration: Option<&str>) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/desktop-api")
            .join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        let temporary = root.join("tmp");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&temporary).unwrap();
        let config = home.join(".stado/config.json");
        let mut service = Self {
            root,
            config,
            home,
            temporary,
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
        service.report["binary_sha256"] =
            json!(binary_digest(Path::new(env!("CARGO_BIN_EXE_stado"))));
        let version = service.cli(&["--version"]);
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .any(|part| Some(part) == service.report["source_revision"].as_str()),
            "the tested executable must identify the exact source revision"
        );
        service.report["binary_version"] = json!(version);
        if let Some(variable) = configuration {
            let input = service.input(variable);
            fs::create_dir_all(service.config.parent().unwrap()).unwrap();
            fs::copy(&input, &service.config)
                .expect("copy the declared real qualification configuration");
        } else {
            service.cli(&["config", "init"]);
        }
        let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
            .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
        service.cli(&["config", "set", "dashboard.request_limits", &limits]);
        let mut child = service
            .command()
            .args(["serve", "--api", "--bind", "127.0.0.1", "--port", "0"])
            .stdout(Stdio::from(
                File::create(service.root.join("service.stdout")).unwrap(),
            ))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        service.report["service"] = json!({"arguments": ["serve", "--api", "--bind", "127.0.0.1", "--port", "0"], "pid": child.id()});
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
        self.binary_command(Path::new(env!("CARGO_BIN_EXE_stado")))
    }

    fn binary_command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(&self.root)
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("STADO_CONFIG", &self.config)
            .env("TMPDIR", &self.temporary)
            .stdin(Stdio::null());
        command
    }

    pub fn cli(&mut self, args: &[&str]) -> String {
        let output = self.execute(Path::new(env!("CARGO_BIN_EXE_stado")), args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn execute(&mut self, program: &Path, args: &[&str]) -> Output {
        self.execute_with_stdin(program, args, Stdio::null())
    }

    pub fn execute_with_stdin(&mut self, program: &Path, args: &[&str], input: Stdio) -> Output {
        let output = self
            .binary_command(program)
            .args(args)
            .stdin(input)
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "interface": "cli", "program": program, "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    pub fn observe(&mut self, name: &str, value: Value) {
        self.report["observations"][name] = value;
        self.save();
    }

    pub fn input(&mut self, name: &str) -> String {
        match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => {
                self.report["inputs"][name] = json!(value);
                self.save();
                value
            }
            _ => {
                self.report["outcome"] = json!("blocked");
                self.report["missing_prerequisite"] = json!(name);
                self.save();
                panic!("real installed-host journey requires {name}");
            }
        }
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
        self.observe(
            "final_controller_configuration",
            fs::read_to_string(&self.config).map_or_else(
                |error| json!({"read_error": error.to_string()}),
                |body| json!({"path": self.config, "body": body}),
            ),
        );
        if let Err(error) = fs::remove_dir_all(&self.home) {
            self.report["outcome"] = json!("failed");
            self.report["cleanup_errors"]["home"] =
                json!({"path": self.home, "error": error.to_string()});
        }
        if let Err(error) = fs::remove_dir_all(&self.temporary) {
            self.report["outcome"] = json!("failed");
            self.report["cleanup_errors"]["temporary"] =
                json!({"path": self.temporary, "error": error.to_string()});
        }
        self.save();
        eprintln!("native API evidence: {}", self.root.display());
    }
}

pub fn binary_digest(path: &Path) -> String {
    let mut binary = File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = binary.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    format!("{:x}", digest.finalize())
}
