//! Driving the real binary and the real agent, and waiting for what they do
//! rather than sleeping for a guess at how long it takes.

use std::fs::{self, File};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{Journey, TARGET};

impl Journey {
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false")
            // The stories read the cheap sections of `space report`; the
            // attribution walk is another test's subject.
            .env("STADO_INVENTORY_BUDGET_SECONDS", "0");
        command
    }

    pub(crate) fn invoke(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("_").replace('/', "_");
        let evidence = self.home.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join(format!("{name}.stdout")), &output.stdout).unwrap();
        fs::write(evidence.join(format!("{name}.stderr")), &output.stderr).unwrap();
        fs::write(
            evidence.join(format!("{name}.exit")),
            format!("{:?}", output.status.code()),
        )
        .unwrap();
        output
    }

    pub(crate) fn start_agent(&mut self) {
        let stdout = File::create(self.home.join("agent.out")).unwrap();
        let stderr = File::create(self.home.join("agent.err")).unwrap();
        self.agent = Some(
            self.command()
                .args(["agent", "--target", TARGET])
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(stderr))
                .spawn()
                .unwrap(),
        );
    }

    /// Start a real `stado capacity hold` and return the child; its stdout
    /// is the JSON receipt, retained beside the run.
    pub(crate) fn start_hold(&self, kind: &str, seconds: u64) -> Child {
        let stdout = File::create(self.home.join("hold.out")).unwrap();
        let stderr = File::create(self.home.join("hold.err")).unwrap();
        self.command()
            .args([
                "capacity",
                "hold",
                "--kind",
                kind,
                "--target",
                TARGET,
                "--seconds",
                &seconds.to_string(),
                "--json",
            ])
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap()
    }

    pub(crate) fn wait_for(
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
            if let Some(agent) = self.agent.as_mut() {
                if let Some(status) = agent.try_wait().unwrap() {
                    panic!(
                        "agent exited before {description}: {status}\nstdout:\n{}\nstderr:\n{}",
                        fs::read_to_string(self.home.join("agent.out")).unwrap_or_default(),
                        fs::read_to_string(self.home.join("agent.err")).unwrap_or_default(),
                    );
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "timed out waiting for {description}\nstdout:\n{}\nstderr:\n{}",
            fs::read_to_string(self.home.join("agent.out")).unwrap_or_default(),
            fs::read_to_string(self.home.join("agent.err")).unwrap_or_default(),
        );
    }

}
