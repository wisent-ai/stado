//! The agent, the job it claims, and the capacity documents it publishes.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::fixture::{Journey, TARGET};

/// How long a journey waits for the agent to reach a state before it gives up
/// and prints the agent's own log.
const PATIENCE: Duration = Duration::from_secs(240);

/// How often the published capacity document is read while waiting. Short
/// against the agent's own poll interval, so no publication is missed.
const SAMPLE: Duration = Duration::from_millis(200);

/// One publication as this test saw it: the stamp the product wrote, and the
/// instant it appeared.
pub struct Publication {
    pub published_at: String,
    pub seen: Instant,
}

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

    /// Sample the published capacity document until `settled` holds, and
    /// return every distinct publication seen with the moment it appeared.
    ///
    /// The stamps come from the product; the instants are this test's own
    /// clock, and only the gaps between them are used.
    pub fn watch_publications(
        &mut self,
        described: &str,
        settled: impl Fn(&Self) -> bool,
    ) -> Vec<Publication> {
        let mut seen: Vec<Publication> = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(published_at) = self
                .capacity()
                .and_then(|document| document["published_at"].as_str().map(str::to_string))
            {
                if seen.last().is_none_or(|last| last.published_at != published_at) {
                    seen.push(Publication {
                        published_at,
                        seen: Instant::now(),
                    });
                }
            }
            if settled(self) {
                return seen;
            }
            if let Some(agent) = self.agent.as_mut() {
                if let Ok(Some(status)) = agent.try_wait() {
                    panic!("the agent exited {status} before {described}\n{}", self.log());
                }
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {described}\n{}",
                self.log()
            );
            std::thread::sleep(SAMPLE);
        }
    }

    pub fn wait_for(&mut self, described: &str, ready: impl Fn(&Self) -> bool) {
        self.watch_publications(described, ready);
    }

    pub fn log(&self) -> String {
        format!(
            "agent stdout:\n{}\nagent stderr:\n{}",
            std::fs::read_to_string(self.home().join("agent.out")).unwrap_or_default(),
            std::fs::read_to_string(self.home().join("agent.err")).unwrap_or_default(),
        )
    }
}

/// The worst gap between consecutive publications.
pub fn worst_gap(publications: &[Publication]) -> Duration {
    publications
        .windows(2)
        .map(|pair| pair[1].seen.duration_since(pair[0].seen))
        .max()
        .expect("at least two publications")
}
