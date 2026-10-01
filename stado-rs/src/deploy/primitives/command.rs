//! One planned subprocess, its captured result, and the injectable runner
//! seam every deploy module executes through.

use std::future::Future;
use std::sync::Arc;

use futures::future::BoxFuture;

use super::process::run_process;

/// One planned external command: full argv (program first) and an optional
/// stdin payload. The command runs until it exits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
}

impl CommandSpec {
    /// A command with no stdin payload.
    pub fn new(argv: Vec<String>) -> Self {
        Self { argv, stdin: None }
    }
}

/// Captured result of a finished command (Python `CompletedProcess` with
/// `capture_output=True, text=True`): exit code plus decoded stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// Python `returncode == 0`.
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Python `r.stderr or r.stdout` — the error detail preferred by the
    /// deploy modules' failure messages.
    pub fn detail(&self) -> &str {
        if !self.stderr.is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

/// Injectable command-runner seam (Python's `runner=` parameter in
/// `host_users.provision_users`, generalized to every deploy subprocess).
/// `Err` is a spawn or pipe failure.
pub type Runner =
    Arc<dyn Fn(CommandSpec) -> BoxFuture<'static, Result<CommandOutput, String>> + Send + Sync>;

/// Wrap a closure as a [`Runner`].
pub fn runner_fn<F, Fut>(f: F) -> Runner
where
    F: Fn(CommandSpec) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<CommandOutput, String>> + Send + 'static,
{
    Arc::new(move |spec| Box::pin(f(spec)))
}

/// The production runner: every command through `tokio::process::Command`.
pub fn production_runner() -> Runner {
    runner_fn(run_process)
}
