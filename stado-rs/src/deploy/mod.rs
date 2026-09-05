//! Deploy subsystem: operator-host provisioning.
//!
//! Port of `stado/deploy/` (`stado/deploy/__init__.py` is an empty package
//! marker — no runtime surface):
//!
//! - [`bootstrap`] — `stado bootstrap`: SSH-based remote provisioning of
//!   kind=local registry targets (pip install + inline systemd units).
//! - [`local_install`] — `stado bootstrap --local`: per-user launchd /
//!   systemd --user install on the current machine for the agent /
//!   coordinator / disk-cleanup / failure-fixer / watchdog kinds.
//! - [`host_recovery`] — `stado host recover`: fixed, narrow SSH recovery
//!   program for managed macOS hosts, with the tab-delimited `STADO_*`
//!   marker protocol ported byte-exactly.
//! - [`host_users`] — `stado host user create`: account creation on
//!   registry hosts over SSH; the password travels only on SSH stdin.
//!
//! The read-only `stado host ...` commands of `docs/missing-commands.md`
//! items two through six have NO Python original. They all ride one
//! channel, [`host_channel`], which is the option set and report shape of
//! [`host_reboot`] factored out:
//!
//! - [`host_uptime`] — `stado host uptime`: uptime, load averages, logins.
//! - [`host_ping`] — `stado host ping`: ssh reachability AND health-beacon
//!   age, combined into the worse of the two verdicts.
//! - [`host_disk`] — `stado host disk`: `df` plus the registry cleanup
//!   policy and the janitor's own recorded state.
//! - [`host_cleanup`] — `stado host cleanup --dry-run`: drives the host's
//!   own janitor in preview mode; contains no cleanup policy itself.
//! - [`host_exec`] — `stado host exec`: one command from a fixed
//!   allowlist, read-only apart from the declared provider sign-in
//!   repairs. Not a shell.
//! - [`host_inventory`] — `stado host inventory`: the stado-managed
//!   binaries, forward markers and loopback listeners of one host, plus
//!   the verdict on whether each marker still matches a live listener.
//!   It is NOT an `host_exec` allowlist entry because it reduces and caps
//!   every value it reads off the host; that table passes a program's
//!   output through untouched.
//! - [`host_object_relocate`] — `stado host object-relocate`: re-address
//!   objects from one key prefix to another INSIDE the store, on the host
//!   that holds it. The object API has no move and no server-side copy, so
//!   the alternative was pulling 134 MiB bodies through the control plane's
//!   loopback writer, which is what took that host's release ingress down.
//!   It previews by default and `--apply` verifies every destination before
//!   it unlinks a source.
//!
//! [`host_release`] is the one WRITE command in that group, and the only
//! thing in this crate that owns "get this build onto that host" — the gap
//! `ARCHITECTURE.md` names. It rides the same channel and follows Weles's
//! shipped auto-deploy order exactly: fetch the exact coordinate, verify it
//! against the operator's configured SHA-256, check the layout, stage it
//! under a versioned directory, and only then atomically repoint the active
//! binary and restart the declared unit. The three phases are three separate
//! programs on the channel, so "nothing activates before it verified" is
//! visible at the [`Runner`] seam rather than promised inside one script.
//!
//! [`host_link`] is not a command at all: it is the connectivity block a
//! host collects about ITSELF and publishes inside its health beacon, so
//! that "why did this machine go quiet" has an answer in the product
//! instead of in an operator's shell history.
//!
//! [`fleet_claim`] is not a command either: it is the fleet-level half of
//! [`host_gates`], reported wherever queued work is shown. `host gates`
//! answers "why is THIS host claiming nothing", one ssh round trip at a
//! time, which is only reachable by an operator who already suspects a
//! specific host. `fleet_claim` answers "can ANYTHING claim this queue" from
//! the store alone, in the same words, so `stado status` and `stado
//! overview` can state the one fact a queue listing cannot show: that a
//! queue with no claimant looks exactly like an empty one.
//!
//! Every subprocess is orchestrated through the [`Runner`] seam so tests
//! can inject a fake command runner and never spawn real
//! ssh/launchctl/systemctl. The production runner is
//! [`production_runner`] (tokio::process).
//!
//! `stado/deploy/templates/*.tmpl` (5 systemd units rendered by the
//! repo-root `install.sh` via sed) are NOT copied into the crate: the only
//! consumer is `install.sh`, which is not ported — `bootstrap.py` renders
//! its own inline units (see [`bootstrap`]).

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

pub mod artifact_install;
pub mod bootstrap;
pub mod fleet_claim;
pub mod fleet_vaults;
pub mod host_backup_audit;
pub mod host_build_caches;
pub mod host_capability;
pub mod host_channel;
pub mod host_cleanup;
pub mod host_cron;
pub mod host_disk;
pub mod host_exec;
pub mod host_gates;
pub mod host_gui_automation;
pub mod host_inventory;
pub mod host_link;
pub mod host_object_relocate;
pub mod host_ping;
pub mod host_precheck_runner;
pub mod host_reboot;
pub mod host_reclaim;
pub mod host_recovery;
pub mod host_recovery_release;
pub mod host_release;
pub mod host_resolver_key;
pub mod host_storage_reconcile;
pub mod host_uptime;
pub mod host_user_delete;
pub mod host_users;
pub mod inference;
pub mod inference_process;
pub mod inference_routes;
pub mod local_install;
pub mod mobile_runtime;
pub mod products;
pub mod reconcile;
pub mod service;
pub mod service_catalog;
pub mod service_env_file;
pub mod service_file_fetch;
pub mod service_label_print;
pub mod service_serving;
pub mod service_spawn_watch;
pub mod ssh_key;
pub mod staged_release;
pub mod stream;
pub mod weles_browser_runtime;
pub mod weles_browser_task;
pub mod weles_capture;

/// Deploy-layer failure carrying the exact Python exception message
/// (RuntimeError / ValueError / LookupError text). The CLI maps it to a
/// click-`ClickException`-style `Error: {msg}` on stderr, exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DeployError(pub String);

impl From<String> for DeployError {
    fn from(msg: String) -> Self {
        Self(msg)
    }
}

impl From<&str> for DeployError {
    fn from(msg: &str) -> Self {
        Self(msg.to_string())
    }
}

/// One planned external command: full argv (program first), an optional
/// stdin payload fed exactly as Python's `subprocess.run(input=...)`, and
/// an optional wall-clock timeout (Python `timeout=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Option<Duration>,
}

impl CommandSpec {
    /// A command with no stdin payload and no timeout.
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            stdin: None,
            timeout: None,
        }
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
/// `Err` mirrors an `OSError`/`SubprocessError` (spawn failure, timeout).
pub type Runner =
    Arc<dyn Fn(CommandSpec) -> BoxFuture<'static, Result<CommandOutput, String>> + Send + Sync>;

/// Wrap a closure as a [`Runner`].
pub fn runner_fn<F, Fut>(f: F) -> Runner
where
    F: Fn(CommandSpec) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<CommandOutput, String>> + Send + 'static,
{
    Arc::new(move |spec| Box::pin(f(spec)))
}

/// The production runner: every command through `tokio::process::Command`.
pub fn production_runner() -> Runner {
    runner_fn(run_process)
}

struct OwnedProcessGroup {
    child_id: Option<u32>,
    armed: bool,
}

impl OwnedProcessGroup {
    fn new(child_id: Option<u32>) -> Self {
        Self {
            child_id,
            armed: true,
        }
    }

    fn terminate(&mut self) -> Option<String> {
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::{killpg, Signal};
            use nix::unistd::Pid;

            let result = match self.child_id {
                Some(pid) => killpg(Pid::from_raw(pid as i32), Signal::SIGKILL),
                None => {
                    self.armed = false;
                    return Some("child PID unavailable".to_string());
                }
            };
            // One exact attempt owns this process-group identity. Never retry
            // from Drop after reaping, when the numeric id could be recycled.
            self.armed = false;
            match result {
                Ok(()) | Err(Errno::ESRCH) => None,
                Err(error) => Some(error.to_string()),
            }
        }
        #[cfg(not(unix))]
        {
            self.armed = false;
            None
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.terminate();
        }
    }
}

async fn run_process(spec: CommandSpec) -> Result<CommandOutput, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (program, args) = spec.argv.split_first().ok_or("empty command argv")?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Every invocation owns a fresh local process group. This includes a local
    // shell or ssh client and its local descendants; it does not assert that
    // processes beyond an ssh connection joined that group.
    #[cfg(unix)]
    command.process_group(0);
    if spec.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::null());
    }
    let mut child = command.spawn().map_err(|exc| exc.to_string())?;
    // Declared after `child` so cancellation drops this guard first and kills
    // the locally owned group while the leader handle still retains identity.
    let mut owned_group = OwnedProcessGroup::new(child.id());
    let stdin_payload = spec.stdin;
    let stdin_pipe = child.stdin.take();
    let mut stdout = child.stdout.take().ok_or("child stdout was not piped")?;
    let mut stderr = child.stderr.take().ok_or("child stderr was not piped")?;

    // Feed stdin and drain both output pipes concurrently, then reap the direct
    // child. The deadline covers the whole communication, including pipe EOF:
    // a descendant retaining stdout cannot outlive supervision indefinitely.
    let communication = async {
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let write_stdin = async move {
            if let (Some(mut pipe), Some(payload)) = (stdin_pipe, stdin_payload) {
                if let Err(error) = pipe.write_all(payload.as_bytes()).await {
                    if error.kind() != std::io::ErrorKind::BrokenPipe {
                        return Err(error);
                    }
                }
            }
            Ok(())
        };
        let read_stdout = stdout.read_to_end(&mut stdout_bytes);
        let read_stderr = stderr.read_to_end(&mut stderr_bytes);
        tokio::try_join!(write_stdin, read_stdout, read_stderr)?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, stdout_bytes, stderr_bytes))
    };
    let completed = match spec.timeout {
        Some(limit) => match tokio::time::timeout(limit, communication).await {
            Ok(result) => result.map_err(|error| error.to_string())?,
            Err(_) => {
                let group_kill_error = owned_group.terminate();
                if group_kill_error.is_some() {
                    let _ = child.start_kill();
                }
                let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
                let argv_repr = py_list_repr(&spec.argv);
                let detail = group_kill_error.map_or_else(String::new, |error| {
                    format!("; locally owned process-group kill failed: {error}")
                });
                return Err(format!(
                    "Command '{argv_repr}' timed out after {} seconds{detail}",
                    limit.as_secs()
                ));
            }
        },
        None => communication.await.map_err(|error| error.to_string())?,
    };
    owned_group.disarm();
    let (status, stdout, stderr) = completed;
    Ok(CommandOutput {
        code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

// ---------------------------------------------------------------------------
// Python-compatible quoting / repr helpers
// ---------------------------------------------------------------------------

/// Python `shlex.quote`: safe chars (`[a-zA-Z0-9_@%+=:,./-]`) pass through,
/// anything else is single-quoted with `'` escaped as `'"'"'`.
pub fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = |b: u8| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    };
    if value.bytes().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Python `repr()` of a (simple) string: single quotes by default, double
/// quotes when the value contains a single quote but no double quote;
/// backslash, the quote char, and `\n`/`\r`/`\t` are escaped.
pub fn py_str_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of a list of strings: `['a', 'b']`.
pub fn py_list_repr(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| py_str_repr(item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Python `str()` of a `dict[str, str]` (`{'K': 'V', ...}`), preserving the
/// given insertion order. Used by the `bootstrap --local --dry-run` env line.
pub fn py_dict_repr(items: &[(String, String)]) -> String {
    let pairs: Vec<String> = items
        .iter()
        .map(|(key, value)| format!("{}: {}", py_str_repr(key), py_str_repr(value)))
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

/// Write `content` to `path` only when it differs from what is already on
/// disk; returns true when the file was (re)written. The Python installers
/// rewrite unconditionally and declare idempotency at the "same resulting
/// state" level; skipping the byte-identical rewrite keeps mtime (and any
/// watching daemon) stable without changing the resulting state.
pub fn write_if_changed(path: &std::path::Path, content: &str) -> Result<bool, std::io::Error> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}
