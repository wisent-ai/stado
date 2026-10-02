//! The host's one Stado process publishes the host's health beacon into the
//! fleet store its queue roles use — the store `stado registry beacon-age`
//! and `stado service list` read. One isolated deployment: `config init`
//! seeds a local registry naming this machine, `stado serve --api
//! --api-local-store … --health-interval-seconds 1` runs as the real product,
//! and the beacon is read back through `stado registry beacon-age` and from
//! the object that reader names.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

struct Deployment {
    root: PathBuf,
    home: PathBuf,
    report: Value,
    child: Option<Child>,
    logs: Vec<JoinHandle<()>>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        // The process binds `$HOME/.stado/release-proxy.sock`, and a Unix
        // socket path is at most 104 bytes on macOS, so the deployment's
        // HOME is the run directory itself under a short name.
        let root = repository
            .join(".build/h")
            .join(&uuid::Uuid::new_v4().simple().to_string()[..6]);
        let home = root.clone();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(root.join("tmp")).unwrap();
        let mut deployment = Self {
            root,
            home,
            report: json!({"commands": [], "outcome": "failed"}),
            child: None,
            logs: Vec::new(),
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
        fs::write(deployment.root.join("source.patch"), &patch.stdout).unwrap();
        deployment.report["source_revision"] =
            json!(String::from_utf8(revision.stdout).unwrap().trim());
        deployment.report["source_patch_sha256"] =
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
        deployment.report["binary_sha256"] = json!(format!("{:x}", digest.finalize()));
        let version = deployment.cli(&["--version"]);
        // `rev <sha>` or `rev <sha>-dirty`; the working-tree patch and its
        // digest are kept beside the report either way.
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .map(|part| part.strip_suffix("-dirty").unwrap_or(part))
                .any(|part| Some(part) == deployment.report["source_revision"].as_str()),
            "the tested executable must identify the exact source revision: {version}"
        );
        deployment.report["binary_version"] = json!(version);
        deployment.cli(&["config", "init"]);
        deployment.save();
        deployment
    }

    fn store(&self) -> PathBuf {
        self.home.join(".stado").join("local-storage")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    fn cli(&mut self, args: &[&str]) -> String {
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

    /// Run the host process with the health role on a one-second cadence and
    /// return the host slug it announced after its first publication.
    fn serve(&mut self) -> String {
        let store = self.store();
        let args = vec![
            "serve".to_string(),
            "--api".to_string(),
            "--bind".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            "0".to_string(),
            "--api-local-store".to_string(),
            store.to_string_lossy().into_owned(),
            "--health-interval-seconds".to_string(),
            "1".to_string(),
        ];
        let mut child = self
            .command()
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        self.report["service"] = json!({"arguments": args, "pid": child.id()});
        let (sender, published) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let stdout_path = self.root.join("service.stdout");
        self.logs.push(std::thread::spawn(move || {
            let mut file = File::create(stdout_path).unwrap();
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                writeln!(file, "{line}").unwrap();
                file.flush().unwrap();
                let _ = sender.send(line);
            }
        }));
        let stderr = child.stderr.take().unwrap();
        let stderr_path = self.root.join("service.stderr");
        self.logs.push(std::thread::spawn(move || {
            let mut file = File::create(stderr_path).unwrap();
            for line in BufReader::new(stderr).lines() {
                writeln!(file, "{}", line.unwrap()).unwrap();
                file.flush().unwrap();
            }
        }));
        self.child = Some(child);
        self.save();
        // The health role prints the host slug once the beacon is stored;
        // the first such line is the first publication.
        let slug = published
            .recv()
            .expect("the host process must announce a stored beacon; inspect service.stderr");
        self.report["first_publication"] = json!(slug);
        self.save();
        slug
    }

    fn pass(&mut self) {
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

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            match child.wait() {
                Ok(status) => self.report["service"]["exit_status"] = json!(status.to_string()),
                Err(error) => self.report["service"]["wait_error"] = json!(error.to_string()),
            }
        }
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        self.save();
        eprintln!("host-health evidence: {}", self.root.display());
    }
}

#[test]
fn the_host_process_publishes_its_beacon_where_the_fleet_reads_it() {
    let mut deployment = Deployment::start();
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let declared = registry["targets"][0]["name"].as_str().unwrap().to_string();
    assert!(
        !deployment.store().join("host_health").exists(),
        "a fresh deployment must hold no beacon before the process runs"
    );

    let slug = deployment.serve();
    let ages = deployment.cli(&["registry", "beacon-age", "--json"]);
    let ages: Value = serde_json::from_str(&ages).unwrap();
    let row = ages["hosts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["host"] == declared)
        .cloned()
        .unwrap_or_else(|| panic!("beacon-age must list {declared}: {ages}"));
    assert_eq!(row["status"], "reported", "{row}");
    assert!(
        row["age_seconds"].as_i64().is_some_and(|age| age >= 0),
        "beacon-age must read the stored beacon: {row}"
    );
    let beacon_path = deployment.store().join(
        row["beacon"]
            .as_str()
            .expect("beacon-age names the beacon object"),
    );
    assert_eq!(
        beacon_path.file_name().unwrap().to_string_lossy(),
        format!("{slug}.json"),
        "the stored object is the one the process announced"
    );
    let beacon: Value = serde_json::from_slice(&fs::read(&beacon_path).unwrap()).unwrap();
    assert_eq!(beacon["host"], slug);
    assert!(beacon["reported_at"].is_string(), "{beacon}");
    assert!(beacon["units"].is_object(), "{beacon}");
    assert!(
        beacon.get("link").is_some(),
        "a beacon about this machine carries its link block: {beacon}"
    );
    deployment.report["beacon"] = beacon;
    deployment.report["beacon_age"] = row;
    deployment.pass();
}
