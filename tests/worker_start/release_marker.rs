//! An idle worker asks the managed binary on disk what it is before it hands
//! off to the release its marker names, and says why when the binary will
//! not answer.
//!
//! The agent's release handoff once turned a managed binary that could not
//! be run, or answered no version, into a silent None and logged only that it
//! "would not state its version". The real stado runs `serve --worker
//! --standalone` from `~/.stado/bin/stado` (a copy of the build under test)
//! with `HOME` under Cargo's test directory and a local store of its own, so
//! the agent loop is the one an owner-managed binary runs:
//!
//! - with `~/.stado/bin/stado.release-version` naming a version this process
//!   is not, while the managed binary on disk is this very build, the loop
//!   says `release-marker-repaired` and the marker names the build's version;
//! - with the managed binary replaced by a file that cannot be run and the
//!   marker naming another version again, the loop says
//!   `release-marker-unverified` with the cause, `<binary> --version could
//!   not run: …`, and the worker keeps running instead of handing off.
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use serde_json::{json, Value};

const REPAIRED: &str = "loop: release-marker-repaired: ";
const UNVERIFIED: &str = "loop: release-marker-unverified: ";

struct Run {
    root: PathBuf,
    report: Value,
    worker: Option<Child>,
}

impl Run {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/worker-start-release-marker")
            .join(std::process::id().to_string());
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(root.join(".stado/bin")).expect("create the managed directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        std::fs::create_dir_all(root.join("waits")).expect("create the wait register");
        let revision = Command::new("git")
            .arg("-C")
            .arg(env!("CARGO_MANIFEST_DIR"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the checkout's revision");
        Self {
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "worker_log": [],
                "outcome": "failed",
            }),
            root,
            worker: None,
        }
    }

    fn managed(&self) -> PathBuf {
        self.root.join(".stado/bin/stado")
    }

    fn marker(&self) -> PathBuf {
        self.root.join(".stado/bin/stado.release-version")
    }

    /// The build under test placed where an owner-managed install puts it,
    /// and the version it states after its own name.
    fn install(&mut self) -> String {
        std::fs::copy(env!("CARGO_BIN_EXE_stado"), self.managed())
            .expect("copy the build under test to the managed path");
        let stated = Command::new(self.managed())
            .arg("--version")
            .output()
            .expect("the managed binary runs");
        assert!(stated.status.success(), "{stated:?}");
        let stated = String::from_utf8_lossy(&stated.stdout).into_owned();
        let mut words = stated.split_whitespace();
        let (Some(_name), Some(version)) = (words.next(), words.next()) else {
            panic!("stado --version stated no version after its name: {stated:?}");
        };
        self.report["version"] = json!(version);
        version.to_owned()
    }

    /// `~/.stado/bin/stado serve --worker --standalone` on a local store;
    /// its stderr lines arrive on the receiver.
    fn serve_worker(&mut self) -> mpsc::Receiver<String> {
        let mut child = Command::new(self.managed())
            .args(["serve", "--worker", "--standalone"])
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("STADO_WAITS_DIR", self.root.join("waits"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real worker from the managed path");
        let stderr = child.stderr.take().unwrap();
        self.worker = Some(child);
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let (line, readable) = match line {
                    Ok(line) => (line, true),
                    Err(error) => (
                        format!("the worker's standard error could not be read: {error}"),
                        false,
                    ),
                };
                if lines.send(line).is_err() || !readable {
                    return;
                }
            }
        });
        received
    }

    /// The first worker line that contains `wanted`.
    fn said(&mut self, lines: &mpsc::Receiver<String>, wanted: &str) -> String {
        loop {
            let Ok(line) = lines.recv() else {
                self.save();
                panic!(
                    "the worker stopped before saying {wanted:?}: {}",
                    self.report["worker_log"]
                );
            };
            self.report["worker_log"]
                .as_array_mut()
                .unwrap()
                .push(json!(line));
            if line.contains(wanted) {
                self.save();
                return line;
            }
        }
    }

    fn still_running(&mut self) -> bool {
        let worker = self.worker.as_mut().expect("the worker was started");
        worker
            .try_wait()
            .expect("the worker's state is readable")
            .is_none()
    }

    fn save(&self) {
        std::fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            let _ = worker.kill();
            let _ = worker.wait();
        }
        self.save();
    }
}

#[test]
fn the_release_marker_is_checked_against_the_managed_binary() {
    let mut run = Run::new();
    let version = run.install();
    let elsewhere = format!("{version}-not-installed");
    std::fs::write(run.marker(), format!("{elsewhere}\n")).unwrap();
    let lines = run.serve_worker();

    // The managed binary is this build: the marker was merely stale.
    let repaired = run.said(&lines, REPAIRED);
    assert!(
        repaired.contains(&elsewhere) && repaired.contains(&version),
        "the repair names what the marker said and what is installed: {repaired}"
    );
    let marker = std::fs::read_to_string(run.marker()).unwrap();
    assert_eq!(marker.trim(), version, "the marker was not corrected");

    // A managed file that cannot be run states no version, and says why.
    std::fs::remove_file(run.managed()).unwrap();
    std::fs::write(run.managed(), "not a program").unwrap();
    std::fs::write(run.marker(), format!("{elsewhere}\n")).unwrap();
    let unverified = run.said(&lines, UNVERIFIED);
    assert!(
        unverified.contains(&elsewhere),
        "the line names the marker's version: {unverified}"
    );
    assert!(
        unverified.contains(&format!(
            "{} --version could not run: ",
            run.managed().display()
        )),
        "the line names why the managed binary stated no version: {unverified}"
    );
    assert!(
        run.still_running(),
        "the worker handed off to a release it could not verify: {}",
        run.report["worker_log"]
    );
    run.report["outcome"] = json!("passed");
    run.save();
}
