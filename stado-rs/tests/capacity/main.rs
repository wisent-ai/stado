//! Real local-worker live-capacity journey.
//!
//! The built Stado binary submits two blocking CPU jobs to an isolated store,
//! then a real worker admits both before either can finish. The fixture leaves
//! the removed registry and environment slot caps at one: observing both jobs
//! running at once proves those values no longer control admission. The same
//! journey checks the worker's public capacity document and both terminal job
//! records rather than treating process output as the result.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const TARGET: &str = "capacity-runner";

struct Journey {
    home: tempfile::TempDir,
    storage: PathBuf,
    agent: Option<Child>,
}

impl Journey {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/capacity-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("capacity-")
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
                "name": TARGET,
                "kind": "local",
                "ssh": "nobody@127.0.0.1",
                "hostnames": [hostname],
                "release_platform": build_platform(),
                "slots": 1,
                "max_concurrent": 1,
                "env_overrides": {"WC_LOCAL_SLOTS": "1"},
                "disk_cleanup": {
                    "mode": "off",
                    "check_interval_seconds": 300,
                    "low_free_gb": 1,
                    "target_free_gb": 2,
                    "max_bytes_per_pass": 1073741824_u64,
                    "max_items_per_pass": 10,
                    "max_scan_items": 100,
                    "cleaners": {}
                }
            }],
            "coordinators": []
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
            .env("WC_LOCAL_SLOTS", "1")
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

    fn submit_blocked(&self, name: &str) -> String {
        let started = self.home.path().join(format!("{name}.started"));
        let release = self.home.path().join("release");
        let finished = self.home.path().join(format!("{name}.finished"));
        let workload = format!(
            ": > '{}'; while [ ! -f '{}' ]; do /bin/sleep 0.1; done; : > '{}'",
            started.display(),
            release.display(),
            finished.display(),
        );
        let run_id = format!("capacity-{name}");
        let output = self.invoke_ok(&[
            "submit",
            &workload,
            "--provider",
            "local",
            "--pin-provider",
            "--pinned-host",
            TARGET,
            "--run-id",
            &run_id,
        ]);
        let receipt = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<Value>(line).ok())
            .expect("submit prints its JSON receipt as the final line");
        receipt["jobs"][0]["job_id"]
            .as_str()
            .expect("receipt carries one job id")
            .to_string()
    }

    fn start_agent(&mut self) {
        let stdout = File::create(self.home.path().join("agent.out")).unwrap();
        let stderr = File::create(self.home.path().join("agent.err")).unwrap();
        self.agent = Some(
            self.command()
                .args(["agent", "--target", TARGET])
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
    }

    fn wait_for(
        &mut self,
        description: &str,
        timeout: Duration,
        predicate: impl Fn(&Self) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(self) {
                return;
            }
            if let Some(status) = self.agent.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "agent exited before {description}: {status}\nstdout:\n{}\nstderr:\n{}",
                    fs::read_to_string(self.home.path().join("agent.out")).unwrap_or_default(),
                    fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default(),
                );
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "timed out waiting for {description}\nstdout:\n{}\nstderr:\n{}",
            fs::read_to_string(self.home.path().join("agent.out")).unwrap_or_default(),
            fs::read_to_string(self.home.path().join("agent.err")).unwrap_or_default(),
        );
    }

    fn newest_capacity(&self) -> Option<Value> {
        let directory = self.storage.join("capacity");
        let entry = fs::read_dir(directory).ok()?.find_map(Result::ok)?;
        serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()
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

fn build_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => panic!("capacity journey has no platform mapping for {os}-{arch}"),
    }
}

#[test]
#[ignore = "Probierz records the real capacity-config cutover journey"]
fn registry_policy_rewrite_removes_legacy_fixed_capacity_declarations() {
    let journey = Journey::new();
    journey.invoke_ok(&["host", "disk-cleanup", TARGET, "--mode", "off", "--json"]);

    let registry: Value =
        serde_json::from_slice(&fs::read(journey.storage.join("registry.json")).unwrap()).unwrap();
    let target = &registry["targets"][0];
    assert!(target.get("slots").is_none());
    assert!(target.get("max_concurrent").is_none());
    assert!(
        target["env_overrides"].get("WC_LOCAL_SLOTS").is_none(),
        "a policy write must not retain the retired environment cap"
    );
    assert_eq!(target["disk_cleanup"]["mode"], "off");
}

#[test]
#[ignore = "Probierz records the real local-worker capacity journey"]
fn live_resources_admit_two_jobs_despite_legacy_single_slot_limits() {
    let mut journey = Journey::new();
    let first = journey.submit_blocked("first");
    let second = journey.submit_blocked("second");
    journey.start_agent();

    journey.wait_for(
        "both workloads to be running",
        Duration::from_secs(45),
        |state| {
            state.home.path().join("first.started").exists()
                && state.home.path().join("second.started").exists()
                && !state.home.path().join("release").exists()
        },
    );
    journey.wait_for(
        "a capacity publication describing both running jobs",
        Duration::from_secs(30),
        |state| {
            state.newest_capacity().is_some_and(|capacity| {
                capacity["running_jobs"]
                    .as_i64()
                    .is_some_and(|count| count >= 2)
                    && capacity["total_cpu_cores"]
                        .as_i64()
                        .is_some_and(|count| count >= 2)
                    && capacity.get("available_cpu_cores").is_some()
                    && capacity.get("free_ram_gb").is_some()
                    && capacity.get("available_accelerators").is_some()
                    && capacity.get("accepting_jobs").is_some()
                    && capacity.get("free_slots").is_none()
            })
        },
    );

    fs::write(journey.home.path().join("release"), b"go\n").unwrap();
    journey.wait_for("both jobs to finish", Duration::from_secs(45), |state| {
        state
            .storage
            .join("completed")
            .join(format!("{first}.json"))
            .exists()
            && state
                .storage
                .join("completed")
                .join(format!("{second}.json"))
                .exists()
            && state.home.path().join("first.finished").exists()
            && state.home.path().join("second.finished").exists()
    });
}
