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
            if fs::read_dir(self.storage.join("capacity"))
                .ok()
                .and_then(|mut entries| entries.next())
                .is_some()
            {
                return;
            }
            if self.agent.as_mut().unwrap().try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "agent published no capacity: {}",
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

/// The fleet's daily build ceiling, through the real CLI against an isolated
/// canonical registry: read it, declare it, spend it, and be refused.
///
/// On 2026-09-21 a session that could not run `stado release submit` declared
/// a recipe and ran it instead, and nothing counted those builds against the
/// workshop's three-a-day rule. The journey holds the count across separate
/// processes, which is the property that matters: a limit one command knows
/// about and the next one does not is not a limit.
#[test]
fn a_days_build_ceiling_is_counted_across_commands_and_refuses_the_next_build() {
    let platform = build_platform();
    let journey = Journey::new();

    let default_budget: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "budget", "--json"]).stdout).unwrap();
    assert_eq!(default_budget["limit"], 3, "the workshop's standing rule");
    assert_eq!(default_budget["used"], 0);

    journey.invoke_ok(&[
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
        "build-output.txt",
        "--platform",
        platform,
    ]);
    journey.invoke_ok(&["builds", "budget", "--limit", "1"]);

    journey.invoke_ok(&["builds", "run", "--run-id", "budget-one", RECIPE]);
    let spent: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "budget", "--json"]).stdout).unwrap();
    assert_eq!(spent["used"], 1, "the run was counted: {spent}");
    assert_eq!(spent["remaining"], 0);

    let refused = journey.invoke(&["builds", "run", "--run-id", "budget-two", RECIPE]);
    assert_ne!(refused.status.code(), Some(0), "a spent day refuses");
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("daily build budget is spent"), "{stderr}");
    assert!(stderr.contains("1 of 1"), "{stderr}");
    assert!(stderr.contains("stado builds budget --limit"), "{stderr}");

    let after: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "budget", "--json"]).stdout).unwrap();
    assert_eq!(after["used"], 1, "a refused build costs nothing: {after}");

    journey.invoke_ok(&["builds", "budget", "--limit", "2"]);
    let raised: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "budget", "--json"]).stdout).unwrap();
    assert_eq!(raised["limit"], 2);
    assert_eq!(raised["used"], 1, "raising the ceiling keeps the count");
    journey.invoke_ok(&["builds", "run", "--run-id", "budget-three", RECIPE]);
    println!("verified budget journey: recipe={RECIPE}; platform={platform}");
}

/// The window count reads the queue's own run manifests, and says so when
/// the day's counter disagrees with them.
///
/// `stado builds budget` reports the number the registry holds, written by
/// the submitters that maintain it. On 2026-09-21 it read `2 of 3 build
/// job(s) used` while this fleet had started 59 builds in twenty-four hours,
/// 47 of them release-pipeline builds that recorded nothing — so the ceiling
/// was true about its own counter and false about the machines. A second
/// reading, taken from the manifests the submitters cannot forget to write,
/// is what makes that visible.
#[test]
fn the_window_counts_builds_from_the_queues_own_manifests() {
    let journey = Journey::new();
    journey.invoke_ok(&[
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
        "build-output.txt",
        "--platform",
        build_platform(),
    ]);

    let empty: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "usage", "--json"]).stdout).unwrap();
    assert_eq!(
        empty["observed"]["total"], 0,
        "a fleet that built nothing counted something: {empty}"
    );

    journey.invoke_ok(&["builds", "run", "--run-id", "usage-one", RECIPE]);
    let counted: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "usage", "--json"]).stdout).unwrap();
    assert_eq!(
        counted["observed"]["total"], 1,
        "the manifest of the build that just started was not counted: {counted}"
    );
    assert_eq!(
        counted["observed"]["by_origin"]["build-manual"], 1,
        "the build was counted against the wrong asker: {counted}"
    );
    assert_eq!(
        counted["budget"]["used"], 1,
        "the day's counter and the window disagree about a recorded build: {counted}"
    );
    assert!(
        counted["unread"]
            .as_array()
            .is_some_and(|unread| unread.is_empty()),
        "a manifest could not be read: {counted}"
    );
}

/// The ceiling has to hold for a submission that never went through
/// `stado builds run`.
///
/// Until 2026-09-21 the count was asked by the three paths that knew they
/// were submitting a build — the poller, `builds run`, the release pipeline —
/// so a raw `stado submit` carrying a build command, a `stado job rerun` of a
/// build job, or a client older than the ceiling spent the fleet's day and
/// left the counter saying nothing had been spent. That is how six builds
/// went out against a ceiling of three. The charge now belongs to the
/// submission itself, so this asks the queue directly.
#[test]
fn a_build_submitted_outside_the_builds_command_is_counted_and_then_refused() {
    let journey = Journey::new();
    let compile = format!(
        "set -eu; cargo build --release; printf '%s' 0.0.1 > {}",
        "build-version.txt"
    );

    journey.invoke_ok(&["builds", "budget", "--limit", "1"]);
    journey.invoke_ok(&["submit", "--command", &compile]);

    let spent: Value =
        serde_json::from_slice(&journey.invoke_ok(&["builds", "budget", "--json"]).stdout).unwrap();
    assert_eq!(
        spent["used"], 1,
        "a build submitted straight to the queue was not counted: {spent}"
    );

    let refused = journey.invoke(&["submit", "--command", &compile]);
    assert_ne!(
        refused.status.code(),
        Some(0),
        "a spent day accepted another compile"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("daily build budget is spent"), "{stderr}");

    let plain = journey.invoke(&["submit", "--command", "printf hello"]);
    assert_eq!(
        plain.status.code(),
        Some(0),
        "a job that compiles nothing was refused by the build ceiling: {}",
        String::from_utf8_lossy(&plain.stderr)
    );
}

/// A queue nobody wants is emptied by one command.
///
/// On 2026-09-21 this fleet held 33 queued jobs that were no longer wanted
/// and the only route was `stado cancel <id>` thirty-three times, which is
/// how a queue stays full.
#[test]
fn the_whole_queue_is_cancelled_by_one_command() {
    let journey = Journey::new();
    journey.invoke_ok(&["submit", "--command", "printf one"]);
    journey.invoke_ok(&["submit", "--command", "printf two"]);

    let cancelled = journey.invoke_ok(&["cancel", "--queued"]);
    let said = String::from_utf8_lossy(&cancelled.stdout);
    assert!(said.contains("cancelled 2 queued job(s)"), "{said}");

    let status = String::from_utf8_lossy(&journey.invoke_ok(&["status"]).stdout).to_string();
    assert!(
        !status.contains("queued"),
        "the queue still holds work after cancelling it: {status}"
    );

    let again = journey.invoke_ok(&["cancel", "--queued"]);
    assert!(
        String::from_utf8_lossy(&again.stdout).contains("cancelled 0 queued job(s)"),
        "cancelling an empty queue is not an error"
    );
}
