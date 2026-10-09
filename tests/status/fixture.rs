//! One isolated local Stado store under this checkout's build directory,
//! driven only through the built `stado` binary: a job is submitted,
//! cancelled and reaped the way the fleet does it, and every command, its
//! exit status, output and the lines it wrote on stderr are kept in the
//! case's `report.json` beside the exact source revision and binary digest.
//!
//! The store is `WC_STORAGE_BACKEND=local` with `WC_LOCAL_STORAGE_PATH`
//! under `CARGO_TARGET_TMPDIR`, and `STADO_CONFIG` names a file that does
//! not exist, so the operator's own configuration, registry and queue are
//! never read or written. A case that reads a declared real queue instead
//! takes that queue's configuration file from an environment variable
//! ([`Store::declared`]) and is blocked, not failed, when it is unset.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// How the store a case reads is reached.
enum Backend {
    /// An isolated local store the case owns.
    Local,
    /// The declared real queue a qualification configuration names.
    Declared(PathBuf),
}

pub struct Store {
    pub root: PathBuf,
    backend: Backend,
    report: Value,
}

impl Store {
    /// An isolated local store for `case`.
    pub fn new(case: &str) -> Self {
        let mut store = Self::prepare(case, Backend::Local);
        for directory in [".locks", ".metadata"] {
            fs::create_dir_all(store.store().join(directory))
                .expect("lay out the isolated local store");
        }
        store.identify();
        store
    }

    /// The declared real queue the configuration file named by `variable`
    /// reaches. When the variable is unset the case is recorded as blocked
    /// and stops: a real read cannot be replaced by an easier one.
    #[allow(dead_code)]
    pub fn declared(case: &str, variable: &str) -> Self {
        let configuration = match std::env::var(variable) {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            _ => {
                let mut store = Self::prepare(case, Backend::Local);
                store.report["outcome"] = json!("blocked");
                store.report["missing_prerequisite"] = json!(variable);
                store.save();
                panic!("a real declared-queue journey requires {variable}");
            }
        };
        let mut store = Self::prepare(case, Backend::Declared(configuration.clone()));
        store.report["inputs"][variable] = json!(configuration);
        store.identify();
        store
    }

    fn prepare(case: &str, backend: Backend) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("reaped-reads");
        fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create the case directory");
        Self {
            root,
            backend,
            report: json!({"commands": [], "outcome": "failed"}),
        }
    }

    /// Bind the report to the exact source and binary under test.
    fn identify(&mut self) {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("the test package sits two levels below the checkout");
        let revision = Command::new("git")
            .current_dir(repository)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the checked-out revision");
        assert!(revision.status.success());
        let revision = String::from_utf8(revision.stdout)
            .expect("a revision is text")
            .trim()
            .to_string();
        let patch = Command::new("git")
            .current_dir(repository)
            .args(["diff", "--binary", "HEAD"])
            .output()
            .expect("git answers the working-tree diff");
        assert!(patch.status.success());
        fs::write(self.root.join("source.patch"), &patch.stdout).expect("keep the source patch");
        self.report["source_revision"] = json!(revision);
        self.report["source_patch_sha256"] = json!(format!("{:x}", Sha256::digest(&patch.stdout)));
        let binary = fs::read(env!("CARGO_BIN_EXE_stado")).expect("read the built stado");
        self.report["binary_sha256"] = json!(format!("{:x}", Sha256::digest(&binary)));
        let version = self.ok(&["--version"]);
        assert!(
            version
                .split_whitespace()
                .map(|part| part.trim_matches(['(', ')']))
                .any(|part| part.trim_end_matches("-dirty") == revision),
            "the tested executable must identify the exact source revision: {version}"
        );
        self.report["binary_version"] = json!(version.trim());
        self.save();
    }

    pub fn store(&self) -> PathBuf {
        self.root.join("store")
    }

    /// The real stado with this case's store and nothing of the operator's.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .current_dir(&self.root)
            .stdin(Stdio::null());
        fs::create_dir_all(self.root.join("home")).expect("create the case home");
        fs::create_dir_all(self.root.join("tmp")).expect("create the case tmp");
        match &self.backend {
            Backend::Local => {
                command
                    .env("WC_STORAGE_BACKEND", "local")
                    .env("WC_LOCAL_STORAGE_PATH", self.store())
                    .env("STADO_CONFIG", self.root.join("no-config.json"));
            }
            Backend::Declared(configuration) => {
                command.env("STADO_CONFIG", configuration);
            }
        }
        command
    }

    /// Run one command to its end and keep what it did.
    pub fn run(&mut self, args: &[&str]) -> Output {
        let output = self.command(args).output().expect("the real stado runs");
        self.report["commands"].as_array_mut().expect("commands").push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    /// Run one command that must succeed; its stdout.
    pub fn ok(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stado prints text")
    }

    /// Submit one local job under `run_id`; its job id, as submit prints it.
    pub fn submit(&mut self, run_id: &str, command: &str) -> String {
        let printed = self.ok(&["submit", "--run-id", run_id, "--provider", "local", command]);
        printed
            .lines()
            .find_map(|line| line.strip_prefix("Job ID: "))
            .map(str::trim)
            .map(str::to_string)
            .expect("submit prints the job id")
    }

    /// Cancel a queued job.
    pub fn cancel(&mut self, job_id: &str) {
        let printed = self.ok(&["cancel", job_id]);
        assert!(printed.contains(&format!("Cancelled {job_id}")), "{printed}");
    }

    /// Run the local control plane until its run reaper has reaped `runs`
    /// runs: the pass that retains every terminal job in its run manifest,
    /// indexes it under `runs/jobs/<job id>` and deletes the job's own
    /// documents. The lines the control plane wrote are kept in the report.
    pub fn reap(&mut self, runs: usize, interval_seconds: &str) {
        let mut child = self
            .command(&[
                "serve",
                "--control-plane",
                "local",
                "--control-plane-interval-seconds",
                interval_seconds,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the local control plane");
        let mut stderr = BufReader::new(child.stderr.take().expect("the control plane's stderr"));
        let mut said = Vec::new();
        let mut reaped: Vec<usize> = Vec::new();
        while reaped.iter().sum::<usize>() < runs {
            let mut line = String::new();
            stderr.read_line(&mut line).expect("read the control plane's stderr");
            assert!(!line.is_empty(), "the control plane ended before reaping: {said:?}");
            if let Some(count) = line
                .split_once("run-reaper: reaped ")
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .and_then(|count| count.parse::<usize>().ok())
            {
                reaped.push(count);
            }
            if line.contains("tick failed") || line.contains("cleanup degraded") {
                child.kill().expect("stop the control plane");
                panic!("the control plane failed: {line}");
            }
            said.push(line);
        }
        child.kill().expect("stop the control plane");
        child.wait().expect("the control plane ends");
        self.report["control_plane"] = json!({
            "reaped_runs": reaped.iter().sum::<usize>(),
            "stderr": said.concat(),
        });
        self.save();
    }

    pub fn observe(&mut self, name: &str, value: Value) {
        self.report["observations"][name] = value;
        self.save();
    }

    pub fn pass(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).expect("the report serializes"),
        )
        .expect("write the report");
    }
}

impl Drop for Store {
    /// The store is removed; the report and the lines it quotes stay.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.store());
        let _ = fs::remove_dir_all(self.root.join("home"));
        let _ = fs::remove_dir_all(self.root.join("tmp"));
    }
}

/// The value of one `name: value` field of a wait line.
#[allow(dead_code)]
pub fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split("; ")
        .find_map(|part| part.strip_prefix(&format!("{name}: ")))
}

/// The run manifests a command read, by the lock it took on each: every
/// `czekam` line for the local store's `runs/<run id>.json`.
#[allow(dead_code)]
pub fn manifest_reads(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("czekam: exclusive lock on the local store's runs/"))
        .filter_map(|rest| rest.split(';').next())
        .filter(|name| name.ends_with(".json"))
        .map(str::to_string)
        .collect()
}
