//! Real `stado builds` recipe → poller → worker → artifact journey.
//!
//! The built Stado binary writes the recipe to an isolated canonical registry,
//! a real coordinator observes the public repository branch, and a real Stado
//! worker claims the platform-constrained job. The assertion reads the uploaded
//! artifact and the reconciled recipe state; no scheduler, Git, worker, or
//! storage stand-in is used.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const RECIPE: &str = "probierz-native-build";
fn build_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("build journey has no platform mapping for {os}-{arch}"),
    }
}
const SOURCE: &str = "https://github.com/wisent-ai/stado.git";

struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    agent: Option<Child>,
}

impl Journey {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/build-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(root)
            .unwrap();
        let storage = home.path().join("store");
        fs::create_dir_all(&storage).unwrap();
        let hostname =
            String::from_utf8(Command::new("hostname").arg("-f").output().unwrap().stdout)
                .unwrap()
                .trim()
                .to_ascii_lowercase();
        let registry = json!({
            "schema_version": 2,
            "targets": [{
                "name": "build-runner",
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "release_platform": build_platform(),
                "hostnames": [hostname],
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 10,
                    "target_free_gb": 12,
                    "max_bytes_per_pass": 53687091200_u64,
                    "max_items_per_pass": 50,
                    "max_scan_items": 10000,
                    "cleaners": {}
                }
            }],
            "coordinators": [{
                "name": "build-coordinator",
                "runtime": "cron",
                "interval_seconds": 60,
                "active": true
            }]
        });
        fs::write(
            storage.join("registry.json"),
            serde_json::to_string_pretty(&registry).unwrap(),
        )
        .unwrap();
        Self {
            home,
            storage,
            agent: None,
        }
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
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    fn invoke(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn invoke_ok(&self, args: &[&str]) -> Output {
        let output = self.invoke(args);
        assert!(
            output.status.success(),
            "stado {} failed\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    fn start_agent(&mut self) {
        let stdout = File::create(self.home.path().join("agent.out")).unwrap();
        let stderr = File::create(self.home.path().join("agent.err")).unwrap();
        self.agent = Some(
            self.command()
                .args(["agent", "--target", "build-runner"])
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(entries) = fs::read_dir(self.storage.join("capacity")) {
                for entry in entries.flatten() {
                    let Ok(bytes) = fs::read(entry.path()) else {
                        continue;
                    };
                    if serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .is_some_and(|capacity| capacity["accepting_jobs"] == true)
                    {
                        return;
                    }
                }
            }
            if self.agent.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "agent did not accept work: {}",
            fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default()
        );
    }

    fn status(&self) -> Value {
        let output = self.invoke_ok(&["builds", "status", RECIPE, "--json"]);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn wait_for_terminal_job(&mut self, job_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(180);
        while Instant::now() < deadline {
            if ["completed", "uploaded", "failed"].iter().any(|prefix| {
                self.storage
                    .join(prefix)
                    .join(format!("{job_id}.json"))
                    .exists()
            }) {
                return;
            }
            if self.agent.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "build job {job_id} did not finish: {}",
            fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default()
        );
    }
}

impl Drop for Journey {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.as_mut() {
            let _ = agent.kill();
            let _ = agent.wait();
        }
    }
}

#[test]
#[ignore = "Probierz records the real public Git and Stado worker journey"]
fn build_recipe_polls_public_git_runs_on_matching_worker_and_publishes_artifact() {
    let platform = build_platform();
    let mut journey = Journey::new();

    let malformed = journey.invoke(&[
        "builds",
        "add",
        "--name",
        "bad-build",
        "--repo",
        "file:///not-public",
        "--branch",
        "main",
        "--command",
        "true",
        "--artifact",
        "out",
        "--platform",
        platform,
    ]);
    assert_eq!(malformed.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&malformed.stderr).contains("--repo must be an https:// clone URL")
    );

    let added = journey.invoke_ok(&[
        "builds",
        "add",
        "--name",
        RECIPE,
        "--repo",
        SOURCE,
        "--branch",
        "main",
        "--command",
        "printf 'built by stado\\n' > build-output.txt",
        "--artifact",
        "build-output.txt",
        "--platform",
        platform,
        "--interval-seconds",
        "1",
        "--json",
    ]);
    let added: Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added["enabled"], false);

    let duplicate = journey.invoke(&[
        "builds",
        "add",
        "--name",
        RECIPE,
        "--repo",
        SOURCE,
        "--branch",
        "main",
        "--command",
        "true",
        "--artifact",
        "out",
        "--platform",
        platform,
    ]);
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&duplicate.stderr)
        .contains("build recipe \"probierz-native-build\" already exists"));

    journey.invoke_ok(&["builds", "enable", RECIPE]);
    journey.start_agent();
    journey.invoke_ok(&["coordinator", "--once"]);
    let submitted = journey.status();
    let run = &submitted["recipe"]["runs"][platform];
    assert_eq!(run["status"], "running", "{submitted}");
    let job_id = run["job_id"].as_str().unwrap();

    journey.wait_for_terminal_job(job_id);
    journey.invoke_ok(&["coordinator", "--once"]);
    let completed = journey.status();
    let run = &completed["recipe"]["runs"][platform];
    assert_eq!(run["status"], "succeeded", "{completed}");
    assert_eq!(completed["job_states"][platform], "completed");
    assert_eq!(run["declared"], false);
    assert!(run["artifact_uris"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let destination = journey.home.path().join("results");
    journey.invoke_ok(&["results", job_id, destination.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(destination.join("build-output.txt")).unwrap(),
        "built by stado\n"
    );
    println!(
        "verified recipe={RECIPE}; job={job_id}; platform={platform}; artifact=build-output.txt"
    );
}
