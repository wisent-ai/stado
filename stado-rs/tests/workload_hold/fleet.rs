//! The queue half of the journey: a submitted job, the agent that claims it,
//! and the records the product writes about it.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fixture::{Journey, TARGET};

/// Every prefix a terminal job record can be written to.
pub const TERMINAL_PREFIXES: [&str; 4] = ["completed", "uploaded", "failed", "cancelled"];

/// How long a journey waits for the agent to reach a state before it gives up
/// and prints the agent's own log. Generous against a debug build claiming its
/// first job, which took ten seconds when this was measured.
const PATIENCE: Duration = Duration::from_secs(180);

impl Journey {
    /// Submit one shell command pinned to this host; returns its job id.
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
        let out = std::fs::File::create(self.home().join("agent.out")).expect("agent log");
        let err = std::fs::File::create(self.home().join("agent.err")).expect("agent log");
        self.agent = Some(
            self.command()
                .args(["agent", "--target", TARGET])
                .stdout(Stdio::from(out))
                .stderr(Stdio::from(err))
                .spawn()
                .expect("the agent started"),
        );
    }

    pub fn stop_agent(&mut self) {
        if let Some(mut agent) = self.agent.take() {
            let _ = agent.kill();
            let _ = agent.wait();
        }
    }

    /// Take the write bit off every terminal prefix, so the agent's
    /// finalization keeps failing and its slot stays retained.
    pub fn refuse_terminal_records(&self, refused: bool) {
        use std::os::unix::fs::PermissionsExt;
        let mode = if refused { 0o500 } else { 0o700 };
        for prefix in TERMINAL_PREFIXES {
            let path = self.storage.join(prefix);
            std::fs::create_dir_all(&path).expect("the terminal prefix");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
                .expect("the terminal prefix's mode is ours to set");
        }
    }

    pub fn recorded(&self, prefix: &str, job: &str) -> bool {
        self.storage
            .join(prefix)
            .join(format!("{job}.json"))
            .is_file()
    }

    /// Whether the store holds a terminal record for `job` under any prefix.
    pub fn terminal(&self, job: &str) -> bool {
        TERMINAL_PREFIXES
            .iter()
            .any(|prefix| self.recorded(prefix, job))
    }

    pub fn wait_for(&mut self, described: &str, ready: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            if ready(self) {
                return;
            }
            if let Some(agent) = self.agent.as_mut() {
                if let Ok(Some(status)) = agent.try_wait() {
                    panic!(
                        "the agent exited {status} before {described}\n{}",
                        self.log()
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("timed out waiting for {described}\n{}", self.log());
    }

    /// Poll the enforcing pass until it stops answering `lock_busy`, and
    /// return how long that took together with the report that ran.
    pub fn wait_for_the_lock(&self) -> (Duration, Value) {
        let started = Instant::now();
        loop {
            let report = self.reclaim();
            if report["lock_busy"] != Value::Bool(true) {
                return (started.elapsed(), report);
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the run lock was still held {:?} after the workload's process was gone\n{}",
                started.elapsed(),
                self.log()
            );
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    pub fn log(&self) -> String {
        format!(
            "agent stdout:\n{}\nagent stderr:\n{}",
            std::fs::read_to_string(self.home().join("agent.out")).unwrap_or_default(),
            std::fs::read_to_string(self.home().join("agent.err")).unwrap_or_default(),
        )
    }
}
