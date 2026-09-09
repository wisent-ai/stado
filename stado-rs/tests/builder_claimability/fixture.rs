//! The isolated store, the product invocation and the readers the
//! builder-claim cases assert against.
//!
//! Isolation is environmental, as everywhere else in this suite:
//! `WC_STORAGE_BACKEND=local` plus `WC_LOCAL_STORAGE_PATH=<TempDir>`, a
//! set-but-missing `STADO_CONFIG`, `HOME` inside the temp dir, and every
//! Skarbiec URL pointed at a dead loopback port. Nothing here can reach the
//! operator's real queue store, vault, registry or fleet.
//!
//! The one declared host is this machine, so builder selection really takes
//! its current-host path: the capacity publication is written under the
//! consumer id a local agent on this machine publishes as, and the registry
//! target declares this machine's own kernel host name.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::document::{self, PRODUCT, VERSION};

/// How long a case waits for the product to write the claim record before it
/// calls the build unclaimed. Generous: the pipeline uploads the source
/// snapshot and writes the run document first.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// An isolated store holding the fleet document, and a committed source
    /// tree the release pipeline can snapshot.
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("an isolated root");
        let fixture = Self { root };
        std::fs::create_dir_all(fixture.home()).expect("create the isolated home");
        std::fs::create_dir_all(fixture.store()).expect("create the isolated store");
        std::fs::create_dir_all(fixture.source()).expect("create the source tree");
        std::fs::write(
            fixture.store().join("registry.json"),
            serde_json::to_vec_pretty(&document::registry(
                &fixture.home(),
                Path::new(env!("CARGO_BIN_EXE_stado")),
            ))
            .expect("the fleet document serialises"),
        )
        .expect("seed the isolated fleet document");
        std::fs::write(
            fixture.source().join(".wisent-release.json"),
            serde_json::to_vec_pretty(&document::manifest()).expect("the manifest serialises"),
        )
        .expect("write the product manifest");
        std::fs::write(fixture.source().join("VERSION"), format!("{VERSION}\n"))
            .expect("write the declared version");
        fixture.commit_source();
        fixture
    }

    pub fn home(&self) -> PathBuf {
        self.root.path().join("home")
    }

    pub fn store(&self) -> PathBuf {
        self.root.path().join("store")
    }

    pub fn source(&self) -> PathBuf {
        self.root.path().join("source")
    }

    /// The release pipeline refuses a source tree that is not a clean
    /// committed Git tree, so the fixture commits one.
    fn commit_source(&self) {
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(self.source())
                .args(args)
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "builder-claim"]);
        git(&["config", "user.email", "builder-claim@localhost"]);
        git(&["add", "."]);
        git(&["commit", "-qm", "builder claim probe"]);
    }

    /// One capacity publication, written where the queue store keeps them and
    /// read by builder selection through the store's own reader.
    pub fn publish(&self, accepting: Option<Value>, diag: Value) {
        let capacity = self.store().join("capacity");
        std::fs::create_dir_all(&capacity).expect("create the capacity prefix");
        std::fs::write(
            capacity.join(format!("{}.json", document::consumer())),
            serde_json::to_vec_pretty(&document::publication(accepting, diag))
                .expect("the publication serialises"),
        )
        .expect("publish capacity");
    }

    fn submit_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args([
                "release",
                "submit",
                "--source",
                &self.source().to_string_lossy(),
                "--version",
                VERSION,
                "--json",
            ])
            .env_clear()
            .env(
                "PATH",
                std::env::var("PATH").expect("the caller has a PATH"),
            )
            .env("HOME", self.home())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.store())
            .env("WC_STADO_STORAGE_NAMESPACE", "builder-claim")
            .env("STADO_CONFIG", self.home().join("no-such-config.json"))
            .env("WC_SKARBIEC_URL", "http://127.0.0.1:1")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    /// Run the submission to completion. Used by the cases whose subject is a
    /// refusal, which is reached before anything long-running.
    pub fn submit(&self) -> Output {
        self.submit_command()
            .output()
            .expect("the built stado binary runs")
    }

    /// Start the submission and leave it running. A submission that does find
    /// a builder goes on to wait for the build itself, so a case about the
    /// claim reads what the product wrote and then stops it.
    pub fn spawn_submit(&self) -> Child {
        self.submit_command()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the built stado binary starts")
    }

    /// The immutable build request the product wrote for this platform, once
    /// it exists: the record naming which builder was allowed to claim.
    pub fn build_request(&self) -> Option<Value> {
        let run = self.run_id()?;
        let path = self
            .store()
            .join("runs/release-pipeline")
            .join(PRODUCT)
            .join(run)
            .join("requests")
            .join(format!("{}.json", document::platform()));
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).expect("the build request is JSON")
    }

    /// Every job document sitting in the queue prefix of this store.
    pub fn queued_jobs(&self) -> Vec<Value> {
        let mut jobs = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.store().join("queue")) else {
            return jobs;
        };
        for entry in entries.flatten() {
            let Ok(bytes) = std::fs::read(entry.path()) else {
                continue;
            };
            if let Ok(job) = serde_json::from_slice(&bytes) {
                jobs.push(job);
            }
        }
        jobs
    }

    /// The run id the product minted for this submission, read off the store
    /// rather than derived: the identity is the product's to decide.
    fn run_id(&self) -> Option<String> {
        let runs = self.store().join("runs/release-pipeline").join(PRODUCT);
        std::fs::read_dir(runs)
            .ok()?
            .flatten()
            .find(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
    }

    /// The durable release-run document, which is where a submission records
    /// its state and, when it fails, the sentence it failed with.
    pub fn run_document(&self) -> Value {
        let run = self.run_id().expect("the submission wrote a release run");
        let path = self
            .store()
            .join("runs/release-pipeline")
            .join(run)
            .join("run.json");
        let bytes = std::fs::read(&path).expect("read the release run document");
        serde_json::from_slice(&bytes).expect("the release run document is JSON")
    }

    /// Wait until the product has written the claim record and queued the
    /// build, then stop the submission: what it does afterwards is the build
    /// itself, which this area is not about.
    pub fn claim(&self, submit: &mut Child) -> (Value, Value) {
        let deadline = Instant::now() + CLAIM_TIMEOUT;
        loop {
            let queued = self.queued_jobs();
            if let (Some(request), Some(job)) = (self.build_request(), queued.first()) {
                let job = job.clone();
                let _ = submit.kill();
                let _ = submit.wait();
                return (request, job);
            }
            if let Some(status) = submit.try_wait().expect("the submission is waitable") {
                panic!(
                    "the submission exited without claiming a builder: {status}\n{}",
                    self.run_document()
                );
            }
            assert!(
                Instant::now() < deadline,
                "no build was claimed within {} seconds",
                CLAIM_TIMEOUT.as_secs()
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
