//! The isolated host, its queue store, and the real agent that fills both.
//!
//! One registry target, and it IS this machine: its `hostnames` carry this
//! kernel's own host name lower-cased, so `stado disk-cleanup` resolves this
//! policy and `stado agent --target` claims for this host. The low watermark
//! is below any real disk — a host under disk pressure stops claiming, and
//! these cases need it to claim — so the enforcing pass is asked for with
//! `--to-target` against a target watermark above the disk, which is the
//! declared way to run one bounded enforcing pass on a healthy host.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The registry target's name; the machine is matched by `hostnames`.
pub const TARGET: &str = "janitor-runner";

/// Where the agent puts a job's tree, relative to `HOME`.
pub const WORK_ROOT: &str = ".stado/work/jobs";

/// How long a journey waits for the agent to reach a state before it gives up
/// and prints the agent's own log.
const PATIENCE: Duration = Duration::from_secs(120);

pub struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    agent: Option<Child>,
}

impl Journey {
    pub fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/janitor-keep-list-runs");
        std::fs::create_dir_all(&root).expect("the area's run root is ours to create");
        let home = tempfile::Builder::new()
            .prefix("keep-list-")
            .tempdir_in(root)
            .expect("a temporary home");
        let storage = home.path().join("store");
        std::fs::create_dir_all(&storage).expect("the store root");
        let journey = Self {
            home,
            storage,
            agent: None,
        };
        journey.declare();
        journey
    }

    /// The canonical registry: this machine, one enforcing policy, and
    /// `queue_workdirs` as its only cleaner. Every number is a registry field
    /// bounded by `targets::validation_disk` — the cleaner takes no age floor
    /// because a workdir is safe to remove when its job is terminal, not when
    /// it is old — and none of them tunes the product.
    fn declare(&self) {
        let document = json!({
            "schema_version": 2,
            "coordinators": [],
            "targets": [{
                "name": TARGET,
                "kind": "local",
                "release_platform": release_platform(),
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname()],
                "slots": 1,
                "max_concurrent": 1,
                "disk_cleanup": {
                    "mode": "enforce",
                    "check_interval_seconds": 60,
                    "low_free_gb": 1,
                    "target_free_gb": 1_000_000,
                    "max_bytes_per_pass": 1_073_741_824_i64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 1000,
                    "max_pass_seconds": 30,
                    "cleaners": {"queue_workdirs": {"min_age_seconds": 0}},
                },
            }],
        });
        std::fs::write(
            self.storage.join("registry.json"),
            serde_json::to_string_pretty(&document).expect("the registry serializes"),
        )
        .expect("the canonical registry is ours to write");
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    pub fn store(&self) -> &Path {
        &self.storage
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.path().join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_LOCAL_SLOTS", "1")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    pub fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("stado ran")
    }

    /// One bounded enforcing pass, and the disk report it printed. The command
    /// prints the disk pass and then the memory pass, so the disk report is
    /// the first line.
    pub fn reclaim(&self) -> Value {
        let args = ["disk-cleanup", "--once", "--to-target"];
        let output = self.invoke(&args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "stado {} exited {:?}\nstderr:\n{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8(output.stdout).expect("the report is utf-8");
        serde_json::from_str(stdout.lines().next().expect("a report line"))
            .expect("the disk report is one JSON document")
    }

    /// Submit one shell command as a job pinned to this host, and return the
    /// job id the receipt names.
    pub fn submit(&self, run_id: &str, command: &str) -> String {
        let output = self.invoke(&[
            "submit",
            command,
            "--provider",
            "local",
            "--pin-provider",
            "--pinned-host",
            TARGET,
            "--run-id",
            run_id,
        ]);
        assert!(
            output.status.success(),
            "submit refused:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let stdout = String::from_utf8(output.stdout).expect("the receipt is utf-8");
        let receipt: Value = stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str(line).ok())
            .expect("submit prints its JSON receipt");
        receipt["jobs"][0]["job_id"]
            .as_str()
            .expect("the receipt carries one job id")
            .to_string()
    }

    pub fn start_agent(&mut self) {
        let out = std::fs::File::create(self.home.path().join("agent.out")).expect("agent log");
        let err = std::fs::File::create(self.home.path().join("agent.err")).expect("agent log");
        self.agent = Some(
            self.command()
                .args(["agent", "--target", TARGET])
                .stdout(Stdio::from(out))
                .stderr(Stdio::from(err))
                .spawn()
                .expect("the agent started"),
        );
    }

    /// Stop the agent, so the janitor's run lock and every job tree are left
    /// exactly as the agent left them.
    pub fn stop_agent(&mut self) {
        if let Some(mut agent) = self.agent.take() {
            let _ = agent.kill();
            let _ = agent.wait();
        }
    }

    pub fn wait_for(&mut self, described: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if ready(self) {
                return;
            }
            if let Some(agent) = self.agent.as_mut() {
                if let Ok(Some(status)) = agent.try_wait() {
                    panic!("the agent exited {status} before {described}\n{}", self.log());
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("timed out waiting for {described}\n{}", self.log());
    }

    fn log(&self) -> String {
        format!(
            "agent stdout:\n{}\nagent stderr:\n{}",
            std::fs::read_to_string(self.home.path().join("agent.out")).unwrap_or_default(),
            std::fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default(),
        )
    }

    /// Whether the store holds a record for `job` under `prefix`.
    pub fn recorded(&self, prefix: &str, job: &str) -> bool {
        self.storage.join(prefix).join(format!("{job}.json")).is_file()
    }

    /// The tree the agent owns for one job.
    pub fn workdir(&self, job: &str) -> PathBuf {
        self.home.path().join(WORK_ROOT).join(format!("wc-{job}"))
    }

    /// One job tree written by this test rather than by a claim, for the
    /// populations no `stado submit` can produce.
    pub fn plant_workdir(&self, job: &str) -> PathBuf {
        let directory = self.workdir(job);
        std::fs::create_dir_all(directory.join("output")).expect("the job tree");
        std::fs::write(directory.join("output/payload.bin"), vec![0x5a; 4096])
            .expect("the job payload");
        directory
    }

    /// Write raw bytes as one job record under `prefix`.
    pub fn plant_record(&self, prefix: &str, job: &str, body: &str) {
        let directory = self.storage.join(prefix);
        std::fs::create_dir_all(&directory).expect("the store prefix");
        std::fs::write(directory.join(format!("{job}.json")), body).expect("the job record");
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        self.stop_agent();
    }
}

/// This machine's own host name, lower-cased because the registry refuses a
/// name it would have to normalize.
fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("/bin/hostname ran");
    String::from_utf8_lossy(&output.stdout).trim().to_lowercase()
}

fn release_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("this area has no release platform for {os}-{arch}"),
    }
}
